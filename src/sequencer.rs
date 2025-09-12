// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! MIDI sequencer implementation that takes care of timing and generating MIDI messages.

use crate::Result;
use crate::midi::*;
use crate::units::*;
use crate::vm::*;
use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::rc::Rc;
use std::{thread, time};

/// Compute the Euclidean Sequence with `pulses`, for the given `len` and `offset`. Also see:
/// https://cgm.cs.mcgill.ca/~godfried/publications/banff.pdf
pub fn euclidean_sequence(pulses: u32, len: u32, offset: u32) -> Vec<bool> {
    assert!(pulses <= len);
    assert!(len == 0 || offset < len);

    let mut sequence: Vec<Vec<bool>> = (0..len)
        .map(|i| if i < pulses { vec![true] } else { vec![false] })
        .collect();

    let mut d = (len - pulses) as isize;
    let mut n = cmp::max(pulses as isize, d);
    let mut k = cmp::min(pulses as isize, d);
    let mut z = d;

    while z > 0 || k > 1 {
        for i in 0..(k as usize) {
            let mut extension = sequence[sequence.len() - 1 - i].clone();
            sequence[i].append(&mut extension);
        }
        sequence.truncate(sequence.len() - (k as usize));
        z -= k;
        d = n - k;
        n = cmp::max(k, d);
        k = cmp::min(k, d);
    }

    sequence
        .into_iter()
        .flatten()
        .cycle()
        .skip(offset as usize)
        .take(len as usize)
        .collect()
}

// === Clips ===================================================================

/// MIDI sequencer interface.
#[derive(Clone, Debug, PartialEq)]
pub enum SeqCall {
    QueueNote(u8, Note, Velocity, Duration),
    QueueSequence(u8, Vec<Note>, Velocity, Duration, u32, u32, u32),
    QueueControl(u8, u8, u8),
}

/// ClipInst is a single instruction of a Clip. It implements convenient conversions to and from
/// a simple DSL which may be used to store instructions for a clip interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipInst {
    Tick(Duration),
    Jmp(i32),
    Call(SeqCall),
}

impl std::str::FromStr for ClipInst {
    type Err = String;
    fn from_str(s: &str) -> Result<Self> {
        if let Some(suffix) = s.strip_prefix("+ ") {
            Ok(ClipInst::Tick(suffix.parse()?))
        } else if let Some(suffix) = s.strip_prefix("j ") {
            Ok(ClipInst::Jmp(suffix.parse().map_err(|e| format!("{}", e))?))
        } else if s == "nop" {
            // nop is an alias for "j 0"
            Ok(ClipInst::Jmp(0))
        } else if let Some(suffix) = s.strip_prefix("n ") {
            let parts: Vec<&str> = suffix.split(' ').collect();
            if parts.len() < 4 {
                return Err("'n' requires 4 arguments".into());
            }
            let chan: u8 = parts[0].parse().map_err(|e| format!("{}", e))?;
            let note: Note = parts[1].parse()?;
            let velocity: Velocity = parts[2].parse()?;
            let duration: Duration = parts[3].parse()?;
            Ok(ClipInst::Call(SeqCall::QueueNote(
                chan, note, velocity, duration,
            )))
        } else if let Some(suffix) = s.strip_prefix("s ") {
            let parts: Vec<&str> = suffix.split(' ').collect();
            if parts.len() < 7 {
                return Err("'s' requires 7 arguments".into());
            }
            let chan: u8 = parts[0].parse().map_err(|e| format!("{}", e))?;
            let mut notes = vec![];
            for note in parts[1].split(',') {
                notes.push(note.parse()?);
            }
            let velocity: Velocity = parts[2].parse()?;
            let duration: Duration = parts[3].parse()?;
            let pulses = parts[4].parse().map_err(|e| format!("{}", e))?;
            let length = parts[5].parse().map_err(|e| format!("{}", e))?;
            let offset = parts[6].parse().map_err(|e| format!("{}", e))?;
            Ok(ClipInst::Call(SeqCall::QueueSequence(
                chan, notes, velocity, duration, pulses, length, offset,
            )))
        } else if let Some(suffix) = s.strip_prefix("cc ") {
            let parts: Vec<&str> = suffix.split(' ').collect();
            if parts.len() < 3 {
                return Err("'cc' requires 3 arguments".into());
            }
            let chan: u8 = parts[0].parse().map_err(|e| format!("{}", e))?;
            let control: u8 = parts[1].parse().map_err(|e| format!("{}", e))?;
            let value: u8 = parts[2].parse().map_err(|e| format!("{}", e))?;
            Ok(ClipInst::Call(SeqCall::QueueControl(chan, control, value)))
        } else {
            Err(format!("unknown instruction: {}", s))
        }
    }
}

impl Display for ClipInst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClipInst::Tick(duration) => {
                write!(f, "+ {}", duration)
            }
            ClipInst::Jmp(offset) => write!(f, "j {}", offset),
            ClipInst::Call(seq) => match seq {
                SeqCall::QueueNote(chan, note, velocity, duration) => {
                    write!(f, "n {} {} {} {}", chan, note, velocity, duration)
                }
                SeqCall::QueueSequence(chan, notes, velocity, duration, pulses, len, offset) => {
                    let notes_as_str: Vec<String> =
                        notes.iter().map(|n| format!("{}", n)).collect();
                    write!(
                        f,
                        "s {} {} {} {} {} {} {}",
                        chan,
                        notes_as_str.join(","),
                        velocity,
                        duration,
                        pulses,
                        len,
                        offset
                    )
                }
                SeqCall::QueueControl(chan, cc, val) => write!(f, "cc {} {} {}", chan, cc, val),
            },
        }
    }
}

pub type Clip = Vec<ClipInst>;

// === Sequencer ===============================================================

pub struct TickClockBase {
    /// The current tick.
    tick: u64,
    /// Desired MIDI PPQN.
    ppqn: u32,
}

impl Default for TickClockBase {
    fn default() -> Self {
        TickClockBase { tick: 0, ppqn: 48 }
    }
}

impl TickClockBase {
    pub fn new(ppqn: u32) -> Self {
        assert!(
            ppqn >= CLOCKS_PER_QN,
            "PPQN must be greater or equal to {}",
            CLOCKS_PER_QN
        );
        assert!(
            ppqn % CLOCKS_PER_QN == 0,
            "PPQN must be divisible by {}",
            CLOCKS_PER_QN
        );
        TickClockBase {
            ppqn,
            ..Default::default()
        }
    }

    /// Reset all internal state to the initial tick.
    fn reset(&mut self) {
        self.tick = 0
    }

    /// Fast-forwards ticks without real time synchronization.
    fn forward_tick(&mut self, ticks: u64) {
        self.tick += ticks;
    }

    /// Convert a duration into MIDI sequencer ticks based on the clock's configuration.
    fn get_ticks(&self, duration: &Duration) -> Option<u64> {
        match *duration {
            Duration::Ticks(ticks) => Some(ticks),
            Duration::Beats(beats, beats_per_bar) => {
                Some(((beats as u64) * 4 * (self.ppqn as u64)) / (beats_per_bar as u64))
            }
            Duration::Begin => None,
            Duration::End => Some(0),
        }
    }

    /// Return the current elapsed time in beats.
    fn elapsed(&self, beats_per_bar: u32) -> Duration {
        let on_beat = ((self.tick * beats_per_bar as u64) / (self.ppqn * 4) as u64) as u32;
        Duration::Beats(on_beat, beats_per_bar)
    }
}

pub trait TickClock {
    /// Base state.
    fn base(&self) -> &TickClockBase;

    /// Mutable base state.
    fn base_mut(&mut self) -> &mut TickClockBase;

    /// Return the current tick.
    fn tick(&self) -> u64 {
        self.base().tick
    }

    /// Return the configured PPQN.
    fn ppqn(&self) -> u32 {
        self.base().ppqn
    }

    /// Reset all internal state to the initial tick.
    fn reset(&mut self) {
        self.base_mut().reset()
    }

    /// Fast-forwards ticks without real time synchronization.
    fn forward_tick(&mut self, ticks: u64) {
        self.base_mut().forward_tick(ticks)
    }

    /// Convert a duration into MIDI sequencer ticks based on the clock's configuration.
    fn get_ticks(&self, duration: &Duration) -> Option<u64> {
        self.base().get_ticks(duration)
    }

    /// Returns the duration per tick, and drift from the targeted sync point.
    fn await_tick(&mut self) -> (time::Duration, time::Duration);

    /// Return the current elapsed time in beats and absolute time.
    fn elapsed(&self, beats_per_bar: u32) -> (Duration, time::Duration);
}

/// Not a real clock.
#[derive(Default)]
pub struct DummyClock {
    base: TickClockBase,
}

impl TickClock for DummyClock {
    fn base(&self) -> &TickClockBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut TickClockBase {
        &mut self.base
    }

    fn await_tick(&mut self) -> (time::Duration, time::Duration) {
        self.forward_tick(1);
        (time::Duration::ZERO, time::Duration::ZERO)
    }

    fn elapsed(&self, beats_per_bar: u32) -> (Duration, time::Duration) {
        (self.base().elapsed(beats_per_bar), time::Duration::ZERO)
    }
}

/// Synchronizes ticks to desired BPM using the OS system clock.
pub struct SystemClock {
    base: TickClockBase,
    /// Desired BPM.
    pub bpm: u32,
    /// The time we started playing.
    start_time: time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            base: TickClockBase::default(),
            bpm: 120,
            start_time: time::Instant::now(),
        }
    }
}

impl SystemClock {
    /// MIDI BPM is quarter notes per minute; PPQN is pulses per quarter note.
    pub fn new(bpm: u32, ppqn: u32) -> Self {
        assert!(bpm > 0, "BPM cannot be 0");
        SystemClock {
            base: TickClockBase::new(ppqn),
            bpm,
            ..Default::default()
        }
    }
}

impl TickClock for SystemClock {
    fn base(&self) -> &TickClockBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut TickClockBase {
        &mut self.base
    }

    fn await_tick(&mut self) -> (time::Duration, time::Duration) {
        let ticks_per_minute = self.bpm * self.ppqn();
        let duration_per_tick = time::Duration::from_secs(60) / ticks_per_minute;

        let drift = if self.tick() == 0 {
            self.start_time = time::Instant::now();
            time::Duration::ZERO
        } else {
            let sync_elapsed = duration_per_tick * (self.tick() as u32);
            let mut elapsed;
            loop {
                elapsed = self.start_time.elapsed();
                if elapsed >= sync_elapsed {
                    break;
                }
                // Try to sleep for half of the time we still have to wait, hoping that the OS will
                // actually wake us some time before the sync point.
                let sleep_time = (sync_elapsed - elapsed) / 4;
                // It's unlikely modern non-RT OSs can precisely sleep this low.
                // Note: This may be more precise with RT OSes.
                if sleep_time < time::Duration::from_micros(20) {
                    continue;
                }
                thread::sleep(sleep_time);
            }
            elapsed - sync_elapsed
        };

        self.forward_tick(1);
        (duration_per_tick, drift)
    }

    fn elapsed(&self, beats_per_bar: u32) -> (Duration, time::Duration) {
        (
            self.base().elapsed(beats_per_bar),
            if self.tick() == 0 {
                time::Duration::ZERO
            } else {
                self.start_time.elapsed()
            },
        )
    }
}

/// Keeps track of MIDI messages and emits them for each tick.
pub struct MidiSequencer {
    /// The current tick.
    pub tick: u64,
    /// Send MIDI clock.
    pub send_clock: bool,
    /// MIDI version.
    midi_ver: u32,
    /// Map of tick to queued raw messages.
    queue: HashMap<u64, Vec<u8>>,
    /// Map of allocated notes (channel, note) and their expiration tick. This is to avoid
    /// accidentally attempting to concurrently play the same note; a note may already be stopped
    /// but still be allocated, e.g. when part of a sequence that has not yet completed.
    allocated: HashMap<(u8, u8), u64>,
    /// Internal channel mapping to route to different final channel.
    chan_map: HashMap<u8, u8>,
}

impl MidiSequencer {
    pub fn new(send_clock: bool, midi_ver: u32) -> Self {
        Self {
            tick: 0,
            send_clock,
            midi_ver,
            queue: HashMap::new(),
            allocated: HashMap::new(),
            chan_map: HashMap::new(),
        }
    }

    /// Advance to the next tick and return the MIDI message stream to be sent to a compatible MIDI
    /// device.
    #[must_use]
    pub fn tick<TC>(&mut self, tick_clock: &TC) -> Vec<u8>
    where
        TC: TickClock + ?Sized,
    {
        let mut queue_opt = self.queue.remove(&self.tick);
        self.tick += 1; // advance tick

        let ticks_per_clock = tick_clock
            .get_ticks(&Duration::Beats(1, 4 * CLOCKS_PER_QN))
            .unwrap();
        if self.send_clock && (self.tick - 1) % ticks_per_clock == 0 {
            // Clock has highest priority; send it first.
            let mut ret: Vec<u8> = MidiMsg::Clock.to_midi(self.midi_ver);
            if let Some(ref mut queue) = queue_opt {
                ret.append(queue);
            }
            ret
        } else {
            queue_opt.unwrap_or_default()
        }
    }

    /// Advance until `delta` duration has elapsed, and for each tick calls back `send_midi`.
    pub fn tick_until<F, TC>(&mut self, tick_clock: &mut TC, delta: &Duration, send_midi: &mut F)
    where
        TC: TickClock + ?Sized,
        F: FnMut(&[u8]) -> bool,
    {
        let until_tick = match tick_clock.get_ticks(delta) {
            Some(t) => self.tick + t,
            _ => panic!("not a valid tick delta: {}", delta),
        };
        while self.tick != until_tick {
            let midi_bytes = self.tick(tick_clock);
            tick_clock.await_tick();
            if !send_midi(&midi_bytes) {
                break;
            }
        }
    }

    /// Fast-forwards until `delta` duration has elapsed, and for each tick calls back `send_midi`.
    pub fn forward_until<F, TC>(&mut self, tick_clock: &mut TC, delta: &Duration, send_midi: &mut F)
    where
        TC: TickClock + ?Sized,
        F: FnMut(&[u8]),
    {
        let until_tick = match tick_clock.get_ticks(delta) {
            Some(t) => self.tick + t,
            _ => panic!("not a valid tick delta: {}", delta),
        };
        while self.tick != until_tick {
            let midi_bytes = self.tick(tick_clock);
            tick_clock.forward_tick(1);
            send_midi(&midi_bytes);
        }
    }

    /// Queue note messages based on raw MIDI parameters.
    fn queue_midi(
        &mut self,
        chan: u8,
        note: u8,
        on_velocity: u8,
        off_velocity: u8,
        begin_tick: u64,
        ticks: Option<u64>,
        skip_allocated: bool,
    ) -> Result<()> {
        let &chan = self.chan_map.get(&chan).unwrap_or(&chan);

        if chan > 15 {
            return Err(format!("invalid channel: {}", chan));
        }
        if note > 127 {
            return Err(format!("invalid note: {}", note));
        }
        if on_velocity > 127 {
            return Err(format!("invalid velocity: {}", on_velocity));
        }
        if off_velocity > 127 {
            return Err(format!("invalid velocity: {}", off_velocity));
        }

        if let Some(expiration) = self.allocated.get(&(chan, note))
            && begin_tick < *expiration
        {
            match ticks {
                Some(0) => {
                    if *expiration != u64::MAX {
                        return Err(format!("cannot stop limited note: {}", note));
                    }
                }
                // We can't start a note that has not yet expired.
                Some(_) | None => {
                    if skip_allocated {
                        return Ok(());
                    } else {
                        return Err(format!("already allocated note: {}", note));
                    }
                }
            }
        }

        let end_tick = match ticks {
            Some(t) => begin_tick + t,
            None => u64::MAX,
        };

        if end_tick != begin_tick {
            // If this note does not stop on this tick, start playing.
            let on_msg = MidiMsg::NoteOn(chan, note, on_velocity);
            self.queue
                .entry(begin_tick)
                .or_default()
                .append(&mut on_msg.to_midi(self.midi_ver));
        }

        if end_tick != u64::MAX {
            // If this is not a forever-playing note, add a stop.
            let off_msg = MidiMsg::NoteOff(chan, note, off_velocity);
            self.queue
                .entry(end_tick)
                .or_default()
                .append(&mut off_msg.to_midi(self.midi_ver));
        }

        self.allocated.insert((chan, note), end_tick);

        Ok(())
    }

    /// Apply a call instruction.
    pub fn apply<TC>(&mut self, tick_clock: &TC, call: &SeqCall, skip_allocated: bool) -> Result<()>
    where
        TC: TickClock + ?Sized,
    {
        match call {
            SeqCall::QueueNote(c, n, v, d) => self.queue_note(tick_clock, *c, n, v, d),
            SeqCall::QueueSequence(c, ns, v, d, p, l, o) => {
                let es = euclidean_sequence(*p, *l, *o);
                self.queue_sequence(tick_clock, *c, ns, v, d, &es, skip_allocated)
            }
            SeqCall::QueueControl(c, cc, v) => self.queue_control(*c, *cc, *v),
        }
    }

    /// Queue a typed note description as MIDI messages.
    pub fn queue_note<TC>(
        &mut self,
        tick_clock: &TC,
        chan: u8,
        note: &Note,
        velocity: &Velocity,
        duration: &Duration,
    ) -> Result<()>
    where
        TC: TickClock + ?Sized,
    {
        let midi_note = Result::from(note)?;
        let velocity = velocity.into();
        let off_velocity = if let Duration::End = duration {
            velocity
        } else {
            0
        };
        let ticks = tick_clock.get_ticks(duration);
        self.queue_midi(
            chan,
            midi_note,
            velocity,
            off_velocity,
            self.tick,
            ticks,
            false,
        )
    }

    /// Queue typed notes based on a rhythmic sequence. Each element in `sequence` maps to the
    /// corresponding element in `notes`, where the latter simply repeats if exhausted.
    pub fn queue_sequence<TC>(
        &mut self,
        tick_clock: &TC,
        chan: u8,
        notes: &[Note],
        velocity: &Velocity,
        unit_duration: &Duration,
        sequence: &[bool],
        skip_allocated: bool,
    ) -> Result<()>
    where
        TC: TickClock + ?Sized,
    {
        let velocity = velocity.into();
        let unit_ticks = tick_clock
            .get_ticks(unit_duration)
            .expect("requires finite duration");
        assert!(unit_ticks != 0, "ticks cannot be 0");

        let mut notes_stream = notes.iter().cycle();
        let mut cur_tick = self.tick;
        for &pulse in sequence {
            if pulse {
                let note = notes_stream.next().unwrap();
                let midi_note = Result::from(note)?;
                self.queue_midi(
                    chan,
                    midi_note,
                    velocity,
                    0,
                    cur_tick,
                    Some(unit_ticks),
                    skip_allocated,
                )?;
            }
            cur_tick += unit_ticks;
        }

        Ok(())
    }

    /// Queue a Control Change message at the current tick.
    pub fn queue_control(&mut self, chan: u8, control: u8, value: u8) -> Result<()> {
        let &chan = self.chan_map.get(&chan).unwrap_or(&chan);

        if chan > 15 {
            return Err(format!("invalid channel: {}", chan));
        }
        if control > 127 {
            return Err(format!("invalid control: {}", control));
        }
        if value > 127 {
            return Err(format!("invalid value: {}", value));
        }

        let msg = MidiMsg::Cc(chan, control, value);
        self.queue
            .entry(self.tick)
            .or_default()
            .append(&mut msg.to_midi(self.midi_ver));

        Ok(())
    }

    /// Stop playing and return the last stream of MIDI messages to stop playing any currently
    /// playing notes. Resets the current tick back to 0.
    #[must_use]
    pub fn stop(&mut self) -> Vec<u8> {
        let mut stop_msgs = vec![];

        // Note: Beware non-stable iteration order.
        for ((chan, note), end_tick) in self.allocated.drain() {
            if self.tick <= end_tick {
                let off_msg = MidiMsg::NoteOff(chan, note, 0);
                stop_msgs.append(&mut off_msg.to_midi(self.midi_ver));
            }
        }

        self.tick = 0;
        self.queue.clear();

        stop_msgs
    }

    /// Map a channel to another, reflected in the MIDI message stream.
    pub fn insert_chan_map(&mut self, from: u8, to: u8) -> Result<()> {
        if from > 15 {
            return Err(format!("invalid channel: {}", from));
        }
        if to > 15 {
            return Err(format!("invalid channel: {}", to));
        }

        self.chan_map.insert(from, to);
        Ok(())
    }

    #[must_use]
    pub fn chan_map(&self) -> &HashMap<u8, u8> {
        &self.chan_map
    }
}

impl Default for MidiSequencer {
    fn default() -> Self {
        Self::new(true, 10)
    }
}

// === Extension for VM ========================================================

pub struct SeqVmState {
    pub clock: RefCell<Box<dyn TickClock>>,
    pub seq: RefCell<MidiSequencer>,
    pub map_note: Box<dyn Fn(i8) -> Note>,
    pub map_duration: Box<dyn Fn(u32) -> Duration>,
}

pub enum SeqVmInst {
    QueueNote(Rc<SeqVmState>),
}

impl InstExtension for SeqVmInst {
    fn eval(&self, stack: &mut Stack, _mboxes: &Mailboxes) -> Result<isize> {
        match self {
            SeqVmInst::QueueNote(vmstate) => {
                if stack.len() < 4 {
                    Err("QueueNote requires 4 arguments".into())
                } else {
                    let duration_int = match stack.pop().unwrap() {
                        Op::Int(i) => i,
                        Op::Float(f) => f as i32,
                    };
                    let duration = if duration_int > 0 {
                        (vmstate.map_duration)(duration_int as u32)
                    } else if duration_int == 0 {
                        Duration::End
                    } else {
                        Duration::Begin
                    };
                    let velocity = match stack.pop().unwrap() {
                        Op::Int(i) => Velocity::Raw(i as u8),
                        Op::Float(f) => Velocity::Raw((f * 127.0) as u8),
                    };
                    let note = match stack.pop().unwrap() {
                        Op::Int(i) => (vmstate.map_note)(i as i8),
                        Op::Float(f) => (vmstate.map_note)(f as i8),
                    };
                    let chan = match stack.pop().unwrap() {
                        Op::Int(i) => i as u8,
                        Op::Float(f) => f as u8,
                    };
                    let mut seq = vmstate.seq.borrow_mut();
                    let clock = vmstate.clock.borrow();
                    seq.queue_note(clock.as_ref(), chan, &note, &velocity, &duration)
                        .map(|_| 1)
                }
            }
        }
    }

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SeqVmInst::QueueNote(_) => f.write_str("SeqVmInst::QueueNote"),
        }
    }
}

impl From<SeqVmInst> for Inst {
    fn from(si: SeqVmInst) -> Self {
        Inst::Ext(Box::new(si))
    }
}

impl SeqVmInst {
    pub fn from(s: &str, vmstate: Rc<SeqVmState>) -> Option<SeqVmInst> {
        match s {
            "qnote" => Some(SeqVmInst::QueueNote(vmstate)),
            _ => None,
        }
    }
}

// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_parsing() {
        assert!("+ 1/4".parse() == Ok(ClipInst::Tick(Duration::Beats(1, 4))));
        assert!("j 10".parse() == Ok(ClipInst::Jmp(10)));
        assert!("j -1".parse() == Ok(ClipInst::Jmp(-1)));
        assert!("nop".parse() == Ok(ClipInst::Jmp(0)));
        assert!(
            "n 3 @60 ff 1/4".parse()
                == Ok(ClipInst::Call(SeqCall::QueueNote(
                    3,
                    Note::Raw(60),
                    Velocity::Ff,
                    Duration::Beats(1, 4)
                )))
        );
        assert!(
            "s 3 @42,@60 ff 1/4 10 12 2".parse()
                == Ok(ClipInst::Call(SeqCall::QueueSequence(
                    3,
                    vec![Note::Raw(42), Note::Raw(60)],
                    Velocity::Ff,
                    Duration::Beats(1, 4),
                    10,
                    12,
                    2
                )))
        );
        assert!("cc 3 4 5".parse() == Ok(ClipInst::Call(SeqCall::QueueControl(3, 4, 5))));
    }

    #[test]
    fn known_euclidean_sequences() {
        let euclidean_seq = |k, n, o| -> Vec<u8> {
            euclidean_sequence(k, n, o)
                .iter()
                .map(|b| if *b { 1 } else { 0 })
                .collect()
        };
        assert_eq!(euclidean_seq(0, 0, 0), vec![]);
        assert_eq!(euclidean_seq(1, 2, 0), vec![1, 0]);
        assert_eq!(euclidean_seq(2, 4, 0), vec![1, 0, 1, 0]);
        assert_eq!(euclidean_seq(2, 5, 3), vec![1, 0, 1, 0, 0]);
        assert_eq!(euclidean_seq(3, 3, 0), vec![1, 1, 1]);
        assert_eq!(euclidean_seq(3, 5, 0), vec![1, 0, 1, 0, 1]);
        assert_eq!(euclidean_seq(5, 6, 0), vec![1, 0, 1, 1, 1, 1]);
        assert_eq!(euclidean_seq(5, 8, 0), vec![1, 0, 1, 1, 0, 1, 1, 0]);
        assert_eq!(euclidean_seq(5, 8, 1), vec![0, 1, 1, 0, 1, 1, 0, 1]);
        assert_eq!(
            euclidean_seq(5, 13, 0),
            vec![1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0]
        );
        assert_eq!(
            euclidean_seq(7, 12, 0),
            vec![1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0]
        );
    }

    #[test]
    fn dummy_clock() {
        let c = DummyClock::default();
        assert_eq!(c.get_ticks(&Duration::Beats(1, 4)), Some(c.ppqn() as u64));
        assert_eq!(
            c.get_ticks(&Duration::Beats(2, 4)),
            Some((2 * c.ppqn()) as u64)
        );
        assert_eq!(
            c.get_ticks(&Duration::Beats(1, 8)),
            Some((c.ppqn() / 2) as u64)
        );
        assert_eq!(
            c.get_ticks(&Duration::Beats(1, 16)),
            Some((c.ppqn() / 4) as u64)
        );
        assert_eq!(c.get_ticks(&Duration::Beats(1, 192)), Some(1));
        assert_eq!(c.get_ticks(&Duration::Beats(1, 193)), Some(0));
    }

    #[test]
    fn tick_clock() {
        let mut c = SystemClock::default();
        assert_eq!(c.get_ticks(&Duration::Beats(1, 4)), Some(c.ppqn() as u64));
        assert_eq!(
            c.get_ticks(&Duration::Beats(2, 4)),
            Some((2 * c.ppqn()) as u64)
        );
        assert_eq!(
            c.get_ticks(&Duration::Beats(1, 8)),
            Some((c.ppqn() / 2) as u64)
        );
        assert_eq!(
            c.get_ticks(&Duration::Beats(1, 16)),
            Some((c.ppqn() / 4) as u64)
        );
        assert_eq!(c.get_ticks(&Duration::Beats(1, 192)), Some(1));
        assert_eq!(c.get_ticks(&Duration::Beats(1, 193)), Some(0));

        while c.tick() < 10 {
            let (_tpm, drift) = c.await_tick();
            // Just sanity check - we can't rely on this being too precise as long as we're not
            // running a RT OS. On an unloaded system this is typically below 10, but not
            // garanteed.
            assert!(drift < time::Duration::from_micros(2000));
        }
    }

    #[test]
    fn one_note_dummy() {
        let mut clock = DummyClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn one_note() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn one_note_no_clock() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::new(false, 10);
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0x91, 60, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn one_note_fast_forward() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64]);
        clock.forward_tick(1);
        assert_eq!(seq.tick(&clock), vec![]);
        clock.forward_tick(1);
        assert_eq!(seq.tick(&clock), vec![0xf8]);
        clock.forward_tick(1);
        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.forward_tick(1);
    }

    #[test]
    fn multiple_notes() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(1, clock.ppqn()),
        )
        .unwrap();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 1),
            &Velocity::Mf,
            &Duration::Beats(1, clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64, 0x91, 62, 64]);
        clock.await_tick();
        seq.queue_note(
            &clock,
            1,
            &Note::Maj(60, 2),
            &Velocity::Mf,
            &Duration::Beats(1, 4 * clock.ppqn()),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0x91, 64, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x81, 64, 0]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x81, 60, 0, 0x81, 62, 0]);
        clock.await_tick();
    }

    #[test]
    fn queue_sequence() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_sequence(
            &clock,
            1,
            &[Note::Maj(60, 0), Note::Maj(60, 1), Note::Maj(60, 2)],
            &Velocity::Mf,
            &Duration::Beats(1, 4 * clock.ppqn()),
            &[true, false, true, true, false, true],
            false,
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 62, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x81, 62, 0, 0x91, 64, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x81, 64, 0]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x91, 60, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x81, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn already_queueing_note() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();
        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(2),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 100]);
        clock.await_tick();
        // Different channel is ok.
        seq.queue_note(
            &clock,
            1,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(10),
        )
        .unwrap();
        assert!(
            seq.queue_note(
                &clock,
                0,
                &Note::Raw(60),
                &Velocity::Raw(100),
                &Duration::Ticks(2),
            )
            .is_err()
        );
        let _ = seq.tick(&clock);
        clock.await_tick();
        // Play on same tick as we are turning it off.
        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(101),
            &Duration::Ticks(2),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x80, 60, 0, 0x90, 60, 101]);
        // Trying to cancel a limited note does not work.
        assert!(
            seq.queue_note(&clock, 1, &Note::Raw(60), &Velocity::None, &Duration::End)
                .is_err()
        );
    }

    #[test]
    fn duration_start_stop() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();

        // Starting and immediately stopping is ok.
        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Begin,
        )
        .unwrap();
        seq.queue_note(&clock, 0, &Note::Raw(60), &Velocity::Raw(3), &Duration::End)
            .unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 100, 0x80, 60, 3]);
        clock.await_tick();

        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(101),
            &Duration::Begin,
        )
        .unwrap();

        assert_eq!(seq.tick(&clock), vec![0x90, 60, 101]);
        clock.await_tick();

        // Restarting already playing note doesn't make sense.
        assert!(
            seq.queue_note(
                &clock,
                0,
                &Note::Raw(60),
                &Velocity::Raw(101),
                &Duration::Begin,
            )
            .is_err()
        );
        // We can stop it now.
        seq.queue_note(&clock, 0, &Note::Raw(60), &Velocity::None, &Duration::End)
            .unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x80, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn stop_restart() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();

        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(1),
        )
        .unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 100]);
        clock.await_tick();

        assert_eq!(seq.tick(&clock), vec![0x80, 60, 0]);
        clock.await_tick();

        // All notes played and stopped, nothing to stop.
        assert_eq!(seq.stop(), vec![]);
        clock.reset();

        // Restart
        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(99),
            &Duration::Ticks(10),
        )
        .unwrap();
        seq.queue_note(
            &clock,
            3,
            &Note::Raw(61),
            &Velocity::Raw(98),
            &Duration::Ticks(1),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 99, 0x93, 61, 98]);
        clock.await_tick();

        // Turn it off.
        let stop_msgs = seq.stop();
        assert!(
            stop_msgs == vec![0x80, 60, 0, 0x83, 61, 0]
                || stop_msgs == vec![0x83, 61, 0, 0x80, 60, 0]
        );
    }

    #[test]
    fn control_change() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();

        seq.queue_note(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(1),
        )
        .unwrap();

        seq.queue_control(3, 5, 42).unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 100, 0xb3, 5, 42]);
        clock.await_tick();

        assert_eq!(seq.tick(&clock), vec![0x80, 60, 0]);
        clock.await_tick();

        seq.queue_control(0, 8, 111).unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0xb0, 8, 111]);
    }

    #[test]
    fn map_chan() {
        let mut clock = SystemClock::default();
        let mut seq = MidiSequencer::default();

        seq.queue_note(
            &clock,
            1,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(1),
        )
        .unwrap();
        seq.queue_control(2, 5, 42).unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 100, 0xb2, 5, 42]);
        clock.await_tick();

        // Does not affect already queued messages, e.g. stop messages are still being sent to the
        // right channel.
        seq.insert_chan_map(1, 5).unwrap();
        seq.insert_chan_map(2, 6).unwrap();

        assert_eq!(seq.tick(&clock), vec![0x81, 60, 0]);
        clock.await_tick();

        seq.queue_note(
            &clock,
            1,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(1),
        )
        .unwrap();
        seq.queue_note(
            &clock,
            1,
            &Note::Raw(61),
            &Velocity::Raw(100),
            &Duration::Ticks(2),
        )
        .unwrap();
        seq.queue_control(2, 8, 111).unwrap();

        assert_eq!(
            seq.tick(&clock),
            vec![0xf8, 0x95, 60, 100, 0x95, 61, 100, 0xb6, 8, 111]
        );
        clock.await_tick();

        assert_eq!(seq.tick(&clock), vec![0x85, 60, 0]);
        clock.await_tick();

        // Stop still gets the right channel after new mapping.
        seq.insert_chan_map(1, 1).unwrap();
        assert_eq!(seq.stop(), vec![0x85, 61, 0]);

        // Input value sanitization.
        assert!(seq.insert_chan_map(30, 3).is_err());
        assert!(seq.insert_chan_map(3, 30).is_err());
        assert!(seq.chan_map().contains_key(&1));
        assert!(seq.chan_map().contains_key(&2));
        assert!(!seq.chan_map().contains_key(&3));
        assert!(!seq.chan_map().contains_key(&30));
    }

    #[test]
    fn vm_prog_err() {
        let vmstate = Rc::new(SeqVmState {
            clock: RefCell::new(Box::new(SystemClock::default())),
            seq: RefCell::default(),
            map_note: Box::new(|n| Note::Maj(60, n)),
            map_duration: Box::new(|d| Duration::Beats(d, 4)),
        });
        let prog = vec![
            Inst::Nop,
            Inst::Push(Op::Int(11)),
            SeqVmInst::from("qnote", vmstate).unwrap().into(), // test string conversion
            Inst::Nop,
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        assert_eq!(
            core.eval(None),
            Err("QueueNote requires 4 arguments".into())
        );
        assert_eq!(core.pc, 4);
        assert_eq!(core.stack, vec![Op::Int(11)]);
    }

    #[test]
    fn vm_prog_one_note() {
        let vmstate = Rc::new(SeqVmState {
            clock: RefCell::new(Box::new(SystemClock::default())),
            seq: RefCell::default(),
            map_note: Box::new(|n| Note::Maj(60, n)),
            map_duration: Box::new(|d| Duration::Beats(d, 4 * 48)),
        });
        let prog = vec![
            Inst::Push(Op::Int(9)),
            Inst::Push(Op::Int(3)),
            Inst::Push(Op::Int(64)),
            Inst::Push(Op::Int(3)),
            SeqVmInst::QueueNote(vmstate.clone()).into(),
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(None).unwrap();
        let mut seq = vmstate.seq.borrow_mut();
        let mut clock = vmstate.clock.borrow_mut();
        assert_eq!(seq.tick(clock.as_ref()), vec![0xf8, 0x99, 65, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(clock.as_ref()), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(clock.as_ref()), vec![0xf8]);
        clock.await_tick();
        assert_eq!(seq.tick(clock.as_ref()), vec![0x89, 65, 0]);
        clock.await_tick();
    }
}

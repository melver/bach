use crate::midi::*;
use crate::units::*;
use crate::vm::*;
use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::fmt;
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

    let mut cyclic = sequence.into_iter().flatten().cycle();
    for _ in 0..offset {
        cyclic.next();
    }
    cyclic.take(len as usize).collect()
}

/// Synchronizes ticks to desired BPM.
pub struct TickClock {
    /// Desired BPM.
    pub bpm: u32,
    /// Desired MIDI PPQN.
    ppqn: u32,
    /// The current tick.
    tick: u64,
    /// The time we started playing.
    start_time: time::Instant,
}

impl Default for TickClock {
    fn default() -> Self {
        TickClock {
            bpm: 120,
            ppqn: 48,
            tick: 0,
            start_time: time::Instant::now(),
        }
    }
}

impl TickClock {
    /// MIDI BPM is quarter notes per minute; PPQN is pulses per quarter note.
    pub fn new(bpm: u32, ppqn: u32) -> Self {
        assert!(bpm > 0, "BPM cannot be 0");
        assert!(ppqn >= CLOCKS_PER_QN, "PPQN must be greater or equal to 24");
        assert!(ppqn % CLOCKS_PER_QN == 0, "PPQN must be divisible by 24");
        TickClock {
            bpm,
            ppqn,
            ..Default::default()
        }
    }

    /// Returns the duration per tick, and drift from the targeted sync point.
    pub fn await_tick(&mut self) -> (time::Duration, time::Duration) {
        let ticks_per_minute = self.bpm * self.ppqn;
        let duration_per_tick = time::Duration::from_secs(60) / ticks_per_minute;

        let drift = if self.tick == 0 {
            self.start_time = time::Instant::now();
            time::Duration::ZERO
        } else {
            let sync_elapsed = duration_per_tick * (self.tick as u32);
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

        self.tick += 1;
        (duration_per_tick, drift)
    }

    /// Fast-forwards ticks without real time synchronization.
    pub fn forward_tick(&mut self, ticks: u64) {
        self.tick += ticks;
    }

    /// Reset all internal state to the initial tick.
    pub fn reset(&mut self) {
        self.tick = 0
    }

    /// Convert a duration into MIDI sequencer ticks.
    pub fn into_ticks(&self, duration: &Duration) -> Option<u64> {
        match *duration {
            Duration::Ticks(ticks) => Some(ticks),
            Duration::Beats(beats, beats_per_bar) => {
                Some(((beats as u64) * 4 * (self.ppqn as u64)) / (beats_per_bar as u64))
            }
            Duration::Begin => None,
            Duration::End => Some(0),
        }
    }
}

/// Keeps track of MIDI commands and emits them for each tick.
pub struct MidiSequencer {
    /// The current tick.
    pub tick: u64,
    /// Map of tick to queued commands.
    queue: HashMap<u64, Vec<u8>>,
    /// Map of allocated notes (channel, note) and their expiration tick. This is to avoid
    /// accidentally attempting to concurrently play the same note; a note may already be stopped
    /// but still be allocated, e.g. when part of a sequence that has not yet completed.
    allocated: HashMap<(u8, u8), u64>,
}

impl MidiSequencer {
    pub fn new() -> Self {
        Self {
            tick: 0,
            queue: HashMap::new(),
            allocated: HashMap::new(),
        }
    }

    #[must_use]
    pub fn tick(&mut self, tick_clock: &TickClock) -> Vec<u8> {
        // TickClock::await_tick() should be called after we're done with all processing.
        assert!(tick_clock.tick == self.tick, "unsynchronized TickClock");

        let mut queue_opt = self.queue.remove(&self.tick);
        self.tick += 1; // advance tick

        let ticks_per_clock = (tick_clock.ppqn / CLOCKS_PER_QN) as u64;
        if (self.tick - 1) % ticks_per_clock == 0 {
            // Clock has highest priority; send it first.
            let mut ret: Vec<u8> = MidiMsg::Clock.into();
            if let Some(ref mut queue) = queue_opt {
                ret.append(queue);
            }
            ret
        } else {
            queue_opt.unwrap_or_default()
        }
    }

    /// Queue note messages based on raw MIDI parameters.
    fn queue_raw(
        &mut self,
        channel: u8,
        note: u8,
        on_velocity: u8,
        off_velocity: u8,
        begin_tick: u64,
        ticks: Option<u64>,
        skip_allocated: bool,
    ) -> Result<(), &'static str> {
        if channel > 15 {
            return Err("invalid channel");
        }
        if note > 127 {
            return Err("invalid note");
        }
        if on_velocity > 127 {
            return Err("invalid velocity");
        }
        if off_velocity > 127 {
            return Err("invalid velocity");
        }

        if let Some(expiration) = self.allocated.get(&(channel, note)) {
            if begin_tick < *expiration {
                match ticks {
                    Some(0) => {
                        if *expiration != u64::MAX {
                            return Err("cannot stop limited note");
                        }
                    }
                    // We can't start a note that has not yet expired.
                    Some(_) | None => {
                        if skip_allocated {
                            return Ok(());
                        } else {
                            return Err("already allocated note");
                        }
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
            let on_cmd = MidiMsg::NoteOn(channel, note, on_velocity);
            self.queue
                .entry(begin_tick)
                .or_default()
                .append(&mut on_cmd.into());
        }

        if end_tick != u64::MAX {
            // If this is not a forever-playing note, add a stop.
            let off_cmd = MidiMsg::NoteOff(channel, note, off_velocity);
            self.queue
                .entry(end_tick)
                .or_default()
                .append(&mut off_cmd.into());
        }

        self.allocated.insert((channel, note), end_tick);

        Ok(())
    }

    /// Queue a typed note description as MIDI messages.
    pub fn queue(
        &mut self,
        tick_clock: &TickClock,
        channel: u8,
        note: &Note,
        velocity: &Velocity,
        duration: &Duration,
    ) -> Result<(), &'static str> {
        let midi_note = Result::from(note)?;
        let velocity = velocity.into();
        let off_velocity = if let Duration::End = duration {
            velocity
        } else {
            0
        };
        let ticks = tick_clock.into_ticks(duration);
        self.queue_raw(
            channel,
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
    pub fn queue_sequence(
        &mut self,
        tick_clock: &TickClock,
        channel: u8,
        notes: &[Note],
        velocity: &Velocity,
        unit_duration: &Duration,
        sequence: &[bool],
        skip_allocated: bool,
    ) -> Result<(), &'static str> {
        let velocity = velocity.into();
        let unit_ticks = tick_clock
            .into_ticks(unit_duration)
            .expect("requires finite duration");
        assert!(unit_ticks != 0, "ticks cannot be 0");

        let mut notes_stream = notes.iter().cycle();
        let mut cur_tick = self.tick;
        for &pulse in sequence {
            if pulse {
                let note = notes_stream.next().unwrap();
                let midi_note = Result::from(note)?;
                self.queue_raw(
                    channel,
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

    #[must_use]
    pub fn stop(&mut self) -> Vec<u8> {
        let mut stop_msgs = vec![];

        // Note: Beware non-stable iteration order.
        for ((chan, note), end_tick) in self.allocated.drain() {
            if self.tick <= end_tick {
                let off_cmd = MidiMsg::NoteOff(chan, note, 0);
                stop_msgs.append(&mut off_cmd.into());
            }
        }

        self.tick = 0;
        self.queue.clear();

        stop_msgs
    }
}

impl Default for MidiSequencer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SeqVmState {
    pub clock: RefCell<TickClock>,
    pub seq: RefCell<MidiSequencer>,
    pub map_note: Box<dyn Fn(i8) -> Note>,
    pub map_duration: Box<dyn Fn(u32) -> Duration>,
}

pub enum SeqInst {
    QueueNote(Rc<SeqVmState>),
}

impl InstExtension for SeqInst {
    fn eval(&self, stack: &mut Stack, _mboxes: &Mailboxes) -> Result<isize, &'static str> {
        match self {
            SeqInst::QueueNote(vmstate) => {
                if stack.len() < 4 {
                    Err("QueueNote requires 4 arguments")
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
                    seq.queue(&clock, chan, &note, &velocity, &duration)
                        .map(|_| 1)
                }
            }
        }
    }

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SeqInst::QueueNote(_) => f.write_str("SeqInst::QueueNote"),
        }
    }
}

impl From<SeqInst> for Inst {
    fn from(si: SeqInst) -> Self {
        Inst::Extension(Box::new(si))
    }
}

impl SeqInst {
    pub fn from(s: &str, vmstate: Rc<SeqVmState>) -> Option<SeqInst> {
        match s {
            "qnote" => Some(SeqInst::QueueNote(vmstate)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn tick_clock() {
        let mut c = TickClock::default();
        assert_eq!(c.into_ticks(&Duration::Beats(1, 4)), Some(c.ppqn as u64));
        assert_eq!(
            c.into_ticks(&Duration::Beats(2, 4)),
            Some((2 * c.ppqn) as u64)
        );
        assert_eq!(
            c.into_ticks(&Duration::Beats(1, 8)),
            Some((c.ppqn / 2) as u64)
        );
        assert_eq!(
            c.into_ticks(&Duration::Beats(1, 16)),
            Some((c.ppqn / 4) as u64)
        );
        assert_eq!(c.into_ticks(&Duration::Beats(1, 192)), Some(1));
        assert_eq!(c.into_ticks(&Duration::Beats(1, 193)), Some(0));

        while c.tick < 10 {
            let (_tpm, drift) = c.await_tick();
            // Just sanity check - we can't rely on this being too precise as long as we're not
            // running a RT OS. On an unloaded system this is typically below 10, but not
            // garanteed.
            assert!(drift < time::Duration::from_micros(2000));
        }
    }

    #[test]
    fn one_note() {
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();
        seq.queue(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn),
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
    fn one_note_fast_forward() {
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();
        seq.queue(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(3, 4 * clock.ppqn),
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
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();
        seq.queue(
            &clock,
            1,
            &Note::Maj(60, 0),
            &Velocity::Mf,
            &Duration::Beats(1, clock.ppqn),
        )
        .unwrap();
        seq.queue(
            &clock,
            1,
            &Note::Maj(60, 1),
            &Velocity::Mf,
            &Duration::Beats(1, clock.ppqn),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x91, 60, 64, 0x91, 62, 64]);
        clock.await_tick();
        seq.queue(
            &clock,
            1,
            &Note::Maj(60, 2),
            &Velocity::Mf,
            &Duration::Beats(1, 4 * clock.ppqn),
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
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();
        seq.queue_sequence(
            &clock,
            1,
            &[Note::Maj(60, 0), Note::Maj(60, 1), Note::Maj(60, 2)],
            &Velocity::Mf,
            &Duration::Beats(1, 4 * clock.ppqn),
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
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();
        seq.queue(
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
        seq.queue(
            &clock,
            1,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Ticks(10),
        )
        .unwrap();
        assert!(seq
            .queue(
                &clock,
                0,
                &Note::Raw(60),
                &Velocity::Raw(100),
                &Duration::Ticks(2),
            )
            .is_err());
        let _ = seq.tick(&clock);
        clock.await_tick();
        // Play on same tick as we are turning it off.
        seq.queue(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(101),
            &Duration::Ticks(2),
        )
        .unwrap();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x80, 60, 0, 0x90, 60, 101]);
        // Trying to cancel a limited note does not work.
        assert!(seq
            .queue(&clock, 1, &Note::Raw(60), &Velocity::None, &Duration::End)
            .is_err());
    }

    #[test]
    fn duration_start_stop() {
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();

        // Starting and immediately stopping is ok.
        seq.queue(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(100),
            &Duration::Begin,
        )
        .unwrap();
        seq.queue(&clock, 0, &Note::Raw(60), &Velocity::Raw(3), &Duration::End)
            .unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x90, 60, 100, 0x80, 60, 3]);
        clock.await_tick();

        seq.queue(
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
        assert!(seq
            .queue(
                &clock,
                0,
                &Note::Raw(60),
                &Velocity::Raw(101),
                &Duration::Begin,
            )
            .is_err());
        // We can stop it now.
        seq.queue(&clock, 0, &Note::Raw(60), &Velocity::None, &Duration::End)
            .unwrap();

        assert_eq!(seq.tick(&clock), vec![0xf8, 0x80, 60, 0]);
        clock.await_tick();
    }

    #[test]
    fn stop_restart() {
        let mut clock = TickClock::default();
        let mut seq = MidiSequencer::new();

        seq.queue(
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
        seq.queue(
            &clock,
            0,
            &Note::Raw(60),
            &Velocity::Raw(99),
            &Duration::Ticks(10),
        )
        .unwrap();
        seq.queue(
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
    fn vm_prog_err() {
        let vmstate = Rc::new(SeqVmState {
            clock: RefCell::default(),
            seq: RefCell::default(),
            map_note: Box::new(|n| Note::Maj(60, n)),
            map_duration: Box::new(|d| Duration::Beats(d, 4)),
        });
        let prog = vec![
            Inst::Nop,
            Inst::Push(Op::Int(11)),
            SeqInst::from("qnote", vmstate).unwrap().into(), // test string conversion
            Inst::Nop,
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        assert_eq!(core.eval(None), Err("QueueNote requires 4 arguments"));
        assert_eq!(core.pc, 4);
        assert_eq!(core.stack, vec![Op::Int(11)]);
    }

    #[test]
    fn vm_prog_one_note() {
        let vmstate = Rc::new(SeqVmState {
            clock: RefCell::default(),
            seq: RefCell::default(),
            map_note: Box::new(|n| Note::Maj(60, n)),
            map_duration: Box::new(|d| Duration::Beats(d, 4 * 48)),
        });
        let prog = vec![
            Inst::Push(Op::Int(9)),
            Inst::Push(Op::Int(3)),
            Inst::Push(Op::Int(64)),
            Inst::Push(Op::Int(3)),
            SeqInst::QueueNote(vmstate.clone()).into(),
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(None).unwrap();
        let mut seq = vmstate.seq.borrow_mut();
        let mut clock = vmstate.clock.borrow_mut();
        assert_eq!(seq.tick(&clock), vec![0xf8, 0x99, 65, 64]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0xf8]);
        clock.await_tick();
        assert_eq!(seq.tick(&clock), vec![0x89, 65, 0]);
        clock.await_tick();
    }
}

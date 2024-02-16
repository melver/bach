// Simple tracker that reads from a text file and outputs real-time MIDI to stdout:
//
//  # comment
//  n <chan> <note> <velocity> <duration>
//  ... more notes ...
//  # advance beats
//  +<duration>
//
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use bach::sequencer;
use bach::units::*;
use std::fs::File;
use std::io::{self, BufRead, Write};

fn main() {
    let bpm: u32 = std::env::args()
        .nth(1)
        .expect("must provide BPM")
        .parse()
        .expect("not a valid integer");
    let ppqn: u32 = std::env::args()
        .nth(2)
        .expect("must provide PPQN")
        .parse()
        .expect("not a valid integer");
    let file_name = std::env::args().nth(3).expect("must provide input file");

    let file = File::open(file_name).unwrap();
    let mut clock = sequencer::TickClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::new();
    let mut note_stack = Vec::new();

    let mut line_num = 0;
    for line in io::BufReader::new(file).lines().flatten() {
        line_num += 1;
        if line.starts_with('#') {
            continue;
        } else if let Some(suffix) = line.strip_prefix("+ ") {
            let tick_delta: Duration = suffix.into();
            let until_tick = match clock.into_ticks(&tick_delta) {
                Some(t) => seq.tick + t,
                _ => panic!("not a valid tick delta: {}", suffix),
            };
            while seq.tick != until_tick {
                let midi_bytes = seq.tick(&clock);
                clock.await_tick();
                io::stdout().write_all(&midi_bytes).unwrap();
                io::stdout().flush().unwrap();
            }
        } else if let Some(suffix) = line.strip_prefix("n ") {
            let parts: Vec<&str> = suffix.split(' ').collect();
            let chan: u8 = parts[0].parse().unwrap();
            let note: Note = parts[1].into();
            let velocity: Velocity = parts[2].into();
            let duration: Duration = parts[3].into();

            seq.queue(&clock, chan, &note, &velocity, &duration)
                .unwrap();
        } else if let Some(suffix) = line.strip_prefix(". ") {
            let note: Note = suffix.into();
            note_stack.push(note);
        } else if let Some(suffix) = line.strip_prefix("s ") {
            let parts: Vec<&str> = suffix.split(' ').collect();
            let chan: u8 = parts[0].parse().unwrap();
            let velocity: Velocity = parts[1].into();
            let duration: Duration = parts[2].into();
            let pulses = parts[3].parse().expect("invalid pulses");
            let length = parts[4].parse().expect("invalid length");
            let offset = parts[5].parse().expect("invalid offset");
            let sequence = sequencer::euclidean_sequence(pulses, length, offset);
            if note_stack.is_empty() {
                panic!("line {}: note stack is empty!", line_num);
            }
            match seq.queue_sequence(
                &clock,
                chan,
                &note_stack,
                &velocity,
                &duration,
                &sequence,
                false,
            ) {
                Ok(()) => {}
                Err(e) => {
                    panic!("line {}: {}", line_num, e);
                }
            }
            note_stack.clear();
        } else {
            panic!("unknown statement: {}", line);
        }
    }

    // Stop all still playing notes.
    io::stdout().write_all(&seq.stop()).unwrap();
}

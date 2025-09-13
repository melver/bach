// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! Simple tracker that reads from a text file and outputs real-time MIDI:
//!
//!  # comment
//!  n <chan> <note> <velocity> <duration>
//!  ... more notes ...
//!  # advance beats
//!  +<duration>

use bach::sequencer::{self, ClipInst};
use bach::units::*;
use std::collections::HashSet;
use std::fs;
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
    let input_path = std::env::args().nth(3).expect("must provide input file");
    let midi_path = std::env::args()
        .nth(4)
        .expect("must provide MIDI output device");

    let input_file = fs::File::open(input_path).unwrap();
    let mut midi_write: Box<dyn FnMut(&[Vec<u8>]) -> bool> = if midi_path == "-" {
        Box::new(|b| -> bool {
            io::stdout().write_all(&b.concat()).unwrap();
            io::stdout().flush().unwrap();
            true
        })
    } else {
        let mut f = fs::OpenOptions::new().write(true).open(&midi_path).unwrap();
        Box::new(move |b| -> bool {
            f.write_all(&b.concat()).unwrap();
            f.flush().unwrap();
            true
        })
    };

    let mut clip: sequencer::Clip = vec![];
    let mut skip_allocated = false;

    let mut line_num = 0;
    for line in io::BufReader::new(input_file).lines().map(|l| l.unwrap()) {
        line_num += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        } else if let Some(suffix) = line.strip_prefix(".skip_allocated ") {
            skip_allocated = suffix.parse().unwrap();
        } else {
            match line.parse() {
                Ok(inst) => clip.push(inst),
                Err(e) => panic!("line {}: {}", line_num, e),
            }
        }
    }
    clip.push(ClipInst::Tick(Duration::Beats(3, 1)));

    let mut clock = sequencer::SystemClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::default();
    let mut skip_inst = HashSet::new();
    let mut inst_idx: isize = 0;

    while inst_idx < clip.len() as isize {
        let clip_inst = &clip[inst_idx as usize];
        if midi_path != "-" && !skip_inst.contains(&inst_idx) {
            println!("<{}> {}", inst_idx, clip_inst);
        }
        match clip_inst {
            ClipInst::Tick(delta) => seq.tick_until(&mut clock, delta, &mut midi_write),
            ClipInst::Jmp(offset) => {
                if skip_inst.insert(inst_idx) {
                    inst_idx = std::cmp::max(0, inst_idx + *offset as isize);
                }
            }
            ClipInst::Call(seq_call) => {
                if let Err(e) = seq.apply(&clock, seq_call, skip_allocated) {
                    println!("<! failed to queue: {}", e);
                }
            }
        }
        inst_idx += 1;
    }

    // Stop all still playing notes.
    midi_write(&seq.stop());
}

// Simple tracker that reads from a text file and outputs real-time MIDI to stdout:
//
//  # comment
//  n <chan> <note> <velocity> <duration>
//  ... more notes ...
//  # advance beats
//  +<duration>
//
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use bach::sequencer::{self, SeqCommand};
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
    let mut midi_write: Box<dyn FnMut(&[u8])> = if midi_path == "-" {
        Box::new(|b| {
            io::stdout().write_all(b).unwrap();
            io::stdout().flush().unwrap();
        })
    } else {
        let mut f = fs::OpenOptions::new().write(true).open(&midi_path).unwrap();
        Box::new(move |b| {
            f.write_all(b).unwrap();
            f.flush().unwrap();
        })
    };

    let mut clip: sequencer::Clip = vec![];
    let mut skip_allocated = false;

    let mut line_num = 0;
    for line in io::BufReader::new(input_file).lines().flatten() {
        line_num += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        } else if let Some(suffix) = line.strip_prefix(".skip_allocated ") {
            let val: u8 = suffix.parse().unwrap();
            skip_allocated = val != 0;
        } else {
            match line.parse() {
                Ok(cmd) => clip.push(cmd),
                Err(e) => panic!("line {}: {}", line_num, e),
            }
        }
    }

    let mut clock = sequencer::TickClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::new();
    let mut skip_cmd = HashSet::new();
    let mut cmd_idx: isize = 0;

    while cmd_idx < clip.len() as isize {
        let seq_cmd = &clip[cmd_idx as usize];
        cmd_idx += 1;
        if midi_path != "-" {
            println!("{}", seq_cmd);
        }
        match seq_cmd {
            SeqCommand::Tick(delta) => seq.tick_until(&mut clock, delta, &mut midi_write),
            SeqCommand::Jmp(offset) => {
                if skip_cmd.insert(cmd_idx) {
                    cmd_idx = std::cmp::max(0, cmd_idx + *offset);
                }
            }
            SeqCommand::QueueNote(c, n, v, d) => seq.queue(&clock, *c, n, v, d).unwrap(),
            SeqCommand::QueueSequence(c, ns, v, d, p, l, o) => {
                let eucl = sequencer::euclidean_sequence(*p, *l, *o);
                seq.queue_sequence(&clock, *c, ns, v, d, &eucl, skip_allocated)
                    .unwrap();
            }
        }
    }

    // Stop all still playing notes.
    midi_write(&seq.stop());
}

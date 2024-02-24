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
    for line in io::BufReader::new(input_file).lines().map(|l| l.unwrap()) {
        line_num += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        } else if let Some(suffix) = line.strip_prefix(".skip_allocated ") {
            skip_allocated = suffix.parse().unwrap();
        } else {
            match line.parse() {
                Ok(cmd) => clip.push(cmd),
                Err(e) => panic!("line {}: {}", line_num, e),
            }
        }
    }
    clip.push(SeqCommand::Tick(Duration::Beats(3, 1)));

    let mut clock = sequencer::TickClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::new();
    let mut skip_cmd = HashSet::new();
    let mut cmd_idx: isize = 0;

    while cmd_idx < clip.len() as isize {
        let seq_cmd = &clip[cmd_idx as usize];
        if midi_path != "-" && !skip_cmd.contains(&cmd_idx) {
            println!("<{}> {}", cmd_idx, seq_cmd);
        }
        match seq_cmd {
            SeqCommand::Tick(delta) => seq.tick_until(&mut clock, delta, &mut midi_write),
            SeqCommand::Jmp(offset) => {
                if skip_cmd.insert(cmd_idx) {
                    cmd_idx = std::cmp::max(0, cmd_idx + *offset);
                }
            }
            SeqCommand::QueueNote(c, n, v, d) => {
                if let Err(e) = seq.queue(&clock, *c, n, v, d) {
                    println!("<! failed to queue: {}", e);
                }
            }
            SeqCommand::QueueSequence(c, ns, v, d, p, l, o) => {
                let eucl = sequencer::euclidean_sequence(*p, *l, *o);
                if let Err(e) = seq.queue_sequence(&clock, *c, ns, v, d, &eucl, skip_allocated) {
                    println!("<! failed to queue: {}", e);
                }
            }
        }
        cmd_idx += 1;
    }

    // Stop all still playing notes.
    midi_write(&seq.stop());
}

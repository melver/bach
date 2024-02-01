// Simple interpreter that reads program from a text file and outputs real-time MIDI to stdout:

use bach::sequencer::*;
use bach::units::*;
use bach::vm::*;
use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufRead, Write};
use std::rc::Rc;

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
    let script = File::open(file_name).unwrap();
    let mut lines = io::BufReader::new(script).lines().flatten();

    // Get scale and time signature first.
    let mut map_note: Option<Box<dyn Fn(i8) -> Note>> = None;
    let mut map_duration: Option<Box<dyn Fn(u32) -> Duration>> = None;
    for line in lines.by_ref() {
        if line.starts_with('#') {
            continue;
        } else if let Some(suffix) = line.strip_prefix(".scale ") {
            map_note = match suffix.into() {
                Note::Raw(_) => Some(Box::new(|n| Note::Raw(n as u8))),
                Note::Maj(k, _) => Some(Box::new(move |n| Note::Maj(k, n))),
            }
        } else if let Some(suffix) = line.strip_prefix(".time_sig ") {
            map_duration = match suffix.into() {
                Duration::Ticks(_) => Some(Box::new(|d| Duration::Ticks(d as u64))),
                Duration::Beats(1, bar) => Some(Box::new(move |d| Duration::Beats(d, bar))),
                _ => panic!("invalid time signature: {}", suffix),
            }
        }

        if map_note.is_some() && map_duration.is_some() {
            break;
        }
    }

    let vmstate = Rc::new(SeqVmState {
        clock: RefCell::new(TickClock::new(bpm, ppqn)),
        seq: RefCell::new(MidiSequencer::new()),
        map_note: map_note.expect("must provide .scale directive"),
        map_duration: map_duration.expect("must provide .time_sig directive"),
    });

    // Read the rest of the program.
    let mut prog = Program::new();
    for line in lines {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(seqinst) = SeqInst::from(line.as_str(), vmstate.clone()) {
            prog.push(seqinst.into());
        } else {
            prog.push(line.as_str().into());
        }
    }

    let mboxes = Mailboxes::default();
    let mut vmcore = Core::new(prog, mboxes.clone());

    while vmcore.is_running() {
        vmcore.eval(None).unwrap();
        // Mailbox 0 denotes tick delta.
        let tick_delta = match mboxes.borrow_mut().remove(&0).expect("require delay") {
            Op::Int(i) if i >= 0 => (vmstate.map_duration)(i as u32),
            op => panic!("unsupported tick delta: {:?}", op),
        };
        let mut seq = vmstate.seq.borrow_mut();
        let mut clock = vmstate.clock.borrow_mut();
        let until_tick = seq.tick + clock.into_ticks(&tick_delta).unwrap();
        while seq.tick != until_tick {
            let midi_bytes = seq.tick(&clock);
            clock.await_tick();
            io::stdout().write_all(&midi_bytes).unwrap();
            io::stdout().flush().unwrap();
        }
    }

    // Stop all still playing notes.
    io::stdout()
        .write_all(&vmstate.seq.borrow_mut().stop())
        .unwrap();
}

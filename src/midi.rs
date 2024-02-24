// Conversion to and from raw MIDI messages.
//
// Some MIDI references:
//   https://www.cs.cmu.edu/~music/cmsip/readings/MIDI%20tutorial%20for%20programmers.html
//
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

// This is defined by the MIDI spec.
pub const CLOCKS_PER_QN: u32 = 24;

/// Raw abstracted MIDI commands.
#[derive(Debug)]
pub enum MidiMsg {
    NoteOn(u8, u8, u8),
    NoteOff(u8, u8, u8),
    Clock,
}

/// Converts a typed MidiMsg to raw MIDI bytes.
impl From<MidiMsg> for Vec<u8> {
    fn from(cmd: MidiMsg) -> Vec<u8> {
        match cmd {
            MidiMsg::NoteOn(c, n, v) => vec![0x90 | c, n, v],
            MidiMsg::NoteOff(c, n, v) => vec![0x80 | c, n, v],
            MidiMsg::Clock => vec![0xf8],
        }
    }
}

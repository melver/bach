// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! MIDI specification definitions. Defines basic MIDI message types and how to convert them to a
//! stream of bytes.

/// This is defined by the MIDI spec.
pub const CLOCKS_PER_QN: u32 = 24;

/// Raw abstracted MIDI commands.
#[derive(Debug)]
pub enum MidiMsg {
    NoteOn(u8, u8, u8),
    NoteOff(u8, u8, u8),
    Cc(u8, u8, u8),
    Clock,
}

/// Converts a typed MidiMsg to raw MIDI bytes.
/// [Source](https://midi.org/expanded-midi-1-0-messages-list).
impl From<MidiMsg> for Vec<u8> {
    fn from(cmd: MidiMsg) -> Vec<u8> {
        match cmd {
            MidiMsg::NoteOn(c, n, v) => vec![0x90 | c, n, v],
            MidiMsg::NoteOff(c, n, v) => vec![0x80 | c, n, v],
            MidiMsg::Cc(c, cc, v) => vec![0xb0 | c, cc, v],
            MidiMsg::Clock => vec![0xf8],
        }
    }
}

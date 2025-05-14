// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! MIDI specification definitions. Defines basic MIDI message types and how to convert them to a
//! stream of bytes.

/// MIDI clock pulses per quarter note (MIDI 1.0 spec).
pub const CLOCKS_PER_QN: u32 = 24;

/// Unified abstracted MIDI message types.
#[derive(Debug, Clone)]
pub enum MidiMsg {
    NoteOn(u8, u8, u8),
    NoteOff(u8, u8, u8),
    Cc(u8, u8, u8),
    Clock,
}

impl MidiMsg {
    pub fn to_midi(&self, ver: u32) -> Vec<u8> {
        match ver {
            10 => self.to_midi10(),
            20 => self.to_midi20(),
            _ => unimplemented!("MIDI {} unsupported", ver),
        }
    }

    /// Convert to MIDI 1.0 byte stream.
    /// [Source](https://midi.org/expanded-midi-1-0-messages-list).
    pub fn to_midi10(&self) -> Vec<u8> {
        match *self {
            MidiMsg::NoteOn(c, n, v) => vec![0x90 | c, n, v],
            MidiMsg::NoteOff(c, n, v) => vec![0x80 | c, n, v],
            MidiMsg::Cc(c, cc, v) => vec![0xb0 | c, cc, v],
            MidiMsg::Clock => vec![0xf8],
        }
    }

    pub fn to_midi20(&self) -> Vec<u8> {
        // FIXME: Implement MIDI 2.0 Universal MIDI Packet encoding.
        unimplemented!("MIDI 2.0 unsupported");
    }
}

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
        match *self {
            MidiMsg::NoteOn(chan, note, velocity) => {
                let status = 0x4; // UMP Message Type: MIDI 2.0 Channel Voice Message (32-bit)
                let opcode = 0x90; // Note On
                let group = chan;
                let mt_and_group = (status << 4) | (group & 0x0f);

                let velocity_u16 = (velocity as u16) * 0x101; // upsample 7-bit to 16-bit

                // Default values for attribute type and data (no per-note control).
                let attr_type = 0;
                let attr_data = 0;

                let word1 = ((mt_and_group as u32) << 24)
                    | (((opcode | chan) as u32) << 16)
                    | ((note as u32) << 8)
                    | ((velocity_u16 >> 8) as u32);

                let word2 = (((velocity_u16 & 0xff) as u32) << 24)
                    | ((attr_type as u32) << 16)
                    | (attr_data as u32);

                word1
                    .to_be_bytes()
                    .into_iter()
                    .chain(word2.to_be_bytes())
                    .collect()
            }
            MidiMsg::NoteOff(chan, note, velocity) => {
                let status = 0x4; // MIDI 2.0 CVM
                let opcode = 0x80; // Note Off
                let group = chan;
                let mt_and_group = (status << 4) | (group & 0x0f);

                let note_u8 = note;
                let velocity_u16 = (velocity as u16) * 0x101;

                let attr_type = 0;
                let attr_data = 0;

                let word1 = ((mt_and_group as u32) << 24)
                    | (((opcode | chan) as u32) << 16)
                    | ((note_u8 as u32) << 8)
                    | ((velocity_u16 >> 8) as u32);

                let word2 = (((velocity_u16 & 0xff) as u32) << 24)
                    | ((attr_type as u32) << 16)
                    | (attr_data as u32);

                word1
                    .to_be_bytes()
                    .into_iter()
                    .chain(word2.to_be_bytes())
                    .collect()
            }
            MidiMsg::Cc(group, cc, val) => {
                let status = 0x4;
                let opcode = 0xb0; // Control Change (CC)
                let ch = group & 0x0f;
                let mt_and_group = (status << 4) | (group & 0x0f);

                let cc_u8 = cc;
                let val_u32 = (val as u32) * 0x010101; // Upsample 7-bit to 32-bit

                let word1 = ((mt_and_group as u32) << 24)
                    | (((opcode | ch) as u32) << 16)
                    | ((cc_u8 as u32) << 8);

                let word2 = val_u32;

                word1
                    .to_be_bytes()
                    .into_iter()
                    .chain(word2.to_be_bytes())
                    .collect()
            }
            MidiMsg::Clock => {
                // No equivalent in UMP for MIDI 1.0 realtime clock.
                vec![]
            }
        }
    }
}

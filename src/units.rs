// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! Various units that are useful to express music, together with conversions to and from strings.

use crate::Result;
use std::fmt::{self, Display};
use std::str::FromStr;

/// Duration of a note.
#[derive(Clone, Debug, PartialEq)]
pub enum Duration {
    /// Raw ticks of the sequencer.
    Ticks(u64),
    /// Beats, and beats per bar. Note: This is unlike MIDI, which usually assumes a beat is a
    /// quarter note.
    Beats(u32, u32),
    /// Beginning of note duration, until explicit end.
    Begin,
    /// End of note duration (required matching Begin).
    End,
}

impl FromStr for Duration {
    type Err = String;
    fn from_str(s: &str) -> Result<Self> {
        if s == "*" {
            Ok(Duration::Begin)
        } else if s == "-" {
            Ok(Duration::End)
        } else if s.ends_with('t') {
            Ok(Duration::Ticks(
                s.trim_end_matches('t')
                    .parse()
                    .map_err(|e| format!("{}", e))?,
            ))
        } else {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() == 2 {
                Ok(Duration::Beats(
                    parts[0].parse().map_err(|e| format!("{}", e))?,
                    parts[1].parse().map_err(|e| format!("{}", e))?,
                ))
            } else {
                Err(format!("invalid duration: {}", s))
            }
        }
    }
}

impl Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Duration::Ticks(t) => write!(f, "{}t", t),
            Duration::Beats(beats, bpb) => write!(f, "{}/{}", beats, bpb),
            Duration::Begin => write!(f, "*"),
            Duration::End => write!(f, "-"),
        }
    }
}

/// Musical scales/modes: the first element is the key, with relative note of the second element.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Note {
    /// Raw MIDI note / chromatic scales.
    Raw(i8),
    /// Major scales (ionian mode).
    Maj(u8, i8),
    /// Natural minor scales (aeolian mode).
    Min(u8, i8),
    /// Harmonic minor scales.
    HMin(u8, i8),
    /// Melodic minor scales.
    MMin(u8, i8),
    /// Phrygian modes.
    Phr(u8, i8),
    // TODO: Support more.
}

impl FromStr for Note {
    type Err = String;
    fn from_str(s: &str) -> Result<Self> {
        if s.starts_with('@') {
            Ok(Note::Raw(
                s.trim_start_matches('@')
                    .parse()
                    .map_err(|e| format!("{}", e))?,
            ))
        } else if s.starts_with("maj@") {
            let parts: Vec<&str> = s.trim_start_matches("maj@").split(':').collect();
            Ok(Note::Maj(
                parts[0].parse().map_err(|e| format!("{}", e))?,
                parts[1].parse().map_err(|e| format!("{}", e))?,
            ))
        } else if s.starts_with("min@") {
            let parts: Vec<&str> = s.trim_start_matches("min@").split(':').collect();
            Ok(Note::Min(
                parts[0].parse().map_err(|e| format!("{}", e))?,
                parts[1].parse().map_err(|e| format!("{}", e))?,
            ))
        } else if s.starts_with("hmin@") {
            let parts: Vec<&str> = s.trim_start_matches("hmin@").split(':').collect();
            Ok(Note::HMin(
                parts[0].parse().map_err(|e| format!("{}", e))?,
                parts[1].parse().map_err(|e| format!("{}", e))?,
            ))
        } else if s.starts_with("mmin@") {
            let parts: Vec<&str> = s.trim_start_matches("mmin@").split(':').collect();
            Ok(Note::MMin(
                parts[0].parse().map_err(|e| format!("{}", e))?,
                parts[1].parse().map_err(|e| format!("{}", e))?,
            ))
        } else if s.starts_with("phr@") {
            let parts: Vec<&str> = s.trim_start_matches("phr@").split(':').collect();
            Ok(Note::Phr(
                parts[0].parse().map_err(|e| format!("{}", e))?,
                parts[1].parse().map_err(|e| format!("{}", e))?,
            ))
        } else {
            Err(format!("unknown note: {}", s))
        }
    }
}

impl Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Raw(v) => write!(f, "@{}", v),
            Note::Maj(key, offset) => write!(f, "maj@{}:{}", key, offset),
            Note::Min(key, offset) => write!(f, "min@{}:{}", key, offset),
            Note::HMin(key, offset) => write!(f, "hmin@{}:{}", key, offset),
            Note::MMin(key, offset) => write!(f, "mmin@{}:{}", key, offset),
            Note::Phr(key, offset) => write!(f, "phr@{}:{}", key, offset),
        }
    }
}

/// Convert a note (in scale dimension) to octave index and offset into that octave.
fn get_octave_offset(note: i8, octave_len: u8) -> (i32, i32) {
    let note = note as i32;
    let octave_len = octave_len as i32;
    let octave = if note >= 0 {
        note / octave_len
    } else {
        (note + 1) / octave_len - 1
    };
    let offset = note - octave * octave_len;
    assert!(
        offset >= 0 && offset < octave_len,
        "note={}, octave={}, offset={}",
        note,
        octave,
        offset
    );
    (octave, offset)
}

/// Convert a Note to raw MIDI note.
impl From<&Note> for Result<u8> {
    fn from(note: &Note) -> Self {
        let ret: i32 = match *note {
            Note::Raw(note) => note as i32,
            Note::Maj(key, note) => {
                let (octave, offset) = get_octave_offset(note, 7);
                (key as i32)
                    + octave * 12
                    + match offset {
                        0 => 0,
                        1 => 2,
                        2 => 4,
                        3 => 5,
                        4 => 7,
                        5 => 9,
                        6 => 11,
                        _ => unreachable!(),
                    }
            }
            Note::Min(key, note) => {
                let (octave, offset) = get_octave_offset(note, 7);
                (key as i32)
                    + octave * 12
                    + match offset {
                        0 => 0,
                        1 => 2,
                        2 => 3,
                        3 => 5,
                        4 => 7,
                        5 => 8,
                        6 => 10,
                        _ => unreachable!(),
                    }
            }
            Note::HMin(key, note) => {
                let (octave, offset) = get_octave_offset(note, 7);
                (key as i32)
                    + octave * 12
                    + match offset {
                        0 => 0,
                        1 => 2,
                        2 => 3,
                        3 => 5,
                        4 => 7,
                        5 => 8,
                        6 => 11,
                        _ => unreachable!(),
                    }
            }
            Note::MMin(key, note) => {
                let (octave, offset) = get_octave_offset(note, 9);
                (key as i32)
                    + octave * 12
                    + match offset {
                        0 => 0,
                        1 => 2,
                        2 => 3,
                        3 => 5,
                        4 => 7,
                        5 => 8,
                        6 => 9,
                        7 => 10,
                        8 => 11,
                        _ => unreachable!(),
                    }
            }
            Note::Phr(key, note) => {
                let (octave, offset) = get_octave_offset(note, 7);
                (key as i32)
                    + octave * 12
                    + match offset {
                        0 => 0,
                        1 => 1,
                        2 => 3,
                        3 => 5,
                        4 => 7,
                        5 => 8,
                        6 => 10,
                        _ => unreachable!(),
                    }
            }
        };
        if ret < 0 || ret > 127 {
            Err(format!("invalid note: {}", note))
        } else {
            Ok(ret as u8)
        }
    }
}

/// Common note velocities.
#[derive(Clone, Debug, PartialEq)]
pub enum Velocity {
    Raw(u8),
    None,
    Pppp,
    Ppp,
    Pp,
    P,
    Mp,
    Mf,
    F,
    Ff,
    Fff,
    Ffff,
}

/// Conversion of Velocity to raw MIDI velocity.
impl From<&Velocity> for u8 {
    fn from(velocity: &Velocity) -> u8 {
        match *velocity {
            Velocity::Raw(v) => v,
            Velocity::None => 0,
            Velocity::Pppp => 8,
            Velocity::Ppp => 20,
            Velocity::Pp => 31,
            Velocity::P => 42,
            Velocity::Mp => 53,
            Velocity::Mf => 64,
            Velocity::F => 80,
            Velocity::Ff => 96,
            Velocity::Fff => 112,
            Velocity::Ffff => 127,
        }
    }
}

impl FromStr for Velocity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "-" => Ok(Velocity::None),
            "pppp" => Ok(Velocity::Pppp),
            "ppp" => Ok(Velocity::Ppp),
            "pp" => Ok(Velocity::Pp),
            "p" => Ok(Velocity::P),
            "mp" => Ok(Velocity::Mp),
            "mf" => Ok(Velocity::Mf),
            "f" => Ok(Velocity::F),
            "ff" => Ok(Velocity::Ff),
            "fff" => Ok(Velocity::Fff),
            "ffff" => Ok(Velocity::Ffff),
            _ => Err(format!("unknown velocity: {}", s)),
        }
    }
}

impl Display for Velocity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Velocity::Raw(v) => write!(f, "@{}", v),
            Velocity::None => write!(f, "-"),
            Velocity::Pppp => write!(f, "pppp"),
            Velocity::Ppp => write!(f, "ppp"),
            Velocity::Pp => write!(f, "pp"),
            Velocity::P => write!(f, "p"),
            Velocity::Mp => write!(f, "mp"),
            Velocity::Mf => write!(f, "mf"),
            Velocity::F => write!(f, "f"),
            Velocity::Ff => write!(f, "ff"),
            Velocity::Fff => write!(f, "fff"),
            Velocity::Ffff => write!(f, "ffff"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn scales() {
        let assert_note_eq = |note: Note, raw| {
            let converted: Result<u8> = (&note).into();
            assert_eq!(converted, Ok(raw));
        };
        assert_note_eq(Note::Maj(60, -7), 48);
        assert_note_eq(Note::Maj(60, -2), 57);
        assert_note_eq(Note::Maj(60, -1), 59);

        assert_note_eq(Note::Maj(60, 0), 60);
        assert_note_eq(Note::Maj(60, 1), 62);
        assert_note_eq(Note::Maj(60, 2), 64);
        assert_note_eq(Note::Maj(60, 3), 65);
        assert_note_eq(Note::Maj(60, 5), 69);
        assert_note_eq(Note::Maj(60, 6), 71);
        assert_note_eq(Note::Maj(60, 7), 72);

        assert_note_eq(Note::Maj(61, -2), 58);
        assert_note_eq(Note::Maj(61, -1), 60);
        assert_note_eq(Note::Maj(61, 0), 61);
        assert_note_eq(Note::Maj(61, 6), 72);
        assert_note_eq(Note::Maj(61, 7), 73);
    }

    #[test]
    fn rand_scales() {
        let mut rng = rand::thread_rng();
        for _ in 0..10000 {
            let converted: Result<u8> =
                (&Note::Maj(rng.gen_range(55..65), rng.gen_range(-20..=20))).into();
            assert!(converted.is_ok(), "{:?}", converted);
        }
    }

    #[test]
    fn basic_string_conversions() {
        assert!("*".parse() == Ok(Duration::Begin));
        assert!("-".parse() == Ok(Duration::End));
        assert!("3t".parse() == Ok(Duration::Ticks(3)));
        assert!("1/4".parse() == Ok(Duration::Beats(1, 4)));

        assert!("@60".parse() == Ok(Note::Raw(60)));
        assert!("maj@60:-1".parse() == Ok(Note::Maj(60, -1)));
        assert!("maj@60:0".parse() == Ok(Note::Maj(60, 0)));

        assert!("-".parse() == Ok(Velocity::None));
        assert!("mf".parse() == Ok(Velocity::Mf));
        assert!("pppp".parse() == Ok(Velocity::Pppp));
        assert!("fff".parse() == Ok(Velocity::Fff));
    }
}

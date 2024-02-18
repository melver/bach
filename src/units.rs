// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use crate::Result;
use std::fmt::{self, Display};

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

impl From<&str> for Duration {
    fn from(s: &str) -> Self {
        if s == "*" {
            Duration::Begin
        } else if s == "-" {
            Duration::End
        } else if s.ends_with('t') {
            Duration::Ticks(s.trim_end_matches('t').parse().unwrap())
        } else {
            let parts: Vec<&str> = s.split('/').collect();
            Duration::Beats(parts[0].parse().unwrap(), parts[1].parse().unwrap())
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

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// Raw MIDI note.
    Raw(u8),
    /// Major scales in the first element's key, with relative note of the second element.
    Maj(u8, i8),
    // TODO: more
}

impl From<&str> for Note {
    fn from(s: &str) -> Self {
        if s.starts_with('@') {
            Note::Raw(s.trim_start_matches('@').parse().unwrap())
        } else if s.starts_with("maj@") {
            let parts: Vec<&str> = s.trim_start_matches("maj@").split(':').collect();
            Note::Maj(parts[0].parse().unwrap(), parts[1].parse().unwrap())
        } else {
            panic!("unknown note: {}", s);
        }
    }
}

impl Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Raw(v) => write!(f, "@{}", v),
            Note::Maj(key, offset) => write!(f, "maj@{}:{}", key, offset),
        }
    }
}

/// Convert a note (in scale dimension) to octave index and offset into that octave.
fn get_octave_offset(note: i8) -> (i32, i32) {
    let note_ = note as i32;
    let octave = if note_ >= 0 {
        note_ / 7
    } else {
        (note_ + 1) / 7 - 1
    };
    let offset = note_ - octave * 7;
    assert!(
        offset >= 0 && offset < 7,
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
                let (octave, offset) = get_octave_offset(note);
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
        };
        if ret < 0 || ret > 127 {
            Err("invalid Note")
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

impl From<&str> for Velocity {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "-" => Velocity::None,
            "pppp" => Velocity::Pppp,
            "ppp" => Velocity::Ppp,
            "pp" => Velocity::Pp,
            "p" => Velocity::P,
            "mp" => Velocity::Mp,
            "mf" => Velocity::Mf,
            "f" => Velocity::F,
            "ff" => Velocity::Ff,
            "fff" => Velocity::Fff,
            "ffff" => Velocity::Ffff,
            _ => panic!("unknown velocity: {}", s),
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
        assert!(Duration::from("*") == Duration::Begin);
        assert!(Duration::from("-") == Duration::End);
        assert!(Duration::from("3t") == Duration::Ticks(3));
        assert!(Duration::from("1/4") == Duration::Beats(1, 4));

        assert!(Note::from("@60") == Note::Raw(60));
        assert!(Note::from("maj@60:-1") == Note::Maj(60, -1));
        assert!(Note::from("maj@60:0") == Note::Maj(60, 0));

        assert!(Velocity::from("-") == Velocity::None);
        assert!(Velocity::from("mf") == Velocity::Mf);
        assert!(Velocity::from("pppp") == Velocity::Pppp);
        assert!(Velocity::from("fff") == Velocity::Fff);
    }
}

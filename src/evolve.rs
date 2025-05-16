// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024-2025, Marco Elver <me@marcoelver.com>

//! Common helpers for auto-evolution sequencer programs.

use crate::ga::{self, Genome};
use crate::sequencer::{Clip, ClipInst, SeqCall};
use crate::units::*;
use crate::Result;
use rand::{rngs::ThreadRng, Rng};
use std::cmp;
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

// === Config ==================================================================

#[derive(Debug)]
pub struct Config {
    pub send_clock: bool,
    pub midi_ver: u32,
    pub cc: Vec<(u8, u8, (u8, u8))>,
    pub channels: (u8, u8),
    pub beats_per_bar: u32,
    pub note_scale: Vec<Note>,
    pub note_min: Vec<Note>,
    pub note_max: Vec<Note>,
    pub chord_weight: f32,
    pub melody_weight: f32,
    pub chan_balance_weight: f32,
    pub rest_weight: f32,
    pub dup_weight: f32,
    pub skip_allocated: bool,
    pub clip_init_len: usize,
    pub clip_tail: Duration,
    pub clip_fixed_len: bool,
    pub song_continue: bool,
    pub population_size: usize,
    pub mutation_probability: f32,
    pub tournament_size: usize,
    pub population_path: String,
}

impl Config {
    pub fn new() -> Self {
        Self {
            send_clock: true,
            midi_ver: 10,
            cc: vec![],
            channels: (0, 2),
            beats_per_bar: 8,
            note_scale: vec![Note::Maj(60, 0)],
            note_min: vec![],
            note_max: vec![],
            skip_allocated: false,
            chord_weight: 1.0,
            melody_weight: 1.0,
            chan_balance_weight: 1.0,
            rest_weight: -1.0,
            dup_weight: -1.0,
            clip_init_len: 30,
            clip_fixed_len: true,
            song_continue: true,
            clip_tail: Duration::Beats(3, 1),
            population_size: 64,
            mutation_probability: 0.02,
            tournament_size: 2,
            population_path: String::new(),
        }
    }

    pub fn with_config_file(mut self) -> Self {
        let path = std::env::args().nth(1).expect("must provide config file");
        if path == "-" {
            println!("<- using default configuration: {:?}", self);
            return self;
        }

        let file = fs::File::open(path).unwrap();
        for line in io::BufReader::new(file).lines().map(|l| l.unwrap()) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            } else if let Some(suffix) = line.strip_prefix("send_clock ") {
                self.send_clock = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("midi_ver ") {
                self.midi_ver = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("cc ") {
                let parts: Vec<&str> = suffix.split(' ').collect();
                for part in &parts {
                    let args: Vec<&str> = part.split(',').collect();
                    let chan = args[0].parse().unwrap();
                    let cc = args[1].parse().unwrap();
                    let vals: Vec<u8> = args[2].split('-').map(|s| s.parse().unwrap()).collect();
                    self.cc.push((chan, cc, (vals[0], vals[1])));
                }
            } else if let Some(suffix) = line.strip_prefix("channels ") {
                let parts: Vec<&str> = suffix.split('-').collect();
                self.channels = (parts[0].parse().unwrap(), parts[1].parse().unwrap());
                assert!(self.channels.0 <= self.channels.1);
            } else if let Some(suffix) = line.strip_prefix("beats_per_bar ") {
                self.beats_per_bar = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("note_scale ") {
                self.note_scale = suffix.split(',').map(|s| s.parse().unwrap()).collect();
                assert!(!self.note_scale.is_empty());
            } else if let Some(suffix) = line.strip_prefix("note_min ") {
                self.note_min = suffix.split(',').map(|s| s.parse().unwrap()).collect();
                assert!(!self.note_min.is_empty());
            } else if let Some(suffix) = line.strip_prefix("note_max ") {
                self.note_max = suffix.split(',').map(|s| s.parse().unwrap()).collect();
                assert!(!self.note_max.is_empty());
            } else if let Some(suffix) = line.strip_prefix("chord_weight ") {
                self.chord_weight = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("melody_weight ") {
                self.melody_weight = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("chan_balance_weight ") {
                self.chan_balance_weight = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("rest_weight ") {
                self.rest_weight = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("dup_weight ") {
                self.dup_weight = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("skip_allocated ") {
                self.skip_allocated = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_init_len ") {
                self.clip_init_len = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_fixed_len ") {
                self.clip_fixed_len = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("song_continue ") {
                self.song_continue = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_tail ") {
                self.clip_tail = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("population_size ") {
                self.population_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("mutation_probability ") {
                self.mutation_probability = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("tournament_size ") {
                self.tournament_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("population_path ") {
                self.population_path = suffix.into();
            } else {
                panic!("unknown configuration: {}", line);
            }
        }

        println!("<- loaded configuration: {:?}", self);
        self
    }

    pub fn channel_count(&self) -> u8 {
        self.channels.1 - self.channels.0 + 1
    }

    pub fn beats_per_bar_order(&self) -> u32 {
        self.beats_per_bar.ilog2()
    }

    // Returns a note in the configured scale and the index into the list of note scales.
    pub fn map_note(&self, chan: u8, note: i8) -> (usize, Note) {
        let note_scale = &self.note_scale;
        let idx = chan as usize % note_scale.len();
        (
            idx,
            match note_scale[idx] {
                Note::Raw(o) => Note::Raw(o + note),
                Note::Maj(k, o) => Note::Maj(k, o + note),
                Note::Min(k, o) => Note::Min(k, o + note),
                Note::HMin(k, o) => Note::HMin(k, o + note),
                Note::MMin(k, o) => Note::MMin(k, o + note),
                Note::Phr(k, o) => Note::Phr(k, o + note),
            },
        )
    }

    pub fn population_path(&self) -> &Path {
        Path::new(&self.population_path)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

// === ClipGenome ==============================================================

#[derive(Default)]
pub struct ClipGenome {
    pub clip: Clip,
    pub init: bool,
    pub fitness: Option<f32>,
    pub comment: String,
    pub cfg: Option<&'static Config>,
}

impl From<Clip> for ClipGenome {
    fn from(v: Clip) -> Self {
        Self {
            clip: v,
            init: true,
            fitness: None,
            comment: String::new(),
            cfg: None,
        }
    }
}

impl From<&ClipGenome> for Clip {
    fn from(g: &ClipGenome) -> Self {
        g.clip.clone()
    }
}

impl ClipGenome {
    fn cfg(&self) -> &Config {
        self.cfg.unwrap()
    }

    pub fn serialize(&self, path: &Path) -> std::result::Result<(), io::Error> {
        let mut file = fs::File::create(path)?;
        writeln!(file, "# comment: {}", self.comment)?;
        if let Some(fitness) = self.fitness {
            writeln!(file, "# fitness: {}", fitness)?;
        }
        writeln!(file, ".skip_allocated {}", self.cfg().skip_allocated)?; // for bach-play
        for inst in &self.clip {
            writeln!(file, "{}", inst)?;
        }
        println!("<- wrote {}", path.display());
        Ok(())
    }

    pub fn deserialize(&mut self, path: &Path) -> Result<()> {
        let file = fs::File::open(path).map_err(|e| format!("{}", e))?;
        let mut line_num = 0;
        let mut clip: Clip = vec![];
        for line in io::BufReader::new(file).lines().map(|l| l.unwrap()) {
            line_num += 1;
            if let Some(suffix) = line.strip_prefix("# comment: ") {
                self.comment = suffix.into();
            }
            if let Some(suffix) = line.strip_prefix("# fitness: ") {
                self.fitness = Some(suffix.parse().map_err(|e| format!("{}", e))?);
            } else if line.trim().is_empty()
                || line.starts_with('#')
                || line.starts_with(".skip_allocated")
            {
                continue;
            } else {
                match line.parse() {
                    Ok(inst) => clip.push(inst),
                    Err(e) => return Err(format!("line {}: {}", line_num, e)),
                }
            }
        }
        // Only update if reading the whole file succeeded.
        self.clip = clip;
        println!("<- loaded {}", path.display());
        Ok(())
    }

    pub fn dump(&self) {
        for i in 0..self.clip.len() {
            println!("<{}> {}", i, self.clip[i]);
        }
    }

    pub fn fitness_as_str(&self) -> String {
        match self.fitness {
            Some(f) => format!("{}", f),
            None => "none".into(),
        }
    }

    fn gen_channel(&self, rng: &mut ThreadRng) -> u8 {
        rng.random_range(self.cfg().channels.0..=self.cfg().channels.1)
    }

    fn gen_note(&self, chan: u8, rng: &mut ThreadRng) -> Note {
        loop {
            // Skew probility to middle octaves.
            let x = rng.random_range(-1.0..=1.0);
            let note_offset = if rng.random_bool(0.9) {
                x * 7.9
            } else {
                let y = rng.random_range(-1.0..=1.0);
                x * y * 14.9
            } as i8;

            let (idx, note) = self.cfg().map_note(chan, note_offset);
            let raw_note = Result::from(&note).expect("try a different note_scale");

            if let Some(note_min) = self.cfg().note_min.get(idx) {
                let raw_note_min = Result::from(note_min).expect("try a different note_min");
                if raw_note < raw_note_min {
                    continue; // retry
                }
            }

            if let Some(note_max) = self.cfg().note_max.get(idx) {
                let raw_note_max = Result::from(note_max).expect("try a different note_max");
                if raw_note > raw_note_max {
                    continue; // retry
                }
            }

            return note;
        }
    }

    fn gen_note_list(&self, chan: u8, rng: &mut ThreadRng) -> Vec<Note> {
        (0..rng.random_range(1..10))
            .map(|_| self.gen_note(chan, rng))
            .collect()
    }

    fn gen_velocity(&self, rng: &mut ThreadRng) -> Velocity {
        let x = rng.random_range(-1.0..=1.0);
        let y = rng.random_range(-1.0..=1.0);
        match (x * y * 3.9) as i8 {
            -3 => Velocity::Pp,
            -2 => Velocity::P,
            -1 => Velocity::Mp,
            0 => Velocity::Mf,
            1 => Velocity::F,
            2 => Velocity::Ff,
            3 => Velocity::Fff,
            _ => unreachable!(),
        }
    }

    fn gen_duration(&self, rng: &mut ThreadRng, only_beats: bool) -> Duration {
        let beats = Duration::Beats(
            1 << rng.random_range(0..=self.cfg().beats_per_bar_order()),
            self.cfg().beats_per_bar,
        );
        if only_beats {
            beats
        } else {
            match rng.random_range(0..100) {
                0..=9 => Duration::Begin,
                10..=19 => Duration::End,
                20..=99 => beats,
                _ => unreachable!(),
            }
        }
    }

    fn gen_euclidean_params(&self, rng: &mut ThreadRng) -> (u32, u32, u32) {
        loop {
            let pulses = rng.random_range(2..=16);
            let len = rng.random_range(pulses..=32);
            let offset = rng.random_range(0..len);
            if pulses < len / 4 {
                // Too few pulses, try again.
                continue;
            }
            return (pulses, len, offset);
        }
    }

    fn gen_inst(&self, rng: &mut ThreadRng) -> ClipInst {
        // The extension range (multiplied by weight) are additional instructions that are only
        // optionally generated.
        let extension_weight = cmp::max(1, 8 / (self.cfg().cc.len() + 1));
        let extension_range = self.cfg().cc.len() * extension_weight;

        match rng.random_range(0..(100 + extension_range)) {
            0..=7 => ClipInst::Jmp(rng.random_range(-20..=0)),
            8..=19 => {
                let chan = self.gen_channel(rng);
                ClipInst::Call(SeqCall::QueueNote(
                    chan,
                    self.gen_note(chan, rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, true),
                ))
            }
            20..=49 => {
                let chan = self.gen_channel(rng);
                let eucl_params = self.gen_euclidean_params(rng);
                ClipInst::Call(SeqCall::QueueSequence(
                    chan,
                    self.gen_note_list(chan, rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, true),
                    eucl_params.0,
                    eucl_params.1,
                    eucl_params.2,
                ))
            }
            50..=99 => ClipInst::Tick(self.gen_duration(rng, true)),
            rnd => {
                let extension_idx: usize = rnd.wrapping_sub(100) / extension_weight;
                if extension_idx < self.cfg().cc.len() {
                    let (chan, control, range) = self.cfg().cc[extension_idx];
                    ClipInst::Call(SeqCall::QueueControl(
                        chan,
                        control,
                        rng.random_range(range.0..=range.1),
                    ))
                } else {
                    unreachable!();
                }
            }
        }
    }
}

impl Genome for ClipGenome {
    fn with_blueprint(mut self, blueprint: &Self) -> Self {
        // From default() or Clip.
        assert!(blueprint.cfg.is_some());
        self.cfg = blueprint.cfg;

        if !self.init {
            // From default().
            self.clip = vec![ClipInst::Jmp(0); self.cfg().clip_init_len];
            self.mutate(1.0);
            self.init = true;
        }
        self
    }

    fn mutate(&mut self, mut_prob: f32) {
        let mut rng = rand::rng();
        let mut used = HashSet::new();
        let to_mutate_precise = self.clip.len() as f32 * mut_prob;
        let mut to_mutate = to_mutate_precise as usize;
        if to_mutate == 0 && rng.random_bool(to_mutate_precise as f64) {
            to_mutate = 1;
        }
        while to_mutate != 0 {
            let idx = rng.random_range(0..self.clip.len());
            if !used.insert(idx) {
                continue;
            }

            self.clip[idx] = match &self.clip[idx] {
                ClipInst::Jmp(_) | ClipInst::Tick(_) => self.gen_inst(&mut rng),
                ClipInst::Call(SeqCall::QueueNote(chan, note, velocity, duration)) => {
                    match rng.random_range(0..=3) {
                        0 => ClipInst::Call(SeqCall::QueueNote(
                            *chan,
                            self.gen_note(*chan, &mut rng),
                            velocity.clone(),
                            duration.clone(),
                        )),
                        1 => ClipInst::Call(SeqCall::QueueNote(
                            *chan,
                            note.clone(),
                            self.gen_velocity(&mut rng),
                            duration.clone(),
                        )),
                        2 => ClipInst::Call(SeqCall::QueueNote(
                            *chan,
                            note.clone(),
                            velocity.clone(),
                            self.gen_duration(&mut rng, true),
                        )),
                        3 => self.gen_inst(&mut rng),
                        _ => unreachable!(),
                    }
                }
                ClipInst::Call(SeqCall::QueueSequence(
                    chan,
                    ref notes,
                    velocity,
                    duration,
                    pulses,
                    len,
                    offset,
                )) => match rng.random_range(0..=4) {
                    0 => ClipInst::Call(SeqCall::QueueSequence(
                        *chan,
                        self.gen_note_list(*chan, &mut rng),
                        velocity.clone(),
                        duration.clone(),
                        *pulses,
                        *len,
                        *offset,
                    )),
                    1 => ClipInst::Call(SeqCall::QueueSequence(
                        *chan,
                        notes.clone(),
                        self.gen_velocity(&mut rng),
                        duration.clone(),
                        *pulses,
                        *len,
                        *offset,
                    )),
                    2 => ClipInst::Call(SeqCall::QueueSequence(
                        *chan,
                        notes.clone(),
                        velocity.clone(),
                        self.gen_duration(&mut rng, true),
                        *pulses,
                        *len,
                        *offset,
                    )),
                    3 => {
                        let eucl_params = self.gen_euclidean_params(&mut rng);
                        ClipInst::Call(SeqCall::QueueSequence(
                            *chan,
                            notes.clone(),
                            velocity.clone(),
                            duration.clone(),
                            eucl_params.0,
                            eucl_params.1,
                            eucl_params.2,
                        ))
                    }
                    4 => self.gen_inst(&mut rng),
                    _ => unreachable!(),
                },
                ClipInst::Call(SeqCall::QueueControl(_, _, _)) => self.gen_inst(&mut rng),
            };

            to_mutate -= 1;
        }
    }

    fn fitness(&self) -> f32 {
        self.fitness.unwrap()
    }

    fn crossover(&self, other: &Self, mut_prob: f32) -> Vec<Self> {
        ga::default_crossover(
            self,
            other,
            mut_prob,
            true,
            self.cfg().clip_fixed_len,
            &mut |len| rand::rng().random_range(0..len),
        )
    }

    fn is_eval(&self) -> bool {
        self.init && self.fitness.is_some()
    }
}

// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use bach::ga::{self, Genome};
use bach::sequencer;
use bach::units::*;
use rand::{rngs::ThreadRng, Rng};
use std::cmp;
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};

#[derive(Debug)]
struct Config {
    beats_per_bar: u32,
    note_scale: Note,
    song_init_len: usize,
    population_size: usize,
    mutation_probability: f32,
    tournament_size: usize,
    tournament_winners: usize,
}

impl Config {
    fn read_from_file(&mut self) {
        let path = std::env::args().nth(1).expect("must provide config file");
        if path == "-" {
            return;
        }

        let file = fs::File::open(path).unwrap();
        for line in io::BufReader::new(file).lines().flatten() {
            if line.starts_with('#') {
                continue;
            } else if let Some(suffix) = line.strip_prefix("beats_per_bar ") {
                self.beats_per_bar = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("note_scale ") {
                self.note_scale = suffix.into();
            } else if let Some(suffix) = line.strip_prefix("song_init_len ") {
                self.song_init_len = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("population_size ") {
                self.population_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("mutation_probability ") {
                self.mutation_probability = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("tournament_size ") {
                self.tournament_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("tournament_winners ") {
                self.tournament_winners = suffix.parse().unwrap();
            } else {
                panic!("unknown configuration: {}", line);
            }
        }

        println!("loaded configuration: {:?}", self);
    }

    fn beats_per_bar_order(&self) -> u32 {
        self.beats_per_bar.ilog2()
    }

    fn map_note(&self, note: i8) -> Note {
        match cfg().note_scale {
            Note::Raw(_) => Note::Raw(note as u8),
            Note::Maj(k, _) => Note::Maj(k, note),
        }
    }
}

static mut CONFIG: Config = Config {
    beats_per_bar: 8,
    note_scale: Note::Maj(60, 0),
    song_init_len: 32,
    population_size: 12,
    mutation_probability: 0.2,
    tournament_size: 4,
    tournament_winners: 2,
};

fn cfg() -> &'static Config {
    // SAFETY: Initialized once at startup.
    unsafe { &CONFIG }
}

#[derive(Clone, Debug)]
enum SeqCommand {
    Jmp(isize),
    QueueNote(u8, Note, Velocity, Duration),
    QueueSequence(u8, Vec<Note>, Velocity, Duration, u32, u32, u32),
    Advance(Duration),
}

struct Song {
    pub cmds: Vec<SeqCommand>,
    pub init: bool,
    pub fitness: Option<f32>,
}

impl From<Vec<SeqCommand>> for Song {
    fn from(v: Vec<SeqCommand>) -> Self {
        Self {
            cmds: v,
            init: true,
            fitness: None,
        }
    }
}

impl From<&Song> for Vec<SeqCommand> {
    fn from(g: &Song) -> Self {
        g.cmds.clone()
    }
}

impl Default for Song {
    fn default() -> Self {
        Self {
            cmds: vec![SeqCommand::Jmp(0); cfg().song_init_len],
            init: false,
            fitness: None,
        }
    }
}

impl Song {
    fn gen_chan(&self, rng: &mut ThreadRng) -> u8 {
        rng.gen_range(0..=3)
    }

    fn gen_note(&self, rng: &mut ThreadRng) -> Note {
        // Skew probility to middle octaves.
        let x = rng.gen_range(-1.0..=1.0);
        let note_offset = if rng.gen_bool(0.8) {
            x * 7.9
        } else {
            let y = rng.gen_range(-1.0..=1.0);
            x * y * 30.9
        } as i8;
        let note = cfg().map_note(note_offset);
        // Detect invalid notes early.
        assert!(Result::from(&note).is_ok(), "try a different scale");
        note
    }

    fn gen_note_list(&self, rng: &mut ThreadRng) -> Vec<Note> {
        (0..rng.gen_range(1..10))
            .map(|_| self.gen_note(rng))
            .collect()
    }

    fn gen_velocity(&self, rng: &mut ThreadRng) -> Velocity {
        let x = rng.gen_range(-1.0..=1.0);
        let y = rng.gen_range(-1.0..=1.0);
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
            1 << rng.gen_range(0..=cfg().beats_per_bar_order()),
            cfg().beats_per_bar,
        );
        if only_beats {
            beats
        } else {
            match rng.gen_range(0..=100) {
                0..=9 => Duration::Begin,
                10..=19 => Duration::End,
                20..=100 => beats,
                _ => unreachable!(),
            }
        }
    }

    fn gen_euclidean_params(&self, rng: &mut ThreadRng) -> (u32, u32, u32) {
        // Constants from Godfried's paper.
        loop {
            let pulses = rng.gen_range(2..=13);
            let len = rng.gen_range(pulses..=24);
            let offset = rng.gen_range(0..len);
            if pulses < len / 4 {
                // Too few pulses, try again.
                continue;
            }
            return (pulses, len, offset);
        }
    }

    fn gen_cmd(&self, rng: &mut ThreadRng) -> SeqCommand {
        match rng.gen_range(0..=100) {
            0..=4 => SeqCommand::Jmp(rng.gen_range(-15..=5)),
            5..=9 => SeqCommand::QueueNote(
                self.gen_chan(rng),
                self.gen_note(rng),
                self.gen_velocity(rng),
                self.gen_duration(rng, false),
            ),
            10..=42 => {
                let eucl_params = self.gen_euclidean_params(rng);
                SeqCommand::QueueSequence(
                    self.gen_chan(rng),
                    self.gen_note_list(rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, true),
                    eucl_params.0,
                    eucl_params.1,
                    eucl_params.2,
                )
            }
            43..=100 => SeqCommand::Advance(self.gen_duration(rng, true)),
            _ => unreachable!(),
        }
    }
}

impl Genome for Song {
    fn with_blueprint(mut self, _blueprint: &Self) -> Self {
        // Idempotent writes.
        if !self.init {
            // From default().
            self.mutate(1.0);
            self.init = true;
        }
        self
    }

    fn mutate(&mut self, mut_prob: f32) {
        let mut rng = rand::thread_rng();
        let mut used = HashSet::new();
        let mut to_mutate = (self.cmds.len() as f32 * mut_prob) as usize;
        while to_mutate != 0 {
            let idx = rng.gen_range(0..self.cmds.len());
            if !used.insert(idx) {
                continue;
            }

            self.cmds[idx] = match self.cmds[idx] {
                SeqCommand::Jmp(_) | SeqCommand::Advance(_) => self.gen_cmd(&mut rng),
                SeqCommand::QueueNote(chan, note, velocity, duration) => {
                    match rng.gen_range(0..=3) {
                        0 => {
                            SeqCommand::QueueNote(chan, self.gen_note(&mut rng), velocity, duration)
                        }
                        1 => {
                            SeqCommand::QueueNote(chan, note, self.gen_velocity(&mut rng), duration)
                        }
                        2 => SeqCommand::QueueNote(
                            chan,
                            note,
                            velocity,
                            self.gen_duration(&mut rng, false),
                        ),
                        3 => self.gen_cmd(&mut rng),
                        _ => unreachable!(),
                    }
                }
                SeqCommand::QueueSequence(
                    chan,
                    ref notes,
                    velocity,
                    duration,
                    pulses,
                    len,
                    offset,
                ) => match rng.gen_range(0..=4) {
                    0 => SeqCommand::QueueSequence(
                        chan,
                        self.gen_note_list(&mut rng),
                        velocity,
                        duration,
                        pulses,
                        len,
                        offset,
                    ),
                    1 => SeqCommand::QueueSequence(
                        chan,
                        notes.clone(),
                        self.gen_velocity(&mut rng),
                        duration,
                        pulses,
                        len,
                        offset,
                    ),
                    2 => SeqCommand::QueueSequence(
                        chan,
                        notes.clone(),
                        velocity,
                        self.gen_duration(&mut rng, true),
                        pulses,
                        len,
                        offset,
                    ),
                    3 => {
                        let eucl_params = self.gen_euclidean_params(&mut rng);
                        SeqCommand::QueueSequence(
                            chan,
                            notes.clone(),
                            velocity,
                            duration,
                            eucl_params.0,
                            eucl_params.1,
                            eucl_params.2,
                        )
                    }
                    4 => self.gen_cmd(&mut rng),
                    _ => unreachable!(),
                },
            };

            to_mutate -= 1;
        }
    }

    fn fitness(&self) -> f32 {
        self.fitness.unwrap()
    }

    fn crossover(&self, other: &Self, mut_prob: f32) -> Vec<Self> {
        ga::default_crossover(self, other, mut_prob, true, false, &mut |len| {
            rand::thread_rng().gen_range(0..len)
        })
    }

    fn is_eval(&self) -> bool {
        self.init && self.fitness.is_some()
    }
}

fn prompt_user(prompt: &str) -> String {
    print!("{} ", prompt);
    io::stdout().flush().unwrap();
    let mut cmd = String::new();
    io::stdin().read_line(&mut cmd).unwrap();
    cmd.trim().into()
}

struct Prog {
    midi_file: fs::File,
    clock: sequencer::TickClock,
    seq: sequencer::MidiSequencer,
    pool: ga::GenomePool<Song>,
}

impl Prog {
    fn new() -> Self {
        let bpm: u32 = std::env::args()
            .nth(2)
            .expect("must provide BPM")
            .parse()
            .expect("not a valid integer");
        let ppqn: u32 = std::env::args()
            .nth(3)
            .expect("must provide PPQN")
            .parse()
            .expect("not a valid integer");
        let midi_path = std::env::args()
            .nth(4)
            .expect("must provide MIDI output device");

        Self {
            midi_file: fs::OpenOptions::new().write(true).open(midi_path).unwrap(),
            clock: sequencer::TickClock::new(bpm, ppqn),
            seq: sequencer::MidiSequencer::new(),
            pool: ga::GenomePool::new(
                Song::default(),
                cfg().population_size,
                cfg().mutation_probability,
            ),
        }
    }

    fn run(&mut self) {
        loop {
            if prompt_user(">") == "quit" {
                break;
            }

            let mut selection = self.pool.select_uniform(cfg().tournament_size);
            for song_ref in &selection {
                let song = &mut self.pool[song_ref];
                if song.1.is_eval() {
                    println!("already evald: {}", song.1.fitness.unwrap());
                    continue;
                }
                println!("gen: {}, cmds: {:?}", song.0, song.1.cmds);
                let mut cmd_idx: isize = 0;
                let mut consumed_jmp = HashSet::new();
                while cmd_idx <= song.1.cmds.len() as isize {
                    let seq_cmd = if cmd_idx < song.1.cmds.len() as isize {
                        &song.1.cmds[cmd_idx as usize]
                    } else {
                        &SeqCommand::Advance(Duration::Beats(2, 1))
                    };

                    cmd_idx += 1;

                    println!("{}: {:?}", cmd_idx, seq_cmd);
                    match seq_cmd {
                        SeqCommand::Jmp(offset) => {
                            if consumed_jmp.insert(cmd_idx) {
                                cmd_idx = cmp::max(0, cmd_idx + *offset);
                            }
                        }
                        SeqCommand::QueueNote(chan, note, velocity, duration) => {
                            match self.seq.queue(&self.clock, *chan, note, velocity, duration) {
                                Ok(()) => {}
                                Err(e) => {
                                    // Just keep playing anyway.
                                    println!("WARNING: {}", e);
                                }
                            }
                        }
                        SeqCommand::QueueSequence(
                            chan,
                            notes,
                            velocity,
                            duration,
                            pulses,
                            len,
                            offset,
                        ) => {
                            let eucl_seq = sequencer::euclidean_sequence(*pulses, *len, *offset);
                            self.seq
                                .queue_sequence(
                                    &self.clock,
                                    *chan,
                                    notes,
                                    velocity,
                                    duration,
                                    &eucl_seq,
                                    true,
                                )
                                .unwrap();
                        }
                        SeqCommand::Advance(duration) => {
                            let until_tick = match self.clock.into_ticks(duration) {
                                Some(t) => self.seq.tick + t,
                                _ => unreachable!(),
                            };
                            while self.seq.tick != until_tick {
                                let midi_bytes = self.seq.tick(&self.clock);
                                self.clock.await_tick();
                                self.midi_file.write_all(&midi_bytes).unwrap();
                                self.midi_file.flush().unwrap();
                            }
                        }
                    }
                }
                self.midi_file.write_all(&self.seq.stop()).unwrap();
                self.clock.reset();
                let song_fitness = prompt_user("fitness>").parse().unwrap();
                song.1.fitness = Some(song_fitness);
            }

            self.pool.sort_selection(&mut selection);
            let mates = &selection[0..cfg().tournament_winners];
            let replace = &selection[cfg().tournament_winners..];
            self.pool.step(mates, replace);
        }
    }
}

impl Drop for Prog {
    fn drop(&mut self) {
        // Stop all still playing notes.
        self.midi_file.write_all(&self.seq.stop()).unwrap();
    }
}

fn main() {
    unsafe {
        CONFIG.read_from_file();
    }
    Prog::new().run()
}

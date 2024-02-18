// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use bach::ga::{self, Genome};
use bach::sequencer::{self, SeqCommand};
use bach::units::*;
use bach::Result;
use rand::{rngs::ThreadRng, Rng};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

// === Config ==================================================================

#[derive(Debug)]
struct Config {
    channels: (u8, u8),
    beats_per_bar: u32,
    note_scale: Note,
    clip_init_len: usize,
    population_size: usize,
    mutation_probability: f32,
    tournament_size: usize,
    tournament_winners: usize,
    population_path: String,
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
            } else if let Some(suffix) = line.strip_prefix("channels ") {
                let parts: Vec<&str> = suffix.split('-').collect();
                self.channels = (parts[0].parse().unwrap(), parts[1].parse().unwrap());
            } else if let Some(suffix) = line.strip_prefix("beats_per_bar ") {
                self.beats_per_bar = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("note_scale ") {
                self.note_scale = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_init_len ") {
                self.clip_init_len = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("population_size ") {
                self.population_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("mutation_probability ") {
                self.mutation_probability = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("tournament_size ") {
                self.tournament_size = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("tournament_winners ") {
                self.tournament_winners = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("population_path ") {
                self.population_path = suffix.into();
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

    fn population_path(&self) -> &Path {
        Path::new(&self.population_path)
    }
}

static mut CONFIG: Config = Config {
    channels: (0, 3),
    beats_per_bar: 8,
    note_scale: Note::Maj(60, 0),
    clip_init_len: 20,
    population_size: 12,
    mutation_probability: 0.2,
    tournament_size: 4,
    tournament_winners: 2,
    population_path: String::new(),
};

fn cfg() -> &'static Config {
    // SAFETY: Initialized once at startup.
    unsafe { &CONFIG }
}

// === ClipGenome ==============================================================

struct ClipGenome {
    pub clip: sequencer::Clip,
    pub init: bool,
    pub fitness: Option<f32>,
    pub comment: String,
}

impl From<sequencer::Clip> for ClipGenome {
    fn from(v: sequencer::Clip) -> Self {
        Self {
            clip: v,
            init: true,
            fitness: None,
            comment: String::new(),
        }
    }
}

impl From<&ClipGenome> for sequencer::Clip {
    fn from(g: &ClipGenome) -> Self {
        g.clip.clone()
    }
}

impl Default for ClipGenome {
    fn default() -> Self {
        Self {
            clip: vec![SeqCommand::Jmp(0); cfg().clip_init_len],
            init: false,
            fitness: None,
            comment: String::new(),
        }
    }
}

impl ClipGenome {
    fn serialize(&self, path: &Path) -> std::result::Result<(), io::Error> {
        let mut file = fs::File::create(path)?;
        writeln!(file, "# comment: {}", self.comment)?;
        if let Some(fitness) = self.fitness {
            writeln!(file, "# fitness: {}", fitness)?;
        }
        writeln!(file, ".skip_allocated 1")?; // for bach-play
        for cmd in &self.clip {
            writeln!(file, "{}", cmd)?;
        }
        println!("wrote {}", path.display());
        Ok(())
    }

    fn deserialize(&mut self, path: &Path) -> Result<()> {
        let file = fs::File::open(path).map_err(|e| format!("{}", e))?;
        let mut line_num = 0;
        let mut clip: sequencer::Clip = vec![];
        for line in io::BufReader::new(file).lines().flatten() {
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
                    Ok(cmd) => clip.push(cmd),
                    Err(e) => return Err(format!("line {}: {}", line_num, e)),
                }
            }
        }
        // Only update if reading the whole file succeeded.
        self.clip = clip;
        println!("loaded {}", path.display());
        Ok(())
    }

    fn gen_channel(&self, rng: &mut ThreadRng) -> u8 {
        rng.gen_range(cfg().channels.0..=cfg().channels.1)
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
                self.gen_channel(rng),
                self.gen_note(rng),
                self.gen_velocity(rng),
                self.gen_duration(rng, false),
            ),
            10..=42 => {
                let eucl_params = self.gen_euclidean_params(rng);
                SeqCommand::QueueSequence(
                    self.gen_channel(rng),
                    self.gen_note_list(rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, true),
                    eucl_params.0,
                    eucl_params.1,
                    eucl_params.2,
                )
            }
            43..=100 => SeqCommand::Tick(self.gen_duration(rng, true)),
            _ => unreachable!(),
        }
    }
}

impl Genome for ClipGenome {
    fn with_blueprint(mut self, _blueprint: &Self) -> Self {
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
        let mut to_mutate = (self.clip.len() as f32 * mut_prob) as usize;
        while to_mutate != 0 {
            let idx = rng.gen_range(0..self.clip.len());
            if !used.insert(idx) {
                continue;
            }

            self.clip[idx] = match &self.clip[idx] {
                SeqCommand::Jmp(_) | SeqCommand::Tick(_) => self.gen_cmd(&mut rng),
                SeqCommand::QueueNote(chan, note, velocity, duration) => {
                    match rng.gen_range(0..=3) {
                        0 => SeqCommand::QueueNote(
                            *chan,
                            self.gen_note(&mut rng),
                            velocity.clone(),
                            duration.clone(),
                        ),
                        1 => SeqCommand::QueueNote(
                            *chan,
                            note.clone(),
                            self.gen_velocity(&mut rng),
                            duration.clone(),
                        ),
                        2 => SeqCommand::QueueNote(
                            *chan,
                            note.clone(),
                            velocity.clone(),
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
                        *chan,
                        self.gen_note_list(&mut rng),
                        velocity.clone(),
                        duration.clone(),
                        *pulses,
                        *len,
                        *offset,
                    ),
                    1 => SeqCommand::QueueSequence(
                        *chan,
                        notes.clone(),
                        self.gen_velocity(&mut rng),
                        duration.clone(),
                        *pulses,
                        *len,
                        *offset,
                    ),
                    2 => SeqCommand::QueueSequence(
                        *chan,
                        notes.clone(),
                        velocity.clone(),
                        self.gen_duration(&mut rng, true),
                        *pulses,
                        *len,
                        *offset,
                    ),
                    3 => {
                        let eucl_params = self.gen_euclidean_params(&mut rng);
                        SeqCommand::QueueSequence(
                            *chan,
                            notes.clone(),
                            velocity.clone(),
                            duration.clone(),
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

// === Prog ====================================================================

fn prompt(prompt: &str) -> String {
    print!("{} ", prompt);
    io::stdout().flush().unwrap();
    let mut cmd = String::new();
    io::stdin().read_line(&mut cmd).unwrap();
    cmd.trim().into()
}

struct Prog {
    midi_file: RefCell<fs::File>,
    clock: RefCell<sequencer::TickClock>,
    seq: RefCell<sequencer::MidiSequencer>,
    pool: RefCell<ga::GenomePool<ClipGenome>>,
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
            midi_file: RefCell::new(fs::OpenOptions::new().write(true).open(midi_path).unwrap()),
            clock: RefCell::new(sequencer::TickClock::new(bpm, ppqn)),
            seq: RefCell::new(sequencer::MidiSequencer::new()),
            pool: RefCell::new(ga::GenomePool::new(
                ClipGenome::default(),
                cfg().population_size,
                cfg().mutation_probability,
            )),
        }
    }

    fn tick_until(&self, duration: &Duration) {
        let mut clock = self.clock.borrow_mut();
        let mut seq = self.seq.borrow_mut();
        let mut midi_file = self.midi_file.borrow_mut();
        seq.tick_until(&mut clock, duration, &mut |b| {
            midi_file.write_all(b).unwrap();
            midi_file.flush().unwrap();
        });
    }

    fn play(&self, clip: &ClipGenome) {
        let mut cmd_idx: isize = 0;
        let mut skip_cmd = HashSet::new();
        while cmd_idx < clip.clip.len() as isize {
            let seq_cmd = &clip.clip[cmd_idx as usize];
            cmd_idx += 1;

            if !skip_cmd.contains(&cmd_idx) {
                println!("<{}> {}", cmd_idx, seq_cmd);
            }

            match seq_cmd {
                SeqCommand::Tick(delta) => self.tick_until(delta),
                SeqCommand::Jmp(offset) => {
                    if skip_cmd.insert(cmd_idx) {
                        cmd_idx = cmp::max(0, cmd_idx + *offset);
                    }
                }
                SeqCommand::QueueNote(chan, note, velocity, duration) => {
                    match self.seq.borrow_mut().queue(
                        &self.clock.borrow(),
                        *chan,
                        note,
                        velocity,
                        duration,
                    ) {
                        Ok(()) => {}
                        Err(e) => {
                            // Just keep playing anyway.
                            println!(":: warning: {}", e);
                        }
                    }
                }
                SeqCommand::QueueSequence(chan, notes, velocity, duration, pulses, len, offset) => {
                    let eucl_seq = sequencer::euclidean_sequence(*pulses, *len, *offset);
                    self.seq
                        .borrow_mut()
                        .queue_sequence(
                            &self.clock.borrow(),
                            *chan,
                            notes,
                            velocity,
                            duration,
                            &eucl_seq,
                            true,
                        )
                        .unwrap();
                }
            }
        }
        // Allow it to complete some of the sequences.
        println!(":: end clip");
        self.tick_until(&Duration::Beats(3, 1));
        self.stop();
    }

    fn stop(&self) {
        // Stop all still playing notes.
        let stop_clip = self.seq.borrow_mut().stop();
        let mut midi_file = self.midi_file.borrow_mut();
        midi_file.write_all(&stop_clip).unwrap();
        self.clock.borrow_mut().reset();
    }

    /// Return true if program should keep running, false otherwise.
    #[must_use]
    fn cmd_prompt(&self, mut clip: Option<&mut ClipGenome>) -> bool {
        loop {
            let prompt_str = match &clip {
                Some(s) => format!("clip[{}]>", s.fitness.unwrap_or(-999.0)),
                None => ">>>".into(),
            };
            let cmd = prompt(&prompt_str);

            match &mut clip {
                Some(clip) => {
                    if cmd == "b" {
                        return true;
                    } else if let Some(suffix) = cmd.strip_prefix('c') {
                        if suffix.is_empty() {
                            println!("current comment: {}", clip.comment);
                        } else {
                            clip.comment = suffix.trim().into();
                        }
                    } else if cmd == "d" {
                        for i in 0..clip.clip.len() {
                            println!("<{}> {}", i, clip.clip[i]);
                        }
                    } else if let Some(suffix) = cmd.strip_prefix("e ") {
                        let parts: Vec<&str> = suffix.split('=').map(|s| s.trim()).collect();
                        if parts.len() < 2 {
                            println!("missing arguments: must provide command");
                        } else {
                            match parts[0].parse::<usize>() {
                                Ok(idx) => match parts[1].parse() {
                                    Ok(cmd) => {
                                        if idx < clip.clip.len() {
                                            clip.clip[idx] = cmd;
                                        } else {
                                            println!(
                                                "index out of bounds: {} >= {}",
                                                idx,
                                                clip.clip.len()
                                            );
                                        }
                                    }
                                    Err(e) => println!("invalid command: {}", e),
                                },
                                Err(e) => println!("invalid index: {}", e),
                            }
                        }
                    } else if let Some(suffix) = cmd.strip_prefix("f ") {
                        match suffix.parse() {
                            Ok(f) => clip.fitness = Some(f),
                            Err(e) => println!("invalid argument: {}", e),
                        }
                    } else if cmd == "i" {
                        println!("comment: {}", clip.comment);
                        println!("fitness: {:?}", clip.fitness);
                    } else if let Some(suffix) = cmd.strip_prefix("l ") {
                        if let Err(e) = clip.deserialize(Path::new(suffix)) {
                            println!("could not read file: {}", e);
                        }
                    } else if cmd == "p" {
                        self.play(clip);
                    } else if let Some(suffix) = cmd.strip_prefix("w ") {
                        if let Err(e) = clip.serialize(Path::new(suffix)) {
                            println!("could not write file: {}", e);
                        }
                    } else if cmd == "q" {
                        return false;
                    } else {
                        if !cmd.is_empty() {
                            println!("unknown command: {}", cmd);
                        }
                        println!("clip help:");
                        println!("  b               : back");
                        println!("  c <comment>     : comment");
                        println!("  d               : dump");
                        println!("  e <idx> = <cmd> : edit command at index");
                        println!("  f <val>         : assign fitness value");
                        println!("  i               : info");
                        println!("  l <file>        : load from file");
                        println!("  p               : play");
                        println!("  q               : quit");
                        println!("  w <file>        : write");
                    }
                }
                None => {
                    if cmd == "c" {
                        return true;
                    } else if let Some(suffix) = cmd.strip_prefix("e ") {
                        match suffix.parse::<usize>() {
                            Ok(idx) => {
                                let mut pool = self.pool.borrow_mut();
                                let selection = pool.select_all();
                                if idx >= selection.len() {
                                    println!("index out of bounds: {} >= {}", idx, selection.len());
                                } else if !self.cmd_prompt(Some(&mut pool[&selection[idx]].1)) {
                                    return false;
                                }
                            }
                            Err(e) => println!("invalid index: {}", e),
                        }
                    } else if cmd == "i" {
                        let pool = self.pool.borrow();
                        println!("on generation: {}", pool.generation());
                        for idx in 0..pool.population().len() {
                            let clip = &pool.population()[idx];
                            println!(
                                "<{}> fitness {:?}, generation {} :: {}",
                                idx, clip.1.fitness, clip.0, clip.1.comment
                            );
                        }
                    } else if cmd == "l" {
                        if cfg().population_path.is_empty() {
                            println!("no population path set");
                        } else {
                            let mut pool = self.pool.borrow_mut();
                            let selection = pool.select_all();
                            for idx in 0..pool.population().len() {
                                let clip = &mut pool[&selection[idx]].1;
                                let path = cfg().population_path().join(format!("{}.ch", idx));
                                if let Err(e) = clip.deserialize(&path) {
                                    println!("could not load file {}: {}", path.display(), e);
                                    break;
                                }
                            }
                        }
                    } else if cmd == "q" {
                        return false;
                    } else if cmd == "w" {
                        if cfg().population_path.is_empty() {
                            println!("no population path set");
                        } else {
                            let pool = self.pool.borrow();
                            for idx in 0..pool.population().len() {
                                let clip = &pool.population()[idx].1;
                                let path = cfg().population_path().join(format!("{}.ch", idx));
                                if let Err(e) = clip.serialize(&path) {
                                    println!("could not write file {}: {}", path.display(), e);
                                    break;
                                }
                            }
                        }
                    } else {
                        if !cmd.is_empty() {
                            println!("unknown command: {}", cmd);
                        }
                        println!("main help:");
                        println!("  c       : continue");
                        println!("  e <idx> : edit clip");
                        println!("  i       : info");
                        println!("  l       : load population");
                        println!("  q       : quit");
                        println!("  w       : write population");
                    }
                }
            }
        }
    }

    fn run(&self) {
        loop {
            if !self.cmd_prompt(None) {
                break;
            }

            println!(
                ":: evaluating generation {}",
                self.pool.borrow().generation()
            );
            let mut pool = self.pool.borrow_mut();
            let mut selection = pool.select_uniform(cfg().tournament_size);
            for clip_ref in &selection {
                let clip = &mut pool[clip_ref];
                if clip.1.is_eval() {
                    continue;
                }
                assert!(clip.1.fitness.is_none());
                println!(
                    ":: begin clip :: generation {} :: {}",
                    clip.0, clip.1.comment,
                );
                self.play(&clip.1);
                while clip.1.fitness.is_none() {
                    println!(":: please set fitness ('f <fitness>')");
                    if !self.cmd_prompt(Some(&mut clip.1)) {
                        return;
                    }
                }
            }

            pool.sort_selection(&mut selection);
            let mates = &selection[0..cfg().tournament_winners];
            let replace = &selection[cfg().tournament_winners..];
            pool.step(mates, replace);
        }
    }
}

impl Drop for Prog {
    fn drop(&mut self) {
        self.stop();
    }
}

fn main() {
    unsafe {
        CONFIG.read_from_file();
    }
    Prog::new().run()
}

// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use bach::ga::{self, Genome};
use bach::sequencer::{self, SeqCommand};
use bach::units::*;
use bach::Result;
use rand::{rngs::ThreadRng, Rng};
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::cell::{Cell, RefCell};
use std::cmp;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

// === Config ==================================================================

#[derive(Debug)]
struct Config {
    channels: (u8, u8),
    beats_per_bar: u32,
    note_scale: Vec<Note>,
    skip_allocated: bool,
    clip_init_len: usize,
    clip_tail: Duration,
    clip_fixed_len: bool,
    population_size: usize,
    mutation_probability: f32,
    tournament_size: usize,
    population_path: String,
}

impl Config {
    fn init(&mut self) {
        // Defaults that need to allocate and can't be done statically.
        self.note_scale = vec![Note::Maj(60, 0)];

        let path = std::env::args().nth(1).expect("must provide config file");
        if path == "-" {
            return;
        }

        let file = fs::File::open(path).unwrap();
        for line in io::BufReader::new(file).lines().map(|l| l.unwrap()) {
            if line.starts_with('#') {
                continue;
            } else if let Some(suffix) = line.strip_prefix("channels ") {
                let parts: Vec<&str> = suffix.split('-').collect();
                self.channels = (parts[0].parse().unwrap(), parts[1].parse().unwrap());
            } else if let Some(suffix) = line.strip_prefix("beats_per_bar ") {
                self.beats_per_bar = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("note_scale ") {
                self.note_scale = suffix.split(',').map(|s| s.parse().unwrap()).collect();
                assert!(!self.note_scale.is_empty());
            } else if let Some(suffix) = line.strip_prefix("skip_allocated ") {
                self.skip_allocated = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_init_len ") {
                self.clip_init_len = suffix.parse().unwrap();
            } else if let Some(suffix) = line.strip_prefix("clip_fixed_len ") {
                self.clip_fixed_len = suffix.parse().unwrap();
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
    }

    fn beats_per_bar_order(&self) -> u32 {
        self.beats_per_bar.ilog2()
    }

    fn map_note(&self, chan: u8, note: i8) -> Note {
        let note_scale = &cfg().note_scale;
        match note_scale[chan as usize % note_scale.len()] {
            Note::Raw(o) => Note::Raw(o + note as u8),
            Note::Maj(k, o) => Note::Maj(k, o + note),
        }
    }

    fn population_path(&self) -> &Path {
        Path::new(&self.population_path)
    }
}

static mut CONFIG: Config = Config {
    channels: (0, 2),
    beats_per_bar: 8,
    note_scale: vec![],
    skip_allocated: false,
    clip_init_len: 30,
    clip_fixed_len: true,
    clip_tail: Duration::Beats(3, 1),
    population_size: 64,
    mutation_probability: 0.02,
    tournament_size: 2,
    population_path: String::new(),
};

fn cfg() -> &'static Config {
    // SAFETY: Initialized once at startup.
    unsafe { &CONFIG }
}

static mut RUNNING: AtomicBool = AtomicBool::new(true);

fn is_running() -> bool {
    unsafe { RUNNING.load(Ordering::Relaxed) }
}

fn continue_running() {
    unsafe {
        RUNNING.store(true, Ordering::Relaxed);
    }
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
        writeln!(file, ".skip_allocated {}", cfg().skip_allocated)?; // for bach-play
        for cmd in &self.clip {
            writeln!(file, "{}", cmd)?;
        }
        println!("<- wrote {}", path.display());
        Ok(())
    }

    fn deserialize(&mut self, path: &Path) -> Result<()> {
        let file = fs::File::open(path).map_err(|e| format!("{}", e))?;
        let mut line_num = 0;
        let mut clip: sequencer::Clip = vec![];
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
                    Ok(cmd) => clip.push(cmd),
                    Err(e) => return Err(format!("line {}: {}", line_num, e)),
                }
            }
        }
        // Only update if reading the whole file succeeded.
        self.clip = clip;
        println!("<- loaded {}", path.display());
        Ok(())
    }

    fn fitness_as_str(&self) -> String {
        match self.fitness {
            Some(f) => format!("{}", f),
            None => "none".into(),
        }
    }

    fn gen_channel(&self, rng: &mut ThreadRng) -> u8 {
        rng.gen_range(cfg().channels.0..=cfg().channels.1)
    }

    fn gen_note(&self, chan: u8, rng: &mut ThreadRng) -> Note {
        // Skew probility to middle octaves.
        let x = rng.gen_range(-1.0..=1.0);
        let note_offset = if rng.gen_bool(0.9) {
            x * 7.9
        } else {
            let y = rng.gen_range(-1.0..=1.0);
            x * y * 14.9
        } as i8;
        let note = cfg().map_note(chan, note_offset);
        // Detect invalid notes early.
        assert!(Result::from(&note).is_ok(), "try a different scale");
        note
    }

    fn gen_note_list(&self, chan: u8, rng: &mut ThreadRng) -> Vec<Note> {
        (0..rng.gen_range(1..10))
            .map(|_| self.gen_note(chan, rng))
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
            match rng.gen_range(0..100) {
                0..=9 => Duration::Begin,
                10..=19 => Duration::End,
                20..=99 => beats,
                _ => unreachable!(),
            }
        }
    }

    fn gen_euclidean_params(&self, rng: &mut ThreadRng) -> (u32, u32, u32) {
        loop {
            let pulses = rng.gen_range(2..=16);
            let len = rng.gen_range(pulses..=32);
            let offset = rng.gen_range(0..len);
            if pulses < len / 4 {
                // Too few pulses, try again.
                continue;
            }
            return (pulses, len, offset);
        }
    }

    fn gen_cmd(&self, rng: &mut ThreadRng) -> SeqCommand {
        match rng.gen_range(0..100) {
            0..=7 => SeqCommand::Jmp(rng.gen_range(-20..=5)),
            8..=19 => {
                let chan = self.gen_channel(rng);
                SeqCommand::QueueNote(
                    chan,
                    self.gen_note(chan, rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, false),
                )
            }
            20..=49 => {
                let chan = self.gen_channel(rng);
                let eucl_params = self.gen_euclidean_params(rng);
                SeqCommand::QueueSequence(
                    chan,
                    self.gen_note_list(chan, rng),
                    self.gen_velocity(rng),
                    self.gen_duration(rng, true),
                    eucl_params.0,
                    eucl_params.1,
                    eucl_params.2,
                )
            }
            50..=99 => SeqCommand::Tick(self.gen_duration(rng, true)),
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
        if to_mutate == 0 && rng.gen_bool(mut_prob as f64) {
            to_mutate = 1;
        }
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
                            self.gen_note(*chan, &mut rng),
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
                        self.gen_note_list(*chan, &mut rng),
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
        ga::default_crossover(
            self,
            other,
            mut_prob,
            true,
            cfg().clip_fixed_len,
            &mut |len| rand::thread_rng().gen_range(0..len),
        )
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
    /// Auto-evaluate until generation.
    eval_until: Cell<u64>,
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
            eval_until: Cell::new(0),
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

    fn stop(&self) {
        // Stop all still playing notes.
        let stop_clip = self.seq.borrow_mut().stop();
        let mut midi_file = self.midi_file.borrow_mut();
        midi_file.write_all(&stop_clip).unwrap();
        midi_file.flush().unwrap();
        self.clock.borrow_mut().reset();
    }

    fn play(&self, clip: &ClipGenome) {
        println!("<- begin clip; {}", clip.comment);
        let mut skip_cmd = HashSet::new();
        let mut cmd_idx: isize = 0;
        let mut silence = true;
        while cmd_idx < clip.clip.len() as isize {
            if !is_running() {
                return;
            }

            let seq_cmd = &clip.clip[cmd_idx as usize];
            if !skip_cmd.contains(&cmd_idx) {
                println!("<{}> {}", cmd_idx, seq_cmd);
            }

            match seq_cmd {
                SeqCommand::Tick(delta) => {
                    if silence {
                        println!("<- skipping silence");
                    } else {
                        self.tick_until(delta);
                    }
                }
                SeqCommand::Jmp(offset) => {
                    if skip_cmd.insert(cmd_idx) {
                        cmd_idx = cmp::max(0, cmd_idx + *offset);
                    }
                }
                SeqCommand::QueueNote(c, n, v, d) => {
                    let mut seq = self.seq.borrow_mut();
                    if let Err(e) = seq.queue(&self.clock.borrow(), *c, n, v, d) {
                        println!("<! failed to queue: {}", e); // Just keep playing.
                    }
                }
                SeqCommand::QueueSequence(c, ns, v, d, p, l, o) => {
                    let eucl = sequencer::euclidean_sequence(*p, *l, *o);
                    let clock = self.clock.borrow();
                    if let Err(e) = self.seq.borrow_mut().queue_sequence(
                        &clock,
                        *c,
                        ns,
                        v,
                        d,
                        &eucl,
                        cfg().skip_allocated,
                    ) {
                        println!("<! failed to queue: {}", e);
                    }
                }
            }

            // Skip initial silence. A jump does count as a non-silence, and can be used to
            // deliberately introduce silence at the beginning.
            silence = if let SeqCommand::Tick(_) = seq_cmd {
                silence
            } else {
                false
            };

            cmd_idx += 1;
        }
        // Allow it to complete some of the sequences.
        println!("<- end clip");
        self.tick_until(&cfg().clip_tail);

        // Do not stop() here, so that chained clips sound smoother. Need to explicitly call
        // stop() where needed.
    }

    /// Evaluate the fitness of a clip without manual feedback based on some generic heuristics of
    /// what sounds good (which is of course rather subjective).
    fn eval(&self, clip: &mut ClipGenome) {
        let mut multiplier = 1.0;

        // Translate into nicer representation to analyze. Each element corresponds to the shortest
        // beat, and each entry contains a list of notes that are playing.
        let sheet = {
            let mut sheet: Vec<Vec<Note>> = vec![];
            let mut cur_beat: u32 = 0;
            let mut insert_sheet = |idx: u32, note: Note| {
                let idx_ = idx as usize;
                if idx_ >= sheet.len() {
                    sheet.resize(idx_ + 1, vec![]);
                }
                sheet[idx_].push(note);
            };
            let mut skip_cmd = HashSet::new();
            let mut cmd_idx: isize = 0;
            while cmd_idx < clip.clip.len() as isize {
                match &clip.clip[cmd_idx as usize] {
                    SeqCommand::Tick(delta) => {
                        // We still have to forward the sequencer to accurately detect if there are
                        // errors when we try to queue notes.
                        let mut clock = self.clock.borrow_mut();
                        let mut seq = self.seq.borrow_mut();
                        seq.forward_until(&mut clock, delta, &mut |_| ());
                        match delta {
                            Duration::Beats(b, bpb) if *bpb == cfg().beats_per_bar => {
                                cur_beat += *b
                            }
                            _ => panic!("unexpected delta: {}", delta),
                        }
                    }
                    SeqCommand::Jmp(offset) => {
                        if skip_cmd.insert(cmd_idx) {
                            cmd_idx = cmp::max(0, cmd_idx + *offset);
                        }
                        // Too many jumps can easily make it boring.
                        multiplier *= 0.95;
                    }
                    SeqCommand::QueueNote(c, n, v, d) => {
                        let mut seq = self.seq.borrow_mut();
                        if seq.queue(&self.clock.borrow(), *c, n, v, d).is_err() {
                            multiplier *= 0.9;
                        } else if let Duration::Beats(beats, _) = d {
                            for b in 0..*beats {
                                insert_sheet(cur_beat + b, n.clone());
                            }
                        } else {
                            // Penalize untimed starts and stops.
                            multiplier *= 0.9;
                        }
                    }
                    SeqCommand::QueueSequence(c, ns, v, d, p, l, o) => {
                        let eucl = sequencer::euclidean_sequence(*p, *l, *o);
                        let clock = self.clock.borrow();
                        let mut seq = self.seq.borrow_mut();
                        if seq
                            .queue_sequence(&clock, *c, ns, v, d, &eucl, false)
                            .is_err()
                        {
                            multiplier *= 0.9;
                        } else if let Duration::Beats(beats, _) = d {
                            let mut notes = ns.iter().cycle();
                            let mut beat_offset = 0;
                            for &pulse in &eucl {
                                if pulse {
                                    let note = notes.next().unwrap();
                                    for b in 0..*beats {
                                        insert_sheet(cur_beat + beat_offset + b, note.clone());
                                    }
                                }
                                beat_offset += beats;
                            }
                        } else {
                            panic!("{}", d);
                        }
                    }
                }

                cmd_idx += 1;
            }
            // Reset single instance of sequencer and clock.
            let _ = self.seq.borrow_mut().stop();
            self.clock.borrow_mut().reset();
            // Make it learn to insert "advance" at the end.
            sheet.resize(cur_beat as usize + 1, vec![]);
            assert_eq!(cur_beat as usize + 1, sheet.len());
            sheet
        };

        // Now we can analyze the flattened view of sequenced notes.
        let mut fitness = 0.0;

        // The harmony table assigns scores to note intervals (in semitone offsets).
        let harmony_table = HashMap::from([
            (0, -0.20),
            (1, 0.05),
            (2, 0.05),
            (3, 0.50),
            (4, 0.50),
            (5, 0.30),
            (6, -0.10),
            (7, 0.50),
            (8, 0.10),
            (9, 0.40),
            (10, -0.02),
            (11, -0.02),
            (12, 0.10),
            (13, -0.05),
            (14, 0.05),
            (15, 0.05),
            (16, 0.50),
            (17, 0.50),
            (18, 0.30),
            (19, -0.10),
            (20, 0.50),
            (21, 0.10),
            (22, 0.40),
            (23, -0.02),
            (24, -0.02),
            (25, 0.10),
            (26, -0.05),
            (27, 0.05),
            (28, 0.05),
            (29, 0.50),
            (30, 0.50),
            (31, 0.30),
            (32, -0.10),
            (33, 0.50),
            (34, 0.10),
            (35, 0.40),
            (36, -0.02),
            (37, -0.02),
            (38, 0.10),
            (39, -0.05),
            (40, 0.05),
            (41, 0.05),
            (42, 0.50),
            (43, 0.50),
            (44, 0.30),
            (45, -0.10),
            (46, 0.50),
            (47, 0.10),
            (48, 0.40),
            (49, -0.02),
            (50, -0.02),
        ]);

        // Calculate harmony score for simultanous notes (chords)
        fitness += {
            let mut chord_score = 0.0;
            for beat_notes in &sheet {
                for i in 0..beat_notes.len() {
                    let note1 = &beat_notes[i];
                    for note2 in beat_notes.iter().skip(i + 1) {
                        let raw1 = <Result<u8>>::from(note1).unwrap() as i8;
                        let raw2 = <Result<u8>>::from(note2).unwrap() as i8;
                        let diff = (raw1 - raw2).abs();
                        match harmony_table.get(&diff) {
                            // Divide by the size of this chord, so that it prefers smaller but
                            // overall better sounding chords.
                            Some(score) => {
                                let difficulty = beat_notes.len() as f32;
                                if *score > 0.0 {
                                    chord_score += score / difficulty;
                                } else {
                                    chord_score += score * difficulty;
                                }
                            }
                            // Warn, so we may add the missing data in future.
                            None => {
                                multiplier *= 0.9;
                                println!("<! no harmony score for interval of {}", diff);
                            }
                        }
                    }
                }
            }
            chord_score
        };

        // Calculate harmony score for non-simultaneous notes (melody/arp).
        fitness += {
            let mut melody_score = 0.0;
            for i in 1..sheet.len() {
                let beat_notes1 = &sheet[i - 1];
                let beat_notes2 = &sheet[i];
                for note1 in beat_notes1 {
                    for note2 in beat_notes2 {
                        let raw1 = <Result<u8>>::from(note1).unwrap() as i8;
                        let raw2 = <Result<u8>>::from(note2).unwrap() as i8;
                        let diff = (raw1 - raw2).abs();
                        match harmony_table.get(&diff) {
                            Some(score) => {
                                let difficulty = (beat_notes1.len() + beat_notes2.len()) as f32;
                                if *score > 0.0 {
                                    melody_score += score / difficulty;
                                } else {
                                    melody_score += score;
                                }
                            }
                            None => {
                                multiplier *= 0.9;
                                println!("<! no harmony score for interval of {}", diff);
                            }
                        }
                    }
                }
            }
            melody_score
        };

        // Penalize too many rests.
        fitness -= (sheet.iter().filter(|e| e.is_empty()).count() as f32) / (sheet.len() as f32);

        // Normalize fitness against length.
        if sheet.is_empty() {
            // Remove them instantly.
            fitness = -1e6;
        } else {
            // Prefer shorter but higher density sequences.
            fitness /= 1.0 + (sheet.len() as f32).log(1.2);
        }

        if clip.clip.len() > 150 {
            // Things will become slow if too large. But we also don't want to discard the
            // information in potentially good genomes, so just slightly penalize them.
            //
            // It can still get to long clips by using jumps (which are also penalized a little to
            // avoid too much repetition).
            multiplier *= 0.9;
        }

        assert_ne!(fitness, f32::INFINITY);
        clip.fitness = Some(multiplier * fitness);
    }

    /// Return true if program should keep running, false otherwise.
    #[must_use]
    fn cmd_prompt(&self, mut clip: Option<&mut ClipGenome>) -> bool {
        loop {
            let prompt_str = match &clip {
                Some(clip) => format!("clip[{}]={}>>", clip.clip.len(), clip.fitness_as_str()),
                None => ">>>".into(),
            };
            let cmd = prompt(&prompt_str);

            // The command prompt can be exit with 'q'.
            continue_running();

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
                            println!("<! missing arguments: must provide command");
                        } else {
                            match parts[0].parse::<usize>() {
                                Ok(idx) => match parts[1].parse() {
                                    Ok(cmd) => {
                                        if idx < clip.clip.len() {
                                            clip.clip[idx] = cmd;
                                        } else {
                                            println!(
                                                "<! index out of bounds: {} >= {}",
                                                idx,
                                                clip.clip.len()
                                            );
                                        }
                                    }
                                    Err(e) => println!("<! invalid command: {}", e),
                                },
                                Err(e) => println!("<! invalid index: {}", e),
                            }
                        }
                    } else if let Some(suffix) = cmd.to_lowercase().strip_prefix("f ") {
                        match suffix.parse() {
                            Ok(f) => clip.fitness = Some(f),
                            Err(e) => println!("<! invalid argument: {}", e),
                        }
                        if cmd.starts_with('F') {
                            return true;
                        }
                    } else if cmd == "i" {
                        println!("comment: {}", clip.comment);
                        println!("fitness: {}", clip.fitness_as_str());
                    } else if let Some(suffix) = cmd.strip_prefix("l ") {
                        if let Err(e) = clip.deserialize(Path::new(suffix)) {
                            println!("<! could not read file: {}", e);
                        }
                    } else if cmd == "p" {
                        self.play(clip);
                        self.stop();
                    } else if let Some(suffix) = cmd.strip_prefix("w ") {
                        if let Err(e) = clip.serialize(Path::new(suffix)) {
                            println!("<! could not write file: {}", e);
                        }
                    } else if cmd == "q" {
                        return false;
                    } else {
                        if !cmd.is_empty() {
                            println!("<! unknown command: {}", cmd);
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
                    if let Some(suffix) = cmd.strip_prefix("a ") {
                        let pool = self.pool.borrow();
                        match suffix.parse::<u64>() {
                            Ok(gen) => self.eval_until.set(pool.generation() + gen),
                            Err(e) => println!("<! invalid generation: {}", e),
                        }
                        return true;
                    } else if let Some(suffix) = cmd.strip_prefix("bpm") {
                        let mut clock = self.clock.borrow_mut();
                        if suffix.is_empty() {
                            println!("BPM: {}", clock.bpm);
                        } else {
                            match suffix.trim().parse() {
                                Ok(val) => {
                                    if val != 0 {
                                        clock.bpm = val;
                                    } else {
                                        println!("<! cannot set BPM to 0");
                                    }
                                }
                                Err(e) => println!("<! invalid BPM: {}", e),
                            }
                        }
                    } else if cmd == "c" {
                        return true;
                    } else if let Some(suffix) = cmd.strip_prefix("e ") {
                        let mut pool = self.pool.borrow_mut();
                        if suffix == "best" {
                            let best_ref = pool.select_best();
                            if !self.cmd_prompt(Some(&mut pool[&best_ref].1)) {
                                return false;
                            }
                        } else {
                            match suffix.parse::<usize>() {
                                Ok(idx) => {
                                    let population = pool.population_mut();
                                    if idx >= population.len() {
                                        println!(
                                            "<! index out of bounds: {} >= {}",
                                            idx,
                                            population.len()
                                        );
                                    } else if !self.cmd_prompt(Some(&mut population[idx].1)) {
                                        return false;
                                    }
                                }
                                Err(e) => println!("<! invalid index: {}", e),
                            }
                        }
                    } else if cmd == "i" {
                        let pool = self.pool.borrow();
                        println!("generation: {}", pool.generation());
                        println!("mean fitness: {}", pool.mean_fitness());
                        println!("best fitness: {}", pool.best_fitness());
                        println!("worst fitness: {}", pool.worst_fitness());
                        for idx in 0..pool.population().len() {
                            let clip = &pool.population()[idx];
                            println!(
                                "<{}> length {}, fitness {}, generation {} :: {}",
                                idx,
                                clip.1.clip.len(),
                                clip.1.fitness_as_str(),
                                clip.0,
                                clip.1.comment
                            );
                        }
                    } else if let Some(suffix) = cmd.strip_prefix("l ") {
                        let limit = if suffix == "*" {
                            usize::MAX
                        } else {
                            match suffix.parse() {
                                Ok(val) => val,
                                Err(e) => {
                                    println!("<! invalid count: {}", e);
                                    0
                                }
                            }
                        };
                        if cfg().population_path.is_empty() {
                            println!("<! no population path set");
                        } else {
                            let mut pool = self.pool.borrow_mut();
                            let population = pool.population_mut();
                            for (idx, clip) in population.iter_mut().enumerate() {
                                if idx >= limit {
                                    break;
                                }
                                let path = cfg().population_path().join(format!("{}.ch", idx));
                                if let Err(e) = clip.1.deserialize(&path) {
                                    println!("<! could not load file {}: {}", path.display(), e);
                                    break;
                                }
                            }
                        }
                    } else if let Some(suffix) = cmd.strip_prefix("mut") {
                        let mut pool = self.pool.borrow_mut();
                        if suffix.is_empty() {
                            println!("mutation probability: {}", pool.mut_prob);
                        } else {
                            match suffix.trim().parse() {
                                Ok(val) => {
                                    if val >= 0.0 && val <= 1.0 {
                                        pool.mut_prob = val;
                                    } else {
                                        println!(
                                            "<! mutation probability must be between 0.0 and 1.0"
                                        );
                                    }
                                }
                                Err(e) => println!("<! invalid mutation probability: {}", e),
                            }
                        }
                    } else if cmd == "q" {
                        return false;
                    } else if let Some(suffix) = cmd.strip_prefix("s ") {
                        for part in suffix.split(',') {
                            if !is_running() {
                                break;
                            }
                            match part.parse::<usize>() {
                                Ok(idx) => {
                                    let pool = self.pool.borrow();
                                    if idx >= pool.population().len() {
                                        println!(
                                            "<! index out of bounds: {} >= {}",
                                            idx,
                                            pool.population().len()
                                        );
                                        break;
                                    }
                                    let clip = &pool.population()[idx].1;
                                    self.play(clip);
                                }
                                Err(e) => {
                                    println!("<! invalid index: {}", e);
                                    break;
                                }
                            }
                        }
                        self.stop();
                    } else if cmd == "w" {
                        if cfg().population_path.is_empty() {
                            println!("<! no population path set");
                        } else {
                            let pool = self.pool.borrow();
                            for idx in 0..pool.population().len() {
                                let clip = &pool.population()[idx].1;
                                let path = cfg().population_path().join(format!("{}.ch", idx));
                                if let Err(e) = clip.serialize(&path) {
                                    println!("<! could not write file {}: {}", path.display(), e);
                                    break;
                                }
                            }
                        }
                    } else {
                        if !cmd.is_empty() {
                            println!("<! unknown command: {}", cmd);
                        }
                        println!("main help:");
                        println!("  a <count>   : auto-evolve next <count> generations");
                        println!("  bpm <val>   : change BPM");
                        println!("  c           : continue");
                        println!("  e <idx>     : edit clip");
                        println!("  i           : info");
                        println!("  l <count>   : load <count> genomes into population");
                        println!("  mut <val>   : change mutation probability to <val>");
                        println!("  q           : quit");
                        println!("  s <idx>,... : play chained clips (song mode)");
                        println!("  w           : write population");
                    }
                }
            }
        }
    }

    fn run(&self) {
        let mut auto_eval = false;
        loop {
            if !auto_eval && !self.cmd_prompt(None) {
                break;
            }

            let mut pool = self.pool.borrow_mut();
            if pool.generation() < self.eval_until.get() && is_running() {
                auto_eval = true;
            } else if auto_eval {
                // Auto eval has finished. Back to the main prompt.
                auto_eval = false;
                continue;
            }

            // One genome wins from a set of tournament_size, but we need 2 genomes to replace the
            // deleted genome.
            let selection = pool.select_uniform(cfg().tournament_size * 2);
            println!("<- advancing to generation {}", pool.generation() + 1);
            for clip_ref in &selection {
                let clip = &mut pool[clip_ref];
                if clip.1.is_eval() {
                    continue;
                }
                assert!(clip.1.fitness.is_none());
                println!("<- evaluating clip from generation {}", clip.0);

                if auto_eval {
                    self.eval(&mut clip.1);
                } else {
                    self.play(&clip.1);
                    self.stop();
                    while clip.1.fitness.is_none() {
                        println!("<? please set fitness");
                        if !self.cmd_prompt(Some(&mut clip.1)) {
                            return;
                        }
                    }
                }
            }

            // Steady-state with tournament-selection and the delete oldest replacement strategy.
            let mut mates = vec![];
            for group in (0..selection.len()).step_by(cfg().tournament_size) {
                let mut tournament = selection[group..group + cfg().tournament_size].to_vec();
                pool.sort_selection(&mut tournament);
                mates.push(*tournament.first().unwrap());
            }
            assert_eq!(mates.len(), 2);
            let replace = &[pool.select_oldest()];
            pool.step(&mates, replace);
            println!(
                "<- advanced to generation {} with mean fitness {}",
                pool.generation(),
                pool.mean_fitness()
            );
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
        CONFIG.init();
    }

    let mut signals = Signals::new([SIGINT]).unwrap();
    let sig_handle = signals.handle();
    let sig_handler = thread::spawn(move || {
        for _ in &mut signals {
            println!("<- stopping ...");
            unsafe {
                RUNNING.store(false, Ordering::Relaxed);
            }
        }
    });

    Prog::new().run();
    sig_handle.close();
    sig_handler.join().unwrap();
}

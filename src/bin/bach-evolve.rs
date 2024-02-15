use bach::ga::{self, Genome};
use bach::sequencer;
use bach::units::*;
use rand::{rngs::ThreadRng, Rng};
use std::cmp;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::rc::Rc;

const BEATS_PER_BAR: u32 = 8;
const BEATS_PER_BAR_ORDER: u32 = 3;
const SONG_INIT_LEN: usize = 32;
const POPULATION_SIZE: usize = 20;
const MUTATION_PROBABILITY: f32 = 0.1;
const TOURNAMENT_SIZE: usize = 5;
const TOURNAMENT_WINNERS: usize = 2;

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
    pub map_note: Option<Rc<dyn Fn(i8) -> Note>>,
}

impl From<Vec<SeqCommand>> for Song {
    fn from(v: Vec<SeqCommand>) -> Self {
        Self {
            cmds: v,
            init: true,
            fitness: None,
            map_note: None,
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
            cmds: vec![SeqCommand::Jmp(0); SONG_INIT_LEN],
            init: false,
            fitness: None,
            map_note: None,
        }
    }
}

impl Song {
    fn with_map_note(mut self, map_note: Rc<dyn Fn(i8) -> Note>) -> Self {
        self.map_note = Some(map_note);
        self
    }

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
        let note = (self.map_note.as_ref().unwrap())(note_offset);
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
        let beats = Duration::Beats(1 << rng.gen_range(0..=BEATS_PER_BAR_ORDER), BEATS_PER_BAR);
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
    fn with_blueprint(mut self, blueprint: &Self) -> Self {
        // Idempotent writes.
        self.map_note = blueprint.map_note.clone();
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

fn main() {
    let bpm: u32 = std::env::args()
        .nth(1)
        .expect("must provide BPM")
        .parse()
        .expect("not a valid integer");
    let ppqn: u32 = std::env::args()
        .nth(2)
        .expect("must provide PPQN")
        .parse()
        .expect("not a valid integer");
    let scale = std::env::args().nth(3).expect("must provide scale");
    let midi_path = std::env::args()
        .nth(4)
        .expect("must provide MIDI output device");

    let map_note: Rc<dyn Fn(i8) -> Note> = match scale.as_str().into() {
        Note::Raw(_) => Rc::new(|n| Note::Raw(n as u8)),
        Note::Maj(k, _) => Rc::new(move |n| Note::Maj(k, n)),
    };

    let mut midi_file = OpenOptions::new().write(true).open(midi_path).unwrap();
    let mut clock = sequencer::TickClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::new();
    let mut pool = ga::GenomePool::new(
        Song::default().with_map_note(map_note),
        POPULATION_SIZE,
        MUTATION_PROBABILITY,
    );

    loop {
        if prompt_user(">") == "quit" {
            break;
        }

        let mut selection = pool.select_uniform(TOURNAMENT_SIZE);
        for song_ref in &selection {
            let song = &mut pool[song_ref];
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
                        match seq.queue(&clock, *chan, note, velocity, duration) {
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
                        seq.queue_sequence(
                            &clock, *chan, notes, velocity, duration, &eucl_seq, true,
                        )
                        .unwrap();
                    }
                    SeqCommand::Advance(duration) => {
                        let until_tick = match clock.into_ticks(&duration) {
                            Some(t) => seq.tick + t,
                            _ => unreachable!(),
                        };
                        while seq.tick != until_tick {
                            let midi_bytes = seq.tick(&clock);
                            clock.await_tick();
                            midi_file.write_all(&midi_bytes).unwrap();
                            midi_file.flush().unwrap();
                        }
                    }
                }
            }
            midi_file.write_all(&seq.stop()).unwrap();
            clock.reset();
            let song_fitness = prompt_user("fitness>").parse().unwrap();
            song.1.fitness = Some(song_fitness);
        }

        pool.sort_selection(&mut selection);
        let mates = &selection[0..TOURNAMENT_WINNERS];
        let elite = &selection[TOURNAMENT_WINNERS..];
        pool.step(mates, elite);
    }

    // Stop all still playing notes.
    midi_file.write_all(&seq.stop()).unwrap();
}

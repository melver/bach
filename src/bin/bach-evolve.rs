use bach::ga::{self, Genome};
use bach::sequencer;
use bach::units::*;
use rand::{rngs::ThreadRng, Rng};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::rc::Rc;

#[derive(Clone, Debug)]
enum SeqCommand {
    Nop,
    QueueNote(u8, Note, Velocity, Duration),
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
            cmds: vec![SeqCommand::Nop; 32],
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
        // TODO: weights
        rng.gen_range(0..9)
    }

    fn gen_note(&self, rng: &mut ThreadRng) -> Note {
        let note = (self.map_note.as_ref().unwrap())(rng.gen_range(-30..30));
        // Detect invalid notes early.
        assert!(Result::from(&note).is_ok(), "try a different scale");
        note
    }

    fn gen_velocity(&self, rng: &mut ThreadRng) -> Velocity {
        match rng.gen_range(0..=100) {
            0 => Velocity::Pppp,
            1..=9 => Velocity::Ppp,
            10..=19 => Velocity::Pp,
            20..=29 => Velocity::P,
            30..=39 => Velocity::Mp,
            40..=69 => Velocity::Mf,
            70..=79 => Velocity::F,
            80..=89 => Velocity::Ff,
            90..=99 => Velocity::Fff,
            100 => Velocity::Ffff,
            _ => unreachable!(),
        }
    }

    fn gen_duration(&self, rng: &mut ThreadRng) -> Duration {
        match rng.gen_range(0..=100) {
            0..=9 => Duration::Begin,
            10..=19 => Duration::End,
            20..=100 => Duration::Beats(1 << rng.gen_range(0..=5), 16),
            _ => unreachable!(),
        }
    }

    fn gen_cmd(&self, rng: &mut ThreadRng) -> SeqCommand {
        match rng.gen_range(0..=100) {
            0..=5 => SeqCommand::Nop,
            6..=59 => SeqCommand::QueueNote(
                self.gen_chan(rng),
                self.gen_note(rng),
                self.gen_velocity(rng),
                self.gen_duration(rng),
            ),
            60..=100 => SeqCommand::Advance(Duration::Beats(1 << rng.gen_range(0..=3), 16)),
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
                SeqCommand::Nop | SeqCommand::Advance(_) => self.gen_cmd(&mut rng),
                SeqCommand::QueueNote(chan, note, velocity, duration) => {
                    match rng.gen_range(0..=4) {
                        0 => {
                            SeqCommand::QueueNote(self.gen_chan(&mut rng), note, velocity, duration)
                        }
                        1 => {
                            SeqCommand::QueueNote(chan, self.gen_note(&mut rng), velocity, duration)
                        }
                        2 => {
                            SeqCommand::QueueNote(chan, note, self.gen_velocity(&mut rng), duration)
                        }
                        3 => {
                            SeqCommand::QueueNote(chan, note, velocity, self.gen_duration(&mut rng))
                        }
                        4 => SeqCommand::Nop,
                        _ => unreachable!(),
                    }
                }
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
    let mut pool = ga::GenomePool::new(Song::default().with_map_note(map_note), 30, 0.02);

    // TODO: change this
    let tournament_size = 4;
    let tournament_winners = 2;

    loop {
        if prompt_user(">") == "quit" {
            break;
        }

        let mut selection = pool.select_uniform(tournament_size);
        for song_ref in &selection {
            let song = &mut pool[song_ref];
            if song.1.is_eval() {
                println!("already evald: {}", song.1.fitness.unwrap());
                continue;
            }
            println!("gen: {}, cmds: {:?}", song.0, song.1.cmds);
            let mut err = false;
            for seq_cmd in &song.1.cmds {
                println!("{:?}", seq_cmd);
                match seq_cmd {
                    SeqCommand::Nop => {}
                    SeqCommand::QueueNote(chan, note, velocity, duration) => {
                        match seq.queue(*chan, note, velocity, duration, &clock) {
                            Ok(()) => {}
                            Err(e) => {
                                println!("err: {}", e);
                                err = true;
                                break;
                            }
                        }
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
            if err {
                song.1.fitness = Some(song_fitness / 2.0);
            } else {
                song.1.fitness = Some(song_fitness);
            }
        }

        pool.sort_selection(&mut selection);
        let mates = &selection[0..tournament_winners];
        let elite = &selection[tournament_winners..];
        pool.step(mates, elite);
    }

    // Stop all still playing notes.
    midi_file.write_all(&seq.stop()).unwrap();
}

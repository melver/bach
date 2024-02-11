use bach::ga::{self, Genome};
use bach::sequencer;
use bach::units::*;
use rand::{rngs::ThreadRng, Rng};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};

#[derive(Clone, Debug)]
enum SeqCommand {
    Nop,
    QueueNote(u8, Note, Velocity, Duration),
    Advance(Duration),
}

#[derive(Debug)]
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
            cmds: vec![SeqCommand::Nop; 32],
            init: false,
            fitness: None,
        }
    }
}

fn gen_chan(rng: &mut ThreadRng) -> u8 {
    // TODO: weights
    rng.gen_range(0..9)
}

fn gen_note(rng: &mut ThreadRng) -> Note {
    Note::Raw(rng.gen_range(0..=127))
}

fn gen_velocity(rng: &mut ThreadRng) -> Velocity {
    // TODO: weights
    Velocity::Raw(rng.gen_range(1..=127))
}

fn gen_duration(rng: &mut ThreadRng) -> Duration {
    // TODO: weights
    match rng.gen_range(0..=2) {
        0 => Duration::Begin,
        1 => Duration::End,
        2 => {
            let beats_per_bar = 1 << rng.gen_range(0..=5); // up to 32
            Duration::Beats(rng.gen_range(1..=(beats_per_bar * 2)), beats_per_bar)
        }
        _ => unreachable!(),
    }
}

fn gen_cmd(rng: &mut ThreadRng) -> SeqCommand {
    // TODO: weights
    match rng.gen_range(0..=2) {
        0 => SeqCommand::Nop,
        1 => SeqCommand::QueueNote(
            gen_chan(rng),
            gen_note(rng),
            gen_velocity(rng),
            gen_duration(rng),
        ),
        2 => SeqCommand::Advance(Duration::Beats(rng.gen_range(1..=32), 32)),
        _ => unreachable!(),
    }
}

impl Genome for Song {
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
        let mut to_mutate = (self.cmds.len() as f32 * mut_prob) as usize;
        while to_mutate != 0 {
            let idx = rng.gen_range(0..self.cmds.len());
            if !used.insert(idx) {
                continue;
            }

            self.cmds[idx] = match self.cmds[idx] {
                SeqCommand::Nop => gen_cmd(&mut rng),
                SeqCommand::QueueNote(chan, note, velocity, duration) => {
                    match rng.gen_range(0..=4) {
                        0 => SeqCommand::QueueNote(gen_chan(&mut rng), note, velocity, duration),
                        1 => SeqCommand::QueueNote(chan, gen_note(&mut rng), velocity, duration),
                        2 => SeqCommand::QueueNote(chan, note, gen_velocity(&mut rng), duration),
                        3 => SeqCommand::QueueNote(chan, note, velocity, gen_duration(&mut rng)),
                        4 => SeqCommand::Nop,
                        _ => unreachable!(),
                    }
                }
                SeqCommand::Advance(_) => match rng.gen_range(0..=1) {
                    0 => SeqCommand::Advance(gen_duration(&mut rng)),
                    1 => SeqCommand::Nop,
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
    let midi_path = std::env::args()
        .nth(3)
        .expect("must provide MIDI output device");

    let mut midi_file = OpenOptions::new().write(true).open(midi_path).unwrap();
    let mut clock = sequencer::TickClock::new(bpm, ppqn);
    let mut seq = sequencer::MidiSequencer::new();
    let mut pool = ga::GenomePool::new(Song::default(), 30, 0.02);

    // TODO: change this
    let tournamet_size = 4;
    let tournament_winners = 2;

    loop {
        if prompt_user(">") == "quit" {
            break;
        }

        let mut selection = pool.select_uniform(tournamet_size);
        for song_ref in &selection {
            let song = &mut pool[song_ref];
            if song.1.is_eval() {
                println!("already evald: {}", song.1.fitness.unwrap());
                continue;
            }
            println!("{:?}", song);
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
            if err {
                song.1.fitness = Some(-999.0);
            } else {
                let song_fitness = prompt_user("fitness>").parse().unwrap();
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

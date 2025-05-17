// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

//! Command line utility to generate, evolve, and edit music clips. The CLI aims to be like a
//! "debugger for music clips".

use bach::evolve::*;
use bach::ga::{self, Genome};
use bach::sequencer::{self, ClipInst, TickClock};
use bach::units::*;
use rand::Rng;
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::cell::{Cell, RefCell};
use std::cmp;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::thread;

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    let path = std::env::args().nth(1).expect("must provide config file");
    Config::default().with_config_file(&path)
});
static RUNNING: AtomicBool = AtomicBool::new(true);

fn cfg() -> &'static Config {
    &CONFIG
}

fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

fn continue_running() {
    RUNNING.store(true, Ordering::Relaxed);
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
    clock: RefCell<sequencer::SystemClock>,
    seq: RefCell<sequencer::MidiSequencer>,
    pool: RefCell<ga::GenomePool<ClipGenome>>,
    /// Auto-evaluate until generation.
    eval_until: Cell<u64>,
    /// Prefix clip.
    prefix_clip: RefCell<ClipGenome>,
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
            clock: RefCell::new(sequencer::SystemClock::new(bpm, ppqn)),
            seq: RefCell::new(sequencer::MidiSequencer::new(
                cfg().send_clock,
                cfg().midi_ver,
            )),
            pool: RefCell::new(ga::GenomePool::new(
                ClipGenome {
                    cfg: Some(cfg()),
                    ..Default::default()
                },
                cfg().population_size,
                cfg().mutation_probability,
            )),
            eval_until: Cell::new(0),
            prefix_clip: RefCell::new(ClipGenome::from(vec![])),
        }
    }

    fn tick_until(&self, duration: &Duration) {
        let mut clock = self.clock.borrow_mut();
        let mut seq = self.seq.borrow_mut();
        let mut midi_file = self.midi_file.borrow_mut();
        seq.tick_until(&mut *clock, duration, &mut |b| {
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
        let prefix_clip = self.prefix_clip.borrow();
        let inst_start: isize = -(prefix_clip.clip.len() as isize);
        let mut skip_inst = HashSet::new();
        let mut inst_idx: isize = inst_start;
        let mut silence = true;
        while inst_idx < clip.clip.len() as isize {
            if !is_running() {
                return;
            }

            // Pick instruction from prefix if negative index.
            let seq_inst = if inst_idx < 0 {
                &prefix_clip.clip[(inst_idx - inst_start) as usize]
            } else {
                &clip.clip[inst_idx as usize]
            };

            if !skip_inst.contains(&inst_idx) {
                let (elapsed_qns, elapsed_real) = self.clock.borrow().elapsed(cfg().beats_per_bar);
                println!(
                    "[{:.2}s | {}] <{}> {}",
                    elapsed_real.as_secs_f32(),
                    elapsed_qns,
                    inst_idx,
                    seq_inst
                );
            }

            match seq_inst {
                ClipInst::Tick(delta) => {
                    if silence {
                        println!("<- skipping silence");
                    } else {
                        self.tick_until(delta);
                    }
                }
                ClipInst::Jmp(offset) => {
                    if skip_inst.insert(inst_idx) {
                        inst_idx = cmp::max(inst_start, inst_idx + *offset as isize);
                    }
                }
                ClipInst::Call(seq_call) => {
                    let clock = self.clock.borrow();
                    let mut seq = self.seq.borrow_mut();
                    if let Err(e) = seq.apply(&*clock, seq_call, cfg().skip_allocated) {
                        println!("<! failed to queue: {}", e); // Just keep playing.
                    }
                }
            }

            // Skip initial silence. A jump does count as a non-silence, and can be used to
            // deliberately introduce silence at the beginning.
            silence = if let ClipInst::Tick(_) = seq_inst {
                silence
            } else {
                false
            };

            inst_idx += 1;
        }
        // Allow it to complete some of the sequences.
        println!("<- end clip");
        self.tick_until(&cfg().clip_tail);

        // Do not stop() here, so that chained clips sound smoother. Need to explicitly call
        // stop() where needed.
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
                    if cmd == "a" {
                        clip.eval();
                    } else if cmd == "b" {
                        return true;
                    } else if let Some(suffix) = cmd.strip_prefix('c') {
                        if suffix.is_empty() {
                            println!("current comment: {}", clip.comment);
                        } else {
                            clip.comment = suffix.trim().into();
                        }
                    } else if cmd == "d" {
                        clip.dump();
                    } else if let Some(suffix) = cmd.strip_prefix("e ") {
                        let parts: Vec<&str> = suffix.split('=').map(|s| s.trim()).collect();
                        if parts.len() < 2 {
                            println!("<! missing arguments: must provide instruction");
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
                                    Err(e) => println!("<! invalid edit: {}", e),
                                },
                                Err(e) => println!("<! invalid index: {}", e),
                            }
                        }
                    } else if let Some(suffix) = cmd.to_lowercase().strip_prefix("f ") {
                        let (base, add_arg) = match suffix.trim().strip_prefix("+= ") {
                            Some(s) => (clip.fitness.unwrap_or(0.0), s),
                            None => (0.0, suffix),
                        };
                        match add_arg.trim().parse::<f32>() {
                            Ok(f) => clip.fitness = Some(base + f),
                            Err(e) => println!("<! invalid argument: {}", e),
                        }
                        if cmd.starts_with('F') {
                            // Upper version immediately returns for convenience.
                            return true;
                        }
                    } else if cmd == "i" {
                        println!("comment: {}", clip.comment);
                        println!("fitness: {}", clip.fitness_as_str());
                    } else if let Some(suffix) = cmd.strip_prefix("l ") {
                        if let Err(e) = clip.deserialize(Path::new(suffix)) {
                            println!("<! could not read file {}: {}", suffix, e);
                        }
                    } else if cmd.to_lowercase() == "p" {
                        loop {
                            self.play(clip);
                            self.stop();
                            if !is_running() || cmd == "p" {
                                break;
                            }
                        }
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
                        println!("  a                : auto-score fitness value");
                        println!("  b                : back");
                        println!("  c <comment>      : comment");
                        println!("  d                : dump");
                        println!("  e <idx> = <inst> : edit instruction at index");
                        println!("  f/F [+=] <val>   : assign fitness value / ..back");
                        println!("  i                : info");
                        println!("  l <file>         : load from file");
                        println!("  p/P              : play / loop");
                        println!("  q                : quit");
                        println!("  w <file>         : write");
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
                    } else if let Some(suffix) = cmd.strip_prefix("chan") {
                        let parts: Vec<&str> = suffix.split(' ').collect();
                        if suffix.is_empty() {
                            let seq = self.seq.borrow();
                            // While the chan_map could be a BTreeMap to avoid this sorting, we
                            // rarely want to display the mapping itself. Just sort it for display.
                            let mut chan_map: Vec<_> = seq.chan_map().iter().collect();
                            chan_map.sort();
                            for (k, v) in chan_map {
                                println!("channel {} -> {}", k, v);
                            }
                        } else if parts.len() < 3 {
                            println!("<! requires 2 arguments");
                        } else if let Ok(from) = parts[1].parse() {
                            let to_idx = rand::rng().random_range(2..parts.len());
                            if let Ok(to) = parts[to_idx].parse() {
                                let mut seq = self.seq.borrow_mut();
                                if let Err(e) = seq.insert_chan_map(from, to) {
                                    println!("<! {}", e);
                                } else {
                                    println!("<- mapping channel {} to {}", from, to);
                                }
                            } else {
                                println!("<! invalid 'to' channel: {}", parts[to_idx]);
                            }
                        } else {
                            println!("<! invalid 'from' channel: {}", parts[1]);
                        };
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
                        let parts: Vec<&str> = suffix.split(' ').collect();
                        if parts.len() < 2 {
                            println!("<! requires 2 arguments");
                            continue;
                        }

                        let limit = if parts[0] == "*" {
                            usize::MAX
                        } else {
                            match parts[0].parse() {
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
                            let file_prefix = parts[1];
                            let mut pool = self.pool.borrow_mut();
                            let population = pool.population_mut();
                            for (idx, clip) in population.iter_mut().enumerate() {
                                if idx >= limit {
                                    break;
                                }
                                let path = cfg()
                                    .population_path()
                                    .join(format!("{}_{}.ch", file_prefix, idx));
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
                    } else if let Some(suffix) = cmd.strip_prefix("pfx") {
                        let path = suffix.trim();
                        let mut prefix_clip = self.prefix_clip.borrow_mut();
                        if path.is_empty() {
                            prefix_clip.dump();
                        } else if path == "-" {
                            prefix_clip.clip.clear();
                        } else if let Err(e) = prefix_clip.deserialize(Path::new(path)) {
                            println!("<! could not read file {}: {}", path, e);
                        }
                    } else if cmd == "q" {
                        return false;
                    } else if cmd.to_lowercase().starts_with("s ") {
                        let suffix = &cmd[2..];
                        let mut parts = suffix.split(',');
                        loop {
                            let part = match parts.next() {
                                Some(s) => s,
                                None => {
                                    // Upper version of command will keep looping.
                                    if cmd.starts_with('S') {
                                        parts = suffix.split(',');
                                        continue;
                                    } else {
                                        break;
                                    }
                                }
                            };
                            if !is_running() {
                                break;
                            }
                            if let Some(path) = part.strip_prefix('@') {
                                // Play directly from file.
                                let mut clip = ClipGenome::default();
                                if let Err(e) = clip.deserialize(Path::new(path)) {
                                    println!("<! could not read file {}: {}", path, e);
                                    break;
                                }
                                self.play(&clip);
                            } else {
                                // Play from the current population.
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
                            if !cfg().song_continue {
                                self.stop();
                            }
                        }
                        self.stop();
                    } else if let Some(suffix) = cmd.strip_prefix("w ") {
                        if cfg().population_path.is_empty() {
                            println!("<! no population path set");
                        } else if suffix.is_empty() {
                            println!("<! must provide filename prefix");
                        } else {
                            let file_prefix = suffix;
                            let pool = self.pool.borrow();
                            for idx in 0..pool.population().len() {
                                let clip = &pool.population()[idx].1;
                                let path = cfg()
                                    .population_path()
                                    .join(format!("{}_{}.ch", file_prefix, idx));
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
                        println!("  a <count>              : auto-evolve next <count> generations");
                        println!("  bpm <val>              : change BPM");
                        println!("  c                      : continue");
                        println!("  chan <from> <to>...    : map channel <from> to one of <to>...");
                        println!("  e <idx>                : edit clip");
                        println!("  i                      : info");
                        println!("  l <count> <prefix>     : load <count> genomes into population");
                        println!("  mut <val>              : change mutation probability to <val>");
                        println!("  pfx <file>             : load prefix clip from <file>");
                        println!("  q                      : quit");
                        println!("  s/S <idx or @file>,... : play/loop chained clips (song mode)");
                        println!("  w <prefix>             : write population");
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
                    clip.1.eval();
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
    let mut signals = Signals::new([SIGINT]).unwrap();
    let sig_handle = signals.handle();
    let sig_handler = thread::spawn(move || {
        for _ in &mut signals {
            println!("<- stopping ...");
            RUNNING.store(false, Ordering::Relaxed);
        }
    });

    Prog::new().run();
    sig_handle.close();
    sig_handler.join().unwrap();
}

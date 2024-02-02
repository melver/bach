use rand::Rng;
use std::cmp;
use std::collections::HashSet;
use std::ops::{Index, IndexMut};

pub trait Genome: Sized {
    /// Copies shared state from `blueprint` and re-initializes if empty.
    fn with_blueprint(self, blueprint: &Self) -> Self;

    /// Mutates this Genome with probability `mut_prob`.
    fn mutate(&mut self, mut_prob: f32);

    /// Returns this Genome's fitness value.
    fn fitness(&self) -> f32;

    /// Crossover this Genome with `other`.
    fn crossover(&self, other: &Self, mut_prob: f32) -> Vec<Self>;

    /// If this Genome has been evaluated and its fitness is known.
    fn is_eval(&self) -> bool;

    /// Compares fitness of `self` with `other`, where a better fitness compares less than a worse
    /// fitness. Allows using various sorting functions to order Genomes from best to worst.
    /// Assumes that larger fitness value means better.
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        assert!(self.is_eval());
        assert!(other.is_eval());
        if self.fitness() > other.fitness() {
            cmp::Ordering::Less
        } else {
            cmp::Ordering::Greater
        }
    }
}

pub fn default_crossover<'a, G, T, F>(
    mate1: &'a G,
    mate2: &'a G,
    mut_prob: f32,
    siblings: bool,
    onepoint: bool,
    mut rand_idx: F,
) -> Vec<G>
where
    G: Genome + From<Vec<T>>,
    &'a G: Into<Vec<T>>,
    T: Clone,
    F: FnMut(usize) -> usize,
{
    let mate1_vec: Vec<T> = mate1.into();
    let mate2_vec: Vec<T> = mate2.into();
    assert!(!mate1_vec.is_empty());
    assert!(!mate2_vec.is_empty());
    // `mate1_cut`/`mate2_cut` never cover the entire length because that case would result in
    // duplicating either `mate1_vec` or `mate2_vec`, which does not make sense for crossover.
    let mate1_cut = rand_idx(mate1_vec.len());
    let mate2_cut = if onepoint {
        mate1_cut
    } else {
        rand_idx(mate2_vec.len())
    };

    let mut child_vec = vec![];
    child_vec.extend_from_slice(&mate1_vec[0..mate1_cut]);
    child_vec.extend_from_slice(&mate2_vec[mate2_cut..]);

    let mut ret = vec![];

    if !child_vec.is_empty() {
        let mut child = G::from(child_vec).with_blueprint(mate1);
        child.mutate(mut_prob);
        ret.push(child);
    }

    if siblings {
        child_vec = vec![];
        child_vec.extend_from_slice(&mate2_vec[0..mate2_cut]);
        child_vec.extend_from_slice(&mate1_vec[mate1_cut..]);
        if !child_vec.is_empty() {
            let mut child = G::from(child_vec).with_blueprint(mate1);
            child.mutate(mut_prob);
            ret.push(child);
        }
    }

    ret
}

pub struct GenomePool<G> {
    pub mut_prob: f32,
    generation: u64,
    population: Vec<(u64, G)>,
    target_len: usize,
}

/// Reference to a Genome in a GenomePool. It effectively implements a fat pointer: a tag (unique
/// to GenomePool instance and the current generation) and the index into `population`.
pub struct GenomeRef(usize, usize);

impl<G: Genome + Default> GenomePool<G> {
    pub fn new(blueprint: G, target_len: usize, mut_prob: f32) -> Self {
        // Mutation rate is a percentage.
        assert!(mut_prob >= 0.0);
        assert!(mut_prob <= 1.0);
        assert_ne!(target_len, 0);
        let mut init_population: Vec<(u64, G)> = (0..target_len - 1)
            .map(|_| (0, G::default().with_blueprint(&blueprint)))
            .collect();
        // Don't let the blueprint go to waste.
        init_population.push((0, blueprint));
        Self {
            mut_prob,
            generation: 0,
            population: init_population,
            target_len,
        }
    }

    /// Replace the current population.
    pub fn with_population(mut self, population: Vec<(u64, G)>) -> Self {
        assert_eq!(population.len(), self.target_len);
        self.population = population;
        self
    }

    /// Return the current generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Return the mean fitness value.
    pub fn mean_fitness(&self) -> f32 {
        self.population.iter().fold(0.0, |a, b| a + b.1.fitness()) / (self.population.len() as f32)
    }

    /// Return the current worst fitness.
    pub fn worst_fitness(&self) -> f32 {
        self.population
            .iter()
            .max_by(|a, b| a.1.cmp(&b.1))
            .unwrap()
            .1
            .fitness()
    }

    /// Return the current best fitness.
    pub fn best_fitness(&self) -> f32 {
        self.population
            .iter()
            .min_by(|a, b| a.1.cmp(&b.1))
            .unwrap()
            .1
            .fitness()
    }

    /// Return vector of unique indices into population, where `get_idx` returns indices (such as
    /// from a random source).
    pub fn select_by<F>(&self, len: usize, mut get_idx: F) -> Vec<GenomeRef>
    where
        F: FnMut() -> usize,
    {
        assert!(self.population.len() >= len);

        let tag = self.get_tag();
        let mut ret = Vec::with_capacity(len);
        let mut used = HashSet::new();
        while ret.len() < len {
            let idx = get_idx();
            assert!(idx < self.population.len());
            if used.insert(idx) {
                ret.push(GenomeRef(tag, idx))
            }
        }

        ret
    }

    /// Select `len` uniformly distributed indices into `population`.
    pub fn select_uniform(&self, len: usize) -> Vec<GenomeRef> {
        let mut rng = rand::thread_rng();
        self.select_by(len, || rng.gen_range(0..self.population.len()))
    }

    /// Select the oldest Genome of `population`. If there are multiple genomes with the same age,
    /// the first found will be returned.
    pub fn select_oldest(&self) -> GenomeRef {
        let mut age = self.generation;
        let mut idx = 0;

        for i in 0..self.population.len() {
            let genome = &self.population[i];
            if genome.0 < age {
                age = genome.0;
                idx = i;
            }
        }

        GenomeRef(self.get_tag(), idx)
    }

    pub fn select_best(&self) -> GenomeRef {
        let mut best = 0;

        for i in 1..self.population.len() {
            let genome = &self.population[i];
            if let cmp::Ordering::Less = genome.1.cmp(&self.population[best].1) {
                best = i;
            }
        }

        GenomeRef(self.get_tag(), best)
    }

    /// Select all genomes in the current population.
    pub fn select_all(&self) -> Vec<GenomeRef> {
        let tag = self.get_tag();
        (0..self.population.len())
            .map(|i| GenomeRef(tag, i))
            .collect()
    }

    /// Sort the `selection` of indices into `population` based on the fitness of the indexed
    /// Genome, from best to worst.
    pub fn sort_selection(&self, selection: &mut [GenomeRef]) {
        selection.sort_by(|a, b| self[a].1.cmp(&self[b].1));
    }

    /// Takes a selection and mates the genomes indexed by `mates`.
    ///
    /// The genomes indexed by `replace` are removed from the population (if not equal to `mates`
    /// can be used to implement elitism).
    ///
    /// All GenomeRef returned before this call are invalidated.
    pub fn step(&mut self, mates: &[GenomeRef], replace: &[GenomeRef]) {
        assert!(!mates.is_empty());
        assert!(!replace.is_empty());

        // Generate offspring.
        let mut children = Vec::with_capacity(replace.len());
        for mate_idx in 0..mates.len() {
            let mate1_ref = &mates[mate_idx];
            let mate1 = &self[mate1_ref].1;
            // Avoid mating same 2 genomes twice.
            for mate2_ref in mates.iter().skip(mate_idx + 1) {
                let mate2 = &self[mate2_ref].1;
                children.extend(mate1.crossover(mate2, self.mut_prob));
                if children.len() >= replace.len() {
                    break;
                }
            }
            if children.len() >= replace.len() {
                break;
            }
        }

        // Replace all genomes from `replace` with ones from `children`, until either we exhausted
        // `children` or `replace`.
        for replace_ref in replace {
            if let Some(child) = children.pop() {
                self[replace_ref] = (self.generation + 1, child);
            } else {
                break;
            }
        }

        // GenomePool size should remain constant.
        assert_eq!(self.population.len(), self.target_len);
        self.generation += 1;
    }

    #[inline(always)]
    fn get_tag(&self) -> usize {
        // FIXME: Consider using some hashing function.
        (self.generation as usize) ^ self.population.as_ptr() as usize
    }
}

impl<G: Genome + Default> Index<&GenomeRef> for GenomePool<G> {
    type Output = (u64, G);

    fn index(&self, idx: &GenomeRef) -> &Self::Output {
        assert_eq!(self.get_tag(), idx.0, "use-after-step");
        &self.population[idx.1]
    }
}

impl<G: Genome + Default> IndexMut<&GenomeRef> for GenomePool<G> {
    fn index_mut(&mut self, idx: &GenomeRef) -> &mut Self::Output {
        assert_eq!(self.get_tag(), idx.0, "use-after-step");
        &mut self.population[idx.1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AddGenome {
        pub genome: Vec<f32>,
    }

    impl From<Vec<f32>> for AddGenome {
        fn from(v: Vec<f32>) -> Self {
            Self { genome: v }
        }
    }

    impl From<&AddGenome> for Vec<f32> {
        fn from(g: &AddGenome) -> Self {
            g.genome.clone()
        }
    }

    impl Default for AddGenome {
        fn default() -> Self {
            Self {
                genome: vec![0.0; 5],
            }
        }
    }

    impl Genome for AddGenome {
        fn with_blueprint(mut self, _blueprint: &Self) -> Self {
            self.mutate(1.0);
            self
        }

        fn mutate(&mut self, mut_prob: f32) {
            let mut rng = rand::thread_rng();
            let mut used = HashSet::new();
            let mut selection_count = (self.genome.len() as f32 * mut_prob) as usize;
            while selection_count != 0 {
                let idx = rng.gen_range(0..self.genome.len());
                if used.insert(idx) {
                    self.genome[idx] += rng.gen_range(-2.0..2.0);
                }
                selection_count -= 1;
            }
        }

        fn fitness(&self) -> f32 {
            if self.genome.len() > 10 {
                // Cap size.
                return -999.0;
            }

            let sum: f32 = self.genome.iter().sum();
            // Sum should be close to 42
            -((42.0 - sum).abs())
        }

        fn crossover(&self, other: &Self, mut_prob: f32) -> Vec<Self> {
            super::default_crossover(self, other, mut_prob, true, false, |len| {
                rand::thread_rng().gen_range(0..len)
            })
        }

        fn is_eval(&self) -> bool {
            true
        }
    }

    #[test]
    fn evolve_add_42() {
        let tournament_size = 10;
        let tournament_winners = 5;
        let mut pool = GenomePool::new(AddGenome::default(), 25, 0.3);
        for generation in 0..50 {
            assert_eq!(pool.generation(), generation);
            // Assert that new genomes are added.
            let mut found_new = false;
            for g_ref in pool.select_all() {
                assert!(pool[&g_ref].0 <= generation);
                if pool[&g_ref].0 == generation {
                    found_new = true;
                }
            }
            assert!(found_new);
            // Advance generation.
            let mut selection = pool.select_uniform(tournament_size);
            pool.sort_selection(&mut selection);
            let mates = &selection[0..tournament_winners];
            let elite = &selection[tournament_winners..];
            assert_eq!(mates.len(), 5);
            assert_eq!(elite.len(), 5);
            pool.step(mates, elite);
            // There should be progress.
            assert!(pool.best_fitness() > pool.mean_fitness());
            assert!(pool.mean_fitness() > pool.worst_fitness());
            assert!(pool[&pool.select_oldest()].0 < pool.generation());
        }
        // Check we produced some fit offspring.
        let best = pool.select_best();
        assert_eq!(pool.best_fitness(), pool[&best].1.fitness());
        assert_ne!(pool.worst_fitness(), pool[&best].1.fitness());
        assert_ne!(pool.mean_fitness(), pool[&best].1.fitness());
        assert_ne!(pool[&best].0, 0);
        assert!(pool[&best].1.genome.len() <= 10);
        let best_sum: f32 = pool[&best].1.genome.iter().sum();
        assert!(
            best_sum >= 41.1 && best_sum <= 42.9,
            "failed to reach target - currently: {}",
            best_sum
        );
    }

    #[test]
    fn distinct_genome_ref() {}

    #[test]
    fn default_crossover() {}
}

//! Explicit PRNGs. DESIGN §2.2.
//!
//! Nothing in the sim may use `rand::thread_rng`, system time, or `HashMap`
//! iteration order. Every random draw in a state-affecting path comes from one
//! of the generators below, seeded from `run_seed` and `screen_index` alone.

/// SplitMix64 step: advances `state` and returns the mixed output.
#[inline]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Stateless 64-bit mix — the finalizer of SplitMix64 applied to `x` directly.
/// Used for deriving seeds, never for streams of values.
#[inline]
pub fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `screen_seed = splitmix64(run_seed ^ (screen_index * GOLDEN))` — DESIGN §2.2.
///
/// Independent per screen: screen 400 is generatable without screens 1..399.
#[inline]
pub fn screen_seed(run_seed: u64, screen_index: u32) -> u64 {
    mix64(run_seed ^ (screen_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// A named sub-stream of a screen seed, so adding a draw in one system cannot
/// shift the values another system sees.
#[inline]
pub fn substream(seed: u64, tag: u64) -> u64 {
    mix64(seed ^ mix64(tag))
}

/// SplitMix64 as a stream.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    #[inline]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        splitmix64(&mut self.state)
    }
}

/// PCG32 (XSH-RR). The workhorse for generation draws.
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub(crate) fn state_words(&self) -> (u64, u64) {
        (self.state, self.inc)
    }

    /// Seeded from a single u64; the stream selector is derived so that two
    /// nearby seeds do not produce correlated streams.
    pub fn new(seed: u64) -> Self {
        let mut s = SplitMix64::new(seed);
        let init = s.next_u64();
        let seq = s.next_u64();
        let mut r = Pcg32 {
            state: 0,
            inc: (seq << 1) | 1,
        };
        r.next_u32();
        r.state = r.state.wrapping_add(init);
        r.next_u32();
        r
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[0, 1)`, 32 bits of mantissa. Exact division by 2^32.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        self.next_u32() as f64 * (1.0 / 4294967296.0)
    }

    /// Uniform in `[lo, hi)`.
    #[inline]
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }

    /// Uniform in `[-m, m]` — signed magnitude draw (wind, elevation delta).
    #[inline]
    pub fn signed(&mut self, m: f64) -> f64 {
        (self.next_f64() * 2.0 - 1.0) * m
    }

    /// Uniform integer in `[lo, hi]` inclusive. Debiased by rejection so the
    /// result does not depend on modulo bias.
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        debug_assert!(hi >= lo);
        let span = hi - lo + 1;
        if span == 0 {
            return self.next_u32();
        }
        let zone = u32::MAX - (u32::MAX % span) - 1;
        loop {
            let v = self.next_u32();
            if v <= zone {
                return lo + v % span;
            }
        }
    }

    /// Index into a weight table. Weights are `u32` so the choice is decided by
    /// integer comparison, never by float accumulation order.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total: u32 = weights.iter().sum();
        if total == 0 {
            return 0;
        }
        let mut roll = self.range_u32(0, total - 1);
        for (i, w) in weights.iter().enumerate() {
            if roll < *w {
                return i;
            }
            roll -= *w;
        }
        weights.len() - 1
    }

    /// Fisher-Yates over indices — deterministic, no allocation of a hash set.
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        if xs.len() < 2 {
            return;
        }
        let mut i = xs.len() - 1;
        while i > 0 {
            let j = self.range_u32(0, i as u32) as usize;
            xs.swap(i, j);
            i -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_matches_reference_vector() {
        // Reference stream for state 0 from the canonical SplitMix64.
        let mut s = 0u64;
        let got: Vec<u64> = (0..4).map(|_| splitmix64(&mut s)).collect();
        assert_eq!(
            got,
            vec![
                0xE220A8397B1DCDAF,
                0x6E789E6AA1B965F4,
                0x06C45D188009454F,
                0xF88BB8A8724C81EC
            ]
        );
    }

    #[test]
    fn screen_seed_is_index_independent() {
        // Screen 400 derives from the run seed alone.
        let a = screen_seed(0xDEAD_BEEF, 400);
        let b = screen_seed(0xDEAD_BEEF, 400);
        assert_eq!(a, b);
        assert_ne!(a, screen_seed(0xDEAD_BEEF, 399));
    }

    #[test]
    fn pcg32_is_reproducible_and_in_range() {
        let a: Vec<u32> = {
            let mut r = Pcg32::new(7);
            (0..8).map(|_| r.next_u32()).collect()
        };
        let b: Vec<u32> = {
            let mut r = Pcg32::new(7);
            (0..8).map(|_| r.next_u32()).collect()
        };
        assert_eq!(a, b);

        let mut r = Pcg32::new(99);
        for _ in 0..2000 {
            let f = r.next_f64();
            assert!((0.0..1.0).contains(&f));
            let i = r.range_u32(3, 9);
            assert!((3..=9).contains(&i));
        }
    }

    #[test]
    fn weighted_respects_zero_weights() {
        let mut r = Pcg32::new(1234);
        for _ in 0..500 {
            let i = r.weighted(&[0, 5, 0, 1]);
            assert!(i == 1 || i == 3, "picked a zero-weight entry: {i}");
        }
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut xs: Vec<u32> = (0..32).collect();
        let mut r = Pcg32::new(55);
        r.shuffle(&mut xs);
        let mut sorted = xs.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..32).collect::<Vec<u32>>());
        assert_ne!(xs, sorted);
    }
}

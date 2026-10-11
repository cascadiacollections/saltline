//! Explicit PRNGs. DESIGN §2.2.
//!
//! Nothing in the sim may use `rand::thread_rng`, system time, or `HashMap`
//! iteration order. Every random draw in a state-affecting path comes from one
//! of the generators below, seeded from `run_seed` and `screen_index` alone.
//!
//! Two generators, two jobs. SplitMix64's mixing function ([`mix64`]) turns
//! related inputs (a run seed and a screen index, a seed and a tag) into
//! unrelated-looking seeds; it is a bijection, so within one run distinct
//! screen indices (or distinct tags on one seed) never share a seed. PCG32 ([`Pcg32`]) then produces the stream of draws for one
//! system. Both are a few lines of wrapping integer arithmetic, so the same
//! seed gives the same numbers on every platform with no crate dependency.
//!
//! The derivation tree is the important idea. Each screen's seed depends only
//! on `(run_seed, screen_index)` ([`screen_seed`]), and each system on a
//! screen draws from its own named [`substream`]. A system that starts
//! drawing one more number shifts only its own stream, so generation stays
//! stable as the game grows.

/// SplitMix64 step: advances `state` and returns the mixed output.
///
/// The state is a Weyl sequence (a counter stepped by the odd constant
/// 2^64/φ, the golden ratio); the output is that counter run through two
/// xor-shift-multiply rounds (Stafford's "Mix13" finaliser), which spread
/// every input bit across the whole word.
#[inline]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Stateless 64-bit mix: one SplitMix64 step on a copy of `x`.
/// Used for deriving seeds, never for streams of values.
///
/// Equal to one [`splitmix64`] step on a copy of `x`. Every stage is
/// invertible, so `mix64` is a bijection on `u64`.
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
///
/// Multiplying the index by the golden-ratio constant (wrapping) spreads
/// consecutive indices far apart before they are xored into the run seed.
/// [`mix64`] is the single `splitmix64` step the formula calls for.
#[inline]
pub fn screen_seed(run_seed: u64, screen_index: u32) -> u64 {
    mix64(run_seed ^ (screen_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// A named sub-stream of a screen seed, so adding a draw in one system cannot
/// shift the values another system sees.
///
/// `tag` is any fixed constant naming the consumer; for example run.rs picks
/// templates from tag `0x5445_4D50`, "TEMP" in ASCII. The tag is mixed before
/// the xor so that nearby tags do not give nearby seeds.
#[inline]
pub fn substream(seed: u64, tag: u64) -> u64 {
    mix64(seed ^ mix64(tag))
}

/// SplitMix64 as a stream, period 2^64. Used here to seed [`Pcg32`].
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Start a stream. Any seed, including 0, is valid.
    #[inline]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    /// Next 64 bits of the stream.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        splitmix64(&mut self.state)
    }
}

/// PCG32 (XSH-RR). The workhorse for generation draws.
///
/// O'Neill's permuted congruential generator: a 64-bit linear congruential
/// generator (LCG) whose weak low bits are never output directly. Each 32-bit
/// output is the high bits of the old state, xor-shifted and then rotated by
/// an amount taken from the state's top five bits. Period 2^64 per stream.
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    /// LCG increment, which selects the stream. Must be odd.
    inc: u64,
}

/// The 64-bit LCG multiplier used by the PCG reference implementation (from
/// Knuth's MMIX).
const PCG_MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub(crate) fn state_words(&self) -> (u64, u64) {
        (self.state, self.inc)
    }

    /// Seeded from a single u64; the stream selector is derived so that two
    /// nearby seeds do not produce correlated streams.
    ///
    /// SplitMix64 expands the seed into an initial state and a stream
    /// selector; the steps below then follow the reference `pcg32_srandom_r`
    /// seeding sequence.
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

    /// Next 32 bits of the stream.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        // The output is computed from the old state while the LCG advances,
        // so the multiply and the permutation do not wait on each other.
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

    /// Uniform in `[lo, hi)`. Consumes one draw.
    #[inline]
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }

    /// Uniform in `[-m, m)` — signed magnitude draw (wind, elevation delta).
    ///
    /// [`Pcg32::next_f64`] never returns 1, so `+m` itself is never drawn.
    #[inline]
    pub fn signed(&mut self, m: f64) -> f64 {
        (self.next_f64() * 2.0 - 1.0) * m
    }

    /// Uniform integer in `[lo, hi]` inclusive. Debiased by rejection so the
    /// result does not depend on modulo bias.
    ///
    /// A plain `next_u32() % span` would favour small results whenever `span`
    /// does not divide 2^32. Rejection sampling instead accepts only draws
    /// from a prefix whose length is a multiple of `span`, so every residue
    /// is equally likely. On average it takes at most two draws.
    ///
    /// The full range `0..=u32::MAX` has a span of 2^32, which wraps to 0;
    /// it returns one unfiltered draw, in debug and release builds alike.
    ///
    /// # Panics
    ///
    /// If `hi < lo`, in every build.
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        assert!(hi >= lo, "range_u32: hi {hi} is below lo {lo}");
        let span = (hi - lo).wrapping_add(1);
        if span == 0 {
            return self.next_u32();
        }
        // Accept `v <= zone`. When `span` divides 2^32 exactly this rejects
        // `span` values it need not; the result is still unbiased, and
        // tightening it would change every stream already recorded.
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
    ///
    /// Entry `i` is chosen with probability `weights[i] / sum`. Zero weights
    /// are never chosen; an all-zero or empty table returns 0 without
    /// drawing.
    ///
    /// # Panics
    ///
    /// If the weights sum past `u32::MAX`, in every build.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total = weights
            .iter()
            .try_fold(0u32, |sum, &w| sum.checked_add(w))
            .expect("weights sum past u32::MAX");
        if total == 0 {
            return 0;
        }
        // Roll once, then walk the table subtracting each weight until the
        // roll falls inside one: a linear scan of the cumulative distribution.
        let mut roll = self.range_u32(0, total - 1);
        for (i, w) in weights.iter().enumerate() {
            if roll < *w {
                return i;
            }
            roll -= *w;
        }
        weights.len() - 1
    }

    /// Fisher–Yates over indices — deterministic, no allocation of a hash set.
    ///
    /// Durstenfeld's in-place form: walk from the back, swapping each slot
    /// with a uniformly chosen slot at or before it. With an unbiased
    /// [`Pcg32::range_u32`] every permutation is equally likely.
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
    fn the_full_u32_range_is_one_plain_draw() {
        // `0..=u32::MAX` has a span of 2^32, which does not fit a u32. It
        // must behave the same in debug and release: one unfiltered draw.
        let mut r = Pcg32::new(4321);
        let mut plain = r.clone();
        for _ in 0..16 {
            assert_eq!(r.range_u32(0, u32::MAX), plain.next_u32());
        }
    }

    #[test]
    #[should_panic(expected = "weights sum past u32::MAX")]
    fn weighted_rejects_a_table_that_overflows_in_every_build() {
        Pcg32::new(1).weighted(&[u32::MAX, 1]);
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

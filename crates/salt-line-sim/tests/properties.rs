//! Property tests: the invariants the example tests pin at a few points,
//! checked across random seeds, widths and inputs.
//!
//! Only the public API is used, the same surface the presentation layer and
//! the replay format depend on. On a failure proptest shrinks the input and
//! records it under `proptest-regressions/`; commit that file so the case
//! stays covered.

mod common;

use common::{play, Decision};
use proptest::prelude::*;
use salt_line_sim::ballistics::Shot;
use salt_line_sim::dmath::{self, MAX_ANGLE_TICKS};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::{state_hash, Replay};
use salt_line_sim::rng::Pcg32;
use salt_line_sim::{POWER_TICKS_MAX, SCREEN_W_MAX, SCREEN_W_MIN};

fn decisions() -> impl Strategy<Value = Vec<Decision>> {
    prop::collection::vec(
        (
            0..=MAX_ANGLE_TICKS,
            0..=POWER_TICKS_MAX,
            0usize..3,
            prop::bool::weighted(0.7),
            prop::option::weighted(0.4, -20.0f64..500.0),
            prop::option::weighted(0.4, -40.0f64..40.0),
        )
            .prop_map(|(angle, power, pick, solve, fuse, reposition)| Decision {
                angle,
                power,
                pick,
                solve,
                fuse,
                reposition,
            }),
        0..40,
    )
}

proptest! {
    // Each case generates real screens, solver checks included, so keep the
    // count modest; the seeds still differ on every run of the suite.
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_run_replays_to_the_same_state(
        seed in any::<u64>(),
        width in SCREEN_W_MIN..=SCREEN_W_MAX,
        moves in decisions(),
    ) {
        let played = play(seed, width, &moves);
        let replay = Replay::of(&played);
        let rebuilt = replay.reconstruct().expect("a recorded run failed to replay");
        prop_assert_eq!(state_hash(&played), state_hash(&rebuilt));
        prop_assert_eq!(&played.log, &rebuilt.log);
        prop_assert_eq!(played.shells, rebuilt.shells);
        prop_assert_eq!(played.screens_cleared, rebuilt.screens_cleared);
    }
}

proptest! {
    #[test]
    fn sin_and_cos_track_the_std_library(x in -100.0f64..100.0) {
        // std is the accuracy oracle; the sim uses its own for determinism.
        prop_assert!((dmath::sin(x) - x.sin()).abs() < 1e-12, "sin({})", x);
        prop_assert!((dmath::cos(x) - x.cos()).abs() < 1e-12, "cos({})", x);
    }

    #[test]
    fn angle_search_lands_within_half_a_tick_of_atan2(
        vx in 1e-3f64..1e4,
        vy in 1e-3f64..1e4,
    ) {
        let got = dmath::angle_ticks_from_dir(vx, vy);
        prop_assert!((0..=MAX_ANGLE_TICKS).contains(&got));
        let exact = vy.atan2(vx).to_degrees() / dmath::TICK_DEG;
        prop_assert!((got as f64 - exact).abs() <= 0.5 + 1e-6, "{} ticks for {}", got, exact);
    }

    #[test]
    fn angle_search_never_leaves_the_quarter_turn(vx in -1e6f64..1e6, vy in -1e6f64..1e6) {
        prop_assert!((0..=MAX_ANGLE_TICKS).contains(&dmath::angle_ticks_from_dir(vx, vy)));
    }

    #[test]
    fn rng_ranges_are_inclusive_and_never_escape(seed in any::<u64>(), lo in any::<u32>(), span in any::<u32>()) {
        let hi = lo.saturating_add(span);
        let mut r = Pcg32::new(seed);
        for _ in 0..16 {
            let v = r.range_u32(lo, hi);
            prop_assert!(lo <= v && v <= hi, "{} outside {}..={}", v, lo, hi);
        }
    }

    #[test]
    fn float_ranges_are_half_open(seed in any::<u64>(), lo in -1e6f64..1e6, span in 1e-3f64..1e6) {
        let hi = lo + span;
        let mut r = Pcg32::new(seed);
        for _ in 0..16 {
            let v = r.range_f64(lo, hi);
            prop_assert!(lo <= v && v < hi, "{} outside {}..{}", v, lo, hi);
        }
    }

    #[test]
    fn a_weighted_pick_never_lands_on_a_zero_weight(
        seed in any::<u64>(),
        weights in prop::collection::vec(prop_oneof![Just(0u32), 1u32..50], 1..8),
    ) {
        prop_assume!(weights.iter().any(|&w| w > 0));
        let mut r = Pcg32::new(seed);
        for _ in 0..32 {
            let i = r.weighted(&weights);
            prop_assert!(weights[i] > 0, "picked index {} with weight 0 from {:?}", i, weights);
        }
    }

    #[test]
    fn a_shuffle_is_a_permutation(seed in any::<u64>(), len in 0usize..40) {
        let mut xs: Vec<usize> = (0..len).collect();
        Pcg32::new(seed).shuffle(&mut xs);
        let mut sorted = xs.clone();
        sorted.sort_unstable();
        prop_assert_eq!(sorted, (0..len).collect::<Vec<_>>());
    }

    #[test]
    fn any_shot_quantizes_into_range(angle in any::<i32>(), power in any::<i32>()) {
        let q = Shot::new(angle, power).quantized(&Loadout::default());
        prop_assert!((0..=MAX_ANGLE_TICKS).contains(&q.angle_ticks));
        prop_assert!((0..=POWER_TICKS_MAX).contains(&q.power_ticks));
    }
}

/// Two items must come out in both orders across seeds; a shuffle that
/// skipped short slices would always leave them as they were.
#[test]
fn even_two_items_get_shuffled() {
    let swapped = (0..64u64).any(|seed| {
        let mut xs = [0, 1];
        Pcg32::new(seed).shuffle(&mut xs);
        xs == [1, 0]
    });
    assert!(swapped);
}

/// A trailing zero weight must never be picked: a roll that ran past the
/// total would fall through the table onto it.
#[test]
fn a_trailing_zero_weight_is_never_picked() {
    for seed in 0..32u64 {
        let mut r = Pcg32::new(seed);
        for _ in 0..64 {
            assert_eq!(r.weighted(&[1, 0]), 0);
            assert_ne!(r.weighted(&[2, 3, 0]), 2);
        }
    }
}

/// The replay property is only as strong as the runs it sees. Check that the
/// scripted player really reaches Airburst and Enfilade and uses them: shots
/// fired at several fuse settings with Airburst in the loadout, and shots
/// fired from a moved emplacement. Without those, a log that drops either
/// input would still replay.
#[test]
fn the_scripted_player_uses_the_fuse_and_enfilade() {
    let (mut fuse_runs, mut offset_runs) = (0, 0);
    for seed in 0..24u64 {
        let played = play(seed, 320, &common::seeded_decisions(seed, 40, 0.7));
        let shots: Vec<_> = played.log.iter().filter(|e| e.angle_ticks >= 0).collect();
        let mut fuses: Vec<_> = shots.iter().map(|e| e.fuse_tenths).collect();
        fuses.sort_unstable();
        fuses.dedup();
        if played.loadout.airburst() && fuses.len() > 2 {
            fuse_runs += 1;
        }
        if shots.iter().any(|e| e.emplacement_offset != 0) {
            offset_runs += 1;
        }
        let rebuilt = Replay::of(&played).reconstruct().expect("replay failed");
        assert_eq!(state_hash(&played), state_hash(&rebuilt), "seed {seed}");
    }
    assert!(
        fuse_runs >= 3,
        "only {fuse_runs} runs used the Airburst fuse"
    );
    assert!(
        offset_runs >= 3,
        "only {offset_runs} runs moved the emplacement"
    );
}

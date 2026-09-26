//! Engine-free slingshot aiming for touch: pull back from where the finger
//! went down, and the pull sets angle and power.
//!
//! The result is in the sim's tick space and on the same grid as the keyboard
//! (the loadout's angle quantum, power in steps of 5), so a dragged shot is
//! exactly as deterministic as a keyed one.
//!
//! # Slingshot aiming
//!
//! The finger is the back of a slingshot: the shell flies the opposite way to
//! the pull. The vector from the current point back to the start gives the
//! launch direction, and its length gives power, linear up to [`FULL_PULL`].
//! Floats are fine here because they never reach the sim: they are rounded
//! to integer ticks first, and the sim only ever sees ticks (angle in 0.05°
//! steps, `0..=MAX_ANGLE_TICKS` for 0–90°; power `0..=POWER_TICKS_MAX`).
//!
//! `game.rs` recomputes the aim every frame while the finger is down, so the
//! readouts and barrel sprite show exactly what letting go would fire, and
//! fires on the drop. A pull that ends back inside [`DEAD_ZONE`] cancels and
//! restores the aim from before the drag.
//!
//! [`short_arc_len`] decides how much of the predicted trajectory the Short
//! arc hint draws, which is the design lever that keeps the preview a hint
//! rather than a solution (DESIGN §2.5).

use salt_line_sim::dmath::{self, MAX_ANGLE_TICKS};
use salt_line_sim::POWER_TICKS_MAX;

/// A pull shorter than this, in buffer pixels, is no aim: dragging back to the
/// start cancels the shot.
pub const DEAD_ZONE: f64 = 6.0;
/// Pull length, in buffer pixels, that gives full power.
pub const FULL_PULL: f64 = 100.0;
/// Power moves in the same steps as a key press, in power ticks.
const POWER_STEP: i32 = 5;

/// Angle and power ticks for a pull from `start` to `now`, in buffer pixels
/// with y pointing down. Pulling down and left aims up and right. `None`
/// inside the dead zone.
///
/// The angle is rounded to the nearest multiple of `angle_quantum` (the
/// loadout's angle step in ticks; values below 1 are treated as 1) and
/// clamped to `0..=MAX_ANGLE_TICKS`: a launch direction below the horizon
/// flattens to 0°, and one pointing backwards but upwards stands at 90°.
/// Power is rounded to the nearest step of 5 and clamped to
/// `0..=POWER_TICKS_MAX`.
pub fn pull_to_aim(start: (f32, f32), now: (f32, f32), angle_quantum: i32) -> Option<(i32, i32)> {
    // Reverse the pull to get the launch direction, and flip y from screen
    // (down) to world (up) at the same time.
    let dx = (start.0 - now.0) as f64;
    let dy_up = (now.1 - start.1) as f64;
    let len = dx.hypot(dy_up);
    if len < DEAD_ZONE {
        return None;
    }
    let q = angle_quantum.max(1);
    let raw = dmath::angle_ticks_from_dir(dx, dy_up);
    // Round half up to the grid. `raw` is never negative, so integer
    // division truncating towards zero is a floor here.
    let angle = ((raw + q / 2) / q * q).clamp(0, MAX_ANGLE_TICKS);
    let frac = (len / FULL_PULL).min(1.0);
    let power = (frac * POWER_TICKS_MAX as f64 / POWER_STEP as f64).round() as i32 * POWER_STEP;
    Some((angle, power.clamp(0, POWER_TICKS_MAX)))
}

/// How many samples of a `len`-sample path the Short preview draws: a fixed
/// time `window`, so its length reads as power, but never past the apex and
/// never more than half the flight, so it never shows where the shell lands.
/// A flat shot peaks at once, so it gets a third of the window, still inside
/// the first half of the flight.
///
/// All three arguments count trajectory samples, one per sim step: `apex` is
/// the index of the highest sample, `len` the path length and `window` the
/// preview time converted to steps. The result is at most `window` and at
/// most `len / 2`.
pub fn short_arc_len(apex: usize, len: usize, window: usize) -> usize {
    window.min(apex.max(window / 3)).min(len / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulling_down_and_left_aims_up_and_right() {
        // 45 degrees, full power.
        let (a, p) = pull_to_aim((200.0, 100.0), (120.0, 180.0), 10).unwrap();
        assert_eq!(a, 900);
        assert_eq!(p, POWER_TICKS_MAX);
    }

    #[test]
    fn angle_lands_on_the_loadout_grid_and_power_on_the_key_step() {
        let (a, p) = pull_to_aim((200.0, 100.0), (170.0, 110.0), 10).unwrap();
        assert_eq!(a % 10, 0);
        assert_eq!(p % POWER_STEP, 0);
        assert!(p > 0 && p < POWER_TICKS_MAX);
    }

    #[test]
    fn a_short_pull_is_a_cancel() {
        assert_eq!(pull_to_aim((50.0, 50.0), (47.0, 52.0), 10), None);
    }

    #[test]
    fn angles_clamp_to_the_quarter_turn() {
        // Straight down pull aims straight up; a pull forward can only go flat.
        assert_eq!(
            pull_to_aim((50.0, 50.0), (50.0, 120.0), 10).unwrap().0,
            MAX_ANGLE_TICKS
        );
        assert_eq!(pull_to_aim((50.0, 50.0), (120.0, 50.0), 10).unwrap().0, 0);
    }

    #[test]
    fn power_grows_with_the_pull_until_full() {
        let p = |d: f32| {
            pull_to_aim((100.0, 50.0), (100.0 - d, 50.0 + d), 10)
                .unwrap()
                .1
        };
        assert!(p(20.0) < p(40.0));
        assert_eq!(p(80.0), p(200.0));
    }

    #[rstest::rstest]
    #[case::window_first(200, 600, 36, 36)]
    #[case::apex_first(20, 600, 36, 20)]
    #[case::weak_lob_capped_at_half(2, 16, 36, 8)]
    #[case::flat_shot_stub(0, 600, 36, 12)]
    fn short_arc_length(
        #[case] apex: usize,
        #[case] len: usize,
        #[case] window: usize,
        #[case] expected: usize,
    ) {
        assert_eq!(short_arc_len(apex, len, window), expected);
    }

    #[test]
    fn short_arc_never_reaches_the_landing() {
        // A lob barely off the muzzle: apex early, whole flight short.
        assert_eq!(short_arc_len(2, 16, 36), 8);
        // A flat shot peaks at once but still shows a stub.
        assert_eq!(short_arc_len(0, 600, 36), 12);
        for len in 0..80 {
            for apex in 0..len.max(1) {
                assert!(short_arc_len(apex, len, 36) <= len / 2);
            }
        }
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        fn point() -> impl Strategy<Value = (f32, f32)> {
            (-500.0f32..900.0, -300.0f32..500.0)
        }

        proptest! {
            #[test]
            fn a_pull_aims_on_the_grid_or_cancels(
                start in point(),
                now in point(),
                quantum in prop::sample::select(vec![1, 2, 5, 10]),
            ) {
                let dx = (start.0 - now.0) as f64;
                let dy = (now.1 - start.1) as f64;
                match pull_to_aim(start, now, quantum) {
                    None => prop_assert!(dx.hypot(dy) < DEAD_ZONE),
                    Some((angle, power)) => {
                        prop_assert!(dx.hypot(dy) >= DEAD_ZONE);
                        prop_assert!((0..=MAX_ANGLE_TICKS).contains(&angle));
                        prop_assert_eq!(angle % quantum, 0);
                        prop_assert!((0..=POWER_TICKS_MAX).contains(&power));
                        prop_assert_eq!(power % POWER_STEP, 0);
                    }
                }
            }

            #[test]
            fn pulling_further_the_same_way_never_weakens_the_shot(
                start in point(),
                dir in (0.05f32..1.0, 0.05f32..1.0),
                a in 0.0f32..150.0,
                b in 0.0f32..150.0,
            ) {
                let (near, far) = if a < b { (a, b) } else { (b, a) };
                let at = |d: f32| (start.0 - dir.0 * d, start.1 + dir.1 * d);
                if let (Some((_, p1)), Some((_, p2))) =
                    (pull_to_aim(start, at(near), 10), pull_to_aim(start, at(far), 10))
                {
                    prop_assert!(p1 <= p2);
                }
            }

            #[test]
            fn the_short_arc_stays_in_the_first_half(apex in 0usize..2000, len in 0usize..2000, window in 1usize..200) {
                prop_assert!(short_arc_len(apex, len, window) <= len / 2);
                prop_assert!(short_arc_len(apex, len, window) <= window);
            }
        }
    }
}

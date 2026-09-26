//! Engine-free slingshot aiming for touch: pull back from where the finger
//! went down, and the pull sets angle and power.
//!
//! The result is in the sim's tick space and on the same grid as the keyboard
//! (the loadout's angle quantum, power in steps of 5), so a dragged shot is
//! exactly as deterministic as a keyed one.

use salt_line_sim::dmath::{self, MAX_ANGLE_TICKS};
use salt_line_sim::POWER_TICKS_MAX;

/// A pull shorter than this, in buffer pixels, is no aim: dragging back to the
/// start cancels the shot.
pub const DEAD_ZONE: f64 = 6.0;
/// Pull length, in buffer pixels, that gives full power.
pub const FULL_PULL: f64 = 100.0;
/// Power moves in the same steps as a key press.
const POWER_STEP: i32 = 5;

/// Angle and power ticks for a pull from `start` to `now`, in buffer pixels
/// with y pointing down. Pulling down and left aims up and right. `None`
/// inside the dead zone.
pub fn pull_to_aim(start: (f32, f32), now: (f32, f32), angle_quantum: i32) -> Option<(i32, i32)> {
    let dx = (start.0 - now.0) as f64;
    let dy_up = (now.1 - start.1) as f64;
    let len = dx.hypot(dy_up);
    if len < DEAD_ZONE {
        return None;
    }
    let q = angle_quantum.max(1);
    let raw = dmath::angle_ticks_from_dir(dx, dy_up);
    let angle = ((raw + q / 2) / q * q).clamp(0, MAX_ANGLE_TICKS);
    let frac = (len / FULL_PULL).min(1.0);
    let power = (frac * POWER_TICKS_MAX as f64 / POWER_STEP as f64).round() as i32 * POWER_STEP;
    Some((angle, power.clamp(0, POWER_TICKS_MAX)))
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
}

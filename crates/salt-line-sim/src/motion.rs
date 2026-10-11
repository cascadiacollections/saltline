//! Shared fixed-step motion for live modes. Coordinates are world units, y up.
//! Rendering resolution and wall-clock time never enter these calculations.

use crate::{ballistics::segment_hits_aabb, ballistics::Aabb, Vec2, DT, GRAVITY};

/// One semi-implicit Euler step, matching ordinary artillery ballistics.
pub fn advance_projectile(position: &mut Vec2, velocity: &mut Vec2, wind: f64) {
    velocity.x += wind * DT;
    velocity.y -= GRAVITY * DT;
    position.x += velocity.x * DT;
    position.y += velocity.y * DT;
}

/// Continuous collision against a box translated linearly during this tick.
/// `target_delta` is displacement, not velocity. Relative motion catches a
/// target crossing a shell even when neither endpoint overlaps. Rotation and
/// changes in target size require a different collision model.
pub fn hits_translating_box(
    shell_before: Vec2,
    shell_after: Vec2,
    target_before: Aabb,
    target_delta: Vec2,
    shell_radius: f64,
) -> bool {
    segment_hits_aabb(
        shell_before,
        Vec2::new(
            shell_after.x - target_delta.x,
            shell_after.y - target_delta.y,
        ),
        target_before.expand(shell_radius),
    )
}

/// First contact fraction within a tick, for ordering simultaneous candidates.
/// Uses the same relative-motion model as `hits_translating_box`.
pub fn translating_box_contact(
    before: Vec2,
    after: Vec2,
    target: Aabb,
    delta: Vec2,
    radius: f64,
) -> Option<f64> {
    let b = target.expand(radius);
    let movement = Vec2::new(after.x - before.x - delta.x, after.y - before.y - delta.y);
    let mut enter: f64 = 0.0;
    let mut exit: f64 = 1.0;
    for (start, speed, low, high) in [
        (before.x, movement.x, b.x0, b.x1),
        (before.y, movement.y, b.y0, b.y1),
    ] {
        if speed == 0.0 {
            if start < low || start > high {
                return None;
            }
        } else {
            let a = (low - start) / speed;
            let z = (high - start) / speed;
            enter = enter.max(a.min(z));
            exit = exit.min(a.max(z));
            if enter > exit {
                return None;
            }
        }
    }
    Some(enter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_target_hits_even_when_endpoints_are_clear() {
        assert!(hits_translating_box(
            Vec2::new(10.0, 10.0),
            Vec2::new(10.0, 10.0),
            Aabb::new(0.0, 9.0, 2.0, 11.0),
            Vec2::new(20.0, 0.0),
            0.0,
        ));
    }

    #[test]
    fn equal_motion_preserves_separation() {
        assert!(!hits_translating_box(
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            Aabb::new(0.0, 9.0, 2.0, 11.0),
            Vec2::new(20.0, 0.0),
            0.0,
        ));
    }

    #[test]
    fn projectile_accelerates_before_moving() {
        let mut p = Vec2::new(3.0, 4.0);
        let mut v = Vec2::new(12.0, 24.0);
        advance_projectile(&mut p, &mut v, 6.0);
        assert_eq!(v, Vec2::new(12.0 + 6.0 * DT, 24.0 - GRAVITY * DT));
        assert_eq!(p, Vec2::new(3.0 + v.x * DT, 4.0 + v.y * DT));
    }

    #[test]
    fn stationary_target_matches_existing_collision() {
        let target = Aabb::new(5.0, 5.0, 8.0, 8.0);
        for y in [4.0, 6.0, 10.0] {
            let a = Vec2::new(0.0, y);
            let b = Vec2::new(12.0, y);
            assert_eq!(
                hits_translating_box(a, b, target, Vec2::default(), 1.0),
                segment_hits_aabb(a, b, target.expand(1.0))
            );
        }
    }
}

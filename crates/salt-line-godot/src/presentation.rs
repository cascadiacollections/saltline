//! Engine-free helpers for presentation timing and held-input repeat.
//!
//! Everything here runs on wall-clock seconds from Godot's frame `delta` and
//! only changes what is drawn or how input is paced. None of it feeds back
//! into `salt-line-sim`, whose outcomes depend only on seed and shots; a
//! screen shake that read sim state, or randomness, would make two replays of
//! the same run look different for no reason. Kept free of Godot types so it
//! can be unit-tested.

// Onboarding lessons, one bit each in the save file's `learned` field
// (`save_format::Save::learned`). A bit is set the first time the player
// demonstrates the thing, and the HUD prompts the lowest unset lesson in
// roughly this order. Bits are persisted, so they must never be renumbered;
// new lessons take the next free bit.

/// Changed the angle.
pub const LEARN_ANGLE: u32 = 1 << 0;
/// Changed the power.
pub const LEARN_POWER: u32 = 1 << 1;
/// Fired a shot.
pub const LEARN_FIRE: u32 = 1 << 2;
/// Saw a shot land. The prompt shown until then is that craters persist
/// (DESIGN §2.6).
pub const LEARN_CRATER: u32 = 1 << 3;
/// Fired with a non-zero wind showing, after learning craters.
pub const LEARN_WIND: u32 = 1 << 4;
/// Fired again after learning wind; shells are the run clock.
pub const LEARN_SHELLS: u32 = 1 << 5;
/// Picked a draft card.
pub const LEARN_DRAFT: u32 = 1 << 6;

/// Seconds the muzzle flash stays drawn after firing.
pub const MUZZLE_TIME: f64 = 0.075;
/// Seconds an impact's flash, dust and screen shake last after detonation.
pub const IMPACT_TIME: f64 = 0.24;
/// Seconds the alert border flashes after a structure collapses.
pub const COLLAPSE_TIME: f64 = 0.34;

/// Hold-to-repeat for one input: fires once on the press, waits `delay`, then
/// fires every `rate` seconds while held — the same behaviour as a desktop
/// keyboard's auto-repeat.
///
/// Shared by keyboard, gamepad and on-screen buttons so all three repeat
/// identically. `Default` is the released state.
#[derive(Default, Clone, Copy, Debug)]
pub struct RepeatState {
    down: bool,
    /// Seconds accumulated towards the next repeat. Starts at `-delay` on the
    /// press, so the delay is simply a debt paid off before the first repeat.
    clock: f64,
}

impl RepeatState {
    /// Advance by `delta` seconds with the input `down` or not, and return how
    /// many activations to apply this frame.
    ///
    /// Returns 1 on the frame the input goes down, 0 while released, and
    /// otherwise the number of whole `rate` intervals accumulated. The first
    /// repeat therefore lands `delay + rate` seconds after the press. A long
    /// frame can return several, capped at 20 per call. The cap does not
    /// discard the excess: whatever is left on the clock carries over to the
    /// following frames.
    ///
    /// `rate` must be positive.
    pub fn tick(&mut self, down: bool, delta: f64, delay: f64, rate: f64) -> u32 {
        if !down {
            *self = Self::default();
            return 0;
        }
        if !self.down {
            self.down = true;
            self.clock = -delay;
            return 1;
        }
        self.clock += delta;
        let mut count = 0;
        while self.clock >= rate && count < 20 {
            self.clock -= rate;
            count += 1;
        }
        count
    }
}

/// Linear fade for a flash that started at `at` and lasts `duration` seconds:
/// 1.0 at the start, falling to 0.0 at `at + duration` and staying there.
/// Before `at` it reads 1.0. Clamped to `0.0..=1.0`.
pub fn flash_strength(now: f64, at: f64, duration: f64) -> f64 {
    (1.0 - ((now - at) / duration).max(0.0)).clamp(0.0, 1.0)
}

/// A tiny deterministic shake pattern. It uses animation time only, never
/// randomness or simulation state.
///
/// `age` is seconds since the impact; the shake lasts [`IMPACT_TIME`].
/// `strength` scales the offset, in whole buffer pixels, so the camera stays
/// on the pixel grid (DESIGN §5.1). Returns `(dx, dy)`, or `(0, 0)` outside
/// the window or for a non-positive strength.
pub fn shake_offset(age: f64, strength: i32) -> (i32, i32) {
    if !(0.0..IMPACT_TIME).contains(&age) || strength <= 0 {
        return (0, 0);
    }
    const PATTERN: [(i32, i32); 8] = [
        (1, 0),
        (-1, 1),
        (0, -1),
        (1, 1),
        (-1, 0),
        (0, 1),
        (1, -1),
        (0, 0),
    ];
    // One pattern step per 1/60 s, so the shake looks the same whatever the
    // display's refresh rate.
    let i = (age * 60.0).floor() as usize % PATTERN.len();
    (PATTERN[i].0 * strength, PATTERN[i].1 * strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_has_edge_delay_and_regular_ticks() {
        let mut r = RepeatState::default();
        assert_eq!(r.tick(true, 0.0, 0.28, 0.05), 1);
        assert_eq!(r.tick(true, 0.27, 0.28, 0.05), 0);
        assert_eq!(r.tick(true, 0.07, 0.28, 0.05), 1);
        assert_eq!(r.tick(false, 1.0, 0.28, 0.05), 0);
        assert_eq!(r.tick(true, 0.0, 0.28, 0.05), 1);
    }

    #[test]
    fn effects_expire_and_shake_is_stable() {
        assert_eq!(flash_strength(2.0, 1.0, 0.2), 0.0);
        assert!(flash_strength(1.05, 1.0, 0.2) > 0.0);
        assert_eq!(shake_offset(0.01, 1), shake_offset(0.01, 1));
        assert_eq!(shake_offset(1.0, 2), (0, 0));
    }
}

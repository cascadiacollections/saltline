//! Engine-free helpers for presentation timing, held-input repeat and
//! onboarding bits.
//!
//! Everything here runs on wall-clock seconds from Godot's frame `delta` and
//! only changes what is drawn or how input is paced. None of it feeds back
//! into `salt-line-sim`, whose outcomes depend only on seed and shots; a
//! screen shake that read sim state, or randomness, would make two replays of
//! the same run look different for no reason. Kept free of Godot types so it
//! can be unit-tested.
//!
//! # Onboarding lessons
//!
//! The `LEARN_*` constants are one bit each in the save file's `learned`
//! field ([`Save::learned`](crate::save_format::Save::learned)). A bit is set
//! the first time the player demonstrates the thing.
//! `app::Game::onboarding_prompt` shows the first unset of angle, power, fire, crater,
//! wind, shells; the draft lesson is prompted whenever a draft is open, until
//! learned. Bits are persisted, so they must never be renumbered; new lessons
//! take the next free bit.

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
/// Fired after learning wind with the reserve below its starting value,
/// which is when the shells-are-the-run-clock prompt shows.
pub const LEARN_SHELLS: u32 = 1 << 5;
/// Picked a draft card.
pub const LEARN_DRAFT: u32 = 1 << 6;

/// Seconds the muzzle flash stays drawn after firing.
pub const MUZZLE_TIME: f64 = 0.075;
/// Seconds an impact's flash, dust and screen shake last after detonation.
pub const IMPACT_TIME: f64 = 0.24;
/// Seconds the alert border flashes after a structure collapses.
pub const COLLAPSE_TIME: f64 = 0.34;

/// The most activations [`RepeatState::tick`] returns for one frame, however
/// long the frame was.
pub const MAX_REPEATS: u32 = 20;

/// Hold-to-repeat for one input: fires once on the press, again
/// `delay + rate` later, then every `rate` while held — the same behaviour as
/// a desktop keyboard's auto-repeat.
///
/// Used by the keyboard, the gamepad and on-screen buttons
/// (`input::Controls`, `touch`), all with the core's `REPEAT_DELAY` and
/// `REPEAT_RATE`. `Default` is the released state.
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
    /// frame can return several, capped at [`MAX_REPEATS`] per call. Every
    /// whole interval is taken off the clock either way, so the excess past
    /// the cap is dropped: after a stall the repeat carries on at its rate
    /// instead of firing a burst on each of the following frames. Only the
    /// part-interval left over carries.
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
        if self.clock < rate {
            return 0;
        }
        // Every whole interval at once, not one at a time: a delta of any
        // size costs the same.
        let whole = (self.clock / rate).floor();
        self.clock -= whole * rate;
        if !(0.0..rate).contains(&self.clock) {
            // A clock too large to subtract from exactly.
            self.clock = 0.0;
        }
        (whole as u64).min(u64::from(MAX_REPEATS)) as u32
    }

    /// `tick` for an input that also reports presses as events. A press seen
    /// this frame always counts once and restarts the delay, even when the key
    /// was let go again before the frame polled it, or was already held.
    pub fn tick_pressed(
        &mut self,
        pressed: bool,
        down: bool,
        delta: f64,
        delay: f64,
        rate: f64,
    ) -> u32 {
        if pressed {
            *self = Self {
                down,
                clock: -delay,
            };
            return 1;
        }
        self.tick(down, delta, delay, rate)
    }
}

/// Linear fade for a flash that started at `at` and lasts `duration` seconds:
/// 1.0 at the start, falling to 0.0 at `at + duration` and staying there.
/// Before `at` it reads 1.0. Clamped to `0.0..=1.0`. `duration` must be
/// positive.
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
    fn a_stall_repeats_at_most_the_cap_once_and_then_carries_on_at_the_rate() {
        let mut r = RepeatState::default();
        assert_eq!(r.tick(true, 0.0, 0.28, 0.05), 1);
        // A five-second hitch is a hundred intervals: capped, and the rest
        // thrown away rather than paid out over the next frames.
        assert_eq!(r.tick(true, 5.0 + 0.28, 0.28, 0.05), MAX_REPEATS);
        assert_eq!(r.tick(true, 0.016, 0.28, 0.05), 0);
        assert_eq!(r.tick(true, 0.016, 0.28, 0.05), 0);
        assert_eq!(r.tick(true, 0.02, 0.28, 0.05), 1);
    }

    #[test]
    fn any_delta_returns_at_once() {
        let mut r = RepeatState::default();
        r.tick(true, 0.0, 0.28, 0.05);
        assert_eq!(r.tick(true, 1e15, 0.28, 0.05), MAX_REPEATS);
        assert_eq!(r.tick(true, 0.016, 0.28, 0.05), 0);
        assert_eq!(r.tick(true, f64::MAX, 0.28, 1e-300), MAX_REPEATS);
    }

    #[test]
    fn a_press_between_polls_still_counts_once() {
        let mut r = RepeatState::default();
        // Down and up again inside one frame: never seen held.
        assert_eq!(r.tick_pressed(true, false, 0.016, 0.28, 0.05), 1);
        assert_eq!(r.tick_pressed(false, false, 0.016, 0.28, 0.05), 0);
        // A normal press, then held into repeat.
        assert_eq!(r.tick_pressed(true, true, 0.016, 0.28, 0.05), 1);
        assert_eq!(r.tick_pressed(false, true, 0.27, 0.28, 0.05), 0);
        assert_eq!(r.tick_pressed(false, true, 0.07, 0.28, 0.05), 1);
        // Released and pressed again within a frame restarts the delay.
        assert_eq!(r.tick_pressed(true, true, 0.016, 0.28, 0.05), 1);
        assert_eq!(r.tick_pressed(false, true, 0.2, 0.28, 0.05), 0);
    }

    #[test]
    fn effects_expire_and_shake_is_stable() {
        assert_eq!(flash_strength(2.0, 1.0, 0.2), 0.0);
        assert!(flash_strength(1.05, 1.0, 0.2) > 0.0);
        assert_eq!(shake_offset(0.01, 1), shake_offset(0.01, 1));
        assert_eq!(shake_offset(1.0, 2), (0, 0));
    }
}

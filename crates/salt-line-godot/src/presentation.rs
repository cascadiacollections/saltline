//! Engine-free helpers for presentation timing and held-input repeat.

pub const LEARN_ANGLE: u32 = 1 << 0;
pub const LEARN_POWER: u32 = 1 << 1;
pub const LEARN_FIRE: u32 = 1 << 2;
pub const LEARN_CRATER: u32 = 1 << 3;
pub const LEARN_WIND: u32 = 1 << 4;
pub const LEARN_SHELLS: u32 = 1 << 5;
pub const LEARN_DRAFT: u32 = 1 << 6;

pub const MUZZLE_TIME: f64 = 0.075;
pub const IMPACT_TIME: f64 = 0.24;
pub const COLLAPSE_TIME: f64 = 0.34;

#[derive(Default, Clone, Copy, Debug)]
pub struct RepeatState {
    down: bool,
    clock: f64,
}

impl RepeatState {
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

pub fn flash_strength(now: f64, at: f64, duration: f64) -> f64 {
    (1.0 - ((now - at) / duration).max(0.0)).clamp(0.0, 1.0)
}

/// A tiny deterministic shake pattern. It uses animation time only, never
/// randomness or simulation state.
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

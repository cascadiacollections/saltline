//! Engine-free record of which joypads drive the game, and which of their
//! stick axes can be trusted.
//!
//! Any joypad may play, but not everything that reports as one is a
//! controller in a hand: a drifting stick, a trigger mapped onto a stick axis
//! or a motion sensor can rest past the push threshold forever. Read as held,
//! that would claim the controller every frame, repeat input nobody gave and
//! keep every frame drawing. So a device counts once it presses a button or
//! rests a stick near centre, and a stick axis is trusted only once it has
//! been seen near centre.
//!
//! This is arming, as in a trigger: an axis starts unarmed, and one reading
//! within [`REST`] of centre arms it. A real stick passes near centre as
//! soon as it is nudged or let go, so it arms almost at once; a drifting
//! stick or a trigger resting at one end never does, and is ignored rather
//! than read as held. Once armed, `game.rs` reads an axis as pushed past
//! [`PUSH`] of full deflection.
//!
//! A stick can also start drifting after it armed. So an armed axis that
//! stays pushed for [`DRIFT_TIME`] on end, with no button pressed on that
//! device meanwhile, is disarmed ([`Pads::tick`]), and arms again the next
//! time it reads near centre. A player holding the stick over to sweep the
//! aim lets go, or presses fire, well within that.
//!
//! Button presses are taken from any pad (a press is what makes it count);
//! buttons held down are read only from pads that count, for input; `busy`
//! counts a press on any pad (`Input::is_anything_pressed`).

/// A stick axis this close to centre (as a fraction of full deflection, -1
/// to 1) is at rest, and may be trusted from then.
pub const REST: f32 = 0.2;

/// Past this fraction of full deflection an armed axis reads as pushed.
pub const PUSH: f32 = 0.45;

/// Seconds an armed axis may stay pushed, with no button pressed on its
/// device, before it is taken for drift and disarmed. Longer than sweeping
/// the aim dial end to end by holding the stick (a few seconds at the
/// repeat rate), so a player holding the stick over is not cut off.
pub const DRIFT_TIME: f64 = 8.0;

/// The stick axes the game reads: left X, left Y.
pub const STICK_AXES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pad {
    /// Godot's joypad device index.
    pub device: i32,
    /// Per stick axis: seen at rest, so a push past the threshold is a push.
    pub armed: [bool; STICK_AXES],
    /// Per stick axis: seconds it has been pushed on end with no button
    /// pressed on this device. At [`DRIFT_TIME`] the axis is disarmed.
    pushed_for: [f64; STICK_AXES],
}

/// The joypads that count, in the order they first did.
#[derive(Debug, Default)]
pub struct Pads {
    pads: Vec<Pad>,
}

impl Pads {
    fn entry(&mut self, device: i32) -> &mut Pad {
        let i = match self.pads.iter().position(|p| p.device == device) {
            Some(i) => i,
            None => {
                self.pads.push(Pad {
                    device,
                    armed: [false; STICK_AXES],
                    pushed_for: [0.0; STICK_AXES],
                });
                self.pads.len() - 1
            }
        };
        &mut self.pads[i]
    }

    /// A button went down: the device is in someone's hands, and a stick
    /// held over on it is being held on purpose.
    pub fn button(&mut self, device: i32) {
        self.entry(device).pushed_for = [0.0; STICK_AXES];
    }

    /// An axis moved. `axis` indexes `STICK_AXES`; other axes are ignored.
    pub fn motion(&mut self, device: i32, axis: usize, value: f32) {
        if axis < STICK_AXES && value.abs() < REST {
            let pad = self.entry(device);
            pad.armed[axis] = true;
            pad.pushed_for[axis] = 0.0;
        }
    }

    /// Advance the drift watch by `delta` seconds, once a frame. `value`
    /// reads a device's stick axis now (device, axis index), as `game.rs`
    /// polls it. An armed axis pushed past [`PUSH`] for [`DRIFT_TIME`] on end
    /// is disarmed until it next reads near centre (`motion`).
    pub fn tick(&mut self, delta: f64, value: impl Fn(i32, usize) -> f32) {
        for pad in &mut self.pads {
            for axis in 0..STICK_AXES {
                if !pad.armed[axis] || value(pad.device, axis).abs() <= PUSH {
                    pad.pushed_for[axis] = 0.0;
                    continue;
                }
                pad.pushed_for[axis] += delta;
                if pad.pushed_for[axis] >= DRIFT_TIME {
                    pad.armed[axis] = false;
                    pad.pushed_for[axis] = 0.0;
                }
            }
        }
    }

    /// Forget devices the platform no longer lists as connected.
    pub fn retain_connected(&mut self, connected: &[i32]) {
        self.pads.retain(|p| connected.contains(&p.device));
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pad> {
        self.pads.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stick_resting_off_centre_never_counts() {
        let mut p = Pads::default();
        for _ in 0..10 {
            p.motion(3, 0, 0.9);
        }
        assert_eq!(p.iter().count(), 0);
        // Even once a button makes the device count, the axis stays untrusted.
        p.button(3);
        assert_eq!(p.iter().next().map(|d| d.armed), Some([false, false]));
    }

    #[test]
    fn a_stick_seen_at_rest_is_trusted_per_axis() {
        let mut p = Pads::default();
        p.motion(1, 1, 0.05);
        p.motion(1, 0, 0.6);
        let pad = *p.iter().next().unwrap();
        assert_eq!((pad.device, pad.armed), (1, [false, true]));
        p.motion(1, 4, 0.0);
        assert_eq!(p.iter().next().unwrap().armed, [false, true]);
    }

    /// `frames` frames of 1/60 s with the pad's axes reading `x` and `y`.
    fn hold(p: &mut Pads, frames: usize, x: f32, y: f32) {
        for _ in 0..frames {
            p.tick(1.0 / 60.0, |_, axis| if axis == 0 { x } else { y });
        }
    }

    #[test]
    fn a_stick_that_starts_drifting_later_is_disarmed_until_it_recentres() {
        let mut p = Pads::default();
        p.motion(0, 0, 0.0);
        p.motion(0, 1, 0.0);
        // Held right for a while, as a player sweeping the dial would.
        hold(&mut p, (DRIFT_TIME * 60.0) as usize - 30, 0.9, 0.0);
        assert_eq!(p.iter().next().unwrap().armed, [true, true]);
        // Pushed with nothing else from the device for DRIFT_TIME: drift.
        hold(&mut p, 31, 0.9, 0.0);
        assert_eq!(p.iter().next().unwrap().armed, [false, true]);
        // Back near centre, it is trusted again.
        p.motion(0, 0, 0.1);
        assert_eq!(p.iter().next().unwrap().armed, [true, true]);
    }

    #[test]
    fn a_button_from_the_device_restarts_the_drift_clock() {
        let mut p = Pads::default();
        p.motion(0, 0, 0.0);
        hold(&mut p, (DRIFT_TIME * 60.0) as usize - 30, -0.9, 0.0);
        p.button(0);
        hold(&mut p, (DRIFT_TIME * 60.0) as usize - 30, -0.9, 0.0);
        assert_eq!(p.iter().next().unwrap().armed, [true, false]);
        // A push that lets go restarts it too.
        hold(&mut p, 1, 0.0, 0.0);
        hold(&mut p, (DRIFT_TIME * 60.0) as usize - 30, -0.9, 0.0);
        assert_eq!(p.iter().next().unwrap().armed, [true, false]);
    }

    #[test]
    fn disconnected_devices_are_dropped() {
        let mut p = Pads::default();
        p.button(0);
        p.button(2);
        p.retain_connected(&[2]);
        assert_eq!(p.iter().map(|d| d.device).collect::<Vec<_>>(), [2]);
    }
}

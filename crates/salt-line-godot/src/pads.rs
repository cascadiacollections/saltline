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
//! within [`REST`] of centre arms it for as long as the device stays
//! connected. A real stick passes near centre as soon as it is nudged or
//! let go, so it arms almost at once; a drifting stick or a trigger resting
//! at one end never does, and is ignored rather than read as held. Once
//! armed, `game.rs` reads an axis as pushed past 0.45 of full deflection.
//! Button presses are taken from any pad (a press is what makes it count);
//! buttons held down are read only from pads that count.

/// A stick axis this close to centre (as a fraction of full deflection, -1
/// to 1) is at rest, and may be trusted from then.
pub const REST: f32 = 0.2;

/// The stick axes the game reads: left X, left Y.
pub const STICK_AXES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    /// Godot's joypad device index.
    pub device: i32,
    /// Per stick axis: seen at rest, so a push past the threshold is a push.
    pub armed: [bool; STICK_AXES],
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
                });
                self.pads.len() - 1
            }
        };
        &mut self.pads[i]
    }

    /// A button went down: the device is in someone's hands.
    pub fn button(&mut self, device: i32) {
        self.entry(device);
    }

    /// An axis moved. `axis` indexes `STICK_AXES`; other axes are ignored.
    pub fn motion(&mut self, device: i32, axis: usize, value: f32) {
        if axis < STICK_AXES && value.abs() < REST {
            self.entry(device).armed[axis] = true;
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

    #[test]
    fn disconnected_devices_are_dropped() {
        let mut p = Pads::default();
        p.button(0);
        p.button(2);
        p.retain_connected(&[2]);
        assert_eq!(p.iter().map(|d| d.device).collect::<Vec<_>>(), [2]);
    }
}

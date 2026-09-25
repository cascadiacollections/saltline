//! Engine-free touch input: on-screen buttons, fingers and hold-to-repeat.
//!
//! Each frame the renderer lays out the buttons it drew; the next frame's input
//! is hit-tested against them. A finger belongs to the button it went down on
//! until it lifts, so a finger held through a screen change never presses
//! whatever new button happens to appear under it.

use crate::presentation::RepeatState;

/// A rectangle in buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && y >= self.y as f32
            && x < (self.x + self.w) as f32
            && y < (self.y + self.h) as f32
    }
}

/// An on-screen button that stands in for an input action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub rect: Rect,
    pub action: &'static str,
}

#[derive(Default)]
pub struct TouchPad {
    /// Finger index and the action of the button it went down on, if any.
    fingers: Vec<(i32, Option<&'static str>)>,
    /// Actions whose button a finger went down on since the last update. Kept
    /// separately so a tap that lifts before the next frame still counts.
    went_down: Vec<&'static str>,
    pressed: Vec<&'static str>,
    repeats: Vec<(&'static str, u32)>,
    repeat_state: Vec<(&'static str, RepeatState)>,
}

impl TouchPad {
    /// A finger touched down. Later buttons are drawn over earlier ones, so the
    /// last one containing the point wins.
    pub fn finger_down(&mut self, index: i32, x: f32, y: f32, buttons: &[Button]) {
        let action = buttons
            .iter()
            .rev()
            .find(|b| b.rect.contains(x, y))
            .map(|b| b.action);
        self.fingers.retain(|(i, _)| *i != index);
        self.fingers.push((index, action));
        if let Some(a) = action {
            self.went_down.push(a);
        }
    }

    pub fn finger_up(&mut self, index: i32) {
        self.fingers.retain(|(i, _)| *i != index);
    }

    /// Lift every finger, e.g. when the app loses focus mid-touch.
    pub fn release_all(&mut self) {
        self.fingers.clear();
    }

    /// Once per frame, before any action is read.
    pub fn update(&mut self, delta: f64, delay: f64, rate: f64) {
        self.pressed = std::mem::take(&mut self.went_down);
        let mut held: Vec<&'static str> = self.fingers.iter().filter_map(|(_, a)| *a).collect();
        held.extend(self.pressed.iter().copied());
        held.sort_unstable();
        held.dedup();

        for &a in &held {
            if !self.repeat_state.iter().any(|(s, _)| *s == a) {
                self.repeat_state.push((a, RepeatState::default()));
            }
        }
        self.repeats.clear();
        for (a, state) in &mut self.repeat_state {
            let n = state.tick(held.contains(a), delta, delay, rate);
            if n > 0 {
                self.repeats.push((a, n));
            }
        }
    }

    /// True on the frame a finger went down on this action's button.
    pub fn pressed(&self, action: &str) -> bool {
        self.pressed.contains(&action)
    }

    /// A finger is on this action's button right now.
    pub fn held(&self, action: &str) -> bool {
        self.fingers.iter().any(|(_, a)| *a == Some(action))
    }

    /// Activations this frame, with hold-to-repeat.
    pub fn repeated(&self, action: &str) -> u32 {
        self.repeats
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: f64 = 0.28;
    const RATE: f64 = 0.035;

    fn buttons() -> Vec<Button> {
        vec![
            Button {
                rect: Rect::new(0, 0, 20, 20),
                action: "left",
            },
            Button {
                rect: Rect::new(30, 0, 20, 20),
                action: "fire",
            },
        ]
    }

    #[test]
    fn a_tap_presses_once_even_if_it_lifts_before_the_frame() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 35.0, 5.0, &buttons());
        pad.finger_up(0);
        pad.update(0.016, DELAY, RATE);
        assert!(pad.pressed("fire"));
        assert_eq!(pad.repeated("fire"), 1);
        pad.update(0.016, DELAY, RATE);
        assert!(!pad.pressed("fire"));
        assert_eq!(pad.repeated("fire"), 0);
    }

    #[test]
    fn holding_repeats_after_the_delay_like_a_key() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 5.0, 5.0, &buttons());
        pad.update(0.016, DELAY, RATE);
        assert_eq!(pad.repeated("left"), 1);
        pad.update(DELAY - 0.01, DELAY, RATE);
        assert_eq!(pad.repeated("left"), 0);
        pad.update(0.02 + RATE, DELAY, RATE);
        assert_eq!(pad.repeated("left"), 1);
        assert!(!pad.pressed("left"));
    }

    #[test]
    fn a_finger_stays_with_the_button_it_went_down_on() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 5.0, 5.0, &buttons());
        pad.update(0.016, DELAY, RATE);
        // The screen changed and a different button now sits under the finger.
        let moved = [Button {
            rect: Rect::new(0, 0, 20, 20),
            action: "confirm",
        }];
        pad.finger_down(1, 100.0, 100.0, &moved);
        pad.update(0.016, DELAY, RATE);
        assert!(!pad.pressed("confirm"));
        assert_eq!(pad.repeated("confirm"), 0);
    }

    #[test]
    fn two_fingers_hold_two_buttons() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 5.0, 5.0, &buttons());
        pad.finger_down(1, 35.0, 5.0, &buttons());
        pad.update(0.016, DELAY, RATE);
        assert!(pad.pressed("left") && pad.pressed("fire"));
        pad.finger_up(1);
        pad.update(DELAY + RATE, DELAY, RATE);
        assert_eq!(pad.repeated("left"), 1);
        assert_eq!(pad.repeated("fire"), 0);
    }

    #[test]
    fn a_touch_off_every_button_does_nothing_and_the_topmost_button_wins() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 25.0, 5.0, &buttons());
        pad.update(0.016, DELAY, RATE);
        assert!(!pad.pressed("left") && !pad.pressed("fire"));

        let stacked = [
            Button {
                rect: Rect::new(0, 0, 50, 50),
                action: "back",
            },
            Button {
                rect: Rect::new(10, 10, 10, 10),
                action: "draft_1",
            },
        ];
        pad.finger_down(1, 15.0, 15.0, &stacked);
        pad.update(0.016, DELAY, RATE);
        assert!(pad.pressed("draft_1") && !pad.pressed("back"));
    }

    #[test]
    fn release_all_stops_held_repeats() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 5.0, 5.0, &buttons());
        pad.update(0.016, DELAY, RATE);
        assert!(pad.held("left") && !pad.held("fire"));
        pad.release_all();
        assert!(!pad.held("left"));
        pad.update(DELAY + RATE, DELAY, RATE);
        assert_eq!(pad.repeated("left"), 0);
    }
}

//! Engine-free touch input: on-screen buttons, fingers and hold-to-repeat.
//!
//! Each frame the renderer lays out the buttons it drew; the next frame's input
//! is hit-tested against them. A finger belongs to the button it went down on
//! until it lifts, so a finger held through a screen change never presses
//! whatever new button happens to appear under it.
//!
//! A finger that goes down off every button is a free drag instead (drag
//! aiming). Only one drag runs at a time; lifting it reports a drop for one
//! frame.
//!
//! # How it fits
//!
//! Nothing here depends on Godot, so the whole input model runs under
//! `cargo test` and `proptest`. The engine side, in `game.rs`, converts each
//! `InputEventScreenTouch` / `InputEventScreenDrag` into the node's local
//! space — which is buffer pixels, because the one canvas texture is drawn
//! at the origin at 1:1 (DESIGN §5.1) — and forwards it here. It also keeps
//! the list of [`Button`]s the renderer pushed while drawing the last frame
//! and passes that to [`TouchPad::finger_down`].
//!
//! # Techniques
//!
//! * **Hit-testing against last frame's layout.** Buttons are declared as a
//!   side effect of drawing them (an immediate-mode UI), so the only layout
//!   that exists when a touch arrives is the one the player was looking at.
//!   That is also the right one to test against.
//! * **Capture on touch-down.** A finger is bound to one action for its whole
//!   life, like pointer capture in a desktop toolkit. Sliding off a button
//!   keeps it held; sliding onto one does not press it.
//! * **Edge latching.** Presses are collected as events arrive and published
//!   once per frame by [`TouchPad::update`], so a tap that goes down and up
//!   between two frames is still seen exactly once.
//! * **Hold-to-repeat** shares [`RepeatState`] with the keyboard and gamepad,
//!   so a held on-screen button repeats at the same delay and rate as a key.
//!
//! Actions are `&'static str` names shared with the keyboard bindings
//! (`"left"`, `"fire"`, `"draft_1"`...), so game code asks one question per
//! action whatever the input device.

use crate::presentation::RepeatState;

/// An axis-aligned rectangle in buffer pixels: `x, y` is the top-left corner
/// and `w, h` the size, with y pointing down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// A rectangle with its top-left corner at `(x, y)`.
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether a point in buffer pixels falls inside. Half-open: the left
    /// and top edges are in, the right and bottom edges are out, so two
    /// buttons that share an edge never both claim a touch on it. Takes
    /// floats because touch positions arrive sub-pixel.
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
    /// Hit area in buffer pixels, normally the plate the button is drawn as.
    pub rect: Rect,
    /// The action a press reports, the same name the keyboard binding uses.
    pub action: &'static str,
}

/// Multi-touch state for one frame at a time: which finger holds which
/// button, the one free drag, and per-action repeat timers.
///
/// Feed it events with [`finger_down`](Self::finger_down),
/// [`finger_moved`](Self::finger_moved) and [`finger_up`](Self::finger_up)
/// as they arrive, call [`update`](Self::update) once per frame, then read it
/// with [`pressed`](Self::pressed), [`held`](Self::held),
/// [`repeated`](Self::repeated), [`drag`](Self::drag) and
/// [`dropped`](Self::dropped). All but [`held`](Self::held) and
/// [`drag`](Self::drag) are latched by the update; those two read the live
/// finger state.
///
/// Finger indices are Godot's touch indices; they are only compared, never
/// used to index anything.
#[derive(Default)]
pub struct TouchPad {
    /// Finger index and the action of the button it went down on, if any.
    fingers: Vec<(i32, Option<&'static str>)>,
    /// Actions whose button a finger went down on since the last update. Kept
    /// separately so a tap that lifts before the next frame still counts.
    went_down: Vec<&'static str>,
    /// `went_down` as published by the last update: this frame's presses.
    pressed: Vec<&'static str>,
    /// Activations this frame, per action, from hold-to-repeat.
    repeats: Vec<(&'static str, u32)>,
    /// One repeat timer per action ever held. Small linear-scanned `Vec`s
    /// rather than maps: there are a few dozen actions at most.
    repeat_state: Vec<(&'static str, RepeatState)>,
    drag: Option<Drag>,
    /// A drag that lifted since the last update, as (start, end).
    dropping: Option<((f32, f32), (f32, f32))>,
    /// `dropping` as published by the last update, visible for one frame.
    dropped: Option<((f32, f32), (f32, f32))>,
}

/// The one free drag: the finger that owns it and where it went down and is
/// now, in buffer pixels.
struct Drag {
    index: i32,
    start: (f32, f32),
    now: (f32, f32),
}

impl TouchPad {
    /// A finger touched down at `(x, y)` in buffer pixels. `buttons` is the
    /// layout drawn last frame. Later buttons are drawn over earlier ones, so
    /// the last one containing the point wins.
    ///
    /// Off every button, the finger starts the free drag if none is running;
    /// otherwise it is tracked but does nothing.
    pub fn finger_down(&mut self, index: i32, x: f32, y: f32, buttons: &[Button]) {
        let action = buttons
            .iter()
            .rev()
            .find(|b| b.rect.contains(x, y))
            .map(|b| b.action);
        // A repeated down for the same index (a lost up event) replaces the
        // old binding rather than leaving a finger stuck on a button.
        self.fingers.retain(|(i, _)| *i != index);
        self.fingers.push((index, action));
        match action {
            Some(a) => self.went_down.push(a),
            None if self.drag.is_none() => {
                self.drag = Some(Drag {
                    index,
                    start: (x, y),
                    now: (x, y),
                })
            }
            None => {}
        }
    }

    /// A finger moved to `(x, y)` in buffer pixels. Only the drag finger's
    /// position is tracked; a finger on a button stays bound to it wherever
    /// it moves.
    pub fn finger_moved(&mut self, index: i32, x: f32, y: f32) {
        if let Some(d) = self.drag.as_mut().filter(|d| d.index == index) {
            d.now = (x, y);
        }
    }

    /// A finger lifted. If it owned the drag, the drag becomes a drop, which
    /// the next [`update`](Self::update) publishes through
    /// [`dropped`](Self::dropped).
    pub fn finger_up(&mut self, index: i32) {
        self.fingers.retain(|(i, _)| *i != index);
        if self.drag.as_ref().is_some_and(|d| d.index == index) {
            let d = self.drag.take().unwrap();
            self.dropping = Some((d.start, d.now));
        }
    }

    /// Lift every finger, e.g. when the app loses focus mid-touch. A drag in
    /// progress is abandoned, not dropped.
    pub fn release_all(&mut self) {
        self.fingers.clear();
        self.cancel_drag();
    }

    /// Forget the current drag so it never drops, e.g. when the screen it
    /// started on no longer takes aim.
    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.dropping = None;
        self.dropped = None;
    }

    /// Once per frame, before any action is read. Publishes the presses and
    /// drop collected since the last call and advances every repeat timer by
    /// `delta` seconds. `delay` and `rate` are the hold-to-repeat timings in
    /// seconds, the same ones the keyboard uses; see [`RepeatState::tick`].
    pub fn update(&mut self, delta: f64, delay: f64, rate: f64) {
        self.pressed = std::mem::take(&mut self.went_down);
        self.dropped = self.dropping.take();
        let mut held: Vec<&'static str> = self.fingers.iter().filter_map(|(_, a)| *a).collect();
        // A tap that already lifted still counts as held for this one frame,
        // so its repeat timer sees the edge and reports one activation.
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

    /// Activations this frame, with hold-to-repeat: 1 on the press, 0 during
    /// the delay, then one per `rate` interval elapsed while held (more than
    /// one on a long frame).
    pub fn repeated(&self, action: &str) -> u32 {
        self.repeats
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    /// The free drag in progress, as (start, now) in buffer pixels.
    pub fn drag(&self) -> Option<((f32, f32), (f32, f32))> {
        self.drag.as_ref().map(|d| (d.start, d.now))
    }

    /// A free drag lifted this frame, as (start, end) in buffer pixels.
    /// `Some` for exactly one frame per drop; cancelled or abandoned drags
    /// never drop.
    pub fn dropped(&self) -> Option<((f32, f32), (f32, f32))> {
        self.dropped
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

    #[test]
    fn a_finger_off_every_button_drags_and_drops_once() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 100.0, 50.0, &buttons());
        pad.finger_moved(0, 80.0, 70.0);
        pad.update(0.016, DELAY, RATE);
        assert_eq!(pad.drag(), Some(((100.0, 50.0), (80.0, 70.0))));
        assert_eq!(pad.dropped(), None);
        pad.finger_up(0);
        pad.update(0.016, DELAY, RATE);
        assert_eq!(pad.drag(), None);
        assert_eq!(pad.dropped(), Some(((100.0, 50.0), (80.0, 70.0))));
        pad.update(0.016, DELAY, RATE);
        assert_eq!(pad.dropped(), None);
    }

    #[test]
    fn a_button_finger_never_drags_and_a_second_free_finger_is_ignored() {
        let mut pad = TouchPad::default();
        let b = buttons();
        let r = b[0].rect;
        pad.finger_down(0, r.x as f32, r.y as f32, &b);
        pad.finger_moved(0, 200.0, 170.0);
        assert_eq!(pad.drag(), None);
        pad.finger_down(1, 150.0, 40.0, &b);
        pad.finger_down(2, 160.0, 60.0, &b);
        pad.finger_moved(2, 10.0, 10.0);
        pad.finger_moved(1, 140.0, 50.0);
        assert_eq!(pad.drag(), Some(((150.0, 40.0), (140.0, 50.0))));
    }

    #[test]
    fn a_cancelled_or_abandoned_drag_never_drops() {
        let mut pad = TouchPad::default();
        pad.finger_down(0, 100.0, 50.0, &buttons());
        pad.finger_up(0);
        pad.cancel_drag();
        pad.update(0.016, DELAY, RATE);
        assert_eq!(pad.dropped(), None);
        pad.finger_down(0, 100.0, 50.0, &buttons());
        pad.release_all();
        pad.update(0.016, DELAY, RATE);
        assert_eq!((pad.drag(), pad.dropped()), (None, None));
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        #[derive(Clone, Debug)]
        enum Ev {
            Down(i32, f32, f32),
            Move(i32, f32, f32),
            Up(i32),
            Frame,
            Release,
        }

        fn events() -> impl Strategy<Value = Vec<Ev>> {
            let finger = 0i32..3;
            let x = 0.0f32..60.0;
            let y = 0.0f32..40.0;
            prop::collection::vec(
                prop_oneof![
                    (finger.clone(), x.clone(), y.clone()).prop_map(|(i, x, y)| Ev::Down(i, x, y)),
                    (finger.clone(), x, y).prop_map(|(i, x, y)| Ev::Move(i, x, y)),
                    finger.prop_map(Ev::Up),
                    Just(Ev::Frame),
                    Just(Ev::Release),
                ],
                0..60,
            )
        }

        proptest! {
            #[test]
            fn only_a_finger_off_every_button_drags_and_drops(evs in events()) {
                let b = buttons();
                let on_button = |x: f32, y: f32| b.iter().any(|b| b.rect.contains(x, y));
                let mut pad = TouchPad::default();
                // Where each drag could have started: free fingers' down points.
                let mut free_downs = Vec::new();
                for ev in evs {
                    match ev {
                        Ev::Down(i, x, y) => {
                            if !on_button(x, y) {
                                free_downs.push((x, y));
                            }
                            pad.finger_down(i, x, y, &b);
                        }
                        Ev::Move(i, x, y) => pad.finger_moved(i, x, y),
                        Ev::Up(i) => pad.finger_up(i),
                        Ev::Frame => pad.update(0.016, DELAY, RATE),
                        Ev::Release => pad.release_all(),
                    }
                    for (start, _) in pad.drag().into_iter().chain(pad.dropped()) {
                        prop_assert!(!on_button(start.0, start.1), "a drag started on a button");
                        prop_assert!(free_downs.contains(&start), "a drag started where no finger went down");
                    }
                }
            }
        }
    }
}

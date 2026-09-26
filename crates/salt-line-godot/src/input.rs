//! Engine-free input: keys, pad buttons, their bindings, and what the node
//! hands the core each frame.
//!
//! The node reads the engine's events and polls; [`Intents`] is the result,
//! in terms that name no engine type: which keys and pad buttons went down
//! since the last frame, which are held, and where the left stick points.
//! Touch arrives separately, as it happens, through [`crate::touch`].
//!
//! Actions are not resolved here but in the core (`app`), because the
//! bindings are part of the save and because what a press means depends on
//! the mode: pad X is draft 1 in a draft and COPY on the title. A press is
//! taken by the first action that asks for it, so two actions bound to the
//! same key in the same mode never both fire; the order the core asks in is
//! part of the behaviour. [`Controls`] keeps the frame's presses and the
//! hold-to-repeat state that makes that work.
//!
//! Key and button names are the tokens a `bind` line in the save uses
//! (`bind fire SPACE A`), so [`Key::from_token`] and [`Pad::from_token`] are
//! the whole of the binding format.

use crate::presentation::RepeatState;

/// A keyboard key the game reads. Everything else is ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Space,
    Enter,
    KpEnter,
    Escape,
    BracketLeft,
    BracketRight,
    Comma,
    Period,
    Num1,
    Num2,
    Num3,
    C,
    R,
    S,
    U,
    N,
    V,
    M,
    L,
    /// The debug overlay. Not bindable.
    F1,
    /// Held for coarse aim steps. Not bindable.
    Shift,
}

impl Key {
    pub const ALL: [Key; 25] = [
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::Space,
        Self::Enter,
        Self::KpEnter,
        Self::Escape,
        Self::BracketLeft,
        Self::BracketRight,
        Self::Comma,
        Self::Period,
        Self::Num1,
        Self::Num2,
        Self::Num3,
        Self::C,
        Self::R,
        Self::S,
        Self::U,
        Self::N,
        Self::V,
        Self::M,
        Self::L,
        Self::F1,
        Self::Shift,
    ];

    /// The key a `bind` line names. `None` for anything unknown, which binds
    /// nothing.
    pub fn from_token(token: &str) -> Option<Key> {
        Some(match token {
            "LEFT" => Self::Left,
            "RIGHT" => Self::Right,
            "UP" => Self::Up,
            "DOWN" => Self::Down,
            "SPACE" => Self::Space,
            "ENTER" => Self::Enter,
            "ESCAPE" => Self::Escape,
            "LBRACKET" => Self::BracketLeft,
            "RBRACKET" => Self::BracketRight,
            "COMMA" => Self::Comma,
            "PERIOD" => Self::Period,
            "1" => Self::Num1,
            "2" => Self::Num2,
            "3" => Self::Num3,
            "C" => Self::C,
            "R" => Self::R,
            "S" => Self::S,
            "U" => Self::U,
            "N" => Self::N,
            "V" => Self::V,
            "M" => Self::M,
            "L" => Self::L,
            _ => return None,
        })
    }
}

/// A joypad button the game reads, on any pad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pad {
    A,
    B,
    X,
    Y,
    Start,
    Back,
    LeftShoulder,
    RightShoulder,
    DpadLeft,
    DpadRight,
    DpadUp,
    DpadDown,
}

impl Pad {
    pub const ALL: [Pad; 12] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::Start,
        Self::Back,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::DpadLeft,
        Self::DpadRight,
        Self::DpadUp,
        Self::DpadDown,
    ];

    /// The button a `bind` line names.
    pub fn from_token(token: &str) -> Option<Pad> {
        Some(match token {
            "A" => Self::A,
            "B" => Self::B,
            "X" => Self::X,
            "Y" => Self::Y,
            "START" => Self::Start,
            "BACK" => Self::Back,
            "LB" => Self::LeftShoulder,
            "RB" => Self::RightShoulder,
            "DPAD_LEFT" => Self::DpadLeft,
            "DPAD_RIGHT" => Self::DpadRight,
            "DPAD_UP" => Self::DpadUp,
            "DPAD_DOWN" => Self::DpadDown,
            _ => return None,
        })
    }
}

/// Directions of the left stick, as indices into [`Intents::stick`].
pub const STICK_LEFT: usize = 0;
pub const STICK_RIGHT: usize = 1;
pub const STICK_UP: usize = 2;
pub const STICK_DOWN: usize = 3;

/// The key and pad tokens an action has until a `bind` line says otherwise.
pub fn default_binding(action: &str) -> (&'static str, &'static str) {
    match action {
        "left" => ("LEFT", "DPAD_LEFT"),
        "right" => ("RIGHT", "DPAD_RIGHT"),
        "up" => ("UP", "DPAD_UP"),
        "down" => ("DOWN", "DPAD_DOWN"),
        "fire" => ("SPACE", "A"),
        "fuse_down" => ("LBRACKET", "LB"),
        "fuse_up" => ("RBRACKET", "RB"),
        "reposition_left" => ("COMMA", "X"),
        "reposition_right" => ("PERIOD", "Y"),
        "draft_1" => ("1", "X"),
        "draft_2" => ("2", "A"),
        "draft_3" => ("3", "B"),
        "confirm" => ("ENTER", "A"),
        "back" => ("ESCAPE", "B"),
        "copy" => ("C", "X"),
        "replay" => ("R", "Y"),
        "controls" => ("V", "BACK"),
        "mute" => ("M", "START"),
        "last_arc" => ("L", "X"),
        "seed" => ("S", "Y"),
        // Only offered over a saved run, where confirm is CONTINUE and
        // pad Y is already seed entry.
        "new_run" => ("N", "RB"),
        _ => ("", ""),
    }
}

/// What a prompt calls an action on a touchscreen: the glyph on its on-screen
/// button, or nothing where the prompt's own line is the button.
pub fn touch_label(action: &str) -> &'static str {
    match action {
        "left" => "<",
        "right" => ">",
        "up" => "+",
        "down" => "-",
        "fire" => "FIRE",
        "fuse_down" => "F-",
        "fuse_up" => "F+",
        "reposition_left" => "E<",
        "reposition_right" => "E>",
        "draft_1" | "draft_2" | "draft_3" => "TAP",
        _ => "",
    }
}

/// One frame of keyboard and joypad input, as the node saw it.
#[derive(Clone, Debug, Default)]
pub struct Intents {
    /// Keys that went down since the last frame, oldest first, even if they
    /// came up again before it. Polling alone loses a press and release
    /// inside one frame, so the node queues them as the events arrive.
    pub keys: Vec<Key>,
    /// Keys down now.
    pub keys_held: Vec<Key>,
    /// Pad buttons that went down since the last frame, on any pad.
    pub pads: Vec<Pad>,
    /// Pad buttons down now on any pad that drives the game (`pads::Pads`).
    pub pads_held: Vec<Pad>,
    /// The left stick is pushed past its threshold that way, on a pad whose
    /// stick has been seen at rest. Indexed by `STICK_LEFT` and the rest.
    pub stick: [bool; 4],
}

impl Intents {
    /// Nothing pressed or held.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.keys_held.clear();
        self.pads.clear();
        self.pads_held.clear();
        self.stick = [false; 4];
    }
}

/// This frame's presses, and hold-to-repeat per key, per pad button and per
/// stick slot.
#[derive(Debug, Default)]
pub struct Controls {
    /// This frame's input. Presses are removed as actions take them.
    now: Intents,
    /// Hold-to-repeat per key, shared by every action bound to it.
    keys: Vec<(Key, RepeatState)>,
    /// Hold-to-repeat per pad button, shared by every action bound to it.
    pad_keys: Vec<(Pad, RepeatState)>,
    /// Hold-to-repeat for the left stick, per action slot.
    pad_repeat: [RepeatState; 8],
}

impl Controls {
    /// Once per frame, before any action is read: this frame's presses are
    /// the ones queued since the last. Presses no action took are dropped.
    pub fn begin(&mut self, intents: &Intents) {
        self.now.clone_from(intents);
    }

    /// True on the frame a key goes down, even if it came up again before
    /// the frame began. The press is taken, so of two actions bound to the
    /// same key only the first one asked fires.
    pub fn pressed(&mut self, key: Key) -> bool {
        let Some(i) = self.now.keys.iter().position(|k| *k == key) else {
            return false;
        };
        self.now.keys.swap_remove(i);
        true
    }

    /// The key is down now.
    pub fn held(&self, key: Key) -> bool {
        self.now.keys_held.contains(&key)
    }

    /// Number of activations this frame, with hold-to-repeat. The repeat
    /// state is per key, so actions sharing a key share its repeats.
    pub fn repeated(&mut self, key: Key, delta: f64, delay: f64, rate: f64) -> u32 {
        let pressed = self.pressed(key);
        let down = self.held(key);
        let i = match self.keys.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                self.keys.push((key, RepeatState::default()));
                self.keys.len() - 1
            }
        };
        self.keys[i]
            .1
            .tick_pressed(pressed, down, delta, delay, rate)
    }

    /// Take this frame's press of a pad button, if it had one. Taken, so a
    /// second action bound to the same button in the same mode does not
    /// fire as well: the first one asked wins.
    pub fn take_pad(&mut self, button: Pad) -> bool {
        let Some(i) = self.now.pads.iter().position(|b| *b == button) else {
            return false;
        };
        self.now.pads.swap_remove(i);
        true
    }

    /// `repeated` for a pad button.
    pub fn pad_repeated(&mut self, button: Pad, delta: f64, delay: f64, rate: f64) -> u32 {
        let press = self.take_pad(button);
        let down = self.now.pads_held.contains(&button);
        let i = match self.pad_keys.iter().position(|(k, _)| *k == button) {
            Some(i) => i,
            None => {
                self.pad_keys.push((button, RepeatState::default()));
                self.pad_keys.len() - 1
            }
        };
        self.pad_keys[i]
            .1
            .tick_pressed(press, down, delta, delay, rate)
    }

    /// The stick is pushed that way (`STICK_LEFT` and the rest).
    pub fn stick(&self, dir: usize) -> bool {
        self.now.stick.get(dir).copied().unwrap_or(false)
    }

    /// Hold-to-repeat for a stick direction, kept per action slot. Ticked
    /// every time the slot is asked, pushed or not, so a release resets it.
    pub fn stick_repeated(
        &mut self,
        slot: usize,
        down: bool,
        delta: f64,
        delay: f64,
        rate: f64,
    ) -> u32 {
        self.pad_repeat[slot].tick(down, delta, delay, rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: [&str; 21] = [
        "left",
        "right",
        "up",
        "down",
        "fire",
        "fuse_down",
        "fuse_up",
        "reposition_left",
        "reposition_right",
        "draft_1",
        "draft_2",
        "draft_3",
        "confirm",
        "back",
        "copy",
        "replay",
        "controls",
        "mute",
        "last_arc",
        "seed",
        "new_run",
    ];

    #[test]
    fn every_default_binding_names_a_real_key_and_button() {
        for action in ACTIONS {
            let (key, pad) = default_binding(action);
            assert!(Key::from_token(key).is_some(), "{action}: {key}");
            assert!(Pad::from_token(pad).is_some(), "{action}: {pad}");
        }
        assert_eq!(default_binding("no_such_action"), ("", ""));
    }

    #[test]
    fn a_press_is_taken_by_the_first_asker_only() {
        let mut c = Controls::default();
        c.begin(&Intents {
            keys: vec![Key::Space],
            pads: vec![Pad::A],
            ..Intents::default()
        });
        assert!(c.pressed(Key::Space));
        assert!(!c.pressed(Key::Space));
        assert!(c.take_pad(Pad::A));
        assert!(!c.take_pad(Pad::A));
    }

    #[test]
    fn two_presses_in_one_frame_are_two_presses() {
        let mut c = Controls::default();
        c.begin(&Intents {
            keys: vec![Key::Left, Key::Left],
            ..Intents::default()
        });
        assert!(c.pressed(Key::Left));
        assert!(c.pressed(Key::Left));
        assert!(!c.pressed(Key::Left));
    }

    #[test]
    fn untaken_presses_do_not_carry_into_the_next_frame() {
        let mut c = Controls::default();
        c.begin(&Intents {
            keys: vec![Key::M],
            ..Intents::default()
        });
        c.begin(&Intents::default());
        assert!(!c.pressed(Key::M));
    }

    #[test]
    fn a_held_key_repeats_after_the_delay_then_at_the_rate() {
        let (delay, rate) = (0.28, 0.035);
        let mut c = Controls::default();
        c.begin(&Intents {
            keys: vec![Key::Up],
            keys_held: vec![Key::Up],
            ..Intents::default()
        });
        assert_eq!(c.repeated(Key::Up, 0.016, delay, rate), 1);
        let held = Intents {
            keys_held: vec![Key::Up],
            ..Intents::default()
        };
        // 20 more frames is 0.32 s held: past the delay and one interval.
        let mut total = 0;
        for _ in 0..20 {
            c.begin(&held);
            total += c.repeated(Key::Up, 0.016, delay, rate);
        }
        assert_eq!(total, 1);
        c.begin(&Intents::default());
        assert_eq!(c.repeated(Key::Up, 0.016, delay, rate), 0);
    }

    #[test]
    fn a_tap_between_frames_still_counts_once() {
        let mut c = Controls::default();
        c.begin(&Intents {
            pads: vec![Pad::DpadLeft],
            ..Intents::default()
        });
        assert_eq!(c.pad_repeated(Pad::DpadLeft, 0.016, 0.28, 0.035), 1);
    }
}

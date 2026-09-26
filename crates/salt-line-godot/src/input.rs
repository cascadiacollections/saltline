//! Engine-free input: keys, pad buttons, their bindings, and what the node
//! hands the core each frame.
//!
//! The node reads the engine's events and polls; [`Intents`] is the result,
//! in terms that name no engine type: which keys and pad buttons went down
//! since the last frame, which are held, and where the left stick points,
//! with the window size, display cutout and date the frame sees. Touch
//! is among the frame's events and reaches [`crate::touch`] through the core.
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

/// Something the player can do: a bindable action (`fire`, `back`...), or a
/// touch-only control (the Controls toggles, the seed keypad). Game code asks
/// about actions, never about keys, pads or buttons, and an enum means an
/// action spelled wrong does not build rather than never firing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Action {
    Left,
    Right,
    Up,
    Down,
    Fire,
    FuseDown,
    FuseUp,
    RepositionLeft,
    RepositionRight,
    Draft1,
    Draft2,
    Draft3,
    Confirm,
    Back,
    Copy,
    Replay,
    Controls,
    Mute,
    LastArc,
    Seed,
    NewRun,
    AimMode,
    ArcHint,
    /// A seed keypad key: types its character.
    Type(char),
    /// The seed keypad's DEL.
    Delete,
}

impl Action {
    /// Every action a `bind` line can name, in the order the save file and
    /// the Controls screen know them.
    pub const BINDABLE: [Action; 21] = [
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::Fire,
        Self::FuseDown,
        Self::FuseUp,
        Self::RepositionLeft,
        Self::RepositionRight,
        Self::Draft1,
        Self::Draft2,
        Self::Draft3,
        Self::Confirm,
        Self::Back,
        Self::Copy,
        Self::Replay,
        Self::Controls,
        Self::Mute,
        Self::LastArc,
        Self::Seed,
        Self::NewRun,
    ];

    /// The action's name in a `bind` line (`bind fire SPACE A`). `None` for a
    /// touch-only control, which no line can bind.
    pub fn token(self) -> Option<&'static str> {
        Some(match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
            Self::Fire => "fire",
            Self::FuseDown => "fuse_down",
            Self::FuseUp => "fuse_up",
            Self::RepositionLeft => "reposition_left",
            Self::RepositionRight => "reposition_right",
            Self::Draft1 => "draft_1",
            Self::Draft2 => "draft_2",
            Self::Draft3 => "draft_3",
            Self::Confirm => "confirm",
            Self::Back => "back",
            Self::Copy => "copy",
            Self::Replay => "replay",
            Self::Controls => "controls",
            Self::Mute => "mute",
            Self::LastArc => "last_arc",
            Self::Seed => "seed",
            Self::NewRun => "new_run",
            Self::AimMode | Self::ArcHint | Self::Type(_) | Self::Delete => return None,
        })
    }

    /// Inverse of [`token`](Self::token). `None` for a name no build binds;
    /// a `bind` line naming it binds nothing.
    pub fn from_token(token: &str) -> Option<Action> {
        Self::BINDABLE
            .into_iter()
            .find(|a| a.token() == Some(token))
    }
}

/// Directions of the left stick, as indices into [`Intents::stick`].
pub const STICK_LEFT: usize = 0;
pub const STICK_RIGHT: usize = 1;
pub const STICK_UP: usize = 2;
pub const STICK_DOWN: usize = 3;

/// The key and pad tokens an action has until a `bind` line says otherwise.
pub fn default_binding(action: Action) -> (&'static str, &'static str) {
    match action {
        Action::Left => ("LEFT", "DPAD_LEFT"),
        Action::Right => ("RIGHT", "DPAD_RIGHT"),
        Action::Up => ("UP", "DPAD_UP"),
        Action::Down => ("DOWN", "DPAD_DOWN"),
        Action::Fire => ("SPACE", "A"),
        Action::FuseDown => ("LBRACKET", "LB"),
        Action::FuseUp => ("RBRACKET", "RB"),
        Action::RepositionLeft => ("COMMA", "X"),
        Action::RepositionRight => ("PERIOD", "Y"),
        Action::Draft1 => ("1", "X"),
        Action::Draft2 => ("2", "A"),
        Action::Draft3 => ("3", "B"),
        Action::Confirm => ("ENTER", "A"),
        Action::Back => ("ESCAPE", "B"),
        Action::Copy => ("C", "X"),
        Action::Replay => ("R", "Y"),
        Action::Controls => ("V", "BACK"),
        Action::Mute => ("M", "START"),
        Action::LastArc => ("L", "X"),
        Action::Seed => ("S", "Y"),
        // Only offered over a saved run, where confirm is CONTINUE and
        // pad Y is already seed entry.
        Action::NewRun => ("N", "RB"),
        _ => ("", ""),
    }
}

/// What a prompt calls an action on a touchscreen: the glyph on its on-screen
/// button, or nothing where the prompt's own line is the button.
pub fn touch_label(action: Action) -> &'static str {
    match action {
        Action::Left => "<",
        Action::Right => ">",
        Action::Up => "+",
        Action::Down => "-",
        Action::Fire => "FIRE",
        Action::FuseDown => "F-",
        Action::FuseUp => "F+",
        Action::RepositionLeft => "E<",
        Action::RepositionRight => "E>",
        Action::Draft1 | Action::Draft2 | Action::Draft3 => "TAP",
        _ => "",
    }
}

/// A key typed in seed entry: text comes from the keyboard's characters, not
/// its bindings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Typed {
    Backspace,
    Enter,
    Escape,
    /// A printable ASCII character.
    Char(char),
}

/// Something the player did between two frames. Kept in order: a key typed
/// in seed entry must know whether the Enter before it has already started
/// the run.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    /// A key went down. `key` is the key, if the game reads it; `typed` is
    /// what it types in seed entry, if anything. A press and release inside
    /// one frame still counts: polling alone would lose it.
    Key {
        key: Option<Key>,
        typed: Option<Typed>,
    },
    /// A finger touched down at `(x, y)` in buffer pixels.
    FingerDown {
        index: i32,
        x: f32,
        y: f32,
    },
    FingerMoved {
        index: i32,
        x: f32,
        y: f32,
    },
    FingerUp {
        index: i32,
    },
    /// The platform's back gesture (Android).
    Back,
}

impl Event {
    /// A key press that types nothing.
    pub fn key(key: Key) -> Event {
        Event::Key {
            key: Some(key),
            typed: None,
        }
    }
}

/// One frame of input, as the node saw it.
#[derive(Clone, Debug, Default)]
pub struct Intents {
    /// Everything the player did since the last frame, oldest first.
    pub events: Vec<Event>,
    /// Keys down now.
    pub keys_held: Vec<Key>,
    /// Pad buttons that went down since the last frame, on any pad.
    pub pads: Vec<Pad>,
    /// Pad buttons down now on any pad that drives the game (`pads::Pads`).
    pub pads_held: Vec<Pad>,
    /// The left stick is pushed past its threshold that way, on a pad whose
    /// stick has been seen at rest. Indexed by `STICK_LEFT` and the rest.
    pub stick: [bool; 4],

    /// The window in screen pixels.
    pub window: (i32, i32),
    /// Screen pixels a display cutout takes from the window's left and right
    /// edges: a camera hole on a phone held landscape. Zero on a desktop.
    pub cutout: (i32, i32),
    /// Days since the Unix epoch, for the daily seed. The clock is read by
    /// the node; nothing in the core or the sim touches system time.
    pub today: u64,
}

impl Intents {
    /// Nothing pressed or held. The window, cutout and day are kept.
    pub fn clear(&mut self) {
        self.events.clear();
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
    /// Key presses since the last frame, oldest first, waiting for it.
    queued: Vec<Key>,
    /// This frame's key presses. Removed as actions take them.
    presses: Vec<Key>,
    /// This frame's input. Pad presses are removed as actions take them.
    now: Intents,
    /// Hold-to-repeat per key, shared by every action bound to it.
    keys: Vec<(Key, RepeatState)>,
    /// Hold-to-repeat per pad button, shared by every action bound to it.
    pad_keys: Vec<(Pad, RepeatState)>,
    /// Hold-to-repeat for the left stick, per action slot.
    pad_repeat: [RepeatState; 8],
}

impl Controls {
    /// A key went down: it is pressed on the next frame.
    pub fn queue(&mut self, key: Key) {
        self.queued.push(key);
    }

    /// The key typed text instead: forget its presses queued so far.
    pub fn swallow(&mut self, key: Key) {
        self.queued.retain(|k| *k != key);
    }

    /// Once per frame, before any action is read: this frame's presses are
    /// the ones queued since the last. Presses no action took are dropped.
    pub fn begin(&mut self, intents: &Intents) {
        std::mem::swap(&mut self.presses, &mut self.queued);
        self.queued.clear();
        self.now.clone_from(intents);
    }

    /// True on the frame a key goes down, even if it came up again before
    /// the frame began. The press is taken, so of two actions bound to the
    /// same key only the first one asked fires.
    pub fn pressed(&mut self, key: Key) -> bool {
        let Some(i) = self.presses.iter().position(|k| *k == key) else {
            return false;
        };
        self.presses.swap_remove(i);
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

    #[test]
    fn every_default_binding_names_a_real_key_and_button() {
        for action in Action::BINDABLE {
            let (key, pad) = default_binding(action);
            assert!(Key::from_token(key).is_some(), "{action:?}: {key}");
            assert!(Pad::from_token(pad).is_some(), "{action:?}: {pad}");
        }
        assert_eq!(default_binding(Action::AimMode), ("", ""));
    }

    #[test]
    fn every_bindable_action_round_trips_through_its_bind_name() {
        for action in Action::BINDABLE {
            let token = action.token().unwrap();
            assert_eq!(Action::from_token(token), Some(action), "{token}");
        }
        // The names existing saves use, spelled out so a rename shows here.
        let names: Vec<&str> = Action::BINDABLE
            .iter()
            .map(|a| a.token().unwrap())
            .collect();
        assert_eq!(
            names,
            [
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
            ]
        );
    }

    #[test]
    fn touch_only_controls_cannot_be_bound() {
        for action in [
            Action::AimMode,
            Action::ArcHint,
            Action::Delete,
            Action::Type('A'),
        ] {
            assert_eq!(action.token(), None);
        }
        for name in ["aim_mode", "arc_hint", "del", "A", "Fire", ""] {
            assert_eq!(Action::from_token(name), None, "{name}");
        }
    }

    #[test]
    fn a_swallowed_key_loses_the_presses_queued_before_it_only() {
        let mut c = Controls::default();
        c.queue(Key::M);
        c.queue(Key::Space);
        c.queue(Key::M);
        c.swallow(Key::M);
        c.queue(Key::M);
        c.begin(&Intents::default());
        assert!(c.pressed(Key::Space));
        assert!(c.pressed(Key::M));
        assert!(!c.pressed(Key::M));
    }

    #[test]
    fn a_press_is_taken_by_the_first_asker_only() {
        let mut c = Controls::default();
        c.queue(Key::Space);
        c.begin(&Intents {
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
        c.queue(Key::Left);
        c.queue(Key::Left);
        c.begin(&Intents::default());
        assert!(c.pressed(Key::Left));
        assert!(c.pressed(Key::Left));
        assert!(!c.pressed(Key::Left));
    }

    #[test]
    fn untaken_presses_do_not_carry_into_the_next_frame() {
        let mut c = Controls::default();
        c.queue(Key::M);
        c.begin(&Intents::default());
        c.begin(&Intents::default());
        assert!(!c.pressed(Key::M));
    }

    #[test]
    fn a_held_key_repeats_after_the_delay_then_at_the_rate() {
        let (delay, rate) = (0.28, 0.035);
        let mut c = Controls::default();
        c.queue(Key::Up);
        c.begin(&Intents {
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

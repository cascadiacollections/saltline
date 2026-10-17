//! Engine-free frame pacing: when to redraw, how fast to run while idle, when
//! to ask the display about its shape again, and a one-entry memo for work
//! that only changes with its inputs.
//!
//! The frame is one software-rasterised buffer (DESIGN §5.1), so an
//! unchanged frame is pure waste on a phone: a full raster pass and a
//! texture upload for pixels already on screen.
//!
//! # Dirty flag plus busy
//!
//! [`Redraw`] is the usual dirty-flag pattern with one addition. Two things
//! make a frame due:
//!
//! - *dirty*: something marked it. The core returns `app::Effect::Redraw`
//!   when what it shows changed, and `game.rs` marks every input event
//!   (except mouse and stick motion), the back gesture and focus returning.
//! - *busy*: something moves on its own, so every frame differs. That is
//!   `app::Game::busy` (a shell in flight, a clear being held, a note
//!   counting down, a finger on the screen, the developer autopilot) plus
//!   what only the node can see: a key or button held, or a trusted stick
//!   pushed (`pads`).
//!
//! The addition is one frame more after busy ends. The last busy frame drew
//! the effect in its final state; the one after it draws the scene without
//! it, so nothing that ran out is left frozen on screen.
//!
//! ```text
//! frame     1     2     3     4     5     6
//! busy      -     -     yes   yes   -     -
//! marked    yes   -     -     -     -     -
//! drawn     yes   -     yes   yes   yes   -
//! ```
//!
//! Skipping the draw is only half the saving. Menus also lower the engine's
//! frame cap to [`IDLE_FPS`] ([`frame_cap`]), so the engine's own loop wakes
//! less often; play keeps the project's cap, because a shell in flight
//! should be smooth.

/// Menus and overlays run at this rate at most, in frames per second.
/// Nothing on them moves on its own, and a highlight or a typed character a
/// frame later at 30 fps is not something a thumb can feel.
pub const IDLE_FPS: i32 = 30;

/// How often, in seconds, the display's safe area is asked again when
/// nothing else has prompted it: a phone turned end over end keeps its
/// window size, but the camera hole moves to the other side.
pub const DISPLAY_RECHECK: f64 = 1.0;

/// Decides whether this frame needs rasterising and uploading.
///
/// Conservative by design: a frame is drawn when anything marked it, when
/// anything is moving, and once more after the last moving frame, so an
/// effect or a note that runs out is cleared rather than left on screen.
#[derive(Debug)]
pub struct Redraw {
    /// Marked since the last frame.
    dirty: bool,
    /// The last frame was busy, so this one is due even if it is not.
    was_busy: bool,
}

impl Default for Redraw {
    /// The first frame is always drawn.
    fn default() -> Self {
        Self {
            dirty: true,
            was_busy: false,
        }
    }
}

impl Redraw {
    /// Something the frame shows has changed.
    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// Once per frame. `busy` is whether anything is animating or held right
    /// now. True means draw this frame.
    pub fn due(&mut self, busy: bool) -> bool {
        let due = self.dirty || busy || self.was_busy;
        self.dirty = false;
        self.was_busy = busy;
        due
    }
}

/// The engine's frame cap for this frame: `IDLE_FPS` on menus, or the
/// project's own cap (0 is unlimited) in play. A project cap tighter than
/// `IDLE_FPS` is kept everywhere.
pub fn frame_cap(on_menu: bool, project_cap: i32) -> i32 {
    match (on_menu, project_cap) {
        (false, cap) => cap,
        (true, cap) if cap > 0 => cap.min(IDLE_FPS),
        (true, _) => IDLE_FPS,
    }
}

/// The tighter of two frame caps where 0 means "no cap": the project's own
/// and the platform's power policy (the `device_power` addon, which also
/// owns Android Game Mode).
pub fn tighter_cap(a: i32, b: i32) -> i32 {
    match (a > 0, b > 0) {
        (true, true) => a.min(b),
        (true, false) => a,
        (false, true) => b,
        (false, false) => 0,
    }
}

/// Whether to ask the display for its size and safe area again.
#[derive(Debug, Default)]
pub struct DisplayPoll {
    size: Option<(i32, i32)>,
    stale: bool,
    clock: f64,
}

impl DisplayPoll {
    /// Focus came back or the app resumed: the display may have changed shape
    /// while nobody was looking.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Once per frame with the window size, which is cheap to read. True when
    /// the safe area should be read again, which may not be.
    pub fn check(&mut self, size: (i32, i32), delta: f64) -> bool {
        self.clock += delta;
        let due = self.stale || self.size != Some(size) || self.clock >= DISPLAY_RECHECK;
        if due {
            self.size = Some(size);
            self.stale = false;
            self.clock = 0.0;
        }
        due
    }
}

/// One cached result and the key it was computed for: memoisation with a
/// cache of one, which is all a per-frame computation needs when its inputs
/// rarely change. `render` keeps the drag preview's trajectory, a whole
/// ballistic flight, in one.
#[derive(Debug)]
pub struct Memo<K, V> {
    entry: Option<(K, V)>,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// The value for `key`, computed only when the key differs from last time.
    pub fn get_or(&mut self, key: K, compute: impl FnOnce() -> V) -> &V {
        if self.entry.as_ref().is_none_or(|(k, _)| *k != key) {
            self.entry = Some((key, compute()));
        }
        &self.entry.as_ref().expect("just filled").1
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_draws_then_an_idle_one_does_not() {
        let mut r = Redraw::default();
        assert!(r.due(false));
        assert!(!r.due(false));
        r.mark();
        assert!(r.due(false));
        assert!(!r.due(false));
    }

    #[test]
    fn a_busy_spell_draws_throughout_and_once_after() {
        let mut r = Redraw::default();
        r.due(false);
        assert!(r.due(true));
        assert!(r.due(true));
        // The frame the effect ran out still draws, so it is wiped.
        assert!(r.due(false));
        assert!(!r.due(false));
    }

    #[test]
    fn menus_cap_and_play_keeps_the_project_cap() {
        assert_eq!(frame_cap(true, 0), IDLE_FPS);
        assert_eq!(frame_cap(false, 0), 0);
        assert_eq!(frame_cap(true, 120), IDLE_FPS);
        assert_eq!(frame_cap(true, 20), 20);
        assert_eq!(frame_cap(false, 120), 120);
    }

    #[test]
    fn the_tighter_cap_wins_and_zero_is_no_cap() {
        assert_eq!(tighter_cap(0, 0), 0);
        assert_eq!(tighter_cap(0, 60), 60);
        assert_eq!(tighter_cap(144, 0), 144);
        assert_eq!(tighter_cap(45, 60), 45);
        assert_eq!(tighter_cap(120, 60), 60);
        assert_eq!(tighter_cap(-1, 60), 60);
        // Menus still drop below a platform cap.
        assert_eq!(frame_cap(true, tighter_cap(0, 60)), IDLE_FPS);
        assert_eq!(frame_cap(false, tighter_cap(0, 60)), 60);
    }

    #[test]
    fn the_display_is_asked_on_size_change_focus_and_now_and_then() {
        let mut p = DisplayPoll::default();
        assert!(p.check((1280, 720), 0.016));
        assert!(!p.check((1280, 720), 0.016));
        assert!(p.check((720, 1280), 0.016));
        assert!(!p.check((720, 1280), 0.016));
        p.invalidate();
        assert!(p.check((720, 1280), 0.016));
        assert!(!p.check((720, 1280), DISPLAY_RECHECK - 0.1));
        assert!(p.check((720, 1280), 0.2));
    }

    #[test]
    fn a_memo_recomputes_only_when_its_key_changes() {
        let mut m: Memo<(i32, i32), i32> = Memo::default();
        let mut runs = 0;
        for key in [(1, 2), (1, 2), (1, 3), (1, 3), (1, 2)] {
            m.get_or(key, || {
                runs += 1;
                key.0 + key.1
            });
        }
        assert_eq!(runs, 3);
        assert_eq!(*m.get_or((1, 2), || unreachable!()), 3);
        m.clear();
        assert_eq!(*m.get_or((1, 2), || 7), 7);
    }
}

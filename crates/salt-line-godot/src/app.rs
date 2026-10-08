//! The game itself, with no engine in it.
//!
//! # Core and adapter
//!
//! The game is split in three, so that everything that decides anything can
//! be run and checked under `cargo test`:
//!
//! * **This module, the core.** [`Game`] owns the mode, the run, the shot
//!   being replayed, the aim, the drag, the onboarding prompts, the save and
//!   the bookkeeping that keeps a run on disk (DESIGN §9.6). It is stepped by
//!   [`Game::tick`] with a frame's [`Intents`] and answers with the
//!   [`Effect`]s the engine must carry out: play a sound, write a file, set
//!   the clipboard, resize the buffer, quit. It reads no clock, file or device
//!   of its own; what it needs from the outside comes in as arguments, and
//!   what it wants done goes out as effects.
//! * **`render`, the drawing.** Functions over `&Game` that paint one frame
//!   into a [`Canvas`](crate::raster::Canvas), the one buffer of DESIGN §5.1,
//!   and lay out the on-screen buttons the next frame's touches are tested
//!   against.
//! * **`game`, the adapter.** The Godot node. It turns the engine's events
//!   into [`Intents`] and calls into the core, carries out the effects
//!   (audio players, `user://` files through `save`, the clipboard, the
//!   window), uploads the canvas as a texture, and handles the notifications.
//!
//! The sim (`salt-line-sim`) sits under all three and knows none of them.
//!
//! ## Why split it
//!
//! * **Testability.** Godot cannot be started under `cargo test`, and a
//!   node's `process` cannot be called without it. Everything worth testing
//!   (menus, pausing, the order of disk writes, what happens after a crash)
//!   is therefore kept out of the node, where a test can build a [`Game`],
//!   hand it a frame of input and read back what it asked for.
//! * **The engine boundary.** Nothing in this module names a Godot type.
//!   Keys, pad buttons and touches arrive as the crate's own
//!   [`Key`], [`Pad`] and
//!   [`Event`]; files, sounds and the window leave as
//!   [`Effect`]s. Moving to another engine, or another gdext release, means
//!   rewriting `game.rs` and nothing here.
//! * **One direction of flow.** Each frame runs the same way round:
//!   intents in, state changed, effects out, then a frame drawn from the
//!   state. The core never calls the engine back mid-frame, so the order of
//!   everything it asked for is visible in one `Vec` (the effect, or
//!   command, pattern: requests as data, carried out by someone else).
//!
//! ```text
//!  Intents ──► Game::tick ──► Vec<Effect> ──► adapter carries them out
//!                  │                          (Store feeds back via stored())
//!                  ▼
//!            &Game ──► render::frame ──► Canvas + Vec<Button>
//!                                            │          │
//!                                   texture upload   set_buttons: next
//!                                                    frame's touches
//! ```
//!
//! The frame loop as the node runs it is drawn at the top of `game.rs`.
//!
//! `Game`'s methods are split by concern: this file has the frame, the
//! environment events, launch and the read-only queries; `app/controls.rs`
//! reads actions and runs each mode's input; `app/play.rs` is a run in play;
//! `app/versus.rs` is a local versus match in play; `app/demo.rs` is the
//! attract demo, which plays both of those for the camera without touching
//! anything the player keeps; `app/watch.rs` plays a finished run or match
//! again from its log the same way, and is the REPLAYS list;
//! `app/session.rs` is the buffer, sound,
//! settings, crash reports and the run on disk.
//!
//! # Order is behaviour
//!
//! Effects are returned in the order the core asked for them, and the
//! adapter applies them in that order. Some orders are load-bearing: the run
//! file is cleared before a finished run's score goes on the board, and the
//! score is written only if that is confirmed, so a crash in between loses
//! the score rather than recording it twice.
//!
//! A save write ([`Effect::Store`]) is reported back through
//! [`Game::stored`], which may say so on screen and ask for more. The adapter
//! applies what `stored` returns at once, before the rest of the list it was
//! working through: a failed score's `RunFile::Write`, which keeps its log,
//! lands straight after the failed write.
//!
//! # After a panic
//!
//! Nothing is written once a panic has happened ([`Effect::survives_panic`]):
//! the state in memory is not trusted enough to replace what is on disk, and
//! the crash report the hook just wrote must stay where the next launch looks
//! for it. The check lives in the adapter, on every effect it applies,
//! because a panic can land mid-frame with effects already queued and before
//! [`Game::broken_tick`] has run; `broken_tick` also drops whatever the
//! crashed frame left queued.
//!
//! Player input comes in with the frame, in [`Intents`]: what is held, and
//! the events since the last frame in the order they happened (keys, typed
//! text, touches, the back gesture). Only the notifications that must not
//! wait for a frame — focus lost, the app paused by the platform, the window
//! closing — are methods the adapter calls as they arrive, returning their
//! effects at once: a phone may never resume a paused app, so its settings
//! are written now. They take the events queued since the last frame first,
//! so nothing is taken out of order.
//!
//! # Effect catalogue
//!
//! Every [`Effect`], who asks for it, and what `game.rs` does with it. Only
//! a save write (`Store`) and `ReadClipboard` report back; only effects
//! that touch a file are dropped after a panic.
//!
//! | Effect | Emitted by | Adapter action | Feedback via `stored()` | Dropped after panic |
//! |---|---|---|---|---|
//! | `Play(Sound)` | firing, landing and collapse (`app/play.rs`), a versus shot landing or hitting and a round or match won (`app/versus.rs`), a draft offered or taken, menu choices, the low-shell warning | `play()` on that sound's `AudioStreamPlayer` | no | no |
//! | `Volume(dB)` | mute, the Controls volume (the node sets it once itself at launch) | `set_volume_db` on every player | no | no |
//! | `TitleMusic(on)` | the title music toggle and transitions to/from the title | start or stop the looping title player | no | no |
//! | `Redraw` | any status note, a buffer change, pausing, and [`Game::tick`] when the frame's [`FrameState`] changed | `frame::Redraw::mark` | no | no |
//! | `RecheckDisplay` | [`Game::tick`], on entering Title or GameOver | `frame::DisplayPoll::invalidate` | no | no |
//! | `Buffer { w, h, letterbox }` | launch, a run started or continued, a draft taken, a screen change, a versus match started or left, a change of window size or cutout | resize the canvas, set the window's content scale size, offset the node | no | no |
//! | `Aspect` | a run started or continued, leaving a resumed run | set the window's content scale aspect | no | no |
//! | `Clipboard(text)` | COPY SEED, COPY LAST CRASH REPORT, the error screen's COPY REPORT, the editor's COPY CODE | `DisplayServer::clipboard_set` | no | no |
//! | `ReadClipboard` | the editor's PASTE CODE (`app/editor.rs`) | `DisplayServer::clipboard_get`, handed to [`Game::pasted`], whose effects are applied at once | no (the text comes back through `pasted`) | no |
//! | `Quit` | back on the title or the error screen | `SceneTree::quit` | no | no |
//! | `Store(Learned)` | an onboarding step done for the first time | `save::store`, then [`Game::stored`] | yes, ignored | yes |
//! | `Store(Settings)` | settings once they settle (`durable::Debounce`) or on leaving Controls; at once on focus lost, pause and close | `save::store`, then [`Game::stored`] | yes: "SETTINGS NOT SAVED" | yes |
//! | `Store(Score)` | a run finished, or a finished run found at launch | `resume::replace` with its `clear` text, then, only if that is confirmed, `save::store` with the entry placed; then [`Game::stored`] | yes: placed in memory on success; on failure the run file is rewritten with the finished runs kept (`RunFile::Write`) | yes |
//! | `Store(Layouts)` | the editor's SAVE, a slot deleted, a toast's UNDO (`app/editor.rs`) | `save::write` of the slot file (`layouts_file`), then [`Game::stored`] | yes: memory takes the new slots only on success, then a saved draft is cleared (`DraftFile(Discard)`); on failure a note, and the draft is kept | yes |
//! | `DraftFile(Write)` | the editor's draft changed, once a stroke has ended (`persist_draft`) | `resume::replace_at` at `layouts_file::DRAFT_FILES` | no | yes |
//! | `DraftFile(Restore)` | [`Game::saved_draft`], once the set-aside draft has been read (or is from a newer build) | rename `draft-loading.txt` back to `draft.txt` | no | yes |
//! | `DraftFile(Discard)` | the draft saved (from `stored`), DISCARD MAP once the editor is left, or a blank draft file at launch ([`Game::saved_draft`]); never over a newer build's draft | `resume::replace_at` with `""` | no | yes |
//! | `DraftFile(Abandon)` | [`Game::saved_draft`], for a draft this build cannot read | as `MatchFile(Abandon)`, at the draft's paths | no | yes |
//! | `RunFile(Write)` | a run started, then each time its log grows (a shot, a draft); [`Game::stored`], for a score that did not reach disk; launch or CONTINUE, dropping a line that will not replay | `resume::replace`: the run file becomes the text (`Game::run_file_text`: finished runs still to score, then the live run) | no | yes |
//! | `RunFile(Restore)` | [`Game::saved_run`], once the set-aside run has rebuilt | rename `run-loading.txt` back to `run.txt` | no | yes |
//! | `RunFile(Discard)` | a CONTINUE that failed to rebuild (a finished run's file is cleared by its `Store(Score)` instead) | `resume::discard`: delete every run file, or blank `run.txt` | no | yes |
//! | `RunFile(Abandon)` | [`Game::saved_run`], for a run this build must not replay | log why; `Unreadable` then as `Discard`; `Interrupted` deletes the set-aside copy and `run.txt.new`, never `run.txt` (`resume::drop_set_aside`) | no | yes |
//! | `MatchFile(Write)` | a player's versus match started, then each shot fired and each round dealt (`persist_match`) | `resume::replace_at` at `match_log::MATCH_FILES`: the match file becomes the line | no | yes |
//! | `MatchFile(Restore)` | [`Game::saved_match`], once the set-aside match has replayed | rename `match-loading.txt` back to `match.txt` | no | yes |
//! | `MatchFile(Discard)` | a kept match left: its result plate left after the win (REMATCH or TITLE), or quit; a saved match that no longer rebuilds | `resume::replace_at` with `""`: delete every match file, or blank `match.txt` | no | yes |
//! | `MatchFile(Abandon)` | [`Game::saved_match`], for a match this build must not replay | log why; `Unreadable` then as `Discard`; `Interrupted` deletes `match-loading.txt` and `match.txt.new`, never `match.txt` (`resume::drop_set_aside_at`) | no | yes |
//! | `CrashReport(Retire)` | the report copied, or a run started | rename the report to the old report | no | yes |
//! | `History(text)` | a kept run finished (`finish_run`), a kept match left once won (`drop_live_match`); never while the demo or a watch runs (`app/watch.rs`) | `save::write` of `user://history.txt` by way of `history.txt.new` | no | yes |
//! | `ScreenAwake(on)` | [`Game::take_effects`], whenever [`Game::screen_awake_wanted`] changes: live play or a versus match not paused, a menu-started demo, each until its quiet limit | `DisplayServer::screen_set_keep_on` | no | no |
//!
//! # Scoring at most once
//!
//! A finished run could reach the board twice: once when it ends, and again
//! when a later launch finds its log on disk and scores it (DESIGN §9.6).
//! Three rules, together, rule that out whatever moment the process dies,
//! and `app/disk_tests.rs` checks them by killing a simulated process after
//! every effect in turn, and with a run file that will not go.
//!
//! 1. **The run file goes first, confirmed.** A `Store(Score)` for a run
//!    kept on disk carries `clear`: the adapter makes the run file hold
//!    only that (`resume::replace`, `""` for no run) and writes the board
//!    only if it can confirm that no copy of the finished log is left. A
//!    crash in between loses the score; the other order would count it
//!    twice. A log that can be neither deleted nor blanked is not scored
//!    now: the write counts as failed, and a later launch that can clear it
//!    scores it.
//! 2. **Memory follows disk.** The entry joins `save.scores` only in
//!    [`Game::stored`], once the write succeeded. Placed earlier, the next
//!    settings write would carry it to disk while a kept run file (rule 3)
//!    scored it again.
//! 3. **A failed score keeps its log.** If the board cannot be written, the
//!    finished run waits in `RunFileState::unscored`, and its line stays at
//!    the top of the run file (`RunFile::Write`), carried along by every
//!    later write of it: a new run started this session does not overwrite
//!    it. The next score this session takes it onto the board with its own,
//!    or else the next launch does, through the same two rules.
//!
//! At launch the adapter moves the saved run aside before the core replays
//! it, and puts it back (`RunFile::Restore`) only once it rebuilt. A log that
//! crashes the sim is found still set aside at the next launch and abandoned
//! ([`SavedRun::Interrupted`]), so it costs one crash rather than one per
//! launch. When launch scores waiting runs or drops a bad line, it puts the
//! file back first and only then rewrites it, and the rewrite deletes
//! nothing until it has written its replacement: on a full disk every line
//! stays where it was.
//!
//! Over a save from a newer build, which this build never writes, no score
//! is tried at all: the finished runs stay in the run file, untouched but
//! for being written back, for a build that can record them.
//!
//! # Where to change things
//!
//! * **A sound.** A [`Sound`] variant, its [`Sound::file`] name and a `.wav`
//!   under `art/audio/` (its voice goes in `tools/gen_audio.py`; run it to
//!   write `art/audio/<name>.wav`), then a `self.play(Sound::…)` where it
//!   happens. The
//!   adapter indexes its players by `sound as usize`, so the variant goes in
//!   [`Sound::ALL`] in declaration order.
//! * **An action.** A variant of [`Action`]. If a `bind` line may name it:
//!   [`Action::BINDABLE`] (an array: bump its length), a match arm in
//!   [`Action::token`] (a save-format name, never to be renamed),
//!   [`input::default_binding`] and [`input::touch_label`]. Then
//!   read it with `action_pressed` or `action_repeated` in the mode's handler
//!   in `app/controls.rs`, placed with the take-once rule in mind: the first
//!   action asked takes a shared key.
//! * **A menu row.** `menu_row` in `render.rs` both draws the line and lays
//!   out its touch button; the matching `action_pressed` goes in that mode's
//!   arm of `handle_mode_input`.
//! * **A setting.** A field on [`Save`] (`Option<T>` while "not chosen yet"
//!   should follow a changing default, as `touch_aim` does), with its line in
//!   `save_format`. Change it in the Controls arm followed by
//!   `self.settings.touch()`, which debounces the write. If it is an
//!   `Option`, give it an `*_on()`-style reader that fills in the default.
//!   Add it to [`FrameState`] so a change redraws (`settings` there is a
//!   tuple, so its type changes too), and give it a `menu_row` in
//!   `render.rs`'s Controls layout.
//! * **A CLI flag.** A field and a parse line on [`DevArgs`], with a test in
//!   `cli.rs`. Act on it in [`Game::start_dev`], keeping any state in `Dev`,
//!   or in the node's `ready` if it needs the engine (`--salt-gallery`,
//!   `--salt-shot`).
//!
//! # Tests
//!
//! The core is tested the way the node drives it. `app/tests.rs` builds a
//! [`Game`], feeds it [`Intents`] frame by frame (a key down, a finger down
//! and moved, a pad button) and asserts on the mode, the run and the effects
//! returned. `render.rs` draws the result into a [`Canvas`](crate::raster::Canvas)
//! and pins it with snapshots. `app/disk_tests.rs` goes one step further: a
//! `FakeDisk` applies effects the way `game.rs` does, to files held in
//! memory, and kills the process after every operation in turn, then
//! launches again on what was left and checks that a run is scored at most
//! once, an offered run is the one played, and a crash report survives.

use salt_line_sim::ballistics::{End, Shot, Trace};
use salt_line_sim::dmath;
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{
    self, buffer_width, daily_seed, format_seed, parse_seed, Phase, RunState, Screen,
};
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_H, SCREEN_W_MAX, SCREEN_W_MIN};

use crate::cli::DevArgs;
use crate::durable::Debounce;
use crate::input::Typed;
use crate::input::{self, Action, Controls, Event, Intents, Key, Pad};
use crate::keypad::{self, Focus};
use crate::match_log;
use crate::presentation::{
    self, LEARN_ANGLE, LEARN_CRATER, LEARN_DRAFT, LEARN_FIRE, LEARN_POWER, LEARN_SHELLS, LEARN_WIND,
};
use crate::raster::View;
use crate::resume::{self, Found};
use crate::save_format::{ArcHint, Entry, Save, TouchAim};
use crate::touch::{Button, TouchPad};

/// Long Lens zooms the buffer out one step vertically. DESIGN §5.1: it changes
/// the internal buffer size, never the sprite scale. Buffer pixels, against
/// the usual `SCREEN_H` of 180; the width is left as it was.
pub const LONG_LENS_H: i32 = 216;

/// How long a cleared screen is held before the run moves on, in seconds.
/// The camera pans across the world during it.
pub const CLEAR_HOLD: f64 = 0.55;
/// Topple time before the rubble sprite replaces the standing one, in
/// seconds after the last shell lands.
pub const TOPPLE_TIME: f64 = 0.22;
/// Hold-to-repeat, shared by keys, pad buttons, the stick and touch buttons:
/// seconds held before repeats start; the first lands one `REPEAT_RATE`
/// later.
pub const REPEAT_DELAY: f64 = 0.28;
/// Seconds between repeats once they start (about 28 a second).
pub const REPEAT_RATE: f64 = 0.035;

/// How long a status line stays up, in seconds.
pub const STATUS_TIME: f64 = 2.4;
/// How long a status line about a lost saved run stays up, in seconds: long
/// enough to read on a title screen the player may not be looking at yet.
pub const STALE_STATUS_TIME: f64 = 6.0;

/// Touch aiming until the player picks one in Controls. Undecided: buttons are
/// the conservative choice while drag aiming is new.
pub const DEFAULT_TOUCH_AIM: TouchAim = TouchAim::Buttons;
/// The drag arc preview until the player picks one. Short keeps reading the
/// wind the player's job (DESIGN §2.5).
pub const DEFAULT_ARC_HINT: ArcHint = ArcHint::Short;
/// Seconds of flight the Short preview shows, so its length reads as power.
pub const SHORT_ARC_SECONDS: f64 = 0.3;
/// The last shot's arc stays on screen until the player turns it off.
pub const DEFAULT_LAST_ARC: bool = true;
/// Seconds of live play without any input after which the screen is no
/// longer held on (DESIGN §12.4): a phone put down mid-run or on a versus
/// plate sleeps on its own timeout after this. The next input holds it
/// again.
pub const AWAKE_QUIET_LIMIT: f64 = 4.0 * 60.0;
/// The same for a demo chosen from the menu, which is watched without
/// input by design, so it gets longer before it lets the screen sleep.
pub const DEMO_AWAKE_LIMIT: f64 = 15.0 * 60.0;

/// Where the player is. Playing and Paused have a run on screen; the rest are
/// menus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The title menu: daily run or CONTINUE, seed entry, controls, quit.
    Title,
    /// Controls and audio: volume, toggles, the bindings in use.
    Controls,
    /// Typing a seed, by keyboard, touch keypad or controller focus.
    SeedEntry,
    /// A run on screen and taking input. The only mode that steps the replay
    /// and is not held to `frame::IDLE_FPS`.
    Playing,
    /// A run on screen, frozen under the pause plate.
    Paused,
    /// The run's result, with its seed to copy or play again.
    GameOver,
    /// Before a local versus match: each player's aim help, then start.
    VersusSetup,
    /// A local versus match on screen, hot-seat: turns, the pass-the-phone
    /// plate, results and its own pause (`app/versus.rs`).
    Versus,
    /// The map editor (DESIGN §13.11, `app/editor.rs`): its start page,
    /// the editing screen, its menus, the slot list and test fire.
    Editor,
    /// The REPLAYS list (DESIGN §9.4, `app/watch.rs`): finished runs and
    /// matches, newest first, to watch again. A watch itself is `Playing`
    /// or `Versus` with [`Game::watching`] set.
    Replays,
}

/// A shot being replayed to the player. The sim has already resolved it.
///
/// The sim resolves a shot the instant it is fired (`RunState::fire`), and
/// the result is already in the run: craters, collapses, even a cleared
/// screen. What the player watches is a playback of the recorded paths, so
/// its speed, or a dropped frame, cannot change an outcome.
pub struct Anim {
    /// Every shell's recorded flight, primary first: one sample per sim step
    /// of `DT` seconds, in world coordinates.
    pub traces: Vec<Trace>,
    /// The screen as it looked before the shot, so craters appear on impact
    /// rather than the instant the trigger is pulled.
    pub before: Screen,
    /// Sim seconds elapsed in the replay.
    pub t: f64,
    /// The longest trace's flight time, in sim seconds: the replay is done
    /// once `t` reaches it.
    pub longest: f64,
    /// Indices into the screen's structures that this shot brought down.
    pub collapsed: Vec<usize>,
    /// Seconds since the last shell landed. Drives the topple, the shake and
    /// the hold before the replay ends.
    pub settled: f64,
    /// The landing sound has been asked for; played once per shot.
    pub impact_played: bool,
    /// The collapse sound has been asked for; played once per shot.
    pub collapse_played: bool,
}

impl Anim {
    /// Every shell has landed. The replay itself ends a little later, once
    /// `settled` has covered the topple.
    pub fn done(&self) -> bool {
        self.t >= self.longest
    }
}

/// The game's sounds, one player each. The adapter owns the players.
///
/// The core names a sound and never touches audio; `Effect::Play` carries
/// the name out. Asking for a sound already playing restarts it, since each
/// has one player.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    /// The trigger pulled.
    Fire,
    /// The shell leaving, alongside `Fire`.
    Flight,
    /// The primary shell landed in the ground.
    Terrain,
    /// The primary shell hit a structure.
    Structure,
    /// The primary shell bounced at least once before it ended.
    Ricochet,
    /// The primary shell's fuse burst it in the air.
    Airburst,
    /// A structure came down.
    Collapse,
    /// A draft appeared or an offer was taken.
    Draft,
    /// The first shot that leaves fewer than 10 shells, once a run.
    LowShell,
    /// A menu choice.
    Ui,
    /// A versus shell reached the other gun: a two-note ding.
    Hit,
    /// A versus round or match won: a short rising fanfare.
    Win,
    /// The looping title track.
    Title,
}

impl Sound {
    /// Every sound, in declaration order. The node creates one player per
    /// entry in this order and finds a sound's player at `sound as usize`,
    /// so the order here must match the enum's.
    pub const ALL: [Sound; 13] = [
        Self::Fire,
        Self::Flight,
        Self::Terrain,
        Self::Structure,
        Self::Ricochet,
        Self::Airburst,
        Self::Collapse,
        Self::Draft,
        Self::LowShell,
        Self::Ui,
        Self::Hit,
        Self::Win,
        Self::Title,
    ];

    /// The file under `res://art/audio`, without `.wav`.
    pub fn file(self) -> &'static str {
        match self {
            Self::Fire => "fire",
            Self::Flight => "flight",
            Self::Terrain => "terrain_impact",
            Self::Structure => "structure_impact",
            Self::Ricochet => "ricochet",
            Self::Airburst => "airburst",
            Self::Collapse => "collapse",
            Self::Draft => "draft",
            Self::LowShell => "low_shell",
            Self::Ui => "ui",
            Self::Hit => "versus_hit",
            Self::Win => "versus_win",
            Self::Title => "title_loop",
        }
    }
}

/// How the window fits the buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aspect {
    /// The buffer is the device's width and fills the window.
    Expand,
    /// The buffer is a resumed run's width and is letterboxed.
    Keep,
}

/// Why the save is being written, which decides what is written and what the
/// player is told.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Store {
    /// An onboarding step was learned. A failure is not worth a word.
    Learned,
    /// Settings changed.
    Settings,
    /// Finished runs go on the board: the save is written with `entries`
    /// placed, and they join the board in memory only once that write
    /// succeeds, so each reaches disk exactly once, here or not at all.
    /// Usually one run; more when earlier scores failed and their runs were
    /// still waiting in the run file (`RunFileState::unscored`).
    ///
    /// `clear`, if set, is what the run file must hold before the board may
    /// be written (`resume::replace`; `""` is no run): the finished logs go
    /// first. If that cannot be confirmed the save is not written and the
    /// score counts as failed. If the board does not reach disk, the
    /// finished runs stay in the run file (`RunFile::Write`), beside any run
    /// kept for CONTINUE, until a later score or launch gets them on the
    /// board. `announce` says "LAST RUN SCORED" on success: a run scored at
    /// launch rather than at its end. See "Scoring at most once" in the
    /// module docs.
    Score {
        entries: Vec<Entry>,
        clear: Option<String>,
        announce: bool,
    },
    /// The player's saved layouts (DESIGN §13.8): the slot file becomes
    /// `text` (`layouts_file::Slots::serialize`), not the save. Memory
    /// takes it only in [`Game::stored`], once it is on disk, and only then
    /// is a draft it saved cleared: a failed write loses nothing.
    Layouts { text: String, why: SlotWrite },
}

/// Why the slot file is being written, which decides what follows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SlotWrite {
    /// The draft saved into `slot`; `before` is the file as it was, for
    /// UNDO if a layout there was replaced.
    Save {
        slot: u8,
        replaced: bool,
        before: String,
        /// The draft's revision saved: only that is cleared once confirmed.
        rev: u64,
    },
    /// `slot` emptied; `before` is the file as it was, for UNDO.
    Delete { slot: u8, before: String },
    /// An UNDO putting the file back.
    Undo,
}

impl Store {
    /// What the run file must hold before this write may go ahead, if
    /// anything. See [`Store::Score`].
    pub fn clear(&self) -> Option<&str> {
        match self {
            Store::Score { clear, .. } => clear.as_deref(),
            Store::Learned | Store::Settings | Store::Layouts { .. } => None,
        }
    }
}

/// Something the engine must do for the core: a command, recorded as data
/// and carried out by the adapter in the order it was asked for. See the
/// module docs for the catalogue.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    /// Play this sound from the start.
    Play(Sound),
    /// Set every sound player's volume, in dB.
    Volume(f32),
    /// Start or stop the dedicated looping title player.
    TitleMusic(bool),
    /// Something visible changed: draw the next frame. See `frame::Redraw`.
    Redraw,
    /// Read the display's safe area again rather than wait for the next
    /// scheduled read: between runs the buffer may take a new width.
    RecheckDisplay,
    /// The buffer is `w` by `h`, with `letterbox` buffer pixels of black kept
    /// clear left and right. Resize the canvas, scale the window's content to
    /// the whole and offset the node past the left inset.
    Buffer {
        w: i32,
        h: i32,
        letterbox: (i32, i32),
    },
    /// How the window fits the buffer.
    Aspect(Aspect),
    /// Put this text on the system clipboard: a seed, or a crash report.
    Clipboard(String),
    /// Leave the game.
    Quit,
    /// Write the save as `Game::save_to_store` has it when the effect is
    /// applied (`save::store`), then report the outcome to `Game::stored`.
    Store(Store),
    /// Something to do with the run kept on disk for CONTINUE.
    RunFile(RunFile),
    /// Something to do with the versus match kept on disk for RESUME MATCH.
    MatchFile(MatchFile),
    /// Something to do with the map editor's draft kept on disk.
    DraftFile(DraftFile),
    /// Read the system clipboard and hand its text to [`Game::pasted`]
    /// (PASTE CODE in the editor).
    ReadClipboard,
    /// Something to do with last session's crash report.
    CrashReport(CrashReport),
    /// Hold the screen on (`true`), or let the operating system turn it
    /// off as it normally would (`false`). The project leaves
    /// `display/window/energy_saving/keep_screen_on` off, so the screen
    /// sleeps unless this holds it (DESIGN §12.4). Asked for only when
    /// [`Game::screen_awake_wanted`] changes; see [`Game::take_effects`].
    ScreenAwake(bool),
    /// Make the history file (`user://history.txt`, DESIGN §9) this text:
    /// written whole and atomically (`save::write`), a run or match added.
    History(String),
}

/// What to do with the saved run (DESIGN §9.6). Which files that means is the
/// adapter's business: `game.rs` keeps it in `user://run.txt`, written by way
/// of `run.txt.new`, and moves it aside to `run-loading.txt` while a launch
/// rebuilds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunFile {
    /// Make the saved run this text, replacing whatever was saved
    /// (`resume::replace`): the running run's line, after the lines of any
    /// finished runs whose score has not reached the board
    /// (`Game::run_file_text`).
    Write(String),
    /// The run set aside at launch rebuilt: put it back to be continued.
    Restore,
    /// Make sure no run is left to resume or score, the one set aside at
    /// launch included. If a file cannot be deleted, the run file is
    /// overwritten with nothing, which `resume::judge` reads as no run: a
    /// finished log is meant never to survive to be scored twice.
    Discard,
    /// Throw away a run this build cannot continue, saying why in the log:
    /// `Discard` for one it could not read, and for one the last launch
    /// died rebuilding only the copy left set aside and any temporary file,
    /// never a `run.txt` written since.
    Abandon(Abandoned),
}

/// What to do with the suspended versus match (DESIGN §11.6). `game.rs`
/// keeps it in `user://match.txt` by `resume`'s rules at `match_log`'s own
/// paths: written by way of `match.txt.new`, moved aside to
/// `match-loading.txt` while a launch rebuilds it. Simpler than the run
/// file: a match has no score to keep, so a finished or quit match is just
/// cleared.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MatchFile {
    /// Make the kept match this line (`resume::replace_at`).
    Write(String),
    /// The match set aside at launch rebuilt: put it back to be resumed.
    Restore,
    /// Leave no match to resume, the copy set aside at launch included.
    Discard,
    /// Throw away a match this build cannot resume, saying why in the log:
    /// as `Discard` for one it could not read or replay, and only the
    /// set-aside copy and any temporary file for one the last launch died
    /// rebuilding.
    Abandon(Abandoned),
}

/// What to do with the map editor's draft (DESIGN §13.11). `game.rs` keeps
/// it in `user://draft.txt` by `resume`'s rules at
/// `layouts_file::DRAFT_FILES`, as the match file is kept.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftFile {
    /// Make the draft this line (`resume::replace_at`).
    Write(String),
    /// The draft set aside at launch was read: put it back.
    Restore,
    /// Leave no draft: it was saved or discarded.
    Discard,
    /// Throw away a draft this build cannot read, saying why in the log.
    Abandon(Abandoned),
}

/// Why a saved run or match was thrown away at launch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Abandoned {
    /// The last launch died rebuilding it.
    Interrupted,
    /// It could not be read or replayed by this build.
    Unreadable(resume::Stale),
}

/// What to do with last session's crash report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrashReport {
    /// The player has copied it or moved on (started a run): stop offering
    /// it. It is kept as the old report, not deleted.
    Retire,
}

impl Effect {
    /// Whether the adapter still carries this out after a panic. Everything
    /// that writes, moves or deletes a file is dropped: see the module docs.
    /// What is left is what the error screen needs: sound, drawing, the
    /// clipboard, quitting.
    pub fn survives_panic(&self) -> bool {
        !self.touches_disk()
    }

    /// Whether this effect writes, moves or deletes a file. None may be
    /// asked for while the attract demo runs (DESIGN §12), and none is
    /// carried out after a panic.
    pub fn touches_disk(&self) -> bool {
        matches!(
            self,
            Effect::Store(_)
                | Effect::RunFile(_)
                | Effect::MatchFile(_)
                | Effect::DraftFile(_)
                | Effect::CrashReport(_)
                | Effect::History(_)
        )
    }
}

/// What the adapter found of a saved run at launch.
#[derive(Clone, Debug)]
pub enum SavedRun {
    /// The copy a launch sets aside was still there: the last launch died
    /// rebuilding a run.
    Interrupted,
    /// The saved run's text, already set aside by the adapter so a log that
    /// brings the sim down costs one crash, not one every launch. The core
    /// answers with `RunFile::Restore` once it has rebuilt it, or throws it
    /// away.
    Found(String),
}

/// Longest seed entry takes, in characters.
pub const SEED_MAX: usize = 20;

/// A fingerprint of what a frame shows, taken before and after each
/// [`Game::tick`]; any difference asks for a redraw. See `Game::frame_state`.
///
/// This is the backstop of the dirty-flag redraw (`frame::Redraw`): code
/// that changes something visible should still push `Effect::Redraw`, but a
/// change that forgets to is caught here as long as its field is listed.
/// Floats are compared by their bits, so a change of any size counts.
#[derive(PartialEq, Debug)]
pub struct FrameState {
    pub mode: Mode,
    aim: (i32, i32),
    cam_x: i32,
    buffer: (i32, i32, (i32, i32)),
    devices: (bool, bool),
    run: Option<(u32, Phase, i32, u32, u64, u64)>,
    seed: (usize, Focus),
    versus: (
        Option<versus::Fingerprint>,
        [bool; 2],
        bool,
        bool,
        crate::opponent::Level,
    ),
    settings: (
        u8,
        bool,
        bool,
        Option<bool>,
        Option<TouchAim>,
        Option<ArcHint>,
        u32,
    ),
    /// The attract demo running, and the IDLE DEMO setting.
    demo: (bool, Option<bool>),
    /// A watch (paused, speed, ended, entry), the REPLAYS list's chosen
    /// row, and how many records it lists.
    watch: (Option<watch::Fingerprint>, usize, usize),
    /// What the map editor shows (`editor_fingerprint`), and the ground
    /// the pre-match screen has picked.
    editor: (Option<editor::Fingerprint>, Option<String>),
    misc: (bool, bool, bool),
}

/// Everything the game knows that is not the engine's, grouped by concern.
///
/// The fields are the crate's: the node reads what it must show or report
/// (the mode, the run, the aim) and the renderer reads what it draws, but
/// only the core changes them, through the methods below. Tests set state
/// directly.
pub struct Game {
    pub(crate) mode: Mode,
    /// Days since the Unix epoch, as the adapter last read the clock. The
    /// sim never touches system time.
    pub(crate) today: u64,
    pub(crate) save: Save,
    /// Settings changed but not yet written. See `durable::Debounce`.
    pub(crate) settings: Debounce,
    /// Told the player this session that their save is from a newer build.
    pub(crate) future_noted: bool,
    /// What has been typed in seed entry, at most `SEED_MAX` characters.
    pub(crate) seed_text: String,
    /// The status line (`note`), shown while `status_time` is above zero.
    pub(crate) status: String,
    /// Seconds the status line has left on screen.
    pub(crate) status_time: f64,
    /// The F1 debug overlay is showing.
    pub(crate) debug: bool,

    pub(crate) display: Display,
    pub(crate) play: Play,
    pub(crate) input: InputState,
    pub(crate) run_file: RunFileState,
    pub(crate) dev: Dev,
    pub(crate) crash: CrashInfo,
    /// Local versus: the pre-match choices and the match on screen.
    pub(crate) versus: VersusState,
    /// The versus match found on disk at launch, already checked, that
    /// RESUME MATCH would rebuild (DESIGN §11.6). Taken when it is
    /// resumed or another match is started; the demo never touches it.
    pub(crate) saved_match: Option<match_log::SavedMatch>,
    /// The attract demo, while it runs (`app/demo.rs`). It holds whatever
    /// of the player's it set aside, untouched, until it ends.
    pub(crate) demo: Option<Demo>,
    /// A finished run or match being watched again (`app/watch.rs`). Like
    /// the demo, it holds what it set aside until it ends.
    pub(crate) watch: Option<Watch>,
    /// The history of finished runs and matches, and the REPLAYS list.
    pub(crate) history: HistoryState,
    /// The player's saved layouts, as last confirmed on disk (DESIGN
    /// §13.8). The demo never reads or writes them.
    pub(crate) layouts: LayoutsState,
    /// The map editor: its draft, page and tools (`app/editor.rs`).
    pub(crate) editor: EditorState,
    /// Seconds the title has gone without input, towards the idle demo
    /// (`demo::IDLE_DEMO_AFTER`). Zero anywhere but the title.
    pub(crate) title_idle: f64,
    /// The leg the next demo begins on: the rotation carries on across
    /// demos in a session rather than restarting at the first run.
    pub(crate) demo_next_leg: u64,
    /// An idle demo ran its `demo::IDLE_DEMO_LIMIT` out; the title waits
    /// for some input before it starts another by itself.
    pub(crate) idle_demo_spent: bool,
    /// The window or app is out of focus or paused (since `focus_lost` or
    /// `app_paused`, until `focus_gained`): the idle demo does not start,
    /// and nothing holds the screen on (`screen_awake_wanted`).
    ///
    /// The adapter calls `focus_gained` on APPLICATION_RESUMED as well as
    /// on focus in, so a resume clears this even if the window has not got
    /// focus back (Android split screen, say, with the other app focused).
    /// The run was auto-paused on the way out, so the worst case is a
    /// versus plate or a draft holding a screen that is still visible.
    pub(crate) unfocused: bool,
    /// Seconds without any input (a key, a touch, a pad button, a held key
    /// or stick) while the screen could be held: live play or a demo. Zero
    /// anywhere else, so entering a screen that holds starts it afresh.
    /// Past `AWAKE_QUIET_LIMIT` (`DEMO_AWAKE_LIMIT` in a menu demo) the
    /// screen is let go (`screen_awake_wanted`).
    pub(crate) input_quiet: f64,

    /// Effects asked for since they were last taken.
    fx: Vec<Effect>,
}

/// The window, the display and the buffer sized for them (DESIGN §5.1).
pub(crate) struct Display {
    /// Running on a phone or tablet.
    pub(crate) mobile: bool,
    /// Extra inset (from the sides, from top and bottom) for edge-anchored HUD
    /// and buttons, in buffer pixels, so nothing crowds a phone's rounded
    /// screen corners. Zero on desktop.
    pub(crate) corner_pad: (i32, i32),
    /// The window in screen pixels, and what a display cutout takes from its
    /// left and right edges, as the adapter last read them.
    pub(crate) window_size: (i32, i32),
    pub(crate) cutout: (i32, i32),
    /// The internal buffer's width in buffer pixels: `pick_width`'s 320 to
    /// 400, or a resumed run's recorded width.
    pub(crate) buffer_w: i32,
    /// The internal buffer's height in buffer pixels: 180, or `LONG_LENS_H`
    /// with Long Lens.
    pub(crate) buffer_h: i32,
    /// Buffer pixels of black kept clear on the left and right for a display
    /// cutout (a phone's camera hole in landscape). The game node is offset
    /// past the left one.
    pub(crate) letterbox: (i32, i32),
    /// The buffer is at a resumed run's width, not the device's, and the
    /// window letterboxes it (`set_run_width`).
    pub(crate) kept_width: bool,
    /// Whether the adapter was last told to hold the screen on
    /// (`Effect::ScreenAwake`): the change detector behind
    /// [`Game::take_effects`]. Starts off, as the project setting does.
    pub(crate) screen_awake: bool,
}

/// The run on screen and how it is being shown.
pub(crate) struct Play {
    /// The run, as the sim has it: already past any shot being replayed.
    /// `None` before the first run of the session.
    pub(crate) run: Option<RunState>,
    /// The last shot, while it is being played back.
    pub(crate) anim: Option<Anim>,
    /// Seconds left holding a cleared screen before the run moves on; zero
    /// when not holding. Counts down from `CLEAR_HOLD`.
    pub(crate) clear_hold: f64,
    /// Cosmetic sky clock. It is not saved or read by the simulation.
    pub(crate) sky_time: f64,
    /// Aim inputs, in the sim's tick space: integers, as the shot log records
    /// them (DESIGN §9.2). Angle ticks run from 0 to `dmath::MAX_ANGLE_TICKS`;
    /// the keys step them by the loadout's angle quantum.
    pub(crate) angle_ticks: i32,
    /// Power ticks, 0 to `POWER_TICKS_MAX`; the keys step them by 5.
    pub(crate) power_ticks: i32,
    /// Camera x is an integer, not a rounded float. DESIGN §5.1 wants it
    /// quantized to whole pixels every frame; keeping the only camera state in
    /// `i32` means there is no float position to forget to round.
    pub(crate) cam_x: i32,
    /// Where the camera is heading, in world pixels. Free to be fractional;
    /// `cam_x` is this rounded and clamped each frame.
    pub(crate) cam_target: f64,
    /// The run on screen is the daily (scored as such on the board).
    pub(crate) daily: bool,
    /// The seed of the run on screen or last played, for COPY SEED and SAME
    /// SEED AGAIN.
    pub(crate) last_seed: u64,
    /// The primary shell's path from the last shot. Drawn only while the sim
    /// still has a `last_apex`, so it goes when the screen does.
    pub(crate) last_arc: Vec<Vec2>,
    /// The low-shell warning has sounded this run.
    pub(crate) low_shell_warned: bool,
    /// Bumped whenever the drag preview must be worked out afresh for a
    /// reason its key cannot see (a new loadout, a new run).
    pub(crate) preview_epoch: u32,
}

impl Default for Play {
    /// Nothing on screen yet: the aim in the middle of its range.
    fn default() -> Self {
        Play {
            run: None,
            anim: None,
            clear_hold: 0.0,
            sky_time: 0.0,
            angle_ticks: 900,
            power_ticks: 600,
            cam_x: 0,
            cam_target: 0.0,
            daily: true,
            last_seed: 0,
            last_arc: Vec::new(),
            low_shell_warned: false,
            preview_epoch: 0,
        }
    }
}

/// What the player's devices are doing.
pub(crate) struct InputState {
    /// This frame's key and pad presses, and hold-to-repeat.
    pub(crate) controls: Controls,
    /// The controller's place on the seed keypad.
    pub(crate) seed_focus: Focus,
    /// The controller was the last device used: prompts name pad buttons.
    /// On at a pad press or stick push, off at a key press.
    pub(crate) controller_active: bool,
    /// Fingers, touch buttons held and the drag, fed by the frame's events.
    pub(crate) touch: TouchPad,
    /// Showing touch controls. On from the first touch, off again at the next
    /// key or button press, like `controller_active`.
    pub(crate) touch_active: bool,
    /// The platform's back gesture (Android), taken as the `back` action.
    /// Cleared at the end of every frame, so it counts for that frame only.
    pub(crate) back_requested: bool,
    /// The touch buttons on screen, in buffer pixels: last frame's layout,
    /// which this frame's touches are hit-tested against. See `set_buttons`.
    pub(crate) buttons: Vec<Button>,
    /// The aim (angle ticks, power ticks) a touch drag started from, put back
    /// if the pull is cancelled.
    pub(crate) drag_from: Option<(i32, i32)>,
    /// Seconds before touches act again after a versus screen changed (see
    /// `app/versus.rs`, "One touch, one screen"). Fingers that go down
    /// meanwhile are spent.
    pub(crate) touch_settle: f64,
}

/// The saved run (DESIGN §9.6): what is on disk and what CONTINUE offers.
pub(crate) struct RunFileState {
    /// The saved run CONTINUE would rebuild, already checked. See `resume`.
    pub(crate) resumable: Option<resume::Saved>,
    /// Log length last saved; `None` before the first save of this run.
    pub(crate) run_saved_len: Option<usize>,
    /// This run is kept as the saved run. Off for a `--salt-screen` jump,
    /// which its log cannot rebuild.
    pub(crate) run_persist: bool,
    /// This run was rebuilt by CONTINUE, for the crash report.
    pub(crate) resumed: bool,
    /// Which new-run action has been pressed once over a saved run and is
    /// waiting for the second press. See `confirm_discard`.
    pub(crate) discard_armed: Option<&'static str>,
    /// Finished runs whose score did not reach the board, oldest first. Their
    /// lines ride at the top of the run file, whatever run follows them, and
    /// they go on the board with the next score, or at the next launch.
    pub(crate) unscored: Vec<Unscored>,
}

/// A finished run waiting for the board. See `RunFileState::unscored`.
#[derive(Clone, Debug)]
pub(crate) struct Unscored {
    /// Its board entry.
    pub(crate) entry: Entry,
    /// Its line in the run file (`resume::Saved::to_line`).
    pub(crate) line: String,
}

/// The developer flags' state (`cli::DevArgs`).
#[derive(Default)]
pub(crate) struct Dev {
    /// `--salt-auto`: fire a solver candidate at the nearest standing target
    /// and take every draft. Lets a build be watched, or captured, without a
    /// hand on the keyboard.
    pub(crate) auto: bool,
    /// Seconds before the autopilot acts again, so each shot and pick can
    /// be seen.
    pub(crate) auto_cooldown: f64,
    /// `--salt-miss`: throw the first shot of each screen away, so craters,
    /// chalk marks and the multi-shot path get exercised too.
    pub(crate) auto_miss: bool,
    /// `--salt-hold-draft`: leave the draft on screen instead of taking an
    /// offer, so the cards can be looked at.
    pub(crate) auto_hold_draft: bool,
    /// `--salt-seed`, for the autopilot's runs.
    pub(crate) auto_seed: Option<u64>,
    /// `--salt-shot` has a capture still to take. Nobody is watching with
    /// focus, so focus loss does not pause.
    pub(crate) capturing: bool,
    /// Why `--salt-layout`'s code was not played, for the adapter to log
    /// (the core has no console of its own).
    pub(crate) layout_refused: Option<String>,
}

/// Crash reports: last session's, and whether this one has broken.
#[derive(Default)]
pub(crate) struct CrashInfo {
    /// Last session's crash report, if it left one.
    pub(crate) last_crash: Option<String>,
    /// The player has copied `last_crash` this session.
    pub(crate) crash_copied: bool,
    /// Showing the error screen. See `broken_tick`.
    pub(crate) broken: bool,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            mode: Mode::Title,
            today: 0,
            save: Save::default(),
            settings: Debounce::default(),
            future_noted: false,
            seed_text: String::new(),
            status: String::new(),
            status_time: 0.0,
            debug: false,
            display: Display {
                mobile: false,
                corner_pad: (0, 0),
                window_size: (0, 0),
                cutout: (0, 0),
                buffer_w: 320,
                buffer_h: SCREEN_H as i32,
                letterbox: (0, 0),
                kept_width: false,
                screen_awake: false,
            },
            play: Play::default(),
            input: InputState {
                controls: Controls::default(),
                seed_focus: Focus::default(),
                controller_active: false,
                touch: TouchPad::default(),
                touch_active: false,
                back_requested: false,
                buttons: Vec::new(),
                drag_from: None,
                touch_settle: 0.0,
            },
            run_file: RunFileState {
                resumable: None,
                run_saved_len: None,
                run_persist: false,
                resumed: false,
                discard_armed: None,
                unscored: Vec::new(),
            },
            dev: Dev::default(),
            crash: CrashInfo::default(),
            versus: VersusState::default(),
            saved_match: None,
            demo: None,
            watch: None,
            history: HistoryState::default(),
            layouts: LayoutsState::default(),
            editor: EditorState::default(),
            title_idle: 0.0,
            demo_next_leg: 0,
            idle_demo_spent: false,
            unfocused: false,
            input_quiet: 0.0,
            fx: Vec::new(),
        }
    }
}

// ------------------------------------------------------------- the frame

impl Game {
    /// A game at the title with `save` loaded. Phones and tablets start with
    /// touch controls; desktops show them only once the screen is touched.
    pub fn new(mobile: bool, save: Save) -> Game {
        let mut g = Game {
            save,
            ..Game::default()
        };
        g.display.mobile = mobile;
        g.input.touch_active = mobile;
        // Rounded screen corners (Pixel 10: 138 px radius at 1080 px tall):
        // 10 buffer pixels in from the side and 6 down keeps HUD text and
        // buttons comfortably inside the curve without raising the buttons
        // off the ground.
        g.display.corner_pad = if mobile { (10, 6) } else { (0, 0) };
        g
    }

    /// Last session's crash report, found at launch, for the title to offer.
    pub fn set_last_crash(&mut self, report: Option<String>) {
        self.crash.last_crash = report;
    }

    /// The `--salt-shot` capture has been taken: focus loss pauses again.
    pub fn capture_taken(&mut self) {
        self.dev.capturing = false;
    }

    /// The effects asked for so far, oldest first. Every method below that
    /// returns effects takes them itself; this is for the ones that do not
    /// (`start_run` from a test, say).
    ///
    /// Every effect leaves through here, so this is also where the screen's
    /// keep-on is settled: if [`Game::screen_awake_wanted`] differs from what
    /// the adapter was last told, one `Effect::ScreenAwake` goes last. No
    /// transition asks for it itself; whatever changed the mode, the demo or
    /// the pause, the state is read afresh, and only a change is sent. That
    /// covers the notification paths (`focus_lost`, `app_paused`) as well
    /// as the frame.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        let awake = self.screen_awake_wanted();
        if awake != self.display.screen_awake {
            self.display.screen_awake = awake;
            self.fx.push(Effect::ScreenAwake(awake));
        }
        std::mem::take(&mut self.fx)
    }

    /// Whether the screen should be held on now (DESIGN §12.4): only while
    /// somebody is watching the phone, and not for ever if nobody touches it.
    ///
    /// * **Live play** ([`Game::live_play`]: `Playing`, the draft included,
    ///   or a versus match not paused, its pass-the-phone plate and results
    ///   too), until [`AWAKE_QUIET_LIMIT`] passes without input.
    /// * **A demo chosen from the menu**, to be watched, until
    ///   [`DEMO_AWAKE_LIMIT`]. A demo that started by itself may have nobody
    ///   watching, so it lets the screen sleep.
    ///
    /// Everything else lets it sleep: the title and the menus, a paused run
    /// or match, the run's result, the error screen, and any mode while the
    /// window or app is out of focus (the platform ignores a backgrounded
    /// app's hold anyway; this keeps desktop from inhibiting the
    /// screensaver behind another window). The quiet limits do not apply to
    /// the dev autopilot (`--salt-auto`, the profiling build) or a pending
    /// `--salt-shot` capture, which nobody touches by design.
    pub fn screen_awake_wanted(&self) -> bool {
        if self.crash.broken || self.unfocused {
            return false;
        }
        let exempt = self.dev.auto || self.dev.capturing;
        // A watch is chosen by someone holding the phone, and watched
        // without input by design, as a demo from the menu is; paused, it
        // lets the screen sleep as a paused run does.
        if let Some(w) = &self.watch {
            return !w.paused && (exempt || self.input_quiet < DEMO_AWAKE_LIMIT);
        }
        if let Some(demo) = &self.demo {
            return demo.started == DemoStarted::Menu
                && (exempt || self.input_quiet < DEMO_AWAKE_LIMIT);
        }
        self.live_play() && (exempt || self.input_quiet < AWAKE_QUIET_LIMIT)
    }

    /// Somebody is playing: [`Game::live_play`], but not the demo or a
    /// watch, which are shown rather than played, and not the error screen.
    /// What asks for desktop GameMode (the adapter's `is_active_play`), so
    /// a watch, paused or not, lets it go.
    pub fn active_play(&self) -> bool {
        !self.spectating() && !self.crash.broken && self.live_play()
    }

    /// A run or a versus match on screen and running: `Playing`, or
    /// `Versus` not paused. What holds the screen on and asks for desktop
    /// GameMode (the adapter's `is_active_play`).
    pub fn live_play(&self) -> bool {
        match self.mode {
            Mode::Playing => true,
            Mode::Versus => self.versus.live.as_ref().is_some_and(|vs| !vs.paused),
            Mode::Title
            | Mode::Controls
            | Mode::SeedEntry
            | Mode::Paused
            | Mode::GameOver
            | Mode::VersusSetup
            | Mode::Editor
            | Mode::Replays => false,
        }
    }

    /// Count `input_quiet`: up while the screen could be held and nothing
    /// came in this frame, back to zero on any input or outside live play
    /// and the demo. Runs at the end of the frame, once the mode has
    /// settled, so a screen entered this frame starts from zero.
    fn watch_input_quiet(&mut self, delta: f64, input: bool) {
        if input || !(self.live_play() || self.spectating()) {
            self.input_quiet = 0.0;
        } else {
            self.input_quiet += delta;
        }
    }

    /// One frame: the events since the last, then timers, the display,
    /// input, the replay, persistence.
    ///
    /// `delta` is the engine's, uncapped. Animation and timers never jump
    /// more than a tenth of a second, however long the app sat in the
    /// background (`resume::frame_delta`).
    pub fn tick(&mut self, delta: f64, intents: &Intents) -> Vec<Effect> {
        self.tick_with(delta, intents, |_| {})
    }

    /// `tick`, calling `before_frame` once the events are in and before
    /// anything else: the node keeps the crash report's picture of the game
    /// current there, so a panic inside `fire` still reports the aim it
    /// fired.
    pub fn tick_with(
        &mut self,
        delta: f64,
        intents: &Intents,
        before_frame: impl FnOnce(&Game),
    ) -> Vec<Effect> {
        self.today = intents.today;
        self.events(&intents.events);
        before_frame(self);
        let delta = resume::frame_delta(delta);
        self.status_time = (self.status_time - delta).max(0.0);
        self.poll_display(intents.window, intents.cutout);
        // Dirty-flag backstop: fingerprint what the frame shows now, and
        // compare once input and the step have run.
        let before = self.frame_state();
        let title_music_before = self.title_music_playing();
        let demo_before = self.in_demo();
        let watch_before = self.watching();
        self.begin_input_frame(delta, intents);
        let versus_before = self.versus_screen();
        let editor_before = self.editor_screen();
        if demo_before {
            // The demo reads no action: any input ends it, and is spent.
            if self.demo_interrupted(intents) {
                self.stop_demo();
            }
        } else if watch_before {
            // A watch reads only its own controls (`app/watch.rs`).
            self.handle_watch_input();
        } else {
            self.handle_mode_input(delta);
        }
        // The back gesture is a one-frame press, like a key's.
        self.input.back_requested = false;
        if self.in_demo() {
            self.demo_step(delta);
        } else if self.dev.auto {
            if matches!(self.mode, Mode::Versus | Mode::VersusSetup) {
                self.versus_autopilot(delta);
            } else {
                self.autopilot(delta);
            }
        }
        let input = self.any_input(intents);
        self.watch_title_idle(delta, input);
        let watching = self.watching();
        if watching {
            // The watch steps the run or match itself, at its own speed.
            self.watch_frame(delta);
        } else {
            self.step(delta);
        }
        let ai_before = self.versus_screen();
        let ai_moved = self.step_versus_ai(delta);
        let ai_after = self.versus_screen();
        if !watching {
            self.step_versus(delta);
        }
        self.step_editor(delta);
        // The demo's own plates change nobody's screen: any tap on them is
        // a request to stop (`demo_interrupted`), never to be spent. Nor
        // does the AI moving its own turn on, when that is the only change
        // this frame: a tap on II just then is a pause, not a leftover.
        let versus_now = self.versus_screen();
        let only_ai = ai_moved && versus_before == ai_before && ai_after == versus_now;
        if ((versus_now != versus_before && !only_ai) || self.editor_screen() != editor_before)
            && !self.spectating()
            && !watch_before
        {
            self.settle_touch();
        }
        self.persist_run();
        self.persist_match();
        self.persist_draft();
        self.tick_settings(delta);
        self.watch_input_quiet(delta, input);
        if (demo_before && self.in_demo()) || (watch_before && self.watching()) {
            // Every write is stopped where it would be asked for (DESIGN
            // §12, §9.4); this is the backstop, and the tests fail if it is
            // hit.
            debug_assert!(
                !self.fx.iter().any(Effect::touches_disk),
                "the demo or a watch asked for a disk write: {:?}",
                self.fx
            );
            self.fx.retain(|e| !e.touches_disk());
        }
        let after = self.frame_state();
        let title_music_after = self.title_music_playing();
        if title_music_after != title_music_before {
            self.fx.push(Effect::TitleMusic(title_music_after));
        }
        if after != before {
            self.fx.push(Effect::Redraw);
            // Between runs the buffer may take a new width: look at the
            // display afresh rather than wait for the next scheduled read.
            if after.mode != before.mode && matches!(after.mode, Mode::Title | Mode::GameOver) {
                self.fx.push(Effect::RecheckDisplay);
            }
        }
        self.take_effects()
    }

    /// A frame of the error screen. A panic has happened somewhere in the
    /// Rust side. gdext caught it, but the state it left is not to be
    /// trusted, so the game stops here: no sim steps, no saves (`save::store`
    /// refuses too), just the report and a way out.
    ///
    /// `delta` is uncapped: wall clock still runs, for `--salt-shot-at`
    /// captures of this screen. `report` is the finished crash report, or
    /// `None` while the hook is still writing it; it is only asked for when
    /// the player copies it.
    pub fn broken_tick(
        &mut self,
        delta: f64,
        intents: &Intents,
        report: impl FnOnce() -> Option<String>,
    ) -> Vec<Effect> {
        if !self.crash.broken {
            // Whatever the game last said no longer applies, and whatever the
            // crashed frame asked for is not to be done.
            self.crash.broken = true;
            self.status_time = 0.0;
            self.fx.clear();
            // Whatever the screen was last told may not have been carried
            // out: a panic inside the adapter's `apply` skips the rest of
            // its list, a `ScreenAwake(false)` included. Assume it is held,
            // so `take_effects` below lets go for certain.
            self.display.screen_awake = true;
        }
        self.today = intents.today;
        self.events(&intents.events);
        self.status_time = (self.status_time - delta).max(0.0);
        self.begin_input_frame(delta, intents);
        if self.action_pressed(Action::Copy) {
            // Only a finished report: a panic off the main thread can stop the
            // game while the hook is still writing, and half a report copied
            // looks like a whole one.
            match report() {
                Some(text) => {
                    self.fx.push(Effect::Clipboard(text));
                    self.note("REPORT COPIED".into());
                }
                None => self.note("REPORT NOT READY - TRY AGAIN".into()),
            }
        } else if self.action_pressed(Action::Back) {
            self.fx.push(Effect::Quit);
        }
        self.input.back_requested = false;
        self.take_effects()
    }

    /// Something on screen is moving, so every frame draws. The adapter adds
    /// what only it can see: a stick or key held. Feeds `frame::Redraw::due`,
    /// the other half of the dirty-flag redraw.
    pub fn busy(&self) -> bool {
        self.play.anim.is_some()
            || self.versus.live.as_ref().is_some_and(|v| {
                // The AI's barrel swings to its aim; its plate and the
                // player's are still, and redraw when they move on.
                (v.anim.is_some() || v.easing() || (v.ai_turn() && v.stage == VersusStage::Aiming))
                    && !v.paused
            })
            || self.editor.testing_in_flight()
            || self.play.clear_hold > 0.0
            || self.status_time > 0.0
            || self.dev.auto
            || self.demo.is_some()
            || self.watch.as_ref().is_some_and(|w| !w.paused && !w.ended)
            || self.input.touch.active()
    }

    /// Menus, overlays and the error screen run at the idle frame rate; so
    /// does the editor, but for a test shot in flight, and a watch paused
    /// or at its end (`app/watch.rs`): nothing on it moves.
    pub fn idle(&self) -> bool {
        self.watch.as_ref().is_some_and(|w| w.paused || w.ended)
            || self.crash.broken
            || !(matches!(self.mode, Mode::Playing | Mode::Versus)
                || (self.mode == Mode::Editor && self.editor.testing_in_flight()))
    }

    /// Every sound player's volume, in dB: the save's percentage as an
    /// amplitude ratio (`20·log10(v/100)`, so 50% is about -6 dB), and -80 dB,
    /// the bottom of Godot's volume range, for muted or zero.
    ///
    /// The attract demo plays silent (`demo::DEMO_MUTED`) by asking for
    /// -80 dB here while it runs; the save's volume and mute are left alone.
    pub fn volume_db(&self) -> f32 {
        let demo_muted = self.demo.is_some() && demo::DEMO_MUTED;
        if demo_muted || self.save.muted || self.save.volume == 0 {
            -80.0
        } else {
            20.0 * (self.save.volume as f32 / 100.0).log10()
        }
    }

    /// Whether the adapter should currently be playing the title loop.
    pub fn title_music_playing(&self) -> bool {
        self.mode == Mode::Title && self.save.title_music
    }
}

// ------------------------------------------------------ environment events

impl Game {
    /// The touch buttons the frame on screen drew, which the next touches are
    /// tested against. The node hands them over after every frame it draws.
    ///
    /// Immediate-mode UI: there is no widget tree, only the rectangles the
    /// renderer declared while drawing. An undrawn frame keeps the last
    /// list, which is still what the player sees.
    pub fn set_buttons(&mut self, buttons: &[Button]) {
        self.input.buttons.clear();
        self.input.buttons.extend_from_slice(buttons);
    }

    /// Take the events in the order they happened, as they would have been
    /// taken the moment they arrived: a touch goes to the touch pad, a key
    /// press waits for the frame's actions, and a key typed in seed entry is
    /// typed and not pressed as well, so an M in a seed does not also mute,
    /// nor the Enter that starts the run fire its first shot.
    fn events(&mut self, events: &[Event]) {
        for event in events {
            match *event {
                Event::Key { key, typed } => {
                    if let Some(key) = key {
                        self.input.controls.queue(key);
                    }
                    if let (Some(typed), Mode::SeedEntry) = (typed, self.mode) {
                        self.typed(typed);
                        if let Some(key) = key {
                            self.input.controls.swallow(key);
                        }
                    } else if let Some(typed) = typed.filter(|_| self.editor_typing()) {
                        self.editor_typed(typed);
                        if let Some(key) = key {
                            self.input.controls.swallow(key);
                        }
                    }
                }
                Event::FingerDown { index, x, y } => {
                    self.input.touch_active = true;
                    if self.input.touch_settle > 0.0 {
                        self.input.touch.finger_down_spent(index);
                    } else {
                        self.input
                            .touch
                            .finger_down(index, x, y, &self.input.buttons);
                    }
                }
                Event::FingerMoved { index, x, y } => self.input.touch.finger_moved(index, x, y),
                Event::FingerUp { index } => {
                    self.input.touch_active = true;
                    self.input.touch.finger_up(index);
                }
                Event::Back => self.input.back_requested = true,
                // Only the demo and the idle count look at triggers.
                Event::Trigger => {}
            }
        }
    }

    /// A key typed in seed entry.
    fn typed(&mut self, key: Typed) {
        match key {
            Typed::Backspace => {
                self.seed_text.pop();
            }
            Typed::Enter => self.start_seeded_run(),
            Typed::Escape => self.leave_seed_entry(),
            Typed::Char(c) => {
                if self.seed_text.chars().count() < SEED_MAX {
                    self.seed_text.push(c);
                }
            }
        }
    }

    /// Window or application focus lost: every finger is up, a live screen
    /// pauses, and pending settings go to disk. `pending` are the events
    /// that arrived since the last frame, which happened first.
    ///
    /// An idle-started demo ends here (`demo_unwatched`), and the idle demo
    /// waits for focus to come back before it can start again.
    pub fn focus_lost(&mut self, pending: &[Event]) -> Vec<Effect> {
        self.events(pending);
        self.unfocused = true;
        self.demo_unwatched();
        self.input.touch.release_all();
        self.auto_pause();
        self.flush_settings();
        self.flush_draft();
        self.fx.push(Effect::Redraw);
        self.take_effects()
    }

    /// The platform paused the app. Pending settings go to disk before the
    /// app can be killed: a phone may never resume a paused app. Counted as
    /// out of focus until it resumes (`focus_gained`), so the screen is let
    /// go even on a screen a pause does not freeze (a versus plate, a
    /// draft).
    pub fn app_paused(&mut self, pending: &[Event]) -> Vec<Effect> {
        self.events(pending);
        self.unfocused = true;
        self.demo_unwatched();
        self.auto_pause();
        self.flush_settings();
        self.flush_draft();
        self.take_effects()
    }

    /// Focus is back: the idle demo may start again once the title has been
    /// quiet for long enough, counted from now.
    pub fn focus_gained(&mut self) {
        self.unfocused = false;
        self.title_idle = 0.0;
    }

    /// The window is closing or the node leaving the tree: a close is final.
    pub fn closing(&mut self, pending: &[Event]) -> Vec<Effect> {
        self.events(pending);
        self.flush_settings();
        self.flush_draft();
        self.take_effects()
    }

    /// The save as an `Effect::Store` for `why` writes it: as it is, with a
    /// score's entry placed. Placed in a copy only: memory takes the entry in
    /// `stored`, once the write has succeeded.
    pub fn save_to_store(&self, why: &Store) -> std::borrow::Cow<'_, Save> {
        match why {
            Store::Score { entries, .. } => {
                let mut save = self.save.clone();
                for entry in entries {
                    save.place(*entry);
                }
                std::borrow::Cow::Owned(save)
            }
            Store::Learned | Store::Settings | Store::Layouts { .. } => {
                std::borrow::Cow::Borrowed(&self.save)
            }
        }
    }

    /// A write the core asked for with `Effect::Store` reached disk, or did
    /// not. The adapter applies what this returns before anything after the
    /// `Store` in the list it came from.
    pub fn stored(&mut self, why: Store, ok: bool) -> Vec<Effect> {
        // The adapter applies what this returns in the middle of the list it
        // was working through, after the `ScreenAwake` change (if any) that
        // list already carried. A screen change sent from here would go out
        // ahead of that one and be overwritten by it, so nothing here may
        // change what the screen wants.
        let awake = self.screen_awake_wanted();
        match why {
            Store::Learned => {}
            Store::Settings => {
                if !ok {
                    self.note_not_saved("SETTINGS NOT SAVED");
                }
            }
            Store::Layouts { text, why } => self.layouts_stored(text, why, ok),
            Store::Score {
                entries,
                clear,
                announce,
            } => {
                if ok {
                    // A score that cleared the run file is the waiting runs,
                    // oldest first (`score_unscored`); one that did not (a
                    // `--salt-screen` run) is none of them, however alike.
                    if clear.is_some() {
                        let n = entries.len().min(self.run_file.unscored.len());
                        self.run_file.unscored.drain(..n);
                    }
                    for entry in entries {
                        self.save.place(entry);
                    }
                    if announce {
                        // Beside, not over, a note launch left about a line
                        // it had to throw away.
                        let text = if self.status_time > 0.0 && !self.status.is_empty() {
                            format!("LAST RUN SCORED - {}", self.status)
                        } else {
                            "LAST RUN SCORED".into()
                        };
                        let time = self.status_time;
                        self.note(text);
                        self.status_time = self.status_time.max(time);
                    }
                } else {
                    self.keep_unscored();
                }
            }
        }
        debug_assert_eq!(
            self.screen_awake_wanted(),
            awake,
            "stored() changed the wanted screen state"
        );
        self.take_effects()
    }
}

// ------------------------------------------------------------------ launch

impl Game {
    /// Size the buffer for the display at launch.
    pub fn fit_display(&mut self, window: (i32, i32), cutout: (i32, i32)) -> Vec<Effect> {
        self.display.window_size = window;
        self.display.cutout = cutout;
        self.display.buffer_w = self.pick_width();
        self.apply_buffer_size();
        self.take_effects()
    }

    /// Act on the developer flags: the autopilot, a run started on a seed and
    /// screen, or CONTINUE.
    pub fn start_dev(&mut self, args: &DevArgs, today: u64) -> Vec<Effect> {
        self.today = today;
        self.dev.capturing = args.shot.is_some();
        self.dev.auto = args.auto;
        self.dev.auto_miss = args.miss;
        self.dev.auto_hold_draft = args.hold_draft;
        self.dev.auto_seed = args.seed;
        if args.demo {
            self.start_demo(DemoStarted::Menu);
        } else if args.editor {
            self.open_editor_from_dev(args.layout.as_deref(), args.editor_page.as_deref());
        } else if args.setup {
            self.open_setup_from_dev(args.layout.as_deref());
        } else if let Some(code) = args.layout.as_deref() {
            match crate::custom::read(code) {
                Ok(layout) if layout.is_duel() => self.start_layout_match(&layout),
                // A one-gun layout is a challenge, which comes later (§13.9).
                Ok(_) => self.note("LAYOUT HAS ONE GUN: NOT A DUEL".into()),
                Err(why) => {
                    self.dev.layout_refused = Some(why);
                    self.note("LAYOUT CODE NOT PLAYABLE".into());
                }
            }
        } else if args.versus {
            self.start_versus();
        } else if let Some(seed) = args.seed {
            self.start_run(seed, false);
            if let Some(n) = args.screen {
                self.jump_to(n);
            }
        } else if args.continue_run {
            self.continue_run();
        }
        self.take_effects()
    }

    /// At launch: offer the saved run, score one that had already ended, or
    /// throw away one this build must not replay.
    pub fn saved_run(&mut self, found: SavedRun) -> Vec<Effect> {
        let text = match found {
            SavedRun::Interrupted => {
                // The last launch died rebuilding it.
                self.fx
                    .push(Effect::RunFile(RunFile::Abandon(Abandoned::Interrupted)));
                self.note(resume::Stale::Diverged.note().into());
                self.status_time = STALE_STATUS_TIME;
                return self.take_effects();
            }
            SavedRun::Found(text) => text,
        };
        // One line a run: finished runs whose score never reached the board,
        // then the run to continue, if there is one.
        let mut stale = None;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match resume::judge(Some(line)) {
                Found::Nothing => {}
                Found::Continue(saved) => self.run_file.resumable = Some(saved),
                Found::Finished(saved, run) => {
                    let (screens, shells, shots) = run.score();
                    let entry = Entry {
                        seed: saved.seed,
                        screens,
                        shells,
                        shots,
                        daily: saved.daily,
                    };
                    self.run_file.unscored.push(Unscored {
                        entry,
                        line: saved.to_line(),
                    });
                }
                Found::Discard(why) => {
                    stale.get_or_insert(why);
                }
            }
        }
        let rewrite = !self.run_file.unscored.is_empty()
            || (stale.is_some() && self.run_file.resumable.is_some());
        if rewrite {
            // Put the file back first. A rename needs no room on the disk, so
            // if the rewrite below cannot be written (a full disk) the whole
            // file is still in place, finished runs, live run and all, and
            // the clear is not confirmed.
            self.fx.push(Effect::RunFile(RunFile::Restore));
        }
        if !self.run_file.unscored.is_empty() {
            // Their lines gone before the scores go on the board, as in
            // `finish_run`; kept for a launch that can record them, if this
            // one cannot. The clear rewrites the file without anything
            // stale in it.
            self.score_unscored(true);
        } else if rewrite {
            self.fx
                .push(Effect::RunFile(RunFile::Write(self.run_file_text())));
        } else if let Some(why) = stale {
            self.fx
                .push(Effect::RunFile(RunFile::Abandon(Abandoned::Unreadable(
                    why,
                ))));
        } else {
            self.fx.push(Effect::RunFile(RunFile::Restore));
        }
        if let Some(why) = stale {
            self.note(why.note().into());
            self.status_time = STALE_STATUS_TIME;
        }
        self.take_effects()
    }

    /// At launch, after [`Game::saved_run`]: offer the suspended versus
    /// match, or throw away one this build must not replay (DESIGN §11.6).
    /// The adapter has set the file aside, as for the run; it goes back
    /// (`MatchFile::Restore`) only once its log has replayed. A match whose
    /// last shot won it is offered too, and resumes on its result plate.
    pub fn saved_match(&mut self, found: SavedRun) -> Vec<Effect> {
        let text = match found {
            SavedRun::Interrupted => {
                self.fx.push(Effect::MatchFile(MatchFile::Abandon(
                    Abandoned::Interrupted,
                )));
                self.note_at_launch(match_log::note(resume::Stale::Diverged));
                return self.take_effects();
            }
            SavedRun::Found(text) => text,
        };
        match match_log::judge(Some(&text)) {
            // The full rebuild RESUME MATCH will do (paths, arcs, the ease),
            // here, while the file is still set aside: one that brings the
            // game down costs this launch only, never every tap of the row.
            match_log::Found::Resume(saved) if VersusPlay::rebuild(&saved).is_some() => {
                self.saved_match = Some(saved);
                self.fx.push(Effect::MatchFile(MatchFile::Restore));
            }
            match_log::Found::Resume(_) => {
                self.fx.push(Effect::MatchFile(MatchFile::Abandon(
                    Abandoned::Unreadable(resume::Stale::Diverged),
                )));
                self.note_at_launch(match_log::note(resume::Stale::Diverged));
            }
            match_log::Found::Nothing => {
                self.fx.push(Effect::MatchFile(MatchFile::Discard));
            }
            match_log::Found::Discard(why) => {
                self.fx.push(Effect::MatchFile(MatchFile::Abandon(
                    Abandoned::Unreadable(why),
                )));
                self.note_at_launch(match_log::note(why));
            }
        }
        self.take_effects()
    }

    /// A note launch leaves on the title for a while: beside, not over, one
    /// already there (a saved run and a saved match may both be lost).
    fn note_at_launch(&mut self, text: &str) {
        let text = if self.status_time > 0.0 && !self.status.is_empty() {
            format!("{} - {text}", self.status)
        } else {
            text.to_string()
        };
        self.note(text);
        self.status_time = STALE_STATUS_TIME;
    }

    /// The runs `--salt-gallery` draws: one screen per terrain template, and
    /// a damaged copy of each, named for the file they go to.
    ///
    /// This is the art-review and tuning tool. Change a terrain constant, run
    /// the gallery, look at seven pictures. Beats playing until the template
    /// you changed happens to come up.
    pub fn gallery(width: usize) -> Vec<(String, Option<RunState>)> {
        // Airburst is held so `Overhang` is on the table at all (DESIGN §2.4).
        let mut lo = Loadout::new();
        lo.take(Modifier::Airburst);
        let mut run = RunState::new(0, width);
        run.loadout = lo.clone();

        let mut out = Vec::new();
        for t in salt_line_sim::terrain::Template::ALL {
            let name = t.name().to_lowercase();
            let index = t.min_band().max(1) * salt_line_sim::BAND_LEN;
            let found = (0..6000u64)
                .map(|seed| run::generate_screen(seed, index, width, &lo))
                .find(|sc| sc.template == t);
            let Some(screen) = found else {
                // Named, so the adapter can say which is missing.
                out.push((t.name().to_string(), None));
                continue;
            };

            // Clean.
            run.screen = screen.clone();
            run.screen_index = index;
            out.push((format!("template_{name}"), Some(run.clone())));

            // Damaged: undermine the first target's footprint and lob a couple
            // of craters short of it, so craters, rims, lean and rubble all
            // appear in one picture.
            let mut damaged = screen;
            if let Some(st) = damaged.structures.first().cloned() {
                for _ in 0..6 {
                    for i in st.x0..st.x0 + st.footprint() {
                        damaged.field.crater(i as f64, 5.0);
                    }
                }
                damaged.field.crater(st.centre_x() - 34.0, 9.0);
                damaged.field.crater(st.centre_x() - 52.0, 6.0);
                damaged.resolve_structures();
            }
            run.screen = damaged;
            out.push((format!("damaged_{name}"), Some(run.clone())));
        }
        out
    }

    /// Show one of `gallery`'s runs, as a frame of play with nothing else on
    /// screen.
    pub fn show_gallery_run(&mut self, run: RunState) {
        self.play.run = Some(run);
        self.mode = Mode::Playing;
        self.status_time = 0.0;
    }
}

// ----------------------------------------------------------------- queries

impl Game {
    /// Internal buffer width. DESIGN §5.1: fix the height at 180, let the
    /// width float 320..400 by device aspect, and generate terrain to fill it.
    pub(crate) fn pick_width(&self) -> i32 {
        let size = self.display.window_size;
        let (l, r) = self.display.cutout;
        let usable = size.0 - l - r;
        let aspect = if size.1 > 0 {
            usable as f64 / size.1 as f64
        } else {
            16.0 / 9.0
        };
        let want = (SCREEN_H as f64 * aspect).round() as usize;
        buffer_width(want.clamp(SCREEN_W_MIN, SCREEN_W_MAX)) as i32
    }

    /// The cutout in buffer pixels, rounded up so the playfield never reaches
    /// under it.
    pub(crate) fn letterbox_now(&self) -> (i32, i32) {
        let (l, r) = self.display.cutout;
        let win = self.display.window_size;
        if win.1 <= 0 || self.display.buffer_h <= 0 || self.display.buffer_w <= 0 {
            return (0, 0);
        }
        // Buffer pixels per screen pixel: fitted by height, unless the buffer
        // (with the insets beside it) is too wide for the window, as a resumed
        // run from a wider device is, and is fitted by width instead.
        let usable = (win.0 - l - r).max(1) as f64;
        let scale = (win.1 as f64 / self.display.buffer_h as f64)
            .min(usable / self.display.buffer_w as f64);
        (
            (l as f64 / scale).ceil() as i32,
            (r as f64 / scale).ceil() as i32,
        )
    }

    /// The run holds Long Lens, so the buffer is `LONG_LENS_H` tall. Never
    /// in versus, which has no loadout to draft: a run set aside for
    /// CONTINUE keeps its lens for when it comes back.
    pub(crate) fn long_lens(&self) -> bool {
        if matches!(self.mode, Mode::VersusSetup | Mode::Versus | Mode::Editor) {
            return false;
        }
        self.play
            .run
            .as_ref()
            .map(|r| r.loadout.has_flag(flags::LONG_LENS))
            .unwrap_or(false)
    }

    /// The run's world width in pixels, or the buffer's with no run. Wider
    /// than the buffer, the camera pans (`step`).
    pub(crate) fn world_width(&self) -> i32 {
        self.play
            .run
            .as_ref()
            .map(|r| r.width as i32)
            .unwrap_or(self.display.buffer_w)
    }

    /// The lead shell's position in the replay, if one is in the air.
    pub(crate) fn shell_head(&self) -> Option<Vec2> {
        let anim = self.play.anim.as_ref()?;
        let primary = anim.traces.first()?;
        let i = (anim.t / DT).floor().max(0.0) as usize;
        primary
            .path
            .get(i.min(primary.path.len().saturating_sub(1)))
            .copied()
    }

    /// The screen to draw: the pre-shot copy while a shot is being replayed,
    /// so craters and collapses land when the shell does.
    pub(crate) fn shown_screen(&self) -> Option<&Screen> {
        match (self.play.anim.as_ref(), self.play.run.as_ref()) {
            (Some(anim), _) if !anim.done() => Some(&anim.before),
            (_, Some(run)) => Some(&run.screen),
            _ => None,
        }
    }

    /// The camera for this frame: `cam_x` less the landing shake, and the
    /// buffer's size. Whole pixels throughout (DESIGN §5.1).
    pub(crate) fn view(&self) -> View {
        let shake = self
            .play
            .anim
            .as_ref()
            .filter(|a| a.done())
            .map(|a| {
                presentation::shake_offset(a.settled, if a.collapsed.is_empty() { 1 } else { 2 }).0
            })
            .unwrap_or(0);
        View {
            cam_x: self.play.cam_x - shake,
            w: self.display.buffer_w,
            h: self.display.buffer_h,
        }
    }

    /// What a frame shows that input or a step can change, compared across
    /// the frame as a second line of defence behind the explicit redraws.
    pub(crate) fn frame_state(&self) -> FrameState {
        FrameState {
            mode: self.mode,
            aim: (self.play.angle_ticks, self.play.power_ticks),
            cam_x: self.play.cam_x,
            buffer: (
                self.display.buffer_w,
                self.display.buffer_h,
                self.display.letterbox,
            ),
            devices: (self.input.touch_active, self.input.controller_active),
            run: self.play.run.as_ref().map(|r| {
                (
                    r.screen_index,
                    r.phase,
                    r.shells,
                    r.shots_on_screen,
                    r.fuse.to_bits(),
                    r.emplacement_offset.to_bits(),
                )
            }),
            seed: (self.seed_text.len(), self.input.seed_focus),
            versus: (
                self.versus_fingerprint(),
                self.versus.help,
                self.versus.rolling_off,
                self.versus.ai,
                self.versus.level,
            ),
            settings: (
                self.save.volume,
                self.save.muted,
                self.save.title_music,
                self.save.last_arc,
                self.save.touch_aim,
                self.save.arc_hint,
                self.save.learned,
            ),
            demo: (self.demo.is_some(), self.save.idle_demo),
            watch: (
                self.watch_fingerprint(),
                self.history.focus,
                self.history.file.records.len(),
            ),
            editor: (
                self.editor_fingerprint(),
                self.versus.ground.as_ref().map(|g| g.label()),
            ),
            misc: (
                self.debug,
                self.crash.crash_copied,
                self.crash.last_crash.is_some(),
            ),
        }
    }

    // The settings below are `Option`s in the save, `None` until the player
    // picks: these read them with the current default filled in.

    pub(crate) fn last_arc_on(&self) -> bool {
        self.save.last_arc.unwrap_or(DEFAULT_LAST_ARC)
    }

    pub(crate) fn touch_aim(&self) -> TouchAim {
        self.save.touch_aim.unwrap_or(DEFAULT_TOUCH_AIM)
    }

    pub(crate) fn arc_hint(&self) -> ArcHint {
        self.save.arc_hint.unwrap_or(DEFAULT_ARC_HINT)
    }

    /// Touch aiming is by dragging rather than the angle and power buttons.
    pub(crate) fn drag_aiming(&self) -> bool {
        self.input.touch_active && self.touch_aim() == TouchAim::Drag
    }

    /// The mode as the crash report names it. A frame of the attract demo
    /// is tagged as such (`Demo/Playing`, `Demo/Versus`), so a report from
    /// one is not read as a player's run.
    pub fn mode_name(&self) -> &'static str {
        if self.in_demo() {
            return match self.mode {
                Mode::Versus => "Demo/Versus",
                Mode::Playing => "Demo/Playing",
                _ => "Demo",
            };
        }
        if self.watching() {
            return match self.mode {
                Mode::Versus => "Watch/Versus",
                _ => "Watch/Playing",
            };
        }
        match self.mode {
            Mode::Title => "Title",
            Mode::Controls => "Controls",
            Mode::SeedEntry => "SeedEntry",
            Mode::Playing => "Playing",
            Mode::Paused => "Paused",
            Mode::GameOver => "GameOver",
            Mode::VersusSetup => "VersusSetup",
            Mode::Versus => "Versus",
            Mode::Editor => "Editor",
            Mode::Replays => "Replays",
        }
    }

    /// The prompt that teaches the next thing the player has not done yet
    /// (`presentation`'s `LEARN_*` bits), if any.
    pub(crate) fn onboarding_prompt(&self, phase: Phase, wind: Option<f64>) -> Option<String> {
        // The demo and a watch teach nothing: their prompts would name
        // controls they do not read.
        if self.spectating() {
            return None;
        }
        if phase == Phase::Drafting
            && self.save.learned & LEARN_DRAFT == 0
            && self.input.touch_active
        {
            return Some("TAP A CARD  PICK A MODIFIER".into());
        }
        if phase == Phase::Drafting && self.save.learned & LEARN_DRAFT == 0 {
            return Some(format!(
                "{} {} {}  PICK A MODIFIER",
                self.input_label(Action::Draft1),
                self.input_label(Action::Draft2),
                self.input_label(Action::Draft3)
            ));
        }
        let learned = self.save.learned;
        if self.drag_aiming()
            && learned & (LEARN_ANGLE | LEARN_POWER | LEARN_FIRE)
                != LEARN_ANGLE | LEARN_POWER | LEARN_FIRE
        {
            return Some(if self.input.touch.drag().is_some() {
                "LET GO TO FIRE  PULL BACK TO CANCEL".into()
            } else {
                "DRAG BACK FROM OPEN GROUND TO AIM".into()
            });
        }
        if learned & LEARN_ANGLE == 0 {
            Some(format!(
                "{} / {}  SET ANGLE",
                self.input_label(Action::Left),
                self.input_label(Action::Right)
            ))
        } else if learned & LEARN_POWER == 0 {
            Some(format!(
                "{} / {}  SET POWER",
                self.input_label(Action::Up),
                self.input_label(Action::Down)
            ))
        } else if learned & LEARN_FIRE == 0 {
            // On a touchscreen the button already says FIRE.
            if self.input.touch_active {
                Some("TAP FIRE".into())
            } else {
                Some(format!("{}  FIRE", self.input_label(Action::Fire)))
            }
        } else if learned & LEARN_CRATER == 0 {
            Some("CRATERS PERSIST - CORRECT FROM THE LAST SHOT".into())
        } else if learned & LEARN_WIND == 0 {
            wind.filter(|w| w.abs() > 0.01)
                .map(|_| "READ THE WINDSOCK - CORRECT AGAINST IT".into())
        } else if learned & LEARN_SHELLS == 0 {
            // Only once the reserve has dipped below where it started: a
            // player still banking refunds has not yet felt the clock.
            self.play
                .run
                .as_ref()
                .is_some_and(|r| r.shells < salt_line_sim::START_SHELLS)
                .then(|| "SHELLS ARE THE RUN CLOCK - SPEND CAREFULLY".into())
        } else {
            None
        }
    }
}

mod controls;
mod demo;
mod editor;
mod play;
mod session;
mod versus;
mod watch;

pub(crate) use demo::Demo;
pub use demo::{Card as DemoCard, Leg as DemoLeg, Started as DemoStarted, IDLE_DEMO_AFTER};
pub(crate) use editor::{draft_label as editor_label, EditorState, GroundPick, LayoutsState};

/// What the title and the editor say while a newer build's draft holds the
/// draft file.
#[cfg(test)]
pub(crate) fn editor_newer_draft_note() -> &'static str {
    editor::NEWER_DRAFT_NOTE
}
pub use editor::{
    Page as EditorPage, Purpose as EditorPurpose, TestFire, Toast, VISIBLE_ROWS as EDITOR_ROWS,
};
pub use versus::{Call as VersusCall, Stage as VersusStage, DING_DELAY, VERSUS_HOLD};
pub(crate) use versus::{VersusPlay, VersusState};
pub use watch::SPEEDS as WATCH_SPEEDS;
pub(crate) use watch::{HistoryState, Watch};

#[cfg(test)]
mod awake_tests;
#[cfg(test)]
mod demo_tests;
#[cfg(test)]
mod disk_tests;
#[cfg(test)]
mod editor_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod versus_tests;
#[cfg(test)]
mod watch_tests;

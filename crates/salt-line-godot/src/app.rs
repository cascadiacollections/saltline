//! The game itself, with no engine in it.
//!
//! # Core and adapter
//!
//! The game is split in three, so that everything that decides anything can
//! be run and checked under `cargo test`:
//!
//! * **This module, the core.** [`Game`] owns the mode, the run, the shot
//!   being replayed, the aim, the drag, the onboarding prompts, the save and
//!   the bookkeeping that keeps a run on disk (DESIGN §9.1). It is stepped by
//!   [`Game::tick`] with a frame's [`Intents`] and answers with the
//!   [`Effect`]s the engine must carry out: play a sound, write a file, set
//!   the clipboard, resize the buffer, quit. It reads no clock, file or
//!   device of its own; what it needs from the outside comes in as
//!   arguments, and what it wants done goes out as effects.
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
//! # Order is behaviour
//!
//! Effects are returned in the order the core asked for them, and the
//! adapter applies them in that order. Some orders are load-bearing: the run
//! file is discarded before a finished run's score goes on the board, so a
//! crash in between loses the score rather than recording it twice.
//!
//! A write whose outcome the player hears about ([`Effect::Store`]) is
//! reported back through [`Game::stored`], which may say so on screen and
//! ask for more.
//!
//! Environment events that must not wait for the next frame — focus lost,
//! the app paused by the platform, the window closing, a touch — are methods
//! the adapter calls as they arrive, and return their effects at once: a
//! phone may never resume a paused app, so its settings are written now.

use salt_line_sim::ballistics::{End, Shot, Trace};
use salt_line_sim::dmath;
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{
    self, buffer_width, daily_seed, format_seed, parse_seed, Phase, RunState, Screen,
};
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_H, SCREEN_W_MAX, SCREEN_W_MIN};

use crate::cli::DevArgs;
use crate::durable::Debounce;
use crate::input::{self, Controls, Intents, Key, Pad};
use crate::keypad::{self, Focus};
use crate::presentation::{
    self, LEARN_ANGLE, LEARN_CRATER, LEARN_DRAFT, LEARN_FIRE, LEARN_POWER, LEARN_SHELLS, LEARN_WIND,
};
use crate::raster::View;
use crate::resume::{self, Found, RUN_LOADING_PATH, RUN_PATH};
use crate::save_format::{ArcHint, Entry, Save, TouchAim};
use crate::touch::{Button, TouchPad};

/// Long Lens zooms the buffer out one step vertically. DESIGN §5.1: it changes
/// the internal buffer size, never the sprite scale.
pub const LONG_LENS_H: i32 = 216;

/// How long a cleared screen is held before the run moves on.
pub const CLEAR_HOLD: f64 = 0.55;
/// Topple time before the rubble sprite replaces the standing one.
pub const TOPPLE_TIME: f64 = 0.22;
/// Key repeat: delay before repeating, then interval.
pub const REPEAT_DELAY: f64 = 0.28;

pub const REPEAT_RATE: f64 = 0.035;

/// How long a status line stays up.
pub const STATUS_TIME: f64 = 2.4;
/// How long a status line about a lost saved run stays up: long enough to
/// read on a title screen the player may not be looking at yet.
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

/// Where the player is. Playing and Paused have a run on screen; the rest are
/// menus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Title,
    Controls,
    SeedEntry,
    Playing,
    Paused,
    GameOver,
}

/// A shot being replayed to the player. The sim has already resolved it.
pub struct Anim {
    pub traces: Vec<Trace>,
    /// The screen as it looked before the shot, so craters appear on impact
    /// rather than the instant the trigger is pulled.
    pub before: Screen,
    /// Sim seconds elapsed in the replay.
    pub t: f64,
    pub longest: f64,
    pub collapsed: Vec<usize>,
    /// Time since the last shell landed.
    pub settled: f64,
    pub impact_played: bool,
    pub collapse_played: bool,
}

impl Anim {
    pub fn done(&self) -> bool {
        self.t >= self.longest
    }
}

/// The game's sounds, one player each. The adapter owns the players.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    Fire,
    Flight,
    Terrain,
    Structure,
    Ricochet,
    Airburst,
    Collapse,
    Draft,
    LowShell,
    Ui,
}

impl Sound {
    pub const ALL: [Sound; 10] = [
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

/// Why the save is being written, which decides what the player is told.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Store {
    /// An onboarding step was learned. A failure is not worth a word.
    Learned,
    /// Settings changed. Not written at all after a panic.
    Settings,
    /// A finished run went on the board. If the board does not reach disk,
    /// `keep` (the finished run's line) is written back to `user://run.txt`
    /// so a later launch can score it. `announce` says "LAST RUN SCORED" on
    /// success: a run scored at launch rather than at its end.
    Score {
        keep: Option<String>,
        announce: bool,
    },
}

/// Something the engine must do for the core. See the module docs.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    Play(Sound),
    /// Set every sound player's volume, in dB.
    Volume(f32),
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
    Clipboard(String),
    Quit,
    /// Write the save (`save::store`), then report the outcome to
    /// `Game::stored`.
    Store {
        save: Box<Save>,
        why: Store,
    },
    /// Write the running run to `user://run.txt`. Skipped after a panic.
    WriteRun(String),
    /// Write a finished run back to `user://run.txt` for a later launch to
    /// score, even after a panic.
    KeepRun(String),
    /// Make sure no run file is left to resume or score. If a file cannot be
    /// deleted, the run file is overwritten with nothing, which
    /// `resume::judge` reads as no run: a finished log must never survive to
    /// be scored twice.
    DiscardRunFile,
    /// Move a `user://` file (`save::rename`).
    Rename {
        from: &'static str,
        to: &'static str,
    },
    /// Delete a `user://` file if it is there.
    Remove(&'static str),
    /// The player has copied or moved on from last session's crash report:
    /// keep it as `crash-old.txt`, not deleted.
    RetireCrashReport,
    /// Log a warning.
    Warn(String),
}

/// What the adapter found of a saved run at launch.
#[derive(Clone, Debug)]
pub enum SavedRun {
    /// `user://run-loading.txt` was there: the last launch died rebuilding a
    /// run.
    Interrupted,
    /// A run file's text, already moved aside to `RUN_LOADING_PATH` so a
    /// log that brings the sim down costs one crash, not one every launch.
    Found(String),
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

/// Longest seed entry takes, in characters.
pub const SEED_MAX: usize = 20;

/// See `Game::frame_state`.
#[derive(PartialEq, Debug)]
pub struct FrameState {
    pub mode: Mode,
    aim: (i32, i32),
    cam_x: i32,
    buffer: (i32, i32, (i32, i32)),
    devices: (bool, bool),
    run: Option<(u32, Phase, i32, u32, u64, u64)>,
    seed: (usize, Focus),
    settings: (
        u8,
        bool,
        Option<bool>,
        Option<TouchAim>,
        Option<ArcHint>,
        u32,
    ),
    misc: (bool, bool, bool),
}

/// Everything the game knows that is not the engine's.
pub struct Game {
    /// Running on a phone or tablet.
    pub mobile: bool,
    /// Extra inset (from the sides, from top and bottom) for edge-anchored HUD
    /// and buttons, in buffer pixels, so nothing crowds a phone's rounded
    /// screen corners. Zero on desktop.
    pub corner_pad: (i32, i32),
    /// The window in screen pixels, and what a display cutout takes from its
    /// left and right edges, as the adapter last read them.
    pub window_size: (i32, i32),
    pub cutout: (i32, i32),
    /// Days since the Unix epoch, as the adapter last read the clock. The
    /// sim never touches system time.
    pub today: u64,

    pub buffer_w: i32,
    pub buffer_h: i32,
    /// Buffer pixels of black kept clear on the left and right for a display
    /// cutout (a phone's camera hole in landscape). The game node is offset
    /// past the left one.
    pub letterbox: (i32, i32),
    /// The buffer is at a resumed run's width, not the device's, and the
    /// window letterboxes it (`set_run_width`).
    pub kept_width: bool,
    /// Camera x is an integer, not a rounded float. DESIGN §5.1 wants it
    /// quantized to whole pixels every frame; keeping the only camera state in
    /// `i32` means there is no float position to forget to round.
    pub cam_x: i32,
    pub cam_target: f64,

    pub mode: Mode,
    pub run: Option<RunState>,
    pub anim: Option<Anim>,
    pub clear_hold: f64,
    /// Aim inputs, in the sim's tick space.
    pub angle_ticks: i32,
    pub power_ticks: i32,

    pub save: Save,
    pub seed_text: String,
    pub daily: bool,
    pub last_seed: u64,
    pub status: String,
    pub status_time: f64,
    pub debug: bool,

    /// This frame's key and pad presses, and hold-to-repeat.
    pub controls: Controls,
    /// The controller's place on the seed keypad.
    pub seed_focus: Focus,
    pub controller_active: bool,
    pub touch: TouchPad,
    /// Showing touch controls. On from the first touch, off again at the next
    /// key or button press, like `controller_active`.
    pub touch_active: bool,
    /// The platform's back gesture (Android), taken as the `back` action.
    pub back_requested: bool,
    /// The aim a touch drag started from, put back if the pull is cancelled.
    pub drag_from: Option<(i32, i32)>,
    /// The primary shell's path from the last shot. Drawn only while the sim
    /// still has a `last_apex`, so it goes when the screen does.
    pub last_arc: Vec<Vec2>,
    pub low_shell_warned: bool,
    /// Bumped whenever the drag preview must be worked out afresh for a
    /// reason its key cannot see (a new loadout, a new run).
    pub preview_epoch: u32,

    /// `--salt-auto`: fire a solver candidate at the nearest standing target
    /// and take every draft. Lets a build be watched, or captured, without a
    /// hand on the keyboard.
    pub auto: bool,
    pub auto_cooldown: f64,
    /// `--salt-miss`: throw the first shot of each screen away, so craters,
    /// chalk marks and the multi-shot path get exercised too.
    pub auto_miss: bool,
    /// `--salt-hold-draft`: leave the draft on screen instead of taking an
    /// offer, so the cards can be looked at.
    pub auto_hold_draft: bool,
    /// `--salt-seed`, for the autopilot's runs.
    pub auto_seed: Option<u64>,
    /// `--salt-shot` has a capture still to take. Nobody is watching with
    /// focus, so focus loss does not pause.
    pub capturing: bool,

    /// Settings changed but not yet written. See `durable::Debounce`.
    pub settings: Debounce,
    /// Last session's crash report, if it left one (`user://crash.txt`).
    pub last_crash: Option<String>,
    /// The player has copied `last_crash` this session.
    pub crash_copied: bool,
    /// Told the player this session that their save is from a newer build.
    pub future_noted: bool,
    /// Showing the error screen. See `broken_tick`.
    pub broken: bool,
    /// The saved run CONTINUE would rebuild, already checked. See `resume`.
    pub resumable: Option<resume::Saved>,
    /// Log length last written to `user://run.txt`; `None` before the first
    /// write of this run.
    pub run_saved_len: Option<usize>,
    /// This run is kept in `user://run.txt`. Off for a `--salt-screen` jump,
    /// which its log cannot rebuild.
    pub run_persist: bool,
    /// This run was rebuilt by CONTINUE, for the crash report.
    pub resumed: bool,
    /// Which new-run action has been pressed once over a saved run and is
    /// waiting for the second press. See `confirm_discard`.
    pub discard_armed: Option<&'static str>,

    /// Effects asked for since they were last taken.
    fx: Vec<Effect>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            mobile: false,
            corner_pad: (0, 0),
            window_size: (0, 0),
            cutout: (0, 0),
            today: 0,
            buffer_w: 320,
            buffer_h: SCREEN_H as i32,
            letterbox: (0, 0),
            kept_width: false,
            cam_x: 0,
            cam_target: 0.0,
            mode: Mode::Title,
            run: None,
            anim: None,
            clear_hold: 0.0,
            angle_ticks: 900,
            power_ticks: 600,
            save: Save::default(),
            seed_text: String::new(),
            daily: true,
            last_seed: 0,
            status: String::new(),
            status_time: 0.0,
            debug: false,
            controls: Controls::default(),
            seed_focus: Focus::default(),
            controller_active: false,
            touch: TouchPad::default(),
            touch_active: false,
            back_requested: false,
            drag_from: None,
            last_arc: Vec::new(),
            low_shell_warned: false,
            preview_epoch: 0,
            auto: false,
            auto_cooldown: 0.0,
            auto_miss: false,
            auto_hold_draft: false,
            auto_seed: None,
            capturing: false,
            settings: Debounce::default(),
            last_crash: None,
            crash_copied: false,
            future_noted: false,
            broken: false,
            resumable: None,
            run_saved_len: None,
            run_persist: false,
            resumed: false,
            discard_armed: None,
            fx: Vec::new(),
        }
    }
}

// ------------------------------------------------------------- the frame

impl Game {
    /// A game at the title with `save` loaded. Phones and tablets start with
    /// touch controls; desktops show them only once the screen is touched.
    pub fn new(mobile: bool, save: Save) -> Game {
        Game {
            mobile,
            touch_active: mobile,
            // Rounded screen corners (Pixel 10: 138 px radius at 1080 px
            // tall): 10 buffer pixels in from the side and 6 down keeps HUD
            // text and buttons comfortably inside the curve without raising
            // the buttons off the ground.
            corner_pad: if mobile { (10, 6) } else { (0, 0) },
            save,
            ..Game::default()
        }
    }

    /// The effects asked for so far, oldest first. Every method below that
    /// returns effects takes them itself; this is for the ones that do not
    /// (`start_run` from a test, say).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.fx)
    }

    /// One frame: timers, the display, input, the replay, persistence.
    ///
    /// `delta` is the engine's, uncapped. Animation and timers never jump
    /// more than a tenth of a second, however long the app sat in the
    /// background (`resume::frame_delta`).
    pub fn tick(&mut self, delta: f64, intents: &Intents) -> Vec<Effect> {
        let delta = resume::frame_delta(delta);
        self.today = intents.today;
        self.status_time = (self.status_time - delta).max(0.0);
        self.poll_display(intents.window, intents.cutout);
        let before = self.frame_state();
        self.begin_input_frame(delta, intents);
        self.handle_mode_input(delta);
        self.back_requested = false;
        if self.auto {
            self.autopilot(delta);
        }
        self.step(delta);
        self.persist_run();
        self.tick_settings(delta);
        let after = self.frame_state();
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
        if !self.broken {
            // Whatever the game last said no longer applies.
            self.broken = true;
            self.status_time = 0.0;
        }
        self.status_time = (self.status_time - delta).max(0.0);
        self.begin_input_frame(delta, intents);
        if self.action_pressed(4, "copy") {
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
        } else if self.action_pressed(2, "back") {
            self.fx.push(Effect::Quit);
        }
        self.back_requested = false;
        self.take_effects()
    }

    /// Something on screen is moving, so every frame draws. The adapter adds
    /// what only it can see: a stick or key held.
    pub fn busy(&self) -> bool {
        self.anim.is_some()
            || self.clear_hold > 0.0
            || self.status_time > 0.0
            || self.auto
            || self.touch.active()
    }

    /// Menus, overlays and the error screen run at the idle frame rate.
    pub fn idle(&self) -> bool {
        self.broken || self.mode != Mode::Playing
    }

    /// Every sound player's volume, in dB.
    pub fn volume_db(&self) -> f32 {
        if self.save.muted || self.save.volume == 0 {
            -80.0
        } else {
            20.0 * (self.save.volume as f32 / 100.0).log10()
        }
    }
}

// ------------------------------------------------------ environment events

impl Game {
    /// The platform's back gesture (Android). Taken as the `back` action on
    /// the next frame.
    pub fn go_back(&mut self) {
        self.back_requested = true;
    }

    /// A finger went down at `(x, y)` in buffer pixels. `buttons` are the
    /// ones the last frame drew.
    pub fn finger_down(&mut self, index: i32, x: f32, y: f32, buttons: &[Button]) {
        self.touch_active = true;
        self.touch.finger_down(index, x, y, buttons);
    }

    pub fn finger_up(&mut self, index: i32) {
        self.touch_active = true;
        self.touch.finger_up(index);
    }

    pub fn finger_moved(&mut self, index: i32, x: f32, y: f32) {
        self.touch.finger_moved(index, x, y);
    }

    /// Window or application focus lost: every finger is up, a live screen
    /// pauses, and pending settings go to disk.
    pub fn focus_lost(&mut self) -> Vec<Effect> {
        self.touch.release_all();
        self.auto_pause();
        self.flush_settings();
        self.fx.push(Effect::Redraw);
        self.take_effects()
    }

    /// The platform paused the app. Pending settings go to disk before the
    /// app can be killed: a phone may never resume a paused app.
    pub fn app_paused(&mut self) -> Vec<Effect> {
        self.auto_pause();
        self.flush_settings();
        self.take_effects()
    }

    /// The window is closing or the node leaving the tree: a close is final.
    pub fn closing(&mut self) -> Vec<Effect> {
        self.flush_settings();
        self.take_effects()
    }

    /// A key typed in seed entry. The adapter only sends them in that mode.
    /// `today` is for a blank seed, which is today's daily.
    pub fn typed(&mut self, key: Typed, today: u64) -> Vec<Effect> {
        match key {
            Typed::Backspace => {
                self.seed_text.pop();
            }
            Typed::Enter => {
                self.today = today;
                self.start_seeded_run();
            }
            Typed::Escape => self.leave_seed_entry(),
            Typed::Char(c) => {
                if self.seed_text.chars().count() < SEED_MAX {
                    self.seed_text.push(c);
                }
            }
        }
        self.take_effects()
    }

    /// A write the core asked for with `Effect::Store` reached disk, or did
    /// not.
    pub fn stored(&mut self, why: Store, ok: bool) -> Vec<Effect> {
        match why {
            Store::Learned => {}
            Store::Settings => {
                if !ok {
                    self.note_not_saved("SETTINGS NOT SAVED");
                }
            }
            Store::Score { keep, announce } => {
                if ok {
                    if announce {
                        self.note("LAST RUN SCORED".into());
                    }
                } else {
                    // Not on the board (a save from a newer build, a full
                    // disk): keep the finished log, so a later launch can
                    // score it.
                    if let Some(line) = keep {
                        self.fx.push(Effect::KeepRun(line));
                    }
                    self.note_not_saved("SCORE NOT SAVED");
                }
            }
        }
        self.take_effects()
    }
}

// ------------------------------------------------------------------ launch

impl Game {
    /// Size the buffer for the display at launch.
    pub fn fit_display(&mut self, window: (i32, i32), cutout: (i32, i32)) -> Vec<Effect> {
        self.window_size = window;
        self.cutout = cutout;
        self.buffer_w = self.pick_width();
        self.apply_buffer_size();
        self.take_effects()
    }

    /// Act on the developer flags: the autopilot, a run started on a seed and
    /// screen, or CONTINUE.
    pub fn start_dev(&mut self, args: &DevArgs, today: u64) -> Vec<Effect> {
        self.today = today;
        self.capturing = args.shot.is_some();
        self.auto = args.auto;
        self.auto_miss = args.miss;
        self.auto_hold_draft = args.hold_draft;
        self.auto_seed = args.seed;
        if let Some(seed) = args.seed {
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
                self.fx.push(Effect::Warn(format!(
                    "salt-line: discarding {RUN_LOADING_PATH}: a rebuild did not finish"
                )));
                self.fx.push(Effect::Remove(RUN_LOADING_PATH));
                self.fx.push(Effect::DiscardRunFile);
                self.note(resume::Stale::Diverged.note().into());
                self.status_time = STALE_STATUS_TIME;
                return self.take_effects();
            }
            SavedRun::Found(text) => text,
        };
        let back = Effect::Rename {
            from: RUN_LOADING_PATH,
            to: RUN_PATH,
        };
        match resume::judge(Some(&text)) {
            Found::Nothing => self.fx.push(back),
            Found::Continue(saved) => {
                self.fx.push(back);
                self.resumable = Some(saved);
            }
            Found::Finished(saved, run) => {
                // Gone before the score goes on the board, as in `finish_run`:
                // a crash in between loses it rather than recording it twice.
                self.fx.push(Effect::Remove(RUN_LOADING_PATH));
                self.fx.push(Effect::DiscardRunFile);
                let (screens, shells, shots) = run.score();
                let entry = Entry {
                    seed: saved.seed,
                    screens,
                    shells,
                    shots,
                    daily: saved.daily,
                };
                // Kept for a launch that can record it, if this one cannot.
                self.record(entry, Some(text), true);
            }
            Found::Discard(why) => {
                self.fx.push(Effect::Warn(format!(
                    "salt-line: discarding {RUN_PATH}: {why:?}"
                )));
                self.fx.push(Effect::Remove(RUN_LOADING_PATH));
                self.fx.push(Effect::DiscardRunFile);
                self.note(why.note().into());
                self.status_time = STALE_STATUS_TIME;
            }
        }
        self.take_effects()
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
        self.run = Some(run);
        self.mode = Mode::Playing;
        self.status_time = 0.0;
    }
}

// ----------------------------------------------------------------- queries

impl Game {
    /// Internal buffer width. DESIGN §5.1: fix the height at 180, let the
    /// width float 320..400 by device aspect, and generate terrain to fill it.
    pub fn pick_width(&self) -> i32 {
        let size = self.window_size;
        let (l, r) = self.cutout;
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
    pub fn letterbox_now(&self) -> (i32, i32) {
        let (l, r) = self.cutout;
        let win = self.window_size;
        if win.1 <= 0 || self.buffer_h <= 0 || self.buffer_w <= 0 {
            return (0, 0);
        }
        // Buffer pixels per screen pixel: fitted by height, unless the buffer
        // (with the insets beside it) is too wide for the window, as a resumed
        // run from a wider device is, and is fitted by width instead.
        let usable = (win.0 - l - r).max(1) as f64;
        let scale = (win.1 as f64 / self.buffer_h as f64).min(usable / self.buffer_w as f64);
        (
            (l as f64 / scale).ceil() as i32,
            (r as f64 / scale).ceil() as i32,
        )
    }

    pub fn long_lens(&self) -> bool {
        self.run
            .as_ref()
            .map(|r| r.loadout.has_flag(flags::LONG_LENS))
            .unwrap_or(false)
    }

    pub fn world_width(&self) -> i32 {
        self.run
            .as_ref()
            .map(|r| r.width as i32)
            .unwrap_or(self.buffer_w)
    }

    /// The lead shell's position in the replay, if one is in the air.
    pub fn shell_head(&self) -> Option<Vec2> {
        let anim = self.anim.as_ref()?;
        let primary = anim.traces.first()?;
        let i = (anim.t / DT).floor().max(0.0) as usize;
        primary
            .path
            .get(i.min(primary.path.len().saturating_sub(1)))
            .copied()
    }

    /// The screen to draw: the pre-shot copy while a shot is being replayed,
    /// so craters and collapses land when the shell does.
    pub fn shown_screen(&self) -> Option<&Screen> {
        match (self.anim.as_ref(), self.run.as_ref()) {
            (Some(anim), _) if !anim.done() => Some(&anim.before),
            (_, Some(run)) => Some(&run.screen),
            _ => None,
        }
    }

    pub fn view(&self) -> View {
        let shake = self
            .anim
            .as_ref()
            .filter(|a| a.done())
            .map(|a| {
                presentation::shake_offset(a.settled, if a.collapsed.is_empty() { 1 } else { 2 }).0
            })
            .unwrap_or(0);
        View {
            cam_x: self.cam_x - shake,
            w: self.buffer_w,
            h: self.buffer_h,
        }
    }

    /// What a frame shows that input or a step can change, compared across
    /// the frame as a second line of defence behind the explicit redraws.
    pub fn frame_state(&self) -> FrameState {
        FrameState {
            mode: self.mode,
            aim: (self.angle_ticks, self.power_ticks),
            cam_x: self.cam_x,
            buffer: (self.buffer_w, self.buffer_h, self.letterbox),
            devices: (self.touch_active, self.controller_active),
            run: self.run.as_ref().map(|r| {
                (
                    r.screen_index,
                    r.phase,
                    r.shells,
                    r.shots_on_screen,
                    r.fuse.to_bits(),
                    r.emplacement_offset.to_bits(),
                )
            }),
            seed: (self.seed_text.len(), self.seed_focus),
            settings: (
                self.save.volume,
                self.save.muted,
                self.save.last_arc,
                self.save.touch_aim,
                self.save.arc_hint,
                self.save.learned,
            ),
            misc: (self.debug, self.crash_copied, self.last_crash.is_some()),
        }
    }

    pub fn last_arc_on(&self) -> bool {
        self.save.last_arc.unwrap_or(DEFAULT_LAST_ARC)
    }

    pub fn touch_aim(&self) -> TouchAim {
        self.save.touch_aim.unwrap_or(DEFAULT_TOUCH_AIM)
    }

    pub fn arc_hint(&self) -> ArcHint {
        self.save.arc_hint.unwrap_or(DEFAULT_ARC_HINT)
    }

    /// Touch aiming is by dragging rather than the angle and power buttons.
    pub fn drag_aiming(&self) -> bool {
        self.touch_active && self.touch_aim() == TouchAim::Drag
    }

    /// The mode as the crash report names it.
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Title => "Title",
            Mode::Controls => "Controls",
            Mode::SeedEntry => "SeedEntry",
            Mode::Playing => "Playing",
            Mode::Paused => "Paused",
            Mode::GameOver => "GameOver",
        }
    }

    /// The prompt that teaches the next thing the player has not done yet
    /// (`presentation`'s `LEARN_*` bits), if any.
    pub fn onboarding_prompt(&self, phase: Phase, wind: Option<f64>) -> Option<String> {
        if phase == Phase::Drafting && self.save.learned & LEARN_DRAFT == 0 && self.touch_active {
            return Some("TAP A CARD  PICK A MODIFIER".into());
        }
        if phase == Phase::Drafting && self.save.learned & LEARN_DRAFT == 0 {
            return Some(format!(
                "{} {} {}  PICK A MODIFIER",
                self.input_label("draft_1"),
                self.input_label("draft_2"),
                self.input_label("draft_3")
            ));
        }
        let learned = self.save.learned;
        if self.drag_aiming()
            && learned & (LEARN_ANGLE | LEARN_POWER | LEARN_FIRE)
                != LEARN_ANGLE | LEARN_POWER | LEARN_FIRE
        {
            return Some(if self.touch.drag().is_some() {
                "LET GO TO FIRE  PULL BACK TO CANCEL".into()
            } else {
                "DRAG BACK FROM OPEN GROUND TO AIM".into()
            });
        }
        if learned & LEARN_ANGLE == 0 {
            Some(format!(
                "{} / {}  SET ANGLE",
                self.input_label("left"),
                self.input_label("right")
            ))
        } else if learned & LEARN_POWER == 0 {
            Some(format!(
                "{} / {}  SET POWER",
                self.input_label("up"),
                self.input_label("down")
            ))
        } else if learned & LEARN_FIRE == 0 {
            // On a touchscreen the button already says FIRE.
            if self.touch_active {
                Some("TAP FIRE".into())
            } else {
                Some(format!("{}  FIRE", self.input_label("fire")))
            }
        } else if learned & LEARN_CRATER == 0 {
            Some("CRATERS PERSIST - CORRECT FROM THE LAST SHOT".into())
        } else if learned & LEARN_WIND == 0 {
            wind.filter(|w| w.abs() > 0.01)
                .map(|_| "READ THE WINDSOCK - CORRECT AGAINST IT".into())
        } else if learned & LEARN_SHELLS == 0 {
            Some("SHELLS ARE THE RUN CLOCK - SPEND CAREFULLY".into())
        } else {
            None
        }
    }
}

// ------------------------------------------------------------------ actions

impl Game {
    fn binding(&self, action: &str, default_key: &str, default_pad: &str) -> (String, String) {
        self.save
            .bindings
            .iter()
            .find(|(a, _, _)| a == action)
            .map(|(_, k, p)| (k.clone(), p.clone()))
            .unwrap_or_else(|| (default_key.into(), default_pad.into()))
    }

    /// The key and pad tokens bound to an action.
    pub fn action_binding(&self, action: &str) -> (String, String) {
        let defaults = input::default_binding(action);
        self.binding(action, defaults.0, defaults.1)
    }

    /// Once per frame, before any action is read.
    fn begin_input_frame(&mut self, delta: f64, intents: &Intents) {
        self.controls.begin(intents);
        self.touch.update(delta, REPEAT_DELAY, REPEAT_RATE);
    }

    /// This frame's press of a key, taken. See `Controls::pressed`.
    fn pressed(&mut self, key: Key) -> bool {
        self.controls.pressed(key)
    }

    fn shift_held(&self) -> bool {
        self.controls.held(Key::Shift)
    }

    /// True on the frame the button went down on any joypad.
    fn pad_pressed(&mut self, button: Pad) -> bool {
        let down = self.controls.take_pad(button);
        if down {
            self.controller_active = true;
            self.touch_active = false;
        }
        down
    }

    fn action_pressed(&mut self, _slot: usize, action: &str) -> bool {
        let (key, _) = self.action_binding(action);
        let key_down = Key::from_token(&key)
            .map(|k| self.pressed(k))
            .unwrap_or(false);
        if key_down {
            self.controller_active = false;
            self.touch_active = false;
        }
        let pad_down = self.pad_action_pressed(action);
        let touch_down = self.touch.pressed(action) || (action == "back" && self.back_requested);
        key_down || pad_down || touch_down
    }

    /// `action_pressed` for the controller alone.
    fn pad_action_pressed(&mut self, action: &str) -> bool {
        let (_, pad) = self.action_binding(action);
        Pad::from_token(&pad)
            .map(|b| self.pad_pressed(b))
            .unwrap_or(false)
    }

    fn action_repeated(&mut self, slot: usize, action: &str, delta: f64) -> u32 {
        let (key, pad) = self.action_binding(action);
        let keyboard = Key::from_token(&key)
            .map(|k| self.controls.repeated(k, delta, REPEAT_DELAY, REPEAT_RATE))
            .unwrap_or(0);
        let buttons = match Pad::from_token(&pad) {
            Some(b) => self
                .controls
                .pad_repeated(b, delta, REPEAT_DELAY, REPEAT_RATE),
            None => 0,
        };
        // Left stick is always available in addition to remappable D-pad.
        let dir = match action {
            "left" => Some(input::STICK_LEFT),
            "right" => Some(input::STICK_RIGHT),
            "up" => Some(input::STICK_UP),
            "down" => Some(input::STICK_DOWN),
            _ => None,
        };
        let stick_down = dir.is_some_and(|d| self.controls.stick(d));
        let stick =
            self.controls
                .stick_repeated(slot, stick_down, delta, REPEAT_DELAY, REPEAT_RATE);
        if keyboard > 0 {
            self.controller_active = false;
            self.touch_active = false;
        }
        if buttons > 0 || stick_down {
            self.controller_active = true;
            self.touch_active = false;
        }
        keyboard + buttons + stick + self.touch.repeated(action)
    }

    /// What a prompt calls an action on the device in use.
    pub fn input_label(&self, action: &str) -> String {
        if self.touch_active {
            return input::touch_label(action).to_string();
        }
        let (key, pad) = self.action_binding(action);
        if self.controller_active {
            pad
        } else {
            key
        }
    }
}

// -------------------------------------------------------------- mode input

impl Game {
    fn handle_mode_input(&mut self, delta: f64) {
        if self.pressed(Key::F1) {
            self.debug = !self.debug;
        }
        if self.action_pressed(15, "mute") {
            self.save.muted = !self.save.muted;
            self.apply_volume();
            self.settings.touch();
            self.note(if self.save.muted {
                "SOUND MUTED".into()
            } else {
                "SOUND ON".into()
            });
        }
        match self.mode {
            Mode::Title => {
                let resumable = self.resumable.is_some();
                let confirm = self.action_pressed(0, "confirm") || self.pressed(Key::KpEnter);
                let daily = if resumable {
                    self.action_pressed(11, "new_run")
                } else {
                    confirm
                };
                if resumable && confirm {
                    self.continue_run();
                } else if daily {
                    if self.confirm_discard("daily") {
                        let seed = daily_seed(self.today);
                        self.play(Sound::Ui);
                        self.start_run(seed, true);
                    }
                } else if self.action_pressed(17, "seed") {
                    if self.confirm_discard("seed") {
                        self.open_seed_entry();
                    }
                } else if self.last_crash.is_some() && self.action_pressed(4, "copy") {
                    self.copy_last_crash();
                } else if self.action_pressed(1, "controls") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Controls;
                } else if self.action_pressed(2, "back") {
                    self.fx.push(Effect::Quit);
                }
                if self.mode != Mode::Title {
                    self.discard_armed = None;
                }
            }
            Mode::Controls => {
                let down = self.action_repeated(0, "left", delta);
                let up = self.action_repeated(1, "right", delta);
                if down + up > 0 {
                    let volume =
                        (self.save.volume as i32 + (up as i32 - down as i32) * 5).clamp(0, 100);
                    self.save.volume = volume as u8;
                    self.save.muted = false;
                    self.apply_volume();
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.touch.pressed("aim_mode") {
                    self.save.touch_aim = Some(self.touch_aim().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                } else if self.touch.pressed("arc_hint") {
                    self.save.arc_hint = Some(self.arc_hint().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(16, "last_arc") {
                    self.save.last_arc = Some(!self.last_arc_on());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(3, "replay") {
                    self.save.bindings.clear();
                    self.settings.touch();
                    self.note("DEFAULT BINDINGS RESTORED".into());
                } else if self.action_pressed(2, "back") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                }
            }
            Mode::Paused => {
                if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Playing;
                } else if self.action_pressed(3, "replay") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                    self.suspend_run();
                }
            }
            Mode::SeedEntry => self.handle_seed_input(delta),
            Mode::GameOver => {
                if self.action_pressed(4, "copy") {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.last_seed);
                    self.fx.push(Effect::Clipboard(seed.clone()));
                    self.note(format!("COPIED {seed}"));
                } else if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.mode = Mode::Title;
                } else if self.action_pressed(3, "replay") {
                    let seed = self.last_seed;
                    let daily = self.daily;
                    self.start_run(seed, daily);
                }
            }
            Mode::Playing => self.handle_play_input(delta),
        }
    }

    /// Keyboard text comes in through `typed`. This is the touch keypad, and
    /// the controller moving a focus over the same keypad: A presses the
    /// focused key, B deletes, or leaves once nothing is typed.
    fn handle_seed_input(&mut self, delta: f64) {
        let dx = self.action_repeated(1, "right", delta) as i32
            - self.action_repeated(0, "left", delta) as i32;
        let dy = self.action_repeated(2, "down", delta) as i32
            - self.action_repeated(3, "up", delta) as i32;
        if dx != 0 || dy != 0 {
            self.seed_focus.step(dx, dy);
        }
        let key = if self.pad_action_pressed("confirm") {
            Some(self.seed_focus.action())
        } else if self.pad_action_pressed("back") {
            Some(if self.seed_text.is_empty() {
                "back"
            } else {
                keypad::DEL
            })
        } else if self.touch.pressed("back") || self.back_requested {
            Some("back")
        } else {
            ["confirm", keypad::DEL]
                .into_iter()
                .chain(keypad::KEYS)
                .find(|k| self.touch.pressed(k))
        };
        match key {
            Some("confirm") => self.start_seeded_run(),
            Some("back") => self.leave_seed_entry(),
            Some(keypad::DEL) => {
                self.seed_text.pop();
            }
            Some(k) if self.seed_text.chars().count() < SEED_MAX => self.seed_text.push_str(k),
            _ => {}
        }
    }

    fn handle_play_input(&mut self, delta: f64) {
        if self.action_pressed(2, "back") {
            self.touch.cancel_drag();
            self.mode = Mode::Paused;
            return;
        }
        let phase = self.run.as_ref().map(|r| r.phase);
        let Some(phase) = phase else { return };

        // A drag only aims on the screen it started on: drafting or a shot in
        // the air forgets it, so it can never drop into a later shot.
        if phase != Phase::Firing || self.anim.is_some() || self.clear_hold > 0.0 {
            self.touch.cancel_drag();
        }

        if phase == Phase::Drafting {
            for (i, action) in ["draft_1", "draft_2", "draft_3"].into_iter().enumerate() {
                if self.action_pressed(5 + i, action) {
                    let taken = self.run.as_mut().and_then(|r| r.take_offer(i));
                    if let Some(m) = taken {
                        self.new_preview();
                        self.learn(LEARN_DRAFT);
                        self.play(Sound::Draft);
                        self.note(format!("TOOK {}", m.name()));
                        self.apply_buffer_size();
                    }
                }
            }
            return;
        }

        // Nothing but waiting while a shot is in the air.
        if self.anim.is_some() || self.clear_hold > 0.0 {
            return;
        }

        let quantum = self
            .run
            .as_ref()
            .map(|r| r.loadout.angle_quantum_ticks)
            .unwrap_or(5)
            .max(1);
        let mult = if self.shift_held() { 10 } else { 1 };

        let left = self.action_repeated(0, "left", delta) as i32;
        let right = self.action_repeated(1, "right", delta) as i32;
        if left + right != 0 {
            self.learn(LEARN_ANGLE);
            self.angle_ticks = (self.angle_ticks + (right - left) * quantum * mult)
                .clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, "down", delta) as i32;
        let up = self.action_repeated(3, "up", delta) as i32;
        if up + down != 0 {
            self.learn(LEARN_POWER);
            self.power_ticks =
                (self.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }

        // Airburst fuse.
        let fuse_step = 4.0 * mult as f64;
        let minus = self.action_repeated(4, "fuse_down", delta) as f64;
        let plus = self.action_repeated(5, "fuse_up", delta) as f64;
        if minus + plus > 0.0 {
            if let Some(r) = self.run.as_mut() {
                if r.loadout.airburst() {
                    let f = r.fuse + (plus - minus) * fuse_step;
                    r.set_fuse(f);
                }
            }
        }

        // Enfilade reposition.
        let l = self.action_pressed(8, "reposition_left");
        let rr = self.action_pressed(9, "reposition_right");
        if l || rr {
            let step = if l { -5.0 } else { 5.0 };
            let ok = self.run.as_mut().map(|r| {
                let target = r.emplacement_offset + step;
                r.reposition(target)
            });
            if ok == Some(false) {
                self.note("NO ENFILADE".into());
            }
        }

        if self.action_pressed(10, "fire") || self.pressed(Key::Enter) {
            self.fire();
        }

        if self.drag_aiming() {
            self.handle_drag_aim(quantum);
        }
    }

    /// Pull back to aim, let go to fire. The aim follows the finger live, so
    /// the readouts and barrel show exactly what a release would fire; pulling
    /// back inside the dead zone puts the old aim back and cancels.
    fn handle_drag_aim(&mut self, quantum: i32) {
        let dropped = self.touch.dropped();
        let Some((start, end)) = dropped.or(self.touch.drag()) else {
            // Abandoned (paused, focus lost): put the old aim back.
            if let Some(before) = self.drag_from.take() {
                (self.angle_ticks, self.power_ticks) = before;
            }
            return;
        };
        let before = *self
            .drag_from
            .get_or_insert((self.angle_ticks, self.power_ticks));
        let aim = crate::aim::pull_to_aim(start, end, quantum);
        match aim {
            Some((angle, power)) => {
                self.learn(LEARN_ANGLE);
                self.learn(LEARN_POWER);
                (self.angle_ticks, self.power_ticks) = (angle, power);
            }
            None => (self.angle_ticks, self.power_ticks) = before,
        }
        if dropped.is_some() {
            self.drag_from = None;
            if aim.is_some() {
                self.fire();
            }
        }
    }

    fn start_seeded_run(&mut self) {
        let seed = if self.seed_text.trim().is_empty() {
            daily_seed(self.today)
        } else {
            parse_seed(&self.seed_text)
        };
        self.start_run(seed, false);
    }

    fn leave_seed_entry(&mut self) {
        self.mode = Mode::Title;
    }

    pub fn open_seed_entry(&mut self) {
        self.seed_text.clear();
        self.seed_focus = Focus::default();
        self.mode = Mode::SeedEntry;
    }
}

// --------------------------------------------------------------- run steps

impl Game {
    pub fn start_run(&mut self, seed: u64, daily: bool) {
        // A new run is generated for the device, whatever a resumed one used.
        self.set_run_width(None);
        // The world is as wide as the buffer, so the run is generated for it.
        let width = self.buffer_w as usize;
        let state = RunState::new(seed, width);
        self.angle_ticks = 900;
        self.power_ticks = 600;
        self.anim = None;
        self.last_arc.clear();
        self.new_preview();
        self.clear_hold = 0.0;
        self.cam_x = 0;
        self.cam_target = 0.0;
        self.daily = daily;
        self.low_shell_warned = false;
        self.last_seed = seed;
        self.retire_last_crash();
        self.run = Some(state);
        self.mode = Mode::Playing;
        self.apply_buffer_size();
        self.begin_persisting();
        self.note(format!("SEED {}", format_seed(seed)));
    }

    /// Level inspector: drop straight onto a screen index without playing the
    /// ones before it. DESIGN §2.2 is what makes this possible — each screen
    /// generates from its own seed, so screen 400 costs the same as screen 1.
    pub fn jump_to(&mut self, index: u32) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.screen_index = index;
        // The log cannot rebuild a jump, so the run is not kept for resume.
        self.run_persist = false;
        run.screen = run::generate_screen(run.run_seed, index, run.width, &run.loadout);
        self.new_preview();
        self.note(format!("SCREEN {}", index + 1));
    }

    fn fire(&mut self) {
        let learned = self.save.learned;
        if learned & LEARN_WIND != 0 {
            self.learn(LEARN_SHELLS);
        } else if learned & LEARN_CRATER != 0
            && self
                .run
                .as_ref()
                .and_then(|r| r.displayed_wind())
                .is_some_and(|w| w.abs() > 0.01)
        {
            self.learn(LEARN_WIND);
        }
        let shot = Shot::new(self.angle_ticks, self.power_ticks);
        let Some(run) = self.run.as_mut() else { return };
        if run.shells <= 0 {
            return;
        }
        let before = run.screen.clone();
        let Some(report) = run.fire(shot, true) else {
            return;
        };
        self.learn(LEARN_FIRE);
        self.play(Sound::Fire);
        self.play(Sound::Flight);
        let longest = report
            .traces
            .iter()
            .map(|t| t.flight_time)
            .fold(0.0f64, f64::max);
        let collapsed = report.collapsed.clone();
        self.last_arc = report
            .traces
            .first()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.anim = Some(Anim {
            traces: report.traces,
            before,
            t: 0.0,
            longest,
            collapsed,
            settled: 0.0,
            impact_played: false,
            collapse_played: false,
        });
        let shells = self.run.as_ref().map(|r| r.shells).unwrap_or(99);
        if shells < 10 && !self.low_shell_warned {
            self.low_shell_warned = true;
            self.play(Sound::LowShell);
            self.note("SHELLS ARE YOUR RUN CLOCK".into());
        }
    }

    fn step(&mut self, delta: f64) {
        if self.mode != Mode::Playing {
            return;
        }

        // Advance the shot replay. The sim ran at a fixed 1/120; this only
        // walks the recorded path, so playback speed cannot affect anything.
        let mut sounds = Vec::new();
        let mut learned_impact = false;
        if let Some(anim) = self.anim.as_mut() {
            anim.t += delta;
            if anim.done() {
                anim.settled += delta;
                if !anim.impact_played {
                    anim.impact_played = true;
                    learned_impact = true;
                    if let Some(sound) = anim.traces.first().map(|trace| {
                        if trace.bounces > 0 {
                            Sound::Ricochet
                        } else {
                            match trace.end {
                                End::Fuse { .. } => Sound::Airburst,
                                End::Structure { .. } => Sound::Structure,
                                _ => Sound::Terrain,
                            }
                        }
                    }) {
                        sounds.push(sound);
                    }
                }
                if !anim.collapse_played && !anim.collapsed.is_empty() {
                    anim.collapse_played = true;
                    sounds.push(Sound::Collapse);
                }
            }
            let settled = anim.settled;
            let finished = anim.done() && settled > TOPPLE_TIME + 0.12;
            if finished {
                self.anim = None;
                let cleared = self.run.as_ref().map(|r| r.phase) == Some(Phase::Cleared);
                if cleared {
                    self.clear_hold = CLEAR_HOLD;
                } else if self.run.as_ref().map(|r| r.is_over()) == Some(true) {
                    self.finish_run();
                }
            }
        } else if self.clear_hold > 0.0 {
            // Hold on the cleared screen and pan forward, then move on.
            self.clear_hold -= delta;
            let span = (self.world_width() - self.buffer_w).max(0) as f64;
            let t = 1.0 - (self.clear_hold / CLEAR_HOLD).clamp(0.0, 1.0);
            self.cam_target = span * t;
            if self.clear_hold <= 0.0 {
                self.clear_hold = 0.0;
                self.cam_target = 0.0;
                self.cam_x = 0;
                let advanced = self.run.as_mut().map(|r| r.resolve_clear()) == Some(true);
                if advanced {
                    self.after_screen_change();
                }
            }
        } else if let Some(run) = self.run.as_ref() {
            if run.is_over() {
                self.finish_run();
            }
        }
        if learned_impact {
            self.learn(LEARN_CRATER);
        }
        for sound in sounds {
            self.play(sound);
        }

        // Follow the shell when the world is wider than the buffer.
        if self.anim.is_some() {
            let head = self.shell_head();
            if let Some(p) = head {
                let span = (self.world_width() - self.buffer_w).max(0) as f64;
                if span > 0.0 {
                    self.cam_target = (p.x - self.buffer_w as f64 * 0.5).clamp(0.0, span);
                }
            }
        }

        // The camera is quantized to a whole pixel every frame. Panning with a
        // fractional camera is what makes low-res terrain shimmer.
        let span = (self.world_width() - self.buffer_w).max(0) as f64;
        self.cam_x = self.cam_target.round().clamp(0.0, span) as i32;
    }

    fn after_screen_change(&mut self) {
        self.apply_buffer_size();
        if self.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            self.play(Sound::Draft);
        }
    }

    /// The run is over: score it and show the result.
    pub fn finish_run(&mut self) {
        // The run file goes before the score is recorded, never after: a
        // crash in between costs this score, where the other order would
        // record it twice (the next launch scores a finished log).
        let line = self
            .run
            .as_ref()
            .filter(|_| self.run_persist)
            .map(|r| resume::Saved::of(r, self.daily).to_line());
        self.stop_persisting();
        if let Some(run) = self.run.as_ref() {
            let (screens, shells, shots) = run.score();
            let entry = Entry {
                seed: run.run_seed,
                screens,
                shells,
                shots,
                daily: self.daily,
            };
            self.record(entry, line, false);
        }
        self.mode = Mode::GameOver;
    }

    /// Put a finished run on the board and write it; `keep` is its line, for
    /// a later launch to score if the board does not reach disk.
    fn record(&mut self, entry: Entry, keep: Option<String>, announce: bool) {
        self.save.place(entry);
        self.fx.push(Effect::Store {
            save: Box::new(self.save.clone()),
            why: Store::Score { keep, announce },
        });
    }

    /// Play the game for the camera: aim with the solver, take offer 1.
    fn autopilot(&mut self, delta: f64) {
        self.auto_cooldown -= delta;
        if self.mode == Mode::Title {
            let seed = self.auto_seed.unwrap_or_else(|| daily_seed(self.today));
            self.start_run(seed, false);
            return;
        }
        if self.mode != Mode::Playing || self.anim.is_some() || self.clear_hold > 0.0 {
            return;
        }
        if self.auto_cooldown > 0.0 {
            return;
        }
        let phase = self.run.as_ref().map(|r| r.phase);
        match phase {
            Some(Phase::Drafting) => {
                if self.auto_hold_draft {
                    return;
                }
                self.run.as_mut().map(|r| r.take_offer(0));
                self.apply_buffer_size();
                self.auto_cooldown = 0.4;
            }
            Some(Phase::Cleared) => {
                self.clear_hold = CLEAR_HOLD;
            }
            Some(Phase::Firing) => {
                let first = self.run.as_ref().map(|r| r.shots_on_screen) == Some(0);
                if self.auto_miss && first {
                    // A high lob that lands short, well clear of any target.
                    self.angle_ticks = 1400;
                    self.power_ticks = 330;
                    self.fire();
                    self.auto_cooldown = 0.35;
                    return;
                }
                let pick = {
                    let Some(run) = self.run.as_ref() else { return };
                    let target = run.screen.structures.iter().position(|s| !s.is_down());
                    target.and_then(|t| {
                        salt_line_sim::solver::solve_target(&run.screen, &run.loadout, t)
                            .first()
                            .map(|c| (c.shot, t))
                    })
                };
                if let Some((shot, target)) = pick {
                    self.angle_ticks = shot.angle_ticks;
                    self.power_ticks = shot.power_ticks;
                    if let Some(run) = self.run.as_mut() {
                        if let Some(f) = run.screen.default_fuse(target) {
                            run.set_fuse(f);
                        }
                    }
                    self.fire();
                    self.auto_cooldown = 0.35;
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------- display, sound and settings

impl Game {
    fn note(&mut self, text: String) {
        self.status = text;
        self.status_time = STATUS_TIME;
        self.fx.push(Effect::Redraw);
    }

    fn play(&mut self, sound: Sound) {
        self.fx.push(Effect::Play(sound));
    }

    fn apply_volume(&mut self) {
        self.fx.push(Effect::Volume(self.volume_db()));
    }

    /// The drag preview must be worked out afresh.
    fn new_preview(&mut self) {
        self.preview_epoch = self.preview_epoch.wrapping_add(1);
    }

    fn learn(&mut self, bit: u32) {
        if self.save.learned & bit == 0 {
            self.save.learned |= bit;
            self.fx.push(Effect::Store {
                save: Box::new(self.save.clone()),
                why: Store::Learned,
            });
        }
    }

    fn apply_buffer_size(&mut self) {
        let h = if self.long_lens() {
            LONG_LENS_H
        } else {
            SCREEN_H as i32
        };
        self.buffer_h = h;
        self.letterbox = self.letterbox_now();
        self.fx.push(Effect::Buffer {
            w: self.buffer_w,
            h,
            letterbox: self.letterbox,
        });
        self.fx.push(Effect::Redraw);
    }

    /// Follow the display: the window size every frame, and the cutout as
    /// the adapter last read it (a platform round trip on a phone, so not
    /// every frame; see `frame::DisplayPoll`).
    fn poll_display(&mut self, window: (i32, i32), cutout: (i32, i32)) {
        self.window_size = window;
        self.cutout = cutout;
        let want_w = self.pick_width();
        let in_run = matches!(self.mode, Mode::Playing | Mode::Paused);
        if !in_run && self.kept_width {
            // A resumed run's width is done with: back to the device's.
            self.set_run_width(None);
            self.apply_buffer_size();
        } else if self.run.is_none() && want_w != self.buffer_w {
            self.buffer_w = want_w;
            self.apply_buffer_size();
        } else if self.letterbox_now() != self.letterbox {
            self.apply_buffer_size();
        }
    }

    /// Size the buffer for a run: the recorded width for a resumed one, the
    /// device's for anything else. A resumed run generated at another width
    /// keeps it (DESIGN §9.2: a replay must use it), shown letterboxed
    /// instead of stretched across the window.
    fn set_run_width(&mut self, width: Option<usize>) {
        let device = self.pick_width();
        let want = width.map_or(device, |w| w as i32);
        self.buffer_w = want;
        self.kept_width = want != device;
        self.fx.push(Effect::Aspect(if want == device {
            Aspect::Expand
        } else {
            Aspect::Keep
        }));
    }

    /// Write pending settings once they settle, or as soon as the player
    /// leaves Controls. Not every volume repeat: see `durable::Debounce`.
    fn tick_settings(&mut self, delta: f64) {
        if self.settings.tick(delta, self.mode == Mode::Controls) {
            self.write_settings();
        }
    }

    /// Write pending settings now, if there are any: focus loss, pause, close.
    fn flush_settings(&mut self) {
        if self.settings.take() {
            self.write_settings();
        }
    }

    fn write_settings(&mut self) {
        self.fx.push(Effect::Store {
            save: Box::new(self.save.clone()),
            why: Store::Settings,
        });
    }

    /// Say a write failed. A save from a newer build is never written by this
    /// one (by design, `save::store`), so every change would fail; that gets
    /// one note saying why, once a session, instead of a complaint each time.
    fn note_not_saved(&mut self, what: &str) {
        if !self.save.from_the_future {
            self.note(what.into());
        } else if !self.future_noted {
            self.future_noted = true;
            self.note("SAVE IS FROM A NEWER VERSION".into());
        }
    }

    fn copy_last_crash(&mut self) {
        let Some(text) = self.last_crash.clone() else {
            return;
        };
        self.fx.push(Effect::Clipboard(text));
        self.play(Sound::Ui);
        if !self.crash_copied {
            self.crash_copied = true;
            self.fx.push(Effect::RetireCrashReport);
        }
    }

    /// The player has moved on from last session's report (started a run):
    /// stop offering it.
    fn retire_last_crash(&mut self) {
        if self.last_crash.take().is_some() && !self.crash_copied {
            self.fx.push(Effect::RetireCrashReport);
        }
    }
}

// ----------------------------------------------------- lifecycle and resume

impl Game {
    /// Focus lost, or the operating system paused the app: stop the clock on
    /// a live screen, so a shot in the air or a clear in progress is still
    /// there to watch on return. Not during a draft, where nothing moves and
    /// the cards themselves say where the player was; a pause screen over
    /// them would only cost a tap. Not for the dev autopilot or screenshots,
    /// which nobody is watching with focus.
    fn auto_pause(&mut self) {
        if self.mode != Mode::Playing || self.auto || self.capturing {
            return;
        }
        if self.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            return;
        }
        self.touch.cancel_drag();
        self.mode = Mode::Paused;
        // From a notification, outside the frame's before/after compare.
        self.fx.push(Effect::Redraw);
    }

    /// A new run replaces whatever was saved: it is written on the next
    /// frame, log and all (empty, at the start).
    fn begin_persisting(&mut self) {
        self.resumable = None;
        self.discard_armed = None;
        self.run_saved_len = None;
        self.run_persist = true;
        self.resumed = false;
    }

    /// The run is over and on the board: nothing is left to resume.
    fn stop_persisting(&mut self) {
        if self.run_persist {
            self.fx.push(Effect::DiscardRunFile);
        }
        self.run_persist = false;
        self.resumable = None;
    }

    /// A new run replaces the saved one, so over a saved run the first press
    /// of a new-run action only asks, and the second goes ahead. Returns
    /// whether to go ahead.
    fn confirm_discard(&mut self, what: &'static str) -> bool {
        if self.resumable.is_none() || self.discard_armed == Some(what) {
            self.discard_armed = None;
            return true;
        }
        self.discard_armed = Some(what);
        self.play(Sound::Ui);
        self.fx.push(Effect::Redraw);
        false
    }

    /// Write `user://run.txt` whenever the log has grown: a shot fired or a
    /// draft taken (a reposition rides in the next shot's entry). Checked
    /// every frame, written only on a change.
    fn persist_run(&mut self) {
        if !self.run_persist {
            return;
        }
        let Some(run) = self.run.as_ref() else { return };
        let len = run.log.len();
        if self.run_saved_len == Some(len) {
            return;
        }
        let line = resume::Saved::of(run, self.daily).to_line();
        // Tried once per change: a failed write is logged by `save::write`,
        // and the next shot tries again.
        self.fx.push(Effect::WriteRun(line));
        self.run_saved_len = Some(len);
    }

    /// Quit to title keeps the run. It is already on disk, so CONTINUE picks
    /// it up this session exactly as it would after the app was killed; only
    /// starting another run gives it up. Stepping away should not cost a run
    /// the player only wanted a break from.
    ///
    /// A run that is already over (the last shell was in the air when the
    /// player paused) is scored now instead, and shows its result: leaving it
    /// on disk would let the next new run overwrite an unrecorded score.
    fn suspend_run(&mut self) {
        if self.run.as_ref().is_some_and(|r| r.is_over()) {
            self.anim = None;
            self.clear_hold = 0.0;
            self.finish_run();
            return;
        }
        if !self.run_persist {
            return;
        }
        self.resumable = self.run.as_ref().map(|r| resume::Saved::of(r, self.daily));
    }

    /// Rebuild the saved run and drop the player back in where it stood.
    pub fn continue_run(&mut self) {
        let Some(saved) = self.resumable.take() else {
            return;
        };
        let Some(run) = saved.reconstruct() else {
            self.fx.push(Effect::DiscardRunFile);
            self.note(resume::Stale::Diverged.note().into());
            return;
        };
        self.set_run_width(Some(saved.width));
        (self.angle_ticks, self.power_ticks) = saved.aim().unwrap_or((900, 600));
        self.anim = None;
        self.last_arc.clear();
        self.clear_hold = 0.0;
        self.cam_x = 0;
        self.cam_target = 0.0;
        self.daily = saved.daily;
        self.last_seed = saved.seed;
        self.low_shell_warned = run.shells < 10;
        self.retire_last_crash();
        let screen = run.screen_index;
        // What is on disk is this run, as rebuilt.
        self.run_saved_len = Some(run.log.len());
        self.run_persist = true;
        self.resumed = true;
        self.run = Some(run);
        self.mode = Mode::Playing;
        self.new_preview();
        self.fx.push(Effect::Redraw);
        self.play(Sound::Ui);
        // Buffer size for the loadout.
        self.after_screen_change();
        self.note(format!(
            "SCREEN {}  SEED {}",
            screen + 1,
            format_seed(saved.seed)
        ));
    }
}

#[cfg(test)]
mod tests;

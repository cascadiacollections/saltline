//! The game's state, with no engine in it.
//!
//! `game.rs` is the Godot node. This module is what the node drives: the mode
//! the game is in, the run, the shot being replayed, the aim, the save and
//! the bookkeeping that keeps a run on disk. It never calls the engine, so it
//! builds and runs under `cargo test`.

use salt_line_sim::ballistics::Trace;
use salt_line_sim::modifiers::flags;
use salt_line_sim::run::{buffer_width, Phase, RunState, Screen};
use salt_line_sim::{Vec2, DT, SCREEN_H, SCREEN_W_MAX, SCREEN_W_MIN};

use crate::durable::Debounce;
use crate::keypad::Focus;
use crate::presentation;
use crate::raster::View;
use crate::resume;
use crate::save::Save;
use crate::save_format::{ArcHint, TouchAim};
use crate::touch::TouchPad;

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

/// The game's sounds, one player each. The node owns the players.
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

/// See `Game::frame_state`.
#[derive(PartialEq)]
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
    /// left and right edges, as the node last read them.
    pub window_size: (i32, i32),
    pub cutout: (i32, i32),

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

    /// Settings changed but not yet written. See `durable::Debounce`.
    pub settings: Debounce,
    /// Last session's crash report, if it left one (`user://crash.txt`).
    pub last_crash: Option<String>,
    /// The player has copied `last_crash` this session.
    pub crash_copied: bool,
    /// Told the player this session that their save is from a newer build.
    pub future_noted: bool,
    /// Showing the error screen. See `broken_frame`.
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
}

impl Default for Game {
    fn default() -> Self {
        Game {
            mobile: false,
            corner_pad: (0, 0),
            window_size: (0, 0),
            cutout: (0, 0),
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
            seed_focus: Focus::default(),
            controller_active: false,
            touch: TouchPad::default(),
            touch_active: false,
            back_requested: false,
            drag_from: None,
            last_arc: Vec::new(),
            low_shell_warned: false,
            auto: false,
            auto_cooldown: 0.0,
            auto_miss: false,
            auto_hold_draft: false,
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
        }
    }
}

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
    /// the frame as a second line of defence behind the explicit marks.
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
}

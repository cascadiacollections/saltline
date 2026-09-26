//! The game node. Rendering, input, UI — no game logic.
//!
//! Everything that decides anything lives in `salt-line-sim`. This file draws
//! what the sim says is true, feeds it integer inputs, and animates the gap
//! between "the shot has been resolved" and "the player has seen it happen".

use godot::classes::notify::CanvasItemNotification;
use godot::classes::{
    AudioStream, AudioStreamPlayer, DisplayServer, INode2D, Image, ImageTexture, Input, InputEvent,
    InputEventKey, InputEventScreenDrag, InputEventScreenTouch, Node2D, Os, RenderingServer,
    ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::prelude::*;

use salt_line_sim::ballistics::{self, End, FireOptions, Shot, Trace};
use salt_line_sim::dmath;
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{
    self, buffer_width, daily_seed, format_seed, parse_seed, Phase, RunState, Screen, StructState,
    FREE_SCREENS,
};
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_H, SCREEN_W_MAX, SCREEN_W_MIN};

use crate::palette::{slot, Palette};
use crate::presentation::{
    self, RepeatState, LEARN_ANGLE, LEARN_CRATER, LEARN_DRAFT, LEARN_FIRE, LEARN_POWER,
    LEARN_SHELLS, LEARN_WIND,
};
use crate::raster::{self, Canvas, View};
use crate::save::{self, Entry, Save};
use crate::save_format::{ArcHint, TouchAim};
use crate::sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT, PIVOT_BASE};
use crate::store::{self, Entitlement};
use crate::touch::{Button, Rect, TouchPad};

const PALETTE_PATH: &str = "res://art/palette_16.png";
const SPRITE_DIR: &str = "res://art/sprites";

/// Long Lens zooms the buffer out one step vertically. DESIGN §5.1: it changes
/// the internal buffer size, never the sprite scale.
const LONG_LENS_H: i32 = 216;

/// How long a cleared screen is held before the run moves on.
const CLEAR_HOLD: f64 = 0.55;
/// Topple time before the rubble sprite replaces the standing one.
const TOPPLE_TIME: f64 = 0.22;
/// Key repeat: delay before repeating, then interval.
const REPEAT_DELAY: f64 = 0.28;

/// The on-screen seed keypad. Each key's label is also its touch action.
const SEED_KEYS: [&str; 36] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];
const REPEAT_RATE: f64 = 0.035;

/// Touch aiming until the player picks one in Controls. Undecided: buttons are
/// the conservative choice while drag aiming is new.
const DEFAULT_TOUCH_AIM: TouchAim = TouchAim::Buttons;
/// The drag arc preview until the player picks one. Short keeps reading the
/// wind the player's job (DESIGN §2.5).
const DEFAULT_ARC_HINT: ArcHint = ArcHint::Short;
/// Seconds of flight the Short preview shows, so its length reads as power.
const SHORT_ARC_SECONDS: f64 = 0.3;
/// The last shot's arc stays on screen until the player turns it off.
const DEFAULT_LAST_ARC: bool = true;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Title,
    Controls,
    SeedEntry,
    Playing,
    Paused,
    Paywall,
    GameOver,
}

/// A shot being replayed to the player. The sim has already resolved it.
struct Anim {
    traces: Vec<Trace>,
    /// The screen as it looked before the shot, so craters appear on impact
    /// rather than the instant the trigger is pulled.
    before: Screen,
    /// Sim seconds elapsed in the replay.
    t: f64,
    longest: f64,
    collapsed: Vec<usize>,
    /// Time since the last shell landed.
    settled: f64,
    impact_played: bool,
    collapse_played: bool,
}

#[derive(Clone, Copy)]
enum Sound {
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
    const ALL: [Sound; 10] = [
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

    fn file(self) -> &'static str {
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

impl Anim {
    fn done(&self) -> bool {
        self.t >= self.longest
    }
}

#[derive(GodotClass)]
#[class(base = Node2D)]
pub struct SaltLine {
    base: Base<Node2D>,

    pal: Palette,
    sprites: Option<Sprites>,
    /// One buffer for everything: terrain, sprites and UI, all at the internal
    /// resolution. DESIGN §5.1 — "UI renders in the same buffer at the same
    /// resolution".
    canvas: Canvas,
    texture: Option<Gd<ImageTexture>>,

    buffer_w: i32,
    buffer_h: i32,
    /// Camera x is an integer, not a rounded float. DESIGN §5.1 wants it
    /// quantized to whole pixels every frame; keeping the only camera state in
    /// `i32` means there is no float position to forget to round.
    cam_x: i32,
    cam_target: f64,

    mode: Mode,
    run: Option<RunState>,
    anim: Option<Anim>,
    clear_hold: f64,
    /// Aim inputs, in the sim's tick space.
    angle_ticks: i32,
    power_ticks: i32,

    save: Save,
    /// What the platform says this player owns. Asked once at startup; the
    /// save file's `unlocked` flag is a cache, never the authority.
    entitlement: Entitlement,
    seed_text: String,
    daily: bool,
    last_seed: u64,
    status: String,
    status_time: f64,
    debug: bool,

    /// Key edge/repeat state: (key, was_down, timer).
    keys: Vec<(Key, bool, f64)>,
    pad_edges: Vec<(JoyButton, bool)>,
    pad_repeat: [RepeatState; 8],
    controller_active: bool,
    /// On-screen buttons drawn last frame, hit-tested by this frame's touches.
    touch_buttons: Vec<Button>,
    touch: TouchPad,
    /// The aim a touch drag started from, put back if the pull is cancelled.
    drag_from: Option<(i32, i32)>,
    /// The primary shell's path from the last shot. Drawn only while the sim
    /// still has a `last_apex`, so it goes when the screen does.
    last_arc: Vec<Vec2>,
    /// Showing touch controls. On from the first touch, off again at the next
    /// key or button press, like `controller_active`.
    touch_active: bool,
    /// The platform's back gesture (Android), taken as the `back` action.
    back_requested: bool,
    /// Running on a phone or tablet.
    mobile: bool,
    /// Extra inset (from the sides, from top and bottom) for edge-anchored HUD
    /// and buttons, in buffer pixels, so nothing crowds a phone's rounded
    /// screen corners. Zero on desktop.
    corner_pad: (i32, i32),
    /// Buffer pixels of black kept clear on the left and right for a display
    /// cutout (a phone's camera hole in landscape). The game node is offset
    /// past the left one.
    letterbox: (i32, i32),
    audio: Vec<Option<Gd<AudioStreamPlayer>>>,
    low_shell_warned: bool,

    /// `--salt-shot <path>`: render a few frames, save the buffer as a PNG at
    /// native resolution, and quit. How the pixel output gets checked without
    /// a human at the keyboard.
    shot_path: Option<String>,
    frames: u64,
    /// Seconds of wall clock since the node came up, for the dev flags below.
    elapsed: f64,
    /// `--salt-shot-at <seconds>`: when to grab the screenshot.
    shot_at: f64,
    /// `--salt-auto`: fire a solver candidate at the nearest standing target
    /// and take every draft. Lets a build be watched, or captured, without a
    /// hand on the keyboard.
    auto: bool,
    auto_cooldown: f64,
    /// `--salt-miss`: throw the first shot of each screen away, so craters,
    /// chalk marks and the multi-shot path get exercised too.
    auto_miss: bool,
    /// `--salt-hold-draft`: leave the draft on screen instead of taking an
    /// offer, so the cards can be looked at.
    auto_hold_draft: bool,
}

#[godot_api]
impl INode2D for SaltLine {
    fn init(base: Base<Node2D>) -> Self {
        SaltLine {
            base,
            pal: Palette::default(),
            sprites: None,
            canvas: Canvas::new(320, 180),
            texture: None,
            buffer_w: 320,
            buffer_h: SCREEN_H as i32,
            cam_x: 0,
            cam_target: 0.0,
            mode: Mode::Title,
            run: None,
            anim: None,
            clear_hold: 0.0,
            angle_ticks: 900,
            power_ticks: 600,
            save: Save::default(),
            entitlement: Entitlement::Free,
            seed_text: String::new(),
            daily: true,
            last_seed: 0,
            status: String::new(),
            status_time: 0.0,
            debug: false,
            keys: Vec::new(),
            pad_edges: Vec::new(),
            pad_repeat: [RepeatState::default(); 8],
            controller_active: false,
            touch_buttons: Vec::new(),
            touch: TouchPad::default(),
            drag_from: None,
            last_arc: Vec::new(),
            touch_active: false,
            back_requested: false,
            mobile: false,
            corner_pad: (0, 0),
            letterbox: (0, 0),
            audio: Vec::new(),
            low_shell_warned: false,
            shot_path: None,
            frames: 0,
            elapsed: 0.0,
            shot_at: 0.05,
            auto: false,
            auto_cooldown: 0.0,
            auto_miss: false,
            auto_hold_draft: false,
        }
    }

    fn ready(&mut self) {
        // Phones and tablets start with touch controls; desktops show them only
        // once the screen is actually touched.
        self.mobile = Os::singleton().has_feature("mobile");
        self.touch_active = self.mobile;
        if self.mobile {
            // Rounded screen corners (Pixel 10: 138 px radius at 1080 px tall):
            // 10 buffer pixels in from the side and 6 down keeps HUD text and
            // buttons comfortably inside the curve without raising the buttons
            // off the ground.
            self.corner_pad = (10, 6);
            // What shows beside the playfield is a letterbox, not sky.
            RenderingServer::singleton().set_default_clear_color(Color::BLACK);
        }
        self.pal = Palette::load(PALETTE_PATH);
        self.sprites = Some(Sprites::load_all(SPRITE_DIR));
        self.save = save::load();
        self.setup_audio();
        self.entitlement = store::entitlement(self.save.unlocked);
        self.buffer_w = self.pick_width();
        self.apply_buffer_size();
        if let Some(dir) = Self::cmdline_value("--salt-gallery") {
            self.gallery(&dir);
            return;
        }
        self.shot_path = Self::screenshot_arg();
        self.shot_at = Self::cmdline_value("--salt-shot-at")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.05);
        self.auto = Self::cmdline_flag("--salt-auto");
        self.auto_miss = Self::cmdline_flag("--salt-miss");
        self.auto_hold_draft = Self::cmdline_flag("--salt-hold-draft");
        if let Some(seed) = Self::seed_arg() {
            self.start_run(seed, false);
            if let Some(n) = Self::screen_arg() {
                self.jump_to(n);
            }
        }
        if !self.pal.sampled {
            godot_warn!("salt-line: palette not sampled from art; using the built-in fallback");
        }
    }

    fn process(&mut self, delta: f64) {
        self.elapsed += delta;
        self.status_time = (self.status_time - delta).max(0.0);
        self.handle_input(delta);
        if self.auto {
            self.autopilot(delta);
        }
        self.step(delta);
        self.render();
        self.base_mut().queue_redraw();
    }

    fn draw(&mut self) {
        // The whole frame is one texture at the internal resolution, blitted
        // at 1:1. Godot's viewport stretch does the only scaling in the game,
        // with nearest filtering set project-wide.
        if let Some(tex) = self.texture.clone() {
            self.base_mut().draw_texture(&tex, Vector2::ZERO);
        }
    }

    fn input(&mut self, event: Gd<InputEvent>) {
        let event = match event.try_cast::<InputEventScreenDrag>() {
            Ok(drag) => {
                let local = self
                    .base()
                    .make_input_local(&drag.clone().upcast::<InputEvent>());
                let pos = local
                    .try_cast::<InputEventScreenDrag>()
                    .map(|d| d.get_position())
                    .unwrap_or_else(|_| drag.get_position());
                self.touch.finger_moved(drag.get_index(), pos.x, pos.y);
                return;
            }
            Err(event) => event,
        };
        let Ok(touch) = event.try_cast::<InputEventScreenTouch>() else {
            return;
        };
        self.touch_active = true;
        // The one texture is drawn at the origin at 1:1, so the node's local
        // space is buffer pixels whatever the window is scaled to.
        let local = self
            .base()
            .make_input_local(&touch.clone().upcast::<InputEvent>());
        let pos = local
            .try_cast::<InputEventScreenTouch>()
            .map(|t| t.get_position())
            .unwrap_or_else(|_| touch.get_position());
        if touch.is_pressed() {
            self.touch
                .finger_down(touch.get_index(), pos.x, pos.y, &self.touch_buttons);
        } else {
            self.touch.finger_up(touch.get_index());
        }
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::WM_GO_BACK_REQUEST => self.back_requested = true,
            CanvasItemNotification::WM_WINDOW_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_FOCUS_OUT => self.touch.release_all(),
            _ => {}
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if self.mode != Mode::SeedEntry {
            return;
        }
        let Ok(key) = event.try_cast::<InputEventKey>() else {
            return;
        };
        if !key.is_pressed() || key.is_echo() {
            return;
        }
        let unicode = key.get_unicode();
        match key.get_keycode() {
            Key::BACKSPACE => {
                self.seed_text.pop();
            }
            Key::ENTER | Key::KP_ENTER => self.start_seeded_run(),
            Key::ESCAPE => self.leave_seed_entry(),
            _ => {
                if (32..127).contains(&unicode) && self.seed_text.chars().count() < 20 {
                    self.seed_text.push(unicode as u8 as char);
                }
            }
        }
    }
}

impl SaltLine {
    // ------------------------------------------------------------- lifecycle

    /// Days since the Unix epoch. The clock is read here, in the presentation
    /// layer — nothing in the sim ever touches system time.
    fn start_seeded_run(&mut self) {
        let seed = if self.seed_text.trim().is_empty() {
            daily_seed(Self::today())
        } else {
            parse_seed(&self.seed_text)
        };
        self.start_run(seed, false);
    }

    fn leave_seed_entry(&mut self) {
        self.mode = Mode::Title;
    }

    fn today() -> u64 {
        let secs = Time::singleton().get_unix_time_from_system();
        (secs / 86400.0).floor().max(0.0) as u64
    }

    /// Internal buffer width. DESIGN §5.1: fix the height at 180, let the
    /// width float 320..400 by device aspect, and generate terrain to fill it.
    fn pick_width(&self) -> i32 {
        let size = DisplayServer::singleton().window_get_size();
        let (l, r) = self.cutout_px();
        let usable = size.x - l - r;
        let aspect = if size.y > 0 {
            usable as f64 / size.y as f64
        } else {
            16.0 / 9.0
        };
        let want = (SCREEN_H as f64 * aspect).round() as usize;
        buffer_width(want.clamp(SCREEN_W_MIN, SCREEN_W_MAX)) as i32
    }

    fn long_lens(&self) -> bool {
        self.run
            .as_ref()
            .map(|r| r.loadout.has_flag(flags::LONG_LENS))
            .unwrap_or(false)
    }

    fn apply_buffer_size(&mut self) {
        let h = if self.long_lens() {
            LONG_LENS_H
        } else {
            SCREEN_H as i32
        };
        self.buffer_h = h;
        self.canvas.resize(self.buffer_w, h);
        self.letterbox = self.letterbox_now();
        let (l, r) = self.letterbox;
        if let Some(mut window) = self.base().get_window() {
            window.set_content_scale_size(Vector2i::new(self.buffer_w + l + r, h));
        }
        self.base_mut().set_position(Vector2::new(l as f32, 0.0));
    }

    /// Screen pixels a display cutout takes from the left and right edges: a
    /// camera hole on a phone held landscape. Only asked on mobile, where the
    /// window is the whole display; on a desktop the "safe area" is the
    /// monitor's, not the window's.
    fn cutout_px(&self) -> (i32, i32) {
        if !self.mobile {
            return (0, 0);
        }
        let ds = DisplayServer::singleton();
        let win = ds.window_get_size();
        let safe = ds.get_display_safe_area();
        if safe.size.x <= 0 || safe.size.y <= 0 {
            return (0, 0);
        }
        let l = safe.position.x.clamp(0, win.x / 4);
        let r = (win.x - safe.position.x - safe.size.x).clamp(0, win.x / 4);
        (l, r)
    }

    /// The cutout in buffer pixels, rounded up so the playfield never reaches
    /// under it.
    fn letterbox_now(&self) -> (i32, i32) {
        let (l, r) = self.cutout_px();
        let win_h = DisplayServer::singleton().window_get_size().y;
        if win_h <= 0 || self.buffer_h <= 0 {
            return (0, 0);
        }
        let scale = win_h as f64 / self.buffer_h as f64;
        (
            (l as f64 / scale).ceil() as i32,
            (r as f64 / scale).ceil() as i32,
        )
    }

    fn start_run(&mut self, seed: u64, daily: bool) {
        // The world is as wide as the buffer, so the run is generated for it.
        let width = self.buffer_w as usize;
        let state = RunState::new(seed, width);
        self.angle_ticks = 900;
        self.power_ticks = 600;
        self.anim = None;
        self.last_arc.clear();
        self.clear_hold = 0.0;
        self.cam_x = 0;
        self.cam_target = 0.0;
        self.daily = daily;
        self.low_shell_warned = false;
        self.last_seed = seed;
        self.run = Some(state);
        self.mode = Mode::Playing;
        self.apply_buffer_size();
        self.note(format!("SEED {}", format_seed(seed)));
    }

    /// Level inspector: drop straight onto a screen index without playing the
    /// ones before it. DESIGN §2.2 is what makes this possible — each screen
    /// generates from its own seed, so screen 400 costs the same as screen 1.
    fn jump_to(&mut self, index: u32) {
        let Some(run) = self.run.as_mut() else { return };
        run.screen_index = index;
        run.screen = run::generate_screen(run.run_seed, index, run.width, &run.loadout);
        self.note(format!("SCREEN {}", index + 1));
    }

    fn note(&mut self, text: String) {
        self.status = text;
        self.status_time = 2.4;
    }

    // ----------------------------------------------------------------- input

    fn setup_audio(&mut self) {
        self.audio.clear();
        for sound in Sound::ALL {
            let path = format!("res://art/audio/{}.wav", sound.file());
            let Some(resource) = ResourceLoader::singleton().load(&path) else {
                godot_warn!("salt-line: missing procedural sound {path}");
                self.audio.push(None);
                continue;
            };
            let Ok(stream) = resource.try_cast::<AudioStream>() else {
                godot_warn!("salt-line: could not load sound {path}");
                self.audio.push(None);
                continue;
            };
            let mut player = AudioStreamPlayer::new_alloc();
            player.set_stream(&stream);
            self.base_mut().add_child(&player);
            self.audio.push(Some(player));
        }
        self.apply_volume();
    }

    fn apply_volume(&mut self) {
        let db = if self.save.muted || self.save.volume == 0 {
            -80.0
        } else {
            20.0 * (self.save.volume as f32 / 100.0).log10()
        };
        for player in self.audio.iter_mut().flatten() {
            player.set_volume_db(db);
        }
    }

    fn play_sound(&mut self, sound: Sound) {
        if let Some(player) = self.audio.get_mut(sound as usize).and_then(Option::as_mut) {
            player.play();
        }
    }

    fn learn(&mut self, bit: u32) {
        if self.save.learned & bit == 0 {
            self.save.learned |= bit;
            save::store(&self.save);
        }
    }

    fn binding(&self, action: &str, default_key: &str, default_pad: &str) -> (String, String) {
        self.save
            .bindings
            .iter()
            .find(|(a, _, _)| a == action)
            .map(|(_, k, p)| (k.clone(), p.clone()))
            .unwrap_or_else(|| (default_key.into(), default_pad.into()))
    }

    fn key_token(token: &str) -> Option<Key> {
        Some(match token {
            "LEFT" => Key::LEFT,
            "RIGHT" => Key::RIGHT,
            "UP" => Key::UP,
            "DOWN" => Key::DOWN,
            "SPACE" => Key::SPACE,
            "ENTER" => Key::ENTER,
            "ESCAPE" => Key::ESCAPE,
            "LBRACKET" => Key::BRACKETLEFT,
            "RBRACKET" => Key::BRACKETRIGHT,
            "COMMA" => Key::COMMA,
            "PERIOD" => Key::PERIOD,
            "1" => Key::KEY_1,
            "2" => Key::KEY_2,
            "3" => Key::KEY_3,
            "C" => Key::C,
            "R" => Key::R,
            "S" => Key::S,
            "V" => Key::V,
            "M" => Key::M,
            "L" => Key::L,
            _ => return None,
        })
    }

    fn pad_token(token: &str) -> Option<JoyButton> {
        Some(match token {
            "A" => JoyButton::A,
            "B" => JoyButton::B,
            "X" => JoyButton::X,
            "Y" => JoyButton::Y,
            "START" => JoyButton::START,
            "BACK" => JoyButton::BACK,
            "LB" => JoyButton::LEFT_SHOULDER,
            "RB" => JoyButton::RIGHT_SHOULDER,
            "DPAD_LEFT" => JoyButton::DPAD_LEFT,
            "DPAD_RIGHT" => JoyButton::DPAD_RIGHT,
            "DPAD_UP" => JoyButton::DPAD_UP,
            "DPAD_DOWN" => JoyButton::DPAD_DOWN,
            _ => return None,
        })
    }

    fn action_binding(&self, action: &str) -> (String, String) {
        let defaults = match action {
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
            _ => ("", ""),
        };
        self.binding(action, defaults.0, defaults.1)
    }

    fn pad_pressed(&mut self, button: JoyButton) -> bool {
        let slot = if let Some(i) = self.pad_edges.iter().position(|(b, _)| *b == button) {
            i
        } else {
            self.pad_edges.push((button, false));
            self.pad_edges.len() - 1
        };
        let down = Input::singleton().is_joy_button_pressed(0, button);
        let was = self.pad_edges[slot].1;
        self.pad_edges[slot].1 = down;
        if down {
            self.controller_active = true;
            self.touch_active = false;
        }
        down && !was
    }

    fn action_pressed(&mut self, _slot: usize, action: &str) -> bool {
        let (key, pad) = self.action_binding(action);
        let key_down = Self::key_token(&key)
            .map(|k| self.pressed(k))
            .unwrap_or(false);
        if key_down {
            self.controller_active = false;
            self.touch_active = false;
        }
        let pad_down = Self::pad_token(&pad)
            .map(|b| self.pad_pressed(b))
            .unwrap_or(false);
        let touch_down = self.touch.pressed(action) || (action == "back" && self.back_requested);
        key_down || pad_down || touch_down
    }

    fn action_repeated(&mut self, slot: usize, action: &str, delta: f64) -> u32 {
        let (key, pad) = self.action_binding(action);
        let keyboard = Self::key_token(&key)
            .map(|k| self.repeated(k, delta))
            .unwrap_or(0);
        let mut pad_down = Self::pad_token(&pad)
            .map(|b| Input::singleton().is_joy_button_pressed(0, b))
            .unwrap_or(false);
        // Left stick is always available in addition to remappable D-pad.
        let axis = match action {
            "left" => Some((JoyAxis::LEFT_X, -1.0)),
            "right" => Some((JoyAxis::LEFT_X, 1.0)),
            "up" => Some((JoyAxis::LEFT_Y, -1.0)),
            "down" => Some((JoyAxis::LEFT_Y, 1.0)),
            _ => None,
        };
        if let Some((axis, sign)) = axis {
            pad_down |= Input::singleton().get_joy_axis(0, axis) * sign > 0.45;
        }
        if keyboard > 0 {
            self.controller_active = false;
            self.touch_active = false;
        }
        if pad_down {
            self.controller_active = true;
            self.touch_active = false;
        }
        keyboard
            + self.pad_repeat[slot].tick(pad_down, delta, REPEAT_DELAY, REPEAT_RATE)
            + self.touch.repeated(action)
    }

    fn input_label(&self, action: &str) -> String {
        if self.touch_active {
            return touch_label(action).to_string();
        }
        let (key, pad) = self.action_binding(action);
        if self.controller_active {
            pad
        } else {
            key
        }
    }

    fn key_slot(&mut self, key: Key) -> usize {
        if let Some(i) = self.keys.iter().position(|(k, _, _)| *k == key) {
            i
        } else {
            self.keys.push((key, false, 0.0));
            self.keys.len() - 1
        }
    }

    /// True on the frame a key goes down.
    fn pressed(&mut self, key: Key) -> bool {
        let down = Input::singleton().is_key_pressed(key);
        let i = self.key_slot(key);
        let was = self.keys[i].1;
        self.keys[i].1 = down;
        if !down {
            self.keys[i].2 = 0.0;
        }
        down && !was
    }

    /// Number of activations this frame, with hold-to-repeat.
    fn repeated(&mut self, key: Key, delta: f64) -> u32 {
        let down = Input::singleton().is_key_pressed(key);
        let i = self.key_slot(key);
        let was = self.keys[i].1;
        self.keys[i].1 = down;
        if !down {
            self.keys[i].2 = 0.0;
            return 0;
        }
        if !was {
            self.keys[i].2 = -REPEAT_DELAY;
            return 1;
        }
        self.keys[i].2 += delta;
        let mut n = 0;
        while self.keys[i].2 >= REPEAT_RATE {
            self.keys[i].2 -= REPEAT_RATE;
            n += 1;
            if n > 20 {
                break;
            }
        }
        n
    }

    fn shift_held(&self) -> bool {
        let input = Input::singleton();
        input.is_key_pressed(Key::SHIFT)
    }

    fn handle_input(&mut self, delta: f64) {
        self.touch.update(delta, REPEAT_DELAY, REPEAT_RATE);
        self.handle_mode_input(delta);
        self.back_requested = false;
    }

    fn handle_mode_input(&mut self, delta: f64) {
        if self.pressed(Key::F1) {
            self.debug = !self.debug;
        }
        if self.action_pressed(15, "mute") {
            self.save.muted = !self.save.muted;
            self.apply_volume();
            save::store(&self.save);
            self.note(if self.save.muted {
                "SOUND MUTED".into()
            } else {
                "SOUND ON".into()
            });
        }
        match self.mode {
            Mode::Title => {
                if self.action_pressed(0, "confirm") || self.pressed(Key::KP_ENTER) {
                    let seed = daily_seed(Self::today());
                    self.play_sound(Sound::Ui);
                    self.start_run(seed, true);
                } else if self.pressed(Key::S) || self.touch.pressed("seed") {
                    self.seed_text.clear();
                    self.mode = Mode::SeedEntry;
                } else if self.action_pressed(1, "controls") {
                    self.play_sound(Sound::Ui);
                    self.mode = Mode::Controls;
                } else if self.action_pressed(2, "back") {
                    self.base().get_tree().quit();
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
                    save::store(&self.save);
                    self.play_sound(Sound::Ui);
                }
                if self.touch.pressed("aim_mode") {
                    self.save.touch_aim = Some(self.touch_aim().next());
                    save::store(&self.save);
                    self.play_sound(Sound::Ui);
                } else if self.touch.pressed("arc_hint") {
                    self.save.arc_hint = Some(self.arc_hint().next());
                    save::store(&self.save);
                    self.play_sound(Sound::Ui);
                }
                if self.action_pressed(16, "last_arc") {
                    self.save.last_arc = Some(!self.last_arc_on());
                    save::store(&self.save);
                    self.play_sound(Sound::Ui);
                }
                if self.action_pressed(3, "replay") {
                    self.save.bindings.clear();
                    save::store(&self.save);
                    self.note("DEFAULT BINDINGS RESTORED".into());
                } else if self.action_pressed(2, "back") {
                    self.play_sound(Sound::Ui);
                    self.mode = Mode::Title;
                }
            }
            Mode::Paused => {
                if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.play_sound(Sound::Ui);
                    self.mode = Mode::Playing;
                } else if self.action_pressed(3, "replay") {
                    self.play_sound(Sound::Ui);
                    self.mode = Mode::Title;
                }
            }
            Mode::SeedEntry => {
                // Keyboard text comes in through `unhandled_input`; these are
                // the touch buttons and the on-screen keypad.
                if self.touch.pressed("confirm") {
                    self.start_seeded_run();
                } else if self.action_pressed(2, "back") {
                    self.leave_seed_entry();
                } else if self.touch.pressed("del") {
                    self.seed_text.pop();
                } else if let Some(k) = SEED_KEYS.iter().find(|k| self.touch.pressed(k)) {
                    if self.seed_text.chars().count() < 20 {
                        self.seed_text.push_str(k);
                    }
                }
            }
            Mode::Paywall => {
                if self.pressed(Key::U) {
                    self.unlock(false);
                } else if self.pressed(Key::R) {
                    self.unlock(true);
                } else if self.pressed(Key::ENTER) || self.pressed(Key::ESCAPE) {
                    self.finish_run();
                }
            }
            Mode::GameOver => {
                if self.action_pressed(4, "copy") {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.last_seed);
                    DisplayServer::singleton().clipboard_set(&GString::from(seed.as_str()));
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
                        self.learn(LEARN_DRAFT);
                        self.play_sound(Sound::Draft);
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

        if self.action_pressed(10, "fire") || self.pressed(Key::ENTER) {
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

    fn last_arc_on(&self) -> bool {
        self.save.last_arc.unwrap_or(DEFAULT_LAST_ARC)
    }

    fn touch_aim(&self) -> TouchAim {
        self.save.touch_aim.unwrap_or(DEFAULT_TOUCH_AIM)
    }

    fn arc_hint(&self) -> ArcHint {
        self.save.arc_hint.unwrap_or(DEFAULT_ARC_HINT)
    }

    /// Touch aiming is by dragging rather than the angle and power buttons.
    fn drag_aiming(&self) -> bool {
        self.touch_active && self.touch_aim() == TouchAim::Drag
    }

    // ------------------------------------------------------------- run steps

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
        self.play_sound(Sound::Fire);
        self.play_sound(Sound::Flight);
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
            self.play_sound(Sound::LowShell);
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
            self.play_sound(sound);
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
            self.play_sound(Sound::Draft);
        }
        // The paywall gate. Brief M6: free to play through screen 15.
        let index = self.run.as_ref().map(|r| r.screen_index).unwrap_or(0);
        if self.entitlement == Entitlement::Free && index >= FREE_SCREENS {
            self.mode = Mode::Paywall;
        }
    }

    /// Ask the platform to sell or restore the unlock. This is the only path
    /// that may grant it; there is deliberately no local override, because a
    /// key that unlocks a paid build is a key that ships in the paid build.
    fn unlock(&mut self, restore: bool) {
        let outcome = if restore {
            store::restore()
        } else {
            store::purchase()
        };
        match outcome {
            store::Outcome::Granted => {
                // Cache it so the title screen is right before the next
                // platform round trip answers.
                self.save.unlocked = true;
                save::store(&self.save);
                self.entitlement = Entitlement::Full;
                self.mode = Mode::Playing;
                self.note("UNLOCKED".into());
            }
            store::Outcome::Cancelled => self.note("CANCELLED".into()),
            store::Outcome::Unavailable(why) => self.note(why.into()),
        }
    }

    fn finish_run(&mut self) {
        if let Some(run) = self.run.as_ref() {
            let (screens, shells, shots) = run.score();
            let entry = Entry {
                seed: run.run_seed,
                screens,
                shells,
                shots,
                daily: self.daily,
            };
            save::record(&mut self.save, entry);
        }
        self.mode = Mode::GameOver;
    }

    fn world_width(&self) -> i32 {
        self.run
            .as_ref()
            .map(|r| r.width as i32)
            .unwrap_or(self.buffer_w)
    }

    /// The lead shell's position in the replay, if one is in the air.
    fn shell_head(&self) -> Option<Vec2> {
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
    fn shown_screen(&self) -> Option<&Screen> {
        match (self.anim.as_ref(), self.run.as_ref()) {
            (Some(anim), _) if !anim.done() => Some(&anim.before),
            (_, Some(run)) => Some(&run.screen),
            _ => None,
        }
    }

    // --------------------------------------------------------------- drawing

    fn render(&mut self) {
        self.touch_buttons.clear();
        let want_w = self.pick_width();
        if self.run.is_none() && want_w != self.buffer_w {
            self.buffer_w = want_w;
            self.apply_buffer_size();
        } else if self.letterbox_now() != self.letterbox {
            self.apply_buffer_size();
        }
        match self.mode {
            Mode::Playing | Mode::Paused | Mode::Paywall => self.render_play(),
            _ => self.render_menu(),
        }

        self.upload();
        self.maybe_screenshot();
    }

    fn upload(&mut self) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let data = self.canvas.to_packed();
        let Some(image) =
            Image::create_from_data(w, h, false, godot::classes::image::Format::RGBA8, &data)
        else {
            return;
        };
        let reusable = self
            .texture
            .as_ref()
            .is_some_and(|t| t.get_width() == w && t.get_height() == h);
        if reusable {
            if let Some(t) = self.texture.as_mut() {
                t.update(&image);
            }
        } else {
            self.texture = ImageTexture::create_from_image(&image);
        }
    }

    /// `--salt-shot <path>` support: save the buffer and quit.
    fn maybe_screenshot(&mut self) {
        self.frames += 1;
        let Some(path) = self.shot_path.clone() else {
            return;
        };
        // Wait for the moment asked for, so a shot in flight or a draft can be
        // captured rather than only the first frame.
        if self.frames < 3 || self.elapsed < self.shot_at {
            return;
        }
        let (w, h) = (self.canvas.w, self.canvas.h);
        let data = self.canvas.to_packed();
        if let Some(image) =
            Image::create_from_data(w, h, false, godot::classes::image::Format::RGBA8, &data)
        {
            let err = image.save_png(&GString::from(path.as_str()));
            godot_print!("salt-line: wrote {path} ({w}x{h}) -> {err:?}");
        }
        self.shot_path = None;
        self.base().get_tree().quit();
    }

    /// Play the game for the camera: aim with the solver, take offer 1.
    fn autopilot(&mut self, delta: f64) {
        self.auto_cooldown -= delta;
        if self.mode == Mode::Title {
            let seed = Self::seed_arg().unwrap_or_else(|| daily_seed(Self::today()));
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

    /// Every argument, whether it arrived before or after `--`.
    fn cmdline_all() -> Vec<String> {
        let os = Os::singleton();
        let mut joined: Vec<String> = os
            .get_cmdline_user_args()
            .as_slice()
            .iter()
            .map(|s| s.to_string())
            .collect();
        joined.extend(
            os.get_cmdline_args()
                .as_slice()
                .iter()
                .map(|s| s.to_string()),
        );
        joined
    }

    /// Presence of a valueless flag.
    fn cmdline_flag(flag: &str) -> bool {
        Self::cmdline_all().iter().any(|a| a == flag)
    }

    fn cmdline_value(flag: &str) -> Option<String> {
        let joined = Self::cmdline_all();
        let mut it = joined.iter();
        while let Some(a) = it.next() {
            if a == flag {
                return it.next().cloned();
            }
            if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
                return Some(rest.to_string());
            }
        }
        None
    }

    fn screenshot_arg() -> Option<String> {
        Self::cmdline_value("--salt-shot")
    }

    /// `--salt-seed <seed>`: start a run immediately, for screenshots and for
    /// looking at a specific screen.
    fn seed_arg() -> Option<u64> {
        Self::cmdline_value("--salt-seed").map(|s| parse_seed(&s))
    }

    /// `--salt-screen <n>`: jump the run forward, for inspecting late bands.
    fn screen_arg() -> Option<u32> {
        Self::cmdline_value("--salt-screen").and_then(|s| s.parse().ok())
    }

    /// Write the buffer to a PNG at native resolution.
    fn save_canvas(&self, path: &str) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let data = self.canvas.to_packed();
        if let Some(image) =
            Image::create_from_data(w, h, false, godot::classes::image::Format::RGBA8, &data)
        {
            let err = image.save_png(&GString::from(path));
            godot_print!("salt-line: wrote {path} ({w}x{h}) -> {err:?}");
        }
    }

    /// `--salt-gallery <dir>`: render one frame per shape template, plus a
    /// damaged variant of each, and quit.
    ///
    /// This is the art-review and tuning tool. Change a terrain constant, run
    /// the gallery, look at seven pictures. Beats playing until the template
    /// you changed happens to come up.
    fn gallery(&mut self, dir: &str) {
        // Airburst is held so `Overhang` is on the table at all (DESIGN §2.4).
        let mut lo = Loadout::new();
        lo.take(Modifier::Airburst);
        let width = self.buffer_w as usize;
        let mut run = RunState::new(0, width);
        run.loadout = lo.clone();

        for t in salt_line_sim::terrain::Template::ALL {
            let index = t.min_band().max(1) * salt_line_sim::BAND_LEN;
            let mut found = None;
            for seed in 0..6000u64 {
                let sc = run::generate_screen(seed, index, width, &lo);
                if sc.template == t {
                    found = Some(sc);
                    break;
                }
            }
            let Some(screen) = found else {
                godot_warn!("salt-line: no {} screen found", t.name());
                continue;
            };

            // Clean.
            run.screen = screen.clone();
            run.screen_index = index;
            self.run = Some(run.clone());
            self.mode = Mode::Playing;
            self.status_time = 0.0;
            self.render_play();
            self.save_canvas(&format!("{dir}/template_{}.png", t.name().to_lowercase()));

            // Damaged: undermine the first target's footprint and lob a couple
            // of craters short of it, so craters, rims, lean and rubble all
            // appear in one picture.
            let mut damaged = screen.clone();
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
            self.run = Some(run.clone());
            self.render_play();
            self.save_canvas(&format!("{dir}/damaged_{}.png", t.name().to_lowercase()));
        }
        self.base().get_tree().quit();
    }

    fn view(&self) -> View {
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

    fn render_menu(&mut self) {
        let pal = self.pal.clone();
        let view = self.view();
        // A calm sky behind the menus, with the same dither as the game.
        raster::draw_sky(&mut self.canvas, &view, &pal);
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let w = self.canvas.w;
        let mut y = 26;

        // The title gets wider tracking, not a bigger face. DESIGN §5.6:
        // resist the urge to make UI larger and smoother than the game.
        let title = "SALT LINE";
        let tracking = 6;
        let tw = title.chars().count() as i32 * tracking;
        let tx = (w - tw) / 2;
        for (i, ch) in title.chars().enumerate() {
            let s = ch.to_string();
            self.canvas
                .text_small(tx + i as i32 * tracking, y, &s, alert);
        }
        y += 12;

        match self.mode {
            Mode::Title => {
                self.centred(y, "ARTILLERY ROGUELIKE", dim);
                y += 14;
                let confirm = self.input_label("confirm");
                y = self.menu_row(
                    y,
                    "confirm",
                    &format!("{confirm}  DAILY RUN"),
                    "DAILY RUN",
                    fg,
                );
                y = self.menu_row(y, "seed", "S      SEED ENTRY", "SEED ENTRY", fg);
                let controls = self.input_label("controls");
                y = self.menu_row(
                    y,
                    "controls",
                    &format!("{controls}  CONTROLS / AUDIO"),
                    "CONTROLS / AUDIO",
                    fg,
                );
                let back = self.input_label("back");
                y = self.menu_row(y, "back", &format!("{back}  QUIT"), "QUIT", dim);
                y += 6;
                let best = self.save.best().copied();
                if let Some(b) = best {
                    self.centred(
                        y,
                        &format!("BEST {} SCREENS  SEED {}", b.screens, b.seed),
                        alert,
                    );
                    y += 10;
                }
                if !store::MONETIZED {
                    // The build is not selling anything, and says so where a
                    // player will see it before they start a run.
                    self.centred(y, "DEVELOPMENT PREVIEW - NOT FOR SALE", alert);
                    y += 8;
                } else if self.entitlement == Entitlement::Free {
                    self.centred(y, &format!("FREE THROUGH SCREEN {FREE_SCREENS}"), dim);
                    y += 8;
                }
                let room = ((self.canvas.h - y - 12) / 7).max(0) as usize;
                let n = self.save.scores.len().min(5).min(room);
                if n > 0 {
                    y += 4;
                    self.centred(y, "LOCAL BOARD", dim);
                    y += 8;
                    for i in 0..n {
                        let e = self.save.scores[i];
                        let line = format!(
                            "{}. {:>3} SCREENS  {:>3} SHELLS  {}",
                            i + 1,
                            e.screens,
                            e.shells,
                            if e.daily { "DAILY" } else { "SEED" }
                        );
                        self.centred(y, &line, fg);
                        y += 7;
                    }
                }
            }
            Mode::Controls if self.touch_active => {
                self.centred(y, "CONTROLS / AUDIO", fg);
                y += 16;
                let muted = if self.save.muted { " MUTED" } else { "" };
                let text = format!("VOLUME {}%{muted}", self.save.volume);
                let tw = crate::font::small_width(&text) as i32;
                let tx = (w - tw) / 2;
                self.canvas.text_small(tx, y, &text, alert);
                self.draw_touch_button(tx - 26, y - 6, 20, 17, "-", "left");
                self.draw_touch_button(tx + tw + 6, y - 6, 20, 17, "+", "right");
                y += 20;
                let mute = if self.save.muted { "UNMUTE" } else { "MUTE" };
                y = self.menu_row(y, "mute", "", mute, fg);
                let last = if self.last_arc_on() {
                    "LAST ARC: ON"
                } else {
                    "LAST ARC: OFF"
                };
                y = self.menu_row(y, "last_arc", "", last, fg);
                let aim = match self.touch_aim() {
                    TouchAim::Buttons => "AIM: BUTTONS",
                    TouchAim::Drag => "AIM: DRAG",
                };
                y = self.menu_row(y, "aim_mode", "", aim, fg);
                if self.touch_aim() == TouchAim::Drag {
                    let arc = match self.arc_hint() {
                        ArcHint::Short => "ARC: SHORT",
                        ArcHint::Full => "ARC: FULL",
                        ArcHint::Wind => "ARC: FULL + WIND",
                    };
                    y = self.menu_row(y, "arc_hint", "", arc, fg);
                    y += 4;
                    self.centred(y, "IN PLAY: PULL BACK, LET GO TO FIRE", fg);
                } else {
                    y += 4;
                    self.centred(y, "IN PLAY: < > ANGLE   - + POWER", fg);
                }
                y += 12;
                self.menu_row(y, "back", "", "BACK", fg);
            }
            Mode::Controls => {
                self.centred(y, "CONTROLS / AUDIO", fg);
                y += 12;
                let muted = if self.save.muted { " MUTED" } else { "" };
                self.centred(
                    y,
                    &format!("< >  VOLUME {}%{muted}", self.save.volume),
                    alert,
                );
                y += 10;
                let rows = [
                    ("AIM / POWER", "left", "up"),
                    ("FIRE", "fire", ""),
                    ("FUSE", "fuse_down", "fuse_up"),
                    ("REPOSITION", "reposition_left", "reposition_right"),
                    ("DRAFT", "draft_1", "draft_2"),
                    ("BACK", "back", ""),
                ];
                for (name, a, b) in rows {
                    let mut labels = self.input_label(a);
                    if !b.is_empty() {
                        labels.push('/');
                        labels.push_str(&self.input_label(b));
                    }
                    self.centred(y, &format!("{name}: {labels}"), fg);
                    y += 7;
                }
                y += 4;
                self.centred(y, "M / START  MUTE     R / Y  RESET", dim);
                y += 7;
                let on = if self.last_arc_on() { "ON" } else { "OFF" };
                let toggle = format!("{}  LAST ARC: {on}", self.input_label("last_arc"));
                self.centred(y, &toggle, fg);
                y += 9;
                self.centred(y, "EDIT bind LINES IN salt_line.txt TO REMAP", dim);
                y += 7;
                self.centred(y, "FORMAT: bind ACTION KEY PAD", dim);
                y += 9;
                let back = self.input_label("back");
                self.centred(y, &format!("{back}  BACK"), fg);
            }
            Mode::SeedEntry => {
                self.centred(y, "ENTER A SEED", dim);
                y += 14;
                let text = self.seed_text.clone();
                let shown = format!("> {text}");
                self.centred(y, &shown, fg);
                y += 10;
                if self.touch_active {
                    y = self.draw_seed_keypad(y) + 6;
                    y = self.menu_row(y, "confirm", "", "START", fg);
                    y = self.menu_row(y, "back", "", "BACK", dim);
                } else {
                    y += 4;
                    self.centred(y, "ANY TEXT WORKS. BLANK USES TODAY'S DAILY.", dim);
                    y += 8;
                    self.centred(y, "ENTER  START     ESC  BACK", fg);
                    y += 14;
                }
                if !text.trim().is_empty() {
                    let seed = parse_seed(&text);
                    self.centred(y, &format!("SEED {}", format_seed(seed)), alert);
                    y += 8;
                    let prior = self.save.best_on(seed).copied();
                    if let Some(p) = prior {
                        self.centred(y, &format!("YOUR BEST HERE {} SCREENS", p.screens), dim);
                    }
                }
            }
            Mode::GameOver => {
                self.centred(y, "OUT OF SHELLS", alert);
                y += 14;
                if let Some(run) = self.run.as_ref() {
                    let (screens, shells, shots) = run.score();
                    let seed = run.run_seed;
                    self.centred(y, &format!("{screens} SCREENS CLEARED"), fg);
                    y += 8;
                    self.centred(y, &format!("{shots} SHOTS   {shells} SHELLS LEFT"), dim);
                    y += 8;
                    self.centred(y, &format!("SEED {}", format_seed(seed)), dim);
                    y += 14;
                }
                y = self.menu_row(y, "replay", "R      SAME SEED AGAIN", "SAME SEED AGAIN", fg);
                y = self.menu_row(y, "copy", "C      COPY SEED", "COPY SEED", fg);
                y = self.menu_row(y, "confirm", "ENTER  TITLE", "TITLE", fg);
                y += 4;
                if self.status_time > 0.0 {
                    let text = self.status.clone();
                    self.centred(y, &text, alert);
                }
            }
            _ => {}
        }
    }

    fn centred(&mut self, y: i32, text: &str, c: crate::palette::Rgba) {
        let x = (self.canvas.w - crate::font::small_width(text) as i32) / 2;
        self.canvas.text_small(x.max(1), y, text, c);
    }

    // ---------------------------------------------------------------- touch

    /// Make a rectangle of this frame's buffer a button for the next frame's
    /// touches. Only called while touch controls are showing.
    fn touch_button(&mut self, rect: Rect, action: &'static str) {
        self.touch_buttons.push(Button { rect, action });
    }

    /// A plate with a glyph on it, lit while a finger is on it.
    fn draw_touch_button(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        label: &str,
        action: &'static str,
    ) {
        let pal = self.pal.clone();
        self.canvas.plate(x, y, w, h, &pal);
        let c = if self.touch.held(action) {
            pal.get(slot::ALERT)
        } else {
            pal.get(slot::UI_FG)
        };
        let tx = x + (w - crate::font::small_width(label) as i32) / 2;
        let ty = y + (h - crate::font::SMALL_H as i32) / 2;
        self.canvas.text_small(tx, ty, label, c);
        self.touch_button(Rect::new(x, y, w, h), action);
    }

    /// The seed keypad: letters and digits in the game's own font, since a seed
    /// is any text and the platform keyboard is not dependable inside a game
    /// view. Returns the y below it.
    fn draw_seed_keypad(&mut self, top: i32) -> i32 {
        let (kw, kh, pitch) = (18, 15, 20);
        let rows = [&SEED_KEYS[..13], &SEED_KEYS[13..26], &SEED_KEYS[26..]];
        let mut y = top;
        for (r, keys) in rows.into_iter().enumerate() {
            let del_w = if r == 2 { 36 + 2 } else { 0 };
            let row_w = keys.len() as i32 * pitch - 2 + del_w;
            let mut x = (self.canvas.w - row_w) / 2;
            for &k in keys {
                self.draw_touch_button(x, y, kw, kh, k, k);
                x += pitch;
            }
            if r == 2 {
                self.draw_touch_button(x, y, 36, kh, "DEL", "del");
            }
            y += kh + 2;
        }
        y
    }

    /// A menu line. With a keyboard or controller it reads "KEY  TEXT"; on a
    /// touchscreen the line itself is the button, spaced for a finger rather
    /// than a key. Returns the y of the next line.
    fn menu_row(
        &mut self,
        y: i32,
        action: &'static str,
        keyed: &str,
        text: &str,
        c: crate::palette::Rgba,
    ) -> i32 {
        if !self.touch_active {
            self.centred(y, keyed, c);
            return y + 8;
        }
        let pal = self.pal.clone();
        let tw = crate::font::small_width(text) as i32;
        let pad = 10;
        let x = (self.canvas.w - tw) / 2 - pad;
        self.canvas.plate(x, y - 4, tw + 2 * pad, 13, &pal);
        let c = if self.touch.held(action) {
            pal.get(slot::ALERT)
        } else {
            c
        };
        self.canvas.text_small(x + pad, y, text, c);
        // The target is a little taller than the plate: rows are 16 apart.
        self.touch_button(Rect::new(x, y - 5, tw + 2 * pad, 15), action);
        y + 16
    }

    /// Aim, power and fire along the bottom edge, which is always solid ground,
    /// never sky, targets or the emplacement. Fuse and reposition sit above
    /// them only while a modifier provides them. The pause button is top centre,
    /// clear of the wind and shells plates.
    fn draw_play_controls(&mut self, loadout: &Loadout) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let (ex, ey) = self.corner_pad;
        let (bw, bh) = (22, 18);
        let y = h - bh - 3 - ey;
        // Right of the ANG/PWR readouts in the bottom-left corner.
        let x0 = 44 + ex;
        // Drag aiming: the open screen is the control, so only the modifier
        // buttons stay, dropped to the bottom row.
        let y2 = if self.drag_aiming() {
            y
        } else {
            self.draw_touch_button(x0, y, bw, bh, "<", "left");
            self.draw_touch_button(x0 + bw + 3, y, bw, bh, ">", "right");
            self.draw_touch_button(x0 + 2 * bw + 12, y, bw, bh, "-", "down");
            self.draw_touch_button(x0 + 3 * bw + 15, y, bw, bh, "+", "up");
            // Left of the screen index in the bottom-right corner.
            self.draw_touch_button(w - 16 - 40 - ex, y, 40, bh, "FIRE", "fire");
            y - bh - 3
        };
        if loadout.airburst() {
            self.draw_touch_button(x0, y2, bw, bh, "F-", "fuse_down");
            self.draw_touch_button(x0 + bw + 3, y2, bw, bh, "F+", "fuse_up");
        }
        if loadout.enfilade_units > 0.0 {
            self.draw_touch_button(x0 + 2 * bw + 12, y2, bw, bh, "E<", "reposition_left");
            self.draw_touch_button(x0 + 3 * bw + 15, y2, bw, bh, "E>", "reposition_right");
        }

        self.draw_touch_button((w - 20) / 2, 3 + ey, 20, 13, "II", "back");
    }

    fn render_play(&mut self) {
        let pal = self.pal.clone();
        let view = self.view();
        let Some(screen) = self.shown_screen().cloned() else {
            return;
        };
        let Some(run) = self.run.as_ref() else { return };
        let loadout = run.loadout.clone();
        let chalk = run.chalk.clone();
        let apex = run.last_apex;
        let phase = run.phase;
        let shells = run.shells;
        let index = run.screen_index;
        let wind_shown = run.displayed_wind();
        let fuse = run.fuse;
        let offers = run.offers.clone();
        let range = screen.range_to_nearest();
        let sappers = loadout.has_flag(flags::SAPPERS_EYE);

        raster::draw_sky(&mut self.canvas, &view, &pal);
        raster::draw_terrain(&mut self.canvas, &view, &screen.field, &pal);
        if sappers {
            raster::draw_material_boundaries(&mut self.canvas, &view, &screen.field, &pal);
        }
        if !chalk.is_empty() {
            raster::draw_chalk(&mut self.canvas, &view, &chalk, &screen.field, &pal);
        }
        if loadout.has_flag(flags::PLUMB_LINE) {
            if let Some(a) = apex {
                raster::draw_plumb(&mut self.canvas, &view, a, &pal);
            }
        }

        // Under the structures and emplacement, so it never hides either.
        if self.anim.is_none() && apex.is_some() && self.last_arc_on() {
            raster::draw_last_arc(&mut self.canvas, &view, &self.last_arc, &pal);
        }

        self.draw_structures(&screen, &view);
        self.draw_player(&screen, &loadout, &view);

        // The shot in flight.
        if let Some(anim) = self.anim.as_ref() {
            let upto = (anim.t / DT).floor().max(0.0) as usize;
            let traces: Vec<&Trace> = anim.traces.iter().collect();
            for t in traces {
                if t.path.is_empty() {
                    continue;
                }
                raster::draw_trajectory(&mut self.canvas, &view, &t.path, upto, &pal);
            }
        }
        self.draw_shot_effects(&view);
        if self.anim.is_none() && phase == Phase::Firing && self.mode == Mode::Playing {
            self.draw_drag_preview(&screen, &loadout, wind_shown, &view);
        }

        self.render_hud(
            shells, index, wind_shown, range, fuse, &loadout, &screen, phase,
        );
        if self.touch_active && self.mode == Mode::Playing && phase != Phase::Drafting {
            self.draw_play_controls(&loadout);
        }

        if phase == Phase::Drafting {
            self.render_draft(&offers, &loadout);
        }
        if self.mode == Mode::Paywall {
            self.render_paywall();
        } else if self.mode == Mode::Paused {
            self.render_pause();
        }
    }

    /// While a drag aims: the band from where the finger went down to where it
    /// is, and the arc the player chose to see. The wind in a Wind preview is
    /// the wind the HUD shows, so it never tells more than Dead Reckoning does.
    fn draw_drag_preview(
        &mut self,
        screen: &Screen,
        loadout: &Loadout,
        wind_shown: Option<f64>,
        view: &View,
    ) {
        if !self.drag_aiming() {
            return;
        }
        let Some((start, now)) = self.touch.drag() else {
            return;
        };
        let pal = self.pal.clone();
        let (sx, sy) = (start.0 as i32, start.1 as i32);
        let (nx, ny) = (now.0 as i32, now.1 as i32);
        let steps = (nx - sx).abs().max((ny - sy).abs()).max(1);
        for i in (0..=steps).step_by(3) {
            let x = sx + (nx - sx) * i / steps;
            let y = sy + (ny - sy) * i / steps;
            self.canvas.put(x, y, pal.get(slot::SKY_LO));
        }
        self.canvas
            .rect(sx - 1, sy - 1, 3, 3, pal.get(slot::SKY_LO));
        if self.drag_from.is_none()
            || crate::aim::pull_to_aim(start, now, loadout.angle_quantum_ticks).is_none()
        {
            return;
        }

        let hint = self.arc_hint();
        let boxes = screen.target_boxes();
        let mut world = screen.world(&boxes);
        world.wind = match hint {
            ArcHint::Wind => wind_shown.unwrap_or(0.0),
            ArcHint::Short | ArcHint::Full => 0.0,
        };
        let opts = FireOptions {
            record_path: true,
            ..FireOptions::default()
        };
        let shot = Shot::new(self.angle_ticks, self.power_ticks);
        let traces = ballistics::fire(&world, screen.muzzle(), shot, loadout, &opts);
        let Some(path) = traces.first().map(|t| &t.path) else {
            return;
        };
        let upto = match hint {
            ArcHint::Short => {
                let apex = path
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.y.total_cmp(&b.1.y))
                    .map_or(0, |(i, _)| i);
                let window = (SHORT_ARC_SECONDS / DT) as usize;
                crate::aim::short_arc_len(apex, path.len(), window)
            }
            ArcHint::Full | ArcHint::Wind => path.len(),
        };
        raster::draw_trajectory(&mut self.canvas, view, path, upto, &pal);
    }

    fn draw_structures(&mut self, screen: &Screen, view: &View) {
        let toppling = self
            .anim
            .as_ref()
            .map(|a| a.done() && a.settled < TOPPLE_TIME && !a.collapsed.is_empty())
            .unwrap_or(false);
        let collapsed_now: Vec<usize> = self
            .anim
            .as_ref()
            .map(|a| a.collapsed.clone())
            .unwrap_or_default();
        let pal = self.pal.clone();

        for (i, st) in screen.structures.iter().enumerate() {
            // While a structure is toppling, keep it standing for a couple of
            // frames so the fall reads as a fall rather than a cut.
            let mut state = st.state;
            if toppling && collapsed_now.contains(&i) {
                state = StructState::Leaning;
            }
            let (sprite, faithful) = match self.sprites.as_ref() {
                Some(s) => {
                    let (spr, faithful) = s.structure(st.kind, state);
                    (spr.cloned(), faithful)
                }
                None => (None, false),
            };
            let footprint = st.kind.footprint() as i32;
            // Rubble lands on the ground that is there *now*. A structure
            // undermined into a pit does not leave its wreckage hanging in the
            // air where its footings used to be.
            let ground = if state == StructState::Collapsed {
                screen.field.seat(st.x0, st.kind.footprint())
            } else {
                st.base
            };
            if state == StructState::Collapsed && !faithful {
                // No collapsed drawing for this silhouette yet: a rubble mound
                // the width of its own footprint beats a water tower's.
                raster::draw_rubble(
                    &mut self.canvas,
                    view,
                    st.x0 as f64,
                    footprint,
                    ground,
                    &pal,
                );
                continue;
            }
            match sprite {
                Some(sprite) => {
                    // Centre on the footprint, seat on the ground, shift with
                    // the lean. Whole pixels only, and never a scale factor.
                    let lean = st.lean as i32;
                    let x = view.sx(st.x0 as f64) + (footprint - sprite.w) / 2 + lean;
                    let y = view.sy(ground) - sprite.h + Sprites::seat_offset(st.kind);
                    self.canvas.blit(&sprite, x, y);
                }
                None => {
                    // No art loaded: flat columns, so the screen still plays.
                    let h = st.kind.height() as i32;
                    let x = view.sx(st.x0 as f64);
                    let base_y = view.sy(ground);
                    let c = if state == StructState::Collapsed {
                        pal.get(slot::ST_DMG)
                    } else {
                        pal.get(slot::ST)
                    };
                    let hh = if state == StructState::Collapsed {
                        5
                    } else {
                        h
                    };
                    self.canvas.rect(x, base_y - hh, footprint, hh, c);
                }
            }
        }
    }

    fn draw_player(&mut self, screen: &Screen, loadout: &Loadout, view: &View) {
        let muzzle = screen.muzzle();
        let shot = Shot::new(self.angle_ticks, self.power_ticks).quantized(loadout);
        let frame = shot.barrel_frame();

        let (tank, pivot, barrel) = match self.sprites.as_ref() {
            Some(s) => {
                let (t, p) = s.tank(loadout);
                (t.cloned(), p, s.barrel(loadout).cloned())
            }
            None => (None, PIVOT_BASE, None),
        };

        let pivot_x = view.sx(muzzle.x);
        let pivot_y = view.sy(muzzle.y);

        // Barrel first, so the chassis overlaps its root.
        if let Some(barrel) = barrel {
            let sx = Sprites::barrel_frame_x(frame);
            self.canvas.blit_region(
                &barrel,
                pivot_x - BARREL_PIVOT.0,
                pivot_y - BARREL_PIVOT.1,
                crate::raster::Rect {
                    x: sx,
                    y: 0,
                    w: BARREL_FRAME,
                    h: BARREL_FRAME,
                },
            );
        }
        if let Some(tank) = tank {
            self.canvas
                .blit(&tank, pivot_x - pivot.0, pivot_y - pivot.1);
        }
    }

    fn draw_shot_effects(&mut self, view: &View) {
        let Some(anim) = self.anim.as_ref() else {
            return;
        };
        let alert = self.pal.get(slot::ALERT);
        let dust = self.pal.get(slot::ST_DMG);
        if presentation::flash_strength(anim.t, 0.0, presentation::MUZZLE_TIME) > 0.0 {
            let p = anim.before.muzzle();
            let x = view.sx(p.x);
            let y = view.sy(p.y);
            self.canvas.hline(x - 3, x + 3, y, alert);
            self.canvas.vline(x, y - 3, y + 3, alert);
            self.canvas.put(x + 2, y - 2, alert);
            self.canvas.put(x + 2, y + 2, alert);
        }
        for (j, trace) in anim.traces.iter().enumerate() {
            let age = anim.t - trace.flight_time;
            if !(0.0..presentation::IMPACT_TIME).contains(&age) || !trace.end.detonates() {
                continue;
            }
            let p = trace.end.pos();
            let x = view.sx(p.x);
            let y = view.sy(p.y);
            if age < 0.06 {
                self.canvas.hline(x - 2, x + 2, y, alert);
                self.canvas.vline(x, y - 2, y + 2, alert);
            }
            let spread = 1 + (age * 30.0) as i32;
            for i in 0..5 {
                let dx = ((i * 7 + j * 3) as i32 % (spread * 2 + 1)) - spread;
                let dy = -((i * 3 + j) as i32 % (spread + 2));
                self.canvas.put(x + dx, y + dy, dust);
            }
        }
        if anim.done()
            && !anim.collapsed.is_empty()
            && anim.settled < presentation::COLLAPSE_TIME
            && (anim.settled * 30.0) as i32 % 2 == 0
        {
            let (w, h) = (self.canvas.w, self.canvas.h);
            self.canvas.rect_outline(1, 1, w - 2, h - 2, alert);
        }
    }

    fn onboarding_prompt(&self, phase: Phase, wind: Option<f64>) -> Option<String> {
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

    #[allow(clippy::too_many_arguments)]
    fn render_hud(
        &mut self,
        shells: i32,
        index: u32,
        wind: Option<f64>,
        range: Option<f64>,
        fuse: f64,
        loadout: &Loadout,
        screen: &Screen,
        phase: Phase,
    ) {
        let pal = self.pal.clone();
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let w = self.canvas.w;
        let (ex, ey) = self.corner_pad;

        // Wind, top left: a windsock flying the way the wind blows, then its
        // strength as a number. The sock never grows with strength, so the
        // number is still the read (DESIGN §2.5). It hangs limp when there is
        // no wind to show, or none shown yet (Dead Reckoning).
        let (wind_text, to_left) = match wind {
            None => ("--".to_string(), false),
            Some(v) if loadout.has_flag(flags::ANEMOMETER) => (format!("{:.1}", v.abs()), v < 0.0),
            Some(v) => (format!("{:.0}", v.abs()), v < 0.0),
        };
        let calm = !wind_text.bytes().any(|b| (b'1'..=b'9').contains(&b));
        let sock = if calm {
            &crate::icons::SOCK_LIMP
        } else {
            &crate::icons::SOCK
        };
        let sock_w = crate::icons::width(sock);
        self.canvas.plate(
            3 + ex,
            3 + ey,
            sock_w + 9 + crate::font::big_width(&wind_text) as i32,
            13,
            &pal,
        );
        self.canvas
            .icon(6 + ex, 4 + ey, sock, &[fg, alert, dim], to_left && !calm);
        self.canvas
            .text_big(6 + ex + sock_w + 3, 4 + ey, &wind_text, alert);

        // Shells, top right: a shell, then the count. Alert accent under 10,
        // the only other place that colour is allowed (DESIGN §5.2).
        let shell_text = format!("{shells}");
        let shell_w = crate::icons::width(&crate::icons::SHELL);
        let plate_w = shell_w + 13 + crate::font::big_width(&shell_text) as i32;
        let px = w - plate_w - 3 - ex;
        self.canvas.plate(px, 3 + ey, plate_w, 13, &pal);
        self.canvas.icon(
            px + 3,
            4 + ey,
            &crate::icons::SHELL,
            &[fg, alert, dim],
            false,
        );
        self.canvas
            .text_small(px + 3 + shell_w + 2, 6 + ey, "X", dim);
        let c = if shells < 10 { alert } else { fg };
        self.canvas
            .text_big(px + 3 + shell_w + 7, 4 + ey, &shell_text, c);

        // Screen index, bottom right, small. No score during the run
        // (DESIGN §1).
        let idx = format!("{}", index + 1);
        let ix = w - crate::font::small_width(&idx) as i32 - 3 - ex;
        self.canvas.text_small(ix, self.canvas.h - 8 - ey, &idx, fg);

        // Instruments, bottom left.
        let mut y = self.canvas.h - 8 - ey;
        let shot = Shot::new(self.angle_ticks, self.power_ticks).quantized(loadout);
        let angle = if loadout.angle_quantum_ticks == 1 {
            format!("ANG {:.2}", shot.angle_degrees())
        } else {
            format!("ANG {:.1}", shot.angle_degrees())
        };
        let power = format!("PWR {}", shot.power_ticks);
        self.canvas.text_small(3 + ex, y, &power, fg);
        y -= 7;
        self.canvas.text_small(3 + ex, y, &angle, fg);
        if let Some(r) = range {
            if loadout.has_flag(flags::RANGEFINDER) {
                y -= 7;
                self.canvas
                    .text_small(3 + ex, y, &format!("RNG {:.0}", r), fg);
            }
        }
        if loadout.airburst() {
            y -= 7;
            self.canvas
                .text_small(3 + ex, y, &format!("FUSE {fuse:.0}"), fg);
        }
        if loadout.enfilade_units > 0.0 {
            y -= 7;
            let off = self
                .run
                .as_ref()
                .map(|r| r.emplacement_offset)
                .unwrap_or(0.0);
            self.canvas
                .text_small(3 + ex, y, &format!("EMPL {off:+.0}"), dim);
        }

        // Held modifiers, one compact line.
        let held: Vec<String> = Modifier::all()
            .filter(|m| loadout.has(*m))
            .map(|m| {
                let n = loadout.stacks(m);
                let short: String = m.name().chars().take(4).collect();
                if n > 1 {
                    format!("{short}{n}")
                } else {
                    short
                }
            })
            .collect();
        if !held.is_empty() {
            let line = held.join(" ");
            self.canvas.text_small(3 + ex, 18 + ey, &line, dim);
        }

        if self.status_time > 0.0 {
            let text = self.status.clone();
            let x = (w - crate::font::small_width(&text) as i32) / 2;
            self.canvas.text_small(x, 24 + ey, &text, alert);
        } else if let Some(text) = self.onboarding_prompt(phase, wind) {
            let x = (w - crate::font::small_width(&text) as i32) / 2;
            self.canvas.text_small(x.max(2), 24 + ey, &text, alert);
        }

        // Only once the shell has actually landed: the sim resolves the clear
        // the instant the trigger is pulled, and announcing it while the shot
        // is still visibly in the air gives the result away.
        let _ = phase;
        if self.clear_hold > 0.0 {
            let text = "SCREEN CLEAR";
            let x = (w - crate::font::small_width(text) as i32) / 2;
            self.canvas.text_small(x, 34 + ey, text, fg);
        }

        if self.debug {
            let m = screen.field.material_at(screen.muzzle().x);
            let lines = [
                format!("TPL {}", screen.template.name()),
                format!("CND {}", screen.candidates.min(999)),
                format!("ATT {}", screen.attempts),
                format!("MAT {}", raster::material_name(m)),
                format!("CAM {}", self.cam_x),
                format!("BUF {}X{}", self.buffer_w, self.buffer_h),
                format!("WND {:.3}", screen.wind),
            ];
            let mut dy = 42;
            for line in lines {
                self.canvas.text_small(3, dy, &line, dim);
                dy += 7;
            }
        }
    }

    fn render_draft(&mut self, offers: &[Modifier], loadout: &Loadout) {
        let pal = self.pal.clone();
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let w = self.canvas.w;
        let h = self.canvas.h;

        // Dim the screen behind the cards without a gradient.
        for y in 0..h {
            for x in 0..w {
                if (x + y) % 2 == 0 {
                    self.canvas.put(x, y, pal.get(slot::UI_BG));
                }
            }
        }

        let title = "DRAFT  TAKE ONE";
        let tx = (w - crate::font::small_width(title) as i32) / 2;
        self.canvas.text_small(tx, 8, title, fg);

        let n = offers.len().max(1) as i32;
        let gap = 4;
        let card_w = ((w - gap * (n + 1)) / n).max(60);
        let card_h = 96;
        let top = 24;
        for (i, m) in offers.iter().enumerate() {
            let x = gap + i as i32 * (card_w + gap);
            self.canvas.plate(x, top, card_w, card_h, &pal);
            if self.touch_active {
                let action = ["draft_1", "draft_2", "draft_3"][i.min(2)];
                self.touch_button(Rect::new(x, top, card_w, card_h), action);
            }
            let e = m.entry();
            self.canvas
                .text_small(x + 3, top + 4, &format!("{}", i + 1), alert);
            self.canvas.text_small(x + 11, top + 4, e.name, fg);
            self.canvas.text_small(x + 3, top + 12, e.tag.name(), alert);
            let stacks = loadout.stacks(*m);
            if stacks > 0 {
                self.canvas.text_small(
                    x + 3,
                    top + 20,
                    &format!("HELD {stacks}/{}", m.max_stacks()),
                    dim,
                );
            }
            // Wrap the blurb into the card at the same size as everything else.
            let max_chars = ((card_w - 6) / crate::font::SMALL_ADVANCE as i32).max(4) as usize;
            let mut y = top + 30;
            let mut line = String::new();
            for word in e.blurb.split_whitespace() {
                if line.is_empty() {
                    line = word.to_string();
                } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
                    line.push(' ');
                    line.push_str(word);
                } else {
                    self.canvas.text_small(x + 3, y, &line, dim);
                    y += 7;
                    line = word.to_string();
                }
            }
            if !line.is_empty() {
                self.canvas.text_small(x + 3, y, &line, dim);
            }
        }

        let hint = if self.touch_active {
            "TAP A CARD"
        } else {
            "1 2 3  CHOOSE"
        };
        let hx = (w - crate::font::small_width(hint) as i32) / 2;
        self.canvas.text_small(hx, top + card_h + 6, hint, fg);
    }

    fn render_paywall(&mut self) {
        let pal = self.pal.clone();
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let (w, h) = (self.canvas.w, self.canvas.h);
        let bw = (w - 40).max(120);
        let bh = 80;
        let x = (w - bw) / 2;
        let y = (h - bh) / 2;
        self.canvas.plate(x, y, bw, bh, &pal);
        let mut cy = y + 8;
        self.centred(cy, &format!("SCREENS 1-{FREE_SCREENS} ARE FREE"), fg);
        cy += 12;
        self.centred(cy, "UNLOCK THE FULL RUN", alert);
        cy += 12;
        self.centred(cy, "ONE PURCHASE, NO ADS, NO TIMERS", dim);
        cy += 14;
        self.centred(cy, "U      UNLOCK", fg);
        cy += 8;
        self.centred(cy, "R      RESTORE PURCHASE", fg);
        cy += 8;
        self.centred(cy, "ENTER  END THE RUN", dim);
    }

    fn render_pause(&mut self) {
        let pal = self.pal.clone();
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let (w, h) = (self.canvas.w, self.canvas.h);
        if self.touch_active {
            let top = (h - 64) / 2;
            self.canvas.plate((w - 150) / 2, top, 150, 64, &pal);
            self.centred(top + 8, "PAUSED", fg);
            let y = self.menu_row(top + 26, "confirm", "", "RESUME", fg);
            self.menu_row(y, "replay", "", "TITLE", dim);
            return;
        }
        self.canvas
            .plate((w - 150) / 2, (h - 48) / 2, 150, 48, &pal);
        self.centred((h - 48) / 2 + 8, "PAUSED", fg);
        let resume = self.input_label("confirm");
        self.centred((h - 48) / 2 + 21, &format!("{resume} / BACK  RESUME"), fg);
        let title = self.input_label("replay");
        self.centred((h - 48) / 2 + 31, &format!("{title}  TITLE"), dim);
    }
}

/// What a prompt calls an action on a touchscreen: the glyph on its on-screen
/// button, or nothing where the prompt's own line is the button.
fn touch_label(action: &str) -> &'static str {
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

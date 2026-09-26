//! The game node. Rendering, input, UI — no game logic.
//!
//! Everything that decides anything lives in `salt-line-sim`. This file draws
//! what the sim says is true, feeds it integer inputs, and animates the gap
//! between "the shot has been resolved" and "the player has seen it happen".

use godot::classes::notify::CanvasItemNotification;
use godot::classes::{
    window::ContentScaleAspect, AudioStream, AudioStreamPlayer, DisplayServer, Engine, INode2D,
    Image, ImageTexture, Input, InputEvent, InputEventJoypadButton, InputEventJoypadMotion,
    InputEventKey, InputEventMouseMotion, InputEventScreenDrag, InputEventScreenTouch, Node2D, Os,
    RenderingServer, ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::prelude::*;

use salt_line_sim::ballistics::{self, End, FireOptions, Shot, Trace};
use salt_line_sim::dmath;
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{
    self, daily_seed, format_seed, parse_seed, Phase, RunState, Screen, StructState,
};
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_H};

use crate::app::*;
use crate::crash;
use crate::frame::{self, DisplayPoll, Memo, Redraw};
use crate::keypad::{self, Focus};
use crate::pads::Pads;
use crate::palette::{slot, Palette};
use crate::presentation::{
    self, RepeatState, LEARN_ANGLE, LEARN_CRATER, LEARN_DRAFT, LEARN_FIRE, LEARN_POWER,
    LEARN_SHELLS, LEARN_WIND,
};
use crate::raster::{self, Canvas, View};
use crate::resume::{self, Found, RUN_LOADING_PATH, RUN_PATH, RUN_TEMP_PATH};
use crate::save::{self, Entry};
use crate::save_format::{ArcHint, TouchAim};
use crate::sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT, PIVOT_BASE};
use crate::touch::{Button, Rect};

const PALETTE_PATH: &str = "res://art/palette_16.png";
const SPRITE_DIR: &str = "res://art/sprites";

/// Everything the drag preview's trajectory depends on. The terrain and the
/// targets only change when a shot lands (`shots`) or the screen does
/// (`seed`, `index`); the muzzle moves with Enfilade (`offset`).
#[derive(PartialEq, Debug)]
struct PreviewKey {
    angle: i32,
    power: i32,
    hint: ArcHint,
    /// Bits of the wind the preview flies in, which is the HUD's wind.
    wind: u64,
    seed: u64,
    index: u32,
    shots: u32,
    offset: u64,
}

/// A cached preview: the primary shell's path and how much of it to draw.
type Preview = (Vec<Vec2>, usize);

#[derive(GodotClass)]
#[class(base = Node2D)]
pub struct SaltLine {
    base: Base<Node2D>,
    /// Everything the game knows that is not the engine's. See `app`.
    g: Game,

    pal: Palette,
    sprites: Option<Sprites>,
    /// One buffer for everything: terrain, sprites and UI, all at the internal
    /// resolution. DESIGN §5.1 — "UI renders in the same buffer at the same
    /// resolution".
    canvas: Canvas,
    /// The frame's image and texture live as long as the buffer keeps its
    /// size: each frame is copied into one of `upload_bufs` and handed to the
    /// image, which shares rather than copies it. Two buffers, so the one
    /// being written is never the one the image still holds.
    image: Option<Gd<Image>>,
    texture: Option<Gd<ImageTexture>>,
    upload_bufs: [PackedByteArray; 2],
    upload_flip: usize,
    /// Whether this frame needs drawing at all. See `frame::Redraw`.
    redraw: Redraw,
    /// The engine's frame cap as the project set it, restored in play.
    project_fps: i32,
    fps_cap: i32,
    /// When to read the display's safe area again, and what it last said.
    display: DisplayPoll,
    /// The drag preview's trajectory, recomputed only when its inputs change.
    preview: Memo<PreviewKey, Preview>,

    /// Key hold-to-repeat state.
    keys: Vec<(Key, RepeatState)>,
    /// Key and button presses as events arrive, and the ones this frame
    /// reads. Polling alone loses a press and release inside one frame.
    key_queue: Vec<Key>,
    key_presses: Vec<Key>,
    pad_queue: Vec<JoyButton>,
    pad_presses: Vec<JoyButton>,
    /// The joypads that drive the game: any that has pressed a button or
    /// shown a stick at rest. See `pads::Pads`.
    pads: Pads,
    /// Seconds until the joypad list is checked against the connected ones.
    pad_prune: f64,
    /// Hold-to-repeat per pad button, shared by every action bound to it.
    pad_keys: Vec<(JoyButton, RepeatState)>,
    /// Hold-to-repeat for the left stick, per action slot.
    pad_repeat: [RepeatState; 8],
    /// On-screen buttons drawn last frame, hit-tested by this frame's touches.
    touch_buttons: Vec<Button>,
    audio: Vec<Option<Gd<AudioStreamPlayer>>>,

    /// `--salt-shot <path>`: render a few frames, save the buffer as a PNG at
    /// native resolution, and quit. How the pixel output gets checked without
    /// a human at the keyboard.
    shot_path: Option<String>,
    frames: u64,
    /// Seconds of wall clock since the node came up, for the dev flags below.
    elapsed: f64,
    /// `--salt-shot-at <seconds>`: when to grab the screenshot.
    shot_at: f64,
}

#[godot_api]
impl INode2D for SaltLine {
    fn init(base: Base<Node2D>) -> Self {
        SaltLine {
            base,
            g: Game::default(),
            pal: Palette::default(),
            sprites: None,
            canvas: Canvas::new(320, 180),
            image: None,
            texture: None,
            upload_bufs: [PackedByteArray::new(), PackedByteArray::new()],
            upload_flip: 0,
            redraw: Redraw::default(),
            project_fps: 0,
            fps_cap: 0,
            display: DisplayPoll::default(),
            preview: Memo::default(),
            keys: Vec::new(),
            key_queue: Vec::new(),
            key_presses: Vec::new(),
            pad_queue: Vec::new(),
            pad_presses: Vec::new(),
            pads: Pads::default(),
            pad_prune: 0.0,
            pad_keys: Vec::new(),
            pad_repeat: [RepeatState::default(); 8],
            touch_buttons: Vec::new(),
            audio: Vec::new(),
            shot_path: None,
            frames: 0,
            elapsed: 0.0,
            shot_at: 0.05,
        }
    }

    fn ready(&mut self) {
        // Phones and tablets start with touch controls; desktops show them only
        // once the screen is actually touched.
        self.g.mobile = Os::singleton().has_feature("mobile");
        self.g.touch_active = self.g.mobile;
        if self.g.mobile {
            // Rounded screen corners (Pixel 10: 138 px radius at 1080 px tall):
            // 10 buffer pixels in from the side and 6 down keeps HUD text and
            // buttons comfortably inside the curve without raising the buttons
            // off the ground.
            self.g.corner_pad = (10, 6);
            // What shows beside the playfield is a letterbox, not sky.
            RenderingServer::singleton().set_default_clear_color(Color::BLACK);
        }
        self.pal = Palette::load(PALETTE_PATH);
        self.sprites = Some(Sprites::load_all(SPRITE_DIR));
        self.g.save = save::load();
        self.find_last_crash();
        self.find_saved_run();
        self.setup_audio();
        self.project_fps = Engine::singleton().get_max_fps();
        self.fps_cap = self.project_fps;
        self.read_display();
        // And again on the first frame: on a phone the safe area may not be
        // settled yet while the scene is still coming up.
        self.display.invalidate();
        self.g.buffer_w = self.g.pick_width();
        self.apply_buffer_size();
        if let Some(dir) = Self::cmdline_value("--salt-gallery") {
            self.gallery(&dir);
            return;
        }
        self.shot_path = Self::screenshot_arg();
        self.shot_at = Self::cmdline_value("--salt-shot-at")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.05);
        self.g.auto = Self::cmdline_flag("--salt-auto");
        self.g.auto_miss = Self::cmdline_flag("--salt-miss");
        self.g.auto_hold_draft = Self::cmdline_flag("--salt-hold-draft");
        if let Some(seed) = Self::seed_arg() {
            self.start_run(seed, false);
            if let Some(n) = Self::screen_arg() {
                self.jump_to(n);
            }
        } else if Self::cmdline_flag("--salt-continue") {
            self.continue_run();
        }
        if !self.pal.sampled {
            godot_warn!("salt-line: palette not sampled from art; using the built-in fallback");
        }
    }

    fn process(&mut self, delta: f64) {
        if crash::panicked() {
            self.broken_frame(delta);
            return;
        }
        self.note_crash_context();
        // Animation and timers never jump more than a tenth of a second, however
        // long the app sat in the background. See `resume::frame_delta`. The
        // dev clock (`--salt-shot-at`) runs on the same time, so a capture on
        // a slow frame lands at the same point of an animation.
        let delta = resume::frame_delta(delta);
        self.elapsed += delta;
        self.g.status_time = (self.g.status_time - delta).max(0.0);
        self.poll_display(delta);
        let before = self.g.frame_state();
        self.handle_input(delta);
        if self.g.auto {
            self.autopilot(delta);
        }
        self.step(delta);
        self.persist_run();
        self.tick_settings(delta);
        let after = self.g.frame_state();
        if after != before {
            self.redraw.mark();
            // Between runs the buffer may take a new width: look at the
            // display afresh rather than wait for the next scheduled read.
            if after.mode != before.mode && matches!(after.mode, Mode::Title | Mode::GameOver) {
                self.display.invalidate();
            }
        }
        self.prune_pads(delta);
        self.apply_frame_cap();
        if self.redraw.due(self.busy()) {
            self.render();
        }
        // Every frame, drawn or not: an undrawn frame is the last one drawn.
        self.maybe_screenshot();
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
        let event = match event.try_cast::<InputEventMouseMotion>() {
            // Nothing follows the mouse.
            Ok(_) => return,
            Err(event) => event,
        };
        let event = match event.try_cast::<InputEventJoypadMotion>() {
            // Stick deflection is polled; `busy` keeps frames coming while
            // one is held over.
            Ok(motion) => {
                let axis = match motion.get_axis() {
                    JoyAxis::LEFT_X => 0,
                    JoyAxis::LEFT_Y => 1,
                    _ => usize::MAX,
                };
                self.pads
                    .motion(motion.get_device(), axis, motion.get_axis_value());
                return;
            }
            Err(event) => event,
        };
        self.redraw.mark();
        let event = match event.try_cast::<InputEventKey>() {
            Ok(key) => {
                if key.is_pressed() && !key.is_echo() {
                    self.key_queue.push(key.get_keycode());
                }
                return;
            }
            Err(event) => event,
        };
        let event = match event.try_cast::<InputEventJoypadButton>() {
            Ok(button) => {
                if button.is_pressed() {
                    self.pads.button(button.get_device());
                    self.pad_queue.push(button.get_button_index());
                }
                return;
            }
            Err(event) => event,
        };
        let event = match event.try_cast::<InputEventScreenDrag>() {
            Ok(drag) => {
                let local = self
                    .base()
                    .make_input_local(&drag.clone().upcast::<InputEvent>());
                let pos = local
                    .try_cast::<InputEventScreenDrag>()
                    .map(|d| d.get_position())
                    .unwrap_or_else(|_| drag.get_position());
                self.g.touch.finger_moved(drag.get_index(), pos.x, pos.y);
                return;
            }
            Err(event) => event,
        };
        let Ok(touch) = event.try_cast::<InputEventScreenTouch>() else {
            return;
        };
        self.g.touch_active = true;
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
            self.g
                .touch
                .finger_down(touch.get_index(), pos.x, pos.y, &self.touch_buttons);
        } else {
            self.g.touch.finger_up(touch.get_index());
        }
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::WM_GO_BACK_REQUEST => {
                self.g.back_requested = true;
                self.redraw.mark();
            }
            CanvasItemNotification::WM_WINDOW_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_FOCUS_OUT => {
                self.g.touch.release_all();
                self.auto_pause();
                self.flush_settings();
                self.redraw.mark();
            }
            // The display may have turned, or grown a cutout, while away.
            CanvasItemNotification::WM_WINDOW_FOCUS_IN
            | CanvasItemNotification::APPLICATION_FOCUS_IN
            | CanvasItemNotification::APPLICATION_RESUMED => {
                self.display.invalidate();
                self.redraw.mark();
            }
            // Pending settings go to disk before the app can be killed: a
            // phone may never resume a paused app, and a close is final.
            CanvasItemNotification::APPLICATION_PAUSED => {
                self.auto_pause();
                self.flush_settings();
            }
            CanvasItemNotification::WM_CLOSE_REQUEST | CanvasItemNotification::EXIT_TREE => {
                self.flush_settings()
            }
            _ => {}
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if self.g.mode != Mode::SeedEntry {
            return;
        }
        let Ok(key) = event.try_cast::<InputEventKey>() else {
            return;
        };
        if !key.is_pressed() || key.is_echo() {
            return;
        }
        let unicode = key.get_unicode();
        let keycode = key.get_keycode();
        match keycode {
            Key::BACKSPACE => {
                self.g.seed_text.pop();
            }
            Key::ENTER | Key::KP_ENTER => self.start_seeded_run(),
            Key::ESCAPE => self.leave_seed_entry(),
            _ if (32..127).contains(&unicode) => {
                if self.g.seed_text.chars().count() < 20 {
                    self.g.seed_text.push(unicode as u8 as char);
                }
            }
            // Arrows and the like fall through to move the keypad focus.
            _ => return,
        }
        // Typed, not pressed: an M in a seed must not also mute, nor the
        // Enter that starts the run fire its first shot.
        self.key_queue.retain(|k| *k != keycode);
    }
}

impl SaltLine {
    // ------------------------------------------------------------- lifecycle

    /// Days since the Unix epoch. The clock is read here, in the presentation
    /// layer — nothing in the sim ever touches system time.
    fn start_seeded_run(&mut self) {
        let seed = if self.g.seed_text.trim().is_empty() {
            daily_seed(Self::today())
        } else {
            parse_seed(&self.g.seed_text)
        };
        self.start_run(seed, false);
    }

    fn leave_seed_entry(&mut self) {
        self.g.mode = Mode::Title;
    }

    fn open_seed_entry(&mut self) {
        self.g.seed_text.clear();
        self.g.seed_focus = Focus::default();
        self.g.mode = Mode::SeedEntry;
    }

    fn today() -> u64 {
        let secs = Time::singleton().get_unix_time_from_system();
        (secs / 86400.0).floor().max(0.0) as u64
    }

    fn apply_buffer_size(&mut self) {
        let h = if self.g.long_lens() {
            LONG_LENS_H
        } else {
            SCREEN_H as i32
        };
        self.g.buffer_h = h;
        self.canvas.resize(self.g.buffer_w, h);
        self.g.letterbox = self.g.letterbox_now();
        let (l, r) = self.g.letterbox;
        if let Some(mut window) = self.base().get_window() {
            window.set_content_scale_size(Vector2i::new(self.g.buffer_w + l + r, h));
        }
        self.base_mut().set_position(Vector2::new(l as f32, 0.0));
        self.redraw.mark();
    }

    /// Read the window size and the cutout it leaves, for `pick_width` and
    /// `letterbox_now`. Not every frame: see `poll_display`.
    fn read_display(&mut self) {
        let size = DisplayServer::singleton().window_get_size();
        self.g.window_size = (size.x, size.y);
        self.g.cutout = self.cutout_px();
    }

    /// Follow the display every frame from the window size, which is cheap,
    /// and the cutout, which is re-read only when the size changes, focus
    /// comes back, a run ends or a second has passed: the safe area is a
    /// platform round trip on a phone.
    fn poll_display(&mut self, delta: f64) {
        let size = DisplayServer::singleton().window_get_size();
        self.g.window_size = (size.x, size.y);
        if self.display.check((size.x, size.y), delta) {
            self.g.cutout = self.cutout_px();
        }
        let want_w = self.g.pick_width();
        let in_run = matches!(self.g.mode, Mode::Playing | Mode::Paused);
        if !in_run && self.g.kept_width {
            // A resumed run's width is done with: back to the device's.
            self.set_run_width(None);
            self.apply_buffer_size();
        } else if self.g.run.is_none() && want_w != self.g.buffer_w {
            self.g.buffer_w = want_w;
            self.apply_buffer_size();
        } else if self.g.letterbox_now() != self.g.letterbox {
            self.apply_buffer_size();
        }
    }

    /// Screen pixels a display cutout takes from the left and right edges: a
    /// camera hole on a phone held landscape. Only asked on mobile, where the
    /// window is the whole display; on a desktop the "safe area" is the
    /// monitor's, not the window's.
    fn cutout_px(&self) -> (i32, i32) {
        if !self.g.mobile {
            return (0, 0);
        }
        let (win_x, _) = self.g.window_size;
        let safe = DisplayServer::singleton().get_display_safe_area();
        if safe.size.x <= 0 || safe.size.y <= 0 {
            return (0, 0);
        }
        let l = safe.position.x.clamp(0, win_x / 4);
        let r = (win_x - safe.position.x - safe.size.x).clamp(0, win_x / 4);
        (l, r)
    }

    fn start_run(&mut self, seed: u64, daily: bool) {
        // A new run is generated for the device, whatever a resumed one used.
        self.set_run_width(None);
        // The world is as wide as the buffer, so the run is generated for it.
        let width = self.g.buffer_w as usize;
        let state = RunState::new(seed, width);
        self.g.angle_ticks = 900;
        self.g.power_ticks = 600;
        self.g.anim = None;
        self.g.last_arc.clear();
        self.preview.clear();
        self.g.clear_hold = 0.0;
        self.g.cam_x = 0;
        self.g.cam_target = 0.0;
        self.g.daily = daily;
        self.g.low_shell_warned = false;
        self.g.last_seed = seed;
        self.retire_last_crash();
        self.g.run = Some(state);
        self.g.mode = Mode::Playing;
        self.apply_buffer_size();
        self.begin_persisting();
        self.note(format!("SEED {}", format_seed(seed)));
    }

    /// Level inspector: drop straight onto a screen index without playing the
    /// ones before it. DESIGN §2.2 is what makes this possible — each screen
    /// generates from its own seed, so screen 400 costs the same as screen 1.
    fn jump_to(&mut self, index: u32) {
        let Some(run) = self.g.run.as_mut() else {
            return;
        };
        run.screen_index = index;
        // The log cannot rebuild a jump, so the run is not kept for resume.
        self.g.run_persist = false;
        run.screen = run::generate_screen(run.run_seed, index, run.width, &run.loadout);
        self.preview.clear();
        self.note(format!("SCREEN {}", index + 1));
    }

    fn note(&mut self, text: String) {
        self.g.status = text;
        self.g.status_time = 2.4;
        self.redraw.mark();
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
        let db = if self.g.save.muted || self.g.save.volume == 0 {
            -80.0
        } else {
            20.0 * (self.g.save.volume as f32 / 100.0).log10()
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
        if self.g.save.learned & bit == 0 {
            self.g.save.learned |= bit;
            save::store(&self.g.save);
        }
    }

    fn binding(&self, action: &str, default_key: &str, default_pad: &str) -> (String, String) {
        self.g
            .save
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
            "U" => Key::U,
            "N" => Key::N,
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
            "seed" => ("S", "Y"),
            // Only offered over a saved run, where confirm is CONTINUE and
            // pad Y is already seed entry.
            "new_run" => ("N", "RB"),
            _ => ("", ""),
        };
        self.binding(action, defaults.0, defaults.1)
    }

    /// Once a second, forget joypads that have been unplugged.
    fn prune_pads(&mut self, delta: f64) {
        self.pad_prune -= delta;
        if self.pad_prune > 0.0 {
            return;
        }
        self.pad_prune = 1.0;
        let connected: Vec<i32> = Input::singleton()
            .get_connected_joypads()
            .iter_shared()
            .map(|d| d as i32)
            .collect();
        self.pads.retain_connected(&connected);
    }

    /// The button is held on any joypad.
    fn pad_held(&self, button: JoyButton) -> bool {
        let input = Input::singleton();
        self.pads
            .iter()
            .any(|p| input.is_joy_button_pressed(p.device, button))
    }

    /// The stick is pushed past the threshold on any joypad whose stick has
    /// been seen at rest.
    fn stick_held(&self, axis: JoyAxis, sign: f32) -> bool {
        let slot = if axis == JoyAxis::LEFT_X { 0 } else { 1 };
        let input = Input::singleton();
        self.pads
            .iter()
            .filter(|p| p.armed[slot])
            .any(|p| input.get_joy_axis(p.device, axis) * sign > 0.45)
    }

    /// Take this frame's press of a pad button, if it had one. Taken, so a
    /// second action bound to the same button in the same mode does not
    /// fire as well: the first one asked wins.
    fn take_pad_press(&mut self, button: JoyButton) -> bool {
        let Some(i) = self.pad_presses.iter().position(|b| *b == button) else {
            return false;
        };
        self.pad_presses.swap_remove(i);
        true
    }

    /// True on the frame the button went down on any joypad.
    fn pad_pressed(&mut self, button: JoyButton) -> bool {
        let down = self.take_pad_press(button);
        if down {
            self.g.controller_active = true;
            self.g.touch_active = false;
        }
        down
    }

    fn action_pressed(&mut self, _slot: usize, action: &str) -> bool {
        let (key, _) = self.action_binding(action);
        let key_down = Self::key_token(&key)
            .map(|k| self.pressed(k))
            .unwrap_or(false);
        if key_down {
            self.g.controller_active = false;
            self.g.touch_active = false;
        }
        let pad_down = self.pad_action_pressed(action);
        let touch_down =
            self.g.touch.pressed(action) || (action == "back" && self.g.back_requested);
        key_down || pad_down || touch_down
    }

    /// `action_pressed` for the controller alone.
    fn pad_action_pressed(&mut self, action: &str) -> bool {
        let (_, pad) = self.action_binding(action);
        Self::pad_token(&pad)
            .map(|b| self.pad_pressed(b))
            .unwrap_or(false)
    }

    fn action_repeated(&mut self, slot: usize, action: &str, delta: f64) -> u32 {
        let (key, pad) = self.action_binding(action);
        let keyboard = Self::key_token(&key)
            .map(|k| self.repeated(k, delta))
            .unwrap_or(0);
        let buttons = match Self::pad_token(&pad) {
            Some(b) => {
                let press = self.take_pad_press(b);
                let down = self.pad_held(b);
                let i = match self.pad_keys.iter().position(|(k, _)| *k == b) {
                    Some(i) => i,
                    None => {
                        self.pad_keys.push((b, RepeatState::default()));
                        self.pad_keys.len() - 1
                    }
                };
                self.pad_keys[i]
                    .1
                    .tick_pressed(press, down, delta, REPEAT_DELAY, REPEAT_RATE)
            }
            None => 0,
        };
        // Left stick is always available in addition to remappable D-pad.
        let axis = match action {
            "left" => Some((JoyAxis::LEFT_X, -1.0)),
            "right" => Some((JoyAxis::LEFT_X, 1.0)),
            "up" => Some((JoyAxis::LEFT_Y, -1.0)),
            "down" => Some((JoyAxis::LEFT_Y, 1.0)),
            _ => None,
        };
        let stick_down = axis.is_some_and(|(axis, sign)| self.stick_held(axis, sign));
        let stick = self.pad_repeat[slot].tick(stick_down, delta, REPEAT_DELAY, REPEAT_RATE);
        if keyboard > 0 {
            self.g.controller_active = false;
            self.g.touch_active = false;
        }
        if buttons > 0 || stick_down {
            self.g.controller_active = true;
            self.g.touch_active = false;
        }
        keyboard + buttons + stick + self.g.touch.repeated(action)
    }

    fn input_label(&self, action: &str) -> String {
        if self.g.touch_active {
            return touch_label(action).to_string();
        }
        let (key, pad) = self.action_binding(action);
        if self.g.controller_active {
            pad
        } else {
            key
        }
    }

    fn key_slot(&mut self, key: Key) -> usize {
        if let Some(i) = self.keys.iter().position(|(k, _)| *k == key) {
            i
        } else {
            self.keys.push((key, RepeatState::default()));
            self.keys.len() - 1
        }
    }

    /// True on the frame a key goes down, even if it came up again before
    /// the frame began. The press is taken, so of two actions bound to the
    /// same key only the first one asked fires.
    fn pressed(&mut self, key: Key) -> bool {
        let Some(i) = self.key_presses.iter().position(|k| *k == key) else {
            return false;
        };
        self.key_presses.swap_remove(i);
        true
    }

    /// Number of activations this frame, with hold-to-repeat. The repeat
    /// state is per key, so actions sharing a key share its repeats.
    fn repeated(&mut self, key: Key, delta: f64) -> u32 {
        let pressed = self.pressed(key);
        let down = Input::singleton().is_key_pressed(key);
        let i = self.key_slot(key);
        self.keys[i]
            .1
            .tick_pressed(pressed, down, delta, REPEAT_DELAY, REPEAT_RATE)
    }

    fn shift_held(&self) -> bool {
        let input = Input::singleton();
        input.is_key_pressed(Key::SHIFT)
    }

    /// Once per frame, before any action is read: this frame's presses are
    /// the ones queued since the last.
    fn begin_input_frame(&mut self, delta: f64) {
        std::mem::swap(&mut self.key_presses, &mut self.key_queue);
        self.key_queue.clear();
        std::mem::swap(&mut self.pad_presses, &mut self.pad_queue);
        self.pad_queue.clear();
        self.g.touch.update(delta, REPEAT_DELAY, REPEAT_RATE);
    }

    fn handle_input(&mut self, delta: f64) {
        self.begin_input_frame(delta);
        self.handle_mode_input(delta);
        self.g.back_requested = false;
    }

    /// Something on screen is moving or being held, so every frame draws.
    fn busy(&self) -> bool {
        let stick = [
            (JoyAxis::LEFT_X, -1.0),
            (JoyAxis::LEFT_X, 1.0),
            (JoyAxis::LEFT_Y, -1.0),
            (JoyAxis::LEFT_Y, 1.0),
        ]
        .into_iter()
        .any(|(axis, sign)| self.stick_held(axis, sign));
        self.g.anim.is_some()
            || self.g.clear_hold > 0.0
            || self.g.status_time > 0.0
            || self.g.auto
            || self.g.touch.active()
            || stick
            || Input::singleton().is_anything_pressed()
    }

    /// Menus, overlays and the error screen run at `frame::IDLE_FPS`; play
    /// runs uncapped, or at whatever the project asked for. `Engine` max FPS
    /// rather than low-processor mode, which would also throttle play and
    /// the shot replay.
    fn apply_frame_cap(&mut self) {
        let idle = self.g.broken || self.g.mode != Mode::Playing;
        let cap = frame::frame_cap(idle, self.project_fps);
        if cap != self.fps_cap {
            self.fps_cap = cap;
            Engine::singleton().set_max_fps(cap);
        }
    }

    fn handle_mode_input(&mut self, delta: f64) {
        if self.pressed(Key::F1) {
            self.g.debug = !self.g.debug;
        }
        if self.action_pressed(15, "mute") {
            self.g.save.muted = !self.g.save.muted;
            self.apply_volume();
            self.g.settings.touch();
            self.note(if self.g.save.muted {
                "SOUND MUTED".into()
            } else {
                "SOUND ON".into()
            });
        }
        match self.g.mode {
            Mode::Title => {
                let resumable = self.g.resumable.is_some();
                let confirm = self.action_pressed(0, "confirm") || self.pressed(Key::KP_ENTER);
                let daily = if resumable {
                    self.action_pressed(11, "new_run")
                } else {
                    confirm
                };
                if resumable && confirm {
                    self.continue_run();
                } else if daily {
                    if self.confirm_discard("daily") {
                        let seed = daily_seed(Self::today());
                        self.play_sound(Sound::Ui);
                        self.start_run(seed, true);
                    }
                } else if self.action_pressed(17, "seed") {
                    if self.confirm_discard("seed") {
                        self.open_seed_entry();
                    }
                } else if self.g.last_crash.is_some() && self.action_pressed(4, "copy") {
                    self.copy_last_crash();
                } else if self.action_pressed(1, "controls") {
                    self.play_sound(Sound::Ui);
                    self.g.mode = Mode::Controls;
                } else if self.action_pressed(2, "back") {
                    self.base().get_tree().quit();
                }
                if self.g.mode != Mode::Title {
                    self.g.discard_armed = None;
                }
            }
            Mode::Controls => {
                let down = self.action_repeated(0, "left", delta);
                let up = self.action_repeated(1, "right", delta);
                if down + up > 0 {
                    let volume =
                        (self.g.save.volume as i32 + (up as i32 - down as i32) * 5).clamp(0, 100);
                    self.g.save.volume = volume as u8;
                    self.g.save.muted = false;
                    self.apply_volume();
                    self.g.settings.touch();
                    self.play_sound(Sound::Ui);
                }
                if self.g.touch.pressed("aim_mode") {
                    self.g.save.touch_aim = Some(self.g.touch_aim().next());
                    self.g.settings.touch();
                    self.play_sound(Sound::Ui);
                } else if self.g.touch.pressed("arc_hint") {
                    self.g.save.arc_hint = Some(self.g.arc_hint().next());
                    self.g.settings.touch();
                    self.play_sound(Sound::Ui);
                }
                if self.action_pressed(16, "last_arc") {
                    self.g.save.last_arc = Some(!self.g.last_arc_on());
                    self.g.settings.touch();
                    self.play_sound(Sound::Ui);
                }
                if self.action_pressed(3, "replay") {
                    self.g.save.bindings.clear();
                    self.g.settings.touch();
                    self.note("DEFAULT BINDINGS RESTORED".into());
                } else if self.action_pressed(2, "back") {
                    self.play_sound(Sound::Ui);
                    self.g.mode = Mode::Title;
                }
            }
            Mode::Paused => {
                if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.play_sound(Sound::Ui);
                    self.g.mode = Mode::Playing;
                } else if self.action_pressed(3, "replay") {
                    self.play_sound(Sound::Ui);
                    self.g.mode = Mode::Title;
                    self.suspend_run();
                }
            }
            Mode::SeedEntry => self.handle_seed_input(delta),
            Mode::GameOver => {
                if self.action_pressed(4, "copy") {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.g.last_seed);
                    DisplayServer::singleton().clipboard_set(&GString::from(seed.as_str()));
                    self.note(format!("COPIED {seed}"));
                } else if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.g.mode = Mode::Title;
                } else if self.action_pressed(3, "replay") {
                    let seed = self.g.last_seed;
                    let daily = self.g.daily;
                    self.start_run(seed, daily);
                }
            }
            Mode::Playing => self.handle_play_input(delta),
        }
    }

    /// Keyboard text comes in through `unhandled_input`. This is the touch
    /// keypad, and the controller moving a focus over the same keypad: A
    /// presses the focused key, B deletes, or leaves once nothing is typed.
    fn handle_seed_input(&mut self, delta: f64) {
        let dx = self.action_repeated(1, "right", delta) as i32
            - self.action_repeated(0, "left", delta) as i32;
        let dy = self.action_repeated(2, "down", delta) as i32
            - self.action_repeated(3, "up", delta) as i32;
        if dx != 0 || dy != 0 {
            self.g.seed_focus.step(dx, dy);
        }
        let key = if self.pad_action_pressed("confirm") {
            Some(self.g.seed_focus.action())
        } else if self.pad_action_pressed("back") {
            Some(if self.g.seed_text.is_empty() {
                "back"
            } else {
                keypad::DEL
            })
        } else if self.g.touch.pressed("back") || self.g.back_requested {
            Some("back")
        } else {
            ["confirm", keypad::DEL]
                .into_iter()
                .chain(keypad::KEYS)
                .find(|k| self.g.touch.pressed(k))
        };
        match key {
            Some("confirm") => self.start_seeded_run(),
            Some("back") => self.leave_seed_entry(),
            Some(keypad::DEL) => {
                self.g.seed_text.pop();
            }
            Some(k) if self.g.seed_text.chars().count() < 20 => self.g.seed_text.push_str(k),
            _ => {}
        }
    }

    fn handle_play_input(&mut self, delta: f64) {
        if self.action_pressed(2, "back") {
            self.g.touch.cancel_drag();
            self.g.mode = Mode::Paused;
            return;
        }
        let phase = self.g.run.as_ref().map(|r| r.phase);
        let Some(phase) = phase else { return };

        // A drag only aims on the screen it started on: drafting or a shot in
        // the air forgets it, so it can never drop into a later shot.
        if phase != Phase::Firing || self.g.anim.is_some() || self.g.clear_hold > 0.0 {
            self.g.touch.cancel_drag();
        }

        if phase == Phase::Drafting {
            for (i, action) in ["draft_1", "draft_2", "draft_3"].into_iter().enumerate() {
                if self.action_pressed(5 + i, action) {
                    let taken = self.g.run.as_mut().and_then(|r| r.take_offer(i));
                    if let Some(m) = taken {
                        self.preview.clear();
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
        if self.g.anim.is_some() || self.g.clear_hold > 0.0 {
            return;
        }

        let quantum = self
            .g
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
            self.g.angle_ticks = (self.g.angle_ticks + (right - left) * quantum * mult)
                .clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, "down", delta) as i32;
        let up = self.action_repeated(3, "up", delta) as i32;
        if up + down != 0 {
            self.learn(LEARN_POWER);
            self.g.power_ticks =
                (self.g.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }

        // Airburst fuse.
        let fuse_step = 4.0 * mult as f64;
        let minus = self.action_repeated(4, "fuse_down", delta) as f64;
        let plus = self.action_repeated(5, "fuse_up", delta) as f64;
        if minus + plus > 0.0 {
            if let Some(r) = self.g.run.as_mut() {
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
            let ok = self.g.run.as_mut().map(|r| {
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

        if self.g.drag_aiming() {
            self.handle_drag_aim(quantum);
        }
    }

    /// Pull back to aim, let go to fire. The aim follows the finger live, so
    /// the readouts and barrel show exactly what a release would fire; pulling
    /// back inside the dead zone puts the old aim back and cancels.
    fn handle_drag_aim(&mut self, quantum: i32) {
        let dropped = self.g.touch.dropped();
        let Some((start, end)) = dropped.or(self.g.touch.drag()) else {
            // Abandoned (paused, focus lost): put the old aim back.
            if let Some(before) = self.g.drag_from.take() {
                (self.g.angle_ticks, self.g.power_ticks) = before;
            }
            return;
        };
        let before = *self
            .g
            .drag_from
            .get_or_insert((self.g.angle_ticks, self.g.power_ticks));
        let aim = crate::aim::pull_to_aim(start, end, quantum);
        match aim {
            Some((angle, power)) => {
                self.learn(LEARN_ANGLE);
                self.learn(LEARN_POWER);
                (self.g.angle_ticks, self.g.power_ticks) = (angle, power);
            }
            None => (self.g.angle_ticks, self.g.power_ticks) = before,
        }
        if dropped.is_some() {
            self.g.drag_from = None;
            if aim.is_some() {
                self.fire();
            }
        }
    }

    // ------------------------------------------------------------- run steps

    fn fire(&mut self) {
        let learned = self.g.save.learned;
        if learned & LEARN_WIND != 0 {
            self.learn(LEARN_SHELLS);
        } else if learned & LEARN_CRATER != 0
            && self
                .g
                .run
                .as_ref()
                .and_then(|r| r.displayed_wind())
                .is_some_and(|w| w.abs() > 0.01)
        {
            self.learn(LEARN_WIND);
        }
        let shot = Shot::new(self.g.angle_ticks, self.g.power_ticks);
        let Some(run) = self.g.run.as_mut() else {
            return;
        };
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
        self.g.last_arc = report
            .traces
            .first()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.g.anim = Some(Anim {
            traces: report.traces,
            before,
            t: 0.0,
            longest,
            collapsed,
            settled: 0.0,
            impact_played: false,
            collapse_played: false,
        });
        let shells = self.g.run.as_ref().map(|r| r.shells).unwrap_or(99);
        if shells < 10 && !self.g.low_shell_warned {
            self.g.low_shell_warned = true;
            self.play_sound(Sound::LowShell);
            self.note("SHELLS ARE YOUR RUN CLOCK".into());
        }
    }

    fn step(&mut self, delta: f64) {
        if self.g.mode != Mode::Playing {
            return;
        }

        // Advance the shot replay. The sim ran at a fixed 1/120; this only
        // walks the recorded path, so playback speed cannot affect anything.
        let mut sounds = Vec::new();
        let mut learned_impact = false;
        if let Some(anim) = self.g.anim.as_mut() {
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
                self.g.anim = None;
                let cleared = self.g.run.as_ref().map(|r| r.phase) == Some(Phase::Cleared);
                if cleared {
                    self.g.clear_hold = CLEAR_HOLD;
                } else if self.g.run.as_ref().map(|r| r.is_over()) == Some(true) {
                    self.finish_run();
                }
            }
        } else if self.g.clear_hold > 0.0 {
            // Hold on the cleared screen and pan forward, then move on.
            self.g.clear_hold -= delta;
            let span = (self.g.world_width() - self.g.buffer_w).max(0) as f64;
            let t = 1.0 - (self.g.clear_hold / CLEAR_HOLD).clamp(0.0, 1.0);
            self.g.cam_target = span * t;
            if self.g.clear_hold <= 0.0 {
                self.g.clear_hold = 0.0;
                self.g.cam_target = 0.0;
                self.g.cam_x = 0;
                let advanced = self.g.run.as_mut().map(|r| r.resolve_clear()) == Some(true);
                if advanced {
                    self.after_screen_change();
                }
            }
        } else if let Some(run) = self.g.run.as_ref() {
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
        if self.g.anim.is_some() {
            let head = self.g.shell_head();
            if let Some(p) = head {
                let span = (self.g.world_width() - self.g.buffer_w).max(0) as f64;
                if span > 0.0 {
                    self.g.cam_target = (p.x - self.g.buffer_w as f64 * 0.5).clamp(0.0, span);
                }
            }
        }

        // The camera is quantized to a whole pixel every frame. Panning with a
        // fractional camera is what makes low-res terrain shimmer.
        let span = (self.g.world_width() - self.g.buffer_w).max(0) as f64;
        self.g.cam_x = self.g.cam_target.round().clamp(0.0, span) as i32;
    }

    fn after_screen_change(&mut self) {
        self.apply_buffer_size();
        if self.g.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            self.play_sound(Sound::Draft);
        }
    }

    fn finish_run(&mut self) {
        // The run file goes before the score is recorded, never after: a
        // crash in between costs this score, where the other order would
        // record it twice (the next launch scores a finished log).
        let line = self
            .g
            .run
            .as_ref()
            .filter(|_| self.g.run_persist)
            .map(|r| resume::Saved::of(r, self.g.daily).to_line());
        self.stop_persisting();
        if let Some(run) = self.g.run.as_ref() {
            let (screens, shells, shots) = run.score();
            let entry = Entry {
                seed: run.run_seed,
                screens,
                shells,
                shots,
                daily: self.g.daily,
            };
            if !save::record(&mut self.g.save, entry).saved {
                // Not on the board (a save from a newer build, a full disk):
                // keep the finished log, so a later launch can score it.
                if let Some(line) = line {
                    save::write(RUN_PATH, RUN_TEMP_PATH, &line);
                }
                self.note_not_saved("SCORE NOT SAVED");
            }
        }
        self.g.mode = Mode::GameOver;
    }

    // --------------------------------------------------------------- drawing

    fn render(&mut self) {
        self.touch_buttons.clear();
        match self.g.mode {
            Mode::Playing | Mode::Paused => self.render_play(),
            _ => self.render_menu(),
        }
        self.upload();
    }

    /// Hand the buffer to the GPU without allocating: the frame is copied
    /// into whichever of the two upload buffers the image is not holding, the
    /// image takes it by reference, and the texture is updated in place. Only
    /// a change of buffer size makes a new texture.
    fn upload(&mut self) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let format = godot::classes::image::Format::RGBA8;
        let bytes = self.canvas.bytes();
        let buf = &mut self.upload_bufs[self.upload_flip];
        if buf.len() != bytes.len() {
            buf.resize(bytes.len());
        }
        buf.as_mut_slice().copy_from_slice(bytes);
        match self.image.as_mut() {
            Some(image) => image.set_data(w, h, false, format, buf),
            None => self.image = Image::create_from_data(w, h, false, format, buf),
        }
        self.upload_flip ^= 1;
        let Some(image) = self.image.as_ref() else {
            return;
        };
        match self.texture.as_mut() {
            Some(t) if t.get_width() == w && t.get_height() == h => t.update(image),
            _ => {
                self.texture = ImageTexture::create_from_image(image);
                self.base_mut().queue_redraw();
            }
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
        self.g.auto_cooldown -= delta;
        if self.g.mode == Mode::Title {
            let seed = Self::seed_arg().unwrap_or_else(|| daily_seed(Self::today()));
            self.start_run(seed, false);
            return;
        }
        if self.g.mode != Mode::Playing || self.g.anim.is_some() || self.g.clear_hold > 0.0 {
            return;
        }
        if self.g.auto_cooldown > 0.0 {
            return;
        }
        let phase = self.g.run.as_ref().map(|r| r.phase);
        match phase {
            Some(Phase::Drafting) => {
                if self.g.auto_hold_draft {
                    return;
                }
                self.g.run.as_mut().map(|r| r.take_offer(0));
                self.apply_buffer_size();
                self.g.auto_cooldown = 0.4;
            }
            Some(Phase::Cleared) => {
                self.g.clear_hold = CLEAR_HOLD;
            }
            Some(Phase::Firing) => {
                let first = self.g.run.as_ref().map(|r| r.shots_on_screen) == Some(0);
                if self.g.auto_miss && first {
                    // A high lob that lands short, well clear of any target.
                    self.g.angle_ticks = 1400;
                    self.g.power_ticks = 330;
                    self.fire();
                    self.g.auto_cooldown = 0.35;
                    return;
                }
                let pick = {
                    let Some(run) = self.g.run.as_ref() else {
                        return;
                    };
                    let target = run.screen.structures.iter().position(|s| !s.is_down());
                    target.and_then(|t| {
                        salt_line_sim::solver::solve_target(&run.screen, &run.loadout, t)
                            .first()
                            .map(|c| (c.shot, t))
                    })
                };
                if let Some((shot, target)) = pick {
                    self.g.angle_ticks = shot.angle_ticks;
                    self.g.power_ticks = shot.power_ticks;
                    if let Some(run) = self.g.run.as_mut() {
                        if let Some(f) = run.screen.default_fuse(target) {
                            run.set_fuse(f);
                        }
                    }
                    self.fire();
                    self.g.auto_cooldown = 0.35;
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
        let width = self.g.buffer_w as usize;
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
            self.g.run = Some(run.clone());
            self.g.mode = Mode::Playing;
            self.g.status_time = 0.0;
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
            self.g.run = Some(run.clone());
            self.render_play();
            self.save_canvas(&format!("{dir}/damaged_{}.png", t.name().to_lowercase()));
        }
        self.base().get_tree().quit();
    }

    fn render_menu(&mut self) {
        let pal = self.pal.clone();
        let view = self.g.view();
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

        match self.g.mode {
            Mode::Title => {
                self.centred(y, "ARTILLERY ROGUELIKE", dim);
                y += 14;
                let confirm = self.input_label("confirm");
                // With a saved run, confirm continues it and a new run is a
                // separate action that asks twice (`confirm_discard`).
                let again = "PRESS AGAIN - SAVED RUN IS LOST";
                if self.g.resumable.is_some() {
                    y = self.menu_row(
                        y,
                        "confirm",
                        &format!("{confirm}  CONTINUE"),
                        "CONTINUE",
                        alert,
                    );
                    let key = self.input_label("new_run");
                    let text = if self.g.discard_armed == Some("daily") {
                        again
                    } else {
                        "NEW DAILY RUN"
                    };
                    y = self.menu_row(y, "new_run", &format!("{key}      {text}"), text, fg);
                } else {
                    y = self.menu_row(
                        y,
                        "confirm",
                        &format!("{confirm}  DAILY RUN"),
                        "DAILY RUN",
                        fg,
                    );
                }
                let seed = self.input_label("seed");
                let seed_text = if self.g.discard_armed == Some("seed") {
                    again
                } else {
                    "SEED ENTRY"
                };
                y = self.menu_row(
                    y,
                    "seed",
                    &format!("{seed}      {seed_text}"),
                    seed_text,
                    fg,
                );
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
                if self.g.last_crash.is_some() {
                    let text = if self.g.crash_copied {
                        "CRASH REPORT COPIED"
                    } else {
                        "COPY LAST CRASH REPORT"
                    };
                    let copy = self.input_label("copy");
                    y = self.menu_row(y, "copy", &format!("{copy}      {text}"), text, alert);
                }
                y += 6;
                if self.g.status_time > 0.0 {
                    let text = self.g.status.clone();
                    self.centred(y, &text, alert);
                    y += 10;
                }
                let best = self.g.save.best().copied();
                if let Some(b) = best {
                    self.centred(
                        y,
                        &format!("BEST {} SCREENS  SEED {}", b.screens, b.seed),
                        alert,
                    );
                    y += 10;
                }
                // The build is not selling anything, and says so where a
                // player will see it before they start a run.
                self.centred(y, "DEVELOPMENT PREVIEW - NOT FOR SALE", alert);
                y += 8;
                let room = ((self.canvas.h - y - 12) / 7).max(0) as usize;
                let n = self.g.save.scores.len().min(5).min(room);
                if n > 0 {
                    y += 4;
                    self.centred(y, "LOCAL BOARD", dim);
                    y += 8;
                    for i in 0..n {
                        let e = self.g.save.scores[i];
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
            Mode::Controls if self.g.touch_active => {
                self.centred(y, "CONTROLS / AUDIO", fg);
                y += 16;
                let muted = if self.g.save.muted { " MUTED" } else { "" };
                let text = format!("VOLUME {}%{muted}", self.g.save.volume);
                let tw = crate::font::small_width(&text) as i32;
                let tx = (w - tw) / 2;
                self.canvas.text_small(tx, y, &text, alert);
                self.draw_touch_button(tx - 26, y - 6, 20, 17, "-", "left");
                self.draw_touch_button(tx + tw + 6, y - 6, 20, 17, "+", "right");
                y += 20;
                let mute = if self.g.save.muted { "UNMUTE" } else { "MUTE" };
                y = self.menu_row(y, "mute", "", mute, fg);
                let last = if self.g.last_arc_on() {
                    "LAST ARC: ON"
                } else {
                    "LAST ARC: OFF"
                };
                y = self.menu_row(y, "last_arc", "", last, fg);
                let aim = match self.g.touch_aim() {
                    TouchAim::Buttons => "AIM: BUTTONS",
                    TouchAim::Drag => "AIM: DRAG",
                };
                y = self.menu_row(y, "aim_mode", "", aim, fg);
                if self.g.touch_aim() == TouchAim::Drag {
                    let arc = match self.g.arc_hint() {
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
                let muted = if self.g.save.muted { " MUTED" } else { "" };
                self.centred(
                    y,
                    &format!("< >  VOLUME {}%{muted}", self.g.save.volume),
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
                let on = if self.g.last_arc_on() { "ON" } else { "OFF" };
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
                let text = self.g.seed_text.clone();
                let shown = format!("> {text}");
                self.centred(y, &shown, fg);
                y += 10;
                if self.plated() {
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
                    let prior = self.g.save.best_on(seed).copied();
                    if let Some(p) = prior {
                        self.centred(y, &format!("YOUR BEST HERE {} SCREENS", p.screens), dim);
                    }
                }
            }
            Mode::GameOver => {
                self.centred(y, "OUT OF SHELLS", alert);
                y += 14;
                if let Some(run) = self.g.run.as_ref() {
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
                if self.g.status_time > 0.0 {
                    let text = self.g.status.clone();
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
        let c = if self.lit(action) {
            pal.get(slot::ALERT)
        } else {
            pal.get(slot::UI_FG)
        };
        if self.focused(action) {
            self.canvas
                .rect_outline(x - 1, y - 1, w + 2, h + 2, pal.get(slot::ALERT));
        }
        let tx = x + (w - crate::font::small_width(label) as i32) / 2;
        let ty = y + (h - crate::font::SMALL_H as i32) / 2;
        self.canvas.text_small(tx, ty, label, c);
        self.touch_button(Rect::new(x, y, w, h), action);
    }

    /// The seed keypad: letters and digits in the game's own font, since a seed
    /// is any text and the platform keyboard is not dependable inside a game
    /// view. Returns the y below it.
    fn draw_seed_keypad(&mut self, top: i32) -> i32 {
        let (kw, kh, pitch, del_w) = (18, 15, 20, 36);
        let mut y = top;
        for keys in &keypad::ROWS[..keypad::KEY_ROWS] {
            let row_w = keys
                .iter()
                .map(|&k| if k == keypad::DEL { del_w + 2 } else { pitch })
                .sum::<i32>()
                - 2;
            let mut x = (self.canvas.w - row_w) / 2;
            for &k in keys.iter() {
                if k == keypad::DEL {
                    self.draw_touch_button(x, y, del_w, kh, "DEL", k);
                } else {
                    self.draw_touch_button(x, y, kw, kh, k, k);
                    x += pitch;
                }
            }
            y += kh + 2;
        }
        y
    }

    /// Menu lines are plates rather than "KEY  TEXT": on a touchscreen, and
    /// for a controller on the seed keypad, where the focus moves over them.
    fn plated(&self) -> bool {
        self.g.touch_active || (self.g.controller_active && self.g.mode == Mode::SeedEntry)
    }

    /// The controller's keypad focus is on this action's plate.
    fn focused(&self, action: &str) -> bool {
        self.g.mode == Mode::SeedEntry
            && self.g.controller_active
            && !self.g.touch_active
            && self.g.seed_focus.action() == action
    }

    /// A plate's label lights while a finger is on it or the focus is.
    fn lit(&self, action: &str) -> bool {
        self.g.touch.held(action) || self.focused(action)
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
        if !self.plated() {
            self.centred(y, keyed, c);
            return y + 8;
        }
        let pal = self.pal.clone();
        let tw = crate::font::small_width(text) as i32;
        let pad = 10;
        let x = (self.canvas.w - tw) / 2 - pad;
        self.canvas.plate(x, y - 4, tw + 2 * pad, 13, &pal);
        if self.focused(action) {
            self.canvas
                .rect_outline(x - 1, y - 5, tw + 2 * pad + 2, 15, pal.get(slot::ALERT));
        }
        let c = if self.lit(action) {
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
        let (ex, ey) = self.g.corner_pad;
        let (bw, bh) = (22, 18);
        let y = h - bh - 3 - ey;
        // Right of the ANG/PWR readouts in the bottom-left corner.
        let x0 = 44 + ex;
        // Drag aiming: the open screen is the control, so only the modifier
        // buttons stay, dropped to the bottom row.
        let y2 = if self.g.drag_aiming() {
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
        let view = self.g.view();
        let Some(screen) = self.g.shown_screen().cloned() else {
            return;
        };
        let Some(run) = self.g.run.as_ref() else {
            return;
        };
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
        if self.g.anim.is_none() && apex.is_some() && self.g.last_arc_on() {
            raster::draw_last_arc(&mut self.canvas, &view, &self.g.last_arc, &pal);
        }

        self.draw_structures(&screen, &view);
        self.draw_player(&screen, &loadout, &view);

        // The shot in flight.
        if let Some(anim) = self.g.anim.as_ref() {
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
        if self.g.anim.is_none() && phase == Phase::Firing && self.g.mode == Mode::Playing {
            self.draw_drag_preview(&screen, &loadout, wind_shown, &view);
        }

        self.render_hud(
            shells, index, wind_shown, range, fuse, &loadout, &screen, phase,
        );
        if self.g.touch_active && self.g.mode == Mode::Playing && phase != Phase::Drafting {
            self.draw_play_controls(&loadout);
        }

        if phase == Phase::Drafting {
            self.render_draft(&offers, &loadout);
        }
        if self.g.mode == Mode::Paused {
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
        if !self.g.drag_aiming() {
            return;
        }
        let Some((start, now)) = self.g.touch.drag() else {
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
        if self.g.drag_from.is_none()
            || crate::aim::pull_to_aim(start, now, loadout.angle_quantum_ticks).is_none()
        {
            return;
        }

        let hint = self.g.arc_hint();
        let wind = match hint {
            ArcHint::Wind => wind_shown.unwrap_or(0.0),
            ArcHint::Short | ArcHint::Full => 0.0,
        };
        let Some(run) = self.g.run.as_ref() else {
            return;
        };
        // A whole ballistic flight per frame while a finger is held still is
        // wasted: the path only changes when one of these does.
        let key = PreviewKey {
            angle: self.g.angle_ticks,
            power: self.g.power_ticks,
            hint,
            wind: wind.to_bits(),
            seed: run.run_seed,
            index: run.screen_index,
            shots: run.shots_on_screen,
            offset: run.emplacement_offset.to_bits(),
        };
        let shot = Shot::new(self.g.angle_ticks, self.g.power_ticks);
        let (path, upto) = self
            .preview
            .get_or(key, || preview_path(screen, loadout, shot, hint, wind));
        raster::draw_trajectory(&mut self.canvas, view, path, *upto, &pal);
    }

    fn draw_structures(&mut self, screen: &Screen, view: &View) {
        let toppling = self
            .g
            .anim
            .as_ref()
            .map(|a| a.done() && a.settled < TOPPLE_TIME && !a.collapsed.is_empty())
            .unwrap_or(false);
        let collapsed_now: Vec<usize> = self
            .g
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
        let shot = Shot::new(self.g.angle_ticks, self.g.power_ticks).quantized(loadout);
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
        let Some(anim) = self.g.anim.as_ref() else {
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
        if phase == Phase::Drafting && self.g.save.learned & LEARN_DRAFT == 0 && self.g.touch_active
        {
            return Some("TAP A CARD  PICK A MODIFIER".into());
        }
        if phase == Phase::Drafting && self.g.save.learned & LEARN_DRAFT == 0 {
            return Some(format!(
                "{} {} {}  PICK A MODIFIER",
                self.input_label("draft_1"),
                self.input_label("draft_2"),
                self.input_label("draft_3")
            ));
        }
        let learned = self.g.save.learned;
        if self.g.drag_aiming()
            && learned & (LEARN_ANGLE | LEARN_POWER | LEARN_FIRE)
                != LEARN_ANGLE | LEARN_POWER | LEARN_FIRE
        {
            return Some(if self.g.touch.drag().is_some() {
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
            if self.g.touch_active {
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
        let (ex, ey) = self.g.corner_pad;

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
        let shot = Shot::new(self.g.angle_ticks, self.g.power_ticks).quantized(loadout);
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
                .g
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

        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
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
        if self.g.clear_hold > 0.0 {
            let text = "SCREEN CLEAR";
            let x = (w - crate::font::small_width(text) as i32) / 2;
            self.canvas.text_small(x, 34 + ey, text, fg);
        }

        if self.g.debug {
            let m = screen.field.material_at(screen.muzzle().x);
            let lines = [
                format!("TPL {}", screen.template.name()),
                format!("CND {}", screen.candidates.min(999)),
                format!("ATT {}", screen.attempts),
                format!("MAT {}", raster::material_name(m)),
                format!("CAM {}", self.g.cam_x),
                format!("BUF {}X{}", self.g.buffer_w, self.g.buffer_h),
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
            if self.g.touch_active {
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

        let hint = if self.g.touch_active {
            "TAP A CARD"
        } else {
            "1 2 3  CHOOSE"
        };
        let hx = (w - crate::font::small_width(hint) as i32) / 2;
        self.canvas.text_small(hx, top + card_h + 6, hint, fg);
    }

    fn render_pause(&mut self) {
        let pal = self.pal.clone();
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let (w, h) = (self.canvas.w, self.canvas.h);
        if self.g.touch_active {
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

/// The drag preview's flight, and how much of it the arc hint shows.
fn preview_path(
    screen: &Screen,
    loadout: &Loadout,
    shot: Shot,
    hint: ArcHint,
    wind: f64,
) -> Preview {
    let boxes = screen.target_boxes();
    let mut world = screen.world(&boxes);
    world.wind = wind;
    let opts = FireOptions {
        record_path: true,
        ..FireOptions::default()
    };
    let mut traces = ballistics::fire(&world, screen.muzzle(), shot, loadout, &opts);
    if traces.is_empty() {
        return (Vec::new(), 0);
    }
    let path = traces.swap_remove(0).path;
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
    (path, upto)
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

// ------------------------------------------------- settings and crash reports

impl SaltLine {
    /// Write pending settings once they settle, or as soon as the player
    /// leaves Controls. Not every volume repeat: see `durable::Debounce`.
    fn tick_settings(&mut self, delta: f64) {
        if self.g.settings.tick(delta, self.g.mode == Mode::Controls) {
            self.write_settings();
        }
    }

    /// Write pending settings now, if there are any: focus loss, pause, close.
    fn flush_settings(&mut self) {
        if self.g.settings.take() {
            self.write_settings();
        }
    }

    fn write_settings(&mut self) {
        // After a panic nothing in memory is trusted enough to write.
        if crash::panicked() {
            return;
        }
        if !save::store(&self.g.save) {
            self.note_not_saved("SETTINGS NOT SAVED");
        }
    }

    /// Say a write failed. A save from a newer build is never written by this
    /// one (by design, `save::store`), so every change would fail; that gets
    /// one note saying why, once a session, instead of a complaint each time.
    fn note_not_saved(&mut self, what: &str) {
        if !self.g.save.from_the_future {
            self.note(what.into());
        } else if !self.g.future_noted {
            self.g.future_noted = true;
            self.note("SAVE IS FROM A NEWER VERSION".into());
        }
    }

    /// Keep the crash hook's picture of the run current. Taken before this
    /// frame's input, so a panic inside `fire` still reports the aim it fired.
    fn note_crash_context(&self) {
        crash::note(
            self.g.mode_name(),
            self.g.run.as_ref(),
            (self.g.angle_ticks, self.g.power_ticks),
            self.g.resumed,
        );
    }

    /// Pick up a report the last session left, for the title screen to offer.
    fn find_last_crash(&mut self) {
        let Some(path) = save::on_disk(crash::REPORT_PATH) else {
            return;
        };
        crash::set_report_file(path.clone());
        self.g.last_crash = std::fs::read_to_string(path)
            .ok()
            .filter(|t| !t.trim().is_empty());
    }

    fn copy_last_crash(&mut self) {
        let Some(text) = self.g.last_crash.clone() else {
            return;
        };
        DisplayServer::singleton().clipboard_set(&GString::from(text.as_str()));
        self.play_sound(Sound::Ui);
        if !self.g.crash_copied {
            self.g.crash_copied = true;
            self.retire_crash_file();
        }
    }

    /// The player has moved on from last session's report (started a run):
    /// stop offering it. The file is kept as `crash-old.txt`, not deleted.
    fn retire_last_crash(&mut self) {
        if self.g.last_crash.take().is_some() && !self.g.crash_copied {
            self.retire_crash_file();
        }
    }

    fn retire_crash_file(&self) {
        if let (Some(from), Some(to)) = (
            save::on_disk(crash::REPORT_PATH),
            save::on_disk(crash::OLD_REPORT_PATH),
        ) {
            if let Err(e) = std::fs::rename(&from, &to) {
                godot_warn!("salt-line: could not move {}: {e}", crash::REPORT_PATH);
            }
        }
    }

    /// A panic has happened somewhere in the Rust side. gdext caught it, but
    /// the state it left is not to be trusted, so the game stops here: no sim
    /// steps, no saves (`save::store` refuses too), just the report and a
    /// way out.
    fn broken_frame(&mut self, delta: f64) {
        // Wall clock still runs, for `--salt-shot-at` captures of this screen.
        self.elapsed += delta;
        if !self.g.broken {
            // Whatever the game last said no longer applies.
            self.g.broken = true;
            self.g.status_time = 0.0;
        }
        self.g.status_time = (self.g.status_time - delta).max(0.0);
        self.begin_input_frame(delta);
        self.apply_frame_cap();
        if self.action_pressed(4, "copy") {
            // Only a finished report: a panic off the main thread can stop the
            // game while the hook is still writing, and half a report copied
            // looks like a whole one.
            if crash::report_ready() {
                let text = crash::report()
                    .unwrap_or_else(|| "SALT LINE CRASH REPORT\n(report unavailable)\n".into());
                DisplayServer::singleton().clipboard_set(&GString::from(text.as_str()));
                self.note("REPORT COPIED".into());
            } else {
                self.note("REPORT NOT READY - TRY AGAIN".into());
            }
        } else if self.action_pressed(2, "back") {
            self.base().get_tree().quit();
        }
        self.g.back_requested = false;

        self.touch_buttons.clear();
        let pal = self.pal.clone();
        self.canvas.fill(pal.get(slot::UI_BG));
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        let mut y = 30;
        self.centred(y, "SOMETHING BROKE", alert);
        y += 14;
        self.centred(y, "THE GAME STOPPED RATHER THAN CARRY ON WRONG.", fg);
        y += 8;
        self.centred(
            y,
            "A REPORT WAS SAVED ON THIS DEVICE. NOTHING WAS SENT.",
            dim,
        );
        y += 8;
        self.centred(y, "PASTE IT INTO A BUG REPORT ON GITHUB.", dim);
        y += 16;
        let copy = self.input_label("copy");
        y = self.menu_row(
            y,
            "copy",
            &format!("{copy}      COPY REPORT"),
            "COPY REPORT",
            fg,
        );
        let back = self.input_label("back");
        y = self.menu_row(y, "back", &format!("{back}  QUIT"), "QUIT", fg);
        y += 4;
        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
            self.centred(y, &text, alert);
        }
        self.upload();
        self.maybe_screenshot();
        self.base_mut().queue_redraw();
    }
}

// ------------------------------------------------------- lifecycle and resume

impl SaltLine {
    /// Focus lost, or the operating system paused the app: stop the clock on
    /// a live screen, so a shot in the air or a clear in progress is still
    /// there to watch on return. Not during a draft, where nothing moves and
    /// the cards themselves say where the player was; a pause screen over
    /// them would only cost a tap. Not for the dev autopilot or screenshots,
    /// which nobody is watching with focus.
    fn auto_pause(&mut self) {
        if self.g.mode != Mode::Playing || self.g.auto || self.shot_path.is_some() {
            return;
        }
        if self.g.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            return;
        }
        self.g.touch.cancel_drag();
        self.g.mode = Mode::Paused;
        // From a notification, outside the frame's before/after compare.
        self.redraw.mark();
    }

    /// Size the buffer for a run: the recorded width for a resumed one, the
    /// device's for anything else. A resumed run generated at another width
    /// keeps it (DESIGN §9.2: a replay must use it), shown letterboxed
    /// instead of stretched across the window.
    fn set_run_width(&mut self, width: Option<usize>) {
        let device = self.g.pick_width();
        let want = width.map_or(device, |w| w as i32);
        self.g.buffer_w = want;
        self.g.kept_width = want != device;
        let aspect = if want == device {
            ContentScaleAspect::EXPAND
        } else {
            ContentScaleAspect::KEEP
        };
        if let Some(mut window) = self.base().get_window() {
            window.set_content_scale_aspect(aspect);
        }
    }

    /// A new run replaces whatever was saved: it is written on the next
    /// frame, log and all (empty, at the start).
    fn begin_persisting(&mut self) {
        self.g.resumable = None;
        self.g.discard_armed = None;
        self.g.run_saved_len = None;
        self.g.run_persist = true;
        self.g.resumed = false;
    }

    /// The run is over and on the board: nothing is left to resume.
    fn stop_persisting(&mut self) {
        if self.g.run_persist {
            self.discard_run_file();
        }
        self.g.run_persist = false;
        self.g.resumable = None;
    }

    /// Make sure no run file is left to resume or score. If a file cannot be
    /// deleted, the run file is overwritten with nothing, which `judge` reads
    /// as no run: a finished log must never survive to be scored twice.
    fn discard_run_file(&self) {
        save::remove(RUN_TEMP_PATH);
        save::remove(RUN_PATH);
        if save::read(RUN_PATH).is_some() || save::read(RUN_TEMP_PATH).is_some() {
            godot_warn!("salt-line: could not delete {RUN_PATH}; blanking it");
            save::write(RUN_PATH, RUN_TEMP_PATH, "");
        }
    }

    /// A new run replaces the saved one, so over a saved run the first press
    /// of a new-run action only asks, and the second goes ahead. Returns
    /// whether to go ahead.
    fn confirm_discard(&mut self, what: &'static str) -> bool {
        if self.g.resumable.is_none() || self.g.discard_armed == Some(what) {
            self.g.discard_armed = None;
            return true;
        }
        self.g.discard_armed = Some(what);
        self.play_sound(Sound::Ui);
        self.redraw.mark();
        false
    }

    /// Write `user://run.txt` whenever the log has grown: a shot fired or a
    /// draft taken (a reposition rides in the next shot's entry). Checked
    /// every frame, written only on a change.
    fn persist_run(&mut self) {
        if !self.g.run_persist || crash::panicked() {
            return;
        }
        let Some(run) = self.g.run.as_ref() else {
            return;
        };
        let len = run.log.len();
        if self.g.run_saved_len == Some(len) {
            return;
        }
        let line = resume::Saved::of(run, self.g.daily).to_line();
        // Tried once per change: a failed write is logged by `save::write`,
        // and the next shot tries again.
        save::write(RUN_PATH, RUN_TEMP_PATH, &line);
        self.g.run_saved_len = Some(len);
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
        if self.g.run.as_ref().is_some_and(|r| r.is_over()) {
            self.g.anim = None;
            self.g.clear_hold = 0.0;
            self.finish_run();
            return;
        }
        if !self.g.run_persist {
            return;
        }
        self.g.resumable = self
            .g
            .run
            .as_ref()
            .map(|r| resume::Saved::of(r, self.g.daily));
    }

    /// At launch: offer the saved run, score one that had already ended, or
    /// throw away one this build must not replay.
    ///
    /// The file is moved aside to `RUN_LOADING_PATH` while it is rebuilt and
    /// only put back once the rebuild succeeds, so a log that brings the sim
    /// down costs one crash, not one every launch.
    fn find_saved_run(&mut self) {
        if save::read(RUN_LOADING_PATH).is_some() {
            // The last launch died rebuilding it.
            godot_warn!("salt-line: discarding {RUN_LOADING_PATH}: a rebuild did not finish");
            save::remove(RUN_LOADING_PATH);
            self.discard_run_file();
            self.note(resume::Stale::Diverged.note().into());
            self.g.status_time = 6.0;
            return;
        }
        let (path, text) = match save::read(RUN_PATH) {
            Some(t) => (RUN_PATH, t),
            None => match save::read(RUN_TEMP_PATH) {
                Some(t) => (RUN_TEMP_PATH, t),
                None => return,
            },
        };
        if text.trim().is_empty() {
            return;
        }
        save::rename(path, RUN_LOADING_PATH);
        match resume::judge(Some(&text)) {
            Found::Nothing => {
                save::rename(RUN_LOADING_PATH, RUN_PATH);
            }
            Found::Continue(saved) => {
                save::rename(RUN_LOADING_PATH, RUN_PATH);
                self.g.resumable = Some(saved);
            }
            Found::Finished(saved, run) => {
                // Gone before the score goes on the board, as in `finish_run`:
                // a crash in between loses it rather than recording it twice.
                save::remove(RUN_LOADING_PATH);
                self.discard_run_file();
                let (screens, shells, shots) = run.score();
                let entry = Entry {
                    seed: saved.seed,
                    screens,
                    shells,
                    shots,
                    daily: saved.daily,
                };
                if save::record(&mut self.g.save, entry).saved {
                    self.note("LAST RUN SCORED".into());
                } else {
                    // Kept for a launch that can record it.
                    save::write(RUN_PATH, RUN_TEMP_PATH, &text);
                    self.note_not_saved("SCORE NOT SAVED");
                }
            }
            Found::Discard(why) => {
                godot_warn!("salt-line: discarding {RUN_PATH}: {why:?}");
                save::remove(RUN_LOADING_PATH);
                self.discard_run_file();
                self.note(why.note().into());
                self.g.status_time = 6.0;
            }
        }
    }

    /// Rebuild the saved run and drop the player back in where it stood.
    fn continue_run(&mut self) {
        let Some(saved) = self.g.resumable.take() else {
            return;
        };
        let Some(run) = saved.reconstruct() else {
            self.discard_run_file();
            self.note(resume::Stale::Diverged.note().into());
            return;
        };
        self.set_run_width(Some(saved.width));
        (self.g.angle_ticks, self.g.power_ticks) = saved.aim().unwrap_or((900, 600));
        self.g.anim = None;
        self.g.last_arc.clear();
        self.g.clear_hold = 0.0;
        self.g.cam_x = 0;
        self.g.cam_target = 0.0;
        self.g.daily = saved.daily;
        self.g.last_seed = saved.seed;
        self.g.low_shell_warned = run.shells < 10;
        self.retire_last_crash();
        let screen = run.screen_index;
        // What is on disk is this run, as rebuilt.
        self.g.run_saved_len = Some(run.log.len());
        self.g.run_persist = true;
        self.g.resumed = true;
        self.g.run = Some(run);
        self.g.mode = Mode::Playing;
        self.preview.clear();
        self.redraw.mark();
        self.play_sound(Sound::Ui);
        // Buffer size for the loadout.
        self.after_screen_change();
        self.note(format!(
            "SCREEN {}  SEED {}",
            screen + 1,
            format_seed(saved.seed)
        ));
    }
}

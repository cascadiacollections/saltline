//! The game node: the adapter between Godot and the engine-free core.
//!
//! Everything that decides anything lives in `app` (the game) and
//! `salt-line-sim` (the rules); see `app` for how the three fit. This node
//! gathers the engine's input into `input::Intents`, steps the core, carries
//! out the `app::Effect`s it returns (audio, `user://` files, the clipboard,
//! the window), and puts each frame on screen as one texture.

use godot::classes::notify::CanvasItemNotification;
use godot::classes::{
    window::ContentScaleAspect, AudioStream, AudioStreamPlayer, DisplayServer, Engine, INode2D,
    Image, ImageTexture, Input, InputEvent, InputEventJoypadButton, InputEventJoypadMotion,
    InputEventKey, InputEventMouseMotion, InputEventScreenDrag, InputEventScreenTouch, Node2D, Os,
    RenderingServer, ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::prelude::*;

use salt_line_sim::ballistics::{self, FireOptions, Shot, Trace};
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{format_seed, parse_seed, Phase, Screen, StructState};
use salt_line_sim::{Vec2, DT};

use crate::app::*;
use crate::cli::DevArgs;
use crate::crash;
use crate::frame::{self, DisplayPoll, Memo, Redraw};
use crate::input::{self, Intents};
use crate::keypad;
use crate::pads::Pads;
use crate::palette::{slot, Palette};
use crate::presentation;
use crate::raster::{self, Canvas, View};
use crate::resume::{RUN_LOADING_PATH, RUN_PATH, RUN_TEMP_PATH};
use crate::save;
use crate::save_format::{ArcHint, TouchAim};
use crate::sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT, PIVOT_BASE};
use crate::touch::{Button, Rect};

const PALETTE_PATH: &str = "res://art/palette_16.png";
const SPRITE_DIR: &str = "res://art/sprites";

/// Everything the drag preview's trajectory depends on. The terrain and the
/// targets only change when a shot lands (`shots`) or the screen does
/// (`seed`, `index`); the muzzle moves with Enfilade (`offset`). A new
/// loadout or run bumps the core's `preview_epoch`.
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
    epoch: u32,
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
    cutout: (i32, i32),
    /// The drag preview's trajectory, recomputed only when its inputs change.
    preview: Memo<PreviewKey, Preview>,

    /// Key and button presses as events arrive, handed to the core as the
    /// next frame's `Intents`. Polling alone loses a press and release
    /// inside one frame.
    key_queue: Vec<Key>,
    pad_queue: Vec<JoyButton>,
    /// The frame's input as the core sees it, kept to reuse its buffers.
    intents: Intents,
    /// The joypads that drive the game: any that has pressed a button or
    /// shown a stick at rest. See `pads::Pads`.
    pads: Pads,
    /// Seconds until the joypad list is checked against the connected ones.
    pad_prune: f64,
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
            cutout: (0, 0),
            preview: Memo::default(),
            key_queue: Vec::new(),
            pad_queue: Vec::new(),
            intents: Intents::default(),
            pads: Pads::default(),
            pad_prune: 0.0,
            touch_buttons: Vec::new(),
            audio: Vec::new(),
            shot_path: None,
            frames: 0,
            elapsed: 0.0,
            shot_at: crate::cli::DEFAULT_SHOT_AT,
        }
    }

    fn ready(&mut self) {
        let mobile = Os::singleton().has_feature("mobile");
        if mobile {
            // What shows beside the playfield is a letterbox, not sky.
            RenderingServer::singleton().set_default_clear_color(Color::BLACK);
        }
        self.pal = Palette::load(PALETTE_PATH);
        self.sprites = Some(Sprites::load_all(SPRITE_DIR));
        self.g = Game::new(mobile, save::load());
        self.find_last_crash();
        self.find_saved_run();
        self.setup_audio();
        self.project_fps = Engine::singleton().get_max_fps();
        self.fps_cap = self.project_fps;
        let window = Self::window_size();
        self.cutout = self.cutout_px(window);
        // And again on the first frame: on a phone the safe area may not be
        // settled yet while the scene is still coming up.
        self.display.invalidate();
        let fx = self.g.fit_display(window, self.cutout);
        self.apply(fx);
        let args = DevArgs::parse(&Self::cmdline_all());
        if let Some(dir) = args.gallery.as_deref() {
            self.gallery(dir);
            return;
        }
        self.shot_path = args.shot.clone();
        self.shot_at = args.shot_at;
        let fx = self.g.start_dev(&args, Self::today());
        self.apply(fx);
        if !self.pal.sampled {
            godot_warn!("salt-line: palette not sampled from art; using the built-in fallback");
        }
    }

    fn process(&mut self, delta: f64) {
        if crash::panicked() {
            self.broken_frame(delta);
            return;
        }
        // Keep the crash hook's picture of the run current. Taken before this
        // frame's input, so a panic inside `fire` still reports the aim it
        // fired.
        crash::note(
            self.g.mode_name(),
            self.g.run.as_ref(),
            (self.g.angle_ticks, self.g.power_ticks),
            self.g.resumed,
        );
        // The dev clock (`--salt-shot-at`) runs on the capped time the core
        // animates by, so a capture on a slow frame lands at the same point
        // of an animation.
        let capped = crate::resume::frame_delta(delta);
        self.elapsed += capped;
        self.poll_display(capped);
        self.gather_intents();
        let fx = self.g.tick(delta, &self.intents);
        self.apply(fx);
        self.prune_pads(capped);
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
                self.g.finger_moved(drag.get_index(), pos.x, pos.y);
                return;
            }
            Err(event) => event,
        };
        let Ok(touch) = event.try_cast::<InputEventScreenTouch>() else {
            return;
        };
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
                .finger_down(touch.get_index(), pos.x, pos.y, &self.touch_buttons);
        } else {
            self.g.finger_up(touch.get_index());
        }
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::WM_GO_BACK_REQUEST => {
                self.g.go_back();
                self.redraw.mark();
            }
            CanvasItemNotification::WM_WINDOW_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_FOCUS_OUT => {
                let fx = self.g.focus_lost();
                self.apply(fx);
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
                let fx = self.g.app_paused();
                self.apply(fx);
            }
            CanvasItemNotification::WM_CLOSE_REQUEST | CanvasItemNotification::EXIT_TREE => {
                let fx = self.g.closing();
                self.apply(fx);
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
        let typed = match keycode {
            Key::BACKSPACE => Typed::Backspace,
            Key::ENTER | Key::KP_ENTER => Typed::Enter,
            Key::ESCAPE => Typed::Escape,
            _ if (32..127).contains(&unicode) => Typed::Char(unicode as u8 as char),
            // Arrows and the like fall through to move the keypad focus.
            _ => return,
        };
        let fx = self.g.typed(typed, Self::today());
        self.apply(fx);
        // Typed, not pressed: an M in a seed must not also mute, nor the
        // Enter that starts the run fire its first shot.
        self.key_queue.retain(|k| *k != keycode);
    }
}

// ------------------------------------------------------------------ effects

impl SaltLine {
    /// Carry out what the core asked for, in the order it asked.
    fn apply(&mut self, fx: Vec<Effect>) {
        for effect in fx {
            match effect {
                Effect::Play(sound) => {
                    if let Some(player) =
                        self.audio.get_mut(sound as usize).and_then(Option::as_mut)
                    {
                        player.play();
                    }
                }
                Effect::Volume(db) => {
                    for player in self.audio.iter_mut().flatten() {
                        player.set_volume_db(db);
                    }
                }
                Effect::Redraw => self.redraw.mark(),
                Effect::RecheckDisplay => self.display.invalidate(),
                Effect::Buffer { w, h, letterbox } => {
                    self.canvas.resize(w, h);
                    let (l, r) = letterbox;
                    if let Some(mut window) = self.base().get_window() {
                        window.set_content_scale_size(Vector2i::new(w + l + r, h));
                    }
                    self.base_mut().set_position(Vector2::new(l as f32, 0.0));
                }
                Effect::Aspect(aspect) => {
                    let aspect = match aspect {
                        Aspect::Expand => ContentScaleAspect::EXPAND,
                        Aspect::Keep => ContentScaleAspect::KEEP,
                    };
                    if let Some(mut window) = self.base().get_window() {
                        window.set_content_scale_aspect(aspect);
                    }
                }
                Effect::Clipboard(text) => {
                    DisplayServer::singleton().clipboard_set(&GString::from(text.as_str()));
                }
                Effect::Quit => self.base().get_tree().quit(),
                Effect::Store { save, why } => {
                    // After a panic nothing in memory is trusted enough to
                    // write, and settings say nothing of it. A score still
                    // hears that it was not saved (`save::store` refuses).
                    if why == Store::Settings && crash::panicked() {
                        continue;
                    }
                    let ok = save::store(&save);
                    let fx = self.g.stored(why, ok);
                    self.apply(fx);
                }
                Effect::WriteRun(line) => {
                    if !crash::panicked() {
                        save::write(RUN_PATH, RUN_TEMP_PATH, &line);
                    }
                }
                Effect::KeepRun(line) => {
                    save::write(RUN_PATH, RUN_TEMP_PATH, &line);
                }
                Effect::DiscardRunFile => {
                    save::remove(RUN_TEMP_PATH);
                    save::remove(RUN_PATH);
                    if save::read(RUN_PATH).is_some() || save::read(RUN_TEMP_PATH).is_some() {
                        godot_warn!("salt-line: could not delete {RUN_PATH}; blanking it");
                        save::write(RUN_PATH, RUN_TEMP_PATH, "");
                    }
                }
                Effect::Rename { from, to } => {
                    save::rename(from, to);
                }
                Effect::Remove(path) => save::remove(path),
                Effect::RetireCrashReport => {
                    if let (Some(from), Some(to)) = (
                        save::on_disk(crash::REPORT_PATH),
                        save::on_disk(crash::OLD_REPORT_PATH),
                    ) {
                        if let Err(e) = std::fs::rename(&from, &to) {
                            godot_warn!("salt-line: could not move {}: {e}", crash::REPORT_PATH);
                        }
                    }
                }
                Effect::Warn(text) => godot_warn!("{text}"),
            }
        }
    }
}

// ------------------------------------------------------------------ launch

impl SaltLine {
    /// Days since the Unix epoch. The clock is read here, in the presentation
    /// layer — nothing in the sim ever touches system time.
    fn today() -> u64 {
        let secs = Time::singleton().get_unix_time_from_system();
        (secs / 86400.0).floor().max(0.0) as u64
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

    /// Hand the core whatever saved run there is.
    ///
    /// The file is moved aside to `RUN_LOADING_PATH` before the core rebuilds
    /// it, and only put back once the rebuild succeeds, so a log that brings
    /// the sim down costs one crash, not one every launch.
    fn find_saved_run(&mut self) {
        let found = if save::read(RUN_LOADING_PATH).is_some() {
            SavedRun::Interrupted
        } else {
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
            SavedRun::Found(text)
        };
        let fx = self.g.saved_run(found);
        self.apply(fx);
    }

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
        let db = self.g.volume_db();
        self.apply(vec![Effect::Volume(db)]);
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

    /// `--salt-gallery <dir>`: render one frame per shape template, plus a
    /// damaged variant of each, and quit. See `Game::gallery`.
    fn gallery(&mut self, dir: &str) {
        for (name, run) in Game::gallery(self.g.buffer_w as usize) {
            let Some(run) = run else {
                godot_warn!("salt-line: no {name} screen found");
                continue;
            };
            self.g.show_gallery_run(run);
            self.render_play();
            self.save_canvas(&format!("{dir}/{name}.png"));
        }
        self.base().get_tree().quit();
    }
}

// ---------------------------------------------------------------- per frame

impl SaltLine {
    fn window_size() -> (i32, i32) {
        let size = DisplayServer::singleton().window_get_size();
        (size.x, size.y)
    }

    /// Follow the display every frame from the window size, which is cheap,
    /// and the cutout, which is re-read only when the size changes, focus
    /// comes back, a run ends or a second has passed: the safe area is a
    /// platform round trip on a phone. The core decides what the buffer
    /// makes of them.
    fn poll_display(&mut self, delta: f64) {
        let size = Self::window_size();
        if self.display.check(size, delta) {
            self.cutout = self.cutout_px(size);
        }
        self.intents.window = size;
        self.intents.cutout = self.cutout;
    }

    /// Screen pixels a display cutout takes from the left and right edges: a
    /// camera hole on a phone held landscape. Only asked on mobile, where the
    /// window is the whole display; on a desktop the "safe area" is the
    /// monitor's, not the window's.
    fn cutout_px(&self, (win_x, _): (i32, i32)) -> (i32, i32) {
        if !self.g.mobile {
            return (0, 0);
        }
        let safe = DisplayServer::singleton().get_display_safe_area();
        if safe.size.x <= 0 || safe.size.y <= 0 {
            return (0, 0);
        }
        let l = safe.position.x.clamp(0, win_x / 4);
        let r = (win_x - safe.position.x - safe.size.x).clamp(0, win_x / 4);
        (l, r)
    }

    /// Once per frame, before the core reads any action: this frame's
    /// presses are the ones queued since the last, with what is held now.
    fn gather_intents(&mut self) {
        let input = Input::singleton();
        let mut i = std::mem::take(&mut self.intents);
        i.clear();
        i.keys.extend(self.key_queue.drain(..).filter_map(core_key));
        i.keys_held.extend(
            KEYS.iter()
                .filter(|(_, k)| input.is_key_pressed(*k))
                .map(|(c, _)| *c),
        );
        i.pads.extend(self.pad_queue.drain(..).filter_map(core_pad));
        for (c, b) in PADS {
            if self
                .pads
                .iter()
                .any(|p| input.is_joy_button_pressed(p.device, b))
            {
                i.pads_held.push(c);
            }
        }
        i.stick = [
            self.stick_held(JoyAxis::LEFT_X, -1.0),
            self.stick_held(JoyAxis::LEFT_X, 1.0),
            self.stick_held(JoyAxis::LEFT_Y, -1.0),
            self.stick_held(JoyAxis::LEFT_Y, 1.0),
        ];
        i.today = Self::today();
        self.intents = i;
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
        self.g.busy() || stick || Input::singleton().is_anything_pressed()
    }

    /// Menus, overlays and the error screen run at `frame::IDLE_FPS`; play
    /// runs uncapped, or at whatever the project asked for. `Engine` max FPS
    /// rather than low-processor mode, which would also throttle play and
    /// the shot replay.
    fn apply_frame_cap(&mut self) {
        let cap = frame::frame_cap(self.g.idle(), self.project_fps);
        if cap != self.fps_cap {
            self.fps_cap = cap;
            Engine::singleton().set_max_fps(cap);
        }
    }

    /// A panic has happened: the error screen, every frame. See
    /// `Game::broken_tick`.
    fn broken_frame(&mut self, delta: f64) {
        // Wall clock still runs, for `--salt-shot-at` captures of this screen.
        self.elapsed += delta;
        self.gather_intents();
        let fx = self.g.broken_tick(delta, &self.intents, || {
            crash::report_ready().then(|| {
                crash::report()
                    .unwrap_or_else(|| "SALT LINE CRASH REPORT\n(report unavailable)\n".into())
            })
        });
        self.apply_frame_cap();
        self.apply(fx);
        self.render_broken();
        self.upload();
        self.maybe_screenshot();
        self.base_mut().queue_redraw();
    }

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
        self.save_canvas(&path);
        self.shot_path = None;
        self.g.capturing = false;
        self.base().get_tree().quit();
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

    /// The error screen. See `Game::broken_tick`.
    fn render_broken(&mut self) {
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
        let copy = self.g.input_label("copy");
        y = self.menu_row(
            y,
            "copy",
            &format!("{copy}      COPY REPORT"),
            "COPY REPORT",
            fg,
        );
        let back = self.g.input_label("back");
        y = self.menu_row(y, "back", &format!("{back}  QUIT"), "QUIT", fg);
        y += 4;
        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
            self.centred(y, &text, alert);
        }
    }
}

// ------------------------------------------------------------------ drawing

impl SaltLine {
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
                let confirm = self.g.input_label("confirm");
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
                    let key = self.g.input_label("new_run");
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
                let seed = self.g.input_label("seed");
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
                let controls = self.g.input_label("controls");
                y = self.menu_row(
                    y,
                    "controls",
                    &format!("{controls}  CONTROLS / AUDIO"),
                    "CONTROLS / AUDIO",
                    fg,
                );
                let back = self.g.input_label("back");
                y = self.menu_row(y, "back", &format!("{back}  QUIT"), "QUIT", dim);
                if self.g.last_crash.is_some() {
                    let text = if self.g.crash_copied {
                        "CRASH REPORT COPIED"
                    } else {
                        "COPY LAST CRASH REPORT"
                    };
                    let copy = self.g.input_label("copy");
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
                    let mut labels = self.g.input_label(a);
                    if !b.is_empty() {
                        labels.push('/');
                        labels.push_str(&self.g.input_label(b));
                    }
                    self.centred(y, &format!("{name}: {labels}"), fg);
                    y += 7;
                }
                y += 4;
                self.centred(y, "M / START  MUTE     R / Y  RESET", dim);
                y += 7;
                let on = if self.g.last_arc_on() { "ON" } else { "OFF" };
                let toggle = format!("{}  LAST ARC: {on}", self.g.input_label("last_arc"));
                self.centred(y, &toggle, fg);
                y += 9;
                self.centred(y, "EDIT bind LINES IN salt_line.txt TO REMAP", dim);
                y += 7;
                self.centred(y, "FORMAT: bind ACTION KEY PAD", dim);
                y += 9;
                let back = self.g.input_label("back");
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
            epoch: self.g.preview_epoch,
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
        } else if let Some(text) = self.g.onboarding_prompt(phase, wind) {
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
        let resume = self.g.input_label("confirm");
        self.centred((h - 48) / 2 + 21, &format!("{resume} / BACK  RESUME"), fg);
        let title = self.g.input_label("replay");
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

/// The keys the core reads, and the engine's names for them.
const KEYS: [(input::Key, Key); 25] = [
    (input::Key::Left, Key::LEFT),
    (input::Key::Right, Key::RIGHT),
    (input::Key::Up, Key::UP),
    (input::Key::Down, Key::DOWN),
    (input::Key::Space, Key::SPACE),
    (input::Key::Enter, Key::ENTER),
    (input::Key::KpEnter, Key::KP_ENTER),
    (input::Key::Escape, Key::ESCAPE),
    (input::Key::BracketLeft, Key::BRACKETLEFT),
    (input::Key::BracketRight, Key::BRACKETRIGHT),
    (input::Key::Comma, Key::COMMA),
    (input::Key::Period, Key::PERIOD),
    (input::Key::Num1, Key::KEY_1),
    (input::Key::Num2, Key::KEY_2),
    (input::Key::Num3, Key::KEY_3),
    (input::Key::C, Key::C),
    (input::Key::R, Key::R),
    (input::Key::S, Key::S),
    (input::Key::U, Key::U),
    (input::Key::N, Key::N),
    (input::Key::V, Key::V),
    (input::Key::M, Key::M),
    (input::Key::L, Key::L),
    (input::Key::F1, Key::F1),
    (input::Key::Shift, Key::SHIFT),
];

/// The pad buttons the core reads, and the engine's names for them.
const PADS: [(input::Pad, JoyButton); 12] = [
    (input::Pad::A, JoyButton::A),
    (input::Pad::B, JoyButton::B),
    (input::Pad::X, JoyButton::X),
    (input::Pad::Y, JoyButton::Y),
    (input::Pad::Start, JoyButton::START),
    (input::Pad::Back, JoyButton::BACK),
    (input::Pad::LeftShoulder, JoyButton::LEFT_SHOULDER),
    (input::Pad::RightShoulder, JoyButton::RIGHT_SHOULDER),
    (input::Pad::DpadLeft, JoyButton::DPAD_LEFT),
    (input::Pad::DpadRight, JoyButton::DPAD_RIGHT),
    (input::Pad::DpadUp, JoyButton::DPAD_UP),
    (input::Pad::DpadDown, JoyButton::DPAD_DOWN),
];

/// A key as the core names it; `None` for one it never reads.
fn core_key(key: Key) -> Option<input::Key> {
    KEYS.iter().find(|(_, k)| *k == key).map(|(c, _)| *c)
}

/// A pad button as the core names it; `None` for one it never reads.
fn core_pad(button: JoyButton) -> Option<input::Pad> {
    PADS.iter().find(|(_, b)| *b == button).map(|(c, _)| *c)
}

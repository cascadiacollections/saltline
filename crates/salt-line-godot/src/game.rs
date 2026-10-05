//! The game node: the adapter between Godot and the engine-free core.
//!
//! Everything that decides anything lives in `app` (the game) and
//! `salt-line-sim` (the rules); see `app` for how the three fit. This node
//! gathers the engine's input into `input::Intents`, steps the core, carries
//! out the `app::Effect`s it returns (audio, `user://` files, the clipboard,
//! the window), and puts each frame on screen as a stack of strip textures.
//!
//! # The frame loop
//!
//! ```text
//!   Godot callbacks between frames            immediate side channels
//!   ──────────────────────────────            ───────────────────────
//!   input():  key down ──► Event::Key         on_notification():
//!             finger   ──► Event::Finger*       focus out ──► Game::focus_lost ─┐
//!             pad btn  ──► pad_queue            app paused ─► Game::app_paused ─┤
//!             stick    ──► pads.motion          close/exit ─► Game::closing ────┤
//!   on_notification(): back ──► Event::Back     (each takes the queued events   │
//!             │                                  first, then its effects are    │
//!             │ queued in `events`               applied at once) ──► apply() ◄─┘
//!             │                                 focus in / resumed ──►
//!             │                                  DisplayPoll::invalidate,
//!             │                                  Redraw::mark
//!             ▼
//!   process(delta)
//!     ├─ crash::panicked()? ──yes──► broken_frame: Game::broken_tick,
//!     │                               file effects dropped, error screen
//!     ├─ poll_display       window size every frame, cutout when due
//!     ├─ watch_drift        disarm a stick axis held over, doing nothing, too long
//!     ├─ gather_intents     events + keys/pads held + stick polled ──► Intents
//!     ├─ Game::tick_with    events → crash::note (for the report) → timers
//!     │                     → display → mode input → step → run file → settings
//!     ├─ apply(effects)     in order; Store ──► save::store ──► Game::stored
//!     ├─ pads.in_use        a stick held over that moved the aim is not drift
//!     ├─ prune_pads         forget unplugged joypads, once a second
//!     ├─ frame cap          idle menus at frame::IDLE_FPS
//!     ├─ redraw.due(busy)?  dirty flag, or anything moving
//!     │     └─ yes: render::frame ──► Canvas ──► set_buttons ──► upload
//!     └─ maybe_screenshot   --salt-shot
//!   draw(): the strip textures at their rows, 1:1; the viewport stretch
//!           scales the result
//! ```
//!
//! `input()` marks a redraw on every event except mouse and stick motion.
//!
//! Crash handling sits in two places. `crash::install` (in `lib.rs`) chains
//! a panic hook that writes the report; gdext catches the panic, and the
//! next `process` sees `crash::panicked()` and runs the error screen instead
//! of the game. Independently, `apply` drops every file effect once a panic
//! has happened, because a panic can land between an effect being queued
//! and being applied. See `app`'s "After a panic".
//!
//! # Techniques
//!
//! * **Events, not polling, for presses.** A key tapped and released inside
//!   one frame is invisible to `Input::is_key_pressed`; `input()` records it
//!   as an event, so it still counts once. Holds are polled, because only
//!   the state now matters for them.
//! * **Dirty-flag redraw.** The frame is rasterised in software and uploaded
//!   whole, so an unchanged frame is skipped (`frame::Redraw`): drawn when an
//!   effect or an input marked it, while anything moves, and once after.
//! * **Strip upload.** The frame is shown as one whole-frame texture with
//!   `strips::STRIPS` horizontal strip textures over it, and only the strips
//!   whose pixels changed are sent again; when most changed, the whole frame
//!   is (`upload`; why, in `strips`). Each texture is updated in place
//!   without allocating: two byte buffers alternate, so the one being filled
//!   is never the one the image still holds.

use godot::classes::notify::CanvasItemNotification;
use godot::classes::{
    window::ContentScaleAspect, AudioStream, AudioStreamPlayer, DisplayServer, Engine, INode2D,
    Image, ImageTexture, Input, InputEvent, InputEventJoypadButton, InputEventJoypadMotion,
    InputEventKey, InputEventMouseMotion, InputEventScreenDrag, InputEventScreenTouch, Node2D, Os,
    RenderingServer, ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::prelude::*;

use crate::app::{Abandoned, Aspect, CrashReport, Effect, Game, RunFile, Sound};
use crate::cli::DevArgs;
use crate::crash;
use crate::frame::{self, DisplayPoll, Redraw};
use crate::input::{self, Event, Intents, Typed};
use crate::pads::Pads;
use crate::palette::Palette;
#[cfg(feature = "profiling")]
use crate::profiling::Capture as ProfileCapture;
use crate::raster::Canvas;
use crate::render::{self, Art, Cache};
use crate::resume::{self, RUN_LOADING_PATH, RUN_PATH};
use crate::save;
use crate::sprites::Sprites;
use crate::strips::{self, Layers, Plan, Shadow, STRIPS};
use crate::touch::Button;

const PALETTE_PATH: &str = "res://art/palette_16.png";
const SPRITE_DIR: &str = "res://art/sprites";
const TITLE_BACKGROUND_PATH: &str = "res://art/title_background_320x180.png";
/// Music sits behind the sharper one-shot effects instead of competing with them.
const TITLE_GAIN_DB: f32 = -8.0;
/// A cold launch should not hit the speaker at the track's full level.
const TITLE_FADE_SECONDS: f64 = 1.0;

/// The one Godot node the game needs, registered with the engine as
/// `SaltLine` and used by `godot/main.tscn`. It owns the core ([`Game`]) and
/// everything engine-side: the canvas and its textures, the audio players,
/// the input queues and the frame pacing.
#[derive(GodotClass)]
#[class(base = Node2D)]
pub struct SaltLine {
    base: Base<Node2D>,
    /// Everything the game knows that is not the engine's. See `app`.
    g: Game,

    /// The palette and sprites, loaded once.
    art: Art,
    /// One buffer for everything: terrain, sprites and UI, all at the internal
    /// resolution. DESIGN §5.1 — "UI renders in the same buffer at the same
    /// resolution".
    canvas: Canvas,
    /// The frame on the GPU: one whole-frame layer and `STRIPS` horizontal
    /// strip layers, which live as long as the buffer keeps its size. See
    /// `upload`.
    whole: Option<Layer>,
    strips: Vec<Layer>,
    /// Which strips are drawn over `whole`.
    layers: Layers,
    /// The frame as on screen, to find the strips that changed.
    shadow: Shadow,
    /// Whether this frame needs drawing at all. See `frame::Redraw`.
    redraw: Redraw,
    /// The engine's frame cap as the project set it, restored in play.
    project_fps: i32,
    fps_cap: i32,
    /// When to read the display's safe area again, and what it last said.
    display: DisplayPoll,
    cutout: (i32, i32),
    /// Drawing work kept between frames. See `render::Cache`.
    cache: Cache,

    /// Key presses, typed text, touches and the back gesture as they arrive,
    /// and pad presses, handed to the core as the next frame's `Intents`.
    /// Polling alone loses a press and release inside one frame.
    events: Vec<Event>,
    /// Pad buttons that went down since the last frame, in engine terms;
    /// `gather_intents` translates them to `input::Pad`.
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
    /// One player per `Sound`, in `Sound::ALL` order, so `sound as usize`
    /// finds it. `None` where the file failed to load: that sound is silent.
    audio: Vec<Option<Gd<AudioStreamPlayer>>>,
    /// Master gain last sent by the core; title music adds its own offset.
    audio_db: f32,
    /// Seconds remaining in the title track's linear-amplitude fade-in.
    title_fade: f64,

    /// `--salt-shot <path>`: render a few frames, save the buffer as a PNG at
    /// native resolution, and quit. How the pixel output gets checked without
    /// a human at the keyboard.
    shot_path: Option<String>,
    /// Frames processed, drawn or not. A capture waits for at least three.
    frames: u64,
    /// Seconds of wall clock since the node came up, for the dev flags below.
    elapsed: f64,
    /// `--salt-shot-at <seconds>`: when to grab the screenshot.
    shot_at: f64,
    /// Present only in the dedicated profiling build; no clocks or samples in
    /// ordinary release builds.
    #[cfg(feature = "profiling")]
    profile: ProfileCapture,
}

#[godot_api]
impl INode2D for SaltLine {
    fn init(base: Base<Node2D>) -> Self {
        SaltLine {
            base,
            g: Game::default(),
            art: Art::default(),
            canvas: Canvas::new(320, 180),
            whole: None,
            strips: Vec::new(),
            layers: Layers::default(),
            shadow: Shadow::default(),
            redraw: Redraw::default(),
            project_fps: 0,
            fps_cap: 0,
            display: DisplayPoll::default(),
            cutout: (0, 0),
            cache: Cache::default(),
            events: Vec::new(),
            pad_queue: Vec::new(),
            intents: Intents::default(),
            pads: Pads::default(),
            pad_prune: 0.0,
            touch_buttons: Vec::new(),
            audio: Vec::new(),
            audio_db: -80.0,
            title_fade: 0.0,
            shot_path: None,
            frames: 0,
            elapsed: 0.0,
            shot_at: crate::cli::DEFAULT_SHOT_AT,
            #[cfg(feature = "profiling")]
            profile: ProfileCapture::new(),
        }
    }

    fn ready(&mut self) {
        let mobile = Os::singleton().has_feature("mobile");
        // Aspect::Keep can letterbox on desktop too (16:10, ultrawide and
        // Long Lens). The area outside the one drawn texture is not sky.
        RenderingServer::singleton().set_default_clear_color(Color::BLACK);
        self.art = Art {
            pal: Palette::load(PALETTE_PATH),
            sprites: Some(Sprites::load_all(SPRITE_DIR)),
            title_background: Sprites::load_one(TITLE_BACKGROUND_PATH),
        };
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
        #[cfg(feature = "profiling")]
        let args = {
            let mut args = args;
            // A profiling APK is deliberately self-driving so every install
            // starts from the same simulation seed without launch arguments.
            args.seed = Some(crate::profiling::FIXED_SEED);
            args.auto = true;
            godot_print!(
                "salt-profile: deterministic profiling run; seed={}",
                crate::profiling::FIXED_SEED
            );
            args
        };
        if let Some(dir) = args.gallery.as_deref() {
            self.gallery(dir);
            return;
        }
        self.shot_path = args.shot.clone();
        self.shot_at = args.shot_at;
        let fx = self.g.start_dev(&args, Self::today());
        self.apply(fx);
        if !self.art.pal.sampled {
            godot_warn!("salt-line: palette not sampled from art; using the built-in fallback");
        }
    }

    fn process(&mut self, delta: f64) {
        if crash::panicked() {
            self.broken_frame(delta);
            return;
        }
        #[cfg(feature = "profiling")]
        let process_started = std::time::Instant::now();
        #[cfg(feature = "profiling")]
        self.profile.begin_frame();
        // The dev clock (`--salt-shot-at`) runs on the capped time the core
        // animates by, so a capture on a slow frame lands at the same point
        // of an animation.
        let capped = resume::frame_delta(delta);
        self.elapsed += capped;
        self.poll_display(capped);
        self.watch_drift(capped);
        self.gather_intents();
        // Keep the crash hook's picture of the run current. Taken before this
        // frame's input is acted on, so a panic inside `fire` still reports
        // the aim it fired. The core gets the raw `delta` and caps it itself.
        let aim = (self.g.play.angle_ticks, self.g.play.power_ticks);
        #[cfg(feature = "profiling")]
        let tick_started = std::time::Instant::now();
        let fx = self.g.tick_with(delta, &self.intents, |g| {
            crash::note(
                g.mode_name(),
                g.play.run.as_ref(),
                (g.play.angle_ticks, g.play.power_ticks),
                g.run_file.resumed,
            )
        });
        #[cfg(feature = "profiling")]
        self.profile.record_tick(tick_started.elapsed());
        self.apply(fx);
        self.tick_title_music(capped);
        // A stick held over that moved the aim this frame is a hand on it,
        // however long it has been held: not drift (`Pads::in_use`).
        if self.intents.stick.iter().any(|&held| held)
            && aim != (self.g.play.angle_ticks, self.g.play.power_ticks)
        {
            self.pads.in_use();
        }
        self.prune_pads(capped);
        self.apply_frame_cap();
        #[cfg(feature = "profiling")]
        self.profile
            .start_if_playing(self.g.mode == crate::app::Mode::Playing);
        if self.redraw.due(self.busy()) {
            self.render();
        }
        // Every frame, drawn or not: an undrawn frame is the last one drawn.
        self.maybe_screenshot();
        #[cfg(feature = "profiling")]
        {
            self.profile.record_process(process_started.elapsed());
            if let Some(lines) = self.profile.finish_if_due() {
                for line in lines {
                    godot_print!("{line}");
                }
            }
        }
    }

    fn draw(&mut self) {
        // The frame is one whole-frame texture at the internal resolution,
        // with the strips that changed since it was last updated drawn over
        // it (see `strips::Layers`), all blitted 1:1. Godot's viewport stretch
        // does the only scaling in the game, with nearest filtering set
        // project-wide. A texture updated in place needs no new draw; this
        // runs again only when which strips are shown changes.
        let layers: Vec<_> = self
            .whole
            .iter()
            .chain(
                self.strips
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| self.layers.shown(i))
                    .map(|(_, s)| s),
            )
            .map(|l| (l.texture.clone(), Vector2::new(0.0, l.y as f32)))
            .collect();
        for (tex, at) in layers {
            self.base_mut().draw_texture(&tex, at);
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
                    self.events.push(key_event(&key));
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
                self.events.push(Event::FingerMoved {
                    index: drag.get_index(),
                    x: pos.x,
                    y: pos.y,
                });
                return;
            }
            Err(event) => event,
        };
        let Ok(touch) = event.try_cast::<InputEventScreenTouch>() else {
            return;
        };
        // The strips are drawn from the origin at 1:1, so the node's local
        // space is buffer pixels whatever the window is scaled to.
        let local = self
            .base()
            .make_input_local(&touch.clone().upcast::<InputEvent>());
        let pos = local
            .try_cast::<InputEventScreenTouch>()
            .map(|t| t.get_position())
            .unwrap_or_else(|_| touch.get_position());
        let index = touch.get_index();
        self.events.push(if touch.is_pressed() {
            Event::FingerDown {
                index,
                x: pos.x,
                y: pos.y,
            }
        } else {
            Event::FingerUp { index }
        });
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::WM_GO_BACK_REQUEST => {
                self.events.push(Event::Back);
                self.redraw.mark();
            }
            CanvasItemNotification::WM_WINDOW_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_FOCUS_OUT => {
                // Handled now, not at the next frame, which may be a long
                // way off. The events queued so far happened first, so they
                // go with it rather than being taken after it.
                let pending = std::mem::take(&mut self.events);
                let fx = self.g.focus_lost(&pending);
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
                let pending = std::mem::take(&mut self.events);
                let fx = self.g.app_paused(&pending);
                self.apply(fx);
            }
            CanvasItemNotification::WM_CLOSE_REQUEST | CanvasItemNotification::EXIT_TREE => {
                let pending = std::mem::take(&mut self.events);
                let fx = self.g.closing(&pending);
                self.apply(fx);
            }
            _ => {}
        }
    }
}

#[godot_api]
impl SaltLine {
    /// Presentation-only state for optional desktop services. A paused run
    /// releases GameMode; neither the service nor this query enters the sim.
    #[func]
    fn is_active_play(&self) -> bool {
        self.g.mode == crate::app::Mode::Playing
    }
}

// ------------------------------------------------------------------ effects

impl SaltLine {
    /// Carry out what the core asked for, in the order it asked.
    fn apply(&mut self, fx: Vec<Effect>) {
        for effect in fx {
            // Nothing is written after a panic, even what was queued before
            // it. See `app`'s module docs.
            if !effect.survives_panic() && crash::panicked() {
                continue;
            }
            match effect {
                Effect::Play(sound) => {
                    if let Some(player) =
                        self.audio.get_mut(sound as usize).and_then(Option::as_mut)
                    {
                        player.play();
                    }
                }
                Effect::Volume(db) => {
                    self.audio_db = db;
                    for (index, player) in self.audio.iter_mut().enumerate() {
                        if let Some(player) = player {
                            let gain = if index == Sound::Title as usize {
                                title_volume_db(db)
                            } else {
                                db
                            };
                            player.set_volume_db(gain);
                        }
                    }
                }
                Effect::TitleMusic(on) => {
                    if let Some(player) = self
                        .audio
                        .get_mut(Sound::Title as usize)
                        .and_then(Option::as_mut)
                    {
                        if on {
                            if !player.is_playing() {
                                self.title_fade = TITLE_FADE_SECONDS;
                                player.set_volume_db(-80.0);
                                player.play();
                            }
                        } else {
                            self.title_fade = 0.0;
                            player.stop();
                        }
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
                Effect::Store(why) => {
                    // A finished log goes before its score, and the score
                    // only once the log is confirmed gone.
                    let cleared = why.clear().is_none_or(|text| {
                        let ok = resume::replace(&mut UserFiles, text);
                        if !ok {
                            godot_warn!(
                                "salt-line: could not clear {RUN_PATH}; not scoring beside it"
                            );
                        }
                        ok
                    });
                    let ok = cleared && save::store(&self.g.save_to_store(&why));
                    // The outcome goes straight back to the core, and what
                    // it asks for in reply is applied now, ahead of the rest
                    // of this list: a failed score's `RunFile::Write`, which
                    // keeps its log, must land before anything queued after
                    // the score.
                    let fx = self.g.stored(why, ok);
                    self.apply(fx);
                }
                Effect::RunFile(RunFile::Write(text)) => {
                    resume::replace(&mut UserFiles, &text);
                }
                Effect::RunFile(RunFile::Restore) => {
                    save::rename(RUN_LOADING_PATH, RUN_PATH);
                }
                Effect::RunFile(RunFile::Discard) => Self::discard_run_file(),
                Effect::RunFile(RunFile::Abandon(why)) => match why {
                    Abandoned::Interrupted => {
                        godot_warn!(
                            "salt-line: discarding {RUN_LOADING_PATH}: a rebuild did not finish"
                        );
                        resume::drop_set_aside(&mut UserFiles);
                    }
                    Abandoned::Unreadable(why) => {
                        godot_warn!("salt-line: discarding {RUN_PATH}: {why:?}");
                        Self::discard_run_file();
                    }
                },
                Effect::CrashReport(CrashReport::Retire) => {
                    if let (Some(from), Some(to)) = (
                        save::on_disk(crash::REPORT_PATH),
                        save::on_disk(crash::OLD_REPORT_PATH),
                    ) {
                        if let Err(e) = std::fs::rename(&from, &to) {
                            godot_warn!("salt-line: could not move {}: {e}", crash::REPORT_PATH);
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ launch

/// The run files on `user://`, through `save`, for `resume`'s rules.
struct UserFiles;

impl resume::RunFiles for UserFiles {
    fn read(&self, path: &str) -> Option<String> {
        save::read(path)
    }
    fn exists(&self, path: &str) -> bool {
        save::exists(path)
    }
    fn write(&mut self, main: &str, temp: &str, text: &str) -> bool {
        save::write(main, temp, text)
    }
    fn rename(&mut self, from: &str, to: &str) -> bool {
        save::rename(from, to)
    }
    fn remove(&mut self, path: &str) {
        save::remove(path);
    }
}

impl SaltLine {
    /// Leave no run to resume or score (`resume::discard`).
    fn discard_run_file() {
        if !resume::discard(&mut UserFiles) {
            godot_warn!("salt-line: could not delete or blank {RUN_PATH}");
        }
    }
}

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
        let report = std::fs::read_to_string(path)
            .ok()
            .filter(|t| !t.trim().is_empty());
        self.g.set_last_crash(report);
    }

    /// Hand the core whatever saved run there is.
    ///
    /// The file is moved aside to `RUN_LOADING_PATH` before the core rebuilds
    /// it, and only put back once the rebuild succeeds, so a log that brings
    /// the sim down costs one crash, not one every launch. Which copy, and
    /// the move, are `resume::take_saved`.
    fn find_saved_run(&mut self) {
        let Some(found) = resume::take_saved(&mut UserFiles) else {
            return;
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
        self.apply(vec![
            Effect::Volume(db),
            Effect::TitleMusic(self.g.title_music_playing()),
        ]);
    }

    /// Keep title playback continuous even on backends that ignore the WAV
    /// loop flag, and ease in only when the track is deliberately started.
    /// A fallback restart after the natural end is already at the settled
    /// level, so the loop does not pulse every fourteen seconds.
    fn tick_title_music(&mut self, delta: f64) {
        if !self.g.title_music_playing() {
            return;
        }
        let target = title_volume_db(self.audio_db);
        let Some(player) = self
            .audio
            .get_mut(Sound::Title as usize)
            .and_then(Option::as_mut)
        else {
            return;
        };
        if !player.is_playing() {
            self.title_fade = 0.0;
            player.set_volume_db(target);
            player.play();
            return;
        }
        if self.title_fade > 0.0 {
            self.title_fade = (self.title_fade - delta).max(0.0);
            let progress = (1.0 - self.title_fade / TITLE_FADE_SECONDS) as f32;
            let db = if progress <= 0.0 {
                -80.0
            } else {
                (target + 20.0 * progress.log10()).max(-80.0)
            };
            player.set_volume_db(db);
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

    /// `--salt-gallery <dir>`: render one frame per shape template, plus a
    /// damaged variant of each, and quit. See `Game::gallery`.
    fn gallery(&mut self, dir: &str) {
        for (name, run) in Game::gallery(self.g.display.buffer_w as usize) {
            let Some(run) = run else {
                godot_warn!("salt-line: no {name} screen found");
                continue;
            };
            self.g.show_gallery_run(run);
            render::play(
                &self.g,
                &self.art,
                &mut self.canvas,
                &mut self.touch_buttons,
                &mut self.cache,
            );
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
        if !self.g.display.mobile {
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
        i.events.append(&mut self.events);
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

    /// Disarm a stick axis that has sat pushed too long to be a hand on it
    /// (`Pads::tick`), before this frame reads the stick.
    fn watch_drift(&mut self, delta: f64) {
        let input = Input::singleton();
        self.pads.tick(delta, |device, slot| {
            let axis = if slot == 0 {
                JoyAxis::LEFT_X
            } else {
                JoyAxis::LEFT_Y
            };
            input.get_joy_axis(device, axis)
        });
    }

    /// The stick is pushed past the threshold on any joypad whose stick has
    /// been seen at rest.
    fn stick_held(&self, axis: JoyAxis, sign: f32) -> bool {
        let slot = if axis == JoyAxis::LEFT_X { 0 } else { 1 };
        let input = Input::singleton();
        self.pads
            .iter()
            .filter(|p| p.armed[slot])
            .any(|p| input.get_joy_axis(p.device, axis) * sign > crate::pads::PUSH)
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
        render::broken(
            &self.g,
            &self.art,
            &mut self.canvas,
            &mut self.touch_buttons,
            &mut self.cache,
        );
        self.g.set_buttons(&self.touch_buttons);
        self.upload();
        self.maybe_screenshot();
        self.base_mut().queue_redraw();
    }

    fn render(&mut self) {
        #[cfg(feature = "profiling")]
        let render_started = std::time::Instant::now();
        render::frame(
            &self.g,
            &self.art,
            &mut self.canvas,
            &mut self.touch_buttons,
            &mut self.cache,
        );
        self.g.set_buttons(&self.touch_buttons);
        #[cfg(feature = "profiling")]
        self.profile.record_render(render_started.elapsed());
        self.upload();
    }

    /// Hand the changed parts of the buffer to the GPU without allocating.
    /// The strips whose rows differ from the screen are uploaded, or, when
    /// more than `strips::WHOLE_ABOVE` do, the whole frame is (`Layers`).
    /// Each upload copies the rows into whichever of the layer's two buffers
    /// its image is not holding, the image takes it by reference, and the
    /// texture is updated in place. Only a change of buffer size makes new
    /// layers.
    fn upload(&mut self) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let bytes = self.canvas.bytes();
        if !self.shadow.fits(w, h) || self.whole.is_none() || self.strips.len() != STRIPS {
            self.whole = Layer::new(bytes, w, 0, h);
            self.strips = (0..STRIPS)
                .map(|i| {
                    let (y0, y1) = strips::rows(h, i);
                    Layer::new(bytes, w, y0, y1)
                })
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default();
            self.shadow.reset(bytes, w, h);
            self.layers.reset();
            self.base_mut().queue_redraw();
            return;
        }
        #[cfg(feature = "profiling")]
        if !self.profile.should_update_texture() {
            self.profile.record_texture_update_skipped();
            return;
        }
        #[cfg(feature = "profiling")]
        let update_started = std::time::Instant::now();
        let changed = std::array::from_fn(|i| self.shadow.take_if_changed(bytes, i));
        let (plan, redraw) = self.layers.plan(changed);
        #[cfg(feature = "profiling")]
        let sent = match &plan {
            Plan::Nothing => 0,
            Plan::Whole => STRIPS as u64,
            Plan::Strips(up) => up.iter().filter(|&&u| u).count() as u64,
        };
        match plan {
            Plan::Nothing => {}
            Plan::Whole => {
                if let Some(whole) = self.whole.as_mut() {
                    whole.update(bytes, w);
                }
            }
            Plan::Strips(up) => {
                for (strip, _) in self.strips.iter_mut().zip(up).filter(|(_, u)| *u) {
                    strip.update(bytes, w);
                }
            }
        }
        #[cfg(feature = "profiling")]
        self.profile
            .record_texture_update(update_started.elapsed(), sent);
        if redraw {
            self.base_mut().queue_redraw();
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
        self.g.capture_taken();
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
}

fn title_volume_db(master: f32) -> f32 {
    (master + TITLE_GAIN_DB).max(-80.0)
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

/// A key press as the core takes it: the key, if the game reads it, and
/// what it types in seed entry, if anything. Arrows and the like type
/// nothing, and so move the seed keypad's focus instead.
fn key_event(key: &Gd<InputEventKey>) -> Event {
    let keycode = key.get_keycode();
    let unicode = key.get_unicode();
    let typed = match keycode {
        Key::BACKSPACE => Some(Typed::Backspace),
        Key::ENTER | Key::KP_ENTER => Some(Typed::Enter),
        Key::ESCAPE => Some(Typed::Escape),
        _ if (32..127).contains(&unicode) => Some(Typed::Char(unicode as u8 as char)),
        _ => None,
    };
    Event::Key {
        key: core_key(keycode),
        typed,
    }
}

/// A key as the core names it; `None` for one it never reads.
fn core_key(key: Key) -> Option<input::Key> {
    KEYS.iter().find(|(_, k)| *k == key).map(|(c, _)| *c)
}

/// A pad button as the core names it; `None` for one it never reads.
fn core_pad(button: JoyButton) -> Option<input::Pad> {
    PADS.iter().find(|(_, b)| *b == button).map(|(c, _)| *c)
}

/// Rows `y..y + h` of the frame on the GPU, as an image and the texture
/// made from it: the whole frame, or one strip of it. See `strips`.
struct Layer {
    y: i32,
    h: i32,
    image: Gd<Image>,
    texture: Gd<ImageTexture>,
    bufs: [PackedByteArray; 2],
    /// The buffer the image is not holding, filled by the next update.
    flip: usize,
}

impl Layer {
    const FORMAT: godot::classes::image::Format = godot::classes::image::Format::RGBA8;

    /// Rows `y0..y1` of a frame `w` pixels wide, uploaded from `frame`.
    fn new(frame: &[u8], w: i32, y0: i32, y1: i32) -> Option<Self> {
        let rows = Self::rows(frame, w, y0, y1);
        let bufs = [PackedByteArray::from(rows), PackedByteArray::from(rows)];
        let image = Image::create_from_data(w, y1 - y0, false, Self::FORMAT, &bufs[0])?;
        let texture = ImageTexture::create_from_image(&image)?;
        Some(Self {
            y: y0,
            h: y1 - y0,
            image,
            texture,
            bufs,
            flip: 1,
        })
    }

    /// Send this layer's rows of `frame` to its texture.
    fn update(&mut self, frame: &[u8], w: i32) {
        let buf = &mut self.bufs[self.flip];
        buf.as_mut_slice()
            .copy_from_slice(Self::rows(frame, w, self.y, self.y + self.h));
        self.image.set_data(w, self.h, false, Self::FORMAT, buf);
        self.flip ^= 1;
        self.texture.update(&self.image);
    }

    fn rows(frame: &[u8], w: i32, y0: i32, y1: i32) -> &[u8] {
        let stride = w as usize * 4;
        &frame[y0 as usize * stride..y1 as usize * stride]
    }
}

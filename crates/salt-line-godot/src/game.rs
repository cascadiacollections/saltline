//! The game node. Rendering, input, UI — no game logic.
//!
//! Everything that decides anything lives in `salt-line-sim`. This file draws
//! what the sim says is true, feeds it integer inputs, and animates the gap
//! between "the shot has been resolved" and "the player has seen it happen".

use godot::classes::{
    DisplayServer, INode2D, Image, ImageTexture, Input, InputEvent, InputEventKey, Node2D, Os, Time,
};
use godot::global::Key;
use godot::prelude::*;

use salt_line_sim::ballistics::{Shot, Trace};
use salt_line_sim::dmath;
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{
    self, buffer_width, daily_seed, format_seed, parse_seed, Phase, RunState, Screen, StructState,
    FREE_SCREENS,
};
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_H, SCREEN_W_MAX, SCREEN_W_MIN};

use crate::palette::{slot, Palette};
use crate::raster::{self, Canvas, View};
use crate::save::{Entry, Save};
use crate::sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT, PIVOT_BASE};

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
const REPEAT_RATE: f64 = 0.035;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Title,
    SeedEntry,
    Playing,
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
    seed_text: String,
    daily: bool,
    last_seed: u64,
    status: String,
    status_time: f64,
    debug: bool,

    /// Key edge/repeat state: (key, was_down, timer).
    keys: Vec<(Key, bool, f64)>,

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
            angle_ticks: 450,
            power_ticks: 600,
            save: Save::default(),
            seed_text: String::new(),
            daily: true,
            last_seed: 0,
            status: String::new(),
            status_time: 0.0,
            debug: false,
            keys: Vec::new(),
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
        self.pal = Palette::load(PALETTE_PATH);
        self.sprites = Some(Sprites::load_all(SPRITE_DIR));
        self.save = Save::load();
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
            Key::ENTER | Key::KP_ENTER => {
                let seed = if self.seed_text.trim().is_empty() {
                    daily_seed(Self::today())
                } else {
                    parse_seed(&self.seed_text)
                };
                self.start_run(seed, false);
            }
            Key::ESCAPE => self.mode = Mode::Title,
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
    fn today() -> u64 {
        let secs = Time::singleton().get_unix_time_from_system();
        (secs / 86400.0).floor().max(0.0) as u64
    }

    /// Internal buffer width. DESIGN §5.1: fix the height at 180, let the
    /// width float 320..400 by device aspect, and generate terrain to fill it.
    fn pick_width(&self) -> i32 {
        let size = DisplayServer::singleton().window_get_size();
        let aspect = if size.y > 0 {
            size.x as f64 / size.y as f64
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
        if let Some(mut window) = self.base().get_window() {
            window.set_content_scale_size(Vector2i::new(self.buffer_w, h));
        }
    }

    fn start_run(&mut self, seed: u64, daily: bool) {
        // The world is as wide as the buffer, so the run is generated for it.
        let width = self.buffer_w as usize;
        let state = RunState::new(seed, width);
        self.angle_ticks = 450;
        self.power_ticks = 600;
        self.anim = None;
        self.clear_hold = 0.0;
        self.cam_x = 0;
        self.cam_target = 0.0;
        self.daily = daily;
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
        if self.pressed(Key::F1) {
            self.debug = !self.debug;
        }
        match self.mode {
            Mode::Title => {
                if self.pressed(Key::ENTER) || self.pressed(Key::KP_ENTER) {
                    let seed = daily_seed(Self::today());
                    self.start_run(seed, true);
                } else if self.pressed(Key::S) {
                    self.seed_text.clear();
                    self.mode = Mode::SeedEntry;
                } else if self.pressed(Key::ESCAPE) {
                    self.base().get_tree().quit();
                }
            }
            Mode::SeedEntry => {
                // Text comes in through `unhandled_input`.
            }
            Mode::Paywall => {
                if self.pressed(Key::U) {
                    self.unlock();
                } else if self.pressed(Key::ENTER) || self.pressed(Key::ESCAPE) {
                    self.finish_run();
                }
            }
            Mode::GameOver => {
                if self.pressed(Key::C) {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.last_seed);
                    DisplayServer::singleton().clipboard_set(&GString::from(seed.as_str()));
                    self.note(format!("COPIED {seed}"));
                } else if self.pressed(Key::ENTER) || self.pressed(Key::ESCAPE) {
                    self.mode = Mode::Title;
                } else if self.pressed(Key::R) {
                    let seed = self.last_seed;
                    let daily = self.daily;
                    self.start_run(seed, daily);
                }
            }
            Mode::Playing => self.handle_play_input(delta),
        }
    }

    fn handle_play_input(&mut self, delta: f64) {
        if self.pressed(Key::ESCAPE) {
            self.mode = Mode::Title;
            return;
        }
        let phase = self.run.as_ref().map(|r| r.phase);
        let Some(phase) = phase else { return };

        if phase == Phase::Drafting {
            for (i, key) in [Key::KEY_1, Key::KEY_2, Key::KEY_3].into_iter().enumerate() {
                if self.pressed(key) {
                    let taken = self.run.as_mut().and_then(|r| r.take_offer(i));
                    if let Some(m) = taken {
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

        let left = self.repeated(Key::LEFT, delta) as i32;
        let right = self.repeated(Key::RIGHT, delta) as i32;
        if left + right != 0 {
            self.angle_ticks = (self.angle_ticks + (right - left) * quantum * mult)
                .clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.repeated(Key::DOWN, delta) as i32;
        let up = self.repeated(Key::UP, delta) as i32;
        if up + down != 0 {
            self.power_ticks =
                (self.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }

        // Airburst fuse.
        let fuse_step = 4.0 * mult as f64;
        let minus = self.repeated(Key::BRACKETLEFT, delta) as f64;
        let plus = self.repeated(Key::BRACKETRIGHT, delta) as f64;
        if minus + plus > 0.0 {
            if let Some(r) = self.run.as_mut() {
                if r.loadout.airburst() {
                    let f = r.fuse + (plus - minus) * fuse_step;
                    r.set_fuse(f);
                }
            }
        }

        // Enfilade reposition.
        let l = self.pressed(Key::COMMA);
        let rr = self.pressed(Key::PERIOD);
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

        if self.pressed(Key::SPACE) || self.pressed(Key::ENTER) {
            self.fire();
        }
    }

    // ------------------------------------------------------------- run steps

    fn fire(&mut self) {
        let shot = Shot::new(self.angle_ticks, self.power_ticks);
        let Some(run) = self.run.as_mut() else { return };
        if run.shells <= 0 {
            return;
        }
        let before = run.screen.clone();
        let Some(report) = run.fire(shot, true) else {
            return;
        };
        let longest = report
            .traces
            .iter()
            .map(|t| t.flight_time)
            .fold(0.0f64, f64::max);
        let collapsed = report.collapsed.clone();
        self.anim = Some(Anim {
            traces: report.traces,
            before,
            t: 0.0,
            longest,
            collapsed,
            settled: 0.0,
        });
    }

    fn step(&mut self, delta: f64) {
        if self.mode != Mode::Playing {
            return;
        }

        // Advance the shot replay. The sim ran at a fixed 1/120; this only
        // walks the recorded path, so playback speed cannot affect anything.
        if let Some(anim) = self.anim.as_mut() {
            anim.t += delta;
            if anim.done() {
                anim.settled += delta;
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
        // The paywall gate. Brief M6: free to play through screen 15.
        let index = self.run.as_ref().map(|r| r.screen_index).unwrap_or(0);
        if !self.save.unlocked && index >= FREE_SCREENS {
            self.mode = Mode::Paywall;
        }
    }

    fn unlock(&mut self) {
        // A real store call goes here: the one-time unlock IAP. The platform
        // SDK is not wired up yet, so this grants it locally and persists the
        // flag — the same code path a successful purchase or a restore would
        // take.
        self.save.unlocked = true;
        self.save.store();
        self.mode = Mode::Playing;
        self.note("UNLOCKED".into());
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
            self.save.record(entry);
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
        let want_w = self.pick_width();
        if self.run.is_none() && want_w != self.buffer_w {
            self.buffer_w = want_w;
            self.apply_buffer_size();
        }
        match self.mode {
            Mode::Playing | Mode::Paywall => self.render_play(),
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
                    self.angle_ticks = 700;
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
        View {
            cam_x: self.cam_x,
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
                self.centred(y, "ENTER  DAILY RUN", fg);
                y += 8;
                self.centred(y, "S      SEED ENTRY", fg);
                y += 8;
                self.centred(y, "ESC    QUIT", dim);
                y += 14;
                let best = self.save.best().copied();
                if let Some(b) = best {
                    self.centred(
                        y,
                        &format!("BEST {} SCREENS  SEED {}", b.screens, b.seed),
                        alert,
                    );
                    y += 10;
                }
                if !self.save.unlocked {
                    self.centred(y, &format!("FREE THROUGH SCREEN {FREE_SCREENS}"), dim);
                    y += 8;
                }
                let n = self.save.scores.len().min(5);
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
            Mode::SeedEntry => {
                self.centred(y, "ENTER A SEED", dim);
                y += 14;
                let text = self.seed_text.clone();
                let shown = format!("> {text}");
                self.centred(y, &shown, fg);
                y += 14;
                self.centred(y, "ANY TEXT WORKS. BLANK USES TODAY'S DAILY.", dim);
                y += 8;
                self.centred(y, "ENTER  START     ESC  BACK", fg);
                y += 14;
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
                self.centred(y, "R      SAME SEED AGAIN", fg);
                y += 8;
                self.centred(y, "C      COPY SEED", fg);
                y += 8;
                self.centred(y, "ENTER  TITLE", fg);
                y += 12;
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

        self.render_hud(
            shells, index, wind_shown, range, fuse, &loadout, &screen, phase,
        );

        if phase == Phase::Drafting {
            self.render_draft(&offers, &loadout);
        }
        if self.mode == Mode::Paywall {
            self.render_paywall();
        }
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

        // Wind, top left. Shown as a signed number, never an arrow — reading a
        // number and converting it to a correction is the core skill
        // (DESIGN §2.5).
        let wind_text = match wind {
            None => "--".to_string(),
            Some(v) => {
                if loadout.has_flag(flags::ANEMOMETER) {
                    format!("{}{:.1}", if v >= 0.0 { "+" } else { "-" }, v.abs())
                } else {
                    format!("{}{:.0}", if v >= 0.0 { "+" } else { "-" }, v.abs())
                }
            }
        };
        self.canvas.plate(
            3,
            3,
            20 + crate::font::big_width(&wind_text) as i32,
            13,
            &pal,
        );
        self.canvas.text_small(6, 5, "WIND", dim);
        self.canvas.text_big(6 + 18, 4, &wind_text, alert);

        // Shells, top right. Alert accent under 10 — the only other place that
        // colour is allowed (DESIGN §5.2).
        let shell_text = format!("{shells}");
        let plate_w = 26 + crate::font::big_width(&shell_text) as i32;
        let px = w - plate_w - 3;
        self.canvas.plate(px, 3, plate_w, 13, &pal);
        self.canvas.text_small(px + 3, 5, "SHELLS", dim);
        let c = if shells < 10 { alert } else { fg };
        self.canvas.text_big(px + 3 + 25, 4, &shell_text, c);

        // Screen index, bottom right, small. No score during the run
        // (DESIGN §1).
        let idx = format!("{}", index + 1);
        let ix = w - crate::font::small_width(&idx) as i32 - 3;
        self.canvas.text_small(ix, self.canvas.h - 8, &idx, fg);

        // Instruments, bottom left.
        let mut y = self.canvas.h - 8;
        let shot = Shot::new(self.angle_ticks, self.power_ticks).quantized(loadout);
        let angle = format!("ANG {:.1}", shot.angle_degrees());
        let power = format!("PWR {}", shot.power_ticks);
        self.canvas.text_small(3, y, &power, fg);
        y -= 7;
        self.canvas.text_small(3, y, &angle, fg);
        if let Some(r) = range {
            if loadout.has_flag(flags::RANGEFINDER) {
                y -= 7;
                self.canvas.text_small(3, y, &format!("RNG {:.0}", r), fg);
            }
        }
        if loadout.airburst() {
            y -= 7;
            self.canvas.text_small(3, y, &format!("FUSE {fuse:.0}"), fg);
        }
        if loadout.enfilade_units > 0.0 {
            y -= 7;
            let off = self
                .run
                .as_ref()
                .map(|r| r.emplacement_offset)
                .unwrap_or(0.0);
            self.canvas
                .text_small(3, y, &format!("EMPL {off:+.0}"), dim);
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
            self.canvas.text_small(3, 18, &line, dim);
        }

        if self.status_time > 0.0 {
            let text = self.status.clone();
            let x = (w - crate::font::small_width(&text) as i32) / 2;
            self.canvas.text_small(x, 24, &text, alert);
        }

        // Only once the shell has actually landed: the sim resolves the clear
        // the instant the trigger is pulled, and announcing it while the shot
        // is still visibly in the air gives the result away.
        let _ = phase;
        if self.clear_hold > 0.0 {
            let text = "SCREEN CLEAR";
            let x = (w - crate::font::small_width(text) as i32) / 2;
            self.canvas.text_small(x, 34, text, fg);
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

        let hint = "1 2 3  CHOOSE";
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
        let bh = 72;
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
        self.centred(cy, "ENTER  END THE RUN", dim);
    }
}

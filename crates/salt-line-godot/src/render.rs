//! Drawing a frame of the game, with no engine in it.
//!
//! Every function here reads a [`Game`] and paints one frame into a
//! [`Canvas`]: terrain, sprites, HUD and menus in the one buffer at the
//! internal resolution (DESIGN §5.1). Nothing is decided here. What the core
//! does not keep and a frame needs comes in as [`Art`], the palette and the
//! sprites the node loaded, and [`Cache`], work kept between frames because
//! it is dear (the drag preview's flight).
//!
//! While drawing, each on-screen control is laid out as a [`Button`] for the
//! next frame's touches to be tested against (`touch`'s immediate-mode
//! layout). The node hands the list to `Game::set_buttons` for the next
//! touches.
//!
//! Because nothing here needs the engine, a frame can be drawn under
//! `cargo test` and compared pixel for pixel; see the tests at the end.
//!
//! # Techniques
//!
//! * **Software rasterising into one buffer.** Every pixel is written by
//!   this crate into an RGBA byte array of `buffer_w` × `buffer_h`; the node
//!   uploads it in strips, the ones that changed. There is no scene graph, no sprite
//!   node, and no GPU scaling except the final nearest-neighbour stretch.
//!   In play the sky and the ground go into layers of their own under it,
//!   kept until they change (`backdrop`, through [`Cache`]); the canvas then
//!   holds everything else over transparent pixels.
//! * **Painter's order.** Each frame starts from a full fill (sky or plate)
//!   and draws back to front: sky, terrain, overlays, last arc, structures,
//!   the emplacement, the shot, effects, the drag preview, HUD, touch
//!   controls, draft, pause.
//!   Nothing is erased; the next frame paints over everything, which the
//!   `every_pixel_of_every_screen_is_painted` test checks.
//! * **Immediate-mode UI.** A control is laid out by drawing it:
//!   `draw_touch_button` and `menu_row` paint the plate and push a
//!   [`Button`] in the same call, so what can be touched is always what is
//!   on screen, and a menu row has one definition, not a layout plus a
//!   drawing.
//! * **Memoisation.** The drag preview flies a whole ballistic path; it is
//!   kept in [`Cache`] keyed on everything it depends on (`PreviewKey`) and
//!   recomputed only when one of them changes.
//! * **Snapshot tests.** Frames are rendered to palette-index text
//!   (`Canvas::palette_text`) and pinned with `insta`, so a pixel that moves
//!   shows in review as a text diff.
//!
//! Units: every coordinate here is in buffer pixels, `(0, 0)` top left;
//! world positions go through [`View`] (`sx`, `sy`) with the camera applied.

use salt_line_sim::ballistics::{self, FireOptions, Shot, Trace};
use salt_line_sim::modifiers::{flags, Loadout, Modifier};
use salt_line_sim::run::{format_seed, parse_seed, Phase, Screen, StructState};
use salt_line_sim::{Vec2, DT};

use crate::app::{Anim, Game, Mode, SHORT_ARC_SECONDS, TOPPLE_TIME};
use crate::backdrop::Backdrop;
use crate::biome::Biome;
use crate::frame::Memo;
use crate::input::Action;
use crate::keypad;
use crate::palette::{slot, Palette};
use crate::presentation;
use crate::raster::{self, Canvas, Sprite, View};
use crate::save_format::{ArcHint, TouchAim};
use crate::sky;
use crate::sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT, PIVOT_BASE};
use crate::touch::{Button, Rect};

/// What the node loaded for drawing: the palette sampled from
/// `art/palette_16.png`, sprites under `art/sprites`, and full-frame art.
/// Missing assets fall back to the built-in palette or procedural drawing.
#[derive(Default)]
pub struct Art {
    pub pal: Palette,
    pub sprites: Option<Sprites>,
    pub title_background: Option<Sprite>,
    pub trial_background: Option<Sprite>,
}

/// Drawing work kept from one frame to the next.
#[derive(Default)]
pub struct Cache {
    /// The drag preview's trajectory, recomputed only when its inputs change.
    preview: Memo<PreviewKey, Preview>,
    /// The sky and ground layers, when frames are drawn in layers; see
    /// `backdrop`. Flat unless the node turns `layered` on.
    pub backdrop: Backdrop,
}

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

/// Draw the frame for the game's mode, and lay out its touch buttons in
/// `buttons` (cleared first).
pub fn frame(
    g: &Game,
    art: &Art,
    canvas: &mut Canvas,
    buttons: &mut Vec<Button>,
    cache: &mut Cache,
) {
    buttons.clear();
    cache.backdrop.begin_frame();
    let mut p = Painter::new(g, art, canvas, buttons, cache);
    match g.mode {
        Mode::Playing | Mode::Paused => p.render_play(),
        Mode::Versus => p.render_versus(),
        Mode::Editor => p.render_editor(),
        Mode::Trial => p.render_trial(),
        _ => p.render_menu(),
    }
    if g.in_demo() {
        p.render_demo_card();
        p.render_demo_banner();
    }
    if g.watching() {
        p.render_watch_bar();
    }
}

/// The attract demo's banner (DESIGN §12), on a touchscreen: always on
/// screen while the demo runs, so nobody mistakes it for a game in
/// progress.
pub const DEMO_BANNER: &str = "DEMO \u{b7} TAP TO PLAY";
/// The banner when keys or a controller are the input in use.
pub const DEMO_BANNER_KEYS: &str = "DEMO \u{b7} PRESS ANY KEY";

/// Draw the playfield alone, whatever the mode, adding its buttons to
/// `buttons`. For `--salt-gallery`.
pub fn play(
    g: &Game,
    art: &Art,
    canvas: &mut Canvas,
    buttons: &mut Vec<Button>,
    cache: &mut Cache,
) {
    cache.backdrop.begin_frame();
    Painter::new(g, art, canvas, buttons, cache).render_play();
}

/// Draw the error screen shown after a panic (`Game::broken_tick`).
pub fn broken(
    g: &Game,
    art: &Art,
    canvas: &mut Canvas,
    buttons: &mut Vec<Button>,
    cache: &mut Cache,
) {
    cache.backdrop.begin_frame();
    Painter::new(g, art, canvas, buttons, cache).render_broken();
}

/// One frame being drawn.
struct Painter<'a> {
    g: &'a Game,
    pal: &'a Palette,
    sprites: Option<&'a Sprites>,
    title_background: Option<&'a Sprite>,
    trial_background: Option<&'a Sprite>,
    canvas: &'a mut Canvas,
    buttons: &'a mut Vec<Button>,
    cache: &'a mut Cache,
}

impl<'a> Painter<'a> {
    fn new(
        g: &'a Game,
        art: &'a Art,
        canvas: &'a mut Canvas,
        buttons: &'a mut Vec<Button>,
        cache: &'a mut Cache,
    ) -> Self {
        Painter {
            g,
            pal: &art.pal,
            sprites: art.sprites.as_ref(),
            title_background: art.title_background.as_ref(),
            trial_background: art.trial_background.as_ref(),
            canvas,
            buttons,
            cache,
        }
    }
}

impl Painter<'_> {
    fn render_trial(&mut self) {
        let Some(trial_state) = self.g.trial.as_ref() else {
            self.canvas.fill(self.pal.get(slot::UI_BG));
            self.centred(80, "TRIAL UNAVAILABLE", self.pal.get(slot::ALERT));
            return;
        };
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let alert = pal.get(slot::ALERT);
        let width = self.canvas.w;
        let height = self.canvas.h;
        let viewport = crate::scene::WorldViewport::fit(width, height, 320.0, 180.0);
        if let Some(background) = self.trial_background {
            Self::draw_scaled_background(self.canvas, background, viewport);
        } else {
            raster::draw_sky(self.canvas, &self.g.view(), pal);
            crate::scene::draw_coast(self.canvas, pal);
        }
        let project = |x: f64, y: f64| viewport.project(Vec2::new(x, y));
        let loadout = Loadout::default();
        let shot = Shot::new(trial_state.angle_ticks, trial_state.power_ticks);
        let (tank_x, tank_y) = project(30.0, 15.0);
        crate::scene::draw_emplacement(self.canvas, self.sprites, &loadout, shot, tank_x, tank_y);

        let direction = shot.velocity(&loadout);
        let length = direction.x.hypot(direction.y).max(1.0);
        for offset in (18..40).step_by(4) {
            let (x, y) = project(
                30.0 + direction.x / length * offset as f64,
                15.0 + direction.y / length * offset as f64,
            );
            self.canvas.put(x, y, alert);
        }

        let simulation = &trial_state.simulation;
        for drone in simulation.drones() {
            let (x, y) = project(drone.position.x, drone.position.y);
            self.canvas.hline(x - 8, x + 8, y - 3, fg);
            self.canvas.rect_outline(x - 9, y - 5, 5, 3, fg);
            self.canvas.rect_outline(x + 4, y - 5, 5, 3, fg);
            let (body_x, body_y) = project(drone.position.x - 5.0, drone.position.y + 3.0);
            self.canvas.rect(body_x, body_y, 10, 6, alert);
        }
        for projectile in simulation.projectiles() {
            let (x, y) = project(projectile.position.x, projectile.position.y);
            self.canvas.rect(x, y, 2, 2, fg);
        }

        let seconds = (salt_line_sim::trial::ROUND_TICKS - simulation.tick()).div_ceil(120);
        self.canvas.text_small(
            6,
            6,
            &format!(
                "TRIAL {seconds}s  SCORE {}  SHELLS {}/8",
                simulation.score(),
                simulation.ammo()
            ),
            fg,
        );
        self.canvas.text_small(
            6,
            16,
            &format!(
                "WIND {:.1}  ANG {:.1}  PWR {}  RELOAD {}s",
                simulation.wind(),
                trial_state.angle_ticks as f64 * 0.05,
                trial_state.power_ticks,
                simulation.reload_ticks_remaining().div_ceil(120)
            ),
            fg,
        );

        if simulation.finished() {
            self.centred(65, "TRIAL COMPLETE", alert);
            if self.g.input.touch_active {
                self.draw_touch_button(
                    width / 2 - 64,
                    height - 22,
                    60,
                    16,
                    "REPLAY",
                    Action::Confirm,
                );
                self.draw_touch_button(width / 2 + 4, height - 22, 60, 16, "TITLE", Action::Back);
            } else {
                self.centred(77, "ENTER / A REPLAY   ESC / B TITLE", fg);
            }
        } else if trial_state.paused {
            self.centred(65, "PAUSED", alert);
            if self.g.input.touch_active {
                self.draw_touch_button(
                    width / 2 - 64,
                    height - 22,
                    60,
                    16,
                    "RESUME",
                    Action::Confirm,
                );
                self.draw_touch_button(width / 2 + 4, height - 22, 60, 16, "TITLE", Action::Back);
            } else {
                self.centred(77, "ENTER / A RESUME   ESC / B TITLE", fg);
            }
        } else if self.g.input.touch_active {
            let bottom = height - 20;
            self.draw_touch_button(3, bottom, 28, 16, "<", Action::Left);
            self.draw_touch_button(33, bottom, 28, 16, ">", Action::Right);
            self.draw_touch_button(75, bottom, 28, 16, "-", Action::Down);
            self.draw_touch_button(105, bottom, 28, 16, "+", Action::Up);
            self.draw_touch_button(width - 48, bottom, 45, 16, "FIRE", Action::Fire);
            let (edge_x, edge_y) = self.g.display.corner_pad;
            self.draw_touch_button(
                width - 45 - edge_x,
                3 + edge_y,
                42,
                13,
                "PAUSE",
                Action::Back,
            );
        } else {
            self.centred(
                height - 9,
                "ARROWS AIM/POWER  SPACE/A FIRE  ESC/B PAUSE",
                fg,
            );
        }
    }

    fn draw_scaled_background(
        canvas: &mut Canvas,
        background: &Sprite,
        viewport: crate::scene::WorldViewport,
    ) {
        let (x, y, width, height) = viewport.bounds(320.0);
        let scale = canvas.scale() as f64;
        let x0 = (x * scale).round() as i32;
        let y0 = (y * scale).round() as i32;
        let x1 = ((x + width) * scale).round() as i32;
        let y1 = ((y + height) * scale).round() as i32;
        let target_width = x1 - x0;
        let target_height = y1 - y0;
        if target_width <= 0 || target_height <= 0 {
            return;
        }
        let source_x_max = background.w.saturating_sub(1);
        let source_y_max = background.h.saturating_sub(1);
        for dy in y0..y1 {
            let sy = if target_height == 1 {
                0
            } else {
                ((dy - y0) * source_y_max / (target_height - 1)).min(source_y_max)
            };
            for dx in x0..x1 {
                let sx = if target_width == 1 {
                    0
                } else {
                    ((dx - x0) * source_x_max / (target_width - 1)).min(source_x_max)
                };
                if let Some(colour) = background.pixel(sx, sy) {
                    canvas.put_detail(dx, dy, colour);
                }
            }
        }
    }

    /// The error screen. See `Game::broken_tick`.
    fn render_broken(&mut self) {
        self.buttons.clear();
        let pal = self.pal;
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
        let copy = self.g.input_label(Action::Copy);
        y = self.menu_row(
            y,
            Action::Copy,
            &format!("{copy}      COPY REPORT"),
            "COPY REPORT",
            fg,
        );
        let back = self.g.input_label(Action::Back);
        y = self.menu_row(y, Action::Back, &format!("{back}  QUIT"), "QUIT", fg);
        y += 4;
        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
            self.centred(y, &text, alert);
        }
    }

    /// Every menu mode: the title, Controls (touch and keyed layouts), seed
    /// entry and the result screen, over the game's own sky.
    fn render_menu(&mut self) {
        let pal = self.pal;
        let view = self.g.view();
        // A calm sky behind the menus, with the same dither as the game.
        raster::draw_sky(self.canvas, &view, pal);
        // Keep the native title art centred. Extend its edge colours into
        // wider phone margins so the horizon meets the painting seamlessly.
        if matches!(self.g.mode, Mode::Title) {
            if let Some(background) = self.title_background {
                let x = (self.canvas.w - background.w) / 2;
                let y = (self.canvas.h - background.h) / 2;
                if let Some(detail) = background
                    .detail
                    .as_ref()
                    .filter(|_| x > 0 && self.canvas.scale() == 4)
                {
                    let right = (x + background.w) * 4;
                    for row in 0..detail.h {
                        for col in 0..x * 4 {
                            if let Some(c) = detail.pixel(0, row) {
                                self.canvas.put_detail(col, y * 4 + row, c);
                            }
                        }
                        for col in right..self.canvas.pixel_w() {
                            if let Some(c) = detail.pixel(detail.w - 1, row) {
                                self.canvas.put_detail(col, y * 4 + row, c);
                            }
                        }
                    }
                } else if x > 0 {
                    let right = x + background.w;
                    for row in 0..background.h {
                        if let Some(c) = background.pixel(0, row) {
                            self.canvas.rect(0, y + row, x, 1, c);
                        }
                        if let Some(c) = background.pixel(background.w - 1, row) {
                            self.canvas
                                .rect(right, y + row, self.canvas.w - right, 1, c);
                        }
                    }
                }
                self.canvas.blit(background, x, y);
            }
        }
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
        if matches!(self.g.mode, Mode::Title) {
            // An instrument-panel heading, using the same native grid and
            // palette as the coast below it. Menu rows keep their hit areas.
            self.canvas
                .hline(tx - 12, tx + tw + 10, y + 8, pal.get(slot::HULL_EDGE));
            self.canvas.rect(tx - 10, y + 1, 3, 3, alert);
            self.canvas.rect(tx + tw + 6, y + 1, 3, 3, alert);
        }
        let title_colour = if matches!(self.g.mode, Mode::Title) {
            fg
        } else {
            alert
        };
        for (i, ch) in title.chars().enumerate() {
            let s = ch.to_string();
            self.canvas
                .text_small(tx + i as i32 * tracking, y, &s, title_colour);
        }
        y += 12;

        match self.g.mode {
            Mode::Title => {
                if self.g.input.touch_active {
                    let (ex, ey) = self.g.display.corner_pad;
                    let label = if self.g.save.title_music {
                        "MUS ON"
                    } else {
                        "MUS OFF"
                    };
                    self.draw_touch_button(w - 42 - ex, 3 + ey, 42, 13, label, Action::TitleMusic);
                    // The demo mirrors the music toggle in the other corner,
                    // which keeps the menu rows (and the board under them)
                    // where they were.
                    self.draw_touch_button(3 + ex, 3 + ey, 42, 13, "DEMO", Action::Demo);
                    // REPLAYS under it: watching, like the demo, and the
                    // phone title has no row to spare (DESIGN §11.6).
                    self.draw_touch_button(3 + ex, 19 + ey, 42, 13, "REPLAYS", Action::Watch);
                    self.draw_touch_button(3 + ex, 35 + ey, 42, 13, "CHALL", Action::Challenges);
                }
                self.centred(y, "ARTILLERY ROGUELIKE", dim);
                y += 14;
                let confirm = self.g.input_label(Action::Confirm);
                // With a saved run, confirm continues it and a new run is a
                // separate action that asks twice (`confirm_discard`).
                let again = "PRESS AGAIN - SAVED RUN IS LOST";
                if self.g.run_file.resumable.is_some() {
                    y = self.menu_row(
                        y,
                        Action::Confirm,
                        &format!("{confirm}  CONTINUE"),
                        "CONTINUE",
                        alert,
                    );
                    let key = self.g.input_label(Action::NewRun);
                    let text = if self.g.run_file.discard_armed == Some("daily") {
                        again
                    } else {
                        "NEW DAILY RUN"
                    };
                    y = self.menu_row(y, Action::NewRun, &format!("{key}      {text}"), text, fg);
                } else {
                    y = self.menu_row(
                        y,
                        Action::Confirm,
                        &format!("{confirm}  DAILY RUN"),
                        "DAILY RUN",
                        fg,
                    );
                }
                let seed = self.g.input_label(Action::Seed);
                let seed_text = if self.g.run_file.discard_armed == Some("seed") {
                    again
                } else {
                    "SEED ENTRY"
                };
                y = self.menu_row(
                    y,
                    Action::Seed,
                    &format!("{seed}      {seed_text}"),
                    seed_text,
                    fg,
                );
                if !self.plated() {
                    let challenges = self.g.input_label(Action::Challenges);
                    let total = salt_line_sim::challenge::ALL.len();
                    let cleared = self.g.save.challenges.len();
                    y = self.menu_row(
                        y,
                        Action::Challenges,
                        &format!("{challenges}  CHALLENGES {cleared}/{total}"),
                        &format!("CHALLENGES {cleared}/{total}"),
                        alert,
                    );
                    let trial = self.g.input_label(Action::DroneTrial);
                    y = self.menu_row(
                        y,
                        Action::DroneTrial,
                        &format!("{trial}  DRONE TRIAL (PROTOTYPE)"),
                        "DRONE TRIAL (PROTOTYPE)",
                        fg,
                    );
                }
                // Local versus is offered as a row on a touchscreen, the
                // device it is for. With keys it is the versus binding (2)
                // alone, so the keyed title stays as it was, unless a match
                // is waiting to be resumed. A suspended match takes the
                // versus row's place rather than adding one (DESIGN §11.6):
                // the phone title has no row to spare.
                let resume_match = self.g.saved_match.is_some();
                if self.plated() {
                    let (text, c) = if resume_match {
                        ("RESUME MATCH", alert)
                    } else {
                        ("LOCAL VERSUS", fg)
                    };
                    y = self.menu_row(y, Action::Versus, "", text, c);
                } else if resume_match {
                    let key = self.g.input_label(Action::Versus);
                    y = self.menu_row(
                        y,
                        Action::Versus,
                        &format!("{key}      RESUME MATCH"),
                        "RESUME MATCH",
                        alert,
                    );
                }
                let controls = self.g.input_label(Action::Controls);
                y = self.menu_row(
                    y,
                    Action::Controls,
                    &format!("{controls}  SETTINGS"),
                    "SETTINGS",
                    fg,
                );
                // On a touchscreen the demo is the corner plate above.
                if !self.plated() {
                    let demo = self.g.input_label(Action::Demo);
                    y = self.menu_row(y, Action::Demo, &format!("{demo}  DEMO"), "DEMO", fg);
                    let watch = self.g.input_label(Action::Watch);
                    y = self.menu_row(
                        y,
                        Action::Watch,
                        &format!("{watch}  REPLAYS"),
                        "REPLAYS",
                        fg,
                    );
                }
                let back = self.g.input_label(Action::Back);
                // Armed, the row says what the second press does, as a
                // match's TITLE does (`Game::quit_armed`).
                let (quit, quit_c) = match (self.g.quit_armed.is_some(), self.plated()) {
                    (false, _) => ("QUIT", dim),
                    (true, true) => (QUIT_AGAIN_TAP, fg),
                    (true, false) => (QUIT_AGAIN_KEY, fg),
                };
                y = self.menu_row(y, Action::Back, &format!("{back}  {quit}"), quit, quit_c);
                if self.g.crash.last_crash.is_some() {
                    let text = if self.g.crash.crash_copied {
                        "CRASH REPORT COPIED"
                    } else {
                        "COPY LAST CRASH REPORT"
                    };
                    let copy = self.g.input_label(Action::Copy);
                    y = self.menu_row(y, Action::Copy, &format!("{copy}      {text}"), text, alert);
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
            Mode::Controls if self.g.input.touch_active => {
                self.centred(y, "SETTINGS", fg);
                y += 12;
                let muted = if self.g.save.muted { " MUTED" } else { "" };
                let text = format!("VOLUME {}%{muted}", self.g.save.volume);
                let tw = crate::font::small_width(&text) as i32;
                let tx = (w - tw) / 2;
                self.canvas.text_small(tx, y, &text, alert);
                self.draw_touch_button(tx - 26, y - 6, 20, 17, "-", Action::Left);
                self.draw_touch_button(tx + tw + 6, y - 6, 20, 17, "+", Action::Right);
                y += 16;
                let mute = if self.g.save.muted { "UNMUTE" } else { "MUTE" };
                self.draw_touch_button(w / 4 - 30, y - 4, 60, 13, mute, Action::Mute);
                let season = format!(
                    "SEASONAL: {}",
                    self.g.save.seasonal.unwrap_or_default().label()
                );
                let sw = crate::font::small_width(&season) as i32 + 20;
                self.draw_touch_button(
                    3 * w / 4 - sw / 2,
                    y - 4,
                    sw,
                    13,
                    &season,
                    Action::Seasonal,
                );
                y += 16;
                let last = if self.g.last_arc_on() {
                    "LAST ARC: ON"
                } else {
                    "LAST ARC: OFF"
                };
                y = self.menu_row(y, Action::LastArc, "", last, fg);
                let demo = if self.g.idle_demo_on() {
                    "IDLE DEMO: ON"
                } else {
                    "IDLE DEMO: OFF"
                };
                y = self.menu_row(y, Action::Demo, "", demo, fg);
                let aim = match self.g.touch_aim() {
                    TouchAim::Buttons => "AIM: BUTTONS",
                    TouchAim::Drag => "AIM: DRAG",
                };
                y = self.menu_row(y, Action::AimMode, "", aim, fg);
                if self.g.touch_aim() == TouchAim::Drag {
                    let arc = match self.g.arc_hint() {
                        ArcHint::Short => "ARC: SHORT",
                        ArcHint::Full => "ARC: FULL",
                        ArcHint::Wind => "ARC: FULL + WIND",
                    };
                    y = self.menu_row(y, Action::ArcHint, "", arc, fg);
                }
                y += 4;
                y = self.menu_row(y, Action::Confirm, "", "CREDITS / LICENSES", fg);
                self.menu_row(y, Action::Back, "", "BACK", fg);
            }
            Mode::Controls => {
                self.centred(y, "SETTINGS", fg);
                y += 12;
                let muted = if self.g.save.muted { " MUTED" } else { "" };
                self.centred(
                    y,
                    &format!("< >  VOLUME {}%{muted}", self.g.save.volume),
                    alert,
                );
                y += 10;
                let rows = [
                    ("AIM / POWER", Action::Left, Some(Action::Up)),
                    ("FIRE", Action::Fire, None),
                    ("FUSE", Action::FuseDown, Some(Action::FuseUp)),
                    (
                        "REPOSITION",
                        Action::RepositionLeft,
                        Some(Action::RepositionRight),
                    ),
                    ("DRAFT", Action::Draft1, Some(Action::Draft2)),
                    ("BACK", Action::Back, None),
                ];
                for (name, a, b) in rows {
                    let mut labels = self.g.input_label(a);
                    if let Some(b) = b {
                        labels.push('/');
                        labels.push_str(&self.g.input_label(b));
                    }
                    self.centred(y, &format!("{name}: {labels}"), fg);
                    y += 7;
                }
                y += 4;
                // Both the key and the button, whichever is in use.
                let (mute_key, mute_pad) = self.g.action_binding(Action::Mute);
                let (reset_key, reset_pad) = self.g.action_binding(Action::Replay);
                self.centred(
                    y,
                    &format!("{mute_key} / {mute_pad}  MUTE     {reset_key} / {reset_pad}  RESET"),
                    dim,
                );
                y += 7;
                let on = if self.g.last_arc_on() { "ON" } else { "OFF" };
                let toggle = format!("{}  LAST ARC: {on}", self.g.input_label(Action::LastArc));
                self.centred(y, &toggle, fg);
                y += 7;
                let on = if self.g.idle_demo_on() { "ON" } else { "OFF" };
                let toggle = format!("{}  IDLE DEMO: {on}", self.g.input_label(Action::Demo));
                self.centred(y, &toggle, fg);
                y += 7;
                let season = format!(
                    "{}  SEASONAL: {}",
                    self.g.input_label(Action::Seasonal),
                    self.g.save.seasonal.unwrap_or_default().label()
                );
                self.centred(y, &season, fg);
                y += 9;
                self.centred(y, "EDIT bind LINES IN salt_line.txt TO REMAP", dim);
                y += 7;
                self.centred(y, "FORMAT: bind ACTION KEY PAD", dim);
                y += 9;
                let confirm = self.g.input_label(Action::Confirm);
                self.centred(y, &format!("{confirm}  CREDITS / LICENSES"), fg);
                y += 9;
                let back = self.g.input_label(Action::Back);
                self.centred(y, &format!("{back}  BACK"), fg);
            }
            Mode::Credits => {
                let pages = crate::credits::pages();
                let page = self.g.credits_page.min(pages.len() - 1);
                self.centred(y, "CREDITS / LICENSES", fg);
                y += 12;
                for line in &pages[page] {
                    self.centred(y, line, fg);
                    y += 7;
                }
                y = 130;
                self.centred(y, &format!("PAGE {} / {}", page + 1, pages.len()), fg);
                y += 12;
                if self.plated() {
                    self.draw_touch_button(w / 2 - 90, y - 4, 50, 13, "PREV", Action::Left);
                    self.draw_touch_button(w / 2 + 40, y - 4, 50, 13, "NEXT", Action::Right);
                } else {
                    let left = self.g.input_label(Action::Left);
                    let right = self.g.input_label(Action::Right);
                    self.centred(y, &format!("{left} / {right}  CHANGE PAGE"), dim);
                }
                y += 17;
                let back = self.g.input_label(Action::Back);
                self.menu_row(y, Action::Back, &format!("{back}  BACK"), "BACK", fg);
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
                    y = self.menu_row(y, Action::Confirm, "", "START", fg);
                    y = self.menu_row(y, Action::Back, "", "BACK", dim);
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
                let is_challenge =
                    self.g.play.challenge.is_some() || self.g.play.custom_challenge.is_some();
                let challenge_over =
                    is_challenge && self.g.play.run.as_ref().is_some_and(|r| r.is_over());
                let heading = if is_challenge {
                    if challenge_over {
                        "CHALLENGE FAILED"
                    } else {
                        "CHALLENGE COMPLETE"
                    }
                } else {
                    "OUT OF SHELLS"
                };
                self.centred(y, heading, alert);
                y += 14;
                if let Some(layout) = self.g.play.custom_challenge.as_ref() {
                    let best = self
                        .g
                        .save
                        .custom_challenge_best(layout.digest())
                        .map_or("-".to_string(), |left| left.to_string());
                    let left = self.g.play.run.as_ref().map_or(0, |run| run.shells.max(0));
                    self.centred(
                        y,
                        &format!("CUSTOM {}", crate::layouts_file::fingerprint(layout)),
                        fg,
                    );
                    y += 8;
                    self.centred(y, &format!("{left} SHELLS LEFT  BEST {best}"), dim);
                    y += 14;
                } else if let Some(index) = self.g.play.challenge {
                    let bests = salt_line_sim::challenge::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, c)| match self.g.save.challenge_best(i) {
                            Some(left) => format!("{} {left}", c.name),
                            None => format!("{} -", c.name),
                        })
                        .collect::<Vec<_>>()
                        .join("   ");
                    let name = salt_line_sim::challenge::get(index).map_or("", |c| c.name);
                    self.centred(y, name, fg);
                    y += 8;
                    self.centred(y, &bests, dim);
                    y += 8;
                    self.centred(y, "BEST SHELLS LEFT", dim);
                    y += 14;
                } else if let Some(run) = self.g.play.run.as_ref() {
                    let (screens, shells, shots) = run.score();
                    let seed = run.run_seed;
                    self.centred(y, &format!("{screens} SCREENS CLEARED"), fg);
                    y += 8;
                    self.centred(y, &format!("{shots} SHOTS   {shells} SHELLS LEFT"), dim);
                    y += 8;
                    self.centred(y, &format!("SEED {}", format_seed(seed)), dim);
                    y += 14;
                }
                // WATCH only for a run kept to watch (not a `--salt-screen`
                // jump, whose log cannot rebuild it).
                let challenge = is_challenge;
                let rows: Vec<(Action, &str)> = if challenge {
                    let again = if challenge_over {
                        "RETRY"
                    } else {
                        "PLAY AGAIN"
                    };
                    let mut rows = vec![(Action::Replay, again)];
                    if self.g.play.custom_challenge.is_some() {
                        rows.push((Action::Watch, "SAVE MAP"));
                    }
                    rows.extend([(Action::Copy, "COPY CODE"), (Action::Confirm, "TITLE")]);
                    rows
                } else {
                    [
                        (Action::Replay, "SAME SEED AGAIN"),
                        (Action::Watch, "WATCH"),
                        (Action::Copy, "COPY SEED"),
                        (Action::Confirm, "TITLE"),
                    ]
                    .into_iter()
                    .filter(|&(a, _)| a != Action::Watch || self.g.history.last_run.is_some())
                    .collect()
                };
                let labels: Vec<(String, &str)> = rows
                    .iter()
                    .map(|&(action, text)| (self.g.input_label(action), text))
                    .collect();
                for (&(action, text), keyed) in rows.iter().zip(keyed_rows(&labels)) {
                    y = self.menu_row(y, action, &keyed, text, fg);
                }
                y += 4;
                if self.g.status_time > 0.0 {
                    let text = self.g.status.clone();
                    self.centred(y, &text, alert);
                }
            }
            Mode::VersusSetup => self.render_versus_setup(y),
            Mode::Replays => self.render_replays(y),
            _ => {}
        }
    }

    fn centred(&mut self, y: i32, text: &str, c: crate::palette::Rgba) {
        let width = crate::font::small_width(text) as i32;
        let x = (self.canvas.w - width) / 2;
        if self.g.mode == Mode::Title && self.title_background.is_some() {
            // The literal board scene is busier than the old title painting.
            // Back only the labels, preserving the art and all menu geometry.
            self.canvas
                .rect(x - 2, y - 1, width + 4, 7, self.pal.get(slot::UI_BG));
        }
        self.canvas.text_small(x.max(1), y, text, c);
    }

    /// Make a rectangle of this frame's buffer a button for the next frame's
    /// touches. Only called while touch controls are showing.
    fn touch_button(&mut self, rect: Rect, action: Action) {
        self.buttons.push(Button { rect, action });
    }

    /// A plate with a glyph on it, lit while a finger is on it.
    fn draw_touch_button(&mut self, x: i32, y: i32, w: i32, h: i32, label: &str, action: Action) {
        let pal = self.pal;
        self.canvas.plate(x, y, w, h, pal);
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
                } else if let Action::Type(c) = k {
                    let mut label = [0; 4];
                    self.draw_touch_button(x, y, kw, kh, c.encode_utf8(&mut label), k);
                    x += pitch;
                }
            }
            y += kh + 2;
        }
        y
    }

    /// Menu lines are plates rather than "KEY  TEXT": on a touchscreen, and
    /// for a controller on the seed keypad (seed entry, or the editor's
    /// FORK A SEED and NAME), where the focus moves over them.
    fn plated(&self) -> bool {
        self.g.input.touch_active || (self.g.input.controller_active && self.on_keypad())
    }

    /// A page with the seed keypad on it is up.
    fn on_keypad(&self) -> bool {
        self.g.mode == Mode::SeedEntry
            || (self.g.mode == Mode::Editor
                && matches!(
                    self.g.editor.page,
                    crate::app::EditorPage::Seed | crate::app::EditorPage::Name
                ))
    }

    /// The controller's keypad focus is on this action's plate.
    fn focused(&self, action: Action) -> bool {
        self.on_keypad()
            && self.g.input.controller_active
            && !self.g.input.touch_active
            && self.g.input.seed_focus.action() == action
    }

    /// A plate's label lights while a finger is on it or the focus is.
    fn lit(&self, action: Action) -> bool {
        self.g.input.touch.held(action) || self.focused(action)
    }

    /// A menu line. With a keyboard or controller it reads "KEY  TEXT"; on a
    /// touchscreen the line itself is the button, spaced for a finger rather
    /// than a key. Returns the y of the next line.
    fn menu_row(
        &mut self,
        y: i32,
        action: Action,
        keyed: &str,
        text: &str,
        c: crate::palette::Rgba,
    ) -> i32 {
        if !self.plated() {
            self.centred(y, keyed, c);
            return y + 8;
        }
        self.menu_row_at(y, action, text, c, 0)
    }

    /// A touchscreen's [`Self::menu_row`] plate, centred `dx` pixels off the
    /// screen's centre. Returns the y of the next line.
    fn menu_row_at(
        &mut self,
        y: i32,
        action: Action,
        text: &str,
        c: crate::palette::Rgba,
        dx: i32,
    ) -> i32 {
        let pal = self.pal;
        let tw = crate::font::small_width(text) as i32;
        let pad = 10;
        let x = (self.canvas.w - tw) / 2 - pad + dx;
        self.canvas.plate(x, y - 4, tw + 2 * pad, 13, pal);
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
        let (ex, ey) = self.g.display.corner_pad;
        let (bw, bh) = (22, 18);
        let y = h - bh - 3 - ey;
        // Right of the ANG/PWR readouts in the bottom-left corner.
        let x0 = 44 + ex;
        // Drag aiming: the open screen is the control, so only the modifier
        // buttons stay, dropped to the bottom row.
        let y2 = if self.g.drag_aiming() {
            y
        } else {
            self.draw_touch_button(x0, y, bw, bh, "<", Action::Left);
            self.draw_touch_button(x0 + bw + 3, y, bw, bh, ">", Action::Right);
            self.draw_touch_button(x0 + 2 * bw + 12, y, bw, bh, "-", Action::Down);
            self.draw_touch_button(x0 + 3 * bw + 15, y, bw, bh, "+", Action::Up);
            // Left of the screen index in the bottom-right corner.
            self.draw_touch_button(w - 16 - 40 - ex, y, 40, bh, "FIRE", Action::Fire);
            y - bh - 3
        };
        if loadout.airburst() {
            self.draw_touch_button(x0, y2, bw, bh, "F-", Action::FuseDown);
            self.draw_touch_button(x0 + bw + 3, y2, bw, bh, "F+", Action::FuseUp);
        }
        if loadout.enfilade_units > 0.0 {
            self.draw_touch_button(x0 + 2 * bw + 12, y2, bw, bh, "E<", Action::RepositionLeft);
            self.draw_touch_button(x0 + 3 * bw + 15, y2, bw, bh, "E>", Action::RepositionRight);
        }

        self.draw_touch_button((w - 20) / 2, 3 + ey, 20, 13, "II", Action::Back);
    }

    /// A frame of play, back to front: the world, the shot, the HUD, then
    /// the touch controls, the draft and the pause plate on top.
    fn render_play(&mut self) {
        let pal = self.pal;
        let view = self.g.view();
        let Some(screen) = self.g.shown_screen().cloned() else {
            return;
        };
        let Some(run) = self.g.play.run.as_ref() else {
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
        let world_pal = Biome::for_run(run.run_seed, index, self.g.play.challenge).palette(pal);

        self.cache.backdrop.draw(
            self.canvas,
            &view,
            &world_pal,
            sky::Phase::for_screen(index),
            Biome::for_run(run.run_seed, index, self.g.play.challenge),
            run.run_seed ^ index as u64,
            self.g.play.sky_time,
            &screen,
            &[screen.muzzle_x],
        );
        if sappers {
            raster::draw_material_boundaries(self.canvas, &view, &screen.field, &world_pal);
        }
        if !chalk.is_empty() {
            raster::draw_chalk(self.canvas, &view, &chalk, &screen.field, &world_pal);
        }
        if loadout.has_flag(flags::PLUMB_LINE) {
            if let Some(a) = apex {
                raster::draw_plumb(self.canvas, &view, a, pal);
            }
        }

        // Under the structures and emplacement, so it never hides either.
        if self.g.play.anim.is_none() && apex.is_some() && self.g.last_arc_on() {
            let under = |x, y| self.cache.backdrop.under(x, y);
            raster::draw_last_arc_on(self.canvas, &view, &self.g.play.last_arc, pal, under);
        }

        // A challenge's hint appears once a shell is spent, so a fresh course
        // stays clean.
        if self.g.play.anim.is_none()
            && !self.g.play.hint_arc.is_empty()
            && run.shots_on_screen > 0
            && !run.screen.cleared()
        {
            let under = |x, y| self.cache.backdrop.under(x, y);
            raster::draw_hint_arc_on(self.canvas, &view, &self.g.play.hint_arc, pal, under);
        }

        self.draw_structures(&screen, &view);
        self.draw_player(&screen, &loadout, &view);

        // The shot in flight.
        if let Some(anim) = self.g.play.anim.as_ref() {
            let upto = (anim.t / DT).floor().max(0.0) as usize;
            let traces: Vec<&Trace> = anim.traces.iter().collect();
            for t in traces {
                if t.path.is_empty() {
                    continue;
                }
                raster::draw_trajectory(self.canvas, &view, &t.path, upto, pal);
            }
        }
        self.draw_shot_effects(&view);
        if self.g.play.anim.is_none() && phase == Phase::Firing && self.g.mode == Mode::Playing {
            self.draw_drag_preview(&screen, &loadout, wind_shown, &view);
        }

        self.render_hud(
            shells, index, wind_shown, range, fuse, &loadout, &screen, phase,
        );
        if self.g.input.touch_active
            && self.g.mode == Mode::Playing
            && phase != Phase::Drafting
            && !self.g.spectating()
        {
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
        let Some((start, now)) = self.g.input.touch.drag() else {
            return;
        };
        let pal = self.pal;
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
        if self.g.input.drag_from.is_none()
            || crate::aim::pull_to_aim(start, now, loadout.angle_quantum_ticks).is_none()
        {
            return;
        }

        let hint = self.g.arc_hint();
        let wind = match hint {
            ArcHint::Wind => wind_shown.unwrap_or(0.0),
            ArcHint::Short | ArcHint::Full => 0.0,
        };
        let Some(run) = self.g.play.run.as_ref() else {
            return;
        };
        // A whole ballistic flight per frame while a finger is held still is
        // wasted: the path only changes when one of these does.
        let key = PreviewKey {
            angle: self.g.play.angle_ticks,
            power: self.g.play.power_ticks,
            hint,
            wind: wind.to_bits(),
            seed: run.run_seed,
            index: run.screen_index,
            shots: run.shots_on_screen,
            offset: run.emplacement_offset.to_bits(),
            epoch: self.g.play.preview_epoch,
        };
        let shot = Shot::new(self.g.play.angle_ticks, self.g.play.power_ticks);
        let (path, upto) = self
            .cache
            .preview
            .get_or(key, || preview_path(screen, loadout, shot, hint, wind));
        raster::draw_trajectory(self.canvas, view, path, *upto, pal);
    }

    /// The targets, as sprites seated on the ground: intact, leaning or
    /// rubble, each a hand-drawn frame where the art has one (DESIGN §5.4),
    /// never a rotation.
    fn draw_structures(&mut self, screen: &Screen, view: &View) {
        let toppling = self
            .g
            .play
            .anim
            .as_ref()
            .map(|a| a.done() && a.settled < TOPPLE_TIME && !a.collapsed.is_empty())
            .unwrap_or(false);
        let collapsed_now: Vec<usize> = self
            .g
            .play
            .anim
            .as_ref()
            .map(|a| a.collapsed.clone())
            .unwrap_or_default();
        let pal = self.pal;
        let biome = self.play_biome();

        for (i, st) in screen.structures.iter().enumerate() {
            if st.state == StructState::Cleared {
                continue;
            }
            // While a structure is toppling, keep it standing for a couple of
            // frames so the fall reads as a fall rather than a cut.
            let mut state = st.state;
            if toppling && collapsed_now.contains(&i) {
                state = StructState::Leaning;
            }
            let (sprite, faithful) = match self.sprites {
                Some(s) => {
                    let season = self
                        .g
                        .dev
                        .season
                        .or(self.g.save.seasonal)
                        .unwrap_or_default()
                        .resolve(self.g.today);
                    let (spr, faithful) = s.seasonal_structure(st.kind, state, season);
                    (spr, faithful)
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
                // The collapsed drawing did not load (every silhouette has
                // one; see `Sprites::structure`), so `sprite` is the intact
                // one: a rubble mound the width of its footprint reads as
                // wreckage where a standing silhouette would not.
                raster::draw_rubble(self.canvas, view, st.x0 as f64, footprint, ground, pal);
                continue;
            }
            match sprite {
                Some(sprite) => {
                    // Centre on the footprint, seat on the ground, shift with
                    // the lean. Whole pixels only, and never a scale factor.
                    let lean = st.lean as i32;
                    let x = view.sx(st.x0 as f64) + (footprint - sprite.w) / 2 + lean;
                    let y = view.sy(ground) - sprite.h + Sprites::seat_offset(st.kind);
                    self.canvas.blit(sprite, x, y);
                    if state != StructState::Collapsed {
                        if let Some((colour, depth)) = biome.cap() {
                            self.canvas.cap_sprite(sprite, x, y, colour, depth);
                        }
                    }
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

    /// The emplacement: chassis and barrel for the loadout, the barrel at the
    /// pre-drawn 5° frame nearest the aim (DESIGN §5.3).
    fn draw_player(&mut self, screen: &Screen, loadout: &Loadout, view: &View) {
        let muzzle = screen.muzzle();
        let shot = Shot::new(self.g.play.angle_ticks, self.g.play.power_ticks).quantized(loadout);
        let frame = shot.barrel_frame();

        let (tank, pivot, barrel) = match self.sprites {
            Some(s) => {
                let (t, p) = s.tank(loadout);
                (t, p, s.barrel(loadout))
            }
            None => (None, PIVOT_BASE, None),
        };

        let pivot_x = view.sx(muzzle.x);
        let pivot_y = view.sy(muzzle.y);

        // Barrel first, so the chassis overlaps its root.
        if let Some(barrel) = barrel {
            let sx = Sprites::barrel_frame_x(frame);
            self.canvas.blit_region(
                barrel,
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
            self.canvas.blit(tank, pivot_x - pivot.0, pivot_y - pivot.1);
        }
    }

    /// The muzzle flash, impact bursts and dust, and the border flash of a
    /// collapse, timed off the replay clock.
    fn draw_shot_effects(&mut self, view: &View) {
        let g = self.g;
        let Some(anim) = g.play.anim.as_ref() else {
            return;
        };
        let ink = raster::Ink::plain(self.pal.get(slot::ALERT));
        let biome = self.play_biome();
        self.draw_anim_effects(anim, anim.before.muzzle(), ink, biome, view);
    }

    /// The biome of the run on screen; the salt flats with none.
    fn play_biome(&self) -> Biome {
        self.g.play.run.as_ref().map_or(Biome::Salt, |run| {
            Biome::for_run(run.run_seed, run.screen_index, self.g.play.challenge)
        })
    }

    /// `draw_shot_effects` for any shot being played back, fired from
    /// `muzzle`: the run's, or a versus match's from either gun. The muzzle
    /// and impact flashes are drawn in `ink` (a run's alert colour, or the
    /// versus shooter's on a dark backing).
    fn draw_anim_effects(
        &mut self,
        anim: &Anim,
        muzzle: Vec2,
        ink: raster::Ink,
        biome: Biome,
        view: &View,
    ) {
        let alert = self.pal.get(slot::ALERT);
        let dust = biome.debris(&biome.palette(self.pal));
        if presentation::flash_strength(anim.t, 0.0, presentation::MUZZLE_TIME) > 0.0 {
            let p = muzzle;
            let x = view.sx(p.x);
            let y = view.sy(p.y);
            let mut flash: Vec<(i32, i32)> = (-3..=3).map(|d| (x + d, y)).collect();
            flash.extend((-3..=3).map(|d| (x, y + d)));
            flash.extend([(x + 2, y - 2), (x + 2, y + 2)]);
            self.canvas.put_inked(&flash, ink);
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
                let mut cross: Vec<(i32, i32)> = (-2..=2).map(|d| (x + d, y)).collect();
                cross.extend((-2..=2).map(|d| (x, y + d)));
                self.canvas.put_inked(&cross, ink);
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
            self.cache.backdrop.outline(self.canvas, alert);
        }
    }

    /// Wind top left, shells top right, screen index bottom right, the
    /// instruments bottom left, held modifiers, and the status line or the
    /// next onboarding prompt.
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
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let w = self.canvas.w;
        let (ex, ey) = self.g.display.corner_pad;

        // Wind, top left: a windsock flying the way the wind blows, then its
        // strength as a number. The sock never grows with strength, so the
        // number is still the read (DESIGN §2.5). It hangs limp when there is
        // no wind to show, or none shown yet (Dead Reckoning).
        let (wind_text, to_left) = match wind {
            None => ("--".to_string(), false),
            Some(v) if loadout.has_flag(flags::ANEMOMETER) => (format!("{:.1}", v.abs()), v < 0.0),
            Some(v) => (format!("{:.0}", v.abs()), v < 0.0),
        };
        self.draw_wind(&wind_text, to_left, false);

        // Shells, top right: a shell, then the count. Alert accent under 10,
        // the only other place that colour is allowed (DESIGN §5.2).
        let shell_text = format!("{shells}");
        let shell_w = crate::icons::width(&crate::icons::SHELL);
        let plate_w = shell_w + 13 + crate::font::big_width(&shell_text) as i32;
        let px = w - plate_w - 3 - ex;
        self.canvas.plate(px, 3 + ey, plate_w, 13, pal);
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
        let shot = Shot::new(self.g.play.angle_ticks, self.g.play.power_ticks).quantized(loadout);
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
                .play
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
        if self.g.play.clear_hold > 0.0 {
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
                format!("CAM {}", self.g.play.cam_x),
                format!(
                    "BUF {}X{}",
                    self.g.display.buffer_w, self.g.display.buffer_h
                ),
                format!("WND {:.3}", screen.wind),
            ];
            let mut dy = 42;
            for line in lines {
                self.canvas.text_small(3, dy, &line, dim);
                dy += 7;
            }
        }
    }

    /// The windsock plate, top left: a sock flying the way the wind blows
    /// (`to_left`), then `text`, its strength, in the alert colour (DESIGN
    /// §5.2). `neutral` draws it without the alert colour or the sky blue,
    /// for versus, where those two belong to the players (§11.4): plain
    /// digits and a brick-and-sand sock. The sock hangs limp when `text`
    /// has no non-zero digit.
    fn draw_wind(&mut self, text: &str, to_left: bool, neutral: bool) {
        let pal = self.pal;
        let (stripe, tail, digits) = if neutral {
            (slot::ST, slot::ROCK, slot::UI_FG)
        } else {
            (slot::ALERT, slot::SKY_LO, slot::ALERT)
        };
        let (fg, dim, alert) = (pal.get(slot::UI_FG), pal.get(tail), pal.get(stripe));
        let (ex, ey) = self.g.display.corner_pad;
        let calm = !text.bytes().any(|b| (b'1'..=b'9').contains(&b));
        let sock = if calm {
            &crate::icons::SOCK_LIMP
        } else {
            &crate::icons::SOCK
        };
        let sock_w = crate::icons::width(sock);
        self.canvas.plate(
            3 + ex,
            3 + ey,
            sock_w + 9 + crate::font::big_width(text) as i32,
            13,
            pal,
        );
        self.canvas
            .icon(6 + ex, 4 + ey, sock, &[fg, alert, dim], to_left && !calm);
        self.canvas
            .text_big(6 + ex + sock_w + 3, 4 + ey, text, pal.get(digits));
    }

    /// Dim everything drawn so far with a checkerboard of the plate colour:
    /// DESIGN §5.1 allows no gradient except through dithering.
    fn dither_dim(&mut self) {
        let bg = self.pal.get(slot::UI_BG);
        for y in 0..self.canvas.h {
            for x in 0..self.canvas.w {
                if (x + y) % 2 == 0 {
                    self.canvas.put(x, y, bg);
                }
            }
        }
    }

    /// The draft: the screen dimmed by a checkerboard (DESIGN §5.1: no
    /// gradients except through dithering), and one card per offer. On a touchscreen each card is its
    /// own button.
    fn render_draft(&mut self, offers: &[Modifier], loadout: &Loadout) {
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let alert = pal.get(slot::ALERT);
        let w = self.canvas.w;

        // Dim the screen behind the cards without a gradient.
        self.dither_dim();

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
            self.canvas.plate(x, top, card_w, card_h, pal);
            if self.g.input.touch_active && !self.g.spectating() {
                let action = [Action::Draft1, Action::Draft2, Action::Draft3][i.min(2)];
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

        // The demo picks for itself; its banner says what a tap does.
        if self.g.spectating() {
            return;
        }
        let hint = if self.g.input.touch_active {
            "TAP A CARD"
        } else {
            "1 2 3  CHOOSE"
        };
        let hx = (w - crate::font::small_width(hint) as i32) / 2;
        self.canvas.text_small(hx, top + card_h + 6, hint, fg);
    }

    /// The demo banner, bottom centre on a plate: clear of the instruments
    /// bottom left, the screen index or round bottom right, the draft cards
    /// and the versus plates. Plain text in the native face, for runs and
    /// duels alike: the alert accent stays reserved (§5.2) and the duel's
    /// side colours mean only a side (§11.4).
    fn render_demo_banner(&mut self) {
        let pal = self.pal;
        let ey = self.g.display.corner_pad.1;
        let text = if self.g.input.touch_active {
            DEMO_BANNER
        } else {
            DEMO_BANNER_KEYS
        };
        let tw = crate::font::small_width(text) as i32;
        let (pw, ph) = (tw + 10, 11);
        let x = (self.canvas.w - pw) / 2;
        let y = self.canvas.h - ph - 2 - ey;
        self.canvas.plate(x, y, pw, ph, pal);
        self.canvas
            .text_small(x + 5, y + 3, text, pal.get(slot::UI_FG));
    }

    /// The card naming the mode a demo leg is about to play (DESIGN §12.2),
    /// over the leg's first frame, top centre on a plate: between the
    /// corner plates and above the duel's handoff plate. Like the banner,
    /// plain text in the native face and no dimming (the demo's plates leave
    /// the board in view): the title in UI text, the hook in the secondary
    /// tone, `SKY_LO` in a run as the menus have it and `ROCK` in a duel,
    /// where `SKY_LO` is the right player's colour (§11.4).
    fn render_demo_card(&mut self) {
        let Some(card) = self.g.demo.as_ref().and_then(|d| d.card()) else {
            return;
        };
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(if self.g.mode == Mode::Versus {
            slot::ROCK
        } else {
            slot::SKY_LO
        });
        let (x, y, w, h) = demo_card_rect(self.g, &card, self.canvas.w);
        self.canvas.plate(x, y, w, h, pal);
        self.centred(y + 6, card.title, fg);
        self.centred(y + 6 + 5 + 5, card.hook, dim);
    }

    /// The pause plate over the frozen run.
    fn render_pause(&mut self) {
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let dim = pal.get(slot::SKY_LO);
        let (w, h) = (self.canvas.w, self.canvas.h);
        // A match's pause can also fork its round into the map editor
        // (DESIGN §13.11); a run's cannot.
        let fork = self.g.mode == Mode::Versus;
        // A match's TITLE asks twice while the match is still to be won
        // (`app/versus.rs`, "Quitting a match"): armed, its
        // row says what the second press does, in UI text.
        let armed = fork
            && self
                .g
                .versus
                .live
                .as_ref()
                .is_some_and(|vs| vs.quit_armed.is_some());
        let (title_text, title_c) = if armed {
            (QUIT_AGAIN_TAP, fg)
        } else {
            ("TITLE", dim)
        };
        if self.g.input.touch_active {
            let ph = if fork { 80 } else { 64 };
            let top = (h - ph) / 2;
            self.canvas.plate((w - 150) / 2, top, 150, ph, pal);
            self.centred(top + 8, "PAUSED", fg);
            let y = self.menu_row(top + 26, Action::Confirm, "", "RESUME", fg);
            let y = self.menu_row(y, Action::Replay, "", title_text, title_c);
            if fork {
                self.menu_row(
                    y,
                    Action::Edit(crate::input::Ed::ForkRound),
                    "",
                    "MAKE MAP OF THIS ROUND",
                    dim,
                );
            }
            return;
        }
        let ph = if fork { 58 } else { 48 };
        self.canvas.plate((w - 150) / 2, (h - ph) / 2, 150, ph, pal);
        self.centred((h - ph) / 2 + 8, "PAUSED", fg);
        let resume = self.g.input_label(Action::Confirm);
        self.centred((h - ph) / 2 + 21, &format!("{resume} / BACK  RESUME"), fg);
        let title = self.g.input_label(Action::Replay);
        let title_text = if armed { QUIT_AGAIN_KEY } else { "TITLE" };
        self.centred(
            (h - ph) / 2 + 31,
            &format!("{title}  {title_text}"),
            title_c,
        );
        if fork {
            let key = versus::map_key(self.g);
            self.centred(
                (h - ph) / 2 + 41,
                &format!("{key}  MAKE MAP OF THIS ROUND"),
                dim,
            );
        }
    }
}

mod editor;
mod versus;
mod watch;

/// The versus TITLE row once armed (the pause plate's, or a round's result
/// plate's), on a touchscreen and with keys (after the key's name): the
/// second press quits.
pub(super) const QUIT_AGAIN_TAP: &str = "TAP AGAIN TO QUIT";
pub(super) const QUIT_AGAIN_KEY: &str = "PRESS AGAIN TO QUIT";

/// Where the demo's mode card sits: `(x, y, w, h)` in a buffer `buffer_w`
/// wide: centred, as wide as its longest line plus 8 a side, two lines
/// tall (27 rows), its top at row 3 below the cutout pad: level with the
/// corner plates (the run's wind and shells, the duel's score), which it
/// stays between. It ends at row 29, or row 35 with a phone's 6-row pad,
/// above the curated runs' tallest targets (row 40) and the duel's handoff
/// plate, which starts below the middle.
pub(crate) fn demo_card_rect(
    g: &Game,
    card: &crate::app::DemoCard,
    buffer_w: i32,
) -> (i32, i32, i32, i32) {
    let tw = crate::font::small_width(card.title).max(crate::font::small_width(card.hook)) as i32;
    let (w, h) = (tw + 16, 6 + 5 + 5 + 5 + 6);
    let y = 3 + g.display.corner_pad.1;
    ((buffer_w - w) / 2, y, w, h)
}

/// Menu lines reading "KEY  TEXT", each label padded to the widest of them
/// and never narrower than ENTER, so the default rows keep their spacing and
/// a longer rebound name (DPAD_LEFT) widens them all alike.
fn keyed_rows(rows: &[(String, &str)]) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0)
        .max(5);
    rows.iter()
        .map(|(label, text)| format!("{label:<width$}  {text}"))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Effect;
    use crate::input::{Event, Intents, Key};
    use crate::save_format::Save;

    const WINDOW: (i32, i32) = (1280, 720);

    fn idle() -> Intents {
        Intents {
            window: WINDOW,
            today: 20_000,
            ..Intents::default()
        }
    }

    /// A game at the title, sized for a 16:9 window.
    fn game() -> Game {
        let mut g = Game::new(false, Save::default());
        g.fit_display(WINDOW, (0, 0));
        g
    }

    /// Draw `g` as the node would, into a canvas the size the core asked for.
    fn draw(g: &Game) -> (Canvas, Vec<Button>) {
        let mut canvas = Canvas::new(g.display.buffer_w, g.display.buffer_h);
        let mut buttons = Vec::new();
        frame(
            g,
            &Art::default(),
            &mut canvas,
            &mut buttons,
            &mut Cache::default(),
        );
        (canvas, buttons)
    }

    /// The iPhone SE (2nd gen), the low-end iOS profiling floor: 1334 by 750
    /// held landscape, a home button and no notch, so the safe area leaves no
    /// cutout. Its 16:9 makes the narrowest buffer, and every screen a phone
    /// reaches must fit in it with nothing assumed of a notch or an inset.
    #[test]
    fn an_iphone_se_lays_out_at_320_with_no_insets_and_every_button_on_screen() {
        let mut g = Game::new(true, Save::default());
        g.fit_display((1334, 750), (0, 0));
        assert_eq!((g.display.buffer_w, g.display.buffer_h), (320, 180));
        assert_eq!(g.display.letterbox, (0, 0));
        let on_screen = |g: &Game, screen: &str| {
            let (_, buttons) = draw(g);
            assert!(!buttons.is_empty(), "{screen}: no touch buttons drawn");
            for b in &buttons {
                let r = b.rect;
                assert!(
                    r.x >= 0 && r.y >= 0 && r.x + r.w <= 320 && r.y + r.h <= 180,
                    "{screen}: {:?} at {r:?} leaves the 320x180 buffer",
                    b.action
                );
            }
        };
        on_screen(&g, "title");
        g.start_run(42, false);
        g.take_effects();
        on_screen(&g, "run");
        let mut g = Game::new(true, Save::default());
        g.fit_display((1334, 750), (0, 0));
        g.start_versus();
        g.take_effects();
        on_screen(&g, "versus");
    }

    #[test]
    fn trial_composites_the_loaded_coastal_background_on_both_canvas_scales() {
        let mut g = game();
        g.start_trial(1);
        let mut px = [91, 82, 73, 255].repeat(1281);
        px[1279 * 4..1280 * 4].copy_from_slice(&[99, 99, 99, 255]);
        px[1280 * 4..1281 * 4].copy_from_slice(&[11, 22, 33, 255]);
        let first = [91, 82, 73, 255];
        let last = [11, 22, 33, 255];
        let art = Art {
            trial_background: Some(Sprite {
                w: 1281,
                h: 1,
                px,
                detail: None,
            }),
            ..Art::default()
        };

        for mut canvas in [Canvas::new(320, 180), Canvas::detailed(320, 180)] {
            frame(
                &g,
                &art,
                &mut canvas,
                &mut Vec::new(),
                &mut Cache::default(),
            );
            assert_eq!(&canvas.bytes()[..4], &first);
            assert_eq!(&canvas.bytes()[canvas.bytes().len() - 4..], &last);
        }
    }

    fn actions(buttons: &[Button]) -> Vec<Action> {
        buttons.iter().map(|b| b.action).collect()
    }

    #[test]
    fn wide_detailed_title_margins_match_the_source_edge_colors() {
        let mut g = game();
        g.fit_display((1600, 720), (0, 0));
        let color = [9, 31, 67, 255];
        let art = Art {
            title_background: Some(Sprite {
                w: 320,
                h: 180,
                px: [201, 202, 203, 255].repeat(320 * 180),
                detail: Some(Box::new(Sprite {
                    w: 1280,
                    h: 720,
                    px: color.repeat(1280 * 720),
                    detail: None,
                })),
            }),
            ..Art::default()
        };
        let mut canvas = Canvas::detailed(400, 180);
        frame(
            &g,
            &art,
            &mut canvas,
            &mut Vec::new(),
            &mut Cache::default(),
        );
        for x in [0, 159, 160, 1439, 1440, 1599] {
            let at = ((700 * 1600 + x) * 4) as usize;
            assert_eq!(&canvas.bytes()[at..at + 4], &color);
        }
    }

    #[test]
    fn the_title_screen_is_pinned() {
        let (canvas, buttons) = draw(&game());
        assert!(buttons.is_empty(), "no touch buttons on a desktop");
        insta::assert_snapshot!(canvas.palette_text(&Palette::default()));
    }

    #[test]
    fn a_fresh_run_is_pinned() {
        // Started the way a player starts one, through the core.
        let mut g = game();
        g.tick(
            1.0 / 60.0,
            &Intents {
                events: vec![Event::key(Key::Enter)],
                ..idle()
            },
        );
        assert_eq!(g.mode, Mode::Playing);
        let (canvas, _) = draw(&g);
        insta::assert_snapshot!(canvas.palette_text(&Palette::default()));
    }

    /// The demo's first frame of each leg, banner and all: a run on its
    /// first curated screen, and a duel's opening handoff plate with no
    /// READY prompt, each under the card naming its mode (ROGUELIKE, TWO
    /// PLAYERS). `the_demo_card_is_a_plate_over_the_leg` checks the cards
    /// change nothing else in these frames.
    #[test]
    fn the_demo_frames_are_pinned() {
        let mut g = game();
        g.start_demo(crate::app::DemoStarted::Menu);
        assert_eq!(g.mode, Mode::Playing);
        let (run, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("demo_run", run.palette_text(&Palette::default()));
        for _ in 0..10_000 {
            if g.mode == Mode::Versus {
                break;
            }
            g.tick(1.0 / 30.0, &idle());
        }
        assert_eq!(g.mode, Mode::Versus);
        let (duel, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("demo_duel", duel.palette_text(&Palette::default()));
    }

    /// The mode card over each leg's first frame: it paints only its own
    /// rectangle, the banner and the rest of the frame are as they are
    /// without it, and it keeps to §5.2: plate, UI text and the secondary
    /// tone (`SKY_LO` in a run, `ROCK` in a duel), never the alert accent
    /// or a side's colour. It clears the corner plates, the banner, the
    /// duel's handoff plate and every curated run's targets (their actual
    /// top rows, measured in the frame without the card), at the narrowest
    /// buffer, with and without a phone's cutout pad.
    #[test]
    fn the_demo_card_is_a_plate_over_the_leg() {
        use crate::app::DemoLeg;
        let pal = Palette::default();
        for pad in [(0, 0), (10, 6)] {
            let mut g = game();
            g.display.corner_pad = pad;
            g.start_demo(crate::app::DemoStarted::Menu);
            // The four curated runs and the duels between them.
            for leg in 0..8u64 {
                for _ in 0..10_000 {
                    if g.demo.as_ref().unwrap().leg_index == leg {
                        break;
                    }
                    g.tick(1.0 / 30.0, &idle());
                }
                let d = g.demo.as_ref().unwrap();
                assert_eq!(d.leg_index, leg);
                let run = d.leg() == DemoLeg::Run;
                let card = d.card().expect("a card");
                let (with, _) = draw(&g);
                let left = std::mem::replace(&mut g.demo.as_mut().unwrap().card, 0.0);
                let (without, _) = draw(&g);
                g.demo.as_mut().unwrap().card = left;
                let (x, y, w, h) = demo_card_rect(&g, &card, with.w);
                let what = format!("leg {leg}, pad {pad:?}");
                assert!(x > 30 && x + w < with.w - 30, "{what}: between the corners");
                assert_eq!(y, 3 + pad.1, "{what}: level with the corner plates");
                let (a, b) = (with.palette_text(&pal), without.palette_text(&pal));
                if run {
                    // The highest structure pixel under the card's columns.
                    let top = b
                        .lines()
                        .position(|row| {
                            row.chars().enumerate().any(|(px, c)| {
                                (x..x + w).contains(&(px as i32)) && "abc".contains(c)
                            })
                        })
                        .expect("a target under the card") as i32;
                    assert!(
                        y + h <= top,
                        "{what}: card ends at {} over a target at {top}",
                        y + h - 1
                    );
                } else {
                    assert!(y + h < 60, "{what}: above the handoff plate");
                }
                let secondary = if run { '6' } else { '1' };
                let mut inside = std::collections::BTreeSet::new();
                for (py, (ra, rb)) in a.lines().zip(b.lines()).enumerate() {
                    for (px, (ca, cb)) in ra.chars().zip(rb.chars()).enumerate() {
                        let (px, py) = (px as i32, py as i32);
                        if (x..x + w).contains(&px) && (y..y + h).contains(&py) {
                            inside.insert(ca);
                        } else {
                            assert_eq!(ca, cb, "{what}: ({px}, {py}) outside the card");
                        }
                    }
                }
                let allowed = ['d', 'e', '3', secondary];
                assert!(
                    inside.iter().all(|c| allowed.contains(c)),
                    "{what}: {inside:?}"
                );
                assert!(inside.contains(&secondary), "{what}: the hook");
                assert!(inside.contains(&'d'), "{what}: the title");
            }
        }
    }

    #[test]
    fn the_demo_banner_fits_and_is_drawn_on_touchscreens_too() {
        let mut g = game();
        g.input.touch_active = true;
        g.display.corner_pad = (10, 6);
        g.start_demo(crate::app::DemoStarted::Idle);
        let (with, buttons) = draw(&g);
        assert!(buttons.is_empty(), "no FIRE or pause in the demo");
        for text in [DEMO_BANNER, DEMO_BANNER_KEYS] {
            let tw = crate::font::small_width(text) as i32;
            assert!(tw + 10 < 320 - 2 * 90, "{text}: clear of the corners");
        }
        g.demo = None;
        let (without, _) = draw(&g);
        assert_ne!(with.bytes(), without.bytes());
    }

    #[test]
    fn every_pixel_of_every_screen_is_painted() {
        let mut g = game();
        let mut screens = vec![draw(&g).0];
        g.start_run(4242, false);
        screens.push(draw(&g).0);
        g.mode = Mode::Paused;
        screens.push(draw(&g).0);
        g.finish_run();
        screens.push(draw(&g).0);
        g.open_seed_entry();
        screens.push(draw(&g).0);
        g.mode = Mode::Controls;
        screens.push(draw(&g).0);
        for (i, c) in screens.iter().enumerate() {
            assert!(
                !c.palette_text(&Palette::default()).contains('.'),
                "screen {i} left pixels undrawn"
            );
        }
    }

    #[test]
    fn result_and_controls_prompts_follow_the_bindings() {
        // Every action each screen names, moved to another key and button.
        for (over, rebind) in [
            (
                true,
                &[
                    ("replay", "T", "LB"),
                    ("copy", "K", "RB"),
                    ("confirm", "SPACE", "B"),
                ][..],
            ),
            (false, &[("replay", "T", "LB"), ("mute", "U", "BACK")][..]),
        ] {
            let mut g = game();
            g.start_run(4242, false);
            if over {
                g.finish_run();
            } else {
                g.mode = Mode::Controls;
            }
            g.take_effects();
            let default = draw(&g).0;
            for &(action, key, pad) in rebind {
                g.save
                    .bindings
                    .push((action.into(), key.into(), pad.into()));
                let rebound = draw(&g).0;
                assert_ne!(
                    default.bytes(),
                    rebound.bytes(),
                    "over {over}: rebinding {action} changed nothing on screen"
                );
                g.save.bindings.clear();
            }
            if over {
                // A controller is prompted with its own buttons.
                g.input.controller_active = true;
                assert_ne!(default.bytes(), draw(&g).0.bytes(), "controller labels");
            }
        }
    }

    #[test]
    fn keyed_rows_pad_every_label_to_the_widest() {
        let rows =
            |labels: [&str; 2]| keyed_rows(&[(labels[0].into(), "ONE"), (labels[1].into(), "TWO")]);
        // The defaults, as they always read.
        assert_eq!(rows(["R", "ENTER"]), ["R      ONE", "ENTER  TWO"]);
        // A longer name widens the column for every row.
        assert_eq!(
            rows(["DPAD_LEFT", "A"]),
            ["DPAD_LEFT  ONE", "A          TWO"]
        );
    }

    #[test]
    fn touch_menus_lay_out_their_buttons() {
        let mut g = game();
        g.input.touch_active = true;
        let (_, buttons) = draw(&g);
        assert_eq!(
            actions(&buttons),
            [
                Action::TitleMusic,
                Action::Demo,
                Action::Watch,
                Action::Challenges,
                Action::Confirm,
                Action::Seed,
                Action::Versus,
                Action::Controls,
                Action::Back
            ]
        );

        g.crash.last_crash = Some("report".into());
        g.run_file.resumable = Some(crate::resume::Saved::of(
            &salt_line_sim::run::RunState::new(1, 320),
            false,
        ));
        let (_, buttons) = draw(&g);
        assert_eq!(
            actions(&buttons),
            [
                Action::TitleMusic,
                Action::Demo,
                Action::Watch,
                Action::Challenges,
                Action::Confirm,
                Action::NewRun,
                Action::Seed,
                Action::Versus,
                Action::Controls,
                Action::Back,
                Action::Copy
            ]
        );
    }

    #[test]
    fn credits_navigation_fits_and_supports_touch_and_controller() {
        let mut g = game();
        g.mode = Mode::Credits;
        for touch in [false, true] {
            g.input.touch_active = touch;
            g.input.controller_active = !touch;
            for page in 0..crate::credits::pages().len() {
                g.credits_page = page;
                let (_, buttons) = draw(&g);
                if touch {
                    for action in [Action::Left, Action::Right, Action::Back] {
                        assert!(buttons.iter().any(|b| b.action == action));
                    }
                    for button in buttons {
                        assert!(button.rect.y >= 0 && button.rect.y + button.rect.h <= 180);
                    }
                } else {
                    assert!(buttons.is_empty());
                }
            }
        }
    }

    #[test]
    fn seasonal_touch_settings_keep_all_buttons_on_screen() {
        for width in [320, 400] {
            let mut g = game();
            g.mode = Mode::Controls;
            g.input.touch_active = true;
            g.display.buffer_w = width;
            g.save.touch_aim = Some(TouchAim::Drag);
            g.save.seasonal = Some(crate::seasonal::Seasonal::Thanksgiving);
            let (canvas, buttons) = draw(&g);
            assert!(actions(&buttons).contains(&Action::Seasonal));
            assert!(actions(&buttons).contains(&Action::Back));
            for button in &buttons {
                assert!(
                    button.rect.x >= 0 && button.rect.y >= 0,
                    "{:?}",
                    button.action
                );
                assert!(
                    button.rect.x + button.rect.w <= canvas.w,
                    "{:?}",
                    button.action
                );
                assert!(
                    button.rect.y + button.rect.h <= canvas.h,
                    "{:?}",
                    button.action
                );
            }
            for (i, a) in buttons.iter().enumerate() {
                for b in &buttons[i + 1..] {
                    assert!(
                        a.rect.x + a.rect.w <= b.rect.x
                            || b.rect.x + b.rect.w <= a.rect.x
                            || a.rect.y + a.rect.h <= b.rect.y
                            || b.rect.y + b.rect.h <= a.rect.y,
                        "overlapping {:?} and {:?}",
                        a.action,
                        b.action
                    );
                }
            }
        }
    }

    /// A suspended versus match (DESIGN §11.6).
    fn saved_match() -> crate::match_log::SavedMatch {
        let duel = crate::versus::Match::new(7, 320);
        crate::match_log::SavedMatch::of(&duel, [false; 2], &[])
    }

    /// On a touchscreen a suspended match takes the versus row's place: the
    /// same buttons in the same spots, so the phone title grows no row.
    /// With keys it adds its own row, which the keyed title otherwise does
    /// not have.
    #[test]
    fn a_suspended_match_takes_the_versus_row() {
        let mut g = game();
        g.input.touch_active = true;
        let (plain, before) = draw(&g);
        g.saved_match = Some(saved_match());
        let (offered, after) = draw(&g);
        assert_ne!(plain.bytes(), offered.bytes(), "the row says RESUME MATCH");
        let rects = |b: &[Button]| b.iter().map(|b| (b.action, b.rect)).collect::<Vec<_>>();
        assert_eq!(rects(&after), rects(&before));

        let mut g = game();
        let (keyed, buttons) = draw(&g);
        g.saved_match = Some(saved_match());
        let (keyed_offer, more) = draw(&g);
        assert!(buttons.is_empty() && more.is_empty());
        assert_ne!(keyed.bytes(), keyed_offer.bytes(), "2  RESUME MATCH");
    }

    /// Everything the phone title can offer at once: a saved run, a saved
    /// match, a crash report and a launch note. Every row is still a whole
    /// button on the screen.
    #[test]
    fn the_phone_title_fits_a_run_a_match_and_a_crash_report() {
        let mut g = game();
        g.input.touch_active = true;
        g.display.corner_pad = (10, 6);
        g.crash.last_crash = Some("report".into());
        g.run_file.resumable = Some(crate::resume::Saved::of(
            &salt_line_sim::run::RunState::new(1, 320),
            false,
        ));
        g.saved_match = Some(saved_match());
        g.status = "SAVED RUN COULD NOT BE RESTORED".into();
        g.status_time = 6.0;
        let (_, buttons) = draw(&g);
        assert_eq!(
            actions(&buttons),
            [
                Action::TitleMusic,
                Action::Demo,
                Action::Watch,
                Action::Challenges,
                Action::Confirm,
                Action::NewRun,
                Action::Seed,
                Action::Versus,
                Action::Controls,
                Action::Back,
                Action::Copy
            ]
        );
        for b in &buttons {
            assert!(
                b.rect.y >= 0 && b.rect.y + b.rect.h <= g.display.buffer_h,
                "{:?} off the screen",
                b.action
            );
        }
        // The REPLAYS corner plate (DESIGN §9.4) shares the corner with
        // DEMO, and no button overlaps another.
        for (i, a) in buttons.iter().enumerate() {
            for b in &buttons[i + 1..] {
                let apart = a.rect.x + a.rect.w <= b.rect.x
                    || b.rect.x + b.rect.w <= a.rect.x
                    || a.rect.y + a.rect.h <= b.rect.y
                    || b.rect.y + b.rect.h <= a.rect.y;
                assert!(apart, "{:?} overlaps {:?}", a.action, b.action);
            }
        }
    }

    #[test]
    fn touch_play_lays_out_aim_fire_and_pause() {
        let mut g = game();
        g.start_run(1, false);
        g.input.touch_active = true;
        let (_, buttons) = draw(&g);
        assert_eq!(
            actions(&buttons),
            [
                Action::Left,
                Action::Right,
                Action::Down,
                Action::Up,
                Action::Fire,
                Action::Back
            ]
        );
        // Drag aiming keeps only the pause button: the open ground aims.
        g.save.touch_aim = Some(TouchAim::Drag);
        let (_, buttons) = draw(&g);
        assert_eq!(actions(&buttons), [Action::Back]);
    }

    #[test]
    fn the_controller_on_the_seed_keypad_gets_plates_and_a_focus() {
        let mut g = game();
        g.open_seed_entry();
        let (keyed, buttons) = draw(&g);
        assert!(buttons.is_empty());
        g.input.controller_active = true;
        let (plated, buttons) = draw(&g);
        assert_eq!(buttons.len(), keypad::KEYS.len() + 3);
        assert!(buttons.iter().any(|b| b.action == keypad::DEL));
        assert_ne!(keyed.bytes(), plated.bytes());
    }

    #[test]
    fn the_drag_band_and_arc_show_only_while_dragging() {
        let mut g = game();
        g.start_run(1, false);
        g.input.touch_active = true;
        g.save.touch_aim = Some(TouchAim::Drag);
        g.take_effects();
        let (still, _) = draw(&g);
        g.tick(
            1.0 / 60.0,
            &Intents {
                events: vec![
                    Event::FingerDown {
                        index: 0,
                        x: 200.0,
                        y: 100.0,
                    },
                    Event::FingerMoved {
                        index: 0,
                        x: 150.0,
                        y: 140.0,
                    },
                ],
                ..idle()
            },
        );
        let (dragging, _) = draw(&g);
        let changed = still
            .bytes()
            .chunks(4)
            .zip(dragging.bytes().chunks(4))
            .filter(|(a, b)| a != b)
            .count();
        // The band, the arc, and the readouts that follow the aim.
        assert!(changed > 30, "{changed} pixels changed");
        let band = Palette::default().get(slot::SKY_LO);
        for (x, y) in [(199, 99), (200, 100), (201, 101)] {
            let i = ((y * dragging.w + x) * 4) as usize;
            assert_eq!(dragging.bytes()[i..i + 4], band, "({x}, {y})");
        }
    }

    #[test]
    fn the_error_screen_offers_the_report() {
        let mut g = game();
        g.input.touch_active = true;
        g.broken_tick(0.0, &idle(), || None);
        let mut canvas = Canvas::new(g.display.buffer_w, g.display.buffer_h);
        let mut buttons = vec![Button {
            rect: Rect::new(0, 0, 1, 1),
            action: Action::Delete,
        }];
        broken(
            &g,
            &Art::default(),
            &mut canvas,
            &mut buttons,
            &mut Cache::default(),
        );
        assert_eq!(actions(&buttons), [Action::Copy, Action::Back]);
    }

    #[test]
    fn every_versus_screen_is_painted_and_lays_out_its_buttons() {
        use crate::app::VersusStage;
        let mut g = game();
        g.input.touch_active = true;
        g.open_versus_setup();
        let mut screens = Vec::new();
        let (c, buttons) = draw(&g);
        screens.push(c);
        assert_eq!(
            actions(&buttons),
            [
                Action::Replay,
                Action::Left,
                Action::Right,
                Action::Up,
                Action::Edit(crate::input::Ed::GroundNext),
                Action::Edit(crate::input::Ed::GroundPrev),
                Action::Edit(crate::input::Ed::GroundNext),
                Action::Confirm,
                Action::Back,
                Action::Edit(crate::input::Ed::MakeMap)
            ]
        );

        g.versus.help = [true, true];
        g.start_versus();
        let (c, buttons) = draw(&g);
        screens.push(c);
        assert_eq!(actions(&buttons), [Action::Confirm, Action::Back]);

        let vs = g.versus.live.as_mut().unwrap();
        vs.stage = VersusStage::Aiming;
        let (aiming, buttons) = draw(&g);
        assert_eq!(
            actions(&buttons),
            [
                Action::Left,
                Action::Right,
                Action::Down,
                Action::Up,
                Action::Fire,
                Action::Back
            ]
        );
        // Aim help draws the short preview arc over the field.
        g.versus.help = [false, false];
        let (bare, _) = draw(&g);
        assert_ne!(aiming.bytes(), bare.bytes(), "aim help drew nothing");
        screens.push(aiming);

        let vs = g.versus.live.as_mut().unwrap();
        vs.paused = true;
        screens.push(draw(&g).0);
        let vs = g.versus.live.as_mut().unwrap();
        vs.paused = false;
        vs.duel.phase = crate::versus::Phase::RoundOver(crate::versus::Side::Right);
        vs.stage = VersusStage::Result;
        let (c, buttons) = draw(&g);
        screens.push(c);
        assert_eq!(
            actions(&buttons),
            [
                Action::Confirm,
                Action::Replay,
                Action::Edit(crate::input::Ed::ForkRound),
                Action::Back
            ]
        );
        // The pause button sits above the plate, clear of its rows, at the
        // phone's 180 rows, with a cutout's pad and without, for every
        // result plate (a round's against the AI has no TITLE row).
        for pad in [0, 6] {
            for (ai, over) in [(false, false), (false, true), (true, false), (true, true)] {
                g.display.corner_pad.1 = pad;
                let vs = g.versus.live.as_mut().unwrap();
                vs.ai = ai.then_some(crate::opponent::Level::Easy);
                vs.duel.phase = if over {
                    crate::versus::Phase::MatchOver(crate::versus::Side::Right)
                } else {
                    crate::versus::Phase::RoundOver(crate::versus::Side::Right)
                };
                let (_, buttons) = draw(&g);
                assert_eq!(g.display.buffer_h, 180);
                let pause = buttons.iter().find(|b| b.action == Action::Back).unwrap();
                let vs = g.versus.live.as_ref().unwrap();
                let (_, plate_top, _, _) =
                    versus::result_plate_rect(&g, vs, true, g.display.buffer_w, 180).unwrap();
                assert!(
                    pause.rect.y + pause.rect.h < plate_top,
                    "pad {pad} ai {ai} over {over}: II at {:?}, plate from {plate_top}",
                    pause.rect
                );
                // And every row's target is on the plate.
                for b in buttons.iter().filter(|b| b.action != Action::Back) {
                    assert!(b.rect.y >= plate_top, "{:?} above the plate", b.action);
                }
            }
        }
        g.display.corner_pad.1 = 0;
        let vs = g.versus.live.as_mut().unwrap();
        vs.ai = None;
        vs.duel.phase = crate::versus::Phase::RoundOver(crate::versus::Side::Right);
        // Armed, a round's TITLE between two players reads TAP AGAIN TO
        // QUIT, and its row still fits the plate at 180 rows.
        let (unarmed, _) = draw(&g);
        g.versus.live.as_mut().unwrap().quit_armed = Some(0.0);
        let (armed, buttons) = draw(&g);
        assert_ne!(armed.bytes(), unarmed.bytes(), "the armed row drew nothing");
        let vs = g.versus.live.as_ref().unwrap();
        let (px, top, pw, ph) =
            versus::result_plate_rect(&g, vs, true, g.display.buffer_w, 180).unwrap();
        let title = buttons.iter().find(|b| b.action == Action::Replay).unwrap();
        assert!(
            title.rect.x > px
                && title.rect.x + title.rect.w < px + pw
                && title.rect.y >= top
                && title.rect.y + title.rect.h <= top + ph,
            "armed TITLE {:?} off the plate {:?}",
            title.rect,
            (px, top, pw, ph)
        );
        // With keys or a controller, its line ("R  PRESS AGAIN TO QUIT",
        // padded like its neighbours) fits the plate too.
        g.input.touch_active = false;
        for pad in [false, true] {
            g.input.controller_active = pad;
            let labels = [
                (g.input_label(Action::Confirm), "NEXT ROUND"),
                (g.input_label(Action::Replay), QUIT_AGAIN_KEY),
                (versus::map_key(&g), "MAKE MAP OF THIS ROUND"),
            ];
            for line in keyed_rows(&labels) {
                let lw = crate::font::small_width(&line) as i32;
                assert!(lw <= pw - 8, "pad {pad}: {line:?} is {lw} wide");
            }
        }
        g.input.controller_active = false;
        g.input.touch_active = true;
        g.versus.live.as_mut().unwrap().quit_armed = None;

        for (i, c) in screens.iter().enumerate() {
            assert!(
                !c.palette_text(&Palette::default()).contains('.'),
                "versus screen {i} left pixels undrawn"
            );
        }
    }

    #[test]
    fn ai_turns_offer_pause_without_human_aim_controls() {
        use crate::app::VersusStage;
        let mut g = game();
        g.input.touch_active = true;
        g.versus.ai = true;
        g.start_versus();
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.to_move = crate::versus::Side::Right;
        for stage in [VersusStage::Handoff, VersusStage::Aiming] {
            g.versus.live.as_mut().unwrap().stage = stage;
            let (_, buttons) = draw(&g);
            assert_eq!(actions(&buttons), [Action::Back]);
        }
    }

    /// Against the AI nothing says to pass the phone, and the sides are
    /// YOU and AI, never LEFT or RIGHT: the pre-match screen's heading and
    /// aim rows at every level and aim-help choice, and every plate and
    /// callout of a whole match (hits, misses, a last shot to tie and the
    /// result). The one exception is the RIGHT OPPONENT row itself, which
    /// is the switch between the two kinds of match and says which gun the
    /// AI takes.
    #[test]
    fn an_ai_match_says_you_and_ai_and_never_pass_the_phone() {
        use crate::opponent::Level;
        use crate::versus::{Phase, Side};
        use versus::{aim_rows, handoff_prompt, hit_line, say, setup_heading, Says};
        let mut said = vec![setup_heading(true).to_string()];
        for level in Level::ALL {
            for help in [[false, false], [true, false], [false, true], [true, true]] {
                said.extend(aim_rows(true, level, help));
            }
        }
        assert!(said.contains(&"YOUR AIM HELP: ON".to_string()));
        assert!(said.contains(&"AI LEVEL: MEDIUM".to_string()));
        // A human match keeps its sides' names.
        assert_eq!(
            aim_rows(false, Level::Hard, [true, false]),
            ["LEFT AIM HELP: ON", "RIGHT AIM HELP: OFF"]
        );
        let mut g = game();
        g.versus.ai = true;
        g.start_versus();
        let vs = g.versus.live.as_mut().unwrap();
        // Hits from both sides to 3-2, a miss, then hits to the end.
        let mut turn = 0;
        while !matches!(vs.duel.phase, Phase::MatchOver(_)) && turn < 60 {
            if let Phase::RoundOver(_) = vs.duel.phase {
                let winner = match vs.duel.phase {
                    Phase::RoundOver(w) => w,
                    _ => unreachable!(),
                };
                said.push(say(true, winner, Says::Wins));
                vs.next_round();
            }
            said.extend(handoff_prompt(vs, false).map(str::to_string));
            for side in Side::BOTH {
                for what in [Says::Name, Says::Scores, Says::ToAnswer, Says::Wins] {
                    said.push(say(true, side, what));
                }
            }
            let duel = &vs.duel;
            let side = duel.to_move;
            let frame = crate::versus::frame_for(&duel.screen, side);
            let target = crate::versus::target_in_frame(&duel.screen, side);
            let shot =
                crate::versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
                    .first()
                    .copied()
                    .filter(|_| turn % 6 != 5)
                    .unwrap_or(Shot::new(1800, 0));
            let report = vs.fire_turn((shot.angle_ticks, shot.power_ticks)).unwrap();
            if report.hit {
                said.push(hit_line(vs, report.shooter));
            }
            turn += 1;
        }
        assert!(said.iter().any(|s| s == "YOU WIN" || s == "AI WINS"));
        for line in &said {
            for word in ["PHONE", "PEEK", "LEFT", "RIGHT"] {
                assert!(!line.contains(word), "{line:?}");
            }
        }
    }

    /// The AI gets no aim help, whatever the right player's row says: its
    /// preview would draw the shot it solved.
    #[test]
    fn the_ai_draws_no_aim_help() {
        use crate::app::VersusStage;
        let mut g = game();
        g.versus.ai = true;
        g.start_versus();
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.to_move = crate::versus::Side::Right;
        vs.stage = VersusStage::Aiming;
        g.versus.help = [false, false];
        let plain = draw(&g).0;
        g.versus.help = [false, true];
        assert_eq!(draw(&g).0.bytes(), plain.bytes());
        // A human's right gun still gets it.
        g.versus.live.as_mut().unwrap().ai = None;
        assert_ne!(draw(&g).0.bytes(), plain.bytes());
    }

    /// The pre-match screen with RIGHT OPPONENT: AI on a phone: every row
    /// and the footer fit in the 180 rows, the footer straight under BACK
    /// and clear of every button.
    #[test]
    fn setup_fits_a_phone() {
        let mut g = Game::new(true, Save::default());
        g.fit_display(crate::cli::PHONE_WINDOW, (0, 0));
        g.input.touch_active = true;
        g.versus.ai = true;
        g.open_versus_setup();
        let (c, buttons) = draw(&g);
        let footer = c.h - 8;
        for b in &buttons {
            assert!(
                b.rect.y + b.rect.h < footer,
                "{:?} runs into the footer",
                b.action
            );
        }
        // The footer's line is where a note shows instead: whatever a note
        // changes is in the footer's five rows, on screen.
        g.status = "THIS MAP DOES NOT PASS - PICK ANOTHER".into();
        g.status_time = 1.0;
        let (noted, _) = draw(&g);
        let (a, b) = (
            c.palette_text(&Palette::default()),
            noted.palette_text(&Palette::default()),
        );
        let changed: Vec<i32> = (0..)
            .zip(a.lines().zip(b.lines()))
            .filter(|(_, (x, y))| x != y)
            .map(|(row, _)| row)
            .collect();
        assert!(!changed.is_empty());
        assert!(
            changed.iter().all(|r| (footer..footer + 5).contains(r)),
            "{changed:?}"
        );
    }

    #[test]
    fn overtime_and_rolls_are_called_on_the_handoff_plate() {
        use crate::app::VersusStage;
        use crate::versus::{RollWay, Side, HITS_TO_WIN_ROUND as N};
        let mut g = game();
        g.input.touch_active = true;
        g.start_versus();
        let plain = draw(&g).0;
        let vs = g.versus.live.as_mut().unwrap();
        assert_eq!(vs.stage, VersusStage::Handoff);
        assert!(versus::handoff_cues(vs).is_empty(), "an ordinary turn");

        // Left opened and reached the mark; right to answer.
        vs.duel.hits = [N, N - 1];
        vs.duel.to_move = Side::Right;
        vs.duel.turn = 2 * N - 1;
        let cues = versus::handoff_cues(vs);
        assert_eq!(
            cues,
            [("LAST SHOT - HIT TO TIE", versus::side_slot(Side::Right))]
        );
        assert_eq!(versus::hit_line(vs, Side::Left), "RIGHT GUN TO ANSWER");
        let last_shot = draw(&g).0;
        assert_ne!(last_shot.bytes(), plain.bytes());

        // Right tied it: left to move, level past the mark.
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.hits = [N, N];
        vs.duel.to_move = Side::Left;
        vs.rolled = Some(RollWay::Apart);
        let texts: Vec<&str> = versus::handoff_cues(vs).iter().map(|c| c.0).collect();
        assert_eq!(texts, ["TIED - KEEP GOING", "GUNS ROLLED APART"]);
        assert_eq!(versus::hit_line(vs, Side::Right), "TIED - KEEP GOING");
        vs.rolled = Some(RollWay::Closer);
        let texts: Vec<&str> = versus::handoff_cues(vs).iter().map(|c| c.0).collect();
        assert_eq!(texts, ["TIED - KEEP GOING", "GUNS ROLLED CLOSER"]);
        // Four hits each: every hit has a pip, and the plates still paint
        // every pixel and fit the buffer.
        vs.duel.hits = [N + 1, N];
        vs.duel.to_move = Side::Right;
        assert_eq!(versus::pip_count(vs.duel.hits), N + 1);
        let (c, _) = draw(&g);
        assert!(!c.palette_text(&Palette::default()).contains('.'));
        let vs = g.versus.live.as_mut().unwrap();
        vs.stage = VersusStage::Aiming;
        let (c, _) = draw(&g);
        assert!(!c.palette_text(&Palette::default()).contains('.'));
        // Left missed at 3-3: right's hit would win outright.
        let vs = g.versus.live.as_mut().unwrap();
        vs.stage = VersusStage::Handoff;
        vs.rolled = None;
        vs.duel.hits = [N, N];
        vs.duel.to_move = Side::Right;
        assert_eq!(
            versus::handoff_cues(vs),
            [("HIT TO WIN", versus::side_slot(Side::Right))]
        );
        // An ordinary hit names the scorer.
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.hits = [1, 0];
        vs.duel.to_move = Side::Right;
        assert_eq!(versus::hit_line(vs, Side::Left), "LEFT GUN SCORES");
        assert_eq!(versus::pip_count(vs.duel.hits), N);
    }

    #[test]
    fn a_long_overtime_score_stays_clear_of_the_pause_button_and_the_plates() {
        use crate::app::VersusStage;
        use crate::versus::Side;
        let mut g = game();
        g.input.touch_active = true;
        g.start_versus();
        assert_eq!(g.display.buffer_w, 320, "the narrowest layout");
        let w = g.display.buffer_w;
        let ex = g.display.corner_pad.0;
        // The pause button's right edge, top centre.
        let pause_right = (w - 20) / 2 + 20;
        for hits in [[4, 3], [5, 4], [7, 6], [10, 9]] {
            let score = versus::score_w(hits);
            assert!(score <= 190, "{hits:?}: {score} wide in a 200 plate");
            let plate_left = w - (score + 8) - 3 - ex;
            assert!(plate_left > pause_right, "{hits:?}: under the pause button");
            assert_eq!(versus::score_numeric(hits), hits[0] > versus::PIP_CAP);
            for stage in [
                VersusStage::Handoff,
                VersusStage::Aiming,
                VersusStage::Result,
            ] {
                let vs = g.versus.live.as_mut().unwrap();
                vs.duel.hits = hits;
                vs.duel.to_move = Side::Right;
                vs.stage = stage;
                vs.duel.phase = if stage == VersusStage::Result {
                    crate::versus::Phase::RoundOver(Side::Left)
                } else {
                    crate::versus::Phase::Aiming
                };
                let (c, _) = draw(&g);
                assert!(
                    !c.palette_text(&Palette::default()).contains('.'),
                    "{hits:?} {stage:?} left pixels undrawn"
                );
            }
        }
        // Numbers past the cap read differently from pips.
        let vs = g.versus.live.as_mut().unwrap();
        vs.stage = VersusStage::Aiming;
        vs.duel.phase = crate::versus::Phase::Aiming;
        vs.duel.hits = [5, 4];
        let pips = draw(&g).0;
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.hits = [7, 6];
        let numbers = draw(&g).0;
        assert_ne!(pips.bytes(), numbers.bytes());
    }

    #[test]
    fn the_aim_help_preview_flies_the_quantized_shot() {
        use crate::versus::{Match, Side};
        let duel = Match::new(5, 320);
        for side in [Side::Left, Side::Right] {
            // 0.2 degrees off the 0.5-degree grid fires as the grid angle,
            // so it must preview as the grid angle too, to the bit.
            let bits = |aim| {
                let (path, upto) = versus::aim_preview(&duel, side, aim);
                let bits: Vec<(u64, u64)> = path
                    .iter()
                    .map(|p| (p.x.to_bits(), p.y.to_bits()))
                    .collect();
                (bits, upto)
            };
            assert_eq!(bits((904, 903)), bits((900, 903)), "{side:?}");
            assert_ne!(bits((910, 903)), bits((900, 903)), "{side:?}");
        }
    }

    /// How many of `c`'s pixels hold `colour` within rows `y0..y1`.
    fn count_in_rows(c: &Canvas, y0: i32, y1: i32, colour: crate::palette::Rgba) -> usize {
        c.bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, px)| {
                let y = *i as i32 / c.w;
                (y0..y1).contains(&y) && **px == colour
            })
            .count()
    }

    #[test]
    fn each_versus_player_has_a_colour_of_their_own() {
        use crate::versus::Side;
        let (left, right) = (
            versus::side_slot(Side::Left),
            versus::side_slot(Side::Right),
        );
        assert_ne!(left, right);
        for side in [left, right] {
            // Neither may be mistaken for plain or secondary text.
            assert!(![slot::UI_FG, slot::UI_BG, slot::ROCK].contains(&side));
        }
        // The handoff plate is bordered in the colour of whoever is next.
        let mut g = game();
        g.start_versus();
        for side in Side::BOTH {
            g.versus.live.as_mut().unwrap().duel.to_move = side;
            let (c, _) = draw(&g);
            // Just inside the plate's dark edge.
            let (x, y) = ((c.w - 200) / 2 + 1, c.h / 2);
            let i = ((y * c.w + x) * 4) as usize;
            let want = Palette::default().get(versus::side_slot(side));
            assert_eq!(c.bytes()[i..i + 4], want, "{side:?}");
        }
    }

    /// A versus match with the left gun aiming, its aim help on.
    fn left_aiming() -> Game {
        let mut g = game();
        g.versus.help = [true, true];
        g.start_versus();
        g.versus.live.as_mut().unwrap().stage = crate::app::VersusStage::Aiming;
        g
    }

    /// Fire `shot` from the left gun of `left_aiming`, one frame in.
    fn left_fires(g: &mut Game, shot: Shot) {
        let space = Intents {
            window: WINDOW,
            events: vec![Event::key(Key::Space)],
            keys_held: vec![Key::Space],
            ..Intents::default()
        };
        (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
        g.tick(1.0 / 60.0, &space);
    }

    /// Idle frames until the landed shot is called out.
    fn until_called(g: &mut Game) {
        let idle = Intents {
            window: WINDOW,
            ..Intents::default()
        };
        while g.versus.live.as_ref().unwrap().callout().is_none() {
            g.tick(1.0 / 60.0, &idle);
        }
    }

    /// A shot from the left gun that hits the right one.
    fn left_hitting(g: &Game) -> Shot {
        use crate::versus::{self as rules, Side};
        let duel = &g.versus.live.as_ref().unwrap().duel;
        let frame = rules::frame_for(&duel.screen, Side::Left);
        let target = rules::target_in_frame(&duel.screen, Side::Left);
        rules::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)[0]
    }

    #[test]
    fn a_landed_versus_shot_is_called_out_large() {
        use crate::app::VersusCall as Call;
        use crate::versus::Side;
        let pal = Palette::default();
        // The callout's band, under the score plate.
        let band = |c: &Canvas, colour| count_in_rows(c, 28, 70, colour);
        // A hit, a short lob into the open, and straight up onto its own gun.
        for call in [Call::Hit, Call::Miss, Call::Oops] {
            let mut g = left_aiming();
            let shot = match call {
                Call::Hit => left_hitting(&g),
                Call::Miss => Shot::new(1400, 330),
                Call::Oops => Shot::new(1800, 0),
            };
            left_fires(&mut g, shot);
            let before = draw(&g).0;
            until_called(&mut g);
            assert_eq!(
                g.versus.live.as_ref().unwrap().callout(),
                Some((Side::Left, call))
            );
            let (c, _) = draw(&g);
            // HIT! in the left player's colour, four times the small face;
            // MISS and OOPS in plain text.
            let colour = pal.get(if call == Call::Hit {
                slot::ALERT
            } else {
                slot::UI_FG
            });
            assert!(band(&c, colour) > 200, "{call:?}: {}", band(&c, colour));
            assert!(band(&before, colour) < 50, "{call:?}: callout too soon");
        }
    }

    #[test]
    fn nothing_in_a_side_colour_touches_the_field() {
        // The right player's blue is the sky's own horizon band, so every
        // mark in a side colour must sit on a plate or a dark backing. Both
        // sides draw through the same code (`side_slot`, `side_ink`); this
        // checks it with the left side's amber, which is nowhere else in a
        // versus frame: each amber pixel may touch only amber, the plate
        // colours, or the impact's dust, which is drawn over the flash and
        // is dark itself.
        use crate::app::VersusStage;
        use crate::versus::Side;
        let pal = Palette::default();
        let ok = [slot::ALERT, slot::UI_BG, slot::SHADOW, slot::ST_DMG].map(|s| pal.get(s));
        let check = |c: &Canvas, what: &str| {
            let px = |x: i32, y: i32| {
                let i = ((y * c.w + x) * 4) as usize;
                [
                    c.bytes()[i],
                    c.bytes()[i + 1],
                    c.bytes()[i + 2],
                    c.bytes()[i + 3],
                ]
            };
            let mut amber = 0;
            for y in 0..c.h {
                for x in 0..c.w {
                    if px(x, y) != ok[0] {
                        continue;
                    }
                    amber += 1;
                    for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                        if (0..c.w).contains(&nx) && (0..c.h).contains(&ny) {
                            assert!(
                                ok.contains(&px(nx, ny)),
                                "{what}: amber at ({x}, {y}) touches {:?} at ({nx}, {ny})",
                                px(nx, ny)
                            );
                        }
                    }
                }
            }
            assert!(amber > 0, "{what}: nothing amber drawn");
        };

        // Aiming, with aim help: the tag, the name, the preview's head and
        // last turn's landing mark.
        let mut g = left_aiming();
        let mark =
            crate::versus::muzzle_of(&g.versus.live.as_ref().unwrap().duel.screen, Side::Left);
        g.versus.live.as_mut().unwrap().duel.landing[0] = Some(Vec2::new(mark.x + 60.0, mark.y));
        check(&draw(&g).0, "aiming");
        // A shot leaving: the muzzle flash and the shell's head.
        let shot = left_hitting(&g);
        left_fires(&mut g, shot);
        check(&draw(&g).0, "fired");
        // Landed on the right gun: the callout, the gun's outline and the
        // screen's edge flash.
        until_called(&mut g);
        check(&draw(&g).0, "hit");
        // The handoff and result plates, left's turn and left's win.
        let mut g = left_aiming();
        g.versus.live.as_mut().unwrap().stage = VersusStage::Handoff;
        check(&draw(&g).0, "handoff");
        let vs = g.versus.live.as_mut().unwrap();
        vs.duel.phase = crate::versus::Phase::MatchOver(Side::Left);
        vs.stage = VersusStage::Result;
        check(&draw(&g).0, "result");
    }

    #[test]
    fn the_aiming_tag_is_under_the_aim_help_preview() {
        // A steep aim's preview climbs through the tag over the gun: the
        // tag is drawn first, so the preview shows over it.
        let mut g = left_aiming();
        (g.play.angle_ticks, g.play.power_ticks) = (1700, 600);
        let (with_help, _) = draw(&g);
        g.versus.help = [false, false];
        let (without, _) = draw(&g);
        let fg = Palette::default().get(slot::UI_FG);
        let screen = &g.versus.live.as_ref().unwrap().duel.screen;
        let m = crate::versus::muzzle_of(screen, crate::versus::Side::Left);
        // The tag's rows, a few pixels either side of the pivot.
        let (x, y) = (m.x.round() as i32, (179.0 - m.y).round() as i32);
        let tag = |c: &Canvas| {
            (y - 30..y - 18)
                .flat_map(|ty| (x - 3..=x + 3).map(move |tx| (tx, ty)))
                .filter(|&(tx, ty)| {
                    let i = ((ty * c.w + tx) * 4) as usize;
                    c.bytes()[i..i + 4] == fg
                })
                .count()
        };
        assert_eq!(tag(&without), 0);
        assert!(tag(&with_help) > 0, "the preview is hidden under the tag");
    }

    #[test]
    fn a_flipped_blit_is_the_blit_reflected() {
        // Three columns, the middle one transparent.
        let sprite = Sprite {
            w: 3,
            h: 1,
            px: vec![1, 2, 3, 255, 0, 0, 0, 0, 7, 8, 9, 255],
            detail: None,
        };
        let mut a = Canvas::new(5, 1);
        a.blit(&sprite, 1, 0);
        let mut b = Canvas::new(5, 1);
        b.blit_flipped(&sprite, 1, 0);
        let px = |c: &Canvas, x: usize| c.bytes()[x * 4..x * 4 + 4].to_vec();
        assert_eq!(px(&a, 1), px(&b, 3));
        assert_eq!(px(&a, 3), px(&b, 1));
        assert_eq!(px(&a, 2), px(&b, 2), "transparent stays transparent");
    }

    #[test]
    fn a_resized_buffer_is_drawn_at_its_new_size() {
        let mut g = game();
        let fx = g.fit_display((1600, 720), (0, 0));
        assert!(fx.contains(&Effect::Buffer {
            w: 400,
            h: 180,
            letterbox: (0, 0)
        }));
        let (canvas, _) = draw(&g);
        assert_eq!((canvas.w, canvas.h), (400, 180));
    }
}

//! A [`Game`] painted into a [`Canvas`]. Engine-free.

use salt_line_sim::modifiers::{Loadout, Modifier};
use salt_line_sim::run::{Screen, StructState, Structure, StructureKind};
use salt_line_sim::terrain::Material;
use shift_sim::expedition::{Pick, State};
use shift_sim::stage::{Role, BASTION_INTEGRITY};
use shift_sim::{PhaseMod, Plane, Stage};

use crate::app::{hit, stage_label, Game, Mode, Play, AIM_NEAR, AIM_SPAN, PAUSE_ITEMS};
use crate::canvas::{dither, Canvas, Rgba, H, W};
use crate::font::{self, centred, text, text_outlined};
use crate::sprites::{Atlas, Ink};

/// How one era looks.
pub struct Grade {
    /// Sky stops from the top of the screen to the horizon.
    pub sky: [Rgba; 4],
    pub sand: Rgba,
    pub rock: Rgba,
    pub bedrock: Rgba,
    pub edge: Rgba,
}

pub const PRESENT: Grade = Grade {
    sky: [
        Rgba::rgb(84, 112, 152),
        Rgba::rgb(118, 146, 180),
        Rgba::rgb(170, 190, 206),
        Rgba::rgb(226, 222, 206),
    ],
    sand: Rgba::rgb(232, 220, 200),
    rock: Rgba::rgb(168, 150, 128),
    bedrock: Rgba::rgb(107, 92, 77),
    edge: Rgba::rgb(248, 242, 228),
};

pub const SIEGE: Grade = Grade {
    sky: [
        Rgba::rgb(22, 26, 52),
        Rgba::rgb(52, 48, 86),
        Rgba::rgb(120, 78, 92),
        Rgba::rgb(214, 132, 84),
    ],
    sand: Rgba::rgb(128, 98, 92),
    rock: Rgba::rgb(92, 72, 74),
    bedrock: Rgba::rgb(58, 46, 54),
    edge: Rgba::rgb(196, 140, 110),
};

pub fn grade(p: Plane) -> &'static Grade {
    match p {
        Plane::Present => &PRESENT,
        Plane::Siege => &SIEGE,
    }
}

pub const INK: Rgba = Rgba::rgb(245, 239, 226);
pub const DARK: Rgba = Rgba::rgb(30, 28, 34);
pub const ALERT: Rgba = Rgba::rgb(232, 163, 61);
pub const ECHO: Rgba = Rgba::rgb(150, 210, 240);
pub const RED: Rgba = Rgba::rgb(214, 84, 72);

/// World y (up) to screen row.
#[inline]
pub fn sy(y: f64) -> i32 {
    H as i32 - y.round() as i32
}

pub struct Renderer {
    pub atlas: Atlas,
    title_stage: Option<Stage>,
    a: Canvas,
    b: Canvas,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Renderer {
            atlas: Atlas::embedded(),
            title_stage: None,
            a: Canvas::new(),
            b: Canvas::new(),
        }
    }

    pub fn draw(&mut self, g: &Game, out: &mut Canvas) {
        match &g.mode {
            Mode::Title { t } => self.title(g, *t, out),
            Mode::Play(p) => self.play(p, out),
            Mode::Draft { play, sel } => {
                self.play(play, out);
                self.draft(play, *sel, out);
            }
            Mode::Over { play, victory } => {
                self.play(play, out);
                self.over(play, *victory, g, out);
            }
        }
    }

    // ------------------------------------------------------------ play

    fn play(&mut self, p: &Play, out: &mut Canvas) {
        let stage = p.shown();
        let base = p.run.base_loadout();
        let now = stage.active();
        let mut a = std::mem::take(&mut self.a);
        self.era(&mut a, stage, now, base, p);
        if let Some((from, t)) = p.fade {
            let mut b = std::mem::take(&mut self.b);
            self.era(&mut b, stage, from, base, p);
            // Dissolve from the old era to the new.
            out.px.copy_from_slice(&b.px);
            out.dissolve(&a, t);
            self.b = b;
        } else {
            out.px.copy_from_slice(&a.px);
        }
        self.a = a;
        self.flight(p, out);
        self.blasts(p, out);
        let (left, amp) = p.shake;
        if left > 0.0 && p.paused.is_none() {
            // A few pixels, decaying, stepping every 1/30 s; the HUD holds still.
            let k = (left / 0.35).min(1.0) * amp;
            let n = (left * 30.0) as i32;
            let (dx, dy) = (
                [1, -1, 0, 1, -1][n as usize % 5],
                [-1, 1, 1, 0, -1][n as usize % 5],
            );
            out.nudge(
                (dx as f32 * k).round() as i32,
                (dy as f32 * k).round() as i32,
            );
        }
        self.hud(p, out);
    }

    /// One era of a stage, complete: sky, ground, the other era's ghosts,
    /// structures, the gun, aim and the last shot's chalk line.
    fn era(&self, c: &mut Canvas, stage: &Stage, era: Plane, base: &Loadout, p: &Play) {
        let screen = stage.screen(era);
        let gr = grade(era);
        sky(c, gr);
        ground(c, screen, gr);
        // Ghosts of what stands in the other era, so you can plan.
        let other = stage.era(era.other());
        for (s, r) in other.screen.structures.iter().zip(&other.roles) {
            if !s.is_down() {
                // Seated on this era's ground, where the player is looking.
                let mut g = s.clone();
                g.base = screen.field.seat(g.x0, g.footprint());
                self.structure(c, &g, *r, era.other(), Ink::Ghost(ECHO, 0.55), false);
            }
        }
        let e = stage.era(era);
        for (s, r) in e.screen.structures.iter().zip(&e.roles) {
            let ink = match r {
                Role::Echo if !s.is_down() => Ink::Tint(ECHO, 0.55),
                _ if s.is_down() => Ink::Tint(gr.bedrock, 0.25),
                _ => Ink::Solid,
            };
            self.structure(c, s, *r, era, ink, true);
        }
        // Chalk line of the last shot in this era.
        for (k, v) in p.last_path[era.index()].iter().enumerate() {
            if k % 5 == 0 {
                c.blend(v.x.round() as i32, sy(v.y), INK, 0.35);
            }
        }
        self.gun(c, screen, era, p);
        let _ = base; // the gun's drafted variants are not drawn yet
    }

    fn structure(
        &self,
        c: &mut Canvas,
        s: &Structure,
        role: Role,
        era: Plane,
        ink: Ink,
        bars: bool,
    ) {
        let name = sprite_name(s.kind, s.state, era);
        let spr = self.atlas.get(&name);
        let cx = s.centre_x();
        let x = (cx - spr.w as f64 / 2.0).round() as i32;
        let y = sy(s.base) - spr.h + 1;
        c.draw_spr(spr, x, y, s.lean < 0, ink);
        if !bars || s.is_down() {
            return;
        }
        let max = if role == Role::Bastion {
            BASTION_INTEGRITY
        } else {
            1.0
        };
        let top = y - 4;
        if role == Role::Bastion {
            // A pennant over the bastion.
            let fx = cx.round() as i32;
            c.line(fx, top - 9, fx, top - 1, DARK, 0);
            c.rect(fx + 1, top - 9, 5, 3, RED);
        }
        if role != Role::Original || s.integrity < max - 1e-9 {
            let w = s.footprint() as i32 + 4;
            let bx = cx.round() as i32 - w / 2;
            c.rect(bx, top, w, 2, DARK);
            let fill = ((s.integrity / max).clamp(0.0, 1.0) * w as f64).round() as i32;
            let col = if role == Role::Echo {
                ECHO
            } else if role == Role::Bastion {
                RED
            } else {
                ALERT
            };
            c.rect(bx, top, fill, 2, col);
        }
    }

    fn gun(&self, c: &mut Canvas, screen: &Screen, era: Plane, p: &Play) {
        let m = screen.muzzle();
        let (mx, my) = (m.x.round() as i32, sy(m.y));
        let deg = p.angle_ticks as f64 * 0.05;
        let frame = ((deg / 5.0).round() as i32).clamp(0, 18);
        match era {
            Plane::Present => {
                // Drafted gun modifiers show on the howitzer, with the
                // engine's own variant art.
                let lo = p.run.base_loadout();
                let ballast = lo.stacks(Modifier::Ballast) > 0;
                let (tank, pivot) = if ballast {
                    (self.atlas.get("tank_ballast"), (13, 4))
                } else {
                    (self.atlas.get("tank_base"), (15, 3))
                };
                c.draw_spr(tank, mx - pivot.0, my - pivot.1, false, Ink::Solid);
                let strip = self.atlas.get(if ballast {
                    "barrel_strip_ballast"
                } else if lo.stacks(Modifier::Sabot) > 0 {
                    "barrel_strip_sabot"
                } else {
                    "barrel_strip_5deg"
                });
                strip.draw_region(c, frame * 28, 28, mx - 3, my - 24, false, Ink::Solid);
            }
            Plane::Siege => {
                let ground = my + 6;
                let fr = self.atlas.get("trebuchet");
                let (fx, fy) = (mx - 15, ground + 1 - fr.h);
                c.draw_spr(fr, fx, fy, false, Ink::Solid);
                let strip = self.atlas.get("trebuchet_arm_strip_5deg");
                // Mid-throw the arm has swung through; at rest it is cocked.
                let shown = if p.flight.as_ref().is_some_and(|f| f.era == Plane::Siege) {
                    frame
                } else {
                    (frame - 12).max(0)
                };
                strip.draw_region(
                    c,
                    shown * 48,
                    48,
                    fx + 15 - 24,
                    fy + 3 - 24,
                    false,
                    Ink::Solid,
                );
            }
        }
        // Aim guide: a dotted ray from the muzzle, longer with power. While
        // a pointer drags, it reaches the point the drag aims at.
        if p.flight.is_none() && p.view.is_none() {
            let len = if p.drag.is_some() {
                AIM_NEAR + AIM_SPAN * p.power_ticks as f64 / 1000.0
            } else {
                10.0 + 34.0 * p.power_ticks as f64 / 1000.0
            };
            let r = deg.to_radians();
            let (ex, ey) = (m.x + r.cos() * len, m.y + r.sin() * len);
            c.line(mx, my, ex.round() as i32, sy(ey), INK, 3);
        }
    }

    fn flight(&self, p: &Play, c: &mut Canvas) {
        let Some(f) = &p.flight else { return };
        if f.era != p.era() {
            return;
        }
        let k = f.step();
        for tr in &f.traces {
            if tr.path.is_empty() {
                continue;
            }
            let i = k.min(tr.path.len() - 1);
            for j in (i.saturating_sub(24)..i).step_by(4) {
                let v = tr.path[j];
                c.blend(v.x.round() as i32, sy(v.y), INK, 0.6);
            }
            let v = tr.path[i];
            let (x, y) = (v.x.round() as i32, sy(v.y));
            if f.era == Plane::Siege {
                c.draw_spr(self.atlas.get("stone"), x - 1, y - 1, false, Ink::Solid);
            } else {
                c.rect(x - 1, y - 1, 2, 2, DARK);
            }
        }
    }

    fn blasts(&self, p: &Play, c: &mut Canvas) {
        for b in &p.blasts {
            if b.era != p.era() {
                continue;
            }
            let t = b.t / 0.7;
            let r = ((if b.big { 13.0 } else { 9.0 }) * t.sqrt()) as i32;
            let (x, y) = (b.pos.x.round() as i32, sy(b.pos.y));
            c.circle(x, y, r, ALERT, (1.0 - t) * 0.8);
            c.circle(x, y, r / 2, INK, (1.0 - t) * 0.9);
        }
    }

    fn hud(&self, p: &Play, c: &mut Canvas) {
        let era = p.era();
        let run = &p.run;
        c.rect_blend(0, 0, W as i32, 9, DARK, 0.7);
        let name = match era {
            Plane::Present => "THE PRESENT - HOWITZER",
            Plane::Siege => "1347 - TREBUCHET",
        };
        text(
            c,
            name,
            3,
            2,
            1,
            if era == Plane::Siege { ALERT } else { INK },
        );
        let ammo = if era == Plane::Siege {
            "STONES"
        } else {
            "SHELLS"
        };
        let right = format!(
            "SCREEN {}  {ammo} {}  ECHOES {}",
            stage_label(p),
            run.shells(),
            p.shown().echoes_left()
        );
        text(
            c,
            &right,
            hit::PAUSE.x - 3 - font::width(&right, 1),
            2,
            1,
            INK,
        );
        // Pause button: two bars.
        let pb = hit::PAUSE;
        c.rect(pb.x, 0, 1, 9, Rgba::rgb(90, 86, 96));
        c.rect(pb.x + 4, 2, 2, 5, INK);
        c.rect(pb.x + 8, 2, 2, 5, INK);

        // Bottom band: aim, wind, SHIFT.
        let y = H as i32 - 8;
        c.rect_blend(0, y - 2, W as i32, 10, DARK, 0.7);
        let aim = format!(
            "ANGLE {:.1}  POWER {}%",
            p.angle_ticks as f64 * 0.05,
            p.power_ticks / 10
        );
        text(c, &aim, 3, y, 1, INK);
        let screen = p.shown().screen(era);
        let lo = era.loadout(run.base_loadout());
        let w = screen.wind * lo.wind_scale;
        let arrows = (1 + (w.abs() / 6.0) as usize).min(4);
        let wind = if w.abs() < 0.5 {
            "WIND CALM".to_string()
        } else if w > 0.0 {
            format!("WIND {} {:.0}", ">".repeat(arrows), w.abs())
        } else {
            format!("WIND {} {:.0}", "<".repeat(arrows), w.abs())
        };
        centred(c, &wind, 160, y, 1, INK);
        let to = if era == Plane::Present {
            "1347"
        } else {
            "PRESENT"
        };
        let (shift, col) = if p.flight.is_some() || p.view.is_some() {
            ("SHIFT ...".to_string(), Rgba::rgb(120, 116, 112))
        } else if *run.state() != State::Firing {
            (String::new(), INK)
        } else if !run.can_shift() {
            ("SHIFT - NO SHELLS".to_string(), RED)
        } else if run.shift_cost() > 0 {
            (format!("[SHIFT] {to} -1"), ALERT)
        } else {
            (format!("[SHIFT] {to}"), ECHO)
        };
        if !shift.is_empty() {
            // A button: set off by a divider, so it reads as tappable.
            let w = font::width(&shift, 1) + 6;
            let x = W as i32 - 1 - w;
            c.rect(x - 1, y - 2, 1, 10, Rgba::rgb(90, 86, 96));
            text(c, &shift, x + 3, y, 1, col);
        }

        // Era modifiers held, under the top band.
        let mut mods = Vec::new();
        if run.holds(PhaseMod::HalfPhased) {
            mods.push("HALF-PHASED");
        }
        if run.holds(PhaseMod::EchoDecay) {
            mods.push("ECHO DECAY");
        }
        if !mods.is_empty() {
            text_outlined(c, &mods.join("  "), 3, 12, 1, ECHO, DARK);
        }

        if let Some(n) = &p.note {
            let a = if n.t > 1.8 {
                1.0 - (n.t - 1.8) / 0.4
            } else {
                1.0
            };
            if a > 0.0 {
                let w = font::width(&n.text, 1) + 10;
                c.rect_blend(160 - w / 2, 40, w, 11, DARK, 0.75 * a);
                if a > 0.15 {
                    centred(c, &n.text, 160, 43, 1, ALERT);
                }
            }
        }
        let banner = if p.drag.is_some() {
            Some(("RELEASE TO FIRE - DRAG TO A BAR TO CANCEL", INK))
        } else {
            first_screen_hint(p)
        };
        if let Some((s, col)) = banner {
            let w = font::width(s, 1) + 8;
            c.rect_blend(160 - w / 2, 12, w, 9, DARK, 0.7);
            centred(c, s, 160, 14, 1, col);
        }
        if let Some(sel) = p.paused {
            c.rect_blend(0, 0, W as i32, H as i32, DARK, 0.6);
            let x = 160 - font::width("PAUSED", 2) / 2;
            text(c, "PAUSED", x, 54, 2, ALERT);
            for (i, item) in PAUSE_ITEMS.iter().enumerate() {
                let r = hit::pause_item(i);
                let on = i == sel;
                c.rect(
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    if on {
                        Rgba::rgb(60, 56, 70)
                    } else {
                        Rgba::rgb(40, 38, 46)
                    },
                );
                frame(c, r, if on { ALERT } else { Rgba::rgb(90, 86, 96) });
                centred(c, item, 160, r.y + 4, 1, if on { ALERT } else { INK });
            }
            centred(c, "ESC RESUMES", 160, 116, 1, Rgba::rgb(180, 176, 170));
        }
    }

    // ------------------------------------------------------------ menus

    fn title(&mut self, g: &Game, t: f32, out: &mut Canvas) {
        if self.title_stage.is_none() {
            self.title_stage = Some(Stage::generate(0x1347, 3, W, &Loadout::new()));
        }
        let stage = self
            .title_stage
            .take()
            .unwrap_or_else(|| Stage::generate(0x1347, 3, W, &Loadout::new()));
        let demo = Play::new(0x1347);
        let mut a = std::mem::take(&mut self.a);
        let mut b = std::mem::take(&mut self.b);
        self.era(&mut a, &stage, Plane::Present, &Loadout::new(), &demo);
        self.era(&mut b, &stage, Plane::Siege, &Loadout::new(), &demo);
        // Slow tide between the eras: 4 s each way.
        let phase = (t / 4.0) % 2.0;
        let k = if phase < 1.0 { phase } else { 2.0 - phase };
        let k = (k * 1.6 - 0.3).clamp(0.0, 1.0);
        out.px.copy_from_slice(&a.px);
        out.dissolve(&b, k);
        self.a = a;
        self.b = b;
        self.title_stage = Some(stage);

        out.rect_blend(0, 22, W as i32, 50, DARK, 0.72);
        let title = "COUNTERWEIGHT";
        let x = 160 - font::width(title, 3) / 2;
        text(out, title, x + 1, 29, 3, DARK);
        text(out, title, x, 28, 3, INK);
        centred(
            out,
            "ONE COAST. TWO ERAS. SHIFT BETWEEN THEM.",
            160,
            50,
            1,
            ALERT,
        );
        centred(
            out,
            "BREAK THE LINE IN BOTH, OR NOT AT ALL.",
            160,
            58,
            1,
            ECHO,
        );

        let blink = (t * 2.0) as i32 % 2 == 0;
        out.rect_blend(58, 92, 204, 70, DARK, 0.75);
        if blink {
            centred(out, "PRESS ENTER OR TAP - EXPEDITION", 160, 96, 1, INK);
        }
        let lines = [
            "UP/DOWN  ANGLE     LEFT/RIGHT  POWER",
            "SPACE  FIRE      SHIFT  CHANGE ERA",
            if g.web {
                "Z  FINE AIM      ESC  PAUSE"
            } else {
                "Z  FINE AIM   ESC  PAUSE   F11  FULLSCREEN"
            },
            "PAD: D-PAD AIM  A FIRE  LB/RB SHIFT  X FINE",
            "MOUSE/TOUCH: DRAG TO AIM, RELEASE TO FIRE",
            "12 SCREENS. THE BASTION WAITS AT THE END.",
        ];
        for (i, l) in lines.iter().enumerate() {
            centred(
                out,
                l,
                160,
                106 + i as i32 * 9,
                1,
                if i == 5 { ALERT } else { INK },
            );
        }
        if g.best > 0 {
            let b = format!("BEST {}", g.best);
            text_outlined(
                out,
                &b,
                W as i32 - 3 - font::width(&b, 1),
                H as i32 - 8,
                1,
                ALERT,
                DARK,
            );
        }
        let stamp = format!("COUNTERWEIGHT  {}  GAME OFF 2026", g.stamp);
        text_outlined(
            out,
            &stamp,
            3,
            H as i32 - 8,
            1,
            Rgba::rgb(180, 176, 170),
            DARK,
        );
    }

    fn draft(&self, p: &Play, sel: usize, c: &mut Canvas) {
        let State::Drafting(offers) = p.run.state() else {
            return;
        };
        c.rect_blend(0, 0, W as i32, H as i32, DARK, 0.55);
        centred(c, "THE LINE HOLDS. CHOOSE AN ADVANTAGE.", 160, 24, 1, ALERT);
        let cw = hit::CARD_W;
        for (i, o) in offers.iter().enumerate() {
            let x = hit::card(i, offers.len()).x;
            let on = i == sel;
            c.rect(
                x,
                40,
                cw,
                92,
                if on {
                    Rgba::rgb(60, 56, 70)
                } else {
                    Rgba::rgb(40, 38, 46)
                },
            );
            let border = if on { ALERT } else { Rgba::rgb(90, 86, 96) };
            c.rect(x, 40, cw, 1, border);
            c.rect(x, 131, cw, 1, border);
            c.rect(x, 40, 1, 92, border);
            c.rect(x + cw - 1, 40, 1, 92, border);
            text(c, &format!("{}", i + 1), x + 4, 44, 1, border);
            let era_mod = matches!(o, Pick::Phase(_));
            let mut y = 54;
            for line in font::wrap(o.name(), cw - 8, 1) {
                text(c, &line, x + 4, y, 1, if era_mod { ECHO } else { INK });
                y += 7;
            }
            y += 4;
            for line in font::wrap(o.blurb(), cw - 8, 1) {
                text(c, &line, x + 4, y, 1, Rgba::rgb(200, 196, 188));
                y += 7;
            }
        }
        centred(c, "1-3, ARROWS + ENTER, OR TAP TWICE", 160, 138, 1, INK);
        let r = hit::SKIP;
        c.rect(r.x, r.y, r.w, r.h, Rgba::rgb(40, 38, 46));
        frame(c, r, Rgba::rgb(90, 86, 96));
        centred(c, "SKIP (ESC)", 160, r.y + 4, 1, INK);
    }

    fn over(&self, p: &Play, victory: bool, g: &Game, c: &mut Canvas) {
        c.rect_blend(0, 0, W as i32, H as i32, DARK, 0.6);
        c.rect(60, 36, 200, 104, Rgba::rgb(40, 38, 46));
        let (head, col) = if victory {
            ("THE BASTION FALLS", ALERT)
        } else {
            ("THE LINE HOLDS", RED)
        };
        let x = 160 - font::width(head, 2) / 2;
        text(c, head, x, 44, 2, col);
        let secs = p.elapsed as u32;
        let rows = [
            format!("SCREENS CLEARED   {}/12", p.run.stages_cleared()),
            format!("SHELLS LEFT       {}", p.run.shells().max(0)),
            format!("SHOTS FIRED       {}", p.run.total_shots()),
            format!("TIME              {}:{:02}", secs / 60, secs % 60),
            format!("SCORE             {}", p.run.score(secs)),
        ];
        for (i, r) in rows.iter().enumerate() {
            text(
                c,
                r,
                84,
                64 + i as i32 * 10,
                1,
                if i == 4 { ALERT } else { INK },
            );
        }
        if g.new_best {
            centred(c, "NEW BEST", 160, 116, 1, ALERT);
        } else if g.best > 0 {
            centred(
                c,
                &format!("BEST {}", g.best),
                160,
                116,
                1,
                Rgba::rgb(180, 176, 170),
            );
        }
        centred(c, "ENTER OR TAP - TITLE", 160, 128, 1, ECHO);
    }
}

fn sky(c: &mut Canvas, g: &Grade) {
    let horizon = 120.0;
    for y in 0..H as i32 {
        let t = (y as f32 / horizon).min(1.0) * 3.0;
        let i = (t.floor() as usize).min(2);
        let f = t - i as f32;
        for x in 0..W as i32 {
            let col = if dither(x, y, f) {
                g.sky[i + 1]
            } else {
                g.sky[i]
            };
            c.set(x, y, col);
        }
    }
}

fn ground(c: &mut Canvas, screen: &Screen, g: &Grade) {
    let f = &screen.field;
    for x in 0..(W as i32).min(f.len() as i32) {
        let surf = f.surface(x as usize);
        let base = f.base[x as usize];
        let top = sy(surf);
        for y in top.max(0)..H as i32 {
            let world_y = (H as i32 - y) as f64;
            let col = if y == top {
                g.edge
            } else {
                // Debris piled above the original ground reads as sand.
                let depth = base - world_y;
                match Material::at_depth(depth.max(0.0)) {
                    Material::Sand => g.sand,
                    Material::Rock => g.rock,
                    _ => g.bedrock,
                }
            };
            c.set(x, y, col);
        }
    }
}

/// Sprite name for a structure kind in a state, in an era.
pub fn sprite_name(kind: StructureKind, state: StructState, era: Plane) -> String {
    let stem = match (kind, era) {
        (StructureKind::Derrick, Plane::Present) => "derrick",
        (StructureKind::RadioMast, Plane::Present) => "radio_mast",
        (StructureKind::WaterTower, Plane::Present) => "tower",
        (StructureKind::Silo, Plane::Present) => "silo",
        (StructureKind::Pylon, Plane::Present) => "pylon",
        (StructureKind::Derrick, Plane::Siege) => "watchtower",
        (StructureKind::RadioMast, Plane::Siege) => "banner_pole",
        (StructureKind::WaterTower, Plane::Siege) => "bell_tower",
        (StructureKind::Silo, Plane::Siege) => "keep",
        (StructureKind::Pylon, Plane::Siege) => "siege_tower",
    };
    let suffix = match state {
        StructState::Intact => "",
        StructState::Leaning => "_leaning",
        StructState::Collapsed | StructState::Cleared => "_collapsed",
    };
    if stem == "tower" && suffix.is_empty() {
        "tower_intact".to_string()
    } else {
        format!("{stem}{suffix}")
    }
}

impl Canvas {
    fn draw_spr(&mut self, s: &crate::sprites::Sprite, x: i32, y: i32, flip: bool, ink: Ink) {
        s.draw(self, x, y, flip, ink);
    }
}

/// On the first screen only, one line at a time on what to do next.
fn first_screen_hint(p: &Play) -> Option<(&'static str, Rgba)> {
    let run = &p.run;
    if run.stage_index() != 0
        || p.flight.is_some()
        || p.view.is_some()
        || p.note.is_some()
        || *run.state() != State::Firing
    {
        return None;
    }
    let log = run.log();
    let first_shift = log.iter().position(|i| *i == shift_sim::Input::Shift);
    let shifted = first_shift.is_some();
    // Fired since the first SHIFT.
    let fired_after = first_shift.is_some_and(|i| {
        log[i..]
            .iter()
            .any(|x| matches!(x, shift_sim::Input::Fire { .. }))
    });
    let stage = p.shown();
    let here = p.era();
    let there = here.other();
    if run.total_shots() == 0 {
        return Some(("BREAK THE LINE: AIM, THEN FIRE", INK));
    }
    if !shifted && stage.era(there).echoes_left() > 0 {
        return Some(("ITS ECHO STANDS IN THE OTHER ERA - SHIFT", ECHO));
    }
    if !shifted && stage.era(here).screen.cleared() && !stage.cleared() {
        return Some(("THIS ERA IS CLEAR - SHIFT TO FINISH THE LINE", ECHO));
    }
    if shifted && !fired_after && stage.era(here).echoes_left() > 0 {
        return Some((
            if here == Plane::Siege {
                "ECHOES ARE WEAKER. THE TREBUCHET SHRUGS OFF WIND"
            } else {
                "ECHOES ARE WEAKER. SHELLS RIDE THE WIND"
            },
            ALERT,
        ));
    }
    None
}

/// A one-pixel border just inside `r`.
fn frame(c: &mut Canvas, r: hit::Rect, col: Rgba) {
    c.rect(r.x, r.y, r.w, 1, col);
    c.rect(r.x, r.y + r.h - 1, r.w, 1, col);
    c.rect(r.x, r.y, 1, r.h, col);
    c.rect(r.x + r.w - 1, r.y, 1, r.h, col);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sprite_the_game_names_exists() {
        let atlas = Atlas::embedded();
        for kind in StructureKind::ALL {
            for st in [
                StructState::Intact,
                StructState::Leaning,
                StructState::Collapsed,
            ] {
                for era in Plane::BOTH {
                    atlas.get(&sprite_name(kind, st, era));
                }
            }
        }
        for n in [
            "tank_base",
            "barrel_strip_5deg",
            "trebuchet",
            "trebuchet_arm_strip_5deg",
            "stone",
        ] {
            atlas.get(n);
        }
    }
}

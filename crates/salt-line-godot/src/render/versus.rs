//! Drawing a local versus match: the pre-match screen, the duel, and the
//! plates between turns. Engine-free like the rest of `render`.
//!
//! The duel is always drawn in the shared frame, left gun on the left. The
//! right gun's sprites are mirrored copies ([`Canvas::blit_flipped`]), and
//! its shots arrive already reflected into the shared frame by the rules
//! (`crate::versus`), so nothing here needs to know which frame a shot was
//! fired in, except the aim-help preview, which is flown like a real shot.
//!
//! What a player sees of the other's aim is kept to what the rules make
//! public: the readouts show only while the player whose turn it is aims,
//! aim help draws only that player's own last shot, and the handoff plate
//! dims the field until the next player says they are ready.

use super::*;
use crate::app::VersusStage as Stage;
use crate::versus::{self, Side};

impl Painter<'_> {
    /// The pre-match screen, under the title lettering `render_menu` drew:
    /// the rules in a line, each side's aim help, start and back.
    pub(super) fn render_versus_setup(&mut self, mut y: i32) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        self.centred(y, "LOCAL VERSUS - PASS THE PHONE", fg);
        y += 9;
        self.centred(
            y,
            &format!(
                "FIRST TO {} HITS TAKES A ROUND - BEST OF {}",
                versus::HITS_TO_WIN_ROUND,
                versus::ROUNDS_TO_WIN_MATCH * 2 - 1
            ),
            dim,
        );
        y += 13;
        let help = self.g.versus.help;
        let on = |b: bool| if b { "ON" } else { "OFF" };
        let rows = [
            (Action::Left, format!("LEFT AIM HELP: {}", on(help[0])), fg),
            (
                Action::Right,
                format!("RIGHT AIM HELP: {}", on(help[1])),
                fg,
            ),
            (Action::Confirm, "START".to_string(), alert),
            (Action::Back, "BACK".to_string(), dim),
        ];
        let labels: Vec<(String, &str)> = rows
            .iter()
            .map(|(action, text, _)| (self.g.input_label(*action), text.as_str()))
            .collect();
        for ((action, text, c), keyed) in rows.iter().zip(keyed_rows(&labels)) {
            y = self.menu_row(y, *action, &keyed, text, *c);
        }
        y += 4;
        self.centred(y, "AIM HELP: YOUR LAST SHOT AND A SHORT ARC", dim);
    }

    /// A frame of a match, back to front: the world, aim help, both guns,
    /// the shot, the HUD, then whichever plate the turn is behind.
    pub(super) fn render_versus(&mut self) {
        let g = self.g;
        let Some(vs) = g.versus.live.as_ref() else {
            return;
        };
        let pal = self.pal;
        let screen = vs.shown_screen();
        let duel = &vs.duel;
        let side = vs.side();
        let shake = vs
            .anim
            .as_ref()
            .filter(|a| a.done() && vs.hit.is_some())
            .map(|a| presentation::shake_offset(a.settled, 2).0)
            .unwrap_or(0);
        let view = View {
            cam_x: -shake,
            w: g.display.buffer_w,
            h: g.display.buffer_h,
        };

        sky::draw(
            self.canvas,
            &view,
            pal,
            sky::Phase::for_screen(0),
            duel.seed ^ duel.round as u64,
            0.0,
        );
        raster::draw_terrain(self.canvas, &view, &screen.field, pal);
        raster::draw_ground_detail(self.canvas, &view, screen, pal);

        let aiming = vs.stage == Stage::Aiming && !vs.paused;
        if aiming && g.versus.help[side.index()] {
            self.draw_aim_help(vs, side, &view);
        }

        for gun in Side::BOTH {
            let aim = if aiming && gun == side {
                g.play.angle_ticks
            } else {
                vs.aims[gun.index()].0
            };
            self.draw_gun(versus::muzzle_of(screen, gun), aim, gun, &view);
        }
        if matches!(vs.stage, Stage::Handoff | Stage::Aiming) {
            // A caret over the gun whose turn it is.
            let m = versus::muzzle_of(screen, side);
            let (x, y) = (view.sx(m.x), view.sy(m.y) - 12);
            let alert = pal.get(slot::ALERT);
            self.canvas.hline(x - 2, x + 2, y, alert);
            self.canvas.hline(x - 1, x + 1, y + 1, alert);
            self.canvas.put(x, y + 2, alert);
        }

        if let Some(anim) = vs.anim.as_ref() {
            let upto = (anim.t / DT).floor().max(0.0) as usize;
            for t in &anim.traces {
                if !t.path.is_empty() {
                    raster::draw_trajectory(self.canvas, &view, &t.path, upto, pal);
                }
            }
            let muzzle = anim
                .traces
                .first()
                .map_or(anim.before.muzzle(), |t| t.origin);
            self.draw_anim_effects(anim, muzzle, &view);
            if let Some(gun) = vs.hit {
                self.draw_hit(anim, versus::emplacement_box(screen, gun), &view);
            }
        }

        self.render_versus_hud(vs, aiming);

        match vs.stage {
            _ if vs.paused => self.render_pause(),
            Stage::Handoff => self.render_handoff(vs),
            Stage::Result => self.render_result(vs),
            Stage::Aiming | Stage::Flight => {}
        }
    }

    /// One gun: chassis and barrel at the pre-drawn 5° frame nearest
    /// `angle_ticks` (DESIGN §5.3), the right gun mirrored to face left.
    fn draw_gun(&mut self, muzzle: Vec2, angle_ticks: i32, gun: Side, view: &View) {
        let lo = Loadout::new();
        let frame = Shot::new(angle_ticks, 0).quantized(&lo).barrel_frame();
        let Some(s) = self.sprites else {
            return;
        };
        let (tank, pivot) = s.tank(&lo);
        let (tank, barrel) = (tank.cloned(), s.barrel(&lo).cloned());
        let (px, py) = (view.sx(muzzle.x), view.sy(muzzle.y));
        let strip = crate::raster::Rect {
            x: Sprites::barrel_frame_x(frame),
            y: 0,
            w: BARREL_FRAME,
            h: BARREL_FRAME,
        };
        // Barrel first, so the chassis overlaps its root. Mirrored, a pixel
        // `d` right of the pivot lands `d` left of it, which puts the
        // sprite's left edge at `pivot + pivot_offset - width + 1`.
        match gun {
            Side::Left => {
                if let Some(barrel) = barrel {
                    self.canvas.blit_region(
                        &barrel,
                        px - BARREL_PIVOT.0,
                        py - BARREL_PIVOT.1,
                        strip,
                    );
                }
                if let Some(tank) = tank {
                    self.canvas.blit(&tank, px - pivot.0, py - pivot.1);
                }
            }
            Side::Right => {
                if let Some(barrel) = barrel {
                    self.canvas.blit_region_flipped(
                        &barrel,
                        px + BARREL_PIVOT.0 - BARREL_FRAME + 1,
                        py - BARREL_PIVOT.1,
                        strip,
                    );
                }
                if let Some(tank) = tank {
                    self.canvas
                        .blit_flipped(&tank, px + pivot.0 - tank.w + 1, py - pivot.1);
                }
            }
        }
    }

    /// Aim help for `side`: its last landing mark and arc this round, and a
    /// short preview of the current aim in calm air, as the drag's Short
    /// arc hint draws it. The preview is flown in the shooter's frame and
    /// reflected, so it is exactly the start of the shot the player would
    /// fire, less the wind they still have to read.
    fn draw_aim_help(&mut self, vs: &crate::app::VersusPlay, side: Side, view: &View) {
        let pal = self.pal;
        let screen = &vs.duel.screen;
        if let Some(mark) = vs.duel.landing[side.index()] {
            raster::draw_chalk(self.canvas, view, &[mark], &screen.field, pal);
        }
        raster::draw_last_arc(self.canvas, view, &vs.arcs[side.index()], pal);

        let mut frame = versus::frame_for(screen, side);
        frame.wind = 0.0;
        let target = versus::target_in_frame(screen, side);
        let shot = Shot::new(self.g.play.angle_ticks, self.g.play.power_ticks);
        let traces = versus::fly(&frame, target, shot, &vs.duel.loadout, true);
        let Some(first) = traces.first() else {
            return;
        };
        let path: Vec<Vec2> = match side {
            Side::Left => first.path.clone(),
            Side::Right => versus::mirror_trace(first, screen.width).path,
        };
        let apex = path
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.y.total_cmp(&b.1.y))
            .map_or(0, |(i, _)| i);
        let window = (SHORT_ARC_SECONDS / DT) as usize;
        let upto = crate::aim::short_arc_len(apex, path.len(), window);
        raster::draw_trajectory(self.canvas, view, &path, upto, pal);
    }

    /// The gun that was hit blinks its outline while the hit is announced.
    fn draw_hit(&mut self, anim: &Anim, b: salt_line_sim::ballistics::Aabb, view: &View) {
        if !anim.done() || (anim.settled * 8.0) as i32 % 2 == 1 {
            return;
        }
        let alert = self.pal.get(slot::ALERT);
        let (x0, x1) = (view.sx(b.x0), view.sx(b.x1));
        let (y0, y1) = (view.sy(b.y1), view.sy(b.y0));
        self.canvas
            .rect_outline(x0 - 1, y0 - 1, x1 - x0 + 2, y1 - y0 + 2, alert);
        if anim.settled < presentation::COLLAPSE_TIME {
            let (w, h) = (self.canvas.w, self.canvas.h);
            self.canvas.rect_outline(1, 1, w - 2, h - 2, alert);
        }
    }

    /// Wind top left, the score top right, the round bottom right, the
    /// aim readouts bottom left while a player aims, the status line, and
    /// the touch controls.
    fn render_versus_hud(&mut self, vs: &crate::app::VersusPlay, aiming: bool) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        let w = self.canvas.w;
        let (ex, ey) = self.g.display.corner_pad;
        let duel = &vs.duel;

        // Wind only once the player has the phone: to one decimal, since a
        // duel's winds are all small (`versus::WIND_MAX`).
        if matches!(vs.stage, Stage::Aiming | Stage::Flight) {
            let wind = vs.shown_wind();
            self.draw_wind(&format!("{:.1}", wind.abs()), wind < 0.0);
        }

        // Hits this round, then rounds won, left to right as the guns stand.
        let (shown_hits, shown_rounds) = vs.shown_score();
        let hits = format!("{}-{}", shown_hits[0], shown_hits[1]);
        let rounds = format!("ROUNDS {}-{}", shown_rounds[0], shown_rounds[1]);
        let plate_w = (crate::font::big_width(&hits) as i32 + 32)
            .max(crate::font::small_width(&rounds) as i32 + 6);
        let px = w - plate_w - 3 - ex;
        self.canvas.plate(px, 3 + ey, plate_w, 21, pal);
        self.canvas.text_small(px + 3, 6 + ey, "HITS", dim);
        self.canvas.text_big(px + 26, 4 + ey, &hits, alert);
        self.canvas.text_small(px + 3, 15 + ey, &rounds, fg);

        let round = format!("ROUND {}", duel.round + 1);
        let rx = w - crate::font::small_width(&round) as i32 - 3 - ex;
        self.canvas
            .text_small(rx, self.canvas.h - 8 - ey, &round, fg);

        if aiming {
            let shot = Shot::new(self.g.play.angle_ticks, self.g.play.power_ticks)
                .quantized(&duel.loadout);
            let y = self.canvas.h - 8 - ey;
            let name = format!("{} GUN", vs.side().name());
            self.canvas
                .text_small(3 + ex, y, &format!("PWR {}", shot.power_ticks), fg);
            self.canvas.text_small(
                3 + ex,
                y - 7,
                &format!("ANG {:.1}", shot.angle_degrees()),
                fg,
            );
            self.canvas.text_small(3 + ex, y - 14, &name, alert);
        }

        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
            let x = (w - crate::font::small_width(&text) as i32) / 2;
            self.canvas.text_small(x, 28 + ey, &text, alert);
        }

        if self.g.input.touch_active && !vs.paused {
            if aiming {
                self.draw_play_controls(&duel.loadout);
            } else if vs.stage == Stage::Flight {
                self.draw_touch_button((w - 20) / 2, 3 + ey, 20, 13, "II", Action::Back);
            }
        }
    }

    /// Between turns: the field dimmed and a plate saying whose turn it
    /// is, until they say they are ready.
    fn render_handoff(&mut self, vs: &crate::app::VersusPlay) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        self.dither_dim();
        let (w, h) = (self.canvas.w, self.canvas.h);
        let duel = &vs.duel;
        let plated = self.plated();
        let ph = if plated { 74 } else { 62 };
        let top = (h - ph) / 2;
        self.canvas.plate((w - 200) / 2, top, 200, ph, pal);
        let mut y = top + 7;
        let opening = duel.turn == 0;
        let heading = if opening {
            format!("ROUND {}", duel.round + 1)
        } else {
            format!("HITS {}-{}", duel.hits[0], duel.hits[1])
        };
        self.centred(y, &heading, dim);
        y += 11;
        self.centred(y, &format!("{} GUN", vs.side().name()), alert);
        y += 10;
        self.centred(y, "TAKE THE PHONE - NO PEEKING", fg);
        y += 13;
        if plated {
            self.menu_row(y + 4, Action::Confirm, "", "READY", fg);
            // The pause button, so a touchscreen can leave between turns.
            let ey = self.g.display.corner_pad.1;
            self.draw_touch_button((w - 20) / 2, 3 + ey, 20, 13, "II", Action::Back);
        } else {
            let ready = self.g.input_label(Action::Confirm);
            self.centred(y, &format!("{ready}  READY"), fg);
        }
    }

    /// A round or the match is won: who, the rounds so far, and what next.
    fn render_result(&mut self, vs: &crate::app::VersusPlay) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        self.dither_dim();
        let (w, h) = (self.canvas.w, self.canvas.h);
        let duel = &vs.duel;
        let (winner, over) = match duel.phase {
            versus::Phase::RoundOver(s) => (s, false),
            versus::Phase::MatchOver(s) => (s, true),
            versus::Phase::Aiming => return,
        };
        let plated = self.plated();
        let ph = if plated { 84 } else { 62 };
        let top = (h - ph) / 2;
        self.canvas.plate((w - 200) / 2, top, 200, ph, pal);
        let mut y = top + 7;
        let title = if over {
            format!("{} GUN WINS THE MATCH", winner.name())
        } else {
            format!("{} GUN WINS ROUND {}", winner.name(), duel.round + 1)
        };
        self.centred(y, &title, alert);
        y += 11;
        self.centred(
            y,
            &format!("ROUNDS {}-{}", duel.rounds[0], duel.rounds[1]),
            fg,
        );
        y += 13;
        let next = if over { "REMATCH" } else { "NEXT ROUND" };
        let rows = [(Action::Confirm, next), (Action::Replay, "TITLE")];
        let labels: Vec<(String, &str)> = rows
            .iter()
            .map(|&(action, text)| (self.g.input_label(action), text))
            .collect();
        for (&(action, text), keyed) in rows.iter().zip(keyed_rows(&labels)) {
            let c = if action == Action::Confirm { fg } else { dim };
            y = self.menu_row(if plated { y + 4 } else { y }, action, &keyed, text, c);
        }
    }
}

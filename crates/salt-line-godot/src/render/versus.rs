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
//!
//! # Reading it from across the sofa
//!
//! The mode is played by a parent and a young child, so who is who and how
//! it is going have to read without reading words:
//!
//! - **A colour each.** The left gun is the alert amber, the right gun the
//!   pale sky blue ([`side_slot`]): far apart in hue, and still apart for
//!   the commonest colour blindness. Whatever names a side is drawn in its
//!   colour: the marker over each gun, the score pips, the handoff and
//!   result plates, the HIT callout and the hit flash. DESIGN §5.2's
//!   sixteen colours stay sixteen; these are two existing slots given a
//!   second job here, so the usual dim text (also sky blue) moves to
//!   [`DIM`] on every versus screen.
//! - **Pips, not digits.** Hits this round are a row of big dots per side,
//!   filled as they land; rounds won are a row of small ones under them.
//! - **Big words.** HIT and MISS, whose turn it is and who won are drawn in
//!   the small face at two to four times size ([`Canvas::text_small_scaled`]),
//!   always on a plate: the right player's blue is close to the sky's.

use super::*;
use crate::app::VersusStage as Stage;
use crate::versus::{self, Side};

/// Secondary text on versus screens. The game's usual dim, `SKY_LO`, is the
/// right player's colour here.
const DIM: usize = slot::ROCK;

/// The palette slot that stands for `side` wherever versus names it.
pub(super) fn side_slot(side: Side) -> usize {
    match side {
        Side::Left => slot::ALERT,
        Side::Right => slot::SKY_LO,
    }
}

/// A hit this round, and a hit still to make: 7 by 7, so a pip is about as
/// wide as a fingertip on a phone.
const PIP: [&str; 7] = [
    "0011100", "0111110", "1111111", "1111111", "1111111", "0111110", "0011100",
];
const PIP_EMPTY: [&str; 7] = [
    "0011100", "0100010", "1000001", "1000001", "1000001", "0100010", "0011100",
];
/// A round won, and a round still to win.
const ROUND_PIP: [&str; 5] = ["01110", "11111", "11111", "11111", "01110"];
const ROUND_PIP_EMPTY: [&str; 5] = ["01110", "10001", "10001", "10001", "01110"];
/// Pixels between pips in a row.
const PIP_GAP: i32 = 2;
/// Width of one side's hit pips.
const HIT_ROW_W: i32 = versus::HITS_TO_WIN_ROUND as i32 * (7 + PIP_GAP) - PIP_GAP;
/// The gap between the two sides' pips.
const PIP_MIDDLE: i32 = 9;
/// Width of both sides' pips, middle gap included: the score plate's
/// inside, and the row the handoff and result plates show.
const SCORE_W: i32 = 2 * HIT_ROW_W + PIP_MIDDLE;
/// Height of the hit pips over the round pips.
const SCORE_H: i32 = 7 + 3 + 5;

/// The aim-help preview for `side` firing `aim` (angle ticks, power
/// ticks): the shell's path in the shared frame, flown in calm air in the
/// shooter's frame and reflected back, and how much of it the Short arc
/// shows. The aim is put on the instruments' grid first, as `Match::fire`
/// puts the real shot, so the preview is the start of the shot that fires.
pub(super) fn aim_preview(duel: &versus::Match, side: Side, aim: (i32, i32)) -> (Vec<Vec2>, usize) {
    let screen = &duel.screen;
    let mut frame = versus::frame_for(screen, side);
    frame.wind = 0.0;
    let target = versus::target_in_frame(screen, side);
    let shot = Shot::new(aim.0, aim.1).quantized(&duel.loadout);
    let traces = versus::fly(&frame, target, shot, &duel.loadout, true);
    let Some(first) = traces.first() else {
        return (Vec::new(), 0);
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
    (path, upto)
}

impl Painter<'_> {
    /// The pre-match screen, under the title lettering `render_menu` drew:
    /// the rules in a line, each side's aim help in its colour, start and
    /// back.
    pub(super) fn render_versus_setup(&mut self, mut y: i32) {
        let pal = self.pal;
        let (fg, dim) = (pal.get(slot::UI_FG), pal.get(DIM));
        let colour = |side| pal.get(side_slot(side));
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
            (
                Action::Left,
                format!("LEFT AIM HELP: {}", on(help[0])),
                colour(Side::Left),
            ),
            (
                Action::Right,
                format!("RIGHT AIM HELP: {}", on(help[1])),
                colour(Side::Right),
            ),
            (Action::Confirm, "START".to_string(), fg),
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
        let guns = Side::BOTH.map(|gun| versus::muzzle_of(screen, gun).x);
        raster::draw_ground_detail_around(self.canvas, &view, screen, &guns, pal);

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
        // A marker over each gun in its player's colour; a big one over the
        // gun whose turn it is.
        let turn = matches!(vs.stage, Stage::Handoff | Stage::Aiming);
        for gun in Side::BOTH {
            let m = versus::muzzle_of(screen, gun);
            self.draw_gun_marker(view.sx(m.x), view.sy(m.y), gun, turn && gun == side);
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
                let scorer = side_slot(gun.other());
                self.draw_hit(anim, versus::emplacement_box(screen, gun), scorer, &view);
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

        let aim = (self.g.play.angle_ticks, self.g.play.power_ticks);
        let (path, upto) = aim_preview(&vs.duel, side, aim);
        raster::draw_trajectory(self.canvas, view, &path, upto, pal);
    }

    /// The gun that was hit blinks its outline while the hit is announced,
    /// and the screen's edge flashes, both in the scorer's colour (`scorer`,
    /// a palette slot).
    fn draw_hit(
        &mut self,
        anim: &Anim,
        b: salt_line_sim::ballistics::Aabb,
        scorer: usize,
        view: &View,
    ) {
        if !anim.done() || (anim.settled * 8.0) as i32 % 2 == 1 {
            return;
        }
        let alert = self.pal.get(scorer);
        let (x0, x1) = (view.sx(b.x0), view.sx(b.x1));
        let (y0, y1) = (view.sy(b.y1), view.sy(b.y0));
        self.canvas
            .rect_outline(x0 - 1, y0 - 1, x1 - x0 + 2, y1 - y0 + 2, alert);
        if anim.settled < presentation::COLLAPSE_TIME {
            let (w, h) = (self.canvas.w, self.canvas.h);
            self.canvas.rect_outline(1, 1, w - 2, h - 2, alert);
        }
    }

    /// Wind top left, the score pips top right, the round bottom right,
    /// the aim readouts bottom left while a player aims, the HIT or MISS
    /// callout, the status line, and the touch controls.
    fn render_versus_hud(&mut self, vs: &crate::app::VersusPlay, aiming: bool) {
        let pal = self.pal;
        let fg = pal.get(slot::UI_FG);
        let w = self.canvas.w;
        let (ex, ey) = self.g.display.corner_pad;
        let duel = &vs.duel;

        // Wind only once the player has the phone: to one decimal, since a
        // duel's winds are all small (`versus::WIND_MAX`).
        if matches!(vs.stage, Stage::Aiming | Stage::Flight) {
            let wind = vs.shown_wind();
            self.draw_wind(&format!("{:.1}", wind.abs()), wind < 0.0, slot::UI_FG);
        }

        // Hits this round over rounds won, left to right as the guns stand.
        let (shown_hits, shown_rounds) = vs.shown_score();
        let plate_w = SCORE_W + 8;
        let px = w - plate_w - 3 - ex;
        self.canvas.plate(px, 3 + ey, plate_w, SCORE_H + 8, pal);
        self.draw_score(px + 4, 7 + ey, shown_hits, shown_rounds);

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
            let colour = pal.get(side_slot(vs.side()));
            self.canvas.text_small(3 + ex, y - 14, &name, colour);
        }

        // The landed shot, called out large under the score: HIT in the
        // shooter's colour with their name under it, or a plain MISS.
        // It sits on a plate: the right player's sky blue is close to the
        // sky itself.
        let callout = vs.callout();
        if let Some((shooter, hit)) = callout {
            let top = 28 + ey;
            let by = format!("{} GUN SCORES", shooter.name());
            let word = if hit { "HIT!" } else { "MISS" };
            let word_w = crate::font::small_width(word) as i32 * 4;
            let inner_w = if hit {
                word_w.max(crate::font::small_width(&by) as i32)
            } else {
                word_w
            };
            let (pw, ph) = (inner_w + 14, if hit { 41 } else { 32 });
            let px = (w - pw) / 2;
            self.canvas.plate(px, top, pw, ph, pal);
            if hit {
                self.side_border(px, top, pw, ph, shooter);
                self.shout(top + 6, word, side_slot(shooter), 4);
                self.shout(top + 30, &by, side_slot(shooter), 1);
            } else {
                self.shout(top + 6, word, slot::UI_FG, 4);
            }
        }

        if self.g.status_time > 0.0 && callout.is_none() {
            let text = self.g.status.clone();
            let x = (w - crate::font::small_width(&text) as i32) / 2;
            self.canvas.text_small(x, 28 + ey, &text, fg);
        }

        if self.g.input.touch_active && !vs.paused {
            if aiming {
                self.draw_play_controls(&duel.loadout);
            } else if vs.stage == Stage::Flight {
                self.draw_touch_button((w - 20) / 2, 3 + ey, 20, 13, "II", Action::Back);
            }
        }
    }

    /// Both sides' score at `x`, `y` ([`SCORE_W`] by [`SCORE_H`]): a big
    /// pip per hit a round takes, filled for each hit made, over a small
    /// pip per round the match takes, filled for each round won. Each side
    /// in its colour, left on the left.
    fn draw_score(&mut self, x: i32, y: i32, hits: [u32; 2], rounds: [u32; 2]) {
        let pal = self.pal;
        let round_row_w = versus::ROUNDS_TO_WIN_MATCH as i32 * (5 + PIP_GAP) - PIP_GAP;
        for side in Side::BOTH {
            let i = side.index();
            let colour = [pal.get(side_slot(side))];
            let x0 = x + i as i32 * (HIT_ROW_W + PIP_MIDDLE);
            for k in 0..versus::HITS_TO_WIN_ROUND {
                let pip: &[&str] = if k < hits[i] { &PIP } else { &PIP_EMPTY };
                let px = x0 + k as i32 * (7 + PIP_GAP);
                self.canvas.icon(px, y, pip, &colour, false);
            }
            let rx = x0 + (HIT_ROW_W - round_row_w) / 2;
            for k in 0..versus::ROUNDS_TO_WIN_MATCH {
                let pip: &[&str] = if k < rounds[i] {
                    &ROUND_PIP
                } else {
                    &ROUND_PIP_EMPTY
                };
                let px = rx + k as i32 * (5 + PIP_GAP);
                self.canvas.icon(px, y + 10, pip, &colour, false);
            }
        }
        let mid = x + HIT_ROW_W + PIP_MIDDLE / 2;
        self.canvas
            .vline(mid, y + 1, y + SCORE_H - 2, pal.get(slot::SHADOW));
    }

    /// `text` centred at `y` in the small face at `scale` (one block per
    /// glyph pixel), in palette slot `c`. It is `5 * scale` tall. Always
    /// over a plate, where both players' colours read.
    fn shout(&mut self, y: i32, text: &str, c: usize, scale: i32) {
        let x = (self.canvas.w - crate::font::small_width(text) as i32 * scale) / 2;
        let c = self.pal.get(c);
        self.canvas.text_small_scaled(x, y, text, c, scale);
    }

    /// The marker over a gun, its bottom `MARKER_LIFT` pixels above the
    /// pivot at `x`, `y`: a small tag with an arrow in the player's colour,
    /// or a big one over the gun whose turn it is (`turn`). A tag rather
    /// than a bare arrow because it sits over sky, which the right player's
    /// blue would vanish into.
    fn draw_gun_marker(&mut self, x: i32, y: i32, gun: Side, turn: bool) {
        const BIG: [&str; 5] = [
            "111111111",
            "011111110",
            "001111100",
            "000111000",
            "000010000",
        ];
        const SMALL: [&str; 3] = ["11111", "01110", "00100"];
        /// Clear of the barrel at any elevation (16 pixels at 90 degrees).
        const MARKER_LIFT: i32 = 19;
        let rows: &[&str] = if turn { &BIG } else { &SMALL };
        let (w, h) = (rows[0].len() as i32 + 4, rows.len() as i32 + 4);
        let (left, top) = (x - w / 2, y - MARKER_LIFT - h + 1);
        self.canvas.plate(left, top, w, h, self.pal);
        let colour = [self.pal.get(side_slot(gun))];
        self.canvas.icon(left + 2, top + 2, rows, &colour, false);
    }

    /// A plate's border in a side's colour, two pixels thick, over the
    /// plate `canvas.plate` drew at the same rectangle.
    fn side_border(&mut self, x: i32, y: i32, w: i32, h: i32, side: Side) {
        let c = self.pal.get(side_slot(side));
        self.canvas.rect_outline(x, y, w, h, c);
        self.canvas.rect_outline(x + 1, y + 1, w - 2, h - 2, c);
    }

    /// Between turns: the field dimmed and a plate in the colour of the
    /// player whose turn it is, saying so in big letters, until they say
    /// they are ready.
    fn render_handoff(&mut self, vs: &crate::app::VersusPlay) {
        let pal = self.pal;
        let (fg, dim) = (pal.get(slot::UI_FG), pal.get(DIM));
        self.dither_dim();
        let (w, h) = (self.canvas.w, self.canvas.h);
        let duel = &vs.duel;
        let side = vs.side();
        let plated = self.plated();
        // A new round says which; later turns show the score instead.
        let opening = duel.turn == 0;
        let heading_h = if opening { 5 } else { SCORE_H };
        let body = 7 + heading_h + 6 + 10 + 6 + 5 + 13;
        let ph = body + if plated { 20 } else { 12 };
        let (px, top) = ((w - 200) / 2, (h - ph) / 2);
        self.canvas.plate(px, top, 200, ph, pal);
        self.side_border(px, top, 200, ph, side);
        let mut y = top + 7;
        if opening {
            self.centred(y, &format!("ROUND {}", duel.round + 1), dim);
        } else {
            self.draw_score((w - SCORE_W) / 2, y, duel.hits, duel.rounds);
        }
        y += heading_h + 6;
        self.shout(y, &format!("{} GUN", side.name()), side_slot(side), 2);
        y += 10 + 6;
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

    /// A round or the match is won: who, in big letters in their colour,
    /// the score as it ended, and what next.
    fn render_result(&mut self, vs: &crate::app::VersusPlay) {
        let pal = self.pal;
        let (fg, dim) = (pal.get(slot::UI_FG), pal.get(DIM));
        self.dither_dim();
        let (w, h) = (self.canvas.w, self.canvas.h);
        let duel = &vs.duel;
        let (winner, over) = match duel.phase {
            versus::Phase::RoundOver(s) => (s, false),
            versus::Phase::MatchOver(s) => (s, true),
            versus::Phase::Aiming => return,
        };
        let plated = self.plated();
        let body = 7 + 10 + 5 + 5 + 7 + SCORE_H + 8;
        let ph = body + if plated { 40 } else { 20 };
        let (px, top) = ((w - 200) / 2, (h - ph) / 2);
        self.canvas.plate(px, top, 200, ph, pal);
        self.side_border(px, top, 200, ph, winner);
        let mut y = top + 7;
        let title = format!("{} GUN WINS", winner.name());
        self.shout(y, &title, side_slot(winner), 2);
        y += 10 + 5;
        let what = if over {
            "THE MATCH!".to_string()
        } else {
            format!("ROUND {}", duel.round + 1)
        };
        self.centred(y, &what, fg);
        y += 5 + 7;
        self.draw_score((w - SCORE_W) / 2, y, duel.hits, duel.rounds);
        y += SCORE_H + 8;
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

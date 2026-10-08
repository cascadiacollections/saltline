//! Drawing the REPLAYS list and the bar over a replay being watched
//! (DESIGN §9.4, `app/watch.rs`). Engine-free like the rest of `render`.
//!
//! # The list
//!
//! ```text
//!                         SALT LINE
//!                    REPLAYS   PAGE 1 / 2
//!               (a note: why a row cannot be watched)
//!       ┌──────────────────────────────────────────┐
//!       │ OCT 2  RUN  14 SCREENS  SEED 7654321     │   [★]  starred: first
//!       │ OCT 8  AI EASY  YOU WON 2-1  MESA        │   [☆]  a row: tap to watch
//!       │ OCT 8  RUN  9 SCREENS  SEED 1234567      │   [☆]  its star: tap to keep
//!       │ OCT 7  2 PLAYERS  LEFT WON 2-0  OLD      │   [☆]  dimmed: another version
//!       └──────────────────────────────────────────┘
//!                   <        BACK        >
//! ```
//!
//! The starred first, then the rest, each newest first; a line each, in
//! the words a child can read out (DESIGN §11.4): when, against whom, who
//! won. A row from another build's sim is dimmed and says OLD (or NEWER);
//! it stays listed, and choosing it says why it cannot be watched (§9.3).
//!
//! On a touchscreen each row has its own star button, a filled star when
//! starred and a hollow one when not, in a column of its own to the right
//! of the rows, apart from them by [`STAR_GAP`], so a child tapping a row to
//! watch it does not star it, nor the other way about. With keys or a
//! controller the chosen row is starred by [`Action::Star`], and a starred
//! row has a star in front of it.
//!
//! # The watch bar
//!
//! ```text
//!                            [EXIT]
//!   ANG 45                                               SCREEN 4
//!   PWR 60      [PAUSE] [2X] [NEXT SHOT] [NEXT ROUND]
//! ```
//!
//! Bottom centre, where the demo's banner sits: clear of the readouts in
//! the bottom corners, the draft cards and the duel's plates. On a
//! touchscreen EXIT is apart from the rest, top centre where a game's II
//! is, so a tap meant for PAUSE never leaves the replay. On a
//! touchscreen it is a row of buttons as tall as the aim buttons (18
//! rows); with keys or a controller a plate saying what each key does. A
//! replay draws no aim controls of its own, and no prompts.

use super::*;
use crate::input::Wb;

/// The watch bar's buttons: as tall as the aim buttons.
const BAR_H: i32 = 18;
/// Pixels between the bar's buttons.
const BAR_GAP: i32 = 3;
/// A list row's star button: as wide as a page button (22 by 13), as tall
/// as a row's touch target (15, a little more than its 13-row plate).
const STAR_W: i32 = 22;
const STAR_H: i32 = 15;
/// Pixels between a row's plate and its star button: about a fingertip at
/// a phone's scale, so a tap on one is not on the other. Never less: where
/// the column would run into the screen's edge (a narrow buffer, a long
/// row, a cutout's pad), the rows move left instead.
const STAR_GAP: i32 = 12;

/// The star drawn on buttons and before a starred row, 7 by 7: filled, and
/// its outline for a star not set.
const STAR_FILLED: [&str; 7] = [
    "...#...", "..###..", "#######", ".#####.", "..###..", ".##.##.", ".#...#.",
];
const STAR_HOLLOW: [&str; 7] = [
    "...#...", "..#.#..", "##...##", ".#...#.", "..###..", ".##.##.", ".#...#.",
];
/// The star's size.
pub(super) const STAR_PX: i32 = 7;

/// A list row's words, and whether it can be watched: a row from another
/// build's sim says OLD or NEWER.
fn row_text(record: &crate::history::Record) -> (String, bool) {
    let mut text = record.summary();
    match record.version() {
        Ok(()) => (text, true),
        Err(why) => {
            text.push_str(match why {
                crate::history::Refused::NewerVersion => "  NEWER",
                _ => "  OLD",
            });
            (text, false)
        }
    }
}

impl Painter<'_> {
    /// A star, its top left at `(x, y)`: filled, or hollow.
    pub(super) fn star_icon(&mut self, x: i32, y: i32, filled: bool, c: crate::palette::Rgba) {
        let rows = if filled { STAR_FILLED } else { STAR_HOLLOW };
        for (dy, row) in rows.iter().enumerate() {
            for (dx, px) in row.bytes().enumerate() {
                if px == b'#' {
                    self.canvas.put(x + dx as i32, y + dy as i32, c);
                }
            }
        }
    }

    /// A touch button showing a star rather than a word.
    fn star_button(&mut self, x: i32, y: i32, w: i32, h: i32, filled: bool, action: Action) {
        let pal = self.pal;
        self.draw_touch_button(x, y, w, h, "", action);
        let c = if filled || self.lit(action) {
            pal.get(slot::ALERT)
        } else {
            pal.get(slot::UI_FG)
        };
        self.star_icon(x + (w - STAR_PX) / 2, y + (h - STAR_PX) / 2, filled, c);
    }

    /// The REPLAYS list (`Mode::Replays`), below the menus' title at `y`.
    pub(super) fn render_replays(&mut self, mut y: i32) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        let g = self.g;
        let records: Vec<&crate::history::Record> = g.history.file.listed().collect();
        let rows = g.replay_rows();
        let pages = records.len().div_ceil(rows).max(1);
        let focus = g.history.focus.min(records.len().saturating_sub(1));
        let page = focus / rows;
        let heading = if pages > 1 {
            format!("REPLAYS   PAGE {} / {pages}", page + 1)
        } else {
            "REPLAYS".to_string()
        };
        self.centred(y, &heading, dim);
        y += 8;
        // One line under the heading: a choice refused, or lines lost.
        if g.status_time > 0.0 {
            let text = g.status.clone();
            self.centred(y, &text, alert);
        } else if g.history.file.damaged > 0 {
            let text = match g.history.file.damaged {
                1 => "1 REPLAY COULD NOT BE READ".to_string(),
                n => format!("{n} REPLAYS COULD NOT BE READ"),
            };
            self.centred(y, &text, dim);
        } else if g.history.file.newer > 0 {
            let text = match g.history.file.newer {
                1 => "1 REPLAY FROM A NEWER VERSION".to_string(),
                n => format!("{n} REPLAYS FROM A NEWER VERSION"),
            };
            self.centred(y, &text, dim);
        }
        y += 12;
        let plated = self.plated();
        if records.is_empty() {
            self.centred(y, "NOTHING TO WATCH YET", fg);
            y += 10;
            self.centred(y, "FINISH A RUN OR WIN A MATCH", dim);
            y += 8;
            self.centred(y, "TO WATCH IT HERE", dim);
            y += 16;
        } else {
            // The star buttons' column: right of the page's widest row.
            let widest = records
                .iter()
                .skip(page * rows)
                .take(rows)
                .map(|r| crate::font::small_width(&row_text(r).0) as i32 + 20)
                .max()
                .unwrap_or(0);
            let want = (self.canvas.w + widest) / 2 + STAR_GAP;
            let star_x = want.min(self.canvas.w - STAR_W - 3 - g.display.corner_pad.0);
            // The rows move left by what the column could not.
            let shift = want - star_x;
            for (i, record) in records.iter().enumerate().skip(page * rows).take(rows) {
                let (text, watchable) = row_text(record);
                let c = if watchable { fg } else { dim };
                let row = (i - page * rows) as u8;
                if plated {
                    let star = Action::Wb(Wb::RowStar(row));
                    self.star_button(star_x, y - 5, STAR_W, STAR_H, record.starred, star);
                    y = self.menu_row_at(y, Action::Wb(Wb::Row(row)), &text, c, -shift);
                } else {
                    let (marker, c) = if i == focus {
                        (">  ", alert)
                    } else {
                        ("   ", c)
                    };
                    // Room for a star before the text, starred or not, so
                    // the rows line up.
                    let line = format!("{marker}   {text}   ");
                    let x = ((self.canvas.w - crate::font::small_width(&line) as i32) / 2).max(1);
                    self.canvas.text_small(x, y, &line, c);
                    if record.starred {
                        let sx = x + crate::font::small_width(marker) as i32 + 1;
                        self.star_icon(sx, y - 1, true, alert);
                    }
                    y += 8;
                }
            }
            // The page keeps its height, so BACK stays put as pages turn.
            let shown = records.len().saturating_sub(page * rows).min(rows);
            y += (rows - shown) as i32 * if plated { 16 } else { 8 };
        }
        if plated {
            if pages > 1 {
                let w = self.canvas.w;
                self.draw_touch_button(w / 2 - 64, y - 4, 22, 13, "<", Action::Wb(Wb::PagePrev));
                self.draw_touch_button(w / 2 + 42, y - 4, 22, 13, ">", Action::Wb(Wb::PageNext));
            }
            self.menu_row(y, Action::Back, "", "BACK", fg);
        } else {
            y += 4;
            let label = |a| g.input_label(a);
            let first = format!(
                "{} / {}  CHOOSE   {}  WATCH   {}  STAR",
                label(Action::Up),
                label(Action::Down),
                label(Action::Confirm),
                label(Action::Star)
            );
            let mut rest = String::new();
            if pages > 1 {
                rest.push_str(&format!(
                    "{} / {}  PAGE   ",
                    label(Action::Left),
                    label(Action::Right)
                ));
            }
            rest.push_str(&format!("{}  BACK", label(Action::Back)));
            // One line if it fits the narrowest screen, else two.
            let one = format!("{first}   {rest}");
            if crate::font::small_width(&one) as i32 <= self.canvas.w - 8 {
                self.centred(y, &one, fg);
            } else {
                self.centred(y, &first, fg);
                self.centred(y + 8, &rest, fg);
            }
        }
    }

    /// The bar over a replay being watched, bottom centre, and the run's
    /// result once a run has played to its end.
    pub(super) fn render_watch_bar(&mut self) {
        let Some(w) = self.g.watch.as_ref() else {
            return;
        };
        let (paused, ended, rate, duel) = (w.paused, w.ended, w.rate(), w.duel());
        let starred = self.g.history.file.is_starred(&w.record);
        let refused = w.star_refused;
        let pal = self.pal;
        // The secondary tone: `SKY_LO`, but `ROCK` over a duel, where
        // `SKY_LO` is the right player's colour (DESIGN §11.4).
        let dim = if duel { slot::ROCK } else { slot::SKY_LO };
        let (fg, dim) = (pal.get(slot::UI_FG), pal.get(dim));
        let (cw, ch) = (self.canvas.w, self.canvas.h);
        let ey = self.g.display.corner_pad.1;
        if ended && !duel {
            // A match's end is its own result plate; a run's is this.
            if let Some(run) = self.g.play.run.as_ref() {
                let screens = run.score().0;
                let lines = [
                    "OUT OF SHELLS".to_string(),
                    format!("{screens} SCREENS CLEARED"),
                ];
                let pw = lines
                    .iter()
                    .map(|l| crate::font::small_width(l) as i32)
                    .max()
                    .unwrap_or(0)
                    + 16;
                let (px, py) = ((cw - pw) / 2, 40);
                self.canvas.plate(px, py, pw, 25, pal);
                self.centred(py + 6, &lines[0], pal.get(slot::ALERT));
                self.centred(py + 15, &lines[1], fg);
            }
        }
        let next_round = if duel { "NEXT ROUND" } else { "NEXT SCREEN" };
        let play = if ended {
            "AGAIN"
        } else if paused {
            "PLAY"
        } else {
            "PAUSE"
        };
        let speed = format!("{rate}X");
        if self.g.input.touch_active {
            // EXIT on its own, top centre where a game's II is, well away
            // from PAUSE: a child tapping the bar should not leave by it.
            self.draw_touch_button((cw - 32) / 2, 3 + ey, 32, 13, "EXIT", Action::Wb(Wb::Exit));
            let star = if starred { "STARRED" } else { "STAR" };
            let mut buttons: Vec<(&str, Wb)> = vec![(play, Wb::Pause), (&speed, Wb::Speed)];
            if !ended {
                buttons.push(("NEXT SHOT", Wb::NextShot));
                buttons.push((next_round, Wb::NextRound));
            } else {
                buttons.push((star, Wb::Star));
            }
            // The star's button has its star drawn before the word.
            let icon = |b: Wb| if b == Wb::Star { STAR_PX + 3 } else { 0 };
            let width =
                |t: &str, b: Wb| (crate::font::small_width(t) as i32 + 10 + icon(b)).max(22);
            let total: i32 = buttons.iter().map(|&(t, b)| width(t, b)).sum::<i32>()
                + BAR_GAP * (buttons.len() as i32 - 1);
            let mut x = (cw - total) / 2;
            let y = ch - BAR_H - 3 - ey;
            for (text, b) in buttons {
                let bw = width(text, b);
                if b == Wb::Star {
                    self.draw_touch_button(x, y, bw, BAR_H, "", Action::Wb(b));
                    let c = if starred || self.lit(Action::Wb(b)) {
                        pal.get(slot::ALERT)
                    } else {
                        fg
                    };
                    let tw = crate::font::small_width(text) as i32;
                    let sx = x + (bw - tw - STAR_PX - 3) / 2;
                    self.star_icon(sx, y + (BAR_H - STAR_PX) / 2, starred, c);
                    let ty = y + (BAR_H - crate::font::SMALL_H as i32) / 2;
                    self.canvas.text_small(sx + STAR_PX + 3, ty, text, c);
                } else {
                    self.draw_touch_button(x, y, bw, BAR_H, text, Action::Wb(b));
                }
                x += bw + BAR_GAP;
            }
            if let Some(why) = refused.filter(|_| ended) {
                let note = why.note();
                let tw = crate::font::small_width(&note) as i32 + 12;
                let ny = y - 15;
                self.canvas.plate((cw - tw) / 2, ny, tw, 11, pal);
                self.centred(ny + 3, &note, pal.get(slot::ALERT));
            }
            return;
        }
        let label = |a| self.g.input_label(a);
        let state = if let Some(why) = refused.filter(|_| ended) {
            why.note()
        } else if ended && starred {
            "END OF REPLAY  -  STARRED".to_string()
        } else if ended {
            "END OF REPLAY".to_string()
        } else if paused {
            "REPLAY  PAUSED".to_string()
        } else {
            format!("REPLAY  {speed}")
        };
        let lines = [
            (state, fg),
            (
                format!(
                    "{}  {}   {}  SPEED   {}  EXIT",
                    label(Action::Confirm),
                    if ended { "AGAIN" } else { "PAUSE" },
                    label(Action::Up),
                    label(Action::Back)
                ),
                fg,
            ),
            (
                if ended {
                    format!(
                        "{}  {}",
                        label(Action::Star),
                        if starred {
                            "TAKE STAR OFF"
                        } else {
                            "STAR - KEEP THIS ONE"
                        }
                    )
                } else {
                    format!(
                        "{}  NEXT SHOT   {}  {next_round}",
                        label(Action::Right),
                        label(Action::Down)
                    )
                },
                dim,
            ),
        ];
        let pw = lines
            .iter()
            .map(|(l, _)| crate::font::small_width(l) as i32)
            .max()
            .unwrap_or(0)
            + 12;
        let ph = 3 * 8 + 3;
        let (px, py) = ((cw - pw) / 2, ch - ph - 2 - ey);
        self.canvas.plate(px, py, pw, ph, pal);
        for (i, (text, c)) in lines.iter().enumerate() {
            self.centred(py + 4 + 8 * i as i32, text, *c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{finished_match, finished_run};
    use crate::history::{Ground, Record};
    use crate::input::{Event, Intents, Key};
    use crate::opponent::Level;
    use crate::save_format::Save;

    const WINDOW: (i32, i32) = (1280, 720);

    fn idle() -> Intents {
        Intents {
            window: WINDOW,
            today: 20_734,
            ..Intents::default()
        }
    }

    /// A game with three things to watch, newest first: a won match
    /// against the AI, a run, and a run from an older sim. `phone` lays it
    /// out as a phone (touch controls, the rounded corners' pad).
    fn game(phone: bool) -> Game {
        let mut g = Game::new(phone, Save::default());
        g.fit_display(WINDOW, (0, 0));
        let old = format!(
            "run 1 20700 {} 42 320 0 3 0 41 v9 @0 Q7 900:600\n",
            salt_line_sim::SIM_EPOCH - 1
        );
        g.set_history(Some(&format!("history 1\n{old}")));
        let run = finished_run(9, 320, 5);
        g.history.file.push(Record::run(20733, &run, false));
        let (saved, duel) = finished_match(31, Some(Level::Easy));
        g.history
            .file
            .push(Record::duel(20734, &saved, &duel, Ground::Generated).unwrap());
        g
    }

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

    fn press(g: &mut Game, key: Key) {
        g.tick(
            1.0 / 60.0,
            &Intents {
                events: vec![Event::key(key)],
                ..idle()
            },
        );
        g.tick(1.0 / 60.0, &idle());
    }

    /// No two buttons overlap, and each is wholly on the screen.
    fn laid_out(g: &Game, buttons: &[Button]) {
        for (i, a) in buttons.iter().enumerate() {
            assert!(
                a.rect.x >= 0
                    && a.rect.y >= 0
                    && a.rect.x + a.rect.w <= g.display.buffer_w
                    && a.rect.y + a.rect.h <= g.display.buffer_h,
                "{:?} off the screen",
                a.action
            );
            for b in &buttons[i + 1..] {
                let apart = a.rect.x + a.rect.w <= b.rect.x
                    || b.rect.x + b.rect.w <= a.rect.x
                    || a.rect.y + a.rect.h <= b.rect.y
                    || b.rect.y + b.rect.h <= a.rect.y;
                assert!(apart, "{:?} overlaps {:?}", a.action, b.action);
            }
        }
    }

    /// `g` with its run (the middle row by date) starred: listed first.
    fn star_the_run(g: &mut Game) {
        let run = g
            .history
            .file
            .records
            .iter()
            .find(|r| r.is_run() && r.version().is_ok());
        let run = run.unwrap().clone();
        g.history.file.set_star(&run, true).unwrap();
        assert!(g.history.file.listed().next().unwrap().starred);
    }

    /// The list with keys (the chosen row marked, a star before the starred
    /// row, STAR in the hint) and on a phone (a plate a row with its star
    /// button apart to the right, BACK under them), the starred run first
    /// and the older version's row dimmed and marked.
    #[test]
    fn the_replays_list_is_pinned() {
        let mut g = game(false);
        star_the_run(&mut g);
        // Opened on the newest row not starred: the match, under the run.
        g.open_replays();
        assert_eq!(g.history.focus, 1);
        let (keys, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("replays_keys", keys.palette_text(&Palette::default()));

        let mut g = game(true);
        star_the_run(&mut g);
        g.display.corner_pad = (10, 6);
        g.open_replays();
        let (phone, buttons) = draw(&g);
        laid_out(&g, &buttons);
        let rows: Vec<Action> = (0..3)
            .flat_map(|i| [Action::Wb(Wb::RowStar(i)), Action::Wb(Wb::Row(i))])
            .collect();
        assert_eq!(
            buttons.iter().map(|b| b.action).collect::<Vec<_>>(),
            [rows, vec![Action::Back]].concat()
        );
        // The stars line up in a column, clear of the widest row.
        let stars: Vec<_> = buttons
            .iter()
            .filter(|b| matches!(b.action, Action::Wb(Wb::RowStar(_))))
            .collect();
        assert!(stars.iter().all(|b| b.rect.x == stars[0].rect.x));
        let widest = buttons
            .iter()
            .filter(|b| matches!(b.action, Action::Wb(Wb::Row(_))))
            .map(|b| b.rect.x + b.rect.w)
            .max()
            .unwrap();
        assert!(stars[0].rect.x - widest >= STAR_GAP);
        insta::assert_snapshot!("replays_phone", phone.palette_text(&Palette::default()));
    }

    /// The star column keeps its gap from the widest row whatever the
    /// buffer's width (320 to 400) and a cutout's pad: the rows move left
    /// rather than the gap shrink. The widest row a list can show is 53
    /// characters.
    #[test]
    fn the_star_column_keeps_its_gap_at_every_width() {
        use crate::history::{DuelRecord, Kind};
        let worst = Record::new(
            20_726,
            Kind::Duel(DuelRecord {
                ai: None,
                ground: Ground::Map("ABCDEFGHIJKL".into()),
                winner: crate::versus::Side::Right,
                rounds: [1, 2],
                line: format!("match 1 {} 1 1 320 1 0 0 0", salt_line_sim::SIM_EPOCH + 1),
            }),
        );
        assert_eq!(
            row_text(&worst).0,
            "SEP 30  2 PLAYERS  RIGHT WON 1-2  ABCDEFGHIJKL  NEWER"
        );
        assert_eq!(row_text(&worst).0.len(), 53);
        let mut widths = Vec::new();
        for window in [
            (1280, 720),
            (1600, 720),
            (2400, 1080),
            (2000, 900),
            (2560, 1080),
        ] {
            for pad in [0, 10, 24] {
                let mut g = Game::new(true, Save::default());
                g.fit_display(window, (0, 0));
                g.display.corner_pad = (pad, 6);
                g.history.file.push(worst.clone());
                g.history
                    .file
                    .push(Record::run(20_733, &finished_run(9, 320, 5), false));
                g.open_replays();
                let (_, buttons) = draw(&g);
                laid_out(&g, &buttons);
                let rect = |a: Wb| {
                    buttons
                        .iter()
                        .find(|b| b.action == Action::Wb(a))
                        .unwrap()
                        .rect
                };
                let widest = (0..2)
                    .map(|i| rect(Wb::Row(i)))
                    .map(|r| r.x + r.w)
                    .max()
                    .unwrap();
                let star = rect(Wb::RowStar(0));
                assert!(
                    star.x - widest >= STAR_GAP,
                    "w {} pad {pad}",
                    g.display.buffer_w
                );
                assert!(
                    star.x + star.w <= g.display.buffer_w - pad,
                    "w {} pad {pad}",
                    g.display.buffer_w
                );
                assert!(rect(Wb::Row(0)).x >= 0);
                widths.push(g.display.buffer_w);
            }
        }
        assert!(
            widths.contains(&320) && widths.iter().any(|&w| w >= 390),
            "{widths:?}"
        );
    }

    /// A replay played to its end.
    fn to_the_end(g: &mut Game) {
        for _ in 0..400 {
            if g.watch.as_ref().is_some_and(|w| w.ended) {
                return;
            }
            press(g, Key::Right);
        }
        for _ in 0..3600 {
            if g.watch.as_ref().is_some_and(|w| w.ended) {
                return;
            }
            g.tick(1.0 / 60.0, &idle());
        }
        panic!("never ended");
    }

    /// STAR at a replay's end: a phone's bar gains it after AGAIN and the
    /// speed (STARRED, lit, once set; a plate over the bar when five are
    /// starred already), and the keys' plate names the key.
    #[test]
    fn the_end_of_a_replay_offers_star() {
        let mut g = game(true);
        g.display.corner_pad = (10, 6);
        g.open_replays();
        press(&mut g, Key::Down);
        press(&mut g, Key::Enter);
        assert_eq!(g.mode, Mode::Playing);
        to_the_end(&mut g);
        g.input.touch_active = true;
        let (_, buttons) = draw(&g);
        laid_out(&g, &buttons);
        assert_eq!(
            buttons.iter().map(|b| b.action).collect::<Vec<_>>(),
            [Wb::Exit, Wb::Pause, Wb::Speed, Wb::Star].map(Action::Wb)
        );
        press(&mut g, Key::S);
        g.input.touch_active = true;
        let (phone, buttons) = draw(&g);
        laid_out(&g, &buttons);
        insta::assert_snapshot!("watch_end_phone", phone.palette_text(&Palette::default()));
        if let Some(w) = g.watch.as_mut() {
            w.star_refused = Some(crate::history::StarError::Full);
        }
        let (full, buttons) = draw(&g);
        laid_out(&g, &buttons);
        insta::assert_snapshot!("watch_end_full", full.palette_text(&Palette::default()));

        let mut g = game(false);
        g.open_replays();
        press(&mut g, Key::Down);
        press(&mut g, Key::Enter);
        to_the_end(&mut g);
        let (keys, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("watch_end_keys", keys.palette_text(&Palette::default()));
    }

    #[test]
    fn an_empty_list_says_how_to_fill_it() {
        let mut g = Game::new(true, Save::default());
        g.fit_display(WINDOW, (0, 0));
        g.open_replays();
        let (_, buttons) = draw(&g);
        assert_eq!(
            buttons.iter().map(|b| b.action).collect::<Vec<_>>(),
            [Action::Back]
        );
    }

    /// The bar over a watch: a phone's buttons over a run, and the keys'
    /// plate over a match's opening plate.
    #[test]
    fn the_watch_bar_is_pinned() {
        let mut g = game(true);
        g.display.corner_pad = (10, 6);
        g.open_replays();
        press(&mut g, Key::Down);
        press(&mut g, Key::Enter);
        assert!(g.watching());
        assert_eq!(g.mode, Mode::Playing);
        // The keys chose it; the phone's finger is back.
        g.input.touch_active = true;
        let (phone, buttons) = draw(&g);
        laid_out(&g, &buttons);
        assert_eq!(
            buttons.iter().map(|b| b.action).collect::<Vec<_>>(),
            [Wb::Exit, Wb::Pause, Wb::Speed, Wb::NextShot, Wb::NextRound].map(Action::Wb),
            "the bar alone: no aim controls, no II"
        );
        // EXIT is well away from PAUSE: a child's tap on one is not on the
        // other.
        let at = |a: Wb| {
            buttons
                .iter()
                .find(|b| b.action == Action::Wb(a))
                .unwrap()
                .rect
        };
        let (exit, pause) = (at(Wb::Exit), at(Wb::Pause));
        assert!(pause.y - (exit.y + exit.h) >= 60, "{exit:?} {pause:?}");
        insta::assert_snapshot!("watch_run_phone", phone.palette_text(&Palette::default()));

        let mut g = game(false);
        g.open_replays();
        press(&mut g, Key::Enter);
        assert_eq!(g.mode, Mode::Versus);
        let (keys, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("watch_duel_keys", keys.palette_text(&Palette::default()));
    }

    /// Where a phone finds them: the REPLAYS plate under DEMO on the title,
    /// and WATCH under REMATCH on a won match's plate.
    #[test]
    fn the_phone_ways_in_are_pinned() {
        let mut g = Game::new(true, Save::default());
        g.fit_display(WINDOW, (0, 0));
        g.display.corner_pad = (10, 6);
        let (title, buttons) = draw(&g);
        laid_out(&g, &buttons);
        insta::assert_snapshot!("phone_title", title.palette_text(&Palette::default()));

        g.saved_match = Some(finished_match(31, Some(Level::Easy)).0);
        g.resume_match();
        let (plate, buttons) = draw(&g);
        laid_out(&g, &buttons);
        insta::assert_snapshot!("phone_won_plate", plate.palette_text(&Palette::default()));
    }

    /// WATCH on the screens a watch starts from: a run's result and a won
    /// match's plate.
    #[test]
    fn the_result_screens_offer_watch() {
        let mut g = Game::new(true, Save::default());
        g.fit_display(WINDOW, (0, 0));
        g.start_run(9, false);
        g.play.run = Some(finished_run(9, g.display.buffer_w as usize, 5));
        g.finish_run();
        let (_, buttons) = draw(&g);
        assert!(buttons.iter().any(|b| b.action == Action::Watch));
        laid_out(&g, &buttons);

        let mut g = Game::new(true, Save::default());
        g.fit_display(WINDOW, (0, 0));
        g.display.corner_pad = (10, 6);
        g.saved_match = Some(finished_match(31, Some(Level::Easy)).0);
        g.resume_match();
        let (_, buttons) = draw(&g);
        assert!(buttons.iter().any(|b| b.action == Action::Watch));
        laid_out(&g, &buttons);
    }
}

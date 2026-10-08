//! Drawing the REPLAYS list and the bar over a replay being watched
//! (DESIGN §9.4, `app/watch.rs`). Engine-free like the rest of `render`.
//!
//! # The list
//!
//! ```text
//!                         SALT LINE
//!                    REPLAYS   PAGE 1 / 2
//!               (a note: why a row cannot be watched)
//!          ┌──────────────────────────────────────────┐
//!          │ OCT 8  AI EASY  YOU WON 2-1  MESA        │  a row: tap to watch
//!          │ OCT 8  RUN  9 SCREENS  SEED 1234567      │
//!          │ OCT 7  2 PLAYERS  LEFT WON 2-0  OLD      │  dimmed: another version
//!          └──────────────────────────────────────────┘
//!                   <        BACK        >
//! ```
//!
//! Newest first, a line each, in the words a child can read out (DESIGN
//! §11.4): when, against whom, who won. A row from another build's sim is
//! dimmed and says OLD (or NEWER); it stays listed, and choosing it says
//! why it cannot be watched (§9.3).
//!
//! # The watch bar
//!
//! ```text
//!   ANG 45                                               SCREEN 4
//!   PWR 60   [EXIT] [PAUSE] [2X] [NEXT SHOT] [NEXT ROUND]
//! ```
//!
//! Bottom centre, where the demo's banner sits: clear of the readouts in
//! the bottom corners, the draft cards and the duel's plates. On a
//! touchscreen it is a row of buttons as tall as the aim buttons (18
//! rows); with keys or a controller a plate saying what each key does. A
//! replay draws no aim controls of its own, and no prompts.

use super::*;
use crate::input::Wb;

/// The watch bar's buttons: as tall as the aim buttons.
const BAR_H: i32 = 18;
/// Pixels between the bar's buttons.
const BAR_GAP: i32 = 3;

impl Painter<'_> {
    /// The REPLAYS list (`Mode::Replays`), below the menus' title at `y`.
    pub(super) fn render_replays(&mut self, mut y: i32) {
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        let g = self.g;
        let records: Vec<&crate::history::Record> = g.history.file.newest_first().collect();
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
            for (i, record) in records.iter().enumerate().skip(page * rows).take(rows) {
                let mut text = record.summary();
                let c = match record.version() {
                    Ok(()) => fg,
                    Err(why) => {
                        text.push_str(match why {
                            crate::history::Refused::NewerVersion => "  NEWER",
                            _ => "  OLD",
                        });
                        dim
                    }
                };
                let row = (i - page * rows) as u8;
                if plated {
                    y = self.menu_row(y, Action::Wb(Wb::Row(row)), "", &text, c);
                } else {
                    let (marker, c) = if i == focus {
                        (">  ", alert)
                    } else {
                        ("   ", c)
                    };
                    self.centred(y, &format!("{marker}{text}   "), c);
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
            let mut hint = format!(
                "{} / {}  CHOOSE   {}  WATCH",
                label(Action::Up),
                label(Action::Down),
                label(Action::Confirm)
            );
            if pages > 1 {
                hint.push_str(&format!(
                    "   {} / {}  PAGE",
                    label(Action::Left),
                    label(Action::Right)
                ));
            }
            hint.push_str(&format!("   {}  BACK", label(Action::Back)));
            self.centred(y, &hint, fg);
        }
    }

    /// The bar over a replay being watched, bottom centre, and the run's
    /// result once a run has played to its end.
    pub(super) fn render_watch_bar(&mut self) {
        let Some(w) = self.g.watch.as_ref() else {
            return;
        };
        let (paused, ended, rate, duel) = (w.paused, w.ended, w.rate(), w.duel());
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
            let mut buttons: Vec<(&str, Wb)> =
                vec![("EXIT", Wb::Exit), (play, Wb::Pause), (&speed, Wb::Speed)];
            if !ended {
                buttons.push(("NEXT SHOT", Wb::NextShot));
                buttons.push((next_round, Wb::NextRound));
            }
            let width = |t: &str| (crate::font::small_width(t) as i32 + 10).max(22);
            let total: i32 = buttons.iter().map(|(t, _)| width(t)).sum::<i32>()
                + BAR_GAP * (buttons.len() as i32 - 1);
            let mut x = (cw - total) / 2;
            let y = ch - BAR_H - 3 - ey;
            for (text, b) in buttons {
                let bw = width(text);
                self.draw_touch_button(x, y, bw, BAR_H, text, Action::Wb(b));
                x += bw + BAR_GAP;
            }
            return;
        }
        let label = |a| self.g.input_label(a);
        let state = if ended {
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
                    String::new()
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

    /// The list with keys (the chosen row marked) and on a phone (a plate a
    /// row, BACK under them), the older version's row dimmed and marked.
    #[test]
    fn the_replays_list_is_pinned() {
        let mut g = game(false);
        g.open_replays();
        press(&mut g, Key::Down);
        let (keys, buttons) = draw(&g);
        assert!(buttons.is_empty());
        insta::assert_snapshot!("replays_keys", keys.palette_text(&Palette::default()));

        let mut g = game(true);
        g.display.corner_pad = (10, 6);
        g.open_replays();
        let (phone, buttons) = draw(&g);
        laid_out(&g, &buttons);
        let rows: Vec<Action> = (0..3).map(|i| Action::Wb(Wb::Row(i))).collect();
        assert_eq!(
            buttons.iter().map(|b| b.action).collect::<Vec<_>>(),
            [rows, vec![Action::Back]].concat()
        );
        insta::assert_snapshot!("replays_phone", phone.palette_text(&Palette::default()));
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

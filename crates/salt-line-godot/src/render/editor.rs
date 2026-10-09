//! Drawing the map editor (DESIGN §13.11): its pages, the draft with its
//! verdict, and test fire. Engine-free like the rest of `render`.
//!
//! # The editing screen
//!
//! ```text
//!  ┌───────────────────────────────────────────────────────────┐
//!  │ MENU   ⛔ LEFT GUN NOT ON GROUND            UNDO   REDO    │ row 1
//!  │ SCULPT SMOOTH STAMP GUNS  WIND  MIRROR TEST                │ row 2
//!  │ BRUSH  S  M  L                                            │ row 3
//!  │                         sky                               │
//!  │ ═╦═                                                  ═╦═  │
//!  │▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ the ground, edited ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│
//!  │ RIDGE 4KQ2                                                │
//!  └───────────────────────────────────────────────────────────┘
//! ```
//!
//! Every control sits in the top 69 rows (63 without a phone's corner
//! pad), above the world's ceiling (`WORLD_CEIL`, 108, the top 72 rows of
//! a 180-row buffer is sky whatever the ground does), so no button ever
//! covers ground a finger means to sculpt, and each is 18 rows tall, as
//! tall as the aim buttons (§13.11: no smaller). The rest of the screen is
//! the canvas. The verdict is a symbol and a few words (§13.11): a stop
//! sign and the first problem that blocks, a warning triangle for one that
//! only warns, or a flag with READY and the fairness sign.

use super::versus::side_slot;
use super::*;
use crate::app::{EditorPage as Page, TestFire, VersusCall as Call};
use crate::editor::{
    problem_gun, problem_words, wind_text, Brush, Editor, Fairness, Severity, Stamp, Tool,
    WindField, WindMode,
};
use crate::input::Ed;
use crate::versus::{self, Side};
use salt_line_sim::layout::WindSpec;

/// The editing screen's control rows: tall as the aim buttons.
const ROW_H: i32 = 18;

impl Painter<'_> {
    /// A frame of the editor, whatever its page.
    pub(super) fn render_editor(&mut self) {
        match self.g.editor.page {
            Page::Edit => self.render_edit(),
            Page::Test => self.render_test(),
            Page::Start | Page::Menu => self.render_editor_rows(),
            Page::Seed | Page::Name => self.render_editor_keypad(),
            Page::Slots(p) => self.render_editor_slots(p),
        }
    }

    /// An editor button: a plate with a label, lit while a finger is on it
    /// or when `on`.
    fn ed_button(&mut self, x: i32, y: i32, w: i32, label: &str, ed: Ed, on: bool) {
        let pal = self.pal;
        self.canvas.plate(x, y, w, ROW_H, pal);
        let lit = on || self.g.input.touch.held(Action::Edit(ed));
        if on {
            self.canvas
                .rect_outline(x + 1, y + 1, w - 2, ROW_H - 2, pal.get(slot::ALERT));
        }
        let c = pal.get(if lit { slot::ALERT } else { slot::UI_FG });
        let tx = x + (w - crate::font::small_width(label) as i32) / 2;
        self.canvas.text_small(tx, y + (ROW_H - 5) / 2, label, c);
        self.touch_button(Rect::new(x, y, w, ROW_H), Action::Edit(ed));
    }

    /// The calm menu backdrop, with a heading under the game's title
    /// lettering. Returns the y for the page's first line.
    fn editor_backdrop(&mut self, heading: &str) -> i32 {
        let pal = self.pal;
        let view = self.g.view();
        raster::draw_sky(self.canvas, &view, pal);
        let ey = self.g.display.corner_pad.1;
        let y = 10 + ey;
        let w = self.canvas.w;
        let tracking = 6;
        let title = "MAP EDITOR";
        let tx = (w - title.chars().count() as i32 * tracking) / 2;
        for (i, ch) in title.chars().enumerate() {
            self.canvas.text_small(
                tx + i as i32 * tracking,
                y,
                &ch.to_string(),
                pal.get(slot::ALERT),
            );
        }
        self.centred(y + 10, heading, pal.get(slot::SKY_LO));
        y + 22
    }

    /// The status line, if a note is up, centred at `y`.
    fn editor_status(&mut self, y: i32) {
        if self.g.status_time > 0.0 {
            let text = self.g.status.clone();
            let w = crate::font::small_width(&text) as i32 + 8;
            let x = (self.canvas.w - w) / 2;
            self.canvas.plate(x, y - 3, w, 11, self.pal);
            self.centred(y, &text, self.pal.get(slot::ALERT));
        }
    }

    /// The UNDO toast, bottom right, if one is up.
    fn editor_toast(&mut self) {
        let Some(toast) = self.g.editor.toast.as_ref() else {
            return;
        };
        let text = toast.text.clone();
        let pal = self.pal;
        let (ex, ey) = self.g.display.corner_pad;
        let (w, h) = (self.canvas.w, self.canvas.h);
        let tw = crate::font::small_width(&text) as i32;
        let bw = 44;
        let pw = tw + bw + 14;
        let (x, y) = (w - pw - 3 - ex, h - ROW_H - 4 - ey);
        self.canvas.plate(x, y - 1, pw, ROW_H + 2, pal);
        self.canvas
            .text_small(x + 5, y + (ROW_H - 5) / 2, &text, pal.get(slot::UI_FG));
        self.ed_button(x + pw - bw - 1, y, bw, "UNDO", Ed::ToastUndo, false);
    }

    /// The start page and the menu: rows of plates, by touch or a focus.
    fn render_editor_rows(&mut self) {
        let menu = self.g.editor.page == Page::Menu;
        let heading = if menu {
            self.g
                .editor
                .ed
                .as_ref()
                .map_or("MENU".to_string(), crate::app::editor_label)
        } else {
            "MAKE A MAP FOR LOCAL VERSUS".to_string()
        };
        let mut y = self.editor_backdrop(&heading);
        let pal = self.pal;
        let touch = self.g.input.touch_active;
        let rows = self.g.editor_rows();
        let w = self.canvas.w;
        if touch {
            // Two columns of buttons as tall as the aim buttons, row by row.
            let (cw, gap, pitch) = (140, 8, ROW_H + 4);
            let x0 = (w - 2 * cw - gap) / 2;
            for (i, (_, text)) in rows.iter().enumerate() {
                let (col, line) = ((i % 2) as i32, (i / 2) as i32);
                let x = x0 + col * (cw + gap);
                self.ed_button(x, y - 6 + line * pitch, cw, text, Ed::Row(i as u8), false);
            }
            y += rows.len().div_ceil(2) as i32 * pitch;
        }
        for (i, (_, text)) in rows.iter().enumerate().filter(|_| !touch) {
            let focused = self.g.editor.focus == i;
            let tw = crate::font::small_width(text) as i32;
            let (x, pw) = ((w - tw) / 2 - 12, tw + 24);
            if focused {
                self.canvas
                    .rect_outline(x, y - 4, pw, 13, pal.get(slot::ALERT));
            }
            let c = pal.get(if focused { slot::ALERT } else { slot::UI_FG });
            self.centred(y, text, c);
            y += 11;
        }
        if self.g.editor.page == Page::Start && self.g.editor.newer_draft {
            self.centred(y, "A NEWER VERSION'S DRAFT IS KEPT:", pal.get(slot::ALERT));
            self.centred(
                y + 8,
                "SAVE TO KEEP WHAT YOU MAKE HERE",
                pal.get(slot::ALERT),
            );
            y += 18;
        }
        if !touch {
            y += 2;
            let hint = if self.g.input.controller_active {
                "UP DOWN  A CHOOSE  B BACK"
            } else {
                "UP DOWN  ENTER CHOOSE  ESC BACK"
            };
            self.centred(y, hint, pal.get(slot::SKY_LO));
            y += 10;
        }
        self.editor_status(y + 4);
        self.editor_toast();
    }

    /// FORK A SEED and NAME: the typed text, the seed keypad, and OK.
    fn render_editor_keypad(&mut self) {
        let seed = self.g.editor.page == Page::Seed;
        let mut y = self.editor_backdrop(if seed {
            "FORK A SEED: A DUEL TO START FROM"
        } else {
            "NAME THIS MAP"
        });
        let pal = self.pal;
        let (fg, dim, alert) = (
            pal.get(slot::UI_FG),
            pal.get(slot::SKY_LO),
            pal.get(slot::ALERT),
        );
        let text = self.g.editor.text.clone();
        self.centred(y, &format!("> {text}"), fg);
        y += 10;
        let w = self.canvas.w;
        if self.plated() {
            y = self.draw_seed_keypad(y) + 3;
            if seed {
                let round = format!("ROUND {}", self.g.editor.round + 1);
                let tw = crate::font::small_width(&round) as i32;
                let x = (w - tw) / 2;
                self.ed_button(x - 30, y, 24, "-", Ed::RoundDown, false);
                self.canvas.text_small(x, y + 6, &round, alert);
                self.ed_button(x + tw + 6, y, 24, "+", Ed::RoundUp, false);
                y += ROW_H + 6;
            }
            let ok = if seed { "FORK" } else { "OK" };
            let y2 = self.menu_row(y + 4, Action::Confirm, "", ok, fg);
            self.menu_row(y2, Action::Back, "", "BACK", dim);
        } else {
            y += 4;
            if seed {
                self.centred(
                    y,
                    &format!("ROUND {} (TAP + - ON A PHONE)", self.g.editor.round + 1),
                    alert,
                );
                y += 10;
                self.centred(y, "TYPE A SEED  ENTER FORK  ESC BACK", fg);
            } else {
                self.centred(y, "A-Z AND 0-9, UP TO 12  ENTER OK  ESC BACK", fg);
            }
        }
        self.editor_status(self.canvas.h - 14);
    }

    /// The slot list: to open a map, or to save into a slot.
    fn render_editor_slots(&mut self, purpose: crate::app::EditorPurpose) {
        use crate::app::{EditorPurpose as Purpose, EDITOR_ROWS as VISIBLE_ROWS};
        let open = purpose == Purpose::Open;
        let mut y = self.editor_backdrop(if open {
            "OPEN A MAP"
        } else {
            "SAVE INTO A SLOT - TAP ONE"
        });
        let pal = self.pal;
        let rows = self.g.slot_rows(purpose);
        let (top, sel) = (self.g.editor.list_top, self.g.editor.list_sel);
        let w = self.canvas.w;
        let (ex, ey) = self.g.display.corner_pad;
        let (x, pw) = (16 + ex, w - 32 - 2 * ex);
        y -= 6;
        for i in 0..VISIBLE_ROWS {
            let Some(&row) = rows.get(top + i) else { break };
            let (text, dimmed) = self.g.slot_row_text(row);
            self.canvas.plate(x, y, pw, ROW_H, pal);
            let chosen = top + i == sel;
            if chosen {
                self.canvas
                    .rect_outline(x + 1, y + 1, pw - 2, ROW_H - 2, pal.get(slot::ALERT));
            }
            let c = pal.get(if dimmed {
                slot::ROCK
            } else if chosen {
                slot::ALERT
            } else {
                slot::UI_FG
            });
            self.canvas.text_small(x + 6, y + 7, &text, c);
            self.touch_button(Rect::new(x, y, pw, ROW_H), Action::Edit(Ed::Row(i as u8)));
            y += ROW_H + 3;
        }
        y += 6;
        if rows.is_empty() {
            self.centred(y, "NO MAPS SAVED YET", pal.get(slot::SKY_LO));
        }
        let by = self.canvas.h - ROW_H - 4 - ey;
        let mut bx = 3 + ex;
        let mut button = |p: &mut Self, label: &str, ed: Ed, bw: i32| {
            p.ed_button(bx, by, bw, label, ed, false);
            bx += bw + 3;
        };
        if rows.len() > VISIBLE_ROWS {
            button(self, "<", Ed::PageUp, 24);
            button(self, ">", Ed::PageDown, 24);
        }
        if open {
            button(self, "OPEN", Ed::Open, 40);
            button(self, "COPY", Ed::Copy, 40);
            button(self, "DELETE", Ed::Delete, 48);
        }
        button(self, "BACK", Ed::Back, 40);
        if !self.g.input.touch_active {
            let hint = if open {
                "ENTER OPEN  C COPY  X DELETE  ESC BACK"
            } else {
                "ENTER SAVE  ESC BACK"
            };
            self.centred(by - 9, hint, pal.get(slot::SKY_LO));
        }
        self.editor_status(by - 18);
        self.editor_toast();
    }

    /// The editing screen. See the module docs.
    fn render_edit(&mut self) {
        let g = self.g;
        let Some(ed) = g.editor.ed.as_ref() else {
            return;
        };
        let pal = self.pal;
        let view = View {
            cam_x: 0,
            w: g.display.buffer_w,
            h: g.display.buffer_h,
        };
        let screen = crate::editor::screen_of(ed.layout());
        let guns = Side::BOTH.map(|s| versus::muzzle_of(&screen, s).x);
        self.cache.backdrop.draw(
            self.canvas,
            &view,
            pal,
            sky::Phase::for_screen(0),
            crate::biome::Biome::Salt,
            screen.seed,
            0.0,
            &screen,
            &guns,
        );
        for side in Side::BOTH {
            self.draw_gun(versus::muzzle_of(&screen, side), 600, side, &view);
        }
        // A mark over each gun a problem names.
        let alert = pal.get(slot::ALERT);
        for (p, _) in &ed.verdict.problems {
            if let Some(i) = problem_gun(p) {
                let side = if i == 0 { Side::Left } else { Side::Right };
                let m = versus::muzzle_of(&screen, side);
                let (x, y) = (view.sx(m.x) - 3, view.sy(m.y) - 20);
                self.canvas.icon(
                    x,
                    y,
                    &crate::icons::WARN,
                    &[alert, pal.get(slot::UI_BG)],
                    false,
                );
            }
        }
        if !g.input.touch_active {
            self.draw_edit_cursor(ed, &screen.field, &view);
        }
        self.render_edit_bar(ed);
        // The draft's name (or where it came from) and its fingerprint.
        let (ex, ey) = g.display.corner_pad;
        let label = crate::app::editor_label(ed);
        let lw = crate::font::small_width(&label) as i32;
        let h = self.canvas.h;
        let ly = if g.input.touch_active {
            h - 12 - ey
        } else {
            h - 22 - ey
        };
        self.canvas.plate(3 + ex, ly, lw + 8, 11, pal);
        self.canvas
            .text_small(7 + ex, ly + 3, &label, pal.get(slot::UI_FG));
        if !g.input.touch_active {
            let hint = if g.input.controller_active {
                "LB RB TOOL  X Y OPTION  PAD MOVE  A APPLY  B MENU"
            } else {
                "[ ] TOOL  , . OPTION  ARROWS  ENTER  Z UNDO  I MIRROR  T TEST  ESC MENU"
            };
            let hw = crate::font::small_width(hint) as i32;
            self.canvas.plate(3 + ex, h - 11 - ey, hw + 8, 10, pal);
            self.canvas
                .text_small(7 + ex, h - 8 - ey, hint, pal.get(slot::SKY_LO));
        }
        self.editor_status(80 + ey);
    }

    /// Where keys and a controller act: a dotted line down to the ground
    /// at the cursor and ticks at the brush's reach.
    fn draw_edit_cursor(
        &mut self,
        ed: &Editor,
        field: &salt_line_sim::terrain::Heightfield,
        view: &View,
    ) {
        if matches!(ed.tool, Tool::Guns | Tool::Wind) {
            return;
        }
        let pal = self.pal;
        let alert = pal.get(slot::ALERT);
        let c = ed.cursor as usize;
        let x = view.sx(c as f64);
        let top = 3 * ROW_H + 12 + self.g.display.corner_pad.1;
        let ground = view.sy(field.surface(c.min(field.len() - 1)));
        self.canvas.vline_dotted(x, top, ground - 2, alert, 2);
        if matches!(ed.tool, Tool::Sculpt | Tool::Smooth) {
            let r = ed.brush.radius();
            for dx in [-r, r] {
                let cx = (c as i32 + dx).clamp(0, field.len() as i32 - 1) as usize;
                let gy = view.sy(field.surface(cx));
                self.canvas.vline(x + dx, gy - 4, gy - 2, alert);
            }
        }
    }

    /// The editing screen's three rows of controls, and the verdict.
    fn render_edit_bar(&mut self, ed: &Editor) {
        let pal = self.pal;
        let (ex, ey) = self.g.display.corner_pad;
        let w = self.canvas.w;
        let alert = pal.get(slot::ALERT);
        // Row 1: MENU, the verdict, UNDO and REDO.
        let y1 = 3 + ey;
        self.ed_button(3 + ex, y1, 36, "MENU", Ed::Menu, false);
        let (rx, bw) = (w - 3 - ex - 2 * 36 - 3, 36);
        self.ed_button(rx, y1, bw, "UNDO", Ed::Undo, false);
        self.ed_button(rx + bw + 3, y1, bw, "REDO", Ed::Redo, false);
        let (sx, sw) = (3 + ex + 39, rx - 3 - (3 + ex + 39));
        self.canvas.plate(sx, y1, sw, ROW_H, pal);
        // A toast (a slot just replaced) takes the verdict's place until the
        // next edit, its UNDO inside it: up here, over the sky, so no drag
        // on the ground can land on it.
        if let Some(toast) = self.g.editor.toast.as_ref() {
            let text = toast.text.clone();
            self.canvas.text_small(sx + 6, y1 + 7, &text, alert);
            self.ed_button(sx + sw - 46, y1, 46, "UNDO", Ed::ToastUndo, false);
        } else {
            self.render_verdict(ed, sx, sw, y1);
        }
        self.render_edit_tools(ed, y1);
    }

    /// The verdict in its plate at `sx`, `sw` wide, on row `y1`: a symbol
    /// and a few words.
    fn render_verdict(&mut self, ed: &Editor, sx: i32, sw: i32, y1: i32) {
        let pal = self.pal;
        let (fg, alert) = (pal.get(slot::UI_FG), pal.get(slot::ALERT));
        let v = &ed.verdict;
        let block = v.problems.iter().find(|(_, s)| *s == Severity::Block);
        let warn = v.problems.iter().find(|(_, s)| *s == Severity::Warn);
        let (icon, colors, text, c) = match (block, warn) {
            (Some((p, _)), _) => {
                let more = v
                    .problems
                    .iter()
                    .filter(|(_, s)| *s == Severity::Block)
                    .count();
                let text = if more > 1 {
                    format!("{} +{}", problem_words(p), more - 1)
                } else {
                    problem_words(p)
                };
                (
                    &crate::icons::STOP,
                    [alert, pal.get(slot::UI_BG), fg],
                    text,
                    alert,
                )
            }
            (None, Some((p, _))) => (
                &crate::icons::WARN,
                [alert, pal.get(slot::UI_BG), fg],
                problem_words(p),
                alert,
            ),
            (None, None) => {
                let fair = v.fairness.map_or("", Fairness::label);
                (
                    &crate::icons::FLAG,
                    [alert, pal.get(slot::UI_BG), fg],
                    format!("READY - {fair}"),
                    fg,
                )
            }
        };
        self.canvas.icon(sx + 4, y1 + 5, icon, &colors, false);
        let tw = crate::font::small_width(&text) as i32;
        let room = sw - 16;
        // Too long for the plate (a narrow layout of buttons): the words
        // are cut, never the plate overrun.
        let text = if tw > room {
            let n = (room / crate::font::SMALL_ADVANCE as i32).max(0) as usize;
            text.chars().take(n).collect()
        } else {
            text
        };
        self.canvas.text_small(sx + 13, y1 + 7, &text, c);
    }

    /// Rows two and three of the editing screen: the tools, then the
    /// tool's options.
    fn render_edit_tools(&mut self, ed: &Editor, y1: i32) {
        let pal = self.pal;
        let (ex, _) = self.g.display.corner_pad;
        let w = self.canvas.w;
        let fg = pal.get(slot::UI_FG);
        // Row 2: the tools, MIRROR and TEST.
        let y2 = y1 + ROW_H + 3;
        let n = Tool::ALL.len() as i32 + 2;
        let gap = 2;
        let tw = (w - 6 - 2 * ex - (n - 1) * gap) / n;
        let mut x = 3 + ex;
        for t in Tool::ALL {
            self.ed_button(x, y2, tw, t.label(), Ed::Tool(t), ed.tool == t);
            x += tw + gap;
        }
        self.ed_button(x, y2, tw, "MIRROR", Ed::Mirror, ed.mirror);
        x += tw + gap;
        self.ed_button(x, y2, tw, "TEST", Ed::Test, false);

        // Row 3: the tool's options.
        let y3 = y2 + ROW_H + 3;
        let x0 = 3 + ex;
        match ed.tool {
            Tool::Sculpt | Tool::Smooth => {
                self.canvas.plate(x0, y3, 40, ROW_H, pal);
                self.canvas.text_small(x0 + 7, y3 + 7, "BRUSH", fg);
                let mut x = x0 + 43;
                for b in Brush::ALL {
                    self.ed_button(x, y3, 26, b.label(), Ed::Brush(b), ed.brush == b);
                    x += 29;
                }
                let hint = if ed.tool == Tool::Sculpt {
                    "DRAG UP OR DOWN"
                } else {
                    "DRAG TO SMOOTH"
                };
                self.canvas.plate(
                    x,
                    y3,
                    crate::font::small_width(hint) as i32 + 10,
                    ROW_H,
                    pal,
                );
                self.canvas
                    .text_small(x + 5, y3 + 7, hint, pal.get(slot::SKY_LO));
            }
            Tool::Stamp => {
                let mut x = x0;
                for s in Stamp::ALL {
                    self.ed_button(x, y3, 46, s.label(), Ed::Stamp(s), ed.stamp == s);
                    x += 49;
                }
            }
            Tool::Guns => {
                let hint = if ed.mirror {
                    "DRAG A GUN - THE OTHER MIRRORS IT"
                } else {
                    "DRAG A GUN"
                };
                self.canvas.plate(
                    x0,
                    y3,
                    crate::font::small_width(hint) as i32 + 10,
                    ROW_H,
                    pal,
                );
                self.canvas
                    .text_small(x0 + 5, y3 + 7, hint, pal.get(slot::SKY_LO));
            }
            Tool::Wind => self.render_wind_row(ed, x0, y3),
        }
    }

    /// WIND's options: CALM, RANGE or FIXED, then the numbers and - +.
    fn render_wind_row(&mut self, ed: &Editor, x0: i32, y: i32) {
        let pal = self.pal;
        let wind = ed.layout().wind;
        let mode = WindMode::of(&wind);
        let mut x = x0;
        for m in WindMode::ALL {
            self.ed_button(x, y, 38, m.label(), Ed::WindMode(m), mode == m);
            x += 40;
        }
        x += 4;
        let fields: Vec<(WindField, String)> = match wind {
            WindSpec::Calm => Vec::new(),
            WindSpec::Fixed(w) => vec![(WindField::Fixed, wind_text(w))],
            WindSpec::Range { min, max } => vec![
                (WindField::Min, format!("MIN {}", wind_text(min))),
                (WindField::Max, format!("MAX {}", wind_text(max))),
            ],
        };
        if fields.is_empty() {
            let hint = "NO WIND";
            self.canvas
                .plate(x, y, crate::font::small_width(hint) as i32 + 10, ROW_H, pal);
            self.canvas
                .text_small(x + 5, y + 7, hint, pal.get(slot::SKY_LO));
            return;
        }
        let one = fields.len() == 1;
        for (f, text) in &fields {
            let bw = crate::font::small_width(text) as i32 + 10;
            self.ed_button(
                x,
                y,
                bw,
                text,
                Ed::WindField(*f),
                !one && ed.wind_field == *f,
            );
            x += bw + 3;
        }
        self.ed_button(x, y, 24, "-", Ed::WindDown, false);
        self.ed_button(x + 27, y, 24, "+", Ed::WindUp, false);
    }

    /// TEST FIRE: the draft's ground, both guns, a shot at a time.
    fn render_test(&mut self) {
        let g = self.g;
        let Some(t) = g.editor.test.as_ref() else {
            return;
        };
        let pal = self.pal;
        let view = View {
            cam_x: 0,
            w: g.display.buffer_w,
            h: g.display.buffer_h,
        };
        let screen = t.shown();
        let guns = Side::BOTH.map(|s| versus::muzzle_of(screen, s).x);
        self.cache.backdrop.draw(
            self.canvas,
            &view,
            pal,
            sky::Phase::for_screen(0),
            crate::biome::Biome::Salt,
            screen.seed,
            0.0,
            screen,
            &guns,
        );
        let aiming = t.anim.is_none();
        if aiming {
            let ink = self.side_ink(t.side);
            if let Some(mark) = t.landing[t.side.index()] {
                raster::draw_chalk_inked(self.canvas, &view, &[mark], &screen.field, ink);
            }
            let under = |x, y| self.cache.backdrop.under(x, y);
            raster::draw_last_arc_on(self.canvas, &view, &t.arcs[t.side.index()], pal, under);
        }
        for side in Side::BOTH {
            let m = versus::muzzle_of(screen, side);
            self.draw_gun_marker(view.sx(m.x), view.sy(m.y), side, side == t.side);
            let aim = if side == t.side {
                g.play.angle_ticks
            } else {
                t.aims[side.index()].0
            };
            self.draw_gun(m, aim, side, &view);
        }
        if let Some(anim) = t.anim.as_ref() {
            let ink = self.side_ink(t.side);
            let upto = (anim.t / DT).floor().max(0.0) as usize;
            for tr in &anim.traces {
                if !tr.path.is_empty() {
                    raster::draw_trajectory_inked(self.canvas, &view, &tr.path, upto, pal, ink);
                }
            }
            let muzzle = anim
                .traces
                .first()
                .map_or(versus::muzzle_of(&anim.before, t.side), |tr| tr.origin);
            self.draw_anim_effects(anim, muzzle, ink, &view);
        }
        self.render_test_hud(t, aiming);
    }

    fn render_test_hud(&mut self, t: &TestFire, aiming: bool) {
        let g = self.g;
        let pal = self.pal;
        let (ex, ey) = g.display.corner_pad;
        let (w, h) = (self.canvas.w, self.canvas.h);
        let fg = pal.get(slot::UI_FG);
        // Who fires and the wind, each a button; the gate's count; DONE.
        let y = 3 + ey;
        let side_label = format!("{} GUN", t.side.name());
        self.canvas.plate(3 + ex, y, 62, ROW_H, pal);
        self.canvas
            .rect_outline(4 + ex, y + 1, 60, ROW_H - 2, pal.get(side_slot(t.side)));
        let lw = crate::font::small_width(&side_label) as i32;
        self.canvas.text_small(
            3 + ex + (62 - lw) / 2,
            y + 7,
            &side_label,
            pal.get(side_slot(t.side)),
        );
        self.touch_button(Rect::new(3 + ex, y, 62, ROW_H), Action::Edit(Ed::TestSide));
        let wind = t.wind();
        let wind_label = if wind == 0 {
            "CALM".to_string()
        } else {
            format!(
                "WIND {}{}",
                wind_text(wind.abs()),
                if wind < 0 { " <" } else { " >" }
            )
        };
        self.ed_button(68 + ex, y, 70, &wind_label, Ed::TestWind, false);
        let gate = format!("GATE {}", t.count());
        let gx = w - 3 - ex - 44 - 3 - 50;
        self.canvas.plate(gx, y, 50, ROW_H, pal);
        let ok = t.count() >= salt_line_sim::MIN_SOLUTIONS;
        self.canvas.text_small(
            gx + 6,
            y + 7,
            &gate,
            pal.get(if ok { slot::UI_FG } else { slot::ALERT }),
        );
        self.ed_button(w - 3 - ex - 44, y, 44, "DONE", Ed::Back, false);
        if let Some((side, call)) = t.last.filter(|_| t.anim.as_ref().is_some_and(Anim::done)) {
            let word = match call {
                Call::Hit => "HIT!",
                Call::Miss => "MISS",
                Call::Oops => "OOPS",
            };
            let c = if call == Call::Hit {
                side_slot(side)
            } else {
                slot::UI_FG
            };
            let pw = crate::font::small_width(word) as i32 * 3 + 12;
            self.canvas.plate((w - pw) / 2, 28 + ey, pw, 22, pal);
            self.shout(w / 2, 32 + ey, word, c, 3);
        }
        if aiming {
            let shot = Shot::new(g.play.angle_ticks, g.play.power_ticks).quantized(&Loadout::new());
            let y = h - 8 - ey;
            self.canvas
                .text_small(3 + ex, y, &format!("PWR {}", shot.power_ticks), fg);
            self.canvas.text_small(
                3 + ex,
                y - 7,
                &format!("ANG {:.1}", shot.angle_degrees()),
                fg,
            );
            if g.input.touch_active {
                self.draw_test_controls();
            } else {
                let hint = if g.input.controller_active {
                    "LB RB WIND  X Y GUN  A FIRE  B DONE"
                } else {
                    "ARROWS AIM  SPACE FIRE  [ ] WIND  , . GUN  ESC DONE"
                };
                let hw = crate::font::small_width(hint) as i32 + 8;
                self.canvas.plate((w - hw) / 2, h - 25 - ey, hw, 11, pal);
                self.centred(h - 22 - ey, hint, pal.get(slot::SKY_LO));
            }
        }
        self.editor_status(56 + ey);
    }

    /// The aim buttons along the bottom, as in a match.
    fn draw_test_controls(&mut self) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let (ex, ey) = self.g.display.corner_pad;
        let (bw, bh) = (22, 18);
        let y = h - bh - 3 - ey;
        if self.g.drag_aiming() {
            return;
        }
        let x0 = 44 + ex;
        self.draw_touch_button(x0, y, bw, bh, "<", Action::Left);
        self.draw_touch_button(x0 + bw + 3, y, bw, bh, ">", Action::Right);
        self.draw_touch_button(x0 + 2 * bw + 12, y, bw, bh, "-", Action::Down);
        self.draw_touch_button(x0 + 3 * bw + 15, y, bw, bh, "+", Action::Up);
        self.draw_touch_button(w - 16 - 40 - ex, y, 40, bh, "FIRE", Action::Fire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Draft, Editor};
    use crate::save_format::Save;

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

    /// A game laid out as a phone (touch, the corner pad) on a 400-column
    /// buffer, or as a desktop with keys at 320.
    fn game(phone: bool) -> Game {
        let mut g = Game::new(phone, Save::default());
        let window = if phone {
            crate::cli::PHONE_WINDOW
        } else {
            (1280, 720)
        };
        g.fit_display(window, (0, 0));
        g.take_effects();
        g
    }

    fn editing(phone: bool, layout: salt_line_sim::layout::Layout) -> Game {
        let mut g = game(phone);
        g.editor.ed = Some(Editor::new(Draft {
            layout,
            from: None,
            slot: None,
        }));
        g.open_editor();
        g.take_effects();
        g
    }

    fn text(c: &Canvas) -> String {
        let t = c.palette_text(&Palette::default());
        assert!(!t.contains('.'), "pixels left undrawn");
        t
    }

    /// The editing screen on a phone, on a bundled map: the three rows of
    /// big controls above the world's ceiling (SCULPT and MIRROR lit, the
    /// brush sizes), the verdict's flag with READY and the fairness sign,
    /// both guns, and the map's name and fingerprint. Drawn at the layout
    /// width inside the phone's corner pad. Pins the touch layout a child
    /// uses.
    #[test]
    fn the_editing_screen_on_a_phone_is_pinned() {
        let g = editing(true, crate::custom::bundled::layouts()[1].clone());
        assert_eq!(g.display.buffer_w, 320, "letterboxed at the layout width");
        let (c, buttons) = draw(&g);
        // Nothing touchable reaches below the sky the ground never fills.
        let ceiling = 180 - salt_line_sim::terrain::WORLD_CEIL as i32;
        assert!(buttons
            .iter()
            .filter(|b| matches!(b.action, Action::Edit(_)))
            .all(|b| b.rect.y + b.rect.h <= ceiling || b.rect.y > 150));
        assert!(buttons.iter().all(|b| b.rect.h >= 15));
        insta::assert_snapshot!("editor_edit_phone", text(&c));
    }

    /// The editing screen with keys, on a map that does not pass: the stop
    /// sign and the first blocking problem in a few words (with how many
    /// more), the warning mark over the gun it names, the cursor and its
    /// brush ticks, and the key hint. Pins how a problem reads.
    #[test]
    fn a_problem_on_the_editing_screen_is_pinned() {
        let mut pit = crate::editor::Draft::blank().layout;
        for c in 288 - 19..288 + 20 {
            let floor = (276..303).contains(&c);
            pit.heights[c] = salt_line_sim::layout::height_steps(if floor { 40.0 } else { 108.0 });
        }
        let mut g = editing(false, pit);
        g.editor.ed.as_mut().unwrap().cursor = 160;
        let (c, _) = draw(&g);
        let ed = g.editor.ed.as_ref().unwrap();
        assert!(!ed.verdict.playable());
        insta::assert_snapshot!("editor_edit_problem", text(&c));
    }

    /// TEST FIRE on a phone, the right gun to fire in the range's strongest
    /// wind: the side and wind buttons, the gate's count, DONE, the gun
    /// markers and the aim buttons. Pins the test page's controls.
    #[test]
    fn test_fire_is_pinned() {
        let mut g = editing(true, crate::custom::bundled::layouts()[0].clone());
        g.editor.page = Page::Test;
        let ed = g.editor.ed.as_ref().unwrap();
        let mut t = crate::app::TestFire::new(ed.layout());
        t.side = Side::Right;
        t.wind_i = t.winds.len() - 1;
        g.editor.test = Some(t);
        let (c, buttons) = draw(&g);
        assert!(buttons
            .iter()
            .any(|b| b.action == Action::Edit(Ed::TestSide)));
        insta::assert_snapshot!("editor_test_fire", text(&c));
    }

    /// The slot list on a phone: the bundled maps, a saved one, one that
    /// no longer passes (dimmed, marked FAILS) and one from a newer build
    /// (dimmed, NEWER VERSION), with OPEN, COPY, DELETE and BACK. Pins how
    /// kept-but-unplayable maps read: shown, never deleted.
    #[test]
    fn the_slot_list_is_pinned() {
        let mut g = game(true);
        let good = crate::custom::fork_duel(4, 0);
        let mut bad = good.clone();
        for c in 288 - 19..288 + 20 {
            let floor = (276..303).contains(&c);
            bad.heights[c] = salt_line_sim::layout::height_steps(if floor { 40.0 } else { 108.0 });
        }
        let text_file = format!(
            "layouts 1\nlayout 1 {}\nlayout 2 {}\nlayout 3 SLZZZZZZZZ\n",
            good.to_code().unwrap(),
            bad.to_code().unwrap()
        );
        g.set_layouts(Some(&text_file));
        g.open_editor();
        g.editor.page = Page::Slots(crate::app::EditorPurpose::Open);
        g.editor.list_sel = 4;
        g.editor.list_top = 1;
        let (c, _) = draw(&g);
        insta::assert_snapshot!("editor_slots", text(&c));
    }

    /// The pre-match screen on a phone with a bundled map on the GROUND
    /// row: its name and fingerprint, the `<` beside it, ROLLING GUNS read
    /// OFF and greyed, and MAKE MAP in the corner. Pins the GROUND row.
    #[test]
    fn the_ground_row_is_pinned() {
        let mut g = game(true);
        g.open_versus_setup();
        g.step_ground(1);
        let (c, buttons) = draw(&g);
        assert!(buttons
            .iter()
            .any(|b| b.action == Action::Edit(Ed::MakeMap)));
        insta::assert_snapshot!("versus_setup_ground", text(&c));
    }
}

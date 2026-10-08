//! The map editor driven as the node drives it: taps on the buttons the
//! last frame drew, drags on the ground, keys, and the effects carried out
//! by a small stand-in for the adapter (DESIGN §13.11). The slot file and
//! the draft as a crash would leave them are `disk_tests`'s.

use super::editor::{Page, Purpose, RowAct};
use super::*;
use crate::editor::{Severity, Stamp, Tool, BLANK_HEIGHT};
use crate::input::{Ed, Event, Intents, Key, Pad};
use crate::render::{self, Art, Cache};
use crate::touch::Button;
use pretty_assertions::assert_eq;
use salt_line_sim::layout::Problem;
use salt_line_sim::layout::{height_steps, Layout, WindSpec, LAYOUT_WIDTH, MIN_GUN_GAP};

const FRAME: f64 = 1.0 / 60.0;
const WINDOW: (i32, i32) = (1280, 720);
/// A Pixel held landscape: a buffer of 400.
const WIDE: (i32, i32) = (2400, 1080);

/// What the adapter keeps for these tests: the files the editor writes,
/// what was put on the clipboard, and what PASTE will read from it.
#[derive(Default)]
struct Node {
    window: (i32, i32),
    layouts: Option<String>,
    draft: Option<String>,
    draft_writes: usize,
    copied: Vec<String>,
    clipboard: String,
    /// Slot-file writes fail, as on a full disk.
    full: bool,
}

impl Node {
    fn idle(&self) -> Intents {
        Intents {
            window: self.window,
            today: 20_000,
            ..Intents::default()
        }
    }

    fn apply(&mut self, g: &mut Game, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Store(Store::Layouts { text, why }) => {
                    let ok = !self.full;
                    if ok {
                        self.layouts = Some(text.clone());
                    }
                    let more = g.stored(Store::Layouts { text, why }, ok);
                    self.apply(g, more);
                }
                Effect::DraftFile(DraftFile::Write(t)) => {
                    self.draft = Some(t);
                    self.draft_writes += 1;
                }
                Effect::DraftFile(DraftFile::Discard) => self.draft = None,
                Effect::Clipboard(t) => self.copied.push(t),
                Effect::ReadClipboard => {
                    let text = self.clipboard.clone();
                    let more = g.pasted(&text);
                    self.apply(g, more);
                }
                _ => {}
            }
        }
    }

    fn tick(&mut self, g: &mut Game, i: Intents) {
        let fx = g.tick(FRAME, &i);
        self.apply(g, fx);
    }

    fn frames(&mut self, g: &mut Game, n: usize) {
        for _ in 0..n {
            let i = self.idle();
            self.tick(g, i);
        }
    }

    fn key(&mut self, g: &mut Game, key: Key) {
        let i = Intents {
            events: vec![Event::key(key)],
            keys_held: vec![key],
            ..self.idle()
        };
        self.tick(g, i);
        self.frames(g, 1);
    }

    fn shift_key(&mut self, g: &mut Game, key: Key) {
        let i = Intents {
            events: vec![Event::key(key)],
            keys_held: vec![key, Key::Shift],
            ..self.idle()
        };
        self.tick(g, i);
        self.frames(g, 1);
    }

    fn pad(&mut self, g: &mut Game, pad: Pad) {
        let i = Intents {
            pads: vec![pad],
            pads_held: vec![pad],
            ..self.idle()
        };
        self.tick(g, i);
        self.frames(g, 1);
    }

    fn typed(&mut self, g: &mut Game, text: &str) {
        for c in text.chars() {
            let i = Intents {
                events: vec![Event::Key {
                    key: None,
                    typed: Some(Typed::Char(c)),
                }],
                ..self.idle()
            };
            self.tick(g, i);
        }
    }

    /// Draw the frame as the node does and hand its buttons back.
    fn draw(&mut self, g: &mut Game) -> Vec<Button> {
        let mut canvas = crate::raster::Canvas::new(g.display.buffer_w, g.display.buffer_h);
        let mut buttons = Vec::new();
        render::frame(
            g,
            &Art::default(),
            &mut canvas,
            &mut buttons,
            &mut Cache::default(),
        );
        g.set_buttons(&buttons);
        buttons
    }

    /// Wait out the touch settle a page change starts, then tap the button
    /// for `action` that the frame on screen drew.
    fn tap(&mut self, g: &mut Game, action: Action) {
        self.frames(g, 20);
        let buttons = self.draw(g);
        let b = buttons
            .iter()
            .find(|b| b.action == action)
            .unwrap_or_else(|| panic!("no {action:?} on screen: {:?}", buttons));
        let (x, y) = (
            b.rect.x as f32 + b.rect.w as f32 / 2.0,
            b.rect.y as f32 + b.rect.h as f32 / 2.0,
        );
        let down = Intents {
            events: vec![
                Event::FingerDown { index: 0, x, y },
                Event::FingerUp { index: 0 },
            ],
            ..self.idle()
        };
        self.tick(g, down);
        self.frames(g, 1);
    }

    fn ed(&mut self, g: &mut Game, ed: Ed) {
        self.tap(g, Action::Edit(ed));
    }

    /// The row on a start page or menu that does `act`.
    fn row(&mut self, g: &mut Game, act: RowAct) {
        let i = g
            .editor_rows()
            .iter()
            .position(|r| r.0 == act)
            .unwrap_or_else(|| panic!("no {act:?} row"));
        self.ed(g, Ed::Row(i as u8));
    }

    /// A finger down at `from`, moved through `to`, and up.
    fn drag(&mut self, g: &mut Game, from: (f32, f32), to: &[(f32, f32)]) {
        self.frames(g, 20);
        self.draw(g);
        let down = Intents {
            events: vec![Event::FingerDown {
                index: 0,
                x: from.0,
                y: from.1,
            }],
            ..self.idle()
        };
        self.tick(g, down);
        for &(x, y) in to {
            let moved = Intents {
                events: vec![Event::FingerMoved { index: 0, x, y }],
                ..self.idle()
            };
            self.tick(g, moved);
        }
        let up = Intents {
            events: vec![Event::FingerUp { index: 0 }],
            ..self.idle()
        };
        self.tick(g, up);
        self.frames(g, 1);
    }
}

/// A touchscreen game at the pre-match screen.
fn setup(window: (i32, i32)) -> (Game, Node) {
    let mut g = Game::new(false, Save::default());
    g.input.touch_active = true;
    g.fit_display(window, (0, 0));
    g.take_effects();
    let mut node = Node {
        window,
        ..Node::default()
    };
    g.open_versus_setup();
    node.frames(&mut g, 2);
    (g, node)
}

/// A touchscreen game in the editor on a blank draft.
fn blank() -> (Game, Node) {
    let (mut g, mut node) = setup(WINDOW);
    node.ed(&mut g, Ed::MakeMap);
    node.row(&mut g, RowAct::Blank);
    assert_eq!(g.editor.page, Page::Edit);
    (g, node)
}

fn layout(g: &Game) -> &Layout {
    g.editor.ed.as_ref().expect("a draft").layout()
}

/// The screen row `units` above the blank ground.
fn above_ground(units: f32) -> f32 {
    179.0 - BLANK_HEIGHT as f32 - units
}

// ------------------------------------------------------------- the way in

#[test]
fn make_map_opens_the_editor_at_the_layout_width_and_back_out() {
    let (mut g, mut node) = setup(WIDE);
    assert_eq!(g.display.buffer_w, 400);
    node.ed(&mut g, Ed::MakeMap);
    assert_eq!(g.mode, Mode::Editor);
    assert_eq!(g.editor.page, Page::Start, "no draft yet");
    node.frames(&mut g, 3);
    assert_eq!(g.display.buffer_w, LAYOUT_WIDTH as i32, "320, letterboxed");
    node.row(&mut g, RowAct::Leave);
    assert_eq!(g.mode, Mode::VersusSetup);
    node.frames(&mut g, 3);
    assert_eq!(g.display.buffer_w, 400, "the device's width again");
    // With keys: E.
    g.input.touch_active = false;
    node.key(&mut g, Key::E);
    assert_eq!(g.mode, Mode::Editor);
    node.key(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::VersusSetup);
    // With a controller: X, and B back out.
    node.pad(&mut g, Pad::X);
    assert_eq!(g.mode, Mode::Editor);
    node.pad(&mut g, Pad::B);
    assert_eq!(g.mode, Mode::VersusSetup);
}

// ---------------------------------------------------------------- editing

#[test]
fn a_drag_sculpts_and_undo_and_redo_take_it_back_by_tap_and_key() {
    let (mut g, mut node) = blank();
    let flat = layout(&g).clone();
    let writes = node.draft_writes;
    node.drag(
        &mut g,
        (100.0, above_ground(0.0)),
        &[(100.0, above_ground(10.0)), (100.0, above_ground(25.0))],
    );
    let hill = layout(&g).clone();
    assert_eq!(hill.heights[100], height_steps(BLANK_HEIGHT + 25.0));
    assert_eq!(hill.asymmetry(), 0.0, "MIRROR is on for a blank map");
    assert_eq!(
        node.draft_writes,
        writes + 1,
        "written once, as the finger lifted"
    );
    assert_eq!(
        node.draft.as_deref(),
        crate::layouts_file::SavedDraft {
            draft: g.editor.ed.as_ref().unwrap().draft.clone(),
            mirror: true
        }
        .to_line()
        .as_deref()
    );
    node.ed(&mut g, Ed::Undo);
    assert_eq!(layout(&g), &flat);
    node.ed(&mut g, Ed::Redo);
    assert_eq!(layout(&g), &hill);
    g.input.touch_active = false;
    node.key(&mut g, Key::Z);
    assert_eq!(layout(&g), &flat);
    node.key(&mut g, Key::Y);
    assert_eq!(layout(&g), &hill);
}

#[test]
fn stamp_and_mirror_by_keys_and_a_controller() {
    let (mut g, mut node) = blank();
    g.input.touch_active = false;
    // ] twice: SCULPT, SMOOTH, STAMP.
    node.key(&mut g, Key::BracketRight);
    node.key(&mut g, Key::BracketRight);
    assert_eq!(g.editor.ed.as_ref().unwrap().tool, Tool::Stamp);
    node.key(&mut g, Key::Period);
    assert_eq!(g.editor.ed.as_ref().unwrap().stamp, Stamp::Chasm);
    // The cursor out to column 80, five at a time with shift.
    let start = g.editor.ed.as_ref().unwrap().cursor;
    for _ in 0..10 {
        node.shift_key(&mut g, Key::Right);
    }
    assert_eq!(g.editor.ed.as_ref().unwrap().cursor, start + 50);
    node.key(&mut g, Key::Enter);
    assert!(layout(&g).heights[start as usize + 50] < height_steps(BLANK_HEIGHT));
    assert_eq!(layout(&g).asymmetry(), 0.0);
    // MIRROR off, and the next stamp is one side's alone.
    node.key(&mut g, Key::I);
    assert!(!g.editor.ed.as_ref().unwrap().mirror);
    node.key(&mut g, Key::Left);
    node.key(&mut g, Key::Enter);
    assert!(layout(&g).asymmetry() > 0.0);
    // A controller: LB back to SMOOTH, the d-pad to the chasm's wall, A
    // smooths it.
    node.pad(&mut g, Pad::LeftShoulder);
    assert_eq!(g.editor.ed.as_ref().unwrap().tool, Tool::Smooth);
    for _ in 0..16 {
        node.pad(&mut g, Pad::DpadLeft);
    }
    let before = layout(&g).clone();
    node.pad(&mut g, Pad::A);
    assert_ne!(layout(&g), &before);
}

#[test]
fn the_guns_stop_at_their_limits_and_the_verdict_follows() {
    let (mut g, mut node) = blank();
    node.ed(&mut g, Ed::Tool(Tool::Guns));
    node.drag(&mut g, (30.0, 140.0), &[(200.0, 140.0), (300.0, 140.0)]);
    let guns = layout(&g).guns.clone();
    assert!(guns[1] - guns[0] >= MIN_GUN_GAP, "{guns:?}");
    assert_eq!(guns[0] as usize + guns[1] as usize, LAYOUT_WIDTH - 1);
    assert!(g.editor.ed.as_ref().unwrap().verdict.playable());
    // MIRROR off, the right gun dragged alone: not mirror images, so the
    // verdict blocks and SAVE refuses with the words on the status line.
    node.ed(&mut g, Ed::Mirror);
    node.drag(&mut g, (guns[1] as f32, 140.0), &[(290.0, 140.0)]);
    let v = &g.editor.ed.as_ref().unwrap().verdict;
    assert!(v
        .problems
        .iter()
        .any(|(p, s)| *p == Problem::GunsNotMirrored && *s == Severity::Block));
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Save);
    assert_eq!(
        g.editor.page,
        Page::Menu,
        "no slot list for a map that fails"
    );
    assert_eq!(g.status, "CAN'T SAVE: GUNS NOT MIRRORED");
}

#[test]
fn wind_name_and_test_fire() {
    let (mut g, mut node) = blank();
    node.ed(&mut g, Ed::Tool(Tool::Wind));
    node.ed(&mut g, Ed::WindMode(crate::editor::WindMode::Fixed));
    node.ed(&mut g, Ed::WindDown);
    assert_eq!(layout(&g).wind, WindSpec::Fixed(2400));
    // NAME, typed on keys.
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Name);
    assert_eq!(g.editor.page, Page::Name);
    node.typed(&mut g, "hill 9");
    assert_eq!(g.editor.text, "HILL9", "keypad characters only");
    let enter = Intents {
        events: vec![Event::Key {
            key: Some(Key::Enter),
            typed: Some(Typed::Enter),
        }],
        ..node.idle()
    };
    node.tick(&mut g, enter);
    assert_eq!(g.editor.page, Page::Edit);
    assert_eq!(layout(&g).name.as_str(), "HILL9");
    // TEST FIRE: calm and the fixed wind both ways; craters stay in the
    // test only.
    let before = layout(&g).clone();
    node.ed(&mut g, Ed::Test);
    assert_eq!(g.editor.page, Page::Test);
    let t = g.editor.test.as_ref().unwrap();
    assert_eq!(t.winds, vec![-2400, 0, 2400]);
    assert_eq!(t.wind(), 0, "starts calm");
    assert!(t.count() >= salt_line_sim::MIN_SOLUTIONS);
    // Aim a shot the gate found, and fire it.
    let shot = {
        let s = &t.screen;
        crate::versus::shots_at(
            &crate::versus::frame_for(s, crate::versus::Side::Left),
            crate::versus::target_in_frame(s, crate::versus::Side::Left),
            &salt_line_sim::modifiers::Loadout::new(),
            crate::versus::HIT_REACH,
            1,
        )[0]
    };
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    node.ed(&mut g, Ed::TestWind);
    assert_eq!(g.editor.test.as_ref().unwrap().wind(), 2400);
    node.ed(&mut g, Ed::TestWind);
    assert_eq!(g.editor.test.as_ref().unwrap().wind(), -2400);
    node.ed(&mut g, Ed::TestWind);
    g.input.touch_active = false;
    node.key(&mut g, Key::Space);
    assert!(g.editor.testing_in_flight());
    assert!(!g.idle(), "a test shot is drawn at the full frame rate");
    for _ in 0..600 {
        if !g.editor.testing_in_flight() {
            break;
        }
        node.frames(&mut g, 1);
    }
    let t = g.editor.test.as_ref().unwrap();
    assert_eq!(t.last.map(|l| l.1), Some(VersusCall::Hit));
    // A short lob from the other gun digs a crater, in the test's ground.
    node.key(&mut g, Key::Comma);
    assert_eq!(
        g.editor.test.as_ref().unwrap().side,
        crate::versus::Side::Right
    );
    (g.play.angle_ticks, g.play.power_ticks) = (1400, 330);
    node.key(&mut g, Key::Space);
    for _ in 0..600 {
        if !g.editor.testing_in_flight() {
            break;
        }
        node.frames(&mut g, 1);
    }
    let t = g.editor.test.as_ref().unwrap();
    assert_eq!(t.last.map(|l| l.1), Some(VersusCall::Miss));
    assert!(t.screen.field.scar.iter().any(|&s| s != 0.0), "a crater");
    node.key(&mut g, Key::Escape);
    assert_eq!(g.editor.page, Page::Edit);
    assert!(g.editor.test.is_none());
    assert_eq!(layout(&g), &before, "the craters were the test's alone");
}

// ----------------------------------------------------------------- slots

#[test]
fn save_open_overwrite_delete_and_their_undo() {
    let (mut g, mut node) = blank();
    node.drag(
        &mut g,
        (150.0, above_ground(0.0)),
        &[(150.0, above_ground(20.0))],
    );
    assert!(node.draft.is_some());
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Save);
    assert_eq!(g.editor.page, Page::Slots(Purpose::Save));
    node.ed(&mut g, Ed::Row(0));
    assert_eq!(g.status, "SAVED TO SLOT 1");
    assert_eq!(g.editor.page, Page::Edit, "back to the map");
    assert_eq!(node.draft, None, "the draft is cleared once saved");
    let first = g.layouts.slots.get(1).unwrap().code.clone();
    assert_eq!(Layout::from_code(&first).as_ref(), Ok(layout(&g)));
    assert!(node.layouts.as_deref().unwrap().contains(&first));
    // Saved and unchanged: leaving does not keep a draft.
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Leave);
    assert!(g.editor.ed.is_none());

    // Open it again from the start page, change it, save over its slot:
    // an overwrite, with an UNDO toast.
    node.ed(&mut g, Ed::MakeMap);
    node.row(&mut g, RowAct::Open);
    let rows = g.slot_rows(Purpose::Open);
    let slot_row = rows
        .iter()
        .position(|r| *r == super::editor::SlotRow::Slot(1))
        .unwrap();
    node.ed(&mut g, Ed::Row(slot_row as u8));
    node.ed(&mut g, Ed::Open);
    assert_eq!(g.editor.ed.as_ref().unwrap().draft.slot, Some(1));
    node.drag(
        &mut g,
        (60.0, above_ground(0.0)),
        &[(60.0, above_ground(-15.0))],
    );
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Save);
    let rows = g.slot_rows(Purpose::Save);
    assert_eq!(
        g.editor.list_sel,
        rows.iter()
            .position(|r| *r == super::editor::SlotRow::Slot(1))
            .unwrap(),
        "its own slot is chosen"
    );
    node.key(&mut g, Key::Enter);
    let second = g.layouts.slots.get(1).unwrap().code.clone();
    assert_ne!(second, first);
    assert_eq!(g.editor.toast.as_ref().unwrap().text, "SLOT 1 REPLACED");
    node.ed(&mut g, Ed::ToastUndo);
    assert_eq!(
        g.layouts.slots.get(1).unwrap().code,
        first,
        "UNDO put it back"
    );
    assert!(node.draft.is_some(), "and the change is a draft again");

    // Delete it from the slot list: gone, and back with UNDO.
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::NewMap);
    node.row(&mut g, RowAct::Open);
    node.ed(&mut g, Ed::Row(slot_row as u8));
    node.ed(&mut g, Ed::Delete);
    assert!(g.layouts.slots.get(1).is_none());
    assert_eq!(g.editor.toast.as_ref().unwrap().text, "SLOT 1 DELETED");
    node.ed(&mut g, Ed::ToastUndo);
    assert_eq!(g.layouts.slots.get(1).unwrap().code, first);
}

#[test]
fn a_failed_slot_write_keeps_the_draft() {
    let (mut g, mut node) = blank();
    node.drag(
        &mut g,
        (150.0, above_ground(0.0)),
        &[(150.0, above_ground(20.0))],
    );
    node.full = true;
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Save);
    node.ed(&mut g, Ed::Row(0));
    assert_eq!(g.status, "MAP NOT SAVED - DRAFT KEPT");
    assert!(
        g.layouts.slots.entries.is_empty(),
        "memory follows the disk"
    );
    assert!(node.draft.is_some());
    assert!(g.editor.unsaved());
}

#[test]
fn discarding_a_map_can_be_undone_until_the_editor_is_left() {
    let (mut g, mut node) = blank();
    node.drag(
        &mut g,
        (150.0, above_ground(0.0)),
        &[(150.0, above_ground(20.0))],
    );
    let made = layout(&g).clone();
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Discard);
    assert_eq!(g.editor.page, Page::Start);
    assert!(g.editor.ed.is_none());
    assert_eq!(node.draft, None);
    node.ed(&mut g, Ed::ToastUndo);
    assert_eq!(layout(&g), &made);
    assert!(node.draft.is_some(), "kept on disk again");
    assert!(g.editor.ed.as_ref().unwrap().can_undo(), "history and all");
}

// ------------------------------------------------------------ the clipboard

#[test]
fn copy_and_paste_a_code() {
    let (mut g, mut node) = blank();
    node.drag(
        &mut g,
        (150.0, above_ground(0.0)),
        &[(150.0, above_ground(20.0))],
    );
    let made = layout(&g).clone();
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Copy);
    assert_eq!(node.copied.last(), Some(&made.to_code().unwrap()));
    assert_eq!(g.status, "CODE COPIED");
    // Garbage on the clipboard: refused, the draft untouched.
    node.clipboard = "hello there".into();
    node.row(&mut g, RowAct::Paste);
    assert_eq!(g.status, "NOT A MAP CODE");
    assert_eq!(layout(&g), &made);
    // A run screen's code: one gun, refused.
    node.clipboard = Layout::from_run_screen(1, 0).to_code().unwrap();
    node.row(&mut g, RowAct::Paste);
    assert_eq!(g.status, "ONE-GUN MAPS COME LATER");
}

/// A code that reads but does not pass is loaded with its problems shown,
/// to be fixed, and UNDO puts the draft before it back.
#[test]
fn a_pasted_invalid_code_loads_with_its_problems() {
    let (mut g, mut node) = blank();
    let made = layout(&g).clone();
    let mut pit = made.clone();
    for c in 288 - 19..288 + 20 {
        let floor = (276..303).contains(&c);
        pit.heights[c] = height_steps(if floor { 40.0 } else { 108.0 });
    }
    node.clipboard = pit.to_code().unwrap();
    g.input.touch_active = false;
    node.key(&mut g, Key::P);
    assert_eq!(g.status, "MAP PASTED - IT HAS PROBLEMS");
    assert_eq!(layout(&g), &pit);
    let v = &g.editor.ed.as_ref().unwrap().verdict;
    assert!(!v.playable());
    assert!(v
        .problems
        .iter()
        .any(|(p, _)| matches!(p, Problem::Unsolvable { gun: 1, .. })));
    node.key(&mut g, Key::Z);
    assert_eq!(layout(&g), &made);
}

// ------------------------------------------------------------------ forks

#[test]
fn fork_a_seed_on_the_keypad() {
    let (mut g, mut node) = setup(WINDOW);
    node.ed(&mut g, Ed::MakeMap);
    node.row(&mut g, RowAct::ForkSeed);
    assert_eq!(g.editor.page, Page::Seed);
    for c in ['D', 'U', 'N', 'E'] {
        node.tap(&mut g, Action::Type(c));
    }
    node.ed(&mut g, Ed::RoundUp);
    node.tap(&mut g, Action::Confirm);
    assert_eq!(g.editor.page, Page::Edit);
    let d = &g.editor.ed.as_ref().unwrap().draft;
    assert_eq!(d.label(), "FROM SEED DUNE R2");
    assert_eq!(d.layout, crate::custom::fork_duel(parse_seed("DUNE"), 1));
}

/// MAKE MAP OF THIS ROUND on a Pixel's 400-column buffer: the round's
/// ground cropped to 320 keeping both guns, the match suspended for RESUME
/// MATCH rather than lost.
#[test]
fn fork_this_round_keeps_both_guns_and_the_match() {
    let (mut g, mut node) = setup(WIDE);
    node.tap(&mut g, Action::Confirm);
    assert_eq!(g.mode, Mode::Versus);
    g.versus.live.as_mut().unwrap().kept = true;
    let live = g.versus.live.as_ref().unwrap().duel.screen.clone();
    assert_eq!(live.width, 400);
    // Pause, then MAKE MAP OF THIS ROUND.
    node.frames(&mut g, 20);
    g.input.touch_active = false;
    node.key(&mut g, Key::Escape);
    assert!(g.versus.live.as_ref().unwrap().paused);
    g.input.touch_active = true;
    node.ed(&mut g, Ed::ForkRound);
    assert_eq!(g.mode, Mode::Editor);
    assert_eq!(g.editor.page, Page::Edit);
    assert!(g.saved_match.is_some(), "the match is suspended, not lost");
    let l = layout(&g);
    assert_eq!(l.width(), LAYOUT_WIDTH);
    assert_eq!(l.guns[0] as usize + l.guns[1] as usize, LAYOUT_WIDTH - 1);
    // Each gun's own side of the ground came with it.
    assert_eq!(
        l.heights[l.guns[0] as usize],
        height_steps(live.field.base[30])
    );
    assert_eq!(
        l.heights[l.guns[1] as usize],
        height_steps(live.field.base[369])
    );
    assert!(!g
        .editor
        .ed
        .as_ref()
        .unwrap()
        .verdict
        .problems
        .iter()
        .any(|(p, _)| matches!(p, Problem::GunNotSeated(_) | Problem::GunOffScreen(_))));
    assert_eq!(g.editor.ed.as_ref().unwrap().draft.label(), "FROM MATCH R1");
    // Leaving goes to the title, where RESUME MATCH is.
    node.ed(&mut g, Ed::Menu);
    node.row(&mut g, RowAct::Leave);
    assert_eq!(g.mode, Mode::Title);
    assert!(g.saved_match.is_some());
}

// ----------------------------------------------------------- GROUND row

#[test]
fn the_ground_row_picks_a_map_and_rolling_reads_off() {
    let (mut g, mut node) = setup(WIDE);
    assert!(g.versus.ground.is_none());
    node.ed(&mut g, Ed::GroundNext);
    let pick = g.versus.ground.clone().expect("the first bundled map");
    assert_eq!(pick.layout, crate::custom::bundled::layouts()[0]);
    // Rolling guns cannot be turned on over a map.
    let rolling = g.versus.rolling_off;
    node.tap(&mut g, Action::Up);
    assert_eq!(g.versus.rolling_off, rolling);
    // Back round to GENERATED with `<`.
    node.ed(&mut g, Ed::GroundPrev);
    assert!(g.versus.ground.is_none());
    node.ed(&mut g, Ed::GroundPrev);
    let last = g.grounds().last().unwrap().clone();
    assert_eq!(g.versus.ground.as_ref(), Some(&last));
    // Keys: DOWN steps on.
    g.input.touch_active = false;
    node.key(&mut g, Key::Down);
    assert!(g.versus.ground.is_none());
    node.key(&mut g, Key::Down);
    node.key(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    let vs = g.versus.live.as_ref().unwrap();
    assert!(vs.kept && vs.layout.is_some());
    assert_eq!(vs.duel.width(), LAYOUT_WIDTH);
    assert_eq!(vs.duel.rules.gun_shift, 0.0, "no rolling on a map");
}

/// A map saved in a slot appears in the GROUND row with its fingerprint,
/// after the bundled ones; one that no longer passes is shown dimmed and
/// does not start.
#[test]
fn saved_maps_join_the_ground_row_and_a_failing_one_does_not_start() {
    let (mut g, mut node) = setup(WINDOW);
    let good = crate::custom::fork_duel(4, 0);
    let mut bad = good.clone();
    for c in 288 - 19..288 + 20 {
        let floor = (276..303).contains(&c);
        bad.heights[c] = height_steps(if floor { 40.0 } else { 108.0 });
    }
    let text = format!(
        "layouts 1\nlayout 2 {}\nlayout 5 {}\n",
        good.to_code().unwrap(),
        bad.to_code().unwrap()
    );
    g.set_layouts(Some(&text));
    let grounds = g.grounds();
    let n = crate::custom::bundled::BUNDLED.len();
    assert_eq!(grounds.len(), n + 2);
    assert_eq!(grounds[n].layout, good);
    assert!(grounds[n].playable);
    assert!(!grounds[n + 1].playable, "shown, not deleted");
    let label = grounds[n].label();
    assert!(
        label.ends_with(&crate::layouts_file::fingerprint(&good)),
        "{label}"
    );
    for _ in 0..n + 2 {
        node.ed(&mut g, Ed::GroundNext);
    }
    assert_eq!(g.versus.ground.as_ref().unwrap().layout, bad);
    node.tap(&mut g, Action::Confirm);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert_eq!(g.status, "THIS MAP DOES NOT PASS - PICK ANOTHER");
}

#[test]
fn the_start_page_and_menu_offer_what_they_should() {
    let (mut g, mut node) = setup(WINDOW);
    node.ed(&mut g, Ed::MakeMap);
    assert_eq!(
        super::editor::test_rows(&g),
        [
            RowAct::Blank,
            RowAct::ForkSeed,
            RowAct::Open,
            RowAct::Paste,
            RowAct::Leave
        ]
    );
    node.row(&mut g, RowAct::Blank);
    node.ed(&mut g, Ed::Menu);
    assert!(
        !super::editor::test_rows(&g).contains(&RowAct::Undo),
        "on screen"
    );
    g.input.touch_active = false;
    assert!(
        super::editor::test_rows(&g).contains(&RowAct::Undo),
        "for keys"
    );
    // A controller reaches every row by focus.
    node.pad(&mut g, Pad::DpadDown);
    assert_eq!(g.editor.focus, 1);
    node.pad(&mut g, Pad::A);
    assert_eq!(g.editor.page, Page::Slots(Purpose::Save), "SAVE");
}

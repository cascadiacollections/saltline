//! The map editor in the game (DESIGN §13.11), and custom duels in the
//! menus (§13.9): the pages, their input, the draft on disk, the slot file,
//! the clipboard, test fire, the pre-match screen's GROUND row, and
//! forking the round on screen. The edits themselves are `crate::editor`'s.
//!
//! # Where it lives
//!
//! ```text
//!  title ─LOCAL VERSUS─► pre-match ──MAKE MAP (corner plate, E)──► editor
//!                            ▲  GROUND: < GENERATED · bundled · slots >
//!                            │                                     │
//!  a match's pause or ──MAKE MAP OF THIS ROUND (E)─────────────────┤
//!  result plate                                                    │
//!                            └──────────── EXIT ◄──────────────────┘
//! ```
//!
//! The phone title has no row to spare (§11.6), so the editor is reached
//! from the versus pre-match screen, the mode its maps are for, through a
//! corner plate that mirrors the title's DEMO and MUS plates. Leaving it
//! goes back there, or to the title if forking a match left that match
//! suspended (where RESUME MATCH is offered).
//!
//! # Pages
//!
//! ```text
//!  Start ─BLANK / FORK A SEED (Seed) / OPEN (Slots) / PASTE─► Edit ◄─► Test
//!                                                              │
//!                                       MENU (Esc, B) ─► Menu ─┤─► Name
//!                                                              └─► Slots (save)
//! ```
//!
//! With a draft already kept, the editor opens straight onto it.
//!
//! # The draft is never lost
//!
//! * **On disk after every edit** (`persist_draft`), as a versus match is
//!   kept (§11.6): `user://draft.txt`, one versioned line, replaced
//!   atomically; not while a finger is still dragging, but as it lifts. It
//!   comes back at the next launch, and the editor opens on it.
//! * **Cleared** only once it is saved to a slot (the slot file's write
//!   confirmed, `layouts_stored`) or discarded.
//! * **Every replacement is an edit.** BLANK, a fork, a paste or a slot
//!   opened over a draft replace it through the editor's undo, so UNDO
//!   brings it back. DISCARD and every slot overwrite or delete leave an
//!   UNDO toast up until the editor is left; a second tap can never do
//!   them, because a page change spends the tap that caused it (`One touch,
//!   one screen` in `app/versus.rs`) and the toast's UNDO sits where no
//!   button of the page before was.
//!
//! # The demo never touches any of it
//!
//! The demo starts only from the title and never opens the editor; the
//! slot file and the draft are only written from the editor's own pages,
//! each write checks for the demo as well (`persist_draft`, `save_to`),
//! and both are disk effects, so the demo's backstop in `Game::tick`
//! drops and flags any that slipped through (DESIGN §12.3).

use super::*;
use crate::editor::{
    self as model, problem_words, Brush, Draft, Editor, Stamp, Tool, WindField, WindMode,
};
use crate::input::Ed;
use crate::layouts_file::{fingerprint, SavedDraft, SlotEntry, Slots};
use crate::versus;
use crate::versus::Side;
use salt_line_sim::layout::{wind_units, Layout, LAYOUT_WIDTH};

/// Slot rows the slot list shows at once.
pub const VISIBLE_ROWS: usize = 6;

/// A page of the editor. See the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// BLANK, FORK A SEED, OPEN, PASTE, BACK.
    Start,
    /// FORK A SEED: the keypad, a round and FORK.
    Seed,
    /// The editing screen: the draft, the toolbar and the verdict.
    Edit,
    /// The editing screen's menu: save, name, codes, new, discard, exit.
    Menu,
    /// NAME: the keypad.
    Name,
    /// The slot list, to open a layout or to save into a slot.
    Slots(Purpose),
    /// TEST FIRE.
    Test,
}

/// What the slot list is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// Pick a bundled or saved layout to edit (or copy or delete).
    Open,
    /// Pick a slot to save the draft into.
    Save,
}

/// A row of the start page or the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowAct {
    Continue,
    Blank,
    ForkSeed,
    Open,
    Paste,
    Leave,
    ToMap,
    Save,
    Name,
    Test,
    Mirror,
    Undo,
    Redo,
    Copy,
    NewMap,
    Discard,
}

/// A row of the slot list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotRow {
    /// A bundled layout, by its place in `custom::bundled::BUNDLED`.
    Bundled(usize),
    /// A saved slot.
    Slot(u8),
    /// The first empty slot, to save into.
    New(u8),
}

/// A word left on screen after a slot was overwritten or deleted, or the
/// draft discarded, with what UNDO puts back.
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub undo: ToastUndo,
}

/// What a toast's UNDO puts back.
#[derive(Clone, Debug)]
pub enum ToastUndo {
    /// The slot file's text as it was.
    Slots(String),
    /// The discarded draft, its undo history and all.
    Draft(Box<Editor>),
}

/// TEST FIRE (§13.11): either gun, at calm and at each wind the gate
/// checks, on a copy of the draft's ground that keeps its craters until
/// the test is left.
pub struct TestFire {
    /// The draft's ground in the shared frame, craters and all.
    pub screen: Screen,
    /// Who fires.
    pub side: Side,
    /// The winds offered, in thousandths ([`Editor::test_winds`]).
    pub winds: Vec<i32>,
    pub wind_i: usize,
    /// The gate's count for each gun in each of `winds`, to its cap.
    pub counts: Vec<(i32, [usize; 2])>,
    /// Each side's aim, kept between turns.
    pub aims: [(i32, i32); 2],
    /// The shot in the air.
    pub anim: Option<Anim>,
    /// Each side's last arc and landing.
    pub arcs: [Vec<Vec2>; 2],
    pub landing: [Option<Vec2>; 2],
    /// How the last shot ended, while it is shown.
    pub last: Option<(Side, VersusCall)>,
    /// Shots fired this test.
    pub shots: u32,
}

impl TestFire {
    pub(crate) fn new(layout: &Layout) -> TestFire {
        let mut winds = crate::custom::duel_winds(&layout.wind);
        winds.push(0);
        winds.sort_unstable();
        winds.dedup();
        let counts = crate::custom::duel_counts_in(layout, &winds, salt_line_sim::solver::GATE_CAP);
        let wind_i = winds.iter().position(|&w| w == 0).unwrap_or(0);
        let mut screen = Screen::from_layout(layout, 0.0);
        screen.wind = wind_units(winds[wind_i]);
        TestFire {
            screen,
            side: Side::Left,
            winds,
            wind_i,
            counts,
            aims: [(900, 600); 2],
            anim: None,
            arcs: [Vec::new(), Vec::new()],
            landing: [None, None],
            last: None,
            shots: 0,
        }
    }

    /// The wind now, in thousandths.
    pub fn wind(&self) -> i32 {
        self.winds[self.wind_i]
    }

    /// The gate's count for the side firing, in this wind.
    pub fn count(&self) -> usize {
        self.counts
            .iter()
            .find(|(w, _)| *w == self.wind())
            .map_or(0, |(_, c)| c[self.side.index()])
    }

    /// The screen to draw: before the shot until it lands.
    pub fn shown(&self) -> &Screen {
        match &self.anim {
            Some(a) if !a.done() => &a.before,
            _ => &self.screen,
        }
    }
}

/// The player's saved layouts, as last confirmed on disk, and which pass.
#[derive(Default)]
pub(crate) struct LayoutsState {
    pub(crate) slots: Slots,
    /// Each slot's layout, if it reads, and whether it passes the duel gate
    /// (§13.4: checked when loaded, not stamped when made). A code that
    /// does not read, or no longer passes, keeps its slot, shown dimmed.
    pub(crate) checked: Vec<(u8, Option<Layout>, bool)>,
}

impl LayoutsState {
    /// Take `slots` as what is on disk, and check every slot.
    pub(crate) fn set(&mut self, slots: Slots) {
        self.checked = slots
            .entries
            .iter()
            .map(|e| {
                let layout = e
                    .layout()
                    .ok()
                    .filter(|l| l.is_duel() && l.width() == LAYOUT_WIDTH);
                let ok = layout
                    .as_ref()
                    .is_some_and(|l| crate::custom::validate(l).is_ok());
                (e.slot, layout, ok)
            })
            .collect();
        self.slots = slots;
    }

    /// Slot `slot`'s layout and whether it passes.
    pub(crate) fn checked(&self, slot: u8) -> Option<(&Option<Layout>, bool)> {
        self.checked
            .iter()
            .find(|(s, _, _)| *s == slot)
            .map(|(_, l, ok)| (l, *ok))
    }
}

/// Where a ground the pre-match screen picked came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GroundSource {
    Bundled(usize),
    Slot(u8),
    /// A match resumed on a layout that is no longer bundled or saved.
    Other,
}

/// A layout the pre-match screen's GROUND row has picked.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct GroundPick {
    pub(crate) source: GroundSource,
    pub(crate) layout: Layout,
    /// It passes the duel gate; a match will not start on it otherwise.
    pub(crate) playable: bool,
}

impl GroundPick {
    /// What the GROUND row says: the name (or UNNAMED) and fingerprint.
    pub(crate) fn label(&self) -> String {
        let name = self.layout.name.as_str();
        let name = if name.is_empty() { "UNNAMED" } else { name };
        format!("{name} {}", fingerprint(&self.layout))
    }
}

/// What a TEST FIRE frame shows: who fires, the wind, the shots, a shot in
/// the air, the aim.
type TestPrint = (Side, usize, u32, bool, (i32, i32));

/// What an editor frame shows, for `FrameState`.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Fingerprint {
    page: Page,
    rev: Option<u64>,
    tools: (Tool, Brush, Stamp, WindField, bool, u16),
    list: (usize, usize, usize),
    keypad: (usize, u32),
    toast: bool,
    test: Option<TestPrint>,
}

/// The editor's state beside the rest of the game's.
pub(crate) struct EditorState {
    /// The draft being edited, if there is one.
    pub(crate) ed: Option<Editor>,
    pub(crate) page: Page,
    /// The row a keyboard or controller is on, on the start page and menu.
    pub(crate) focus: usize,
    /// What the keypad has typed (FORK A SEED, NAME).
    pub(crate) text: String,
    /// FORK A SEED's round, from 0.
    pub(crate) round: u32,
    /// The slot list's chosen row and the first row shown.
    pub(crate) list_sel: usize,
    pub(crate) list_top: usize,
    pub(crate) toast: Option<Toast>,
    pub(crate) test: Option<TestFire>,
    /// The draft's revision the draft file holds; `None` for none.
    pub(crate) written: Option<u64>,
    /// The draft's revision last saved to a slot, after which the draft
    /// file was cleared.
    pub(crate) saved: Option<u64>,
    /// A draft file from a newer build was left in place unread.
    pub(crate) newer_draft: bool,
}

impl Default for EditorState {
    fn default() -> Self {
        EditorState {
            ed: None,
            page: Page::Start,
            focus: 0,
            text: String::new(),
            round: 0,
            list_sel: 0,
            list_top: 0,
            toast: None,
            test: None,
            written: None,
            saved: None,
            newer_draft: false,
        }
    }
}

impl EditorState {
    /// A test shot is in the air: frames are drawn at the full rate.
    pub(crate) fn testing_in_flight(&self) -> bool {
        self.page == Page::Test && self.test.as_ref().is_some_and(|t| t.anim.is_some())
    }

    /// The draft has changes no slot holds.
    pub(crate) fn unsaved(&self) -> bool {
        self.ed.as_ref().is_some_and(|e| self.saved != Some(e.rev))
    }
}

// ------------------------------------------------------- launch and disk

impl Game {
    /// At launch: the slot file's text, if there is one (the adapter picks
    /// between it and its temporary file, as for the save).
    pub fn set_layouts(&mut self, text: Option<&str>) {
        self.layouts.set(text.map(Slots::parse).unwrap_or_default());
    }

    /// At launch, after the match: the draft kept on disk, set aside by the
    /// adapter as the match is. A draft that reads is restored and the
    /// editor will open on it; one from a newer build is put back unread.
    pub fn saved_draft(&mut self, found: SavedRun) -> Vec<Effect> {
        let text = match found {
            SavedRun::Interrupted => {
                self.fx.push(Effect::DraftFile(DraftFile::Abandon(
                    Abandoned::Interrupted,
                )));
                return self.take_effects();
            }
            SavedRun::Found(text) => text,
        };
        if text.trim().is_empty() {
            self.fx.push(Effect::DraftFile(DraftFile::Discard));
            return self.take_effects();
        }
        match SavedDraft::parse(&text) {
            Ok(saved) => {
                let ed = Editor::with_mirror(saved.draft, saved.mirror);
                self.editor.written = Some(ed.rev);
                self.editor.ed = Some(ed);
                self.fx.push(Effect::DraftFile(DraftFile::Restore));
            }
            Err(resume::Stale::NewerVersion) => {
                self.editor.newer_draft = true;
                self.fx.push(Effect::DraftFile(DraftFile::Restore));
            }
            Err(why) => {
                self.fx.push(Effect::DraftFile(DraftFile::Abandon(
                    Abandoned::Unreadable(why),
                )));
                self.note_at_launch("MAP DRAFT COULD NOT BE RESTORED");
            }
        }
        self.take_effects()
    }

    /// Keep the draft on disk whenever it has changed (see "The draft is
    /// never lost"): checked every frame, written once a stroke has ended,
    /// never during the demo.
    pub(super) fn persist_draft(&mut self) {
        if self.in_demo() {
            return;
        }
        let Some(ed) = self.editor.ed.as_ref() else {
            return;
        };
        if ed.stroking() || self.editor.written == Some(ed.rev) || self.editor.saved == Some(ed.rev)
        {
            return;
        }
        self.editor.written = Some(ed.rev);
        let saved = SavedDraft {
            draft: ed.draft.clone(),
            mirror: ed.mirror,
        };
        if let Some(line) = saved.to_line() {
            self.fx.push(Effect::DraftFile(DraftFile::Write(line)));
        }
    }

    /// Drop the draft and clear it from disk.
    fn clear_draft(&mut self) {
        self.editor.ed = None;
        self.editor.written = None;
        self.editor.saved = None;
        if !self.in_demo() {
            self.fx.push(Effect::DraftFile(DraftFile::Discard));
        }
    }

    /// The slot file reached disk, or did not (`Store::Layouts`).
    pub(super) fn layouts_stored(&mut self, text: String, why: SlotWrite, ok: bool) {
        if !ok {
            self.note(match why {
                SlotWrite::Save { .. } => "MAP NOT SAVED - DRAFT KEPT".into(),
                SlotWrite::Delete { .. } | SlotWrite::Undo => "MAPS NOT SAVED".into(),
            });
            return;
        }
        self.layouts.set(Slots::parse(&text));
        match why {
            SlotWrite::Save {
                slot,
                replaced,
                before,
            } => {
                if let Some(ed) = self.editor.ed.as_mut() {
                    ed.saved_to(slot);
                    self.editor.saved = Some(ed.rev);
                    self.editor.written = None;
                    self.fx.push(Effect::DraftFile(DraftFile::Discard));
                }
                if replaced {
                    self.editor.toast = Some(Toast {
                        text: format!("SLOT {slot} REPLACED"),
                        undo: ToastUndo::Slots(before),
                    });
                } else {
                    self.editor.toast = None;
                    self.note(format!("SAVED TO SLOT {slot}"));
                }
            }
            SlotWrite::Delete { slot, before } => {
                self.editor.toast = Some(Toast {
                    text: format!("SLOT {slot} DELETED"),
                    undo: ToastUndo::Slots(before),
                });
            }
            SlotWrite::Undo => {
                self.editor.toast = None;
                self.note("UNDONE".into());
            }
        }
        self.fx.push(Effect::Redraw);
    }

    /// Ask for the slot file to become `slots`, for `why`.
    fn write_slots(&mut self, slots: &Slots, why: SlotWrite) {
        if self.in_demo() {
            return;
        }
        if self.layouts.slots.from_the_future {
            self.note("MAPS FILE IS FROM A NEWER VERSION".into());
            return;
        }
        self.fx.push(Effect::Store(Store::Layouts {
            text: slots.serialize(),
            why,
        }));
    }

    /// Text from the clipboard, for PASTE CODE: read and checked, then
    /// loaded into the editor as the draft (an edit, so UNDO takes it
    /// back). A layout that reads but does not pass is loaded anyway, its
    /// problems shown, to be fixed; text that is not a duel layout of the
    /// layout width is refused with a note.
    pub fn pasted(&mut self, text: &str) -> Vec<Effect> {
        if self.mode != Mode::Editor || self.in_demo() {
            return self.take_effects();
        }
        match Layout::from_code(text) {
            Err(_) => self.note("NOT A MAP CODE".into()),
            Ok(l) if !l.is_duel() => self.note("ONE-GUN MAPS COME LATER".into()),
            Ok(l) if l.width() != LAYOUT_WIDTH => self.note("MAP IS NOT 320 WIDE".into()),
            Ok(layout) => {
                let ok = model::verdict(&layout).playable();
                self.start_draft(Draft {
                    layout,
                    from: None,
                    slot: None,
                });
                self.note(if ok {
                    "MAP PASTED".into()
                } else {
                    "MAP PASTED - IT HAS PROBLEMS".into()
                });
            }
        }
        self.take_effects()
    }
}

// --------------------------------------------------------- in and out

impl Game {
    /// Open the editor: on the kept draft if there is one, else its start
    /// page. Drawn at the layout width, letterboxed on a wider phone.
    pub(crate) fn open_editor(&mut self) {
        self.mode = Mode::Editor;
        self.editor.page = if self.editor.ed.is_some() {
            Page::Edit
        } else {
            Page::Start
        };
        self.editor.focus = 0;
        self.editor.test = None;
        self.editor.toast = None;
        self.input.touch.cancel_drag();
        self.set_run_width(Some(LAYOUT_WIDTH));
        self.apply_buffer_size();
    }

    /// `--salt-editor`, with `--salt-layout`'s code loaded if given and
    /// `--salt-editor-page`'s page open.
    pub(super) fn open_editor_from_dev(&mut self, code: Option<&str>, page: Option<&str>) {
        self.open_editor();
        if let Some(code) = code {
            let fx = self.pasted(code);
            self.fx.extend(fx);
        }
        match page {
            Some("start") => self.goto(Page::Start),
            Some("menu") => self.goto(Page::Menu),
            Some("slots") => self.goto(Page::Slots(Purpose::Open)),
            Some("save") => self.goto(Page::Slots(Purpose::Save)),
            Some("seed") => {
                self.editor.text = "DUNE".into();
                self.goto(Page::Seed);
            }
            Some("name") => self.editor_row(RowAct::Name),
            Some("test") => self.begin_test(),
            _ => {}
        }
    }

    /// `--salt-setup`: the pre-match screen, `code`'s layout picked on the
    /// GROUND row if given.
    pub(super) fn open_setup_from_dev(&mut self, code: Option<&str>) {
        self.open_versus_setup();
        if let Some(layout) = code.and_then(|c| Layout::from_code(c).ok()) {
            self.versus.ground = Some(self.ground_for(&layout));
        }
    }

    /// Leave the editor: to the pre-match screen, or to the title if a
    /// match was left suspended to fork it (RESUME MATCH is there). A draft
    /// a slot already holds is done with; any other stays kept.
    pub(crate) fn leave_editor(&mut self) {
        if let Some(ed) = self.editor.ed.as_mut() {
            ed.end_stroke();
        }
        if !self.editor.unsaved() && self.editor.ed.is_some() {
            self.editor.ed = None;
            self.editor.written = None;
            self.editor.saved = None;
        }
        self.editor.test = None;
        self.editor.toast = None;
        self.input.touch.cancel_drag();
        self.mode = if self.saved_match.is_some() {
            Mode::Title
        } else {
            Mode::VersusSetup
        };
        self.refresh_ground();
        self.apply_buffer_size();
    }

    /// Start editing `draft`: over the draft there is, as an undoable
    /// edit, or as a new one.
    pub(crate) fn start_draft(&mut self, draft: Draft) {
        match self.editor.ed.as_mut() {
            Some(ed) => ed.replace(draft),
            None => {
                self.editor.ed = Some(Editor::new(draft));
                self.editor.written = None;
                self.editor.saved = None;
            }
        }
        self.editor.page = Page::Edit;
        self.editor.test = None;
    }

    /// MAKE MAP OF THIS ROUND: the round on screen forked (cropped to the
    /// layout width, `editor::crop_live`) into the editor. An unfinished
    /// match is suspended, not lost: RESUME MATCH is offered on the title.
    pub(crate) fn fork_this_round(&mut self) {
        if self.in_demo() {
            return;
        }
        let Some(vs) = self.versus.live.as_ref() else {
            return;
        };
        let wind = vs
            .layout
            .as_ref()
            .map_or_else(|| crate::custom::wind_spec(&vs.duel.rules), |l| l.wind);
        let draft = model::fork_live(&vs.duel.screen, vs.duel.seed, vs.duel.round, wind);
        let over = matches!(vs.duel.phase, versus::Phase::MatchOver(_));
        if over {
            self.drop_live_match();
        } else if let Some(vs) = self.versus.live.take() {
            self.suspend_live_match(vs);
        }
        self.open_editor();
        self.start_draft(draft);
    }

    /// Which editor page is up, if any: a change spends the touches in
    /// progress (`settle_touch`), as a versus screen change does.
    pub(super) fn editor_screen(&self) -> Option<Page> {
        (self.mode == Mode::Editor).then_some(self.editor.page)
    }

    /// What an editor frame shows, for `FrameState`.
    pub(super) fn editor_fingerprint(&self) -> Option<Fingerprint> {
        if self.mode != Mode::Editor {
            return None;
        }
        let e = &self.editor;
        let tools = e.ed.as_ref().map_or(
            (
                Tool::Sculpt,
                Brush::Medium,
                Stamp::Ridge,
                WindField::Max,
                false,
                0,
            ),
            |ed| {
                (
                    ed.tool,
                    ed.brush,
                    ed.stamp,
                    ed.wind_field,
                    ed.mirror,
                    ed.cursor,
                )
            },
        );
        Some(Fingerprint {
            page: e.page,
            rev: e.ed.as_ref().map(|ed| ed.rev),
            tools,
            list: (e.focus, e.list_sel, e.list_top),
            keypad: (e.text.len(), e.round),
            toast: e.toast.is_some(),
            test: e.test.as_ref().map(|t| {
                (
                    t.side,
                    t.wind_i,
                    t.shots,
                    t.anim.is_some(),
                    (self.play.angle_ticks, self.play.power_ticks),
                )
            }),
        })
    }
}

// ----------------------------------------------------------------- input

impl Game {
    /// A touch button of the editor's pressed this frame.
    fn ed_pressed(&self, ed: Ed) -> bool {
        self.input.touch.pressed(Action::Edit(ed))
    }

    /// The rows of the start page or the menu, for input and drawing alike.
    pub(crate) fn editor_rows(&self) -> Vec<(RowAct, String)> {
        let e = &self.editor;
        match e.page {
            Page::Start => {
                let mut rows = Vec::new();
                if e.ed.is_some() {
                    rows.push((RowAct::Continue, "BACK TO MY MAP".to_string()));
                }
                rows.push((RowAct::Blank, "NEW BLANK MAP".into()));
                rows.push((RowAct::ForkSeed, "FORK A SEED".into()));
                rows.push((RowAct::Open, "OPEN A MAP".into()));
                rows.push((RowAct::Paste, "PASTE A CODE".into()));
                rows.push((RowAct::Leave, "BACK".into()));
                rows
            }
            Page::Menu => {
                let mirror = e.ed.as_ref().is_some_and(|ed| ed.mirror);
                let mut rows = vec![
                    (RowAct::ToMap, "BACK TO MAP".to_string()),
                    (RowAct::Save, "SAVE".into()),
                    (RowAct::Name, "NAME".into()),
                ];
                // A touchscreen has these on the editing screen itself.
                if !self.input.touch_active {
                    rows.push((RowAct::Test, "TEST FIRE".into()));
                    rows.push((
                        RowAct::Mirror,
                        format!("MIRROR: {}", if mirror { "ON" } else { "OFF" }),
                    ));
                    rows.push((RowAct::Undo, "UNDO".into()));
                    rows.push((RowAct::Redo, "REDO".into()));
                }
                rows.push((RowAct::Copy, "COPY CODE".into()));
                rows.push((RowAct::Paste, "PASTE CODE".into()));
                rows.push((RowAct::NewMap, "NEW MAP".into()));
                rows.push((RowAct::Discard, "DISCARD MAP".into()));
                rows.push((RowAct::Leave, "EXIT EDITOR".into()));
                rows
            }
            _ => Vec::new(),
        }
    }

    /// The slot list's rows: the bundled layouts and the slots to open, or
    /// the first free slot and the slots to save over.
    pub(crate) fn slot_rows(&self, purpose: Purpose) -> Vec<SlotRow> {
        let mut rows = Vec::new();
        match purpose {
            Purpose::Open => {
                rows.extend((0..crate::custom::bundled::BUNDLED.len()).map(SlotRow::Bundled));
            }
            Purpose::Save => {
                if let Some(free) = self.layouts.slots.free() {
                    rows.push(SlotRow::New(free));
                }
            }
        }
        rows.extend(
            self.layouts
                .slots
                .entries
                .iter()
                .map(|e| SlotRow::Slot(e.slot)),
        );
        rows
    }

    /// What a slot row says, and whether it is dimmed (a layout that does
    /// not read or no longer passes).
    pub(crate) fn slot_row_text(&self, row: SlotRow) -> (String, bool) {
        match row {
            SlotRow::Bundled(i) => {
                let l = crate::custom::bundled::layouts().get(i).cloned();
                match l {
                    Some(l) => (
                        format!("MAP  {}  {}", l.name.as_str(), fingerprint(&l)),
                        false,
                    ),
                    None => ("MAP".into(), true),
                }
            }
            SlotRow::New(n) => (format!("{n:>2}  NEW SLOT"), false),
            SlotRow::Slot(n) => match self.layouts.checked(n) {
                Some((Some(l), ok)) => {
                    let name = l.name.as_str();
                    let name = if name.is_empty() {
                        self.layouts
                            .slots
                            .get(n)
                            .and_then(|e| e.origin.as_ref())
                            .map_or("UNNAMED".to_string(), |o| o.label())
                    } else {
                        name.to_string()
                    };
                    let mark = if ok { "" } else { "  ! FAILS" };
                    (format!("{n:>2}  {name}  {}{mark}", fingerprint(l)), !ok)
                }
                _ => (format!("{n:>2}  NEWER VERSION"), true),
            },
        }
    }

    /// The map editor's input for this frame.
    pub(super) fn handle_editor_input(&mut self, delta: f64) {
        if self.ed_pressed(Ed::ToastUndo) {
            self.toast_undo();
            return;
        }
        match self.editor.page {
            Page::Start | Page::Menu => self.handle_editor_rows(),
            Page::Seed | Page::Name => self.handle_editor_keypad(delta),
            Page::Slots(p) => self.handle_editor_slots(p),
            Page::Edit => self.handle_editor_edit(delta),
            Page::Test => self.handle_editor_test(delta),
        }
    }

    /// The start page and the menu: rows, by touch or by a focus.
    fn handle_editor_rows(&mut self) {
        let rows = self.editor_rows();
        let n = rows.len().max(1);
        let mut act = None;
        for (i, (a, _)) in rows.iter().enumerate() {
            if self.ed_pressed(Ed::Row(i as u8)) {
                act = Some(*a);
            }
        }
        if self.action_pressed(Action::Down) {
            self.editor.focus = (self.editor.focus + 1) % n;
        } else if self.action_pressed(Action::Up) {
            self.editor.focus = (self.editor.focus + n - 1) % n;
        }
        self.editor.focus = self.editor.focus.min(n - 1);
        if act.is_none() && (self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter)) {
            act = rows.get(self.editor.focus).map(|r| r.0);
        }
        if act.is_none() && self.action_pressed(Action::Back) {
            act = Some(if self.editor.page == Page::Menu {
                RowAct::ToMap
            } else {
                RowAct::Leave
            });
        }
        if self.editor.page == Page::Menu && self.pressed(Key::P) {
            act = Some(RowAct::Paste);
        }
        if let Some(act) = act {
            self.play(Sound::Ui);
            self.editor_row(act);
        }
    }

    fn goto(&mut self, page: Page) {
        self.editor.page = page;
        self.editor.focus = 0;
    }

    fn editor_row(&mut self, act: RowAct) {
        match act {
            RowAct::Continue | RowAct::ToMap => self.goto(Page::Edit),
            RowAct::Blank => self.start_draft(Draft::blank()),
            RowAct::ForkSeed => {
                self.editor.text.clear();
                self.editor.round = 0;
                self.input.seed_focus = Focus::default();
                self.goto(Page::Seed);
            }
            RowAct::Open => {
                self.editor.list_sel = 0;
                self.editor.list_top = 0;
                self.goto(Page::Slots(Purpose::Open));
            }
            RowAct::Paste => self.fx.push(Effect::ReadClipboard),
            RowAct::Leave => self.leave_editor(),
            RowAct::Save => self.begin_save(),
            RowAct::Name => {
                self.editor.text = self
                    .editor
                    .ed
                    .as_ref()
                    .map(|e| e.layout().name.as_str().to_string())
                    .unwrap_or_default();
                self.input.seed_focus = Focus::default();
                self.goto(Page::Name);
            }
            RowAct::Test => self.begin_test(),
            RowAct::Mirror => {
                if let Some(ed) = self.editor.ed.as_mut() {
                    ed.toggle_mirror();
                }
            }
            RowAct::Undo => self.editor_undo(false),
            RowAct::Redo => self.editor_undo(true),
            RowAct::Copy => self.copy_draft(),
            RowAct::NewMap => self.goto(Page::Start),
            RowAct::Discard => self.discard_draft(),
        }
    }

    fn editor_undo(&mut self, redo: bool) {
        let Some(ed) = self.editor.ed.as_mut() else {
            return;
        };
        let done = if redo { ed.redo() } else { ed.undo() };
        if done {
            self.play(Sound::Ui);
        } else {
            self.note(
                if redo {
                    "NOTHING TO REDO"
                } else {
                    "NOTHING TO UNDO"
                }
                .into(),
            );
        }
    }

    fn copy_draft(&mut self) {
        let Some(code) = self
            .editor
            .ed
            .as_ref()
            .and_then(|e| e.layout().to_code().ok())
        else {
            return;
        };
        self.fx.push(Effect::Clipboard(code));
        self.note("CODE COPIED".into());
    }

    /// DISCARD MAP: the draft cleared from disk, with an UNDO toast on the
    /// start page that brings it back, history and all.
    fn discard_draft(&mut self) {
        let Some(ed) = self.editor.ed.take() else {
            return;
        };
        let undo = ToastUndo::Draft(Box::new(ed));
        self.clear_draft();
        self.editor.toast = Some(Toast {
            text: "MAP DISCARDED".into(),
            undo,
        });
        self.goto(Page::Start);
    }

    /// The toast's UNDO.
    fn toast_undo(&mut self) {
        let Some(toast) = self.editor.toast.take() else {
            return;
        };
        self.play(Sound::Ui);
        match toast.undo {
            ToastUndo::Slots(before) => {
                // The draft a replaced slot was saved over is a draft again.
                if let Some(ed) = self.editor.ed.as_ref() {
                    if self.editor.saved == Some(ed.rev) {
                        self.editor.saved = None;
                    }
                }
                let slots = Slots::parse(&before);
                self.write_slots(&slots, SlotWrite::Undo);
            }
            ToastUndo::Draft(ed) => {
                self.editor.ed = Some(*ed);
                self.editor.written = None;
                self.editor.saved = None;
                self.goto(Page::Edit);
            }
        }
    }

    /// FORK A SEED and NAME: the keypad, by touch, a controller's focus or
    /// typed keys (`editor_typed`).
    fn handle_editor_keypad(&mut self, delta: f64) {
        let dx = self.action_repeated(1, Action::Right, delta) as i32
            - self.action_repeated(0, Action::Left, delta) as i32;
        let dy = self.action_repeated(2, Action::Down, delta) as i32
            - self.action_repeated(3, Action::Up, delta) as i32;
        if dx != 0 || dy != 0 {
            self.input.seed_focus.step(dx, dy);
        }
        if self.editor.page == Page::Seed {
            if self.ed_pressed(Ed::RoundDown) {
                self.editor.round = self.editor.round.saturating_sub(1);
            } else if self.ed_pressed(Ed::RoundUp) {
                self.editor.round = (self.editor.round + 1).min(model::MAX_ROUND - 1);
            }
        }
        let key = if self.pad_action_pressed(Action::Confirm) {
            Some(self.input.seed_focus.action())
        } else if self.pad_action_pressed(Action::Back) {
            Some(if self.editor.text.is_empty() {
                Action::Back
            } else {
                keypad::DEL
            })
        } else if self.ed_pressed(Ed::Ok) {
            Some(Action::Confirm)
        } else if self.input.touch.pressed(Action::Back) || self.input.back_requested {
            Some(Action::Back)
        } else {
            [Action::Confirm, keypad::DEL]
                .into_iter()
                .chain(keypad::KEYS)
                .find(|k| self.input.touch.pressed(*k))
        };
        match key {
            Some(Action::Confirm) => self.keypad_ok(),
            Some(Action::Back) => self.keypad_back(),
            Some(keypad::DEL) => {
                self.editor.text.pop();
            }
            Some(Action::Type(c)) => self.keypad_type(c),
            _ => {}
        }
    }

    /// A key typed on FORK A SEED or NAME.
    pub(super) fn editor_typed(&mut self, key: Typed) {
        match key {
            Typed::Backspace => {
                self.editor.text.pop();
            }
            Typed::Enter => self.keypad_ok(),
            Typed::Escape => self.keypad_back(),
            Typed::Char(c) => self.keypad_type(c.to_ascii_uppercase()),
        }
    }

    /// Whether typed keys go to the editor's keypad now.
    pub(super) fn editor_typing(&self) -> bool {
        self.mode == Mode::Editor && matches!(self.editor.page, Page::Seed | Page::Name)
    }

    fn keypad_type(&mut self, c: char) {
        let max = if self.editor.page == Page::Name {
            model::NAME_MAX
        } else {
            SEED_MAX
        };
        let keypad = salt_line_sim::layout::LayoutName::CHARSET.contains(&(c as u8));
        if keypad && self.editor.text.chars().count() < max {
            self.editor.text.push(c);
        }
    }

    fn keypad_ok(&mut self) {
        match self.editor.page {
            Page::Seed => {
                if self.editor.text.is_empty() {
                    self.note("TYPE A SEED FIRST".into());
                    return;
                }
                let draft = Draft::from_seed(&self.editor.text, self.editor.round);
                self.play(Sound::Ui);
                self.start_draft(draft);
            }
            Page::Name => {
                let text = self.editor.text.clone();
                if let Some(ed) = self.editor.ed.as_mut() {
                    ed.set_name(&text);
                }
                self.play(Sound::Ui);
                self.goto(Page::Edit);
            }
            _ => {}
        }
    }

    fn keypad_back(&mut self) {
        let back = if self.editor.page == Page::Name {
            Page::Edit
        } else {
            Page::Start
        };
        self.goto(back);
    }

    /// The slot list.
    fn handle_editor_slots(&mut self, purpose: Purpose) {
        let rows = self.slot_rows(purpose);
        let n = rows.len();
        let mut chosen = None;
        for i in 0..VISIBLE_ROWS {
            if self.ed_pressed(Ed::Row(i as u8)) && self.editor.list_top + i < n {
                chosen = Some(self.editor.list_top + i);
            }
        }
        if self.action_pressed(Action::Down) && n > 0 {
            self.editor.list_sel = (self.editor.list_sel + 1).min(n - 1);
        } else if self.action_pressed(Action::Up) {
            self.editor.list_sel = self.editor.list_sel.saturating_sub(1);
        }
        if self.ed_pressed(Ed::PageDown) {
            self.editor.list_top =
                (self.editor.list_top + VISIBLE_ROWS).min(n.saturating_sub(VISIBLE_ROWS));
            self.editor.list_sel = self.editor.list_sel.max(self.editor.list_top);
        } else if self.ed_pressed(Ed::PageUp) {
            self.editor.list_top = self.editor.list_top.saturating_sub(VISIBLE_ROWS);
            self.editor.list_sel = self
                .editor
                .list_sel
                .min(self.editor.list_top + VISIBLE_ROWS - 1);
        }
        self.editor.list_sel = self.editor.list_sel.min(n.saturating_sub(1));
        // Keep the chosen row in view.
        if self.editor.list_sel < self.editor.list_top {
            self.editor.list_top = self.editor.list_sel;
        } else if self.editor.list_sel >= self.editor.list_top + VISIBLE_ROWS {
            self.editor.list_top = self.editor.list_sel + 1 - VISIBLE_ROWS;
        }
        if self.action_pressed(Action::Back) || self.ed_pressed(Ed::Back) {
            self.play(Sound::Ui);
            self.goto(match purpose {
                Purpose::Open => Page::Start,
                Purpose::Save => Page::Menu,
            });
            return;
        }
        let confirm = self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter);
        match purpose {
            Purpose::Save => {
                // A tap on a row saves there: the page change before it
                // spent the tap that opened the list.
                let pick = chosen.or(confirm.then_some(self.editor.list_sel));
                if let Some(row) = pick.and_then(|i| rows.get(i).copied()) {
                    self.play(Sound::Ui);
                    match row {
                        SlotRow::New(s) | SlotRow::Slot(s) => self.save_to(s),
                        SlotRow::Bundled(_) => {}
                    }
                }
            }
            Purpose::Open => {
                if let Some(i) = chosen {
                    self.editor.list_sel = i;
                }
                let sel = rows.get(self.editor.list_sel).copied();
                if self.ed_pressed(Ed::Open) || confirm {
                    if let Some(row) = sel {
                        self.play(Sound::Ui);
                        self.open_row(row);
                    }
                } else if self.ed_pressed(Ed::Copy) || self.action_pressed(Action::Copy) {
                    if let Some(code) = sel.and_then(|r| self.row_code(r)) {
                        self.fx.push(Effect::Clipboard(code));
                        self.note("CODE COPIED".into());
                    }
                } else if self.ed_pressed(Ed::Delete) || self.pressed(Key::X) {
                    if let Some(SlotRow::Slot(s)) = sel {
                        self.play(Sound::Ui);
                        self.delete_slot(s);
                    }
                }
            }
        }
    }

    fn row_code(&self, row: SlotRow) -> Option<String> {
        match row {
            SlotRow::Bundled(i) => crate::custom::bundled::BUNDLED
                .get(i)
                .map(|c| c.to_string()),
            SlotRow::Slot(s) => self.layouts.slots.get(s).map(|e| e.code.clone()),
            SlotRow::New(_) => None,
        }
    }

    /// Open a bundled layout (as a new draft of its own) or a slot (a
    /// draft that saves back to it). One that does not read cannot be.
    fn open_row(&mut self, row: SlotRow) {
        let (layout, from, slot) = match row {
            SlotRow::Bundled(i) => match crate::custom::bundled::layouts().get(i) {
                Some(l) => (l.clone(), None, None),
                None => return,
            },
            SlotRow::Slot(s) => match self.layouts.checked(s) {
                Some((Some(l), _)) => (
                    l.clone(),
                    self.layouts.slots.get(s).and_then(|e| e.origin.clone()),
                    Some(s),
                ),
                _ => {
                    self.note("THIS MAP IS FROM A NEWER VERSION".into());
                    return;
                }
            },
            SlotRow::New(_) => return,
        };
        self.start_draft(Draft { layout, from, slot });
    }

    /// SAVE: refused while the draft does not pass; otherwise the slot
    /// list, on the slot it came from (or the first free one).
    fn begin_save(&mut self) {
        let Some(ed) = self.editor.ed.as_ref() else {
            return;
        };
        if !ed.verdict.playable() {
            let why = ed
                .verdict
                .problems
                .iter()
                .find(|(_, s)| *s == model::Severity::Block)
                .map_or_else(String::new, |(p, _)| problem_words(p));
            self.note(format!("CAN'T SAVE: {why}"));
            return;
        }
        let rows = self.slot_rows(Purpose::Save);
        let own = ed.draft.slot;
        self.editor.list_sel = rows
            .iter()
            .position(|r| matches!(r, SlotRow::Slot(s) if Some(*s) == own))
            .unwrap_or(0);
        self.editor.list_top = self.editor.list_sel.saturating_sub(VISIBLE_ROWS - 1);
        self.goto(Page::Slots(Purpose::Save));
    }

    /// Save the draft into `slot`. Memory and the draft follow once the
    /// write is confirmed (`layouts_stored`).
    pub(crate) fn save_to(&mut self, slot: u8) {
        let Some(ed) = self.editor.ed.as_ref() else {
            return;
        };
        if !ed.verdict.playable() {
            return;
        }
        let Ok(code) = ed.layout().to_code() else {
            return;
        };
        let before = self.layouts.slots.serialize();
        let mut slots = self.layouts.slots.clone();
        let old = slots.put(SlotEntry {
            slot,
            code: code.clone(),
            origin: ed.draft.from.clone(),
            tail: None,
        });
        let replaced = old.is_some_and(|o| o.code != code);
        self.write_slots(
            &slots,
            SlotWrite::Save {
                slot,
                replaced,
                before,
            },
        );
        self.goto(Page::Edit);
    }

    fn delete_slot(&mut self, slot: u8) {
        let before = self.layouts.slots.serialize();
        let mut slots = self.layouts.slots.clone();
        if slots.remove(slot).is_none() {
            return;
        }
        self.write_slots(&slots, SlotWrite::Delete { slot, before });
    }

    /// The editing screen.
    fn handle_editor_edit(&mut self, delta: f64) {
        if self.editor.ed.is_none() {
            self.goto(Page::Start);
            return;
        }
        if self.ed_pressed(Ed::Menu) || self.action_pressed(Action::Back) {
            self.play(Sound::Ui);
            self.end_editor_stroke();
            self.goto(Page::Menu);
            return;
        }
        if self.ed_pressed(Ed::Test) || self.pressed(Key::T) {
            self.end_editor_stroke();
            self.begin_test();
            return;
        }
        if self.ed_pressed(Ed::Undo) || self.pressed(Key::Z) {
            self.editor_undo(false);
        } else if self.ed_pressed(Ed::Redo) || self.pressed(Key::Y) {
            self.editor_undo(true);
        }
        if self.pressed(Key::C) {
            self.copy_draft();
        } else if self.pressed(Key::P) {
            self.fx.push(Effect::ReadClipboard);
        }
        let shift = self.shift_held();
        let tool_step = self.action_pressed(Action::FuseUp) as i32
            - self.action_pressed(Action::FuseDown) as i32;
        let opt_step = self.action_pressed(Action::RepositionRight) as i32
            - self.action_pressed(Action::RepositionLeft) as i32;
        let left = self.action_repeated(0, Action::Left, delta) as i32;
        let right = self.action_repeated(1, Action::Right, delta) as i32;
        let down = self.action_repeated(2, Action::Down, delta) as i32;
        let up = self.action_repeated(3, Action::Up, delta) as i32;
        let apply = self.action_pressed(Action::Confirm)
            || self.action_pressed(Action::Fire)
            || self.pressed(Key::KpEnter);
        let mirror = self.ed_pressed(Ed::Mirror) || self.pressed(Key::I);
        let mut pressed = Vec::new();
        for ed in edit_buttons() {
            if self.ed_pressed(ed) {
                pressed.push(ed);
            }
        }
        let drag = self.input.touch.drag();
        let dropped = self.input.touch.dropped();
        let Some(ed) = self.editor.ed.as_mut() else {
            return;
        };
        if mirror {
            ed.toggle_mirror();
        }
        if tool_step != 0 {
            ed.set_tool(ed.tool.step(tool_step));
        }
        for b in pressed {
            match b {
                Ed::Tool(t) => ed.set_tool(t),
                Ed::Brush(b) => ed.brush = b,
                Ed::Stamp(s) => ed.stamp = s,
                Ed::WindMode(m) => ed.set_wind_mode(m),
                Ed::WindField(f) => ed.wind_field = f,
                Ed::WindDown => ed.step_wind(-1),
                Ed::WindUp => ed.step_wind(1),
                _ => {}
            }
        }
        if opt_step != 0 {
            step_option(ed, opt_step);
        }
        let mult = if shift { 5 } else { 1 };
        if ed.tool == Tool::Wind {
            if left + right != 0 {
                ed.step_wind((right - left) * mult);
            }
            if up + down != 0 {
                ed.wind_field = next_field(ed, up - down);
            }
        } else {
            if left + right != 0 {
                ed.move_cursor((right - left) * mult);
            }
            if up + down != 0 && ed.tool != Tool::Guns {
                for _ in 0..(up + down) {
                    ed.nudge(if up > 0 { 1 } else { -1 });
                }
            }
            if apply && matches!(ed.tool, Tool::Stamp | Tool::Smooth) {
                ed.nudge(1);
            }
        }
        // A drag on the ground is a stroke with the tool in hand.
        match (drag, dropped) {
            (_, Some((start, end))) => {
                if !ed.stroking() {
                    ed.begin_stroke(start.0, start.1);
                }
                ed.drag_stroke(end.0, end.1);
                ed.end_stroke();
            }
            (Some((start, now)), None) => {
                if !ed.stroking() {
                    ed.begin_stroke(start.0, start.1);
                }
                ed.drag_stroke(now.0, now.1);
            }
            (None, None) => ed.end_stroke(),
        }
    }

    fn end_editor_stroke(&mut self) {
        self.input.touch.cancel_drag();
        if let Some(ed) = self.editor.ed.as_mut() {
            ed.end_stroke();
        }
    }

    /// TEST FIRE: refused while the guns cannot be stood where they are.
    fn begin_test(&mut self) {
        let Some(ed) = self.editor.ed.as_ref() else {
            return;
        };
        let placed = ed
            .verdict
            .problems
            .iter()
            .all(|(p, _)| matches!(p, salt_line_sim::layout::Problem::Unsolvable { .. }));
        if !placed {
            self.note("FIX THE GUNS FIRST".into());
            return;
        }
        let test = TestFire::new(ed.layout());
        (self.play.angle_ticks, self.play.power_ticks) = test.aims[0];
        self.editor.test = Some(test);
        self.input.drag_from = None;
        self.goto(Page::Test);
    }

    /// TEST FIRE's input: who fires, the wind, the aim and FIRE. Leaving
    /// throws the craters away.
    fn handle_editor_test(&mut self, delta: f64) {
        if self.action_pressed(Action::Back) || self.ed_pressed(Ed::Back) {
            self.play(Sound::Ui);
            self.editor.test = None;
            self.input.touch.cancel_drag();
            self.goto(Page::Edit);
            return;
        }
        let Some(t) = self.editor.test.as_ref() else {
            self.goto(Page::Edit);
            return;
        };
        if t.anim.is_some() {
            self.input.touch.cancel_drag();
            return;
        }
        let side_switch = self.ed_pressed(Ed::TestSide)
            || self.action_pressed(Action::RepositionLeft)
            || self.action_pressed(Action::RepositionRight);
        let wind_step = self.ed_pressed(Ed::TestWind) as i32
            + self.action_pressed(Action::FuseUp) as i32
            - self.action_pressed(Action::FuseDown) as i32;
        let aim = (self.play.angle_ticks, self.play.power_ticks);
        if let Some(t) = self.editor.test.as_mut() {
            if side_switch {
                t.aims[t.side.index()] = aim;
                t.side = t.side.other();
                (self.play.angle_ticks, self.play.power_ticks) = t.aims[t.side.index()];
                self.input.drag_from = None;
            }
            if wind_step != 0 {
                let n = t.winds.len() as i32;
                t.wind_i = (t.wind_i as i32 + wind_step).rem_euclid(n) as usize;
                t.screen.wind = wind_units(t.wind());
            }
        }
        let mirrored = self
            .editor
            .test
            .as_ref()
            .is_some_and(|t| t.side == Side::Right);
        let quantum = Loadout::new().angle_quantum_ticks.max(1);
        let mult = if self.shift_held() { 10 } else { 1 };
        let left = self.action_repeated(0, Action::Left, delta) as i32;
        let right = self.action_repeated(1, Action::Right, delta) as i32;
        if left + right != 0 {
            let steps = if mirrored { left - right } else { right - left };
            self.play.angle_ticks =
                (self.play.angle_ticks + steps * quantum * mult).clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, Action::Down, delta) as i32;
        let up = self.action_repeated(3, Action::Up, delta) as i32;
        if up + down != 0 {
            self.play.power_ticks =
                (self.play.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }
        let mut fire = self.action_pressed(Action::Fire) || self.pressed(Key::Enter);
        if self.drag_aiming() {
            fire |= self.drag_aim(quantum, mirrored, false);
        }
        if fire {
            self.test_fire();
        }
    }

    /// Fire the test gun's aim, as a match would (`versus::resolve`).
    fn test_fire(&mut self) {
        let aim = (self.play.angle_ticks, self.play.power_ticks);
        let Some(t) = self.editor.test.as_mut() else {
            return;
        };
        let lo = Loadout::new();
        let side = t.side;
        let before = t.screen.clone();
        let r = versus::resolve(
            &t.screen,
            side,
            Shot::new(aim.0, aim.1),
            &lo,
            versus::Rules::DEFAULT.hit_reach,
            true,
        );
        t.screen.field = r.field;
        t.aims[side.index()] = aim;
        t.arcs[side.index()] = r
            .traces
            .first()
            .map(|tr| tr.path.clone())
            .unwrap_or_default();
        t.landing[side.index()] = r.traces.first().and_then(|tr| tr.detonation());
        t.last = Some((
            side,
            if r.hit {
                VersusCall::Hit
            } else if r.self_hit {
                VersusCall::Oops
            } else {
                VersusCall::Miss
            },
        ));
        t.shots += 1;
        let longest = r.traces.iter().map(|tr| tr.flight_time).fold(0.0, f64::max);
        t.anim = Some(Anim {
            traces: r.traces,
            before,
            t: 0.0,
            longest,
            collapsed: Vec::new(),
            settled: 0.0,
            impact_played: false,
            collapse_played: false,
        });
        self.play(Sound::Fire);
        self.play(Sound::Flight);
    }

    /// The test shot's playback.
    pub(super) fn step_editor(&mut self, delta: f64) {
        if self.mode != Mode::Editor {
            return;
        }
        let mut sounds = Vec::new();
        if let Some(t) = self.editor.test.as_mut() {
            let hit = matches!(t.last, Some((_, VersusCall::Hit)));
            if let Some(anim) = t.anim.as_mut() {
                anim.t += delta;
                if anim.done() {
                    anim.settled += delta;
                    if !anim.impact_played {
                        anim.impact_played = true;
                        sounds.push(Sound::Terrain);
                        if hit {
                            sounds.push(Sound::Hit);
                        }
                    }
                    if anim.settled > VERSUS_HOLD {
                        t.anim = None;
                        self.fx.push(Effect::Redraw);
                    }
                }
            }
        }
        for s in sounds {
            self.play(s);
        }
    }
}

/// The editing screen's toolbar and options, every button the page may
/// draw: what `handle_editor_edit` listens for.
fn edit_buttons() -> Vec<Ed> {
    let mut b: Vec<Ed> = Tool::ALL.iter().map(|t| Ed::Tool(*t)).collect();
    b.extend(Brush::ALL.iter().map(|x| Ed::Brush(*x)));
    b.extend(Stamp::ALL.iter().map(|x| Ed::Stamp(*x)));
    b.extend(WindMode::ALL.iter().map(|x| Ed::WindMode(*x)));
    b.extend([
        Ed::WindField(WindField::Min),
        Ed::WindField(WindField::Max),
        Ed::WindField(WindField::Fixed),
        Ed::WindDown,
        Ed::WindUp,
    ]);
    b
}

/// `,` and `.` (pad X and Y): the current tool's options, in turn.
fn step_option(ed: &mut Editor, step: i32) {
    fn next<T: Copy + PartialEq>(all: &[T], now: T, step: i32) -> T {
        let i = all.iter().position(|x| *x == now).unwrap_or(0) as i32;
        all[(i + step).rem_euclid(all.len() as i32) as usize]
    }
    match ed.tool {
        Tool::Sculpt | Tool::Smooth => ed.brush = next(&Brush::ALL, ed.brush, step),
        Tool::Stamp => ed.stamp = next(&Stamp::ALL, ed.stamp, step),
        Tool::Wind => {
            let mode = next(&WindMode::ALL, WindMode::of(&ed.layout().wind), step);
            ed.set_wind_mode(mode);
        }
        Tool::Guns => {}
    }
}

/// Up and down on WIND: which number `-`/`+` moves (a range's ends).
fn next_field(ed: &Editor, step: i32) -> WindField {
    match ed.layout().wind {
        salt_line_sim::layout::WindSpec::Range { .. } => {
            if step > 0 {
                WindField::Max
            } else {
                WindField::Min
            }
        }
        salt_line_sim::layout::WindSpec::Fixed(_) => WindField::Fixed,
        salt_line_sim::layout::WindSpec::Calm => ed.wind_field,
    }
}

// ------------------------------------------------------- the GROUND row

impl Game {
    /// Every ground the pre-match screen offers after GENERATED: the
    /// bundled layouts, then the saved slots that read as duels.
    pub(crate) fn grounds(&self) -> Vec<GroundPick> {
        let mut out: Vec<GroundPick> = crate::custom::bundled::layouts()
            .into_iter()
            .enumerate()
            .map(|(i, layout)| GroundPick {
                source: GroundSource::Bundled(i),
                playable: crate::custom::validate(&layout).is_ok(),
                layout,
            })
            .collect();
        for (slot, layout, ok) in &self.layouts.checked {
            if let Some(layout) = layout {
                out.push(GroundPick {
                    source: GroundSource::Slot(*slot),
                    layout: layout.clone(),
                    playable: *ok,
                });
            }
        }
        out
    }

    /// The GROUND row moved by `step`: GENERATED, then each ground, round.
    pub(crate) fn step_ground(&mut self, step: i32) {
        let grounds = self.grounds();
        let n = grounds.len() as i32 + 1;
        let now = match &self.versus.ground {
            None => 0,
            Some(g) => grounds
                .iter()
                .position(|x| x.source == g.source && x.layout == g.layout)
                .map_or(0, |i| i as i32 + 1),
        };
        let next = (now + step).rem_euclid(n);
        self.versus.ground = (next > 0).then(|| grounds[next as usize - 1].clone());
    }

    /// The ground picked may have changed under the pre-match screen (a
    /// slot overwritten, deleted or saved in the editor): pick it again by
    /// where it came from, or fall back to GENERATED if it is gone.
    pub(crate) fn refresh_ground(&mut self) {
        let Some(g) = self.versus.ground.clone() else {
            return;
        };
        if g.source == GroundSource::Other {
            return;
        }
        self.versus.ground = self.grounds().into_iter().find(|x| x.source == g.source);
    }

    /// The pick for a match resumed on `layout`: its bundled or slot entry
    /// if one holds it, else on its own.
    pub(crate) fn ground_for(&self, layout: &Layout) -> GroundPick {
        self.grounds()
            .into_iter()
            .find(|g| &g.layout == layout)
            .unwrap_or_else(|| GroundPick {
                source: GroundSource::Other,
                layout: layout.clone(),
                playable: true,
            })
    }
}

#[cfg(test)]
pub(crate) fn test_rows(g: &Game) -> Vec<RowAct> {
    g.editor_rows().into_iter().map(|r| r.0).collect()
}

/// Used by the renderer: the draft's label and fingerprint.
pub(crate) fn draft_label(ed: &Editor) -> String {
    format!("{}  {}", ed.draft.label(), fingerprint(ed.layout()))
}

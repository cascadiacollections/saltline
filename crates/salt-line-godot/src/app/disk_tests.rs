//! Persistence end to end: the core's effects applied to an in-memory disk
//! the way the node applies them to `user://`, with the process killed after
//! every effect in turn and the game launched again on what was left.
//!
//! What must hold whatever the moment of the crash: a finished run is on the
//! board at most once, a saved run that is offered is the run as it was
//! played, and last session's crash report is never lost.
//!
//! # The technique
//!
//! Exhaustive fault injection, cheap because the core does no I/O of its
//! own. A flow (finish a run, quit mid-run, continue and finish) is played
//! once to count its operations, `n` (every effect, file or not, and the
//! rename at launch). It is then replayed `n + 1` times from the same
//! starting disk, with [`FakeDisk`]'s `budget` letting through only the
//! first `k` operations: the process "dies" after the `k`-th, and everything
//! after it is lost, as a power cut would lose it.
//! Each surviving disk is launched on twice, as the node would launch, and
//! the invariants are checked. Each effect is an operation, and so is each
//! run-file step inside one: a run-file write is its temporary file and then
//! the rename (`durable`'s temp-and-rename, so the run file itself is either
//! the old text or the new), and launch picks a copy and moves it aside
//! with the node's own `resume::take_saved`. The save's write is one
//! operation. Clearing the run file (`resume::replace`, for a score and for
//! `RunFile::Discard`) is the node's several steps, each its own operation,
//! so a crash part-way through one is exercised too. So is a `run.txt` that
//! will not go at all (`stuck`): its score must stay off the board; and a
//! full disk (`full`), on which nothing may be lost.
//!
//! The tests would fail if the core or the adapter could score twice: for
//! instance a score written without its `clear`, or written although the
//! clear failed.

use super::*;
use pretty_assertions::assert_eq;
use salt_line_sim::run::Phase;

use crate::input::{Event, Intents, Key};
use crate::layouts_file::{
    self, DRAFT_FILES, DRAFT_LOADING_PATH, DRAFT_PATH, DRAFT_TEMP_PATH, LAYOUTS_PATH,
    LAYOUTS_TEMP_PATH,
};
use crate::match_log::{self, MATCH_FILES, MATCH_LOADING_PATH, MATCH_PATH, MATCH_TEMP_PATH};
use crate::resume::{self, RunFiles, RUN_LOADING_PATH, RUN_PATH, RUN_TEMP_PATH};

const FRAME: f64 = 1.0 / 60.0;
const WINDOW: (i32, i32) = (1280, 720);
const TODAY: u64 = 20_000;

fn idle() -> Intents {
    Intents {
        window: WINDOW,
        today: TODAY,
        ..Intents::default()
    }
}

fn keys(keys: &[Key]) -> Intents {
    Intents {
        events: keys.iter().map(|k| Event::key(*k)).collect(),
        keys_held: keys.to_vec(),
        ..idle()
    }
}

/// The game's files, in memory: the save, the run kept for CONTINUE (its
/// temporary file, and the copy a launch sets aside while it rebuilds it),
/// and the crash reports. Each effect is one operation, and each run-file
/// operation inside it (`resume::RunFiles`) one more. `budget` is how many
/// more operations happen before the process dies; after that nothing
/// reaches the disk.
#[derive(Clone, Default)]
struct FakeDisk {
    save: Option<String>,
    run: Option<String>,
    temp: Option<String>,
    set_aside: Option<String>,
    /// The versus match kept for RESUME MATCH, its temporary file and its
    /// set-aside copy (`match_log::MATCH_FILES`).
    match_main: Option<String>,
    match_temp: Option<String>,
    match_aside: Option<String>,
    /// The player's saved layouts and their temporary file (DESIGN §13.8),
    /// and the map editor's draft, its temporary file and its set-aside
    /// copy (`layouts_file::DRAFT_FILES`).
    layouts: Option<String>,
    layouts_temp: Option<String>,
    draft_main: Option<String>,
    draft_temp: Option<String>,
    draft_aside: Option<String>,
    crash_report: Option<String>,
    old_crash_report: Option<String>,
    budget: Option<usize>,
    /// Operations carried out, for sizing the crash loop.
    done: usize,
    /// Score writes still to fail, as a full disk would.
    fail_scores: usize,
    /// `run.txt` can be neither replaced nor deleted, as a read-only or
    /// failing file would be: a rename onto or off it, and its removal, fail.
    stuck: bool,
    /// The disk is full: nothing new can be written (a run-file write's
    /// temporary file, the save), though renames and deletes still work.
    full: bool,
}

impl FakeDisk {
    /// One disk operation, if the process is still alive to do it.
    fn alive(&mut self) -> bool {
        if self.budget == Some(0) {
            return false;
        }
        if let Some(b) = self.budget.as_mut() {
            *b -= 1;
        }
        self.done += 1;
        true
    }

    /// Carry out effects as `game.rs`'s `apply` does, `stored` feedback
    /// first.
    fn apply(&mut self, g: &mut Game, fx: Vec<Effect>) {
        for e in fx {
            if !self.alive() {
                return;
            }
            match e {
                Effect::Store(Store::Layouts { text, why }) => {
                    // The slot file, written whole by way of its temporary
                    // file, as `save::write` does.
                    let ok = !self.full && self.write(LAYOUTS_PATH, LAYOUTS_TEMP_PATH, &text);
                    let more = g.stored(Store::Layouts { text, why }, ok);
                    self.apply(g, more);
                }
                Effect::Store(why) => {
                    let cleared = why.clear().is_none_or(|text| resume::replace(self, text));
                    let fail = matches!(why, Store::Score { .. }) && self.fail_scores > 0;
                    let ok = if !cleared {
                        false
                    } else if fail {
                        self.fail_scores -= 1;
                        false
                    } else if self.full || g.save.from_the_future {
                        // `save::store` refuses a newer build's save.
                        false
                    } else if self.alive() {
                        self.save = Some(g.save_to_store(&why).serialize());
                        true
                    } else {
                        false
                    };
                    let more = g.stored(why, ok);
                    self.apply(g, more);
                }
                Effect::RunFile(RunFile::Write(text)) => {
                    resume::replace(self, &text);
                }
                Effect::RunFile(RunFile::Restore) => {
                    self.rename(RUN_LOADING_PATH, RUN_PATH);
                }
                Effect::RunFile(RunFile::Abandon(Abandoned::Interrupted)) => {
                    resume::drop_set_aside(self);
                }
                Effect::RunFile(RunFile::Discard | RunFile::Abandon(_)) => {
                    resume::discard(self);
                }
                Effect::MatchFile(MatchFile::Write(text)) => {
                    resume::replace_at(self, MATCH_FILES, &text);
                }
                Effect::MatchFile(MatchFile::Restore) => {
                    self.rename(MATCH_LOADING_PATH, MATCH_PATH);
                }
                Effect::MatchFile(MatchFile::Abandon(Abandoned::Interrupted)) => {
                    resume::drop_set_aside_at(self, MATCH_FILES);
                }
                Effect::MatchFile(MatchFile::Discard | MatchFile::Abandon(_)) => {
                    resume::replace_at(self, MATCH_FILES, "");
                }
                Effect::DraftFile(DraftFile::Write(text)) => {
                    resume::replace_at(self, DRAFT_FILES, &text);
                }
                Effect::DraftFile(DraftFile::Restore) => {
                    self.rename(DRAFT_LOADING_PATH, DRAFT_PATH);
                }
                Effect::DraftFile(DraftFile::Abandon(Abandoned::Interrupted)) => {
                    resume::drop_set_aside_at(self, DRAFT_FILES);
                }
                Effect::DraftFile(DraftFile::Discard | DraftFile::Abandon(_)) => {
                    resume::replace_at(self, DRAFT_FILES, "");
                }
                Effect::CrashReport(CrashReport::Retire) => {
                    if let Some(text) = self.crash_report.take() {
                        self.old_crash_report = Some(text);
                    }
                }
                _ => {}
            }
        }
    }

    /// Launch the game on what is on disk, as the node's `ready` does.
    fn launch(&mut self) -> Game {
        let save = self.save.as_deref().map(Save::parse).unwrap_or_default();
        let mut g = Game::new(false, save);
        g.set_last_crash(self.crash_report.clone());
        if let Some(found) = resume::take_saved(self) {
            let fx = g.saved_run(found);
            self.apply(&mut g, fx);
        }
        if let Some(found) = resume::take_aside(self, MATCH_FILES, match_log::sound) {
            let fx = g.saved_match(found);
            self.apply(&mut g, fx);
        }
        // The slot file as the node picks it, then the draft set aside.
        let pick = crate::durable::pick_by(
            self.layouts.as_deref(),
            self.layouts_temp.as_deref(),
            layouts_file::sound,
        );
        let slots = match pick {
            crate::durable::Pick::Main => self.layouts.clone(),
            crate::durable::Pick::Temp => self.layouts_temp.clone(),
            crate::durable::Pick::Empty => None,
        };
        g.set_layouts(slots.as_deref());
        if let Some(found) = resume::take_aside(self, DRAFT_FILES, layouts_file::draft_sound) {
            let fx = g.saved_draft(found);
            self.apply(&mut g, fx);
        }
        let fx = g.fit_display(WINDOW, (0, 0));
        self.apply(&mut g, fx);
        g
    }

    fn board(&self) -> Vec<Entry> {
        self.save
            .as_deref()
            .map(|t| Save::parse(t).scores)
            .unwrap_or_default()
    }
}

impl FakeDisk {
    fn file(&mut self, path: &str) -> &mut Option<String> {
        match path {
            RUN_PATH => &mut self.run,
            RUN_TEMP_PATH => &mut self.temp,
            RUN_LOADING_PATH => &mut self.set_aside,
            MATCH_PATH => &mut self.match_main,
            MATCH_TEMP_PATH => &mut self.match_temp,
            MATCH_LOADING_PATH => &mut self.match_aside,
            LAYOUTS_PATH => &mut self.layouts,
            LAYOUTS_TEMP_PATH => &mut self.layouts_temp,
            DRAFT_PATH => &mut self.draft_main,
            DRAFT_TEMP_PATH => &mut self.draft_temp,
            DRAFT_LOADING_PATH => &mut self.draft_aside,
            _ => unreachable!("{path}"),
        }
    }
}

/// The node's `user://` operations, each one step the process can die
/// after. A write is two: the temporary file, then the rename.
impl resume::RunFiles for FakeDisk {
    fn read(&self, path: &str) -> Option<String> {
        match path {
            RUN_PATH => self.run.clone(),
            RUN_TEMP_PATH => self.temp.clone(),
            RUN_LOADING_PATH => self.set_aside.clone(),
            MATCH_PATH => self.match_main.clone(),
            MATCH_TEMP_PATH => self.match_temp.clone(),
            MATCH_LOADING_PATH => self.match_aside.clone(),
            LAYOUTS_PATH => self.layouts.clone(),
            LAYOUTS_TEMP_PATH => self.layouts_temp.clone(),
            DRAFT_PATH => self.draft_main.clone(),
            DRAFT_TEMP_PATH => self.draft_temp.clone(),
            DRAFT_LOADING_PATH => self.draft_aside.clone(),
            _ => unreachable!("{path}"),
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }

    fn write(&mut self, main: &str, temp: &str, text: &str) -> bool {
        if !self.alive() || self.full {
            return false;
        }
        *self.file(temp) = Some(text.into());
        self.rename(temp, main)
    }

    fn rename(&mut self, from: &str, to: &str) -> bool {
        if !self.alive() || (self.stuck && (from == RUN_PATH || to == RUN_PATH)) {
            return false;
        }
        let Some(text) = self.file(from).take() else {
            return false;
        };
        *self.file(to) = Some(text);
        true
    }

    fn remove(&mut self, path: &str) {
        if self.alive() && !(self.stuck && path == RUN_PATH) {
            *self.file(path) = None;
        }
    }
}

fn tick(g: &mut Game, disk: &mut FakeDisk, i: Intents) {
    let fx = g.tick(FRAME, &i);
    disk.apply(g, fx);
}

fn until(g: &mut Game, disk: &mut FakeDisk, done: impl Fn(&Game) -> bool) {
    for _ in 0..5000 {
        if done(g) {
            return;
        }
        tick(g, disk, idle());
    }
    panic!("flow did not finish");
}

/// Fire a wild shot and wait for it to land.
fn throw_one(g: &mut Game, disk: &mut FakeDisk) {
    g.play.angle_ticks = 1400;
    g.play.power_ticks = 0;
    tick(g, disk, keys(&[Key::Space]));
    until(g, disk, |g| {
        g.play.anim.is_none() && g.play.clear_hold <= 0.0
    });
}

/// Throw every shell away, as a player out of luck would, so the log
/// replays to a finished run.
fn throw_all(g: &mut Game, disk: &mut FakeDisk) {
    while g.mode != Mode::GameOver {
        if g.play.run.as_ref().unwrap().phase == Phase::Drafting {
            tick(g, disk, keys(&[Key::Num1]));
        } else {
            throw_one(g, disk);
        }
        tick(g, disk, idle());
    }
}

/// What a flow leaves to check: the run it played, and the run a launch
/// may offer to continue if that is another one.
struct Played {
    seed: u64,
    log: Vec<salt_line_sim::run::LoggedShot>,
    live: Option<(u64, Vec<salt_line_sim::run::LoggedShot>)>,
}

impl Played {
    fn of(g: &Game) -> Played {
        let run = g.play.run.as_ref().unwrap();
        Played {
            seed: run.run_seed,
            log: run.log.clone(),
            live: None,
        }
    }
}

/// Launch, start today's daily, throw every shell away.
fn finish_a_run(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    Played::of(&g)
}

/// Launch, start a run, fire twice, quit to the title.
fn quit_mid_run(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_one(&mut g, disk);
    throw_one(&mut g, disk);
    tick(&mut g, disk, keys(&[Key::Escape]));
    tick(&mut g, disk, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Title);
    Played::of(&g)
}

/// Launch over a saved run, continue it, and throw every shell left.
fn continue_and_finish(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    Played::of(&g)
}

/// Another run, on its own seed, in the same session.
const NEXT_SEED: u64 = 777;

/// A finished run whose score cannot be written, then, the same session,
/// another run started and two shots into it: its writes must not lose the
/// kept log.
fn score_fails_then_play_on(disk: &mut FakeDisk) -> Played {
    disk.fail_scores = 1;
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    let mut played = Played::of(&g);
    g.start_run(NEXT_SEED, false);
    throw_one(&mut g, disk);
    throw_one(&mut g, disk);
    let next = g.play.run.as_ref().unwrap();
    played.live = Some((next.run_seed, next.log.clone()));
    played
}

/// As `score_fails_then_play_on`, and the second run is played out too:
/// its score takes the first one's onto the board with it.
fn score_fails_then_finish_another(disk: &mut FakeDisk) -> Played {
    disk.fail_scores = 1;
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    let mut played = Played::of(&g);
    g.start_run(NEXT_SEED, false);
    throw_all(&mut g, disk);
    let next = g.play.run.as_ref().unwrap();
    played.live = Some((next.run_seed, next.log.clone()));
    played
}

/// A finished run whose score cannot be written, then a settings change
/// that can: the settings write must not carry the score to disk while the
/// kept run file scores it again at the next launch.
fn score_write_fails(disk: &mut FakeDisk) -> Played {
    disk.fail_scores = 1;
    let played = finish_a_run(disk);
    let mut g = disk.launch_without_scoring();
    tick(&mut g, disk, keys(&[Key::M]));
    played
}

impl FakeDisk {
    /// The same session going on after its failed score, not a relaunch:
    /// the board in memory is what the failed write left.
    fn launch_without_scoring(&mut self) -> Game {
        let save = self.save.as_deref().map(Save::parse).unwrap_or_default();
        let mut g = Game::new(false, save);
        g.fit_display(WINDOW, (0, 0));
        g
    }
}

/// A disk with last session's crash report on it.
fn with_crash_report() -> FakeDisk {
    FakeDisk {
        crash_report: Some("SALT LINE CRASH REPORT".into()),
        ..FakeDisk::default()
    }
}

/// Run `flow` on `start` once through, then again with the process killed
/// after each operation in turn; each time launch twice on what is left and
/// check what must hold.
fn crash_everywhere(start: FakeDisk, flow: fn(&mut FakeDisk) -> Played) {
    let mut whole = start.clone();
    let played = flow(&mut whole);
    let ops = whole.done;
    assert!(ops > 3, "the flow touched the disk {ops} times");
    for k in 0..=ops {
        let mut disk = start.clone();
        disk.budget = Some(k);
        flow(&mut disk);
        disk.budget = None;
        disk.fail_scores = 0;
        disk.stuck = false;
        disk.full = false;
        let first = disk.launch();
        if let Some(saved) = first.run_file.resumable.as_ref() {
            // Either run, as far as it got before the crash.
            let as_played = std::iter::once((played.seed, &played.log))
                .chain(played.live.as_ref().map(|(s, l)| (*s, l)))
                .any(|(seed, log)| saved.seed == seed && log.starts_with(&saved.log));
            assert!(
                as_played,
                "crash after {k}: the offered run is not one that was played"
            );
        }
        disk.launch();
        let scored = disk
            .board()
            .iter()
            .filter(|e| e.seed == played.seed)
            .count();
        assert!(scored <= 1, "crash after {k}: scored {scored} times");
        if start.crash_report.is_some() {
            assert!(
                disk.crash_report.is_some() || disk.old_crash_report.is_some(),
                "crash after {k}: the crash report went"
            );
        }
    }
}

#[test]
fn a_finished_run_is_scored_once_and_its_file_goes() {
    let mut disk = FakeDisk::default();
    let played = finish_a_run(&mut disk);
    assert_eq!(disk.board().len(), 1);
    assert_eq!(disk.board()[0].seed, played.seed);
    assert!(disk.run.is_none());
    disk.launch();
    assert_eq!(disk.board().len(), 1);
}

#[test]
fn finishing_a_run_survives_a_crash_anywhere() {
    crash_everywhere(with_crash_report(), finish_a_run);
}

#[test]
fn quitting_mid_run_survives_a_crash_anywhere() {
    crash_everywhere(FakeDisk::default(), quit_mid_run);
}

#[test]
fn a_quit_run_is_offered_again_exactly_as_played() {
    let mut disk = FakeDisk::default();
    let played = quit_mid_run(&mut disk);
    let g = disk.launch();
    let saved = g.run_file.resumable.as_ref().expect("offered");
    assert_eq!(saved.log, played.log);
}

#[test]
fn continuing_a_run_survives_a_crash_anywhere() {
    let mut start = FakeDisk::default();
    quit_mid_run(&mut start);
    // Count only the operations of the flow under test.
    start.done = 0;
    crash_everywhere(start, continue_and_finish);
}

#[test]
fn a_score_that_cannot_be_written_is_scored_once_later() {
    let mut disk = FakeDisk::default();
    let played = score_write_fails(&mut disk);
    // The settings write did not take the score with it.
    assert!(disk.board().is_empty());
    assert!(disk.run.is_some(), "the finished run was kept");
    let g = disk.launch();
    assert_eq!(g.status, "LAST RUN SCORED");
    disk.launch();
    let board = disk.board();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].seed, played.seed);
}

#[test]
fn a_failed_score_survives_a_crash_anywhere() {
    crash_everywhere(FakeDisk::default(), score_write_fails);
}

#[test]
fn a_failed_score_outlives_the_next_run_of_the_session() {
    let mut disk = FakeDisk::default();
    let played = score_fails_then_play_on(&mut disk);
    assert!(disk.board().is_empty());
    let g = disk.launch();
    assert_eq!(g.status, "LAST RUN SCORED");
    let offered = g
        .run_file
        .resumable
        .as_ref()
        .expect("the new run is offered");
    assert_eq!(offered.seed, NEXT_SEED);
    assert_eq!(Some(&offered.log), played.live.as_ref().map(|(_, l)| l));
    disk.launch();
    let board = disk.board();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].seed, played.seed);
}

#[test]
fn a_failed_score_goes_on_the_board_with_the_next_one() {
    let mut disk = FakeDisk::default();
    let played = score_fails_then_finish_another(&mut disk);
    let mut seeds: Vec<u64> = disk.board().iter().map(|e| e.seed).collect();
    seeds.sort();
    let mut want = vec![played.seed, NEXT_SEED];
    want.sort();
    assert_eq!(seeds, want);
    assert!(disk.run.is_none());
    disk.launch();
    assert_eq!(disk.board().len(), 2);
}

#[test]
fn a_failed_score_and_the_next_run_survive_a_crash_anywhere() {
    crash_everywhere(FakeDisk::default(), score_fails_then_play_on);
    crash_everywhere(FakeDisk::default(), score_fails_then_finish_another);
}

/// Launch, start today's daily and throw every shell away, but from the
/// last shot on `run.txt` will not go: the finished log it now holds can be
/// neither deleted nor blanked, so the score must not be written either.
fn finish_with_the_file_stuck(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    while g.mode != Mode::GameOver {
        let run = g.play.run.as_ref().unwrap();
        if run.phase == Phase::Drafting {
            tick(&mut g, disk, keys(&[Key::Num1]));
        } else {
            let last = run.shells == 1;
            g.play.angle_ticks = 1400;
            g.play.power_ticks = 0;
            // Firing grows the log, which is written this same frame.
            tick(&mut g, disk, keys(&[Key::Space]));
            if last {
                disk.stuck = true;
            }
            until(&mut g, disk, |g| {
                g.play.anim.is_none() && g.play.clear_hold <= 0.0
            });
        }
        tick(&mut g, disk, idle());
    }
    Played::of(&g)
}

#[test]
fn a_run_file_that_will_not_go_keeps_its_score_for_later_not_twice() {
    let mut disk = FakeDisk::default();
    let played = finish_with_the_file_stuck(&mut disk);
    assert!(disk.run.is_some(), "the finished log stayed");
    assert!(disk.board().is_empty(), "scored beside a log that stayed");
    // Once the file can go, the next launch scores it, once.
    disk.stuck = false;
    let g = disk.launch();
    assert_eq!(g.status, "LAST RUN SCORED");
    disk.launch();
    let board = disk.board();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].seed, played.seed);
}

#[test]
fn a_run_file_that_will_not_go_survives_a_crash_anywhere() {
    crash_everywhere(FakeDisk::default(), finish_with_the_file_stuck);
}

/// A disk holding a finished run whose score never saved, above a run
/// quit two shots in, and what was played.
fn kept_and_live() -> (FakeDisk, Played) {
    let mut disk = FakeDisk::default();
    let played = score_fails_then_play_on(&mut disk);
    disk.done = 0;
    (disk, played)
}

/// The run a launch offers is the live run of `played`, as played.
fn offers_the_live_run(g: &Game, played: &Played) {
    let (seed, log) = played.live.as_ref().unwrap();
    let offered = g
        .run_file
        .resumable
        .as_ref()
        .expect("the live run is offered");
    assert_eq!(offered.seed, *seed);
    assert_eq!(&offered.log, log);
}

#[test]
fn a_full_disk_at_launch_loses_neither_the_run_nor_the_kept_score() {
    let (mut disk, played) = kept_and_live();
    disk.full = true;
    for _ in 0..2 {
        let g = disk.launch();
        assert!(disk.board().is_empty());
        offers_the_live_run(&g, &played);
    }
    disk.full = false;
    let g = disk.launch();
    assert_eq!(g.status, "LAST RUN SCORED");
    offers_the_live_run(&g, &played);
    let g = disk.launch();
    offers_the_live_run(&g, &played);
    let board = disk.board();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].seed, played.seed);
}

#[test]
fn a_full_disk_keeps_a_good_run_beside_a_line_gone_bad() {
    let mut disk = FakeDisk::default();
    let mut played = quit_mid_run(&mut disk);
    played.live = Some((played.seed, played.log.clone()));
    let live = disk.run.clone().unwrap();
    disk.run = Some(format!("resume junk\n{live}"));
    disk.full = true;
    let g = disk.launch();
    offers_the_live_run(&g, &played);
    disk.full = false;
    let g = disk.launch();
    offers_the_live_run(&g, &played);
}

#[test]
fn a_launch_that_dies_scoring_never_costs_the_live_run() {
    // Killed after every step of a launch that scores the kept run and
    // rewrites the file down to the live one: once the live run is back in
    // `run.txt`, no later launch loses it.
    let (start, played) = kept_and_live();
    let mut whole = start.clone();
    whole.launch();
    for k in 0..=whole.done {
        let mut disk = start.clone();
        disk.budget = Some(k);
        disk.launch();
        disk.budget = None;
        let restored = disk.run.is_some();
        disk.launch();
        let g = disk.launch();
        if restored {
            offers_the_live_run(&g, &played);
        }
        assert!(disk.board().len() <= 1, "crash after {k}");
    }
}

#[test]
fn a_run_already_back_in_place_outlives_the_copy_left_set_aside() {
    // The state a launch killed between writing the live run and deleting
    // the set-aside copy would leave: the next launch throws away the copy
    // (it may be what killed the last one), never the run beside it.
    let (mut disk, played) = kept_and_live();
    let live = resume::Saved {
        epoch: salt_line_sim::SIM_EPOCH,
        seed: played.live.as_ref().unwrap().0,
        width: 320,
        daily: false,
        log: played.live.as_ref().unwrap().1.clone(),
    };
    disk.set_aside = disk.run.take();
    disk.run = Some(live.to_line());
    let g = disk.launch();
    assert!(
        g.run_file.resumable.is_none(),
        "the set-aside copy is not replayed"
    );
    let g = disk.launch();
    offers_the_live_run(&g, &played);
}

// ----------------------------------------------------------- after a panic

#[test]
fn nothing_queued_before_a_panic_is_replayed_after_it() {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g.crash.last_crash = Some("report".into());
    // A frame that got this far and panicked: a run started, its crash
    // report retired and the run file written, all still queued.
    g.start_run(1, false);
    g.finish_run();
    assert!(!g.fx.is_empty());
    let fx = g.broken_tick(FRAME, &idle(), || None);
    // Only the error screen's own release of the screen, which is new, not
    // replayed (`app/awake_tests.rs`).
    assert_eq!(fx, vec![Effect::ScreenAwake(false)]);
    assert!(g.focus_lost(&[]).iter().all(Effect::survives_panic));
    assert_eq!(g.closing(&[]), vec![]);
}

#[test]
fn the_node_drops_every_file_effect_after_a_panic() {
    let dropped = [
        Effect::Store(Store::Settings),
        Effect::RunFile(RunFile::Write(String::new())),
        Effect::RunFile(RunFile::Discard),
        Effect::RunFile(RunFile::Restore),
        Effect::RunFile(RunFile::Abandon(Abandoned::Interrupted)),
        Effect::MatchFile(MatchFile::Write(String::new())),
        Effect::MatchFile(MatchFile::Discard),
        Effect::MatchFile(MatchFile::Restore),
        Effect::MatchFile(MatchFile::Abandon(Abandoned::Interrupted)),
        Effect::CrashReport(CrashReport::Retire),
    ];
    assert!(dropped.iter().all(|e| !e.survives_panic()));
    let kept = [
        Effect::Play(Sound::Ui),
        Effect::Redraw,
        Effect::Clipboard(String::new()),
        Effect::Quit,
    ];
    assert!(kept.iter().all(Effect::survives_panic));
}

// ------------------------------------------------------------ attract demo

impl FakeDisk {
    /// Every file, as text: what a demo must leave exactly as it found.
    fn files(&self) -> [Option<String>; 14] {
        [
            self.save.clone(),
            self.run.clone(),
            self.temp.clone(),
            self.set_aside.clone(),
            self.match_main.clone(),
            self.match_temp.clone(),
            self.match_aside.clone(),
            self.crash_report.clone(),
            self.old_crash_report.clone(),
            self.layouts.clone(),
            self.layouts_temp.clone(),
            self.draft_main.clone(),
            self.draft_temp.clone(),
            self.draft_aside.clone(),
        ]
    }
}

/// A run quit to the title, then the idle demo through a whole cycle (a run
/// and a duel) and into the next run, then a key back to the title: the
/// disk is byte for byte as it was, no disk effect was asked for, and
/// CONTINUE picks the run up exactly as it was played, now and at the next
/// launch.
#[test]
fn a_suspended_run_survives_a_demo_cycle_byte_for_byte() {
    let mut disk = with_crash_report();
    let mut g = disk.launch();
    tick(&mut g, &mut disk, keys(&[Key::Enter]));
    throw_one(&mut g, &mut disk);
    throw_one(&mut g, &mut disk);
    tick(&mut g, &mut disk, keys(&[Key::Escape]));
    tick(&mut g, &mut disk, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Title);
    let played = Played::of(&g);
    let files = disk.files();
    assert!(files[1].is_some(), "the run is on disk");
    let offered = g.run_file.resumable.clone();

    // Quiet on the title until the demo starts, then a full cycle.
    let mut during = Vec::new();
    for _ in 0..30_000 {
        let fx = g.tick(1.0 / 30.0, &idle());
        if g.in_demo() {
            during.extend(fx.iter().cloned());
        }
        disk.apply(&mut g, fx);
        if g.demo.as_ref().is_some_and(|d| d.leg_index == 2) {
            break;
        }
    }
    assert!(g.demo.as_ref().is_some_and(|d| d.leg_index == 2));
    let disk_fx: Vec<_> = during.iter().filter(|e| e.touches_disk()).collect();
    assert!(disk_fx.is_empty(), "the demo asked for {disk_fx:?}");
    assert_eq!(disk.files(), files, "the demo changed a file");

    tick(&mut g, &mut disk, keys(&[Key::Space]));
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(disk.files(), files);
    assert_eq!(g.run_file.resumable, offered);
    assert_eq!(Played::of(&g).log, played.log);

    // A relaunch on this disk offers it exactly as it was played.
    let mut relaunched = disk.clone();
    let saved = relaunched.launch().run_file.resumable.expect("offered");
    assert_eq!((saved.seed, &saved.log), (played.seed, &played.log));

    // And CONTINUE now plays on from the very shot it was left at.
    tick(&mut g, &mut disk, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Playing);
    let run = g.play.run.as_ref().unwrap();
    assert_eq!((run.run_seed, &run.log), (played.seed, &played.log));
}

// ---------------------------------------------------------- the match file
//
// A versus match kept on disk (DESIGN §11.6), killed at each moment a phone
// may kill it, and resumed: the match as played, byte for byte.

fn vs(g: &Game) -> &VersusPlay {
    g.versus.live.as_ref().expect("a match on screen")
}

fn stage(g: &Game) -> versus::Stage {
    vs(g).stage
}

/// Press `key` and let it go, so the next press of it is a press.
fn press(g: &mut Game, disk: &mut FakeDisk, key: Key) {
    tick(g, disk, keys(&[key]));
    tick(g, disk, idle());
}

/// Launch and start a match from the title: versus, then START.
fn start_match(disk: &mut FakeDisk) -> Game {
    let mut g = disk.launch();
    press(&mut g, disk, Key::Num2);
    assert_eq!(g.mode, Mode::VersusSetup);
    press(&mut g, disk, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    g
}

/// A high lob that lands short of the other gun.
const LOB: (i32, i32) = (1400, 330);

/// Shots in a match's log, new rounds aside.
fn shots_fired(vs: &VersusPlay) -> usize {
    vs.log
        .iter()
        .filter(|e| matches!(e, match_log::Entry::Shot(..)))
        .count()
}

/// The scripted aim for the next shot: a hit, except every third shot,
/// which lobs short, so pairs with a hit roll the guns and rounds are won.
fn scripted(g: &Game) -> (i32, i32) {
    let vs = vs(g);
    if shots_fired(vs) % 3 == 1 {
        return LOB;
    }
    let duel = &vs.duel;
    let side = duel.to_move;
    let frame = crate::versus::frame_for(&duel.screen, side);
    let target = crate::versus::target_in_frame(&duel.screen, side);
    crate::versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
        .first()
        .map_or(LOB, |s| (s.angle_ticks, s.power_ticks))
}

/// From the handoff plate (or a round's result, which deals the next
/// round): ready, and dial the scripted aim.
fn ready_and_aim(g: &mut Game, disk: &mut FakeDisk) {
    if stage(g) == versus::Stage::Result {
        press(g, disk, Key::Enter);
    }
    assert_eq!(stage(g), versus::Stage::Handoff);
    press(g, disk, Key::Enter);
    assert_eq!(stage(g), versus::Stage::Aiming);
    (g.play.angle_ticks, g.play.power_ticks) = scripted(g);
}

/// One whole turn: ready, aim, fire, and wait for the next plate.
fn match_turn(g: &mut Game, disk: &mut FakeDisk) {
    ready_and_aim(g, disk);
    tick(g, disk, keys(&[Key::Space]));
    assert_eq!(stage(g), versus::Stage::Flight);
    until(g, disk, |g| stage(g) != versus::Stage::Flight);
}

/// Everything a resumed match must have back, as text, every float by its
/// `Debug` form (exact for a finite float): the rules' state, each side's
/// kept aim and last arc, the log, the roll the handoff plate announces,
/// whose turn and the pre-match choices it is played under.
fn match_state(g: &Game) -> String {
    let vs = vs(g);
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        vs.duel,
        vs.aims,
        vs.arcs,
        vs.log,
        vs.rolled,
        vs.side(),
        g.versus.help,
        g.versus.rolling_off,
    )
}

/// Where a match is killed.
#[derive(Clone, Copy, Debug)]
enum Kill {
    /// Between turns, on the pass-the-phone plate (or a round's result).
    Plate,
    /// A player has readied up and is aiming, the shot not yet fired.
    MidTurn,
    /// The shot is fired and still in the air.
    MidFlight,
}

/// Start a match with aim help on the right, play `turns` scripted turns,
/// then stop at `kill`: the disk as the app left it, and the game.
fn play_and_kill(turns: usize, kill: Kill) -> (FakeDisk, Game) {
    let mut disk = FakeDisk::default();
    let mut g = disk.launch();
    press(&mut g, &mut disk, Key::Num2);
    press(&mut g, &mut disk, Key::Right);
    press(&mut g, &mut disk, Key::Enter);
    for _ in 0..turns {
        match_turn(&mut g, &mut disk);
    }
    match kill {
        Kill::Plate => {}
        Kill::MidTurn => ready_and_aim(&mut g, &mut disk),
        Kill::MidFlight => {
            ready_and_aim(&mut g, &mut disk);
            tick(&mut g, &mut disk, keys(&[Key::Space]));
            tick(&mut g, &mut disk, idle());
            assert_eq!(stage(&g), versus::Stage::Flight);
            assert!(!vs(&g).anim.as_ref().unwrap().done(), "still in the air");
        }
    }
    (disk, g)
}

/// Relaunch on `disk` and take RESUME MATCH from the title.
fn relaunch_and_resume(disk: &mut FakeDisk) -> Game {
    let mut g = disk.launch();
    assert_eq!(g.mode, Mode::Title);
    assert!(g.saved_match.is_some(), "RESUME MATCH is offered");
    press(&mut g, disk, Key::Num2);
    assert_eq!(g.mode, Mode::Versus);
    g
}

#[test]
fn a_killed_match_resumes_byte_for_byte_wherever_it_was_killed() {
    let mut rolls = 0;
    let mut results = 0;
    for turns in 0..10 {
        for kill in [Kill::Plate, Kill::MidTurn, Kill::MidFlight] {
            let (mut disk, live) = play_and_kill(turns, kill);
            let want = match_state(&live);
            rolls += vs(&live).rolled.is_some() as u32;
            let back = relaunch_and_resume(&mut disk);
            assert_eq!(match_state(&back), want, "{turns} turns, killed {kill:?}");
            // On the pass-the-phone plate for whoever is next, or on the
            // result plate of a round the last shot won.
            let won = matches!(vs(&back).duel.phase, crate::versus::Phase::RoundOver(_));
            results += won as u32;
            let plate = if won {
                versus::Stage::Result
            } else {
                versus::Stage::Handoff
            };
            assert_eq!(stage(&back), plate, "{turns} turns, killed {kill:?}");
            // On the plate the guns are where the live game had them: a
            // roll not yet eased is eased once the next player is ready.
            if matches!(kill, Kill::Plate) {
                assert_eq!(vs(&back).ease, vs(&live).ease);
            }
            // Nothing new was written by the resume: the file is the line.
            assert_eq!(
                disk.match_main.as_deref(),
                Some(
                    match_log::SavedMatch::of(&vs(&back).duel, back.versus.help, &vs(&back).log)
                        .to_line()
                        .as_str()
                )
            );
        }
    }
    assert!(rolls > 0, "no kill came after a roll");
    assert!(results > 0, "no kill came after a round was won");
}

#[test]
fn a_resumed_match_plays_on_to_the_same_end() {
    // Uninterrupted.
    let mut disk = FakeDisk::default();
    let mut whole = start_match(&mut disk);
    while !matches!(vs(&whole).duel.phase, crate::versus::Phase::MatchOver(_)) {
        match_turn(&mut whole, &mut disk);
    }
    // Killed in mid-flight five turns in, resumed, and played on.
    let (mut disk, _) = play_and_kill(5, Kill::MidFlight);
    let mut g = relaunch_and_resume(&mut disk);
    while !matches!(vs(&g).duel.phase, crate::versus::Phase::MatchOver(_)) {
        match_turn(&mut g, &mut disk);
    }
    assert_eq!(
        format!("{:?}|{:?}", vs(&g).duel, vs(&g).log),
        format!("{:?}|{:?}", vs(&whole).duel, vs(&whole).log)
    );
    // Won, and its result plate left: nothing is left to resume.
    until(&mut g, &mut disk, |g| stage(g) == versus::Stage::Result);
    press(&mut g, &mut disk, Key::R);
    assert_eq!(disk.match_main, None);
    assert!(disk.launch().saved_match.is_none());
}

/// A match killed while its winning shell is in the air comes back on the
/// result plate that says who won; leaving that plate clears it.
#[test]
fn a_won_match_is_kept_until_its_result_plate_is_left() {
    let mut disk = FakeDisk::default();
    let mut g = start_match(&mut disk);
    loop {
        ready_and_aim(&mut g, &mut disk);
        tick(&mut g, &mut disk, keys(&[Key::Space]));
        if matches!(vs(&g).duel.phase, crate::versus::Phase::MatchOver(_)) {
            break;
        }
        until(&mut g, &mut disk, |g| stage(g) != versus::Stage::Flight);
    }
    // Still in the air, and its winning shot is on disk.
    assert_eq!(stage(&g), versus::Stage::Flight);
    let live = match_state(&g);
    let mut killed = disk.clone();
    let mut back = relaunch_and_resume(&mut killed);
    assert_eq!(stage(&back), versus::Stage::Result);
    assert_eq!(match_state(&back), live);
    // REMATCH from the resumed result plate clears it.
    press(&mut back, &mut killed, Key::Enter);
    assert_eq!(back.mode, Mode::VersusSetup);
    assert_eq!(killed.match_main, None);
    assert!(killed.launch().saved_match.is_none());

    // Live: kept on the result plate, cleared by REMATCH, and the
    // rematch is a new match, kept from its start.
    until(&mut g, &mut disk, |g| stage(g) == versus::Stage::Result);
    assert!(disk.match_main.is_some());
    press(&mut g, &mut disk, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert_eq!(disk.match_main, None);
    press(&mut g, &mut disk, Key::Enter);
    let line = disk.match_main.clone().expect("the rematch is kept");
    assert_eq!(
        match_log::SavedMatch::parse(&line).unwrap().seed,
        vs(&g).duel.seed
    );
}

#[test]
fn a_quit_match_is_cleared() {
    // From the pause plate.
    let (mut disk, mut g) = play_and_kill(3, Kill::MidTurn);
    assert!(disk.match_main.is_some());
    press(&mut g, &mut disk, Key::Escape);
    assert!(vs(&g).paused);
    press(&mut g, &mut disk, Key::R);
    assert!(disk.match_main.is_some(), "one press only arms TITLE");
    press(&mut g, &mut disk, Key::R);
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(disk.match_main, None);
    assert!(disk.launch().saved_match.is_none());
    // From a round's result plate (TITLE).
    let mut disk = FakeDisk::default();
    let mut g = start_match(&mut disk);
    while stage(&g) != versus::Stage::Result {
        match_turn(&mut g, &mut disk);
    }
    assert!(disk.match_main.is_some());
    press(&mut g, &mut disk, Key::R);
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(disk.match_main, None);
    // A resumed match, quit again.
    let (mut disk, _) = play_and_kill(2, Kill::Plate);
    let mut g = relaunch_and_resume(&mut disk);
    press(&mut g, &mut disk, Key::Escape);
    press(&mut g, &mut disk, Key::R);
    press(&mut g, &mut disk, Key::R);
    assert_eq!(disk.match_main, None);
    assert!(disk.launch().saved_match.is_none());
}

/// Start a match, play four turns, then let the app be killed.
fn match_then_killed(disk: &mut FakeDisk) -> Vec<match_log::Entry> {
    let mut g = start_match(disk);
    for _ in 0..4 {
        match_turn(&mut g, disk);
    }
    vs(&g).log.clone()
}

/// Start a match, play four turns, then quit it from the pause plate.
fn match_then_quit(disk: &mut FakeDisk) -> Vec<match_log::Entry> {
    let mut g = start_match(disk);
    for _ in 0..4 {
        match_turn(&mut g, disk);
    }
    let shots = vs(&g).log.clone();
    press(&mut g, disk, Key::Escape);
    press(&mut g, disk, Key::R);
    press(&mut g, disk, Key::R);
    assert_eq!(g.mode, Mode::Title);
    shots
}

/// `flow` played once through, then again with the process killed after
/// each disk operation in turn: whatever a launch offers is the match as
/// played up to some shot, and it rebuilds.
fn match_survives_a_crash_anywhere(flow: fn(&mut FakeDisk) -> Vec<match_log::Entry>) -> FakeDisk {
    let mut whole = FakeDisk::default();
    let played = flow(&mut whole);
    let ops = whole.done;
    for k in 0..=ops {
        let mut disk = FakeDisk {
            budget: Some(k),
            ..FakeDisk::default()
        };
        flow(&mut disk);
        disk.budget = None;
        for _ in 0..2 {
            let mut g = disk.launch();
            if let Some(saved) = g.saved_match.clone() {
                assert!(
                    played.starts_with(&saved.log),
                    "crash after {k}: the offered match is not the one played"
                );
                press(&mut g, &mut disk, Key::Num2);
                assert_eq!(vs(&g).log, saved.log, "crash after {k}");
                // Leave it on disk for the second launch: the app killed
                // again rather than quit.
            }
        }
    }
    whole
}

#[test]
fn a_kept_match_survives_a_crash_anywhere() {
    let mut whole = match_survives_a_crash_anywhere(match_then_killed);
    let offered = whole.launch().saved_match.expect("offered");
    assert_eq!(offered.log.len(), 4);
}

#[test]
fn quitting_a_match_survives_a_crash_anywhere() {
    let mut whole = match_survives_a_crash_anywhere(match_then_quit);
    assert!(whole.launch().saved_match.is_none());
}

#[rstest::rstest]
#[case::garbage("match 1 banana\n", "SAVED MATCH COULD NOT BE RESTORED")]
#[case::nul_tail("match 1\0\0\0\0\n", "SAVED MATCH COULD NOT BE RESTORED")]
#[case::older(
    &format!("match 1 {} 0 7 320 1 0 0 0\n", salt_line_sim::SIM_EPOCH),
    "SAVED MATCH FROM AN OLDER VERSION"
)]
#[case::newer_sim(
    &format!("match 1 {} 1 7 320 1 0 0 0\n", salt_line_sim::SIM_EPOCH + 1),
    "SAVED MATCH FROM A NEWER VERSION"
)]
#[case::newer_format("match 4 x\n", "SAVED MATCH FROM A NEWER VERSION")]
fn a_match_file_that_will_not_do_is_cleared_with_a_note(#[case] text: &str, #[case] note: &str) {
    let mut disk = FakeDisk {
        match_main: Some(text.into()),
        ..FakeDisk::default()
    };
    let g = disk.launch();
    assert!(g.saved_match.is_none());
    assert_eq!(g.status, note);
    assert!(g.status_time > STATUS_TIME, "kept up long enough to read");
    assert_eq!(disk.files(), FakeDisk::default().files(), "cleared");
}

#[test]
fn a_truncated_match_file_is_cleared_with_a_note() {
    let (disk, _) = play_and_kill(5, Kill::Plate);
    let line = disk.match_main.clone().unwrap();
    for cut in [1, line.len() / 2, line.len() - 3, line.len() - 1] {
        let mut torn = FakeDisk {
            match_main: Some(line[..cut].into()),
            ..FakeDisk::default()
        };
        let g = torn.launch();
        assert!(g.saved_match.is_none(), "cut at {cut}");
        assert_eq!(g.status, "SAVED MATCH COULD NOT BE RESTORED");
        assert_eq!(torn.match_main, None);
    }
    // A torn main file beside a whole temporary file: the write that died
    // before its rename is the copy to trust, as for the run file.
    let mut torn = FakeDisk {
        match_main: Some(line[..line.len() / 2].into()),
        match_temp: Some(line.clone()),
        ..FakeDisk::default()
    };
    let g = torn.launch();
    assert!(g.saved_match.is_some());
    assert_eq!(torn.match_main.as_deref(), Some(line.as_str()));
}

#[test]
fn a_match_that_crashed_its_rebuild_costs_one_launch() {
    let (mut disk, _) = play_and_kill(3, Kill::Plate);
    let line = disk.match_main.clone().unwrap();
    // As a launch that died rebuilding it would leave it.
    disk.match_aside = disk.match_main.take();
    let g = disk.launch();
    assert!(g.saved_match.is_none());
    assert_eq!(g.status, "SAVED MATCH COULD NOT BE RESTORED");
    assert_eq!(disk.match_aside, None);
    // A match file written since is kept for the launch after.
    let mut disk = FakeDisk {
        match_main: Some(line.clone()),
        match_aside: Some(line),
        ..FakeDisk::default()
    };
    assert!(disk.launch().saved_match.is_none());
    assert!(disk.launch().saved_match.is_some());
}

/// A launch that died rebuilding, after a crash between a write's fsync and
/// its rename: a set-aside copy beside a whole temporary file, for the run
/// and the match alike. The next launch offers nothing and leaves nothing
/// behind; the temporary file, with its copy gone, would otherwise be
/// picked by the launch after and likely crash it too.
#[test]
fn an_interrupted_launch_takes_a_whole_temp_file_with_it() {
    let (played, _) = play_and_kill(3, Kill::Plate);
    let match_line = played.match_main.clone().unwrap();
    let mut with_run = FakeDisk::default();
    quit_mid_run(&mut with_run);
    let run_line = with_run.run.clone().unwrap();
    let mut disk = FakeDisk {
        set_aside: Some(run_line.clone()),
        temp: Some(run_line),
        match_aside: Some(match_line.clone()),
        match_temp: Some(match_line),
        ..FakeDisk::default()
    };
    let g = disk.launch();
    assert!(g.run_file.resumable.is_none());
    assert!(g.saved_match.is_none());
    assert_eq!(disk.files(), FakeDisk::default().files(), "nothing left");
    let g = disk.launch();
    assert!(g.run_file.resumable.is_none() && g.saved_match.is_none());
}

#[test]
fn a_lost_run_and_a_lost_match_are_both_noted() {
    let mut disk = FakeDisk {
        run: Some("resume junk\n".into()),
        match_main: Some("match junk\n".into()),
        ..FakeDisk::default()
    };
    let g = disk.launch();
    assert_eq!(
        g.status,
        "SAVED RUN COULD NOT BE RESTORED - SAVED MATCH COULD NOT BE RESTORED"
    );
    // The title centres it in the narrowest buffer, clear of the edges.
    let w = crate::font::small_width(&g.status) as i32;
    assert!(w + 8 <= salt_line_sim::SCREEN_W_MIN as i32, "{w} px");
}

/// A run quit to the title and a match killed later the same session: both
/// are offered at the next launch, and taking either leaves the other
/// exactly where it was.
#[test]
fn a_suspended_run_and_a_suspended_match_live_side_by_side() {
    let mut disk = FakeDisk::default();
    let played = quit_mid_run(&mut disk);
    let mut g = disk.launch();
    tick(&mut g, &mut disk, idle());
    press(&mut g, &mut disk, Key::Num2);
    press(&mut g, &mut disk, Key::Enter);
    for _ in 0..3 {
        match_turn(&mut g, &mut disk);
    }
    let live = match_state(&g);
    let run_file = disk.run.clone();

    let mut g = disk.launch();
    assert!(g.run_file.resumable.is_some(), "CONTINUE");
    assert!(g.saved_match.is_some(), "RESUME MATCH");
    press(&mut g, &mut disk, Key::Num2);
    assert_eq!(match_state(&g), live);
    assert_eq!(disk.run, run_file, "the match left the run alone");
    assert!(g.run_file.resumable.is_some());
    // Quit the match: the run is still there to continue.
    press(&mut g, &mut disk, Key::Escape);
    press(&mut g, &mut disk, Key::R);
    press(&mut g, &mut disk, Key::R);
    assert_eq!(disk.match_main, None);
    assert_eq!(disk.run, run_file);
    press(&mut g, &mut disk, Key::Enter);
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.run.as_ref().unwrap().log, played.log);
}

#[test]
fn a_resumed_match_keeps_its_width_on_a_wider_screen() {
    let (mut disk, live) = play_and_kill(2, Kill::Plate);
    let width = vs(&live).duel.width() as i32;
    let wide = (2400, 1080);
    let at = |i: Intents| Intents { window: wide, ..i };
    let mut g = disk.launch();
    g.fit_display(wide, (0, 0));
    assert!(g.display.buffer_w > width);
    let device = g.display.buffer_w;
    tick(&mut g, &mut disk, at(keys(&[Key::Num2])));
    for _ in 0..3 {
        tick(&mut g, &mut disk, at(idle()));
    }
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(
        g.display.buffer_w, width,
        "the match's own width, letterboxed"
    );
    assert_eq!(match_state(&g), match_state(&live));
    // Quit: back to the device's width.
    tick(&mut g, &mut disk, at(keys(&[Key::Escape])));
    tick(&mut g, &mut disk, at(idle()));
    for _ in 0..2 {
        tick(&mut g, &mut disk, at(keys(&[Key::R])));
        tick(&mut g, &mut disk, at(idle()));
    }
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(g.display.buffer_w, device);
}

/// A suspended match through a whole idle demo cycle, a duel included: no
/// file changes, no disk effect is asked for, the offer is untouched, and
/// RESUME MATCH afterwards is the match as played.
#[test]
fn a_suspended_match_survives_a_demo_cycle_byte_for_byte() {
    let (mut disk, live) = play_and_kill(4, Kill::MidFlight);
    let want = match_state(&live);
    let mut g = disk.launch();
    let files = disk.files();
    assert!(files[4].is_some(), "the match is on disk");
    let offered = g.saved_match.clone();
    assert!(offered.is_some());

    let mut during = Vec::new();
    for _ in 0..30_000 {
        let fx = g.tick(1.0 / 30.0, &idle());
        if g.in_demo() {
            during.extend(fx.iter().cloned());
        }
        disk.apply(&mut g, fx);
        if g.demo.as_ref().is_some_and(|d| d.leg_index == 2) {
            break;
        }
    }
    assert!(g.demo.as_ref().is_some_and(|d| d.leg_index == 2));
    let disk_fx: Vec<_> = during.iter().filter(|e| e.touches_disk()).collect();
    assert!(disk_fx.is_empty(), "the demo asked for {disk_fx:?}");
    assert_eq!(disk.files(), files, "the demo changed a file");
    assert_eq!(g.saved_match, offered);

    tick(&mut g, &mut disk, keys(&[Key::Space]));
    tick(&mut g, &mut disk, idle());
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(disk.files(), files);
    press(&mut g, &mut disk, Key::Num2);
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(match_state(&g), want);
}

/// The demo's own duels are never kept, even with no match suspended: no
/// match file appears, and a demo that ends mid-duel clears nothing.
#[test]
fn the_demo_never_writes_or_clears_a_match_file() {
    let mut disk = FakeDisk::default();
    let mut g = disk.launch();
    g.start_demo(DemoStarted::Menu);
    for _ in 0..30_000 {
        let fx = g.tick(1.0 / 30.0, &idle());
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::MatchFile(_))),
            "{fx:?}"
        );
        disk.apply(&mut g, fx);
        if g.mode == Mode::Versus && g.versus.live.as_ref().is_some_and(|v| !v.log.is_empty()) {
            break;
        }
    }
    assert_eq!(g.mode, Mode::Versus, "into the duel leg");
    assert!(!vs(&g).kept);
    // A tap mid-duel ends it: nothing to clear, nothing written.
    let fx = g.tick(1.0 / 30.0, &keys(&[Key::Space]));
    assert!(!fx.iter().any(Effect::touches_disk), "{fx:?}");
    assert_eq!(disk.files(), FakeDisk::default().files());
}

// ------------------------------------------------- the editor and its maps

/// Launch, open the pre-match screen and the editor (`2`, then `E`).
fn open_editor(disk: &mut FakeDisk) -> Game {
    let mut g = disk.launch();
    press(&mut g, disk, Key::Num2);
    assert_eq!(g.mode, Mode::VersusSetup);
    press(&mut g, disk, Key::E);
    assert_eq!(g.mode, Mode::Editor);
    g
}

fn draft(g: &Game) -> Option<&salt_line_sim::layout::Layout> {
    g.editor.ed.as_ref().map(|e| e.layout())
}

/// A blank map, then a few edits by keys: raise under the cursor, move
/// along, lower. Returns every layout the draft passed through.
fn blank_and_edit(g: &mut Game, disk: &mut FakeDisk) -> Vec<salt_line_sim::layout::Layout> {
    assert_eq!(g.editor.page, crate::app::EditorPage::Start);
    // The first row of a start page without a draft is NEW BLANK MAP.
    press(g, disk, Key::Enter);
    assert_eq!(g.editor.page, crate::app::EditorPage::Edit);
    let mut seen = vec![draft(g).unwrap().clone()];
    for step in [
        Key::Up,
        Key::Up,
        Key::Right,
        Key::Right,
        Key::Right,
        Key::Down,
    ] {
        press(g, disk, step);
        let now = draft(g).unwrap();
        if seen.last() != Some(now) {
            seen.push(now.clone());
        }
    }
    seen
}

/// Save the draft into the first slot offered: the menu (Esc), SAVE (one
/// row down), the slot list's first row.
fn save_first_slot(g: &mut Game, disk: &mut FakeDisk) {
    press(g, disk, Key::Escape);
    press(g, disk, Key::Down);
    press(g, disk, Key::Enter);
    assert!(matches!(
        g.editor.page,
        crate::app::EditorPage::Slots(crate::app::EditorPurpose::Save)
    ));
    press(g, disk, Key::Enter);
}

/// The draft kept on disk is the draft as it was edited, byte for byte, and
/// a relaunch opens the editor straight onto it.
#[test]
fn a_draft_survives_a_kill_byte_for_byte() {
    let mut disk = FakeDisk::default();
    let mut g = open_editor(&mut disk);
    let seen = blank_and_edit(&mut g, &mut disk);
    let line = crate::layouts_file::SavedDraft {
        draft: g.editor.ed.as_ref().unwrap().draft.clone(),
        mirror: true,
    }
    .to_line()
    .unwrap();
    assert_eq!(disk.draft_main.as_deref(), Some(line.as_str()));
    // Killed: the game is gone, the disk is what it left.
    drop(g);
    let files = disk.files();
    let mut g = disk.launch();
    assert_eq!(draft(&g), seen.last());
    assert_eq!(
        disk.files(),
        files,
        "the launch put the draft back as it was"
    );
    press(&mut g, &mut disk, Key::Num2);
    press(&mut g, &mut disk, Key::E);
    assert_eq!(
        g.editor.page,
        crate::app::EditorPage::Edit,
        "straight onto it"
    );
    assert!(g.editor.ed.as_ref().unwrap().mirror);
    // Nothing new written by opening it.
    assert_eq!(disk.files(), files);
}

/// Editing and saving, killed after every disk operation in turn: whatever
/// a launch finds is something the player made. Before the first edit
/// reached disk there may be nothing; after it, a draft the editing passed
/// through, or the saved map in its slot, or both.
#[test]
fn editing_and_saving_survive_a_crash_anywhere() {
    fn flow(disk: &mut FakeDisk) -> (Vec<salt_line_sim::layout::Layout>, usize) {
        let mut g = open_editor(disk);
        let seen = blank_and_edit(&mut g, disk);
        let first_write = disk.done;
        save_first_slot(&mut g, disk);
        (seen, first_write)
    }
    let mut whole = FakeDisk::default();
    let (seen, edited) = flow(&mut whole);
    assert!(whole.draft_main.is_none(), "saved: no draft left");
    let saved = seen.last().unwrap().clone();
    assert_eq!(
        crate::layouts_file::Slots::parse(whole.layouts.as_deref().unwrap())
            .get(1)
            .unwrap()
            .layout(),
        Ok(saved.clone())
    );
    let ops = whole.done;
    let mut found_draft = 0;
    for k in 0..=ops {
        let mut disk = FakeDisk {
            budget: Some(k),
            ..FakeDisk::default()
        };
        flow(&mut disk);
        disk.budget = None;
        for _ in 0..2 {
            let g = disk.launch();
            let slot = g.layouts.checked(1).and_then(|(l, _)| l.clone());
            if let Some(l) = draft(&g) {
                assert!(seen.contains(l), "crash after {k}: a draft never made");
                found_draft += 1;
            }
            if let Some(l) = &slot {
                assert_eq!(l, &saved, "crash after {k}: a slot never saved");
            }
            // Once the last edit had reached disk it is never lost again,
            // whatever the save that followed got to: a launch finds it as
            // the draft, in its slot, or both.
            if k >= edited {
                assert!(
                    draft(&g) == Some(&saved) || slot.as_ref() == Some(&saved),
                    "crash after {k}: the map was lost"
                );
            }
            if k >= ops {
                assert!(slot.is_some(), "the finished save is there");
                assert!(draft(&g).is_none(), "and no draft beside it");
            }
        }
    }
    assert!(found_draft > 0, "no crash left a draft to find");
}

/// The GROUND row's choice holds through a rematch, and a match on a map
/// killed mid-play comes back on that map, its line in format 2, with the
/// GROUND row showing it.
#[test]
fn a_custom_duel_survives_rematch_and_resume() {
    let mut disk = FakeDisk::default();
    let mut g = disk.launch();
    press(&mut g, &mut disk, Key::Num2);
    press(&mut g, &mut disk, Key::Down);
    let bundled = crate::custom::bundled::layouts()[0].clone();
    assert_eq!(
        g.versus.ground.as_ref().map(|p| p.layout.clone()),
        Some(bundled.clone())
    );
    press(&mut g, &mut disk, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(vs(&g).layout.as_ref(), Some(&bundled));
    for _ in 0..3 {
        match_turn(&mut g, &mut disk);
    }
    let line = disk.match_main.clone().expect("kept");
    assert!(line.starts_with("match 2 "), "{line}");
    assert!(line.contains(&bundled.to_code().unwrap()));
    let want = match_state(&g);
    // Killed, relaunched, resumed: the same match on the same map.
    drop(g);
    let mut g = relaunch_and_resume(&mut disk);
    assert_eq!(match_state(&g), want);
    assert_eq!(vs(&g).layout.as_ref(), Some(&bundled));
    assert_eq!(
        g.versus.ground.as_ref().map(|p| p.layout.clone()),
        Some(bundled.clone()),
        "the GROUND row shows the map it resumed on"
    );
    assert_eq!(
        g.display.buffer_w,
        salt_line_sim::layout::LAYOUT_WIDTH as i32
    );
    // Played to its end, then REMATCH: back on the pre-match screen with
    // the map still picked, and the rematch is on it.
    for _ in 0..200 {
        if matches!(vs(&g).duel.phase, crate::versus::Phase::MatchOver(_))
            && stage(&g) == versus::Stage::Result
        {
            break;
        }
        match_turn(&mut g, &mut disk);
    }
    press(&mut g, &mut disk, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert_eq!(
        g.versus.ground.as_ref().map(|p| p.layout.clone()),
        Some(bundled.clone())
    );
    press(&mut g, &mut disk, Key::Enter);
    assert_eq!(vs(&g).layout.as_ref(), Some(&bundled), "the rematch");
    assert!(disk.match_main.as_deref().unwrap().starts_with("match 2 "));
}

/// A match on a fixed-wind map (bundled SPIRES) killed at every turn of a
/// first round and into the second, and resumed: the same match, its wind
/// the pair rule's for the turn it resumes on.
#[test]
fn a_fixed_wind_match_resumes_on_the_right_wind() {
    let n = crate::custom::bundled::BUNDLED.len();
    let spires = crate::custom::bundled::layouts()[n - 1].clone();
    assert!(matches!(
        spires.wind,
        salt_line_sim::layout::WindSpec::Fixed(_)
    ));
    let mut rounds = 0;
    for turns in [0, 1, 2, 3, 4, 5, 7, 10, 13, 16] {
        let mut disk = FakeDisk::default();
        let mut g = disk.launch();
        press(&mut g, &mut disk, Key::Num2);
        for _ in 0..n {
            press(&mut g, &mut disk, Key::Down);
        }
        assert_eq!(g.versus.ground.as_ref().unwrap().layout, spires);
        press(&mut g, &mut disk, Key::Enter);
        for _ in 0..turns {
            if matches!(vs(&g).duel.phase, crate::versus::Phase::MatchOver(_)) {
                break;
            }
            match_turn(&mut g, &mut disk);
        }
        let want = match_state(&g);
        rounds = rounds.max(vs(&g).duel.round);
        drop(g);
        let back = relaunch_and_resume(&mut disk);
        assert_eq!(match_state(&back), want, "{turns} turns");
        let duel = &vs(&back).duel;
        assert_eq!(duel.screen.wind, duel.wind_at(duel.turn), "{turns} turns");
    }
    assert!(rounds >= 1, "the kills reached a second round");
}

/// The demo never reads or writes the slot file or the draft (DESIGN
/// §12.3, §13.8): through a whole demo cycle every file is byte for byte as
/// it was, no disk effect is asked for, and the editor afterwards opens on
/// the same draft.
#[test]
fn the_demo_leaves_the_maps_and_the_draft_alone() {
    let mut disk = FakeDisk::default();
    let mut g = open_editor(&mut disk);
    blank_and_edit(&mut g, &mut disk);
    save_first_slot(&mut g, &mut disk);
    press(&mut g, &mut disk, Key::Up);
    assert!(disk.layouts.is_some() && disk.draft_main.is_some());
    let made = draft(&g).unwrap().clone();
    drop(g);
    let mut g = disk.launch();
    let files = disk.files();
    let mut during = Vec::new();
    for _ in 0..30_000 {
        let fx = g.tick(1.0 / 30.0, &idle());
        if g.in_demo() {
            during.extend(fx.iter().cloned());
        }
        disk.apply(&mut g, fx);
        if g.demo.as_ref().is_some_and(|d| d.leg_index == 2) {
            break;
        }
    }
    assert!(g.demo.is_some(), "the idle demo ran");
    let disk_fx: Vec<_> = during.iter().filter(|e| e.touches_disk()).collect();
    assert!(disk_fx.is_empty(), "the demo asked for {disk_fx:?}");
    assert!(g.versus.ground.is_none(), "the demo's duels are generated");
    assert_eq!(disk.files(), files, "the demo changed a file");
    tick(&mut g, &mut disk, keys(&[Key::Space]));
    tick(&mut g, &mut disk, idle());
    assert_eq!(g.mode, Mode::Title);
    press(&mut g, &mut disk, Key::Num2);
    press(&mut g, &mut disk, Key::E);
    assert_eq!(draft(&g), Some(&made));
    assert_eq!(disk.files(), files);
}

/// A newer build's draft is never written over or cleared: a map made
/// meanwhile is not kept as a draft, and saving it leaves that file alone.
#[test]
fn a_newer_builds_draft_survives_editing_and_saving() {
    let newer = "draft 9 1 - - SL00\n";
    let mut disk = FakeDisk {
        draft_main: Some(newer.into()),
        ..FakeDisk::default()
    };
    let mut g = open_editor(&mut disk);
    assert!(g.editor.newer_draft);
    blank_and_edit(&mut g, &mut disk);
    assert_eq!(disk.draft_main.as_deref(), Some(newer));
    save_first_slot(&mut g, &mut disk);
    assert!(disk.layouts.is_some(), "the map was saved");
    assert_eq!(disk.draft_main.as_deref(), Some(newer), "and the file kept");
}

/// A draft file this build cannot read is dropped with a note; one from a
/// newer build is put back unread, for that build.
#[rstest::rstest]
#[case::damaged("draft 1 1 - - banana\n", false)]
#[case::newer("draft 9 1 - - SL00\n", true)]
fn a_draft_that_will_not_do(#[case] text: &str, #[case] kept: bool) {
    let mut disk = FakeDisk {
        draft_main: Some(text.into()),
        ..FakeDisk::default()
    };
    let g = disk.launch();
    assert!(g.editor.ed.is_none());
    if kept {
        assert_eq!(disk.draft_main.as_deref(), Some(text));
        assert!(g.editor.newer_draft);
        assert_eq!(g.status, crate::app::editor_newer_draft_note());
    } else {
        assert_eq!(disk.draft_main, None);
        assert_eq!(g.status, "MAP DRAFT COULD NOT BE RESTORED");
    }
}

/// A draft the last launch died reading is dropped, with the same note.
#[test]
fn an_interrupted_draft_is_dropped_with_a_note() {
    let mut disk = FakeDisk {
        draft_aside: Some("draft 1 1 - - SL00\n".into()),
        ..FakeDisk::default()
    };
    let g = disk.launch();
    assert!(g.editor.ed.is_none());
    assert_eq!(disk.draft_aside, None);
    assert_eq!(g.status, "MAP DRAFT COULD NOT BE RESTORED");
}

/// A match suspended by MAKE MAP OF THIS ROUND, then a map made in the
/// editor, killed there after every disk operation in turn: a launch offers
/// at most the match as played, and at most a draft the editing passed
/// through; with the whole flow on disk it offers both, RESUME MATCH and
/// the editor reopening on the draft.
#[test]
fn a_suspended_match_and_a_draft_survive_a_crash_anywhere() {
    fn flow(disk: &mut FakeDisk) -> (Vec<match_log::Entry>, Vec<salt_line_sim::layout::Layout>) {
        let mut g = start_match(disk);
        for _ in 0..3 {
            match_turn(&mut g, disk);
        }
        let log = vs(&g).log.clone();
        press(&mut g, disk, Key::Escape);
        press(&mut g, disk, Key::E);
        assert_eq!(g.mode, Mode::Editor);
        assert!(g.saved_match.is_some());
        let mut seen = vec![draft(&g).unwrap().clone()];
        for step in [Key::Up, Key::Up, Key::Right, Key::Down] {
            press(&mut g, disk, step);
            let now = draft(&g).unwrap();
            if seen.last() != Some(now) {
                seen.push(now.clone());
            }
        }
        (log, seen)
    }
    let mut whole = FakeDisk::default();
    let (log, seen) = flow(&mut whole);
    let ops = whole.done;
    for k in 0..=ops {
        let mut disk = FakeDisk {
            budget: Some(k),
            ..FakeDisk::default()
        };
        flow(&mut disk);
        disk.budget = None;
        let mut g = disk.launch();
        if let Some(saved) = &g.saved_match {
            assert!(log.starts_with(&saved.log), "crash after {k}");
        }
        if let Some(l) = draft(&g) {
            assert!(seen.contains(l), "crash after {k}: a draft never made");
        }
        if k == ops {
            assert_eq!(
                g.saved_match.as_ref().map(|s| s.log.clone()),
                Some(log.clone())
            );
            assert_eq!(draft(&g), seen.last());
            press(&mut g, &mut disk, Key::Num2);
            assert_eq!(g.mode, Mode::Versus, "RESUME MATCH");
            assert_eq!(vs(&g).log, log);
        }
    }
    // And the editor reopens on the draft, the match still offered.
    let mut g = whole.launch();
    assert!(g.saved_match.is_some());
    g.open_editor();
    assert_eq!(g.editor.page, crate::app::EditorPage::Edit);
    assert_eq!(draft(&g), seen.last());
}

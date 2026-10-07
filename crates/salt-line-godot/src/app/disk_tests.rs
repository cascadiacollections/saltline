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
    fn files(&self) -> [Option<String>; 6] {
        [
            self.save.clone(),
            self.run.clone(),
            self.temp.clone(),
            self.set_aside.clone(),
            self.crash_report.clone(),
            self.old_crash_report.clone(),
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

//! Persistence end to end: the core's effects applied to an in-memory disk
//! the way the node applies them to `user://`, with the process killed after
//! every effect in turn and the game launched again on what was left.
//!
//! What must hold whatever the moment of the crash: a finished run is on the
//! board at most once, a saved run that is offered is the run as it was
//! played, and last session's crash report is never lost.

use std::collections::BTreeMap;

use super::*;
use pretty_assertions::assert_eq;
use salt_line_sim::run::Phase;

use crate::crash;
use crate::input::{Intents, Key};
use crate::resume::{RUN_LOADING_PATH, RUN_PATH, RUN_TEMP_PATH};

const FRAME: f64 = 1.0 / 60.0;
const WINDOW: (i32, i32) = (1280, 720);
const TODAY: u64 = 20_000;
const SAVE: &str = "user://salt_line.txt";

fn idle() -> Intents {
    Intents {
        window: WINDOW,
        today: TODAY,
        ..Intents::default()
    }
}

fn keys(keys: &[Key]) -> Intents {
    Intents {
        keys: keys.to_vec(),
        keys_held: keys.to_vec(),
        ..idle()
    }
}

/// `user://` in memory. Every write is atomic, as `durable::write_atomic`
/// makes it. `budget` is how many more operations happen before the process
/// dies; after that nothing reaches the disk.
#[derive(Clone, Default)]
struct FakeDisk {
    files: BTreeMap<&'static str, String>,
    budget: Option<usize>,
    /// Operations carried out, for sizing the crash loop.
    done: usize,
    /// Score writes still to fail, as a full disk would.
    fail_scores: usize,
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

    fn rename(&mut self, from: &'static str, to: &'static str) {
        if let Some(text) = self.files.remove(from) {
            self.files.insert(to, text);
        }
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
                    let fail = matches!(why, Store::Score { .. }) && self.fail_scores > 0;
                    let ok = if fail {
                        self.fail_scores -= 1;
                        false
                    } else {
                        let text = g.save_to_store(&why).serialize();
                        self.files.insert(SAVE, text);
                        true
                    };
                    let more = g.stored(why, ok);
                    self.apply(g, more);
                }
                Effect::WriteRun(line) | Effect::KeepRun(line) => {
                    self.files.insert(RUN_PATH, line);
                }
                Effect::DiscardRunFile => {
                    self.files.remove(RUN_TEMP_PATH);
                    self.files.remove(RUN_PATH);
                }
                Effect::Rename { from, to } => self.rename(from, to),
                Effect::Remove(path) => {
                    self.files.remove(path);
                }
                Effect::RetireCrashReport => {
                    self.rename(crash::REPORT_PATH, crash::OLD_REPORT_PATH);
                }
                _ => {}
            }
        }
    }

    /// Launch the game on what is on disk, as the node's `ready` does.
    fn launch(&mut self) -> Game {
        let save = self
            .files
            .get(SAVE)
            .map(|t| Save::parse(t))
            .unwrap_or_default();
        let mut g = Game::new(false, save);
        g.last_crash = self.files.get(crash::REPORT_PATH).cloned();
        let found = if self.files.contains_key(RUN_LOADING_PATH) {
            Some(SavedRun::Interrupted)
        } else {
            let path = [RUN_PATH, RUN_TEMP_PATH]
                .into_iter()
                .find(|p| self.files.contains_key(p));
            match path {
                Some(p) if !self.files[p].trim().is_empty() => {
                    let text = self.files[p].clone();
                    if self.alive() {
                        self.rename(p, RUN_LOADING_PATH);
                    }
                    Some(SavedRun::Found(text))
                }
                _ => None,
            }
        };
        if let Some(found) = found {
            let fx = g.saved_run(found);
            self.apply(&mut g, fx);
        }
        let fx = g.fit_display(WINDOW, (0, 0));
        self.apply(&mut g, fx);
        g
    }

    fn board(&self) -> Vec<Entry> {
        self.files
            .get(SAVE)
            .map(|t| Save::parse(t).scores)
            .unwrap_or_default()
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
    g.angle_ticks = 1400;
    g.power_ticks = 0;
    tick(g, disk, keys(&[Key::Space]));
    until(g, disk, |g| g.anim.is_none() && g.clear_hold <= 0.0);
}

/// Throw every shell away, as a player out of luck would, so the log
/// replays to a finished run.
fn throw_all(g: &mut Game, disk: &mut FakeDisk) {
    while g.mode != Mode::GameOver {
        if g.run.as_ref().unwrap().phase == Phase::Drafting {
            tick(g, disk, keys(&[Key::Num1]));
        } else {
            throw_one(g, disk);
        }
        tick(g, disk, idle());
    }
}

/// What a flow leaves to check: the run it played.
struct Played {
    seed: u64,
    log: Vec<salt_line_sim::run::LoggedShot>,
}

/// Launch, start today's daily, throw every shell away.
fn finish_a_run(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    let run = g.run.as_ref().unwrap();
    Played {
        seed: run.run_seed,
        log: run.log.clone(),
    }
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
    let run = g.run.as_ref().unwrap();
    Played {
        seed: run.run_seed,
        log: run.log.clone(),
    }
}

/// Launch over a saved run, continue it, fire once more, then finish it.
fn continue_and_finish(disk: &mut FakeDisk) -> Played {
    let mut g = disk.launch();
    tick(&mut g, disk, keys(&[Key::Enter]));
    throw_all(&mut g, disk);
    let run = g.run.as_ref().unwrap();
    Played {
        seed: run.run_seed,
        log: run.log.clone(),
    }
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
        let save = self
            .files
            .get(SAVE)
            .map(|t| Save::parse(t))
            .unwrap_or_default();
        let mut g = Game::new(false, save);
        g.fit_display(WINDOW, (0, 0));
        g
    }
}

/// A disk with last session's crash report on it.
fn with_crash_report() -> FakeDisk {
    let mut disk = FakeDisk::default();
    disk.files
        .insert(crash::REPORT_PATH, "SALT LINE CRASH REPORT".into());
    disk
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
        let first = disk.launch();
        if let Some(saved) = first.resumable.as_ref() {
            assert_eq!(saved.seed, played.seed, "crash after {k}");
            assert!(
                played.log.starts_with(&saved.log),
                "crash after {k}: the offered run is not the one played"
            );
        }
        disk.launch();
        let scored = disk
            .board()
            .iter()
            .filter(|e| e.seed == played.seed)
            .count();
        assert!(scored <= 1, "crash after {k}: scored {scored} times");
        if start.files.contains_key(crash::REPORT_PATH) {
            assert!(
                disk.files.contains_key(crash::REPORT_PATH)
                    || disk.files.contains_key(crash::OLD_REPORT_PATH),
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
    assert!(!disk.files.contains_key(RUN_PATH));
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
    let saved = g.resumable.as_ref().expect("offered");
    assert_eq!(saved.log, played.log);
}

#[test]
fn continuing_a_run_survives_a_crash_anywhere() {
    let mut start = FakeDisk::default();
    quit_mid_run(&mut start);
    start.done = 0;
    crash_everywhere(start, continue_and_finish);
}

#[test]
fn a_score_that_cannot_be_written_is_scored_once_later() {
    let mut disk = FakeDisk::default();
    let played = score_write_fails(&mut disk);
    // The settings write did not take the score with it.
    assert!(disk.board().is_empty());
    assert!(
        disk.files.contains_key(RUN_PATH),
        "the finished run was kept"
    );
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

// ----------------------------------------------------------- after a panic

#[test]
fn nothing_queued_before_a_panic_is_replayed_after_it() {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g.last_crash = Some("report".into());
    // A frame that got this far and panicked: a run started, its crash
    // report retired and the run file written, all still queued.
    g.start_run(1, false);
    g.finish_run();
    assert!(!g.fx.is_empty());
    let fx = g.broken_tick(FRAME, &idle(), || None);
    assert_eq!(fx, vec![]);
    assert!(g.focus_lost().iter().all(Effect::survives_panic));
    assert_eq!(g.closing(), vec![]);
}

#[test]
fn the_node_drops_every_file_effect_after_a_panic() {
    let dropped = [
        Effect::Store(Store::Settings),
        Effect::WriteRun(String::new()),
        Effect::KeepRun(String::new()),
        Effect::DiscardRunFile,
        Effect::Rename {
            from: RUN_LOADING_PATH,
            to: RUN_PATH,
        },
        Effect::Remove(RUN_LOADING_PATH),
        Effect::RetireCrashReport,
    ];
    assert!(dropped.iter().all(|e| !e.survives_panic()));
    let kept = [
        Effect::Play(Sound::Ui),
        Effect::Redraw,
        Effect::Clipboard(String::new()),
        Effect::Quit,
        Effect::Warn(String::new()),
    ];
    assert!(kept.iter().all(Effect::survives_panic));
}

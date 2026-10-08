//! Watching a finished run or match again (`app/watch.rs`), stepped as the
//! node steps it: what is recorded and when, that a watch replays exactly
//! what was played and changes nothing the player keeps, its controls, and
//! the REPLAYS list.

use super::*;
use pretty_assertions::assert_eq;

use crate::app::versus::Stage;
use crate::history::tests::{finished_match, finished_run};
use crate::history::{Ground, Kind, Record};
use crate::input::{Event, Intents, Key, Wb};
use crate::match_log::SavedMatch;
use crate::opponent::Level;
use crate::versus::{self as rules, Side};
use salt_line_sim::replay::state_hash;

const FRAME: f64 = 1.0 / 60.0;
const WINDOW: (i32, i32) = (1280, 720);
const TODAY: u64 = 20_734;

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

/// A key pressed for a frame and let go for one.
fn press(g: &mut Game, key: Key) -> Vec<Effect> {
    let mut fx = g.tick(FRAME, &keys(&[key]));
    fx.extend(g.tick(FRAME, &idle()));
    fx
}

fn game() -> Game {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g.tick(FRAME, &idle());
    g.take_effects();
    g
}

/// Idle frames until `done`, or panic after `seconds`. Every effect.
fn until(g: &mut Game, seconds: f64, done: impl Fn(&Game) -> bool) -> Vec<Effect> {
    let mut fx = Vec::new();
    for _ in 0..(seconds / FRAME) as usize {
        if done(g) {
            return fx;
        }
        fx.extend(g.tick(FRAME, &idle()));
    }
    assert!(done(g), "not done after {seconds} s");
    fx
}

fn ended(g: &Game) -> bool {
    g.watch.as_ref().is_some_and(|w| w.ended)
}

/// A run kept on disk, played to its end by the sim's solver on `seed`
/// and finished in the game: on its result screen, recorded. Returns the
/// game, the run as it ended, and the effects of its end.
fn finished(seed: u64) -> (Game, RunState, Vec<Effect>) {
    let mut g = game();
    g.start_run(seed, false);
    g.tick(FRAME, &idle());
    g.take_effects();
    let run = finished_run(seed, g.display.buffer_w as usize, 5);
    g.play.run = Some(run.clone());
    g.finish_run();
    let fx = g.take_effects();
    assert_eq!(g.mode, Mode::GameOver);
    (g, run, fx)
}

fn history_writes(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::History(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A won match against the AI, resumed onto its result plate as a kept
/// match: the plate's WATCH is on screen.
fn on_won_plate() -> (Game, SavedMatch, rules::Match) {
    let mut g = game();
    let (saved, duel) = finished_match(31, Some(Level::Easy));
    g.saved_match = Some(saved.clone());
    g.resume_match();
    g.tick(FRAME, &idle());
    g.take_effects();
    assert_eq!(g.mode, Mode::Versus);
    let vs = g.versus.live.as_ref().unwrap();
    assert_eq!(vs.stage, Stage::Result);
    assert!(matches!(vs.duel.phase, rules::Phase::MatchOver(_)));
    (g, saved, duel)
}

// ------------------------------------------------------------- recording

#[test]
fn a_finished_run_is_recorded_after_its_score() {
    let (g, run, fx) = finished(9);
    let score = fx
        .iter()
        .position(|e| matches!(e, Effect::Store(Store::Score { .. })))
        .unwrap();
    let write = fx
        .iter()
        .position(|e| matches!(e, Effect::History(_)))
        .expect("recorded");
    assert!(write > score, "the score goes first: {fx:?}");
    let text = &history_writes(&fx)[0];
    let read = crate::history::History::parse(text);
    assert_eq!(read.records.len(), 1);
    assert_eq!(read.records[0].day, TODAY);
    match &read.records[0].kind {
        Kind::Run(r) => {
            assert_eq!((r.seed, r.score), (9, run.score()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(g.history.last_run.as_ref(), Some(&read.records[0]));
}

#[test]
fn a_run_its_log_cannot_rebuild_is_not_recorded_or_offered() {
    let mut g = game();
    g.start_run(9, false);
    g.jump_to(4);
    g.finish_run();
    let fx = g.take_effects();
    assert!(history_writes(&fx).is_empty());
    assert!(g.history.last_run.is_none());
    // So the result screen offers no WATCH, and W does nothing.
    press(&mut g, Key::W);
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::GameOver);
}

#[test]
fn a_won_match_is_recorded_as_its_plate_is_left_and_a_quit_one_is_not() {
    let (mut g, saved, duel) = on_won_plate();
    let fx = press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    let writes = history_writes(&fx);
    assert_eq!(writes.len(), 1);
    // Recorded where the match file is cleared.
    let clear = fx
        .iter()
        .position(|e| *e == Effect::MatchFile(MatchFile::Discard))
        .unwrap();
    let write = fx.iter().position(|e| matches!(e, Effect::History(_)));
    assert_eq!(write, Some(clear + 1));
    let read = crate::history::History::parse(&writes[0]);
    let want = Record::duel(TODAY, &saved, &duel, Ground::Generated).unwrap();
    assert_eq!(read.records, [want]);

    // A match quit before it is won: cleared, never recorded.
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    press(&mut g, Key::Escape);
    let mut fx = press(&mut g, Key::R);
    fx.extend(press(&mut g, Key::R));
    assert_eq!(g.mode, Mode::Title);
    assert!(fx.contains(&Effect::MatchFile(MatchFile::Discard)));
    assert!(history_writes(&fx).is_empty());
    assert_eq!(g.history.file.records.len(), 1);
}

#[test]
fn a_finished_run_a_launch_scores_is_not_recorded() {
    // The app died on the run's last shot: launch scores it, and leaves
    // the history alone (DESIGN §9.1).
    let mut g = game();
    let run = finished_run(9, 320, 5);
    let line = crate::resume::Saved::of(&run, false).to_line();
    let fx = g.saved_run(SavedRun::Found(line));
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::Store(Store::Score { .. }))));
    assert!(history_writes(&fx).is_empty());
    assert!(g.history.file.records.is_empty());
}

// ------------------------------------------------------------------ watch

#[test]
fn watching_a_run_replays_it_exactly_and_writes_nothing() {
    let (mut g, run, _) = finished(9);
    let before = g.play.run.as_ref().map(state_hash);
    let fx = press(&mut g, Key::W);
    assert!(g.watching());
    assert_eq!(g.mode, Mode::Playing);
    // From the start: the first screen, a full reserve, nothing logged.
    assert!(g.play.run.as_ref().unwrap().log.len() <= 1);
    let mut all = fx;
    all.extend(until(&mut g, 600.0, ended));
    assert!(!all.iter().any(Effect::touches_disk), "{all:?}");
    // Not muted: the shots are heard at the player's volume.
    assert!(all.contains(&Effect::Play(Sound::Fire)));
    assert!(!all.iter().any(|e| matches!(e, Effect::Volume(_))));
    // The same run, bit for bit, as it ended.
    let watched = g.play.run.as_ref().unwrap();
    assert_eq!(state_hash(watched), state_hash(&run));
    assert_eq!(watched.log, run.log);
    // Exit: back on the result screen, the run there untouched.
    let fx = press(&mut g, Key::Escape);
    assert!(!fx.iter().any(Effect::touches_disk));
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::GameOver);
    assert_eq!(g.play.run.as_ref().map(state_hash), before);
}

#[test]
fn watching_a_match_replays_it_and_goes_back_to_its_plate() {
    let (mut g, saved, duel) = on_won_plate();
    let plate_before = rules_hash(&g);
    press(&mut g, Key::W);
    assert!(g.watching());
    let vs = g.versus.live.as_ref().unwrap();
    assert_eq!(vs.duel.round, 0);
    assert_eq!(vs.stage, Stage::Handoff);
    assert_eq!(vs.ai, Some(Level::Easy), "YOU and AI on the plates");
    assert!(!vs.kept);
    let fx = until(&mut g, 900.0, ended);
    assert!(!fx.iter().any(Effect::touches_disk), "{fx:?}");
    assert!(fx.contains(&Effect::Play(Sound::Win)));
    let vs = g.versus.live.as_ref().unwrap();
    assert_eq!(
        crate::match_log::state_hash(&vs.duel),
        crate::match_log::state_hash(&duel)
    );
    assert_eq!(vs.log, saved.log);
    // Back, on the very plate it was started from: WATCH again, REMATCH.
    press(&mut g, Key::Escape);
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(rules_hash(&g), plate_before);
    assert!(g.versus.live.as_ref().unwrap().kept);
}

fn rules_hash(g: &Game) -> u64 {
    crate::match_log::state_hash(&g.versus.live.as_ref().unwrap().duel)
}

#[test]
fn a_watch_set_over_a_suspended_run_leaves_it_and_continue_alone() {
    // A run quit to the title, kept for CONTINUE, then another watched
    // from the list: everything kept is as it was.
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::Title);
    g.start_run(5, false);
    g.tick(FRAME, &idle());
    g.play.angle_ticks = 1400;
    g.play.power_ticks = 0;
    press(&mut g, Key::Space);
    until(&mut g, 10.0, |g| g.play.anim.is_none());
    press(&mut g, Key::Escape);
    press(&mut g, Key::R);
    assert!(g.run_file.resumable.is_some());
    let kept = (
        g.run_file.resumable.clone(),
        g.play.run.as_ref().map(state_hash),
        g.save.serialize(),
        g.display.buffer_w,
    );
    g.take_effects();
    press(&mut g, Key::W);
    assert_eq!(g.mode, Mode::Replays);
    let mut fx = press(&mut g, Key::Enter);
    assert!(g.watching());
    fx.extend(until(&mut g, 600.0, ended));
    fx.extend(press(&mut g, Key::Escape));
    assert!(!fx.iter().any(Effect::touches_disk), "{fx:?}");
    assert_eq!(g.mode, Mode::Replays);
    assert_eq!(
        kept,
        (
            g.run_file.resumable.clone(),
            g.play.run.as_ref().map(state_hash),
            g.save.serialize(),
            g.display.buffer_w,
        )
    );
}

/// Seconds of play a watch of `g`'s last run takes to end at `speed`
/// (an index into `SPEEDS`).
fn seconds_to_end(speed: usize) -> f64 {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    for _ in 0..speed {
        press(&mut g, Key::Up);
    }
    assert_eq!(
        g.watch.as_ref().unwrap().rate(),
        super::watch::SPEEDS[speed]
    );
    let mut frames = 0;
    while !ended(&g) {
        g.tick(FRAME, &idle());
        frames += 1;
        assert!(frames < 60 * 900, "never ended");
    }
    frames as f64 * FRAME
}

#[test]
fn the_speeds_play_it_two_and_four_times_as_fast() {
    let one = seconds_to_end(0);
    let two = seconds_to_end(1);
    let four = seconds_to_end(2);
    // Each stage of each shot ends on a whole frame, at any speed: within
    // that much.
    assert!((1.85..2.15).contains(&(one / two)), "{one} {two}");
    assert!((3.5..4.5).contains(&(one / four)), "{one} {four}");
    // And it wraps back to 1x.
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    for _ in 0..3 {
        press(&mut g, Key::Up);
    }
    assert_eq!(g.watch.as_ref().unwrap().rate(), 1);
}

#[test]
fn pause_holds_everything_and_lets_the_screen_sleep() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    until(&mut g, 30.0, |g| g.play.anim.is_some());
    assert!(g.screen_awake_wanted(), "watched: the screen is held");
    press(&mut g, Key::Enter);
    assert!(g.watch.as_ref().unwrap().paused);
    assert!(!g.screen_awake_wanted());
    let t = g.play.anim.as_ref().unwrap().t;
    let cursor = g.watch.as_ref().unwrap().cursor;
    for _ in 0..300 {
        g.tick(FRAME, &idle());
    }
    assert_eq!(g.play.anim.as_ref().unwrap().t, t);
    assert_eq!(g.watch.as_ref().unwrap().cursor, cursor);
    press(&mut g, Key::Space);
    assert!(!g.watch.as_ref().unwrap().paused);
    assert!(g.screen_awake_wanted());
}

#[test]
fn focus_lost_pauses_a_watch_instead_of_the_run() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    g.focus_lost(&[]);
    assert!(g.watching());
    assert_eq!(g.mode, Mode::Playing, "not the run's pause plate");
    assert!(g.watch.as_ref().unwrap().paused);
    g.focus_gained();
}

#[test]
fn next_shot_fires_the_next_shot_at_once_and_silently() {
    let (mut g, run, _) = finished(9);
    press(&mut g, Key::W);
    let shots = |g: &Game| g.watch.as_ref().unwrap().fired;
    assert_eq!(shots(&g), 0);
    let fx = press(&mut g, Key::Right);
    assert_eq!(shots(&g), 1);
    assert!(g.play.anim.is_some(), "the shot skipped to is shown flying");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Play(Sound::Fire))));
    press(&mut g, Key::Right);
    assert_eq!(shots(&g), 2);
    // Skipping all the way still ends on the run as it was played.
    let total = run.log.iter().filter(|e| e.angle_ticks >= 0).count();
    for _ in 0..total + 2 {
        press(&mut g, Key::Right);
    }
    until(&mut g, 30.0, ended);
    assert_eq!(state_hash(g.play.run.as_ref().unwrap()), state_hash(&run));
}

#[test]
fn next_round_moves_on_to_the_next_screen_or_round() {
    let (mut g, run, _) = finished(9);
    assert!(run.screens_cleared >= 1, "the test run clears a screen");
    press(&mut g, Key::W);
    press(&mut g, Key::Down);
    let r = g.play.run.as_ref().unwrap();
    assert_eq!(r.screen_index, 1);
    assert!(g.play.anim.is_none() && g.play.clear_hold <= 0.0);

    let (mut g, _, duel) = on_won_plate();
    assert!(duel.round >= 1);
    press(&mut g, Key::W);
    press(&mut g, Key::Down);
    let vs = g.versus.live.as_ref().unwrap();
    assert_eq!((vs.duel.round, vs.stage), (1, Stage::Handoff));
    // Past the last round: the end, the match won as it was.
    for _ in 0..4 {
        press(&mut g, Key::Down);
    }
    until(&mut g, 60.0, ended);
    let vs = g.versus.live.as_ref().unwrap();
    assert_eq!(vs.duel.phase, duel.phase);
}

#[test]
fn again_plays_it_from_the_start() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    for _ in 0..200 {
        press(&mut g, Key::Right);
        if ended(&g) {
            break;
        }
    }
    until(&mut g, 30.0, ended);
    press(&mut g, Key::Enter);
    let w = g.watch.as_ref().unwrap();
    assert!(!w.ended && w.cursor == 0 && w.fired == 0);
    assert!(g.play.run.as_ref().unwrap().log.is_empty());
}

#[test]
fn a_replay_that_does_not_match_is_never_shown() {
    let (mut g, _, _) = finished(9);
    if let Some(Record {
        kind: Kind::Run(r), ..
    }) = g.history.last_run.as_mut()
    {
        r.score.0 += 1;
    }
    press(&mut g, Key::W);
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::GameOver);
    assert_eq!(g.status, crate::history::Refused::Mismatch.note());
}

// ------------------------------------------------------------------- list

#[test]
fn the_list_shows_newest_first_and_watches_the_chosen_row() {
    let (mut g, _, _) = finished(9);
    let (mut other, _, _) = finished(11);
    let newer = other.history.last_run.take().unwrap();
    g.history.file.push(Record {
        day: TODAY + 1,
        ..newer
    });
    press(&mut g, Key::Enter);
    press(&mut g, Key::W);
    assert_eq!(g.mode, Mode::Replays);
    assert_eq!(g.history.focus, 0);
    press(&mut g, Key::Down);
    assert_eq!(g.history.focus, 1);
    press(&mut g, Key::Down);
    assert_eq!(g.history.focus, 1, "stops at the last row");
    press(&mut g, Key::Up);
    press(&mut g, Key::Enter);
    assert!(g.watching());
    assert_eq!(g.play.run.as_ref().unwrap().run_seed, 11, "newest first");
    press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Replays);
    press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn an_older_versions_row_is_listed_but_says_why_it_cannot_be_watched() {
    let mut g = game();
    let line = format!(
        "history 1\nrun 1 20000 {} 42 320 0 3 0 41 v9 @0 Q7 900:600\n",
        salt_line_sim::SIM_EPOCH - 1
    );
    g.set_history(Some(&line));
    press(&mut g, Key::W);
    assert_eq!(g.history.file.records.len(), 1);
    let fx = press(&mut g, Key::Enter);
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::Replays);
    assert_eq!(
        g.status,
        crate::history::Refused::OlderVersion.note(),
        "said, not shown"
    );
    assert!(!fx.iter().any(Effect::touches_disk));
}

#[test]
fn a_touchscreen_watches_by_tapping_a_row_and_drives_the_bar() {
    let mut g = Game::new(true, Save::default());
    g.fit_display(WINDOW, (0, 0));
    let (_, run, _) = finished(9);
    g.history.file.push(Record::run(TODAY, &run, false));
    touch_frame(&mut g, Vec::new());
    tap(&mut g, Action::Watch);
    assert_eq!(g.mode, Mode::Replays);
    tap(&mut g, Action::Wb(Wb::Row(0)));
    assert!(g.watching());
    // The bar: pause, speed, next shot (a step, still paused), exit.
    tap(&mut g, Action::Wb(Wb::Pause));
    assert!(g.watch.as_ref().unwrap().paused);
    let fired = g.watch.as_ref().unwrap().fired;
    tap(&mut g, Action::Wb(Wb::Speed));
    assert_eq!(g.watch.as_ref().unwrap().rate(), 2);
    tap(&mut g, Action::Wb(Wb::NextShot));
    assert_eq!(g.watch.as_ref().unwrap().fired, fired + 1);
    assert!(g.watch.as_ref().unwrap().paused);
    tap(&mut g, Action::Wb(Wb::Exit));
    assert!(!g.watching());
    assert_eq!(g.mode, Mode::Replays);
}

fn touch_frame(g: &mut Game, events: Vec<Event>) -> Vec<Effect> {
    let fx = g.tick(FRAME, &Intents { events, ..idle() });
    let mut canvas = crate::raster::Canvas::new(g.display.buffer_w, g.display.buffer_h);
    let mut buttons = Vec::new();
    crate::render::frame(
        g,
        &crate::render::Art::default(),
        &mut canvas,
        &mut buttons,
        &mut crate::render::Cache::default(),
    );
    g.set_buttons(&buttons);
    fx
}

/// A finger tapping `action`'s button, held as a phone's tap is. Every
/// effect.
fn tap(g: &mut Game, action: Action) -> Vec<Effect> {
    let b = g
        .input
        .buttons
        .iter()
        .find(|b| b.action == action)
        .unwrap_or_else(|| panic!("no {action:?} button on screen"));
    let (x, y) = (
        b.rect.x as f32 + b.rect.w as f32 / 2.0,
        b.rect.y as f32 + b.rect.h as f32 / 2.0,
    );
    let mut fx = touch_frame(g, vec![Event::FingerDown { index: 0, x, y }]);
    for _ in 0..7 {
        fx.extend(touch_frame(g, Vec::new()));
    }
    fx.extend(touch_frame(g, vec![Event::FingerUp { index: 0 }]));
    for _ in 0..20 {
        fx.extend(touch_frame(g, Vec::new()));
    }
    fx
}

#[test]
fn the_won_plates_watch_needs_the_match_won() {
    // A round's plate has no WATCH: W does nothing there.
    let (saved, _) = finished_match(31, None);
    let mut g = game();
    let mut first_round = saved.clone();
    let cut = first_round
        .log
        .iter()
        .position(|e| *e == crate::match_log::Entry::NextRound)
        .unwrap();
    first_round.log.truncate(cut);
    g.saved_match = Some(first_round);
    g.resume_match();
    g.tick(FRAME, &idle());
    assert_eq!(g.versus.live.as_ref().unwrap().stage, Stage::Result);
    press(&mut g, Key::W);
    assert!(!g.watching());
    let _ = Side::Left;
}

#[test]
fn a_watch_is_shown_not_played_and_idles_when_paused_or_done() {
    let (mut g, _, _) = finished(9);
    // A run in play is active play: GameMode on desktop, the full rate.
    let mut playing = game();
    playing.start_run(3, false);
    assert!(playing.active_play() && !playing.idle());

    press(&mut g, Key::W);
    assert!(g.watching());
    // Playing: the full frame rate, but never GameMode.
    assert!(!g.active_play());
    assert!(!g.idle());
    press(&mut g, Key::Enter);
    assert!(g.watch.as_ref().unwrap().paused);
    assert!(g.idle(), "paused: the idle frame rate");
    assert!(!g.active_play());
    press(&mut g, Key::Enter);
    for _ in 0..200 {
        press(&mut g, Key::Right);
        if ended(&g) {
            break;
        }
    }
    until(&mut g, 30.0, ended);
    assert!(g.idle(), "at its end: the idle frame rate");
    assert!(!g.active_play());
}

#[test]
fn mute_and_the_overlay_work_while_watching_and_mute_is_saved_after() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    let mut fx = press(&mut g, Key::M);
    assert!(g.save.muted);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::Volume(v) if *v <= -80.0)));
    fx.extend(press(&mut g, Key::F1));
    assert!(g.debug);
    // Still watching, and nothing written while it plays.
    for _ in 0..120 {
        fx.extend(g.tick(FRAME, &idle()));
    }
    assert!(g.watching());
    assert!(!fx.iter().any(Effect::touches_disk), "{fx:?}");
    // Once the watch is over, the setting goes to disk as any does.
    assert!(g.settings.dirty(), "kept pending through the watch");
    let mut fx = press(&mut g, Key::Escape);
    fx.extend(until(&mut g, 3.0, |g| !g.settings.dirty()));
    assert!(
        fx.contains(&Effect::Store(Store::Settings)),
        "the mute was written after the watch"
    );
}

#[test]
fn recording_the_same_match_twice_keeps_one() {
    let (mut g, saved, duel) = on_won_plate();
    let record = Record::duel(TODAY, &saved, &duel, Ground::Generated).unwrap();
    g.history.file.push(record);
    // Leaving the plate would record it again: it is already listed.
    let fx = press(&mut g, Key::Enter);
    assert!(history_writes(&fx).is_empty());
    assert_eq!(g.history.file.records.len(), 1);
}

#[test]
fn a_mute_during_a_watch_survives_the_app_being_paused() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    press(&mut g, Key::M);
    assert!(g.watching() && g.save.muted);
    // A phone may kill the app once it is paused, watch or no watch.
    let fx = g.app_paused(&[]);
    assert!(
        fx.contains(&Effect::Store(Store::Settings)),
        "the mute was written as the app paused: {fx:?}"
    );
    assert!(!g.settings.dirty());
}

// ------------------------------------------------------------------ stars

/// A game on the REPLAYS list with runs of seeds 9, 11 and 13 recorded
/// (13 the newest), with keys.
fn listed_three() -> Game {
    let (mut g, _, _) = finished(9);
    for (seed, day) in [(11, TODAY + 1), (13, TODAY + 2)] {
        let (mut other, _, _) = finished(seed);
        let r = other.history.last_run.take().unwrap();
        g.history.file.push(Record { day, ..r });
    }
    press(&mut g, Key::Enter);
    press(&mut g, Key::W);
    assert_eq!(g.mode, Mode::Replays);
    g.take_effects();
    g
}

fn listed_seeds(g: &Game) -> Vec<u64> {
    g.history
        .file
        .listed()
        .map(|r| match &r.kind {
            Kind::Run(r) => r.seed,
            Kind::Duel(_) => 0,
        })
        .collect()
}

#[test]
fn the_star_key_stars_the_chosen_row_and_writes_the_file() {
    let mut g = listed_three();
    assert_eq!(listed_seeds(&g), [13, 11, 9]);
    press(&mut g, Key::Down);
    press(&mut g, Key::Down);
    let fx = press(&mut g, Key::S);
    // Starred, written at once, and listed first with the focus on it.
    let writes = history_writes(&fx);
    assert_eq!(writes.len(), 1);
    let read = crate::history::History::parse(&writes[0]);
    assert_eq!(read.stars(), 1);
    assert_eq!(listed_seeds(&g), [9, 13, 11]);
    assert_eq!(g.history.focus, 0, "the chosen row followed it");
    assert_eq!(g.status, "STARRED - THIS ONE STAYS");
    // Enter watches the starred one.
    press(&mut g, Key::Enter);
    assert_eq!(g.play.run.as_ref().unwrap().run_seed, 9);
    press(&mut g, Key::Escape);
    // S again takes the star off: it counts as just added, so it stays at
    // the top, the first of the rest, and the chosen row with it.
    let fx = press(&mut g, Key::S);
    assert_eq!(history_writes(&fx).len(), 1);
    assert_eq!(g.history.file.stars(), 0);
    assert_eq!(listed_seeds(&g), [9, 13, 11]);
    assert_eq!(g.history.focus, 0);
    assert_eq!(g.status, "STAR TAKEN OFF");
}

#[test]
fn a_sixth_star_is_refused_plainly() {
    let mut g = listed_three();
    for seed in 20..20 + crate::history::MAX_STARS as u64 {
        let (mut other, _, _) = finished(seed);
        let r = other.history.last_run.take().unwrap();
        g.history.file.push(r.clone());
        g.history.file.set_star(&r, true).unwrap();
    }
    g.history.focus = g.history.file.records.len() - 1;
    let fx = press(&mut g, Key::S);
    assert!(history_writes(&fx).is_empty());
    assert_eq!(g.status, "5 STARS - TAKE ONE OFF FIRST");
    assert_eq!(g.history.file.stars(), crate::history::MAX_STARS);
}

#[test]
fn a_touchscreen_stars_by_the_rows_own_star_and_watches_by_the_row() {
    let mut g = Game::new(true, Save::default());
    g.fit_display(WINDOW, (0, 0));
    for (seed, day) in [(9, TODAY), (11, TODAY + 1)] {
        let (mut other, _, _) = finished(seed);
        let r = other.history.last_run.take().unwrap();
        g.history.file.push(Record { day, ..r });
    }
    touch_frame(&mut g, Vec::new());
    tap(&mut g, Action::Watch);
    assert_eq!(g.mode, Mode::Replays);
    // The second row's star: starred and written, not watched.
    let fx = tap(&mut g, Action::Wb(Wb::RowStar(1)));
    assert!(!g.watching());
    assert_eq!(history_writes(&fx).len(), 1);
    assert!(g.history.file.listed().next().unwrap().starred);
    // The star button stands apart from its row.
    let rect = |a: Wb| {
        g.input
            .buttons
            .iter()
            .find(|b| b.action == Action::Wb(a))
            .unwrap()
            .rect
    };
    let (row, star) = (rect(Wb::Row(0)), rect(Wb::RowStar(0)));
    assert!(star.x - (row.x + row.w) >= 12, "{row:?} {star:?}");
    assert!(star.w >= 22 && star.h >= 13);
    // A tap on the row still watches it: the starred one, now on top.
    tap(&mut g, Action::Wb(Wb::Row(0)));
    assert!(g.watching());
    assert_eq!(g.play.run.as_ref().unwrap().run_seed, 9);
}

/// `g`, watching, played on to its end.
fn to_the_end(g: &mut Game) {
    for _ in 0..400 {
        press(g, Key::Right);
        if ended(g) {
            break;
        }
    }
    until(g, 60.0, ended);
}

#[test]
fn star_at_a_replays_end_is_written_once_the_watch_is_over() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    // Not before its end: S does nothing mid-replay.
    press(&mut g, Key::S);
    assert_eq!(g.history.file.stars(), 0);
    to_the_end(&mut g);
    let mut fx = press(&mut g, Key::S);
    let starred = g.history.last_run.clone().unwrap();
    assert!(g.history.file.is_starred(&starred), "set in memory");
    for _ in 0..60 {
        fx.extend(g.tick(FRAME, &idle()));
    }
    assert!(g.watching());
    assert!(!fx.iter().any(Effect::touches_disk), "{fx:?}");
    assert!(g.history.unsaved);
    // EXIT: written now.
    let fx = press(&mut g, Key::Escape);
    let writes = history_writes(&fx);
    assert_eq!(writes.len(), 1, "{fx:?}");
    assert!(crate::history::History::parse(&writes[0]).is_starred(&starred));
    assert!(!g.history.unsaved);
    assert_eq!(g.mode, Mode::GameOver);
}

#[test]
fn star_at_a_replays_end_survives_the_app_being_paused() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    to_the_end(&mut g);
    press(&mut g, Key::S);
    // Starred, then unstarred and starred again: one write, of the last.
    press(&mut g, Key::S);
    press(&mut g, Key::S);
    let fx = g.app_paused(&[]);
    let writes = history_writes(&fx);
    assert_eq!(writes.len(), 1, "{fx:?}");
    let read = crate::history::History::parse(&writes[0]);
    assert_eq!(read.stars(), 1);
    assert!(!g.history.unsaved);
    // Still watching; leaving writes nothing more.
    assert!(g.watching());
    g.focus_gained();
    let fx = press(&mut g, Key::Escape);
    assert!(history_writes(&fx).is_empty());
    // And on a close the same.
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    to_the_end(&mut g);
    press(&mut g, Key::S);
    let fx = g.closing(&[]);
    assert_eq!(history_writes(&fx).len(), 1);
}

#[test]
fn star_at_the_end_of_a_won_match_replay_keeps_it() {
    let (mut g, saved, duel) = on_won_plate();
    assert!(g.history.file.records.is_empty(), "not recorded until left");
    press(&mut g, Key::W);
    for _ in 0..8 {
        press(&mut g, Key::Down);
    }
    until(&mut g, 60.0, ended);
    press(&mut g, Key::S);
    let fx = press(&mut g, Key::Escape);
    assert_eq!(history_writes(&fx).len(), 1);
    let want = Record::duel(TODAY, &saved, &duel, Ground::Generated).unwrap();
    assert!(g.history.file.is_starred(&want));
    // Leaving the plate records it as ever: already there, still starred.
    let fx = press(&mut g, Key::Enter);
    assert!(history_writes(&fx).is_empty());
    assert_eq!(g.history.file.records.len(), 1);
    assert!(g.history.file.is_starred(&want));
}

#[test]
fn a_starred_run_outlasts_more_than_twenty_newer_ones() {
    let (mut g, _, _) = finished(9);
    let first = g.history.last_run.clone().unwrap();
    press(&mut g, Key::Enter);
    press(&mut g, Key::W);
    press(&mut g, Key::S);
    let mut text = String::new();
    // Twenty-five newer runs finished in the game.
    for seed in 100..125 {
        g.start_run(seed, false);
        g.tick(FRAME, &idle());
        g.play.run = Some(finished_run(seed, g.display.buffer_w as usize, 1));
        g.finish_run();
        text = history_writes(&g.take_effects()).pop().expect("recorded");
    }
    let read = crate::history::History::parse(&text);
    assert!(read.is_starred(&first));
    assert_eq!(read.records.len(), crate::history::MAX_RUNS + 1);
    assert_eq!(read.listed().next().unwrap().same_game(&first), true);
}

#[test]
fn the_list_opens_on_the_game_just_played_not_a_favourite() {
    let mut g = listed_three();
    // Seed 9, the oldest, starred: listed first.
    let nine = g.history.file.listed().nth(2).unwrap().clone();
    g.history.file.set_star(&nine, true).unwrap();
    press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Title);
    press(&mut g, Key::W);
    assert_eq!(listed_seeds(&g), [9, 13, 11]);
    assert_eq!(g.history.focus, 1, "the newest not starred: 13");
    press(&mut g, Key::Enter);
    assert_eq!(g.play.run.as_ref().unwrap().run_seed, 13);
    // All starred: the top row.
    press(&mut g, Key::Escape);
    press(&mut g, Key::Escape);
    for r in g.history.file.records.clone() {
        g.history.file.set_star(&r, true).unwrap();
    }
    press(&mut g, Key::W);
    assert_eq!(g.history.focus, 0);
}

#[test]
fn a_tap_on_a_rows_star_keeps_the_page() {
    let mut g = Game::new(true, Save::default());
    g.fit_display(WINDOW, (0, 0));
    // Eight runs: two pages of six on a phone.
    let (_, run, _) = finished(9);
    for day in 1..=8 {
        let mut r = Record::run(day, &run, false);
        if let Kind::Run(k) = &mut r.kind {
            k.seed = day;
        }
        g.history.file.push(r);
    }
    touch_frame(&mut g, Vec::new());
    tap(&mut g, Action::Watch);
    tap(&mut g, Action::Wb(Wb::PageNext));
    let rows = g.replay_rows();
    assert_eq!(g.history.focus / rows, 1, "on the second page");
    // Star the second page's first row: it moves to page one, the page
    // under the finger does not.
    let fx = tap(&mut g, Action::Wb(Wb::RowStar(0)));
    assert_eq!(history_writes(&fx).len(), 1);
    assert_eq!(g.history.file.stars(), 1);
    assert_eq!(g.history.focus / rows, 1, "still the second page");
    assert!(g.history.file.listed().next().unwrap().starred);
}

#[test]
fn a_star_in_a_newer_builds_file_says_it_lasts_this_session() {
    let mut g = game();
    let line = run_line(9);
    g.set_history(Some(&format!("history 9\n{line}")));
    assert!(g.history.file.from_the_future);
    press(&mut g, Key::W);
    let fx = press(&mut g, Key::S);
    assert!(history_writes(&fx).is_empty());
    assert_eq!(g.history.file.stars(), 1);
    assert_eq!(g.status, "STARRED UNTIL THE GAME CLOSES");
}

/// The line of a run on `seed`, finished and recorded.
fn run_line(seed: u64) -> String {
    let (g, _, _) = finished(seed);
    g.history.last_run.unwrap().to_line()
}

#[test]
fn star_at_the_end_says_why_a_run_too_long_to_keep_cannot_be_starred() {
    let (mut g, _, _) = finished(9);
    press(&mut g, Key::W);
    to_the_end(&mut g);
    // As if its line were too long to keep: never listed.
    g.history.file.records.clear();
    if let Some(w) = g.watch.as_mut() {
        if let Kind::Run(r) = &mut w.record.kind {
            while r.log.len() <= crate::history::MAX_LINE_CHARS {
                r.log.push_str(" 1:100:100");
            }
        }
    }
    press(&mut g, Key::S);
    assert_eq!(
        g.watch.as_ref().unwrap().star_refused,
        Some(crate::history::StarError::NotKept)
    );
    assert!(!g.history.unsaved);
    let fx = press(&mut g, Key::Escape);
    assert!(history_writes(&fx).is_empty());
}

#[test]
fn a_match_not_kept_on_disk_offers_no_watch_and_so_no_star() {
    // A dev `--layout` match: on screen, never kept, never recorded.
    let (mut g, saved, _) = on_won_plate();
    g.versus.live.as_mut().unwrap().kept = false;
    let _ = saved;
    press(&mut g, Key::W);
    assert!(!g.watching());
    g.watch_live_match();
    assert!(!g.watching());
    assert!(g.history.file.records.is_empty());
}

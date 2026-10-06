//! Local versus stepped as the node steps it: the title's way in, the
//! pre-match screen, a turn from handoff to handoff, and a match to its end.

use super::*;
use pretty_assertions::assert_eq;

use crate::app::versus::Stage;
use crate::input::{Event, Intents, Key};
use crate::versus::{self as rules, Side};

const FRAME: f64 = 1.0 / 60.0;
const WINDOW: (i32, i32) = (1280, 720);

fn idle() -> Intents {
    Intents {
        window: WINDOW,
        today: 20_000,
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

fn press(g: &mut Game, key: Key) -> Vec<Effect> {
    let fx = g.tick(FRAME, &keys(&[key]));
    // Let go, so the next press of the same key is a press.
    g.tick(FRAME, &idle());
    fx
}

fn game() -> Game {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g.take_effects();
    g
}

/// A game in a match, at the first handoff.
fn in_match() -> Game {
    let mut g = game();
    press(&mut g, Key::Num2);
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    g
}

fn vs(g: &Game) -> &VersusPlay {
    g.versus.live.as_ref().expect("a match on screen")
}

/// Idle frames until `done`; panics after ten seconds of them.
fn until(g: &mut Game, done: impl Fn(&Game) -> bool) -> Vec<Effect> {
    let mut fx = Vec::new();
    for _ in 0..600 {
        if done(g) {
            return fx;
        }
        fx.extend(g.tick(FRAME, &idle()));
    }
    panic!("not done after 600 frames");
}

/// Ready up, aim `shot` and fire it, then wait for the next plate.
fn take_turn(g: &mut Game, shot: Shot) {
    assert_eq!(vs(g).stage, Stage::Handoff);
    press(g, Key::Enter);
    assert_eq!(vs(g).stage, Stage::Aiming);
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    press(g, Key::Space);
    assert_eq!(vs(g).stage, Stage::Flight);
    until(g, |g| vs(g).stage != Stage::Flight);
}

/// A shot the side to move can hit the other gun with, as things stand.
fn hitting(g: &Game) -> Shot {
    let duel = &vs(g).duel;
    let side = duel.to_move;
    let frame = rules::frame_for(&duel.screen, side);
    let target = rules::target_in_frame(&duel.screen, side);
    rules::shots_at(&frame, target, &duel.loadout, 1)[0]
}

const MISS: Shot = Shot {
    angle_ticks: 1800,
    power_ticks: 0,
};

#[test]
fn the_title_opens_versus_and_the_setup_sets_aim_help() {
    let mut g = game();
    press(&mut g, Key::Num2);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert_eq!(g.versus.help, [false, false], "off by default");
    press(&mut g, Key::Right);
    assert_eq!(g.versus.help, [false, true]);
    press(&mut g, Key::Left);
    press(&mut g, Key::Right);
    assert_eq!(g.versus.help, [true, false]);
    press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Title);
    // The choice is kept for the next match this session.
    press(&mut g, Key::Num2);
    assert_eq!(g.versus.help, [true, false]);
}

#[test]
fn a_match_leaves_a_saved_run_and_the_disk_alone() {
    let mut g = game();
    g.start_run(77, false);
    g.tick(FRAME, &idle());
    g.mode = Mode::Paused;
    press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
    assert!(g.run_file.resumable.is_some());
    g.take_effects();

    let mut fx = press(&mut g, Key::Num2);
    fx.extend(press(&mut g, Key::Enter));
    for _ in 0..2 {
        let shot = hitting(&g);
        take_turn(&mut g, shot);
        fx.extend(until(&mut g, |_| true));
    }
    press(&mut g, Key::Escape);
    fx.extend(press(&mut g, Key::R));
    assert_eq!(g.mode, Mode::Title);
    assert!(g.versus.live.is_none());
    assert!(g.run_file.resumable.is_some(), "CONTINUE is still offered");
    assert!(
        !fx.iter().any(|e| matches!(
            e,
            Effect::Store(_) | Effect::RunFile(_) | Effect::CrashReport(_)
        )),
        "a match writes nothing: {fx:?}"
    );
}

#[test]
fn a_turn_passes_the_phone_and_keeps_each_players_aim() {
    let mut g = in_match();
    assert_eq!(vs(&g).stage, Stage::Handoff);
    assert_eq!(vs(&g).side(), Side::Left);
    // Nothing aims behind the handoff plate.
    let before = (g.play.angle_ticks, g.play.power_ticks);
    g.tick(FRAME, &keys(&[Key::Up]));
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), before);
    g.tick(FRAME, &idle());

    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).stage, Stage::Aiming);
    press(&mut g, Key::Up);
    press(&mut g, Key::Left);
    let left_aim = (g.play.angle_ticks, g.play.power_ticks);
    assert_eq!(left_aim, (890, 605));
    let fx = press(&mut g, Key::Space);
    assert!(fx.contains(&Effect::Play(Sound::Fire)));
    let fx = until(&mut g, |g| vs(g).stage == Stage::Handoff);
    assert!(
        fx.contains(&Effect::Play(Sound::Terrain)) || fx.contains(&Effect::Play(Sound::Structure))
    );
    assert_eq!(vs(&g).side(), Side::Right);

    // The right player starts from their own aim, not the left's.
    press(&mut g, Key::Enter);
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), (900, 600));
    press(&mut g, Key::Space);
    until(&mut g, |g| vs(g).stage == Stage::Handoff);
    press(&mut g, Key::Enter);
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), left_aim);
}

#[test]
fn a_whole_match_plays_to_a_result_and_a_rematch() {
    let mut g = in_match();
    // Left wins two rounds straight; right never hits.
    for round in 0..rules::ROUNDS_TO_WIN_MATCH {
        assert_eq!(vs(&g).duel.round, round);
        for _ in 0..rules::HITS_TO_WIN_ROUND {
            if vs(&g).side() == Side::Right {
                take_turn(&mut g, MISS);
            }
            let shot = hitting(&g);
            take_turn(&mut g, shot);
        }
        assert_eq!(vs(&g).stage, Stage::Result);
        if round + 1 < rules::ROUNDS_TO_WIN_MATCH {
            assert_eq!(vs(&g).duel.phase, rules::Phase::RoundOver(Side::Left));
            press(&mut g, Key::Enter);
            assert_eq!(vs(&g).stage, Stage::Handoff);
            assert_eq!(vs(&g).duel.hits, [0, 0]);
        }
    }
    assert_eq!(vs(&g).duel.phase, rules::Phase::MatchOver(Side::Left));
    assert_eq!(vs(&g).duel.rounds, [2, 0]);
    let seed = vs(&g).duel.seed;
    // A rematch is a new match on a new seed, the aim help as it was.
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    assert_eq!(vs(&g).duel.rounds, [0, 0]);
    assert_ne!(vs(&g).duel.seed, seed);
}

#[test]
fn the_right_players_drag_is_mirrored() {
    let mut g = in_match();
    g.input.touch_active = true;
    g.save.touch_aim = Some(crate::save_format::TouchAim::Drag);
    // Left's turn passes with a miss.
    take_turn(&mut g, MISS);
    let finger = |events| Intents { events, ..idle() };
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).side(), Side::Right);
    // Pull back down and to the right: a throw up and to the left, which
    // for the right gun is an elevation above flat.
    g.tick(
        FRAME,
        &finger(vec![
            Event::FingerDown {
                index: 0,
                x: 100.0,
                y: 60.0,
            },
            Event::FingerMoved {
                index: 0,
                x: 140.0,
                y: 100.0,
            },
        ]),
    );
    assert_eq!(g.play.angle_ticks, 900, "45 degrees, as for the left gun");
    g.tick(FRAME, &finger(vec![Event::FingerUp { index: 0 }]));
    assert_eq!(vs(&g).stage, Stage::Flight);
    let path = &vs(&g).anim.as_ref().unwrap().traces[0].path;
    assert!(path[path.len() - 1].x < path[0].x, "the shell flew left");
}

#[test]
fn back_pauses_a_match_and_focus_loss_pauses_a_turn() {
    let mut g = in_match();
    press(&mut g, Key::Enter);
    press(&mut g, Key::Escape);
    assert!(vs(&g).paused);
    // Paused, the aim does not move and the shot does not fire.
    let aim = (g.play.angle_ticks, g.play.power_ticks);
    press(&mut g, Key::Up);
    press(&mut g, Key::Space);
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), aim);
    press(&mut g, Key::Enter);
    assert!(!vs(&g).paused);

    press(&mut g, Key::Space);
    g.focus_lost(&[]);
    assert!(vs(&g).paused);
    let t = vs(&g).anim.as_ref().unwrap().t;
    g.tick(FRAME, &idle());
    assert_eq!(vs(&g).anim.as_ref().unwrap().t, t, "the flight waits");
}

#[test]
fn the_score_and_wind_wait_for_the_shell_to_land() {
    let mut g = in_match();
    press(&mut g, Key::Enter);
    let wind = vs(&g).duel.screen.wind;
    let shot = hitting(&g);
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    press(&mut g, Key::Space);
    // Already scored and the next wind drawn, but neither is shown yet.
    assert_eq!(vs(&g).duel.hits, [1, 0]);
    assert_eq!(vs(&g).shown_score().0, [0, 0]);
    assert_eq!(vs(&g).shown_wind().to_bits(), wind.to_bits());
    until(&mut g, |g| vs(g).anim.as_ref().is_some_and(|a| a.done()));
    g.tick(FRAME, &idle());
    assert_eq!(vs(&g).shown_score().0, [1, 0]);
    assert_eq!(g.status, "HIT  RIGHT GUN");
}

#[test]
fn versus_runs_at_the_play_frame_rate_only_while_a_shot_flies() {
    let mut g = in_match();
    assert!(!g.idle());
    assert!(!g.busy());
    press(&mut g, Key::Enter);
    press(&mut g, Key::Space);
    assert!(g.busy());
    g.mode = Mode::VersusSetup;
    assert!(g.idle());
}

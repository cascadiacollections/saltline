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

/// Ready up, aim `shot` and fire it, then wait for the next plate. Returns
/// the turn's effects.
fn take_turn(g: &mut Game, shot: Shot) -> Vec<Effect> {
    assert_eq!(vs(g).stage, Stage::Handoff);
    press(g, Key::Enter);
    assert_eq!(vs(g).stage, Stage::Aiming);
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    let mut fx = press(g, Key::Space);
    assert_eq!(vs(g).stage, Stage::Flight);
    fx.extend(until(g, |g| vs(g).stage != Stage::Flight));
    fx
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
    // Last session's crash report stays offered: a match is not a run.
    g.crash.last_crash = Some("report".into());
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
    assert!(g.crash.last_crash.is_some());
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
        let mut fx = Vec::new();
        for _ in 0..rules::HITS_TO_WIN_ROUND {
            assert!(!fx.contains(&Effect::Play(Sound::Win)), "won too soon");
            if vs(&g).side() == Side::Right {
                fx.extend(take_turn(&mut g, MISS));
            }
            let shot = hitting(&g);
            fx.extend(take_turn(&mut g, shot));
        }
        assert_eq!(vs(&g).stage, Stage::Result);
        let wins = fx.iter().filter(|e| **e == Effect::Play(Sound::Win));
        assert_eq!(wins.count(), 1, "one fanfare as the result shows");
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
    // A rematch goes back to the aim-help choice, still set as it was.
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert!(g.versus.live.is_none());
    assert_eq!(g.versus.help, [false, false]);
    // From there it is a new match on a new seed.
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    assert_eq!(vs(&g).duel.rounds, [0, 0]);
    assert_ne!(vs(&g).duel.seed, seed);
}

#[test]
fn a_rematch_keeps_the_last_aim_help_and_lets_it_change() {
    let mut g = game();
    press(&mut g, Key::Num2);
    press(&mut g, Key::Right);
    press(&mut g, Key::Enter);
    assert_eq!(g.versus.help, [false, true]);
    // Skip to the end of the match: the rules decide it, not the plates.
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::MatchOver(Side::Right);
    live.stage = Stage::Result;
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    assert_eq!(
        g.versus.help,
        [false, true],
        "prefilled from the last match"
    );
    press(&mut g, Key::Left);
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(g.versus.help, [true, true]);
}

#[test]
fn the_right_players_drag_is_mirrored_and_learns_nothing() {
    let mut g = in_match();
    g.input.touch_active = true;
    g.save.touch_aim = Some(crate::save_format::TouchAim::Drag);
    // Left's turn passes with a miss.
    take_turn(&mut g, MISS);
    let finger = |events| Intents { events, ..idle() };
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).side(), Side::Right);
    // Touches wait out the new screen's settle.
    for _ in 0..30 {
        g.tick(FRAME, &idle());
    }
    g.take_effects();
    // Pull back down and to the right: a throw up and to the left, which
    // for the right gun is an elevation above flat.
    let mut fx = g.tick(
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
                y: 80.0,
            },
        ]),
    );
    // atan(20 / 40) is 26.57 degrees, on the 0.5-degree grid.
    assert_eq!(g.play.angle_ticks, 530, "as the left gun would read it");
    fx.extend(g.tick(FRAME, &finger(vec![Event::FingerUp { index: 0 }])));
    assert_eq!(vs(&g).stage, Stage::Flight);
    // Aiming in versus teaches single-player nothing and writes nothing.
    assert_eq!(g.save.learned, 0);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Store(_))), "{fx:?}");
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
    // The ding waits for the landing too, and comes with it.
    loop {
        let fx = g.tick(FRAME, &idle());
        let landed = vs(&g).anim.as_ref().is_some_and(|a| a.done());
        assert_eq!(fx.contains(&Effect::Play(Sound::Hit)), landed);
        if landed {
            break;
        }
    }
    assert_eq!(vs(&g).shown_score().0, [1, 0]);
    assert_eq!(vs(&g).callout(), Some((Side::Left, true)), "LEFT hit");
    // The callout lasts until the phone is passed, then goes.
    until(&mut g, |g| vs(g).stage == Stage::Handoff);
    assert_eq!(vs(&g).callout(), None);
}

#[test]
fn left_and_right_turn_both_barrels_the_same_way_on_screen() {
    // The barrel tip's x relative to the pivot, in screen terms: the left
    // gun's tip is to the right of its pivot, the right gun's to the left.
    let tip_x = |g: &Game| {
        let a = g.play.angle_ticks as f64 * dmath::TICK_DEG * dmath::PI / 180.0;
        let x = dmath::cos(a);
        if vs(g).side() == Side::Right {
            -x
        } else {
            x
        }
    };
    let mut g = in_match();
    g.input.touch_active = true;
    for side in [Side::Left, Side::Right] {
        assert_eq!(vs(&g).side(), side);
        press(&mut g, Key::Enter);
        let before = tip_x(&g);
        press(&mut g, Key::Right);
        assert!(tip_x(&g) < before, "{side:?}: Right moved the tip left");
        // The on-screen `>` is the same action.
        let at = g.play.angle_ticks;
        g.input.touch.finger_down(
            1,
            0.0,
            0.0,
            &[crate::touch::Button {
                rect: crate::touch::Rect::new(0, 0, 4, 4),
                action: Action::Left,
            }],
        );
        g.input.touch.finger_up(1);
        g.tick(FRAME, &idle());
        let moved = g.play.angle_ticks - at;
        let want = if side == Side::Left { -10 } else { 10 };
        assert_eq!(moved, want, "{side:?}: `<`");
        press(&mut g, Key::Space);
        until(&mut g, |g| vs(g).stage == Stage::Handoff);
    }
}

#[test]
fn focus_loss_does_not_stall_the_versus_autopilot() {
    let mut g = game();
    let args: Vec<String> = ["--salt-versus", "--salt-auto"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    g.start_dev(&crate::cli::DevArgs::parse(&args), 20_000);
    until(&mut g, |g| vs(g).stage == Stage::Flight);
    g.focus_lost(&[]);
    assert!(!vs(&g).paused, "nobody is watching with focus");
    until(&mut g, |g| vs(g).duel.turn >= 2);
}

#[test]
fn each_launch_deals_a_different_first_match() {
    let first_seed = |entropy: u64| {
        let mut g = game();
        g.set_versus_entropy(entropy);
        g.start_versus();
        vs(&g).duel.seed
    };
    // Same day, two launches: different first matches.
    assert_ne!(
        first_seed(1_759_000_000_000_001),
        first_seed(1_759_000_000_000_002)
    );
    // The same launch is the same match: the seed fixes everything.
    assert_eq!(first_seed(42), first_seed(42));
    let mut g = game();
    g.set_versus_entropy(42);
    g.start_versus();
    let one = vs(&g).duel.seed;
    g.start_versus();
    assert_ne!(vs(&g).duel.seed, one, "a rematch is a new match");
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

// ------------------------------------------------------------- touch taps

/// One frame as the node runs it on a phone: the events in, the frame
/// drawn, and its buttons handed back for the next frame's touches.
fn touch_frame(g: &mut Game, events: Vec<Event>, delta: f64) {
    g.tick(delta, &Intents { events, ..idle() });
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
}

/// The centre of the touch button for `action` on screen now.
fn button_centre(g: &Game, action: Action) -> (f32, f32) {
    let b = g
        .input
        .buttons
        .iter()
        .find(|b| b.action == action)
        .unwrap_or_else(|| panic!("no {action:?} button on screen"));
    (
        b.rect.x as f32 + b.rect.w as f32 / 2.0,
        b.rect.y as f32 + b.rect.h as f32 / 2.0,
    )
}

/// A finger tapping `action`'s button: down, held about 120 ms over a few
/// frames (a phone's tap), and up, with a frame drawn after each.
fn tap(g: &mut Game, action: Action, delta: f64) {
    let (x, y) = button_centre(g, action);
    touch_frame(g, vec![Event::FingerDown { index: 0, x, y }], delta);
    let mut held = 0.0;
    while held < 0.12 {
        touch_frame(g, Vec::new(), delta);
        held += delta;
    }
    touch_frame(g, vec![Event::FingerUp { index: 0 }], delta);
    for _ in 0..30 {
        touch_frame(g, Vec::new(), delta);
    }
}

/// A tap as a noisy device may report it: the press repeated on every frame
/// it is held (a down for a finger already down, as after a lost event),
/// and a last move far from where it went down (as when one of the copies
/// arrives in other coordinates) before it lifts.
fn noisy_tap(g: &mut Game, action: Action, delta: f64) {
    let (x, y) = button_centre(g, action);
    let mut held = 0.0;
    while held < 0.12 {
        touch_frame(g, vec![Event::FingerDown { index: 0, x, y }], delta);
        held += delta;
    }
    touch_frame(
        g,
        vec![
            Event::FingerMoved {
                index: 0,
                x: x - 150.0,
                y: y + 60.0,
            },
            Event::FingerUp { index: 0 },
        ],
        delta,
    );
    for _ in 0..30 {
        touch_frame(g, Vec::new(), delta);
    }
}

fn touch_game(drag: bool) -> Game {
    let mut g = Game::new(true, Save::default());
    g.fit_display(WINDOW, (0, 0));
    if drag {
        g.save.touch_aim = Some(crate::save_format::TouchAim::Drag);
    }
    g.take_effects();
    touch_frame(&mut g, Vec::new(), FRAME);
    g
}

#[test]
fn a_tap_on_start_or_ready_does_only_that() {
    // At the play frame rate, the idle menu rate, both aim modes, and taps
    // clean and noisy.
    for delta in [FRAME, 1.0 / 10.0] {
        for (drag, noisy) in [(false, false), (true, false), (false, true), (true, true)] {
            let tap = |g: &mut Game, action| {
                if noisy {
                    noisy_tap(g, action, delta)
                } else {
                    tap(g, action, delta)
                }
            };
            let mut g = touch_game(drag);
            tap(&mut g, Action::Versus);
            assert_eq!(g.mode, Mode::VersusSetup);
            tap(&mut g, Action::Confirm);
            assert_eq!(g.mode, Mode::Versus, "drag {drag}");
            assert_eq!(vs(&g).stage, Stage::Handoff, "START went on: drag {drag}");
            assert_eq!(vs(&g).duel.turn, 0, "a shot was fired");
            tap(&mut g, Action::Confirm);
            assert_eq!(vs(&g).stage, Stage::Aiming, "drag {drag}");
            assert!(vs(&g).anim.is_none(), "READY fired a shot: drag {drag}");
            assert_eq!(vs(&g).duel.turn, 0, "READY fired a shot: drag {drag}");
        }
    }
}

#[test]
fn a_tap_on_rematch_stops_at_the_aim_help_choice() {
    // REMATCH and the pre-match START both sit mid-plate: one tap must not
    // go through both and start the next match.
    for noisy in [false, true] {
        let mut g = touch_game(false);
        tap(&mut g, Action::Versus, FRAME);
        tap(&mut g, Action::Confirm, FRAME);
        let live = g.versus.live.as_mut().unwrap();
        live.duel.phase = rules::Phase::MatchOver(Side::Left);
        live.stage = Stage::Result;
        for _ in 0..30 {
            touch_frame(&mut g, Vec::new(), FRAME);
        }
        if noisy {
            noisy_tap(&mut g, Action::Confirm, FRAME);
        } else {
            tap(&mut g, Action::Confirm, FRAME);
        }
        assert_eq!(g.mode, Mode::VersusSetup, "noisy {noisy}");
        tap(&mut g, Action::Confirm, FRAME);
        assert_eq!(g.mode, Mode::Versus, "noisy {noisy}");
        assert_eq!(vs(&g).stage, Stage::Handoff, "noisy {noisy}");
    }
}

#[test]
fn a_miss_is_called_out_for_the_shooter() {
    let mut g = in_match();
    press(&mut g, Key::Enter);
    (g.play.angle_ticks, g.play.power_ticks) = (MISS.angle_ticks, MISS.power_ticks);
    let mut fx = press(&mut g, Key::Space);
    assert_eq!(vs(&g).callout(), None, "nothing is called while it flies");
    fx.extend(until(&mut g, |g| {
        vs(g).anim.as_ref().is_some_and(|a| a.done())
    }));
    assert_eq!(vs(&g).callout(), Some((Side::Left, false)));
    assert!(!fx.contains(&Effect::Play(Sound::Hit)));
}

#[test]
fn back_from_the_rematch_choice_leaves_versus_as_title_does() {
    let mut g = in_match();
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::MatchOver(Side::Left);
    live.stage = Stage::Result;
    press(&mut g, Key::Enter);
    assert_eq!(g.mode, Mode::VersusSetup);
    g.take_effects();
    let fx = press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Title);
    assert!(g.versus.live.is_none());
    // The buffer is put back for the title, as leaving a match does.
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Buffer { .. })),
        "{fx:?}"
    );
}

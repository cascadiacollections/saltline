//! Local versus stepped as the node steps it: the title's way in, the
//! pre-match screen, a turn from handoff to handoff, and a match to its end.

use super::*;
use pretty_assertions::assert_eq;

use crate::app::versus::{Call, Stage, DING_DELAY};
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
    rules::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)[0]
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
fn rolling_guns_can_be_turned_off_for_a_match_and_its_rematches() {
    let mut g = game();
    press(&mut g, Key::Num2);
    assert!(!g.versus.rolling_off, "on by default");
    assert_eq!(g.versus.rules().gun_shift, rules::GUN_SHIFT);
    press(&mut g, Key::Up);
    assert!(g.versus.rolling_off);
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).duel.rules.gun_shift, 0.0);
    // Equal shots stays on either way.
    assert!(vs(&g).duel.rules.equal_shots);
    // A hit and its answer: the guns stay put.
    let home = vs(&g).duel.screen.muzzle_x;
    let shot = hitting(&g);
    take_turn(&mut g, shot);
    take_turn(&mut g, MISS);
    assert_eq!(vs(&g).duel.screen.muzzle_x, home);
    // A rematch keeps the choice.
    let seed = vs(&g).duel.seed;
    g.start_versus();
    assert_ne!(vs(&g).duel.seed, seed);
    assert_eq!(vs(&g).duel.rules.gun_shift, 0.0);
    // And it turns back on.
    g.open_versus_setup();
    press(&mut g, Key::Up);
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).duel.rules.gun_shift, rules::GUN_SHIFT);
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
fn rolled_guns_are_found_moved_after_the_handoff() {
    let mut g = in_match();
    let home = vs(&g).duel.screen.muzzle_x;
    let shot = hitting(&g);
    take_turn(&mut g, shot);
    // Right answers; the pair had a hit, so the rules roll the guns now.
    press(&mut g, Key::Enter);
    (g.play.angle_ticks, g.play.power_ticks) = (MISS.angle_ticks, MISS.power_ticks);
    press(&mut g, Key::Space);
    assert_ne!(vs(&g).duel.screen.muzzle_x, home, "the rules rolled them");
    // In flight and while the MISS is held, the guns are drawn where they
    // stood; the crater shows once the shell is down.
    until(&mut g, |g| vs(g).anim.as_ref().is_some_and(|a| a.done()));
    assert_eq!(vs(&g).shown_screen().muzzle_x, home);
    let scar = |s: &Screen| s.field.scar.iter().sum::<f64>();
    let held = scar(vs(&g).shown_screen());
    assert!(
        held > scar(&vs(&g).anim.as_ref().unwrap().before),
        "no crater"
    );
    // Behind the plate, they are at their new spots.
    until(&mut g, |g| vs(g).stage == Stage::Handoff);
    assert_eq!(vs(&g).shown_screen().muzzle_x, vs(&g).duel.screen.muzzle_x);
}

#[test]
fn overtime_holds_the_fanfare_until_someone_actually_wins() {
    let mut g = in_match();
    let mut fx = Vec::new();
    let no_fanfare = |fx: &[Effect]| !fx.contains(&Effect::Play(Sound::Win));
    // Both hit every time: left's third hit is answered by right's third.
    for n in 1..=rules::HITS_TO_WIN_ROUND {
        fx.extend({
            let shot = hitting(&g);
            take_turn(&mut g, shot)
        });
        assert_eq!(vs(&g).stage, Stage::Handoff, "left's hit {n} is no win");
        if n == rules::HITS_TO_WIN_ROUND {
            assert_eq!(vs(&g).duel.stakes(), Some(rules::Stakes::LastShot));
        }
        fx.extend({
            let shot = hitting(&g);
            take_turn(&mut g, shot)
        });
        assert_eq!(vs(&g).stage, Stage::Handoff);
    }
    assert_eq!(vs(&g).duel.stakes(), Some(rules::Stakes::Tied));
    assert!(no_fanfare(&fx), "no fanfare while the round is still open");
    // Left misses, right hits: right is ahead after the pair and wins.
    fx.extend(take_turn(&mut g, MISS));
    assert!(no_fanfare(&fx));
    fx.extend({
        let shot = hitting(&g);
        take_turn(&mut g, shot)
    });
    assert_eq!(vs(&g).stage, Stage::Result);
    assert_eq!(vs(&g).duel.phase, rules::Phase::RoundOver(Side::Right));
    let wins = fx
        .iter()
        .filter(|e| **e == Effect::Play(Sound::Win))
        .count();
    assert_eq!(wins, 1, "one fanfare, with the result");
}

#[test]
fn the_handoff_knows_which_way_the_guns_rolled_until_the_next_shot() {
    let mut g = in_match();
    assert_eq!(vs(&g).rolled, None);
    let home = vs(&g).duel.screen.muzzle_x;
    {
        let shot = hitting(&g);
        take_turn(&mut g, shot)
    };
    assert_eq!(vs(&g).rolled, None, "not before the pair is done");
    take_turn(&mut g, MISS);
    let way = vs(&g).rolled.expect("the pair had a hit");
    let closer = vs(&g).duel.screen.muzzle_x > home;
    assert_eq!(way == rules::RollWay::Closer, closer);
    take_turn(&mut g, MISS);
    assert_eq!(vs(&g).rolled, None, "a new shot clears it");
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
        if vs(&g).stage != Stage::Result {
            // Left opened this round, so right answers its third hit, and
            // no fanfare has played while that answer was due.
            assert!(!fx.contains(&Effect::Play(Sound::Win)), "won too soon");
            fx.extend(take_turn(&mut g, MISS));
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
    // The ding waits for the landing too, and a moment after it.
    let fx = until(&mut g, |g| vs(g).anim.as_ref().is_some_and(|a| a.done()));
    assert!(!fx.contains(&Effect::Play(Sound::Hit)), "dinged in the air");
    assert_eq!(vs(&g).shown_score().0, [1, 0]);
    assert_eq!(vs(&g).callout(), Some((Side::Left, Call::Hit)), "LEFT hit");
    let mut dinged_at = None;
    for _ in 0..60 {
        if g.tick(FRAME, &idle()).contains(&Effect::Play(Sound::Hit)) {
            dinged_at = vs(&g).anim.as_ref().map(|a| a.settled);
            break;
        }
    }
    let at = dinged_at.expect("no ding");
    assert!(
        (DING_DELAY..DING_DELAY + 2.0 * FRAME).contains(&at),
        "dinged {at} s after the impact"
    );
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

/// Fire `shot` for the side to move from its handoff, and play the turn
/// out to the next plate, returning its effects and what was called out.
fn called(g: &mut Game, shot: Shot) -> (Vec<Effect>, Option<(Side, Call)>) {
    press(g, Key::Enter);
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    let mut fx = press(g, Key::Space);
    assert_eq!(vs(g).callout(), None, "nothing is called while it flies");
    fx.extend(until(g, |g| vs(g).anim.as_ref().is_some_and(|a| a.done())));
    let call = vs(g).callout();
    fx.extend(until(g, |g| vs(g).stage != Stage::Flight));
    (fx, call)
}

#[test]
fn a_miss_is_called_out_for_the_shooter() {
    let mut g = in_match();
    // A short lob into the open ground between the guns.
    let (fx, call) = called(&mut g, Shot::new(1400, 330));
    assert_eq!(call, Some((Side::Left, Call::Miss)));
    assert!(!fx.contains(&Effect::Play(Sound::Hit)));
}

#[test]
fn a_shell_on_your_own_gun_is_an_oops_with_no_point_or_ding() {
    let mut g = in_match();
    // Straight up, and down onto the left gun itself.
    let (fx, call) = called(&mut g, MISS);
    assert_eq!(call, Some((Side::Left, Call::Oops)));
    assert!(!fx.contains(&Effect::Play(Sound::Hit)), "no ding");
    assert_eq!(vs(&g).duel.hits, [0, 0], "no pip");
    // The right gun's own-goal is an oops too.
    let (fx, call) = called(&mut g, MISS);
    assert_eq!(call, Some((Side::Right, Call::Oops)));
    assert!(!fx.contains(&Effect::Play(Sound::Hit)));
    assert_eq!(vs(&g).duel.hits, [0, 0]);
}

#[test]
fn the_right_gun_dings_when_it_hits() {
    let mut g = in_match();
    take_turn(&mut g, MISS);
    assert_eq!(vs(&g).side(), Side::Right);
    let shot = hitting(&g);
    let (fx, call) = called(&mut g, shot);
    assert_eq!(call, Some((Side::Right, Call::Hit)));
    let dings = fx.iter().filter(|e| **e == Effect::Play(Sound::Hit));
    assert_eq!(dings.count(), 1);
    assert_eq!(vs(&g).duel.hits, [0, 1]);
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

// --------------------------------------------------------- easing a roll

/// Left hits and right answers short until the guns roll: the game on the
/// handoff plate just after the roll.
fn after_a_roll() -> Game {
    let mut g = in_match();
    for _ in 0..20 {
        let shot = if vs(&g).side() == Side::Left {
            hitting(&g)
        } else {
            Shot::new(1400, 330)
        };
        take_turn(&mut g, shot);
        if vs(&g).rolled.is_some() {
            assert_eq!(vs(&g).stage, Stage::Handoff);
            return g;
        }
    }
    panic!("the guns never rolled");
}

/// Where each gun stood before the roll and stands after it.
fn spots(g: &Game) -> ([Vec2; 2], [Vec2; 2]) {
    let v = vs(g);
    let moved = v.ease.expect("a roll to ease").moved;
    let new = Side::BOTH.map(|s| rules::muzzle_of(&v.duel.screen, s));
    let old = Side::BOTH.map(|s| {
        let (n, d) = (new[s.index()], moved[s.index()]);
        Vec2::new(n.x - d.x, n.y - d.y)
    });
    (old, new)
}

#[test]
fn the_ease_runs_smoothly_from_all_to_nothing_in_its_time() {
    use crate::app::versus::{RollEase, ROLL_EASE};
    let ease = |t| RollEase {
        moved: [Vec2::new(16.0, 1.0), Vec2::new(-16.0, 1.0)],
        t,
    };
    assert_eq!(ease(None).remaining(), 1.0, "held until the field is shown");
    assert_eq!(ease(Some(0.0)).remaining(), 1.0);
    assert_eq!(ease(Some(ROLL_EASE / 2.0)).remaining(), 0.5);
    assert_eq!(ease(Some(ROLL_EASE)).remaining(), 0.0);
    assert_eq!(ease(Some(10.0)).remaining(), 0.0);
    let mut last = 1.0;
    for i in 1..=45 {
        let r = ease(Some(ROLL_EASE * i as f64 / 45.0)).remaining();
        assert!(r < last, "always moving on");
        last = r;
    }
    assert!((0.4..=0.5).contains(&ROLL_EASE));
}

/// The guns stay where they stood while the plate covers the field, roll
/// to their new spots over `ROLL_EASE` once the player is ready, and end
/// exactly where the rules put them; the rules' state never moves.
#[test]
fn rolled_guns_ease_to_their_new_spots_once_the_field_is_shown() {
    use crate::app::versus::ROLL_EASE;
    let mut g = after_a_roll();
    let (old, new) = spots(&g);
    assert!(old != new, "the guns moved");
    let rules_state = format!("{:?}", vs(&g).duel);
    for _ in 0..90 {
        g.tick(FRAME, &idle());
        for s in Side::BOTH {
            assert_eq!(vs(&g).shown_muzzle(s), old[s.index()], "held on the plate");
        }
    }
    assert!(!vs(&g).easing());
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).stage, Stage::Aiming);
    let mut frames = 2;
    let mut gap = f64::INFINITY;
    while vs(&g).ease.is_some() {
        assert!(vs(&g).easing() && g.busy(), "every frame is drawn");
        let shown = vs(&g).shown_muzzle(Side::Left);
        let to_go = (new[0].x - shown.x).abs();
        assert!(to_go < gap, "rolling on, never back");
        gap = to_go;
        let between = |a: f64, b: f64, x: f64| x >= a.min(b) && x <= a.max(b);
        for s in Side::BOTH {
            let (o, n, m) = (old[s.index()], new[s.index()], vs(&g).shown_muzzle(s));
            assert!(between(o.x, n.x, m.x) && between(o.y, n.y, m.y));
        }
        let fx = g.tick(FRAME, &idle());
        frames += 1;
        if vs(&g).ease.is_none() {
            assert!(fx.contains(&Effect::Redraw), "the last step is drawn");
        }
        assert_eq!(format!("{:?}", vs(&g).duel), rules_state);
    }
    // Timing: the ease takes ROLL_EASE of play, give or take a frame.
    let took = frames as f64 * FRAME;
    assert!(
        (ROLL_EASE - FRAME..=ROLL_EASE + FRAME).contains(&took),
        "took {took} s"
    );
    // The final position is exact, and nothing is left to draw.
    for s in Side::BOTH {
        assert_eq!(vs(&g).shown_muzzle(s), new[s.index()]);
        assert_eq!(vs(&g).gun_offset(s), Vec2::new(0.0, 0.0));
    }
    assert!(!g.busy());
    assert_eq!(format!("{:?}", vs(&g).duel), rules_state);
}

#[test]
fn the_versus_pause_holds_the_ease() {
    let mut g = after_a_roll();
    press(&mut g, Key::Enter);
    let t = vs(&g).ease.unwrap().t;
    press(&mut g, Key::Escape);
    assert!(vs(&g).paused);
    for _ in 0..60 {
        g.tick(FRAME, &idle());
    }
    assert_eq!(vs(&g).ease.unwrap().t, t);
    press(&mut g, Key::Escape);
    until(&mut g, |g| vs(g).ease.is_none());
}

/// A shot fired before the guns have rolled home is the same shot as one
/// fired after: the ease is drawing only.
#[test]
fn a_shot_fired_mid_ease_flies_from_the_gun_where_it_stands() {
    let mut g = after_a_roll();
    press(&mut g, Key::Enter);
    assert!(vs(&g).easing());
    let shot = hitting(&g);
    let mut twin = vs(&g).duel.clone();
    let want = twin.fire(shot, true).unwrap();
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    press(&mut g, Key::Space);
    assert_eq!(vs(&g).stage, Stage::Flight);
    let anim = vs(&g).anim.as_ref().unwrap();
    assert_eq!(
        format!("{:?}", anim.traces[0].path),
        format!("{:?}", want.traces[0].path)
    );
    assert_eq!(format!("{:?}", vs(&g).duel), format!("{twin:?}"));
    for s in Side::BOTH {
        assert_eq!(
            vs(&g).gun_offset(s),
            Vec2::new(0.0, 0.0),
            "no ease in flight"
        );
    }
}

fn draw(g: &Game) -> crate::raster::Canvas {
    let mut canvas = crate::raster::Canvas::new(g.display.buffer_w, g.display.buffer_h);
    crate::render::frame(
        g,
        &crate::render::Art::default(),
        &mut canvas,
        &mut Vec::new(),
        &mut crate::render::Cache::default(),
    );
    canvas
}

/// Drawn: the gun tags start at the old spots and end at the new, the
/// last frame is the frame of a match with no roll to ease, and aim help
/// appears only once the guns are home.
#[test]
fn the_ease_is_drawn_and_aim_help_waits_for_it() {
    let mut g = after_a_roll();
    g.versus.help = [true, true];
    press(&mut g, Key::Enter);
    let mid = draw(&g);
    // The same frame without the roll to ease: the guns at the new spots.
    let ease = g.versus.live.as_mut().unwrap().ease.take();
    let home = draw(&g);
    g.versus.live.as_mut().unwrap().ease = ease;
    assert_ne!(mid.bytes(), home.bytes(), "mid-ease is not home");
    // Aim help is not drawn mid-ease.
    g.versus.help = [false, false];
    assert_eq!(draw(&g).bytes(), mid.bytes(), "no aim help while rolling");
    g.versus.help = [true, true];
    until(&mut g, |g| vs(g).ease.is_none());
    assert_eq!(draw(&g).bytes(), home.bytes(), "home, exactly");
    g.versus.help = [false, false];
    assert_ne!(draw(&g).bytes(), home.bytes(), "aim help, once home");
}

// ------------------------------------------------------------- the AI

use crate::app::versus::{AI_AIM, AI_HANDOFF, AI_HANDOFF_CUED, AUTO_READY, AUTO_READY_CUED};
use crate::opponent::Level;

/// A match against the AI at `level`, chosen on the pre-match screen with
/// keys, at the player's first handoff.
fn ai_match(level: Level) -> Game {
    let mut g = game();
    press(&mut g, Key::Num2);
    press(&mut g, Key::R);
    while g.versus.level != level {
        press(&mut g, Key::Right);
    }
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).ai, Some(level));
    g
}

/// Idle frames for `seconds`.
fn wait(g: &mut Game, seconds: f64) {
    for _ in 0..(seconds / FRAME).round() as usize {
        g.tick(FRAME, &idle());
    }
}

#[test]
fn the_right_row_steps_the_ai_level_and_keeps_the_right_aim_help() {
    let mut g = game();
    press(&mut g, Key::Num2);
    press(&mut g, Key::Right);
    assert_eq!(g.versus.help, [false, true]);
    press(&mut g, Key::R);
    assert!(g.versus.ai);
    assert_eq!(g.versus.level, Level::Easy, "EASY by default");
    for want in [Level::Medium, Level::Hard, Level::Easy] {
        press(&mut g, Key::Right);
        assert_eq!(g.versus.level, want);
    }
    assert_eq!(g.versus.help, [true, true], "the human's choice keeps");
    press(&mut g, Key::Left);
    assert_eq!(g.versus.help, [false, true], "the player's own aim help");
    press(&mut g, Key::R);
    press(&mut g, Key::Right);
    assert_eq!(g.versus.help, [false, false], "a human again");
}

#[test]
fn the_players_aim_help_is_remembered_for_each_opponent() {
    use crate::app::versus::PlayerHelp;
    let mut g = game();
    press(&mut g, Key::Num2);
    assert_eq!(g.versus.help, [false, false], "off against a human");
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [true, false], "on against the AI");
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [false, false], "off again for a human");
    // The right player's choice is theirs alone, whatever the opponent.
    press(&mut g, Key::Right);
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [true, true]);
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [false, true]);
    assert_eq!(g.versus.player_help, PlayerHelp::default());

    // The row sets the help for the opponent chosen only; switching shows
    // the other opponent's, and switching back finds each as it was set.
    let mut g = game();
    press(&mut g, Key::Num2);
    press(&mut g, Key::R);
    press(&mut g, Key::Left);
    assert_eq!(g.versus.help[0], false, "set OFF against the AI");
    press(&mut g, Key::R);
    assert_eq!(g.versus.help[0], false, "a human's default");
    press(&mut g, Key::Left);
    assert_eq!(g.versus.help[0], true, "set ON against a human");
    for (ai, want) in [(true, false), (false, true), (true, false)] {
        press(&mut g, Key::R);
        assert_eq!(g.versus.ai, ai);
        assert_eq!(g.versus.help[0], want, "ai {ai}");
    }
    assert_eq!(
        g.versus.player_help,
        PlayerHelp {
            human: true,
            ai: false
        }
    );
    // Kept for the session, through a match and back to the pre-match
    // screen from the title.
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).ai, Some(Level::Easy));
    assert_eq!(g.versus.help, [false, false]);
    press(&mut g, Key::Escape);
    press(&mut g, Key::R);
    press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
    press(&mut g, Key::Num2);
    assert_eq!(g.versus.help, [false, false]);
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [true, false]);
}

#[test]
fn an_ai_match_starts_with_aim_help_and_keeps_it_on_disk() {
    let mut g = ai_match(Level::Easy);
    assert_eq!(g.versus.help, [true, false]);
    press(&mut g, Key::Enter);
    let mut fx = Vec::new();
    (g.play.angle_ticks, g.play.power_ticks) = (MISS.angle_ticks, MISS.power_ticks);
    fx.extend(press(&mut g, Key::Space));
    fx.extend(until(&mut g, |g| vs(g).stage != Stage::Flight));
    let line = fx
        .iter()
        .rev()
        .find_map(|e| match e {
            Effect::MatchFile(MatchFile::Write(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    // Format 3 as before: the help is the field it always was.
    assert!(line.starts_with("match 3 "), "{line}");
    let saved = crate::match_log::SavedMatch::parse(&line).unwrap();
    assert_eq!(saved.help, [true, false]);

    // Resumed with the help it was saved with, which is then the
    // player's help against the AI: a switch to a human shows theirs, and
    // back to the AI finds the match's again.
    let mut resumed = game();
    let mut off = saved.clone();
    off.help = [false, false];
    resumed.saved_match = Some(off);
    resumed.resume_match();
    assert_eq!(resumed.versus.help, [false, false]);
    assert!(!resumed.versus.player_help.ai);
    resumed.open_versus_setup();
    press(&mut resumed, Key::R);
    assert!(!resumed.versus.ai);
    press(&mut resumed, Key::Left);
    assert_eq!(resumed.versus.help, [true, false], "a human's, set ON");
    press(&mut resumed, Key::R);
    assert!(resumed.versus.ai);
    assert_eq!(resumed.versus.help, [false, false], "the match's");

    // A human match resumed with the player's help on stores it as theirs
    // against a human, and leaves the AI's default alone.
    let mut resumed = game();
    let mut human = saved.clone();
    human.ai = None;
    human.help = [true, false];
    resumed.saved_match = Some(human);
    resumed.resume_match();
    assert!(!resumed.versus.ai);
    assert_eq!(resumed.versus.help, [true, false]);
    assert_eq!(
        resumed.versus.player_help,
        crate::app::versus::PlayerHelp {
            human: true,
            ai: true
        }
    );
}

#[test]
fn the_demo_leaves_the_players_aim_help_alone() {
    let mut g = game();
    press(&mut g, Key::Num2);
    // Set ON against a human, then the AI chosen.
    press(&mut g, Key::Left);
    press(&mut g, Key::R);
    let player_help = g.versus.player_help;
    press(&mut g, Key::Escape);
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(g.versus.help, [true, false]);
    g.start_demo(crate::app::demo::Started::Menu);
    g.begin_demo_leg(1);
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(vs(&g).ai, None, "the demo's duel is two guns of its own");
    assert_eq!(
        g.versus.help,
        [true, true],
        "the demo shows both sides' help"
    );
    wait(&mut g, 2.0);
    g.stop_demo();
    assert_eq!(g.versus.help, [true, false]);
    assert!(g.versus.ai);
    assert_eq!(
        g.versus.player_help, player_help,
        "the demo set nothing for the player"
    );
    press(&mut g, Key::Num2);
    press(&mut g, Key::R);
    assert_eq!(g.versus.help, [true, false], "the human's, as set");
}

// ------------------------------------------------- pause on a result plate

/// A touch game in a match (against the AI if `ai`) put on the result
/// plate of `phase`, settled.
fn touch_result(ai: bool, phase: rules::Phase) -> Game {
    let mut g = touch_game(false);
    tap(&mut g, Action::Versus, FRAME);
    if ai {
        tap(&mut g, Action::Replay, FRAME);
    }
    tap(&mut g, Action::Confirm, FRAME);
    assert_eq!(vs(&g).ai.is_some(), ai);
    let live = g.versus.live.as_mut().unwrap();
    live.kept = true;
    live.duel.phase = phase;
    live.stage = Stage::Result;
    for _ in 0..30 {
        touch_frame(&mut g, Vec::new(), FRAME);
    }
    g.take_effects();
    g
}

#[test]
fn pause_on_a_result_plate_resumes_to_it_and_moves_nothing_on() {
    for phase in [
        rules::Phase::RoundOver(Side::Left),
        rules::Phase::MatchOver(Side::Right),
    ] {
        for ai in [false, true] {
            for noisy in [false, true] {
                let at = format!("{phase:?} ai {ai} noisy {noisy}");
                let tap = |g: &mut Game, action| {
                    if noisy {
                        noisy_tap(g, action, FRAME)
                    } else {
                        tap(g, action, FRAME)
                    }
                };
                let mut g = touch_result(ai, phase);
                let (round, log) = (vs(&g).duel.round, vs(&g).log.len());
                tap(&mut g, Action::Back);
                assert!(vs(&g).paused, "{at}: II pauses");
                // RESUME sits where NEXT ROUND or REMATCH does: one tap,
                // back on the plate, nothing advanced.
                tap(&mut g, Action::Confirm);
                assert_eq!(g.mode, Mode::Versus, "{at}");
                assert!(!vs(&g).paused, "{at}");
                assert_eq!(vs(&g).stage, Stage::Result, "{at}");
                assert_eq!(vs(&g).duel.phase, phase, "{at}");
                assert_eq!(vs(&g).duel.round, round, "{at}");
                assert_eq!(vs(&g).log.len(), log, "{at}: no round dealt");
                // The plate still works as it did.
                tap(&mut g, Action::Confirm);
                match phase {
                    rules::Phase::RoundOver(_) => {
                        assert_eq!(vs(&g).stage, Stage::Handoff, "{at}");
                        assert_eq!(vs(&g).duel.round, round + 1, "{at}");
                    }
                    _ => assert_eq!(g.mode, Mode::VersusSetup, "{at}"),
                }
            }
        }
    }
}

#[test]
fn pause_on_a_result_plate_leaves_by_title() {
    for ai in [false, true] {
        let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
        assert_eq!(
            g.input.buttons.iter().any(|b| b.action == Action::Replay),
            !ai,
            "TITLE on a round's plate only between two players"
        );
        tap(&mut g, Action::Back, FRAME);
        assert!(vs(&g).paused);
        // The match is still to be won: TITLE asks twice.
        tap(&mut g, Action::Replay, FRAME);
        assert!(vs(&g).quit_armed.is_some(), "ai {ai}");
        tap(&mut g, Action::Replay, FRAME);
        assert_eq!(g.mode, Mode::Title, "ai {ai}");
        assert!(g.versus.live.is_none());
    }
    // A won match's plate, paused, quits on the first TITLE.
    let mut g = touch_result(true, rules::Phase::MatchOver(Side::Left));
    tap(&mut g, Action::Back, FRAME);
    tap(&mut g, Action::Replay, FRAME);
    assert_eq!(g.mode, Mode::Title);
    // With keys, the pause key on the plate does the same.
    let mut g = ai_match(Level::Easy);
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::RoundOver(Side::Left);
    live.stage = Stage::Result;
    press(&mut g, Key::Escape);
    assert!(vs(&g).paused);
    press(&mut g, Key::Escape);
    assert!(!vs(&g).paused);
    assert_eq!(vs(&g).stage, Stage::Result);
    assert_eq!(vs(&g).duel.round, 0);
    let fx = press(&mut g, Key::Escape);
    assert!(!fx.contains(&Effect::MatchFile(MatchFile::Discard)));
    let fx = press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Versus, "one press only arms TITLE");
    assert!(!fx.contains(&Effect::MatchFile(MatchFile::Discard)));
    let fx = press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
    assert!(
        fx.contains(&Effect::MatchFile(MatchFile::Discard)),
        "quit, so the match is cleared"
    );
}

#[test]
fn title_on_the_pause_plate_asks_again_before_quitting_a_match() {
    use crate::app::versus::QUIT_CONFIRM;
    let armed = |g: &Game| vs(g).quit_armed.is_some();
    for ai in [false, true] {
        // One tap, clean or noisy (repeated downs, a stray move), can only
        // arm TITLE: it is spent once the plate changes.
        for noisy in [false, true] {
            let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
            tap(&mut g, Action::Back, FRAME);
            if noisy {
                noisy_tap(&mut g, Action::Replay, FRAME);
            } else {
                tap(&mut g, Action::Replay, FRAME);
            }
            assert_eq!(g.mode, Mode::Versus, "ai {ai} noisy {noisy}");
            assert!(armed(&g), "ai {ai} noisy {noisy}");
        }
        // A finger held on TITLE, or pressed again as the plate changes,
        // is spent: the second press has to be a new tap.
        let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
        tap(&mut g, Action::Back, FRAME);
        let (x, y) = button_centre(&g, Action::Replay);
        touch_frame(&mut g, vec![Event::FingerDown { index: 0, x, y }], FRAME);
        assert!(armed(&g));
        for _ in 0..10 {
            touch_frame(&mut g, vec![Event::FingerDown { index: 0, x, y }], FRAME);
        }
        touch_frame(&mut g, vec![Event::FingerUp { index: 0 }], FRAME);
        touch_frame(&mut g, vec![Event::FingerDown { index: 1, x, y }], FRAME);
        touch_frame(&mut g, vec![Event::FingerUp { index: 1 }], FRAME);
        assert_eq!(g.mode, Mode::Versus, "ai {ai}: a bounce quit");
        assert!(armed(&g));
        for _ in 0..30 {
            touch_frame(&mut g, Vec::new(), FRAME);
        }
        tap(&mut g, Action::Replay, FRAME);
        assert_eq!(g.mode, Mode::Title, "ai {ai}: the second tap quits");

        // RESUME disarms: pausing again asks twice again.
        let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
        tap(&mut g, Action::Back, FRAME);
        tap(&mut g, Action::Replay, FRAME);
        tap(&mut g, Action::Confirm, FRAME);
        assert!(!vs(&g).paused);
        assert!(!armed(&g));
        tap(&mut g, Action::Back, FRAME);
        tap(&mut g, Action::Replay, FRAME);
        assert_eq!(g.mode, Mode::Versus, "ai {ai}: re-armed, not quit");
        assert!(armed(&g));

        // Waiting disarms.
        let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
        tap(&mut g, Action::Back, FRAME);
        tap(&mut g, Action::Replay, FRAME);
        let mut waited = 0.0;
        while waited < QUIT_CONFIRM + 0.2 {
            touch_frame(&mut g, Vec::new(), FRAME);
            waited += FRAME;
        }
        assert!(vs(&g).paused);
        assert!(!armed(&g), "ai {ai}: the timeout disarms");
        tap(&mut g, Action::Replay, FRAME);
        assert_eq!(g.mode, Mode::Versus, "ai {ai}: asks again");

        // Focus going disarms.
        let mut g = touch_result(ai, rules::Phase::RoundOver(Side::Left));
        tap(&mut g, Action::Back, FRAME);
        tap(&mut g, Action::Replay, FRAME);
        g.focus_lost(&[]);
        g.focus_gained();
        assert!(vs(&g).paused);
        assert!(!armed(&g), "ai {ai}: focus loss disarms");
        for _ in 0..30 {
            touch_frame(&mut g, Vec::new(), FRAME);
        }
        tap(&mut g, Action::Replay, FRAME);
        assert_eq!(g.mode, Mode::Versus, "ai {ai}: asks again");
    }

    // The armed row says what the next press does, and the keyed plate
    // too; the pause key disarms with keys as RESUME does.
    let mut g = in_match();
    press(&mut g, Key::Escape);
    press(&mut g, Key::R);
    assert!(armed(&g));
    let armed_plate = draw(&g).bytes().to_vec();
    press(&mut g, Key::Escape);
    assert!(!vs(&g).paused);
    assert!(!armed(&g));
    press(&mut g, Key::Escape);
    assert!(
        draw(&g).bytes() != armed_plate.as_slice(),
        "the armed row is drawn differently"
    );
    press(&mut g, Key::R);
    let fx = press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
    assert!(fx.contains(&Effect::MatchFile(MatchFile::Discard)));
}

#[test]
fn ai_opponent_takes_only_right_turns_and_resumes_as_ai() {
    let mut g = ai_match(Level::Medium);
    wait(&mut g, AUTO_READY - 0.1);
    assert_eq!(vs(&g).stage, Stage::Handoff, "the plate stays a moment");
    wait(&mut g, 0.2);
    assert_eq!(vs(&g).stage, Stage::Aiming, "and readies itself");
    assert_eq!(vs(&g).duel.turn, 0, "the player must still fire");
    (g.play.angle_ticks, g.play.power_ticks) = (MISS.angle_ticks, MISS.power_ticks);
    press(&mut g, Key::Space);
    until(&mut g, |g| vs(g).stage == Stage::Handoff);
    assert_eq!(vs(&g).side(), Side::Right);
    // Player aim/fire input cannot drive the computer's turn.
    press(&mut g, Key::Space);
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).duel.turn, 1);
    press(&mut g, Key::Escape);
    wait(&mut g, 3.0);
    assert_eq!(vs(&g).duel.turn, 1, "nothing moves under the pause plate");
    press(&mut g, Key::Escape);
    let effects = until(&mut g, |g| vs(g).duel.turn == 2);
    let line = effects
        .iter()
        .find_map(|e| match e {
            Effect::MatchFile(MatchFile::Write(s)) => Some(s),
            _ => None,
        })
        .expect("AI shots are persisted");
    assert!(line.starts_with("match 3 "), "{line}");
    let saved = crate::match_log::SavedMatch::parse(line).unwrap();
    assert_eq!(saved.ai, Some(Level::Medium));
    let mut resumed = game();
    resumed.saved_match = Some(saved);
    resumed.resume_match();
    assert_eq!(vs(&resumed).ai, Some(Level::Medium));
    assert!(resumed.versus.ai);
    assert_eq!(resumed.versus.level, Level::Medium);
    assert_eq!(vs(&resumed).duel.turn, 2);
    until(&mut g, |g| vs(g).stage == Stage::Handoff);
    assert_eq!(vs(&g).side(), Side::Left);
}

#[test]
fn a_human_match_still_writes_its_old_formats() {
    let mut g = in_match();
    let fx = take_turn(&mut g, MISS);
    let line = fx
        .iter()
        .find_map(|e| match e {
            Effect::MatchFile(MatchFile::Write(s)) => Some(s),
            _ => None,
        })
        .unwrap();
    assert!(line.starts_with("match 1 "), "{line}");
}

#[test]
fn ai_readies_itself_when_opening_the_next_round() {
    let mut g = in_match();
    g.versus.live.as_mut().unwrap().ai = Some(Level::Easy);
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::RoundOver(Side::Left);
    live.stage = Stage::Result;
    g.after_versus_result();
    assert_eq!(vs(&g).side(), Side::Right);
    until(&mut g, |g| vs(g).stage == Stage::Flight);
    assert_eq!(vs(&g).log.len(), 2, "next round and AI shot");
}

#[test]
fn ai_on_custom_ground_survives_suspend_and_resumes_its_turn() {
    let mut g = game();
    g.open_versus_setup();
    g.versus.ai = true;
    g.versus.level = Level::Hard;
    g.step_ground(1);
    g.start_versus();
    take_turn(&mut g, Shot::new(900, 600));
    assert_eq!(vs(&g).side(), Side::Right);
    let live = g.versus.live.take().unwrap();
    g.suspend_live_match(live);
    let saved = g.saved_match.as_ref().unwrap();
    assert_eq!(saved.ai, Some(Level::Hard));
    assert!(saved.ground.is_some());
    let line = saved.to_line();
    g.saved_match = Some(crate::match_log::SavedMatch::parse(&line).unwrap());
    g.resume_match();
    g.auto_pause_versus();
    assert!(vs(&g).paused, "AI handoff must pause when focus is lost");
    press(&mut g, Key::Escape);
    until(&mut g, |g| vs(g).stage == Stage::Flight);
    assert_eq!(vs(&g).log.len(), 2);
    assert!(vs(&g).layout.is_some());
}

/// The AI's plate holds [`AI_HANDOFF`], or [`AI_HANDOFF_CUED`] with a cue
/// to read; its aim swings and fires at [`AI_AIM`].
#[test]
fn the_ai_plate_holds_longer_with_a_cue() {
    let mut g = ai_match(Level::Easy);
    press(&mut g, Key::Enter);
    take_turn_from_aiming(&mut g, MISS);
    assert!(vs(&g).ai_turn() && !vs(&g).handoff_cued());
    wait(&mut g, AI_HANDOFF - 0.1);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    until(&mut g, |g| vs(g).stage == Stage::Aiming);
    wait(&mut g, AI_AIM - 0.1);
    assert_eq!(vs(&g).stage, Stage::Aiming, "the aim is shown");
    wait(&mut g, 0.2);
    assert_eq!(vs(&g).stage, Stage::Flight);

    // A last shot to tie: the opener 3-2 up, the AI to answer.
    let mut g = ai_match(Level::Easy);
    last_shot_for_the_ai(&mut g);
    assert!(vs(&g).handoff_cued());
    wait(&mut g, AI_HANDOFF + 0.1);
    assert_eq!(vs(&g).stage, Stage::Handoff, "the cue is read out");
    wait(&mut g, AI_HANDOFF_CUED - AI_HANDOFF);
    assert_eq!(vs(&g).stage, Stage::Aiming);
}

/// The player's plate readies itself after [`AUTO_READY`], or
/// [`AUTO_READY_CUED`] (the AI's own cued hold) when it has a cue to read.
#[test]
fn the_players_plate_holds_longer_with_a_cue() {
    const { assert!(AUTO_READY_CUED >= 2.0 && AUTO_READY_CUED > AUTO_READY) };
    assert_eq!(AUTO_READY_CUED, AI_HANDOFF_CUED);
    // Level past the mark, the player (who opens round 1) to move: TIED.
    let mut g = ai_match(Level::Easy);
    let live = g.versus.live.as_mut().unwrap();
    let n = rules::HITS_TO_WIN_ROUND;
    live.duel.hits = [n, n];
    live.duel.turn = 2 * n;
    assert_eq!(live.duel.stakes(), Some(rules::Stakes::Tied));
    assert!(!vs(&g).ai_turn() && vs(&g).handoff_cued());
    wait(&mut g, AUTO_READY + 0.1);
    assert_eq!(vs(&g).stage, Stage::Handoff, "the cue is read out");
    wait(&mut g, AUTO_READY_CUED - AUTO_READY - 0.2);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    wait(&mut g, 0.2);
    assert_eq!(vs(&g).stage, Stage::Aiming);
}

/// Aim `shot` and fire it from the aiming stage, then wait for the next
/// plate.
fn take_turn_from_aiming(g: &mut Game, shot: Shot) {
    assert_eq!(vs(g).stage, Stage::Aiming);
    (g.play.angle_ticks, g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
    press(g, Key::Space);
    until(g, |g| vs(g).stage != Stage::Flight);
}

/// Hits until the left gun, which opens, is 3-2 up: the AI's last shot.
fn last_shot_for_the_ai(g: &mut Game) {
    let live = g.versus.live.as_mut().unwrap();
    for _ in 0..5 {
        let duel = &live.duel;
        let side = duel.to_move;
        let frame = rules::frame_for(&duel.screen, side);
        let target = rules::target_in_frame(&duel.screen, side);
        let shot = rules::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)[0];
        let report = live
            .fire_turn((shot.angle_ticks, shot.power_ticks))
            .unwrap();
        assert!(report.hit);
    }
    assert_eq!(live.duel.hits, [3, 2]);
    assert_eq!(live.duel.stakes(), Some(rules::Stakes::LastShot));
    assert_eq!(live.side(), Side::Right);
}

#[test]
fn the_ai_takes_its_last_shot_under_equal_shots() {
    for level in Level::ALL {
        let mut g = ai_match(level);
        last_shot_for_the_ai(&mut g);
        until(&mut g, |g| vs(g).stage == Stage::Flight);
        assert_eq!(vs(&g).log.len(), 6, "{level:?}: the AI's answer");
        let duel = &vs(&g).duel;
        // Either it tied, and the round plays on, or the left gun took it.
        match duel.phase {
            rules::Phase::Aiming => assert_eq!(duel.hits, [3, 3]),
            rules::Phase::RoundOver(winner) => assert_eq!(winner, Side::Left),
            rules::Phase::MatchOver(_) => panic!("one round cannot end the match"),
        }
    }
}

#[test]
fn auto_ready_and_the_ai_wait_under_pause_and_out_of_focus() {
    // The player's plate.
    let mut g = ai_match(Level::Easy);
    wait(&mut g, 1.0);
    g.focus_lost(&[]);
    assert!(vs(&g).paused, "the plate moves on by itself, so it pauses");
    g.focus_gained();
    wait(&mut g, 5.0);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    press(&mut g, Key::Escape);
    assert!(!vs(&g).paused);
    // The wait starts afresh for whoever came back to it.
    wait(&mut g, AUTO_READY - 0.2);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    wait(&mut g, 0.3);
    assert_eq!(vs(&g).stage, Stage::Aiming);

    // Out of focus but not paused (a platform that sends no pause): still
    // nothing moves, the player's plate or the AI's.
    for ai_turn in [false, true] {
        let mut g = ai_match(Level::Easy);
        if ai_turn {
            press(&mut g, Key::Enter);
            take_turn_from_aiming(&mut g, MISS);
        }
        g.unfocused = true;
        wait(&mut g, 5.0);
        assert_eq!(vs(&g).stage, Stage::Handoff, "ai turn {ai_turn}");
        g.unfocused = false;
        until(&mut g, |g| vs(g).stage == Stage::Aiming);
    }
}

#[test]
fn ready_skips_the_auto_ready() {
    let mut g = ai_match(Level::Easy);
    press(&mut g, Key::Enter);
    assert_eq!(vs(&g).stage, Stage::Aiming);
}

#[test]
fn a_finger_held_through_the_auto_ready_fires_nothing() {
    let mut g = touch_game(true);
    tap(&mut g, Action::Versus, FRAME);
    tap(&mut g, Action::Replay, FRAME);
    assert!(g.versus.ai);
    tap(&mut g, Action::Confirm, FRAME);
    assert_eq!(vs(&g).stage, Stage::Handoff);
    // A finger down on the open field, held while the plate readies.
    let (w, h) = (g.display.buffer_w as f32, g.display.buffer_h as f32);
    touch_frame(
        &mut g,
        vec![Event::FingerDown {
            index: 0,
            x: w * 0.3,
            y: h * 0.4,
        }],
        FRAME,
    );
    while vs(&g).stage == Stage::Handoff {
        touch_frame(&mut g, Vec::new(), FRAME);
    }
    // Pulled back as a drag would be, and let go.
    touch_frame(
        &mut g,
        vec![
            Event::FingerMoved {
                index: 0,
                x: w * 0.1,
                y: h * 0.7,
            },
            Event::FingerUp { index: 0 },
        ],
        FRAME,
    );
    for _ in 0..30 {
        touch_frame(&mut g, Vec::new(), FRAME);
    }
    assert_eq!(vs(&g).stage, Stage::Aiming);
    assert_eq!(vs(&g).duel.turn, 0, "the held finger fired nothing");
}

#[test]
fn the_ai_moving_on_does_not_eat_a_tap_on_pause() {
    for into in [Stage::Aiming, Stage::Flight] {
        let mut g = touch_game(false);
        tap(&mut g, Action::Versus, FRAME);
        tap(&mut g, Action::Replay, FRAME);
        tap(&mut g, Action::Confirm, FRAME);
        tap(&mut g, Action::Confirm, FRAME);
        assert_eq!(vs(&g).stage, Stage::Aiming);
        (g.play.angle_ticks, g.play.power_ticks) = (MISS.angle_ticks, MISS.power_ticks);
        tap(&mut g, Action::Fire, FRAME);
        while !(vs(&g).ai_turn() && vs(&g).stage == Stage::Handoff) {
            touch_frame(&mut g, Vec::new(), FRAME);
        }
        let from = if into == Stage::Aiming {
            Stage::Handoff
        } else {
            Stage::Aiming
        };
        while vs(&g).stage == from || vs(&g).stage != into {
            touch_frame(&mut g, Vec::new(), FRAME);
        }
        // The very next frame: II, where it was before the AI moved on.
        tap(&mut g, Action::Back, FRAME);
        assert!(vs(&g).paused, "into {into:?}");
    }
}

#[test]
fn the_round_plate_against_the_ai_does_not_quit_on_title() {
    let mut g = ai_match(Level::Easy);
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::RoundOver(Side::Left);
    live.stage = Stage::Result;
    press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Versus, "a mashed TITLE keeps the match");
    assert_eq!(vs(&g).stage, Stage::Result);
    // The match won, TITLE leaves: there is nothing left to lose.
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::MatchOver(Side::Left);
    press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
    // Between two players TITLE is where it was.
    let mut g = in_match();
    let live = g.versus.live.as_mut().unwrap();
    live.duel.phase = rules::Phase::RoundOver(Side::Left);
    live.stage = Stage::Result;
    press(&mut g, Key::R);
    assert_eq!(g.mode, Mode::Title);
}

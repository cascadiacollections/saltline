//! The core stepped as the node steps it, with no engine: intents in,
//! effects out.

use super::*;
use pretty_assertions::assert_eq;
use salt_line_sim::run::Phase;

use crate::input::{Event, Intents, Key, Pad};
use crate::resume::Saved;

/// A 60 Hz frame.
const FRAME: f64 = 1.0 / 60.0;
/// A 16:9 window, which makes a 320-wide buffer.
const WINDOW: (i32, i32) = (1280, 720);
const TODAY: u64 = 20_000;

fn idle() -> Intents {
    Intents {
        window: WINDOW,
        today: TODAY,
        ..Intents::default()
    }
}

#[test]
fn a_running_trial_uses_the_gameplay_frame_rate_and_a_paused_trial_idles() {
    let mut g = game();
    g.start_trial(1);

    assert!(!g.idle());

    g.trial.as_mut().unwrap().paused = true;
    assert!(g.idle());
}

/// Keys that went down this frame and are still held.
fn keys(keys: &[Key]) -> Intents {
    Intents {
        events: keys.iter().map(|k| Event::key(*k)).collect(),
        keys_held: keys.to_vec(),
        ..idle()
    }
}

/// A key typed in seed entry, on a key the game does not otherwise read.
fn typing(typed: Typed) -> Intents {
    Intents {
        events: vec![Event::Key {
            key: None,
            typed: Some(typed),
        }],
        ..idle()
    }
}

/// Keys held from an earlier frame, with no new press.
fn held(keys: &[Key]) -> Intents {
    Intents {
        keys_held: keys.to_vec(),
        ..idle()
    }
}

fn pads(buttons: &[Pad]) -> Intents {
    Intents {
        pads: buttons.to_vec(),
        pads_held: buttons.to_vec(),
        ..idle()
    }
}

/// A game at the title, sized for `WINDOW`, with its launch effects taken.
fn game() -> Game {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g
}

/// A game in a fresh run on `seed`, a frame in, so the run file is written.
fn playing(seed: u64) -> Game {
    let mut g = game();
    g.start_run(seed, false);
    g.take_effects();
    g.tick(FRAME, &idle());
    g
}

#[test]
fn night_sky_redraws_sparingly_and_freezes_when_paused() {
    let mut g = playing(42);
    g.jump_to(6);
    g.status_time = 0.0;
    g.play.sky_time = 0.0;
    g.take_effects();

    assert!(!g.tick(0.1, &idle()).contains(&Effect::Redraw));
    assert!(g.tick(0.1, &idle()).contains(&Effect::Redraw));
    assert!(!g.tick(0.1, &idle()).contains(&Effect::Redraw));

    g.mode = Mode::Paused;
    let held_time = g.play.sky_time;
    g.tick(0.1, &idle());
    assert_eq!(g.play.sky_time, held_time);
}

fn tick(g: &mut Game, i: Intents) -> Vec<Effect> {
    g.tick(FRAME, &i)
}

/// Idle frames until `done`, or panic after `limit`. Returns every effect.
fn run_until(g: &mut Game, limit: usize, done: impl Fn(&Game) -> bool) -> Vec<Effect> {
    let mut all = Vec::new();
    for _ in 0..limit {
        if done(g) {
            return all;
        }
        all.extend(tick(g, idle()));
    }
    assert!(done(g), "not done after {limit} frames");
    all
}

/// `n` idle frames. Returns every effect.
fn frames(g: &mut Game, n: usize) -> Vec<Effect> {
    (0..n).flat_map(|_| tick(g, idle())).collect()
}

fn count(fx: &[Effect], want: impl Fn(&Effect) -> bool) -> usize {
    fx.iter().filter(|e| want(e)).count()
}

fn is_score(e: &Effect) -> bool {
    matches!(e, Effect::Store(Store::Score { .. }))
}

fn is_settings(e: &Effect) -> bool {
    matches!(e, Effect::Store(Store::Settings))
}

fn is_run_write(e: &Effect) -> bool {
    matches!(e, Effect::RunFile(RunFile::Write(_)))
}

fn entry(seed: u64) -> Entry {
    Entry {
        seed,
        screens: 1,
        shells: 0,
        shots: 40,
        daily: false,
    }
}

/// A run with every shell thrown away: over, and ready to be scored.
fn spent_run(seed: u64) -> RunState {
    let mut run = RunState::new(seed, 320);
    while !run.is_over() {
        if run.fire(Shot::new(1400, 0), false).is_none() {
            // A lucky lob cleared the screen.
            run.resolve_clear();
            run.take_offer(0);
        }
    }
    run
}

/// A run held at its first draft by the autopilot.
fn at_draft() -> Game {
    let mut g = playing(777);
    g.dev.auto = true;
    g.dev.auto_hold_draft = true;
    run_until(&mut g, 5000, |g| {
        g.play.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting)
    });
    g.dev.auto = false;
    g.take_effects();
    g
}

// ------------------------------------------------------------------ modes

#[test]
fn the_game_starts_at_the_title_sized_for_the_window() {
    let mut g = Game::new(false, Save::default());
    let fx = g.fit_display(WINDOW, (0, 0));
    assert_eq!(g.mode, Mode::Title);
    assert_eq!((g.display.buffer_w, g.display.buffer_h), (320, 180));
    assert_eq!(
        fx,
        vec![
            Effect::Buffer {
                w: 320,
                h: 180,
                letterbox: (0, 0)
            },
            Effect::Redraw
        ]
    );
}

#[test]
fn a_wider_window_makes_a_wider_buffer() {
    let mut g = Game::new(false, Save::default());
    g.fit_display((1600, 720), (0, 0));
    assert_eq!(g.display.buffer_w, 400);
}

#[test]
fn confirm_on_the_title_starts_todays_daily() {
    let mut g = game();
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(g.play.daily);
    assert_eq!(g.play.last_seed, daily_seed(TODAY));
    assert_eq!(g.play.run.as_ref().unwrap().width, 320);
    assert!(fx.contains(&Effect::Play(Sound::Ui)));
    assert!(fx.contains(&Effect::Aspect(Aspect::Keep)));
    assert_eq!(g.status, format!("SEED {}", format_seed(daily_seed(TODAY))));
    // A new run goes to disk at once, log and all.
    assert_eq!(count(&fx, is_run_write), 1);
}

#[test]
fn a_pad_can_start_a_run_and_the_prompts_follow_it() {
    let mut g = game();
    tick(&mut g, pads(&[Pad::A]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(g.input.controller_active);
    assert_eq!(g.input_label(Action::Fire), "A");
    tick(&mut g, keys(&[Key::Left]));
    assert!(!g.input.controller_active);
    assert_eq!(g.input_label(Action::Fire), "SPACE");
}

#[test]
fn a_challenge_carries_a_hint_arc_and_a_normal_run_leaves_none_behind() {
    let mut g = game();
    g.start_challenge(0);
    assert_eq!(g.play.challenge, Some(0));
    assert!(!g.play.hint_arc.is_empty());
    g.start_run(7, false);
    assert_eq!(g.play.challenge, None);
    assert!(g.play.hint_arc.is_empty());
}

#[test]
fn pasted_and_saved_one_gun_layouts_start_custom_challenges() {
    let challenge = salt_line_sim::challenge::get(0).unwrap();
    let layout = salt_line_sim::challenge::layout(challenge);
    let code = layout.to_code().unwrap();
    let budget = crate::custom::challenge_shots(&layout);

    let mut pasted = game();
    pasted.mode = Mode::Editor;
    pasted.pasted(&code);
    assert_eq!(pasted.mode, Mode::Playing);
    assert_eq!(pasted.play.custom_challenge.as_ref(), Some(&layout));
    assert_eq!(pasted.play.challenge, None);
    assert_eq!(pasted.play.run.as_ref().unwrap().run_seed, layout.digest());
    assert_eq!(pasted.play.run.as_ref().unwrap().shells, budget);
    assert!(!pasted.play.hint_arc.is_empty());
    assert!(!pasted.run_file.run_persist);
    assert!(pasted.play.run.as_ref().unwrap().log.is_empty());

    let mut saved = game();
    saved.start_custom_challenge(layout.clone());
    saved.mode = Mode::GameOver;
    saved.save_custom_challenge_layout();
    let (text, why) = saved
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Store(Store::Layouts { text, why }) => Some((text, why)),
            _ => None,
        })
        .expect("saving a custom challenge writes its layout slot");
    assert_eq!(
        crate::layouts_file::Slots::parse(&text)
            .get(1)
            .unwrap()
            .layout()
            .unwrap(),
        layout
    );
    saved.stored(Store::Layouts { text, why }, true);
    saved.open_row(crate::app::editor::SlotRow::Slot(1));
    assert_eq!(saved.mode, Mode::Playing);
    assert_eq!(saved.play.custom_challenge.as_ref(), Some(&layout));
}

#[test]
fn challenges_begin_at_the_first_uncleared_and_keep_the_best() {
    let mut g = game();
    assert_eq!(g.next_challenge(), 0);
    g.start_challenge(0);
    g.play.run.as_mut().unwrap().shells = 2;
    g.record_challenge_clear();
    assert_eq!(g.save.challenge_best(0), Some(2));
    assert_eq!(g.next_challenge(), 1);
    g.play.run.as_mut().unwrap().shells = 1;
    g.record_challenge_clear();
    assert_eq!(
        g.save.challenge_best(0),
        Some(2),
        "a worse clear keeps the best"
    );
    for i in 1..salt_line_sim::challenge::ALL.len() {
        g.save.record_challenge(i as u8, 1);
    }
    assert_eq!(g.next_challenge(), 0, "all cleared starts over");
}

#[test]
fn custom_challenge_best_is_recorded_by_layout_digest() {
    let challenge = salt_line_sim::challenge::get(0).unwrap();
    let layout = salt_line_sim::challenge::layout(challenge);
    let digest = layout.digest();
    let mut g = game();
    g.start_custom_challenge(layout);
    g.play.run.as_mut().unwrap().shells = 2;
    g.record_challenge_clear();
    assert_eq!(g.save.custom_challenge_best(digest), Some(2));
    assert_eq!(
        Save::parse(&g.save.serialize()).custom_challenge_best(digest),
        Some(2)
    );
}

#[test]
fn custom_challenge_finishes_without_a_run_score_or_replay() {
    let layout = salt_line_sim::challenge::layout(salt_line_sim::challenge::get(0).unwrap());
    let mut g = game();
    g.start_custom_challenge(layout);
    g.play.run.as_mut().unwrap().phase = Phase::Over(salt_line_sim::run::RunOver::OutOfShells);
    g.finish_run();
    assert_eq!(g.mode, Mode::GameOver);
    assert_eq!(g.status, "CHALLENGE COMPLETE");
    assert!(!g.run_file.run_persist);
    assert!(g.history.last_run.is_none());
    assert!(!g
        .take_effects()
        .iter()
        .any(|effect| matches!(effect, Effect::Store(Store::Score { .. }))));
}

#[test]
fn every_title_row_is_reachable_from_a_pad() {
    let mut g = game();
    tick(&mut g, pads(&[Pad::DpadUp]));
    assert_eq!(g.mode, Mode::Replays, "d-pad up is REPLAYS");
    let mut g = game();
    tick(&mut g, pads(&[Pad::DpadLeft]));
    assert_eq!(g.mode, Mode::Playing, "d-pad left is CHALLENGES");
    assert!(g.play.challenge.is_some());
}

#[test]
fn a_challenge_copies_its_layout_code() {
    let mut g = game();
    g.start_challenge(1);
    g.play.run.as_mut().unwrap().phase =
        salt_line_sim::run::Phase::Over(salt_line_sim::run::RunOver::OutOfShells);
    g.finish_run();
    g.take_effects();
    let fx = tick(&mut g, keys(&[Key::C]));
    let code = fx
        .iter()
        .find_map(|e| match e {
            Effect::Clipboard(c) => Some(c.clone()),
            _ => None,
        })
        .expect("a code is copied");
    let challenge = salt_line_sim::challenge::get(1).unwrap();
    let expected = salt_line_sim::challenge::layout(challenge);
    assert_eq!(
        salt_line_sim::layout::Layout::from_code(&code)
            .unwrap()
            .digest(),
        expected.digest()
    );
    assert_eq!(g.mode, Mode::GameOver);
}

#[test]
fn a_failed_challenge_retries_itself_not_a_seeded_run() {
    let mut g = game();
    g.start_challenge(1);
    g.play.run.as_mut().unwrap().phase =
        salt_line_sim::run::Phase::Over(salt_line_sim::run::RunOver::OutOfShells);
    g.finish_run();
    assert_eq!(g.mode, Mode::GameOver);
    tick(&mut g, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.challenge, Some(1));
    assert_eq!(g.play.run.as_ref().unwrap().shells, 4);
}

#[test]
fn a_phone_remembers_the_controller_across_launches() {
    let mut g = Game::new(true, Save::default());
    assert!(g.input.touch_active);
    tick(&mut g, pads(&[Pad::A]));
    assert_eq!(g.save.controller, Some(true));
    let reloaded = Game::new(true, Save::parse(&g.save.serialize()));
    assert!(reloaded.input.controller_active && !reloaded.input.touch_active);

    let mut g = reloaded;
    tick(
        &mut g,
        Intents {
            events: vec![Event::FingerDown {
                index: 0,
                x: 15.0,
                y: 15.0,
            }],
            ..Intents::default()
        },
    );
    assert_eq!(g.save.controller, Some(false));
    assert!(Game::new(true, g.save.clone()).input.touch_active);
}

#[test]
fn back_pauses_and_back_again_resumes() {
    let mut g = playing(1);
    tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(g.mode, Mode::Paused);
    let fx = tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(fx.contains(&Effect::Play(Sound::Ui)));
}

#[test]
fn the_platform_back_gesture_is_the_back_action_once() {
    let mut g = playing(1);
    g.events(&[Event::Back]);
    tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Paused);
    tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Paused, "taken once, not held");
}

#[test]
fn quitting_to_the_title_keeps_the_run_for_continue() {
    let mut g = playing(5);
    tick(&mut g, keys(&[Key::Escape]));
    let fx = tick(&mut g, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Title);
    assert!(g.run_file.resumable.is_some());
    assert!(!fx.contains(&Effect::RunFile(RunFile::Discard)));
    // Entering the title looks at the display again.
    assert!(fx.contains(&Effect::RecheckDisplay));

    // Confirm is CONTINUE now, and puts the player back where they were.
    let seed = g.play.run.as_ref().unwrap().run_seed;
    tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(g.run_file.resumed);
    assert_eq!(g.play.run.as_ref().unwrap().run_seed, seed);
    assert_eq!(g.status, format!("SCREEN 1  SEED {}", format_seed(seed)));
}

#[test]
fn a_new_run_over_a_saved_one_asks_twice() {
    let mut g = playing(5);
    tick(&mut g, keys(&[Key::Escape]));
    tick(&mut g, keys(&[Key::R]));
    let fx = tick(&mut g, keys(&[Key::N]));
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(g.run_file.discard_armed, Some("daily"));
    assert!(fx.contains(&Effect::Play(Sound::Ui)));
    // A different action arms itself instead.
    tick(&mut g, keys(&[Key::S]));
    assert_eq!(g.run_file.discard_armed, Some("seed"));
    tick(&mut g, keys(&[Key::N]));
    tick(&mut g, keys(&[Key::N]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(g.play.daily);
    assert!(g.run_file.resumable.is_none());
    assert_eq!(g.run_file.discard_armed, None);
}

#[test]
fn controls_and_back() {
    let mut g = game();
    tick(&mut g, keys(&[Key::V]));
    assert_eq!(g.mode, Mode::Controls);
    tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn back_on_the_title_quits_on_the_second_press() {
    let mut g = game();
    assert!(!tick(&mut g, keys(&[Key::Escape])).contains(&Effect::Quit));
    assert!(g.quit_armed.is_some());
    tick(&mut g, Intents::default());
    assert!(tick(&mut g, keys(&[Key::Escape])).contains(&Effect::Quit));
}

#[test]
fn an_armed_quit_on_the_title_lapses() {
    let mut g = game();
    tick(&mut g, keys(&[Key::Escape]));
    assert!(g.busy(), "the lapse needs frames to count down");
    let frames = (versus::QUIT_CONFIRM / FRAME).ceil() as usize + 1;
    for _ in 0..frames {
        tick(&mut g, Intents::default());
    }
    assert_eq!(g.quit_armed, None);
    assert!(!tick(&mut g, keys(&[Key::Escape])).contains(&Effect::Quit));
}

#[test]
fn leaving_the_title_disarms_quit() {
    let mut g = game();
    tick(&mut g, keys(&[Key::Escape]));
    tick(&mut g, keys(&[Key::V]));
    assert_eq!(g.mode, Mode::Controls);
    assert_eq!(g.quit_armed, None);
    tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(g.mode, Mode::Title);
    assert!(!tick(&mut g, keys(&[Key::Escape])).contains(&Effect::Quit));
}

// ------------------------------------------------------------- seed entry

#[test]
fn a_typed_seed_starts_that_run() {
    let mut g = game();
    tick(&mut g, keys(&[Key::S]));
    assert_eq!(g.mode, Mode::SeedEntry);
    for c in "SALTY".chars() {
        tick(&mut g, typing(Typed::Char(c)));
    }
    tick(&mut g, typing(Typed::Backspace));
    assert_eq!(g.seed_text, "SALT");
    tick(&mut g, typing(Typed::Enter));
    assert_eq!(g.mode, Mode::Playing);
    assert!(!g.play.daily);
    assert_eq!(g.play.last_seed, parse_seed("SALT"));
}

#[test]
fn a_blank_seed_is_todays_daily_seed_but_not_a_daily_run() {
    let mut g = game();
    tick(&mut g, keys(&[Key::S]));
    tick(
        &mut g,
        Intents {
            today: TODAY + 1,
            ..typing(Typed::Enter)
        },
    );
    assert_eq!(g.play.last_seed, daily_seed(TODAY + 1));
    assert!(!g.play.daily);
}

#[test]
fn seed_text_stops_at_twenty_characters_and_escape_leaves() {
    let mut g = game();
    g.open_seed_entry();
    for _ in 0..30 {
        tick(&mut g, typing(Typed::Char('7')));
    }
    assert_eq!(g.seed_text.len(), SEED_MAX);
    tick(&mut g, typing(Typed::Escape));
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn the_controller_types_on_the_keypad() {
    let mut g = game();
    g.open_seed_entry();
    // Right once from the first key, then A presses it.
    tick(&mut g, pads(&[Pad::DpadRight]));
    tick(&mut g, pads(&[Pad::A]));
    assert_eq!(g.seed_text, "B");
    assert_eq!(keypad::KEYS[1], Action::Type('B'));
    // B deletes, then leaves once nothing is typed.
    tick(&mut g, pads(&[Pad::B]));
    assert_eq!(g.seed_text, "");
    assert_eq!(g.mode, Mode::SeedEntry);
    tick(&mut g, pads(&[Pad::B]));
    assert_eq!(g.mode, Mode::Title);
}

// ------------------------------------------------------------------ aim

#[test]
fn a_tap_steps_the_aim_once_and_is_learned_once() {
    let mut g = playing(1);
    let quantum = g
        .play
        .run
        .as_ref()
        .unwrap()
        .loadout
        .angle_quantum_ticks
        .max(1);
    let fx = tick(&mut g, keys(&[Key::Right]));
    assert_eq!(g.play.angle_ticks, 900 + quantum);
    assert_eq!(g.save.learned & LEARN_ANGLE, LEARN_ANGLE);
    let learned = |e: &Effect| matches!(e, Effect::Store(Store::Learned));
    assert_eq!(count(&fx, learned), 1);
    let fx = tick(&mut g, keys(&[Key::Right]));
    assert_eq!(count(&fx, learned), 0, "already learned");
}

#[test]
fn a_held_key_repeats_after_the_delay() {
    let mut g = playing(1);
    tick(&mut g, keys(&[Key::Up]));
    assert_eq!(g.play.power_ticks, 605);
    // Held for just under the delay: nothing more.
    for _ in 0..16 {
        tick(&mut g, held(&[Key::Up]));
    }
    assert_eq!(g.play.power_ticks, 605);
    // Held on: repeats every 35 ms, about one every other frame.
    for _ in 0..60 {
        tick(&mut g, held(&[Key::Up]));
    }
    assert!(g.play.power_ticks > 605 + 5 * 20, "{}", g.play.power_ticks);
    // Let go: it stops.
    let before = g.play.power_ticks;
    tick(&mut g, idle());
    tick(&mut g, idle());
    assert_eq!(g.play.power_ticks, before);
}

#[test]
fn shift_makes_the_steps_coarse_and_the_aim_is_clamped() {
    let mut g = playing(1);
    let mut i = keys(&[Key::Down]);
    i.keys_held.push(Key::Shift);
    tick(&mut g, i);
    assert_eq!(g.play.power_ticks, 550);
    g.play.power_ticks = POWER_TICKS_MAX - 1;
    tick(&mut g, keys(&[Key::Up]));
    assert_eq!(g.play.power_ticks, POWER_TICKS_MAX);
    g.play.angle_ticks = 0;
    tick(&mut g, keys(&[Key::Left]));
    assert_eq!(g.play.angle_ticks, 0);
}

#[test]
fn the_stick_aims_as_the_dpad_does() {
    let mut g = playing(1);
    let mut i = idle();
    i.stick[input::STICK_RIGHT] = true;
    tick(&mut g, i);
    assert!(g.play.angle_ticks > 900);
    assert!(g.input.controller_active);
}

#[test]
fn the_fuse_moves_only_with_airburst() {
    let mut g = playing(1);
    let fuse = g.play.run.as_ref().unwrap().fuse;
    tick(&mut g, keys(&[Key::BracketRight]));
    assert_eq!(g.play.run.as_ref().unwrap().fuse, fuse);
    g.play
        .run
        .as_mut()
        .unwrap()
        .loadout
        .take(Modifier::Airburst);
    tick(&mut g, keys(&[Key::BracketRight]));
    assert!(g.play.run.as_ref().unwrap().fuse > fuse);
}

#[test]
fn reposition_without_enfilade_says_so() {
    let mut g = playing(1);
    tick(&mut g, keys(&[Key::Period]));
    assert_eq!(g.status, "NO ENFILADE");
    assert_eq!(g.play.run.as_ref().unwrap().emplacement_offset, 0.0);
}

// ----------------------------------------------------------------- firing

#[test]
fn firing_replays_the_shot_then_lands_it() {
    let mut g = playing(1);
    let shells = g.play.run.as_ref().unwrap().shells;
    let fx = tick(&mut g, keys(&[Key::Space]));
    assert!(g.play.anim.is_some());
    assert_eq!(g.play.run.as_ref().unwrap().shells, shells - 1);
    assert!(fx.contains(&Effect::Play(Sound::Fire)));
    assert!(fx.contains(&Effect::Play(Sound::Flight)));
    // The log grew, so the run file is written in the same frame.
    assert_eq!(count(&fx, is_run_write), 1);
    assert!(!g.play.last_arc.is_empty());

    // The screen shown is the one before the shot until the shell lands.
    assert!(std::ptr::eq(
        g.shown_screen().unwrap(),
        &g.play.anim.as_ref().unwrap().before
    ));

    // No aiming while it flies.
    let aim = (g.play.angle_ticks, g.play.power_ticks);
    tick(&mut g, keys(&[Key::Up]));
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), aim);

    let fx = run_until(&mut g, 2000, |g| g.play.anim.is_none());
    let impacts = count(&fx, |e| {
        matches!(
            e,
            Effect::Play(Sound::Terrain | Sound::Structure | Sound::Ricochet | Sound::Airburst)
        )
    });
    assert_eq!(impacts, 1);
    assert_eq!(count(&fx, is_run_write), 0, "nothing new to write");
    assert_eq!(g.save.learned & LEARN_CRATER, LEARN_CRATER);
    assert!(std::ptr::eq(
        g.shown_screen().unwrap(),
        &g.play.run.as_ref().unwrap().screen
    ));
}

#[test]
fn shell_run_clock_lesson_waits_until_the_reserve_falls_below_starting_value() {
    let mut g = playing(1);
    g.save.learned |= LEARN_WIND;
    assert_eq!(
        g.play.run.as_ref().unwrap().shells,
        salt_line_sim::START_SHELLS
    );

    g.fire();
    assert_eq!(g.save.learned & LEARN_SHELLS, 0);
    assert!(g.play.run.as_ref().unwrap().shells < salt_line_sim::START_SHELLS);

    g.play.anim = None;
    g.fire();
    assert_eq!(g.save.learned & LEARN_SHELLS, LEARN_SHELLS);
}

#[test]
fn the_shells_prompt_shows_only_once_the_reserve_is_below_its_start() {
    const PROMPT: &str = "SHELLS ARE THE RUN CLOCK - SPEND CAREFULLY";
    let mut g = playing(1);
    g.save.learned |= LEARN_ANGLE | LEARN_POWER | LEARN_FIRE | LEARN_CRATER | LEARN_WIND;
    let start = salt_line_sim::START_SHELLS;
    g.play.run.as_mut().unwrap().shells = start + 5;
    assert_eq!(g.onboarding_prompt(Phase::Firing, None), None);
    g.play.run.as_mut().unwrap().shells = start;
    assert_eq!(g.onboarding_prompt(Phase::Firing, None), None);
    g.play.run.as_mut().unwrap().shells = start - 1;
    assert_eq!(
        g.onboarding_prompt(Phase::Firing, None).as_deref(),
        Some(PROMPT)
    );
    g.save.learned |= LEARN_SHELLS;
    assert_eq!(g.onboarding_prompt(Phase::Firing, None), None);
}

#[test]
fn a_long_frame_moves_the_replay_a_tenth_of_a_second_at_most() {
    let mut g = playing(1);
    tick(&mut g, keys(&[Key::Space]));
    let t = g.play.anim.as_ref().unwrap().t;
    g.tick(30.0, &idle());
    assert!((g.play.anim.as_ref().unwrap().t - t - resume::MAX_FRAME_DELTA).abs() < 1e-9);
    g.status_time = 1.0;
    g.tick(f64::NAN, &idle());
    assert_eq!(g.status_time, 1.0, "a NaN frame is no time at all");
    g.tick(-1.0, &idle());
    assert_eq!(g.status_time, 1.0);
}

#[test]
fn the_last_shell_ends_the_run_and_it_is_scored_exactly_once() {
    let mut g = playing(9);
    let seed = g.play.last_seed;
    g.play.run.as_mut().unwrap().shells = 1;
    tick(&mut g, keys(&[Key::Space]));
    let fx = run_until(&mut g, 4000, |g| g.mode == Mode::GameOver);
    assert_eq!(count(&fx, is_score), 1);
    // Not on the board until the write says it is on disk.
    assert!(g.save.scores.is_empty());
    // The run file goes first, as part of the score: nothing discards it
    // unconfirmed ahead of it.
    assert!(!fx.contains(&Effect::RunFile(RunFile::Discard)));
    let score = fx.iter().position(is_score).unwrap();
    // Scored with its line kept in case the board cannot be written.
    let Effect::Store(why) = fx[score].clone() else {
        unreachable!()
    };
    assert_eq!(why.clear(), Some(""), "the finished log must go first");
    match &why {
        Store::Score {
            entries, announce, ..
        } => {
            assert_eq!(entries.iter().map(|e| e.seed).collect::<Vec<_>>(), [seed]);
            assert!(!announce);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(g.run_file.unscored.len(), 1, "waiting until it is on disk");
    assert_eq!(g.save_to_store(&why).scores.len(), 1);
    assert!(g.stored(why, true).is_empty());
    assert_eq!(g.save.scores.len(), 1);
    assert_eq!(g.save.scores[0].seed, seed);
    assert!(g.run_file.unscored.is_empty());
    assert!(fx.contains(&Effect::RecheckDisplay));
    let fx = frames(&mut g, 60);
    assert_eq!(count(&fx, is_score), 0);
    assert_eq!(count(&fx, is_run_write), 0);
}

#[test]
fn game_over_copies_the_seed_and_replays_it() {
    let mut g = playing(9);
    g.finish_run();
    g.take_effects();
    let fx = tick(&mut g, keys(&[Key::C]));
    assert!(fx.contains(&Effect::Clipboard(format_seed(9))));
    assert_eq!(g.status, format!("COPIED {}", format_seed(9)));
    tick(&mut g, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.last_seed, 9);
    g.finish_run();
    tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn quitting_a_run_that_already_ended_scores_it() {
    let mut g = playing(9);
    *g.play.run.as_mut().unwrap() = spent_run(9);
    g.mode = Mode::Paused;
    let fx = tick(&mut g, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::GameOver);
    assert_eq!(count(&fx, is_score), 1);
    assert!(g.run_file.resumable.is_none());
}

#[test]
fn a_board_that_cannot_be_written_keeps_the_run_and_says_so() {
    let mut g = game();
    g.run_file.unscored.push(Unscored {
        entry: entry(1),
        line: "line\n".into(),
    });
    let fx = g.stored(
        Store::Score {
            entries: vec![entry(1)],
            clear: Some(String::new()),
            announce: false,
        },
        false,
    );
    assert_eq!(fx[0], Effect::RunFile(RunFile::Write("line\n".into())));
    assert_eq!(g.run_file.unscored.len(), 1);
    assert_eq!(g.status, "SCORE NOT SAVED");
    assert!(
        g.save.scores.is_empty(),
        "a failed score stays off the board"
    );

    // A save from a newer build fails every write: one note saying why.
    let mut g = game();
    g.save.from_the_future = true;
    g.stored(Store::Settings, false);
    assert_eq!(g.status, "SAVE IS FROM A NEWER VERSION");
    g.status.clear();
    g.stored(Store::Settings, false);
    assert_eq!(g.status, "");
    assert!(g.stored(Store::Learned, false).is_empty());
}

// ------------------------------------------------------------------ draft

#[test]
fn a_draft_pick_takes_the_offer() {
    let mut g = at_draft();
    let offer = g.play.run.as_ref().unwrap().offers[1];
    let fx = tick(&mut g, keys(&[Key::Num2]));
    assert!(g.play.run.as_ref().unwrap().loadout.has(offer));
    assert!(fx.contains(&Effect::Play(Sound::Draft)));
    assert_eq!(g.status, format!("TOOK {}", offer.name()));
    assert_eq!(g.save.learned & LEARN_DRAFT, LEARN_DRAFT);
    // A draft pick is logged, so the run file follows it.
    assert_eq!(count(&fx, is_run_write), 1);
}

#[test]
fn in_a_draft_pad_a_picks_the_second_card_and_does_not_fire() {
    let mut g = at_draft();
    let offer = g.play.run.as_ref().unwrap().offers[1];
    tick(&mut g, pads(&[Pad::A]));
    assert!(g.play.run.as_ref().unwrap().loadout.has(offer));
    assert!(g.play.anim.is_none());
}

#[test]
fn the_draft_prompt_names_the_keys_until_a_card_is_taken() {
    let g = at_draft();
    assert_eq!(
        g.onboarding_prompt(Phase::Drafting, None).as_deref(),
        Some("1 2 3  PICK A MODIFIER")
    );
}

// ------------------------------------------------------------ drag aiming

fn drag_game() -> Game {
    let mut g = playing(1);
    g.input.touch_active = true;
    g.save.touch_aim = Some(TouchAim::Drag);
    g
}

#[test]
fn a_drag_aims_live_and_fires_on_release() {
    let mut g = drag_game();
    g.events(&[Event::FingerDown {
        index: 0,
        x: 200.0,
        y: 100.0,
    }]);
    g.events(&[Event::FingerMoved {
        index: 0,
        x: 150.0,
        y: 140.0,
    }]);
    tick(&mut g, idle());
    let aim = (g.play.angle_ticks, g.play.power_ticks);
    assert_ne!(aim, (900, 600));
    assert_eq!(g.input.drag_from, Some((900, 600)));
    assert_eq!(
        g.onboarding_prompt(Phase::Firing, None).as_deref(),
        Some("LET GO TO FIRE  PULL BACK TO CANCEL")
    );
    g.events(&[Event::FingerUp { index: 0 }]);
    let fx = tick(&mut g, idle());
    assert!(fx.contains(&Effect::Play(Sound::Fire)));
    assert!(g.play.anim.is_some());
    assert_eq!(g.input.drag_from, None);
}

#[test]
fn a_drag_pulled_back_into_the_dead_zone_cancels() {
    let mut g = drag_game();
    g.events(&[Event::FingerDown {
        index: 0,
        x: 200.0,
        y: 100.0,
    }]);
    g.events(&[Event::FingerMoved {
        index: 0,
        x: 150.0,
        y: 140.0,
    }]);
    tick(&mut g, idle());
    g.events(&[Event::FingerMoved {
        index: 0,
        x: 201.0,
        y: 101.0,
    }]);
    tick(&mut g, idle());
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), (900, 600));
    g.events(&[Event::FingerUp { index: 0 }]);
    tick(&mut g, idle());
    assert!(g.play.anim.is_none());
}

#[test]
fn losing_focus_mid_drag_puts_the_aim_back() {
    let mut g = drag_game();
    g.events(&[Event::FingerDown {
        index: 0,
        x: 200.0,
        y: 100.0,
    }]);
    g.events(&[Event::FingerMoved {
        index: 0,
        x: 150.0,
        y: 140.0,
    }]);
    tick(&mut g, idle());
    g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Paused);
    // Back by touch: a key press would hand aiming to the keyboard.
    g.events(&[Event::Back]);
    tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Playing);
    tick(&mut g, idle());
    assert_eq!((g.play.angle_ticks, g.play.power_ticks), (900, 600));
    assert!(g.play.anim.is_none());
}

#[test]
fn touch_buttons_press_their_actions() {
    let mut g = playing(1);
    let fire = Button {
        rect: crate::touch::Rect::new(10, 10, 20, 20),
        action: Action::Fire,
    };
    g.set_buttons(&[fire]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 15.0,
        y: 15.0,
    }]);
    assert!(g.input.touch_active);
    tick(&mut g, idle());
    assert!(g.play.anim.is_some());
    // A key press hands the prompts back to the keyboard.
    tick(&mut g, keys(&[Key::M]));
    assert!(!g.input.touch_active);
}

// -------------------------------------------------------- focus and pause

#[test]
fn losing_focus_pauses_a_live_screen() {
    let mut g = playing(1);
    let fx = g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Paused);
    assert!(fx.contains(&Effect::Redraw));

    let mut g = playing(1);
    g.app_paused(&[]);
    assert_eq!(g.mode, Mode::Paused);
}

#[test]
fn losing_focus_does_not_pause_a_draft_the_autopilot_or_a_capture() {
    let mut g = at_draft();
    g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Playing);

    let mut g = playing(1);
    g.dev.auto = true;
    g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Playing);

    let mut g = playing(1);
    g.dev.capturing = true;
    g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Playing);

    let mut g = game();
    g.focus_lost(&[]);
    assert_eq!(g.mode, Mode::Title);
}

// --------------------------------------------------------------- settings

#[test]
fn volume_changes_are_written_once_they_settle() {
    let mut g = game();
    tick(&mut g, keys(&[Key::V]));
    let fx = tick(&mut g, keys(&[Key::Left]));
    assert_eq!(g.save.volume, Save::default().volume - 5);
    assert!(fx.contains(&Effect::Volume(g.volume_db())));
    assert!(fx.contains(&Effect::Play(Sound::Ui)));
    assert_eq!(count(&fx, is_settings), 0, "not every step");
    let fx = frames(&mut g, 60);
    assert_eq!(count(&fx, is_settings), 1);
}

#[test]
fn leaving_controls_or_losing_focus_writes_settings_at_once() {
    let mut g = game();
    tick(&mut g, keys(&[Key::V]));
    tick(&mut g, keys(&[Key::L]));
    assert_eq!(g.save.last_arc, Some(false));
    let fx = tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(count(&fx, is_settings), 1);

    // Anywhere else a change is written in its own frame.
    let mut g = playing(1);
    let fx = tick(&mut g, keys(&[Key::M]));
    assert!(g.save.muted);
    assert_eq!(g.status, "SOUND MUTED");
    assert_eq!(count(&fx, is_settings), 1);

    // In Controls, focus loss does not wait for the change to settle.
    let mut g = game();
    tick(&mut g, keys(&[Key::V]));
    tick(&mut g, keys(&[Key::Right]));
    let fx = g.focus_lost(&[]);
    assert_eq!(count(&fx, is_settings), 1);
    assert_eq!(
        count(&g.closing(&[]), is_settings),
        0,
        "nothing left to write"
    );
}

#[test]
fn muting_silences_every_player() {
    let mut g = game();
    let fx = tick(&mut g, keys(&[Key::M]));
    assert!(fx.contains(&Effect::Volume(-80.0)));
    let fx = tick(&mut g, keys(&[Key::M]));
    assert!(fx.contains(&Effect::Volume(g.volume_db())));
    assert!(g.volume_db() < 0.0 && g.volume_db() > -80.0);
}

#[test]
fn the_title_music_button_is_separate_from_effects_and_persists() {
    let mut g = game();
    g.set_buttons(&[button(Action::TitleMusic)]);
    g.events(&[
        Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        },
        Event::FingerUp { index: 0 },
    ]);
    let fx = tick(&mut g, idle());
    assert!(!g.save.title_music);
    assert!(!g.save.muted);
    assert_eq!(g.status, "TITLE MUSIC OFF");
    assert!(fx.contains(&Effect::TitleMusic(false)));
    assert_eq!(count(&fx, is_settings), 1);
}

#[test]
fn starting_a_run_stops_the_title_loop() {
    let mut g = game();
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(fx.contains(&Effect::TitleMusic(false)));
}

#[test]
fn a_settings_write_that_fails_says_so() {
    let mut g = game();
    g.stored(Store::Settings, false);
    assert_eq!(g.status, "SETTINGS NOT SAVED");
}

// ---------------------------------------------------------- crash reports

#[test]
fn last_sessions_crash_report_is_copied_once_and_retired() {
    let mut g = game();
    g.crash.last_crash = Some("SALT LINE CRASH REPORT".into());
    let fx = tick(&mut g, keys(&[Key::C]));
    assert!(fx.contains(&Effect::Clipboard("SALT LINE CRASH REPORT".into())));
    assert_eq!(
        count(&fx, |e| *e == Effect::CrashReport(CrashReport::Retire)),
        1
    );
    let fx = tick(&mut g, keys(&[Key::C]));
    assert_eq!(
        count(&fx, |e| *e == Effect::CrashReport(CrashReport::Retire)),
        0
    );
    // Starting a run stops offering it; the file was already moved.
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert!(g.crash.last_crash.is_none());
    assert_eq!(
        count(&fx, |e| *e == Effect::CrashReport(CrashReport::Retire)),
        0
    );
}

#[test]
fn starting_a_run_retires_an_uncopied_report() {
    let mut g = game();
    g.crash.last_crash = Some("report".into());
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(
        count(&fx, |e| *e == Effect::CrashReport(CrashReport::Retire)),
        1
    );
}

#[test]
fn the_error_screen_copies_only_a_finished_report() {
    let mut g = playing(1);
    g.status_time = 2.0;
    let fx = g.broken_tick(FRAME, &keys(&[Key::C]), || None);
    assert!(g.crash.broken);
    assert!(g.idle());
    assert!(fx.iter().all(|e| !matches!(e, Effect::Clipboard(_))));
    assert_eq!(g.status, "REPORT NOT READY - TRY AGAIN");
    let fx = g.broken_tick(FRAME, &keys(&[Key::C]), || Some("report".into()));
    assert!(fx.contains(&Effect::Clipboard("report".into())));
    let fx = g.broken_tick(FRAME, &keys(&[Key::Escape]), || unreachable!());
    assert!(fx.contains(&Effect::Quit));
}

// ----------------------------------------------------------------- resume

#[test]
fn a_rebuild_that_never_finished_is_thrown_away() {
    let mut g = game();
    let fx = g.saved_run(SavedRun::Interrupted);
    assert_eq!(
        fx[0],
        Effect::RunFile(RunFile::Abandon(Abandoned::Interrupted))
    );
    assert_eq!(g.status, resume::Stale::Diverged.note());
    assert_eq!(g.status_time, STALE_STATUS_TIME);
    assert!(g.run_file.resumable.is_none());
}

#[test]
fn a_saved_run_is_offered_and_its_file_put_back() {
    let mut run = RunState::new(42, 320);
    run.fire(Shot::new(1400, 330), false);
    let line = Saved::of(&run, true).to_line();
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found(line));
    assert_eq!(fx, vec![Effect::RunFile(RunFile::Restore)]);
    assert_eq!(g.run_file.resumable.as_ref().map(|s| s.seed), Some(42));

    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(g.play.daily);
    assert_eq!(g.play.run.as_ref().unwrap().log.len(), 1);
    // Already on disk as rebuilt: nothing to write.
    assert_eq!(count(&fx, is_run_write), 0);
}

#[test]
fn a_run_from_another_width_keeps_it_letterboxed_until_the_title() {
    let run = RunState::new(42, 400);
    let mut g = game();
    g.saved_run(SavedRun::Found(Saved::of(&run, false).to_line()));
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.display.buffer_w, 400);
    assert!(g.display.kept_width);
    assert!(fx.contains(&Effect::Aspect(Aspect::Keep)));
    tick(&mut g, keys(&[Key::Escape]));
    let fx = tick(&mut g, keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Title);
    let fx = [fx, tick(&mut g, idle())].concat();
    assert_eq!(g.display.buffer_w, 320);
    assert!(!g.display.kept_width);
    assert!(fx.contains(&Effect::Aspect(Aspect::Keep)));
}

#[test]
fn a_saved_run_that_had_ended_is_scored_at_launch() {
    let run = spent_run(3);
    let line = Saved::of(&run, false).to_line();
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found(line.clone()));
    // Put back first, so a clear that cannot be written leaves it in place.
    assert_eq!(fx[0], Effect::RunFile(RunFile::Restore));
    let Effect::Store(why) = fx[1].clone() else {
        panic!("{:?}", fx[1])
    };
    // The file goes before the score, confirmed.
    assert_eq!(why.clear(), Some(""));
    match &why {
        Store::Score { announce, .. } => assert!(announce),
        other => panic!("{other:?}"),
    }
    // Its line kept, in case this launch cannot write the board either.
    let kept: Vec<&str> = g
        .run_file
        .unscored
        .iter()
        .map(|u| u.line.as_str())
        .collect();
    assert_eq!(kept, [line.as_str()]);
    assert!(g.run_file.resumable.is_none());
    g.stored(why, true);
    assert_eq!(g.save.scores.len(), 1);
    assert_eq!(g.status, "LAST RUN SCORED");
}

#[test]
fn a_kept_score_beside_a_saved_run_is_scored_and_the_run_offered() {
    let finished = Saved::of(&spent_run(3), false).to_line();
    let mut live = RunState::new(42, 320);
    live.fire(Shot::new(1400, 330), false);
    let live = Saved::of(&live, false).to_line();
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found(format!("{finished}{live}")));
    assert_eq!(fx[0], Effect::RunFile(RunFile::Restore));
    let Effect::Store(why) = fx[1].clone() else {
        panic!("{:?}", fx[1])
    };
    // The finished line goes, the live one stays, before the board.
    assert_eq!(why.clear(), Some(live.as_str()));
    assert_eq!(g.run_file.resumable.as_ref().map(|s| s.seed), Some(42));
    // Not written: kept, with the run to continue after it.
    let fx = g.stored(why, false);
    assert_eq!(
        fx[0],
        Effect::RunFile(RunFile::Write(format!("{finished}{live}")))
    );

    // A line gone bad beside a good run: the run is kept, alone.
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found(format!("resume junk\n{live}")));
    assert_eq!(fx[0], Effect::RunFile(RunFile::Restore));
    assert_eq!(fx[1], Effect::RunFile(RunFile::Write(live.clone())));
    assert_eq!(g.status_time, STALE_STATUS_TIME);
    assert!(g.run_file.resumable.is_some());
}

#[test]
fn a_save_from_a_newer_build_scores_nothing_and_leaves_the_run_file_alone() {
    let mut g = playing(9);
    g.save.from_the_future = true;
    *g.play.run.as_mut().unwrap() = spent_run(9);
    g.take_effects();
    g.finish_run();
    let fx = g.take_effects();
    // No clear and no save write, which would only be refused: the finished
    // run is written back as it is, for a build that can score it.
    assert_eq!(count(&fx, is_score), 0);
    let line = Saved::of(&spent_run(9), false).to_line();
    assert!(fx.contains(&Effect::RunFile(RunFile::Write(line.clone()))));
    assert_eq!(g.status, "SAVE IS FROM A NEWER VERSION");
    assert_eq!(g.run_file.unscored.len(), 1);

    // At launch too: the file is only put back.
    let mut g = game();
    g.save.from_the_future = true;
    let fx = g.saved_run(SavedRun::Found(line.clone()));
    assert_eq!(count(&fx, is_score), 0);
    assert_eq!(fx[0], Effect::RunFile(RunFile::Restore));
    assert!(!fx.contains(&Effect::RunFile(RunFile::Discard)));
}

#[test]
fn a_score_that_never_touched_the_run_file_leaves_the_waiting_ones_waiting() {
    let mut g = game();
    g.run_file.unscored.push(Unscored {
        entry: entry(1),
        line: "line\n".into(),
    });
    // A `--salt-screen` run with exactly the same entry.
    g.stored(
        Store::Score {
            entries: vec![entry(1)],
            clear: None,
            announce: false,
        },
        true,
    );
    assert_eq!(g.run_file.unscored.len(), 1);
    assert_eq!(g.save.scores.len(), 1);
}

#[test]
fn a_score_at_launch_does_not_hide_a_line_gone_bad() {
    let finished = Saved::of(&spent_run(3), false).to_line();
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found(format!("resume junk\n{finished}")));
    let Some(Effect::Store(why)) = fx.into_iter().find(is_score) else {
        panic!("no score")
    };
    g.stored(why, true);
    assert!(g.status.contains("LAST RUN SCORED"), "{}", g.status);
    assert!(
        g.status.contains(resume::Stale::Damaged.note()),
        "{}",
        g.status
    );
}

#[test]
fn a_run_file_this_build_cannot_read_is_discarded() {
    let mut g = game();
    let fx = g.saved_run(SavedRun::Found("not a run".into()));
    assert!(matches!(
        fx[0],
        Effect::RunFile(RunFile::Abandon(Abandoned::Unreadable(_)))
    ));
    assert_eq!(g.status_time, STALE_STATUS_TIME);
    assert!(g.run_file.resumable.is_none());
}

#[test]
fn continue_that_cannot_rebuild_drops_the_run() {
    let mut g = game();
    let mut saved = Saved::of(&RunState::new(42, 320), false);
    // A logged shot at a screen the run never reached cannot replay.
    saved.log.push(salt_line_sim::run::LoggedShot {
        screen_index: 9,
        angle_ticks: 900,
        power_ticks: 600,
        fuse_tenths: 0,
        emplacement_offset: 0,
    });
    g.run_file.resumable = Some(saved);
    let fx = tick(&mut g, keys(&[Key::Enter]));
    assert!(fx.contains(&Effect::RunFile(RunFile::Discard)));
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(g.status, resume::Stale::Diverged.note());
}

// -------------------------------------------------------------- dev flags

#[test]
fn a_screen_jump_is_not_kept_for_resume() {
    let mut g = game();
    let args = DevArgs::parse(&[
        "--salt-seed".into(),
        "7".into(),
        "--salt-screen".into(),
        "12".into(),
    ]);
    g.start_dev(&args, TODAY);
    assert_eq!(g.play.run.as_ref().unwrap().screen_index, 12);
    assert_eq!(g.status, "SCREEN 13");
    assert!(!g.run_file.run_persist);
    let fx = frames(&mut g, 30);
    assert_eq!(count(&fx, is_run_write), 0);
}

#[test]
fn the_autopilot_starts_a_run_and_plays_it() {
    let mut g = game();
    g.start_dev(&DevArgs::parse(&["--salt-auto".into()]), TODAY);
    tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.last_seed, daily_seed(TODAY));
    run_until(&mut g, 600, |g| {
        g.play.run.as_ref().unwrap().total_shots > 0
    });
}

// ----------------------------------------------------------------- redraw

#[test]
fn a_quiet_frame_asks_for_no_redraw() {
    let mut g = playing(1);
    g.status_time = 0.0;
    assert!(!g.busy());
    assert_eq!(tick(&mut g, idle()), vec![]);
    let fx = tick(&mut g, keys(&[Key::Up]));
    assert!(fx.contains(&Effect::Redraw));
}

#[test]
fn f1_toggles_the_debug_overlay() {
    let mut g = playing(1);
    tick(&mut g, keys(&[Key::F1]));
    assert!(g.debug);
}

// ----------------------------------------------------------- display fit

#[test]
fn a_phone_starts_with_touch_controls_and_corner_insets() {
    let g = Game::new(true, Save::default());
    assert!(g.input.touch_active);
    assert_eq!(g.display.corner_pad, (10, 6));
    assert_eq!(Game::new(false, Save::default()).display.corner_pad, (0, 0));
}

#[test]
fn a_camera_cutout_is_letterboxed_and_followed() {
    let mut g = Game::new(true, Save::default());
    // A 20:9 phone held landscape with a camera hole on the left.
    g.fit_display((2400, 1080), (96, 0));
    // 2304 usable pixels at 6 screen pixels to one.
    assert_eq!((g.display.buffer_w, g.display.buffer_h), (384, 180));
    assert_eq!(g.display.letterbox, (16, 0));
    // The phone turns round: the hole is on the right now.
    let mut i = idle();
    i.window = (2400, 1080);
    i.cutout = (0, 96);
    let fx = tick(&mut g, i);
    assert_eq!(g.display.letterbox, (0, 16));
    assert!(fx.contains(&Effect::Buffer {
        w: 384,
        h: 180,
        letterbox: (0, 16)
    }));
}

#[test]
fn the_buffer_keeps_its_width_through_a_run() {
    let mut g = playing(1);
    let mut wide = idle();
    wide.window = (1600, 720);
    tick(&mut g, wide.clone());
    assert_eq!(g.display.buffer_w, 320, "the world is as wide as the run");
    // The next run is generated for the window as it is now.
    g.finish_run();
    let mut again = keys(&[Key::R]);
    again.window = (1600, 720);
    tick(&mut g, again);
    assert_eq!(g.display.buffer_w, 400);
    assert_eq!(g.play.run.as_ref().unwrap().width, 400);
}

// ---------------------------------------------------------- touch menus

fn button(action: Action) -> Button {
    Button {
        rect: crate::touch::Rect::new(0, 0, 10, 10),
        action,
    }
}

#[test]
fn touch_settings_cycle_and_are_written_on_leaving() {
    let mut g = game();
    g.mode = Mode::Controls;
    g.set_buttons(&[button(Action::AimMode)]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 5.0,
        y: 5.0,
    }]);
    g.events(&[Event::FingerUp { index: 0 }]);
    let fx = tick(&mut g, idle());
    assert_eq!(g.save.touch_aim, Some(TouchAim::Drag));
    assert!(fx.contains(&Effect::Play(Sound::Ui)));
    g.set_buttons(&[button(Action::ArcHint)]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 5.0,
        y: 5.0,
    }]);
    g.events(&[Event::FingerUp { index: 0 }]);
    tick(&mut g, idle());
    assert_eq!(g.save.arc_hint, Some(DEFAULT_ARC_HINT.next()));
    g.set_buttons(&[button(Action::Back)]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 5.0,
        y: 5.0,
    }]);
    g.events(&[Event::FingerUp { index: 0 }]);
    let fx = tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(count(&fx, is_settings), 1);
}

#[test]
fn seasonal_controls_change_only_cosmetics_and_persist_on_exit() {
    use crate::seasonal::Seasonal;
    let mut g = game();
    g.start_run(42, false);
    let before = Saved::of(g.play.run.as_ref().unwrap(), false).to_line();
    g.mode = Mode::Controls;
    tick(&mut g, keys(&[Key::U]));
    assert_eq!(g.save.seasonal, Some(Seasonal::Off));
    tick(&mut g, idle());
    tick(
        &mut g,
        Intents {
            pads: vec![Pad::RightShoulder],
            ..idle()
        },
    );
    assert_eq!(g.save.seasonal, Some(Seasonal::Halloween));
    g.set_buttons(&[button(Action::Seasonal)]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 5.0,
        y: 5.0,
    }]);
    g.events(&[Event::FingerUp { index: 0 }]);
    tick(&mut g, idle());
    assert_eq!(g.save.seasonal, Some(Seasonal::Thanksgiving));
    let after = Saved::of(g.play.run.as_ref().unwrap(), false).to_line();
    assert_eq!(before, after, "skins must not change the replay log");
    let fx = tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(count(&fx, is_settings), 1);
    let back = Save::parse(&g.save.serialize());
    assert_eq!(back.seasonal, Some(Seasonal::Thanksgiving));
}

#[test]
fn capture_season_does_not_overwrite_saved_choice() {
    use crate::seasonal::Seasonal;
    let mut g = game();
    g.save.seasonal = Some(Seasonal::Off);
    let args = DevArgs::parse(&["--salt-season=christmas".into()]);
    g.start_dev(&args, TODAY);
    assert_eq!(g.dev.season, Some(Seasonal::Christmas));
    assert_eq!(
        Save::parse(&g.save.serialize()).seasonal,
        Some(Seasonal::Off)
    );
}

#[test]
fn the_touch_keypad_types_a_seed() {
    let mut g = game();
    g.open_seed_entry();
    for key in [
        Action::Type('S'),
        Action::Type('E'),
        Action::Type('A'),
        keypad::DEL,
        Action::Type('A'),
    ] {
        g.set_buttons(&[button(key)]);
        g.events(&[Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        }]);
        g.events(&[Event::FingerUp { index: 0 }]);
        tick(&mut g, idle());
    }
    assert_eq!(g.seed_text, "SEA");
    g.set_buttons(&[button(Action::Confirm)]);
    g.events(&[Event::FingerDown {
        index: 0,
        x: 5.0,
        y: 5.0,
    }]);
    g.events(&[Event::FingerUp { index: 0 }]);
    tick(&mut g, idle());
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.last_seed, parse_seed("SEA"));
}

#[test]
fn bind_lines_in_an_existing_save_still_bind() {
    let save = Save::parse(
        "version 2\nbind fire U B\nbind back C X\nbind aim_mode X X\nbind no_such_action L L\n",
    );
    let mut g = Game::new(false, save);
    g.fit_display(WINDOW, (0, 0));
    assert_eq!(g.action_binding(Action::Fire), ("U".into(), "B".into()));
    assert_eq!(g.action_binding(Action::Back), ("C".into(), "X".into()));
    // Unbindable and unknown names bind nothing; their neighbours keep their
    // defaults.
    assert_eq!(g.action_binding(Action::LastArc), ("L".into(), "X".into()));
    assert_eq!(g.action_binding(Action::AimMode), ("".into(), "".into()));
    tick(&mut g, keys(&[Key::Enter]));
    tick(&mut g, keys(&[Key::U]));
    assert!(g.play.anim.is_some(), "U fires");
    tick(&mut g, keys(&[Key::C]));
    assert_eq!(g.mode, Mode::Paused, "C is back");
}

#[test]
fn rebinding_moves_an_action_and_reset_restores_it() {
    let mut g = playing(1);
    g.save
        .bindings
        .push(("fire".into(), "U".into(), "B".into()));
    tick(&mut g, keys(&[Key::Space]));
    assert!(g.play.anim.is_none(), "space no longer fires");
    assert_eq!(g.input_label(Action::Fire), "U");
    tick(&mut g, keys(&[Key::U]));
    assert!(g.play.anim.is_some());

    let mut g = game();
    g.save
        .bindings
        .push(("back".into(), "C".into(), "B".into()));
    g.mode = Mode::Controls;
    let fx = tick(&mut g, keys(&[Key::R]));
    assert!(g.save.bindings.is_empty());
    assert_eq!(g.status, "DEFAULT BINDINGS RESTORED");
    assert_eq!(count(&fx, is_settings), 0, "written on leaving Controls");
}

// ------------------------------------------------------- events in order

fn typed_key(key: Key, typed: Typed) -> Event {
    Event::Key {
        key: Some(key),
        typed: Some(typed),
    }
}

fn with_events(events: Vec<Event>) -> Intents {
    Intents { events, ..idle() }
}

#[test]
fn an_m_typed_in_a_seed_does_not_also_mute() {
    let mut g = game();
    g.open_seed_entry();
    tick(
        &mut g,
        with_events(vec![typed_key(Key::M, Typed::Char('M'))]),
    );
    assert_eq!(g.seed_text, "M");
    assert!(!g.save.muted);
    // Outside seed entry the same key is only a press.
    let mut g = game();
    tick(
        &mut g,
        with_events(vec![typed_key(Key::M, Typed::Char('M'))]),
    );
    assert!(g.save.muted);
}

#[test]
fn the_enter_that_starts_a_seeded_run_does_not_fire_but_a_later_space_does() {
    let mut g = game();
    g.open_seed_entry();
    tick(
        &mut g,
        with_events(vec![
            typed_key(Key::S, Typed::Char('S')),
            typed_key(Key::Enter, Typed::Enter),
        ]),
    );
    assert_eq!(g.mode, Mode::Playing);
    assert_eq!(g.play.last_seed, parse_seed("S"));
    assert!(g.play.anim.is_none(), "Enter typed, not pressed");

    // A Space after the Enter, in the same frame, is no longer in seed entry.
    let mut g = game();
    g.open_seed_entry();
    tick(
        &mut g,
        with_events(vec![
            typed_key(Key::Enter, Typed::Enter),
            typed_key(Key::Space, Typed::Char(' ')),
        ]),
    );
    assert_eq!(g.mode, Mode::Playing);
    assert!(
        g.play.anim.is_some(),
        "the Space fired in the run's first frame"
    );
}

#[test]
fn a_key_that_types_nothing_moves_the_keypad_focus() {
    let mut g = game();
    g.open_seed_entry();
    g.input.controller_active = true;
    tick(&mut g, with_events(vec![Event::key(Key::Right)]));
    assert_eq!(g.input.seed_focus.action(), Action::Type('B'));
    assert_eq!(g.seed_text, "");
}

#[test]
fn the_crash_context_sees_the_frame_after_its_events() {
    let mut g = game();
    g.open_seed_entry();
    let mut seen = None;
    g.tick_with(
        FRAME,
        &with_events(vec![typed_key(Key::Enter, Typed::Enter)]),
        |g| seen = Some(g.mode),
    );
    assert_eq!(seen, Some(Mode::Playing));
}

#[test]
fn touches_arrive_with_the_frame() {
    let mut g = playing(1);
    let fire = Button {
        rect: crate::touch::Rect::new(10, 10, 20, 20),
        action: Action::Fire,
    };
    g.set_buttons(&[fire]);
    tick(
        &mut g,
        with_events(vec![
            Event::FingerDown {
                index: 0,
                x: 15.0,
                y: 15.0,
            },
            Event::FingerUp { index: 0 },
        ]),
    );
    assert!(g.input.touch_active);
    assert!(g.play.anim.is_some(), "a tap inside one frame still fires");
}

#[test]
fn a_touch_queued_before_focus_is_lost_is_let_go_with_the_rest() {
    let mut g = playing(1);
    let fx = g.focus_lost(&[Event::FingerDown {
        index: 0,
        x: 200.0,
        y: 100.0,
    }]);
    assert!(fx.contains(&Effect::Redraw));
    assert!(g.input.touch_active);
    assert!(!g.input.touch.active(), "no finger left stuck down");
    assert_eq!(g.mode, Mode::Paused);
}

#[test]
fn every_sound_has_its_player_slot_and_its_file() {
    // The node finds a sound's player at `sound as usize`, so `ALL` must be
    // in declaration order; and a sound with no file would play silence.
    let audio = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../art/audio");
    for (i, sound) in Sound::ALL.into_iter().enumerate() {
        assert_eq!(sound as usize, i, "{sound:?} out of order in Sound::ALL");
        let wav = audio.join(format!("{}.wav", sound.file()));
        assert!(wav.is_file(), "{sound:?}: no {}", wav.display());
        let import = audio.join(format!("{}.wav.import", sound.file()));
        assert!(
            import.is_file(),
            "{sound:?}: not imported, no {}",
            import.display()
        );
    }
}

#[test]
fn credits_return_to_settings_and_pages_redraw() {
    let mut g = game();
    tick(&mut g, keys(&[Key::V]));
    tick(&mut g, keys(&[Key::Enter]));
    assert_eq!(g.mode, Mode::Credits);
    let before = g.frame_state();
    tick(&mut g, keys(&[Key::Right]));
    assert_eq!(g.credits_page, 1);
    assert_ne!(before, g.frame_state());
    tick(&mut g, keys(&[Key::Left]));
    assert_eq!(g.credits_page, 0);
    tick(&mut g, keys(&[Key::Left]));
    assert_eq!(g.credits_page, crate::credits::pages().len() - 1);
    tick(&mut g, keys(&[Key::Escape]));
    assert_eq!(g.mode, Mode::Controls);
}

#[test]
fn controller_can_open_page_and_leave_credits() {
    let mut g = game();
    g.mode = Mode::Controls;
    tick(&mut g, pads(&[Pad::A]));
    assert_eq!(g.mode, Mode::Credits);
    tick(&mut g, pads(&[Pad::DpadRight]));
    assert_eq!(g.credits_page, 1);
    tick(&mut g, pads(&[Pad::B]));
    assert_eq!(g.mode, Mode::Controls);
}

// --------------------------------------------------------------- shareware

/// A run one screen short of the shareware cap, played by the autopilot.
fn one_short_of_the_cap(tier: crate::tier::Tier) -> Game {
    let mut g = playing(777);
    g.tier = tier;
    let last = crate::tier::SHAREWARE_SCREENS - 1;
    g.jump_to(last);
    g.play.run.as_mut().unwrap().screens_cleared = last;
    g.dev.auto = true;
    g.take_effects();
    g
}

#[test]
fn a_shareware_run_ends_and_is_scored_at_the_cap() {
    let mut g = one_short_of_the_cap(crate::tier::Tier::Shareware);
    let fx = run_until(&mut g, 20_000, |g| g.mode == Mode::GameOver);
    let run = g.play.run.as_ref().unwrap();
    assert_eq!(run.screens_cleared, crate::tier::SHAREWARE_SCREENS);
    assert!(!run.is_over(), "the cap ends the run, not the shells");
    assert_eq!(count(&fx, is_score), 1);
}

#[test]
fn a_full_run_plays_past_the_shareware_cap() {
    let mut g = one_short_of_the_cap(crate::tier::Tier::Full);
    run_until(&mut g, 20_000, |g| {
        g.play.run.as_ref().unwrap().screens_cleared > crate::tier::SHAREWARE_SCREENS
    });
    assert_ne!(g.mode, Mode::GameOver);
}

#[test]
fn shareware_leaves_challenges_and_the_editor_out() {
    let mut g = game();
    g.tier = crate::tier::Tier::Shareware;
    g.start_challenge(0);
    assert_eq!(g.mode, Mode::Title);
    assert!(g.play.challenge.is_none());
    assert_eq!(g.status, crate::tier::FULL_GAME_ONLY);
    g.open_editor();
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn the_shareware_flag_parses() {
    assert!(DevArgs::parse(&["--salt-shareware".into()]).shareware);
    assert!(!DevArgs::parse(&[]).shareware);
}

//! The attract demo (`app/demo.rs`), stepped as the node steps it: how it
//! starts, what it plays, how it ends, and that it changes nothing the
//! player keeps. The suspended run's survival on disk is checked end to end
//! in `app/disk_tests.rs`.

use super::*;
use pretty_assertions::assert_eq;

use super::demo::{
    card_of, leg_of, CARD_DUEL, CARD_RUN, DEMO_CARD, DEMO_DUEL_TIME, DEMO_END_HOLD, DEMO_RUNS,
    DEMO_RUN_SCREENS, DEMO_RUN_TIME, IDLE_DEMO_AFTER, IDLE_DEMO_LIMIT, LOB,
};
use crate::input::{Event, Intents, Key, Pad};
use crate::resume::Saved;
use crate::touch::{Button, Rect};

/// The demo runs at `frame::IDLE_FPS`, so the tests step it at that rate.
const FRAME: f64 = 1.0 / 30.0;
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

fn finger(events: Vec<Event>) -> Intents {
    Intents { events, ..idle() }
}

fn game() -> Game {
    let mut g = Game::new(false, Save::default());
    g.fit_display(WINDOW, (0, 0));
    g
}

/// `seconds` of idle frames. Returns every effect.
fn wait(g: &mut Game, seconds: f64) -> Vec<Effect> {
    let n = (seconds / FRAME).round() as usize;
    (0..n).flat_map(|_| g.tick(FRAME, &idle())).collect()
}

/// Idle frames until `done`, or panic after `seconds`. Returns every effect.
fn wait_until(g: &mut Game, seconds: f64, done: impl Fn(&Game) -> bool) -> Vec<Effect> {
    let mut all = Vec::new();
    for _ in 0..(seconds / FRAME) as usize {
        if done(g) {
            return all;
        }
        all.extend(g.tick(FRAME, &idle()));
    }
    assert!(done(g), "not done after {seconds} s");
    all
}

fn leg_index(g: &Game) -> u64 {
    g.demo.as_ref().map_or(u64::MAX, |d| d.leg_index)
}

/// A title with a run quit to it: started, two shots fired, paused and
/// left, so CONTINUE offers it and `play.run` still holds it.
fn title_over_a_suspended_run() -> Game {
    let mut g = game();
    g.tick(FRAME, &keys(&[Key::Enter]));
    for _ in 0..2 {
        g.play.angle_ticks = 1400;
        g.play.power_ticks = 0;
        g.tick(FRAME, &keys(&[Key::Space]));
        wait_until(&mut g, 10.0, |g| {
            g.play.anim.is_none() && g.play.clear_hold <= 0.0
        });
    }
    g.tick(FRAME, &keys(&[Key::Escape]));
    g.tick(FRAME, &keys(&[Key::R]));
    assert_eq!(g.mode, Mode::Title);
    assert!(g.run_file.resumable.is_some());
    g.take_effects();
    g
}

/// Everything the player keeps in memory, as text: the save, the run kept
/// for CONTINUE and the one on screen, the run-file bookkeeping, the
/// versus choices, the crash report.
fn kept(g: &Game) -> String {
    format!(
        "{}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{}",
        g.save.serialize(),
        g.run_file.resumable.as_ref().map(Saved::to_line),
        g.play
            .run
            .as_ref()
            .map(|r| Saved::of(r, g.play.daily).to_line()),
        g.run_file.run_saved_len,
        g.run_file.unscored.len(),
        g.run_file.run_persist,
        g.run_file.resumed,
        (g.play.angle_ticks, g.play.power_ticks, g.play.last_seed),
        g.play.last_arc.len(),
        g.play.low_shell_warned,
        (g.versus.help, g.versus.rolling_off, g.versus.started),
        g.crash.last_crash,
        g.display.buffer_w,
        g.settings.dirty(),
    )
}

// ------------------------------------------------------------------ start

#[test]
fn the_idle_demo_starts_after_thirty_quiet_seconds_on_the_title() {
    let mut g = game();
    wait(&mut g, IDLE_DEMO_AFTER - 1.0);
    assert!(!g.in_demo());
    let fx = wait(&mut g, 1.5);
    assert!(g.in_demo());
    assert_eq!(g.mode, Mode::Playing);
    // Started by itself: nothing holds the screen on.
    assert!(!fx.iter().any(|e| matches!(e, Effect::ScreenAwake(_))));
    assert!(!g.screen_awake_wanted());
}

#[test]
fn any_input_on_the_title_starts_the_count_again() {
    let mut g = game();
    wait(&mut g, IDLE_DEMO_AFTER - 2.0);
    // A key the game does not read still counts as someone there.
    g.tick(
        FRAME,
        &Intents {
            events: vec![Event::Key {
                key: None,
                typed: None,
            }],
            ..idle()
        },
    );
    wait(&mut g, IDLE_DEMO_AFTER - 2.0);
    assert!(!g.in_demo());
    // So does a key held down, or a pad's stick.
    g.tick(
        FRAME,
        &Intents {
            keys_held: vec![Key::Shift],
            ..idle()
        },
    );
    g.tick(
        FRAME,
        &Intents {
            stick: [true, false, false, false],
            ..idle()
        },
    );
    wait(&mut g, IDLE_DEMO_AFTER - 2.0);
    assert!(!g.in_demo());
    wait(&mut g, 2.5);
    assert!(g.in_demo());
}

#[test]
fn the_idle_demo_comes_only_from_the_title() {
    for mode in [Mode::Controls, Mode::SeedEntry, Mode::VersusSetup] {
        let mut g = game();
        g.mode = mode;
        wait(&mut g, IDLE_DEMO_AFTER * 2.0);
        assert!(!g.in_demo(), "{mode:?}");
        assert_eq!(g.mode, mode);
    }
    // Nor from a paused run.
    let mut g = game();
    g.start_run(42, false);
    g.mode = Mode::Paused;
    wait(&mut g, IDLE_DEMO_AFTER * 2.0);
    assert!(!g.in_demo());
    assert_eq!(g.mode, Mode::Paused);
}

#[test]
fn the_idle_demo_can_be_turned_off_in_controls_and_is_saved() {
    let mut g = game();
    assert!(g.idle_demo_on(), "on by default, an old save included");
    g.tick(FRAME, &keys(&[Key::V]));
    assert_eq!(g.mode, Mode::Controls);
    g.tick(FRAME, &keys(&[Key::D]));
    assert_eq!(g.save.idle_demo, Some(false));
    let fx = g.tick(FRAME, &keys(&[Key::Escape]));
    assert!(fx.contains(&Effect::Store(Store::Settings)));
    assert!(g.save.serialize().contains("idle_demo 0\n"));
    wait(&mut g, IDLE_DEMO_AFTER * 2.0);
    assert!(!g.in_demo());
    // The title's DEMO still works with the idle demo off.
    g.tick(FRAME, &keys(&[Key::D]));
    assert!(g.in_demo());
}

#[test]
fn the_title_demo_item_starts_it_and_keeps_the_screen_on() {
    let mut g = game();
    let fx = g.tick(FRAME, &keys(&[Key::D]));
    assert!(g.in_demo());
    assert!(fx.contains(&Effect::ScreenAwake(true)));
    let fx = g.tick(FRAME, &keys(&[Key::Space]));
    assert!(!g.in_demo());
    assert!(fx.contains(&Effect::ScreenAwake(false)));
    // And by pad: d-pad down.
    let fx = g.tick(
        FRAME,
        &Intents {
            pads: vec![Pad::DpadDown],
            ..idle()
        },
    );
    assert!(g.in_demo());
    assert!(fx.contains(&Effect::ScreenAwake(true)));
}

#[test]
fn the_dev_flag_starts_the_demo_as_the_menu_does() {
    let mut g = game();
    let args = crate::cli::DevArgs::parse(&["--salt-demo".to_string()]);
    let fx = g.start_dev(&args, TODAY);
    assert!(g.in_demo());
    assert!(fx.contains(&Effect::ScreenAwake(true)));
}

// ------------------------------------------------------------------- loop

#[test]
fn the_demo_alternates_a_run_and_a_duel() {
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    let mut seen = Vec::new();
    for leg in 0..4u64 {
        wait_until(&mut g, 1.0, |g| leg_index(g) == leg);
        let what = match g.mode {
            Mode::Playing => {
                let run = g.play.run.as_ref().expect("a run leg has a run");
                format!("run {} at {}", run.run_seed, run.screen_index)
            }
            Mode::Versus => "duel".into(),
            other => format!("{other:?}"),
        };
        seen.push(what);
        assert_eq!(
            leg_of(leg) == DemoLeg::Run,
            g.mode == Mode::Playing,
            "leg {leg}"
        );
        // Each leg ends by itself, inside its time cap and a margin. A duel
        // ends on a result plate: a round won at least, hits and misses
        // both shown on the way.
        let mut duel_turns = 0;
        let mut duel_hits = 0;
        let mut seconds = 0.0;
        while leg_index(&g) == leg {
            if let Some(vs) = g.versus.live.as_ref() {
                duel_turns = duel_turns.max(vs.duel.turn);
                duel_hits = duel_hits
                    .max(vs.duel.hits.iter().sum::<u32>() + vs.duel.rounds.iter().sum::<u32>() * 3);
            }
            g.tick(FRAME, &idle());
            seconds += FRAME;
            assert!(seconds < DEMO_RUN_TIME + 30.0, "leg {leg} did not end");
        }
        if leg_of(leg) == DemoLeg::Duel {
            assert!(seconds > 10.0, "a duel of {seconds} s");
            assert!(duel_hits >= 3, "no round won: {duel_hits} hits");
            assert!(duel_turns > duel_hits.min(3), "no misses in the duel");
        } else {
            assert!(seconds > 5.0, "a run of {seconds} s");
        }
    }
    let run = |i: usize| {
        let (seed, screen) = DEMO_RUNS[i];
        format!("run {} at {screen}", parse_seed(seed))
    };
    assert_eq!(seen, [run(0), "duel".into(), run(1), "duel".into()]);
}

#[test]
fn every_run_leg_clears_its_screens_with_near_misses_and_moves_on() {
    for cycle in 0..DEMO_RUNS.len() as u64 {
        let mut g = game();
        g.start_demo(DemoStarted::Menu);
        g.begin_demo_leg(2 * cycle);
        let (mut shots, mut lobs, mut cleared) = (0, 0, 0);
        while leg_index(&g) == 2 * cycle {
            let before = g.play.run.as_ref().map_or(0, |r| r.log.len());
            g.tick(FRAME, &idle());
            let Some(run) = g.play.run.as_ref() else {
                continue;
            };
            if run.log.len() > before {
                let shot = run.log.last().unwrap();
                if shot.angle_ticks != salt_line_sim::replay::DRAFT_MARK {
                    shots += 1;
                    lobs += ((shot.angle_ticks, shot.power_ticks) == LOB) as u32;
                }
            }
            cleared = cleared.max(run.screens_cleared);
            if run.phase == Phase::Drafting {
                // The cards are taken within about two seconds.
                let mut up = 0.0;
                while g
                    .play
                    .run
                    .as_ref()
                    .is_some_and(|r| r.phase == Phase::Drafting)
                {
                    g.tick(FRAME, &idle());
                    up += FRAME;
                }
                assert!(up < 2.0, "cycle {cycle}: the draft stayed {up} s");
            }
            assert!(g.demo.as_ref().unwrap().leg_time < DEMO_RUN_TIME + 5.0);
        }
        assert!(
            cleared >= DEMO_RUN_SCREENS,
            "cycle {cycle}: cleared {cleared}"
        );
        // Misses on purpose, and near ones: more shots than screens, and
        // the lob kept for when no near miss can be found is rare.
        assert!(
            shots > cleared,
            "cycle {cycle}: {shots} shots, {cleared} screens"
        );
        assert!(
            lobs * 4 <= shots,
            "cycle {cycle}: {lobs} lobs in {shots} shots"
        );
    }
}

#[test]
fn the_demo_plays_the_same_way_each_time() {
    let trace = || {
        let mut g = game();
        g.start_demo(DemoStarted::Idle);
        wait(&mut g, 40.0);
        let run = g.play.run.as_ref().map(|r| (r.screen_index, r.log.len()));
        let duel = g
            .versus
            .live
            .as_ref()
            .map(|v| (v.duel.round, v.duel.turn, v.duel.hits));
        (leg_index(&g), run, duel)
    };
    assert_eq!(trace(), trace());
}

#[test]
fn a_demo_run_that_ends_is_not_scored_and_shows_no_result() {
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    // Out of shells on the next shot.
    g.play.run.as_mut().unwrap().shells = 1;
    let fx = wait_until(&mut g, 20.0, |g| leg_index(g) == 1);
    assert!(!fx.iter().any(Effect::touches_disk));
    assert!(g.save.scores.is_empty());
    assert_eq!(g.mode, Mode::Versus);
}

// ------------------------------------------------------------------- exit

#[test]
fn a_key_ends_the_demo_and_does_nothing_else() {
    for key in [Key::Enter, Key::Space, Key::Escape, Key::M, Key::N, Key::D] {
        let mut g = title_over_a_suspended_run();
        let before = kept(&g);
        g.start_demo(DemoStarted::Idle);
        wait(&mut g, 5.0);
        let fx = g.tick(FRAME, &keys(&[key]));
        assert!(!g.in_demo(), "{key:?}");
        assert_eq!(g.mode, Mode::Title, "{key:?}");
        // Not CONTINUE, not quit, not mute, not a new run, not the demo again.
        assert!(!fx.contains(&Effect::Quit), "{key:?}");
        let fx = wait(&mut g, 1.0);
        assert_eq!(g.mode, Mode::Title, "{key:?}");
        assert!(!fx.iter().any(Effect::touches_disk), "{key:?}");
        assert_eq!(kept(&g), before, "{key:?}");
    }
}

#[test]
fn a_pad_button_or_the_back_gesture_ends_the_demo() {
    for intents in [
        Intents {
            pads: vec![Pad::A],
            pads_held: vec![Pad::A],
            ..idle()
        },
        Intents {
            stick: [false, false, true, false],
            ..idle()
        },
        finger(vec![Event::Back]),
    ] {
        let mut g = title_over_a_suspended_run();
        g.start_demo(DemoStarted::Idle);
        wait(&mut g, 2.0);
        let fx = g.tick(FRAME, &intents);
        assert_eq!(g.mode, Mode::Title);
        assert!(
            !fx.contains(&Effect::Quit),
            "back from the demo is not quit"
        );
        assert!(g.run_file.resumable.is_some());
    }
}

#[test]
fn a_tap_ends_the_demo_and_presses_nothing_under_it() {
    let mut g = title_over_a_suspended_run();
    let before = kept(&g);
    g.start_demo(DemoStarted::Idle);
    wait(&mut g, 3.0);
    // The worst case: the whole screen is a CONTINUE button under the tap.
    let continue_button = [Button {
        rect: Rect::new(0, 0, 400, 216),
        action: Action::Confirm,
    }];
    g.set_buttons(&continue_button);
    let down = Event::FingerDown {
        index: 0,
        x: 160.0,
        y: 90.0,
    };
    g.tick(FRAME, &finger(vec![down]));
    assert_eq!(g.mode, Mode::Title);
    g.set_buttons(&continue_button);
    // Reported again while still down, then lifted: still nothing.
    g.tick(FRAME, &finger(vec![down]));
    g.tick(FRAME, &finger(vec![Event::FingerUp { index: 0 }]));
    wait(&mut g, 1.0);
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(kept(&g), before);
    // A fresh tap, once things settle, is the player's: CONTINUE.
    g.set_buttons(&continue_button);
    g.tick(FRAME, &finger(vec![down]));
    assert_eq!(g.mode, Mode::Playing);
    assert!(!g.in_demo());
}

#[test]
fn the_tap_that_starts_a_menu_demo_does_not_end_it() {
    let mut g = game();
    g.input.touch_active = true;
    let demo_plate = [Button {
        rect: Rect::new(0, 0, 50, 20),
        action: Action::Demo,
    }];
    g.set_buttons(&demo_plate);
    let down = Event::FingerDown {
        index: 0,
        x: 10.0,
        y: 10.0,
    };
    g.tick(FRAME, &finger(vec![down]));
    assert!(g.in_demo());
    // A device reporting the same press twice, then the lift.
    g.tick(FRAME, &finger(vec![down]));
    g.tick(FRAME, &finger(vec![Event::FingerUp { index: 0 }]));
    assert!(g.in_demo());
}

// --------------------------------------------------------------- isolation

#[test]
fn nothing_is_written_or_changed_during_a_demo_cycle() {
    // A fresh player (nothing learned), a crash report on offer, a run kept
    // for CONTINUE: everything the demo could disturb.
    let mut g = title_over_a_suspended_run();
    g.save.learned = 0;
    g.crash.last_crash = Some("SALT LINE CRASH REPORT".into());
    let before = kept(&g);
    // Chosen from the menu: an idle-started demo ends when focus goes.
    g.start_demo(DemoStarted::Menu);
    // A whole cycle, a run and a duel, and into the next run.
    let mut fx = wait_until(&mut g, 400.0, |g| leg_index(g) == 2);
    // Focus lost and the app paused mid-demo: no pause plate, no write.
    fx.extend(g.focus_lost(&[]));
    fx.extend(g.app_paused(&[]));
    fx.extend(wait(&mut g, 2.0));
    assert!(g.in_demo());
    assert_eq!(g.mode, Mode::Playing, "the demo does not pause");
    let disk: Vec<_> = fx.iter().filter(|e| e.touches_disk()).collect();
    assert!(disk.is_empty(), "the demo asked for {disk:?}");
    assert_eq!(g.save.learned, 0, "the demo taught nothing");
    g.tick(FRAME, &keys(&[Key::Space]));
    assert_eq!(kept(&g), before);
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn pending_settings_are_written_before_the_demo_not_during_it() {
    let mut g = game();
    g.settings.touch();
    let fx = g.tick(FRAME, &keys(&[Key::D]));
    assert!(g.in_demo());
    let at = |want: &dyn Fn(&Effect) -> bool| fx.iter().position(want).unwrap();
    assert!(
        // The demo's mute is its first effect once it counts as running.
        at(&|e| *e == Effect::Store(Store::Settings)) < at(&|e| matches!(e, Effect::Volume(_))),
        "flushed first: {fx:?}"
    );
    assert!(!g.settings.dirty());
}

#[test]
fn sound_is_muted_without_touching_the_volume() {
    let mut g = game();
    g.save.volume = 40;
    let playing = g.volume_db();
    let fx = g.tick(FRAME, &keys(&[Key::D]));
    assert!(fx.contains(&Effect::Volume(-80.0)));
    assert_eq!(g.volume_db(), -80.0);
    let fx = g.tick(FRAME, &keys(&[Key::Space]));
    assert!(fx.contains(&Effect::Volume(playing)));
    assert_eq!((g.save.volume, g.save.muted), (40, false));
    // The title music comes back with the title.
    assert!(fx.contains(&Effect::TitleMusic(true)));
}

#[test]
fn crash_reports_from_the_demo_say_so() {
    let mut g = game();
    g.start_demo(DemoStarted::Idle);
    assert_eq!(g.mode_name(), "Demo/Playing");
    wait_until(&mut g, DEMO_RUN_TIME + 30.0, |g| g.mode == Mode::Versus);
    assert_eq!(g.mode_name(), "Demo/Versus");
    g.stop_demo();
    assert_eq!(g.mode_name(), "Title");
}

#[test]
fn the_demo_draws_no_prompts_or_buttons_and_redraws_while_it_runs() {
    let mut g = game();
    g.input.touch_active = true;
    g.start_demo(DemoStarted::Idle);
    assert!(g.busy());
    assert_eq!(g.onboarding_prompt(Phase::Firing, Some(1.0)), None);
    let (canvas, buttons) = draw(&g);
    assert!(
        buttons.is_empty(),
        "{:?}",
        buttons.iter().map(|b| b.action).collect::<Vec<_>>()
    );
    assert!(!canvas
        .palette_text(&crate::palette::Palette::default())
        .contains('.'));
    wait_until(&mut g, DEMO_RUN_TIME + 30.0, |g| g.mode == Mode::Versus);
    for _ in 0..200 {
        let (_, buttons) = draw(&g);
        assert!(buttons.is_empty());
        wait(&mut g, 0.2);
    }
}

fn draw(g: &Game) -> (crate::raster::Canvas, Vec<Button>) {
    let mut canvas = crate::raster::Canvas::new(g.display.buffer_w, g.display.buffer_h);
    let mut buttons = Vec::new();
    crate::render::frame(
        g,
        &crate::render::Art::default(),
        &mut canvas,
        &mut buttons,
        &mut crate::render::Cache::default(),
    );
    (canvas, buttons)
}

// ------------------------------------------------------- review follow-ups

/// The demo's duel plates change the versus screen several times a turn.
/// Each change used to settle touches, which ignored a tap for 0.3 s: about
/// one tap in five went unheard. A tap on the frame after a plate change
/// must end the demo.
#[test]
fn a_tap_just_after_a_duel_plate_changes_ends_the_demo() {
    let screen = |g: &Game| g.versus.live.as_ref().map(|v| (v.stage, v.duel.turn));
    // The frames on which the duel's plates change, over its first turns.
    let mut g = game();
    g.start_demo(DemoStarted::Idle);
    let (mut frames, mut changes, mut last) = (0, Vec::new(), None);
    while changes.len() < 10 {
        g.tick(FRAME, &idle());
        frames += 1;
        let now = screen(&g);
        if now.is_some() && now != last {
            changes.push(frames);
        }
        last = now;
        assert!(frames < 10_000, "no duel");
    }
    // The same demo again to each of those frames, then a tap.
    for n in changes {
        let mut g = game();
        g.start_demo(DemoStarted::Idle);
        for _ in 0..n {
            g.tick(FRAME, &idle());
        }
        let at = screen(&g);
        g.tick(
            FRAME,
            &finger(vec![Event::FingerDown {
                index: 0,
                x: 100.0,
                y: 100.0,
            }]),
        );
        assert!(!g.in_demo(), "the tap after the change to {at:?} was spent");
        assert_eq!(g.mode, Mode::Title);
    }
}

#[test]
fn an_analog_trigger_ends_the_demo_and_counts_as_input() {
    let mut g = game();
    g.start_demo(DemoStarted::Idle);
    wait(&mut g, 2.0);
    g.tick(FRAME, &finger(vec![Event::Trigger]));
    assert!(!g.in_demo());
    assert_eq!(g.mode, Mode::Title);
    wait(&mut g, IDLE_DEMO_AFTER - 2.0);
    g.tick(FRAME, &finger(vec![Event::Trigger]));
    wait(&mut g, IDLE_DEMO_AFTER - 2.0);
    assert!(!g.in_demo(), "the trigger restarted the count");
}

#[test]
fn a_half_confirmed_new_run_is_forgotten_by_the_demo() {
    let mut g = title_over_a_suspended_run();
    // N once over a saved run: asks to press again.
    g.tick(FRAME, &keys(&[Key::N]));
    assert_eq!(g.run_file.discard_armed, Some("daily"));
    wait(&mut g, IDLE_DEMO_AFTER + 1.0);
    assert!(g.in_demo());
    assert_eq!(g.run_file.discard_armed, None);
    g.tick(FRAME, &keys(&[Key::Space]));
    // The next N asks again rather than throwing the run away.
    g.tick(FRAME, &keys(&[Key::N]));
    assert_eq!(g.mode, Mode::Title);
    assert!(g.run_file.resumable.is_some());
}

#[test]
fn the_rotation_carries_on_from_the_last_demo() {
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    assert_eq!(leg_index(&g), 0);
    g.tick(FRAME, &keys(&[Key::Space]));
    g.tick(FRAME, &keys(&[Key::D]));
    // The duel that would have come next.
    assert_eq!(leg_index(&g), 1);
    assert_eq!(g.mode, Mode::Versus);
    g.tick(FRAME, &keys(&[Key::Space]));
    g.tick(FRAME, &keys(&[Key::D]));
    let (seed, screen) = DEMO_RUNS[1];
    let run = g.play.run.as_ref().unwrap();
    assert_eq!((run.run_seed, run.screen_index), (parse_seed(seed), screen));
}

#[test]
fn an_unfocused_window_neither_starts_nor_keeps_an_idle_demo() {
    let mut g = game();
    g.focus_lost(&[]);
    wait(&mut g, IDLE_DEMO_AFTER * 2.0);
    assert!(!g.in_demo(), "nobody is looking");
    g.focus_gained();
    wait(&mut g, IDLE_DEMO_AFTER + 1.0);
    assert!(g.in_demo());
    // Focus goes again: the idle demo hands the title back, with the
    // sound. It never held the screen on, so there is nothing to let go.
    let fx = g.focus_lost(&[]);
    assert!(!g.in_demo());
    assert_eq!(g.mode, Mode::Title);
    assert!(!fx.iter().any(|e| matches!(e, Effect::ScreenAwake(_))));
    assert!(fx.contains(&Effect::Volume(g.volume_db())));
    // A demo chosen from the menu carries on.
    g.focus_gained();
    g.tick(FRAME, &keys(&[Key::D]));
    g.focus_lost(&[]);
    assert!(g.in_demo());
}

#[test]
fn an_idle_demo_gives_the_title_back_after_ten_minutes() {
    let mut g = game();
    wait(&mut g, IDLE_DEMO_AFTER + 1.0);
    assert!(g.in_demo());
    let fx = wait_until(&mut g, IDLE_DEMO_LIMIT + 5.0, |g| !g.in_demo());
    assert!(!fx.iter().any(Effect::touches_disk));
    assert_eq!(g.mode, Mode::Title);
    // And stays there until someone comes back.
    wait(&mut g, IDLE_DEMO_AFTER * 3.0);
    assert!(!g.in_demo());
    g.tick(FRAME, &keys(&[Key::Shift]));
    wait(&mut g, IDLE_DEMO_AFTER + 1.0);
    assert!(g.in_demo());
    // A demo chosen from the menu has no limit.
    let mut g = game();
    g.tick(FRAME, &keys(&[Key::D]));
    wait(&mut g, IDLE_DEMO_LIMIT + 30.0);
    assert!(g.in_demo());
}

#[test]
fn the_barrel_swings_to_each_aim_before_it_fires() {
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    let start = (g.play.angle_ticks, g.play.power_ticks);
    let mut seen = vec![start];
    while g.play.run.as_ref().is_some_and(|r| r.log.is_empty()) {
        g.tick(FRAME, &idle());
        let aim = (g.play.angle_ticks, g.play.power_ticks);
        if seen.last() != Some(&aim) {
            seen.push(aim);
        }
    }
    let fired = &g.play.run.as_ref().unwrap().log[0];
    assert_eq!(seen.last(), Some(&(fired.angle_ticks, fired.power_ticks)));
    assert!(seen.len() > 5, "snapped: {seen:?}");
}

// ------------------------------------------------------------- mode cards

/// Idle frames while the current leg's card is up; panics if the leg
/// changes first. Returns the frames counted.
fn frames_of_card(g: &mut Game) -> usize {
    let leg = leg_index(g);
    let mut frames = 0;
    while g.demo.as_ref().and_then(|d| d.card()).is_some() {
        assert_eq!(leg_index(g), leg);
        g.tick(FRAME, &idle());
        frames += 1;
        assert!(frames < 1000, "the card never went");
    }
    frames
}

#[test]
fn each_leg_opens_with_a_card_naming_its_mode() {
    assert_eq!(CARD_RUN.0, "ROGUELIKE");
    assert_eq!(CARD_DUEL.0, "TWO PLAYERS");
    assert!((1.5..=2.0).contains(&DEMO_CARD));
    for leg in 0..4u64 {
        let card = card_of(leg);
        let want = if leg_of(leg) == DemoLeg::Run {
            CARD_RUN
        } else {
            CARD_DUEL
        };
        assert_eq!((card.title, card.hook), want, "leg {leg}");
    }
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    for leg in 0..4u64 {
        // The card is up from the leg's first frame, the very first leg's
        // included, over the leg itself rather than a black screen.
        wait_until(&mut g, DEMO_RUN_TIME + 30.0, |g| leg_index(g) == leg);
        let d = g.demo.as_ref().unwrap();
        assert_eq!(d.card(), Some(card_of(leg)), "leg {leg}");
        let want = if leg_of(leg) == DemoLeg::Run {
            Mode::Playing
        } else {
            Mode::Versus
        };
        assert_eq!(g.mode, want, "leg {leg}: the card is over the leg");
        // Nothing plays under it: the aim holds, no shot is fired, the
        // duel's opening plate waits, and the leg's caps have not started.
        let aim = (g.play.angle_ticks, g.play.power_ticks);
        let shots = g.play.run.as_ref().map(|r| r.log.len());
        let stage = g.versus.live.as_ref().map(|v| (v.stage, v.duel.turn));
        let seconds = frames_of_card(&mut g) as f64 * FRAME;
        assert!(
            (seconds - DEMO_CARD).abs() <= FRAME * 1.5,
            "leg {leg}: the card stayed {seconds} s"
        );
        assert_eq!((g.play.angle_ticks, g.play.power_ticks), aim, "leg {leg}");
        assert_eq!(g.play.run.as_ref().map(|r| r.log.len()), shots);
        assert_eq!(
            g.versus.live.as_ref().map(|v| (v.stage, v.duel.turn)),
            stage
        );
        assert!(g.demo.as_ref().unwrap().leg_time < FRAME, "leg {leg}");
        assert!(g.in_demo());
    }
}

#[test]
fn a_demo_that_starts_on_a_duel_opens_with_its_card() {
    let mut g = game();
    g.demo_next_leg = 1;
    g.start_demo(DemoStarted::Idle);
    assert_eq!(g.mode, Mode::Versus);
    assert_eq!(g.demo.as_ref().unwrap().card(), Some(card_of(1)));
}

#[test]
fn input_during_a_card_ends_the_demo_as_anywhere_else() {
    let down = Event::FingerDown {
        index: 0,
        x: 160.0,
        y: 90.0,
    };
    // On the first leg's card, and on the duel's card mid-demo.
    for leg in [0u64, 1] {
        for input in [keys(&[Key::Enter]), finger(vec![down])] {
            let mut g = title_over_a_suspended_run();
            let before = kept(&g);
            g.start_demo(DemoStarted::Idle);
            // Past the start's own touch settling, so a tap is a new tap.
            wait_until(&mut g, DEMO_RUN_TIME + 30.0, |g| {
                leg_index(g) == leg && g.input.touch_settle <= 0.0
            });
            assert!(g.demo.as_ref().unwrap().card().is_some(), "leg {leg}");
            // The worst case: the whole screen a CONTINUE button under it.
            let continue_button = [Button {
                rect: Rect::new(0, 0, 400, 216),
                action: Action::Confirm,
            }];
            g.set_buttons(&continue_button);
            let fx = g.tick(FRAME, &input);
            assert!(!g.in_demo(), "leg {leg}");
            assert_eq!(g.mode, Mode::Title, "leg {leg}");
            assert!(!fx.iter().any(Effect::touches_disk));
            // Spent: a tap reported again while down, then its lift, press
            // nothing; the key is still held.
            g.set_buttons(&continue_button);
            if input.events.contains(&down) {
                g.tick(FRAME, &input);
                g.tick(FRAME, &finger(vec![Event::FingerUp { index: 0 }]));
            } else {
                g.tick(
                    FRAME,
                    &Intents {
                        keys_held: vec![Key::Enter],
                        ..idle()
                    },
                );
            }
            wait(&mut g, 1.0);
            assert_eq!(g.mode, Mode::Title, "leg {leg}");
            assert_eq!(kept(&g), before, "leg {leg}");
            // The next demo carries on from the leg after.
            assert_eq!(g.demo_next_leg, leg + 1);
        }
    }
}

/// With the card added, every leg over the first eight cycles still ends
/// by itself inside its cap: the card, then at most the cap of play, then
/// a run's end hold or the duel's last turn to the handoff. The caps count
/// play only, so they bind as they did before the card.
#[test]
fn the_legs_with_their_cards_stay_inside_their_caps() {
    let mut g = game();
    g.start_demo(DemoStarted::Menu);
    let (mut runs, mut duels) = (Vec::new(), Vec::new());
    for leg in 0..16u64 {
        assert_eq!(leg_index(&g), leg);
        let mut frames = 0usize;
        while leg_index(&g) == leg {
            g.tick(FRAME, &idle());
            frames += 1;
            assert!(frames < 10_000, "leg {leg} did not end");
        }
        let seconds = frames as f64 * FRAME;
        if leg_of(leg) == DemoLeg::Run {
            runs.push(seconds);
        } else {
            duels.push(seconds);
        }
    }
    let span = |v: &[f64]| {
        let lo = v.iter().copied().fold(f64::MAX, f64::min);
        let hi = v.iter().copied().fold(0.0, f64::max);
        (lo, hi)
    };
    let (runs, duels) = (span(&runs), span(&duels));
    eprintln!("legs with cards: runs {runs:.1?} s, duels {duels:.1?} s");
    // Four screens take well over the card, and end a run well inside its
    // cap.
    assert!(runs.0 > DEMO_CARD + 10.0, "runs {runs:?}");
    assert!(
        runs.1 < DEMO_CARD + DEMO_RUN_TIME * 0.5 + DEMO_END_HOLD,
        "runs {runs:?}"
    );
    // A duel plays its round; the longest may reach its cap and end at the
    // next handoff, a turn later.
    assert!(duels.0 > DEMO_CARD + 10.0, "duels {duels:?}");
    assert!(
        duels.1 < DEMO_CARD + DEMO_DUEL_TIME + 10.0,
        "duels {duels:?}"
    );
}

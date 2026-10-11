//! Holding the screen on (DESIGN §12.4), stepped as the node steps it: which
//! screens hold it, that each change is sent once and only once, and that
//! no pause, focus or demo path leaves it held.
//!
//! Every test drives a [`Phone`], which applies `Effect::ScreenAwake` as the
//! adapter does and checks, after every call into the core, that the screen
//! is in the state the core wants, and that no effect repeated the last one.

use super::*;
use pretty_assertions::assert_eq;

use crate::app::versus::Stage;
use crate::input::{Intents, Key};
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

/// The game and the screen as the adapter left it.
struct Phone {
    g: Game,
    /// The keep-on state last applied; off at launch, as `project.godot`
    /// has it.
    on: bool,
    /// `ScreenAwake` effects applied so far.
    sent: usize,
}

impl Phone {
    fn new() -> Phone {
        let mut g = Game::new(false, Save::default());
        g.fit_display(WINDOW, (0, 0));
        let mut phone = Phone {
            g,
            on: false,
            sent: 0,
        };
        let fx = phone.g.take_effects();
        phone.apply(fx);
        phone
    }

    /// Carry out `fx`'s screen effects, then check the screen is as wanted.
    fn apply(&mut self, fx: Vec<Effect>) {
        let screen: Vec<bool> = fx
            .iter()
            .filter_map(|e| match e {
                Effect::ScreenAwake(on) => Some(*on),
                _ => None,
            })
            .collect();
        assert!(screen.len() <= 1, "more than one screen effect: {fx:?}");
        for on in screen {
            assert_ne!(on, self.on, "a screen effect that changed nothing");
            self.on = on;
            self.sent += 1;
        }
        assert_eq!(self.on, self.g.screen_awake_wanted(), "screen out of step");
    }

    fn tick(&mut self, intents: Intents) {
        let fx = self.g.tick(FRAME, &intents);
        self.apply(fx);
    }

    fn press(&mut self, key: Key) {
        self.tick(keys(&[key]));
        self.tick(idle());
    }

    /// `seconds` of quiet frames, at the longest step a frame takes
    /// (`resume::frame_delta` caps it at a tenth of a second).
    fn wait(&mut self, seconds: f64) {
        for _ in 0..(seconds / 0.1).round() as usize {
            let fx = self.g.tick(0.1, &idle());
            self.apply(fx);
        }
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.tick(idle());
        }
    }

    fn until(&mut self, seconds: f64, done: impl Fn(&Game) -> bool) {
        for _ in 0..(seconds / FRAME) as usize {
            if done(&self.g) {
                return;
            }
            self.tick(idle());
        }
        assert!(done(&self.g), "not done after {seconds} s");
    }

    fn focus_lost(&mut self) {
        let fx = self.g.focus_lost(&[]);
        self.apply(fx);
    }

    fn app_paused(&mut self) {
        let fx = self.g.app_paused(&[]);
        self.apply(fx);
    }

    /// Focus back, as the node has it: no effects of its own, so the next
    /// frame settles the screen.
    fn focus_gained(&mut self) {
        self.g.focus_gained();
        self.tick(idle());
    }

    /// Whether the screen is held now, after a few quiet frames that must
    /// not change it.
    fn held(&mut self) -> bool {
        let sent = self.sent;
        self.frames(3);
        assert_eq!(self.sent, sent, "quiet frames sent a screen effect");
        self.on
    }

    fn vs(&self) -> &VersusPlay {
        self.g.versus.live.as_ref().expect("a match on screen")
    }

    fn stage(&self) -> Stage {
        self.vs().stage
    }
}

/// A shot the side to move can hit the other gun with.
fn hitting(g: &Game) -> Shot {
    let duel = &g.versus.live.as_ref().unwrap().duel;
    let side = duel.to_move;
    let frame = rules::frame_for(&duel.screen, side);
    let target = rules::target_in_frame(&duel.screen, side);
    rules::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)[0]
}

const MISS: Shot = Shot {
    angle_ticks: 1800,
    power_ticks: 0,
};

impl Phone {
    /// From a handoff plate: ready, aim `shot`, fire, and wait for the
    /// next plate.
    fn take_turn(&mut self, shot: Shot) {
        assert_eq!(self.stage(), Stage::Handoff);
        self.press(Key::Enter);
        self.fire(shot);
    }

    /// While aiming: aim `shot`, fire, and wait for the next plate.
    fn fire(&mut self, shot: Shot) {
        assert_eq!(self.stage(), Stage::Aiming);
        (self.g.play.angle_ticks, self.g.play.power_ticks) = (shot.angle_ticks, shot.power_ticks);
        self.press(Key::Space);
        assert_eq!(self.stage(), Stage::Flight);
        self.until(10.0, |g| {
            g.versus.live.as_ref().unwrap().stage != Stage::Flight
        });
    }
}

// ------------------------------------------------------------ by screen

#[test]
fn menus_let_the_screen_sleep_and_a_run_holds_it_only_while_live() {
    let mut p = Phone::new();
    assert_eq!(p.g.mode, Mode::Title);
    assert!(!p.held(), "the title");
    p.press(Key::V);
    assert_eq!(p.g.mode, Mode::Controls);
    assert!(!p.held(), "controls");
    p.press(Key::Escape);
    p.press(Key::S);
    assert_eq!(p.g.mode, Mode::SeedEntry);
    assert!(!p.held(), "seed entry");
    // Seed entry reads Escape as typed text.
    p.tick(Intents {
        events: vec![Event::Key {
            key: Some(Key::Escape),
            typed: Some(Typed::Escape),
        }],
        ..idle()
    });
    assert_eq!(p.g.mode, Mode::Title);
    assert_eq!(p.sent, 0, "nothing sent in the menus");

    // The daily: held while it plays, let go under the pause plate.
    p.press(Key::Enter);
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(p.held(), "live play");
    p.press(Key::Escape);
    assert_eq!(p.g.mode, Mode::Paused);
    assert!(!p.held(), "paused");
    p.press(Key::Enter);
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(p.held(), "resumed");
    // A shot in the air is still live play: nothing changes.
    let sent = p.sent;
    p.press(Key::Space);
    p.frames(120);
    assert_eq!(p.sent, sent);
    assert!(p.on);

    // The result screen lets go, and the title after it.
    p.g.finish_run();
    p.tick(idle());
    assert_eq!(p.g.mode, Mode::GameOver);
    assert!(!p.held(), "game over");
    p.press(Key::Enter);
    assert_eq!(p.g.mode, Mode::Title);
    assert!(!p.held());
    // Play, pause, resume, end: four changes, one effect each.
    assert_eq!(p.sent, 4);
}

#[test]
fn quitting_a_paused_run_to_the_title_stays_let_go() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.press(Key::Escape);
    p.press(Key::R);
    assert_eq!(p.g.mode, Mode::Title);
    assert!(!p.held());
    // CONTINUE holds it again.
    p.press(Key::Enter);
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(p.held());
}

#[test]
fn a_versus_match_holds_the_screen_through_its_plates_but_not_its_pause() {
    let mut p = Phone::new();
    p.press(Key::Num2);
    assert_eq!(p.g.mode, Mode::VersusSetup);
    assert!(!p.held(), "the aim-help choice");
    p.press(Key::Enter);
    assert_eq!(p.g.mode, Mode::Versus);
    assert_eq!(p.stage(), Stage::Handoff);
    assert!(p.held(), "the pass-the-phone plate");
    p.press(Key::Enter);
    assert_eq!(p.stage(), Stage::Aiming);
    assert!(p.held(), "aiming");

    // The versus pause lets go; resuming holds again.
    p.press(Key::Escape);
    assert!(p.vs().paused);
    assert!(!p.held(), "the versus pause");
    p.press(Key::Enter);
    assert!(!p.vs().paused);
    assert!(p.held());

    // Left wins the round; right never hits. Every plate on the way holds.
    p.press(Key::Escape);
    p.press(Key::Enter);
    p.fire(hitting(&p.g));
    while p.stage() != Stage::Result {
        let shot = if p.vs().side() == Side::Right {
            MISS
        } else {
            hitting(&p.g)
        };
        p.take_turn(shot);
        assert!(p.on, "between turns");
    }
    assert!(p.held(), "the result plate");
    // Paused on the result plate too.
    p.press(Key::Escape);
    assert!(!p.held());
    // Leaving the match from its pause (TITLE, pressed twice) goes back
    // to the title.
    p.press(Key::R);
    p.press(Key::R);
    assert_eq!(p.g.mode, Mode::Title);
    assert!(!p.held());
}

// ---------------------------------------------------------- quiet limit

#[test]
fn a_run_left_alone_lets_go_after_the_quiet_limit_and_input_holds_again() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    assert!(p.on);
    let sent = p.sent;
    p.wait(AWAKE_QUIET_LIMIT - 1.0);
    assert!(p.on, "still inside the limit");
    p.wait(2.0);
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(!p.on, "nobody has touched it for the limit");
    assert_eq!(p.sent, sent + 1, "one release");
    // Any input holds it again, a key the game ignores included.
    p.tick(Intents {
        events: vec![Event::Key {
            key: None,
            typed: None,
        }],
        ..idle()
    });
    assert!(p.on);
    assert_eq!(p.sent, sent + 2);
    // And the count starts again from that input.
    p.wait(AWAKE_QUIET_LIMIT - 1.0);
    assert!(p.on);
}

#[test]
fn a_held_key_or_stick_counts_as_someone_there() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    for _ in 0..((AWAKE_QUIET_LIMIT + 5.0) / 0.1) as usize {
        let fx = p.g.tick(
            0.1,
            &Intents {
                stick: [false, false, true, false],
                ..idle()
            },
        );
        p.apply(fx);
    }
    assert!(p.on);
}

#[test]
fn a_versus_plate_left_alone_lets_go_too() {
    let mut p = Phone::new();
    p.press(Key::Num2);
    p.press(Key::Enter);
    assert_eq!(p.stage(), Stage::Handoff);
    p.wait(AWAKE_QUIET_LIMIT + 1.0);
    assert!(!p.on);
    p.press(Key::Enter);
    assert_eq!(p.stage(), Stage::Aiming);
    assert!(p.on);
}

#[test]
fn entering_a_screen_that_holds_starts_the_count_afresh() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.wait(AWAKE_QUIET_LIMIT - 10.0);
    // Paused and resumed: a fresh count, not the old one carried on.
    p.press(Key::Escape);
    assert_eq!(p.g.input_quiet, 0.0);
    p.press(Key::Enter);
    p.wait(AWAKE_QUIET_LIMIT - 1.0);
    assert!(p.on);
}

#[test]
fn a_menu_demo_has_the_longer_limit() {
    let mut p = Phone::new();
    p.press(Key::D);
    assert!(p.on);
    p.wait(AWAKE_QUIET_LIMIT + 10.0);
    assert!(p.g.in_demo());
    assert!(p.on, "a demo is watched without input");
    p.wait(DEMO_AWAKE_LIMIT - AWAKE_QUIET_LIMIT - 10.0);
    assert!(p.g.in_demo(), "the demo plays on");
    assert!(!p.on, "but lets the screen sleep");
    assert_eq!(p.sent, 2);
}

#[test]
fn the_dev_autopilot_is_exempt_from_the_quiet_limit() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.g.dev.auto = true;
    p.g.input_quiet = AWAKE_QUIET_LIMIT + 60.0;
    assert!(p.g.screen_awake_wanted(), "--salt-auto and profiling runs");
    p.g.dev.auto = false;
    p.g.dev.capturing = true;
    assert!(p.g.screen_awake_wanted(), "a --salt-shot capture");
    p.g.dev.capturing = false;
    assert!(!p.g.screen_awake_wanted());
    // Stepped, the autopilot plays on untouched, and whenever it is in
    // live play the screen is held, well past the limit.
    p.g.input_quiet = 0.0;
    p.g.dev.auto = true;
    let mut live = 0;
    for _ in 0..((AWAKE_QUIET_LIMIT + 10.0) / 0.1) as usize {
        let fx = p.g.tick(0.1, &idle());
        p.apply(fx);
        if p.g.live_play() {
            live += 1;
            assert!(p.on);
        }
    }
    assert!(p.g.input_quiet > AWAKE_QUIET_LIMIT);
    assert!(live > 0);
}

// ------------------------------------------------------------ the demo

#[test]
fn an_idle_demo_never_holds_the_screen_even_in_its_duel() {
    let mut p = Phone::new();
    p.until(demo::IDLE_DEMO_AFTER + 1.0, |g| g.in_demo());
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(!p.held(), "a run leg, started by itself");
    p.until(demo::DEMO_RUN_TIME + 5.0, |g| g.mode == Mode::Versus);
    assert!(p.g.in_demo());
    assert!(!p.held(), "a duel leg, started by itself");
    p.press(Key::Space);
    assert!(!p.g.in_demo());
    assert!(!p.held());
    assert_eq!(p.sent, 0);
}

#[test]
fn a_menu_demo_holds_the_screen_once_for_all_its_legs() {
    let mut p = Phone::new();
    p.press(Key::D);
    assert!(p.g.in_demo());
    assert!(p.held(), "chosen to be watched");
    assert_eq!(p.sent, 1);
    // Into the duel and back to a run: the legs change modes, the screen
    // stays as it is.
    p.until(demo::DEMO_RUN_TIME + 5.0, |g| g.mode == Mode::Versus);
    p.until(demo::DEMO_DUEL_TIME + 5.0, |g| g.mode == Mode::Playing);
    assert!(p.g.in_demo());
    assert_eq!(p.sent, 1);
    // Any key ends it at the title, which lets go.
    p.press(Key::Space);
    assert!(!p.g.in_demo());
    assert_eq!(p.g.mode, Mode::Title);
    assert!(!p.held());
    assert_eq!(p.sent, 2);
}

#[test]
fn the_dev_flag_demo_holds_the_screen_as_the_menu_does() {
    let mut p = Phone::new();
    let args = crate::cli::DevArgs::parse(&["--salt-demo".to_string()]);
    let fx = p.g.start_dev(&args, 20_000);
    p.apply(fx);
    assert!(p.g.in_demo());
    assert!(p.on);
}

// --------------------------------------------- focus, app pause, crash

#[test]
fn focus_loss_and_app_pause_let_go_of_a_run() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    assert!(p.on);
    // Focus goes: the run pauses, and the effect comes with the
    // notification, not a frame later.
    p.focus_lost();
    assert_eq!(p.g.mode, Mode::Paused);
    assert!(!p.on);
    p.focus_gained();
    assert!(!p.held(), "still paused after focus comes back");
    p.press(Key::Enter);
    assert!(p.held());
    // The platform pauses the app with no focus event first.
    p.app_paused();
    assert_eq!(p.g.mode, Mode::Paused);
    assert!(!p.on);
    p.focus_gained();
    assert!(!p.held());
}

#[test]
fn focus_loss_lets_go_of_screens_a_pause_does_not_freeze() {
    // A draft is not paused by focus loss, but nobody is looking.
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.g.play.run.as_mut().unwrap().phase = Phase::Drafting;
    p.focus_lost();
    assert_eq!(p.g.mode, Mode::Playing);
    assert!(!p.on, "unfocused");
    p.focus_gained();
    assert!(p.held(), "back to the draft");

    // Nor is a versus plate.
    let mut p = Phone::new();
    p.press(Key::Num2);
    p.press(Key::Enter);
    assert_eq!(p.stage(), Stage::Handoff);
    p.app_paused();
    assert!(!p.vs().paused);
    assert!(!p.on, "backgrounded");
    p.focus_gained();
    assert!(p.held(), "back to the plate");

    // A versus turn being aimed pauses, and stays let go.
    p.press(Key::Enter);
    assert_eq!(p.stage(), Stage::Aiming);
    p.focus_lost();
    assert!(p.vs().paused);
    assert!(!p.on);
    p.focus_gained();
    assert!(!p.held());
}

#[test]
fn focus_coming_back_sends_nothing_itself_and_the_next_frame_holds() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.g.play.run.as_mut().unwrap().phase = Phase::Drafting;
    p.focus_lost();
    assert!(!p.on);
    p.g.focus_gained();
    assert!(
        !p.g.fx.iter().any(|e| matches!(e, Effect::ScreenAwake(_))),
        "nothing queued by focus_gained"
    );
    assert!(!p.on, "not yet");
    let fx = p.g.tick(FRAME, &idle());
    assert_eq!(
        fx.iter()
            .filter(|e| matches!(e, Effect::ScreenAwake(_)))
            .collect::<Vec<_>>(),
        [&Effect::ScreenAwake(true)]
    );
    p.apply(fx);
    assert!(p.on);
}

#[test]
fn a_menu_demo_lets_go_while_unfocused_and_holds_again_after() {
    let mut p = Phone::new();
    p.press(Key::D);
    assert!(p.on);
    p.focus_lost();
    assert!(p.g.in_demo(), "a menu demo carries on");
    assert!(!p.on);
    p.focus_gained();
    assert!(p.held());
}

#[test]
fn the_error_screen_lets_go_even_if_the_last_release_was_lost() {
    // Out of play the core has already sent a release, but a panic in the
    // adapter may have skipped it: the error screen sends it again.
    let mut p = Phone::new();
    p.press(Key::Enter);
    p.g.finish_run();
    let fx = p.g.take_effects();
    assert!(fx.contains(&Effect::ScreenAwake(false)));
    // (That list died with the adapter; the real screen is still held.)
    let fx = p.g.broken_tick(FRAME, &idle(), || None);
    assert!(fx.contains(&Effect::ScreenAwake(false)));
    // Once only.
    let fx = p.g.broken_tick(FRAME, &idle(), || None);
    assert!(!fx.iter().any(|e| matches!(e, Effect::ScreenAwake(_))));
}

#[test]
fn the_error_screen_lets_go() {
    let mut p = Phone::new();
    p.press(Key::Enter);
    assert!(p.on);
    let fx = p.g.broken_tick(FRAME, &idle(), || None);
    p.apply(fx);
    assert!(!p.on);
}

//! The attract demo: the game playing itself on the title's behalf, the way
//! an arcade cabinet does between players (DESIGN §12).
//!
//! # The loop
//!
//! ```text
//!  title ──30 s no input, IDLE DEMO on──┐
//!  title ──DEMO (D, d-pad down, plate)──┤
//!                                       ▼
//!          leg 0: run ──► leg 1: duel ──► leg 2: run ──► leg 3: duel ──► …
//!                                       │
//!   any key, tap, pad button, trigger, stick or back ──► title (input spent)
//! ```
//!
//! A *run leg* starts one of [`DEMO_RUNS`] (a seed and a screen picked for
//! how they look) and plays until [`DEMO_RUN_SCREENS`] screens are cleared,
//! the run ends, or [`DEMO_RUN_TIME`] passes. A *duel leg* plays a local
//! versus match on its own seed until [`DEMO_DUEL_ROUNDS`] round is won or
//! [`DEMO_DUEL_TIME`] passes. Each leg opens with a card naming its mode
//! ([`card_of`]), held [`DEMO_CARD`] over the leg's first frame before
//! anything moves; the caps count from the card's end. The legs carry on
//! from one demo to the next in a session (`Game::demo_next_leg`). Both aim
//! with the solver, as `--salt-auto` does, and miss on purpose now and
//! then, so craters, chalk and landing marks and rolling guns all get
//! shown. Which shots miss is a hash of the demo cycle (two legs) and the
//! shot's place in it, so a given cycle always plays the same way
//! ([`demo_hash`]).
//!
//! # Isolation: set aside, never touched
//!
//! The demo drives the ordinary run and versus code, so it looks exactly
//! like the game. What it must not do is change anything the player keeps
//! (the suspended run and CONTINUE, the run files, the suspended versus
//! match and its file, the board, the learned lessons, the settings, crash
//! reports). Two rules make that so:
//!
//! 1. **Set aside whole.** On entry the player's [`Play`] (which holds the
//!    run kept for CONTINUE), the versus state and the buffer width are
//!    moved into [`Demo`]'s stash with `mem::take`, and moved back on exit.
//!    Nothing reads or writes them in between; `run_file` and
//!    `saved_match` (the CONTINUE and RESUME MATCH offers themselves) are
//!    never touched at all.
//! 2. **No write is asked for.** Every place that would ask for a disk
//!    effect checks [`Game::in_demo`] first: `persist_run`, `finish_run`
//!    (a demo run is not scored), `persist_match` and `drop_live_match`
//!    (the demo's duels are never marked kept in the first place),
//!    `learn`, `write_settings` and `tick_settings`, `retire_last_crash`.
//!    Pending settings are flushed as the demo starts, before it counts as
//!    running. `Game::tick` keeps a backstop that drops any disk effect a
//!    demo frame asks for, and `debug_assert!`s that there was none, so the
//!    tests fail if a guard is ever missed.
//!
//! `app/demo_tests.rs` checks both (and `app/disk_tests.rs` the same for a
//! suspended versus match): a run suspended to the title survives a
//! demo cycle byte for byte, on disk and in memory, and no disk effect is
//! asked for while the demo runs.
//!
//! # Exit, sound, power
//!
//! Any new input ends the demo on the frame it arrives and is spent there:
//! no action is read that frame, key and pad presses are dropped at the
//! next frame's `Controls::begin`, and touches are settled
//! (`settle_touch`) so the tap that ended the demo cannot press a title
//! button. Only the demo's start settles touches: its duel's plate changes
//! do not (`Game::tick`), or a tap just after one would be ignored. An
//! idle-started demo also ends when focus goes or the app pauses, and after
//! [`IDLE_DEMO_LIMIT`]. Sound is muted by `volume_db` while the demo runs
//! ([`DEMO_MUTED`]), which leaves the saved volume and mute alone. A demo
//! started from the menu holds the screen on; one that started by itself
//! does not, so the phone sleeps as it would on the title. Nothing here asks
//! for that: `Game::screen_awake_wanted` reads [`Started`] each frame. The
//! demo runs at `frame::IDLE_FPS` (30).

use super::versus::Stage;
use super::*;
use crate::versus;
use salt_line_sim::rng::mix64;

/// Seconds the title waits without input before the idle demo starts.
pub const IDLE_DEMO_AFTER: f64 = 30.0;
/// The idle demo until the player turns it off in Controls.
pub const DEFAULT_IDLE_DEMO: bool = true;
/// The demo plays silent. The player's volume is not touched: see
/// `Game::volume_db`.
pub const DEMO_MUTED: bool = true;

/// The run legs, in turn: a seed (as seed entry reads it) and the screen
/// to start on (zero-based, as `--salt-screen` counts; the HUD shows one
/// more), each picked from screenshots for a striking first screen.
/// Night skies, a water tower over a chasm, two targets on a mesa, spires.
pub const DEMO_RUNS: [(&str, u32); 4] = [("DUNE", 7), ("TOWER", 12), ("1984", 8), ("MESA", 16)];
/// Screens a run leg clears before the duel.
pub const DEMO_RUN_SCREENS: u32 = 4;
/// Longest a run leg plays, in seconds, should its screens go slowly.
pub const DEMO_RUN_TIME: f64 = 75.0;
/// Rounds a duel leg plays at most (a match is best of three): one, so
/// the duel is a taste of versus rather than most of the loop.
pub const DEMO_DUEL_ROUNDS: u32 = 1;
/// Longest a duel leg plays, in seconds; it ends at the next handoff.
pub const DEMO_DUEL_TIME: f64 = 40.0;
/// Longest an idle-started demo runs before it gives the title back, in
/// seconds. It does not start again by itself until someone touches
/// something. A demo chosen from the menu runs until it is stopped.
pub const IDLE_DEMO_LIMIT: f64 = 600.0;

/// Seconds the new aim is shown, barrel and readouts, before it fires. The
/// barrel swings to it over the first [`DEMO_SWING`] of that.
pub const DEMO_AIM: f64 = 0.8;
/// Seconds the barrel and readouts take to swing from the last aim to the
/// new one; the rest of [`DEMO_AIM`] holds it still, so it is read.
pub const DEMO_SWING: f64 = 0.55;
/// Seconds the draft cards stay up before an offer is taken.
pub const DEMO_DRAFT: f64 = 1.6;
/// Seconds the pass-the-phone plate stays up.
pub const DEMO_HANDOFF: f64 = 0.8;
/// Seconds the round or match result plate stays up.
pub const DEMO_RESULT: f64 = 1.8;
/// Seconds a run leg's last screen is held before the duel.
pub const DEMO_END_HOLD: f64 = 1.2;
/// Seconds the card naming the mode stays over a leg's first frame before
/// the leg starts to play (DESIGN §12.2). Not counted in the leg's
/// [`DEMO_RUN_TIME`] or [`DEMO_DUEL_TIME`].
pub const DEMO_CARD: f64 = 1.8;

/// The card before a run leg: the mode, and what a run asks of you
/// (DESIGN §1: clear each screen's targets; a draft every band).
pub const CARD_RUN: (&str, &str) = ("ROGUELIKE", "TOPPLE TARGETS \u{b7} DRAFT A BUILD");
/// The card before a duel leg: the mode, and how it is played (DESIGN §11:
/// two players pass one phone, a shot each).
pub const CARD_DUEL: (&str, &str) = ("TWO PLAYERS", "ONE PHONE \u{b7} TAKE TURNS");

/// What the card before a leg says: the mode, and one line on it. A run's
/// seed name is left off: it is a third line that pushed the card down onto
/// the tallest curated targets, and a word a first-time viewer cannot read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Card {
    /// The mode, in plain UI text.
    pub title: &'static str,
    /// One line on what it is.
    pub hook: &'static str,
}

/// The card before leg `index`.
pub fn card_of(index: u64) -> Card {
    let (title, hook) = match leg_of(index) {
        Leg::Run => CARD_RUN,
        Leg::Duel => CARD_DUEL,
    };
    Card { title, hook }
}

/// Mixed into duel seeds and miss hashes, so the demo's matches are not
/// any player's.
const DEMO_SALT: u64 = 0x5A17_D3E0_0DE0_0001;
/// A high lob that lands short, well clear of any target: the miss when no
/// near one can be found.
pub(super) const LOB: (i32, i32) = (1400, 330);
/// Changes to the solver's aim tried, in turn, for a near miss: (angle,
/// power) ticks, power first. A flat, fast shot topples a structure over a
/// wide spread of power, so steeper and flatter barrels follow.
const NEAR_MISS: [(i32, i32); 16] = [
    (0, 45),
    (0, -45),
    (0, 75),
    (0, -75),
    (0, 120),
    (0, -120),
    (0, 170),
    (0, -170),
    (170, 0),
    (-170, 0),
    (240, 0),
    (-240, 0),
    (320, 0),
    (-320, 0),
    (420, 0),
    (-420, 0),
];

/// The near misses to try around `aim`, the opposite way first when `flip`
/// is -1, so the hash decides between over and short.
fn near_misses(aim: Shot, flip: i32) -> impl Iterator<Item = Shot> {
    NEAR_MISS.into_iter().map(move |(da, dp)| {
        Shot::new(
            (aim.angle_ticks + flip * da).clamp(0, dmath::MAX_ANGLE_TICKS),
            (aim.power_ticks + flip * dp).clamp(0, POWER_TICKS_MAX),
        )
    })
}

/// What the demo is playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leg {
    Run,
    Duel,
}

/// How the demo began, which decides whether it holds the screen on
/// (`Game::screen_awake_wanted`), whether it ends when focus goes, and
/// whether it has a time limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Started {
    /// The title sat without input for [`IDLE_DEMO_AFTER`].
    Idle,
    /// Asked for: the title's DEMO, or `--salt-demo`.
    Menu,
}

/// The leg at `index`: runs on even legs, duels on odd ones.
pub fn leg_of(index: u64) -> Leg {
    if index.is_multiple_of(2) {
        Leg::Run
    } else {
        Leg::Duel
    }
}

/// The deterministic choice behind a demo shot: the same cycle, screen or
/// round, and shot always hash alike.
pub fn demo_hash(cycle: u64, a: u64, b: u64) -> u64 {
    mix64(DEMO_SALT ^ mix64(cycle) ^ mix64(a.rotate_left(32) ^ b))
}

/// What the driver last waited on: a change starts its hold afresh.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cue {
    /// Something is moving (a shell, a clear): nothing to do.
    Busy,
    Draft,
    Aim,
    Handoff,
    Result,
}

/// The player's state, moved out whole while the demo runs.
struct Stash {
    play: Play,
    versus_live: Option<VersusPlay>,
    help: [bool; 2],
    rolling_off: bool,
    ground: Option<GroundPick>,
    buffer_w: i32,
    kept_width: bool,
}

/// An aim chosen and swinging into place, waiting [`DEMO_AIM`] to fire.
#[derive(Clone, Copy)]
struct Pending {
    /// The aim it swings from (angle ticks, power ticks).
    from: (i32, i32),
    /// The shot that fires.
    to: Shot,
    /// The fuse a run shot sets.
    fuse: Option<f64>,
}

/// The demo while it runs. See the module docs.
pub(crate) struct Demo {
    pub(crate) started: Started,
    /// Seconds since the demo began, for [`IDLE_DEMO_LIMIT`].
    pub(crate) time: f64,
    /// The leg on screen, counted from 0; [`leg_of`] says which kind.
    pub(crate) leg_index: u64,
    /// Seconds into the leg's play, from the end of its card.
    pub(crate) leg_time: f64,
    /// Seconds the leg's card has left on screen; nothing plays until it
    /// runs out ([`DEMO_CARD`]).
    pub(crate) card: f64,
    /// Seconds before the driver acts on its current cue.
    wait: f64,
    cue: Option<Cue>,
    pending: Option<Pending>,
    /// The leg is over; seconds before the next one begins.
    ending: Option<f64>,
    stash: Stash,
}

impl Demo {
    pub(crate) fn leg(&self) -> Leg {
        leg_of(self.leg_index)
    }

    /// The card on screen, while it is.
    pub(crate) fn card(&self) -> Option<Card> {
        (self.card > 0.0).then(|| card_of(self.leg_index))
    }

    /// Two legs, a run and a duel, make a cycle.
    pub(crate) fn cycle(&self) -> u64 {
        self.leg_index / 2
    }

    /// Wait on `cue`: on a new cue the wait restarts at `hold`. True once
    /// it has run out.
    fn ready(&mut self, cue: Cue, hold: f64) -> bool {
        if self.cue != Some(cue) {
            self.cue = Some(cue);
            self.wait = hold;
        }
        self.wait <= 0.0
    }
}

impl Game {
    /// The attract demo is running.
    pub(crate) fn in_demo(&self) -> bool {
        self.demo.is_some()
    }

    /// The title starts the demo by itself after a spell without input.
    pub(crate) fn idle_demo_on(&self) -> bool {
        self.save.idle_demo.unwrap_or(DEFAULT_IDLE_DEMO)
    }

    /// Whether this frame brought any input, or a finger rests on the
    /// screen: a key, a touch, a pad button or trigger, a held key or pad
    /// button, the stick. Read by the idle demo's count and the screen's
    /// quiet limit (`Game::input_quiet`).
    pub(super) fn any_input(&self, intents: &Intents) -> bool {
        !intents.events.is_empty()
            || !intents.pads.is_empty()
            || !intents.keys_held.is_empty()
            || !intents.pads_held.is_empty()
            || intents.stick.iter().any(|&s| s)
            || self.input.touch.active()
    }

    /// Count the title's quiet time, and start the idle demo once it
    /// reaches [`IDLE_DEMO_AFTER`]. Any input, a finger resting on the
    /// screen, or any other screen starts the count again: the idle demo
    /// comes only from the title.
    ///
    /// Not while the window is out of focus (a desktop with another window
    /// on top: nobody is looking), and not again after an idle demo ran
    /// its [`IDLE_DEMO_LIMIT`] out, until someone gives some input.
    pub(super) fn watch_title_idle(&mut self, delta: f64, input: bool) {
        if input {
            self.idle_demo_spent = false;
        }
        let quiet = self.mode == Mode::Title
            && !self.in_demo()
            && !self.crash.broken
            && !input
            && !self.unfocused
            && !self.idle_demo_spent;
        if !quiet {
            self.title_idle = 0.0;
            return;
        }
        self.title_idle += delta;
        // Not under the dev autopilot, which leaves the title at once
        // anyway, nor while a screenshot waits to be taken.
        let dev = self.dev.auto || self.dev.capturing;
        if self.title_idle >= IDLE_DEMO_AFTER && self.idle_demo_on() && !dev {
            self.start_demo(Started::Idle);
        }
    }

    /// Focus lost or the app paused: an idle-started demo has nobody
    /// watching it, so it gives the title back. A demo chosen from the menu
    /// carries on (it is paused with the app anyway).
    pub(super) fn demo_unwatched(&mut self) {
        if self
            .demo
            .as_ref()
            .is_some_and(|d| d.started == Started::Idle)
        {
            self.stop_demo();
        }
    }

    /// Whether this frame's input ends the demo: a key (any key, read by
    /// the game or not), a pad button, an analog trigger, the stick, the
    /// back gesture, or a finger going down. A finger that goes down while
    /// touches settle is the one that started a menu demo, reported again,
    /// not a new tap: the demo's own screen changes (a duel's plates) do
    /// not settle touches (`Game::tick`), so only its start does.
    pub(super) fn demo_interrupted(&self, intents: &Intents) -> bool {
        let settling = self.input.touch_settle > 0.0;
        intents.events.iter().any(|e| match e {
            Event::Key { .. } | Event::Back | Event::Trigger => true,
            Event::FingerDown { .. } => !settling,
            Event::FingerMoved { .. } | Event::FingerUp { .. } => false,
        }) || !intents.pads.is_empty()
            || intents.stick.iter().any(|&s| s)
    }

    /// Start the demo: flush pending settings while writes are still
    /// allowed, set the player's state aside, mute, and begin the first run
    /// leg.
    pub(crate) fn start_demo(&mut self, started: Started) {
        if self.in_demo() {
            return;
        }
        self.flush_settings();
        self.title_idle = 0.0;
        self.status_time = 0.0;
        // A half-confirmed new run on the title is forgotten, as leaving
        // the title forgets it: the press after the demo asks again.
        self.run_file.discard_armed = None;
        let stash = Stash {
            play: std::mem::take(&mut self.play),
            versus_live: self.versus.live.take(),
            help: self.versus.help,
            rolling_off: self.versus.rolling_off,
            ground: self.versus.ground.take(),
            buffer_w: self.display.buffer_w,
            kept_width: self.display.kept_width,
        };
        // Aim help on, so the duel shows its landing marks and arcs; the
        // guns roll, as in a player's match by default.
        self.versus.help = [true; 2];
        self.versus.rolling_off = false;
        // The rotation carries on from the last demo this session, so a
        // second look is not the same run again.
        let first = self.demo_next_leg;
        self.demo = Some(Demo {
            started,
            time: 0.0,
            leg_index: first,
            leg_time: 0.0,
            card: 0.0,
            wait: 0.0,
            cue: None,
            pending: None,
            ending: None,
            stash,
        });
        self.settle_touch();
        self.apply_volume();
        self.begin_demo_leg(first);
    }

    /// End the demo and put everything back as it was: the player's run
    /// and versus state, the buffer, the volume. Back
    /// at the title, with touches settled so the ending tap presses nothing.
    pub(crate) fn stop_demo(&mut self) {
        let Some(demo) = self.demo.take() else {
            return;
        };
        self.demo_next_leg = demo.leg_index + 1;
        let s = demo.stash;
        self.play = s.play;
        self.versus.live = s.versus_live;
        self.versus.help = s.help;
        self.versus.rolling_off = s.rolling_off;
        self.versus.ground = s.ground;
        self.display.buffer_w = s.buffer_w;
        self.display.kept_width = s.kept_width;
        self.mode = Mode::Title;
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.settle_touch();
        self.title_idle = 0.0;
        self.apply_volume();
        self.apply_buffer_size();
        self.fx.push(Effect::Redraw);
    }

    /// Begin leg `index`: a run on the next of [`DEMO_RUNS`], or a duel on
    /// the cycle's own seed, under its card.
    pub(super) fn begin_demo_leg(&mut self, index: u64) {
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        d.leg_index = index;
        d.leg_time = 0.0;
        d.card = DEMO_CARD;
        d.wait = 0.0;
        d.cue = None;
        d.pending = None;
        d.ending = None;
        let cycle = d.cycle();
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        match leg_of(index) {
            Leg::Run => {
                let (seed, screen) = DEMO_RUNS[(cycle % DEMO_RUNS.len() as u64) as usize];
                self.start_demo_run(parse_seed(seed), screen);
            }
            Leg::Duel => {
                self.play = Play::default();
                self.start_versus_on(mix64(DEMO_SALT ^ cycle), versus::Rules::DEFAULT);
            }
        }
    }

    /// A run for the demo, as `start_run` starts one but kept off disk and
    /// away from the crash report, and dropped straight onto `screen`, as
    /// `--salt-screen` does (each screen generates from its own seed).
    fn start_demo_run(&mut self, seed: u64, screen: u32) {
        self.versus.live = None;
        self.play = Play::default();
        self.set_run_width(None);
        let width = self.display.buffer_w as usize;
        let mut state = RunState::new(seed, width);
        if screen > 0 {
            state.screen_index = screen;
            state.screen = run::generate_screen(seed, screen, width, &state.loadout);
        }
        self.play.daily = false;
        self.play.last_seed = seed;
        self.play.run = Some(state);
        self.mode = Mode::Playing;
        self.new_preview();
        self.apply_buffer_size();
    }

    /// One frame of the demo's play, in place of the player's input. While
    /// a leg's card is up nothing is driven, so the leg holds its first
    /// frame under it; the leg's time and waits start when the card goes.
    pub(super) fn demo_step(&mut self, delta: f64) {
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        d.time += delta;
        if d.started == Started::Idle && d.time >= IDLE_DEMO_LIMIT {
            self.stop_demo();
            self.idle_demo_spent = true;
            return;
        }
        if let Some(t) = d.ending.as_mut() {
            *t -= delta;
            if *t <= 0.0 {
                let next = d.leg_index + 1;
                self.begin_demo_leg(next);
            }
            return;
        }
        if d.card > 0.0 {
            d.card -= delta;
            return;
        }
        d.leg_time += delta;
        d.wait -= delta;
        match d.leg() {
            Leg::Run => self.demo_run_step(),
            Leg::Duel => self.demo_duel_step(),
        }
    }

    fn demo_run_step(&mut self) {
        let Some(run) = self.play.run.as_ref() else {
            return;
        };
        let (phase, over, cleared) = (run.phase, run.is_over(), run.screens_cleared);
        let offers = run.offers.len();
        let screen = run.screen_index as u64;
        let busy = self.play.anim.is_some() || self.play.clear_hold > 0.0;
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        let cycle = d.cycle();
        let done = over
            || (cleared >= DEMO_RUN_SCREENS && phase == Phase::Firing)
            || d.leg_time >= DEMO_RUN_TIME;
        if busy {
            d.ready(Cue::Busy, 0.0);
            return;
        }
        if done {
            d.ending = Some(DEMO_END_HOLD);
            return;
        }
        match phase {
            Phase::Drafting => {
                if !d.ready(Cue::Draft, DEMO_DRAFT) || offers == 0 {
                    return;
                }
                d.cue = None;
                let pick = ((cycle + screen) % offers as u64) as usize;
                if self
                    .play
                    .run
                    .as_mut()
                    .and_then(|r| r.take_offer(pick))
                    .is_some()
                {
                    self.new_preview();
                    self.play(Sound::Draft);
                    self.apply_buffer_size();
                }
            }
            Phase::Cleared => self.play.clear_hold = CLEAR_HOLD,
            Phase::Firing => {
                if d.pending.is_none() {
                    if !d.ready(Cue::Aim, 0.0) {
                        return;
                    }
                    let (shot, fuse) = self.demo_run_shot(cycle);
                    self.aim_demo(shot, fuse);
                } else if let Some(p) = self.swing_demo_aim() {
                    if let (Some(f), Some(run)) = (p.fuse, self.play.run.as_mut()) {
                        run.set_fuse(f);
                    }
                    self.fire();
                }
            }
            Phase::Over(_) => {}
        }
    }

    /// Choose `shot` as the next aim: the barrel swings to it, then it
    /// fires ([`DEMO_AIM`]).
    fn aim_demo(&mut self, shot: Shot, fuse: Option<f64>) {
        let from = (self.play.angle_ticks, self.play.power_ticks);
        if let Some(d) = self.demo.as_mut() {
            d.pending = Some(Pending {
                from,
                to: shot,
                fuse,
            });
            d.wait = DEMO_AIM;
        }
    }

    /// Swing the aim toward the pending shot, eased in and out, so the
    /// barrel and the readouts move as a hand would move them. Returns the
    /// shot, with the aim exactly on it, once [`DEMO_AIM`] is up.
    fn swing_demo_aim(&mut self) -> Option<Pending> {
        let d = self.demo.as_mut()?;
        let p = d.pending?;
        let to = (p.to.angle_ticks, p.to.power_ticks);
        if d.wait > 0.0 {
            let t = ((DEMO_AIM - d.wait) / DEMO_SWING).clamp(0.0, 1.0);
            let e = t * t * (3.0 - 2.0 * t);
            let lerp = |a: i32, b: i32| a + ((b - a) as f64 * e).round() as i32;
            (self.play.angle_ticks, self.play.power_ticks) =
                (lerp(p.from.0, to.0), lerp(p.from.1, to.1));
            return None;
        }
        d.pending = None;
        d.cue = None;
        (self.play.angle_ticks, self.play.power_ticks) = to;
        Some(p)
    }

    /// The run leg's next shot and fuse: the solver's, or a deliberate near
    /// miss. A screen opens with a miss two times in three and its second
    /// shot misses one time in four; never a third, so screens keep moving.
    fn demo_run_shot(&self, cycle: u64) -> (Shot, Option<f64>) {
        let lob = (Shot::new(LOB.0, LOB.1), None);
        let Some(run) = self.play.run.as_ref() else {
            return lob;
        };
        let Some((aim, target)) = self.solver_shot() else {
            return lob;
        };
        let fuse = run.screen.default_fuse(target);
        let h = demo_hash(cycle, run.screen_index as u64, run.shots_on_screen as u64);
        let miss = match run.shots_on_screen {
            0 => !h.is_multiple_of(3),
            1 => h.is_multiple_of(4),
            _ => false,
        };
        if !miss {
            return (aim, fuse);
        }
        // Over first or short first, by the hash: a shot that lands near
        // the target and leaves it standing, tried on a copy of the run.
        let flip = if (h >> 8) & 1 == 0 { 1 } else { -1 };
        for shot in near_misses(aim, flip) {
            let mut trial = run.clone();
            if let Some(f) = fuse {
                trial.set_fuse(f);
            }
            if trial.fire(shot, false).is_some()
                && trial
                    .screen
                    .structures
                    .get(target)
                    .is_some_and(|s| !s.is_down())
            {
                return (shot, fuse);
            }
        }
        (lob.0, fuse)
    }

    fn demo_duel_step(&mut self) {
        let Some(vs) = self.versus.live.as_ref() else {
            return;
        };
        let (stage, phase, round) = (vs.stage, vs.duel.phase, vs.duel.round);
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        let cycle = d.cycle();
        if stage == Stage::Handoff && d.leg_time >= DEMO_DUEL_TIME {
            d.ending = Some(0.0);
            return;
        }
        match stage {
            Stage::Handoff => {
                if d.ready(Cue::Handoff, DEMO_HANDOFF) {
                    d.cue = None;
                    self.begin_versus_turn();
                }
            }
            Stage::Aiming => {
                if d.pending.is_none() {
                    if !d.ready(Cue::Aim, 0.0) {
                        return;
                    }
                    let shot = self.demo_duel_shot(cycle);
                    self.aim_demo(shot, None);
                } else if self.swing_demo_aim().is_some() {
                    self.versus_fire();
                }
            }
            Stage::Flight => {
                d.ready(Cue::Busy, 0.0);
            }
            Stage::Result => {
                if !d.ready(Cue::Result, DEMO_RESULT) {
                    return;
                }
                d.cue = None;
                let match_over = matches!(phase, versus::Phase::MatchOver(_));
                if match_over || round + 1 >= DEMO_DUEL_ROUNDS {
                    d.ending = Some(0.0);
                } else {
                    self.after_versus_result();
                }
            }
        }
    }

    /// The duel leg's next shot: one that hits, or two times in five a
    /// near miss, tried on a copy of the match so it really misses.
    fn demo_duel_shot(&self, cycle: u64) -> Shot {
        let lob = Shot::new(LOB.0, LOB.1);
        let Some(vs) = self.versus.live.as_ref() else {
            return lob;
        };
        opponent_shot(&vs.duel, cycle)
    }
}

/// Shared local opponent policy: solve the current field, with deterministic near misses.
pub(super) fn opponent_shot(duel: &versus::Match, salt: u64) -> Shot {
    let lob = Shot::new(LOB.0, LOB.1);
    let side = duel.to_move;
    let frame = versus::frame_for(&duel.screen, side);
    let target = versus::target_in_frame(&duel.screen, side);
    let Some(aim) = versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
        .first()
        .copied()
    else {
        return lob;
    };
    let h = demo_hash(salt ^ DEMO_SALT, duel.round as u64, duel.turn as u64);
    if h % 5 >= 2 {
        return aim;
    }
    let flip = if (h >> 8) & 1 == 0 { 1 } else { -1 };
    for shot in near_misses(aim, flip) {
        let mut trial = duel.clone();
        if let Some(report) = trial.fire(shot, false) {
            if !report.hit && !report.self_hit {
                return shot;
            }
        }
    }
    lob
}

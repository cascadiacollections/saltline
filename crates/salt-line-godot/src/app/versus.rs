//! A local versus match in play: the pre-match screen, passing the phone,
//! aiming, the shot, and the round and match results. The rules are
//! `crate::versus`; this is the pacing, the same split `play.rs` makes for a
//! run.
//!
//! # A turn
//!
//! ```text
//!  Handoff ──ready──► Aiming ──fire──► Flight ──shell down, HIT/MISS held──┐
//!     ▲                                                                    │
//!     └──────────── match still in a round ◄───────────────────────────────┤
//!                                                                          ▼
//!                                  Result (round or match won) ──next──► Handoff
//!                                                     │
//!                                       match won ──rematch──► pre-match screen
//! ```
//!
//! The handoff plate covers the field between turns, so the player taking
//! the phone sees nothing of the last player's aim until they say they are
//! ready, and the readouts show only while the player whose turn it is
//! aims. Each player's aim is kept for their next turn.
//!
//! As in a run, the shot is resolved the moment it is fired and the flight
//! is only played back ([`Anim`]); the renderer shows the screen from
//! before the shot until the shell lands.
//!
//! Nothing here reaches the board, the run file or the onboarding lessons,
//! and a single-player run set aside for CONTINUE is left exactly as it
//! was. The match itself is kept on disk, as its seed, options and shots
//! (`match_log`, DESIGN §11.6), so a match the operating system killed can
//! be resumed from the title.
//!
//! # The match file
//!
//! ```text
//!  start_versus ─► kept; persist_match writes the line on every shot
//!                    │
//!   app killed ──────┤ kept; launch offers RESUME MATCH ─► resume_match
//!   match won ───────┤ cleared the frame the winning shot is fired
//!   quit (pause or ──┘ cleared (leave_versus); a rematch starts a new one
//!    result: TITLE)
//! ```
//!
//! A live turn and a rebuild take the same steps ([`VersusPlay::fire_turn`],
//! [`VersusPlay::next_round`]), so a resumed match is the match as played:
//! the rules' state, each side's kept aim and last arc, and the roll the
//! handoff plate announces. The demo's matches are never kept
//! ([`VersusPlay::kept`] is only set by [`Game::start_versus`] and
//! [`Game::resume_match`]), and `persist_match` and `drop_live_match` check
//! for the demo as well (DESIGN §12.3).
//!
//! # Easing a roll
//!
//! The guns' roll is resolved with the shot (the rules move them at once,
//! and the next shot fires from the new spots). What the players see eases
//! it: the guns are drawn at their old spots while the handoff plate covers
//! the field, then roll to the new ones over [`ROLL_EASE`] once the player
//! taking the phone is ready ([`RollEase`]). Only the drawn pivot moves;
//! the terrain under it, the crater-filled footprints included, is already
//! the new one, and aim help (the landing mark, the last arc, the preview)
//! appears once the guns have arrived.
//!
//! # One touch, one screen
//!
//! Touch presses are not taken the way key presses are, and the plates
//! between turns put their buttons where the last screen's were: READY sits
//! over START, and the open field a drag aims from sits under both. A tap a
//! device reports more than once (a repeated down for a finger already down,
//! a copy in other coordinates) could therefore start the match, ready the
//! turn and drag-fire a shot, all from one press. So whenever the versus
//! screen changes (the mode, the stage or the pause plate), every finger
//! down is spent until it lifts and new touches are spent for
//! [`TOUCH_SETTLE`]: one tap acts on one screen only. Keys and pad buttons
//! are taken per press already and are not held back.

use super::*;
use crate::match_log::{self, Entry, SavedMatch};
use crate::versus::{self, Match, Side};

/// Seconds the HIT or MISS is held on the landed shot before the phone is
/// passed on.
pub const VERSUS_HOLD: f64 = 1.1;

/// Seconds the guns take to roll to their new spots on screen, once the
/// field is shown after a roll. Presentation only: see "Easing a roll".
pub const ROLL_EASE: f64 = 0.45;

/// A roll of the guns being eased on screen. See "Easing a roll".
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RollEase {
    /// How far each gun's pivot moved, by [`Side::index`]
    /// ([`versus::Roll::moved`]).
    pub moved: [Vec2; 2],
    /// Seconds into the ease. `None` while the handoff plate still covers
    /// the field, the guns held where they were.
    pub t: Option<f64>,
}

impl RollEase {
    /// The share of the move still to go: 1 until the ease starts, then
    /// down to 0 at [`ROLL_EASE`], slow at both ends (smoothstep), as a
    /// heavy gun starts and stops.
    pub fn remaining(&self) -> f64 {
        let Some(t) = self.t else {
            return 1.0;
        };
        let u = (t / ROLL_EASE).clamp(0.0, 1.0);
        1.0 - u * u * (3.0 - 2.0 * u)
    }
}

/// Seconds after the shell lands that the hit's ding plays: just after the
/// impact and collapse, so it is heard on its own rather than under them.
pub const DING_DELAY: f64 = 0.08;

/// What the landed shot is called out as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Call {
    /// The shell reached the other gun: a point to the shooter.
    Hit,
    /// It reached neither gun.
    Miss,
    /// It reached only the shooter's own gun: no point for anyone, but not
    /// a MISS either, which would be a strange thing to be told.
    Oops,
}

/// Seconds a versus screen ignores new touches after it appears. Longer
/// than a tap (about 0.12 s on a phone), so whatever a device reports for
/// the tap that changed the screen is spent rather than taken again.
pub const TOUCH_SETTLE: f64 = 0.3;

/// Where a versus turn is. See the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    /// The plate over the field: pass the phone, then ready.
    Handoff,
    /// The player whose turn it is aims and fires.
    Aiming,
    /// The shot is being played back.
    Flight,
    /// A round or the match has been won; the plate says who.
    Result,
}

/// What a versus frame shows that input or a step can change, for
/// `FrameState`: the stage, round, turn, the hits shown and the pause.
pub(crate) type Fingerprint = (Stage, u32, u32, [u32; 2], bool);

/// The versus mode's own state, beside the run's in `Game`.
#[derive(Default)]
pub(crate) struct VersusState {
    /// Aim help for each side, as the pre-match screen has it: the last
    /// landing mark, the last arc and a short preview of the current aim.
    /// Off for both by default.
    pub(crate) help: [bool; 2],
    /// ROLLING GUNS OFF on the pre-match screen: the guns stay where they
    /// are all match ([`versus::GUN_SHIFT`] off). On by default, and kept
    /// for a rematch, so the rule can be tried both ways.
    pub(crate) rolling_off: bool,
    /// Matches started this session, so each gets its own seed.
    pub(crate) started: u64,
    /// Different at every launch (the adapter reads the clock), so the
    /// first match of one launch is not the first match of the last.
    pub(crate) entropy: u64,
    /// The match on screen, if one is.
    pub(crate) live: Option<VersusPlay>,
}

/// A match on screen and how it is being shown.
pub(crate) struct VersusPlay {
    /// The match, as the rules have it: already past any shot being shown.
    pub(crate) duel: Match,
    pub(crate) stage: Stage,
    /// The last shot, while it is being played back.
    pub(crate) anim: Option<Anim>,
    /// Each side's aim (angle ticks, power ticks), kept between its turns.
    pub(crate) aims: [(i32, i32); 2],
    /// Each side's last shell path this round, in the shared frame, for
    /// aim help.
    pub(crate) arcs: [Vec<Vec2>; 2],
    /// The gun the last shot hit, while the hit is being shown.
    pub(crate) hit: Option<Side>,
    /// The last shot reached only its own gun, while it is being shown.
    pub(crate) self_hit: bool,
    /// Who fired the shot being shown, for the callout.
    pub(crate) shooter: Side,
    /// The hit's ding has played ([`DING_DELAY`]).
    pub(crate) dinged: bool,
    /// Hits and rounds as they stood before the shot in the air, shown
    /// until it lands: the rules score a shot the moment it is fired, and
    /// a score that changed in mid-flight would give the result away.
    pub(crate) score_before: Option<([u32; 2], [u32; 2])>,
    /// Under the pause plate.
    pub(crate) paused: bool,
    /// When the last shot rolled the guns ([`versus::Match::roll_guns`]):
    /// the screen as the shot left it ([`versus::Roll::landed`]), craters
    /// dug and guns still where they were, shown while the HIT or MISS is
    /// held. They are found at their new spots when the phone has been
    /// passed.
    pub(crate) landed: Option<Screen>,
    /// Which way the guns rolled after the last shot, if they did: the
    /// handoff plate says so. Cleared by the next shot.
    pub(crate) rolled: Option<versus::RollWay>,
    /// The last roll, while it is still to be eased on screen.
    pub(crate) ease: Option<RollEase>,
    /// Every shot fired and round dealt this match, in order: the match's
    /// log (`match_log`).
    pub(crate) log: Vec<Entry>,
    /// A player's match, kept on disk for RESUME MATCH. Never the demo's.
    pub(crate) kept: bool,
    /// Entries in the line last written; `None` before the first write.
    pub(crate) saved_len: Option<usize>,
}

impl VersusPlay {
    /// `duel` on screen from its start: the first player's handoff plate,
    /// both aims in the middle of the dial, nothing kept on disk yet.
    pub(crate) fn new(duel: Match) -> VersusPlay {
        VersusPlay {
            duel,
            stage: Stage::Handoff,
            anim: None,
            aims: [(900, 600); 2],
            arcs: [Vec::new(), Vec::new()],
            hit: None,
            self_hit: false,
            shooter: Side::Left,
            dinged: false,
            score_before: None,
            paused: false,
            landed: None,
            rolled: None,
            ease: None,
            log: Vec::new(),
            kept: false,
            saved_len: None,
        }
    }

    /// Fire the side to move's `aim` and keep what the screens show of it:
    /// the aim for that side's next turn, its arc (moved with its gun if
    /// the guns rolled), the roll to announce and ease, and the shot in the
    /// log. A live shot and a rebuild both come through here. `None` if the
    /// rules take no shot now.
    pub(crate) fn fire_turn(&mut self, aim: (i32, i32)) -> Option<versus::TurnReport> {
        let report = self.duel.fire(Shot::new(aim.0, aim.1), true)?;
        self.log.push(Entry::Shot(aim.0, aim.1));
        let i = report.shooter.index();
        self.aims[i] = aim;
        self.arcs[i] = report
            .traces
            .first()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.shooter = report.shooter;
        // Each side's last arc moves with its gun, as the rules move its
        // landing mark, so aim help stays attached to the gun that fired.
        if let Some(roll) = &report.rolled {
            for (arc, d) in self.arcs.iter_mut().zip(roll.moved) {
                for p in arc.iter_mut() {
                    *p = Vec2::new(p.x + d.x, p.y + d.y);
                }
            }
        }
        self.rolled = report.rolled.as_ref().map(versus::Roll::way);
        self.ease = report.rolled.as_ref().map(|roll| RollEase {
            moved: roll.moved,
            t: None,
        });
        Some(report)
    }

    /// Deal the next round after a round won, clear the last round's arcs,
    /// and log it. False unless a round has just ended.
    pub(crate) fn next_round(&mut self) -> bool {
        if !self.duel.next_round() {
            return false;
        }
        self.arcs = [Vec::new(), Vec::new()];
        self.log.push(Entry::NextRound);
        true
    }

    /// The match `saved` logged, rebuilt by taking its entries again
    /// through [`VersusPlay::fire_turn`] and [`VersusPlay::next_round`]. It
    /// lands on the pass-the-phone plate for whoever is next, or on the
    /// result plate if the last shot won a round. `None` for a log that
    /// does not replay, or one whose last shot won the match: nothing is
    /// left to resume.
    pub(crate) fn rebuild(saved: &SavedMatch) -> Option<VersusPlay> {
        let duel = Match::with_rules(saved.seed, saved.width, saved.rules());
        let mut vs = VersusPlay::new(duel);
        for entry in &saved.log {
            let taken = match *entry {
                Entry::Shot(angle, power) => vs.fire_turn((angle, power)).is_some(),
                Entry::NextRound => vs.next_round(),
            };
            if !taken {
                return None;
            }
        }
        vs.stage = match vs.duel.phase {
            versus::Phase::Aiming => Stage::Handoff,
            versus::Phase::RoundOver(_) => Stage::Result,
            versus::Phase::MatchOver(_) => return None,
        };
        Some(vs)
    }

    /// How far `gun` is drawn from where it stands: back along its last
    /// roll, by the share of it still to be eased. Zero while a shot is
    /// being shown, which draws the screen from before the roll.
    pub(crate) fn gun_offset(&self, gun: Side) -> Vec2 {
        match (&self.anim, &self.ease) {
            (None, Some(ease)) => {
                let d = ease.moved[gun.index()];
                let k = ease.remaining();
                Vec2::new(d.x * k, d.y * k)
            }
            _ => Vec2::new(0.0, 0.0),
        }
    }

    /// Where `gun`'s pivot is drawn: where it stands on the screen shown,
    /// less what is left of its roll ([`VersusPlay::gun_offset`]).
    pub(crate) fn shown_muzzle(&self, gun: Side) -> Vec2 {
        let m = versus::muzzle_of(self.shown_screen(), gun);
        let d = self.gun_offset(gun);
        Vec2::new(m.x - d.x, m.y - d.y)
    }

    /// The guns are rolling on screen (an ease begun and not finished).
    pub(crate) fn easing(&self) -> bool {
        self.anim.is_none() && self.ease.is_some_and(|e| e.t.is_some())
    }

    /// Whose gun the readouts, the controls and the aim belong to.
    pub(crate) fn side(&self) -> Side {
        self.duel.to_move
    }

    /// The screen to draw: the pre-shot copy while a shot is in the air, so
    /// the crater appears when the shell lands, and the guns where they
    /// were until the turn passes.
    pub(crate) fn shown_screen(&self) -> &Screen {
        match self.anim.as_ref() {
            Some(anim) if !anim.done() => &anim.before,
            Some(_) => self.landed.as_ref().unwrap_or(&self.duel.screen),
            None => &self.duel.screen,
        }
    }

    /// Hits and rounds to show: as they were until the shell lands.
    pub(crate) fn shown_score(&self) -> ([u32; 2], [u32; 2]) {
        self.score_before
            .unwrap_or((self.duel.hits, self.duel.rounds))
    }

    /// What to call out, and who fired: from the moment the shell lands
    /// until the phone is passed on ([`VERSUS_HOLD`]). The renderer draws
    /// it large: a hit in the shooter's colour, a miss or an oops plain.
    pub(crate) fn callout(&self) -> Option<(Side, Call)> {
        let landed = self.anim.as_ref().is_some_and(Anim::done);
        let call = if self.hit.is_some() {
            Call::Hit
        } else if self.self_hit {
            Call::Oops
        } else {
            Call::Miss
        };
        (self.stage == Stage::Flight && landed).then_some((self.shooter, call))
    }

    /// The wind to show: the wind the shot in the air was fired into, until
    /// the turn passes; the rules have already drawn the next turn's.
    pub(crate) fn shown_wind(&self) -> f64 {
        self.anim
            .as_ref()
            .map_or(self.duel.screen.wind, |anim| anim.before.wind)
    }
}

impl VersusState {
    /// The rules a match started now plays by: the game's, with the guns'
    /// roll off if the pre-match screen says so.
    pub(crate) fn rules(&self) -> versus::Rules {
        match_log::rules_for(!self.rolling_off)
    }
}

impl Game {
    /// Something that differs at every launch, for match seeds. The core
    /// reads no clock; the adapter passes the wall clock once at launch.
    pub fn set_versus_entropy(&mut self, entropy: u64) {
        self.versus.entropy = entropy;
    }

    /// Open the pre-match screen.
    pub(crate) fn open_versus_setup(&mut self) {
        self.drop_live_match();
        self.mode = Mode::VersusSetup;
    }

    /// Start a match at the buffer's width, the left player to ready up
    /// first. It is kept on disk from its first frame (`persist_match`),
    /// replacing any match suspended before it.
    pub(crate) fn start_versus(&mut self) {
        self.versus.started += 1;
        // A seed of its own for each match: the launch's entropy, the date
        // and a count. Everything after it is deterministic from the seed.
        let seed = salt_line_sim::rng::mix64(
            self.versus.entropy
                ^ salt_line_sim::rng::mix64(daily_seed(self.today) ^ self.versus.started),
        );
        self.start_versus_on(seed, self.versus.rules());
        if let Some(vs) = self.versus.live.as_mut() {
            vs.kept = true;
        }
        self.saved_match = None;
    }

    /// Start a match on `seed` by `rules` at the buffer's width: a player's
    /// match, or the demo's (`app/demo.rs`). Not kept on disk: a player's
    /// match is marked kept by [`Game::start_versus`].
    pub(super) fn start_versus_on(&mut self, seed: u64, rules: versus::Rules) {
        let width = self.display.buffer_w as usize;
        self.versus.live = Some(VersusPlay::new(Match::with_rules(seed, width, rules)));
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.mode = Mode::Versus;
        // Long Lens belongs to a run; a duel is drawn at the usual height.
        self.apply_buffer_size();
    }

    /// Leave versus for the title. The match is dropped, and with it its
    /// line on disk: the players quit it.
    pub(crate) fn leave_versus(&mut self) {
        self.drop_live_match();
        self.input.touch.cancel_drag();
        self.mode = Mode::Title;
        self.apply_buffer_size();
    }

    /// Drop the match on screen, and clear its line if it was kept. A
    /// match the demo set aside is not on screen, and the demo's own are
    /// never kept, but the demo is checked too (DESIGN §12.3).
    fn drop_live_match(&mut self) {
        let Some(vs) = self.versus.live.take() else {
            return;
        };
        if vs.kept && !self.in_demo() {
            self.fx.push(Effect::MatchFile(MatchFile::Discard));
        }
    }

    /// Write the match's line whenever a shot has been fired, and once at
    /// its start; clear it the frame the match is won. Checked every frame,
    /// written only on a change, never during the demo.
    pub(super) fn persist_match(&mut self) {
        if self.in_demo() {
            return;
        }
        let help = self.versus.help;
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if !vs.kept {
            return;
        }
        if matches!(vs.duel.phase, versus::Phase::MatchOver(_)) {
            // A finished match leaves nothing to resume.
            vs.kept = false;
            self.fx.push(Effect::MatchFile(MatchFile::Discard));
            return;
        }
        let len = vs.log.len();
        if vs.saved_len == Some(len) {
            return;
        }
        // Tried once per entry: a failed write is logged by `save::write`,
        // and the next shot tries again.
        vs.saved_len = Some(len);
        let line = SavedMatch::of(&vs.duel, help, &vs.log).to_line();
        self.fx.push(Effect::MatchFile(MatchFile::Write(line)));
    }

    /// RESUME MATCH: rebuild the suspended match and put it on the
    /// pass-the-phone plate for whoever is next, with its aim help and
    /// rolling guns as they were chosen for it. A match generated at
    /// another width keeps it, letterboxed, as a resumed run does. A log
    /// that no longer rebuilds is cleared with a note.
    pub(crate) fn resume_match(&mut self) {
        let Some(saved) = self.saved_match.take() else {
            return;
        };
        let Some(mut vs) = VersusPlay::rebuild(&saved) else {
            self.fx.push(Effect::MatchFile(MatchFile::Discard));
            self.note(match_log::note(resume::Stale::Diverged).into());
            return;
        };
        vs.kept = true;
        // What is on disk is this match, as rebuilt.
        vs.saved_len = Some(vs.log.len());
        self.versus.help = saved.help;
        self.versus.rolling_off = !saved.rolling;
        self.versus.live = Some(vs);
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.set_run_width(Some(saved.width));
        self.mode = Mode::Versus;
        self.apply_buffer_size();
    }

    /// The pre-match screen: each side's aim help, rolling guns, start,
    /// back.
    pub(super) fn handle_versus_setup_input(&mut self) {
        if self.action_pressed(Action::Up) {
            self.versus.rolling_off = !self.versus.rolling_off;
            self.play(Sound::Ui);
        }
        for (action, side) in [(Action::Left, Side::Left), (Action::Right, Side::Right)] {
            if self.action_pressed(action) {
                let i = side.index();
                self.versus.help[i] = !self.versus.help[i];
                self.play(Sound::Ui);
            }
        }
        if self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter) {
            self.play(Sound::Ui);
            self.start_versus();
        } else if self.action_pressed(Action::Back) {
            self.play(Sound::Ui);
            self.leave_versus();
        }
    }

    /// A match in play. As in a run, nothing but pause is read while a shot
    /// is in the air.
    pub(super) fn handle_versus_input(&mut self, delta: f64) {
        let Some(vs) = self.versus.live.as_ref() else {
            self.mode = Mode::Title;
            return;
        };
        let (stage, paused) = (vs.stage, vs.paused);
        if paused {
            if self.action_pressed(Action::Confirm) || self.action_pressed(Action::Back) {
                self.play(Sound::Ui);
                self.set_versus_paused(false);
            } else if self.action_pressed(Action::Replay) {
                self.play(Sound::Ui);
                self.leave_versus();
            }
            return;
        }
        if self.action_pressed(Action::Back) {
            self.input.touch.cancel_drag();
            self.set_versus_paused(true);
            return;
        }
        if stage != Stage::Aiming {
            self.input.touch.cancel_drag();
        }
        match stage {
            Stage::Handoff => {
                if self.action_pressed(Action::Confirm)
                    || self.action_pressed(Action::Fire)
                    || self.pressed(Key::KpEnter)
                {
                    self.play(Sound::Ui);
                    self.begin_versus_turn();
                }
            }
            Stage::Aiming => self.handle_versus_aim(delta),
            Stage::Flight => {}
            Stage::Result => {
                if self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter) {
                    self.play(Sound::Ui);
                    self.after_versus_result();
                } else if self.action_pressed(Action::Replay) {
                    self.play(Sound::Ui);
                    self.leave_versus();
                }
            }
        }
    }

    fn set_versus_paused(&mut self, paused: bool) {
        if let Some(vs) = self.versus.live.as_mut() {
            vs.paused = paused;
        }
        self.fx.push(Effect::Redraw);
    }

    /// The player taking the phone is ready: their own aim comes back.
    pub(super) fn begin_versus_turn(&mut self) {
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        vs.stage = Stage::Aiming;
        (self.play.angle_ticks, self.play.power_ticks) = vs.aims[vs.side().index()];
        self.input.drag_from = None;
    }

    /// Aim and fire, as in a run: the same keys, buttons and drag. The
    /// right gun fires leftward, so its controls are mirrored. Left and
    /// Right (keys, pad, and the `<` `>` buttons) move its barrel's tip the
    /// same way across the screen as they move the left gun's, which is the
    /// other way in angle; and its drag is reflected: pull back to the right
    /// to throw to the left.
    fn handle_versus_aim(&mut self, delta: f64) {
        let quantum = self
            .versus
            .live
            .as_ref()
            .map(|vs| vs.duel.loadout.angle_quantum_ticks)
            .unwrap_or(10)
            .max(1);
        let mult = if self.shift_held() { 10 } else { 1 };
        let mirrored = self.versus.live.as_ref().map(VersusPlay::side) == Some(Side::Right);
        let left = self.action_repeated(0, Action::Left, delta) as i32;
        let right = self.action_repeated(1, Action::Right, delta) as i32;
        if left + right != 0 {
            let steps = if mirrored { left - right } else { right - left };
            self.play.angle_ticks =
                (self.play.angle_ticks + steps * quantum * mult).clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, Action::Down, delta) as i32;
        let up = self.action_repeated(3, Action::Up, delta) as i32;
        if up + down != 0 {
            self.play.power_ticks =
                (self.play.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }
        let mut fire = self.action_pressed(Action::Fire) || self.pressed(Key::Enter);
        if self.drag_aiming() {
            fire |= self.drag_aim(quantum, mirrored, false);
        }
        if fire {
            self.versus_fire();
        }
    }

    /// Fire the aim of the side whose turn it is and start its playback.
    pub(super) fn versus_fire(&mut self) {
        let aim = (self.play.angle_ticks, self.play.power_ticks);
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        let before = vs.duel.screen.clone();
        let score = (vs.duel.hits, vs.duel.rounds);
        let Some(report) = vs.fire_turn(aim) else {
            return;
        };
        vs.hit = report.hit.then(|| report.shooter.other());
        vs.self_hit = report.self_hit;
        vs.dinged = false;
        vs.score_before = Some(score);
        vs.landed = report.rolled.map(|roll| roll.landed);
        let longest = report
            .traces
            .iter()
            .map(|t| t.flight_time)
            .fold(0.0f64, f64::max);
        vs.anim = Some(Anim {
            traces: report.traces,
            before,
            t: 0.0,
            longest,
            collapsed: Vec::new(),
            settled: 0.0,
            impact_played: false,
            collapse_played: false,
        });
        vs.stage = Stage::Flight;
        self.play(Sound::Fire);
        self.play(Sound::Flight);
    }

    /// Walk the playback, announce the landing, then pass the phone or show
    /// the result.
    pub(super) fn step_versus(&mut self, delta: f64) {
        if self.mode != Mode::Versus {
            return;
        }
        let mut sounds = Vec::new();
        let mut finished = false;
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if vs.paused {
            return;
        }
        // The guns roll on screen once the player taking the phone has the
        // field in front of them: from READY, not behind the plate.
        if vs.stage == Stage::Aiming {
            if let Some(ease) = vs.ease.as_mut() {
                let t = ease.t.get_or_insert(0.0);
                *t += delta;
                if *t >= ROLL_EASE {
                    vs.ease = None;
                    // Busy no longer asks for frames: draw the guns home.
                    self.fx.push(Effect::Redraw);
                }
            }
        }
        let Some(anim) = vs.anim.as_mut() else {
            return;
        };
        anim.t += delta;
        if anim.done() {
            anim.settled += delta;
            if !anim.impact_played {
                anim.impact_played = true;
                vs.score_before = None;
                let struck = anim
                    .traces
                    .first()
                    .is_some_and(|t| matches!(t.end, End::Structure { .. }));
                sounds.push(if struck {
                    Sound::Structure
                } else {
                    Sound::Terrain
                });
                // The renderer calls the shot out from here on
                // (`VersusPlay::callout`); the sounds go with it.
                if vs.hit.is_some() {
                    sounds.push(Sound::Collapse);
                }
            }
            if vs.hit.is_some() && !vs.dinged && anim.settled >= DING_DELAY {
                vs.dinged = true;
                sounds.push(Sound::Hit);
            }
            finished = anim.settled > VERSUS_HOLD;
        }
        if finished {
            vs.anim = None;
            vs.hit = None;
            vs.self_hit = false;
            vs.landed = None;
            vs.stage = match vs.duel.phase {
                versus::Phase::Aiming => Stage::Handoff,
                versus::Phase::RoundOver(_) | versus::Phase::MatchOver(_) => {
                    // The fanfare comes with the plate that names the winner.
                    sounds.push(Sound::Win);
                    Stage::Result
                }
            };
            self.fx.push(Effect::Redraw);
        }
        for sound in sounds {
            self.play(sound);
        }
    }

    /// The result plate was dismissed: the next round, or a rematch. A
    /// rematch goes back to the pre-match screen rather than straight into
    /// a new match, so each player can turn aim help on or off before the
    /// next one; the choices from the last match are still set there.
    pub(super) fn after_versus_result(&mut self) {
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        match vs.duel.phase {
            versus::Phase::RoundOver(_) => {
                vs.next_round();
                vs.stage = Stage::Handoff;
            }
            versus::Phase::MatchOver(_) => self.open_versus_setup(),
            versus::Phase::Aiming => vs.stage = Stage::Handoff,
        }
    }

    /// Focus lost or the app paused: a turn being aimed or watched waits
    /// under the pause plate. The handoff and result plates already wait.
    pub(super) fn auto_pause_versus(&mut self) {
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if self.mode == Mode::Versus && matches!(vs.stage, Stage::Aiming | Stage::Flight) {
            self.input.touch.cancel_drag();
            vs.paused = true;
            self.fx.push(Effect::Redraw);
        }
    }

    /// `--salt-versus --salt-auto`: play both guns for the camera. Each
    /// turn readies at once and fires a shot that hits, except with
    /// `--salt-miss`, where every other turn throws a short lob; results
    /// are dismissed, and a finished match goes back through the pre-match
    /// screen to a rematch.
    pub(super) fn versus_autopilot(&mut self, delta: f64) {
        self.dev.auto_cooldown -= delta;
        if self.dev.auto_cooldown > 0.0 {
            return;
        }
        if self.mode == Mode::VersusSetup {
            self.start_versus();
            return;
        }
        let Some(vs) = self.versus.live.as_ref() else {
            return;
        };
        if vs.paused {
            return;
        }
        match vs.stage {
            Stage::Handoff => self.begin_versus_turn(),
            Stage::Aiming => {
                let duel = &vs.duel;
                let side = duel.to_move;
                let shot = if self.dev.auto_miss && duel.turn % 2 == 1 {
                    Some(Shot::new(1400, 330))
                } else {
                    let frame = versus::frame_for(&duel.screen, side);
                    let target = versus::target_in_frame(&duel.screen, side);
                    versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
                        .first()
                        .copied()
                };
                if let Some(shot) = shot {
                    (self.play.angle_ticks, self.play.power_ticks) =
                        (shot.angle_ticks, shot.power_ticks);
                    self.versus_fire();
                }
            }
            Stage::Flight => return,
            Stage::Result => self.after_versus_result(),
        }
        self.dev.auto_cooldown = 0.5;
    }

    /// Which versus screen is up, if any: the mode, the stage and the pause
    /// plate. A change spends the touches in progress (`settle_touch`).
    pub(super) fn versus_screen(&self) -> Option<(Mode, Option<Stage>, bool)> {
        matches!(self.mode, Mode::VersusSetup | Mode::Versus).then(|| {
            let live = self.versus.live.as_ref();
            (
                self.mode,
                live.map(|vs| vs.stage),
                live.is_some_and(|vs| vs.paused),
            )
        })
    }

    /// The versus screen changed: see "One touch, one screen".
    pub(super) fn settle_touch(&mut self) {
        self.input.touch.spend_all();
        self.input.touch_settle = TOUCH_SETTLE;
    }

    /// What a versus frame shows, for `FrameState`.
    pub(super) fn versus_fingerprint(&self) -> Option<Fingerprint> {
        self.versus.live.as_ref().map(|vs| {
            (
                vs.stage,
                vs.duel.round,
                vs.duel.turn,
                vs.shown_score().0,
                vs.paused,
            )
        })
    }
}

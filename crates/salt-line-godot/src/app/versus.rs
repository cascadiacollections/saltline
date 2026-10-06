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
//! Nothing here is saved: no run file, no board, no onboarding lessons. A
//! single-player run set aside for CONTINUE is left exactly as it was.
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
use crate::versus::{self, Match, Side};

/// Seconds the HIT or MISS is held on the landed shot before the phone is
/// passed on.
pub const VERSUS_HOLD: f64 = 1.1;

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
    /// Hits and rounds as they stood before the shot in the air, shown
    /// until it lands: the rules score a shot the moment it is fired, and
    /// a score that changed in mid-flight would give the result away.
    pub(crate) score_before: Option<([u32; 2], [u32; 2])>,
    /// Under the pause plate.
    pub(crate) paused: bool,
}

impl VersusPlay {
    /// Whose gun the readouts, the controls and the aim belong to.
    pub(crate) fn side(&self) -> Side {
        self.duel.to_move
    }

    /// The screen to draw: the pre-shot copy while a shot is in the air, so
    /// the crater appears when the shell lands.
    pub(crate) fn shown_screen(&self) -> &Screen {
        match self.anim.as_ref() {
            Some(anim) if !anim.done() => &anim.before,
            _ => &self.duel.screen,
        }
    }

    /// Hits and rounds to show: as they were until the shell lands.
    pub(crate) fn shown_score(&self) -> ([u32; 2], [u32; 2]) {
        self.score_before
            .unwrap_or((self.duel.hits, self.duel.rounds))
    }

    /// The wind to show: the wind the shot in the air was fired into, until
    /// the turn passes; the rules have already drawn the next turn's.
    pub(crate) fn shown_wind(&self) -> f64 {
        self.anim
            .as_ref()
            .map_or(self.duel.screen.wind, |anim| anim.before.wind)
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
        self.versus.live = None;
        self.mode = Mode::VersusSetup;
    }

    /// Start a match at the buffer's width, the left player to ready up
    /// first.
    pub(crate) fn start_versus(&mut self) {
        self.versus.started += 1;
        // A seed of its own for each match: the launch's entropy, the date
        // and a count. Everything after it is deterministic from the seed.
        let seed = salt_line_sim::rng::mix64(
            self.versus.entropy
                ^ salt_line_sim::rng::mix64(daily_seed(self.today) ^ self.versus.started),
        );
        let width = self.display.buffer_w as usize;
        self.versus.live = Some(VersusPlay {
            duel: Match::new(seed, width),
            stage: Stage::Handoff,
            anim: None,
            aims: [(900, 600); 2],
            arcs: [Vec::new(), Vec::new()],
            hit: None,
            score_before: None,
            paused: false,
        });
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.mode = Mode::Versus;
        // Long Lens belongs to a run; a duel is drawn at the usual height.
        self.apply_buffer_size();
    }

    /// Leave versus for the title. The match is dropped.
    pub(crate) fn leave_versus(&mut self) {
        self.versus.live = None;
        self.input.touch.cancel_drag();
        self.mode = Mode::Title;
        self.apply_buffer_size();
    }

    /// The pre-match screen: each side's aim help, start, back.
    pub(super) fn handle_versus_setup_input(&mut self) {
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
            self.mode = Mode::Title;
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
    fn begin_versus_turn(&mut self) {
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
    fn versus_fire(&mut self) {
        let shot = Shot::new(self.play.angle_ticks, self.play.power_ticks);
        let aim = (self.play.angle_ticks, self.play.power_ticks);
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        let before = vs.duel.screen.clone();
        let score = (vs.duel.hits, vs.duel.rounds);
        let Some(report) = vs.duel.fire(shot, true) else {
            return;
        };
        let i = report.shooter.index();
        vs.aims[i] = aim;
        vs.arcs[i] = report
            .traces
            .first()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        vs.hit = report.hit.then(|| report.shooter.other());
        vs.score_before = Some(score);
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
        let mut said = None;
        let mut finished = false;
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if vs.paused {
            return;
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
                said = Some(match vs.hit {
                    Some(gun) => {
                        sounds.push(Sound::Collapse);
                        format!("HIT  {} GUN", gun.name())
                    }
                    None => "MISS".to_string(),
                });
            }
            finished = anim.settled > VERSUS_HOLD;
        }
        if finished {
            vs.anim = None;
            vs.hit = None;
            vs.stage = match vs.duel.phase {
                versus::Phase::Aiming => Stage::Handoff,
                versus::Phase::RoundOver(_) | versus::Phase::MatchOver(_) => Stage::Result,
            };
            self.fx.push(Effect::Redraw);
        }
        for sound in sounds {
            self.play(sound);
        }
        if let Some(text) = said {
            self.note(text);
        }
    }

    /// The result plate was dismissed: the next round, or a rematch. A
    /// rematch goes back to the pre-match screen rather than straight into
    /// a new match, so each player can turn aim help on or off before the
    /// next one; the choices from the last match are still set there.
    fn after_versus_result(&mut self) {
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        match vs.duel.phase {
            versus::Phase::RoundOver(_) => {
                vs.duel.next_round();
                vs.arcs = [Vec::new(), Vec::new()];
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
                    versus::shots_at(&frame, target, &duel.loadout, 1)
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

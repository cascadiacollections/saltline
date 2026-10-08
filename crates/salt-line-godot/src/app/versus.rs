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
//!                    │   and every round dealt, the winning shot included
//!   app killed ──────┤ kept; launch offers RESUME MATCH ─► resume_match
//!   match won, ──────┤ cleared (drop_live_match); a rematch starts a new
//!    its plate left  │   one. Killed before that, it resumes on the plate
//!   quit (pause or ──┘ cleared (leave_versus)
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
//!
//! The AI's own stage changes (its plate to its aim to its shot) are the
//! exception: nobody touched anything to make them, and the only button on
//! either side of them is the pause button, in the same place. Settling
//! there would only swallow a tap on II made just as the AI moved on, so
//! `Game::tick` lets them pass ([`Game::step_versus_ai`]).
//!
//! # Against the AI
//!
//! With RIGHT OPPONENT: AI (DESIGN §11.7) the right gun is the computer's
//! (`crate::opponent`). Its turn runs itself: its plate holds for
//! [`AI_HANDOFF`] ([`AI_HANDOFF_CUED`] when the plate has a cue to read),
//! then the barrel swings to its shot over [`AI_SWING`] and fires at
//! [`AI_AIM`]. The player's plate stays, so the turns still read as turns,
//! but readies itself after [`AUTO_READY`] ([`AUTO_READY_CUED`] with a
//! cue, as long as the AI's own cued plate; READY skips it). Every one of
//! these waits stops under the pause plate, and the handoff plates pause
//! when focus goes, since in an AI match they no longer wait for a tap.
//!
//! The player's aim help is remembered per opponent for the session
//! ([`PlayerHelp`]): ON against the AI and OFF between two players until
//! they set it, and RIGHT OPPONENT brings back the one for its choice.
//!
//! # Quitting a match
//!
//! TITLE on the pause plate, and on a round's result plate between two
//! players, drops the match on screen and its line on disk, so while that
//! match is still to be won it asks twice: the first press arms it (the
//! row reads TAP AGAIN TO QUIT, or the key's PRESS AGAIN TO QUIT) and only
//! a second press within [`QUIT_CONFIRM`] quits. On the pause plate
//! RESUME, the pause key and MAKE MAP disarm it; on the result plate NEXT
//! ROUND, MAKE MAP and the pause key (which opens the pause plate,
//! disarmed) do; focus going and the timeout disarm it on either. A won
//! match's plate, paused or not, quits on the first press: nothing is left
//! to lose. Arming changes the versus screen (`Game::versus_screen`), so
//! the touch that armed it is spent and the second press has to be a new
//! tap after [`TOUCH_SETTLE`] (see "One touch, one screen"). Paused over a
//! round's result plate, TITLE sits where NEXT ROUND's target was (DESIGN
//! §11.4): after an accidental II, a tap meant for NEXT ROUND lands on
//! TITLE, and only arms it.

use super::*;
use crate::match_log::{self, Entry, SavedMatch};
use crate::opponent::{self, Level};
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

/// Seconds the AI's handoff plate is held before it aims.
pub const AI_HANDOFF: f64 = 1.2;
/// Seconds the AI's handoff plate is held when it shows a cue (LAST SHOT,
/// HIT TO WIN, TIED, GUNS ROLLED): long enough to be read out to a child.
pub const AI_HANDOFF_CUED: f64 = 2.0;
/// Seconds the AI's barrel and readouts take to swing to its shot.
pub const AI_SWING: f64 = 0.7;
/// Seconds from the AI's aim appearing to its shot: the swing, then a hold
/// on the aim, so it is seen.
pub const AI_AIM: f64 = 1.0;
/// Seconds the player's handoff plate waits in an AI match before it
/// readies by itself. READY (a tap, a key) skips it.
pub const AUTO_READY: f64 = 1.5;
/// Seconds the player's handoff plate waits when it shows a cue (the set
/// `VersusPlay::handoff_cued` tests): as long as the AI's cued plate, so a
/// LAST SHOT or GUNS ROLLED is read out before the player's aim appears.
pub const AUTO_READY_CUED: f64 = AI_HANDOFF_CUED;

/// Seconds a versus screen ignores new touches after it appears. Longer
/// than a tap (about 0.12 s on a phone), so whatever a device reports for
/// the tap that changed the screen is spent rather than taken again.
pub const TOUCH_SETTLE: f64 = 0.3;

/// Seconds an armed TITLE (the pause plate's, or a round's result plate's)
/// waits for its second press before it disarms (see "Quitting a match").
pub const QUIT_CONFIRM: f64 = 3.0;

/// The player's aim help (the left gun's, YOUR AIM HELP against the AI),
/// remembered for the session for each kind of opponent: ON against the
/// AI and OFF against a human until set (DESIGN §11.1). The pre-match
/// row sets the one for the opponent chosen; RIGHT OPPONENT brings back
/// the other's; resuming a match stores the match's own help as its
/// opponent's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PlayerHelp {
    /// Against a human.
    pub(crate) human: bool,
    /// Against the AI.
    pub(crate) ai: bool,
}

impl Default for PlayerHelp {
    fn default() -> PlayerHelp {
        PlayerHelp {
            human: false,
            ai: true,
        }
    }
}

impl PlayerHelp {
    /// The player's help against the AI (`ai`) or a human.
    pub(crate) fn get(self, ai: bool) -> bool {
        if ai {
            self.ai
        } else {
            self.human
        }
    }

    /// Remember `on` as the player's help against the AI (`ai`) or a human.
    pub(crate) fn set(&mut self, ai: bool, on: bool) {
        if ai {
            self.ai = on;
        } else {
            self.human = on;
        }
    }
}

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
    /// Off for both by default. The left player's is [`Self::player_help`]
    /// for the opponent chosen ([`VersusState::set_opponent`]); the right
    /// player's is theirs alone.
    pub(crate) help: [bool; 2],
    /// The left player's aim help for each kind of opponent, kept for the
    /// session: `help[0]` is the one for [`Self::ai`] (DESIGN §11.1).
    pub(crate) player_help: PlayerHelp,
    /// RIGHT OPPONENT: AI on the pre-match screen: the right gun is the
    /// computer's (DESIGN §11.7). Off by default; kept for a rematch.
    pub(crate) ai: bool,
    /// The AI's level, as the pre-match screen has it. Kept while the AI
    /// is off, so turning it back on finds it where it was.
    pub(crate) level: Level,
    /// ROLLING GUNS OFF on the pre-match screen: the guns stay where they
    /// are all match ([`versus::GUN_SHIFT`] off). On by default, and kept
    /// for a rematch, so the rule can be tried both ways.
    pub(crate) rolling_off: bool,
    /// The GROUND row: the layout a match started now plays on (DESIGN
    /// §13.9), or `None` for GENERATED. Kept for a rematch, and set again
    /// by RESUME MATCH on a layout.
    pub(crate) ground: Option<super::GroundPick>,
    /// Where the GROUND row's pick for a match suspended this session came
    /// from (MAKE MAP OF THIS ROUND): RESUME MATCH picks it again by that,
    /// so a rematch plays a slot as it now is, not as the match began.
    pub(crate) suspended_source: Option<super::editor::GroundSource>,
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
    /// The right gun is the computer's, at this level (DESIGN §11.7).
    pub(crate) ai: Option<Level>,
    /// Seconds the current plate or the AI's aim has been up: what the
    /// AI's pacing and the player's auto-ready count. Zeroed on each stage.
    ai_wait: f64,
    /// The aim the AI's barrel swings from, and the shot it swings to and
    /// fires, once chosen for this turn.
    ai_aim: Option<((i32, i32), Shot)>,
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
    /// TITLE on the pause plate, or on a round's result plate, has been
    /// pressed once and waits for the second press that quits: seconds
    /// since it was armed, up to [`QUIT_CONFIRM`]. Never set on both: the
    /// pause plate opens and closes disarmed. See "Quitting a match".
    pub(crate) quit_armed: Option<f64>,
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
    /// The layout every round is played on, for a match on one (DESIGN
    /// §13.9): its code goes in the match's line (`match_log` format 2).
    pub(crate) layout: Option<salt_line_sim::layout::Layout>,
    /// Where the GROUND row's pick this match was started on came from.
    pub(crate) source: Option<super::editor::GroundSource>,
}

impl VersusPlay {
    /// `duel` on screen from its start: the first player's handoff plate,
    /// both aims in the middle of the dial, nothing kept on disk yet.
    pub(crate) fn new(duel: Match) -> VersusPlay {
        VersusPlay {
            duel,
            stage: Stage::Handoff,
            ai: None,
            ai_wait: 0.0,
            ai_aim: None,
            anim: None,
            aims: [(900, 600); 2],
            arcs: [Vec::new(), Vec::new()],
            hit: None,
            self_hit: false,
            shooter: Side::Left,
            dinged: false,
            score_before: None,
            paused: false,
            quit_armed: None,
            landed: None,
            rolled: None,
            ease: None,
            log: Vec::new(),
            kept: false,
            saved_len: None,
            layout: None,
            source: None,
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
    /// result plate if the last shot won a round or the match (so a match
    /// killed as it was won still says who won). `None` for a log that
    /// does not replay.
    pub(crate) fn rebuild(saved: &SavedMatch) -> Option<VersusPlay> {
        let mut vs = VersusPlay::new(saved.start()?);
        vs.layout = saved.layout().ok().flatten();
        vs.ai = saved.ai;
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
            versus::Phase::RoundOver(_) | versus::Phase::MatchOver(_) => Stage::Result,
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

    /// The computer has the turn: an AI match, and the right gun to move.
    pub(crate) fn ai_turn(&self) -> bool {
        self.ai.is_some() && self.side() == Side::Right
    }

    /// The handoff plate has a cue under the name: what rides on the shot,
    /// or a roll of the guns. The same test as the renderer's
    /// `handoff_cues` being non-empty, for the AI's longer hold.
    pub(crate) fn handoff_cued(&self) -> bool {
        self.duel.stakes().is_some() || self.rolled.is_some()
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

/// What `vs` is kept on disk as, with the aim help `help`: format 1, 2 on
/// a layout's ground, or 3 against the AI (`match_log`).
pub(super) fn saved_line(vs: &VersusPlay, help: [bool; 2]) -> Option<SavedMatch> {
    let mut saved = match &vs.layout {
        Some(l) => SavedMatch::on_ground(&vs.duel, l, help, &vs.log)?,
        None => SavedMatch::of(&vs.duel, help, &vs.log),
    };
    saved.ai = vs.ai;
    Some(saved)
}

impl VersusState {
    /// The rules a match started now plays by: the game's, with the guns'
    /// roll off if the pre-match screen says so.
    pub(crate) fn rules(&self) -> versus::Rules {
        match_log::rules_for(!self.rolling_off)
    }

    /// RIGHT OPPONENT set to the AI (`ai`) or a human. The left player's
    /// aim help becomes the one remembered for that opponent
    /// ([`PlayerHelp`]): a child playing the computer alone starts with
    /// the help, two players start without, and each keeps what was set
    /// for it. The right player's is untouched.
    pub(crate) fn set_opponent(&mut self, ai: bool) {
        self.ai = ai;
        self.help[0] = self.player_help.get(ai);
    }

    /// The left player's aim help set to `on` for the opponent chosen.
    fn set_player_help(&mut self, on: bool) {
        self.help[0] = on;
        self.player_help.set(self.ai, on);
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
    /// first, or on the GROUND row's layout at the layout's width. It is
    /// kept on disk from its first frame (`persist_match`), replacing any
    /// match suspended before it. A layout that does not pass the duel gate
    /// (one saved before a sim change, say) starts nothing.
    pub(crate) fn start_versus(&mut self) {
        if let Some(pick) = self.versus.ground.clone() {
            if !pick.playable {
                self.note("THIS MAP DOES NOT PASS - PICK ANOTHER".into());
                return;
            }
            self.start_layout_match(&pick.layout);
            if let Some(vs) = self.versus.live.as_mut() {
                vs.kept = true;
                vs.source = Some(pick.source);
            }
            self.saved_match = None;
            self.versus.suspended_source = None;
            return;
        }
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
        self.start_versus_with(Match::with_rules(seed, width, rules));
    }

    /// Put `duel` on screen at its first handoff, not kept on disk.
    fn start_versus_with(&mut self, duel: Match) {
        let mut vs = VersusPlay::new(duel);
        vs.ai = (self.versus.ai && !self.spectating()).then_some(self.versus.level);
        self.versus.live = Some(vs);
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.mode = Mode::Versus;
        // Long Lens belongs to a run; a duel is drawn at the usual height.
        self.apply_buffer_size();
    }

    /// Start a match on a two-gun layout (DESIGN §13.9) at the layout's
    /// width, letterboxed on a wider phone, under the pre-match screen's
    /// rules with the layout's wind. Not kept on disk by itself:
    /// [`Game::start_versus`] marks a player's match kept, and its line
    /// carries the layout's code; `--salt-layout`'s is not.
    pub(crate) fn start_layout_match(&mut self, layout: &salt_line_sim::layout::Layout) {
        self.versus.started += 1;
        let seed = salt_line_sim::rng::mix64(
            self.versus.entropy ^ salt_line_sim::rng::mix64(layout.digest() ^ self.versus.started),
        );
        let rules = self.versus.rules();
        self.set_run_width(Some(layout.width()));
        self.start_versus_with(crate::custom::layout_match(layout, seed, rules));
        if let Some(vs) = self.versus.live.as_mut() {
            vs.layout = Some(layout.clone());
        }
    }

    /// Leave versus for the title. The match is dropped, and with it its
    /// line on disk: the players quit it.
    pub(crate) fn leave_versus(&mut self) {
        self.drop_live_match();
        self.input.touch.cancel_drag();
        self.mode = Mode::Title;
        self.apply_buffer_size();
    }

    /// Take the match on screen off it without ending it: its line stays
    /// on disk and RESUME MATCH offers it, as after the app was killed.
    /// For MAKE MAP OF THIS ROUND mid-match.
    pub(super) fn suspend_live_match(&mut self, vs: VersusPlay) {
        if !vs.kept || self.spectating() {
            return;
        }
        self.saved_match = saved_line(&vs, self.versus.help);
        self.versus.suspended_source = vs.source;
    }

    /// Drop the match on screen, and clear its line if it was kept: a won
    /// match is kept to watch again (DESIGN §9) as its line goes; a match
    /// quit before it was won is not. A match the demo or a watch set
    /// aside is not on screen, and theirs are never kept, but both are
    /// checked too (DESIGN §12.3).
    pub(super) fn drop_live_match(&mut self) {
        let Some(vs) = self.versus.live.take() else {
            return;
        };
        if vs.kept && !self.spectating() {
            self.fx.push(Effect::MatchFile(MatchFile::Discard));
            self.record_match(&vs);
        }
    }

    /// Write the match's line once at its start and whenever a shot is
    /// fired or a round dealt, the winning shot included: a match killed
    /// as it was won comes back on its result plate. The line is cleared
    /// when the players leave that plate (`drop_live_match`). Checked every
    /// frame, written only on a change, never during the demo.
    pub(super) fn persist_match(&mut self) {
        if self.spectating() {
            return;
        }
        let help = self.versus.help;
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if !vs.kept {
            return;
        }
        let len = vs.log.len();
        if vs.saved_len == Some(len) {
            return;
        }
        // Tried once per entry: a failed write is logged by `save::write`,
        // and the next shot tries again.
        vs.saved_len = Some(len);
        if let Some(saved) = saved_line(vs, help) {
            self.fx
                .push(Effect::MatchFile(MatchFile::Write(saved.to_line())));
        }
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
        // The match's own aim help comes back as it was played, and is
        // what the player has against that opponent from now on: a
        // rematch, or a switch away and back, finds it as it was.
        self.versus.help = saved.help;
        self.versus.ai = saved.ai.is_some();
        self.versus.player_help.set(self.versus.ai, saved.help[0]);
        if let Some(level) = saved.ai {
            self.versus.level = level;
        }
        // A match on a layout never rolls, whatever was chosen: the
        // choice for the next generated match stays as it was.
        let source = self.versus.suspended_source.take();
        match vs.layout.clone() {
            Some(l) => {
                // The match goes on on its own ground; the GROUND row (and
                // so a rematch) is the pick it came from, as that slot now
                // is, saying so if the map was saved over meanwhile.
                let again =
                    source.and_then(|src| self.grounds().into_iter().find(|g| g.source == src));
                match again {
                    Some(pick) => {
                        if pick.layout != l {
                            self.note("A REMATCH PLAYS THE MAP AS SAVED".into());
                        }
                        vs.source = Some(pick.source);
                        self.versus.ground = Some(pick);
                    }
                    None => self.versus.ground = Some(self.ground_for(&l)),
                }
            }
            None => {
                self.versus.rolling_off = !saved.rolling;
                self.versus.ground = None;
            }
        }
        self.versus.live = Some(vs);
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.set_run_width(Some(saved.width));
        self.mode = Mode::Versus;
        self.apply_buffer_size();
    }

    /// The pre-match screen: the right opponent, each side's aim help (the
    /// AI's level in the right one's place), rolling guns, the GROUND row,
    /// start, back, and MAKE MAP. Rolling guns is OFF, and does not toggle,
    /// on a layout's ground (DESIGN §13.9).
    pub(super) fn handle_versus_setup_input(&mut self) {
        if self.action_pressed(Action::Replay) {
            self.versus.set_opponent(!self.versus.ai);
            self.play(Sound::Ui);
        }
        let on_map = self.versus.ground.is_some();
        if self.action_pressed(Action::Up) && !on_map {
            self.versus.rolling_off = !self.versus.rolling_off;
            self.play(Sound::Ui);
        }
        let step = (self.action_pressed(Action::Down)
            || self
                .input
                .touch
                .pressed(Action::Edit(crate::input::Ed::GroundNext))) as i32
            - self
                .input
                .touch
                .pressed(Action::Edit(crate::input::Ed::GroundPrev)) as i32;
        if step != 0 {
            self.step_ground(step);
            self.play(Sound::Ui);
        }
        for (action, side) in [(Action::Left, Side::Left), (Action::Right, Side::Right)] {
            if !self.action_pressed(action) {
                continue;
            }
            let i = side.index();
            // The AI needs no aim help: its row steps its level instead,
            // and the right player's choice waits for a human match.
            if side == Side::Right && self.versus.ai {
                self.versus.level = self.versus.level.next();
            } else if side == Side::Left {
                self.versus.set_player_help(!self.versus.help[i]);
            } else {
                self.versus.help[i] = !self.versus.help[i];
            }
            self.play(Sound::Ui);
        }
        // MAKE MAP's keys are asked last, so a binding of a key or button
        // they share wins (presses are taken once).
        if self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter) {
            self.play(Sound::Ui);
            self.start_versus();
        } else if self.action_pressed(Action::Back) {
            self.play(Sound::Ui);
            self.leave_versus();
        } else if self.make_map_pressed(crate::input::Ed::MakeMap) {
            self.play(Sound::Ui);
            self.open_editor();
        }
    }

    /// MAKE MAP (`ed` on a touchscreen, E, a controller's X). Asked only
    /// where it is read, after the screen's own actions: the press is taken.
    fn make_map_pressed(&mut self, ed: crate::input::Ed) -> bool {
        self.input.touch.pressed(Action::Edit(ed))
            || self.pressed(Key::E)
            || self.pad_pressed(Pad::X)
    }

    /// A match in play. As in a run, nothing but pause is read while a shot
    /// is in the air.
    pub(super) fn handle_versus_input(&mut self, delta: f64) {
        let Some(vs) = self.versus.live.as_ref() else {
            self.mode = Mode::Title;
            return;
        };
        let (stage, paused, ai_turn) = (vs.stage, vs.paused, vs.ai_turn());
        // A won match's plate has nothing left to lose: its TITLE, paused,
        // quits at once. Anything else asks twice.
        let won = stage == Stage::Result && matches!(vs.duel.phase, versus::Phase::MatchOver(_));
        let quit_armed = vs.quit_armed;
        // Between rounds of a match against the AI, TITLE is not offered:
        // a child playing alone mashing the plate (or Y, which toggles the
        // AI on the pre-match screen) would throw the match away. Leaving
        // mid-match is the pause plate's TITLE, as it is between turns:
        // the pause key (and II on a touchscreen) works on every result
        // plate, and RESUME comes back to the same plate, nothing dealt.
        let round_title =
            !(vs.ai.is_some() && matches!(vs.duel.phase, versus::Phase::RoundOver(_)));
        let fork = crate::input::Ed::ForkRound;
        if paused {
            if self.action_pressed(Action::Confirm) || self.action_pressed(Action::Back) {
                self.play(Sound::Ui);
                self.set_versus_paused(false);
            } else if self.action_pressed(Action::Replay) {
                self.press_versus_title(won, quit_armed);
            } else if self.make_map_pressed(fork) {
                self.play(Sound::Ui);
                self.fork_this_round();
            } else {
                self.count_quit_armed(quit_armed, delta);
            }
            return;
        }
        if self.action_pressed(Action::Back) {
            self.input.touch.cancel_drag();
            self.set_versus_paused(true);
            return;
        }
        if ai_turn && matches!(stage, Stage::Handoff | Stage::Aiming) {
            self.input.touch.cancel_drag();
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
                // A round's plate between two players asks twice before its
                // TITLE drops the match, as the pause plate does; the match's
                // own plate quits at once. NEXT ROUND (or REMATCH) and MAKE
                // MAP disarm it, as the pause key does by opening the pause
                // plate.
                if self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter) {
                    self.set_quit_armed(None);
                    self.play(Sound::Ui);
                    self.after_versus_result();
                } else if won && self.action_pressed(Action::Watch) {
                    // WATCH the match just won, then back to this plate.
                    self.watch_live_match();
                } else if round_title && self.action_pressed(Action::Replay) {
                    self.press_versus_title(won, quit_armed);
                } else if self.make_map_pressed(fork) {
                    self.set_quit_armed(None);
                    self.play(Sound::Ui);
                    self.fork_this_round();
                } else {
                    self.count_quit_armed(quit_armed, delta);
                }
            }
        }
    }

    /// TITLE pressed on the pause plate or a round's result plate: quit if
    /// nothing is left to lose (`won`) or it was armed already, else arm
    /// it. See "Quitting a match".
    fn press_versus_title(&mut self, won: bool, quit_armed: Option<f64>) {
        self.play(Sound::Ui);
        if won || quit_armed.is_some() {
            self.leave_versus();
        } else {
            self.set_quit_armed(Some(0.0));
        }
    }

    /// An armed TITLE waits `delta` more for its second press, and disarms
    /// after [`QUIT_CONFIRM`].
    fn count_quit_armed(&mut self, quit_armed: Option<f64>, delta: f64) {
        if let Some(t) = quit_armed {
            let t = t + delta;
            self.set_quit_armed((t < QUIT_CONFIRM).then_some(t));
        }
    }

    /// Arm TITLE (`Some(0.0)`), count it on, or disarm it (`None`), on
    /// the pause plate or a round's result plate: see "Quitting a match".
    /// Redrawn when it arms or disarms.
    fn set_quit_armed(&mut self, armed: Option<f64>) {
        if let Some(vs) = self.versus.live.as_mut() {
            if vs.quit_armed.is_some() != armed.is_some() {
                self.fx.push(Effect::Redraw);
            }
            vs.quit_armed = armed;
        }
    }

    fn set_versus_paused(&mut self, paused: bool) {
        if let Some(vs) = self.versus.live.as_mut() {
            vs.paused = paused;
            vs.quit_armed = None;
            // A plate that moves on by itself starts its wait afresh, so
            // whoever comes back to it has time to read it.
            if vs.stage == Stage::Handoff {
                vs.ai_wait = 0.0;
            }
        }
        self.fx.push(Effect::Redraw);
    }

    /// The player taking the phone is ready: their own aim comes back.
    pub(super) fn begin_versus_turn(&mut self) {
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        vs.stage = Stage::Aiming;
        vs.ai_wait = 0.0;
        vs.ai_aim = None;
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

    /// An AI match's clock: the computer's turn (its plate, its aim, its
    /// shot) and the player's plate readying itself ([`AUTO_READY`]). True
    /// if the computer's own turn moved on a stage this frame, which is not
    /// a screen change a touch must be settled for (see "One touch, one
    /// screen"); the player's auto-ready is, and returns false.
    ///
    /// Nothing moves under the pause plate or while the window is out of
    /// focus, and nothing here runs in the demo or for the dev autopilot,
    /// which drive both guns themselves.
    pub(super) fn step_versus_ai(&mut self, delta: f64) -> bool {
        if self.mode != Mode::Versus || self.spectating() || self.dev.auto || self.unfocused {
            return false;
        }
        let Some(vs) = self.versus.live.as_mut() else {
            return false;
        };
        let Some(level) = vs.ai else {
            return false;
        };
        if vs.paused {
            return false;
        }
        match vs.stage {
            Stage::Handoff if !vs.ai_turn() => {
                vs.ai_wait += delta;
                let hold = if vs.handoff_cued() {
                    AUTO_READY_CUED
                } else {
                    AUTO_READY
                };
                if vs.ai_wait >= hold {
                    self.begin_versus_turn();
                }
                false
            }
            Stage::Handoff => {
                vs.ai_wait += delta;
                let hold = if vs.handoff_cued() {
                    AI_HANDOFF_CUED
                } else {
                    AI_HANDOFF
                };
                if vs.ai_wait < hold {
                    return false;
                }
                self.begin_versus_turn();
                true
            }
            Stage::Aiming if vs.ai_turn() => {
                // The guns finish rolling before the AI touches its aim.
                if vs.easing() {
                    return false;
                }
                let (from, shot) = *vs.ai_aim.get_or_insert_with(|| {
                    (
                        (self.play.angle_ticks, self.play.power_ticks),
                        opponent::ai_shot(&vs.duel, level),
                    )
                });
                vs.ai_wait += delta;
                let t = (vs.ai_wait / AI_SWING).clamp(0.0, 1.0);
                let e = t * t * (3.0 - 2.0 * t);
                let lerp = |a: i32, b: i32| a + ((b - a) as f64 * e).round() as i32;
                (self.play.angle_ticks, self.play.power_ticks) = (
                    lerp(from.0, shot.angle_ticks),
                    lerp(from.1, shot.power_ticks),
                );
                if vs.ai_wait < AI_AIM {
                    return false;
                }
                self.versus_fire();
                true
            }
            Stage::Aiming | Stage::Flight | Stage::Result => false,
        }
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
            vs.ai_wait = 0.0;
            vs.ai_aim = None;
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
                vs.ai_wait = 0.0;
                vs.ai_aim = None;
                vs.stage = Stage::Handoff;
            }
            versus::Phase::MatchOver(_) => self.open_versus_setup(),
            versus::Phase::Aiming => vs.stage = Stage::Handoff,
        }
    }

    /// Focus lost or the app paused: a turn being aimed or watched waits
    /// under the pause plate. So does the handoff plate in a match against
    /// the AI, which moves on by itself (the AI's turn, and the player's
    /// auto-ready); between two players it waits for READY already, and the
    /// result plate always waits for a press.
    ///
    /// A TITLE armed on the pause plate or a round's result plate is
    /// disarmed: whoever comes back asks again.
    pub(super) fn auto_pause_versus(&mut self) {
        self.set_quit_armed(None);
        let Some(vs) = self.versus.live.as_mut() else {
            return;
        };
        if self.mode == Mode::Versus
            && (matches!(vs.stage, Stage::Aiming | Stage::Flight)
                || (vs.ai.is_some() && vs.stage == Stage::Handoff))
        {
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

    /// Which versus screen is up, if any: the mode, the stage, the pause
    /// plate and whether its TITLE is armed. A change spends the touches
    /// in progress (`settle_touch`), so the tap that arms TITLE cannot
    /// also confirm it.
    pub(super) fn versus_screen(&self) -> Option<(Mode, Option<Stage>, bool, bool)> {
        matches!(self.mode, Mode::VersusSetup | Mode::Versus).then(|| {
            let live = self.versus.live.as_ref();
            (
                self.mode,
                live.map(|vs| vs.stage),
                live.is_some_and(|vs| vs.paused),
                live.is_some_and(|vs| vs.quit_armed.is_some()),
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

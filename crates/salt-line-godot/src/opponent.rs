//! The computer's gun in local versus (DESIGN §11.7), and the attract demo's
//! duel shots (§12.2), which aim the same way.
//!
//! Engine-free: a shot is a pure function of the match as it stands and a
//! hash, so a match against the AI replays from its log like any other, and
//! the duel harness (`examples/duel.rs`) can put the AI on either gun.
//!
//! # How it aims
//!
//! The AI knows the wind and the range exactly. Each turn it asks the
//! solver for a shot that hits ([`versus::shots_at`], flown by the
//! integrator in this turn's wind), then a hash of the turn decides whether
//! to fire it or to miss on purpose. A miss is a *near* miss: the hitting
//! aim moved in power, then in angle ([`NEAR_MISS`]), each tried on a copy of
//! the match until one leaves the other gun unhit and its own gun alone. If
//! none does, or the solver finds nothing, the shot falls back by
//! [`Fallback`]: the AI tries a few high, short lobs the same way
//! ([`FALLBACKS`], [`Fallback::Tested`]), so it avoids shelling itself
//! wherever any of them allows; the attract demo fires the plain [`LOB`]
//! untried ([`Fallback::Lob`]), exactly as it always has.
//!
//! # How often it hits
//!
//! [`Level`] sets the odds of firing the hitting shot: EASY 2 in 10, MEDIUM
//! 4 in 10, HARD 6 in 10 (the demo's 3 in 5). While the AI leads on hits
//! in the round being played, it drops one step: HARD plays at MEDIUM's
//! odds, MEDIUM at EASY's, and EASY at 1 in 10 ([`odds_now`]). Rounds won
//! do not count, so a turn level or behind on hits, a last shot to tie
//! among them, is always taken at full odds. Which turns
//! hit is [`turn_hash`] of the match seed, the round, the turn and the
//! level: no random stream, no frame timing, so the same match and the same
//! shots always get the same answer.

use salt_line_sim::ballistics::Shot;
use salt_line_sim::dmath;
use salt_line_sim::rng::mix64;
use salt_line_sim::POWER_TICKS_MAX;

use crate::versus::{self, Match, Side};

/// How good the AI is, set on the pre-match screen. EASY by default: the
/// AI is there for a child playing alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum Level {
    #[default]
    Easy,
    Medium,
    Hard,
}

impl Level {
    /// Every level, easiest first, in the order the pre-match row steps.
    pub const ALL: [Level; 3] = [Level::Easy, Level::Medium, Level::Hard];

    /// How the screens name it.
    pub fn name(self) -> &'static str {
        match self {
            Level::Easy => "EASY",
            Level::Medium => "MEDIUM",
            Level::Hard => "HARD",
        }
    }

    /// The next level on the pre-match row, HARD going round to EASY.
    pub fn next(self) -> Level {
        match self {
            Level::Easy => Level::Medium,
            Level::Medium => Level::Hard,
            Level::Hard => Level::Easy,
        }
    }

    /// Its token in a match's line (`match_log` format 3).
    pub fn token(self) -> &'static str {
        match self {
            Level::Easy => "easy",
            Level::Medium => "medium",
            Level::Hard => "hard",
        }
    }

    /// The level a line's token names, if any.
    pub fn from_token(token: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.token() == token)
    }

    /// The odds of firing the hitting shot on an even or trailing turn.
    pub fn odds(self) -> Odds {
        match self {
            Level::Easy => Odds::tenths(2),
            Level::Medium => Odds::tenths(4),
            Level::Hard => Odds::tenths(6),
        }
    }

    /// The odds while the AI is ahead: one step down, EASY to
    /// [`EASED_FLOOR`].
    pub fn eased(self) -> Odds {
        match self {
            Level::Easy => EASED_FLOOR,
            Level::Medium => Level::Easy.odds(),
            Level::Hard => Level::Medium.odds(),
        }
    }

    /// Mixed into [`turn_hash`], so each level misses on turns of its own.
    fn salt(self) -> u64 {
        match self {
            Level::Easy => 1,
            Level::Medium => 2,
            Level::Hard => 3,
        }
    }
}

/// The chance of firing the hitting shot: `hits` in `out_of`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Odds {
    pub hits: u64,
    pub out_of: u64,
}

impl Odds {
    /// `n` in 10.
    pub const fn tenths(n: u64) -> Odds {
        Odds {
            hits: n,
            out_of: 10,
        }
    }

    /// Whether a turn hashed to `h` fires the hitting shot. The top `hits`
    /// residues hit, which for the demo's 3 in 5 is the `h % 5 >= 2` it has
    /// always used.
    pub fn hit_on(self, h: u64) -> bool {
        h % self.out_of >= self.out_of - self.hits
    }
}

/// The attract demo's duel odds (DESIGN §12.2): three hits in five.
pub const DEMO_ODDS: Odds = Odds { hits: 3, out_of: 5 };

/// The least an AI that is ahead still hits: 1 in 10.
pub const EASED_FLOOR: Odds = Odds::tenths(1);

/// Mixed into [`turn_hash`], so the AI's turns are not the demo's.
const AI_SALT: u64 = 0xA1_0FF0_5E7A_11CE;

/// A high lob that lands short, well clear of any target: the first miss
/// tried when no near one can be found, and the demo run's.
pub const LOB: (i32, i32) = (1400, 330);

/// The misses tried, in turn, when no near miss will do (or the solver
/// finds nothing): [`LOB`], then flatter and steeper short shots, in case
/// the wind blows the first back onto the shooter's own gun.
pub const FALLBACKS: [(i32, i32); 5] = [LOB, (1100, 330), (800, 300), (1600, 250), (600, 400)];

/// Changes to the solver's aim tried, in turn, for a near miss: (angle,
/// power) ticks, power first. A flat, fast shot topples a structure over a
/// wide spread of power, so steeper and flatter barrels follow.
pub const NEAR_MISS: [(i32, i32); 16] = [
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
pub fn near_misses(aim: Shot, flip: i32) -> impl Iterator<Item = Shot> {
    NEAR_MISS.into_iter().map(move |(da, dp)| {
        Shot::new(
            (aim.angle_ticks + flip * da).clamp(0, dmath::MAX_ANGLE_TICKS),
            (aim.power_ticks + flip * dp).clamp(0, POWER_TICKS_MAX),
        )
    })
}

/// The hash behind the AI's turn: the match seed, the round, the turn and
/// the level, and nothing else. The seed and the level are each hashed on
/// their own, so (seed, level) pairs do not alias the way `seed ^ level`
/// made them (seed 1 at EASY was seed 3 at MEDIUM). The demo's duels hash
/// their turns their own way (`app::demo`), so this changes none of them.
pub fn turn_hash(seed: u64, round: u32, turn: u32, level: Level) -> u64 {
    mix64(
        AI_SALT
            ^ mix64(seed)
            ^ mix64(level.salt().rotate_left(48))
            ^ mix64((round as u64).rotate_left(32) ^ turn as u64),
    )
}

/// `side` leads on hits in the round being played. Rounds won do not
/// count: a side behind or level on hits is never ahead.
pub fn ahead(duel: &Match, side: Side) -> bool {
    duel.hits[side.index()] > duel.hits[side.other().index()]
}

/// The odds for the side to move at `level`: eased while it leads on hits
/// this round ([`ahead`]).
pub fn odds_now(duel: &Match, level: Level) -> Odds {
    if ahead(duel, duel.to_move) {
        level.eased()
    } else {
        level.odds()
    }
}

/// What a shot falls back to when the solver finds nothing, or no near
/// miss misses cleanly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    /// The plain [`LOB`], untried: the attract demo's, as it has always
    /// been, so the demo's duels are main's by construction.
    Lob,
    /// [`FALLBACKS`] tried on a copy of the match (`tested_fallback`):
    /// the AI opponent's.
    Tested,
}

/// The AI's shot for the side to move at `level`.
pub fn ai_shot(duel: &Match, level: Level) -> Shot {
    let h = turn_hash(duel.seed, duel.round, duel.turn, level);
    shot(duel, h, odds_now(duel, level), Fallback::Tested)
}

/// The side to move's shot: the solver's hitting aim when `odds` say so for
/// a turn hashed to `h`, otherwise a near miss, tried on a copy of the
/// match; `fallback` when neither is found. See the module docs.
pub fn shot(duel: &Match, h: u64, odds: Odds, fallback: Fallback) -> Shot {
    let fall_back = || match fallback {
        Fallback::Lob => Shot::new(LOB.0, LOB.1),
        Fallback::Tested => tested_fallback(duel),
    };
    let side = duel.to_move;
    let frame = versus::frame_for(&duel.screen, side);
    let target = versus::target_in_frame(&duel.screen, side);
    let Some(aim) = versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
        .first()
        .copied()
    else {
        return fall_back();
    };
    if odds.hit_on(h) {
        return aim;
    }
    let flip = if (h >> 8) & 1 == 0 { 1 } else { -1 };
    near_misses(aim, flip)
        .find(|&shot| misses_cleanly(duel, shot))
        .unwrap_or_else(fall_back)
}

/// `shot`, fired on a copy of `duel`, hits neither gun.
fn misses_cleanly(duel: &Match, shot: Shot) -> bool {
    let mut trial = duel.clone();
    trial
        .fire(shot, false)
        .is_some_and(|r| !r.hit && !r.self_hit)
}

/// The AI's fallback, each of [`FALLBACKS`] tried on a copy, in this order:
///
/// 1. the first that hits neither gun, so a turn meant to miss misses;
/// 2. failing that, the first that hits only the other gun: a point
///    given away rather than the shooter's own gun shelled;
/// 3. failing every one, [`LOB`], which may shell the shooter.
fn tested_fallback(duel: &Match) -> Shot {
    let shots = FALLBACKS.map(|(a, p)| Shot::new(a, p));
    shots
        .into_iter()
        .find(|&s| misses_cleanly(duel, s))
        .or_else(|| {
            shots.into_iter().find(|&s| {
                let mut trial = duel.clone();
                trial.fire(s, false).is_some_and(|r| !r.self_hit)
            })
        })
        .unwrap_or(shots[0])
}

#[cfg(test)]
mod tests;

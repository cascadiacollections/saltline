//! Resuming a local versus match the operating system killed. DESIGN §11.6.
//!
//! A match is a seed, its options and the shots fired (DESIGN §11: every
//! wind, roll and screen follows from the seed and the shots), so the match
//! in play is kept on disk as exactly that, the way a run is (DESIGN §9.6,
//! `resume`): `user://match.txt`, one line, rewritten atomically
//! (`durable::write_atomic`) every time a shot is fired or a new round is
//! dealt. On launch the line is replayed and the title offers RESUME MATCH.
//!
//! Engine-free, like `resume`: the line format, the replay and the decision
//! (offer it, or throw it away) live here, where they can be tested
//! headless. The core (`app/versus.rs`) asks for writes as
//! `app::MatchFile` effects; `game.rs` carries them out with `resume`'s
//! file rules at this module's own three paths ([`MATCH_FILES`]).
//!
//! # The line
//!
//! ```text
//! match <format> <sim epoch> <match epoch> <seed> <width> <rolling> <help L> <help R> <n> <log>
//! match 1 6 1 1234567 360 1 0 1 4 900:600 870:640 / 850:610
//! ```
//!
//! | Field | Meaning |
//! |---|---|
//! | `format` | [`FORMAT`], the layout of this line. Checked first: another layout may not even have the fields below |
//! | `sim epoch` | `SIM_EPOCH` of the build that wrote it (DESIGN §9.3): the sim resolves every shot |
//! | `match epoch` | [`MATCH_EPOCH`]: the duel rules and screens, which live in this crate and which `SIM_EPOCH` does not cover |
//! | `seed` | the match seed, raw decimal |
//! | `width` | buffer width the match was generated at; a resumed match keeps it, letterboxed if the device's differs |
//! | `rolling` | `1` if the guns roll after a hit (ROLLING GUNS on the pre-match screen), `0` if not |
//! | `help L`, `help R` | each side's aim help, `1` on |
//! | `n` | how many log entries follow, so a line missing some is never read as a shorter match |
//! | `log` | in order: `A:P` for a shot, the aim as dialled (angle ticks, power ticks) before `Match::fire` puts it on the instruments' grid; `/` for the next round dealt, when the players moved on from a round's result |
//!
//! The line must end in its newline: a line cut anywhere, even inside its
//! last shot, is damaged, never a different match.
//!
//! A new round is its own entry, not implied by the next shot, because the
//! players can be on either side of it: on the round's result plate, or on
//! the next round's first handoff plate. Dealing it is a pure function of
//! the seed, so the entry carries nothing else.
//!
//! # What resuming promises
//!
//! The match rebuilt from its line is the match as it was played, byte for
//! byte: the rules' state ([`versus::Match`]), each side's kept aim and last
//! arc, and the roll the handoff plate announces (`app/disk_tests.rs`
//! kills a match on the handoff plate, mid-turn and mid-flight and checks
//! it). A shot in the air when the app died was already resolved when it
//! was fired, as in a run, so it is in the log and the match resumes with it
//! landed, on the pass-the-phone plate for whoever is next (or on the
//! round's result plate, if that shot won the round). A shot that won the
//! match ends it: a finished match leaves nothing to resume, and its line is
//! cleared the moment that shot is fired.
//!
//! # Epochs
//!
//! A log only replays to its match on rules that resolve it identically.
//! `SIM_EPOCH` covers the sim; the duel's own rules (screen generation, the
//! hit test, winds, rolls, equal shots) are this crate's, so they have their
//! own [`MATCH_EPOCH`], held by a golden match whose final state is pinned
//! ([`MATCH_DIGEST`]). A change to the rules fails that test, which says to
//! bump the epoch. A line from another format or either epoch is thrown away
//! at launch with a note on the title, never replayed.

use salt_line_sim::ballistics::Shot;
use salt_line_sim::dmath::MAX_ANGLE_TICKS;
use salt_line_sim::rng::mix64;
use salt_line_sim::{POWER_TICKS_MAX, SCREEN_W_MAX, SCREEN_W_MIN, SIM_EPOCH};

use crate::resume::{Paths, Stale};
use crate::versus::{self, Match, Phase, Rules, Side};

pub const MATCH_PATH: &str = "user://match.txt";
/// Written in full, then moved over `MATCH_PATH`.
pub const MATCH_TEMP_PATH: &str = "user://match.txt.new";
/// Where the match file is moved while launch rebuilds it: a log that
/// brings the game down costs one crash, not one every launch, as for the
/// run file (`resume`).
pub const MATCH_LOADING_PATH: &str = "user://match-loading.txt";

/// The match file's paths, for `resume`'s file rules.
pub const MATCH_FILES: Paths = Paths {
    main: MATCH_PATH,
    temp: MATCH_TEMP_PATH,
    loading: MATCH_LOADING_PATH,
};

/// First token of the line, so neither the run file nor a run history line
/// (DESIGN §9.2) can be mistaken for it.
const TAG: &str = "match";

/// The line's layout. Bumped when fields are added, moved or reread.
pub const FORMAT: u32 = 1;

/// The duel rules' own epoch: bumped on any change to this crate that
/// changes what a logged match replays to (`crate::versus`'s screens, hit
/// test, winds, rolls or round rules). The pinned golden match
/// (`a_golden_match_still_replays_to_its_digest`) fails until it is.
pub const MATCH_EPOCH: u32 = 1;

/// [`state_hash`] of the golden match at [`MATCH_EPOCH`].
pub const MATCH_DIGEST: u64 = 0x587b_e0ec_ba47_c60e;

/// The longest log read back. A real match is a few dozen entries; this
/// only keeps a damaged or hand-made file from taking minutes to replay.
pub const MAX_ENTRIES: usize = 2000;

/// One step of a match, as the log records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A shot, as dialled: (angle ticks, power ticks). `A:P`.
    Shot(i32, i32),
    /// The next round dealt after a round won. `/`.
    NextRound,
}

/// The match in play as it goes to disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedMatch {
    /// `SIM_EPOCH` of the build that wrote it.
    pub epoch: u32,
    /// [`MATCH_EPOCH`] of the build that wrote it.
    pub match_epoch: u32,
    pub seed: u64,
    /// Buffer width the match was generated at.
    pub width: usize,
    /// The guns roll after a hit.
    pub rolling: bool,
    /// Aim help, by [`Side::index`].
    pub help: [bool; 2],
    /// Every shot and new round, in order.
    pub log: Vec<Entry>,
}

/// The rules a match plays by, given whether its guns roll: the game's, with
/// [`versus::GUN_SHIFT`] off when they do not. The pre-match screen and a
/// replay both build them here, so the two cannot drift.
pub fn rules_for(rolling: bool) -> Rules {
    Rules {
        gun_shift: if rolling { versus::GUN_SHIFT } else { 0.0 },
        ..Rules::DEFAULT
    }
}

impl SavedMatch {
    /// The line for `duel` (its seed, width and rules) with this aim help
    /// and this log, written by this build.
    pub fn of(duel: &Match, help: [bool; 2], log: &[Entry]) -> Self {
        Self {
            epoch: SIM_EPOCH,
            match_epoch: MATCH_EPOCH,
            seed: duel.seed,
            width: duel.width(),
            rolling: duel.rules.gun_shift > 0.0,
            help,
            log: log.to_vec(),
        }
    }

    /// The rules it was played by.
    pub fn rules(&self) -> Rules {
        rules_for(self.rolling)
    }

    /// The module docs' line, newline-terminated.
    pub fn to_line(&self) -> String {
        let flag = |b: bool| u8::from(b);
        let mut line = format!(
            "{TAG} {FORMAT} {} {} {} {} {} {} {} {}",
            self.epoch,
            self.match_epoch,
            self.seed,
            self.width,
            flag(self.rolling),
            flag(self.help[0]),
            flag(self.help[1]),
            self.log.len(),
        );
        for entry in &self.log {
            match entry {
                Entry::Shot(a, p) => line.push_str(&format!(" {a}:{p}")),
                Entry::NextRound => line.push_str(" /"),
            }
        }
        line.push('\n');
        line
    }

    /// Parse [`SavedMatch::to_line`]'s format. The format and both epochs
    /// are checked before anything after them is read.
    pub fn parse(text: &str) -> Result<Self, Stale> {
        // The newline ends the line: without it the last shot may have been
        // cut short (`900:6` for `900:640`), which the count cannot see.
        if text.contains('\0') || !text.ends_with('\n') {
            return Err(Stale::Damaged);
        }
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let line = lines.next().ok_or(Stale::Damaged)?;
        if lines.next().is_some() {
            return Err(Stale::Damaged);
        }
        let mut fields = line.split(' ');
        if fields.next() != Some(TAG) {
            return Err(Stale::Damaged);
        }
        let format: u32 = number(fields.next())?;
        version(format, FORMAT)?;
        let epoch: u32 = number(fields.next())?;
        version(epoch, SIM_EPOCH)?;
        let match_epoch: u32 = number(fields.next())?;
        version(match_epoch, MATCH_EPOCH)?;
        let seed: u64 = number(fields.next())?;
        let width: usize = number(fields.next())?;
        if !(SCREEN_W_MIN..=SCREEN_W_MAX).contains(&width) {
            return Err(Stale::Damaged);
        }
        let rolling = flag(fields.next())?;
        let help = [flag(fields.next())?, flag(fields.next())?];
        let n: usize = number(fields.next())?;
        if n > MAX_ENTRIES {
            return Err(Stale::Damaged);
        }
        let log = fields.map(entry).collect::<Result<Vec<_>, _>>()?;
        if log.len() != n {
            return Err(Stale::Damaged);
        }
        Ok(Self {
            epoch,
            match_epoch,
            seed,
            width,
            rolling,
            help,
            log,
        })
    }

    /// The match rebuilt from its log: each shot fired and each round dealt
    /// as the game does it. `None` if an entry cannot be taken (a shot
    /// while a round's result is up or after the match was won, a new
    /// round with no round won): a damaged log, never something to show
    /// diverging.
    ///
    /// Only the rules' state. The core rebuilds what it shows around it
    /// (each side's kept aim and last arc) by the same steps, through
    /// `VersusPlay::rebuild`.
    pub fn replay(&self) -> Option<Match> {
        let mut duel = Match::with_rules(self.seed, self.width, self.rules());
        for entry in &self.log {
            match *entry {
                Entry::Shot(angle, power) => {
                    duel.fire(Shot::new(angle, power), false)?;
                }
                Entry::NextRound => {
                    if !duel.next_round() {
                        return None;
                    }
                }
            }
        }
        Some(duel)
    }
}

/// `Ok` for this build's `want`; older or newer, the matching [`Stale`].
fn version(got: u32, want: u32) -> Result<(), Stale> {
    match got.cmp(&want) {
        std::cmp::Ordering::Less => Err(Stale::OlderVersion),
        std::cmp::Ordering::Greater => Err(Stale::NewerVersion),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

/// A number written only the way `Display` writes it, as `resume` reads them.
fn number<T: core::str::FromStr + ToString>(tok: Option<&str>) -> Result<T, Stale> {
    let tok = tok.ok_or(Stale::Damaged)?;
    let v: T = tok.parse().map_err(|_| Stale::Damaged)?;
    if v.to_string() == tok {
        Ok(v)
    } else {
        Err(Stale::Damaged)
    }
}

fn flag(tok: Option<&str>) -> Result<bool, Stale> {
    match tok {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err(Stale::Damaged),
    }
}

/// One log entry: `/`, or an `A:P` shot, each within the dial's range.
fn entry(tok: &str) -> Result<Entry, Stale> {
    if tok == "/" {
        return Ok(Entry::NextRound);
    }
    let (a, p) = tok.split_once(':').ok_or(Stale::Damaged)?;
    let a: i32 = number(Some(a))?;
    let p: i32 = number(Some(p))?;
    if !(0..=MAX_ANGLE_TICKS).contains(&a) || !(0..=POWER_TICKS_MAX).contains(&p) {
        return Err(Stale::Damaged);
    }
    Ok(Entry::Shot(a, p))
}

/// What the title says, once, as it throws a match file away.
pub fn note(why: Stale) -> &'static str {
    match why {
        Stale::OlderVersion => "SAVED MATCH FROM AN OLDER VERSION",
        Stale::NewerVersion => "SAVED MATCH FROM A NEWER VERSION",
        Stale::Damaged | Stale::Diverged => "SAVED MATCH COULD NOT BE RESTORED",
    }
}

/// Whether a match file's text is a whole copy, for choosing between the
/// file and its temporary file (`resume::take_aside`): one well-formed line,
/// of this build or another.
pub fn sound(text: &str) -> bool {
    matches!(
        SavedMatch::parse(text),
        Ok(_) | Err(Stale::OlderVersion | Stale::NewerVersion)
    )
}

/// What to do with the match file at launch.
#[derive(Debug)]
pub enum Found {
    /// No file, or a blank one.
    Nothing,
    /// Offer RESUME MATCH.
    Resume(SavedMatch),
    /// The last shot won the match, and the app died before its line was
    /// cleared: nothing to resume or keep.
    Finished,
    /// Throw it away and say why.
    Discard(Stale),
}

/// Decide what the match file's text is worth.
pub fn judge(text: Option<&str>) -> Found {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Found::Nothing;
    };
    let saved = match SavedMatch::parse(text) {
        Ok(s) => s,
        Err(why) => return Found::Discard(why),
    };
    match saved.replay() {
        None => Found::Discard(Stale::Diverged),
        Some(duel) if matches!(duel.phase, Phase::MatchOver(_)) => Found::Finished,
        Some(_) => Found::Resume(saved),
    }
}

/// A fingerprint of a match's whole state, every float by its bits: the
/// field, the guns' spot, the wind, the score, whose turn and the phase,
/// and the landing marks. Equal only for matches that are the same bit for
/// bit, which is what [`MATCH_DIGEST`] pins.
pub fn state_hash(duel: &Match) -> u64 {
    let mut h = 0x5A17_11E5_0000_0001u64;
    let mut eat = |x: u64| h = mix64(h ^ x);
    let s = &duel.screen;
    for i in 0..s.field.len() {
        eat(s.field.base[i].to_bits());
        eat(s.field.scar[i].to_bits());
        eat(s.field.material[i] as u64);
    }
    eat(s.muzzle_x.to_bits());
    eat(s.wind.to_bits());
    eat(s.seed);
    eat(duel.round as u64);
    eat(duel.turn as u64);
    eat(duel.to_move.index() as u64);
    for side in Side::BOTH {
        eat(duel.hits[side.index()] as u64);
        eat(duel.rounds[side.index()] as u64);
        match duel.landing[side.index()] {
            Some(p) => {
                eat(1);
                eat(p.x.to_bits());
                eat(p.y.to_bits());
            }
            None => eat(0),
        }
    }
    eat(match duel.phase {
        Phase::Aiming => 0,
        Phase::RoundOver(s) => 1 + s.index() as u64,
        Phase::MatchOver(s) => 3 + s.index() as u64,
    });
    eat(duel.roll_due as u64);
    h
}

#[cfg(test)]
mod tests;

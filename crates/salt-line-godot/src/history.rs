//! Finished runs and matches, kept to watch again. DESIGN §9.
//!
//! A run is a seed and a shot log, and a match is a seed, its pre-match
//! choices and its shots (DESIGN §3, §11.6), so a finished one is kept the
//! way the run and match files keep one in play: as text, a line each, in
//! `user://history.txt`. WATCH (`app/watch.rs`) feeds a line's shots back
//! through the game's own run and versus code, so what is shown is the
//! game as it was played, not a recording of the screen.
//!
//! Engine-free, like `resume` and `match_log`: the line format, the cap,
//! and the check that a line still replays to what it says live here,
//! where they can be tested headless. The core (`app/watch.rs`) asks for
//! the file to be written as an `app::Effect::History`; `game.rs` writes it
//! whole through `save::write`, atomically, as it writes the slot file.
//!
//! # The file
//!
//! ```text
//! history 1
//! run 1 <day> <sim epoch> <seed> <width> <daily> <screens> <shells> <shots> <log>
//! run 1 20369 6 1234567 360 0 9 0 41 v2 @0 F0 900:600 870:640 …
//! duel 1 <day> <opponent> <ground> <winner> <rounds L> <rounds R> <match line>
//! duel 1 20369 easy - L 2 1 match 3 6 1 99 360 1 1 0 easy - 31 900:600 …
//! ```
//!
//! A header naming the file's [`FORMAT`], then one line per finished run or
//! match, oldest first, at most [`MAX_RECORDS`] of them (one list for both
//! kinds: see "Why one list"). Each line starts with its kind and its own
//! [`LINE_FORMAT`], so a line read on its own (a share code, later: DESIGN
//! §9.6) still says how to read it. Then:
//!
//! | Field | Meaning |
//! |---|---|
//! | `day` | days since the Unix epoch when it finished, for the list's date; `0` if the clock was not known |
//! | `sim epoch` | a run's `SIM_EPOCH` (DESIGN §9.3). A match's epochs are in its match line |
//! | `seed`, `width`, `daily` | the run's identity, as the run file has them (`resume`) |
//! | `screens`, `shells`, `shots` | the run's result, as the board records it |
//! | `log` | the run's shot log, `replay::encode_log`'s text (it starts with its own format, `v2`), to the end of the line |
//! | `opponent` | `2p` for two players, or the AI's level (`easy`, `medium`, `hard`) |
//! | `ground` | `-` for generated screens, `=` for a map with no name, else the map's name (A–Z, 0–9, as the keypad types it) |
//! | `winner`, `rounds L`, `rounds R` | who took the match (`L` or `R`) and the rounds each side won |
//! | `match line` | the match as the match file had it (`match_log`'s line, all three of its versions included), to the end of the line |
//!
//! The fields a list row shows (the date, the kind, the result, the seed,
//! the opponent and the map) are read the same way whatever build wrote
//! the line. The log and the match line are only taken apart by a build of
//! the same epochs; another build's are kept verbatim and written back.
//!
//! # Epochs
//!
//! A log only replays to its run on a sim that resolves it identically
//! (DESIGN §9.3). A line from another epoch is still listed, with its date,
//! seed and result, but cannot be watched, and its row says so; it is never
//! replayed, let alone shown diverging. Before anything is watched, the
//! line is rebuilt off screen and the result compared with the one it
//! records ([`Record::check`]): a line that does not rebuild to its own
//! result is refused, whatever its epoch says.
//!
//! # Durability
//!
//! The file is written whole (`durable::write_atomic` through
//! `save::write`) each time a run or match is added, never appended to, so a
//! crash leaves the old file or the new one. Reading is total: a line this
//! build cannot read (torn, hand-edited, NULs where a crash left them) is
//! dropped and counted ([`History::damaged`]), and the list says so; the
//! next write leaves it out. A file whose header is from a newer build is
//! read for what this build understands and never written
//! ([`History::from_the_future`]), as the save and the slot file are not:
//! what is finished meanwhile is kept for the session only.
//!
//! # Why one list
//!
//! Runs and matches share one list of [`MAX_RECORDS`], newest first, rather
//! than a cap per kind. It is a family's list of "what did we play
//! lately", read top down by a child; two lists would mean a choice before
//! every look. The cost: a long afternoon of matches pushes the last run
//! off the end. Twenty is a session or two of either.

use salt_line_sim::layout::{LayoutName, NAME_MAX};
use salt_line_sim::replay::{decode_log, encode_log, Replay};
use salt_line_sim::run::{format_seed, RunState};
use salt_line_sim::{SCREEN_W_MAX, SCREEN_W_MIN, SIM_EPOCH};

use crate::match_log::SavedMatch;
use crate::opponent::Level;
use crate::resume::Stale;
use crate::versus::{self, Match, Side};

pub const HISTORY_PATH: &str = "user://history.txt";
/// Written in full, then moved over `HISTORY_PATH`.
pub const HISTORY_TEMP_PATH: &str = "user://history.txt.new";

/// The file's format, its header's number. A build that changes any line
/// bumps this too, so an older build reads the file as from the future and
/// leaves it alone.
pub const FORMAT: u32 = 1;
/// Each line's own format, after its kind.
pub const LINE_FORMAT: u32 = 1;

/// Finished runs and matches kept, both kinds together, oldest dropped
/// first. A list a family pages through; see "Why one list".
pub const MAX_RECORDS: usize = 20;

/// The longest file read back: twenty long runs (a few kilobytes each)
/// with room to spare. Anything larger is damage, not history.
pub const MAX_FILE_CHARS: usize = 512 * 1024;

const HEADER: &str = "history";
const RUN: &str = "run";
const DUEL: &str = "duel";

/// A finished run, as its line holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRecord {
    /// `SIM_EPOCH` of the build that played it.
    pub epoch: u32,
    pub seed: u64,
    /// Buffer width it was generated at: a replay must use it.
    pub width: usize,
    pub daily: bool,
    /// Screens cleared, shells left, shots fired: `RunState::score`.
    pub score: (u32, i32, u32),
    /// The shot log's text (`encode_log`), verbatim: another epoch's may
    /// not be this build's to decode.
    pub log: String,
}

/// The ground a match was played on, as a list row names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    /// Generated screens.
    Generated,
    /// A map (a layout, DESIGN §13.9), by its name; empty for a map with
    /// no name.
    Map(String),
}

/// A finished match, as its line holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuelRecord {
    /// The AI's level, if the right gun was the computer's.
    pub ai: Option<Level>,
    pub ground: Ground,
    /// Who took the match.
    pub winner: Side,
    /// Rounds each side won, by [`Side::index`].
    pub rounds: [u32; 2],
    /// The match file's line for it (`SavedMatch::to_line`) without its
    /// newline, verbatim: another epoch's may not be this build's to read.
    pub line: String,
}

/// What a line holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Run(RunRecord),
    Duel(DuelRecord),
}

/// One finished run or match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Days since the Unix epoch when it finished; 0 if not known.
    pub day: u64,
    pub kind: Kind,
}

/// Why a record cannot be watched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Played on an earlier sim or duel rules: it would replay as something
    /// else (DESIGN §9.3).
    OlderVersion,
    /// Played on a newer build than this one.
    NewerVersion,
    /// Same epochs, but it does not rebuild to the result it records: a
    /// determinism bug or a hand edit, never something to show.
    Mismatch,
}

impl Refused {
    /// What the list says when the row is chosen.
    pub fn note(self) -> &'static str {
        match self {
            Refused::OlderVersion => "MADE BY AN OLDER VERSION - CANNOT WATCH",
            Refused::NewerVersion => "MADE BY A NEWER VERSION - CANNOT WATCH",
            Refused::Mismatch => "THIS REPLAY DOES NOT MATCH - NOT SHOWN",
        }
    }

    fn of(stale: Stale) -> Refused {
        match stale {
            Stale::OlderVersion => Refused::OlderVersion,
            Stale::NewerVersion => Refused::NewerVersion,
            Stale::Damaged | Stale::Diverged => Refused::Mismatch,
        }
    }
}

/// What WATCH plays, once a record has passed [`Record::check`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Watchable {
    /// A run: its seed, width and log, and the score it must end on.
    Run(Replay, (u32, i32, u32)),
    /// A match: its line, and who must take it with how many rounds each.
    Duel(SavedMatch, Side, [u32; 2]),
}

impl Record {
    /// The finished `run` (one whose log rebuilds it: never a
    /// `--salt-screen` jump), on `day`.
    pub fn run(day: u64, run: &RunState, daily: bool) -> Record {
        Record {
            day,
            kind: Kind::Run(RunRecord {
                epoch: SIM_EPOCH,
                seed: run.run_seed,
                width: run.width,
                daily,
                score: run.score(),
                log: encode_log(&run.log),
            }),
        }
    }

    /// The won match `duel`, kept as `saved`, on `ground`, on `day`. `None`
    /// while the match is still to be won.
    pub fn duel(day: u64, saved: &SavedMatch, duel: &Match, ground: Ground) -> Option<Record> {
        let versus::Phase::MatchOver(winner) = duel.phase else {
            return None;
        };
        Some(Record {
            day,
            kind: Kind::Duel(DuelRecord {
                ai: saved.ai,
                ground,
                winner,
                rounds: duel.rounds,
                line: saved.to_line().trim_end().to_string(),
            }),
        })
    }

    /// Whether this build can watch it: `Ok` for the same epochs, else
    /// which way it differs. Cheap: nothing is replayed. [`Record::check`]
    /// is the full test.
    pub fn version(&self) -> Result<(), Refused> {
        match &self.kind {
            Kind::Run(r) => match r.epoch.cmp(&SIM_EPOCH) {
                std::cmp::Ordering::Less => Err(Refused::OlderVersion),
                std::cmp::Ordering::Greater => Err(Refused::NewerVersion),
                std::cmp::Ordering::Equal => Ok(()),
            },
            Kind::Duel(d) => match SavedMatch::parse(&format!("{}\n", d.line)) {
                Ok(_) => Ok(()),
                Err(why) => Err(Refused::of(why)),
            },
        }
    }

    /// Rebuild it off screen and compare the result with the one it
    /// records: what WATCH plays, or why it will not. A run must end, with
    /// its score; a match must be won, by the side and the rounds recorded,
    /// against the same opponent. Anything else is [`Refused::Mismatch`],
    /// and is never shown.
    pub fn check(&self) -> Result<Watchable, Refused> {
        self.version()?;
        match &self.kind {
            Kind::Run(r) => {
                let log = decode_log(&r.log).map_err(|_| Refused::Mismatch)?;
                let replay = Replay {
                    run_seed: r.seed,
                    width: r.width,
                    log,
                };
                let rebuilt = replay.reconstruct().map_err(|_| Refused::Mismatch)?;
                if !rebuilt.is_over() || rebuilt.score() != r.score {
                    return Err(Refused::Mismatch);
                }
                Ok(Watchable::Run(replay, r.score))
            }
            Kind::Duel(d) => {
                let saved = SavedMatch::parse(&format!("{}\n", d.line)).map_err(Refused::of)?;
                if saved.ai != d.ai || saved.layout().is_err() {
                    return Err(Refused::Mismatch);
                }
                let duel = saved.replay().ok_or(Refused::Mismatch)?;
                if duel.phase != versus::Phase::MatchOver(d.winner) || duel.rounds != d.rounds {
                    return Err(Refused::Mismatch);
                }
                Ok(Watchable::Duel(saved, d.winner, d.rounds))
            }
        }
    }

    /// The record's line, newline-terminated: the module docs' format.
    pub fn to_line(&self) -> String {
        match &self.kind {
            Kind::Run(r) => format!(
                "{RUN} {LINE_FORMAT} {} {} {} {} {} {} {} {} {}\n",
                self.day,
                r.epoch,
                r.seed,
                r.width,
                u8::from(r.daily),
                r.score.0,
                r.score.1,
                r.score.2,
                r.log
            ),
            Kind::Duel(d) => format!(
                "{DUEL} {LINE_FORMAT} {} {} {} {} {} {} {}\n",
                self.day,
                d.ai.map_or("2p", Level::token),
                ground_token(&d.ground),
                match d.winner {
                    Side::Left => "L",
                    Side::Right => "R",
                },
                d.rounds[0],
                d.rounds[1],
                d.line
            ),
        }
    }

    /// Read one line of [`Record::to_line`]'s format. `None` for anything
    /// else: torn, edited, NULs, or a line format this build does not know.
    pub fn parse(line: &str) -> Option<Record> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.contains('\0') || line.contains('\n') {
            return None;
        }
        let mut f = line.splitn(3, ' ');
        let tag = f.next()?;
        if number::<u32>(f.next())? != LINE_FORMAT {
            return None;
        }
        let rest = f.next()?;
        match tag {
            RUN => Self::parse_run(rest),
            DUEL => Self::parse_duel(rest),
            _ => None,
        }
    }

    fn parse_run(rest: &str) -> Option<Record> {
        let mut f = rest.splitn(9, ' ');
        let day = number(f.next())?;
        let epoch: u32 = number(f.next())?;
        let seed = number(f.next())?;
        let width: usize = number(f.next())?;
        if !(SCREEN_W_MIN..=SCREEN_W_MAX).contains(&width) {
            return None;
        }
        let daily = flag(f.next())?;
        let score = (number(f.next())?, number(f.next())?, number(f.next())?);
        let log = f.next()?.to_string();
        // This build's log must decode; another epoch's is kept as it is,
        // as long as it is one line of tokens.
        if epoch == SIM_EPOCH {
            decode_log(&log).ok()?;
        } else if log.trim().is_empty() || log != log.trim() {
            return None;
        }
        Some(Record {
            day,
            kind: Kind::Run(RunRecord {
                epoch,
                seed,
                width,
                daily,
                score,
                log,
            }),
        })
    }

    fn parse_duel(rest: &str) -> Option<Record> {
        let mut f = rest.splitn(7, ' ');
        let day = number(f.next())?;
        let ai = match f.next()? {
            "2p" => None,
            token => Some(Level::from_token(token)?),
        };
        let ground = match f.next()? {
            "-" => Ground::Generated,
            "=" => Ground::Map(String::new()),
            name => {
                let parsed = LayoutName::parse(name)?;
                if parsed.as_str() != name || name.len() > NAME_MAX {
                    return None;
                }
                Ground::Map(name.to_string())
            }
        };
        let winner = match f.next()? {
            "L" => Side::Left,
            "R" => Side::Right,
            _ => return None,
        };
        let rounds = [number(f.next())?, number(f.next())?];
        let line = f.next()?.to_string();
        let record = Record {
            day,
            kind: Kind::Duel(DuelRecord {
                ai,
                ground,
                winner,
                rounds,
                line,
            }),
        };
        // The match line must be one: this build's whole, another's at
        // least another version's (it is read no further).
        match record.version() {
            Ok(()) | Err(Refused::OlderVersion | Refused::NewerVersion) => Some(record),
            Err(Refused::Mismatch) => None,
        }
    }

    /// One row of the list, newest first: the date, what it was, how it
    /// ended. Words a child can read out (DESIGN §11.4).
    pub fn summary(&self) -> String {
        let date = date(self.day).map(|d| format!("{d}  ")).unwrap_or_default();
        match &self.kind {
            Kind::Run(r) => {
                let screens = match r.score.0 {
                    1 => "1 SCREEN".to_string(),
                    n => format!("{n} SCREENS"),
                };
                if r.daily {
                    format!("{date}DAILY RUN  {screens}")
                } else {
                    format!("{date}RUN  {screens}  SEED {}", format_seed(r.seed))
                }
            }
            Kind::Duel(d) => {
                let (who, won) = match (d.ai, d.winner) {
                    (Some(level), Side::Left) => (format!("AI {}", level.name()), "YOU WON"),
                    (Some(level), Side::Right) => (format!("AI {}", level.name()), "AI WON"),
                    (None, Side::Left) => ("2 PLAYERS".to_string(), "LEFT WON"),
                    (None, Side::Right) => ("2 PLAYERS".to_string(), "RIGHT WON"),
                };
                let map = match &d.ground {
                    Ground::Generated => String::new(),
                    Ground::Map(name) if name.is_empty() => "  MAP".into(),
                    Ground::Map(name) => format!("  {name}"),
                };
                format!("{date}{who}  {won} {}-{}{map}", d.rounds[0], d.rounds[1])
            }
        }
    }
}

fn ground_token(ground: &Ground) -> &str {
    match ground {
        Ground::Generated => "-",
        Ground::Map(name) if name.is_empty() => "=",
        Ground::Map(name) => name,
    }
}

/// `day` (days since 1970-01-01) as the list shows it, `OCT 8`; `None`
/// for 0, a clock that was not known. Howard Hinnant's `civil_from_days`,
/// for the proleptic Gregorian calendar, in whole numbers.
pub fn date(day: u64) -> Option<String> {
    if day == 0 {
        return None;
    }
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    Some(format!("{} {d}", MONTHS[(m - 1) as usize]))
}

/// A number written only the way `Display` writes it, as the run and
/// match files read theirs: one spelling per value.
fn number<T: core::str::FromStr + ToString>(tok: Option<&str>) -> Option<T> {
    let tok = tok?;
    let v: T = tok.parse().ok()?;
    (v.to_string() == tok).then_some(v)
}

fn flag(tok: Option<&str>) -> Option<bool> {
    match tok? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// The history file in memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Oldest first, at most [`MAX_RECORDS`].
    pub records: Vec<Record>,
    /// Lines that could not be read, and were dropped: the list says so,
    /// and the next write leaves them out.
    pub damaged: usize,
    /// The header is from a newer build: read for what this build
    /// understands, never written.
    pub from_the_future: bool,
}

impl History {
    /// Read the file. Total: any text is a `History`, a line it cannot
    /// read dropped and counted rather than a reason to lose the rest.
    pub fn parse(text: &str) -> History {
        let mut out = History::default();
        if text.len() > MAX_FILE_CHARS {
            out.damaged = 1;
            return out;
        }
        let mut lines = text
            .split('\n')
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.trim().is_empty())
            .peekable();
        // The header, if it is there. A file without one is read as this
        // format (it can only be a hand-made one).
        if let Some(first) = lines.peek() {
            let mut f = first.split(' ');
            if f.next() == Some(HEADER) {
                match (number::<u32>(f.next()), f.next()) {
                    (Some(v), _) if v > FORMAT => out.from_the_future = true,
                    (Some(_), None) => {}
                    _ => out.damaged += 1,
                }
                lines.next();
            }
        }
        for line in lines {
            match Record::parse(line) {
                Some(r) => out.records.push(r),
                // A newer build's line is not damage: it is just not this
                // build's to read, and this build never writes over it.
                None if out.from_the_future => {}
                None => out.damaged += 1,
            }
        }
        // A hand-made file may hold more than the cap: the newest are kept.
        let over = out.records.len().saturating_sub(MAX_RECORDS);
        out.records.drain(..over);
        out
    }

    /// The file's text: the header, then a line per record, oldest first.
    pub fn serialize(&self) -> String {
        let mut s = format!("{HEADER} {FORMAT}\n");
        for r in &self.records {
            s.push_str(&r.to_line());
        }
        s
    }

    /// Add a finished run or match as the newest, dropping the oldest past
    /// [`MAX_RECORDS`]. Returns the file's new text, or `None` if the file
    /// is a newer build's and must not be written: the record is kept in
    /// memory, for this session.
    pub fn push(&mut self, record: Record) -> Option<String> {
        self.records.push(record);
        let over = self.records.len().saturating_sub(MAX_RECORDS);
        self.records.drain(..over);
        // Written now, the damaged lines are gone from disk too.
        self.damaged = 0;
        (!self.from_the_future).then(|| self.serialize())
    }

    /// The records newest first, as the list shows them.
    pub fn newest_first(&self) -> impl Iterator<Item = &Record> {
        self.records.iter().rev()
    }
}

/// Whether the file's text is a whole copy, for choosing between the file
/// and its temporary file at launch (`durable::pick_by`): no NULs, and a
/// header line, of this build or another.
pub fn sound(text: &str) -> bool {
    !text.contains('\0')
        && text
            .lines()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.split(' ').next() == Some(HEADER))
}

#[cfg(test)]
pub(crate) mod tests;

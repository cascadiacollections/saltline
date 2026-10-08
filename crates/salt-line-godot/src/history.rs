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
//! star
//! run 1 20370 6 7654321 360 0 4 0 30 v2 @0 F0 880:610 …
//! ```
//!
//! A header naming the file's format ([`FORMAT_PLAIN`], or [`FORMAT`] once
//! anything is starred: see "Stars"), then one line per finished run or
//! match, oldest first: at most [`MAX_RUNS`] runs and [`MAX_MATCHES`]
//! matches that are not starred, and [`MAX_STARS`] starred ones (see "One
//! list, two caps"). A starred record has a line saying `star` just before
//! its own. Each record's line starts with its kind and its own
//! [`LINE_FORMAT`], so a line read on its own (a share code, later: DESIGN
//! §9.7) still says how to read it. Then:
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
//! crash leaves the old file or the new one. It is kept under
//! [`MAX_FILE_CHARS`] by dropping the oldest records as it is written.
//!
//! Reading is total and line by line. The header is read first: a file
//! whose header is from a newer build is read for what this build
//! understands and never written ([`History::from_the_future`]), as the save
//! and the slot file are not; what is finished meanwhile is kept for the
//! session only. Then each line on its own: one this build cannot read
//! (torn, hand-edited, NULs where a crash left them, or longer than
//! [`MAX_LINE_CHARS`]) is dropped and counted ([`History::damaged`]), a line
//! of a newer line format is dropped and counted apart
//! ([`History::newer`]), and the list says so; the next write leaves them
//! out. No size of file costs the good lines in it.
//!
//! # One list, two caps
//!
//! Runs and matches are shown as one list, the starred first (see
//! "Stars"), then the rest newest first: a family's "what did we play
//! lately", read top down by a child, with no choice to make before every
//! look. "Newest" is the file's order, which is the order records were
//! added: a record whose star is taken off counts as added again. But each kind has its own cap (owner decision,
//! October 2026), and a new record pushes out the oldest of its own kind,
//! so an afternoon of matches never costs the last run, nor a long run
//! session the last match.
//!
//! # Stars
//!
//! A family can star a few favourites so they are never pushed off (owner
//! decision, October 2026). A starred record is exempt from the caps (they
//! count unstarred records only) and is the last to go when a write trims
//! the file to [`MAX_FILE_CHARS`]: every unstarred record goes first,
//! oldest first. At most [`MAX_STARS`] are starred, so the file stays
//! bounded (at most [`MAX_RECORDS`] records); starring one more is refused
//! with [`StarError::Full`], and the list says to take one off first.
//!
//! **Taking a star off counts as adding the record again** (owner decision,
//! October 2026): it moves to the end of the file, the newest place of its
//! kind, so it survives 20 more games of its kind, across launches too,
//! since the file's order is what is kept. Its line is untouched: the date
//! it shows is still the day it was played. In the list it moves from the
//! starred, at the top, to just below them: the first of the rest, where
//! the eye already is, so a tap by mistake is easy to undo. Like any record
//! added, it may push the oldest unstarred one of its kind past the cap.
//!
//! The star is the player's own, not part of the game: it is kept on a line
//! of its own before the record's (`star`), never inside it, so the record
//! line a share code will carry (DESIGN §9.7) is the same starred or not.
//!
//! **Older builds.** A build of [`FORMAT_PLAIN`] reads a `star` line as
//! damage and drops it, and would drop the starred records past the caps on
//! its next write. So a file holding a star says [`FORMAT`] in its header:
//! an older build reads it as from a newer build and never writes it, as
//! for any newer build's file. It does still cap what it read in memory, so
//! its list shows only the newest 20 of each kind, unstarred: an old
//! starred record can be missing from that build's list, but it is still in
//! the file, and this build shows it again. The cost: while a star is kept, an older build installed over this
//! one keeps what it finishes for the session only. A file with no star
//! says [`FORMAT_PLAIN`], and an older build reads and writes it as ever.

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
/// leaves it alone. 2 added `star` lines; a file with none is written as
/// [`FORMAT_PLAIN`], which is all a format-1 build needs to read it.
pub const FORMAT: u32 = 2;
/// The header of a file holding no star: format 1's, unchanged, so a build
/// from before stars still reads and writes it (see "Stars").
pub const FORMAT_PLAIN: u32 = 1;
/// Each line's own format, after its kind.
pub const LINE_FORMAT: u32 = 1;

/// Finished runs kept; a new one pushes out the oldest run.
pub const MAX_RUNS: usize = 20;
/// Finished matches kept; a new one pushes out the oldest match.
pub const MAX_MATCHES: usize = 20;
/// Starred records, of both kinds together. Exempt from the caps, so the
/// file's bound grows by this many. Five: a number a child can count on one
/// hand, a few favourites rather than a second list; and fewer than the
/// rows of a touchscreen's page (`app::watch::LIST_ROWS_TOUCH`), so with the
/// starred listed first the newest game is still on the first page.
pub const MAX_STARS: usize = 5;
/// All together: the most the list ever shows once read.
pub const MAX_RECORDS: usize = MAX_RUNS + MAX_MATCHES + MAX_STARS;

/// The longest line read. The longest a line can honestly be is a match on
/// a map: its layout's code (at most `layout::MAX_CODE_CHARS`, 4,096) and
/// up to `match_log::MAX_ENTRIES` (2,000) log entries of at most 10
/// characters, about 24,300 characters with the fields. Runs are shorter:
/// about 8 characters a shot, a few hundred shots at most. Anything longer
/// is damage.
pub const MAX_LINE_CHARS: usize = 32 * 1024;

/// The longest file written: [`History::push`] drops the oldest records
/// until the text fits. Real lines are a few hundred characters (a run, a
/// match on generated screens) to about 5,000 (a match on a map's code), so
/// a full file is about 10–30 KB and never comes near it; forty worst-case
/// map-match lines would, and lose their oldest few. Reading has no limit
/// of its own: lines are judged one at a time ([`MAX_LINE_CHARS`]).
pub const MAX_FILE_CHARS: usize = 512 * 1024;

const HEADER: &str = "history";
pub(crate) const STAR: &str = "star";
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
    /// [`Record::version`]'s answer, worked out once when the record is
    /// made: a match's needs its line parsed, and the list asks every frame
    /// it draws.
    pub(crate) verdict: Result<(), Refused>,
    /// Starred by the player: kept past the caps (see "Stars"). Not part
    /// of the record's line ([`Record::to_line`]), which is the game's.
    pub starred: bool,
}

/// Why a star cannot be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StarError {
    /// [`MAX_STARS`] are starred already.
    Full,
    /// The record is not in the history and cannot be: its line is too
    /// long to read back ([`MAX_LINE_CHARS`]).
    NotKept,
}

impl StarError {
    /// What the list or the replay's end says. Words a child can read out.
    pub fn note(self) -> String {
        match self {
            StarError::Full => format!("{MAX_STARS} STARS - TAKE ONE OFF FIRST"),
            StarError::NotKept => "TOO LONG TO KEEP - CANNOT STAR".to_string(),
        }
    }
}

/// Why a line is not read as a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineError {
    /// Torn, edited, NULs, too long: not a line of this format.
    Damaged,
    /// A line format newer than [`LINE_FORMAT`]: a newer build's, which
    /// this one cannot read but can say so (a share code from a newer
    /// version, later).
    Newer,
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
    /// A run: its seed, width and log, the score it must end on, and
    /// whether it was the daily.
    Run(Replay, (u32, i32, u32), bool),
    /// A match: its line, and who must take it with how many rounds each.
    Duel(SavedMatch, Side, [u32; 2]),
}

impl Record {
    /// A record of `kind` on `day`, its verdict worked out.
    pub fn new(day: u64, kind: Kind) -> Record {
        let verdict = verdict_of(&kind);
        Record {
            day,
            kind,
            verdict,
            starred: false,
        }
    }

    /// The same run or match as `other`, whatever day each was recorded:
    /// the same seed and log, or the same match line. Never recorded twice.
    pub fn same_game(&self, other: &Record) -> bool {
        match (&self.kind, &other.kind) {
            (Kind::Run(a), Kind::Run(b)) => {
                (a.epoch, a.seed, a.width, &a.log) == (b.epoch, b.seed, b.width, &b.log)
            }
            (Kind::Duel(a), Kind::Duel(b)) => a.line == b.line,
            _ => false,
        }
    }

    /// Whether this is a run (else a match), for the caps.
    pub fn is_run(&self) -> bool {
        matches!(self.kind, Kind::Run(_))
    }

    /// The finished `run` (one whose log rebuilds it: never a
    /// `--salt-screen` jump), on `day`.
    pub fn run(day: u64, run: &RunState, daily: bool) -> Record {
        Record::new(
            day,
            Kind::Run(RunRecord {
                epoch: SIM_EPOCH,
                seed: run.run_seed,
                width: run.width,
                daily,
                score: run.score(),
                log: encode_log(&run.log),
            }),
        )
    }

    /// The won match `duel`, kept as `saved`, on `ground`, on `day`. `None`
    /// while the match is still to be won.
    pub fn duel(day: u64, saved: &SavedMatch, duel: &Match, ground: Ground) -> Option<Record> {
        let versus::Phase::MatchOver(winner) = duel.phase else {
            return None;
        };
        Some(Record::new(
            day,
            Kind::Duel(DuelRecord {
                ai: saved.ai,
                ground,
                winner,
                rounds: duel.rounds,
                line: saved.to_line().trim_end().to_string(),
            }),
        ))
    }

    /// Whether this build can watch it: `Ok` for the same epochs, else
    /// which way it differs. Kept from when the record was made, so free to
    /// ask every frame. [`Record::check`] is the full test.
    pub fn version(&self) -> Result<(), Refused> {
        self.verdict
    }

    /// Rebuild it off screen and compare the result with the one it
    /// records: what WATCH plays, or why it will not. A run must end, with
    /// its score; a match must be won, by the side and the rounds recorded,
    /// against the same opponent. Anything else is [`Refused::Mismatch`],
    /// and is never shown.
    pub fn check(&self) -> Result<Watchable, Refused> {
        // Afresh: the kept verdict is for the list, this is for the screen.
        verdict_of(&self.kind)?;
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
                Ok(Watchable::Run(replay, r.score, r.daily))
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

    /// The record's line, newline-terminated: the module docs' format. The
    /// game's alone: a star is not in it.
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

    /// Read one line of [`Record::to_line`]'s format: torn, edited, NULs
    /// or too long is [`LineError::Damaged`]; a newer line format, of any
    /// kind, [`LineError::Newer`].
    pub fn parse(line: &str) -> Result<Record, LineError> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.len() > MAX_LINE_CHARS || line.contains('\0') || line.contains('\n') {
            return Err(LineError::Damaged);
        }
        let mut f = line.splitn(3, ' ');
        let tag = f.next().ok_or(LineError::Damaged)?;
        let format: u32 = number(f.next()).ok_or(LineError::Damaged)?;
        if format > LINE_FORMAT {
            return Err(LineError::Newer);
        }
        let rest = f.next().filter(|_| format == LINE_FORMAT);
        let record = match (tag, rest) {
            (RUN, Some(rest)) => Self::parse_run(rest),
            (DUEL, Some(rest)) => Self::parse_duel(rest),
            _ => None,
        };
        record.ok_or(LineError::Damaged)
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
        Some(Record::new(
            day,
            Kind::Run(RunRecord {
                epoch,
                seed,
                width,
                daily,
                score,
                log,
            }),
        ))
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
        let record = Record::new(
            day,
            Kind::Duel(DuelRecord {
                ai,
                ground,
                winner,
                rounds,
                line,
            }),
        );
        // The match line must be one: this build's whole, another's at
        // least another version's (it is read no further).
        match record.version() {
            Ok(()) | Err(Refused::OlderVersion | Refused::NewerVersion) => Some(record),
            Err(Refused::Mismatch) => None,
        }
    }

    /// One row of the list: the date, what it was, how it
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

/// Whether this build can watch a record of `kind`: the same epochs, or
/// which way they differ. A match's needs its line parsed.
fn verdict_of(kind: &Kind) -> Result<(), Refused> {
    match kind {
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
    /// Oldest first, at most [`MAX_RUNS`] runs and [`MAX_MATCHES`] matches.
    pub records: Vec<Record>,
    /// Lines that could not be read, and were dropped: the list says so,
    /// and the next write leaves them out.
    pub damaged: usize,
    /// Lines of a newer line format, dropped the same way but said apart.
    pub newer: usize,
    /// The header is from a newer build: read for what this build
    /// understands, never written.
    pub from_the_future: bool,
}

impl History {
    /// Read the file. Total: any text is a `History`, and a line it cannot
    /// read is dropped and counted rather than a reason to lose the rest,
    /// however large the file.
    pub fn parse(text: &str) -> History {
        let mut out = History::default();
        let mut lines = text
            .split('\n')
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.trim().is_empty())
            .peekable();
        // The header first, if it is there. A file without one is read as
        // this format (it can only be a hand-made one).
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
        // A `star` line marks the record on the line after it. One with no
        // line after it, or another `star`, marks nothing: damage.
        let mut star = false;
        for line in lines {
            if line == STAR {
                if star && !out.from_the_future {
                    out.damaged += 1;
                }
                star = true;
                continue;
            }
            let starred = std::mem::take(&mut star);
            match Record::parse(line) {
                Ok(r) => out.records.push(Record { starred, ..r }),
                // A newer build's file is never written, so its lines are
                // not lost: they are only not this build's to read.
                Err(_) if out.from_the_future => {}
                Err(LineError::Newer) => out.newer += 1,
                Err(LineError::Damaged) => out.damaged += 1,
            }
        }
        if star && !out.from_the_future {
            out.damaged += 1;
        }
        // A hand-made file may hold more than the caps or the stars: the
        // newest stars and the newest of each kind are kept.
        let mut stars = 0;
        for r in out.records.iter_mut().rev() {
            if r.starred {
                stars += 1;
                r.starred = stars <= MAX_STARS;
            }
        }
        out.cap();
        out
    }

    /// Drop the oldest unstarred records of each kind past its cap.
    fn cap(&mut self) {
        let (mut runs, mut matches) = (0, 0);
        let mut keep: Vec<bool> = self
            .records
            .iter()
            .rev()
            .map(|r| {
                if r.starred {
                    return true;
                }
                let n = if r.is_run() { &mut runs } else { &mut matches };
                *n += 1;
                *n <= if r.is_run() { MAX_RUNS } else { MAX_MATCHES }
            })
            .collect();
        keep.reverse();
        let mut kept = keep.into_iter();
        self.records.retain(|_| kept.next().unwrap_or(false));
    }

    /// The file's text: the header, then a line per record, oldest first,
    /// each starred one after a `star` line. The header says [`FORMAT`] only
    /// if something is starred (see "Stars").
    pub fn serialize(&self) -> String {
        let format = if self.stars() > 0 {
            FORMAT
        } else {
            FORMAT_PLAIN
        };
        let mut s = format!("{HEADER} {format}\n");
        for r in &self.records {
            if r.starred {
                s.push_str(STAR);
                s.push('\n');
            }
            s.push_str(&r.to_line());
        }
        s
    }

    /// How many records are starred.
    pub fn stars(&self) -> usize {
        self.records.iter().filter(|r| r.starred).count()
    }

    /// The file's text to write now, trimmed to [`MAX_FILE_CHARS`] by
    /// dropping the oldest unstarred records, then (only if those are all
    /// gone) the oldest starred; `None` for a newer build's file, which is
    /// never written.
    pub fn written(&mut self) -> Option<String> {
        if self.from_the_future {
            return None;
        }
        let mut text = self.serialize();
        while text.len() > MAX_FILE_CHARS && self.records.len() > 1 {
            let oldest = self.records.iter().position(|r| !r.starred).unwrap_or(0);
            self.records.remove(oldest);
            text = self.serialize();
        }
        // Written now, the dropped lines are gone from disk too.
        self.damaged = 0;
        self.newer = 0;
        Some(text)
    }

    /// Add a finished run or match as the newest, dropping the oldest of
    /// its kind past its cap, then the oldest of any kind until the file
    /// fits [`MAX_FILE_CHARS`]. Returns the file's new text, or `None` when
    /// there is nothing to write: the same game is already listed (it is
    /// not added twice), or the file is a newer build's and must not be
    /// written (the record is kept in memory, for this session). A record
    /// whose line is longer than [`MAX_LINE_CHARS`] is not kept at all: the
    /// next launch would read it as damaged and drop it, so a run that long
    /// is watched from its result screen only, never from the list.
    pub fn push(&mut self, record: Record) -> Option<String> {
        if record.to_line().trim_end().len() > MAX_LINE_CHARS {
            return None;
        }
        if self.records.iter().any(|r| r.same_game(&record)) {
            return None;
        }
        self.records.push(record);
        self.cap();
        self.written()
    }

    /// Star `record` (`on`) or unstar it, found by [`Record::same_game`].
    /// One not yet in the history (a won match whose plate is still up) is
    /// added, starred, as [`History::push`] adds one. Returns the file's
    /// new text, or `None` when there is nothing to write: no change, or a
    /// newer build's file (the star is kept in memory, for this session).
    /// Taking a star off moves the record to the newest place of its kind,
    /// as if added again (see "Stars").
    pub fn set_star(&mut self, record: &Record, on: bool) -> Result<Option<String>, StarError> {
        let full = self.stars() >= MAX_STARS;
        match self.records.iter_mut().find(|r| r.same_game(record)) {
            Some(r) if r.starred == on => Ok(None),
            Some(_) | None if on && full => Err(StarError::Full),
            Some(r) => {
                r.starred = on;
                if !on {
                    // Added again: the newest of its kind, then the caps.
                    let at = self.records.iter().position(|r| r.same_game(record));
                    if let Some(at) = at {
                        let r = self.records.remove(at);
                        self.records.push(r);
                    }
                    self.cap();
                }
                Ok(self.written())
            }
            None if !on => Ok(None),
            None => {
                let starred = Record {
                    starred: true,
                    ..record.clone()
                };
                let text = self.push(starred);
                if self.records.iter().any(|r| r.same_game(record)) {
                    Ok(text)
                } else {
                    Err(StarError::NotKept)
                }
            }
        }
    }

    /// Whether `record` is in the history, starred.
    pub fn is_starred(&self, record: &Record) -> bool {
        self.records
            .iter()
            .any(|r| r.starred && r.same_game(record))
    }

    /// The records as the list shows them: the starred first, then the
    /// rest, each newest first. A child's favourites are always at the top,
    /// and with at most [`MAX_STARS`] of them the newest game is still on
    /// the first page.
    pub fn listed(&self) -> impl Iterator<Item = &Record> {
        let starred = self.records.iter().rev().filter(|r| r.starred);
        starred.chain(self.records.iter().rev().filter(|r| !r.starred))
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

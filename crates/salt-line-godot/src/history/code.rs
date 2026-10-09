//! Replay codes: a finished run or match as a line of text to share.
//! DESIGN §9.7.
//!
//! ```text
//!   SR 0 4 ............................................ 7 Q
//!   ── ─────────────────── base32 ──────────────────────────
//!   prefix   format byte · packed record · CRC-16
//! ```
//!
//! A code is `SR` and then the same base32 and CRC-16 as a map's `SL…`
//! code (DESIGN §13.7, `salt_line_sim::codec`): every character is on the
//! seed keypad, case does not matter, `O` reads as `0` and `I` or `L` as
//! `1`, and spaces, hyphens and line breaks (a chat app wrapping a long
//! code) are skipped. A code is one unbroken word, like a map's: a chat
//! wraps it where it likes, and the reader puts it back together.
//!
//! # Not the history line, re-encoded
//!
//! A history line (`history.rs`) is text made to be read: a run is about 8
//! characters a shot, and a match on a map carries the map's whole `SL…`
//! code. Base32 of that text would cost 1.6 characters for each one, and a
//! map's code would be encoded twice. So the code packs the record itself,
//! field by field, into bits, and carries a map's ground as the layout's
//! own bytes ([`Layout::to_bytes`], the bytes inside its `SL…` code), not
//! its text. Decoding builds the same [`Record`], whose line is the line
//! the sender's history holds, byte for byte (the tests hold it to that).
//!
//! # The bytes
//!
//! The format byte ([`CODE_FORMAT`]), then, most significant bit first:
//!
//! | Field | Bits |
//! |---|---|
//! | kind: 0 a run, 1 a match; 2 and 3 a later build's (UPDATE TO WATCH) | 2 |
//! | `SIM_EPOCH`, and for a match `MATCH_EPOCH` | gamma each |
//! | the day it was played (for the list's date) | wide |
//! | the seed | wide |
//! | the width | 9 |
//! | a run: daily; screens, shells (zigzagged), shots | 1; gamma, wide, gamma |
//! | a match: rolling guns, aim help L, aim help R | 1, 1, 1 |
//! | a match: opponent, 0 two players, 1–3 the AI's level easiest first (full: a fourth level is a new format) | 2 |
//! | a match: ground, 0 generated, 1 a map: its byte count, then its bytes | 1, then gamma and 8 each |
//! | a match: winner (0 left), rounds L, rounds R | 1, gamma, gamma |
//! | how many log entries | gamma |
//! | the exp-Golomb parameters of the angle and power changes | 4, 4 |
//! | the log (below) | |
//!
//! then zero bits to the end of the byte, and the CRC-16 of all of it.
//! "Gamma" is Elias gamma (`codec::put_gamma`: one bit for 0, three for 1
//! or 2, five up to 6), for counts that are almost always small; "wide" is
//! a 7-bit length and then that many bits (`codec::put_wide`), for a seed
//! or a day, which may use all 64.
//!
//! **The log.** Each shot is its angle and power ticks as a change from a
//! guess, zigzagged and exp-Golomb coded (`codec::put_exp_golomb`), the
//! parameter chosen per code to spend the fewest bits, as a map's ground
//! is. Exp-Golomb rather than the map's Rice: most changes are small (the
//! aim walked in on a target), but the first shot at each new screen is a
//! jump, which Rice would spend 42 bits on and exp-Golomb about 15. The guess is
//! the shot before, in a run (a player walks the aim in on a target), and
//! the shot two before, in a match (the same side's last shot: turns
//! alternate). The first guess is the middle of the dials. A run's log has
//! more than shots, so each entry starts with a prefix code saying what it
//! is:
//!
//! | Code | Entry |
//! |---|---|
//! | `0` | a shot: the angle's change, then the power's |
//! | `10` | a draft pick: the offer, gamma |
//! | `110` | the screen moves on: by one more than this, gamma |
//! | `1110` | the Airburst fuse changes: to this, in tenths, exp-Golomb at 8 (each screen starts it afresh, so its value says as much as a change would) |
//! | `1111` | the Enfilade offset changes: by this, zigzagged, less one, wide |
//!
//! The last three are sticky and come before the shot (or pick) they apply
//! to, at most once each, in that order, and only when the value changes,
//! as `replay::encode_log` writes `@`, `F` and `E`: one spelling per log.
//! A match's entries are `0` and a shot, or `1` for the next round dealt.
//!
//! **Not in the code:** the star (it is the player's, not the game's: a
//! pasted replay arrives unstarred), and anything a build works out from
//! the rest (the screens, the winds, the rolls, the AI's shots are all in
//! the log already, as fired).
//!
//! # Size
//!
//! A run of 60 shots is about 350 characters, half its line; a match on
//! generated screens about 110, a three-round match on a map about 340,
//! most of it the map (measured in `code/tests.rs`, which holds them under
//! their budgets; the table is in NOTES §15).
//! Text longer than [`MAX_CODE_CHARS`] is refused before anything is
//! decoded, and a record whose code would be longer is not offered to copy.
//!
//! # Reading one
//!
//! [`Record::from_code`] is total: any text is a record or a [`ReadError`],
//! never a panic. It says apart what a player can do something about: not a
//! replay code at all (or a map's), damaged on the way (the checksum, a cut
//! or a typo), made by a newer version, or too long. A record from another
//! epoch reads (the format says how to read it, whatever the sim) but
//! cannot be watched: its [`Record::version`] says which way it differs.
//! What reads is then checked as WATCH checks any record, by rebuilding it
//! off screen ([`Record::check`]), before anything is shown.
//!
//! PASTE CODE reads through [`Record::from_clipboard`], which finds the
//! first code that reads anywhere in the text (`codec::candidates`): in a
//! chat message, wrapped over lines, with zero-width spaces or a byte
//! order mark slipped in.

use salt_line_sim::codec::{
    best_exp_golomb_k, candidates, frame, get_exp_golomb, get_gamma, get_wide, put_exp_golomb,
    put_gamma, put_wide, unframe, unzigzag, zigzag, BitReader, BitWriter, FrameError,
};
use salt_line_sim::dmath::MAX_ANGLE_TICKS;
use salt_line_sim::layout::{self, Layout, LAYOUT_FORMAT};
use salt_line_sim::replay::{decode_log, encode_log, DRAFT_MARK};
use salt_line_sim::run::LoggedShot;
use salt_line_sim::{POWER_TICKS_MAX, SCREEN_W_MAX, SCREEN_W_MIN};

use super::{DuelRecord, Ground, Kind, Record, Refused, RunRecord};
use crate::match_log::{Entry, SavedMatch, MAX_ENTRIES};
use crate::opponent::Level;
use crate::versus::Side;

/// What every replay code starts with.
pub const CODE_PREFIX: &str = "SR";

/// The format the bytes are in, the first byte. A reader refuses any other:
/// a higher one is a newer build's ([`ReadError::Newer`]), which this build
/// can say but cannot read. Independent of the epochs, which are inside: a
/// change to the sim changes what a log replays to, not how it is written.
pub const CODE_FORMAT: u8 = 1;

/// The longest code made or read: about a 600-shot run, or a long match on
/// a busy map. Every honest game a family plays fits many times over (see
/// "Size"); a code is meant to be pasted into a chat, where a few thousand
/// characters still go in one message. Checked before anything is decoded.
pub const MAX_CODE_CHARS: usize = 4096;

/// The most entries a run's log may hold in a code: more than
/// [`MAX_CODE_CHARS`] can carry, so the count is checked before the log is
/// read, and nothing is allocated past it.
pub const MAX_RUN_ENTRIES: usize = 8192;

const KIND_RUN: u32 = 0;
const KIND_DUEL: u32 = 1;
// A width is 9 bits.
const _: () = assert!(SCREEN_W_MAX < 1 << 9);
/// The exp-Golomb parameter of a fuse, in tenths: 9 bits below 256, 11 to
/// 17 for the rest up to the 6,000 a 400-wide screen allows.
const FUSE_K: u32 = 8;
/// The latest day a code may carry: thousands of years from now. A day is
/// only shown as a date, but a hostile one must not reach the date's
/// arithmetic.
const MAX_DAY: u64 = 1 << 32;
/// The first guess for a shot: the middle of the dials.
const START: (i32, i32) = (MAX_ANGLE_TICKS / 2, POWER_TICKS_MAX / 2);
/// The longest map's bytes a code carries: more than any layout's code holds
/// (`layout::MAX_CODE_CHARS` characters of base32).
const MAX_GROUND_BYTES: usize = layout::MAX_CODE_CHARS * 5 / 8;

/// Why a record cannot be shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareError {
    /// Played on another build's rules: it could only be shared as what it
    /// would replay to here, which is another game.
    Version(Refused),
    /// Its code would be longer than [`MAX_CODE_CHARS`].
    TooLong,
    /// A field the format cannot hold (a hand-edited line: a shot off the
    /// dials, a map that is not the one named).
    Unencodable,
}

impl ShareError {
    /// What the list or the replay's end says. Words a child can read out.
    pub fn note(self) -> &'static str {
        match self {
            ShareError::Version(Refused::NewerVersion) => "MADE BY A NEWER VERSION - CANNOT SHARE",
            ShareError::Version(_) => "MADE BY AN OLDER VERSION - CANNOT SHARE",
            ShareError::TooLong => "TOO LONG TO SHARE",
            ShareError::Unencodable => "THIS REPLAY CANNOT BE SHARED",
        }
    }
}

/// Why text is not a replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Not a replay code: empty, or something else on the clipboard.
    NotACode,
    /// A map's code (`SL…`): the editor's PASTE CODE takes those.
    MapCode,
    /// A replay code damaged on its way: a character that is not base32,
    /// cut short, a typo the checksum catches, or bytes that are not a
    /// record.
    Damaged,
    /// A format (or a map format) newer than this build reads.
    Newer,
    /// Longer than [`MAX_CODE_CHARS`].
    TooLong,
}

impl ReadError {
    /// What REPLAYS says when PASTE CODE finds this. Words a child can read
    /// out, and what to do about it.
    pub fn note(self) -> &'static str {
        match self {
            ReadError::NotACode => "NO REPLAY CODE TO PASTE",
            ReadError::MapCode => "THAT IS A MAP CODE - PASTE IT IN VERSUS, MAKE MAP",
            ReadError::Damaged => "REPLAY CODE IS DAMAGED - COPY IT AGAIN",
            ReadError::Newer => "MADE BY A NEWER VERSION - UPDATE TO WATCH",
            ReadError::TooLong => "REPLAY CODE IS TOO LONG",
        }
    }
}

impl Record {
    /// This record as a replay code. Only a record this build can watch
    /// (its epochs this build's), and only if it fits the format and
    /// [`MAX_CODE_CHARS`].
    pub fn to_code(&self) -> Result<String, ShareError> {
        self.version().map_err(ShareError::Version)?;
        let body = match &self.kind {
            Kind::Run(r) => encode_run(self.day, r),
            Kind::Duel(d) => encode_duel(self.day, d),
        }
        .ok_or(ShareError::Unencodable)?;
        let mut bytes = vec![CODE_FORMAT];
        bytes.extend(body);
        let code = frame(CODE_PREFIX, bytes);
        if code.len() > MAX_CODE_CHARS {
            return Err(ShareError::TooLong);
        }
        Ok(code)
    }

    /// The record a replay code holds, unstarred. Total: any text is a
    /// record or a [`ReadError`]. Not yet checked: [`Record::check`] it
    /// before it is shown, as WATCH does any record.
    pub fn from_code(text: &str) -> Result<Record, ReadError> {
        // A chat may wrap a long code over lines: put it back together. The
        // raw text gets twice the room, for what wrapping adds; the code
        // itself is held to the limit.
        if text.len() > 2 * MAX_CODE_CHARS {
            return Err(ReadError::TooLong);
        }
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if text.len() > MAX_CODE_CHARS {
            return Err(ReadError::TooLong);
        }
        let bytes = unframe(CODE_PREFIX, &text).map_err(|e| match e {
            FrameError::Prefix if is_map_code(&text) => ReadError::MapCode,
            FrameError::Prefix => ReadError::NotACode,
            FrameError::Base32(_) | FrameError::Short | FrameError::Checksum => ReadError::Damaged,
        })?;
        match bytes[0] {
            CODE_FORMAT => decode(&bytes[1..]),
            f if f > CODE_FORMAT => Err(ReadError::Newer),
            _ => Err(ReadError::Damaged),
        }
    }
}

/// The most of a clipboard [`Record::from_clipboard`] looks through: a
/// pasted chat log, not a book.
const MAX_CLIPBOARD_CHARS: usize = 16 * MAX_CODE_CHARS;
/// Base32 characters after `SR` that make a run look like a code meant to
/// be one, so a word like "SRI" in a message is not called damaged.
const LOOKS_LIKE_A_CODE: usize = 10;

impl Record {
    /// The replay in text pasted from anywhere: the first `SR…` code in it
    /// that reads, wherever it sits (words round it, a chat app's line
    /// breaks, zero-width spaces, a byte order mark:
    /// `codec::candidates`). With none that reads, the most useful error:
    /// a newer version, too long, or damaged for something that looked
    /// like a replay code; a map's code; else not a code at all.
    pub fn from_clipboard(text: &str) -> Result<Record, ReadError> {
        let end = text
            .char_indices()
            .nth(MAX_CLIPBOARD_CHARS)
            .map_or(text.len(), |(i, _)| i);
        let text = &text[..end];
        let rank = |e: ReadError| match e {
            ReadError::Newer => 3,
            ReadError::TooLong => 2,
            ReadError::Damaged => 1,
            _ => 0,
        };
        let mut worst: Option<ReadError> = None;
        for code in candidates(text, CODE_PREFIX, MAX_CODE_CHARS) {
            match Record::from_code(&code) {
                Ok(record) => return Ok(record),
                Err(ReadError::Damaged) if code.len() < CODE_PREFIX.len() + LOOKS_LIKE_A_CODE => {}
                Err(e) if worst.is_none_or(|w| rank(e) > rank(w)) => worst = Some(e),
                Err(_) => {}
            }
        }
        if let Some(e) = worst {
            return Err(e);
        }
        let map = candidates(text, layout::CODE_PREFIX, layout::MAX_CODE_CHARS)
            .iter()
            .any(|c| Layout::from_code(c).is_ok());
        Err(if map {
            ReadError::MapCode
        } else {
            ReadError::NotACode
        })
    }
}

/// Whether text holds a replay code, read or not: for the map editor's
/// PASTE CODE, to say where it goes.
pub fn holds_a_replay_code(text: &str) -> bool {
    !matches!(
        Record::from_clipboard(text),
        Err(ReadError::NotACode | ReadError::MapCode)
    )
}

/// Whether text that is not a replay code is a map's.
fn is_map_code(text: &str) -> bool {
    text.get(..layout::CODE_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(layout::CODE_PREFIX))
}

// ------------------------------------------------------------- encoding

/// `v` zigzagged as a whole `i64`: a fuse or offset change may span more
/// than `codec::zigzag`'s ±2^31.
fn zigzag64(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag64(z: u64) -> i64 {
    ((z >> 1) as i64) ^ -((z & 1) as i64)
}

/// A shot on the dials, as a log may hold one: angle and power in range.
fn on_dials(a: i32, p: i32) -> bool {
    (0..=MAX_ANGLE_TICKS).contains(&a) && (0..=POWER_TICKS_MAX).contains(&p)
}

/// The shots' changes from their guesses, zigzagged, and the exp-Golomb
/// parameters that spend fewest bits on them. `back` is how many shots
/// back the guess is: 1 for a run, 2 for a match.
fn changes(shots: &[(i32, i32)], back: usize) -> (Vec<(u32, u32)>, u32, u32) {
    let mut out = Vec::with_capacity(shots.len());
    for (i, &(a, p)) in shots.iter().enumerate() {
        let guess = i.checked_sub(back).map_or(START, |j| shots[j]);
        out.push((zigzag((a - guess.0) as i64), zigzag((p - guess.1) as i64)));
    }
    let angles: Vec<u32> = out.iter().map(|c| c.0).collect();
    let powers: Vec<u32> = out.iter().map(|c| c.1).collect();
    (
        out,
        best_exp_golomb_k(&angles).0,
        best_exp_golomb_k(&powers).0,
    )
}

/// Rebuilds shots from their changes, as [`changes`] made them.
struct Guesser {
    back: usize,
    shots: Vec<(i32, i32)>,
}

impl Guesser {
    fn new(back: usize) -> Self {
        Guesser {
            back,
            shots: Vec::new(),
        }
    }

    /// The next shot, read as its two changes; `None` off the dials.
    fn read(&mut self, r: &mut BitReader, k: (u32, u32)) -> Option<(i32, i32)> {
        let i = self.shots.len();
        let guess = i.checked_sub(self.back).map_or(START, |j| self.shots[j]);
        let a = guess.0 as i64 + unzigzag(get_exp_golomb(r, k.0)?);
        let p = guess.1 as i64 + unzigzag(get_exp_golomb(r, k.1)?);
        let shot = (i32::try_from(a).ok()?, i32::try_from(p).ok()?);
        if !on_dials(shot.0, shot.1) {
            return None;
        }
        self.shots.push(shot);
        Some(shot)
    }
}

fn put_shot(b: &mut BitWriter, change: (u32, u32), k: (u32, u32)) {
    put_exp_golomb(b, change.0, k.0);
    put_exp_golomb(b, change.1, k.1);
}

fn encode_run(day: u64, r: &RunRecord) -> Option<Vec<u8>> {
    let log = decode_log(&r.log).ok()?;
    if !(SCREEN_W_MIN..=SCREEN_W_MAX).contains(&r.width)
        || log.len() > MAX_RUN_ENTRIES
        || day > MAX_DAY
    {
        return None;
    }
    let shots: Vec<(i32, i32)> = log
        .iter()
        .filter(|e| e.angle_ticks != DRAFT_MARK)
        .map(|e| (e.angle_ticks, e.power_ticks))
        .collect();
    if !shots.iter().all(|&(a, p)| on_dials(a, p)) {
        return None;
    }
    let (changes, ka, kp) = changes(&shots, 1);
    let mut b = BitWriter::default();
    b.put(KIND_RUN, 2);
    put_gamma(&mut b, r.epoch);
    put_wide(&mut b, day);
    put_wide(&mut b, r.seed);
    b.put(r.width as u32, 9);
    b.put(u32::from(r.daily), 1);
    put_gamma(&mut b, r.score.0);
    put_wide(&mut b, zigzag64(r.score.1 as i64));
    put_gamma(&mut b, r.score.2);
    put_gamma(&mut b, log.len() as u32);
    b.put(ka, 4);
    b.put(kp, 4);
    let (mut screen, mut fuse, mut offset) = (0u32, 0i32, 0i32);
    let mut shot = 0;
    for e in &log {
        if e.screen_index != screen {
            // Screens only go forward in a log that replays.
            let gap = e.screen_index.checked_sub(screen)?;
            b.put(0b110, 3);
            put_gamma(&mut b, gap - 1);
            screen = e.screen_index;
        }
        if e.angle_ticks == DRAFT_MARK {
            if e.fuse_tenths != 0 || e.emplacement_offset != 0 || e.power_ticks < 0 {
                return None;
            }
            b.put(0b10, 2);
            put_gamma(&mut b, e.power_ticks as u32);
            continue;
        }
        if e.fuse_tenths != fuse {
            // The fuse as it is: each screen starts it afresh (the target's
            // own), so a change from the last says no more than the value.
            b.put(0b1110, 4);
            put_exp_golomb(&mut b, u32::try_from(e.fuse_tenths).ok()?, FUSE_K);
            fuse = e.fuse_tenths;
        }
        if e.emplacement_offset != offset {
            b.put(0b1111, 4);
            put_wide(
                &mut b,
                zigzag64(e.emplacement_offset as i64 - offset as i64) - 1,
            );
            offset = e.emplacement_offset;
        }
        b.put(0, 1);
        put_shot(&mut b, changes[shot], (ka, kp));
        shot += 1;
    }
    Some(b.finish())
}

fn encode_duel(day: u64, d: &DuelRecord) -> Option<Vec<u8>> {
    let saved = SavedMatch::parse(&format!("{}\n", d.line)).ok()?;
    if saved.ai != d.ai || saved.log.len() > MAX_ENTRIES || day > MAX_DAY {
        return None;
    }
    // The ground the record names must be the match's, and its code the
    // one this build writes for it: the line read back is then the same.
    let ground = match (&saved.ground, &d.ground) {
        (None, Ground::Generated) => None,
        (Some(code), Ground::Map(name)) => {
            let layout = Layout::from_code(code).ok()?;
            if layout.to_code().ok()? != *code || layout.name.as_str() != name {
                return None;
            }
            Some(layout.to_bytes().ok()?)
        }
        _ => return None,
    };
    let shots: Vec<(i32, i32)> = saved
        .log
        .iter()
        .filter_map(|e| match *e {
            Entry::Shot(a, p) => Some((a, p)),
            Entry::NextRound => None,
        })
        .collect();
    let (changes, ka, kp) = changes(&shots, 2);
    let mut b = BitWriter::default();
    b.put(KIND_DUEL, 2);
    put_gamma(&mut b, saved.epoch);
    put_gamma(&mut b, saved.match_epoch);
    put_wide(&mut b, day);
    put_wide(&mut b, saved.seed);
    b.put(saved.width as u32, 9);
    b.put(u32::from(saved.rolling), 1);
    b.put(u32::from(saved.help[0]), 1);
    b.put(u32::from(saved.help[1]), 1);
    let level = saved.ai.map_or(0, |l| {
        Level::ALL.iter().position(|&x| x == l).map_or(0, |i| i + 1)
    });
    b.put(level as u32, 2);
    match &ground {
        None => b.put(0, 1),
        Some(bytes) => {
            b.put(1, 1);
            put_gamma(&mut b, bytes.len() as u32);
            for &byte in bytes {
                b.put(byte as u32, 8);
            }
        }
    }
    b.put(u32::from(d.winner == Side::Right), 1);
    put_gamma(&mut b, d.rounds[0]);
    put_gamma(&mut b, d.rounds[1]);
    put_gamma(&mut b, saved.log.len() as u32);
    b.put(ka, 4);
    b.put(kp, 4);
    let mut shot = 0;
    for e in &saved.log {
        match e {
            Entry::Shot(..) => {
                b.put(0, 1);
                put_shot(&mut b, changes[shot], (ka, kp));
                shot += 1;
            }
            Entry::NextRound => b.put(1, 1),
        }
    }
    Some(b.finish())
}

// ------------------------------------------------------------- decoding

fn decode(body: &[u8]) -> Result<Record, ReadError> {
    let mut r = BitReader::new(body);
    let record = match r.get(2) {
        Some(KIND_RUN) => decode_run(&mut r),
        Some(KIND_DUEL) => decode_duel(&mut r)?,
        // The two kinds not yet used are a later build's (a challenge, a
        // course): say so, rather than damaged.
        Some(_) => return Err(ReadError::Newer),
        None => None,
    }
    .ok_or(ReadError::Damaged)?;
    // Only zero padding may follow, and less than a byte of it.
    if !r.rest_is_padding() {
        return Err(ReadError::Damaged);
    }
    Ok(record)
}

fn width(r: &mut BitReader) -> Option<usize> {
    let w = r.get(9)? as usize;
    (SCREEN_W_MIN..=SCREEN_W_MAX).contains(&w).then_some(w)
}

/// A count, checked against `max` before anything is allocated for it.
fn count(r: &mut BitReader, max: usize) -> Option<usize> {
    let n = get_gamma(r)? as usize;
    (n <= max).then_some(n)
}

/// A sticky change: an `i32` moved by a non-zero amount.
fn changed(r: &mut BitReader, from: i32) -> Option<i32> {
    let z = get_wide(r)?.checked_add(1)?;
    i32::try_from((from as i64).checked_add(unzigzag64(z))?).ok()
}

fn decode_run(r: &mut BitReader) -> Option<Record> {
    let epoch = get_gamma(r)?;
    let day = get_wide(r)?;
    if day > MAX_DAY {
        return None;
    }
    let seed = get_wide(r)?;
    let width = width(r)?;
    let daily = r.get(1)? == 1;
    let screens = get_gamma(r)?;
    let shells = i32::try_from(unzigzag64(get_wide(r)?)).ok()?;
    let shots = get_gamma(r)?;
    let n = count(r, MAX_RUN_ENTRIES)?;
    let k = (r.get(4)?, r.get(4)?);
    let mut guesser = Guesser::new(1);
    let mut log = Vec::with_capacity(n);
    let (mut screen, mut fuse, mut offset) = (0u32, 0i32, 0i32);
    // The sticky changes seen since the last entry: 1 the screen, 2 the
    // fuse, 3 the offset; each once, in that order.
    let mut stage = 0;
    while log.len() < n {
        if r.get(1)? == 0 {
            let (angle_ticks, power_ticks) = guesser.read(r, k)?;
            log.push(LoggedShot {
                screen_index: screen,
                angle_ticks,
                power_ticks,
                fuse_tenths: fuse,
                emplacement_offset: offset,
            });
            stage = 0;
        } else if r.get(1)? == 0 {
            // A pick: the fuse and offset are a shot's, never changed
            // before one.
            if stage > 1 {
                return None;
            }
            let offer = i32::try_from(get_gamma(r)?).ok()?;
            log.push(LoggedShot {
                screen_index: screen,
                angle_ticks: DRAFT_MARK,
                power_ticks: offer,
                fuse_tenths: 0,
                emplacement_offset: 0,
            });
            stage = 0;
        } else if r.get(1)? == 0 {
            if stage >= 1 {
                return None;
            }
            screen = screen.checked_add(get_gamma(r)?)?.checked_add(1)?;
            stage = 1;
        } else if r.get(1)? == 0 {
            if stage >= 2 {
                return None;
            }
            let to = i32::try_from(get_exp_golomb(r, FUSE_K)?).ok()?;
            if to == fuse {
                return None;
            }
            fuse = to;
            stage = 2;
        } else {
            if stage >= 3 {
                return None;
            }
            offset = changed(r, offset)?;
            stage = 3;
        }
    }
    if stage != 0 {
        return None;
    }
    Some(Record::new(
        day,
        Kind::Run(RunRecord {
            epoch,
            seed,
            width,
            daily,
            score: (screens, shells, shots),
            log: encode_log(&log),
        }),
    ))
}

/// `Err` only for a map in a newer format; `Ok(None)` for any other damage.
fn decode_duel(r: &mut BitReader) -> Result<Option<Record>, ReadError> {
    let Some(head) = duel_head(r) else {
        return Ok(None);
    };
    let (epoch, match_epoch, day, seed, width, rolling, help, ai) = head;
    let ground = match r.get(1) {
        Some(0) => None,
        Some(_) => {
            let Some(bytes) = ground_bytes(r) else {
                return Ok(None);
            };
            match Layout::from_bytes(&bytes) {
                Ok(layout) => Some(layout),
                Err(layout::CodeError::Format(f)) if f > LAYOUT_FORMAT => {
                    return Err(ReadError::Newer)
                }
                Err(_) => return Ok(None),
            }
        }
        None => return Ok(None),
    };
    Ok(duel_rest(r).and_then(|(winner, rounds, log)| {
        let (code, ground) = match ground {
            None => (None, Ground::Generated),
            Some(l) => (
                Some(l.to_code().ok()?),
                Ground::Map(l.name.as_str().to_string()),
            ),
        };
        let saved = SavedMatch {
            epoch,
            match_epoch,
            seed,
            width,
            rolling,
            help,
            ai,
            log,
            ground: code,
        };
        Some(Record::new(
            day,
            Kind::Duel(DuelRecord {
                ai,
                ground,
                winner,
                rounds,
                line: saved.to_line().trim_end().to_string(),
            }),
        ))
    }))
}

/// A match's epochs, day, seed, width, rolling, aim help and opponent.
#[allow(clippy::type_complexity)]
fn duel_head(
    r: &mut BitReader,
) -> Option<(u32, u32, u64, u64, usize, bool, [bool; 2], Option<Level>)> {
    let epoch = get_gamma(r)?;
    let match_epoch = get_gamma(r)?;
    let day = get_wide(r)?;
    if day > MAX_DAY {
        return None;
    }
    let seed = get_wide(r)?;
    let width = width(r)?;
    let rolling = r.get(1)? == 1;
    let help = [r.get(1)? == 1, r.get(1)? == 1];
    let ai = match r.get(2)? {
        0 => None,
        i => Some(*Level::ALL.get(i as usize - 1)?),
    };
    Some((epoch, match_epoch, day, seed, width, rolling, help, ai))
}

/// A map's bytes: their count, checked first, then each.
fn ground_bytes(r: &mut BitReader) -> Option<Vec<u8>> {
    let n = count(r, MAX_GROUND_BYTES)?;
    if n * 8 > r.left() {
        return None;
    }
    (0..n).map(|_| r.get(8).map(|b| b as u8)).collect()
}

/// A match's result and log.
fn duel_rest(r: &mut BitReader) -> Option<(Side, [u32; 2], Vec<Entry>)> {
    let winner = if r.get(1)? == 0 {
        Side::Left
    } else {
        Side::Right
    };
    let rounds = [get_gamma(r)?, get_gamma(r)?];
    let n = count(r, MAX_ENTRIES)?;
    let k = (r.get(4)?, r.get(4)?);
    let mut guesser = Guesser::new(2);
    let mut log = Vec::with_capacity(n);
    for _ in 0..n {
        log.push(if r.get(1)? == 0 {
            let (a, p) = guesser.read(r, k)?;
            Entry::Shot(a, p)
        } else {
            Entry::NextRound
        });
    }
    Some((winner, rounds, log))
}

#[cfg(test)]
mod tests;

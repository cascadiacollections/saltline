//! Seed plus shot log to verified reconstruction. DESIGN §3, brief M4.
//!
//! Nothing else is persisted. A run is `(run_seed, width, log)`, where each
//! log entry is `(screen_index, angle_ticks, power_ticks, fuse_tenths,
//! emplacement_offset)`: the aim, plus the Airburst fuse and Enfilade offset
//! the shot was fired with.
//!
//! Draft picks ride in the same entry: an entry with `angle_ticks ==
//! DRAFT_MARK` records "took offer `power_ticks`" rather than a shot. A draft
//! is a player decision, so it has to be in the log for a reconstruction to be
//! exact — and reusing the entry keeps the persisted form to a few small
//! numbers per decision, with no second stream to keep in sync.
//!
//! `encode_log` / `decode_log` give the log a readable text form (DESIGN §9.2,
//! extended with fuse and offset tokens), used by the golden-run corpus.
//!
//! # Event sourcing
//!
//! A replay stores inputs, never state. Everything else, from terrain and
//! craters to the loadout and the shell count, is recomputed by feeding the
//! log back through the same [`RunState`] methods the live game calls. That
//! only works because the sim is deterministic to the bit (see the crate
//! docs), and it is what keeps a whole run to a few kilobytes of text
//! (DESIGN §9). The trade-off is that a log is only meaningful to a sim that
//! resolves shots identically, which is what [`crate::SIM_EPOCH`] tracks
//! (DESIGN §9.3).
//!
//! # Verification
//!
//! [`state_hash`] folds the run state into one `u64`, floats by bit pattern.
//! Comparing a live run's hash with its reconstruction's is the determinism
//! check: the property tests do it for random runs, `src/bin/determinism.rs`
//! does it for 200 runs on every CI platform (DESIGN §6), and
//! `tests/golden_runs.rs` does it against hashes recorded in a checked-in
//! corpus.
//!
//! Status: the game does not yet keep or replay run history (DESIGN §9 is
//! proposed). Today this module serves those tests and the determinism gate.

use crate::ballistics::Shot;
use crate::run::{LoggedShot, Phase, RunState};

/// `angle_ticks` sentinel marking a draft pick rather than a shot. Angles are
/// never negative ([`RunState::fire`] clamps logged angles to
/// `0..=MAX_ANGLE_TICKS`), so the encoding is unambiguous.
pub const DRAFT_MARK: i32 = -1;

/// A complete, replayable run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    /// The run's seed.
    pub run_seed: u64,
    /// Buffer width the run was played at. A replay must use it whatever the
    /// current device, because width changes generation (DESIGN §9.2).
    pub width: usize,
    /// Every shot and draft pick, in order.
    pub log: Vec<LoggedShot>,
}

impl Replay {
    /// Snapshot a run's identity and log. Cheap apart from cloning the log;
    /// valid at any point, since a prefix of a valid log is a valid log.
    /// The rebuilt state matches the live one only at replay boundaries (see
    /// [`state_hash`]): not while `Cleared`, nor after an unfired reposition.
    pub fn of(state: &RunState) -> Self {
        Self {
            run_seed: state.run_seed,
            width: state.width,
            log: state.log.clone(),
        }
    }

    /// Rebuild the run. Entries that no longer apply (a shot logged while the
    /// run is over, a draft pick that is not on offer) end the reconstruction
    /// rather than being silently skipped — a log that does not replay cleanly
    /// is a determinism bug, and it should be visible as one.
    ///
    /// Each shot entry replays its inputs in a fixed order: move the
    /// emplacement, set the fuse, fire, then resolve a clear at once. Shot
    /// values are passed through [`RunState::fire`], which quantises them, so
    /// an off-grid angle in a hand-written log replays as the quantised shot
    /// and is logged that way in the rebuilt state. Angle and power are
    /// clamped like live input; fuse and offset must reproduce the logged
    /// value exactly, or the entry is refused.
    ///
    /// # Errors
    ///
    /// A [`ReplayError`] naming the index of the first entry that cannot be
    /// applied. The run up to that entry is discarded.
    pub fn reconstruct(&self) -> Result<RunState, ReplayError> {
        let mut state = RunState::new(self.run_seed, self.width);
        for (i, entry) in self.log.iter().enumerate() {
            if entry.angle_ticks == DRAFT_MARK {
                if entry.fuse_tenths != 0 || entry.emplacement_offset != 0 {
                    return Err(ReplayError::MalformedDraft { at: i });
                }
                if state.phase != Phase::Drafting {
                    return Err(ReplayError::UnexpectedDraft { at: i });
                }
                state
                    .take_offer(entry.power_ticks as usize)
                    .ok_or(ReplayError::NoSuchOffer { at: i })?;
                continue;
            }
            if state.phase != Phase::Firing {
                return Err(ReplayError::UnexpectedShot { at: i });
            }
            if state.screen_index != entry.screen_index {
                return Err(ReplayError::ScreenMismatch {
                    at: i,
                    expected: state.screen_index,
                    found: entry.screen_index,
                });
            }
            // The emplacement moves only where the rules allow it: with
            // Enfilade, before the screen's first shot. A logged offset the
            // run cannot reach is a broken log, not something to skip.
            if entry.emplacement_offset != state.offset_units()
                && !(state.reposition(entry.emplacement_offset as f64)
                    && state.offset_units() == entry.emplacement_offset)
            {
                return Err(ReplayError::OffsetRefused { at: i });
            }
            state.set_fuse_tenths(entry.fuse_tenths);
            if state.fuse_tenths() != entry.fuse_tenths {
                return Err(ReplayError::FuseOutOfRange { at: i });
            }
            state
                .fire(Shot::new(entry.angle_ticks, entry.power_ticks), false)
                .ok_or(ReplayError::ShotRefused { at: i })?;
            // Headless callers do not wait for a collapse animation.
            state.resolve_clear();
        }
        Ok(state)
    }

    /// Reconstruct and hash. This is the number CI compares across platforms.
    ///
    /// # Errors
    ///
    /// As [`Replay::reconstruct`].
    pub fn final_hash(&self) -> Result<u64, ReplayError> {
        self.reconstruct().map(|s| state_hash(&s))
    }
}

/// Why a log failed to replay. `at` is the index into [`Replay::log`] of the
/// first entry that could not be applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayError {
    /// A shot while the run was not in `Phase::Firing`: after it ended, or
    /// while a draft was open.
    UnexpectedShot { at: usize },
    /// A draft pick while no draft was open.
    UnexpectedDraft { at: usize },
    /// A draft pick whose index has no offer.
    NoSuchOffer { at: usize },
    /// [`RunState::fire`] refused the shot. Not expected after the phase
    /// check, since the run leaves `Firing` as soon as the shells run out;
    /// kept so a refusal is an error rather than a panic.
    ShotRefused { at: usize },
    /// A shot logged against a different screen than the one the run is on:
    /// the log and the reconstruction have diverged.
    ScreenMismatch {
        at: usize,
        expected: u32,
        found: u32,
    },
    /// The logged emplacement offset cannot be applied: no Enfilade, a shot
    /// already taken on the screen, or outside the allowed range.
    OffsetRefused { at: usize },
    /// The logged fuse is outside the range `RunState::set_fuse` allows.
    FuseOutOfRange { at: usize },
    /// A draft entry with a fuse or offset: `take_offer` logs both as 0.
    MalformedDraft { at: usize },
}

// ------------------------------------------------------------- text form

/// Version of the text form. `encode_log` writes it as a leading `v2` token
/// and `decode_log` accepts no other: a log from before the fuse and offset
/// were recorded has no way to say what they were, and guessing would replay
/// it as a different run.
pub const LOG_FORMAT: u32 = 2;

/// Encode a log as space-separated tokens. DESIGN §9.2, plus a version and
/// two tokens for the inputs that section predates:
///
/// - `v2` comes first: the format version, `LOG_FORMAT`.
/// - `@N` sets the screen index for the entries after it.
/// - `A:P` is a shot at angle ticks `A`, power ticks `P`.
/// - `DN` is a pick of draft offer `N`.
/// - `FT` sets the fuse, in tenths of a unit, for the shots after it.
/// - `EO` sets the emplacement offset, in whole units, for the shots after it.
///
/// `@`, `F` and `E` are sticky and written only when they change, in that
/// order, immediately before the entry that needs them. The fuse has no
/// starting value, so the first shot always carries an `F`; the offset
/// starts at 0. A draft entry's fuse and offset are not written (a valid log
/// has them at 0, and `Replay::reconstruct` rejects any other).
pub fn encode_log(log: &[LoggedShot]) -> String {
    let mut out = vec![format!("v{LOG_FORMAT}")];
    let mut screen = None;
    let mut fuse = None;
    let mut offset = 0;
    for e in log {
        if screen != Some(e.screen_index) {
            screen = Some(e.screen_index);
            out.push(format!("@{}", e.screen_index));
        }
        if e.angle_ticks == DRAFT_MARK {
            out.push(format!("D{}", e.power_ticks));
            continue;
        }
        if fuse != Some(e.fuse_tenths) {
            fuse = Some(e.fuse_tenths);
            out.push(format!("F{}", e.fuse_tenths));
        }
        if offset != e.emplacement_offset {
            offset = e.emplacement_offset;
            out.push(format!("E{offset}"));
        }
        out.push(format!("{}:{}", e.angle_ticks, e.power_ticks));
    }
    out.join(" ")
}

/// Why a text log was rejected. `at` is the token index, counting the
/// version token as 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogError {
    /// The text does not start with a `vN` token.
    MissingVersion,
    /// A version other than `LOG_FORMAT` — typically a log from an earlier
    /// build, which cannot be replayed.
    UnsupportedVersion(u32),
    /// Not a token of the format, or a number out of range or not written
    /// the way `encode_log` writes it (`+5`, `05`, `-0`).
    BadToken { at: usize },
    /// A shot or pick before any `@N`.
    NoScreen { at: usize },
    /// A shot before any `F`: its fuse is unknown.
    NoFuse { at: usize },
    /// `@`, `F` or `E` repeated, restating the current value, out of order,
    /// or not followed by an entry that uses it.
    StrayToken { at: usize },
}

/// Parse `encode_log`'s format. Only canonical text is accepted, so for any
/// text this accepts, `encode_log(&decode_log(text)?)` is the same tokens.
///
/// Strict on purpose. A lenient parser would give one log several spellings
/// (`@01`, a repeated `F5`, a dangling `E3`), so two texts that differ could
/// name the same run, and a round trip would not be the identity. With one
/// canonical form a run line can be compared, deduplicated or hashed as
/// text, and a hand-edited or corrupted line is rejected instead of being
/// read as something nearby. Whitespace is the one allowance:
/// leading, trailing and any run between tokens.
///
/// Decoding checks the syntax only. Whether the entries make a legal run
/// (the right screens, picks with something on offer, fuses in range) is
/// for [`Replay::reconstruct`] to decide.
///
/// # Errors
///
/// A [`LogError`] for the first token that breaks the format.
pub fn decode_log(text: &str) -> Result<Vec<LoggedShot>, LogError> {
    let mut toks = text.split_whitespace().enumerate();
    let version = toks
        .next()
        .and_then(|(_, t)| t.strip_prefix('v'))
        .ok_or(LogError::MissingVersion)?;
    let version: u32 = canonical(version).ok_or(LogError::MissingVersion)?;
    if version != LOG_FORMAT {
        return Err(LogError::UnsupportedVersion(version));
    }

    let mut log = Vec::new();
    let mut screen: Option<u32> = None;
    let mut fuse: Option<i32> = None;
    let mut offset = 0;
    // Sticky tokens seen since the last entry, as a bitmask in their
    // required order (@ = 1, F = 2, E = 4), with the index of the first.
    let mut pending = 0u8;
    let mut pending_at = 0;
    for (at, tok) in toks {
        let bad = LogError::BadToken { at };
        let mut sticky = |bit: u8, changed: bool| {
            // Each at most once per entry, in order, and only on a change.
            if pending >= bit || !changed {
                return Err(LogError::StrayToken { at });
            }
            if pending == 0 {
                pending_at = at;
            }
            pending |= bit;
            Ok(())
        };
        if let Some(n) = tok.strip_prefix('@') {
            let n = canonical(n).ok_or(bad)?;
            sticky(1, screen != Some(n))?;
            screen = Some(n);
        } else if let Some(t) = tok.strip_prefix('F') {
            let t = canonical(t).ok_or(bad)?;
            sticky(2, fuse != Some(t))?;
            fuse = Some(t);
        } else if let Some(o) = tok.strip_prefix('E') {
            let o = canonical(o).ok_or(bad)?;
            sticky(4, offset != o)?;
            offset = o;
        } else if let Some(n) = tok.strip_prefix('D') {
            if pending & !1 != 0 {
                // A fuse or offset change is only ever written before a shot.
                return Err(LogError::StrayToken { at: pending_at });
            }
            log.push(LoggedShot {
                screen_index: screen.ok_or(LogError::NoScreen { at })?,
                angle_ticks: DRAFT_MARK,
                power_ticks: canonical(n).ok_or(bad)?,
                fuse_tenths: 0,
                emplacement_offset: 0,
            });
            pending = 0;
        } else {
            let (a, p) = tok.split_once(':').ok_or(bad)?;
            let angle_ticks = canonical(a).ok_or(bad)?;
            if angle_ticks == DRAFT_MARK {
                return Err(bad);
            }
            log.push(LoggedShot {
                screen_index: screen.ok_or(LogError::NoScreen { at })?,
                angle_ticks,
                power_ticks: canonical(p).ok_or(bad)?,
                fuse_tenths: fuse.ok_or(LogError::NoFuse { at })?,
                emplacement_offset: offset,
            });
            pending = 0;
        }
    }
    if pending != 0 {
        return Err(LogError::StrayToken { at: pending_at });
    }
    Ok(log)
}

/// Parse a number only if it is written exactly as `Display` writes it.
/// Parse-then-print-and-compare rejects every alternative spelling (`+5`,
/// `05`, `-0`) without listing them.
fn canonical<T: core::str::FromStr + ToString>(s: &str) -> Option<T> {
    let v: T = s.parse().ok()?;
    (v.to_string() == s).then_some(v)
}

// ----------------------------------------------------------------- hashing

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a. Chosen because it is trivially identical on every platform and has
/// no library dependency — the sim crate takes none.
#[derive(Clone, Copy, Debug)]
pub struct Hasher(u64);

impl Default for Hasher {
    fn default() -> Self {
        Hasher(FNV_OFFSET)
    }
}

impl Hasher {
    /// A hasher at the FNV-1a offset basis.
    pub fn new() -> Self {
        Self::default()
    }
    /// The hash of everything written so far.
    pub fn finish(self) -> u64 {
        self.0
    }
    /// Mix in one byte: the FNV-1a step, xor then multiply.
    pub fn write_u8(&mut self, v: u8) {
        self.0 ^= v as u64;
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }
    /// Mix in eight bytes, little-endian whatever the host, so the hash is
    /// the same on every platform.
    pub fn write_u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }
    /// Sign-extended to 64 bits, then as [`Hasher::write_u64`].
    pub fn write_i32(&mut self, v: i32) {
        self.write_u64(v as i64 as u64);
    }
    /// Widened to 64 bits, then as [`Hasher::write_u64`].
    pub fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
    /// Widened to 64 bits, so 32- and 64-bit targets agree.
    pub fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    /// Hashes the bit pattern, so a one-ULP difference anywhere shows up.
    pub fn write_f64(&mut self, v: f64) {
        self.write_u64(v.to_bits());
    }
}

/// Hash of the full run state, including terrain damage and structure states.
///
/// Every float goes in as its bit pattern: the point of this hash is to catch
/// a determinism leak, so it must not tolerate a one-ULP drift.
///
/// It is meant to be compared at replay boundaries: a live run right after a
/// logged entry (a shot resolved, a pick taken) against its reconstruction
/// from the log so far. That is where CI, the golden runs and the property
/// tests compare it. Covered: the economy, phase, loadout, offers, the whole
/// current screen (terrain, damage, structures, and `muzzle_x`, so the
/// Enfilade offset in effect) and the full log, which carries every shot's
/// fuse and offset. Not covered: the live `fuse`, chalk marks, the last apex
/// and whether the wind is revealed. The fuse is left out deliberately: it
/// is a pending input, not an outcome — the player can turn it after the
/// last shot and the log cannot know — and what a shot flew with is already
/// in its log entry. The others are derived from the log and the screen, or
/// only affect what the HUD shows. For the same reason a live run whose
/// player has repositioned but not yet fired (a pending `muzzle_x`) does not
/// match its reconstruction until that shot is logged.
///
/// Changing what is hashed, or its order, moves every golden-run hash and
/// the epoch digest, so it needs the `SIM_EPOCH` procedure (DESIGN §9.3).
pub fn state_hash(s: &RunState) -> u64 {
    let mut h = Hasher::new();
    h.write_u64(s.run_seed);
    h.write_usize(s.width);
    h.write_i32(s.shells);
    h.write_u32(s.screen_index);
    h.write_u32(s.screens_cleared);
    h.write_u32(s.total_shots);
    h.write_u32(s.shots_on_screen);
    h.write_u32(s.one_shot_streak);
    h.write_u8(match s.phase {
        Phase::Firing => 0,
        Phase::Cleared => 1,
        Phase::Drafting => 2,
        Phase::Over(_) => 3,
    });
    for m in crate::modifiers::Modifier::all() {
        h.write_u8(s.loadout.stacks(m));
    }
    for o in &s.offers {
        h.write_usize(o.index());
    }
    hash_screen(&mut h, &s.screen);
    for entry in &s.log {
        h.write_u32(entry.screen_index);
        h.write_i32(entry.angle_ticks);
        h.write_i32(entry.power_ticks);
        h.write_i32(entry.fuse_tenths);
        h.write_i32(entry.emplacement_offset);
    }
    h.finish()
}

/// Hash of one screen: terrain, damage, and every structure's state.
///
/// What goes in, and in what order, is part of every pinned digest and of
/// the epoch digest: adding, removing or reordering a field changes them
/// all, even though no outcome changed, and needs the same `SIM_EPOCH`
/// procedure as a sim change.
///
/// `wind_late` goes in as `unwrap_or(0.0)`, so `None` and `Some(0.0)` hash
/// the same. `candidates`, `attempts` and `fell_back` are not hashed, so the
/// pinned snapshots do not catch a change to them.
pub fn hash_screen(h: &mut Hasher, screen: &crate::run::Screen) {
    h.write_u32(screen.index);
    h.write_u64(screen.seed);
    h.write_usize(screen.width);
    h.write_u8(screen.template as u8);
    h.write_f64(screen.wind);
    h.write_f64(screen.wind_late.unwrap_or(0.0));
    h.write_f64(screen.muzzle_x);
    h.write_u8(screen.supply as u8);
    for i in 0..screen.field.len() {
        h.write_f64(screen.field.base[i]);
        h.write_f64(screen.field.scar[i]);
        h.write_u8(screen.field.material[i] as u8);
    }
    for s in &screen.structures {
        h.write_u8(s.kind as u8);
        h.write_usize(s.x0);
        h.write_f64(s.base);
        h.write_f64(s.original_mean);
        h.write_f64(s.integrity);
        h.write_f64(s.support);
        h.write_u8(s.state as u8);
        h.write_u8(s.lean as u8);
    }
}

/// Hash of a screen on its own. The pinned screen digests
/// (`tests/snapshots.rs`), the epoch digest (`tests/golden_runs.rs`) and the
/// "screen 400 without screens 1..399" acceptance test compare it.
pub fn screen_hash(screen: &crate::run::Screen) -> u64 {
    let mut h = Hasher::new();
    hash_screen(&mut h, screen);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Pcg32;
    use crate::run::{shot, RunState};

    /// Play a run with deterministic pseudo-inputs. Not a good player — the
    /// point is to touch drafts, misses, hits and the reserve running dry.
    pub(crate) fn play(run_seed: u64, width: usize, max_decisions: usize) -> RunState {
        let mut state = RunState::new(run_seed, width);
        let mut r = Pcg32::new(run_seed ^ 0xB0A7);
        for _ in 0..max_decisions {
            match state.phase {
                Phase::Firing => {
                    // Aim vaguely at the nearest target, then wobble.
                    let angle = r.range_u32(300, 1600) as i32;
                    let power = r.range_u32(250, 1000) as i32;
                    if state.fire(shot(angle, power), false).is_none() {
                        break;
                    }
                }
                Phase::Cleared => {
                    state.resolve_clear();
                }
                Phase::Drafting => {
                    let n = state.offers.len();
                    if n == 0 {
                        state.skip_draft();
                    } else {
                        let which = r.range_u32(0, n as u32 - 1) as usize;
                        state.take_offer(which);
                    }
                }
                Phase::Over(_) => break,
            }
        }
        state
    }

    #[test]
    fn a_recorded_run_replays_to_the_same_state() {
        let played = play(0xFEED_FACE, 320, 300);
        let replay = Replay::of(&played);
        let rebuilt = replay.reconstruct().expect("replay failed");
        assert_eq!(state_hash(&played), state_hash(&rebuilt));
        assert_eq!(played.screens_cleared, rebuilt.screens_cleared);
        assert_eq!(played.shells, rebuilt.shells);
        assert_eq!(played.log, rebuilt.log);
    }

    #[test]
    fn drafts_are_carried_in_the_log() {
        let mut played = RunState::new(31337, 320);
        while played.screen_index < crate::BAND_LEN {
            let target = played
                .screen
                .structures
                .iter()
                .position(|s| !s.is_down())
                .expect("screen has no standing target");
            let candidates = crate::solver::solve_target(&played.screen, &played.loadout, target);
            played.fuse = played.screen.default_fuse(target).unwrap_or(0.0);
            played.fire(candidates[0].shot, false);
            played.resolve_clear();
            if played.phase == Phase::Drafting {
                played.take_offer(0);
            }
        }
        let drafts = played
            .log
            .iter()
            .filter(|e| e.angle_ticks == DRAFT_MARK)
            .count();
        assert!(drafts > 0, "the run never reached a draft");
        played.fire(shot(900, 500), false);
        // Dropping the draft entries changes the reconstruction, which is why
        // they have to be in the log at all.
        let mut stripped = Replay::of(&played);
        stripped.log.retain(|e| e.angle_ticks != DRAFT_MARK);
        assert!(stripped.reconstruct().is_err());
    }

    #[test]
    fn the_hash_notices_a_single_changed_tick() {
        let played = play(4242, 320, 200);
        let base = Replay::of(&played);
        let h0 = base.final_hash().unwrap();
        let mut tweaked = base.clone();
        let idx = tweaked
            .log
            .iter()
            .position(|e| e.angle_ticks != DRAFT_MARK)
            .unwrap();
        tweaked.log[idx].angle_ticks += 10;
        let h1 = tweaked.final_hash().unwrap();
        assert_ne!(h0, h1, "hash is blind to a changed shot");
    }

    /// A run on a fixed seed that drafts Enfilade and Airburst and uses both.
    fn with_enfilade_and_airburst() -> RunState {
        let mut state = RunState::new(31337, 320);
        let mut moved = false;
        let mut fused = false;
        while !(moved && fused) && state.screen_index < 40 {
            match state.phase {
                Phase::Firing => {
                    if state.reposition(if state.screen_index.is_multiple_of(2) {
                        9.0
                    } else {
                        -6.0
                    }) {
                        moved = true;
                    }
                    let target = state
                        .screen
                        .structures
                        .iter()
                        .position(|s| !s.is_down())
                        .unwrap();
                    let c = crate::solver::solve_target(&state.screen, &state.loadout, target);
                    if state.loadout.airburst() {
                        // A deliberately short fuse: the shot bursts early.
                        state.set_fuse(state.screen.default_fuse(target).unwrap() * 0.5);
                        fused = true;
                    }
                    state.fire(c[0].shot, false);
                    state.resolve_clear();
                }
                Phase::Drafting => {
                    let pick = state
                        .offers
                        .iter()
                        .position(|&m| {
                            m == crate::modifiers::Modifier::Airburst
                                || m == crate::modifiers::Modifier::Enfilade
                        })
                        .unwrap_or(0);
                    state.take_offer(pick);
                }
                _ => break,
            }
        }
        assert!(moved && fused, "the fixed run never drafted both");
        state
    }

    #[test]
    fn the_fuse_and_the_emplacement_are_replayed() {
        let played = with_enfilade_and_airburst();
        let replay = Replay::of(&played);
        assert_eq!(replay.final_hash(), Ok(state_hash(&played)));

        // Either input dropped from the log changes the outcome.
        let shots = || (0..replay.log.len()).filter(|&i| replay.log[i].angle_ticks >= 0);
        let moved = shots()
            .find(|&i| replay.log[i].emplacement_offset != 0)
            .unwrap();
        let mut unmoved = replay.clone();
        unmoved.log[moved].emplacement_offset = 0;
        assert_ne!(unmoved.final_hash(), Ok(state_hash(&played)));
        let fused = shots().next_back().unwrap();
        let mut refused = replay.clone();
        refused.log[fused].fuse_tenths += 1;
        assert_ne!(refused.final_hash(), Ok(state_hash(&played)));
    }

    #[test]
    fn an_offset_the_rules_forbid_is_an_error() {
        // No Enfilade on the first screen.
        let mut state = RunState::new(5, 320);
        state.fire(shot(900, 500), false);
        let mut replay = Replay::of(&state);
        replay.log[0].emplacement_offset = 5;
        assert_eq!(
            replay.reconstruct().err(),
            Some(ReplayError::OffsetRefused { at: 0 })
        );

        // With Enfilade, not after the screen's first shot.
        let mut played = with_enfilade_and_airburst();
        assert!(played.loadout.enfilade_units > 0.0);
        // Two near-vertical misses on the next screen.
        played.fire(shot(1780, 60), false);
        played.fire(shot(1780, 60), false);
        let i = played.log.len() - 1;
        assert_eq!(played.log[i - 1].screen_index, played.log[i].screen_index);
        let mut replay = Replay::of(&played);
        replay.log[i].emplacement_offset += 1;
        assert_eq!(
            replay.reconstruct().err(),
            Some(ReplayError::OffsetRefused { at: i })
        );
        // Out of the modifier's range, even before the first shot.
        let mut replay = Replay::of(&played);
        replay.log[i - 1].emplacement_offset = 26;
        assert_eq!(
            replay.reconstruct().err(),
            Some(ReplayError::OffsetRefused { at: i - 1 })
        );

        let mut replay = Replay::of(&played);
        let last = replay.log.len() - 1;
        replay.log[last].fuse_tenths = -1;
        assert_eq!(
            replay.reconstruct().err(),
            Some(ReplayError::FuseOutOfRange { at: last })
        );
    }

    #[test]
    fn the_text_form_round_trips() {
        for played in [with_enfilade_and_airburst(), play(77, 400, 200)] {
            let text = encode_log(&played.log);
            assert!(text.starts_with("v2 @0 F"), "{text}");
            assert_eq!(decode_log(&text).as_ref(), Ok(&played.log), "{text}");
        }
        assert_eq!(encode_log(&[]), "v2");
        assert_eq!(decode_log("v2"), Ok(vec![]));
        let log = decode_log("v2 @0 F1200 900:600 F800 E-5 860:640 @3 D1 @4 700:520").unwrap();
        assert_eq!(log.len(), 4);
        assert_eq!((log[1].fuse_tenths, log[1].emplacement_offset), (800, -5));
        assert_eq!(log[2].angle_ticks, DRAFT_MARK);
        assert_eq!((log[3].screen_index, log[3].fuse_tenths), (4, 800));
        assert_eq!(log[3].emplacement_offset, -5, "the offset is sticky too");
    }

    #[test]
    fn the_text_form_rejects_anything_it_would_not_write() {
        use LogError::*;
        let cases: &[(&str, LogError)] = &[
            // Versions: an epoch-1 log has no fuse to give its shots.
            ("", MissingVersion),
            ("@0 F10 900:600", MissingVersion),
            ("v+2 @0", MissingVersion),
            ("v1 @0 900:600", UnsupportedVersion(1)),
            ("v3", UnsupportedVersion(3)),
            // Entries without their context.
            ("v2 900:600", NoScreen { at: 1 }),
            ("v2 D0", NoScreen { at: 1 }),
            ("v2 @0 900:600", NoFuse { at: 2 }),
            // Malformed or non-canonical numbers.
            ("v2 @x", BadToken { at: 1 }),
            ("v2 @0 Fq 1:2", BadToken { at: 2 }),
            ("v2 @0 D", BadToken { at: 2 }),
            ("v2 @0 F1 1:2:3", BadToken { at: 3 }),
            ("v2 @+1", BadToken { at: 1 }),
            ("v2 @01", BadToken { at: 1 }),
            ("v2 @0 F10 E+5 1:2", BadToken { at: 3 }),
            ("v2 @0 F10 E-0 1:2", BadToken { at: 3 }),
            ("v2 @0 F-10 -1:2", BadToken { at: 3 }),
            ("v2 @4294967296", BadToken { at: 1 }),
            // Sticky tokens repeated, restated, out of order or dangling.
            ("v2 @0 @1 F1 1:2", StrayToken { at: 2 }),
            ("v2 @0 F1 1:2 @0 3:4", StrayToken { at: 4 }),
            ("v2 @0 F1 F2 1:2", StrayToken { at: 3 }),
            ("v2 @0 F1 1:2 F1 3:4", StrayToken { at: 4 }),
            ("v2 @0 F1 E0 1:2", StrayToken { at: 3 }),
            ("v2 @0 E5 F1 1:2", StrayToken { at: 3 }),
            ("v2 @0 F1 1:2 @1", StrayToken { at: 4 }),
            ("v2 @0 F1 1:2 F3", StrayToken { at: 4 }),
            ("v2 @0 F1 1:2 F3 D0", StrayToken { at: 4 }),
        ];
        for (text, err) in cases {
            assert_eq!(decode_log(text), Err(*err), "{text:?}");
        }
    }

    #[test]
    fn a_draft_entry_with_a_fuse_or_offset_is_an_error() {
        let played = with_enfilade_and_airburst();
        let d = played
            .log
            .iter()
            .position(|e| e.angle_ticks == DRAFT_MARK)
            .unwrap();
        for (fuse, offset) in [(1, 0), (0, -3)] {
            let mut replay = Replay::of(&played);
            replay.log[d].fuse_tenths = fuse;
            replay.log[d].emplacement_offset = offset;
            assert_eq!(
                replay.reconstruct().err(),
                Some(ReplayError::MalformedDraft { at: d })
            );
        }
    }

    #[test]
    fn two_hundred_runs_replay_to_matching_hashes() {
        // Brief M4 acceptance. CI runs this on Linux, macOS and Windows and
        // compares the printed digest across platforms.
        let mut digest = Hasher::new();
        for i in 0..200u64 {
            let seed = 0x5A17_0000_0000_0000 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let played = play(seed, 320, 160);
            let replay = Replay::of(&played);
            let h = replay.final_hash().expect("replay failed");
            assert_eq!(
                h,
                state_hash(&played),
                "run {i} (seed {seed:#x}) diverged on replay"
            );
            digest.write_u64(h);
        }
        // Printed so the CI job can compare the number between platforms.
        println!("replay-digest: {:#018x}", digest.finish());
    }
}

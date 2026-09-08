//! Seed plus shot log to verified reconstruction. DESIGN §3, brief M4.
//!
//! Nothing else is persisted. A run is `(run_seed, width, log)`, where each
//! log entry is the triple `(screen_index, angle_ticks, power_ticks)`.
//!
//! Draft picks ride in the same triple: an entry with `angle_ticks ==
//! DRAFT_MARK` records "took offer `power_ticks`" rather than a shot. A draft
//! is a player decision, so it has to be in the log for a reconstruction to be
//! exact — and reusing the triple keeps the persisted form to three small
//! numbers per decision, with no second stream to keep in sync.

use crate::ballistics::Shot;
use crate::run::{LoggedShot, Phase, RunState};

/// `angle_ticks` sentinel marking a draft pick rather than a shot. Angles are
/// never negative, so the encoding is unambiguous.
pub const DRAFT_MARK: i32 = -1;

/// A complete, replayable run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    pub run_seed: u64,
    pub width: usize,
    pub log: Vec<LoggedShot>,
}

impl Replay {
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
    pub fn reconstruct(&self) -> Result<RunState, ReplayError> {
        let mut state = RunState::new(self.run_seed, self.width);
        for (i, entry) in self.log.iter().enumerate() {
            if entry.angle_ticks == DRAFT_MARK {
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
            state
                .fire(Shot::new(entry.angle_ticks, entry.power_ticks), false)
                .ok_or(ReplayError::ShotRefused { at: i })?;
            // Headless callers do not wait for a collapse animation.
            state.resolve_clear();
        }
        Ok(state)
    }

    /// Reconstruct and hash. This is the number CI compares across platforms.
    pub fn final_hash(&self) -> Result<u64, ReplayError> {
        self.reconstruct().map(|s| state_hash(&s))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayError {
    UnexpectedShot {
        at: usize,
    },
    UnexpectedDraft {
        at: usize,
    },
    NoSuchOffer {
        at: usize,
    },
    ShotRefused {
        at: usize,
    },
    ScreenMismatch {
        at: usize,
        expected: u32,
        found: u32,
    },
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
    pub fn new() -> Self {
        Self::default()
    }
    pub fn finish(self) -> u64 {
        self.0
    }
    pub fn write_u8(&mut self, v: u8) {
        self.0 ^= v as u64;
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }
    pub fn write_u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }
    pub fn write_i32(&mut self, v: i32) {
        self.write_u64(v as i64 as u64);
    }
    pub fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
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
    }
    h.finish()
}

/// Hash of one screen: terrain, damage, and every structure's state.
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

/// Hash of a screen generated in isolation — used by the batch harness and by
/// the "screen 400 without screens 1..399" acceptance test.
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
                    let angle = r.range_u32(150, 800) as i32;
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
        let played = play(31337, 320, 400);
        let drafts = played
            .log
            .iter()
            .filter(|e| e.angle_ticks == DRAFT_MARK)
            .count();
        assert!(drafts > 0, "the run never reached a draft");
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
        tweaked.log[idx].angle_ticks += 5;
        let h1 = tweaked.final_hash().unwrap();
        assert_ne!(h0, h1, "hash is blind to a changed shot");
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

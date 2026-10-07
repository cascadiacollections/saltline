//! Salt Line — deterministic simulation core.
//!
//! This crate is the whole game: terrain generation, ballistics, damage, the
//! shell economy, the modifier draft and the run state machine. The Godot
//! crate (`salt-line-godot`) renders what this crate says is true, reads
//! input and plays sound (DESIGN §6). Keeping every rule on this side of the
//! split is what makes a run reproducible from its seed and its inputs, and
//! what lets the rules be tested headless, in bulk, without an engine.
//!
//! Invariants (see design/DESIGN.md):
//!   * No Godot dependency. Builds and tests headless.
//!   * Fixed timestep `DT`, semi-implicit Euler, `f64` throughout.
//!   * Explicit PRNG only. No thread_rng, no HashMap iteration order,
//!     no system time in any state-affecting path.
//!   * Wind is an acceleration, never a velocity offset.
//!
//! Where to start reading:
//!
//! - [`run`]: a run as a state machine (DESIGN §1), and how each screen is
//!   assembled and validated from its own seed (§2.2, §2.3).
//! - [`replay`]: a run persisted as seed plus input log and rebuilt from it,
//!   and the state hash that catches determinism leaks (§6, §9).
//! - [`layout`]: a screen as plain integer data rather than a seed, for the
//!   map editor and custom games (§13).
//! - [`SIM_EPOCH`] and [`EPOCHS`]: how a change to the sim's outcomes is
//!   detected and versioned instead of silently breaking old logs (§9.3).
//!
//! Units used throughout: world units are pixels of the internal buffer, y
//! up; angles are integer ticks of 0.05 degrees; power is an integer tick
//! count, `0..=POWER_TICKS_MAX`; time is seconds, stepped by [`DT`]. The
//! Airburst fuse is held on a grid of tenths of a world unit and logged as
//! an integer count of tenths.
//!
//! Comments cite "brief M1" to "brief M6": milestones of the brief the first
//! build was written against, which is no longer in the repository.

pub mod ballistics; // M1 — integration, collision
pub mod dmath; // platform-stable trig, so the sim never calls libm
pub mod layout; // DESIGN §13 — a screen as editable, shareable data
pub mod modifiers; // M3 — pool, draft, loadout deltas
pub mod replay; // M4 — seed + shot log -> verified reconstruction
pub mod rng; // M1 — SplitMix64, PCG32
pub mod run; // M3 — economy, screen assembly, state machine
pub mod solver; // M2 — solvability validation
pub mod terrain; // M1 — noise, heightfield, scar buffer, materials

/// Version of the sim's outcomes. DESIGN §9.3.
///
/// Bumped on any change that alters a replay's outcome: generation,
/// ballistics, damage, the economy, or what the shot log records. A log
/// recorded under one epoch is only replayed by a sim of the same epoch;
/// under any other it keeps its seed and nothing else.
///
/// Enforced rather than remembered: `tests/golden_runs.rs` hashes the pinned
/// screen digests together with the golden-run corpus and asserts the result
/// is this epoch's entry in [`EPOCHS`]. Every bump gets a line in NOTES.md.
///
/// Epoch 1 is everything before the fuse and emplacement offset were logged.
pub const SIM_EPOCH: u32 = 6;

/// Every epoch's digest, oldest first. Append-only: a sim change appends
/// `(SIM_EPOCH, digest)` after bumping `SIM_EPOCH`; an existing entry is never
/// edited, so a digest cannot be replaced without a bump. The tests check
/// that the last entry is `SIM_EPOCH`, that no digest repeats, and that each
/// is recorded in NOTES.md. Epoch 1 predates the digest and has no entry.
pub const EPOCHS: &[(u32, u64)] = &[
    (2, 0xce60_de84_8487_e4a5),
    (3, 0x6471_3d32_7a31_a792),
    (4, 0x1104_ab32_9b37_6f2c),
    (5, 0x9c4e_f5d0_0758_cebe),
    (6, 0x0e22_2e08_a33e_af8a),
];

/// The digest `SIM_EPOCH` vouches for: the last entry of [`EPOCHS`].
pub const EPOCH_DIGEST: u64 = EPOCHS[EPOCHS.len() - 1].1;

/// Fixed simulation timestep. Never make this variable.
pub const DT: f64 = 1.0 / 120.0;

/// Internal buffer height in pixels. Width floats 320..=400 by device aspect.
pub const SCREEN_H: usize = 180;

/// One heightfield sample per pixel column.
pub const TERRAIN_STEP: f64 = 1.0;

/// Screens per band. A modifier draft fires at the end of each band.
pub const BAND_LEN: u32 = 4;

/// Starting shell reserve.
pub const START_SHELLS: i32 = 40;

// ------------------------------------------------------- world scale (§2.5)

/// Narrowest and widest internal buffer. DESIGN §5.1: fix the height, let the
/// width float, generate terrain to fill whatever the device reports.
pub const SCREEN_W_MIN: usize = 320;
/// Widest internal buffer; see [`SCREEN_W_MIN`].
pub const SCREEN_W_MAX: usize = 400;
/// The width the design table in DESIGN §2.5 is tuned against.
pub const SCREEN_W_REF: usize = 320;

/// Emplacement x. DESIGN §2.5: "the emplacement sits at x~30".
pub const EMPLACEMENT_X: f64 = 30.0;
/// Right-hand limit for target placement at the reference width.
pub const FAR_EDGE_X: f64 = 310.0;

// ------------------------------------------------------ ballistics (§3, §2.4)

/// Gravity, world units per second squared.
pub const GRAVITY: f64 = 140.0;
/// Muzzle velocity at power tick 0 and at full power, before loadout scaling.
/// World units per second.
pub const POWER_MIN: f64 = 30.0;
/// Muzzle velocity at [`POWER_TICKS_MAX`], world units per second, before
/// loadout scaling; see [`POWER_MIN`]. Speed is linear in power ticks between.
pub const POWER_MAX: f64 = 300.0;
/// Power is an integer tick count so a shot is exactly reproducible.
pub const POWER_TICKS_MAX: i32 = 1000;

/// Default angle input quantization, in 0.05-degree ticks: 0.5 degrees.
/// Vernier's first stack takes this to 0.1 degrees and its second to 0.05.
pub const ANGLE_QUANTUM_TICKS: i32 = 10;
/// The barrel sprite shows 5-degree steps while the numeric field shows the
/// true value. DESIGN §5.3: that gap is intentional.
pub const BARREL_FRAME_TICKS: i32 = 100;

/// Point projectile with a collision radius (DESIGN §3).
pub const SHELL_RADIUS: f64 = 1.0;
/// Base crater radius before material and loadout multipliers.
pub const BASE_CRATER_RADIUS: f64 = 7.0;
/// Base blast radius for damage overlap tests.
pub const BASE_BLAST_RADIUS: f64 = 9.0;
/// Off-screen horizontally past this many screen widths ends the shot.
pub const MISS_WIDTH_FACTOR: f64 = 1.5;
/// Hard step cap per shell, so a pathological shot cannot hang the sim.
pub const MAX_SHELL_STEPS: u32 = 4800; // 40 s at 1/120

// -------------------------------------------------------------- generation

/// Minimum horizontal separation between targets. DESIGN §2.3 step 5.
pub const MIN_TARGET_SEPARATION: f64 = 40.0;
/// Surviving trajectory candidates required to accept a screen. DESIGN §2.4.
pub const MIN_SOLUTIONS: usize = 3;
/// Flight-time samples swept by the analytic solve. DESIGN §2.4.
pub const SOLVER_TIME_SAMPLES: usize = 40;
/// Reject-and-regenerate cap before falling back to `Flat`. DESIGN §2.3 step 6.
pub const MAX_GEN_ATTEMPTS: u32 = 8;

/// Band index for a screen. DESIGN §2.5 drives everything from this.
#[inline]
pub const fn band_of(screen_index: u32) -> u32 {
    screen_index / BAND_LEN
}

/// A point in world space, y up, one unit per pixel.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    /// World units rightward from the screen's left edge.
    pub x: f64,
    /// World units upward.
    pub y: f64,
}

impl Vec2 {
    /// A point at `(x, y)`, in world units.
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_boundaries_are_where_the_draft_fires() {
        assert_eq!(band_of(0), 0);
        assert_eq!(band_of(3), 0);
        assert_eq!(band_of(4), 1);
        assert_eq!(band_of(400), 100);
    }
}

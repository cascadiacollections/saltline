//! Salt Line — deterministic simulation core.
//!
//! Invariants (see design/DESIGN.md and IMPLEMENTATION_PROMPT.md):
//!   * No Godot dependency. Builds and tests headless.
//!   * Fixed timestep `DT`, semi-implicit Euler, `f64` throughout.
//!   * Explicit PRNG only. No thread_rng, no HashMap iteration order,
//!     no system time in any state-affecting path.
//!   * Wind is an acceleration, never a velocity offset.

pub mod ballistics; // M1 — integration, collision
pub mod dmath; // platform-stable trig, so the sim never calls libm
pub mod modifiers; // M3 — pool, draft, loadout deltas
pub mod replay;
pub mod rng; // M1 — SplitMix64, PCG32
pub mod run; // M3 — economy, screen assembly, state machine
pub mod solver; // M2 — solvability validation
pub mod terrain; // M1 — noise, heightfield, scar buffer, materials // M4 — seed + shot log -> verified reconstruction

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
pub const POWER_MIN: f64 = 30.0;
pub const POWER_MAX: f64 = 300.0;
/// Power is an integer tick count so a shot is exactly reproducible.
pub const POWER_TICKS_MAX: i32 = 1000;

/// Default angle input quantization, in 0.1-degree ticks: 0.5 degrees.
/// `Vernier` takes this to 1 tick (DESIGN §4).
pub const ANGLE_QUANTUM_TICKS: i32 = 5;
/// The barrel sprite shows 5-degree steps while the numeric field shows the
/// true value. DESIGN §5.3: that gap is intentional.
pub const BARREL_FRAME_TICKS: i32 = 50;

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
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
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

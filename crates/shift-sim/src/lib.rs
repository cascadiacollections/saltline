//! Game Off 2026 entry rules (gameoff/DESIGN.md), built on `salt-line-sim`.
//!
//! Every stage exists in two phases, Salt and Brine. Each phase is an
//! ordinary [`salt_line_sim::run::Screen`] generated from its own sub-stream
//! of the stage seed, so terrain, structures, wind and damage are tracked per
//! phase by the sim's existing model and never leak between phases.
//!
//! Invariants, inherited from the sim:
//!   * Deterministic: everything derives from the run seed and the input log.
//!   * No engine dependency. Builds and tests headless.
//!   * `salt-line-sim` is used, never changed: its epoch and golden runs are
//!     not this crate's to move.

pub mod stage;

pub use stage::{Plane, Stage};

/// Version of this crate's input-log format and outcomes. Bump on any change
/// that alters a run's outcome from the same seed and inputs.
pub const SHIFT_LOG_VERSION: u32 = 1;

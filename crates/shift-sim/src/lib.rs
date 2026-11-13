//! Game Off 2026 entry rules (gameoff/DESIGN.md), built on `salt-line-sim`.
//!
//! Every stage exists in two eras: Salt, the present, fired with a howitzer,
//! and Siege, 1347, fired with a trebuchet. Each era is an ordinary
//! [`salt_line_sim::run::Screen`] generated from its own sub-stream of the
//! run seed, so terrain, structures, wind and damage are tracked per era by
//! the sim's existing model and never leak between eras, except through
//! echoes ([`stage`]).
//!
//! Invariants, inherited from the sim:
//!   * Deterministic: everything derives from the run seed and the input log.
//!   * No engine dependency. Builds and tests headless.
//!   * `salt-line-sim` is used, never changed: its epoch and golden runs are
//!     not this crate's to move.

pub mod bot;
pub mod expedition;
pub mod log;
pub mod stage;

pub use expedition::{Event, Expedition, Input, PhaseMod, Pick, State};
pub use stage::{Plane, Role, Stage};

/// Version of this crate's input-log format and outcomes. Bump on any change
/// that alters a run's outcome from the same seed and inputs; a log of
/// another version is refused rather than replayed wrong.
pub const SHIFT_LOG_VERSION: u32 = 1;

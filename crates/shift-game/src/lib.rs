//! Counterweight, the Game Off 2026 entry: everything but the engine.
//!
//! The rules live in `shift-sim` (on `salt-line-sim`). This crate is the
//! game around them: modes and input ([`app`]), drawing into a 320x180 RGBA
//! buffer ([`render`], [`canvas`], [`sprites`], [`font`]) and sound cues
//! ([`audio`]). It never calls an engine, so it is tested headless and can
//! render review frames without Godot; `shift-godot` is the thin node that
//! feeds it input and uploads its frame.

pub mod app;
pub mod audio;
pub mod canvas;
pub mod font;
pub mod render;
pub mod sprites;

pub use app::{Button, Effect, Game, Held, Input, Pointer};
pub use canvas::{Canvas, H, W};
pub use render::Renderer;

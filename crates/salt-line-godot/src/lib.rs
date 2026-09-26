//! Godot 4 presentation layer. Rendering, input, UI only — no game logic.
//!
//! Terrain draws as vertical spans of solid colour, one pixel column each,
//! straight into an image buffer. No tilemap, no mesh, no sprites.
//! Camera x is quantised to whole pixels every frame.
//!
//! Everything that decides anything is in `salt-line-sim`, which has no idea
//! this crate exists.

use godot::prelude::*;

pub mod aim;
pub mod font;
pub mod game;
pub mod icons;
pub mod palette;
pub mod presentation;
pub mod raster;
pub mod save;
pub mod save_format;
pub mod sprites;
pub mod store;
pub mod touch;

struct SaltLineExtension;

#[gdextension]
unsafe impl ExtensionLibrary for SaltLineExtension {}

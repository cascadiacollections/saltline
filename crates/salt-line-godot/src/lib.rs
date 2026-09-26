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
pub mod crash;
pub mod durable;
pub mod font;
pub mod frame;
pub mod game;
pub mod icons;
pub mod keypad;
pub mod pads;
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
unsafe impl ExtensionLibrary for SaltLineExtension {
    fn on_stage_init(stage: InitStage) {
        // Scene is the first stage the node's classes, and `ProjectSettings`
        // for finding `user://`, can be relied on. gdext installed its own
        // panic hook before this runs; `crash::install` chains it.
        if stage == InitStage::Scene {
            crash::install(save::on_disk(crash::REPORT_PATH));
        }
    }
}

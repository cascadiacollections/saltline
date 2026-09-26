//! Godot 4 presentation layer: rendering, input, UI. No game rules.
//!
//! Terrain draws as vertical spans of solid colour, one pixel column each,
//! straight into an image buffer. No tilemap, no mesh, no sprites.
//! Camera x is quantised to whole pixels every frame.
//!
//! Everything that decides anything is in `salt-line-sim`, which has no idea
//! this crate exists. Within this crate, the game (`app`) and its drawing
//! (`render`) are engine-free and run under `cargo test`; `game` is the one
//! Godot node, a thin adapter around them. See `app` for how they fit.

use godot::prelude::*;

pub mod aim;
pub mod app;
pub mod cli;
pub mod crash;
pub mod durable;
pub mod font;
pub mod frame;
pub mod game;
pub mod icons;
pub mod input;
pub mod keypad;
pub mod pads;
pub mod palette;
pub mod presentation;
pub mod raster;
pub mod render;
pub mod resume;
pub mod save;
pub mod save_format;
pub mod sprites;
pub mod touch;

struct SaltLineExtension;

#[gdextension]
unsafe impl ExtensionLibrary for SaltLineExtension {
    fn on_stage_init(stage: InitStage) {
        // Scene is the first stage the node's classes, and `ProjectSettings`
        // for finding `user://`, can be relied on. gdext installed its own
        // panic hook before this runs; `crash::install` chains it.
        //
        // Not in the editor: a panic there is the developer's to read in the
        // output panel, not a player's crash, and the editor hot-reloads this
        // library (`reloadable = true`), which is where a hook left pointing
        // into an unloaded library would bite.
        if stage == InitStage::Scene && !godot::classes::Engine::singleton().is_editor_hint() {
            crash::install(save::on_disk(crash::REPORT_PATH));
        }
    }

    fn on_stage_deinit(stage: InitStage) {
        // The hook is code in this library; take it out before the library
        // goes. Does nothing if it was never installed.
        if stage == InitStage::Scene {
            crash::uninstall();
        }
    }
}

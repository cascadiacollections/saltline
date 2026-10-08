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
//!
//! # Where to start reading
//!
//! Follow one frame: [`game`]'s `process` gathers input into
//! [`input::Intents`], hands it to [`app::Game`], applies the
//! [`app::Effect`]s that come back, and, if [`frame::Redraw`] says the frame
//! changed, has [`render`] paint the game into a [`raster::Canvas`], which is
//! uploaded to the GPU in strips. The rules themselves are in `salt-line-sim`;
//! `design/DESIGN.md` §5 is the art direction this crate holds to.
//!
//! # Crate map
//!
//! ```text
//! entry      lib            the GDExtension entry point; installs the crash hook
//! adapter    game           the one Godot node: engine events in, effects out
//! core       app            the game: modes, menus, runs, effects (engine-free)
//!            input, touch,  keys and pad buttons, on-screen buttons, slingshot
//!            aim, keypad    aiming, the seed keypad
//!            presentation   timers, held-input repeat, onboarding
//!            versus         local two-player rules: duel screens, mirroring, turns
//!            cli            developer flags
//! drawing    render         a Game painted into a Canvas (engine-free)
//!            raster         the Canvas and its drawing primitives
//!            palette, font, the 16 colours, bitmap fonts, HUD icons,
//!            icons, sprites sprite sheets (palette and sprites load via Godot)
//! pacing     frame          redraw only when something changed; idle frame cap
//!            strips         upload only the parts of the frame that changed
//!            pads           which joypads, and which stick axes, to trust
//! disk       save_format    the save file's text format (engine-free)
//!            durable        atomic replace and which copy to trust on load
//!            save           `user://` paths and file IO through them
//!            resume         the in-progress run file, for CONTINUE
//!            match_log      the suspended versus match's file, for RESUME MATCH
//!            crash          the panic hook and its report
//! dev        profiling      opt-in (`--features profiling`) frame-upload timing
//! ```
//!
//! "Engine-free" means the module compiles and is tested under `cargo test`
//! with no Godot running. The modules that call into the engine are `lib`,
//! `game`, `save`, `palette` and `sprites` (texture loading), `profiling`
//! (only to print its summary), and `raster` only to hand its bytes over as a
//! `PackedByteArray`.

use godot::prelude::*;

pub mod aim;
pub mod app;
pub mod cli;
pub mod crash;
pub mod custom;
pub mod durable;
pub mod editor;
pub mod font;
pub mod frame;
pub mod game;
pub mod icons;
pub mod input;
pub mod keypad;
pub mod layouts_file;
pub mod match_log;
pub mod pads;
pub mod palette;
pub mod presentation;
#[cfg(feature = "profiling")]
mod profiling;
pub mod raster;
pub mod render;
pub mod resume;
pub mod save;
pub mod save_format;
pub mod sky;
pub mod sprites;
pub mod strips;
pub mod touch;
pub mod versus;

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

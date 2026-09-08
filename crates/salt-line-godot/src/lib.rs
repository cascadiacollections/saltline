//! Godot 4 presentation layer. Rendering, input, UI only — no game logic.
//!
//! Terrain draws as vertical spans of solid colour, one pixel column each,
//! straight into an image buffer. No tilemap, no mesh, no sprites.
//! Camera x is quantised to whole pixels every frame.

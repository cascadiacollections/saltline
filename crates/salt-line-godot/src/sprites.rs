//! Sprite loading and slicing. DESIGN §5.3, §5.4.
//!
//! No sprite is ever scaled or rotated. The barrel is nineteen pre-drawn
//! frames in a horizontal strip, sliced at 28px, because runtime rotation at
//! this resolution produces stair-stepping that changes shape frame to frame.

use godot::classes::Texture2D;
use godot::prelude::*;
use salt_line_sim::modifiers::{Loadout, Modifier};
use salt_line_sim::run::{StructState, StructureKind};

use crate::raster::Sprite;

/// Width of one barrel frame in the strip.
pub const BARREL_FRAME: i32 = 28;
/// Frames in the strip: 0..90 degrees in 5-degree steps.
pub const BARREL_FRAMES: usize = 19;

/// Barrel pivot inside the tank sprite, from `gen_sprites.py::tank`:
/// `(x0 + 13, ty - 1)` for the base chassis, one lower and two left for the
/// wider Ballast chassis.
pub const PIVOT_BASE: (i32, i32) = (15, 3);
pub const PIVOT_BALLAST: (i32, i32) = (13, 4);
/// Pivot inside a barrel frame: its centre.
pub const BARREL_PIVOT: (i32, i32) = (14, 14);

pub struct Sprites {
    pub tank_base: Option<Sprite>,
    pub tank_ballast: Option<Sprite>,
    pub tank_forced: Option<Sprite>,
    pub barrel_base: Option<Sprite>,
    pub barrel_sabot: Option<Sprite>,
    pub barrel_cluster: Option<Sprite>,
    pub barrel_auger: Option<Sprite>,
    pub tower_intact: Option<Sprite>,
    pub tower_leaning: Option<Sprite>,
    pub tower_collapsed: Option<Sprite>,
    pub derrick: Option<Sprite>,
    pub radio_mast: Option<Sprite>,
    pub silo: Option<Sprite>,
    pub pylon: Option<Sprite>,
    pub missing: Vec<String>,
}

/// Read a PNG into a plain RGBA byte buffer.
///
/// The sprites become Rust-side pixels rather than engine textures because
/// every one of them is composited into the same native-resolution buffer the
/// terrain is drawn into. One buffer, one upload, one draw call — and no way
/// for a sprite to end up sampled or filtered on its way to the screen.
fn load(path: &str, missing: &mut Vec<String>) -> Option<Sprite> {
    // Loaded as a `Texture2D` resource rather than with `Image::load_from_file`
    // so it keeps working in an exported build, where the PNG has become a
    // .ctex and the raw file is gone.
    let Ok(texture) = try_load::<Texture2D>(path) else {
        missing.push(path.to_string());
        return None;
    };
    let Some(mut image) = texture.get_image() else {
        missing.push(path.to_string());
        return None;
    };
    if image.get_width() <= 0 {
        missing.push(path.to_string());
        return None;
    }
    if image.get_format() != godot::classes::image::Format::RGBA8 {
        image.convert(godot::classes::image::Format::RGBA8);
    }
    let (w, h) = (image.get_width(), image.get_height());
    let data = image.get_data();
    let px = data.to_vec();
    if px.len() < (w * h * 4) as usize {
        missing.push(path.to_string());
        return None;
    }
    Some(Sprite { w, h, px })
}

impl Sprites {
    pub fn load_all(dir: &str) -> Sprites {
        let mut missing = Vec::new();
        let p = |name: &str| format!("{dir}/{name}.png");
        let s = Sprites {
            tank_base: load(&p("tank_base"), &mut missing),
            tank_ballast: load(&p("tank_ballast"), &mut missing),
            tank_forced: load(&p("tank_forced_march"), &mut missing),
            barrel_base: load(&p("barrel_strip_5deg"), &mut missing),
            barrel_sabot: load(&p("barrel_strip_sabot"), &mut missing),
            barrel_cluster: load(&p("barrel_strip_cluster"), &mut missing),
            barrel_auger: load(&p("barrel_strip_auger"), &mut missing),
            tower_intact: load(&p("tower_intact"), &mut missing),
            tower_leaning: load(&p("tower_leaning"), &mut missing),
            tower_collapsed: load(&p("tower_collapsed"), &mut missing),
            derrick: load(&p("derrick"), &mut missing),
            radio_mast: load(&p("radio_mast"), &mut missing),
            silo: load(&p("silo"), &mut missing),
            pylon: load(&p("pylon"), &mut missing),
            missing,
        };
        if !s.missing.is_empty() {
            godot_warn!("salt-line: missing sprites: {:?}", s.missing);
        }
        s
    }

    /// Chassis for the current build. DESIGN §5.3: the player should be able
    /// to identify their build from the silhouette alone.
    pub fn tank(&self, lo: &Loadout) -> (Option<&Sprite>, (i32, i32)) {
        if lo.has(Modifier::ForcedMarch) {
            // Loses its highlight tone, reads darker.
            (self.tank_forced.as_ref(), PIVOT_BASE)
        } else if lo.has(Modifier::Ballast) {
            // Widens to 30px and sits 1px lower.
            (self.tank_ballast.as_ref(), PIVOT_BALLAST)
        } else {
            (self.tank_base.as_ref(), PIVOT_BASE)
        }
    }

    /// Barrel strip for the current build. Sabot lengthens and thins it,
    /// Cluster shortens and thickens it, Auger adds a muzzle bulge.
    pub fn barrel(&self, lo: &Loadout) -> Option<&Sprite> {
        let pick = if lo.has(Modifier::Sabot) {
            self.barrel_sabot.as_ref()
        } else if lo.has(Modifier::Cluster) {
            self.barrel_cluster.as_ref()
        } else if lo.auger() {
            self.barrel_auger.as_ref()
        } else {
            self.barrel_base.as_ref()
        };
        pick.or(self.barrel_base.as_ref())
    }

    /// Left edge of a barrel frame in the strip. `frame` is the angle
    /// quantized to 5 degrees — the sprite is coarse feedback, the numeric
    /// field is the instrument, and that gap is intentional (DESIGN §5.3).
    pub fn barrel_frame_x(frame: usize) -> i32 {
        frame.min(BARREL_FRAMES - 1) as i32 * BARREL_FRAME
    }

    /// How far a kind's sprite sinks into the ground, in pixels. Taken from
    /// the blits in `gen_sprites.py::scene`, so the running game seats
    /// structures exactly the way the reference art does.
    pub fn seat_offset(kind: StructureKind) -> i32 {
        match kind {
            StructureKind::WaterTower => 3,
            StructureKind::RadioMast => 1,
            StructureKind::Derrick => 2,
            StructureKind::Silo => 2,
            StructureKind::Pylon => 2,
        }
    }

    /// Structure sprite for a kind and damage state.
    ///
    /// Only the water tower has all three states drawn — it is the one the
    /// brief wires up end to end, because a bulb on legs makes undermining
    /// legible. The others fall back to the shared rubble sprite when they go
    /// down, and to their intact silhouette while leaning. Those are the
    /// drawings to add next; dimensions and palette stay fixed.
    pub fn structure(&self, kind: StructureKind, state: StructState) -> (Option<&Sprite>, bool) {
        let intact = match kind {
            StructureKind::WaterTower => self.tower_intact.as_ref(),
            StructureKind::Derrick => self.derrick.as_ref(),
            StructureKind::RadioMast => self.radio_mast.as_ref(),
            StructureKind::Silo => self.silo.as_ref(),
            StructureKind::Pylon => self.pylon.as_ref(),
        };
        match state {
            StructState::Intact => (intact, true),
            StructState::Leaning => {
                if kind == StructureKind::WaterTower {
                    (self.tower_leaning.as_ref(), true)
                } else {
                    // No leaning drawing yet: fall back, and let the shifted
                    // bounding box do the talking.
                    (intact, false)
                }
            }
            StructState::Collapsed => (
                self.tower_collapsed.as_ref(),
                kind == StructureKind::WaterTower,
            ),
        }
    }
}

//! The fixed 16-colour palette, sampled from `art/palette_16.png`.
//!
//! DESIGN §5.2: sixteen colours, fixed, no exceptions — and the brief says to
//! sample the swatch image programmatically rather than transcribing hex
//! values by hand. So the image is the single source of truth; edit
//! `tools/gen_sprites.py`, regenerate, and the game follows.
//!
//! Everything the game draws goes into one native-resolution
//! [`Canvas`](crate::raster::Canvas) (DESIGN §5.1), and every colour that
//! reaches it comes from this table by slot index. Code names a *role*
//! ([`slot::ALERT`], [`slot::UI_FG`]) rather than an RGB value, so the
//! allocation in DESIGN §5.2 (four terrain tones, three sky, three hull,
//! three structure, two UI, one alert) is visible at every call site. Slots
//! are plain `usize` constants, so nothing stops a stray `[u8; 4]` literal;
//! the rule is kept by drawing only through [`Palette::get`]. Read DESIGN §5
//! before adding a colour: the answer is to reuse a slot.
//!
//! Colours are stored as straight RGBA bytes. The software rasteriser writes
//! bytes into the buffer, so converting from Godot's float `Color` happens
//! once, at load, instead of once per pixel.

use godot::classes::{Image, Texture2D};
use godot::prelude::*;

/// Slot names in the order they appear in `art/palette_16.png`, matching
/// `gen_sprites.py`. `SLOTS[i]` is the name of the colour at [`slot`] index
/// `i`. Nothing reads it at runtime; it records the order `gen_sprites.py`
/// writes the swatches in.
pub const SLOTS: [&str; 16] = [
    "sand",
    "rock",
    "bedrock",
    "shadow",
    "sky_hi",
    "sky_mid",
    "sky_lo",
    "hull",
    "hull_hi",
    "hull_edge",
    "st",
    "st_hi",
    "st_dmg",
    "ui_fg",
    "ui_bg",
    "alert",
];

/// Indices into the palette. Named after their DESIGN §5.2 role.
///
/// Each is a valid index into [`Palette::colours`], `0..16`.
pub mod slot {
    pub const SAND: usize = 0;
    pub const ROCK: usize = 1;
    pub const BEDROCK: usize = 2;
    pub const SHADOW: usize = 3;
    pub const SKY_HI: usize = 4;
    pub const SKY_MID: usize = 5;
    pub const SKY_LO: usize = 6;
    pub const HULL: usize = 7;
    pub const HULL_HI: usize = 8;
    pub const HULL_EDGE: usize = 9;
    pub const ST: usize = 10;
    pub const ST_HI: usize = 11;
    pub const ST_DMG: usize = 12;
    pub const UI_FG: usize = 13;
    pub const UI_BG: usize = 14;
    pub const ALERT: usize = 15;
}

/// One colour as `[r, g, b, a]` bytes, the layout of an RGBA8 image row.
/// Palette colours are always fully opaque (`a == 255`).
pub type Rgba = [u8; 4];

/// The palette as straight bytes, so the rasteriser never touches a float.
#[derive(Clone, Debug)]
pub struct Palette {
    /// The sixteen colours, indexed by [`slot`].
    pub colours: [Rgba; 16],
    /// True when the swatch image was found and sampled.
    pub sampled: bool,
}

/// Fallback, used only if `art/palette_16.png` is missing from the export.
/// Identical to the values `gen_sprites.py` writes into the image.
const FALLBACK: [Rgba; 16] = [
    [232, 220, 200, 255],
    [168, 150, 128, 255],
    [107, 92, 77, 255],
    [74, 62, 54, 255],
    [36, 54, 92, 255],
    [60, 84, 128, 255],
    [110, 134, 168, 255],
    [90, 107, 88, 255],
    [126, 143, 118, 255],
    [46, 56, 48, 255],
    [181, 103, 74, 255],
    [216, 144, 122, 255],
    [110, 58, 44, 255],
    [245, 239, 226, 255],
    [46, 42, 38, 255],
    [232, 163, 61, 255],
];

impl Default for Palette {
    fn default() -> Self {
        Palette {
            colours: FALLBACK,
            sampled: false,
        }
    }
}

impl Palette {
    /// The colour in `slot` (see [`slot`]).
    ///
    /// Out-of-range indices clamp to 15 rather than panic, so a bad index
    /// shows up on screen as the alert colour instead of crashing a frame.
    #[inline]
    pub fn get(&self, slot: usize) -> Rgba {
        self.colours[slot.min(15)]
    }

    /// Same colour as a Godot `Color`, for the few things drawn as vectors.
    pub fn color(&self, slot: usize) -> Color {
        let c = self.get(slot);
        Color::from_rgba8(c[0], c[1], c[2], c[3])
    }

    /// Read the sixteen swatches out of `art/palette_16.png`.
    ///
    /// The swatch sheet is sixteen 12px cells across the top row, written at
    /// 4x (so 48px cells in the file), and cell `i` is sampled at its centre.
    /// The cell size is derived from the image width, so the scale factor
    /// can change without touching this code.
    ///
    /// Never fails: a missing or undersized sheet logs a warning and returns
    /// [`Palette::default`], with [`Palette::sampled`] left `false`.
    pub fn load(path: &str) -> Palette {
        let Ok(image) = try_load_image(path) else {
            godot_warn!("salt-line: {path} missing, falling back to built-in palette");
            return Palette::default();
        };
        let w = image.get_width();
        let h = image.get_height();
        if w < 16 || h < 4 {
            godot_warn!("salt-line: {path} is {w}x{h}, too small to be the palette sheet");
            return Palette::default();
        }
        // Cell width in whatever scale the sheet was written at.
        let cell = w / 16;
        let y = (cell / 2).min(h - 1);
        // Any start value would do; every slot is overwritten below.
        let mut colours = FALLBACK;
        for (i, c) in colours.iter_mut().enumerate() {
            let x = (i as i32 * cell + cell / 2).min(w - 1);
            let col = image.get_pixel(x, y);
            // Godot hands back float channels in 0.0..=1.0; round rather than
            // truncate so a byte round-trips through the float exactly. Alpha
            // is forced opaque: the palette has no translucent colours.
            *c = [
                (col.r * 255.0).round() as u8,
                (col.g * 255.0).round() as u8,
                (col.b * 255.0).round() as u8,
                255,
            ];
        }
        Palette {
            colours,
            sampled: true,
        }
    }
}

/// Read the swatch sheet as an imported `Texture2D` resource, so it survives
/// an export (where the PNG has become a .ctex).
fn try_load_image(path: &str) -> Result<Gd<Image>, ()> {
    let texture = try_load::<Texture2D>(path).map_err(|_| ())?;
    match texture.get_image() {
        Some(img) if img.get_width() > 0 => Ok(img),
        _ => Err(()),
    }
}

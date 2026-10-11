//! Terrain generation, heightfield, scar buffer, materials. DESIGN §2.
//!
//! The noise, fBm, domain warp and crater math are ported from
//! `tools/gen_sprites.py` — the constants there are the ones that produce
//! `art/scene_native_320x180.png`, so they are reproduced exactly rather than
//! re-derived. The one change is orientation: the generator works in screen
//! space (y down, `base_y = 128`), the sim works in world space (y up from the
//! bottom of the screen buffer, y = 0), so `elevation = 180 - screen_y` and `BASE_ELEVATION = 52`.
//!
//! Buffers are `f64`, not `f32` as DESIGN §2.1 says: the brief's determinism
//! constraint ("`f64` throughout the sim") is non-negotiable and outranks it.
//! Quantization to integers still happens only at render time.
//!
//! The module covers steps 1–4 of the DESIGN §2.3 pipeline:
//!
//! ```text
//! base_ridge   value-noise fBm, domain-warped                  (steps 1, 2)
//! shape        template: damped relief plus added features,
//!              clamped                                         (step 3)
//! Heightfield  base + scar + material buffers                  (§2.1, step 4)
//! ```
//!
//! Target placement and the solvability gate (steps 5 and 6) live in `run.rs`
//! and [`crate::solver`], because they depend on the loadout.
//!
//! After generation only the scar buffer changes: [`Heightfield::crater`]
//! digs and clamps at [`WORLD_FLOOR`]; [`Heightfield::mound`] piles debris
//! and clamps at [`WORLD_CEIL`]. Both recompute the surface material from
//! excavation depth. Keeping `base` untouched is what lets a
//! replay rebuild a screen from its seed and re-apply the shots.

use crate::dmath::{self, abs, clamp, smoothstep};
use crate::rng::mix64;
use crate::{SCREEN_H, TERRAIN_STEP};

// ----------------------------------------------------------------- materials

/// Surface layer id. DESIGN §2.3 step 4.
///
/// Layers are defined by depth below the *generated* surface, so untouched
/// ground is always Sand and deeper layers appear only as craters dig down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Material {
    /// Top [`SAND_DEPTH`] units. Craters widest (×1.4).
    Sand = 0,
    /// From [`SAND_DEPTH`] to [`ROCK_DEPTH`].
    Rock = 1,
    /// Below [`ROCK_DEPTH`]. Stops Auger.
    Bedrock = 2,
    /// Structures. Never stored in the heightfield: the run uses it for a
    /// direct hit on a structure.
    Steel = 3,
}

/// Depth below the original surface at which `Rock` begins.
pub const SAND_DEPTH: f64 = 8.0;
/// Depth below the original surface at which `Bedrock` begins.
pub const ROCK_DEPTH: f64 = 30.0;

impl Material {
    /// Crater radius multiplier. DESIGN §2.6.
    pub const fn crater_mult(self) -> f64 {
        match self {
            Material::Sand => 1.4,
            Material::Rock => 1.0,
            Material::Bedrock => 0.55,
            Material::Steel => 0.3,
        }
    }

    /// Layer at `depth` world units below the original surface. Never returns
    /// Steel; zero and negative depths are Sand.
    pub fn at_depth(depth: f64) -> Material {
        if depth < SAND_DEPTH {
            Material::Sand
        } else if depth < ROCK_DEPTH {
            Material::Rock
        } else {
            Material::Bedrock
        }
    }

    /// Auger tunnels through one layer; bedrock stops it. DESIGN §4.
    pub const fn is_tunnelable(self) -> bool {
        matches!(self, Material::Sand | Material::Rock)
    }
}

// --------------------------------------------------------------------- noise

/// 32-bit integer hash. Port of `gen_sprites.py::h1`.
///
/// Maps a lattice index to a pseudo-random value in `[0, 1]`. Xor-shift and
/// multiply rounds, using the multipliers of MurmurHash3's 32-bit finaliser,
/// scramble neighbouring indices into unrelated values. Only the low 32 bits
/// of `n` take part.
fn hash32(n: i64, seed: u32) -> f64 {
    let mut n = (n as i32 as u32) ^ seed;
    n = (n ^ (n >> 13)).wrapping_mul(0x85EB_CA6B);
    n = (n ^ (n >> 16)).wrapping_mul(0xC2B2_AE35);
    let out = n ^ (n >> 13);
    out as f64 / 4294967295.0
}

/// Value noise with smoothstep interpolation. Port of `gen_sprites.py::vnoise`.
/// Value noise rather than Perlin per DESIGN §2.3 step 1 — the blockier
/// character suits flat-colour art.
///
/// One-dimensional value noise: a random value at each integer lattice point
/// (`hash32`), blended between neighbours with [`smoothstep`] so the curve
/// is smooth where the lattice cells meet. Output is in `[0, 1]`. Perlin
/// (gradient) noise would instead place random slopes at the lattice points.
pub fn vnoise(x: f64, seed: u32) -> f64 {
    let i = dmath::floor(x);
    let f = smoothstep(x - i);
    let i = i as i64;
    hash32(i, seed) * (1.0 - f) + hash32(i + 1, seed) * f
}

/// Octaves of value noise. DESIGN §2.3: 4 octaves, lacunarity 2.0, gain 0.5.
pub const OCTAVES: u32 = 4;
/// Frequency multiplier from one octave to the next.
pub const LACUNARITY: f64 = 2.0;
/// Amplitude multiplier from one octave to the next.
pub const GAIN: f64 = 0.5;
/// Per-octave seed stride, from `gen_sprites.py::fbm`.
const OCTAVE_STRIDE: u32 = 7919;

/// Fractal Brownian motion: [`OCTAVES`] layers of [`vnoise`], each at
/// [`LACUNARITY`] times the frequency and [`GAIN`] times the amplitude of the
/// last. Large, slow hills come from the first octave and detail from the
/// later ones. Dividing by the summed amplitudes keeps the result in `[0, 1]`.
/// Each octave gets its own seed so their lattices do not line up.
pub fn fbm(x: f64, seed: u32) -> f64 {
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..OCTAVES {
        sum += vnoise(x * freq, seed.wrapping_add(o.wrapping_mul(OCTAVE_STRIDE))) * amp;
        norm += amp;
        amp *= GAIN;
        freq *= LACUNARITY;
    }
    sum / norm
}

// ----------------------------------------------- base ridge constants (§2.3)

/// Surface elevation with zero relief. `180 - 128` from the generator.
pub const BASE_ELEVATION: f64 = 52.0;
/// Domain warp frequency and strength — DESIGN §2.3 step 2, "~15 units"
/// offset either side of the sample point.
///
/// Frequencies are noise-lattice cells per world unit, so one lattice cell
/// spans `1 / freq` pixels: about 170 for the warp field.
pub const WARP_FREQ: f64 = 0.006;
/// Full width of the warp offset. The warp noise is centred by subtracting
/// 0.5 before scaling, so this gives up to ±15 units.
pub const WARP_STRENGTH: f64 = 30.0;
/// Frequency of the detail layer's first octave: cells about 70 px across.
pub const DETAIL_FREQ: f64 = 0.014;
/// Height range of the detail layer, in world units, before band amplitude.
pub const DETAIL_AMP: f64 = 26.0;
/// Frequency of the broad ridge layer's first octave: cells 250 px across.
pub const RIDGE_FREQ: f64 = 0.004;
/// Height range of the ridge layer, in world units, before band amplitude.
pub const RIDGE_AMP: f64 = 20.0;

/// Lowest surface elevation the world permits. Craters clamp here so the
/// heightfield can never invert (DESIGN §2.6).
pub const WORLD_FLOOR: f64 = 12.0;
/// Highest surface elevation, leaving headroom for a 48px structure.
pub const WORLD_CEIL: f64 = 108.0;

/// Seed offsets, preserving the relationships in `gen_sprites.py::terrain`.
fn field_seeds(screen_seed: u64) -> (u32, u32, u32) {
    let s = mix64(screen_seed) as u32;
    (s, s.wrapping_add(555), s.wrapping_add(191))
}

/// Steps 1 and 2 of DESIGN §2.3: base ridge plus domain warp. Returns
/// elevation above y = 0, one sample per pixel column.
///
/// `amp` is the band's terrain amplitude multiplier (DESIGN §2.5, 1.0 rising
/// to 2.2). Heights run from [`BASE_ELEVATION`] up to
/// `BASE_ELEVATION + (DETAIL_AMP + RIDGE_AMP) * amp`; [`shape`] clamps them
/// into the world afterwards.
pub fn base_ridge(width: usize, screen_seed: u64, amp: f64) -> Vec<f64> {
    let (s_detail, s_warp, s_ridge) = field_seeds(screen_seed);
    let mut out = Vec::with_capacity(width);
    for i in 0..width {
        let x = i as f64 * TERRAIN_STEP;
        // Domain warp: offset where the noise is sampled, not the height it
        // returns. That bends hills sideways into lopsided shapes instead of
        // just making them taller or shorter.
        let warp = (fbm(x * WARP_FREQ, s_warp) - 0.5) * WARP_STRENGTH;
        let n = fbm((x + warp) * DETAIL_FREQ, s_detail);
        let ridge = fbm((x + warp) * RIDGE_FREQ, s_ridge);
        out.push(BASE_ELEVATION + (n * DETAIL_AMP + ridge * RIDGE_AMP) * amp);
    }
    out
}

// ------------------------------------------------------------------ template

/// Coarse shape applied over the base ridge. DESIGN §2.3 step 3.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Template {
    /// Gentle roll, target in the open. Keeps all the noise relief.
    Flat = 0,
    /// One tall obstruction a little past mid-way to the target.
    Ridge = 1,
    /// A gap in front of the target, which sits on a raised far lip.
    Chasm = 2,
    /// Flat-topped plateau with the target on top.
    Mesa = 3,
    /// Narrow columns between emplacement and target
    /// ([`ShapeParams::spire_count`], at most three).
    Spires = 4,
    /// A thin wall just in front of a sunken pocket holding the target.
    Overhang = 5,
    /// Target down in a wide depression, with a rim on the near side.
    Basin = 6,
}

impl Template {
    /// Every template, in discriminant order.
    pub const ALL: [Template; 7] = [
        Template::Flat,
        Template::Ridge,
        Template::Chasm,
        Template::Mesa,
        Template::Spires,
        Template::Overhang,
        Template::Basin,
    ];

    /// First band the template may appear in. DESIGN §2.3 step 3 table.
    pub const fn min_band(self) -> u32 {
        match self {
            Template::Flat | Template::Ridge => 0,
            Template::Chasm => 1,
            Template::Mesa => 2,
            Template::Spires => 3,
            Template::Overhang => 5,
            Template::Basin => 6,
        }
    }

    /// `Overhang` fails the direct solvability check by construction and is
    /// gated on the loadout, not just the band. DESIGN §2.4.
    pub const fn needs_loadout_unlock(self) -> bool {
        matches!(self, Template::Overhang)
    }

    /// Display name, as used in logs and the batch report.
    pub const fn name(self) -> &'static str {
        match self {
            Template::Flat => "Flat",
            Template::Ridge => "Ridge",
            Template::Chasm => "Chasm",
            Template::Mesa => "Mesa",
            Template::Spires => "Spires",
            Template::Overhang => "Overhang",
            Template::Basin => "Basin",
        }
    }
}

/// Everything the shaping pass needs that comes from outside the terrain
/// module — the emplacement, where the primary target wants to sit, and the
/// band-driven elevation delta (DESIGN §2.5).
#[derive(Clone, Copy, Debug)]
pub struct ShapeParams {
    /// Screen width in columns.
    pub width: usize,
    /// Terrain amplitude multiplier for the band; used by [`base_ridge`].
    pub amp: f64,
    /// Emplacement x, world units.
    pub muzzle_x: f64,
    /// Where the primary target wants to sit, world units. Templates build
    /// their features relative to it.
    pub target_x: f64,
    /// Signed height change, world units, blended in from the emplacement
    /// to the target.
    pub elev_delta: f64,
    /// Number of columns for [`Template::Spires`]; values above 3 act as 3.
    pub spire_count: u32,
}

/// Flat-topped plateau with smooth shoulders. Compact support, no `exp`.
fn plateau(d: f64, half_width: f64, shoulder: f64) -> f64 {
    let a = abs(d);
    if a <= half_width {
        1.0
    } else if a >= half_width + shoulder {
        0.0
    } else {
        1.0 - smoothstep((a - half_width) / shoulder)
    }
}

/// Apply the shape template in place. Heights are elevations, y up.
///
/// Every column is clamped to `[WORLD_FLOOR, WORLD_CEIL]` on the way out.
pub fn shape(heights: &mut [f64], template: Template, p: &ShapeParams) {
    let span = p.target_x - p.muzzle_x;
    let span = if abs(span) < 1.0 { 1.0 } else { span };
    for (i, h) in heights.iter_mut().enumerate() {
        let x = i as f64 * TERRAIN_STEP;
        let rel = *h - BASE_ELEVATION;
        // 0 at the emplacement, 1 at the target, held at the ends.
        let u = clamp((x - p.muzzle_x) / span, 0.0, 1.0);

        // Templates keep a fraction of the noise relief and add their own
        // structure. Damping the relief is what keeps the added shape
        // readable — but `Flat` keeps all of it, because "gentle roll" still
        // has to read as rolling ground, and that is the terrain the reference
        // scene in `art/` was rendered from.
        let (relief_keep, mut add) = match template {
            Template::Flat => (1.0, 0.0),
            Template::Ridge => {
                let xm = p.muzzle_x + span * 0.55;
                (0.75, 40.0 * dmath::bump(x - xm, 26.0))
            }
            Template::Chasm => {
                let xg = p.target_x - 26.0;
                (
                    0.7,
                    -44.0 * plateau(x - xg, 6.0, 16.0) + 13.0 * dmath::bump(x - p.target_x, 14.0),
                )
            }
            Template::Mesa => (0.6, 32.0 * plateau(x - p.target_x, 22.0, 9.0)),
            Template::Spires => {
                let mut a = 0.0;
                let fracs: [f64; 3] = [0.40, 0.62, 0.82];
                for frac in fracs.iter().take(p.spire_count.min(3) as usize) {
                    let c = p.muzzle_x + span * frac;
                    a += 56.0 * plateau(x - c, 2.0, 5.0);
                }
                (0.55, a)
            }
            Template::Overhang => {
                // A thin wall immediately in front of a narrow pocket. No
                // heightfield can hold a true overhang, so the pocket does the
                // same job: nothing on a direct arc gets in.
                let wall = p.target_x - 15.0;
                (
                    0.5,
                    66.0 * plateau(x - wall, 2.5, 3.0) - 20.0 * plateau(x - p.target_x, 9.0, 4.0),
                )
            }
            Template::Basin => {
                let rim = p.target_x - 46.0;
                (
                    0.4,
                    -36.0 * plateau(x - p.target_x, 26.0, 18.0) + 12.0 * dmath::bump(x - rim, 16.0),
                )
            }
        };

        // Elevation delta ramps in from the emplacement toward the target so
        // the firing position itself is never yanked around. DESIGN §2.5.
        add += p.elev_delta * smoothstep(u);

        *h = clamp(
            BASE_ELEVATION + rel * relief_keep + add,
            WORLD_FLOOR,
            WORLD_CEIL,
        );
    }
}

// --------------------------------------------------------------- heightfield

/// Three parallel buffers, one sample per pixel column. DESIGN §2.1.
///
/// Rendered surface is `base[i] - scar[i]`. Keeping them separate is what lets
/// a screen be rebuilt from its seed and replayed by re-applying the shot log
/// to a fresh scar buffer.
#[derive(Clone, Debug)]
pub struct Heightfield {
    /// Generated, immutable.
    pub base: Vec<f64>,
    /// Accumulated subtraction. Negative entries are debris mounds (DESIGN §2.7).
    pub scar: Vec<f64>,
    /// Layer id at the surface, recomputed from excavation depth (`scar`).
    pub material: Vec<Material>,
}

impl Heightfield {
    /// A fresh field over generated heights: no scars, all Sand.
    pub fn from_base(base: Vec<f64>) -> Self {
        let n = base.len();
        Self {
            base,
            scar: vec![0.0; n],
            material: vec![Material::Sand; n],
        }
    }

    /// Number of columns.
    #[inline]
    pub fn len(&self) -> usize {
        self.base.len()
    }

    /// True for a field with no columns.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.base.is_empty()
    }

    /// Surface elevation at a column index, clamped at the ends.
    ///
    /// # Panics
    ///
    /// If the field is empty.
    #[inline]
    pub fn surface(&self, i: usize) -> f64 {
        let i = i.min(self.len().saturating_sub(1));
        self.base[i] - self.scar[i]
    }

    /// Surface elevation at a continuous x, linearly interpolated.
    ///
    /// DESIGN §2.1: the sim needs sub-pixel precision for collision even
    /// though the display has none. Rounding here makes shallow trajectories
    /// collide inconsistently.
    ///
    /// Outside the field the end columns extend flat; an empty field reads 0.
    pub fn surface_at(&self, x: f64) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let last = self.len() - 1;
        if x <= 0.0 {
            return self.surface(0);
        }
        if x >= last as f64 {
            return self.surface(last);
        }
        let i = dmath::floor(x);
        let f = x - i;
        let i = i as usize;
        self.surface(i) * (1.0 - f) + self.surface(i + 1) * f
    }

    /// Original (pre-damage) surface elevation, clamped at the ends like
    /// [`Heightfield::surface`].
    ///
    /// # Panics
    ///
    /// If the field is empty.
    #[inline]
    pub fn original(&self, i: usize) -> f64 {
        self.base[i.min(self.len().saturating_sub(1))]
    }

    /// Surface layer at a column.
    ///
    /// Takes the nearest column rather than interpolating, clamped to the
    /// field. An empty field reads Sand.
    #[inline]
    pub fn material_at(&self, x: f64) -> Material {
        if self.is_empty() {
            return Material::Sand;
        }
        let i = clamp(dmath::round(x), 0.0, (self.len() - 1) as f64) as usize;
        self.material[i]
    }

    /// Layer at the point `(x, y)`, from its depth below the original
    /// surface of the nearest column (DESIGN §2.3 step 4). Unlike
    /// [`Heightfield::material_at`] this reads underground: a tunnelling
    /// shell meets bedrock at its own depth, whatever the surface is.
    ///
    /// Points above the original surface read Sand, like a debris mound. An
    /// empty field reads Sand.
    pub fn material_below(&self, x: f64, y: f64) -> Material {
        if self.is_empty() {
            return Material::Sand;
        }
        let i = clamp(dmath::round(x), 0.0, (self.len() - 1) as f64) as usize;
        Material::at_depth(self.base[i] - y)
    }

    fn recompute_material(&mut self, from: usize, to: usize) {
        // Excavation depth is the scar itself. Negative scar is a debris mound
        // piled on top, which is loose material and reads as Sand.
        for i in from..to.min(self.len()) {
            let depth = self.scar[i];
            self.material[i] = if depth <= 0.0 {
                Material::Sand
            } else {
                Material::at_depth(depth)
            };
        }
    }

    /// Smooth crater subtraction with squared falloff. DESIGN §2.6:
    /// `scar[i] += r * (1 - (d/r)^2)^2`.
    ///
    /// Clamped so the surface can never be cut below the world floor — past
    /// that the heightfield inverts and collision breaks.
    ///
    /// `cx` and `radius` are in world units. Craters accumulate: a second
    /// blast in the same place digs deeper. The profile is [`dmath::bump`]
    /// scaled by the radius, so depth at the centre equals the radius and the
    /// walls meet the ground with zero slope: a rounded bowl rather than a
    /// V-shaped notch.
    ///
    /// The clamp is at [`WORLD_FLOOR`], not at bedrock: DESIGN §2.6 says to
    /// "cap total scar depth at bedrock", but a column can be dug on into
    /// bedrock, down to the floor.
    pub fn crater(&mut self, cx: f64, radius: f64) {
        if radius <= 0.0 || self.is_empty() {
            return;
        }
        let lo = clamp(dmath::floor(cx - radius), 0.0, (self.len() - 1) as f64) as usize;
        let hi = clamp(dmath::floor(cx + radius) + 1.0, 0.0, self.len() as f64) as usize;
        for i in lo..hi {
            let d = (i as f64 * TERRAIN_STEP - cx) / radius;
            if abs(d) >= 1.0 {
                continue;
            }
            let t = 1.0 - d * d;
            self.scar[i] += radius * t * t;
            let max_scar = self.base[i] - WORLD_FLOOR;
            if self.scar[i] > max_scar {
                self.scar[i] = max_scar;
            }
        }
        self.recompute_material(lo, hi);
    }

    /// Debris mound — a small positive addition where a structure fell.
    /// DESIGN §2.7: "terrain gets slightly higher where a structure fell".
    ///
    /// A [`dmath::bump`] of `height` units at `cx`, recorded as negative scar
    /// and clamped at [`WORLD_CEIL`].
    pub fn mound(&mut self, cx: f64, radius: f64, height: f64) {
        if radius <= 0.0 || height <= 0.0 || self.is_empty() {
            return;
        }
        let lo = clamp(dmath::floor(cx - radius), 0.0, (self.len() - 1) as f64) as usize;
        let hi = clamp(dmath::floor(cx + radius) + 1.0, 0.0, self.len() as f64) as usize;
        for i in lo..hi {
            let d = i as f64 * TERRAIN_STEP - cx;
            self.scar[i] -= height * dmath::bump(d, radius);
            let min_scar = self.base[i] - WORLD_CEIL;
            if self.scar[i] < min_scar {
                self.scar[i] = min_scar;
            }
        }
        self.recompute_material(lo, hi);
    }

    /// Highest surface across a footprint — seats a structure so no column of
    /// it floats. Port of `gen_sprites.py::seat`.
    ///
    /// Columns past the end are ignored; if none remain, falls back to the
    /// clamped surface at `x0`.
    ///
    /// # Panics
    ///
    /// If the field is empty.
    pub fn seat(&self, x0: usize, footprint: usize) -> f64 {
        let mut m = f64::MIN;
        for i in x0..(x0 + footprint).min(self.len()) {
            let s = self.surface(i);
            if s > m {
                m = s;
            }
        }
        if m == f64::MIN {
            self.surface(x0)
        } else {
            m
        }
    }

    /// Mean surface across a footprint, normalised to the original heights.
    /// This is the support fraction of DESIGN §2.7.
    ///
    /// `original_mean` is the mean surface across the footprint when the
    /// structure was placed. The result is clamped to `[0, 1]`, so debris
    /// piled higher than the original ground never counts as extra support.
    /// Degenerate inputs (an empty footprint, a non-positive mean, a
    /// footprint entirely past the end) read as fully supported.
    pub fn support_fraction(&self, x0: usize, footprint: usize, original_mean: f64) -> f64 {
        if footprint == 0 || original_mean <= 0.0 {
            return 1.0;
        }
        let mut sum = 0.0;
        let mut n = 0.0;
        for i in x0..(x0 + footprint).min(self.len()) {
            sum += self.surface(i);
            n += 1.0;
        }
        if n == 0.0 {
            return 1.0;
        }
        clamp((sum / n) / original_mean, 0.0, 1.0)
    }
}

/// Full generation of the terrain for one screen: base ridge, domain warp,
/// shape template, material layering. Target placement and solvability live in
/// `run.rs` / `solver.rs` because they need the loadout.
pub fn generate(screen_seed: u64, template: Template, p: &ShapeParams) -> Heightfield {
    let mut base = base_ridge(p.width, screen_seed, p.amp);
    shape(&mut base, template, p);
    Heightfield::from_base(base)
}

/// Sanity bound used by the tests: nothing may sit outside the internal
/// buffer, `0..=SCREEN_H`.
pub fn in_world(elev: f64) -> bool {
    elev >= 0.0 && elev <= SCREEN_H as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(width: usize) -> ShapeParams {
        ShapeParams {
            width,
            amp: 1.0,
            muzzle_x: 30.0,
            target_x: 200.0,
            elev_delta: 0.0,
            spire_count: 2,
        }
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        for i in 0..500 {
            let x = i as f64 * 0.37;
            let a = fbm(x, 12345);
            let b = fbm(x, 12345);
            assert_eq!(a.to_bits(), b.to_bits(), "fbm not bit-stable at {x}");
            assert!((0.0..=1.0).contains(&a), "fbm out of range: {a}");
        }
    }

    #[test]
    fn vnoise_interpolates_between_lattice_values() {
        // At integers the value equals the lattice hash exactly.
        let at_0 = vnoise(0.0, 7);
        let at_1 = vnoise(1.0, 7);
        let mid = vnoise(0.5, 7);
        let lo = at_0.min(at_1);
        let hi = at_0.max(at_1);
        assert!(mid >= lo && mid <= hi, "{mid} outside [{lo},{hi}]");
    }

    #[test]
    fn base_ridge_stays_inside_the_buffer() {
        for seed in 0..64u64 {
            let hs = base_ridge(400, seed * 7919, 2.2);
            for h in hs {
                assert!(in_world(h), "elevation {h} off-buffer");
            }
        }
    }

    #[test]
    fn every_template_stays_within_world_bounds() {
        for t in Template::ALL {
            for seed in 0..24u64 {
                let f = generate(seed * 131, t, &params(320));
                for i in 0..f.len() {
                    let s = f.surface(i);
                    assert!(
                        (WORLD_FLOOR..=WORLD_CEIL).contains(&s),
                        "{} col {i} = {s}",
                        t.name()
                    );
                }
            }
        }
    }

    #[test]
    fn templates_are_distinguishable() {
        // Each template must actually change the shape, or selection is a lie.
        let flat = generate(42, Template::Flat, &params(320));
        for t in Template::ALL {
            if t == Template::Flat {
                continue;
            }
            let other = generate(42, t, &params(320));
            let diff: f64 = (0..320)
                .map(|i| abs(other.surface(i) - flat.surface(i)))
                .sum();
            assert!(diff > 100.0, "{} barely differs from Flat", t.name());
        }
    }

    #[test]
    fn crater_has_a_rim_and_a_deepest_point() {
        let mut f = Heightfield::from_base(vec![80.0; 64]);
        let before = f.surface(32);
        f.crater(32.0, 8.0);
        // Deepest at the centre, shallower toward the rim, untouched outside.
        let d_centre = before - f.surface(32);
        let d_mid = before - f.surface(36);
        let d_rim = before - f.surface(40);
        assert!(d_centre > d_mid, "{d_centre} !> {d_mid}");
        assert!(d_mid > d_rim, "{d_mid} !> {d_rim}");
        assert_eq!(f.surface(41), before);
        // Squared falloff, not linear: the profile is convex near the rim.
        assert!(d_rim.abs() < 1e-12);
    }

    #[test]
    fn craters_clamp_at_the_world_floor() {
        let mut f = Heightfield::from_base(vec![40.0; 32]);
        for _ in 0..80 {
            f.crater(16.0, 20.0);
        }
        for i in 0..f.len() {
            assert!(
                f.surface(i) >= WORLD_FLOOR - 1e-9,
                "col {i} = {}",
                f.surface(i)
            );
        }
    }

    #[test]
    fn deep_craters_expose_deeper_material() {
        let mut f = Heightfield::from_base(vec![100.0; 32]);
        assert_eq!(f.material_at(16.0), Material::Sand);
        f.crater(16.0, 12.0);
        assert_eq!(f.material_at(16.0), Material::Rock);
        f.crater(16.0, 20.0);
        assert_eq!(f.material_at(16.0), Material::Bedrock);
    }

    #[test]
    fn mound_raises_terrain_and_clamps_at_the_ceiling() {
        let mut f = Heightfield::from_base(vec![60.0; 32]);
        f.mound(16.0, 6.0, 4.0);
        assert!(f.surface(16) > 60.0);
        assert_eq!(f.surface(31), 60.0);
        for _ in 0..200 {
            f.mound(16.0, 6.0, 10.0);
        }
        assert!(f.surface(16) <= WORLD_CEIL + 1e-9);
    }

    #[test]
    fn surface_at_interpolates() {
        let mut f = Heightfield::from_base(vec![0.0; 4]);
        f.base = vec![10.0, 20.0, 20.0, 20.0];
        assert!(abs(f.surface_at(0.5) - 15.0) < 1e-12);
        assert!(abs(f.surface_at(0.0) - 10.0) < 1e-12);
        assert!(abs(f.surface_at(-5.0) - 10.0) < 1e-12);
        assert!(abs(f.surface_at(99.0) - 20.0) < 1e-12);
    }
}

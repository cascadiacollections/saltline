//! Layouts: a screen written down as plain integer data, so a person can
//! make one, keep it and share it. DESIGN §13.
//!
//! A generated screen is a function of a seed (§2.2): nothing about it is
//! stored, because the generator can always make it again. A **layout** is
//! the opposite: the ground, the targets, the guns and the wind are the
//! data, and nothing has to be generated. It is what the map editor edits
//! and what a custom game plays.
//!
//! # The data model
//!
//! | Field | Type | Unit |
//! |---|---|---|
//! | `heights` | `Vec<u16>`, one per column, so its length is the width | sixteenths of a world unit ([`HEIGHT_SCALE`]) |
//! | `structures` | kind and leftmost column ([`Placed`]) | columns |
//! | `guns` | one or two barrel pivots | columns |
//! | `wind` | [`WindSpec`]: calm, fixed, or a range of strengths | thousandths of a unit/s² ([`WIND_SCALE`]) |
//! | `name` | [`LayoutName`], the seed keypad's A–Z and 0–9 | |
//!
//! **Why integers.** The sim keeps `f64` everywhere (§3), but a layout is
//! not a sim state: it is something a person edits, a share code carries
//! and a file keeps. Integers make all three exact. An edit is a whole
//! number of steps, so undo and mirroring are exact; the share code can
//! delta-code columns without rounding (`code`); and two devices that read
//! the same layout build bit-identical `f64` heights from it, since
//! `h / 16` is exact in binary. Every height on the grid is a dyadic
//! rational, so nothing about it depends on the platform.
//!
//! **Why sixteenths.** A height is used four ways: drawn (whole pixels),
//! collided with (interpolated between columns, so sub-pixel matters a
//! little, §2.1), and read for seating and support (the highest and the
//! mean of a footprint, §2.7). Rounding a generated height to the nearest
//! sixteenth moves it by at most 1/32 of a pixel. That is under the
//! smallest thing any of those uses notice: a structure seats at most 1/32
//! higher or lower, support moves by well under a thousandth of a
//! threshold, and a shell's path is sampled every 1/120 s, steps of up to
//! 2.5 units, so a 1/32 shift in the ground changes which step a shell
//! lands on only for a grazing shot. NOTES §9 measures it. A coarser grid
//! would make share codes a little shorter (about a bit a column per
//! halving) and terraced slopes more likely; a finer one costs a bit a
//! column per doubling for nothing anyone can see.
//!
//! **Why not the exact `f64`s.** A generated height is an arbitrary
//! double. Keeping every bit (a fixed point with 49 fraction bits covers the
//! world's range exactly) would make a fork's state hash equal the
//! generated screen's, but costs about six bytes a column: a share code
//! of some 3,080 characters, against about 200 here. DESIGN §13.14 weighs
//! it.
//!
//! # From layout to screen
//!
//! [`Screen::from_layout`] builds the same kind of [`Screen`] the generator
//! does, with the generated base swapped for the layout's: the heights
//! become the base buffer, the structures are seated by the generator's own
//! rule (on the highest column of the footprint, `run::seat_at`), the first
//! gun is the emplacement, and the wind is set. Scars, materials, craters,
//! collapse, the solver and versus's mirrored frame all work on it
//! unchanged, because none of them can tell where a base came from.
//!
//! # Forking a seed
//!
//! [`Layout::from_run_screen`] generates a run's screen at the layout width
//! and writes it down: base rounded to sixteenths, structures and gun as
//! placed, wind rounded to thousandths. The duel equivalent lives with the
//! duel generator, in the Godot crate (`custom::fork_duel`), since the sim
//! knows nothing of versus (§11.2). The generator is the first draft; the
//! editor starts from it.
//!
//! # Checking a layout
//!
//! [`check_placement`] applies the rules a layout must meet before anything
//! is played on it (§13.5): the width, the ground inside the world, every
//! structure seated rather than floating or sunk, every gun seated and
//! clear. [`validate`] adds the solvability gate for a one-gun layout,
//! at each extreme of its wind; the Godot crate adds the duel gate for two.
//!
//! # Share codes
//!
//! [`Layout::to_code`] and [`Layout::from_code`], in the `code` module: a
//! versioned, checksummed, compact text form. Decoding is total: any text
//! at all is either a layout or an error, never a panic.

mod code;

pub use code::{CodeError, CODE_PREFIX, LAYOUT_FORMAT, MAX_CODE_CHARS};

use crate::dmath;
use crate::modifiers::Loadout;
use crate::replay::Hasher;
use crate::rng::{substream, Pcg32};
use crate::run::{self, generate_screen, Screen, StructureKind, CHASSIS_WIDTH};
use crate::solver;
use crate::terrain::{Heightfield, Template, WORLD_CEIL, WORLD_FLOOR};
use crate::{BASE_BLAST_RADIUS, MIN_TARGET_SEPARATION, SCREEN_W_REF};

/// Every layout is this wide (owner decision, October 2026): one width, so
/// a layout plays the same on every phone. Wider buffers show it
/// letterboxed. It is the reference width the design tables are tuned to
/// (§2.5).
pub const LAYOUT_WIDTH: usize = SCREEN_W_REF;

/// Height steps per world unit: heights are sixteenths.
pub const HEIGHT_SCALE: u16 = 16;

/// Wind steps per unit/s²: winds are thousandths.
pub const WIND_SCALE: i32 = 1000;

/// The strongest wind a layout may set, in thousandths: the run's cap
/// (§2.5, 7.0).
pub const MAX_WIND: i32 = 7000;

/// Longest name, in characters.
pub const NAME_MAX: usize = 12;

/// Most targets a one-gun layout may hold: the generator's most (§2.5).
pub const MAX_TARGETS: usize = 3;

/// How far a structure's seat may stand above the ground under the middle
/// of its footprint, in world units, before it counts as floating. A
/// structure is seated on the highest column under it (§2.7), so on a
/// slope or an edge part of it is always in the air. This is measured at
/// the median column, so it reads "at least half its footprint is within
/// this of the ground": a silo on a gentle slope passes, one balanced on a
/// spire's tip does not. Generated structures pass nine times in ten with
/// room to spare; the rest stand on spires and chasm lips (NOTES §9).
pub const FLOAT_LIMIT: f64 = 4.0;

/// Columns either side of a structure that are looked at for burial: a
/// structure with ground higher than its top on both sides within this
/// many columns is sunk in a slot, not standing on the ground. The
/// generator never does that.
pub const BURY_MARGIN: usize = 3;

/// As [`FLOAT_LIMIT`], for a gun's 26-column chassis ([`CHASSIS_WIDTH`]):
/// how far its seat may stand above the ground under the middle of it.
/// Above the most any generated run or duel screen stands its gun (NOTES
/// §9), so a fork's guns always pass.
pub const GUN_FLOAT_LIMIT: f64 = 12.0;

/// The closest a gun's pivot may stand to the edge of the screen: half the
/// chassis plus a column, so the whole gun is on screen. Versus uses the
/// same margin for its rolling guns.
pub const GUN_EDGE_MARGIN: u16 = 14;

/// Clear columns between a gun's chassis and the nearest structure.
pub const GUN_CLEARANCE: usize = 8;

/// The least distance between a duel's two pivots, in columns: the
/// 26-column chassis ([`CHASSIS_WIDTH`]) plus twice the base blast radius
/// (`BASE_BLAST_RADIUS`, 9), the reach a blast has to hit a gun (§11.1).
/// With the boxes at least 18 apart, no one detonation reaches both guns,
/// so a shot is never a hit and a self-hit at once, and neither chassis
/// overlaps the other.
pub const MIN_GUN_GAP: u16 = CHASSIS_WIDTH as u16 + 2 * BASE_BLAST_RADIUS as u16;

// ------------------------------------------------------------------ name

/// A layout's name: up to [`NAME_MAX`] characters from the seed keypad
/// (`A`–`Z`, `0`–`9`), possibly none.
///
/// Only text the keypad can type is a name, so nothing a player shares can
/// carry words beyond what a seed already can (DESIGN §7, §13.10): no
/// spaces, punctuation, lower case or other scripts. Built only through
/// [`LayoutName::parse`], so every value of the type is valid.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LayoutName(String);

impl LayoutName {
    /// The keypad's characters, in the order the share code numbers them.
    pub const CHARSET: &'static [u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    /// `text` as a name, if it is one: at most [`NAME_MAX`] keypad
    /// characters. Lower-case letters are taken as upper case, as seed
    /// entry takes them.
    pub fn parse(text: &str) -> Option<LayoutName> {
        let up = text.to_ascii_uppercase();
        let ok = up.len() <= NAME_MAX && up.bytes().all(|b| Self::CHARSET.contains(&b));
        ok.then_some(LayoutName(up))
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ------------------------------------------------------------------ wind

/// The wind a layout plays in. Strengths are thousandths of a unit/s²
/// ([`WIND_SCALE`]), positive blowing right, in the layout's own frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindSpec {
    /// No wind.
    Calm,
    /// The same wind on every shot.
    Fixed(i32),
    /// A fresh wind each time, either way with even odds, its strength
    /// uniform from `min` to `max`: a duel's per-turn wind (§11.1) is
    /// `Range { min: 0, max: 2500 }`.
    Range {
        /// Weakest strength, `0..=max`.
        min: i32,
        /// Strongest strength, `min..=MAX_WIND`.
        max: i32,
    },
}

impl WindSpec {
    /// The winds the solvability gate checks, in thousandths, each once:
    /// the fixed wind itself, or both extremes of a range either way (the
    /// weakest included, which is calm when `min` is 0).
    pub fn extremes(&self) -> Vec<i32> {
        let mut out: Vec<i32> = match *self {
            WindSpec::Calm => vec![0],
            WindSpec::Fixed(w) => vec![w],
            WindSpec::Range { min, max } => vec![-max, -min, min, max],
        };
        out.dedup();
        out
    }

    /// One wind for one screen, in units/s²: fixed or calm as set, or a
    /// draw from `seed` for a range. A range's draw is the same rule as a
    /// duel turn's (`versus::turn_wind`): one uniform `u` in `[-1, 1)` gives
    /// the direction by its sign and the strength by its size.
    pub fn draw(&self, seed: u64) -> f64 {
        match *self {
            WindSpec::Calm => 0.0,
            WindSpec::Fixed(w) => wind_units(w),
            WindSpec::Range { min, max } => {
                // ASCII "LWND": a stream of its own, like every draw (§2.2).
                let u = Pcg32::new(substream(seed, 0x4C57_4E44)).signed(1.0);
                let (lo, hi) = (wind_units(min), wind_units(max));
                let strength = lo + (hi - lo) * dmath::abs(u);
                if u < 0.0 {
                    -strength
                } else {
                    strength
                }
            }
        }
    }
}

/// Thousandths of a unit/s² as units/s².
pub fn wind_units(milli: i32) -> f64 {
    milli as f64 / WIND_SCALE as f64
}

/// A wind in units/s² to the nearest thousandth.
pub fn wind_milli(units: f64) -> i32 {
    dmath::round(units * WIND_SCALE as f64) as i32
}

/// A height in sixteenths as world units. Exact: every `u16` over 16 is a
/// double.
pub fn height_units(h: u16) -> f64 {
    h as f64 / HEIGHT_SCALE as f64
}

/// A height in world units to the nearest sixteenth, clamped to what a
/// `u16` holds.
pub fn height_steps(units: f64) -> u16 {
    dmath::clamp(
        dmath::round(units * HEIGHT_SCALE as f64),
        0.0,
        u16::MAX as f64,
    ) as u16
}

// ---------------------------------------------------------------- layout

/// A structure as a layout places it: what it is and its leftmost column.
/// Its height is not stored; it is seated on the ground under it, as the
/// generator seats one, so it can never be buried or left hanging by an
/// edit to the ground beneath.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placed {
    /// Which structure (§5.4). Its footprint is `kind.footprint()` columns.
    pub kind: StructureKind,
    /// Leftmost footprint column.
    pub x0: u16,
}

/// A screen as data. See the module docs for the units and DESIGN §13.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Shown in lists. Keypad characters only.
    pub name: LayoutName,
    /// Ground height per column, in sixteenths. Its length is the width.
    pub heights: Vec<u16>,
    /// Targets, left to right as placed. A one-gun layout's targets; empty
    /// for a duel (Phase 1).
    pub structures: Vec<Placed>,
    /// Barrel pivot columns: one gun for a target layout, two for a duel,
    /// left first. A duel's two stand mirrored (§13.5).
    pub guns: Vec<u16>,
    /// The wind.
    pub wind: WindSpec,
}

impl Layout {
    /// Columns: the length of `heights`.
    pub fn width(&self) -> usize {
        self.heights.len()
    }

    /// Two guns: a duel layout.
    pub fn is_duel(&self) -> bool {
        self.guns.len() == 2
    }

    /// Write down a screen as it was generated: its base rounded to
    /// sixteenths, its structures where they stand, `guns`, and `wind`.
    /// Craters, damage and the screen's own wind are not read.
    pub fn from_screen(screen: &Screen, guns: Vec<u16>, wind: WindSpec) -> Layout {
        Layout {
            name: LayoutName::default(),
            heights: screen.field.base.iter().map(|&b| height_steps(b)).collect(),
            structures: screen
                .structures
                .iter()
                .map(|s| Placed {
                    kind: s.kind,
                    x0: s.x0 as u16,
                })
                .collect(),
            guns,
            wind,
        }
    }

    /// Fork screen `index` of the run seeded `run_seed`: generate it at
    /// [`LAYOUT_WIDTH`] for the base loadout and write it down, with its
    /// wind as a fixed wind (§13.6).
    ///
    /// The base loadout because a layout carries none (§13.1): a later
    /// screen of a real run may have been drawn for a loadout that reached
    /// further, and this is the screen the same seed shows a fresh run. The
    /// layout width because a layout has only one; on a wider phone the
    /// run's screen may differ toward its right edge.
    pub fn from_run_screen(run_seed: u64, index: u32) -> Layout {
        let screen = generate_screen(run_seed, index, LAYOUT_WIDTH, &Loadout::new());
        let gun = dmath::round(screen.muzzle_x) as u16;
        Layout::from_screen(&screen, vec![gun], WindSpec::Fixed(wind_milli(screen.wind)))
    }

    /// A digest of everything in the layout, name included. Stands in for
    /// a generated screen's seed where one is wanted (the renderer's
    /// ground speckle, `Screen::seed`), so the same layout always looks the
    /// same.
    pub fn digest(&self) -> u64 {
        let mut h = Hasher::new();
        for b in self.name.as_str().bytes() {
            h.write_u8(b);
        }
        h.write_usize(self.heights.len());
        for &v in &self.heights {
            h.write_u32(v as u32);
        }
        h.write_usize(self.structures.len());
        for s in &self.structures {
            h.write_u8(s.kind as u8);
            h.write_u32(s.x0 as u32);
        }
        h.write_usize(self.guns.len());
        for &g in &self.guns {
            h.write_u32(g as u32);
        }
        match self.wind {
            WindSpec::Calm => h.write_u8(0),
            WindSpec::Fixed(w) => {
                h.write_u8(1);
                h.write_i32(w);
            }
            WindSpec::Range { min, max } => {
                h.write_u8(2);
                h.write_i32(min);
                h.write_i32(max);
            }
        }
        h.finish()
    }

    /// The largest difference between a column and its mirror image, in
    /// world units: zero for a perfectly symmetric ground. A fairness
    /// reading for duels; generated duels keep a tenth of their original
    /// asymmetry (§11.3).
    pub fn asymmetry(&self) -> f64 {
        let n = self.heights.len();
        (0..n / 2)
            .map(|i| self.heights[i].abs_diff(self.heights[n - 1 - i]))
            .max()
            .map_or(0.0, height_units)
    }

    /// Copy the left half of the ground onto the right, mirrored, so both
    /// sides are the same (the editor's MIRROR). Exact: whole steps are
    /// copied, not averaged. Structures and guns are left alone.
    pub fn mirror_left(&mut self) {
        let n = self.heights.len();
        for i in 0..n / 2 {
            self.heights[n - 1 - i] = self.heights[i];
        }
    }
}

// ---------------------------------------------------------------- screen

impl Screen {
    /// The screen a layout plays as, in wind `wind` (units/s², the
    /// layout's frame): see the module docs. `wind` is usually
    /// `layout.wind.draw(seed)`.
    ///
    /// Nothing is validated here: a layout that fails [`check_placement`]
    /// still builds a screen, but only a checked one should be played. Its
    /// `index` is 0 and its `seed` the layout's digest; `template` reads
    /// `Flat` and `rung` is `None`, since nothing was generated. The first
    /// gun is the emplacement (`muzzle_x`); in a duel, versus places the
    /// second as its mirror.
    ///
    /// # Panics
    ///
    /// If the layout has no columns or no gun.
    pub fn from_layout(layout: &Layout, wind: f64) -> Screen {
        let field =
            Heightfield::from_base(layout.heights.iter().map(|&h| height_units(h)).collect());
        let structures = layout
            .structures
            .iter()
            .map(|p| run::seat_at(&field, p.kind, p.x0 as usize))
            .collect();
        Screen {
            index: 0,
            seed: layout.digest(),
            width: layout.width(),
            template: Template::Flat,
            field,
            structures,
            wind,
            wind_late: None,
            muzzle_x: layout.guns[0] as f64,
            candidates: 0,
            attempts: 0,
            fell_back: false,
            rung: None,
            supply: false,
        }
    }
}

// ------------------------------------------------------------ validation

/// One thing wrong with a layout. Indices are into `structures` and `guns`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    /// Not [`LAYOUT_WIDTH`] columns.
    Width(usize),
    /// A column outside `WORLD_FLOOR..=WORLD_CEIL`.
    HeightOutOfWorld {
        /// The first such column.
        column: usize,
    },
    /// Not one gun or two.
    GunCount(usize),
    /// A gun's chassis would hang off the screen.
    GunOffScreen(usize),
    /// More than half a gun's chassis is over ground further than
    /// [`GUN_FLOAT_LIMIT`] below its seat.
    GunNotSeated(usize),
    /// A duel's guns are not mirror images: versus places the right gun as
    /// the mirror of the left (§11.2).
    GunsNotMirrored,
    /// A duel's first gun is not the left one.
    GunsOutOfOrder,
    /// A duel's pivots are closer than [`MIN_GUN_GAP`].
    GunsTooClose,
    /// A one-gun layout with no targets.
    NoTargets,
    /// More than [`MAX_TARGETS`] targets.
    TooManyTargets(usize),
    /// A duel layout with structures in it. Phase 1 plays duels on bare
    /// ground; structures as cover are an open question (§13.13).
    StructuresInDuel,
    /// A structure's footprint runs off the screen or onto its edge column.
    StructureOffScreen(usize),
    /// More than half a structure's footprint is over ground further than
    /// [`FLOAT_LIMIT`] below its seat.
    StructureFloating(usize),
    /// Ground on both sides of a structure, within [`BURY_MARGIN`]
    /// columns, stands higher than its top.
    StructureBuried(usize),
    /// A structure overlaps a gun's chassis or its clearance.
    StructureOnGun(usize),
    /// A structure stands behind (left of) the gun, which only fires right.
    StructureBehindGun(usize),
    /// Two structures' centres closer than `MIN_TARGET_SEPARATION`.
    StructuresTooClose(usize, usize),
    /// A wind stronger than [`MAX_WIND`], or a range with `min > max` or a
    /// negative end.
    Wind,
    /// Fewer than `MIN_SOLUTIONS` distinct shots from `gun` hit what it
    /// must (every target, or the other gun) in wind `wind` (thousandths).
    Unsolvable {
        /// The gun that cannot: 0 left, 1 right.
        gun: usize,
        /// The wind it was checked in, in the layout's frame.
        wind: i32,
        /// Distinct shots found for the hardest target, up to the gate's
        /// cap.
        shots: usize,
    },
}

/// Every placement rule a layout breaks (DESIGN §13.5), in a fixed order;
/// empty if none. Cheap: no shot is flown.
pub fn check_placement(layout: &Layout) -> Vec<Problem> {
    let mut out = Vec::new();
    let w = layout.width();
    if w != LAYOUT_WIDTH {
        out.push(Problem::Width(w));
    }
    if let Some(column) = layout
        .heights
        .iter()
        .position(|&h| !(WORLD_FLOOR..=WORLD_CEIL).contains(&height_units(h)))
    {
        out.push(Problem::HeightOutOfWorld { column });
    }
    match layout.wind {
        WindSpec::Calm => {}
        WindSpec::Fixed(v) => {
            if !(-MAX_WIND..=MAX_WIND).contains(&v) {
                out.push(Problem::Wind);
            }
        }
        WindSpec::Range { min, max } => {
            if min < 0 || min > max || max > MAX_WIND {
                out.push(Problem::Wind);
            }
        }
    }
    let n_guns = layout.guns.len();
    if !(1..=2).contains(&n_guns) {
        out.push(Problem::GunCount(n_guns));
        return out;
    }
    if w == 0 {
        return out;
    }
    let field = Heightfield::from_base(layout.heights.iter().map(|&h| height_units(h)).collect());
    let half = CHASSIS_WIDTH as u16 / 2;
    for (i, &g) in layout.guns.iter().enumerate() {
        if g < GUN_EDGE_MARGIN || g as usize + GUN_EDGE_MARGIN as usize >= w {
            out.push(Problem::GunOffScreen(i));
            continue;
        }
        // The right gun is seated in the mirrored frame (§11.2), where its
        // chassis starts one column further right than the left gun's rule
        // would put it: columns `g - 12 ..= g + 13`, the mirror of the left
        // gun's `g - 13 ..= g + 12`.
        let x0 = (g - half) as usize + i;
        if hang(&field, x0, CHASSIS_WIDTH) > GUN_FLOAT_LIMIT {
            out.push(Problem::GunNotSeated(i));
        }
    }
    if n_guns == 2 {
        let (left, right) = (layout.guns[0], layout.guns[1]);
        if left as usize + right as usize != w - 1 {
            out.push(Problem::GunsNotMirrored);
        }
        if left >= right {
            out.push(Problem::GunsOutOfOrder);
        } else if right - left < MIN_GUN_GAP {
            out.push(Problem::GunsTooClose);
        }
    }
    if n_guns == 2 && !layout.structures.is_empty() {
        out.push(Problem::StructuresInDuel);
    }
    if n_guns == 1 && layout.structures.is_empty() {
        out.push(Problem::NoTargets);
    }
    if layout.structures.len() > MAX_TARGETS {
        out.push(Problem::TooManyTargets(layout.structures.len()));
    }
    let gun = layout.guns[0] as usize;
    for (i, p) in layout.structures.iter().enumerate() {
        let fp = p.kind.footprint();
        let x0 = p.x0 as usize;
        // The generator keeps a footprint off the edge columns (`seat_at`'s
        // callers clamp to `1..=width - fp - 1`).
        if x0 < 1 || x0 + fp + 1 > w {
            out.push(Problem::StructureOffScreen(i));
            continue;
        }
        if hang(&field, x0, fp) > FLOAT_LIMIT {
            out.push(Problem::StructureFloating(i));
        }
        let top = field.seat(x0, fp) + p.kind.height();
        let above = |cols: std::ops::Range<usize>| cols.into_iter().any(|c| field.surface(c) > top);
        if above(x0.saturating_sub(BURY_MARGIN)..x0)
            && above(x0 + fp..(x0 + fp + BURY_MARGIN).min(w))
        {
            out.push(Problem::StructureBuried(i));
        }
        if n_guns == 1 {
            let chassis_right = gun + CHASSIS_WIDTH / 2;
            if x0 + fp <= gun {
                out.push(Problem::StructureBehindGun(i));
            } else if x0 < chassis_right + GUN_CLEARANCE {
                out.push(Problem::StructureOnGun(i));
            }
        }
    }
    for a in 0..layout.structures.len() {
        for b in a + 1..layout.structures.len() {
            let centre = |p: &Placed| p.x0 as f64 + p.kind.footprint() as f64 * 0.5;
            let d = dmath::abs(centre(&layout.structures[a]) - centre(&layout.structures[b]));
            if d < MIN_TARGET_SEPARATION {
                out.push(Problem::StructuresTooClose(a, b));
            }
        }
    }
    out
}

/// How far a footprint of `n` columns from `x0`, seated on its highest
/// column, stands above the ground under the middle of it (its median
/// column), in world units.
fn hang(field: &Heightfield, x0: usize, n: usize) -> f64 {
    let seat = field.seat(x0, n);
    let mut gaps: Vec<f64> = (x0..(x0 + n).min(field.len()))
        .map(|c| seat - field.surface(c))
        .collect();
    gaps.sort_by(f64::total_cmp);
    gaps.get(gaps.len() / 2).copied().unwrap_or(0.0)
}

/// The solvability gate for a one-gun layout (DESIGN §2.4, §13.5): in each
/// of its wind's extremes ([`WindSpec::extremes`]), every target must have
/// `MIN_SOLUTIONS` distinct shots, as a generated screen must, for the base
/// loadout. Empty for a duel layout, whose gate is versus's
/// (`custom::validate` in the Godot crate), and for a layout that breaks a
/// placement rule, which is not worth flying shots over.
pub fn check_targets(layout: &Layout) -> Vec<Problem> {
    if layout.guns.len() != 1 || !check_placement(layout).is_empty() {
        return Vec::new();
    }
    let lo = Loadout::new();
    let mut out = Vec::new();
    for wind in layout.wind.extremes() {
        let screen = Screen::from_layout(layout, wind_units(wind));
        let report = solver::validate(&screen, &lo);
        if !report.solvable() {
            out.push(Problem::Unsolvable {
                gun: 0,
                wind,
                shots: report.worst,
            });
        }
    }
    out
}

/// Placement, then for a one-gun layout the solvability gate: `Ok` if the
/// layout may be played. A duel layout needs versus's gate on top
/// (`custom::validate` in the Godot crate).
pub fn validate(layout: &Layout) -> Result<(), Vec<Problem>> {
    let mut problems = check_placement(layout);
    if problems.is_empty() {
        problems = check_targets(layout);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests;

//! Modifier pool, drafting, and loadout deltas. DESIGN §4.
//!
//! Modifiers are data, not branches: every entry in `POOL` is a row of effect
//! deltas, and `Loadout::recompute` folds the held stacks into the scalar
//! fields that ballistics, the solver and the economy read. A modifier with a
//! scalar effect (wind, power, crater or blast scale, angle quantum, refunds)
//! needs only a row. One whose effect is a behaviour (Airburst, Auger,
//! Thermal Lance, Storm Front and the like) sets a flag, and the code that
//! owns that behaviour checks it.
//!
//! Determinism: the fold runs in `POOL` order, not draft order, so the same
//! held stacks always give bit-identical floats however they were drafted.
//! Draft offers come from a seeded shuffle ([`draft_offers`]).

use crate::rng::{substream, Pcg32};
use crate::ANGLE_QUANTUM_TICKS;

/// Build the modifier supports. DESIGN §4. Display only; nothing in the sim
/// branches on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Drift,
    Demo,
    Precision,
    Economy,
    Risk,
}

impl Tag {
    /// Upper-case label, as printed on draft cards.
    pub const fn name(self) -> &'static str {
        match self {
            Tag::Drift => "DRIFT",
            Tag::Demo => "DEMO",
            Tag::Precision => "PRECISION",
            Tag::Economy => "ECONOMY",
            Tag::Risk => "RISK",
        }
    }
}

/// Which draft column a modifier belongs to. Presentation only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Shell,
    Instrument,
    Economy,
    Structural,
}

/// Every modifier in DESIGN §4.
///
/// The discriminant is the index into [`POOL`] and into the loadout's stack
/// array. The replay state hash writes stack counts in this order and draft
/// offers by index, so reordering the variants changes every state hash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Modifier {
    // Shell
    Ballast = 0,
    Sabot,
    Airburst,
    Skipstone,
    Auger,
    Cluster,
    ThermalLance,
    Featherweight,
    // Instrument
    Vernier,
    Anemometer,
    Rangefinder,
    ChalkMarks,
    DeadReckoning,
    PlumbLine,
    // Economy
    Salvage,
    Quartermaster,
    Depot,
    StandingOrders,
    ScavengerRites,
    ForcedMarch,
    // Structural
    Enfilade,
    SappersEye,
    LongLens,
    StormFront,
}

/// Number of [`Modifier`] variants, and the length of [`POOL`].
pub const MODIFIER_COUNT: usize = 24;

/// Capability flags, for modifiers whose effect is a behaviour rather than a
/// scalar.
///
/// Flags are derived from the held stacks by [`Loadout::recompute`] and are
/// neither stored nor hashed (the state hash records stack counts), so the
/// bit values are internal and need only be distinct.
pub mod flags {
    /// Airburst: detonate at a fuse distance.
    pub const AIRBURST: u32 = 1 << 0;
    /// Auger: tunnel through one layer.
    pub const AUGER: u32 = 1 << 1;
    /// Thermal Lance: Steel craters as Rock, stronger structural damage.
    pub const THERMAL_LANCE: u32 = 1 << 2;
    /// Anemometer: wind shown to one decimal.
    pub const ANEMOMETER: u32 = 1 << 3;
    /// Rangefinder: exact distance to the nearest target shown.
    pub const RANGEFINDER: u32 = 1 << 4;
    /// Dead Reckoning: wind hidden until the first shot.
    pub const DEAD_RECKONING: u32 = 1 << 5;
    /// Plumb Line: guide at the last trajectory's apex.
    pub const PLUMB_LINE: u32 = 1 << 6;
    /// Standing Orders: one-shot streak bonus.
    pub const STANDING_ORDERS: u32 = 1 << 7;
    /// Scavenger Rites: every fifth screen is a supply screen.
    pub const SCAVENGER_RITES: u32 = 1 << 8;
    /// Forced March: doubled refunds, capped reserve.
    pub const FORCED_MARCH: u32 = 1 << 9;
    /// Sapper's Eye: material layers drawn as colour bands.
    pub const SAPPERS_EYE: u32 = 1 << 10;
    /// Long Lens: camera zoomed out one step.
    pub const LONG_LENS: u32 = 1 << 11;
    /// Storm Front: wind resamples at the apex.
    pub const STORM_FRONT: u32 = 1 << 12;
}

/// One row of effect deltas. Multiplicative fields apply once per stack;
/// additive fields are multiplied by the stack count.
///
/// The exceptions: `angle_quantum` divides once per stack, `reserve_cap`
/// takes the minimum, `salvage_radius` the maximum, `flags` are ORed in, and
/// `instant_shells` is not folded at all; [`Loadout::take`] returns it.
/// [`Effect::NONE`] is the neutral row every entry starts from.
#[derive(Clone, Copy, Debug)]
pub struct Effect {
    /// Multiplier on wind acceleration.
    pub wind_mult: f64,
    /// Multiplier on the whole power range.
    pub power_mult: f64,
    /// Multiplier on muzzle speed (Sabot).
    pub muzzle_mult: f64,
    /// Multiplier on crater radius.
    pub crater_mult: f64,
    /// Multiplier on blast radius.
    pub blast_mult: f64,
    /// Multiplier on shell mass. Folded into [`Loadout::mass`], which
    /// nothing reads.
    pub mass_mult: f64,
    /// Divisor applied to angle quantization per stack (`1` is neutral).
    pub angle_quantum: i32,
    /// Ricochets allowed per shell.
    pub ricochet_bounces: u8,
    /// Submunitions per detonation.
    pub cluster_submunitions: u8,
    /// Past landing points kept marked.
    pub chalk_marks: u8,
    /// Added to every refund tier.
    pub refund_bonus: i32,
    /// Multiplier on refunds.
    pub refund_mult: f64,
    /// Granted once, the moment the modifier is taken.
    pub instant_shells: i32,
    /// Granted on every screen cleared.
    pub shells_per_clear: i32,
    /// Hard reserve cap, if the modifier imposes one.
    pub reserve_cap: i32,
    /// How far, in world units, the emplacement may move either way.
    pub enfilade_units: f64,
    /// Radius inside which a near miss still refunds.
    pub salvage_radius: f64,
    /// Shells refunded by a qualifying near miss, per stack.
    pub salvage_shells: i32,
    /// [`flags`] bits this modifier sets.
    pub flags: u32,
}

impl Effect {
    /// The neutral row: no change to anything.
    pub const NONE: Effect = Effect {
        wind_mult: 1.0,
        power_mult: 1.0,
        muzzle_mult: 1.0,
        crater_mult: 1.0,
        blast_mult: 1.0,
        mass_mult: 1.0,
        angle_quantum: 1,
        ricochet_bounces: 0,
        cluster_submunitions: 0,
        chalk_marks: 0,
        refund_bonus: 0,
        refund_mult: 1.0,
        instant_shells: 0,
        shells_per_clear: 0,
        reserve_cap: i32::MAX,
        enfilade_units: 0.0,
        salvage_radius: 0.0,
        salvage_shells: 0,
        flags: 0,
    };
}

/// A pool entry: the modifier, its display data, and its effect row.
pub struct Entry {
    /// Which modifier this row describes.
    pub modifier: Modifier,
    /// Upper-case display name.
    pub name: &'static str,
    /// One-line effect text for the draft card.
    pub blurb: &'static str,
    /// Build the modifier supports.
    pub tag: Tag,
    /// Draft column.
    pub category: Category,
    /// Most stacks a loadout may hold (DESIGN §4 "Stacks").
    pub max_stacks: u8,
    /// What each stack does.
    pub effect: Effect,
}

const fn entry(
    modifier: Modifier,
    name: &'static str,
    blurb: &'static str,
    tag: Tag,
    category: Category,
    max_stacks: u8,
    effect: Effect,
) -> Entry {
    Entry {
        modifier,
        name,
        blurb,
        tag,
        category,
        max_stacks,
        effect,
    }
}

/// The full pool, in declaration order. Index equals the `Modifier` discriminant.
pub static POOL: [Entry; MODIFIER_COUNT] = [
    // ------------------------------------------------------------ shell (§4)
    entry(
        Modifier::Ballast,
        "BALLAST",
        "Shell mass x1.6. Wind x0.5, power -20%.",
        Tag::Drift,
        Category::Shell,
        2,
        Effect {
            wind_mult: 0.5,
            power_mult: 0.8,
            mass_mult: 1.6,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Sabot,
        "SABOT",
        "Muzzle velocity +25%, crater radius x0.5.",
        Tag::Precision,
        Category::Shell,
        2,
        Effect {
            muzzle_mult: 1.25,
            crater_mult: 0.5,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Airburst,
        "AIRBURST",
        "Fuse set in distance. Blast x1.8, no contact needed.",
        Tag::Precision,
        Category::Shell,
        1,
        Effect {
            blast_mult: 1.8,
            flags: flags::AIRBURST,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Skipstone,
        "SKIPSTONE",
        "Ricochet at <=20 degrees incidence, keeping 70% velocity.",
        Tag::Demo,
        Category::Shell,
        2,
        Effect {
            ricochet_bounces: 1,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Auger,
        "AUGER",
        "Tunnels one material layer before detonating. Bedrock stops it.",
        Tag::Demo,
        Category::Shell,
        1,
        Effect {
            flags: flags::AUGER,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Cluster,
        "CLUSTER",
        "Splits into 3 submunitions at +/-15 degrees, 40% damage each.",
        Tag::Demo,
        Category::Shell,
        2,
        Effect {
            cluster_submunitions: 3,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::ThermalLance,
        "THERMAL LANCE",
        "Steel craters as Rock. Structures collapse rather than chip.",
        Tag::Demo,
        Category::Shell,
        1,
        Effect {
            flags: flags::THERMAL_LANCE,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Featherweight,
        "FEATHERWEIGHT",
        "Wind x2.2, power range +40%.",
        Tag::Risk,
        Category::Shell,
        1,
        Effect {
            wind_mult: 2.2,
            power_mult: 1.4,
            ..Effect::NONE
        },
    ),
    // ------------------------------------------------------- instrument (§4)
    entry(
        Modifier::Vernier,
        "VERNIER",
        "Angle quantization 0.5 -> 0.1 -> 0.05 degrees.",
        Tag::Precision,
        Category::Instrument,
        2,
        Effect {
            angle_quantum: 5,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Anemometer,
        "ANEMOMETER",
        "Wind shown to one decimal instead of rounded.",
        Tag::Precision,
        Category::Instrument,
        1,
        Effect {
            flags: flags::ANEMOMETER,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Rangefinder,
        "RANGEFINDER",
        "Exact horizontal distance to the nearest target.",
        Tag::Precision,
        Category::Instrument,
        1,
        Effect {
            flags: flags::RANGEFINDER,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::ChalkMarks,
        "CHALK MARKS",
        "The previous shot's landing point stays marked.",
        Tag::Precision,
        Category::Instrument,
        2,
        Effect {
            chalk_marks: 1,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::DeadReckoning,
        "DEAD RECKONING",
        "Wind hidden until the first shot lands. +2 shells per clear.",
        Tag::Risk,
        Category::Instrument,
        1,
        Effect {
            shells_per_clear: 2,
            flags: flags::DEAD_RECKONING,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::PlumbLine,
        "PLUMB LINE",
        "Vertical guide at the apex of the last trajectory.",
        Tag::Precision,
        Category::Instrument,
        1,
        Effect {
            flags: flags::PLUMB_LINE,
            ..Effect::NONE
        },
    ),
    // ---------------------------------------------------------- economy (§4)
    entry(
        Modifier::Salvage,
        "SALVAGE",
        "Misses cratering within 30 units refund 1 shell per stack.",
        Tag::Economy,
        Category::Economy,
        2,
        Effect {
            salvage_radius: 30.0,
            salvage_shells: 1,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Quartermaster,
        "QUARTERMASTER",
        "+1 to every refund tier, including 4+.",
        Tag::Economy,
        Category::Economy,
        2,
        Effect {
            refund_bonus: 1,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::Depot,
        "DEPOT",
        "+8 shells immediately. No ongoing effect.",
        Tag::Economy,
        Category::Economy,
        3,
        Effect {
            instant_shells: 8,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::StandingOrders,
        "STANDING ORDERS",
        "One-shot clears grant +1 per consecutive streak.",
        Tag::Economy,
        Category::Economy,
        1,
        Effect {
            flags: flags::STANDING_ORDERS,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::ScavengerRites,
        "SCAVENGER RITES",
        "Every 5th screen is a supply screen: no targets, +6 shells.",
        Tag::Economy,
        Category::Economy,
        1,
        Effect {
            flags: flags::SCAVENGER_RITES,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::ForcedMarch,
        "FORCED MARCH",
        "Refunds x2. Reserve cap drops to 15.",
        Tag::Risk,
        Category::Economy,
        1,
        Effect {
            refund_mult: 2.0,
            reserve_cap: 15,
            flags: flags::FORCED_MARCH,
            ..Effect::NONE
        },
    ),
    // ------------------------------------------------------- structural (§4)
    entry(
        Modifier::Enfilade,
        "ENFILADE",
        "Reposition the emplacement +/-25 units before the first shot.",
        Tag::Precision,
        Category::Structural,
        2,
        Effect {
            enfilade_units: 25.0,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::SappersEye,
        "SAPPER'S EYE",
        "Material layers visible as colour banding.",
        Tag::Demo,
        Category::Structural,
        1,
        Effect {
            flags: flags::SAPPERS_EYE,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::LongLens,
        "LONG LENS",
        "Camera zooms out one step; the whole screen is framed.",
        Tag::Precision,
        Category::Structural,
        1,
        Effect {
            flags: flags::LONG_LENS,
            ..Effect::NONE
        },
    ),
    entry(
        Modifier::StormFront,
        "STORM FRONT",
        "Wind resamples mid-flight once per shot. +12 shells now.",
        Tag::Risk,
        Category::Structural,
        1,
        Effect {
            instant_shells: 12,
            flags: flags::STORM_FRONT,
            ..Effect::NONE
        },
    ),
];

impl Modifier {
    /// Position in [`POOL`]; equal to the discriminant.
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
    /// The modifier's pool row.
    #[inline]
    pub fn entry(self) -> &'static Entry {
        &POOL[self.index()]
    }
    /// Upper-case display name.
    #[inline]
    pub fn name(self) -> &'static str {
        self.entry().name
    }
    /// Build tag.
    #[inline]
    pub fn tag(self) -> Tag {
        self.entry().tag
    }
    /// Most stacks a loadout may hold.
    #[inline]
    pub fn max_stacks(self) -> u8 {
        self.entry().max_stacks
    }
    /// Every modifier, in pool order.
    pub fn all() -> impl Iterator<Item = Modifier> {
        POOL.iter().map(|e| e.modifier)
    }
    /// Recover a modifier from its discriminant, for replay decoding.
    /// `None` past the end of the pool.
    pub fn from_index(i: usize) -> Option<Modifier> {
        POOL.get(i).map(|e| e.modifier)
    }
}

/// The folded effect of everything held. Ballistics, the solver and the
/// economy read only this — never the stack list.
///
/// The public fields are derived: [`Loadout::recompute`] overwrites them from
/// the stack counts, so edit the loadout through [`Loadout::take`].
#[derive(Clone, Debug)]
pub struct Loadout {
    stacks: [u8; MODIFIER_COUNT],
    /// Multiplier on wind acceleration (Ballast ×0.5, Featherweight ×2.2).
    pub wind_scale: f64,
    /// Multiplier on the whole power range (Ballast ×0.8, Featherweight ×1.4).
    pub power_scale: f64,
    /// Multiplier on muzzle speed (Sabot ×1.25).
    pub muzzle_scale: f64,
    /// Multiplier on crater radius (Sabot ×0.5).
    pub crater_scale: f64,
    /// Multiplier on blast radius (Airburst ×1.8).
    pub blast_scale: f64,
    /// Shell mass relative to the default (Ballast ×1.6). Tracked, but read
    /// by nothing: Ballast's wind resistance is carried by `wind_scale`.
    pub mass: f64,
    /// Angle input step in 0.05° ticks: 10 (0.5°), then 2 (0.1°) and 1
    /// (0.05°) with one and two Vernier stacks.
    pub angle_quantum_ticks: i32,
    /// Ricochets allowed per primary shell (Skipstone).
    pub ricochet_bounces: u8,
    /// Submunitions per Cluster split: 3 per stack.
    pub cluster_submunitions: u8,
    /// Past landing points kept marked (Chalk Marks).
    pub chalk_marks: u8,
    /// Added to every refund tier (Quartermaster).
    pub refund_bonus: i32,
    /// Multiplier on refunds (Forced March ×2).
    pub refund_mult: f64,
    /// Shells granted per screen cleared (Dead Reckoning).
    pub shells_per_clear: i32,
    /// Reserve cap; `i32::MAX` means uncapped. Read it through
    /// [`Loadout::cap`].
    pub reserve_cap: i32,
    /// Emplacement repositioning range either way, world units (Enfilade).
    pub enfilade_units: f64,
    /// Near-miss refund radius, world units (Salvage).
    pub salvage_radius: f64,
    /// Shells refunded by a qualifying near miss (Salvage).
    pub salvage_shells: i32,
    /// OR of every held modifier's [`flags`].
    pub flags: u32,
}

impl Default for Loadout {
    fn default() -> Self {
        let mut l = Loadout {
            stacks: [0; MODIFIER_COUNT],
            wind_scale: 1.0,
            power_scale: 1.0,
            muzzle_scale: 1.0,
            crater_scale: 1.0,
            blast_scale: 1.0,
            mass: 1.0,
            angle_quantum_ticks: ANGLE_QUANTUM_TICKS,
            ricochet_bounces: 0,
            cluster_submunitions: 0,
            chalk_marks: 0,
            refund_bonus: 0,
            refund_mult: 1.0,
            shells_per_clear: 0,
            reserve_cap: i32::MAX,
            enfilade_units: 0.0,
            salvage_radius: 0.0,
            salvage_shells: 0,
            flags: 0,
        };
        l.recompute();
        l
    }
}

impl Loadout {
    /// Nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stacks held of `m`.
    #[inline]
    pub fn stacks(&self, m: Modifier) -> u8 {
        self.stacks[m.index()]
    }

    /// Whether at least one stack of `m` is held.
    #[inline]
    pub fn has(&self, m: Modifier) -> bool {
        self.stacks(m) > 0
    }

    /// Whether any bit of `f` (a [`flags`] constant) is set.
    #[inline]
    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    /// Whether `m` is at its stack limit, and so out of the draft.
    #[inline]
    pub fn at_max(&self, m: Modifier) -> bool {
        self.stacks(m) >= m.max_stacks()
    }

    /// Take a modifier. Returns the shells granted immediately (Depot, Storm
    /// Front), or 0. Refuses to exceed max stacks.
    ///
    /// The caller adds the returned shells to the reserve; the loadout does
    /// not track shells.
    pub fn take(&mut self, m: Modifier) -> i32 {
        if self.at_max(m) {
            return 0;
        }
        self.stacks[m.index()] += 1;
        self.recompute();
        m.entry().effect.instant_shells
    }

    /// Fold the held stacks into the derived fields.
    ///
    /// Resets every derived field to neutral, then applies each held
    /// modifier in [`POOL`] order.
    pub fn recompute(&mut self) {
        let base = Loadout {
            stacks: self.stacks,
            wind_scale: 1.0,
            power_scale: 1.0,
            muzzle_scale: 1.0,
            crater_scale: 1.0,
            blast_scale: 1.0,
            mass: 1.0,
            angle_quantum_ticks: ANGLE_QUANTUM_TICKS,
            ricochet_bounces: 0,
            cluster_submunitions: 0,
            chalk_marks: 0,
            refund_bonus: 0,
            refund_mult: 1.0,
            shells_per_clear: 0,
            reserve_cap: i32::MAX,
            enfilade_units: 0.0,
            salvage_radius: 0.0,
            salvage_shells: 0,
            flags: 0,
        };
        *self = base;
        // Pool order is fixed, so the fold order is fixed: multiplication of
        // f64 is not associative and the result must not depend on draft order.
        for entry in POOL.iter() {
            let n = self.stacks[entry.modifier.index()];
            if n == 0 {
                continue;
            }
            let e = &entry.effect;
            for _ in 0..n {
                self.wind_scale *= e.wind_mult;
                self.power_scale *= e.power_mult;
                self.muzzle_scale *= e.muzzle_mult;
                self.crater_scale *= e.crater_mult;
                self.blast_scale *= e.blast_mult;
                self.mass *= e.mass_mult;
                self.refund_mult *= e.refund_mult;
            }
            let n32 = n as i32;
            // Integer division: Vernier's divisor of 5 takes 10 ticks to 2,
            // then 2 to 0, which the floor of 1 turns into the finest step.
            for _ in 0..n {
                self.angle_quantum_ticks = (self.angle_quantum_ticks / e.angle_quantum).max(1);
            }
            self.ricochet_bounces += e.ricochet_bounces * n;
            self.cluster_submunitions += e.cluster_submunitions * n;
            self.chalk_marks += e.chalk_marks * n;
            self.refund_bonus += e.refund_bonus * n32;
            self.shells_per_clear += e.shells_per_clear * n32;
            self.reserve_cap = self.reserve_cap.min(e.reserve_cap);
            self.enfilade_units += e.enfilade_units * n as f64;
            self.salvage_radius = self.salvage_radius.max(e.salvage_radius);
            self.salvage_shells += e.salvage_shells * n32;
            self.flags |= e.flags;
        }
    }

    // ------------------------------------------------------------ shorthands
    /// Airburst held.
    #[inline]
    pub fn airburst(&self) -> bool {
        self.has_flag(flags::AIRBURST)
    }
    /// Auger held.
    #[inline]
    pub fn auger(&self) -> bool {
        self.has_flag(flags::AUGER)
    }
    /// Thermal Lance held.
    #[inline]
    pub fn thermal_lance(&self) -> bool {
        self.has_flag(flags::THERMAL_LANCE)
    }
    /// Skipstone held (at least one ricochet).
    #[inline]
    pub fn ricochet(&self) -> bool {
        self.ricochet_bounces > 0
    }
    /// Cluster held (shells split on detonation).
    #[inline]
    pub fn cluster(&self) -> bool {
        self.cluster_submunitions > 0
    }
    /// Storm Front held.
    #[inline]
    pub fn storm_front(&self) -> bool {
        self.has_flag(flags::STORM_FRONT)
    }
    /// Dead Reckoning held.
    #[inline]
    pub fn dead_reckoning(&self) -> bool {
        self.has_flag(flags::DEAD_RECKONING)
    }
    /// Scavenger Rites held.
    #[inline]
    pub fn scavenger_rites(&self) -> bool {
        self.has_flag(flags::SCAVENGER_RITES)
    }
    /// Standing Orders held.
    #[inline]
    pub fn standing_orders(&self) -> bool {
        self.has_flag(flags::STANDING_ORDERS)
    }

    /// `Overhang` is only offered once the loadout can get into a pocket:
    /// ricochet, airburst or tunneling. DESIGN §2.4.
    #[inline]
    pub fn unlocks_overhang(&self) -> bool {
        self.airburst() || self.auger() || self.ricochet()
    }

    /// Effective reserve cap.
    ///
    /// Uncapped unless a modifier imposes one — `Forced March` drops it to 15.
    /// DESIGN §4 says the cap "drops to 15" without stating a starting cap;
    /// leaving the default open is the reading that keeps Depot, Storm Front
    /// and Quartermaster worth drafting, since a 40-shell ceiling would eat
    /// most of a Depot on the very first draft. Flagged rather than retuned.
    #[inline]
    pub fn cap(&self) -> i32 {
        self.reserve_cap.max(1)
    }

    /// Quantize an angle in ticks to what the player can actually dial in.
    ///
    /// Rounds to the nearest multiple of the quantum, halves upward. Meant for
    /// non-negative input: integer division truncates toward zero, so
    /// negative ticks do not round symmetrically. [`Shot::quantized`] clamps
    /// the result.
    ///
    /// [`Shot::quantized`]: crate::ballistics::Shot::quantized
    #[inline]
    pub fn quantize_angle(&self, ticks: i32) -> i32 {
        let q = self.angle_quantum_ticks.max(1);
        let n = (ticks + q / 2) / q;
        n * q
    }
}

/// Three offers drawn by screen seed, excluding anything already at max
/// stacks. DESIGN §4.
///
/// Shuffles the eligible modifiers with a seeded [`Pcg32`] (Fisher–Yates)
/// and keeps the first three: a draw without replacement, so the offers are
/// distinct and each eligible modifier is equally likely. Fewer than three
/// come back when fewer are eligible, and none when everything is maxed.
/// The run passes the seed of the first screen of the coming band.
pub fn draft_offers(screen_seed: u64, loadout: &Loadout) -> Vec<Modifier> {
    let mut eligible: Vec<Modifier> = Modifier::all().filter(|m| !loadout.at_max(*m)).collect();
    let mut r = Pcg32::new(substream(screen_seed, 0xD8AF_7000_0000_0001));
    r.shuffle(&mut eligible);
    eligible.truncate(3);
    eligible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::START_SHELLS;

    #[test]
    fn pool_indices_match_discriminants() {
        for (i, e) in POOL.iter().enumerate() {
            assert_eq!(e.modifier.index(), i, "{} out of order", e.name);
        }
        assert_eq!(POOL.len(), MODIFIER_COUNT);
    }

    #[test]
    fn ballast_halves_wind_and_narrows_power() {
        let mut l = Loadout::new();
        assert_eq!(l.wind_scale, 1.0);
        l.take(Modifier::Ballast);
        assert_eq!(l.wind_scale, 0.5);
        assert_eq!(l.power_scale, 0.8);
        assert_eq!(l.mass, 1.6);
        l.take(Modifier::Ballast);
        assert_eq!(l.wind_scale, 0.25);
        // Max 2 stacks — the third is refused.
        assert!(l.at_max(Modifier::Ballast));
        l.take(Modifier::Ballast);
        assert_eq!(l.stacks(Modifier::Ballast), 2);
    }

    #[test]
    fn fold_order_is_independent_of_draft_order() {
        let mut a = Loadout::new();
        a.take(Modifier::Ballast);
        a.take(Modifier::Sabot);
        a.take(Modifier::Featherweight);
        let mut b = Loadout::new();
        b.take(Modifier::Featherweight);
        b.take(Modifier::Sabot);
        b.take(Modifier::Ballast);
        assert_eq!(a.wind_scale.to_bits(), b.wind_scale.to_bits());
        assert_eq!(a.power_scale.to_bits(), b.power_scale.to_bits());
        assert_eq!(a.crater_scale.to_bits(), b.crater_scale.to_bits());
    }

    #[test]
    fn vernier_tightens_quantization() {
        let l = Loadout::new();
        assert_eq!(l.angle_quantum_ticks, 10);
        assert_eq!(l.quantize_angle(904), 900);
        assert_eq!(l.quantize_angle(906), 910);
        let mut v = Loadout::new();
        v.take(Modifier::Vernier);
        assert_eq!(v.angle_quantum_ticks, 2);
        assert_eq!(v.quantize_angle(905), 906);
        v.take(Modifier::Vernier);
        assert_eq!(v.angle_quantum_ticks, 1);
        assert_eq!(v.quantize_angle(907), 907);
    }

    #[test]
    fn salvage_refund_scales_with_stacks() {
        let mut l = Loadout::new();
        l.take(Modifier::Salvage);
        assert_eq!(l.salvage_radius, 30.0);
        assert_eq!(l.salvage_shells, 1);
        l.take(Modifier::Salvage);
        assert_eq!(l.salvage_radius, 30.0);
        assert_eq!(l.salvage_shells, 2);
    }

    #[test]
    fn instant_grants_are_returned_once() {
        let mut l = Loadout::new();
        assert_eq!(l.take(Modifier::Depot), 8);
        assert_eq!(l.take(Modifier::Depot), 8);
        assert_eq!(l.take(Modifier::Depot), 8);
        assert_eq!(l.take(Modifier::Depot), 0, "4th Depot exceeds max stacks");
        assert_eq!(l.take(Modifier::StormFront), 12);
    }

    #[test]
    fn forced_march_caps_the_reserve() {
        let mut l = Loadout::new();
        assert!(l.cap() > START_SHELLS, "the reserve is uncapped by default");
        l.take(Modifier::ForcedMarch);
        assert_eq!(l.cap(), 15);
        assert_eq!(l.refund_mult, 2.0);
    }

    #[test]
    fn overhang_unlock_needs_ricochet_airburst_or_tunneling() {
        let mut l = Loadout::new();
        assert!(!l.unlocks_overhang());
        l.take(Modifier::Vernier);
        l.take(Modifier::Quartermaster);
        assert!(!l.unlocks_overhang());
        for m in [Modifier::Airburst, Modifier::Auger, Modifier::Skipstone] {
            let mut l = Loadout::new();
            l.take(m);
            assert!(l.unlocks_overhang(), "{} should unlock Overhang", m.name());
        }
    }

    #[test]
    fn draft_excludes_maxed_modifiers_and_is_seed_stable() {
        let mut l = Loadout::new();
        l.take(Modifier::Airburst); // max 1
        let a = draft_offers(999, &l);
        let b = draft_offers(999, &l);
        assert_eq!(a, b);
        assert_eq!(a.len(), 3);
        assert!(!a.contains(&Modifier::Airburst));
        // Three distinct offers.
        assert_ne!(a[0], a[1]);
        assert_ne!(a[1], a[2]);
        assert_ne!(a[0], a[2]);
    }

    #[test]
    fn draft_shrinks_gracefully_when_the_pool_empties() {
        let mut l = Loadout::new();
        for m in Modifier::all() {
            for _ in 0..m.max_stacks() {
                l.take(m);
            }
        }
        assert!(draft_offers(5, &l).is_empty());
    }
}

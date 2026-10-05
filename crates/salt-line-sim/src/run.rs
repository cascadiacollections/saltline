//! Screen assembly, structures, damage, economy and the run state machine.
//! DESIGN §1, §2.3, §2.7.
//!
//! This is the top of the sim: the one type the presentation layer drives is
//! [`RunState`]. The Godot crate calls [`RunState::fire`],
//! [`RunState::resolve_clear`] and [`RunState::take_offer`] in response to
//! input and timers, and draws whatever [`Screen`] and [`ShotReport`] say
//! happened. It never decides an outcome itself (DESIGN §6).
//!
//! # Screens
//!
//! A screen is generated from nothing but the run seed, its index, the buffer
//! width and the loadout ([`generate_screen`]). Its seed comes from
//! [`screen_seed`], a stateless hash of run seed and index (DESIGN §2.2), so
//! no generator state carries from one screen to the next. Screen 400 needs
//! none of the screens before it (later bands cost more only because they
//! try harder templates), which is what lets tests, the batch harness and a
//! level inspector jump anywhere in a run. Inside a screen, each system (wind, shape, placement, template
//! choice) draws from its own named [`substream`] of that seed, so adding a
//! draw to one cannot shift the values another sees.
//!
//! Generation is generate-and-test: assemble a candidate, ask
//! [`solver::validate`] whether it has enough distinct solutions, and on
//! failure try again with the next seed, then with the `Flat` template, then
//! with the terrain flattened, then on level ground in calm air (DESIGN §2.3
//! step 6, §2.4). Every rung of that ladder passes through the same gate, so
//! no screen with targets is shown unproven.
//!
//! # The run state machine
//!
//! [`Phase`] is the state; the transitions are:
//!
//! ```text
//! Firing --fire, not cleared, shells left--> Firing
//! Firing --fire, not cleared, no shells----> Over
//! Firing --fire, screen cleared------------> Cleared
//! Cleared --resolve_clear, end of band-----> Drafting
//! Cleared --resolve_clear, otherwise-------> Firing (next screen) or Over
//! Drafting --take_offer / skip_draft-------> Firing (next screen) or Over
//! ```
//!
//! Each transition is a method call that returns once the state is settled;
//! nothing here runs on a clock. `Cleared` exists only so the presentation
//! layer can animate a collapse on the screen that was cleared before the
//! run moves on (see [`Phase::Cleared`]).
//!
//! # The shot log
//!
//! Every player decision that can change an outcome is appended to
//! [`RunState::log`] as a [`LoggedShot`]: shots with the fuse and emplacement
//! offset they were fired with, and draft picks. Together with the seed and
//! width that is the entire persisted run; [`crate::replay`] rebuilds the
//! rest. This is event sourcing: the state is a fold over the inputs, so
//! nothing derived ever needs saving.

use crate::ballistics::{
    self, blast_radius, crater_radius, fire, Aabb, End, FireOptions, Shot, Trace, World,
};
use crate::dmath::{abs, clamp};
use crate::modifiers::{draft_offers, Loadout, Modifier};
use crate::rng::{screen_seed, substream, Pcg32};
use crate::solver;
use crate::terrain::{self, Heightfield, Material, ShapeParams, Template};
use crate::{
    band_of, Vec2, BAND_LEN, EMPLACEMENT_X, MAX_GEN_ATTEMPTS, MIN_SOLUTIONS, MIN_TARGET_SEPARATION,
    SCREEN_W_REF, START_SHELLS,
};

// --------------------------------------------------------------- structures

/// Target silhouettes. DESIGN §5.4.
///
/// The discriminants are stable: [`crate::replay::hash_screen`] hashes them,
/// so renumbering a kind changes every pinned screen digest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum StructureKind {
    /// Narrow tapering lattice.
    Derrick = 0,
    /// Thin vertical with guy wires; the narrowest footprint.
    RadioMast = 1,
    /// A bulb on legs: the most top-heavy, so the easiest to undermine.
    WaterTower = 2,
    /// Solid cylinder with a domed cap; the least top-heavy.
    Silo = 3,
    /// Wide-based A-frame; the widest footprint.
    Pylon = 4,
}

impl StructureKind {
    /// Every kind in discriminant order, the order `kind_weights` indexes.
    pub const ALL: [StructureKind; 5] = [
        StructureKind::Derrick,
        StructureKind::RadioMast,
        StructureKind::WaterTower,
        StructureKind::Silo,
        StructureKind::Pylon,
    ];

    /// Footprint width in pixel columns. DESIGN §5.4.
    pub const fn footprint(self) -> usize {
        match self {
            StructureKind::Derrick => 7,
            StructureKind::RadioMast => 5,
            StructureKind::WaterTower => 9,
            StructureKind::Silo => 9,
            StructureKind::Pylon => 11,
        }
    }

    /// Sprite height, from `art/sprites/`.
    pub const fn height(self) -> f64 {
        match self {
            StructureKind::Derrick => 42.0,
            StructureKind::RadioMast => 32.0,
            StructureKind::WaterTower => 34.0,
            StructureKind::Silo => 27.0,
            StructureKind::Pylon => 33.0,
        }
    }

    /// How top-heavy the silhouette is. The water tower is a bulb on legs, so
    /// undermining it reads unmistakably — DESIGN §5.4 wants that legible.
    pub const fn top_heaviness(self) -> f64 {
        match self {
            StructureKind::WaterTower => 1.6,
            StructureKind::RadioMast => 1.3,
            StructureKind::Derrick => 1.1,
            StructureKind::Pylon => 0.9,
            StructureKind::Silo => 0.8,
        }
    }

    /// Lower-case display name, for test and harness messages.
    pub const fn name(self) -> &'static str {
        match self {
            StructureKind::Derrick => "derrick",
            StructureKind::RadioMast => "radio mast",
            StructureKind::WaterTower => "water tower",
            StructureKind::Silo => "silo",
            StructureKind::Pylon => "pylon",
        }
    }
}

/// Damage state of a structure. DESIGN §2.7. `Leaning` is a real state, not
/// a cosmetic one.
///
/// Derived, never set directly: `resolve_state` recomputes it from
/// [`Structure::integrity`] and [`Structure::support`] after every shot,
/// until the structure collapses. A later direct hit can clear its wreck.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum StructState {
    /// Integrity above 0.35 and support above 0.6.
    Intact = 0,
    /// Integrity at or below 0.35, or support at or below 0.6, and not
    /// collapsed. The bounding box shifts toward the weaker side, and the
    /// screen is not cleared.
    Leaning = 1,
    /// Integrity at or below 0, or support at or below 0.3. A collapsed
    /// structure takes no further blast damage and is never re-resolved.
    Collapsed = 2,
    /// A direct shell hit removed an already collapsed wreck. It no longer
    /// draws or collides, but still counts as a downed target.
    Cleared = 3,
}

/// How far a leaning structure's bounding box shifts, in pixels. Enough to
/// change the shot you need.
pub const LEAN_SHIFT: f64 = 4.0;
/// Material factor on a blast that lands inside the bounding box. Above 1.0
/// so that a clean direct hit zeroes integrity — DESIGN §1's refund table pays
/// for one-shot clears, and §2.4 requires every screen to have one.
pub const STEEL_BLAST_FACTOR: f64 = 1.15;
/// Thermal Lance: structures collapse rather than chip (DESIGN §4).
pub const THERMAL_BLAST_FACTOR: f64 = 2.6;

/// One target on a screen. DESIGN §2.7.
///
/// Two independent kill paths meet here: blasts lower `integrity` (direct),
/// craters in the footprint lower `support` (undermining). Either can collapse
/// it, and having both is the point of the design.
#[derive(Clone, Debug)]
pub struct Structure {
    /// Silhouette, which fixes footprint, height and top-heaviness.
    pub kind: StructureKind,
    /// Leftmost footprint column.
    pub x0: usize,
    /// Seated base elevation at spawn.
    pub base: f64,
    /// Mean footprint surface elevation at spawn — the denominator for support.
    pub original_mean: f64,
    /// Structural health, 1.0 at spawn. Each blast subtracts overlap times
    /// material factor times the shell's damage scale; it can go negative.
    pub integrity: f64,
    /// How well the ground still holds it up, 0.0..=1.0: the footprint's
    /// remaining height relative to `original_mean`, with the loss
    /// multiplied by [`StructureKind::top_heaviness`] (above 1 for top-heavy
    /// kinds, below 1 for Silo and Pylon), clamped to 0..=1.
    pub support: f64,
    /// Damage state, derived from `integrity` and `support`; fixed once
    /// `Collapsed` or `Cleared`.
    pub state: StructState,
    /// -1 leaning left, 0 upright, +1 leaning right.
    pub lean: i8,
}

impl Structure {
    /// Footprint width in pixel columns; see [`StructureKind::footprint`].
    pub fn footprint(&self) -> usize {
        self.kind.footprint()
    }

    /// Horizontal centre of the footprint, in world units. Ignores any lean:
    /// range, fuse and salvage distances are measured from the spawn position.
    pub fn centre_x(&self) -> f64 {
        self.x0 as f64 + self.footprint() as f64 * 0.5
    }

    /// Bounding box in world space. A lean shifts it by several pixels, which
    /// is the whole point of the state (DESIGN §2.7).
    pub fn aabb(&self) -> Aabb {
        let shift = self.lean as f64 * LEAN_SHIFT;
        let h = match self.state {
            StructState::Collapsed | StructState::Cleared => 6.0,
            // A leaning silhouette is redrawn at ~15 degrees, so it loses a
            // little height and gains a little width.
            StructState::Leaning => self.kind.height() * 0.95,
            StructState::Intact => self.kind.height(),
        };
        let extra = if self.state == StructState::Leaning {
            2.0
        } else {
            0.0
        };
        Aabb::new(
            self.x0 as f64 - extra + shift.min(0.0),
            self.base,
            self.x0 as f64 + self.footprint() as f64 + extra + shift.max(0.0),
            self.base + h,
        )
    }

    /// Whether it has been downed. Cleared wrecks still count toward clearing
    /// a screen; leaning structures do not.
    pub fn is_down(&self) -> bool {
        matches!(self.state, StructState::Collapsed | StructState::Cleared)
    }

    /// DESIGN §2.7 threshold table. Collapse is checked first.
    fn resolve_state(&mut self) {
        self.state = if self.integrity <= 0.0 || self.support <= 0.3 {
            StructState::Collapsed
        } else if self.integrity <= 0.35 || self.support <= 0.6 {
            StructState::Leaning
        } else {
            StructState::Intact
        };
    }

    /// Lean toward the weaker side of the footprint.
    fn lean_direction(&self, field: &Heightfield) -> i8 {
        let n = self.footprint();
        let half = n / 2;
        let mean = |from: usize, to: usize| -> f64 {
            let mut s = 0.0;
            let mut c = 0.0;
            for i in from..to.min(field.len()) {
                s += field.surface(i);
                c += 1.0;
            }
            if c == 0.0 {
                0.0
            } else {
                s / c
            }
        };
        let left = mean(self.x0, self.x0 + half);
        let right = mean(self.x0 + n - half, self.x0 + n);
        if left < right - 0.25 {
            -1
        } else if right < left - 0.25 {
            1
        } else {
            0
        }
    }
}

// -------------------------------------------------------------------- screen

/// What one shot did. Returned by `Screen::apply_shot` so the run state and the
/// presentation layer see the same account of it.
#[derive(Clone, Debug)]
pub struct ShotReport {
    /// The shell fired, then any cluster submunitions.
    pub traces: Vec<Trace>,
    /// Indices into [`Screen::structures`] of structures this shot collapsed.
    pub collapsed: Vec<usize>,
    /// Indices of structures that went from Intact to Leaning on this shot.
    pub newly_leaning: Vec<usize>,
    /// Whether the screen is clear after this shot; see [`Screen::cleared`].
    pub cleared: bool,
    /// A miss that cratered close enough for Salvage to refund it.
    pub salvage: bool,
    /// Landing point for Chalk Marks.
    pub chalk: Option<Vec2>,
    /// Apex for Plumb Line.
    pub apex: Vec2,
}

/// One screen: an emplacement, terrain and a target set. DESIGN §1, §2.
///
/// Built only by [`generate_screen`], then mutated in place by shots
/// ([`Screen::apply_shot`]): craters and debris go into the heightfield's
/// scar buffer, damage into the structures. A screen is discarded when the
/// run moves on; nothing from it reaches the next one.
#[derive(Clone, Debug)]
pub struct Screen {
    /// Position in the run, from 0.
    pub index: u32,
    /// The seed this screen was actually built from: the screen seed of
    /// DESIGN §2.2, offset by a retry if earlier attempts failed validation.
    pub seed: u64,
    /// Buffer width in world units. Not clamped here; the game clamps it with
    /// [`buffer_width`] before starting a run.
    pub width: usize,
    /// Shape template used; `Flat` when generation fell back.
    pub template: Template,
    /// Heightfield, including every crater and debris mound so far.
    pub field: Heightfield,
    /// Targets, in placement order. Indices into this are what
    /// [`ShotReport`] and the solver use.
    pub structures: Vec<Structure>,
    /// Signed wind acceleration, resampled per screen (DESIGN §2.5).
    pub wind: f64,
    /// Wind after Storm Front's mid-flight resample, if held.
    pub wind_late: Option<f64>,
    /// Emplacement x in world units: `EMPLACEMENT_X` plus any Enfilade offset.
    /// Set by [`RunState`], which owns the offset.
    pub muzzle_x: f64,
    /// Surviving-candidate count for the hardest target. The difficulty proxy
    /// DESIGN §2.4 asks to be logged.
    pub candidates: usize,
    /// Generation attempts it took, counting from 1. Above `MAX_GEN_ATTEMPTS`
    /// means the template attempts all failed; for the batch harness; 0 for
    /// a supply screen.
    pub attempts: u32,
    /// Whether generation gave up on the drawn template and used `Flat`.
    pub fell_back: bool,
    /// Which rung of the generation ladder produced the screen (see
    /// [`generate_screen`]); `None` for a supply screen, which is not
    /// generated against the gate. Not hashed, like `attempts`.
    pub rung: Option<Rung>,
    /// Scavenger Rites supply screen: no targets, shells instead.
    pub supply: bool,
}

/// Height of the barrel pivot above the seated surface.
///
/// Measured off the art rather than picked: `gen_sprites.py::tank` puts the
/// pivot at sprite row 3 of a 12-row chassis whose lowest drawn row is 11, and
/// the reference scene seats that chassis two rows into the ground. Pivot to
/// ground is therefore six pixels.
pub const MUZZLE_HEIGHT: f64 = 6.0;
/// Chassis width used to seat the emplacement.
pub const CHASSIS_WIDTH: usize = 26;

impl Screen {
    /// Barrel pivot in world space.
    pub fn muzzle(&self) -> Vec2 {
        let x0 = clamp(
            self.muzzle_x - CHASSIS_WIDTH as f64 * 0.5,
            0.0,
            self.width as f64,
        ) as usize;
        Vec2::new(
            self.muzzle_x,
            self.field.seat(x0, CHASSIS_WIDTH) + MUZZLE_HEIGHT,
        )
    }

    /// Boxes for every structure, collapsed ones included as low rubble, in
    /// structure order. A cleared wreck gets an unreachable off-world box so
    /// ballistic hit indices still correspond to `structures`.
    pub fn target_boxes(&self) -> Vec<Aabb> {
        self.structures
            .iter()
            .map(|s| {
                if s.state == StructState::Cleared {
                    Aabb::new(-1.0e9, -1.0e9, -1.0e9 + 1.0, -1.0e9 + 1.0)
                } else {
                    s.aabb()
                }
            })
            .collect()
    }

    /// Whether every structure has collapsed. A supply screen is always
    /// clear. Leaning does not count (DESIGN §2.7).
    pub fn cleared(&self) -> bool {
        self.supply || self.structures.iter().all(|s| s.is_down())
    }

    /// The view of this screen that ballistics needs, with `boxes` from
    /// [`Screen::target_boxes`]. The boxes are passed in rather than built
    /// here so the caller owns them for the lifetime of the `World`.
    pub fn world<'a>(&'a self, boxes: &'a [Aabb]) -> World<'a> {
        World {
            field: &self.field,
            targets: boxes,
            width: self.width,
            wind: self.wind,
        }
    }

    /// Horizontal distance to the nearest standing target — what Rangefinder
    /// displays (DESIGN §4). World units, muzzle to target centre; `None`
    /// once every target is down.
    pub fn range_to_nearest(&self) -> Option<f64> {
        let m = self.muzzle();
        self.structures
            .iter()
            .filter(|s| !s.is_down())
            .map(|s| abs(s.centre_x() - m.x))
            .fold(None, |acc: Option<f64>, d| {
                Some(match acc {
                    Some(a) if a <= d => a,
                    _ => d,
                })
            })
    }

    /// The fuse an airburst wants for a given target: the horizontal distance
    /// from the muzzle to its centre, in world units. `None` if `target` is
    /// not an index into `structures`.
    pub fn default_fuse(&self, target: usize) -> Option<f64> {
        self.structures
            .get(target)
            .map(|s| abs(s.centre_x() - self.muzzle().x))
    }

    fn salvage_qualifies(&self, detonation: Vec2, lo: &Loadout) -> bool {
        lo.salvage_radius > 0.0
            && self
                .structures
                .iter()
                .filter(|s| !s.is_down())
                .any(|s| abs(s.centre_x() - detonation.x) <= lo.salvage_radius)
    }

    fn fire_options(&self, lo: &Loadout, fuse: Option<f64>, record_path: bool) -> FireOptions {
        FireOptions {
            record_path,
            fuse: if lo.airburst() { fuse } else { None },
            wind_after_apex: if lo.storm_front() {
                self.wind_late
            } else {
                None
            },
        }
    }

    /// Fire a shot and apply everything it does: craters, structural damage,
    /// undermining, collapse and debris.
    ///
    /// `shot` is used as given; [`RunState::fire`] quantises it to the
    /// loadout first, and callers that bypass it (tests, the solver's
    /// candidates) pass shots already on the grid. `fuse` is the Airburst
    /// fuse in world units from the muzzle, ignored without Airburst.
    /// `record_path` keeps the sampled flight path in the traces for the
    /// renderer; headless callers pass `false` to skip the allocation.
    ///
    /// Order matters for determinism and is fixed: every detonation is
    /// applied (craters, then damage) before support and states are
    /// resolved once, then debris mounds are added for what collapsed.
    pub fn apply_shot(
        &mut self,
        shot: Shot,
        lo: &Loadout,
        fuse: Option<f64>,
        record_path: bool,
    ) -> ShotReport {
        let boxes = self.target_boxes();
        let opts = self.fire_options(lo, fuse, record_path);
        let muzzle = self.muzzle();
        let traces = {
            let world = self.world(&boxes);
            fire(&world, muzzle, shot, lo, &opts)
        };

        let mut report = ShotReport {
            collapsed: Vec::new(),
            newly_leaning: Vec::new(),
            cleared: false,
            salvage: false,
            chalk: traces[0].detonation(),
            apex: traces[0].apex,
            traces,
        };

        let before: Vec<StructState> = self.structures.iter().map(|s| s.state).collect();

        for i in 0..report.traces.len() {
            let (end, damage_scale) = (report.traces[i].end, report.traces[i].damage_scale);
            if !end.detonates() {
                continue;
            }
            self.detonate(end, damage_scale, lo);
        }

        self.resolve_structures();

        for (i, s) in self.structures.iter().enumerate() {
            match (before[i], s.state) {
                (b, StructState::Collapsed) if b != StructState::Collapsed => {
                    report.collapsed.push(i)
                }
                (StructState::Intact, StructState::Leaning) => report.newly_leaning.push(i),
                _ => {}
            }
        }

        // Debris adds a small positive mound where the structure landed, so
        // terrain gets slightly higher where a structure fell (DESIGN §2.7).
        // The mound goes in after this shot's states were resolved, so any
        // effect it has on support is seen at the next resolve.
        for &i in &report.collapsed {
            let s = self.structures[i].clone();
            let dir = s.lean as f64;
            let cx = s.centre_x() + dir * s.kind.height() * 0.35;
            self.field.mound(
                cx,
                s.footprint() as f64 * 0.9 + 3.0,
                2.0 + s.kind.height() * 0.05,
            );
        }

        // Salvage: a shot that felled nothing but whose first detonation
        // landed horizontally within `salvage_radius` (30 units with the modifier) of a
        // standing target's centre.
        if lo.salvage_radius > 0.0 && report.collapsed.is_empty() {
            if let Some(det) = report.traces.iter().filter_map(|t| t.detonation()).next() {
                report.salvage = self.salvage_qualifies(det, lo);
            }
        }

        report.cleared = self.cleared();
        report
    }

    /// One detonation: crater the ground, damage what the blast overlaps.
    ///
    /// The crater is sized by the surface layer of the detonation's column
    /// (Steel for a direct hit), not by [`End::Terrain`]'s `material`: a
    /// tunnelling shell that stops in a deeper layer still craters as the
    /// surface above it does.
    fn detonate(&mut self, end: End, damage_scale: f64, lo: &Loadout) {
        let pos = end.pos();
        let direct = match end {
            End::Structure { index, .. } => Some(index),
            _ => None,
        };
        if let Some(i) = direct {
            if self.structures[i].state == StructState::Collapsed {
                self.structures[i].state = StructState::Cleared;
            }
        }
        let material = if direct.is_some() {
            Material::Steel
        } else {
            self.field.material_at(pos.x)
        };

        // Crater, attenuated by height above the surface — an airburst high
        // over the target barely marks the ground.
        let r = crater_radius(material, lo, damage_scale);
        let height_above = pos.y - self.field.surface_at(pos.x);
        if height_above < r {
            let effective = r * (1.0 - clamp(height_above / r, 0.0, 1.0));
            self.field.crater(pos.x, effective);
        }

        // Direct and splash damage. DESIGN §2.7: blast overlap x material factor.
        let blast = blast_radius(lo, damage_scale);
        let factor = if lo.thermal_lance() {
            THERMAL_BLAST_FACTOR
        } else {
            STEEL_BLAST_FACTOR
        };
        for (i, s) in self.structures.iter_mut().enumerate() {
            if s.is_down() {
                continue;
            }
            let overlap = if direct == Some(i) {
                1.0
            } else {
                let d = s.aabb().distance_to(pos);
                if d >= blast {
                    continue;
                }
                1.0 - d / blast
            };
            s.integrity -= overlap * factor * damage_scale;
        }
    }

    /// Recompute support from the scar buffer and re-resolve the state of
    /// every standing structure. Called once per shot, after every
    /// detonation (DESIGN §2.7 says after each detonation; with Cluster this
    /// differs — see NOTES or the follow-up issue).
    ///
    /// Collapse is permanent: a collapsed structure is skipped, so its
    /// support, state and lean stay as they were when it fell, even if debris
    /// later refills its footprint.
    ///
    /// Public because undermining can happen without a detonation of its own —
    /// debris landing, or a test excavating a footprint directly.
    pub fn resolve_structures(&mut self) {
        for s in self.structures.iter_mut() {
            if s.is_down() {
                continue;
            }
            let raw = self
                .field
                .support_fraction(s.x0, s.kind.footprint(), s.original_mean);
            // Top-heavy silhouettes lose their footing sooner — that is what
            // makes undermining the water tower legible.
            let loss = (1.0 - raw) * s.kind.top_heaviness();
            s.support = clamp(1.0 - loss, 0.0, 1.0);
            s.resolve_state();
            if s.state == StructState::Collapsed {
                if s.lean == 0 {
                    s.lean = s.lean_direction(&self.field);
                    if s.lean == 0 {
                        s.lean = 1;
                    }
                }
            } else if s.state == StructState::Leaning {
                s.lean = s.lean_direction(&self.field);
            }
        }
    }
}

// ------------------------------------------------------------ curve (§2.5)

/// Target distance in world units. `110 + band*6`, capped at 290.
pub fn target_distance(band: u32) -> f64 {
    (110.0 + band as f64 * 6.0).min(290.0)
}

/// Wind magnitude. `band*0.35`, capped at 7.0, resampled per screen.
pub fn wind_magnitude(band: u32) -> f64 {
    (band as f64 * 0.35).min(7.0)
}

/// Terrain amplitude multiplier. `1.0 + band*0.05`, capped at 2.2.
pub fn terrain_amplitude(band: u32) -> f64 {
    (1.0 + band as f64 * 0.05).min(2.2)
}

/// Maximum target count: 1, rising to 2 at band 1 and 3 at band 2.
pub fn target_count(band: u32) -> u32 {
    if band >= 2 {
        3
    } else if band >= 1 {
        2
    } else {
        1
    }
}

/// Weighted target-count mix for a band. Multi-target screens enter alongside
/// the first wind band, then ramp gradually so the opening reserve does not
/// spend twelve screens growing on guaranteed single-target clears.
pub fn target_count_weights(band: u32) -> [u32; 3] {
    match band {
        0 => [100, 0, 0],
        1 => [65, 35, 0],
        2 => [40, 55, 5],
        3..=5 => [20, 60, 20],
        6..=8 => [10, 55, 35],
        9..=19 => [10, 40, 50],
        _ => [5, 25, 70],
    }
}

/// Elevation delta half-width. `+/-(8 + band*3)`, capped at +/-70.
pub fn elevation_delta_limit(band: u32) -> f64 {
    (8.0 + band as f64 * 3.0).min(70.0)
}

/// Template weights for a band, gated on the loadout. DESIGN §2.3 step 3 and
/// §2.5: past band 20 stop ramping magnitude and ramp structural difficulty
/// toward Spires / Overhang / Basin instead.
pub fn template_weights(band: u32, lo: &Loadout) -> [u32; 7] {
    let late = band.saturating_sub(20).min(30);
    let mut w = [0u32; 7];
    for (i, t) in Template::ALL.iter().enumerate() {
        if band < t.min_band() {
            continue;
        }
        if t.needs_loadout_unlock() && !lo.unlocks_overhang() {
            continue;
        }
        w[i] = match t {
            Template::Flat => 26u32.saturating_sub(band.min(20)),
            Template::Ridge => 22,
            Template::Chasm => 20,
            Template::Mesa => 20,
            Template::Spires => 18 + late * 2,
            Template::Overhang => 16 + late * 3,
            Template::Basin => 16 + late * 2,
        };
    }
    if w.iter().all(|x| *x == 0) {
        w[Template::Flat as usize] = 1;
    }
    w
}

/// Give a short first run a readable sequence of shot problems. The other
/// screens still draw from the band's weights, and every choice still passes
/// the normal solvability gate.
fn screen_template_weights(index: u32, lo: &Loadout) -> [u32; 7] {
    let featured = match index {
        2 => Some(Template::Ridge),
        5 => Some(Template::Chasm),
        _ => None,
    };
    if let Some(template) = featured {
        let mut weights = [0; 7];
        weights[template as usize] = 1;
        weights
    } else {
        template_weights(band_of(index), lo)
    }
}

// -------------------------------------------------------- screen assembly

fn kind_weights(band: u32) -> [u32; 5] {
    // The water tower leads early: DESIGN §5.4 wants undermining to teach
    // itself, and a bulb on legs is the silhouette that does it.
    if band < 2 {
        [10, 6, 40, 10, 4]
    } else if band < 6 {
        [16, 14, 26, 20, 14]
    } else {
        [20, 20, 20, 20, 20]
    }
}

fn place_structures(
    field: &Heightfield,
    rng: &mut Pcg32,
    index: u32,
    band: u32,
    width: usize,
    primary_x: f64,
    count: u32,
) -> Vec<Structure> {
    let mut xs: Vec<f64> = Vec::new();
    xs.push(primary_x);
    // Extra targets step back toward the emplacement, never closer than the
    // 40-unit minimum separation (DESIGN §2.3 step 5).
    for k in 1..count {
        let step = rng.range_f64(MIN_TARGET_SEPARATION, MIN_TARGET_SEPARATION + 26.0);
        let candidate_left = primary_x - step * k as f64;
        let candidate_right = primary_x + step * k as f64;
        let x = if candidate_right < width as f64 - 16.0 && rng.next_f64() < 0.35 {
            candidate_right
        } else {
            candidate_left
        };
        xs.push(clamp(x, EMPLACEMENT_X + 60.0, width as f64 - 16.0));
    }

    let mut out: Vec<Structure> = Vec::new();
    for x in xs {
        if out
            .iter()
            .any(|s| abs(s.centre_x() - x) < MIN_TARGET_SEPARATION)
        {
            continue;
        }
        let sampled = StructureKind::ALL[rng.weighted(&kind_weights(band))];
        let kind = if out.is_empty() {
            match index {
                2 => StructureKind::RadioMast,
                5 => StructureKind::Silo,
                _ => sampled,
            }
        } else {
            sampled
        };
        out.push(seat_structure(field, kind, x, width));
    }
    out
}

/// An intact structure of `kind` centred as near `x` as the buffer allows,
/// seated on the highest column of its footprint so nothing floats.
fn seat_structure(field: &Heightfield, kind: StructureKind, x: f64, width: usize) -> Structure {
    let fp = kind.footprint();
    let x0 = clamp(x - fp as f64 * 0.5, 1.0, (width - fp - 1) as f64) as usize;
    let base = field.seat(x0, fp);
    let mut sum = 0.0;
    for i in x0..x0 + fp {
        sum += field.surface(i);
    }
    Structure {
        kind,
        x0,
        base,
        original_mean: sum / fp as f64,
        integrity: 1.0,
        support: 1.0,
        state: StructState::Intact,
        lean: 0,
    }
}

/// Share of the loadout's flat-ground maximum that a target may be placed at.
/// The rest is headroom for wind drift and the elevation delta.
pub const RANGE_MARGIN: f64 = 0.75;

/// Target distance for a band, clamped to what the loadout can actually reach.
pub fn reachable_distance(band: u32, lo: &Loadout) -> f64 {
    target_distance(band).min(ballistics::max_flat_range(lo) * RANGE_MARGIN)
}

fn assemble(
    index: u32,
    seed: u64,
    width: usize,
    template: Template,
    band: u32,
    lo: &Loadout,
    force_flat_terrain: bool,
) -> Screen {
    // One named stream per system; the tags are ASCII "WAND", "SHAP" and
    // "PLAC". Wind, shape and placement each draw from their own generator,
    // so an extra draw in one (a new shape parameter, say) cannot shift the
    // values the others draw.
    let mut r_wind = Pcg32::new(substream(seed, 0x5741_4E44));
    let mut r_shape = Pcg32::new(substream(seed, 0x5348_4150));
    let mut r_place = Pcg32::new(substream(seed, 0x504C_4143));

    let wind = r_wind.signed(wind_magnitude(band));
    // Storm Front's mid-flight resample is drawn now so it is part of the
    // screen, not of the moment the trigger is pulled.
    let wind_late = Some(r_wind.signed(wind_magnitude(band)));
    let elev_delta = r_shape.signed(elevation_delta_limit(band));
    let spire_count = r_shape.range_u32(2, 3);

    let primary_x = clamp(
        EMPLACEMENT_X + reachable_distance(band, lo),
        EMPLACEMENT_X + 60.0,
        width as f64 - 16.0,
    );

    let p = ShapeParams {
        width,
        amp: terrain_amplitude(band),
        muzzle_x: EMPLACEMENT_X,
        target_x: primary_x,
        elev_delta,
        spire_count,
    };

    let field = if force_flat_terrain {
        // Last-resort fallback: a gentle roll with no template at all. A
        // boring screen is better than a hang (DESIGN §2.3 step 6).
        let mut base = terrain::base_ridge(width, seed, 0.35);
        for h in base.iter_mut() {
            *h = clamp(
                *h * 0.5 + terrain::BASE_ELEVATION * 0.5,
                terrain::WORLD_FLOOR,
                70.0,
            );
        }
        Heightfield::from_base(base)
    } else {
        terrain::generate(seed, template, &p)
    };

    let count = r_place.weighted(&target_count_weights(band)) as u32 + 1;
    let structures = place_structures(&field, &mut r_place, index, band, width, primary_x, count);

    Screen {
        index,
        seed,
        width,
        template: if force_flat_terrain {
            Template::Flat
        } else {
            template
        },
        field,
        structures,
        wind,
        wind_late,
        muzzle_x: EMPLACEMENT_X,
        candidates: 0,
        attempts: 0,
        fell_back: force_flat_terrain,
        rung: Some(if force_flat_terrain {
            Rung::Terrainless
        } else {
            Rung::Drawn
        }),
        supply: false,
    }
}

/// Build screen `index` of the run seeded `run_seed`, at buffer width `width`
/// and for loadout `lo`.
///
/// DESIGN §2.3 step 6: reject and regenerate with `screen_seed + 1` on
/// failure, capped at 8 attempts, then fall back to `Flat`. The ladder has
/// four rungs ([`Rung`]):
///
/// 1. Up to `MAX_GEN_ATTEMPTS` drawn templates, each on the next seed.
/// 2. Up to `MAX_GEN_ATTEMPTS` `Flat` screens on a separate run of seeds.
/// 3. No template: the screen seed's base ridge flattened to a gentle roll,
///    cut to one target.
/// 4. Dead level ground, no wind before or after the apex, one Pylon well
///    inside the loadout's range ([`level_screen`]).
///
/// Every rung returns a screen only if it passes [`solver::validate`], so
/// DESIGN §2.4 holds for fallbacks too. Rung 4 is built so that it cannot
/// fail for any loadout the game can hold; the gate is there to prove it
/// rather than to choose. Should it fail anyway, debug and test builds panic,
/// naming the seed, screen and loadout. A release build returns the rung-4
/// screen with its failing candidate count, which the batch harness treats
/// as an error. Supply screens (Scavenger Rites) have no targets, so there
/// is nothing to validate; they are exempt by nature and report no rung.
///
/// Pure: the same arguments always give a bit-identical screen, whatever was
/// generated before. The loadout is an input because it changes what is
/// reachable ([`reachable_distance`]) and which templates are allowed
/// ([`template_weights`]). So two runs on the same seed can meet different
/// screens once their drafts differ; the seed alone fixes the run only
/// together with the player's picks, which is why picks are logged.
pub fn generate_screen(run_seed: u64, index: u32, width: usize, lo: &Loadout) -> Screen {
    generate_screen_from(run_seed, index, width, lo, Rung::Drawn)
}

/// Rungs of the generation ladder, in the order [`generate_screen`] climbs
/// down them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rung {
    /// A template drawn from the band's weights.
    Drawn,
    /// The `Flat` template on a separate run of seeds.
    Flat,
    /// No template: the base ridge flattened to a gentle roll, one target.
    Terrainless,
    /// Level ground, calm air, one target well inside range.
    Level,
}

impl Rung {
    /// Every rung, top first.
    pub const ALL: [Rung; 4] = [Rung::Drawn, Rung::Flat, Rung::Terrainless, Rung::Level];
}

/// [`generate_screen`], starting the ladder at rung `from` rather than the
/// top. `Rung::Drawn` is `generate_screen` itself; the later rungs exist so
/// tests can force a fallback without hunting for a seed that reaches it.
/// Supply screens are still supply screens whatever `from` says.
pub fn generate_screen_from(
    run_seed: u64,
    index: u32,
    width: usize,
    lo: &Loadout,
    from: Rung,
) -> Screen {
    let band = band_of(index);
    let seed0 = screen_seed(run_seed, index);

    // Scavenger Rites: every 5th screen is a supply screen (DESIGN §4).
    if lo.scavenger_rites() && index > 0 && index.is_multiple_of(5) {
        let mut s = assemble(index, seed0, width, Template::Flat, band, lo, false);
        s.structures.clear();
        s.supply = true;
        s.rung = None;
        s.candidates = usize::MAX;
        return s;
    }

    if from <= Rung::Drawn {
        let mut r_template = Pcg32::new(substream(seed0, 0x5445_4D50));
        let weights = screen_template_weights(index, lo);
        for attempt in 0..MAX_GEN_ATTEMPTS {
            let seed = seed0.wrapping_add(attempt as u64);
            let template = Template::ALL[r_template.weighted(&weights)];
            let mut screen = assemble(index, seed, width, template, band, lo, false);
            screen.attempts = attempt + 1;
            if gate(&mut screen, lo) {
                return screen;
            }
        }
    }

    if from <= Rung::Flat {
        for attempt in 0..MAX_GEN_ATTEMPTS {
            let seed = seed0.wrapping_add(0x1000 + attempt as u64);
            let mut screen = assemble(index, seed, width, Template::Flat, band, lo, false);
            screen.attempts = MAX_GEN_ATTEMPTS + attempt + 1;
            screen.fell_back = true;
            screen.rung = Some(Rung::Flat);
            if gate(&mut screen, lo) {
                return screen;
            }
        }
    }

    if from <= Rung::Terrainless {
        let mut screen = assemble(index, seed0, width, Template::Flat, band, lo, true);
        screen.structures.truncate(1);
        screen.attempts = MAX_GEN_ATTEMPTS * 2 + 1;
        if gate(&mut screen, lo) {
            return screen;
        }
    }

    let mut screen = level_screen(index, seed0, width, band, lo);
    if !gate(&mut screen, lo) && cfg!(debug_assertions) {
        panic!(
            "unsolvable screen: run seed {run_seed:#x}, screen {index}, width {width}: \
             even the level fallback has only {} direct candidates for {lo:?}",
            screen.candidates
        );
    }
    screen
}

/// Validate `screen` against `lo` and record the direct-aim count in
/// `candidates`. True if it passes the DESIGN §2.4 gate.
fn gate(screen: &mut Screen, lo: &Loadout) -> bool {
    let report = solver::validate(screen, lo);
    screen.candidates = report.worst_direct;
    report.solvable()
}

/// Share of the loadout's flat-ground range at which [`level_screen`] puts
/// its target.
pub const LEVEL_RANGE_SHARE: f64 = 0.5;

/// Rung 4 of the generation ladder: the simplest screen that is still a
/// screen.
///
/// Solvable by construction, whatever the loadout:
///
/// - The ground is dead level at `BASE_ELEVATION`, so nothing stands between
///   muzzle and target.
/// - Wind is zero before and after the apex. Every wind modifier (Ballast,
///   Featherweight, Anemometer, Storm Front, Dead Reckoning) scales,
///   resamples or hides a quantity that is now nothing.
/// - The target sits at [`LEVEL_RANGE_SHARE`] of
///   [`ballistics::max_flat_range`], or at the band's design distance if
///   that is nearer, and never nearer than the usual 60-unit minimum. At half
///   range every speed from about 71% of the maximum up has both a low and a
///   high angle that lands there, so the sweep finds shots across a wide
///   band of power; the coarsest angle quantum (0.5°) moves the landing
///   point by a few units at most, less than the target's width. The minimum
///   speed is no obstacle: a steep enough angle shortens any shot.
///   Featherweight, Sabot and Ballast only move the maximum, and the
///   distance moves with it.
/// - The target is a Pylon, the largest box of the five kinds, and a direct
///   hit from any loadout takes its integrity below zero
///   (`STEEL_BLAST_FACTOR` is above 1). Airburst's default fuse bursts over
///   the target centre, which the low-angle shots cross at the target's own
///   height.
///
/// Long Lens changes the camera, not the world, and Overhang is a template,
/// so neither reaches this rung. The width is only a clamp: the target is
/// kept 16 units inside the right edge, as everywhere else. The seed is
/// recorded but draws nothing.
pub fn level_screen(index: u32, seed: u64, width: usize, band: u32, lo: &Loadout) -> Screen {
    let field = Heightfield::from_base(vec![terrain::BASE_ELEVATION; width]);
    let distance = target_distance(band).min(ballistics::max_flat_range(lo) * LEVEL_RANGE_SHARE);
    let x = clamp(
        EMPLACEMENT_X + distance,
        EMPLACEMENT_X + 60.0,
        width as f64 - 16.0,
    );
    let structures = vec![seat_structure(&field, StructureKind::Pylon, x, width)];
    Screen {
        index,
        seed,
        width,
        template: Template::Flat,
        field,
        structures,
        wind: 0.0,
        wind_late: Some(0.0),
        muzzle_x: EMPLACEMENT_X,
        candidates: 0,
        attempts: MAX_GEN_ATTEMPTS * 2 + 2,
        fell_back: true,
        rung: Some(Rung::Level),
        supply: false,
    }
}

// ------------------------------------------------------------ economy (§1)

/// Refund for clearing a screen in `shots` shots, before modifiers.
/// DESIGN §1: 1 shot +3, 2 shots +2, 3 shots +1, 4 or more 0.
pub fn base_refund(shots: u32) -> i32 {
    match shots {
        0 | 1 => 3,
        2 => 2,
        3 => 1,
        _ => 0,
    }
}

/// Refund after Quartermaster (+1 to every tier, including 4+) and Forced
/// March (x2). Applied in that order. The multiplied value is truncated
/// toward zero back to whole shells.
pub fn refund(shots: u32, lo: &Loadout) -> i32 {
    let base = base_refund(shots) + lo.refund_bonus;
    (base as f64 * lo.refund_mult) as i32
}

/// Why a run ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunOver {
    /// The reserve reached zero. The only way a run ends (DESIGN §1: no
    /// death, no level select).
    OutOfShells,
}

/// What the run is waiting for: the state of the run state machine.
///
/// Each variant names the one call that moves the run on, so the
/// presentation layer can drive the run by matching on it. See the module
/// docs for the transition table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Waiting for a shot on the current screen.
    Firing,
    /// The last shot cleared the screen. The refund has not been paid and the
    /// run has not moved on yet — `RunState::resolve_clear` does both.
    ///
    /// This exists for the presentation layer: a collapse animates, and the
    /// screen it animates on must still be the screen that was cleared. The
    /// sim does not wait on anything, so headless callers (replay, tests, the
    /// batch harness) simply call `resolve_clear` straight away.
    ///
    /// Splitting the clear into two calls costs nothing in determinism: no
    /// logged input can take effect in between (`fire` refuses outside
    /// `Firing`, `reposition` after the screen's first shot), so the log
    /// needs no entry for the pause; a fuse set meanwhile is reset when the
    /// next screen is entered.
    Cleared,
    /// A draft is open; `RunState::take_offer` must be called.
    Drafting,
    /// The run has ended. Terminal: no call changes the phase again.
    Over(RunOver),
}

/// One entry in the shot log. DESIGN §3 and §9.2: the run seed and width plus
/// the log reconstruct a run.
///
/// Besides the aim, a shot's outcome depends on the Airburst fuse and the
/// Enfilade emplacement offset, so both are recorded as they stood when the
/// shot was fired. Both are integers — the fuse in tenths of a unit, the
/// offset in whole units — and the live game only ever holds values on that
/// grid (`RunState::set_fuse`, `RunState::reposition`), so a replay applies
/// bit-identical floats. They are recorded on every shot, whether or not the
/// loadout uses them; a draft entry leaves both at 0.
///
/// A draft pick is stored in the same struct, marked by `angle_ticks ==
/// replay::DRAFT_MARK`, with the offer index in `power_ticks`; see
/// [`crate::replay`] for why picks belong in the log at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LoggedShot {
    /// Screen the entry was made on. For a draft pick, the screen that was
    /// just cleared, since the run has not advanced when the pick is made.
    pub screen_index: u32,
    /// Angle in 0.05-degree ticks, after quantisation to the loadout's
    /// angle grid and clamping to `0..=MAX_ANGLE_TICKS`. `DRAFT_MARK` for a
    /// draft pick.
    pub angle_ticks: i32,
    /// Power ticks, `0..=POWER_TICKS_MAX`, after clamping. The offer index
    /// for a draft pick.
    pub power_ticks: i32,
    /// Airburst fuse, in tenths of a unit from the muzzle.
    pub fuse_tenths: i32,
    /// Enfilade offset of the emplacement from `EMPLACEMENT_X`, whole units.
    pub emplacement_offset: i32,
}

/// Fuse resolution: the fuse is held and logged in tenths of a unit.
pub const FUSE_STEPS_PER_UNIT: i32 = 10;

/// The run state machine. DESIGN §1.
///
/// Everything about a run in progress: economy, loadout, the current screen
/// and the log of every decision so far. [`Phase`] says which call is legal
/// next; the others refuse and return `None` or `false` rather than panic,
/// so a replay of a broken log fails cleanly.
///
/// Only the current screen is held. The next is generated on arrival from
/// the seed, its index and the loadout at that moment, so the state never
/// grows with the length of the run, apart from the log.
///
/// Fields are public for the presentation layer and tests to read. Writing
/// them directly bypasses the rules; the methods are the supported way to
/// change a run, and only their effects are recorded in the log.
#[derive(Clone, Debug)]
pub struct RunState {
    /// The run's identity, shareable as text via [`format_seed`].
    pub run_seed: u64,
    /// Buffer width the run is generated at. Part of the run's identity: the
    /// same seed at another width is a different run (DESIGN §9.2).
    pub width: usize,
    /// Shell reserve. Never above `loadout.cap()`; 0 ends the run.
    pub shells: i32,
    /// Index of the current screen, from 0.
    pub screen_index: u32,
    /// Screens cleared, supply screens included. The score (DESIGN §1).
    pub screens_cleared: u32,
    /// Shots fired over the whole run.
    pub total_shots: u32,
    /// Shots fired on the current screen, which sets the refund tier.
    pub shots_on_screen: u32,
    /// Consecutive screens cleared in one shot, paid by Standing Orders.
    pub one_shot_streak: u32,
    /// Modifiers held. Changed only by draft picks.
    pub loadout: Loadout,
    /// The screen being played.
    pub screen: Screen,
    /// Where the run is in the state machine.
    pub phase: Phase,
    /// Offers on the table while `phase == Drafting`.
    pub offers: Vec<Modifier>,
    /// Airburst fuse, in units from the muzzle. Player-set, always a whole
    /// number of tenths (see `set_fuse`); `fire` re-quantises it anyway, so a
    /// direct write cannot put an unloggable value into a shot.
    pub fuse: f64,
    /// Enfilade offset applied to this screen, if any. Always a whole number
    /// of units; only `reposition` should change it.
    pub emplacement_offset: f64,
    /// Every shot and draft pick so far, in order. With `run_seed` and
    /// `width`, the whole persisted run ([`crate::replay::Replay`]).
    pub log: Vec<LoggedShot>,
    /// Last landing points, for Chalk Marks.
    pub chalk: Vec<Vec2>,
    /// Apex of the last shot on this screen, for Plumb Line.
    pub last_apex: Option<Vec2>,
    /// Wind stays hidden until the first shot lands, under Dead Reckoning.
    pub wind_revealed: bool,
}

impl RunState {
    /// A fresh run: screen 0, `START_SHELLS` shells, an empty loadout and
    /// log, in `Phase::Firing`. `width` is used as given; the game clamps it
    /// with [`buffer_width`] first.
    pub fn new(run_seed: u64, width: usize) -> Self {
        let loadout = Loadout::new();
        let screen = generate_screen(run_seed, 0, width, &loadout);
        let mut s = Self {
            run_seed,
            width,
            shells: START_SHELLS,
            screen_index: 0,
            screens_cleared: 0,
            total_shots: 0,
            shots_on_screen: 0,
            one_shot_streak: 0,
            loadout,
            screen,
            phase: Phase::Firing,
            offers: Vec::new(),
            fuse: 0.0,
            emplacement_offset: 0.0,
            log: Vec::new(),
            chalk: Vec::new(),
            last_apex: None,
            wind_revealed: false,
        };
        s.on_screen_entered();
        s
    }

    /// Score is screens cleared, tiebreak on shells remaining then total shots
    /// (DESIGN §1). Never shown during a run.
    pub fn score(&self) -> (u32, i32, u32) {
        (self.screens_cleared, self.shells, self.total_shots)
    }

    /// Whether the run has ended (`Phase::Over`).
    pub fn is_over(&self) -> bool {
        matches!(self.phase, Phase::Over(_))
    }

    /// Wind as the HUD should show it, or `None` while Dead Reckoning hides it.
    pub fn displayed_wind(&self) -> Option<f64> {
        if self.loadout.dead_reckoning() && !self.wind_revealed {
            None
        } else if self.loadout.has_flag(crate::modifiers::flags::ANEMOMETER) {
            Some(self.screen.wind)
        } else {
            // Rounded to a whole number without the instrument.
            Some(crate::dmath::round(self.screen.wind))
        }
    }

    /// Per-screen reset. The emplacement returns home and the fuse starts at
    /// the first target's distance, snapped to the logged grid; neither
    /// needs a log entry, because a replay performs the same reset.
    fn on_screen_entered(&mut self) {
        self.shots_on_screen = 0;
        self.chalk.clear();
        self.last_apex = None;
        self.wind_revealed = !self.loadout.dead_reckoning();
        self.emplacement_offset = 0.0;
        self.screen.muzzle_x = EMPLACEMENT_X;
        self.set_fuse(self.screen.default_fuse(0).unwrap_or(0.0));
        if self.screen.supply {
            self.shells = (self.shells + 6).min(self.loadout.cap());
        }
    }

    /// Enfilade: reposition the emplacement before the first shot of a screen.
    /// Refuses once a shot has been taken (DESIGN §4).
    ///
    /// The offset is rounded to whole units, then clamped to the modifier's
    /// range and to keep the emplacement between x = 14 and mid-screen, so
    /// the muzzle always sits on a value the shot log can record exactly.
    ///
    /// Returns whether the move was allowed: `false` without Enfilade or
    /// after the first shot, `true` otherwise, even if the offset was clamped.
    /// Not logged by itself: the offset in effect is recorded on the next
    /// shot's [`LoggedShot::emplacement_offset`].
    pub fn reposition(&mut self, offset: f64) -> bool {
        let limit = self.loadout.enfilade_units;
        if limit <= 0.0 || self.shots_on_screen > 0 {
            return false;
        }
        self.apply_offset(crate::dmath::round(offset) as i32);
        true
    }

    /// Move the emplacement to a whole-unit offset, clamped. The caller has
    /// already checked that Enfilade allows it.
    fn apply_offset(&mut self, offset: i32) {
        let limit = crate::dmath::floor(self.loadout.enfilade_units) as i32;
        let lo = limit.min(EMPLACEMENT_X as i32 - 14);
        let hi = limit.min(self.width as i32 / 2 - EMPLACEMENT_X as i32);
        let off = offset.clamp(-lo, hi.max(-lo));
        self.emplacement_offset = off as f64;
        self.screen.muzzle_x = EMPLACEMENT_X + off as f64;
    }

    /// The emplacement offset as the shot log records it.
    pub fn offset_units(&self) -> i32 {
        self.emplacement_offset as i32
    }

    /// Set the Airburst fuse, in units. Rounded to the nearest tenth and
    /// clamped to 0..=1.5 screen widths.
    pub fn set_fuse(&mut self, units: f64) {
        // `as` saturates and maps NaN to 0, so any input lands on the grid.
        self.set_fuse_tenths(crate::dmath::round(units * FUSE_STEPS_PER_UNIT as f64) as i32);
    }

    /// Set the fuse directly in tenths of a unit, clamped like `set_fuse`.
    /// Replay uses this, so a logged value reaches the same float the live
    /// run held: both are `tenths as f64 / 10.0`.
    pub fn set_fuse_tenths(&mut self, tenths: i32) {
        let max = self.width as i32 * FUSE_STEPS_PER_UNIT * 3 / 2;
        self.fuse = tenths.clamp(0, max) as f64 / FUSE_STEPS_PER_UNIT as f64;
    }

    /// The fuse as the shot log records it.
    pub fn fuse_tenths(&self) -> i32 {
        crate::dmath::round(self.fuse * FUSE_STEPS_PER_UNIT as f64) as i32
    }

    /// Fire. Consumes a shell, applies the shot, advances the run.
    ///
    /// The shot is quantised to the loadout's angle grid and clamped to the
    /// input ranges first, and the logged entry records the quantised values
    /// with the fuse and offset in effect, so the log holds exactly what
    /// flew. The entry is appended before the shot resolves.
    ///
    /// Afterwards the phase is `Cleared` if every target is down (checked
    /// first, so a clear on the last shell still pays its refund), else
    /// `Over` if the reserve is empty, else `Firing`. Returns `None`, and
    /// changes nothing, outside `Phase::Firing` or with no shells left.
    pub fn fire(&mut self, shot: Shot, record_path: bool) -> Option<ShotReport> {
        if self.phase != Phase::Firing || self.shells <= 0 {
            return None;
        }
        let shot = shot.quantized(&self.loadout);
        // Snap the fuse to the logged grid, in case it was written directly.
        self.set_fuse_tenths(self.fuse_tenths());
        self.shells -= 1;
        self.total_shots += 1;
        self.shots_on_screen += 1;
        self.log.push(LoggedShot {
            screen_index: self.screen_index,
            angle_ticks: shot.angle_ticks,
            power_ticks: shot.power_ticks,
            fuse_tenths: self.fuse_tenths(),
            emplacement_offset: self.offset_units(),
        });

        let fuse = Some(self.fuse);
        let report = self
            .screen
            .apply_shot(shot, &self.loadout, fuse, record_path);

        self.wind_revealed = true;
        if let Some(c) = report.chalk {
            if self.loadout.chalk_marks > 0 {
                self.chalk.push(c);
                let keep = self.loadout.chalk_marks as usize;
                while self.chalk.len() > keep {
                    self.chalk.remove(0);
                }
            }
        }
        self.last_apex = Some(report.apex);

        if report.salvage {
            self.grant(self.loadout.salvage_shells);
        }

        if report.cleared {
            self.phase = Phase::Cleared;
        } else if self.shells <= 0 {
            self.phase = Phase::Over(RunOver::OutOfShells);
        }
        Some(report)
    }

    fn grant(&mut self, n: i32) {
        self.shells = (self.shells + n).min(self.loadout.cap());
    }

    /// Pay for the cleared screen and move on. Idempotent: returns false if
    /// the run is not sitting on a cleared screen.
    ///
    /// Leaves the run in `Drafting` at the end of a band, otherwise on the
    /// next screen in `Firing`, or `Over` if the refund left no shells. The
    /// Godot layer calls this once its collapse animation and camera pan
    /// have finished; headless callers call it straight after every `fire`.
    pub fn resolve_clear(&mut self) -> bool {
        if self.phase != Phase::Cleared {
            return false;
        }
        self.clear_screen();
        true
    }

    fn clear_screen(&mut self) {
        let shots = self.shots_on_screen;
        self.screens_cleared += 1;

        let mut gain = refund(shots, &self.loadout) + self.loadout.shells_per_clear;
        if shots <= 1 {
            self.one_shot_streak += 1;
            if self.loadout.standing_orders() {
                gain += self.one_shot_streak as i32;
            }
        } else {
            self.one_shot_streak = 0;
        }
        self.grant(gain);
        self.leave_screen();
    }

    /// Leave a cleared screen: open the band-end draft if the next screen
    /// starts a band, otherwise advance to it.
    ///
    /// A draft fires at the end of each band (DESIGN §1). The offers come
    /// from the next screen's seed and the loadout, not from a running
    /// generator, so they are fixed by what the log already contains and a
    /// replay meets the same offers.
    fn leave_screen(&mut self) {
        let next = self.screen_index + 1;
        if next.is_multiple_of(BAND_LEN) {
            let offers = draft_offers(screen_seed(self.run_seed, next), &self.loadout);
            if !offers.is_empty() {
                self.offers = offers;
                self.phase = Phase::Drafting;
                return;
            }
        }
        self.advance();
    }

    /// Take one of the three offers. The other two are gone.
    ///
    /// `which` indexes [`RunState::offers`]; there can be fewer than three
    /// once most modifiers are at their stack limit. Logs the pick, applies
    /// the modifier (including any instant shells and a lowered cap), then
    /// advances to the next screen. Returns the modifier taken, or `None`,
    /// changing nothing, outside `Phase::Drafting` or for an index with no
    /// offer.
    pub fn take_offer(&mut self, which: usize) -> Option<Modifier> {
        if self.phase != Phase::Drafting {
            return None;
        }
        let m = *self.offers.get(which)?;
        // Draft picks go in the shot log too, or a replay cannot rebuild the
        // loadout. See replay::DRAFT_MARK.
        self.log.push(LoggedShot {
            screen_index: self.screen_index,
            angle_ticks: crate::replay::DRAFT_MARK,
            power_ticks: which as i32,
            fuse_tenths: 0,
            emplacement_offset: 0,
        });
        let instant = self.loadout.take(m);
        self.offers.clear();
        if instant > 0 {
            self.grant(instant);
        }
        // A new cap may already have been exceeded (Forced March).
        self.shells = self.shells.min(self.loadout.cap());
        self.advance();
        Some(m)
    }

    /// Skipping is allowed only when the pool is exhausted.
    ///
    /// A safety valve rather than a normal path: `resolve_clear` opens a
    /// draft only when there is something to offer, so a run does not reach
    /// `Drafting` with no offers. A skip is not logged, which is safe only
    /// because of that.
    pub fn skip_draft(&mut self) {
        if self.phase == Phase::Drafting && self.offers.is_empty() {
            self.advance();
        }
    }

    fn advance(&mut self) {
        self.screen_index += 1;
        self.screen = generate_screen(self.run_seed, self.screen_index, self.width, &self.loadout);
        self.on_screen_entered();
        self.phase = if self.shells <= 0 {
            Phase::Over(RunOver::OutOfShells)
        } else {
            Phase::Firing
        };
        // A supply screen has nothing to shoot at; walk straight through it.
        // It counts as cleared (DESIGN §4) and its +6 shells were granted on
        // entry. No refund is paid, but the band-end draft check is still
        // made, so a supply screen just before a band start (15, 35, …)
        // leaves the run drafting on it.
        if self.phase == Phase::Firing && self.screen.supply {
            self.screens_cleared += 1;
            self.leave_screen();
        }
    }
}

/// A run seed for a calendar day, so every player gets the same daily run.
/// Takes the day number rather than reading the clock — nothing in the sim
/// touches system time.
///
/// `days_since_epoch` counts days since the Unix epoch; the caller converts
/// the date. The constant is the ASCII bytes of "SALTLINE".
pub fn daily_seed(days_since_epoch: u64) -> u64 {
    crate::rng::mix64(0x5341_4C54_4C49_4E45 ^ days_since_epoch)
}

/// Parse a shared seed. Accepts decimal, `0x` hex, or any other text, which is
/// hashed so that every string is a valid seed.
///
/// Surrounding whitespace is ignored. Never fails: seed entry is free text,
/// and a word or phrase is as good a seed as a number. The hash is FNV-1a
/// over the UTF-8 bytes, then [`crate::rng::mix64`] to spread it. Stable
/// across platforms and builds, since a typed seed has to name the same run
/// everywhere; changing it would change which run every word names.
pub fn parse_seed(text: &str) -> u64 {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            return v;
        }
    }
    if let Ok(v) = t.parse::<u64>() {
        return v;
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in t.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    crate::rng::mix64(h)
}

/// Format a seed for sharing: plain decimal, which [`parse_seed`] reads back
/// to the same value.
pub fn format_seed(seed: u64) -> String {
    format!("{seed}")
}

/// Width clamped into the internal-buffer range. DESIGN §5.1.
pub fn buffer_width(requested: usize) -> usize {
    requested.clamp(crate::SCREEN_W_MIN, crate::SCREEN_W_MAX)
}

/// The reference width the DESIGN §2.5 table is tuned against.
pub const fn reference_width() -> usize {
    SCREEN_W_REF
}

/// Minimum surviving candidates a screen must show. Re-exported for the batch
/// harness so it reports against the same number the generator enforces.
pub const fn min_solutions() -> usize {
    MIN_SOLUTIONS
}

/// Shorthand used by tests and the batch harness.
pub fn shot(angle_ticks: i32, power_ticks: i32) -> Shot {
    ballistics::Shot::new(angle_ticks, power_ticks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::screen_hash;
    use crate::solver;
    use crate::terrain::WORLD_FLOOR;

    const SEED: u64 = 0x0B00_1234_5678_9ABC;

    /// Every modifier at its stack limit, alone and in every pair, plus the
    /// empty loadout. Covers the power extremes (Featherweight, Ballast, both
    /// with Sabot), Vernier's quantum, Dead Reckoning, Storm Front, Airburst
    /// and the Overhang unlocks.
    fn loadout_matrix() -> Vec<Loadout> {
        let maxed = |ms: &[Modifier]| {
            let mut lo = Loadout::new();
            for &m in ms {
                for _ in 0..m.max_stacks() {
                    lo.take(m);
                }
            }
            lo
        };
        let all: Vec<Modifier> = Modifier::all().collect();
        let mut out = vec![Loadout::new()];
        for (i, &a) in all.iter().enumerate() {
            out.push(maxed(&[a]));
            for &b in &all[i + 1..] {
                out.push(maxed(&[a, b]));
            }
        }
        out.push(maxed(&[
            Modifier::Featherweight,
            Modifier::Sabot,
            Modifier::StormFront,
            Modifier::DeadReckoning,
        ]));
        out.push(maxed(&[
            Modifier::Ballast,
            Modifier::StormFront,
            Modifier::Vernier,
            Modifier::Airburst,
        ]));
        out
    }

    /// Force the ladder to start at `from` for every loadout in the matrix
    /// and assert the result passes the gate. Returns how many screens came
    /// off each rung.
    fn fallback_is_solvable_from(from: Rung) -> [usize; 4] {
        let mut rungs = [0usize; 4];
        for (n, lo) in loadout_matrix().iter().enumerate() {
            for (k, index) in [0u32, 21, 83, 203, 401].into_iter().enumerate() {
                let seed = SEED ^ ((n * 5 + k) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let width = if (n + k) % 2 == 0 { 320 } else { 400 };
                let s = generate_screen_from(seed, index, width, lo, from);
                let Some(rung) = s.rung else {
                    // Supply screens have no targets: nothing to validate.
                    assert!(s.supply && s.structures.is_empty());
                    continue;
                };
                assert!(rung >= from, "{rung:?} is above the starting rung");
                let r = solver::validate(&s, lo);
                assert!(
                    r.solvable(),
                    "rung {rung:?} unsolvable: loadout {n}, screen {index}, width {width}, \
                     {} candidates",
                    r.worst
                );
                assert_eq!(s.candidates, r.worst_direct);
                rungs[rung as usize] += 1;
            }
        }
        rungs
    }

    #[test]
    fn the_terrain_free_fallback_is_gated_and_solvable_for_every_loadout() {
        // DESIGN §2.4: every screen is proven solvable before it is shown,
        // including the ones generation had to fall back for. Start the
        // ladder at the terrain-free rung so the fallback is exercised
        // whatever the seed would have done.
        let rungs = fallback_is_solvable_from(Rung::Terrainless);
        assert!(rungs[Rung::Terrainless as usize] + rungs[Rung::Level as usize] > 1000);
    }

    #[test]
    fn the_level_fallback_is_solvable_for_every_loadout() {
        // The bottom rung, forced for every loadout in the matrix. This is the
        // one that must never fail.
        let rungs = fallback_is_solvable_from(Rung::Level);
        assert!(rungs[Rung::Level as usize] > 1000);
    }

    #[test]
    fn the_level_fallback_ignores_the_seed_and_the_wind() {
        let mut lo = Loadout::new();
        lo.take(Modifier::StormFront);
        let a = generate_screen_from(1, 83, 320, &lo, Rung::Level);
        let b = generate_screen_from(2, 83, 320, &lo, Rung::Level);
        assert_eq!(a.wind, 0.0);
        assert_eq!(a.wind_late, Some(0.0));
        assert_eq!(a.structures[0].x0, b.structures[0].x0);
        assert_eq!(a.field.base, b.field.base);
        assert!(a.fell_back);
    }

    #[test]
    fn a_failing_terrain_free_screen_drops_to_the_level_rung() {
        // Before the gate, rung 3 was returned unvalidated. No loadout a
        // draft can build has been seen to break it, so this one is doctored:
        // wind forty times over leaves the gentle roll unsolvable on most
        // seeds. Old code shipped such a screen; now it must come off rung 4
        // instead, still proven solvable.
        let mut lo = Loadout::new();
        lo.wind_scale = 40.0;
        let index = 401;
        let run_seed = (0..64u64)
            .find(|&seed| {
                let mut s = assemble(
                    index,
                    screen_seed(seed, index),
                    320,
                    Template::Flat,
                    band_of(index),
                    &lo,
                    true,
                );
                s.structures.truncate(1);
                !solver::validate(&s, &lo).solvable()
            })
            .expect("no seed breaks the terrain-free rung");
        let s = generate_screen_from(run_seed, index, 320, &lo, Rung::Terrainless);
        assert_eq!(s.rung, Some(Rung::Level));
        assert!(solver::validate(&s, &lo).solvable());
    }

    #[test]
    #[should_panic(expected = "unsolvable screen")]
    fn an_unsolvable_last_rung_panics_rather_than_shipping() {
        // A doctored loadout no real draft can produce: the shell barely
        // leaves the barrel, so nothing is in range and every rung fails.
        let mut lo = Loadout::new();
        lo.power_scale = 0.05;
        generate_screen_from(SEED, 21, 320, &lo, Rung::Terrainless);
    }

    // ------------------------------------------------------- M1 acceptance
    #[test]
    fn screen_400_is_generated_without_the_399_screens_before_it() {
        // DESIGN §2.2: each screen generates from its own seed, no sequential
        // state carries forward. Brief M1 acceptance.
        let lo = Loadout::new();
        let direct = generate_screen(SEED, 400, 320, &lo);

        let mut in_sequence = None;
        for i in 0..=400u32 {
            let s = generate_screen(SEED, i, 320, &lo);
            if i == 400 {
                in_sequence = Some(s);
            }
        }
        let in_sequence = in_sequence.unwrap();
        assert_eq!(
            screen_hash(&direct),
            screen_hash(&in_sequence),
            "screen 400 differs when reached in sequence"
        );
        assert_eq!(direct.template, in_sequence.template);
        assert_eq!(direct.wind.to_bits(), in_sequence.wind.to_bits());
    }

    #[test]
    fn generation_is_bit_stable_across_calls() {
        let lo = Loadout::new();
        for index in [0u32, 3, 17, 64, 199] {
            let a = generate_screen(SEED, index, 320, &lo);
            let b = generate_screen(SEED, index, 320, &lo);
            assert_eq!(screen_hash(&a), screen_hash(&b));
        }
    }

    // -------------------------------------------------- placement invariants
    #[test]
    fn structures_are_seated_and_separated() {
        let lo = Loadout::new();
        for seed in 0..30u64 {
            for index in [0u32, 4, 12, 36, 80] {
                let s = generate_screen(seed * 6151 + 7, index, 320, &lo);
                for st in &s.structures {
                    // Nothing floats: the base sits on the highest footprint
                    // column, and nothing hangs off the buffer.
                    assert!(st.x0 >= 1 && st.x0 + st.footprint() < s.width);
                    let seat = s.field.seat(st.x0, st.footprint());
                    assert!((st.base - seat).abs() < 1e-9, "structure not seated");
                    assert!(st.base >= WORLD_FLOOR);
                    assert!(
                        st.base + st.kind.height() <= crate::SCREEN_H as f64,
                        "{} pokes out of the buffer",
                        st.kind.name()
                    );
                    // Never on top of the emplacement.
                    assert!(st.centre_x() > s.muzzle_x + 40.0);
                }
                for (i, a) in s.structures.iter().enumerate() {
                    for b in &s.structures[i + 1..] {
                        assert!(
                            (a.centre_x() - b.centre_x()).abs() >= MIN_TARGET_SEPARATION,
                            "targets closer than the 40-unit minimum"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn target_counts_and_curve_match_the_design_table() {
        // DESIGN §2.5.
        assert_eq!(target_distance(0), 110.0);
        assert_eq!(target_distance(10), 170.0);
        assert_eq!(target_distance(100), 290.0, "distance cap");
        assert_eq!(wind_magnitude(0), 0.0);
        assert_eq!(wind_magnitude(4), 1.4);
        assert_eq!(wind_magnitude(100), 7.0, "wind cap");
        assert_eq!(terrain_amplitude(0), 1.0);
        assert_eq!(terrain_amplitude(100), 2.2, "amplitude cap");
        assert_eq!(target_count(0), 1);
        assert_eq!(target_count(1), 2);
        assert_eq!(target_count(2), 3);
        assert_eq!(target_count(8), 3);
        assert_eq!(target_count(9), 3);
        assert_eq!(target_count(200), 3);
        assert_eq!(elevation_delta_limit(0), 8.0);
        assert_eq!(elevation_delta_limit(100), 70.0, "elevation cap");
    }

    #[test]
    fn wind_stays_inside_the_band_envelope() {
        let lo = Loadout::new();
        for index in [0u32, 4, 8, 40, 120, 400] {
            let band = band_of(index);
            for seed in 0..20u64 {
                let s = generate_screen(seed * 977 + 1, index, 320, &lo);
                assert!(
                    s.wind.abs() <= wind_magnitude(band) + 1e-9,
                    "screen {index} wind {} exceeds band envelope",
                    s.wind
                );
            }
        }
        // Band 0 is dead calm, so the first screens teach the arc alone.
        let s = generate_screen(1, 0, 320, &lo);
        assert_eq!(s.wind, 0.0);
    }

    #[test]
    fn late_bands_ramp_structure_not_magnitude() {
        // DESIGN §2.5: past band 20, stop ramping magnitude and ramp template
        // weight toward Spires / Overhang / Basin instead.
        let mut lo = Loadout::new();
        lo.take(Modifier::Airburst);
        let early = template_weights(20, &lo);
        let late = template_weights(60, &lo);
        for t in [Template::Spires, Template::Overhang, Template::Basin] {
            assert!(
                late[t as usize] > early[t as usize],
                "{} did not gain weight late",
                t.name()
            );
        }
        assert!(late[Template::Flat as usize] <= early[Template::Flat as usize]);
        assert_eq!(
            wind_magnitude(20),
            wind_magnitude(60),
            "magnitude still ramping"
        );
    }

    #[test]
    fn templates_are_gated_by_band() {
        let lo = Loadout::new();
        for band in 0..8u32 {
            let w = template_weights(band, &lo);
            for t in Template::ALL {
                if band < t.min_band() {
                    assert_eq!(w[t as usize], 0, "{} offered too early", t.name());
                }
            }
        }
    }

    #[test]
    fn opening_encounters_feature_distinct_terrain_and_remain_solvable() {
        let lo = Loadout::new();
        for seed in 0..64 {
            for width in [320, 400] {
                for (index, expected) in [(2, Template::Ridge), (5, Template::Chasm)] {
                    let screen = generate_screen(seed, index, width, &lo);
                    assert_eq!(screen.template, expected, "seed {seed}, screen {index}");
                    let kind = if index == 2 {
                        StructureKind::RadioMast
                    } else {
                        StructureKind::Silo
                    };
                    assert_eq!(screen.structures[0].kind, kind);
                    assert!(!screen.fell_back, "seed {seed}, screen {index}");
                    assert!(screen.candidates >= MIN_SOLUTIONS);
                    if index == 2 {
                        let muzzle = screen.muzzle();
                        let target = solver::aim_points(&screen, 0).unwrap()[0];
                        let blocked = ((muzzle.x as usize + 3)..target.x as usize).any(|x| {
                            let y = muzzle.y
                                + (target.y - muzzle.y) * (x as f64 - muzzle.x)
                                    / (target.x - muzzle.x);
                            screen.field.surface_at(x as f64) > y
                        });
                        assert!(blocked, "ridge leaves a clear sightline for seed {seed}");
                    }
                }
            }
        }
    }

    #[test]
    fn target_count_mix_overlaps_progression_tiers() {
        assert_eq!(target_count_weights(0), [100, 0, 0]);
        assert_eq!(target_count_weights(1), [65, 35, 0]);
        assert_eq!(target_count_weights(2), [40, 55, 5]);
        assert!(target_count_weights(4)[2] > 0);
        assert!(target_count_weights(6)[2] > target_count_weights(4)[2]);
        assert!(target_count_weights(20)[2] > target_count_weights(20)[1]);
    }

    #[test]
    fn eligible_overhang_has_signature_weight() {
        let plain = Loadout::new();
        assert_eq!(template_weights(8, &plain)[Template::Overhang as usize], 0);
        let mut unlocked = Loadout::new();
        unlocked.take(Modifier::Airburst);
        let weights = template_weights(8, &unlocked);
        assert!(
            weights[Template::Overhang as usize] >= weights[Template::Basin as usize],
            "eligible Overhang should be common enough to exercise its build"
        );
    }

    // ------------------------------------------------- M3 acceptance: DEMO
    #[test]
    fn undermining_alone_collapses_a_structure_with_no_direct_hit() {
        // Brief M3 acceptance. Crater only the footprint; never touch the
        // structure's bounding box. DESIGN §2.7 says this must be a kill path.
        let lo = Loadout::new();
        let mut screen = generate_screen(SEED, 0, 320, &lo);
        assert!(!screen.structures.is_empty());
        let st = screen.structures[0].clone();
        let box_ = st.aabb();
        let integrity_before = st.integrity;

        for _ in 0..12 {
            for i in st.x0..st.x0 + st.footprint() {
                screen.field.crater(i as f64, 6.0);
            }
            screen.resolve_structures();
            if screen.structures[0].state == StructState::Collapsed {
                break;
            }
        }

        assert_eq!(
            screen.structures[0].state,
            StructState::Collapsed,
            "support {} never reached the collapse threshold",
            screen.structures[0].support
        );
        assert_eq!(
            screen.structures[0].integrity, integrity_before,
            "integrity changed — that would be a direct hit, not undermining"
        );
        assert!(box_.y0 >= WORLD_FLOOR);
    }

    #[test]
    fn collapse_is_permanent_when_the_footprint_is_refilled() {
        // Debris can pile back over a felled structure's footprint. The
        // wreck must stay down, or a cleared screen could un-clear.
        let lo = Loadout::new();
        let mut screen = generate_screen(SEED, 0, 320, &lo);
        let st = screen.structures[0].clone();
        for _ in 0..12 {
            for i in st.x0..st.x0 + st.footprint() {
                screen.field.crater(i as f64, 6.0);
            }
            screen.resolve_structures();
            if screen.structures[0].is_down() {
                break;
            }
        }
        assert!(
            screen.structures[0].is_down(),
            "undermining never felled it"
        );
        let lean = screen.structures[0].lean;

        screen
            .field
            .mound(st.centre_x(), st.footprint() as f64 * 2.0, 60.0);
        screen.resolve_structures();
        let s = &screen.structures[0];
        assert_eq!(s.state, StructState::Collapsed, "support {}", s.support);
        assert_eq!(s.lean, lean, "a wreck does not change the way it fell");
    }

    #[test]
    fn leaning_is_a_real_state_that_moves_the_box() {
        let lo = Loadout::new();
        let mut screen = generate_screen(0xBEEF, 0, 320, &lo);
        let st = screen.structures[0].clone();
        let upright = st.aabb();
        // Excavate one side of the footprint only, so it leans rather than falls.
        for _ in 0..10 {
            for i in st.x0..st.x0 + st.footprint() / 2 {
                screen.field.crater(i as f64, 4.0);
            }
            screen.resolve_structures();
            if screen.structures[0].state != StructState::Intact {
                break;
            }
        }
        let s = &screen.structures[0];
        assert_eq!(
            s.state,
            StructState::Leaning,
            "one-sided undermining should lean before it fells; support {}",
            s.support
        );
        assert_ne!(s.lean, 0, "a leaning structure must pick a side");
        assert_eq!(s.lean, -1, "it should tilt toward the undermined side");
        let leaned = s.aabb();
        assert!(
            (leaned.x0 - upright.x0).abs() >= LEAN_SHIFT - 2.0
                || (leaned.x1 - upright.x1).abs() >= LEAN_SHIFT - 2.0,
            "the bounding box did not move"
        );
        // Leaning grants no refund: the screen is not cleared.
        assert!(!screen.cleared());
    }

    #[test]
    fn a_direct_hit_can_clear_a_screen_in_one_shot() {
        // DESIGN §1 pays +3 for a one-shot clear, so one shot has to be able
        // to do it. The solver's candidates are exactly those shots.
        let lo = Loadout::new();
        let mut screen = generate_screen(SEED, 0, 320, &lo);
        let cands = solver::solve_target(&screen, &lo, 0);
        assert!(cands.len() >= MIN_SOLUTIONS);
        let report = screen.apply_shot(cands[0].shot, &lo, screen.default_fuse(0), false);
        assert!(report.cleared, "a solver candidate failed to clear");
        assert_eq!(report.collapsed, vec![0]);
    }

    #[test]
    fn collapse_leaves_debris_behind() {
        // DESIGN §2.7: terrain gets slightly higher where a structure fell.
        let lo = Loadout::new();
        let mut screen = generate_screen(SEED, 0, 320, &lo);
        let st = screen.structures[0].clone();
        let cands = solver::solve_target(&screen, &lo, 0);
        let before: Vec<f64> = (0..screen.width).map(|i| screen.field.surface(i)).collect();
        screen.apply_shot(cands[0].shot, &lo, screen.default_fuse(0), false);
        assert!(screen.structures[0].is_down());
        let lo_col = st.x0.saturating_sub(30);
        let hi_col = (st.x0 + st.footprint() + 30).min(screen.width);
        let risen = (lo_col..hi_col).any(|i| screen.field.surface(i) > before[i] + 0.5);
        assert!(risen, "no debris mound anywhere near the collapse");
    }

    #[test]
    fn a_direct_hit_clears_old_rubble_without_reviving_the_target() {
        let lo = Loadout::new();
        let mut screen = (3..20)
            .map(|index| generate_screen(SEED, index, 320, &lo))
            .find(|screen| screen.structures.len() > 1)
            .expect("a multi-target screen");
        assert_eq!(screen.structures.len(), 2);
        screen.structures[0].state = StructState::Collapsed;
        let rubble = screen.target_boxes()[0];
        let hit = rubble.centre();

        screen.detonate(End::Structure { index: 0, pos: hit }, 1.0, &lo);
        assert_eq!(screen.structures[0].state, StructState::Cleared);
        assert!(screen.structures[0].is_down());
        assert!(!screen.cleared(), "the other target still stands");
        assert_eq!(screen.structures[1].state, StructState::Intact);

        let boxes = screen.target_boxes();
        assert_eq!(boxes.len(), 2, "ballistic indices must stay stable");
        assert!(!crate::ballistics::segment_hits_aabb(
            Vec2::new(rubble.x0 - 10.0, hit.y),
            Vec2::new(rubble.x1 + 10.0, hit.y),
            boxes[0].expand(crate::SHELL_RADIUS),
        ));
        let integrity = screen.structures[0].integrity;
        screen.detonate(
            End::Terrain {
                pos: hit,
                material: Material::Sand,
            },
            1.0,
            &lo,
        );
        assert_eq!(screen.structures[0].state, StructState::Cleared);
        assert_eq!(screen.structures[0].integrity, integrity);
    }

    #[test]
    fn a_second_shell_passes_through_a_cleared_wreck() {
        let lo = Loadout::new();
        let mut screen = generate_screen(SEED, 0, 320, &lo);
        screen.structures[0].state = StructState::Collapsed;
        let shot = shot(0, 820);

        let first = screen.apply_shot(shot, &lo, None, false);
        assert!(matches!(
            first.traces[0].end,
            End::Structure { index: 0, .. }
        ));
        assert_eq!(screen.structures[0].state, StructState::Cleared);
        let second = screen.apply_shot(shot, &lo, None, false);
        assert!(!matches!(
            second.traces[0].end,
            End::Structure { index: 0, .. }
        ));
    }

    #[test]
    fn thermal_lance_collapses_rather_than_chips() {
        let plain = Loadout::new();
        let mut lance = Loadout::new();
        lance.take(Modifier::ThermalLance);
        // A glancing blast that only leans a structure should fell it outright
        // once Thermal Lance is held.
        let mut chipped: u32 = 0;
        let mut felled: u32 = 0;
        for seed in 0..12u64 {
            let base = generate_screen(seed * 31 + 5, 0, 320, &plain);
            let st = base.structures[0].clone();
            let near = Vec2::new(st.aabb().x0 - 5.0, st.base + 4.0);
            for (lo, counter) in [
                (&plain, &mut chipped as &mut u32),
                (&lance, &mut felled as &mut u32),
            ] {
                let mut s = base.clone();
                s.detonate(
                    End::Terrain {
                        pos: near,
                        material: Material::Sand,
                    },
                    1.0,
                    lo,
                );
                s.resolve_structures();
                if s.structures[0].is_down() {
                    *counter += 1;
                }
            }
        }
        assert!(
            felled > chipped,
            "Thermal Lance felled {felled}, plain felled {chipped}"
        );
    }

    // ------------------------------------------------------- economy (§1)
    #[test]
    fn the_refund_table_matches_the_design() {
        let lo = Loadout::new();
        assert_eq!(refund(1, &lo), 3);
        assert_eq!(refund(2, &lo), 2);
        assert_eq!(refund(3, &lo), 1);
        assert_eq!(refund(4, &lo), 0);
        assert_eq!(refund(9, &lo), 0);
    }

    #[test]
    fn quartermaster_lifts_every_tier_including_the_last() {
        let mut lo = Loadout::new();
        lo.take(Modifier::Quartermaster);
        assert_eq!(refund(1, &lo), 4);
        assert_eq!(refund(4, &lo), 1, "the 4+ tier must move too");
        let mut both = lo.clone();
        both.take(Modifier::ForcedMarch);
        assert_eq!(refund(1, &both), 8);
        assert_eq!(refund(4, &both), 2);
    }

    #[test]
    fn qualifying_salvage_craters_refund_one_shell_per_stack() {
        let screen = generate_screen(0x005A_1A6E, 0, 320, &Loadout::new());
        let target_x = screen.structures[0].centre_x();
        let near = Vec2::new(target_x + 20.0, screen.field.surface_at(target_x + 20.0));
        let far = Vec2::new(target_x + 31.0, screen.field.surface_at(target_x + 31.0));

        let mut salvage = Loadout::new();
        salvage.take(Modifier::Salvage);
        assert!(screen.salvage_qualifies(near, &salvage));
        assert!(!screen.salvage_qualifies(far, &salvage));
        assert_eq!(salvage.salvage_shells, 1);

        salvage.take(Modifier::Salvage);
        assert!(screen.salvage_qualifies(near, &salvage));
        assert_eq!(salvage.salvage_shells, 2);
    }

    #[test]
    fn net_drain_is_negative_for_a_perfect_player() {
        // DESIGN §1: a perfect player still trends slowly downward without
        // economy modifiers. One shot costs 1 and refunds 3 — but a screen
        // with three targets costs at least three shots.
        let lo = Loadout::new();
        let per_screen = |targets: u32| -> i32 {
            let shots = targets as i32;
            refund(targets, &lo) - shots
        };
        assert_eq!(per_screen(1), 2, "single-target screens are net positive");
        assert_eq!(per_screen(2), 0);
        assert_eq!(per_screen(3), -2, "three-target screens drain");
    }

    #[test]
    fn a_run_starts_with_forty_shells_and_ends_when_they_run_out() {
        let mut state = RunState::new(9, 320);
        assert_eq!(state.shells, START_SHELLS);
        assert_eq!(state.screen_index, 0);
        // Fire deliberately wide until the reserve empties.
        for _ in 0..600 {
            if state.is_over() {
                break;
            }
            match state.phase {
                Phase::Firing => {
                    state.fire(shot(40, 30), false);
                }
                Phase::Cleared => {
                    state.resolve_clear();
                }
                Phase::Drafting => {
                    state.take_offer(0);
                }
                Phase::Over(_) => break,
            }
        }
        assert_eq!(state.phase, Phase::Over(RunOver::OutOfShells));
        assert!(state.shells <= 0);
        assert!(state.total_shots >= START_SHELLS as u32);
    }

    #[test]
    fn a_draft_fires_at_the_end_of_every_band() {
        let mut state = RunState::new(0x5EED, 320);
        let lo = state.loadout.clone();
        // Clear four screens with solver candidates, then expect a draft.
        for expected_index in 0..BAND_LEN {
            assert_eq!(state.screen_index, expected_index);
            clear_current_screen(&mut state, &lo);
        }
        assert_eq!(state.phase, Phase::Drafting);
        assert_eq!(state.offers.len(), 3, "three offers, take one");
        let taken = state.take_offer(1).unwrap();
        assert_eq!(state.loadout.stacks(taken), 1);
        assert_eq!(state.phase, Phase::Firing);
        assert_eq!(state.screen_index, BAND_LEN);
    }

    /// Clear whatever screen the run is on, using solver candidates.
    fn clear_current_screen(state: &mut RunState, _lo: &Loadout) {
        let start = state.screen_index;
        for _ in 0..8 {
            if state.screen_index != start || state.phase != Phase::Firing {
                break;
            }
            let lo = state.loadout.clone();
            let Some(target) = state.screen.structures.iter().position(|s| !s.is_down()) else {
                break;
            };
            let cands = solver::solve_target(&state.screen, &lo, target);
            assert!(!cands.is_empty(), "no candidate for a standing target");
            state.fuse = state.screen.default_fuse(target).unwrap_or(0.0);
            state.fire(cands[0].shot, false);
            state.resolve_clear();
            if state.phase != Phase::Firing {
                break;
            }
        }
    }

    #[test]
    fn one_shot_clears_pay_the_top_tier() {
        let mut state = RunState::new(0xC0FFEE, 320);
        let lo = state.loadout.clone();
        let before = state.shells;
        let cands = solver::solve_target(&state.screen, &lo, 0);
        state.fire(cands[0].shot, false);
        assert_eq!(state.phase, Phase::Cleared, "the clear is not paid yet");
        assert_eq!(state.screens_cleared, 0);
        state.resolve_clear();
        assert_eq!(state.screens_cleared, 1);
        // -1 for the shell fired, +3 for the one-shot clear.
        assert_eq!(state.shells, before - 1 + 3);
        assert_eq!(state.one_shot_streak, 1);
    }

    #[test]
    fn standing_orders_pays_the_streak_and_a_miss_resets_it() {
        let mut state = RunState::new(0xC0FFEE, 320);
        state.loadout.take(Modifier::StandingOrders);
        let lo = state.loadout.clone();
        let cands = solver::solve_target(&state.screen, &lo, 0);
        let before = state.shells;
        state.fire(cands[0].shot, false);
        state.resolve_clear();
        // -1 shell, +3 refund, +1 for the first streak step.
        assert_eq!(state.shells, before - 1 + 3 + 1);
        assert_eq!(state.one_shot_streak, 1);
        // A miss on the next screen resets it.
        state.fire(shot(20, 20), false);
        assert_eq!(state.one_shot_streak, 1, "still standing until the clear");
        let lo = state.loadout.clone();
        let target = state
            .screen
            .structures
            .iter()
            .position(|s| !s.is_down())
            .unwrap();
        let c = solver::solve_target(&state.screen, &lo, target);
        state.fire(c[0].shot, false);
        state.resolve_clear();
        assert_eq!(
            state.one_shot_streak, 0,
            "two shots is not a one-shot clear"
        );
    }

    #[test]
    fn forced_march_clamps_the_reserve_the_moment_it_is_taken() {
        let mut state = RunState::new(11, 320);
        assert_eq!(state.shells, 40);
        state.phase = Phase::Drafting;
        state.offers = vec![Modifier::ForcedMarch];
        state.take_offer(0);
        assert_eq!(state.shells, 15, "reserve cap applies immediately");
    }

    #[test]
    fn depot_grants_its_shells_once_on_take() {
        let mut state = RunState::new(12, 320);
        state.shells = 10;
        state.phase = Phase::Drafting;
        state.offers = vec![Modifier::Depot];
        state.take_offer(0);
        assert_eq!(state.shells, 18);
    }

    #[test]
    fn scavenger_rites_inserts_supply_screens() {
        let mut lo = Loadout::new();
        lo.take(Modifier::ScavengerRites);
        let supply = generate_screen(7, 5, 320, &lo);
        assert!(supply.supply);
        assert!(supply.structures.is_empty());
        assert!(supply.cleared(), "a supply screen has nothing to shoot");
        assert!(solver::validate(&supply, &lo).solvable());
        let normal = generate_screen(7, 6, 320, &lo);
        assert!(!normal.supply);
        // And they do not appear without the modifier.
        assert!(!generate_screen(7, 5, 320, &Loadout::new()).supply);
    }

    /// Supply screens fall on multiples of 5, so 15, 35, … sit just before a
    /// band start. Walking through one must still open that band's draft.
    #[test]
    fn supply_screens_before_a_band_start_still_open_its_draft() {
        let mut state = RunState::new(0x5CA7, 320);
        state.loadout.take(Modifier::ScavengerRites);
        let mut drafts = Vec::new();
        while state.screen_index < 40 {
            assert!(!state.is_over(), "ran dry at screen {}", state.screen_index);
            if state.phase == Phase::Drafting {
                drafts.push(state.screen_index + 1);
                state.take_offer(0);
            } else {
                clear_current_screen(&mut state, &Loadout::new());
            }
        }
        let bands: Vec<u32> = (1..=10).map(|b| b * BAND_LEN).collect();
        assert_eq!(drafts, bands, "every band start opens its draft");
    }

    #[test]
    fn dead_reckoning_hides_the_wind_until_the_first_shot() {
        let mut state = RunState::new(0xDEAD_0001, 320);
        state.loadout.take(Modifier::DeadReckoning);
        state.wind_revealed = false;
        assert!(state.displayed_wind().is_none());
        state.fire(shot(800, 500), false);
        assert!(
            state.displayed_wind().is_some(),
            "wind stays hidden after firing"
        );
    }

    #[test]
    fn anemometer_stops_rounding_the_wind() {
        let mut state = RunState::new(0xA1, 320);
        state.screen.wind = 2.44;
        assert_eq!(state.displayed_wind(), Some(2.0));
        state.loadout.take(Modifier::Anemometer);
        assert_eq!(state.displayed_wind(), Some(2.44));
    }

    #[test]
    fn rangefinder_reads_the_distance_to_the_nearest_target() {
        let state = RunState::new(0xA2, 320);
        let r = state.screen.range_to_nearest().unwrap();
        let nearest = state
            .screen
            .structures
            .iter()
            .map(|s| (s.centre_x() - state.screen.muzzle().x).abs())
            .fold(f64::MAX, f64::min);
        assert!((r - nearest).abs() < 1e-9);
    }

    #[test]
    fn enfilade_moves_the_emplacement_only_before_the_first_shot() {
        let mut state = RunState::new(0xE1, 320);
        assert!(!state.reposition(10.0), "no Enfilade, no repositioning");
        state.loadout.take(Modifier::Enfilade);
        assert!(state.reposition(40.0));
        // Clamped to the modifier's +/-25 units.
        assert_eq!(state.emplacement_offset, 25.0);
        assert_eq!(state.screen.muzzle_x, EMPLACEMENT_X + 25.0);
        state.fire(shot(800, 500), false);
        assert!(!state.reposition(-10.0), "repositioning after firing");
    }

    #[test]
    fn enfilade_offsets_are_whole_units() {
        let mut state = RunState::new(0xE1, 320);
        state.loadout.take(Modifier::Enfilade);
        assert!(state.reposition(7.4));
        assert_eq!(state.emplacement_offset, 7.0);
        assert!(state.reposition(-3.5));
        assert_eq!(state.emplacement_offset, -3.0);
        // The emplacement stops at x = 14, short of the modifier's -25.
        assert!(state.reposition(-25.0));
        assert_eq!(state.emplacement_offset, 14.0 - EMPLACEMENT_X);
        assert_eq!(state.screen.muzzle_x, 14.0);
        state.fire(shot(800, 500), false);
        assert_eq!(state.log[0].emplacement_offset, -16);
    }

    #[test]
    fn the_fuse_is_held_and_logged_in_tenths() {
        let mut state = RunState::new(0xF5, 320);
        state.set_fuse(123.456);
        assert_eq!(state.fuse, 123.5);
        assert_eq!(state.fuse_tenths(), 1235);
        state.set_fuse(-4.0);
        assert_eq!(state.fuse, 0.0);
        state.set_fuse(f64::NAN);
        assert_eq!(state.fuse, 0.0);
        state.set_fuse(1e9);
        assert_eq!(state.fuse, 480.0, "clamped to 1.5 screen widths");
        // A direct write is snapped to the grid when the shot is fired, so
        // what flew is what was logged.
        state.fuse = 87.6543;
        state.fire(shot(800, 500), false);
        assert_eq!(state.fuse, 87.7);
        assert_eq!(state.log[0].fuse_tenths, 877);
    }

    #[test]
    fn a_new_screen_starts_with_the_fuse_on_the_grid() {
        let state = RunState::new(0xF6, 333);
        let tenths = state.fuse_tenths();
        assert_eq!(state.fuse, tenths as f64 / 10.0);
        let exact = state.screen.default_fuse(0).unwrap();
        assert!((state.fuse - exact).abs() <= 0.05 + 1e-9);
    }

    #[test]
    fn chalk_marks_keep_only_as_many_marks_as_stacks() {
        let mut state = RunState::new(0xCA1, 320);
        state.loadout.take(Modifier::ChalkMarks);
        for _ in 0..4 {
            if state.phase != Phase::Firing {
                break;
            }
            state.fire(shot(1700, 100), false);
        }
        assert_eq!(state.chalk.len(), 1);
        let mut state = RunState::new(0xCA1, 320);
        state.loadout.take(Modifier::ChalkMarks);
        state.loadout.take(Modifier::ChalkMarks);
        for _ in 0..4 {
            if state.phase != Phase::Firing {
                break;
            }
            state.fire(shot(1700, 100), false);
        }
        assert_eq!(state.chalk.len(), 2);
    }

    #[test]
    fn seed_entry_accepts_anything_a_player_might_type() {
        assert_eq!(parse_seed("12345"), 12345);
        assert_eq!(parse_seed(" 0xFF "), 255);
        assert_eq!(parse_seed("salt line"), parse_seed("salt line"));
        assert_ne!(parse_seed("salt line"), parse_seed("salt lines"));
        assert_eq!(format_seed(4242), "4242");
        assert_eq!(parse_seed(&format_seed(987654321)), 987654321);
        // The daily seed depends only on the day number, never on the clock.
        assert_eq!(daily_seed(20_000), daily_seed(20_000));
        assert_ne!(daily_seed(20_000), daily_seed(20_001));
    }

    #[test]
    fn a_narrowed_power_range_pulls_the_targets_in() {
        // Two stacks of Ballast cut the power range by 36%, which puts the
        // late-band distances past what the shell can physically cover. The
        // generator has to place the target inside the loadout's reach —
        // reshaping terrain cannot fix an out-of-range target. Found by the
        // batch harness at band 13+.
        let mut heavy = Loadout::new();
        heavy.take(Modifier::Ballast);
        heavy.take(Modifier::Ballast);
        let reach = crate::ballistics::max_flat_range(&heavy);
        assert!(
            reach < target_distance(40),
            "this test is only meaningful while Ballast cannot reach the cap"
        );
        for index in [52u32, 60, 80, 120, 200] {
            assert!(reachable_distance(band_of(index), &heavy) <= reach * RANGE_MARGIN + 1e-9);
            for seed in 0..8u64 {
                let s = generate_screen(seed * 7 + 3, index, 320, &heavy);
                let r = solver::validate(&s, &heavy);
                assert!(
                    r.solvable(),
                    "Ballast x2 screen {index} ({}) only {} candidates",
                    s.template.name(),
                    r.worst
                );
                assert!(!s.fell_back, "should not need the Flat fallback");
            }
        }
        // A plain loadout still gets the full design distance.
        let plain = Loadout::new();
        assert_eq!(reachable_distance(40, &plain), target_distance(40));
    }

    #[test]
    fn buffer_width_stays_in_range() {
        assert_eq!(buffer_width(200), 320);
        assert_eq!(buffer_width(360), 360);
        assert_eq!(buffer_width(1920), 400);
    }

    #[test]
    fn wide_buffers_generate_terrain_all_the_way_across() {
        // DESIGN §5.1: generate terrain to fill whatever width the device
        // reports. Difficulty stays in world units, so the extra width is
        // overscan rather than a harder screen.
        let lo = Loadout::new();
        for w in [320usize, 360, 400] {
            let s = generate_screen(SEED, 12, w, &lo);
            assert_eq!(s.field.len(), w);
            assert_eq!(s.width, w);
            for i in 0..w {
                assert!(s.field.surface(i) >= WORLD_FLOOR);
            }
        }
        let a = generate_screen(SEED, 12, 320, &lo);
        let b = generate_screen(SEED, 12, 400, &lo);
        assert_eq!(
            a.structures[0].centre_x().round(),
            b.structures[0].centre_x().round(),
            "target distance should not depend on device width"
        );
    }
}

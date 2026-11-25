//! Fixed-step integration and collision. DESIGN §3.
//!
//! Wind is an **acceleration**, not a velocity offset: `vx` gains
//! `wind * dt` every step, so lateral drift grows with the square of flight
//! time and long shots are disproportionately wind-sensitive. Nothing in here
//! may be "simplified" into a velocity offset.
//!
//! Semi-implicit Euler at a fixed `DT`. The render layer interpolates; the sim
//! never does. Trig is only ever evaluated on quantized angles via `dmath`.
//!
//! This module is the one place that decides where a shell goes. The same
//! [`fire`] call serves the run applying a real shot
//! ([`crate::run::Screen::apply_shot`]), the solver proving a screen clearable
//! ([`crate::solver`]), and the Godot layer drawing aim arcs and playing a
//! flight back sample by sample from [`Trace::path`]. Because the solver runs
//! the same integrator on the same integer [`Shot`] the player would dial in,
//! a solver candidate is exactly the shot the player gets.
//!
//! Ballistics knows nothing about structural integrity, the economy or
//! drafting. It takes a [`World`] (heightfield, target boxes, wind) and
//! returns [`Trace`]s saying how each flight ended; cratering and damage are
//! the caller's job.
//!
//! Units: world units are pixels, y up from the bottom of the screen buffer
//! (y = 0); time is in
//! seconds; velocities are units/s; gravity and wind are units/s².
//!
//! Each step, in order: accelerate and move, then test the Airburst fuse, the
//! structure boxes (a swept segment test), the terrain (a point with a radius
//! against the interpolated heightfield, refined by bisection), and finally
//! the off-screen limits. Skipstone ricochet, Auger tunnelling and Storm Front
//! are handled inside that loop; Cluster submunitions are spawned afterwards
//! by [`fire`].

use crate::dmath::{self, abs, clamp};
use crate::modifiers::Loadout;
use crate::terrain::{Heightfield, Material, ROCK_DEPTH, SAND_DEPTH};
use crate::{
    Vec2, BASE_BLAST_RADIUS, BASE_CRATER_RADIUS, DT, GRAVITY, MAX_SHELL_STEPS, MISS_WIDTH_FACTOR,
    POWER_MAX, POWER_MIN, POWER_TICKS_MAX, SHELL_RADIUS,
};

/// Ricochet is allowed at or below this incidence to the surface. DESIGN §4,
/// Skipstone: "ricochet once off any surface at <=20 degrees incidence".
pub const RICOCHET_MAX_INCIDENCE_DEG: f64 = 20.0;
/// Velocity retained through a ricochet.
pub const RICOCHET_RETENTION: f64 = 0.70;
/// Submunition spread either side of the base direction. DESIGN §4, Cluster.
pub const CLUSTER_SPREAD_DEG: f64 = 15.0;
/// Submunition damage share.
pub const CLUSTER_DAMAGE: f64 = 0.40;
/// Submunition speed as a share of the parent's impact speed.
pub const CLUSTER_SPEED: f64 = 0.45;

/// A player input. Integers, so a shot is exactly reproducible and the shot
/// log stays three small numbers per shot (DESIGN §3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Shot {
    /// Elevation above horizontal in 0.05° ticks ([`dmath::TICK_DEG`]):
    /// 0 is flat, [`dmath::MAX_ANGLE_TICKS`] (1800) is straight up.
    pub angle_ticks: i32,
    /// Power in `0..=`[`POWER_TICKS_MAX`], mapped linearly onto
    /// [`POWER_MIN`]..[`POWER_MAX`] units/s before loadout scaling.
    pub power_ticks: i32,
}

impl Shot {
    /// Raw input, neither clamped nor quantized. See [`Shot::quantized`].
    pub fn new(angle_ticks: i32, power_ticks: i32) -> Self {
        Self {
            angle_ticks,
            power_ticks,
        }
    }

    /// Snap to what the player's instruments can actually dial in.
    ///
    /// The angle is rounded to the loadout's angle quantum (Vernier, DESIGN
    /// §4) and clamped to `0..=1800`; power is clamped to
    /// `0..=`[`POWER_TICKS_MAX`]. [`fire`] applies this itself, so callers may
    /// pass raw input.
    pub fn quantized(self, lo: &Loadout) -> Shot {
        Shot {
            angle_ticks: clamp(
                lo.quantize_angle(self.angle_ticks) as f64,
                0.0,
                dmath::MAX_ANGLE_TICKS as f64,
            ) as i32,
            power_ticks: self.power_ticks.clamp(0, POWER_TICKS_MAX),
        }
    }

    /// Muzzle speed after loadout scaling, in units/s. `power_scale` moves the
    /// whole range (Ballast -20%, Featherweight +40%); `muzzle_scale` is Sabot.
    /// Power ticks are clamped; the angle plays no part.
    pub fn speed(self, lo: &Loadout) -> f64 {
        let t = self.power_ticks.clamp(0, POWER_TICKS_MAX) as f64 / POWER_TICKS_MAX as f64;
        (POWER_MIN + (POWER_MAX - POWER_MIN) * t) * lo.power_scale * lo.muzzle_scale
    }

    /// Initial velocity, units/s. Trig only on the integer tick count.
    ///
    /// Uses `angle_ticks` as given: call [`Shot::quantized`] first (or go
    /// through [`fire`], which does) to get what the player's instruments
    /// would actually fire.
    pub fn velocity(self, lo: &Loadout) -> Vec2 {
        let (c, s) = dmath::dir_from_ticks(self.angle_ticks);
        let v = self.speed(lo);
        Vec2::new(c * v, s * v)
    }

    /// Elevation in degrees, for display.
    pub fn angle_degrees(self) -> f64 {
        self.angle_ticks as f64 * dmath::TICK_DEG
    }

    /// The barrel sprite frame to display: angle quantized to 5 degrees.
    /// DESIGN §5.3 — the sprite is coarse feedback, the number is the
    /// instrument, and the gap between them is the point. Rounds to the
    /// nearest frame, `0..=18`.
    pub fn barrel_frame(self) -> usize {
        ((self.angle_ticks + crate::BARREL_FRAME_TICKS / 2) / crate::BARREL_FRAME_TICKS)
            .clamp(0, 18) as usize
    }
}

/// Lowest and highest speed the loadout can produce, for the solver's range check.
/// Units/s, at power tick 0 and at [`POWER_TICKS_MAX`].
pub fn speed_bounds(lo: &Loadout) -> (f64, f64) {
    (
        Shot::new(0, 0).speed(lo),
        Shot::new(0, POWER_TICKS_MAX).speed(lo),
    )
}

/// Flat-ground range at the best elevation, for the loadout's full power.
///
/// This is the textbook `v²/g`: the range of a 45° shot over level ground with
/// no wind and no drag, in world units.
///
/// Target placement is clamped against this: DESIGN §2.4 requires every screen
/// to be clearable with the player's *current* loadout, and no amount of
/// terrain reshaping can fix a target that is out of range. Two stacks
/// of Ballast cut the power range by 36%, which puts the band-20 distance past
/// what the shell can physically cover.
pub fn max_flat_range(lo: &Loadout) -> f64 {
    let (_, v_max) = speed_bounds(lo);
    v_max * v_max / crate::GRAVITY
}

/// Nearest power tick to a speed, and whether it is inside the range.
///
/// The inverse of [`Shot::speed`]: `None` when `speed` (units/s) is outside
/// what the loadout can fire, otherwise the tick whose speed is closest.
/// Rounding to a tick means the returned shot is slightly faster or slower
/// than asked; the solver re-simulates for exactly that reason.
pub fn power_ticks_for_speed(speed: f64, lo: &Loadout) -> Option<i32> {
    let (lo_s, hi_s) = speed_bounds(lo);
    // `hi_s <= lo_s` only for a degenerate zero-scale loadout, where the
    // division below would be meaningless.
    if speed < lo_s || speed > hi_s || hi_s <= lo_s {
        return None;
    }
    let t = (speed / (lo.power_scale * lo.muzzle_scale) - POWER_MIN) / (POWER_MAX - POWER_MIN);
    Some(clamp(
        dmath::round(t * POWER_TICKS_MAX as f64),
        0.0,
        POWER_TICKS_MAX as f64,
    ) as i32)
}

/// Axis-aligned box in world space, y up.
///
/// `(x0, y0)` is the lower-left corner and `(x1, y1)` the upper-right; every
/// method assumes `x0 <= x1` and `y0 <= y1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    /// Left edge.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
}

impl Aabb {
    /// Box from its corners. Not normalised: pass the smaller values first.
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }
    /// Grow by `r` on every side. Growing the box by the shell's radius turns
    /// "does a circle touch the box" into "does a point enter the bigger box"
    /// (a Minkowski sum, with square corners standing in for round ones).
    pub fn expand(self, r: f64) -> Aabb {
        Aabb::new(self.x0 - r, self.y0 - r, self.x1 + r, self.y1 + r)
    }
    /// Point-in-box, edges inclusive.
    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    /// Midpoint of the box.
    pub fn centre(self) -> Vec2 {
        Vec2::new((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }
    /// Shortest distance from a point to the box, 0 inside.
    pub fn distance_to(self, p: Vec2) -> f64 {
        let dx = if p.x < self.x0 {
            self.x0 - p.x
        } else if p.x > self.x1 {
            p.x - self.x1
        } else {
            0.0
        };
        let dy = if p.y < self.y0 {
            self.y0 - p.y
        } else if p.y > self.y1 {
            p.y - self.y1
        } else {
            0.0
        };
        dmath::hypot(dx, dy)
    }
}

/// Liang–Barsky segment/box overlap. A step is 2.5 units at full base power,
/// about 4.4 with Featherweight and one Sabot and about 5.5 with two, and a
/// radio mast is 5px wide — a point-in-box test would let shells tunnel
/// through thin structures.
///
/// True if any point of the segment `a`→`b` lies in the box, edges included.
pub fn segment_hits_aabb(a: Vec2, b: Vec2, box_: Aabb) -> bool {
    if box_.contains(a) || box_.contains(b) {
        return true;
    }
    // Parametric clipping: the segment is `a + t*(b - a)` for t in [0, 1].
    // Each box edge trims that interval; if it empties, the segment misses.
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    // `p` is the segment's component along the edge's outward normal
    // (negative when it crosses that edge heading in, positive heading out)
    // and `q` the start point's distance inside the edge. `p == 0` means the
    // segment runs parallel to the edge, so it is either wholly inside it or
    // wholly outside.
    let clip = |p: f64, q: f64, t0: &mut f64, t1: &mut f64| -> bool {
        if p == 0.0 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > *t1 {
                return false;
            }
            if r > *t0 {
                *t0 = r;
            }
        } else {
            if r < *t0 {
                return false;
            }
            if r < *t1 {
                *t1 = r;
            }
        }
        true
    };
    clip(-dx, a.x - box_.x0, &mut t0, &mut t1)
        && clip(dx, box_.x1 - a.x, &mut t0, &mut t1)
        && clip(-dy, a.y - box_.y0, &mut t0, &mut t1)
        && clip(dy, box_.y1 - a.y, &mut t0, &mut t1)
}

/// Largest magnitude, for a step's ends and a box's edges, at which
/// [`Clearance`] will vouch for a step: 2^40 world units. Every screen the
/// game builds is far inside it; the limit exists so the proof on
/// [`Clearance`] holds for any input at all.
const CLEARANCE_LIMIT: f64 = 1_099_511_627_776.0;
/// How far beyond a box's edge both ends of a step must be before
/// [`Clearance`] vouches for it.
const CLEARANCE_MARGIN: f64 = 1.0;

/// The region in which a step cannot touch any target box: left of them all,
/// right of them all, or above them all, by more than [`CLEARANCE_MARGIN`].
///
/// [`simulate_shell`] asks this before sweeping a step against the boxes.
/// It is an early out and nothing else: [`Clearance::clears`] is true only
/// when [`segment_hits_aabb`] is false for every box grown by the shell
/// radius, so skipping the sweep cannot change where a shell ends.
///
/// "Outside the bounding box" alone would not be enough to promise that.
/// [`segment_hits_aabb`] works in rounded arithmetic, and for a step that
/// ends within a rounding error of an edge it can report a hit that exact
/// arithmetic would not. The margin and the magnitude limit put every step
/// passed here well clear of that. Take a step left of a box edge `e`, both
/// ends below `e - 1` and every magnitude under 2^40. The first clip has
/// `q = a.x - e` and `p = -(b.x - a.x)`, each rounded by at most 2^-12.
/// Moving toward the box, `|q|` exceeds `|p|` by more than 0.99, so
/// `q / p` rounds above 1 and the clip rejects the step. Moving away, `q / p`
/// is negative and no smaller than 0.99 / 2^41 in size, so it cannot
/// underflow to zero and the clip rejects it again. Standing still in x
/// (`p == 0`) is rejected because `q < 0`. The right and upper sides are the
/// mirror image through the second and fourth clips; by then the earlier
/// clips can only have narrowed `t0..t1` inside `0..1`, which is all the
/// argument needs. A NaN coordinate fails every comparison in `clears`, and
/// a box with an edge that is NaN or beyond the limit turns the early out
/// off altogether.
#[derive(Clone, Copy, Debug)]
struct Clearance {
    /// Both ends of a step below this x: clear on the left.
    left: f64,
    /// Both ends above this x: clear on the right.
    right: f64,
    /// Both ends above this y: clear overhead.
    above: f64,
}

impl Clearance {
    /// Never vouches for anything.
    const NEVER: Clearance = Clearance {
        left: f64::NEG_INFINITY,
        right: f64::INFINITY,
        above: f64::INFINITY,
    };

    /// The clear region around `targets`, each grown by [`SHELL_RADIUS`]
    /// exactly as [`simulate_shell`] grows it.
    fn of(targets: &[Aabb]) -> Clearance {
        let mut c = Clearance {
            left: f64::INFINITY,
            right: f64::NEG_INFINITY,
            above: f64::NEG_INFINITY,
        };
        for b in targets {
            let b = b.expand(SHELL_RADIUS);
            // Written so that a NaN edge fails it.
            let in_range = abs(b.x0) <= CLEARANCE_LIMIT
                && abs(b.x1) <= CLEARANCE_LIMIT
                && abs(b.y1) <= CLEARANCE_LIMIT;
            if !in_range {
                return Clearance::NEVER;
            }
            c.left = c.left.min(b.x0 - CLEARANCE_MARGIN);
            c.right = c.right.max(b.x1 + CLEARANCE_MARGIN);
            c.above = c.above.max(b.y1 + CLEARANCE_MARGIN);
        }
        c
    }

    /// True only if the step `a`→`b` touches no target box. False says
    /// nothing: the caller then tests each box.
    #[inline]
    fn clears(&self, a: Vec2, b: Vec2) -> bool {
        const L: f64 = CLEARANCE_LIMIT;
        (a.x > -L && b.x > -L && a.x < self.left && b.x < self.left)
            || (a.x < L && b.x < L && a.x > self.right && b.x > self.right)
            || (a.y < L && b.y < L && a.y > self.above && b.y > self.above)
    }
}

/// Everything a shell can interact with. Ballistics knows nothing about
/// structures, the economy, or drafting — only boxes and a heightfield.
pub struct World<'a> {
    /// Terrain to collide with. Read-only: craters are applied by the caller
    /// after the flight.
    pub field: &'a Heightfield,
    /// Target bounding boxes, in the same order as the screen's structures.
    pub targets: &'a [Aabb],
    /// Screen width in columns. Sets the horizontal miss limits
    /// ([`MISS_WIDTH_FACTOR`] widths beyond either edge).
    pub width: usize,
    /// Signed wind, in world units per second squared. Positive pushes toward
    /// +x. Scaled by [`Loadout::wind_scale`] inside the integrator.
    pub wind: f64,
}

/// Per-shot options that are not part of the loadout.
#[derive(Clone, Copy, Debug, Default)]
pub struct FireOptions {
    /// Keep every sampled position, for rendering. The solver leaves this off.
    pub record_path: bool,
    /// Airburst fuse, in horizontal units from the muzzle. Ignored unless the
    /// loadout holds Airburst.
    pub fuse: Option<f64>,
    /// Storm Front: wind resamples once mid-flight, at the trajectory apex.
    /// The replacement wind, units/s², takes over from the first step that
    /// begins with the shell no longer rising.
    pub wind_after_apex: Option<f64>,
}

/// How a shell's flight ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum End {
    /// Hit the ground. `pos` is the refined contact point, or where a
    /// tunnelling shell stopped; `material` is the surface layer of that
    /// column, or for a tunnelling shell the layer it stopped in. The run
    /// sizes the crater from the surface layer either way.
    Terrain { pos: Vec2, material: Material },
    /// Entered target box `index` (into [`World::targets`]). `pos` is the end
    /// of the step that entered it, not a refined contact point.
    Structure { index: usize, pos: Vec2 },
    /// The Airburst fuse ran out.
    Fuse { pos: Vec2 },
    /// Left the playfield: more than [`MISS_WIDTH_FACTOR`] screen widths past
    /// either edge, or more than a screen height below y = 0.
    OffScreen { pos: Vec2 },
    /// Hit the [`MAX_SHELL_STEPS`] cap without ending any other way.
    Timeout { pos: Vec2 },
}

impl End {
    /// Where the flight ended.
    pub fn pos(self) -> Vec2 {
        match self {
            End::Terrain { pos, .. }
            | End::Structure { pos, .. }
            | End::Fuse { pos }
            | End::OffScreen { pos }
            | End::Timeout { pos } => pos,
        }
    }
    /// A shell that left the screen or ran out of clock never detonates.
    pub fn detonates(self) -> bool {
        matches!(
            self,
            End::Terrain { .. } | End::Structure { .. } | End::Fuse { .. }
        )
    }
}

/// One shell's flight.
#[derive(Clone, Debug)]
pub struct Trace {
    /// Where the shell started.
    pub origin: Vec2,
    /// How and where it stopped.
    pub end: End,
    /// Highest sampled position. Starts at `origin`, so a shell that never
    /// rises reports its origin.
    pub apex: Vec2,
    /// Seconds of simulated flight, accumulated one [`DT`] per step.
    pub flight_time: f64,
    /// Ricochets taken (Skipstone).
    pub bounces: u8,
    /// Whether Auger engaged at any point.
    pub tunneled: bool,
    /// 1.0 for the shell the player fired, 0.4 for a cluster submunition.
    pub damage_scale: f64,
    /// Speed after the final step, units/s. Cluster submunitions launch at a
    /// share of it.
    pub impact_speed: f64,
    /// Integration steps taken.
    pub steps: u32,
    /// Sampled positions, present only when `record_path` was set.
    ///
    /// The origin, then one sample per step, so `path.len() == steps + 1` and
    /// `path[k]` is the position after `k` steps (`k * DT` seconds). The Godot
    /// layer relies on this to play a flight back in real time. On a ricochet
    /// step the sample is the pre-lift position.
    pub path: Vec<Vec2>,
}

impl Trace {
    /// Where the shell detonated, or `None` if it left the screen or timed out.
    pub fn detonation(&self) -> Option<Vec2> {
        if self.end.detonates() {
            Some(self.end.pos())
        } else {
            None
        }
    }
    /// Which structure the shell struck directly, if any.
    pub fn struck(&self) -> Option<usize> {
        match self.end {
            End::Structure { index, .. } => Some(index),
            _ => None,
        }
    }
}

/// Crater radius for a detonation. DESIGN §2.6, plus Sabot's `crater_scale`
/// and Thermal Lance treating Steel as Rock.
///
/// World units. `material` is what the shell hit (Steel for a direct hit on a
/// structure); `damage_scale` is [`Trace::damage_scale`], so submunitions dig
/// smaller holes.
pub fn crater_radius(material: Material, lo: &Loadout, damage_scale: f64) -> f64 {
    let m = if material == Material::Steel && lo.thermal_lance() {
        Material::Rock
    } else {
        material
    };
    BASE_CRATER_RADIUS * m.crater_mult() * lo.crater_scale * damage_scale
}

/// Blast radius for structural damage overlap. Airburst widens it.
///
/// World units, scaled by `damage_scale` like [`crater_radius`]. The run uses
/// it for damage falloff; the solver uses it to discard candidates that
/// detonate too far from their target to matter.
pub fn blast_radius(lo: &Loadout, damage_scale: f64) -> f64 {
    BASE_BLAST_RADIUS * lo.blast_scale * damage_scale
}

/// Local surface slope, from a central difference over one column.
fn slope_at(field: &Heightfield, x: f64) -> f64 {
    (field.surface_at(x + 1.0) - field.surface_at(x - 1.0)) * 0.5
}

/// Upward surface normal at `x`.
fn normal_at(field: &Heightfield, x: f64) -> Vec2 {
    let m = slope_at(field, x);
    let len = dmath::hypot(1.0, m);
    Vec2::new(-m / len, 1.0 / len)
}

/// Auger's travel budget: the nominal thickness of the layer it entered
/// (Sand 8 units, Rock 22). It does not subtract depth already excavated in
/// that column, and it is spent as path length, not vertical depth.
/// Bedrock stops it outright, on contact or wherever the tunnelling shell
/// reaches it with budget to spare (DESIGN §4).
fn auger_budget(material: Material) -> f64 {
    match material {
        Material::Sand => SAND_DEPTH,
        Material::Rock => ROCK_DEPTH - SAND_DEPTH,
        Material::Bedrock | Material::Steel => 0.0,
    }
}

/// Integrate one shell to termination.
///
/// `origin` and `velocity` are in world units and units/s. `damage_scale` is
/// carried through to [`Trace::damage_scale`] untouched. `allow_ricochet` and
/// `allow_tunnel` gate Skipstone and Auger even when the loadout holds them;
/// cluster submunitions pass `false` for both.
///
/// Always terminates: every path through the loop either ends the flight or
/// counts a step against [`MAX_SHELL_STEPS`]. Identical inputs give
/// bit-identical traces on every platform.
#[allow(clippy::too_many_arguments)]
pub fn simulate_shell(
    world: &World,
    origin: Vec2,
    velocity: Vec2,
    lo: &Loadout,
    opts: &FireOptions,
    damage_scale: f64,
    allow_ricochet: bool,
    allow_tunnel: bool,
) -> Trace {
    let mut p = origin;
    let mut v = velocity;
    let mut t = 0.0f64;
    let mut apex = origin;
    let mut bounces_left = if allow_ricochet {
        lo.ricochet_bounces
    } else {
        0
    };
    let mut tunnel_left = 0.0f64;
    let mut tunneling = false;
    let mut tunneled = false;
    let mut bounces = 0u8;
    let mut wind = world.wind;
    let mut wind_switched = false;
    let mut path = Vec::new();
    if opts.record_path {
        path.push(p);
    }

    let miss_x = world.width as f64 * MISS_WIDTH_FACTOR;
    let mut steps = 0u32;
    let clearance = Clearance::of(world.targets);

    let end = loop {
        if steps >= MAX_SHELL_STEPS {
            break End::Timeout { pos: p };
        }
        steps += 1;

        // Storm Front resamples the wind once, at the apex. DESIGN §4.
        // The latch is set whether or not a replacement wind was supplied, so
        // this test stops mattering after the apex.
        if !wind_switched && v.y <= 0.0 {
            if let Some(w) = opts.wind_after_apex {
                wind = w;
            }
            wind_switched = true;
        }

        // Semi-implicit Euler: accelerate, then move. Wind is acceleration.
        // Explicit Euler would move with the old velocity instead. The order
        // is part of the replay contract: swapping it shifts every trajectory
        // slightly and so needs a `SIM_EPOCH` bump (DESIGN §9.3).
        v.x += wind * lo.wind_scale * DT;
        v.y -= GRAVITY * DT;
        let prev = p;
        p.x += v.x * DT;
        p.y += v.y * DT;
        t += DT;
        if opts.record_path {
            path.push(p);
        }
        if p.y > apex.y {
            apex = p;
        }

        // Airburst fuse, measured in horizontal units from the muzzle.
        if let Some(fuse) = opts.fuse {
            if lo.airburst() && abs(p.x - origin.x) >= fuse {
                break End::Fuse { pos: p };
            }
        }

        // Structures. Checked before terrain: a shell entering a structure's
        // box has hit steel, whatever the ground under it is doing. The test
        // sweeps the whole step against each box grown by the shell radius,
        // so a fast shell cannot step clean over a thin mast. A tunnelling
        // shell is underground and skips it.
        //
        // Most steps are nowhere near a structure, and `clearance` lets them
        // skip the boxes: it passes only steps that every box test below
        // would reject (see [`Clearance`]), so `hit` is `None` either way.
        if !tunneling && !clearance.clears(prev, p) {
            let mut hit = None;
            for (i, b) in world.targets.iter().enumerate() {
                if segment_hits_aabb(prev, p, b.expand(SHELL_RADIUS)) {
                    hit = Some(i);
                    break;
                }
            }
            if let Some(index) = hit {
                break End::Structure { index, pos: p };
            }
        }

        // A point with a collision radius against the interpolated surface
        // under it (DESIGN §3).
        let surface = world.field.surface_at(p.x);
        let inside = p.y - SHELL_RADIUS <= surface;

        if tunneling {
            // The budget is spent as distance travelled along the path.
            tunnel_left -= dmath::hypot(p.x - prev.x, p.y - prev.y);
            if !inside {
                // Punched through and came out the far side. Checked first:
                // a point in the air has no layer, and one over a deep crater
                // would otherwise read as bedrock and stop the shell there.
                tunneling = false;
            } else {
                // Bedrock stops it at the shell's own depth (DESIGN §4),
                // which can come before the budget runs out when it entered
                // Rock.
                let material = world.field.material_below(p.x, p.y);
                if material == Material::Bedrock || tunnel_left <= 0.0 {
                    break End::Terrain { pos: p, material };
                }
            }
        } else if inside {
            // Refine the contact point between the last two samples so shallow
            // trajectories collide consistently (DESIGN §2.1).
            let contact = refine_contact(world.field, prev, p);
            let material = world.field.material_at(contact.x);

            // Ricochet first: a grazing hit never craters.
            if bounces_left > 0 {
                let n = normal_at(world.field, contact.x);
                let speed = dmath::hypot(v.x, v.y);
                if speed > 0.0 {
                    let dot = (v.x * n.x + v.y * n.y) / speed;
                    // |dot| is sin(incidence) against the surface plane.
                    let sin_max = dmath::sin(RICOCHET_MAX_INCIDENCE_DEG * dmath::PI / 180.0);
                    if abs(dot) <= sin_max {
                        // Mirror reflection about the surface, v - 2(v.n)n,
                        // then scaled down by the retention factor.
                        let vn = v.x * n.x + v.y * n.y;
                        v.x = (v.x - 2.0 * vn * n.x) * RICOCHET_RETENTION;
                        v.y = (v.y - 2.0 * vn * n.y) * RICOCHET_RETENTION;
                        bounces_left -= 1;
                        bounces += 1;
                        // Lift clear of the surface so the next step does not
                        // re-collide on the same sample. The recorded path
                        // keeps the pre-lift sample for this step.
                        p = Vec2::new(
                            contact.x + n.x * (SHELL_RADIUS + 0.25),
                            world.field.surface_at(contact.x)
                                + n.y * (SHELL_RADIUS + 0.25)
                                + SHELL_RADIUS,
                        );
                        continue;
                    }
                }
            }

            // Then tunnelling: at most once per shell, and only into a layer
            // with a budget. Bedrock and Steel fall through to detonation.
            if allow_tunnel && lo.auger() && !tunneled && material.is_tunnelable() {
                tunnel_left = auger_budget(material);
                if tunnel_left > 0.0 {
                    tunneling = true;
                    tunneled = true;
                    continue;
                }
            }

            break End::Terrain {
                pos: contact,
                material,
            };
        }

        // Off-screen horizontally past 1.5 screen widths is a miss (DESIGN §3).
        // Off-screen vertically is fine — shells arc above the viewport.
        if p.x < -miss_x || p.x > world.width as f64 + miss_x {
            break End::OffScreen { pos: p };
        }
        // A shell that has fallen far below y = 0 is gone.
        if p.y < -(crate::SCREEN_H as f64) {
            break End::OffScreen { pos: p };
        }
    };

    Trace {
        origin,
        end,
        apex,
        flight_time: t,
        bounces,
        tunneled,
        damage_scale,
        impact_speed: dmath::hypot(v.x, v.y),
        steps,
        path,
    }
}

/// Bisect the last step to find where the shell met the surface. Linear in the
/// sampled positions, so it stays deterministic and cheap.
///
/// The step is treated as a straight segment from `a` (above the surface) to
/// `b` (touching or below it). Twelve halvings put the contact within 1/4096
/// of a step, and returning the `hi` end means the point is always touching.
fn refine_contact(field: &Heightfield, a: Vec2, b: Vec2) -> Vec2 {
    let mut lo = 0.0f64;
    let mut hi = 1.0f64;
    for _ in 0..12 {
        let m = (lo + hi) * 0.5;
        let p = Vec2::new(a.x + (b.x - a.x) * m, a.y + (b.y - a.y) * m);
        if p.y - SHELL_RADIUS <= field.surface_at(p.x) {
            hi = m;
        } else {
            lo = m;
        }
    }
    Vec2::new(a.x + (b.x - a.x) * hi, a.y + (b.y - a.y) * hi)
}

/// Fire one shell, including any cluster submunitions it throws.
/// `traces[0]` is always the shell the player fired.
///
/// The shot is quantized to the loadout first ([`Shot::quantized`]), so raw
/// input is safe. Submunitions follow only if the loadout holds Cluster and
/// the primary detonated; a shell that left the screen throws nothing.
pub fn fire(
    world: &World,
    origin: Vec2,
    shot: Shot,
    lo: &Loadout,
    opts: &FireOptions,
) -> Vec<Trace> {
    let shot = shot.quantized(lo);
    let primary = simulate_shell(world, origin, shot.velocity(lo), lo, opts, 1.0, true, true);
    let mut out = Vec::with_capacity(1 + lo.cluster_submunitions as usize);

    let spawn = if lo.cluster() && primary.end.detonates() {
        Some(submunitions(world, &primary, lo, opts))
    } else {
        None
    };
    out.push(primary);
    if let Some(subs) = spawn {
        out.extend(subs);
    }
    out
}

/// Cluster split: `n` submunitions fanned across +/-15 degrees, each carrying
/// 40% damage. DESIGN §4. Submunitions do not split again, and carry neither
/// ricochet nor tunnelling.
///
/// `n` is [`Loadout::cluster_submunitions`]: 3 per Cluster stack, so two
/// stacks fan six across the same spread.
fn submunitions(world: &World, parent: &Trace, lo: &Loadout, opts: &FireOptions) -> Vec<Trace> {
    let n = lo.cluster_submunitions as usize;
    if n == 0 {
        return Vec::new();
    }
    let pos = parent.end.pos();
    let speed = parent.impact_speed * CLUSTER_SPEED;
    if speed <= 0.0 {
        return Vec::new();
    }

    // Spray outward: off terrain, along the reflected direction; off a
    // structure or an airburst, along the incoming direction.
    let base = match parent.end {
        End::Terrain { .. } => {
            let n_hat = normal_at(world.field, pos.x);
            // Incoming direction from the parent's terminal velocity is not
            // retained, so reflect the vector from apex to impact — the same
            // sign, and stable.
            let inx = pos.x - parent.apex.x;
            let iny = pos.y - parent.apex.y;
            let len = dmath::hypot(inx, iny).max(1e-9);
            let (dx, dy) = (inx / len, iny / len);
            let vn = dx * n_hat.x + dy * n_hat.y;
            let (rx, ry) = (dx - 2.0 * vn * n_hat.x, dy - 2.0 * vn * n_hat.y);
            Vec2::new(rx, ry)
        }
        _ => {
            // The same apex-to-impact chord stands in for the incoming
            // direction here.
            let inx = pos.x - parent.apex.x;
            let iny = pos.y - parent.apex.y;
            let len = dmath::hypot(inx, iny).max(1e-9);
            Vec2::new(inx / len, iny / len)
        }
    };

    // Spread angles are constants, so the trig is on quantized input.
    let mut out = Vec::with_capacity(n);
    let sub_opts = FireOptions {
        record_path: opts.record_path,
        fuse: None,
        wind_after_apex: None,
    };
    for k in 0..n {
        let f = if n == 1 {
            0.0
        } else {
            -1.0 + 2.0 * (k as f64) / ((n - 1) as f64)
        };
        // `f` runs evenly from -1 to 1, fanning the shells edge to edge.
        let a = f * CLUSTER_SPREAD_DEG * dmath::PI / 180.0;
        let (c, s) = (dmath::cos(a), dmath::sin(a));
        // 2D rotation of `base` by `a`.
        let dir = Vec2::new(base.x * c - base.y * s, base.x * s + base.y * c);
        // Start just clear of the surface so the submunition gets a step.
        let start = Vec2::new(pos.x + dir.x * 1.5, pos.y + dir.y * 1.5 + 0.5);
        out.push(simulate_shell(
            world,
            start,
            Vec2::new(dir.x * speed, dir.y * speed),
            lo,
            &sub_opts,
            CLUSTER_DAMAGE,
            false,
            false,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Heightfield;

    fn flat(width: usize, h: f64) -> Heightfield {
        Heightfield::from_base(vec![h; width])
    }

    fn world<'a>(f: &'a Heightfield, targets: &'a [Aabb], wind: f64) -> World<'a> {
        World {
            field: f,
            targets,
            width: f.len(),
            wind,
        }
    }

    #[test]
    fn a_shot_lands_downrange_and_craters() {
        let f = flat(320, 50.0);
        let w = world(&f, &[], 0.0);
        let lo = Loadout::new();
        let t = simulate_shell(
            &w,
            Vec2::new(30.0, 55.0),
            Shot::new(900, 700).velocity(&lo),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        match t.end {
            End::Terrain { pos, material } => {
                assert!(pos.x > 100.0, "landed at {}", pos.x);
                assert_eq!(material, Material::Sand);
            }
            other => panic!("expected terrain impact, got {other:?}"),
        }
    }

    #[test]
    fn wind_drift_grows_with_the_square_of_flight_time() {
        // The single most important property in the model (DESIGN §3): a shot
        // with twice the flight time drifts about four times as far.
        let f = flat(4000, 20.0);
        let lo = Loadout::new();
        let drift_for = |power: i32| -> (f64, f64) {
            let calm = world(&f, &[], 0.0);
            let windy = world(&f, &[], 4.0);
            let o = Vec2::new(30.0, 25.0);
            let a = simulate_shell(
                &calm,
                o,
                Shot::new(900, power).velocity(&lo),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            );
            let b = simulate_shell(
                &windy,
                o,
                Shot::new(900, power).velocity(&lo),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            );
            (b.end.pos().x - a.end.pos().x, a.flight_time)
        };
        let (d_short, t_short) = drift_for(400);
        let (d_long, t_long) = drift_for(800);
        assert!(d_short > 0.0 && d_long > 0.0);
        // The longer shot's drift should scale as the square of its flight time.
        let ratio = t_long / t_short;
        let expected = ratio * ratio;
        let got = d_long / d_short;
        assert!(
            got > expected * 0.75 && got < expected * 1.25,
            "drift ratio {got} vs time-squared {expected}"
        );
    }

    #[test]
    fn wind_is_not_a_velocity_offset() {
        // Under a velocity offset, drift would be linear in flight time and
        // the initial step would already be displaced. Neither holds.
        let f = flat(4000, 20.0);
        let lo = Loadout::new();
        let opts = FireOptions {
            record_path: true,
            ..Default::default()
        };
        let calm = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 25.0),
            Shot::new(900, 600).velocity(&lo),
            &lo,
            &opts,
            1.0,
            true,
            true,
        );
        let windy = simulate_shell(
            &world(&f, &[], 6.0),
            Vec2::new(30.0, 25.0),
            Shot::new(900, 600).velocity(&lo),
            &lo,
            &opts,
            1.0,
            true,
            true,
        );
        let first_step_gap = abs(windy.path[1].x - calm.path[1].x);
        // One step of acceleration displaces by w*dt^2 ~ 4e-4, not w*dt ~ 0.05.
        assert!(first_step_gap < 1e-3, "first step gap {first_step_gap}");
        let final_gap = abs(windy.end.pos().x - calm.end.pos().x);
        assert!(final_gap > 5.0, "no meaningful drift: {final_gap}");
    }

    #[test]
    fn ballast_halves_the_drift() {
        let f = flat(4000, 20.0);
        let plain = Loadout::new();
        let mut ballast = Loadout::new();
        ballast.take(crate::modifiers::Modifier::Ballast);
        let shoot = |lo: &Loadout, wind: f64| {
            simulate_shell(
                &world(&f, &[], wind),
                Vec2::new(30.0, 25.0),
                Shot::new(900, 500).velocity(lo),
                lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            )
            .end
            .pos()
            .x
        };
        let d_plain = shoot(&plain, 5.0) - shoot(&plain, 0.0);
        let d_ballast = shoot(&ballast, 5.0) - shoot(&ballast, 0.0);
        // Wind scale 0.5, and shorter flight from the narrowed power range.
        assert!(d_ballast < d_plain * 0.6, "{d_ballast} vs {d_plain}");
    }

    #[test]
    fn structures_stop_a_shell_even_when_thin() {
        let f = flat(320, 40.0);
        // A 5px-wide radio mast, the thinnest footprint in DESIGN §5.4.
        let mast = Aabb::new(160.0, 40.0, 165.0, 72.0);
        let w = world(&f, std::slice::from_ref(&mast), 0.0);
        let lo = Loadout::new();
        let mut hits = 0;
        for power in 300..=900 {
            let t = simulate_shell(
                &w,
                Vec2::new(30.0, 45.0),
                Shot::new(600, power).velocity(&lo),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            );
            if let End::Structure { index, .. } = t.end {
                assert_eq!(index, 0);
                hits += 1;
            }
        }
        assert!(
            hits > 0,
            "never hit a 5px mast across the whole power sweep"
        );
    }

    #[test]
    fn off_screen_horizontally_ends_the_shot() {
        let f = flat(320, 5.0);
        let w = world(&f, &[], 0.0);
        let lo = Loadout::new();
        let t = simulate_shell(
            &w,
            Vec2::new(30.0, 120.0),
            Vec2::new(600.0, 400.0),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert!(matches!(t.end, End::OffScreen { .. }), "{:?}", t.end);
        assert!(!t.end.detonates());
    }

    #[test]
    fn airburst_detonates_at_the_fuse_distance() {
        let f = flat(320, 20.0);
        let w = world(&f, &[], 0.0);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Airburst);
        let opts = FireOptions {
            fuse: Some(120.0),
            ..Default::default()
        };
        let t = simulate_shell(
            &w,
            Vec2::new(30.0, 25.0),
            Shot::new(1000, 700).velocity(&lo),
            &lo,
            &opts,
            1.0,
            true,
            true,
        );
        match t.end {
            End::Fuse { pos } => assert!(
                abs(pos.x - 150.0) < 2.0,
                "fuse fired at x={}, wanted ~150",
                pos.x
            ),
            other => panic!("expected a fuse detonation, got {other:?}"),
        }
        assert!(blast_radius(&lo, 1.0) > blast_radius(&Loadout::new(), 1.0));
    }

    #[test]
    fn skipstone_ricochets_off_a_shallow_hit_only() {
        let f = flat(2000, 30.0);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Skipstone);
        // Shallow: a low arc grazes and skips.
        let shallow = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 33.0),
            Shot::new(120, 900).velocity(&lo),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert_eq!(shallow.bounces, 1, "shallow shot did not skip");
        // Steep: comes in well over 20 degrees and buries itself.
        let steep = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 33.0),
            Shot::new(1600, 700).velocity(&lo),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert_eq!(steep.bounces, 0, "steep shot should not skip");
    }

    #[test]
    fn auger_tunnels_one_layer_and_bedrock_stops_it() {
        let f = flat(320, 60.0);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Auger);
        let t = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 65.0),
            Shot::new(1400, 600).velocity(&lo),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert!(t.tunneled, "auger did not tunnel");
        assert!(
            t.end.pos().y < 60.0,
            "detonated above the surface at {}",
            t.end.pos().y
        );
        // Without auger the same shot stops at the surface.
        let plain = Loadout::new();
        let t2 = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 65.0),
            Shot::new(1400, 600).velocity(&plain),
            &plain,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert!(!t2.tunneled);
        assert!(t2.end.pos().y > t.end.pos().y);
    }

    #[test]
    fn auger_stops_at_bedrock_below_the_surface() {
        // Dug down 25 units into Rock, 5 short of bedrock. The Rock budget
        // (22) would carry the shell well past it; bedrock must stop it
        // where the shell reaches it, not only where it is exposed.
        let mut f = flat(320, 60.0);
        f.scar.fill(25.0);
        f.material.fill(Material::Rock);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Auger);
        let t = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 40.0),
            Shot::new(1400, 600).velocity(&lo),
            &lo,
            &FireOptions::default(),
            1.0,
            true,
            true,
        );
        assert!(t.tunneled, "auger did not tunnel");
        let depth = 60.0 - t.end.pos().y;
        assert!(
            depth < ROCK_DEPTH + 3.0,
            "tunnelled {depth} units below the original surface, past bedrock at {ROCK_DEPTH}"
        );
        assert!(
            matches!(
                t.end,
                End::Terrain {
                    material: Material::Bedrock,
                    ..
                }
            ),
            "{:?}",
            t.end
        );
    }

    #[test]
    fn auger_leaving_the_ground_does_not_stop_in_the_air() {
        // A Rock ledge (dug 15 into ground at 60) drops into a pit whose
        // columns stand on higher ground (90) dug out to 20, so a point in
        // the pit's air is more than 30 below its column's original surface.
        // A shell that tunnels out of the ledge must fly on into the pit,
        // not read that air as bedrock and stop in it.
        let mut f = flat(320, 60.0);
        for i in 0..f.len() {
            if i < 100 {
                f.scar[i] = 15.0;
                f.material[i] = Material::Rock;
            } else {
                f.base[i] = 90.0;
                f.scar[i] = 70.0;
                f.material[i] = Material::Bedrock;
            }
        }
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Auger);
        for k in 0..20 {
            let t = simulate_shell(
                &world(&f, &[], 0.0),
                Vec2::new(80.0 + k as f64 * 0.1, 50.0),
                Vec2::new(180.0, -40.0),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            );
            assert!(t.tunneled, "offset {k}: auger did not tunnel");
            let p = t.end.pos();
            assert!(
                p.y - SHELL_RADIUS <= f.surface_at(p.x) + 1e-9,
                "offset {k}: stopped in the air at ({}, {}), surface {}",
                p.x,
                p.y,
                f.surface_at(p.x)
            );
        }
    }

    #[test]
    fn cluster_throws_submunitions_that_detonate_apart() {
        let f = flat(320, 40.0);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Cluster);
        let traces = fire(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 45.0),
            Shot::new(900, 600),
            &lo,
            &FireOptions::default(),
        );
        assert_eq!(traces.len(), 4, "one shell plus three submunitions");
        assert_eq!(traces[0].damage_scale, 1.0);
        for s in &traces[1..] {
            assert_eq!(s.damage_scale, CLUSTER_DAMAGE);
        }
        let xs: Vec<f64> = traces[1..].iter().map(|t| t.end.pos().x).collect();
        let spread = xs.iter().cloned().fold(f64::MIN, f64::max)
            - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 2.0, "submunitions landed on top of each other");
    }

    #[test]
    fn identical_inputs_produce_bit_identical_flights() {
        let f = flat(320, 44.0);
        let lo = Loadout::new();
        let run = || {
            simulate_shell(
                &world(&f, &[], 3.7),
                Vec2::new(30.0, 49.0),
                Shot::new(874, 613).velocity(&lo),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            )
        };
        let a = run();
        let b = run();
        assert_eq!(a.end.pos().x.to_bits(), b.end.pos().x.to_bits());
        assert_eq!(a.end.pos().y.to_bits(), b.end.pos().y.to_bits());
        assert_eq!(a.flight_time.to_bits(), b.flight_time.to_bits());
        assert_eq!(a.steps, b.steps);
    }

    #[test]
    fn power_ticks_round_trip_through_speed() {
        for m in [
            None,
            Some(crate::modifiers::Modifier::Ballast),
            Some(crate::modifiers::Modifier::Sabot),
            Some(crate::modifiers::Modifier::Featherweight),
        ] {
            let mut lo = Loadout::new();
            if let Some(m) = m {
                lo.take(m);
            }
            for ticks in [0, 1, 250, 499, 750, 1000] {
                let s = Shot::new(0, ticks).speed(&lo);
                assert_eq!(power_ticks_for_speed(s, &lo), Some(ticks));
            }
            let (lo_s, hi_s) = speed_bounds(&lo);
            assert_eq!(power_ticks_for_speed(lo_s - 1.0, &lo), None);
            assert_eq!(power_ticks_for_speed(hi_s + 1.0, &lo), None);
        }
    }

    #[test]
    fn barrel_frames_are_five_degree_steps() {
        assert_eq!(Shot::new(0, 0).barrel_frame(), 0);
        assert_eq!(Shot::new(49, 0).barrel_frame(), 0);
        assert_eq!(Shot::new(51, 0).barrel_frame(), 1);
        assert_eq!(Shot::new(900, 0).barrel_frame(), 9);
        assert_eq!(Shot::new(1800, 0).barrel_frame(), 18);
    }

    #[test]
    fn segment_box_overlap_catches_pass_through() {
        let b = Aabb::new(10.0, 10.0, 12.0, 40.0);
        assert!(segment_hits_aabb(
            Vec2::new(8.0, 20.0),
            Vec2::new(14.0, 20.0),
            b
        ));
        assert!(!segment_hits_aabb(
            Vec2::new(8.0, 50.0),
            Vec2::new(14.0, 50.0),
            b
        ));
        assert!(segment_hits_aabb(
            Vec2::new(11.0, 20.0),
            Vec2::new(11.0, 21.0),
            b
        ));
    }

    /// Coordinates chosen to sit on, beside and far beyond everything
    /// [`Clearance`] compares against.
    fn awkward(around: &[f64]) -> Vec<f64> {
        let mut out = vec![
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -0.0,
            5e-324,
            -5e-324,
            1e-300,
            CLEARANCE_LIMIT,
            -CLEARANCE_LIMIT,
            CLEARANCE_LIMIT.next_down(),
            (-CLEARANCE_LIMIT).next_up(),
            CLEARANCE_LIMIT * 4.0,
            -CLEARANCE_LIMIT * 4.0,
            1e300,
            -1e300,
            f64::MAX,
            f64::MIN,
        ];
        for &v in around {
            for d in [
                0.0,
                CLEARANCE_MARGIN,
                -CLEARANCE_MARGIN,
                SHELL_RADIUS,
                -SHELL_RADIUS,
            ] {
                let e = v + d;
                out.extend([
                    e,
                    e.next_up(),
                    e.next_down(),
                    e + 1e-9,
                    e - 1e-9,
                    e + 3.0,
                    e - 3.0,
                ]);
            }
        }
        out
    }

    #[test]
    fn clearance_only_vouches_for_steps_that_miss_every_box() {
        // The wreck box is the one `Screen::target_boxes` gives a cleared
        // structure; the last three sets are boxes no screen would hold.
        let wreck = Aabb::new(-1.0e9, -1.0e9, -1.0e9 + 1.0, -1.0e9 + 1.0);
        let sets: Vec<Vec<Aabb>> = vec![
            vec![],
            vec![Aabb::new(200.0, 50.0, 205.0, 98.0)],
            vec![
                Aabb::new(170.0, 40.0, 190.0, 60.0),
                Aabb::new(260.0, 70.0, 266.0, 110.0),
            ],
            vec![wreck, Aabb::new(240.0, 52.0, 252.0, 76.0)],
            vec![Aabb::new(240.0, 52.0, 252.0, 76.0), wreck, wreck],
            vec![Aabb::new(1e-9, 0.0, 2e-9, 1e-9)],
            vec![Aabb::new(205.0, 98.0, 200.0, 50.0)],
            vec![Aabb::new(200.0, 50.0, f64::NAN, 98.0)],
            vec![Aabb::new(-1e300, 50.0, 205.0, 1e300)],
        ];
        let mut r = crate::rng::Pcg32::new(0xC1EA);
        let (mut vouched, mut asked) = (0u32, 0u32);
        for boxes in &sets {
            let clearance = Clearance::of(boxes);
            let mut check = |a: Vec2, b: Vec2| {
                asked += 1;
                if clearance.clears(a, b) {
                    vouched += 1;
                    for bx in boxes {
                        assert!(
                            !segment_hits_aabb(a, b, bx.expand(SHELL_RADIUS)),
                            "vouched for {a:?} -> {b:?}, which hits {bx:?}"
                        );
                    }
                }
            };

            let edges: Vec<f64> = boxes
                .iter()
                .flat_map(|b| [b.x0, b.x1, b.y0, b.y1])
                .collect();
            let coords = awkward(&edges);
            // Every pairing of awkward coordinates along one axis, with the
            // other axis cycling through a few values, both ways round.
            let other = [
                f64::NAN,
                -1e300,
                -5.0,
                30.0,
                77.0,
                111.5,
                113.0,
                400.0,
                1e300,
            ];
            let mut pick = 0usize;
            for &a1 in &coords {
                for &b1 in &coords {
                    let (a2, b2) = (other[pick % 9], other[(pick / 9) % 9]);
                    pick += 1;
                    check(Vec2::new(a1, a2), Vec2::new(b1, b2));
                    check(Vec2::new(a2, a1), Vec2::new(b2, b1));
                }
            }
            // Ordinary steps: a point anywhere near the play area and a
            // step of a few units from it, as the integrator takes them.
            for _ in 0..40_000 {
                let a = Vec2::new(r.range_f64(-200.0, 700.0), r.range_f64(-50.0, 500.0));
                let b = Vec2::new(a.x + r.signed(7.0), a.y + r.signed(7.0));
                check(a, b);
            }
        }
        // The early out has to fire to be worth having, and must not always.
        assert!(vouched > 50_000, "vouched for only {vouched} of {asked}");
        assert!(vouched < asked);

        // A NaN edge or one past the limit turns it off entirely.
        let off = Clearance::of(&[Aabb::new(200.0, 50.0, f64::NAN, 98.0)]);
        assert!(!off.clears(Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)));
        let off = Clearance::of(&[Aabb::new(200.0, 50.0, 205.0, CLEARANCE_LIMIT * 2.0)]);
        assert!(!off.clears(Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)));
        // A NaN end is never vouched for.
        let on = Clearance::of(&[Aabb::new(200.0, 50.0, 205.0, 98.0)]);
        assert!(on.clears(Vec2::new(10.0, 60.0), Vec2::new(12.0, 61.0)));
        assert!(!on.clears(Vec2::new(f64::NAN, 60.0), Vec2::new(12.0, 61.0)));
        assert!(!on.clears(Vec2::new(10.0, 60.0), Vec2::new(f64::NAN, 61.0)));
        // Inside the margin it declines, even though the step misses.
        let near = 200.0 - SHELL_RADIUS - CLEARANCE_MARGIN * 0.5;
        assert!(!on.clears(Vec2::new(near, 60.0), Vec2::new(near + 0.1, 60.0)));
        assert!(!segment_hits_aabb(
            Vec2::new(near, 60.0),
            Vec2::new(near + 0.1, 60.0),
            Aabb::new(200.0, 50.0, 205.0, 98.0).expand(SHELL_RADIUS)
        ));
    }

    #[test]
    fn a_shell_ends_the_same_way_with_far_boxes_or_none() {
        // The early out skips the box sweep on most steps. A flight that
        // never comes near a box must therefore match the same flight with
        // no boxes at all, and one aimed at a box must still strike it.
        let f = flat(400, 40.0);
        let lo = Loadout::new();
        let opts = FireOptions::default();
        let boxes = [Aabb::new(300.0, 40.0, 306.0, 88.0)];
        let origin = Vec2::new(30.0, 46.0);
        let mut struck = 0;
        for angle in (100..=1700).step_by(50) {
            for power in (100..=1000).step_by(100) {
                let shot = Shot::new(angle, power);
                let with = fire(&world(&f, &boxes, 2.0), origin, shot, &lo, &opts);
                let without = fire(&world(&f, &[], 2.0), origin, shot, &lo, &opts);
                if with[0].struck().is_some() {
                    struck += 1;
                } else {
                    assert_eq!(format!("{with:?}"), format!("{without:?}"));
                }
            }
        }
        assert!(struck > 5, "only {struck} shots struck the box");
    }
}

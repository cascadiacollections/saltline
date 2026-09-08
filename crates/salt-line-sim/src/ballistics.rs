//! Fixed-step integration and collision. DESIGN §3.
//!
//! Wind is an **acceleration**, not a velocity offset: `vx` gains
//! `wind * dt` every step, so lateral drift grows with the square of flight
//! time and long shots are disproportionately wind-sensitive. Nothing in here
//! may be "simplified" into a velocity offset.
//!
//! Semi-implicit Euler at a fixed `DT`. The render layer interpolates; the sim
//! never does. Trig is only ever evaluated on quantized angles via `dmath`.

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
    pub angle_ticks: i32,
    pub power_ticks: i32,
}

impl Shot {
    pub fn new(angle_ticks: i32, power_ticks: i32) -> Self {
        Self {
            angle_ticks,
            power_ticks,
        }
    }

    /// Snap to what the player's instruments can actually dial in.
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

    /// Muzzle speed after loadout scaling. `power_scale` moves the whole range
    /// (Ballast -20%, Featherweight +40%); `muzzle_scale` is Sabot.
    pub fn speed(self, lo: &Loadout) -> f64 {
        let t = self.power_ticks.clamp(0, POWER_TICKS_MAX) as f64 / POWER_TICKS_MAX as f64;
        (POWER_MIN + (POWER_MAX - POWER_MIN) * t) * lo.power_scale * lo.muzzle_scale
    }

    /// Initial velocity. Trig only on the quantized tick count.
    pub fn velocity(self, lo: &Loadout) -> Vec2 {
        let (c, s) = dmath::dir_from_ticks(self.angle_ticks);
        let v = self.speed(lo);
        Vec2::new(c * v, s * v)
    }

    pub fn angle_degrees(self) -> f64 {
        self.angle_ticks as f64 * dmath::TICK_DEG
    }

    /// The barrel sprite frame to display: angle quantized to 5 degrees.
    /// DESIGN §5.3 — the sprite is coarse feedback, the number is the
    /// instrument, and the gap between them is the point.
    pub fn barrel_frame(self) -> usize {
        ((self.angle_ticks + crate::BARREL_FRAME_TICKS / 2) / crate::BARREL_FRAME_TICKS)
            .clamp(0, 18) as usize
    }
}

/// Lowest and highest speed the loadout can produce, for the solver's range check.
pub fn speed_bounds(lo: &Loadout) -> (f64, f64) {
    (
        Shot::new(0, 0).speed(lo),
        Shot::new(0, POWER_TICKS_MAX).speed(lo),
    )
}

/// Flat-ground range at the best elevation, for the loadout's full power.
///
/// Target placement is clamped against this: DESIGN §2.4 requires every screen
/// to be clearable with the player's *current* loadout, and no amount of
/// terrain reshaping can fix a target that is simply out of range. Two stacks
/// of Ballast cut the power range by 36%, which puts the band-20 distance past
/// what the shell can physically cover.
pub fn max_flat_range(lo: &Loadout) -> f64 {
    let (_, v_max) = speed_bounds(lo);
    v_max * v_max / crate::GRAVITY
}

/// Nearest power tick to a speed, and whether it is inside the range.
pub fn power_ticks_for_speed(speed: f64, lo: &Loadout) -> Option<i32> {
    let (lo_s, hi_s) = speed_bounds(lo);
    if speed < lo_s || speed > hi_s || hi_s <= lo_s {
        return None;
    }
    let t = (speed / (lo.power_scale * lo.muzzle_scale) - POWER_MIN) / (POWER_MAX - POWER_MIN);
    Some(clamp(dmath::round(t * POWER_TICKS_MAX as f64), 0.0, POWER_TICKS_MAX as f64) as i32)
}

/// Axis-aligned box in world space, y up.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Aabb {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn expand(self, r: f64) -> Aabb {
        Aabb::new(self.x0 - r, self.y0 - r, self.x1 + r, self.y1 + r)
    }
    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
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

/// Liang-Barsky segment/box overlap. A step can be up to ~3 units long, and a
/// radio mast is 5px wide — a point-in-box test would let shells tunnel
/// through thin structures.
pub fn segment_hits_aabb(a: Vec2, b: Vec2, box_: Aabb) -> bool {
    if box_.contains(a) || box_.contains(b) {
        return true;
    }
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
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

/// Everything a shell can interact with. Ballistics knows nothing about
/// structures, the economy, or drafting — only boxes and a heightfield.
pub struct World<'a> {
    pub field: &'a Heightfield,
    /// Target bounding boxes, in the same order as the screen's structures.
    pub targets: &'a [Aabb],
    pub width: usize,
    /// Signed wind, in world units per second squared.
    pub wind: f64,
}

/// Per-shot options that are not part of the loadout.
#[derive(Clone, Copy, Debug, Default)]
pub struct FireOptions {
    /// Keep every sampled position, for rendering. The solver leaves this off.
    pub record_path: bool,
    /// Airburst fuse, in horizontal units from the muzzle.
    pub fuse: Option<f64>,
    /// Storm Front: wind resamples once mid-flight, at the trajectory apex.
    pub wind_after_apex: Option<f64>,
}

/// How a shell's flight ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum End {
    Terrain { pos: Vec2, material: Material },
    Structure { index: usize, pos: Vec2 },
    Fuse { pos: Vec2 },
    OffScreen { pos: Vec2 },
    Timeout { pos: Vec2 },
}

impl End {
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
    pub origin: Vec2,
    pub end: End,
    pub apex: Vec2,
    pub flight_time: f64,
    pub bounces: u8,
    pub tunneled: bool,
    /// 1.0 for the shell the player fired, 0.4 for a cluster submunition.
    pub damage_scale: f64,
    pub impact_speed: f64,
    pub steps: u32,
    /// Sampled positions, present only when `record_path` was set.
    pub path: Vec<Vec2>,
}

impl Trace {
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
pub fn crater_radius(material: Material, lo: &Loadout, damage_scale: f64) -> f64 {
    let m = if material == Material::Steel && lo.thermal_lance() {
        Material::Rock
    } else {
        material
    };
    BASE_CRATER_RADIUS * m.crater_mult() * lo.crater_scale * damage_scale
}

/// Blast radius for structural damage overlap. Airburst widens it.
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

/// Auger's travel budget: the remaining thickness of the layer it entered.
/// Bedrock stops it outright (DESIGN §4).
fn auger_budget(material: Material) -> f64 {
    match material {
        Material::Sand => SAND_DEPTH,
        Material::Rock => ROCK_DEPTH - SAND_DEPTH,
        Material::Bedrock | Material::Steel => 0.0,
    }
}

/// Integrate one shell to termination.
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
    let mut bounces_left = if allow_ricochet { lo.ricochet_bounces } else { 0 };
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

    let end = loop {
        if steps >= MAX_SHELL_STEPS {
            break End::Timeout { pos: p };
        }
        steps += 1;

        // Storm Front resamples the wind once, at the apex. DESIGN §4.
        if !wind_switched && v.y <= 0.0 {
            if let Some(w) = opts.wind_after_apex {
                wind = w;
            }
            wind_switched = true;
        }

        // Semi-implicit Euler: accelerate, then move. Wind is acceleration.
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
        // box has hit steel, whatever the ground under it is doing.
        if !tunneling {
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

        let surface = world.field.surface_at(p.x);
        let inside = p.y - SHELL_RADIUS <= surface;

        if tunneling {
            tunnel_left -= dmath::hypot(p.x - prev.x, p.y - prev.y);
            let material = world.field.material_at(p.x);
            if material == Material::Bedrock || tunnel_left <= 0.0 {
                break End::Terrain { pos: p, material };
            }
            if !inside {
                // Punched through and came out the far side.
                tunneling = false;
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
                        let vn = v.x * n.x + v.y * n.y;
                        v.x = (v.x - 2.0 * vn * n.x) * RICOCHET_RETENTION;
                        v.y = (v.y - 2.0 * vn * n.y) * RICOCHET_RETENTION;
                        bounces_left -= 1;
                        bounces += 1;
                        // Lift clear of the surface so the next step does not
                        // re-collide on the same sample.
                        p = Vec2::new(
                            contact.x + n.x * (SHELL_RADIUS + 0.25),
                            world.field.surface_at(contact.x) + n.y * (SHELL_RADIUS + 0.25)
                                + SHELL_RADIUS,
                        );
                        continue;
                    }
                }
            }

            // Then tunneling.
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
        // A shell that has fallen far below the world floor is gone.
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
/// ricochet nor tunneling.
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
            let last = parent.origin; // direction of travel, reconstructed below
            let _ = last;
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
        let a = f * CLUSTER_SPREAD_DEG * dmath::PI / 180.0;
        let (c, s) = (dmath::cos(a), dmath::sin(a));
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
            Shot::new(450, 700).velocity(&lo),
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
                Shot::new(450, power).velocity(&lo),
                &lo,
                &FireOptions::default(),
                1.0,
                true,
                true,
            );
            let b = simulate_shell(
                &windy,
                o,
                Shot::new(450, power).velocity(&lo),
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
        let mut opts = FireOptions::default();
        opts.record_path = true;
        let calm = simulate_shell(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 25.0),
            Shot::new(450, 600).velocity(&lo),
            &lo,
            &opts,
            1.0,
            true,
            true,
        );
        let windy = simulate_shell(
            &world(&f, &[], 6.0),
            Vec2::new(30.0, 25.0),
            Shot::new(450, 600).velocity(&lo),
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
                Shot::new(450, 500).velocity(lo),
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
                Shot::new(300, power).velocity(&lo),
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
        assert!(hits > 0, "never hit a 5px mast across the whole power sweep");
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
            Shot::new(500, 700).velocity(&lo),
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
            Shot::new(60, 900).velocity(&lo),
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
            Shot::new(800, 700).velocity(&lo),
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
            Shot::new(700, 600).velocity(&lo),
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
            Shot::new(700, 600).velocity(&plain),
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
    fn cluster_throws_submunitions_that_detonate_apart() {
        let f = flat(320, 40.0);
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Cluster);
        let traces = fire(
            &world(&f, &[], 0.0),
            Vec2::new(30.0, 45.0),
            Shot::new(450, 600),
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
                Shot::new(437, 613).velocity(&lo),
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
        assert_eq!(Shot::new(24, 0).barrel_frame(), 0);
        assert_eq!(Shot::new(26, 0).barrel_frame(), 1);
        assert_eq!(Shot::new(450, 0).barrel_frame(), 9);
        assert_eq!(Shot::new(900, 0).barrel_frame(), 18);
    }

    #[test]
    fn segment_box_overlap_catches_pass_through() {
        let b = Aabb::new(10.0, 10.0, 12.0, 40.0);
        assert!(segment_hits_aabb(Vec2::new(8.0, 20.0), Vec2::new(14.0, 20.0), b));
        assert!(!segment_hits_aabb(
            Vec2::new(8.0, 50.0),
            Vec2::new(14.0, 50.0),
            b
        ));
        assert!(segment_hits_aabb(Vec2::new(11.0, 20.0), Vec2::new(11.0, 21.0), b));
    }
}

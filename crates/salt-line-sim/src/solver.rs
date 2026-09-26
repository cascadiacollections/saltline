//! Solvability validation. DESIGN §2.4.
//!
//! Every screen is proven clearable before it is shown — not "probably".
//!
//! For a fixed flight time the trajectory equations are linear in the velocity
//! components, so the solve is direct rather than iterative:
//!
//! ```text
//! dx = vx*t + 0.5*w*t^2   ->  vx = (dx - 0.5*w*t^2) / t
//! dy = vy*t - 0.5*g*t^2   ->  vy = (dy + 0.5*g*t^2) / t
//! ```
//!
//! Sweep `t` over 40 samples, keep every `t` whose `(theta, v)` lands inside
//! the loadout's angle and power ranges, then **verify by simulation**. The
//! closed form is only a candidate generator: the authority on whether a shot
//! connects is always the fixed-step integrator, run against the same
//! quantized inputs the player would dial in.
//!
//! A candidate survives if firing it would leave the target reading
//! `Collapsed`. With more than one target a single shell is not required to
//! clear the screen, so the requirement is per target: each standing structure needs at
//! least `MIN_SOLUTIONS` shots that would fell it.
//!
//! Each target is tried from three aim points ([`aim_points`]): the box
//! centre, which is the direct answer DESIGN §2.4 describes, plus the
//! footprint base and the upper box for the undermining and toppling kill
//! paths. Duplicate dialable shots are simulated once.
//!
//! Verification is deliberately expensive: every candidate that survives a
//! cheap proximity filter is applied to a clone of the screen with
//! [`Screen::apply_shot`], so the solver judges a shot by the game's own
//! damage model rather than a copy of it. [`validate`] also stops counting at
//! [`GATE_CAP`], which is all [`crate::run::generate_screen`] needs.
//!
//! The closed form is exact physics; the integrator is not. After time `t`,
//! semi-implicit Euler lands `½·g·t·DT` lower (and `½·w·t·DT` further
//! downwind) than the closed form, and quantizing angle and power adds more
//! error. The closed form also knows nothing of Storm Front, Airburst,
//! ricochet or tunnelling; only the re-simulation sees them. Hence every
//! candidate is re-simulated.

use crate::ballistics::{
    blast_radius, fire, power_ticks_for_speed, speed_bounds, End, FireOptions, Shot,
};
use crate::dmath;
use crate::modifiers::Loadout;
use crate::run::{Screen, StructState};
use crate::{Vec2, GRAVITY, MIN_SOLUTIONS, SOLVER_TIME_SAMPLES};

/// A shot the player could dial in that would fell a specific target.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Candidate {
    /// Already quantized to the loadout's input grid.
    pub shot: Shot,
    /// Index into the screen's structures.
    pub target: usize,
    /// Flight time of the closed-form solve that produced it.
    pub solved_time: f64,
    /// Flight time the integrator actually took.
    pub flight_time: f64,
}

/// The outcome of validating a screen.
///
/// Counts stop at the cap passed to [`validate_capped`], so under
/// [`validate`] they read at most [`GATE_CAP`]. A supply screen reports
/// `usize::MAX` for both worst counts; a screen that fails early pads the
/// remaining targets with 0.
#[derive(Clone, Debug, Default)]
pub struct SolveReport {
    /// Surviving-candidate count per structure, in structure order.
    pub per_target: Vec<usize>,
    /// Same, counting only the 40-sample sweep aimed at the box centre.
    pub per_target_direct: Vec<usize>,
    /// The hardest target's count. This is what the generator gates on.
    pub worst: usize,
    /// The hardest target's direct-aim count — the difficulty proxy DESIGN
    /// §2.4 asks to be logged. Out of at most `SOLVER_TIME_SAMPLES`, so it
    /// stays a readable number instead of saturating.
    pub worst_direct: usize,
    /// Every surviving candidate, grouped by target.
    pub candidates: Vec<Candidate>,
}

impl SolveReport {
    /// True when the hardest target has at least [`MIN_SOLUTIONS`] candidates.
    pub fn solvable(&self) -> bool {
        self.worst >= MIN_SOLUTIONS
    }
}

/// Closed-form candidate shots for a target, in loadout-quantized input space.
/// Duplicates (distinct flight times collapsing onto the same dialable input)
/// are dropped — three candidates must mean three distinct shots.
///
/// `origin` and `target` are world positions; `wind_accel` is the effective
/// wind in units/s², already scaled by the loadout. Each result pairs the
/// shot with the flight time, in seconds, that produced it. Quantizing angle
/// and power moves the shot slightly off the exact solution, which is why
/// every candidate is simulated before it counts.
pub fn analytic_shots(
    origin: Vec2,
    target: Vec2,
    wind_accel: f64,
    lo: &Loadout,
) -> Vec<(Shot, f64)> {
    let dx = target.x - origin.x;
    let dy = target.y - origin.y;
    let (_, v_max) = speed_bounds(lo);
    // Longest possible flight: straight up at full power.
    let t_max = 2.0 * v_max / GRAVITY + 0.5;
    // Keeps `t` clear of zero, where the solve below divides by it.
    let t_min = 0.15;

    let mut out: Vec<(Shot, f64)> = Vec::with_capacity(SOLVER_TIME_SAMPLES);
    for k in 0..SOLVER_TIME_SAMPLES {
        let f = k as f64 / (SOLVER_TIME_SAMPLES - 1) as f64;
        let t = t_min + (t_max - t_min) * f;
        let vx = (dx - 0.5 * wind_accel * t * t) / t;
        let vy = (dy + 0.5 * GRAVITY * t * t) / t;
        // The barrel elevates 0..90 degrees, so vx must be forward and vy
        // must not point down.
        if vx <= 0.0 || vy < 0.0 {
            continue;
        }
        let speed = dmath::hypot(vx, vy);
        let Some(power_ticks) = power_ticks_for_speed(speed, lo) else {
            continue;
        };
        let angle_ticks = lo.quantize_angle(dmath::angle_ticks_from_dir(vx, vy));
        let shot = Shot::new(angle_ticks, power_ticks).quantized(lo);
        if out.iter().any(|(s, _)| *s == shot) {
            continue;
        }
        out.push((shot, t));
    }
    out
}

/// Would this shot fell `target`? Simulated, then confirmed against the real
/// damage model on a scratch copy of the screen.
///
/// Returns the primary shell's flight time when it does. The shot is
/// simulated twice: once here for the cheap filter, and again inside
/// `apply_shot`.
fn fells_target(screen: &Screen, lo: &Loadout, shot: Shot, target: usize) -> Option<f64> {
    let boxes = screen.target_boxes();
    let opts = FireOptions {
        record_path: false,
        fuse: if lo.airburst() {
            screen.default_fuse(target)
        } else {
            None
        },
        wind_after_apex: if lo.storm_front() {
            screen.wind_late
        } else {
            None
        },
    };
    let traces = {
        let world = screen.world(&boxes);
        fire(&world, screen.muzzle(), shot, lo, &opts)
    };

    // Cheap rejection first: nothing that detonates far from the target box can
    // possibly fell it, and most candidates die here.
    let target_box = boxes[target];
    let near = traces.iter().any(|t| match t.end {
        End::Structure { index, .. } => index == target,
        End::Terrain { pos, .. } | End::Fuse { pos } => {
            target_box.distance_to(pos) < blast_radius(lo, t.damage_scale)
                // Undermining: a detonation within 8 units of the box's x-span
                // counts even when the blast never touches the structure
                // (DESIGN §2.7).
                || (pos.x >= target_box.x0 - 8.0 && pos.x <= target_box.x1 + 8.0)
        }
        _ => false,
    });
    if !near {
        return None;
    }

    // Confirm against the damage model itself rather than a re-derivation of
    // it, so solvability can never drift away from what the game does.
    let mut scratch = screen.clone();
    let report = scratch.apply_shot(shot, lo, screen.default_fuse(target), false);
    if scratch.structures[target].state == StructState::Collapsed {
        Some(report.traces[0].flight_time)
    } else {
        None
    }
}

/// The three points a candidate can be aimed at.
///
/// The box centre is the direct answer and the one DESIGN §2.4 describes. The
/// other two exist because there are two kill paths: the footprint base for
/// undermining, and just under the top for the top-heavy silhouettes that
/// topple from a high hit. They also carry `Overhang`, which by construction
/// has no direct answer at all.
///
/// Order is `[centre, base, top]`. `None` if `target` is out of range.
pub fn aim_points(screen: &Screen, target: usize) -> Option<[Vec2; 3]> {
    let s = screen.structures.get(target)?;
    let b = s.aabb();
    let c = b.centre();
    Some([c, Vec2::new(c.x, s.base + 1.0), Vec2::new(c.x, b.y1 - 2.0)])
}

/// Candidates from one aim point. `seen` carries the shots already simulated
/// for this target, so overlapping aim points cost nothing extra.
fn solve_from_aim(
    screen: &Screen,
    lo: &Loadout,
    target: usize,
    aim: Vec2,
    cap: usize,
    seen: &mut Vec<Shot>,
    out: &mut Vec<Candidate>,
) -> usize {
    let origin = screen.muzzle();
    let wind_accel = screen.wind * lo.wind_scale;
    let mut found = 0usize;
    for (shot, solved_time) in analytic_shots(origin, aim, wind_accel, lo) {
        if let Some(prior) = seen.iter().position(|s| *s == shot) {
            // Already simulated: count it here if it worked.
            let _ = prior;
            if out.iter().any(|c| c.shot == shot) {
                found += 1;
            }
            continue;
        }
        seen.push(shot);
        if let Some(flight_time) = fells_target(screen, lo, shot, target) {
            out.push(Candidate {
                shot,
                target,
                solved_time,
                flight_time,
            });
            found += 1;
            if out.len() >= cap {
                break;
            }
        }
    }
    found
}

/// Candidates that fell one target, and how many of them came from the direct
/// centre-of-box aim.
///
/// `cap` stops the search early once enough have been found. Screen generation
/// only needs to know the count clears `MIN_SOLUTIONS`; the batch harness
/// passes `usize::MAX` because it wants the whole histogram.
///
/// The centre aim runs first and the other two only if the cap is still
/// unmet, so the direct count can be cut short by the cap but never inflated
/// by the other aims.
pub fn solve_target_capped(
    screen: &Screen,
    lo: &Loadout,
    target: usize,
    cap: usize,
) -> (Vec<Candidate>, usize) {
    let Some(aims) = aim_points(screen, target) else {
        return (Vec::new(), 0);
    };
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: Vec<Shot> = Vec::new();
    let direct = solve_from_aim(screen, lo, target, aims[0], cap, &mut seen, &mut out);
    if out.len() < cap {
        for aim in &aims[1..] {
            solve_from_aim(screen, lo, target, *aim, cap, &mut seen, &mut out);
            if out.len() >= cap {
                break;
            }
        }
    }
    (out, direct)
}

/// Every candidate that fells one target, uncapped. Used by tests, the
/// Godot layer and `site_shot`.
pub fn solve_target(screen: &Screen, lo: &Loadout, target: usize) -> Vec<Candidate> {
    solve_target_capped(screen, lo, target, usize::MAX).0
}

/// Validate a whole screen. DESIGN §2.4: accept only if at least three
/// candidates survive — per target, since one shell is not expected to clear
/// a screen holding three structures.
///
/// Supply screens pass trivially. A screen with no structures reports zero
/// candidates and so is not solvable.
pub fn validate_capped(screen: &Screen, lo: &Loadout, cap: usize) -> SolveReport {
    if screen.supply {
        return SolveReport {
            per_target: Vec::new(),
            per_target_direct: Vec::new(),
            worst: usize::MAX,
            worst_direct: usize::MAX,
            candidates: Vec::new(),
        };
    }
    if screen.structures.is_empty() {
        return SolveReport::default();
    }

    let mut report = SolveReport {
        per_target: Vec::with_capacity(screen.structures.len()),
        per_target_direct: Vec::with_capacity(screen.structures.len()),
        worst: usize::MAX,
        worst_direct: usize::MAX,
        candidates: Vec::new(),
    };
    for i in 0..screen.structures.len() {
        let (cands, direct) = solve_target_capped(screen, lo, i, cap);
        report.per_target.push(cands.len());
        report.per_target_direct.push(direct);
        report.worst = report.worst.min(cands.len());
        report.worst_direct = report.worst_direct.min(direct);
        report.candidates.extend(cands);
        // Early out: a screen that has already failed does not need the rest.
        if report.worst < MIN_SOLUTIONS {
            while report.per_target.len() < screen.structures.len() {
                report.per_target.push(0);
                report.per_target_direct.push(0);
            }
            break;
        }
    }
    report
}

/// Gate used by screen generation. Stops counting at `GATE_CAP` candidates.
pub fn validate(screen: &Screen, lo: &Loadout) -> SolveReport {
    validate_capped(screen, lo, GATE_CAP)
}

/// Full count, for the batch harness histogram.
pub fn validate_full(screen: &Screen, lo: &Loadout) -> SolveReport {
    validate_capped(screen, lo, usize::MAX)
}

/// Enough headroom above `MIN_SOLUTIONS` to stay a useful signal while
/// keeping generation cheap.
pub const GATE_CAP: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::generate_screen;
    use crate::terrain::Template;

    #[test]
    fn the_closed_form_agrees_with_the_integrator() {
        // A candidate's solved flight time should be close to what the
        // fixed-step integration actually takes. If these drift apart the
        // closed form has the wrong physics in it.
        let lo = Loadout::new();
        let screen = generate_screen(0xABCD, 0, 320, &lo);
        let cands = solve_target(&screen, &lo, 0);
        assert!(!cands.is_empty());
        for c in &cands {
            let ratio = c.flight_time / c.solved_time;
            assert!(
                ratio > 0.75 && ratio < 1.35,
                "solved {:.3}s vs simulated {:.3}s",
                c.solved_time,
                c.flight_time
            );
        }
    }

    #[test]
    fn candidates_are_distinct_dialable_shots() {
        let lo = Loadout::new();
        let screen = generate_screen(77, 8, 320, &lo);
        let cands = solve_target(&screen, &lo, 0);
        for (i, a) in cands.iter().enumerate() {
            for b in &cands[i + 1..] {
                assert_ne!(a.shot, b.shot, "duplicate candidate shot");
            }
            // Every candidate is on the loadout's input grid.
            assert_eq!(a.shot, a.shot.quantized(&lo));
        }
    }

    #[test]
    fn wind_shifts_the_solution_set() {
        let lo = Loadout::new();
        let mut screen = generate_screen(5150, 40, 320, &lo);
        let calm: Vec<Shot> = {
            screen.wind = 0.0;
            solve_target(&screen, &lo, 0)
                .iter()
                .map(|c| c.shot)
                .collect()
        };
        screen.wind = 6.5;
        let windy: Vec<Shot> = solve_target(&screen, &lo, 0)
            .iter()
            .map(|c| c.shot)
            .collect();
        assert!(!calm.is_empty() && !windy.is_empty());
        assert_ne!(calm, windy, "wind did not change the answers");
    }

    #[test]
    fn every_generated_screen_is_solvable() {
        let lo = Loadout::new();
        for seed in 0..24u64 {
            for index in [0u32, 1, 7, 12, 30] {
                let s = generate_screen(seed * 7919 + 1, index, 320, &lo);
                let r = validate(&s, &lo);
                assert!(
                    r.solvable(),
                    "seed {seed} screen {index} ({}) only {} candidates",
                    s.template.name(),
                    r.worst
                );
            }
        }
    }

    #[test]
    fn overhang_is_not_offered_without_the_unlock() {
        let plain = Loadout::new();
        for seed in 0..40u64 {
            for index in [20u32, 24, 28, 40, 80, 120] {
                let s = generate_screen(seed * 131 + 3, index, 320, &plain);
                assert_ne!(
                    s.template,
                    Template::Overhang,
                    "Overhang offered to a loadout that cannot get into a pocket"
                );
            }
        }
    }

    #[test]
    fn overhang_appears_once_the_loadout_unlocks_it() {
        let mut lo = Loadout::new();
        lo.take(crate::modifiers::Modifier::Airburst);
        let mut seen = false;
        for seed in 0..80u64 {
            for index in [24u32, 40, 60, 96, 140] {
                let s = generate_screen(seed * 977 + 11, index, 320, &lo);
                if s.template == Template::Overhang {
                    seen = true;
                    assert!(validate(&s, &lo).solvable());
                }
            }
        }
        assert!(seen, "Overhang never appeared with airburst held");
    }
}

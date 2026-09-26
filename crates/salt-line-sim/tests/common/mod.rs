//! Shared by the integration tests: a scripted player that touches every
//! input a shot log records, and the screen set the digests pin.
#![allow(dead_code)] // each test binary uses a different part of this

use salt_line_sim::ballistics::Shot;
use salt_line_sim::dmath::MAX_ANGLE_TICKS;
use salt_line_sim::modifiers::Modifier;
use salt_line_sim::replay::{screen_hash, Replay, DRAFT_MARK};
use salt_line_sim::rng::Pcg32;
use salt_line_sim::run::{LoggedShot, Phase, RunState};
use salt_line_sim::solver::solve_target;
use salt_line_sim::POWER_TICKS_MAX;

/// One player decision. How it applies depends on the phase it lands in: a
/// shot while firing, a pick while drafting.
///
/// A shot is either a random one or, when `solve` is set, a solver candidate
/// for the first standing target, so runs actually clear screens and reach
/// drafts. Before a shot the decision may also turn the Airburst fuse or move
/// the emplacement (Enfilade), the two inputs besides the shot itself that
/// change its outcome. Drafts take Airburst or Enfilade whenever they are on
/// offer, so those inputs matter in as many runs as possible.
#[derive(Clone, Debug)]
pub struct Decision {
    /// Random aim, angle ticks. Used when `solve` is off or finds nothing.
    pub angle: i32,
    /// Random aim, power ticks.
    pub power: i32,
    /// Offer index to take when neither Airburst nor Enfilade is offered.
    pub pick: usize,
    /// Fire a solver candidate instead of the random aim.
    pub solve: bool,
    /// Fuse to set before the shot, in world units, if any.
    pub fuse: Option<f64>,
    /// Enfilade offset to request before the shot, in world units, if any.
    pub reposition: Option<f64>,
}

/// `n` decisions drawn from a seeded generator, with the same shape the
/// property test's strategy produces. Used where a run has to be reproducible
/// from a number, like the golden-run corpus. `skill` is the share of shots
/// that are solver candidates rather than random.
pub fn seeded_decisions(seed: u64, n: usize, skill: f64) -> Vec<Decision> {
    let mut r = Pcg32::new(seed);
    (0..n)
        .map(|_| Decision {
            angle: r.range_u32(0, MAX_ANGLE_TICKS as u32) as i32,
            power: r.range_u32(0, POWER_TICKS_MAX as u32) as i32,
            pick: r.range_u32(0, 2) as usize,
            solve: r.next_f64() < skill,
            fuse: (r.next_f64() < 0.4).then(|| r.range_f64(-20.0, 500.0)),
            reposition: (r.next_f64() < 0.4).then(|| r.range_f64(-40.0, 40.0)),
        })
        .collect()
}

/// The shot a decision fires: a solver candidate or its own random aim.
fn aim(state: &mut RunState, d: &Decision) -> Shot {
    let random = Shot::new(d.angle, d.power);
    if !d.solve {
        return random;
    }
    let Some(target) = state.screen.structures.iter().position(|s| !s.is_down()) else {
        return random;
    };
    if let Some(f) = state.screen.default_fuse(target) {
        state.set_fuse(f);
    }
    solve_target(&state.screen, &state.loadout, target)
        .first()
        .map_or(random, |c| c.shot)
}

/// Play the decisions, checking the run's invariants after every step.
///
/// Drives the run the way a headless caller must: every `fire` is followed
/// by `resolve_clear`, so `Phase::Cleared` is never left waiting. Stops
/// early when the run ends or a shot is refused.
pub fn play(seed: u64, width: usize, moves: &[Decision]) -> RunState {
    let mut state = RunState::new(seed, width);
    for d in moves {
        let before = (state.screen_index, state.log.len());
        match state.phase {
            Phase::Firing => {
                // Move first, since a solver shot's fuse depends on where the
                // muzzle is.
                if let Some(off) = d.reposition {
                    state.reposition(off);
                }
                let shot = aim(&mut state, d);
                if let Some(f) = d.fuse {
                    state.set_fuse(f);
                }
                if state.fire(shot, false).is_none() {
                    break;
                }
                state.resolve_clear();
            }
            Phase::Drafting => {
                let wanted = state
                    .offers
                    .iter()
                    .position(|&m| m == Modifier::Airburst || m == Modifier::Enfilade);
                let pick = wanted.unwrap_or(d.pick.min(state.offers.len().saturating_sub(1)));
                if state.take_offer(pick).is_none() {
                    state.skip_draft();
                }
            }
            Phase::Cleared => {
                state.resolve_clear();
            }
            Phase::Over(_) => break,
        }
        assert!(state.shells >= 0, "shells went negative: {}", state.shells);
        assert!(
            state.screen_index >= before.0,
            "the screen index went backwards"
        );
        assert!(state.log.len() >= before.1, "the log lost entries");
    }
    state
}

/// The screens whose digests are pinned: a few seeds, both extreme widths,
/// early screens and two deeper bands where screens carry several targets.
pub fn pinned_screens() -> impl Iterator<Item = (u64, usize, u32)> {
    [1u64, 0xFEED_FACE, 20260907].into_iter().flat_map(|seed| {
        [320usize, 400].into_iter().flat_map(move |width| {
            (0..12)
                .chain(40..44)
                .chain(120..124)
                .map(move |index| (seed, width, index))
        })
    })
}

/// What a run's outcome is, leaving out the log itself (which records the
/// inputs, so it always differs when they do).
fn outcome(s: &RunState) -> (u64, i32, u32, u32, u32) {
    (
        screen_hash(&s.screen),
        s.shells,
        s.screen_index,
        s.screens_cleared,
        s.total_shots,
    )
}

/// Whether changing one logged input on some shot from `from` on changes
/// how the run turns out (or stops it replaying at all). `eligible` picks the
/// shots; `mutate` changes the entry. Stops at the first that does.
fn some_input_matters(
    replay: &Replay,
    from: usize,
    eligible: impl Fn(&LoggedShot) -> bool,
    mutate: impl Fn(&mut LoggedShot),
) -> bool {
    let Ok(base) = replay.reconstruct() else {
        return false;
    };
    let base = outcome(&base);
    (from..replay.log.len()).any(|i| {
        let entry = replay.log[i];
        if entry.angle_ticks == DRAFT_MARK || !eligible(&entry) {
            return false;
        }
        let mut changed = replay.clone();
        mutate(&mut changed.log[i]);
        changed.reconstruct().map(|s| outcome(&s)) != Ok(base)
    })
}

/// Index of the first log entry after which the loadout has Airburst, found
/// by bisection (a modifier, once drafted, stays).
fn airburst_from(replay: &Replay) -> Option<usize> {
    let has = |n: usize| {
        Replay {
            log: replay.log[..n].to_vec(),
            ..replay.clone()
        }
        .reconstruct()
        .is_ok_and(|s| s.loadout.airburst())
    };
    let (mut lo, mut hi) = (0, replay.log.len());
    if !has(hi) {
        return None;
    }
    while lo < hi {
        let mid = (lo + hi) / 2;
        if has(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

/// A shot fired with Airburst in the loadout whose fuse decides the outcome:
/// halve it and the run turns out differently. Every shot logs a fuse, so
/// counting distinct fuse values proves nothing; this does.
pub fn fuse_matters(replay: &Replay) -> bool {
    airburst_from(replay).is_some_and(|from| {
        some_input_matters(
            replay,
            from,
            |_| true,
            |e| {
                e.fuse_tenths = if e.fuse_tenths > 1 {
                    e.fuse_tenths / 2
                } else {
                    500
                }
            },
        )
    })
}

/// A shot fired from a moved emplacement whose offset decides the outcome:
/// fire it from the default position instead and the run turns out
/// differently.
pub fn offset_matters(replay: &Replay) -> bool {
    some_input_matters(
        replay,
        0,
        |e| e.emplacement_offset != 0,
        |e| e.emplacement_offset = 0,
    )
}

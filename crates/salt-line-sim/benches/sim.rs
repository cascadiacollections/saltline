//! Benchmarks (divan): `cargo bench -p salt-line-sim`.
//!
//! The costs that matter are the ones a phone pays between shots: generating
//! the next screen (terrain, placement and the solvability search that proves
//! it clearable, DESIGN §2.4), resolving a shot, and rebuilding a run from its
//! log. Screens are generated at several depths because later bands try more
//! templates and harder shapes before one passes.
//!
//! Benchmarks catch what the tests cannot: a change that keeps every output
//! identical but makes it slower. They assert nothing; compare runs by hand
//! before and after a change to generation, the solver or the integrator.
//! Inputs go through `black_box` so the optimiser cannot fold a constant
//! seed into a precomputed result.

use divan::{black_box, Bencher};
use salt_line_sim::ballistics::{fire, FireOptions, Shot};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::Replay;
use salt_line_sim::run::{generate_screen, Phase, RunState};
use salt_line_sim::solver;

fn main() {
    divan::main();
}

const SEED: u64 = 20260907;

/// Screen index by band: the first screen, then deeper into the run.
#[divan::bench(args = [0, 12, 40, 120])]
fn generate_a_screen(bencher: Bencher, index: u32) {
    let lo = Loadout::default();
    bencher.bench(|| generate_screen(black_box(SEED), black_box(index), 320, &lo));
}

/// The solvability proof on its own, over an already generated screen.
#[divan::bench(args = [0, 12, 40, 120])]
fn prove_a_screen_solvable(bencher: Bencher, index: u32) {
    let lo = Loadout::default();
    let screen = generate_screen(SEED, index, 320, &lo);
    bencher.bench(|| solver::validate(black_box(&screen), &lo));
}

/// One shot, with and without the recorded path the renderer draws.
#[divan::bench(args = [false, true])]
fn resolve_a_shot(bencher: Bencher, record_path: bool) {
    let lo = Loadout::default();
    let screen = generate_screen(SEED, 12, 320, &lo);
    let boxes = screen.target_boxes();
    let world = screen.world(&boxes);
    let opts = FireOptions {
        record_path,
        ..FireOptions::default()
    };
    bencher.bench(|| {
        fire(
            &world,
            screen.muzzle(),
            black_box(Shot::new(900, 600)),
            &lo,
            &opts,
        )
    });
}

/// Rebuilding a whole run from its seed and shot log, which is what the
/// determinism gate does, and what watching or resuming a run would do
/// (DESIGN §9.6 puts a 60-shot rebuild at about a quarter of a millisecond).
/// Sharing a seed alone does not replay anything; it only regenerates.
#[divan::bench]
fn replay_a_run(bencher: Bencher) {
    let mut state = RunState::new(SEED, 320);
    let mut angle = 700;
    for _ in 0..60 {
        match state.phase {
            Phase::Firing => {
                angle = 500 + (angle + 137) % 1100;
                if state.fire(Shot::new(angle, 650), false).is_none() {
                    break;
                }
                state.resolve_clear();
            }
            Phase::Drafting => {
                if state.take_offer(0).is_none() {
                    state.skip_draft();
                }
            }
            Phase::Cleared => {
                state.resolve_clear();
            }
            Phase::Over(_) => break,
        }
    }
    let replay = Replay::of(&state);
    bencher.bench(|| black_box(&replay).reconstruct().expect("replay"));
}

//! Cross-platform determinism check. Brief M4 acceptance.
//!
//! Plays 200 runs from fixed seeds with a deterministic input generator,
//! replays each from `(run_seed, shot log)` alone, and asserts the
//! reconstruction hashes match. Then prints one digest over all 200 hashes.
//!
//! CI runs this on Linux, macOS and Windows and compares the digest. If the
//! digests differ there is a determinism leak — a libm call, a `HashMap`
//! iteration, a variable timestep — and it must be fixed rather than papered
//! over with a tolerance.
//!
//! ```text
//! cargo run --release -p salt-line-sim --bin determinism
//! cargo run --release -p salt-line-sim --bin determinism -- --expect 0x...
//! ```

use salt_line_sim::ballistics::Shot;
use salt_line_sim::replay::{state_hash, Hasher, Replay};
use salt_line_sim::rng::Pcg32;
use salt_line_sim::run::{Phase, RunState};

const RUNS: u64 = 200;
const DECISIONS: usize = 160;
const WIDTH: usize = 320;

/// Deterministic stand-in for a player. Not a good one — the point is to
/// touch drafts, misses, hits, collapses and the reserve running dry.
fn play(run_seed: u64) -> RunState {
    let mut state = RunState::new(run_seed, WIDTH);
    let mut r = Pcg32::new(run_seed ^ 0xB0A7);
    for _ in 0..DECISIONS {
        match state.phase {
            Phase::Firing => {
                let angle = r.range_u32(150, 800) as i32;
                let power = r.range_u32(250, 1000) as i32;
                if state.fire(Shot::new(angle, power), false).is_none() {
                    break;
                }
            }
            Phase::Cleared => {
                state.resolve_clear();
            }
            Phase::Drafting => {
                let n = state.offers.len();
                if n == 0 {
                    state.skip_draft();
                } else {
                    state.take_offer(r.range_u32(0, n as u32 - 1) as usize);
                }
            }
            Phase::Over(_) => break,
        }
    }
    state
}

fn main() {
    let mut expect: Option<u64> = None;
    let mut verbose = false;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--expect" => {
                i += 1;
                let v = argv.get(i).cloned().unwrap_or_default();
                let t = v.trim_start_matches("0x");
                expect = u64::from_str_radix(t, 16).ok();
                if expect.is_none() {
                    eprintln!("--expect wants a hex digest, got '{v}'");
                    std::process::exit(2);
                }
            }
            "--verbose" | "-v" => verbose = true,
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    println!("salt-line determinism: {RUNS} runs, up to {DECISIONS} decisions each, width {WIDTH}");
    println!("target: {}", std::env::consts::ARCH);
    println!("os:     {}", std::env::consts::OS);

    let mut digest = Hasher::new();
    let mut diverged = 0u64;
    let mut shots = 0usize;
    let mut screens = 0u32;

    for n in 0..RUNS {
        let run_seed = 0x5A17_0000_0000_0000 ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let played = play(run_seed);
        let played_hash = state_hash(&played);
        let replay = Replay::of(&played);
        match replay.final_hash() {
            Ok(h) if h == played_hash => {}
            Ok(h) => {
                diverged += 1;
                eprintln!(
                    "run {n} (seed {run_seed:#018x}): replay {h:#018x} != live {played_hash:#018x}"
                );
            }
            Err(e) => {
                diverged += 1;
                eprintln!("run {n} (seed {run_seed:#018x}): replay failed: {e:?}");
            }
        }
        shots += played.log.len();
        screens += played.screens_cleared;
        digest.write_u64(played_hash);
        if verbose {
            println!(
                "  run {n:>3} seed {run_seed:#018x} screens {:>3} shells {:>3} hash {played_hash:#018x}",
                played.screens_cleared, played.shells
            );
        }
    }

    let digest = digest.finish();
    println!("\nlogged decisions: {shots}, screens cleared: {screens}");
    println!("replay divergences: {diverged}");
    println!("digest: {digest:#018x}");

    if diverged > 0 {
        std::process::exit(1);
    }
    if let Some(want) = expect {
        if want != digest {
            eprintln!("digest mismatch: expected {want:#018x}, got {digest:#018x}");
            std::process::exit(1);
        }
        println!("digest matches the expected value");
    }
}

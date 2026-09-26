//! Golden runs and the sim epoch. DESIGN §9.3, §9.5.
//!
//! `golden_runs.txt` holds recorded runs, each with the `state_hash` its
//! replay must reach. They are the regression corpus for replays: a sim
//! change that alters any of them breaks every saved log, which is what
//! `SIM_EPOCH` exists to say.
//!
//! Regenerate the corpus, only alongside a `SIM_EPOCH` bump, with:
//!
//! ```text
//! cargo test -p salt-line-sim --test golden_runs -- --ignored regenerate
//! ```
//!
//! which rewrites the file and prints the new `EPOCH_DIGEST`.

mod common;

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::{decode_log, encode_log, screen_hash, state_hash, Hasher, Replay};
use salt_line_sim::run::{generate_screen, Phase, RunState};
use salt_line_sim::{band_of, EPOCH_DIGEST, SIM_EPOCH};

const CORPUS: &str = include_str!("golden_runs.txt");

/// The recorded runs: `(run seed, width, decisions, skill)`, played by the
/// scripted player in `common`. Chosen to span both extreme widths and an odd
/// one, short and long runs, deep bands, a run that runs dry, and runs that
/// draft Airburst and Enfilade and use them; `the_corpus_covers_what_it_should`
/// holds it to that.
const SPECS: [(u64, usize, usize, f64); 8] = [
    (0x0001, 320, 12, 0.7),
    (0x0002, 400, 40, 0.7),
    (0x0003, 357, 60, 0.8),
    (0x0004, 320, 90, 0.9),
    (0x0005, 400, 90, 0.9),
    (0x0006, 320, 140, 0.95),
    (0x0007, 333, 120, 0.9),
    (0x0008, 320, 120, 0.2),
];

struct Golden {
    line: usize,
    replay: Replay,
    hash: u64,
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// Lines are `<seed> <width> <hash> <log…>`; `#` starts a comment.
fn parse_line(l: &str) -> Option<(Replay, u64)> {
    let mut parts = l.splitn(4, ' ');
    let run_seed = hex(parts.next()?)?;
    let width = parts.next()?.parse().ok()?;
    let hash = hex(parts.next()?)?;
    let log = decode_log(parts.next().unwrap_or(""))?;
    Some((
        Replay {
            run_seed,
            width,
            log,
        },
        hash,
    ))
}

fn corpus() -> Vec<Golden> {
    CORPUS
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(i, l)| {
            let (replay, hash) = parse_line(l)
                .unwrap_or_else(|| panic!("golden_runs.txt:{}: malformed line", i + 1));
            Golden {
                line: i + 1,
                replay,
                hash,
            }
        })
        .collect()
}

fn replayed(g: &Golden) -> RunState {
    g.replay
        .reconstruct()
        .unwrap_or_else(|e| panic!("golden_runs.txt:{}: {e:?}", g.line))
}

#[test]
fn golden_runs_reproduce() {
    let runs = corpus();
    assert_eq!(runs.len(), SPECS.len(), "corpus and SPECS disagree");
    for g in &runs {
        assert_eq!(
            state_hash(&replayed(g)),
            g.hash,
            "golden_runs.txt:{}: the run replays to a different state. If the \
             sim change is intended, bump SIM_EPOCH and regenerate (see the top \
             of tests/golden_runs.rs).",
            g.line
        );
    }
}

#[test]
fn the_corpus_covers_what_it_should() {
    let runs: Vec<_> = corpus().iter().map(replayed).collect();
    let shots = |s: &RunState| {
        s.log
            .iter()
            .filter(|e| e.angle_ticks >= 0)
            .copied()
            .collect::<Vec<_>>()
    };
    let fuse_settings = |s: &RunState| {
        let mut f: Vec<_> = shots(s).iter().map(|e| e.fuse_tenths).collect();
        f.sort_unstable();
        f.dedup();
        f.len()
    };
    assert!(
        runs.iter()
            .any(|s| s.loadout.airburst() && fuse_settings(s) >= 3),
        "no golden run uses the Airburst fuse"
    );
    assert!(
        runs.iter()
            .any(|s| shots(s).iter().any(|e| e.emplacement_offset != 0)),
        "no golden run moves the emplacement"
    );
    assert!(
        runs.iter().any(|s| band_of(s.screen_index) >= 4),
        "no golden run reaches band 4"
    );
    assert!(
        runs.iter().any(|s| matches!(s.phase, Phase::Over(_))),
        "no golden run runs out of shells"
    );
    for w in [320, 400] {
        assert!(runs.iter().any(|s| s.width == w), "no run at width {w}");
    }
}

/// Everything a replay's outcome depends on, in one number: the pinned
/// screen digests and the golden runs' final state hashes.
fn epoch_digest(run_hashes: impl IntoIterator<Item = u64>) -> u64 {
    let lo = Loadout::default();
    let mut h = Hasher::new();
    for (seed, width, index) in common::pinned_screens() {
        h.write_u64(screen_hash(&generate_screen(seed, index, width, &lo)));
    }
    for hash in run_hashes {
        h.write_u64(hash);
    }
    h.finish()
}

#[test]
fn sim_epoch_matches_its_digest() {
    // The reconstructed states, not the recorded hashes: the digest must
    // move when the sim does, even if the corpus was regenerated.
    let digest = epoch_digest(corpus().iter().map(|g| state_hash(&replayed(g))));
    assert!(
        digest == EPOCH_DIGEST,
        "the sim's outcomes changed under SIM_EPOCH {SIM_EPOCH}: the digest over \
         the pinned screens and golden runs is {digest:#018x}, not \
         EPOCH_DIGEST {EPOCH_DIGEST:#018x}.\n\
         Saved runs from this epoch would replay differently. If the change is \
         intended: bump SIM_EPOCH, regenerate golden_runs.txt (see the top of \
         tests/golden_runs.rs), set EPOCH_DIGEST to the new value, and note the \
         bump and its reason in NOTES.md. If it is not, it is a determinism bug."
    );
}

/// Rewrites `golden_runs.txt` from `SPECS` and prints the new digest.
#[test]
#[ignore = "rewrites tests/golden_runs.txt; run by hand alongside a SIM_EPOCH bump"]
fn regenerate() {
    let mut out = String::from(HEADER);
    let mut hashes = Vec::new();
    for &(seed, width, n, skill) in &SPECS {
        let decisions = common::seeded_decisions(seed ^ 0x601D, n, skill);
        let played = common::play(seed, width, &decisions);
        let hash = state_hash(&played);
        assert_eq!(Replay::of(&played).final_hash(), Ok(hash), "seed {seed:#x}");
        hashes.push(hash);
        out.push_str(&format!(
            "{seed:#x} {width} {hash:#018x} {}\n",
            encode_log(&played.log)
        ));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden_runs.txt");
    std::fs::write(path, out).expect("writing golden_runs.txt");
    println!("wrote {path}");
    println!("EPOCH_DIGEST = {:#018x}", epoch_digest(hashes));
}

const HEADER: &str = "\
# Golden runs: recorded runs and the state_hash each must replay to.
# Checked by tests/golden_runs.rs, and folded into EPOCH_DIGEST (see
# SIM_EPOCH in src/lib.rs). Generated; do not edit by hand. Regenerate,
# only alongside a SIM_EPOCH bump, with
#
#   cargo test -p salt-line-sim --test golden_runs -- --ignored regenerate
#
# One run per line: <seed> <width> <final state_hash> <log>
#
# The log is space-separated tokens (DESIGN §9.2, plus F and E):
#   @N    the screen index for the entries after it
#   A:P   a shot at angle ticks A, power ticks P
#   DN    a pick of draft offer N
#   FT    the Airburst fuse, in tenths of a unit, for the shots after it
#   EO    the Enfilade emplacement offset, whole units, for the shots after it
# @, F and E are sticky and written only when they change; E starts at 0.
";

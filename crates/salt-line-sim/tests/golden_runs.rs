//! Golden runs and the sim epoch. DESIGN §9.3, §9.5.
//!
//! `golden_runs.txt` holds recorded runs, each with the epoch it was recorded
//! under and the `state_hash` its replay must reach. They are the regression
//! corpus for replays: a sim change that alters any of them breaks every
//! saved log, which is what `SIM_EPOCH` exists to say.
//!
//! Regenerate the corpus, only alongside a `SIM_EPOCH` bump, with:
//!
//! ```text
//! cargo test -p salt-line-sim --test golden_runs -- --ignored regenerate
//! ```
//!
//! which rewrites the file and prints the digest to append to `EPOCHS`.

mod common;

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::{decode_log, encode_log, screen_hash, state_hash, Hasher, Replay};
use salt_line_sim::run::{generate_screen, Phase, RunState};
use salt_line_sim::{band_of, EPOCHS, EPOCH_DIGEST, SIM_EPOCH};

const CORPUS: &str = include_str!("golden_runs.txt");
const NOTES: &str = include_str!("../../../NOTES.md");

/// The recorded runs: `(run seed, width, decisions, skill)`, played by the
/// scripted player in `common`. Chosen to span both extreme widths and odd
/// ones, short and long runs, deep bands, a run that runs dry, and runs that
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
    epoch: u32,
    replay: Replay,
    hash: u64,
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// Lines are `<epoch> <seed> <width> <hash> <log…>`; `#` starts a comment.
fn parse_line(l: &str) -> Result<(u32, Replay, u64), String> {
    let mut parts = l.splitn(5, ' ');
    let mut field = |name: &str| parts.next().ok_or(format!("no {name}"));
    let epoch = field("epoch")?.parse().map_err(|_| "bad epoch")?;
    let run_seed = hex(field("seed")?).ok_or("bad seed")?;
    let width = field("width")?.parse().map_err(|_| "bad width")?;
    let hash = hex(field("hash")?).ok_or("bad hash")?;
    let log = decode_log(field("log")?).map_err(|e| format!("log: {e:?}"))?;
    Ok((
        epoch,
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
            let (epoch, replay, hash) =
                parse_line(l).unwrap_or_else(|e| panic!("golden_runs.txt:{}: {e}", i + 1));
            Golden {
                line: i + 1,
                epoch,
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
            g.epoch, SIM_EPOCH,
            "golden_runs.txt:{}: recorded under epoch {}, but SIM_EPOCH is \
             {SIM_EPOCH}. Regenerate the corpus (see the top of \
             tests/golden_runs.rs).",
            g.line, g.epoch
        );
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
    let runs = corpus();
    let states: Vec<_> = runs.iter().map(replayed).collect();
    assert!(
        runs.iter().any(|g| common::fuse_matters(&g.replay)),
        "no golden run has a shot whose Airburst fuse decides its outcome"
    );
    assert!(
        runs.iter().any(|g| common::offset_matters(&g.replay)),
        "no golden run has a shot whose Enfilade offset decides its outcome"
    );
    assert!(
        states.iter().any(|s| band_of(s.screen_index) >= 4),
        "no golden run reaches band 4"
    );
    assert!(
        states.iter().any(|s| matches!(s.phase, Phase::Over(_))),
        "no golden run runs out of shells"
    );
    for w in [320, 400] {
        assert!(states.iter().any(|s| s.width == w), "no run at width {w}");
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

/// `EPOCHS` is append-only history, so a digest cannot be quietly replaced
/// under the same epoch: the current epoch is the last entry, every epoch
/// has one digest, no digest serves two epochs, and each is written down in
/// NOTES.md with the reason for the bump.
#[test]
fn the_epoch_history_is_consistent() {
    let (last, digest) = *EPOCHS.last().expect("EPOCHS is empty");
    assert_eq!(
        last, SIM_EPOCH,
        "the last EPOCHS entry is epoch {last}, but SIM_EPOCH is {SIM_EPOCH}"
    );
    assert_eq!(digest, EPOCH_DIGEST);
    for w in EPOCHS.windows(2) {
        assert!(w[0].0 < w[1].0, "EPOCHS out of order at epoch {}", w[1].0);
    }
    for (i, &(e, d)) in EPOCHS.iter().enumerate() {
        assert!(
            EPOCHS[..i].iter().all(|&(_, other)| other != d),
            "epoch {e} reuses an earlier epoch's digest"
        );
        assert!(
            NOTES.contains(&format!("{d:#018x}")),
            "NOTES.md does not record epoch {e}'s digest {d:#018x}"
        );
    }
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
         {EPOCH_DIGEST:#018x}.\n\
         Saved runs from this epoch would replay differently. If the change is \
         intended: bump SIM_EPOCH, regenerate golden_runs.txt (see the top of \
         tests/golden_runs.rs), append (SIM_EPOCH, new digest) to EPOCHS in \
         src/lib.rs -- never edit an existing entry -- and note the bump, its \
         reason and the digest in NOTES.md. If it is not intended, it is a \
         determinism bug."
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
            "{SIM_EPOCH} {seed:#x} {width} {hash:#018x} {}\n",
            encode_log(&played.log)
        ));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden_runs.txt");
    std::fs::write(path, out).expect("writing golden_runs.txt");
    println!("wrote {path}");
    println!(
        "digest for epoch {SIM_EPOCH}: {:#018x}",
        epoch_digest(hashes)
    );
}

const HEADER: &str = "\
# Golden runs: recorded runs and the state_hash each must replay to.
# Checked by tests/golden_runs.rs, and folded into the epoch digest (see
# SIM_EPOCH and EPOCHS in src/lib.rs). Generated; do not edit by hand.
# Regenerate, only alongside a SIM_EPOCH bump, with
#
#   cargo test -p salt-line-sim --test golden_runs -- --ignored regenerate
#
# One run per line: <epoch> <seed> <width> <final state_hash> <log>
#
# The log is replay::encode_log's text form (DESIGN §9.2, plus v, F and E):
#   v2    the format version, always first
#   @N    the screen index for the entries after it
#   A:P   a shot at angle ticks A, power ticks P
#   DN    a pick of draft offer N
#   FT    the Airburst fuse, in tenths of a unit, for the shots after it
#   EO    the Enfilade emplacement offset, whole units, for the shots after it
# @, F and E are sticky, written in that order and only when they change.
# The first shot always has an F; E starts at 0.
";

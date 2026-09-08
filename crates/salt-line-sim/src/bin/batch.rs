//! Batch solvability harness. Brief M2 acceptance.
//!
//! Generates screens across many seeds, asserts every one is solvable, and
//! prints a histogram of surviving-candidate counts bucketed by band. This is
//! how DESIGN §2.5 gets tuned without playing the game a thousand times: change
//! a constant, re-run, read the histogram.
//!
//! ```text
//! cargo run --release -p salt-line-sim --bin batch
//! cargo run --release -p salt-line-sim --bin batch -- --seeds 50 --screens 100 --build demo
//! ```
//!
//! Exit status is non-zero if any screen failed to validate, so CI can gate on it.

use salt_line_sim::modifiers::{draft_offers, Loadout, Modifier};
use salt_line_sim::rng::screen_seed;
use salt_line_sim::run::{generate_screen, Screen};
use salt_line_sim::solver;
use salt_line_sim::terrain::Template;
use salt_line_sim::{band_of, BAND_LEN, MIN_SOLUTIONS, SCREEN_W_REF};

/// Which loadout the screens are validated against. Solvability is a property
/// of the screen *and* the loadout, so the harness has to pick one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Build {
    /// Nothing drafted. The strictest case for the generator.
    Base,
    /// Takes a seeded offer every band, the way a run actually goes.
    Evolving,
    Drift,
    Demo,
    Precision,
    Risk,
}

impl Build {
    fn parse(s: &str) -> Option<Build> {
        Some(match s {
            "base" => Build::Base,
            "evolving" => Build::Evolving,
            "drift" => Build::Drift,
            "demo" => Build::Demo,
            "precision" => Build::Precision,
            "risk" => Build::Risk,
        _ => return None,
        })
    }

    /// The intended build shapes from DESIGN §4.
    fn fixed_picks(self) -> &'static [Modifier] {
        match self {
            Build::Drift => &[Modifier::Ballast, Modifier::Anemometer, Modifier::Vernier],
            Build::Demo => &[Modifier::Auger, Modifier::Cluster, Modifier::ThermalLance],
            Build::Precision => &[Modifier::Sabot, Modifier::Rangefinder, Modifier::ChalkMarks],
            Build::Risk => &[
                Modifier::Featherweight,
                Modifier::DeadReckoning,
                Modifier::ForcedMarch,
            ],
            _ => &[],
        }
    }
}

struct Args {
    seeds: u64,
    screens: u32,
    width: usize,
    build: Build,
    /// Cap the candidate count instead of counting them all. Much faster, but
    /// the histogram saturates.
    fast: bool,
    verbose: bool,
}

impl Default for Args {
    fn default() -> Self {
        // 50 seeds x 100 screens = 5,000 screens, per the brief.
        Args {
            seeds: 50,
            screens: 100,
            width: SCREEN_W_REF,
            build: Build::Evolving,
            fast: false,
            verbose: false,
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let mut value = || {
            i += 1;
            argv.get(i).cloned().unwrap_or_default()
        };
        match flag {
            "--seeds" => a.seeds = value().parse().unwrap_or(a.seeds),
            "--screens" => a.screens = value().parse().unwrap_or(a.screens),
            "--width" => a.width = value().parse().unwrap_or(a.width),
            "--build" => {
                let v = value();
                a.build = Build::parse(&v).unwrap_or_else(|| {
                    eprintln!("unknown build '{v}'; use base|evolving|drift|demo|precision|risk");
                    std::process::exit(2)
                });
            }
            "--fast" => a.fast = true,
            "--verbose" | "-v" => a.verbose = true,
            "--help" | "-h" => {
                println!(
                    "batch [--seeds N] [--screens N] [--width W] [--build BUILD] [--fast] [-v]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    a
}

/// One bucket of the histogram.
#[derive(Clone)]
struct Bucket {
    screens: u64,
    failures: u64,
    fallbacks: u64,
    attempts: u64,
    candidate_sum: u64,
    /// Histogram of direct-aim candidate counts. See `BUCKET_LABELS`.
    counts: [u64; 13],
    targets: u64,
    min_direct: usize,
}

impl Default for Bucket {
    fn default() -> Self {
        Bucket {
            screens: 0,
            failures: 0,
            fallbacks: 0,
            attempts: 0,
            candidate_sum: 0,
            counts: [0; 13],
            targets: 0,
            min_direct: usize::MAX,
        }
    }
}

/// Fine buckets where it matters (the gate is at 3) and coarse above.
const BUCKET_LABELS: [&str; 13] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8-9", "10-14", "15-19", "20-29", "30+",
];

fn bucket_of(n: usize) -> usize {
    match n {
        0..=7 => n,
        8..=9 => 8,
        10..=14 => 9,
        15..=19 => 10,
        20..=29 => 11,
        _ => 12,
    }
}

impl Bucket {
    fn record(&mut self, screen: &Screen, worst: usize, direct: usize) {
        self.screens += 1;
        self.attempts += screen.attempts as u64;
        self.targets += screen.structures.len() as u64;
        if screen.fell_back {
            self.fallbacks += 1;
        }
        if worst < MIN_SOLUTIONS {
            self.failures += 1;
        }
        self.candidate_sum += direct as u64;
        self.counts[bucket_of(direct)] += 1;
        if direct < self.min_direct {
            self.min_direct = direct;
        }
    }

    fn mean_candidates(&self) -> f64 {
        if self.screens == 0 {
            0.0
        } else {
            self.candidate_sum as f64 / self.screens as f64
        }
    }
}

/// Loadout for a screen index under the chosen build.
fn loadout_for(build: Build, run_seed: u64, screen_index: u32) -> Loadout {
    let mut lo = Loadout::new();
    match build {
        Build::Base => {}
        Build::Evolving => {
            // One seeded pick per completed band, mirroring `RunState`.
            let bands = screen_index / BAND_LEN;
            for b in 1..=bands {
                let at = b * BAND_LEN;
                let offers = draft_offers(screen_seed(run_seed, at), &lo);
                if let Some(m) = offers.first() {
                    lo.take(*m);
                }
            }
        }
        fixed => {
            // Stack the build's picks in as the bands go by.
            let bands = (screen_index / BAND_LEN) as usize;
            for (i, m) in fixed.fixed_picks().iter().enumerate() {
                if bands > i {
                    lo.take(*m);
                }
            }
        }
    }
    lo
}

fn bar(n: u64, max: u64, width: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let filled = ((n as f64 / max as f64) * width as f64).round() as usize;
    let mut s = String::with_capacity(width);
    for i in 0..width {
        s.push(if i < filled { '#' } else { ' ' });
    }
    s
}

fn main() {
    let args = parse_args();
    let total = args.seeds * args.screens as u64;
    println!(
        "salt-line batch: {} seeds x {} screens = {total} screens, width {}, build {:?}{}",
        args.seeds,
        args.screens,
        args.width,
        args.build,
        if args.fast { ", fast" } else { "" }
    );
    println!("gate: worst-target candidates >= {MIN_SOLUTIONS}\n");

    let mut by_band: Vec<Bucket> = Vec::new();
    let mut by_template = [Bucket::default(); 0].to_vec();
    by_template.resize(Template::ALL.len(), Bucket::default());
    let mut failures: Vec<(u64, u32, &'static str, usize)> = Vec::new();
    let mut supply_screens = 0u64;
    let started = std::time::Instant::now();

    for s in 0..args.seeds {
        let run_seed = salt_line_sim::rng::mix64(0x5341_4C54 ^ s.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for index in 0..args.screens {
            let lo = loadout_for(args.build, run_seed, index);
            let screen = generate_screen(run_seed, index, args.width, &lo);
            if screen.supply {
                supply_screens += 1;
                continue;
            }
            let report = if args.fast {
                solver::validate(&screen, &lo)
            } else {
                solver::validate_full(&screen, &lo)
            };
            // The gate is the all-aims count; the histogram is the direct-aim
            // count, which is bounded by the 40-sample sweep and so stays a
            // readable difficulty signal.
            let worst = report.worst;
            let direct = report.worst_direct;

            let band = band_of(index) as usize;
            if by_band.len() <= band {
                by_band.resize(band + 1, Bucket::default());
            }
            by_band[band].record(&screen, worst, direct);
            by_template[screen.template as usize].record(&screen, worst, direct);

            if worst < MIN_SOLUTIONS {
                failures.push((run_seed, index, screen.template.name(), worst));
                if args.verbose {
                    eprintln!(
                        "FAIL seed {run_seed:#x} screen {index} band {band} {} -> {worst} candidates",
                        screen.template.name()
                    );
                }
            }
        }
    }

    let elapsed = started.elapsed();

    // ------------------------------------------------------ band histogram
    println!("surviving candidates for the hardest target on each screen, by band");
    println!("buckets: {}", BUCKET_LABELS.join(" "));
    println!(
        "{:>5} {:>7} {:>6} {:>6} {:>5}  {}",
        "band", "screens", "mean", "min", "fbk", "histogram"
    );
    let max_cell = by_band
        .iter()
        .flat_map(|b| b.counts.iter().cloned())
        .max()
        .unwrap_or(1);
    for (band, b) in by_band.iter().enumerate() {
        if b.screens == 0 {
            continue;
        }
        let min_seen = if b.min_direct == usize::MAX {
            0
        } else {
            b.min_direct
        };
        let mut cells = String::new();
        for c in b.counts.iter() {
            let ch = if *c == 0 {
                '.'
            } else {
                let scaled = (*c as f64 / max_cell as f64 * 9.0).ceil() as u32;
                char::from_digit(scaled.min(9), 10).unwrap_or('9')
            };
            cells.push(ch);
        }
        println!(
            "{band:>5} {:>7} {:>6.2} {:>6} {:>5}  {cells}",
            b.screens,
            b.mean_candidates(),
            min_seen,
            b.fallbacks
        );
    }

    // ---------------------------------------------------- template summary
    println!("\nby template");
    println!(
        "{:>9} {:>7} {:>6} {:>6} {:>7} {:>6}  {}",
        "template", "screens", "mean", "share", "attempts", "fbk", ""
    );
    let max_screens = by_template.iter().map(|b| b.screens).max().unwrap_or(1);
    for (i, b) in by_template.iter().enumerate() {
        if b.screens == 0 {
            continue;
        }
        let share = b.screens as f64 / total.max(1) as f64;
        println!(
            "{:>9} {:>7} {:>6.2} {:>5.1}% {:>7.2} {:>6}  {}",
            Template::ALL[i].name(),
            b.screens,
            b.mean_candidates(),
            share * 100.0,
            b.attempts as f64 / b.screens as f64,
            b.fallbacks,
            bar(b.screens, max_screens, 24)
        );
    }

    // -------------------------------------------------------------- totals
    let screens_checked: u64 = by_band.iter().map(|b| b.screens).sum();
    let fallbacks: u64 = by_band.iter().map(|b| b.fallbacks).sum();
    let targets: u64 = by_band.iter().map(|b| b.targets).sum();
    println!(
        "\n{screens_checked} screens validated in {:.1}s ({:.1}/s), {targets} targets, \
         {supply_screens} supply screens skipped",
        elapsed.as_secs_f64(),
        screens_checked as f64 / elapsed.as_secs_f64().max(1e-9)
    );
    println!(
        "fallbacks to Flat: {fallbacks} ({:.2}%)",
        fallbacks as f64 / screens_checked.max(1) as f64 * 100.0
    );

    if failures.is_empty() {
        println!("unsolvable screens: 0 — every screen passed the gate");
    } else {
        println!("unsolvable screens: {}", failures.len());
        for (seed, index, template, worst) in failures.iter().take(20) {
            println!("  seed {seed:#018x} screen {index:<4} {template:<9} {worst} candidates");
        }
        std::process::exit(1);
    }
}

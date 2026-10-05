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
//! cargo run --release -p salt-line-sim --bin batch -- --seeds 100 --screens 40 --skill steady
//! ```
//!
//! Exit status is non-zero if any screen failed to validate, so CI can gate on it.
//!
//! Flags: `--seeds N`, `--screens N` (per seed), `--width W`, `--build
//! base|evolving|drift|demo|precision|risk` (default evolving), `--skill
//! perfect|steady|rough` (default perfect), `--fast` (cap
//! candidate counts at [`solver::GATE_CAP`], so the histogram saturates) and
//! `-v` (print each failure as it happens). Skills are fixed hit assumptions:
//! perfect (100%), steady (80%) or rough (55%). A miss wastes one shot; the
//! simulation does not try to model aim, ballistics or player learning.
//!
//! The report includes surviving direct-aim candidates by band and by
//! template, per-band reserve/target curves, solver-assisted economy metrics,
//! and totals. Screen generation climbs down a four-rung ladder
//! (eight drawn seeds, eight Flat seeds, a terrain-free single target, then a
//! level calm-air single target) and every rung is gated on the solver, so a
//! failure here means even the level rung failed. The totals count how many
//! screens came off each rung, and every screen with targets, fallbacks
//! included, is re-validated here rather than trusted. Seeds are the first N
//! values of a fixed schedule; changing skill/build preserves the seed set.
//! The economy model treats a screen as indivisible and stops a run when it
//! cannot afford all modeled attempts for the next screen.

use salt_line_sim::modifiers::{draft_offers, Loadout, Modifier, MODIFIER_COUNT};
use salt_line_sim::rng::{screen_seed, Pcg32};
use salt_line_sim::run::{generate_screen, refund, Rung, Screen};
use salt_line_sim::solver;
use salt_line_sim::terrain::Template;
use salt_line_sim::{band_of, BAND_LEN, MIN_SOLUTIONS, SCREEN_W_REF, START_SHELLS};

/// Which loadout the screens are validated against. Solvability is a property
/// of the screen *and* the loadout, so the harness has to pick one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Build {
    /// Nothing drafted. The strictest case for the generator.
    Base,
    /// Takes a seeded offer every band, the way a run actually goes.
    Evolving,
    /// Ballast, Anemometer, Vernier (DESIGN §4 build shapes).
    Drift,
    /// Auger, Cluster, Thermal Lance.
    Demo,
    /// Sabot, Rangefinder, Chalk Marks.
    Precision,
    /// Featherweight, Dead Reckoning, Forced March.
    Risk,
}

/// Deterministic accuracy assumptions for the solver-assisted economy model.
/// Each target takes geometric(p) shots to fell; misses consume shells but do
/// not alter the generated screen. This is an economy sensitivity model, not
/// an aim/ballistics simulation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Skill {
    Perfect,
    Steady,
    Rough,
}

impl Skill {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "perfect" => Self::Perfect,
            "steady" => Self::Steady,
            "rough" => Self::Rough,
            _ => return None,
        })
    }

    fn hit_percent(self) -> u32 {
        match self {
            Self::Perfect => 100,
            Self::Steady => 80,
            Self::Rough => 55,
        }
    }
}

impl Build {
    /// The `--build` spelling.
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
    skill: Skill,
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
            skill: Skill::Perfect,
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
            "--skill" => {
                let v = value();
                a.skill = Skill::parse(&v).unwrap_or_else(|| {
                    eprintln!("unknown skill '{v}'; use perfect|steady|rough");
                    std::process::exit(2)
                });
            }
            "--fast" => a.fast = true,
            "--verbose" | "-v" => a.verbose = true,
            "--help" | "-h" => {
                println!(
                    "batch [--seeds N] [--screens N] [--width W] [--build BUILD] [--skill perfect|steady|rough] [--fast] [-v]"
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

#[derive(Default)]
struct BandEconomy {
    clears: u64,
    supplies: u64,
    exhausted: u64,
    targets: [u64; 3],
    reserve_before: i64,
    reserve_after: i64,
    min_after: i32,
    max_after: i32,
}

impl BandEconomy {
    fn record(&mut self, targets: usize, before: i32, after: i32, supply: bool) {
        self.clears += 1;
        if supply {
            self.supplies += 1;
        } else if (1..=3).contains(&targets) {
            self.targets[targets - 1] += 1;
        }
        self.reserve_before += i64::from(before);
        self.reserve_after += i64::from(after);
        if self.clears == 1 {
            self.min_after = after;
            self.max_after = after;
        } else {
            self.min_after = self.min_after.min(after);
            self.max_after = self.max_after.max(after);
        }
    }
}

/// One row of the report: a band or a template.
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
///
/// Rebuilt from nothing for every screen rather than carried forward, so any
/// screen can be checked on its own. Fixed builds gain one pick per
/// completed band, in the listed order.
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

/// A `#` bar of `width` characters, filled in proportion to `n / max`.
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

/// Number of attempts needed to land one target under the selected seeded
/// accuracy assumption. A cap makes even the rough profile's tail finite.
fn attempts_per_target(skill: Skill, rng: &mut Pcg32) -> u32 {
    let hit_percent = skill.hit_percent();
    let mut attempts = 1;
    while attempts < 64 && rng.range_u32(0, 99) >= hit_percent {
        attempts += 1;
    }
    attempts
}

fn main() {
    let args = parse_args();
    let total = args.seeds * args.screens as u64;
    println!(
        "salt-line batch: {} seeds x {} screens = {total} screens, width {}, build {:?}, skill {:?} ({}% hit chance){}",
        args.seeds,
        args.screens,
        args.width,
        args.build,
        args.skill,
        args.skill.hit_percent(),
        if args.fast { ", fast" } else { "" }
    );
    println!(
        "seed schedule: mix64(0x53414C54 ^ n * 0x9E3779B97F4A7C15), n=0..{}",
        args.seeds
    );
    println!("gate: worst-target candidates >= {MIN_SOLUTIONS}\n");

    let mut by_band: Vec<Bucket> = Vec::new();
    let mut economy_by_band: Vec<BandEconomy> = Vec::new();
    let mut by_template: Vec<Bucket> = vec![Bucket::default(); Template::ALL.len()];
    let mut failures: Vec<(u64, u32, &'static str, usize)> = Vec::new();
    let mut supply_screens = 0u64;
    let mut rung_counts = [0u64; Rung::ALL.len()];
    let mut modifier_screens = [0u64; MODIFIER_COUNT];
    let mut modifier_picks = [0u64; MODIFIER_COUNT];
    let mut economy_spent = 0u64;
    let mut economy_refunds = 0u64;
    let mut economy_bonus = 0u64;
    let mut combat_clears = 0u64;
    let mut targets_cleared = 0u64;
    let mut one_shot_clears = 0u64;
    let mut clear_tiers = [0u64; 4];
    let mut indirect_shots = 0u64;
    let mut skill_misses = 0u64;
    let mut deaths = 0u64;
    let mut death_screen_sum = 0u64;
    let mut earliest_death = u32::MAX;
    let mut latest_death = 0u32;
    let mut run_length_sum = 0u64;
    let mut ending_shell_sum = 0i64;
    let mut min_ending_shells = i32::MAX;
    let started = std::time::Instant::now();

    for s in 0..args.seeds {
        // Well-spread run seeds, derived from the seed number alone.
        let run_seed =
            salt_line_sim::rng::mix64(0x5341_4C54 ^ s.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut reserve = START_SHELLS;
        let mut accuracy_rng = Pcg32::new(run_seed ^ 0xAC_C0_A1_71);
        let mut previous = Loadout::new();
        let mut alive = true;
        let mut run_length = 0u32;
        let mut one_shot_streak = 0u32;
        for index in 0..args.screens {
            let lo = loadout_for(args.build, run_seed, index);
            // Diff against the previous screen's loadout to find new picks
            // and grant their instant shells, as taking them in a run would.
            for m in Modifier::all() {
                if lo.has(m) {
                    modifier_screens[m.index()] += 1;
                }
                let added = lo.stacks(m).saturating_sub(previous.stacks(m));
                if added > 0 {
                    modifier_picks[m.index()] += added as u64;
                    if alive {
                        let grant = m.entry().effect.instant_shells * added as i32;
                        reserve = (reserve + grant).min(lo.cap());
                        economy_bonus += grant.max(0) as u64;
                    }
                }
            }
            if alive {
                reserve = reserve.min(lo.cap());
            }
            previous = lo.clone();
            let screen = generate_screen(run_seed, index, args.width, &lo);
            let band = band_of(index) as usize;
            if economy_by_band.len() <= band {
                economy_by_band.resize_with(band + 1, BandEconomy::default);
            }
            if screen.supply {
                supply_screens += 1;
                if alive {
                    let before = reserve;
                    reserve = (reserve + 6).min(lo.cap());
                    economy_bonus += 6;
                    run_length += 1;
                    economy_by_band[band].record(0, before, reserve, true);
                }
                continue;
            }
            let Some(rung) = screen.rung else {
                eprintln!(
                    "seed {run_seed:#x} screen {index}: a screen with targets reports no rung"
                );
                std::process::exit(1);
            };
            rung_counts[rung as usize] += 1;
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

            // Economy model: sample a seeded number of attempts per target,
            // then pay the screen's refund and modifiers. Misses consume
            // shells but do not change the screen or the next target's aim.
            // A run exits when it cannot cover the next modeled screen.
            if alive {
                let target_count = screen.structures.len();
                let shots: u32 = (0..target_count)
                    .map(|_| attempts_per_target(args.skill, &mut accuracy_rng))
                    .sum();
                let before = reserve;
                if reserve < shots as i32 {
                    alive = false;
                    deaths += 1;
                    economy_by_band[band].exhausted += 1;
                    death_screen_sum += index as u64;
                    earliest_death = earliest_death.min(index);
                    latest_death = latest_death.max(index);
                } else {
                    reserve -= shots as i32;
                    economy_spent += shots as u64;
                    targets_cleared += target_count as u64;
                    skill_misses += (shots - target_count as u32) as u64;
                    indirect_shots += report
                        .per_target_direct
                        .iter()
                        .filter(|&&count| count == 0)
                        .count() as u64;
                    let paid = refund(shots, &lo);
                    let mut bonus = lo.shells_per_clear;
                    if shots == 1 {
                        one_shot_streak += 1;
                        if lo.standing_orders() {
                            bonus += one_shot_streak as i32;
                        }
                    } else {
                        one_shot_streak = 0;
                    }
                    reserve = (reserve + paid + bonus).min(lo.cap());
                    economy_by_band[band].record(target_count, before, reserve, false);
                    economy_refunds += paid.max(0) as u64;
                    economy_bonus += bonus.max(0) as u64;
                    combat_clears += 1;
                    run_length += 1;
                    clear_tiers[(shots.clamp(1, 4) - 1) as usize] += 1;
                    if shots == 1 {
                        one_shot_clears += 1;
                    }
                }
            }

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
        run_length_sum += run_length as u64;
        ending_shell_sum += reserve as i64;
        min_ending_shells = min_ending_shells.min(reserve);
    }

    let elapsed = started.elapsed();

    // ------------------------------------------------------ band histogram
    println!("surviving candidates for the hardest target on each screen, by band");
    println!("buckets: {}", BUCKET_LABELS.join(" "));
    println!(
        "{:>5} {:>7} {:>6} {:>6} {:>5}  histogram",
        "band", "screens", "mean", "min", "fbk"
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
        "{:>9} {:>7} {:>6} {:>6} {:>7} {:>6}",
        "template", "screens", "mean", "share", "attempts", "fbk"
    );
    let max_screens = by_template.iter().map(|b| b.screens).max().unwrap_or(1);
    for (i, b) in by_template.iter().enumerate() {
        if b.screens == 0 {
            continue;
        }

        let share = b.screens as f64 / total.max(1) as f64;
        println!(
            "{:>9} {:>7} {:>6.2} {:>5.1}% {:>7.2} {:>6}  {bar}",
            Template::ALL[i].name(),
            b.screens,
            b.mean_candidates(),
            share * 100.0,
            b.attempts as f64 / b.screens as f64,
            b.fallbacks,
            bar = bar(b.screens, max_screens, 24)
        );
    }

    // ----------------------------------------- economy curves by band
    // Each row pools completed screens from the surviving seeded runs.
    println!("\nreserve trajectory by band (completed screens; per-screen averages)");
    println!(
        "{:>5} {:>7} {:>7} {:>7} {:>18} {:>8} {:>16} {:>12}",
        "band",
        "clears",
        "supply",
        "dry",
        "target mix 1/2/3",
        "mean Δ",
        "reserve before→after",
        "end min..max"
    );
    for (band, stats) in economy_by_band.iter().enumerate() {
        if stats.clears == 0 && stats.exhausted == 0 {
            continue;
        }
        let combat_clears: u64 = stats.targets.iter().sum();
        let mix = if combat_clears == 0 {
            String::from("—")
        } else {
            format!(
                "{:.0}/{:.0}/{:.0}%",
                stats.targets[0] as f64 / combat_clears as f64 * 100.0,
                stats.targets[1] as f64 / combat_clears as f64 * 100.0,
                stats.targets[2] as f64 / combat_clears as f64 * 100.0
            )
        };
        let clears = stats.clears.max(1) as f64;
        let mean_before = stats.reserve_before as f64 / clears;
        let mean_after = stats.reserve_after as f64 / clears;
        let mean_delta = mean_after - mean_before;
        println!(
            "{band:>5} {:>7} {:>7} {:>7} {:>18} {mean_delta:>+8.2} {mean_before:>7.1}→{mean_after:<7.1} {:>5}..{:<5}",
            stats.clears,
            stats.supplies,
            stats.exhausted,
            mix,
            stats.min_after,
            stats.max_after
        );
    }

    // -------------------------------------------- deterministic play model
    // One modeled successful hit per target plus seeded geometric misses at
    // the selected accuracy rate. Direct/indirect classification still comes
    // from the solver; the misses only change shell spending and refund tier.
    println!("\nsolver-assisted run metrics");
    println!(
        "shell economy: spent {economy_spent}, clear refunds {economy_refunds}, \
         modifier/supply bonus {economy_bonus}, mean ending reserve {:.2}, min {}",
        ending_shell_sum as f64 / args.seeds.max(1) as f64,
        if min_ending_shells == i32::MAX {
            0
        } else {
            min_ending_shells
        }
    );
    println!(
        "clear efficiency: {combat_clears} combat clears, {:.2} shots/clear, {:.2} targets/shell, \
         {:.1}% single-shot screens; tiers 1/2/3/4+ = {}/{}/{}/{}",
        economy_spent as f64 / combat_clears.max(1) as f64,
        targets_cleared as f64 / economy_spent.max(1) as f64,
        one_shot_clears as f64 / combat_clears.max(1) as f64 * 100.0,
        clear_tiers[0],
        clear_tiers[1],
        clear_tiers[2],
        clear_tiers[3]
    );
    println!(
        "targeting: {indirect_shots}/{} targets ({:.2}%) lack a direct candidate; modeled extra misses: {skill_misses}",
        targets_cleared,
        indirect_shots as f64 / targets_cleared.max(1) as f64 * 100.0
    );
    println!(
        "death/run length: {deaths}/{} runs exhausted; mean run {:.1}/{} screens{}",
        args.seeds,
        run_length_sum as f64 / args.seeds.max(1) as f64,
        args.screens,
        if deaths == 0 {
            String::from(", no death screen")
        } else {
            format!(
                ", death screen mean {:.1}, range {}..{}",
                death_screen_sum as f64 / deaths as f64,
                earliest_death,
                latest_death
            )
        }
    );
    println!("modifier frequency (picks, share of generated screens held)");
    for m in Modifier::all() {
        let held = modifier_screens[m.index()];
        let picks = modifier_picks[m.index()];
        if held > 0 || picks > 0 {
            println!(
                "  {:<18} {:>5} {:>7.2}%",
                m.name(),
                picks,
                held as f64 / total.max(1) as f64 * 100.0
            );
        }
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
    println!("screens by generation rung (every rung validated):");
    for r in Rung::ALL {
        let n = rung_counts[r as usize];
        println!(
            "  {:<11} {n:>6} ({:.2}%)",
            format!("{r:?}"),
            n as f64 / screens_checked.max(1) as f64 * 100.0
        );
    }

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

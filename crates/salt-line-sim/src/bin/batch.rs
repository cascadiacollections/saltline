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
//! `--max-exhausted N` and `--ending MIN..MAX` add an economy gate: the run
//! also fails if more than N seeded runs ran out of shells, or the mean ending
//! reserve falls outside MIN..MAX. The run is deterministic, so a gate never
//! flakes; the bounds are what let small, intended tuning through, while a
//! larger shift (in generation, refunds, the curve or this file's play model)
//! trips it. The golden runs already catch *that* the sim changed; this
//! catches *how far* the economy moved.
//!
//! Flags: `--seeds N`, `--screens N` (per seed), `--width W`, `--build
//! base|evolving|drift|demo|precision|risk` (default evolving), `--skill
//! perfect|steady|rough` (default perfect), `--hit N` (a custom 10-100% hit
//! chance in place of a named skill), `--aim flat|scaled` (default flat; see
//! below), `--fast` (cap
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
//! values of a fixed schedule; changing skill, hit chance or build preserves the seed set.
//! The economy model samples every shot a screen will take up front, then
//! plays them in order: each shot costs a shell, a Salvage near miss refunds
//! some back, and a run that is out of shells with a target still standing
//! has run dry mid-screen, so the reserve is recorded as 0. Seeds and the miss stream
//! depend only on the seed index, so every hit chance is compared on the same
//! luck. Across builds the miss draws can drift apart once target counts
//! differ. By default the hit rate is flat: it ignores wind, distance and
//! indirect-only targets. Read the profiles against each other, not as
//! calibrated player types.
//!
//! Two modifiers act on misses and are modelled with stated assumptions
//! rather than ballistics: Salvage refunds its shells for a miss with
//! probability [`SALVAGE_NEAR_SHARE`] (the share of misses assumed to land
//! within its 30 units of a standing target), and Chalk Marks raises the odds
//! of a hit on each retry at the same target by [`CHALK_RETRY_ODDS`]. Salvage
//! draws from its own stream; Chalk changes the accuracy draws themselves, so
//! a build holding it diverges from the first retry on. A build holding
//! neither plays out exactly as before.
//!
//! `--aim scaled` replaces the flat rate with one per target: the selected
//! chance applies at a reference difficulty, and each target's difficulty
//! comes from its range, the screen's wind and how many direct-aim shots fell
//! it (see `shot_difficulty`). Bands then miss more as the curve hardens, so
//! runs past ~40 screens average below the selected chance; that decline,
//! not where misses fall within a band, is what moves survival. The
//! band table's `hit%` is the hit rate on completed screens (the screen a run
//! runs dry on is left out, so late bands read slightly high) and `diff` the
//! mean target difficulty.

use salt_line_sim::modifiers::{draft_offers, Loadout, Modifier, MODIFIER_COUNT};
use salt_line_sim::rng::{screen_seed, Pcg32};
use salt_line_sim::run::{generate_screen, refund, Rung, Screen};
use salt_line_sim::solver;
use salt_line_sim::terrain::Template;
use salt_line_sim::{
    band_of, BAND_LEN, EMPLACEMENT_X, FAR_EDGE_X, MIN_SOLUTIONS, SCREEN_W_REF, SOLVER_TIME_SAMPLES,
    START_SHELLS,
};

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

    fn name(self) -> &'static str {
        match self {
            Self::Perfect => "perfect",
            Self::Steady => "steady",
            Self::Rough => "rough",
        }
    }

    fn hit_percent(self) -> u32 {
        match self {
            Self::Perfect => 100,
            Self::Steady => 80,
            Self::Rough => 55,
        }
    }
}

/// How the hit chance varies from shot to shot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Aim {
    /// Every shot hits at the selected chance.
    Flat,
    /// The selected chance applies to a shot of reference difficulty
    /// ([`REFERENCE_DIFFICULTY`]); farther, windier and tighter shots hit less
    /// often, easier ones more. See [`shot_difficulty`].
    Scaled,
}

impl Aim {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "flat" => Self::Flat,
            "scaled" => Self::Scaled,
            _ => return None,
        })
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
    /// The named skill, or "custom" for `--hit`.
    accuracy: &'static str,
    /// Chance, in whole percent, that one shot fells its target.
    hit_percent: u32,
    aim: Aim,
    /// Cap the candidate count instead of counting them all. Much faster, but
    /// the histogram saturates.
    fast: bool,
    verbose: bool,
    /// Economy gate: most runs allowed to run out of shells.
    max_exhausted: Option<u64>,
    /// Economy gate: allowed range of the mean ending reserve.
    ending: Option<(f64, f64)>,
}

impl Default for Args {
    fn default() -> Self {
        // 50 seeds x 100 screens = 5,000 screens, per the brief.
        Args {
            seeds: 50,
            screens: 100,
            width: SCREEN_W_REF,
            build: Build::Evolving,
            accuracy: Skill::Perfect.name(),
            hit_percent: Skill::Perfect.hit_percent(),
            aim: Aim::Flat,
            fast: false,
            verbose: false,
            max_exhausted: None,
            ending: None,
        }
    }
}

/// `MIN..MAX` as two finite numbers with MIN <= MAX.
fn parse_range(s: &str) -> Option<(f64, f64)> {
    let (lo, hi) = s.split_once("..")?;
    let (lo, hi): (f64, f64) = (lo.trim().parse().ok()?, hi.trim().parse().ok()?);
    (lo.is_finite() && hi.is_finite() && lo <= hi).then_some((lo, hi))
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
                let skill = Skill::parse(&v).unwrap_or_else(|| {
                    eprintln!("unknown skill '{v}'; use perfect|steady|rough");
                    std::process::exit(2)
                });
                a.accuracy = skill.name();
                a.hit_percent = skill.hit_percent();
            }
            "--hit" => {
                let v = value();
                a.accuracy = "custom";
                a.hit_percent = v
                    .parse()
                    .ok()
                    .filter(|p| (10..=100).contains(p))
                    .unwrap_or_else(|| {
                        eprintln!("bad hit chance '{v}'; use a whole percent from 10 to 100");
                        std::process::exit(2)
                    });
            }
            "--aim" => {
                let v = value();
                a.aim = Aim::parse(&v).unwrap_or_else(|| {
                    eprintln!("unknown aim '{v}'; use flat|scaled");
                    std::process::exit(2)
                });
            }
            "--fast" => a.fast = true,
            "--verbose" | "-v" => a.verbose = true,
            "--max-exhausted" => {
                let v = value();
                a.max_exhausted = Some(v.parse().unwrap_or_else(|_| {
                    eprintln!("bad --max-exhausted '{v}'; use a run count");
                    std::process::exit(2)
                }));
            }
            "--ending" => {
                let v = value();
                a.ending = Some(parse_range(&v).unwrap_or_else(|| {
                    eprintln!("bad --ending '{v}'; use MIN..MAX, e.g. 58..71");
                    std::process::exit(2)
                }));
            }
            "--help" | "-h" => {
                println!(
                    "batch [--seeds N] [--screens N] [--width W] [--build BUILD] [--skill perfect|steady|rough | --hit N] [--aim flat|scaled] [--fast] [-v] [--max-exhausted N] [--ending MIN..MAX]"
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
    /// Targets felled and shells fired on completed combat screens: their
    /// ratio is the hit rate the band actually saw.
    hits: u64,
    shots: u64,
    /// Sum of [`shot_difficulty`] over those targets; only under `--aim scaled`.
    difficulty: f64,
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

/// The strongest wind the curve ever draws: `wind_magnitude` at its ceiling.
const WIND_CAP: f64 = 7.0;

/// Difficulty of a shot at which `--aim scaled` hits at exactly the selected
/// chance: the mean over the default `just balance` run (base build, 100
/// seeds x 40 screens, 0.324). Over that run a scaled profile's hit rate lands
/// near its flat one. Difficulty keeps climbing past screen 40 (about 0.56 by
/// band 24), so longer runs average below the selected chance: steady over
/// 100 evolving screens realises ~75%, and survives like a flat 75-76%. In
/// practice the scaled model acts as a hit rate that declines with the band;
/// where the misses fall within a band matters little.
const REFERENCE_DIFFICULTY: f64 = 0.32;

/// How strongly difficulty moves the hit chance: the odds of a hit fall by a
/// factor of e^SLOPE from the easiest shot (0) to the hardest (1). At 2.0 a
/// steady player hits about 88% of the easiest possible shots and 51% of the
/// hardest. Range never drops below ~0.2 (targets sit at least 60 units
/// out), so those ends are not reached: band means run ~82% at band 0 to
/// ~70% at band 24.
const DIFFICULTY_SLOPE: f64 = 2.0;

/// A 0..=1 score for how hard one target is to hit, from what the solver and
/// the screen already know:
///
/// - range: the target's distance from the muzzle over the full
///   emplacement-to-far-edge span of a reference-width screen (weight 0.4);
/// - wind: the wind the shell actually flies through (|wind| times the
///   loadout's wind scale, so Ballast calms it and Featherweight sharpens it;
///   the stronger of the two winds under Storm Front) over [`WIND_CAP`]
///   (weight 0.3);
/// - arc share: 1 minus the share of the 40 direct-aim time samples that fell
///   it, so a target few direct arcs reach, or none (indirect only), scores
///   high (weight 0.3). Indirect-only targets are rare (0 of ~6,800 over the
///   default base run, ~0.3% on long evolving runs), so this mostly measures
///   how many arcs work, not whether any do.
///
/// The weights are a judgement call, not fitted to players. The solver already
/// accounts for wind, so the wind and arc-share terms partly overlap.
fn shot_difficulty(screen: &Screen, lo: &Loadout, target: usize, direct: usize) -> f64 {
    let span = FAR_EDGE_X - EMPLACEMENT_X;
    let range = ((screen.structures[target].centre_x() - screen.muzzle_x).abs() / span).min(1.0);
    let late = match screen.wind_late {
        Some(late) if lo.storm_front() => late.abs(),
        _ => 0.0,
    };
    let wind = (screen.wind.abs().max(late) * lo.wind_scale / WIND_CAP).min(1.0);
    let arcs = 1.0 - (direct.min(SOLVER_TIME_SAMPLES) as f64 / SOLVER_TIME_SAMPLES as f64);
    0.4 * range + 0.3 * wind + 0.3 * arcs
}

/// The selected hit chance adjusted for one shot's difficulty, in whole
/// percent. Works on the odds so it never leaves 0..100: the odds are scaled
/// by e^(-SLOPE x (difficulty - reference)). A perfect profile stays perfect,
/// and anything else is held to 10..=99 so the attempt cap stays negligible
/// and no imperfect player becomes infallible on easy shots.
fn scaled_hit_percent(hit_percent: u32, difficulty: f64) -> u32 {
    if hit_percent >= 100 {
        return 100;
    }
    let p = f64::from(hit_percent) / 100.0;
    let odds = p / (1.0 - p) * (-DIFFICULTY_SLOPE * (difficulty - REFERENCE_DIFFICULTY)).exp();
    ((odds / (1.0 + odds) * 100.0).round() as u32).clamp(10, 99)
}

/// Share of misses assumed to land within Salvage's radius (30 units of a
/// standing target's centre). A judgement call: a player aiming at a target
/// mostly misses short or long by less than that, but a badly judged power or
/// a gust can carry a shell well past. Not measured, and it matters for weak
/// aim: on evolving, rough runs that run dry over 100 screens go 84 -> 60 of
/// 100 as the share goes 0 -> 1. Grounding it in the solver (nudging each
/// target's direct shot a step or two and seeing where the misses land) is a
/// possible follow-up.
const SALVAGE_NEAR_SHARE: f64 = 0.75;

/// Chalk Marks keeps the last landing point marked, so a retry at the same
/// target starts from a visible reference. Modelled as multiplying the odds
/// of a hit on every retry (not the first shot) by this factor; at steady's
/// 80% that lifts retries to ~86%. A judgement call, not measured. Two
/// simplifications, both conservative: a second stack (Chalk Marks can be
/// taken twice, keeping more marks) adds nothing here, and the mark left by
/// the previous target's last shot, which in the game stays up for the next
/// target, is not credited.
const CHALK_RETRY_ODDS: f64 = 1.5;

/// The hit chance on a retry with Chalk Marks held, in whole percent.
fn chalk_retry_percent(hit_percent: u32, chalk_marks: u8) -> u32 {
    if chalk_marks == 0 || hit_percent >= 100 {
        return hit_percent;
    }
    let p = f64::from(hit_percent) / 100.0;
    let odds = p / (1.0 - p) * CHALK_RETRY_ODDS;
    ((odds / (1.0 + odds) * 100.0).round() as u32).clamp(hit_percent, 99)
}

/// Plays one screen's sampled shots in order against the reserve: `attempts`
/// holds each target's shot count, the last of which hits. Each shot costs a
/// shell; a miss refunds `salvage` shells (capped at `cap`) when `near`
/// says it landed close enough. Returns the reserve left and the shells
/// salvaged, or `None` if the reserve hits 0 with a shot still to fire.
fn play_screen(
    mut reserve: i32,
    attempts: &[u32],
    cap: i32,
    salvage: i32,
    mut near: impl FnMut() -> bool,
) -> Option<(i32, u32)> {
    let mut salvaged = 0;
    for &n in attempts {
        for shot in 1..=n {
            if reserve <= 0 {
                return None;
            }
            reserve -= 1;
            if shot < n && salvage > 0 && near() {
                let after = (reserve + salvage).min(cap);
                salvaged += (after - reserve).max(0) as u32;
                reserve = after;
            }
        }
    }
    Some((reserve, salvaged))
}

/// Number of attempts needed to land one target under the selected seeded
/// accuracy assumption. The 64-attempt cap is a safety bound: `--hit` stops
/// at 10%, where it truncates about 0.1% of targets (0.9^63).
/// `retry_percent` applies from the second attempt on (Chalk Marks); pass the
/// same chance twice for none, which draws exactly as a single chance would.
fn attempts_per_target(first_percent: u32, retry_percent: u32, rng: &mut Pcg32) -> u32 {
    let mut attempts = 1;
    let mut hit = first_percent;
    while attempts < 64 && rng.range_u32(0, 99) >= hit {
        attempts += 1;
        hit = retry_percent;
    }
    attempts
}

fn main() {
    let args = parse_args();
    let total = args.seeds * args.screens as u64;
    println!(
        "salt-line batch: {} seeds x {} screens = {total} screens, width {}, build {:?}, accuracy {} ({}% hit chance){}",
        args.seeds,
        args.screens,
        args.width,
        args.build,
        args.accuracy,
        args.hit_percent,
        if args.fast { ", fast" } else { "" }
    );
    println!(
        "seed schedule: mix64(0x53414C54 ^ n * 0x9E3779B97F4A7C15), n < {}",
        args.seeds
    );
    println!("gate: worst-target candidates >= {MIN_SOLUTIONS}");
    println!(
        "{}",
        match args.aim {
            Aim::Flat => format!(
                "economy: fixed {}% hit chance per shot, independent of target difficulty and aim",
                args.hit_percent
            ),
            Aim::Scaled => format!(
                "economy: {}% hit chance at difficulty {REFERENCE_DIFFICULTY}, scaled per target by \
                 range, wind and direct-arc share (slope {DIFFICULTY_SLOPE})",
                args.hit_percent
            ),
        }
    );
    println!(
        "miss modifiers, when held (assumed, not measured): Salvage near-miss share \
         {SALVAGE_NEAR_SHARE}, Chalk Marks retry odds x{CHALK_RETRY_ODDS}\n"
    );

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
    let mut economy_salvage = 0u64;
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
        // Its own stream, so holding Salvage doesn't shift the miss draws.
        let mut salvage_rng = Pcg32::new(run_seed ^ 0x5A_17_A6_E0);
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
                // Scaled aim needs the real direct-aim counts; under --fast
                // they stop at the gate cap, so recount here. The direct sweep
                // runs first and yields at most SOLVER_TIME_SAMPLES, so that
                // cap keeps it exact without counting the other aim points.
                let difficulties: Vec<f64> = match args.aim {
                    Aim::Flat => Vec::new(),
                    Aim::Scaled => {
                        let full;
                        let direct = if args.fast {
                            full = solver::validate_capped(&screen, &lo, SOLVER_TIME_SAMPLES);
                            &full.per_target_direct
                        } else {
                            &report.per_target_direct
                        };
                        (0..target_count)
                            .map(|t| shot_difficulty(&screen, &lo, t, direct[t]))
                            .collect()
                    }
                };
                let attempts: Vec<u32> = (0..target_count)
                    .map(|t| {
                        let hit = match args.aim {
                            Aim::Flat => args.hit_percent,
                            Aim::Scaled => scaled_hit_percent(args.hit_percent, difficulties[t]),
                        };
                        let retry = chalk_retry_percent(hit, lo.chalk_marks);
                        attempts_per_target(hit, retry, &mut accuracy_rng)
                    })
                    .collect();
                let shots: u32 = attempts.iter().sum();
                let before = reserve;
                let played = play_screen(reserve, &attempts, lo.cap(), lo.salvage_shells, || {
                    salvage_rng.next_f64() < SALVAGE_NEAR_SHARE
                });
                if let Some((left, salvaged)) = played {
                    reserve = left;
                    economy_salvage += u64::from(salvaged);
                }
                if played.is_none() {
                    alive = false;
                    reserve = 0;
                    deaths += 1;
                    economy_by_band[band].exhausted += 1;
                    death_screen_sum += index as u64;
                    earliest_death = earliest_death.min(index);
                    latest_death = latest_death.max(index);
                } else {
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
                    let stats = &mut economy_by_band[band];
                    stats.record(target_count, before, reserve, false);
                    stats.hits += target_count as u64;
                    stats.shots += u64::from(shots);
                    stats.difficulty += difficulties.iter().sum::<f64>();
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
    println!(
        "\nreserve trajectory by band (runs still alive only; clears include supply screens; \
         dry = runs that ran out of shells there)"
    );
    println!(
        "{:>5} {:>7} {:>7} {:>7} {:>18} {:>6} {:>6} {:>8} {:>20} {:>12}",
        "band",
        "clears",
        "supply",
        "dry",
        "target mix 1/2/3",
        "hit%",
        "diff",
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
        let hit_rate = if stats.shots == 0 {
            String::from("—")
        } else {
            format!("{:.0}", stats.hits as f64 / stats.shots as f64 * 100.0)
        };
        let difficulty = if args.aim == Aim::Flat || stats.hits == 0 {
            String::from("—")
        } else {
            format!("{:.2}", stats.difficulty / stats.hits as f64)
        };
        // A band where every run ran dry has no completed screens to average.
        let (delta, trajectory, range) = if stats.clears == 0 {
            (String::from("—"), String::from("—"), String::from("—"))
        } else {
            let clears = stats.clears as f64;
            let mean_before = stats.reserve_before as f64 / clears;
            let mean_after = stats.reserve_after as f64 / clears;
            (
                format!("{:+.2}", mean_after - mean_before),
                format!("{mean_before:.1}→{mean_after:.1}"),
                format!("{}..{}", stats.min_after, stats.max_after),
            )
        };
        println!(
            "{band:>5} {:>7} {:>7} {:>7} {mix:>18} {hit_rate:>6} {difficulty:>6} {delta:>8} {trajectory:>20} {range:>12}",
            stats.clears, stats.supplies, stats.exhausted,
        );
    }

    // -------------------------------------------- deterministic play model
    // One modeled successful hit per target plus seeded geometric misses at
    // the selected accuracy rate. Direct/indirect classification still comes
    // from the solver; the misses only change shell spending and refund tier.
    println!("\nsolver-assisted run metrics");
    println!(
        "shell economy: spent {economy_spent}, clear refunds {economy_refunds}, \
         modifier/supply bonus {economy_bonus}, salvage {economy_salvage}, mean ending reserve {:.2}, min {}",
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
        "targeting: {indirect_shots}/{} targets ({:.2}%) lack a direct candidate; modeled extra misses: {skill_misses}{}",
        targets_cleared,
        indirect_shots as f64 / targets_cleared.max(1) as f64 * 100.0,
        match args.aim {
            Aim::Flat => String::new(),
            Aim::Scaled => format!(
                "; mean difficulty {:.3}",
                economy_by_band.iter().map(|b| b.difficulty).sum::<f64>()
                    / targets_cleared.max(1) as f64
            ),
        }
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

    let mut failed = false;
    if failures.is_empty() {
        println!("unsolvable screens: 0 — every screen passed the gate");
    } else {
        println!("unsolvable screens: {}", failures.len());
        for (seed, index, template, worst) in failures.iter().take(20) {
            println!("  seed {seed:#018x} screen {index:<4} {template:<9} {worst} candidates");
        }
        failed = true;
    }

    let mean_ending = ending_shell_sum as f64 / args.seeds.max(1) as f64;
    let violations = economy_violations(&args, deaths, mean_ending);
    if args.max_exhausted.is_some() || args.ending.is_some() {
        // Always print the observed values next to the bounds, so a passing
        // CI log records drift and a failing one says which run broke and
        // what it measured.
        let ending = match args.ending {
            Some((lo, hi)) => format!("mean ending reserve {mean_ending:.2} (bound {lo}..{hi})"),
            None => format!("mean ending reserve {mean_ending:.2}"),
        };
        let exhausted = match args.max_exhausted {
            Some(max) => format!("{deaths}/{} runs exhausted (bound <= {max})", args.seeds),
            None => format!("{deaths}/{} runs exhausted", args.seeds),
        };
        println!(
            "economy gate [build {:?}, accuracy {} {}%, aim {:?}]: {}; {ending}; {exhausted}",
            args.build,
            args.accuracy,
            args.hit_percent,
            args.aim,
            if violations.is_empty() {
                "passed"
            } else {
                "FAILED"
            }
        );
        for v in &violations {
            println!("economy gate: broke — {v}");
        }
        if !violations.is_empty() {
            println!(
                "economy gate: if the change was intended, re-centre the bounds in \
                 .github/workflows/ci.yml around the values above"
            );
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }
}

/// Which economy-gate bounds, if any, this run broke.
fn economy_violations(args: &Args, exhausted: u64, mean_ending: f64) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(max) = args.max_exhausted {
        if exhausted > max {
            out.push(format!("{exhausted} runs exhausted, at most {max} allowed"));
        }
    }
    if let Some((lo, hi)) = args.ending {
        if !(lo..=hi).contains(&mean_ending) {
            out.push(format!(
                "mean ending reserve {mean_ending:.2} outside {lo}..{hi}"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use salt_line_sim::run::wind_magnitude;

    #[test]
    fn a_screen_without_salvage_costs_one_shell_per_shot() {
        // Two targets: one first-time hit, one after two misses.
        assert_eq!(play_screen(10, &[1, 3], 99, 0, || true), Some((6, 0)));
        // Exactly enough shells still clears; one fewer runs dry mid-screen.
        assert_eq!(play_screen(4, &[1, 3], 99, 0, || true), Some((0, 0)));
        assert_eq!(play_screen(3, &[1, 3], 99, 0, || true), None);
    }

    #[test]
    fn salvage_refunds_near_misses_only_and_respects_the_cap() {
        // Two misses, both near: each refunds one, so 3 shots cost 1.
        assert_eq!(play_screen(5, &[3], 99, 1, || true), Some((4, 2)));
        // The same misses landing far refund nothing.
        assert_eq!(play_screen(5, &[3], 99, 1, || false), Some((2, 0)));
        // A refund can keep a run alive that would otherwise run dry.
        assert_eq!(play_screen(1, &[3], 99, 1, || true), Some((0, 2)));
        assert_eq!(play_screen(1, &[3], 99, 1, || false), None);
        // The cap clips a refund, and only the shells that fit count.
        assert_eq!(play_screen(5, &[2], 5, 2, || true), Some((4, 1)));
    }

    #[test]
    fn chalk_lifts_retries_but_not_perfect_or_unmarked_aim() {
        assert_eq!(chalk_retry_percent(80, 0), 80);
        assert_eq!(chalk_retry_percent(100, 1), 100);
        assert_eq!(chalk_retry_percent(80, 1), 86);
        assert_eq!(chalk_retry_percent(55, 1), 65);
        for hit in [10, 55, 80, 95, 99] {
            let retry = chalk_retry_percent(hit, 1);
            assert!((hit..=99).contains(&retry), "{hit} -> {retry}");
        }
    }

    #[test]
    fn equal_retry_chance_draws_like_a_single_chance() {
        let (mut a, mut b) = (Pcg32::new(11), Pcg32::new(11));
        for _ in 0..1000 {
            let single = attempts_per_target(80, 80, &mut a);
            let mut n = 1;
            while n < 64 && b.range_u32(0, 99) >= 80 {
                n += 1;
            }
            assert_eq!(single, n);
        }
    }

    #[test]
    fn ending_ranges_parse_strictly() {
        assert_eq!(parse_range("58..71"), Some((58.0, 71.0)));
        assert_eq!(parse_range("26.5..40"), Some((26.5, 40.0)));
        assert_eq!(parse_range("71..58"), None);
        assert_eq!(parse_range("58"), None);
        assert_eq!(parse_range("a..b"), None);
        assert_eq!(parse_range("NaN..1"), None);
    }

    #[test]
    fn economy_gate_reports_each_broken_bound() {
        let args = Args {
            max_exhausted: Some(2),
            ending: Some((26.0, 40.0)),
            ..Args::default()
        };
        assert!(economy_violations(&args, 2, 26.0).is_empty());
        assert!(economy_violations(&args, 0, 40.0).is_empty());
        assert_eq!(economy_violations(&args, 3, 33.0).len(), 1);
        assert_eq!(economy_violations(&args, 3, 41.0).len(), 2);
        assert!(economy_violations(&Args::default(), 100, 0.0).is_empty());
    }

    #[test]
    fn wind_cap_matches_the_curve() {
        assert_eq!(WIND_CAP, wind_magnitude(u32::MAX));
    }

    #[test]
    fn loadout_wind_scale_moves_the_wind_term() {
        let mut lo = Loadout::new();
        let mut screen = generate_screen(0x5EED, 12, SCREEN_W_REF, &lo);
        screen.wind = WIND_CAP / 2.0;
        let plain = shot_difficulty(&screen, &lo, 0, SOLVER_TIME_SAMPLES);
        lo.wind_scale = 0.5;
        let ballasted = shot_difficulty(&screen, &lo, 0, SOLVER_TIME_SAMPLES);
        assert!((plain - ballasted - 0.3 * 0.25).abs() < 1e-12);
    }

    #[test]
    fn perfect_aim_never_misses() {
        let mut rng = Pcg32::new(7);
        assert!((0..1000).all(|_| attempts_per_target(100, 100, &mut rng) == 1));
    }

    /// Fraction of shots that miss over many targets. Tight enough to catch
    /// an off-by-one in the roll range (80/99 vs 80/100 hits).
    fn miss_fraction(hit_percent: u32) -> f64 {
        let mut rng = Pcg32::new(7);
        let shots: u32 = (0..50_000)
            .map(|_| attempts_per_target(hit_percent, hit_percent, &mut rng))
            .sum();
        f64::from(shots - 50_000) / f64::from(shots)
    }

    #[test]
    fn miss_rate_matches_the_hit_chance() {
        for skill in [Skill::Steady, Skill::Rough] {
            let expected = 1.0 - f64::from(skill.hit_percent()) / 100.0;
            let got = miss_fraction(skill.hit_percent());
            assert!((got - expected).abs() < 0.004, "{skill:?}: {got}");
        }
    }

    #[test]
    fn scaled_aim_leaves_the_reference_shot_and_perfect_aim_alone() {
        for hit in [55, 80, 95] {
            assert_eq!(scaled_hit_percent(hit, REFERENCE_DIFFICULTY), hit);
        }
        for difficulty in [0.0, 0.5, 1.0] {
            assert_eq!(scaled_hit_percent(100, difficulty), 100);
        }
    }

    #[test]
    fn harder_shots_hit_less_often() {
        let mut last = u32::MAX;
        for step in 0..=10 {
            let hit = scaled_hit_percent(80, f64::from(step) / 10.0);
            assert!(hit <= last, "difficulty {step}/10 raised the hit chance");
            assert!((10..=99).contains(&hit));
            last = hit;
        }
        assert_eq!(scaled_hit_percent(80, 0.0), 88);
        assert_eq!(scaled_hit_percent(80, 1.0), 51);
    }

    #[test]
    fn shot_difficulty_tracks_range_wind_and_arc_share() {
        let lo = Loadout::new();
        let mut screen = generate_screen(0x5EED, 12, SCREEN_W_REF, &lo);
        assert!(!screen.structures.is_empty());
        screen.wind = 0.0;
        let calm_wide = shot_difficulty(&screen, &lo, 0, SOLVER_TIME_SAMPLES);
        let calm_tight = shot_difficulty(&screen, &lo, 0, 0);
        assert!((calm_tight - calm_wide - 0.3).abs() < 1e-12);
        screen.wind = -WIND_CAP;
        let gusty_wide = shot_difficulty(&screen, &lo, 0, SOLVER_TIME_SAMPLES);
        assert!((gusty_wide - calm_wide - 0.3).abs() < 1e-12);
        assert!((0.0..=1.0).contains(&calm_wide) && gusty_wide <= 1.0);
    }

    #[test]
    fn skill_names_round_trip() {
        for skill in [Skill::Perfect, Skill::Steady, Skill::Rough] {
            assert_eq!(Skill::parse(skill.name()), Some(skill));
        }
        assert_eq!(Skill::parse("sloppy"), None);
    }
}

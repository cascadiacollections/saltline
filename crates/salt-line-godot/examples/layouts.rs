//! Layout harness: forks many generated screens into layouts (DESIGN §13)
//! and reports what NOTES §9 records: share-code lengths, how close a fork
//! plays to the screen it came from, how many forks pass their checks, and
//! what the checks cost.
//!
//! ```text
//! cargo run --release -p salt-line-godot --example layouts
//! cargo run --release -p salt-line-godot --example layouts -- --seeds 100 --screens 40 --rounds 3
//! cargo run --release -p salt-line-godot --example layouts -- fork-run DUNE:8
//! cargo run --release -p salt-line-godot --example layouts -- fork-duel 42:0
//! cargo run --release -p salt-line-godot --example layouts -- check SL…
//! ```
//!
//! The report forks `--screens` run screens of each of `--seeds` run seeds
//! and `--rounds` duel rounds of as many match seeds, all at the layout
//! width, and for each:
//!
//! - **Code length**: the share code's characters, prefix included, and
//!   what keeping the generated heights bit for bit would cost instead (a
//!   fixed point with 49 fraction bits, coded the same way).
//! - **Ground**: the largest difference between a generated height and its
//!   fork's, and between a structure's seat or a wind and its fork's.
//! - **Play**: shots fired at both screens, which must end the same way:
//!   the same kind of end within a unit, the same structures down or
//!   leaning, the same clear, the same duel hit. The shots are the gate's
//!   own candidates (shots that only just work, where a difference would
//!   show first) and as many random ones.
//! - **Checks**: how many forks pass placement and the solvability gate,
//!   the problems found, and the time each check took.
//!
//! The timings are wall-clock; everything else is deterministic.

use std::collections::BTreeMap;
use std::time::Instant;

use salt_line_godot::custom;
use salt_line_godot::versus::{self, fly, frame_for, hits, shots_at, target_in_frame, Rules, Side};
use salt_line_sim::ballistics::{End, Shot};
use salt_line_sim::layout::{self, wind_units, Layout, WindSpec, LAYOUT_WIDTH};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::rng::{mix64, Pcg32};
use salt_line_sim::run::{generate_screen, Screen};
use salt_line_sim::solver::{self, GATE_CAP};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fork-run") => print!(
            "{}",
            custom::fork_report(args.get(1).map(String::as_str), None).unwrap_or_default()
        ),
        Some("fork-duel") => print!(
            "{}",
            custom::fork_report(None, args.get(1).map(String::as_str)).unwrap_or_default()
        ),
        Some("check") => match custom::read(args.get(1).map_or("", String::as_str)) {
            Ok(l) => println!(
                "passes: {} columns, {} guns, {} structures, {:?}",
                l.width(),
                l.guns.len(),
                l.structures.len(),
                l.wind
            ),
            Err(why) => println!("{why}"),
        },
        _ => report(&args),
    }
}

fn flag(args: &[String], name: &str, default: u32) -> u32 {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Default)]
struct Tally {
    lengths: Vec<usize>,
    lossless: Vec<usize>,
    ground: f64,
    seat: f64,
    wind: f64,
    shots: usize,
    same: usize,
    passed: usize,
    problems: BTreeMap<String, usize>,
    micros: Vec<u128>,
}

impl Tally {
    fn print(&mut self, what: &str) {
        let n = self.lengths.len();
        println!("{what}: {n} forks");
        println!("  code length (chars): {}", spread(&mut self.lengths));
        println!(
            "  bit-exact heights would be: {}",
            spread(&mut self.lossless)
        );
        println!(
            "  largest change: ground {:.4}, structure seat {:.4}, wind {:.4}",
            self.ground, self.seat, self.wind
        );
        println!(
            "  shots ending the same: {}/{} ({:.2}%)",
            self.same,
            self.shots,
            100.0 * self.same as f64 / self.shots.max(1) as f64
        );
        println!("  pass every check: {}/{n}", self.passed);
        for (p, c) in &self.problems {
            println!("    {p}: {c}");
        }
        let mut ms: Vec<usize> = self.micros.iter().map(|&m| m as usize).collect();
        println!("  check time (µs): {}", spread(&mut ms));
    }
}

fn spread(v: &mut [usize]) -> String {
    if v.is_empty() {
        return "none".into();
    }
    v.sort_unstable();
    let n = v.len();
    format!(
        "min {} median {} p99 {} max {}",
        v[0],
        v[n / 2],
        v[(n * 99 / 100).min(n - 1)],
        v[n - 1]
    )
}

fn report(args: &[String]) {
    let seeds = flag(args, "--seeds", 100) as u64;
    let screens = flag(args, "--screens", 40);
    let rounds = flag(args, "--rounds", 3);
    let lo = Loadout::new();
    let mut run = Tally::default();
    for s in 0..seeds {
        let seed = mix64(s ^ 0x1A70);
        for index in 0..screens {
            let gen = generate_screen(seed, index, LAYOUT_WIDTH, &lo);
            let l = Layout::from_run_screen(seed, index);
            measure_ground(&mut run, &gen, &l);
            let wind = match l.wind {
                WindSpec::Fixed(w) => wind_units(w),
                _ => unreachable!("a run fork's wind is fixed"),
            };
            run.wind = run.wind.max((wind - gen.wind).abs());
            let fork = Screen::from_layout(&l, wind);
            for (g, f) in gen.structures.iter().zip(&fork.structures) {
                run.seat = run.seat.max((g.base - f.base).abs());
            }
            let mut shots: Vec<Shot> = solver::validate(&gen, &lo)
                .candidates
                .iter()
                .map(|c| c.shot)
                .collect();
            shots.extend(random_shots(seed ^ index as u64, 16));
            for shot in shots {
                run.shots += 1;
                let (mut a, mut b) = (gen.clone(), fork.clone());
                let (ra, rb) = (
                    a.apply_shot(shot, &lo, None, false),
                    b.apply_shot(shot, &lo, None, false),
                );
                let same = same_end(ra.traces[0].end, rb.traces[0].end)
                    && ra.cleared == rb.cleared
                    && ra.collapsed == rb.collapsed
                    && ra.newly_leaning == rb.newly_leaning;
                run.same += same as usize;
            }
            check(&mut run, &l);
        }
    }
    run.print("run screens");

    let mut duel = Tally::default();
    let reach = Rules::DEFAULT.hit_reach;
    for s in 0..seeds {
        let seed = mix64(s ^ 0xD0E1);
        for round in 0..rounds {
            let gen = versus::generate_duel(seed, round, LAYOUT_WIDTH, &lo, &Rules::DEFAULT);
            let l = custom::fork_duel(seed, round);
            measure_ground(&mut duel, &gen, &l);
            let fork = Screen::from_layout(&l, 0.0);
            for wind in l.wind.extremes() {
                let (mut g, mut f) = (gen.clone(), fork.clone());
                g.wind = wind_units(wind);
                f.wind = g.wind;
                for side in Side::BOTH {
                    let (fg, ff) = (frame_for(&g, side), frame_for(&f, side));
                    let (tg, tf) = (target_in_frame(&g, side), target_in_frame(&f, side));
                    let mut shots = shots_at(&fg, tg, &lo, reach, GATE_CAP);
                    shots.extend(random_shots(seed ^ round as u64 ^ wind as u64, 8));
                    for shot in shots {
                        duel.shots += 1;
                        let (a, b) = (
                            fly(&fg, tg, shot, &lo, false),
                            fly(&ff, tf, shot, &lo, false),
                        );
                        let same = same_end(a[0].end, b[0].end)
                            && hits(&a[0], tg, &lo, reach) == hits(&b[0], tf, &lo, reach);
                        duel.same += same as usize;
                    }
                }
            }
            check(&mut duel, &l);
        }
    }
    duel.print("duel rounds");
}

/// Code lengths and the largest height change.
fn measure_ground(t: &mut Tally, gen: &Screen, l: &Layout) {
    t.lengths.push(l.to_code().map(|c| c.len()).unwrap_or(0));
    t.lossless.push(lossless_chars(&gen.field.base, l));
    let fork = Screen::from_layout(l, 0.0);
    for (a, b) in gen.field.base.iter().zip(&fork.field.base) {
        t.ground = t.ground.max((a - b).abs());
    }
}

fn check(t: &mut Tally, l: &Layout) {
    let started = Instant::now();
    let result = custom::validate(l);
    t.micros.push(started.elapsed().as_micros());
    match result {
        Ok(()) => t.passed += 1,
        Err(problems) => {
            for p in problems {
                let name = format!("{p:?}");
                let name = name.split(['(', ' ']).next().unwrap_or("").to_string();
                *t.problems.entry(name).or_insert(0) += 1;
            }
        }
    }
}

/// The same kind of end, within a unit.
fn same_end(a: End, b: End) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
        && (a.pos().x - b.pos().x).abs() < 1.0
        && (a.pos().y - b.pos().y).abs() < 1.0
}

fn random_shots(seed: u64, n: usize) -> Vec<Shot> {
    let mut r = Pcg32::new(seed ^ 0x5407);
    (0..n)
        .map(|_| {
            salt_line_sim::run::shot(r.range_u32(300, 1600) as i32, r.range_u32(250, 1000) as i32)
        })
        .collect()
}

/// The characters a code would take with the generated heights kept bit
/// for bit: each as a fixed point with 49 fraction bits (every double from
/// 12 to 108 is a whole number of 2^-49), coded as the real code codes
/// sixteenths, plus the real code's other fields.
fn lossless_chars(base: &[f64], l: &Layout) -> usize {
    let fixed: Vec<i128> = base
        .iter()
        .map(|&b| (b * (1u64 << 49) as f64) as i128)
        .collect();
    let mut residuals = Vec::new();
    let mut slope: i128 = 0;
    for w in fixed.windows(2) {
        let d = w[1] - w[0];
        let r = d - slope;
        residuals.push(((r << 1) ^ (r >> 127)) as u128);
        slope = d;
    }
    let bits = (0..64)
        .map(|k| {
            residuals
                .iter()
                .map(|&z| (z >> k) as u64 + 1 + k as u64)
                .sum::<u64>()
        })
        .min()
        .unwrap_or(0);
    let header = layout_header_chars(l);
    header + (bits as usize + 64).div_ceil(5)
}

/// Characters a code spends on everything but the heights: the real code's
/// length less what its heights cost.
fn layout_header_chars(l: &Layout) -> usize {
    let mut flat = l.clone();
    flat.heights
        .iter_mut()
        .for_each(|h| *h = layout::height_steps(52.0));
    flat.to_code().map(|c| c.len()).unwrap_or(0)
}

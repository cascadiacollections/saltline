//! Duel harness: two bots play local versus matches, to weigh the rules.
//!
//! The batch harness (`salt-line-sim`'s `batch`) tunes the run without
//! playing it a thousand times; this does the same for a duel. Each match is
//! played to the end by two bots under one set of [`Rules`], and the report
//! says how quickly they hit, how often a shot that just hit would hit again,
//! how much opening a round is worth, how long rounds and matches run, and
//! what a roll of the guns costs.
//!
//! ```text
//! cargo run --release -p salt-line-godot --example duel
//! cargo run --release -p salt-line-godot --example duel -- --left kid --right steady
//! cargo run --release -p salt-line-godot --example duel -- --no-equal-shots --shift 0
//! cargo run --release -p salt-line-godot --example duel -- --left kid --right ai-easy
//! ```
//!
//! Flags: `--matches N` (default 200), `--width W` (default 320), `--bot
//! kid|steady` for both guns or `--left` / `--right` for one (default kid;
//! either also takes `ai-easy`, `ai-medium` or `ai-hard`, the game's own AI
//! opponent, DESIGN §11.7),
//! and one per lever in [`Rules`]: `--wind MAX`, `--wind-min MIN`, `--reach
//! R`, `--equal-shots` / `--no-equal-shots`, `--shift D` and `--back N` /
//! `--forward N` (the roll ladder, in steps). A lever left out takes the
//! game's value, so with no flags this measures the rules as shipped.
//!
//! # The bots
//!
//! A bot plays the way a person learns a duel from the landing marks, not the
//! way the solver would. It picks an angle for the round (the hitting shot
//! nearest 45° in calm air) and mostly changes only power. Each turn it fires
//!
//! ```text
//! power = ideal(wind it believes) + bias + noise
//! ```
//!
//! - `ideal(w)` is the power, on the 5-tick grid a drag or key gives, that
//!   lands nearest the other gun's centre in wind `w`, found by flying the
//!   shot on the ground and with the guns as the round opened. It stands for
//!   the range the player has worked out; if the guns roll, the bot does not
//!   know until its shells land somewhere else.
//! - The **wind it believes** moves from the wind of its last shot toward this
//!   turn's by `wind_read` (0 ignores a change of wind, 1 reads it exactly).
//!   On its first shot of a round it starts from calm air, which is what aim
//!   help's preview shows.
//! - The **bias** starts as an opening error up to `open` ticks either way and
//!   after every shot is corrected by `learn` of what the landing showed (the
//!   difference between what it fired and what would have hit in the wind
//!   that blew). With `wind_read` at 0 and `learn` at 1 this is exactly
//!   "fire again the shot that would have hit last time".
//! - The **noise** is a hand that is not perfectly steady: up to `noise`
//!   ticks either way.
//! - **Lapses**: after a miss, with chance `overcorrect` it corrects by
//!   twice what it should, and with chance `fidget` it tries another
//!   hitting angle instead, keeping a bias learned at the old one.
//!
//! `kid` stands for a young player with aim help on: a shakier hand, slower
//! learning, little wind reading, and lapses. `steady` is a grown-up without
//! aim help and without lapses. Both are guesses, so read lever settings
//! against each other, not as a forecast of real play.
//!
//! The `ai-*` players are not bots of this kind but the game's AI itself
//! (`salt_line_godot::opponent::ai_shot`), shot for shot as a match against
//! it plays, easing off while it leads on hits included; on the left gun it plays the
//! same way. The report adds each gun's hit rate, to read its odds against.
//!
//! Everything is seeded: the same flags print the same report. The roll
//! timing is wall-clock and is the one line that varies.

use std::time::Instant;

use salt_line_godot::opponent::{self, Level};
use salt_line_godot::versus::{self, Match, Phase, Rules, Side, HITS_TO_WIN_ROUND};
use salt_line_sim::ballistics::{End, Shot};
use salt_line_sim::rng::{substream, Pcg32};
use salt_line_sim::solver::GATE_CAP;
use salt_line_sim::POWER_TICKS_MAX;

/// How a bot aims. See the module docs.
#[derive(Clone, Copy, Debug)]
struct Profile {
    name: &'static str,
    wind_read: f64,
    learn: f64,
    open: f64,
    noise: f64,
    overcorrect: f64,
    fidget: f64,
}

const KID: Profile = Profile {
    name: "kid",
    wind_read: 0.2,
    learn: 0.5,
    open: 120.0,
    noise: 25.0,
    overcorrect: 0.15,
    fidget: 0.10,
};

const STEADY: Profile = Profile {
    name: "steady",
    wind_read: 0.6,
    learn: 0.85,
    open: 120.0,
    noise: 8.0,
    overcorrect: 0.0,
    fidget: 0.0,
};

/// Who plays a gun: a bot, or the game's AI at a level.
#[derive(Clone, Copy, Debug)]
enum Player {
    Bot(Profile),
    Ai(Level),
}

impl Player {
    fn name(self) -> String {
        match self {
            Player::Bot(p) => p.name.to_string(),
            Player::Ai(level) => format!("ai-{}", level.token()),
        }
    }
}

/// Power moves in steps of this many ticks, as a key press or a drag does.
const POWER_STEP: i32 = 5;

/// A turn cap per round, so a pathological screen cannot hang the harness.
const MAX_TURNS: u32 = 400;

/// Seed tag for the bots' own randomness, ASCII "DBOT".
const TAG_BOT: u64 = 0x4442_4F54;

struct Args {
    matches: u64,
    width: usize,
    /// Each gun's player, by [`Side::index`].
    bots: [Player; 2],
    rules: Rules,
}

fn parse_args() -> Args {
    let mut a = Args {
        matches: 200,
        width: 320,
        bots: [Player::Bot(KID); 2],
        rules: Rules::DEFAULT,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let bad = |flag: &str, v: &str| -> ! {
        eprintln!("bad {flag} '{v}'");
        std::process::exit(2)
    };
    let profile = |flag: &str, v: &str| match v {
        "kid" => Player::Bot(KID),
        "steady" => Player::Bot(STEADY),
        _ => match v.strip_prefix("ai-").and_then(Level::from_token) {
            Some(level) => Player::Ai(level),
            None => bad(flag, v),
        },
    };
    while i < argv.len() {
        let flag = argv[i].as_str();
        let mut value = || {
            i += 1;
            argv.get(i).cloned().unwrap_or_default()
        };
        match flag {
            "--matches" => {
                let v = value();
                a.matches = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--width" => {
                let v = value();
                a.width = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--bot" => {
                let p = profile(flag, &value());
                a.bots = [p, p];
            }
            "--left" => a.bots[0] = profile(flag, &value()),
            "--right" => a.bots[1] = profile(flag, &value()),
            "--wind" => {
                let v = value();
                a.rules.wind_max = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--wind-min" => {
                let v = value();
                a.rules.wind_min = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--reach" => {
                let v = value();
                a.rules.hit_reach = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--equal-shots" => a.rules.equal_shots = true,
            "--no-equal-shots" => a.rules.equal_shots = false,
            "--shift" => {
                let v = value();
                a.rules.gun_shift = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--back" => {
                let v = value();
                a.rules.roll_back = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--forward" => {
                let v = value();
                a.rules.roll_forward = v.parse().unwrap_or_else(|_| bad(flag, &v));
            }
            "--help" | "-h" => {
                println!(
                    "duel [--matches N] [--width W] [--bot P] [--left P] [--right P] (P: kid|steady|ai-easy|ai-medium|ai-hard) [--wind MAX] [--wind-min MIN] [--reach R] [--equal-shots | --no-equal-shots] [--shift D] [--back N] [--forward N]"
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

/// Where a shot from `side` ends up, as a horizontal distance from the
/// other gun's centre in `side`'s frame, flown in wind `wind` (also in that
/// frame). Positive is long.
fn miss_by(duel: &Match, side: Side, shot: Shot, wind: f64) -> f64 {
    let mut frame = versus::frame_for(&duel.screen, side);
    frame.wind = wind;
    let target = versus::target_in_frame(&duel.screen, side);
    let traces = versus::fly(&frame, target, shot, &duel.loadout, false);
    let end = traces.first().map(|t| t.end);
    let x = match end {
        Some(End::Structure { .. }) | None => return 0.0,
        Some(e) => e.pos().x,
    };
    x - target.centre().x
}

/// The power on the step grid that lands nearest the other gun at `angle` in
/// `wind` (shooter's frame): a coarse sweep, then the steps around the best.
fn ideal_power(duel: &Match, side: Side, angle: i32, wind: f64) -> i32 {
    let score = |p: i32| miss_by(duel, side, Shot::new(angle, p), wind).abs();
    let coarse = (0..=POWER_TICKS_MAX)
        .step_by(40)
        .min_by(|&a, &b| score(a).total_cmp(&score(b)))
        .unwrap_or(0);
    ((coarse - 40).max(0)..=(coarse + 40).min(POWER_TICKS_MAX))
        .step_by(POWER_STEP as usize)
        .min_by(|&a, &b| score(a).total_cmp(&score(b)))
        .unwrap_or(coarse)
}

/// The angles of the hitting shots in calm air, nearest 45° first.
fn hitting_angles(duel: &Match, side: Side) -> Vec<i32> {
    let mut frame = versus::frame_for(&duel.screen, side);
    frame.wind = 0.0;
    let target = versus::target_in_frame(&duel.screen, side);
    let mut angles: Vec<i32> = versus::shots_at(
        &frame,
        target,
        &duel.loadout,
        duel.rules.hit_reach,
        GATE_CAP,
    )
    .iter()
    .map(|s| s.angle_ticks)
    .collect();
    angles.sort_by_key(|a| (a - 900).abs());
    angles.dedup();
    if angles.is_empty() {
        angles.push(900);
    }
    angles
}

/// One bot's memory for a round.
struct Bot {
    angles: Vec<i32>,
    angle: i32,
    bias: f64,
    /// Wind of its last shot, in its own frame.
    last_wind: Option<f64>,
    /// Its last shot, if that shot hit.
    last_hit: Option<Shot>,
}

/// This turn's wind in `side`'s own frame.
fn own_wind(duel: &Match, side: Side) -> f64 {
    versus::frame_for(&duel.screen, side).wind
}

#[derive(Default)]
struct Tally {
    rounds: u64,
    turns: u64,
    hits: u64,
    shots: u64,
    /// Turns that followed a hit by the same side, and how many of those the
    /// hitting shot, fired again unchanged, would have hit too.
    repeat_chances: u64,
    repeat_hits: u64,
    opener_won: u64,
    first_opener_matches: u64,
    /// Matches won, by [`Side::index`].
    side_matches: [u64; 2],
    /// Shots fired and hits made, by [`Side::index`].
    side_shots: [u64; 2],
    side_hits: [u64; 2],
    /// Rounds that ended with someone past [`HITS_TO_WIN_ROUND`] hits.
    overtime: u64,
    shutouts: u64,
    capped: u64,
    matches: u64,
    /// Turns each finished match took.
    match_turns: Vec<u32>,
    /// Seconds spent in each shot that rolled the guns (the re-gate is most
    /// of it), and in each that did not.
    rolled_fire: Vec<f64>,
    plain_fire: Vec<f64>,
    /// How far apart the two pivots stood in height after each roll.
    roll_heights: Vec<f64>,
}

fn play_match(seed: u64, args: &Args, t: &mut Tally) {
    let mut duel = Match::with_rules(seed, args.width, args.rules);
    let mut rng = Pcg32::new(substream(seed, TAG_BOT));
    let mut match_turns = 0;
    loop {
        let opener = duel.to_move;
        // What the bots learned of the range is the ground and guns as the
        // round opened; anything since (craters, a roll) shows up only in
        // where their shells land, and the bias learns it from there.
        let reference = duel.clone();
        let mut bots = Side::BOTH.map(|side| {
            let angles = hitting_angles(&duel, side);
            let open = match args.bots[side.index()] {
                Player::Bot(p) => p.open,
                Player::Ai(_) => 0.0,
            };
            Bot {
                angle: angles[0],
                angles,
                bias: rng.signed(open),
                last_wind: None,
                last_hit: None,
            }
        });
        let mut turns = 0;
        while duel.phase == Phase::Aiming && turns < MAX_TURNS {
            let side = duel.to_move;
            let p = match args.bots[side.index()] {
                Player::Bot(p) => p,
                Player::Ai(level) => {
                    let shot = opponent::ai_shot(&duel, level);
                    let report = fire(&mut duel, shot, t);
                    t.side_shots[side.index()] += 1;
                    t.side_hits[side.index()] += report.hit as u64;
                    bots[side.index()].last_hit = None;
                    turns += 1;
                    continue;
                }
            };
            let wind = own_wind(&duel, side);
            let bot = &mut bots[side.index()];
            if let Some(shot) = bot.last_hit {
                t.repeat_chances += 1;
                t.repeat_hits += would_hit(&duel, side, shot) as u64;
            }
            let believed = match bot.last_wind {
                Some(last) => last + p.wind_read * (wind - last),
                None => p.wind_read * wind,
            };
            let aim = ideal_power(&reference, side, bot.angle, believed) as f64
                + bot.bias
                + rng.signed(p.noise);
            let power =
                ((aim / POWER_STEP as f64).round() as i32 * POWER_STEP).clamp(0, POWER_TICKS_MAX);
            let shot = Shot::new(bot.angle, power);
            // What would have hit in the wind that blew, read before the
            // shot digs its crater.
            let wanted = ideal_power(&duel, side, bot.angle, wind) as f64;
            let report = fire(&mut duel, shot, t);
            t.side_shots[side.index()] += 1;
            t.side_hits[side.index()] += report.hit as u64;
            let bot = &mut bots[side.index()];
            let gain = if !report.hit && rng.next_f64() < p.overcorrect {
                2.0 * p.learn
            } else {
                p.learn
            };
            bot.bias -= gain * (power as f64 - wanted);
            if !report.hit && bot.angles.len() > 1 && rng.next_f64() < p.fidget {
                let pick = rng.range_u32(0, bot.angles.len() as u32 - 1) as usize;
                bot.angle = bot.angles[pick];
            }
            bot.last_wind = Some(wind);
            bot.last_hit = report.hit.then_some(shot);
            t.shots += 1;
            t.hits += report.hit as u64;
            turns += 1;
        }
        t.rounds += 1;
        t.turns += turns as u64;
        match_turns += turns;
        let winner = match duel.phase {
            Phase::RoundOver(w) | Phase::MatchOver(w) => w,
            Phase::Aiming => {
                t.capped += 1;
                return;
            }
        };
        t.opener_won += (winner == opener) as u64;
        t.shutouts += (duel.hits[winner.other().index()] == 0) as u64;
        t.overtime += (duel.hits[winner.index()] > HITS_TO_WIN_ROUND) as u64;
        if let Phase::MatchOver(w) = duel.phase {
            t.matches += 1;
            t.first_opener_matches += (w == Match::first_to_move(0)) as u64;
            t.side_matches[w.index()] += 1;
            t.match_turns.push(match_turns);
            return;
        }
        duel.next_round();
    }
}

/// Fire `shot`, timing it and noting what a roll of the guns cost.
fn fire(duel: &mut Match, shot: Shot, t: &mut Tally) -> versus::TurnReport {
    let clock = Instant::now();
    let report = duel.fire(shot, false).expect("aiming");
    let spent = clock.elapsed().as_secs_f64();
    if report.rolled.is_some() {
        t.rolled_fire.push(spent);
        let (l, r) = (
            versus::muzzle_of(&duel.screen, Side::Left),
            versus::muzzle_of(&duel.screen, Side::Right),
        );
        t.roll_heights.push((l.y - r.y).abs());
    } else {
        t.plain_fire.push(spent);
    }
    report
}

/// Would `shot` hit the other gun now, as the guns stand, in this turn's
/// wind?
fn would_hit(duel: &Match, side: Side, shot: Shot) -> bool {
    let frame = versus::frame_for(&duel.screen, side);
    let target = versus::target_in_frame(&duel.screen, side);
    versus::fly(&frame, target, shot, &duel.loadout, false)
        .iter()
        .any(|tr| versus::hits(tr, target, &duel.loadout, duel.rules.hit_reach))
}

fn pct(n: u64, d: u64) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn max(xs: &[f64]) -> f64 {
    xs.iter().copied().fold(0.0, f64::max)
}

fn main() {
    let args = parse_args();
    let r = args.rules;
    let mut t = Tally::default();
    for m in 0..args.matches {
        play_match(substream(0x5EED_D0E1, m), &args, &mut t);
    }
    let [left, right] = args.bots.map(Player::name);
    println!(
        "{left} v {right} | wind {:.1}..{:.1} | reach {:.1} | equal shots {} | shift {:.0} (-{}..+{}) | {} matches at width {}",
        r.wind_min,
        r.wind_max,
        r.hit_reach,
        if r.equal_shots { "on" } else { "off" },
        r.gun_shift,
        r.roll_back,
        r.roll_forward,
        args.matches,
        args.width,
    );
    let rounds = t.rounds - t.capped;
    println!(
        "shots per hit {:.2} | turns per round {:.1} | repeat hits {:.0}% of {} | opener wins round {:.1}% | first opener wins match {:.1}% | shutouts {:.0}% | past {} hits {:.0}%{}",
        t.shots as f64 / t.hits.max(1) as f64,
        t.turns as f64 / t.rounds.max(1) as f64,
        pct(t.repeat_hits, t.repeat_chances),
        t.repeat_chances,
        pct(t.opener_won, rounds),
        pct(t.first_opener_matches, t.matches),
        pct(t.shutouts, rounds),
        HITS_TO_WIN_ROUND,
        pct(t.overtime, rounds),
        if t.capped > 0 {
            format!(" | {} rounds hit the turn cap", t.capped)
        } else {
            String::new()
        },
    );
    let mut turns = t.match_turns.clone();
    turns.sort_unstable();
    let p90 = turns
        .get((turns.len() * 9 / 10).min(turns.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0);
    println!(
        "matches won: left ({left}) {:.1}%, right ({right}) {:.1}% | turns per match mean {:.1}, p90 {}",
        pct(t.side_matches[0], t.matches),
        pct(t.side_matches[1], t.matches),
        turns.iter().map(|&n| n as f64).sum::<f64>() / turns.len().max(1) as f64,
        p90,
    );
    println!(
        "hit rate: left ({left}) {:.1}% of {}, right ({right}) {:.1}% of {}",
        pct(t.side_hits[0], t.side_shots[0]),
        t.side_shots[0],
        pct(t.side_hits[1], t.side_shots[1]),
        t.side_shots[1],
    );
    println!(
        "rolls {} | shot with a roll: mean {:.2} ms, max {:.2} ms (without: mean {:.2} ms) | pivot height gap after a roll: mean {:.2}, max {:.2}",
        t.rolled_fire.len(),
        mean(&t.rolled_fire) * 1e3,
        max(&t.rolled_fire) * 1e3,
        mean(&t.plain_fire) * 1e3,
        mean(&t.roll_heights),
        max(&t.roll_heights),
    );
}

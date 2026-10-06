//! Duel harness: two bots play local versus matches, to weigh the rules.
//!
//! The batch harness (`salt-line-sim`'s `batch`) tunes the run without
//! playing it a thousand times; this does the same for a duel. Each match is
//! played to the end by two bots under one set of [`Rules`], and the report
//! says how quickly they hit, how often a shot that just hit would hit again,
//! and how much opening a round is worth.
//!
//! ```text
//! cargo run --release -p salt-line-godot --example duel
//! cargo run --release -p salt-line-godot --example duel -- --wind 5 --wind-min 1.5
//! cargo run --release -p salt-line-godot --example duel -- --bot steady --no-equal-shots --shift 0
//! ```
//!
//! Flags: `--matches N` (default 200), `--width W` (default 320), `--bot
//! kid|steady` (default kid), and one per lever in [`Rules`]: `--wind MAX`,
//! `--wind-min MIN`, `--reach R`, `--equal-shots` / `--no-equal-shots` and
//! `--shift D`. A lever left out takes the game's value, so with no flags
//! this measures the rules as shipped.
//!
//! # The bots
//!
//! A bot plays the way a person learns a duel from the landing marks, not the
//! way the solver would. It picks one angle for the round (the hitting shot
//! nearest 45° in calm air) and only ever changes power. Each turn it fires
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
//!
//! `kid` stands for a young player with aim help on, `steady` for a grown-up
//! without it. Both are guesses, so read lever settings against each other,
//! not as a forecast of real play.
//!
//! Everything is seeded: the same flags print the same report.

use salt_line_godot::versus::{self, Match, Phase, Rules, Side};
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
}

const KID: Profile = Profile {
    name: "kid",
    wind_read: 0.3,
    learn: 0.7,
    open: 80.0,
    noise: 15.0,
};

const STEADY: Profile = Profile {
    name: "steady",
    wind_read: 0.6,
    learn: 0.85,
    open: 120.0,
    noise: 8.0,
};

/// Power moves in steps of this many ticks, as a key press or a drag does.
const POWER_STEP: i32 = 5;

/// A turn cap per round, so a pathological screen cannot hang the harness.
const MAX_TURNS: u32 = 400;

/// Seed tag for the bots' own randomness, ASCII "DBOT".
const TAG_BOT: u64 = 0x4442_4F54;

struct Args {
    matches: u64,
    width: usize,
    profile: Profile,
    rules: Rules,
}

fn parse_args() -> Args {
    let mut a = Args {
        matches: 200,
        width: 320,
        profile: KID,
        rules: Rules::DEFAULT,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let bad = |flag: &str, v: &str| -> ! {
        eprintln!("bad {flag} '{v}'");
        std::process::exit(2)
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
                let v = value();
                a.profile = match v.as_str() {
                    "kid" => KID,
                    "steady" => STEADY,
                    _ => bad(flag, &v),
                };
            }
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
            "--help" | "-h" => {
                println!(
                    "duel [--matches N] [--width W] [--bot kid|steady] [--wind MAX] [--wind-min MIN] [--reach R] [--equal-shots | --no-equal-shots] [--shift D]"
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

/// The angle a bot keeps for a round: of the hitting shots in calm air, the
/// one nearest 45°.
fn round_angle(duel: &Match, side: Side) -> i32 {
    let mut frame = versus::frame_for(&duel.screen, side);
    frame.wind = 0.0;
    let target = versus::target_in_frame(&duel.screen, side);
    versus::shots_at(
        &frame,
        target,
        &duel.loadout,
        duel.rules.hit_reach,
        GATE_CAP,
    )
    .iter()
    .map(|s| s.angle_ticks)
    .min_by_key(|a| (a - 900).abs())
    .unwrap_or(900)
}

/// One bot's memory for a round.
struct Bot {
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
    shutouts: u64,
    capped: u64,
    matches: u64,
}

fn play_match(seed: u64, args: &Args, t: &mut Tally) {
    let p = args.profile;
    let mut duel = Match::with_rules(seed, args.width, args.rules);
    let mut rng = Pcg32::new(substream(seed, TAG_BOT));
    loop {
        let opener = duel.to_move;
        // What the bots learned of the range is the ground and guns as the
        // round opened; anything since (craters, a roll) shows up only in
        // where their shells land, and the bias learns it from there.
        let reference = duel.clone();
        let mut bots = Side::BOTH.map(|side| Bot {
            angle: round_angle(&duel, side),
            bias: rng.signed(p.open),
            last_wind: None,
            last_hit: None,
        });
        let mut turns = 0;
        while duel.phase == Phase::Aiming && turns < MAX_TURNS {
            let side = duel.to_move;
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
            let report = duel.fire(shot, false).expect("aiming");
            let bot = &mut bots[side.index()];
            bot.bias -= p.learn * (power as f64 - wanted);
            bot.last_wind = Some(wind);
            bot.last_hit = report.hit.then_some(shot);
            t.shots += 1;
            t.hits += report.hit as u64;
            turns += 1;
        }
        t.rounds += 1;
        t.turns += turns as u64;
        let winner = match duel.phase {
            Phase::RoundOver(w) | Phase::MatchOver(w) => w,
            Phase::Aiming => {
                t.capped += 1;
                return;
            }
        };
        t.opener_won += (winner == opener) as u64;
        t.shutouts += (duel.hits[winner.other().index()] == 0) as u64;
        if let Phase::MatchOver(w) = duel.phase {
            t.matches += 1;
            t.first_opener_matches += (w == Match::first_to_move(0)) as u64;
            return;
        }
        duel.next_round();
    }
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

fn main() {
    let args = parse_args();
    let r = args.rules;
    let mut t = Tally::default();
    for m in 0..args.matches {
        play_match(substream(0x5EED_D0E1, m), &args, &mut t);
    }
    println!(
        "bot {} | wind {:.1}..{:.1} | reach {:.1} | equal shots {} | shift {:.0} | {} matches at width {}",
        args.profile.name,
        r.wind_min,
        r.wind_max,
        r.hit_reach,
        if r.equal_shots { "on" } else { "off" },
        r.gun_shift,
        args.matches,
        args.width,
    );
    println!(
        "shots per hit {:.2} | turns per round {:.1} | repeat hits {:.0}% of {} | opener wins round {:.1}% | first opener wins match {:.1}% | shutouts {:.0}%{}",
        t.shots as f64 / t.hits.max(1) as f64,
        t.turns as f64 / t.rounds.max(1) as f64,
        pct(t.repeat_hits, t.repeat_chances),
        t.repeat_chances,
        pct(t.opener_won, t.rounds - t.capped),
        pct(t.first_opener_matches, t.matches),
        pct(t.shutouts, t.rounds - t.capped),
        if t.capped > 0 {
            format!(" | {} rounds hit the turn cap", t.capped)
        } else {
            String::new()
        },
    );
}

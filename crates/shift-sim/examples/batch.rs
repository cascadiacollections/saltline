//! Balance harness: the solver bot plays many seeds; prints how far it gets.
//! `cargo run --release -p shift-sim --example batch -- [seeds]`

use salt_line_sim::rng::Pcg32;
use salt_line_sim::SCREEN_W_REF;
use shift_sim::{bot, Expedition, Input, State};

/// The bot with human-ish aim: each shot nudged by up to `angle` ticks and
/// `power` ticks either way.
fn play_noisy(seed: u64, width: usize, angle: i32, power: i32) -> Expedition {
    let mut run = Expedition::new(seed, width);
    let mut rng = Pcg32::new(seed ^ 0xA1A1);
    for _ in 0..5_000 {
        let Some(mut input) = bot::next_input(&run) else {
            break;
        };
        if let Input::Fire {
            angle_ticks,
            power_ticks,
        } = &mut input
        {
            if angle > 0 {
                *angle_ticks += rng.range_u32(0, 2 * angle as u32) as i32 - angle;
            }
            if power > 0 {
                *power_ticks += rng.range_u32(0, 2 * power as u32) as i32 - power;
            }
        }
        if run.apply(input).is_none() {
            break;
        }
    }
    run
}

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200);
    // Optional aim noise in 0.05-degree ticks and power ticks: `-- 200 20 30`.
    let arg = |i: usize| -> i32 {
        std::env::args()
            .nth(i)
            .and_then(|a| a.parse().ok())
            .unwrap_or(0)
    };
    let (na, np) = (arg(2), arg(3));
    let (mut wins, mut shots, mut shells, mut shifts) = (0u64, 0u64, 0i64, 0u64);
    let mut reached = [0u32; 13];
    for seed in 0..n {
        let run = play_noisy(
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            SCREEN_W_REF,
            na,
            np,
        );
        reached[run.stages_cleared() as usize] += 1;
        if *run.state() != State::Victory && n <= 20 {
            println!(
                "seed #{seed}: stopped at stage {} in {:?}, {} shells, bastions left {}",
                run.stage_index() + 1,
                run.state(),
                run.shells(),
                run.stage().bastions_left()
            );
        }
        shots += u64::from(run.total_shots());
        shifts += run
            .log()
            .iter()
            .filter(|i| **i == shift_sim::Input::Shift)
            .count() as u64;
        if *run.state() == State::Victory {
            wins += 1;
            shells += i64::from(run.shells());
        }
    }
    println!(
        "seeds {n}  victories {wins} ({:.0}%)",
        100.0 * wins as f64 / n as f64
    );
    println!(
        "mean shots/run {:.1}  mean shifts/run {:.1}",
        shots as f64 / n as f64,
        shifts as f64 / n as f64
    );
    if wins > 0 {
        println!(
            "mean shells left on victory {:.1}",
            shells as f64 / wins as f64
        );
    }
    println!("stages cleared histogram (0..=12): {reached:?}");
}

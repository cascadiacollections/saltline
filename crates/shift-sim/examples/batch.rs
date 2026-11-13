//! Balance harness: the solver bot plays many seeds; prints how far it gets.
//! `cargo run --release -p shift-sim --example batch -- [seeds]`

use salt_line_sim::SCREEN_W_REF;
use shift_sim::{bot, State};

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200);
    let (mut wins, mut shots, mut shells, mut shifts) = (0u64, 0u64, 0i64, 0u64);
    let mut reached = [0u32; 13];
    for seed in 0..n {
        let run = bot::play(
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            SCREEN_W_REF,
            5_000,
        );
        reached[run.stages_cleared() as usize] += 1;
        if *run.state() != State::Victory {
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

//! A replay is a seed and a shot log. Reconstruction must turn any log, however
//! wrong (shots after the run ends, draft picks with nothing on offer, screens
//! out of order, negative angles), into a clean `Err` rather than a panic. A
//! log that does replay must replay to the same state every time.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use salt_line_sim::replay::{state_hash, Replay};
use salt_line_sim::run::{buffer_width, LoggedShot};

#[derive(Arbitrary, Debug)]
struct Input {
    seed: u64,
    /// Width comes from the device and is always clamped by the game.
    width: u16,
    log: Vec<(u8, i16, i16)>,
}

fuzz_target!(|input: Input| {
    let replay = Replay {
        run_seed: input.seed,
        width: buffer_width(input.width as usize),
        log: input
            .log
            .iter()
            .take(48)
            .map(|&(screen, angle, power)| LoggedShot {
                screen_index: screen as u32,
                angle_ticks: angle as i32,
                power_ticks: power as i32,
            })
            .collect(),
    };
    if let Ok(state) = replay.reconstruct() {
        let again = replay.reconstruct().expect("a log that replayed once failed the second time");
        assert_eq!(state_hash(&state), state_hash(&again));
    }
});

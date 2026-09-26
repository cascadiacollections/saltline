//! A replay is a seed and a shot log. Reconstruction must turn any log, however
//! wrong (shots after the run ends, draft picks with nothing on offer, screens
//! out of order, negative angles, a fuse out of range, an emplacement moved
//! without Enfilade or after the first shot), into a clean `Err` rather than a
//! panic. A log that does replay must replay to the same state every time, and
//! its text form must parse back to the same log.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use salt_line_sim::replay::{decode_log, encode_log, state_hash, Replay};
use salt_line_sim::run::{buffer_width, LoggedShot};

#[derive(Arbitrary, Debug)]
struct Input {
    seed: u64,
    /// Width comes from the device and is always clamped by the game.
    width: u16,
    /// `(screen, angle, power, fuse tenths, emplacement offset)`.
    log: Vec<(u8, i16, i16, i16, i8)>,
}

fuzz_target!(|input: Input| {
    let replay = Replay {
        run_seed: input.seed,
        width: buffer_width(input.width as usize),
        log: input
            .log
            .iter()
            .take(48)
            .map(|&(screen, angle, power, fuse, offset)| LoggedShot {
                screen_index: screen as u32,
                angle_ticks: angle as i32,
                power_ticks: power as i32,
                fuse_tenths: fuse as i32,
                emplacement_offset: offset as i32,
            })
            .collect(),
    };
    if let Ok(state) = replay.reconstruct() {
        let again = replay
            .reconstruct()
            .expect("a log that replayed once failed the second time");
        assert_eq!(state_hash(&state), state_hash(&again));
        assert_eq!(decode_log(&encode_log(&state.log)).as_ref(), Ok(&state.log));
    }
});

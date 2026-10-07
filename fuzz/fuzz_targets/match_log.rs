//! A suspended versus match is a line in `user://match.txt` (DESIGN §11.6),
//! which a launch parses and replays. Two ways in, both of which must end in
//! a clean `Err` or `None`, never a panic:
//!
//! - **Raw text**, as a damaged or hand-edited file would hold: whatever
//!   parses must survive being written back and read again unchanged.
//! - **Any log**, built from arbitrary entries (shots anywhere on the dial
//!   or off it, new rounds where no round was won, shots after the match is
//!   over): the line round-trips, and a log that replays replays to the
//!   same match twice.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use salt_line_godot::match_log::{judge, state_hash, Entry, SavedMatch};
use salt_line_sim::run::buffer_width;
use salt_line_sim::{SCREEN_W_MAX, SIM_EPOCH};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    text: &'a str,
    seed: u64,
    /// Width comes from the device and is always clamped by the game.
    width: u16,
    rolling: bool,
    help: [bool; 2],
    /// `None` is a new round; `Some` a shot, clamped onto the dial.
    log: Vec<Option<(u16, u16)>>,
}

fuzz_target!(|input: Input| {
    if let Ok(saved) = SavedMatch::parse(input.text) {
        assert_eq!(SavedMatch::parse(&saved.to_line()), Ok(saved));
    }
    // Whatever the text, launch's verdict on it is reached without a panic.
    let _ = judge(Some(input.text));

    let saved = SavedMatch {
        epoch: SIM_EPOCH,
        match_epoch: salt_line_godot::match_log::MATCH_EPOCH,
        seed: input.seed,
        width: buffer_width((input.width as usize).min(SCREEN_W_MAX * 2)),
        rolling: input.rolling,
        help: input.help,
        // A few dozen entries is a long match; replays are the slow half.
        log: input
            .log
            .iter()
            .take(48)
            .map(|e| match e {
                None => Entry::NextRound,
                Some((a, p)) => Entry::Shot((*a % 1801) as i32, (*p % 1001) as i32),
            })
            .collect(),
    };
    let line = saved.to_line();
    assert_eq!(SavedMatch::parse(&line).as_ref(), Ok(&saved));
    if let Some(duel) = saved.replay() {
        let again = saved
            .replay()
            .expect("a log that replayed once failed the second time");
        assert_eq!(state_hash(&duel), state_hash(&again));
    }
});

//! Seed entry takes any text. Parsing must never panic, and a seed's canonical
//! text must parse back to the same seed, or a shared seed would not be the
//! same run.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_sim::run::{format_seed, parse_seed};

fuzz_target!(|text: &str| {
    let seed = parse_seed(text);
    assert_eq!(parse_seed(&format_seed(seed)), seed);
});

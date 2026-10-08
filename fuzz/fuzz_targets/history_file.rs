//! Finished runs and matches are kept in `user://history.txt` (DESIGN §9),
//! which a launch reads whatever it holds: torn by a crash, edited by hand,
//! or written by a newer build. The reader must never panic, and:
//!
//! - **Nothing read is changed by writing it.** Whatever records the text
//!   parses to, writing them out and reading them again gives the same
//!   records, each line its own text again.
//! - **A record's check never panics** (`Record::check`, which WATCH runs
//!   before anything is shown): it replays the line off screen and either
//!   hands over what to watch or says why not.
//! - **The cap holds**: never more than `MAX_RECORDS` read.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_godot::history::{sound, History, Record, MAX_RECORDS};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = sound(text);
    let history = History::parse(text);
    assert!(history.records.len() <= MAX_RECORDS);
    let again = History::parse(&history.serialize());
    assert_eq!(again.records, history.records);
    assert_eq!(again.damaged, 0);
    for record in &history.records {
        let line = record.to_line();
        assert_eq!(Record::parse(&line).as_ref(), Some(record));
        let _ = record.version();
        let _ = record.summary();
        // A replay is the slow half: only a few short ones per input.
        if line.len() < 2048 {
            let _ = record.check();
        }
    }
});

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
//! - **The caps hold**: never more than `MAX_RECORDS` read, whatever the
//!   size of the file, nor more than `MAX_STARS` starred.
//! - **A star is not the record's**: each record's own line reads back to
//!   the same record unstarred (the star is a line of its own, never part
//!   of the game a share code carries), and the header says a format a
//!   build before stars leaves alone whenever anything is starred.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_godot::history::{sound, History, Record, FORMAT_PLAIN, MAX_RECORDS, MAX_STARS};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = sound(text);
    let history = History::parse(text);
    assert!(history.records.len() <= MAX_RECORDS);
    assert!(history.stars() <= MAX_STARS);
    let text = history.serialize();
    let again = History::parse(&text);
    assert_eq!(again.records, history.records);
    let plain = text.starts_with(&format!("history {FORMAT_PLAIN}\n"));
    assert_eq!(plain, history.stars() == 0);
    assert_eq!((again.damaged, again.newer), (0, 0));
    for record in &history.records {
        let line = record.to_line();
        let mut unstarred = record.clone();
        unstarred.starred = false;
        assert_eq!(Record::parse(&line), Ok(unstarred));
        let _ = record.version();
        let _ = record.summary();
        // A replay is the slow half: only a few short ones per input.
        if line.len() < 2048 {
            let _ = record.check();
        }
    }
});

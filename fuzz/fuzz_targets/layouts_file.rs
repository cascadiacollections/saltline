//! The player's saved layouts (`user://layouts.txt`) and the editor's
//! draft (`user://draft.txt`, DESIGN §13.8, §13.11) are files a launch
//! reads whatever they hold: torn by a crash, edited by hand, or written by
//! a newer build. Both readers must never panic, and:
//!
//! - **The slot file** loses nothing on a round trip: whatever it parses
//!   to, writing that out and reading it again gives the same slots and the
//!   same kept lines.
//! - **The draft line** that parses writes back to a line that parses to
//!   the same draft.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_godot::layouts_file::{draft_sound, SavedDraft, Slots};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let slots = Slots::parse(text);
    // A file from a newer build is never written, so only this build's
    // need survive being written.
    if !slots.from_the_future && !slots.damaged {
        assert_eq!(Slots::parse(&slots.serialize()), slots);
    }
    for e in &slots.entries {
        let _ = e.layout();
    }

    let _ = draft_sound(text);
    if let Ok(draft) = SavedDraft::parse(text) {
        let line = draft
            .to_line()
            .expect("a draft that was read can be written");
        assert_eq!(SavedDraft::parse(&line), Ok(draft));
    }
});

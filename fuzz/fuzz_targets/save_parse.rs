//! The save file is plain text the player owns and may edit by hand, so the
//! parser meets arbitrary input. It must never panic, and whatever it makes of
//! the input must survive being written back and read again unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_godot::save_format::Save;

fuzz_target!(|text: &str| {
    let save = Save::parse(text);
    // A save from a newer build is never written back (see Save::from_the_future).
    if save.from_the_future {
        return;
    }
    assert_eq!(Save::parse(&save.serialize()), save);
});

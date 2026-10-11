//! Local persistence: the unlock flag and the leaderboard. Brief M6.
//!
//! This file is the engine side only — opening, writing and replacing the
//! file. The format itself lives in `save_format`, with no engine in it, so it
//! can be tested headless.

use godot::classes::file_access::ModeFlags;
use godot::classes::{DirAccess, FileAccess};
use godot::prelude::*;

pub use crate::save_format::{Entry, Save, KEEP, VERSION};

pub const SAVE_PATH: &str = "user://salt_line.txt";
/// Written in full, then moved over the real file. A crash or a full disk
/// during the write costs the new record, not the existing board.
pub const TEMP_PATH: &str = "user://salt_line.txt.new";

/// Read the save file. A missing file is an empty board, not an error.
pub fn load() -> Save {
    let Some(f) = FileAccess::open(SAVE_PATH, ModeFlags::READ) else {
        return Save::default();
    };
    let save = Save::parse(&f.get_as_text().to_string());
    if save.from_the_future {
        godot_warn!(
            "salt-line: {SAVE_PATH} was written by a newer build (this one writes \
             version {VERSION}); starting an empty board and leaving the file alone"
        );
    }
    save
}

/// Persist the save. Returns whether it reached disk.
pub fn store(save: &Save) -> bool {
    if save.from_the_future {
        // Refusing to write is the whole point of the flag: do not overwrite a
        // newer file with a downgraded build's understanding of it.
        return false;
    }
    let text = save.serialize();
    let Some(mut f) = FileAccess::open(TEMP_PATH, ModeFlags::WRITE) else {
        godot_warn!("salt-line: could not write {TEMP_PATH}");
        return false;
    };
    f.store_string(&GString::from(text.as_str()));
    let err = f.get_error();
    f.close();
    if err != godot::global::Error::OK {
        godot_warn!("salt-line: {TEMP_PATH} was not written cleanly; keeping the old save");
        return false;
    }
    let Some(mut dir) = DirAccess::open("user://") else {
        godot_warn!("salt-line: could not open user:// to replace {SAVE_PATH}");
        return false;
    };
    // `rename` replaces the destination if it exists, which is what makes the
    // swap atomic from the player's point of view.
    if dir.rename(TEMP_PATH, SAVE_PATH) != godot::global::Error::OK {
        godot_warn!("salt-line: could not replace {SAVE_PATH}; the old save is intact");
        return false;
    }
    true
}

/// Record a finished run and persist. Returns its position on the board, or
/// `None` if it did not make the cut.
pub fn record(save: &mut Save, entry: Entry) -> Option<usize> {
    let at = save.place(entry);
    store(save);
    at
}

/// Keep Trial replays separate from the existing artillery save format.
pub fn store_trial_replay(replay: &salt_line_sim::trial::Replay) -> bool {
    let Ok(text) = replay.encode() else {
        return false;
    };
    let temporary = "user://drone_trial.replay.new";
    let target = "user://drone_trial.replay";
    let Some(mut file) = FileAccess::open(temporary, ModeFlags::WRITE) else {
        return false;
    };
    file.store_string(&GString::from(text.as_str()));
    let error = file.get_error();
    file.close();
    if error != godot::global::Error::OK {
        return false;
    }
    let Some(mut dir) = DirAccess::open("user://") else {
        return false;
    };
    dir.rename(temporary, target) == godot::global::Error::OK
}

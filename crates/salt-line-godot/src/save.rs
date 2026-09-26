//! Local persistence: the unlock flag and the leaderboard. Brief M6.
//!
//! This file is the engine side only — finding `user://` on disk. The format
//! lives in `save_format` and the write/recovery rules in `durable`, with no
//! engine in either, so both can be tested headless.

use std::path::PathBuf;

use godot::classes::file_access::ModeFlags;
use godot::classes::{DirAccess, FileAccess, ProjectSettings};
use godot::prelude::*;

use crate::durable::{self, Pick};
pub use crate::save_format::{Entry, Save, KEEP, VERSION};

pub const SAVE_PATH: &str = "user://salt_line.txt";
/// Written in full, then moved over the real file. A crash or a full disk
/// during the write costs the new record, not the existing board.
pub const TEMP_PATH: &str = "user://salt_line.txt.new";

/// A `user://` path as a real filesystem path, for `std::fs`. `None` where the
/// platform has no such thing (Godot hands the virtual path back unchanged);
/// callers then fall back to Godot's own file API.
pub fn on_disk(user_path: &str) -> Option<PathBuf> {
    let global = ProjectSettings::singleton()
        .globalize_path(user_path)
        .to_string();
    if global.is_empty() || global.contains("://") {
        return None;
    }
    Some(PathBuf::from(global))
}

fn read(user_path: &str) -> Option<String> {
    match on_disk(user_path) {
        Some(p) => std::fs::read_to_string(p).ok(),
        None => FileAccess::open(user_path, ModeFlags::READ).map(|f| f.get_as_text().to_string()),
    }
}

/// Read the save file. A missing file is an empty board, not an error. If the
/// save is missing or empty but the temporary file survived — the game went
/// down between writing it and moving it into place — that is loaded instead.
pub fn load() -> Save {
    let main = read(SAVE_PATH);
    let temp = read(TEMP_PATH);
    let save = match durable::pick(main.as_deref(), temp.as_deref()) {
        Pick::Main => Save::parse(main.as_deref().unwrap_or_default()),
        Pick::Temp => {
            godot_warn!("salt-line: {SAVE_PATH} missing or damaged; recovered {TEMP_PATH}");
            Save::parse(temp.as_deref().unwrap_or_default())
        }
        Pick::Empty => Save::default(),
    };
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
    if crate::crash::panicked() {
        // After a panic the state in memory is not trusted enough to replace
        // the last good save with. `record` goes through here too.
        return false;
    }
    let text = save.serialize();
    if let (Some(main), Some(temp)) = (on_disk(SAVE_PATH), on_disk(TEMP_PATH)) {
        return match durable::write_atomic(&main, &temp, &text) {
            Ok(()) => true,
            Err(e) => {
                godot_warn!("salt-line: could not write {SAVE_PATH} ({e}); the old save is intact");
                false
            }
        };
    }
    store_via_engine(&text)
}

/// Where `user://` has no real path: Godot's file API, write-then-rename, but
/// with no way to flush the data before the rename.
fn store_via_engine(text: &str) -> bool {
    let Some(mut f) = FileAccess::open(TEMP_PATH, ModeFlags::WRITE) else {
        godot_warn!("salt-line: could not write {TEMP_PATH}");
        return false;
    };
    f.store_string(&GString::from(text));
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
    if dir.rename(TEMP_PATH, SAVE_PATH) != godot::global::Error::OK {
        godot_warn!("salt-line: could not replace {SAVE_PATH}; the old save is intact");
        return false;
    }
    true
}

/// What happened to a finished run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recorded {
    /// Its position on the board, or `None` if it did not make the cut.
    pub place: Option<usize>,
    /// Whether the board reached disk. The player is told when it did not.
    pub saved: bool,
}

/// Record a finished run and persist.
pub fn record(save: &mut Save, entry: Entry) -> Recorded {
    if crate::crash::panicked() {
        // A score from a run that panicked is not one to put on the board,
        // even in memory.
        return Recorded {
            place: None,
            saved: false,
        };
    }
    let place = save.place(entry);
    let saved = store(save);
    Recorded { place, saved }
}

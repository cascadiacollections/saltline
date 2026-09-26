//! Local persistence: settings, bindings and the leaderboard. Brief M6.
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

/// A `user://` file's text. `None` if it is missing or unreadable.
pub fn read(user_path: &str) -> Option<String> {
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
        // the last good save with, a finished run's score included.
        return false;
    }
    write(SAVE_PATH, TEMP_PATH, &save.serialize())
}

/// Replace a `user://` file with `text` by way of `temp`:
/// `durable::write_atomic` where there is a disk path, Godot's file API where
/// there is not. Returns whether it reached disk; on failure the old file is
/// left as it was.
pub fn write(main: &str, temp: &str, text: &str) -> bool {
    if let (Some(main_disk), Some(temp_disk)) = (on_disk(main), on_disk(temp)) {
        return match durable::write_atomic(&main_disk, &temp_disk, text) {
            Ok(()) => true,
            Err(e) => {
                godot_warn!("salt-line: could not write {main} ({e}); the old file is intact");
                false
            }
        };
    }
    write_via_engine(main, temp, text)
}

/// Move a `user://` file, replacing any file at `to`. Returns whether it
/// moved; a missing `from` is not worth a warning.
pub fn rename(from: &str, to: &str) -> bool {
    if let (Some(f), Some(t)) = (on_disk(from), on_disk(to)) {
        return match std::fs::rename(&f, &t) {
            Ok(()) => true,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    godot_warn!("salt-line: could not move {from} to {to} ({e})");
                }
                false
            }
        };
    }
    let Some(mut dir) = DirAccess::open("user://") else {
        return false;
    };
    FileAccess::file_exists(from) && dir.rename(from, to) == godot::global::Error::OK
}

/// Delete a `user://` file if it is there.
pub fn remove(user_path: &str) {
    match on_disk(user_path) {
        Some(p) => {
            if let Err(e) = std::fs::remove_file(&p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    godot_warn!("salt-line: could not delete {user_path} ({e})");
                }
            }
        }
        None => {
            if FileAccess::file_exists(user_path) {
                if let Some(mut dir) = DirAccess::open("user://") {
                    dir.remove(user_path);
                }
            }
        }
    }
}

/// Where `user://` has no real path: Godot's file API, write-then-rename, but
/// with no way to flush the data before the rename.
fn write_via_engine(main: &str, temp: &str, text: &str) -> bool {
    let Some(mut f) = FileAccess::open(temp, ModeFlags::WRITE) else {
        godot_warn!("salt-line: could not write {temp}");
        return false;
    };
    f.store_string(&GString::from(text));
    let err = f.get_error();
    f.close();
    if err != godot::global::Error::OK {
        godot_warn!("salt-line: {temp} was not written cleanly; keeping the old file");
        return false;
    }
    let Some(mut dir) = DirAccess::open("user://") else {
        godot_warn!("salt-line: could not open user:// to replace {main}");
        return false;
    };
    if dir.rename(temp, main) != godot::global::Error::OK {
        godot_warn!("salt-line: could not replace {main}; the old file is intact");
        return false;
    }
    true
}

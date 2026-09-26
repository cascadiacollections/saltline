//! Engine-free rules for getting the save file onto disk and back. Brief M6.
//!
//! `save` does the engine side (where `user://` is); this is the part that can
//! be tested headless: the atomic replace, which copy to trust on the way back
//! in, and when a burst of settings changes is worth a write.
//!
//! The save is the leaderboard, and a leaderboard that can be lost to a power
//! cut at the wrong moment is not a record of anything.
//!
//! # The write: temp, fsync, rename, directory fsync
//!
//! [`write_atomic`] is the classic write-then-rename replace. Each step is
//! there for a failure the one before it leaves open:
//!
//! ```text
//! step                          why                          a crash during it leaves
//! 1 write all of it to <temp>   main is never opened for     main untouched; temp torn
//!   (create truncates any old)  writing, so is never torn    or zero-filled
//! 2 fsync <temp> (sync_all)     the data reaches the device  main untouched; temp
//!                               before any rename can be     possibly still torn
//!                               committed ahead of it
//! 3 rename <temp> over main     one swap (atomic on POSIX):  old main or new main,
//!                               a reader sees old or new     each whole
//! 4 fsync the directory         the rename itself reaches    old main or new main,
//!   (Unix only; errors ignored) the device                   each whole; once it
//!                                                            succeeds, the new one
//! ```
//!
//! Step 2 is the one most often left out. A journalling filesystem with
//! delayed allocation (ext4) may commit the rename's metadata before the
//! file's data; power lost in between leaves a file of the right length full
//! of zeros, or an empty one. That is what the NUL and `version`-line tests
//! in [`pick`] catch. The run files (`resume`) do not use `pick`: `run.txt`
//! wins whenever it exists.
//!
//! # The read: which copy to trust
//!
//! [`pick`] looks at both files and prefers, in order: a *sound* main, a
//! sound temp, a merely *legible* main, a legible temp, and otherwise an
//! empty board. A sound main beats a sound temp even though the temp may be
//! newer: a temp beside a sound main is a write that did not reach step 3,
//! and the old save is the one known to be whole. The temp only wins when
//! main is missing or damaged, as on the very first save.
//!
//! # Platform caveats
//!
//! On Unix `rename` is atomic by POSIX. On Windows `std` calls `MoveFileExW`
//! or `SetFileInformationByHandle`; Microsoft does not document either as
//! atomic, though NTFS journals the rename and in practice a reader sees one
//! file or the other. Windows has no directory fsync through `std`, and `std`
//! does not ask for write-through on the rename, so there step 4 is skipped:
//! a power cut just after the rename may bring back the old save, never a
//! torn one. Where `user://` has no real path at all (`save::on_disk`
//! returns `None`), `save` falls back to Godot's file API, which has no
//! fsync: write-then-rename still keeps main from being torn by a crash of
//! the game, but not by a power cut.
//!
//! A save written by a newer build is loaded as an empty board and never
//! written over; that rule is `save_format`'s `from_the_future` flag, and
//! `save::store` enforces it.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Replace `main` with `text`, never leaving a half-written file in its place.
///
/// The new contents go to `temp` in full and are flushed to the device
/// (`sync_all`) before `temp` is renamed over `main`. Without the flush a
/// journalling filesystem may commit the rename before the data, and a crash
/// then leaves an empty save where the old one was. On Unix `rename` is
/// atomic by POSIX. On Windows `std` calls `MoveFileExW` or
/// `SetFileInformationByHandle`; Microsoft does not document either as
/// atomic, though NTFS journals the rename and in practice a reader sees one
/// file or the other. On Unix the directory is flushed too, so the rename
/// itself survives; failing that is not worth failing the save over. Windows
/// has no directory flush through `std` (and `std` does not ask for
/// `MOVEFILE_WRITE_THROUGH`), so there a power cut just after the swap may
/// undo it: the old save, never a torn one. See the module docs for what a
/// crash at each step leaves.
pub fn write_atomic(main: &Path, temp: &Path, text: &str) -> io::Result<()> {
    {
        let mut f = File::create(temp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(temp, main)?;
    #[cfg(unix)]
    if let Some(dir) = main.parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Which copy of the save to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The save itself.
    Main,
    /// The temporary file: the save is missing or damaged, and the game went
    /// down between writing the temporary file and moving it into place, so
    /// it is the best complete copy there is.
    Temp,
    /// Neither: a first launch, or nothing readable. An empty board.
    Empty,
}

/// Choose between the save and its temporary file. `None` is missing or
/// unreadable.
///
/// A copy is *sound* if it has a `version` line and no NUL bytes. NULs are
/// what a crash leaves on a filesystem that committed a file's length but not
/// its data (a zero-filled tail, or nothing but zeros); a save without its
/// header line is torn, or not a save at all. A sound save wins over
/// anything: a leftover temporary file beside it is a write that failed
/// before the rename, the case the temporary file exists to make harmless.
/// Then a sound temporary file.
///
/// Failing both, a copy that is merely *legible* — anything besides NULs and
/// whitespace — still beats an empty board, save first. Files from before the
/// format had a header (`save_format` reads versionless as version 1) land
/// here, and load as they always did.
pub fn pick(main: Option<&str>, temp: Option<&str>) -> Pick {
    let sound = |t: Option<&str>| {
        t.is_some_and(|t| {
            !t.contains('\0')
                && t.lines()
                    .any(|l| l.split_whitespace().next() == Some("version"))
        })
    };
    let legible =
        |t: Option<&str>| t.is_some_and(|t| t.chars().any(|c| c != '\0' && !c.is_whitespace()));
    if sound(main) {
        Pick::Main
    } else if sound(temp) {
        Pick::Temp
    } else if legible(main) {
        Pick::Main
    } else if legible(temp) {
        Pick::Temp
    } else {
        Pick::Empty
    }
}

/// Quiet time after the last settings change before it is written.
pub const SETTLE: f64 = 0.5;

/// Batches settings writes. Holding a volume button repeats about 28 times a
/// second; writing (and flushing) the whole save on every step is wasted wear
/// on a phone's flash for a value that will change again in 35 ms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Debounce {
    dirty: bool,
    /// Seconds since the last `touch`.
    idle: f64,
}

impl Debounce {
    /// A setting changed.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.idle = 0.0;
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Advance by one frame. Returns true when the pending change should be
    /// written now: once `SETTLE` seconds pass with no further change, or at
    /// once when the player is no longer on the screen making changes.
    pub fn tick(&mut self, delta: f64, editing: bool) -> bool {
        if !self.dirty {
            return false;
        }
        self.idle += delta;
        if !editing || self.idle >= SETTLE {
            self.dirty = false;
            return true;
        }
        false
    }

    /// Write now, whatever the timer says: pause, focus loss, close. Returns
    /// whether there was anything to write.
    pub fn take(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("salt-line-durable-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_write_replaces_the_save_and_leaves_no_temp_behind() {
        let s = Scratch::new();
        let (main, temp) = (s.0.join("save.txt"), s.0.join("save.txt.new"));
        write_atomic(&main, &temp, "first").unwrap();
        write_atomic(&main, &temp, "second").unwrap();
        assert_eq!(fs::read_to_string(&main).unwrap(), "second");
        assert!(!temp.exists());
    }

    #[test]
    fn a_failed_write_leaves_the_old_save_alone() {
        let s = Scratch::new();
        let main = s.0.join("save.txt");
        fs::write(&main, "board").unwrap();
        // The temporary file's directory does not exist, so the write fails
        // before anything touches the save.
        let temp = s.0.join("missing").join("save.txt.new");
        assert!(write_atomic(&main, &temp, "lost").is_err());
        assert_eq!(fs::read_to_string(&main).unwrap(), "board");
    }

    #[rstest]
    #[case::normal(Some("version 2\n"), None, Pick::Main)]
    #[case::stale_temp_ignored(Some("version 2\n"), Some("version 2\nrun 1 2 3 4 0\n"), Pick::Main)]
    #[case::crash_before_rename(None, Some("version 2\n"), Pick::Temp)]
    #[case::emptied_by_a_crash(Some(""), Some("version 2\n"), Pick::Temp)]
    #[case::whitespace_only(Some(" \n"), Some("version 2\n"), Pick::Temp)]
    #[case::zero_filled(Some("\0\0\0\0"), Some("version 2\n"), Pick::Temp)]
    #[case::zero_filled_tail(Some("version 2\nvol\0\0\0"), Some("version 2\n"), Pick::Temp)]
    #[case::headerless_garbage(Some("}{ not a save"), Some("version 2\n"), Pick::Temp)]
    #[case::versionless_legacy_alone(Some("unlocked 1\n"), None, Pick::Main)]
    #[case::torn_main_beats_blank_temp(Some("version 2\nvol\0"), Some(""), Pick::Main)]
    #[case::torn_temp_beats_nothing(None, Some("version 2\nvol\0"), Pick::Temp)]
    #[case::all_zeros_everywhere(Some("\0\0"), Some("\0"), Pick::Empty)]
    #[case::first_launch(None, None, Pick::Empty)]
    #[case::both_empty(Some(""), Some("\n"), Pick::Empty)]
    fn the_right_copy_is_loaded(
        #[case] main: Option<&str>,
        #[case] temp: Option<&str>,
        #[case] want: Pick,
    ) {
        assert_eq!(pick(main, temp), want);
    }

    #[test]
    fn held_volume_writes_once_after_it_settles() {
        let mut d = Debounce::default();
        let mut writes = 0;
        // Two seconds of a held button at 60 fps, a change every frame.
        for _ in 0..120 {
            d.touch();
            writes += d.tick(1.0 / 60.0, true) as u32;
        }
        assert_eq!(writes, 0);
        for _ in 0..60 {
            writes += d.tick(1.0 / 60.0, true) as u32;
        }
        assert_eq!(writes, 1);
        assert!(!d.dirty());
    }

    #[test]
    fn leaving_the_screen_writes_at_once() {
        let mut d = Debounce::default();
        d.touch();
        assert!(!d.tick(0.01, true));
        assert!(d.tick(0.01, false));
        assert!(!d.tick(0.01, false), "written once, not every frame");
    }

    #[test]
    fn take_reports_whether_anything_was_pending() {
        let mut d = Debounce::default();
        assert!(!d.take());
        d.touch();
        assert!(d.take());
        assert!(!d.take());
        assert!(!d.tick(1.0, false));
    }
}

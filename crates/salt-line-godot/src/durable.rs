//! Engine-free rules for getting the save file onto disk and back. Brief M6.
//!
//! `save` does the engine side (where `user://` is); this is the part that can
//! be tested headless: the atomic replace, which copy to trust on the way back
//! in, and when a burst of settings changes is worth a write.
//!
//! The save is the leaderboard, and a leaderboard that can be lost to a power
//! cut at the wrong moment is not a record of anything.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Replace `main` with `text`, never leaving a half-written file in its place.
///
/// The new contents go to `temp` in full and are flushed to the device
/// (`sync_all`) before `temp` is renamed over `main`. Without the flush a
/// journalling filesystem may commit the rename before the data, and a crash
/// then leaves an empty save where the old one was. `rename` replaces the
/// destination atomically on Unix, and via `MoveFileEx(REPLACE_EXISTING)` on
/// Windows. On Unix the directory is flushed too, so the rename itself
/// survives; failing that is not worth failing the save over.
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
    /// The temporary file: the game went down between writing it and moving
    /// it into place, so it is the newest complete copy there is.
    Temp,
    /// Neither: a first launch, or nothing readable. An empty board.
    Empty,
}

/// Choose between the save and its temporary file. `None` is missing or
/// unreadable. A file of nothing but whitespace counts as unreadable too: it
/// is what a crash leaves behind on a filesystem that committed the file's
/// creation but not its data, and it holds no board worth preferring.
///
/// The save wins whenever it has anything in it. A leftover temporary file
/// beside a good save is a write that failed before the rename, which is the
/// case the temporary file exists to make harmless.
pub fn pick(main: Option<&str>, temp: Option<&str>) -> Pick {
    let usable = |t: Option<&str>| t.is_some_and(|t| !t.trim().is_empty());
    if usable(main) {
        Pick::Main
    } else if usable(temp) {
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
    #[case::normal(Some("v 3"), None, Pick::Main)]
    #[case::stale_temp_ignored(Some("v 3"), Some("v 3 newer"), Pick::Main)]
    #[case::crash_before_rename(None, Some("v 3"), Pick::Temp)]
    #[case::emptied_by_a_crash(Some(""), Some("v 3"), Pick::Temp)]
    #[case::whitespace_only(Some(" \n"), Some("v 3"), Pick::Temp)]
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

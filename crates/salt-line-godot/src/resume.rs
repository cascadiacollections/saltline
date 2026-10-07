//! Resuming a run the operating system killed. DESIGN §9.6.
//!
//! Phones kill backgrounded apps, and a run is only a seed and a shot log
//! (DESIGN §3), so the in-progress run is kept on disk as exactly that:
//! `user://run.txt`, one line, rewritten (atomically, `durable::write_atomic`)
//! every time the log grows. On launch the line is rebuilt with
//! `Replay::reconstruct` and the title screen offers CONTINUE.
//!
//! The file may also start with the lines of finished runs whose score did
//! not reach the board (a full disk, a save from a newer build): one line a
//! run, the same format, kept until a score or a launch gets them there
//! (`app::RunFileState::unscored`). [`judge`] reads one line; the core
//! splits the file.
//!
//! Engine-free: the file format and the decision (offer it, score it, or
//! throw it away) live here, where they can be tested headless. `game` does
//! the reading, writing and the menu.
//!
//! A shot that was in the air when the app died is already in the log (the
//! sim resolves a shot the instant it is fired; the flight is presentation),
//! so it is committed and the run resumes with it resolved. A run whose last
//! shot ended it is not offered to continue: it is a finished run whose score
//! never reached the board, and is recorded there instead.
//!
//! # The run files
//!
//! Three `user://` paths carry the run. The core asks for changes as
//! `app::RunFile` effects (and a score's `clear`); `game.rs` turns them into
//! file operations through `save::write` (the atomic replace in
//! `durable::write_atomic`), `save::rename` and `save::remove`, the
//! multi-step ones by way of [`take_saved`], [`replace`] and [`discard`]
//! below.
//!
//! ```text
//! file              written by                       when
//! run.txt.new       save::write, step 1-2 of the     every RunFile::Write (the log
//!                   atomic replace                   grew, or a finished run's
//!                                                    score did not reach disk)
//! run.txt           the rename at step 3 of the      straight after run.txt.new;
//!                   same replace; RunFile::Restore   Restore once a launch has
//!                                                    rebuilt the run
//! run-loading.txt   take_saved moving run.txt (or   at launch, before the text
//!                   run.txt.new, if run.txt is       reaches the core, so before
//!                   missing or damaged) aside        any rebuild
//! ```
//!
//! `RunFile::Discard` deletes all three, and if a file will not go, writes
//! an empty `run.txt`, which [`judge`] reads as no run ([`discard`]). A
//! finished run's score clears the files the same way ([`replace`]) before
//! the board is written, and only goes ahead if that is confirmed.
//!
//! What a crash costs, step by step (crash / next launch finds / cost):
//!
//! ```text
//! writing run.txt.new
//!     finds  the previous line in run.txt; with no run.txt, or a damaged
//!            one, run.txt.new itself if it is whole (`take_saved`)
//!     cost   one entry behind; a torn run.txt.new with nothing better
//!            beside it fails `judge` and is discarded with a note
//! after the rename
//!     finds  the new line
//!     cost   nothing
//! rebuilding at launch (killed, or a log that brings the sim down)
//!     finds  run-loading.txt
//!     cost   that run, once: discarded with a note, run.txt.new with it
//!            (left by a write that died before its rename, it would be
//!            picked next, and likely crash the rebuild again)
//! after RunFile::Restore
//!     finds  run.txt
//!     cost   nothing
//! a run ends: its Store clears the run file, then writes the board
//!     finds  no run file, or run.txt blank, once the clear is done
//!     cost   that score, never scored twice
//! a run ends: the run file will not go (not deleted, not blanked)
//!     finds  run.txt, the finished log
//!     cost   nothing: the board was not written, so it is scored by the
//!            first launch that can clear it
//! a run ends: the Store fails (full disk, a newer build's save)
//!     finds  run.txt, its line kept by a RunFile::Write, and carried at
//!            the top of every later Write this session
//!     cost   nothing, unless that write fails too
//! ```
//!
//! Two properties fall out of that ordering:
//!
//! - **A run is scored at most once.** A score's `Store` clears the
//!   finished log first ([`replace`]) and writes the board only once no copy
//!   of it is left to read (`app`'s "order is behaviour"). By the time a
//!   score can be on the board the log is gone, so no launch can find it
//!   and score it again. A crash in between loses the score instead; a log
//!   that will not go keeps its score off the board until a launch can
//!   clear it. A finished log is written back only after its `Store` has
//!   failed, so never beside a board that holds it.
//!   Against a power cut it also needs the deletes to be durable before the
//!   save's rename: on Unix the directory fsync after that rename covers
//!   them (same directory); on Windows nothing forces it.
//! - **A corrupt run file costs one crash, not every launch.** The file is
//!   moved to `run-loading.txt` before it is parsed or replayed, and is put
//!   back only once [`judge`] has returned. If the rebuild panics, the
//!   `Restore` is never applied (no file effect survives a panic, see
//!   `app::Effect::survives_panic`); the next launch sees
//!   `run-loading.txt`, never replays it, and throws it away
//!   (`app::Abandoned::Interrupted`, [`drop_set_aside`]), leaving any
//!   `run.txt` beside it for the launch after. Text that merely fails to parse or
//!   replay is caught by [`judge`] as [`Stale`] and discarded with a note,
//!   without a crash at all. If the move aside at launch fails, the
//!   one-crash guard is lost for that launch.
//!
//! `app/disk_tests.rs` holds the first property, and the resumed run
//! matching the run as played, end to end: it applies the core's effects to
//! an in-memory disk, kills the process after every effect in turn, and
//! launches again on what was left. It runs the functions below on its
//! files, so a crash between any two of their steps is exercised, and so is
//! a `run.txt` that will not go; that each write really is atomic is
//! `durable`'s job.

use salt_line_sim::replay::{decode_log, encode_log, Replay, DRAFT_MARK};
use salt_line_sim::run::{LoggedShot, RunState};
use salt_line_sim::{SCREEN_W_MAX, SCREEN_W_MIN, SIM_EPOCH};

use crate::app::SavedRun;
use crate::durable::{self, Pick};

pub const RUN_PATH: &str = "user://run.txt";
/// Written in full, then moved over `RUN_PATH`.
pub const RUN_TEMP_PATH: &str = "user://run.txt.new";
/// Where the run file is moved while it is being rebuilt at launch. If the
/// rebuild brings the game down, the next launch finds the file here rather
/// than at `RUN_PATH`, and throws it away instead of crashing again.
pub const RUN_LOADING_PATH: &str = "user://run-loading.txt";

/// The three paths one kept log moves through: the file itself, the
/// temporary file its atomic replace writes first, and where launch sets it
/// aside while it is rebuilt. The run's are [`RUN_FILES`]; a suspended
/// versus match keeps its own three (`match_log::MATCH_FILES`) under the
/// same rules, so the functions below take the paths they work on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paths {
    pub main: &'static str,
    pub temp: &'static str,
    pub loading: &'static str,
}

/// The run file's paths.
pub const RUN_FILES: Paths = Paths {
    main: RUN_PATH,
    temp: RUN_TEMP_PATH,
    loading: RUN_LOADING_PATH,
};

/// First token of the line. Not `run`, which DESIGN §9.2 gives the finished
/// run history's lines, so neither file can be mistaken for the other.
const TAG: &str = "resume";

/// The in-progress run as it goes to disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// `SIM_EPOCH` of the build that wrote it. Only a sim of the same epoch
    /// may replay the log (DESIGN §9.3).
    pub epoch: u32,
    pub seed: u64,
    /// Buffer width the run was generated at. A resumed run must use it,
    /// whatever the device reports now.
    pub width: usize,
    pub daily: bool,
    pub log: Vec<LoggedShot>,
}

impl Saved {
    pub fn of(run: &RunState, daily: bool) -> Self {
        Self {
            epoch: SIM_EPOCH,
            seed: run.run_seed,
            width: run.width,
            daily,
            log: run.log.clone(),
        }
    }

    /// `resume <epoch> <seed> <width> <daily> <log>`, newline-terminated,
    /// where the log is `replay::encode_log`'s text (it starts with its own
    /// format version, `v2`). The seed is the raw decimal number, which
    /// `format_seed` also writes.
    pub fn to_line(&self) -> String {
        format!(
            "{TAG} {} {} {} {} {}\n",
            self.epoch,
            self.seed,
            self.width,
            u8::from(self.daily),
            encode_log(&self.log)
        )
    }

    /// Parse `to_line`'s format. The epoch is checked before anything after
    /// it is read: another build's line may not even be this format.
    pub fn parse(text: &str) -> Result<Self, Stale> {
        if text.contains('\0') {
            // What a crash leaves where the data was not committed.
            return Err(Stale::Damaged);
        }
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let line = lines.next().ok_or(Stale::Damaged)?;
        if lines.next().is_some() {
            return Err(Stale::Damaged);
        }
        let mut fields = line.trim().splitn(6, ' ');
        if fields.next() != Some(TAG) {
            return Err(Stale::Damaged);
        }
        let epoch: u32 = number(fields.next())?;
        if epoch < SIM_EPOCH {
            return Err(Stale::OlderVersion);
        }
        if epoch > SIM_EPOCH {
            return Err(Stale::NewerVersion);
        }
        let seed: u64 = number(fields.next())?;
        let width: usize = number(fields.next())?;
        if !(SCREEN_W_MIN..=SCREEN_W_MAX).contains(&width) {
            return Err(Stale::Damaged);
        }
        let daily = match fields.next() {
            Some("0") => false,
            Some("1") => true,
            _ => return Err(Stale::Damaged),
        };
        let log = decode_log(fields.next().ok_or(Stale::Damaged)?).map_err(|_| Stale::Damaged)?;
        Ok(Self {
            epoch,
            seed,
            width,
            daily,
            log,
        })
    }

    pub fn replay(&self) -> Replay {
        Replay {
            run_seed: self.seed,
            width: self.width,
            log: self.log.clone(),
        }
    }

    /// The run rebuilt. `None` if the log does not replay: a determinism bug
    /// if the epoch matched, and never something to show diverging.
    pub fn reconstruct(&self) -> Option<RunState> {
        self.replay().reconstruct().ok()
    }

    /// The aim to put back on the dial: the last shot's, which is where the
    /// live game leaves it (the aim carries across screens and drafts).
    /// `None` before the first shot.
    pub fn aim(&self) -> Option<(i32, i32)> {
        self.log
            .iter()
            .rev()
            .find(|e| e.angle_ticks != DRAFT_MARK)
            .map(|e| (e.angle_ticks, e.power_ticks))
    }
}

fn number<T: core::str::FromStr + ToString>(tok: Option<&str>) -> Result<T, Stale> {
    let tok = tok.ok_or(Stale::Damaged)?;
    let v: T = tok.parse().map_err(|_| Stale::Damaged)?;
    // Only the way `Display` writes it, like `decode_log`.
    if v.to_string() == tok {
        Ok(v)
    } else {
        Err(Stale::Damaged)
    }
}

/// Why a saved run is thrown away rather than offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// Written under an earlier `SIM_EPOCH`: this sim would replay it as a
    /// different run.
    OlderVersion,
    /// Written by a newer build (a downgrade).
    NewerVersion,
    /// Not a line of this format: torn, edited, or garbage.
    Damaged,
    /// Parsed, same epoch, but the log does not reconstruct.
    Diverged,
}

impl Stale {
    /// What the title screen says, once, as it discards the file.
    pub fn note(self) -> &'static str {
        match self {
            Stale::OlderVersion => "SAVED RUN FROM AN OLDER VERSION",
            Stale::NewerVersion => "SAVED RUN FROM A NEWER VERSION",
            Stale::Damaged | Stale::Diverged => "SAVED RUN COULD NOT BE RESTORED",
        }
    }
}

/// What to do with `user://run.txt` at launch.
#[derive(Debug)]
pub enum Found {
    /// No file (or an empty one): nothing to say.
    Nothing,
    /// Offer CONTINUE.
    Continue(Saved),
    /// The log's last shot ended the run, but the app died before the score
    /// was recorded: record it and delete the file. Carries the rebuilt run
    /// for its score.
    Finished(Saved, Box<RunState>),
    /// Delete the file and say why.
    Discard(Stale),
}

/// Decide what one line of the run file is worth. `None` is no file.
pub fn judge(text: Option<&str>) -> Found {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Found::Nothing;
    };
    let saved = match Saved::parse(text) {
        Ok(s) => s,
        Err(why) => return Found::Discard(why),
    };
    match saved.reconstruct() {
        None => Found::Discard(Stale::Diverged),
        Some(run) if run.is_over() => Found::Finished(saved, Box::new(run)),
        Some(_) => Found::Continue(saved),
    }
}

// ------------------------------------------------------------ the run files

/// The file operations the run files need. `game.rs` carries them out on
/// `user://` through `save`; `app/disk_tests.rs` on files held in memory,
/// with the process killed between any two. Both go through the functions
/// below, so the rules the crash tests check are the ones the game runs.
pub trait RunFiles {
    /// A file's text; `None` if it is missing or unreadable.
    fn read(&self, path: &str) -> Option<String>;
    /// Whether a file is there, readable or not.
    fn exists(&self, path: &str) -> bool;
    /// Replace `main` with `text` by way of `temp` (`save::write`). Returns
    /// whether it reached disk; on failure `main` is left as it was.
    fn write(&mut self, main: &str, temp: &str, text: &str) -> bool;
    /// Move `from` to `to`, replacing `to`. Returns whether it moved.
    fn rename(&mut self, from: &str, to: &str) -> bool;
    /// Delete a file if it is there.
    fn remove(&mut self, path: &str);
}

/// Whether a run file's text is a whole copy: no NUL bytes (what a crash
/// leaves where data was not committed), and a well-formed `resume` line.
/// A line from another epoch counts: it is whole, only not this build's to
/// replay, and [`judge`] says so with its own note.
pub fn sound(text: &str) -> bool {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).peekable();
    !text.contains('\0')
        && lines.peek().is_some()
        && lines.all(|l| {
            matches!(
                Saved::parse(l),
                Ok(_) | Err(Stale::OlderVersion | Stale::NewerVersion)
            )
        })
}

/// At launch: find the saved run and move it aside to [`RUN_LOADING_PATH`]
/// before the core parses or replays it (see "A corrupt run file costs one
/// crash" above). `None` is no run.
///
/// Of `run.txt` and `run.txt.new` the copy is chosen as the save's is
/// (`durable::pick_by`, with [`sound`]): a whole `run.txt`, else a whole
/// `run.txt.new` (a write that died before its rename, beside a damaged or
/// missing `run.txt`), else whichever is legible, to be judged and
/// discarded with a note. A blank `run.txt` is what a discard that could not
/// delete leaves: no run, unless a whole `run.txt.new` was written since.
pub fn take_saved(files: &mut impl RunFiles) -> Option<SavedRun> {
    take_aside(files, RUN_FILES, sound)
}

/// [`take_saved`] for any kept log: its own `paths`, and its own test of a
/// whole copy (`sound`).
pub fn take_aside(
    files: &mut impl RunFiles,
    paths: Paths,
    sound: impl Fn(&str) -> bool,
) -> Option<SavedRun> {
    if files.read(paths.loading).is_some() {
        return Some(SavedRun::Interrupted);
    }
    let main = files.read(paths.main);
    let temp = files.read(paths.temp);
    let (path, text) = match durable::pick_by(main.as_deref(), temp.as_deref(), sound) {
        Pick::Main => (paths.main, main?),
        Pick::Temp => (paths.temp, temp?),
        Pick::Empty => return None,
    };
    files.rename(path, paths.loading);
    Some(SavedRun::Found(text))
}

/// Make the saved run exactly `text`, or no run at all for `""`: `run.txt`
/// is written with `text` (or deleted, or failing that blanked), and only
/// once that has worked do the set-aside copy and the temporary file go. A
/// write that fails (a full disk) therefore leaves every copy where it was,
/// never the only one deleted. If something is still left that a launch
/// could read as a run other than `text` (a file that would not delete),
/// `run.txt` is written once more with `text`, which for `""` is the empty
/// file [`judge`] reads as no run.
///
/// Returns whether that is confirmed: `text` is in place, and neither
/// `run.txt` nor `run.txt.new` holds anything but `text` or nothing; a file
/// that is there but cannot be read counts as holding something else. A
/// score may only go on the board once its finished log is confirmed gone,
/// or a later launch scores it again (`app`'s "Scoring at most once"). A
/// set-aside copy that will not delete does not count against it: a launch
/// that finds one throws it away unread.
pub fn replace(files: &mut impl RunFiles, text: &str) -> bool {
    replace_at(files, RUN_FILES, text)
}

/// [`replace`] for any kept log, at its own `paths`.
pub fn replace_at(files: &mut impl RunFiles, paths: Paths, text: &str) -> bool {
    let placed = if text.is_empty() {
        files.remove(paths.main);
        !files.exists(paths.main) || files.write(paths.main, paths.temp, "")
    } else {
        files.write(paths.main, paths.temp, text)
    };
    if !placed {
        return false;
    }
    files.remove(paths.loading);
    files.remove(paths.temp);
    if holds_only(files, paths, text) {
        return true;
    }
    files.write(paths.main, paths.temp, text) && holds_only(files, paths, text)
}

/// Neither the main file nor its temporary file holds a log other than
/// `text`. A file there that cannot be read may hold anything.
fn holds_only(files: &impl RunFiles, paths: Paths, text: &str) -> bool {
    [paths.main, paths.temp]
        .into_iter()
        .all(|path| match files.read(path) {
            Some(t) => t.trim().is_empty() || t == text,
            None => !files.exists(path),
        })
}

/// At launch, over a copy left set aside by a launch that died rebuilding
/// it: throw that copy away, unread, and the temporary file with it. A
/// `run.txt` beside it was written after the copy was set aside (the live
/// run put back), so it is kept for the next launch.
///
/// The temporary file goes because it can only be a write that died
/// before its rename, and with the main file gone (moved aside, then
/// dropped here) the next launch would pick it ([`take_saved`]): most
/// likely the same log that just brought the rebuild down, costing another
/// crashed launch. A whole `run.txt` beside it would have won anyway.
pub fn drop_set_aside(files: &mut impl RunFiles) {
    drop_set_aside_at(files, RUN_FILES);
}

/// [`drop_set_aside`] for any kept log, at its own `paths`.
pub fn drop_set_aside_at(files: &mut impl RunFiles, paths: Paths) {
    files.remove(paths.loading);
    files.remove(paths.temp);
}

/// Leave no run to resume or score: [`replace`] with nothing.
pub fn discard(files: &mut impl RunFiles) -> bool {
    replace(files, "")
}

/// Seconds of wall clock any one frame may advance animation and UI timers
/// by. After minutes in the background the engine hands over the whole gap as
/// one delta; uncapped, a shot would finish, or a clear be skipped, in a
/// single frame the player never saw. Presentation only: the sim resolved
/// every outcome when the shot was fired.
pub const MAX_FRAME_DELTA: f64 = 0.1;

/// A frame's delta as the animation and timers should see it.
pub fn frame_delta(delta: f64) -> f64 {
    if delta.is_finite() {
        delta.clamp(0.0, MAX_FRAME_DELTA)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use salt_line_sim::ballistics::Shot;
    use salt_line_sim::run::Phase;

    /// A short run played the way the game plays it: the solver's shot at
    /// the first standing target, drafts taking offer 0.
    fn played(seed: u64, width: usize, entries: usize) -> RunState {
        let mut run = RunState::new(seed, width);
        while run.log.len() < entries && !run.is_over() {
            match run.phase {
                Phase::Drafting => {
                    run.take_offer(0);
                }
                Phase::Cleared => {
                    run.resolve_clear();
                }
                Phase::Firing => {
                    let target = run
                        .screen
                        .structures
                        .iter()
                        .position(|s| !s.is_down())
                        .unwrap();
                    let shot =
                        salt_line_sim::solver::solve_target(&run.screen, &run.loadout, target)
                            .first()
                            .map(|c| c.shot)
                            .unwrap_or(Shot::new(900, 600));
                    if let Some(f) = run.screen.default_fuse(target) {
                        run.set_fuse(f);
                    }
                    run.fire(shot, false);
                }
                Phase::Over(_) => unreachable!(),
            }
        }
        run
    }

    #[test]
    fn the_line_format_is_pinned() {
        let run = played(20260907, 320, 3);
        insta::assert_snapshot!(Saved::of(&run, true).to_line());
    }

    #[test]
    fn a_saved_run_round_trips_and_rebuilds_the_same_state() {
        for (seed, width, n) in [(1, 320, 0), (20260907, 360, 7), (42, 400, 20)] {
            let run = played(seed, width, n);
            let saved = Saved::of(&run, false);
            let back = Saved::parse(&saved.to_line()).unwrap();
            assert_eq!(back, saved);
            let mut live = run.clone();
            // The live game resolves a clear after its hold; the rebuild has
            // no hold to wait for.
            live.resolve_clear();
            let rebuilt = back.reconstruct().unwrap();
            assert_eq!(
                salt_line_sim::replay::state_hash(&rebuilt),
                salt_line_sim::replay::state_hash(&live)
            );
            assert_eq!(rebuilt.fuse_tenths(), live.fuse_tenths());
        }
    }

    #[test]
    fn a_live_run_is_offered_to_continue() {
        let run = played(7, 320, 5);
        let line = Saved::of(&run, true).to_line();
        match judge(Some(&line)) {
            Found::Continue(s) => {
                assert_eq!((s.seed, s.width, s.daily), (7, 320, true));
                assert_eq!(s.log.len(), 5);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_run_that_already_ended_is_scored_not_continued() {
        let mut run = RunState::new(3, 320);
        // Throw every shell away.
        while !run.is_over() {
            if run.fire(Shot::new(1400, 0), false).is_none() {
                // A lucky lob cleared the screen.
                run.resolve_clear();
                run.take_offer(0);
            }
        }
        let line = Saved::of(&run, false).to_line();
        match judge(Some(&line)) {
            Found::Finished(s, rebuilt) => {
                assert_eq!(s.log.len(), run.log.len());
                assert_eq!(rebuilt.score(), run.score());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn another_epoch_is_never_replayed() {
        let run = played(7, 320, 2);
        let mut saved = Saved::of(&run, false);
        saved.epoch = SIM_EPOCH - 1;
        assert!(matches!(
            judge(Some(&saved.to_line())),
            Found::Discard(Stale::OlderVersion)
        ));
        saved.epoch = SIM_EPOCH + 1;
        assert!(matches!(
            judge(Some(&saved.to_line())),
            Found::Discard(Stale::NewerVersion)
        ));
        // The epoch decides before the rest is read: an old build's line may
        // be in a format this one cannot parse.
        let old = format!("resume {} 7 320 0 @0 900:600\n", SIM_EPOCH - 1);
        assert_eq!(Saved::parse(&old), Err(Stale::OlderVersion));
        assert_eq!(
            Stale::OlderVersion.note(),
            "SAVED RUN FROM AN OLDER VERSION"
        );
    }

    #[test]
    fn a_log_that_does_not_replay_is_discarded() {
        // Screen 3 before screen 0 has been cleared.
        let line = format!("resume {SIM_EPOCH} 7 320 0 v2 @3 F0 900:600\n");
        assert!(Saved::parse(&line).is_ok());
        assert!(matches!(
            judge(Some(&line)),
            Found::Discard(Stale::Diverged)
        ));
    }

    #[rstest::rstest]
    #[case::wrong_tag("run 2 7 320 0 v2")]
    #[case::torn("resume 2 7 32")]
    #[case::nul("resume 2 7 320 0 v2\0\0")]
    #[case::two_lines("resume 2 7 320 0 v2\nresume 2 7 320 0 v2\n")]
    #[case::narrow("resume 2 7 100 0 v2")]
    #[case::wide("resume 2 7 4000 0 v2")]
    #[case::daily_not_a_flag("resume 2 7 320 yes v2")]
    #[case::seed_padded("resume 2 07 320 0 v2")]
    #[case::no_log("resume 2 7 320 0")]
    #[case::bad_log("resume 2 7 320 0 v2 @0 banana")]
    #[case::old_log_format("resume 2 7 320 0 @0 900:600")]
    fn damaged_lines_are_discarded(#[case] text: &str) {
        // The cases are written against epoch 2; keep them honest if it moves.
        // Every line is rewritten, so the two-line case is damaged for being
        // two lines, not for a stale epoch on its second.
        let text = text.replace("resume 2 ", &format!("resume {SIM_EPOCH} "));
        assert!(matches!(judge(Some(&text)), Found::Discard(Stale::Damaged)));
    }

    /// The run files in memory, for `take_saved` and `replace`. Files named
    /// in `stuck` can be neither replaced nor deleted; those in `unreadable`
    /// are there but read as `None`, as `save::read` reports any error.
    #[derive(Default)]
    struct Mem(
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeSet<&'static str>,
        std::collections::BTreeSet<&'static str>,
    );

    impl RunFiles for Mem {
        fn read(&self, path: &str) -> Option<String> {
            if self.2.contains(path) {
                return None;
            }
            self.0.get(path).cloned()
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn write(&mut self, main: &str, _temp: &str, text: &str) -> bool {
            if self.1.contains(main) {
                return false;
            }
            self.0.insert(main.into(), text.into());
            true
        }
        fn rename(&mut self, from: &str, to: &str) -> bool {
            if self.1.contains(from) || self.1.contains(to) {
                return false;
            }
            let Some(text) = self.0.remove(from) else {
                return false;
            };
            self.0.insert(to.into(), text);
            true
        }
        fn remove(&mut self, path: &str) {
            if !self.1.contains(path) {
                self.0.remove(path);
            }
        }
    }

    #[test]
    fn a_run_file_that_is_there_but_unreadable_is_not_cleared() {
        let finished = Saved::of(&played(7, 320, 2), false).to_line();
        let mut m = files(Some(&finished), None);
        m.1.insert(RUN_PATH);
        m.2.insert(RUN_PATH);
        assert!(
            !discard(&mut m),
            "a file that could not be read is not gone"
        );
        assert!(!replace(&mut m, "resume x\n"));
        // Readable again, and able to go: cleared.
        m.1.clear();
        m.2.clear();
        assert!(discard(&mut m));
    }

    fn files(main: Option<&str>, temp: Option<&str>) -> Mem {
        let mut m = Mem::default();
        for (path, text) in [(RUN_PATH, main), (RUN_TEMP_PATH, temp)] {
            if let Some(t) = text {
                m.0.insert(path.into(), t.into());
            }
        }
        m
    }

    fn found(files: &mut Mem) -> Option<String> {
        match take_saved(files) {
            Some(crate::app::SavedRun::Found(text)) => Some(text),
            Some(crate::app::SavedRun::Interrupted) => Some("interrupted".into()),
            None => None,
        }
    }

    #[test]
    fn launch_prefers_a_sound_copy_as_the_save_does() {
        let good = Saved::of(&played(7, 320, 2), false).to_line();
        let newer = Saved::of(&played(7, 320, 3), false).to_line();
        // Cut inside the header, so it cannot pass for a shorter log.
        let torn = &good[..10];
        let zeros = "\0".repeat(good.len());
        // A damaged run.txt loses to a whole run.txt.new, which is moved
        // aside in its place.
        for damaged in [torn, zeros.as_str()] {
            let mut m = files(Some(damaged), Some(&good));
            assert_eq!(found(&mut m).as_deref(), Some(good.as_str()));
            assert_eq!(m.read(RUN_LOADING_PATH).as_deref(), Some(good.as_str()));
            assert_eq!(m.read(RUN_TEMP_PATH), None);
        }
        // A whole run.txt beats a newer run.txt.new: that write never landed.
        let mut m = files(Some(&good), Some(&newer));
        assert_eq!(found(&mut m).as_deref(), Some(good.as_str()));
        // A damaged copy alone is still handed over, to be judged and
        // discarded with a note.
        let mut m = files(Some(torn), None);
        assert_eq!(found(&mut m).as_deref(), Some(torn));
        // Blank is no run; so is nothing.
        assert_eq!(found(&mut files(Some(""), None)), None);
        assert_eq!(found(&mut files(None, None)), None);
        // A run from another epoch is a whole copy, to be discarded with
        // its own note rather than passed over for an older temp.
        let mut other = Saved::of(&played(7, 320, 2), false);
        other.epoch = SIM_EPOCH + 1;
        let other = other.to_line();
        let mut m = files(Some(&other), Some(&good));
        assert_eq!(found(&mut m).as_deref(), Some(other.as_str()));
        // Left set aside by a launch that died: never replayed.
        let mut m = files(Some(&good), None);
        m.0.insert(RUN_LOADING_PATH.into(), good.clone());
        assert_eq!(found(&mut m).as_deref(), Some("interrupted"));
    }

    #[test]
    fn no_file_is_nothing_to_say() {
        assert!(matches!(judge(None), Found::Nothing));
        assert!(matches!(judge(Some(" \n")), Found::Nothing));
    }

    #[test]
    fn the_aim_comes_back_from_the_last_shot_not_a_draft() {
        let log = decode_log("v2 @0 F10 900:600 870:640 @4 D1").unwrap();
        let saved = Saved {
            epoch: SIM_EPOCH,
            seed: 1,
            width: 320,
            daily: false,
            log,
        };
        assert_eq!(saved.aim(), Some((870, 640)));
        assert_eq!(
            Saved {
                log: vec![],
                ..saved
            }
            .aim(),
            None
        );
    }

    #[rstest::rstest]
    #[case::normal(1.0 / 60.0, 1.0 / 60.0)]
    #[case::background_minutes(300.0, MAX_FRAME_DELTA)]
    #[case::negative(-1.0, 0.0)]
    #[case::nan(f64::NAN, 0.0)]
    #[case::inf(f64::INFINITY, 0.0)]
    fn the_frame_delta_is_capped(#[case] delta: f64, #[case] want: f64) {
        assert_eq!(frame_delta(delta), want);
    }
}

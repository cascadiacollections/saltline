//! Resuming a run the operating system killed. DESIGN §9.6.
//!
//! Phones kill backgrounded apps, and a run is only a seed and a shot log
//! (DESIGN §3), so the in-progress run is kept on disk as exactly that:
//! `user://run.txt`, one line, rewritten (atomically, `durable::write_atomic`)
//! every time the log grows. On launch the line is rebuilt with
//! `Replay::reconstruct` and the title screen offers CONTINUE.
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

use salt_line_sim::replay::{decode_log, encode_log, Replay, DRAFT_MARK};
use salt_line_sim::run::{LoggedShot, RunState};
use salt_line_sim::{SCREEN_W_MAX, SCREEN_W_MIN, SIM_EPOCH};

pub const RUN_PATH: &str = "user://run.txt";
/// Written in full, then moved over `RUN_PATH`.
pub const RUN_TEMP_PATH: &str = "user://run.txt.new";

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

/// Decide what a saved run's text is worth. `None` is no file.
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
        let text = text.replacen("resume 2 ", &format!("resume {SIM_EPOCH} "), 1);
        assert!(matches!(judge(Some(&text)), Found::Discard(Stale::Damaged)));
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

//! A run in play: starting one, firing, replaying the shot, clearing the
//! screen and scoring the end. The rules are the sim's; this is the pacing.
//!
//! # Resolve first, show afterwards
//!
//! `fire` hands the shot to the sim, which resolves it completely at once:
//! craters dug, structures toppled, the screen cleared or the run over.
//! What follows is presentation. [`Anim`] keeps the recorded paths and a
//! copy of the screen from before the shot, `step` walks the paths at the
//! frame's pace, and the renderer shows the old screen until the shell lands.
//! Frame rate, pausing, or an app killed mid-flight cannot change a result,
//! and the run's shot log is complete the moment the trigger is pulled: the
//! log is the run (event sourcing), so `persist_run` can write it on the
//! same frame and a resumed run replays to the same state (DESIGN §9.6).
//!
//! # Timeline of one shot
//!
//! ```text
//!  fire ─► anim.t runs to `longest` ─► shells down: impact and collapse sounds
//!        ─► `settled` passes TOPPLE_TIME + 0.12 s ─► anim ends
//!        ─► cleared?  clear_hold = CLEAR_HOLD, camera pans, then next screen
//!           over?     finish_run: run file discarded, score stored, GameOver
//! ```

use super::*;

impl Game {
    /// A new run on `seed`, generated for the device's width, with the aim
    /// reset and the run file taken over (`begin_persisting`): the next
    /// frame writes it, replacing any saved run.
    pub(crate) fn start_run(&mut self, seed: u64, daily: bool) {
        // A new run is generated for the device, whatever a resumed one used.
        self.set_run_width(None);
        // The world is as wide as the buffer, so the run is generated for it.
        let width = self.display.buffer_w as usize;
        let state = RunState::new(seed, width);
        self.play.angle_ticks = 900;
        self.play.power_ticks = 600;
        self.play.anim = None;
        self.play.last_arc.clear();
        self.new_preview();
        self.play.clear_hold = 0.0;
        self.play.sky_time = 0.0;
        self.play.cam_x = 0;
        self.play.cam_target = 0.0;
        self.play.daily = daily;
        self.play.low_shell_warned = false;
        self.play.last_seed = seed;
        self.retire_last_crash();
        self.play.run = Some(state);
        self.mode = Mode::Playing;
        self.apply_buffer_size();
        self.begin_persisting();
        self.note(format!("SEED {}", format_seed(seed)));
    }

    /// Start the bundled one-gun course without touching the saved run.
    pub(crate) fn start_challenge(&mut self, index: usize) {
        let Some(challenge) = salt_line_sim::challenge::get(index) else {
            self.note("NO MORE CHALLENGES".into());
            return;
        };
        let layout = salt_line_sim::challenge::layout(challenge);
        self.set_run_width(Some(layout.width()));
        let run = RunState::from_challenge(&challenge, &layout);
        self.play.challenge = Some(index);
        self.play.angle_ticks = 900;
        self.play.power_ticks = 600;
        self.play.anim = None;
        self.play.last_arc.clear();
        self.play.clear_hold = 0.0;
        self.play.sky_time = 0.0;
        self.play.cam_x = 0;
        self.play.cam_target = 0.0;
        self.play.daily = false;
        self.play.low_shell_warned = false;
        self.play.last_seed = run.run_seed;
        self.play.run = Some(run);
        self.run_file.run_persist = false;
        self.run_file.resumable = None;
        self.retire_last_crash();
        self.new_preview();
        self.mode = Mode::Playing;
        self.apply_buffer_size();
        let hint_count = salt_line_sim::challenge::hints(challenge).len();
        self.note(format!(
            "{}  {} SHOTS  HINTS {}",
            challenge.name, challenge.shots, hint_count
        ));
    }

    /// Level inspector: drop straight onto a screen index without playing the
    /// ones before it. DESIGN §2.2 is what makes this possible — each screen
    /// generates from its own seed, so screen 400 costs the same as screen 1.
    pub(crate) fn jump_to(&mut self, index: u32) {
        let Some(run) = self.play.run.as_mut() else {
            return;
        };
        run.screen_index = index;
        // The log cannot rebuild a jump, so the run is not kept for resume.
        self.run_file.run_persist = false;
        run.screen = run::generate_screen(run.run_seed, index, run.width, &run.loadout);
        self.new_preview();
        self.note(format!("SCREEN {}", index + 1));
    }

    /// Fire the current aim, if the sim takes it (the firing phase, a shell
    /// left). The sim resolves the shot and logs it now; this starts its
    /// replay.
    pub(super) fn fire(&mut self) {
        // Onboarding advances one lesson per shot: wind is learned by firing
        // with wind showing once craters are, shells by firing while the
        // shells prompt shows (after wind, with the reserve below its start;
        // see `onboarding_prompt`).
        let learned = self.save.learned;
        if learned & LEARN_WIND != 0
            && self
                .play
                .run
                .as_ref()
                .is_some_and(|r| r.shells < salt_line_sim::START_SHELLS)
        {
            self.learn(LEARN_SHELLS);
        } else if learned & LEARN_CRATER != 0
            && self
                .play
                .run
                .as_ref()
                .and_then(|r| r.displayed_wind())
                .is_some_and(|w| w.abs() > 0.01)
        {
            self.learn(LEARN_WIND);
        }
        let shot = Shot::new(self.play.angle_ticks, self.play.power_ticks);
        let Some(run) = self.play.run.as_mut() else {
            return;
        };
        if run.shells <= 0 {
            return;
        }
        // Kept for the replay: the renderer draws this until the shell lands.
        let before = run.screen.clone();
        let Some(report) = run.fire(shot, true) else {
            return;
        };
        self.learn(LEARN_FIRE);
        self.play(Sound::Fire);
        self.play(Sound::Flight);
        let longest = report
            .traces
            .iter()
            .map(|t| t.flight_time)
            .fold(0.0f64, f64::max);
        let collapsed = report.collapsed.clone();
        self.play.last_arc = report
            .traces
            .first()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.play.anim = Some(Anim {
            traces: report.traces,
            before,
            t: 0.0,
            longest,
            collapsed,
            settled: 0.0,
            impact_played: false,
            collapse_played: false,
        });
        let shells = self.play.run.as_ref().map(|r| r.shells).unwrap_or(99);
        if shells < 10 && !self.play.low_shell_warned {
            self.play.low_shell_warned = true;
            self.play(Sound::LowShell);
            self.note("SHELLS ARE YOUR RUN CLOCK".into());
        }
    }

    /// Advance the presentation by `delta` seconds (already capped): the
    /// shot replay, the clear hold and its pan, the end of the run, and the
    /// camera.
    pub(super) fn step(&mut self, delta: f64) {
        if self.mode != Mode::Playing {
            return;
        }

        // Star changes need a redraw only five times a second. Daylight
        // remains fully static, preserving the idle texture-upload saving.
        if let Some(run) = self.play.run.as_ref() {
            let old_step = (self.play.sky_time * 5.0).floor() as u64;
            self.play.sky_time += delta;
            if crate::sky::Phase::for_screen(run.screen_index).has_stars()
                && (self.play.sky_time * 5.0).floor() as u64 != old_step
            {
                self.fx.push(Effect::Redraw);
            }
        }

        // Advance the shot replay. The sim ran at a fixed 1/120; this only
        // walks the recorded path, so playback speed cannot affect anything.
        let mut sounds = Vec::new();
        let mut learned_impact = false;
        if let Some(anim) = self.play.anim.as_mut() {
            anim.t += delta;
            if anim.done() {
                anim.settled += delta;
                if !anim.impact_played {
                    anim.impact_played = true;
                    learned_impact = true;
                    if let Some(sound) = anim.traces.first().map(|trace| {
                        if trace.bounces > 0 {
                            Sound::Ricochet
                        } else {
                            match trace.end {
                                End::Fuse { .. } => Sound::Airburst,
                                End::Structure { .. } => Sound::Structure,
                                _ => Sound::Terrain,
                            }
                        }
                    }) {
                        sounds.push(sound);
                    }
                }
                if !anim.collapse_played && !anim.collapsed.is_empty() {
                    anim.collapse_played = true;
                    sounds.push(Sound::Collapse);
                }
            }
            // A beat after the topple, so the rubble is seen before the
            // screen moves on.
            let settled = anim.settled;
            let finished = anim.done() && settled > TOPPLE_TIME + 0.12;
            if finished {
                self.play.anim = None;
                let cleared = self.play.run.as_ref().map(|r| r.phase) == Some(Phase::Cleared);
                if cleared {
                    self.play.clear_hold = CLEAR_HOLD;
                } else if self.play.run.as_ref().map(|r| r.is_over()) == Some(true) {
                    self.finish_run();
                }
            }
        } else if self.play.clear_hold > 0.0 {
            // Hold on the cleared screen and pan forward, then move on.
            self.play.clear_hold -= delta;
            let span = (self.world_width() - self.display.buffer_w).max(0) as f64;
            let t = 1.0 - (self.play.clear_hold / CLEAR_HOLD).clamp(0.0, 1.0);
            self.play.cam_target = span * t;
            if self.play.clear_hold <= 0.0 {
                self.play.clear_hold = 0.0;
                self.play.cam_target = 0.0;
                self.play.cam_x = 0;
                if let Some(index) = self.play.challenge {
                    if let Some(next) = salt_line_sim::challenge::get(index + 1) {
                        let layout = salt_line_sim::challenge::layout(next);
                        self.play.run = Some(RunState::from_challenge(&next, &layout));
                        self.play.challenge = Some(index + 1);
                        self.play.angle_ticks = 900;
                        self.play.power_ticks = 600;
                        self.play.last_arc.clear();
                        self.play.last_seed =
                            self.play.run.as_ref().map(|r| r.run_seed).unwrap_or(0);
                        self.new_preview();
                        self.note(format!("{}  NEXT CHALLENGE", next.name));
                    } else {
                        self.play.run.as_mut().map(|r| r.resolve_clear());
                        self.finish_run();
                    }
                } else {
                    let advanced = self.play.run.as_mut().map(|r| r.resolve_clear()) == Some(true);
                    if advanced {
                        self.after_screen_change();
                    }
                }
            }
        } else if let Some(run) = self.play.run.as_ref() {
            // Over with nothing left to show: score it now.
            if run.is_over() {
                self.finish_run();
            }
        }
        if learned_impact {
            self.learn(LEARN_CRATER);
        }
        for sound in sounds {
            self.play(sound);
        }

        // Follow the shell when the world is wider than the buffer.
        if self.play.anim.is_some() {
            let head = self.shell_head();
            if let Some(p) = head {
                let span = (self.world_width() - self.display.buffer_w).max(0) as f64;
                if span > 0.0 {
                    self.play.cam_target =
                        (p.x - self.display.buffer_w as f64 * 0.5).clamp(0.0, span);
                }
            }
        }

        // The camera is quantized to a whole pixel every frame. Panning with a
        // fractional camera is what makes low-res terrain shimmer.
        let span = (self.world_width() - self.display.buffer_w).max(0) as f64;
        self.play.cam_x = self.play.cam_target.round().clamp(0.0, span) as i32;
    }

    /// The run moved to a new screen or a draft: the loadout may have changed
    /// the buffer (Long Lens), and a draft announces itself.
    pub(super) fn after_screen_change(&mut self) {
        self.apply_buffer_size();
        if self.play.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            self.play(Sound::Draft);
        }
    }

    /// The run is over: score it, keep it to watch again, and show the
    /// result.
    pub(crate) fn finish_run(&mut self) {
        // A demo run that ends is not scored, and its result screen is not
        // shown: the demo moves on to its next leg (`demo_step`).
        if self.spectating() {
            return;
        }
        if self.play.challenge.is_some() {
            self.run_file.run_persist = false;
            self.mode = Mode::GameOver;
            self.note("CHALLENGE COURSE COMPLETE".into());
            return;
        }
        // The run file goes before the score is recorded, never after, and
        // the score only if it is confirmed gone (`Store::Score`'s `clear`):
        // a crash in between costs this score, where the other order would
        // record it twice (the next launch scores a finished log).
        let persisted = self.stop_persisting();
        let Some(run) = self.play.run.as_ref() else {
            if persisted {
                self.fx.push(Effect::RunFile(RunFile::Discard));
            }
            self.mode = Mode::GameOver;
            return;
        };
        let (screens, shells, shots) = run.score();
        let entry = Entry {
            seed: run.run_seed,
            screens,
            shells,
            shots,
            daily: self.play.daily,
        };
        self.history.last_run = None;
        if persisted {
            // Scored with any earlier runs still waiting in the run file.
            let line = resume::Saved::of(run, self.play.daily).to_line();
            self.run_file.unscored.push(Unscored { entry, line });
            self.score_unscored(false);
            // Kept to watch again (DESIGN §9), after the score: a crash in
            // between costs the replay, never a score.
            self.record_run();
        } else {
            // Never in the run file (a `--salt-screen` jump): nothing to
            // clear, and nothing kept if the board cannot be written.
            self.fx.push(Effect::Store(Store::Score {
                entries: vec![entry],
                clear: None,
                announce: false,
            }));
        }
        self.mode = Mode::GameOver;
    }

    /// Ask for every run in `RunFileState::unscored` to go on the board,
    /// once the run file holds only the run kept for CONTINUE, if any. The
    /// entries are only placed in memory once they are on disk (`stored`):
    /// placed first, a later settings write would carry them to disk while
    /// their kept lines scored them again at the next launch.
    ///
    /// Over a save from a newer build, which this one never writes
    /// (`save::store`), there is no attempt: the run file is not cleared
    /// only to be written back, and the runs simply stay in it.
    pub(super) fn score_unscored(&mut self, announce: bool) {
        if self.save.from_the_future {
            self.keep_unscored();
            return;
        }
        self.fx.push(Effect::Store(Store::Score {
            entries: self.run_file.unscored.iter().map(|u| u.entry).collect(),
            clear: Some(self.kept_line().unwrap_or_default()),
            announce,
        }));
    }

    /// The waiting runs did not reach the board (a save from a newer build,
    /// a full disk, a finished log that would not go): keep their lines in
    /// the run file, so a later score or launch can get them there, and say
    /// so.
    pub(super) fn keep_unscored(&mut self) {
        if !self.run_file.unscored.is_empty() {
            self.fx
                .push(Effect::RunFile(RunFile::Write(self.run_file_text())));
        }
        self.note_not_saved("SCORE NOT SAVED");
    }

    /// A shot the solver says brings down the first standing target, and
    /// that target's index: the autopilot's aim, and the demo's.
    pub(super) fn solver_shot(&self) -> Option<(Shot, usize)> {
        let run = self.play.run.as_ref()?;
        let target = run.screen.structures.iter().position(|s| !s.is_down())?;
        salt_line_sim::solver::solve_target(&run.screen, &run.loadout, target)
            .first()
            .map(|c| (c.shot, target))
    }

    /// Play the game for the camera: aim with the solver, take offer 1.
    pub(super) fn autopilot(&mut self, delta: f64) {
        self.dev.auto_cooldown -= delta;
        if self.mode == Mode::Title {
            let seed = self.dev.auto_seed.unwrap_or_else(|| daily_seed(self.today));
            self.start_run(seed, false);
            return;
        }
        if self.mode != Mode::Playing || self.play.anim.is_some() || self.play.clear_hold > 0.0 {
            return;
        }
        if self.dev.auto_cooldown > 0.0 {
            return;
        }
        let phase = self.play.run.as_ref().map(|r| r.phase);
        match phase {
            Some(Phase::Drafting) => {
                if self.dev.auto_hold_draft {
                    return;
                }
                self.play.run.as_mut().map(|r| r.take_offer(0));
                self.apply_buffer_size();
                self.dev.auto_cooldown = 0.4;
            }
            Some(Phase::Cleared) => {
                self.play.clear_hold = CLEAR_HOLD;
            }
            Some(Phase::Firing) => {
                let first = self.play.run.as_ref().map(|r| r.shots_on_screen) == Some(0);
                if self.dev.auto_miss && first {
                    // A high lob that lands short, well clear of any target.
                    self.play.angle_ticks = 1400;
                    self.play.power_ticks = 330;
                    self.fire();
                    self.dev.auto_cooldown = 0.35;
                    return;
                }
                if let Some((shot, target)) = self.solver_shot() {
                    self.play.angle_ticks = shot.angle_ticks;
                    self.play.power_ticks = shot.power_ticks;
                    if let Some(run) = self.play.run.as_mut() {
                        if let Some(f) = run.screen.default_fuse(target) {
                            run.set_fuse(f);
                        }
                    }
                    self.fire();
                    self.dev.auto_cooldown = 0.35;
                }
            }
            _ => {}
        }
    }
}

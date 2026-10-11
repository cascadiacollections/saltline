//! Upkeep around the game: the buffer and window, sounds, settings, crash
//! reports, pausing on focus loss, and keeping the run on disk (DESIGN §9.6).
//!
//! Almost every helper here does one thing: push an [`Effect`]. None of them
//! touches a file, a speaker or the window; that is the adapter's job, done
//! later, in order (see the effect catalogue in `app`).
//!
//! # The run file
//!
//! The run in play is kept on disk as its seed and shot log, so a run the
//! operating system killed can be continued (DESIGN §9.6). Its life:
//!
//! ```text
//!  start_run ─► begin_persisting ─► persist_run writes on every log change
//!                                     │
//!        quit to title: suspend_run ──┤ kept; CONTINUE offers it
//!        new run:  confirm_discard ───┤ two presses, then replaced
//!        run over: finish_run ────────┘ stop_persisting; Score clears it first
//! ```

use super::*;

impl Game {
    /// Show `text` on the status line for `STATUS_TIME` seconds. Not in
    /// the attract demo, whose only words are its banner.
    pub(super) fn note(&mut self, text: String) {
        if self.spectating() {
            return;
        }
        self.status = text;
        self.status_time = STATUS_TIME;
        self.fx.push(Effect::Redraw);
    }

    /// Ask for `sound`.
    pub(super) fn play(&mut self, sound: Sound) {
        self.fx.push(Effect::Play(sound));
    }

    /// Ask for the players' volume to follow the save's.
    pub(super) fn apply_volume(&mut self) {
        self.fx.push(Effect::Volume(self.volume_db()));
    }

    /// The drag preview must be worked out afresh.
    pub(super) fn new_preview(&mut self) {
        self.play.preview_epoch = self.play.preview_epoch.wrapping_add(1);
    }

    /// The player has done the thing onboarding bit `bit` teaches. Written
    /// at once, and only the first time, so a lesson learned is never shown
    /// again even if the app is killed straight after.
    /// The attract demo learns nothing: its shots are not the player's.
    pub(super) fn learn(&mut self, bit: u32) {
        if self.spectating() {
            return;
        }
        if self.save.learned & bit == 0 {
            self.save.learned |= bit;
            self.fx.push(Effect::Store(Store::Learned));
        }
    }

    /// Size the buffer for the display and the loadout, and tell the adapter.
    /// Always asks, even when nothing changed; `Canvas::resize` does nothing
    /// at an unchanged size.
    pub(super) fn apply_buffer_size(&mut self) {
        let h = if self.long_lens() {
            LONG_LENS_H
        } else {
            SCREEN_H as i32
        };
        self.display.buffer_h = h;
        self.display.letterbox = self.letterbox_now();
        self.fx.push(Effect::Buffer {
            w: self.display.buffer_w,
            h,
            letterbox: self.display.letterbox,
        });
        self.fx.push(Effect::Redraw);
    }

    /// Follow the display: the window size every frame, and the cutout as
    /// the adapter last read it (a platform round trip on a phone, so not
    /// every frame; see `frame::DisplayPoll`).
    pub(super) fn poll_display(&mut self, window: (i32, i32), cutout: (i32, i32)) {
        self.display.window_size = window;
        self.display.cutout = cutout;
        let want_w = self.pick_width();
        // A resumed match, or a match on a layout, keeps its width as a
        // resumed run does.
        let in_run = matches!(self.mode, Mode::Playing | Mode::Paused | Mode::Editor)
            || (self.mode == Mode::Versus && self.versus.live.is_some());
        if !in_run && self.display.kept_width {
            // A resumed run's width is done with: back to the device's.
            self.set_run_width(None);
            self.apply_buffer_size();
        } else if self.play.run.is_none()
            && self.versus.live.is_none()
            && self.mode != Mode::Editor
            && want_w != self.display.buffer_w
        {
            // Not under a versus match either: its screen was generated
            // for the width it has.
            self.display.buffer_w = want_w;
            self.apply_buffer_size();
        } else if self.letterbox_now() != self.display.letterbox {
            self.apply_buffer_size();
        }
    }

    /// Size the buffer for a run: the recorded width for a resumed one, the
    /// device's for anything else. A resumed run generated at another width
    /// keeps it (DESIGN §9.2: a replay must use it), shown letterboxed
    /// instead of stretched across the window.
    pub(super) fn set_run_width(&mut self, width: Option<usize>) {
        let device = self.pick_width();
        let want = width.map_or(device, |w| w as i32);
        self.display.buffer_w = want;
        self.display.kept_width = want != device;
        // A 320..400px width cap and Long Lens can make the buffer narrower
        // than the window's aspect. Keep the full image centered rather than
        // expanding the logical viewport past the frame we draw.
        self.fx.push(Effect::Aspect(Aspect::Keep));
    }

    /// Write pending settings once they settle, or as soon as the player
    /// leaves Controls. Not every volume repeat: see `durable::Debounce`.
    pub(super) fn tick_settings(&mut self, delta: f64) {
        // Settings are flushed as the demo starts, and none can change
        // during it; anything pending would wait for it to end.
        if self.spectating() {
            return;
        }
        if self.settings.tick(delta, self.mode == Mode::Controls) {
            self.write_settings();
        }
    }

    /// Write pending settings now, if there are any: focus loss, pause, close.
    pub(super) fn flush_settings(&mut self) {
        if self.settings.take() {
            self.write_settings();
        }
    }

    /// [`Self::flush_settings`] for the app being paused or closed, when it
    /// may never run again. A watch keeps a mute pending (nothing is written
    /// while one plays), but the save holds only the player's settings, which
    /// a watch changes only by that mute, so here it is written: a phone may
    /// kill the paused app before the watch ends. The demo still keeps its
    /// change for after it.
    pub(super) fn flush_settings_final(&mut self) {
        if self.in_demo() {
            return;
        }
        if self.settings.take() {
            self.fx.push(Effect::Store(Store::Settings));
        }
    }

    /// Ask for the save to be written for a settings change. During the
    /// attract demo the change is kept pending instead, for after it.
    pub(super) fn write_settings(&mut self) {
        if self.spectating() {
            self.settings.touch();
            return;
        }
        self.fx.push(Effect::Store(Store::Settings));
    }

    /// Say a write failed. A save from a newer build is never written by this
    /// one (by design, `save::store`), so every change would fail; that gets
    /// one note saying why, once a session, instead of a complaint each time.
    pub(super) fn note_not_saved(&mut self, what: &str) {
        if !self.save.from_the_future {
            self.note(what.into());
        } else if !self.future_noted {
            self.future_noted = true;
            self.note("SAVE IS FROM A NEWER VERSION".into());
        }
    }

    /// Put last session's crash report on the clipboard. The first copy
    /// retires the report on disk, so the next launch does not offer it.
    pub(super) fn copy_last_crash(&mut self) {
        let Some(text) = self.crash.last_crash.clone() else {
            return;
        };
        self.fx.push(Effect::Clipboard(text));
        self.play(Sound::Ui);
        if !self.crash.crash_copied {
            self.crash.crash_copied = true;
            self.fx.push(Effect::CrashReport(CrashReport::Retire));
        }
    }

    /// The player has moved on from last session's report (started a run):
    /// stop offering it.
    pub(super) fn retire_last_crash(&mut self) {
        if self.spectating() {
            return;
        }
        if self.crash.last_crash.take().is_some() && !self.crash.crash_copied {
            self.fx.push(Effect::CrashReport(CrashReport::Retire));
        }
    }
}

// ----------------------------------------------------- lifecycle and resume

impl Game {
    /// Focus lost, or the operating system paused the app: stop the clock on
    /// a live screen, so a shot in the air or a clear in progress is still
    /// there to watch on return. Not during a draft, where nothing moves and
    /// the cards themselves say where the player was; a pause screen over
    /// them would only cost a tap. Not for the dev autopilot or screenshots,
    /// which nobody is watching with focus.
    ///
    /// Nor for the attract demo: it has no pause plate, and a phone that
    /// sleeps its screen stops the demo with the app anyway.
    pub(super) fn auto_pause(&mut self) {
        if self.dev.auto || self.dev.capturing || self.in_demo() {
            return;
        }
        // A watch pauses itself, under its own bar: nobody is watching.
        if let Some(w) = self.watch.as_mut() {
            w.paused = true;
            self.fx.push(Effect::Redraw);
            return;
        }
        if self.mode == Mode::Trial {
            if let Some(trial) = self.trial.as_mut() {
                trial.paused = true;
                self.fx.push(Effect::Redraw);
            }
            return;
        }
        self.auto_pause_versus();
        if self.mode != Mode::Playing {
            return;
        }
        if self.play.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            return;
        }
        self.input.touch.cancel_drag();
        self.mode = Mode::Paused;
        // From a notification, outside the frame's before/after compare.
        self.fx.push(Effect::Redraw);
    }

    /// A new run replaces whatever was saved: it is written on the next
    /// frame, log and all (empty, at the start).
    pub(super) fn begin_persisting(&mut self) {
        self.run_file.resumable = None;
        self.run_file.discard_armed = None;
        self.run_file.run_saved_len = None;
        self.run_file.run_persist = true;
        self.run_file.resumed = false;
    }

    /// The run is over: nothing is left to resume. Returns whether it was
    /// being kept on disk, in which case its finished log is still there,
    /// for the score to clear before it goes on the board (`finish_run`).
    pub(super) fn stop_persisting(&mut self) -> bool {
        self.run_file.resumable = None;
        std::mem::take(&mut self.run_file.run_persist)
    }

    /// A new run replaces the saved one, so over a saved run the first press
    /// of a new-run action only asks, and the second goes ahead. Returns
    /// whether to go ahead.
    pub(super) fn confirm_discard(&mut self, what: &'static str) -> bool {
        if self.run_file.resumable.is_none() || self.run_file.discard_armed == Some(what) {
            self.run_file.discard_armed = None;
            return true;
        }
        self.run_file.discard_armed = Some(what);
        self.play(Sound::Ui);
        self.fx.push(Effect::Redraw);
        false
    }

    /// Save the run whenever the log has grown: a shot fired or a
    /// draft taken (a reposition rides in the next shot's entry). Checked
    /// every frame, written only on a change.
    ///
    /// Never during the attract demo: `run_persist` may still be set for the
    /// suspended run, and the run on screen is the demo's.
    pub(super) fn persist_run(&mut self) {
        if !self.run_file.run_persist || self.spectating() {
            return;
        }
        let Some(run) = self.play.run.as_ref() else {
            return;
        };
        let len = run.log.len();
        if self.run_file.run_saved_len == Some(len) {
            return;
        }
        // Tried once per change: a failed write is logged by `save::write`,
        // and the next shot tries again.
        self.fx
            .push(Effect::RunFile(RunFile::Write(self.run_file_text())));
        self.run_file.run_saved_len = Some(len);
    }

    /// The run file as it should be now: the lines of finished runs whose
    /// score has not reached the board, then the run kept for CONTINUE.
    pub(super) fn run_file_text(&self) -> String {
        let mut text: String = self
            .run_file
            .unscored
            .iter()
            .map(|u| u.line.as_str())
            .collect();
        text.push_str(&self.kept_line().unwrap_or_default());
        text
    }

    /// The line of the run kept for CONTINUE: the one being played, if it
    /// is kept on disk, else the one found at launch.
    pub(super) fn kept_line(&self) -> Option<String> {
        if self.run_file.run_persist {
            let run = self.play.run.as_ref()?;
            Some(resume::Saved::of(run, self.play.daily).to_line())
        } else {
            self.run_file.resumable.as_ref().map(resume::Saved::to_line)
        }
    }

    /// Quit to title keeps the run. It is already on disk, so CONTINUE picks
    /// it up this session exactly as it would after the app was killed; only
    /// starting another run gives it up. Stepping away should not cost a run
    /// the player only wanted a break from.
    ///
    /// A run that is already over (the last shell was in the air when the
    /// player paused) is scored now instead, and shows its result: leaving it
    /// on disk would let the next new run overwrite an unrecorded score.
    pub(super) fn suspend_run(&mut self) {
        if self.play.run.as_ref().is_some_and(|r| r.is_over()) {
            self.play.anim = None;
            self.play.clear_hold = 0.0;
            self.finish_run();
            return;
        }
        if !self.run_file.run_persist {
            return;
        }
        self.run_file.resumable = self
            .play
            .run
            .as_ref()
            .map(|r| resume::Saved::of(r, self.play.daily));
    }

    /// Rebuild the saved run and drop the player back in where it stood.
    pub(crate) fn continue_run(&mut self) {
        let Some(saved) = self.run_file.resumable.take() else {
            return;
        };
        let Some(run) = saved.reconstruct() else {
            // Only the finished runs waiting for the board stay.
            let text = self.run_file_text();
            self.fx.push(Effect::RunFile(if text.is_empty() {
                RunFile::Discard
            } else {
                RunFile::Write(text)
            }));
            self.note(resume::Stale::Diverged.note().into());
            return;
        };
        self.set_run_width(Some(saved.width));
        (self.play.angle_ticks, self.play.power_ticks) = saved.aim().unwrap_or((900, 600));
        self.play.anim = None;
        self.play.last_arc.clear();
        self.play.clear_hold = 0.0;
        self.play.cam_x = 0;
        self.play.cam_target = 0.0;
        self.play.daily = saved.daily;
        self.play.last_seed = saved.seed;
        self.play.low_shell_warned = run.shells < 10;
        self.retire_last_crash();
        let screen = run.screen_index;
        // What is on disk is this run, as rebuilt.
        self.run_file.run_saved_len = Some(run.log.len());
        self.run_file.run_persist = true;
        self.run_file.resumed = true;
        self.play.challenge = None;
        self.play.custom_challenge = None;
        self.play.hint_arc.clear();
        self.play.run = Some(run);
        self.mode = Mode::Playing;
        self.new_preview();
        self.fx.push(Effect::Redraw);
        self.play(Sound::Ui);
        // Buffer size for the loadout.
        self.after_screen_change();
        self.note(format!(
            "SCREEN {}  SEED {}",
            screen + 1,
            format_seed(saved.seed)
        ));
    }
}

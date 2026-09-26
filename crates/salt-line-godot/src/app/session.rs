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
//!        run over: finish_run ────────┘ stop_persisting: Discard, then Score
//! ```

use super::*;

impl Game {
    /// Show `text` on the status line for `STATUS_TIME` seconds.
    pub(super) fn note(&mut self, text: String) {
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
    pub(super) fn learn(&mut self, bit: u32) {
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
        let in_run = matches!(self.mode, Mode::Playing | Mode::Paused);
        if !in_run && self.display.kept_width {
            // A resumed run's width is done with: back to the device's.
            self.set_run_width(None);
            self.apply_buffer_size();
        } else if self.play.run.is_none() && want_w != self.display.buffer_w {
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
        self.fx.push(Effect::Aspect(if want == device {
            Aspect::Expand
        } else {
            Aspect::Keep
        }));
    }

    /// Write pending settings once they settle, or as soon as the player
    /// leaves Controls. Not every volume repeat: see `durable::Debounce`.
    pub(super) fn tick_settings(&mut self, delta: f64) {
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

    /// Ask for the save to be written for a settings change.
    pub(super) fn write_settings(&mut self) {
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
    pub(super) fn auto_pause(&mut self) {
        if self.mode != Mode::Playing || self.dev.auto || self.dev.capturing {
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

    /// The run is over and on the board: nothing is left to resume.
    pub(super) fn stop_persisting(&mut self) {
        if self.run_file.run_persist {
            self.fx.push(Effect::RunFile(RunFile::Discard));
        }
        self.run_file.run_persist = false;
        self.run_file.resumable = None;
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
    pub(super) fn persist_run(&mut self) {
        if !self.run_file.run_persist {
            return;
        }
        let Some(run) = self.play.run.as_ref() else {
            return;
        };
        let len = run.log.len();
        if self.run_file.run_saved_len == Some(len) {
            return;
        }
        let line = resume::Saved::of(run, self.play.daily).to_line();
        // Tried once per change: a failed write is logged by `save::write`,
        // and the next shot tries again.
        self.fx.push(Effect::RunFile(RunFile::Write(line)));
        self.run_file.run_saved_len = Some(len);
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
            self.fx.push(Effect::RunFile(RunFile::Discard));
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

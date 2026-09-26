//! Upkeep around the game: the buffer and window, sounds, settings, crash
//! reports, pausing on focus loss, and keeping the run on disk (DESIGN §9.1).

use super::*;

impl Game {
    pub(super) fn note(&mut self, text: String) {
        self.status = text;
        self.status_time = STATUS_TIME;
        self.fx.push(Effect::Redraw);
    }

    pub(super) fn play(&mut self, sound: Sound) {
        self.fx.push(Effect::Play(sound));
    }

    pub(super) fn apply_volume(&mut self) {
        self.fx.push(Effect::Volume(self.volume_db()));
    }

    /// The drag preview must be worked out afresh.
    pub(super) fn new_preview(&mut self) {
        self.preview_epoch = self.preview_epoch.wrapping_add(1);
    }

    pub(super) fn learn(&mut self, bit: u32) {
        if self.save.learned & bit == 0 {
            self.save.learned |= bit;
            self.fx.push(Effect::Store(Store::Learned));
        }
    }

    pub(super) fn apply_buffer_size(&mut self) {
        let h = if self.long_lens() {
            LONG_LENS_H
        } else {
            SCREEN_H as i32
        };
        self.buffer_h = h;
        self.letterbox = self.letterbox_now();
        self.fx.push(Effect::Buffer {
            w: self.buffer_w,
            h,
            letterbox: self.letterbox,
        });
        self.fx.push(Effect::Redraw);
    }

    /// Follow the display: the window size every frame, and the cutout as
    /// the adapter last read it (a platform round trip on a phone, so not
    /// every frame; see `frame::DisplayPoll`).
    pub(super) fn poll_display(&mut self, window: (i32, i32), cutout: (i32, i32)) {
        self.window_size = window;
        self.cutout = cutout;
        let want_w = self.pick_width();
        let in_run = matches!(self.mode, Mode::Playing | Mode::Paused);
        if !in_run && self.kept_width {
            // A resumed run's width is done with: back to the device's.
            self.set_run_width(None);
            self.apply_buffer_size();
        } else if self.run.is_none() && want_w != self.buffer_w {
            self.buffer_w = want_w;
            self.apply_buffer_size();
        } else if self.letterbox_now() != self.letterbox {
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
        self.buffer_w = want;
        self.kept_width = want != device;
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

    pub(super) fn copy_last_crash(&mut self) {
        let Some(text) = self.last_crash.clone() else {
            return;
        };
        self.fx.push(Effect::Clipboard(text));
        self.play(Sound::Ui);
        if !self.crash_copied {
            self.crash_copied = true;
            self.fx.push(Effect::RetireCrashReport);
        }
    }

    /// The player has moved on from last session's report (started a run):
    /// stop offering it.
    pub(super) fn retire_last_crash(&mut self) {
        if self.last_crash.take().is_some() && !self.crash_copied {
            self.fx.push(Effect::RetireCrashReport);
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
        if self.mode != Mode::Playing || self.auto || self.capturing {
            return;
        }
        if self.run.as_ref().map(|r| r.phase) == Some(Phase::Drafting) {
            return;
        }
        self.touch.cancel_drag();
        self.mode = Mode::Paused;
        // From a notification, outside the frame's before/after compare.
        self.fx.push(Effect::Redraw);
    }

    /// A new run replaces whatever was saved: it is written on the next
    /// frame, log and all (empty, at the start).
    pub(super) fn begin_persisting(&mut self) {
        self.resumable = None;
        self.discard_armed = None;
        self.run_saved_len = None;
        self.run_persist = true;
        self.resumed = false;
    }

    /// The run is over and on the board: nothing is left to resume.
    pub(super) fn stop_persisting(&mut self) {
        if self.run_persist {
            self.fx.push(Effect::DiscardRunFile);
        }
        self.run_persist = false;
        self.resumable = None;
    }

    /// A new run replaces the saved one, so over a saved run the first press
    /// of a new-run action only asks, and the second goes ahead. Returns
    /// whether to go ahead.
    pub(super) fn confirm_discard(&mut self, what: &'static str) -> bool {
        if self.resumable.is_none() || self.discard_armed == Some(what) {
            self.discard_armed = None;
            return true;
        }
        self.discard_armed = Some(what);
        self.play(Sound::Ui);
        self.fx.push(Effect::Redraw);
        false
    }

    /// Write `user://run.txt` whenever the log has grown: a shot fired or a
    /// draft taken (a reposition rides in the next shot's entry). Checked
    /// every frame, written only on a change.
    pub(super) fn persist_run(&mut self) {
        if !self.run_persist {
            return;
        }
        let Some(run) = self.run.as_ref() else { return };
        let len = run.log.len();
        if self.run_saved_len == Some(len) {
            return;
        }
        let line = resume::Saved::of(run, self.daily).to_line();
        // Tried once per change: a failed write is logged by `save::write`,
        // and the next shot tries again.
        self.fx.push(Effect::WriteRun(line));
        self.run_saved_len = Some(len);
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
        if self.run.as_ref().is_some_and(|r| r.is_over()) {
            self.anim = None;
            self.clear_hold = 0.0;
            self.finish_run();
            return;
        }
        if !self.run_persist {
            return;
        }
        self.resumable = self.run.as_ref().map(|r| resume::Saved::of(r, self.daily));
    }

    /// Rebuild the saved run and drop the player back in where it stood.
    pub fn continue_run(&mut self) {
        let Some(saved) = self.resumable.take() else {
            return;
        };
        let Some(run) = saved.reconstruct() else {
            self.fx.push(Effect::DiscardRunFile);
            self.note(resume::Stale::Diverged.note().into());
            return;
        };
        self.set_run_width(Some(saved.width));
        (self.angle_ticks, self.power_ticks) = saved.aim().unwrap_or((900, 600));
        self.anim = None;
        self.last_arc.clear();
        self.clear_hold = 0.0;
        self.cam_x = 0;
        self.cam_target = 0.0;
        self.daily = saved.daily;
        self.last_seed = saved.seed;
        self.low_shell_warned = run.shells < 10;
        self.retire_last_crash();
        let screen = run.screen_index;
        // What is on disk is this run, as rebuilt.
        self.run_saved_len = Some(run.log.len());
        self.run_persist = true;
        self.resumed = true;
        self.run = Some(run);
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

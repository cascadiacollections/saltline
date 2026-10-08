//! Reading actions from the frame's input, and what each mode does with
//! them. See `input` for the take-once press rule the order here relies on.
//!
//! Game code asks about actions, never devices: `action_pressed(Fire)` is
//! true for the bound key, the bound pad button or the FIRE touch button
//! alike. Asking is also what records which device is in use
//! (`controller_active`, `touch_active`), so prompts name the input the
//! player last touched.
//!
//! # Take-once, and why order matters
//!
//! A key or pad press is removed as the first action that asks for it
//! returns true. Pad X is draft 1, reposition left, COPY and LAST ARC by
//! default; each mode asks only its own actions, and within a mode the
//! earlier question wins. That is why the handlers below are written as
//! `if … else if` chains in a deliberate order, and why moving a check can
//! change behaviour. Touch presses are not taken: every touch button belongs
//! to one action, so there is nothing to share.

use super::*;

impl Game {
    /// The key and pad tokens a `bind` line in the save gives `action`, or
    /// the defaults passed in when there is none.
    pub(super) fn binding(
        &self,
        action: Action,
        default_key: &str,
        default_pad: &str,
    ) -> (String, String) {
        self.save
            .bindings
            .iter()
            .find(|(a, _, _)| action.token() == Some(a.as_str()))
            .map(|(_, k, p)| (k.clone(), p.clone()))
            .unwrap_or_else(|| (default_key.into(), default_pad.into()))
    }

    /// The key and pad tokens bound to an action.
    pub(crate) fn action_binding(&self, action: Action) -> (String, String) {
        let defaults = input::default_binding(action);
        self.binding(action, defaults.0, defaults.1)
    }

    /// Once per frame, before any action is read: publish the presses queued
    /// since the last frame (edge latching) and advance the touch repeat
    /// timers.
    pub(super) fn begin_input_frame(&mut self, delta: f64, intents: &Intents) {
        self.input.controls.begin(intents);
        self.input.touch.update(delta, REPEAT_DELAY, REPEAT_RATE);
        // After the events, which were taken while it still ran.
        self.input.touch_settle = (self.input.touch_settle - delta).max(0.0);
    }

    /// This frame's press of a key, taken. See `Controls::pressed`.
    pub(super) fn pressed(&mut self, key: Key) -> bool {
        self.input.controls.pressed(key)
    }

    /// Shift is held: aim steps are ten times coarser.
    pub(super) fn shift_held(&self) -> bool {
        self.input.controls.held(Key::Shift)
    }

    /// True on the frame the button went down on any joypad. Taken, and
    /// marks the controller as the device in use.
    pub(super) fn pad_pressed(&mut self, button: Pad) -> bool {
        let down = self.input.controls.take_pad(button);
        if down {
            self.input.controller_active = true;
            self.input.touch_active = false;
        }
        down
    }

    /// The action was pressed this frame, on any device: its key, its pad
    /// button, its touch button, or for `Back` the platform's back gesture.
    /// Key and pad presses are taken, so ask in the order that should win.
    pub(super) fn action_pressed(&mut self, action: Action) -> bool {
        let (key, _) = self.action_binding(action);
        let key_down = Key::from_token(&key)
            .map(|k| self.pressed(k))
            .unwrap_or(false);
        if key_down {
            self.input.controller_active = false;
            self.input.touch_active = false;
        }
        let pad_down = self.pad_action_pressed(action);
        let touch_down = self.input.touch.pressed(action)
            || (action == Action::Back && self.input.back_requested);
        key_down || pad_down || touch_down
    }

    /// `action_pressed` for the controller alone.
    pub(super) fn pad_action_pressed(&mut self, action: Action) -> bool {
        let (_, pad) = self.action_binding(action);
        Pad::from_token(&pad)
            .map(|b| self.pad_pressed(b))
            .unwrap_or(false)
    }

    /// Activations of `action` this frame with hold-to-repeat, summed over
    /// key, pad button, left stick and touch button: 1 on the press, then
    /// one every `REPEAT_RATE` from `REPEAT_DELAY + REPEAT_RATE` on.
    ///
    /// The key and pad repeat state is kept per key and per button; the
    /// stick has no press to take, so its state is kept per `slot`, one for
    /// each place that asks (0 to 7). Two places asking with the same slot in
    /// one frame would tick that state twice.
    pub(super) fn action_repeated(&mut self, slot: usize, action: Action, delta: f64) -> u32 {
        let (key, pad) = self.action_binding(action);
        let keyboard = Key::from_token(&key)
            .map(|k| {
                self.input
                    .controls
                    .repeated(k, delta, REPEAT_DELAY, REPEAT_RATE)
            })
            .unwrap_or(0);
        let buttons = match Pad::from_token(&pad) {
            Some(b) => self
                .input
                .controls
                .pad_repeated(b, delta, REPEAT_DELAY, REPEAT_RATE),
            None => 0,
        };
        // Left stick is always available in addition to remappable D-pad.
        let dir = match action {
            Action::Left => Some(input::STICK_LEFT),
            Action::Right => Some(input::STICK_RIGHT),
            Action::Up => Some(input::STICK_UP),
            Action::Down => Some(input::STICK_DOWN),
            _ => None,
        };
        let stick_down = dir.is_some_and(|d| self.input.controls.stick(d));
        let stick =
            self.input
                .controls
                .stick_repeated(slot, stick_down, delta, REPEAT_DELAY, REPEAT_RATE);
        if keyboard > 0 {
            self.input.controller_active = false;
            self.input.touch_active = false;
        }
        if buttons > 0 || stick_down {
            self.input.controller_active = true;
            self.input.touch_active = false;
        }
        keyboard + buttons + stick + self.input.touch.repeated(action)
    }

    /// What a prompt calls an action on the device in use.
    pub(crate) fn input_label(&self, action: Action) -> String {
        if self.input.touch_active {
            return input::touch_label(action).to_string();
        }
        let (key, pad) = self.action_binding(action);
        if self.input.controller_active {
            pad
        } else {
            key
        }
    }
}

// -------------------------------------------------------------- mode input

impl Game {
    /// Run the current mode's input for this frame. Mute and the F1 overlay
    /// work in every mode, so they are asked first.
    pub(super) fn handle_mode_input(&mut self, delta: f64) {
        if self.pressed(Key::F1) {
            self.debug = !self.debug;
        }
        if self.action_pressed(Action::Mute) {
            self.save.muted = !self.save.muted;
            self.apply_volume();
            self.settings.touch();
            self.note(if self.save.muted {
                "SOUND MUTED".into()
            } else {
                "SOUND ON".into()
            });
        }
        match self.mode {
            Mode::Title => {
                if self.action_pressed(Action::TitleMusic) {
                    self.save.title_music = !self.save.title_music;
                    self.settings.touch();
                    self.note(if self.save.title_music {
                        "TITLE MUSIC ON".into()
                    } else {
                        "TITLE MUSIC OFF".into()
                    });
                }
                // With a saved run, confirm is CONTINUE and a new daily run
                // moves to its own action; without one, confirm starts the
                // daily.
                let resumable = self.run_file.resumable.is_some();
                let confirm = self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter);
                let daily = if resumable {
                    self.action_pressed(Action::NewRun)
                } else {
                    confirm
                };
                if resumable && confirm {
                    self.continue_run();
                } else if daily {
                    if self.confirm_discard("daily") {
                        let seed = daily_seed(self.today);
                        self.play(Sound::Ui);
                        self.start_run(seed, true);
                    }
                } else if self.action_pressed(Action::Seed) {
                    if self.confirm_discard("seed") {
                        self.open_seed_entry();
                    }
                } else if self.action_pressed(Action::Versus) {
                    // A match never touches the run kept for CONTINUE, so
                    // there is nothing to confirm. Over a suspended match
                    // the versus row is RESUME MATCH; a new match is had by
                    // quitting that one, as in play.
                    self.play(Sound::Ui);
                    if self.saved_match.is_some() {
                        self.resume_match();
                    } else {
                        self.open_versus_setup();
                    }
                } else if self.action_pressed(Action::Demo) {
                    // Like versus, the demo never touches the kept run.
                    self.start_demo(DemoStarted::Menu);
                } else if self.action_pressed(Action::Watch) {
                    // Nor does watching a finished one (DESIGN §9.4).
                    self.play(Sound::Ui);
                    self.open_replays();
                } else if self.crash.last_crash.is_some() && self.action_pressed(Action::Copy) {
                    self.copy_last_crash();
                } else if self.action_pressed(Action::Controls) {
                    self.play(Sound::Ui);
                    self.mode = Mode::Controls;
                } else if self.action_pressed(Action::Back) {
                    self.fx.push(Effect::Quit);
                }
                // Leaving the title disarms a half-confirmed discard, so a
                // press on a later visit asks again.
                if self.mode != Mode::Title {
                    self.run_file.discard_armed = None;
                }
            }
            Mode::Controls => {
                let down = self.action_repeated(0, Action::Left, delta);
                let up = self.action_repeated(1, Action::Right, delta);
                if down + up > 0 {
                    // Five percent a step. The write is debounced
                    // (`settings.touch`), not made on every repeat.
                    let volume =
                        (self.save.volume as i32 + (up as i32 - down as i32) * 5).clamp(0, 100);
                    self.save.volume = volume as u8;
                    self.save.muted = false;
                    self.apply_volume();
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.input.touch.pressed(Action::AimMode) {
                    self.save.touch_aim = Some(self.touch_aim().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                } else if self.input.touch.pressed(Action::ArcHint) {
                    self.save.arc_hint = Some(self.arc_hint().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(Action::LastArc) {
                    self.save.last_arc = Some(!self.last_arc_on());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(Action::Demo) {
                    self.save.idle_demo = Some(!self.idle_demo_on());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(Action::Replay) {
                    self.save.bindings.clear();
                    self.settings.touch();
                    self.note("DEFAULT BINDINGS RESTORED".into());
                } else if self.action_pressed(Action::Back) {
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                }
            }
            Mode::Paused => {
                if self.action_pressed(Action::Confirm) || self.action_pressed(Action::Back) {
                    self.play(Sound::Ui);
                    self.mode = Mode::Playing;
                } else if self.action_pressed(Action::Replay) {
                    // Quit to title keeps the run for CONTINUE.
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                    self.suspend_run();
                }
            }
            Mode::SeedEntry => self.handle_seed_input(delta),
            Mode::GameOver => {
                if self.action_pressed(Action::Copy) {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.play.last_seed);
                    self.fx.push(Effect::Clipboard(seed.clone()));
                    self.note(format!("COPIED {seed}"));
                } else if self.action_pressed(Action::Confirm) || self.action_pressed(Action::Back)
                {
                    self.mode = Mode::Title;
                } else if self.action_pressed(Action::Replay) {
                    let seed = self.play.last_seed;
                    let daily = self.play.daily;
                    self.start_run(seed, daily);
                } else if self.action_pressed(Action::Watch) {
                    // WATCH the run just ended, then back to this screen.
                    self.watch_last_run();
                }
            }
            Mode::Replays => self.handle_replays_input(delta),
            Mode::Playing => self.handle_play_input(delta),
            Mode::VersusSetup => self.handle_versus_setup_input(),
            Mode::Versus => self.handle_versus_input(delta),
            Mode::Editor => self.handle_editor_input(delta),
        }
    }

    /// Keyboard text comes in as typed events (`Game::typed`). This is the touch keypad, and
    /// the controller moving a focus over the same keypad: A presses the
    /// focused key, B deletes, or leaves once nothing is typed.
    pub(super) fn handle_seed_input(&mut self, delta: f64) {
        let dx = self.action_repeated(1, Action::Right, delta) as i32
            - self.action_repeated(0, Action::Left, delta) as i32;
        let dy = self.action_repeated(2, Action::Down, delta) as i32
            - self.action_repeated(3, Action::Up, delta) as i32;
        if dx != 0 || dy != 0 {
            self.input.seed_focus.step(dx, dy);
        }
        let key = if self.pad_action_pressed(Action::Confirm) {
            Some(self.input.seed_focus.action())
        } else if self.pad_action_pressed(Action::Back) {
            Some(if self.seed_text.is_empty() {
                Action::Back
            } else {
                keypad::DEL
            })
        } else if self.input.touch.pressed(Action::Back) || self.input.back_requested {
            Some(Action::Back)
        } else {
            [Action::Confirm, keypad::DEL]
                .into_iter()
                .chain(keypad::KEYS)
                .find(|k| self.input.touch.pressed(*k))
        };
        match key {
            Some(Action::Confirm) => self.start_seeded_run(),
            Some(Action::Back) => self.leave_seed_entry(),
            Some(keypad::DEL) => {
                self.seed_text.pop();
            }
            Some(Action::Type(c)) if self.seed_text.chars().count() < SEED_MAX => {
                self.seed_text.push(c)
            }
            _ => {}
        }
    }

    /// A run in play: pause, draft picks, then aim, fuse, reposition and fire.
    /// Nothing but pause is read while a shot is being replayed or a clear
    /// held.
    pub(super) fn handle_play_input(&mut self, delta: f64) {
        if self.action_pressed(Action::Back) {
            self.input.touch.cancel_drag();
            self.mode = Mode::Paused;
            return;
        }
        let phase = self.play.run.as_ref().map(|r| r.phase);
        let Some(phase) = phase else { return };

        // A drag only aims on the screen it started on: drafting or a shot in
        // the air forgets it, so it can never drop into a later shot.
        if phase != Phase::Firing || self.play.anim.is_some() || self.play.clear_hold > 0.0 {
            self.input.touch.cancel_drag();
        }

        if phase == Phase::Drafting {
            for (i, action) in [Action::Draft1, Action::Draft2, Action::Draft3]
                .into_iter()
                .enumerate()
            {
                if self.action_pressed(action) {
                    let taken = self.play.run.as_mut().and_then(|r| r.take_offer(i));
                    if let Some(m) = taken {
                        self.new_preview();
                        self.learn(LEARN_DRAFT);
                        self.play(Sound::Draft);
                        self.note(format!("TOOK {}", m.name()));
                        self.apply_buffer_size();
                    }
                }
            }
            return;
        }

        // Nothing but waiting while a shot is in the air.
        if self.play.anim.is_some() || self.play.clear_hold > 0.0 {
            return;
        }

        let quantum = self
            .play
            .run
            .as_ref()
            .map(|r| r.loadout.angle_quantum_ticks)
            .unwrap_or(5)
            .max(1);
        let mult = if self.shift_held() { 10 } else { 1 };

        // Stick slots: 0 left, 1 right, 2 down, 3 up, 4 fuse down, 5 fuse up.

        let left = self.action_repeated(0, Action::Left, delta) as i32;
        let right = self.action_repeated(1, Action::Right, delta) as i32;
        if left + right != 0 {
            self.learn(LEARN_ANGLE);
            self.play.angle_ticks = (self.play.angle_ticks + (right - left) * quantum * mult)
                .clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, Action::Down, delta) as i32;
        let up = self.action_repeated(3, Action::Up, delta) as i32;
        if up + down != 0 {
            self.learn(LEARN_POWER);
            self.play.power_ticks =
                (self.play.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }

        // Airburst fuse.
        let fuse_step = 4.0 * mult as f64;
        let minus = self.action_repeated(4, Action::FuseDown, delta) as f64;
        let plus = self.action_repeated(5, Action::FuseUp, delta) as f64;
        if minus + plus > 0.0 {
            if let Some(r) = self.play.run.as_mut() {
                if r.loadout.airburst() {
                    let f = r.fuse + (plus - minus) * fuse_step;
                    r.set_fuse(f);
                }
            }
        }

        // Enfilade reposition.
        let l = self.action_pressed(Action::RepositionLeft);
        let rr = self.action_pressed(Action::RepositionRight);
        if l || rr {
            let step = if l { -5.0 } else { 5.0 };
            let ok = self.play.run.as_mut().map(|r| {
                let target = r.emplacement_offset + step;
                r.reposition(target)
            });
            if ok == Some(false) {
                self.note("NO ENFILADE".into());
            }
        }

        if self.action_pressed(Action::Fire) || self.pressed(Key::Enter) {
            self.fire();
        }

        if self.drag_aiming() && self.drag_aim(quantum, false, true) {
            self.fire();
        }
    }

    /// Pull back to aim, let go to fire. The aim follows the finger live, so
    /// the readouts and barrel show exactly what a release would fire; pulling
    /// back inside the dead zone puts the old aim back and cancels. Returns
    /// true when the finger has just let go of an aim: the caller fires.
    ///
    /// `mirrored` is for a gun that fires leftward (versus, the right
    /// player): the pull is reflected left to right before it becomes an
    /// aim, so pulling back to the right throws to the left. `teach` marks
    /// the angle and power lessons learned (`learn`), which only a run
    /// does: versus writes nothing.
    pub(super) fn drag_aim(&mut self, quantum: i32, mirrored: bool, teach: bool) -> bool {
        let dropped = self.input.touch.dropped();
        let Some((start, end)) = dropped.or(self.input.touch.drag()) else {
            // Abandoned (paused, focus lost): put the old aim back.
            if let Some(before) = self.input.drag_from.take() {
                (self.play.angle_ticks, self.play.power_ticks) = before;
            }
            return false;
        };
        let (start, end) = if mirrored {
            ((-start.0, start.1), (-end.0, end.1))
        } else {
            (start, end)
        };
        let before = *self
            .input
            .drag_from
            .get_or_insert((self.play.angle_ticks, self.play.power_ticks));
        let aim = crate::aim::pull_to_aim(start, end, quantum);
        match aim {
            Some((angle, power)) => {
                if teach {
                    self.learn(LEARN_ANGLE);
                    self.learn(LEARN_POWER);
                }
                (self.play.angle_ticks, self.play.power_ticks) = (angle, power);
            }
            None => (self.play.angle_ticks, self.play.power_ticks) = before,
        }
        if dropped.is_some() {
            self.input.drag_from = None;
            return aim.is_some();
        }
        false
    }

    /// Start a run on the typed seed; blank starts today's daily seed, but
    /// not as the daily (it is scored as a seeded run).
    pub(super) fn start_seeded_run(&mut self) {
        let seed = if self.seed_text.trim().is_empty() {
            daily_seed(self.today)
        } else {
            parse_seed(&self.seed_text)
        };
        self.start_run(seed, false);
    }

    pub(super) fn leave_seed_entry(&mut self) {
        self.mode = Mode::Title;
    }

    /// Seed entry, empty, with the keypad focus on its first key.
    pub(crate) fn open_seed_entry(&mut self) {
        self.seed_text.clear();
        self.input.seed_focus = Focus::default();
        self.mode = Mode::SeedEntry;
    }
}

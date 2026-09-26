//! Reading actions from the frame's input, and what each mode does with
//! them. See `input` for the take-once press rule the order here relies on.

use super::*;

impl Game {
    pub(super) fn binding(
        &self,
        action: &str,
        default_key: &str,
        default_pad: &str,
    ) -> (String, String) {
        self.save
            .bindings
            .iter()
            .find(|(a, _, _)| a == action)
            .map(|(_, k, p)| (k.clone(), p.clone()))
            .unwrap_or_else(|| (default_key.into(), default_pad.into()))
    }

    /// The key and pad tokens bound to an action.
    pub fn action_binding(&self, action: &str) -> (String, String) {
        let defaults = input::default_binding(action);
        self.binding(action, defaults.0, defaults.1)
    }

    /// Once per frame, before any action is read.
    pub(super) fn begin_input_frame(&mut self, delta: f64, intents: &Intents) {
        self.controls.begin(intents);
        self.touch.update(delta, REPEAT_DELAY, REPEAT_RATE);
    }

    /// This frame's press of a key, taken. See `Controls::pressed`.
    pub(super) fn pressed(&mut self, key: Key) -> bool {
        self.controls.pressed(key)
    }

    pub(super) fn shift_held(&self) -> bool {
        self.controls.held(Key::Shift)
    }

    /// True on the frame the button went down on any joypad.
    pub(super) fn pad_pressed(&mut self, button: Pad) -> bool {
        let down = self.controls.take_pad(button);
        if down {
            self.controller_active = true;
            self.touch_active = false;
        }
        down
    }

    pub(super) fn action_pressed(&mut self, _slot: usize, action: &str) -> bool {
        let (key, _) = self.action_binding(action);
        let key_down = Key::from_token(&key)
            .map(|k| self.pressed(k))
            .unwrap_or(false);
        if key_down {
            self.controller_active = false;
            self.touch_active = false;
        }
        let pad_down = self.pad_action_pressed(action);
        let touch_down = self.touch.pressed(action) || (action == "back" && self.back_requested);
        key_down || pad_down || touch_down
    }

    /// `action_pressed` for the controller alone.
    pub(super) fn pad_action_pressed(&mut self, action: &str) -> bool {
        let (_, pad) = self.action_binding(action);
        Pad::from_token(&pad)
            .map(|b| self.pad_pressed(b))
            .unwrap_or(false)
    }

    pub(super) fn action_repeated(&mut self, slot: usize, action: &str, delta: f64) -> u32 {
        let (key, pad) = self.action_binding(action);
        let keyboard = Key::from_token(&key)
            .map(|k| self.controls.repeated(k, delta, REPEAT_DELAY, REPEAT_RATE))
            .unwrap_or(0);
        let buttons = match Pad::from_token(&pad) {
            Some(b) => self
                .controls
                .pad_repeated(b, delta, REPEAT_DELAY, REPEAT_RATE),
            None => 0,
        };
        // Left stick is always available in addition to remappable D-pad.
        let dir = match action {
            "left" => Some(input::STICK_LEFT),
            "right" => Some(input::STICK_RIGHT),
            "up" => Some(input::STICK_UP),
            "down" => Some(input::STICK_DOWN),
            _ => None,
        };
        let stick_down = dir.is_some_and(|d| self.controls.stick(d));
        let stick =
            self.controls
                .stick_repeated(slot, stick_down, delta, REPEAT_DELAY, REPEAT_RATE);
        if keyboard > 0 {
            self.controller_active = false;
            self.touch_active = false;
        }
        if buttons > 0 || stick_down {
            self.controller_active = true;
            self.touch_active = false;
        }
        keyboard + buttons + stick + self.touch.repeated(action)
    }

    /// What a prompt calls an action on the device in use.
    pub fn input_label(&self, action: &str) -> String {
        if self.touch_active {
            return input::touch_label(action).to_string();
        }
        let (key, pad) = self.action_binding(action);
        if self.controller_active {
            pad
        } else {
            key
        }
    }
}

// -------------------------------------------------------------- mode input

impl Game {
    pub(super) fn handle_mode_input(&mut self, delta: f64) {
        if self.pressed(Key::F1) {
            self.debug = !self.debug;
        }
        if self.action_pressed(15, "mute") {
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
                let resumable = self.resumable.is_some();
                let confirm = self.action_pressed(0, "confirm") || self.pressed(Key::KpEnter);
                let daily = if resumable {
                    self.action_pressed(11, "new_run")
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
                } else if self.action_pressed(17, "seed") {
                    if self.confirm_discard("seed") {
                        self.open_seed_entry();
                    }
                } else if self.last_crash.is_some() && self.action_pressed(4, "copy") {
                    self.copy_last_crash();
                } else if self.action_pressed(1, "controls") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Controls;
                } else if self.action_pressed(2, "back") {
                    self.fx.push(Effect::Quit);
                }
                if self.mode != Mode::Title {
                    self.discard_armed = None;
                }
            }
            Mode::Controls => {
                let down = self.action_repeated(0, "left", delta);
                let up = self.action_repeated(1, "right", delta);
                if down + up > 0 {
                    let volume =
                        (self.save.volume as i32 + (up as i32 - down as i32) * 5).clamp(0, 100);
                    self.save.volume = volume as u8;
                    self.save.muted = false;
                    self.apply_volume();
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.touch.pressed("aim_mode") {
                    self.save.touch_aim = Some(self.touch_aim().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                } else if self.touch.pressed("arc_hint") {
                    self.save.arc_hint = Some(self.arc_hint().next());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(16, "last_arc") {
                    self.save.last_arc = Some(!self.last_arc_on());
                    self.settings.touch();
                    self.play(Sound::Ui);
                }
                if self.action_pressed(3, "replay") {
                    self.save.bindings.clear();
                    self.settings.touch();
                    self.note("DEFAULT BINDINGS RESTORED".into());
                } else if self.action_pressed(2, "back") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                }
            }
            Mode::Paused => {
                if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Playing;
                } else if self.action_pressed(3, "replay") {
                    self.play(Sound::Ui);
                    self.mode = Mode::Title;
                    self.suspend_run();
                }
            }
            Mode::SeedEntry => self.handle_seed_input(delta),
            Mode::GameOver => {
                if self.action_pressed(4, "copy") {
                    // Seed sharing: the seed is the whole run, so the
                    // clipboard is the whole share format.
                    let seed = format_seed(self.last_seed);
                    self.fx.push(Effect::Clipboard(seed.clone()));
                    self.note(format!("COPIED {seed}"));
                } else if self.action_pressed(0, "confirm") || self.action_pressed(2, "back") {
                    self.mode = Mode::Title;
                } else if self.action_pressed(3, "replay") {
                    let seed = self.last_seed;
                    let daily = self.daily;
                    self.start_run(seed, daily);
                }
            }
            Mode::Playing => self.handle_play_input(delta),
        }
    }

    /// Keyboard text comes in through `typed`. This is the touch keypad, and
    /// the controller moving a focus over the same keypad: A presses the
    /// focused key, B deletes, or leaves once nothing is typed.
    pub(super) fn handle_seed_input(&mut self, delta: f64) {
        let dx = self.action_repeated(1, "right", delta) as i32
            - self.action_repeated(0, "left", delta) as i32;
        let dy = self.action_repeated(2, "down", delta) as i32
            - self.action_repeated(3, "up", delta) as i32;
        if dx != 0 || dy != 0 {
            self.seed_focus.step(dx, dy);
        }
        let key = if self.pad_action_pressed("confirm") {
            Some(self.seed_focus.action())
        } else if self.pad_action_pressed("back") {
            Some(if self.seed_text.is_empty() {
                "back"
            } else {
                keypad::DEL
            })
        } else if self.touch.pressed("back") || self.back_requested {
            Some("back")
        } else {
            ["confirm", keypad::DEL]
                .into_iter()
                .chain(keypad::KEYS)
                .find(|k| self.touch.pressed(k))
        };
        match key {
            Some("confirm") => self.start_seeded_run(),
            Some("back") => self.leave_seed_entry(),
            Some(keypad::DEL) => {
                self.seed_text.pop();
            }
            Some(k) if self.seed_text.chars().count() < SEED_MAX => self.seed_text.push_str(k),
            _ => {}
        }
    }

    pub(super) fn handle_play_input(&mut self, delta: f64) {
        if self.action_pressed(2, "back") {
            self.touch.cancel_drag();
            self.mode = Mode::Paused;
            return;
        }
        let phase = self.run.as_ref().map(|r| r.phase);
        let Some(phase) = phase else { return };

        // A drag only aims on the screen it started on: drafting or a shot in
        // the air forgets it, so it can never drop into a later shot.
        if phase != Phase::Firing || self.anim.is_some() || self.clear_hold > 0.0 {
            self.touch.cancel_drag();
        }

        if phase == Phase::Drafting {
            for (i, action) in ["draft_1", "draft_2", "draft_3"].into_iter().enumerate() {
                if self.action_pressed(5 + i, action) {
                    let taken = self.run.as_mut().and_then(|r| r.take_offer(i));
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
        if self.anim.is_some() || self.clear_hold > 0.0 {
            return;
        }

        let quantum = self
            .run
            .as_ref()
            .map(|r| r.loadout.angle_quantum_ticks)
            .unwrap_or(5)
            .max(1);
        let mult = if self.shift_held() { 10 } else { 1 };

        let left = self.action_repeated(0, "left", delta) as i32;
        let right = self.action_repeated(1, "right", delta) as i32;
        if left + right != 0 {
            self.learn(LEARN_ANGLE);
            self.angle_ticks = (self.angle_ticks + (right - left) * quantum * mult)
                .clamp(0, dmath::MAX_ANGLE_TICKS);
        }
        let down = self.action_repeated(2, "down", delta) as i32;
        let up = self.action_repeated(3, "up", delta) as i32;
        if up + down != 0 {
            self.learn(LEARN_POWER);
            self.power_ticks =
                (self.power_ticks + (up - down) * 5 * mult).clamp(0, POWER_TICKS_MAX);
        }

        // Airburst fuse.
        let fuse_step = 4.0 * mult as f64;
        let minus = self.action_repeated(4, "fuse_down", delta) as f64;
        let plus = self.action_repeated(5, "fuse_up", delta) as f64;
        if minus + plus > 0.0 {
            if let Some(r) = self.run.as_mut() {
                if r.loadout.airburst() {
                    let f = r.fuse + (plus - minus) * fuse_step;
                    r.set_fuse(f);
                }
            }
        }

        // Enfilade reposition.
        let l = self.action_pressed(8, "reposition_left");
        let rr = self.action_pressed(9, "reposition_right");
        if l || rr {
            let step = if l { -5.0 } else { 5.0 };
            let ok = self.run.as_mut().map(|r| {
                let target = r.emplacement_offset + step;
                r.reposition(target)
            });
            if ok == Some(false) {
                self.note("NO ENFILADE".into());
            }
        }

        if self.action_pressed(10, "fire") || self.pressed(Key::Enter) {
            self.fire();
        }

        if self.drag_aiming() {
            self.handle_drag_aim(quantum);
        }
    }

    /// Pull back to aim, let go to fire. The aim follows the finger live, so
    /// the readouts and barrel show exactly what a release would fire; pulling
    /// back inside the dead zone puts the old aim back and cancels.
    pub(super) fn handle_drag_aim(&mut self, quantum: i32) {
        let dropped = self.touch.dropped();
        let Some((start, end)) = dropped.or(self.touch.drag()) else {
            // Abandoned (paused, focus lost): put the old aim back.
            if let Some(before) = self.drag_from.take() {
                (self.angle_ticks, self.power_ticks) = before;
            }
            return;
        };
        let before = *self
            .drag_from
            .get_or_insert((self.angle_ticks, self.power_ticks));
        let aim = crate::aim::pull_to_aim(start, end, quantum);
        match aim {
            Some((angle, power)) => {
                self.learn(LEARN_ANGLE);
                self.learn(LEARN_POWER);
                (self.angle_ticks, self.power_ticks) = (angle, power);
            }
            None => (self.angle_ticks, self.power_ticks) = before,
        }
        if dropped.is_some() {
            self.drag_from = None;
            if aim.is_some() {
                self.fire();
            }
        }
    }

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

    pub fn open_seed_entry(&mut self) {
        self.seed_text.clear();
        self.seed_focus = Focus::default();
        self.mode = Mode::SeedEntry;
    }
}

use super::*;
use salt_line_sim::ballistics::Shot;

pub(crate) struct TrialState {
    pub(crate) seed: u64,
    pub(crate) simulation: salt_line_sim::trial::Trial,
    pub(crate) paused: bool,
    accumulator: f64,
    commands: Vec<salt_line_sim::trial::FireCommand>,
    pub(crate) angle_ticks: i32,
    pub(crate) power_ticks: i32,
}

impl TrialState {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            simulation: salt_line_sim::trial::Trial::new(seed),
            paused: false,
            accumulator: 0.0,
            commands: Vec::new(),
            angle_ticks: 900,
            power_ticks: 600,
        }
    }
}

impl Game {
    pub(crate) fn start_trial(&mut self, seed: u64) {
        self.trial = Some(TrialState::new(seed));
        self.mode = Mode::Trial;
        self.apply_buffer_size();
        self.fx.push(Effect::Redraw);
    }

    pub(super) fn handle_trial_input(&mut self, delta: f64) {
        if self.action_pressed(Action::Back) {
            let finished = self.trial.as_ref().is_some_and(|t| t.simulation.finished());
            if self.trial.as_ref().is_some_and(|t| t.paused) || finished {
                self.trial = None;
                self.mode = Mode::Title;
                self.apply_buffer_size();
            } else if let Some(trial) = self.trial.as_mut() {
                trial.paused = true;
            }
            return;
        }
        if self.action_pressed(Action::Confirm) {
            let finished = self.trial.as_ref().is_some_and(|t| t.simulation.finished());
            if finished {
                let seed = self.trial.as_ref().unwrap().seed;
                self.start_trial(seed);
                return;
            }
            if let Some(trial) = self.trial.as_mut() {
                trial.paused = false;
            }
        }
        let paused = self.trial.as_ref().is_none_or(|t| t.paused || t.simulation.finished());
        if paused {
            return;
        }
        let left = self.action_repeated(0, Action::Left, delta) as i32;
        let right = self.action_repeated(1, Action::Right, delta) as i32;
        let down = self.action_repeated(2, Action::Down, delta) as i32;
        let up = self.action_repeated(3, Action::Up, delta) as i32;
        let multiplier = if self.shift_held() { 10 } else { 1 };
        let trial = self.trial.as_mut().unwrap();
        trial.angle_ticks = (trial.angle_ticks + (right - left) * 10 * multiplier).clamp(0, 1800);
        trial.power_ticks = (trial.power_ticks + (up - down) * 10 * multiplier).clamp(0, 1000);
        if self.action_pressed(Action::Fire) {
            self.fire_trial();
        }
    }

    fn fire_trial(&mut self) {
        let trial = self.trial.as_mut().unwrap();
        let shot = Shot::new(trial.angle_ticks, trial.power_ticks);
        trial.commands.push(salt_line_sim::trial::FireCommand {
            tick: trial.simulation.tick(),
            shot,
        });
        match trial.simulation.fire(shot) {
            salt_line_sim::trial::FireResult::Fired => self.play(Sound::Fire),
            salt_line_sim::trial::FireResult::Reloading => self.note("RELOADING".into()),
            salt_line_sim::trial::FireResult::Empty => self.note("OUT OF SHELLS".into()),
            salt_line_sim::trial::FireResult::Finished => {}
        }
    }

    pub(super) fn step_trial(&mut self, delta: f64) {
        let (hit, finished, replay) = {
            let Some(trial) = self.trial.as_mut() else {
                return;
            };
            if trial.paused || trial.simulation.finished() {
                return;
            }
            trial.accumulator += delta;
            let mut hit = false;
            let mut replay = None;
            let mut finished = false;
            for _ in 0..240 {
                if trial.accumulator < salt_line_sim::DT || trial.simulation.finished() {
                    break;
                }
                trial.simulation.step();
                trial.accumulator -= salt_line_sim::DT;
                let events = trial.simulation.take_events();
                hit |= events
                    .iter()
                    .any(|e| matches!(e, salt_line_sim::trial::Event::Hit { .. }));
                if events.contains(&salt_line_sim::trial::Event::Finished) {
                    finished = true;
                    let record = salt_line_sim::trial::Replay {
                        rules_version: salt_line_sim::trial::RULES_VERSION,
                        seed: trial.seed,
                        commands: trial.commands.clone(),
                    };
                    replay = record.encode().ok();
                    break;
                }
            }
            (hit, finished, replay)
        };
        if hit {
            self.play(Sound::Structure);
        }
        if finished {
            if let Some(text) = replay {
                self.fx.push(Effect::TrialReplay(text));
            } else {
                self.note("REPLAY COULD NOT BE SAVED".into());
            }
            self.note("TRIAL COMPLETE".into());
        }
    }
}

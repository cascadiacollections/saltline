//! A scripted player built on the sim's solver: for the golden-seed test and
//! the balance harness, not the game. It plays a plain, honest line: clear
//! the active era target by target, shift when it is clear, take the first
//! offer at every draft.

use salt_line_sim::solver::solve_target;

use crate::expedition::{Expedition, Input, State};

/// The bot's next input, or `None` when the run is over or it is stuck
/// (no standing target in either era has a solvable shot).
pub fn next_input(run: &Expedition) -> Option<Input> {
    match run.state() {
        State::Victory | State::Defeat => None,
        State::Drafting(_) => Some(Input::Pick(0)),
        State::Firing => {
            let stage = run.stage();
            let here = stage.active();
            if let Some(i) = aim(run) {
                return Some(i);
            }
            let there_standing = !stage.screen(here.other()).cleared();
            (there_standing && run.can_shift()).then_some(Input::Shift)
        }
    }
}

/// A shot at the first standing target in the active era that the solver
/// can hit, aimed as if the target were fresh so heavy targets (the bastion)
/// still get a direct-hit line.
fn aim(run: &Expedition) -> Option<Input> {
    let lo = run.active_loadout();
    let screen = run.stage().screen(run.stage().active());
    for (i, s) in screen.structures.iter().enumerate() {
        if s.is_down() {
            continue;
        }
        let mut probe = screen.clone();
        probe.structures[i].integrity = 1.0;
        if let Some(c) = solve_target(&probe, &lo, i).first() {
            return Some(Input::Fire {
                angle_ticks: c.shot.angle_ticks,
                power_ticks: c.shot.power_ticks,
            });
        }
    }
    None
}

/// Plays a run on `seed` to the end, or until the bot is stuck or `max_inputs`
/// is spent. Returns the run as it stands.
pub fn play(seed: u64, width: usize, max_inputs: usize) -> Expedition {
    let mut run = Expedition::new(seed, width);
    for _ in 0..max_inputs {
        let Some(input) = next_input(&run) else { break };
        if run.apply(input).is_none() {
            break;
        }
    }
    run
}

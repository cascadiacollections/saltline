//! Bundled one-gun challenges (DESIGN §13.12).
//!
//! Challenges are layouts rather than generated run screens: their ground and
//! target placement stay fixed across devices, while the normal `RunState`
//! still owns ballistics, damage and replayable shot resolution.

use crate::layout::Layout;
use crate::modifiers::Loadout;
use crate::run::Screen;
use crate::solver::{self, Candidate};
use crate::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub name: &'static str,
    pub seed: u64,
    pub screen: u32,
    pub shots: i32,
}

pub const ALL: [Challenge; 8] = [
    Challenge {
        name: "LOB",
        seed: 0x53414c54,
        screen: 2,
        shots: 3,
    },
    Challenge {
        name: "CHASM",
        seed: 0x53414c54,
        screen: 5,
        shots: 4,
    },
    Challenge {
        name: "WIND",
        seed: 0x53414c54,
        screen: 9,
        shots: 4,
    },
    Challenge {
        name: "PAIR",
        seed: 0x53414c54,
        screen: 13,
        shots: 5,
    },
    Challenge {
        name: "TIGHT",
        seed: 0x53414c54,
        screen: 15,
        shots: 5,
    },
    Challenge {
        name: "GALE",
        seed: 0x53414c54,
        screen: 31,
        shots: 5,
    },
    Challenge {
        name: "TRIO",
        seed: 0x53414c54,
        screen: 26,
        shots: 6,
    },
    Challenge {
        name: "STORM",
        seed: 0x53414c54,
        screen: 36,
        shots: 6,
    },
];

pub fn get(index: usize) -> Option<Challenge> {
    ALL.get(index).copied()
}

pub fn layout(challenge: Challenge) -> Layout {
    Layout::from_run_screen(challenge.seed, challenge.screen)
}

pub fn hints(challenge: Challenge) -> Vec<Candidate> {
    let layout = layout(challenge);
    let screen = Screen::from_layout(&layout, layout.wind.draw(challenge.seed));
    solver::solve_target(&screen, &Loadout::new(), 0)
}

/// The first solver hint flown on the untouched layout: the path of its shell,
/// for drawing as a ghost arc. Empty when the layout has no hint.
pub fn hint_path(challenge: Challenge) -> Vec<Vec2> {
    let layout = layout(challenge);
    let screen = Screen::from_layout(&layout, layout.wind.draw(challenge.seed));
    let lo = Loadout::new();
    // Only the first hint is drawn, so only the first is looked for, and it
    // is flown without being applied: the arc is the flight, not its damage.
    let Some(hint) = solver::first_solution(&screen, &lo, 0) else {
        return Vec::new();
    };
    screen
        .fire_traces(hint.shot, &lo, Some(0.0), true)
        .into_iter()
        .next()
        .map(|t| t.path)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::validate;

    #[test]
    fn bundled_challenges_are_playable_layouts() {
        for challenge in ALL {
            let layout = layout(challenge);
            assert_eq!(layout.guns.len(), 1);
            assert!(!layout.structures.is_empty());
            assert!(validate(&layout).is_ok(), "{}", challenge.name);
            assert!(!hints(challenge).is_empty(), "{}", challenge.name);
            assert!(hint_path(challenge).len() > 10, "{}", challenge.name);
        }
    }

    #[test]
    fn the_hint_arc_is_the_first_hint_as_apply_shot_would_fly_it() {
        // `hint_path` used to find every hint and apply the first to the
        // screen; it now finds one and only flies it. Same arc.
        for challenge in ALL {
            let layout = layout(challenge);
            let mut screen = Screen::from_layout(&layout, layout.wind.draw(challenge.seed));
            let first = hints(challenge)[0];
            let old = screen
                .apply_shot(first.shot, &Loadout::new(), Some(0.0), true)
                .traces
                .remove(0)
                .path;
            assert_eq!(hint_path(challenge), old, "{}", challenge.name);
        }
    }
}

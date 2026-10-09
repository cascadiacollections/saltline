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

pub const ALL: [Challenge; 3] = [
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
    let Some(hint) = hints(challenge).into_iter().next() else {
        return Vec::new();
    };
    let layout = layout(challenge);
    let mut screen = Screen::from_layout(&layout, layout.wind.draw(challenge.seed));
    let report = screen.apply_shot(hint.shot, &Loadout::new(), Some(0.0), true);
    report
        .traces
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
}

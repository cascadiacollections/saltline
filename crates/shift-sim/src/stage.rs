//! A stage: one screen, two phases.

use salt_line_sim::ballistics::Shot;
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::rng::substream;
use salt_line_sim::run::{generate_screen, Screen, ShotReport};

/// Which phase is real. Named `Plane` in code so it can't be confused with
/// `salt_line_sim::run::Phase`, the run state machine; the game calls it a
/// phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Plane {
    /// Pale, day.
    Salt,
    /// Deep, dusk.
    Brine,
}

impl Plane {
    /// The other phase.
    pub fn other(self) -> Self {
        match self {
            Plane::Salt => Plane::Brine,
            Plane::Brine => Plane::Salt,
        }
    }

    fn index(self) -> usize {
        match self {
            Plane::Salt => 0,
            Plane::Brine => 1,
        }
    }

    /// Sub-stream tag naming this phase. ASCII "SALT" / "BRIN".
    fn tag(self) -> u64 {
        match self {
            Plane::Salt => 0x5341_4C54,
            Plane::Brine => 0x4252_494E,
        }
    }
}

/// One stage of a run: the same screen index in both phases.
#[derive(Clone, Debug)]
pub struct Stage {
    phases: [Screen; 2],
    active: Plane,
}

impl Stage {
    /// Generates both phases of stage `index` of the run on `run_seed`.
    ///
    /// Each phase runs the sim's full generator (and its solvability gate) on
    /// its own seed, so each phase on its own is clearable with `lo`.
    pub fn generate(run_seed: u64, index: u32, width: usize, lo: &Loadout) -> Self {
        let gen = |p: Plane| generate_screen(substream(run_seed, p.tag()), index, width, lo);
        Stage {
            phases: [gen(Plane::Salt), gen(Plane::Brine)],
            active: Plane::Salt,
        }
    }

    /// The phase shells currently land in.
    pub fn active(&self) -> Plane {
        self.active
    }

    /// Swaps the active phase. SHIFT costs nothing (DESIGN: balance).
    pub fn shift(&mut self) {
        self.active = self.active.other();
    }

    /// One phase's screen.
    pub fn screen(&self, p: Plane) -> &Screen {
        &self.phases[p.index()]
    }

    /// Fires into the active phase only. The inactive phase is not touched.
    pub fn fire(&mut self, shot: Shot, lo: &Loadout, record_path: bool) -> ShotReport {
        let i = self.active.index();
        self.phases[i].apply_shot(shot, lo, None, record_path)
    }

    /// A stage is clear only when both phases are.
    pub fn cleared(&self) -> bool {
        self.phases.iter().all(Screen::cleared)
    }
}

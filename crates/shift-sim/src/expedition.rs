//! Expedition: a fixed run of [`STAGES`] stages ending at the bastion.

use salt_line_sim::modifiers::{Loadout, Modifier};
use salt_line_sim::replay::{hash_screen, Hasher};
use salt_line_sim::rng::{substream, Pcg32};
use salt_line_sim::run::{refund, shot};

use crate::stage::{Plane, Stage, StageShot};

/// Stages in an Expedition. The last is the bastion.
pub const STAGES: u32 = 12;
/// Shells at the start of a run.
pub const START_SHELLS: i32 = 30;
/// A draft follows clearing these stages (1-based screen numbers 4 and 8).
pub const DRAFT_AFTER: [u32; 2] = [4, 8];
/// Offers per draft.
pub const OFFERS: usize = 3;
/// Sub-stream tag for draft offers: ASCII "DRFT".
const TAG_DRAFT: u64 = 0x4452_4654;

/// The two modifiers that bend the eras rather than the gun.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum PhaseMod {
    /// Shots land in both eras, the inactive one at reduced effect. Every SHIFT
    /// costs a shell: dangerous to both worlds, safe in neither.
    HalfPhased,
    /// Each SHIFT strips integrity from the live echoes in the era shifted into.
    EchoDecay,
}

/// One entry of the draft pool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    /// A stock salt-line-sim modifier, folded into the loadout.
    Gun(Modifier),
    /// An era modifier, held by the run.
    Phase(PhaseMod),
}

impl Pick {
    /// Title for the draft card.
    pub fn name(self) -> &'static str {
        match self {
            Pick::Gun(m) => m.entry().name,
            Pick::Phase(PhaseMod::HalfPhased) => "HALF-PHASED PLATING",
            Pick::Phase(PhaseMod::EchoDecay) => "ECHO DECAY",
        }
    }

    /// One line for the draft card.
    pub fn blurb(self) -> &'static str {
        match self {
            Pick::Gun(m) => m.entry().blurb,
            Pick::Phase(PhaseMod::HalfPhased) => {
                "Shots strike both eras, the other at 55%. Every SHIFT costs a shell."
            }
            Pick::Phase(PhaseMod::EchoDecay) => {
                "Each SHIFT weakens the echoes waiting in the era you enter."
            }
        }
    }
}

/// The whole pool, kept tight (DESIGN: 6 entries).
pub const POOL: [Pick; 6] = [
    Pick::Gun(Modifier::Ballast),
    Pick::Gun(Modifier::Sabot),
    Pick::Gun(Modifier::Airburst),
    Pick::Gun(Modifier::Vernier),
    Pick::Phase(PhaseMod::HalfPhased),
    Pick::Phase(PhaseMod::EchoDecay),
];

/// A player input. A run is its seed, its width and the list of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// Swap eras.
    Shift,
    /// Fire into the active era.
    Fire { angle_ticks: i32, power_ticks: i32 },
    /// Take offer `n` of the current draft.
    Pick(u8),
    /// Decline the current draft.
    Skip,
}

/// Where the run is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// Aiming and firing on the current stage.
    Firing,
    /// Choosing one of these, or skipping.
    Drafting(Vec<Pick>),
    /// The bastion fell in both eras.
    Victory,
    /// Shells ran out with the stage standing.
    Defeat,
}

/// What an accepted input did, for the presentation layer.
#[derive(Clone, Debug)]
pub enum Event {
    Shifted { to: Plane, cost: i32 },
    Fired(Box<StageShot>),
    StageCleared { refund: i32 },
    Drafted(Option<Pick>),
    Victory,
    Defeat,
}

/// An Expedition in progress.
#[derive(Clone, Debug)]
pub struct Expedition {
    seed: u64,
    width: usize,
    base: Loadout,
    held: Vec<Pick>,
    stage_index: u32,
    stage: Stage,
    shells: i32,
    shots_on_stage: u32,
    total_shots: u32,
    state: State,
    log: Vec<Input>,
}

impl Expedition {
    /// A fresh run on `seed`, `width` world units wide.
    pub fn new(seed: u64, width: usize) -> Self {
        let base = Loadout::new();
        let stage = make_stage(seed, 0, width, &base);
        Expedition {
            seed,
            width,
            base,
            held: Vec::new(),
            stage_index: 0,
            stage,
            shells: START_SHELLS,
            shots_on_stage: 0,
            total_shots: 0,
            state: State::Firing,
            log: Vec::new(),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn stage(&self) -> &Stage {
        &self.stage
    }
    /// 0-based stage index; the HUD shows `stage_index() + 1` of [`STAGES`].
    pub fn stage_index(&self) -> u32 {
        self.stage_index
    }
    pub fn shells(&self) -> i32 {
        self.shells
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn log(&self) -> &[Input] {
        &self.log
    }
    pub fn held(&self) -> &[Pick] {
        &self.held
    }
    pub fn total_shots(&self) -> u32 {
        self.total_shots
    }
    /// The drafted loadout, before the active era's gun is applied.
    pub fn base_loadout(&self) -> &Loadout {
        &self.base
    }
    /// The loadout shots in the active era fly with.
    pub fn active_loadout(&self) -> Loadout {
        self.stage.active().loadout(&self.base)
    }
    pub fn holds(&self, m: PhaseMod) -> bool {
        self.held.contains(&Pick::Phase(m))
    }

    /// Shells a SHIFT costs right now.
    pub fn shift_cost(&self) -> i32 {
        i32::from(self.holds(PhaseMod::HalfPhased))
    }

    /// Whether SHIFT is accepted now. The presentation also locks it while a
    /// shot is animating; the sim resolves shots instantly.
    pub fn can_shift(&self) -> bool {
        self.state == State::Firing && self.shells > self.shift_cost()
    }

    /// Stages cleared so far.
    pub fn stages_cleared(&self) -> u32 {
        match self.state {
            State::Victory => STAGES,
            State::Drafting(_) => self.stage_index + 1,
            _ => self.stage_index,
        }
    }

    /// DESIGN score: stages × 1000 + shells × 100 + time bonus. Time is the
    /// presentation's wall clock and never enters the sim's state.
    pub fn score(&self, seconds: u32) -> u32 {
        let time = 900u32.saturating_sub(seconds);
        self.stages_cleared() * 1000 + self.shells.max(0) as u32 * 100 + time
    }

    /// Applies one input. Returns what happened, or `None` (and logs nothing)
    /// if the input is not valid in the current state.
    pub fn apply(&mut self, input: Input) -> Option<Vec<Event>> {
        let events = match (&self.state, input) {
            (State::Firing, Input::Shift) => {
                if !self.can_shift() {
                    return None;
                }
                let cost = self.shift_cost();
                self.shells -= cost;
                self.stage.shift(self.holds(PhaseMod::EchoDecay));
                let mut ev = vec![Event::Shifted {
                    to: self.stage.active(),
                    cost,
                }];
                // Echo Decay can finish a stage on its own.
                self.after_change(&mut ev);
                ev
            }
            (
                State::Firing,
                Input::Fire {
                    angle_ticks,
                    power_ticks,
                },
            ) => {
                if self.shells <= 0 {
                    return None;
                }
                let lo = self.active_loadout();
                let s = shot(angle_ticks, power_ticks).quantized(&lo);
                self.shells -= 1;
                self.shots_on_stage += 1;
                self.total_shots += 1;
                let half = self.holds(PhaseMod::HalfPhased);
                let report = self.stage.fire(s, &self.base, half, true);
                let mut ev = vec![Event::Fired(Box::new(report))];
                self.after_change(&mut ev);
                ev
            }
            (State::Drafting(offers), Input::Pick(n)) => {
                let pick = *offers.get(n as usize)?;
                match pick {
                    Pick::Gun(m) => self.shells += self.base.take(m),
                    Pick::Phase(_) => self.held.push(pick),
                }
                self.next_stage();
                vec![Event::Drafted(Some(pick))]
            }
            (State::Drafting(_), Input::Skip) => {
                self.next_stage();
                vec![Event::Drafted(None)]
            }
            _ => return None,
        };
        self.log.push(input);
        Some(events)
    }

    /// Resolve a clear or a defeat after the board changed. Clear is checked
    /// first, so clearing on the last shell still pays its refund.
    fn after_change(&mut self, ev: &mut Vec<Event>) {
        if self.stage.cleared() {
            if self.stage_index + 1 == STAGES {
                self.state = State::Victory;
                ev.push(Event::Victory);
                return;
            }
            // Two eras: a stage fired cleanly takes about twice a screen's shots.
            let r = refund(self.shots_on_stage.div_ceil(2), &self.base).max(0);
            self.shells += r;
            ev.push(Event::StageCleared { refund: r });
            if DRAFT_AFTER.contains(&(self.stage_index + 1)) {
                self.state = State::Drafting(self.offers());
            } else {
                self.next_stage();
            }
        } else if self.shells <= 0 {
            self.state = State::Defeat;
            ev.push(Event::Defeat);
        }
    }

    /// Three distinct offers, drawn from the stage seed, skipping era
    /// modifiers already held.
    fn offers(&self) -> Vec<Pick> {
        let mut avail: Vec<Pick> = POOL
            .iter()
            .copied()
            .filter(|p| !(matches!(p, Pick::Phase(_)) && self.held.contains(p)))
            .collect();
        let mut rng = Pcg32::new(substream(
            self.seed,
            TAG_DRAFT ^ u64::from(self.stage_index),
        ));
        let mut out = Vec::new();
        while out.len() < OFFERS && !avail.is_empty() {
            let i = rng.range_u32(0, avail.len() as u32 - 1) as usize;
            out.push(avail.remove(i));
        }
        out
    }

    fn next_stage(&mut self) {
        self.stage_index += 1;
        self.stage = make_stage(self.seed, self.stage_index, self.width, &self.base);
        self.shots_on_stage = 0;
        self.state = State::Firing;
    }

    /// Hash of everything that decides the run's future. Equal hashes after
    /// equal inputs is the determinism contract.
    pub fn state_hash(&self) -> u64 {
        let mut h = Hasher::new();
        for p in Plane::BOTH {
            hash_screen(&mut h, self.stage.screen(p));
        }
        h.write_u64(self.stage_index as u64);
        h.write_u64(self.shells as u64);
        h.write_u64(self.stage.active().index() as u64);
        h.write_u64(self.held.len() as u64);
        h.finish()
    }
}

fn make_stage(seed: u64, index: u32, width: usize, base: &Loadout) -> Stage {
    let s = Stage::generate(seed, index, width, base);
    if index + 1 == STAGES {
        s.into_bastion()
    } else {
        s
    }
}

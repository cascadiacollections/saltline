//! A stage: one screen in two eras, with echoes between them.

use salt_line_sim::ballistics::Shot;
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::rng::substream;
use salt_line_sim::run::{
    generate_screen, Screen, ShotReport, StructState, Structure, StructureKind,
};

/// Which era is real. Named `Plane` in code so it can't be confused with
/// `salt_line_sim::run::Phase`, the run state machine. The game calls it a
/// phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Plane {
    /// The present: a howitzer on the salt flats. Pale, day.
    Present,
    /// 1347: a trebuchet before a castle. Deep, dusk.
    Siege,
}

impl Plane {
    /// Both eras, in index order.
    pub const BOTH: [Plane; 2] = [Plane::Present, Plane::Siege];

    /// The other era.
    pub fn other(self) -> Self {
        match self {
            Plane::Present => Plane::Siege,
            Plane::Siege => Plane::Present,
        }
    }

    /// Index into per-era arrays.
    pub fn index(self) -> usize {
        match self {
            Plane::Present => 0,
            Plane::Siege => 1,
        }
    }

    /// Sub-stream tag naming this era: ASCII "PRES" and "SIEG".
    fn tag(self) -> u64 {
        match self {
            Plane::Present => 0x5052_4553,
            Plane::Siege => 0x5349_4547,
        }
    }

    /// The gun this era fires, applied over the drafted loadout.
    ///
    /// A trebuchet throws a heavy stone: slower off the arm, so a shorter
    /// range and a longer flight, barely pushed by wind, and digging a bigger
    /// hole where it lands. Applied after `Loadout::recompute`, so drafted
    /// modifiers stack under it. Never fed back into the loadout itself.
    pub fn loadout(self, base: &Loadout) -> Loadout {
        let mut lo = base.clone();
        if self == Plane::Siege {
            lo.muzzle_scale *= TREBUCHET_MUZZLE;
            lo.wind_scale *= TREBUCHET_WIND;
            lo.crater_scale *= TREBUCHET_CRATER;
            lo.blast_scale *= TREBUCHET_BLAST;
        }
        lo
    }
}

/// Trebuchet stone speed relative to a shell.
pub const TREBUCHET_MUZZLE: f64 = 0.85;
/// Wind's effect on a stone relative to a shell.
pub const TREBUCHET_WIND: f64 = 0.35;
/// Crater size of a stone relative to a shell.
pub const TREBUCHET_CRATER: f64 = 1.5;
/// Blast reach of a stone relative to a shell.
pub const TREBUCHET_BLAST: f64 = 1.3;

/// Stage `i` is generated as the sim's screen `i * DIFFICULTY_STRIDE`, so a
/// 12-stage Expedition climbs the sim's difficulty bands (four screens each)
/// to band 5: wind, distance and up to three targets, not a calm band 0-2.
pub const DIFFICULTY_STRIDE: u32 = 2;

/// An echo's starting integrity, as a share of a fresh structure's 1.0.
/// DESIGN: 40–50%.
pub const ECHO_INTEGRITY: f64 = 0.45;
/// The bastion's integrity in each era: about three direct hits.
pub const BASTION_INTEGRITY: f64 = 2.4;
/// Half-Phased Plating: share of effect a shot has in the inactive era.
pub const HALF_PHASED_EFFECT: f64 = 0.55;
/// Echo Decay: integrity stripped from each live echo in the era you shift into.
pub const ECHO_DECAY: f64 = 0.10;

/// What a structure is to the run, beyond what the sim tracks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Role {
    /// Generated with the screen. Leaves an echo in the other era when it falls.
    Original,
    /// Left by a structure that fell in the other era. Leaves nothing.
    Echo,
    /// The final stage's heavy structure, anchored in both eras. Leaves nothing.
    Bastion,
}

/// One era of a stage: the sim's screen plus the role of each structure,
/// index for index.
#[derive(Clone, Debug)]
pub struct Era {
    pub screen: Screen,
    pub roles: Vec<Role>,
}

impl Era {
    /// Live echoes in this era.
    pub fn echoes_left(&self) -> usize {
        self.live(Role::Echo)
    }

    fn live(&self, role: Role) -> usize {
        self.screen
            .structures
            .iter()
            .zip(&self.roles)
            .filter(|(s, r)| **r == role && !s.is_down())
            .count()
    }
}

/// What one shot did, in each era it touched.
#[derive(Clone, Debug)]
pub struct StageShot {
    /// The era the shot was aimed into, and the sim's report for it.
    pub active: ShotReport,
    /// The other era's report, when Half-Phased Plating carried the shot there.
    pub phased: Option<ShotReport>,
    /// Echoes this shot created, as (era they appeared in, structure index).
    pub echoes: Vec<(Plane, usize)>,
}

/// One stage of a run: the same screen index in both eras.
#[derive(Clone, Debug)]
pub struct Stage {
    eras: [Era; 2],
    active: Plane,
}

impl Stage {
    /// Generates both eras of stage `index` of the run on `run_seed`.
    ///
    /// Each era runs the sim's full generator and solvability gate on its own
    /// seed with its own gun, so each era on its own is clearable with `base`
    /// as that era fires it.
    pub fn generate(run_seed: u64, index: u32, width: usize, base: &Loadout) -> Self {
        let gen = |p: Plane| {
            let screen = generate_screen(
                substream(run_seed, p.tag()),
                index * DIFFICULTY_STRIDE,
                width,
                &p.loadout(base),
            );
            let roles = vec![Role::Original; screen.structures.len()];
            Era { screen, roles }
        };
        Stage {
            eras: [gen(Plane::Present), gen(Plane::Siege)],
            active: Plane::Present,
        }
    }

    /// Turns this stage into the bastion stage: each era keeps only its
    /// farthest structure, as a bastion with [`BASTION_INTEGRITY`]. Farthest
    /// because the generator has already proved that range reachable.
    pub fn into_bastion(mut self) -> Self {
        for era in &mut self.eras {
            let far = era
                .screen
                .structures
                .iter()
                .max_by(|a, b| a.centre_x().total_cmp(&b.centre_x()))
                .cloned();
            if let Some(far) = far {
                // Always the heaviest silhouette (silo / keep), re-seated on
                // the same centre.
                let mut s = reseat(&era.screen, &far, StructureKind::Silo);
                s.integrity = BASTION_INTEGRITY;
                era.screen.structures = vec![s];
                era.roles = vec![Role::Bastion];
            }
        }
        self
    }

    /// The era shots currently land in.
    pub fn active(&self) -> Plane {
        self.active
    }

    /// Swaps the active era. With `decay`, every live echo in the era shifted
    /// into loses [`ECHO_DECAY`] integrity first (the Echo Decay modifier).
    pub fn shift(&mut self, decay: bool) {
        self.active = self.active.other();
        if decay {
            let era = &mut self.eras[self.active.index()];
            for (s, r) in era.screen.structures.iter_mut().zip(&era.roles) {
                if *r == Role::Echo && !s.is_down() {
                    s.integrity -= ECHO_DECAY;
                }
            }
            era.screen.resolve_structures();
        }
    }

    /// One era.
    pub fn era(&self, p: Plane) -> &Era {
        &self.eras[p.index()]
    }

    /// One era's screen.
    pub fn screen(&self, p: Plane) -> &Screen {
        &self.eras[p.index()].screen
    }

    /// Fires into the active era with that era's gun. With `half_phased`, the
    /// same shot also lands in the other era at [`HALF_PHASED_EFFECT`].
    /// Structures that fall as originals leave echoes in the opposite era.
    pub fn fire(
        &mut self,
        shot: Shot,
        base: &Loadout,
        half_phased: bool,
        record_path: bool,
    ) -> StageShot {
        let here = self.active;
        let lo = here.loadout(base);
        let active = self.eras[here.index()]
            .screen
            .apply_shot(shot, &lo, None, record_path);
        let mut echoes = self.raise_echoes(here, &active.collapsed);

        let phased = if half_phased {
            let there = here.other();
            let report = self.fire_weakened(there, shot, &lo, record_path);
            echoes.extend(self.raise_echoes(there, &report.collapsed));
            Some(report)
        } else {
            None
        };
        StageShot {
            active,
            phased,
            echoes,
        }
    }

    /// The shot as it lands in the inactive era: smaller craters, and only
    /// [`HALF_PHASED_EFFECT`] of the structural damage it did.
    fn fire_weakened(&mut self, p: Plane, shot: Shot, lo: &Loadout, record: bool) -> ShotReport {
        let mut weak = lo.clone();
        weak.crater_scale *= HALF_PHASED_EFFECT;
        let screen = &mut self.eras[p.index()].screen;
        let before: Vec<Structure> = screen.structures.clone();
        let mut report = screen.apply_shot(shot, &weak, None, record);

        // Undo the part of each structure's damage above the reduced share,
        // and let the sim re-derive its state from that. A structure already
        // down before the shot keeps whatever the shot did to its wreck.
        for (s, b) in screen.structures.iter_mut().zip(&before) {
            if b.is_down() {
                continue;
            }
            let lost = b.integrity - s.integrity;
            s.integrity = b.integrity - lost * HALF_PHASED_EFFECT;
            s.state = b.state;
            s.lean = b.lean;
        }
        screen.resolve_structures();
        report.collapsed = screen
            .structures
            .iter()
            .zip(&before)
            .enumerate()
            .filter(|(_, (s, b))| s.is_down() && !b.is_down())
            .map(|(i, _)| i)
            .collect();
        report.cleared = screen.cleared();
        report
    }

    /// Echoes for every original in `fell` that collapsed in era `p`, placed
    /// in the other era at the same centre, seated on that era's ground.
    fn raise_echoes(&mut self, p: Plane, fell: &[usize]) -> Vec<(Plane, usize)> {
        let there = p.other();
        let mut out = Vec::new();
        for &i in fell {
            let (role, src) = {
                let era = &self.eras[p.index()];
                (era.roles[i], era.screen.structures[i].clone())
            };
            if role != Role::Original {
                continue;
            }
            let era = &mut self.eras[there.index()];
            let echo = seat_echo(&era.screen, &src);
            era.screen.structures.push(echo);
            era.roles.push(Role::Echo);
            out.push((there, era.roles.len() - 1));
        }
        out
    }

    /// A stage is clear only when every structure and echo is down in both eras.
    pub fn cleared(&self) -> bool {
        self.eras.iter().all(|e| e.screen.cleared())
    }

    /// Live echoes across both eras: the HUD's echo count.
    pub fn echoes_left(&self) -> usize {
        self.eras.iter().map(Era::echoes_left).sum()
    }

    /// Live bastions across both eras.
    pub fn bastions_left(&self) -> usize {
        self.eras.iter().map(|e| e.live(Role::Bastion)).sum()
    }
}

/// An intact echo of `src` on `screen`'s ground, at the same centre x, seated
/// on the highest column of its footprint as the sim seats every structure.
fn seat_echo(screen: &Screen, src: &Structure) -> Structure {
    let mut s = reseat(screen, src, src.kind);
    s.integrity = ECHO_INTEGRITY;
    s
}

/// An intact structure of `kind` centred where `src` stands, seated on
/// `screen`'s ground the way the sim seats every structure.
fn reseat(screen: &Screen, src: &Structure, kind: StructureKind) -> Structure {
    let fp = kind.footprint();
    let max_x0 = screen.width.saturating_sub(fp + 1).max(1);
    let want = (src.centre_x() - fp as f64 * 0.5).round().max(1.0) as usize;
    let x0 = want.clamp(1, max_x0);
    let field = &screen.field;
    let base = field.seat(x0, fp);
    let mean = (x0..x0 + fp).map(|i| field.surface(i)).sum::<f64>() / fp as f64;
    Structure {
        kind,
        x0,
        base,
        original_mean: mean,
        integrity: 1.0,
        support: 1.0,
        state: StructState::Intact,
        lean: 0,
    }
}

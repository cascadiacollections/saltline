//! Drone Trial rules v1: fixed arena, direct hits, no terrain or modifiers.
//! Call `step` once per simulation tick; pause by not calling it.
use crate::{
    ballistics::{Aabb, Shot},
    modifiers::Loadout,
    motion::{advance_projectile, translating_box_contact},
    rng::Pcg32,
    Vec2, DT, SHELL_RADIUS,
};

pub const RULES_VERSION: u32 = 1;
pub const ROUND_TICKS: u32 = 30 * 120;
pub const RELOAD_TICKS: u32 = 60;
pub const START_AMMO: u32 = 8;
const SPAWN_TICKS: u32 = 180;
const MAX_DRONES: usize = 4;
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 180.0;
const ORIGIN: Vec2 = Vec2::new(30.0, 15.0);

#[derive(Clone, Debug, PartialEq)]
pub struct Drone {
    pub id: u32,
    pub position: Vec2,
    pub velocity: Vec2,
}
impl Drone {
    fn bounds(&self) -> Aabb {
        Aabb::new(
            self.position.x - 5.0,
            self.position.y - 3.0,
            self.position.x + 5.0,
            self.position.y + 3.0,
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub id: u32,
    pub position: Vec2,
    pub velocity: Vec2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireResult {
    Fired,
    Reloading,
    Empty,
    Finished,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Spawned(u32),
    Fired(u32),
    Hit { projectile: u32, drone: u32 },
    Escaped(u32),
    Missed(u32),
    Finished,
}
#[derive(Clone, Debug)]
pub struct Trial {
    tick: u32,
    ammo: u32,
    score: u32,
    ready_tick: u32,
    next_drone: u32,
    next_projectile: u32,
    wind: f64,
    rng: Pcg32,
    drones: Vec<Drone>,
    projectiles: Vec<Projectile>,
    events: Vec<Event>,
}
impl Trial {
    pub fn new(seed: u64) -> Self {
        let mut rng = Pcg32::new(seed);
        let wind = rng.signed(3.0);
        Self {
            tick: 0,
            ammo: START_AMMO,
            score: 0,
            ready_tick: 0,
            next_drone: 0,
            next_projectile: 0,
            wind,
            rng,
            drones: Vec::new(),
            projectiles: Vec::new(),
            events: Vec::new(),
        }
    }
    pub fn tick(&self) -> u32 {
        self.tick
    }
    pub fn ammo(&self) -> u32 {
        self.ammo
    }
    pub fn score(&self) -> u32 {
        self.score
    }
    pub fn wind(&self) -> f64 {
        self.wind
    }
    pub fn finished(&self) -> bool {
        self.tick >= ROUND_TICKS
    }
    pub fn drones(&self) -> &[Drone] {
        &self.drones
    }
    pub fn projectiles(&self) -> &[Projectile] {
        &self.projectiles
    }
    /// Drain presentation events regularly; events never influence simulation.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    /// Fire before stepping tick N. Rejected input has no state effect.
    pub fn fire(&mut self, shot: Shot) -> FireResult {
        if self.finished() {
            return FireResult::Finished;
        }
        if self.ammo == 0 {
            return FireResult::Empty;
        }
        if self.tick < self.ready_tick {
            return FireResult::Reloading;
        }
        let loadout = Loadout::default();
        let velocity = shot.quantized(&loadout).velocity(&loadout);
        let id = self.next_projectile;
        self.next_projectile += 1;
        self.projectiles.push(Projectile {
            id,
            position: ORIGIN,
            velocity,
        });
        self.events.push(Event::Fired(id));
        self.ammo -= 1;
        self.ready_tick = self.tick + RELOAD_TICKS;
        FireResult::Fired
    }
    pub fn step(&mut self) {
        if self.finished() {
            return;
        }
        if self.tick.is_multiple_of(SPAWN_TICKS) && self.drones.len() < MAX_DRONES {
            let id = self.next_drone;
            self.next_drone += 1;
            self.drones.push(Drone {
                id,
                position: Vec2::new(WIDTH + 5.0, self.rng.range_f64(55.0, 150.0)),
                velocity: Vec2::new(-self.rng.range_f64(25.0, 45.0), 0.0),
            });
            self.events.push(Event::Spawned(id));
        }
        // Resolve globally by contact time; IDs break exact ties reproducibly.
        let mut contacts = Vec::new();
        for p in &mut self.projectiles {
            let before = p.position;
            advance_projectile(&mut p.position, &mut p.velocity, self.wind);
            for d in &self.drones {
                let delta = Vec2::new(d.velocity.x * DT, d.velocity.y * DT);
                if let Some(t) =
                    translating_box_contact(before, p.position, d.bounds(), delta, SHELL_RADIUS)
                {
                    contacts.push((t, p.id, d.id));
                }
            }
        }
        contacts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut hit_shells = Vec::new();
        let mut hit_drones = Vec::new();
        for (_, projectile, drone) in contacts {
            if hit_shells.contains(&projectile) || hit_drones.contains(&drone) {
                continue;
            }
            hit_shells.push(projectile);
            hit_drones.push(drone);
            self.score += 25;
            self.events.push(Event::Hit { projectile, drone });
        }
        self.projectiles.retain(|p| {
            if hit_shells.contains(&p.id) {
                return false;
            }
            let alive = p.position.x >= 0.0
                && p.position.x <= WIDTH
                && p.position.y >= 0.0
                && p.position.y <= HEIGHT;
            if !alive {
                self.events.push(Event::Missed(p.id));
            }
            alive
        });
        self.drones.retain_mut(|d| {
            if hit_drones.contains(&d.id) {
                return false;
            }
            d.position.x += d.velocity.x * DT;
            d.position.y += d.velocity.y * DT;
            if d.position.x < -5.0 {
                self.events.push(Event::Escaped(d.id));
                return false;
            }
            true
        });
        self.tick += 1;
        if self.finished() {
            self.projectiles.clear();
            self.events.push(Event::Finished);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FireCommand {
    pub tick: u32,
    pub shot: Shot,
}
#[derive(Clone, Debug)]
pub struct Replay {
    pub rules_version: u32,
    pub seed: u64,
    pub commands: Vec<FireCommand>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    UnsupportedRules,
    InvalidTickOrder,
}
impl Replay {
    /// Commands in the same tick run in recorded order, including rejected fires.
    pub fn play(&self) -> Result<Trial, ReplayError> {
        if self.rules_version != RULES_VERSION {
            return Err(ReplayError::UnsupportedRules);
        }
        if self.commands.iter().any(|c| c.tick >= ROUND_TICKS)
            || self.commands.windows(2).any(|w| w[0].tick > w[1].tick)
        {
            return Err(ReplayError::InvalidTickOrder);
        }
        let mut trial = Trial::new(self.seed);
        let mut commands = self.commands.iter().peekable();
        while !trial.finished() {
            while commands.peek().is_some_and(|c| c.tick == trial.tick) {
                trial.fire(commands.next().unwrap().shot);
            }
            trial.step();
        }
        Ok(trial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reload_ammo_and_expiry_boundaries() {
        let mut t = Trial::new(1);
        for _ in 0..START_AMMO {
            assert_eq!(t.fire(Shot::new(900, 500)), FireResult::Fired);
            assert_eq!(
                t.fire(Shot::default()),
                if t.ammo == 0 {
                    FireResult::Empty
                } else {
                    FireResult::Reloading
                }
            );
            for _ in 0..RELOAD_TICKS {
                t.step();
            }
        }
        assert_eq!(t.fire(Shot::default()), FireResult::Empty);
        while t.tick < ROUND_TICKS - 1 {
            t.step();
        }
        assert!(!t.finished());
        t.step();
        assert!(t.finished());
        assert_eq!(t.fire(Shot::default()), FireResult::Finished);
        t.take_events();
        t.step();
        assert!(t.take_events().is_empty());
        assert_eq!(t.tick, ROUND_TICKS);
    }
    #[test]
    fn nearest_hit_wins_regardless_of_target_order() {
        let mut t = Trial::new(1);
        t.tick = 1;
        t.wind = 0.0;
        t.drones = vec![
            Drone {
                id: 0,
                position: Vec2::new(25.0, 10.0),
                velocity: Vec2::default(),
            },
            Drone {
                id: 1,
                position: Vec2::new(15.0, 10.0),
                velocity: Vec2::default(),
            },
        ];
        t.projectiles.push(Projectile {
            id: 0,
            position: Vec2::new(0.0, 10.0),
            velocity: Vec2::new(3600.0, 0.0),
        });
        t.step();
        assert_eq!(t.score(), 25);
        assert_eq!(t.drones()[0].id, 0);
        assert!(t.take_events().contains(&Event::Hit {
            projectile: 0,
            drone: 1
        }));
    }
    #[test]
    fn two_shells_cannot_score_the_same_drone_twice() {
        let mut t = Trial::new(1);
        t.tick = 1;
        t.wind = 0.0;
        t.drones = vec![Drone {
            id: 0,
            position: Vec2::new(15.0, 10.0),
            velocity: Vec2::default(),
        }];
        // ID 1 reaches the drone earlier despite appearing second in storage.
        t.projectiles = vec![
            Projectile {
                id: 0,
                position: Vec2::new(0.0, 10.0),
                velocity: Vec2::new(2400.0, 0.0),
            },
            Projectile {
                id: 1,
                position: Vec2::new(5.0, 10.0),
                velocity: Vec2::new(2400.0, 0.0),
            },
        ];
        t.step();
        assert_eq!(t.score(), 25);
        assert!(t.drones().is_empty());
        assert_eq!(t.projectiles()[0].id, 0);
        assert!(t.take_events().contains(&Event::Hit {
            projectile: 1,
            drone: 0
        }));
    }

    #[test]
    fn moving_drone_crossing_shell_is_detected() {
        let mut t = Trial::new(1);
        t.tick = 1;
        t.wind = 0.0;
        t.drones = vec![Drone {
            id: 0,
            position: Vec2::new(0.0, 10.0),
            velocity: Vec2::new(2400.0, 0.0),
        }];
        t.projectiles = vec![Projectile {
            id: 0,
            position: Vec2::new(10.0, 10.0),
            velocity: Vec2::default(),
        }];
        t.step();
        assert_eq!(t.score(), 25);
        assert!(t.projectiles().is_empty());
        assert!(t.drones().is_empty());
    }

    #[test]
    fn live_execution_and_replay_agree() {
        let commands: Vec<_> = (0..START_AMMO)
            .map(|i| FireCommand {
                tick: i * 180,
                shot: Shot::new(900, 600),
            })
            .collect();
        let mut live = Trial::new(42);
        for tick in 0..ROUND_TICKS {
            for c in commands.iter().filter(|c| c.tick == tick) {
                live.fire(c.shot);
            }
            live.step();
            assert!(live.drones.len() <= MAX_DRONES);
            assert!(live.projectiles.len() <= START_AMMO as usize);
        }
        let mut replay = Replay {
            rules_version: RULES_VERSION,
            seed: 42,
            commands,
        }
        .play()
        .unwrap();
        assert_eq!(live.score, replay.score);
        assert_eq!(live.ammo, replay.ammo);
        assert_eq!(live.drones, replay.drones);
        assert_eq!(live.projectiles, replay.projectiles);
        assert_eq!(live.take_events(), replay.take_events());
        assert_eq!(live.rng.next_u32(), replay.rng.next_u32());
    }
    #[test]
    fn replay_rejects_unknown_rules_and_bad_ticks() {
        let mut r = Replay {
            rules_version: 99,
            seed: 1,
            commands: vec![],
        };
        assert!(matches!(r.play(), Err(ReplayError::UnsupportedRules)));
        r.rules_version = RULES_VERSION;
        r.commands.push(FireCommand {
            tick: ROUND_TICKS,
            shot: Shot::default(),
        });
        assert!(matches!(r.play(), Err(ReplayError::InvalidTickOrder)));
    }
}

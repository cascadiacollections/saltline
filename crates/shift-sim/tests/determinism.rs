//! Same seed, same stage; phases independent (gameoff/DESIGN.md, tests).

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::{hash_screen, Hasher};
use salt_line_sim::run::{shot, Screen};
use salt_line_sim::SCREEN_W_REF;
use shift_sim::{Plane, Stage};

fn digest(s: &Screen) -> u64 {
    let mut h = Hasher::new();
    hash_screen(&mut h, s);
    h.finish()
}

fn stage(seed: u64, index: u32) -> Stage {
    Stage::generate(seed, index, SCREEN_W_REF, &Loadout::new())
}

#[test]
fn same_seed_gives_identical_phases() {
    for seed in [1u64, 0xDEAD_BEEF, 42_424_242] {
        for index in 0..4 {
            let (a, b) = (stage(seed, index), stage(seed, index));
            for p in [Plane::Salt, Plane::Brine] {
                assert_eq!(
                    digest(a.screen(p)),
                    digest(b.screen(p)),
                    "seed {seed} stage {index} {p:?}"
                );
            }
        }
    }
}

#[test]
fn phases_differ() {
    let s = stage(7, 0);
    assert_ne!(
        digest(s.screen(Plane::Salt)),
        digest(s.screen(Plane::Brine))
    );
}

#[test]
fn shift_swaps_and_returns() {
    let mut s = stage(7, 0);
    assert_eq!(s.active(), Plane::Salt);
    s.shift();
    assert_eq!(s.active(), Plane::Brine);
    s.shift();
    assert_eq!(s.active(), Plane::Salt);
}

#[test]
fn damage_stays_in_the_active_phase() {
    let mut s = stage(99, 1);
    let brine_before = digest(s.screen(Plane::Brine));
    let salt_before = digest(s.screen(Plane::Salt));
    // A spread of shots; some land on terrain, which always craters.
    for (a, p) in [(900, 400), (700, 600), (1100, 300)] {
        s.fire(shot(a, p), &Loadout::new(), false);
    }
    assert_ne!(
        digest(s.screen(Plane::Salt)),
        salt_before,
        "shots should have left a mark in Salt"
    );
    assert_eq!(
        digest(s.screen(Plane::Brine)),
        brine_before,
        "Brine must not change"
    );
}

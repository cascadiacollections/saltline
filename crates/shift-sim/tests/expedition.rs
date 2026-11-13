//! Echoes, the bastion, the log round-trip and the golden-seed Expedition.

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::SCREEN_W_REF;
use shift_sim::expedition::{STAGES, START_SHELLS};
use shift_sim::log::{encode, replay, LogError};
use shift_sim::stage::{BASTION_INTEGRITY, ECHO_INTEGRITY};
use shift_sim::{bot, Event, Expedition, Input, Plane, Role, Stage, State};

const W: usize = SCREEN_W_REF;

/// Fire the bot at the active era until something falls; return the events.
fn fell_something(run: &mut Expedition) -> Vec<(Plane, usize)> {
    for _ in 0..12 {
        let Some(Input::Fire {
            angle_ticks,
            power_ticks,
        }) = bot::next_input(run)
        else {
            panic!("bot had no shot");
        };
        let ev = run
            .apply(Input::Fire {
                angle_ticks,
                power_ticks,
            })
            .unwrap();
        for e in ev {
            if let Event::Fired(r) = e {
                if !r.echoes.is_empty() {
                    return r.echoes.clone();
                }
            }
        }
    }
    panic!("nothing fell in 12 aimed shots");
}

#[test]
fn a_fallen_original_leaves_an_echo_in_the_other_era() {
    let mut run = Expedition::new(11, W);
    let siege_before = run.stage().era(Plane::Siege).screen.structures.len();
    let echoes = fell_something(&mut run);
    let (era, i) = echoes[0];
    assert_eq!(era, Plane::Siege);
    let e = run.stage().era(Plane::Siege);
    assert_eq!(e.screen.structures.len(), siege_before + echoes.len());
    assert_eq!(e.roles[i], Role::Echo);
    assert!((e.screen.structures[i].integrity - ECHO_INTEGRITY).abs() < 1e-12);
    assert_eq!(run.stage().echoes_left(), echoes.len());
}

#[test]
fn echoes_do_not_echo() {
    let mut run = Expedition::new(5, W);
    fell_something(&mut run);
    // Now in Siege, kill an echo and check the Present gains nothing new.
    run.apply(Input::Shift).unwrap();
    let salt_len = run.stage().screen(Plane::Present).structures.len();
    for _ in 0..20 {
        let Some(input) = bot::next_input(&run) else {
            break;
        };
        if input == Input::Shift {
            break;
        }
        run.apply(input);
    }
    let salt = run.stage().era(Plane::Present);
    let new_from_echoes = salt.roles[salt_len..]
        .iter()
        .filter(|r| **r == Role::Echo)
        .count();
    let siege = run.stage().era(Plane::Siege);
    let originals_felled_in_siege = siege
        .screen
        .structures
        .iter()
        .zip(&siege.roles)
        .filter(|(s, r)| **r == Role::Original && s.is_down())
        .count();
    assert_eq!(new_from_echoes, originals_felled_in_siege);
}

#[test]
fn bastion_stage_is_one_heavy_structure_per_era() {
    let s = Stage::generate(77, STAGES - 1, W, &Loadout::new()).into_bastion();
    for p in Plane::BOTH {
        let e = s.era(p);
        assert_eq!(e.roles, vec![Role::Bastion]);
        assert_eq!(e.screen.structures[0].integrity, BASTION_INTEGRITY);
    }
    assert_eq!(s.bastions_left(), 2);
}

#[test]
fn trebuchet_flies_differently_from_the_howitzer() {
    let base = Loadout::new();
    let (salt, siege) = (Plane::Present.loadout(&base), Plane::Siege.loadout(&base));
    assert!(siege.muzzle_scale < salt.muzzle_scale);
    assert!(siege.wind_scale < salt.wind_scale);
    assert!(siege.crater_scale > salt.crater_scale);
}

#[test]
fn invalid_inputs_are_refused_and_not_logged() {
    let mut run = Expedition::new(3, W);
    assert!(run.apply(Input::Pick(0)).is_none());
    assert!(run.apply(Input::Skip).is_none());
    assert!(run.log().is_empty());
    assert_eq!(run.shells(), START_SHELLS);
}

#[test]
fn log_round_trips_to_the_same_state() {
    let run = bot::play(0x5EED, W, 60);
    assert!(!run.log().is_empty());
    let text = encode(&run);
    let again = replay(&text).expect("replay");
    assert_eq!(again.state_hash(), run.state_hash());
    assert_eq!(again.log(), run.log());
    assert_eq!(encode(&again), text);
}

#[test]
fn log_refuses_other_versions_and_garbage() {
    assert_eq!(
        replay("SHIFT999:1:320:").unwrap_err(),
        LogError::Version(999)
    );
    assert_eq!(replay("nope").unwrap_err(), LogError::Malformed);
    assert_eq!(
        replay("SHIFT1:1:320:p0").unwrap_err(),
        LogError::Rejected(0)
    );
}

/// The golden seed: the bot, playing straight through on the real economy,
/// reaches the bastion and fells it in both eras.
#[test]
fn golden_seed_expedition_reaches_and_kills_the_bastion() {
    let run = bot::play(GOLDEN_SEED, W, 2_000);
    assert_eq!(
        *run.state(),
        State::Victory,
        "ended at stage {} with {} shells after {} shots",
        run.stage_index() + 1,
        run.shells(),
        run.total_shots()
    );
    // Same seed, same inputs, same run.
    let again = bot::play(GOLDEN_SEED, W, 2_000);
    assert_eq!(again.state_hash(), run.state_hash());
}

const GOLDEN_SEED: u64 = 0x5A17_11E5;

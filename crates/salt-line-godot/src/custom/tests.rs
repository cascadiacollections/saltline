//! Custom duels: forks of duel rounds, the duel gate on a layout, a match
//! on one, and the dev flag that starts it.

use super::*;
use pretty_assertions::assert_eq;
use salt_line_sim::layout::{height_steps, height_units, LayoutName};
use salt_line_sim::terrain::BASE_ELEVATION;

use crate::app::{Game, Mode};
use crate::cli::DevArgs;
use crate::save_format::Save;
use crate::versus::{fly, hits, Phase};

/// Level ground with the guns where versus stands them, under the game's
/// wind.
fn level() -> Layout {
    Layout {
        name: LayoutName::parse("FLAT").unwrap(),
        heights: vec![height_steps(BASE_ELEVATION); LAYOUT_WIDTH],
        structures: Vec::new(),
        guns: vec![30, 289],
        wind: WindSpec::Range { min: 0, max: 2500 },
    }
}

/// The right gun at the bottom of a pit walled to the top of the world:
/// nothing gets out, and nothing gets in.
fn pit() -> Layout {
    let mut l = level();
    for c in 288 - 19..288 + 20 {
        let floor = (276..303).contains(&c);
        l.heights[c] = height_steps(if floor { 40.0 } else { 108.0 });
    }
    l
}

// ----------------------------------------------------------------- forks

/// A forked duel round is the generated one to within a rounding of its
/// ground: the guns where versus stands them, the game's wind exactly, no
/// structures, every height within half a step. It passes the duel gate,
/// is a fixed point of forking, and its code reads back as itself.
#[test]
fn a_forked_duel_is_the_generated_one_to_within_a_rounding() {
    let lo = Loadout::new();
    for seed in [0u64, 5, 42, 0xC0FFEE] {
        for round in 0..3 {
            let gen = versus::generate_duel(seed, round, LAYOUT_WIDTH, &lo, &Rules::DEFAULT);
            let l = fork_duel(seed, round);
            assert_eq!(l.guns, vec![30, 289]);
            assert_eq!(l.wind, WindSpec::Range { min: 0, max: 2500 });
            assert!(l.structures.is_empty());
            for (a, &b) in gen.field.base.iter().zip(&l.heights) {
                assert!((a - height_units(b)).abs() <= 1.0 / 32.0);
            }
            assert_eq!(validate(&l), Ok(()), "seed {seed} round {round}");
            let fork = Screen::from_layout(&l, 0.0);
            assert_eq!(Layout::from_screen(&fork, l.guns.clone(), l.wind), l);
            assert_eq!(Layout::from_code(&l.to_code().unwrap()), Ok(l));
        }
    }
}

/// The shots the gate found on a generated duel, fired at its fork, hit or
/// miss as they did (NOTES §9 measures every shot ending the same 99.3% of
/// the time; the differences are grazing shots).
#[test]
fn a_forked_duel_plays_like_the_generated_one() {
    let lo = Loadout::new();
    let reach = Rules::DEFAULT.hit_reach;
    let (mut same, mut total) = (0, 0);
    for seed in 0..6u64 {
        let mut gen = versus::generate_duel(seed, 0, LAYOUT_WIDTH, &lo, &Rules::DEFAULT);
        let mut fork = Screen::from_layout(&fork_duel(seed, 0), 0.0);
        for wind in [-2.5, 0.0, 2.5] {
            (gen.wind, fork.wind) = (wind, wind);
            for side in Side::BOTH {
                let (fg, ff) = (frame_for(&gen, side), frame_for(&fork, side));
                let (tg, tf) = (target_in_frame(&gen, side), target_in_frame(&fork, side));
                for shot in shots_at(&fg, tg, &lo, reach, GATE_CAP) {
                    let a = hits(&fly(&fg, tg, shot, &lo, false)[0], tg, &lo, reach);
                    let b = hits(&fly(&ff, tf, shot, &lo, false)[0], tf, &lo, reach);
                    total += 1;
                    same += (a == b) as usize;
                }
            }
        }
    }
    assert!(same * 100 >= total * 98, "{same}/{total}");
}

// -------------------------------------------------------------- the gate

#[test]
fn a_duel_layout_must_pass_the_duel_gate_for_both_guns() {
    assert_eq!(validate(&level()), Ok(()));
    let counts = duel_counts(&pit());
    assert_eq!(counts.len(), 3, "both extremes and calm");
    let Err(problems) = validate(&pit()) else {
        panic!("a gun in a pit passes");
    };
    for gun in [0, 1] {
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::Unsolvable { gun: g, .. } if *g == gun)),
            "{problems:?}"
        );
    }
}

#[test]
fn placement_comes_before_the_gate() {
    // Still in the pit, one column off the mirror.
    let mirrored = Layout {
        guns: vec![30, 288],
        ..pit()
    };
    assert_eq!(validate(&mirrored), Err(vec![Problem::GunsNotMirrored]));
    let one = Layout {
        guns: vec![30],
        ..level()
    };
    assert_eq!(
        validate(&one),
        Err(vec![Problem::NoTargets]),
        "the sim's rules"
    );
}

#[test]
fn symmetric_ground_is_fair_in_every_wind() {
    let mut l = fork_duel(9, 1);
    l.mirror_left();
    for (wind, [left, right]) in duel_counts(&l) {
        let mirrored = duel_counts(&l)
            .into_iter()
            .find(|(w, _)| *w == -wind)
            .expect("the range is checked both ways")
            .1;
        // Mirroring is exact, so the left gun in a wind is the right gun in
        // the opposite wind, shot for shot.
        assert_eq!([left, right], [mirrored[1], mirrored[0]], "wind {wind}");
    }
}

// ------------------------------------------------------------- the match

#[test]
fn the_wind_maps_onto_the_rules() {
    let base = Rules::DEFAULT;
    let r = duel_rules(&level(), base);
    assert_eq!((r.wind_min, r.wind_max), (0.0, 2.5));
    assert_eq!(r.gun_shift, 0.0, "a layout's guns stay put");
    assert_eq!(r.hit_reach, base.hit_reach, "the other levers are kept");
    let calm = Layout {
        wind: WindSpec::Calm,
        ..level()
    };
    assert_eq!(duel_rules(&calm, base).wind_max, 0.0);
    let fixed = Layout {
        wind: WindSpec::Fixed(-1200),
        ..level()
    };
    let r = duel_rules(&fixed, base);
    assert_eq!((r.wind_min, r.wind_max), (1.2, 1.2));
}

/// A match on a layout deals the layout every round, fresh, and draws its
/// winds from the layout's range, or holds its fixed wind every turn.
#[test]
fn a_match_on_a_layout_plays_every_round_on_it() {
    let l = fork_duel(3, 0);
    let ground = Screen::from_layout(&l, 0.0);
    let mut m = layout_match(&l, 77, Rules::DEFAULT);
    assert_eq!(m.screen.field.base, ground.field.base);
    assert_eq!(m.rules.gun_shift, 0.0, "no rolling on a layout");
    let mut winds = Vec::new();
    let lo = m.loadout.clone();
    // The left gun fires the gate's first shot at the right, which always
    // hits; the right drops a short one. Three hits up wins the round.
    for _ in 0..40 {
        if m.phase != Phase::Aiming {
            break;
        }
        winds.push(m.screen.wind);
        let side = m.to_move;
        let shot = match side {
            Side::Left => shots_at(
                &frame_for(&m.screen, side),
                target_in_frame(&m.screen, side),
                &lo,
                m.rules.hit_reach,
                1,
            )[0],
            Side::Right => salt_line_sim::run::shot(900, 100),
        };
        let report = m.fire(shot, false).expect("aiming");
        assert!(report.rolled.is_none(), "the guns never roll on a layout");
        assert_eq!(m.screen.muzzle_x, 30.0);
    }
    assert!(matches!(m.phase, Phase::RoundOver(_)));
    assert!(m.screen.field.scar.iter().any(|&s| s != 0.0), "craters dug");
    assert!(winds.iter().all(|w| w.abs() <= 2.5));
    assert!(winds.windows(2).any(|w| w[0] != w[1]), "a wind each turn");
    assert!(m.next_round());
    assert_eq!(m.round, 1);
    assert_eq!(m.screen.index, 1);
    assert_eq!(m.screen.field.base, ground.field.base, "the same ground");
    assert!(m.screen.field.scar.iter().all(|&s| s == 0.0), "fresh");
    assert_eq!(m.screen.muzzle_x, 30.0, "guns back home");

    let fixed = Layout {
        wind: WindSpec::Fixed(1500),
        ..l
    };
    let mut m = layout_match(&fixed, 77, Rules::DEFAULT);
    for _ in 0..4 {
        assert_eq!(m.screen.wind, 1.5);
        m.fire(salt_line_sim::run::shot(900, 100), false);
    }
}

#[test]
fn a_ground_never_rolls_even_under_rolling_rules() {
    let l = fork_duel(3, 0);
    let ground = Ground {
        screen: Screen::from_layout(&l, 0.0),
        wind: None,
    };
    let mut m = Match::on_ground(5, ground, Rules::DEFAULT);
    assert_eq!(m.rules.gun_shift, 0.0);
    m.rules.gun_shift = versus::GUN_SHIFT;
    assert_eq!(m.roll_guns(), None, "nor if the rules are changed after");
}

#[test]
fn a_generated_match_is_untouched() {
    // No ground: the same match as before layouts existed.
    let m = Match::new(5, 320);
    assert!(m.ground.is_none());
    assert_eq!(m.screen.wind, versus::turn_wind(5, 0, 0, &Rules::DEFAULT));
    assert_eq!(m.wind_at(3), versus::turn_wind(5, 0, 3, &Rules::DEFAULT));
}

// ---------------------------------------------------------- the dev flags

fn game(window: (i32, i32)) -> Game {
    let mut g = Game::new(false, Save::default());
    g.fit_display(window, (0, 0));
    g.take_effects();
    g
}

fn dev(g: &mut Game, args: &[&str]) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    g.start_dev(&DevArgs::parse(&args), 20_000);
}

#[test]
fn salt_layout_starts_a_duel_on_the_layout_at_its_width() {
    // A wide phone: 400 columns, so the layout is letterboxed.
    let mut g = game((1600, 720));
    assert_eq!(g.display.buffer_w, 400);
    let code = fork_duel(11, 2).to_code().unwrap();
    dev(&mut g, &["--salt-layout", &code]);
    assert_eq!(g.mode, Mode::Versus);
    let duel = &g.versus.live.as_ref().expect("a match").duel;
    assert!(duel.ground.is_some());
    assert_eq!(duel.width(), LAYOUT_WIDTH);
    assert_eq!(g.display.buffer_w, LAYOUT_WIDTH as i32);
    // The width holds while the match is on, as a resumed run's does...
    for _ in 0..5 {
        g.tick(
            1.0 / 60.0,
            &crate::input::Intents {
                window: (1600, 720),
                today: 20_000,
                ..Default::default()
            },
        );
    }
    assert_eq!(g.display.buffer_w, LAYOUT_WIDTH as i32);
}

#[test]
fn a_bad_layout_code_leaves_the_title_up_with_a_note() {
    for code in ["SL0000", "nonsense", &pit().to_code().unwrap()] {
        let mut g = game((1280, 720));
        dev(&mut g, &["--salt-layout", code]);
        assert_eq!(g.mode, Mode::Title, "{code}");
        assert!(g.versus.live.is_none());
        assert_eq!(g.status, "LAYOUT CODE NOT PLAYABLE");
        let why = g.dev.layout_refused.as_deref().unwrap_or("");
        assert!(
            why.starts_with("not a layout code") || why.starts_with("the layout does not pass")
        );
    }
    let mut g = game((1280, 720));
    let one_gun = Layout::from_run_screen(1, 0);
    assert_eq!(validate(&one_gun), Ok(()));
    dev(&mut g, &["--salt-layout", &one_gun.to_code().unwrap()]);
    assert_eq!(g.mode, Mode::Title);
    assert_eq!(g.status, "LAYOUT HAS ONE GUN: NOT A DUEL");
}

#[test]
fn the_fork_flags_print_a_code_that_plays() {
    let text = fork_report(None, Some("42:1")).unwrap();
    let code = text.lines().next().unwrap();
    assert_eq!(
        Layout::from_code(code),
        Ok(fork_duel(salt_line_sim::run::parse_seed("42"), 1))
    );
    assert!(text.contains("passes"), "{text}");
    let text = fork_report(Some("DUNE:8"), None).unwrap();
    assert!(text.starts_with("SL"), "{text}");
    assert_eq!(fork_report(None, None), None);
    for bad in ["DUNE", "DUNE:", "DUNE:x"] {
        assert!(fork_report(Some(bad), None)
            .unwrap()
            .starts_with("expected"));
    }
}

#[test]
fn reading_a_code_says_why_not() {
    assert!(read("SLZZ").unwrap_err().starts_with("not a layout code"));
    assert!(read(&pit().to_code().unwrap())
        .unwrap_err()
        .starts_with("the layout does not pass"));
    assert_eq!(read(&level().to_code().unwrap()), Ok(level()));
}

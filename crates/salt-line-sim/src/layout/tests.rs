//! Example tests for layouts: the model, building a screen, the placement
//! rules, the solvability gate, forks and the pinned share code. The
//! round trip over random layouts is a property test
//! (`tests/properties.rs`) and the decoder is fuzzed (`fuzz/`).

use super::*;
use crate::replay::screen_hash;
use crate::run::{parse_seed, StructState};
use crate::terrain::BASE_ELEVATION;

/// Level ground at `h` world units, the layout width across.
fn level(h: f64) -> Vec<u16> {
    vec![height_steps(h); LAYOUT_WIDTH]
}

/// One silo at column 200 on level ground, the gun at the emplacement, in
/// calm air: a layout that passes everything.
fn simple() -> Layout {
    Layout {
        name: LayoutName::parse("SILO").unwrap(),
        heights: level(BASE_ELEVATION),
        structures: vec![Placed {
            kind: StructureKind::Silo,
            x0: 200,
        }],
        guns: vec![30],
        wind: WindSpec::Calm,
    }
}

/// Bare level ground with two mirrored guns: a duel layout.
fn duel() -> Layout {
    Layout {
        name: LayoutName::default(),
        heights: level(BASE_ELEVATION),
        structures: Vec::new(),
        guns: vec![30, 289],
        wind: WindSpec::Range { min: 0, max: 2500 },
    }
}

/// A silo in a pit at column 220 with a wall `gap` columns in front of it
/// (toward the gun): placed fine, and hard to reach.
fn walled(wall: f64, gap: usize, floor: f64, wind: WindSpec) -> Layout {
    let mut heights = level(BASE_ELEVATION);
    let x0 = 220;
    for h in &mut heights[x0 - 6..x0 + 15] {
        *h = height_steps(floor);
    }
    for h in &mut heights[x0 - gap - 4..x0 - gap] {
        *h = height_steps(wall);
    }
    Layout {
        structures: vec![Placed {
            kind: StructureKind::Silo,
            x0: x0 as u16,
        }],
        heights,
        wind,
        ..simple()
    }
}

// ------------------------------------------------------------- the model

#[test]
fn a_name_is_keypad_text_only() {
    assert_eq!(LayoutName::parse("dune2").unwrap().as_str(), "DUNE2");
    assert_eq!(LayoutName::parse("").unwrap().as_str(), "");
    assert!(LayoutName::parse("ABCDEFGHIJKL").is_some());
    assert!(LayoutName::parse("ABCDEFGHIJKLM").is_none(), "too long");
    for bad in ["TWO WORDS", "HÉ", "A-B", "a.b", "\u{0}"] {
        assert!(LayoutName::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn heights_and_winds_are_whole_steps() {
    assert_eq!(height_steps(52.0), 832);
    assert_eq!(height_units(833), 52.0625);
    // Every step is exact: there and back changes nothing.
    for h in [0, 1, 191, 832, 1728, u16::MAX] {
        assert_eq!(height_steps(height_units(h)), h);
    }
    assert_eq!(height_steps(-3.0), 0);
    assert_eq!(height_steps(1.0e9), u16::MAX);
    assert_eq!(wind_milli(-2.4994), -2499);
    assert_eq!(wind_units(2500), 2.5);
}

#[test]
fn a_wind_checks_its_extremes_and_draws_inside_them() {
    assert_eq!(WindSpec::Calm.extremes(), vec![0]);
    assert_eq!(WindSpec::Fixed(-1200).extremes(), vec![-1200]);
    assert_eq!(
        WindSpec::Range { min: 0, max: 2500 }.extremes(),
        vec![-2500, 0, 2500]
    );
    assert_eq!(
        WindSpec::Range {
            min: 500,
            max: 2500
        }
        .extremes(),
        vec![-2500, -500, 500, 2500]
    );
    assert_eq!(WindSpec::Range { min: 0, max: 0 }.extremes(), vec![0]);

    assert_eq!(WindSpec::Calm.draw(7), 0.0);
    assert_eq!(WindSpec::Fixed(-1250).draw(7), -1.25);
    let range = WindSpec::Range {
        min: 500,
        max: 2500,
    };
    let mut signs = [false; 2];
    for seed in 0..200 {
        let w = range.draw(seed);
        assert_eq!(w, range.draw(seed), "a draw is a function of its seed");
        assert!((0.5..=2.5).contains(&w.abs()), "{w}");
        signs[(w > 0.0) as usize] = true;
    }
    assert_eq!(signs, [true, true], "both ways");
}

#[test]
fn mirroring_copies_the_left_half_exactly() {
    let mut l = Layout::from_run_screen(9, 4);
    assert!(l.asymmetry() > 0.0);
    l.mirror_left();
    assert_eq!(l.asymmetry(), 0.0);
    let n = l.width();
    assert!((0..n).all(|i| l.heights[i] == l.heights[n - 1 - i]));
}

// ------------------------------------------------------------ the screen

#[test]
fn a_layout_builds_the_screen_the_generator_would_on_its_ground() {
    let l = simple();
    let s = Screen::from_layout(&l, 1.5);
    assert_eq!(s.width, LAYOUT_WIDTH);
    assert_eq!(s.field.base[0], BASE_ELEVATION);
    assert!(s.field.scar.iter().all(|&v| v == 0.0));
    assert_eq!(s.wind, 1.5);
    assert_eq!(s.wind_late, None);
    assert_eq!(s.muzzle_x, 30.0);
    assert_eq!(s.seed, l.digest());
    let silo = &s.structures[0];
    assert_eq!((silo.kind, silo.x0), (StructureKind::Silo, 200));
    assert_eq!(
        (silo.base, silo.original_mean),
        (BASE_ELEVATION, BASE_ELEVATION)
    );
    assert_eq!(silo.state, StructState::Intact);
    // A layout screen plays: the solver's shots clear it, by the usual rules.
    let lo = Loadout::new();
    let shot = solver::validate(&s, &lo).candidates[0].shot;
    let mut played = s.clone();
    assert!(played.apply_shot(shot, &lo, None, false).cleared);
}

#[test]
fn a_structure_is_seated_on_its_highest_column() {
    let mut l = simple();
    // A step under the silo's footprint: it stands on the high side.
    for h in &mut l.heights[204..] {
        *h = height_steps(55.0);
    }
    let s = Screen::from_layout(&l, 0.0);
    assert_eq!(s.structures[0].base, 55.0);
}

// ------------------------------------------------------------- placement

#[test]
fn a_good_layout_passes_every_check() {
    assert_eq!(check_placement(&simple()), vec![]);
    assert_eq!(validate(&simple()), Ok(()));
    assert_eq!(check_placement(&duel()), vec![]);
}

#[test]
fn each_placement_rule_names_what_breaks_it() {
    let cases: Vec<(Layout, Problem)> = vec![
        (
            Layout {
                heights: vec![height_steps(BASE_ELEVATION); 360],
                ..simple()
            },
            Problem::Width(360),
        ),
        (
            {
                let mut l = simple();
                l.heights[77] = height_steps(WORLD_FLOOR - 0.0625);
                l
            },
            Problem::HeightOutOfWorld { column: 77 },
        ),
        (
            Layout {
                guns: vec![],
                ..simple()
            },
            Problem::GunCount(0),
        ),
        (
            Layout {
                guns: vec![13],
                ..simple()
            },
            Problem::GunOffScreen(0),
        ),
        (
            {
                // A ditch under most of the chassis: it would sit on its rim.
                let mut l = simple();
                for h in &mut l.heights[20..40] {
                    *h = height_steps(30.0);
                }
                l
            },
            Problem::GunNotSeated(0),
        ),
        (
            Layout {
                guns: vec![30, 280],
                ..duel()
            },
            Problem::GunsNotMirrored,
        ),
        (
            Layout {
                guns: vec![289, 30],
                ..duel()
            },
            Problem::GunsOutOfOrder,
        ),
        (
            // Mirrored, but the chassis overlap.
            Layout {
                guns: vec![150, 169],
                ..duel()
            },
            Problem::GunsTooClose,
        ),
        (
            // One spike under the right gun's last chassis column, which
            // only the mirrored seating sees (§11.2).
            {
                let mut l = duel();
                l.heights[289 + 13] = height_steps(BASE_ELEVATION + 30.0);
                l
            },
            Problem::GunNotSeated(1),
        ),
        (
            Layout {
                structures: Vec::new(),
                ..simple()
            },
            Problem::NoTargets,
        ),
        (
            Layout {
                structures: [100u16, 150, 200, 250]
                    .map(|x0| Placed {
                        kind: StructureKind::RadioMast,
                        x0,
                    })
                    .to_vec(),
                ..simple()
            },
            Problem::TooManyTargets(4),
        ),
        (
            Layout {
                structures: simple().structures,
                ..duel()
            },
            Problem::StructuresInDuel,
        ),
        (
            Layout {
                structures: vec![Placed {
                    kind: StructureKind::Pylon,
                    x0: 309,
                }],
                ..simple()
            },
            Problem::StructureOffScreen(0),
        ),
        (
            {
                // Balanced on a one-column spike.
                let mut l = simple();
                l.heights[204] = height_steps(70.0);
                l
            },
            Problem::StructureFloating(0),
        ),
        (
            {
                // Walls taller than the silo on both sides.
                let mut l = simple();
                for c in (197..200).chain(209..212) {
                    l.heights[c] = height_steps(BASE_ELEVATION + 30.0);
                }
                l
            },
            Problem::StructureBuried(0),
        ),
        (
            Layout {
                structures: vec![Placed {
                    kind: StructureKind::Silo,
                    x0: 48,
                }],
                ..simple()
            },
            Problem::StructureOnGun(0),
        ),
        (
            Layout {
                structures: vec![Placed {
                    kind: StructureKind::Silo,
                    x0: 200,
                }],
                guns: vec![260],
                ..simple()
            },
            Problem::StructureBehindGun(0),
        ),
        (
            Layout {
                structures: [150u16, 180]
                    .map(|x0| Placed {
                        kind: StructureKind::Silo,
                        x0,
                    })
                    .to_vec(),
                ..simple()
            },
            Problem::StructuresTooClose(0, 1),
        ),
        (
            Layout {
                wind: WindSpec::Fixed(-7001),
                ..simple()
            },
            Problem::Wind,
        ),
        (
            Layout {
                wind: WindSpec::Range {
                    min: 2000,
                    max: 1000,
                },
                ..simple()
            },
            Problem::Wind,
        ),
    ];
    for (layout, problem) in cases {
        assert_eq!(check_placement(&layout), vec![problem], "{problem:?}");
        assert_eq!(validate(&layout), Err(vec![problem]));
    }
}

#[test]
fn a_gun_is_seated_on_its_own_chassis_columns() {
    // The right gun's chassis is the mirror of the left's: columns
    // 277..=302 for a pivot at 289, so a spike at 276 is beside it, not
    // under it. The left gun's spans 17..=42.
    for (column, gun) in [
        (289 + 13, Some(1)),
        (289 - 13, None),
        (30 - 13, Some(0)),
        (30 + 13, None),
    ] {
        let mut l = duel();
        l.heights[column] = height_steps(BASE_ELEVATION + 30.0);
        let want: Vec<Problem> = gun.into_iter().map(Problem::GunNotSeated).collect();
        assert_eq!(check_placement(&l), want, "spike at {column}");
    }
}

#[test]
fn the_gate_flies_shots_only_over_a_well_placed_layout() {
    let off = Layout {
        guns: vec![13],
        ..walled(108.0, 2, 20.0, WindSpec::Calm)
    };
    assert_eq!(check_targets(&off), vec![]);
    assert_eq!(validate(&off), Err(vec![Problem::GunOffScreen(0)]));
    assert_eq!(check_targets(&duel()), vec![], "a duel's gate is versus's");
}

// ---------------------------------------------------------- solvability

#[test]
fn a_target_no_shot_can_reach_is_unsolvable() {
    let l = walled(108.0, 2, 20.0, WindSpec::Calm);
    assert_eq!(check_placement(&l), vec![], "placed fine");
    assert_eq!(
        validate(&l),
        Err(vec![Problem::Unsolvable {
            gun: 0,
            wind: 0,
            shots: 0
        }])
    );
}

#[test]
fn the_gate_checks_every_extreme_of_the_wind() {
    // Fine in calm air and into the wind; a strong tailwind carries every
    // lob over the wall's lee.
    let calm = walled(90.0, 2, 30.0, WindSpec::Calm);
    assert_eq!(validate(&calm), Ok(()));
    let ranged = walled(90.0, 2, 30.0, WindSpec::Range { min: 0, max: 7000 });
    let Err(problems) = validate(&ranged) else {
        panic!("passes at the extremes too");
    };
    assert!(problems
        .iter()
        .all(|p| matches!(p, Problem::Unsolvable { wind: 7000, .. })));
}

// ----------------------------------------------------------------- forks

/// Forking a run screen keeps everything but rounding: the same structures
/// at the same columns, the same gun, every height and seat within half a
/// step, the wind within half a thousandth. The fork is a fixed point
/// (forking the screen it builds gives it back), and its share code
/// rebuilds a bit-identical screen.
#[test]
fn a_forked_run_screen_is_the_generated_one_to_within_a_rounding() {
    let lo = Loadout::new();
    for seed in [
        1u64,
        7,
        0xDEAD_BEEF,
        parse_seed("DUNE"),
        parse_seed("20260907"),
    ] {
        for index in [0, 2, 5, 9, 17, 33, 60] {
            let gen = generate_screen(seed, index, LAYOUT_WIDTH, &lo);
            let l = Layout::from_run_screen(seed, index);
            let WindSpec::Fixed(w) = l.wind else {
                panic!("a run fork's wind is fixed");
            };
            let fork = Screen::from_layout(&l, wind_units(w));
            assert_eq!(l.guns, vec![30]);
            assert_eq!(fork.muzzle_x, gen.muzzle_x);
            assert!((fork.wind - gen.wind).abs() <= 0.0005);
            assert_eq!(fork.structures.len(), gen.structures.len());
            for (f, g) in fork.structures.iter().zip(&gen.structures) {
                assert_eq!((f.kind, f.x0), (g.kind, g.x0));
                assert!((f.base - g.base).abs() <= 1.0 / 32.0);
                assert!((f.original_mean - g.original_mean).abs() <= 1.0 / 32.0);
            }
            for (f, g) in fork.field.base.iter().zip(&gen.field.base) {
                assert!((f - g).abs() <= 1.0 / 32.0);
            }
            let again = Layout::from_screen(&fork, l.guns.clone(), l.wind);
            assert_eq!(again, l, "a fork of the fork is the fork");
            let decoded = Layout::from_code(&l.to_code().unwrap()).unwrap();
            assert_eq!(decoded, l);
            assert_eq!(
                screen_hash(&Screen::from_layout(&decoded, wind_units(w))),
                screen_hash(&fork)
            );
        }
    }
}

/// The fork plays like the screen it came from. Every shot the gate found
/// on the generated screen, and as many random ones, is fired at both; at
/// least 99.5% must end the same way (NOTES §9 measures 99.9%). The rest
/// are grazing shots, where a ground moved by 1/32 changes which step a
/// shell lands on.
#[test]
fn a_forked_run_screen_plays_like_the_generated_one() {
    let lo = Loadout::new();
    let (mut same, mut total) = (0, 0);
    for seed in 0..12u64 {
        for index in [1, 6, 14, 30] {
            let gen = generate_screen(seed, index, LAYOUT_WIDTH, &lo);
            let l = Layout::from_run_screen(seed, index);
            let fork = Screen::from_layout(&l, l.wind.draw(0));
            let mut r = Pcg32::new(seed ^ index as u64);
            let mut shots: Vec<_> = solver::validate(&gen, &lo)
                .candidates
                .iter()
                .map(|c| c.shot)
                .collect();
            shots.extend(
                (0..8).map(|_| {
                    run::shot(r.range_u32(300, 1600) as i32, r.range_u32(250, 1000) as i32)
                }),
            );
            for shot in shots {
                let a = gen.clone().apply_shot(shot, &lo, None, false);
                let b = fork.clone().apply_shot(shot, &lo, None, false);
                let (ea, eb) = (a.traces[0].end.pos(), b.traces[0].end.pos());
                total += 1;
                same += ((ea.x - eb.x).abs() < 1.0
                    && (ea.y - eb.y).abs() < 1.0
                    && a.cleared == b.cleared
                    && a.collapsed == b.collapsed) as usize;
            }
        }
    }
    assert!(same * 1000 >= total * 995, "{same}/{total}");
}

/// Forks pass the solvability gate as their screens did. The placement
/// rules are another matter: the generator stands some structures on a
/// spire or a ridge's flank (NOTES §9), which the editor flags as floating.
#[test]
fn a_forked_run_screen_passes_the_gate() {
    for seed in 0..10u64 {
        for index in [0, 3, 8, 21, 40] {
            let l = Layout::from_run_screen(seed, index);
            let placed = check_placement(&l);
            assert!(
                placed
                    .iter()
                    .all(|p| matches!(p, Problem::StructureFloating(_))),
                "seed {seed} screen {index}: {placed:?}"
            );
            if placed.is_empty() {
                assert_eq!(validate(&l), Ok(()), "seed {seed} screen {index}");
            }
        }
    }
}

// ------------------------------------------------------------- the code

#[test]
fn the_share_code_is_pinned() {
    // The format: a hand-made layout's code changes only with LAYOUT_FORMAT.
    let hand = simple().to_code().unwrap();
    // The generator too: a fork's code also moves when a SIM_EPOCH change
    // moves the screen it was forked from.
    let fork = Layout::from_run_screen(parse_seed("20260907"), 2)
        .to_code()
        .unwrap();
    insta::assert_snapshot!(format!("{hand}\n{fork}\n"));
}

#[test]
fn any_one_mistyped_character_is_caught() {
    let code = Layout::from_run_screen(5, 3).to_code().unwrap();
    let alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for (i, c) in code.char_indices().skip(CODE_PREFIX.len()) {
        for other in alphabet.chars().filter(|&o| o != c) {
            let mut typo = code.clone();
            typo.replace_range(i..i + 1, &other.to_string());
            assert!(Layout::from_code(&typo).is_err(), "{typo}");
        }
    }
    // Case, characters read alike and separators are not mistakes.
    let relaxed = format!(
        " {}-{} ",
        code[..10].to_ascii_lowercase(),
        code[10..].replace('0', "O").replace('1', "l")
    );
    assert_eq!(Layout::from_code(&relaxed), Layout::from_code(&code));
}

#[test]
fn decoding_refuses_rather_than_guesses() {
    let code = simple().to_code().unwrap();
    assert_eq!(Layout::from_code(""), Err(CodeError::Prefix));
    assert_eq!(Layout::from_code("XX0000"), Err(CodeError::Prefix));
    assert_eq!(Layout::from_code("SLU"), Err(CodeError::Character));
    assert_eq!(Layout::from_code("SL00"), Err(CodeError::Malformed));
    assert!(
        matches!(
            Layout::from_code(&code[..code.len() - 4]),
            Err(CodeError::Checksum | CodeError::Padding)
        ),
        "cut short"
    );
    assert_eq!(
        Layout::from_code(&"S".repeat(MAX_CODE_CHARS + 1)),
        Err(CodeError::TooLong)
    );
    assert_eq!(
        Layout {
            guns: vec![30, 289, 160],
            ..simple()
        }
        .to_code(),
        Err(CodeError::Unencodable)
    );
    // Wider than any buffer: the decoder would refuse it, so the encoder
    // does too.
    assert_eq!(
        Layout {
            heights: vec![height_steps(BASE_ELEVATION); crate::SCREEN_W_MAX + 1],
            ..simple()
        }
        .to_code(),
        Err(CodeError::Unencodable)
    );
}

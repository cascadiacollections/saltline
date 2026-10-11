//! The versus rules: mirroring, the hit rule, scoring, duel generation and
//! the turn wind.

use super::*;
use proptest::prelude::*;
use salt_line_sim::ballistics::{blast_radius, Shot};
use salt_line_sim::terrain::Material;
use salt_line_sim::SCREEN_W_MAX;

/// A shot that goes nowhere near the other gun: straight up at the lowest
/// power, so it comes down beside the shooter.
const MISS: Shot = Shot {
    angle_ticks: 1800,
    power_ticks: 0,
};

/// A match whose screen is replaced by one over `base`, in calm air.
fn match_on(base: Vec<f64>) -> Match {
    let mut m = Match::new(7, base.len());
    m.screen = assemble(0, 7, base.len(), Template::Flat, true);
    m.screen.field = Heightfield::from_base(base);
    m.screen.wind = 0.0;
    m
}

/// A perfectly symmetric field: a hill in the middle and a dip near each
/// gun, so a shot has real terrain to land on.
fn symmetric_base(width: usize) -> Vec<f64> {
    (0..width)
        .map(|i| {
            let d = (i as f64 - (width as f64 - 1.0) * 0.5).abs();
            60.0 + 25.0 * (-(d * d) / 900.0).exp() - 6.0 * (-((d - 110.0).powi(2)) / 200.0).exp()
        })
        .collect()
}

/// A shot `side` can fire that hits the other gun, as things stand.
fn hitting_shot(m: &Match, side: Side) -> Shot {
    let frame = frame_for(&m.screen, side);
    let target = target_in_frame(&m.screen, side);
    *shots_at(&frame, target, &m.loadout, HIT_REACH, 1)
        .first()
        .unwrap_or_else(|| panic!("{side:?} has no shot at the other gun"))
}

fn scars_bits(f: &Heightfield) -> Vec<u64> {
    f.scar.iter().map(|s| s.to_bits()).collect()
}

// --------------------------------------------------------------- mirroring

proptest! {
    #[test]
    fn mirroring_a_field_twice_is_the_identity(
        heights in prop::collection::vec(12.0f64..108.0, 2..64),
        craters in prop::collection::vec((0.0f64..64.0, 1.0f64..12.0), 0..6),
    ) {
        let mut f = Heightfield::from_base(heights);
        for (x, r) in craters {
            f.crater(x, r);
        }
        let back = mirror_field(&mirror_field(&f));
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        prop_assert_eq!(bits(&back.base), bits(&f.base));
        prop_assert_eq!(bits(&back.scar), bits(&f.scar));
        prop_assert_eq!(back.material, f.material);
    }

    #[test]
    fn mirroring_points_and_boxes_twice_is_the_identity(
        x in -500.0f64..900.0,
        y in -100.0f64..300.0,
        w in 1.0f64..40.0,
        width in 320usize..=400,
    ) {
        // Points are floats, so `(w - 1) - ((w - 1) - x)` can be off in the
        // last bit; the fields above round-trip exactly because they are
        // only reordered.
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * (1.0 + b.abs());
        let p = Vec2::new(x, y);
        let back = mirror_point(mirror_point(p, width), width);
        prop_assert!(close(back.x, p.x) && back.y == p.y, "{:?}", back);
        let b = Aabb::new(x, y, x + w, y + w);
        let m = mirror_box(b, width);
        prop_assert!(m.x0 <= m.x1);
        let bb = mirror_box(m, width);
        prop_assert!(close(bb.x0, b.x0) && close(bb.x1, b.x1), "{:?}", bb);
        prop_assert!(bb.y0 == b.y0 && bb.y1 == b.y1);
    }
}

#[test]
fn a_mirrored_field_reads_the_same_ground_from_the_other_side() {
    let base: Vec<f64> = (0..50).map(|i| 20.0 + i as f64 * 1.5).collect();
    let f = Heightfield::from_base(base);
    let m = mirror_field(&f);
    for x in [0.0, 3.25, 17.5, 48.9, 49.0] {
        let there = f.surface_at(x);
        let here = m.surface_at(mirror_x(x, 50));
        assert!((there - here).abs() < 1e-9, "x {x}: {there} vs {here}");
    }
}

#[test]
fn on_symmetric_ground_the_two_guns_are_mirror_images() {
    let width = 340;
    let m = match_on(symmetric_base(width));
    let left = muzzle_of(&m.screen, Side::Left);
    let right = muzzle_of(&m.screen, Side::Right);
    assert_eq!(right.x, mirror_x(left.x, width));
    assert!((right.y - left.y).abs() < 1e-9);
    let lb = emplacement_box(&m.screen, Side::Left);
    assert_eq!(
        emplacement_box(&m.screen, Side::Right),
        mirror_box(lb, width)
    );
}

#[test]
fn a_right_shot_lands_where_the_mirror_of_a_left_shot_does() {
    // On symmetric ground, the right gun firing into a wind is the left gun
    // firing into the opposite wind, seen in a mirror: same flight, same
    // landing, same crater, all reflected.
    let width = 333;
    let shot = Shot::new(700, 520);
    for wind in [0.0, 1.7, -2.2] {
        let mut left = match_on(symmetric_base(width));
        left.screen.wind = wind;
        let l = left.fire(shot, true).unwrap();
        assert_eq!(l.shooter, Side::Left);

        let mut right = match_on(symmetric_base(width));
        right.to_move = Side::Right;
        right.screen.wind = -wind;
        let r = right.fire(shot, true).unwrap();
        assert_eq!(r.shooter, Side::Right);

        let (lp, rp) = (l.landing.unwrap(), r.landing.unwrap());
        assert!((rp.x - mirror_x(lp.x, width)).abs() < 1e-6, "{lp:?} {rp:?}");
        assert!((rp.y - lp.y).abs() < 1e-6);
        // The right shell leaves the right gun and flies left.
        let path = &r.traces[0].path;
        assert_eq!(path[0], muzzle_of(&right.screen, Side::Right));
        assert!(path[path.len() - 1].x < path[0].x);
        // The crater is the left crater, reflected into the shared frame.
        let ls = &left.screen.field.scar;
        let rs = &right.screen.field.scar;
        for i in 0..width {
            assert!((rs[i] - ls[width - 1 - i]).abs() < 1e-9, "column {i}");
        }
        assert!(rs.iter().any(|s| *s > 0.5), "the shot dug nothing");
    }
}

#[test]
fn a_right_shot_digs_its_crater_at_the_reflected_column() {
    // Uneven ground this time: the right player's shot, fired in the
    // mirrored frame, matches the same shot fired on a screen built by hand
    // from the reversed ground, and its crater comes back reflected.
    let width = 320;
    let base: Vec<f64> = (0..width)
        .map(|i| 50.0 + 20.0 * ((i as f64) * 0.031).sin() + (i % 7) as f64)
        .collect();
    let shot = Shot::new(820, 610);
    let mut m = match_on(base.clone());
    m.to_move = Side::Right;
    m.screen.wind = 1.25;
    let report = m.fire(shot, false).unwrap();

    let mut by_hand = match_on(base.iter().rev().copied().collect());
    by_hand.screen.wind = -1.25;
    let expected = by_hand.fire(shot, false).unwrap();

    let got = report.landing.unwrap();
    let want = mirror_point(expected.landing.unwrap(), width);
    assert_eq!(got.x.to_bits(), want.x.to_bits());
    assert_eq!(got.y.to_bits(), want.y.to_bits());
    let mut reflected = scars_bits(&by_hand.screen.field);
    reflected.reverse();
    assert_eq!(scars_bits(&m.screen.field), reflected);
}

// ----------------------------------------------------------------- the hit

fn trace_ending(end: End) -> Trace {
    Trace {
        origin: Vec2::new(30.0, 60.0),
        end,
        apex: Vec2::new(100.0, 140.0),
        flight_time: 1.0,
        bounces: 0,
        tunneled: false,
        damage_scale: 1.0,
        impact_speed: 100.0,
        steps: 120,
        path: Vec::new(),
    }
}

#[test]
fn a_hit_is_any_blast_that_reaches_the_box() {
    let lo = Loadout::new();
    let target = Aabb::new(270.0, 50.0, 296.0, 62.0);
    let blast = blast_radius(&lo, 1.0);
    let at = |x: f64, y: f64| End::Terrain {
        pos: Vec2::new(x, y),
        material: Material::Sand,
    };
    let cases = [
        // Straight into the box.
        (
            End::Structure {
                index: 0,
                pos: Vec2::new(280.0, 60.0),
            },
            true,
        ),
        // Short of the box, but the blast overlaps it.
        (at(270.0 - blast + 0.5, 50.0), true),
        // Just out of reach, short and long.
        (at(270.0 - blast - 0.5, 50.0), false),
        (at(296.0 + blast + 0.5, 50.0), false),
        // Under the gun counts as well as beside it.
        (at(283.0, 50.0 - blast + 1.0), true),
        // An airburst over the gun.
        (
            End::Fuse {
                pos: Vec2::new(283.0, 62.0 + blast - 1.0),
            },
            true,
        ),
        // A shell that leaves the screen or runs out of clock hits nothing,
        // wherever it was.
        (
            End::OffScreen {
                pos: Vec2::new(283.0, 55.0),
            },
            false,
        ),
        (
            End::Timeout {
                pos: Vec2::new(283.0, 55.0),
            },
            false,
        ),
    ];
    for (end, want) in cases {
        assert_eq!(
            hits(&trace_ending(end), target, &lo, HIT_REACH),
            want,
            "{end:?}"
        );
    }
}

#[test]
fn a_shorter_reach_needs_a_closer_shell() {
    let lo = Loadout::new();
    let target = Aabb::new(270.0, 50.0, 296.0, 62.0);
    let at = |x: f64| End::Terrain {
        pos: Vec2::new(x, 50.0),
        material: Material::Sand,
    };
    let hit = |x: f64, reach: f64| hits(&trace_ending(at(x)), target, &lo, reach);
    // Four units short: in the default reach, out of a reach of three.
    assert!(hit(266.0, HIT_REACH));
    assert!(!hit(266.0, 3.0));
    // With no reach at all, only a shell landing in the box counts.
    assert!(hit(283.0, 0.0));
    assert!(!hit(269.5, 0.0));
}

#[test]
fn a_hit_is_scored_only_against_the_other_gun() {
    let mut m = match_on(symmetric_base(320));
    // Dropping a shell on your own gun is not a point for anyone; the
    // report says it was a self-hit, for the screens to call out.
    let r = m.fire(MISS, false).unwrap();
    assert!(!r.hit);
    assert!(r.self_hit);
    assert_eq!(m.hits, [0, 0]);
    let shot = hitting_shot(&m, Side::Right);
    let r = m.fire(shot, false).unwrap();
    assert!(r.hit);
    assert!(!r.self_hit);
    assert_eq!(m.hits, [0, 1]);
    // A right self-hit is found in the shared frame too.
    m.fire(Shot::new(1400, 330), false).unwrap();
    let r = m.fire(MISS, false).unwrap();
    assert_eq!(r.shooter, Side::Right);
    assert!(r.self_hit && !r.hit);
}

// ----------------------------------------------------- scoring and flow

#[test]
fn players_alternate_and_each_turn_draws_its_own_wind() {
    let mut m = Match::new(0xBEEF, 320);
    assert_eq!(m.to_move, Side::Left);
    assert_eq!(
        m.screen.wind.to_bits(),
        turn_wind(0xBEEF, 0, 0, &Rules::DEFAULT).to_bits()
    );
    for turn in 1..6 {
        let before = m.to_move;
        m.fire(MISS, false).unwrap();
        assert_eq!(m.to_move, before.other());
        assert_eq!(m.turn, turn);
        assert_eq!(
            m.screen.wind.to_bits(),
            turn_wind(0xBEEF, 0, turn, &Rules::DEFAULT).to_bits()
        );
    }
}

#[test]
fn three_hits_take_a_round_and_two_rounds_take_the_match() {
    let mut m = Match::new(0x5A17, 320);
    // Round 1: left hits every time, right always misses. Left's third hit
    // puts it three clear, which no answer could tie, so the round ends on
    // it (EQUAL_SHOTS only gives right a shot that could tie).
    for n in 1..=HITS_TO_WIN_ROUND {
        let shot = hitting_shot(&m, Side::Left);
        assert!(m.fire(shot, false).unwrap().hit);
        assert_eq!(m.hits, [n, 0]);
        if n < HITS_TO_WIN_ROUND {
            assert_eq!(m.phase, Phase::Aiming);
            assert!(!m.fire(MISS, false).unwrap().hit);
        }
    }
    assert_eq!(m.phase, Phase::RoundOver(Side::Left));
    assert_eq!(m.rounds, [1, 0]);
    // Nothing fires between rounds.
    assert!(m.fire(MISS, false).is_none());

    // A new screen, the hits back to zero, and right opens round 2.
    let old = m.screen.seed;
    assert!(m.next_round());
    assert!(!m.next_round(), "only once per round");
    assert_eq!((m.round, m.turn, m.hits), (1, 0, [0, 0]));
    assert_eq!(m.to_move, Side::Right);
    assert_ne!(m.screen.seed, old);
    assert!(
        m.screen.field.scar.iter().all(|s| *s == 0.0),
        "fresh ground"
    );

    // Round 2: right wins it.
    for _ in 0..HITS_TO_WIN_ROUND {
        let shot = hitting_shot(&m, Side::Right);
        m.fire(shot, false).unwrap();
        if m.phase == Phase::Aiming {
            m.fire(MISS, false).unwrap();
        }
    }
    assert_eq!(m.phase, Phase::RoundOver(Side::Right));
    assert_eq!(m.rounds, [1, 1]);

    // Round 3 decides it, left opening again.
    assert!(m.next_round());
    assert_eq!(m.to_move, Side::Left);
    for _ in 0..HITS_TO_WIN_ROUND {
        let shot = hitting_shot(&m, Side::Left);
        m.fire(shot, false).unwrap();
        if m.phase == Phase::Aiming {
            m.fire(MISS, false).unwrap();
        }
    }
    assert_eq!(m.phase, Phase::MatchOver(Side::Left));
    assert_eq!(m.rounds, [2, 1]);
    assert!(m.fire(MISS, false).is_none());
    assert!(!m.next_round(), "the match is over");
}

#[test]
fn with_equal_shots_the_side_that_did_not_open_answers_when_a_hit_could_tie() {
    let equal = Rules {
        equal_shots: true,
        ..Rules::DEFAULT
    };
    let first = Rules {
        equal_shots: false,
        ..Rules::DEFAULT
    };
    let (l, r) = (Side::Left, Side::Right);
    let n = HITS_TO_WIN_ROUND;
    // Left opened and has just made its winning hit one ahead: right still
    // shoots, since a hit would tie.
    assert_eq!(round_winner([n, n - 1], l, l, &equal), None);
    assert_eq!(round_winner([n, n - 1], l, l, &first), Some(l));
    // Two or more ahead, no answer could tie: left wins on its own shot.
    assert_eq!(round_winner([n, n - 2], l, l, &equal), Some(l));
    assert_eq!(round_winner([n, 0], l, l, &equal), Some(l));
    // Two ahead but short of the mark is not a win.
    assert_eq!(round_winner([n - 1, n - 3], l, l, &equal), None);
    // Right's answer misses: left wins. It hits: level, so play goes on.
    assert_eq!(round_winner([n, n - 1], r, l, &equal), Some(l));
    assert_eq!(round_winner([n, n], r, l, &equal), None);
    // Level past the mark, a pair that ends with one side ahead decides it.
    assert_eq!(round_winner([n + 1, n], r, l, &equal), Some(l));
    assert_eq!(round_winner([n, n + 1], r, l, &equal), Some(r));
    // The side that did not open wins on its own shot when it gets there
    // first, since the opener has already had as many shots.
    assert_eq!(round_winner([n - 1, n], r, l, &equal), Some(r));
    // Below the mark nobody wins, whoever shot.
    assert_eq!(round_winner([n - 1, n - 2], r, l, &equal), None);
    // Right opens round 2: the same rule with the sides swapped.
    assert_eq!(round_winner([n - 1, n], r, r, &equal), None);
    assert_eq!(round_winner([n - 1, n], l, r, &equal), Some(r));
}

#[test]
fn the_stakes_say_what_rides_on_the_coming_shot() {
    let n = HITS_TO_WIN_ROUND;
    let mut m = Match::new(0x5A17, 320);
    let at = |m: &mut Match, hits: [u32; 2], to_move: Side| {
        m.hits = hits;
        m.to_move = to_move;
        m.stakes()
    };
    // Left opens round 1. An ordinary turn.
    assert_eq!(at(&mut m, [0, 0], Side::Left), None);
    // Left has the mark one ahead: right's last shot ties.
    assert_eq!(at(&mut m, [n, n - 1], Side::Right), Some(Stakes::LastShot));
    // Two or more ahead never reaches right: the round ended on left's
    // shot. (Asked anyway, a hit could not even tie, so no LAST SHOT.)
    assert_ne!(at(&mut m, [n, n - 2], Side::Right), Some(Stakes::LastShot));
    // Level past the mark after a pair: it plays on.
    assert_eq!(at(&mut m, [n, n], Side::Left), Some(Stakes::Tied));
    assert_eq!(at(&mut m, [n + 1, n], Side::Right), Some(Stakes::LastShot));
    // Left missed at 3-3: a right hit wins outright.
    assert_eq!(at(&mut m, [n, n], Side::Right), Some(Stakes::HitToWin));
    // So does a right hit at 2-2, and a left hit two clear of right.
    assert_eq!(
        at(&mut m, [n - 1, n - 1], Side::Right),
        Some(Stakes::HitToWin)
    );
    assert_eq!(
        at(&mut m, [n - 1, n - 3], Side::Left),
        Some(Stakes::HitToWin)
    );
    // A left hit at 2-2 only reaches the mark one ahead: right answers.
    assert_eq!(at(&mut m, [n - 1, n - 1], Side::Left), None);
    // Without equal shots the first to the mark wins, so only HIT TO WIN.
    m.rules.equal_shots = false;
    assert_eq!(at(&mut m, [n - 1, 0], Side::Left), Some(Stakes::HitToWin));
    assert_eq!(at(&mut m, [n - 2, n - 2], Side::Right), None);
    // Nothing between rounds.
    m.phase = Phase::RoundOver(Side::Left);
    assert_eq!(at(&mut m, [n - 1, 0], Side::Left), None);
}

#[test]
fn a_roll_says_which_way_it_went() {
    let landed = Match::new(1, 320).screen;
    let roll = |dx: f64| Roll {
        moved: [Vec2::new(dx, 0.0), Vec2::new(-dx, 0.0)],
        landed: landed.clone(),
    };
    assert_eq!(roll(GUN_SHIFT).way(), RollWay::Closer);
    assert_eq!(roll(-GUN_SHIFT).way(), RollWay::Apart);
}

#[test]
fn an_answered_winning_hit_plays_on() {
    let mut m = Match::new(0x5A17, 320);
    for _ in 0..HITS_TO_WIN_ROUND {
        let shot = hitting_shot(&m, Side::Left);
        assert!(m.fire(shot, false).unwrap().hit);
        let shot = hitting_shot(&m, Side::Right);
        assert!(m.fire(shot, false).unwrap().hit);
    }
    assert_eq!(m.hits, [HITS_TO_WIN_ROUND, HITS_TO_WIN_ROUND]);
    assert_eq!(m.phase, Phase::Aiming, "level after a pair: no winner yet");
    assert_eq!(m.to_move, Side::Left);
    // Left goes ahead and right misses its answer: left takes the round.
    let shot = hitting_shot(&m, Side::Left);
    m.fire(shot, false).unwrap();
    m.fire(MISS, false).unwrap();
    assert_eq!(m.phase, Phase::RoundOver(Side::Left));
}

#[test]
fn the_guns_roll_after_a_pair_with_a_hit_and_stay_mirrored() {
    let mut m = Match::new(0x5A17, 320);
    let home = m.screen.muzzle_x;
    // A pair with no hit: the guns stay.
    m.fire(MISS, false).unwrap();
    assert!(m.fire(MISS, false).unwrap().rolled.is_none());
    assert_eq!(m.screen.muzzle_x, home);
    // Left hits: nothing moves before right has answered...
    let shot = hitting_shot(&m, Side::Left);
    let r = m.fire(shot, false).unwrap();
    assert!(r.hit && r.rolled.is_none());
    assert_eq!(m.screen.muzzle_x, home);
    let marks = m.landing;
    let before = Side::BOTH.map(|s| muzzle_of(&m.screen, s));
    // ...then both guns roll one step, the same way.
    let r = m.fire(MISS, false).unwrap();
    let moved = r.rolled.expect("a pair with a hit rolls the guns").moved;
    assert_eq!((m.screen.muzzle_x - home).abs(), GUN_SHIFT);
    let left = muzzle_of(&m.screen, Side::Left).x;
    let right = muzzle_of(&m.screen, Side::Right).x;
    assert_eq!(right, mirror_x(left, m.width()), "still mirror images");
    // The report says how far each pivot went: opposite ways across the
    // screen, the same distance.
    for side in Side::BOTH {
        let now = muzzle_of(&m.screen, side);
        let d = moved[side.index()];
        assert_eq!(
            (d.x, d.y),
            (
                now.x - before[side.index()].x,
                now.y - before[side.index()].y
            )
        );
    }
    assert_eq!(moved[0].x, -moved[1].x);
    // Left's landing mark rode along with left's gun. Right's is the MISS
    // it just fired, which moved with right's gun.
    let left_mark = marks[0].expect("left landed");
    let now = m.landing[0].unwrap();
    assert_eq!(
        (now.x, now.y),
        (left_mark.x + moved[0].x, left_mark.y + moved[0].y)
    );
    let right_mark = r.landing.expect("right landed");
    let now = m.landing[1].unwrap();
    assert_eq!(
        (now.x, now.y),
        (right_mark.x + moved[1].x, right_mark.y + moved[1].y)
    );
}

#[test]
fn the_default_ladder_keeps_the_guns_on_screen() {
    let back = GUN_SHIFT * GUN_ROLL_BACK as f64;
    // Both ways reachable from home.
    const { assert!(GUN_ROLL_BACK >= 1 && GUN_ROLL_FORWARD >= 1) };
    assert!(
        EMPLACEMENT_X - back >= GUN_EDGE_MARGIN,
        "a gun rolled all the way back would hang off the screen"
    );
}

#[test]
fn rolled_guns_stay_in_bounds_and_on_solvable_ground() {
    let lo = Loadout::new();
    let mut all = Vec::new();
    for seed in 0..12u64 {
        let mut m = Match::new(seed, 320);
        let mut spots = vec![m.screen.muzzle_x];
        for turn in 0..12u32 {
            m.screen.wind = turn_wind(seed, 0, turn, &m.rules);
            m.turn = turn;
            if m.roll_guns().is_some() {
                spots.push(m.screen.muzzle_x);
            }
            let rung = (m.screen.muzzle_x - EMPLACEMENT_X) / GUN_SHIFT;
            assert_eq!(rung, rung.round(), "off the ladder: {rung}");
            assert!(
                (-(GUN_ROLL_BACK as f64)..=GUN_ROLL_FORWARD as f64).contains(&rung),
                "{rung}"
            );
            assert!(duel_candidates(&m.screen, &lo, &m.rules) >= MIN_SOLUTIONS);
        }
        assert!(spots.len() > 1, "seed {seed}: the guns never moved");
        all.extend(spots);
        // Pure: the same match rolls the same way.
        let mut again = Match::new(seed, 320);
        again.turn = 0;
        again.screen.wind = turn_wind(seed, 0, 0, &again.rules);
        let mut first = Match::new(seed, 320);
        first.turn = 0;
        first.screen.wind = again.screen.wind;
        assert_eq!(again.roll_guns().is_some(), first.roll_guns().is_some());
        assert_eq!(again.screen.muzzle_x, first.screen.muzzle_x);
    }
    // Both ways get used: some rolls go back past home, some forward.
    assert!(all.iter().any(|&x| x < EMPLACEMENT_X), "never rolled back");
    assert!(
        all.iter().any(|&x| x > EMPLACEMENT_X),
        "never rolled forward"
    );
}

#[test]
fn rolled_guns_are_seated_alike_on_fresh_ground() {
    // Crater a long stretch in front of the right gun, where its next spot
    // forward is, as a run of hits around it would. Without filling, the
    // right gun would roll down into it.
    let mut m = Match::new(0x5A17, 320);
    let w = m.width();
    // Deep enough under the right gun's spots to reach rock.
    for _ in 0..3 {
        for x in (230..=300).step_by(6) {
            m.screen.field.crater(x as f64, 7.0);
        }
    }
    assert!(m
        .screen
        .field
        .material
        .iter()
        .any(|&mat| mat != Material::Sand));
    for x in (230..=300).step_by(6) {
        m.screen.field.crater(x as f64, 7.0);
    }
    let base = Heightfield::from_base(m.screen.field.base.clone());
    let mut rolled = 0;
    for turn in 0..8u32 {
        m.turn = turn;
        if m.roll_guns().is_none() {
            continue;
        }
        rolled += 1;
        let (l, r) = (
            muzzle_of(&m.screen, Side::Left),
            muzzle_of(&m.screen, Side::Right),
        );
        assert_eq!(r.x, mirror_x(l.x, w));
        // Same rule, mirrored: the right gun reads the mirrored field
        // exactly as the left reads the shared one.
        let mirrored = frame_for(&m.screen, Side::Right);
        assert_eq!(r.y.to_bits(), mirrored.muzzle().y.to_bits());
        // Both stand on the ground as generated, so they differ only by
        // what is left of its asymmetry, as they did before any shot.
        let mut bare = m.screen.clone();
        bare.field = base.clone();
        for side in Side::BOTH {
            assert_eq!(
                muzzle_of(&m.screen, side).y.to_bits(),
                muzzle_of(&bare, side).y.to_bits(),
                "{side:?} seated in a crater"
            );
        }
        assert!(
            (l.y - r.y).abs() < 5.0,
            "pivots {} apart",
            (l.y - r.y).abs()
        );
        // The filled columns are fresh ground through and through: Sand,
        // the terrain's rule for no scar, under both guns alike.
        let x0 = (m.screen.muzzle_x - CHASSIS_WIDTH as f64 * 0.5) as usize;
        for i in x0..x0 + CHASSIS_WIDTH {
            for c in [i, w - 1 - i] {
                assert_eq!(m.screen.field.scar[c], 0.0);
                assert_eq!(m.screen.field.material[c], Material::Sand, "column {c}");
            }
        }
    }
    assert!(rolled > 0);
    // Craters away from the guns are left alone.
    assert!(m.screen.field.scar.iter().any(|s| *s > 0.0));
}

#[test]
fn a_new_round_puts_the_guns_back() {
    let mut m = Match::new(0x5A17, 320);
    while m.phase == Phase::Aiming {
        let shot = hitting_shot(&m, m.to_move);
        m.fire(if m.to_move == Side::Left { shot } else { MISS }, false);
    }
    assert!(m.next_round());
    assert_eq!(m.screen.muzzle_x, EMPLACEMENT_X);
    assert!(!m.roll_due);
}

#[test]
fn craters_stay_for_the_round() {
    let mut m = Match::new(3, 320);
    m.fire(Shot::new(600, 500), false).unwrap();
    let after_one: f64 = m.screen.field.scar.iter().sum();
    assert!(after_one > 0.0);
    m.fire(Shot::new(600, 500), false).unwrap();
    let after_two: f64 = m.screen.field.scar.iter().sum();
    assert!(
        after_two > after_one,
        "the second shot's crater joins the first"
    );
}

// ------------------------------------------------------------- generation

#[test]
fn every_duel_screen_is_solvable_from_both_sides() {
    let lo = Loadout::new();
    for width in [320, 360, SCREEN_W_MAX] {
        for seed in 0..12u64 {
            for round in 0..3 {
                let s = generate_duel(seed * 7919, round, width, &lo, &Rules::DEFAULT);
                assert!(
                    s.candidates >= MIN_SOLUTIONS,
                    "seed {seed} round {round} width {width}: {} shots",
                    s.candidates
                );
                // Checked again here rather than trusted: each gun, each
                // gate wind, its own count.
                for wind in [-WIND_MAX, 0.0, WIND_MAX] {
                    let mut shared = s.clone();
                    shared.wind = wind;
                    for side in Side::BOTH {
                        let frame = frame_for(&shared, side);
                        let n = shots_at(
                            &frame,
                            target_in_frame(&shared, side),
                            &lo,
                            HIT_REACH,
                            GATE_CAP,
                        )
                        .len();
                        assert!(n >= MIN_SOLUTIONS, "{side:?} in wind {wind}: {n}");
                    }
                }
            }
        }
    }
}

#[test]
fn duel_generation_is_pure_and_mostly_draws_templates() {
    let lo = Loadout::new();
    let a = generate_duel(42, 1, 340, &lo, &Rules::DEFAULT);
    let b = generate_duel(42, 1, 340, &lo, &Rules::DEFAULT);
    let bits = |s: &Screen| s.field.base.iter().map(|h| h.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
    assert_eq!(a.template, b.template);
    let drawn = (0..24u64)
        .filter(|&seed| generate_duel(seed, 0, 320, &lo, &Rules::DEFAULT).rung == Some(Rung::Drawn))
        .count();
    assert!(drawn >= 20, "only {drawn} of 24 screens drew a template");
}

#[test]
fn level_ground_is_solvable_by_construction() {
    let lo = Loadout::new();
    for width in 320..=400 {
        let mut s = assemble(0, 1, width, Template::Flat, true);
        assert!(
            gate(&mut s, &lo, &Rules::DEFAULT),
            "width {width}: {}",
            s.candidates
        );
    }
}

#[test]
fn duel_ground_is_near_symmetric() {
    let lo = Loadout::new();
    for seed in 0..8u64 {
        let s = generate_duel(seed, 0, 320, &lo, &Rules::DEFAULT);
        let base = &s.field.base;
        let n = base.len();
        // What is left of any asymmetry is a tenth of what the generator
        // made, and the generator stays within the world's height.
        for i in 0..n {
            assert!((base[i] - base[n - 1 - i]).abs() <= 0.1 * 96.0 + 1e-9);
        }
    }
    let mut h = vec![10.0, 20.0, 50.0, 90.0];
    symmetrize(&mut h);
    let gap = |h: &[f64]| (h[0] - h[3]).abs();
    assert!((gap(&h) - 0.1 * 80.0).abs() < 1e-9, "{h:?}");
}

// -------------------------------------------------------------------- wind

#[test]
fn turn_wind_is_a_pure_function_of_seed_round_and_turn() {
    for seed in [0u64, 1, 0xDEAD_BEEF] {
        for round in 0..3 {
            let winds: Vec<f64> = (0..20)
                .map(|t| turn_wind(seed, round, t, &Rules::DEFAULT))
                .collect();
            for (t, w) in winds.iter().enumerate() {
                assert_eq!(
                    w.to_bits(),
                    turn_wind(seed, round, t as u32, &Rules::DEFAULT).to_bits()
                );
                assert!((-WIND_MAX..WIND_MAX).contains(w), "{w}");
            }
            // Resampled: twenty turns do not all blow the same.
            assert!(winds.iter().any(|w| (w - winds[0]).abs() > 0.1));
            assert!(winds.iter().any(|w| *w < 0.0) && winds.iter().any(|w| *w > 0.0));
        }
    }
    // Another round, or another match, is another sequence.
    assert_ne!(
        turn_wind(1, 0, 0, &Rules::DEFAULT).to_bits(),
        turn_wind(1, 1, 0, &Rules::DEFAULT).to_bits()
    );
    assert_ne!(
        turn_wind(1, 0, 0, &Rules::DEFAULT).to_bits(),
        turn_wind(2, 0, 0, &Rules::DEFAULT).to_bits()
    );
}

#[test]
fn a_wind_floor_keeps_every_turn_windy_and_zero_changes_nothing() {
    let floored = Rules {
        wind_min: 1.5,
        wind_max: 5.0,
        ..Rules::DEFAULT
    };
    let calm_floor = Rules {
        wind_min: 0.0,
        ..Rules::DEFAULT
    };
    for turn in 0..200 {
        let w = turn_wind(7, 0, turn, &floored);
        assert!((1.5..5.0).contains(&w.abs()), "{w}");
        // A floor of zero is the plain signed draw, bit for bit, so matches
        // played before the floor existed blow the same.
        let stream = substream(substream(round_seed(7, 0), TAG_WIND), turn as u64);
        assert_eq!(
            turn_wind(7, 0, turn, &calm_floor).to_bits(),
            Pcg32::new(stream).signed(WIND_MAX).to_bits()
        );
    }
}

#[test]
fn winds_do_not_depend_on_the_shots() {
    // Hit or miss, the third turn of a round blows the same.
    let mut a = Match::new(99, 320);
    let mut b = Match::new(99, 320);
    a.fire(MISS, false).unwrap();
    b.fire(hitting_shot(&b, Side::Left), false).unwrap();
    a.fire(MISS, false).unwrap();
    b.fire(MISS, false).unwrap();
    assert_eq!(a.screen.wind.to_bits(), b.screen.wind.to_bits());
}

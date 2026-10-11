//! The editor's model: every edit, the verdict and the forks.

use super::*;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use salt_line_sim::layout::HEIGHT_SCALE;

const ONE: u16 = HEIGHT_SCALE;

fn blank() -> Editor {
    Editor::new(Draft::blank())
}

fn h(e: &Editor, c: usize) -> u16 {
    e.layout().heights[c]
}

/// A row on screen `units` world units above the blank ground's surface
/// (buffer rows count down from the top; 180 rows tall).
fn row(units: f32) -> f32 {
    179.0 - BLANK_HEIGHT as f32 - units
}

#[test]
fn a_blank_draft_is_level_mirrored_fair_and_playable() {
    let e = blank();
    assert!(e.mirror, "a blank draft is symmetric, so MIRROR starts on");
    assert_eq!(e.layout().width(), LAYOUT_WIDTH);
    assert!(e
        .layout()
        .heights
        .iter()
        .all(|&v| v == height_steps(BLANK_HEIGHT)));
    assert_eq!(e.layout().guns, vec![30, 289]);
    assert!(e.verdict.playable(), "{:?}", e.verdict);
    assert_eq!(e.verdict.fairness, Some(Fairness::Fair));
    assert!(!e.can_undo());
    assert_eq!(e.draft.label(), "UNNAMED");
}

#[test]
fn sculpt_raises_in_whole_units_and_is_reversible_within_the_stroke() {
    let mut e = blank();
    e.mirror = false;
    let flat = height_steps(BLANK_HEIGHT);
    e.begin_stroke(100.0, row(0.0));
    e.drag_stroke(100.0, row(20.0));
    assert_eq!(h(&e, 100), flat + 20 * ONE, "the centre follows the finger");
    let r = e.brush.radius() as usize;
    assert!(
        h(&e, 100 - r / 2) > flat && h(&e, 100 - r / 2) < h(&e, 100),
        "soft"
    );
    assert_eq!(h(&e, 100 + r + 2), flat, "nothing past the brush");
    assert!(
        e.layout()
            .heights
            .iter()
            .all(|&v| (v - flat).is_multiple_of(ONE)),
        "whole units"
    );
    assert!(
        e.layout().heights[200..].iter().all(|&v| v == flat),
        "MIRROR off"
    );
    // Sliding sideways paints the hill along.
    e.drag_stroke(140.0, row(20.0));
    assert_eq!(h(&e, 120), flat + 20 * ONE);
    // Back to where it started: nothing changed, and no undo step left.
    e.drag_stroke(140.0, row(0.0));
    e.end_stroke();
    assert!(e.layout().heights.iter().all(|&v| v == flat));
    assert!(!e.can_undo(), "a stroke that changed nothing is forgotten");
    // A real stroke is one undo step, however far it went.
    e.begin_stroke(60.0, row(0.0));
    for y in 0..30 {
        e.drag_stroke(60.0 + y as f32, row(-(y as f32)));
    }
    e.end_stroke();
    assert_eq!(e.undo_depth(), 1);
    assert!(h(&e, 60) < flat);
    assert!(e.undo());
    assert!(e.layout().heights.iter().all(|&v| v == flat));
}

#[test]
fn the_ground_never_leaves_the_world() {
    let mut e = blank();
    e.brush = Brush::Large;
    e.begin_stroke(160.0, row(0.0));
    e.drag_stroke(160.0, -500.0);
    assert_eq!(h(&e, 160), height_steps(WORLD_CEIL));
    e.drag_stroke(160.0, 900.0);
    assert_eq!(h(&e, 160), height_steps(WORLD_FLOOR));
    e.end_stroke();
    for _ in 0..200 {
        e.nudge(-1);
    }
    assert!(e
        .layout()
        .heights
        .iter()
        .all(|&v| (height_steps(WORLD_FLOOR)..=height_steps(WORLD_CEIL)).contains(&v)));
}

#[test]
fn mirror_makes_every_edit_on_both_sides() {
    let mut e = blank();
    assert!(e.mirror);
    for tool in [Tool::Sculpt, Tool::Smooth, Tool::Stamp] {
        e.set_tool(tool);
        e.begin_stroke(70.0, row(0.0));
        e.drag_stroke(90.0, row(15.0));
        e.end_stroke();
        assert_eq!(e.layout().asymmetry(), 0.0, "{tool:?}");
    }
    e.set_tool(Tool::Stamp);
    e.stamp = Stamp::Spires;
    e.cursor = 250;
    e.nudge(1);
    assert_eq!(e.layout().asymmetry(), 0.0);
    // Off, an edit is one side's alone; on again makes the draft
    // symmetric, as one undoable step.
    e.toggle_mirror();
    e.set_tool(Tool::Sculpt);
    e.begin_stroke(40.0, row(0.0));
    e.drag_stroke(40.0, row(30.0));
    e.end_stroke();
    assert!(e.layout().asymmetry() > 0.0);
    let lopsided = e.layout().clone();
    let depth = e.undo_depth();
    e.toggle_mirror();
    assert_eq!(e.layout().asymmetry(), 0.0);
    assert_eq!(e.undo_depth(), depth + 1);
    assert!(e.undo());
    assert_eq!(e.layout(), &lopsided);
}

#[test]
fn smooth_wears_a_spike_down() {
    let mut e = blank();
    e.mirror = false;
    e.draft.layout.heights[100] = height_steps(90.0);
    let spike = h(&e, 100);
    e.set_tool(Tool::Smooth);
    e.begin_stroke(100.0, 50.0);
    for _ in 0..6 {
        e.drag_stroke(100.0, 50.0);
        e.drag_stroke(101.0, 50.0);
    }
    e.end_stroke();
    assert!(h(&e, 100) < spike - 10 * ONE, "{} from {spike}", h(&e, 100));
    assert!(
        h(&e, 99) > height_steps(BLANK_HEIGHT),
        "spread to its neighbours"
    );
}

#[test]
fn every_stamp_is_the_generators_shape_where_it_is_dropped() {
    for stamp in Stamp::ALL {
        let mut e = blank();
        e.mirror = false;
        e.set_tool(Tool::Stamp);
        e.stamp = stamp;
        e.begin_stroke(160.0, 50.0);
        e.end_stroke();
        let flat = height_steps(BLANK_HEIGHT) as i32;
        let d: Vec<i32> = e
            .layout()
            .heights
            .iter()
            .map(|&v| v as i32 - flat)
            .collect();
        let changed: Vec<usize> = (0..d.len()).filter(|&c| d[c] != 0).collect();
        assert!(!changed.is_empty(), "{stamp:?}");
        let (lo, hi) = (changed[0], changed[changed.len() - 1]);
        assert!(lo <= 160 && hi >= 160, "{stamp:?}: {lo}..{hi}");
        // The feature is where it was dropped: a crest or a top, or a gap
        // or a floor.
        match stamp {
            Stamp::Chasm | Stamp::Basin => assert!(d[160] < 0, "{stamp:?}"),
            _ => assert!(d[160] > 0, "{stamp:?}"),
        }
        assert_eq!(e.undo_depth(), 1);
    }
    // Dragging a stamp moves it rather than stamping a trail.
    let mut e = blank();
    e.mirror = false;
    e.set_tool(Tool::Stamp);
    e.stamp = Stamp::Mesa;
    e.begin_stroke(100.0, 50.0);
    e.drag_stroke(200.0, 50.0);
    e.end_stroke();
    let mut once = blank();
    once.mirror = false;
    once.set_tool(Tool::Stamp);
    once.stamp = Stamp::Mesa;
    once.begin_stroke(200.0, 50.0);
    once.end_stroke();
    assert_eq!(e.layout(), once.layout());
}

#[test]
fn guns_keep_their_gap_their_edge_and_their_seat() {
    let mut e = blank();
    e.set_tool(Tool::Guns);
    // Dragged right as far as it goes: the pair stops MIN_GUN_GAP apart.
    e.begin_stroke(30.0, 140.0);
    e.drag_stroke(300.0, 140.0);
    e.end_stroke();
    let [l, r] = [e.layout().guns[0], e.layout().guns[1]];
    assert_eq!(l as usize + r as usize, LAYOUT_WIDTH - 1, "mirrored");
    assert!(r - l >= MIN_GUN_GAP && r - l < MIN_GUN_GAP + 2, "{l} {r}");
    // And left: the edge margin.
    e.begin_stroke(l as f32, 140.0);
    e.drag_stroke(-50.0, 140.0);
    e.end_stroke();
    assert_eq!(e.layout().guns[0], GUN_EDGE_MARGIN);
    assert!(e.verdict.playable(), "{:?}", e.verdict.problems);
    // A cliff it cannot sit on: the gun stops at the last spot it can.
    let mut e = blank();
    for c in 60..LAYOUT_WIDTH - 60 {
        e.draft.layout.heights[c] = height_steps(WORLD_FLOOR);
    }
    e.draft.layout.heights[..60].fill(height_steps(100.0));
    e.draft.layout.heights[LAYOUT_WIDTH - 60..].fill(height_steps(100.0));
    e.set_tool(Tool::Guns);
    e.begin_stroke(30.0, 50.0);
    e.drag_stroke(130.0, 50.0);
    e.end_stroke();
    let g = e.layout().guns[0];
    assert!((50..72).contains(&g), "stopped at the cliff edge, at {g}");
    assert!(
        !e.verdict
            .problems
            .iter()
            .any(|(p, _)| matches!(p, Problem::GunNotSeated(_))),
        "{:?}",
        e.verdict.problems
    );
    // MIRROR off: one gun alone, and the verdict says so.
    let mut e = blank();
    e.toggle_mirror();
    e.set_tool(Tool::Guns);
    e.begin_stroke(289.0, 50.0);
    e.drag_stroke(250.0, 50.0);
    e.end_stroke();
    assert_eq!(e.layout().guns, vec![30, 250]);
    assert!(!e.verdict.playable());
    assert!(e
        .verdict
        .problems
        .iter()
        .any(|(p, _)| *p == Problem::GunsNotMirrored));
    // With keys the cursor drags the left gun, one undo step a run.
    let mut e = blank();
    e.set_tool(Tool::Guns);
    for _ in 0..10 {
        e.move_cursor(1);
    }
    assert_eq!(e.layout().guns, vec![40, 279]);
    assert_eq!(e.undo_depth(), 1);
    for _ in 0..40 {
        e.move_cursor(-1);
    }
    assert_eq!(e.layout().guns[0], GUN_EDGE_MARGIN);
}

#[test]
fn undo_keeps_sixty_four_steps_and_a_new_edit_clears_redo() {
    let mut e = blank();
    e.mirror = false;
    for i in 0..70 {
        e.cursor = (20 + i * 4) as u16;
        e.nudge(1);
    }
    assert_eq!(e.undo_depth(), UNDO_DEPTH);
    let after = e.layout().clone();
    for _ in 0..UNDO_DEPTH {
        assert!(e.undo());
    }
    assert!(!e.undo(), "the oldest steps are gone");
    assert!(e.can_redo());
    for _ in 0..UNDO_DEPTH {
        assert!(e.redo());
    }
    assert_eq!(e.layout(), &after);
    e.undo();
    e.cursor = 3;
    e.nudge(1);
    assert!(!e.can_redo(), "a new edit forgets what was undone");
    // A run of nudges at one spot is one step.
    let mut e = blank();
    for _ in 0..5 {
        e.nudge(1);
    }
    assert_eq!(e.undo_depth(), 1);
    e.move_cursor(4);
    e.nudge(1);
    assert_eq!(e.undo_depth(), 2);
}

#[test]
fn wind_steps_in_tenths_and_keeps_a_forks_thousandths() {
    let fixed = WindSpec::Fixed(1234);
    assert_eq!(step_wind(fixed, WindField::Fixed, 1), WindSpec::Fixed(1334));
    assert_eq!(
        step_wind(fixed, WindField::Fixed, -20),
        WindSpec::Fixed(-766)
    );
    assert_eq!(
        step_wind(WindSpec::Fixed(6950), WindField::Fixed, 1),
        WindSpec::Fixed(MAX_WIND)
    );
    let range = WindSpec::Range { min: 500, max: 900 };
    assert_eq!(
        step_wind(range, WindField::Min, 5),
        WindSpec::Range {
            min: 1000,
            max: 1000
        },
        "the ends stay in order"
    );
    assert_eq!(
        step_wind(range, WindField::Max, -9),
        WindSpec::Range { min: 0, max: 0 }
    );
    assert_eq!(
        wind_as(fixed, WindMode::Range),
        WindSpec::Range { min: 0, max: 1234 }
    );
    assert_eq!(wind_as(range, WindMode::Fixed), WindSpec::Fixed(900));
    assert_eq!(
        wind_as(WindSpec::Calm, WindMode::Fixed),
        WindSpec::Fixed(1000)
    );
    assert_eq!(wind_text(1234), "1.234");
    assert_eq!(wind_text(-2500), "-2.5");

    let mut e = blank();
    e.set_wind_mode(WindMode::Fixed);
    assert_eq!(e.layout().wind, WindSpec::Fixed(2500));
    e.step_wind(-3);
    e.step_wind(-3);
    assert_eq!(e.layout().wind, WindSpec::Fixed(1900));
    e.set_wind_mode(WindMode::Calm);
    assert_eq!(e.layout().wind, WindSpec::Calm);
    assert_eq!(e.undo_depth(), 1, "a run of WIND presses is one step");
    assert_eq!(e.test_winds(), vec![0]);
    e.undo();
    assert_eq!(e.test_winds(), vec![-2500, 0, 2500]);
}

#[test]
fn a_name_is_keypad_characters_only() {
    let mut e = blank();
    assert!(e.set_name("ME"));
    assert!(e.set_name("MESA9"));
    assert!(!e.set_name("NO SPACES"));
    assert!(!e.set_name("THIRTEENCHARS"));
    assert_eq!(e.layout().name.as_str(), "MESA9");
    assert_eq!(e.draft.label(), "MESA9");
    assert_eq!(e.undo_depth(), 1);
}

// ---------------------------------------------------------------- verdict

/// The right gun at the bottom of a pit walled to the top of the world.
fn pit() -> Editor {
    let mut e = blank();
    e.mirror = false;
    for c in 288 - 19..288 + 20 {
        let floor = (276..303).contains(&c);
        e.draft.layout.heights[c] = height_steps(if floor { 40.0 } else { 108.0 });
    }
    e.changed();
    e
}

#[test]
fn the_verdict_names_what_is_wrong_and_who_is_harder() {
    let e = pit();
    assert!(!e.verdict.playable());
    let words: Vec<String> = e
        .verdict
        .problems
        .iter()
        .map(|(p, _)| problem_words(p))
        .collect();
    assert!(
        words.iter().any(|w| w == "RIGHT CAN'T HIT - CALM"),
        "{words:?}"
    );
    assert!(
        words.iter().any(|w| w == "RIGHT CAN'T HIT - WIND 2.5"),
        "{words:?}"
    );
    assert!(e
        .verdict
        .problems
        .iter()
        .all(|(p, s)| *s == Severity::Block && problem_gun(p).is_some()));
    // The gate's counts say who has it harder: here, the gun in the pit.
    assert_eq!(e.verdict.counts.len(), 3, "calm and the range both ways");
    assert_eq!(e.verdict.fairness, Some(Fairness::RightHarder));
    assert_eq!(
        fairness(&[(0, [40, 40]), (2500, [41, 37])]),
        Some(Fairness::Fair),
        "a shot or two either way is noise"
    );
    assert_eq!(
        fairness(&[(0, [40, 30]), (2500, [40, 37])]),
        Some(Fairness::RightHarder)
    );
    assert_eq!(fairness(&[(0, [2, 8])]), Some(Fairness::LeftHarder));
    assert_eq!(fairness(&[(0, [1, 2])]), Some(Fairness::Fair));
    assert_eq!(fairness(&[]), None);
}

/// A floating structure is a warning, not a block (owner decision): a fork
/// of a run screen whose generator stood a target on a spire can still be
/// saved, as long as the target can be hit.
#[test]
fn a_floating_structure_only_warns() {
    let floating = (0..400u64)
        .flat_map(|seed| (4..16).map(move |s| (seed, s)))
        .map(|(seed, s)| Layout::from_run_screen(seed, s))
        .find(|l| {
            let p = check_placement(l);
            p.len() == 1 && matches!(p[0], Problem::StructureFloating(_))
        })
        .expect("the generator floats a target in about one screen in ten");
    assert_eq!(severity(&Problem::StructureFloating(0)), Severity::Warn);
    let v = verdict(&floating);
    assert!(v.warns());
    assert!(v.playable(), "{:?}", v.problems);
    assert!(
        layout::validate(&floating).is_err(),
        "the sim's rule still reports it"
    );
}

// ------------------------------------------------------------------ forks

#[test]
fn fork_this_round_keeps_both_guns_whatever_the_width() {
    let ramp = |w: usize| -> Vec<f64> { (0..w).map(|c| 20.0 + c as f64 * 0.2).collect() };
    // At 320 it is the screen itself.
    let (h, g) = crop_live(&ramp(320), 30.0);
    assert_eq!((h.len(), g), (320, 30));
    assert_eq!(h[100], height_steps(40.0));
    // Wide, guns home: they cannot both fit in the middle 320, so each
    // keeps its own side; the middle is lost and the seam eased.
    let base = ramp(400);
    let (h, g) = crop_live(&base, 30.0);
    assert_eq!((h.len(), g), (320, 30));
    assert_eq!(h[30], height_steps(base[30]), "the left gun's ground");
    assert_eq!(h[289], height_steps(base[369]), "the right gun's ground");
    let step = (h[160] as i32 - h[159] as i32).abs();
    assert!(step <= ONE as i32, "no cliff at the join: {step}");
    // Guns rolled forward on a wide buffer fit: the centred window.
    let (h, g) = crop_live(&base, 62.0);
    assert_eq!(g, 62 - 40);
    assert_eq!(h[0], height_steps(base[40]));
    // An odd difference still gives mirror images.
    let screen =
        crate::versus::generate_duel(5, 1, 333, &Loadout::new(), &crate::versus::Rules::DEFAULT);
    let d = fork_live(&screen, 5, 1, WindSpec::Range { min: 0, max: 2500 });
    let l = &d.layout;
    assert_eq!(l.width(), LAYOUT_WIDTH);
    assert_eq!(l.guns[0] as usize + l.guns[1] as usize, LAYOUT_WIDTH - 1);
    assert!(
        !check_placement(l)
            .iter()
            .any(|p| matches!(p, Problem::GunOffScreen(_) | Problem::GunNotSeated(_))),
        "{:?}",
        check_placement(l)
    );
    assert_eq!(d.label(), "FROM MATCH R2");
}

#[test]
fn a_live_fork_at_the_layout_width_is_the_fork_of_the_seed() {
    let screen =
        crate::versus::generate_duel(9, 0, 320, &Loadout::new(), &crate::versus::Rules::DEFAULT);
    let live = fork_live(&screen, 9, 0, WindSpec::Range { min: 0, max: 2500 });
    assert_eq!(live.layout, custom::fork_duel(9, 0));
}

#[test]
fn a_seed_fork_is_labelled_and_plays() {
    let d = Draft::from_seed("DUNE", 1);
    assert_eq!(d.label(), "FROM SEED DUNE R2");
    assert_eq!(d.layout, custom::fork_duel(parse_seed("DUNE"), 1));
    assert!(verdict(&d.layout).playable());
}

#[test]
fn origins_survive_their_token() {
    for o in [
        Origin::Seed {
            text: "DUNE".into(),
            round: 1,
        },
        Origin::Match {
            seed: u64::MAX,
            round: 0,
            width: 400,
        },
    ] {
        assert_eq!(Origin::parse(&o.token()), Some(o));
    }
    for bad in [
        "",
        "seed",
        "seed::1",
        "seed:dune:1",
        "seed:A B:1",
        "seed:DUNE:01",
        "seed:DUNE:1:2",
        "seed:DUNE:99",
        "match:1:2",
        "match:x:1:320",
        "other:1",
    ] {
        assert_eq!(Origin::parse(bad), None, "{bad}");
    }
}

// ------------------------------------------------------------- properties

#[derive(Clone, Debug)]
enum Op {
    Stroke(Tool, f32, f32, f32, f32),
    Nudge(i32),
    Cursor(i32),
    Mirror,
    Undo,
    Redo,
    Brush(usize),
    Stamp(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let tool = prop_oneof![
        Just(Tool::Sculpt),
        Just(Tool::Smooth),
        Just(Tool::Stamp),
        Just(Tool::Guns),
    ];
    prop_oneof![
        4 => (tool, -20f32..340.0, -20f32..200.0, -20f32..340.0, -20f32..200.0)
            .prop_map(|(t, a, b, c, d)| Op::Stroke(t, a, b, c, d)),
        2 => (-1i32..=1).prop_map(Op::Nudge),
        1 => (-30i32..=30).prop_map(Op::Cursor),
        1 => Just(Op::Mirror),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => (0usize..3).prop_map(Op::Brush),
        1 => (0usize..5).prop_map(Op::Stamp),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever is done: the ground stays in the world and the layout
    /// width, MIRROR on keeps it symmetric, the guns stay on screen and
    /// apart, and undoing everything gives back the start, exactly.
    #[test]
    fn edits_keep_the_layout_sound(ops in prop::collection::vec(op(), 1..30)) {
        let mut e = blank();
        let start = e.draft.clone();
        // MIRROR on keeps the draft symmetric from the moment it is turned
        // on, until an UNDO or REDO brings back a step taken without it.
        let mut symmetric = true;
        for op in ops {
            match op {
                Op::Mirror => symmetric = !e.mirror,
                Op::Undo | Op::Redo => symmetric = false,
                _ => {}
            }
            match op {
                Op::Stroke(tool, a, b, c, d) => {
                    e.set_tool(tool);
                    e.begin_stroke(a, b);
                    e.drag_stroke(c, d);
                    e.end_stroke();
                }
                Op::Nudge(d) => e.nudge(d),
                Op::Cursor(d) => e.move_cursor(d),
                Op::Mirror => e.toggle_mirror(),
                Op::Undo => { e.undo(); }
                Op::Redo => { e.redo(); }
                Op::Brush(i) => e.brush = Brush::ALL[i],
                Op::Stamp(i) => e.stamp = Stamp::ALL[i],
            }
            let l = e.layout();
            prop_assert_eq!(l.width(), LAYOUT_WIDTH);
            prop_assert!(l.heights.iter().all(|&v| (height_steps(WORLD_FLOOR)..=height_steps(WORLD_CEIL)).contains(&v)));
            prop_assert!(l.guns[0] >= GUN_EDGE_MARGIN);
            prop_assert!(l.guns[1] as usize + (GUN_EDGE_MARGIN as usize) < LAYOUT_WIDTH);
            prop_assert!(l.guns[1] >= l.guns[0] + MIN_GUN_GAP);
            if e.mirror && symmetric {
                prop_assert_eq!(l.asymmetry(), 0.0);
                prop_assert_eq!(l.guns[0] as usize + l.guns[1] as usize, LAYOUT_WIDTH - 1);
            }
            prop_assert!(e.undo_depth() <= UNDO_DEPTH);
        }
        // Fewer steps than UNDO_DEPTH: every one comes back off.
        while e.undo() {}
        prop_assert_eq!(e.draft, start);
    }
}

//! The layered frame must equal the flat frame, pixel for pixel. See the
//! module docs for why it should; these check that it does.

use super::*;
use crate::app::{Game, Mode};
use crate::cli::{DevArgs, PHONE_WINDOW};
use crate::input::Intents;
use crate::render::{frame, Art, Cache};
use crate::save_format::Save;
use crate::touch::Button;
use salt_line_sim::modifiers::Modifier;
use salt_line_sim::Vec2;

const DESKTOP: (i32, i32) = (1280, 720);

/// One game drawn both ways every frame, with a canvas and a cache for
/// each kept across frames as the node keeps them, so stale pixels and
/// stale layers would show.
struct Both {
    art: Art,
    flat: (Canvas, Cache, Vec<Button>),
    layered: (Canvas, Cache, Vec<Button>),
    /// Frames checked, and how many of them were layered.
    frames: usize,
    shown: usize,
    outlined: usize,
}

impl Both {
    fn new() -> Self {
        let mut layered = Cache::default();
        layered.backdrop.layered = true;
        Both {
            art: Art::default(),
            flat: (Canvas::new(1, 1), Cache::default(), Vec::new()),
            layered: (Canvas::new(1, 1), layered, Vec::new()),
            frames: 0,
            shown: 0,
            outlined: 0,
        }
    }

    /// Draw `g` both ways and insist they agree.
    fn check(&mut self, g: &Game, what: &str) {
        let (w, h) = (g.display.buffer_w, g.display.buffer_h);
        let (fc, fcache, fb) = &mut self.flat;
        fc.resize(w, h);
        frame(g, &self.art, fc, fb, fcache);
        let (lc, lcache, lb) = &mut self.layered;
        lc.resize(w, h);
        frame(g, &self.art, lc, lb, lcache);

        let backdrop = &lcache.backdrop;
        assert!(
            lc.bytes()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 0 || p[3] == 255),
            "{what}: the overlay has a pixel neither transparent nor opaque"
        );
        let got = backdrop.composite(lc);
        if got != fc.bytes() {
            let i = got
                .as_chunks::<4>()
                .0
                .iter()
                .zip(fc.bytes().as_chunks::<4>().0)
                .position(|(a, b)| a != b)
                .unwrap_or(0) as i32;
            panic!(
                "{what}, frame {}: layered differs from flat first at ({}, {}) in a {w}x{h} buffer \
                 (cam_x {:?})",
                self.frames,
                i % w,
                i / w,
                backdrop.shown(),
            );
        }
        assert_eq!(fb.len(), lb.len(), "{what}: the same buttons");
        self.frames += 1;
        self.shown += usize::from(backdrop.shown().is_some());
        self.outlined += usize::from(backdrop.shown().is_some_and(|s| s.outline));
    }
}

fn intents(window: (i32, i32)) -> Intents {
    Intents {
        window,
        today: 20_000,
        ..Intents::default()
    }
}

/// Play `g` for `seconds` at `fps`, checking every frame.
fn play(both: &mut Both, g: &mut Game, window: (i32, i32), seconds: f64, fps: f64, what: &str) {
    for _ in 0..(seconds * fps) as usize {
        g.tick(1.0 / fps, &intents(window));
        both.check(g, what);
    }
}

fn game(window: (i32, i32), touch: bool) -> Game {
    let mut g = Game::new(touch, Save::default());
    g.fit_display(window, (0, 0));
    g.input.touch_active = touch;
    g
}

/// The autopilot's run, as the profiling build plays it, at a phone's
/// width with touch controls and at a desktop's without: shots, craters,
/// collapses, the landing shake (to both sides of the world's left edge),
/// screen clears and drafts.
#[test]
fn an_autopiloted_run_is_the_same_layered() {
    for (window, touch, seed, miss) in [
        (PHONE_WINDOW, true, 20261004, false),
        (DESKTOP, false, 7, true),
    ] {
        let mut both = Both::new();
        let mut g = game(window, touch);
        let mut args = DevArgs::parse(&[]);
        args.seed = Some(seed);
        args.auto = true;
        args.miss = miss;
        g.start_dev(&args, 20_000);
        play(&mut both, &mut g, window, 40.0, 30.0, "run");
        assert!(both.shown > both.frames / 2, "most frames were layered");
        assert!(both.outlined > 0, "a collapse flashed the outline");
    }
}

/// Dusk, night (stars, the shooting star) and dawn, with the modifiers
/// that draw over the ground (Sapper's Eye, Chalk Marks, Plumb Line) and
/// Long Lens's taller buffer.
#[test]
fn later_screens_and_modifiers_are_the_same_layered() {
    for (screen, lens) in [(3, false), (6, true), (9, false)] {
        let mut both = Both::new();
        let mut g = game(PHONE_WINDOW, true);
        let mut args = DevArgs::parse(&[]);
        args.seed = Some(42);
        args.screen = Some(screen);
        args.auto = true;
        args.miss = true;
        g.start_dev(&args, 20_000);
        let run = g.play.run.as_mut().expect("a run");
        for m in [
            Modifier::SappersEye,
            Modifier::ChalkMarks,
            Modifier::PlumbLine,
        ] {
            run.loadout.take(m);
        }
        if lens {
            run.loadout.take(Modifier::LongLens);
        }
        g.fit_display(PHONE_WINDOW, (0, 0));
        if lens {
            assert!(
                g.display.buffer_h > 180,
                "Long Lens makes the buffer taller"
            );
        }
        // Long enough for the night's shooting star (one per ~23 s).
        play(&mut both, &mut g, PHONE_WINDOW, 30.0, 30.0, "later screens");
        assert!(both.shown > 0);
    }
}

/// A versus match played by the autopilot: both guns, aim help's last
/// arcs (the one translucent mark, over the ground), hits and their shake
/// (the camera to the left of the world), and the handoff plates.
#[test]
fn an_autopiloted_match_is_the_same_layered() {
    let mut both = Both::new();
    let mut g = game(PHONE_WINDOW, true);
    let mut args = DevArgs::parse(&[]);
    args.versus = true;
    args.auto = true;
    g.start_dev(&args, 20_000);
    play(&mut both, &mut g, PHONE_WINDOW, 40.0, 30.0, "versus");
    assert!(both.shown > 0);
}

/// The attract demo: a run leg and a duel leg, with the card and banner.
#[test]
fn the_demo_is_the_same_layered() {
    let mut both = Both::new();
    let mut g = game(DESKTOP, false);
    g.start_demo(crate::app::DemoStarted::Menu);
    play(&mut both, &mut g, DESKTOP, 60.0, 15.0, "demo");
    assert!(both.shown > 0);
}

/// A duel round's layout (`just layouts fork-duel 42:0`), for the editor.
const LAYOUT: &str = "SL06G08004W91X4408Y0FT94J88244WS442BBPXEYZXZQVEJCZ7777SYFSW49225Z7WZ7SWSS0VFQFVZEGC2244C01H08G10R24G814CC0M2880186673G604050Q340SCM1332RG4928S0600100HG0R663VZQXZQFEMKKSWZKYFZ74928RSWZWZSYF77SYE8VVYZXZXZEVQDPTP0S77644C469400QKQ";

/// The map editor's editing screen and its test fire.
#[test]
fn the_editor_is_the_same_layered() {
    for page in [None, Some("test")] {
        let mut both = Both::new();
        let mut g = game(PHONE_WINDOW, true);
        let mut args = DevArgs::parse(&[]);
        args.editor = true;
        args.layout = Some(LAYOUT.into());
        args.editor_page = page.map(String::from);
        g.start_dev(&args, 20_000);
        assert_eq!(g.mode, Mode::Editor);
        play(&mut both, &mut g, PHONE_WINDOW, 3.0, 30.0, "editor");
        assert!(both.shown > 0, "{page:?} has a backdrop");
    }
}

/// Menus have no backdrop: the overlay is the whole frame.
#[test]
fn a_menu_is_drawn_whole() {
    let mut both = Both::new();
    let g = game(DESKTOP, false);
    both.check(&g, "title");
    assert_eq!(both.layered.1.backdrop.shown(), None);
}

/// Every ground of the gallery (each shape template, clean and damaged),
/// seen from every camera position from three columns before the world to
/// three past it, with a last arc blended across it: every edge column
/// against every kind of ground. Some of these need an edge patch, which is
/// the point.
#[test]
fn every_camera_over_every_ground_is_the_same_layered() {
    let pal = Palette::default();
    let mut patched = 0;
    for w in [320, 400] {
        for (name, run) in Game::gallery(w as usize) {
            let Some(run) = run else { continue };
            let screen = &run.screen;
            let len = screen.field.len() as i32;
            let guns = [screen.muzzle_x];
            let mut backdrop = Backdrop {
                layered: true,
                ..Backdrop::default()
            };
            let arc: Vec<Vec2> = (0..len)
                .map(|x| Vec2::new(x as f64, 60.0 + (x as f64 / 9.0).sin() * 50.0))
                .collect();
            for cam_x in -3..=(len - w + 3) {
                let view = View { cam_x, w, h: 180 };
                let phase = sky::Phase::for_screen(cam_x.unsigned_abs() % 12);
                let mut flat = Canvas::new(w, 180);
                Backdrop::default().draw(&mut flat, &view, &pal, phase, 5, 1.0, screen, &guns);
                raster::draw_last_arc(&mut flat, &view, &arc, &pal);

                let mut overlay = Canvas::new(w, 180);
                backdrop.begin_frame();
                backdrop.draw(&mut overlay, &view, &pal, phase, 5, 1.0, screen, &guns);
                patched += backdrop.patches.len();
                raster::draw_last_arc_on(&mut overlay, &view, &arc, &pal, |x, y| {
                    backdrop.under(x, y)
                });
                assert!(
                    backdrop.composite(&overlay) == flat.bytes(),
                    "{name} at width {w}, cam_x {cam_x}"
                );
            }
        }
    }
    assert!(patched > 0, "the sweep exercised the edge patches");
}

/// A layer is drawn again only when what it shows changes.
#[test]
fn layers_are_redrawn_only_when_their_inputs_change() {
    let pal = Palette::default();
    let run = Game::gallery(320).into_iter().find_map(|(_, r)| r).unwrap();
    let mut screen = run.screen.clone();
    let guns = [screen.muzzle_x];
    let mut b = Backdrop {
        layered: true,
        ..Backdrop::default()
    };
    let mut c = Canvas::new(320, 180);
    let mut draw = |b: &mut Backdrop, screen: &Screen, cam_x: i32, phase: sky::Phase| {
        let view = View {
            cam_x,
            w: 320,
            h: 180,
        };
        b.begin_frame();
        b.draw(&mut c, &view, &pal, phase, 1, 0.0, screen, &guns);
        (b.sky().1, b.ground().1)
    };
    let first = draw(&mut b, &screen, 0, sky::Phase::Day);
    assert_eq!(
        draw(&mut b, &screen, 2, sky::Phase::Day),
        first,
        "a shake uploads nothing"
    );
    let night = draw(&mut b, &screen, 0, sky::Phase::Night);
    assert_eq!(night, (first.0 + 1, first.1), "a new phase: the sky only");
    screen.field.scar[100] += 3.0;
    assert_eq!(
        draw(&mut b, &screen, 0, sky::Phase::Night),
        (night.0, night.1 + 1),
        "a crater: the ground only"
    );
}

#[test]
fn the_ground_window_never_reads_outside_the_layer() {
    assert_eq!(
        window(0, 320, 322),
        Some(Window {
            src_x: 0,
            dst_x: 0,
            w: 320
        })
    );
    // Shaken left of the world: the first buffer columns show no ground.
    assert_eq!(
        window(-2, 320, 322),
        Some(Window {
            src_x: 0,
            dst_x: 2,
            w: 318
        })
    );
    // Past the world's end.
    assert_eq!(
        window(5, 320, 322),
        Some(Window {
            src_x: 5,
            dst_x: 0,
            w: 317
        })
    );
    assert_eq!(window(400, 320, 322), None);
    assert_eq!(window(-320, 320, 322), None);
}

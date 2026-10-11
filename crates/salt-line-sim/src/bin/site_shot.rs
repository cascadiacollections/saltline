//! Emit a marketing-site shot verified by the real solver and damage model.
//!
//! Input on stdin is:
//! `width height wind target_x`, followed by `width` screen-space terrain heights.
//!
//! `tools/build_site.py` drives it. The terrain arrives in screen space (y
//! down) and is flipped into world elevations. A leaning water tower,
//! already at the Leaning threshold, is seated just right of `target_x`, and
//! every solver candidate that fells it is replayed with the real damage
//! model. The one with the highest apex that still stays at least 22 px
//! below the top of the frame wins, so the arc fits the image.
//!
//! Output on stdout:
//!
//! ```text
//! angle_ticks speed impact_x impact_y path_len
//! x y                      (path_len lines, world space)
//! SURFACE
//! elevation                (width lines, the cratered surface)
//! ```
//!
//! Panics with a message on malformed input or if no candidate fits.

use std::io::{self, Read};

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::run::{Rung, Screen, StructState, Structure, StructureKind};
use salt_line_sim::solver;
use salt_line_sim::terrain::{Heightfield, Template};
use salt_line_sim::EMPLACEMENT_X;

/// Next whitespace-separated token parsed as `T`; panics naming `name` if it
/// is missing or malformed.
fn value<T: std::str::FromStr>(values: &mut impl Iterator<Item = String>, name: &str) -> T {
    values
        .next()
        .unwrap_or_else(|| panic!("missing {name}"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {name}"))
}

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).expect("read stdin");
    let mut values = input.split_whitespace().map(str::to_owned);

    let width: usize = value(&mut values, "width");
    let height: f64 = value(&mut values, "height");
    let wind: f64 = value(&mut values, "wind");
    let target_x: usize = value(&mut values, "target x");
    let base: Vec<f64> = (0..width)
        .map(|_| height - value::<f64>(&mut values, "terrain height"))
        .collect();
    assert!(values.next().is_none(), "unexpected input after terrain");

    let field = Heightfield::from_base(base);
    let kind = StructureKind::WaterTower;
    let x0 = target_x + 4;
    let base = field.seat(x0, kind.footprint());
    let original_mean = (x0..x0 + kind.footprint())
        .map(|x| field.surface(x))
        .sum::<f64>()
        / kind.footprint() as f64;
    // A hand-built screen. The index and seed are placeholders: nothing below
    // regenerates terrain from them.
    let screen = Screen {
        index: 34,
        seed: 20260907,
        width,
        template: Template::Flat,
        field,
        structures: vec![Structure {
            kind,
            x0,
            base,
            original_mean,
            integrity: 0.35,
            support: 1.0,
            state: StructState::Leaning,
            lean: 1,
        }],
        wind,
        wind_late: None,
        muzzle_x: EMPLACEMENT_X,
        candidates: 0,
        attempts: 1,
        fell_back: false,
        rung: Some(Rung::Drawn),
        supply: false,
    };
    let loadout = Loadout::new();

    let selected = solver::solve_target(&screen, &loadout, 0)
        .into_iter()
        .filter_map(|candidate| {
            let mut result = screen.clone();
            let report = result.apply_shot(candidate.shot, &loadout, None, true);
            let trace = report.traces.into_iter().next()?;
            (report.collapsed.contains(&0) && trace.apex.y <= height - 22.0).then_some((
                candidate.shot,
                trace,
                result,
            ))
        })
        .max_by(|a, b| a.1.apex.y.total_cmp(&b.1.apex.y))
        .expect("no solver-verified shot fits inside the marketing frame");

    let (shot, trace, result) = selected;
    let impact = trace.end.pos();
    println!(
        "{} {:.17} {:.17} {:.17} {}",
        shot.angle_ticks,
        shot.speed(&loadout),
        impact.x,
        impact.y,
        trace.path.len()
    );
    for point in trace.path {
        println!("{:.17} {:.17}", point.x, point.y);
    }
    println!("SURFACE");
    for x in 0..width {
        println!("{:.17}", result.field.surface(x));
    }
}

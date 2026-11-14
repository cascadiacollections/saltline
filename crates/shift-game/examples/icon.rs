//! Generates Counterweight's icon from the game's own sprites: the howitzer
//! in the present on the left, the trebuchet in 1347 on the right, the two
//! eras dithered together down the middle. 64x64, written 4x.
//!
//! `cargo run --release -p shift-game --example icon -- gameoff/godot/icon.png`

use shift_game::canvas::{dither, Canvas, Rgba};
use shift_game::render::{PRESENT, SIEGE};
use shift_game::sprites::{Atlas, Ink};

const N: i32 = 64;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "gameoff/godot/icon.png".into());
    let atlas = Atlas::embedded();
    let mut c = Canvas::new();
    let ground = 48;
    for y in 0..N {
        for x in 0..N {
            // Left is the present, right is 1347; dissolve across x 24..40.
            let t = ((x - 24) as f32 / 16.0).clamp(0.0, 1.0);
            let g = if dither(x, y, t) { &SIEGE } else { &PRESENT };
            let col = if y < ground {
                let k = (y as usize * g.sky.len() / ground as usize).min(g.sky.len() - 1);
                g.sky[k]
            } else if y == ground {
                g.edge
            } else if y < ground + 7 {
                g.sand
            } else {
                g.rock
            };
            c.set(x, y, col);
        }
    }
    // The howitzer, barrel at 45 degrees (frame 9 of the 5-degree strip).
    let tank = atlas.get("tank_base");
    let (mx, my) = (12, ground - 3);
    tank.draw(&mut c, mx - 15, my - 3, false, Ink::Solid);
    atlas.get("barrel_strip_5deg").draw_region(
        &mut c,
        9 * 28,
        28,
        mx - 3,
        my - 24,
        false,
        Ink::Solid,
    );
    // The trebuchet, arm cocked.
    let fr = atlas.get("trebuchet");
    let (fx, fy) = (34, ground + 1 - fr.h);
    fr.draw(&mut c, fx, fy, false, Ink::Solid);
    atlas.get("trebuchet_arm_strip_5deg").draw_region(
        &mut c,
        0,
        48,
        fx + 15 - 24,
        fy + 3 - 24,
        false,
        Ink::Solid,
    );
    // A thin frame so it reads on light and dark backgrounds.
    let edge = Rgba::rgb(30, 28, 34);
    for i in 0..N {
        for (x, y) in [(i, 0), (i, N - 1), (0, i), (N - 1, i)] {
            c.set(x, y, edge);
        }
    }
    let mut rgb = Vec::with_capacity((N * N * 3) as usize);
    for y in 0..N {
        for x in 0..N {
            let p = c.get(x, y);
            rgb.extend_from_slice(&[p.0, p.1, p.2]);
        }
    }
    shift_game::png::write(&out, N as usize, N as usize, &rgb, 4).unwrap();
    println!("{out}");
}

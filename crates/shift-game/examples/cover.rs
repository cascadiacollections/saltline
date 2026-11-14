//! Generates the itch.io cover image (630x500): the title screen's two
//! eras dissolving, over a band with the name and tagline. Drawn at
//! 315x250 and written 2x.
//!
//! `cargo run --release -p shift-game --example cover -- <out.png>`

use shift_game::canvas::{dither, Canvas, Rgba};
use shift_game::font::{self, centred, text};
use shift_game::render::{ALERT, DARK, ECHO, INK};
use shift_game::{Renderer, H, W};

const CW: usize = 315;
const CH: usize = 250;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cover.png".into());
    // The present on the left, 1347 on the right (the title's tide at its
    // two ends), dithered together through a band down the middle.
    let mut r = Renderer::new();
    let (mut present, mut siege) = (Canvas::new(), Canvas::new());
    r.draw_title_scene(0.0, &mut present);
    r.draw_title_scene(4.0, &mut siege);
    let mut scene = Canvas::new();
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let t = ((x - 130) as f32 / 60.0).clamp(0.0, 1.0);
            let from = if dither(x, y, t) { &siege } else { &present };
            scene.set(x, y, from.get(x, y));
        }
    }
    // The band under it, drawn on its own canvas.
    let mut band = Canvas::new();
    band.px.fill(DARK);
    let title = "COUNTERWEIGHT";
    let tx = 160 - font::width(title, 3) / 2;
    text(&mut band, title, tx + 1, 9, 3, Rgba::rgb(0, 0, 0));
    text(&mut band, title, tx, 8, 3, INK);
    centred(
        &mut band,
        "ONE COAST. TWO ERAS. SHIFT BETWEEN THEM.",
        160,
        30,
        1,
        ALERT,
    );
    centred(
        &mut band,
        "HOWITZER OR TREBUCHET: BREAK THE LINE IN BOTH.",
        160,
        40,
        1,
        ECHO,
    );
    centred(
        &mut band,
        "GITHUB GAME OFF 2026",
        160,
        56,
        1,
        Rgba::rgb(180, 176, 170),
    );

    let x0 = (W - CW) / 2;
    let mut rgb = Vec::with_capacity(CW * CH * 3);
    for y in 0..CH {
        for x in 0..CW {
            let p = if y < H {
                scene.px[y * W + x0 + x]
            } else {
                band.px[(y - H) * W + x0 + x]
            };
            rgb.extend_from_slice(&[p.0, p.1, p.2]);
        }
    }
    shift_game::png::write(&out, CW, CH, &rgb, 2).unwrap();
    println!("{out}");
}

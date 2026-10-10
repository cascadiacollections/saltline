//! Source-art sky plate and native palette sky phases. This is presentation, keyed to the screen number;
//! it never consumes the simulation's random stream or changes a shot.

use crate::biome::Biome;
use crate::palette::{slot, Palette};
use crate::raster::{Canvas, View};

// Offline import writes both the PNG and these palette indices. Embedding
// the native pixels keeps engine-free tests and both frame paths identical.
const SALT_IRON_SKY: &[u8; 320 * 180] =
    include_bytes!("../../../art/backgrounds/salt_iron_sky_320x180.pal8");

// Full source-color plate for the detailed presentation. Physics still uses
// the same logical 320-wide world; this is not a larger generated screen.
const DETAIL_SKY: &[u8; 1280 * 720 * 4] = include_bytes!("../../../art/detail/sky.rgba");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Day,
    Dusk,
    Night,
    Dawn,
}

impl Phase {
    /// Three screens per phase gives a full, repeating twelve-screen day.
    pub fn for_screen(index: u32) -> Self {
        match (index / 3) % 4 {
            0 => Self::Day,
            1 => Self::Dusk,
            2 => Self::Night,
            _ => Self::Dawn,
        }
    }

    pub fn has_stars(self) -> bool {
        self != Self::Day
    }
}

fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A clean second band near the horizon. Terrain paints over the lower part,
/// so a hill naturally hides the colour, sun and stars.
pub fn draw(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    phase: Phase,
    biome: Biome,
    seed: u64,
    time: f64,
) {
    draw_still(canvas, view, pal, phase);
    draw_stars(canvas, view, pal, phase, seed, time, |_, _| true);
    draw_biome(canvas, view, pal, biome, seed, time, |_, _| true);
}

/// The part of [`draw`] that holds still for a whole screen: the imported
/// Salt & Iron harbour plate. It depends only on the phase and the buffer's
/// size, which is what lets `backdrop` keep it in a texture of its own.
pub fn draw_still(canvas: &mut Canvas, view: &View, pal: &Palette, phase: Phase) {
    canvas.fill(pal.get(slot::SKY_HI));
    let top = view.h - 180;
    if canvas.scale() == 4 {
        for y in 0..view.h * 4 {
            // Long Lens adds sky matching the source plate, rather than a
            // different palette-color band above the detailed background.
            let sy = (y - top * 4).max(0) as usize;
            for x in 0..view.w * 4 {
                let sx = if (x / 1280) % 2 == 0 {
                    x % 1280
                } else {
                    1279 - x % 1280
                };
                let at = (sy * 1280 + sx as usize) * 4;
                let mut c: [u8; 4] = DETAIL_SKY[at..at + 4].try_into().unwrap();
                let brightness = match phase {
                    Phase::Night => 45,
                    Phase::Dusk => 75,
                    Phase::Dawn => 90,
                    Phase::Day => 100,
                };
                for v in &mut c[..3] {
                    *v = (*v as u32 * brightness / 100) as u8;
                }
                canvas.put_detail(x, y, c);
            }
        }
        return;
    }
    for y in top.max(0)..view.h {
        let source_y = (y - top) as usize;
        for x in 0..view.w {
            // Mirror alternate native-width panels for wider buffers without
            // runtime scaling, fractional sampling or a new random stream.
            let panel = x / 320;
            let source_x = if panel % 2 == 0 {
                x % 320
            } else {
                319 - x % 320
            };
            let source = SALT_IRON_SKY[source_y * 320 + source_x as usize] as usize;
            let ink = match (phase, source) {
                (Phase::Night, slot::SKY_LO) => slot::SKY_MID,
                (Phase::Night, slot::SKY_MID) => slot::SKY_HI,
                (Phase::Night, slot::ALERT | slot::ST_HI) => slot::SKY_LO,
                (Phase::Dusk, slot::SKY_LO) => slot::SKY_MID,
                _ => source,
            };
            canvas.put(x, y, pal.get(ink));
        }
    }
}

/// The part of [`draw`] that moves: the twinkling stars and the shooting
/// star. Each is put only where `open(x, y)` says the sky shows, so the
/// layered frame can draw them over the ground texture and still let a hill
/// hide them, as painting the terrain over them does in [`draw`].
pub fn draw_stars(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    phase: Phase,
    seed: u64,
    time: f64,
    open: impl Fn(i32, i32) -> bool,
) {
    if !phase.has_stars() {
        return;
    }

    let count = if phase == Phase::Night { 14 } else { 5 };
    let star_top = 25;
    let star_span = (view.h - 120).max(30);
    let star_width = (view.w - 48).max(1);
    let step = (time * 5.0).floor() as u64;
    for i in 0..count {
        let h = hash(seed ^ (i as u64).wrapping_mul(0x6a09_e667_f3bc_c909));
        let x = 24 + (h % star_width as u64) as i32;
        let y = star_top + ((h >> 16) % star_span as u64) as i32;
        let bright = (step + (h >> 32)).is_multiple_of(13);
        if open(x, y) {
            canvas.put(
                x,
                y,
                pal.get(if bright { slot::UI_FG } else { slot::SKY_LO }),
            );
        }
    }

    if phase == Phase::Night {
        shooting_star(canvas, view, pal, seed, time, &open);
    }
}

/// Deterministic environmental details, drawn only where the sky is open so
/// terrain hides them in both flat and layered rendering.
pub fn draw_biome(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    biome: Biome,
    seed: u64,
    time: f64,
    open: impl Fn(i32, i32) -> bool,
) {
    match biome {
        Biome::Salt => draw_dust(canvas, view, pal, seed, time, &open),
        Biome::Snow => draw_snow(canvas, view, pal, seed, time, &open),
        // Source-art mode keeps the harbour scenery visible; biome coatings
        // and weather carry the variation instead of native tree/cactus shapes.
        Biome::Forest | Biome::Desert if canvas.scale() == 4 => {
            draw_dust(canvas, view, pal, seed, time, &open)
        }
        Biome::Forest => draw_forest(canvas, view, pal, seed, &open),
        Biome::Desert => draw_desert(canvas, view, pal, seed, time, &open),
        Biome::Night => draw_night(canvas, view, pal, seed, time, &open),
        Biome::Volcanic => draw_embers(canvas, view, pal, seed, time, &open),
    }
}

fn draw_dust(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let step = (time * 3.0).floor() as u64;
    for i in 0..10 {
        let h = hash(seed ^ 0xd0_u64.wrapping_add(i as u64 * 0x517c_c1b7));
        let x = ((h.wrapping_add(step * 3) % view.w.max(1) as u64) as i32).rem_euclid(view.w);
        let y = view.h - 105 - ((h >> 16) % 45) as i32;
        if open(x, y) {
            canvas.put(
                x,
                y,
                pal.get(if i % 3 == 0 {
                    slot::ST_HI
                } else {
                    slot::SKY_LO
                }),
            );
        }
    }
}

fn draw_snow(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let step = (time * 2.0).floor() as u64;
    for i in 0..18 {
        let h = hash(seed ^ 0x534e_4f57 ^ (i as u64).wrapping_mul(0x9e37_79b9));
        let x = ((h % view.w.max(1) as u64) as i32 + step as i32 * (i % 3 - 1)).rem_euclid(view.w);
        let span = (view.h - 115).max(20);
        let y = 20 + ((h >> 20) as i32 + step as i32 * (1 + i % 2)) % span;
        if open(x, y) {
            canvas.put(
                x,
                y,
                pal.get(if i % 4 == 0 {
                    slot::UI_FG
                } else {
                    slot::SKY_LO
                }),
            );
        }
    }
}

fn draw_forest(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let horizon = view.h - 91;
    for i in 0..9 {
        let h = hash(seed ^ 0x5049_4e45 ^ (i as u64).wrapping_mul(0x6a09_e667));
        let x = 12 + (h % view.w.saturating_sub(24).max(1) as u64) as i32;
        let height = 12 + ((h >> 12) % 17) as i32;
        let top = horizon - height;
        let colour = pal.get(if i % 2 == 0 {
            slot::BEDROCK
        } else {
            slot::ROCK
        });
        for row in 0..height {
            let width = 1 + row * 2 / 3;
            for dx in -width..=width {
                let px = x + dx;
                let py = top + row;
                if open(px, py) {
                    canvas.put(px, py, colour);
                }
            }
        }
        for py in top + height..=horizon {
            if open(x, py) {
                canvas.put(x, py, colour);
            }
        }
    }
}

/// Saguaro silhouettes on the horizon and a few heat-shimmer motes.
fn draw_desert(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let horizon = view.h - 91;
    let colour = pal.get(slot::BEDROCK);
    for i in 0..5 {
        let h = hash(seed ^ 0x4341_4354 ^ (i as u64).wrapping_mul(0x6a09_e667));
        let x = 14 + (h % view.w.saturating_sub(28).max(1) as u64) as i32;
        let height = 10 + ((h >> 12) % 11) as i32;
        let arm = height / 2;
        for py in horizon - height..=horizon {
            for px in [x, x + 1] {
                if open(px, py) {
                    canvas.put(px, py, colour);
                }
            }
        }
        let ay = horizon - arm - (i % 2) * 2;
        for (from, to) in [(x - 3, x), (x + 1, x + 4)] {
            for px in from..=to {
                if open(px, ay) {
                    canvas.put(px, ay, colour);
                }
            }
            let tip = if to == x { from } else { to };
            for py in ay - 3..ay {
                if open(tip, py) {
                    canvas.put(tip, py, colour);
                }
            }
        }
    }
    let step = (time * 2.0).floor() as u64;
    for i in 0..6 {
        let h = hash(seed ^ 0x4845_4154 ^ (i as u64).wrapping_mul(0x9e37_79b9));
        let x = ((h.wrapping_add(step) % view.w.max(1) as u64) as i32).rem_euclid(view.w);
        let y = horizon - 4 - ((h >> 16) % 14) as i32;
        if open(x, y) {
            canvas.put(x, y, pal.get(slot::ST_HI));
        }
    }
}

/// A moon with a dim halo; the stars come from the sky phase.
fn draw_night(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let h = hash(seed ^ 0x4d4f_4f4e);
    let cx = 40 + (h % view.w.saturating_sub(80).max(1) as u64) as i32;
    let cy = 34 + ((h >> 20) % 30) as i32;
    for dy in -8..=8i32 {
        for dx in -8..=8i32 {
            let d = dx * dx + dy * dy;
            let colour = if d <= 25 {
                // A crescent: the bite is a second disc offset to the right.
                let bite = (dx - 3) * (dx - 3) + (dy + 1) * (dy + 1);
                if bite <= 20 {
                    continue;
                }
                slot::UI_FG
            } else if d <= 64 && (dx + dy) % 2 == 0 {
                slot::SKY_LO
            } else {
                continue;
            };
            if open(cx + dx, cy + dy) {
                canvas.put(cx + dx, cy + dy, pal.get(colour));
            }
        }
    }
    let step = (time * 4.0).floor() as u64;
    for i in 0..22 {
        let h = hash(seed ^ 0x5354_4152 ^ (i as u64).wrapping_mul(0x9e37_79b9));
        let x = (h % view.w.max(1) as u64) as i32;
        let y = 18 + ((h >> 20) % (view.h - 120).max(20) as u64) as i32;
        let bright = (step + (h >> 40)).is_multiple_of(9);
        if open(x, y) {
            canvas.put(
                x,
                y,
                pal.get(if bright { slot::UI_FG } else { slot::SKY_LO }),
            );
        }
    }
}

/// Embers drifting up from the ground.
fn draw_embers(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    let step = (time * 6.0).floor() as i32;
    let span = (view.h - 110).max(30);
    for i in 0..16 {
        let h = hash(seed ^ 0x454d_4252 ^ (i as u64).wrapping_mul(0x9e37_79b9));
        let x = ((h % view.w.max(1) as u64) as i32 + (step / 3) * (i % 3 - 1)).rem_euclid(view.w);
        let rise = (h >> 20) as i32 % span;
        let y = (view.h - 95 - (rise + step * (1 + i % 3)).rem_euclid(span)).max(0);
        if open(x, y) {
            canvas.put(
                x,
                y,
                pal.get(if i % 4 == 0 {
                    slot::ST_HI
                } else {
                    slot::SKY_LO
                }),
            );
        }
    }
}

fn shooting_star(
    canvas: &mut Canvas,
    view: &View,
    pal: &Palette,
    seed: u64,
    time: f64,
    open: &impl Fn(i32, i32) -> bool,
) {
    // One short crossing per ~23 seconds. Its start and route depend on the
    // run seed, but its clock is cosmetic and never enters a replay digest.
    let t = (time + (hash(seed) % 23) as f64).max(0.0);
    let cycle = (t / 23.0).floor() as u64;
    let local = t % 23.0;
    if local >= 0.7 {
        return;
    }
    let h = hash(seed ^ cycle ^ 0x7374_6172);
    let x = 28 + (h % (view.w - 90).max(1) as u64) as i32 + (local * 29.0) as i32;
    let y = 28 + ((h >> 18) % (view.h - 140).max(22) as u64) as i32 + (local * 11.0) as i32;
    for tail in 0..6 {
        let colour = if tail == 0 {
            slot::UI_FG
        } else if tail < 3 {
            slot::SKY_LO
        } else {
            slot::SKY_MID
        };
        let (px, py) = (x - tail * 2, y - tail);
        if open(px, py) {
            canvas.put(px, py, pal.get(colour));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_sky_copies_native_pixels_and_extends_without_scaling() {
        let pal = Palette::default();
        for (w, h) in [(320, 180), (640, 216)] {
            let view = View { cam_x: 137, w, h };
            let mut canvas = Canvas::new(w, h);
            draw_still(&mut canvas, &view, &pal, Phase::Day);
            for y in 0..h {
                for x in 0..w {
                    let want = if y < h - 180 {
                        pal.get(slot::SKY_HI)
                    } else {
                        let sx = if x < 320 { x } else { 639 - x };
                        let index = ((y - (h - 180)) * 320 + sx) as usize;
                        pal.get(SALT_IRON_SKY[index] as usize)
                    };
                    let index = ((y * w + x) * 4) as usize;
                    assert_eq!(&canvas.bytes()[index..index + 4], &want, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn detailed_long_lens_sky_extends_the_source_plate_without_a_color_seam() {
        let mut canvas = Canvas::detailed(400, 216);
        let view = View {
            cam_x: 137,
            w: 400,
            h: 216,
        };
        draw_still(&mut canvas, &view, &Palette::default(), Phase::Day);
        for y in [0, 143, 144, 300, 863] {
            for x in [0, 1279, 1280, 1599] {
                let sx = if x < 1280 { x } else { 2559 - x };
                let sy = (y - 144).max(0);
                let at = ((y * 1600 + x) * 4) as usize;
                let source = ((sy * 1280 + sx) * 4) as usize;
                assert_eq!(&canvas.bytes()[at..at + 4], &DETAIL_SKY[source..source + 4]);
            }
        }
    }

    #[test]
    fn phases_repeat_without_touching_simulation_state() {
        assert_eq!(Phase::for_screen(0), Phase::Day);
        assert_eq!(Phase::for_screen(3), Phase::Dusk);
        assert_eq!(Phase::for_screen(6), Phase::Night);
        assert_eq!(Phase::for_screen(9), Phase::Dawn);
        assert_eq!(Phase::for_screen(12), Phase::Day);
    }

    #[test]
    fn stars_change_only_with_the_cosmetic_clock() {
        let view = View {
            cam_x: 0,
            w: 320,
            h: 180,
        };
        let pal = Palette::default();
        let mut a = Canvas::new(320, 180);
        let mut b = Canvas::new(320, 180);
        draw(&mut a, &view, &pal, Phase::Night, Biome::Salt, 42, 1.0);
        draw(&mut b, &view, &pal, Phase::Night, Biome::Salt, 42, 1.0);
        assert_eq!(a.bytes(), b.bytes());
        draw(&mut b, &view, &pal, Phase::Night, Biome::Salt, 42, 2.0);
        assert_ne!(a.bytes(), b.bytes());
    }
}

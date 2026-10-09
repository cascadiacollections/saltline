//! Palette-only sky phases. This is presentation, keyed to the screen number;
//! it never consumes the simulation's random stream or changes a shot.

use crate::biome::Biome;
use crate::palette::{slot, Palette};
use crate::raster::{Canvas, View};

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

/// The part of [`draw`] that holds still for a whole screen: the fill, the
/// horizon band and the sun. It depends only on the phase and the buffer's
/// size, which is what lets `backdrop` keep it in a texture of its own.
pub fn draw_still(canvas: &mut Canvas, view: &View, pal: &Palette, phase: Phase) {
    let (base, horizon) = match phase {
        Phase::Day => (slot::SKY_MID, slot::SKY_LO),
        Phase::Dusk => (slot::SKY_HI, slot::SKY_MID),
        Phase::Night => (slot::SKY_HI, slot::SKY_HI),
        Phase::Dawn => (slot::SKY_HI, slot::SKY_LO),
    };
    canvas.fill(pal.get(base));
    let horizon_y = view.h - 88;
    if base != horizon {
        canvas.rect(
            0,
            horizon_y - 3,
            canvas.w,
            canvas.h - horizon_y + 3,
            pal.get(horizon),
        );
    }
    if matches!(phase, Phase::Dusk | Phase::Dawn) {
        let x = if phase == Phase::Dusk {
            view.w * 3 / 4
        } else {
            view.w / 5
        };
        let colour = if phase == Phase::Dusk {
            slot::ST_DMG
        } else {
            slot::ST_HI
        };
        canvas.rect(x, horizon_y + 1, 3, 3, pal.get(colour));
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
        Biome::Forest => draw_forest(canvas, view, pal, seed, &open),
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

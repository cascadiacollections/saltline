//! Palette-only sky phases. This is presentation, keyed to the screen number;
//! it never consumes the simulation's random stream or changes a shot.

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
pub fn draw(canvas: &mut Canvas, view: &View, pal: &Palette, phase: Phase, seed: u64, time: f64) {
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
        canvas.put(
            x,
            y,
            pal.get(if bright { slot::UI_FG } else { slot::SKY_LO }),
        );
    }

    if phase == Phase::Night {
        shooting_star(canvas, view, pal, seed, time);
    }
}

fn shooting_star(canvas: &mut Canvas, view: &View, pal: &Palette, seed: u64, time: f64) {
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
        canvas.put(x - tail * 2, y - tail, pal.get(colour));
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
        draw(&mut a, &view, &pal, Phase::Night, 42, 1.0);
        draw(&mut b, &view, &pal, Phase::Night, 42, 1.0);
        assert_eq!(a.bytes(), b.bytes());
        draw(&mut b, &view, &pal, Phase::Night, 42, 2.0);
        assert_ne!(a.bytes(), b.bytes());
    }
}

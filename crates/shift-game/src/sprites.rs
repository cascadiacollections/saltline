//! Sprites, embedded as the raw RGBA blob gameoff/tools/export_atlas.py writes.

use crate::canvas::{Canvas, Rgba};

#[derive(Clone)]
pub struct Sprite {
    pub w: i32,
    pub h: i32,
    pub px: Vec<Rgba>,
}

/// How to put a sprite down.
#[derive(Clone, Copy)]
pub enum Ink {
    /// As drawn.
    Solid,
    /// Mixed toward a colour by a share, on every other pixel: a ghost.
    Ghost(Rgba, f32),
    /// Every opaque pixel mixed toward a colour by a share.
    Tint(Rgba, f32),
}

impl Sprite {
    /// Draws the sub-rectangle starting at column `sx`, `w` wide, with its
    /// top-left at (x, y); `flip` mirrors it horizontally.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_region(
        &self,
        c: &mut Canvas,
        sx: i32,
        w: i32,
        x: i32,
        y: i32,
        flip: bool,
        ink: Ink,
    ) {
        for j in 0..self.h {
            for i in 0..w {
                let src = self.px[(j * self.w + sx + i) as usize];
                if src.3 < 128 {
                    continue;
                }
                let dx = if flip { x + w - 1 - i } else { x + i };
                let dy = y + j;
                match ink {
                    Ink::Solid => c.set(dx, dy, src),
                    Ink::Tint(t, a) => c.set(dx, dy, src.mix(t, a)),
                    Ink::Ghost(t, a) => {
                        if (dx + dy) & 1 == 0 {
                            c.set(dx, dy, c.get(dx, dy).mix(src.mix(t, 0.6), a));
                        }
                    }
                }
            }
        }
    }

    pub fn draw(&self, c: &mut Canvas, x: i32, y: i32, flip: bool, ink: Ink) {
        self.draw_region(c, 0, self.w, x, y, flip, ink);
    }
}

pub struct Atlas {
    sprites: Vec<(String, Sprite)>,
}

static BLOB: &[u8] = include_bytes!("../assets/sprites.bin");

impl Atlas {
    pub fn embedded() -> Self {
        Self::parse(BLOB)
    }

    pub fn parse(mut b: &[u8]) -> Self {
        let mut sprites = Vec::new();
        while !b.is_empty() {
            let n = b[0] as usize;
            let name = String::from_utf8_lossy(&b[1..1 + n]).into_owned();
            b = &b[1 + n..];
            let w = u16::from_le_bytes([b[0], b[1]]) as i32;
            let h = u16::from_le_bytes([b[2], b[3]]) as i32;
            b = &b[4..];
            let len = (w * h * 4) as usize;
            let px = b[..len]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| Rgba(p[0], p[1], p[2], p[3]))
                .collect();
            b = &b[len..];
            sprites.push((name, Sprite { w, h, px }));
        }
        Atlas { sprites }
    }

    /// A sprite by name. Panics on a name the exporter does not write, which
    /// the tests check for every name the game asks for.
    pub fn get(&self, name: &str) -> &Sprite {
        self.sprites
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
            .unwrap_or_else(|| panic!("no sprite {name}"))
    }
}

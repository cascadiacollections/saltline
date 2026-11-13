//! A fixed-size RGBA pixel buffer and the few primitives the game draws with.

/// Logical resolution. Godot scales it to the window with nearest filtering.
pub const W: usize = 320;
pub const H: usize = 180;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba(r, g, b, 255)
    }
    /// Linear mix toward `o` by `t` (0..=1).
    pub fn mix(self, o: Rgba, t: f32) -> Rgba {
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgba(l(self.0, o.0), l(self.1, o.1), l(self.2, o.2), 255)
    }
}

/// 4x4 ordered-dither thresholds, 0..16.
pub const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Whether pixel (x, y) is on at coverage `t` (0..=1) under ordered dither.
pub fn dither(x: i32, y: i32, t: f32) -> bool {
    let b = BAYER[(y & 3) as usize][(x & 3) as usize] as f32;
    t * 16.0 > b + 0.5
}

#[derive(Clone)]
pub struct Canvas {
    pub px: Vec<Rgba>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    pub fn new() -> Self {
        Canvas {
            px: vec![Rgba::rgb(0, 0, 0); W * H],
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: Rgba) {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.px[y as usize * W + x as usize] = c;
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.px[y as usize * W + x as usize]
        } else {
            Rgba::rgb(0, 0, 0)
        }
    }

    /// Mix `c` over the pixel by `a` (0..=1).
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgba, a: f32) {
        let under = self.get(x, y);
        self.set(x, y, under.mix(c, a));
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for j in y..y + h {
            for i in x..x + w {
                self.set(i, j, c);
            }
        }
    }

    pub fn rect_blend(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba, a: f32) {
        for j in y..y + h {
            for i in x..x + w {
                self.blend(i, j, c, a);
            }
        }
    }

    /// Bresenham line; `dash` > 0 draws every `dash`-th pixel only.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba, dash: u32) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut n = 0u32;
        loop {
            if dash == 0 || n.is_multiple_of(dash) {
                self.set(x, y, c);
            }
            n += 1;
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, c: Rgba, a: f32) {
        for j in -r..=r {
            for i in -r..=r {
                if i * i + j * j <= r * r {
                    self.blend(cx + i, cy + j, c, a);
                }
            }
        }
    }

    /// Per-pixel dithered crossfade: where the dither says so, take `other`.
    pub fn dissolve(&mut self, other: &Canvas, t: f32) {
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                if dither(x, y, t) {
                    let i = y as usize * W + x as usize;
                    self.px[i] = other.px[i];
                }
            }
        }
    }

    /// The frame as tightly packed RGBA bytes, for the engine.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(W * H * 4);
        for p in &self.px {
            out.extend_from_slice(&[p.0, p.1, p.2, 255]);
        }
        out
    }
}

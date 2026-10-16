//! Software painter with logical world coordinates and optional 4x source art.
//!
//! The native canvas remains the palette snapshot/fallback path. Detailed
//! canvases retain the same logical bounds but store four display pixels per
//! unit. Logical writes fill blocks; source sprites copy their actual pixels.
//! Terrain masks and material selection always come from the heightfield.
//! Barrel rotations and source resampling happen offline. The engine composites
//! detailed frames flat, preserving painter order and per-pixel arc blending.

use godot::builtin::PackedByteArray;
use salt_line_sim::rng::mix64;
use salt_line_sim::run::Screen;
use salt_line_sim::terrain::{Heightfield, Material, ROCK_DEPTH, SAND_DEPTH};
use salt_line_sim::SCREEN_H;

use crate::font;
use crate::palette::{slot, Palette, Rgba};

/// Source-art soil samples, ordered sand, rock, bedrock.
const DETAIL_SOIL: &[u8; 3 * 64 * 64 * 4] = include_bytes!("../../../art/detail/soil.rgba");

/// 4x4 Bayer matrix: the values 0 to 15 laid out so that any threshold
/// switches on pixels spread as evenly as possible. The only dithering in
/// the game is the sky gradient and the material transition band — DESIGN
/// §5.2 is explicit that dithering everything is the other common low-res
/// failure.
pub const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The dither threshold for buffer or world pixel `(x, y)`, strictly between
/// 0 and 1 (1/32 to 31/32). A blend fraction above it takes the second
/// colour.
#[inline]
fn bayer(x: i32, y: i32) -> f64 {
    let bx = x.rem_euclid(4) as usize;
    let by = y.rem_euclid(4) as usize;
    (BAYER[by][bx] as f64 + 0.5) / 16.0
}

/// The colour a shot's own marks are drawn in (the shell head, the
/// landing mark, the muzzle and impact flashes), and an optional backing
/// colour drawn one pixel around them first. A run draws them in the alert
/// colour with no backing; local versus draws them in the shooter's colour
/// on a dark backing, since the right player's colour is the sky's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ink {
    pub colour: Rgba,
    pub backing: Option<Rgba>,
}

impl Ink {
    /// `colour` with no backing.
    pub fn plain(colour: Rgba) -> Ink {
        Ink {
            colour,
            backing: None,
        }
    }
}

/// An RGBA8 backing image with logical layout bounds.
pub struct Canvas {
    /// Width in buffer pixels.
    pub w: i32,
    /// Height in buffer pixels.
    pub h: i32,
    /// Physical pixels, row by row, four bytes each; `pixel_w() * pixel_h()`.
    px: Vec<u8>,
    scale: i32,
}

impl Canvas {
    /// A transparent buffer of `w` by `h` buffer pixels, at least 1 by 1.
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        Canvas {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
            scale: 1,
        }
    }

    /// Artwork pixels per logical world unit. Coordinates and hit geometry
    /// remain logical; only the backing image changes resolution.
    pub fn detailed(w: i32, h: i32) -> Self {
        let mut canvas = Self::new(w, h);
        canvas.scale = 4;
        canvas.px = vec![0; (canvas.pixel_w() * canvas.pixel_h() * 4) as usize];
        canvas
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }
    pub fn pixel_w(&self) -> i32 {
        self.w * self.scale
    }
    pub fn pixel_h(&self) -> i32 {
        self.h * self.scale
    }

    /// Write a display pixel without changing the logical drawing grid.
    pub fn put_detail(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.pixel_w() || y >= self.pixel_h() {
            return;
        }
        let i = ((y * self.pixel_w() + x) * 4) as usize;
        self.px[i..i + 4].copy_from_slice(&c);
    }

    /// Change size, clearing the buffer only if the size actually changed.
    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.px = vec![0; (self.pixel_w() * self.pixel_h() * 4) as usize];
        }
    }

    /// Every pixel to transparent black, which `Canvas::palette_text`
    /// shows as `.`.
    pub fn clear(&mut self) {
        self.px.fill(0);
    }

    pub fn fill(&mut self, c: Rgba) {
        for p in self.px.chunks_exact_mut(c.len()) {
            p.copy_from_slice(&c);
        }
    }

    /// Set one pixel. Anything off the buffer is dropped, so every drawing
    /// routine can run partly off the edge without clipping for itself.
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        for dy in 0..self.scale {
            for dx in 0..self.scale {
                self.put_detail(x * self.scale + dx, y * self.scale + dy, c);
            }
        }
    }

    /// Alpha-blended write, for the one translucent mark (the last arc).
    /// `alpha` is 0 (nothing) to 255 (opaque); the result is always opaque,
    /// and its colour is off the palette unless `alpha` is 255.
    ///
    /// A pixel this canvas has not painted (transparent) is blended with
    /// `under(px, py)`, in display pixels (`put_detail`'s coordinates, the
    /// logical ones at scale 1): the colour a layer beneath shows there
    /// (`backdrop::Backdrop::under`). The layered frame leaves the sky and
    /// the ground out of this canvas, so a translucent mark over them needs
    /// their colour to come out as in the flat frame, and to stay opaque:
    /// the overlay's pixels must be fully transparent or fully opaque. This
    /// is the only blending write, so there is no way to blend without
    /// saying what is beneath.
    #[inline]
    pub fn put_blend_on(
        &mut self,
        x: i32,
        y: i32,
        c: Rgba,
        alpha: u8,
        under: impl Fn(i32, i32) -> Rgba,
    ) {
        if alpha == 255 {
            return self.put(x, y, c);
        }
        if x < 0 || y < 0 || x >= self.w || y >= self.h || alpha == 0 {
            return;
        }
        let a = alpha as u32;
        for dy in 0..self.scale {
            for dx in 0..self.scale {
                let (px, py) = (x * self.scale + dx, y * self.scale + dy);
                let i = ((py * self.pixel_w() + px) * 4) as usize;
                if self.px[i + 3] == 0 {
                    let u = under(px, py);
                    self.px[i..i + 3].copy_from_slice(&u[..3]);
                }
                for (dst, src) in self.px[i..i + 3].iter_mut().zip(c.iter()) {
                    *dst = ((*src as u32 * a + *dst as u32 * (255 - a)) / 255) as u8;
                }
                self.px[i + 3] = 255;
            }
        }
    }

    /// The pixel at `(x, y)`, transparent if off the buffer or unpainted.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return [0; 4];
        }
        let i =
            (((y * self.scale + self.scale / 2) * self.pixel_w() + x * self.scale + self.scale / 2)
                * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// The display pixel at `(px, py)` (`put_detail`'s coordinates),
    /// transparent if off the buffer or unpainted.
    #[inline]
    pub fn get_detail(&self, px: i32, py: i32) -> Rgba {
        if px < 0 || py < 0 || px >= self.pixel_w() || py >= self.pixel_h() {
            return [0; 4];
        }
        let i = ((py * self.pixel_w() + px) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for j in y..y + h {
            for i in x..x + w {
                self.put(i, j, c);
            }
        }
    }

    pub fn rect_outline(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for i in x..x + w {
            self.put(i, y, c);
            self.put(i, y + h - 1, c);
        }
        for j in y..y + h {
            self.put(x, j, c);
            self.put(x + w - 1, j, c);
        }
    }

    pub fn hline(&mut self, x0: i32, x1: i32, y: i32, c: Rgba) {
        for x in x0.min(x1)..=x0.max(x1) {
            self.put(x, y, c);
        }
    }

    pub fn vline(&mut self, x: i32, y0: i32, y1: i32, c: Rgba) {
        for y in y0.min(y1)..=y0.max(y1) {
            self.put(x, y, c);
        }
    }

    /// Dotted vertical line — the Plumb Line guide (DESIGN §4).
    pub fn vline_dotted(&mut self, x: i32, y0: i32, y1: i32, c: Rgba, period: i32) {
        for y in y0.min(y1)..=y0.max(y1) {
            if y.rem_euclid(period) == 0 {
                self.put(x, y, c);
            }
        }
    }

    /// An icon from `icons`, with its digits picking from `colors`. `flip`
    /// mirrors it left to right.
    pub fn icon(&mut self, x: i32, y: i32, rows: &[&str], colors: &[Rgba], flip: bool) {
        for (j, row) in rows.iter().enumerate() {
            let w = row.len() as i32;
            for (i, b) in row.bytes().enumerate() {
                let Some(&c) = b.checked_sub(b'1').and_then(|k| colors.get(k as usize)) else {
                    continue;
                };
                let i = i as i32;
                let dx = if flip { w - 1 - i } else { i };
                self.put(x + dx, y + j as i32, c);
            }
        }
    }

    /// Set every pixel in `points` to `ink`'s colour, after putting its
    /// backing (if any) on their four neighbours, so each mark is ringed in
    /// the backing wherever it is not touching itself.
    pub fn put_inked(&mut self, points: &[(i32, i32)], ink: Ink) {
        if let Some(backing) = ink.backing {
            for &(x, y) in points {
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    self.put(x + dx, y + dy, backing);
                }
            }
        }
        for &(x, y) in points {
            self.put(x, y, ink.colour);
        }
    }

    /// 3x5 text. Returns the x cursor after the string.
    pub fn text_small(&mut self, x: i32, y: i32, s: &str, c: Rgba) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(g) = font::small_glyph(ch) {
                for (j, row) in g.iter().enumerate() {
                    for (i, bit) in row.bytes().enumerate() {
                        if bit == b'1' {
                            self.put(cx + i as i32, y + j as i32, c);
                        }
                    }
                }
            }
            cx += font::SMALL_ADVANCE as i32;
        }
        cx
    }

    /// 3x5 text with every glyph pixel drawn as a `scale` by `scale` block,
    /// for the few words a young player has to read across the room (the
    /// versus HIT and MISS, whose turn it is, who won). Whole blocks on the
    /// buffer's own grid, so it is still pixel type, only chunkier; the
    /// instruments stay at native size. Returns the x cursor after the
    /// string.
    pub fn text_small_scaled(&mut self, x: i32, y: i32, s: &str, c: Rgba, scale: i32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(g) = font::small_glyph(ch) {
                for (j, row) in g.iter().enumerate() {
                    for (i, bit) in row.bytes().enumerate() {
                        if bit == b'1' {
                            let (px, py) = (cx + i as i32 * scale, y + j as i32 * scale);
                            self.rect(px, py, scale, scale, c);
                        }
                    }
                }
            }
            cx += font::SMALL_ADVANCE as i32 * scale;
        }
        cx
    }

    /// 5x7 text — the shell counter and the wind value (DESIGN §5.6).
    pub fn text_big(&mut self, x: i32, y: i32, s: &str, c: Rgba) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(g) = font::big_glyph(ch) {
                for (j, row) in g.iter().enumerate() {
                    for (i, bit) in row.bytes().enumerate() {
                        if bit == b'1' {
                            self.put(cx + i as i32, y + j as i32, c);
                        }
                    }
                }
            }
            cx += font::BIG_ADVANCE as i32;
        }
        cx
    }

    /// A UI plate: background, one-pixel border, no rounding, no shadow.
    pub fn plate(&mut self, x: i32, y: i32, w: i32, h: i32, pal: &Palette) {
        self.rect(x, y, w, h, pal.get(slot::UI_BG));
        self.rect_outline(x, y, w, h, pal.get(slot::SHADOW));
    }

    /// Raw byte access, for tests.
    #[cfg(test)]
    pub fn px_at(&self, i: usize) -> u8 {
        self.px[i]
    }

    /// The frame as text for snapshot tests: one character per pixel, the
    /// palette slot as a hex digit, `.` for untouched and `?` for a colour off
    /// the palette (a blend). A diff then shows exactly which pixels moved.
    #[cfg(test)]
    pub fn palette_text(&self, pal: &crate::palette::Palette) -> String {
        let mut out = String::with_capacity(((self.w + 1) * self.h) as usize);
        for row in self.px.as_chunks::<4>().0.chunks(self.w as usize) {
            for c in row {
                let ch = if c[3] == 0 {
                    '.'
                } else {
                    (0..16)
                        .find(|&i| pal.get(i) == *c)
                        .and_then(|i| char::from_digit(i as u32, 16))
                        .unwrap_or('?')
                };
                out.push(ch);
            }
            out.push('\n');
        }
        out
    }

    /// The frame as RGBA8 bytes, row by row.
    pub fn bytes(&self) -> &[u8] {
        &self.px
    }

    /// A transparent canvas the same size and scale as this one.
    pub fn blank_like(&self) -> Canvas {
        if self.scale == 1 {
            Canvas::new(self.w, self.h)
        } else {
            let mut canvas = Canvas::new(self.w, self.h);
            canvas.scale = self.scale;
            canvas.px = vec![0; (canvas.pixel_w() * canvas.pixel_h() * 4) as usize];
            canvas
        }
    }

    /// Every pixel from `src`, which must be the same size and scale: one
    /// copy of the whole buffer.
    pub fn copy_from(&mut self, src: &Canvas) {
        assert!(
            (self.w, self.h, self.scale) == (src.w, src.h, src.scale),
            "copy_from between canvases of different shapes"
        );
        self.px.copy_from_slice(&src.px);
    }

    pub fn to_packed(&self) -> PackedByteArray {
        PackedByteArray::from(self.px.as_slice())
    }
}

/// Where the camera is and how tall the buffer is this frame.
///
/// DESIGN §5.1: camera position is quantized to whole pixels every frame. A
/// camera at x=41.3 makes the entire terrain shimmer as it pans, and that is
/// the single most common way low-res games look cheap. `cam_x` is an `i32`
/// for exactly that reason — there is no float to forget to round.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// World x at the buffer's left edge, in world units (whole pixels).
    pub cam_x: i32,
    /// Buffer width, in buffer pixels.
    pub w: i32,
    /// Buffer height, in buffer pixels: 180, or more under Long Lens.
    pub h: i32,
}

impl View {
    /// World x (world units) to screen x (buffer pixels), rounded to the
    /// nearest whole pixel.
    #[inline]
    pub fn sx(&self, world_x: f64) -> i32 {
        (world_x.round() as i32) - self.cam_x
    }

    /// World y (up, 0 at the world floor) to screen y (down).
    ///
    /// Extra buffer height shows up as extra sky above the world, which is
    /// what Long Lens is for: shells arc above the viewport.
    #[inline]
    pub fn sy(&self, world_y: f64) -> i32 {
        self.h - 1 - (world_y.round() as i32)
    }

    /// The world row a screen row corresponds to.
    #[inline]
    pub fn world_row(&self, screen_y: i32) -> i32 {
        self.h - 1 - screen_y
    }
}

/// Sky gradient, dithered between bands. Ported from `gen_sprites.py::scene`
/// so the running game matches `art/scene_native_320x180.png`.
pub fn draw_sky(canvas: &mut Canvas, view: &View, pal: &Palette) {
    for y in 0..canvas.h {
        // Anchor the gradient to the world, not to the buffer, so Long Lens
        // adds sky at the top instead of shifting the whole ramp.
        let world_row = SCREEN_H as i32 - 1 - view.world_row(y);
        // `world_row` counts pixels down from the top of the 180-pixel world
        // (negative in Long Lens's extra sky), and `t` measures it in
        // 105-pixel steps: flat SKY_HI to 47 pixels down, SKY_HI to SKY_MID
        // over the next 58, SKY_MID to SKY_LO over the 44 after that.
        let t = world_row as f64 / 105.0;
        for x in 0..canvas.w {
            // Dither by world x, not buffer x, so the pattern pans with the
            // camera instead of crawling across the sky.
            let th = bayer(x + view.cam_x, y);
            let (lo, hi, f) = if t < 1.0 {
                (
                    slot::SKY_HI,
                    slot::SKY_MID,
                    ((t - 0.45) / 0.55).clamp(0.0, 1.0),
                )
            } else {
                (
                    slot::SKY_MID,
                    slot::SKY_LO,
                    ((t - 1.0) * 2.4).clamp(0.0, 1.0),
                )
            };
            // The dither threshold decides the whole ramp: at f <= 0 nothing
            // beats it and at f >= 1 everything does, so the flat ends fall
            // out of the same comparison.
            let c = if f > th { pal.get(hi) } else { pal.get(lo) };
            canvas.put(x, y, c);
        }
    }
}

/// Terrain: one solid vertical span per pixel column.
///
/// Material bands are measured from the *original* surface, not the current
/// one. On undamaged ground the two are identical — this is the banding in
/// `art/scene_native_320x180.png` — but in a crater it is the difference
/// between showing the rock and bedrock the shell actually exposed and
/// painting fresh sand at the bottom of a 40px pit. The rendered layers agree
/// with `Heightfield::material_at`, which is what the sim charges craters
/// against.
pub fn draw_terrain(canvas: &mut Canvas, view: &View, field: &Heightfield, pal: &Palette) {
    for sx in 0..canvas.w {
        let world_x = sx + view.cam_x;
        if world_x < 0 || world_x as usize >= field.len() {
            continue;
        }
        let col = world_x as usize;
        let surface = field.surface(col);
        let top = view.sy(surface);
        // How far below the original surface this column now starts.
        let offset = field.scar[col].max(0.0);

        for y in top.max(0)..canvas.h {
            let d = (y - top) as f64 + offset;
            let c = if d < SAND_DEPTH - 1.0 {
                pal.get(slot::SAND)
            } else if d < ROCK_DEPTH {
                pal.get(slot::ROCK)
            } else if d < ROCK_DEPTH + 8.0 {
                // Dithered transition into bedrock.
                if bayer(world_x, y) > (d - ROCK_DEPTH) / 8.0 {
                    pal.get(slot::ROCK)
                } else {
                    pal.get(slot::BEDROCK)
                }
            } else {
                pal.get(slot::BEDROCK)
            };
            if canvas.scale() == 4 {
                // Texture samples come from the board, while the material
                // selection and silhouette still come from the heightfield.
                let material = if c == pal.get(slot::SAND) {
                    0
                } else if c == pal.get(slot::ROCK) {
                    1
                } else {
                    2
                };
                for dy in 0..4 {
                    for dx in 0..4 {
                        let tx = (world_x * 4 + dx).rem_euclid(64) as usize;
                        let ty = (view.world_row(y) * 4 - dy).rem_euclid(64) as usize;
                        let at = (material * 64 * 64 + ty * 64 + tx) * 4;
                        canvas.put_detail(
                            sx * 4 + dx,
                            y * 4 + dy,
                            DETAIL_SOIL[at..at + 4].try_into().unwrap(),
                        );
                    }
                }
            } else {
                canvas.put(sx, y, c);
            }
        }

        // Crater rims get one pixel of the shadow tone on the inner edge,
        // wherever a neighbouring column stands 0.4 world units higher, and a
        // second pixel on a steeper wall (1.2 units).
        // That single line is what makes a crater read as depth rather than as
        // a bite taken out of the ground (DESIGN §5.5).
        let left = field.surface(col.saturating_sub(1));
        let right = field.surface((col + 1).min(field.len() - 1));
        if (left > surface + 0.4 || right > surface + 0.4) && top >= 0 {
            canvas.put(sx, top, pal.get(slot::SHADOW));
            if left > surface + 1.2 || right > surface + 1.2 {
                canvas.put(sx, top + 1, pal.get(slot::SHADOW));
            }
        }
    }
}

/// Sparse soil flecks and grass tufts, fixed to world columns. The grass is
/// removed by a crater because only the untouched surface can grow it.
pub fn draw_ground_detail(canvas: &mut Canvas, view: &View, screen: &Screen, pal: &Palette) {
    draw_ground_detail_around(canvas, view, screen, &[screen.muzzle_x], pal);
}

/// [`draw_ground_detail`] with no grass within 22 columns of any of `guns`
/// (world x of each emplacement): the screen's own for a run, both guns'
/// in versus.
pub fn draw_ground_detail_around(
    canvas: &mut Canvas,
    view: &View,
    screen: &Screen,
    guns: &[f64],
    pal: &Palette,
) {
    for sx in 0..canvas.w {
        let world_x = sx + view.cam_x;
        if world_x < 0 || world_x as usize >= screen.field.len() {
            continue;
        }
        let col = world_x as usize;
        let top = view.sy(screen.field.surface(col));
        if top < 4 || top >= canvas.h - 8 {
            continue;
        }
        let hash = detail_hash(screen, world_x);
        if screen.field.scar[col] < 2.0 && hash.is_multiple_of(17) {
            canvas.put(sx, top + 4 + ((hash >> 8) % 3) as i32, pal.get(slot::ROCK));
        }
        if !grass_grows(screen, world_x, hash) || guns.iter().any(|&g| near_gun(world_x, g)) {
            continue;
        }
        let grass = pal.get(slot::HULL_HI);
        canvas.put(sx, top - 1, grass);
        canvas.put(sx, top - 2, grass);
        canvas.put(sx - 1, top - 1, grass);
        if hash & 1 == 0 {
            canvas.put(sx + 1, top - 2, grass);
        }
    }
}

/// The hash that places a column's fleck and tuft, fixed to the world.
fn detail_hash(screen: &Screen, world_x: i32) -> u64 {
    mix64(screen.seed ^ (world_x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// Whether column `world_x` (on the field) grows a tuft, guns aside: its
/// hash picks it, no crater has touched it, and no structure stands on it.
fn grass_grows(screen: &Screen, world_x: i32, hash: u64) -> bool {
    hash.is_multiple_of(37)
        && screen.field.scar[world_x as usize].abs() <= 0.5
        && !screen
            .structures
            .iter()
            .any(|s| (world_x as f64 - s.centre_x()).abs() < 10.0)
}

/// The world columns [`draw_ground_detail_around`] would grow a tuft on in
/// a buffer `h` tall, with no guns about: the only columns a gun's
/// clearing can change. `backdrop` keys the ground layer on which of them a
/// gun keeps clear, so a gun rolling over bare ground redraws nothing.
pub fn tuft_columns(screen: &Screen, h: i32) -> impl Iterator<Item = i32> + '_ {
    let view = View { cam_x: 0, w: 0, h };
    (0..screen.field.len() as i32).filter(move |&x| {
        let top = view.sy(screen.field.surface(x as usize));
        (4..h - 8).contains(&top) && grass_grows(screen, x, detail_hash(screen, x))
    })
}

/// Whether world column `world_x` is too near a gun at world x `gun` for
/// grass: strictly within 22 columns. The one definition, so that
/// `backdrop`'s key agrees with [`draw_ground_detail_around`] to the last
/// rounding of the float subtraction.
#[inline]
pub fn near_gun(world_x: i32, gun: f64) -> bool {
    (world_x as f64 - gun).abs() < 22.0
}

/// Material banding for Sapper's Eye: a single darker line at each layer
/// boundary, so the depths are legible rather than just tinted.
pub fn draw_material_boundaries(
    canvas: &mut Canvas,
    view: &View,
    field: &Heightfield,
    pal: &Palette,
) {
    for sx in 0..canvas.w {
        let world_x = sx + view.cam_x;
        if world_x < 0 || world_x as usize >= field.len() {
            continue;
        }
        let col = world_x as usize;
        let original = field.original(col);
        for depth in [SAND_DEPTH, ROCK_DEPTH] {
            let y = view.sy(original - depth);
            if y >= view.sy(field.surface(col)) {
                canvas.put(sx, y, pal.get(slot::SHADOW));
            }
        }
    }
}

/// The dotted trajectory of the shot in flight, the shell's head in the
/// alert colour.
pub fn draw_trajectory(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    upto: usize,
    pal: &Palette,
) {
    let head = Ink::plain(pal.get(slot::ALERT));
    draw_trajectory_inked(canvas, view, path, upto, pal, head);
}

/// [`draw_trajectory`] with the shell's head, two pixels square, in `head`.
pub fn draw_trajectory_inked(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    upto: usize,
    pal: &Palette,
    head: Ink,
) {
    let end = upto.min(path.len());
    for (i, p) in path[..end].iter().enumerate() {
        if i % 6 != 0 {
            continue;
        }
        canvas.put(view.sx(p.x), view.sy(p.y), pal.get(slot::UI_FG));
    }
    if let Some(p) = path.get(end.saturating_sub(1)) {
        let (x, y) = (view.sx(p.x), view.sy(p.y));
        canvas.put_inked(&[(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)], head);
    }
}

/// The last shot's arc, left behind as a faint dotted line until the next shot
/// (or the next screen) replaces it, over layers whose colour at a pixel is
/// `under(px, py)` in display pixels: see [`Canvas::put_blend_on`].
pub fn draw_last_arc_on(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    pal: &Palette,
    under: impl Fn(i32, i32) -> Rgba,
) {
    for p in path.iter().step_by(4) {
        canvas.put_blend_on(
            view.sx(p.x),
            view.sy(p.y),
            pal.get(slot::UI_FG),
            110,
            &under,
        );
    }
}

/// A challenge's solver hint: a sparser, fainter dotted arc than the last
/// shot's, so the two read apart when they overlap.
pub fn draw_hint_arc_on(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    pal: &Palette,
    under: impl Fn(i32, i32) -> Rgba,
) {
    for p in path.iter().step_by(7) {
        canvas.put_blend_on(view.sx(p.x), view.sy(p.y), pal.get(slot::ALERT), 90, &under);
    }
}

/// Chalk Marks: the previous shot's landing point stays marked on the terrain.
pub fn draw_chalk(
    canvas: &mut Canvas,
    view: &View,
    marks: &[salt_line_sim::Vec2],
    field: &Heightfield,
    pal: &Palette,
) {
    draw_chalk_inked(canvas, view, marks, field, Ink::plain(pal.get(slot::ALERT)));
}

/// [`draw_chalk`] in `ink`: versus marks a landing in the shooter's colour.
pub fn draw_chalk_inked(
    canvas: &mut Canvas,
    view: &View,
    marks: &[salt_line_sim::Vec2],
    field: &Heightfield,
    ink: Ink,
) {
    for m in marks {
        let col = m.x.round().clamp(0.0, (field.len() - 1) as f64) as usize;
        let surface = field.surface(col);
        let y = view.sy(surface) - 1;
        let x = view.sx(m.x);
        let mut points: Vec<(i32, i32)> = (-2..=2)
            .map(|dx| {
                let c = (col as i32 + dx).clamp(0, field.len() as i32 - 1) as usize;
                (x + dx, view.sy(field.surface(c)) - 1)
            })
            .collect();
        points.push((x, y - 3));
        canvas.put_inked(&points, ink);
    }
}

/// Plumb Line: a vertical guide at the apex of the last trajectory.
pub fn draw_plumb(canvas: &mut Canvas, view: &View, apex: salt_line_sim::Vec2, pal: &Palette) {
    let x = view.sx(apex.x);
    canvas.vline_dotted(x, view.sy(apex.y), canvas.h - 1, pal.get(slot::SKY_LO), 3);
}

/// Rubble mound for a collapsed structure that has no drawn sprite yet.
pub fn draw_rubble(
    canvas: &mut Canvas,
    view: &View,
    x0: f64,
    width: i32,
    base: f64,
    pal: &Palette,
) {
    let sx0 = view.sx(x0);
    let base_y = view.sy(base);
    for i in 0..width {
        let h = 2 + (i * 7 % 3);
        for j in 0..h {
            let c = if j == h - 1 {
                pal.get(slot::ST)
            } else {
                pal.get(slot::ST_DMG)
            };
            canvas.put(sx0 + i, base_y - j, c);
        }
    }
}

/// Debug read-out of the material under a point, used by the level inspector.
pub fn material_name(m: Material) -> &'static str {
    match m {
        Material::Sand => "SAND",
        Material::Rock => "ROCK",
        Material::Bedrock => "BEDROCK",
        Material::Steel => "STEEL",
    }
}

/// A rectangle of a sprite sheet, in whole pixels.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A sprite as plain RGBA bytes, blitted straight into the buffer.
///
/// DESIGN §5.1: no scaling of any sprite, ever. There is no scale factor here
/// to set wrong — `blit` copies pixels one for one and skips transparent ones.
#[derive(Clone)]
pub struct Sprite {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
    /// Optional source-color artwork, exactly four times these logical bounds.
    pub detail: Option<Box<Sprite>>,
}

impl Sprite {
    #[inline]
    pub fn pixel(&self, x: i32, y: i32) -> Option<Rgba> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        let i = ((y * self.w + x) * 4) as usize;
        if self.px[i + 3] == 0 {
            return None;
        }
        Some([self.px[i], self.px[i + 1], self.px[i + 2], 255])
    }
}

impl Canvas {
    /// Lay `colour` over the top `depth` pixels of every column of a sprite
    /// drawn at (x, y): snow on a roof, moss on a wall. Only pixels the
    /// sprite itself covers are touched.
    pub fn cap_sprite(&mut self, sprite: &Sprite, x: i32, y: i32, colour: Rgba, depth: i32) {
        for i in 0..sprite.w {
            let Some(top) = (0..sprite.h).find(|&j| sprite.pixel(i, j).is_some()) else {
                continue;
            };
            for j in top..(top + depth).min(sprite.h) {
                if sprite.pixel(i, j).is_some() {
                    self.put(x + i, y + j, colour);
                }
            }
        }
    }

    fn blit_detail(&mut self, sprite: &Sprite, x: i32, y: i32, src: Rect, flipped: bool) -> bool {
        let Some(detail) = sprite.detail.as_ref().filter(|_| self.scale == 4) else {
            return false;
        };
        for j in 0..src.h * 4 {
            for i in 0..src.w * 4 {
                if let Some(c) = detail.pixel(src.x * 4 + i, src.y * 4 + j) {
                    let dx = if flipped { src.w * 4 - 1 - i } else { i };
                    self.put_detail(x * 4 + dx, y * 4 + j, c);
                }
            }
        }
        true
    }

    /// Blit a whole sprite at a whole-pixel position.
    pub fn blit(&mut self, sprite: &Sprite, x: i32, y: i32) {
        self.blit_region(
            sprite,
            x,
            y,
            Rect {
                x: 0,
                y: 0,
                w: sprite.w,
                h: sprite.h,
            },
        );
    }

    /// Blit one region of a sprite sheet — the barrel strip, sliced at 28px.
    pub fn blit_region(&mut self, sprite: &Sprite, x: i32, y: i32, src: Rect) {
        if self.blit_detail(sprite, x, y, src, false) {
            return;
        }
        let Rect { x: sx, y: sy, w, h } = src;
        for j in 0..h {
            for i in 0..w {
                if let Some(c) = sprite.pixel(sx + i, sy + j) {
                    self.put(x + i, y + j, c);
                }
            }
        }
    }

    /// Blit a whole sprite mirrored left to right, its left edge at `x`.
    pub fn blit_flipped(&mut self, sprite: &Sprite, x: i32, y: i32) {
        self.blit_region_flipped(
            sprite,
            x,
            y,
            Rect {
                x: 0,
                y: 0,
                w: sprite.w,
                h: sprite.h,
            },
        );
    }

    /// [`Canvas::blit_region`] mirrored left to right: the region's last
    /// column lands at `x`. A reflection, not a rotation, so DESIGN §5.1's
    /// rule against rotating sprites still holds: every pixel is copied one
    /// for one. Versus draws the right-hand gun this way.
    pub fn blit_region_flipped(&mut self, sprite: &Sprite, x: i32, y: i32, src: Rect) {
        if self.blit_detail(sprite, x, y, src, true) {
            return;
        }
        let Rect { x: sx, y: sy, w, h } = src;
        for j in 0..h {
            for i in 0..w {
                if let Some(c) = sprite.pixel(sx + i, sy + j) {
                    self.put(x + (w - 1 - i), y + j, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use salt_line_sim::rng::screen_seed;
    use salt_line_sim::terrain::{self, ShapeParams, Template};

    #[test]
    fn detail_retains_logical_coordinates_and_resizes_physical_storage() {
        let mut c = Canvas::detailed(3, 2);
        c.put(1, 0, [1, 2, 3, 255]);
        assert_eq!((c.w, c.h, c.pixel_w(), c.pixel_h()), (3, 2, 12, 8));
        for y in 0..8 {
            for x in 0..12 {
                let i = ((y * 12 + x) * 4) as usize;
                let expected = if y < 4 && (4..8).contains(&x) {
                    [1, 2, 3, 255]
                } else {
                    [0; 4]
                };
                assert_eq!(&c.bytes()[i..i + 4], &expected);
            }
        }
        c.resize(4, 3);
        assert_eq!(c.bytes().len(), 16 * 12 * 4);
        assert!(c.bytes().iter().all(|&v| v == 0));
        assert_eq!(c.scale(), 4);
    }

    #[test]
    fn source_pixels_survive_sheet_slicing_mirroring_and_later_occlusion() {
        let detail = Sprite {
            w: 8,
            h: 4,
            px: (0..32)
                .flat_map(|i| [i, 0, 0, if i % 8 == 7 { 0 } else { 255 }])
                .collect(),
            detail: None,
        };
        let sprite = Sprite {
            w: 2,
            h: 1,
            px: vec![99; 8],
            detail: Some(Box::new(detail)),
        };
        let mut c = Canvas::detailed(2, 1);
        c.fill([80, 80, 80, 255]);
        c.blit_region_flipped(
            &sprite,
            0,
            0,
            Rect {
                x: 1,
                y: 0,
                w: 1,
                h: 1,
            },
        );
        assert_eq!(
            &c.bytes()[0..4],
            &[80, 80, 80, 255],
            "transparent source leaves the backing"
        );
        assert_eq!(&c.bytes()[4..8], &[6, 0, 0, 255]);
        assert_eq!(&c.bytes()[12..16], &[4, 0, 0, 255]);
        c.put(0, 0, [42, 0, 0, 255]);
        assert_eq!(
            c.get(0, 0),
            [42, 0, 0, 255],
            "later terrain/UI covers every source pixel"
        );
        c.blit_region(
            &sprite,
            -1,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
        );
        assert_eq!(
            &c.bytes()[0..4],
            &[4, 0, 0, 255],
            "clipping keeps the correct sheet pixel"
        );
    }

    #[test]
    fn translucent_arc_blends_each_source_pixel_without_erasing_detail() {
        let mut c = Canvas::detailed(1, 1);
        for x in 0..4 {
            c.put_detail(x, 0, [x as u8 * 40, 0, 0, 255]);
        }
        c.put_blend_on(0, 0, [200, 0, 0, 255], 128, |_, _| [80, 0, 0, 255]);
        assert_eq!(c.bytes()[0], 100);
        assert_eq!(c.bytes()[12], 160);
        assert_eq!(
            c.bytes()[16],
            140,
            "transparent pixels blend against the supplied backing"
        );
        assert!(c.bytes().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn detail_terrain_uses_the_identical_heightfield_mask() {
        let mut field = field(320);
        field.crater(110.0, 28.0);
        field.crater(200.0, 16.0);
        let view = View {
            cam_x: 0,
            w: 320,
            h: 180,
        };
        let pal = Palette::default();
        let mut native = Canvas::new(320, 180);
        let mut detail = Canvas::detailed(320, 180);
        draw_terrain(&mut native, &view, &field, &pal);
        draw_terrain(&mut detail, &view, &field, &pal);
        for y in 0..180 {
            for x in 0..320 {
                let alpha = native.get(x, y)[3];
                for dy in 0..4 {
                    for dx in 0..4 {
                        let i = (((y * 4 + dy) * 1280 + x * 4 + dx) * 4) as usize;
                        assert_eq!(detail.bytes()[i + 3], alpha);
                    }
                }
            }
        }
    }

    #[test]
    fn a_cap_covers_only_the_top_of_the_sprite() {
        // A 3x3 sprite with a hole in its top-left corner.
        let mut px = vec![0u8; 3 * 3 * 4];
        for j in 0..3 {
            for i in 0..3 {
                if (i, j) != (0, 0) {
                    let at = (j * 3 + i) * 4;
                    px[at..at + 4].copy_from_slice(&[10, 20, 30, 255]);
                }
            }
        }
        let sprite = Sprite {
            w: 3,
            h: 3,
            px,
            detail: None,
        };
        let mut canvas = Canvas::new(5, 5);
        canvas.blit(&sprite, 1, 1);
        let white = [255, 255, 255, 255];
        canvas.cap_sprite(&sprite, 1, 1, white, 1);
        let at = |c: &Canvas, x: i32, y: i32| {
            let i = ((y * c.w + x) * 4) as usize;
            [
                c.bytes()[i],
                c.bytes()[i + 1],
                c.bytes()[i + 2],
                c.bytes()[i + 3],
            ]
        };
        assert_eq!(at(&canvas, 1, 1), [0, 0, 0, 0], "the hole stays open");
        assert_eq!(
            at(&canvas, 1, 2),
            white,
            "the column's first pixel is capped"
        );
        assert_eq!(at(&canvas, 2, 1), white);
        assert_eq!(
            at(&canvas, 2, 2),
            [10, 20, 30, 255],
            "below the cap is untouched"
        );
    }

    fn field(width: usize) -> Heightfield {
        let p = ShapeParams {
            width,
            amp: 1.2,
            muzzle_x: 30.0,
            target_x: 200.0,
            elev_delta: 6.0,
            spire_count: 2,
        };
        terrain::generate(screen_seed(0xA11CE, 3), Template::Flat, &p)
    }

    fn view(cam_x: i32, w: i32) -> View {
        View {
            cam_x,
            w,
            h: SCREEN_H as i32,
        }
    }

    fn column(canvas: &Canvas, x: i32) -> Vec<Rgba> {
        (0..canvas.h)
            .map(|y| {
                let i = ((y * canvas.w + x) * 4) as usize;
                let mut c = [0u8; 4];
                for (k, b) in c.iter_mut().enumerate() {
                    *b = canvas.px_at(i + k);
                }
                c
            })
            .collect()
    }

    #[test]
    fn no_grass_grows_under_either_versus_gun() {
        use crate::versus::{self, Side};
        let pal = Palette::default();
        let grass = pal.get(slot::HULL_HI);
        // A tuft is three pixels wide, so one just outside the 22-column
        // clearing reaches a column in: look within 20.
        let has_grass_near = |canvas: &Canvas, gx: f64| {
            let lo = (gx - 20.0).ceil() as i32;
            let hi = (gx + 20.0).floor() as i32;
            (lo..=hi).any(|x| column(canvas, x).contains(&grass))
        };
        let lo = salt_line_sim::modifiers::Loadout::new();
        let mut seen_grass_there = false;
        for seed in 0..40u64 {
            let s = versus::generate_duel(seed, 0, 320, &lo, &versus::Rules::DEFAULT);
            let right = versus::muzzle_of(&s, Side::Right).x;
            let guns = Side::BOTH.map(|g| versus::muzzle_of(&s, g).x);
            let mut single = Canvas::new(320, SCREEN_H as i32);
            draw_ground_detail(&mut single, &view(0, 320), &s, &pal);
            seen_grass_there |= has_grass_near(&single, right);
            let mut duel = Canvas::new(320, SCREEN_H as i32);
            draw_ground_detail_around(&mut duel, &view(0, 320), &s, &guns, &pal);
            for g in guns {
                assert!(!has_grass_near(&duel, g), "seed {seed}: grass at {g}");
            }
        }
        assert!(
            seen_grass_there,
            "no seed grew grass by the right gun at all"
        );
    }

    #[test]
    fn panning_the_camera_translates_terrain_exactly() {
        // DESIGN §5.1: a camera at x=41.3 makes the entire terrain shimmer as
        // it pans, and that is the most common way low-res games look cheap.
        // Camera x is an integer here, so a one-pixel pan must move every
        // terrain column by exactly one pixel and change nothing else.
        let f = field(400);
        let pal = Palette::default();
        let mut a = Canvas::new(320, SCREEN_H as i32);
        let mut b = Canvas::new(320, SCREEN_H as i32);
        draw_terrain(&mut a, &view(20, 320), &f, &pal);
        draw_terrain(&mut b, &view(21, 320), &f, &pal);
        for x in 0..319 {
            assert_eq!(
                column(&a, x + 1),
                column(&b, x),
                "column {x} is not a pure translation"
            );
        }
    }

    #[test]
    fn a_crater_lowers_only_the_columns_it_touches() {
        let mut f = field(320);
        let pal = Palette::default();
        let v = view(0, 320);
        let mut before = Canvas::new(320, SCREEN_H as i32);
        draw_terrain(&mut before, &v, &f, &pal);
        f.crater(160.0, 7.0);
        let mut after = Canvas::new(320, SCREEN_H as i32);
        draw_terrain(&mut after, &v, &f, &pal);
        for x in 0..320 {
            let d = (x as f64 - 160.0).abs();
            let changed = column(&before, x) != column(&after, x);
            // Outside the radius nothing may move.
            assert!(
                d < 8.0 || !changed,
                "column {x} is outside the crater but changed"
            );
            // Inside the core the drop is more than a pixel, so every one of
            // those columns must move. The rim is left out: there the drop is
            // a fraction of a pixel and can legitimately round to no change.
            // Asserting only the first half of that pair is what let this test
            // pass while drawing nothing at all.
            assert!(
                d > 4.0 || changed,
                "column {x} is inside the crater but did not change"
            );
        }
    }

    #[test]
    fn exposed_layers_match_what_the_sim_charges_for() {
        // The bands are drawn from the original surface, so the colour at the
        // bottom of a crater is the material the next shell will crater
        // against.
        let mut f = Heightfield::from_base(vec![100.0; 64]);
        let pal = Palette::default();
        let v = view(0, 64);
        f.crater(32.0, 14.0); // deep enough to be into rock
        let mut c = Canvas::new(64, SCREEN_H as i32);
        draw_terrain(&mut c, &v, &f, &pal);
        let top = v.sy(f.surface(32));
        let i = ((top * c.w + 32) * 4) as usize;
        let drawn = [c.px_at(i), c.px_at(i + 1), c.px_at(i + 2), c.px_at(i + 3)];
        // The rim shadow can claim the very top pixel; check the row below it.
        let i2 = (((top + 2) * c.w + 32) * 4) as usize;
        let below = [
            c.px_at(i2),
            c.px_at(i2 + 1),
            c.px_at(i2 + 2),
            c.px_at(i2 + 3),
        ];
        let material = f.material_at(32.0);
        assert_eq!(material, Material::Rock);
        assert!(
            below == pal.get(slot::ROCK) || drawn == pal.get(slot::ROCK),
            "crater floor drawn as {below:?}, expected rock {:?}",
            pal.get(slot::ROCK)
        );
    }

    #[test]
    fn the_buffer_clips_instead_of_panicking() {
        let mut c = Canvas::new(8, 8);
        let white = [255, 255, 255, 255];
        c.put(-1, 4, white);
        c.put(4, -1, white);
        c.put(8, 8, white);
        c.rect(-4, -4, 100, 100, white);
        c.text_small(-3, 6, "SALT LINE", white);
        c.text_big(6, 6, "1234", white);
    }

    #[test]
    fn view_maps_world_to_screen_without_fractions() {
        let v = view(30, 320);
        assert_eq!(v.sx(30.0), 0);
        assert_eq!(v.sx(30.4), 0);
        assert_eq!(v.sx(31.0), 1);
        // World y is up, screen y is down.
        assert_eq!(v.sy(0.0), SCREEN_H as i32 - 1);
        assert_eq!(v.sy((SCREEN_H - 1) as f64), 0);
        // Extra buffer height becomes sky above the world.
        let tall = View {
            cam_x: 0,
            w: 320,
            h: 216,
        };
        assert_eq!(tall.sy(0.0), 215);
        assert_eq!(tall.world_row(215), 0);
    }

    /// A whole frame of a real screen, sky, terrain and a shot's arc, pinned
    /// pixel for pixel. Stands in for the byte-identical smoke frame check.
    #[test]
    #[cfg_attr(miri, ignore = "insta opens a socket, which Miri cannot emulate")]
    fn a_frame_of_a_real_screen_is_pinned() {
        use salt_line_sim::ballistics::{fire, FireOptions, Shot};
        use salt_line_sim::modifiers::Loadout;
        use salt_line_sim::run::generate_screen;

        let lo = Loadout::default();
        let screen = generate_screen(20260907, 12, 320, &lo);
        let pal = Palette::default();
        let v = view(0, 320);
        let mut c = Canvas::new(320, SCREEN_H as i32);
        draw_sky(&mut c, &v, &pal);
        draw_terrain(&mut c, &v, &screen.field, &pal);
        let boxes = screen.target_boxes();
        let opts = FireOptions {
            record_path: true,
            ..FireOptions::default()
        };
        let traces = fire(
            &screen.world(&boxes),
            screen.muzzle(),
            Shot::new(900, 600),
            &lo,
            &opts,
        );
        let path = &traces[0].path;
        draw_trajectory(&mut c, &v, path, path.len(), &pal);
        insta::assert_snapshot!(c.palette_text(&pal));
    }

    #[test]
    fn scaled_text_is_the_small_face_in_whole_blocks() {
        let c = [1, 2, 3, 255];
        let mut small = Canvas::new(40, 8);
        let end = small.text_small(1, 1, "HIT!", c);
        let mut one = Canvas::new(40, 8);
        assert_eq!(one.text_small_scaled(1, 1, "HIT!", c, 1), end);
        assert_eq!(one.bytes(), small.bytes(), "scale 1 is the small face");

        let mut big = Canvas::new(160, 32);
        let end = big.text_small_scaled(0, 0, "HIT!", c, 4);
        assert_eq!(end, 4 * crate::font::SMALL_ADVANCE as i32 * 4);
        let lit = |canvas: &Canvas, x: i32, y: i32| {
            canvas.bytes()[((y * canvas.w + x) * 4) as usize] != 0
        };
        for y in 0..big.h {
            for x in 0..big.w {
                let want =
                    x / 4 + 1 < small.w && y / 4 + 1 < small.h && lit(&small, x / 4 + 1, y / 4 + 1);
                assert_eq!(lit(&big, x, y), want, "({x}, {y})");
            }
        }
    }
}

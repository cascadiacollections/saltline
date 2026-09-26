//! The pixel buffer, and everything drawn into it.
//!
//! DESIGN §5.5: terrain is vertical spans of solid colour, one pixel column
//! each, straight into an image buffer. No tilemap, no mesh, no sprites for
//! the ground. It is a per-column loop and it is fast enough at 320 wide to
//! redraw every frame.
//!
//! Nothing here scales, rotates or anti-aliases. Every coordinate that reaches
//! `put` is already a whole pixel.

use godot::builtin::PackedByteArray;
use salt_line_sim::terrain::{Heightfield, Material, ROCK_DEPTH, SAND_DEPTH};
use salt_line_sim::SCREEN_H;

use crate::font;
use crate::palette::{slot, Palette, Rgba};

/// 4x4 Bayer matrix. The only dithering in the game is the sky gradient and
/// the material transition band — DESIGN §5.2 is explicit that dithering
/// everything is the other common low-res failure.
pub const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

#[inline]
fn bayer(x: i32, y: i32) -> f64 {
    let bx = x.rem_euclid(4) as usize;
    let by = y.rem_euclid(4) as usize;
    (BAYER[by][bx] as f64 + 0.5) / 16.0
}

/// An RGBA8 pixel buffer at the internal resolution.
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        Canvas {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
        }
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.px = vec![0; (w * h * 4) as usize];
        }
    }

    pub fn clear(&mut self) {
        self.px.fill(0);
    }

    pub fn fill(&mut self, c: Rgba) {
        for p in self.px.chunks_exact_mut(c.len()) {
            p.copy_from_slice(&c);
        }
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        self.px[i..i + 4].copy_from_slice(&c);
    }

    /// Alpha-blended write, for the few translucent plates in the HUD.
    #[inline]
    pub fn put_blend(&mut self, x: i32, y: i32, c: Rgba, alpha: u8) {
        if alpha == 255 {
            return self.put(x, y, c);
        }
        if x < 0 || y < 0 || x >= self.w || y >= self.h || alpha == 0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let a = alpha as u32;
        for (dst, src) in self.px[i..i + 3].iter_mut().zip(c.iter()) {
            let under = *dst as u32;
            *dst = ((*src as u32 * a + under * (255 - a)) / 255) as u8;
        }
        self.px[i + 3] = 255;
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
    pub cam_x: i32,
    pub w: i32,
    pub h: i32,
}

impl View {
    /// World x to screen x.
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
        let t = world_row as f64 / 105.0;
        for x in 0..canvas.w {
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
            canvas.put(sx, y, c);
        }

        // Crater rims get one pixel of the shadow tone on the inner edge.
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

/// The dotted trajectory of the shot in flight.
pub fn draw_trajectory(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    upto: usize,
    pal: &Palette,
) {
    let end = upto.min(path.len());
    for (i, p) in path[..end].iter().enumerate() {
        if i % 6 != 0 {
            continue;
        }
        canvas.put(view.sx(p.x), view.sy(p.y), pal.get(slot::UI_FG));
    }
    if let Some(head) = path.get(end.saturating_sub(1)) {
        let (x, y) = (view.sx(head.x), view.sy(head.y));
        canvas.rect(x, y, 2, 2, pal.get(slot::ALERT));
    }
}

/// The last shot's arc, left behind as a faint dotted line until the next shot
/// (or the next screen) replaces it.
pub fn draw_last_arc(
    canvas: &mut Canvas,
    view: &View,
    path: &[salt_line_sim::Vec2],
    pal: &Palette,
) {
    for p in path.iter().step_by(4) {
        canvas.put_blend(view.sx(p.x), view.sy(p.y), pal.get(slot::UI_FG), 110);
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
    for m in marks {
        let col = m.x.round().clamp(0.0, (field.len() - 1) as f64) as usize;
        let surface = field.surface(col);
        let y = view.sy(surface) - 1;
        let x = view.sx(m.x);
        for dx in -2..=2 {
            let c = (col as i32 + dx).clamp(0, field.len() as i32 - 1) as usize;
            canvas.put(x + dx, view.sy(field.surface(c)) - 1, pal.get(slot::ALERT));
        }
        canvas.put(x, y - 3, pal.get(slot::ALERT));
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
        let Rect { x: sx, y: sy, w, h } = src;
        for j in 0..h {
            for i in 0..w {
                if let Some(c) = sprite.pixel(sx + i, sy + j) {
                    self.put(x + i, y + j, c);
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
}

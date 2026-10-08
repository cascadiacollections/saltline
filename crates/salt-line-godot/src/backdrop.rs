//! The layered frame: the sky and the ground as textures of their own, so
//! most frames upload only what moved over them.
//!
//! # Why
//!
//! A frame used to be one canvas, sky and ground included, rasterised and
//! compared with the screen every frame, and uploaded where it changed (see
//! `strips`). On a phone both halves of that are dear, and the sky and the
//! ground are most of both:
//!
//! * they are most of the raster: the sky is a fill of every pixel and the
//!   ground a span per column, against a few hundred pixels of sprites and
//!   HUD;
//! * they are what turns a few changed strips into a whole-frame upload. The
//!   landing shake moves the camera a pixel or two, every ground row
//!   changes, and the whole frame goes up for a frame that moved nothing but
//!   the camera.
//!
//! Yet neither changes often. The sky's fill, band and sun depend on the
//! screen's phase and the buffer's size, and the ground only changes when a
//! shell lands.
//!
//! # How
//!
//! A frame of the playfield is drawn as three layers, back to front:
//!
//! 1. **Sky**: [`sky::draw_still`] into a canvas the size of the buffer.
//!    Re-drawn (and uploaded by the node) only when its inputs change.
//! 2. **Ground**: [`raster::draw_terrain`] and the ground detail for the
//!    *whole world*, every column of the heightfield, into a canvas the
//!    world's width (and [`GROUND_PAD`]), transparent above the surface. Re-drawn only when the
//!    heightfield or what the detail depends on changes. The node draws the
//!    `cam_x`-th column at the buffer's left edge, so a camera move, the
//!    landing shake included, is a different source rectangle on the GPU and
//!    no upload at all.
//! 3. **Overlay**: everything else, drawn by the CPU into the frame's canvas
//!    as before, but over transparent pixels instead of over the sky. It is
//!    still uploaded in strips (`strips`), and most of it is transparent, so
//!    fewer strips change.
//!
//! The layers only move by whole pixels and the GPU only copies them (nearest
//! filtering, no scaling; Godot's viewport stretch scales the finished
//! viewport as before). The GPU draws no shader of its own: every pixel of
//! every layer is drawn by the same Rust code that drew the single canvas.
//!
//! # Why the result is pixel-exact
//!
//! The flat frame, drawn as before, is the reference; the layered frame has
//! to come out identical, pixel for pixel. Painting is in order (sky, then
//! ground, then everything else), so the three layers composite to the flat
//! frame provided
//!
//! * the overlay's pixels are each either transparent (alpha 0, the layers
//!   beneath show) or opaque (alpha 255, it hides them). Blending is then
//!   exact on any GPU: `src * 1 + dst * 0` or `src * 0 + dst * 1`. Every
//!   colour the game paints is opaque, and [`Canvas::put_blend_on`], the one
//!   translucent write, blends with the colour beneath ([`Backdrop::under`])
//!   on the CPU and writes the opaque result;
//! * the ground layer, seen through the window `cam_x .. cam_x + w`, is what
//!   the flat frame's per-view drawing gives. Every ground pixel is a
//!   function of its world column and its row, except a grass tuft, which
//!   spills one pixel into the column either side. Drawn per view, a column
//!   just off either edge is not drawn and spills nothing in; drawn for the
//!   whole world, it does. So the two edge columns are worked out per view
//!   as well, by drawing a two-column view at each edge, and any pixel that
//!   differs is painted into the overlay (see `edge_patches`);
//! * the stars, drawn in the flat frame before the ground hides them, are in
//!   the overlay, and drawn only where no ground is in front of them
//!   (`sky::draw_stars`).
//!
//! The tests below hold the layered frame to this: [`Backdrop::composite`]
//! puts the three layers together on the CPU as the node does on the GPU,
//! and every frame of autopiloted runs, versus matches, the editor and the
//! demo, at several buffer sizes, must equal the flat frame byte for byte.
//! The profiling build checks the GPU's side on the device itself: it reads
//! the viewport back and compares it with the flat frame (`profiling`).
//!
//! `layered` off draws the flat frame, as before, for a fallback and for
//! the tests' reference.

use salt_line_sim::run::Screen;

use crate::palette::{Palette, Rgba};
use crate::raster::{self, Canvas, View};
use crate::sky;

/// The ground layer starts this many columns left of world column 0, and
/// ends as many right of the last: a tuft on the world's first or last
/// column spills a pixel past it, and a view shaken past the world's end
/// shows that pixel. Drawn per view it is there, so it must be in the layer.
pub const GROUND_PAD: i32 = 1;

/// The layers kept from frame to frame, and what this frame shows of them.
pub struct Backdrop {
    /// Draw frames in layers. Off (the default) draws the flat frame.
    pub layered: bool,
    sky: Kept<SkyKey>,
    ground: Kept<GroundKey>,
    /// The overlay pixels that put the ground's edge columns right. See
    /// `edge_patches`.
    patches: Vec<Patch>,
    /// Scratch canvas for `edge_patches`.
    narrow: Canvas,
    /// What this frame shows under the overlay: `None` when the overlay is
    /// the whole frame (a menu, or `layered` off).
    shown: Option<Shown>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Backdrop {
            layered: false,
            sky: Kept::default(),
            ground: Kept::default(),
            patches: Vec::new(),
            narrow: Canvas::new(2, 1),
            shown: None,
        }
    }
}

/// One layer's canvas and the inputs it was drawn from.
struct Kept<K> {
    key: Option<K>,
    canvas: Canvas,
    /// Bumped each time the canvas is drawn again, so the node can tell it
    /// needs uploading.
    version: u64,
}

impl<K> Default for Kept<K> {
    fn default() -> Self {
        Kept {
            key: None,
            canvas: Canvas::new(1, 1),
            version: 0,
        }
    }
}

/// What the sky layer was drawn from.
#[derive(PartialEq)]
struct SkyKey {
    phase: sky::Phase,
    w: i32,
    h: i32,
    pal: [Rgba; 16],
}

/// What the ground layer was drawn from: everything `raster::draw_terrain`
/// and `raster::draw_ground_detail_around` read.
#[derive(PartialEq)]
struct GroundKey {
    base: Vec<f64>,
    scar: Vec<f64>,
    seed: u64,
    structures: Vec<f64>,
    guns: Vec<f64>,
    h: i32,
    pal: [Rgba; 16],
}

impl GroundKey {
    fn matches(&self, screen: &Screen, guns: &[f64], h: i32, pal: &Palette) -> bool {
        self.seed == screen.seed
            && self.h == h
            && self.pal == pal.colours
            && self.base == screen.field.base
            && self.scar == screen.field.scar
            && self.guns == guns
            && self
                .structures
                .iter()
                .copied()
                .eq(screen.structures.iter().map(|s| s.centre_x()))
    }

    fn of(screen: &Screen, guns: &[f64], h: i32, pal: &Palette) -> Self {
        GroundKey {
            base: screen.field.base.clone(),
            scar: screen.field.scar.clone(),
            seed: screen.seed,
            structures: screen.structures.iter().map(|s| s.centre_x()).collect(),
            guns: guns.to_vec(),
            h,
            pal: pal.colours,
        }
    }
}

/// A pixel of the overlay at `(x, y)` that corrects the ground there: the
/// ground's colour per view, or `None` where per view there is no ground.
#[derive(Clone, Copy, Debug)]
struct Patch {
    x: i32,
    y: i32,
    ground: Option<Rgba>,
}

/// What a layered frame shows under its overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The world column at the buffer's left edge (`View::cam_x`, shake
    /// included). May be negative or run past the world's end: no ground
    /// shows there.
    pub cam_x: i32,
}

impl Shown {
    /// The ground layer's column at the buffer's left edge.
    pub fn ground_x(self) -> i32 {
        self.cam_x + GROUND_PAD
    }
}

/// Where the ground layer goes: its columns `src_x .. src_x + w` land at
/// buffer columns `dst_x .. dst_x + w`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub src_x: i32,
    pub dst_x: i32,
    pub w: i32,
}

/// The part of a ground layer `layer_w` columns wide that shows in a buffer
/// `buffer_w` wide whose left edge is the layer's column `left`; `None` if
/// none of it does. The node draws exactly this rectangle, so nothing is
/// ever sampled from outside the texture.
pub fn window(left: i32, buffer_w: i32, layer_w: i32) -> Option<Window> {
    let src_x = left.max(0);
    let end = (left + buffer_w).min(layer_w);
    (end > src_x).then_some(Window {
        src_x,
        dst_x: src_x - left,
        w: end - src_x,
    })
}

impl Backdrop {
    /// A new frame: nothing shown under the overlay until a playfield's
    /// backdrop is drawn.
    pub fn begin_frame(&mut self) {
        self.shown = None;
    }

    /// What this frame shows under the overlay, if anything.
    pub fn shown(&self) -> Option<Shown> {
        self.shown
    }

    /// The sky layer, and how many times it has been drawn.
    pub fn sky(&self) -> (&Canvas, u64) {
        (&self.sky.canvas, self.sky.version)
    }

    /// The ground layer, and how many times it has been drawn.
    pub fn ground(&self) -> (&Canvas, u64) {
        (&self.ground.canvas, self.ground.version)
    }

    /// Draw a playfield's sky and ground: the screen's `phase` sky with its
    /// stars (`seed`, `time`), and `screen`'s ground with its detail, no
    /// grass near `guns`. Flat, into `canvas` as before; layered, into the
    /// two layers where they changed, leaving `canvas` transparent but for
    /// the stars and the edge patches.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        view: &View,
        pal: &Palette,
        phase: sky::Phase,
        seed: u64,
        time: f64,
        screen: &Screen,
        guns: &[f64],
    ) {
        if !self.layered {
            sky::draw(canvas, view, pal, phase, seed, time);
            raster::draw_terrain(canvas, view, &screen.field, pal);
            raster::draw_ground_detail_around(canvas, view, screen, guns, pal);
            return;
        }
        canvas.clear();
        self.keep_sky(view, pal, phase);
        self.keep_ground(screen, guns, view.h, pal);
        self.shown = Some(Shown { cam_x: view.cam_x });
        self.edge_patches(view, screen, guns, pal);
        for p in &self.patches {
            // No ground here per view: the sky shows, so the overlay must
            // show it too, over the ground layer's stray pixel.
            let c = p.ground.unwrap_or_else(|| self.sky.canvas.get(p.x, p.y));
            canvas.put(p.x, p.y, c);
        }
        sky::draw_stars(canvas, view, pal, phase, seed, time, |x, y| {
            self.ground_at(x, y).is_none()
        });
    }

    fn keep_sky(&mut self, view: &View, pal: &Palette, phase: sky::Phase) {
        let key = SkyKey {
            phase,
            w: view.w,
            h: view.h,
            pal: pal.colours,
        };
        if self.sky.key.as_ref() == Some(&key) {
            return;
        }
        self.sky.canvas.resize(view.w, view.h);
        sky::draw_still(&mut self.sky.canvas, view, pal, phase);
        self.sky.key = Some(key);
        self.sky.version += 1;
    }

    fn keep_ground(&mut self, screen: &Screen, guns: &[f64], h: i32, pal: &Palette) {
        if let Some(key) = &self.ground.key {
            if key.matches(screen, guns, h, pal) {
                return;
            }
        }
        let world = View {
            cam_x: -GROUND_PAD,
            w: screen.field.len() as i32 + 2 * GROUND_PAD,
            h,
        };
        let canvas = &mut self.ground.canvas;
        canvas.resize(world.w, h);
        canvas.clear();
        raster::draw_terrain(canvas, &world, &screen.field, pal);
        raster::draw_ground_detail_around(canvas, &world, screen, guns, pal);
        self.ground.key = Some(GroundKey::of(screen, guns, h, pal));
        self.ground.version += 1;
    }

    /// Find where the ground layer, seen through this view, differs from the
    /// ground drawn for this view alone, which is what the flat frame shows.
    /// Only a grass tuft from a column just off either edge can make it
    /// differ (see the module docs), so only the edge columns are checked:
    /// each is drawn per view by drawing a view two columns wide, the edge
    /// column and its inner neighbour, the only columns that paint it.
    fn edge_patches(&mut self, view: &View, screen: &Screen, guns: &[f64], pal: &Palette) {
        self.patches.clear();
        let ground = &self.ground.canvas;
        let (w, h) = (view.w, view.h);
        // (the narrow view's left edge, the edge column within it, and the
        // buffer column that is)
        let edges = [(view.cam_x, 0, 0), (view.cam_x + w - 2, 1, w - 1)];
        for (i, &(cam_x, col, x)) in edges.iter().enumerate() {
            if i == 1 && w < 2 {
                break;
            }
            let narrow_view = View { cam_x, w: 2, h };
            self.narrow.resize(2, h);
            self.narrow.clear();
            raster::draw_terrain(&mut self.narrow, &narrow_view, &screen.field, pal);
            raster::draw_ground_detail_around(&mut self.narrow, &narrow_view, screen, guns, pal);
            for y in 0..h {
                let per_view = opaque(self.narrow.get(col, y));
                let layer = opaque(ground.get(x + view.cam_x + GROUND_PAD, y));
                if per_view != layer {
                    self.patches.push(Patch {
                        x,
                        y,
                        ground: per_view,
                    });
                }
            }
        }
    }

    /// The ground's colour at buffer pixel `(x, y)` as this frame shows it,
    /// patches included; `None` where the sky shows.
    fn ground_at(&self, x: i32, y: i32) -> Option<Rgba> {
        if let Some(p) = self.patches.iter().find(|p| p.x == x && p.y == y) {
            return p.ground;
        }
        let left = self.shown.map_or(0, Shown::ground_x);
        opaque(self.ground.canvas.get(x + left, y))
    }

    /// The colour the layers beneath the overlay show at buffer pixel
    /// `(x, y)`: the ground's, or else the sky's. For a translucent write
    /// over a pixel the overlay has not painted ([`Canvas::put_blend_on`]).
    /// Flat, there is nothing beneath: the canvas holds every colour itself.
    pub fn under(&self, x: i32, y: i32) -> Rgba {
        if self.shown.is_none() {
            return [0; 4];
        }
        self.ground_at(x, y)
            .unwrap_or_else(|| self.sky.canvas.get(x, y))
    }

    /// The finished frame from `overlay`, this frame's canvas: the layers put
    /// together on the CPU exactly as the node draws them on the GPU, sky,
    /// then the ground's [`window`], then the overlay, each opaque pixel
    /// hiding what is under it. For captures (`--salt-shot`) and tests; the
    /// flat frame is `overlay` itself.
    pub fn composite(&self, overlay: &Canvas) -> Vec<u8> {
        let Some(shown) = self.shown else {
            return overlay.bytes().to_vec();
        };
        let (w, h) = (overlay.w, overlay.h);
        let mut out = self.sky.canvas.bytes().to_vec();
        let ground = &self.ground.canvas;
        let stride = w as usize * 4;
        if let Some(win) = window(shown.ground_x(), w, ground.w) {
            for y in 0..h.min(ground.h) {
                for i in 0..win.w {
                    let c = ground.get(win.src_x + i, y);
                    if c[3] != 0 {
                        let at = y as usize * stride + (win.dst_x + i) as usize * 4;
                        out[at..at + 4].copy_from_slice(&c);
                    }
                }
            }
        }
        for (dst, src) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(overlay.bytes().as_chunks::<4>().0)
        {
            if src[3] != 0 {
                dst.copy_from_slice(src);
            }
        }
        out
    }
}

/// `c` if it is painted, `None` if transparent.
fn opaque(c: Rgba) -> Option<Rgba> {
    (c[3] != 0).then_some(c)
}

#[cfg(test)]
mod tests;

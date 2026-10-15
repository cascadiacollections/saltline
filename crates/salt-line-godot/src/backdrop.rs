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
//! 3. **Outline**: the alert outline a collapse flashes around the buffer
//!    ([`Backdrop::outline`]), a texture drawn once and shown or not. It is
//!    one pixel wide but touches every row, so drawn into the frame it would
//!    send the whole frame up every time it blinked.
//! 4. **Overlay**: everything else, drawn by the CPU into the frame's canvas
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
//!   (`sky::draw_stars`);
//! * the outline is painted mid-frame: whatever the overlay holds on the
//!   outline's pixels by then is painted over in the flat frame, so
//!   [`Backdrop::outline`] makes those pixels transparent again, and the
//!   outline layer under the overlay shows through them until something
//!   painted later covers it, as in the flat frame;
//! * the overlay on the GPU is the canvas as last drawn. It goes up in
//!   strips over a whole-frame texture (`strips`), so each of its rows must
//!   be drawn from one texture only: a transparent pixel of a strip drawn
//!   over the whole-frame texture would show that texture's older pixel
//!   (`strips::Layers::whole_rows`, and the test there).
//!
//! * at the detailed scale (`Canvas::detailed`, the source art at 4 display
//!   pixels per buffer pixel) the layers are drawn at the frame's scale, so
//!   they carry the same detailed sky and soil. Every ground pixel is still
//!   painted a whole buffer pixel at a time, so whether ground shows (for
//!   the stars) is one sample per buffer pixel; but a buffer pixel's display
//!   pixels differ in colour, so everything that copies a colour from a
//!   layer, the edge patches, [`Backdrop::under`] and
//!   [`Backdrop::composite`], works display pixel by display pixel.
//!
//! The tests below hold the layered frame to this: [`Backdrop::composite`]
//! puts the three layers together on the CPU as the node does on the GPU,
//! and every frame of autopiloted runs, versus matches, the editor and the
//! demo, at several buffer sizes and at both scales, must equal the flat
//! frame byte for byte.
//! The profiling build checks the GPU's side on the device itself: it reads
//! the viewport back and compares it with the flat frame (`profiling`).
//!
//! `layered` off draws the flat frame, as before: the reference the layers
//! are tested against, and a developer fallback (`--salt-flat`) for
//! telling a layering bug from any other. It is not a player setting.
//!
//! Every caller of [`Backdrop::draw`] (a run, versus, the editor and its
//! test fire, and the demo and a watch through them) needs a scene in `backdrop/tests.rs`
//! that draws it both ways; a new caller without one is untested here.
//!
//! # Measured
//!
//! Pixel 10, the profiling build's fixed-seed autopilot run, four runs of
//! each build interleaved (2026-10): process time per frame, mean 1.51 ms
//! to 1.00 ms, p95 3.48 to 2.00 ms, p50 1.00 to 0.53 ms. The CPU raster went
//! from 0.60 to 0.20 ms, the upload pass (comparing and every texture
//! update) from 0.60 to 0.49 ms, and whole-frame uploads from 221 to 34 in
//! ~960 frames (plus 25 sky or ground uploads, about one per landing). The
//! worst frame of a run was 22 to 44 ms on main and 24 to 51 ms here: rare
//! one-off frames this does not touch, as noisy in both builds.
//!
//! Power was not measured. `strips::WHOLE_ABOVE` records a power-rail
//! capture in which upload choices made no difference the rails could
//! show, so claim frame time, not battery or heat. This was measured on one
//! phone and GPU, the Pixel 10's; re-measure on a Mali GPU (a Galaxy A16)
//! before a release leans on it.
//!
//! # Why not a shader
//!
//! The first idea was to upload a heightfield and draw the sand, strata and
//! dither in a canvas shader. It would save the same uploads, but it would
//! be a second renderer of the ground, in GLSL at mobile float precision,
//! that had to agree with this one bit for bit: the dither thresholds
//! compare fractions of an `f64` crater depth, and the sky's star hash is
//! 64-bit integer arithmetic that GLSL ES 3.0 does not have. Keeping the CPU
//! the only renderer and the GPU a copier keeps exactness a matter of
//! compositing, which the tests below can check exhaustively.

use salt_line_sim::run::Screen;

use crate::biome::Biome;
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
    /// The flat frame's still sky (`keep_flat_sky`), also keyed on the
    /// canvas's own size and scale, which it is copied into whole.
    flat_sky: Kept<(SkyKey, i32, i32, i32)>,
    ground: Kept<GroundKey>,
    outline: Kept<(i32, i32, Rgba)>,
    /// The overlay pixels that put the ground's edge columns right. See
    /// `edge_patches`.
    patches: Vec<Patch>,
    /// Scratch canvas for `edge_patches`.
    narrow: Canvas,
    /// Scratch canvas for `redraw_ground_columns`.
    scratch: Canvas,
    /// A version per [`GROUND_CHUNK`] columns of the ground layer, bumped
    /// when any of its columns is drawn again, so the node uploads only
    /// the chunks a landing changed.
    ground_chunks: Vec<u64>,
    /// What `patches` were found for: the ground layer's version, the
    /// camera column and the buffer's size and scale. They depend on nothing
    /// else, so most frames (a still camera) keep them.
    patch_key: Option<(u64, i32, i32, i32, i32)>,
    /// Per column of the ground layer, its first display row with anything
    /// on it (the layer's display height where none), found when the layer
    /// is drawn. See [`Band`].
    ground_tops: Vec<i32>,
    /// What this frame shows under the overlay: `None` when the overlay is
    /// the whole frame (a menu, or `layered` off).
    shown: Option<Shown>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Backdrop {
            layered: false,
            sky: Kept::default(),
            flat_sky: Kept::default(),
            ground: Kept::default(),
            outline: Kept::default(),
            patches: Vec::new(),
            narrow: Canvas::new(2, 1),
            scratch: Canvas::new(1, 1),
            ground_chunks: Vec::new(),
            patch_key: None,
            ground_tops: Vec::new(),
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

impl<K> Kept<K> {
    /// Start the layer again if it is not at `like`'s scale: a layer is
    /// always drawn at the scale of the frame it goes under.
    fn match_scale(&mut self, like: &Canvas) {
        if self.canvas.scale() != like.scale() {
            self.canvas = like.blank_like();
            self.key = None;
        }
    }
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

/// What the sky layer was drawn from: everything [`sky::draw_still`]
/// reads. Not the biome, which only the moving part draws: a new screen of
/// another biome keeps the sky, a 4x plate the weakest phones take a dozen
/// milliseconds to redraw.
#[derive(PartialEq)]
struct SkyKey {
    phase: sky::Phase,
    w: i32,
    h: i32,
    /// Only when the sky reads it (`sky::still_reads_palette`).
    pal: Option<[Rgba; 16]>,
}

/// What the ground layer was drawn from: everything `raster::draw_terrain`
/// and `raster::draw_ground_detail_around` read. The guns go in as which of
/// the columns that would grow a tuft (`raster::tuft_columns`) they keep
/// clear, not as their positions: a versus gun rolling along eases by
/// fractions of a column every frame, but the ground only looks different
/// when a tuft appears or goes. The test is `raster::near_gun`, the one the
/// drawing uses, so the strict `<` falls the same way.
///
/// Compared every frame and refreshed in place when it differs, so a
/// change costs no allocation once the vectors have grown.
#[derive(Default)]
struct GroundKey {
    base: Vec<f64>,
    scar: Vec<f64>,
    seed: u64,
    structures: Vec<f64>,
    h: i32,
    pal: [Rgba; 16],
    /// The columns that grow a tuft when no gun is near, for the field
    /// above, and whether a gun keeps each clear.
    tufts: Vec<i32>,
    cleared: Vec<bool>,
}

/// Whether a gun in `guns` keeps world column `x` clear of grass.
fn cleared(x: i32, guns: &[f64]) -> bool {
    guns.iter().any(|&g| raster::near_gun(x, g))
}

impl GroundKey {
    fn matches(&self, screen: &Screen, guns: &[f64], h: i32, pal: &Palette) -> bool {
        // The field first: `tufts` is only right for the field it was
        // found on.
        self.seed == screen.seed
            && self.h == h
            && self.pal == pal.colours
            && self.base == screen.field.base
            && self.scar == screen.field.scar
            && self
                .structures
                .iter()
                .copied()
                .eq(screen.structures.iter().map(|s| s.centre_x()))
            && self
                .cleared
                .iter()
                .copied()
                .eq(self.tufts.iter().map(|&x| cleared(x, guns)))
    }

    /// The world columns `c0..=c1` whose ground pixels may differ between
    /// the field this key was taken from and `screen`, `guns`: columns whose
    /// height or scar changed, every column a structure that came, went or
    /// moved keeps clear of grass, and each tuft that grows or is cleared
    /// differently. `None` asks for the whole layer: the size, palette,
    /// seed or world's width changed, or no column did (a change this does
    /// not know how to place).
    fn changed_columns(
        &self,
        screen: &Screen,
        guns: &[f64],
        h: i32,
        pal: &Palette,
    ) -> Option<(i32, i32)> {
        let field = &screen.field;
        if self.seed != screen.seed
            || self.h != h
            || self.pal != pal.colours
            || self.base.len() != field.base.len()
            || self.scar.len() != field.scar.len()
        {
            return None;
        }
        let mut range: Option<(i32, i32)> = None;
        let mut add = |a: i32, b: i32| {
            range = Some(range.map_or((a, b), |(x0, x1)| (x0.min(a), x1.max(b))));
        };
        for (c, ((b0, b1), (s0, s1))) in self
            .base
            .iter()
            .zip(&field.base)
            .zip(self.scar.iter().zip(&field.scar))
            .enumerate()
        {
            if b0 != b1 || s0 != s1 {
                add(c as i32, c as i32);
            }
        }
        let now: Vec<f64> = screen.structures.iter().map(|s| s.centre_x()).collect();
        if now != self.structures {
            // `raster::grass_grows`: no grass within 10 columns of one.
            for &x in now.iter().chain(&self.structures) {
                let x = x.round() as i32;
                add(x - 11, x + 11);
            }
        }
        // A tuft drawn before and not now, or now and not before.
        let before = |x: i32| {
            self.tufts
                .iter()
                .position(|&t| t == x)
                .is_some_and(|i| !self.cleared[i])
        };
        let tufts: Vec<i32> = raster::tuft_columns(screen, h).collect();
        for &x in tufts.iter().chain(&self.tufts) {
            let after = tufts.contains(&x) && !cleared(x, guns);
            if after != before(x) {
                add(x, x);
            }
        }
        range
    }

    fn refresh(&mut self, screen: &Screen, guns: &[f64], h: i32, pal: &Palette) {
        self.base.clone_from(&screen.field.base);
        self.scar.clone_from(&screen.field.scar);
        self.seed = screen.seed;
        self.structures.clear();
        self.structures
            .extend(screen.structures.iter().map(|s| s.centre_x()));
        self.h = h;
        self.pal = pal.colours;
        self.tufts.clear();
        self.tufts.extend(raster::tuft_columns(screen, h));
        self.cleared.clear();
        self.cleared
            .extend(self.tufts.iter().map(|&x| cleared(x, guns)));
    }
}

/// A display pixel of the overlay at `(x, y)` (`Canvas::put_detail`'s
/// coordinates) that corrects the ground there: the ground's colour per
/// view, or `None` where per view there is no ground.
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
    /// The collapse outline is on. See [`Backdrop::outline`].
    pub outline: bool,
    /// The buffer's width this frame: how many ground columns show.
    pub w: i32,
    /// What the node draws of the sky and ground, column band by band.
    pub bands: [Band; BANDS],
}

/// The ground layer's columns per chunk: the node keeps one texture per
/// chunk and uploads only those drawn again ([`Backdrop::ground_chunks`]).
/// At 4x a chunk is 128 display columns, ~370 KB a 720-row layer.
pub const GROUND_CHUNK: i32 = 32;

/// How many column bands [`Shown::bands`] cuts the buffer into.
pub const BANDS: usize = 32;

/// What can show of the sky and ground in buffer columns `x0..x1`. The
/// GPU pays for every pixel it draws, and on a weak one the layers' overlap
/// is most of the frame, so the node draws only this much of each:
///
/// * the sky down to `sky_h` (buffer rows): below it every column of the
///   band has opaque ground in front (`raster::draw_terrain` paints from
///   the surface down), or the band has a column off the world and the sky
///   goes all the way down;
/// * the ground from `ground_top` (display rows of the ground layer): above
///   it the layer is transparent in every column of the band.
///
/// Neither changes a pixel; [`Backdrop::composite`] mirrors both, so the
/// tests catch a band that leaves out too much.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Band {
    pub x0: i32,
    pub x1: i32,
    pub sky_h: i32,
    pub ground_top: i32,
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

    /// For each [`GROUND_CHUNK`] columns of the ground layer, the version
    /// of [`ground`](Self::ground) that last drew any of them.
    pub fn ground_chunks(&self) -> &[u64] {
        &self.ground_chunks
    }

    /// The outline layer, and how many times it has been drawn.
    pub fn outline_layer(&self) -> (&Canvas, u64) {
        (&self.outline.canvas, self.outline.version)
    }

    /// Paint the one-pixel `colour` outline just inside the buffer's edge,
    /// as `Canvas::rect_outline(1, 1, w - 2, h - 2, colour)` would. Flat, or
    /// with no backdrop this frame, that is what it does. Layered, the
    /// outline layer is shown instead, and the overlay's pixels under the
    /// outline are cleared, as the outline would have painted over them:
    /// only what is painted after this shows over it (see the module docs).
    pub fn outline(&mut self, canvas: &mut Canvas, colour: Rgba) {
        let (w, h) = (canvas.w, canvas.h);
        let Some(shown) = self.shown.as_mut() else {
            canvas.rect_outline(1, 1, w - 2, h - 2, colour);
            return;
        };
        shown.outline = true;
        canvas.rect_outline(1, 1, w - 2, h - 2, [0; 4]);
        self.outline.match_scale(canvas);
        let key = (w, h, colour);
        if self.outline.key != Some(key) {
            let layer = &mut self.outline.canvas;
            layer.resize(w, h);
            layer.clear();
            layer.rect_outline(1, 1, w - 2, h - 2, colour);
            self.outline.key = Some(key);
            self.outline.version += 1;
        }
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
        biome: Biome,
        seed: u64,
        time: f64,
        screen: &Screen,
        guns: &[f64],
    ) {
        if !self.layered {
            // `sky::draw`, with its still part copied from the last frame's:
            // on a detailed canvas that plate is most of the frame's work.
            self.keep_flat_sky(canvas, view, pal, phase);
            canvas.copy_from(&self.flat_sky.canvas);
            sky::draw_stars(canvas, view, pal, phase, seed, time, |_, _| true);
            sky::draw_biome(canvas, view, pal, biome, seed, time, |_, _| true);
            raster::draw_terrain(canvas, view, &screen.field, pal);
            raster::draw_ground_detail_around(canvas, view, screen, guns, pal);
            return;
        }
        canvas.clear();
        // The layers are drawn at the frame's own scale, so a detailed
        // frame's layers carry the detailed art.
        self.sky.match_scale(canvas);
        self.ground.match_scale(canvas);
        if self.narrow.scale() != canvas.scale() {
            self.narrow = canvas.blank_like();
        }
        self.keep_sky(view, pal, phase);
        self.keep_ground(screen, guns, view.h, pal);
        let bands = self.bands(view, screen, canvas.scale());
        self.shown = Some(Shown {
            cam_x: view.cam_x,
            outline: false,
            w: view.w,
            bands,
        });
        let key = (
            self.ground.version,
            view.cam_x,
            view.w,
            view.h,
            canvas.scale(),
        );
        if self.patch_key != Some(key) {
            self.edge_patches(view, screen, guns, pal);
            self.patch_key = Some(key);
        }
        for p in &self.patches {
            // No ground here per view: the sky shows, so the overlay must
            // show it too, over the ground layer's stray pixel.
            let c = p
                .ground
                .unwrap_or_else(|| self.sky.canvas.get_detail(p.x, p.y));
            canvas.put_detail(p.x, p.y, c);
        }
        sky::draw_stars(canvas, view, pal, phase, seed, time, |x, y| {
            self.ground_at(x, y).is_none()
        });
        sky::draw_biome(canvas, view, pal, biome, seed, time, |x, y| {
            self.ground_at(x, y).is_none()
        });
    }

    fn keep_sky(&mut self, view: &View, pal: &Palette, phase: sky::Phase) {
        self.sky.canvas.resize(view.w, view.h);
        let key = SkyKey {
            phase,
            w: view.w,
            h: view.h,
            pal: sky::still_reads_palette(&self.sky.canvas, view).then_some(pal.colours),
        };
        if self.sky.key.as_ref() == Some(&key) {
            return;
        }
        sky::draw_still(&mut self.sky.canvas, view, pal, phase);
        self.sky.key = Some(key);
        self.sky.version += 1;
    }

    /// The flat frame's still sky, drawn at `canvas`'s size and scale; it
    /// never reaches a texture of its own.
    fn keep_flat_sky(&mut self, canvas: &Canvas, view: &View, pal: &Palette, phase: sky::Phase) {
        let key = (
            SkyKey {
                phase,
                w: view.w,
                h: view.h,
                pal: sky::still_reads_palette(canvas, view).then_some(pal.colours),
            },
            canvas.w,
            canvas.h,
            canvas.scale(),
        );
        if self.flat_sky.key.as_ref() == Some(&key) {
            return;
        }
        let mut still = canvas.blank_like();
        sky::draw_still(&mut still, view, pal, phase);
        self.flat_sky.canvas = still;
        self.flat_sky.key = Some(key);
    }

    /// This frame's [`Band`]s for `view` over `screen`, the ground layer
    /// already drawn, on a canvas of scale `s`.
    fn bands(&self, view: &View, screen: &Screen, s: i32) -> [Band; BANDS] {
        let len = screen.field.len() as i32;
        let ph = view.h * s;
        std::array::from_fn(|b| {
            let x0 = b as i32 * view.w / BANDS as i32;
            let x1 = (b as i32 + 1) * view.w / BANDS as i32;
            let mut band = Band {
                x0,
                x1,
                sky_h: 0,
                ground_top: ph,
            };
            for x in x0..x1 {
                let world_x = view.cam_x + x;
                let sky = if (0..len).contains(&world_x) {
                    view.sy(screen.field.surface(world_x as usize))
                        .clamp(0, view.h)
                } else {
                    view.h
                };
                band.sky_h = band.sky_h.max(sky);
                let column = (world_x + GROUND_PAD) as usize;
                let top = self.ground_tops.get(column).copied().unwrap_or(ph);
                band.ground_top = band.ground_top.min(top);
            }
            band
        })
    }

    fn keep_ground(&mut self, screen: &Screen, guns: &[f64], h: i32, pal: &Palette) {
        let changed = match &self.ground.key {
            Some(key) if key.matches(screen, guns, h, pal) => return,
            Some(key) => key.changed_columns(screen, guns, h, pal),
            None => None,
        };
        // Drawn with `guns` as given: the key says the grass comes out the
        // same for any gun positions that clear the same tufts.
        let world = View {
            cam_x: -GROUND_PAD,
            w: screen.field.len() as i32 + 2 * GROUND_PAD,
            h,
        };
        let whole = (self.ground.canvas.w, self.ground.canvas.h) != (world.w, h)
            || self.ground_tops.len() != world.w as usize;
        let columns = match changed.filter(|_| !whole) {
            // A shell lands: only the columns about the crater, which on a
            // slow phone is a few milliseconds of a frame against a dozen.
            Some((c0, c1)) => self.redraw_ground_columns(&world, c0, c1, screen, guns, pal),
            None => {
                let canvas = &mut self.ground.canvas;
                canvas.resize(world.w, h);
                canvas.clear();
                raster::draw_terrain(canvas, &world, &screen.field, pal);
                raster::draw_ground_detail_around(canvas, &world, screen, guns, pal);
                self.ground_tops.resize(world.w as usize, 0);
                0..world.w
            }
        };
        let chunks = (world.w + GROUND_CHUNK - 1) / GROUND_CHUNK;
        if self.ground_chunks.len() != chunks as usize {
            self.ground_chunks = vec![0; chunks as usize];
        }
        let next = self.ground.version + 1;
        let touched = columns.start / GROUND_CHUNK..(columns.end + GROUND_CHUNK - 1) / GROUND_CHUNK;
        for v in &mut self.ground_chunks[touched.start as usize..touched.end as usize] {
            *v = next;
        }
        // Each layer column's first painted display row, for the bands.
        let canvas = &self.ground.canvas;
        let (s, pw, ph) = (canvas.scale(), canvas.pixel_w() as usize, canvas.pixel_h());
        let px = canvas.bytes().as_chunks::<4>().0;
        for c in columns {
            self.ground_tops[c as usize] = (0..ph)
                .find(|&y| {
                    let row = y as usize * pw;
                    (c * s..(c + 1) * s).any(|x| px[row + x as usize][3] != 0)
                })
                .unwrap_or(ph);
        }
        self.ground
            .key
            .get_or_insert_with(GroundKey::default)
            .refresh(screen, guns, h, pal);
        self.ground.version += 1;
    }

    /// Draw again the ground layer's columns about world columns `c0..=c1`:
    /// those, their neighbours (a crater's rim reads the columns either
    /// side) and the columns a tuft spills into, drawn as a narrow view
    /// with a margin, so every column copied back had all its neighbours
    /// drawn, and copied over the layer. Returns the layer columns replaced.
    fn redraw_ground_columns(
        &mut self,
        world: &View,
        c0: i32,
        c1: i32,
        screen: &Screen,
        guns: &[f64],
        pal: &Palette,
    ) -> std::ops::Range<i32> {
        const REACH: i32 = 3;
        const MARGIN: i32 = 2;
        let l0 = (c0 - REACH + GROUND_PAD).max(0);
        let l1 = (c1 + REACH + GROUND_PAD + 1).min(world.w);
        if l1 <= l0 {
            return 0..0;
        }
        let view = View {
            cam_x: l0 - GROUND_PAD - MARGIN,
            w: l1 - l0 + 2 * MARGIN,
            h: world.h,
        };
        if self.scratch.scale() != self.ground.canvas.scale() {
            self.scratch = self.ground.canvas.blank_like();
        }
        self.scratch.resize(view.w, view.h);
        self.scratch.clear();
        raster::draw_terrain(&mut self.scratch, &view, &screen.field, pal);
        raster::draw_ground_detail_around(&mut self.scratch, &view, screen, guns, pal);
        let s = self.scratch.scale();
        self.ground
            .canvas
            .copy_columns(&self.scratch, MARGIN * s, l0 * s, (l1 - l0) * s);
        l0..l1
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
            // Display pixel by display pixel: a detailed column is `s`
            // pixels wide, and each must agree.
            let s = self.narrow.scale();
            let layer_x = (x + view.cam_x + GROUND_PAD) * s;
            for py in 0..h * s {
                for dx in 0..s {
                    let per_view = opaque(self.narrow.get_detail(col * s + dx, py));
                    let layer = opaque(ground.get_detail(layer_x + dx, py));
                    if per_view != layer {
                        self.patches.push(Patch {
                            x: x * s + dx,
                            y: py,
                            ground: per_view,
                        });
                    }
                }
            }
        }
    }

    /// The ground's colour at display pixel `(px, py)` as this frame shows
    /// it, patches included; `None` where the sky shows.
    fn ground_at_detail(&self, px: i32, py: i32) -> Option<Rgba> {
        if let Some(p) = self.patches.iter().find(|p| p.x == px && p.y == py) {
            return p.ground;
        }
        let s = self.ground.canvas.scale();
        let left = self.shown.map_or(0, Shown::ground_x) * s;
        opaque(self.ground.canvas.get_detail(px + left, py))
    }

    /// Whether ground shows at buffer pixel `(x, y)`, sampled where
    /// `Canvas::get` samples. Every ground pixel is painted a whole buffer
    /// pixel at a time (`raster::draw_terrain`), so one sample answers for
    /// all of its display pixels.
    fn ground_at(&self, x: i32, y: i32) -> Option<Rgba> {
        let s = self.ground.canvas.scale();
        self.ground_at_detail(x * s + s / 2, y * s + s / 2)
    }

    /// The colour the layers beneath the overlay show at display pixel
    /// `(px, py)` (`Canvas::put_detail`'s coordinates): the ground's, or
    /// else the sky's. For a translucent write over a pixel the overlay has
    /// not painted ([`Canvas::put_blend_on`]). Flat, there is nothing
    /// beneath: the canvas holds every colour itself.
    pub fn under(&self, px: i32, py: i32) -> Rgba {
        let Some(shown) = self.shown else {
            return [0; 4];
        };
        if shown.outline {
            if let Some(c) = opaque(self.outline.canvas.get_detail(px, py)) {
                return c;
            }
        }
        self.ground_at_detail(px, py)
            .unwrap_or_else(|| self.sky.canvas.get_detail(px, py))
    }

    /// The finished frame from `overlay`, this frame's canvas: the layers put
    /// together on the CPU exactly as the node draws them on the GPU, sky,
    /// then the ground's [`window`], then the outline if on, then the
    /// overlay, each opaque pixel hiding what is under it. For captures (`--salt-shot`) and tests; the
    /// flat frame is `overlay` itself.
    pub fn composite(&self, overlay: &Canvas) -> Vec<u8> {
        let Some(shown) = self.shown else {
            return overlay.bytes().to_vec();
        };
        let s = overlay.scale();
        let (w, h) = (overlay.pixel_w(), overlay.pixel_h());
        let mut out = self.sky.canvas.bytes().to_vec();
        let ground = &self.ground.canvas;
        let stride = w as usize * 4;
        // As the node draws: no sky below each band's `sky_h`. A colour no
        // art uses stands in for whatever the GPU's target held, so a pixel
        // the ground failed to cover shows in the comparison.
        for band in &shown.bands {
            for y in band.sky_h * s..h {
                let row = y as usize * stride;
                out[row + (band.x0 * s) as usize * 4..row + (band.x1 * s) as usize * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .fill(UNDRAWN);
            }
        }
        debug_assert_eq!(shown.w, overlay.w, "the overlay is the buffer's size");
        debug_assert_eq!(
            out.len(),
            overlay.bytes().len(),
            "the sky is the overlay's size"
        );
        if let Some(win) = window(shown.ground_x(), shown.w, ground.w) {
            for band in &shown.bands {
                // The band's part of the window, in buffer columns.
                let (a, b) = (band.x0.max(win.dst_x), band.x1.min(win.dst_x + win.w));
                for y in band.ground_top..h.min(ground.pixel_h()) {
                    for i in a * s..b * s {
                        let c = ground.get_detail(win.src_x * s + i - win.dst_x * s, y);
                        if c[3] != 0 {
                            let at = y as usize * stride + i as usize * 4;
                            out[at..at + 4].copy_from_slice(&c);
                        }
                    }
                }
            }
        }
        if shown.outline {
            let layer = self.outline.canvas.bytes().as_chunks::<4>().0;
            for (dst, src) in out.as_chunks_mut::<4>().0.iter_mut().zip(layer) {
                if src[3] != 0 {
                    dst.copy_from_slice(src);
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

/// What [`Backdrop::composite`] puts where no layer is drawn.
const UNDRAWN: Rgba = [255, 0, 255, 255];

/// `c` if it is painted, `None` if transparent.
fn opaque(c: Rgba) -> Option<Rgba> {
    (c[3] != 0).then_some(c)
}

#[cfg(test)]
mod tests;

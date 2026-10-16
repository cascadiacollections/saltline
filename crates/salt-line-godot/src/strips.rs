//! Engine-free bookkeeping for uploading only the parts of the frame that
//! changed.
//!
//! The frame is cut into [`STRIPS`] horizontal strips, each its own texture,
//! drawn over one texture of the whole frame. A [`Shadow`] keeps a copy of
//! what is on screen, so after the raster pass each strip is compared with it
//! and only a strip that differs is uploaded again. When more than
//! [`WHOLE_ABOVE`] strips differ (a camera pan), the whole-frame texture is
//! updated instead and the strips are hidden: see [`Layers`].
//!
//! Why: on a phone the upload itself is the expensive part of a frame, not
//! the raster. On a Pixel 10, `ImageTexture::update` of the whole 375x180
//! buffer cost ~1.4 ms against ~0.6 ms for drawing it. That is CPU time inside
//! the call (Godot's and the driver's; a ring of textures did not shorten it,
//! so it is not a wait on the GPU), and most of it scales with the bytes
//! sent. In play most
//! frames change only a few strips (a shell in flight, the HUD), and about a
//! third change nothing at all.
//!
//! Each upload also has a fixed cost (~0.18 ms on the Pixel), so twelve strip
//! uploads cost more than one whole-frame upload. Hence the whole-frame
//! texture underneath: the worst frame costs one whole upload, as before, plus
//! the comparison with the shadow (tens of microseconds).
//!
//! The strips sit at whole-pixel offsets in the internal-resolution buffer,
//! and the viewport stretch scales the finished viewport, so they meet with
//! no seam.
//!
//! Every row is drawn from exactly one texture: a shown strip's rows from the
//! strip, the rest from the whole-frame texture ([`Layers::whole_rows`]). The
//! frame can be transparent in places (the layered frame, `backdrop`), and a
//! transparent pixel of a strip drawn over the whole-frame texture would let
//! that texture's older pixel show through.

/// How many strips the frame is cut into. 12 strips of a 180-row buffer are
/// 15 rows each: small enough that a shell in flight dirties one or two. A
/// change across most of the frame (a camera pan) takes the whole-frame path
/// instead, see [`WHOLE_ABOVE`]. A buffer must be at least `STRIPS` rows
/// tall, so no strip is empty.
pub const STRIPS: usize = 12;

/// Above this many changed strips, one whole-frame upload is cheaper than the
/// strip uploads. On the Pixel a strip upload is ~0.18 ms of fixed cost plus
/// ~0.1 ms of copying, and a whole upload ~1.4 ms: they break even at about
/// five strips.
///
/// Measured, not just estimated: a 2026-10 sweep on the Pixel 10 (profiling
/// build, three interleaved rounds per value) gave a mean process time of
/// 1.94 ms at 0 (always whole), 1.53 at 3, 1.48 at 4, 1.51 at 5, 1.52 at 6
/// and 1.72 at 12 (never whole, with p95 rising from ~3.4 to 4.3 ms). 3 to 6
/// are within noise of each other. Re-measure on a phone before changing it;
/// a weaker GPU or driver may move the break-even.
///
/// A weaker one did, and it did not matter: on a Lenovo TB336FU (MT8755,
/// Mali-G57 MC2) a strip upload costs ~0.3 ms and a whole one ~2.3 ms, so
/// they break even near eight strips; yet a sweep there (2026-10, the
/// layered 4x frame, three interleaved rounds) gave a mean process time of
/// 2.84 ms at 4, 2.74 at 6, 2.83 at 8 and 2.80 at 12, within its ~0.2 ms
/// run-to-run spread. The layered overlay rarely changes more than four
/// strips at once, so the threshold seldom comes into play.
///
/// Power did not separate them: a Perfetto power-rail capture of 0, 4 and 12
/// (three interleaved rounds) showed CPU, GPU and memory rails within a few mW
/// of each other (~100 mW per rail) and no consistent direction, while
/// whole-device totals swung by hundreds of mW between runs of the same value.
/// So this threshold is a frame-time choice, not a battery one.
pub const WHOLE_ABOVE: usize = 4;

/// The rows `[start, end)` of strip `i` in a buffer `h` rows tall. Strips
/// cover every row exactly once, in order; when `h` does not divide evenly
/// some are a row taller.
pub fn rows(h: i32, i: usize) -> (i32, i32) {
    let n = STRIPS as i32;
    let i = i as i32;
    (i * h / n, (i + 1) * h / n)
}

/// A copy of the frame as last uploaded, to tell which strips changed.
#[derive(Default)]
pub struct Shadow {
    bytes: Vec<u8>,
    w: i32,
    h: i32,
    /// Each strip's [`tiles`](Self::tiles) as last uploaded.
    tiles: [u32; STRIPS],
}

/// How many column tiles [`Shadow::tiles`] cuts a strip into.
pub const TILES: usize = 32;

/// The columns `x0..x1` of tile `t` in a frame `w` pixels wide. Tiles cover
/// every column exactly once, in order.
pub fn tile_columns(w: i32, t: usize) -> (i32, i32) {
    let (n, t) = (TILES as i32, t as i32);
    (t * w / n, (t + 1) * w / n)
}

/// Which column tiles of `rows` (RGBA8, `w` pixels a row) hold a pixel that
/// is not transparent, bit `t` for tile `t`.
fn opaque_tiles(rows: &[u8], w: i32) -> u32 {
    let mut mask = 0u32;
    for row in rows.chunks_exact(w as usize * 4) {
        let px = row.as_chunks::<4>().0;
        for t in 0..TILES {
            if mask & (1 << t) != 0 {
                continue;
            }
            let (x0, x1) = tile_columns(w, t);
            if px[x0 as usize..x1 as usize].iter().any(|p| p[3] != 0) {
                mask |= 1 << t;
            }
        }
        if mask == u32::MAX {
            break;
        }
    }
    mask
}

/// The runs of set bits in `mask`, as tile ranges `t0..t1`.
pub fn tile_runs(mask: u32) -> impl Iterator<Item = (usize, usize)> {
    let mut t = 0;
    std::iter::from_fn(move || {
        while t < TILES && mask & (1 << t) == 0 {
            t += 1;
        }
        let start = t;
        while t < TILES && mask & (1 << t) != 0 {
            t += 1;
        }
        (start < TILES).then_some((start, t))
    })
}

impl Shadow {
    /// Whether the shadow holds a frame of this size.
    pub fn fits(&self, w: i32, h: i32) -> bool {
        self.w == w && self.h == h && !self.bytes.is_empty()
    }

    /// Start over from `bytes`, a whole frame of `w` by `h` RGBA8 pixels,
    /// all of it now on the GPU.
    pub fn reset(&mut self, bytes: &[u8], w: i32, h: i32) {
        self.bytes.clear();
        self.bytes.extend_from_slice(bytes);
        self.w = w;
        self.h = h;
        let stride = w as usize * 4;
        self.tiles = std::array::from_fn(|i| {
            let (y0, y1) = rows(h, i);
            opaque_tiles(&bytes[y0 as usize * stride..y1 as usize * stride], w)
        });
    }

    /// Which column tiles of strip `i` ([`tile_columns`]), as last
    /// uploaded, hold any pixel not transparent, bit `t` for tile `t`. Over
    /// the layered frame's backdrop (`backdrop`) only these need drawing: a
    /// transparent pixel drawn changes nothing beneath it.
    pub fn tiles(&self, i: usize) -> u32 {
        self.tiles[i]
    }

    /// Whether strip `i`, as last uploaded, is wholly transparent.
    pub fn empty(&self, i: usize) -> bool {
        self.tiles[i] == 0
    }

    /// [`tiles`](Self::tiles) for every strip.
    pub fn all_tiles(&self) -> [u32; STRIPS] {
        self.tiles
    }

    /// Whether strip `i` of `frame` differs from what was last uploaded. If
    /// it does, the shadow takes the new rows, as the caller is about to
    /// upload them. `frame` must be the size the shadow [`fits`](Self::fits).
    pub fn take_if_changed(&mut self, frame: &[u8], i: usize) -> bool {
        let stride = self.w as usize * 4;
        let (y0, y1) = rows(self.h, i);
        let range = y0 as usize * stride..y1 as usize * stride;
        if frame[range.clone()] == self.bytes[range.clone()] {
            return false;
        }
        self.bytes[range.clone()].copy_from_slice(&frame[range.clone()]);
        self.tiles[i] = opaque_tiles(&frame[range], self.w);
        true
    }
}

/// Which textures hold what is on screen: the whole-frame texture, with
/// some strips drawn over it.
///
/// The rule that keeps the screen right: a strip is shown exactly when its
/// rows changed after the last whole-frame upload. A hidden strip's rows are
/// therefore still what the whole-frame texture holds, and a shown strip
/// holds its own latest rows.
#[derive(Default)]
pub struct Layers {
    shown: [bool; STRIPS],
}

/// What to upload this frame, from [`Layers::plan`].
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Nothing changed.
    Nothing,
    /// Update the whole-frame texture; every strip is now hidden.
    Whole,
    /// Update these strips; each is now shown.
    Strips([bool; STRIPS]),
}

impl Layers {
    /// Whether strip `i` is drawn over the whole-frame texture.
    pub fn shown(&self, i: usize) -> bool {
        self.shown[i]
    }

    /// The rows `[start, end)` to draw from the whole-frame texture, in runs:
    /// those of every strip not shown. With [`shown`](Self::shown) strips
    /// drawn from their own textures, every row of a frame `h` tall is drawn
    /// once.
    pub fn whole_rows(&self, h: i32) -> Vec<(i32, i32)> {
        let mut runs: Vec<(i32, i32)> = Vec::new();
        for i in (0..STRIPS).filter(|&i| !self.shown[i]) {
            let (y0, y1) = rows(h, i);
            match runs.last_mut() {
                Some(last) if last.1 == y0 => last.1 = y1,
                _ => runs.push((y0, y1)),
            }
        }
        runs
    }

    /// Start over: the whole-frame texture holds the frame, no strip shown.
    pub fn reset(&mut self) {
        self.shown = [false; STRIPS];
    }

    /// Decide this frame's uploads from which strips changed, and note what
    /// will then be shown. The second value is whether the set of shown
    /// strips changed, so the node must draw again.
    pub fn plan(&mut self, changed: [bool; STRIPS]) -> (Plan, bool) {
        let n = changed.iter().filter(|&&c| c).count();
        let before = self.shown;
        let plan = if n == 0 {
            Plan::Nothing
        } else if n > WHOLE_ABOVE {
            self.shown = [false; STRIPS];
            Plan::Whole
        } else {
            for (shown, &c) in self.shown.iter_mut().zip(&changed) {
                *shown |= c;
            }
            Plan::Strips(changed)
        };
        (plan, self.shown != before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_cover_every_row_once_in_order() {
        for h in [1, 7, 12, 179, 180, 181, 360] {
            let mut next = 0;
            for i in 0..STRIPS {
                let (a, b) = rows(h, i);
                assert_eq!(a, next, "h={h} strip {i} starts where the last ended");
                assert!(b >= a);
                next = b;
            }
            assert_eq!(next, h, "h={h}: the last strip ends at the bottom");
        }
    }

    #[test]
    fn a_180_row_frame_is_cut_into_15_row_strips() {
        assert!((0..STRIPS).all(|i| {
            let (a, b) = rows(180, i);
            b - a == 15
        }));
    }

    /// Model the screen with a version number per strip and check, over
    /// many random frames, that what is drawn always matches the frame.
    #[test]
    fn the_screen_always_shows_the_latest_frame() {
        let mut layers = Layers::default();
        let mut frame = [0u32; STRIPS]; // the version of each strip's rows
        let mut whole = frame; // what the whole-frame texture holds
        let mut strip = frame; // what each strip texture holds
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut wholes = 0;
        for step in 0..5000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            // Mostly a few strips, sometimes all of them (a pan).
            let pan = seed.is_multiple_of(10);
            let mut changed = [false; STRIPS];
            for (i, c) in changed.iter_mut().enumerate() {
                *c = pan || (seed >> (i + 8)).is_multiple_of(7);
                if *c {
                    frame[i] += 1;
                }
            }
            match layers.plan(changed).0 {
                Plan::Nothing => {}
                Plan::Whole => {
                    whole = frame;
                    wholes += 1;
                }
                Plan::Strips(up) => {
                    for i in (0..STRIPS).filter(|&i| up[i]) {
                        strip[i] = frame[i];
                    }
                }
            }
            for i in 0..STRIPS {
                let drawn = if layers.shown(i) { strip[i] } else { whole[i] };
                assert_eq!(drawn, frame[i], "step {step}: strip {i} is stale");
            }
            // And each row comes from one texture only: the whole frame's
            // under a hidden strip, the strip's own under a shown one.
            let h = 180;
            let mut from = vec![0; h as usize];
            for (y0, y1) in layers.whole_rows(h) {
                (y0..y1).for_each(|y| from[y as usize] += 1);
            }
            for i in (0..STRIPS).filter(|&i| layers.shown(i)) {
                let (y0, y1) = rows(h, i);
                (y0..y1).for_each(|y| from[y as usize] += 1);
            }
            assert!(from.iter().all(|&n| n == 1), "step {step}: {from:?}");
        }
        assert!(wholes > 0, "the test must exercise whole-frame uploads");
    }

    #[test]
    fn the_whole_frame_is_drawn_in_runs_around_shown_strips() {
        let mut layers = Layers::default();
        assert_eq!(layers.whole_rows(180), [(0, 180)]);
        let mut some = [false; STRIPS];
        some[0] = true;
        some[5] = true;
        layers.plan(some);
        assert_eq!(layers.whole_rows(180), [(15, 75), (90, 180)]);
        layers.plan([true; STRIPS]);
        assert_eq!(
            layers.whole_rows(180),
            [(0, 180)],
            "a whole upload hides them"
        );
    }

    #[test]
    fn the_shadow_knows_which_strips_are_wholly_transparent() {
        let (w, h) = (4, 24);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        let mut shadow = Shadow::default();
        shadow.reset(&frame, w, h);
        assert_eq!(shadow.all_tiles(), [0; STRIPS]);
        // One opaque pixel in strip 5's last row.
        let (_, y1) = rows(h, 5);
        let at = (((y1 - 1) * w + 3) * 4) as usize;
        frame[at..at + 4].copy_from_slice(&[9, 9, 9, 255]);
        assert!(shadow.take_if_changed(&frame, 5));
        assert!(!shadow.empty(5));
        // Four columns over 32 tiles: column 3 is the last tile's.
        assert_eq!(shadow.tiles(5), 1 << 31);
        assert!((0..STRIPS).filter(|&i| i != 5).all(|i| shadow.empty(i)));
        // Unchanged strips keep their answer; cleared again, empty again.
        frame[at + 3] = 0;
        assert!(shadow.take_if_changed(&frame, 5));
        assert!(shadow.empty(5));
        frame[at + 3] = 255;
        shadow.reset(&frame, w, h);
        assert!(!shadow.empty(5) && shadow.empty(4));
    }

    #[test]
    fn tiles_cover_every_column_once_and_mark_where_pixels_are() {
        for w in [32, 100, 1280, 1500] {
            let mut next = 0;
            for t in 0..TILES {
                let (a, b) = tile_columns(w, t);
                assert_eq!(a, next, "w={w} tile {t}");
                assert!(b > a, "w={w}: no tile is empty");
                next = b;
            }
            assert_eq!(next, w);
        }
        let (w, h) = (64, 12);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        // Strip 2 is row 2: pixels in tiles 0, 1 and 31 (2 columns a tile).
        for x in [0, 3, 63] {
            frame[((2 * w + x) * 4 + 3) as usize] = 255;
        }
        let mut shadow = Shadow::default();
        shadow.reset(&frame, w, h);
        assert_eq!(shadow.tiles(2), 0b11 | 1 << 31);
        assert_eq!(
            tile_runs(shadow.tiles(2)).collect::<Vec<_>>(),
            [(0, 2), (31, 32)]
        );
        assert_eq!(tile_runs(0).count(), 0);
        assert_eq!(tile_runs(u32::MAX).collect::<Vec<_>>(), [(0, TILES)]);
    }

    #[test]
    fn a_pan_uploads_once_and_hides_the_strips() {
        let mut layers = Layers::default();
        let mut one = [false; STRIPS];
        one[3] = true;
        assert_eq!(layers.plan(one), (Plan::Strips(one), true));
        assert_eq!(
            layers.plan(one),
            (Plan::Strips(one), false),
            "already shown"
        );
        assert_eq!(layers.plan([false; STRIPS]), (Plan::Nothing, false));
        assert_eq!(layers.plan([true; STRIPS]), (Plan::Whole, true));
        assert!(!layers.shown(3));
        assert_eq!(layers.plan([true; STRIPS]), (Plan::Whole, false));
    }

    fn frame(w: i32, h: i32, fill: u8) -> Vec<u8> {
        vec![fill; (w * h * 4) as usize]
    }

    #[test]
    fn only_the_strip_with_a_changed_pixel_is_dirty_and_only_once() {
        let (w, h) = (8, 24);
        let mut shadow = Shadow::default();
        assert!(!shadow.fits(w, h));
        let mut f = frame(w, h, 0);
        shadow.reset(&f, w, h);
        assert!(shadow.fits(w, h));
        assert!(!shadow.fits(w + 1, h), "a resize no longer fits");

        // One pixel on row 9: 24 rows / 12 strips = 2 rows each, so strip 4.
        f[(9 * w as usize + 3) * 4] = 255;
        let dirty: Vec<usize> = (0..STRIPS)
            .filter(|&i| shadow.take_if_changed(&f, i))
            .collect();
        assert_eq!(dirty, [4]);
        // Taken: the same frame again changes nothing.
        assert!((0..STRIPS).all(|i| !shadow.take_if_changed(&f, i)));
    }
}

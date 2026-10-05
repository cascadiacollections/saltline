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
        self.bytes[range.clone()].copy_from_slice(&frame[range]);
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
        }
        assert!(wholes > 0, "the test must exercise whole-frame uploads");
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

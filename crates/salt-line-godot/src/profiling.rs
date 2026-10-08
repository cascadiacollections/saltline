//! Opt-in A/B timing capture for an optimized Android profiling APK.
//!
//! Enable with `--features profiling`. The build starts a deterministic
//! autopilot run, then alternates normal texture updates with a frozen texture
//! while leaving simulation and software rasterization enabled in both modes.
//! Timings are CPU-side; in particular, `ImageTexture::update` does not
//! include deferred GPU completion.

use godot::prelude::*;
use std::time::{Duration, Instant};

/// Ignore startup and renderer warmup before collecting samples.
const WARMUP_SECONDS: f64 = 2.0;
/// Each A/B phase is long enough for hundreds of frames at 60 Hz.
const PHASE_SECONDS: f64 = 8.0;
/// A-B-B-A helps reduce bias from gradual thermal or clock changes.
const PHASES: [Mode; 4] = [Mode::Updated, Mode::Frozen, Mode::Frozen, Mode::Updated];

/// Stable seed for comparing profiling APKs built from different commits.
pub const FIXED_SEED: u64 = 20261004;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Updated,
    Frozen,
}

impl Mode {
    fn index(self) -> usize {
        match self {
            Self::Updated => 0,
            Self::Frozen => 1,
        }
    }
}

#[derive(Default)]
struct Samples {
    values_ns: Vec<u64>,
}

impl Samples {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            values_ns: Vec::with_capacity(capacity),
        }
    }

    fn observe(&mut self, duration: Duration) {
        self.values_ns
            .push(duration.as_nanos().min(u64::MAX as u128) as u64);
    }

    fn summary(&mut self) -> String {
        if self.values_ns.is_empty() {
            return "n=0".into();
        }
        self.values_ns.sort_unstable();
        let n = self.values_ns.len();
        let percentile = |p: usize| self.values_ns[((n * p).div_ceil(100)).saturating_sub(1)];
        let mean =
            self.values_ns.iter().map(|&v| v as u128).sum::<u128>() as f64 / n as f64 / 1_000.0;
        format!(
            "n={n} mean={mean:.1}us p50={:.1}us p95={:.1}us max={:.1}us",
            percentile(50) as f64 / 1_000.0,
            percentile(95) as f64 / 1_000.0,
            self.values_ns[n - 1] as f64 / 1_000.0,
        )
    }
}

pub struct Capture {
    started: Option<Instant>,
    /// Whether the run is in play this frame; frames outside play (a game
    /// over inside the capture window) are not counted.
    playing: bool,
    /// The phase this frame belongs to, fixed by `begin_frame` so a frame
    /// that straddles a phase boundary is counted wholly in one phase.
    frame_mode: Option<Mode>,
    finished: bool,
    frames: [u64; 2],
    rendered: [u64; 2],
    texture_updates: [u64; 2],
    texture_update_skips: [u64; 2],
    /// Strips that changed, and whole-frame uploads, summed over frames.
    strips_changed: [u64; 2],
    wholes: [u64; 2],
    /// Sky and ground layer uploads (`backdrop`), summed over frames.
    backdrop_uploads: [u64; 2],
    process: [Samples; 2],
    tick: [Samples; 2],
    render: [Samples; 2],
    texture_update: [Samples; 2],
}

impl Capture {
    pub fn new() -> Self {
        let capacity = (PHASE_SECONDS * 60.0 * 2.0).ceil() as usize;
        let pair = || {
            [
                Samples::with_capacity(capacity),
                Samples::with_capacity(capacity),
            ]
        };
        Self {
            started: None,
            playing: false,
            frame_mode: None,
            finished: false,
            frames: [0; 2],
            rendered: [0; 2],
            texture_updates: [0; 2],
            texture_update_skips: [0; 2],
            strips_changed: [0; 2],
            wholes: [0; 2],
            backdrop_uploads: [0; 2],
            process: pair(),
            tick: pair(),
            render: pair(),
            texture_update: pair(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_mode = self.mode();
    }

    pub fn start_if_playing(&mut self, playing: bool) {
        self.playing = playing;
        if playing && self.started.is_none() && !self.finished {
            self.started = Some(Instant::now());
            godot_print!(
                "salt-profile: A/B capture starting; warmup={WARMUP_SECONDS:.0}s phases={PHASES:?} phase_seconds={PHASE_SECONDS:.0} seed={FIXED_SEED}"
            );
        }
    }

    /// Keep startup and finished runs in normal update mode; freeze only in
    /// the two measured frozen phases.
    pub fn should_update_texture(&self) -> bool {
        self.frame_mode != Some(Mode::Frozen)
    }

    fn mode(&self) -> Option<Mode> {
        if self.finished || !self.playing {
            return None;
        }
        let elapsed = self.started?.elapsed().as_secs_f64() - WARMUP_SECONDS;
        if elapsed < 0.0 {
            return None;
        }
        let phase = (elapsed / PHASE_SECONDS) as usize;
        PHASES.get(phase).copied()
    }

    pub fn record_tick(&mut self, duration: Duration) {
        if let Some(mode) = self.frame_mode {
            self.tick[mode.index()].observe(duration);
        }
    }

    pub fn record_render(&mut self, duration: Duration) {
        if let Some(mode) = self.frame_mode {
            let i = mode.index();
            self.rendered[i] += 1;
            self.render[i].observe(duration);
        }
    }

    /// The strip pass of one frame: comparing each strip with the screen,
    /// `changed` of them differing, and uploading them, or the whole frame
    /// (`whole` is 1) when too many did; and `backdrop` sky or ground layer
    /// uploads.
    pub fn record_texture_update(
        &mut self,
        duration: Duration,
        changed: u64,
        whole: u64,
        backdrop: u64,
    ) {
        if let Some(mode) = self.frame_mode {
            let i = mode.index();
            self.texture_updates[i] += 1;
            self.strips_changed[i] += changed;
            self.wholes[i] += whole;
            self.backdrop_uploads[i] += backdrop;
            self.texture_update[i].observe(duration);
        }
    }

    pub fn record_texture_update_skipped(&mut self) {
        if let Some(mode) = self.frame_mode {
            self.texture_update_skips[mode.index()] += 1;
        }
    }

    pub fn record_process(&mut self, duration: Duration) {
        if let Some(mode) = self.frame_mode {
            let i = mode.index();
            self.frames[i] += 1;
            self.process[i].observe(duration);
        }
    }

    pub fn finish_if_due(&mut self) -> Option<Vec<String>> {
        let elapsed = self.started?.elapsed();
        let capture_seconds =
            Duration::from_secs_f64(WARMUP_SECONDS + PHASE_SECONDS * PHASES.len() as f64);
        if self.finished || elapsed < capture_seconds {
            return None;
        }
        self.finished = true;
        Some(vec![
            format!(
                "salt-profile: done wall={:.2}s seed={FIXED_SEED} warmup={WARMUP_SECONDS:.0}s phase_order=updated,frozen,frozen,updated phase_seconds={PHASE_SECONDS:.0}",
                elapsed.as_secs_f64()
            ),
            format!(
                "updated: frames={} rendered={} texture_updates={} texture_update_skips={}",
                self.frames[0], self.rendered[0], self.texture_updates[0], self.texture_update_skips[0]
            ),
            format!("updated process: {}", self.process[0].summary()),
            format!("updated core_tick: {}", self.tick[0].summary()),
            format!("updated software_render: {}", self.render[0].summary()),
            format!("updated strip_pass: {} strips_changed_per_pass={:.2} whole_uploads={} backdrop_uploads={} (compare + update of changed strips and layers; CPU call time, GPU completion is deferred)", self.texture_update[0].summary(), self.strips_changed[0] as f64 / self.texture_updates[0].max(1) as f64, self.wholes[0], self.backdrop_uploads[0]),
            format!(
                "frozen: frames={} rendered={} texture_updates={} texture_update_skips={}",
                self.frames[1], self.rendered[1], self.texture_updates[1], self.texture_update_skips[1]
            ),
            format!("frozen process: {}", self.process[1].summary()),
            format!("frozen core_tick: {}", self.tick[1].summary()),
            format!("frozen software_render: {}", self.render[1].summary()),
            format!("frozen strip_pass: {} strips_changed_per_pass={:.2} whole_uploads={} (should be n=0)", self.texture_update[1].summary(), self.strips_changed[1] as f64 / self.texture_updates[1].max(1) as f64, self.wholes[1]),
        ])
    }
}

/// How many drawn frames the GPU check compares after the capture.
const GPU_CHECK_FRAMES: u64 = 600;

/// After the timing capture, check the GPU's side of the layered frame
/// (`backdrop`) on the device itself: each drawn frame is read back from
/// the viewport and compared, pixel for pixel, with the same frame drawn
/// flat on the CPU. Readback stalls the GPU, which is why it runs only
/// once the timings are taken.
#[derive(Default)]
pub struct GpuCheck {
    /// Drawn frames still to compare; 0 when not running.
    left: u64,
    /// The flat frame the viewport should show after this frame is drawn,
    /// its size, and whether it was drawn in layers.
    expected: Option<(Vec<u8>, i32, i32, bool)>,
    frames: u64,
    layered: u64,
    bad_frames: u64,
    bad_pixels: u64,
    sizes: Option<(i32, i32, i32, i32)>,
}

impl GpuCheck {
    pub fn start(&mut self) {
        self.left = GPU_CHECK_FRAMES;
    }

    /// Whether this frame's flat reference is wanted.
    pub fn running(&self) -> bool {
        self.left > 0
    }

    /// The frame just drawn should look like `flat`, `w` by `h`.
    pub fn expect(&mut self, flat: &[u8], w: i32, h: i32, layered: bool) {
        if self.running() {
            self.expected = Some((flat.to_vec(), w, h, layered));
        }
    }

    /// Whether a frame is waiting to be compared with the viewport.
    pub fn pending(&self) -> bool {
        self.expected.is_some()
    }

    /// Compare the last drawn frame with `image`, the viewport read back,
    /// whose buffer starts `left` pixels in (the letterbox). Returns the
    /// report when the last frame has been compared.
    pub fn compare(&mut self, image: Gd<godot::classes::Image>, left: i32) -> Option<String> {
        let (want, w, h, layered) = self.expected.take()?;
        let mut image = image;
        image.convert(godot::classes::image::Format::RGBA8);
        let (iw, ih) = (image.get_width(), image.get_height());
        self.sizes = Some((w, h, iw, ih));
        let got = image.get_data();
        let got = got.as_slice();
        let mut bad = 0u64;
        if ih != h || iw < left + w {
            bad = (w * h) as u64;
        } else {
            for y in 0..h {
                for x in 0..w {
                    let a = ((y * iw + x + left) * 4) as usize;
                    let b = ((y * w + x) * 4) as usize;
                    if got[a..a + 3] != want[b..b + 3] {
                        bad += 1;
                    }
                }
            }
        }
        self.frames += 1;
        self.layered += u64::from(layered);
        self.bad_frames += u64::from(bad > 0);
        self.bad_pixels += bad;
        self.left -= 1;
        (self.left == 0).then(|| {
            format!(
                "salt-profile: gpu check frames={} layered={} mismatched_frames={} mismatched_pixels={} buffer_and_viewport={:?}",
                self.frames, self.layered, self.bad_frames, self.bad_pixels, self.sizes
            )
        })
    }
}

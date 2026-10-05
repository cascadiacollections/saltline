//! Opt-in A/B timing capture for an optimized Android profiling APK.
//!
//! Enable with `--features profiling`. The build starts a deterministic
//! autopilot run, then alternates normal texture updates with a frozen texture
//! while leaving simulation, software rasterization, upload preparation, and
//! draw submission enabled in both modes. Timings are CPU-side; in particular,
//! `ImageTexture::update` does not include deferred GPU completion.

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
    finished: bool,
    frames: [u64; 2],
    rendered: [u64; 2],
    texture_updates: [u64; 2],
    texture_update_skips: [u64; 2],
    process: [Samples; 2],
    tick: [Samples; 2],
    render: [Samples; 2],
    upload_prepare: [Samples; 2],
    texture_update: [Samples; 2],
    draw_submit: [Samples; 2],
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
            finished: false,
            frames: [0; 2],
            rendered: [0; 2],
            texture_updates: [0; 2],
            texture_update_skips: [0; 2],
            process: pair(),
            tick: pair(),
            render: pair(),
            upload_prepare: pair(),
            texture_update: pair(),
            draw_submit: pair(),
        }
    }

    pub fn start_if_playing(&mut self, playing: bool) {
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
        self.mode() != Some(Mode::Frozen)
    }

    fn mode(&self) -> Option<Mode> {
        if self.finished {
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
        if let Some(mode) = self.mode() {
            self.tick[mode.index()].observe(duration);
        }
    }

    pub fn record_render(&mut self, duration: Duration) {
        if let Some(mode) = self.mode() {
            let i = mode.index();
            self.rendered[i] += 1;
            self.render[i].observe(duration);
        }
    }

    pub fn record_upload_prepare(&mut self, duration: Duration) {
        if let Some(mode) = self.mode() {
            self.upload_prepare[mode.index()].observe(duration);
        }
    }

    pub fn record_texture_update(&mut self, duration: Duration) {
        if let Some(mode) = self.mode() {
            let i = mode.index();
            self.texture_updates[i] += 1;
            self.texture_update[i].observe(duration);
        }
    }

    pub fn record_texture_update_skipped(&mut self) {
        if let Some(mode) = self.mode() {
            self.texture_update_skips[mode.index()] += 1;
        }
    }

    pub fn record_draw_submit(&mut self, duration: Duration) {
        if let Some(mode) = self.mode() {
            self.draw_submit[mode.index()].observe(duration);
        }
    }

    pub fn record_process(&mut self, duration: Duration) {
        if let Some(mode) = self.mode() {
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
            format!("updated upload_prepare: {}", self.upload_prepare[0].summary()),
            format!("updated texture_update_call: {} (CPU call time; GPU completion is deferred)", self.texture_update[0].summary()),
            format!("updated draw_texture_submit: {} (CPU call time; GPU draw completion is deferred)", self.draw_submit[0].summary()),
            format!(
                "frozen: frames={} rendered={} texture_updates={} texture_update_skips={}",
                self.frames[1], self.rendered[1], self.texture_updates[1], self.texture_update_skips[1]
            ),
            format!("frozen process: {}", self.process[1].summary()),
            format!("frozen core_tick: {}", self.tick[1].summary()),
            format!("frozen software_render: {}", self.render[1].summary()),
            format!("frozen upload_prepare: {}", self.upload_prepare[1].summary()),
            format!("frozen texture_update_call: {} (should be n=0)", self.texture_update[1].summary()),
            format!("frozen draw_texture_submit: {} (CPU call time; GPU draw completion is deferred)", self.draw_submit[1].summary()),
        ])
    }
}

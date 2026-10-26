//! Frame timing on a device: the debug overlay and the soak benchmark.
//!
//! Two consumers of the same numbers, both aimed at the low-end profiling
//! floor (iPhone SE 2nd gen on iOS, Galaxy A16 on Android; see
//! `docs/profiling-ios.md`):
//!
//! * [`Hud`], a debug-only overlay: frame time (mean, p95, p99 over the last
//!   [`WINDOW`] frames), draw calls, static and video memory, and the
//!   device's thermal state and memory headroom where the platform says. It
//!   exists only in builds with debug assertions (the `*.debug` libraries in
//!   `salt_line.gdextension`), so a release build carries none of it. On a
//!   phone it shows from launch; on a desktop it follows the F1 debug view.
//! * [`Bench`], `--salt-bench`: a fixed-seed autopilot run
//!   ([`BENCH_SEED`]) for [`BENCH_SECONDS`] that records every frame and
//!   writes the rows as CSV under `user://bench/` when it ends, then quits.
//!   It works in any build, so a release export can be soaked.
//!
//! The statistics and the CSV are engine-free and tested here; the engine
//! reads (Godot's `Performance` monitors, the overlay's `Label`) are in
//! [`Hud`] and [`Sample::read`].
//!
//! [`device`] is the platform shim: on iOS (and macOS, for thermal state) it
//! asks `NSProcessInfo` and `os_proc_available_memory` directly through the
//! Objective-C runtime, so there is no plugin to build or export.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Frames the overlay's statistics cover: four seconds at 60 Hz.
pub const WINDOW: usize = 240;

/// Seconds between overlay refreshes. Updating a `Label` every frame would
/// cost a text shape a frame in the frame being measured.
#[cfg(debug_assertions)]
const HUD_REFRESH: f64 = 0.5;

/// The benchmark's seed: the profiling build's (`profiling::FIXED_SEED`),
/// so a benchmark and a profiling APK play the same run.
pub const BENCH_SEED: u64 = 20261004;

/// How long `--salt-bench` runs unless `--salt-bench-seconds` says.
pub const BENCH_SECONDS: f64 = 300.0;

/// Where the benchmark writes, one file per run.
pub const BENCH_DIR: &str = "user://bench";

/// The last [`WINDOW`] frame times, in milliseconds.
#[derive(Default)]
pub struct FrameTimes {
    ms: VecDeque<f64>,
    sorted: Vec<f64>,
}

/// Mean and tail frame times, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl FrameTimes {
    pub fn push(&mut self, ms: f64) {
        if self.ms.len() == WINDOW {
            self.ms.pop_front();
        }
        self.ms.push_back(ms);
    }

    /// `None` until a frame has been timed.
    pub fn summary(&mut self) -> Option<Summary> {
        summarize(self.ms.iter().copied(), &mut self.sorted)
    }
}

/// Nearest-rank percentiles, as `profiling::Samples` takes them, so the
/// overlay, the benchmark and the profiling APK agree on what p95 means.
fn summarize(values: impl Iterator<Item = f64>, sorted: &mut Vec<f64>) -> Option<Summary> {
    sorted.clear();
    sorted.extend(values.filter(|v| v.is_finite()));
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable_by(f64::total_cmp);
    let n = sorted.len();
    let rank = |p: usize| sorted[(n * p).div_ceil(100).saturating_sub(1)];
    Some(Summary {
        n,
        mean: sorted.iter().sum::<f64>() / n as f64,
        p95: rank(95),
        p99: rank(99),
        max: sorted[n - 1],
    })
}

/// `NSProcessInfoThermalState`, in Apple's numbering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Thermal {
    Nominal = 0,
    Fair = 1,
    Serious = 2,
    Critical = 3,
}

impl Thermal {
    pub fn from_raw(raw: isize) -> Option<Thermal> {
        Some(match raw {
            0 => Thermal::Nominal,
            1 => Thermal::Fair,
            2 => Thermal::Serious,
            3 => Thermal::Critical,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Thermal::Nominal => "nominal",
            Thermal::Fair => "fair",
            Thermal::Serious => "serious",
            Thermal::Critical => "critical",
        }
    }
}

/// What the platform says about heat and memory, where it says anything.
pub mod device {
    use super::Thermal;

    /// The thermal state, on Apple platforms; `None` elsewhere.
    pub fn thermal() -> Option<Thermal> {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            apple::thermal_state().and_then(Thermal::from_raw)
        }
        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            None
        }
    }

    /// Bytes this process may still allocate before iOS ends it
    /// (`os_proc_available_memory`, the jetsam headroom). `None` off iOS,
    /// and where the call answers 0 (it does when the process has no
    /// limit, as in the simulator).
    pub fn available_memory() -> Option<u64> {
        #[cfg(target_os = "ios")]
        {
            apple::available_memory()
        }
        #[cfg(not(target_os = "ios"))]
        {
            None
        }
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    mod apple {
        //! `[[NSProcessInfo processInfo] thermalState]` through the
        //! Objective-C runtime's C API, which Godot's binary already links.
        //! Both selectors exist since iOS 11 and macOS 10.10.3, and neither
        //! returns an owned object, so nothing here retains or releases.

        use std::ffi::{c_char, c_void};

        type Id = *mut c_void;
        type Sel = *mut c_void;

        #[link(name = "objc")]
        extern "C" {
            fn objc_getClass(name: *const c_char) -> Id;
            fn sel_registerName(name: *const c_char) -> Sel;
            fn objc_msgSend();
        }

        #[link(name = "Foundation", kind = "framework")]
        extern "C" {}

        #[cfg(target_os = "ios")]
        extern "C" {
            fn os_proc_available_memory() -> usize;
        }

        pub fn thermal_state() -> Option<isize> {
            // SAFETY: objc_msgSend is called through the signature of each
            // method sent (arm64 requires the cast); `processInfo` returns
            // the shared instance and `thermalState` an NSInteger.
            unsafe {
                let class = objc_getClass(c"NSProcessInfo".as_ptr());
                if class.is_null() {
                    return None;
                }
                let send_id: unsafe extern "C" fn(Id, Sel) -> Id =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let info = send_id(class, sel_registerName(c"processInfo".as_ptr()));
                if info.is_null() {
                    return None;
                }
                let send_int: unsafe extern "C" fn(Id, Sel) -> isize =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                Some(send_int(info, sel_registerName(c"thermalState".as_ptr())))
            }
        }

        #[cfg(target_os = "ios")]
        pub fn available_memory() -> Option<u64> {
            // SAFETY: a plain query with no arguments (iOS 13+).
            let bytes = unsafe { os_proc_available_memory() };
            (bytes > 0).then_some(bytes as u64)
        }
    }
}

/// One frame's measurements.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Wall clock since the previous frame began.
    pub frame_ms: f64,
    /// The node's own `process`, input to upload.
    pub process_ms: f64,
    /// Whether the frame was rasterised and uploaded (`frame::Redraw`).
    pub rendered: bool,
    pub draw_calls: u32,
    pub static_bytes: u64,
    pub video_bytes: u64,
    pub thermal: Option<Thermal>,
    pub available_bytes: Option<u64>,
}

impl Sample {
    /// The engine's side of a frame: its monitors, and the device shim's
    /// answers (`device` is read by the caller at its own rate).
    pub fn read(frame_ms: f64, process_ms: f64, rendered: bool) -> Sample {
        use godot::classes::performance::Monitor;
        use godot::classes::Performance;
        use godot::obj::Singleton as _;
        let perf = Performance::singleton();
        let get = |m: Monitor| perf.get_monitor(m).max(0.0);
        Sample {
            frame_ms,
            process_ms,
            rendered,
            draw_calls: get(Monitor::RENDER_TOTAL_DRAW_CALLS_IN_FRAME) as u32,
            static_bytes: get(Monitor::MEMORY_STATIC) as u64,
            video_bytes: get(Monitor::RENDER_VIDEO_MEM_USED) as u64,
            thermal: None,
            available_bytes: None,
        }
    }
}

/// The platform's answers, read at most once a second: they are system
/// calls, and neither changes faster than that.
#[derive(Default)]
pub struct DeviceReads {
    last: Option<Instant>,
    thermal: Option<Thermal>,
    available: Option<u64>,
}

impl DeviceReads {
    pub fn fill(&mut self, s: &mut Sample) {
        if self
            .last
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.last = Some(Instant::now());
            self.thermal = device::thermal();
            self.available = device::available_memory();
        }
        s.thermal = self.thermal;
        s.available_bytes = self.available;
    }
}

const MIB: f64 = 1024.0 * 1024.0;

/// The overlay's text for `summary` and the latest `sample`.
pub fn hud_text(summary: Option<Summary>, s: &Sample) -> String {
    let mut out = String::new();
    match summary {
        Some(f) => {
            let _ = writeln!(
                out,
                "{:.1} FPS  {:.2} ms  p95 {:.2}  p99 {:.2}",
                1000.0 / f.mean.max(0.001),
                f.mean,
                f.p95,
                f.p99
            );
        }
        None => out.push_str("-- ms\n"),
    }
    let _ = write!(
        out,
        "DRAW {}  STATIC {:.1}M  VRAM {:.1}M",
        s.draw_calls,
        s.static_bytes as f64 / MIB,
        s.video_bytes as f64 / MIB
    );
    if let Some(t) = s.thermal {
        let _ = write!(out, "\nTHERMAL {}", t.name().to_uppercase());
    }
    if let Some(b) = s.available_bytes {
        let _ = write!(out, "  HEADROOM {:.0}M", b as f64 / MIB);
    }
    out
}

/// The debug overlay: a `Label` on a `CanvasLayer` of its own over the
/// game, so it is never part of the frame the game uploads.
#[cfg(debug_assertions)]
pub struct Hud {
    label: godot::obj::Gd<godot::classes::Label>,
    times: FrameTimes,
    refresh: f64,
}

#[cfg(debug_assertions)]
impl Hud {
    /// The overlay and the layer it is drawn on, for the caller to add to
    /// the tree (through `base_mut`: the node is bound while it does).
    pub fn new() -> (Hud, godot::obj::Gd<godot::classes::CanvasLayer>) {
        use godot::classes::{CanvasLayer, Label};
        use godot::prelude::*;
        let mut layer = CanvasLayer::new_alloc();
        layer.set_layer(128);
        let mut label = Label::new_alloc();
        // The logical viewport is 180 tall; canvas_items stretch draws the
        // text at the window's resolution, so 4 is legible on a 4.7" phone
        // (about 17 physical pixels on the SE). Below the windsock plate,
        // clear of the HUD in every mode.
        label.add_theme_font_size_override("font_size", 4);
        label.add_theme_constant_override("line_spacing", -1);
        label.add_theme_constant_override("outline_size", 1);
        label.add_theme_color_override("font_outline_color", Color::BLACK);
        label.set_position(Vector2::new(3.0, 24.0));
        layer.add_child(&label);
        let hud = Hud {
            label,
            times: FrameTimes::default(),
            refresh: 0.0,
        };
        (hud, layer)
    }

    pub fn update(&mut self, visible: bool, s: &Sample, delta: f64) {
        self.times.push(s.frame_ms);
        self.label.set_visible(visible);
        self.refresh -= delta;
        if visible && self.refresh <= 0.0 {
            self.refresh = HUD_REFRESH;
            self.label.set_text(&hud_text(self.times.summary(), s));
        }
    }
}

/// `--salt-bench`: every frame of a fixed-seed autopilot run, kept in memory
/// (five minutes at 60 Hz is about a megabyte) and written once at the end,
/// so file IO never lands in a measured frame.
pub struct Bench {
    seconds: f64,
    started: Instant,
    rows: Vec<(f64, Sample)>,
    done: bool,
}

impl Bench {
    pub fn new(seconds: f64) -> Bench {
        Bench {
            seconds,
            started: Instant::now(),
            rows: Vec::with_capacity((seconds * 61.0) as usize),
            done: false,
        }
    }

    pub fn record(&mut self, s: Sample) {
        if !self.done {
            let t = self.started.elapsed().as_secs_f64();
            self.rows.push((t, s));
        }
    }

    /// Whether the run has lasted its time; true once.
    pub fn finish_if_due(&mut self) -> bool {
        if self.done || self.started.elapsed().as_secs_f64() < self.seconds {
            return false;
        }
        self.done = true;
        true
    }

    pub fn csv(&self) -> String {
        csv(&self.rows)
    }

    pub fn summary_line(&self) -> String {
        summary_line(&self.rows, self.seconds)
    }
}

/// The CSV header; one row a frame follows.
pub const CSV_HEADER: &str =
    "frame,t_s,frame_ms,process_ms,rendered,draw_calls,static_mib,video_mib,thermal,available_mib";

fn csv(rows: &[(f64, Sample)]) -> String {
    let mut out = String::with_capacity(64 * (rows.len() + 1));
    out.push_str(CSV_HEADER);
    out.push('\n');
    for (i, (t, s)) in rows.iter().enumerate() {
        let _ = writeln!(
            out,
            "{i},{t:.4},{:.3},{:.3},{},{},{:.2},{:.2},{},{}",
            s.frame_ms,
            s.process_ms,
            u8::from(s.rendered),
            s.draw_calls,
            s.static_bytes as f64 / MIB,
            s.video_bytes as f64 / MIB,
            s.thermal.map_or("", Thermal::name),
            s.available_bytes
                .map_or(String::new(), |b| format!("{:.1}", b as f64 / MIB)),
        );
    }
    out
}

fn summary_line(rows: &[(f64, Sample)], seconds: f64) -> String {
    let mut sorted = Vec::new();
    // The first frames carry loading and shader warm-up; leave out the
    // first two seconds, as the profiling APK does.
    let steady = rows
        .iter()
        .filter(|(t, _)| *t >= 2.0)
        .map(|(_, s)| s.frame_ms);
    let f = summarize(steady, &mut sorted);
    let worst = rows
        .iter()
        .filter_map(|(_, s)| s.thermal)
        .max_by_key(|t| *t as u8);
    let min_headroom = rows.iter().filter_map(|(_, s)| s.available_bytes).min();
    format!(
        "salt-bench: seed={BENCH_SEED} seconds={seconds:.0} frames={} {} worst_thermal={} min_headroom_mib={}",
        rows.len(),
        f.map_or("frame_ms n=0".into(), |f| format!(
            "frame_ms n={} mean={:.2} p95={:.2} p99={:.2} max={:.2}",
            f.n, f.mean, f.p95, f.p99, f.max
        )),
        worst.map_or("n/a", Thermal::name),
        min_headroom.map_or("n/a".into(), |b| format!("{:.0}", b as f64 / MIB)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn percentiles_are_nearest_rank_over_the_window() {
        let mut t = FrameTimes::default();
        assert_eq!(t.summary(), None);
        for ms in 1..=100 {
            t.push(ms as f64);
        }
        let s = t.summary().unwrap();
        assert_eq!((s.n, s.p95, s.p99, s.max), (100, 95.0, 99.0, 100.0));
        assert_eq!(s.mean, 50.5);
    }

    #[test]
    fn the_window_forgets_the_oldest_frames() {
        let mut t = FrameTimes::default();
        t.push(1000.0);
        for _ in 0..WINDOW {
            t.push(16.0);
        }
        let s = t.summary().unwrap();
        assert_eq!((s.n, s.max), (WINDOW, 16.0));
    }

    #[test]
    fn a_non_finite_time_is_left_out() {
        let mut t = FrameTimes::default();
        t.push(f64::NAN);
        t.push(8.0);
        assert_eq!(t.summary().unwrap().n, 1);
    }

    #[test]
    fn thermal_states_follow_apples_numbering() {
        assert_eq!(Thermal::from_raw(0), Some(Thermal::Nominal));
        assert_eq!(Thermal::from_raw(3), Some(Thermal::Critical));
        assert_eq!(Thermal::from_raw(4), None);
        assert_eq!(Thermal::from_raw(-1), None);
    }

    #[test]
    fn the_hud_shows_what_the_platform_reports_and_nothing_it_does_not() {
        let s = Sample {
            draw_calls: 5,
            static_bytes: 32 * 1024 * 1024,
            video_bytes: 7 * 1024 * 1024,
            ..Sample::default()
        };
        let mut t = FrameTimes::default();
        t.push(16.0);
        let text = hud_text(t.summary(), &s);
        assert_eq!(
            text,
            "62.5 FPS  16.00 ms  p95 16.00  p99 16.00\nDRAW 5  STATIC 32.0M  VRAM 7.0M"
        );
        let ios = Sample {
            thermal: Some(Thermal::Serious),
            available_bytes: Some(900 * 1024 * 1024),
            ..s
        };
        assert!(hud_text(None, &ios).ends_with("\nTHERMAL SERIOUS  HEADROOM 900M"));
    }

    #[test]
    fn the_csv_has_a_row_a_frame_and_blanks_for_what_was_not_reported() {
        let rows = [
            (
                0.0,
                Sample {
                    frame_ms: 16.6667,
                    rendered: true,
                    draw_calls: 3,
                    ..Sample::default()
                },
            ),
            (
                0.0167,
                Sample {
                    frame_ms: 17.0,
                    thermal: Some(Thermal::Fair),
                    available_bytes: Some(1536 * 1024 * 1024),
                    ..Sample::default()
                },
            ),
        ];
        assert_eq!(
            csv(&rows),
            format!(
                "{CSV_HEADER}\n0,0.0000,16.667,0.000,1,3,0.00,0.00,,\n1,0.0167,17.000,0.000,0,0,0.00,0.00,fair,1536.0\n"
            )
        );
    }

    #[test]
    fn the_summary_skips_warm_up_and_reports_the_worst_heat() {
        let mut rows = vec![(
            0.5,
            Sample {
                frame_ms: 500.0,
                ..Sample::default()
            },
        )];
        for i in 0..100 {
            rows.push((
                2.0 + i as f64 / 60.0,
                Sample {
                    frame_ms: 16.0,
                    thermal: Some(if i == 50 {
                        Thermal::Serious
                    } else {
                        Thermal::Nominal
                    }),
                    available_bytes: Some((1000 - i) * 1024 * 1024),
                    ..Sample::default()
                },
            ));
        }
        let line = summary_line(&rows, 300.0);
        assert!(
            line.contains("frames=101 frame_ms n=100 mean=16.00"),
            "{line}"
        );
        assert!(
            line.ends_with("worst_thermal=serious min_headroom_mib=901"),
            "{line}"
        );
    }

    #[test]
    fn the_benchmark_ends_once() {
        let mut b = Bench::new(0.0);
        assert!(b.finish_if_due());
        assert!(!b.finish_if_due());
        b.record(Sample::default());
        assert!(b.rows.is_empty(), "nothing is recorded after the end");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_shim_answers_on_a_mac() {
        // The same Objective-C path the iOS build takes.
        assert!(device::thermal().is_some());
        assert_eq!(device::available_memory(), None);
    }
}

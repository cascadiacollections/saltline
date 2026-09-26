//! Crash reports without telemetry.
//!
//! The game has no network and phones home to nobody. When the Rust side
//! panics, gdext catches it and logs to a console the player never sees (a
//! Windows GUI build has none; a phone has logcat), and the game would carry
//! on with half-updated state. Instead a panic hook writes `user://crash.txt`
//! and raises a flag; the node sees the flag and stops the game on an error
//! screen with a button that puts the report on the clipboard. What leaves the
//! device is whatever the player chooses to paste into a bug report.
//!
//! Engine-free: the hook cannot reach the node, so the node keeps a small
//! global context up to date, and everything here is plain `std`.

use std::fmt::Write as _;
use std::fs;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};

use salt_line_sim::run::{format_seed, LoggedShot, RunState};

/// The report file, beside the save.
pub const REPORT_PATH: &str = "user://crash.txt";
/// Where a report goes once the player has copied or dismissed it.
pub const OLD_REPORT_PATH: &str = "user://crash-old.txt";
/// Where bug reports go. Printed in the report so a pasted one says how to
/// send it on.
const ISSUES: &str = "https://github.com/cascadiacollections/saltline/issues";

/// What the game was doing. Updated by the node every frame; the hook reads
/// it. The shot log is appended to, not copied, so a long run costs nothing
/// per frame but a compare of its last entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// `None` on the menus.
    pub run: Option<RunContext>,
    /// The UI mode, as the node names it.
    pub mode: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunContext {
    pub seed: u64,
    pub width: usize,
    pub screen_index: u32,
    /// The aim on the dial. A panic inside `fire` happens before the shot is
    /// in anything the node has seen, so this is how the fatal shot is known.
    pub angle_ticks: i32,
    pub power_ticks: i32,
    /// DESIGN §3: the seed plus this rebuilds the run.
    pub log: Vec<LoggedShot>,
}

static CONTEXT: Mutex<Context> = Mutex::new(Context {
    run: None,
    mode: "",
});
static PANICKED: AtomicBool = AtomicBool::new(false);
static REPORT: Mutex<Option<String>> = Mutex::new(None);
static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Bring the shared context up to date. Cheap enough for every frame: one
/// uncontended lock, and new shots appended rather than the log copied.
pub fn note(mode: &'static str, run: Option<&RunState>, aim: (i32, i32)) {
    let mut ctx = CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    ctx.mode = mode;
    let Some(run) = run else {
        ctx.run = None;
        return;
    };
    let rc = ctx.run.get_or_insert_with(RunContext::default);
    // Same run if the seed and width match and the log still ends where it
    // did last frame; checking the last entry, not the whole prefix, keeps
    // this constant-time.
    let have = rc.log.len();
    let same_run = rc.seed == run.run_seed
        && rc.width == run.width
        && have <= run.log.len()
        && rc.log.last() == have.checked_sub(1).and_then(|i| run.log.get(i));
    if !same_run {
        rc.log.clear();
    }
    rc.seed = run.run_seed;
    rc.width = run.width;
    rc.screen_index = run.screen_index;
    (rc.angle_ticks, rc.power_ticks) = aim;
    let have = rc.log.len();
    rc.log.extend_from_slice(&run.log[have..]);
}

/// Everything a report says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report<'a> {
    pub version: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub message: &'a str,
    /// `file:line:column` of the panic.
    pub location: Option<String>,
    /// `None` if the context could not be read (the panic happened while it
    /// was being written).
    pub context: Option<&'a Context>,
}

/// The report as text. Plain ASCII lines, so it survives any clipboard, issue
/// form or chat window it gets pasted into.
pub fn format_report(r: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "SALT LINE CRASH REPORT");
    let _ = writeln!(s, "Paste this into a bug report: {ISSUES}");
    let _ = writeln!(s, "Nothing was sent anywhere.");
    let _ = writeln!(s);
    let _ = writeln!(s, "version: {}", r.version);
    let _ = writeln!(s, "platform: {} {}", r.os, r.arch);
    let _ = writeln!(s, "panic: {}", r.message.replace('\n', "\n  "));
    let _ = writeln!(s, "at: {}", r.location.as_deref().unwrap_or("unknown"));
    let Some(ctx) = r.context else {
        let _ = writeln!(s, "context: unavailable");
        return s;
    };
    let _ = writeln!(
        s,
        "mode: {}",
        if ctx.mode.is_empty() { "?" } else { ctx.mode }
    );
    let Some(run) = &ctx.run else {
        let _ = writeln!(s, "run: none");
        return s;
    };
    let _ = writeln!(s, "seed: {}", format_seed(run.seed));
    let _ = writeln!(s, "width: {}", run.width);
    // As the player counts it, then as `--salt-screen` and the log take it.
    let _ = writeln!(
        s,
        "screen: {} (index {})",
        run.screen_index + 1,
        run.screen_index
    );
    let _ = writeln!(
        s,
        "aim: angle {} power {}",
        run.angle_ticks, run.power_ticks
    );
    // The log exactly as the sim keeps it, screen_index:angle:power per
    // entry (draft picks included, under the sim's own marker), so it can be
    // fed back to a replay unchanged. Eight to a line keeps a long run
    // readable in an issue.
    let _ = writeln!(s, "log: {}", run.log.len());
    for chunk in run.log.chunks(8) {
        let line: Vec<String> = chunk
            .iter()
            .map(|l| format!("{}:{}:{}", l.screen_index, l.angle_ticks, l.power_ticks))
            .collect();
        let _ = writeln!(s, "  {}", line.join(" "));
    }
    s
}

fn payload_message(info: &PanicHookInfo) -> String {
    let p = info.payload();
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "(no message)".to_string()
    }
}

/// Install the hook, keeping whatever hook was there (gdext's, which prints
/// to the Godot console) running first. `report_file` is `user://crash.txt` on
/// disk, or `None` where there is no disk path, in which case the report is
/// kept in memory for the error screen only.
pub fn install(report_file: Option<PathBuf>) {
    if let Some(p) = report_file {
        let _ = FILE.set(p);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        // Only the first panic is the interesting one; anything after it is
        // likely fallout, and must not overwrite the report.
        if PANICKED.swap(true, Ordering::SeqCst) {
            return;
        }
        let message = payload_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        // `try_lock`: a panic while the context is being written would
        // deadlock on a plain lock.
        let guard = match CONTEXT.try_lock() {
            Ok(g) => Some(g),
            Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        let text = format_report(&Report {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            message: &message,
            location,
            context: guard.as_deref(),
        });
        drop(guard);
        if let Some(path) = FILE.get() {
            let _ = write_report(path, &text);
        }
        if let Ok(mut r) = REPORT.try_lock() {
            *r = Some(text);
        }
    }));
}

fn write_report(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Set the report file if `install` could not. Harmless if already set.
pub fn set_report_file(path: PathBuf) {
    let _ = FILE.set(path);
}

/// Whether a panic has happened this session.
pub fn panicked() -> bool {
    PANICKED.load(Ordering::SeqCst)
}

/// This session's report, for the error screen.
pub fn report() -> Option<String> {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn shot(screen_index: u32, angle_ticks: i32, power_ticks: i32) -> LoggedShot {
        LoggedShot {
            screen_index,
            angle_ticks,
            power_ticks,
        }
    }

    fn report(context: Option<&Context>) -> String {
        format_report(&Report {
            version: "0.1.0",
            os: "linux",
            arch: "x86_64",
            message: "attempt to subtract with overflow",
            location: Some("crates/salt-line-sim/src/run.rs:940:9".into()),
            context,
        })
    }

    #[test]
    fn the_report_format_is_pinned() {
        let mut log: Vec<LoggedShot> = (0..9).map(|i| shot(i / 3, 900 + i as i32, 600)).collect();
        log.push(shot(3, -1, 1));
        let ctx = Context {
            mode: "Playing",
            run: Some(RunContext {
                seed: 20260907,
                width: 480,
                screen_index: 3,
                angle_ticks: 875,
                power_ticks: 640,
                log,
            }),
        };
        insta::assert_snapshot!(report(Some(&ctx)));
    }

    #[test]
    fn a_panic_on_the_menus_has_no_run() {
        let ctx = Context {
            mode: "Title",
            run: None,
        };
        let text = report(Some(&ctx));
        assert!(text.ends_with("mode: Title\nrun: none\n"), "{text}");
        assert!(report(None).ends_with("context: unavailable\n"));
    }

    #[test]
    fn the_report_is_ascii_the_font_and_any_clipboard_can_carry() {
        let text = report(None);
        assert!(text.is_ascii());
    }

    #[test]
    fn note_appends_to_the_log_and_restarts_it_for_a_new_run() {
        let mut run = RunState::new(7, 320);
        note("Playing", Some(&run), (900, 600));
        run.log.push(shot(0, 900, 600));
        run.log.push(shot(0, 905, 600));
        note("Playing", Some(&run), (905, 600));
        let got = CONTEXT.lock().unwrap().run.clone().unwrap();
        assert_eq!(got.log, run.log);
        assert_eq!((got.angle_ticks, got.power_ticks), (905, 600));

        let fresh = RunState::new(8, 320);
        note("Playing", Some(&fresh), (900, 600));
        let got = CONTEXT.lock().unwrap().run.clone().unwrap();
        assert_eq!(got.seed, 8);
        assert!(got.log.is_empty());

        note("Title", None, (0, 0));
        assert_eq!(CONTEXT.lock().unwrap().run, None);
    }
}

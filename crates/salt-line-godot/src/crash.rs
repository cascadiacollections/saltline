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
//!
//! # How the pieces fit
//!
//! - **Hook chaining.** [`install`] takes the hook already installed (gdext's,
//!   which prints the panic to the Godot console) and installs a closure
//!   that calls it first and then `on_panic`, which writes the report.
//!   Nothing the engine already did is lost; the report is added on top.
//!   `INSTALLED` makes a second call a no-op, so the hook is never chained
//!   to itself.
//! - **The context.** Every frame, before the frame's input is acted on,
//!   the node calls [`note`] with the mode, the run and the aim. The hook
//!   reads that under `CONTEXT.try_lock()`: a panic raised while `note`
//!   holds the lock would deadlock on a plain `lock()` (the same thread,
//!   waiting for itself), so the report says `context: unavailable` instead.
//! - **Two flags.** `PANICKED` is set first thing in the hook, so the node
//!   stops the game on its very next check ([`panicked`]). `READY` is set
//!   last, once the report is written and held, so the error screen's copy
//!   button ([`report`]) never hands over half a report. The gap between
//!   the two is only visible after a panic off the main thread, where the
//!   node can reach the error screen while the hook is still writing; a copy
//!   then says the report is not ready.
//! - **Privacy.** The report holds the version, platform, panic message and
//!   location, mode, seed, aim and shot log: what is needed to replay the
//!   crash (DESIGN §3) and nothing about the player. [`tidy_path`] strips
//!   the build machine's directories, and so its user name, from the panic
//!   location. The message is copied as it is, so a panic that quotes a
//!   path carries it. The report leaves the device only if the player
//!   pastes it.
//! - **Nothing written after a panic.** The report is the last file the
//!   game's code writes this session. `save::store` refuses, and the node
//!   drops every file effect (`app::Effect::survives_panic`): state that was half-updated
//!   when the panic unwound is not trusted to replace the last good save or
//!   run file, and the report must stay where the next launch looks for it.
//! - **Editor and hot reload.** `lib.rs` installs the hook only outside the
//!   editor: there a panic is the developer's to read in the output panel,
//!   and the editor hot-reloads this library. On unload, [`uninstall`] takes
//!   the hook out, since it is code in the library being unloaded.

use std::fmt::Write as _;
use std::fs;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};

use salt_line_sim::replay::encode_log;
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
    /// The run was rebuilt from `user://run.txt` after the app was killed,
    /// rather than played from its first shot in this session.
    pub resumed: bool,
}

static CONTEXT: Mutex<Context> = Mutex::new(Context {
    run: None,
    mode: "",
});
/// A panic has happened. Set first, so the node stops at once.
static PANICKED: AtomicBool = AtomicBool::new(false);
/// The report is finished: written to disk (or tried) and held in `REPORT`.
/// Set last. A panic off the main thread can leave the node looking at a
/// set `PANICKED` while the hook is still writing; copying before this is
/// set would hand the player half a report.
static READY: AtomicBool = AtomicBool::new(false);
/// Whether our hook is the installed one. Makes `install` idempotent: a hot
/// reload can run the init stages again.
static INSTALLED: AtomicBool = AtomicBool::new(false);
static REPORT: Mutex<Option<String>> = Mutex::new(None);
static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Bring the shared context up to date. Cheap enough for every frame: one
/// uncontended lock, and new shots appended rather than the log copied.
pub fn note(mode: &'static str, run: Option<&RunState>, aim: (i32, i32), resumed: bool) {
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
    rc.resumed = resumed;
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
    let at = r.location.as_deref().map_or("unknown".into(), tidy_path);
    let _ = writeln!(s, "at: {at}");
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
    if run.resumed {
        let _ = writeln!(s, "resumed: yes");
    }
    // The log in the replay text form (`replay::encode_log`, fuse and
    // offset included), so it can be fed back to `decode_log` and a replay
    // unchanged: line breaks and indentation are only whitespace to it. Ten
    // tokens to a line keeps a long run readable in an issue.
    let _ = writeln!(s, "log: {}", run.log.len());
    let text = encode_log(&run.log);
    let tokens: Vec<&str> = text.split(' ').collect();
    for chunk in tokens.chunks(10) {
        let _ = writeln!(s, "  {}", chunk.join(" "));
    }
    s
}

/// A panic location with the build machine taken out of it. Our own crates
/// already report `crates/...`, relative to the workspace, but a dependency's
/// or the standard library's path is absolute: `~/.cargo/registry/...` carries
/// the developer's user name, which has no business in a pasted report. Keep
/// from the crate directory (the component before the last `src`) onward, or
/// just the file name if there is no `src`.
///
/// Separators become `/` first. A Windows build reports
/// `crates\salt-line-godot\src\game.rs`; the same bug should read the same
/// in a report whichever platform it came from.
pub fn tidy_path(loc: &str) -> String {
    let loc = loc.replace('\\', "/");
    let absolute = loc.starts_with('/') || loc.as_bytes().get(1) == Some(&b':');
    if !absolute {
        return loc;
    }
    let from = match loc.rfind("/src/") {
        Some(i) => loc[..i].rfind('/').map_or(0, |j| j + 1),
        None => loc.rfind('/').map_or(0, |j| j + 1),
    };
    loc[from..].to_string()
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
/// kept in memory for the error screen only. A second call does nothing.
pub fn install(report_file: Option<PathBuf>) {
    if let Some(p) = report_file {
        let _ = FILE.set(p);
    }
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        on_panic(info);
    }));
}

/// Take our hook out again, for an extension unload (hot reload). The hook
/// is code in this library; left installed, it would outlive it. The hook it
/// chained to is gdext's, also in this library, so it goes too: the default
/// hook stands until the reloaded library installs both again.
pub fn uninstall() {
    if INSTALLED.swap(false, Ordering::SeqCst) {
        drop(std::panic::take_hook());
    }
}

fn on_panic(info: &PanicHookInfo) {
    // Only the first panic is the interesting one; anything after it is
    // likely fallout, and must not overwrite the report.
    if PANICKED.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = payload_message(info);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    // `try_lock`: a panic while the context is being written would deadlock
    // on a plain lock.
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
    // A plain lock: nothing holds `REPORT` for longer than a clone.
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
    READY.store(true, Ordering::SeqCst);
}

/// Straight over the file, not by way of a temporary one as `durable` does:
/// a panic hook should do as little as it can, and a torn report is still
/// worth more than none. Flushed (`sync_all`) because the process may not
/// live much longer.
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

/// Whether this session's report is finished. Until it is, the error screen
/// has nothing whole to copy.
pub fn report_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// This session's report, for the error screen. `None` until it is ready.
pub fn report() -> Option<String> {
    if !report_ready() {
        return None;
    }
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
            fuse_tenths: if angle_ticks < 0 { 0 } else { 120 },
            emplacement_offset: 0,
        }
    }

    fn sample(context: Option<&Context>) -> String {
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
                resumed: true,
            }),
        };
        insta::assert_snapshot!(sample(Some(&ctx)));
    }

    #[test]
    fn a_panic_on_the_menus_has_no_run() {
        let ctx = Context {
            mode: "Title",
            run: None,
        };
        let text = sample(Some(&ctx));
        assert!(text.ends_with("mode: Title\nrun: none\n"), "{text}");
        assert!(sample(None).ends_with("context: unavailable\n"));
    }

    #[test]
    fn the_report_is_ascii_the_font_and_any_clipboard_can_carry() {
        let text = sample(None);
        assert!(text.is_ascii());
    }

    #[test]
    fn note_appends_to_the_log_and_restarts_it_for_a_new_run() {
        let mut run = RunState::new(7, 320);
        note("Playing", Some(&run), (900, 600), false);
        run.log.push(shot(0, 900, 600));
        run.log.push(shot(0, 905, 600));
        note("Playing", Some(&run), (905, 600), false);
        let got = CONTEXT.lock().unwrap().run.clone().unwrap();
        assert_eq!(got.log, run.log);
        assert_eq!((got.angle_ticks, got.power_ticks), (905, 600));

        let fresh = RunState::new(8, 320);
        note("Playing", Some(&fresh), (900, 600), true);
        let got = CONTEXT.lock().unwrap().run.clone().unwrap();
        assert_eq!(got.seed, 8);
        assert!(got.log.is_empty());
        assert!(got.resumed);

        note("Title", None, (0, 0), false);
        assert_eq!(CONTEXT.lock().unwrap().run, None);
    }

    #[rstest::rstest]
    #[case::our_crate(
        "crates/salt-line-sim/src/run.rs:940:9",
        "crates/salt-line-sim/src/run.rs:940:9"
    )]
    #[case::registry_unix(
        "/home/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/godot-core-0.5.5/src/private.rs:331:5",
        "godot-core-0.5.5/src/private.rs:331:5"
    )]
    #[case::registry_windows(
        "C:\\Users\\someone\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\godot-core-0.5.5\\src\\private.rs:331:5",
        "godot-core-0.5.5/src/private.rs:331:5"
    )]
    #[case::our_crate_on_windows(
        "crates\\salt-line-godot\\src\\crash.rs:457:45",
        "crates/salt-line-godot/src/crash.rs:457:45"
    )]
    #[case::unc_windows(
        "\\\\server\\share\\someone\\deps\\glam-0.32.1\\src\\f32.rs:1:1",
        "glam-0.32.1/src/f32.rs:1:1"
    )]
    #[case::mixed_separators(
        "D:/a/saltline\\crates\\salt-line-sim\\src/run.rs:9:1",
        "salt-line-sim/src/run.rs:9:1"
    )]
    #[case::no_src_windows("C:\\Users\\someone\\out\\generated.rs:3:1", "generated.rs:3:1")]
    #[case::std(
        "/rustc/0123abcd/library/core/src/panicking.rs:75:14",
        "core/src/panicking.rs:75:14"
    )]
    #[case::nested_src(
        "/home/someone/w/crates/salt-line-godot/src/bin/src/x.rs:1:1",
        "bin/src/x.rs:1:1"
    )]
    #[case::no_src("/home/someone/build/out/generated.rs:3:1", "generated.rs:3:1")]
    fn locations_lose_the_build_machine(#[case] loc: &str, #[case] want: &str) {
        assert_eq!(tidy_path(loc), want);
    }

    #[test]
    fn the_report_carries_the_tidied_location() {
        let text = format_report(&Report {
            version: "0.1.0",
            os: "linux",
            arch: "x86_64",
            message: "boom",
            location: Some(
                "/home/someone/.cargo/registry/src/i/godot-core-0.5.5/src/x.rs:1:2".into(),
            ),
            context: None,
        });
        assert!(
            text.contains("at: godot-core-0.5.5/src/x.rs:1:2\n"),
            "{text}"
        );
        assert!(!text.contains("someone"), "{text}");
    }

    // The hook itself. It is process-wide state, so these need a process
    // each: cargo-nextest (what CI runs) gives every test its own; under
    // plain `cargo test` they would share one and see each other's panics,
    // so they stand down there.
    fn own_process() -> bool {
        let yes = std::env::var_os("NEXTEST").is_some();
        if !yes {
            eprintln!("skipped: needs cargo-nextest's process-per-test");
        }
        yes
    }

    fn scratch_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("salt-line-crash-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("crash.txt")
    }

    #[test]
    fn the_first_panic_writes_the_report_and_later_ones_do_not() {
        if !own_process() {
            return;
        }
        let file = scratch_file("first");
        install(Some(file.clone()));
        note(
            "Playing",
            Some(&RunState::new(20260907, 320)),
            (900, 600),
            false,
        );
        assert!(!panicked() && !report_ready() && report().is_none());

        let _ = std::panic::catch_unwind(|| panic!("the first one"));
        assert!(panicked() && report_ready());
        let on_disk = fs::read_to_string(&file).unwrap();
        assert_eq!(report().as_deref(), Some(on_disk.as_str()));
        assert!(on_disk.contains("panic: the first one\n"), "{on_disk}");
        assert!(on_disk.contains("seed: 20260907\n"), "{on_disk}");
        assert!(
            on_disk.contains("at: crates/salt-line-godot/src/crash.rs:"),
            "{on_disk}"
        );

        let _ = std::panic::catch_unwind(|| panic!("fallout"));
        assert_eq!(fs::read_to_string(&file).unwrap(), on_disk);
        let _ = fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn the_previous_hook_still_runs_and_install_is_idempotent() {
        if !own_process() {
            return;
        }
        use std::sync::atomic::AtomicU32;
        static CALLS: AtomicU32 = AtomicU32::new(0);
        std::panic::set_hook(Box::new(|_| {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }));
        install(None);
        // A hot reload runs the init stage again; that must not chain the
        // hook to itself and report twice.
        install(None);
        let _ = std::panic::catch_unwind(|| panic!("chained"));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert!(report().unwrap().contains("panic: chained\n"));

        // Unloading takes ours (and what it chained) out.
        uninstall();
        uninstall();
        let _ = std::panic::catch_unwind(|| panic!("after unload"));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panic_while_the_context_is_held_reports_it_unavailable() {
        if !own_process() {
            return;
        }
        std::panic::set_hook(Box::new(|_| {}));
        install(None);
        let _ = std::panic::catch_unwind(|| {
            let _held = CONTEXT.lock().unwrap();
            panic!("inside note");
        });
        let text = report().unwrap();
        assert!(text.ends_with("context: unavailable\n"), "{text}");
    }
}

//! The developer flags on the command line, with no engine in them.
//!
//! The node collects the arguments (Godot splits them either side of `--`,
//! and both halves are read) and [`DevArgs::parse`] reads them. CI's smoke
//! test runs exported builds with these flags, so their spelling is a
//! contract: `--salt-seed 20260907 --salt-screen 12 --salt-shot smoke.png
//! --salt-shot-at 0.5`.
//!
//! A flag with a value takes it as the next argument or after `=`
//! (`--salt-seed 42`, `--salt-seed=42`). The first occurrence wins. A next
//! argument that starts `--` is another flag, not the value: the flag is
//! read as having none.
//!
//! Parsing is kept apart from acting: [`DevArgs`] is plain data, tested here
//! without an engine, and `Game::start_dev` (in the core) or the node's
//! `ready` (for what needs the engine: captures and the gallery) act on it.
//! A number that does not parse is ignored, as if its flag were absent.

use salt_line_sim::run::parse_seed;

/// When `--salt-shot` captures if `--salt-shot-at` does not say.
pub const DEFAULT_SHOT_AT: f64 = 0.05;

/// The window `--salt-phone` lays the game out for: a Pixel 10 held
/// landscape, 2424 by 1080 (a buffer of 400).
pub const PHONE_WINDOW: (i32, i32) = (2424, 1080);

/// Every developer flag, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct DevArgs {
    /// `--salt-shot <path>`: render a few frames, save the buffer as a PNG at
    /// native resolution, and quit. How the pixel output gets checked without
    /// a human at the keyboard.
    pub shot: Option<String>,
    /// `--salt-shot-at <seconds>`: when to grab the screenshot, in seconds
    /// since the node came up. Counted in capped frame time
    /// (`resume::frame_delta`), so a slow frame does not skip past it.
    pub shot_at: f64,
    /// `--salt-auto`: play with the solver. See `Game::autopilot`.
    pub auto: bool,
    /// `--salt-miss`: the autopilot throws the first shot of each screen away.
    pub miss: bool,
    /// `--salt-hold-draft`: the autopilot leaves the draft on screen.
    pub hold_draft: bool,
    /// `--salt-seed <seed>`: start a run immediately, for screenshots and for
    /// looking at a specific screen. Any text, as seed entry takes it.
    pub seed: Option<u64>,
    /// `--salt-screen <n>`: jump the run forward, for inspecting late bands.
    /// Zero-based, as the sim counts screens (the HUD shows `n + 1`); only
    /// read with `--salt-seed`, and a jumped run is not kept for CONTINUE.
    pub screen: Option<u32>,
    /// `--salt-continue`: continue the saved run, as the title's CONTINUE.
    pub continue_run: bool,
    /// `--salt-gallery <dir>`: render the terrain template gallery and quit.
    pub gallery: Option<String>,
    /// `--salt-versus`: start a local versus match at once, at its first
    /// handoff. With `--salt-auto`, the autopilot plays both guns.
    pub versus: bool,
    /// `--salt-demo`: start the attract demo at once, as the title's DEMO
    /// does (DESIGN §12): the screen is kept on until it ends.
    pub demo: bool,
    /// `--salt-layout <code>`: start a local versus match on a layout's
    /// share code (DESIGN §13), at its first handoff. A code that does not
    /// read or pass leaves the title up with a note.
    pub layout: Option<String>,
    /// `--salt-editor`: open the map editor at once (DESIGN §13.11), on the
    /// kept draft if there is one, or on `--salt-layout`'s code if given.
    pub editor: bool,
    /// `--salt-editor-page <page>`: with `--salt-editor`, open that page
    /// (`start`, `menu`, `slots`, `save`, `seed`, `name` or `test`), for
    /// captures. The editing screen otherwise.
    pub editor_page: Option<String>,
    /// `--salt-setup`: open the versus pre-match screen at once, with
    /// `--salt-layout`'s code picked on its GROUND row if given.
    pub setup: bool,
    /// `--salt-phone`: lay the game out as a phone would (touch controls,
    /// the corner pad, [`PHONE_WINDOW`]), for captures on a desktop.
    pub phone: bool,
    /// `--salt-flat`: draw every frame as one canvas, sky and ground
    /// included, instead of in layers (`backdrop`). The reference the layers
    /// are compared with, and a developer fallback for telling a layering
    /// bug from any other; not a player setting.
    pub flat: bool,
    /// `--salt-fork-run <seed>:<screen>`: print the share code of that run
    /// screen forked into a layout, and quit (`custom::fork_report`).
    pub fork_run: Option<String>,
    /// `--salt-fork-duel <seed>:<round>`: as `--salt-fork-run`, for round
    /// `round` (from 0) of the duel on match seed `seed`.
    pub fork_duel: Option<String>,
}

impl DevArgs {
    /// Read the flags out of every argument, whether it arrived before or
    /// after `--`.
    pub fn parse(args: &[String]) -> DevArgs {
        DevArgs {
            shot: value(args, "--salt-shot"),
            shot_at: value(args, "--salt-shot-at")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_SHOT_AT),
            auto: flag(args, "--salt-auto"),
            miss: flag(args, "--salt-miss"),
            hold_draft: flag(args, "--salt-hold-draft"),
            seed: value(args, "--salt-seed").map(|s| parse_seed(&s)),
            screen: value(args, "--salt-screen").and_then(|s| s.parse().ok()),
            continue_run: flag(args, "--salt-continue"),
            gallery: value(args, "--salt-gallery"),
            versus: flag(args, "--salt-versus"),
            demo: flag(args, "--salt-demo"),
            layout: value(args, "--salt-layout"),
            editor: flag(args, "--salt-editor"),
            editor_page: value(args, "--salt-editor-page"),
            setup: flag(args, "--salt-setup"),
            phone: flag(args, "--salt-phone"),
            flat: flag(args, "--salt-flat"),
            fork_run: value(args, "--salt-fork-run"),
            fork_duel: value(args, "--salt-fork-duel"),
        }
    }
}

/// Presence of a valueless flag.
fn flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// The value of the first occurrence of `flag`: the argument after it, or
/// the text after `flag=`. `None` if the flag is absent, last, or followed
/// by another flag (anything starting `--`): `--salt-shot --salt-auto` is a
/// shot with no path, not a screenshot saved as `--salt-auto`. A value that
/// really starts `--` can still be given after `=`.
fn value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().filter(|v| !v.starts_with("--")).cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_ci_smoke_flags_parse() {
        let a = DevArgs::parse(&args(
            "--headless -- --salt-seed 20260907 --salt-screen 12 --salt-shot /x/smoke.png --salt-shot-at 0.5",
        ));
        assert_eq!(a.seed, Some(parse_seed("20260907")));
        assert_eq!(a.screen, Some(12));
        assert_eq!(a.shot.as_deref(), Some("/x/smoke.png"));
        assert_eq!(a.shot_at, 0.5);
        assert!(!a.auto && !a.continue_run);
    }

    #[test]
    fn nothing_given_is_nothing_set() {
        let a = DevArgs::parse(&[]);
        assert_eq!(
            a,
            DevArgs {
                shot: None,
                shot_at: DEFAULT_SHOT_AT,
                auto: false,
                miss: false,
                hold_draft: false,
                seed: None,
                screen: None,
                continue_run: false,
                gallery: None,
                versus: false,
                demo: false,
                layout: None,
                editor: false,
                editor_page: None,
                setup: false,
                phone: false,
                flat: false,
                fork_run: None,
                fork_duel: None,
            }
        );
    }

    #[test]
    fn the_layout_flags_take_their_values() {
        let a = DevArgs::parse(&args(
            "-- --salt-layout SL04G0 --salt-fork-run DUNE:8 --salt-fork-duel=42:1",
        ));
        assert_eq!(a.layout.as_deref(), Some("SL04G0"));
        assert_eq!(a.fork_run.as_deref(), Some("DUNE:8"));
        assert_eq!(a.fork_duel.as_deref(), Some("42:1"));
        assert!(!a.versus && a.seed.is_none());
    }

    #[test]
    fn versus_is_a_flag_of_its_own() {
        let a = DevArgs::parse(&args("--salt-versus --salt-auto"));
        assert!(a.versus && a.auto);
        assert_eq!(a.seed, None);
    }

    #[test]
    fn demo_is_a_flag_of_its_own() {
        let a = DevArgs::parse(&args("-- --salt-demo"));
        assert!(a.demo && !a.versus && !a.auto);
        assert_eq!(a.seed, None);
    }

    #[test]
    fn values_follow_an_equals_sign_too() {
        let a = DevArgs::parse(&args("--salt-seed=HELLO --salt-gallery=/tmp/g --salt-auto"));
        assert_eq!(a.seed, Some(parse_seed("HELLO")));
        assert_eq!(a.gallery.as_deref(), Some("/tmp/g"));
        assert!(a.auto);
    }

    #[test]
    fn a_bad_number_is_ignored_and_a_missing_value_is_none() {
        let a = DevArgs::parse(&args(
            "--salt-screen twelve --salt-shot-at soon --salt-shot",
        ));
        assert_eq!(a.screen, None);
        assert_eq!(a.shot_at, DEFAULT_SHOT_AT);
        assert_eq!(a.shot, None);
    }

    #[test]
    fn a_flag_is_not_taken_as_the_value_before_it() {
        // `--salt-shot` given no path must not save to a file named
        // `--salt-auto`, and `--salt-auto` must still count.
        let a = DevArgs::parse(&args("--salt-shot --salt-auto --salt-seed --salt-miss"));
        assert_eq!(a.shot, None);
        assert_eq!(a.seed, None);
        assert!(a.auto && a.miss);
    }

    #[test]
    fn a_flag_is_not_a_prefix_match() {
        // `--salt-shot-at` must not be read as `--salt-shot` with a value.
        let a = DevArgs::parse(&args("--salt-shot-at 2"));
        assert_eq!(a.shot, None);
        assert_eq!(a.shot_at, 2.0);
    }
}

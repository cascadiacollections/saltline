//! The developer flags on the command line, with no engine in them.
//!
//! The node collects the arguments (Godot splits them either side of `--`,
//! and both halves are read) and [`DevArgs::parse`] reads them. CI's smoke
//! test runs exported builds with these flags, so their spelling is a
//! contract: `--salt-seed 20260907 --salt-screen 12 --salt-shot smoke.png
//! --salt-shot-at 0.5`.
//!
//! A flag with a value takes it as the next argument or after `=`
//! (`--salt-seed 42`, `--salt-seed=42`). The first occurrence wins.
//!
//! Parsing is kept apart from acting: [`DevArgs`] is plain data, tested here
//! without an engine, and `Game::start_dev` (in the core) or the node's
//! `ready` (for what needs the engine: captures and the gallery) act on it.
//! A number that does not parse is ignored, as if its flag were absent.

use salt_line_sim::run::parse_seed;

/// When `--salt-shot` captures if `--salt-shot-at` does not say.
pub const DEFAULT_SHOT_AT: f64 = 0.05;

/// Every developer flag, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct DevArgs {
    /// `--salt-shot <path>`: render a few frames, save the buffer as a PNG at
    /// native resolution, and quit. How the pixel output gets checked without
    /// a human at the keyboard.
    pub shot: Option<String>,
    /// `--salt-shot-at <seconds>`: when to grab the screenshot, in seconds
    /// of capped game time since the node came up.
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
        }
    }
}

/// Presence of a valueless flag.
fn flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// The value of the first occurrence of `flag`: the argument after it, or
/// the text after `flag=`. `None` if the flag is absent or last. The next
/// argument is taken whatever it is, even another flag.
fn value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
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
            }
        );
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
    fn a_flag_is_not_a_prefix_match() {
        // `--salt-shot-at` must not be read as `--salt-shot` with a value.
        let a = DevArgs::parse(&args("--salt-shot-at 2"));
        assert_eq!(a.shot, None);
        assert_eq!(a.shot_at, 2.0);
    }
}

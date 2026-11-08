//! Which edition this build is: the free shareware build or the full game.
//!
//! The code is the same in both. An export preset with the `shareware`
//! custom feature (or `--salt-shareware` on the command line) makes the
//! node start the game as [`Tier::Shareware`]. That edition:
//!
//! - ends endless and daily runs after [`SHAREWARE_SCREENS`] cleared
//!   screens, scored as usual, with an end-of-shareware result screen;
//! - leaves the bundled challenge course and the map editor out.
//!
//! The simulation never sees the tier. A seed plays the same screens, drafts
//! and wind in both editions up to the cap, so replays, share codes and the
//! daily board stay comparable.

/// Cleared screens a shareware run gets: three drafts' worth.
pub const SHAREWARE_SCREENS: u32 = 12;

/// The note shown when the shareware build is asked for a full-game feature.
pub const FULL_GAME_ONLY: &str = "FULL GAME ONLY";

/// The edition of the game this build is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tier {
    /// Everything, no cap. The default, so tests and dev runs see the whole
    /// game unless they ask otherwise.
    #[default]
    Full,
    /// The free build: capped runs, no challenges, no editor.
    Shareware,
}

impl Tier {
    /// The tier for a build with these feature flags: shareware if the export
    /// preset or the command line asked for it.
    pub fn detect(export_feature: bool, dev_flag: bool) -> Self {
        if export_feature || dev_flag {
            Self::Shareware
        } else {
            Self::Full
        }
    }

    pub fn is_shareware(self) -> bool {
        self == Self::Shareware
    }

    /// Whether a run that has cleared `screens_cleared` screens has reached
    /// this edition's end.
    pub fn run_capped(self, screens_cleared: u32) -> bool {
        self.is_shareware() && screens_cleared >= SHAREWARE_SCREENS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_the_default_and_never_caps() {
        assert_eq!(Tier::default(), Tier::Full);
        assert!(!Tier::Full.run_capped(u32::MAX));
    }

    #[test]
    fn either_flag_selects_shareware() {
        assert_eq!(Tier::detect(false, false), Tier::Full);
        assert_eq!(Tier::detect(true, false), Tier::Shareware);
        assert_eq!(Tier::detect(false, true), Tier::Shareware);
    }

    #[test]
    fn shareware_caps_at_the_limit_not_before() {
        assert!(!Tier::Shareware.run_capped(SHAREWARE_SCREENS - 1));
        assert!(Tier::Shareware.run_capped(SHAREWARE_SCREENS));
    }
}

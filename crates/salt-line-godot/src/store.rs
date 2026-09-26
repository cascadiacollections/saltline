//! Entitlement and the storefront seam. DESIGN §1: one purchase, no ads, no
//! timers.
//!
//! Nothing here grants anything. The local save file is a *cache* of what the
//! platform said last time, never the authority — it is a plain text file in
//! `user://` that a player can edit, so treating it as proof of purchase would
//! be treating the player's own disk as a receipt.
//!
//! The build ships one of two ways:
//!
//! * **Development preview** (default, `monetization` off). There is no
//!   paywall and nothing is for sale. Every screen is playable and the title
//!   screen says so.
//! * **Monetized** (`--features monetization`). The paywall is live and the
//!   unlock must come from a platform purchase or restore. Until a real
//!   storefront SDK is wired up, both answer `Unavailable`, which is the
//!   honest answer and cannot be mistaken for a sale.
//!
//! The module is a *seam*: a narrow, engine-free interface that `game.rs`
//! calls at the three points a store matters (launch, purchase, restore).
//! A platform SDK replaces the bodies of these functions; nothing else in the
//! game needs to know which store, if any, is behind them. `game.rs` shows the
//! paywall once a `Free` player reaches the screen after the last free one,
//! and sets the cached flag only on [`Outcome::Granted`].

/// Is this build selling anything at all? True only with the
/// `monetization` Cargo feature.
pub const MONETIZED: bool = cfg!(feature = "monetization");

/// What the platform says the player owns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entitlement {
    /// Everything is playable. In a development preview this is unconditional.
    Full,
    /// Free content only — screens
    /// 1..=[`FREE_SCREENS`](salt_line_sim::run::FREE_SCREENS), counted from 1
    /// as the player sees them.
    Free,
}

/// Result of asking the platform to sell or restore the unlock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The platform confirmed the entitlement.
    Granted,
    /// The player backed out. Not an error.
    Cancelled,
    /// No storefront could answer. The reason is shown to the player.
    Unavailable(&'static str),
}

/// What this build is entitled to at launch.
///
/// `cached` is the `unlocked` flag read from the save file. It is currently
/// ignored: a development preview is always [`Entitlement::Full`], and a
/// monetized build with no storefront to ask is always
/// [`Entitlement::Free`]. When a real SDK lands, this function asks it and
/// the cache becomes a display hint, never the authority.
pub fn entitlement(cached: bool) -> Entitlement {
    if !MONETIZED {
        // Nothing is for sale, so there is nothing to gate.
        return Entitlement::Full;
    }
    let _ = cached;
    // No storefront SDK is linked. A monetized build with no way to verify a
    // purchase must not unlock on the strength of an editable local file.
    Entitlement::Free
}

/// Begin the one-time unlock purchase.
pub fn purchase() -> Outcome {
    if !MONETIZED {
        return Outcome::Unavailable("THIS BUILD IS NOT FOR SALE");
    }
    Outcome::Unavailable("STORE UNAVAILABLE")
}

/// Restore a purchase already made on this account.
pub fn restore() -> Outcome {
    if !MONETIZED {
        return Outcome::Unavailable("THIS BUILD IS NOT FOR SALE");
    }
    Outcome::Unavailable("STORE UNAVAILABLE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_development_preview_gates_nothing_and_sells_nothing() {
        if MONETIZED {
            return;
        }
        assert_eq!(entitlement(false), Entitlement::Full);
        assert!(matches!(purchase(), Outcome::Unavailable(_)));
        assert!(matches!(restore(), Outcome::Unavailable(_)));
    }

    #[test]
    fn a_local_flag_never_grants_the_unlock() {
        // The save file is a cache. With no storefront to confirm it, a
        // monetized build stays on the free tier no matter what it says.
        if !MONETIZED {
            return;
        }
        assert_eq!(entitlement(true), Entitlement::Free);
    }

    #[test]
    fn no_code_path_here_grants_anything_without_a_storefront() {
        assert_ne!(purchase(), Outcome::Granted);
        assert_ne!(restore(), Outcome::Granted);
    }
}

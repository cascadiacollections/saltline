//! Snapshot tests (insta). Each pins output that should only ever change on
//! purpose; `cargo insta review` shows the diff and accepts it.

mod common;

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::screen_hash;
use salt_line_sim::run::generate_screen;

/// Screen digests for a few seeds and both extreme widths. These are what
/// seed sharing promises: the same seed is the same screen on every platform
/// and every build. CI runs this on Linux, macOS and Windows, so a diff here
/// is a determinism break, and an intended one belongs in NOTES.md.
///
/// The screen set is `common::pinned_screens`: early screens, then deeper
/// bands, where screens carry several targets and their spacing is pinned.
#[test]
fn screen_digests_are_pinned() {
    let lo = Loadout::default();
    let mut out = String::new();
    let mut heading = None;
    for (seed, width, index) in common::pinned_screens() {
        if heading != Some((seed, width)) {
            heading = Some((seed, width));
            out.push_str(&format!("seed {seed:#x}, width {width}\n"));
        }
        let h = screen_hash(&generate_screen(seed, index, width, &lo));
        out.push_str(&format!("  {index:3} {h:016x}\n"));
    }
    insta::assert_snapshot!(out);
}

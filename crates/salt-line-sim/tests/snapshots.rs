//! Snapshot tests (insta). Each pins output that should only ever change on
//! purpose; `cargo insta review` shows the diff and accepts it.

use salt_line_sim::modifiers::Loadout;
use salt_line_sim::replay::screen_hash;
use salt_line_sim::run::generate_screen;

/// Screen digests for a few seeds and both extreme widths. These are what
/// seed sharing promises: the same seed is the same screen on every platform
/// and every build. CI runs this on Linux, macOS and Windows, so a diff here
/// is a determinism break, and an intended one belongs in NOTES.md.
#[test]
fn screen_digests_are_pinned() {
    let lo = Loadout::default();
    let mut out = String::new();
    for seed in [1u64, 0xFEED_FACE, 20260907] {
        for width in [320usize, 400] {
            out.push_str(&format!("seed {seed:#x}, width {width}\n"));
            // Early screens, then deeper bands, where screens carry several
            // targets and their spacing is part of what is pinned.
            for index in (0..12).chain(40..44).chain(120..124) {
                let h = screen_hash(&generate_screen(seed, index, width, &lo));
                out.push_str(&format!("  {index:3} {h:016x}\n"));
            }
        }
    }
    insta::assert_snapshot!(out);
}

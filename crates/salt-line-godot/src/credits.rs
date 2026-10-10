//! Offline credits and license notices, rendered with the game's bitmap font.
//! The bundled notices come from Godot's LICENSE.txt, gdext's License.txt,
//! and the Cargo.lock versions of glam/libc (MIT option). Update notices when
//! replacing dependencies; addon notices remain in their upstream directory.

const OVERVIEW: &str = "SALT LINE\nCascadia Collections\nCopyright 2026 Cascadia Collections\nSource: github.com/cascadiacollections/saltline\nLicense: MIT\n\nGODOT ENGINE\nGodot Engine contributors\nJuan Linietsky and Ariel Manzur\nLicense: MIT\ngodotengine.org/license\n\nGODOT RUST BINDINGS (GDEXT)\ngodot-rust contributors\nLicense: MPL-2.0\ngithub.com/godot-rust/gdext\nmozilla.org/MPL/2.0\n\nRUST RUNTIME LIBRARIES\nglam - MIT OR Apache-2.0\ngithub.com/bitshifter/glam-rs\nlibc - MIT OR Apache-2.0\ngithub.com/rust-lang/libc\n\nLINUX DESKTOP INTEGRATION\ngodot-xdg-portals - Kevin Coughlin\nLicense: MIT\ngithub.com/KevinTCoughlin/godot-xdg-portals\ngodot-cpp - Godot Engine contributors (MIT)\nGLib / GIO / GObject (system): LGPL-2.1+\nxdg-desktop-portal (service): LGPL-2.1+\n\nTHANK YOU\nTo the free and open-source communities\nwhose work makes Salt Line possible.";

/// Wrap at the native 320px buffer width; pages leave room for navigation.
pub fn pages() -> &'static [Vec<String>] {
    static PAGES: std::sync::OnceLock<Vec<Vec<String>>> = std::sync::OnceLock::new();
    PAGES.get_or_init(build_pages)
}

fn build_pages() -> Vec<Vec<String>> {
    let notices = format!(
        "{OVERVIEW}\n\nSALT LINE LICENSE\n{}\n\nLINUX ADDON LICENSE\n{}\n\nLINUX THIRD-PARTY NOTICES\n{}\n\nGODOT ENGINE LICENSE\n{}\n\nGDEXT LICENSE\n{}\n\nGLAM LICENSE\n{}\n\nLIBC LICENSE\n{}",
        include_str!("../../../LICENSE"),
        include_str!("../../../addons/xdg_portals/LICENSE"),
        include_str!("../../../addons/xdg_portals/THIRD_PARTY_NOTICES.md"),
        include_str!("../../../third_party/godot-LICENSE.txt"),
        include_str!("../../../third_party/gdext-LICENSE-MPL-2.0.txt"),
        include_str!("../../../third_party/glam-LICENSE-MIT.txt"),
        include_str!("../../../third_party/libc-LICENSE-MIT.txt"),
    );
    let mut lines = Vec::new();
    for paragraph in notices.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace().flat_map(|word| {
            word.chars()
                .collect::<Vec<_>>()
                .chunks(48)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
        }) {
            if !line.is_empty() && line.len() + word.len() + 1 > 48 {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines.chunks(12).map(|page| page.to_vec()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_license_pages_fit_the_native_buffer() {
        for page in super::pages() {
            assert!(page.len() <= 12);
            for line in page {
                assert!(crate::font::small_width(line) <= 300, "{line}");
            }
        }
    }
}

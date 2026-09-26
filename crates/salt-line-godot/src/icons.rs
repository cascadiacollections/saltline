//! HUD icons, drawn like the font: bitmaps in the game's own buffer, so the
//! HUD reads without words (DESIGN §5.1, §5.6).
//!
//! Each row is one pixel row. `.` is transparent and a digit picks a colour
//! from the list the caller passes: `1` the first, `2` the second, and so on.
//! All icons are as tall as the big digits they sit beside.
//!
//! This is palette-indexed pixel art in its smallest form: the icon stores
//! colour *indices* into a short per-call list, not colours, and
//! [`Canvas::icon`](crate::raster::Canvas::icon) resolves them against
//! palette entries the caller chooses. The same icon can therefore be drawn
//! in different slots without a second bitmap, and can never introduce a
//! seventeenth colour (DESIGN §5.2). `Canvas::icon` can also mirror an icon
//! horizontally, which is how the windsock points left — a flip, not a
//! rotation (DESIGN §5.1).

use crate::font::BIG_H;

/// One icon: [`BIG_H`] rows of equal length, each character `.` or a
/// colour digit `1`–`9`. Width is the row length in buffer pixels.
pub type Icon = [&'static str; BIG_H];

/// A shell standing on its base: body, then the driving band.
pub const SHELL: Icon = [
    ".11.", //
    "1111", //
    "1111", //
    "1111", //
    "1111", //
    "3333", //
    "1111", //
];

/// A windsock flying to the right, striped from the pole. Mirror it for wind
/// to the left. It is the same size whatever the strength: the number beside
/// it is the read (DESIGN §2.5).
pub const SOCK: Icon = [
    "3........",
    "3222111..",
    "322211111",
    "3222111..",
    "3........",
    "3........",
    "3........",
];

/// The windsock hanging limp: no wind, or none shown (Dead Reckoning). As wide
/// as the flying sock so the number beside it never moves.
pub const SOCK_LIMP: Icon = [
    "3........",
    "32.......",
    "322......",
    "321......",
    "321......",
    "31.......",
    "3........",
];

/// Width of `icon` in buffer pixels, taken from its first row; every row is
/// the same length (checked by the tests below).
pub fn width(icon: &Icon) -> i32 {
    icon[0].len() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_rectangular_and_use_only_colour_digits() {
        for icon in [&SHELL, &SOCK, &SOCK_LIMP] {
            for row in icon.iter() {
                assert_eq!(row.len(), icon[0].len());
                assert!(row.bytes().all(|b| b == b'.' || (b'1'..=b'3').contains(&b)));
            }
        }
    }

    #[test]
    fn the_sock_is_as_wide_limp_as_flying() {
        assert_eq!(width(&SOCK), width(&SOCK_LIMP));
    }

    #[test]
    #[cfg_attr(miri, ignore = "insta opens a socket, which Miri cannot emulate")]
    fn the_hud_icons_draw_as_designed() {
        use crate::palette::{slot, Palette};
        use crate::raster::Canvas;
        let pal = Palette::default();
        let colors = [
            pal.get(slot::UI_FG),
            pal.get(slot::ALERT),
            pal.get(slot::SKY_LO),
        ];
        let mut c = Canvas::new(40, 7);
        c.icon(0, 0, &SHELL, &colors, false);
        c.icon(6, 0, &SOCK, &colors, false);
        c.icon(17, 0, &SOCK, &colors, true);
        c.icon(28, 0, &SOCK_LIMP, &colors, false);
        insta::assert_snapshot!(c.palette_text(&pal));
    }
}

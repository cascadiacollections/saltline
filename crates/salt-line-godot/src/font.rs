//! Bitmap fonts, drawn at native resolution into the pixel buffer.
//!
//! DESIGN §5.6: 5x7 for the shell counter and the wind value, 3x5 for the
//! screen index, and never a TTF scaled down into the buffer. The draft cards
//! use the same font at the same size — "resist the urge to make draft UI
//! larger and smoother than the game".
//!
//! Each glyph is a 1-bit mask written as strings of `'0'` and `'1'`, one
//! string per pixel row, top to bottom. The mask carries no colour: the
//! caller picks a palette slot and
//! [`Canvas::text_small`](crate::raster::Canvas::text_small) /
//! [`Canvas::text_big`](crate::raster::Canvas::text_big) set every `1` pixel
//! to it, so text obeys the same sixteen-colour rule as everything else
//! (DESIGN §5.2). Against packed bits, strings keep each glyph legible and
//! diffable in the source; the cost is a few bytes of `.rodata` and a byte
//! compare per pixel, negligible at this resolution.
//!
//! The layout is monospaced: every character, including one with no glyph,
//! advances the cursor by the same amount. That keeps [`small_width`] and
//! [`big_width`] a multiplication, which is what the HUD uses to size its
//! plates before drawing.

/// Width of a 3x5 glyph, in buffer pixels.
pub const SMALL_W: usize = 3;
/// Height of a 3x5 glyph, in buffer pixels.
pub const SMALL_H: usize = 5;
/// One pixel of tracking between 3x5 glyphs.
pub const SMALL_ADVANCE: usize = 4;

/// The 3x5 mask for `c`, or `None` if the face has no glyph for it.
///
/// The subset in `tools/gen_sprites.py`, completed to the full alphabet so UI
/// strings do not silently lose letters.
///
/// Case-insensitive: the face is capitals only, so lower-case input draws as
/// upper case. A `None` still advances the cursor when drawn, so an
/// unsupported character shows as a gap rather than collapsing the string.
pub fn small_glyph(c: char) -> Option<[&'static str; SMALL_H]> {
    Some(match c.to_ascii_uppercase() {
        'A' => ["111", "101", "111", "101", "101"],
        'B' => ["110", "101", "110", "101", "110"],
        'C' => ["111", "100", "100", "100", "111"],
        'D' => ["110", "101", "101", "101", "110"],
        'E' => ["111", "100", "111", "100", "111"],
        'F' => ["111", "100", "111", "100", "100"],
        'G' => ["111", "100", "101", "101", "111"],
        'H' => ["101", "101", "111", "101", "101"],
        'I' => ["111", "010", "010", "010", "111"],
        'J' => ["001", "001", "001", "101", "111"],
        'K' => ["101", "101", "110", "101", "101"],
        'L' => ["100", "100", "100", "100", "111"],
        // `gen_sprites.py` gives M and N the same glyph; N is narrowed here so
        // the two are distinguishable at 3px wide.
        'M' => ["101", "111", "111", "101", "101"],
        'N' => ["101", "111", "101", "101", "101"],
        // Clipped corners, so the letter O and the numeral 0 are not the same
        // five rows. At 3px wide a shared glyph is a misspelling on screen.
        'O' => ["010", "101", "101", "101", "010"],
        'P' => ["111", "101", "111", "100", "100"],
        'Q' => ["111", "101", "101", "111", "001"],
        'R' => ["110", "101", "110", "101", "101"],
        // The numeral 5 keeps the square top bar; the letter S loses its
        // corners, the same trick as O against 0.
        'S' => ["011", "100", "010", "001", "110"],
        'T' => ["111", "010", "010", "010", "010"],
        'U' => ["101", "101", "101", "101", "111"],
        'V' => ["101", "101", "101", "101", "010"],
        'W' => ["101", "101", "111", "111", "101"],
        'X' => ["101", "101", "010", "101", "101"],
        'Y' => ["101", "101", "111", "010", "010"],
        'Z' => ["111", "001", "010", "100", "111"],
        '0' => ["111", "101", "101", "101", "111"],
        '1' => ["010", "110", "010", "010", "111"],
        '2' => ["111", "001", "111", "100", "111"],
        '3' => ["111", "001", "111", "001", "111"],
        '4' => ["101", "101", "111", "001", "001"],
        '5' => ["111", "100", "111", "001", "111"],
        '6' => ["111", "100", "111", "101", "111"],
        '7' => ["111", "001", "001", "001", "001"],
        '8' => ["111", "101", "111", "101", "111"],
        '9' => ["111", "101", "111", "001", "111"],
        '.' => ["000", "000", "000", "000", "010"],
        // A middle dot, the separator in the demo banner (DESIGN §12).
        '\u{b7}' => ["000", "000", "010", "000", "000"],
        ',' => ["000", "000", "000", "010", "100"],
        '+' => ["000", "010", "111", "010", "000"],
        '-' => ["000", "000", "111", "000", "000"],
        ':' => ["000", "010", "000", "010", "000"],
        '/' => ["001", "001", "010", "100", "100"],
        '\'' => ["010", "010", "000", "000", "000"],
        '!' => ["010", "010", "010", "000", "010"],
        '?' => ["111", "001", "011", "000", "010"],
        '>' => ["100", "110", "111", "110", "100"],
        '<' => ["001", "011", "111", "011", "001"],
        '(' => ["010", "100", "100", "100", "010"],
        ')' => ["010", "001", "001", "001", "010"],
        // The map editor's keys for the previous and next tool.
        '[' => ["110", "100", "100", "100", "110"],
        ']' => ["011", "001", "001", "001", "011"],
        // A small multiplication sign. A lower-case `x` never gets here: it
        // is upper-cased above and draws as the letter `X`.
        '*' => ["000", "101", "010", "101", "000"],
        ' ' => ["000", "000", "000", "000", "000"],
        _ => return None,
    })
}

/// Width of a 5x7 glyph, in buffer pixels.
pub const BIG_W: usize = 5;
/// Height of a 5x7 glyph, in buffer pixels. HUD icons are drawn to the same height so
/// they sit level with the digits beside them (see [`crate::icons`]).
pub const BIG_H: usize = 7;
/// One pixel of tracking between 5x7 glyphs.
pub const BIG_ADVANCE: usize = 6;

/// The 5x7 mask for `c`, or `None` outside digits, `+ - . :` and space.
///
/// Digits and the few symbols the instruments need. Labels stay 3x5; only
/// the numbers a player reads mid-shot get the larger face, so unlike
/// [`small_glyph`] there are no letters.
pub fn big_glyph(c: char) -> Option<[&'static str; BIG_H]> {
    Some(match c {
        '0' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11111", "00010", "00100", "00010", "00001", "10001", "01110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "11110", "00001", "00001", "10001", "01110",
        ],
        '6' => [
            "00110", "01000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00010", "01100",
        ],
        '+' => [
            "00000", "00100", "00100", "11111", "00100", "00100", "00000",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        ':' => [
            "00000", "01100", "01100", "00000", "01100", "01100", "00000",
        ],
        ' ' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "00000",
        ],
        _ => return None,
    })
}

/// Rendered width of a string in the 3x5 face, in buffer pixels, without the
/// trailing tracking. Counts `char`s, not bytes, and counts characters with
/// no glyph too, matching how the canvas advances. Use it to centre or
/// right-align text before drawing.
pub fn small_width(s: &str) -> usize {
    let n = s.chars().count();
    if n == 0 {
        0
    } else {
        n * SMALL_ADVANCE - (SMALL_ADVANCE - SMALL_W)
    }
}

/// Rendered width of a string in the 5x7 face, in buffer pixels, on the same
/// terms as [`small_width`].
pub fn big_width(s: &str) -> usize {
    let n = s.chars().count();
    if n == 0 {
        0
    } else {
        n * BIG_ADVANCE - (BIG_ADVANCE - BIG_W)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_the_declared_size() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,+-:/'!?><()\u{b7}".chars() {
            let g = small_glyph(c).unwrap_or_else(|| panic!("no 3x5 glyph for {c:?}"));
            assert_eq!(g.len(), SMALL_H);
            for row in g {
                assert_eq!(row.len(), SMALL_W, "{c:?} row width");
                assert!(row.bytes().all(|b| b == b'0' || b == b'1'));
            }
        }
        for c in "0123456789+-.: ".chars() {
            let g = big_glyph(c).unwrap_or_else(|| panic!("no 5x7 glyph for {c:?}"));
            assert_eq!(g.len(), BIG_H);
            for row in g {
                assert_eq!(row.len(), BIG_W, "{c:?} row width");
                assert!(row.bytes().all(|b| b == b'0' || b == b'1'));
            }
        }
    }

    #[test]
    fn letters_are_distinguishable() {
        // `gen_sprites.py` gives M and N the same 3x5 glyph. At this size a
        // duplicate glyph is a misspelling on screen.
        let mut seen: Vec<(char, [&str; SMALL_H])> = Vec::new();
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            let g = small_glyph(c).unwrap();
            if let Some((other, _)) = seen.iter().find(|(_, h)| *h == g) {
                panic!("{c:?} and {other:?} share a glyph");
            }
            seen.push((c, g));
        }
    }

    #[test]
    fn widths_account_for_tracking_but_not_a_trailing_gap() {
        assert_eq!(small_width(""), 0);
        assert_eq!(small_width("A"), SMALL_W);
        assert_eq!(small_width("AB"), SMALL_W + SMALL_ADVANCE);
        assert_eq!(big_width("1"), BIG_W);
        assert_eq!(big_width("40"), BIG_W + BIG_ADVANCE);
    }
}

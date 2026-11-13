//! A 3x5 bitmap font, drawn at any integer scale. Upper case, digits and the
//! punctuation the game prints; lower case draws as upper case.

use crate::canvas::{Canvas, Rgba};

const GLYPHS: &[(char, &str)] = &[
    ('A', ".#.#.#####.##.#"),
    ('B', "##.#.###.#.###."),
    ('C', ".###..#..#...##"),
    ('D', "##.#.##.##.###."),
    ('E', "####..##.#..###"),
    ('F', "####..##.#..#.."),
    ('G', ".###..#.##.#.##"),
    ('H', "#.##.#####.##.#"),
    ('I', "###.#..#..#.###"),
    ('J', "..#..#..##.#.#."),
    ('K', "#.##.###.#.##.#"),
    ('L', "#..#..#..#..###"),
    ('M', "#.########.##.#"),
    ('N', "##.#.##.##.##.#"),
    ('O', ".#.#.##.##.#.#."),
    ('P', "##.#.###.#..#.."),
    ('Q', ".#.#.##.###..##"),
    ('R', "##.#.###.#.##.#"),
    ('S', ".###...#...###."),
    ('T', "###.#..#..#..#."),
    ('U', "#.##.##.##.####"),
    ('V', "#.##.##.##.#.#."),
    ('W', "#.##.########.#"),
    ('X', "#.##.#.#.#.##.#"),
    ('Y', "#.##.#.#..#..#."),
    ('Z', "###..#.#.#..###"),
    ('0', "####.##.##.####"),
    ('1', ".#.##..#..#.###"),
    ('2', "##...#.#.#..###"),
    ('3', "##...#.#...###."),
    ('4', "#.##.####..#..#"),
    ('5', "####..##...###."),
    ('6', ".###..####.####"),
    ('7', "###..#.#..#..#."),
    ('8', "####.#####.####"),
    ('9', "####.####..###."),
    ('.', ".............#."),
    (',', "..........#.#.."),
    (':', "....#.....#...."),
    ('/', "..#..#.#.#..#.."),
    ('-', "......###......"),
    ('+', "....#.###.#...."),
    ('!', ".#..#..#.....#."),
    ('?', "##...#.#.....#."),
    ('%', "#.#..#.#.#..#.#"),
    ('(', ".#.#..#..#...#."),
    (')', ".#...#..#..#.#."),
    ('\'', ".#..#.........."),
    ('>', "#...#...#.#.#.."),
    ('<', "..#.#.#...#...#"),
    ('=', "...###...###..."),
    ('[', "##.#..#..#..##."),
    (']', ".##..#..#..#.##"),
    ('#', "#.#####.#####.#"),
];

fn glyph(ch: char) -> Option<&'static str> {
    let ch = ch.to_ascii_uppercase();
    GLYPHS.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g)
}

/// Pixel width of `s` at `scale`, with one column between glyphs.
pub fn width(s: &str, scale: i32) -> i32 {
    let n = s.chars().count() as i32;
    if n == 0 {
        0
    } else {
        (n * 4 - 1) * scale
    }
}

/// Draws `s` with its top-left at (x, y). Returns the x after the last glyph.
pub fn text(c: &mut Canvas, s: &str, x: i32, y: i32, scale: i32, col: Rgba) -> i32 {
    let mut cx = x;
    for ch in s.chars() {
        if let Some(g) = glyph(ch) {
            let cells: Vec<bool> = g.chars().map(|b| b == '#').collect();
            for row in 0..5 {
                for col_i in 0..3 {
                    let k = row * 3 + col_i;
                    if cells.get(k).copied().unwrap_or(false) {
                        c.rect(
                            cx + col_i as i32 * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            col,
                        );
                    }
                }
            }
        }
        cx += 4 * scale;
    }
    cx
}

/// Draws `s` with a one-pixel dark outline, for text over busy art.
pub fn text_outlined(c: &mut Canvas, s: &str, x: i32, y: i32, scale: i32, col: Rgba, edge: Rgba) {
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        text(c, s, x + dx, y + dy, scale, edge);
    }
    text(c, s, x, y, scale, col);
}

/// `s` centred on `cx`.
pub fn centred(c: &mut Canvas, s: &str, cx: i32, y: i32, scale: i32, col: Rgba) {
    text(c, s, cx - width(s, scale) / 2, y, scale, col);
}

/// Splits `s` into lines no wider than `max_px` at `scale`.
pub fn wrap(s: &str, max_px: i32, scale: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let cand = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if width(&cand, scale) > max_px && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

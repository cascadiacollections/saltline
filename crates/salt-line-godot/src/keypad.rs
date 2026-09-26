//! Engine-free layout of the on-screen seed keypad, and a focus that a D-pad
//! moves over it, so a controller can type a seed without a keyboard.
//!
//! Each cell's name is also its touch action: a letter or digit types itself,
//! `del` deletes, `confirm` starts the run and `back` leaves.

/// Every key that types a character, in keypad order.
pub const KEYS: [&str; 36] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

pub const DEL: &str = "del";

/// The keypad's rows as drawn: three rows of keys (the digits end in DEL),
/// then the START and BACK lines beneath.
pub const ROWS: [&[&str]; 5] = [
    &[
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    ],
    &[
        "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    ],
    &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", DEL],
    &["confirm"],
    &["back"],
];

/// Rows of `ROWS` that are keys, rather than menu lines.
pub const KEY_ROWS: usize = 3;

/// Which cell a controller is on. The column is remembered across short rows,
/// so going down onto START and back up returns to the key it left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    row: usize,
    col: usize,
}

impl Focus {
    fn col_in_row(self) -> usize {
        self.col.min(ROWS[self.row].len() - 1)
    }

    /// The action of the focused cell.
    pub fn action(self) -> &'static str {
        ROWS[self.row][self.col_in_row()]
    }

    /// Move by whole cells. Left and right wrap within a row; up and down stop
    /// at the top and bottom.
    pub fn step(&mut self, dx: i32, dy: i32) {
        if dx != 0 {
            let len = ROWS[self.row].len() as i32;
            self.col = (self.col_in_row() as i32 + dx).rem_euclid(len) as usize;
        }
        if dy != 0 {
            self.row = (self.row as i32 + dy).clamp(0, ROWS.len() as i32 - 1) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_rows_hold_every_key_in_order_then_del() {
        let keys: Vec<&str> = ROWS[..KEY_ROWS]
            .iter()
            .flat_map(|r| r.iter().copied())
            .filter(|k| *k != DEL)
            .collect();
        assert_eq!(keys, KEYS);
        assert_eq!(ROWS[KEY_ROWS - 1].last(), Some(&DEL));
    }

    #[test]
    fn focus_starts_on_a_and_wraps_sideways() {
        let mut f = Focus::default();
        assert_eq!(f.action(), "A");
        f.step(-1, 0);
        assert_eq!(f.action(), "M");
        f.step(1, 0);
        assert_eq!(f.action(), "A");
    }

    #[test]
    fn focus_keeps_its_column_through_short_rows() {
        let mut f = Focus::default();
        f.step(12, 0);
        assert_eq!(f.action(), "M");
        f.step(0, 1);
        assert_eq!(f.action(), "Z");
        f.step(0, 1);
        assert_eq!(f.action(), DEL);
        f.step(0, 1);
        assert_eq!(f.action(), "confirm");
        f.step(0, 1);
        assert_eq!(f.action(), "back");
        f.step(0, 1);
        assert_eq!(f.action(), "back");
        f.step(0, -4);
        assert_eq!(f.action(), "M");
        f.step(0, -1);
        assert_eq!(f.action(), "M");
    }

    #[test]
    fn sideways_from_a_short_row_moves_from_where_it_shows() {
        let mut f = Focus::default();
        f.step(12, 2);
        assert_eq!(f.action(), DEL);
        f.step(-1, 0);
        assert_eq!(f.action(), "9");
    }
}

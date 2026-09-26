//! Engine-free layout of the on-screen seed keypad, and a focus that a D-pad
//! moves over it, so a controller can type a seed without a keyboard.
//!
//! Each cell is also its touch action: a letter or digit types itself, DEL
//! deletes, START (`Confirm`) starts the run and BACK leaves.

use crate::input::Action;

/// Every key that types a character, in keypad order.
pub const KEYS: [Action; 36] = [
    Action::Type('A'),
    Action::Type('B'),
    Action::Type('C'),
    Action::Type('D'),
    Action::Type('E'),
    Action::Type('F'),
    Action::Type('G'),
    Action::Type('H'),
    Action::Type('I'),
    Action::Type('J'),
    Action::Type('K'),
    Action::Type('L'),
    Action::Type('M'),
    Action::Type('N'),
    Action::Type('O'),
    Action::Type('P'),
    Action::Type('Q'),
    Action::Type('R'),
    Action::Type('S'),
    Action::Type('T'),
    Action::Type('U'),
    Action::Type('V'),
    Action::Type('W'),
    Action::Type('X'),
    Action::Type('Y'),
    Action::Type('Z'),
    Action::Type('0'),
    Action::Type('1'),
    Action::Type('2'),
    Action::Type('3'),
    Action::Type('4'),
    Action::Type('5'),
    Action::Type('6'),
    Action::Type('7'),
    Action::Type('8'),
    Action::Type('9'),
];

pub const DEL: Action = Action::Delete;

/// The keypad's rows as drawn: three rows of keys (the digits end in DEL),
/// then the START and BACK lines beneath.
pub const ROWS: [&[Action]; 5] = [
    &[
        Action::Type('A'),
        Action::Type('B'),
        Action::Type('C'),
        Action::Type('D'),
        Action::Type('E'),
        Action::Type('F'),
        Action::Type('G'),
        Action::Type('H'),
        Action::Type('I'),
        Action::Type('J'),
        Action::Type('K'),
        Action::Type('L'),
        Action::Type('M'),
    ],
    &[
        Action::Type('N'),
        Action::Type('O'),
        Action::Type('P'),
        Action::Type('Q'),
        Action::Type('R'),
        Action::Type('S'),
        Action::Type('T'),
        Action::Type('U'),
        Action::Type('V'),
        Action::Type('W'),
        Action::Type('X'),
        Action::Type('Y'),
        Action::Type('Z'),
    ],
    &[
        Action::Type('0'),
        Action::Type('1'),
        Action::Type('2'),
        Action::Type('3'),
        Action::Type('4'),
        Action::Type('5'),
        Action::Type('6'),
        Action::Type('7'),
        Action::Type('8'),
        Action::Type('9'),
        DEL,
    ],
    &[Action::Confirm],
    &[Action::Back],
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
    pub fn action(self) -> Action {
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
        let keys: Vec<Action> = ROWS[..KEY_ROWS]
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
        assert_eq!(f.action(), Action::Type('A'));
        f.step(-1, 0);
        assert_eq!(f.action(), Action::Type('M'));
        f.step(1, 0);
        assert_eq!(f.action(), Action::Type('A'));
    }

    #[test]
    fn focus_keeps_its_column_through_short_rows() {
        let mut f = Focus::default();
        f.step(12, 0);
        assert_eq!(f.action(), Action::Type('M'));
        f.step(0, 1);
        assert_eq!(f.action(), Action::Type('Z'));
        f.step(0, 1);
        assert_eq!(f.action(), DEL);
        f.step(0, 1);
        assert_eq!(f.action(), Action::Confirm);
        f.step(0, 1);
        assert_eq!(f.action(), Action::Back);
        f.step(0, 1);
        assert_eq!(f.action(), Action::Back);
        f.step(0, -4);
        assert_eq!(f.action(), Action::Type('M'));
        f.step(0, -1);
        assert_eq!(f.action(), Action::Type('M'));
    }

    #[test]
    fn sideways_from_a_short_row_moves_from_where_it_shows() {
        let mut f = Focus::default();
        f.step(12, 2);
        assert_eq!(f.action(), DEL);
        f.step(-1, 0);
        assert_eq!(f.action(), Action::Type('9'));
    }
}

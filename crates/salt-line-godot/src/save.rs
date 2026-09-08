//! Local persistence: the unlock flag and the leaderboard. Brief M6.
//!
//! Plain text under `user://`, one record per line. Small enough to read by
//! hand, which matters when a player reports a score that looks wrong.

use godot::classes::file_access::ModeFlags;
use godot::classes::FileAccess;
use godot::prelude::*;

pub const SAVE_PATH: &str = "user://salt_line.txt";
/// Leaderboard depth kept on disk.
pub const KEEP: usize = 20;

/// One finished run. Score is screens cleared, tiebreak shells then shots
/// (DESIGN §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    pub seed: u64,
    pub screens: u32,
    pub shells: i32,
    pub shots: u32,
    pub daily: bool,
}

impl Entry {
    /// Sort key: more screens, then more shells left, then fewer shots.
    fn rank(&self) -> (u32, i32, i64) {
        (self.screens, self.shells, -(self.shots as i64))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Save {
    pub unlocked: bool,
    pub scores: Vec<Entry>,
}

impl Save {
    pub fn load() -> Save {
        let mut save = Save::default();
        let Some(f) = FileAccess::open(SAVE_PATH, ModeFlags::READ) else {
            return save;
        };
        let text = f.get_as_text().to_string();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("unlocked") => save.unlocked = it.next() == Some("1"),
                Some("run") => {
                    let parts: Vec<&str> = it.collect();
                    if parts.len() >= 5 {
                        if let (Ok(seed), Ok(screens), Ok(shells), Ok(shots)) = (
                            parts[0].parse::<u64>(),
                            parts[1].parse::<u32>(),
                            parts[2].parse::<i32>(),
                            parts[3].parse::<u32>(),
                        ) {
                            save.scores.push(Entry {
                                seed,
                                screens,
                                shells,
                                shots,
                                daily: parts[4] == "1",
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        save.sort();
        save
    }

    fn sort(&mut self) {
        self.scores.sort_by_key(|e| std::cmp::Reverse(e.rank()));
        self.scores.truncate(KEEP);
    }

    pub fn store(&self) {
        let Some(mut f) = FileAccess::open(SAVE_PATH, ModeFlags::WRITE) else {
            godot_warn!("salt-line: could not write {SAVE_PATH}");
            return;
        };
        let flag = format!("unlocked {}", if self.unlocked { 1 } else { 0 });
        let line = GString::from(flag.as_str());
        f.store_line(&line);
        for e in &self.scores {
            let record = format!(
                "run {} {} {} {} {}",
                e.seed,
                e.screens,
                e.shells,
                e.shots,
                if e.daily { 1 } else { 0 }
            );
            f.store_line(&GString::from(record.as_str()));
        }
    }

    /// Record a finished run and persist. Returns its position on the board,
    /// or `None` if it did not make the cut.
    pub fn record(&mut self, entry: Entry) -> Option<usize> {
        self.scores.push(entry);
        self.sort();
        let at = self.scores.iter().position(|e| *e == entry);
        self.store();
        at
    }

    pub fn best(&self) -> Option<&Entry> {
        self.scores.first()
    }

    /// Best score on a specific seed — what a shared seed is judged against.
    pub fn best_on(&self, seed: u64) -> Option<&Entry> {
        self.scores.iter().find(|e| e.seed == seed)
    }
}

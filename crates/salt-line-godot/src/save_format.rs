//! The on-disk save format, with no engine in it.
//!
//! Parsing and serialising live here so both can be tested headless. `save.rs`
//! is only the file IO around them. The format is still plain text, one record
//! per line, small enough to read by hand when a player reports a score that
//! looks wrong.
//!
//! ```text
//! version 2
//! unlocked 0
//! volume 75
//! muted 0
//! learned 0
//! bind fire SPACE A
//! run <seed> <screens> <shells> <shots> <daily>
//! ```

/// Format version written by this build.
pub const VERSION: u32 = 2;
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

/// How touch aims: on-screen buttons, or pulling back on the playfield.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TouchAim {
    Buttons,
    Drag,
}

impl TouchAim {
    pub fn token(self) -> &'static str {
        match self {
            Self::Buttons => "buttons",
            Self::Drag => "drag",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "buttons" => Some(Self::Buttons),
            "drag" => Some(Self::Drag),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Buttons => Self::Drag,
            Self::Drag => Self::Buttons,
        }
    }
}

/// How much of the trajectory the drag aim previews.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArcHint {
    /// The first stretch of the still-air arc: direction and power, never the
    /// landing. Reading the wind stays the player's job (DESIGN 2.5).
    Short,
    /// The whole arc in still air.
    Full,
    /// The whole arc with the wind as displayed.
    Wind,
}

impl ArcHint {
    pub fn token(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Full => "full",
            Self::Wind => "wind",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "short" => Some(Self::Short),
            "full" => Some(Self::Full),
            "wind" => Some(Self::Wind),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Short => Self::Full,
            Self::Full => Self::Wind,
            Self::Wind => Self::Short,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save {
    pub unlocked: bool,
    pub scores: Vec<Entry>,
    /// Master effects volume, as an integer percentage for a stable text
    /// representation.
    pub volume: u8,
    pub muted: bool,
    /// Bitset of contextual lessons the player has already demonstrated.
    pub learned: u32,
    /// User-editable action bindings: action, keyboard token, pad token.
    /// Missing actions fall back to the built-in defaults.
    pub bindings: Vec<(String, String, String)>,
    /// Touch settings. `None` until the player picks one, so the built-in
    /// default can change without overriding anyone's choice.
    pub touch_aim: Option<TouchAim>,
    pub arc_hint: Option<ArcHint>,
    /// Keep the last shot's arc drawn until the next one. `None` until the
    /// player toggles it, like the touch settings.
    pub last_arc: Option<bool>,
    /// The file on disk was written by a newer build than this one. The save
    /// is treated as empty and, more importantly, is never overwritten — a
    /// player who downgrades for one session should not lose their board.
    pub from_the_future: bool,
}

impl Default for Save {
    fn default() -> Self {
        Self {
            unlocked: false,
            scores: Vec::new(),
            volume: 75,
            muted: false,
            learned: 0,
            bindings: Vec::new(),
            touch_aim: None,
            arc_hint: None,
            last_arc: None,
            from_the_future: false,
        }
    }
}

impl Save {
    /// Parse a save file. Unparsable lines are skipped rather than fatal:
    /// a truncated write should cost the last record, not the whole board.
    pub fn parse(text: &str) -> Save {
        let mut save = Save::default();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("version") => {
                    // An unreadable version is a future one by assumption; the
                    // conservative reading is the one that preserves data.
                    let v = it.next().and_then(|s| s.parse::<u32>().ok());
                    if v.map(|v| v > VERSION).unwrap_or(true) {
                        return Save {
                            from_the_future: true,
                            ..Save::default()
                        };
                    }
                }
                Some("unlocked") => save.unlocked = it.next() == Some("1"),
                Some("volume") => {
                    if let Some(v) = it.next().and_then(|s| s.parse::<u8>().ok()) {
                        save.volume = v.min(100);
                    }
                }
                Some("muted") => save.muted = it.next() == Some("1"),
                Some("touch_aim") => save.touch_aim = it.next().and_then(TouchAim::from_token),
                Some("arc_hint") => save.arc_hint = it.next().and_then(ArcHint::from_token),
                Some("last_arc") => {
                    save.last_arc = match it.next() {
                        Some("1") => Some(true),
                        Some("0") => Some(false),
                        _ => None,
                    }
                }
                Some("learned") => {
                    if let Some(v) = it.next().and_then(|s| s.parse::<u32>().ok()) {
                        save.learned = v;
                    }
                }
                Some("bind") => {
                    let parts: Vec<&str> = it.collect();
                    if parts.len() == 3
                        && parts
                            .iter()
                            .all(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    {
                        save.bindings.push((
                            parts[0].to_ascii_lowercase(),
                            parts[1].to_ascii_uppercase(),
                            parts[2].to_ascii_uppercase(),
                        ));
                    }
                }
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
                // Unknown keys are ignored so a later version can add one
                // without this build discarding the rest of the file.
                _ => {}
            }
        }
        save.sort();
        save
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("version {VERSION}\n"));
        out.push_str(&format!("unlocked {}\n", if self.unlocked { 1 } else { 0 }));
        out.push_str(&format!("volume {}\n", self.volume.min(100)));
        out.push_str(&format!("muted {}\n", if self.muted { 1 } else { 0 }));
        out.push_str(&format!("learned {}\n", self.learned));
        if let Some(a) = self.touch_aim {
            out.push_str(&format!("touch_aim {}\n", a.token()));
        }
        if let Some(h) = self.arc_hint {
            out.push_str(&format!("arc_hint {}\n", h.token()));
        }
        if let Some(on) = self.last_arc {
            out.push_str(&format!("last_arc {}\n", on as u8));
        }
        for (action, key, pad) in &self.bindings {
            out.push_str(&format!("bind {action} {key} {pad}\n"));
        }
        for e in &self.scores {
            out.push_str(&format!(
                "run {} {} {} {} {}\n",
                e.seed,
                e.screens,
                e.shells,
                e.shots,
                if e.daily { 1 } else { 0 }
            ));
        }
        out
    }

    pub(crate) fn sort(&mut self) {
        self.scores.sort_by_key(|e| std::cmp::Reverse(e.rank()));
        self.scores.truncate(KEEP);
    }

    /// Place a finished run on the board. Returns its position, or `None` if
    /// it did not make the cut. Persisting is the caller's job.
    pub fn place(&mut self, entry: Entry) -> Option<usize> {
        self.scores.push(entry);
        self.sort();
        self.scores.iter().position(|e| *e == entry)
    }

    pub fn best(&self) -> Option<&Entry> {
        self.scores.first()
    }

    /// Best score on a specific seed — what a shared seed is judged against.
    pub fn best_on(&self, seed: u64) -> Option<&Entry> {
        self.scores.iter().find(|e| e.seed == seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(screens: u32, shells: i32, shots: u32) -> Entry {
        Entry {
            seed: 7,
            screens,
            shells,
            shots,
            daily: false,
        }
    }

    #[test]
    fn a_save_survives_a_round_trip() {
        let mut s = Save {
            unlocked: true,
            ..Save::default()
        };
        s.place(entry(9, 3, 40));
        s.place(Entry {
            seed: 12,
            screens: 4,
            shells: 1,
            shots: 20,
            daily: true,
        });
        let back = Save::parse(&s.serialize());
        assert_eq!(back.unlocked, s.unlocked);
        assert_eq!(back.scores, s.scores);
        assert_eq!(back.volume, 75);
    }

    #[test]
    fn the_board_ranks_by_screens_then_shells_then_shots() {
        let mut s = Save::default();
        s.place(entry(5, 0, 10));
        s.place(entry(5, 4, 30)); // more shells wins the tiebreak
        s.place(entry(5, 4, 12)); // fewer shots wins the next one
        s.place(entry(6, 0, 99)); // more screens beats all of it
        let got: Vec<(u32, i32, u32)> = s
            .scores
            .iter()
            .map(|e| (e.screens, e.shells, e.shots))
            .collect();
        assert_eq!(got, vec![(6, 0, 99), (5, 4, 12), (5, 4, 30), (5, 0, 10)]);
    }

    #[test]
    fn the_board_is_truncated_to_keep() {
        let mut s = Save::default();
        for i in 0..(KEEP as u32 + 15) {
            s.place(entry(i, 0, 0));
        }
        assert_eq!(s.scores.len(), KEEP);
        // Truncation drops the worst, not the newest.
        assert_eq!(s.best().unwrap().screens, KEEP as u32 + 14);
        assert_eq!(s.scores.last().unwrap().screens, 15);
    }

    #[test]
    fn a_run_that_missed_the_cut_reports_no_position() {
        let mut s = Save::default();
        for i in 0..KEEP as u32 {
            s.place(entry(100 + i, 0, 0));
        }
        assert_eq!(s.place(entry(1, 0, 0)), None);
        assert_eq!(s.scores.len(), KEEP);
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let text = "version 1\n\
                    unlocked 1\n\
                    run 1 2 3 4 0\n\
                    run not-a-number 2 3 4 0\n\
                    run 9 9\n\
                    run\n\
                    garbage here\n\
                    \n\
                    run 5 6 7 8 1\n";
        let s = Save::parse(text);
        assert!(s.unlocked);
        assert_eq!(s.scores.len(), 2);
        assert_eq!(s.scores[0].screens, 6);
    }

    #[test]
    fn a_truncated_final_line_costs_only_that_record() {
        let mut full = Save::default();
        full.place(entry(3, 1, 5));
        full.place(entry(8, 2, 9));
        let text = full.serialize();
        let cut = &text[..text.len() - 6];
        let s = Save::parse(cut);
        assert_eq!(s.scores.len(), 1);
        assert_eq!(s.scores[0].screens, 8);
    }

    #[test]
    fn a_version_one_file_reads_the_same_as_a_versionless_one() {
        let legacy = "unlocked 1\nrun 4 5 6 7 1\n";
        let current = "version 1\nunlocked 1\nrun 4 5 6 7 1\n";
        assert_eq!(Save::parse(legacy).scores, Save::parse(current).scores);
        assert!(Save::parse(legacy).unlocked);
    }

    #[test]
    fn a_future_version_is_read_as_empty_and_flagged() {
        for text in ["version 3\nunlocked 1\nrun 4 5 6 7 1\n", "version x\n"] {
            let s = Save::parse(text);
            assert!(s.from_the_future, "{text:?}");
            assert!(s.scores.is_empty());
            assert!(!s.unlocked);
        }
    }

    #[test]
    fn an_absent_or_empty_file_is_an_empty_board() {
        let s = Save::parse("");
        assert_eq!(s, Save::default());
        assert!(s.best().is_none());
        assert!(s.best_on(7).is_none());
    }

    #[test]
    fn best_on_finds_the_top_entry_for_a_seed() {
        let mut s = Save::default();
        s.place(Entry {
            seed: 42,
            screens: 3,
            shells: 0,
            shots: 9,
            daily: false,
        });
        s.place(Entry {
            seed: 42,
            screens: 11,
            shells: 0,
            shots: 9,
            daily: false,
        });
        s.place(entry(20, 0, 1));
        assert_eq!(s.best_on(42).unwrap().screens, 11);
        assert!(s.best_on(999).is_none());
    }

    #[test]
    fn the_serialised_header_is_the_current_version() {
        let s = Save::default();
        assert_eq!(s.serialize().lines().next(), Some("version 2"));
    }

    #[test]
    fn version_one_and_versionless_files_gain_safe_setting_defaults() {
        for text in ["version 1\nunlocked 1\n", "unlocked 1\n"] {
            let s = Save::parse(text);
            assert_eq!(s.volume, 75);
            assert!(!s.muted);
            assert_eq!(s.learned, 0);
            assert!(s.bindings.is_empty());
        }
    }

    #[test]
    fn settings_and_bindings_round_trip_and_malformed_values_are_safe() {
        let text = "version 2\nvolume 240\nmuted 1\nlearned 37\n\
                    bind fire F X\nbind bad-token ! A\n";
        let s = Save::parse(text);
        assert_eq!(s.volume, 100);
        assert!(s.muted);
        assert_eq!(s.learned, 37);
        assert_eq!(s.bindings, vec![("fire".into(), "F".into(), "X".into())]);
        assert_eq!(Save::parse(&s.serialize()), s);
    }

    #[test]
    fn touch_settings_are_saved_only_once_chosen() {
        let fresh = Save::default().serialize();
        assert!(!fresh.contains("touch_aim") && !fresh.contains("arc_hint"));

        let chosen = Save {
            touch_aim: Some(TouchAim::Drag),
            arc_hint: Some(ArcHint::Wind),
            ..Save::default()
        };
        let back = Save::parse(&chosen.serialize());
        assert_eq!(back.touch_aim, Some(TouchAim::Drag));
        assert_eq!(back.arc_hint, Some(ArcHint::Wind));

        let junk = Save::parse("touch_aim sideways\narc_hint everything\n");
        assert_eq!((junk.touch_aim, junk.arc_hint), (None, None));
    }

    #[test]
    fn the_last_arc_setting_is_saved_only_once_toggled() {
        assert!(!Save::default().serialize().contains("last_arc"));
        for on in [true, false] {
            let chosen = Save {
                last_arc: Some(on),
                ..Save::default()
            };
            assert_eq!(Save::parse(&chosen.serialize()).last_arc, Some(on));
        }
        assert_eq!(Save::parse("last_arc maybe\n").last_arc, None);
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        // Bindings as the parser normalizes them: lowercase actions, uppercase keys.
        fn action() -> impl Strategy<Value = String> {
            "[a-z0-9_]{1,12}"
        }

        fn token() -> impl Strategy<Value = String> {
            "[A-Z0-9_]{1,12}"
        }

        fn saves() -> impl Strategy<Value = Save> {
            let entry = (
                any::<u64>(),
                any::<u32>(),
                any::<i32>(),
                any::<u32>(),
                any::<bool>(),
            )
                .prop_map(|(seed, screens, shells, shots, daily)| Entry {
                    seed,
                    screens,
                    shells,
                    shots,
                    daily,
                });
            let aim = prop::option::of(prop_oneof![Just(TouchAim::Buttons), Just(TouchAim::Drag)]);
            let hint = prop::option::of(prop_oneof![
                Just(ArcHint::Short),
                Just(ArcHint::Full),
                Just(ArcHint::Wind)
            ]);
            (
                (
                    any::<bool>(),
                    prop::collection::vec(entry, 0..20),
                    any::<u8>(),
                    any::<bool>(),
                ),
                (
                    any::<u32>(),
                    prop::collection::vec((action(), token(), token()), 0..6),
                ),
                (aim, hint, prop::option::of(any::<bool>())),
            )
                .prop_map(
                    |(
                        (unlocked, scores, volume, muted),
                        (learned, bindings),
                        (touch_aim, arc_hint, last_arc),
                    )| Save {
                        unlocked,
                        scores,
                        volume,
                        muted,
                        learned,
                        bindings,
                        touch_aim,
                        arc_hint,
                        last_arc,
                        ..Save::default()
                    },
                )
        }

        proptest! {
            #[test]
            fn any_text_parses_without_panicking(text in "\\PC*") {
                let _ = Save::parse(&text);
            }

            #[test]
            fn any_line_soup_parses_without_panicking(
                lines in prop::collection::vec("(version|unlocked|volume|muted|learned|touch_aim|arc_hint|last_arc|bind|run)( [^\\n ]{0,24}){0,6}", 0..30)
            ) {
                let _ = Save::parse(&lines.join("\n"));
            }

            #[test]
            fn a_save_reads_back_as_written(save in saves()) {
                // What a save should come back as: volume capped, board ranked and cut.
                let mut expected = save.clone();
                expected.volume = expected.volume.min(100);
                expected.sort();
                let back = Save::parse(&save.serialize());
                prop_assert_eq!(&back, &expected);
                // And writing it again changes nothing.
                prop_assert_eq!(Save::parse(&back.serialize()), back);
            }
        }
    }
}

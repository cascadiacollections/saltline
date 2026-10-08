//! The slot file and the draft line.

use super::*;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use crate::custom::fork_duel;

fn code(seed: u64) -> String {
    fork_duel(seed, 0).to_code().unwrap()
}

#[test]
fn slots_round_trip_in_slot_order() {
    let mut s = Slots::default();
    s.put(SlotEntry {
        slot: 7,
        code: code(1),
        origin: None,
        tail: None,
    });
    s.put(SlotEntry {
        slot: 2,
        code: code(2),
        origin: Some(Origin::Seed {
            text: "DUNE".into(),
            round: 1,
        }),
        tail: None,
    });
    let text = s.serialize();
    assert!(text.starts_with("layouts 1\nlayout 2 SL"), "{text}");
    assert!(text.contains(" seed:DUNE:1\nlayout 7 SL"), "{text}");
    assert_eq!(Slots::parse(&text), s);
    assert!(sound(&text));
    assert_eq!(s.free(), Some(1));
    assert_eq!(s.get(2).unwrap().layout(), Ok(fork_duel(2, 0)));
    // Replacing a slot hands back what was there.
    let old = s.put(SlotEntry {
        slot: 2,
        code: code(3),
        origin: None,
        tail: None,
    });
    assert_eq!(old.map(|e| e.code), Some(code(2)));
    assert_eq!(s.entries.len(), 2);
    assert_eq!(s.remove(7).map(|e| e.slot), Some(7));
    assert_eq!(s.remove(7), None);
}

/// Nothing a player made is thrown away by a build that does not
/// understand it (§13.8): unreadable lines come back verbatim, a code in a
/// newer format keeps its slot, and a newer file is never written.
#[test]
fn what_this_build_cannot_read_is_kept() {
    let newer_code = "SLZZZZZZZZ";
    let text = format!(
        "layouts 1\nlayout 3 {newer_code}\nlayout 3 {}\nchallenge 1 SL00\nlayout 99 {}\nlayout 4 {} meadow green\nlayout 6 banana\n",
        code(1),
        code(2),
        code(3),
    );
    let s = Slots::parse(&text);
    assert_eq!(s.entries.len(), 2);
    assert_eq!(s.get(3).unwrap().code, newer_code);
    assert!(s.get(3).unwrap().layout().is_err(), "kept, shown dimmed");
    // An origin this build cannot read keeps the layout, its words kept.
    let four = s.get(4).unwrap();
    assert_eq!(
        (four.origin.as_ref(), four.tail.as_deref()),
        (None, Some("meadow green"))
    );
    assert_eq!(s.kept.len(), 4, "{:?}", s.kept);
    assert!(!s.from_the_future);
    let again = Slots::parse(&s.serialize());
    assert_eq!(again, s);
    assert!(s.serialize().contains(" meadow green\n"));
    // A kept line that names a slot holds it: a save never sits beside it.
    assert!(s.taken(6));
    assert_eq!(s.free(), Some(1));
    let mut full = Slots::parse(&text);
    for n in [1, 2, 5] {
        full.put(SlotEntry {
            slot: n,
            code: code(n as u64),
            origin: None,
            tail: None,
        });
    }
    assert_eq!(full.free(), Some(7), "3, 4 and 6 are taken");
    let future = Slots::parse("layouts 2\nlayout 1 SL00\n");
    assert!(future.from_the_future, "shown, never written");
    // A file with no header still gives up its slots.
    let bare = Slots::parse(&format!("layout 5 {}\n", code(4)));
    assert_eq!(bare.entries.len(), 1);
    assert!(!sound(&format!("layout 5 {}\n", code(4))));
    assert!(!sound("layouts 1\0\n"));
    assert!(Slots::parse("\0").damaged, "never written over");
    assert!(Slots::parse("\0").entries.is_empty());
}

#[test]
fn a_fingerprint_is_four_keypad_characters_and_tells_layouts_apart() {
    let a = fingerprint(&fork_duel(1, 0));
    let b = fingerprint(&fork_duel(2, 0));
    assert_eq!(a.len(), 4);
    assert!(a.bytes().all(|c| c.is_ascii_alphanumeric()));
    assert_ne!(a, b);
    assert_eq!(a, fingerprint(&fork_duel(1, 0)));
}

#[test]
fn a_draft_survives_its_line() {
    let d = SavedDraft {
        draft: Draft {
            layout: fork_duel(9, 2),
            from: Some(Origin::Match {
                seed: 123,
                round: 2,
                width: 400,
            }),
            slot: Some(5),
        },
        mirror: true,
    };
    let line = d.to_line().unwrap();
    assert!(line.starts_with("draft 1 1 5 match:123:2:400 SL"), "{line}");
    assert_eq!(SavedDraft::parse(&line), Ok(d.clone()));
    assert!(draft_sound(&line));
    let blank = SavedDraft {
        draft: Draft::blank(),
        mirror: false,
    };
    let line = blank.to_line().unwrap();
    assert!(line.starts_with("draft 1 0 - - SL"), "{line}");
    assert_eq!(SavedDraft::parse(&line), Ok(blank.clone()));
    // An origin a newer build wrote is dropped, the draft kept.
    let odd = line.replacen("draft 1 0 - -", "draft 1 0 - comet:7", 1);
    assert_eq!(SavedDraft::parse(&odd), Ok(blank));
}

#[rstest::rstest]
#[case::cut_short("draft 1 1 - - SL0")]
#[case::no_newline("draft 1 1 - -")]
#[case::wrong_tag("drift 1 1 - - SLX\n")]
#[case::mirror_not_a_flag("draft 1 2 - - SLX\n")]
#[case::slot_zero("draft 1 1 0 - SLX\n")]
#[case::slot_too_big("draft 1 1 33 - SLX\n")]
#[case::not_a_code("draft 1 1 - - banana\n")]
#[case::bad_checksum("draft 1 1 - - SL0000000\n")]
#[case::two_lines("draft 1 1 - - SLX\ndraft 1 1 - - SLX\n")]
#[case::nul("draft 1\0\n")]
fn a_damaged_draft_is_never_read(#[case] text: &str) {
    assert_eq!(SavedDraft::parse(text), Err(Stale::Damaged), "{text:?}");
    assert!(!draft_sound(text));
}

/// A draft holding a layout this editor cannot hold (one gun, another
/// width) is a later build's, kept for it like a slot, never deleted.
#[test]
fn a_draft_this_editor_cannot_hold_is_a_newer_builds() {
    let one_gun = salt_line_sim::layout::Layout::from_run_screen(1, 0);
    let mut wide = crate::custom::fork_duel(1, 0);
    wide.heights.push(wide.heights[0]);
    for l in [one_gun, wide] {
        let text = format!("draft 1 1 - - {}\n", l.to_code().unwrap());
        assert_eq!(SavedDraft::parse(&text), Err(Stale::NewerVersion));
        assert!(draft_sound(&text), "a whole copy, kept");
    }
}

/// Only a code format past this build's is a newer build's; one before it
/// is an older build's, and anything else damage.
#[test]
fn a_code_format_is_newer_only_if_it_is_past_this_builds() {
    use salt_line_sim::layout::LAYOUT_FORMAT;
    assert_eq!(
        code_stale(CodeError::Format(LAYOUT_FORMAT + 1)),
        Stale::NewerVersion
    );
    assert_eq!(
        code_stale(CodeError::Format(LAYOUT_FORMAT - 1)),
        Stale::OlderVersion
    );
    assert_eq!(code_stale(CodeError::Checksum), Stale::Damaged);
}

#[test]
fn another_draft_format_is_named() {
    let line = SavedDraft {
        draft: Draft::blank(),
        mirror: true,
    }
    .to_line()
    .unwrap();
    let newer = line.replacen("draft 1", "draft 2", 1);
    assert_eq!(SavedDraft::parse(&newer), Err(Stale::NewerVersion));
    assert!(draft_sound(&newer));
    let older = line.replacen("draft 1", "draft 0", 1);
    assert_eq!(SavedDraft::parse(&older), Err(Stale::OlderVersion));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the file holds, reading it, writing it and reading it again
    /// gives the same slots and the same kept lines: nothing is lost or
    /// changed by a round trip. The fuzz target
    /// (`fuzz/fuzz_targets/layouts_file.rs`) throws raw bytes at it too.
    #[test]
    fn any_slot_file_survives_a_round_trip(
        lines in prop::collection::vec(
            prop_oneof![
                3 => (1u8..=40, 0u64..6, prop::option::of(0u32..12)).prop_map(|(slot, seed, round)| {
                    let origin = round.map_or(String::new(), |r| format!(" seed:DUNE:{r}"));
                    format!("layout {slot} {}{origin}", code(seed))
                }),
                1 => "[ -~]{0,40}",
                1 => Just("layouts 1".to_string()),
            ],
            0..12,
        ),
    ) {
        let text = lines.join("\n");
        let s = Slots::parse(&text);
        let again = Slots::parse(&s.serialize());
        prop_assert_eq!(&again, &s);
        prop_assert!(s.entries.windows(2).all(|w| w[0].slot < w[1].slot));
        prop_assert!(s.entries.iter().all(|e| (1..=MAX_SLOTS).contains(&e.slot)));
    }
}

use super::*;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

/// A short high lob that lands well short of the other gun.
const LOB: (i32, i32) = (1400, 330);

/// A match played by a bot the way the game plays it: the first shot that
/// hits, except every third shot, which lobs short, and the next round
/// dealt after each round won. Stops once the match is won or `max` shots
/// are fired. Returns the log and the match.
fn bot_match(seed: u64, width: usize, rolling: bool, max: usize) -> (Vec<Entry>, Match) {
    let mut duel = Match::with_rules(seed, width, rules_for(rolling));
    let mut log = Vec::new();
    let mut fired = 0;
    while fired < max && !matches!(duel.phase, Phase::MatchOver(_)) {
        if matches!(duel.phase, Phase::RoundOver(_)) {
            duel.next_round();
            log.push(Entry::NextRound);
        }
        let side = duel.to_move;
        let aim = if fired % 3 == 2 {
            LOB
        } else {
            let frame = versus::frame_for(&duel.screen, side);
            let target = versus::target_in_frame(&duel.screen, side);
            versus::shots_at(&frame, target, &duel.loadout, duel.rules.hit_reach, 1)
                .first()
                .map_or(LOB, |s| (s.angle_ticks, s.power_ticks))
        };
        duel.fire(Shot::new(aim.0, aim.1), false).unwrap();
        log.push(Entry::Shot(aim.0, aim.1));
        fired += 1;
    }
    (log, duel)
}

/// The golden match: rolling guns, aim help on the left, played to its end.
fn golden() -> (SavedMatch, Match) {
    let (log, duel) = bot_match(20261007, 320, true, 200);
    let saved = SavedMatch {
        epoch: SIM_EPOCH,
        match_epoch: MATCH_EPOCH,
        seed: 20261007,
        width: 320,
        rolling: true,
        help: [true, false],
        log,
    };
    (saved, duel)
}

#[test]
fn the_line_format_is_pinned() {
    let (saved, _) = golden();
    insta::assert_snapshot!(saved.to_line());
}

/// The rules' epoch, enforced rather than remembered (DESIGN §9.3's
/// pattern): if this fails, a change to `crate::versus` changed what a
/// logged match replays to. Bump `MATCH_EPOCH` and set `MATCH_DIGEST` to
/// the new value, so old match files are thrown away with a note rather
/// than replayed as a different match.
#[test]
fn a_golden_match_still_replays_to_its_digest() {
    let (saved, live) = golden();
    // The golden match covers what the rules can do: a roll, a second
    // round, and a winner.
    assert!(matches!(live.phase, Phase::MatchOver(_)));
    assert!(live.round >= 1);
    let replayed = saved.replay().unwrap();
    assert_eq!(state_hash(&replayed), state_hash(&live));
    assert_eq!(
        state_hash(&replayed),
        MATCH_DIGEST,
        "the duel rules changed what a logged match replays to: bump MATCH_EPOCH \
         and set MATCH_DIGEST to {:#x}",
        state_hash(&replayed)
    );
}

#[test]
fn the_golden_match_rolls_its_guns() {
    let (saved, _) = golden();
    let mut duel = Match::with_rules(saved.seed, saved.width, saved.rules());
    let mut rolls = 0;
    for entry in &saved.log {
        match *entry {
            Entry::Shot(a, p) => {
                rolls += duel.fire(Shot::new(a, p), false).unwrap().rolled.is_some() as u32;
            }
            Entry::NextRound => assert!(duel.next_round()),
        }
    }
    assert!(rolls > 0, "the golden match never rolled its guns");
    assert!(saved.log.contains(&Entry::NextRound));
}

/// A round is only dealt where one was won, and a shot is only taken
/// while a round is open: either out of place is a log that does not
/// replay, thrown away rather than shown diverging.
#[test]
fn rounds_are_dealt_only_where_one_was_won() {
    let (saved, _) = golden();
    let first = saved
        .log
        .iter()
        .position(|e| *e == Entry::NextRound)
        .unwrap();
    // A shot fired over a round's result plate.
    let mut skipped = saved.clone();
    skipped.log.remove(first);
    assert!(skipped.replay().is_none());
    // A round dealt in the middle of one.
    let mut early = saved.clone();
    early.log.insert(1, Entry::NextRound);
    assert!(early.replay().is_none());
    assert!(matches!(
        judge(Some(&early.to_line())),
        Found::Discard(Stale::Diverged)
    ));
    // Stopped on the result plate, and just after the next round is dealt:
    // both resumable, one round apart.
    let mut on_result = saved.clone();
    on_result.log.truncate(first);
    let mut dealt = saved;
    dealt.log.truncate(first + 1);
    let (a, b) = (on_result.replay().unwrap(), dealt.replay().unwrap());
    assert!(matches!(a.phase, Phase::RoundOver(_)));
    assert_eq!((b.phase, b.round), (Phase::Aiming, a.round + 1));
}

#[test]
fn a_saved_match_round_trips_and_replays_to_the_match_as_played() {
    for (seed, width, rolling, n) in [
        (1, 320, true, 0),
        (2, 360, false, 5),
        (3, 400, true, 17),
        (4, 333, true, 40),
    ] {
        let (shots, live) = bot_match(seed, width, rolling, n);
        let saved = SavedMatch::of(&live, [rolling, !rolling], &shots);
        let back = SavedMatch::parse(&saved.to_line()).unwrap();
        assert_eq!(back, saved);
        assert_eq!(back.rules(), live.rules);
        let replayed = back.replay().unwrap();
        assert_eq!(state_hash(&replayed), state_hash(&live), "seed {seed}");
        // Bit for bit, not just by the hash: the whole state as text.
        assert_eq!(format!("{replayed:?}"), format!("{live:?}"));
    }
}

#[test]
fn judge_offers_a_match_in_play_or_just_won() {
    let (shots, live) = bot_match(9, 320, true, 7);
    let line = SavedMatch::of(&live, [false; 2], &shots).to_line();
    match judge(Some(&line)) {
        Found::Resume(saved) => assert_eq!(saved.log, shots),
        other => panic!("{other:?}"),
    }
    let (saved, _) = golden();
    // Won by its last shot: offered, to come back on its result plate.
    assert!(matches!(judge(Some(&saved.to_line())), Found::Resume(_)));
    assert!(matches!(judge(None), Found::Nothing));
    assert!(matches!(judge(Some(" \n")), Found::Nothing));
}

#[test]
fn a_shot_after_the_match_is_won_does_not_replay() {
    let (mut saved, _) = golden();
    saved.log.push(Entry::Shot(900, 600));
    assert_eq!(saved.replay().map(|_| ()), None);
    assert!(matches!(
        judge(Some(&saved.to_line())),
        Found::Discard(Stale::Diverged)
    ));
}

#[test]
fn another_format_or_epoch_is_never_replayed() {
    let (shots, live) = bot_match(5, 320, true, 4);
    let saved = SavedMatch::of(&live, [false; 2], &shots);
    let line = saved.to_line();
    let field = |i: usize, v: u32| {
        let mut f: Vec<String> = line.trim_end().split(' ').map(String::from).collect();
        f[i] = v.to_string();
        f.join(" ") + "\n"
    };
    for (i, now) in [(1, FORMAT), (2, SIM_EPOCH), (3, MATCH_EPOCH)] {
        assert_eq!(
            SavedMatch::parse(&field(i, now - 1)),
            Err(Stale::OlderVersion),
            "field {i}"
        );
        assert_eq!(
            SavedMatch::parse(&field(i, now + 1)),
            Err(Stale::NewerVersion),
            "field {i}"
        );
        // Whole, only not this build's: a sound copy, discarded with a note.
        assert!(sound(&field(i, now + 1)));
    }
    // The format decides before anything after it is read: another
    // layout's line may not even have these fields.
    let other = format!("match {} banana\n", FORMAT + 1);
    assert_eq!(SavedMatch::parse(&other), Err(Stale::NewerVersion));
    assert_eq!(
        note(Stale::OlderVersion),
        "SAVED MATCH FROM AN OLDER VERSION"
    );
    assert_eq!(
        note(Stale::NewerVersion),
        "SAVED MATCH FROM A NEWER VERSION"
    );
    assert_eq!(note(Stale::Damaged), "SAVED MATCH COULD NOT BE RESTORED");
}

#[rstest::rstest]
#[case::wrong_tag("resume 1 E M 7 320 1 0 0 0")]
#[case::run_history("run 1 E M 7 320 1 0 0 0")]
#[case::torn_header("match 1 E M 7 32")]
#[case::nul("match 1 E M 7 320 1 0 0 0\0\0")]
#[case::two_lines("match 1 E M 7 320 1 0 0 0\nmatch 1 E M 7 320 1 0 0 0\n")]
#[case::narrow("match 1 E M 7 100 1 0 0 0")]
#[case::wide("match 1 E M 7 4000 1 0 0 0")]
#[case::rolling_not_a_flag("match 1 E M 7 320 yes 0 0 0")]
#[case::help_not_a_flag("match 1 E M 7 320 1 2 0 0")]
#[case::seed_padded("match 1 E M 07 320 1 0 0 0")]
#[case::no_count("match 1 E M 7 320 1 0 0")]
#[case::count_short("match 1 E M 7 320 1 0 0 2 900:600")]
#[case::count_long("match 1 E M 7 320 1 0 0 1 900:600 870:640")]
#[case::bad_shot("match 1 E M 7 320 1 0 0 1 banana")]
#[case::half_a_shot("match 1 E M 7 320 1 0 0 1 900:")]
#[case::angle_out_of_range("match 1 E M 7 320 1 0 0 1 1900:600")]
#[case::negative_power("match 1 E M 7 320 1 0 0 1 900:-5")]
#[case::power_out_of_range("match 1 E M 7 320 1 0 0 1 900:1001")]
#[case::not_a_round_mark("match 1 E M 7 320 1 0 0 1 //")]
#[case::double_space("match 1 E M 7 320 1 0 0 1  900:600")]
#[case::too_many("match 1 E M 7 320 1 0 0 2001")]
fn damaged_lines_are_discarded(#[case] text: &str) {
    // Newline-terminated like a real line, so each is damaged for its own
    // reason.
    let text = text
        .replace(" E ", &format!(" {SIM_EPOCH} "))
        .replace(" M ", &format!(" {MATCH_EPOCH} "))
        + "\n";
    assert!(
        matches!(judge(Some(&text)), Found::Discard(Stale::Damaged)),
        "{text:?}"
    );
    assert!(!sound(&text), "{text:?}");
}

/// A line cut short anywhere, as a torn write would leave it, is never read
/// as a shorter match or a different one, even when the cut falls inside
/// its last shot.
#[test]
fn a_truncated_line_is_damaged_wherever_it_is_cut() {
    let (shots, live) = bot_match(11, 320, true, 9);
    let line = SavedMatch::of(&live, [true; 2], &shots).to_line();
    for cut in 1..line.len() {
        let torn = &line[..cut];
        assert!(
            matches!(judge(Some(torn)), Found::Discard(Stale::Damaged)),
            "cut at {cut}: {torn:?}"
        );
    }
    assert!(matches!(judge(Some(&line)), Found::Resume(_)));
}

#[test]
fn the_pre_match_choices_survive_the_line() {
    for rolling in [false, true] {
        for help in [[false, false], [true, false], [false, true], [true, true]] {
            let duel = Match::with_rules(3, 320, rules_for(rolling));
            let saved = SavedMatch::of(&duel, help, &[]);
            let back = SavedMatch::parse(&saved.to_line()).unwrap();
            assert_eq!((back.rolling, back.help), (rolling, help));
            assert_eq!(
                back.rules().gun_shift,
                if rolling { versus::GUN_SHIFT } else { 0.0 }
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any well-formed match line survives the text form unchanged. The
    /// fuzz target (`fuzz/fuzz_targets/match_log.rs`) throws raw bytes and
    /// arbitrary logs at the parser and the replay.
    #[test]
    fn any_line_round_trips(
        seed in any::<u64>(),
        width in SCREEN_W_MIN..=SCREEN_W_MAX,
        rolling in any::<bool>(),
        help in any::<[bool; 2]>(),
        log in prop::collection::vec(
            prop_oneof![
                9 => (0..=MAX_ANGLE_TICKS, 0..=POWER_TICKS_MAX).prop_map(|(a, p)| Entry::Shot(a, p)),
                1 => Just(Entry::NextRound),
            ],
            0..60,
        ),
    ) {
        let saved = SavedMatch {
            epoch: SIM_EPOCH,
            match_epoch: MATCH_EPOCH,
            seed,
            width,
            rolling,
            help,
            log,
        };
        let line = saved.to_line();
        prop_assert_eq!(SavedMatch::parse(&line), Ok(saved));
        prop_assert!(sound(&line));
    }
}

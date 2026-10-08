use super::*;
use crate::match_log::{rules_for, Entry, MATCH_EPOCH};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use salt_line_sim::ballistics::Shot;
use salt_line_sim::run::Phase;

/// A short high lob that lands well short of anything.
const LOB: (i32, i32) = (1400, 330);

/// A run played to its end the way the game plays it: the solver's shot
/// at the first standing target for `good` shots (drafts take offer 0),
/// then every shell thrown away.
pub(crate) fn finished_run(seed: u64, width: usize, good: usize) -> RunState {
    let mut run = RunState::new(seed, width);
    let mut fired = 0;
    while !run.is_over() {
        match run.phase {
            Phase::Drafting => {
                run.take_offer(0);
            }
            Phase::Cleared => {
                run.resolve_clear();
            }
            Phase::Firing => {
                let target = run.screen.structures.iter().position(|s| !s.is_down());
                let shot = target
                    .filter(|_| fired < good)
                    .and_then(|t| {
                        if let Some(f) = run.screen.default_fuse(t) {
                            run.set_fuse(f);
                        }
                        salt_line_sim::solver::solve_target(&run.screen, &run.loadout, t)
                            .first()
                            .map(|c| c.shot)
                    })
                    .unwrap_or(Shot::new(1400, 0));
                run.fire(shot, false);
                fired += 1;
            }
            Phase::Over(_) => unreachable!(),
        }
    }
    run
}

/// A match played to its end by a bot: the first shot that hits, except
/// every third, which lobs short; the next round dealt after each round.
/// Returns its line (as the match file keeps it) and the match.
pub(crate) fn finished_match(seed: u64, ai: Option<Level>) -> (SavedMatch, Match) {
    finished_match_on(seed, ai, None)
}

/// [`finished_match`] on `layout`'s ground (format 2, or 3 with the AI),
/// where the guns do not roll.
pub(crate) fn finished_match_on(
    seed: u64,
    ai: Option<Level>,
    layout: Option<&salt_line_sim::layout::Layout>,
) -> (SavedMatch, Match) {
    let rolling = layout.is_none();
    let mut duel = match layout {
        None => Match::with_rules(seed, 320, rules_for(rolling)),
        Some(l) => crate::custom::layout_match(l, seed, rules_for(rolling)),
    };
    let mut log = Vec::new();
    let mut fired = 0;
    while !matches!(duel.phase, versus::Phase::MatchOver(_)) {
        if matches!(duel.phase, versus::Phase::RoundOver(_)) {
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
    let saved = SavedMatch {
        epoch: SIM_EPOCH,
        match_epoch: MATCH_EPOCH,
        seed,
        width: 320,
        rolling,
        help: [true, false],
        ai,
        log,
        ground: layout.map(|l| l.to_code().unwrap()),
    };
    (saved, duel)
}

fn run_record(seed: u64) -> Record {
    Record::run(20734, &finished_run(seed, 320, 6), false)
}

fn duel_record(seed: u64, ai: Option<Level>, ground: Ground) -> Record {
    let (saved, duel) = finished_match(seed, ai);
    Record::duel(20734, &saved, &duel, ground).unwrap()
}

/// A won match on a map called `name` (a forked duel round's ground).
fn map_record(seed: u64, ai: Option<Level>, name: &str) -> Record {
    let mut layout = crate::custom::fork_duel(seed, 0);
    layout.name = salt_line_sim::layout::LayoutName::parse(name).unwrap();
    let (saved, duel) = finished_match_on(seed, ai, Some(&layout));
    Record::duel(20734, &saved, &duel, Ground::Map(name.into())).unwrap()
}

#[test]
fn the_file_format_is_pinned() {
    let mut h = History::default();
    h.push(run_record(7));
    h.push(Record::run(20735, &finished_run(9, 360, 3), true));
    h.push(duel_record(11, Some(Level::Easy), Ground::Generated));
    h.push(map_record(12, None, "MESA"));
    insta::assert_snapshot!(h.serialize());
    // Every line of it is watchable: it rebuilds to its own result.
    for r in &h.records {
        assert!(r.check().is_ok(), "{}", r.to_line());
    }
}

#[test]
fn every_kind_of_record_round_trips() {
    let records = [
        run_record(7),
        Record::run(0, &finished_run(9, 400, 2), true),
        duel_record(11, Some(Level::Hard), Ground::Generated),
        map_record(12, None, "MESA"),
        map_record(13, Some(Level::Medium), ""),
    ];
    for r in &records {
        let line = r.to_line();
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(Record::parse(&line).as_ref(), Some(r), "{line}");
        assert_eq!(r.version(), Ok(()));
    }
    let mut h = History::default();
    for r in records.clone() {
        h.push(r);
    }
    let back = History::parse(&h.serialize());
    assert_eq!(back, h);
    assert_eq!(back.records, records);
}

#[test]
fn the_list_keeps_the_newest_twenty_of_either_kind() {
    let mut h = History::default();
    let run = run_record(7);
    let duel = duel_record(11, None, Ground::Generated);
    for day in 1..=MAX_RECORDS as u64 + 5 {
        let base = if day % 2 == 0 { &run } else { &duel };
        let text = h
            .push(Record {
                day,
                ..base.clone()
            })
            .unwrap();
        assert_eq!(History::parse(&text), h);
    }
    assert_eq!(h.records.len(), MAX_RECORDS);
    // The oldest five went; the newest is listed first.
    assert_eq!(h.records[0].day, 6);
    assert_eq!(h.newest_first().next().unwrap().day, MAX_RECORDS as u64 + 5);
    // A hand-made file over the cap keeps its newest twenty too.
    let mut text = format!("{HEADER} {FORMAT}\n");
    for day in 1..=30 {
        text.push_str(&Record { day, ..run.clone() }.to_line());
    }
    let read = History::parse(&text);
    assert_eq!(read.records.len(), MAX_RECORDS);
    assert_eq!(read.records[0].day, 11);
}

#[test]
fn damaged_lines_are_dropped_and_counted_never_the_rest() {
    let good = run_record(7).to_line();
    let duel = duel_record(11, None, Ground::Generated).to_line();
    let torn = &good[..good.len() / 2];
    let bad = [
        torn.to_string(),
        "\0\0\0\0".to_string(),
        "banana".to_string(),
        good.replacen("run 1 ", "run 2 ", 1),
        good.replacen("run 1 ", "walk 1 ", 1),
        good.replacen(" 320 ", " 0320 ", 1),
        good.replacen(" 320 ", " 9999 ", 1),
        good.replacen(" v2 ", " v1 ", 1),
        duel.replacen(" L ", " X ", 1).replacen(" R ", " X ", 1),
        duel.replacen(" 2p ", " expert ", 1),
        duel.replacen(" - ", " mesa ", 1),
        duel.replacen("match ", "batch ", 1),
    ];
    let mut text = format!("{HEADER} {FORMAT}\n{good}");
    for b in &bad {
        text.push_str(b);
        text.push('\n');
    }
    text.push_str(&duel);
    let mut h = History::parse(&text);
    assert_eq!(h.records.len(), 2, "{text}");
    assert_eq!(h.damaged, bad.len());
    // The next write leaves them out.
    let written = h.push(run_record(8)).unwrap();
    assert_eq!(h.damaged, 0);
    assert_eq!(History::parse(&written).damaged, 0);
    assert_eq!(History::parse(&written).records.len(), 3);
}

#[test]
fn a_file_too_large_or_without_a_header_is_still_read_safely() {
    let huge = "x".repeat(MAX_FILE_CHARS + 1);
    let h = History::parse(&huge);
    assert!(h.records.is_empty());
    assert_eq!(h.damaged, 1);
    // No header: read as this format, as a hand-made file would be.
    let h = History::parse(&run_record(7).to_line());
    assert_eq!(h.records.len(), 1);
    assert_eq!(h.damaged, 0);
    // A damaged header is counted, and written fresh next time.
    let h = History::parse(&format!("history one\n{}", run_record(7).to_line()));
    assert_eq!((h.records.len(), h.damaged), (1, 1));
    assert_eq!(History::parse(""), History::default());
}

#[test]
fn a_newer_builds_file_is_read_but_never_written() {
    let text = format!(
        "{HEADER} {}\n{}future 9 something new\n",
        FORMAT + 1,
        run_record(7).to_line()
    );
    let mut h = History::parse(&text);
    assert!(h.from_the_future);
    assert_eq!(h.records.len(), 1);
    // Its lines this build cannot read are not counted as damage: they
    // are not this build's to read, and are never written over.
    assert_eq!(h.damaged, 0);
    assert_eq!(h.push(run_record(8)), None);
    // Kept in memory for the session all the same.
    assert_eq!(h.records.len(), 2);
}

#[test]
fn another_epoch_stays_listed_but_cannot_be_watched() {
    for (epoch, why) in [
        (SIM_EPOCH - 1, Refused::OlderVersion),
        (SIM_EPOCH + 1, Refused::NewerVersion),
    ] {
        // Its log may be in a form this build has never seen: kept as is.
        let line = format!("run 1 20000 {epoch} 42 320 0 3 0 41 v9 @0 Q7 900:600\n");
        let r = Record::parse(&line).expect("listed");
        assert_eq!(r.version(), Err(why));
        assert_eq!(r.check(), Err(why));
        assert_eq!(r.to_line(), line, "written back as it was");
        assert!(r.summary().contains("3 SCREENS"));
        assert!(r.summary().contains("SEED 42"));
    }
    // A match from older duel rules, likewise: its result is still listed.
    let (mut saved, duel) = finished_match(11, Some(Level::Easy));
    saved.match_epoch = MATCH_EPOCH - 1;
    let r = Record::duel(20000, &saved, &duel, Ground::Generated).unwrap();
    let r = Record::parse(&r.to_line()).expect("listed");
    assert_eq!(r.version(), Err(Refused::OlderVersion));
    assert_eq!(r.check(), Err(Refused::OlderVersion));
    assert!(r.summary().contains("AI EASY"));
    // This build's own epoch with a log it cannot read is damage, not age.
    let line = format!("run 1 20000 {SIM_EPOCH} 42 320 0 3 0 41 v9 @0 Q7 900:600\n");
    assert_eq!(Record::parse(&line), None);
}

#[test]
fn a_record_is_watched_only_if_it_rebuilds_to_its_own_result() {
    let run = run_record(7);
    match run.check() {
        Ok(Watchable::Run(replay, score)) => {
            let rebuilt = replay.reconstruct().unwrap();
            assert_eq!(rebuilt.score(), score);
            assert!(rebuilt.is_over());
        }
        other => panic!("{other:?}"),
    }
    // A score that is not the log's.
    let mut tampered = run.clone();
    if let Kind::Run(r) = &mut tampered.kind {
        r.score.0 += 1;
    }
    assert_eq!(tampered.check(), Err(Refused::Mismatch));
    // A log cut short: it replays, but to a run still going.
    let mut short = run.clone();
    if let Kind::Run(r) = &mut short.kind {
        let cut = r.log.rfind(' ').unwrap();
        r.log.truncate(cut);
    }
    assert_eq!(short.check(), Err(Refused::Mismatch));

    let duel = duel_record(11, Some(Level::Easy), Ground::Generated);
    assert!(matches!(duel.check(), Ok(Watchable::Duel(..))));
    for tamper in [
        |d: &mut DuelRecord| d.winner = d.winner.other(),
        |d: &mut DuelRecord| d.rounds[0] += 1,
        |d: &mut DuelRecord| d.ai = Some(Level::Hard),
        |d: &mut DuelRecord| d.ai = None,
    ] {
        let mut bad = duel.clone();
        if let Kind::Duel(d) = &mut bad.kind {
            tamper(d);
        }
        assert_eq!(bad.check(), Err(Refused::Mismatch));
    }
}

#[test]
fn only_a_won_match_is_a_record() {
    let (saved, mut duel) = finished_match(11, None);
    duel.phase = versus::Phase::Aiming;
    assert_eq!(Record::duel(1, &saved, &duel, Ground::Generated), None);
}

#[rstest::rstest]
#[case::unknown(0, None)]
#[case::first_day(1, Some("JAN 2"))]
#[case::leap_day(19782, Some("FEB 29"))]
#[case::march(59, Some("MAR 1"))]
#[case::today(20734, Some("OCT 8"))]
fn dates_read_as_a_child_would_say_them(#[case] day: u64, #[case] want: Option<&str>) {
    assert_eq!(date(day).as_deref(), want);
}

#[test]
fn rows_say_who_won_against_whom() {
    let run = Record::run(20734, &finished_run(7, 320, 6), true);
    assert!(
        run.summary().starts_with("OCT 8  DAILY RUN  "),
        "{}",
        run.summary()
    );
    let won = |winner: Side, rounds: [u32; 2], ai, ground| Record {
        day: 20734,
        kind: Kind::Duel(DuelRecord {
            ai,
            ground,
            winner,
            rounds,
            line: String::new(),
        }),
    };
    let rows: Vec<String> = [
        won(Side::Left, [2, 1], Some(Level::Easy), Ground::Generated),
        won(Side::Right, [0, 2], Some(Level::Hard), Ground::Generated),
        won(Side::Left, [2, 0], None, Ground::Map("MESA".into())),
        won(Side::Right, [1, 2], None, Ground::Map(String::new())),
        Record::run(0, &finished_run(7, 320, 1), false),
    ]
    .iter()
    .map(Record::summary)
    .collect();
    insta::assert_debug_snapshot!(rows);
}

#[test]
fn a_whole_copy_has_its_header_and_no_nuls() {
    let text = {
        let mut h = History::default();
        h.push(run_record(7)).unwrap()
    };
    assert!(sound(&text));
    assert!(sound(&format!("\n{HEADER} 9\n")));
    assert!(!sound(&text.replacen('h', "\0", 1)));
    assert!(!sound(&run_record(7).to_line()));
    assert!(!sound(""));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any run line round-trips, from this epoch or another, whatever its
    /// numbers.
    #[test]
    fn run_lines_round_trip(
        day in any::<u64>(),
        seed in any::<u64>(),
        width in SCREEN_W_MIN..=SCREEN_W_MAX,
        daily in any::<bool>(),
        score in (any::<u32>(), any::<i32>(), any::<u32>()),
        epoch in prop_oneof![Just(SIM_EPOCH), 0..SIM_EPOCH + 5],
    ) {
        let log = if epoch == SIM_EPOCH {
            "v2 @0 F0 900:600 870:640".to_string()
        } else {
            "v7 anything goes here".to_string()
        };
        let r = Record {
            day,
            kind: Kind::Run(RunRecord { epoch, seed, width, daily, score, log }),
        };
        prop_assert_eq!(Record::parse(&r.to_line()), Some(r));
    }

    /// Any text at all reads without a panic, and what it reads writes
    /// back to the same records.
    #[test]
    fn any_text_reads_and_rereads_the_same(text in "\\PC{0,400}") {
        let h = History::parse(&text);
        let again = History::parse(&h.serialize());
        prop_assert_eq!(again.records, h.records);
    }
}

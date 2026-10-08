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
        assert_eq!(Record::parse(&line).as_ref(), Ok(r), "{line}");
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

/// A run from an older sim, cheap to make many of: listed, never replayed.
fn old_run(seed: u64, log_len: usize) -> Record {
    Record::new(
        seed,
        Kind::Run(RunRecord {
            epoch: SIM_EPOCH - 1,
            seed,
            width: 320,
            daily: false,
            score: (1, 0, 40),
            log: format!("v9{}", " 900:600".repeat(log_len)),
        }),
    )
}

/// A match from older duel rules, likewise.
fn old_duel(seed: u64) -> Record {
    Record::new(
        seed,
        Kind::Duel(DuelRecord {
            ai: None,
            ground: Ground::Generated,
            winner: Side::Left,
            rounds: [2, 0],
            line: format!("match 1 {} 1 {seed} 320 1 0 0 0", SIM_EPOCH - 1),
        }),
    )
}

#[test]
fn each_kind_keeps_its_newest_twenty_and_pushes_out_only_its_own() {
    let mut h = History::default();
    // Twenty-five runs, then one match: the five oldest runs went.
    for seed in 1..=MAX_RUNS as u64 + 5 {
        let text = h.push(old_run(seed, 3)).unwrap();
        assert_eq!(History::parse(&text), h);
    }
    h.push(old_duel(1000)).unwrap();
    assert_eq!(h.records.iter().filter(|r| r.is_run()).count(), MAX_RUNS);
    assert_eq!(h.records[0].day, 6);
    // Twenty-five matches: every run stays, the oldest matches go.
    for seed in 1001..=1000 + MAX_MATCHES as u64 + 5 {
        h.push(old_duel(seed)).unwrap();
    }
    let runs = h.records.iter().filter(|r| r.is_run()).count();
    let matches = h.records.len() - runs;
    assert_eq!((runs, matches), (MAX_RUNS, MAX_MATCHES));
    assert!(!h.records.iter().any(|r| r.day <= 1005 && !r.is_run()));
    // One list, newest first.
    assert_eq!(
        h.listed().next().unwrap().day,
        1000 + MAX_MATCHES as u64 + 5
    );
    // A hand-made file over the caps keeps the newest of each kind too.
    let mut text = format!("{HEADER} {FORMAT}\n");
    for seed in 1..=30 {
        text.push_str(&old_run(seed, 3).to_line());
        text.push_str(&old_duel(100 + seed).to_line());
    }
    let read = History::parse(&text);
    assert_eq!(read.records.len(), MAX_RUNS + MAX_MATCHES);
    assert_eq!(read.records[0].day, 11);
    assert_eq!(read.damaged, 0, "lines past the cap are not damage");
}

#[test]
fn the_same_game_is_never_recorded_twice() {
    let mut h = History::default();
    let run = run_record(7);
    let duel = duel_record(11, None, Ground::Generated);
    assert!(h.push(run.clone()).is_some());
    assert!(h.push(duel.clone()).is_some());
    // Recorded again on another day (a launch, a resumed plate): skipped.
    assert_eq!(h.push(Record { day: 1, ..run }), None);
    assert_eq!(h.push(Record { day: 1, ..duel }), None);
    assert_eq!(h.records.len(), 2);
}

#[test]
fn a_written_file_fits_its_limit_by_losing_its_oldest() {
    let mut h = History::default();
    // Lines of about 30,000 characters: the limit holds about 17.
    let mut text = String::new();
    for seed in 1..=MAX_RUNS as u64 {
        text = h.push(old_run(seed, 3_700)).unwrap();
    }
    assert!(text.len() <= MAX_FILE_CHARS, "{}", text.len());
    assert!(h.records.len() < MAX_RUNS);
    assert_eq!(h.listed().next().unwrap().day, MAX_RUNS as u64);
    assert_eq!(History::parse(&text), h);
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
        good.replacen("run 1 ", "walk 1 ", 1),
        good.replacen("run 1 ", "run 0 ", 1),
        good.replacen(" 320 ", " 0320 ", 1),
        good.replacen(" 320 ", " 9999 ", 1),
        good.replacen(" v2 ", " v1 ", 1),
        duel.replacen(" L ", " X ", 1).replacen(" R ", " X ", 1),
        duel.replacen(" 2p ", " expert ", 1),
        duel.replacen(" - ", " mesa ", 1),
        duel.replacen("match ", "batch ", 1),
        // Too long to be honest, however it parses.
        old_run(1, MAX_LINE_CHARS / 8).to_line(),
    ];
    // A newer line format, of a kind this build knows or not, is counted
    // apart: a newer version's, not damage.
    let newer = [
        good.replacen("run 1 ", "run 2 ", 1),
        "film 7 anything".to_string(),
    ];
    let mut text = format!("{HEADER} {FORMAT}\n{good}");
    for b in bad.iter().chain(&newer) {
        text.push_str(b);
        text.push('\n');
    }
    text.push_str(&duel);
    let mut h = History::parse(&text);
    assert_eq!(h.records.len(), 2, "{text}");
    assert_eq!(h.damaged, bad.len());
    assert_eq!(h.newer, newer.len());
    // The next write leaves them out.
    let written = h.push(run_record(8)).unwrap();
    assert_eq!((h.damaged, h.newer), (0, 0));
    let back = History::parse(&written);
    assert_eq!((back.damaged, back.newer, back.records.len()), (0, 0, 3));
}

#[test]
fn a_line_says_whether_it_is_damaged_or_newer() {
    let good = run_record(7).to_line();
    assert_eq!(Record::parse("run 1 x"), Err(LineError::Damaged));
    assert_eq!(
        Record::parse(&good.replacen("run 1 ", "run 2 ", 1)),
        Err(LineError::Newer)
    );
    assert_eq!(Record::parse("film 3 a new kind"), Err(LineError::Newer));
    assert_eq!(
        Record::parse("film 1 an unknown kind"),
        Err(LineError::Damaged)
    );
}

#[test]
fn a_large_file_or_one_without_a_header_keeps_its_good_lines() {
    // Junk far past the write limit, around good lines: they are read.
    let junk = format!("{}\n", "x".repeat(MAX_FILE_CHARS + 1));
    let text = format!(
        "{HEADER} {FORMAT}\n{junk}{}{junk}{}",
        run_record(7).to_line(),
        duel_record(11, None, Ground::Generated).to_line()
    );
    assert!(text.len() > 2 * MAX_FILE_CHARS);
    let h = History::parse(&text);
    assert_eq!((h.records.len(), h.damaged), (2, 2));
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
        "{HEADER} {}\n{}future 9 something new\nbanana\n",
        FORMAT + 1,
        run_record(7).to_line()
    );
    let mut h = History::parse(&text);
    assert!(h.from_the_future);
    assert_eq!(h.records.len(), 1);
    // Its lines this build cannot read are not counted: they are not this
    // build's to read, and are never written over.
    assert_eq!((h.damaged, h.newer), (0, 0));
    assert_eq!(h.push(run_record(8)), None);
    // Kept in memory for the session all the same.
    assert_eq!(h.records.len(), 2);
    // And nothing being written, nothing counted is forgotten.
    let mut h = History { damaged: 3, ..h };
    assert_eq!(h.push(run_record(9)), None);
    assert_eq!(h.damaged, 3);
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
    assert_eq!(Record::parse(&line), Err(LineError::Damaged));
}

#[test]
fn a_record_is_watched_only_if_it_rebuilds_to_its_own_result() {
    let run = run_record(7);
    match run.check() {
        Ok(Watchable::Run(replay, score, daily)) => {
            assert!(!daily);
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
    let won = |winner: Side, rounds: [u32; 2], ai, ground| {
        Record::new(
            20734,
            Kind::Duel(DuelRecord {
                ai,
                ground,
                winner,
                rounds,
                line: String::new(),
            }),
        )
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
        let r = Record::new(day, Kind::Run(RunRecord { epoch, seed, width, daily, score, log }));
        prop_assert_eq!(Record::parse(&r.to_line()), Ok(r));
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

#[test]
fn a_record_too_long_to_read_back_is_never_written() {
    let mut h = History::default();
    h.push(run_record(7));
    // A run so long its line passes MAX_LINE_CHARS: the next launch would
    // read it as damaged and drop it, so it is not kept at all.
    let mut long = run_record(8);
    if let Kind::Run(r) = &mut long.kind {
        while r.log.len() <= MAX_LINE_CHARS {
            r.log.push_str(" 1:100:100");
        }
    }
    assert_eq!(h.push(long), None);
    assert_eq!(h.records.len(), 1);
    // Whatever push writes, parse reads back whole.
    let text = h.push(run_record(9)).unwrap();
    let back = History::parse(&text);
    assert_eq!(back.records, h.records);
    assert_eq!(back.damaged, 0);
}

// ------------------------------------------------------------------ stars

/// `h` with the record of day `day` starred, its new text.
fn star_day(h: &mut History, day: u64) -> Result<Option<String>, StarError> {
    let r = h.records.iter().find(|r| r.day == day).unwrap().clone();
    h.set_star(&r, true)
}

#[test]
fn a_starred_record_is_never_pushed_off_by_its_kind() {
    let mut h = History::default();
    h.push(old_run(1, 3));
    h.push(old_duel(2));
    star_day(&mut h, 1).unwrap().unwrap();
    star_day(&mut h, 2).unwrap().unwrap();
    // Far more than the caps of each kind, newer than the starred.
    for seed in 10..10 + MAX_RUNS as u64 + 5 {
        h.push(old_run(seed, 3)).unwrap();
    }
    for seed in 100..100 + MAX_MATCHES as u64 + 5 {
        h.push(old_duel(seed)).unwrap();
    }
    let text = h.push(old_run(1000, 3)).unwrap();
    assert!(h.records.iter().any(|r| r.day == 1 && r.starred));
    assert!(h.records.iter().any(|r| r.day == 2 && r.starred));
    // The caps count the unstarred only.
    let unstarred = |h: &History, run: bool| {
        h.records
            .iter()
            .filter(|r| !r.starred && r.is_run() == run)
            .count()
    };
    assert_eq!(unstarred(&h, true), MAX_RUNS);
    assert_eq!(unstarred(&h, false), MAX_MATCHES);
    // And so on the next launch.
    let read = History::parse(&text);
    assert_eq!(read, h);
    assert_eq!(read.stars(), 2);
}

#[test]
fn the_starred_are_listed_first_then_the_rest_newest_first() {
    let mut h = History::default();
    for day in 1..=6 {
        h.push(old_run(day, 3));
    }
    star_day(&mut h, 2).unwrap();
    star_day(&mut h, 4).unwrap();
    let days: Vec<u64> = h.listed().map(|r| r.day).collect();
    assert_eq!(days, [4, 2, 6, 5, 3, 1]);
    // The file keeps its own order, oldest first, with a mark before each
    // starred line.
    assert_eq!(
        h.records.iter().map(|r| r.day).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn only_a_few_can_be_starred() {
    let mut h = History::default();
    for day in 1..=MAX_STARS as u64 + 2 {
        h.push(old_run(day, 3));
    }
    for day in 1..=MAX_STARS as u64 {
        assert!(star_day(&mut h, day).unwrap().is_some());
    }
    let before = h.clone();
    assert_eq!(star_day(&mut h, MAX_STARS as u64 + 1), Err(StarError::Full));
    assert_eq!(h, before, "a refused star changes nothing");
    assert_eq!(StarError::Full.note(), "5 STARS - UNSTAR ONE FIRST");
    // Starring one already starred is no change, and no write.
    assert_eq!(star_day(&mut h, 1), Ok(None));
    // Unstar one, and another can be starred.
    let first = h.records[0].clone();
    assert!(h.set_star(&first, false).unwrap().is_some());
    assert!(star_day(&mut h, MAX_STARS as u64 + 1).unwrap().is_some());
    assert_eq!(h.stars(), MAX_STARS);
    // A hand-made file with more keeps its newest stars.
    let mut text = format!("{HEADER} {FORMAT}\n");
    for day in 1..=MAX_STARS as u64 + 3 {
        text.push_str("star\n");
        text.push_str(&old_run(day, 3).to_line());
    }
    let read = History::parse(&text);
    assert_eq!(read.stars(), MAX_STARS);
    assert!(read.records.iter().rev().take(MAX_STARS).all(|r| r.starred));
    assert_eq!(read.damaged, 0);
}

#[test]
fn unstarring_drops_nothing_on_the_spot() {
    let mut h = History::default();
    h.push(old_run(1, 3));
    star_day(&mut h, 1).unwrap();
    for seed in 10..10 + MAX_RUNS as u64 {
        h.push(old_run(seed, 3));
    }
    assert_eq!(h.records.len(), MAX_RUNS + 1);
    // Unstarred by mistake: still listed, and starred again.
    let one = h.records[0].clone();
    let text = h.set_star(&one, false).unwrap().unwrap();
    assert_eq!(h.records.len(), MAX_RUNS + 1);
    assert!(h.set_star(&one, true).unwrap().is_some());
    // Left unstarred, it goes with the next run of its kind...
    h.set_star(&one, false).unwrap();
    h.push(old_run(500, 3));
    assert!(!h.records.iter().any(|r| r.day == 1));
    assert_eq!(h.records.len(), MAX_RUNS);
    // ...or the next time the file is read.
    assert!(!History::parse(&text).records.iter().any(|r| r.day == 1));
}

#[test]
fn a_full_file_loses_its_unstarred_first() {
    let mut h = History::default();
    // Lines of about 30,000 characters: the limit holds about 17.
    h.push(old_run(1, 3_700));
    h.push(old_run(2, 3_700));
    star_day(&mut h, 1).unwrap();
    star_day(&mut h, 2).unwrap();
    let mut text = String::new();
    for seed in 10..10 + MAX_RUNS as u64 {
        text = h.push(old_run(seed, 3_700)).unwrap();
    }
    assert!(text.len() <= MAX_FILE_CHARS, "{}", text.len());
    // The oldest two, starred, are kept; the oldest unstarred went.
    assert_eq!(h.records[0].day, 1);
    assert_eq!(h.records[1].day, 2);
    assert!(!h.records.iter().any(|r| r.day == 10));
    assert_eq!(History::parse(&text), h);
    // Only once nothing unstarred is left does a starred one go, oldest
    // first: a file of starred lines too long to fit together (by hand:
    // five stars of real lines never come near it).
    let mut h = History {
        records: (1..=20)
            .map(|day| Record {
                starred: true,
                ..old_run(day, 3_700)
            })
            .collect(),
        ..History::default()
    };
    let text = h.written().unwrap();
    assert!(text.len() <= MAX_FILE_CHARS);
    assert!(h.records.len() < 20);
    assert_eq!(h.records.last().unwrap().day, 20);
    assert_eq!(h.records[0].day, 20 - h.records.len() as u64 + 1);
}

#[test]
fn a_star_is_its_own_line_and_round_trips() {
    let mut h = History::default();
    h.push(run_record(7));
    h.push(duel_record(11, Some(Level::Easy), Ground::Generated));
    h.push(map_record(12, None, "MESA"));
    let plain = h.serialize();
    assert!(
        plain.starts_with("history 1\n"),
        "no star: format 1, as before"
    );
    let duel = h.records[1].clone();
    let text = h.set_star(&duel, true).unwrap().unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "history 2");
    assert_eq!(lines[2], "star");
    // The record's own line is the same starred or not: a star is not part
    // of the game a share code carries (DESIGN §9.7).
    assert_eq!(lines[3], duel.to_line().trim_end());
    assert_eq!(
        text.replace("star\n", "").replace("history 2", "history 1"),
        plain
    );
    let read = History::parse(&text);
    assert_eq!(read, h);
    assert!(read.records[1].starred && read.is_starred(&duel));
    assert!(read.records[1].check().is_ok());
    // A record's line on its own never carries the star.
    assert_eq!(
        Record::parse(&read.records[1].to_line()).unwrap().starred,
        false
    );
    // Unstarred again, the file is format 1 again.
    let mut back = read;
    let text = back.set_star(&duel, false).unwrap().unwrap();
    assert_eq!(text, plain);
}

#[test]
fn an_older_build_reads_a_starred_file_but_never_writes_it() {
    // What a format-1 build does with a file: a header past 1 is a newer
    // build's, read but never written (`a_newer_builds_file_is_read_but_
    // never_written`), and a `star` line is not a record line.
    let mut h = History::default();
    h.push(run_record(7));
    h.push(run_record(8));
    let first = h.records[0].clone();
    let text = h.set_star(&first, true).unwrap().unwrap();
    let header: u32 = text.lines().next().unwrap()[HEADER.len() + 1..]
        .parse()
        .unwrap();
    assert!(header > FORMAT_PLAIN, "an older build leaves it alone");
    assert_eq!(Record::parse(STAR), Err(LineError::Damaged));
    // Every record line is one a format-1 build reads: the list there shows
    // them all, starred ones included, just without their stars.
    for line in text.lines().skip(1).filter(|l| *l != STAR) {
        assert!(Record::parse(line).is_ok(), "{line}");
    }
    // And this build reads a newer build's starred file the same way.
    let newer = text.replacen("history 2", "history 3", 1);
    let mut read = History::parse(&newer);
    assert!(read.from_the_future);
    assert_eq!(read.records.len(), 2);
    assert_eq!(
        read.set_star(&read.records[1].clone(), true),
        Ok(None),
        "never written"
    );
}

#[test]
fn starring_a_game_not_yet_listed_adds_it() {
    // A won match whose plate is still up is recorded once its plate is
    // left; STAR at the end of its replay adds it now, starred.
    let mut h = History::default();
    let duel = duel_record(11, None, Ground::Generated);
    let text = h.set_star(&duel, true).unwrap().unwrap();
    assert_eq!(h.records.len(), 1);
    assert!(h.records[0].starred);
    assert!(History::parse(&text).records[0].starred);
    // Recorded again as its plate is left: already listed, still starred.
    assert_eq!(h.push(duel.clone()), None);
    assert!(h.is_starred(&duel));
    // Unstarring one that is not listed is no change.
    assert_eq!(h.set_star(&run_record(7), false), Ok(None));
    assert_eq!(h.records.len(), 1);
    // One too long to keep cannot be starred.
    let mut long = run_record(8);
    if let Kind::Run(r) = &mut long.kind {
        while r.log.len() <= MAX_LINE_CHARS {
            r.log.push_str(" 1:100:100");
        }
    }
    assert_eq!(h.set_star(&long, true), Err(StarError::NotKept));
}

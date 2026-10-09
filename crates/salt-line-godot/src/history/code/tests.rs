//! Replay codes (`history/code.rs`): every kind of record there and back,
//! the format pinned, damage and newer versions told apart, and the sizes
//! a family's games come to.

use super::*;
use crate::history::tests::{finished_match, finished_match_on, finished_run};
use crate::match_log::{rules_for, MATCH_EPOCH};
use crate::versus::{self, Match, HITS_TO_WIN_ROUND};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use salt_line_sim::ballistics::Shot;
use salt_line_sim::codec::{base32, crc16};
use salt_line_sim::layout::LayoutName;
use salt_line_sim::rng::mix64;
use salt_line_sim::run::{Phase, RunState};
use salt_line_sim::SIM_EPOCH;

/// A short high lob that lands well short of anything.
const LOB: (i32, i32) = (1400, 330);
const DAY: u64 = 20_734;

/// A run played the way a person plays one: each target walked in on,
/// `misses` shots near the solver's (the aim moved a little from the last
/// one, closer each time), then the solver's own; drafts take offer 0.
/// Plays until the shells run out.
fn played_run(seed: u64, width: usize, misses: u32) -> RunState {
    let mut run = RunState::new(seed, width);
    let mut n = 0u64;
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
                let solved = target.and_then(|t| {
                    if let Some(f) = run.screen.default_fuse(t) {
                        run.set_fuse(f);
                    }
                    salt_line_sim::solver::solve_target(&run.screen, &run.loadout, t)
                        .first()
                        .map(|c| c.shot)
                });
                let Some(shot) = solved else {
                    run.fire(Shot::new(LOB.0, LOB.1), false);
                    continue;
                };
                for miss in (1..=misses).rev() {
                    // Off by a few degrees and a few power, less each time.
                    let r = mix64(seed ^ n);
                    n += 1;
                    let da = (r % 41) as i32 + 20 * miss as i32;
                    let dp = ((r >> 8) % 21) as i32 + 8 * miss as i32;
                    let sign = if (r >> 16) & 1 == 0 { 1 } else { -1 };
                    let a = (shot.angle_ticks + sign * da).clamp(0, MAX_ANGLE_TICKS);
                    let p = (shot.power_ticks - dp).clamp(0, POWER_TICKS_MAX);
                    if run.is_over() || run.phase != Phase::Firing {
                        break;
                    }
                    run.fire(Shot::new(a, p), false);
                }
                if !run.is_over() && run.phase == Phase::Firing {
                    run.fire(shot, false);
                }
            }
            Phase::Over(_) => unreachable!(),
        }
    }
    run
}

/// A match played by a bot that hits, except where `miss` says (the round,
/// and the turn within it), when it lobs short. On `layout`'s ground if
/// given, else generated screens; guns roll if `rolling`. Returns the
/// match's line, the match, and whether a round went past its mark tied
/// (overtime).
fn match_by(
    seed: u64,
    ai: Option<Level>,
    layout: Option<&Layout>,
    rolling: bool,
    miss: impl Fn(u32, usize) -> bool,
) -> (SavedMatch, Match, bool) {
    let mut duel = match layout {
        None => Match::with_rules(seed, 320, rules_for(rolling)),
        Some(l) => crate::custom::layout_match(l, seed, rules_for(rolling)),
    };
    let (mut log, mut turn, mut overtime) = (Vec::new(), 0, false);
    while !matches!(duel.phase, versus::Phase::MatchOver(_)) {
        if matches!(duel.phase, versus::Phase::RoundOver(_)) {
            duel.next_round();
            log.push(Entry::NextRound);
            turn = 0;
        }
        let side = duel.to_move;
        let aim = if miss(duel.round, turn) {
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
        turn += 1;
        overtime |= duel.phase == versus::Phase::Aiming
            && duel.hits.iter().all(|&h| h >= HITS_TO_WIN_ROUND);
        assert!(log.len() < 500, "a bot match that never ends");
    }
    let saved = SavedMatch {
        epoch: SIM_EPOCH,
        match_epoch: MATCH_EPOCH,
        seed,
        width: 320,
        rolling,
        help: [false, true],
        ai,
        log,
        ground: layout.map(|l| l.to_code().unwrap()),
    };
    (saved, duel, overtime)
}

/// A map: a forked duel round's ground, named.
fn map(seed: u64, name: &str) -> Layout {
    let mut layout = crate::custom::fork_duel(seed, 0);
    layout.name = LayoutName::parse(name).unwrap();
    layout
}

fn ground_of(layout: Option<&Layout>) -> Ground {
    layout.map_or(Ground::Generated, |l| {
        Ground::Map(l.name.as_str().to_string())
    })
}

/// A won match on `MESA` that went all three rounds, the first to overtime
/// (both sides hit until a tie past the mark, then the right misses once),
/// the second to its opener (whoever answers misses every shot), the
/// third as it falls.
fn overtime_on_map(ai: Option<Level>) -> Record {
    let layout = map(12, "MESA");
    let (saved, duel, overtime) =
        match_by(12, ai, Some(&layout), false, |round, turn| match round {
            0 => turn == 7,
            1 => turn % 2 == 1,
            _ => turn % 3 == 2,
        });
    assert!(overtime, "the match went to overtime");
    assert_eq!(duel.rounds.iter().sum::<u32>(), 3, "all three rounds");
    Record::duel(DAY, &saved, &duel, ground_of(Some(&layout))).unwrap()
}

/// One record of every kind a family makes: runs (daily, every width,
/// with drafts and fuses), matches between two players and against each
/// level of the AI, on generated ground with the guns rolling and on maps
/// named and not, one into overtime.
fn every_kind() -> Vec<Record> {
    let mut out = vec![
        Record::run(DAY, &finished_run(7, 320, 6), false),
        Record::run(DAY + 1, &finished_run(9, 400, 2), true),
        Record::run(0, &played_run(5, 360, 2), false),
    ];
    for ai in [
        None,
        Some(Level::Easy),
        Some(Level::Medium),
        Some(Level::Hard),
    ] {
        let (saved, duel) = finished_match(31, ai);
        out.push(Record::duel(DAY, &saved, &duel, Ground::Generated).unwrap());
        let layout = map(13, "RIDGE");
        let (saved, duel) = finished_match_on(13, ai, Some(&layout));
        out.push(Record::duel(DAY, &saved, &duel, ground_of(Some(&layout))).unwrap());
    }
    let unnamed = map(14, "");
    let (saved, duel) = finished_match_on(14, None, Some(&unnamed));
    out.push(Record::duel(DAY, &saved, &duel, ground_of(Some(&unnamed))).unwrap());
    out.push(overtime_on_map(Some(Level::Medium)));
    out
}

#[test]
fn every_kind_of_record_goes_there_and_back() {
    let records = every_kind();
    // The guns roll in the matches on generated ground.
    let rolled = records.iter().any(|r| match &r.kind {
        Kind::Duel(d) => SavedMatch::parse(&format!("{}\n", d.line)).unwrap().rolling,
        Kind::Run(_) => false,
    });
    assert!(rolled);
    for r in &records {
        let code = r.to_code().unwrap();
        assert!(code.starts_with(CODE_PREFIX), "{code}");
        assert!(code[2..]
            .bytes()
            .all(|b| salt_line_sim::codec::ALPHABET.contains(&b)));
        let back = Record::from_code(&code).unwrap();
        // The same record, its line byte for byte, and as watchable.
        assert_eq!(&back, r);
        assert_eq!(back.to_line(), r.to_line());
        assert!(!back.starred);
        assert_eq!(back.check(), r.check());
        assert!(back.check().is_ok(), "{}", r.to_line());
        // Read whatever the case, and wrapped by a chat app.
        assert_eq!(
            Record::from_code(&code.to_ascii_lowercase()).as_ref(),
            Ok(r)
        );
        let wrapped: String = code
            .as_bytes()
            .chunks(40)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Record::from_code(&format!("  {wrapped}\n")).as_ref(), Ok(r));
    }
}

#[test]
fn a_star_stays_home() {
    let mut r = Record::run(DAY, &finished_run(7, 320, 6), false);
    let plain = r.to_code().unwrap();
    r.starred = true;
    assert_eq!(r.to_code().unwrap(), plain);
    assert!(!Record::from_code(&plain).unwrap().starred);
}

/// The format is pinned: a change to how a record is packed changes these
/// codes, and needs a new [`CODE_FORMAT`] (old codes must still read as
/// what they were, or say they cannot).
#[test]
fn the_format_is_pinned() {
    let records = [
        Record::run(DAY, &finished_run(7, 320, 6), false),
        Record::run(DAY, &played_run(5, 360, 1), true),
        {
            let (saved, duel) = finished_match(31, Some(Level::Easy));
            Record::duel(DAY, &saved, &duel, Ground::Generated).unwrap()
        },
        overtime_on_map(None),
    ];
    let codes: Vec<String> = records
        .iter()
        .map(|r| format!("{}\n{}\n", r.to_line().trim_end(), r.to_code().unwrap()))
        .collect();
    insta::assert_snapshot!(codes.join("\n"));
}

/// The bytes, written out here by hand from the module docs' table rather
/// than by the encoder: a run of one screen, a fuse set, two shots and a
/// pick. Checks the format as documented, not just the code against
/// itself.
#[test]
fn a_hand_packed_run_reads_as_the_docs_say() {
    let mut b = BitWriter::default();
    b.put(KIND_RUN, 2);
    put_gamma(&mut b, SIM_EPOCH);
    put_wide(&mut b, DAY);
    put_wide(&mut b, 1234);
    b.put(320, 9);
    b.put(0, 1); // not the daily
    put_gamma(&mut b, 1); // screens
    put_wide(&mut b, zigzag64(-2)); // shells
    put_gamma(&mut b, 2); // shots
    put_gamma(&mut b, 3); // entries: two shots and a pick
    b.put(0, 4); // the angle's exp-Golomb parameter: fewest bits for 8 and 0
    b.put(0, 4); // the power's: fewest bits for 1 and 0
    b.put(0b1110, 4); // the fuse: 0 to 10
    put_exp_golomb(&mut b, 10, FUSE_K);
    b.put(0, 1); // a shot: 900 + 4, 500 - 1
    put_exp_golomb(&mut b, zigzag(4), 0);
    put_exp_golomb(&mut b, zigzag(-1), 0);
    b.put(0, 1); // a shot: the same again
    put_exp_golomb(&mut b, 0, 0);
    put_exp_golomb(&mut b, 0, 0);
    b.put(0b110, 3); // screen 0 to 1
    put_gamma(&mut b, 0);
    b.put(0b10, 2); // offer 2
    put_gamma(&mut b, 2);
    let mut bytes = vec![CODE_FORMAT];
    bytes.extend(b.finish());
    let crc = crc16(&bytes);
    bytes.extend(crc.to_be_bytes());
    let code = format!("SR{}", base32(&bytes));
    let r = Record::from_code(&code).unwrap();
    assert_eq!(r.day, DAY);
    let Kind::Run(run) = &r.kind else {
        panic!("{r:?}");
    };
    assert_eq!(
        (run.epoch, run.seed, run.width, run.daily, run.score),
        (SIM_EPOCH, 1234, 320, false, (1, -2, 2))
    );
    assert_eq!(run.log, "v2 @0 F10 904:499 904:499 @1 D2");
    // And the encoder writes exactly those bytes.
    assert_eq!(r.to_code().unwrap(), code);
}

/// Every character of a code changed to any other is caught by the
/// checksum (or the padding), never read as another game; so is a code cut
/// short or run on.
#[test]
fn damage_is_caught_and_said() {
    let r = overtime_on_map(None);
    let code = r.to_code().unwrap();
    let alphabet = salt_line_sim::codec::ALPHABET;
    for i in CODE_PREFIX.len()..code.len() {
        for &c in alphabet {
            if c == code.as_bytes()[i] {
                continue;
            }
            let mut bad = code.clone().into_bytes();
            bad[i] = c;
            let bad = String::from_utf8(bad).unwrap();
            assert_eq!(Record::from_code(&bad), Err(ReadError::Damaged), "{i} {c}");
        }
    }
    for cut in [3, code.len() / 2, code.len() - 1] {
        assert_eq!(Record::from_code(&code[..cut]), Err(ReadError::Damaged));
    }
    assert_eq!(
        Record::from_code(&format!("{code}0")),
        Err(ReadError::Damaged)
    );
    assert_eq!(Record::from_code("SR U"), Err(ReadError::Damaged));
    assert_eq!(Record::from_code("SR"), Err(ReadError::Damaged));
    assert_eq!(Record::from_code(""), Err(ReadError::NotACode));
    assert_eq!(
        Record::from_code("https://example.com"),
        Err(ReadError::NotACode)
    );
    assert_eq!(Record::from_code("1234567"), Err(ReadError::NotACode));
    let map = map(12, "MESA").to_code().unwrap();
    assert_eq!(Record::from_code(&map), Err(ReadError::MapCode));
    assert_eq!(
        Record::from_code(&"0".repeat(MAX_CODE_CHARS + 1)),
        Err(ReadError::TooLong)
    );
    assert_eq!(
        Record::from_code(&" ".repeat(2 * MAX_CODE_CHARS + 1)),
        Err(ReadError::TooLong)
    );
}

/// Numbers no honest code holds are damage, never a panic: an offset moved
/// past `i64`, a day past `MAX_DAY` (which the list's date would overflow
/// on).
#[test]
fn hostile_numbers_are_damage() {
    let head = |b: &mut BitWriter, day: u64| {
        b.put(KIND_RUN, 2);
        put_gamma(b, SIM_EPOCH);
        put_wide(b, day);
        put_wide(b, 1);
        b.put(320, 9);
        b.put(0, 1);
        put_gamma(b, 0);
        put_wide(b, 0);
        put_gamma(b, 0);
    };
    // Offset +1, a shot, then a change of nearly 2^64.
    let mut b = BitWriter::default();
    head(&mut b, DAY);
    put_gamma(&mut b, 2);
    b.put(0, 4);
    b.put(0, 4);
    b.put(0b1111, 4);
    put_wide(&mut b, 1);
    b.put(0, 1);
    put_exp_golomb(&mut b, 0, 0);
    put_exp_golomb(&mut b, 0, 0);
    b.put(0b1111, 4);
    put_wide(&mut b, u64::MAX - 2);
    b.put(0, 1);
    put_exp_golomb(&mut b, 0, 0);
    put_exp_golomb(&mut b, 0, 0);
    assert_eq!(
        Record::from_code(&framed(CODE_FORMAT, b.finish())),
        Err(ReadError::Damaged)
    );
    // A day past the limit, in an otherwise empty run.
    for (day, ok) in [(MAX_DAY, true), (MAX_DAY + 1, false), (u64::MAX, false)] {
        let mut b = BitWriter::default();
        head(&mut b, day);
        put_gamma(&mut b, 0);
        b.put(0, 8);
        let read = Record::from_code(&framed(CODE_FORMAT, b.finish()));
        assert_eq!(read.is_ok(), ok, "{day}");
        if let Ok(r) = read {
            let _ = r.summary();
        }
    }
}

/// A code framed with the right checksum around `format` and `body`.
fn framed(format: u8, body: Vec<u8>) -> String {
    let mut bytes = vec![format];
    bytes.extend(body);
    salt_line_sim::codec::frame(CODE_PREFIX, bytes)
}

#[test]
fn a_newer_format_says_so() {
    assert_eq!(
        Record::from_code(&framed(CODE_FORMAT + 1, vec![0; 8])),
        Err(ReadError::Newer)
    );
    assert_eq!(
        Record::from_code(&framed(0, vec![0; 8])),
        Err(ReadError::Damaged)
    );
    // A match on a map whose bytes are a newer layout format.
    let mut b = BitWriter::default();
    b.put(KIND_DUEL, 2);
    put_gamma(&mut b, SIM_EPOCH);
    put_gamma(&mut b, MATCH_EPOCH);
    put_wide(&mut b, DAY);
    put_wide(&mut b, 1);
    b.put(320, 9);
    b.put(0, 3);
    b.put(0, 2);
    b.put(1, 1);
    put_gamma(&mut b, 3);
    for byte in [LAYOUT_FORMAT + 1, 0, 0] {
        b.put(byte as u32, 8);
    }
    assert_eq!(
        Record::from_code(&framed(CODE_FORMAT, b.finish())),
        Err(ReadError::Newer)
    );
}

/// The two kinds not yet used are a later build's, never damage: a
/// player is told to update.
#[test]
fn a_kind_from_a_later_build_says_update() {
    for kind in [2, 3] {
        let mut b = BitWriter::default();
        b.put(kind, 2);
        put_gamma(&mut b, SIM_EPOCH);
        assert_eq!(
            Record::from_code(&framed(CODE_FORMAT, b.finish())),
            Err(ReadError::Newer)
        );
    }
}

/// What a clipboard holds: the first code that reads, whatever is round
/// it; else the most useful error.
#[test]
fn the_clipboard_is_searched_for_a_code() {
    let r = overtime_on_map(None);
    let code = r.to_code().unwrap();
    let (a, b) = code.split_at(30);
    let (b, c) = b.split_at(40);
    for text in [
        code.clone(),
        format!("\u{FEFF}{code}"),
        format!("look! {code} :)"),
        format!("{a}\n{b}\r\n{c}"),
        format!("{a}\u{200B}{b}\u{2060}{c}"),
        format!("try this\n\n{a}\n{b}\n{c}\n\nsent from my phone"),
        format!("SRI said so: {code}"),
        code.to_ascii_lowercase(),
    ] {
        assert_eq!(Record::from_clipboard(&text).as_ref(), Ok(&r), "{text:?}");
    }
    let mut cut = code.clone();
    cut.truncate(code.len() - 3);
    assert_eq!(
        Record::from_clipboard(&format!("here: {cut}")),
        Err(ReadError::Damaged)
    );
    assert_eq!(
        Record::from_clipboard("I said SRI, not Sam"),
        Err(ReadError::NotACode)
    );
    let map = map(12, "MESA").to_code().unwrap();
    assert_eq!(
        Record::from_clipboard(&format!("my map: {map}")),
        Err(ReadError::MapCode)
    );
    assert_eq!(Record::from_clipboard(""), Err(ReadError::NotACode));
    assert!(holds_a_replay_code(&format!("x {code} y")));
    assert!(!holds_a_replay_code(&map));
    // A huge paste is looked through only so far, and never panics.
    let huge = format!("{}{code}", "word ".repeat(MAX_CLIPBOARD_CHARS));
    assert_eq!(Record::from_clipboard(&huge), Err(ReadError::NotACode));
}

/// A record from another sim is not shared; one shared from another sim
/// (a build either side of this one) reads, but cannot be watched.
#[test]
fn another_epoch_reads_but_cannot_be_watched() {
    let r = Record::run(DAY, &finished_run(7, 320, 6), false);
    let Kind::Run(run) = &r.kind else {
        unreachable!()
    };
    for (epoch, why) in [
        (SIM_EPOCH - 1, Refused::OlderVersion),
        (SIM_EPOCH + 1, Refused::NewerVersion),
    ] {
        let other = Record::new(
            DAY,
            Kind::Run(RunRecord {
                epoch,
                ..run.clone()
            }),
        );
        assert_eq!(other.to_code(), Err(ShareError::Version(why)));
        // As that build would have shared it.
        let code = framed(CODE_FORMAT, encode_run(DAY, run_of(&other)).unwrap());
        let back = Record::from_code(&code).unwrap();
        assert_eq!(back, other);
        assert_eq!(back.version(), Err(why));
        assert_eq!(back.check().err(), Some(why));
    }
    // A match from older duel rules.
    let (mut saved, duel) = finished_match(31, None);
    saved.match_epoch = MATCH_EPOCH - 1;
    let old = Record::duel(DAY, &saved, &duel, Ground::Generated).unwrap();
    assert_eq!(
        old.to_code(),
        Err(ShareError::Version(Refused::OlderVersion))
    );
    let Kind::Duel(d) = &old.kind else {
        unreachable!()
    };
    // Its line no longer parses here, so it is built by hand from this
    // epoch's code with the match epoch's gamma changed.
    let this = {
        let (saved, duel) = finished_match(31, None);
        Record::duel(DAY, &saved, &duel, Ground::Generated).unwrap()
    };
    let Kind::Duel(this_d) = &this.kind else {
        unreachable!()
    };
    // This epoch's code, its header packed again with the older match
    // epoch and the rest of its bits copied after it.
    let mut b = BitWriter::default();
    b.put(KIND_DUEL, 2);
    put_gamma(&mut b, SIM_EPOCH);
    put_gamma(&mut b, MATCH_EPOCH - 1);
    let rest = encode_duel(DAY, this_d).unwrap();
    // Copy every bit of the real body after its two epochs.
    let mut r = BitReader::new(&rest);
    r.get(2);
    get_gamma(&mut r);
    get_gamma(&mut r);
    while r.left() >= 8 {
        b.put(r.get(8).unwrap(), 8);
    }
    let left = r.left() as u32;
    b.put(r.get(left).unwrap(), left);
    let back = Record::from_code(&framed(CODE_FORMAT, b.finish())).unwrap();
    assert_eq!(back.version(), Err(Refused::OlderVersion));
    let Kind::Duel(back_d) = &back.kind else {
        unreachable!()
    };
    assert_eq!(back_d.line, d.line);
}

fn run_of(r: &Record) -> &RunRecord {
    match &r.kind {
        Kind::Run(run) => run,
        Kind::Duel(_) => unreachable!(),
    }
}

/// A run whose code would pass the limit is not offered: it says so.
#[test]
fn too_long_to_share_says_so() {
    let mut log = vec!["v2 @0 F0".to_string()];
    for i in 0..3000 {
        log.push(format!("{}:{}", (i * 797) % 1801, (i * 331) % 1001));
    }
    let r = Record::new(
        DAY,
        Kind::Run(RunRecord {
            epoch: SIM_EPOCH,
            seed: 1,
            width: 320,
            daily: false,
            score: (0, 0, 3000),
            log: log.join(" "),
        }),
    );
    assert_eq!(r.to_code(), Err(ShareError::TooLong));
    // A shot off the dials, as only a hand-edited line holds.
    let r = Record::new(
        DAY,
        Kind::Run(RunRecord {
            epoch: SIM_EPOCH,
            seed: 1,
            width: 320,
            daily: false,
            score: (0, 0, 1),
            log: "v2 @0 F0 5000:10".into(),
        }),
    );
    assert_eq!(r.to_code(), Err(ShareError::Unencodable));
}

/// What a family's games come to, measured: printed with `--nocapture`, and
/// held under budgets with room to spare, so a change to the format that
/// makes typical codes much longer is noticed. Budgets: a 60-shot run
/// pastes in a chat as a short paragraph, a three-round match on a map a
/// little more, most of it the map.
#[test]
fn typical_games_stay_well_under_the_limit() {
    let size = |r: &Record| r.to_code().unwrap().len();
    let shots = |r: &Record| match &r.kind {
        Kind::Run(run) => run.score.2 as usize,
        Kind::Duel(d) => SavedMatch::parse(&format!("{}\n", d.line))
            .unwrap()
            .log
            .iter()
            .filter(|e| matches!(e, Entry::Shot(..)))
            .count(),
    };
    // A run cut short: the first shots of one played the way a person
    // plays (its record's result aside, which costs a few bits whatever
    // it is), so a run of any length can be measured.
    let played = Record::run(DAY, &played_run(5, 360, 2), false);
    let first = |n: usize| {
        let mut r = played.clone();
        if let Kind::Run(run) = &mut r.kind {
            let mut log = decode_log(&run.log).unwrap();
            let shot_at = log
                .iter()
                .enumerate()
                .filter(|(_, e)| e.angle_ticks != DRAFT_MARK)
                .nth(n)
                .map_or(log.len(), |(i, _)| i);
            log.truncate(shot_at);
            run.log = encode_log(&log);
            run.score.2 = n as u32;
        }
        r
    };
    let short = first(12);
    let sixty = first(60);
    let long = played;
    let generated = {
        let (saved, duel) = finished_match(31, Some(Level::Easy));
        Record::duel(DAY, &saved, &duel, Ground::Generated).unwrap()
    };
    let custom = overtime_on_map(None);
    let rows = [
        ("short run", &short, 120),
        ("60-shot run", &sixty, 400),
        ("long run", &long, 1200),
        ("match, generated", &generated, 200),
        ("match, map, overtime", &custom, 600),
    ];
    eprintln!("| game | shots | line chars | code chars | budget |");
    eprintln!("|---|---:|---:|---:|---:|");
    for (name, r, budget) in rows {
        let (code, line) = (size(r), r.to_line().trim_end().len());
        eprintln!("| {name} | {} | {line} | {code} | {budget} |", shots(r));
        assert!(code <= budget, "{name}: {code} > {budget}");
        assert!(code < line, "{name}: the code is shorter than its line");
    }
    assert_eq!(shots(&sixty), 60);
    assert!(size(&long) < MAX_CODE_CHARS / 2);
}

prop_compose! {
    /// A run's log of any shape the format can hold: screens that only go
    /// forward, fuses and offsets that change or not, picks between shots.
    fn any_log()(steps in prop::collection::vec(
        (0u32..3, any::<bool>(), 0i32..7000, any::<bool>(), -20i32..20,
         0..=MAX_ANGLE_TICKS, 0..=POWER_TICKS_MAX, prop::option::of(0i32..3)),
        0..120,
    )) -> Vec<LoggedShot> {
        let (mut screen, mut fuse, mut offset) = (0, 0, 0);
        let mut log = Vec::new();
        for (gap, fused, f, moved, o, a, p, pick) in steps {
            screen += gap;
            if fused {
                fuse = f;
            }
            if moved {
                offset = o;
            }
            match pick {
                Some(offer) => log.push(LoggedShot {
                    screen_index: screen,
                    angle_ticks: DRAFT_MARK,
                    power_ticks: offer,
                    fuse_tenths: 0,
                    emplacement_offset: 0,
                }),
                None => log.push(LoggedShot {
                    screen_index: screen,
                    angle_ticks: a,
                    power_ticks: p,
                    fuse_tenths: fuse,
                    emplacement_offset: offset,
                }),
            }
        }
        log
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Any run record of this epoch, whatever its numbers and log, goes
    /// there and back the same, its line too.
    #[test]
    fn any_run_goes_there_and_back(
        day in 0..=MAX_DAY,
        seed in any::<u64>(),
        width in SCREEN_W_MIN..=SCREEN_W_MAX,
        daily in any::<bool>(),
        score in (any::<u32>(), any::<i32>(), any::<u32>()),
        log in any_log(),
    ) {
        let r = Record::new(day, Kind::Run(RunRecord {
            epoch: SIM_EPOCH, seed, width, daily, score, log: encode_log(&log),
        }));
        let code = r.to_code().unwrap();
        prop_assert_eq!(Record::from_code(&code), Ok(r.clone()));
        prop_assert_eq!(Record::parse(&r.to_line()), Ok(r));
    }

    /// Any match record of this epoch's numbers goes there and back (the
    /// log need not be one that replays: that is `check`'s to say).
    #[test]
    fn any_match_goes_there_and_back(
        day in 0..=MAX_DAY,
        seed in any::<u64>(),
        flags in (any::<bool>(), any::<bool>(), any::<bool>()),
        ai in prop::option::of(0usize..3),
        right in any::<bool>(),
        rounds in (0u32..5, 0u32..5),
        shots in prop::collection::vec(
            prop::option::of((0..=MAX_ANGLE_TICKS, 0..=POWER_TICKS_MAX)), 0..80),
        on_map in any::<bool>(),
    ) {
        let layout = on_map.then(|| map(seed % 50, "DUNE"));
        let saved = SavedMatch {
            epoch: SIM_EPOCH,
            match_epoch: MATCH_EPOCH,
            seed,
            width: 320,
            rolling: flags.0,
            help: [flags.1, flags.2],
            ai: ai.map(|i| Level::ALL[i]),
            log: shots
                .iter()
                .map(|s| s.map_or(Entry::NextRound, |(a, p)| Entry::Shot(a, p)))
                .collect(),
            ground: layout.as_ref().map(|l| l.to_code().unwrap()),
        };
        let r = Record::new(day, Kind::Duel(DuelRecord {
            ai: saved.ai,
            ground: ground_of(layout.as_ref()),
            winner: if right { Side::Right } else { Side::Left },
            rounds: [rounds.0, rounds.1],
            line: saved.to_line().trim_end().to_string(),
        }));
        let code = r.to_code().unwrap();
        prop_assert_eq!(Record::from_code(&code), Ok(r));
    }

    /// Any text, and any body framed with a good checksum, reads without a
    /// panic; what reads shares again as the same record.
    #[test]
    fn anything_reads_without_a_panic(
        text in "\\PC{0,300}",
        body in prop::collection::vec(any::<u8>(), 0..200),
    ) {
        for text in [text, framed(CODE_FORMAT, body)] {
            if let Ok(r) = Record::from_code(&text) {
                if let Ok(code) = r.to_code() {
                    prop_assert_eq!(Record::from_code(&code), Ok(r));
                }
            }
        }
    }
}

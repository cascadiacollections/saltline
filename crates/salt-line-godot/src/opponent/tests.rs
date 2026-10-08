use super::*;
use crate::versus::{Phase, Rules, Stakes};
use pretty_assertions::assert_eq;

/// The share of `samples` (seed, round, turn) turns that fire the hitting
/// shot at `odds`, hashed for `level`.
fn rate(level: Level, odds: Odds, samples: u64) -> f64 {
    let mut hits = 0;
    let mut n = 0;
    for seed in 0..samples / 30 {
        let seed = mix64(seed);
        for round in 0..3 {
            for turn in 0..10 {
                hits += odds.hit_on(turn_hash(seed, round, turn, level)) as u64;
                n += 1;
            }
        }
    }
    hits as f64 / n as f64
}

#[test]
fn each_level_hits_at_its_odds() {
    // 6000 turns: a binomial's spread at these odds is under 0.7%.
    for (level, want, eased) in [
        (Level::Easy, 0.2, 0.1),
        (Level::Medium, 0.4, 0.2),
        (Level::Hard, 0.6, 0.4),
    ] {
        let got = rate(level, level.odds(), 6000);
        assert!(
            (got - want).abs() < 0.025,
            "{level:?}: {got:.3} not near {want}"
        );
        let got = rate(level, level.eased(), 6000);
        assert!(
            (got - eased).abs() < 0.025,
            "{level:?} eased: {got:.3} not near {eased}"
        );
    }
}

#[test]
fn hard_is_the_demos_odds_and_the_demos_rule_is_unchanged() {
    assert_eq!(Level::Hard.odds(), Odds::tenths(6));
    // Three in five, as `h % 5 >= 2` has always said.
    for h in 0..1000u64 {
        let h = mix64(h);
        assert_eq!(DEMO_ODDS.hit_on(h), h % 5 >= 2);
    }
}

#[test]
fn the_hash_is_the_turn_and_nothing_else() {
    let h = turn_hash(7, 1, 4, Level::Medium);
    assert_eq!(h, turn_hash(7, 1, 4, Level::Medium));
    for other in [
        turn_hash(8, 1, 4, Level::Medium),
        turn_hash(7, 2, 4, Level::Medium),
        turn_hash(7, 1, 5, Level::Medium),
        turn_hash(7, 1, 4, Level::Hard),
    ] {
        assert_ne!(h, other);
    }
    // The seed and the level do not alias: `seed ^ salt` once made seed 1
    // at EASY the same turns as seed 3 at MEDIUM.
    for (seed, round, turn) in [(1, 0, 0), (1, 2, 7), (0x55, 1, 3)] {
        assert_ne!(
            turn_hash(seed, round, turn, Level::Easy),
            turn_hash(seed ^ 1 ^ 2, round, turn, Level::Medium)
        );
        assert_ne!(
            turn_hash(seed, round, turn, Level::Easy),
            turn_hash(seed ^ 1 ^ 3, round, turn, Level::Hard)
        );
    }
}

#[test]
fn levels_step_and_name_themselves() {
    assert_eq!(Level::default(), Level::Easy);
    let mut l = Level::Easy;
    for want in [Level::Medium, Level::Hard, Level::Easy] {
        l = l.next();
        assert_eq!(l, want);
    }
    for l in Level::ALL {
        assert_eq!(Level::from_token(l.token()), Some(l));
    }
    assert_eq!(Level::from_token("1"), None);
}

#[test]
fn the_ai_eases_off_one_step_while_it_is_ahead() {
    let mut duel = Match::new(3, 320);
    duel.to_move = Side::Right;
    for level in Level::ALL {
        assert_eq!(odds_now(&duel, level), level.odds(), "level at the start");
    }
    // Ahead this round.
    duel.hits = [1, 2];
    assert!(ahead(&duel, Side::Right));
    assert_eq!(odds_now(&duel, Level::Hard), Level::Medium.odds());
    assert_eq!(odds_now(&duel, Level::Medium), Level::Easy.odds());
    assert_eq!(odds_now(&duel, Level::Easy), EASED_FLOOR);
    // Ahead on hits, behind on rounds: still ahead, on hits.
    duel.rounds = [1, 0];
    assert_eq!(odds_now(&duel, Level::Hard), Level::Medium.odds());
    // Ahead on rounds, behind on hits: not ahead. Rounds won never count.
    duel.hits = [2, 0];
    duel.rounds = [0, 1];
    assert!(!ahead(&duel, Side::Right));
    for level in Level::ALL {
        assert_eq!(odds_now(&duel, level), level.odds(), "{level:?}");
    }
    // Level on hits, or behind: full odds, whatever the rounds say.
    duel.hits = [1, 1];
    assert_eq!(odds_now(&duel, Level::Hard), Level::Hard.odds());
    duel.rounds = [1, 1];
    assert_eq!(odds_now(&duel, Level::Hard), Level::Hard.odds());
    duel.hits = [3, 2];
    assert_eq!(odds_now(&duel, Level::Easy), Level::Easy.odds());
}

/// A round up, the AI still plays a level or trailing turn at full odds.
/// In round 2 the AI opens, so the stake it can face there is TIED (a pair
/// ended level past the mark); a LAST SHOT is the left gun's in that round.
#[test]
fn a_round_up_still_plays_level_or_behind_at_full_odds() {
    let n = versus::HITS_TO_WIN_ROUND;
    let mut duel = Match::new(5, 320);
    duel.rounds = [0, 1];
    duel.round = 1;
    duel.to_move = Match::first_to_move(1);
    assert_eq!(duel.to_move, Side::Right, "the AI opens round 2");
    // Level past the mark, the AI to open the next pair.
    duel.hits = [n, n];
    duel.turn = 2 * n;
    assert_eq!(duel.stakes(), Some(Stakes::Tied));
    for level in Level::ALL {
        assert_eq!(odds_now(&duel, level), level.odds(), "tied: {level:?}");
    }
    // Behind on hits this round.
    duel.hits = [2, 1];
    duel.turn = 4;
    for level in Level::ALL {
        assert_eq!(odds_now(&duel, level), level.odds(), "behind: {level:?}");
    }
}

/// What firing `shot` would do now, on a copy.
fn trial(duel: &Match, shot: Shot) -> versus::TurnReport {
    duel.clone().fire(shot, false).expect("aiming")
}

const ALWAYS: Odds = Odds { hits: 1, out_of: 1 };
const NEVER: Odds = Odds { hits: 0, out_of: 1 };

#[test]
fn a_hit_hits_and_a_miss_misses_cleanly() {
    // Real matches, both guns, rounds played on with craters and rolls.
    for seed in 0..6u64 {
        let mut duel = Match::new(mix64(seed ^ 0xA1), 320);
        for turn in 0..14u64 {
            if let Phase::RoundOver(_) = duel.phase {
                duel.next_round();
            }
            if duel.phase != Phase::Aiming {
                break;
            }
            let h = mix64(turn);
            let hit = trial(&duel, shot(&duel, h, ALWAYS, Fallback::Tested));
            assert!(hit.hit && !hit.self_hit, "seed {seed} turn {turn}");
            let miss = trial(&duel, shot(&duel, h, NEVER, Fallback::Tested));
            assert!(!miss.hit && !miss.self_hit, "seed {seed} turn {turn}");
            // On to the next turn, a hit one time in three.
            let odds = Odds { hits: 1, out_of: 3 };
            duel.fire(shot(&duel, h, odds, Fallback::Tested), false);
        }
    }
}

#[test]
fn the_tested_fallback_spares_its_own_gun_in_a_gale() {
    // A gale straight back at the shooter blows the high lob onto its own
    // gun: the AI tries the next fallback instead.
    let rules = Rules {
        wind_min: 40.0,
        wind_max: 40.0,
        ..Rules::DEFAULT
    };
    let mut blown_back = 0;
    for seed in 0..6u64 {
        let mut duel = Match::with_rules(seed, 320, rules);
        for _ in 0..2 {
            blown_back += trial(&duel, Shot::new(LOB.0, LOB.1)).self_hit as u32;
            assert!(
                !trial(&duel, tested_fallback(&duel)).self_hit,
                "seed {seed}"
            );
            for odds in [ALWAYS, NEVER] {
                assert!(
                    !trial(&duel, shot(&duel, 0, odds, Fallback::Tested)).self_hit,
                    "seed {seed}"
                );
            }
            duel.fire(Shot::new(1800, 0), false);
        }
    }
    assert!(blown_back > 0, "the gale must matter to this test");
}

#[test]
fn the_ai_answers_a_last_shot_like_any_turn() {
    // The left gun opens and goes 3-2 up: the right gun's last shot.
    let mut duel = Match::new(11, 320);
    for _ in 0..5 {
        let aim = shot(&duel, 0, ALWAYS, Fallback::Tested);
        assert!(duel.fire(aim, false).unwrap().hit);
    }
    assert_eq!(duel.stakes(), Some(Stakes::LastShot));
    assert!(!ahead(&duel, Side::Right), "behind: no easing off");
    for level in Level::ALL {
        let mut d = duel.clone();
        let report = d.fire(ai_shot(&duel, level), false).unwrap();
        let tied = report.hit;
        assert_eq!(
            d.phase,
            if tied {
                Phase::Aiming
            } else {
                Phase::RoundOver(Side::Left)
            },
            "{level:?}"
        );
    }
}

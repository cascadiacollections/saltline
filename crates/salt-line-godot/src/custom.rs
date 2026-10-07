//! Custom games on layouts: the versus half of DESIGN §13.
//!
//! The sim's `layout` module holds the data model, the share code, the
//! placement rules and the one-gun solvability gate, and builds a screen
//! from a layout. What it cannot do is anything with two guns, because the
//! sim knows nothing of versus (§11.2). That half is here: forking a duel
//! from its match seed ([`fork_duel`]), the duel gate for a two-gun layout
//! ([`validate`]), the fairness reading the editor will show
//! ([`duel_counts`]), and a match played on a layout ([`layout_match`]).
//!
//! A two-gun layout plays through versus unchanged. Its ground becomes a
//! [`Ground`] the match deals every round instead of a generated screen,
//! its wind becomes the rules' wind range (or a fixed wind), and its guns
//! are where versus already stands them: the left at the first gun's
//! column, the right at its mirror, so a duel layout's guns must be mirror
//! images (`Problem::GunsNotMirrored`).

use salt_line_sim::layout::{
    self, check_placement, wind_units, Layout, Problem, WindSpec, LAYOUT_WIDTH,
};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::run::Screen;
use salt_line_sim::solver::GATE_CAP;
use salt_line_sim::MIN_SOLUTIONS;

use crate::versus::{self, frame_for, shots_at, target_in_frame, Ground, Match, Rules, Side};

/// Fork round `round` of the match seeded `match_seed`: generate its duel
/// screen at the layout width for the base loadout, under the game's
/// rules, and write it down (§13.6). The guns are versus's, at
/// `EMPLACEMENT_X` and its mirror; the wind is the game's per-turn range,
/// which a layout holds exactly.
///
/// The screen a phone of another width deals for the same seed is not this
/// one: a duel is laid out between guns at both edges and mirrored about
/// the middle, so its whole ground depends on the width.
pub fn fork_duel(match_seed: u64, round: u32) -> Layout {
    let rules = Rules::DEFAULT;
    let screen = versus::generate_duel(match_seed, round, LAYOUT_WIDTH, &Loadout::new(), &rules);
    let left = screen.muzzle_x as u16;
    let right = (LAYOUT_WIDTH - 1) as u16 - left;
    Layout::from_screen(&screen, vec![left, right], wind_spec(&rules))
}

/// The layout wind that is `rules`' per-turn wind.
fn wind_spec(rules: &Rules) -> WindSpec {
    let (min, max) = (
        layout::wind_milli(rules.wind_min),
        layout::wind_milli(rules.wind_max),
    );
    if max == 0 {
        WindSpec::Calm
    } else {
        WindSpec::Range { min, max }
    }
}

/// `base` with a layout's wind in it, and the guns' roll off: a range or
/// calm becomes the rules' wind range; a fixed wind is held as a range of
/// that one strength, its direction the [`Ground`]'s. Rolling guns are off
/// on a layout until they can roll from the layout's own spots (DESIGN
/// §13.13), whatever the pre-match screen says.
pub fn duel_rules(layout: &Layout, base: Rules) -> Rules {
    let (min, max) = match layout.wind {
        WindSpec::Calm => (0, 0),
        WindSpec::Fixed(w) => (w.abs(), w.abs()),
        WindSpec::Range { min, max } => (min, max),
    };
    Rules {
        wind_min: wind_units(min),
        wind_max: wind_units(max),
        gun_shift: 0.0,
        ..base
    }
}

/// The match a two-gun layout plays as: every round on its ground, by
/// `base` with the layout's wind ([`duel_rules`]). `seed` draws the winds
/// and the rolls. Only a layout that passed [`validate`] should be played.
pub fn layout_match(layout: &Layout, seed: u64, base: Rules) -> Match {
    let wind = match layout.wind {
        WindSpec::Fixed(w) => Some(wind_units(w)),
        _ => None,
    };
    let ground = Ground {
        screen: Screen::from_layout(layout, 0.0),
        wind,
    };
    Match::on_ground(seed, ground, duel_rules(layout, base))
}

/// For each wind a duel layout's gate checks (in thousandths, the
/// layout's frame), the distinct hitting shots each gun has at the other,
/// left then right, counting up to the gate's cap. The editor's fairness
/// reading: a side with fewer shots in some wind has the harder ground.
/// Empty for a layout without two guns.
pub fn duel_counts(layout: &Layout) -> Vec<(i32, [usize; 2])> {
    if !layout.is_duel() {
        return Vec::new();
    }
    let lo = Loadout::new();
    let reach = Rules::DEFAULT.hit_reach;
    let ground = Screen::from_layout(layout, 0.0);
    layout
        .wind
        .extremes()
        .into_iter()
        .map(|wind| {
            let mut shared = ground.clone();
            shared.wind = wind_units(wind);
            let counts = Side::BOTH.map(|side| {
                let frame = frame_for(&shared, side);
                let target = target_in_frame(&shared, side);
                shots_at(&frame, target, &lo, reach, GATE_CAP).len()
            });
            (wind, counts)
        })
        .collect()
}

/// Everything wrong with a layout, or `Ok` if it may be played: the
/// placement rules, then the solvability gate. One gun: every target needs
/// `MIN_SOLUTIONS` shots in each extreme of the wind (the sim's
/// `layout::validate`). Two guns: each gun needs as many at the other, in
/// each extreme, as a generated duel does (§11.3).
pub fn validate(layout: &Layout) -> Result<(), Vec<Problem>> {
    if !layout.is_duel() {
        return layout::validate(layout);
    }
    let placement = check_placement(layout);
    if !placement.is_empty() {
        return Err(placement);
    }
    let mut problems = Vec::new();
    for (wind, counts) in duel_counts(layout) {
        for (gun, shots) in counts.into_iter().enumerate() {
            if shots < MIN_SOLUTIONS {
                problems.push(Problem::Unsolvable { gun, wind, shots });
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// A share code read and checked: the layout, or one line saying why not,
/// for the dev flags (`--salt-layout`) and the `layouts` example.
pub fn read(code: &str) -> Result<Layout, String> {
    let layout = Layout::from_code(code).map_err(|e| format!("not a layout code: {e:?}"))?;
    validate(&layout).map_err(|p| format!("the layout does not pass: {p:?}"))?;
    Ok(layout)
}

/// What `--salt-fork-run <seed>:<screen>` and `--salt-fork-duel
/// <seed>:<round>` print: the fork's share code, its length and whether it
/// passes, one per line. `None` when neither flag was given. The seed is
/// any text, as seed entry takes it; the number after the last `:` is the
/// screen (from 0, as `--salt-screen` counts) or the round (from 0).
pub fn fork_report(run: Option<&str>, duel: Option<&str>) -> Option<String> {
    let (spec, is_duel) = match (run, duel) {
        (Some(s), _) => (s, false),
        (None, Some(s)) => (s, true),
        (None, None) => return None,
    };
    let Some((seed, n)) = spec
        .rsplit_once(':')
        .and_then(|(seed, n)| Some((seed, n.parse::<u32>().ok()?)))
    else {
        return Some(format!("expected <seed>:<number>, got {spec:?}"));
    };
    let seed = salt_line_sim::run::parse_seed(seed);
    let layout = if is_duel {
        fork_duel(seed, n)
    } else {
        Layout::from_run_screen(seed, n)
    };
    let mut out = String::new();
    match layout.to_code() {
        Ok(code) => out.push_str(&format!("{code}\n{} characters\n", code.len())),
        Err(e) => out.push_str(&format!("cannot encode: {e:?}\n")),
    }
    match validate(&layout) {
        Ok(()) => out.push_str("passes\n"),
        Err(p) => out.push_str(&format!("does not pass: {p:?}\n")),
    }
    Some(out)
}

#[cfg(test)]
mod tests;

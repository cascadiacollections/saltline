//! The map editor's model, with no engine in it (DESIGN §13.11).
//!
//! A draft layout and every edit a player can make to it: sculpting and
//! smoothing the ground under a soft brush, stamping the generator's shapes,
//! dragging the guns, mirroring, the wind, a name, and undo and redo. Plus
//! the live verdict the editor shows after every edit (the placement rules,
//! the duel gate and a fairness reading), and the two ways a draft starts
//! from a generated duel: forking the round on screen (cropped to the
//! layout width) and forking a typed seed.
//!
//! The app (`app/editor.rs`) turns input into these calls, keeps the draft
//! on disk and draws it; nothing here reads input or writes a file, so all
//! of it runs under `cargo test`.
//!
//! # Edits are whole steps
//!
//! A layout is integers (§13.2), and every edit here moves a height by a
//! whole number of sixteenths and stays inside the world
//! (`WORLD_FLOOR..=WORLD_CEIL`), so undo, redo and mirroring give back
//! exactly what was there. A brush is soft (a smooth falloff to its edge)
//! but its result is rounded, and the sculpt brush moves the ground in whole
//! world units ("coarse snapping": a finger is not a stylus).
//!
//! # A stroke
//!
//! A drag is one stroke and one undo step, whatever it does on the way:
//!
//! ```text
//!   finger down ──► begin_stroke: snapshot the draft (the undo step)
//!   finger moves ─► drag_stroke: the tool's edit, from the snapshot or on
//!                   the current draft, re-checked each time
//!   finger up ────► end_stroke: kept, or forgotten if nothing changed
//! ```
//!
//! **SCULPT** is reversible within the stroke: how far the finger is above
//! or below where it went down is how far the ground under the brush is
//! raised or lowered, from the snapshot, and the brush paints wherever the
//! finger has been. So a drag up raises a hill, sliding sideways widens it,
//! and dragging back to the start puts it back. Keys and a controller nudge
//! by one unit under a cursor instead (`nudge`), coalesced into one undo
//! step per cursor spot.
//!
//! # Mirror
//!
//! With MIRROR on (the default for a duel), every edit is made on both sides
//! at once: brushes and stamps are mirrored about the middle and the guns
//! move as a pair, so the ground stays exactly symmetric and the guns mirror
//! images, as versus needs (§11.2). Turning it on makes the draft symmetric
//! (the left half copied onto the right, the right gun put at the left
//! one's mirror), as one undoable edit. With it off, either side can be
//! edited alone, and either gun dragged alone; the verdict then says when
//! the guns are no longer mirror images, which a duel cannot be played
//! with.

use salt_line_sim::dmath;
use salt_line_sim::layout::{
    self, check_placement, height_steps, height_units, wind_units, Layout, LayoutName, Problem,
    WindSpec, GUN_EDGE_MARGIN, LAYOUT_WIDTH, MAX_WIND, MIN_GUN_GAP,
};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::run::{parse_seed, Screen};
use salt_line_sim::solver;
use salt_line_sim::terrain::{
    self, ShapeParams, Template, BASE_ELEVATION, WORLD_CEIL, WORLD_FLOOR,
};
use salt_line_sim::EMPLACEMENT_X;

use crate::custom;

/// How far the editor counts each gun's hitting shots in each wind, for
/// the fairness sign. The gate stops at its cap of 8, which nearly every
/// playable duel reaches on both sides, so a fairness sign read from it
/// would say FAIR of almost anything; counted this far, a lopsided ground
/// shows. The gate's verdict is the same either way: a gun with fewer
/// than `MIN_SOLUTIONS` is short of both caps. Counting this far costs
/// about 0.9 ms on a desktop against the gate's 0.1 (NOTES §9), so a drag's
/// frames count to the gate's cap only ([`quick_verdict`]).
pub const FAIRNESS_CAP: usize = 64;

/// Snapshots the undo stack keeps (§13.11): a layout is under a kilobyte,
/// so 64 of them cost nothing.
pub const UNDO_DEPTH: usize = 64;

/// Height of a BLANK draft's level ground, in world units: low enough that
/// a stamp or a hill has room to rise, high enough for a chasm to cut.
pub const BLANK_HEIGHT: f64 = 40.0;

/// The step one press of WIND's `-` and `+` moves a wind, in thousandths:
/// a tenth, the duel HUD's precision. A press adds or takes away a step, so
/// a fork's wind (1.234, say) is never snapped to the grid unasked.
pub const WIND_STEP: i32 = 100;

/// The wind a FIXED wind starts at when chosen from CALM or a range, in
/// thousandths: the range's strongest, or this.
pub const DEFAULT_FIXED_WIND: i32 = 1000;

/// A range's strongest when RANGE is chosen from CALM: the game's own duel
/// range (`versus::WIND_MAX`).
pub const DEFAULT_RANGE_MAX: i32 = 2500;

/// Sculpt brush centres snap to this many columns: small fingers land
/// within a column or two of where they meant to, and a pair of columns
/// is still finer than anything the eye picks out at 320.
pub const COLUMN_SNAP: u16 = 2;

/// Heights in sixteenths, the world's floor and ceiling.
fn floor_steps() -> i32 {
    height_steps(WORLD_FLOOR) as i32
}

fn ceil_steps() -> i32 {
    height_steps(WORLD_CEIL) as i32
}

/// A height in sixteenths, clamped into the world.
fn clamp_steps(h: i32) -> u16 {
    h.clamp(floor_steps(), ceil_steps()) as u16
}

// ------------------------------------------------------------------ tools

/// What a drag on the ground does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Tool {
    /// Drag up to raise the ground under the brush, down to lower it.
    Sculpt,
    /// Drag to even out bumps under the brush.
    Smooth,
    /// Tap to drop one of the generator's shapes; drag to move it.
    Stamp,
    /// Drag a gun along the ground.
    Guns,
    /// CALM, RANGE or FIXED, and their strengths.
    Wind,
}

impl Tool {
    /// The tools in toolbar order.
    pub const ALL: [Tool; 5] = [
        Tool::Sculpt,
        Tool::Smooth,
        Tool::Stamp,
        Tool::Guns,
        Tool::Wind,
    ];

    /// Its toolbar label.
    pub fn label(self) -> &'static str {
        match self {
            Tool::Sculpt => "SCULPT",
            Tool::Smooth => "SMOOTH",
            Tool::Stamp => "STAMP",
            Tool::Guns => "GUNS",
            Tool::Wind => "WIND",
        }
    }

    /// The next tool along the toolbar (`step` of 1 or -1), wrapping.
    pub fn step(self, step: i32) -> Tool {
        let i = Tool::ALL.iter().position(|t| *t == self).unwrap_or(0) as i32;
        Tool::ALL[(i + step).rem_euclid(Tool::ALL.len() as i32) as usize]
    }
}

/// How wide SCULPT and SMOOTH reach.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Brush {
    Small,
    Medium,
    Large,
}

impl Brush {
    pub const ALL: [Brush; 3] = [Brush::Small, Brush::Medium, Brush::Large];

    /// Columns either side of the centre the brush reaches.
    pub fn radius(self) -> i32 {
        match self {
            Brush::Small => 6,
            Brush::Medium => 14,
            Brush::Large => 28,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Brush::Small => "S",
            Brush::Medium => "M",
            Brush::Large => "L",
        }
    }
}

/// The shapes STAMP drops: the generator's own templates (§2.3), laid out
/// around the point tapped. `Overhang` is left out: no duel uses it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stamp {
    Ridge,
    Chasm,
    Mesa,
    Spires,
    Basin,
}

impl Stamp {
    pub const ALL: [Stamp; 5] = [
        Stamp::Ridge,
        Stamp::Chasm,
        Stamp::Mesa,
        Stamp::Spires,
        Stamp::Basin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stamp::Ridge => "RIDGE",
            Stamp::Chasm => "CHASM",
            Stamp::Mesa => "MESA",
            Stamp::Spires => "SPIRES",
            Stamp::Basin => "BASIN",
        }
    }

    fn template(self) -> Template {
        match self {
            Stamp::Ridge => Template::Ridge,
            Stamp::Chasm => Template::Chasm,
            Stamp::Mesa => Template::Mesa,
            Stamp::Spires => Template::Spires,
            Stamp::Basin => Template::Basin,
        }
    }

    /// The generator's shape for this template, centred on column `x`, as
    /// a change in height per column in sixteenths: what `terrain::shape`
    /// adds to level ground, with the template's emplacement and target
    /// placed so its feature lands on `x` (a ridge's crest, a chasm's gap,
    /// a mesa's top, the middle spire, a basin's floor).
    pub fn delta(self, x: i32, width: usize) -> Vec<i32> {
        // The span from emplacement to target the templates are laid out
        // over; a duel's is about 260, a stamp is a single feature.
        let span = 100.0;
        let x = x as f64;
        let (muzzle, target) = match self {
            Stamp::Ridge => (x - 0.55 * span, x + 0.45 * span),
            Stamp::Chasm => (x + 26.0 - span, x + 26.0),
            Stamp::Mesa | Stamp::Basin => (x - span, x),
            Stamp::Spires => (x - 0.62 * span, x + 0.38 * span),
        };
        let params = ShapeParams {
            width,
            amp: 1.0,
            muzzle_x: muzzle,
            target_x: target,
            elev_delta: 0.0,
            spire_count: 3,
        };
        // Level ground at the generator's base elevation: everything the
        // template keeps of the noise relief is zero, so what comes out is
        // the template's own shape.
        let mut h = vec![BASE_ELEVATION; width];
        terrain::shape(&mut h, self.template(), &params);
        h.iter()
            .map(|&v| height_steps(v) as i32 - height_steps(BASE_ELEVATION) as i32)
            .collect()
    }
}

/// Which kind of wind WIND is setting.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum WindMode {
    Calm,
    Range,
    Fixed,
}

impl WindMode {
    pub const ALL: [WindMode; 3] = [WindMode::Calm, WindMode::Range, WindMode::Fixed];

    pub fn of(spec: &WindSpec) -> WindMode {
        match spec {
            WindSpec::Calm => WindMode::Calm,
            WindSpec::Range { .. } => WindMode::Range,
            WindSpec::Fixed(_) => WindMode::Fixed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WindMode::Calm => "CALM",
            WindMode::Range => "RANGE",
            WindMode::Fixed => "FIXED",
        }
    }
}

/// Which number a WIND `-`/`+` press moves.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum WindField {
    /// A fixed wind's strength and direction.
    Fixed,
    /// A range's weakest.
    Min,
    /// A range's strongest.
    Max,
}

/// `spec` with `field` moved by `steps` of [`WIND_STEP`], held to what a
/// layout allows: a fixed wind within ±[`MAX_WIND`], a range's ends within
/// `0..=MAX_WIND` and in order (moving one end past the other carries the
/// other with it). A fork's odd thousandths are kept: the step is added.
pub fn step_wind(spec: WindSpec, field: WindField, steps: i32) -> WindSpec {
    let d = steps * WIND_STEP;
    match (spec, field) {
        (WindSpec::Fixed(w), _) => WindSpec::Fixed((w + d).clamp(-MAX_WIND, MAX_WIND)),
        (WindSpec::Range { min, max }, WindField::Min) => {
            let min = (min + d).clamp(0, MAX_WIND);
            WindSpec::Range {
                min,
                max: max.max(min),
            }
        }
        (WindSpec::Range { min, max }, _) => {
            let max = (max + d).clamp(0, MAX_WIND);
            WindSpec::Range {
                min: min.min(max),
                max,
            }
        }
        (WindSpec::Calm, _) => WindSpec::Calm,
    }
}

/// `spec` as `mode`, keeping what it can: a range from a fixed wind spans
/// calm to its strength; a fixed wind from a range is its strongest.
pub fn wind_as(spec: WindSpec, mode: WindMode) -> WindSpec {
    match (mode, spec) {
        (WindMode::Calm, _) => WindSpec::Calm,
        (WindMode::Range, WindSpec::Range { .. }) | (WindMode::Fixed, WindSpec::Fixed(_)) => spec,
        (WindMode::Range, WindSpec::Fixed(w)) if w != 0 => WindSpec::Range {
            min: 0,
            max: w.abs(),
        },
        (WindMode::Range, _) => WindSpec::Range {
            min: 0,
            max: DEFAULT_RANGE_MAX,
        },
        (WindMode::Fixed, WindSpec::Range { max, .. }) if max != 0 => WindSpec::Fixed(max),
        (WindMode::Fixed, _) => WindSpec::Fixed(DEFAULT_FIXED_WIND),
    }
}

/// A wind in thousandths as the editor shows it: to the tenth when that is
/// all it holds, to the thousandth when a fork brought finer.
pub fn wind_text(milli: i32) -> String {
    let w = wind_units(milli);
    if milli % 100 == 0 {
        format!("{w:.1}")
    } else {
        format!("{w:.3}")
    }
}

// ------------------------------------------------------------- the draft

/// Where a fork came from (§13.11): shown as its label until it is saved
/// under a name, and kept beside its code in the slot file and the draft
/// file. Not part of the share code: a code is the layout alone, and its
/// format is unchanged (§13.7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Origin {
    /// FORK SEED: a typed seed (keypad characters) and a round, from 0.
    Seed { text: String, round: u32 },
    /// Fork this round: the match on screen's seed and round, from 0, and
    /// the width it was dealt at, before the crop to the layout width.
    Match { seed: u64, round: u32, width: u16 },
}

impl Origin {
    /// What the editor and the lists show: `FROM SEED X R2`, or
    /// `FROM MATCH R2` (a match's seed is a twenty-digit number nobody
    /// reads). Rounds count from 1 on screen.
    pub fn label(&self) -> String {
        match self {
            Origin::Seed { text, round } => format!("FROM SEED {text} R{}", round + 1),
            Origin::Match { round, .. } => format!("FROM MATCH R{}", round + 1),
        }
    }

    /// One token for a file line: `seed:<text>:<round>` or
    /// `match:<seed>:<round>:<width>`.
    pub fn token(&self) -> String {
        match self {
            Origin::Seed { text, round } => format!("seed:{text}:{round}"),
            Origin::Match { seed, round, width } => format!("match:{seed}:{round}:{width}"),
        }
    }

    /// [`Origin::token`] read back. `None` for anything else.
    pub fn parse(token: &str) -> Option<Origin> {
        let mut parts = token.split(':');
        let from = match parts.next()? {
            "seed" => {
                let text = parts.next()?;
                let ok = !text.is_empty()
                    && text.len() <= crate::app::SEED_MAX
                    && text.bytes().all(|b| LayoutName::CHARSET.contains(&b));
                if !ok {
                    return None;
                }
                Origin::Seed {
                    text: text.to_string(),
                    round: canonical(parts.next()?)?,
                }
            }
            "match" => Origin::Match {
                seed: canonical(parts.next()?)?,
                round: canonical(parts.next()?)?,
                width: canonical(parts.next()?)?,
            },
            _ => return None,
        };
        let round = match &from {
            Origin::Seed { round, .. } | Origin::Match { round, .. } => *round,
        };
        (parts.next().is_none() && round < MAX_ROUND).then_some(from)
    }
}

/// Rounds FORK SEED offers, and the most a provenance may name.
pub const MAX_ROUND: u32 = 9;

/// A number only as `Display` writes it.
fn canonical<T: std::str::FromStr + ToString>(tok: &str) -> Option<T> {
    let v: T = tok.parse().ok()?;
    (v.to_string() == tok).then_some(v)
}

/// The layout being edited, where it came from, and the slot it was opened
/// from or last saved to (the default place SAVE offers).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Draft {
    pub layout: Layout,
    pub from: Option<Origin>,
    pub slot: Option<u8>,
}

impl Draft {
    /// BLANK: level ground, the guns where versus stands them, the game's
    /// own duel wind, no name.
    pub fn blank() -> Draft {
        let left = EMPLACEMENT_X as u16;
        Draft {
            layout: Layout {
                name: LayoutName::default(),
                heights: vec![height_steps(BLANK_HEIGHT); LAYOUT_WIDTH],
                structures: Vec::new(),
                guns: vec![left, (LAYOUT_WIDTH - 1) as u16 - left],
                wind: WindSpec::Range {
                    min: 0,
                    max: DEFAULT_RANGE_MAX,
                },
            },
            from: None,
            slot: None,
        }
    }

    /// FORK SEED: round `round` (from 0) of the duel on the typed seed, at
    /// the layout width (`custom::fork_duel`), labelled as such.
    pub fn from_seed(text: &str, round: u32) -> Draft {
        Draft {
            layout: custom::fork_duel(parse_seed(text), round),
            from: Some(Origin::Seed {
                text: text.to_string(),
                round,
            }),
            slot: None,
        }
    }

    /// What the editor calls it: its name, else where it came from, else
    /// UNNAMED.
    pub fn label(&self) -> String {
        if !self.layout.name.as_str().is_empty() {
            self.layout.name.as_str().to_string()
        } else if let Some(from) = &self.from {
            from.label()
        } else {
            "UNNAMED".to_string()
        }
    }
}

// ------------------------------------------------------- fork this round

/// Fork the round on screen (§13.11, owner decision October 2026): the
/// ground as it was dealt (no craters) and the guns where they stand,
/// cropped to [`LAYOUT_WIDTH`]. Returns the heights in sixteenths and the
/// left gun's column; the right gun is its mirror.
///
/// **The crop rule.** A duel is laid out between guns that stand the same
/// distance from their own edges, so on a buffer wider than the layout
/// (a phone's 360 to 400) the guns are further apart than 320 columns
/// can hold with both chassis on screen.
///
/// 1. If both guns, chassis and edge margin, fit in the centred 320-column
///    window, that window is the fork: the middle of the ground as the
///    players saw it, guns and all. That is a buffer of 320, a narrow
///    phone, or guns rolled forward.
/// 2. Otherwise each gun keeps its own side of the ground: the leftmost
///    and rightmost 160 columns are joined and the middle is what is lost,
///    with the eight columns either side of the join eased to meet half way
///    so no cliff is left there. Each gun keeps its distance from its own
///    edge, so the guns stay mirror images.
///
/// Either way the guns are put back as mirror images (on an odd difference
/// in width the centred window cannot be exactly central, and the right
/// gun moves a column to stay the left's mirror).
pub fn crop_live(base: &[f64], left_gun: f64) -> (Vec<u16>, u16) {
    let w = base.len();
    let n = LAYOUT_WIDTH;
    let steps: Vec<u16> = base.iter().map(|&h| height_steps(h)).collect();
    let g = dmath::round(left_gun).max(0.0) as usize;
    if w <= n {
        // Narrower can only be the layout width itself (buffers are at
        // least 320); pad defensively with the last column.
        let mut h = steps;
        let last = h.last().copied().unwrap_or(height_steps(BLANK_HEIGHT));
        h.resize(n, last);
        return (h, g.min(n / 2 - 1) as u16);
    }
    let x0 = (w - n) / 2;
    let margin = GUN_EDGE_MARGIN as usize;
    let right = w - 1 - g;
    if g >= x0 + margin && right + margin < x0 + n {
        return (steps[x0..x0 + n].to_vec(), (g - x0) as u16);
    }
    let half = n / 2;
    let mut h: Vec<i32> = steps[..half]
        .iter()
        .chain(&steps[w - half..])
        .map(|&v| v as i32)
        .collect();
    let (a, b) = (h[half - 1], h[half]);
    let mid = (a + b) / 2;
    let ease = 8;
    for k in 0..ease {
        let share = (ease - k) as i32;
        h[half - 1 - k] += (mid - a) * share / ease as i32;
        h[half + k] += (mid - b) * share / ease as i32;
    }
    (h.into_iter().map(clamp_steps).collect(), g as u16)
}

/// Fork this round: the screen the match on screen dealt for this round,
/// cropped by [`crop_live`], under `wind` (the match's own: the game's duel
/// range, or a layout's wind), labelled with the match's seed and round.
pub fn fork_live(screen: &Screen, seed: u64, round: u32, wind: WindSpec) -> Draft {
    let (heights, left) = crop_live(&screen.field.base, screen.muzzle_x);
    Draft {
        layout: Layout {
            name: LayoutName::default(),
            heights,
            structures: Vec::new(),
            guns: vec![left, (LAYOUT_WIDTH - 1) as u16 - left],
            wind,
        },
        from: Some(Origin::Match {
            seed,
            round,
            width: screen.width as u16,
        }),
        slot: None,
    }
}

// --------------------------------------------------------------- verdict

/// How much a problem matters to the editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// The layout can be saved but is flagged (owner decision: a floating
    /// structure, which the generator itself leaves on about 9% of run
    /// screens, §13.5).
    Warn,
    /// The layout cannot be played or saved.
    Block,
}

/// How bad `problem` is. Everything blocks but a floating structure, which
/// warns: the rule is kept for runs and challenges, downgraded (owner
/// decision, October 2026). Duels have no structures, so for them every
/// problem blocks.
pub fn severity(problem: &Problem) -> Severity {
    match problem {
        Problem::StructureFloating(_) => Severity::Warn,
        _ => Severity::Block,
    }
}

/// The fairness sign (§13.11): which side has fewer ways through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fairness {
    LeftHarder,
    Fair,
    RightHarder,
}

impl Fairness {
    pub fn label(self) -> &'static str {
        match self {
            Fairness::LeftHarder => "LEFT HARDER",
            Fairness::Fair => "FAIR",
            Fairness::RightHarder => "RIGHT HARDER",
        }
    }
}

/// The fairness sign from the duel gate's shot counts (each gun's distinct
/// hitting shots in each wind checked, counted to [`FAIRNESS_CAP`]).
/// Summed over the winds, a side with more than a tenth fewer than the
/// other (and at least two) has the harder ground; closer than that it is
/// FAIR. The counts move by a shot or two with any small change to the
/// ground, so a tighter line would flicker between signs as a kid drags a
/// hill about. On mirrored ground the counts are mirror images, so it
/// always reads FAIR.
pub fn fairness(counts: &[(i32, [usize; 2])]) -> Option<Fairness> {
    if counts.is_empty() {
        return None;
    }
    let (l, r) = counts
        .iter()
        .fold((0, 0), |(l, r), (_, [a, b])| (l + a, r + b));
    let harder = |few: usize, many: usize| few + 2 <= many && few * 10 < many * 9;
    Some(if harder(l, r) {
        Fairness::LeftHarder
    } else if harder(r, l) {
        Fairness::RightHarder
    } else {
        Fairness::Fair
    })
}

/// What the editor shows of a layout: every problem with its severity, the
/// duel gate's counts, and the fairness sign.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Verdict {
    pub problems: Vec<(Problem, Severity)>,
    /// `custom::duel_counts`, if the placement rules let the gate run.
    pub counts: Vec<(i32, [usize; 2])>,
    pub fairness: Option<Fairness>,
}

impl Verdict {
    /// Nothing blocks: the layout can be saved and played.
    pub fn playable(&self) -> bool {
        !self.problems.iter().any(|(_, s)| *s == Severity::Block)
    }

    /// Something warns.
    pub fn warns(&self) -> bool {
        self.problems.iter().any(|(_, s)| *s == Severity::Warn)
    }
}

/// Check `layout` as the editor does after every edit: the placement rules
/// (§13.5), then, unless one of those blocks, the solvability gate at each
/// wind it checks: the duel gate for two guns, the run gate for one. About
/// a tenth of a millisecond (NOTES §9).
pub fn verdict(layout: &Layout) -> Verdict {
    verdict_counted(layout, FAIRNESS_CAP)
}

/// [`verdict`] counting only to the gate's cap: the same problems for a
/// tenth of the time, but counts too shallow for a fairness sign, which is
/// left out. What each frame of a drag runs (NOTES §9).
pub fn quick_verdict(layout: &Layout) -> Verdict {
    Verdict {
        fairness: None,
        ..verdict_counted(layout, salt_line_sim::solver::GATE_CAP)
    }
}

fn verdict_counted(layout: &Layout, cap: usize) -> Verdict {
    let mut problems: Vec<(Problem, Severity)> = check_placement(layout)
        .into_iter()
        .map(|p| (p, severity(&p)))
        .collect();
    let blocked = problems.iter().any(|(_, s)| *s == Severity::Block);
    let mut counts = Vec::new();
    if !blocked {
        if layout.is_duel() {
            counts = custom::duel_counts_capped(layout, cap);
            for (wind, c) in &counts {
                for (gun, &shots) in c.iter().enumerate() {
                    if shots < salt_line_sim::MIN_SOLUTIONS {
                        problems.push((
                            Problem::Unsolvable {
                                gun,
                                wind: *wind,
                                shots,
                            },
                            Severity::Block,
                        ));
                    }
                }
            }
        } else {
            // The run gate, as `layout::check_targets` runs it, but past a
            // warning: a floating target still has to be reachable.
            let lo = Loadout::new();
            for wind in layout.wind.extremes() {
                let report = solver::validate(&Screen::from_layout(layout, wind_units(wind)), &lo);
                if !report.solvable() {
                    problems.push((
                        Problem::Unsolvable {
                            gun: 0,
                            wind,
                            shots: report.worst,
                        },
                        Severity::Block,
                    ));
                }
            }
        }
    }
    Verdict {
        fairness: fairness(&counts),
        problems,
        counts,
    }
}

/// A problem in a few words a child can read (§13.11: a symbol and short
/// words, not a sentence). The symbol is the renderer's.
pub fn problem_words(p: &Problem) -> String {
    let gun = |i: usize| if i == 0 { "LEFT" } else { "RIGHT" };
    match *p {
        Problem::Width(_) => "WRONG WIDTH".into(),
        Problem::HeightOutOfWorld { .. } => "GROUND OFF THE WORLD".into(),
        Problem::GunCount(_) => "NEEDS TWO GUNS".into(),
        Problem::GunOffScreen(i) => format!("{} GUN OFF SCREEN", gun(i)),
        Problem::GunNotSeated(i) => format!("{} GUN NOT ON GROUND", gun(i)),
        Problem::GunsNotMirrored => "GUNS NOT MIRRORED".into(),
        Problem::GunsOutOfOrder => "GUNS SWAPPED".into(),
        Problem::GunsTooClose => "GUNS TOO CLOSE".into(),
        Problem::NoTargets => "NO TARGETS".into(),
        Problem::TooManyTargets(_) => "TOO MANY TARGETS".into(),
        Problem::StructuresInDuel => "NO TARGETS IN A DUEL".into(),
        Problem::StructureOffScreen(_) => "TARGET OFF SCREEN".into(),
        Problem::StructureFloating(_) => "TARGET FLOATING".into(),
        Problem::StructureBuried(_) => "TARGET BURIED".into(),
        Problem::StructureOnGun(_) => "TARGET ON THE GUN".into(),
        Problem::StructureBehindGun(_) => "TARGET BEHIND GUN".into(),
        Problem::StructuresTooClose(..) => "TARGETS TOO CLOSE".into(),
        Problem::Wind => "WIND TOO STRONG".into(),
        Problem::Unsolvable { gun: g, wind, .. } => {
            if wind == 0 {
                format!("{} CAN'T HIT - CALM", gun(g))
            } else {
                format!("{} CAN'T HIT - WIND {}", gun(g), wind_text(wind))
            }
        }
    }
}

/// The gun a problem is about, if one: the editor marks it.
pub fn problem_gun(p: &Problem) -> Option<usize> {
    match *p {
        Problem::GunOffScreen(i) | Problem::GunNotSeated(i) => Some(i),
        Problem::Unsolvable { gun, .. } => Some(gun),
        _ => None,
    }
}

// ---------------------------------------------------------------- editor

/// What a drag does, fixed at its start.
#[derive(Clone, Debug)]
enum StrokeKind {
    /// Columns the brush has reached and the most of it each has had, and
    /// the column it was last at, so a fast finger paints the whole way.
    Sculpt {
        peak: Vec<f64>,
        last: i32,
    },
    Smooth,
    Stamp,
    /// Which gun moves: 0 left, 1 right.
    Gun(usize),
}

/// A drag in progress. See "A stroke".
#[derive(Clone, Debug)]
struct Stroke {
    /// The draft as the stroke found it: its undo step.
    base: Draft,
    /// Where the finger went down, in buffer pixels (`x` a column, `y` a
    /// row from the top).
    start: (f32, f32),
    kind: StrokeKind,
}

/// Consecutive edits that make one undo step (a held key, a run of WIND
/// presses), so one UNDO takes back the whole run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    /// Nudges under the cursor with this tool, at this column.
    Nudge(Tool, u16),
    /// WIND presses.
    Wind,
    /// NAME keypad presses.
    Name,
}

/// The editor: a draft, the tools, the undo history and the live verdict.
#[derive(Clone, Debug)]
pub struct Editor {
    pub draft: Draft,
    undo: Vec<Draft>,
    redo: Vec<Draft>,
    pub tool: Tool,
    pub brush: Brush,
    pub stamp: Stamp,
    /// Which WIND number `-`/`+` moves.
    pub wind_field: WindField,
    /// Edits are made on both sides at once (see "Mirror").
    pub mirror: bool,
    /// The keyboard and controller's column.
    pub cursor: u16,
    stroke: Option<Stroke>,
    group: Option<Group>,
    /// The draft's verdict, kept current by every edit.
    pub verdict: Verdict,
    /// Bumped by every change to the draft: what the app keeps on disk
    /// and redraws by.
    pub rev: u64,
}

impl Editor {
    /// An editor on `draft`, MIRROR on for a duel (owner decision: the
    /// default for duels), which a duel's guns need to be played. The
    /// ground is left as it is: a fork's tenth of asymmetry stays until
    /// MIRROR is turned off and on again, but every edit from here is made
    /// on both sides.
    pub fn new(draft: Draft) -> Editor {
        let mirror = draft.layout.is_duel();
        Editor::with_mirror(draft, mirror)
    }

    /// An editor on `draft` with MIRROR as given (a draft restored from
    /// disk keeps the setting it was left with).
    pub fn with_mirror(draft: Draft, mirror: bool) -> Editor {
        let verdict = verdict(&draft.layout);
        let cursor = draft.layout.guns.first().copied().unwrap_or(0);
        Editor {
            draft,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Sculpt,
            brush: Brush::Medium,
            stamp: Stamp::Ridge,
            wind_field: WindField::Max,
            mirror,
            cursor,
            stroke: None,
            group: None,
            verdict,
            rev: 0,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.draft.layout
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Steps UNDO could take back.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// A stroke is in progress.
    pub fn stroking(&self) -> bool {
        self.stroke.is_some()
    }

    /// The draft changed: re-check it and bump the revision.
    /// While a finger drags, each frame is checked with [`quick_verdict`]
    /// (every problem, in a tenth of the time) and the fairness sign holds
    /// what it last said; the stroke's end checks it in full.
    fn changed(&mut self) {
        if self.stroke.is_some() {
            let fairness = self.verdict.fairness;
            self.verdict = quick_verdict(&self.draft.layout);
            if !self.verdict.counts.is_empty() {
                self.verdict.fairness = fairness;
            }
        } else {
            self.verdict = verdict(&self.draft.layout);
        }
        self.rev += 1;
    }

    /// Keep the draft as it is now as an undo step, unless `group` is the
    /// run of edits already being kept as one. Clears REDO.
    fn checkpoint(&mut self, group: Option<Group>) {
        if group.is_some() && group == self.group {
            return;
        }
        self.group = group;
        self.push_undo(self.draft.clone());
    }

    /// Make an edit with `f` as one undo step (or part of `group`'s): kept
    /// only if it changed the draft, so a press that does nothing (a gun
    /// at its limit, a wind at its cap) leaves no empty step and keeps REDO.
    fn edit(&mut self, group: Option<Group>, f: impl FnOnce(&mut Self)) {
        self.end_stroke();
        let before = self.draft.clone();
        f(self);
        if self.draft == before {
            return;
        }
        let after = std::mem::replace(&mut self.draft, before);
        self.checkpoint(group);
        self.draft = after;
        self.changed();
    }

    fn push_undo(&mut self, snapshot: Draft) {
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Replace the whole draft (a fork, a paste, a slot opened, BLANK) as
    /// one undoable edit, so nothing is lost to a wrong tap.
    pub fn replace(&mut self, draft: Draft) {
        self.mirror = draft.layout.is_duel();
        self.cursor = draft.layout.guns.first().copied().unwrap_or(0);
        self.edit(None, |e| e.draft = draft);
    }

    /// UNDO: back one step. False if there is none.
    pub fn undo(&mut self) -> bool {
        self.end_stroke();
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.draft, prev));
        self.group = None;
        self.changed();
        true
    }

    /// REDO: forward one step undone. False if there is none.
    pub fn redo(&mut self) -> bool {
        self.end_stroke();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.draft, next));
        self.group = None;
        self.changed();
        true
    }

    /// Choose a tool. Ends any stroke and any run of grouped edits.
    pub fn set_tool(&mut self, tool: Tool) {
        self.end_stroke();
        self.group = None;
        self.tool = tool;
    }

    /// MIRROR: on makes the draft symmetric, as one undoable edit (see
    /// "Mirror"); off changes nothing but how later edits are made.
    pub fn toggle_mirror(&mut self) {
        self.end_stroke();
        self.group = None;
        self.mirror = !self.mirror;
        if self.mirror {
            self.edit(None, |e| {
                let l = &mut e.draft.layout;
                l.mirror_left();
                if l.guns.len() == 2 {
                    // The left gun stays, pulled back if its mirror would
                    // stand too close.
                    let n = l.width() as u16;
                    l.guns[0] = l.guns[0].min((n - 1 - MIN_GUN_GAP) / 2);
                    l.guns[1] = n - 1 - l.guns[0];
                }
            });
        }
    }

    /// The brush's weight at `d` columns from its centre: 1 at the centre,
    /// falling smoothly to nothing past its radius.
    fn weight(&self, d: i32) -> f64 {
        let r = self.brush.radius() as f64 + 1.0;
        dmath::bump(d as f64, r)
    }

    /// Weights over every column for a brush at `x`, mirrored if MIRROR is
    /// on (the larger of a column's and its mirror's).
    fn brush_weights(&self, x: i32) -> Vec<f64> {
        self.sweep_weights(x, x)
    }

    /// Weights over every column for a brush swept from column `a` to `b`:
    /// each column's weight at its nearest point of the sweep.
    fn sweep_weights(&self, a: i32, b: i32) -> Vec<f64> {
        let n = self.draft.layout.width();
        let (lo, hi) = (a.min(b), a.max(b));
        let mut w: Vec<f64> = (0..n as i32)
            .map(|c| self.weight(if c < lo { lo - c } else { (c - hi).max(0) }))
            .collect();
        if self.mirror {
            for c in 0..n / 2 {
                let m = w[c].max(w[n - 1 - c]);
                w[c] = m;
                w[n - 1 - c] = m;
            }
        }
        w
    }

    /// A column under the finger at buffer `x`, snapped to [`COLUMN_SNAP`]
    /// and kept on the layout.
    fn column(&self, x: f32) -> i32 {
        let n = self.draft.layout.width() as i32;
        let snap = COLUMN_SNAP as f32;
        (((x / snap).round() * snap) as i32).clamp(0, n - 1)
    }

    /// A finger went down at `(x, y)` (buffer pixels, the layout's column
    /// and a row from the top) with the current tool: the start of a
    /// stroke. WIND takes no strokes.
    pub fn begin_stroke(&mut self, x: f32, y: f32) {
        self.end_stroke();
        self.group = None;
        let n = self.draft.layout.width();
        let kind = match self.tool {
            Tool::Sculpt => StrokeKind::Sculpt {
                peak: vec![0.0; n],
                last: self.column(x),
            },
            Tool::Smooth => StrokeKind::Smooth,
            Tool::Stamp => StrokeKind::Stamp,
            Tool::Guns => {
                let guns = &self.draft.layout.guns;
                // The gun nearer the finger.
                let nearest = guns
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &g)| (g as f32 - x).abs() as i32)
                    .map_or(0, |(i, _)| i);
                StrokeKind::Gun(nearest)
            }
            Tool::Wind => return,
        };
        self.stroke = Some(Stroke {
            base: self.draft.clone(),
            start: (x, y),
            kind,
        });
        self.drag_stroke(x, y);
    }

    /// The finger of the stroke in progress is at `(x, y)`.
    pub fn drag_stroke(&mut self, x: f32, y: f32) {
        let Some(mut stroke) = self.stroke.take() else {
            return;
        };
        let before = self.draft.layout.clone();
        let col = self.column(x);
        match &mut stroke.kind {
            StrokeKind::Sculpt { peak, last } => {
                for (p, w) in peak.iter_mut().zip(self.sweep_weights(*last, col)) {
                    *p = p.max(w);
                }
                *last = col;
                // Up the screen is up the world: a buffer pixel is a world
                // unit, and the ground moves in whole ones.
                let units = (stroke.start.1 - y).round() as i32;
                let base = &stroke.base.layout.heights;
                let one = height_steps(1.0) as i32;
                for (c, h) in self.draft.layout.heights.iter_mut().enumerate() {
                    let lift = dmath::round(units as f64 * peak[c]) as i32;
                    *h = clamp_steps(base[c] as i32 + lift * one);
                }
            }
            StrokeKind::Smooth => self.smooth_at(col),
            StrokeKind::Stamp => {
                self.draft.layout.heights = stroke.base.layout.heights.clone();
                self.stamp_at(col);
            }
            StrokeKind::Gun(i) => {
                // With MIRROR on the left gun leads and the right follows,
                // whichever was grabbed: a grabbed right gun follows the
                // finger by moving the left one to its mirror.
                let x = x.round() as i32;
                let n = self.draft.layout.width() as i32;
                match (*i, self.mirror) {
                    (1, true) => self.move_gun(0, n - 1 - x),
                    (i, _) => self.move_gun(i, x),
                }
            }
        }
        self.stroke = Some(stroke);
        if self.draft.layout != before {
            self.changed();
        }
    }

    /// The finger lifted: the stroke is kept as one undo step, or forgotten
    /// if it changed nothing.
    pub fn end_stroke(&mut self) {
        let Some(stroke) = self.stroke.take() else {
            return;
        };
        if stroke.base.layout != self.draft.layout {
            self.push_undo(stroke.base);
            self.group = None;
            self.verdict = verdict(&self.draft.layout);
        }
    }

    /// The stroke is abandoned (the screen changed under it, focus lost):
    /// the draft goes back to how the stroke found it.
    pub fn cancel_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            if stroke.base != self.draft {
                self.draft = stroke.base;
                self.changed();
            }
        }
    }

    /// One pass of a (1, 2, 1) smoothing under the brush at `col`, each
    /// column moved by the brush's weight of the way to its smoothed height.
    fn smooth_at(&mut self, col: i32) {
        let w = self.brush_weights(col);
        let h: Vec<i32> = self
            .draft
            .layout
            .heights
            .iter()
            .map(|&v| v as i32)
            .collect();
        let n = h.len();
        for c in 0..n {
            if w[c] <= 0.0 {
                continue;
            }
            let (l, r) = (h[c.saturating_sub(1)], h[(c + 1).min(n - 1)]);
            let avg = (l + 2 * h[c] + r) as f64 / 4.0;
            let moved = dmath::round(w[c] * (avg - h[c] as f64)) as i32;
            self.draft.layout.heights[c] = clamp_steps(h[c] + moved);
        }
    }

    /// Add the current stamp's shape at `col` (and at its mirror, with
    /// MIRROR on: the left half's shape copied to the right).
    fn stamp_at(&mut self, col: i32) {
        let n = self.draft.layout.width();
        let x = if self.mirror {
            col.min(n as i32 - 1 - col)
        } else {
            col
        };
        let delta = self.stamp.delta(x, n);
        let h = &mut self.draft.layout.heights;
        if self.mirror {
            for (c, d) in delta.iter().enumerate().take(n / 2) {
                for at in [c, n - 1 - c] {
                    h[at] = clamp_steps(h[at] as i32 + d);
                }
            }
            if n % 2 == 1 {
                h[n / 2] = clamp_steps(h[n / 2] as i32 + delta[n / 2]);
            }
        } else {
            for (v, d) in h.iter_mut().zip(delta) {
                *v = clamp_steps(*v as i32 + d);
            }
        }
    }

    /// Move gun `i` toward column `want`, as far as it can go while the
    /// guns stay on screen, at least [`MIN_GUN_GAP`] apart and seated
    /// (§13.5): it stops at the last column it can stand on. With MIRROR
    /// on the other gun follows as its mirror.
    pub fn move_gun(&mut self, i: usize, want: i32) {
        let l = &self.draft.layout;
        if l.guns.len() != 2 || i > 1 {
            return;
        }
        let n = l.width() as i32;
        let margin = GUN_EDGE_MARGIN as i32;
        let gap = MIN_GUN_GAP as i32;
        let (lo, hi) = match (i, self.mirror) {
            (0, true) => (margin, (n - 1 - gap) / 2),
            (0, false) => (margin, l.guns[1] as i32 - gap),
            (_, _) => (l.guns[0] as i32 + gap, n - 1 - margin),
        };
        let want = want.clamp(lo, hi.max(lo));
        let from = l.guns[i] as i32;
        let mut at = from;
        let step = (want - at).signum();
        while at != want {
            let next = at + step;
            if !self.gun_stands(i, next) {
                break;
            }
            at = next;
        }
        // A gun that could not move stays as it is, and so does the other:
        // an undo can bring back guns MIRROR did not place.
        if at != from {
            self.place_gun(i, at);
        }
    }

    /// The guns as they would be with gun `i` at column `at`.
    fn guns_with(&self, i: usize, at: i32) -> Vec<u16> {
        let n = self.draft.layout.width() as i32;
        let mut guns = self.draft.layout.guns.clone();
        guns[i] = at as u16;
        if self.mirror {
            guns[1 - i] = (n - 1 - at) as u16;
        }
        guns
    }

    fn place_gun(&mut self, i: usize, at: i32) {
        self.draft.layout.guns = self.guns_with(i, at);
        self.cursor = self.draft.layout.guns[0];
    }

    /// Whether gun `i` could stand at `at`: no placement rule about guns
    /// broken there that is not already broken where they stand.
    fn gun_stands(&self, i: usize, at: i32) -> bool {
        let gun_rules = |l: &Layout| -> Vec<Problem> {
            check_placement(l)
                .into_iter()
                .filter(|p| {
                    matches!(
                        p,
                        Problem::GunOffScreen(_)
                            | Problem::GunNotSeated(_)
                            | Problem::GunsTooClose
                            | Problem::GunsOutOfOrder
                    )
                })
                .collect()
        };
        let mut moved = self.draft.layout.clone();
        moved.guns = self.guns_with(i, at);
        let now = gun_rules(&self.draft.layout);
        gun_rules(&moved).iter().all(|p| now.contains(p))
    }

    /// Keys and a controller: move the cursor by `d` columns, kept on the
    /// layout. Under GUNS the cursor is the left gun, which moves with it.
    pub fn move_cursor(&mut self, d: i32) {
        self.end_stroke();
        let n = self.draft.layout.width() as i32;
        if self.tool == Tool::Guns {
            let g = self.draft.layout.guns.first().copied().unwrap_or(0) as i32;
            self.edit(Some(Group::Nudge(Tool::Guns, 0)), |e| e.move_gun(0, g + d));
            return;
        }
        self.cursor = (self.cursor as i32 + d).clamp(0, n - 1) as u16;
    }

    /// Keys and a controller: the current tool once at the cursor, `dir`
    /// up (+1) or down (-1). SCULPT raises or lowers the ground under the
    /// brush by a whole unit; SMOOTH smooths once; STAMP drops a shape.
    /// A run of nudges at one spot is one undo step.
    pub fn nudge(&mut self, dir: i32) {
        let col = self.cursor as i32;
        let group = match self.tool {
            Tool::Stamp => None,
            tool => Some(Group::Nudge(tool, self.cursor)),
        };
        self.edit(group, |e| match e.tool {
            Tool::Sculpt => {
                let w = e.brush_weights(col);
                let one = height_steps(1.0) as i32;
                for (c, h) in e.draft.layout.heights.iter_mut().enumerate() {
                    let lift = dmath::round(dir as f64 * w[c]) as i32;
                    *h = clamp_steps(*h as i32 + lift * one);
                }
            }
            Tool::Smooth => e.smooth_at(col),
            Tool::Stamp => e.stamp_at(col),
            Tool::Guns | Tool::Wind => {}
        });
    }

    /// WIND: CALM, RANGE or FIXED (`wind_as`). One undo step with the
    /// presses around it.
    pub fn set_wind_mode(&mut self, mode: WindMode) {
        let spec = wind_as(self.draft.layout.wind, mode);
        self.wind_field = match mode {
            WindMode::Fixed => WindField::Fixed,
            _ => WindField::Max,
        };
        self.set_wind(spec);
    }

    /// WIND `-`/`+`: move the chosen number `steps` tenths ([`step_wind`]).
    pub fn step_wind(&mut self, steps: i32) {
        let spec = step_wind(self.draft.layout.wind, self.wind_field, steps);
        self.set_wind(spec);
    }

    fn set_wind(&mut self, spec: WindSpec) {
        self.edit(Some(Group::Wind), |e| e.draft.layout.wind = spec);
    }

    /// NAME: the name as typed so far, if it is one (keypad characters, at
    /// most `NAME_MAX`). A run of typing is one undo step.
    pub fn set_name(&mut self, text: &str) -> bool {
        let Some(name) = LayoutName::parse(text) else {
            return false;
        };
        self.edit(Some(Group::Name), |e| e.draft.layout.name = name);
        true
    }

    /// The draft was saved to `slot`: it now belongs there. Not an edit.
    pub fn saved_to(&mut self, slot: u8) {
        self.draft.slot = Some(slot);
        self.rev += 1;
    }

    /// The winds TEST FIRE offers (§13.11): calm, and every wind the duel
    /// gate checks ([`custom::duel_winds`]), weakest to strongest, left
    /// before right, in thousandths.
    pub fn test_winds(&self) -> Vec<i32> {
        let mut winds = custom::duel_winds(&self.draft.layout.wind);
        winds.push(0);
        winds.sort_unstable();
        winds.dedup();
        winds
    }
}

/// A height in sixteenths as world units (re-exported for the renderer's
/// cursor readout).
pub fn units(h: u16) -> f64 {
    height_units(h)
}

/// The layout's ground as the duel deals it, for drawing.
pub fn screen_of(l: &Layout) -> Screen {
    Screen::from_layout(l, 0.0)
}

/// Re-exported for the app's checks.
pub use layout::NAME_MAX;

#[cfg(test)]
mod tests;

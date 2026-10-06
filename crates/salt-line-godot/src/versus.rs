//! Local versus: two emplacements on one duel screen, hot-seat on one device.
//!
//! The rules of a match, with no engine in it: how a duel screen is built,
//! whose turn it is, what counts as a hit, and when a round or the match is
//! won. The app (`app/versus.rs`) paces it and the renderer
//! (`render/versus.rs`) draws it; the sim knows nothing of any of this.
//!
//! # One sim, two directions: the mirrored frame
//!
//! The sim only fires rightward: a [`Shot`]'s angle runs from flat-right to
//! straight up, the emplacement sits at [`EMPLACEMENT_X`] on the left, and the
//! solver, the integrator and every determinism test are built that way. The
//! right-hand player needs to fire leftward. Rather than teach the sim a
//! second direction, each shot is resolved in the **shooter's frame**: the
//! shared screen as that player would see it if they stood on the left.
//!
//! ```text
//!   shared frame (what is drawn)        right player's frame (what is fired)
//!   L ......../\......... R             R ........./\........ L
//!   wind  -->                           wind  <--
//!   x                                   width - 1 - x
//! ```
//!
//! For the left player the shooter's frame *is* the shared frame. For the
//! right player it is [`frame_for`]: the heightfield reversed column for
//! column, the wind's sign flipped, and the right gun now standing at
//! `EMPLACEMENT_X`. The shot is fired, craters are dug into that mirrored
//! heightfield, and the result is reversed back ([`mirror_field`]). Reversing
//! an array is exact, so the shared screen gets back precisely the craters
//! the shot dug, with no floating-point drift, and mirroring twice is the
//! identity bit for bit.
//!
//! Why not a small sim change instead (a sign on the angle, say)? The sim's
//! determinism is guarded by golden runs and an epoch digest; any change
//! inside the integrator puts that at risk for a mode those digests do not
//! cover. Mirroring keeps every line of `ballistics` and `solver` exactly as
//! the single-player game runs it, and confines the cost to copying a few
//! hundred floats per shot.
//!
//! # The duel screen
//!
//! [`generate_duel`] reuses the terrain generator (DESIGN §2.3): base ridge,
//! domain warp, a shape template laid out from one gun to the other, then
//! blended toward its own mirror image so neither side gets the easier
//! ground ([`NEAR_SYMMETRY`]). It is gated like a single-player screen
//! (DESIGN §2.4): each gun needs at least `MIN_SOLUTIONS` distinct shots
//! that hit the other, checked at both extremes of the wind and in calm air,
//! with the same retry-then-fall-back ladder.
//!
//! # A match
//!
//! [`Match`] is the state machine. Players alternate shots; a hit is any
//! detonation whose blast overlaps the other emplacement ([`hits`]), the
//! same overlap test a blast uses against a structure (DESIGN §2.7). First
//! to [`HITS_TO_WIN_ROUND`] hits takes the round, first to
//! [`ROUNDS_TO_WIN_MATCH`] rounds takes the match. Wind is drawn afresh for
//! every turn from the match seed and the turn number ([`turn_wind`]);
//! craters stay for the round, and every round is a new screen.
//!
//! ```text
//! Aiming --fire, no round won--------> Aiming (other side, new wind)
//! Aiming --fire, round won-----------> RoundOver(side)
//! Aiming --fire, match won-----------> MatchOver(side)
//! RoundOver --next_round-------------> Aiming (new screen)
//! ```
//!
//! Everything is deterministic from the match seed and the shots, like a
//! run, but nothing here is logged or saved: a versus match is not resumed
//! after the app is killed and never reaches the board.

use salt_line_sim::ballistics::{self, blast_radius, Aabb, End, FireOptions, Shot, Trace, World};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::rng::{screen_seed, substream, Pcg32};
use salt_line_sim::run::{self, Rung, Screen};
use salt_line_sim::solver::{self, GATE_CAP};
use salt_line_sim::terrain::{self, Heightfield, ShapeParams, Template};
use salt_line_sim::{Vec2, EMPLACEMENT_X, MAX_GEN_ATTEMPTS, MIN_SOLUTIONS};

/// Hits that win a round.
pub const HITS_TO_WIN_ROUND: u32 = 3;
/// Rounds that win the match: best of three.
pub const ROUNDS_TO_WIN_MATCH: u32 = 2;

/// Strongest wind a turn can draw, world units per second squared, either
/// way. About band 7 of a run (DESIGN §2.5): enough that it has to be read,
/// not so much that a ten-year-old's first shots all blow away.
pub const WIND_MAX: f64 = 2.5;

/// The band whose terrain amplitude duel screens use (DESIGN §2.5): a
/// little more relief than the opening screens, well short of the late game.
pub const DUEL_BAND: u32 = 4;

/// How far each column is pulled toward its mirror image, from 0 (the
/// generated terrain untouched) to 0.5 (perfectly symmetric). Just short of
/// 0.5, so the two sides are equally hard without being the same picture
/// twice: what is left is a tenth of the original difference.
pub const NEAR_SYMMETRY: f64 = 0.45;

/// The templates a duel draws from, with their weights. `Overhang` is left
/// out: it has no direct answer by construction (DESIGN §2.4) and the
/// versus loadout holds nothing that solves it.
pub const TEMPLATES: [(Template, u32); 6] = [
    (Template::Flat, 18),
    (Template::Ridge, 26),
    (Template::Chasm, 12),
    (Template::Mesa, 14),
    (Template::Spires, 16),
    (Template::Basin, 14),
];

/// The emplacement's box around its muzzle, in world units: left, right,
/// below and above. Measured off `tank_base.png` as the renderer seats it:
/// the 26-pixel chassis is centred on the barrel pivot (`PIVOT_BASE` is two
/// columns of padding plus 13), with three rows above the pivot and nine
/// below, the last two sunk into the ground.
pub const TANK_BOX: (f64, f64, f64, f64) = (13.0, 13.0, 9.0, 3.0);

// Substream tags, ASCII: "DUEL", "VTPL", "VSHP", "VWND".
const TAG_DUEL: u64 = 0x4455_454C;
const TAG_TEMPLATE: u64 = 0x5654_504C;
const TAG_SHAPE: u64 = 0x5653_4850;
const TAG_WIND: u64 = 0x5657_4E44;

// ------------------------------------------------------------------- sides

/// Which gun. Left fires rightward in the shared frame, right fires leftward.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Both sides, left first. `index` is the position in this array.
    pub const BOTH: [Side; 2] = [Side::Left, Side::Right];

    /// The other gun.
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /// 0 for left, 1 for right: an index into the per-side arrays.
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    /// How the screens name the side.
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        }
    }
}

// --------------------------------------------------------------- mirroring

/// An x in one frame as the other frame has it. Column `i` of a
/// `width`-column field is column `width - 1 - i` of its mirror, so this is
/// a reflection about the field's middle, and its own inverse.
#[inline]
pub fn mirror_x(x: f64, width: usize) -> f64 {
    (width as f64 - 1.0) - x
}

/// A point reflected left to right; y is unchanged.
#[inline]
pub fn mirror_point(p: Vec2, width: usize) -> Vec2 {
    Vec2::new(mirror_x(p.x, width), p.y)
}

/// A box reflected left to right. Its left edge becomes its right edge, so
/// the corners swap to keep `x0 <= x1`.
pub fn mirror_box(b: Aabb, width: usize) -> Aabb {
    Aabb::new(mirror_x(b.x1, width), b.y0, mirror_x(b.x0, width), b.y1)
}

/// A heightfield reversed column for column: base, scar and material alike.
/// Exact, since nothing is computed, only moved.
pub fn mirror_field(f: &Heightfield) -> Heightfield {
    let rev = |v: &[f64]| v.iter().rev().copied().collect::<Vec<f64>>();
    Heightfield {
        base: rev(&f.base),
        scar: rev(&f.scar),
        material: f.material.iter().rev().copied().collect(),
    }
}

/// How a flight ended, reflected. Structure indices are kept: there is only
/// ever one target box in a duel.
pub fn mirror_end(e: End, width: usize) -> End {
    let m = |p| mirror_point(p, width);
    match e {
        End::Terrain { pos, material } => End::Terrain {
            pos: m(pos),
            material,
        },
        End::Structure { index, pos } => End::Structure { index, pos: m(pos) },
        End::Fuse { pos } => End::Fuse { pos: m(pos) },
        End::OffScreen { pos } => End::OffScreen { pos: m(pos) },
        End::Timeout { pos } => End::Timeout { pos: m(pos) },
    }
}

/// A whole flight reflected, path and all, so the renderer can draw a right
/// player's shot in the shared frame.
pub fn mirror_trace(t: &Trace, width: usize) -> Trace {
    Trace {
        origin: mirror_point(t.origin, width),
        end: mirror_end(t.end, width),
        apex: mirror_point(t.apex, width),
        path: t.path.iter().map(|p| mirror_point(*p, width)).collect(),
        ..t.clone()
    }
}

/// The shared screen as `side` fires in it: its own gun at
/// [`EMPLACEMENT_X`], the other gun to the right. For the left player that is
/// the shared screen itself; for the right player the field is reversed and
/// the wind turned round. `shared.wind` is the shared frame's wind.
pub fn frame_for(shared: &Screen, side: Side) -> Screen {
    let mut frame = shared.clone();
    if side == Side::Right {
        frame.field = mirror_field(&shared.field);
        frame.wind = -shared.wind;
        frame.wind_late = shared.wind_late.map(|w| -w);
    }
    frame
}

/// The box of the gun standing at the left of `frame`, around its muzzle.
fn own_box(frame: &Screen) -> Aabb {
    let m = frame.muzzle();
    let (left, right, below, above) = TANK_BOX;
    Aabb::new(m.x - left, m.y - below, m.x + right, m.y + above)
}

/// Where `side`'s barrel pivot is, in the shared frame. The gun is seated on
/// the ground as it stands now, so cratering under a gun lowers it.
pub fn muzzle_of(shared: &Screen, side: Side) -> Vec2 {
    match side {
        Side::Left => shared.muzzle(),
        Side::Right => mirror_point(frame_for(shared, side).muzzle(), shared.width),
    }
}

/// `side`'s emplacement box in the shared frame.
pub fn emplacement_box(shared: &Screen, side: Side) -> Aabb {
    match side {
        Side::Left => own_box(shared),
        Side::Right => mirror_box(own_box(&frame_for(shared, side)), shared.width),
    }
}

/// The box `shooter` is aiming at, in the shooter's own frame: the other
/// gun's box, reflected if the shooter is on the right.
pub fn target_in_frame(shared: &Screen, shooter: Side) -> Aabb {
    let target = emplacement_box(shared, shooter.other());
    match shooter {
        Side::Left => target,
        Side::Right => mirror_box(target, shared.width),
    }
}

// --------------------------------------------------------------- the shot

/// Did this flight hit the target box? A shell that ended in the box hit it
/// outright; any other detonation hits if its blast reaches the box, the
/// same overlap a blast needs to damage a structure (DESIGN §2.7:
/// `overlap = 1 - distance / blast`, so any overlap at all is a distance
/// short of the blast radius). A shell that left the screen hit nothing.
pub fn hits(trace: &Trace, target: Aabb, lo: &Loadout) -> bool {
    match trace.end {
        End::Structure { .. } => true,
        end if end.detonates() => {
            target.distance_to(end.pos()) < blast_radius(lo, trace.damage_scale)
        }
        _ => false,
    }
}

/// Fly `shot` from `frame`'s gun with `target` as the only box it can strike,
/// and touch nothing. The traces are in `frame`'s coordinates.
pub fn fly(
    frame: &Screen,
    target: Aabb,
    shot: Shot,
    lo: &Loadout,
    record_path: bool,
) -> Vec<Trace> {
    let targets = [target];
    let world = World {
        field: &frame.field,
        targets: &targets,
        width: frame.width,
        wind: frame.wind,
    };
    let opts = FireOptions {
        record_path,
        ..FireOptions::default()
    };
    ballistics::fire(&world, frame.muzzle(), shot, lo, &opts)
}

/// What one turn's shot did, in the shared frame.
#[derive(Clone, Debug)]
pub struct TurnReport {
    /// Who fired.
    pub shooter: Side,
    /// The shell, then any submunitions (none in versus), reflected into
    /// the shared frame.
    pub traces: Vec<Trace>,
    /// The other gun was hit.
    pub hit: bool,
    /// The shell missed the other gun but its blast reached the shooter's
    /// own. It scores nothing (DESIGN §11.1); the screens call it out so a
    /// player is not told MISS for a shell that landed on themselves.
    pub self_hit: bool,
    /// Where the shell detonated, if it did: the landing mark aim help shows.
    pub landing: Option<Vec2>,
}

/// Every distinct shot `frame`'s gun can dial in that hits `target`, up to
/// `cap`. Candidates come from the solver's closed form (DESIGN §2.4), aimed
/// at the box centre and just under its top; each is then flown by the
/// integrator and kept only if [`hits`] says so, so a candidate is exactly a
/// shot the player could fire.
pub fn shots_at(frame: &Screen, target: Aabb, lo: &Loadout, cap: usize) -> Vec<Shot> {
    let c = target.centre();
    let aims = [c, Vec2::new(c.x, target.y1 - 1.0)];
    let mut seen: Vec<Shot> = Vec::new();
    let mut out: Vec<Shot> = Vec::new();
    for aim in aims {
        for (shot, _) in solver::analytic_shots(frame.muzzle(), aim, frame.wind * lo.wind_scale, lo)
        {
            if seen.contains(&shot) {
                continue;
            }
            seen.push(shot);
            if fly(frame, target, shot, lo, false)
                .iter()
                .any(|t| hits(t, target, lo))
            {
                out.push(shot);
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------- generation

/// The seed a round's screen and winds come from: one stream per match,
/// split by round the way a run is split by screen (DESIGN §2.2).
pub fn round_seed(match_seed: u64, round: u32) -> u64 {
    screen_seed(substream(match_seed, TAG_DUEL), round)
}

/// The wind for one turn of one round, in the shared frame, uniform in
/// `[-WIND_MAX, WIND_MAX)`. A pure function of the match seed, the round and
/// the turn, so the same match plays the same winds whatever happened
/// before.
pub fn turn_wind(match_seed: u64, round: u32, turn: u32) -> f64 {
    let stream = substream(
        substream(round_seed(match_seed, round), TAG_WIND),
        turn as u64,
    );
    Pcg32::new(stream).signed(WIND_MAX)
}

/// Pull every column toward its mirror image by [`NEAR_SYMMETRY`]. Both
/// columns of a pair move toward their mean, so the field stays inside the
/// world bounds the generator clamped it to.
pub fn symmetrize(heights: &mut [f64]) {
    let original = heights.to_vec();
    let n = original.len();
    for (i, h) in heights.iter_mut().enumerate() {
        *h = original[i] * (1.0 - NEAR_SYMMETRY) + original[n - 1 - i] * NEAR_SYMMETRY;
    }
}

/// One candidate duel screen: terrain laid out from the left gun to the
/// right one, made near-symmetric, with no structures and calm air (the
/// match sets each turn's wind).
fn assemble(round: u32, seed: u64, width: usize, template: Template, level: bool) -> Screen {
    let mut r_shape = Pcg32::new(substream(seed, TAG_SHAPE));
    let spire_count = r_shape.range_u32(2, 3);
    let base = if level {
        vec![terrain::BASE_ELEVATION; width]
    } else {
        let p = ShapeParams {
            width,
            amp: run::terrain_amplitude(DUEL_BAND),
            muzzle_x: EMPLACEMENT_X,
            target_x: mirror_x(EMPLACEMENT_X, width),
            // Level from gun to gun: neither side starts above the other.
            elev_delta: 0.0,
            spire_count,
        };
        let mut base = terrain::base_ridge(width, seed, p.amp);
        terrain::shape(&mut base, template, &p);
        symmetrize(&mut base);
        base
    };
    Screen {
        index: round,
        seed,
        width,
        template,
        field: Heightfield::from_base(base),
        structures: Vec::new(),
        wind: 0.0,
        wind_late: None,
        muzzle_x: EMPLACEMENT_X,
        candidates: 0,
        attempts: 0,
        fell_back: false,
        rung: Some(Rung::Drawn),
        supply: false,
    }
}

/// The fewest hitting shots either gun has against the other, over calm air
/// and both extremes of [`WIND_MAX`], counting up to [`GATE_CAP`].
pub fn duel_candidates(shared: &Screen, lo: &Loadout) -> usize {
    let mut worst = usize::MAX;
    for wind in [-WIND_MAX, 0.0, WIND_MAX] {
        let mut s = shared.clone();
        s.wind = wind;
        for side in Side::BOTH {
            let frame = frame_for(&s, side);
            let n = shots_at(&frame, target_in_frame(&s, side), lo, GATE_CAP).len();
            worst = worst.min(n);
            if worst < MIN_SOLUTIONS {
                return worst;
            }
        }
    }
    worst
}

/// Gate a candidate screen and record its count. True if both guns have
/// `MIN_SOLUTIONS` shots at each other in every wind checked.
fn gate(screen: &mut Screen, lo: &Loadout) -> bool {
    screen.candidates = duel_candidates(screen, lo);
    screen.candidates >= MIN_SOLUTIONS
}

/// Build round `round`'s duel screen at buffer width `width`.
///
/// The single-player ladder, reused (DESIGN §2.3 step 6, §2.4): up to
/// `MAX_GEN_ATTEMPTS` drawn templates on successive seeds, then as many
/// `Flat` screens on a separate run of seeds, then dead level ground. Every
/// rung passes the same gate. Level ground with nothing between two guns
/// about 260 to 340 units apart, well inside the base loadout's range, is
/// solvable by construction; debug and test builds panic if it ever is not.
pub fn generate_duel(match_seed: u64, round: u32, width: usize, lo: &Loadout) -> Screen {
    let seed0 = round_seed(match_seed, round);
    let mut r_template = Pcg32::new(substream(seed0, TAG_TEMPLATE));
    let weights: Vec<u32> = TEMPLATES.iter().map(|(_, w)| *w).collect();
    for attempt in 0..MAX_GEN_ATTEMPTS {
        let template = TEMPLATES[r_template.weighted(&weights)].0;
        let seed = seed0.wrapping_add(attempt as u64);
        let mut screen = assemble(round, seed, width, template, false);
        screen.attempts = attempt + 1;
        if gate(&mut screen, lo) {
            return screen;
        }
    }
    for attempt in 0..MAX_GEN_ATTEMPTS {
        let seed = seed0.wrapping_add(0x1000 + attempt as u64);
        let mut screen = assemble(round, seed, width, Template::Flat, false);
        screen.attempts = MAX_GEN_ATTEMPTS + attempt + 1;
        screen.fell_back = true;
        screen.rung = Some(Rung::Flat);
        if gate(&mut screen, lo) {
            return screen;
        }
    }
    let mut screen = assemble(round, seed0, width, Template::Flat, true);
    screen.attempts = MAX_GEN_ATTEMPTS * 2 + 1;
    screen.fell_back = true;
    screen.rung = Some(Rung::Level);
    if !gate(&mut screen, lo) && cfg!(debug_assertions) {
        panic!(
            "unsolvable duel: match seed {match_seed:#x}, round {round}, width {width}: \
             level ground has only {} shots",
            screen.candidates
        );
    }
    screen
}

// ------------------------------------------------------------- the match

/// Where a match is. Each variant names the call that moves it on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Waiting for `to_move` to fire ([`Match::fire`]).
    Aiming,
    /// The side won the round; [`Match::next_round`] deals a new screen.
    RoundOver(Side),
    /// The side won the match. Terminal.
    MatchOver(Side),
}

/// A versus match: the state machine in the module docs.
///
/// Fields are public for the app and renderer to read; the methods are the
/// way to change a match, in the same spirit as the sim's `RunState`.
#[derive(Clone, Debug)]
pub struct Match {
    /// Everything about the match follows from this and the shots.
    pub seed: u64,
    /// The base loadout, for both guns: unlimited shells, no drafts.
    pub loadout: Loadout,
    /// Round being played, from 0.
    pub round: u32,
    /// Shots fired this round; the wind's turn number.
    pub turn: u32,
    /// Whose shot it is.
    pub to_move: Side,
    /// Hits this round, by [`Side::index`].
    pub hits: [u32; 2],
    /// Rounds won, by [`Side::index`].
    pub rounds: [u32; 2],
    /// The duel screen in the shared frame, craters and all. `wind` is this
    /// turn's wind, in the shared frame.
    pub screen: Screen,
    /// Where each side's last shell landed this round, in the shared frame.
    pub landing: [Option<Vec2>; 2],
    /// Where the match is.
    pub phase: Phase,
}

impl Match {
    /// A fresh match on `seed` at buffer width `width`: round 1, left to
    /// fire first.
    pub fn new(seed: u64, width: usize) -> Match {
        let loadout = Loadout::new();
        let mut screen = generate_duel(seed, 0, width, &loadout);
        screen.wind = turn_wind(seed, 0, 0);
        Match {
            seed,
            loadout,
            round: 0,
            turn: 0,
            to_move: Match::first_to_move(0),
            hits: [0, 0],
            rounds: [0, 0],
            screen,
            landing: [None, None],
            phase: Phase::Aiming,
        }
    }

    /// Who opens a round: the sides take turns, left opening the first.
    pub fn first_to_move(round: u32) -> Side {
        if round.is_multiple_of(2) {
            Side::Left
        } else {
            Side::Right
        }
    }

    /// The width the match was generated at.
    pub fn width(&self) -> usize {
        self.screen.width
    }

    /// Fire `to_move`'s shot, resolve it, and move the match on: score a
    /// hit, end the round or the match, or pass the turn with a new wind.
    /// `None` outside `Phase::Aiming`.
    ///
    /// The shot is resolved in the shooter's frame ([`frame_for`]) and its
    /// craters and traces are reflected back into the shared frame.
    pub fn fire(&mut self, shot: Shot, record_path: bool) -> Option<TurnReport> {
        if self.phase != Phase::Aiming {
            return None;
        }
        let shooter = self.to_move;
        let width = self.width();
        let lo = self.loadout.clone();
        let own_box = emplacement_box(&self.screen, shooter);
        let target = target_in_frame(&self.screen, shooter);
        let mut frame = frame_for(&self.screen, shooter);
        let traces = fly(&frame, target, shot.quantized(&lo), &lo, record_path);
        let hit = traces.iter().any(|t| hits(t, target, &lo));
        for t in &traces {
            run::crater_detonation(&mut frame.field, t.end, t.damage_scale, &lo);
        }
        let (field, traces) = match shooter {
            Side::Left => (frame.field, traces),
            Side::Right => (
                mirror_field(&frame.field),
                traces.iter().map(|t| mirror_trace(t, width)).collect(),
            ),
        };
        self.screen.field = field;
        let landing = traces.first().and_then(Trace::detonation);
        self.landing[shooter.index()] = landing;
        // In the shared frame, where both the traces and the box now are.
        // A shell that ended on a structure ended on the other gun, the
        // only box it can strike, so a self-hit is never also a hit.
        let self_hit = !hit && traces.iter().any(|t| hits(t, own_box, &lo));

        if hit {
            self.hits[shooter.index()] += 1;
        }
        if self.hits[shooter.index()] >= HITS_TO_WIN_ROUND {
            self.rounds[shooter.index()] += 1;
            self.phase = if self.rounds[shooter.index()] >= ROUNDS_TO_WIN_MATCH {
                Phase::MatchOver(shooter)
            } else {
                Phase::RoundOver(shooter)
            };
        } else {
            self.turn += 1;
            self.to_move = shooter.other();
            self.screen.wind = turn_wind(self.seed, self.round, self.turn);
        }
        Some(TurnReport {
            shooter,
            traces,
            hit,
            self_hit,
            landing,
        })
    }

    /// Deal the next round: a new screen, hits back to zero, the other side
    /// opening. Returns false unless a round has just ended.
    pub fn next_round(&mut self) -> bool {
        if !matches!(self.phase, Phase::RoundOver(_)) {
            return false;
        }
        self.round += 1;
        self.turn = 0;
        self.hits = [0, 0];
        self.landing = [None, None];
        self.to_move = Match::first_to_move(self.round);
        self.screen = generate_duel(self.seed, self.round, self.width(), &self.loadout);
        self.screen.wind = turn_wind(self.seed, self.round, 0);
        self.phase = Phase::Aiming;
        true
    }
}

#[cfg(test)]
mod tests;

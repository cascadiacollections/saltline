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
//! A custom duel (DESIGN §13.9, `custom.rs`) plays every round on one
//! layout's ground instead: the match holds it as a [`Ground`] and deals it
//! fresh each round.
//!
//! # A match
//!
//! [`Match`] is the state machine. Players alternate shots; a hit is any
//! detonation whose blast overlaps the other emplacement ([`hits`]), the
//! same overlap test a blast uses against a structure (DESIGN §2.7). First
//! to [`HITS_TO_WIN_ROUND`] hits takes the round, but the side that did not
//! open gets an answering shot whenever a hit could still tie
//! ([`round_winner`]), and first to
//! [`ROUNDS_TO_WIN_MATCH`] rounds takes the match. Wind is drawn afresh for
//! every turn from the match seed and the turn number ([`turn_wind`]);
//! craters stay for the round, and every round is a new screen.
//!
//! ```text
//! Aiming --fire, no round won--------> Aiming (other side, new wind;
//!                                       the guns roll if a pair had a hit)
//! Aiming --fire, round won-----------> RoundOver(side)
//! Aiming --fire, match won-----------> MatchOver(side)
//! RoundOver --next_round-------------> Aiming (new screen)
//! ```
//!
//! # Why the guns roll, and why both sides get as many shots
//!
//! The first rules let a player who had found the range fire the same shot
//! again and again: duel winds are small and the hit window (the gun's box
//! plus the blast's reach) is about 44 units wide, so a repeated hit landed
//! about 96% of the time, and whoever opened a round won it about two times
//! in three. Two levers answer that, measured with the duel harness
//! (`examples/duel.rs`, `just duel`):
//!
//! - **The guns roll after a hit** ([`GUN_SHIFT`]). Both guns move the same
//!   way at once, so the duel stays mirrored and fair, but the distance
//!   between them changes by twice the shift and the shot that just worked
//!   no longer does. They roll at the end of the pair of shots the hit was
//!   in, never between its two shots, so the side answering a hit aims at
//!   the guns the hit was scored on.
//! - **Equal shots** ([`EQUAL_SHOTS`]). The opener's winning hit is answered
//!   whenever the answer could tie it (one ahead), and a tie past the mark
//!   plays on; two or more ahead, the round ends at once rather than ask for
//!   a shot that cannot matter.
//!
//! Stronger wind and a shorter hit reach were measured too, and kept as
//! levers ([`WIND_MIN`], [`WIND_MAX`], [`HIT_REACH`]) at their old values:
//! wind strong enough to spoil a repeat swamps the shot, and a shorter
//! reach only makes every shot harder. [`Rules`] gathers the levers.
//!
//! Everything is deterministic from the match seed and the shots, like a
//! run, so a match is kept on disk as exactly that and resumed after the
//! app is killed (`match_log`, DESIGN §11.6). It never reaches the board.
//! A change here that changes what a logged match replays to needs
//! `match_log::MATCH_EPOCH` bumped; its golden match test says so.

use salt_line_sim::ballistics::{self, Aabb, End, FireOptions, Shot, Trace, World};
use salt_line_sim::modifiers::Loadout;
use salt_line_sim::rng::{screen_seed, substream, Pcg32};
use salt_line_sim::run::{self, Rung, Screen, CHASSIS_WIDTH};
use salt_line_sim::solver::{self, GATE_CAP};
use salt_line_sim::terrain::{self, Heightfield, Material, ShapeParams, Template};
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

/// The weakest wind a turn can draw, either way. Zero lets a turn come up
/// calm; above zero, every turn has some wind to read.
pub const WIND_MIN: f64 = 0.0;

/// How far outside a gun's box a detonation may land and still hit it, in
/// world units. The base blast radius: any overlap at all, as a blast
/// needs against a structure (DESIGN §2.7). Zero would mean the shell has
/// to land on the box itself.
pub const HIT_REACH: f64 = salt_line_sim::BASE_BLAST_RADIUS;

/// Whether the side that did not open a round gets an answering shot when
/// the opener reaches the mark and a hit would tie: an opener one ahead is
/// answered, an opener two or more ahead wins at once, and a tie plays on
/// in pairs ([`round_winner`]).
pub const EQUAL_SHOTS: bool = true;

/// How far each gun rolls after a hit, in world units, so the range that
/// just worked no longer does and both players have to find it again. Zero
/// leaves the guns where they are.
///
/// The guns move together and mirror each other (one value,
/// [`Screen::muzzle_x`], places both: see [`frame_for`]), so the duel stays
/// as fair as it was; only the distance between them changes, by twice
/// this. Which way they roll is drawn from the match seed
/// ([`Match::roll_guns`]).
///
/// 16 is the longest step that can still be taken once backwards from
/// [`EMPLACEMENT_X`] with the whole chassis on screen ([`GUN_EDGE_MARGIN`]).
/// It changes the range by 32 of the 44-unit hit window, which is enough
/// that firing the last hit again is no better than aiming afresh (NOTES
/// §7 has the harness numbers).
pub const GUN_SHIFT: f64 = 16.0;

/// How many [`GUN_SHIFT`] steps a gun may roll back from [`EMPLACEMENT_X`],
/// toward its edge of the screen. The guns stand on a ladder of spots a step
/// apart, from this many back to [`GUN_ROLL_FORWARD`] forward, so every roll
/// lands on a spot and both ways are always reachable from home.
pub const GUN_ROLL_BACK: u32 = 1;

/// How many [`GUN_SHIFT`] steps a gun may roll forward from
/// [`EMPLACEMENT_X`], toward the other gun.
pub const GUN_ROLL_FORWARD: u32 = 2;

/// The furthest back a gun's pivot may stand, from the screen edge: half the
/// 26-pixel chassis plus a pixel, so the whole gun stays on screen (the limit
/// Enfilade's emplacement offset keeps in a run). A test holds the default
/// ladder to it.
pub const GUN_EDGE_MARGIN: f64 = 14.0;

/// The tunable rules of a duel, one field per lever. [`Rules::DEFAULT`] is
/// what the game plays, built from the constants above; the duel harness
/// (`examples/duel.rs`) builds others to compare them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rules {
    /// The strongest wind a turn can draw, either way ([`WIND_MAX`]).
    pub wind_max: f64,
    /// The weakest wind a turn can draw, either way ([`WIND_MIN`]).
    pub wind_min: f64,
    /// How far outside the box a detonation still hits ([`HIT_REACH`]).
    pub hit_reach: f64,
    /// The side that did not open answers when a hit would tie
    /// ([`EQUAL_SHOTS`]).
    pub equal_shots: bool,
    /// How far both guns roll after a hit ([`GUN_SHIFT`]).
    pub gun_shift: f64,
    /// Steps a gun may roll back from home ([`GUN_ROLL_BACK`]).
    pub roll_back: u32,
    /// Steps a gun may roll forward from home ([`GUN_ROLL_FORWARD`]).
    pub roll_forward: u32,
}

impl Rules {
    /// The rules the game plays.
    pub const DEFAULT: Rules = Rules {
        wind_max: WIND_MAX,
        wind_min: WIND_MIN,
        hit_reach: HIT_REACH,
        equal_shots: EQUAL_SHOTS,
        gun_shift: GUN_SHIFT,
        roll_back: GUN_ROLL_BACK,
        roll_forward: GUN_ROLL_FORWARD,
    };
}

impl Default for Rules {
    fn default() -> Self {
        Rules::DEFAULT
    }
}

// Substream tags, ASCII: "DUEL", "VTPL", "VSHP", "VWND", "VROL".
const TAG_DUEL: u64 = 0x4455_454C;
const TAG_TEMPLATE: u64 = 0x5654_504C;
const TAG_SHAPE: u64 = 0x5653_4850;
const TAG_WIND: u64 = 0x5657_4E44;
const TAG_ROLL: u64 = 0x5652_4F4C; // "VROL"

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
/// outright; any other detonation hits if it landed in the box or less than
/// `reach` outside it ([`Rules::hit_reach`]). At the default reach,
/// [`HIT_REACH`], that is the overlap a blast needs to damage a structure
/// (DESIGN §2.7: `overlap = 1 - distance / blast`, so any overlap at all is
/// a distance short of the blast radius). Reach scales with the loadout's
/// blast and the shell's damage, as a blast does. A shell that left the
/// screen hit nothing.
pub fn hits(trace: &Trace, target: Aabb, lo: &Loadout, reach: f64) -> bool {
    match trace.end {
        End::Structure { .. } => true,
        end if end.detonates() => {
            let d = target.distance_to(end.pos());
            d == 0.0 || d < reach * lo.blast_scale * trace.damage_scale
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
    /// The roll that followed this shot, if the guns rolled
    /// ([`Match::roll_guns`]).
    pub rolled: Option<Roll>,
}

/// A roll of the guns, as [`Match::fire`] reports it.
#[derive(Clone, Debug)]
pub struct Roll {
    /// How far each gun's pivot moved, by [`Side::index`], in the shared
    /// frame.
    pub moved: [Vec2; 2],
    /// The shared screen as the shot left it, before the roll: craters dug,
    /// guns where they stood. What to show while the shot's result is held.
    pub landed: Screen,
}

/// Which way a roll took the guns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RollWay {
    /// Toward each other: a shorter range.
    Closer,
    /// Back toward the edges: a longer range.
    Apart,
}

impl Roll {
    /// Which way the guns went: the left gun moving right is closer.
    pub fn way(&self) -> RollWay {
        if self.moved[0].x > 0.0 {
            RollWay::Closer
        } else {
            RollWay::Apart
        }
    }
}

/// What rides on the coming shot, when it is more than an ordinary turn
/// ([`Match::stakes`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stakes {
    /// The opener has the mark and is one hit ahead: the side to move has
    /// one shot, and a hit ties it.
    LastShot,
    /// A pair has ended level past the mark: play goes on in pairs.
    Tied,
    /// A hit by the side to move wins the round outright.
    HitToWin,
}

/// Every distinct shot `frame`'s gun can dial in that hits `target`, up to
/// `cap`. Candidates come from the solver's closed form (DESIGN §2.4), aimed
/// at the box centre and just under its top; each is then flown by the
/// integrator and kept only if [`hits`] says so, so a candidate is exactly a
/// shot the player could fire. `reach` is the hit rule's
/// ([`Rules::hit_reach`]).
pub fn shots_at(frame: &Screen, target: Aabb, lo: &Loadout, reach: f64, cap: usize) -> Vec<Shot> {
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
                .any(|t| hits(t, target, lo, reach))
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

/// The wind for one turn of one round, in the shared frame: either way with
/// equal odds, its strength uniform from `rules.wind_min` up to (not
/// including) `rules.wind_max`. A pure function of the match seed, the round
/// and the turn, so the same match plays the same winds whatever happened
/// before.
///
/// One draw makes both the direction and the strength: `u` is uniform in
/// `[-1, 1)`, its sign is the direction and its size is spread over the
/// strength range. With `wind_min` at zero that is exactly
/// `signed(wind_max)`.
pub fn turn_wind(match_seed: u64, round: u32, turn: u32, rules: &Rules) -> f64 {
    let stream = substream(
        substream(round_seed(match_seed, round), TAG_WIND),
        turn as u64,
    );
    let u = Pcg32::new(stream).signed(1.0);
    let strength = rules.wind_min + (rules.wind_max - rules.wind_min) * u.abs();
    if u < 0.0 {
        -strength
    } else {
        strength
    }
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
/// and both extremes of `rules.wind_max`, under `rules`' hit reach,
/// counting up to [`GATE_CAP`].
pub fn duel_candidates(shared: &Screen, lo: &Loadout, rules: &Rules) -> usize {
    let mut worst = usize::MAX;
    for wind in [-rules.wind_max, 0.0, rules.wind_max] {
        let mut s = shared.clone();
        s.wind = wind;
        for side in Side::BOTH {
            let frame = frame_for(&s, side);
            let target = target_in_frame(&s, side);
            let n = shots_at(&frame, target, lo, rules.hit_reach, GATE_CAP).len();
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
fn gate(screen: &mut Screen, lo: &Loadout, rules: &Rules) -> bool {
    screen.candidates = duel_candidates(screen, lo, rules);
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
/// The gate checks `rules`' wind range and hit reach.
pub fn generate_duel(
    match_seed: u64,
    round: u32,
    width: usize,
    lo: &Loadout,
    rules: &Rules,
) -> Screen {
    let seed0 = round_seed(match_seed, round);
    let mut r_template = Pcg32::new(substream(seed0, TAG_TEMPLATE));
    let weights: Vec<u32> = TEMPLATES.iter().map(|(_, w)| *w).collect();
    for attempt in 0..MAX_GEN_ATTEMPTS {
        let template = TEMPLATES[r_template.weighted(&weights)].0;
        let seed = seed0.wrapping_add(attempt as u64);
        let mut screen = assemble(round, seed, width, template, false);
        screen.attempts = attempt + 1;
        if gate(&mut screen, lo, rules) {
            return screen;
        }
    }
    for attempt in 0..MAX_GEN_ATTEMPTS {
        let seed = seed0.wrapping_add(0x1000 + attempt as u64);
        let mut screen = assemble(round, seed, width, Template::Flat, false);
        screen.attempts = MAX_GEN_ATTEMPTS + attempt + 1;
        screen.fell_back = true;
        screen.rung = Some(Rung::Flat);
        if gate(&mut screen, lo, rules) {
            return screen;
        }
    }
    let mut screen = assemble(round, seed0, width, Template::Flat, true);
    screen.attempts = MAX_GEN_ATTEMPTS * 2 + 1;
    screen.fell_back = true;
    screen.rung = Some(Rung::Level);
    if !gate(&mut screen, lo, rules) && cfg!(debug_assertions) {
        panic!(
            "unsolvable duel: match seed {match_seed:#x}, round {round}, width {width}: \
             level ground has only {} shots",
            screen.candidates
        );
    }
    screen
}

// ------------------------------------------------------------- the match

/// Ground a match keeps for every round, in place of a generated duel
/// screen per round: a custom duel on a layout (DESIGN §13.9,
/// `custom::layout_match`).
#[derive(Clone, Debug)]
pub struct Ground {
    /// The screen each round starts on, in the shared frame, with no
    /// craters. Its `wind` is ignored; the match sets each turn's.
    pub screen: Screen,
    /// A wind for every turn, in the shared frame, in place of
    /// [`turn_wind`]'s draw; `None` draws as usual, by the rules' range.
    pub wind: Option<f64>,
}

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
    /// The levers this match plays by ([`Rules::DEFAULT`] in the game).
    pub rules: Rules,
    /// Someone has hit since the guns last had a chance to roll: they roll
    /// once the side that did not open has shot ([`Match::roll_guns`]).
    pub roll_due: bool,
    /// The ground every round is played on, for a match on a layout;
    /// `None` deals a generated screen per round ([`generate_duel`]).
    pub ground: Option<Ground>,
}

impl Match {
    /// A fresh match on `seed` at buffer width `width`: round 1, left to
    /// fire first, under the game's rules.
    pub fn new(seed: u64, width: usize) -> Match {
        Match::with_rules(seed, width, Rules::DEFAULT)
    }

    /// A fresh match under other `rules`, for the duel harness and tests.
    pub fn with_rules(seed: u64, width: usize, rules: Rules) -> Match {
        let loadout = Loadout::new();
        let mut screen = generate_duel(seed, 0, width, &loadout, &rules);
        screen.wind = turn_wind(seed, 0, 0, &rules);
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
            rules,
            roll_due: false,
            ground: None,
        }
    }

    /// A fresh match on `ground` rather than on generated screens: every
    /// round starts on `ground.screen` as it is, under `rules`. `seed` still
    /// draws the winds (unless the ground fixes one) and the rolls.
    pub fn on_ground(seed: u64, ground: Ground, rules: Rules) -> Match {
        let mut m = Match {
            seed,
            loadout: Loadout::new(),
            round: 0,
            turn: 0,
            to_move: Match::first_to_move(0),
            hits: [0, 0],
            rounds: [0, 0],
            screen: ground.screen.clone(),
            landing: [None, None],
            phase: Phase::Aiming,
            rules,
            roll_due: false,
            ground: Some(ground),
        };
        m.screen.index = 0;
        m.screen.wind = m.wind_at(0);
        m
    }

    /// The wind for turn `turn` of this round, in the shared frame: the
    /// ground's fixed wind if it has one, else [`turn_wind`]'s draw.
    pub fn wind_at(&self, turn: u32) -> f64 {
        match &self.ground {
            Some(Ground { wind: Some(w), .. }) => *w,
            _ => turn_wind(self.seed, self.round, turn, &self.rules),
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
        let hit = traces
            .iter()
            .any(|t| hits(t, target, &lo, self.rules.hit_reach));
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
        let self_hit = !hit
            && traces
                .iter()
                .any(|t| hits(t, own_box, &lo, self.rules.hit_reach));

        if hit {
            self.hits[shooter.index()] += 1;
            self.roll_due = true;
        }
        let mut rolled = None;
        let opener = Match::first_to_move(self.round);
        if let Some(winner) = round_winner(self.hits, shooter, opener, &self.rules) {
            self.rounds[winner.index()] += 1;
            self.phase = if self.rounds[winner.index()] >= ROUNDS_TO_WIN_MATCH {
                Phase::MatchOver(winner)
            } else {
                Phase::RoundOver(winner)
            };
        } else {
            // Roll at the end of each pair of shots, never between the two
            // shots of a pair, so the side answering a hit fires at the same
            // guns the hit was scored on, as the opener did.
            if self.roll_due && shooter != opener {
                self.roll_due = false;
                let landed = self.screen.clone();
                rolled = self.roll_guns().map(|moved| Roll { moved, landed });
            }
            self.turn += 1;
            self.to_move = shooter.other();
            self.screen.wind = self.wind_at(self.turn);
        }
        Some(TurnReport {
            shooter,
            traces,
            hit,
            self_hit,
            landing,
            rolled,
        })
    }

    /// Roll both guns one `rules.gun_shift` step, back toward the edges or
    /// forward toward each other, and say how far each pivot moved.
    /// [`Match::fire`] calls it after the second shot of any pair in which
    /// someone hit, unless the round is over.
    ///
    /// The guns stand on a ladder of spots a step apart, from
    /// `rules.roll_back` steps behind [`EMPLACEMENT_X`] to
    /// `rules.roll_forward` in front. The way is drawn from the match seed,
    /// the round and the turn, even odds; a gun at the end of the ladder
    /// rolls the other way, and a spot is only taken if the screen still
    /// passes the duel gate from there ([`duel_candidates`]). If neither way
    /// does, the guns stay put and this returns `None`.
    ///
    /// Each gun rolls onto fresh ground: the craters under its chassis at
    /// the new spot are filled ([`fill_under_guns`]), so both guns are seated
    /// on the ground as it was generated, by the same rule mirrored
    /// ([`muzzle_of`]), and neither starts its next shot from the bottom of
    /// a hole the other dug. Each side's landing mark moves with its gun, so
    /// aim help still shows where the last shell fell relative to the gun
    /// that fired it.
    pub fn roll_guns(&mut self) -> Option<[Vec2; 2]> {
        let step = self.rules.gun_shift;
        if step <= 0.0 {
            return None;
        }
        let stream = substream(
            substream(round_seed(self.seed, self.round), TAG_ROLL),
            self.turn as u64,
        );
        let forward_first = Pcg32::new(stream).next_u32() & 1 == 1;
        let ways: [i64; 2] = if forward_first { [1, -1] } else { [-1, 1] };
        let rung = ((self.screen.muzzle_x - EMPLACEMENT_X) / step).round() as i64;
        let ladder = -(self.rules.roll_back as i64)..=self.rules.roll_forward as i64;
        for way in ways {
            let next = rung + way;
            if !ladder.contains(&next) {
                continue;
            }
            let mut moved = self.screen.clone();
            moved.muzzle_x = EMPLACEMENT_X + next as f64 * step;
            fill_under_guns(&mut moved);
            if duel_candidates(&moved, &self.loadout, &self.rules) < MIN_SOLUTIONS {
                continue;
            }
            let delta = Side::BOTH.map(|side| {
                let (a, b) = (muzzle_of(&self.screen, side), muzzle_of(&moved, side));
                Vec2::new(b.x - a.x, b.y - a.y)
            });
            self.screen = moved;
            for (mark, d) in self.landing.iter_mut().zip(delta) {
                if let Some(m) = mark {
                    *m = Vec2::new(m.x + d.x, m.y + d.y);
                }
            }
            return Some(delta);
        }
        None
    }

    /// What rides on the coming shot, as the side to move sees it: a last
    /// shot to tie ([`Stakes::LastShot`]), a tie that plays on
    /// ([`Stakes::Tied`]), or a hit that wins the round
    /// ([`Stakes::HitToWin`]). `None` for an ordinary turn and outside
    /// `Phase::Aiming`. The first two only arise with equal shots.
    pub fn stakes(&self) -> Option<Stakes> {
        if self.phase != Phase::Aiming {
            return None;
        }
        let opener = Match::first_to_move(self.round);
        let (theirs, mine) = (self.hits[opener.index()], self.hits[opener.other().index()]);
        let mover = self.to_move;
        let equal = self.rules.equal_shots;
        let mut after_hit = self.hits;
        after_hit[mover.index()] += 1;
        if equal && mover != opener && theirs >= HITS_TO_WIN_ROUND && theirs == mine + 1 {
            Some(Stakes::LastShot)
        } else if equal && mover == opener && theirs >= HITS_TO_WIN_ROUND && theirs == mine {
            Some(Stakes::Tied)
        } else if round_winner(after_hit, mover, opener, &self.rules) == Some(mover) {
            Some(Stakes::HitToWin)
        } else {
            None
        }
    }

    /// Deal the next round: a new screen (or the ground again, fresh), hits
    /// back to zero, the other side opening. Returns false unless a round
    /// has just ended.
    pub fn next_round(&mut self) -> bool {
        if !matches!(self.phase, Phase::RoundOver(_)) {
            return false;
        }
        self.round += 1;
        self.turn = 0;
        self.hits = [0, 0];
        self.landing = [None, None];
        self.roll_due = false;
        self.to_move = Match::first_to_move(self.round);
        self.screen = match &self.ground {
            Some(ground) => Screen {
                index: self.round,
                ..ground.screen.clone()
            },
            None => generate_duel(
                self.seed,
                self.round,
                self.width(),
                &self.loadout,
                &self.rules,
            ),
        };
        self.screen.wind = self.wind_at(0);
        self.phase = Phase::Aiming;
        true
    }
}

/// Fill the craters under both guns' chassis, wherever `shared.muzzle_x`
/// stands them: the columns each gun is seated on ([`Screen::muzzle`] reads
/// the highest of them) go back to the ground as generated. The right gun's
/// columns are the left's mirrored, so the two are filled alike.
///
/// Without this a gun could roll into a stretch the other side had been
/// cratering and sit well below it: across harness matches the two pivots
/// were up to 16 units apart after a roll, against under 5 on the ground as
/// generated (what is left of its asymmetry, [`NEAR_SYMMETRY`]).
pub fn fill_under_guns(shared: &mut Screen) {
    let w = shared.width;
    let x0 = (shared.muzzle_x - CHASSIS_WIDTH as f64 * 0.5).clamp(0.0, w as f64) as usize;
    for i in x0..(x0 + CHASSIS_WIDTH).min(w) {
        for c in [i, w - 1 - i] {
            // Unexcavated ground is Sand: the terrain module's own rule for
            // a scar of zero, so later craters there are sized as on any
            // fresh ground, and the two sides stay mirror images.
            shared.field.scar[c] = 0.0;
            shared.field.material[c] = Material::Sand;
        }
    }
}

/// Who has won the round, if anyone, now that `shooter` has fired and
/// `hits` counts that shot.
///
/// Normally the first to [`HITS_TO_WIN_ROUND`] wins at once. With
/// [`Rules::equal_shots`], the side that did not open always gets its shot
/// when that shot could still tie: a round ends after the second shot of a
/// pair with whoever leads at or past the mark, or straight after the
/// opener's shot if the opener has the mark and leads by two or more, when
/// no answer could catch up. An opener who reaches the mark one hit ahead
/// is answered by one more shot; if that ties it, play goes on in pairs
/// until a pair ends with one side ahead.
pub fn round_winner(hits: [u32; 2], shooter: Side, opener: Side, rules: &Rules) -> Option<Side> {
    if !rules.equal_shots {
        return (hits[shooter.index()] >= HITS_TO_WIN_ROUND).then_some(shooter);
    }
    if shooter == opener {
        let (theirs, mine) = (hits[opener.index()], hits[opener.other().index()]);
        return (theirs >= HITS_TO_WIN_ROUND && theirs >= mine + 2).then_some(opener);
    }
    let (mine, theirs) = (hits[shooter.index()], hits[opener.index()]);
    if mine.max(theirs) < HITS_TO_WIN_ROUND || mine == theirs {
        None
    } else if mine > theirs {
        Some(shooter)
    } else {
        Some(opener)
    }
}

#[cfg(test)]
mod tests;

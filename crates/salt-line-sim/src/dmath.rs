//! Platform-stable float math.
//!
//! DESIGN §3 requires that `sin`/`cos` never be called on unquantized input in
//! a state-affecting path, and the brief forbids any determinism leak across
//! Linux / macOS / Windows. `f64::sin` is a libm call and libm is *not*
//! required to be bit-identical between platforms, so the sim does not use it.
//!
//! Everything here is built from the IEEE-754 operations that *are* exactly
//! specified — add, subtract, multiply, divide, `sqrt`, `floor` — so results
//! are byte-identical everywhere. `f64::floor` and `f64::sqrt` are the only
//! std float math functions the sim crate calls (comparisons such as
//! `f64::min` and `f64::max` are exact too).
//!
//! The techniques are standard and worth knowing: `sin` is a Taylor series
//! evaluated by Horner's method after argument reduction into
//! `[-pi/2, pi/2]`; `cos` is `sin` shifted by a quarter turn; and `atan2` is
//! replaced outright by a binary search over the integer angle grid
//! ([`angle_ticks_from_dir`]), which needs only the trig above.
//!
//! Precision is ~1e-16 relative on the reduced interval, which is far tighter
//! than anything the sim can observe: input angles are quantized to 0.05° ticks
//! before any trig happens, and the authority on whether a shot connects is
//! always the fixed-step integration, never the closed-form solve.

/// π, rounded to the nearest `f64`. Constants are literals, so they are the
/// same bits everywhere.
pub const PI: f64 = core::f64::consts::PI;
/// 2π (τ), one full turn in radians.
pub const TWO_PI: f64 = core::f64::consts::TAU;
/// π/2, a quarter turn in radians.
pub const HALF_PI: f64 = core::f64::consts::FRAC_PI_2;

/// Degrees per angle tick. `Vernier` changes the *input* quantization
/// (DESIGN §4); the integer tick grid underneath is always 0.05°.
pub const TICK_DEG: f64 = 0.05;
/// Radians per angle tick.
pub const TICK_RAD: f64 = TICK_DEG * PI / 180.0;
/// Ticks spanning 0°..=90°.
pub const MAX_ANGLE_TICKS: i32 = 1800;

/// Reciprocal factorials, 3!..19!, odd terms only.
const R3: f64 = 1.0 / 6.0;
const R5: f64 = 1.0 / 120.0;
const R7: f64 = 1.0 / 5040.0;
const R9: f64 = 1.0 / 362880.0;
const R11: f64 = 1.0 / 39916800.0;
const R13: f64 = 1.0 / 6227020800.0;
const R15: f64 = 1.0 / 1307674368000.0;
const R17: f64 = 1.0 / 355687428096000.0;
const R19: f64 = 1.0 / 121645100408832000.0;

/// Taylor series for sin on |x| <= pi/2, through the x^19 term.
///
/// The series alternates, so the truncation error is bounded by the first
/// omitted term, x^21/21!: about 2.6e-16 at the ends of the interval, roughly
/// one ulp of 1.0, and far smaller nearer zero.
#[inline]
fn sin_poly(x: f64) -> f64 {
    let x2 = x * x;
    // Horner from the tail inward: nested multiply-adds in powers of x²,
    // fewer operations and less rounding than summing each power separately.
    let mut s = R19;
    s = R17 - x2 * s;
    s = R15 - x2 * s;
    s = R13 - x2 * s;
    s = R11 - x2 * s;
    s = R9 - x2 * s;
    s = R7 - x2 * s;
    s = R5 - x2 * s;
    s = R3 - x2 * s;
    x * (1.0 - x2 * s)
}

/// Deterministic sine. Argument reduction uses only floor / mul / sub.
///
/// Accurate for the modest arguments the sim passes (a few turns at most).
/// The reduction is a plain `x - k*2pi`, not a Cody–Waite or Payne–Hanek
/// scheme, so the error grows with `|x|` for large inputs.
pub fn sin(x: f64) -> f64 {
    // Reduce to [-pi, pi]: `k` is the nearest whole number of turns.
    let k = (x * (1.0 / TWO_PI) + 0.5).floor();
    let mut r = x - k * TWO_PI;
    // Fold to [-pi/2, pi/2] using sin(pi - r) == sin(r).
    if r > HALF_PI {
        r = PI - r;
    } else if r < -HALF_PI {
        r = -PI - r;
    }
    sin_poly(r)
}

/// Deterministic cosine, through the co-function identity
/// `cos(x) = sin(pi/2 - x)`.
#[inline]
pub fn cos(x: f64) -> f64 {
    sin(HALF_PI - x)
}

/// `(cos, sin)` of a quantized angle. This is the only entry point the
/// ballistics core uses for player input; its other trig calls (the ricochet
/// limit, the cluster fan) take fixed constants, never a raw float from play.
///
/// `ticks` is in 0.05° steps ([`TICK_DEG`]); any `i32` works, though the sim
/// only passes `0..=`[`MAX_ANGLE_TICKS`].
#[inline]
pub fn dir_from_ticks(ticks: i32) -> (f64, f64) {
    let a = ticks as f64 * TICK_RAD;
    (cos(a), sin(a))
}

/// Exactly-rounded square root — IEEE-754 specifies this one.
#[inline]
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// `hypot` built from specified operations. Not `f64::hypot`, which is libm.
#[inline]
pub fn hypot(a: f64, b: f64) -> f64 {
    sqrt(a * a + b * b)
}

/// Largest integer not greater than `x`. Exact in IEEE-754.
#[inline]
pub fn floor(x: f64) -> f64 {
    x.floor()
}

/// Absolute value, written as a comparison so no std call is involved.
/// Returns `-0.0` for `-0.0` and passes NaN through unchanged.
#[inline]
pub fn abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Round-half-up, expressed through `floor`.
///
/// Halves go toward +infinity (`-0.5` rounds to `0.0`), unlike
/// `f64::round`, which rounds halves away from zero.
#[inline]
pub fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `x` limited to `[lo, hi]`. Unlike `f64::clamp` it does not panic when
/// `lo > hi`, though the result is then not meaningful.
#[inline]
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Hermite smoothstep. Used by the value noise (DESIGN §2.3) — matches
/// `tools/gen_sprites.py::vnoise`.
///
/// `3f² - 2f³` on `f` in `[0, 1]`: 0 to 1 with zero slope at both ends, so
/// noise interpolated with it has no creases at the lattice points.
#[inline]
pub fn smoothstep(f: f64) -> f64 {
    f * f * (3.0 - 2.0 * f)
}

/// Compact-support bump on `[-1, 1]`, peak 1 at 0, C1 at the edges.
/// Stands in for a Gaussian everywhere in terrain shaping so that no `exp`
/// call ever enters a state-affecting path.
///
/// `(1 - (d/radius)²)²` inside the radius, 0 outside and for `radius <= 0`.
/// The same squared-falloff kernel shapes craters (DESIGN §2.6).
#[inline]
pub fn bump(d: f64, radius: f64) -> f64 {
    if radius <= 0.0 {
        return 0.0;
    }
    let u = abs(d) / radius;
    if u >= 1.0 {
        0.0
    } else {
        let t = 1.0 - u * u;
        t * t
    }
}

/// Nearest angle tick to the direction `(vx, vy)`, clamped to `0..=1800`.
///
/// A binary search over the tick grid using only `sin`/`cos` of quantized
/// angles, in place of `atan2`. The cross product
/// `vy*cos(a) - vx*sin(a) = |v|*sin(theta - a)` is monotonically decreasing in
/// `a` across the quarter turn, so the search is exact.
///
/// Directions outside the quarter turn clamp: any direction with `vy <= 0`
/// gives 0, and one with `vy > 0` and `vx <= 0` gives [`MAX_ANGLE_TICKS`].
/// The search takes eleven halvings of the 1800-tick range. The solver uses
/// this to turn a closed-form velocity into a dialable angle, and the Godot
/// layer uses it for pull-to-aim.
pub fn angle_ticks_from_dir(vx: f64, vy: f64) -> i32 {
    if vx <= 0.0 {
        return if vy > 0.0 { MAX_ANGLE_TICKS } else { 0 };
    }
    let cross = |t: i32| -> f64 {
        let (c, s) = dir_from_ticks(t);
        vy * c - vx * s
    };
    if cross(0) <= 0.0 {
        return 0;
    }
    if cross(MAX_ANGLE_TICKS) >= 0.0 {
        return MAX_ANGLE_TICKS;
    }
    // Invariant: cross(lo) > 0 >= cross(hi).
    let (mut lo, mut hi) = (0i32, MAX_ANGLE_TICKS);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if cross(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // Pick whichever of the bracketing ticks is closer in direction.
    if abs(cross(lo)) <= abs(cross(hi)) {
        lo
    } else {
        hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_cos_track_the_std_library() {
        // std is the accuracy oracle here; the sim uses ours for determinism.
        let mut x = -20.0f64;
        while x < 20.0 {
            assert!(
                abs(sin(x) - x.sin()) < 1e-12,
                "sin({x}) = {} vs {}",
                sin(x),
                x.sin()
            );
            assert!(abs(cos(x) - x.cos()) < 1e-12, "cos({x})");
            x += 0.013;
        }
    }

    #[test]
    fn known_values_are_exact_enough() {
        assert!(abs(sin(0.0)) < 1e-18);
        assert!(abs(sin(HALF_PI) - 1.0) < 1e-15);
        assert!(abs(cos(0.0) - 1.0) < 1e-15);
        assert!(abs(cos(HALF_PI)) < 1e-15);
        assert!(abs(sin(PI)) < 1e-15);
        let (c, s) = dir_from_ticks(900); // 45 degrees
        assert!(abs(c - s) < 1e-15);
        assert!(abs(hypot(c, s) - 1.0) < 1e-15);
    }

    #[test]
    fn unit_circle_holds_on_every_tick() {
        for t in 0..=MAX_ANGLE_TICKS {
            let (c, s) = dir_from_ticks(t);
            assert!(abs(c * c + s * s - 1.0) < 1e-14, "tick {t}");
        }
    }

    #[rstest::rstest]
    #[case::flat(1.0, 0.0, 0)]
    #[case::straight_up(0.0, 1.0, MAX_ANGLE_TICKS)]
    #[case::diagonal(1.0, 1.0, 900)]
    #[case::behind_and_up(-1.0, 5.0, MAX_ANGLE_TICKS)]
    #[case::behind_and_down(-1.0, -5.0, 0)]
    #[case::forward_and_down(3.0, -1.0, 0)]
    #[case::zero(0.0, 0.0, 0)]
    fn angle_ticks_at_the_edges(#[case] vx: f64, #[case] vy: f64, #[case] expected: i32) {
        assert_eq!(angle_ticks_from_dir(vx, vy), expected);
    }

    #[test]
    fn angle_ticks_round_trip() {
        for t in 0..=MAX_ANGLE_TICKS {
            let (c, s) = dir_from_ticks(t);
            let v = 137.5;
            let got = angle_ticks_from_dir(c * v, s * v);
            assert_eq!(got, t, "tick {t} round-tripped to {got}");
        }
    }

    #[test]
    fn bump_has_compact_support() {
        assert_eq!(bump(0.0, 4.0), 1.0);
        assert_eq!(bump(4.0, 4.0), 0.0);
        assert_eq!(bump(9.0, 4.0), 0.0);
        assert!(bump(2.0, 4.0) > 0.0 && bump(2.0, 4.0) < 1.0);
    }
}

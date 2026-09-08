# Implementation brief — Salt Line

Paste this whole file as the opening prompt to a local coding agent (Claude Code
or equivalent) running at the repository root. It assumes `design/DESIGN.md`,
`art/`, and `tools/` are present.

---

## Your task

Build **Salt Line**, a single-player artillery roguelike. A deterministic Rust
simulation crate with a Godot 4 presentation layer bound via `gdext`.

`design/DESIGN.md` is the specification. Read it in full before writing code. It
is authoritative on mechanics, numbers, terrain generation, the modifier pool,
and art constraints. Where this brief and the spec disagree, the spec wins —
except on build order and acceptance criteria, which are defined here.

## Non-negotiable constraints

These are architectural, not stylistic. Violating any of them invalidates the
design and will require rework.

1. **`salt-line-sim` has zero Godot dependency.** It builds and its tests pass
   with `cargo test -p salt-line-sim` and no engine present. If you find
   yourself wanting a Godot type in the sim crate, the boundary is wrong.
2. **The simulation is fully deterministic.** Same seed plus same input sequence
   produces a byte-identical final state on every platform.
   - Fixed timestep, `dt = 1/120`, semi-implicit Euler. No variable timestep in
     the sim, ever.
   - `f64` throughout the sim.
   - Explicit PRNG (SplitMix64 / PCG32). Never `rand::thread_rng`, never
     `HashMap` iteration order, never system time, never floating-point
     `sin`/`cos` on unquantized input in a code path that affects state.
3. **Wind is an acceleration, not a velocity offset.** Drift grows with the
   square of flight time. Do not "simplify" this.
4. **Every screen is validated solvable before display.** See DESIGN §2.4. Three
   or more surviving trajectory candidates required. No screen is ever shown to
   the player without passing this check.
5. **Native resolution is 180px tall.** No sprite scaling, no runtime rotation,
   no anti-aliasing. Camera position quantized to whole pixels every frame.
6. **No asset, code, or naming lineage from any existing artillery game.** See
   DESIGN §7. Do not consult, port, or reference `GORILLA.BAS` or any
   reimplementation of it, at any point, for any reason.

## Build order

Work through these in sequence. Do not start a milestone before the previous
one's acceptance criteria pass. Commit at each milestone boundary.

### M1 — Deterministic core

`crates/salt-line-sim/src/{rng,terrain,ballistics}.rs`

- SplitMix64 and PCG32. Value noise with smoothstep interpolation, fBm at 4
  octaves, domain warp per DESIGN §2.3.
- Heightfield: `base`, `scar`, `material` as parallel buffers. One sample per
  pixel column, `TERRAIN_STEP = 1.0`.
- Crater subtraction with squared falloff, material-dependent radius, clamped at
  bedrock (DESIGN §2.6).
- Projectile integration with gravity and wind acceleration; heightfield
  collision.

**Acceptance:** `cargo test -p salt-line-sim` passes with no engine installed.
A test generates screen 400 directly from a run seed without generating screens
1–399, and asserts it matches the same screen generated in sequence.

`tools/gen_sprites.py` contains a working Python implementation of the noise,
fBm, domain warp, and crater math. Port from it — the constants there produce
the terrain in `art/scene_native_320x180.png`.

### M2 — Solver and screen assembly

`crates/salt-line-sim/src/solver.rs`

- Analytic trajectory solve: for a fixed flight time the equations are linear in
  the velocity components, so solve directly for `(vx, vy)`, then derive
  `(θ, v)` and range-check. Sweep 40 flight-time samples.
- Raycast each candidate against the heightfield at sim timestep.
- Screen assembly: template selection by band, material layering, target
  placement with 40-unit minimum separation, reject-and-regenerate on failure
  (cap 8 attempts, then fall back to `Flat`).

**Acceptance:** a batch harness generates 5,000 screens across 50 seeds and
asserts every one is solvable. It emits a histogram of surviving-candidate
counts bucketed by band. Ship this as a `--bin` or a `#[test]` with output; you
will use it to tune DESIGN §2.5 without playing the game.

### M3 — Run state and structures

`crates/salt-line-sim/src/{run,modifiers}.rs`

- Shell economy and the refund table (DESIGN §1). Draft every 4 screens, 3
  offers drawn by screen seed, excluding anything already at max stacks.
- Structure integrity and support tracking, the three-state threshold table, and
  lean-direction computation (DESIGN §2.7).
- Implement the full modifier pool from DESIGN §4. Modifiers are data, not
  branches — a table of effect deltas applied to a `Loadout` struct that the
  ballistics and solver read from.

**Acceptance:** undermining alone can collapse a structure with no direct hit —
write a test that craters only the footprint and asserts `Collapsed`. Template
gating works: `Overhang` is never offered unless the loadout holds ricochet,
airburst, or tunneling.

### M4 — Replay

`crates/salt-line-sim/src/replay.rs`

Shot log is `(screen_index, angle_ticks, power_ticks)`. Seed plus log
reconstructs a run exactly; nothing else is persisted.

**Acceptance:** 200 recorded runs replay to matching final-state hashes. Wire
this into CI across Linux, macOS, and Windows. If hashes diverge across
platforms, stop and fix the determinism leak before continuing — do not paper
over it with tolerances.

### M5 — Godot layer

`crates/salt-line-godot`, plus a Godot 4 project.

- Terrain renders as vertical spans of solid color, one pixel column each,
  directly into an image buffer (DESIGN §5.5). No tilemap, no mesh, no sprites.
- Stretch mode `viewport`, aspect `expand`, default texture filter `nearest` set
  project-wide in Project Settings.
- Fix internal height at 180px; let width float 320–400 by device aspect and
  generate terrain to fill the reported width.
- Camera x quantized to whole pixels every frame.
- Barrel uses the 19-frame strip in `art/sprites/barrel_strip_5deg.png`, sliced
  at 28px. The barrel displays angle quantized to 5°; the numeric field shows
  the true value. This gap is intentional — do not "fix" it.
- Bitmap font at native size for all UI. Never scale a TTF into the buffer.

**Acceptance:** the running game at screen 1 is visually consistent with
`art/scene_native_320x180.png`. Pan the camera and confirm no terrain shimmer.

### M6 — Shell

Daily seeded run, local leaderboard on screens cleared, seed entry and sharing.
Free to play through screen 15, then a single one-time unlock IAP.

## Art assets

`art/sprites/` holds native-resolution PNGs — tank variants, the 19-frame barrel
strip, five structure types, and the water tower's three damage states.
`art/palette_16.png` is the fixed 16-color palette; sample it programmatically
rather than transcribing hex values by hand.

These are programmer art produced by `tools/gen_sprites.py`, good enough to
build and tune against. Regenerate with `python3 tools/gen_sprites.py` after
editing the generator. Replace them with hand-drawn work before release, but
keep the dimensions and palette fixed.

The water tower is the first structure to wire up end to end — top-heavy
silhouettes make undermining legible, and that is how the demolition build
teaches itself without a tutorial.

## Working agreement

- Ask before adding any dependency to `salt-line-sim`. It should need almost
  nothing.
- Do not add features not in DESIGN.md. If you think something is missing,
  raise it rather than building it.
- When a spec constant produces bad results in play, report the number and what
  you observed. Do not silently retune.
- Prefer a failing test that encodes the spec over a comment describing it.

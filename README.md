# Salt Line

Single-player artillery roguelike. Endless procedural horizon, persistent
terrain damage, shells as the run resource, modifier drafting every four screens.

Deterministic Rust simulation core, Godot 4 presentation layer via `gdext`.

**Status: 0.1.0 desktop development preview.** Not for sale, nothing to buy,
every screen playable. Binaries are unsigned. `RELEASING.md` has the supported
platforms, the release checklist, and the known-issues list.

Requires Rust `1.98.1` (pinned in `rust-toolchain.toml`) and Godot `4.4.1`.

The `godot` crate is pinned to the matching `api-4-4` feature. If you raise the
engine version, raise that feature and `compatibility_minimum` in
`salt_line.gdextension` with it: bindings built against a newer API than the
running engine do not load, and the game boots to an empty window instead of
failing loudly.

## Contents

| Path | What it is |
|---|---|
| `design/DESIGN.md` | Full specification — mechanics, terrain, modifiers, art |
| `IMPLEMENTATION_PROMPT.md` | The brief this build was written against |
| `crates/salt-line-sim` | The whole game. No Godot dependency, no dependencies at all |
| `crates/salt-line-godot` | Rendering, input, UI. Draws what the sim says is true |
| `art/` | Native-resolution sprites, 16-colour palette, reference scene |
| `tools/gen_sprites.py` | Regenerates all art; working reference for the terrain math |
| `project.godot`, `godot/` | The Godot 4 project. Its root is the repo root, so `res://art/...` is the same path the generator writes |
| `RELEASING.md` | What the preview is, the release checklist, the playtest matrix, known issues |

## Quick start

```sh
cargo test                       # sim + presentation tests, no engine needed
cargo build -p salt-line-godot   # build the GDExtension
godot --path .                   # play
```

`cargo build` before `godot`: the extension is loaded from
`target/{debug,release}/`, so a stale build is what the engine will run.

## Controls

| Key | Action |
|---|---|
| Left / Right | Angle, one quantization step (Shift: ten) |
| Up / Down | Power (Shift: ten steps) |
| Space | Fire |
| `[` `]` | Airburst fuse distance |
| `,` `.` | Reposition the emplacement (needs Enfilade) |
| 1 / 2 / 3 | Take a draft offer |
| Enter | Daily run, confirm, continue |
| S | Seed entry |
| C | Copy the seed (on the run-over screen) |
| R | Replay the same seed |
| F1 | Debug overlay: template, candidate count, camera, buffer size |
| Esc | Back / quit |

The barrel sprite shows the angle rounded to 5°; the numeric field shows the
true value. That gap is intentional — it is why `Vernier` feels like an
instrument rather than a stat.

## Tools

Everything here runs headless. None of it needs a human at the keyboard.

```sh
# Solvability: 5,000 screens across 50 seeds, and the candidate histogram that
# DESIGN §2.5 is tuned against. Non-zero exit if any screen fails the gate.
cargo run --release -p salt-line-sim --bin batch
cargo run --release -p salt-line-sim --bin batch -- --seeds 50 --screens 100 --build demo

# Determinism: 200 runs played, replayed from (seed, shot log), hashed.
# CI compares the digest across Linux, macOS and Windows.
cargo run --release -p salt-line-sim --bin determinism

# Art review: one frame per shape template plus a damaged variant of each.
godot --headless --path . -- --salt-gallery /tmp/gallery

# Look at any screen without playing to it, and screenshot it.
godot --headless --path . -- --salt-seed 20260907 --salt-screen 96 \
    --salt-shot /tmp/screen96.png
```

Dev flags: `--salt-seed <seed>`, `--salt-screen <n>`, `--salt-auto` (aims with
the solver and takes offer 1), `--salt-miss` (throws the first shot of each
screen away, to exercise craters and chalk marks), `--salt-hold-draft`,
`--salt-shot <path>`, `--salt-shot-at <seconds>`, `--salt-gallery <dir>`.

## Art

`art/sprites/` is native-resolution programmer art from `tools/gen_sprites.py`.
Regenerate after editing the generator:

```sh
python3 tools/gen_sprites.py     # needs Pillow; writes into art/
```

The 16-colour palette is sampled out of `art/palette_16.png` at runtime rather
than transcribed, so the image stays the single source of truth.

Regenerating rewrites every PNG, and different Pillow versions encode identical
pixels to different bytes. Compare pixels, not file hashes, before committing
the churn.

All five silhouettes now have all three damage states. The water tower is drawn
per state by hand; the derrick, mast, silo and pylon are sheared about their own
footing and wrecked from the intact drawing, so the states cannot drift away
from the geometry they came from.

Replace all of it with hand-drawn work before any public release, but keep the
dimensions and the palette fixed — they are the contract. Still open: no barrel
strip for the Ballast or Forced March chassis, so those builds read from the
hull alone.

## The three things that matter

1. **`salt-line-sim` never depends on Godot.** It builds headless, and it has
   no dependencies at all — not even `rand`.
2. **Wind is an acceleration, not a velocity offset.** Drift grows with the
   square of flight time. Most of the difficulty curve falls out of this.
3. **Every screen is proven solvable before it is shown** — three or more
   surviving trajectory candidates per target, checked against the player's
   current loadout, and verified by the same damage model the game runs.

## Determinism

Same seed plus same input sequence gives a byte-identical final state on every
platform. What that costs, concretely:

- Fixed `dt = 1/120`, semi-implicit Euler, `f64` throughout. The render
  interpolates; the sim never does.
- Explicit PRNGs (SplitMix64, PCG32) seeded from `run_seed` and the screen
  index alone. No `thread_rng`, no system time, no `HashMap` iteration order in
  any state-affecting path.
- **No libm.** `f64::sin` is not required to be bit-identical between
  platforms, so `sim/dmath.rs` builds `sin`, `cos` and `hypot` out of the IEEE
  operations that *are* exactly specified. `sqrt` and `floor` are the only std
  float methods the sim calls.
- Modifier effects fold in pool order, not draft order, because f64
  multiplication is not associative.
- A run persists as `(run_seed, width, shot log)` and nothing else. Draft picks
  ride in the same triple with `angle_ticks = -1`.

## Monetization

The preview is built **without** the `monetization` feature and sells nothing.
The paywall, the free-screen gate and the storefront seam all exist in
`crates/salt-line-godot/src/store.rs`, behind that flag.

No build grants the unlock locally. Entitlement is asked of the platform at
startup; the `unlocked` line in the save file is a cache of that answer and is
never treated as proof of purchase, because it is a text file the player owns.
With no storefront SDK linked, a monetized build answers `Unavailable` — which
is why it is not the build that ships.

## Independence

No code, assets, or naming lineage from any existing artillery game. See
`design/DESIGN.md` §7 for the pre-release checklist, and `RELEASING.md` for the
signing, store and clearance work that has to happen outside this repository.

## License

MIT. See `LICENSE`.

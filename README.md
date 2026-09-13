# Salt Line

Single-player artillery roguelike. Endless procedural horizon, persistent
terrain damage, shells as the run resource, modifier drafting every four screens.

Deterministic Rust simulation core, Godot 4 presentation layer via `gdext`.

## Contents

| Path | What it is |
|---|---|
| `design/DESIGN.md` | Full specification — mechanics, terrain, modifiers, art |
| `IMPLEMENTATION_PROMPT.md` | The brief this build was written against |
| `crates/salt-line-sim` | The whole game. No Godot dependency, no dependencies at all |
| `crates/salt-line-godot` | Rendering, input, UI. Draws what the sim says is true |
| `art/` | Native-resolution sprites, 16-colour palette, reference scene |
| `tools/gen_sprites.py` | Regenerates all art; working reference for the terrain math |
| `site/` | The alpha marketing page, published to GitHub Pages |
| `tools/build_site.py` | Generates every image on that page out of the art pipeline |
| `project.godot`, `godot/` | The Godot 4 project. Its root is the repo root, so `res://art/...` is the same path the generator writes |

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

Replace these with hand-drawn work before release, but keep the dimensions and
the palette fixed. The gap to close first: only the water tower has all three
damage states drawn. Every other silhouette falls back to its intact sprite
while leaning, and to a procedural rubble mound when it goes down.

## The marketing page

`site/` is a static page — no framework, no build step for the markup, no
third-party requests. Its media is not hand-exported: `tools/build_site.py`
imports `tools/gen_sprites.py` and draws the hero screen, the animated shot and
the social card with the same palette, rasteriser and native grid the game uses.
Change a sprite and the page changes with it.

```sh
python3 tools/build_site.py           # needs Pillow; writes into site/assets/
python3 -m http.server -d site 8000   # preview at http://localhost:8000
```

`.github/workflows/pages.yml` regenerates the media on every push to `main` that
touches `site/`, `art/` or either generator, then publishes `site/` to Pages.
The live URL is whatever Pages reports for this repository — the workflow logs
it and stamps it into the page's canonical and Open Graph tags, so nothing has
to be hardcoded while the repo is internal.

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

## Independence

No code, assets, or naming lineage from any existing artillery game. See
`design/DESIGN.md` §7 for the pre-release checklist.

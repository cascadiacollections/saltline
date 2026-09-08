# Salt Line

Single-player artillery roguelike. Endless procedural horizon, persistent
terrain damage, shells as the run resource, modifier drafting every four screens.

Deterministic Rust simulation core, Godot 4 presentation layer via `gdext`.

## Contents

| Path | What it is |
|---|---|
| `IMPLEMENTATION_PROMPT.md` | Paste into a local coding agent to start the build |
| `design/DESIGN.md` | Full specification — mechanics, terrain, modifiers, art |
| `art/` | Native-resolution sprites, 16-colour palette, reference scene |
| `tools/gen_sprites.py` | Regenerates all art; working reference for the terrain math |
| `crates/` | Workspace skeleton with module stubs |

## Quick start

```sh
python3 tools/gen_sprites.py     # regenerate art (needs Pillow)
cargo test -p salt-line-sim      # sim crate builds and tests with no engine
```

Then hand `IMPLEMENTATION_PROMPT.md` to the agent.

## The three things that matter

1. **`salt-line-sim` never depends on Godot.** It builds headless.
2. **Wind is an acceleration, not a velocity offset.** Drift grows with the
   square of flight time. Most of the difficulty curve falls out of this.
3. **Every screen is proven solvable before it is shown** — three or more
   surviving trajectory candidates, checked against the player's current loadout.

## Independence

No code, assets, or naming lineage from any existing artillery game. See
`design/DESIGN.md` §7 for the pre-release checklist.

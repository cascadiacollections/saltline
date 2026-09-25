# Releasing

This describes the **0.1.0 desktop development preview**: what it is, what has
to be true before it goes out, and what it deliberately does not do.

It is a preview, not a product. Nothing in it is for sale.

---

## What the preview is

| | |
|---|---|
| Version | `0.1.0-preview` |
| Shape | Desktop, keyboard and controller, windowed or fullscreen |
| Platforms | Linux x86_64, macOS (Apple silicon and Intel), Windows x86_64 |
| Monetization | **None.** Built without the `monetization` feature; every screen is playable |
| Save data | Plain text at `user://salt_line.txt`, format version 2 (reads version 1/versionless) |
| Distribution | CI artifacts and GitHub releases. Not a store build |

Supported toolchain, pinned and checked in:

| | |
|---|---|
| Rust | `1.98.1` — `rust-toolchain.toml` |
| Godot | `4.7.2` — `project.godot` `config/features`, the CI export job, and the `api-4-7` feature on the `godot` crate |
| Python | 3.9+ with Pillow, managed by `uv`, only to regenerate `art/` |

## Build it

```sh
uv sync --locked
cargo build --release -p salt-line-godot
godot --headless --path . --import
godot --headless --path . --export-release "Linux" build/SaltLine.x86_64
```

`--import` is not optional on a clean checkout: without it the export succeeds
and ships with no art in it.

The monetized build is a different build:

```sh
cargo build --release -p salt-line-godot --features monetization
```

It arms the paywall at screen 15 and then has no storefront to satisfy it —
see "Monetization" below. Do not ship it.

---

## Release checklist

Everything here is either automated in `.github/workflows/ci.yml` or is a human
step that no amount of CI will cover.

**Automated — must be green on the release commit**

- [ ] `cargo test -p salt-line-sim` on Linux, macOS and Windows
- [ ] `cargo test -p salt-line-godot`
- [ ] `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -D warnings`
- [ ] `salt-line-sim` still has zero dependencies
- [ ] Determinism digests identical across all three platforms
- [ ] Batch solvability: 5,000 screens, zero unsolvable, zero `Flat` fallbacks
- [ ] Export succeeds for all three presets **and writes a file** — Godot has
      exited 0 having produced nothing
- [ ] Each exported artifact **launches and renders a frame** (the smoke job).
      This waits on the rendered PNG rather than the exit code, because a
      Godot-exported Windows binary is GUI-subsystem and returns 0 the instant
      it is started
- [ ] `.sha256` published next to each artifact

**Manual — before tagging**

- [ ] Version bumped in `Cargo.toml`, `project.godot` and `export_presets.cfg`,
      and they agree
- [ ] The playtest matrix below passes
- [ ] Fresh-machine install and launch on each platform (below)
- [ ] Known issues list updated and copied into the release notes
- [ ] Release notes state plainly: development preview, no monetization,
      unsigned binaries

**Fresh-machine check** — on a machine that has never had the toolchain:

- [ ] Linux: `chmod +x SaltLine.x86_64 && ./SaltLine.x86_64` on a stock distro
      with no Godot installed
- [ ] macOS: unzip, launch, work through the Gatekeeper warning, confirm it
      runs
- [ ] Windows: run the `.exe`, work through SmartScreen, confirm it runs
- [ ] Verify the published `.sha256` against the downloaded file

---

## Manual playtest matrix

The sim is covered by tests; none of this is. Run it on one platform per
release and on all three before any public build.

**Onboarding**

- [ ] Title → daily run → first screen with no prior save
- [ ] Non-blocking prompts teach angle, power, fire, correction/craters, wind,
      shell economy and the first draft; completed prompts stay completed
- [ ] Prompts swap keyboard/controller labels after the active device changes
- [ ] Debug overlay (`F1`) toggles and does not disturb the buffer

**Core loop**

- [ ] Angle and power, with and without `Shift`, at both ends of their ranges
- [ ] Firing, craters persisting, chalk marks after a miss
- [ ] Muzzle/impact/dust/debris effects and restrained shake line up with the
      recorded trace; collapse emphasis does not alter replay outcomes
- [ ] Fire/flight, terrain, structure, ricochet, airburst, collapse, draft,
      low-shell and UI sounds are audible and distinct
- [ ] Structures: intact → leaning → collapsed, for **all five** silhouettes
- [ ] Screen clear hold, then advance
- [ ] Shell economy: earning, spending, reaching zero

**Drafts**

- [ ] An offer appears every four screens; 1/2/3 all take
- [ ] Each modifier visibly does what its card says
- [ ] Stacking: take the same pick twice where the table allows it
- [ ] Chassis and barrel silhouettes change with the build
- [ ] Long Lens changes the buffer height without changing sprite scale

**Seeds and runs**

- [ ] Daily seed is the same on two launches on the same day
- [ ] Seed entry accepts a pasted seed and rejects a malformed one
- [ ] `C` copies the seed; `R` replays it; the replay matches
- [ ] A long run — 60+ screens — without a crash or a visual fault

**Window and display**

- [ ] Resize narrow and wide; the width floats, the height stays fixed
- [ ] Fullscreen and back
- [ ] Controller menus, aim/power repeat and dead zone, fire, draft, fuse,
      reposition, pause/back, copy and replay
- [ ] Controls screen volume/mute persist; edited `bind` lines work; reset
      defaults restores the documented layout
- [ ] No filtering, no shimmer while panning, at 1x and at large scales

**Game over**

- [ ] Out of shells
- [ ] Quit mid-run from the pause/escape path
- [ ] Score lands on the local board in the right position
- [ ] Board persists across a relaunch
- [ ] Delete `user://salt_line.txt` mid-session, finish a run, confirm a clean
      new file
- [ ] Corrupt the save by hand (truncate it, garbage a line) and confirm the
      board survives minus the damaged record
- [ ] Load a version-1 and versionless save; settings gain defaults. Load a
      future-version save; verify it is not overwritten

---

## Known issues and limitations

Copy this into the release notes, trimmed to what is still true.

**Deliberate for the preview**

- No monetization of any kind. The paywall exists in the code behind an
  off-by-default feature flag and has no storefront behind it.
- Binaries are unsigned and un-notarized. macOS Gatekeeper and Windows
  SmartScreen will both warn. See "Owner action required".
- Remapping is intentionally plain-text in this preview rather than an
  interactive capture UI. The in-game controls screen documents the format,
  displays active bindings and can reset defaults.
- Seed text entry and the F1 developer overlay require a keyboard.
- Programmer art throughout. Dimensions and the 16-colour palette are fixed and
  are the contract a real artist would work against.

**Open, tracked in `NOTES.md`**

- `Vernier` and `Salvage` have no effect on their second stack. Both are listed
  as `Stacks: 2` in DESIGN §4. This needs a design decision, not a patch.
- Band 0 has no wind at all (`wind = band * 0.35`), so the opening four screens
  teach the arc with nothing to read on the HUD. May be deliberate.
- `Overhang` is rare — about 4% of screens — because it needs both the unlock
  and band 5. If it is meant to be a signature screen, its weight is too low.
- No barrel strip for the Ballast or Forced March chassis; those builds read
  from the hull alone.

---

## Owner action required

None of these can be done from the repository. They are listed so nothing is
mistaken for "already handled".

- **Independence and employer review.** Complete the checklist in
  `design/DESIGN.md` §7 before anything is published.
- **Code signing.** An Apple Developer ID certificate and notarization for
  macOS; an Authenticode certificate for Windows. `export_presets.cfg` has
  signing explicitly disabled and says why.
- **Store presence.** If the game is ever sold, the `monetization` feature
  needs a real storefront SDK behind `crates/salt-line-godot/src/store.rs`:
  purchase, restore, and an entitlement query that is the authority. The local
  save flag is a cache and must never be trusted as proof of purchase.
- **Trademark and naming clearance** for "Salt Line".
- **Privacy statement.** The game collects and transmits nothing today. Say so
  explicitly if it is ever published anywhere that asks.

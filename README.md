# Salt Line

Single-player artillery roguelike. Endless procedural horizon, persistent
terrain damage, shells as the run resource, modifier drafting every four screens.

Deterministic Rust simulation core, Godot 4 presentation layer via `gdext`.

**Status: 0.1.0 desktop development preview.** Not for sale, nothing to buy,
every screen playable. Binaries are unsigned. `RELEASING.md` has the supported
platforms, the release checklist, and the known-issues list.

Requires Rust `1.98.1` (pinned in `rust-toolchain.toml`), Godot `4.7.2`, and
`uv` for the Python asset tools. `just` is optional but provides the documented
shortcuts below.

The `godot` crate is pinned to the matching `api-4-7` feature. If you raise the
engine version, raise that feature and `compatibility_minimum` in
`salt_line.gdextension` with it: bindings built against a newer API than the
running engine do not load, and the game boots to an empty window instead of
failing loudly.

## Contents

| Path | What it is |
|---|---|
| `design/DESIGN.md` | Full specification — mechanics, terrain, modifiers, art |
| `IMPLEMENTATION_PROMPT.md` | The brief this build was written against |
| `crates/salt-line-sim` | The whole game. No Godot dependency, no runtime dependencies at all |
| `crates/salt-line-godot` | Rendering, input, UI. Draws what the sim says is true |
| `art/` | Native-resolution sprites, 16-colour palette, reference scene |
| `tools/gen_sprites.py` | Regenerates all art; working reference for the terrain math |
| `site/` | The alpha marketing page, published to GitHub Pages |
| `tools/build_site.py` | Generates every image on that page out of the art pipeline |
| `tools/gen_audio.py` | Deterministically generates the original PCM sound effects |
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

With `just` installed, the same common operations are available as `just test`,
`just build`, and `just run`. The Python tools use the locked environment from
`pyproject.toml` and `uv.lock`; run `uv sync` once after checkout.

### Development builds

Every push to `main` that passes CI replaces the
[`dev` prerelease](https://github.com/cascadiacollections/saltline/releases/tag/dev)
with that commit's builds: Linux x86_64, macOS, Windows x86_64 and arm64, and
an Android arm64 APK. They are for testing, not distribution: the desktop builds
are unsigned, and the APK is signed with a debug key (with the
`ANDROID_DEBUG_KEYSTORE` repository secret, the same key as local
`just android` builds, so either installs over the other).

### Android

`just android` builds the arm64 APK and installs and launches it on the
attached device or emulator (set `ANDROID_SERIAL` if there is more than one);
`just android-log` follows its log. It needs rustup, the Android SDK and NDK
(`ANDROID_HOME`, `ANDROID_NDK_HOME`, defaulting to `~/Android/Sdk` and its
newest NDK), Godot's export templates, and a debug keystore in the Godot editor
settings. Phones use the Compatibility renderer; the desktop keeps Forward+.

### iOS

On a Mac, `just ios` builds the extension for arm64, exports an Xcode project
to `build/ios`, signs it, then installs and launches it on the attached iPhone
(set `IOS_DEVICE` to a name or UDID if there is more than one). Signing uses
Xcode's automatic provisioning, which also registers the phone with your
account. It needs a paid Apple developer account for installs that last a year.
It also needs:

- Xcode, signed in to that account under Settings > Accounts
- rustup and Godot's export templates
- your Team ID as `application/app_store_team_id` in the iOS preset of
  `export_presets.cfg` (it is public, and in every app you sign)
- Developer Mode on the iPhone (Settings > Privacy & Security), with the phone
  trusting the Mac

If the command-line signing step fails, `just ios-project` stops after the
export. Open `build/ios/SaltLine.xcodeproj`, pick your team under Signing &
Capabilities and press Run. CI builds the same project unsigned on every pull
request.

## Controls

| Keyboard | Controller | Touch | Action |
|---|---|---|---|
| Left / Right | D-pad or left stick | `<` `>`, or drag | Angle, one quantization step (Shift: ten) |
| Up / Down | D-pad or left stick | `-` `+`, or drag | Power (Shift: ten steps) |
| Space | A | FIRE, or let go of a drag | Fire |
| `[` `]` | LB / RB | `F-` `F+` | Airburst fuse distance (with Airburst) |
| `,` `.` | X / Y | `E<` `E>` | Reposition the emplacement (needs Enfilade) |
| 1 / 2 / 3 | X / A / B | Tap a card | Take a draft offer |
| Enter | A | Tap the row | Daily run, confirm, continue |
| S | — | SEED ENTRY | Seed entry (touch has an on-screen keypad) |
| C | X | COPY SEED | Copy the seed (on the run-over screen) |
| R | Y | SAME SEED AGAIN | Replay the same seed |
| V | Back/Select | CONTROLS / AUDIO | Controls and audio screen |
| M | Start | MUTE | Mute/unmute |
| L | X (on Controls) | LAST ARC | Keep the last shot's arc on screen (on by default) |
| F1 | — | — | Debug overlay: template, candidate count, camera, buffer size |
| Esc | B | `II`, or the back gesture | Pause / back |

Touch buttons are drawn in the game's own buffer and font (DESIGN §5.1, §5.6)
and sit along the bottom edge, which is always solid ground. Phones start with
them showing; elsewhere they appear on the first touch. Any key or controller
press switches back, and a tap switches to touch again. Holding a button
repeats like holding a key.

Touch aiming has two modes, chosen with AIM on the CONTROLS / AUDIO screen:

- **Buttons**: the `<` `>` `-` `+` and FIRE buttons.
- **Drag**: put a finger down on open ground and pull back, like a slingshot.
  The direction of the pull sets the angle and its length sets the power, both
  on the same grid as the keys. Let go to fire, or pull back to where you
  started to cancel. ARC chooses the preview shown while you pull. **Short**
  shows only the first stretch of flight, never past the apex or half the
  flight, so never the landing. **Full** shows the
  whole arc without wind. **Full + wind** adds the wind the HUD shows, which is
  none under Dead Reckoning. Short is the default: reading wind stays the
  player's job (DESIGN §2.5).

Until a player picks one, the defaults are `DEFAULT_TOUCH_AIM` and
`DEFAULT_ARC_HINT` in `game.rs`.

LAST ARC, on the same screen for every input, leaves the last shot's path
drawn as a faint dotted line until the next shot or the next screen. It is on
by default (`DEFAULT_LAST_ARC`). It overlaps Chalk Marks and Plumb Line, which
still add more: Chalk keeps up to two landings, and Plumb draws a clear guide at
the apex.

The game automatically changes onboarding labels after the most recent
keyboard, controller or touch input. Stick aiming uses a 0.45 dead zone and the same
delayed repeat as the keyboard, so simulation inputs remain quantized.

Volume, mute, learned onboarding steps, and bindings are stored in the
human-readable `user://salt_line.txt`. The controls screen displays current
bindings and resets them with R/Y. For practical custom remapping, add lines
using `bind ACTION KEY PAD`, for example `bind fire F X`. Supported actions are
`left`, `right`, `up`, `down`, `fire`, `fuse_down`, `fuse_up`,
`reposition_left`, `reposition_right`, `draft_1`, `draft_2`, `draft_3`,
`confirm`, `back`, `copy`, `replay`, `controls`, and `mute`. The displayed
default token names are accepted; unknown or malformed lines are ignored.

The barrel sprite shows the angle rounded to 5°; the numeric field shows the
true value. That gap is intentional — it is why `Vernier` feels like an
instrument rather than a stat.

## Tools

Everything here runs headless. None of it needs a human at the keyboard.

`cargo test` includes property tests (proptest). They check invariants
across random inputs: any recorded run replays to the same state, any save
reads back as written, and drag aim never leaves the grid. On a failure,
proptest shrinks the input to a minimal case and records it under
`proptest-regressions/`; commit that file so the case stays covered.
CI runs the tests with cargo-nextest (`.config/nextest.toml`), plus the
doctests. `just cov` prints line coverage, which CI also writes to the coverage
job's summary. `just hygiene` runs typos (spelling), zizmor (workflow security)
and cargo-machete (unused dependencies), as the hygiene job does.
Snapshot tests (insta) pin output that should only change on purpose: screen
digests per seed, the save file format, the HUD icons and a whole rendered
frame. Frames are stored as text, one palette digit per pixel, so a diff shows
exactly which pixels moved. After an intended change, run `cargo insta review`
(or `INSTA_UPDATE=always cargo test`) and commit the updated `.snap` files. A
changed screen digest is a determinism change and belongs in NOTES.md.
`just miri` runs the pure modules under Miri, the Rust interpreter that
catches undefined behaviour.
`just bench` runs the divan benchmarks. On this machine a whole screen,
solvability proof included, takes 40–105 µs, and a shot takes under a
microsecond. Two slower checks run weekly (`.github/workflows/weekly.yml`)
rather than on every pull request. Mutation testing (cargo-mutants,
`.cargo/mutants.toml`) reports code changes that no test notices; run it on one
file with `just mutants <file>`. Fuzzing (cargo-fuzz, `fuzz/`) runs the save
parser, the seed parser and replay reconstruction; run one locally with
`just fuzz <target>`.
`just deny` (cargo-deny, `deny.toml`) checks the dependency tree for security
advisories, disallowed licences and unknown sources. CI runs it on every pull
request.

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
uv run python tools/gen_sprites.py     # writes into art/
uv run python tools/gen_audio.py       # writes art/audio/
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

## The marketing page

`site/` is a static page — no framework, no build step for the markup, no
third-party requests. Its media is not hand-exported: `tools/build_site.py`
imports `tools/gen_sprites.py` and draws the hero screen, the animated shot and
the social card with the same palette, rasteriser and native grid the game uses.
Change a sprite and the page changes with it.

```sh
uv run python tools/build_site.py      # needs Pillow + Rust; writes into site/assets/
python3 -m http.server -d site 8000   # preview at http://localhost:8000
```

`.github/workflows/pages.yml` regenerates the media on every push to `main` that
touches `site/`, `art/` or either generator, then publishes `site/` to Pages.
The live URL is whatever Pages reports for this repository — the workflow logs
it and stamps it into the page's canonical and Open Graph tags, so nothing has
to be hardcoded while the repo is internal.

## The three things that matter

1. **`salt-line-sim` never depends on Godot.** It builds headless, and it has
   no runtime dependencies at all — not even `rand`. Tests may use
   dev-dependencies (proptest), which never reach a build of the game; CI
   fails if a normal dependency appears.
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

## Reporting bugs

The game has no network code and no telemetry. If the Rust side panics, the
game stops on a "SOMETHING BROKE" screen instead of carrying on with
half-updated state, and writes a plain-text report beside the save: version,
OS and architecture, the panic message and source location, and the run's
seed, width, screen and shot log (enough to replay it). **COPY REPORT** puts it
on the clipboard; the next launch offers **COPY LAST CRASH REPORT** on the title
screen, then keeps the file as `crash-old.txt`. Nothing leaves the device unless
you paste it somewhere.

Open an issue with the bug template and paste the report, or the seed if
nothing crashed. The files live in Godot's user data folder:

| Platform | Folder |
|---|---|
| Linux | `~/.local/share/godot/app_userdata/Salt Line/` |
| macOS | `~/Library/Application Support/Godot/app_userdata/Salt Line/` |
| Windows | `%APPDATA%\Godot\app_userdata\Salt Line\` |
| Android | App-private; use the in-game copy button |

The save (`salt_line.txt`) is written to a temporary file, flushed to disk and
renamed over the old one, so a crash or power cut mid-write costs at most the
newest record; if only the temporary file survives, the next launch loads it.

## License

MIT. See `LICENSE`.

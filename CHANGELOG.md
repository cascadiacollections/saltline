# Changelog

All notable changes to Salt Line are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A tag `vX.Y.Z` publishes a GitHub Release whose notes are that version's
section below, verbatim, so write entries for players first and developers
second. See RELEASING.md for the release steps.

## [Unreleased]

### Fixed

- Drone Trial spent most of each frame resampling its still coastline. The
  picture is scaled once per buffer size and copied after that.
- On a controller, d-pad up on the title started the challenge course and
  REPLAYS could not be reached, because both were bound to it. CHALLENGES is
  now d-pad left.
- On a controller, the third draft card could not be taken. It was bound to
  B, which pauses, and pause is read first. The third card is now Y.
- On a controller, A during a Drone Trial did not fire. Confirm is also A,
  and it was read first, so the press was spent resuming a round that was
  already going. A fires while the round is live, and resumes or restarts
  only from pause or the result.
- A very long one-shot streak could end the run, or crash a debug build,
  once the shell reserve passed the top of a 32-bit count. The reserve now
  stops at that top instead of wrapping.
- Play took about twice the CPU it needed on every frame, because the sky
  picture was painted again from scratch each time. It is now painted once
  per screen; on a Lenovo Tab (MediaTek MT8755) frame time halves and play
  holds the display's 90 Hz.

## [0.2.0] - 2026-10-09

A development preview: no monetization, and the desktop and Android builds are
unsigned (macOS and Windows will warn before they run).

### Added

- COPY CODE on a challenge's result screen puts its layout's share code on
  the clipboard. Paste it in the editor (PASTE A CODE) to test-fire, change
  or pass it on.
- Three more cosmetic biomes: Desert (cactus silhouettes, heat shimmer),
  Night (moon and stars) and Volcanic (rising embers, glowing caps). Runs
  pick among six themes from existing run data, so seeds, saves and replays
  are unchanged; challenges 4-6 use the new ones.
- Five more challenges (PAIR, TIGHT, GALE, TRIO, STORM): two- and
  three-target layouts and stronger wind, for eight in all. Each is a
  validated, solvable layout.
- The level editor and its test-fire now show the biome a layout's seed
  gives in a versus match (Snow, Forest or Salt). It is derived from the
  seed, so layout codes are unchanged.

- Biome polish: snow settles on top of standing structures and moss on forest
  ones, impacts throw white powder in snow and dark earth in forest, and
  versus matches take a biome too (their dust stays dark so side colours
  stay readable).
- Challenge progress: your most-shells-left result per challenge is saved,
  CHALLENGES on the title shows how many are cleared and starts the first one
  you have not, and the result screen offers RETRY (or PLAY AGAIN) with your
  bests instead of the run-only rows.
- Challenge hints: once you spend a shell, a faint dotted ghost arc shows a
  solving shot. Nothing is drawn on a fresh course.
- The controller is remembered: a phone that was last played with a gamepad
  starts in controller mode instead of touch, until you touch the screen.
- `just ios release` and `just ios-project release` build the iOS app with the
  optimised Rust library and Godot release template; the default stays debug.
- Cosmetic world biomes for play: normal runs now deterministically rotate
  between the original salt flats, snow, and forest palettes, while the
  bundled challenges use stable Snow, Forest, and Salt themes. Biomes change
  presentation only; terrain, ballistics, seeds, and replays remain intact.
- Biome atmosphere: Forest adds deterministic pine silhouettes, Snow adds
  drifting flakes, and Salt adds moving dust motes. These effects are
  presentation-only and remain hidden behind terrain.
- A first challenge course: CHALLENGES on the title opens three fixed,
  one-gun layouts with bounded shell budgets and a short HINTS readout.
  Clearing one advances to the next; running out of shells reports a failed
  course without affecting the normal run history.
- A map editor for local versus: MAKE MAP on the pre-match screen. Sculpt
  hills and dig chasms with a finger, smooth them, stamp ridges, mesas and
  spires, move the guns, set the wind, name it, test-fire both guns, and
  save it to one of 32 slots. Everything can be undone, and the map you are
  making is kept even if the app is killed. Works with a mouse, keys or a
  controller too.
- Custom duels: the pre-match screen's GROUND row plays a match on a map,
  every round on the same ground: four ship with the game, then your own.
  A rematch keeps the map, and a match on a map resumes after the app is
  killed like any other.
- MAKE MAP OF THIS ROUND, on a match's pause or result plate, starts a map
  from the round you are playing; the match waits for RESUME MATCH.
- Share a map as a code: COPY CODE and PASTE CODE in the editor, with a
  four-character fingerprint to check two phones hold the same map.
- The attract demo opens each part with a short card naming what is coming:
  ROGUELIKE (topple targets, draft a build) before a run, TWO PLAYERS (one
  phone, take turns) before a duel.
- Every download on a release, and on the `dev` build, now comes with a signed
  build provenance attestation, so you can check it was built by this
  project's CI from the tagged commit (`gh attestation verify`), and with
  `SaltLine-sbom.cdx.json`, a CycloneDX list of everything inside: every Rust
  crate, the Godot version and the Linux portal bridge. RELEASING.md,
  "Verifying a download", shows how.
- Replays: every finished run and every won match is kept (the last 20 of
  each) to watch again. REPLAYS on the title (`W`, d-pad up, or the plate
  under DEMO) lists them; WATCH on the end-of-run screen and on a won
  match's plate plays what you just played. The game plays the shots again
  itself, at 1x, 2x or 4x, with pause, NEXT SHOT and NEXT ROUND, and
  nothing you keep changes while you watch (#75).
- Star up to five favourite replays so they are never pushed off the list:
  the star beside a row, `S` or Y on the chosen row, or STAR at the end of
  a replay. Starred replays are listed first and don't count toward the
  20 runs and 20 matches; taking a star off gives the replay another 20
  games before it can go (#77).
- Share a replay as a code: COPY beside a row of REPLAYS (`C` or X on the
  chosen row), or COPY CODE at the end of a replay, puts a short `SR…`
  code on the clipboard to paste into a chat. PASTE CODE on REPLAYS (`P`)
  plays a friend's code at once, and KEEP adds it to your list. A typical
  run of 60 shots is about 350 characters; a three-round match on a map
  about 340. Paste the whole message: the game finds the code in it,
  even wrapped over lines. A code that was damaged on the way, or made by
  a newer version, says so rather than playing something else.
- Linux on ARM: `SaltLine-linux-arm64.zip` joins every dev build and
  release, for a Raspberry Pi 5, an Ampere or Graviton machine or an arm64
  laptop. It is built, exported and launched on an arm64 runner in CI, with
  the same GameMode bridge as the x86_64 build.
- For developers: the cross-platform determinism check now also runs on Android
  (x86_64 emulator) and iOS (arm64 simulator) in CI, and `just android-digest`
  runs it on an attached Android phone as arm64-v8a.
- For developers: layouts, the groundwork for a map editor and custom games
  (DESIGN §13). Any generated run screen or duel round can be forked into a
  layout and shared as a short checked code; `--salt-layout <code>` plays a
  local duel on one, and `--salt-fork-run` / `--salt-fork-duel` print a
  fork's code.
- For developers: replay codes are a packed binary payload (DESIGN §9.7,
  `history/code.rs`): a map's ground travels as its layout's bytes, not its
  `SL…` text, and the base32, CRC-16 and bit packing are now shared with
  map codes in `salt_line_sim::codec`. A `replay_code` fuzz target joins
  the weekly job.
- For developers: `--salt-editor` (with `--salt-editor-page` and
  `--salt-layout`), `--salt-setup` and `--salt-phone` (a Pixel's layout on a
  desktop, for headless captures); `just layouts editor` times the editor's
  checks; a `layouts_file` fuzz target joins the weekly job.

### Changed

- A map's fixed wind turns round every pair of turns, so both players shoot
  with it and against it as often (only maps have fixed winds; generated
  duels are unchanged).

### Fixed

- Starting a normal run after a challenge no longer keeps the challenge's
  rules (no score, challenge result screen).
- The Linux download now carries the game's libraries beside the binary.
  The zip used to hold the binary alone, which could not load the game; CI
  now unpacks each Linux zip on its own and launches it from there.
- The Windows downloads (x64 and ARM64) now carry `salt_line_godot.dll`
  beside `SaltLine.exe`. The zip used to hold the `.exe` alone, which could
  not load the game; CI now unpacks each Windows zip, and the macOS one, on
  its own and launches it from there.

## [0.1.0] - 2026-10-04

### Added

- The first six screens now feature a radio mast behind a ridge that calls for
  a lob and a silo across a chasm, so a short run shows more target variety.
- Sparse soil flecks and grass tufts make the ground less bare; craters clear
  the grass they dig through.
- Android build (arm64): landscape and immersive, with touch controls across
  the whole game: angle, power and FIRE buttons, tappable menus and draft
  cards, an on-screen keypad for seed entry, and the back gesture as "back".
- Drag aiming on touch screens: pull back from open ground like a slingshot and
  let go to fire, with a choice of arc preview (short, full, or full with wind).
  Pick it under Controls / Audio; the buttons remain the default.
- The last shot's arc stays on screen as a faint dotted line until the next
  shot. On by default; toggle it with LAST ARC on the Controls screen.
- Controller support, a pause screen, a controls screen, volume and mute, and
  plain-text key rebinding (`bind` lines in the save file).
- Sound effects and pixel feedback for firing, impacts, debris and collapses,
  with restrained camera shake.
- Contextual onboarding prompts that follow the device you are using.
- Development builds: every change to `main` publishes Linux, macOS, Windows
  (x64 and ARM64) and Android downloads as the rolling `dev` prerelease.
- Per-platform download links on the website.
- For developers: nextest, snapshot tests (insta), property tests (proptest),
  rstest, Miri, line coverage, cargo-deny, and typos / zizmor / machete checks
  in CI; design proposals for caches and seed rulesets (DESIGN §8).

### Changed

- A direct hit clears an already collapsed wreck so later shells can pass
  through it; the debris mound remains diggable ground.
- Playfield art is quieter: a flat deep sky replaces the busy full-screen
  backgrounds, and soil flecks and grass appear less often so targets and
  terrain edges stand out.
- Windows mouse clicks now feed the touch controls, including drag aiming.
- The HUD reads without words: a windsock that flies with the wind in place of
  `WIND +3`, and a shell icon in place of `SHELLS`.
- On phones, the playfield and HUD stay clear of camera cutouts and rounded
  screen corners.
- Vernier now refines aim precision on each stack, and Salvage refunds one
  shell per stack.
- Screen variety: encounter weighting improved, with every screen still proven
  solvable.
- Engine target raised to Godot 4.7.2.

### Removed

- The unused paywall and store code. Every build plays every screen. Old save
  files still load; their `unlocked` line is ignored.

[Unreleased]: https://github.com/cascadiacollections/saltline/compare/v0.1.0...main
[0.1.0]: https://github.com/cascadiacollections/saltline/releases/tag/v0.1.0

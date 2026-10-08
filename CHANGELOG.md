# Changelog

All notable changes to Salt Line are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A tag `vX.Y.Z` publishes a GitHub Release whose notes are that version's
section below, verbatim, so write entries for players first and developers
second. See RELEASING.md for the release steps.

## [Unreleased]

### Added

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
- For developers: the cross-platform determinism check now also runs on Android
  (x86_64 emulator) and iOS (arm64 simulator) in CI, and `just android-digest`
  runs it on an attached Android phone as arm64-v8a.
- For developers: layouts, the groundwork for a map editor and custom games
  (DESIGN §13). Any generated run screen or duel round can be forked into a
  layout and shared as a short checked code; `--salt-layout <code>` plays a
  local duel on one, and `--salt-fork-run` / `--salt-fork-duel` print a
  fork's code.
- For developers: `--salt-editor` (with `--salt-editor-page` and
  `--salt-layout`), `--salt-setup` and `--salt-phone` (a Pixel's layout on a
  desktop, for headless captures); `just layouts editor` times the editor's
  checks; a `layouts_file` fuzz target joins the weekly job.

### Changed

- A map's fixed wind turns round every pair of turns, so both players shoot
  with it and against it as often (only maps have fixed winds; generated
  duels are unchanged).

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

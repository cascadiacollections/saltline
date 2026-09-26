# Changelog

All notable changes to Salt Line are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A tag `vX.Y.Z` publishes a GitHub Release whose notes are that version's
section below, verbatim, so write entries for players first and developers
second. See RELEASING.md for the release steps.

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/cascadiacollections/saltline/commits/main

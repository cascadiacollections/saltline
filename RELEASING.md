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
| Platforms | Linux x86_64, macOS (one universal build: Apple silicon and Intel), Windows x86_64 and arm64; Android arm64 (sideload builds, touch controls); iOS arm64 (build and sign locally with `just ios`) |
| Monetization | **None.** No paywall and no store code; every screen is playable |
| Save data | Plain text at `user://salt_line.txt`, format version 2 (reads version 1/versionless) |
| Distribution | GitHub releases: a rolling `dev` prerelease from `main`, and a release per `vX.Y.Z` tag. Not a store build |

Supported toolchain, pinned and checked in:

| | |
|---|---|
| Rust | `1.98.1` — `rust-toolchain.toml` |
| Godot | `4.7.2` — `project.godot` `config/features`, the CI export job, and the `api-4-7` feature on the `godot` crate |
| Python | 3.10+ with Pillow, managed by `uv`, to regenerate `art/` and the site; `tools/release_version.py` needs only a stock Python 3.9+ |

## Build it

```sh
uv sync --locked
cargo build --release -p salt-line-godot
bash tools/godot_headless.sh --import
bash tools/godot_headless.sh --export-release "Linux" build/SaltLine.x86_64
```

`--import` is not optional on a clean checkout: without it the export succeeds
and ships with no art in it.

---

## Release checklist

Everything here is either automated in `.github/workflows/ci.yml` or is a human
step that no amount of CI will cover.

**Automated — must be green on the release commit**

- [ ] `cargo test -p salt-line-sim` on Linux, macOS and Windows
- [ ] `cargo test -p salt-line-godot`
- [ ] `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -D warnings`
- [ ] `salt-line-sim` still has zero dependencies
- [ ] Determinism digests identical across all seven sim platforms (Linux
      x86_64 and arm64, macOS arm64, Windows x86_64 and arm64, Android x86_64
      emulator, iOS arm64 simulator)
- [ ] `just android-digest` on an arm64 Android phone matches them
- [ ] Version consistency (`version` job, `just version-check`); on a tag it
      also checks the tag and the CHANGELOG section
- [ ] Batch solvability: 5,000 screens, zero unsolvable, zero `Flat` fallbacks
- [ ] Export succeeds for all desktop presets, including Windows ARM64, **and
      writes a file** — Godot has
      exited 0 having produced nothing
- [ ] Each exported artifact, including the Windows ARM64 artifact, **launches
      and renders a frame** (the smoke job). This waits on the rendered PNG
      rather than the exit code, because a Godot-exported Windows binary is
      GUI-subsystem and returns 0 the instant it is started
- [ ] `.sha256` published next to each artifact

**Manual — before tagging**

- [ ] Version bumped in `Cargo.toml`, `project.godot` and `export_presets.cfg`,
      and `just version-check` agrees (see "Version rule")
- [ ] `CHANGELOG.md`: `[Unreleased]` renamed to `[X.Y.Z] - YYYY-MM-DD`, a fresh
      empty `[Unreleased]` above it, link references updated
- [ ] The playtest matrix below passes
- [ ] Fresh-machine install and launch on each platform (below)
- [ ] Known issues list updated and copied into the release notes
- [ ] Release notes (the CHANGELOG section) state plainly: development
      preview, no monetization, and which binaries are unsigned

**Fresh-machine check** — on a machine that has never had the toolchain:

- [ ] Linux: `chmod +x SaltLine.x86_64 && ./SaltLine.x86_64` on a stock distro
      with no Godot installed
- [ ] Linux: exported build contains the XDG Portals bridge; on a desktop with
      GameMode available, a live run registers and pause/game over releases it.
      On a desktop without that portal, play and saves still work.
- [ ] macOS: unzip, launch, work through the Gatekeeper warning (none on a
      notarized release), confirm it runs, on Apple silicon **and** on an
      Intel Mac: CI never launches the universal build's x86_64 half
- [ ] Android: install the release APK over the previous release (same key,
      higher versionCode) and confirm the save survives
- [ ] Windows: run the `.exe`, work through SmartScreen, confirm it runs
- [ ] Verify the published `.sha256` against the downloaded file

---

## Cutting a release

Releases are tag-driven, from `.github/workflows/ci.yml`. There is no separate
release workflow: a tag runs exactly the jobs a push to `main` runs, and one
more at the end. What changed in each version lives in
[`CHANGELOG.md`](CHANGELOG.md) (Keep a Changelog format).

1. On a branch, bump the version everywhere it is written down (see "Version
   rule"), move the `[Unreleased]` entries of `CHANGELOG.md` into
   `## [X.Y.Z] - YYYY-MM-DD`, and merge it to `main`.
2. Tag the merge commit and push the tag:
   `git tag -a vX.Y.Z -m "Salt Line X.Y.Z"`, then `git push origin vX.Y.Z`.
3. CI builds and tests everything (sim on seven platforms, determinism,
   solvability, lint, the three desktop exports with their smoke launch,
   Windows arm64, Android). The `version` job fails the build if the tag does
   not match the tree, or if CHANGELOG.md has no section for it.
4. The `release` job then publishes a normal (not pre-) GitHub Release
   `vX.Y.Z`, marked latest, with the same files as the `dev` prerelease plus
   `SHA256SUMS`, and that version's CHANGELOG section as its notes.
5. Right after, bump `main` to the next version (for example `0.1.1`) with an
   empty `[Unreleased]` section, so dev builds overtake the release on Android
   (see "Android versionCode").

A failed tag build publishes nothing. Fix forward: delete the tag
(`git push origin :vX.Y.Z`), fix, and tag again. If the release job itself
failed after creating the release, delete the release as well before re-running.

Only a commit on `main` can be released: the `version` job fails a tag that
points anywhere else. The release is marked **latest** unconditionally, so
tagging an older line after a newer release (say `v0.1.2` after `v0.2.0`)
would make the older one "latest"; fix that by hand in the release settings
if it ever happens.

Tag builds never restore the Rust cache. A tag run can only restore caches
written on `main` (pull request caches are scoped to `refs/pull/N/merge`), but
a release should not depend on even that, so tag builds start cold and take
longer; the macOS job's timeout is raised to 75 minutes on tags to cover that
and the notarization wait.

### Version rule

A release version is plain `MAJOR.MINOR.PATCH`, and the tag is that with a
`v` in front. All of these must be equal, and `tools/release_version.py check`
(the `version` job, `just version-check`) enforces it on every CI run:

- `Cargo.toml` `[workspace.package] version`
- `project.godot` `config/version`, **up to its first `-`**. The part after it
  is a channel label for players (`0.1.0-preview` today) and is ignored, so the
  label can be dropped or changed without counting as a version change
- `export_presets.cfg`: Android `version/name`; macOS `short_version` and
  `version`; both Windows presets' `file_version` and `product_version`
  (which carry a fourth `.0`)

Pre-release tags such as `v0.2.0-rc.1` are rejected by the gate; the rolling
`dev` prerelease already serves that purpose. The Android versionCode scheme
bounds the version: major at most 20, minor and patch at most 99.

### Android versionCode

Android refuses to install an APK over one with a higher `versionCode`, and
Godot's CLI cannot override it, so CI writes it into the Android preset of its
own checkout before exporting. The `1` committed in `export_presets.cfg` is
only used by local builds.

```
versionCode = major × 100,000,000 + minor × 1,000,000 + patch × 10,000 + build
build       = 9999                            for a vX.Y.Z tag
            = min(github.run_number, 9998)    for main and pull request builds
```

So `v0.1.0` is `1009999`, and the dev build from run 70 once `main` says
`0.1.1` is `1010070`. The order that falls out:

- Every build of a higher version beats every build of a lower one.
- Within one version, dev builds rise with the run number and the release
  beats all of them. That is why `main` is bumped straight after tagging: a
  dev build still carrying the released version would be refused over the
  release.
- A local `just android` build keeps `versionCode=1`, lower than any CI build,
  so it installs with `adb install -r -d`: `-d` permits the downgrade, which
  Android allows only for a debuggable APK, and `just android` exports a debug
  one. The key must still match (below).

Dev builds also get `version/name` `X.Y.Z-dev.<run>`, so Settings > Apps shows
which build is installed.

**The 9998 cap is permanent.** `github.run_number` never resets: it counts
every run of this workflow, pull requests included, across all versions (about
70 by 0.1.0). Once it passes 9998, every later dev build gets `build = 9998`
for good. They still install over each other (Android accepts an equal code)
and releases still beat them, but dev builds within one version are no longer
ordered by age. When that is near, change the scheme in
`tools/release_version.py`, for example subtract a recorded offset from the
run number, or reset the build field per version from the tag history, and
note the change here.

**Keys come first.** Android only compares versionCodes between APKs signed
with the same key; an APK signed with a different key does not install at all
until the old one is uninstalled, which deletes `user://` saves.

- Pushes to `main` (dev builds) and `v*` tags are signed with the **release
  key** once it exists (see "Owner action required"), so dev builds and
  releases install over each other in versionCode order. Consequence: a dev
  build installed over a player's release replaces it, and the player cannot
  go back to that release, because it has the lower versionCode. Their way
  back is the next release, which beats every dev build before it.
- Pull request builds are always signed with the **debug key**
  (`ANDROID_DEBUG_KEYSTORE`, the same key as a local `just android`). They do
  not install over release-signed builds, or those over them, without an
  uninstall.
- Until the release key exists, main and tag builds fall back to the debug key
  with a warning, and everything installs over everything in versionCode order.

### macOS universal build

The macOS export ships one universal extension: CI builds
`aarch64-apple-darwin` and `x86_64-apple-darwin`, `lipo`s them into
`target/universal-apple-darwin/release/libsalt_line_godot.dylib` (what
`macos.release` in `salt_line.gdextension` points at), and checks both the
built library and the one inside the exported `.app` with `lipo`. Godot's
export template is itself universal. Debug builds stay host-only at
`target/debug`, so `cargo build` and `just run` are unchanged; to export a
release on a Mac, run `just macos-universal` first. The x86_64 half is built
and checked but never launched in CI (the runner is Apple silicon), so the
Intel launch stays in the fresh-machine checklist.

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

- No monetization of any kind. There is no paywall and no store code.
- Desktop binaries are unsigned and un-notarized unless the owner has set up
  the signing secrets below, so macOS Gatekeeper and Windows SmartScreen will
  both warn. Android dev builds and releases are signed with the debug key
  until the release key exists. See "Owner action required".
- Remapping is intentionally plain-text in this preview rather than an
  interactive capture UI. The in-game controls screen documents the format,
  displays active bindings and can reset defaults.
- Seed text entry and the F1 developer overlay require a keyboard.
- A saved run is not carried back across a downgrade in one case: after a
  score that could not be written (a full disk, or a save from a newer
  build), `run.txt` holds more than one line, and an older build, which
  expects one, reads it as damaged and discards it, with the run in progress
  and the scores still waiting in it.
- Programmer art throughout. Dimensions and the 16-colour palette are fixed and
  are the contract a real artist would work against.

**Open, tracked in `NOTES.md`**

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
- **The `release` environment and a `v*` tag ruleset.** See below. Do this
  before adding any signing secret.
- **Android release key.** See below. Until it exists, dev builds and
  releases are signed with the debug key.
- **macOS signing and notarization.** See below. Until the secrets exist, the
  macOS build is unsigned.
- **Windows code signing.** Documentation only, see below.
- **Trademark and naming clearance** for "Salt Line".
- **Privacy statement.** The game collects and transmits nothing today. Say so
  explicitly if it is ever published anywhere that asks.

### The `release` environment and tag ruleset

A step's `if:` is not a security boundary: anyone with write access could push
a branch whose workflow simply prints a repository secret. So the signing
secrets live in a GitHub **Environment** named `release`, and only runs the
environment admits can read them. CI runs the Android job in it for pushes to
`main` and `v*` tags, and the desktop export jobs (macOS signing) for `v*`
tags only; pull requests never
enter it.

1. Settings > Environments > New environment, named `release`.
2. Deployment branches and tags: **Selected branches and tags**, with the
   branch rule `main` and the tag rule `v*`. Nothing else may deploy to it.
   Optionally add yourself as a required reviewer, which makes every main push
   and tag wait for approval (the price of a manual gate on each dev build).
3. Add the `ANDROID_RELEASE_*` and `APPLE_*` secrets below as **environment
   secrets** of `release`, not repository secrets (`gh secret set NAME --env
   release`). Only `ANDROID_DEBUG_KEYSTORE` stays a repository secret.
4. Settings > Rules > Rulesets > New tag ruleset, targeting `v*`: restrict
   creation, update and deletion to the maintainers (bypass list), so no one
   else can push a tag that the environment would admit. Protect `main` with a
   branch ruleset as well (pull requests required), since a push to `main` is
   admitted too.

Owner decision: **main dev builds are signed with the release key** (see "Android versionCode", "Keys come first"). The alternative that was
considered was a separate package id for dev builds
(`com.cascadiacollections.saltline.dev`, installable side by side, with its own
saves and the debug key, keeping the release key to tags only). It was not
chosen because a dev build could then never upgrade a player's install or
carry their saves; the cost of the chosen option is that the release key is
used on every push to `main`, so the protection of `main` matters as much as
that of the tags.

### Android release key

Every APK a player installs must be signed by the same key for the rest of the
game's life. Android refuses to update an app with an APK signed by a
different key, so a changed or lost key forces players to uninstall, and
uninstalling deletes `user://`: their saves, settings and board. Outside
Google Play's app signing, which is not in use, a lost key cannot be
recovered.

Generate it once, on a trusted machine, valid for 30 years (25+ is the norm;
Google Play would require validity past 2033). PKCS12, keytool's default,
uses one password for the store and the key, which is what Godot expects:

```sh
keytool -genkeypair -v \
  -keystore saltline-release.keystore -storetype PKCS12 \
  -alias saltline -keyalg RSA -keysize 4096 -validity 10950 \
  -dname "CN=Salt Line, O=Cascadia Collections, C=US"
keytool -list -v -keystore saltline-release.keystore   # note the SHA-256
```

Store it as three environment secrets of `release` (see above; `gh secret set
NAME --env release`):

| Secret | Value |
|---|---|
| `ANDROID_RELEASE_KEYSTORE` | `base64 < saltline-release.keystore` (`base64 -w0` on Linux for one line) |
| `ANDROID_RELEASE_KEYSTORE_PASSWORD` | the password chosen above |
| `ANDROID_RELEASE_KEY_ALIAS` | `saltline` |

Then keep an **offline backup**: the keystore file and its password in a
password manager, plus a second copy on encrypted offline media stored apart
from it. A GitHub secret cannot be read back, so it is not a backup. The
workflow asks for these secrets only on pushes to `main` and `v*` tags, and
once they are environment secrets GitHub enforces that: a run the `release`
environment does not admit (a pull request, another branch, another tag)
cannot read them. Until they are moved there, a repository secret is readable
by any workflow run with write access behind it. The Android job prints each
APK's certificate digest (`apksigner verify --print-certs`), which must match
the one noted above.

An Android App Bundle (`.aab`) is required only to publish on Google Play,
which is not planned; sideloaded APKs are the distribution. If Play ever
happens it needs a Gradle build (`gradle_build/use_gradle_build`) and a
decision about Play App Signing, with this key as the upload key.

### macOS: Developer ID and notarization

Needs the paid Apple Developer Program. Tag builds sign the app (hardened
runtime), notarize it and staple the ticket, but only when all five secrets
are set; otherwise those steps are skipped with a warning and the app ships
unsigned, as today.

1. **Developer ID Application certificate.** Create it in Xcode (Settings >
   Accounts > Manage Certificates) or on developer.apple.com (Certificates >
   "+" > Developer ID Application), then export it with its private key from
   Keychain Access as a password-protected `.p12`.
2. **App Store Connect API key** for `notarytool`: App Store Connect > Users
   and Access > Integrations > App Store Connect API > Team Keys, role
   Developer. Download the `.p8` (possible only once) and note the key ID and
   the issuer ID.
3. Environment secrets of `release` (tag builds only use them):

| Secret | Value |
|---|---|
| `APPLE_DEVELOPER_ID_P12` | `base64 < DeveloperID.p12` |
| `APPLE_DEVELOPER_ID_PASSWORD` | the `.p12` export password |
| `APPLE_NOTARY_API_KEY` | `base64 < AuthKey_KEYID.p8` |
| `APPLE_NOTARY_KEY_ID` | the key ID |
| `APPLE_NOTARY_ISSUER` | the issuer ID |

Keep the `.p12` and `.p8` offline as well. A Developer ID certificate lasts
five years and can be replaced without harm, unlike the Android key. Signing
happens in CI after the export, so `export_presets.cfg` keeps Godot's own
signing off.

### Windows code signing

Not wired up, so SmartScreen will warn. The options, for when it matters:
**Azure Trusted Signing** (cheapest, and CI-friendly through
`azure/trusted-signing-action`, but its eligibility for individuals and new
organisations has been restricted and keeps changing, so confirm it first),
or an OV certificate from a CA on a hardware token or cloud HSM (a few hundred
dollars a year; SmartScreen reputation still has to build up over downloads).
Either would sign `SaltLine.exe` and `SaltLine-arm64.exe` with `signtool`
before packaging.

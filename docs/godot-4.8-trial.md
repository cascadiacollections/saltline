# Godot 4.8-beta5 trial

Salt Line's editor, CI and release exports stay on Godot **4.7.2**. This
trial builds and exports the same project against the official 4.8-beta5
editor and export templates, then stops. It does not change
`tools/godot_templates.lock.json`, `GODOT_VERSION`, or the `api-4-7` feature.

4.8-beta5 is a preview. Older notes that cite a 71 MB native library or an
87 MB APK are not a current Salt Line package measurement. The numbers
below were taken on 2 Nov 2026 from this trial.

## Measured on 2 Nov 2026 (4.8-beta2; beta5 numbers are on #127)

Cloud VM, official editor and export templates, same Salt Line commit, headless.
No Android SDK was installed, so there is no Salt Line APK or IPA from this
machine. The Linux exports are the game packages. The Android rows are the
official export-template archives (every ABI), which is the engine a
template-based APK packages before Gradle strips it. The GitHub workflow
exports the game APK when it is run.

Both exported Linux binaries launched and wrote the same 1280×720 PNG
(31,075 bytes, SHA-256 `1b560a9e80443c3274ad50187e9ad7e13e4cd2a5cfcbc7c4578cef7d0f144a68`).
The `api-4-7` GDExtension loaded. No script error, deprecation, or
GDExtension failure appeared in the 4.8-beta2 import log that was not also
in the 4.7.2 log. Both processes logged `1 resources still in use at exit`
after the screenshot; that line is not new in 4.8.

| | 4.7.2 official | 4.8-beta2 official | delta |
| --- | ---: | ---: | ---: |
| Linux export (engine + pck) | 78,591,488 B | 81,727,232 B | +3,135,744 B |
| `libsalt_line_godot.so` beside it | 7,559,712 B | 7,559,712 B | 0 |
| `linux_release.x86_64` template | 73,519,416 B | 76,655,160 B | +3,135,744 B |
| `android_release.apk` template | 104,803,333 B | 102,428,389 B | −2,374,944 B |
| `libgodot_android.so` arm64, uncompressed | 71,114,944 B | 64,818,536 B | −6,296,408 B |
| same library, deflated in the template APK | 24,023,949 B | 23,361,165 B | −662,784 B |

The Linux export grew by exactly the template binary's growth, so the
embedded pack did not change size. The official arm64 Android library is
smaller in 4.8-beta2, not larger. 71,114,944 B is still the size of the
official 4.7.2 arm64 library inside `android_release.apk`; it is not a
Salt Line APK. Editors checked out as `4.7.2.stable.official.ed1daf0bf` and
`4.8.beta2.official.cafc17a19`, matching the lock and `version.txt`.

Opening the project in 4.8-beta2 rewrites PNG `.import` files with three
default keys (`compress/high_quality_mode=0`,
`mipmaps/preserve_alpha_test_coverage=false`,
`mipmaps/alpha_test_threshold=0.5`). The trial restores those files. They
were not committed: the values are the defaults, and writing them would
churn the 4.7.2 tree. The shared build profile matches the 4.8-beta2 source
once `module_regex_enabled` is dropped for the trial build only. The
stripped engine was not compiled here.

No GDScript or gdext change was required for this headless pass.

## How to run it

Locally, with a release GDExtension already built
(`cargo build --release -p salt-line-godot`):

```sh
just godot-48-trial
# or: python3 tools/godot_4_8_trial.py run
```

That downloads the pinned editor and export templates, checks their
provenance, imports and exports Linux on 4.7.2 and 4.8-beta5, and writes
`build/godot-4.8-trial/report.md`. When `ANDROID_HOME` and `JAVA_HOME` are
set and `target/aarch64-linux-android/release/libsalt_line_godot.so` exists,
it also exports a debug-signed APK for each version.

In GitHub Actions, run the **godot-4.8-trial** workflow by hand. Pull
requests do not run it, and it is not a required check. The run exports both
official templates (Linux and Android) and uploads the report. The optional
**custom engine** checkbox builds a stripped Android arm64 template. That
job is allow-failure. It does not publish a release and does not edit the
4.7.2 lock.

```sh
just godot-48-trial-engine          # local stripped build; needs uv and the Android SDK
python3 tools/godot_4_8_trial.py check-python
```

## Provenance

`tools/godot_4_8_trial.lock.json` records the editor version, the engine
commit, the SHA-512 of the official Linux editor and export templates for
both 4.8-beta5 and the 4.7.2 comparison, and the custom-build key.

`run` downloads `SHA512-SUMS.txt` from the matching `godotengine/godot-builds`
release and refuses the files unless the lock, the published sums, and the
downloaded bytes all agree. It then checks that `godot --version`,
`templates/version.txt`, and `linux_release.x86_64 --version` carry that
version and the commit's 9-character prefix.

A stripped trial build writes `build-report.json`. `validate-build` requires
its tag, commit, `GODOT_VERSION_STATUS=beta5`, custom-build key, and
SHA-256 to match the binary. The key hashes the 4.8-beta5 commit, the pinned
SCons version, the shared build profile minus the trial-only drops, and the
same release flags as 4.7.2. It is not the shipping lock key.

The `4.8-beta5` tag is not on `godotengine/godot` (the snapshot is published
from godot-builds). The trial build fetches commit
`a661dfdbccc664ccca57ad76c2bb5dcd8f3fc768` and sets
`GODOT_VERSION_STATUS=beta5`, which is what the official editor reports
(`4.8.beta5.official.a661dfdbc`).

## Python, for the stripped build only

Godot 4.7.2's `SConstruct` calls `EnsurePythonVersion(3, 9)`. 4.8-beta5
calls `EnsurePythonVersion(3, 10)`. `check-python`, and `build`, refuse an
older interpreter. Salt Line's 4.7.2 scripts are unchanged and still run on
Python 3.9+.

## Profile notes for a custom build

`module_regex_enabled` is dropped for the trial build only. 4.8-beta5 has no
`modules/regex`, and the shared profile check would refuse the option.
`tools/godot_build_profile.json` is not edited, so the 4.7.2 lock key stays
put.

`rendering_device=false` is set for the trial build only (`profile_sets` in
the trial lock). 4.8 adds a `rendering_device` SCons option, on by default,
and defines `RD_ENABLED` from it rather than from `vulkan`. With only
`vulkan=false`, as the 4.7.2 profile has it, the stripped Android build fails:
`display_server_android.cpp` calls `free_vulkan_global_context()` under
`RD_ENABLED`, but declares it only under `VULKAN_ENABLED` (seen on beta3, beta4 and beta5,
same code on beta2 and `master`). The option does not exist in 4.7.2, so it
cannot go in the shared profile until the pin moves.

These modules are in the 4.8-beta5 tree and not in the 4.7.2 tree, and the
trial leaves them at Godot's default (enabled): `texture_streaming`,
`tilemap`, `visionos_xr`. Worth a look before any real pin. Their presence
is not, by itself, a measured package-size regression.

## Device checklist

These are upstream 4.8 fixes to verify on hardware. They are not confirmed
Salt Line defects. The Linux trial cannot sign, install on a phone, or
produce an IPA.

### ARM64 launch

Install an arm64 Android build (the trial APK from the workflow, or a local
`just android` pointed at 4.8-beta5) and launch it. iOS needs a Mac, a
signing team, and the existing TestFlight flow (`docs/testflight.md`).

### Touch, including clipped and off-screen buttons

Godot 4.8 fixes button touch handling against the visible viewport. Salt
Line does not use Godot `Button` nodes. `game.rs` turns
`InputEventScreenTouch` and `InputEventScreenDrag` into the `Node2D`'s local
buffer pixels with `make_input_local` (Godot's coordinates are already past
stretch and letterboxing), and `touch.rs` hit-tests the rectangles the
renderer laid out.

On a phone, compare 4.7.2 and 4.8-beta5: controls against the letterbox or a
cut-out, a press that starts off a button, and a press on a control that
sits partly outside the visible area. A miss here would be the upstream
viewport fix not reaching that mapping, which this trial has not confirmed.

### Multiply blending on the Compatibility renderer

Godot 4.8 fixes overly dark multiply blending in the Compatibility
renderer. Salt Line selects that renderer and composites its frame in
software before handing Godot a texture, so the fix may not move the
picture.

On Android and on iOS, compare night, embers, and any darkened sprites or
effects between a 4.7.2 build and a 4.8-beta5 build of the same game
commit. Look for a change in darkness. This is the upstream fix to verify,
not a known Salt Line bug.

### Save and resume

Start a run, leave the app, kill it, and continue. Also pause and resume
from the app switcher. The trial does not change the save format.

## What still needs a person

- A phone for the checklist above. This VM rendered one identical headless
  frame from each Linux export. It cannot press the glass, and it had no
  Android SDK, so it did not build a Salt Line APK. Run the workflow for
  that APK; the job signs it with a debug key.
- Android release signing, if the trial APK should install over a
  release-signed build. The workflow signs with a debug key.
- An iOS export. There is no Mac runner in this trial. Use
  `docs/testflight.md` on 4.8-beta5 when you want an IPA, and do not point
  the 4.7.2 TestFlight steps at the beta by accident.
- A decision, later, about the new default-on modules and the removed regex
  module before anyone bumps the shipping pin. Follow "Bumping Godot" in
  `docs/godot-templates.md` only for a stable release.

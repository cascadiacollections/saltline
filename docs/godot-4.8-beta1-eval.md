# Godot 4.8 beta 1 evaluation

2026-10-19, on the eval branch `eval/godot-4.8-beta1` only. The owner decided not to switch
engines: production stays on 4.7.2 (engine and stripped templates together), and nothing here
is meant to merge.

The 4.8 beta 1 editor and export templates are the official builds from
godotengine/godot-builds, tag `4.8-beta1` (`4.8.beta1.official.4d10419ca`). Their SHA512 matches
`SHA512-SUMS.txt`. They are installed next to 4.7.2: the editor in `~/.local/share/godot/bin/`
and the templates in `export_templates/4.8.beta1`.

The branch carries two eval-only changes:

- The Android preset's package is `com.cascadiacollections.saltline.eval48`, so it never
  installs over the real app.
- `GODOT_TEMPLATES_DIR` overrides the template directory in `tools/godot_template.py` and the
  `.build_version` it writes.

## Sizes (release export, arm64-v8a, Gradle; Pixel 10 57220DLCR001SH)

| Build | APK | libgodot_android.so | Installed (`du -sk` of the app dir) |
|---|---:|---:|---:|
| 4.7.2 official | 87,500,499 | 71,114,944 | 85,543 KB |
| 4.7.2 stripped (lock `96290a6ebc101c9c`, shipping) | 39,577,299 | 23,186,040 | 38,687 KB |
| 4.8 beta 1 official | 81,913,301 | 65,524,792 | 80,075 KB |

The official 4.8 engine is 5.6 MB (8%) smaller than the official 4.7.2 engine. It is still
2.8x the stripped 4.7.2 engine. The libraries are stored uncompressed.

## Findings

1. **gdext**: the `api-4-7` build (gdext 0.5.5) loads and runs on 4.8.
   - The log reads `Initialize godot-rust (API v4.7.stable.official, runtime v4.8.beta1.official, safeguards strict)`.
   - There are no gdext warnings, on desktop or on the device.
   - gdext 0.5.5 has no `api-4-8` feature. An `api-custom` build against the 4.8 binary
     (`GODOT4_BIN`) compiles with no warnings. On 4.8 it passes `--salt-validate-engine` with
     the same report. Not committed.
2. **Engine validation**: `--salt-validate-engine` gives a byte-identical `report.json` on
   4.7.2 and 4.8 (state, frame and sound hashes for the run, duel and map fixtures).
   - The PNGs are pixel-identical (ImageMagick AE 0), but the files are about 1.6% larger:
     4.8's PNG encoder changed.
   - Anything that hashes PNG bytes would see a difference.
3. **Touch on the Pixel (4.8)**: launch, DAILY RUN, the angle `>` and power `+` buttons
   (45.0/600 to 45.5/605) and FIRE (ammo 40 to 39, shell in flight) all work.
   - Multi-touch was not exercised: `adb input` sends only one pointer, and injecting raw
     events needs root.
   - Salt Line's `android_profile.py` was not run on 4.8. The racer's frame-time comparison
     covers the engine.
4. **Saves (desktop, temporary `XDG_DATA_HOME`)**: `user://` resolves to the same directory on
   both engines.
   - A run started on 4.7.2 (`--salt-seed --salt-auto`) continues on 4.8 (`--salt-continue`),
     and the reverse also works. The `run.txt` shots extend the same seed.
   - The same seed writes a byte-identical `run.txt` on both engines.
   - Loading the real `salt_line.txt` (version 2), `match.txt` and `drone_trial.replay` on
     either engine leaves them unchanged.
   - Layouts, draft and history go through the same Rust file I/O. They were not exercised one
     by one.
   - On the device, the eval package has its own data, so no real saves were touched.
5. **godot-device-power AAR** (built against `org.godotengine:godot:4.7.2.stable`) inside the
   4.8 Gradle export:
   - `DevicePowerAndroid` registers.
   - The log shows `backend=android_plugin` and `frame rate vote 120.0 via android_plugin: ok`
     (rates 60/120), the same as 4.7.2 official and stripped.
6. **Editor-managed Android/Java SDK**: our tooling needs a change before any switch.
   - The first 4.8 export failed with `Invalid Java SDK path in Editor Settings`.
   - 4.8 seeds `export/android/java_sdk_path` from `JAVA_HOME` only when the key is missing.
     Otherwise it defaults to `<data>/godot/java-sdk-17`, an editor-managed JDK 17. An existing
     `editor_settings-4.8.tres` with an empty value is not filled from `JAVA_HOME`.
   - CI starts with fresh editor settings and exports `JAVA_HOME`, so it should still work.
     Local runs need the setting, or the isolated-editor approach `android_profile.py` already
     uses.
   - The Android SDK path defaults to `<data>/godot/android-sdk` the same way when
     `ANDROID_HOME` is unset.
7. **Gradle template changes**:
   - AGP 8.6.1 to 8.13.2 and Gradle 8.11.1 to 8.14.5.
   - New `dataLib`, `fusedDepsLib` and `assetPackInstallTime` modules, with assets copied into
     `dataLib`.
   - The template's ndkVersion is 29.0.14206865 (our NDK is 30).
   - `prepare-android` overlays still apply.
   - `android/.build_version` must read `4.8.beta1`.
   - Export prints `ERROR: EditorSettings not instantiated yet ... shutdown_adb_on_exit` at exit.
     It is harmless (a beta bug).
8. **xdg_portals on 4.8 desktop**: `XdgPortalNative` registers (13 methods) and instantiates on
   both engines.
   - **New in 4.8**: the Linux engine requests Feral GameMode for the whole process by default.
     The new setting `application/run/use_game_mode` defaults to true, and the log reads
     `GameMode: Enabled successfully.`
   - That defeats our "GameMode only during active play" bridge. A switch must set
     `application/run/use_game_mode=false`.
9. **Stripped templates**: not built for 4.8.
   - `tools/godot_build_profile.json` is **not** valid as is for 4.8: `module_regex_enabled` no
     longer exists (RegEx moved to `core/string`), and `check_profile` would fail.
   - 4.8 adds the `tilemap`, `texture_streaming` and `visionos_xr` modules. The first two are
     candidates to disable.
10. **Editor rewrites**: the 4.8 import and export changed no tracked file: `project.godot`,
    `.uid` files and scenes are untouched. The import log has no deprecation warnings.

## Risks

- Beta: Android SDK management is new code.
- The build profile breaks (RegEx).
- Engine-wide GameMode is on by default.
- Gradle and AGP bumps affect our manifest overlay.
- gdext has no `api-4-8` until a gdext release.

## Recommendation

Stay on 4.7.2. Revisit at 4.8 RC1 or stable, with:

- a gdext release that has `api-4-8`;
- `godot_build_profile.json` updated (drop `module_regex_enabled`, consider disabling `tilemap`
  and `texture_streaming`) and a stripped 4.8 build measured;
- `application/run/use_game_mode=false`;
- an explicit `java_sdk_path` in every editor-settings path we use.

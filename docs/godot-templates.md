# The stripped Godot engine

Salt Line draws every pixel in Rust and hands Godot a few textures, a sound
player and the input events. The official export templates carry the whole
engine anyway: 3D, physics, navigation, XR, Vulkan, the text shaping stack,
every image and audio codec. On Android that is a 71 MB library, deflated to
25 MB inside a 37 MB APK.

So the Android release APK ships a Godot engine built from the same source
tag without those parts. Everything else in the APK (the Java side, the
manifest, the resources) is the official template of the same Godot version;
only `lib/arm64-v8a/libgodot_android.so` is replaced.

| Release APK (arm64, 4.7.2) | Official template | Stripped engine |
| --- | ---: | ---: |
| APK | 36,685,396 B | 18,671,188 B (−49%) |
| `libgodot_android.so`, uncompressed | 71,114,944 B | 23,186,048 B (−67%) |
| `libgodot_android.so`, deflated in the APK | 25,072,330 B | 8,439,831 B |

The APK figure also includes a smaller GDExtension: with gdext's lazy method
tables (below) `libsalt_line_godot.so` fell from 5.1 MB to 3.7 MB.

On the phone the saving is larger than the download. The template's
manifest sets `extractNativeLibs="true"`, so Android unpacks every library
next to the APK at install: about 36.7 + 71.1 + 5.1 + 1.4 = 114 MB installed
on the official template against 18.7 + 23.2 + 3.7 + 1.4 = 47 MB on the
stripped one (APK plus the unpacked `libgodot_android.so`,
`libsalt_line_godot.so` and `libc++_shared.so`; computed from the files, not
read off a phone).

## What is left out, and why it is safe

`tools/godot_build_profile.json` is a Godot build profile: the SCons options
it sets are applied as if passed on the command line. `tools/godot_template.py`
adds how it is compiled: `target=template_release production=yes lto=full
optimize=size_extra debug_symbols=no`.

| Left out | Why Salt Line does not need it |
| --- | --- |
| `disable_3d`, `disable_physics_2d/3d`, `disable_navigation_2d/3d`, `disable_xr` | One `Node2D` draws textures; the sim (`salt-line-sim`) does its own ballistics. |
| `disable_advanced_gui` | No `Control` nodes; every button and plate is rastered in Rust. |
| `vulkan`, `d3d12`, `metal` | `rendering_method` is `gl_compatibility` on every platform (project.godot). |
| `module_text_server_*`, `module_freetype`, `module_msdfgen`, `brotli` | No Godot text is drawn (`font.rs` is a bitmap font). Godot falls back to its built-in dummy text server. |
| `module_svg`, `jpg`, `tga`, `bmp`, `hdr`, `dds`, `ktx`, `tinyexr`, `basis_universal`, `astcenc`, `etcpak`, `bcdec`, `betsy`, `cvtt` | Textures are PNG files imported lossless; the WebP module stays because lossless `.ctex` files may be WebP inside. |
| `ogg`, `vorbis`, `mp3`, `theora`, `interactive_music` | Every sound is a WAV (`AudioStreamWAV` is core). |
| `gltf`, `fbx`, `csg`, `gridmap`, `meshoptimizer`, `xatlas_unwrap`, `vhacd`, `lightmapper_rd`, `raycast`, `noise`, `visual_shader`, `navigation_*`, `godot_physics_*`, `jolt_physics`, `openxr`, `mobile_vr`, `webxr`, `camera` | 3D, physics, XR and editor-side tooling. |
| `enet`, `multiplayer`, `webrtc`, `websocket`, `upnp`, `jsonrpc`, `mbedtls`, `regex`, `zip`, `minizip` | No networking, no TLS, no regex, no zip packs. Core's own hashing still builds its few mbedtls files. |
| `objectdb_profiler`, `mono`, `accesskit` | Debugging, C#, screen readers (the game is a picture). |

Kept: GDScript (`godot/platform_services.gd` and the XDG Portals addon's
autoload run on every platform), WebP, and everything in core and the 2D
scene tree.

Also kept, deliberately: `deprecated` (the engine's compatibility code).
SConstruct reads that option before it applies a build profile, so a profile
cannot turn it off; a build with `"deprecated": false` in the profile came out
byte for byte identical to one without it. Turning it off would take a
command-line flag; it stays on, so engine APIs Godot marks deprecated still
exist for gdext and GDScript.

That rebuild also shows the build is reproducible on one machine: the same
inputs gave the same bytes.

Godot applies a profile's options without checking their names: an option a
future Godot renames would be silently ignored. `godot_template.py build`
checks every option in the profile against the source tree it is about to
build (SConstruct, the platforms' `detect.py`, the `modules/` directory) and
refuses an unknown one. The `deprecated` case above is the other trap: an
option SConstruct reads before the profile is applied. When bumping Godot,
compare the engine library's size or hash with and without a changed option.

### gdext and missing classes

godot-rust (gdext) normally looks up every engine method of every class it
generated bindings for when the library loads, and panics on the first one the
engine does not have. With `disable_3d` that is the first 3D class.
`crates/salt-line-godot/Cargo.toml` therefore enables gdext's
`lazy-function-tables`: a method is looked up on its first call. The costs:
each engine call borrows a `RefCell` around the table and does a hash lookup
(a few hundred calls a frame; nothing measurable next to the raster); the
table is not thread-safe, so no engine call may ever be made off the main
thread; and a call into a removed class would panic only when made.

That last one is covered at startup instead. `src/engine_classes.rs` lists
every engine class the game uses, and the library checks them all with
`ClassDB.class_exists` when it loads, failing at once with the missing names.
A unit test reads every `godot::classes::` path in the crate and fails if one
names a class that is not on the list, so a new engine dependency shows up in
review, next to the code that needs it.

## Adding something back

When the game starts using an engine class or format it did not use before
(a new node type, an OGG file, a `Label`, physics, a TLS request):

1. Add the class to `REQUIRED` in `crates/salt-line-godot/src/engine_classes.rs`
   (the unit test insists, for anything imported from `godot::classes`).
2. Find what provides it: a module under `modules/` in the Godot source, or
   one of the `disable_*` switches (the class's source directory tells you:
   `scene/3d`, `scene/gui` advanced controls, `servers/physics_2d`, ...).
3. Delete that line from `tools/godot_build_profile.json` (or set it to
   `true`). The profile is canonicalised before hashing, so formatting does
   not matter, but any real change gives a new key.
4. `just godot-template` and `just android-release` to try it on a phone.
5. Publish and pin the new build (below).

A class the engine does not have stops the game at startup with
`this Godot engine build lacks classes Salt Line uses: ...` in `adb logcat`,
or, from GDScript, a parse error naming the class.

## Building it

```sh
just godot-template            # Android arm64 into build/godot-templates/
just godot-template linux      # Linux x86_64
just android-release           # release APK on it, installed over `just android`
```

`tools/godot_template.py build` clones the pinned tag into
`~/.cache/salt-line/godot-<tag>` (checking the commit it resolves to) and runs
the pinned SCons there through uv (`uvx --from scons==4.11.1 scons`). Android
needs `ANDROID_HOME`; Godot installs the NDK version it wants with
`sdkmanager` when it is missing. `package-android` refuses an engine built by
a different compiler than the official template's (it shares that
template's `libc++_shared.so`). An Android build took 3 minutes on a 16-core
machine; expect tens of minutes on a 4-core CI runner (not measured yet; the
workflow allows 90).

## How CI uses it

Building the engine on every pull request would add tens of minutes per
platform, so it is built once per set of inputs and pinned:

1. **Key.** `godot_template.py key` hashes the Godot tag and commit, the
   build profile, the published platforms' SCons flags, the pinned SCons
   version, and the source of the script's build and packaging functions.
   Any change to what would be built changes the key.
2. **Build and publish.** `.github/workflows/godot-templates.yml` runs when
   one of those inputs changes on `main`, or by hand. If no
   `godot-templates-<version>-<key>` release exists yet, it builds Android
   arm64 in a job with a read-only token. Only on `main`, a separate job (the
   only one with write, OIDC and attestation permissions, and no build code)
   attests the file's provenance and publishes it as that prerelease. It
   refuses to publish when the `ATTEST` variable is `off`, since an
   unattested build could never be pinned. A published key is never rebuilt
   or overwritten by the workflow.
3. **Try it, then pin it.** Before the lock pull request:
   - `python3 tools/godot_template.py lock --release` downloads the published
     file, runs `gh attestation verify` on it (signer
     `cascadiacollections/saltline/.github/workflows/godot-templates.yml`,
     source ref `refs/heads/main`), refuses it if that fails, and writes
     `tools/godot_templates.lock.json` (the release and the file's sha256).
   - `python3 tools/godot_template.py fetch` puts that very file in
     `build/godot-templates/`; `just android-release` packages it, exports a
     release APK and installs it on the phone.
   - On the phone: play a run, a versus match, the map editor and a replay,
     with sound, and pause and resume the app.
   - Then open the pull request with the lock file.
4. **Use.** The `android` job in `ci.yml` runs `godot_template.py fetch`: it
   downloads the pinned file and checks its sha256 against the lock, then
   puts the engine into the official template it already installed and points
   the Android preset at the result (in CI's throwaway checkout; the committed
   presets never name a custom template).

When the lock does not match the current key (the profile changed and the new
build is not pinned yet), pull requests and `main` export with the official
template and a warning; a release tag fails instead, as it does if the lock
names no Android engine. When the lock does match, a download failure or a
hash mismatch always fails the job.

**Off switch.** Setting the repository variable `STRIPPED_ENGINE` to `off`
(Settings > Secrets and variables > Actions > Variables) makes the `android`
job export on the official template everywhere, tags included, with a
warning in every run. Use it if the stripped engine turns out to break
something on players' phones and a fixed build is not ready.

The SBOM lists the stripped engine, with its hash, exactly when the lock
matches and the switch is not off, which is exactly when the Android job
uses it.

The lock file is what decides what ships. The release can be deleted or
replaced by anyone with write access, but a replaced file does not match the
pinned hash. **Owner option:** turning on GitHub's immutable releases for the
repository (Settings > General > Releases) would also stop a published
`godot-templates-*` release from being changed at all; it applies to every
release in the repository, including `dev`, which the CI deletes and
recreates on each push to `main`, so check that flow still works first.

## Bumping Godot

1. Change `GODOT_VERSION` in `.github/workflows/ci.yml` and in
   `tools/godot_template.py`, and set `GODOT_COMMIT` to the commit the new
   `-stable` tag points at (`git ls-remote https://github.com/godotengine/godot
   refs/tags/<version>-stable`). `key` fails while the two versions disagree.
2. New modules default to enabled; check the new version's `modules/` list
   and `SConstruct` options for anything worth adding to the profile. `build`
   refuses profile options the new source does not know (a renamed one would
   otherwise be ignored without a word), but cannot catch an option the new
   SConstruct reads before applying the profile (see `deprecated` above).
3. Build and test locally (`just godot-template`, `just android-release`),
   then merge; the workflow publishes the build for the new key, and a
   follow-up pull request pins it. Until then CI exports with the official
   templates, so do not tag a release in between.

## Automated engine comparison

The `engine-validation` workflow builds a native Linux template with the same
profile, exports one PCK, and runs that pack on both the official and stripped
engines. The stripped Linux binary is cached by its build inputs and used only
for testing. This does not change the Linux engine shipped to players.

`--salt-validate-engine <directory>` runs before save-file loading. It checks
the imported palette, title image, every sprite and all thirteen WAV streams,
then plays three fixed `SR` fixtures through WATCH: a run with a draft, a
generated versus match, and a match on a custom map. Each advances by exactly
1/60 second per step, with a fixed date and window size. The final state must
match reconstruction of its log; a disk-write request, divergence, missing
asset or timeout fails. The fixtures are tied to the sim/match epochs and must
be revisited deliberately when those change.

The report includes the final state hash, a hash of software-rendered frames
sampled once per simulated second and at the end, and sound-event and audio
length hashes. Samples exercise Godot's image and texture APIs; each fixture's
final frame is saved as a PNG. Both engines must produce identical reports
and PNG images. Logs and captures are retained as CI artifacts, including on failure.

After exporting a release pack with its GDExtension beside it:

```sh
python3 tools/engine_validation.py \
  --official /path/to/linux_release.x86_64 \
  --stripped /path/to/godot.linuxbsd.template_release.x86_64 \
  --pack build/engine-validation/game.pck
```

This is resource-loader, replay and software-frame parity. Headless mode uses
dummy rendering/audio drivers, so it cannot prove GPU composition, audible
sound, Android lifecycle behavior or input handling. The profiling APK's GPU
checks and device play still cover different ground.

Local verification on October 9, 2026: the official and stripped Linux ARM64
export templates matched over 19,921 replay steps and 336 sampled frames, with
identical final PNG images and all thirteen audio streams loaded. The report also
matched the Windows ARM64 editor's. An export deliberately missing `fire.wav`
failed validation and left no successful report. These are headless results,
not a phone playback or performance measurement.

## Profiling the engine that ships

`just android-profile-apk` now fetches the engine named by the release lock,
verifies its SHA-256, and exports on it. A cached library is reused only if its
hash matches. A stale or missing lock fails instead of measuring a stock
engine accidentally. `just android-profile-apk official` explicitly selects
the official engine for a baseline. The repository's CI-only
`STRIPPED_ENGINE=off` switch does not silently change this local selection.

`tools/android_profile.py` can also be run directly through `mise exec`, with
`--sdk`, `--ndk`, `--java` and `--godot` (or their usual environment variables),
including on Windows. Install the `aarch64-linux-android` target in the pinned
Rust toolchain first. It builds the host and Android extensions and restores
`export_presets.cfg` byte for byte even when exporting fails.
The export uses a temporary self-contained editor with the requested SDK and
Java paths, leaving the developer's editor settings untouched.

The profiler keeps its separate `com.cascadiacollections.saltline.profile`
package and key. After export, the script checks the engine library inside
the APK, then writes `build/SaltLine-profile.engine.json` with the selected
engine, source release and verified hash. Keep this metadata with timing
results so the measured engine is unambiguous.

Pixel 10 verification on October 9, 2026: both the official and pinned
Android engines passed GPU readback checks for run (600 frames), versus
(2,400 frames), and night with Long Lens (900 frames). Every checked frame
matched the newest CPU reference; none matched only an older buffer or
failed to match. The pinned APK's engine SHA-256 matched the release lock.
The existing profiler had a different signing key, so these runs used the
separate `com.cascadiacollections.saltline.profile.validation` package.

One 34-second ABBA capture per engine measured updated-process median/p95
CPU call times of 553.4/1,979.7 microseconds for official and 547.9/2,052.2
microseconds for pinned. These single captures do not establish a speed
improvement and do not measure deferred GPU completion. Local evidence is
in `build/phone-official-profile.log` and `build/phone-pinned-profile.log`.

## Not covered yet

- **Desktop.** The script builds a Linux x86_64 template, but the workflow
  does not publish it and the desktop exports still use the official
  templates.
  Measured locally, the Linux release template falls from 73,519,416 B to
  17,349,728 B (gzip −9: 28.3 MB to 6.6 MB), and an export on it starts and
  renders. Windows (x64, ARM64), macOS and iOS would need their own runners
  or cross toolchains; the profile itself carries over unchanged.
- **Linux glibc.** Official templates are built against glibc 2.28. A
  template built on the CI runner needs that runner's glibc (2.39 on
  `ubuntu-latest`; a Fedora 44 build needs 2.43). Shipping it would mean
  building in an older-glibc container or with Godot's buildroot SDK, as
  upstream does. That, not size, is why the Linux export is not switched yet.

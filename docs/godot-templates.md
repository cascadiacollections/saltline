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
| `objectdb_profiler`, `mono`, `accesskit`, `deprecated` | Debugging, C#, screen readers (the game is a picture), compatibility shims. |

Kept: GDScript (`godot/platform_services.gd` and the XDG Portals addon's
autoload run on every platform), WebP, and everything in core and the 2D
scene tree.

### gdext and missing classes

godot-rust (gdext) normally looks up every engine method of every class it
generated bindings for when the library loads, and panics on the first one the
engine does not have. With `disable_3d` that is the first 3D class.
`crates/salt-line-godot/Cargo.toml` therefore enables gdext's
`lazy-function-tables`: a method is looked up on its first call. The costs: a
hash lookup per engine call (a few hundred a frame; nothing measurable next to
the raster) and that a call into a removed class panics when it is made rather
than at startup. That is why adding engine API to the game needs the check
below.

## Adding something back

When the game starts using an engine class or format it did not use before
(a new node type, an OGG file, a `Label`, physics, a TLS request):

1. Find what provides it: a module under `modules/` in the Godot source, or
   one of the `disable_*` switches (the class's source directory tells you:
   `scene/3d`, `scene/gui` advanced controls, `servers/physics_2d`, ...).
2. Delete that line from `tools/godot_build_profile.json` (or set it to
   `true` for a module). The profile is canonicalised before hashing, so
   formatting does not matter, but any real change gives a new key.
3. `just godot-template` and `just android-release` to try it on a phone.
4. Publish and pin the new build (below).

A class the engine does not have shows up as a gdext panic (`Failed to load
class method ...`) in `adb logcat` the moment the code path runs, or, from
GDScript, a parse error naming the class.

## Building it

```sh
just godot-template            # Android arm64 into build/godot-templates/
just godot-template linux      # Linux x86_64
just android-release           # release APK on it, installed over `just android`
```

`tools/godot_template.py build` clones the pinned tag into
`~/.cache/salt-line/godot-<tag>` (checking the commit it resolves to) and runs
SCons there (`uvx scons` if `scons` is not on PATH). Android needs
`ANDROID_HOME`; Godot installs the NDK version it wants with `sdkmanager`
when it is missing. On a 16-core machine an Android build takes about 3
minutes; a 4-core CI runner takes far longer.

## How CI uses it

Building the engine on every pull request would add tens of minutes per
platform, so it is built once per set of inputs and pinned:

1. **Key.** `godot_template.py key` hashes the Godot tag and commit, the
   build profile, the published platforms' SCons flags and a recipe version. Any change to what
   would be built changes the key.
2. **Build and publish.** `.github/workflows/godot-templates.yml` runs when
   one of those inputs changes on `main`, or by hand. If no
   `godot-templates-<version>-<key>` release exists yet, it builds Android
   arm64 in a job with read-only tokens, then a separate job
   (the only one with write, OIDC and attestation permissions, and no build
   code) attests the files' provenance and publishes them as that prerelease.
   A published key is never rebuilt or overwritten.
3. **Pin.** `python3 tools/godot_template.py lock --release` writes
   `tools/godot_templates.lock.json`: the release and the sha256 of each file.
   Check the attestation first:
   `gh attestation verify --repo cascadiacollections/saltline <file>`.
   The lock change goes through a pull request like any other.
4. **Use.** The `android` job in `ci.yml` runs `godot_template.py fetch`: it
   downloads the pinned files and checks their sha256 against the lock, then
   puts the engine into the official template it already installed and points
   the Android preset at the result (in CI's throwaway checkout; the committed
   presets never name a custom template).

When the lock does not match the current key (the profile changed and the new
build is not pinned yet), pull requests and `main` export with the official
template and a warning; a release tag fails instead. When the lock does match,
a download failure or a hash mismatch always fails the job. The SBOM lists the
stripped engine, with its hash, exactly when the lock matches, which is exactly
when the Android job uses it.

The lock file is what decides what ships. The release can be deleted or
replaced by anyone with write access, but a replaced file does not match the
pinned hash.

## Bumping Godot

1. Change `GODOT_VERSION` in `.github/workflows/ci.yml` and in
   `tools/godot_template.py`, and set `GODOT_COMMIT` to the commit the new
   `-stable` tag points at (`git ls-remote https://github.com/godotengine/godot
   refs/tags/<version>-stable`). `key` fails while the two versions disagree.
2. New modules default to enabled; check the new version's `modules/` list
   and `SConstruct` options for anything worth adding to the profile.
3. Build and test locally (`just godot-template`, `just android-release`),
   then merge; the workflow publishes the build for the new key, and a
   follow-up pull request pins it. Until then CI exports with the official
   templates, so do not tag a release in between.

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
- **The profiling APK** (`just android-profile`) still exports on the
  official template.

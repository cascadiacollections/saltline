# Profiling on iOS

The low-end iOS floor is the **iPhone SE (2nd generation)** (A2275): A13 Bionic, 3 GB RAM, a 4.7" 1334 × 750 @2x display with a home button and no notch, on iOS 27. If a build holds frame time there, it holds on every iPhone the game supports. The Android equivalent is the Galaxy A16 (`tools/android_profile.py`).

Held landscape the SE is exactly 16:9. That gives the narrowest buffer, 320 × 180, with no cutout and no letterbox. `render::tests::an_iphone_se_lays_out_at_320_with_no_insets_and_every_button_on_screen` checks that every touch button fits.

## One-time device setup

1. **Register the device.** Plug the SE into the Mac, open Xcode, and trust the computer on the phone. `just ios` builds with `-allowProvisioningUpdates`, so Xcode registers the UDID with team `9L45FNA7XM` on the first signed build. To register it by hand instead: Xcode > Window > Devices and Simulators gives you the identifier, then add it under Certificates, Identifiers & Profiles > Devices on developer.apple.com.
2. **Turn on Developer Mode** on the phone: Settings > Privacy & Security > Developer Mode, then restart. The option only appears once Xcode has seen the device.
3. **Name the phone for the recipes:** `export IOS_DEVICE="iPhone SE"`, or use its UDID from `xcrun devicectl list devices`.

You also need Godot 4.7.2 with its export templates, Xcode signed in to the team, and rustup (see `docs/testflight.md`).

## Builds

| Build | Command | Use it for |
| --- | --- | --- |
| Debug | `just ios` | The perf overlay, Instruments with symbols. Rust is unoptimised (opt-level 1), so don't trust its frame times. |
| Release | `just ios release` | Benchmarks and Game Performance traces. No overlay. |

Both export through `ios-project`. The iOS preset is arm64-only, iPhone and iPad, with `min_ios_version` 16.0. The Rust library is built for `aarch64-apple-ios` with the same deployment target (`ios-lib`, `tools/testflight.py`). `salt_line.gdextension` maps `ios.debug` and `ios.release` to `target/aarch64-apple-ios/{debug,release}`.

## Renderer

iOS runs the **Mobile** renderer (Metal), from `rendering/renderer/rendering_method.ios` in `project.godot`. Android and desktop keep **Compatibility**. If Metal fails to start, `rendering_device/fallback_to_opengl3` drops to Compatibility automatically.

To try Compatibility without rebuilding:

```sh
just ios-launch --rendering-method gl_compatibility
```

To ship it, set the `.ios` line to `"gl_compatibility"`.

Compatibility used about 190 MiB less memory on desktop (`docs/rendering-footprint.md`), and the SE has only 3 GB. Run the benchmark on both renderers before deciding.

## Perf overlay (debug builds)

A debug build shows the overlay from launch on a phone, below the windsock plate:

```text
59.8 FPS  16.72 ms  p95 17.40  p99 21.02
DRAW 25  STATIC 45.7M  VRAM 34.4M
THERMAL NOMINAL  HEADROOM 1210M
```

- **Frame time:** wall clock between frames. Mean, p95 and p99 cover the last 240 frames.
- **DRAW, STATIC, VRAM:** Godot's `Performance` monitors (`RENDER_TOTAL_DRAW_CALLS_IN_FRAME`, `MEMORY_STATIC`, `RENDER_VIDEO_MEM_USED`). They describe the frame the renderer finished last.
- **THERMAL:** `NSProcessInfo.thermalState`.
- **HEADROOM:** `os_proc_available_memory()`, how much more the process can allocate before jetsam kills it. The simulator has no limit, so the line is hidden there.

The thermal and headroom values come from `perf::device`, a small shim that calls the Objective-C runtime directly. There is no plugin to export. On a desktop the overlay follows F1; `--salt-perf` shows it anywhere. Release builds compile none of this.

## Soak benchmark

```sh
just ios release            # optimised build, installed
just ios-bench              # 300 s on the Mobile renderer
just ios-bench 300 gl_compatibility
just ios-bench-pull         # CSVs into build/bench/
```

`--salt-bench` is deterministic in what it plays:

- The autopilot plays seed `20261004`, the same seed as the Android profiling APK.
- It aims with the solver and takes the first draft offer.
- When a run ends it starts the same seed again.
- The camera follows the shots, so the camera path is the same every time.

Wall-clock pacing, and so the exact frame on which each event lands, still varies between devices and thermal states. That variation is what the benchmark measures.

Every frame is kept in memory and written once, when the benchmark ends. The file is `Documents/bench/bench-<unix time>-done.csv`, or `-paused` / `-closed` if the app is backgrounded or quit early. The app then exits.

The log line `salt-bench: ...` summarises the run: mean, p95 and p99 frame time after a 2 s warm-up, the worst thermal state, and the lowest headroom. The CSV columns are:

```text
frame,t_s,frame_ms,process_ms,rendered,draw_calls,static_mib,video_mib,thermal,available_mib
```

`process_ms` is the game node's own `process` (input, sim, raster, upload). `rendered` is 0 on frames the dirty-flag skipped (`frame::Redraw`).

For a soak, keep the phone unplugged, at fixed brightness and not in a case, and run the benchmark three times back to back. Compare p99 and thermal state across the three runs, not just the mean. `DevicePower` lowers the frame cap under thermal pressure, so a rising thermal state together with a falling frame rate is the game behaving as designed, not a regression.

## Instruments

Use a **release** build for timing and a **debug** build when you need Rust symbols. The release dylib currently ships without a dSYM (see `docs/testflight.md`).

1. Run `just ios release`, then open Instruments and choose the SE as the target. Attach to *SaltLine*, or have Instruments launch it.
2. **Game Performance** template: frame pacing (look for hitches over 16.7 ms), thermal state, and CPU on the main thread versus Godot's render thread. Start a benchmark from Xcode's scheme instead to get a repeatable trace. Add `--salt-bench --salt-bench-seconds 120` under Arguments Passed On Launch.
3. **Metal System Trace** template: GPU time per frame, encoder and command-buffer timing, and drawable waits. The game draws a handful of textured quads a frame (the whole frame, changed strips, sky and ground), so GPU time should be small. If frames are long, look first at upload (`ImageTexture::update`, the strip pass) and the Rust raster.
4. Record 30–60 s past the warm-up, and mark where the first shot lands so traces line up between builds.

Xcode's own debug gauges (Debug Navigator > FPS, Memory, Energy) are useful for a quick look. They add overhead, though, so don't compare their numbers with release benchmarks.

## What needs the device

These need the physical SE: registering it, signing, Developer Mode, Instruments traces, real thermal states, and memory headroom. In CI the iOS job only runs the determinism check on the simulator, because device runs need signing.

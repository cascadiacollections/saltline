default:
    @just --list

test:
    cargo test

build:
    cargo build -p salt-line-godot

build-release:
    cargo build --release -p salt-line-godot

run: build
    godot --path .

# Play as the free shareware edition (crates/salt-line-godot/src/tier.rs): runs end after 12 screens; no challenges or editor.
run-shareware: build
    godot --path . -- --salt-shareware

# Export the shareware edition: "Linux", "Windows Desktop", "macOS" or "Android" (a debug export, as android-apk). Writes build/shareware/.
export-shareware platform="Linux":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p build/shareware
    case "{{ platform }}" in
        Linux) out=SaltLine-shareware.x86_64 ;;
        "Windows Desktop") out=SaltLine-shareware.exe ;;
        macOS) out=SaltLine-shareware.zip ;;
        Android)
            just android-lib
            bash tools/godot_headless.sh --frame-delay 800 --import
            bash tools/godot_headless.sh --export-debug "Android Shareware" build/shareware/SaltLine-shareware.apk
            test -e build/shareware/SaltLine-shareware.apk || { echo "error: export wrote nothing" >&2; exit 1; }
            echo build/shareware/SaltLine-shareware.apk
            exit 0 ;;
        *) echo "error: no shareware preset for '{{ platform }}'" >&2; exit 2 ;;
    esac
    cargo build --release -p salt-line-godot
    bash tools/godot_headless.sh --import
    bash tools/godot_headless.sh --export-release "{{ platform }} Shareware" "build/shareware/$out"
    # --export-release has been known to exit 0 having written nothing.
    test -e "build/shareware/$out" || { echo "error: export wrote nothing" >&2; exit 1; }
    echo "build/shareware/$out"

gen-art: export-sprites
    uv run python tools/gen_sprites.py

# Export art/aseprite/*.aseprite to art/sprites/*.png (needs the aseprite CLI; set ASEPRITE to override).
export-sprites:
    uv run python tools/export_sprites.py

# Fail if art/sprites is stale vs art/aseprite or breaks the palette rules.
check-sprites:
    uv run python tools/export_sprites.py --check

gen-audio:
    uv run python tools/gen_audio.py

site:
    uv run python tools/build_site.py

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p salt-line-godot --all-targets --features profiling -- -D warnings

# Deterministic shell-economy and solvability report. Skill is perfect, steady or rough;
# aim is flat (same hit chance every shot) or scaled (harder shots miss more;
# no effect on perfect, which never misses).
# Always passes --fast, so the candidate histogram saturates at the gate cap.
balance seeds="100" screens="40" skill="perfect" build="base" aim="flat":
    cargo run --release -p salt-line-sim --bin batch -- --seeds {{ seeds }} --screens {{ screens }} --skill {{ skill }} --build {{ build }} --aim {{ aim }} --fast

# Sweep the hit chance (10-100) and print one survival/reserve line per step,
# to find where the shell economy stops carrying a player. Same seeds at every
# step. Aim is flat or scaled. Stops with batch's report if a screen fails to validate.
balance-sweep seeds="100" screens="40" build="base" from="40" to="100" step="5" aim="flat":
    #!/usr/bin/env bash
    set -euo pipefail
    printf '%5s  %-10s %-9s %s\n' "hit%" "exhausted" "mean run" "ending reserve (mean, min)"
    for hit in $(seq {{ from }} {{ step }} {{ to }}); do
        if ! out=$(cargo run --release -q -p salt-line-sim --bin batch -- --seeds {{ seeds }} --screens {{ screens }} --hit "$hit" --build {{ build }} --aim {{ aim }} --fast); then
            printf '%s\n' "$out"
            exit 1
        fi
        deaths=$(sed -n 's/^death\/run length: \([0-9]*\/[0-9]*\) runs exhausted; mean run \([0-9.]*\/[0-9]*\).*/\1 \2/p' <<<"$out")
        ending=$(sed -n 's/.*mean ending reserve \([0-9.]*\), min \(-\{0,1\}[0-9]*\).*/\1, \2/p' <<<"$out")
        if [[ -z "$deaths" || -z "$ending" ]]; then
            echo "balance-sweep: batch output format changed; update the sed patterns" >&2
            exit 1
        fi
        read -r exhausted run <<<"$deaths"
        printf '%5s  %-10s %-9s %s\n' "$hit" "$exhausted" "$run" "$ending"
    done

# Two bots play local versus matches; the report gives shots per hit, turns per round,
# how often a repeated hit lands and how often the opener wins. Bot is kid (aim help)
# or steady, or the game's AI opponent as ai-easy, ai-medium or ai-hard (DESIGN §11.7);
# flags set one lever each (see examples/duel.rs), e.g. `just duel 400 kid --right steady`,
# `just duel 400 kid --right ai-easy` or `just duel 200 steady --no-equal-shots --shift 0`.
# Deterministic local-versus rules report (duel harness).
duel matches="200" bot="kid" *flags:
    cargo run --release -p salt-line-godot --example duel -- --matches {{ matches }} --bot {{ bot }} {{ flags }}

# Layouts (DESIGN §13): fork generated run screens and duel rounds, and report share-code
# lengths, how closely forks play, and what the checks cost. Or one at a time:
# `just layouts fork-run DUNE:8`, `just layouts fork-duel 42:0`, `just layouts check SL…`.
layouts *args:
    cargo run --release -p salt-line-godot --example layouts -- {{ args }}

# macOS exports ship one universal dylib: `macos.release` in salt_line.gdextension points at
# target/universal-apple-darwin/release. Debug builds stay host-only at target/debug, so
# `just run` does not need this; run it before `godot --export-release "macOS"` on a Mac.

# Build the universal (arm64 + x86_64) macOS release library.
macos-universal:
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build --release -p salt-line-godot --target aarch64-apple-darwin
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build --release -p salt-line-godot --target x86_64-apple-darwin
    mkdir -p target/universal-apple-darwin/release
    lipo -create -output target/universal-apple-darwin/release/libsalt_line_godot.dylib \
        target/aarch64-apple-darwin/release/libsalt_line_godot.dylib \
        target/x86_64-apple-darwin/release/libsalt_line_godot.dylib
    lipo -info target/universal-apple-darwin/release/libsalt_line_godot.dylib

# Check the version is consistent everywhere it is written down (RELEASING.md, "Version rule").
version-check:
    python3 tools/release_version.py check

# Android (arm64-v8a). Needs rustup, the Android SDK + NDK (ANDROID_HOME, ANDROID_NDK_HOME;
# defaults to ~/Android/Sdk and its newest NDK), Godot's export templates and a debug keystore
# in the editor settings. Set ANDROID_SERIAL to pick a device when more than one is attached.
android_sdk := env("ANDROID_HOME", home_directory() / "Android/Sdk")
android_ndk := env("ANDROID_NDK_HOME", `ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1`)
android_host := if os() == "macos" { "darwin-x86_64" } else { "linux-x86_64" }
android_bin := android_ndk / "toolchains/llvm/prebuilt" / android_host / "bin"
adb := android_sdk / "platform-tools/adb"

# rustup's cargo and rustc are called explicitly: a system Rust earlier on PATH has no Android std.

# Cross-compile the GDExtension for Android and strip its debug info.
android-lib profile="debug":
    rustup target add aarch64-linux-android
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build -p salt-line-godot \
        --target aarch64-linux-android {{ if profile == "release" { "--release" } else { "" } }} \
        --config 'target.aarch64-linux-android.linker="{{ android_bin }}/aarch64-linux-android24-clang"'
    "{{ android_bin }}/llvm-strip" --strip-debug target/aarch64-linux-android/{{ profile }}/libsalt_line_godot.so

# Run the determinism check (DESIGN §6) on the attached device as arm64-v8a, the ABI the APK
# ships, and fail unless its digest matches this machine's. CI can only run Android on an
# x86_64 emulator, so this is how a phone's own bionic and CPU join the comparison. It runs
# a plain binary from /data/local/tmp over adb shell; nothing is installed.
android-digest:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add aarch64-linux-android
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build --release -q -p salt-line-sim --bin determinism \
        --target aarch64-linux-android \
        --config 'target.aarch64-linux-android.linker="{{ android_bin }}/aarch64-linux-android24-clang"'
    host=$(RUSTC="$(rustup which rustc)" "$(rustup which cargo)" run --release -q -p salt-line-sim --bin determinism \
        | grep -oE 'digest: 0x[0-9a-f]+' | cut -d' ' -f2)
    echo "host ($(uname -sm)) digest: $host"
    ADB="{{ adb }}" bash tools/android_digest.sh target/aarch64-linux-android/release/determinism --expect "$host"

# Build the debug APK at build/SaltLine.apk.
android-apk: build android-lib
    mkdir -p build
    bash tools/godot_headless.sh --frame-delay 800 --import
    bash tools/godot_headless.sh --export-debug "Android" build/SaltLine.apk

# Build, install and launch on the attached device or emulator.
android: android-apk
    # -d: CI builds carry a far higher versionCode than this local build's 1, and a debug APK may downgrade.
    "{{ adb }}" install -r -d build/SaltLine.apk
    "{{ adb }}" shell am force-stop com.cascadiacollections.saltline
    "{{ adb }}" shell monkey -p com.cascadiacollections.saltline -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Build Salt Line's stripped Godot engine from source (docs/godot-templates.md) into
# build/godot-templates: `android` (arm64) or `linux` (x86_64). Needs uv (it runs the pinned
# SCons) and, for Android, the SDK. 3 minutes on 16 cores; tens on a small machine.
godot-template platform="android":
    python3 tools/godot_template.py build {{ platform }}

# Opt-in Godot 4.8-beta2 trial (docs/godot-4.8-trial.md). Does not change the 4.7.2 pin.
# Downloads the official editor and export templates, exports Linux, and writes
# build/godot-4.8-trial/report.md. An Android APK is included when the SDK is set.
godot-4.8-trial:
    python3 tools/godot_4_8_trial.py run

# Stripped 4.8-beta2 template. Refuses Python older than 3.10. Does not publish
# or rewrite tools/godot_templates.lock.json.
godot-4.8-trial-engine platform="android":
    python3 tools/godot_4_8_trial.py build {{ platform }}

# Release APK at build/SaltLine-release.apk on the stripped engine (run `just godot-template`
# first), signed with the editor's debug key so it installs over `just android` and keeps the
# saves. Set GODOT_ANDROID_KEYSTORE_RELEASE_PATH/_USER/_PASSWORD to sign with another key.
android-release: build (android-lib "release")
    #!/usr/bin/env bash
    set -euo pipefail
    engine=build/godot-templates/libgodot.android.template_release.arm64.so
    [ -f "$engine" ] || { echo "error: no $engine; run 'just godot-template' first" >&2; exit 1; }
    python3 tools/godot_template.py package-android "$engine"
    backup=$(mktemp)
    cp export_presets.cfg "$backup"
    trap 'cp "$backup" export_presets.cfg; rm -f "$backup"' EXIT
    python3 tools/godot_template.py use Android "$PWD/build/godot-templates/android_release.apk"
    bash tools/godot_headless.sh --frame-delay 800 --import
    export GODOT_ANDROID_KEYSTORE_RELEASE_PATH="${GODOT_ANDROID_KEYSTORE_RELEASE_PATH:-${XDG_DATA_HOME:-$HOME/.local/share}/godot/keystores/debug.keystore}"
    export GODOT_ANDROID_KEYSTORE_RELEASE_USER="${GODOT_ANDROID_KEYSTORE_RELEASE_USER:-androiddebugkey}"
    export GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD="${GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD:-android}"
    bash tools/godot_headless.sh --export-release "Android" build/SaltLine-release.apk
    ls -l build/SaltLine-release.apk
    "{{ adb }}" install -r build/SaltLine-release.apk
    "{{ adb }}" shell am force-stop com.cascadiacollections.saltline
    "{{ adb }}" shell monkey -p com.cascadiacollections.saltline -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Optimized, fixed-seed profiling APK. This separate feature build auto-plays
# one deterministic run, warms up for 2 seconds, then captures 32 seconds of
# A-B-B-A texture-update-enabled/frozen phases plus a final timing summary.
android-profile-apk engine="pinned":
    python3 tools/android_profile.py --engine {{ engine }}

# Build, install and launch the profiling APK on the attached device.
android-profile: android-profile-apk
    "{{ adb }}" install -r -d build/SaltLine-profile.apk
    "{{ adb }}" shell am force-stop com.cascadiacollections.saltline.profile
    "{{ adb }}" shell monkey -p com.cascadiacollections.saltline.profile -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Run the profiling build on the attached device to the end and print its
# timings (and, with --gpu-check, each GPU check scene's verdict). Pass an APK
# to install it first, e.g. `just android-profile-run build/SaltLine-profile.apk --gpu-check`.
android-profile-run *args:
    python3 tools/android_profile_run.py --adb "{{ adb }}" {{ args }}

# Follow the profiling build's timing output on the attached device.
android-profile-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof com.cascadiacollections.saltline.profile)"

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof com.cascadiacollections.saltline)"

# iOS (arm64). macOS only. Needs Xcode signed in to your Apple developer account
# (Settings > Accounts), rustup, Godot's export templates, and your Team ID in the iOS preset
# (application/app_store_team_id in export_presets.cfg). Godot exports an Xcode project,
# xcodebuild signs it with automatic provisioning (which also registers the phone), and
# devicectl installs it. IOS_TEAM overrides the preset's Team ID. Set IOS_DEVICE to a
# device name or UDID when more than one physical iPhone is attached.

# The iOS preset's application/min_ios_version; the library is built for the same floor.
ios_min := "16.0"

# Build the GDExtension for iOS.
ios-lib profile="debug":
    rustup target add aarch64-apple-ios
    IPHONEOS_DEPLOYMENT_TARGET={{ ios_min }} RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build -p salt-line-godot \
        --target aarch64-apple-ios {{ if profile == "release" { "--release" } else { "" } }}

# Builds the host extension too: Godot loads it while importing the project on a clean checkout.
# The default debug build is opt-level 1, for development; `release` uses the LTO Rust library
# and Godot's release template.

# Export the Xcode project to build/ios (open build/ios/SaltLine.xcodeproj to run from Xcode).
ios-project profile="debug": build (ios-lib profile)
    #!/usr/bin/env bash
    set -euo pipefail
    team="${IOS_TEAM:-$(sed -n 's/^application\/app_store_team_id="\(.*\)"$/\1/p' export_presets.cfg)}"
    if [ -z "$team" ]; then
        echo "error: set application/app_store_team_id in the iOS preset of export_presets.cfg" >&2
        echo "       or set IOS_TEAM (your 10-character Team ID)" >&2
        exit 1
    fi
    rm -rf build/ios && mkdir -p build/ios
    bash tools/godot_headless.sh --frame-delay 800 --import
    bash tools/godot_headless.sh --export-{{ profile }} "iOS" build/ios/SaltLine.ipa
    test -d build/ios/SaltLine.xcodeproj
    # Godot 4.7.2 emits empty usage strings even though Salt Line does not use
    # the camera, microphone or photo library. Empty strings make Xcode warn.
    info=build/ios/SaltLine/SaltLine-Info.plist
    for key in NSCameraUsageDescription NSMicrophoneUsageDescription NSPhotoLibraryUsageDescription; do
        plutil -remove "$key" "$info" 2>/dev/null || true
    done
    # The template also compiles dummy.h as a source file; this directive then warns.
    sed -i '' '/^#pragma once$/d' build/ios/SaltLine/dummy.h
    # Godot skips a missing extension library without a word, and the app then starts
    # with no game in it, so check it was wrapped into a framework.
    if ! find build/ios -name 'libsalt_line_godot.framework' | grep -q .; then
        echo "error: the export has no libsalt_line_godot.framework; is target/aarch64-apple-ios built?" >&2
        exit 1
    fi

# Sign, install and launch on the attached iPhone. `just ios release` for the optimised build.
ios profile="debug": (ios-project profile)
    #!/usr/bin/env bash
    set -euo pipefail
    team="${IOS_TEAM:-$(sed -n 's/^application\/app_store_team_id="\(.*\)"$/\1/p' export_presets.cfg)}"
    # Documents may be backed by a macOS file provider, which adds FinderInfo to copied
    # frameworks and makes codesign reject them. Stage the generated project outside it.
    stage=$(mktemp -d "${TMPDIR:-/tmp}/saltline-ios.XXXXXX")
    trap 'rm -rf "$stage"' EXIT
    mkdir -p "$stage/project" "$stage/out"
    rsync -a --exclude out build/ios/ "$stage/project/"
    xattr -cr "$stage"
    xcodebuild -project "$stage/project/SaltLine.xcodeproj" -configuration {{ if profile == "release" { "Release" } else { "Debug" } }} -sdk iphoneos \
        -allowProvisioningUpdates DEVELOPMENT_TEAM="$team" CODE_SIGN_STYLE=Automatic \
        COMPILER_INDEX_STORE_ENABLE=NO SYMROOT="$stage/out" -quiet build
    app=$(ls -d "$stage"/out/{{ if profile == "release" { "Release" } else { "Debug" } }}-iphoneos/*.app | head -1)
    device="${IOS_DEVICE:-}"
    if [ -z "$device" ]; then
        list=$(mktemp)
        xcrun devicectl list devices --json-output "$list" >/dev/null
        device=$(python3 -c '
    import json, sys
    devices = json.load(open(sys.argv[1]))["result"]["devices"]
    phones = [d for d in devices if
        d.get("properties", {}).get("hardware", {}).get("deviceType") == "iPhone"
        and d.get("properties", {}).get("hardware", {}).get("reality") == "physical"
        and d.get("properties", {}).get("connection", {}).get("state") == "connected"]
    if len(phones) != 1:
        names = ", ".join(d.get("properties", {}).get("state", {}).get("name", d["identifier"]) for d in phones)
        suffix = f": {names}" if names else "; plug one in or set IOS_DEVICE"
        sys.exit(f"error: expected one connected physical iPhone, found {len(phones)}{suffix}")
    print(phones[0]["identifier"])
    ' "$list")
        rm -f "$list"
    fi
    xcrun devicectl device install app --device "$device" "$app"
    xcrun devicectl device process launch --device "$device" com.cascadiacollections.saltline

# IOS_DEVICE picks the phone (a name or UDID; `xcrun devicectl list devices`). E.g.
# `just ios-launch --salt-perf` or `just ios-launch --rendering-method gl_compatibility`.
# Launch the installed app on the iPhone with developer flags, streaming its log.
ios-launch *args:
    xcrun devicectl device process launch --console --terminate-existing \
        --device "${IOS_DEVICE:?set IOS_DEVICE to the iPhone name or UDID}" \
        com.cascadiacollections.saltline {{ args }}

# renderer is mobile or gl_compatibility; the CSV lands in the app's Documents/bench.
# Soak benchmark on the iPhone (docs/profiling-ios.md): `just ios release` first, then this.
ios-bench seconds="300" renderer="mobile":
    just ios-launch --rendering-method {{ renderer }} --salt-bench --salt-bench-seconds {{ seconds }}

# Copy the benchmark CSVs off the iPhone into build/bench.
ios-bench-pull:
    mkdir -p build/bench
    xcrun devicectl device copy from --device "${IOS_DEVICE:?set IOS_DEVICE to the iPhone name or UDID}" \
        --domain-type appDataContainer --domain-identifier com.cascadiacollections.saltline \
        --source Documents/bench --destination build/bench

# Check advisories, licences, bans and sources (deny.toml). Needs cargo-deny.
deny:
    cargo deny check

# Spelling, workflow security and unused dependencies (typos, zizmor, cargo-machete), as CI runs them.
hygiene:
    typos
    zizmor .
    cargo machete

# Line coverage summary (cargo-llvm-cov + cargo-nextest, llvm-tools-preview).
cov:
    cargo llvm-cov nextest --workspace --summary-only --ignore-filename-regex '/bin/'

# Run the pure modules under Miri, which fails on undefined behaviour (nightly + miri component).
# The GDExtension entry point is omitted under Miri; see salt-line-godot's lib.rs.
miri:
    PROPTEST_CASES=4 MIRIFLAGS=-Zmiri-disable-isolation rustup run nightly cargo miri test -p salt-line-sim --lib -- dmath:: rng::
    PROPTEST_CASES=4 MIRIFLAGS=-Zmiri-disable-isolation rustup run nightly cargo miri test -p salt-line-godot --lib -- save_format:: touch:: aim:: icons::

# Benchmarks: screen generation, the solvability proof, a shot, a replay (divan).
bench:
    cargo bench -p salt-line-sim --bench sim

# Fuzz one target for a while (nightly + cargo-fuzz): save_parse, seed_parse, replay, match_log
# or layout_code.
fuzz target="save_parse" seconds="60":
    cd fuzz && rustup run nightly cargo fuzz run {{ target }} -- -max_total_time={{ seconds }}

# Mutation testing for one file (cargo-mutants); the weekly workflow runs the lot.
mutants file:
    cargo mutants -f {{ file }}

# Signed archive; add --upload to distribute to internal TestFlight testers.
testflight *args:
    python3 tools/testflight.py {{ args }}

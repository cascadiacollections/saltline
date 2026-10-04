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

gen-art:
    uv run python tools/gen_sprites.py

gen-audio:
    uv run python tools/gen_audio.py

site:
    uv run python tools/build_site.py

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

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

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof com.cascadiacollections.saltline)"

# iOS (arm64). macOS only. Needs Xcode signed in to your Apple developer account
# (Settings > Accounts), rustup, Godot's export templates, and your Team ID in the iOS preset
# (application/app_store_team_id in export_presets.cfg). Godot exports an Xcode project,
# xcodebuild signs it with automatic provisioning (which also registers the phone), and
# devicectl installs it. IOS_TEAM overrides the preset's Team ID. Set IOS_DEVICE to a
# device name or UDID when more than one physical iPhone is attached.

# Build the GDExtension for iOS.
ios-lib profile="debug":
    rustup target add aarch64-apple-ios
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build -p salt-line-godot \
        --target aarch64-apple-ios {{ if profile == "release" { "--release" } else { "" } }}

# Export the Xcode project to build/ios (open build/ios/SaltLine.xcodeproj to run from Xcode).
# Build the host extension too: Godot loads it while importing the project on a clean checkout.
ios-project: build ios-lib
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
    bash tools/godot_headless.sh --export-debug "iOS" build/ios/SaltLine.ipa
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

# Sign, install and launch on the attached iPhone.
ios: ios-project
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
    xcodebuild -project "$stage/project/SaltLine.xcodeproj" -configuration Debug -sdk iphoneos \
        -allowProvisioningUpdates DEVELOPMENT_TEAM="$team" CODE_SIGN_STYLE=Automatic \
        COMPILER_INDEX_STORE_ENABLE=NO SYMROOT="$stage/out" -quiet build
    app=$(ls -d "$stage"/out/Debug-iphoneos/*.app | head -1)
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
miri:
    PROPTEST_CASES=4 MIRIFLAGS=-Zmiri-disable-isolation rustup run nightly cargo miri test -p salt-line-sim --lib -- dmath:: rng::
    PROPTEST_CASES=4 MIRIFLAGS=-Zmiri-disable-isolation rustup run nightly cargo miri test -p salt-line-godot --lib -- save_format:: touch:: aim:: icons::

# Benchmarks: screen generation, the solvability proof, a shot, a replay (divan).
bench:
    cargo bench -p salt-line-sim --bench sim

# Fuzz one target for a while (nightly + cargo-fuzz): save_parse, seed_parse or replay.
fuzz target="save_parse" seconds="60":
    cd fuzz && rustup run nightly cargo fuzz run {{ target }} -- -max_total_time={{ seconds }}

# Mutation testing for one file (cargo-mutants); the weekly workflow runs the lot.
mutants file:
    cargo mutants -f {{ file }}

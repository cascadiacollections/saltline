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
android-apk: android-lib
    mkdir -p build
    godot --headless --path . --frame-delay 800 --import
    godot --headless --path . --export-debug "Android" build/SaltLine.apk

# Build, install and launch on the attached device or emulator.
android: android-apk
    # -d: CI builds carry a far higher versionCode than this local build's 1, and a debug APK may downgrade.
    "{{ adb }}" install -r -d build/SaltLine.apk
    "{{ adb }}" shell am force-stop com.cascadiacollections.saltline
    "{{ adb }}" shell monkey -p com.cascadiacollections.saltline -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof com.cascadiacollections.saltline)"

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

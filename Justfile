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
    "{{ adb }}" install -r build/SaltLine.apk
    "{{ adb }}" shell am force-stop com.cascadiacollections.saltline
    "{{ adb }}" shell monkey -p com.cascadiacollections.saltline -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof com.cascadiacollections.saltline)"

# iOS (arm64). macOS only. Needs Xcode signed in to your Apple developer account
# (Settings > Accounts), rustup, Godot's export templates, and your Team ID in the iOS preset
# (application/app_store_team_id in export_presets.cfg). Godot exports an Xcode project,
# xcodebuild signs it with automatic provisioning (which also registers the phone), and
# devicectl installs it. Set IOS_DEVICE to a device name or UDID when more than one is attached.

# Build the GDExtension for iOS.
ios-lib profile="debug":
    rustup target add aarch64-apple-ios
    RUSTC="$(rustup which rustc)" "$(rustup which cargo)" build -p salt-line-godot \
        --target aarch64-apple-ios {{ if profile == "release" { "--release" } else { "" } }}

# Export the Xcode project to build/ios (open build/ios/SaltLine.xcodeproj to run from Xcode).
ios-project: ios-lib
    #!/usr/bin/env bash
    set -euo pipefail
    team=$(sed -n 's/^application\/app_store_team_id="\(.*\)"$/\1/p' export_presets.cfg)
    if [ -z "$team" ]; then
        echo "error: set application/app_store_team_id in the iOS preset of export_presets.cfg" >&2
        echo "       (your 10-character Team ID, from developer.apple.com/account > Membership)" >&2
        exit 1
    fi
    rm -rf build/ios && mkdir -p build/ios
    godot --headless --path . --frame-delay 800 --import
    godot --headless --path . --export-debug "iOS" build/ios/SaltLine.ipa
    test -d build/ios/SaltLine.xcodeproj
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
    team=$(sed -n 's/^application\/app_store_team_id="\(.*\)"$/\1/p' export_presets.cfg)
    xcodebuild -project build/ios/SaltLine.xcodeproj -configuration Debug -sdk iphoneos \
        -allowProvisioningUpdates DEVELOPMENT_TEAM="$team" CODE_SIGN_STYLE=Automatic \
        SYMROOT="$PWD/build/ios/out" -quiet build
    app=$(ls -d build/ios/out/Debug-iphoneos/*.app | head -1)
    device="${IOS_DEVICE:-}"
    if [ -z "$device" ]; then
        list=$(mktemp)
        xcrun devicectl list devices --json-output "$list" >/dev/null
        device=$(python3 -c 'import json,sys; d=[x for x in json.load(open(sys.argv[1]))["result"]["devices"] if x.get("hardwareProperties",{}).get("platform")=="iOS"]; print(d[0]["identifier"] if d else "")' "$list")
        rm -f "$list"
    fi
    if [ -z "$device" ]; then echo "error: no iPhone found; plug it in and trust this Mac" >&2; exit 1; fi
    xcrun devicectl device install app --device "$device" "$app"
    xcrun devicectl device process launch --device "$device" com.cascadiacollections.saltline

# Check advisories, licences, bans and sources (deny.toml). Needs cargo-deny.
deny:
    cargo deny check

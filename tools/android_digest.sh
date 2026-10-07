#!/usr/bin/env bash
set -euo pipefail

# Run a cross-compiled determinism binary on an Android device or emulator over
# adb and print its output, for example
#   bash tools/android_digest.sh target/aarch64-linux-android/release/determinism
#   bash tools/android_digest.sh target/aarch64-linux-android/release/determinism --expect 0x...
# Arguments after the binary go to it unchanged. ADB picks the adb binary (adb
# on PATH by default) and ANDROID_SERIAL the device. The binary runs as the
# shell user from /data/local/tmp: nothing is installed, and it is removed
# afterwards. The exit status is the binary's (adb shell passes it through).
#
# `just android-digest` (arm64-v8a, a real phone) and CI's `sim-android` job
# (x86_64, an emulator) both run through this.

if [ "$#" -lt 1 ]; then
    echo "usage: $0 BINARY [determinism flags...]" >&2
    exit 2
fi
bin=$1
shift
adb=${ADB:-adb}
remote=/data/local/tmp/salt-line-determinism

# Remove the binary however the script ends, a failed step or Ctrl-C included.
trap '"$adb" shell rm -f "$remote" >/dev/null 2>&1 || true' EXIT

"$adb" push "$bin" "$remote" >/dev/null
"$adb" shell chmod 755 "$remote"
"$adb" shell getprop ro.build.fingerprint | sed 's/^/device: /'
# adb shell joins the arguments and the device shell splits them again, which
# is fine for the flags the binary takes (--expect 0x...), not for spaces.
status=0
"$adb" shell "$remote" "$@" || status=$?
exit "$status"

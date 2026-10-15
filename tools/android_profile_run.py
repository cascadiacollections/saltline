#!/usr/bin/env python3
"""Run the installed-or-given profiling APK on a device and report it.

`just android-profile` builds and launches the profiling build
(tools/android_profile.py); this runs it to the end and prints what it
measured, so a capture can be repeated and compared without watching
logcat:

* installs the APK if one is given, then wakes the device and launches it;
* polls the Godot log (tag `godot`) with `logcat -d` after enlarging the
  log buffer, so a device whose other apps flood logcat cannot push the
  summary out before it is read, and an adb server restart cannot cut a
  stream short;
* waits for the timing summary (`salt-profile: done` and the lines after
  it), and with --gpu-check also for each GPU check scene's verdict;
* warns when the game lost focus during the run (a lock screen or a
  notification shade over it pauses the game, which stretches the capture
  and spoils its timings), and fails if a GPU check failed.

It never unlocks a device: keep it unlocked and awake (Developer options,
Stay awake) for the length of the run.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import time
from pathlib import Path

PACKAGE = "com.cascadiacollections.saltline.profile"
GPU_SCENES = 3
SUMMARY = re.compile(r"(updated|frozen)( [a-z_]+)?: ")
VERDICT = re.compile(r"salt-profile: gpu check scene=(\S+) (PASS|FAIL)")


def message(line: str) -> str:
    """The text of a `logcat -v time` line after its tag and pid."""
    return line.split("): ", 1)[1] if "): " in line else line


def summary(lines: list[str]) -> list[str]:
    """The capture's summary: the `done` line and the timing lines after it."""
    out: list[str] = []
    started = False
    for text in map(message, lines):
        if text.startswith("salt-profile: done"):
            started = True
        if started and (text.startswith("salt-profile: done") or SUMMARY.match(text)):
            out.append(text)
    return out


def verdicts(lines: list[str]) -> list[tuple[str, bool]]:
    """Each GPU check scene's name and whether it passed, in order."""
    found = []
    for text in map(message, lines):
        m = VERDICT.search(text)
        if m:
            found.append((m.group(1), m.group(2) == "PASS"))
    return found


def merge(seen: dict[str, None], dump: str) -> None:
    """Add the new lines of a `logcat -d` dump to `seen`, in log order.

    Each dump repeats what is still in the buffer, oldest first, so a line
    not seen before is newer than every line before it in the dump, and
    lines logged in the same millisecond keep the order they were logged
    in (sorting them would not)."""
    for line in dump.splitlines():
        if line and line not in seen:
            seen[line] = None


def focus_is_elsewhere(window_dump: str) -> bool:
    """Whether a lock screen or the notification shade has the focus."""
    focus = next((l for l in window_dump.splitlines() if "mCurrentFocus" in l), "")
    return bool(re.search(r"NotificationShade|Keyguard|StatusBar", focus))


class Device:
    def __init__(self, adb: str, serial: str | None):
        self.base = [adb] + (["-s", serial] if serial else [])

    def run(self, *args: str, timeout: float = 60) -> str:
        result = subprocess.run(
            self.base + list(args), capture_output=True, text=True, timeout=timeout
        )
        return result.stdout

    def shell(self, command: str, timeout: float = 60) -> str:
        return self.run("shell", command, timeout=timeout)


def main() -> int:
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Android/Sdk"))
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("apk", nargs="?", help="install this APK first (default: run what is installed)")
    parser.add_argument("--gpu-check", action="store_true", help="also wait for every GPU check scene")
    parser.add_argument("--serial", default=os.environ.get("ANDROID_SERIAL"))
    parser.add_argument("--adb", default=str(sdk / "platform-tools/adb"))
    parser.add_argument("--timeout", type=float, default=900, help="seconds before giving up")
    parser.add_argument("--log", type=Path, help="also write the whole godot log here")
    args = parser.parse_args()

    device = Device(args.adb, args.serial)
    device.run("wait-for-device", timeout=120)
    if args.apk:
        out = device.run("install", "-r", "-d", args.apk, timeout=300)
        if "Success" not in out:
            print(f"install failed: {out.strip()}", file=sys.stderr)
            return 2
    device.run("logcat", "-G", "16M")
    device.run("logcat", "-c")
    device.shell("input keyevent KEYCODE_WAKEUP")
    if focus_is_elsewhere(device.shell("dumpsys window")):
        print("the device is locked or a shade is open: unlock it and keep it awake", file=sys.stderr)
        return 2
    device.shell(f"am force-stop {PACKAGE}")
    device.shell(f"monkey -p {PACKAGE} -c android.intent.category.LAUNCHER 1")

    lines: dict[str, None] = {}
    lost_focus: list[str] = []
    deadline = time.monotonic() + args.timeout
    done = False
    while time.monotonic() < deadline:
        time.sleep(5)
        merge(lines, device.run("logcat", "-d", "-v", "time", "-s", "godot:I"))
        ordered = list(lines)
        if focus_is_elsewhere(device.shell("dumpsys window")):
            lost_focus.append(time.strftime("%H:%M:%S"))
        finished = bool(summary(ordered))
        if args.gpu_check:
            finished = finished and len(verdicts(ordered)) >= GPU_SCENES
        if finished:
            done = True
            break
        if not device.shell(f"pidof {PACKAGE}").strip():
            print("the profiling app stopped before it finished", file=sys.stderr)
            break
    device.shell(f"am force-stop {PACKAGE}")

    ordered = list(lines)
    if args.log:
        args.log.write_text("\n".join(ordered) + "\n", encoding="utf-8")
    for line in summary(ordered):
        print(line)
    failed = False
    for scene, passed in verdicts(ordered):
        print(f"gpu check {scene}: {'PASS' if passed else 'FAIL'}")
        failed |= not passed
    if lost_focus:
        print(f"warning: the game lost focus at {', '.join(lost_focus)}; its timings are not comparable", file=sys.stderr)
    if not done:
        print("timed out or stopped before the capture finished", file=sys.stderr)
        return 1
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Version bookkeeping for releases. Standard library only and Python 3.9+,
so CI and `just` can run it with any python3 and no environment.

    release_version.py check [--tag vX.Y.Z]
        Every place the version is written down agrees (see RELEASING.md,
        "Version rule"). With --tag, the tag must match it too, and
        CHANGELOG.md must have a non-empty section for that version.

    release_version.py version
        Print the Cargo workspace version.

    release_version.py notes X.Y.Z
        Print that version's CHANGELOG.md section (the release notes).

    release_version.py android-code --run N [--release]
        Print the Android versionCode for this build (RELEASING.md,
        "Android versionCode").

    release_version.py set-android --code N [--name NAME]
        Write version/code (and optionally version/name) into the Android,
        Android Shareware and F-Droid presets in export_presets.cfg, and
        the same fields in fdroid/version.txt. Godot's CLI cannot override
        the preset fields. CI calls this on a throwaway checkout and still
        exports only Android and Android Shareware.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NoReturn

try:
    import tomllib  # Python 3.11+
except ImportError:  # pragma: no cover - older Pythons fall back to a regex
    tomllib = None

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")

# versionCode = major*10^8 + minor*10^6 + patch*10^4 + build. Android caps
# versionCode at 2,100,000,000, which is what bounds major at 20.
MAX_MAJOR, MAX_MINOR, MAX_PATCH = 20, 99, 99
RELEASE_BUILD = 9999
MAX_DEV_BUILD = RELEASE_BUILD - 1


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def read(name: str) -> str:
    try:
        return (ROOT / name).read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"{name} not found at the repository root")


def cargo_version() -> str:
    text = read("Cargo.toml")
    if tomllib is not None:
        try:
            version = tomllib.loads(text).get("workspace", {}).get("package", {}).get("version")
        except tomllib.TOMLDecodeError as e:
            fail(f"Cargo.toml does not parse: {e}")
    else:
        m = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
        v = m and re.search(r'^version\s*=\s*"([^"]*)"', m.group(1), re.M)
        version = v.group(1) if v else None
    if not version:
        fail("Cargo.toml has no [workspace.package] version")
    return version


def godot_version() -> str:
    text = read("project.godot")
    m = re.search(r'^config/version="([^"]*)"', text, re.M)
    if not m:
        fail("project.godot has no config/version")
    return m.group(1)


def presets() -> dict[str, dict[str, str]]:
    """Map preset name -> its [preset.N.options] key/values (raw strings)."""
    names: dict[str, str] = {}
    options: dict[str, dict[str, str]] = {}
    section = None
    for line in read("export_presets.cfg").splitlines():
        line = line.strip()
        if not line or line.startswith(";"):
            continue
        m = re.match(r"^\[(preset\.\d+)(\.options)?\]$", line)
        if m:
            section = m.group(0)[1:-1]
            continue
        if section and "=" in line:
            key, value = line.split("=", 1)
            value = value.strip().strip('"')
            if section.endswith(".options"):
                options.setdefault(section[: -len(".options")], {})[key] = value
            elif key == "name":
                names[section] = value
    return {names[p]: options.get(p, {}) for p in names}


def parse_semver(version: str) -> tuple[int, int, int]:
    m = SEMVER.match(version)
    if not m:
        fail(f"{version!r} is not a plain MAJOR.MINOR.PATCH version")
    major, minor, patch = (int(g) for g in m.groups())
    if major > MAX_MAJOR or minor > MAX_MINOR or patch > MAX_PATCH:
        fail(
            f"{version} is outside the Android versionCode scheme "
            f"(major <= {MAX_MAJOR}, minor and patch <= {MAX_MINOR}); see RELEASING.md"
        )
    return major, minor, patch


def changelog_section(version: str) -> str:
    """The version's CHANGELOG.md section; fails unless it has bullet entries."""
    text = read("CHANGELOG.md")
    m = re.search(
        rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## |^\[[^\]]+\]: |\Z)",
        text,
        re.M | re.S,
    )
    if not m:
        fail(f"CHANGELOG.md has no '## [{version}]' section")
    section = m.group(1).strip()
    if not re.search(r"^\s*[-*] \S", section, re.M):
        fail(f"CHANGELOG.md's '## [{version}]' section has no entries")
    return section


def check(tag: str | None) -> None:
    version = cargo_version()
    parse_semver(version)
    found = {"Cargo.toml [workspace.package] version": version}

    gv = godot_version()
    # The displayed version may carry a channel label ("0.1.0-preview"); only
    # the part before the first '-' has to agree.
    found['project.godot config/version (before "-")'] = gv.split("-", 1)[0]

    p = presets()
    fdroid_code, fdroid_name = fdroid_version_file()
    found[f"{FDROID_VERSION_FILE} versionName"] = fdroid_name
    try:
        # Each shareware preset is a copy of its full one (`tier.rs`) and
        # carries the same version. F-Droid is the full edition too.
        for name in ("Android", "Android Shareware", FDROID_PRESET):
            found[f"export_presets.cfg {name} version/name"] = p[name]["version/name"]
        fdroid_preset_code = p[FDROID_PRESET]["version/code"]
        for name in ("macOS", "macOS Shareware"):
            for key in ("application/short_version", "application/version"):
                found[f"export_presets.cfg {name} {key}"] = p[name][key]
        for name in ("Windows Desktop", "Windows Desktop ARM64", "Windows Desktop Shareware"):
            for key in ("application/file_version", "application/product_version"):
                v = p[name][key]
                # Windows wants four parts; the fourth is always 0.
                found[f"export_presets.cfg {name} {key} (without .0)"] = v.removesuffix(".0")
    except KeyError as e:
        fail(f"export_presets.cfg is missing {e}")

    if tag is not None:
        if not tag.startswith("v"):
            fail(f"tag {tag!r} does not start with 'v'")
        parse_semver(tag[1:])
        found[f"tag {tag}"] = tag[1:]

    width = max(len(k) for k in found)
    for k, v in found.items():
        print(f"{k:<{width}}  {v}")
    if len(set(found.values())) != 1:
        fail("versions disagree; bump them together (RELEASING.md, 'Version rule')")

    expected_code = str(release_code_for(version))
    codes = {
        f"export_presets.cfg {FDROID_PRESET} version/code": fdroid_preset_code,
        f"{FDROID_VERSION_FILE} versionCode": fdroid_code,
    }
    for label, got in codes.items():
        print(f"{label:<{width}}  {got}")
        if got != expected_code:
            fail(
                f"{label} is {got}, expected {expected_code} for {version} "
                "(RELEASING.md, 'Version rule')"
            )

    if tag is not None:
        changelog_section(version)
    print(f"ok: {version}")


def release_code_for(version: str) -> int:
    """versionCode for a vX.Y.Z tag of this version (RELEASING.md)."""
    major, minor, patch = parse_semver(version)
    return major * 10**8 + minor * 10**6 + patch * 10**4 + RELEASE_BUILD


def android_code(run: int, release: bool) -> int:
    version = cargo_version()
    if release:
        return release_code_for(version)
    major, minor, patch = parse_semver(version)
    build = min(max(run, 0), MAX_DEV_BUILD)
    return major * 10**8 + minor * 10**6 + patch * 10**4 + build


# The full Android preset and its shareware copy (`tier.rs`) share a version.
# CI exports those two. F-Droid is a third Android preset: same rewrite, so a
# third platform preset does not change what those two exports receive.
ANDROID_PRESETS = ("Android", "Android Shareware")
FDROID_PRESET = "F-Droid"
VERSIONED_ANDROID_PRESETS = ANDROID_PRESETS + (FDROID_PRESET,)
FDROID_VERSION_FILE = "fdroid/version.txt"


def fdroid_version_file() -> tuple[str, str]:
    """(versionCode, versionName) from fdroid/version.txt."""
    text = read(FDROID_VERSION_FILE)
    code = re.search(r"^versionCode=(\d+)\s*$", text, re.M)
    name = re.search(r"^versionName=(\S+)\s*$", text, re.M)
    if not code or not name:
        fail(f"{FDROID_VERSION_FILE} must contain versionCode=<int> and versionName=<version>")
    return code.group(1), name.group(1)


def write_fdroid_version_file(code: int, name: str | None) -> None:
    path = ROOT / FDROID_VERSION_FILE
    text = read(FDROID_VERSION_FILE)
    if not re.search(r"^versionCode=\d+\s*$", text, re.M):
        fail(f"{FDROID_VERSION_FILE} has no versionCode")
    text = re.sub(r"^versionCode=\d+\s*$", f"versionCode={code}", text, count=1, flags=re.M)
    if name is not None:
        if not re.search(r"^versionName=\S+\s*$", text, re.M):
            fail(f"{FDROID_VERSION_FILE} has no versionName")
        text = re.sub(r"^versionName=\S+\s*$", f"versionName={name}", text, count=1, flags=re.M)
    if not text.endswith("\n"):
        text += "\n"
    path.write_text(text, encoding="utf-8")


def set_android(code: int, name: str | None) -> None:
    # Fail before either file is written if the F-Droid bookkeeping is absent.
    fdroid_version_file()
    path = ROOT / "export_presets.cfg"
    lines = read("export_presets.cfg").splitlines(keepends=True)
    sections = {
        s: opts for s, opts in _preset_sections(lines).items() if opts in VERSIONED_ANDROID_PRESETS
    }
    missing = set(VERSIONED_ANDROID_PRESETS) - set(sections.values())
    if missing:
        fail(f"no {', '.join(sorted(missing))} preset in export_presets.cfg")
    want = {"code"} | ({"name"} if name is not None else set())
    changed: dict[str, set[str]] = {preset: set() for preset in sections}
    section = None
    for i, line in enumerate(lines):
        m = re.match(r"^\[(.*)\]\s*$", line)
        if m:
            section = m.group(1)
            continue
        preset = section.removesuffix(".options") if section and section.endswith(".options") else None
        if preset not in sections:
            continue
        if line.startswith("version/code="):
            lines[i] = f"version/code={code}\n"
            changed[preset].add("code")
        elif name is not None and line.startswith("version/name="):
            lines[i] = f'version/name="{name}"\n'
            changed[preset].add("name")
    for preset, keys in changed.items():
        if keys != want:
            fail(f"could not find {sorted(want - keys)} in the {sections[preset]} preset")
    path.write_text("".join(lines), encoding="utf-8")
    write_fdroid_version_file(code, name)
    print(
        f"{', '.join(VERSIONED_ANDROID_PRESETS)} presets: version/code={code}"
        + (f' version/name="{name}"' if name else "")
    )
    print(
        f"{FDROID_VERSION_FILE}: versionCode={code}"
        + (f" versionName={name}" if name else "")
    )


def _preset_sections(lines: list[str]) -> dict[str, str]:
    out, section = {}, None
    for line in lines:
        m = re.match(r"^\[(preset\.\d+)\]\s*$", line)
        if m:
            section = m.group(1)
        elif section and line.startswith("name="):
            out[section] = line.split("=", 1)[1].strip().strip('"')
            section = None
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check")
    c.add_argument("--tag")
    sub.add_parser("version")
    n = sub.add_parser("notes")
    n.add_argument("version")
    a = sub.add_parser("android-code")
    a.add_argument("--run", type=int, required=True)
    a.add_argument("--release", action="store_true")
    s = sub.add_parser("set-android")
    s.add_argument("--code", type=int, required=True)
    s.add_argument("--name")
    args = ap.parse_args()

    if args.cmd == "check":
        check(args.tag)
    elif args.cmd == "version":
        print(cargo_version())
    elif args.cmd == "notes":
        print(changelog_section(args.version))
    elif args.cmd == "android-code":
        print(android_code(args.run, args.release))
    elif args.cmd == "set-android":
        set_android(args.code, args.name)


if __name__ == "__main__":
    main()

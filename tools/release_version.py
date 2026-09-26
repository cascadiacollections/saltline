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
        Write version/code (and optionally version/name) into the Android
        preset in export_presets.cfg. Godot's CLI cannot override them.
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
    try:
        found["export_presets.cfg Android version/name"] = p["Android"]["version/name"]
        found["export_presets.cfg macOS application/short_version"] = p["macOS"]["application/short_version"]
        found["export_presets.cfg macOS application/version"] = p["macOS"]["application/version"]
        for name in ("Windows Desktop", "Windows Desktop ARM64"):
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

    if tag is not None:
        changelog_section(version)
    print(f"ok: {version}")


def android_code(run: int, release: bool) -> int:
    major, minor, patch = parse_semver(cargo_version())
    build = RELEASE_BUILD if release else min(max(run, 0), MAX_DEV_BUILD)
    return major * 10**8 + minor * 10**6 + patch * 10**4 + build


def set_android(code: int, name: str | None) -> None:
    path = ROOT / "export_presets.cfg"
    lines = read("export_presets.cfg").splitlines(keepends=True)
    android = next(
        (s for s, opts in _preset_sections(lines).items() if opts == "Android"), None
    )
    if android is None:
        fail("no Android preset in export_presets.cfg")
    section, changed = None, set()
    for i, line in enumerate(lines):
        m = re.match(r"^\[(.*)\]\s*$", line)
        if m:
            section = m.group(1)
            continue
        if section != f"{android}.options":
            continue
        if line.startswith("version/code="):
            lines[i] = f"version/code={code}\n"
            changed.add("code")
        elif name is not None and line.startswith("version/name="):
            lines[i] = f'version/name="{name}"\n'
            changed.add("name")
    want = {"code"} | ({"name"} if name is not None else set())
    if changed != want:
        fail(f"could not find {sorted(want - changed)} in the Android preset")
    path.write_text("".join(lines), encoding="utf-8")
    print(f"Android preset: version/code={code}" + (f' version/name="{name}"' if name else ""))


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

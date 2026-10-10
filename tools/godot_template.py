#!/usr/bin/env python3
"""Salt Line's own Godot export templates: Godot built from source without
the parts of the engine this game never uses (docs/godot-templates.md).
Standard library only and Python 3.9+, like release_version.py.

    godot_template.py key
        Print the key: a hash of everything that decides what the build
        produces (Godot tag and commit, the SCons version, the build profile,
        the published platforms' SCons flags, and the source of this file's
        build and packaging functions). The release holding a build is
        named godot-templates-<version>-<key>.

    godot_template.py build {android,linux} [--arch A] [--src DIR] [--out DIR] [-j N]
        Fetch the pinned Godot source (a shallow clone of the tag, checked
        against the pinned commit) and build one release template into --out
        (default build/godot-templates). Runs the pinned SCons through uv
        (`uvx --from scons==SCONS_VERSION scons`); Android also needs
        ANDROID_HOME with the NDK Godot asks for (Godot installs it with
        sdkmanager when missing). An Android build took 3 minutes on 16
        cores; expect tens of minutes on a 4-core CI runner (not measured).

    godot_template.py fetch [--out DIR] [--require]
        Download the build tools/godot_templates.lock.json names (gh CLI) and
        check every file against the lock's sha256. When the lock is for the
        current key, anything short of matching files fails. When it is for
        another key (the profile or flags changed and nobody has rebuilt yet),
        or missing, this prints a warning and exits 0 with no files, so the
        export falls back to the official templates; --require makes that
        fail too, and also fails a lock that names no Android engine (CI
        passes it on release tags).

    godot_template.py package-android ENGINE_SO [--official APK] [--out APK]
        Make an Android release template: the official android_release.apk
        with its arm64 engine library replaced by ENGINE_SO and the ABIs
        Salt Line does not ship removed. The Java side is the official one
        of the same Godot tag, so only the native library differs. Fails
        unless ENGINE_SO was built by the same compiler as the official
        engine (the ELF .comment), since both share libc++_shared.so.

    godot_template.py use PRESET TEMPLATE
        Point PRESET's custom_template/release in export_presets.cfg at
        TEMPLATE (an absolute path). For CI's throwaway checkout and local
        exports; the committed presets keep the official templates.

    godot_template.py lock [--dir DIR | --release]
        Rewrite tools/godot_templates.lock.json for the current key from the
        files in DIR (default build/godot-templates), or with --release from
        the files the godot-templates workflow published for this key, after
        `gh attestation verify` proves each was built by that workflow on
        main. A local lock (--dir) is for trying things out, never for a
        pull request.
"""

from __future__ import annotations

import argparse
import hashlib
import inspect
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parent.parent
PROFILE = ROOT / "tools" / "godot_build_profile.json"
# The publishing workflow dogfoods this portable recipe. Hash it as well
# as the legacy local recipe so a real CI build change cannot reuse a release.
PORTABLE_RECIPE = tuple(ROOT / ".github" / "actions" / "build-godot-template" / name
                        for name in ("action.yml", "build.py")) + (ROOT / ".github" / "workflows" / "godot-templates.yml",)
LOCK = ROOT / "tools" / "godot_templates.lock.json"
DEFAULT_OUT = ROOT / "build" / "godot-templates"
REPO = "cascadiacollections/saltline"

# The engine version. GODOT_VERSION in .github/workflows/ci.yml must match
# (checked by `key` when that variable is set, as it is in CI).
GODOT_VERSION = "4.7.2"
GODOT_TAG = f"{GODOT_VERSION}-stable"
# The commit the tag points at. A moved tag fails the build instead of
# quietly building something else.
GODOT_COMMIT = "ed1daf0bf001b61586d9930840f2f1394092c079"
GODOT_GIT = "https://github.com/godotengine/godot.git"
# SCons, pinned: a different SCons can build different bytes. Run through uv.
SCONS_VERSION = "4.11.1"
SCONS = ["uvx", "--from", f"scons=={SCONS_VERSION}", "scons"]
# Who may have built a release this lock trusts (lock --release).
SIGNER_WORKFLOW = f"{REPO}/.github/workflows/godot-templates.yml"
SIGNER_REF = "refs/heads/main"

# SCons arguments shared by every platform. The build profile holds the
# feature switches (what is left out); these are how it is compiled.
# `lto` and `production` must be on the command line: SConstruct sets them
# after it reads the profile.
COMMON_FLAGS = [
    "target=template_release",
    "production=yes",
    "lto=full",
    "optimize=size_extra",
    "debug_symbols=no",
]
PLATFORM_FLAGS = {
    # Swappy paces Vulkan frames; this build has no Vulkan.
    "android": ["platform=android", "swappy=no"],
    # No text-to-speech or system fonts: the game draws its own text.
    "linux": ["platform=linuxbsd", "speechd=no", "fontconfig=no"],
}
DEFAULT_ARCH = {"android": "arm64", "linux": "x86_64"}
# What the godot-templates workflow builds and CI ships. Linux builds locally
# but is not published (docs/godot-templates.md, "Not covered yet").
PUBLISHED = ["android"]
# Android ABIs in the official template that Salt Line does not ship
# (export_presets.cfg, Android: arm64-v8a only).
ANDROID_ABI = "arm64-v8a"


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def warn(msg: str) -> None:
    # GitHub turns this into an annotation; locally it is just a line.
    prefix = "::warning::" if os.environ.get("GITHUB_ACTIONS") else "warning: "
    print(f"{prefix}{msg}", file=sys.stderr)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def output_name(platform: str, arch: str) -> str:
    """The file name SCons writes into bin/."""
    if platform == "android":
        return f"libgodot.android.template_release.{arch}.so"
    return f"godot.linuxbsd.template_release.{arch}"


def scons_args(platform: str, arch: str) -> list[str]:
    return [*PLATFORM_FLAGS[platform], f"arch={arch}", *COMMON_FLAGS]


def key() -> str:
    env_version = os.environ.get("GODOT_VERSION")
    if env_version and env_version != GODOT_VERSION:
        fail(
            f"GODOT_VERSION is {env_version} but tools/godot_template.py pins {GODOT_VERSION}; "
            "bump both together (docs/godot-templates.md, 'Bumping Godot')"
        )
    h = hashlib.sha256()
    h.update(f"{GODOT_TAG}\n{GODOT_COMMIT}\nscons {SCONS_VERSION}\n".encode())
    # The code that builds and packages, so a change to it moves the key.
    for fn in RECIPE:
        h.update(inspect.getsource(fn).encode())
    for path in PORTABLE_RECIPE:
        h.update(path.name.encode())
        h.update(path.read_bytes())
    for platform in PUBLISHED:
        h.update(f"{platform}: {' '.join(scons_args(platform, DEFAULT_ARCH[platform]))}\n".encode())
    # Canonical JSON, so reformatting the profile does not force a rebuild.
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    h.update(json.dumps(profile, sort_keys=True, separators=(",", ":")).encode())
    return h.hexdigest()[:16]


def release_tag(k: str) -> str:
    return f"godot-templates-{GODOT_VERSION}-{k}"


def run(cmd: list[str], cwd: Path | None = None) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def fetch_source(src: Path) -> None:
    if not (src / ".git").exists():
        src.parent.mkdir(parents=True, exist_ok=True)
        run(["git", "clone", "--depth", "1", "--branch", GODOT_TAG, GODOT_GIT, str(src)])
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=src, check=True, capture_output=True, text=True
    ).stdout.strip()
    if head != GODOT_COMMIT:
        fail(f"{src} is at {head}, not {GODOT_TAG} ({GODOT_COMMIT})")
    dirty = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        cwd=src, check=True, capture_output=True, text=True,
    ).stdout.strip()
    if dirty:
        fail(f"{src} has local changes to tracked files:\n{dirty}")


def check_profile(src: Path) -> None:
    """Godot applies a build profile's options without checking their names,
    so an option renamed or removed in a new Godot would be silently ignored.
    Fail instead when this source tree does not know one."""
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    known = (src / "SConstruct").read_text(encoding="utf-8")
    for detect in (src / "platform").glob("*/detect.py"):
        known += detect.read_text(encoding="utf-8")
    unknown = []
    for option in profile.get("disabled_build_options", {}):
        m = re.fullmatch(r"module_(\w+)_enabled", option)
        if m:
            if not (src / "modules" / m.group(1) / "config.py").is_file():
                unknown.append(option)
        elif f'"{option}"' not in known:
            unknown.append(option)
    if unknown:
        fail(f"{PROFILE.name} sets options Godot {GODOT_TAG} does not have: {', '.join(unknown)}")


def build(platform: str, arch: str | None, src: Path, out: Path, jobs: int) -> None:
    arch = arch or DEFAULT_ARCH[platform]
    fetch_source(src)
    check_profile(src)
    run([*SCONS, f"-j{jobs}", *scons_args(platform, arch), f"build_profile={PROFILE}"], cwd=src)
    built = src / "bin" / output_name(platform, arch)
    if platform == "android":
        # Android builds move the library into the Gradle project's libs.
        abi = {"arm64": "arm64-v8a", "x86_64": "x86_64"}.get(arch, arch)
        built = src / "platform" / "android" / "java" / "lib" / "libs" / "release" / abi / "libgodot_android.so"
    if not built.is_file():
        fail(f"SCons finished but {built} is missing")
    out.mkdir(parents=True, exist_ok=True)
    dest = out / output_name(platform, arch)
    shutil.copy2(built, dest)
    print(f"{dest}  {dest.stat().st_size} bytes  sha256 {sha256(dest)}")


def official_templates() -> Path:
    """Where the Godot editor keeps the official templates for this version."""
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support" / "Godot"
    elif sys.platform == "win32":
        base = Path(os.environ["APPDATA"]) / "Godot"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")) / "godot"
    return base / "export_templates" / f"{GODOT_VERSION}.stable"


def elf_comment(data: bytes) -> list[str]:
    """The strings in an ELF64 little-endian file's .comment section: the
    compiler and linker versions that built it."""
    if data[:6] != b"\x7fELF\x02\x01":
        fail("not a 64-bit little-endian ELF file")
    shoff = int.from_bytes(data[0x28:0x30], "little")
    shentsize = int.from_bytes(data[0x3A:0x3C], "little")
    shnum = int.from_bytes(data[0x3C:0x3E], "little")
    shstrndx = int.from_bytes(data[0x3E:0x40], "little")

    def section(i: int) -> tuple[int, int, int]:
        base = shoff + i * shentsize
        name = int.from_bytes(data[base : base + 4], "little")
        off = int.from_bytes(data[base + 0x18 : base + 0x20], "little")
        size = int.from_bytes(data[base + 0x20 : base + 0x28], "little")
        return name, off, size

    _, stroff, _ = section(shstrndx)
    for i in range(shnum):
        name, off, size = section(i)
        end = data.index(b"\0", stroff + name)
        if data[stroff + name : end] == b".comment":
            return [x.decode("utf-8", "replace") for x in data[off : off + size].split(b"\0") if x]
    return []


def check_compiler(engine: bytes, official: bytes) -> None:
    """The stripped engine and the official one must come from the same
    compiler: they load the same libc++_shared.so from the template."""
    ours, theirs = elf_comment(engine), elf_comment(official)
    if not ours or not theirs or ours[0] != theirs[0]:
        fail(
            "the engine was not built by the compiler the official template was "
            f"(ours: {ours[:1]}, official: {theirs[:1]}); build with the NDK Godot "
            "asks for (platform/android/detect.py, get_ndk_version)"
        )
    print(f"compiler matches the official engine: {ours[0]}")


def package_android(engine: Path, official: Path, out: Path) -> None:
    if not official.is_file():
        fail(f"official Android template not found at {official} (install Godot's export templates)")
    lib = f"lib/{ANDROID_ABI}/libgodot_android.so"
    with zipfile.ZipFile(official) as src:
        check_compiler(engine.read_bytes(), src.read(lib))
    out.parent.mkdir(parents=True, exist_ok=True)
    replaced = False
    with zipfile.ZipFile(official) as src, zipfile.ZipFile(out, "w") as dst:
        for info in src.infolist():
            name = info.filename
            if name.startswith("lib/") and not name.startswith(f"lib/{ANDROID_ABI}/"):
                continue  # an ABI the preset does not export
            if name == lib:
                data, replaced = engine.read_bytes(), True
            else:
                data = src.read(info)
            # Same name, time and compression as the official entry: Godot's
            # exporter copies each template entry with its own method.
            dst.writestr(info, data, compress_type=info.compress_type)
    if not replaced:
        fail(f"{official} has no {lib}")
    print(f"{out}  {out.stat().st_size} bytes")


def set_preset_template(preset: str, template: str) -> None:
    path = ROOT / "export_presets.cfg"
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    section_of, current = {}, None
    for line in lines:
        m = re.match(r"^\[(preset\.\d+)\]\s*$", line)
        if m:
            current = m.group(1)
        elif current and line.startswith("name="):
            section_of[line.split("=", 1)[1].strip().strip('"')] = current
            current = None
    if preset not in section_of:
        fail(f"no preset named {preset!r} in export_presets.cfg")
    want = f"[{section_of[preset]}.options]"
    entry = f'custom_template/release="{template}"\n'
    out, in_options, done = [], False, False
    for line in lines:
        if line.strip().startswith("["):
            in_options = line.strip() == want
            out.append(line)
            if in_options:
                out.append(entry)
                done = True
            continue
        if in_options and line.startswith("custom_template/release="):
            continue
        out.append(line)
    if not done:
        fail(f"preset {preset!r} has no {want} section")
    path.write_text("".join(out), encoding="utf-8")
    print(f"{preset}: custom_template/release={template}")


def read_lock() -> dict:
    try:
        return json.loads(LOCK.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}


def download(release: str, names: list[str], into: Path) -> bool:
    cmd = ["gh", "release", "download", release, "--repo", REPO, "--dir", str(into)]
    for n in names:
        cmd += ["--pattern", n]
    print("+ " + " ".join(cmd), flush=True)
    return subprocess.run(cmd, check=False).returncode == 0


def lock_from_release() -> None:
    k = key()
    names = [output_name(p, DEFAULT_ARCH[p]) for p in PUBLISHED]
    with tempfile.TemporaryDirectory() as tmp:
        cmd = ["gh", "release", "download", release_tag(k), "--repo", REPO, "--dir", tmp]
        print("+ " + " ".join(cmd), flush=True)
        if subprocess.run(cmd, check=False).returncode != 0:
            fail(f"no release {release_tag(k)}; run the godot-templates workflow first")
        found = [Path(tmp) / n for n in names if (Path(tmp) / n).is_file()]
        if not found:
            fail(f"{release_tag(k)} holds none of {names}")
        for f in found:
            # Built by the godot-templates workflow, from main, in this repo.
            cmd = [
                "gh", "attestation", "verify", str(f), "--repo", REPO,
                "--signer-workflow", SIGNER_WORKFLOW, "--source-ref", SIGNER_REF,
            ]
            print("+ " + " ".join(cmd), flush=True)
            if subprocess.run(cmd, check=False).returncode != 0:
                fail(f"{f.name} in {release_tag(k)} has no valid attestation from {SIGNER_WORKFLOW} on {SIGNER_REF}")
        write_lock(Path(tmp))


def write_lock(directory: Path) -> None:
    k = key()
    files = {}
    for platform in PUBLISHED:
        f = directory / output_name(platform, DEFAULT_ARCH[platform])
        if f.is_file():
            files[f.name] = sha256(f)
    if not files:
        fail(f"no built templates in {directory}")
    lock = {
        "godot": GODOT_TAG,
        "commit": GODOT_COMMIT,
        "key": k,
        "release": release_tag(k),
        "files": dict(sorted(files.items())),
    }
    LOCK.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {LOCK.relative_to(ROOT)} for {lock['release']}")


def fetch(out: Path, require: bool) -> None:
    def unavailable(msg: str) -> None:
        if require:
            fail(msg)
        warn(msg + "; exporting with the official Godot templates")
        sys.exit(0)

    k = key()
    lock = read_lock()
    if lock.get("key") != k:
        unavailable(
            f"tools/godot_templates.lock.json is for key {lock.get('key')}, the build inputs are {k} "
            "(run the godot-templates workflow, then `godot_template.py lock --release`)"
        )
    engine = output_name("android", DEFAULT_ARCH["android"])
    if require and engine not in lock.get("files", {}):
        fail(f"tools/godot_templates.lock.json pins no {engine}")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        names = list(lock["files"])
        if not download(lock["release"], names, Path(tmp)):
            # The lock is current, so this build is meant to ship it.
            fail(f"could not download {lock['release']}")
        for n in names:
            got = Path(tmp) / n
            if not got.is_file():
                fail(f"{lock['release']} has no {n}")
            digest = sha256(got)
            if digest != lock["files"][n]:
                fail(f"{n} from {lock['release']} has sha256 {digest}, the lock says {lock['files'][n]}")
            shutil.move(str(got), out / n)
            print(f"{out / n}  sha256 {digest}  (matches the lock)")


# What `key` hashes the source of. Defined after them; read at call time.
RECIPE = (output_name, scons_args, fetch_source, check_profile, build, elf_comment, check_compiler, package_android)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("key")
    b = sub.add_parser("build")
    b.add_argument("platform", choices=sorted(PLATFORM_FLAGS))
    b.add_argument("--arch")
    b.add_argument("--src", type=Path, default=Path.home() / ".cache" / "salt-line" / f"godot-{GODOT_TAG}")
    b.add_argument("--out", type=Path, default=DEFAULT_OUT)
    b.add_argument("-j", "--jobs", type=int, default=os.cpu_count() or 4)
    f = sub.add_parser("fetch")
    f.add_argument("--out", type=Path, default=DEFAULT_OUT)
    f.add_argument("--require", action="store_true")
    p = sub.add_parser("package-android")
    p.add_argument("engine", type=Path)
    p.add_argument("--official", type=Path, default=None)
    p.add_argument("--out", type=Path, default=DEFAULT_OUT / "android_release.apk")
    u = sub.add_parser("use")
    u.add_argument("preset")
    u.add_argument("template")
    lk = sub.add_parser("lock")
    lk_src = lk.add_mutually_exclusive_group()
    lk_src.add_argument("--dir", type=Path, default=DEFAULT_OUT)
    lk_src.add_argument("--release", action="store_true")
    args = ap.parse_args()

    if args.cmd == "key":
        print(key())
    elif args.cmd == "build":
        build(args.platform, args.arch, args.src.resolve(), args.out.resolve(), args.jobs)
    elif args.cmd == "fetch":
        fetch(args.out.resolve(), args.require)
    elif args.cmd == "package-android":
        package_android(args.engine, args.official or official_templates() / "android_release.apk", args.out)
    elif args.cmd == "use":
        if not os.path.isabs(args.template):
            fail("the template path must be absolute: Godot resolves it from its own working directory")
        set_preset_template(args.preset, args.template)
    elif args.cmd == "lock":
        if args.release:
            lock_from_release()
        else:
            write_lock(args.dir.resolve())


if __name__ == "__main__":
    main()

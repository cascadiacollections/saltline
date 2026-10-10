#!/usr/bin/env python3
"""Portable, standard-library driver for pinned Godot release-template builds."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ARCHES = {"android": {"arm64", "arm32", "x86_64", "x86_32"}, "linux": {"x86_64", "arm64"}}
COMMON = ["target=template_release", "production=yes", "lto=full", "optimize=size_extra", "debug_symbols=no"]


def run(args, cwd=None):
    subprocess.run(args, cwd=cwd, check=True)


def sha256(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def configuration(env=os.environ):
    commit = env["ENGINE_COMMIT"]
    tag = env["ENGINE_TAG"]
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("commit must be a full lowercase 40-character SHA")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,100}", tag):
        raise ValueError("tag must be a simple Godot source tag")
    if not re.fullmatch(r"\d+\.\d+\.\d+", env["SCONS_VERSION"]):
        raise ValueError("SCons version must be an exact numeric version")
    platform, arch = env["BUILD_PLATFORM"], env["BUILD_ARCH"]
    if arch not in ARCHES.get(platform, set()):
        raise ValueError("unsupported platform/architecture pair")
    profile = json.loads(Path(env["BUILD_PROFILE"]).read_text())
    if not isinstance(profile, dict) or profile.get("type") != "build_profile":
        raise ValueError("profile must be a Godot build_profile")
    options = profile.get("disabled_build_options", {})
    if not isinstance(options, dict) or any(not isinstance(k, str) or not isinstance(v, bool) for k, v in options.items()):
        raise ValueError("disabled_build_options must map option names to booleans")
    classes = profile.get("disabled_classes", [])
    if not isinstance(classes, list) or any(not isinstance(name, str) for name in classes):
        raise ValueError("disabled_classes must be a list of class names")
    if not env.get("BUILD_TOOLCHAIN", "").strip():
        raise ValueError("toolchain identity is required for safe cache partitioning")
    flags = json.loads(env.get("BUILD_FLAGS", "[]"))
    if not isinstance(flags, list) or any(not isinstance(f, str) or not re.fullmatch(r"[A-Za-z0-9_]+=[A-Za-z0-9_.+-]+", f) for f in flags):
        raise ValueError("flags must be a JSON array of name=value options")
    reserved = {"platform", "arch", "target", "production", "lto", "optimize", "debug_symbols", "build_profile"}
    if any(f.split("=", 1)[0] in reserved for f in flags):
        raise ValueError("flags cannot override the optimized release recipe")
    if len({f.split("=", 1)[0] for f in flags}) != len(flags):
        raise ValueError("duplicate additional flags are not allowed")
    return dict(commit=commit, tag=tag, scons=env["SCONS_VERSION"], platform=platform, arch=arch, profile=profile, flags=flags)


def scons_args(config):
    return [f"platform={'linuxbsd' if config['platform'] == 'linux' else 'android'}", f"arch={config['arch']}", *COMMON, *config["flags"]]


def output_name(config):
    platform, arch = config["platform"], config["arch"]
    return f"libgodot.android.template_release.{arch}.so" if platform == "android" else f"godot.linuxbsd.template_release.{arch}"


def content_key(config, env=os.environ):
    # The cache partitions by runner image and caller's toolchain identity;
    # content-addressing is invalidation, not a reproducible-build claim.
    recipe = dict(config, script=sha256(Path(__file__)), runner={k: env.get(k, "") for k in ("RUNNER_OS", "RUNNER_ARCH", "ImageOS", "ImageVersion")}, toolchain=env.get("BUILD_TOOLCHAIN", ""))
    return hashlib.sha256(json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def emit(values):
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            for key, value in values.items():
                if "\n" in str(value) or "\r" in str(value):
                    raise ValueError("output cannot contain newlines")
                stream.write(f"{key}={value}\n")


def check_profile(src, profile, flags=()):
    known = (src / "SConstruct").read_text()
    known += "".join(p.read_text() for p in (src / "platform").glob("*/detect.py"))
    for option in [*profile.get("disabled_build_options", {}), *(flag.split("=", 1)[0] for flag in flags)]:
        module = re.fullmatch(r"module_(\w+)_enabled", option)
        if (module and not (src / "modules" / module[1] / "config.py").is_file()) or (not module and f'"{option}"' not in known):
            raise ValueError(f"profile option unknown to pinned engine: {option}")


def build(config, src, out):
    if src.exists():
        raise ValueError("source directory must not already exist")
    run(["git", "clone", "--depth", "1", "--branch", config["tag"], "https://github.com/godotengine/godot.git", str(src)])
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=src, text=True).strip()
    if head != config["commit"]:
        raise ValueError(f"tag resolves to {head}, expected {config['commit']}")
    check_profile(src, config["profile"], config["flags"])
    profile = src.parent / "build-profile.json"
    profile.write_text(json.dumps(config["profile"]))
    run(["uvx", "--from", f"scons=={config['scons']}", "scons", f"-j{os.cpu_count() or 2}", *scons_args(config), f"build_profile={profile}"], cwd=src)
    built = src / "bin" / output_name(config)
    if config["platform"] == "android":
        abi = {"arm64": "arm64-v8a", "arm32": "armeabi-v7a", "x86_32": "x86", "x86_64": "x86_64"}[config["arch"]]
        built = src / "platform/android/java/lib/libs/release" / abi / "libgodot_android.so"
    out.mkdir(parents=True, exist_ok=True)
    shutil.copy2(built, out / output_name(config))


def report(config, out):
    binary = out / output_name(config)
    digest, size = sha256(binary), binary.stat().st_size
    (out / "SHA256SUMS").write_text(f"{digest}  {binary.name}\n")
    manifest = dict(config, key=content_key(config), file=binary.name, sha256=digest, bytes=size)
    (out / "build-report.json").write_text(json.dumps(manifest, indent=2) + "\n")
    emit(dict(binary=str(binary), sha256=digest, bytes=size, report=str(out / "build-report.json")))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as stream:
            stream.write(f"Godot `{config['platform']}/{config['arch']}`: {size:,} bytes; SHA-256 `{digest}`.\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["key", "build", "report"])
    args = parser.parse_args()
    config = configuration()
    out = Path(os.environ["BUILD_OUTPUT"]).resolve()
    if args.command == "key":
        emit(dict(key=content_key(config), directory=str(out)))
    elif args.command == "build":
        build(config, Path(os.environ["BUILD_SOURCE"]).resolve(), out)
    else:
        report(config, out)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError) as error:
        sys.exit(str(error))

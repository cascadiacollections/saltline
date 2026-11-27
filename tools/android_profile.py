#!/usr/bin/env python3
"""Build the isolated profiling APK on the pinned release engine by default.

Run through the repository's toolchain (mise exec -- python3 ...). Requires
Godot, the export templates, Android SDK/NDK, Java and the Android Rust target.
--engine official is an explicit baseline; metadata beside the APK records
which library is actually inside it. Never installs over the player's game.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from contextlib import contextmanager
from pathlib import Path

import godot_template as templates

ROOT = templates.ROOT
PACKAGE = "com.cascadiacollections.saltline.profile"


def pinned_engine(directory: Path) -> Path:
    lock = templates.read_lock()
    name = templates.output_name("android", "arm64")
    if lock.get("key") != templates.key() or name not in lock.get("files", {}):
        raise RuntimeError("no current pinned Android engine; publish and pin it first")
    engine = directory / name
    # Cache is useful for an offline device session, but is never trusted by name.
    if not engine.is_file() or templates.sha256(engine) != lock["files"][name]:
        templates.fetch(directory, require=True)
    if templates.sha256(engine) != lock["files"][name]:
        raise RuntimeError("cached engine does not match the release lock")
    return engine


def configure_profile(text: str) -> str:
    """Point the store Android preset at the profiling package.

    The F-Droid preset reuses the display name "Salt Line", so a whole-file
    count of that name is not one, and replacing every copy would rename the
    F-Droid build too. Only the preset named Android is rewritten.
    """
    lines = text.splitlines(keepends=True)
    section = _only_preset(lines, "Android")
    want = f"[{section}.options]"
    app_id = 'package/unique_name="com.cascadiacollections.saltline"'
    app_name = 'package/name="Salt Line"'
    out: list[str] = []
    in_options = False
    saw_id = False
    saw_name = False
    for line in lines:
        header = line.strip()
        if header.startswith("[") and header.endswith("]"):
            in_options = header == want
        if in_options and line.startswith("package/unique_name="):
            if line.strip() != app_id:
                raise ValueError(f"Android preset package id is {line.strip()!r}, not the store id")
            saw_id = True
            line = f'package/unique_name="{PACKAGE}"{_endl(line)}'
        elif in_options and line.startswith("package/name="):
            if line.strip() != app_name:
                raise ValueError(f"Android preset name is {line.strip()!r}")
            saw_name = True
            line = f'package/name="Salt Line Profiler"{_endl(line)}'
        out.append(line)
    if not saw_id or not saw_name:
        raise ValueError("Android preset is missing its package id or name")
    return "".join(out)


def _only_preset(lines: list[str], name: str) -> str:
    """The `[preset.N]` whose `name` is `name`. Two of them is a mistake."""
    found: list[str] = []
    current: str | None = None
    for line in lines:
        header = re.match(r"^\[(preset\.\d+)\]\s*$", line)
        if header:
            current = header.group(1)
            continue
        if current and line.startswith("name="):
            if line.split("=", 1)[1].strip().strip('"') == name:
                found.append(current)
            current = None
    if len(found) != 1:
        raise ValueError(f"expected exactly one {name} preset, found {len(found)}")
    return found[0]


def _endl(line: str) -> str:
    if line.endswith("\r\n"):
        return "\r\n"
    if line.endswith("\n"):
        return "\n"
    return ""


@contextmanager
def profile_preset(template: Path):
    """Restore the exact original bytes even after a failed import/export."""
    presets = ROOT / "export_presets.cfg"
    original = presets.read_bytes()
    try:
        presets.write_text(configure_profile(original.decode("utf-8")), encoding="utf-8")
        templates.set_preset_template("Android", template.resolve().as_posix())
        yield
    finally:
        presets.write_bytes(original)


@contextmanager
def isolated_editor(godot: str, sdk: Path, java: Path):
    """Godot reads SDK paths from EditorSettings, not ANDROID_HOME/JAVA_HOME.

    Its self-contained mode keeps these settings and the export's doc cache
    out of the developer's editor configuration, on all three desktop OSes.
    """
    executable = Path(shutil.which(godot) or godot).resolve()
    with tempfile.TemporaryDirectory(prefix="profile-editor-", dir=ROOT / "build") as tmp:
        stage = Path(tmp)
        shutil.copy2(executable, stage / executable.name)
        if executable.name.endswith("_console.exe"):
            gui = executable.with_name(executable.name.replace("_console.exe", ".exe"))
            shutil.copy2(gui, stage / gui.name)
        (stage / "_sc_").touch()
        config = stage / "editor_data"
        config.mkdir()
        version = ".".join(templates.GODOT_VERSION.split(".")[:2])
        settings = ('[gd_resource type="EditorSettings" format=3]\n\n[resource]\n'
                    f'export/android/android_sdk_path = {json.dumps(sdk.as_posix())}\n'
                    f'export/android/java_sdk_path = {json.dumps(java.as_posix())}\n'
                    # The editor otherwise kills the adb server on exit, cutting off
                    # any other adb session (a logcat, another device's capture).
                    'export/android/shutdown_adb_on_exit = false\n')
        (config / f"editor_settings-{version}.tres").write_text(settings, encoding="utf-8")
        yield str(stage / executable.name)


def build(engine_kind: str, godot: str, sdk: Path, ndk: Path, java: Path) -> None:
    out = ROOT / "build"
    out.mkdir(exist_ok=True)
    (out / ".gdignore").touch()
    official = templates.official_templates() / "android_release.apk"
    if engine_kind == "pinned":
        engine = pinned_engine(out / "godot-templates")
        template = out / "godot-templates" / "android_release.apk"
        templates.package_android(engine, official, template)
        expected = templates.sha256(engine)
    else:
        template = official
        with zipfile.ZipFile(official) as archive:
            expected = hashlib.sha256(archive.read("lib/arm64-v8a/libgodot_android.so")).hexdigest()
    host = "windows-x86_64" if sys.platform == "win32" else "darwin-x86_64" if sys.platform == "darwin" else "linux-x86_64"
    bin_dir = ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin"
    linker = bin_dir / ("aarch64-linux-android24-clang.cmd" if sys.platform == "win32" else "aarch64-linux-android24-clang")
    if not linker.is_file():
        raise FileNotFoundError(f"Android linker missing: {linker}")
    env = dict(os.environ, ANDROID_HOME=str(sdk), JAVA_HOME=str(java),
               CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(linker))
    env["PATH"] = str(java / "bin") + os.pathsep + env["PATH"]

    def run(*args: str) -> None:
        subprocess.run(args, cwd=ROOT, env=env, check=True)

    run("cargo", "build", "-p", "salt-line-godot")
    if sys.platform == "win32":
        # The native Windows ARM editor resolves the explicit ARM64 path.
        host_info = subprocess.check_output(["rustc", "-vV"], cwd=ROOT, env=env, text=True)
        if "host: aarch64-pc-windows-msvc" in host_info:
            stage = ROOT / "target/aarch64-pc-windows-msvc/debug"
            stage.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / "target/debug/salt_line_godot.dll", stage / "salt_line_godot.dll")
    run("cargo", "build", "--release", "-p", "salt-line-godot", "--features", "profiling",
        "--target", "aarch64-linux-android")
    strip = bin_dir / ("llvm-strip.exe" if sys.platform == "win32" else "llvm-strip")
    run(str(strip), "--strip-debug", str(ROOT / "target/aarch64-linux-android/release/libsalt_line_godot.so"))
    key = out / "saltline-profile.keystore"
    if not key.exists():
        keytool = java / "bin" / ("keytool.exe" if sys.platform == "win32" else "keytool")
        run(str(keytool), "-genkeypair", "-keystore", str(key), "-storetype", "PKCS12",
            "-storepass", "android", "-keypass", "android", "-alias", "androiddebugkey",
            "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000", "-dname", "CN=SaltLine Profiling,O=Local,C=US")
    env.update(GODOT_ANDROID_KEYSTORE_RELEASE_PATH=str(key),
               GODOT_ANDROID_KEYSTORE_RELEASE_USER="androiddebugkey",
               GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD="android", GODOT_BIN=godot)
    apk = out / "SaltLine-profile.apk"
    metadata = out / "SaltLine-profile.engine.json"
    metadata.unlink(missing_ok=True)
    # An explicit official path also overrides any local custom template.
    with profile_preset(template), isolated_editor(godot, sdk, java) as editor:
        env["GODOT_BIN"] = editor
        bash = os.environ.get("BASH_BIN", "bash")
        if sys.platform == "win32" and "BASH_BIN" not in os.environ:
            git_bash = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
            if git_bash.is_file():
                bash = str(git_bash)
        run(bash, "tools/godot_headless.sh", "--frame-delay", "800", "--import")
        run(bash, "tools/godot_headless.sh", "--export-release", "Android", str(apk))
        with zipfile.ZipFile(apk) as archive:
            actual = hashlib.sha256(archive.read("lib/arm64-v8a/libgodot_android.so")).hexdigest()
        accepted = {expected}
        if engine_kind == "pinned":
            # The Gradle build (use_gradle_build) strips native libraries as
            # it packages them, so the pinned engine arrives as
            # `llvm-strip --strip-unneeded` of itself.
            with tempfile.TemporaryDirectory() as scratch:
                stripped = Path(scratch) / "libgodot_android.so"
                run(str(strip), "--strip-unneeded", "-o", str(stripped), str(engine))
                accepted.add(templates.sha256(stripped))
        if actual not in accepted:
            raise RuntimeError("exported APK does not contain the selected engine")
        info = {"engine": engine_kind, "godot": templates.GODOT_VERSION,
                "sha256": actual, "package": PACKAGE,
                "release": templates.read_lock().get("release") if engine_kind == "pinned" else None}
        metadata.write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(info, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", choices=("pinned", "official"), default="pinned")
    parser.add_argument("--godot", default=os.environ.get("GODOT_BIN", "godot"))
    parser.add_argument("--sdk", type=Path, default=os.environ.get("ANDROID_HOME", str(Path.home() / "Android/Sdk")))
    parser.add_argument("--ndk", type=Path, default=os.environ.get("ANDROID_NDK_HOME"))
    parser.add_argument("--java", type=Path, default=os.environ.get("JAVA_HOME"))
    args = parser.parse_args()
    ndks = sorted((args.sdk / "ndk").glob("*"), key=lambda p: tuple(int(n) for n in p.name.split(".")))
    ndk = args.ndk or (ndks[-1] if ndks else None)
    if not ndk or not args.java:
        parser.error("set ANDROID_NDK_HOME and JAVA_HOME (or --ndk and --java)")
    build(args.engine, args.godot, args.sdk.resolve(), ndk.resolve(), args.java.resolve())

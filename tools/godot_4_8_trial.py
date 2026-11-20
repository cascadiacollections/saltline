#!/usr/bin/env python3
"""Opt-in Godot 4.8-beta4 trial. The 4.7.2 pin is unchanged.

    godot_4_8_trial.py guard-stable
        Fail if this checkout no longer ships 4.7.2, or if the trial's
        custom-build key does not match tools/godot_4_8_trial.lock.json.

    godot_4_8_trial.py check-python
        Fail if this interpreter is older than 3.10. Godot 4.8-beta4's
        SConstruct calls EnsurePythonVersion(3, 10). The 4.7.2 scripts do
        not: they still run on Python 3.9+.

    godot_4_8_trial.py build-key [--check]
        Print the stripped-template key for the trial (tag, commit, SCons,
        shared profile minus the trial's drops, and the same release flags
        as 4.7.2). --check fails when the lock disagrees.

    godot_4_8_trial.py provenance
        Download each release's SHA512-SUMS.txt and require the lock's
        editor and template digests to match the published file.

    godot_4_8_trial.py run
        Provenance, then install both official editors and export templates,
        import and export Salt Line on each, and write
        build/godot-4.8-trial/report.md. An Android game APK is exported
        when ANDROID_HOME and JAVA_HOME are set and the arm64 GDExtension
        is built; otherwise the report measures the official template APKs.

    godot_4_8_trial.py build {android,linux}
        Build one stripped 4.8-beta4 release template. Refuses Python < 3.10.
        Fetches the pinned commit (the 4.8-beta4 tag is not on
        godotengine/godot). Does not publish and does not edit
        tools/godot_templates.lock.json.

    godot_4_8_trial.py validate-build DIRECTORY
        Require the build report's tag, commit, key and file digest to
        match the trial lock and the binary in DIRECTORY.

    godot_4_8_trial.py check-profile SRC
        Require the shared build profile, minus the trial drops, to be
        options SRC actually has, and require each drop to be unknown there.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import zipfile
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Generator

import godot_template

ROOT = godot_template.ROOT
LOCK_PATH = ROOT / "tools" / "godot_4_8_trial.lock.json"
REPORT_DIR = ROOT / "build" / "godot-4.8-trial"
ENGINE_OUT = ROOT / "build" / "godot-4.8-trial-engine"
HOST_RELEASE = ROOT / "target" / "release" / "libsalt_line_godot.so"
HOST_DEBUG = ROOT / "target" / "debug" / "libsalt_line_godot.so"
ANDROID_SO = ROOT / "target" / "aarch64-linux-android" / "release" / "libsalt_line_godot.so"
SUMS_NAME = "SHA512-SUMS.txt"
VERSION_RE = re.compile(
    r"^(?P<version>\d+\.\d+(?:\.\d+)?\.(?:stable|beta\d*|rc\d*|dev\d*))\."
    r"(?P<build>official|custom_build)\.(?P<hash>[0-9a-f]{9})\s*$"
)
INTERESTING = re.compile(
    r"SCRIPT ERROR|Parse Error|GDExtension|Failed to load|does not exist|is deprecated|ERROR:",
    re.IGNORECASE,
)


class TrialError(Exception):
    """A trial check failed. The stable pin is left alone."""


def load_lock() -> dict[str, Any]:
    return json.loads(LOCK_PATH.read_text(encoding="utf-8"))


LOCK = load_lock()


def trial() -> dict[str, Any]:
    return LOCK["trial"]


def compare() -> dict[str, Any]:
    return LOCK["compare"]


def specs() -> list[dict[str, Any]]:
    return [compare(), trial()]


def sha256(path: Path) -> str:
    return godot_template.sha256(path)


def sha512(path: Path) -> str:
    digest = hashlib.sha512()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cache_dir() -> Path:
    override = os.environ.get("GODOT_TRIAL_CACHE", "").strip()
    if override:
        return Path(override)
    return Path.home() / ".cache" / "salt-line" / "godot-4.8-trial"


def python_requirement_error(version: tuple[int, ...] | None = None) -> str | None:
    """None when this interpreter can build Godot 4.8-beta4."""
    if version is None:
        version = (sys.version_info[0], sys.version_info[1])
    if version >= (3, 10):
        return None
    return (
        f"Python {version[0]}.{version[1]} is below 3.10. Godot 4.8-beta4's SConstruct "
        "calls EnsurePythonVersion(3, 10); 4.7.2 still calls EnsurePythonVersion(3, 9). "
        "This check is only for the stripped 4.8 trial build. Salt Line's 4.7.2 scripts "
        "stay on Python 3.9+."
    )


def check_python() -> None:
    message = python_requirement_error()
    if message:
        raise TrialError(message)


def adjusted_profile(lock: dict[str, Any] | None = None) -> dict[str, Any]:
    """Shared profile with the trial-only drops removed. The file is not modified."""
    data = lock if lock is not None else LOCK
    profile = json.loads(godot_template.PROFILE.read_text(encoding="utf-8"))
    options = profile["disabled_build_options"]
    drops = list(data["custom_build"]["profile_drops"])
    missing = [name for name in drops if name not in options]
    if missing:
        raise TrialError(f"trial profile drops are not in the shared profile: {', '.join(missing)}")
    for name in drops:
        del options[name]
    # Options 4.8 adds that the 4.7.2 profile cannot name. 4.8 gates RD_ENABLED on its own
    # `rendering_device` option rather than on `vulkan`, so a Vulkan-less Android build must
    # turn it off too, or display_server_android.cpp calls a Vulkan-only function.
    sets = dict(data["custom_build"].get("profile_sets", {}))
    clash = sorted(name for name in sets if name in options)
    if clash:
        raise TrialError(f"trial profile sets are already in the shared profile: {', '.join(clash)}")
    options.update(sets)
    return profile


def custom_build_key(lock: dict[str, Any] | None = None) -> str:
    """Key of a stripped 4.8-beta4 template. Not the 4.7.2 lock key."""
    data = lock if lock is not None else LOCK
    custom = data["custom_build"]
    if custom["scons"] != godot_template.SCONS_VERSION:
        raise TrialError(
            f"trial lock pins SCons {custom['scons']} but tools/godot_template.py pins "
            f"{godot_template.SCONS_VERSION}"
        )
    digest = hashlib.sha256()
    spec = data["trial"]
    digest.update(
        f"{spec['godot']}\n{spec['commit']}\nstatus {spec['version_status']}\n"
        f"scons {custom['scons']}\n".encode()
    )
    drops = sorted(custom["profile_drops"])
    digest.update(("drops " + " ".join(drops) + "\n").encode())
    digest.update(json.dumps(adjusted_profile(data), sort_keys=True, separators=(",", ":")).encode())
    for platform in ("android", "linux"):
        flags = " ".join(godot_template.scons_args(platform, godot_template.DEFAULT_ARCH[platform]))
        digest.update(f"{platform}: {flags}\n".encode())
    return digest.hexdigest()[:16]


def guard_stable() -> None:
    """The shipping pin is still 4.7.2, and the trial key matches its lock."""
    if godot_template.GODOT_VERSION != "4.7.2" or godot_template.GODOT_TAG != "4.7.2-stable":
        raise TrialError(
            f"tools/godot_template.py pins {godot_template.GODOT_TAG}, not 4.7.2-stable"
        )
    if godot_template.GODOT_COMMIT != compare()["commit"]:
        raise TrialError("tools/godot_template.py commit does not match the trial's 4.7.2 compare pin")
    stable_lock = json.loads(godot_template.LOCK.read_text(encoding="utf-8"))
    if stable_lock.get("godot") != "4.7.2-stable" or stable_lock.get("commit") != compare()["commit"]:
        raise TrialError("tools/godot_templates.lock.json is no longer the 4.7.2 pin")
    if stable_lock.get("key") != godot_template.key():
        raise TrialError(
            "tools/godot_templates.lock.json key does not match the current 4.7.2 inputs; "
            "the trial must not move that key"
        )
    for relative in (".github/workflows/ci.yml", ".github/workflows/engine-validation.yml"):
        text = (ROOT / relative).read_text(encoding="utf-8")
        if not re.search(r"(?m)^  GODOT_VERSION: 4\.7\.2$", text):
            raise TrialError(f"{relative} no longer sets GODOT_VERSION: 4.7.2")
    features = (ROOT / "project.godot").read_text(encoding="utf-8")
    if '"4.7"' not in features:
        raise TrialError("project.godot config/features no longer names 4.7")
    cargo = (ROOT / "crates" / "salt-line-godot" / "Cargo.toml").read_text(encoding="utf-8")
    if 'features = ["api-4-7"' not in cargo:
        raise TrialError("salt-line-godot no longer pins api-4-7")
    got = custom_build_key()
    want = LOCK["custom_build"]["key"]
    if got != want:
        raise TrialError(f"trial custom-build key is {got}, tools/godot_4_8_trial.lock.json says {want}")


def parse_sums(text: str) -> dict[str, str]:
    found: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        digest, name = line.split(None, 1)
        found[name.lstrip("*").strip()] = digest
    return found


def provenance_errors(published: dict[str, str], artifacts: dict[str, Any]) -> list[str]:
    errors = []
    for artifact in artifacts.values():
        name = artifact["name"]
        if name not in published:
            errors.append(f"SHA512-SUMS.txt has no {name}")
        elif published[name] != artifact["sha512"]:
            errors.append(f"{name} lock sha512 does not match the published SHA512-SUMS.txt")
    return errors


def fetch_text(url: str) -> str:
    return subprocess.check_output(["curl", "-fsSL", "--retry", "3", url], text=True)


def check_provenance() -> dict[str, Any]:
    """Lock digests match the godot-builds SHA512-SUMS.txt for both versions."""
    checked = {}
    for spec in specs():
        url = spec["builds_release"].rstrip("/") + "/" + SUMS_NAME
        published = parse_sums(fetch_text(url))
        errors = provenance_errors(published, spec["artifacts"])
        if errors:
            raise TrialError(f"{spec['godot']}: " + "; ".join(errors))
        checked[spec["id"]] = {"sums": url, "artifacts": list(spec["artifacts"])}
        print(f"provenance ok: {spec['godot']} ({url})")
    return checked


def editor_version_error(text: str, spec: dict[str, Any]) -> str | None:
    match = VERSION_RE.match(text.strip().splitlines()[-1] if text.strip() else "")
    if not match:
        return f"unrecognized Godot version {text.strip()!r}"
    if match.group("version") != spec["editor_version"]:
        return f"editor is {match.group('version')}, lock says {spec['editor_version']}"
    if match.group("hash") != spec["commit_prefix"]:
        return f"editor commit {match.group('hash')} is not {spec['commit_prefix']} ({spec['commit']})"
    return None


def download(url: str, dest: Path, digest: str) -> None:
    if dest.is_file() and sha512(dest) == digest:
        print(f"cached {dest.name}")
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    partial = dest.with_name(dest.name + ".partial")
    print(f"download {url}")
    subprocess.run(["curl", "-fL", "--retry", "3", "-o", str(partial), url], check=True)
    got = sha512(partial)
    if got != digest:
        partial.unlink(missing_ok=True)
        raise TrialError(f"{dest.name} sha512 {got} does not match the trial lock")
    partial.replace(dest)


def artifact_path(cache: Path, spec: dict[str, Any], kind: str) -> Path:
    artifact = spec["artifacts"][kind]
    path = cache / artifact["name"]
    url = spec["builds_release"].rstrip("/") + "/" + artifact["name"]
    download(url, path, artifact["sha512"])
    return path


def godot_data_dir() -> Path:
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "Godot"
    if sys.platform == "win32":
        return Path(os.environ["APPDATA"]) / "Godot"
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")) / "godot"


def templates_root() -> Path:
    return godot_data_dir() / "export_templates"


def templates_ready(dest: Path, version: str) -> bool:
    return (
        (dest / "version.txt").is_file()
        and (dest / "version.txt").read_text(encoding="utf-8").strip() == version
        and (dest / "linux_release.x86_64").is_file()
        and (dest / "android_release.apk").is_file()
    )


def install_templates(tpz: Path, spec: dict[str, Any]) -> Path:
    version = spec["editor_version"]
    dest = templates_root() / version
    if not templates_ready(dest, version):
        if dest.exists():
            shutil.rmtree(dest)
        dest.mkdir(parents=True)
        print(f"install templates -> {dest}")
        with zipfile.ZipFile(tpz) as archive:
            raw = archive.read("templates/version.txt").decode().strip()
            if raw != version:
                raise TrialError(f"{tpz.name} version.txt is {raw!r}, lock says {version}")
            root = dest.resolve()
            for info in archive.infolist():
                name = info.filename
                if not name.startswith("templates/") or name.endswith("/"):
                    continue
                relative = name[len("templates/") :]
                out = (dest / relative).resolve()
                if not out.is_relative_to(root):
                    raise TrialError(f"template archive has an unsafe path {name}")
                out.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as src, out.open("wb") as dst:
                    shutil.copyfileobj(src, dst)
                mode = info.external_attr >> 16
                if mode & 0o111:
                    out.chmod(out.stat().st_mode | 0o755)
    binary = dest / "linux_release.x86_64"
    binary.chmod(binary.stat().st_mode | 0o755)
    text = subprocess.run(
        [str(binary), "--version"], check=True, capture_output=True, text=True, timeout=60
    ).stdout
    error = editor_version_error(text, spec)
    if error:
        raise TrialError(f"{version} template binary: {error}")
    print(f"template {version}: {text.strip()}")
    return dest


def install_editor(archive: Path, spec: dict[str, Any], cache: Path) -> tuple[Path, str]:
    name = spec["artifacts"]["editor_linux_x86_64"]["name"]
    binary_name = name[: -len(".zip")] if name.endswith(".zip") else name
    dest = cache / binary_name
    if not dest.is_file():
        with zipfile.ZipFile(archive) as zipped:
            zipped.extract(binary_name, cache)
        dest.chmod(0o755)
    completed = subprocess.run(
        [str(dest), "--version"], check=True, capture_output=True, text=True, timeout=60
    )
    text = completed.stdout.strip()
    error = editor_version_error(text, spec)
    if error:
        raise TrialError(error)
    print(f"editor {text}")
    return dest, text


def install_spec(spec: dict[str, Any], cache: Path) -> dict[str, Any]:
    editor_zip = artifact_path(cache, spec, "editor_linux_x86_64")
    templates_zip = artifact_path(cache, spec, "export_templates")
    editor, version_text = install_editor(editor_zip, spec, cache)
    templates = install_templates(templates_zip, spec)
    return {
        "editor": editor,
        "templates": templates,
        "version_text": version_text,
        "template_version_text": (templates / "version.txt").read_text(encoding="utf-8").strip(),
    }


def editor_settings_version(editor_version: str) -> str:
    major, minor, *_rest = editor_version.split(".")
    return f"{major}.{minor}"


@contextmanager
def self_contained_editor(
    editor: Path, settings_version: str, sdk: Path | None, java: Path | None
) -> Generator[Path, None, None]:
    """A throwaway editor so the trial does not write the developer's settings.

    Export templates stay in the shared Godot data dir; this links them in,
    because a self-contained editor looks beside its own binary.
    """
    build = ROOT / "build"
    build.mkdir(exist_ok=True)
    (build / ".gdignore").touch()
    temporary = tempfile.TemporaryDirectory(prefix="godot-trial-editor-", dir=build)
    try:
        stage = Path(temporary.name)
        dest = stage / editor.name
        shutil.copy2(editor, dest)
        dest.chmod(0o755)
        (stage / "_sc_").touch()
        data = stage / "editor_data"
        data.mkdir()
        (data / "export_templates").symlink_to(templates_root(), target_is_directory=True)
        if sdk is not None and java is not None:
            settings = (
                '[gd_resource type="EditorSettings" format=3]\n\n[resource]\n'
                f"export/android/android_sdk_path = {json.dumps(sdk.as_posix())}\n"
                f"export/android/java_sdk_path = {json.dumps(java.as_posix())}\n"
                "export/android/shutdown_adb_on_exit = false\n"
            )
            (data / f"editor_settings-{settings_version}.tres").write_text(settings, encoding="utf-8")
        yield dest
    finally:
        temporary.cleanup()


def android_toolchain() -> tuple[Path, Path] | None:
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    java = os.environ.get("JAVA_HOME_17_X64") or os.environ.get("JAVA_HOME")
    if not sdk or not java:
        return None
    sdk_path, java_path = Path(sdk), Path(java)
    if sdk_path.is_dir() and java_path.is_dir():
        return sdk_path, java_path
    return None


def require_host_extension() -> None:
    if not HOST_RELEASE.is_file():
        raise TrialError(
            "missing target/release/libsalt_line_godot.so "
            "(cargo build --release -p salt-line-godot)"
        )
    HOST_DEBUG.parent.mkdir(parents=True, exist_ok=True)
    if not HOST_DEBUG.is_file():
        shutil.copy2(HOST_RELEASE, HOST_DEBUG)


def run_godot(editor: Path, args: list[str], spec: dict[str, Any], env: dict[str, str]) -> None:
    run_env = dict(env)
    run_env["GODOT_BIN"] = str(editor)
    run_env["GODOT_TEMPLATE_VERSION"] = spec["editor_version"]
    subprocess.run(["bash", "tools/godot_headless.sh", *args], cwd=ROOT, env=run_env, check=True)


def interesting_lines(text: str) -> list[str]:
    found: list[str] = []
    for raw in text.splitlines():
        line = raw.strip()
        if line and INTERESTING.search(line) and line not in found:
            found.append(line)
    return found[:80]


def only_in_trial(trial_lines: list[str], stable_lines: list[str]) -> list[str]:
    stable = set(stable_lines)
    return [line for line in trial_lines if line not in stable]


def inspect_package(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise TrialError(f"missing package {path}")
    info: dict[str, Any] = {"path": str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path),
                            "bytes": path.stat().st_size}
    if zipfile.is_zipfile(path):
        libraries = []
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                if entry.is_dir() or not entry.filename.endswith((".so", ".dylib", ".dll")):
                    continue
                payload = archive.read(entry)
                libraries.append({
                    "name": entry.filename,
                    "bytes": entry.file_size,
                    "compressed_bytes": entry.compress_size,
                    "sha256": hashlib.sha256(payload).hexdigest(),
                })
        info["kind"] = "zip"
        info["libraries"] = libraries
        return info
    info["kind"] = "file"
    info["sha256"] = sha256(path)
    libraries = []
    for sibling in sorted(path.parent.glob("*salt_line_godot*")):
        if sibling.is_file():
            libraries.append({"name": sibling.name, "bytes": sibling.stat().st_size, "sha256": sha256(sibling)})
    info["libraries"] = libraries
    return info


def find_library(package: dict[str, Any], suffix: str) -> dict[str, Any] | None:
    for library in package.get("libraries") or []:
        if library["name"].endswith(suffix):
            return library
    return None


def smoke_launch(exe: Path, png: Path) -> dict[str, Any]:
    if png.exists():
        png.unlink()
    log_path = png.with_suffix(".log")
    with log_path.open("w", encoding="utf-8") as handle:
        process = subprocess.Popen(
            [str(exe), "--headless", "--", "--salt-seed", "20260907", "--salt-screen", "12",
             "--salt-shot", str(png.resolve()), "--salt-shot-at", "0.5"],
            cwd=ROOT, stdout=handle, stderr=subprocess.STDOUT, start_new_session=True,
        )
        for _ in range(90):
            if png.is_file() and png.stat().st_size > 8:
                break
            if process.poll() is not None:
                break
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                continue
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass  # the process group already exited before SIGTERM
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass  # the process group already exited before SIGKILL
        process.wait(timeout=3)
    ok = png.is_file() and png.read_bytes()[:4] == b"\x89PNG"
    tail = log_path.read_text(encoding="utf-8", errors="replace")[-2000:] if log_path.is_file() else ""
    if not ok:
        raise TrialError(f"{exe.name} did not render a PNG. Last log:\n{tail}")
    return {"png_bytes": png.stat().st_size, "log_tail": tail[-500:]}


def ensure_keystore(path: Path, java: Path) -> None:
    if path.is_file():
        return
    subprocess.run(
        [str(java / "bin" / "keytool"), "-genkeypair", "-keystore", str(path), "-storetype", "PKCS12",
         "-storepass", "android", "-keypass", "android", "-alias", "androiddebugkey",
         "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
         "-dname", "CN=Salt Line Godot Trial,O=Local,C=US"],
        check=True,
    )


def export_android(editor: Path, spec: dict[str, Any], out: Path, sdk: Path, java: Path) -> dict[str, Any]:
    if not ANDROID_SO.is_file():
        return {"skipped": "arm64 GDExtension is not built (target/aarch64-linux-android/release)"}
    ensure_keystore(out / "debug.keystore", java)
    env = os.environ.copy()
    env.update({
        "ANDROID_HOME": str(sdk),
        "JAVA_HOME": str(java),
        "GODOT_ANDROID_KEYSTORE_RELEASE_PATH": str(out / "debug.keystore"),
        "GODOT_ANDROID_KEYSTORE_RELEASE_USER": "androiddebugkey",
        "GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD": "android",
    })
    apk = out / "SaltLine.apk"
    run_godot(editor, ["--export-release", "Android", str(apk)], spec, env)
    if not apk.is_file() or apk.stat().st_size == 0:
        raise TrialError(f"{spec['godot']} Android export produced no APK")
    return inspect_package(apk)


def export_one(spec: dict[str, Any], installed: dict[str, Any], out: Path) -> dict[str, Any]:
    godot_dir = ROOT / ".godot"
    if godot_dir.exists():
        shutil.rmtree(godot_dir)
    out.mkdir(parents=True, exist_ok=True)
    toolchain = android_toolchain()
    sdk, java = toolchain if toolchain else (None, None)
    env = os.environ.copy()
    with self_contained_editor(
        installed["editor"], editor_settings_version(spec["editor_version"]), sdk, java
    ) as editor:
        run_godot(editor, ["--frame-delay", "800", "--import"], spec, env)
        log_path = godot_dir / "import.log"
        import_text = log_path.read_text(encoding="utf-8", errors="replace") if log_path.is_file() else ""
        (out / "import.log").write_text(import_text, encoding="utf-8")
        linux = out / "SaltLine.x86_64"
        run_godot(editor, ["--export-release", "Linux", str(linux)], spec, env)
        if not linux.is_file():
            raise TrialError(f"{spec['godot']} Linux export produced no binary")
        linux.chmod(linux.stat().st_mode | 0o755)
        smoked = smoke_launch(linux, out / "smoke.png")
        android_error = None
        if sdk is None or java is None:
            apk: dict[str, Any] = {"skipped": "ANDROID_HOME or JAVA_HOME is unset; game APK not exported"}
        else:
            try:
                apk = export_android(editor, spec, out, sdk, java)
            except (TrialError, subprocess.CalledProcessError, OSError) as exc:
                android_error = str(exc)
                apk = {"skipped": f"Android export failed: {exc}"}
    templates = installed["templates"]
    result = {
        "id": spec["id"],
        "editor_version": installed["version_text"],
        "template_version": installed["template_version_text"],
        "commit": spec["commit"],
        "import_lines": interesting_lines(import_text),
        "linux": inspect_package(linux),
        "smoke": smoked,
        "template_linux_x86_64_bytes": (templates / "linux_release.x86_64").stat().st_size,
        "template_android": inspect_package(templates / "android_release.apk"),
        "apk": apk,
    }
    if android_error:
        result["android_error"] = android_error
    return result


def delta(left: int | None, right: int | None) -> str:
    if left is None or right is None:
        return "—"
    sign = "+" if right - left > 0 else ""
    return f"{sign}{right - left:,} B"


def size_rows(stable: dict[str, Any], preview: dict[str, Any]) -> list[tuple[str, str, str, str]]:
    def linux_bytes(result: dict[str, Any]) -> int | None:
        linux = result.get("linux") or {}
        return linux.get("bytes")

    def apk_bytes(result: dict[str, Any]) -> int | None:
        apk = result.get("apk") or {}
        return apk.get("bytes")

    def template_apk(result: dict[str, Any]) -> int | None:
        return (result.get("template_android") or {}).get("bytes")

    def template_lib(result: dict[str, Any]) -> int | None:
        library = find_library(result.get("template_android") or {}, "libgodot_android.so")
        return library.get("bytes") if library else None

    def game_lib(result: dict[str, Any], suffix: str) -> int | None:
        library = find_library(result.get("apk") or {}, suffix)
        return library.get("bytes") if library else None

    rows_src = [
        ("Linux export (engine + pck)", linux_bytes(stable), linux_bytes(preview)),
        ("libsalt_line_godot.so beside the Linux export",
         (find_library(stable.get("linux") or {}, "libsalt_line_godot.so") or {}).get("bytes"),
         (find_library(preview.get("linux") or {}, "libsalt_line_godot.so") or {}).get("bytes")),
        ("Official linux_release.x86_64 template",
         stable.get("template_linux_x86_64_bytes"), preview.get("template_linux_x86_64_bytes")),
        ("Official android_release.apk template", template_apk(stable), template_apk(preview)),
        ("libgodot_android.so inside that template (uncompressed)",
         template_lib(stable), template_lib(preview)),
        ("Salt Line APK", apk_bytes(stable), apk_bytes(preview)),
        ("libgodot_android.so inside the Salt Line APK",
         game_lib(stable, "libgodot_android.so"), game_lib(preview, "libgodot_android.so")),
        ("libsalt_line_godot.so inside the Salt Line APK",
         game_lib(stable, "libsalt_line_godot.so"), game_lib(preview, "libsalt_line_godot.so")),
    ]
    rendered = []
    for label, left, right in rows_src:
        left_text = f"{left:,} B" if isinstance(left, int) else "—"
        right_text = f"{right:,} B" if isinstance(right, int) else "—"
        rendered.append((label, left_text, right_text, delta(left if isinstance(left, int) else None,
                                                             right if isinstance(right, int) else None)))
    return rendered


def engine_identity(package: dict[str, Any] | None) -> str | None:
    if not package or package.get("skipped"):
        return None
    library = find_library(package, "libgodot_android.so")
    if not library:
        return None
    return library["sha256"]


def check_packaged_engines(stable: dict[str, Any], preview: dict[str, Any]) -> list[str]:
    """A game APK must not have been packaged with the other version's engine."""
    notes = []
    identities = {
        "4.7.2 template": engine_identity(stable.get("template_android")),
        "4.8-beta4 template": engine_identity(preview.get("template_android")),
        "4.7.2 apk": engine_identity(stable.get("apk")),
        "4.8-beta4 apk": engine_identity(preview.get("apk")),
    }
    if identities["4.7.2 apk"] and identities["4.7.2 apk"] == identities["4.8-beta4 template"]:
        raise TrialError("the 4.7.2 APK contains the 4.8-beta4 template engine")
    if identities["4.8-beta4 apk"] and identities["4.8-beta4 apk"] == identities["4.7.2 template"]:
        raise TrialError("the 4.8-beta4 APK contains the 4.7.2 template engine")
    for label, digest in identities.items():
        if digest:
            notes.append(f"{label} libgodot_android.so sha256 {digest}")
    return notes


def render_markdown(report: dict[str, Any]) -> str:
    stable = report["exports"].get(compare()["id"], {})
    preview = report["exports"].get(trial()["id"], {})
    lines = [
        "# Godot 4.8-beta4 trial",
        "",
        "Official editor and export templates. 4.7.2 stays the shipping pin. Not a release build.",
        "",
        f"- 4.7.2 editor: `{stable.get('editor_version', 'not run')}`",
        f"- 4.8-beta4 editor: `{preview.get('editor_version', 'not run')}`",
        f"- Trial custom-build key (not built by `run`): `{report.get('custom_build_key', '')}`",
        f"- Shipping 4.7.2 template lock key: `{report.get('stable_lock_key', '')}`",
        "",
        "| | 4.7.2 official | 4.8-beta4 official | delta |",
        "| --- | ---: | ---: | ---: |",
    ]
    if stable and preview and "linux" in stable and "linux" in preview:
        for label, left, right, change in size_rows(stable, preview):
            lines.append(f"| {label} | {left} | {right} | {change} |")
    else:
        lines.append("| (export incomplete) | | | |")
    lines.append("")
    lines.append("Template APK figures are the official `android_release.apk` libraries, "
                 "measured from the installed templates. A Salt Line APK row is filled only "
                 "when this machine exported one. Gradle may strip native libraries, so an "
                 "APK library digest can differ from the template library digest.")
    lines.append("")
    introduced = report.get("warnings_only_on_4_8") or []
    lines.append("## Lines only in the 4.8-beta4 import log")
    lines.append("")
    if introduced:
        lines.extend(f"- `{line}`" for line in introduced)
    else:
        lines.append("None of the filtered import lines (script errors, GDExtension failures, "
                     "deprecations) were unique to 4.8-beta4.")
    lines.append("")
    for note in report.get("notes") or []:
        lines.append(f"- {note}")
    if report.get("error"):
        lines.extend(["", f"Stopped: {report['error']}"])
    lines.append("")
    return "\n".join(lines)


def write_report(report: dict[str, Any]) -> None:
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    (REPORT_DIR / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    text = render_markdown(report)
    (REPORT_DIR / "report.md").write_text(text, encoding="utf-8")
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(text)


def dirty_tracked() -> set[str]:
    text = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    names = set()
    for line in text.splitlines():
        if line.startswith("??"):
            continue
        names.add(line[3:])
    return names


def untracked_paths() -> set[str]:
    text = subprocess.check_output(["git", "status", "--porcelain", "-uall"], cwd=ROOT, text=True)
    return {line[3:] for line in text.splitlines() if line.startswith("?? ")}


def restore_worktree(before_dirty: set[str], before_untracked: set[str]) -> list[str]:
    """Put back files Godot rewrote. Leave the trial's own untracked files."""
    notes = []
    new_dirty = sorted(dirty_tracked() - before_dirty)
    if new_dirty:
        diffs = []
        for relative in new_dirty:
            path = ROOT / relative
            if path.is_file():
                original = subprocess.check_output(["git", "show", f"HEAD:{relative}"], cwd=ROOT)
                current = path.read_bytes()
                if original != current:
                    diff = "".join(difflib.unified_diff(
                        original.decode("utf-8", "replace").splitlines(True),
                        current.decode("utf-8", "replace").splitlines(True),
                        fromfile=relative, tofile=relative,
                    ))
                    diffs.append(diff[:4000])
        subprocess.run(["git", "checkout", "--", *new_dirty], cwd=ROOT, check=True)
        notes.append("restored " + ", ".join(new_dirty))
        notes.extend(diffs)
    for relative in sorted(untracked_paths() - before_untracked):
        if relative.startswith("build/") or relative.startswith("target/"):
            continue
        path = ROOT / relative
        if path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)
        notes.append(f"removed untracked {relative}")
    return notes


@contextmanager
def preserve_project() -> Generator[list[str], None, None]:
    """Restore files an editor run rewrites, including between the two versions."""
    before_dirty = dirty_tracked()
    before_untracked = untracked_paths()
    saved = {path: path.read_bytes() for path in (ROOT / "project.godot", ROOT / "export_presets.cfg")}
    notes: list[str] = []
    try:
        yield notes
    finally:
        for path, data in saved.items():
            if not path.is_file() or path.read_bytes() == data:
                continue
            diff = "".join(difflib.unified_diff(
                data.decode("utf-8", "replace").splitlines(True),
                path.read_text(encoding="utf-8", errors="replace").splitlines(True),
                fromfile=str(path.relative_to(ROOT)), tofile=str(path.relative_to(ROOT)),
            ))
            if diff:
                notes.append(diff[:4000])
            path.write_bytes(data)
        notes.extend(restore_worktree(before_dirty, before_untracked))


def cmd_run() -> None:
    guard_stable()
    require_host_extension()
    check_provenance()
    cache = cache_dir()
    installed = {spec["id"]: install_spec(spec, cache) for spec in specs()}
    report: dict[str, Any] = {
        "custom_build_key": custom_build_key(),
        "stable_lock_key": json.loads(godot_template.LOCK.read_text(encoding="utf-8"))["key"],
        "exports": {},
        "worktree": [],
        "notes": [
            "Compared official templates with each other. The stripped 4.7.2 engine "
            "pinned in tools/godot_templates.lock.json was not rebuilt or substituted.",
            "4.8-beta4 source tag is not published on godotengine/godot; official builds "
            f"are commit {trial()['commit']}.",
        ],
    }
    failure: str | None = None
    try:
        for spec in specs():
            print(f"== export {spec['godot']}")
            with preserve_project() as notes:
                report["exports"][spec["id"]] = export_one(spec, installed[spec["id"]], REPORT_DIR / spec["id"])
            report["worktree"].extend(notes)
        stable = report["exports"][compare()["id"]]
        preview = report["exports"][trial()["id"]]
        report["warnings_only_on_4_8"] = only_in_trial(preview["import_lines"], stable["import_lines"])
        report["notes"].extend(check_packaged_engines(stable, preview))
        android_errors = [
            f"{item['id']}: {item['android_error']}"
            for item in report["exports"].values()
            if item.get("android_error")
        ]
        if android_errors:
            raise TrialError("Android export failed. " + " ".join(android_errors))
    except (TrialError, subprocess.CalledProcessError, OSError) as exc:
        failure = str(exc)
        report["error"] = failure
    finally:
        write_report(report)
    if failure:
        raise TrialError(failure)


def option_known(src: Path, option: str, known: str) -> bool:
    match = re.fullmatch(r"module_(\w+)_enabled", option)
    if match:
        return (src / "modules" / match.group(1) / "config.py").is_file()
    return f'"{option}"' in known


def check_trial_profile(src: Path, lock: dict[str, Any] | None = None) -> None:
    data = lock if lock is not None else LOCK
    profile = json.loads(godot_template.PROFILE.read_text(encoding="utf-8"))
    drops = set(data["custom_build"]["profile_drops"])
    known = (src / "SConstruct").read_text(encoding="utf-8")
    platform = src / "platform"
    if platform.is_dir():
        known += "".join(path.read_text(encoding="utf-8") for path in platform.glob("*/detect.py"))
    unknown = []
    stale = []
    for option in profile["disabled_build_options"]:
        is_known = option_known(src, option, known)
        if option in drops:
            if is_known:
                stale.append(option)
        elif not is_known:
            unknown.append(option)
    for option in data["custom_build"].get("profile_sets", {}):
        if not option_known(src, option, known):
            unknown.append(option)
    extra = sorted(drops - set(profile["disabled_build_options"]))
    if extra or stale or unknown:
        raise TrialError(
            "trial profile does not match this Godot tree: "
            f"unknown={unknown or '-'} stale_drops={stale or '-'} not_in_profile={extra or '-'}"
        )


def run(cmd: list[str], cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def fetch_trial_source(src: Path) -> None:
    """Clone the pinned commit. `4.8-beta4` is not a tag on godotengine/godot."""
    commit = trial()["commit"]
    if (src / ".git").is_dir():
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=src, text=True).strip()
        if head != commit:
            raise TrialError(f"{src} is at {head}, not {commit}")
        return
    if src.exists():
        raise TrialError(f"{src} exists and is not a git checkout of {commit}")
    src.parent.mkdir(parents=True, exist_ok=True)
    src.mkdir()
    run(["git", "init", "-q"], cwd=src)
    run(["git", "remote", "add", "origin", godot_template.GODOT_GIT], cwd=src)
    run(["git", "fetch", "--depth", "1", "origin", commit], cwd=src)
    run(["git", "checkout", "--detach", "FETCH_HEAD"], cwd=src)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=src, text=True).strip()
    if head != commit:
        raise TrialError(f"fetched {head}, not {commit}")


def cmd_build(platform: str, arch: str | None, jobs: int, src: Path, out: Path) -> None:
    check_python()
    guard_stable()
    arch = arch or godot_template.DEFAULT_ARCH[platform]
    fetch_trial_source(src)
    check_trial_profile(src)
    profile_path = src.parent / "saltline-4.8-trial-profile.json"
    profile_path.write_text(json.dumps(adjusted_profile()), encoding="utf-8")
    env = os.environ.copy()
    env["GODOT_VERSION_STATUS"] = trial()["version_status"]
    run([*godot_template.SCONS, f"-j{jobs}", *godot_template.scons_args(platform, arch),
         f"build_profile={profile_path}"], cwd=src, env=env)
    built = src / "bin" / godot_template.output_name(platform, arch)
    if platform == "android":
        abi = {"arm64": "arm64-v8a", "arm32": "armeabi-v7a", "x86_32": "x86", "x86_64": "x86_64"}[arch]
        built = src / "platform" / "android" / "java" / "lib" / "libs" / "release" / abi / "libgodot_android.so"
    if not built.is_file():
        raise TrialError(f"SCons finished but {built} is missing")
    out.mkdir(parents=True, exist_ok=True)
    dest = out / godot_template.output_name(platform, arch)
    shutil.copy2(built, dest)
    report = {
        "tag": trial()["godot"],
        "editor_version": trial()["editor_version"],
        "commit": trial()["commit"],
        "version_status": trial()["version_status"],
        "key": custom_build_key(),
        "python": f"{sys.version_info[0]}.{sys.version_info[1]}.{sys.version_info[2]}",
        "scons": godot_template.SCONS_VERSION,
        "platform": platform,
        "arch": arch,
        "file": dest.name,
        "sha256": sha256(dest),
        "bytes": dest.stat().st_size,
        "profile_drops": list(LOCK["custom_build"]["profile_drops"]),
        "profile_sets": dict(LOCK["custom_build"].get("profile_sets", {})),
    }
    (out / "build-report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{dest}  {report['bytes']} bytes  sha256 {report['sha256']}  key {report['key']}")


def validate_build(directory: Path) -> None:
    report_path = directory / "build-report.json"
    if not report_path.is_file():
        raise TrialError(f"missing {report_path}")
    report = json.loads(report_path.read_text(encoding="utf-8"))
    binary = directory / report["file"]
    if report.get("tag") != trial()["godot"] or report.get("commit") != trial()["commit"]:
        raise TrialError("build report tag or commit does not match the trial lock")
    if report.get("editor_version") != trial()["editor_version"]:
        raise TrialError("build report editor version does not match the trial lock")
    if report.get("key") != custom_build_key():
        raise TrialError(f"build report key {report.get('key')} is not {custom_build_key()}")
    if report.get("version_status") != trial()["version_status"]:
        raise TrialError(f"build report was not built with GODOT_VERSION_STATUS={trial()['version_status']}")
    python = tuple(int(part) for part in str(report.get("python", "0")).split("."))
    if python_requirement_error(python):
        raise TrialError(f"build report python {report.get('python')} is below 3.10")
    if not binary.is_file():
        raise TrialError(f"missing {binary}")
    digest = sha256(binary)
    if digest != report.get("sha256") or binary.stat().st_size != report.get("bytes"):
        raise TrialError("build report digest or size does not match the binary")
    print(f"validated {binary.name}: key {report['key']} commit {report['commit']} sha256 {digest}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("guard-stable")
    sub.add_parser("check-python")
    key_parser = sub.add_parser("build-key")
    key_parser.add_argument("--check", action="store_true")
    sub.add_parser("provenance")
    sub.add_parser("run")
    build_parser = sub.add_parser("build")
    build_parser.add_argument("platform", choices=sorted(godot_template.PLATFORM_FLAGS))
    build_parser.add_argument("--arch")
    build_parser.add_argument("-j", "--jobs", type=int, default=os.cpu_count() or 4)
    build_parser.add_argument("--src", type=Path, default=Path.home() / ".cache" / "salt-line" / "godot-4.8-beta4")
    build_parser.add_argument("--out", type=Path, default=ENGINE_OUT)
    validate_parser = sub.add_parser("validate-build")
    validate_parser.add_argument("directory", type=Path)
    profile_parser = sub.add_parser("check-profile")
    profile_parser.add_argument("src", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.cmd == "guard-stable":
            guard_stable()
            print("stable pin is 4.7.2; trial key matches the lock")
        elif args.cmd == "check-python":
            check_python()
            print(f"Python {sys.version_info[0]}.{sys.version_info[1]} can build the 4.8-beta4 trial")
        elif args.cmd == "build-key":
            got = custom_build_key()
            print(got)
            if args.check:
                want = LOCK["custom_build"]["key"]
                if got != want:
                    raise TrialError(f"trial custom-build key is {got}, lock says {want}")
        elif args.cmd == "provenance":
            check_provenance()
        elif args.cmd == "run":
            cmd_run()
        elif args.cmd == "build":
            cmd_build(args.platform, args.arch, args.jobs, args.src.resolve(), args.out.resolve())
        elif args.cmd == "validate-build":
            validate_build(args.directory.resolve())
        elif args.cmd == "check-profile":
            check_trial_profile(args.src.resolve())
            print(f"profile ok for {args.src}")
    except TrialError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

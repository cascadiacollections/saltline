#!/usr/bin/env python3
"""Build a signed iOS archive; optionally upload for TestFlight testing."""
import argparse
import datetime
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
APP = "SaltLine"
CRATE = "salt-line-godot"
LIB = "libsalt_line_godot"
BUNDLE = "com.cascadiacollections.saltline"

def run(*args, **kwargs):
    subprocess.run([str(arg) for arg in args], cwd=ROOT, check=True, **kwargs)

def default_build_number():
    now = datetime.datetime.now(datetime.timezone.utc)
    days = (now.date() - datetime.date(2020, 1, 1)).days
    return f"{days}.{now.hour}.{now.minute}"

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upload", action="store_true", help="upload the archive to TestFlight (internal-only unless --external is set)")
    parser.add_argument("--external", action="store_true", help="allow an uploaded build to be submitted for external beta review")
    parser.add_argument("--check", action="store_true", help="check local prerequisites without building or contacting Apple")
    parser.add_argument("--build-number", default=default_build_number())
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Run on macOS with Xcode installed")
    for tool in ("xcodebuild", "rustup", "python3"):
        if not shutil.which(tool):
            parser.error(f"Missing {tool}")
    godot = os.environ.get("GODOT_BIN", "godot")
    version = subprocess.check_output([godot, "--version"], text=True).strip()
    if not version.startswith("4.7.2."):
        parser.error(f"Godot 4.7.2 and matching export templates required; found {version}. Set GODOT_BIN.")
    preset = (ROOT / "export_presets.cfg").read_text()
    match = re.search(r'^application/app_store_team_id="([^"\n]+)"', preset, re.M)
    team = os.environ.get("IOS_TEAM") or (match.group(1) if match else "")
    if not re.fullmatch(r"[A-Z0-9]{10}", team):
        parser.error("Set IOS_TEAM to your Apple Developer Program team ID")
    if not re.fullmatch(r"[1-9][0-9]{0,3}(?:\.[0-9]{1,2}){0,2}", args.build_number):
        parser.error("Build number must have a positive major of up to four digits and up to two components of two digits")
    run("xcodebuild", "-version")
    print(f"{APP}: {BUNDLE}; team {team}; build {args.build_number}", flush=True)
    if args.check:
        return
    cargo = subprocess.check_output(["rustup", "which", "cargo"], cwd=ROOT, text=True).strip()
    rustc = subprocess.check_output(["rustup", "which", "rustc"], cwd=ROOT, text=True).strip()
    env = dict(os.environ, RUSTC=rustc)
    run("rustup", "target", "add", "aarch64-apple-ios")
    run(cargo, "build", "-p", CRATE, env=env)
    run(cargo, "build", "--release", "-p", CRATE, "--target", "aarch64-apple-ios", env=env)
    stage = ROOT / "build" / "testflight" / args.build_number
    stage.mkdir(parents=True, exist_ok=False)
    project = stage / "project"
    project.mkdir()
    run("bash", "tools/godot_headless.sh", "--frame-delay", "800", "--import")
    run("bash", "tools/godot_headless.sh", "--export-release", "iOS", project / f"{APP}.ipa")
    if not list(project.rglob(f"{LIB}.framework")):
        raise RuntimeError("Export is missing the iOS GDExtension framework")
    info = project / APP / f"{APP}-Info.plist"
    with info.open("rb") as f:
        metadata = plistlib.load(f)
    metadata["CFBundleVersion"] = args.build_number
    # Neither game requests camera, microphone, or photo access.
    for key in ("NSCameraUsageDescription", "NSMicrophoneUsageDescription", "NSPhotoLibraryUsageDescription"):
        if metadata.get(key) == "":
            metadata.pop(key)
    with info.open("wb") as f:
        plistlib.dump(metadata, f)
    archive = stage / f"{APP}.xcarchive"
    run("xcodebuild", "-project", project / f"{APP}.xcodeproj", "-scheme", APP,
        "-configuration", "Release", "-destination", "generic/platform=iOS",
        "-archivePath", archive, "-allowProvisioningUpdates", f"DEVELOPMENT_TEAM={team}",
        f"PRODUCT_BUNDLE_IDENTIFIER={BUNDLE}", "CODE_SIGN_STYLE=Automatic", "CODE_SIGN_IDENTITY=Apple Development",
        "PROVISIONING_PROFILE_SPECIFIER=", "archive")
    print(f"Archive: {archive}", flush=True)
    if args.upload:
        options = stage / "ExportOptions.plist"
        with options.open("wb") as f:
            plistlib.dump(dict(method="app-store-connect", destination="upload", teamID=team,
                              signingStyle="automatic", testFlightInternalTestingOnly=not args.external,
                              manageAppVersionAndBuildNumber=False, uploadSymbols=True), f)
        run("xcodebuild", "-exportArchive", "-archivePath", archive,
            "-exportOptionsPlist", options, "-exportPath", stage / "upload", "-allowProvisioningUpdates")
        print("Uploaded. Wait for Apple processing, then assign the build to a TestFlight group.")

if __name__ == "__main__":
    main()

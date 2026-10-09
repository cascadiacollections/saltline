"""Select expensive Apple CI coverage; main and tags always cover all platforms."""
import json
import os
from pathlib import Path
import subprocess


def needs_apple(paths):
    # Changes to dependencies, export inputs, or automation need Apple feedback
    # before merge. Ordinary game logic/assets get that feedback on main.
    return any(
        path.startswith((".github/", ".cargo/", "tools/", "fastlane/"))
        or path in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "mise.toml",
                    "Justfile", "project.godot", "export_presets.cfg",
                    "salt_line.gdextension"}
        or path.endswith("/Cargo.toml")
        or any(part in path.lower() for part in ("macos", "ios", "apple", "darwin"))
        for path in paths
    )


def plan(apple):
    sim = ["ubuntu-latest", "ubuntu-24.04-arm", "windows-latest", "windows-11-arm"]
    playable = [
        dict(os="ubuntu-24.04", preset="Linux", output="SaltLine.x86_64", zip="SaltLine-linux-x86_64.zip"),
        dict(os="ubuntu-24.04-arm", preset="Linux ARM64", output="SaltLine.arm64", zip="SaltLine-linux-arm64.zip"),
        dict(os="windows-latest", preset="Windows Desktop", output="SaltLine.exe", zip="SaltLine-windows-x86_64.zip"),
    ]
    if apple:
        sim.append("macos-latest")
        playable.append(dict(os="macos-latest", preset="macOS", output="SaltLine.app", zip="SaltLine-macos.zip"))
    return dict(apple=str(apple).lower(), sim=json.dumps(sim),
                playable=json.dumps(playable), digests=str(len(sim) + 1 + int(apple)))


if __name__ == "__main__":
    apple = True
    if os.environ["EVENT"] == "pull_request":
        changed = subprocess.check_output([
            "git", "diff", "--name-only", "-z",
            os.environ["BASE_SHA"] + "..." + os.environ["HEAD_SHA"],
        ]).decode().split("\0")
        apple = needs_apple(changed)
    outputs = plan(apple)
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for key, value in outputs.items():
            print(f"{key}={value}", file=output)
    print("Apple coverage:", outputs["apple"], "— expected digests:", outputs["digests"])

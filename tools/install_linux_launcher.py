#!/usr/bin/env python3
"""Register a local game launcher so GNOME/Wayland can resolve its app icon."""

import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def exec_argument(value):
    # Desktop Exec syntax uses double quotes, not shell single quotes.
    value = str(value).replace("\\", "\\\\\\\\")
    for character in ('"', "`", "$"):
        value = value.replace(character, "\\" + character)
    return f'"{value}"'


def main():
    engine = shutil.which("godot")
    if engine is None:
        raise SystemExit("Godot must be installed before registering the launcher")
    data = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
    applications = data / "applications"
    icons = data / "icons/hicolor/1024x1024/apps"
    applications.mkdir(parents=True, exist_ok=True)
    icons.mkdir(parents=True, exist_ok=True)
    icon = icons / "salt-line.png"
    shutil.copyfile(ROOT / "art/icon_1024.png", icon)
    # Godot's Wayland app ID is the project name; its X11 WM_CLASS is also
    # "Salt Line". The filename and StartupWMClass cover both backends.
    launcher = applications / "Salt Line.desktop"
    launcher.write_text(
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Name=Salt Line\n"
        "Comment=Artillery roguelike — local development build\n"
        f"Exec={exec_argument(engine)} --path {exec_argument(ROOT)}\n"
        f"Icon={icon}\n"
        "Terminal=false\n"
        "Categories=Game;\n"
        "StartupNotify=true\n"
        "StartupWMClass=Salt Line\n",
        encoding="utf-8",
    )
    updater = shutil.which("update-desktop-database")
    if updater:
        subprocess.run([updater, str(applications)], check=True)
    print(launcher)


if __name__ == "__main__":
    main()

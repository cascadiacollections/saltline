Original supplied Saltline icon artwork: default, Halloween, Thanksgiving,
and Christmas. These sources are excluded from the gameplay resource pack.

uv run python tools/build_app_icons.py
uv run python tools/build_app_icons.py --variant halloween

Run before Godot import/export. The selected image becomes art/icon_1024.png,
used by the project, macOS, both Windows presets, Android and iOS exports.
The standard builds use default; seasonal builds are selected explicitly,
never by the build machine's date. Restore default before committing.

#!/usr/bin/env bash
set -euo pipefail

# Check a Windows release zip the way a player gets it, for example
#   bash tools/check_windows_zip.sh dist/SaltLine-windows-x86_64.zip SaltLine.exe x86_64 build
#   bash tools/check_windows_zip.sh dist/SaltLine-windows-arm64.zip SaltLine-arm64.exe arm64 build
# Run from Git Bash on a Windows runner of the same architecture.
#
# The export writes the GDExtension (salt_line_godot.dll) beside the .exe, and
# the .exe cannot load the game without it. The smoke launch runs from the
# checkout, where the editor-side target/ paths still resolve, so it passed
# while the zip held the .exe alone. This unpacks the zip into an empty
# directory, checks it holds everything the export wrote to the build
# directory, checks every PE in it is built for the wanted architecture (the
# Windows counterpart of the macOS `lipo -verify_arch`), and launches the game
# from there until it renders a frame.
if [ $# -ne 4 ]; then
    echo "usage: $0 ZIP EXE x86_64|arm64 BUILD_DIR" >&2
    exit 2
fi
zip=$1
exe=$2
arch=$3
build=$4

case "$arch" in
    x86_64) want=8664 ;;  # IMAGE_FILE_MACHINE_AMD64
    arm64) want=aa64 ;;   # IMAGE_FILE_MACHINE_ARM64
    *) echo "unknown architecture $arch" >&2; exit 2 ;;
esac

zip=$(cd "$(dirname "$zip")" && pwd)/$(basename "$zip")
build=$(cd "$build" && pwd)
dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/unpacked-windows"
rm -rf "$dir"
mkdir -p "$dir"
7z x -y "$zip" -o"$dir" > /dev/null
cd "$dir"
ls -l

# Everything the export wrote (other than our checksum) has to be in the zip:
# that catches a library or a .pck added to the export and left out of the zip.
missing=0
for f in "$build"/*; do
    name=$(basename "$f")
    case "$name" in *.sha256) continue ;; esac
    if [ ! -e "$name" ]; then
        echo "$(basename "$zip") is missing $name, which the export wrote" >&2
        missing=1
    fi
done
for f in "$exe" salt_line_godot.dll; do
    if [ ! -f "$f" ]; then
        echo "$(basename "$zip") is missing $f" >&2
        missing=1
    fi
done
if [ "$missing" -ne 0 ]; then exit 1; fi

# The PE machine field: a little-endian u16 four bytes past the "PE\0\0"
# signature, which sits at the offset stored at 0x3c.
for f in ./*.exe ./*.dll; do
    [ -f "$f" ] || continue
    pe=$(od -An -tu4 -j60 -N4 "$f" | tr -d ' ')
    sig=$(od -An -tx1 -j"$pe" -N4 "$f" | tr -d ' \n')
    if [ "$sig" != "50450000" ]; then
        echo "$f is not a PE image" >&2
        exit 1
    fi
    machine=$(od -An -tx1 -j$((pe + 4)) -N2 "$f" | tr -d ' \n')
    machine=${machine:2:2}${machine:0:2}
    echo "$f: PE machine 0x$machine"
    if command -v file > /dev/null; then file -b "$f"; fi
    if [ "$machine" != "$want" ]; then
        echo "$f is not an $arch PE (machine 0x$machine, want 0x$want)" >&2
        exit 1
    fi
done

# Same launch as the smoke step: a Godot-exported .exe is GUI-subsystem and
# returns 0 at once, so wait on the rendered PNG, not the exit code. The game
# wants the shot path absolute and in Windows form (D:/...).
shot=$(cygpath -m "$dir/smoke.png" 2>/dev/null || echo "$dir/smoke.png")
"./$exe" --headless -- \
    --salt-seed 20260907 --salt-screen 12 \
    --salt-shot "$shot" --salt-shot-at 0.5 &
launcher=$!
for _ in $(seq 1 60); do
    if [ -s smoke.png ]; then break; fi
    if ! kill -0 "$launcher" 2>/dev/null; then break; fi
    sleep 1
done
kill "$launcher" 2>/dev/null || true
sleep 2
kill -9 "$launcher" 2>/dev/null || true
wait "$launcher" 2>/dev/null || true
if [ ! -s smoke.png ] || [ "$(head -c 4 smoke.png | od -An -tx1 | tr -d ' \n')" != "89504e47" ]; then
    echo "the unpacked $(basename "$zip") never rendered a frame" >&2
    exit 1
fi
echo "unpacked $(basename "$zip") launched and rendered $(wc -c < smoke.png) bytes"

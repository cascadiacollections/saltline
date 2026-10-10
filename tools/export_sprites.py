#!/usr/bin/env python3
"""Export art/aseprite/*.aseprite to art/sprites/*.png and validate them.

    uv run python tools/export_sprites.py          # export + validate
    uv run python tools/export_sprites.py --check  # export to a temp dir, fail if
                                                   # committed PNGs differ or break rules

Each source maps 1:1 to art/sprites/<name>.png. Multi-frame sources are packed
into a horizontal strip, which is the layout the Godot layer slices
(DESIGN §5.3). Every exported pixel must be fully opaque or fully transparent
and drawn from the 16-colour palette in art/palette_16.png (DESIGN §5).
"""
import argparse
import os
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path

from PIL import Image


def pixels(img):
    return img.get_flattened_data() if hasattr(img, "get_flattened_data") else img.getdata()


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "art", "aseprite")
DST = os.path.join(ROOT, "art", "sprites")
ASEPRITE = os.environ.get("ASEPRITE") or shutil.which("aseprite")

# Fixed canvas sizes consumed by the runtime; canvas/pivot changes need code changes.
SIZES = {name: (30, 12) for name in ("tank_base", "tank_ballast", "tank_forced_march")}
for name, intact, leaning, collapsed in (
    ("derrick", (15, 42), (31, 42), (15, 10)),
    ("radio_mast", (13, 32), (25, 32), (13, 8)),
    ("silo", (11, 27), (21, 27), (11, 6)),
    ("pylon", (17, 33), (29, 33), (17, 8)),
):
    SIZES.update({name: intact, name + "_leaning": leaning, name + "_collapsed": collapsed})
for suffix in ("", "_halloween", "_thanksgiving", "_christmas"):
    for state in ("intact", "leaning", "collapsed"):
        SIZES[f"tower{suffix}_{state}"] = (17, 8 if state == "collapsed" else 34)
for variant in ("5deg", "ballast", "forced_march", "sabot", "cluster", "auger"):
    SIZES["barrel_strip_" + variant] = (532, 28)


def palette():
    with Image.open(os.path.join(ROOT, "art", "palette_16.png")) as img:
        colours = {p[:3] for p in pixels(img.convert("RGBA")) if p[3] == 255}
    if len(colours) != 16:
        raise ValueError(f"palette_16.png: expected 16 colours, found {len(colours)}")
    return colours


def source_errors(path):
    with open(path, "rb") as source:
        header = source.read(14)
    if len(header) != 14:
        return ["truncated Aseprite header"]
    _, magic, frames, width, height, depth = struct.unpack("<I5H", header)
    if magic != 0xA5E0:
        return ["not an Aseprite file; fetch LFS sources with `git lfs pull`"]
    name = Path(path).stem
    barrel = name.startswith("barrel_strip_")
    expected = (19, 28, 28) if barrel else (1, *SIZES[name])
    errors = []
    if (frames, width, height) != expected:
        errors.append(f"expected frames/width/height {expected}, got {(frames, width, height)}")
    if depth != 32:
        errors.append("expected RGBA source (32-bit colour mode)")
    return errors


def export(src, dst):
    subprocess.run(
        [ASEPRITE, "-b", src, "--sheet", dst, "--sheet-type", "horizontal"],
        check=True,
        stdout=subprocess.PIPE,
    )


def validate(path, pal):
    errs = set()
    with Image.open(path) as image:
        name = Path(path).stem
        if image.size != SIZES[name]:
            errs.add(f"expected canvas {SIZES[name]}, got {image.size}")
        rgba = image.convert("RGBA")
        for r, g, b, a in pixels(rgba):
            if a not in (0, 255):
                errs.add("partial alpha")
            elif a == 255 and (r, g, b) not in pal:
                errs.add(f"off-palette colour {(r, g, b)}")
        if name.startswith("barrel_strip_") and image.size == (532, 28):
            for frame in range(19):
                x = frame * 28
                edge = [(x + i, y) for i in range(28) for y in (0, 27)]
                edge += [(x + i, y) for i in (0, 27) for y in range(28)]
                if any(rgba.getpixel(point)[3] for point in edge):
                    errs.add(f"barrel frame {frame}: opaque edge pixels risk clipping")
    return sorted(errs)


def same_pixels(first, second):
    with Image.open(first) as a, Image.open(second) as b:
        return a.size == b.size and a.convert("RGBA").tobytes() == b.convert("RGBA").tobytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="validate without writing PNGs")
    check = parser.parse_args().check
    if not ASEPRITE:
        parser.error("Aseprite CLI not found; install it or set ASEPRITE=/path/to/aseprite")
    names = {p.stem for p in Path(SRC).glob("*.aseprite")}
    if names != SIZES.keys():
        parser.error(f"source set differs: missing={sorted(SIZES.keys() - names)}, "
                     f"unexpected={sorted(names - SIZES.keys())}")
    pal = palette()
    failed = False
    with tempfile.TemporaryDirectory() as out_dir:
        for stem in sorted(names):
            src = os.path.join(SRC, stem + ".aseprite")
            out = os.path.join(out_dir, stem + ".png")
            errors = source_errors(src)
            if not errors:
                export(src, out)
                errors = validate(out, pal)
                committed = os.path.join(DST, stem + ".png")
                if check and (not os.path.isfile(committed) or not same_pixels(out, committed)):
                    errors.append("PNG is missing or stale; run `just export-sprites`")
            for error in errors:
                print(f"FAIL {stem}: {error}")
                failed = True
        if not failed and not check:
            os.makedirs(DST, exist_ok=True)
            for stem in sorted(names):
                out = os.path.join(out_dir, stem + ".png")
                dst = os.path.join(DST, stem + ".png")
                if not os.path.isfile(dst) or not same_pixels(out, dst):
                    shutil.copyfile(out, dst)
    print("FAILED" if failed else "ok")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

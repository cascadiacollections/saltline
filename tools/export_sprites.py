#!/usr/bin/env python3
"""Export native sprites, 4x artwork and backgrounds from art/aseprite/.

    uv run python tools/export_sprites.py          # export + validate
    uv run python tools/export_sprites.py --check  # export to a temp dir, fail if
                                                   # committed PNGs differ or break rules

Each source maps 1:1 to art/sprites/<name>.png. Multi-frame sources are packed
into a horizontal strip, which is the layout the Godot layer slices
(DESIGN §5.3). Every exported pixel must be fully opaque or fully transparent
and drawn from the 16-colour palette in art/palette_16.png (DESIGN §5).
Detailed companions preserve full RGB colour within the same logical bounds.
Embedded sky/soil bytes are regenerated with their PNGs.
"""
import argparse
import json
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

NATIVE_CONTRACTS = Path(ROOT) / "art/source/salt-and-iron/native-contracts.json"
DETAIL_SIZES = {
    name: tuple(value * 4 for value in size)
    for name, size in json.loads(NATIVE_CONTRACTS.read_text()).items()
}
DETAIL_SIZES.update({"title": (1280, 720), "sky": (1280, 720),
                     "sand": (64, 64), "rock": (64, 64), "bedrock": (64, 64)})
SCREEN_SIZES = {"title_background_320x180": (320, 180),
                "salt_iron_sky_320x180": (320, 180)}


def contracts():
    return [
        ("", Path(DST), SIZES, True),
        ("detail", Path(ROOT) / "art/detail", DETAIL_SIZES, False),
        ("screens", Path(ROOT) / "art", {"title_background_320x180": SCREEN_SIZES["title_background_320x180"]}, True),
        ("backgrounds", Path(ROOT) / "art/backgrounds", {"salt_iron_sky_320x180": SCREEN_SIZES["salt_iron_sky_320x180"]}, True),
    ]


def palette():
    with Image.open(os.path.join(ROOT, "art", "palette_16.png")) as img:
        colours = {p[:3] for p in pixels(img.convert("RGBA")) if p[3] == 255}
    if len(colours) != 16:
        raise ValueError(f"palette_16.png: expected 16 colours, found {len(colours)}")
    return colours


def source_errors(path, size=None):
    with open(path, "rb") as source:
        header = source.read(14)
    if len(header) != 14:
        return ["truncated Aseprite header"]
    _, magic, frames, width, height, depth = struct.unpack("<I5H", header)
    if magic != 0xA5E0:
        return ["not an Aseprite file; fetch LFS sources with `git lfs pull`"]
    name = Path(path).stem
    barrel = name.startswith("barrel_strip_")
    size = size or SIZES[name]
    expected = (19, size[1], size[1]) if barrel else (1, *size)
    errors = []
    if (frames, width, height) != expected:
        errors.append(f"expected frames/width/height {expected}, got {(frames, width, height)}")
    if depth != 32:
        errors.append("expected RGBA source (32-bit colour mode)")
    return errors


def export(src, dst):
    if ASEPRITE is None:
        raise RuntimeError("Aseprite CLI not found; install it or set ASEPRITE=/path/to/aseprite")
    subprocess.run(
        [ASEPRITE, "-b", src, "--sheet", dst, "--sheet-type", "horizontal"],
        check=True,
        stdout=subprocess.PIPE,
    )


def validate(path, pal, size=None):
    errs = set()
    with Image.open(path) as image:
        name = Path(path).stem
        size = size or SIZES[name]
        if image.size != size:
            errs.add(f"expected canvas {size}, got {image.size}")
        rgba = image.convert("RGBA")
        for r, g, b, a in pixels(rgba):
            if a not in (0, 255):
                errs.add("partial alpha")
            elif a != 255 and name in ("sky", "title", "sand", "rock", "bedrock",
                                        "title_background_320x180", "salt_iron_sky_320x180"):
                errs.add("background/terrain texture must be opaque")
            elif a == 255 and pal is not None and (r, g, b) not in pal:
                errs.add(f"off-palette colour {(r, g, b)}")
        if name.startswith("barrel_strip_") and image.size == size:
            width = size[1]
            margin = width // 28
            alpha = rgba.getchannel("A")
            for frame in range(19):
                x = frame * width
                borders = list(range(margin)) + list(range(width - margin, width))
                edge = [(x + i, y) for i in range(width) for y in borders]
                edge += [(x + i, y) for i in borders for y in range(width)]
                if any(alpha.getpixel(point) for point in edge):
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
    groups = contracts()
    for folder, _, sizes, _ in groups:
        names = {p.stem for p in (Path(SRC) / folder).glob("*.aseprite")}
        if names != sizes.keys():
            parser.error(f"{folder or 'native'} source set differs: "
                         f"missing={sorted(sizes.keys() - names)}, unexpected={sorted(names - sizes.keys())}")
    pal = palette()
    failed = False
    with tempfile.TemporaryDirectory() as out_dir:
        outputs = []
        for folder, destination, sizes, native in groups:
            staging = Path(out_dir) / folder
            staging.mkdir(parents=True, exist_ok=True)
            for stem, size in sorted(sizes.items()):
                src = Path(SRC) / folder / (stem + ".aseprite")
                out = staging / (stem + ".png")
                committed = destination / out.name
                errors = source_errors(src, size)
                if not errors:
                    export(str(src), str(out))
                    errors = validate(out, pal if native else None, size)
                    if check and (not committed.is_file() or not same_pixels(out, committed)):
                        errors.append("PNG is missing or stale; run `just export-sprites`")
                for error in errors:
                    print(f"FAIL {folder}/{stem}: {error}")
                    failed = True
                outputs.append((out, committed))
        if not failed:
            from gen_sprites import P
            embedded = {}
            for name in ("sky", "sand", "rock", "bedrock"):
                with Image.open(Path(out_dir) / "detail" / (name + ".png")) as image:
                    embedded[name] = image.convert("RGBA").tobytes()
            indices = {colour: index for index, colour in enumerate(P.values())}
            with Image.open(Path(out_dir) / "backgrounds/salt_iron_sky_320x180.png") as image:
                native_sky = bytes(indices[p[:3]] for p in pixels(image.convert("RGBA")))
            raw_outputs = {
                Path(ROOT) / "art/detail/sky.rgba": embedded["sky"],
                Path(ROOT) / "art/detail/soil.rgba": b"".join(embedded[name] for name in ("sand", "rock", "bedrock")),
                Path(ROOT) / "art/backgrounds/salt_iron_sky_320x180.pal8": native_sky,
            }
            for path, data in raw_outputs.items():
                if check and (not path.is_file() or path.read_bytes() != data):
                    print(f"FAIL {path.name}: embedded texture is stale; run `just export-sprites`")
                    failed = True
        if not failed and not check:
            for out, dst in outputs:
                dst.parent.mkdir(parents=True, exist_ok=True)
                if not dst.is_file() or not same_pixels(out, dst):
                    shutil.copyfile(out, dst)
            for path, data in raw_outputs.items():
                if not path.is_file() or path.read_bytes() != data:
                    path.write_bytes(data)
    print("FAILED" if failed else "ok")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

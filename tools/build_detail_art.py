#!/usr/bin/env python3
"""Preserve the board's source colors at 4 pixels per logical world unit.

Source cutouts, not enlarged native sprites. Logical bounds and pivots match
native-contracts.json; only presentation sampling changes.
"""

import json
import math
from pathlib import Path
from PIL import Image
import import_concept_art as native

ROOT = native.ROOT
OUT = ROOT / "art/detail"
S = 4


def fit(source, size, bounds=None):
    image = Image.new("RGBA", (size[0] * S, size[1] * S))
    x, y, w, h = bounds or (0, 0, *size)
    source = source.crop(source.getbbox())
    image.paste(source.resize((w * S, h * S), Image.Resampling.LANCZOS), (x * S, y * S))
    return binary(image)


def binary(image):
    image = image.convert("RGBA")
    image.putalpha(image.getchannel("A").point(lambda a: 255 if a >= 128 else 0))
    return image


def build(out=OUT):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    board = Image.open(native.SOURCE / "01-salt-and-iron.png").convert("RGBA")
    products = {}
    guns = [
        (
            "tank_base",
            (45, 714, 220, 856),
            (134, 775),
            (15, 3),
            2,
            27,
            [
                (90, 714),
                (97, 714),
                (97, 748),
                (145, 748),
                (160, 795),
                (192, 800),
                (220, 821),
                (220, 856),
                (45, 856),
                (45, 820),
                (66, 800),
                (66, 757),
                (90, 757),
            ],
        ),
        (
            "tank_ballast",
            (299, 700, 493, 857),
            (383, 773),
            (13, 4),
            0,
            29,
            [
                (346, 700),
                (355, 700),
                (355, 741),
                (411, 741),
                (435, 797),
                (464, 803),
                (493, 824),
                (493, 857),
                (299, 857),
                (299, 821),
                (327, 797),
                (327, 749),
                (346, 749),
            ],
        ),
        (
            "tank_forced_march",
            (572, 741, 771, 856),
            (670, 767),
            (15, 3),
            2,
            27,
            [
                (619, 741),
                (666, 741),
                (691, 792),
                (738, 800),
                (771, 824),
                (771, 856),
                (572, 856),
                (572, 817),
                (599, 798),
                (615, 794),
                (615, 750),
            ],
        ),
    ]
    for name, box, mount, pivot, left, right, poly in guns:
        source = native.cutout(board, box, solid=True, polygon=poly)
        image = Image.new("RGBA", (30 * S, 12 * S))
        for y in range(12 * S):
            sy = round(
                native.mapped(y / S, 11, pivot[1], mount[1] - box[1], source.height - 1)
            )
            for x in range(left * S, (right + 1) * S):
                sx = round(
                    native.mapped(
                        x / S - left,
                        right - left,
                        pivot[0] - left,
                        mount[0] - box[0],
                        source.width - 1,
                    )
                )
                image.putpixel(
                    (x, y),
                    source.getpixel(
                        (min(source.width - 1, sx), min(source.height - 1, sy))
                    ),
                )
        products[name] = image
    for name, box, damaged in [
        ("tower_intact", (922, 686, 1029, 855), False),
        ("tower_leaning", (1094, 686, 1201, 855), True),
    ]:
        source = native.cutout(board, box)
        source = source.crop(source.getbbox())
        image = Image.new("RGBA", (17 * S, 34 * S))
        # Preserve the physical silhouette and ground contact, but sample the
        # original trestles at display resolution instead of discarding them.
        for y in range(S, 33 * S):
            sy = min(source.height - 1, round((y / S - 1) * (source.height - 1) / 31))
            width = 13 if y < 18 * S else 9
            shift = min(2, max(0, (32 - y / S) / 9)) if damaged else 0
            row = source.crop((0, sy, source.width, sy + 1)).resize(
                (width * S, 1), Image.Resampling.LANCZOS
            )
            image.paste(row, (round(((17 - width) / 2 + shift) * S), y))
        products[name] = binary(image)
    products["tower_collapsed"] = fit(
        native.cutout(board, (1251, 752, 1455, 858)), (17, 8)
    )
    contracts = json.loads((native.SOURCE / "native-contracts.json").read_text())
    for name, box, size, ground in [
        ("derrick", (1511, 190, 1557, 422), (15, 42), 41),
        ("radio_mast", (971, 191, 1050, 422), (13, 32), 31),
        ("pylon", (1511, 190, 1557, 422), (17, 33), 32),
        ("silo", (931, 691, 1020, 770), (11, 27), 27),
    ]:
        image = fit(
            native.cutout(board, box, foreground=name != "silo"),
            size,
            (1, 0, size[0] - 2, ground),
        )
        products[name] = image
        w, h = contracts[name + "_leaning"]
        leaned = Image.new("RGBA", (w * S, h * S))
        pad = (w - size[0]) * S // 2
        for y in range(h * S):
            shift = (
                0 if y >= (h - 4) * S else round(pad * (h * S - 1 - y) / (h * S - 1))
            )
            leaned.paste(image.crop((0, y, image.width, y + 1)), (pad + shift, y))
        products[name + "_leaning"] = leaned
        # Wreckage uses actual board fragments; damage bounds stay unchanged.
        products[name + "_collapsed"] = fit(
            native.cutout(board, (1251, 752, 1455, 858)), contracts[name + "_collapsed"]
        )
    tube = Image.open(native.SOURCE / "extracted-barrel.png").convert("RGBA")
    tube = binary(tube)
    tube = tube.crop(tube.getbbox())
    for name, length, thickness in [
        ("barrel_strip_5deg", 16, 3),
        ("barrel_strip_ballast", 18, 4),
        ("barrel_strip_forced_march", 14, 3),
        ("barrel_strip_sabot", 22, 2),
        ("barrel_strip_cluster", 9, 5),
        ("barrel_strip_auger", 16, 3),
    ]:
        strip = Image.new("RGBA", (28 * 19 * S, 28 * S))
        source = tube.resize(
            ((length + 1) * S, thickness * S), Image.Resampling.LANCZOS
        )
        for frame, angle in enumerate(range(0, 95, 5)):
            co, si = math.cos(math.radians(angle)), math.sin(math.radians(angle))
            for y in range(S, 27 * S):
                for x in range(S, 27 * S):
                    dx, dy = x - 3 * S, y - 24 * S
                    u = round(dx * co - dy * si)
                    v = round(dx * si + dy * co + (thickness * S - 1) / 2)
                    if 0 <= u < source.width and 0 <= v < source.height:
                        strip.putpixel((frame * 28 * S + x, y), source.getpixel((u, v)))
        products[name] = binary(strip)
    for name, image in products.items():
        assert image.size == tuple(v * S for v in contracts[name]), (
            name,
            image.size,
            contracts[name],
        )
        binary(image).save(out / f"{name}.png")
    plate = Image.open(native.SOURCE / "clean-background.png").convert("RGBA")
    plate = native.sky_background(plate, (320 * S, 180 * S), Image.Resampling.LANCZOS)
    textures = []
    for name, box in [
        ("sand", (560, 416, 568, 424)),
        ("rock", (560, 450, 624, 514)),
        ("bedrock", (520, 560, 584, 624)),
    ]:
        tile = board.crop(box).resize((64, 64), Image.Resampling.NEAREST)
        tile.save(out / f"{name}.png")
        textures.append(tile.tobytes())
    (out / "soil.rgba").write_bytes(b"".join(textures))
    plate.save(out / "sky.png")
    (out / "sky.rgba").write_bytes(plate.tobytes())
    native.title_background(board, (320 * S, 180 * S), Image.Resampling.LANCZOS).save(
        out / "title.png"
    )
    build_preview(out)
    return products


def build_preview(out=OUT):
    out = Path(out)
    sheet = Image.new("RGBA", (1280, 400), (25, 32, 39, 255))
    for i, name in enumerate(
        [
            "tank_base",
            "tank_ballast",
            "tank_forced_march",
            "tower_intact",
            "tower_leaning",
            "tower_collapsed",
            "derrick",
            "radio_mast",
            "silo",
            "pylon",
        ]
    ):
        with Image.open(out / f"{name}.png") as source:
            image = source.convert("RGBA")
            sheet.alpha_composite(image, (i * 124, 188 - image.height))
    with Image.open(out / "barrel_strip_5deg.png") as source:
        sheet.alpha_composite(source.convert("RGBA").crop((0, 0, 1280, 112)), (0, 240))
    generated = native.generated
    generated.save_preview(sheet, str(out / "preview.png"))


if __name__ == "__main__":
    build()

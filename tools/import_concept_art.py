#!/usr/bin/env python3
"""Convert the Salt & Iron board into the existing game art contracts.

All sampling, background removal, palette mapping and barrel rotation happen
offline. No change to simulation geometry, native sprite sizes or muzzle pivots.
The original board is the source for chassis and structures; ImageGen cleaned
the harbour plate and isolated the barrel (see art/source/salt-and-iron/README.md).
"""

from functools import lru_cache
import argparse
import json
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

import gen_sprites as generated

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "art/source/salt-and-iron"
PALETTE = list(generated.P.values())


@lru_cache(maxsize=None)
def colour(rgb, candidates=tuple(range(16))):
    return min(
        candidates, key=lambda i: sum((a - b) ** 2 for a, b in zip(rgb, PALETTE[i]))
    )


def quantize(image, hull=False, sky=False):
    def index(pixel):
        r, g, b = pixel[:3]
        if sky:
            # Amber is reserved for gameplay/versus ink. Background lights
            # use the pale rust highlight, blue scenery only the sky ramp.
            candidates = (11, 13) if r > b and r > 120 else (4, 5, 6)
            return colour(pixel[:3], candidates)
        if hull:
            if r > g * 1.35 and r > b * 1.5:
                return colour(pixel[:3], (10, 11, 12))
            if max(r, g, b) > 170 and max(r, g, b) - min(r, g, b) < 80:
                return colour(pixel[:3], (0, 1, 13))
            if max(r, g, b) < 45:
                return colour(pixel[:3], (9, 14))
            return colour(pixel[:3], (7, 8, 9))
        return colour(pixel[:3], tuple(range(15)))

    result = Image.new("RGBA", image.size)
    result.putdata(
        [
            (*PALETTE[index(p)], 255) if p[3] >= 128 else (0, 0, 0, 0)
            for p in image.convert("RGBA").get_flattened_data()
        ]
    )
    return result


def cutout(board, box, foreground=False, solid=False, polygon=None):
    image = board.crop(box).convert("RGBA")
    mask = Image.new("L", image.size)
    for y in range(image.height):
        for x in range(image.width):
            r, g, b, _ = image.getpixel((x, y))
            if foreground:
                keep = r > b + 15 and r > g
            else:
                # The showcase backdrop is near-neutral charcoal. Preserve
                # warm rust, olive armour and pale metal, including dark rust.
                keep = max(r, g, b) > 55 or r - b > 18 or g - b > 15
            if keep:
                mask.putpixel((x, y), 255)
    if polygon:
        clip = Image.new("L", image.size)
        ImageDraw.Draw(clip).polygon(
            [(x - box[0], y - box[1]) for x, y in polygon], fill=255
        )
        mask.putdata(
            [
                min(a, b)
                for a, b in zip(mask.get_flattened_data(), clip.get_flattened_data())
            ]
        )
    if solid:
        # Keep dark enclosed wheel/armour pixels, while external background
        # remains transparent. Towers deliberately retain their open trestles.
        padded = Image.new("L", (image.width + 2, image.height + 2))
        padded.paste(mask, (1, 1))
        ImageDraw.floodfill(padded, (0, 0), 128)
        mask = padded.crop((1, 1, image.width + 1, image.height + 1))
        mask.putdata([0 if p == 128 else 255 for p in mask.get_flattened_data()])
    image.putalpha(mask)
    return image


def mapped(t, end, pivot, source_pivot, source_end):
    """Piecewise nearest sampling: preserve the declared anchor exactly."""
    if t <= pivot:
        return t * source_pivot / max(pivot, 1)
    return source_pivot + (t - pivot) * (source_end - source_pivot) / max(
        end - pivot, 1
    )


def chassis(source, size, pivot, source_pivot, left, right):
    image = Image.new("RGBA", size)
    for y in range(size[1]):
        sy = round(mapped(y, size[1] - 1, pivot[1], source_pivot[1], source.height - 1))
        for x in range(left, right + 1):
            sx = round(
                mapped(
                    x - left,
                    right - left,
                    pivot[0] - left,
                    source_pivot[0],
                    source.width - 1,
                )
            )
            image.putpixel((x, y), source.getpixel((sx, sy)))
    return quantize(image, hull=True)


def fit(source, size, bounds=None):
    source = source.crop(source.getbbox())
    image = Image.new("RGBA", size)
    x, y, w, h = bounds or (0, 0, *size)
    image.paste(source.resize((w, h), Image.Resampling.NEAREST), (x, y))
    return quantize(image)


def tower(source, damaged=False):
    source = source.crop(source.getbbox())
    image = Image.new("RGBA", (17, 34))
    # The board's tank occupies the upper half; narrow the lower trestle to
    # the existing 9px footprint. Row 32 is the ground contact in the renderer.
    for y in range(1, 33):
        sy = round((y - 1) * (source.height - 1) / 31)
        width = 13 if y < 18 else 9
        shift = min(2, (32 - y) // 9) if damaged else 0
        for x in range(width):
            sx = round(x * (source.width - 1) / (width - 1))
            image.putpixel(
                ((17 - width) // 2 + x + shift, y), source.getpixel((sx, sy))
            )
    return quantize(image)


def barrel_strip(source, length, thickness, variant):
    # Inverse sampling avoids holes in oblique frames. Runtime only blits the
    # same nineteen 28px frames as before, at the same (3,24) mounting point.
    tube = source.resize((length + 1, thickness), Image.Resampling.NEAREST)
    strip = Image.new("RGBA", (28 * 19, 28))
    for frame, angle in enumerate(range(0, 95, 5)):
        theta = math.radians(angle)
        co, si = math.cos(theta), math.sin(theta)
        for y in range(1, 27):
            for x in range(1, 27):
                dx, dy = x - 3, y - 24
                u = round(dx * co - dy * si)
                v = round(dx * si + dy * co + (thickness - 1) / 2)
                if 0 <= u <= length and 0 <= v < thickness:
                    strip.putpixel((frame * 28 + x, y), tube.getpixel((u, v)))
        if variant == "auger":
            x, y = round(3 + co * length), round(24 - si * length)
            ImageDraw.Draw(strip).rectangle(
                (frame * 28 + x - 1, y - 1, frame * 28 + x + 1, y + 1),
                fill=(*generated.P["hull_hi"], 255),
            )
    return quantize(strip, hull=True)


def title_scene(board):
    """Remove lettering and its dark outline using the source sky's own ramp.

    Expand the warm/bright letter mask to cover the extruded dark edges, then
    interpolate untouched sky endpoints on each row. The rest of the hero scene
    remains the original board; no differently lit cleanup rectangle is pasted.
    """
    scene = board.crop((0, 0, board.width, 626)).convert("RGBA")
    patch = scene.crop((0, 0, 440, 130))
    mask = Image.new("L", patch.size)
    mask.putdata(
        [255 if p[0] > p[2] or p[0] > 60 else 0 for p in patch.get_flattened_data()]
    )
    mask = mask.filter(ImageFilter.MaxFilter(25))
    for y in range(130):
        left = board.getpixel((0, y))
        right = board.getpixel((440, y))
        for x in range(440):
            if mask.getpixel((x, y)):
                f = x / 440
                scene.putpixel(
                    (x, y),
                    tuple(round(a * (1 - f) + b * f) for a, b in zip(left, right)),
                )
    return scene


def title_background(board, size, resample=Image.Resampling.NEAREST):
    """Fit the complete wide hero scene by width, preserving its proportions.

    Anchor its terrain at the bottom and extend the scene's own top sky row
    above it. Keeping the full width retains the gun and both outer towers.
    """
    scene = title_scene(board)
    height = round(size[0] * scene.height / scene.width)
    if height > size[1]:
        raise ValueError("title target must be taller than the hero scene")
    scene = scene.resize((size[0], height), resample)
    image = Image.new("RGBA", size)
    top = size[1] - height
    if top:
        image.paste(scene.crop((0, 0, size[0], 1)).resize((size[0], top)), (0, 0))
    image.paste(scene, (0, top))
    return image


def build_title(out=None):
    """The actual board's hero scene, with its baked-in title removed."""
    out = Path(out or generated.OUT)
    board = Image.open(SOURCE / "01-salt-and-iron.png").convert("RGBA")
    quantize(title_background(board, (320, 180))).save(
        out / "title_background_320x180.png"
    )


def sky_background(plate, size, resample=Image.Resampling.NEAREST):
    """Scale the harbour uniformly and place its horizon below the aim area.

    Keep the source's 40%-height horizon near 60% of the gameplay frame.
    Extend only empty sky above it and crop excess water below the frame.
    """
    height = round(size[0] * plate.height / plate.width)
    plate = plate.resize((size[0], height), resample)
    top = round(size[1] * 0.6 - height * 0.4)
    image = Image.new("RGBA", size)
    if top > 0:
        image.paste(plate.crop((0, 0, size[0], 1)).resize((size[0], top)), (0, 0))
    image.paste(plate, (0, top))
    return image


def build(out=None):
    out = Path(out or generated.OUT)
    sprites = out / "sprites"
    backgrounds = out / "backgrounds"
    sprites.mkdir(parents=True, exist_ok=True)
    backgrounds.mkdir(parents=True, exist_ok=True)
    board = Image.open(SOURCE / "01-salt-and-iron.png").convert("RGBA")
    products = {}
    # Original board coordinates, excluding ground and detachable barrels.
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
    for name, box, mount, pivot, left, right, polygon in guns:
        source = cutout(board, box, solid=True, polygon=polygon)
        products[name] = chassis(
            source, (30, 12), pivot, (mount[0] - box[0], mount[1] - box[1]), left, right
        )
    products["tower_intact"] = tower(cutout(board, (922, 686, 1029, 855)))
    products["tower_leaning"] = tower(
        cutout(board, (1094, 686, 1201, 855)), damaged=True
    )
    products["tower_collapsed"] = fit(cutout(board, (1251, 752, 1455, 858)), (17, 8))
    # Foreground ironwork from the board's main scene, stripped of blue sky.
    for name, box, size, ground, phase in [
        ("derrick", (1511, 190, 1557, 422), (15, 42), 41, 1.7),
        ("radio_mast", (971, 191, 1050, 422), (13, 32), 31, 2.3),
        ("pylon", (1511, 190, 1557, 422), (17, 33), 32, 2.9),
    ]:
        source = cutout(board, box, foreground=True)
        products[name] = fit(source, size, (1, 0, size[0] - 2, ground))
        native = generated.Buf(*size)
        native.px = [
            list(products[name].crop((0, y, size[0], y + 1)).get_flattened_data())
            for y in range(size[1])
        ]
        products[name + "_leaning"] = generated.leaning(native).image()
        products[name + "_collapsed"] = generated.collapsed(native, phase).image()
        generated.check_lean_alignment(name, native, generated.leaning(native))
    # Use the intact board tank as the existing short silo, without trestles.
    products["silo"] = fit(
        cutout(board, (931, 691, 1020, 770)), (11, 27), (1, 0, 9, 27)
    )
    silo = generated.Buf(11, 27)
    silo.px = [
        list(products["silo"].crop((0, y, 11, y + 1)).get_flattened_data())
        for y in range(27)
    ]
    products["silo_leaning"] = generated.leaning(silo).image()
    products["silo_collapsed"] = generated.collapsed(silo, 1.1).image()
    tube = Image.open(SOURCE / "extracted-barrel.png").convert("RGBA")
    alpha = tube.getchannel("A").point(lambda a: 255 if a >= 128 else 0)
    tube.putalpha(alpha)
    tube = tube.crop(tube.getbbox())
    for name, length, thickness, variant in [
        ("barrel_strip_5deg", 16, 3, "base"),
        ("barrel_strip_ballast", 18, 4, "ballast"),
        ("barrel_strip_forced_march", 14, 3, "forced"),
        ("barrel_strip_sabot", 22, 2, "sabot"),
        ("barrel_strip_cluster", 9, 5, "cluster"),
        ("barrel_strip_auger", 16, 3, "auger"),
    ]:
        products[name] = barrel_strip(tube, length, thickness, variant)
    for name, image in products.items():
        image.save(sprites / f"{name}.png")
    # Native sky's palette indices are embedded in Rust, so headless tests,
    # flat rendering and layered rendering use the actual imported artwork.
    plate = Image.open(SOURCE / "clean-background.png").convert("RGBA")
    sky = quantize(sky_background(plate, (320, 180)), sky=True)
    sky.save(backgrounds / "salt_iron_sky_320x180.png")
    indices = bytes(colour(p[:3]) for p in sky.get_flattened_data())
    (backgrounds / "salt_iron_sky_320x180.pal8").write_bytes(indices)
    build_title(out)
    contracts = {name: list(image.size) for name, image in sorted(products.items())}
    expected = json.loads((SOURCE / "native-contracts.json").read_text())
    assert contracts == expected, "concept art changed a native sprite contract"
    # A review sheet enlarges only offline, never in the game renderer.
    sheet = Image.new("RGBA", (320, 100), (*generated.P["ui_bg"], 255))
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
        image = products[name]
        sheet.alpha_composite(image, (i * 31, 47 - image.height))
    sheet.alpha_composite(products["barrel_strip_5deg"].crop((0, 0, 320, 28)), (0, 60))
    sheet.resize((1280, 400), Image.Resampling.NEAREST).save(
        out / "concept_assets_preview_4x.png"
    )
    return products


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--title-only", action="store_true")
    args = parser.parse_args()
    if args.title_only:
        build_title()
    else:
        build()
    print("Salt & Iron: original board artwork imported at native sizes and palette")

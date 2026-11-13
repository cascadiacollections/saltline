#!/usr/bin/env python3
"""Counterweight: the Siege era's sprites, drawn procedurally.

Medieval skins for the engine's five structure footprints, the trebuchet
frame, its throwing-arm strip and the stone. Every sprite uses the engine's
16-colour palette and the same native size contracts as the Present sprites
(art/source/salt-and-iron/native-contracts.json), so the sim's footprints,
heights and pivots are unchanged; only the drawing differs. Leaning and
collapsed states go through the engine's shared shear and rubble passes.

    uv run python gameoff/tools/gen_medieval.py      # writes gameoff/art/
"""
import json
import math
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "tools"))
import gen_sprites as g  # noqa: E402  (engine palette, Buf, damage passes)

OUT = os.path.join(ROOT, "gameoff", "art")
SPR = os.path.join(OUT, "sprites")
CONTRACTS = os.path.join(ROOT, "art/source/salt-and-iron/native-contracts.json")

Buf, c = g.Buf, g.rgba
WOOD, WOOD_HI, WOOD_DK = c("st"), c("st_hi"), c("st_dmg")
STONE, STONE_HI, STONE_DK = c("rock"), c("sand"), c("bedrock")
MORTAR, BANNER, IRON = c("shadow"), c("alert"), c("hull_edge")


def masonry(b, x, y, w, h):
    """Coursed stone: a fill with staggered mortar joints and a lit left edge."""
    b.rect(x, y, w, h, STONE)
    for j in range(y + 2, y + h, 3):
        b.rect(x, j, w, 1, MORTAR)
        off = 0 if ((j - y) // 3) % 2 else 2
        for i in range(x + off, x + w, 4):
            b.set(i, j - 1, MORTAR)
    b.rect(x, y, 1, h, STONE_HI)


def crenellate(b, x, y, w):
    for i in range(x, x + w, 2):
        b.rect(i, y, 1, 2, STONE)
    b.set(x, y, STONE_HI)


# ------------------------------------------------------------- structures
def watchtower():
    """Derrick slot, 15x42: timber lattice, a platform and a hide roof."""
    b = Buf(15, 42)
    for j in range(8, 42):
        t = (j - 8) / 34.0
        half = int(2 + t * 5)
        b.set(7 - half, j, WOOD)
        b.set(7 + half, j, WOOD)
    for j in range(12, 40, 7):
        half = int(2 + (j - 8) / 34.0 * 5)
        b.line(7 - half, j, 7 + half, j + 6, WOOD_DK)
        b.line(7 + half, j, 7 - half, j + 6, WOOD_DK)
    b.rect(2, 8, 11, 2, WOOD)
    b.rect(2, 8, 11, 1, WOOD_HI)
    b.rect(3, 4, 1, 4, WOOD_DK)
    b.rect(11, 4, 1, 4, WOOD_DK)
    for i in range(7):  # pitched roof
        b.rect(7 - i, 1 + i // 2, 1 + 2 * i, 1, WOOD_DK)
    b.set(7, 0, BANNER)
    return b


def banner_pole():
    """Radio-mast slot, 13x32: a lance pole on a stone plinth, pennant flying."""
    b = Buf(13, 32)
    b.rect(6, 2, 1, 26, WOOD_DK)
    b.set(6, 1, IRON)
    for j in range(5):  # swallow-tailed pennant
        b.rect(7, 3 + j, 6 - abs(2 - j), 1, BANNER)
    b.set(12, 5, c("ui_bg") if False else BANNER)
    b.set(11, 5, (0, 0, 0, 0))
    masonry(b, 3, 27, 7, 5)
    return b


def bell_tower():
    """Water-tower slot, 17x34: stone tower, arched belfry, spire."""
    b = Buf(17, 34)
    masonry(b, 3, 14, 11, 20)
    b.rect(2, 12, 13, 2, STONE_DK)
    b.rect(4, 6, 9, 6, STONE)
    b.rect(6, 7, 5, 5, MORTAR)  # belfry arch
    b.rect(7, 6, 3, 1, MORTAR)
    b.rect(8, 9, 1, 2, BANNER)  # the bell
    for i in range(6):  # spire
        b.rect(8 - i // 1 + 0, 0 + i, 1 + 2 * (i // 1), 1, WOOD_DK) if i < 5 else None
    b.rect(3, 5, 11, 1, WOOD_DK)
    b.rect(7, 22, 3, 5, MORTAR)  # door
    return b


def keep():
    """Silo slot, 11x27: a round stone keep, crenellated, arrow slits."""
    b = Buf(11, 27)
    masonry(b, 1, 4, 9, 23)
    crenellate(b, 1, 2, 9)
    b.rect(1, 4, 9, 1, STONE_DK)
    for y in (9, 16):
        b.rect(5, y, 1, 3, MORTAR)
    b.rect(4, 22, 3, 5, MORTAR)
    return b


def siege_tower():
    """Pylon slot, 17x33: tapering timber tower, hide panels, raised bridge."""
    b = Buf(17, 33)
    for j in range(3, 33):
        t = (j - 3) / 30.0
        half = int(3 + t * 5)
        b.rect(8 - half, j, half * 2 + 1, 1, WOOD)
        b.set(8 - half, j, WOOD_HI)
    for j in range(6, 31, 5):
        b.rect(2, j, 13, 1, WOOD_DK)
    b.rect(5, 9, 7, 8, c("hull"))  # hide panels
    b.rect(5, 20, 7, 6, c("hull"))
    b.line(12, 3, 16, 9, WOOD_DK)  # drawbridge, raised
    b.rect(5, 0, 7, 3, WOOD_DK)
    b.set(4, 31, IRON)
    b.set(12, 31, IRON)
    return b


STRUCTURES = {
    # engine sprite name -> (medieval name, drawing)
    "derrick": ("watchtower", watchtower),
    "radio_mast": ("banner_pole", banner_pole),
    "tower": ("bell_tower", bell_tower),
    "silo": ("keep", keep),
    "pylon": ("siege_tower", siege_tower),
}


# ------------------------------------------------------------- the gun
#
# The trebuchet does not reuse the howitzer's 30x12 chassis or its 28px barrel
# frames: an arm with a counterweight needs room on both sides of its pivot.
# The sim only cares where the muzzle is (MUZZLE_HEIGHT above the seated
# ground), so the presentation seats the frame on the ground and draws the
# arm frame with its pivot at the frame's axle; the stone leaves from the
# arm's tip.
FRAME_W, FRAME_H = 30, 22
AXLE = (15, 3)  # inside the frame
ARM_FRAME = 48
ARM_PIVOT = (24, 24)  # inside each arm frame
ARM_LONG, ARM_SHORT = 20, 7


def trebuchet_frame():
    """Wheeled base, A-frame uprights meeting at the axle, braces."""
    b = Buf(FRAME_W, FRAME_H)
    ax, ay = AXLE
    b.rect(1, FRAME_H - 5, 28, 2, WOOD)
    b.rect(1, FRAME_H - 5, 28, 1, WOOD_HI)
    for foot in (6, 24):
        b.line(foot, FRAME_H - 5, ax, ay, WOOD_DK, thick=2)
    b.line(9, FRAME_H - 11, 21, FRAME_H - 11, WOOD)  # cross brace
    b.rect(ax - 1, ay - 1, 3, 3, IRON)
    for x in (4, 25):
        b.rect(x - 2, FRAME_H - 4, 4, 4, WOOD_DK)
        b.set(x - 1, FRAME_H - 3, IRON)
    return b, AXLE


def arm(angle_deg):
    """One throwing-arm frame: the long arm points at `angle_deg` (the
    release angle the player dials), sling hanging from its tip, and the
    counterweight box hangs plumb from the short end."""
    b = Buf(ARM_FRAME, ARM_FRAME)
    px, py = ARM_PIVOT
    a = math.radians(angle_deg)
    tx, ty = round(px + math.cos(a) * ARM_LONG), round(py - math.sin(a) * ARM_LONG)
    sx, sy = round(px - math.cos(a) * ARM_SHORT), round(py + math.sin(a) * ARM_SHORT)
    b.line(px, py, tx, ty, WOOD_DK, thick=2)
    b.line(px, py, sx, sy, WOOD_DK, thick=2)
    # Sling: a short cord from the tip with the stone pouch.
    b.line(tx, ty, tx + 2, ty + 3, MORTAR)
    b.rect(tx + 2, ty + 3, 2, 2, STONE_DK)
    # Counterweight: hanger, then an iron-banded box.
    b.line(sx, sy, sx, sy + 3, IRON)
    b.rect(sx - 3, sy + 3, 7, 6, STONE_DK)
    b.rect(sx - 3, sy + 3, 7, 1, STONE)
    b.rect(sx - 3, sy + 6, 7, 1, IRON)
    b.rect(px - 1, py - 1, 3, 3, IRON)
    return b


def arm_strip():
    """Nineteen frames at 5-degree steps, matching the barrel strips' steps."""
    frames = [arm(a) for a in range(0, 95, 5)]
    b = Buf(ARM_FRAME * len(frames), ARM_FRAME)
    for k, f in enumerate(frames):
        b.blit(f, k * ARM_FRAME, 0)
    return b


def stone():
    b = Buf(3, 3)
    b.rect(0, 0, 3, 3, STONE_DK)
    b.set(0, 0, (0, 0, 0, 0))
    b.set(2, 2, (0, 0, 0, 0))
    b.set(1, 0, STONE)
    return b


# ------------------------------------------------------------- output
def build():
    """Every sprite, keyed by output name, with the engine contract it fills."""
    out = {}
    for engine, (name, draw) in STRUCTURES.items():
        src = draw()
        prefix = "tower" if engine == "tower" else engine
        intact_key = "tower_intact" if engine == "tower" else engine
        out[name] = (src, intact_key)
        out[name + "_leaning"] = (g.leaning(src), prefix + "_leaning")
        out[name + "_collapsed"] = (g.collapsed(src), prefix + "_collapsed")
    frame, _ = trebuchet_frame()
    out["trebuchet"] = (frame, None)
    out["trebuchet_arm_strip_5deg"] = (arm_strip(), None)
    out["stone"] = (stone(), None)
    return out


def check(sprites):
    with open(CONTRACTS) as f:
        contracts = json.load(f)
    bad = []
    for name, (b, key) in sprites.items():
        if key and name.endswith("_leaning"):
            # Heights hold; widths follow the shear rule (intact + 2m), which
            # keeps the base column fixed. The engine's hand-cut leaning
            # water tower is narrower than that rule, so its width can't apply.
            intact = sprites[name[: -len("_leaning")]][0]
            m = max(2, intact.h // 5)
            if [b.w, b.h] != [intact.w + 2 * m, contracts[key][1]]:
                bad.append(f"{name}: {b.w}x{b.h}, want {intact.w + 2 * m}x{contracts[key][1]}")
        elif key and [b.w, b.h] != contracts[key]:
            bad.append(f"{name}: {b.w}x{b.h}, contract {key} is {contracts[key]}")
    if bad:
        raise SystemExit("size contract broken:\n  " + "\n  ".join(bad))


def sheet(sprites):
    names = [n for n in sprites if not n.endswith("strip_5deg")]
    cell = 46
    b = Buf(cell * len(names), 64, c("sky_lo"))
    b.rect(0, 54, b.w, 10, c("rock"))
    for k, n in enumerate(names):
        spr = sprites[n][0]
        b.blit(spr, k * cell + (cell - spr.w) // 2, 54 - spr.h)
    # Assembled trebuchets at a few release angles, arm pivot on the axle.
    frame = sprites["trebuchet"][0]
    row = Buf(b.w, 64, c("sky_hi"))
    row.rect(0, 56, row.w, 8, c("rock"))
    for k, ang in enumerate(range(0, 95, 15)):
        x = 8 + k * 70
        row.blit(frame, x, 56 - FRAME_H)
        ax, ay = x + AXLE[0], 56 - FRAME_H + AXLE[1]
        row.blit(arm(ang), ax - ARM_PIVOT[0], ay - ARM_PIVOT[1])
    out = Buf(b.w, b.h + row.h, c("sky_hi"))
    out.blit(b, 0, 0)
    out.blit(row, 0, b.h)
    return out


def main():
    sprites = build()
    check(sprites)
    os.makedirs(SPR, exist_ok=True)
    for name, (b, _) in sprites.items():
        g.save_preview(b.image(1), os.path.join(SPR, name + ".png"))
    g.save_preview(sheet(sprites).image(4), os.path.join(OUT, "medieval_sheet_4x.png"))
    print(f"ok: {len(sprites)} sprites")


if __name__ == "__main__":
    main()

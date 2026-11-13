#!/usr/bin/env python3
"""Pack Counterweight's sprites into one blob the game embeds.

The game crate is engine-free and has no image decoder, so sprites ship as
raw RGBA: for each sprite, a length-prefixed name, u16 width, u16 height
(little-endian), then width*height*4 bytes. Present-era sprites come from the
engine's art/sprites (unchanged); Siege-era sprites from gameoff/art/sprites.

    uv run python gameoff/tools/export_atlas.py
"""
import os
import struct

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "crates", "shift-game", "assets", "sprites.bin")

PRESENT = [
    "tank_base", "barrel_strip_5deg",
    "tank_ballast", "barrel_strip_ballast", "barrel_strip_sabot",
    "derrick", "derrick_leaning", "derrick_collapsed",
    "radio_mast", "radio_mast_leaning", "radio_mast_collapsed",
    "tower_intact", "tower_leaning", "tower_collapsed",
    "silo", "silo_leaning", "silo_collapsed",
    "pylon", "pylon_leaning", "pylon_collapsed",
]
SIEGE = [
    "trebuchet", "trebuchet_arm_strip_5deg", "stone",
    "watchtower", "watchtower_leaning", "watchtower_collapsed",
    "banner_pole", "banner_pole_leaning", "banner_pole_collapsed",
    "bell_tower", "bell_tower_leaning", "bell_tower_collapsed",
    "keep", "keep_leaning", "keep_collapsed",
    "siege_tower", "siege_tower_leaning", "siege_tower_collapsed",
]


def main():
    blob = bytearray()
    sources = [(n, os.path.join(ROOT, "art", "sprites", n + ".png")) for n in PRESENT]
    sources += [(n, os.path.join(ROOT, "gameoff", "art", "sprites", n + ".png")) for n in SIEGE]
    for name, path in sources:
        im = Image.open(path).convert("RGBA")
        raw = name.encode()
        blob += struct.pack("<B", len(raw)) + raw + struct.pack("<HH", *im.size) + im.tobytes()
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(blob)
    print(f"ok: {len(sources)} sprites, {len(blob)} bytes")


if __name__ == "__main__":
    main()

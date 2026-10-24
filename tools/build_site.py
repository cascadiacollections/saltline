#!/usr/bin/env python3
"""Salt Line — marketing site media builder.

Every image the site ships uses the exported game sprites in art/sprites/.
Terrain and UI come from tools/gen_sprites.py: same 16-colour palette, same
native 320x180 grid, same rasteriser. Nothing is anti-aliased, and the only
scaling is integer NEAREST. No Aseprite installation is needed for site builds.

    python3 tools/build_site.py     # needs Pillow + Rust; writes into site/assets/

Outputs:
    site/assets/scene_native.png      the reference scene, 1x
    site/assets/scene_4x.png          the reference scene, 4x
    site/assets/hero_wide.png         a 640x180 screen, 2x, for the hero
    site/assets/shot.gif              one shot, integrated at the sim's dt
    site/assets/shot_static.png       reduced-motion final frame of that shot
    site/assets/sprites/*.png         native-resolution sprites
    site/assets/sprite_sheet_4x.png   contact sheet
    site/assets/palette_16.png        the palette, which is the source of truth
    site/assets/og.png                1200x630 social card
    site/assets/favicon.png           64x64
"""
import html
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from PIL import GifImagePlugin, Image

import gen_sprites as G
from gen_sprites import BAYER, Buf, hud, rgba, text

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ART = os.path.join(ROOT, "art")
OUT = os.path.join(ROOT, "site", "assets")
OUT_SPR = os.path.join(OUT, "sprites")

# Sim constants, lifted from crates/salt-line-sim/src/lib.rs so the arc the
# site animates is the arc the game integrates.
DT = 1.0 / 120.0
GRAVITY = 140.0
EMPLACEMENT_X = 30.0


# ---------------------------------------------------------------- scene parts
def sky(b, width, height):
    for y in range(height):
        t = y / 105.0
        for x in range(width):
            th = (BAYER[y % 4][x % 4] + 0.5) / 16.0
            if t < 1.0:
                lo, hi, f = "sky_hi", "sky_mid", max(0.0, (t - 0.45) / 0.55)
            else:
                lo, hi, f = "sky_mid", "sky_lo", min(1.0, (t - 1.0) * 2.4)
            if f <= 0.02:
                b.set(x, y, rgba(lo))
            elif f >= 0.98:
                b.set(x, y, rgba(hi))
            else:
                b.set(x, y, rgba(hi if f > th else lo))


def ground(b, hs, width, height):
    for x in range(width):
        top = int(hs[x])
        for y in range(top, height):
            d = y - top
            if d < 7:
                c = rgba("sand")
            elif d < 30:
                c = rgba("rock")
            elif d < 38:
                c = (
                    rgba("rock")
                    if BAYER[y % 4][x % 4] / 16.0 > (d - 30) / 8.0
                    else rgba("bedrock")
                )
            else:
                c = rgba("bedrock")
            b.set(x, y, c)
        if 0 < x < width - 1 and hs[x] > hs[x - 1] + 0.4:
            b.set(x, top, rgba("shadow"))
            b.set(x, top + 1, rgba("shadow"))


def stars(b, width, seed=7):
    n = 0
    for x in range(3, width, 7):
        n = (n * 1103515245 + x * 12345 + seed) & 0xFFFFFFFF
        y = 8 + (n >> 13) % 46
        if (n >> 5) % 5 == 0:
            b.set(x, y, rgba("ui_fg"))


def compose(
    width=320, height=180, seed=20260907, craters=(), props=(), terrain_heights=None
):
    """A screen: sky, terrain, structures, the emplacement. No shell."""
    b = Buf(width, height, rgba("sky_hi"))
    sky(b, width, height)
    stars(b, width, seed)

    hs = (
        list(terrain_heights)
        if terrain_heights is not None
        else G.terrain(width, seed, amp=1.25)
    )
    if terrain_heights is None:
        for cx, r in craters:
            G.crater(hs, cx, r)
    ground(b, hs, width, height)

    for kind, x, state in props:
        name = {"tower": "tower", "derrick": "derrick", "mast": "radio_mast",
                "silo": "silo", "pylon": "pylon"}[kind]
        if kind == "tower" or state not in (None, "intact"):
            name += "_" + state
        spr = G.load_sprite(name)
        b.blit(spr, x, G.seat(hs, x, spr.w) - spr.h + 2)

    tk, pivot = G.load_tank("base")
    px = int(EMPLACEMENT_X) - pivot[0]
    py = G.seat(hs, int(EMPLACEMENT_X) - 13, 26) - tk.h + 3
    muzzle = (px + pivot[0], py + pivot[1])
    return b, hs, (px, py), pivot, muzzle


def with_barrel(b, tank_xy, pivot, angle_deg):
    px, py = tank_xy
    tk, _ = G.load_tank("base")
    bar, bp = G.load_barrel(angle_deg)
    b.blit(bar, px + pivot[0] - bp[0], py + pivot[1] - bp[1])
    b.blit(tk, px, py)


# ---------------------------------------------------------------- ballistics
def fly(muzzle, angle_deg, speed, wind, hs, width, height):
    """Semi-implicit Euler at the sim's fixed dt. Wind is an acceleration."""
    import math

    x, y = float(muzzle[0]), float(muzzle[1])
    a = math.radians(angle_deg)
    vx, vy = speed * math.cos(a), -speed * math.sin(a)
    path = []
    for _ in range(4800):
        path.append((x, y))
        vx += wind * DT
        vy += GRAVITY * DT
        x += vx * DT
        y += vy * DT
        if x < 0 or x > width - 1 or y > height - 1:
            return path, None
        if y >= hs[int(x)]:
            return path, (x, y)
    return path, None


def aim(muzzle, target_x, wind, hs, width, height, arc="high"):
    """Brute-force what the game's solver does: find inputs that land the
    shell on the target. Among the solutions that hit, prefer the lofted one —
    it is the arc that shows wind drift off."""
    hits = []
    for ang in range(30, 71):
        for sp in range(90, 261, 2):
            path, hit = fly(muzzle, ang, float(sp), wind, hs, width, height)
            if hit is None:
                continue
            apex = min(p[1] for p in path)
            if apex < 22:  # keep the whole arc inside the frame
                continue
            hits.append((abs(hit[0] - target_x), apex, path, hit, ang, sp))
    if not hits:
        return None
    best_err = min(h[0] for h in hits)
    near = [h for h in hits if h[0] <= best_err + 1.0]
    pick = min(near, key=lambda h: h[1]) if arc == "high" else max(near, key=lambda h: h[1])
    return pick[0], pick[2], pick[3], pick[4], pick[5]


def verified_shot(hs, width, height, wind, target_x):
    """Return a dialable shot accepted by salt-line-sim's solver and damage model."""
    request = " ".join(
        [str(width), str(height), str(wind), str(int(target_x))]
        + [format(y, ".17g") for y in hs]
    )
    proc = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--release",
            "-p",
            "salt-line-sim",
            "--bin",
            "site_shot",
        ],
        cwd=ROOT,
        input=request,
        capture_output=True,
        check=True,
        text=True,
    )
    lines = iter(proc.stdout.splitlines())
    angle_ticks, speed, impact_x, impact_y, path_len = next(lines).split()
    path = []
    for _ in range(int(path_len)):
        x, y = next(lines).split()
        path.append((float(x), height - float(y)))
    if next(lines) != "SURFACE":
        raise RuntimeError("invalid site_shot output")
    after_hs = [height - float(next(lines)) for _ in range(width)]
    return (
        path,
        (float(impact_x), height - float(impact_y)),
        float(angle_ticks) / 20.0,
        float(speed),
        after_hs,
    )


def tracer(b, path, hit, every=10):
    """The shot, dotted. Alert colour so it reads against the dithered sky,
    where cream would be mistaken for a star."""
    for i in range(0, len(path), every):
        x, y = path[i]
        b.set(int(x), int(y), rgba("alert"))
    if hit is not None:
        b.rect(int(hit[0]) - 1, int(hit[1]) - 1, 2, 2, rgba("alert"))


# ---------------------------------------------------------------- media
PROPS = (("tower", 188, "leaning"), ("mast", 236, None), ("derrick", 268, None))
CRATERS = ((196, 11), (150, 7), (96, 5))


def hero_wide(scale=2):
    width, height = 640, 180
    props = (
        ("tower", 196, "leaning"),
        ("mast", 250, None),
        ("derrick", 300, None),
        ("silo", 360, None),
        ("pylon", 430, None),
        ("tower", 492, "intact"),
        ("derrick", 560, None),
        ("mast", 604, None),
    )
    b, hs, tank_xy, pivot, muzzle = compose(
        width, height, seed=20260907, craters=((150, 7), (96, 5)), props=props
    )
    shot = aim(muzzle, 300.0, 4.0, hs, width, height)
    _, path, hit, ang, _ = shot
    with_barrel(b, tank_xy, pivot, ang)
    tracer(b, path, hit)
    hud(b, width, height, 4.0, 31, "27")
    return b.image(scale)


def shot_gif(scale=2, path_out=None):
    """One solver-verified shot and its lasting damage state.

    Uses 16 colours, with no dithering added by the encoder.
    """
    width, height = 320, 180
    wind = 3.2
    target = 188.0

    b, hs, tank_xy, pivot, muzzle = compose(
        width, height, seed=20260907, craters=((150, 7), (96, 5)), props=PROPS
    )
    path, hit, ang, speed, after_hs = verified_shot(
        hs, width, height, wind, target
    )
    err = abs(hit[0] - target)

    with_barrel(b, tank_xy, pivot, ang)
    hud(b, width, height, 3.2, 28, "34")
    base = b.image(1)

    # after: the same screen after the sim applies the impact, tower down
    after_props = (("tower", 188, "collapsed"), ("mast", 236, None), ("derrick", 268, None))
    ab, _, atank, apivot, _ = compose(
        width,
        height,
        seed=20260907,
        props=after_props,
        terrain_heights=after_hs,
    )
    with_barrel(ab, atank, apivot, ang)
    hud(ab, width, height, 3.2, 27, "34")
    after = ab.image(1)

    fg = tuple(G.P["ui_fg"]) + (255,)
    alert = tuple(G.P["alert"]) + (255,)
    frames, durations = [], []

    step = 6
    trail = []
    for i in range(0, len(path), step):
        f = base.copy()
        px = f.load()
        for tx, ty in trail[-26:][::4]:
            if 0 <= int(tx) < width and 0 <= int(ty) < height:
                px[int(tx), int(ty)] = fg
        x, y = path[i]
        for dx in (0, 1):
            for dy in (0, 1):
                if 0 <= int(x) + dx < width and 0 <= int(y) + dy < height:
                    px[int(x) + dx, int(y) + dy] = alert
        frames.append(f)
        durations.append(40)
        trail.extend(path[i : i + step])

    # impact: an expanding ring, then the crater that stays
    for r in (3, 7, 11):
        f = after.copy()
        px = f.load()
        for t in range(0, 360, 6):
            import math

            x = int(hit[0] + r * math.cos(math.radians(t)))
            y = int(hit[1] + r * math.sin(math.radians(t)))
            if 0 <= x < width and 0 <= y < height:
                px[x, y] = alert
        frames.append(f)
        durations.append(70)
    frames.append(after)
    durations.append(1600)

    frames = [f.convert("RGB").resize((width * scale, height * scale), Image.Resampling.NEAREST) for f in frames]
    pal = frames[0].quantize(colors=16, method=Image.Quantize.MAXCOVERAGE)
    frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
    frames[0].save(
        path_out,
        save_all=True,
        append_images=frames[1:],
        duration=durations,
        loop=0,
        optimize=True,
        disposal=2,
    )
    return ang, speed, err


def og_card():
    """1200x630. Native 300x157 grid, drawn then scaled 4x."""
    width, height = 300, 157
    b, hs, tank_xy, pivot, muzzle = compose(
        width,
        height,
        seed=20260907,
        craters=((150, 7), (96, 5)),
        props=(("tower", 178, "leaning"), ("mast", 222, None), ("derrick", 258, None)),
    )
    _, path, hit, ang, _ = aim(muzzle, 178.0, 3.2, hs, width, height)
    with_barrel(b, tank_xy, pivot, ang)
    tracer(b, path, hit, every=9)

    b.rect(0, 0, width, 34, rgba("ui_bg"))
    text(b, "SALT LINE", 8, 6, rgba("ui_fg"), )
    text(b, "ARTILLERY ROGUELIKE", 8, 18, rgba("alert"))
    text(b, "ALPHA", width - 26, 6, rgba("alert"))
    b.rect(0, height - 14, width, 14, rgba("ui_bg"))
    text(b, "FREE AND OPEN SOURCE", 8, height - 10, rgba("ui_fg"))
    card = Image.new("RGB", (1200, 630), G.P["ui_bg"])
    card.paste(b.image(4).convert("RGB"), (0, 1))
    return card


def favicon():
    b = Buf(16, 16, rgba("ui_bg"))
    tk, pivot = G.load_tank("base")
    bar, bp = G.load_barrel(45)
    b.blit(bar, 1 + pivot[0] - bp[0] - 7, 3 + pivot[1] - bp[1])
    b.blit(tk, -7, 3)
    return b.image(4)


def license_page():
    """Publish the bundled notices locally, without relying on repository access."""
    notices = [
        ("Salt Line — MIT", "LICENSE"),
        ("Godot Engine — MIT", "third_party/godot-LICENSE.txt"),
        ("Godot Rust bindings (gdext) — MPL-2.0", "third_party/gdext-LICENSE-MPL-2.0.txt"),
        ("glam — MIT option", "third_party/glam-LICENSE-MIT.txt"),
        ("libc — MIT option", "third_party/libc-LICENSE-MIT.txt"),
        ("Linux desktop integration — MIT", "addons/xdg_portals/LICENSE"),
        ("Linux integration third-party notices", "addons/xdg_portals/THIRD_PARTY_NOTICES.md"),
    ]
    sections = []
    for title, relative_path in notices:
        with open(os.path.join(ROOT, relative_path), encoding="utf-8") as source:
            notice = source.read()
        sections.append(
            f'<details><summary>{html.escape(title)}</summary>'
            f'<pre class="license-text">{html.escape(notice)}</pre></details>'
        )
    document = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Salt Line — credits and licenses</title>
<meta name="description" content="Salt Line's MIT license and bundled free and open-source software acknowledgments.">
<meta name="theme-color" content="#2e2a26">
<link rel="icon" href="assets/favicon.png" type="image/png">
<link rel="stylesheet" href="style.css">
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="topbar"><a class="brand" href="index.html">SALT LINE</a>
<nav aria-label="Project links"><a href="index.html#download">Downloads</a></nav></header>
<main id="main"><section aria-labelledby="licenses-h">
<h1 id="licenses-h">Credits &amp; licenses</h1>
<p>Salt Line is by Cascadia Collections and is licensed under MIT.
Thank you to the free and open-source communities behind Godot Engine,
the gdext Rust bindings, glam, libc, and Linux desktop integration.</p>
<p>The complete bundled notices below are also available offline in the game's
Settings → Credits / Licenses. The Linux integration notices describe its
Godot C++ bindings and system libraries and services.</p>
<p><a href="index.html">Back to Salt Line</a></p>
""" + "\n".join(sections) + "\n</section></main>\n</body>\n</html>\n"
    with open(os.path.join(ROOT, "site", "licenses.html"), "w", encoding="utf-8") as out:
        out.write(document)


# ---------------------------------------------------------------- driver
def main():
    G.main()
    license_page()
    shutil.rmtree(OUT_SPR, ignore_errors=True)
    os.makedirs(OUT_SPR, exist_ok=True)

    # the shipped art, copied verbatim — the site never re-encodes it
    shutil.copyfile(os.path.join(ART, "scene_native_320x180.png"), os.path.join(OUT, "scene_native.png"))
    shutil.copyfile(os.path.join(ART, "scene_preview_4x.png"), os.path.join(OUT, "scene_4x.png"))
    shutil.copyfile(os.path.join(ART, "sprite_sheet_4x.png"), os.path.join(OUT, "sprite_sheet_4x.png"))
    shutil.copyfile(os.path.join(ART, "palette_16.png"), os.path.join(OUT, "palette_16.png"))
    for name in sorted(os.listdir(os.path.join(ART, "sprites"))):
        if name.endswith(".png"):
            shutil.copyfile(
                os.path.join(ART, "sprites", name), os.path.join(OUT_SPR, name)
            )
    print("copied art/ -> site/assets/")

    hero_wide().save(os.path.join(OUT, "hero_wide.png"))
    print("hero_wide.png")

    ang, speed, err = shot_gif(path_out=os.path.join(OUT, "shot.gif"))
    with Image.open(os.path.join(OUT, "shot.gif")) as animation:
        assert isinstance(animation, GifImagePlugin.GifImageFile)
        animation.seek(animation.n_frames - 1)
        animation.convert("RGB").save(os.path.join(OUT, "shot_static.png"))
    print(f"shot.gif (angle {ang} deg, muzzle {speed}, impact error {err:.2f}px)")

    og_card().save(os.path.join(OUT, "og.png"))
    print("og.png")

    Image.open(os.path.join(ART, "icon_1024.png")).resize((64, 64), Image.Resampling.LANCZOS).save(os.path.join(OUT, "favicon.png"))
    print("favicon.png")


if __name__ == "__main__":
    main()

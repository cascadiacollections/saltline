#!/usr/bin/env python3
"""Salt Line — sprite + scene generator.

Renders at the spec's native internal resolution (180px tall) using the
16-colour fixed palette. Nothing here is anti-aliased, scaled or rotated at
runtime; every sprite is rasterised directly into the pixel grid.
"""
import math, os, struct
from PIL import Image

# Writes into the repo's art/ directory by default; override with SALT_LINE_ART.
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.environ.get("SALT_LINE_ART", os.path.join(ROOT, "art"))
SPR = os.path.join(OUT, "sprites")
os.makedirs(SPR, exist_ok=True)

# ---------------------------------------------------------------- palette (16)
P = {
    # terrain ramp (4)
    "sand":      (232, 220, 200),
    "rock":      (168, 150, 128),
    "bedrock":   (107,  92,  77),
    "shadow":    ( 74,  62,  54),
    # sky (3)
    "sky_hi":    ( 36,  54,  92),
    "sky_mid":   ( 60,  84, 128),
    "sky_lo":    (110, 134, 168),
    # player (3)
    "hull":      ( 90, 107,  88),
    "hull_hi":   (126, 143, 118),
    "hull_edge": ( 46,  56,  48),
    # structures (3)
    "st":        (181, 103,  74),
    "st_hi":     (216, 144, 122),
    "st_dmg":    (110,  58,  44),
    # ui (2)
    "ui_fg":     (245, 239, 226),
    "ui_bg":     ( 46,  42,  38),
    # alert (1)
    "alert":     (232, 163,  61),
}
T = (0, 0, 0, 0)


def rgba(name, a=255):
    r, g, b = P[name]
    return (r, g, b, a)


class Buf:
    def __init__(self, w, h, fill=T):
        self.w, self.h = w, h
        self.px = [[fill for _ in range(w)] for _ in range(h)]

    def set(self, x, y, c):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[int(y)][int(x)] = c

    def get(self, x, y):
        if 0 <= x < self.w and 0 <= y < self.h:
            return self.px[int(y)][int(x)]
        return T

    def rect(self, x, y, w, h, c):
        for j in range(int(y), int(y + h)):
            for i in range(int(x), int(x + w)):
                self.set(i, j, c)

    def line(self, x0, y0, x1, y1, c, thick=1):
        x0, y0, x1, y1 = int(x0), int(y0), int(x1), int(y1)
        dx, dy = abs(x1 - x0), -abs(y1 - y0)
        sx = 1 if x0 < x1 else -1
        sy = 1 if y0 < y1 else -1
        err = dx + dy
        while True:
            if thick == 1:
                self.set(x0, y0, c)
            else:
                o = thick // 2
                for a in range(thick):
                    for b in range(thick):
                        self.set(x0 - o + a, y0 - o + b, c)
            if x0 == x1 and y0 == y1:
                break
            e2 = 2 * err
            if e2 >= dy:
                err += dy
                x0 += sx
            if e2 <= dx:
                err += dx
                y0 += sy

    def blit(self, spr, x, y):
        for j in range(spr.h):
            for i in range(spr.w):
                c = spr.px[j][i]
                if c[3] != 0:
                    self.set(x + i, y + j, c)

    def image(self, scale=1):
        im = Image.new("RGBA", (self.w, self.h))
        im.putdata([self.px[j][i] for j in range(self.h) for i in range(self.w)])
        if scale != 1:
            im = im.resize((self.w * scale, self.h * scale), Image.NEAREST)
        return im


# ---------------------------------------------------------------- 3x5 font
F = {
    "A": "111 101 111 101 101", "C": "111 100 100 100 111",
    "D": "110 101 101 101 110", "E": "111 100 111 100 111",
    "F": "111 100 111 100 100", "H": "101 101 111 101 101",
    "I": "111 010 010 010 111", "L": "100 100 100 100 111",
    "N": "101 111 111 101 101", "O": "111 101 101 101 111",
    "P": "111 101 111 100 100", "R": "110 101 110 101 101",
    "S": "111 100 111 001 111", "T": "111 010 010 010 010",
    "U": "101 101 101 101 111", "W": "101 101 111 111 101",
    "Y": "101 101 111 010 010", "M": "101 111 111 101 101",
    "0": "111 101 101 101 111", "1": "010 110 010 010 111",
    "2": "111 001 111 100 111", "3": "111 001 111 001 111",
    "4": "101 101 111 001 001", "5": "111 100 111 001 111",
    "6": "111 100 111 101 111", "7": "111 001 001 001 001",
    "8": "111 101 111 101 111", "9": "111 101 111 001 111",
    ".": "000 000 000 000 010", "+": "000 010 111 010 000",
    "-": "000 000 111 000 000", ":": "000 010 000 010 000",
    " ": "000 000 000 000 000",
}


def text(buf, s, x, y, c):
    cx = x
    for ch in s.upper():
        g = F.get(ch)
        if g:
            rows = g.split()
            for j, row in enumerate(rows):
                for i, bit in enumerate(row):
                    if bit == "1":
                        buf.set(cx + i, y + j, c)
        cx += 4
    return cx


# ---------------------------------------------------------------- noise
def h1(n, seed):
    n = (n ^ seed) & 0xFFFFFFFF
    n = (n ^ (n >> 13)) * 0x85EBCA6B & 0xFFFFFFFF
    n = (n ^ (n >> 16)) * 0xC2B2AE35 & 0xFFFFFFFF
    return ((n ^ (n >> 13)) & 0xFFFFFFFF) / 0xFFFFFFFF


def vnoise(x, seed):
    i = math.floor(x)
    f = x - i
    f = f * f * (3 - 2 * f)          # smoothstep
    return h1(i, seed) * (1 - f) + h1(i + 1, seed) * f


def fbm(x, seed, oct=4, lac=2.0, gain=0.5):
    a, fr, s = 1.0, 1.0, 0.0
    norm = 0.0
    for o in range(oct):
        s += vnoise(x * fr, seed + o * 7919) * a
        norm += a
        a *= gain
        fr *= lac
    return s / norm


def terrain(width, seed, amp=1.0, base_y=128):
    """Value-noise fBm with domain warp — §2.3 steps 1 and 2."""
    hs = []
    for x in range(width):
        warp = (fbm(x * 0.006, seed + 555) - 0.5) * 30.0   # domain warp
        n = fbm((x + warp) * 0.014, seed)
        ridge = fbm((x + warp) * 0.004, seed + 191)
        h = base_y - (n * 26 + ridge * 20) * amp
        hs.append(h)
    return hs


# ---------------------------------------------------------------- entities
def tank(variant="base"):
    w, h = 30, 12
    b = Buf(w, h)
    wide = variant == "ballast"
    dark = variant == "forced"
    body = rgba("hull_edge") if dark else rgba("hull")
    hi = rgba("hull") if dark else rgba("hull_hi")
    cw = 30 if wide else 26
    x0 = (w - cw) // 2
    ty = 5 if wide else 4
    # hull: low trapezoid, rear taper
    for j in range(6):
        inset = 0 if j > 1 else (2 - j)
        b.rect(x0 + inset + (2 if j < 2 else 0), ty + j, cw - inset * 2 - (2 if j < 2 else 0), 1, body)
    if not dark:
        b.rect(x0 + 4, ty + 1, cw - 9, 1, hi)
    # turret block
    b.rect(x0 + 9, ty - 2, 9, 3, body)
    b.rect(x0 + 10, ty - 2, 6, 1, hi)
    # track band + dark bottom edge
    b.rect(x0 + 1, ty + 6, cw - 2, 2, rgba("hull_edge"))
    for i in range(x0 + 2, x0 + cw - 2, 3):
        b.set(i, ty + 6, rgba("hull"))
    b.rect(x0, ty + 8, cw, 1, rgba("shadow"))
    return b, (x0 + 13, ty - 1)   # sprite + barrel pivot


def barrel(angle_deg, variant="base"):
    """Pre-rendered frame — no runtime rotation (§5.3)."""
    length, thick = 16, 3
    if variant == "sabot":
        length, thick = 22, 2
    elif variant == "cluster":
        length, thick = 9, 5
    elif variant == "auger":
        length, thick = 16, 3
    s = 28
    b = Buf(s, s)
    cx = cy = s // 2
    a = math.radians(angle_deg)
    ex = cx + math.cos(a) * length
    ey = cy - math.sin(a) * length
    b.line(cx, cy, ex, ey, rgba("hull"), thick)
    b.line(cx, cy, ex, ey, rgba("hull_edge"), 1)
    if variant == "auger":
        b.rect(ex - 1, ey - 1, 3, 3, rgba("hull_hi"))
    return b, (cx, cy)


def water_tower(state="intact"):
    w, h = 17, 34
    b = Buf(w, h)
    if state == "collapsed":
        return rubble(17, 8)
    lean = 0
    if state == "leaning":
        lean = 1
    def sx(j):   # shear offset grows toward the top
        return int(lean * (h - j) / 3.5)
    # legs (9px footprint)
    for j in range(18, 33):
        b.set(4 + sx(j), j, rgba("st"))
        b.set(12 + sx(j), j, rgba("st"))
    for j in (22, 27):
        b.line(4 + sx(j), j, 12 + sx(j), j + 3, rgba("st_dmg"))
        b.line(12 + sx(j), j, 4 + sx(j), j + 3, rgba("st_dmg"))
    # bulb
    for j in range(6, 18):
        t = (j - 6) / 11.0
        rad = int(8 - abs(t - 0.35) * 6)
        o = sx(j)
        b.rect(8 - rad + o, j, rad * 2 + 1, 1, rgba("st"))
    b.rect(5 + sx(8), 8, 4, 1, rgba("st_hi"))
    b.rect(4 + sx(10), 10, 3, 4, rgba("st_hi"))
    # cap
    b.rect(6 + sx(5), 5, 5, 1, rgba("st"))
    b.set(8 + sx(3), 3, rgba("st_hi"))
    b.set(8 + sx(4), 4, rgba("st"))
    if state == "leaning":
        for j in range(28, 33):
            b.set(4 + sx(j), j, rgba("st_dmg"))
    return b


def derrick():
    w, h = 15, 42
    b = Buf(w, h)
    for j in range(0, 41):
        t = j / 41.0
        half = int(1 + t * 5)
        b.set(7 - half, j, rgba("st"))
        b.set(7 + half, j, rgba("st"))
        if j % 5 == 0 and j > 2:
            b.line(7 - half, j, 7 + half, j, rgba("st_dmg"))
        if j % 5 == 2:
            nh = int(1 + ((j + 3) / 41.0) * 5)
            b.line(7 - half, j, 7 + nh, j + 3, rgba("st_dmg"))
    b.rect(6, 0, 3, 2, rgba("st_hi"))
    return b


def radio_mast():
    w, h = 13, 32
    b = Buf(w, h)
    b.rect(6, 0, 2, 31, rgba("st"))
    b.set(6, 0, rgba("alert"))
    for j in (10, 20):
        b.line(6, j, 0, 31, rgba("st_dmg"))
        b.line(7, j, 12, 31, rgba("st_dmg"))
    b.rect(4, 6, 6, 1, rgba("st_hi"))
    b.rect(5, 14, 4, 1, rgba("st_hi"))
    return b


def silo():
    w, h = 11, 27
    b = Buf(w, h)
    b.rect(1, 4, 9, 23, rgba("st"))
    b.rect(2, 5, 2, 21, rgba("st_hi"))
    b.rect(2, 2, 7, 2, rgba("st"))
    b.rect(3, 1, 5, 1, rgba("st_hi"))
    for j in (11, 18):
        b.rect(1, j, 9, 1, rgba("st_dmg"))
    return b


def pylon():
    w, h = 17, 33
    b = Buf(w, h)
    for j in range(32):
        t = j / 32.0
        half = int(1 + t * 6)
        b.set(8 - half, j, rgba("st"))
        b.set(8 + half, j, rgba("st"))
    for cy_ in (7, 15):
        half = int(1 + (cy_ / 32.0) * 6)
        b.rect(8 - half - 3, cy_, (half + 3) * 2 + 1, 1, rgba("st"))
        b.set(8 - half - 3, cy_ - 1, rgba("st_hi"))
        b.set(8 + half + 3, cy_ - 1, rgba("st_hi"))
    for j in range(4, 30, 6):
        b.line(3, j, 13, j + 5, rgba("st_dmg"))
    return b


def rubble(w=17, h=8):
    b = Buf(w, h)
    for i in range(w):
        hh = int(2 + 3 * abs(math.sin(i * 1.7)))
        for j in range(hh):
            c = rgba("st_dmg") if j < hh - 1 else rgba("st")
            b.set(i, h - 1 - j, c)
    return b


# ---------------------------------------------------------------- scene
def crater(hs, cx, r):
    """§2.6 smooth subtraction with squared falloff."""
    for i in range(max(0, int(cx - r)), min(len(hs), int(cx + r) + 1)):
        d = abs(i - cx) / r
        hs[i] += r * (1 - d * d) ** 2


def seat(hs, x, w):
    """Lowest surface point across the footprint, so nothing floats."""
    return int(max(hs[x:x + w]))


BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]


def scene(width=320, height=180, seed=20260907):
    b = Buf(width, height, rgba("sky_hi"))
    # sky: two dithered transitions across the full height (§5.2)
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

    hs = terrain(width, seed, amp=1.25)
    crater(hs, 196, 11)   # undermining hit at the tower footing
    crater(hs, 150, 7)    # ranging miss
    crater(hs, 96, 5)     # ranging miss

    for x in range(width):
        top = int(hs[x])
        for y in range(top, height):
            d = y - top
            if d < 7:
                c = rgba("sand")
            elif d < 30:
                c = rgba("rock")
            elif d < 38:
                c = rgba("rock") if BAYER[y % 4][x % 4] / 16.0 > (d - 30) / 8.0 else rgba("bedrock")
            else:
                c = rgba("bedrock")
            b.set(x, y, c)
        if 0 < x < width - 1 and hs[x] > hs[x - 1] + 0.4:
            b.set(x, top, rgba("shadow"))
            b.set(x, top + 1, rgba("shadow"))

    # structures, seated into the surface
    tower, tx = water_tower("leaning"), 188
    b.blit(tower, tx, seat(hs, tx, 7) - tower.h + 3)
    m, mx = radio_mast(), 236
    b.blit(m, mx, seat(hs, mx, 13) - m.h + 1)
    d, dx = derrick(), 268
    b.blit(d, dx, seat(hs, dx, 15) - d.h + 2)

    # player
    tk, pivot = tank("base")
    px = 20
    py = seat(hs, px, 30) - tk.h + 3
    bar, bp = barrel(45)
    b.blit(bar, px + pivot[0] - bp[0], py + pivot[1] - bp[1])
    b.blit(tk, px, py)

    # shot in flight — dotted arc, wind as acceleration (§3)
    x, y = float(px + pivot[0]), float(py + pivot[1])
    vx, vy, g, w = 2.30, -3.15, 0.090, 0.0040
    for step in range(46):
        if step % 4 == 0:
            b.set(int(x), int(y), rgba("ui_fg"))
        x += vx; y += vy; vy += g; vx += w
    b.rect(int(x) - 1, int(y) - 1, 2, 2, rgba("alert"))

    # chalk mark from the previous shot (§4, Chalk Marks)
    cm = 150
    for i in (-2, -1, 0, 1, 2):
        b.set(cm + i, int(hs[cm + i]) - 1, rgba("alert"))
    b.set(cm, int(hs[cm]) - 4, rgba("alert"))

    # UI, same buffer, same resolution (§5.1)
    b.rect(3, 3, 41, 9, rgba("ui_bg"))
    text(b, "WIND +2.4", 5, 5, rgba("alert"))
    b.rect(width - 42, 3, 39, 9, rgba("ui_bg"))
    text(b, "SHELLS 28", width - 40, 5, rgba("ui_fg"))
    text(b, "34", width - 11, height - 8, rgba("ui_fg"))
    return b


# ---------------------------------------------------------------- contact sheet
def sheet():
    items = [
        ("TANK", tank("base")[0]), ("BALLAST", tank("ballast")[0]),
        ("FORCED", tank("forced")[0]),
        ("TOWER", water_tower("intact")), ("LEANING", water_tower("leaning")),
        ("COLLAPSED", water_tower("collapsed")),
        ("DERRICK", derrick()), ("MAST", radio_mast()),
        ("SILO", silo()), ("PYLON", pylon()),
    ]
    cell_w, cell_h = 46, 56
    cols = len(items)
    b = Buf(cell_w * cols, cell_h + 30, rgba("sky_mid"))
    b.rect(0, cell_h + 4, b.w, 26, rgba("sand"))
    for k, (name, spr) in enumerate(items):
        ox = k * cell_w + (cell_w - spr.w) // 2
        oy = cell_h + 4 - spr.h
        b.blit(spr, ox, oy)
        text(b, name[:10], k * cell_w + 3, cell_h + 10, rgba("shadow"))
    # barrel frame strip
    b2 = Buf(b.w, 34, rgba("sky_hi"))
    for k, ang in enumerate(range(0, 95, 10)):
        br, bp = barrel(ang)
        b2.blit(br, k * 34, 2)
    out = Buf(b.w, b.h + 36, rgba("sky_hi"))
    out.blit(b, 0, 0)
    out.blit(b2, 0, b.h + 2)
    return out


# ---------------------------------------------------------------- palette swatch
def palette_img():
    order = ["sand", "rock", "bedrock", "shadow", "sky_hi", "sky_mid", "sky_lo",
             "hull", "hull_hi", "hull_edge", "st", "st_hi", "st_dmg",
             "ui_fg", "ui_bg", "alert"]
    b = Buf(16 * 12, 20, rgba("ui_bg"))
    for i, k in enumerate(order):
        b.rect(i * 12, 0, 12, 12, rgba(k))
    for i, k in enumerate(order):
        text(b, str(i + 1), i * 12 + 2, 14, rgba("ui_fg"))
    return b


if __name__ == "__main__":
    sc = scene()
    sc.image(1).save(f"{OUT}/scene_native_320x180.png")
    sc.image(4).save(f"{OUT}/scene_preview_4x.png")

    sh = sheet()
    sh.image(4).save(f"{OUT}/sprite_sheet_4x.png")

    palette_img().image(4).save(f"{OUT}/palette_16.png")

    # clean native-resolution assets
    tank("base")[0].image().save(f"{SPR}/tank_base.png")
    tank("ballast")[0].image().save(f"{SPR}/tank_ballast.png")
    tank("forced")[0].image().save(f"{SPR}/tank_forced_march.png")
    water_tower("intact").image().save(f"{SPR}/tower_intact.png")
    water_tower("leaning").image().save(f"{SPR}/tower_leaning.png")
    water_tower("collapsed").image().save(f"{SPR}/tower_collapsed.png")
    derrick().image().save(f"{SPR}/derrick.png")
    radio_mast().image().save(f"{SPR}/radio_mast.png")
    silo().image().save(f"{SPR}/silo.png")
    pylon().image().save(f"{SPR}/pylon.png")

    # 19-frame barrel strips, 5° increments, packed horizontally — one per
    # build silhouette (§5.3). Sliced at 28px by the Godot layer.
    for variant, name in (
        ("base", "barrel_strip_5deg"),
        ("sabot", "barrel_strip_sabot"),
        ("cluster", "barrel_strip_cluster"),
        ("auger", "barrel_strip_auger"),
    ):
        frames = [barrel(a, variant)[0] for a in range(0, 95, 5)]
        strip = Buf(28 * len(frames), 28)
        for i, f in enumerate(frames):
            strip.blit(f, i * 28, 0)
        strip.image().save(f"{SPR}/{name}.png")

    print("ok")

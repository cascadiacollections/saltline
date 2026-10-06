#!/usr/bin/env python3
"""Generate Salt Line's original, redistributable procedural sound effects.

Uses only the Python standard library. Output is deterministic PCM WAV; there
are no samples, downloads, or third-party source material.
"""

import math
import pathlib
import struct
import wave

RATE = 22050
OUT = pathlib.Path(__file__).resolve().parents[1] / "art" / "audio"


def noise(seed):
    seed = (1664525 * seed + 1013904223) & 0xFFFFFFFF
    return seed, ((seed >> 8) / 0x7FFFFF - 1.0)


def render(name, seconds, voice):
    count = int(RATE * seconds)
    seed = 0x53414C54
    samples = []
    for i in range(count):
        t = i / RATE
        seed, n = noise(seed)
        value = max(-1.0, min(1.0, voice(t, seconds, n)))
        samples.append(struct.pack("<h", int(value * 24500)))
    OUT.mkdir(parents=True, exist_ok=True)
    with wave.open(str(OUT / f"{name}.wav"), "wb") as out:
        out.setparams((1, 2, RATE, count, "NONE", "not compressed"))
        out.writeframes(b"".join(samples))


def env(t, d, power=2):
    return max(0.0, 1.0 - t / d) ** power


def notes(t, pitches, step, tail):
    """A plain chime: `pitches` (Hz) one after another, `step` seconds apart,
    each ringing out over `step` (the last over `tail`). Sine plus a quiet
    octave, so it reads as a bell on a phone speaker."""
    i = min(int(t / step), len(pitches) - 1)
    local = t - i * step
    ring = tail if i == len(pitches) - 1 else step
    f = pitches[i]
    attack = min(1.0, local / .006)
    return attack * env(local, ring, 1.6) * (.42*math.sin(2*math.pi*f*t) + .12*math.sin(4*math.pi*f*t))


def main():
    render("fire", .22, lambda t, d, n: env(t, d) * (.75*n + .25*math.sin(2*math.pi*(90-35*t/d)*t)))
    render("flight", .30, lambda t, d, n: env(t, d, .7) * (.13*n + .25*math.sin(2*math.pi*(760-420*t/d)*t)))
    render("terrain_impact", .28, lambda t, d, n: env(t, d) * (.68*n + .24*math.sin(2*math.pi*62*t)))
    render("structure_impact", .32, lambda t, d, n: env(t, d) * (.38*n + .5*math.sin(2*math.pi*(175-80*t/d)*t)))
    render("ricochet", .24, lambda t, d, n: env(t, d, 1.2) * (.15*n + .58*math.sin(2*math.pi*(1050+900*t/d)*t)))
    render("airburst", .26, lambda t, d, n: env(t, d) * (.72*n + .18*math.sin(2*math.pi*115*t)))
    render("collapse", .52, lambda t, d, n: env(t, d, .8) * (.48*n + .30*math.sin(2*math.pi*(75-30*t/d)*t)))
    render("draft", .24, lambda t, d, n: env(t, d, 1.3) * .42*math.sin(2*math.pi*(360+500*t/d)*t))
    render("low_shell", .34, lambda t, d, n: env(t, d, 1.4) * .44*math.sin(2*math.pi*(220 if t < .14 else 165)*t))
    render("ui", .09, lambda t, d, n: env(t, d, 1.5) * .32*math.sin(2*math.pi*620*t))
    # Local versus: a two-note ding for a hit, and a rising C-major arpeggio
    # for a round or match won.
    render("versus_hit", .36, lambda t, d, n: notes(t, (880, 1318.5), .09, .27))
    render("versus_win", .78, lambda t, d, n: notes(t, (523.25, 659.25, 783.99, 1046.5), .1, .48))


if __name__ == "__main__":
    main()

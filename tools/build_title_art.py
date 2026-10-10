#!/usr/bin/env python3
"""Normalize the authored title painting to the native grid and game palette."""
from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]


def main():
    with Image.open(ROOT / 'art/palette_16.png') as swatches:
        colours = [swatches.convert('RGB').getpixel((int((i + .5) * swatches.width / 16), swatches.height // 4)) for i in range(16)]
    palette = Image.new('P', (1, 1))
    palette.putpalette([channel for colour in colours for channel in colour] * 16)
    with Image.open(ROOT / 'art/source/title_salt_flat.png') as source:
        native = source.convert('RGB').resize((320, 180), Image.Resampling.NEAREST)
        native = native.quantize(palette=palette, dither=Image.Dither.NONE).convert('RGB')
    native.save(ROOT / 'art/title_background_320x180.png')
    assert len(native.getcolors(320 * 180)) <= 16
    print('Title background: 320x180, fixed game palette')


if __name__ == '__main__':
    main()

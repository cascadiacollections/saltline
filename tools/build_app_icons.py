#!/usr/bin/env python3
"""Size the supplied icon artwork for native packaging; default is evergreen.

Use --variant halloween, thanksgiving or christmas before exporting a seasonal
build. Run without arguments to restore the standard icon. No gameplay changes.
"""
import argparse
from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
VARIANTS = ('default', 'halloween', 'thanksgiving', 'christmas')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--variant', choices=VARIANTS, default='default')
    args = parser.parse_args()
    with Image.open(ROOT / 'art/app-icons' / f'saltline-{args.variant}.png') as source:
        icon = source.convert('RGB').resize((1024, 1024), Image.Resampling.LANCZOS)
    icon.save(ROOT / 'art/icon_1024.png')
    icon.resize((64, 64), Image.Resampling.LANCZOS).save(ROOT / 'site/assets/favicon.png')
    print(f'Packaged {args.variant} icon (1024×1024)')


if __name__ == '__main__':
    main()

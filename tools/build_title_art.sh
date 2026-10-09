#!/usr/bin/env bash
# Normalize the authored title painting to the game's native grid and palette.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
palette=$(mktemp "${TMPDIR:-/tmp}/salt-title-palette.XXXXXX.png")
trap 'rm -f "$palette"' EXIT
# Only the swatches, excluding the labels in the bottom 40 percent.
magick "$root/art/palette_16.png" -crop 100%x60%+0+0 +repage "$palette"
magick "$root/art/source/title_salt_flat.png" \
    -filter point -resize '320x180!' +dither -remap "$palette" -alpha off \
    "$root/art/title_background_320x180.png"

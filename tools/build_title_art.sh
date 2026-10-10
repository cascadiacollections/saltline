#!/usr/bin/env bash
# Normalize the authored title painting to the game's native grid and palette.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
uv run python tools/build_title_art.py

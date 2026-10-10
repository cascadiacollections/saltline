#!/usr/bin/env bash
# Preserve the Salt & Iron hero scene's proportions in native and detailed art.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
exec uv run python tools/build_title_art.py

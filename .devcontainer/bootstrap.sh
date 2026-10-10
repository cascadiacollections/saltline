#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
# Resolve only the checked-in lock; no compilation, assets or engine downloads.
uv sync --locked
printf '%s\n' 'Ready: cargo test --locked; cargo build --locked -p salt-line-godot' \
    'Godot GUI and platform SDKs are opt-in; see docs/development-environment.md.'

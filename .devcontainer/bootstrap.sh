#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
# Resolve only the checked-in lock; no compilation, assets or engine downloads.
# The lint group puts the locked ty in .venv for the editor's ty extension.
uv sync --locked --group lint
printf '%s\n' 'Ready: cargo test --locked; cargo build --locked -p salt-line-godot' \
    'Godot GUI and platform SDKs are opt-in; see docs/development-environment.md.'

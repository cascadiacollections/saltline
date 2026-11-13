#!/usr/bin/env bash
# Build Counterweight's GDExtension and play it in Godot (4.7+ on PATH as
# `godot`). Extra arguments go to the game, e.g. `--cw-autoplay` (the solver
# bot plays) or `--cw-seed 1347`.
#
#   gameoff/run.sh                 play
#   gameoff/run.sh --cw-autoplay   watch the bot play
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
# rustup's cargo directly, so an untrusted mise.toml cannot get in the way.
cargo=$(command -v "$HOME/.cargo/bin/cargo" || command -v cargo)
"$cargo" build --release -p shift-godot
case "$(uname -s)" in
    Darwin) lib=libshift_godot.dylib ;;
    MINGW*|MSYS*|CYGWIN*) lib=shift_godot.dll ;;
    *) lib=libshift_godot.so ;;
esac
mkdir -p gameoff/godot/bin gameoff/godot/.godot
cp "target/release/$lib" gameoff/godot/bin/
echo 'res://counterweight.gdextension' > gameoff/godot/.godot/extension_list.cfg
exec godot --path gameoff/godot -- "$@"

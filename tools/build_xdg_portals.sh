#!/usr/bin/env bash
set -euo pipefail

# Pin the Linux native bridge to the audited addon revision copied into
# addons/xdg_portals. The addon remains usable with its null backend elsewhere.
revision=6e14d9975f3bb0b054d958dddb7afc977fb7256e
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_dir="${XDG_PORTALS_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/saltline/xdg-portals-src}"
output_dir="$root/addons/xdg_portals/bin"
descriptor="$root/addons/xdg_portals/xdg_portals.gdextension"

if [ ! -d "$source_dir/.git" ]; then
    mkdir -p "$source_dir"
    git -C "$source_dir" init -q
    git -C "$source_dir" remote add origin https://github.com/KevinTCoughlin/godot-xdg-portals.git
fi
git -C "$source_dir" fetch -q --depth 1 origin "$revision"
git -C "$source_dir" checkout -q --detach FETCH_HEAD
if [ "$(git -C "$source_dir" rev-parse HEAD)" != "$revision" ]; then
    echo "XDG Portals source revision did not match the pin" >&2
    exit 1
fi

mkdir -p "$output_dir"
cp "$root/tools/xdg_portals_linux.gdextension.in" "$descriptor"
if [ "$#" -eq 0 ]; then
    set -- editor template_debug template_release
fi
for target in "$@"; do
    "$source_dir/scripts/build.sh" --target "$target"
    library="libxdg_portals.linux.${target}.x86_64.so"
    cp "$source_dir/addons/xdg_portals/bin/$library" "$output_dir/$library"
    test -s "$output_dir/$library"
done

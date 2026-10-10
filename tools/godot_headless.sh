#!/usr/bin/env bash
set -euo pipefail

# Run the Godot editor headless on this project, for example
#   bash tools/godot_headless.sh --frame-delay 800 --import
#   bash tools/godot_headless.sh --export-release "Linux" build/SaltLine.x86_64
# GODOT_BIN picks the binary.
#
# Works around godotengine/godot#111048 (fix pending in godotengine/godot#123658;
# remove this once the pinned Godot has it). The editor can queue a deferred
# EditorHelp::_gen_extensions_docs that, when the editor quits first, runs after
# the docs are freed and segfaults (exit 139) with the work already done. It is
# queued from two places, and this closes both:
#   - Discovering a GDExtension on the first scan of a clean checkout reloads the
#     docs. Listing the extensions in .godot/extension_list.cfg loads them at
#     startup instead, so the scan finds nothing new.
#   - Loading the editor's doc cache (written by any earlier editor run) always
#     queues it. Without a cache the docs are generated synchronously and nothing
#     is queued, at the cost of a second or two per run.
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

# Godot's scan skips hidden directories and any directory holding a .gdignore;
# the list has to match what the scan finds, or the scan still counts an
# extension as newly added.
extensions=()
while IFS= read -r path; do
    path=${path#./}
    dir=$(dirname "$path")
    ignored=false
    while [ "$dir" != . ]; do
        if [ -e "$dir/.gdignore" ]; then ignored=true; break; fi
        dir=$(dirname "$dir")
    done
    if [ "$ignored" = false ]; then extensions+=("res://$path"); fi
done < <(find . -name '.*' ! -name . -prune -o -name '*.gdextension' -print | LC_ALL=C sort)

mkdir -p .godot
if [ "${#extensions[@]}" -gt 0 ]; then
    printf '%s\n' "${extensions[@]}" > .godot/extension_list.cfg
else
    rm -f .godot/extension_list.cfg
fi
echo "GDExtensions listed for startup: ${extensions[*]:-none}"

# EditorPaths::get_cache_dir() for a non-self-contained editor.
case "$(uname -s)" in
    Darwin) cache_dir="$HOME/Library/Caches/Godot" ;;
    MINGW* | MSYS* | CYGWIN*) cache_dir="$(cygpath -u "${LOCALAPPDATA:-}")/Godot" ;;
    *) cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/godot" ;;
esac
rm -f "$cache_dir"/editor_doc_cache-*.res

godot="${GODOT_BIN:-godot}"
args=("$@")
for ((i=0; i<${#args[@]}-1; i++)); do
    if [[ "${args[i]}" = --export-* && "${args[i+1]}" = Android ]]; then
        prepare_args=()
        if [ "${args[i]}" = --export-debug ]; then prepare_args+=(--debug); fi
        if [ "${args[i]}" != --export-pack ]; then
            python3 "$root/tools/godot_template.py" prepare-android "${prepare_args[@]}"
        fi
        break
    fi
done
is_import=false
for arg in "$@"; do
    if [ "$arg" = --import ]; then is_import=true; fi
done
if [ "$is_import" = false ]; then
    exec "$godot" --headless --path . "$@"
fi

# An import also checks that the first fix held: the reload it prevents logs
# "Reloading all scripts" under --verbose.
log=.godot/import.log
status=0
"$godot" --headless --verbose --path . "$@" 2>&1 | tee "$log" || status=$?
if grep -q "Reloading all scripts" "$log"; then
    echo "warning: the import discovered a GDExtension on its first scan;" \
        "is its library for this editor built?" >&2
fi
# The crash comes after the import has finished, so one more run (now with a
# warm .godot) only confirms it. Retry that exit only, once, and say so.
if [ "$status" -eq 139 ]; then
    echo "warning: Godot segfaulted after importing (godotengine/godot#111048); retrying once" >&2
    rm -f "$cache_dir"/editor_doc_cache-*.res
    status=0
    "$godot" --headless --path . "$@" || status=$?
fi
exit "$status"

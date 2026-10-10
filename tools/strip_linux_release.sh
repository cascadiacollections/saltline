#!/usr/bin/env bash
set -euo pipefail

# Run only on a native Linux build. Keep the dynamic exports (Godot's entry
# point) and unwind metadata; remove symbols unused by the runtime loader.
library=${1:-target/release/libsalt_line_godot.so}
export LC_ALL=C
before_size=$(stat -c %s "$library")
before_exports=$(nm --dynamic --defined-only "$library")
if ! awk '$NF == "gdext_rust_init" { found = 1 } END { exit !found }' <<< "$before_exports"; then
    echo "missing Godot entry point: $library" >&2
    exit 1
fi
strip --strip-unneeded "$library"
after_exports=$(nm --dynamic --defined-only "$library")
if [ "$before_exports" != "$after_exports" ]; then
    echo "stripping changed dynamic exports: $library" >&2
    exit 1
fi
after_size=$(stat -c %s "$library")
if [ "$after_size" -gt "$before_size" ]; then
    echo "stripping increased the library size: $library" >&2
    exit 1
fi
printf 'Linux GDExtension: %s -> %s bytes (%s saved)\n' \
    "$before_size" "$after_size" "$((before_size - after_size))"

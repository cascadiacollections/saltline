#!/usr/bin/env bash
# Run the profiling build's GPU check (crates/salt-line-godot/src/profiling.rs,
# GpuCheck) on the desktop and fail unless every scene's frames read back from
# the GPU equal the flat frame drawn on the CPU, byte for byte.
#
# Needs a real GL context (an X or Wayland display; under CI, xvfb-run with
# Mesa's llvmpipe) and the profiling GDExtension where the editor loads it:
#   cargo build -p salt-line-godot --features profiling
#   bash tools/godot_headless.sh --import
#   bash tools/gpu_check_desktop.sh [log]
# GODOT_BIN picks the editor binary; GPU_CHECK_TIMEOUT caps the run (seconds).
set -euo pipefail
cd "$(dirname "$0")/.."
godot=${GODOT_BIN:-godot}
log=${1:-gpu-check.log}
limit=${GPU_CHECK_TIMEOUT:-1200}
scenes=3
verdicts() { grep -c 'salt-profile: gpu check scene=.* \(PASS\|FAIL\)' "$log" || true; }

# The profiling build plays itself: a timed capture, then each check scene.
"$godot" --path . --resolution 1280x720 >"$log" 2>&1 &
game=$!
deadline=$((SECONDS + limit))
while [ $SECONDS -lt $deadline ] && [ "$(verdicts)" -lt $scenes ]; do
    kill -0 "$game" 2>/dev/null || break
    sleep 5
done
# It keeps running after the last scene; stop it on our terms.
kill "$game" 2>/dev/null || true
sleep 2
kill -9 "$game" 2>/dev/null || true
wait "$game" 2>/dev/null || true

grep -E 'OpenGL API|salt-profile: gpu check scene=.* (PASS|FAIL)' "$log" | cut -c1-400 || true
if ! grep -q 'salt-profile: deterministic profiling run' "$log"; then
    echo "not the profiling build: build salt-line-godot with --features profiling" >&2
    exit 1
fi
passed=$(grep -c 'salt-profile: gpu check scene=.* PASS' "$log" || true)
if [ "$passed" -ne $scenes ]; then
    echo "GPU check: $passed of $scenes scenes passed (log: $log)" >&2
    exit 1
fi
echo "GPU check: all $scenes scenes exact"

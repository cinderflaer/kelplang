#!/usr/bin/env bash
# Polyp's headless smoke test: build examples/polyp-counter-v0 with the default
# backend and run it under Xvfb, where the program drives itself.
#
# The run is real: a GTK window, real click-callback thunks, the real queue. The
# only thing missing is a user, so with POLYP_SMOKE=1 the program schedules its
# own input (Inc, Inc, Dec, quit) and prints the final count. stdout must be
# exactly "1".
#
# The toolchain lives in the GTK shell next to this repository. Without
# POLYP_IN_SHELL this script re-enters it; POLYP_DEV_GTK overrides the path.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DEV_GTK=${POLYP_DEV_GTK:-$(dirname "$ROOT")/dev-gtk.nix}

if [ "${POLYP_IN_SHELL:-}" != "1" ]; then
    exec nix-shell "$DEV_GTK" --run \
        "POLYP_IN_SHELL=1 POLYP_DEV_GTK='$DEV_GTK' '$ROOT/examples/polyp-counter-v0/smoke.sh'"
fi

cd "$ROOT"

# The default backend, whatever the invoking shell exported: the retired backend variables
# belongs to the compiler's own suite, not to this proof.
unset KELPC_ORACLE KELPC_BACKEND

# GTK finds its modules and GSettings schemas through its own prefix's data
# directories, and a bare Xvfb session sets neither.
GTK_PREFIX=$(pkg-config --variable=prefix gtk4)
export XDG_DATA_DIRS="$GTK_PREFIX/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
if [ -d "$GTK_PREFIX/share/gsettings-schemas" ]; then
    GSETTINGS_SCHEMA_DIR=$(ls -d "$GTK_PREFIX"/share/gsettings-schemas/*/ | head -1)
    export GSETTINGS_SCHEMA_DIR
fi

cargo build --quiet --bin kelpc
KELPC=${KELPC:-$ROOT/target/debug/kelpc}

BIN=${TMPDIR:-/tmp}/polyp-counter-v0-smoke
"$KELPC" build examples/polyp-counter-v0 -o "$BIN"

# stdout only: GTK's own complaints (the EGL/DRI3 warning Xvfb always produces)
# go to stderr and are not the program's answer.
out=$(xvfb-run -a env GSK_RENDERER=cairo GDK_BACKEND=x11 POLYP_SMOKE=1 timeout 60 "$BIN")

if [ "$out" != "1" ]; then
    echo "smoke: expected stdout '1', got '$out'" >&2
    exit 1
fi
echo "smoke: ok (counter ended at 1)"

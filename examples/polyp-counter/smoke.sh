#!/usr/bin/env bash
# Polyp's headless smoke test: build examples/polyp-counter with the default
# backend and run it under Xvfb, where the program drives itself.
#
# The run is real: a GTK window, real click-callback thunks, the real queue, and
# an entry whose text is set through its own `changed` signal. The only thing
# missing is a user, so with POLYP_SMOKE=1 the program schedules its own input
# (Inc, Inc, Dec, set the entry, quit) and prints the final count, the entry's
# text, and how many entry widgets were ever built. stdout must be exactly
# "1\ntyped\n1": the count after the clicks, the text the entry posted, and one
# entry (a rebuild that reused it) rather than one per render.
#
# The toolchain lives in the GTK shell next to this repository. Without
# POLYP_IN_SHELL this script re-enters it; POLYP_DEV_GTK overrides the path.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DEV_GTK=${POLYP_DEV_GTK:-$(dirname "$ROOT")/dev-gtk.nix}

if [ "${POLYP_IN_SHELL:-}" != "1" ]; then
    exec nix-shell "$DEV_GTK" --run \
        "POLYP_IN_SHELL=1 POLYP_DEV_GTK='$DEV_GTK' '$ROOT/examples/polyp-counter/smoke.sh'"
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

BIN=${TMPDIR:-/tmp}/polyp-counter-smoke
"$KELPC" build examples/polyp-counter -o "$BIN"

# stdout only: GTK's own complaints (the EGL/DRI3 warning Xvfb always produces)
# go to stderr and are not the program's answer.
out=$(xvfb-run -a env GSK_RENDERER=cairo GDK_BACKEND=x11 POLYP_SMOKE=1 timeout 60 "$BIN")

expected=$'1\ntyped\n1'
if [ "$out" != "$expected" ]; then
    echo "smoke: expected stdout '$expected', got '$out'" >&2
    exit 1
fi
echo "smoke: ok (counter ended at 1, entry text 'typed', one entry built)"

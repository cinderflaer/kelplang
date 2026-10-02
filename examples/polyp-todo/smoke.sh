#!/usr/bin/env bash
# Polyp v2's headless smoke test: build examples/polyp-todo and run it under
# Xvfb, where the program drives itself, on both backends.
#
# The run is real: a GTK window, real click-callback thunks, the real queue, and
# an entry whose text is set through its own `changed` signal. With
# POLYP_SMOKE=1 the program schedules its own input: it focuses the entry, types
# three titles, adds them, ticks the second row, moves the third row up, removes
# the row that is now the middle one, and quits.
#
# What the script asserts are the library's own counters, not guesses:
#   - the final model is two items, `alpha` not done and `beta` done;
#   - `created` grows by five per added row and then does not move again while
#     the list is ticked, reordered and shortened, which is the whole point of
#     a patch: a rebuild-everything implementation keeps creating widgets and
#     fails the `marks` equality below;
#   - one row was removed, and `live` is `created` minus it;
#   - the focus was present and survived every patch;
#   - one entry widget was ever built, and it still shows the last text typed
#     into it even though the model cleared its draft.
#
# Both backends run: the default (`llvm`) one and the C oracle, and their output
# has to pass the same checks.
#
# The toolchain lives in the GTK shell next to this repository. Without
# POLYP_IN_SHELL this script re-enters it; POLYP_DEV_GTK overrides the path.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DEV_GTK=${POLYP_DEV_GTK:-$(dirname "$ROOT")/dev-gtk.nix}

if [ "${POLYP_IN_SHELL:-}" != "1" ]; then
    exec nix-shell "$DEV_GTK" --run \
        "POLYP_IN_SHELL=1 POLYP_DEV_GTK='$DEV_GTK' '$ROOT/examples/polyp-todo/smoke.sh'"
fi

cd "$ROOT"

# The default backend, whatever the invoking shell exported: the oracle switch
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

check() {
    local label=$1
    local out=$2
    python3 - "$label" "$out" <<'PY'
import sys

label, out = sys.argv[1], sys.argv[2]
# The last value is the model's draft, which is empty here; `splitlines` would
# drop it, so split explicitly and keep the empty tail.
lines = out.rstrip("\n").split("\n")


def bad(msg):
    print(f"smoke[{label}]: {msg}", file=sys.stderr)
    sys.exit(1)


if len(lines) != 21:
    bad(f"expected 21 lines, got {len(lines)}: {lines!r}")

# Final model: two items, alpha not done and beta done.
if lines[0] != "2":
    bad(f"expected 2 items, got {lines[0]!r}")
if lines[1:5] != ["alpha", "false", "beta", "true"]:
    bad(f"wrong final items: {lines[1:5]!r}")

# The creation marks: one per action, recorded before that action's patch. Each
# add is followed by a patch that creates its five widgets, so the count grows
# across the first four marks; ticking, moving and removing then create nothing.
# A rebuild pass would keep growing the count and fail the equality below.
marks = [int(x) for x in lines[5:11]]
if not (marks[0] < marks[1] < marks[2] < marks[3]):
    bad(f"each add should create widgets, marks={marks!r}")
if marks[4] != marks[3] or marks[5] != marks[3]:
    bad(f"ticking, moving and removing must create nothing, marks={marks!r}")

created = int(lines[11])
updated = int(lines[12])
removed = int(lines[13])
live = int(lines[14])
if created != marks[5]:
    bad(f"nothing should be created after the last mark: created={created} marks={marks!r}")
if removed != 5:
    bad(f"one five-widget row was removed, got removed={removed}")
if live != created - removed:
    bad(f"live must be created minus removed: {live} != {created} - {removed}")
if updated <= 0:
    bad(f"a patch should have updated widgets, updated={updated}")

# Focus and the entry's text.
if lines[15] != "true":
    bad(f"no focus was ever present: focus_present={lines[15]!r}")
if lines[16] != "true":
    bad(f"a patch dropped the focus: focus_survived={lines[16]!r}")
if lines[17] != "1":
    bad(f"one entry widget must be built across the run, got {lines[17]!r}")
if lines[18] != "gamma":
    bad(f"the entry should still show the last text typed, got {lines[18]!r}")
if lines[19] != "":
    bad(f"the model's draft should be empty after the last add, got {lines[19]!r}")
if lines[20] != "end":
    bad(f"the program did not finish: last line {lines[20]!r}")
PY
}

run() {
    local label=$1
    local bin="$ROOT/target/polyp-todo-$label"
    "$KELPC" build examples/polyp-todo -o "$bin"
    # stdout only: GTK's own complaints (the EGL/DRI3 warning Xvfb always
    # produces) go to stderr and are not the program's answer.
    local out
    out=$(xvfb-run -a env GSK_RENDERER=cairo GDK_BACKEND=x11 POLYP_SMOKE=1 timeout 60 "$bin")
    check "$label" "$out"
    echo "smoke: $label ok"
    printf '%s\n' "$out" | sed 's/^/    /'
}

run llvm
KELPC_ORACLE=c run c-oracle

#!/bin/sh
# Managed-mode differential check (WP4). Runs every corpus program in corpus/ with the hosted build
# and compares stdout, stderr and exit status with the recorded expectation. When a managed-capable
# kelpc is named, it also builds each program in managed mode and requires the same three things,
# byte for byte, as the hosted run.
#
#   sh kelpc/tests/managed/differential.sh             hosted-only check against expected/
#   KELPC_MANAGED=/path/to/kelpc MANAGED_FLAGS=... sh kelpc/tests/managed/differential.sh
#   RECORD=1 sh kelpc/tests/managed/differential.sh    rewrite expected/ from the hosted build
#
# Environment: KELPC_HOSTED (default: the 26.10.5 release asset), KELPC_MANAGED (unset means managed
# is not yet available and is reported as such, not as a failure), MANAGED_FLAGS (extra build flags
# that select managed mode, for example the flag WP2 defines; empty until then), OUT (scratch dir),
# PROGRAM_TIMEOUT (seconds per run, default 60).
#
# Builds run one at a time under the shared build lock; runs are not locked. Exit status is 1 when a
# hosted or managed comparison fails.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
CORPUS="$HERE/corpus"
EXPECTED="$CORPUS/expected"
LOCK=${LOCK:-/home/juniper/Projects/kelp-work/.build.lock}
KELPC_HOSTED=${KELPC_HOSTED:-/home/juniper/Projects/kelp-work/release-assets/kelpc-26.10.5-linux-x86_64}
KELPC_MANAGED=${KELPC_MANAGED:-}
MANAGED_FLAGS=${MANAGED_FLAGS:-}
OUT=${OUT:-${TMPDIR:-/tmp}/kelp-gc-differential}
PROGRAM_TIMEOUT=${PROGRAM_TIMEOUT:-60}
RECORD=${RECORD:-}
export PATH=/nix/store/79mr0jw3qccq7hhf1hh62knxd88dwazc-gcc-wrapper-15.3.0/bin:$PATH

mkdir -p "$OUT" "$EXPECTED"
fail=0
managed_ok=0
[ -n "$KELPC_MANAGED" ] && [ -x "$KELPC_MANAGED" ] && managed_ok=1
[ -n "$KELPC_MANAGED" ] && [ "$managed_ok" = 0 ] && { echo "differential: KELPC_MANAGED=$KELPC_MANAGED is not executable" >&2; exit 1; }
[ -x "$KELPC_HOSTED" ] || { echo "differential: no hosted kelpc at $KELPC_HOSTED" >&2; exit 1; }

# run_prog BIN OUTPREFIX: runs BIN once, writing OUTPREFIX.out, .err and .rc
run_prog() {
    timeout "$PROGRAM_TIMEOUT" "$1" > "$2.out" 2> "$2.err"
    echo $? > "$2.rc"
}

for src in "$CORPUS"/*.kp; do
    name=$(basename "$src" .kp)
    h="$OUT/$name.hosted"
    flock "$LOCK" timeout 300 "$KELPC_HOSTED" build -O2 "$src" -o "$h.bin" > "$h.build" 2>&1 || {
        echo "FAIL $name: hosted build failed (see $h.build)"; fail=1; continue; }
    run_prog "$h.bin" "$h"

    if [ -n "$RECORD" ]; then
        cp "$h.out" "$EXPECTED/$name.out"
        echo "recorded $name ($(wc -l < "$EXPECTED/$name.out") lines, rc=$(cat "$h.rc"))"
        continue
    fi

    if [ "$(cat "$h.rc")" != 0 ] || [ -s "$h.err" ] || ! cmp -s "$h.out" "$EXPECTED/$name.out"; then
        echo "FAIL $name: hosted output differs from expected/ (rc=$(cat "$h.rc"))"
        fail=1
        continue
    fi
    hosted_status="hosted ok"

    if [ "$managed_ok" = 1 ]; then
        m="$OUT/$name.managed"
        flock "$LOCK" timeout 300 "$KELPC_MANAGED" build $MANAGED_FLAGS -O2 "$src" -o "$m.bin" > "$m.build" 2>&1 || {
            echo "FAIL $name: managed build failed (see $m.build)"; fail=1; continue; }
        run_prog "$m.bin" "$m"
        if cmp -s "$h.out" "$m.out" && cmp -s "$h.err" "$m.err" && [ "$(cat "$h.rc")" = "$(cat "$m.rc")" ]; then
            echo "PASS $name: $hosted_status, managed identical"
        else
            echo "FAIL $name: managed differs from hosted (stdout, stderr or exit status)"
            fail=1
        fi
    else
        echo "PASS $name: $hosted_status, managed not yet available"
    fi
done

if [ -n "$RECORD" ]; then exit 0; fi
[ "$managed_ok" = 1 ] || echo "managed side: not run (set KELPC_MANAGED and MANAGED_FLAGS once WP2/WP3 land)"
[ "$fail" = 0 ] && echo "differential: all green" || echo "differential: FAILURES"
exit "$fail"

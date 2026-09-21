#!/bin/sh
# Runs the end-to-end fixture suite once per implemented backend.
#
# The suite is backend-agnostic by construction: it compiles a fixture, runs the
# result and compares stdout, so pointing it at a backend validates that backend
# with no test edits. Running every backend on every change is what keeps the C
# backend usable as the differential oracle: a backend only earns its place by
# agreeing with it, and agreement is only known if both are run.
#
# `KELPC_ORACLE` is the project's one backend switch, and it names only the C
# oracle; unsetting it selects the default `llvm` backend. This script is its
# only caller outside the tests, and the suite's own `backend()` reads the same
# variable, so the run and the builds it spawns cannot disagree about which
# backend is under test.
#
# `BACKENDS` overrides the list. `llvm` is the compiler's default since MK7 S7
# and lowers the whole corpus, so `BACKENDS="c llvm"` runs both and is the
# default here: after the flip the oracle is no longer what a plain run
# exercises, so running only `llvm` would stop checking the backend every other
# one is compared against. The per-slice gates are the separate `run-llvm-*.sh`
# scripts.
#
# Usage, from the repository root:  ./kelpc/tests/run-all-backends.sh
set -e

BACKENDS="${BACKENDS:-c llvm}"

# One command under one backend: `c` sets the oracle switch, anything else
# removes it so the default backend runs. The retired `KELPC_BACKEND` is stripped
# either way, so a developer's stale export cannot turn every build into the
# refusal that variable now earns.
run_backend() {
    if [ "$backend" = "c" ]; then
        env -u KELPC_BACKEND KELPC_ORACLE=c "$@"
    else
        env -u KELPC_BACKEND -u KELPC_ORACLE "$@"
    fi
}

fail=0
for backend in $BACKENDS; do
    printf '\n=== backend: %s ===\n' "$backend"
    if run_backend cargo test --test compile_and_run; then
        printf '=== backend %s: ok ===\n' "$backend"
    else
        printf '=== backend %s: FAILED ===\n' "$backend"
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    echo "one or more backends failed"
fi
exit "$fail"

#!/bin/sh
# Runs the end-to-end fixture suite once per implemented backend.
#
# The suite is backend-agnostic by construction: it compiles a fixture, runs the
# result and compares stdout, so pointing `KELPC_BACKEND` at a second backend
# validates it with no test edits. Running every backend on every change is what
# keeps the C backend usable as the differential oracle: a backend only earns
# its place by agreeing with it, and agreement is only known if both are run.
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

fail=0
for backend in $BACKENDS; do
    printf '\n=== backend: %s ===\n' "$backend"
    if KELPC_BACKEND="$backend" cargo test --test compile_and_run; then
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

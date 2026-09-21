#!/bin/sh
# Runs the end-to-end fixture suite once per implemented backend.
#
# The suite is backend-agnostic by construction: it compiles a fixture, runs the
# result and compares stdout, so pointing `KELPC_BACKEND` at a second backend
# validates it with no test edits. Running every backend on every change is what
# keeps the C backend usable as the differential oracle: a backend only earns
# its place by agreeing with it, and agreement is only known if both are run.
#
# `BACKENDS` overrides the list. `llvm` is selectable at MK7 S1 (the scalar
# subset; everything else is refused by name), so `BACKENDS="c llvm"` runs both.
# The default stays `c`: the C backend is the oracle every other backend is
# checked against, and the S1 subset gate is the separate script
# `run-llvm-scalar-subset.sh`.
#
# Usage, from the repository root:  ./kelpc/tests/run-all-backends.sh
set -e

BACKENDS="${BACKENDS:-c}"

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

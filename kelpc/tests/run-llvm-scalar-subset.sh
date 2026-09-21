#!/bin/sh
# The MK7 S1 gate: runs the scalar-subset fixtures against the `llvm` backend.
#
# S1 (plan/mk7-llvm-backend.md §5) is "IR for the scalar subset, `llvm`
# selectable", and its exit criterion is byte-identical stdout on the fixtures
# that use only arithmetic, comparisons, `if`/`while`, calls and `print` of the
# scalars. This script names exactly that set.
#
# The whole suite is backend-agnostic by construction, so the rest of
# `compile_and_run.rs` runs against `llvm` too, one backend at a time, through
# `run-all-backends.sh` (`BACKENDS="c llvm"`). What this script adds is the
# *closed* S1 list: a fixture that has grown a construct outside the subset is
# refused by name, and a refusal is the correct S1 answer, not a failure to hide.
# Tying the gate to names rather than to a substring means a later fixture cannot
# drift into it silently.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-scalar-subset.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# Exactly the S1 fixtures, by test function. `hello.kp` and `fib.kp` are under
# `examples/`; the rest are under `kelpc/tests/fixtures/`.
TESTS="
hello_prints_greeting
fib_computes_correctly
if_expression_classifies_correctly
while_loop_accumulates_correctly
nested_while_keeps_the_outer_accumulator_live
named_and_positional_args_match_correctly
integer_conversions_widen_exactly_and_narrow_with_a_check
remainder_works_at_every_integer_width
float_arithmetic_comparisons_and_unordered_nan
integer_to_float_conversions_widen_and_round
a_non_finite_float_literal_emits_a_math_h_constant
a_const_may_call_a_function_and_the_call_is_evaluated_at_compile_time
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains an S1 name out of the gate.
# shellcheck disable=SC2086
KELPC_BACKEND="$BACKEND" cargo test --release --test compile_and_run -- \
    --exact $TESTS

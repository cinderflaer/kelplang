#!/bin/sh
# The MK7 S2A gate: runs the `datac` fixtures against the `llvm` backend.
#
# S2 is "aggregates, containers and drops" (the MK7 record in kelp wiki page meta/milestones.md), and
# **S2A is its plain-record slice**: a `datac` whose fields are all scalars or
# earlier all-scalar `datac`s, lowered to an identified struct with the same
# field order and natural alignment the C typedef has. Its exit criterion is
# byte-identical stdout on the fixtures that use only that: the struct type, a
# literal, a field read, a field write, a whole-value assignment, a struct as a
# local, a `move` parameter, a return value, a nested struct, and the shared or
# exclusive borrow a `datac` parameter resolves to.
#
# This script names the closed S2A list: the S1 scalar fixtures, which must keep
# passing, plus every positive fixture whose source declares a `datac` that this
# slice now lowers. A fixture that has grown a construct outside the slice is
# refused by name and is deliberately *not* here: an owned field, an `enum`, a
# container, an array, a `pd` and the runtime all refuse until their own slice
# (S2B, S2C, S2D, S3), and a refusal is the correct S2A answer, not a failure to
# hide.
# Tying the gate to names rather than to a substring means a later fixture cannot
# drift into it silently.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-datac-subset.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# The retired KELPC_BACKEND and KELPC_ORACLE are stripped, so a stale export
# cannot refuse a build.
backend_env() {
    env -u KELPC_BACKEND -u KELPC_ORACLE "$@"
}

# The S1 fixtures (kelpc/tests/run-llvm-scalar-subset.sh) plus the S2A `datac`
# fixtures. `hello.kp` and `fib.kp` are under `examples/`; the rest are under
# `kelpc/tests/fixtures/`.
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
datac_fields_and_mutation_work
a_datac_can_embed_an_earlier_datac
a_two_eightbyte_datac_passes_and_returns_in_a_register_pair
a_memory_class_datac_returns_through_a_hidden_pointer
a_memory_class_datac_passes_by_value_in_the_argument_area
a_float_field_datac_travels_in_the_vector_register_file
field_read_does_not_move_the_container
move_then_reinit_is_allowed
receiver_methods_resolve_and_mutate_through_self
a_float_field_is_written_through_the_xmm_file
borrow_basic_shared_and_exclusive_work
a_shared_borrow_of_a_temporary_is_allowed_for_a_call
each_borrowed_temporary_gets_a_slot_of_its_own
templates_print_literals_and_values
a_documentation_comment_is_a_comment_to_the_compiler
multi_file_project_with_manifest_runs
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
backend_env cargo test --release --test compile_and_run -- \
    --exact $TESTS

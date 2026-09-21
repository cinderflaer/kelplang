#!/bin/sh
# The MK7 S2B gate: runs the runtime, `String`/`Bytes` and owned-field drop
# fixtures against the `llvm` backend.
#
# S2 is "aggregates, containers and drops" (plan/mk7-llvm-backend.md §5). S2A
# was its plain-record slice; **S2B is its runtime and heap slice**: the C
# runtime object `runtime/kelp_rt.c` compiled once and linked into every LLVM
# binary, the `str`/`String`/`Bytes`/`View` representations, a `datac` with an
# owned field, and the drop plan emitted at the same points `codegen_c.rs`
# emits it. Its exit criterion is byte-identical stdout and exit status on the
# fixtures that use only those, plus the runtime's own panic and assertion
# paths.
#
# This script carries the S1 and S2A lists forward and adds the S2B fixtures, so
# one run is the whole opt-in surface at this stage. Its list is closed by
# `--exact`: a fixture that has grown a construct outside the slice is refused
# by name and is deliberately *not* here. Still outside: the containers
# (`List`/`Map`/`Box`/arrays) and `enum`/`when` are S2C, closures and function
# values are S2D, `pd` and `extern fun` are S3. A refusal is the correct S2B
# answer, not a failure to hide.
#
# `owned_datac_fields_are_dropped_with_the_struct` (`string_field.kp`) is the
# one that pins the move rule of MK7 §3.4: `val moved = outer` must free the
# owned `String` exactly once, with no second drop behind the moved-from
# binding. `a_moved_from_binding_can_be_reassigned` pins the same rule for a
# rebuilt binding, and `an_owned_local_dies_at_the_end_of_its_block` pins the
# block-scoped drop.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-strings-drops.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# The S1 scalar fixtures, the S2A `datac` fixtures, and the S2B heap fixtures.
# `hello.kp` and `fib.kp` are under `examples/`; the rest are under
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
owned_datac_fields_are_dropped_with_the_struct
a_float_field_datac_over_sixteen_bytes_is_memory_class
bytes_construct_measure_index_mutate_clone_and_print
bytes_grow_by_push_and_append
an_owned_local_dies_at_the_end_of_its_block
a_moved_from_binding_can_be_reassigned
an_owned_value_can_be_returned_by_a_function
reassigning_an_owned_binding_frees_the_old_value
a_bytes_parameter_is_a_borrow_not_a_move
a_string_grows_in_place_and_survives_growth
drops_follow_control_flow
panic_reports_its_message_and_assert_reports_failure
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
KELPC_BACKEND="$BACKEND" cargo test --release --test compile_and_run -- \
    --exact $TESTS

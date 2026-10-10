#!/bin/sh
# The MK7 S2F gate: runs the reference fixtures (a `&T` to a scalar, a borrow of
# a field, the read and the write through one, a `&T` parameter and a `&T`
# return) against the `llvm` backend.
#
# S2 is "aggregates, containers and drops" (the MK7 record in plan/done.md), and
# **S2F is its reference slice**: every borrow that is not a `datac`, a
# container, a heap type or `str`. `Ty::Ref` is one pointer whatever the pointee
# (MK7 §3.1): a `mut i32` parameter, an `&i32` local and a borrow a container
# method returns all lower to the pointee's address, and the load through one is
# the pointee type's own load. `str` and `View` are pointer-plus-length and were
# already handled, so this slice must not disturb them.
#
# This script carries the S1, S2A, S2B, S2C and S2D lists forward and adds the
# S2F fixtures, so one run is the whole opt-in surface at this stage. Its list is
# closed by `--exact`: a fixture that has grown a construct outside the slice is
# refused by name and is deliberately *not* here. Still outside: closures and
# function values are S2E, the runtime, `extern fun` and `pd` are S3. A refusal
# is the correct answer for those, not a failure to hide.
#
# The fixtures that make this slice worth its own gate:
#
#   * `a_scalar_local_can_be_borrowed_and_written_through_the_pointer`
#     (`scalar_borrow.kp`) is the whole slice in one program: a `mut i64`
#     parameter, an `&mut x` binding, the read `*p`, the store `*p = v`, and a
#     pointer held across a call. A borrowed field would be a *place* the
#     backend did not address before.
#   * `a_float_scalar_can_be_borrowed_and_written_through_the_pointer`
#     (`scalar_borrow_f64.kp`) is the float half: only the load and the store
#     change register file, so a wrong one prints a plausible wrong number.
#   * `a_function_may_return_a_borrow_of_any_type` (`borrow_returned.kp`) is the
#     `&T` return value, for a `datac`, a `String` and a `Bytes`. It built
#     before this slice only incidentally, through the S2D reference-place work;
#     it is in the gate now so it is a tested shape rather than an accident.
#   * `an_explicit_borrow_of_a_field_of_a_borrow_is_accepted`
#     (`borrow_chain_through_a_borrow.kp`) is the borrow of a scalar *field*
#     reached through a container method's returned borrow (`&xs.get(i).v`).
#   * `a_moved_parameter_is_dropped_on_every_path_out` (`move_param_return.kp`)
#     and `a_panicking_tail_satisfies_any_return_type` (`panic_tail`) are
#     BUG-b2Ldp, the two fixtures that used to die with the backend's own
#     "internal error" instead of building. A partial backend refuses by name
#     and never reports an internal error, so they are in the gate now that
#     they build.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-references.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# The retired KELPC_BACKEND and KELPC_ORACLE are stripped, so a stale export
# cannot refuse a build.
backend_env() {
    env -u KELPC_BACKEND -u KELPC_ORACLE "$@"
}

# The S1 scalar fixtures, the S2A `datac` fixtures, the S2B runtime fixtures,
# the S2C tagged-union fixtures, the S2D container fixtures, and the S2F
# reference fixtures. `hello.kp` and `fib.kp` are under `examples/`; the rest
# are under `kelpc/tests/fixtures/`.
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
an_enum_matches_on_its_live_variant
an_owned_payload_binds_as_a_borrow
an_enum_payload_is_laid_out_the_way_the_c_typedef_is
the_enum_node_kinds_keep_two_live_locals_in_different_registers
an_enum_payload_eightbyte_is_classified_through_the_union
when_desugars_to_if_else_chain
when_arms_do_not_need_a_newline_inside_parentheses
when_a_temporary_scrutinee_is_staged_into_a_local
when_a_field_of_a_temporary_is_staged_at_its_root
if_let_binds_one_variant_and_else_covers_the_rest
if_let_else_chain_reads_its_scrutinee_more_than_once
an_else_if_let_behind_a_plain_if_compiles
try_propagates_a_failure_out_of_the_enclosing_function
a_propagated_failure_frees_the_frame_it_leaves
a_one_eightbyte_try_payload_builds_without_a_destination
a_generic_datac_and_enum_are_monomorphized_per_use_site
a_generic_function_works_over_an_owned_type
explicit_type_arguments_are_written_at_the_call
a_borrow_cannot_be_a_list_element
a_borrow_cannot_be_an_array_element
a_box_cannot_hold_a_borrow
a_box_holds_a_value_or_nothing
a_cleared_map_is_empty_and_reusable
a_container_cannot_be_a_list_element
a_datac_can_be_a_list_element
a_datac_can_be_recursive_through_a_box
a_datac_can_be_recursive_through_a_list_and_mutually_recursive_through_boxes
a_fixed_size_array_is_a_value_and_its_length_is_in_its_type
a_float_element_and_a_memory_class_array_are_values_too
a_float_field_datac_crosses_the_hand_written_container_boundary
a_for_loop_needs_its_in
a_for_loop_only_walks_a_list
a_generic_type_can_be_recursive_through_a_box
a_list_copies_elements_at_their_own_width
a_list_grows_indexes_and_shrinks
a_list_grows_past_several_doublings_and_reuses_its_buffer
a_list_inserts_and_removes_by_shifting_owned_elements
a_list_returns_a_narrow_element_at_its_own_width
a_list_sorts_and_searches_every_element_kind_it_can_hold
a_list_sorts_reverses_searches_and_slices
a_map_cannot_hold_a_container_or_be_created_without_a_value_type
a_map_entry_moves_the_value_pointer_with_the_key_width
a_map_grows_by_rehashing_and_keeps_insertion_order
a_map_is_not_copy_so_assigning_one_moves_it
a_map_key_can_be_a_scalar_and_the_shorthand_still_means_string
a_map_probe_steps_over_a_tombstone_and_still_finds_the_live_key
an_array_element_may_own_memory_a_datac_or_another_array
an_array_length_that_is_not_a_constant_is_rejected
an_array_literal_of_the_wrong_length_is_rejected
an_empty_container_takes_its_element_from_the_first_use
an_empty_list_needs_its_element_type
an_enum_is_a_container_element_and_a_container_drops_it
an_enum_is_a_list_element_a_map_value_and_a_box_element
an_owned_field_in_a_box_can_be_taken_and_set
a_recursive_enum_nests_through_a_box
a_repeat_literal_over_an_owning_element_is_rejected
a_view_cannot_be_a_list_element
box_owned_elements_are_freed_once_when_replaced_or_taken
list_sort_and_slice_reject_what_they_cannot_do
nested_aggregate_literals_are_staged_in_slots_of_their_own
pushing_an_owned_element_moves_it
setting_an_owned_element_frees_the_one_it_replaces
sorting_and_slicing_a_list_of_owned_elements_moves_them_safely
a_scalar_local_can_be_borrowed_and_written_through_the_pointer
a_float_scalar_can_be_borrowed_and_written_through_the_pointer
a_function_may_return_a_borrow_of_any_type
an_explicit_borrow_of_a_field_of_a_borrow_is_accepted
a_moved_parameter_is_dropped_on_every_path_out
a_panicking_tail_satisfies_any_return_type
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
backend_env cargo test --release --test compile_and_run -- \
    --exact $TESTS

# The reference shapes those fixtures exercise, read back out of the IR the
# backend writes beside the binary (`<out>.ll`, MK7 §4.4).
#
# A `&T` is one `ptr` whatever the pointee (MK7 §3.1), so `mut i64`/`mut f64`
# parameters are `ptr` and a `&T` return is `ptr`. A backend that modelled the
# borrow as a value, or that widened the pointee, changes one of these lines, and
# the fixture that prints the wrong bytes would only sometimes notice.
#
# The panic tail is the BUG-b2Ldp shape: `panic` ends the block with a call and
# an `unreachable`, and no `ret` follows it, so the value-returning function is
# terminated by the divergence rather than by a fabricated value.
#
# `cargo test` above leaves the compiler binary in `target/release`; this uses it
# directly rather than through the harness because the assertion is about the
# generated IR, which the harness does not expose.
REF_OUT="$(mktemp -d)"
trap 'rm -rf "$REF_OUT"' EXIT
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/scalar_borrow.kp -o "$REF_OUT/scalar" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/scalar_borrow_f64.kp -o "$REF_OUT/scalar_f64" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/borrow_returned.kp -o "$REF_OUT/returned" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/panic_tail.kp -o "$REF_OUT/panic" >/dev/null
# MK7 S4 attaches a `!dbg` location to every instruction and every function
# definition. The assertions below are about the shape of the instruction, not
# about the source line it came from, so each IR file is read through a copy
# with the debug metadata stripped. That a statement resolves to a `.kp` line is
# a separate claim, checked end to end by `cargo test --test llvm_debug_info`.
strip_dbg() {
    sed -E 's/, !dbg ![0-9]+//g; s/ !dbg ![0-9]+//g' "$1" > "$1.nodbg"
    echo "$1.nodbg"
}

SCALAR_IR="$(strip_dbg "$REF_OUT/scalar.ll")"
F64_IR="$(strip_dbg "$REF_OUT/scalar_f64.ll")"
RETURNED_IR="$(strip_dbg "$REF_OUT/returned.ll")"
PANIC_IR="$(strip_dbg "$REF_OUT/panic.ll")"
# A `mut` parameter is an exclusive borrow, so the signature marks it
# `noalias` (`codegen_llvm::declare`), on a scalar borrow as on an array.
for want in \
    'define void @kelp_add(ptr noalias %0, i64 %1) {' \
    'define i64 @kelp_accumulate(ptr noalias %0, i64 %1, i64 %2) {'
do
    if ! grep -qxF "$want" "$SCALAR_IR"; then
        echo "the scalar borrow in '$SCALAR_IR' is missing: $want" >&2
        exit 1
    fi
done
for want in \
    'define void @kelp_scale(ptr noalias %0, double %1) {'
do
    if ! grep -qxF "$want" "$F64_IR"; then
        echo "the float scalar borrow in '$F64_IR' is missing: $want" >&2
        exit 1
    fi
done
for want in \
    'define ptr @kelp_origin(ptr %0) {' \
    'define ptr @kelp_pick(ptr %0, i1 %1) {' \
    'define ptr @kelp_text(ptr %0) {' \
    'define ptr @kelp_buf(ptr %0) {'
do
    if ! grep -qxF "$want" "$RETURNED_IR"; then
        echo "the borrow return in '$RETURNED_IR' is missing: $want" >&2
        exit 1
    fi
done
# BUG-b2Ldp: the divergent tail is a `kelp_panic` call ending in `unreachable`,
# so the value-returning function is terminated by the divergence rather than by
# a fabricated value. `sed` bounds the assertion to this function's body.
if ! sed -n '/^define i64 @kelp_always_fails/,/^}/p' "$PANIC_IR" \
    | grep -q 'call void @kelp_panic(ptr @str)'; then
    echo "the panic tail in '$PANIC_IR' does not call kelp_panic" >&2
    exit 1
fi
if ! sed -n '/^define i64 @kelp_always_fails/,/^}/p' "$PANIC_IR" \
    | grep -q '^  unreachable$'; then
    echo "the panic tail in '$PANIC_IR' does not end unreachable" >&2
    exit 1
fi

echo "$BACKEND gate passed: $(printf '%s\n' $TESTS | wc -l) fixtures, and the scalar, float and borrow-return signatures plus the panic tail read back out of the IR"

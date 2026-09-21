#!/bin/sh
# The MK7 S2D gate: runs the container fixtures (`List<T>`, `Map<K, V>`,
# `Box<T>` and `[T; N]`) against the `llvm` backend.
#
# S2 is "aggregates, containers and drops" (plan/mk7-llvm-backend.md §5), and
# **S2D is its container slice**: the monomorphized function sets `codegen_c.rs`
# generates per element or value type, generated into the module as IR instead
# (the plan's §3.2 exception), with the same bodies, the same function names and
# the same bounds-check and panic text the C backend writes. A container header
# is a named struct whose field order and widths match the C typedefs field for
# field, but it never crosses to C by value: every runtime helper it calls takes
# a pointer (MK7 §3.3, BUG-vN4tR).
#
# This script carries the S1, S2A, S2B and S2C lists forward and adds the S2D
# fixtures, so one run is the whole opt-in surface at this stage. Its list is
# closed by `--exact`: a fixture that has grown a construct outside the slice is
# refused by name and is deliberately *not* here. Still outside: closures and
# function values are S2E, `pd` and `extern fun` are S3, so a container fixture
# that also calls `std/mem` (`map_basic`, `list_clone`, `list_owned`, ...) or
# reads a `pd` (`for_loop`, `list_field`) is refused and is not in this list. A
# refusal is the correct S2D answer, not a failure to hide.
#
# The fixtures that make this slice worth its own gate:
#
#   * `a_list_grows_past_several_doublings_and_reuses_its_buffer`
#     (`list_growth_and_rebuild.kp`) and `a_list_inserts_and_removes_by_shifting_owned_elements`
#     (`list_insert_remove_shifts.kp`) exercise `reserve`'s doubling loop and
#     the shifting `insert`/`remove`, which are the two places the C bodies do
#     arithmetic rather than a load or a store.
#   * `a_list_copies_elements_at_their_own_width` (`list_element_widths.kp`) is
#     the element-width fixture: `bool`, `i64` and a 12-byte `datac`, the
#     widths a four-byte `i32` list never reaches.
#   * `a_map_grows_by_rehashing_and_keeps_insertion_order`
#     (`map_growth_and_rehash.kp`) and `a_map_probe_steps_over_a_tombstone_and_still_finds_the_live_key`
#     (`map_remove_missing.kp`-adjacent) exercise the dense entries plus the
#     separate open-addressed index, including the `SIZE_MAX` tombstone.
#   * `an_array_element_may_own_memory_a_datac_or_another_array`
#     (`array_owned.kp`) is the inline `[T; N]`: no allocation in the path, an
#     owned element's drop, and a nested array.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-containers.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# KELPC_ORACLE is the project's one backend switch, and it names only the C
# oracle; unsetting it selects the default llvm backend. This wrapper keeps the
# gate's one-backend-at-a-time shape now that the user-facing selector is gone:
# BACKEND=c sets the switch and anything else removes it. The retired
# KELPC_BACKEND is stripped either way, so a stale export cannot refuse a build.
backend_env() {
    if [ "$BACKEND" = "c" ]; then
        env -u KELPC_BACKEND KELPC_ORACLE=c "$@"
    else
        env -u KELPC_BACKEND -u KELPC_ORACLE "$@"
    fi
}

# The S1 scalar fixtures, the S2A `datac` fixtures, the S2B runtime fixtures,
# the S2C tagged-union fixtures, and the S2D container fixtures. `hello.kp` and
# `fib.kp` are under `examples/`; the rest are under `kelpc/tests/fixtures/`.
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
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
backend_env cargo test --release --test compile_and_run -- \
    --exact $TESTS

# The container layouts those fixtures exercise, read back out of the IR the
# backend writes beside the binary (`<out>.ll`, MK7 §4.4), and the guarantee
# BUG-vN4tR demands: a container header never crosses to the C runtime by value.
#
# `float_datac_containers.kp` names a `List`, a `Box` and a `Map`, so its header
# shapes are the three pointer containers; `array_owned.kp` names the inline
# `[T; N]`, held as the array field of its wrapper struct. A wrong field, a
# reordered one or a wider one changes one of these lines, and only the fixture
# that prints the wrong bytes would otherwise notice.
#
# `cargo test` above leaves the compiler binary in `target/release`; this uses it
# directly rather than through the harness because the assertion is about the
# generated IR, which the harness does not expose.
CONTAINER_OUT="$(mktemp -d)"
trap 'rm -rf "$CONTAINER_OUT"' EXIT
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/float_datac_containers.kp -o "$CONTAINER_OUT/float" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/array_owned.kp -o "$CONTAINER_OUT/array" >/dev/null
# MK7 S4 attaches a `!dbg` location to every instruction and every function
# definition. The assertions below are about the shape of the instruction, not
# about the source line it came from, so each IR file is read through a copy
# with the debug metadata stripped. That a statement resolves to a `.kp` line is
# a separate claim, checked end to end by `cargo test --test llvm_debug_info`.
strip_dbg() {
    sed -E 's/, !dbg ![0-9]+//g; s/ !dbg ![0-9]+//g' "$1" > "$1.nodbg"
    echo "$1.nodbg"
}

FLOAT_IR="$(strip_dbg "$CONTAINER_OUT/float.ll")"
ARRAY_IR="$(strip_dbg "$CONTAINER_OUT/array.ll")"
for want in \
    '%kelp_List__V2 = type { ptr, i64, i64 }' \
    '%kelp_Box__V2 = type { ptr }' \
    '%kelp_Map__i64__V2 = type { ptr, i64, i64, i64, ptr, i64, i64 }' \
    '%kelp_Map__i64__V2_entry = type { i64, ptr, i8 }' \
    'declare i64 @kelp_map_hash(ptr, i64)' \
    'declare i64 @kelp_map_mix(i64)' \
    'declare i1 @kelp_map_key_eq(ptr, ptr, i64)'
do
    if ! grep -qxF "$want" "$FLOAT_IR"; then
        echo "the container layout in '$FLOAT_IR' is missing: $want" >&2
        exit 1
    fi
done
for want in \
    '%kelp_Array__String__2 = type { [2 x %kelp_String] }' \
    '%kelp_Array__i32__3 = type { [3 x i32] }' \
    '%kelp_Array__Gate__4 = type { [4 x %tag_Gate] }'
do
    if ! grep -qxF "$want" "$ARRAY_IR"; then
        echo "the array layout in '$ARRAY_IR' is missing: $want" >&2
        exit 1
    fi
done
# BUG-vN4tR (MK7 §3.3): an aggregate over 16 bytes must not cross a C boundary
# by value, and the container header is such an aggregate. Every runtime helper
# the module declares is IR-to-C, so no `declare` line may name a container
# header. A header passed by value would be a `sret`/`byval`-shaped call the
# backend does not derive, which the verifier and the fixture suite are both
# blind to.
if grep -h '^declare' "$FLOAT_IR" "$ARRAY_IR" | grep -qE 'kelp_(List|Box|Map|Array)__'; then
    echo "a runtime declaration passes a container header by value" >&2
    grep -h '^declare' "$FLOAT_IR" "$ARRAY_IR" | grep -E 'kelp_(List|Box|Map|Array)__' >&2
    exit 1
fi

echo "$BACKEND gate passed: $(printf '%s\n' $TESTS | wc -l) fixtures, and the container header, entry, array and map-helper layouts read back out of the IR"

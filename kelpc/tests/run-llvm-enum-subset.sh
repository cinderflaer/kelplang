#!/bin/sh
# The MK7 S2C gate: runs the tagged-union (`enum`), `when`, `if let` and `?`
# fixtures against the `llvm` backend.
#
# S2 is "aggregates, containers and drops" (plan/mk7-llvm-backend.md §5), and
# **S2C is its tagged-union slice**: the same `{ tag, union }` layout
# `codegen_c.rs::write_enum` emits today, with the same tag width and the same
# payload offsets (MK7 §3.1), the drop of the live variant only, and the four
# `tir` node kinds the frontend actually produces for a match. `when` and
# `if let` desugar to an `If` chain over `EnumTag`/`EnumField` (`infer.rs::
# check_when`), so no new node was invented; `?` is `Try`.
#
# This script carries the S1, S2A and S2B lists forward and adds the S2C
# fixtures, so one run is the whole opt-in surface at this stage. Its list is
# closed by `--exact`: a fixture that has grown a construct outside the slice is
# refused by name and is deliberately *not* here. Still outside: the containers
# and arrays (`List`/`Map`/`Box`/`[T; n]`) are S2D, closures and function values
# are S2E, `pd` and `extern fun` are S3. A refusal is the correct S2C answer, not
# a failure to hide.
#
# The fixtures that make this slice worth its own gate:
#
#   * `an_enum_payload_is_laid_out_the_way_the_c_typedef_is` (`enum_layout.kp`)
#     pins the payload offset and the union size at the three SysV classes an
#     enum lands in (one eightbyte, two, and MEMORY), each passed and returned
#     by value. The section after the list asserts the same three layouts in the
#     generated IR, because a wrong offset and a wrong union size are both
#     silent.
#   * `an_owned_payload_binds_as_a_borrow` (`enum_owned.kp`) and
#     `when_a_field_of_a_temporary_is_staged_at_its_root` put an owned `String`
#     in a payload and inside an `enum`-typed `datac` field, so the generated
#     per-`enum` drop (one `switch`, the live variant's case only) is exercised
#     on both a success and a leak path.
#   * `a_propagated_failure_frees_the_frame_it_leaves` (`try_frame_drops.kp`) is
#     the `?` whose failure arm frees the frame it returns from; the values pin
#     the success arm, and the valgrind sweep is what proves the failure arm.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-enum-subset.sh
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

# The S1 scalar fixtures, the S2A `datac` fixtures, the S2B runtime fixtures and
# the S2C tagged-union fixtures. `hello.kp` and `fib.kp` are under `examples/`;
# the rest are under `kelpc/tests/fixtures/`.
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
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
backend_env cargo test --release --test compile_and_run -- \
    --exact $TESTS

# The layouts those fixtures exercise, read back out of the IR the backend
# writes beside the binary (`<out>.ll`, MK7 §4.4). The `enum` is a `{ tag, union
# }` struct whose union is anchored on the widest payload's alignment type, and
# these three lines are the tag width and the payload offset at each SysV class
# `enum_layout.kp` pins: a one-byte union at offset 4, an eight-byte union at
# offset 8 with a bare `i64` anchor, and a 24-byte union at offset 8. A wrong
# anchor or a missing pad changes one of these lines, and nothing else in the
# suite would notice.
#
# `cargo test` above leaves the compiler binary in `target/release`; this uses it
# directly rather than through the harness because the assertion is about the
# generated IR, which the harness does not expose.
LAYOUT_OUT="$(mktemp -d)"
trap 'rm -rf "$LAYOUT_OUT"' EXIT
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/enum_layout.kp -o "$LAYOUT_OUT/enum_layout" >/dev/null
# MK7 S4 attaches a `!dbg` location to every instruction and every function
# definition. The assertions below are about the shape of the instruction, not
# about the source line it came from, so each IR file is read through a copy
# with the debug metadata stripped. That a statement resolves to a `.kp` line is
# a separate claim, checked end to end by `cargo test --test llvm_debug_info`.
strip_dbg() {
    sed -E 's/, !dbg ![0-9]+//g; s/ !dbg ![0-9]+//g' "$1" > "$1.nodbg"
    echo "$1.nodbg"
}

IR="$(strip_dbg "$LAYOUT_OUT/enum_layout.ll")"
for want in \
    '%tag_Narrow = type { i32, %tag_Narrow.kelp_v }' \
    '%tag_Narrow.kelp_v = type { i8, [2 x i8] }' \
    '%tag_Eight.kelp_v = type { i64 }' \
    '%tag_Wide.kelp_v = type { i64, [16 x i8] }' \
    '%tag_Wide.kelp_v_1 = type { %tag_Entry }' \
    '%tag_Entry = type { i32, i64 }'
do
    if ! grep -qxF "$want" "$IR"; then
        echo "the enum layout in '$IR' is missing: $want" >&2
        exit 1
    fi
done

echo "$BACKEND gate passed: $(printf '%s\n' $TESTS | wc -l) fixtures, and the enum tag/payload layout read back out of the IR"

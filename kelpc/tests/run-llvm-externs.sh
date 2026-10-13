#!/bin/sh
# The MK7 S3A gate: runs the fixtures whose `extern fun` bodies the `llvm`
# backend now declares and links.
#
# S3 is "the runtime, the externs and the capability check"
# (the MK7 record in plan/done.md), and **S3A is its `extern fun` slice**: the C
# text in `kelpc/src/externs.rs` that backs the raw stdlib primitives (`malloc`,
# `strlen`, `sqrt`, `clock_gettime`, a raw `read`). Every `extern fun` the
# program carries is declared with the exact C signature its body has (a
# `Bytes`/`String` return is 24 bytes and comes back through the hidden first
# argument clang gives it, an owned parameter is a borrow and therefore a
# pointer: BUG-vN4tR, MK7 §3.3), and the bodies are compiled into a per-program
# object from the same `write_extern` text the C backend emits.
#
# One extern is the exception, and the declarations below say so: `std/math`'s
# `sqrt_raw` is declared as LLVM's own `llvm.sqrt.f64` instead of with its C
# signature, because the C body is a `call` at every use and a call inside a
# loop is what stops the SLP vectorizer from packing the arithmetic around it.
# Its C body is still written to the per-program object, where it is now
# unreferenced; the C backend still calls it.
#
# This script carries the S1, S2A, S2B, S2C, S2D, S2F and S2E lists forward and
# adds the S3A fixtures, so one run is the whole opt-in surface at this slice.
# Its list is closed by `--exact`: a fixture that has grown a construct outside
# the slice is refused by name and is deliberately *not* here. The rest of S3
# (`pd`, the capability check at a gated call site, and the two S2B leftovers)
# landed in `kelpc/tests/run-llvm-pd-caps.sh`, which is what carries this list
# forward now: this gate keeps the earlier slices green, not the whole opt-in
# surface.
#
# The fixtures that make this slice worth its own gate:
#
#   * `extern_bodies_lower_with_their_c_signature` (`extern_signatures.kp`) is
#     this slice's own fixture: one extern per C signature shape, so a
#     declaration that drifted from its body prints a wrong value here rather
#     than only failing to link.
#   * `path_normalize_is_the_function_the_capability_checker_uses`
#     (`path_normalize.kp`) is the one body whose reference is not to the C
#     runtime: `normalize_raw` calls `kelp_std__rt__path_normalize`, a Kelp
#     function, so it pins the per-program object's reference into `std/rt`.
#   * `time_and_rand_are_gated_domains_with_no_named_resource`
#     (`caps_time_rand.kp`) is a gated domain call with no resource argument, so
#     the domain gate alone admits it and no allowlist check is owed: this slice
#     lowers it, and a gated call that *does* name a resource is lowered with its
#     check by the S3 gate above.
#   * `a_compiler_shipped_stdlib_module_can_be_imported` (`stdlib_mem.kp`),
#     `stdlib_fmt_formats_numbers_hex_padding_and_bytes` (`stdlib_fmt.kp`),
#     `the_str_stdlib_module_runs_from_kelp_source` (`stdlib_str.kp`),
#     `stdlib_text_builds_numbers_replaces_and_parses` (`stdlib_build.kp`),
#     `stdlib_parsing_rejects_anything_but_a_whole_number`
#     (`stdlib_parse.kp`), `the_encoding_stdlib_module_runs_from_kelp_source`
#     (`stdlib_encoding.kp`) and `std_math_overloads_resolve_on_argument_type`
#     (`stdlib_math.kp`) are the stdlib paths that reach the extern bodies and
#     `String.from_bytes` (the 24-byte-by-value entry point the runtime object
#     carries a pointer-spelled adapter for).
#   * `str_views_borrow_and_owned_truncations_allocate` (`string_views.kp`),
#     `string_and_view_construct_measure_slice_and_convert`
#     (`string_view_basic.kp`), `a_view_carries_its_length_and_slices_without_allocating`
#     (`view.kp`), `a_string_with_zero_capacity_is_a_valid_empty_string`
#     (`string_with_capacity_zero.kp`), `printing_a_temporary_owned_buffer_works`
#     (`heap_temporary_print.kp`), `datac_owned_fields_recurse_and_drop_once`
#     (`datac_owned_fields.kp`), `a_map_of_owned_values_borrows_clones_and_moves`
#     (`map_owned.kp`) and `option_and_result_ship_as_stdlib_modules`
#     (`stdlib_option_result.kp`) are the owned-buffer paths that cross the new
#     boundary and have to free exactly once.
#
# The fixtures that make this slice worth its own gate:
#
#   * `function_values_and_closures_need_no_stdlib` (`fn_value_plain.kp`) is the
#     slice's own fixture: no `impl`, so no imported module's body is lowered and
#     no `std.mem.alloc_raw` call refuses the target. A function value as a
#     local, a parameter, a return value and a `datac` field, and a capturing
#     closure as a `Box` element, a `move` parameter and a field of its own.
#   * `a_fun_literal_is_a_function_value` (`fn_literal.kp`) is a literal with no
#     capture, so its value is a `FnRef`: a bare code pointer, called indirectly,
#     with a `?` inside its body against its own `.return`.
#   * `a_fun_literal_captures_rather_than_failing` (`fn_literal_capture.kp`) is
#     the smallest closure: one `Copy` capture means a heap environment, a
#     `Capture` read in the generated body and a drop that frees it.
#   * `a_two_field_environment_reads_and_drops_each_capture_at_its_own_offset`
#     (`closure_two_captures.kp`) is MK7 §5 item 6 trap 1: an environment whose
#     fields have different alignments (`i32`, `i64`, owned `String`), so a field
#     walk that packed the captures reads the wrong bytes.
#   * `an_indirect_call_numbers_the_environment_after_a_mem_class_return_pointer`
#     (`closure_env_wide_ret.kp`) is trap 8: a callee that returns a 24-byte
#     `MEMORY`-class `datac` *and* carries an environment, so both want a
#     register in the same file.
#   * `a_closure_is_dropped_where_its_binding_dies` (`closure_move.kp`) is the
#     three drop points: the end of a block, an early `return`, and once per loop
#     iteration.
#   * `a_plain_function_value_is_widened_into_a_closure`
#     (`closure_value_widening.kp`) is both widening shapes: the shim (a known
#     function, no allocation) and the thunk (an identity only the call knows,
#     one-field heap environment).
#   * `a_closure_parameter_may_borrow_a_module_declared_type`
#     (`closure_datac_borrow_param.kp`) and
#     `a_library_may_take_a_closure_over_its_own_type` (the
#     `closure_datac_param` project) are a closure signature whose parameter is a
#     borrow of a module-declared `datac`, the BUG-2 shape.
#   * `a_discarded_wide_return_still_builds` (`discard_wide_return.kp`) and
#     `try_frees_the_frame_it_returns_from` (`try_drops_frame.kp`) are two
#     earlier slices' shapes whose last function holds a function value: a
#     discarded wide return through one, and a `?` whose frame owns a closure.
#     They were refused until this slice, so they are in the gate now.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-closures.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# The retired KELPC_BACKEND and KELPC_ORACLE are stripped, so a stale export
# cannot refuse a build.
backend_env() {
    env -u KELPC_BACKEND -u KELPC_ORACLE "$@"
}

# The S1 scalar fixtures, the S2A `datac` fixtures, the S2B runtime fixtures,
# the S2C tagged-union fixtures, the S2D container fixtures, the S2F reference
# fixtures, and the S2E closure and function-value fixtures. `hello.kp` and
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
an_enum_payload_is_laid_out_the_way_the_layout_rule_says
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
a_discarded_wide_return_still_builds
try_frees_the_frame_it_returns_from
a_fun_literal_is_a_function_value
a_fun_literal_captures_rather_than_failing
function_values_and_closures_need_no_stdlib
a_plain_function_value_is_widened_into_a_closure
a_two_field_environment_reads_and_drops_each_capture_at_its_own_offset
an_indirect_call_numbers_the_environment_after_a_mem_class_return_pointer
a_closure_is_dropped_where_its_binding_dies
a_closure_parameter_may_borrow_a_module_declared_type
a_library_may_take_a_closure_over_its_own_type
extern_bodies_lower_with_their_c_signature
path_normalize_is_the_function_the_capability_checker_uses
a_compiler_shipped_stdlib_module_can_be_imported
stdlib_fmt_formats_numbers_hex_padding_and_bytes
std_math_overloads_resolve_on_argument_type
time_and_rand_are_gated_domains_with_no_named_resource
the_str_stdlib_module_runs_from_kelp_source
stdlib_text_builds_numbers_replaces_and_parses
stdlib_parsing_rejects_anything_but_a_whole_number
the_encoding_stdlib_module_runs_from_kelp_source
option_and_result_ship_as_stdlib_modules
str_views_borrow_and_owned_truncations_allocate
string_and_view_construct_measure_slice_and_convert
a_view_carries_its_length_and_slices_without_allocating
a_string_with_zero_capacity_is_a_valid_empty_string
a_map_of_owned_values_borrows_clones_and_moves
printing_a_temporary_owned_buffer_works
datac_owned_fields_recurse_and_drop_once
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
# `noalias` (`codegen_llvm::declare`): the scalar borrows below are what show
# that attribute on a parameter that is not an array.
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

# The S2E shapes, read back out of the IR the same way.
#
# A closure is one identified struct per signature and three `ptr`s, member for
# member the C typedef's `{ fn, env, drop }`; a function value is a bare `ptr`,
# which is why a `fun` parameter and a `fun` return are both `ptr` here. A
# closure parameter is a borrow of a non-`Copy` value, so `own`'s is the fat
# pointer by value where `apply`'s function value is a pointer. An indirect call
# names a value: through a function value it is `call i32 %f(i32 ...)`, and
# through a closure the environment is the hidden first argument,
# `call i32 %fn(ptr %env, i32 ...)`.
#
# The environment is a struct of its own with one field per capture at the
# capture's own natural alignment, so an `i32`, an `i64` and an owned `String`
# are `{ i32, i64, %kelp_String }` and a packed walk would read the wrong bytes.
#
# The widening's two shapes have their own names: `kelp_shim__<fn>__<sig>` for a
# known function (the environment is ignored, nothing is allocated) and
# `kelp_coerce__<sig>` with its `kelp_coerce_drop__<sig>` for one whose identity
# the call knows, plus the per-signature `kelp_closure_drop__<sig>`.
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/fn_value_plain.kp -o "$REF_OUT/fnp" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/closure_two_captures.kp -o "$REF_OUT/ctc" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/closure_value_widening.kp -o "$REF_OUT/cvw" >/dev/null
FNP_IR="$(strip_dbg "$REF_OUT/fnp.ll")"
CTC_IR="$(strip_dbg "$REF_OUT/ctc.ll")"
CVW_IR="$(strip_dbg "$REF_OUT/cvw.ll")"
for want in \
    '%kelp_Closure__i32__i32 = type { ptr, ptr, ptr }' \
    'define i32 @kelp_apply(ptr %0, i32 %1) {' \
    'define ptr @kelp_pick(i1 %0) {' \
    'define i32 @kelp_own(%kelp_Closure__i32__i32 %0, i32 %1) {'
do
    if ! grep -qxF "$want" "$FNP_IR"; then
        echo "the function value in '$FNP_IR' is missing: $want" >&2
        exit 1
    fi
done
# An indirect call through a plain function value (the pointer itself) and
# through a closure (the environment first).
if ! grep -qE 'call i32 %[A-Za-z0-9_.]+\(i32' "$FNP_IR"; then
    echo "'$FNP_IR' has no call through a plain function value" >&2
    exit 1
fi
if ! grep -qE 'call i32 %[A-Za-z0-9_.]+\(ptr %env[0-9]*, i32' "$FNP_IR"; then
    echo "'$FNP_IR' has no call through a closure with the environment first" >&2
    exit 1
fi
# The two-capture environment: each capture at its own alignment.
if ! grep -qE '^%kelp_Env__[^ ]+ = type \{ i32, i64, %kelp_String \}$' "$CTC_IR"; then
    echo "the two-capture environment in '$CTC_IR' is not an aligned { i32, i64, %kelp_String }" >&2
    exit 1
fi
if ! grep -qxF 'define void @kelp_closure_drop__Closure__i64(ptr %0) {' "$CTC_IR"; then
    echo "'$CTC_IR' is missing the per-signature closure drop" >&2
    exit 1
fi
# The widening's two shapes, both named.
for want in \
    '%kelp_ClosureCoerceEnv = type { ptr }' \
    'define i32 @kelp_coerce__Closure__i32__i32(ptr %0, i32 %1) {' \
    'define void @kelp_coerce_drop__Closure__i32__i32(ptr %0) {' \
    'define i32 @kelp_shim__inc__Closure__i32__i32(ptr %0, i32 %1) {' \
    'define void @kelp_closure_drop__Closure__i32__i32(ptr %0) {'
do
    if ! grep -qxF "$want" "$CVW_IR"; then
        echo "the widening in '$CVW_IR' is missing: $want" >&2
        exit 1
    fi
done

# The S3A shapes, read back out of the IR the same way.
#
# Every `extern fun` is a *declaration*, and its declared type is the C ABI's
# shape of its body, not the written one (BUG-vN4tR, MK7 §3.3). The one thing
# the IR verifier cannot check is that the declaration matches the object the
# body is compiled into, so the shapes are pinned here by hand:
#
#   * a 24-byte `Bytes`/`String` return is `void` with the destination as
#     argument 0 (`%kelp_Bytes`/`%kelp_String` never appear as a declared return
#     where clang would pass a hidden pointer);
#   * `size_t`/`uint64_t` are `i64`, `uint8_t` is `i8`, `bool` is `i1`, and a
#     scalar `f64` argument and return are `double`;
#   * a `&str`/`&Bytes`/`mut T` parameter is a `ptr`, and a `&str` *return* is a
#     `ptr` too;
#   * `String#from_bytes`'s C entry point takes its 24-byte argument by value,
#     which cannot cross, so the IR calls the runtime object's pointer-spelled
#     adapter with a staged argument instead.
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/extern_signatures.kp -o "$REF_OUT/externs" >/dev/null
backend_env ./target/release/kelpc build \
    kelpc/tests/fixtures/path_normalize.kp -o "$REF_OUT/pathnorm" >/dev/null
EXTERN_IR="$(strip_dbg "$REF_OUT/externs.ll")"
PATH_IR="$(strip_dbg "$REF_OUT/pathnorm.ll")"
# The C ABI's sub-word extension (BUG-rK2mW): a `uint8_t` or `bool`
# parameter or return carries the `zeroext` clang gives the same prototype,
# which is what makes the C body's widening read the value rather than the
# argument register's upper bits.
for want in \
    'declare void @kelp_std__mem__alloc_raw(ptr, i64)' \
    'declare void @kelp_std__fmt__hex_raw(ptr, i64)' \
    'declare void @kelp_std__str__from_f64_raw(ptr, double, i32)' \
    'declare zeroext i1 @kelp_std__math__is_nan_raw(double)' \
    'declare i64 @kelp_std__str__byte_len_raw(ptr)' \
    'declare zeroext i8 @kelp_std__str__byte_at_raw(ptr, i64)' \
    'declare ptr @kelp_std__str__slice_from_raw(ptr, i64)' \
    'declare zeroext i1 @kelp_std__str__parse_i64_raw(ptr, ptr)' \
    'declare void @kelp_std__path__normalize_raw(ptr, ptr)' \
    'declare void @kelp_String__from_bytes_ptr(ptr, ptr)'
do
    if ! grep -qxF "$want" "$EXTERN_IR"; then
        echo "the extern declaration in '$EXTERN_IR' is missing: $want" >&2
        exit 1
    fi
done
# The one extern that is not declared with its C signature: `sqrt_raw` is
# LLVM's `llvm.sqrt.f64` (see the header). Its C body must not appear on any
# call path, or the vectorizer sees the `call` the intrinsic removed.
if ! grep -qF 'declare double @llvm.sqrt.f64(double)' "$EXTERN_IR"; then
    echo "'$EXTERN_IR' has no llvm.sqrt.f64 declaration" >&2
    exit 1
fi
if grep -q 'kelp_std__math__sqrt_raw' "$EXTERN_IR"; then
    echo "'$EXTERN_IR' still calls std/math's C sqrt body" >&2
    exit 1
fi
# The adapter is *called* with a staged argument, not with a struct value, and
# no declared return is a 24-byte aggregate left by value.
if ! grep -q 'call void @kelp_String__from_bytes_ptr(ptr ' "$EXTERN_IR"; then
    echo "'$EXTERN_IR' has no call to the String#from_bytes adapter" >&2
    exit 1
fi
if grep -qE '^declare %kelp_(Bytes|String) @' "$EXTERN_IR"; then
    echo "'$EXTERN_IR' returns a 24-byte aggregate by value where clang passes a hidden pointer" >&2
    exit 1
fi
# `path.normalize_raw`'s body is the one that calls into `std/rt`, so the
# function it references has to be defined in the same module.
if ! grep -qxF 'define i1 @kelp_std__rt__path_normalize(ptr %0, ptr noalias %1) {' "$PATH_IR"; then
    echo "'$PATH_IR' does not define kelp_std__rt__path_normalize" >&2
    exit 1
fi

# A gated call that *names a resource* is no longer refused: the MK7 S3 rest
# slice lowers the runtime allowlist check, and `kelpc/tests/run-llvm-pd-caps.sh`
# is its gate.
if ! backend_env ./target/release/kelpc build \
    kelpc/tests/projects/caps_env/main.kp -o "$REF_OUT/caps_env" >/dev/null; then
    echo "the $BACKEND backend refuses 'caps_env', whose gated call the S3 rest slice lowers" >&2
    exit 1
fi

echo "$BACKEND gate passed: $(printf '%s\n' $TESTS | wc -l) fixtures, and the scalar, float and borrow-return signatures, the panic tail, the closure fat pointer, environment, indirect call and widening, the extern declarations and their sret/adapter shapes, and the gated call's lowered check read back out of the IR"

use super::*;

// ---------------------------------------------------------------------------
// A-4 phase A: `move T`, a parameter that takes ownership of its argument.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_move_parameter_takes_ownership_of_its_argument() {
    // Also in the AddressSanitizer list: every buffer here is freed exactly
    // once, by whichever function owns it when the scope ends.
    assert_compiles_and_runs("tests/fixtures/move_param.kp", "5\n4\n7\n4\n4\n2\n");
}

#[test]
pub(crate) fn a_moved_parameter_is_dropped_on_every_path_out() {
    // The tail, an early `return`, and a `return` inside a loop. Also in the
    // AddressSanitizer list, so a missed drop is a leak.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_return.kp",
        "4\ntrue\n3\nlong\n5\n",
    );
}

#[test]
pub(crate) fn a_move_parameter_and_the_body_local_beside_it_are_both_freed() {
    // BUG-tV9qH: the frame's owned parameter and the owned local the body
    // declared share one block end in the drop plan, and the parameter's list
    // was written over the block's own, so the body local stayed in no drop
    // set at all. valgrind reported its buffer as definitely lost, on both
    // backends; both buffers are freed now.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_and_body_local.kp",
        "param\nbody\n",
    );
}

#[test]
pub(crate) fn a_body_local_shadowing_a_move_parameter_leaves_no_buffer_behind() {
    // The same one block end, with both bindings of one spelling: the
    // parameter's drop resolved by name to the newest binding and freed the
    // local's buffer while the local's own drop had been overwritten, so
    // exactly one of the two leaked whichever way it went. The parameter's drop
    // names the parameter and the local's names the local, so both are freed.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_shadowed_by_a_local.kp",
        "param\nbody\n",
    );
}

#[test]
pub(crate) fn using_a_moved_argument_again_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/move_param_use_after.kp",
        "use of moved value 'a'",
    );
    // The same rule one iteration later: the second turn of the loop has
    // nothing left to move.
    assert_rejected_with(
        "tests/fixtures/move_param_loop.kp",
        "use of moved value 'a'",
    );
}

#[test]
pub(crate) fn moving_out_of_a_borrow_is_rejected() {
    // The callee's own `&String` parameter cannot be handed to a `move
    // String` parameter: the borrow is not an owner.
    assert_rejected_with(
        "tests/fixtures/move_param_borrow_arg.kp",
        "this call moves its argument in, and a borrow cannot be moved out of",
    );
}

#[test]
pub(crate) fn move_parameter_misuse_is_rejected() {
    // `str` and `&T` are already borrowed, so there is nothing to own.
    assert_rejected_with(
        "tests/fixtures/move_param_str.kp",
        "'move' cannot be written on 'str'",
    );
    assert_rejected_with(
        "tests/fixtures/move_param_ref.kp",
        "'move' cannot be written on '&String'",
    );
    // One parameter cannot both borrow exclusively and take ownership.
    assert_rejected_with(
        "tests/fixtures/move_param_mut_conflict.kp",
        "'mut' and 'move' cannot be combined on one parameter",
    );
    // A receiver is always borrowed.
    assert_rejected_with(
        "tests/fixtures/move_param_receiver.kp",
        "'move' cannot be written on a receiver",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase B: function values, a declared `fun` as a value of type
// `fun(T) => U`, called through a local, a field, a passdown or a parameter.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_function_name_is_a_value_and_a_value_is_callable() {
    // Also in the AddressSanitizer list (the `move` callback frees a buffer).
    assert_compiles_and_runs(
        "tests/fixtures/fn_value.kp",
        "42\n10\n12\n12\n4\n2\n12\n18\n27\n22\n0\n2\n4\n30\n4.000000\n",
    );
}

#[test]
pub(crate) fn a_function_value_with_the_wrong_shape_is_rejected() {
    // The parameter types are part of the function type, so a callback of the
    // wrong shape is an ordinary argument-type mismatch, and a `fun(move
    // String)` callback cannot be handed a borrow.
    assert_rejected_with(
        "tests/fixtures/fn_value_mismatch.kp",
        "expected 'fun(f64) => f64'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_move_arg.kp",
        "this call moves its argument in, and a borrow cannot be moved out of",
    );
}

#[test]
pub(crate) fn a_function_value_that_needs_something_extra_is_rejected() {
    // Each of these names a *function* the compiler cannot turn into a plain
    // C function pointer, so each says which and why rather than "undefined
    // variable".
    assert_rejected_with(
        "tests/fixtures/fn_value_generic.kp",
        "'first' is a generic function",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_passdown.kp",
        "'uses_greeting' reads a passdown, so it cannot be a function value",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_main.kp",
        "cannot reference 'main': it is the program entry point",
    );
}

#[test]
pub(crate) fn function_type_misuse_is_rejected() {
    // A borrow of a function value (`mut` and `&` spellings), a container
    // element, a function type returning a function type, and `print`.
    assert_rejected_with(
        "tests/fixtures/fn_value_borrow.kp",
        "'mut' cannot be written on the function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_amp.kp",
        "'&' cannot be written on the function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_element.kp",
        "cannot be a 'List' element yet",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_nested_return.kp",
        "a function type cannot return another function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_print.kp",
        "cannot pass a value of type 'fun(i32) => i32' to 'print'",
    );
}

#[test]
pub(crate) fn indirect_call_misuse_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/fn_value_named_args.kp",
        "cannot use named arguments",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_arity.kp",
        "a call through 'f' takes 1 argument(s), found 2",
    );
    // A call's *result* is not itself callable: `pick()(4)` needs a binding.
    assert_rejected("tests/fixtures/fn_value_call_result.kp");
}

// ---------------------------------------------------------------------------
// A-4 phase C: anonymous `fun` literals, one synthesized function each,
// checked in a scope of their own.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_fun_literal_is_a_function_value() {
    // Also in the AddressSanitizer list: the `move` literal frees what it is
    // handed. Covers a literal passed directly, a block body, a nested
    // literal, a literal in a `datac` field, a literal returned from a
    // function, and `?` propagating against the literal's own return type.
    assert_compiles_and_runs(
        "tests/fixtures/fn_literal.kp",
        "42\n6\n7\n4\n31\n9\n12\ntrue\n4\n",
    );
}

#[test]
pub(crate) fn a_fun_literal_captures_rather_than_failing() {
    // Phase E replaced the "cannot capture yet" refusal: a name from the
    // enclosing scope is now the thing that makes the literal a closure.
    assert_compiles_and_runs("tests/fixtures/fn_literal_capture.kp", "10\n10\n");
}

#[test]
pub(crate) fn function_values_and_closures_need_no_stdlib() {
    // The closure slice's own fixture (MK7 S2E): the shapes of `fn_value` and
    // `closure_basic` with no `impl` that drags in a std module, because an
    // imported module's bodies are lowered too and `std/str` reaches
    // `std.mem.alloc_raw`, which is the runtime slice. A function value as a
    // local, a parameter, a return value and a `datac` field; a capturing
    // closure as a `Box` element, a `move` parameter and a field of its own.
    assert_compiles_and_runs(
        "tests/fixtures/fn_value_plain.kp",
        "42\n12\n18\n21\n20\n6\n7\n",
    );
}

#[test]
pub(crate) fn a_plain_function_value_is_widened_into_a_closure() {
    // Which of the two widening shapes is used depends on whether the compiler
    // knows which function the value is: a named one gets a shim and allocates
    // nothing, one chosen at the call gets a heap environment and a thunk.
    assert_compiles_and_runs(
        "tests/fixtures/closure_value_widening.kp",
        "6\n11\n9\n101\n",
    );
}

#[test]
pub(crate) fn a_two_field_environment_reads_and_drops_each_capture_at_its_own_offset() {
    // MK7 §5 item 6 trap 1: the only environment with more than one field in
    // the corpus. `a` is an `i32` at offset 0, `b` an `i64` C pads to 8 and `s`
    // an owned `String` at 16, so a packed field walk reads the wrong bytes,
    // and the printed number names all three fields.
    assert_compiles_and_runs("tests/fixtures/closure_two_captures.kp", "3711\n7\n11\n");
}

#[test]
pub(crate) fn an_indirect_call_numbers_the_environment_after_a_mem_class_return_pointer() {
    // MK7 §5 item 6 trap 8: the callee returns a 24-byte `MEMORY`-class `datac`
    // *and* carries an environment, so both take a register in the same
    // integer file, the hidden return pointer first. Numbering the environment
    // `ARG64[0]` would hand it the return pointer instead.
    assert_compiles_and_runs("tests/fixtures/closure_env_wide_ret.kp", "4\n5\n6\n");
}

#[test]
pub(crate) fn a_named_function_ending_in_an_if_without_else_says_what_to_write() {
    assert_rejected_with(
        "tests/fixtures/fun_unit_body_if_no_else.kp",
        "end the block with 'return <value>'",
    );
}

#[test]
pub(crate) fn fun_literal_misuse_is_rejected() {
    // A passdown is a hidden argument a function value cannot carry, a borrow
    // return has no elision root, the tail is checked against the return type like any
    // function's, and a body is required.
    assert_rejected_with(
        "tests/fixtures/fn_literal_passdown.kp",
        "a 'fun' literal cannot read passdown 'base'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_borrow_return.kp",
        "a 'fun' literal cannot return a borrow ('&str')",
    );
    assert_rejected_with(
        "tests/fixtures/fn_type_borrow_return.kp",
        "a function type cannot return a borrow ('&Point')",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_tail_type.kp",
        "this 'fun' literal is declared to return 'i32' but its body evaluates to 'unit'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_no_body.kp",
        "a 'fun' literal needs a body",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase D: the stdlib callbacks, what function values and a move-taking
// parameter were for.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn option_and_result_combinators_take_callbacks() {
    // Also in the AddressSanitizer list: `map` builds owned `String`s.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_combinators.kp",
        "5\nHELLO\n2\ntrue\n7\n7\n42\n105\n4\n11\n4\n",
    );
}

#[test]
pub(crate) fn list_callbacks_work_over_copy_and_owned_elements() {
    // Also in the AddressSanitizer list.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_list_callbacks.kp",
        "6\n2\n123\ntrue\ntrue\n2\n2\n4\n1\ntrue\n2\n",
    );
}

#[test]
pub(crate) fn a_combinator_needing_a_copy_payload_says_so() {
    // `list.filter` reads an accepted element out of a *borrowed* list, so its
    // element type must be `Copy`; the failure is inside the module, so the
    // message names the module and says why. The two helpers that only needed a
    // partial move now take their receiver with `move` instead (A-11): those
    // rejections moved to `option_unwrap_owned.kp` and `result_map_owned.kp`.
    assert_rejected_with(
        "tests/fixtures/stdlib_list_filter_owned.kp",
        "this is inside the shipped standard library ('std/list')",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase E: captures, a `fun` literal that names a binding from the
// enclosing scope becomes a closure (`closure fun(T) => U`), which owns the heap
// environment its captures were moved into.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_closure_carries_its_environment() {
    // Also in the AddressSanitizer list: every owned capture is freed exactly
    // once. Covers a `Copy` capture, an owned capture read through a borrow, a
    // capturing literal passed to a closure parameter and to a `move` one, a
    // closure returned out of the frame that made it, one in a `datac` field,
    // one in a `Box`, and an inner literal calling a closure the frame holds.
    assert_compiles_and_runs(
        "tests/fixtures/closure_basic.kp",
        "11\n10\n3\n30\n-6\n14\n15\n42\n13\n5\n11\ntrue\n",
    );
}

#[test]
pub(crate) fn a_closure_parameter_may_borrow_a_module_declared_type() {
    // BUG-2: the closure typedef and its coercion environment are emitted
    // before the `datac`s, so a `&Item` parameter has to be spelled with the
    // forward-declared struct tag there rather than with `Item`'s typedef
    // name. Naming the typedef name made `cc` reject any callback API over a
    // module's own type.
    assert_compiles_and_runs(
        "tests/fixtures/closure_datac_borrow_param.kp",
        "1\n",
    );
}

#[test]
pub(crate) fn a_library_may_take_a_closure_over_its_own_type() {
    // The other half of BUG-2, and the shape that motivated it: inside a
    // non-entry module the parameter type is module-qualified (`&items.Item`),
    // so the generated closure type is named after it. That name used to come
    // out of the mangling as `refitems.Item`, a dot in a C identifier, and
    // every generated use of the closure type failed to compile. Needs a
    // project because the module prefix is what makes the name qualified.
    assert_project_runs(
        "closure_datac_param",
        &project("closure_datac_param"),
        "1\n4\ntrue\n",
    );
}

#[test]
pub(crate) fn a_closure_is_dropped_where_its_binding_dies() {
    // Also in the AddressSanitizer list: a missed drop is a leak and a second
    // one a double free. Covers the three drop points, the end of a block, an
    // early `return`, and once per loop iteration.
    assert_compiles_and_runs(
        "tests/fixtures/closure_move.kp",
        "5\n7\n0\n10\n11\n12\n",
    );
}

#[test]
pub(crate) fn the_stdlib_callbacks_take_closures() {
    // Also in the AddressSanitizer list: every capturing callback here owns an
    // environment, and one of them captured an owned `String`. Covers a
    // capturing callback through `map`/`any`/`each`, a declared function and a
    // non-capturing literal (widened through a generated shim, so no
    // allocation), a function value whose identity is known only at the call
    // (widened into a one-field environment), and the same widening through
    // `option` and `result`.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_closure_callbacks.kp",
        "3\n9\ntrue\nv=1v=2v=3\n2\ntrue\n4\n107\n5\n12\n",
    );
}

#[test]
pub(crate) fn a_closure_capture_is_checked() {
    // A borrow has no owner to move into the environment, a receiver's field
    // would have to be read out of a borrow, a nested literal cannot move an
    // owned value out of the enclosing environment, and an owned capture can
    // only be borrowed inside the body.
    assert_rejected_with(
        "tests/fixtures/closure_capture_borrow.kp",
        "a 'fun' literal cannot capture 's': it is a borrow ('str')",
    );
    assert_rejected_with(
        "tests/fixtures/closure_capture_receiver.kp",
        "it is a field of 'Counter'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_nested_owned.kp",
        "it belongs to the enclosing closure's environment",
    );
    assert_rejected_with(
        "tests/fixtures/closure_capture_by_value.kp",
        "cannot read a captured value out of the closure's environment by value",
    );
}

#[test]
pub(crate) fn the_closure_type_and_its_limits_are_checked() {
    // `&closure fun(...)` is not a type, a closure is not a container element, the
    // widening needs the same signature, a closure cannot go where a plain
    // function value is expected, and the one widening that exists (a `fun`
    // value where a closure is expected) does not blur the ownership rules:
    // capturing an owned binding still moves it, and a closure handed to a
    // `move` parameter is still gone afterwards.
    assert_rejected_with(
        "tests/fixtures/closure_type_amp.kp",
        "'&' cannot be written on the closure type",
    );
    assert_rejected_with(
        "tests/fixtures/closure_element.kp",
        "cannot be a 'List' element yet",
    );
    assert_rejected_with(
        "tests/fixtures/closure_signature_mismatch.kp",
        "declared as 'closure fun(i32) => i32'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_where_fn_expected.kp",
        "this is a closure: its body captured a binding",
    );
    assert_rejected_with(
        "tests/fixtures/closure_use_after_capture.kp",
        "use of moved value 'text'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_use_after_move.kp",
        "use of moved value 'g'",
    );
}

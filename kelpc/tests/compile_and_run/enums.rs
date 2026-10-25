use super::*;

// ---------------------------------------------------------------------------
// A-1: `enum`, tagged unions, variant construction, and pattern `when`.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn an_enum_matches_on_its_live_variant() {
    assert_fixture_runs("tests/fixtures/enum_basic.kp");
}

#[test]
pub(crate) fn an_owned_payload_binds_as_a_borrow() {
    assert_fixture_runs("tests/fixtures/enum_owned.kp");
}

#[test]
pub(crate) fn a_recursive_enum_nests_through_a_box() {
    assert_fixture_runs("tests/fixtures/enum_recursive.kp");
}

#[test]
pub(crate) fn an_enum_payload_is_laid_out_the_way_the_layout_rule_says() {
    // The three SysV classes a tagged union lands in, each with a payload that
    // only reads back correctly at the offset clang gives it: `Narrow`'s union
    // is at offset 4 because every payload is a `u8` (so the whole enum is 8
    // bytes, `AggClass::One`), `Eight`'s is at 8 (16 bytes, a register pair),
    // and `Wide`'s is at 8 with a 24-byte union (32 bytes, `MEMORY`). A wrong
    // payload offset and a wrong union size are both silent, which is what
    // makes this fixture worth its length.
    assert_fixture_runs("tests/fixtures/enum_layout.kp");
}

#[test]
pub(crate) fn an_enum_is_a_list_element_and_a_box_element() {
    assert_fixture_runs("tests/fixtures/enum_in_containers.kp");
}

#[test]
pub(crate) fn the_enum_node_kinds_keep_two_live_locals_in_different_registers() {
    // The register allocator's liveness pass has twice handed two live locals
    // the same register because a node kind it never walked made one of them
    // look dead on declaration. This fixture keeps two scalars alive across
    // each of this slice's four node kinds, and, added with the slice's own
    // defect fix, across `if let` and across a call with an enum-typed local
    // live on the far side, the two shapes the slice's report claimed were
    // probed when the fixture held neither. A repeat of the mistake is a wrong
    // sum rather than a silent one.
    assert_fixture_runs("tests/fixtures/enum_liveness.kp");
}

#[test]
pub(crate) fn nested_aggregate_literals_are_staged_in_slots_of_their_own() {
    // A pre-existing bug, not one this slice introduces: every staged aggregate
    // value used one shared frame slot, so a field initializer that staged
    // another one overwrote what the outer value had already written. Three of
    // these five prints were wrong before the fix (`7` for `6`, `10` for `9`,
    // `20` for `19`).
    assert_fixture_runs("tests/fixtures/nested_literal_staging.kp");
}

#[test]
pub(crate) fn an_enum_is_a_container_element_and_a_container_drops_it() {
    assert_fixture_runs("tests/fixtures/enum_container.kp");
}

#[test]
pub(crate) fn when_arms_do_not_need_a_newline_inside_parentheses() {
    // A `when` is an expression, and inside `(` `)` the lexer emits no
    // newline tokens at all, so an arm separator cannot be required there.
    assert_fixture_runs("tests/fixtures/when_in_args.kp");
}

#[test]
pub(crate) fn when_a_temporary_scrutinee_is_staged_into_a_local() {
    // A call's result is not a place, so it is bound to a local of its own and
    // the arms match that: a payload binds as a borrow, and the borrow needs
    // the temporary to outlive the arm.
    assert_fixture_runs("tests/fixtures/enum_temp_scrutinee.kp");
}

#[test]
pub(crate) fn when_a_field_of_a_temporary_is_staged_at_its_root() {
    // `mk().t` stages `mk()` and matches `#when0.t` rather than staging the
    // field itself, so the whole `Holder` is freed once, `extra` included.
    // The same fixture covers the `if let` desugar and its `elif let` chain.
    assert_fixture_runs("tests/fixtures/scrutinee_field_of_temporary.kp");
}

#[test]
pub(crate) fn an_enum_method_is_called_on_a_binding() {
    // A method declared on an `enum` is called on a value of the enum type: a
    // binding reaches the method tables and resolves `fun Shape.label()`.
    assert_fixture_runs("tests/fixtures/enum_method_binding.kp");
}

#[test]
pub(crate) fn an_enum_method_is_refused_on_a_variant_path() {
    // The receiver builds the variant, so it names a type rather than a value;
    // the method exists and the fix is still to bind the variant first, which
    // is what the message has to say.
    assert_rejected_with(
        "tests/fixtures/enum_method.kp",
        "an 'enum' method is called on a value, not on a variant path",
    );
}

#[test]
pub(crate) fn enum_misuse_is_rejected() {
    for name in [
        // Exhaustiveness, and arm/pattern shape.
        "enum_non_exhaustive",
        "enum_unknown_variant",
        "enum_duplicate_arm",
        "enum_bind_count",
        "enum_datac_scrutinee",
        // Construction and access.
        "enum_ctor_name",
        "enum_bare_variant",
        "enum_field_access",
        "enum_unknown_method",
        // Declaration rules, shared with `datac`.
        "enum_datac_clash",
        "enum_self_by_value",
        // A payload reached through a borrow binds as a shared borrow.
        "enum_payload_move",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

// #281: a variant named without its enum builds that variant when no other
// enum of the module declares it. The qualified form still works.
#[test]
pub(crate) fn a_unique_variant_builds_by_its_bare_name() {
    assert_fixture_runs("tests/fixtures/enum_bare_construct.kp");
}

// #281: a variant name shared by two enums of the module must be qualified.
#[test]
pub(crate) fn a_variant_name_shared_by_two_enums_must_be_qualified() {
    assert_rejected_with(
        "tests/fixtures/enum_bare_clash.kp",
        "so it must be written with its enum: 'Drawing.Circle' or 'Shape.Circle'",
    );
}

// #281: a function of the same name keeps the bare name.
#[test]
pub(crate) fn a_function_of_the_same_name_keeps_the_bare_name() {
    assert_fixture_runs("tests/fixtures/enum_bare_function.kp");
}

// #281: a prelude name is never rewritten to a module enum's variant.
#[test]
pub(crate) fn a_prelude_name_is_never_a_bare_variant() {
    assert_rejected_with("tests/fixtures/enum_bare_prelude.kp", "call to undefined function 'Some'");
}

// ---------------------------------------------------------------------------
// MK5-S6-M1: `if let`, a pattern condition, desugared to a single-arm `when`.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn if_let_binds_one_variant_and_else_covers_the_rest() {
    assert_fixture_runs("tests/fixtures/if_let_basic.kp");
}

#[test]
pub(crate) fn if_let_else_chain_reads_its_scrutinee_more_than_once() {
    assert_fixture_runs("tests/fixtures/if_let_chain_same_scrutinee.kp");
}

#[test]
pub(crate) fn let_else_binds_through_option_and_enum() {
    // design/syntax.md (kelp wiki) 4.1: a matched let-else binds for the rest of the block,
    // and a missed one runs its else block (return, or continue in a loop).
    assert_fixture_runs("tests/fixtures/let_else.kp");
}

#[test]
pub(crate) fn let_else_else_block_drops_the_enclosing_scope() {
    // The else block's `return` drops the local declared before the `let`,
    // the same drop a plain `return` makes, before the caller prints.
    assert_fixture_runs("tests/fixtures/let_else_drops.kp");
}

#[test]
pub(crate) fn let_else_refusals_name_the_rule() {
    assert_rejected_with(
        "tests/fixtures/let_else_falls_through.kp",
        "let-else: the else block falls through",
    );
    assert_rejected_with(
        "tests/fixtures/let_else_plain_name.kp",
        "is a plain name, which always matches",
    );
    assert_rejected_with(
        "tests/fixtures/let_else_type_mismatch.kp",
        "let-else: the pattern 'Circle' must match the expression's type",
    );
}

#[test]
pub(crate) fn if_let_misuse_is_rejected() {
    for name in ["if_let_unknown_variant", "if_let_bind_count"] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
pub(crate) fn an_else_if_let_behind_a_plain_if_compiles() {
    // BUG-6: the checker's else-if arm assumed `Expr::If`, which only a plain
    // `else if` is (an `else if let` desugars to a block), so this shape was an
    // internal error rather than a program. The chain starts with a plain
    // `if`, a payload-free variant and a link whose pattern misses are all
    // here.
    assert_fixture_runs("tests/fixtures/else_if_let.kp");
}

// ---------------------------------------------------------------------------
// MK5-S6-M1: `?`, propagate the failure of an `Ok`/`Err` or `Some`/`None` enum.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn try_propagates_a_failure_out_of_the_enclosing_function() {
    assert_fixture_runs("tests/fixtures/try_operator.kp");
}

#[test]
pub(crate) fn option_and_result_ship_as_stdlib_modules() {
    assert_fixture_runs("tests/fixtures/stdlib_option_result.kp");
}

#[test]
pub(crate) fn a_panicking_tail_satisfies_any_return_type() {
    // The body diverges, so no `return` is needed after the `panic`, and the
    // panic still fires when that path is taken.
    assert_fixture_panics("panic_tail", "always_fails: non-positive");
}

#[test]
pub(crate) fn try_frees_the_frame_it_returns_from() {
    // `?`'s early return is a `return`: a frame that owns something and
    // propagates a failure has to free what it owns first. The output pins the
    // success paths (freed exactly once) and the failure paths (freed at the
    // `?`); the AddressSanitizer gate below is what proves the second half,
    // because a leak prints nothing.
    assert_fixture_runs("tests/fixtures/try_drops_frame.kp");
}

#[test]
pub(crate) fn a_propagated_failure_frees_the_frame_it_leaves() {
    // `?`'s early return is a `return`, so the frame's owned locals die on it
    // too. `try_drops_frame.kp` pins the same shape, but its last function
    // wraps it in a `fun` literal, which this backend refuses by name, so this
    // fixture is the same mechanism without the closure. The printed values pin
    // the success arm; the valgrind sweep is what proves the failure arm frees,
    // because a leak prints nothing.
    assert_fixture_runs("tests/fixtures/try_frame_drops.kp");
}

#[test]
pub(crate) fn a_one_eightbyte_try_payload_builds_without_a_destination() {
    // A `?` evaluated in a position that can hold one eightbyte (`expr`, which
    // is what a `let` of a one-eightbyte `datac` uses) refused *every*
    // aggregate payload for want of a destination, and said "over eight bytes"
    // about a three-byte `datac`. `Result<D3, u8>` is eight bytes in total, so
    // its success payload fits `rax` and the program must build, while a
    // genuinely wider payload keeps the refusal.
    assert_fixture_runs("tests/fixtures/try_narrow_payload.kp");
}

#[test]
pub(crate) fn try_misuse_is_rejected() {
    for name in [
        // `main` returns `Unit`, so there is nowhere to propagate to.
        "try_in_main",
        // The operand has to be an enum, of the right shape, held by value.
        "try_non_enum",
        "try_wrong_shape",
        "try_borrowed",
        // The failure payload types have to agree, and so do the shapes.
        "try_error_type",
        "try_shape_mismatch",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

// ---------------------------------------------------------------------------
// A-2: user-declarable generic types, `datac`/`enum` templates, monomorphized
// at each use site, with the type arguments inferred from the construction.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_generic_datac_and_enum_are_monomorphized_per_use_site() {
    assert_fixture_runs("tests/fixtures/generic_type_basic.kp");
}

#[test]
pub(crate) fn a_generic_instantiation_owns_and_drops_like_any_other_type() {
    assert_fixture_runs("tests/fixtures/generic_owned.kp");
}

#[test]
pub(crate) fn a_generic_type_can_be_recursive_through_a_box() {
    assert_fixture_runs("tests/fixtures/generic_recursive.kp");
}

#[test]
pub(crate) fn a_generic_function_is_instantiated_per_call_site() {
    assert_fixture_runs("tests/fixtures/generic_function_basic.kp");
}

#[test]
pub(crate) fn a_generic_function_works_over_an_owned_type() {
    assert_fixture_runs("tests/fixtures/generic_function_owned.kp");
}

#[test]
pub(crate) fn a_method_on_a_generic_type_is_instantiated_per_receiver() {
    assert_fixture_runs("tests/fixtures/generic_method_basic.kp");
}

#[test]
pub(crate) fn explicit_type_arguments_are_written_at_the_call() {
    assert_fixture_runs("tests/fixtures/generic_explicit_args.kp");
}

#[test]
pub(crate) fn nested_generic_constructions_infer_from_their_arguments() {
    assert_fixture_runs("tests/fixtures/generic_nested_inference.kp");
}

#[test]
pub(crate) fn explicit_type_argument_misuse_is_rejected() {
    for name in [
        // The wrong number of arguments.
        "generic_explicit_arity",
        // A type argument list on something that is not generic.
        "generic_explicit_plain",
        // A method's parameters come from its receiver, so it takes none.
        "generic_explicit_method",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
pub(crate) fn generic_method_misuse_is_rejected() {
    for name in [
        // The receiver omits its type arguments entirely.
        "generic_method",
        // ... or writes parameters the type does not have, in the wrong order.
        "generic_method_wrong_params",
        // A non-generic type has nothing to parameterize.
        "generic_method_on_plain",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
pub(crate) fn generic_function_misuse_is_rejected() {
    for name in [
        "generic_function_cannot_infer",
        "generic_function_arity",
        "generic_main",
        "generic_function_clash",
        "generic_function_shadow",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
pub(crate) fn generic_type_misuse_is_rejected() {
    for name in [
        "generic_needs_args",
        "generic_arity",
        "generic_cannot_infer",
        "generic_param_shadow",
        "generic_param_duplicate",
        "generic_clash",
        "generic_borrowed_field",
        "generic_self_value",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

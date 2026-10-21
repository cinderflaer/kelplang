use super::*;

// ---------------------------------------------------------------------------
// M8: the builtin owning pointer `Box<T>`, recursive `datac`s, and the
// container emission order that makes them work.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_box_holds_a_value_or_nothing() {
    assert_compiles_and_runs(
        "tests/fixtures/box_basic.kp",
        "true\n7\nfalse\n9\ntrue\nhi\n5\n2\n1\n3\n",
    );
}

// Manual memory blocks, slice 1 (decision 139). Step 2 parses `block name: T { ... }`
// and refuses it until the block runtime lands.

#[test]
pub(crate) fn block_stays_an_ordinary_name_outside_a_block_introducer() {
    assert_compiles_and_runs("tests/fixtures/block_names_stay_ordinary.kp", "5\n8\n3\n");
}

#[test]
pub(crate) fn a_block_result_must_be_used_with_try_or_when() {
    assert_rejected_with(
        "tests/fixtures/block_region_not_built.kp",
        "block 'request' can fail to start, and its result is not used",
    );
}

#[test]
pub(crate) fn a_block_try_has_nowhere_to_go_in_a_unit_function() {
    assert_rejected_with(
        "tests/fixtures/block_try_in_unit_function.kp",
        "'?' has nowhere to send a failure, because 'main' returns Unit",
    );
}

#[test]
pub(crate) fn a_block_name_is_unique_in_its_function() {
    assert_rejected_with("tests/fixtures/block_name_twice.kp", "the block 'a' is already declared in this function");
}

#[test]
pub(crate) fn a_let_may_not_shadow_a_block_name() {
    assert_rejected_with("tests/fixtures/block_name_shadowed_by_let.kp", "shares its name with a 'let' in this function");
}

#[test]
pub(crate) fn a_block_box_is_valid_only_inside_its_block() {
    assert_rejected_with("tests/fixtures/block_box_outside_its_block.kp", "'a.box()' is valid only inside the block 'a'");
}

#[test]
pub(crate) fn a_block_box_is_bound_by_a_let_inside_its_block() {
    assert_rejected_with(
        "tests/fixtures/block_box_not_bound_by_let.kp",
        "a block box from 'request.box()?' must be bound by a 'let' in its block",
    );
}

#[test]
pub(crate) fn a_block_box_may_not_leave_its_block_through_a_plain_binding() {
    assert_rejected_with(
        "tests/fixtures/block_box_escapes.kp",
        "block value 'cell' cannot be moved out of block 'request' into 'copy'",
    );
}

#[test]
pub(crate) fn a_block_box_may_not_be_passed_to_a_call() {
    assert_rejected_with(
        "tests/fixtures/block_box_moved_into_call.kp",
        "block value 'cell' cannot be moved out of block 'request' into the parameter 'n'",
    );
}

#[test]
pub(crate) fn a_block_box_may_not_be_the_value_of_its_block() {
    assert_rejected_with(
        "tests/fixtures/block_box_returned_as_value.kp",
        "block value 'cell' cannot leave block 'request' as the value of its block",
    );
}

#[test]
pub(crate) fn a_block_box_may_not_be_stored_in_a_place_outside_its_block() {
    assert_rejected_with(
        "tests/fixtures/block_box_stored_outside.kp",
        "block value 'cell' cannot be stored in 'keep', which is declared outside block 'request'",
    );
}

#[test]
pub(crate) fn a_block_box_may_not_be_stored_in_a_shared_list() {
    assert_rejected_with(
        "tests/fixtures/block_box_stored_in_list.kp",
        "block value 'cell' cannot be stored in 'list', because 'list' is shared-pool memory that can outlive block 'request'",
    );
}

#[test]
pub(crate) fn a_closure_may_not_capture_a_block_box() {
    assert_rejected_with(
        "tests/fixtures/block_box_captured_by_closure.kp",
        "this closure captures block value 'cell', so its environment would be shared-pool memory",
    );
}

#[test]
pub(crate) fn a_scoped_block_may_not_capture_a_block_box() {
    assert_rejected_with(
        "tests/fixtures/block_box_captured_by_scoped.kp",
        "scoped block captures block value 'cell', which would leave block 'request'",
    );
}

#[test]
pub(crate) fn a_spawned_thread_may_not_capture_a_block_box() {
    assert_rejected_with(
        "tests/fixtures/block_box_captured_by_spawn.kp",
        "block value 'cell' cannot be passed to a spawned thread",
    );
}

#[test]
pub(crate) fn a_list_made_inside_a_block_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/block_list_made_in_block.kp",
        "a List made in block 'request' is shared-pool memory, so the block cannot free it",
    );
}

#[test]
pub(crate) fn a_string_made_inside_a_block_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/block_string_made_in_block.kp",
        "a String made in block 'request' is shared-pool memory, so the block cannot free it",
    );
}

#[test]
pub(crate) fn a_block_element_holds_scalar_fields_only() {
    assert_rejected_with("tests/fixtures/block_element_not_scalar.kp", "cannot hold 'String'");
}

#[test]
pub(crate) fn a_block_region_runs_and_releases_on_every_exit() {
    // The end of a block, a loop, a `return`, a `?` that leaves the function,
    // and a `defer` in the caller (decision 139, step 4).
    assert_compiles_and_runs("tests/fixtures/block_region_runs.kp", "5\n3\n7\n0\n1\n2\n5\n");
}

#[test]
pub(crate) fn a_block_and_shared_pool_code_share_one_file() {
    // Decision 139, step 6: shared-pool code and one block side by side, with no
    // mode line. The loop creates and frees a thousand blocks, and the list that
    // lives in shared-pool memory is intact afterwards.
    assert_compiles_and_runs("tests/fixtures/block_end_to_end.kp", "4\n10\n1\n999000\n1\n");
}

#[test]
pub(crate) fn a_block_box_is_read_through_get() {
    // `cell.get().value` reads a field of the element, which stays in its block
    // (design section 5). The zeroed element is 0, so the result is the argument.
    assert_compiles_and_runs("tests/fixtures/block_box_get_reads_field.kp", "7\n");
}

#[test]
pub(crate) fn a_block_box_field_without_get_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/block_box_field_without_get.kp",
        "block box 'cell' has no field 'value': its element is read through 'get()'",
    );
}

#[test]
pub(crate) fn a_block_holds_one_box() {
    assert_rejected_with("tests/fixtures/block_box_twice.kp", "already has a box");
}

#[test]
pub(crate) fn a_datac_can_be_recursive_through_a_box() {
    // The linked list: `next` is an owned pointer, `Box.empty()` terminates
    // it, and the whole chain is freed recursively (also under ASAN).
    assert_compiles_and_runs(
        "tests/fixtures/box_recursive.kp",
        "6\n3\n3\n2\n1\n203\n19906\n198\ntrue\n198\n",
    );
}

#[test]
pub(crate) fn a_datac_can_be_recursive_through_a_list_and_mutually_recursive_through_boxes() {
    assert_compiles_and_runs("tests/fixtures/recursive_datac.kp", "113\n2\n100\nl1\nl1\nb1\n");
}

#[test]
pub(crate) fn using_a_field_after_it_was_moved_out_is_refused() {
    // The partial move takes the field, so the second statement reaches
    // through a field that is gone. The refusal is pinned so the fixture
    // cannot pass for the old "no partial moves" reason.
    assert_rejected_with(
        "tests/fixtures/owned_field_read.kp",
        "use of moved value 'bag.xs'",
    );
}

#[test]
pub(crate) fn a_partial_move_takes_an_owned_field_and_leaves_the_rest() {
    // The moved field's slot is zeroed at the move and the binding's own drop
    // frees the field that stayed, so both this fixture and
    // `partial_move_conditional` are in the AddressSanitizer and valgrind
    // gates: a double free or a leak fails there.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_field.kp",
        "kept\n1\n",
    );
}

#[test]
pub(crate) fn a_partial_move_on_one_branch_still_frees_once_on_both() {
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_conditional.kp",
        "a\n1\nb\n2\n",
    );
}

#[test]
pub(crate) fn a_field_is_owned_again_after_a_fresh_value_is_assigned_to_it() {
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_reassign.kp",
        "1\n2\n",
    );
}

#[test]
pub(crate) fn a_move_parameter_is_an_owned_local_for_a_partial_move() {
    // A `move` parameter owns what it was handed, so reading an owned field
    // out of it by value takes the field out the same way a `val` binding
    // does. The callee frees the fields that stayed, so a double free of the
    // moved buffer fails here and under valgrind.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_move_param.kp",
        "taken\n1\n",
    );
}

#[test]
pub(crate) fn a_partial_move_out_of_a_borrow_stays_refused() {
    assert_rejected_with(
        "tests/fixtures/partial_move_from_borrow.kp",
        "cannot move the field 'xs' out of 'b': the value is borrowed",
    );
}

#[test]
pub(crate) fn a_field_cannot_be_moved_out_twice() {
    assert_rejected_with(
        "tests/fixtures/partial_move_double.kp",
        "use of moved value 'bag.xs'",
    );
}

#[test]
pub(crate) fn a_partially_moved_binding_cannot_be_used_as_a_whole() {
    assert_rejected_with(
        "tests/fixtures/partial_move_use_base.kp",
        "cannot use 'bag' as a whole value: its field 'xs' was moved out",
    );
}

#[test]
pub(crate) fn a_partially_moved_binding_cannot_be_borrowed_as_a_whole() {
    // `&bag` is a whole-value read too, so the borrow is refused where the
    // binding is no longer whole. Two fields are gone and the refusal names
    // one, which pins the moved set as ordered: read out of a hash set, the
    // field named here would change from run to run.
    assert_rejected_with(
        "tests/fixtures/partial_move_borrow_base.kp",
        "cannot use 'bag' as a whole value: its field 'name' was moved out",
    );
}

#[test]
pub(crate) fn a_partial_move_takes_an_owned_enum_payload_and_leaves_the_rest() {
    // The enum-payload half of A-11. The moved payload's slot is zeroed at the
    // move and the enum's own drop frees the payloads the move left behind, so
    // this fixture is in the AddressSanitizer and valgrind gates: a double free
    // or a leak fails there. A payload that stayed is still readable, and the
    // first case moves it out of a `move` parameter.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_enum_payload_owned.kp",
        "moved\nkept\n2\n",
    );
}

#[test]
pub(crate) fn an_enum_payload_cannot_be_moved_out_twice() {
    // The first `when` took the value and cleared its slot, so the second read
    // of the same payload is a use of the moved value, named through the
    // projection.
    assert_rejected_with(
        "tests/fixtures/partial_move_enum_payload_double.kp",
        "use of moved value 'h.text'",
    );
}

#[test]
pub(crate) fn an_enum_payload_cannot_be_moved_out_of_a_borrow() {
    // A payload binds as a shared borrow when the scrutinee is one, so an
    // `if let` over a borrowed enum hands the arm a `&String` and cannot
    // produce the owned one. `enum_payload_move.kp` is the same refusal
    // through a `when`.
    assert_rejected_with(
        "tests/fixtures/partial_move_enum_payload_from_borrow.kp",
        "return type mismatch: expected 'String', found '&String'",
    );
}

#[test]
pub(crate) fn the_owned_option_payload_comes_out_with_unwrap() {
    // A-11 unblocks the accessors that hand a payload out by value: `unwrap`,
    // `expect` and `unwrap_or` take the option with `move`, so an owned payload
    // is read out of the enum by value. The payload's slot is cleared at that
    // read, so this is in the AddressSanitizer and valgrind gates.
    assert_compiles_and_runs(
        "tests/fixtures/option_unwrap_owned.kp",
        "hello\nnone\nexpected\nheld\nstaged\n",
    );
}

#[test]
pub(crate) fn a_result_combinator_hands_an_owned_payload_back() {
    // `result.map`/`map_err` take the result with `move`, so the side they
    // rebuild may be owned: `map` moves a `datac` error into the fresh `Err`
    // and `map_err` moves an owned `String` into the fresh `Ok`.
    assert_compiles_and_runs(
        "tests/fixtures/result_map_owned.kp",
        "5\ntrue\ntrue\ntrue\n",
    );
}

#[test]
pub(crate) fn an_owned_field_in_a_box_can_be_taken_and_set() {
    assert_compiles_and_runs("tests/fixtures/box_field_take.kp", "1\ntrue\n1\n2\n");
}

#[test]
pub(crate) fn box_owned_elements_are_freed_once_when_replaced_or_taken() {
    // A `set` on a full box frees the `String`/`Bytes` the slot held, and a
    // `take` moves the value out to a second owner without a second free.
    assert_compiles_and_runs(
        "tests/fixtures/box_owned_elements.kp",
        "false\nhello\nworld\ntrue\nworld\ntrue\nfalse\nalpha\nbeta\nbeta\ntrue\n3\n5\ndefgh\ntrue\ndefgh\n104\n4\n5\n9\ntrue\n5\n3\n6\n2\n103\n4956\n98\n195\ntrue\nfalse\n98\n103\n4956\n",
    );
}

#[test]
pub(crate) fn a_box_cannot_hold_a_borrow() {
    assert_rejected("tests/fixtures/box_borrow_element.kp");
}

#[test]
pub(crate) fn a_datac_cannot_hold_itself_by_value() {
    assert_rejected("tests/fixtures/datac_self_value.kp");
}

#[test]
pub(crate) fn a_map_cannot_be_a_list_element() {
    // A-10 slice 1 made a `List<T>` and a `Box<T>` list elements, but a map
    // value set is narrower, so a `Map` element stays refused.
    assert_rejected_with(
        "tests/fixtures/list_container_element.kp",
        "'Map<String, i32>' cannot be a 'List' element",
    );
}

#[test]
pub(crate) fn reaching_into_an_empty_box_panics() {
    assert_fixture_panics("box_empty_panic", "Box.get: the box is empty");
}

#[test]
pub(crate) fn a_datac_can_be_a_list_element() {
    assert_compiles_and_runs("tests/fixtures/list_datac_element.kp", "2\nspades\n2\nspades\n");
}

#[test]
pub(crate) fn a_map_stores_keys_and_returns_insertion_order() {
    assert_compiles_and_runs(
        "tests/fixtures/map_basic.kp",
        "0\ntrue\n3\ntrue\nfalse\n2\n99\n22\n1\n2\ntwo three \n2\n0\ntrue\n",
    );
}

#[test]
pub(crate) fn a_map_of_owned_values_borrows_clones_and_moves() {
    // The value is inline in the entry, the key is an owned `String`, and
    // `get_or`/`remove`/`values` all transfer or clone it; ASAN re-runs this
    // fixture (see `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_owned.kp",
        "2\ntwo\ntwo\nfallback\nTWO\none\n1\nbeta\nTWO\ntrue\n",
    );
}

#[test]
pub(crate) fn a_map_holds_datac_values_and_can_live_in_a_datac_field() {
    assert_compiles_and_runs("tests/fixtures/map_datac.kp", "30\n2\n3\n1\n");
}

#[test]
pub(crate) fn a_map_deep_copies_a_datac_value_out_through_get_or_and_values() {
    // MISSING-qM4Zx: `get_or` and `values` are gated on the same
    // declaration-graph deep-copy predicate `List.slice` uses, so a `datac`
    // whose fields all copy takes the same derived `kelp_clone_<T>` out of the
    // map that it takes out of a list. The value owns a `String`, so a shallow
    // copy would print the right text and then double-free the buffer; ASAN
    // re-runs this fixture (see `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_value_clone.kp",
        "alpha\n10\nnone\n0\n2\nalpha\n10\nbeta\n20\nalpha\n2\n",
    );
}

#[test]
pub(crate) fn a_map_cannot_hold_a_container_or_be_created_without_a_value_type() {
    assert_rejected("tests/fixtures/map_container_value.kp");
    assert_rejected("tests/fixtures/map_no_value_type.kp");
}

#[test]
pub(crate) fn a_map_key_can_be_a_scalar_and_the_shorthand_still_means_string() {
    assert_compiles_and_runs(
        "tests/fixtures/map_key_types.kp",
        "2\n99\ntrue\nfalse\n0\n2\n1\n1\n10\n2\nyes\nno\n70\n2\n9\nfalse\n2\n6\nbb\n",
    );
}

#[test]
pub(crate) fn a_key_type_the_compiler_cannot_hash_or_compare_is_rejected() {
    for name in [
        // `nan != nan` would make an inserted key unfindable.
        "map_float_key",
        // A `datac` with one field that is not keyable is not a key either:
        // the keyability fixpoint over the declaration graph leaves it out.
        "map_unkeyable_datac_key",
        // A borrow and a container have no derived hash and equality.
        "map_borrow_key",
        "map_list_key",
        // One argument (`Map<V>`) or two (`Map<K, V>`), never three.
        "map_arity",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
pub(crate) fn a_datac_whose_fields_are_keyable_is_a_map_key() {
    // A-6: the compiler derives the key's hash and equality for a `datac` the
    // same way it derives its order and its deep copy, so no comparator and no
    // trait is involved.
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_key.kp",
        "1\n10\nfalse\nfalse\n",
    );
}

#[test]
pub(crate) fn a_datac_map_key_is_cloned_in_overwritten_and_removed() {
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_key_ok.kp",
        "2\n99\n20\nfalse\ntrue\n2\n1\n2\n99\n1\nfalse\n31\n7\n29\nfalse\n",
    );
}

#[test]
pub(crate) fn a_datac_map_key_may_own_a_string_field() {
    // The `String` field is hashed byte-wise and compared length-aware, and the
    // entry owns a deep copy of the key, so ASAN re-runs this fixture (see
    // `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_nested_datac_key.kp",
        "2\n10\nfalse\n20\n10\n1\ntrue\n",
    );
}

#[test]
pub(crate) fn a_datac_that_declares_hash_and_eq_is_a_map_key() {
    // A-12: the declared pair is the escape hatch from the derived key set. A
    // float is never keyable (`nan != nan`), so `Sample` is a type the
    // keyability fixpoint refuses and the map reaches the two declared methods
    // instead of a derived hash and equality.
    assert_compiles_and_runs(
        "tests/fixtures/map_declared_key.kp",
        "2\n10\n20\nfalse\n2\n99\n2\nalpha\n1.500000\n20\n1\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn an_enum_that_declares_hash_and_eq_is_a_map_key() {
    // A-12: the same rule for an `enum`. The declared `hash` folds only the
    // tag, so the two `Ratio` keys collide and `eq` is what the probe has to
    // call to tell them apart.
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("map_declared_key_enum", "describes the enum 'Kind' only as a heap object");
}

#[test]
pub(crate) fn a_datac_with_derived_eq_and_hash_is_a_map_key() {
    // lang-derive-eq: `derive(eq, hash)` writes the pair field by field, so the
    // `String`, `bool`, `char` and `i32` fields are the key, and `==` uses the
    // same derived `eq`. The last line is the removed entry's value and then
    // the map's length, and the probe with `on = false` finds nothing.
    assert_compiles_and_runs(
        "tests/fixtures/derive_map_key_datac.kp",
        "2\n10\nfalse\n2\n99\nfalse\n99\n1\ntrue\n",
    );
}

#[test]
pub(crate) fn an_enum_with_derived_eq_and_hash_is_a_map_key() {
    // lang-derive-eq: the same rule for an `enum`. Two variants with the same
    // payload fields are different keys, and a unit variant is one key.
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("derive_map_key_enum", "describes the enum 'Shape' only as a heap object");
}

#[test]
pub(crate) fn derive_refuses_what_it_cannot_check() {
    // lang-derive-eq: each refusal names the field or the method, not a vague
    // "not a key" at a `Map` use site.
    assert_rejected_with(
        "tests/fixtures/derive_refuse_field_without_eq.kp",
        "field 'inner' of type 'Inner'",
    );
    assert_rejected_with(
        "tests/fixtures/derive_refuse_float_field.kp",
        "cannot hash field 'ratio' of type 'f64'",
    );
    assert_rejected_with(
        "tests/fixtures/derive_refuse_hand_written.kp",
        "the hand-written 'fun Point.eq' already declares it",
    );
    assert_rejected_with(
        "tests/fixtures/derive_refuse_generic_field.kp",
        "cannot check field 'item' of type 'T'",
    );
    assert_rejected_with(
        "tests/fixtures/derive_refuse_bad_name.kp",
        "'derive' can derive 'eq' and 'hash', not 'show'",
    );
}

#[test]
pub(crate) fn a_datac_that_declares_eq_gets_the_equality_operator() {
    // H-5: `a == b` lowers to `a.eq(&b)` and `a != b` to `!a.eq(&b)`, with
    // `eq` alone enough for the operator. The fixture also pins that each
    // operand is one evaluation, left to right, and that a borrow against an
    // owned value works.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_datac.kp",
        "true\ntrue\nfalse\ntrue\ntrue\n3\n3\nfalse\n4\n4\nfalse\nguard\n3\n",
    );
}

#[test]
pub(crate) fn an_enum_that_declares_eq_gets_the_equality_operator() {
    // H-5: the rule is the same for an `enum`.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_enum.kp",
        "true\ntrue\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn a_view_compares_with_a_view_and_a_str() {
    // H-5: `View` against `View` and `View` against `str` lower to the builtin
    // `View.eq`, which compares the bytes and the lengths.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_view.kp",
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn a_lone_eq_declares_the_operator_and_is_not_a_map_key() {
    // A-12 + H-5: `eq` alone gives `==`/`!=` and leaves the type a *derived*
    // `Map` key (both halves are still what a declared key needs).
    assert_compiles_and_runs(
        "tests/fixtures/map_key_pair_only_eq.kp",
        "true\ntrue\ntrue\n2\n20\n",
    );
}

#[test]
pub(crate) fn the_equality_operator_is_refused_without_a_declared_eq() {
    // H-5: nothing is derived, so a `datac` without `eq` keeps the old
    // refusal, and `<` is refused even on a type that has `eq`.
    assert_rejected_with(
        "tests/fixtures/eq_operator_no_eq.kp",
        "a datac has no generated equality",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_order.kp",
        "cannot compare 'P' values with '<'",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_order_gt.kp",
        "cannot compare 'P' values with '>'",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_bad_signature.kp",
        "'P.eq' takes exactly one parameter",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_type_mismatch.kp",
        "cannot compare 'A' with 'B'",
    );
}

// ---------------------------------------------------------------------------
// Language-completeness 5: operator traits and `Display` (`design/traits.md (kelp wiki)` 13).
// `std/ops` declares `Add`/`Sub`/`Mul`/`Div`/`Rem`/`Neg`/`Ord` and `std/fmt`
// declares `Display`, and the front end lowers each operator and each
// interpolation into the method it names, so both backends run the ordinary
// direct call.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_arithmetic_operators_lower_to_std_ops_methods() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_arithmetic.kp",
        "4\n6\n-1\n0\n-1\n-2\n3\n8\n4\n0\n1\n3\n",
    );
}

#[test]
pub(crate) fn ord_gives_the_order_operators_and_a_hand_written_sort_works() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_ord.kp",
        "true\nfalse\nfalse\ntrue\n1\n9\n2\n0\n2\n1\n",
    );
}

#[test]
pub(crate) fn display_lets_a_value_into_a_template_and_print_and_nests() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_display.kp",
        "v = (3, 4)\n(3, 4)\n3\nline = (3, 4) -> (5, 6)\n(3, 4) -> (5, 6)\n(3, 4) -> (5, 6)\n",
    );
}

#[test]
pub(crate) fn compound_assignment_on_a_user_type_is_the_add_then_assign_desugar() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_compound_assign.kp",
        "4\n6\n1\n2\n3\n8\n4\n5\n",
    );
}

#[test]
pub(crate) fn every_operand_of_an_overloaded_operator_is_evaluated_once_left_to_right() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_operands_once.kp",
        "1\n2\n3\n3\n4\ntrue\n5\n6\n7\n4\n",
    );
}

#[test]
pub(crate) fn an_operator_resolves_on_a_bounded_generic_type_parameter() {
    // A generic body is checked once per instantiation, so `T` is the concrete
    // `Vec2` when `a + b` and `a < b` are lowered.
    assert_compiles_and_runs(
        "tests/fixtures/ops_generic.kp",
        "4\n7\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn an_operator_needs_its_trait_and_the_order_trait_is_not_equality() {
    assert_rejected_with(
        "tests/fixtures/ops_arith_no_add.kp",
        "it does not implement 'Add' from 'std/ops'",
    );
    assert_rejected_with(
        "tests/fixtures/ops_ord_no_eq.kp",
        "'==' and '!=' come from a declared 'eq' method, and 'Ord' does not supply one",
    );
    assert_rejected_with(
        "tests/fixtures/ops_display_missing.kp",
        "it does not implement 'Display'",
    );
    assert_rejected_with(
        "tests/fixtures/ops_orphan_builtin.kp",
        "'i32' is a builtin type, which has no module",
    );
    assert_rejected_with(
        "tests/fixtures/ops_mixed_types.kp",
        "the operands must have the same type",
    );
}

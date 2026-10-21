use super::*;

// ---------------------------------------------------------------------------
// MISSING-z2bB9: tuple types and the destructuring binding that takes one
// apart (`design/syntax.md (kelp wiki)` 3 and 4). A tuple is a record with positional
// fields, so it is a value like every other owned one: it owns its elements,
// it takes the derived deep copy and the derived order a `datac` takes in
// position order, it may appear in every position the element rules allow, and
// it is refused in the positions they refuse.
// ---------------------------------------------------------------------------

/// The item's own example: a function hands back two things as one tuple, and
/// the caller takes it apart, so no one-shot `datac` is needed per pair.
#[test]
pub(crate) fn a_function_hands_back_two_things_as_a_tuple() {
    assert_compiles_and_runs("tests/fixtures/tuple_min_max.kp", "1\n9\n8\n");
}

/// A tuple is a `datac` field, a parameter (a borrow) and a `move` parameter,
/// a `List` element, a `Map` key and value, a `Box` element, an array element,
/// an `enum` payload and a generic argument. Every position follows the element
/// rule of the position rather than a second set of rules of its own, so this
/// fixture is also what says a tuple key is keyable by derivation.
#[test]
pub(crate) fn a_tuple_is_a_value_in_every_position() {
    assert_compiles_and_runs("tests/fixtures/tuple_datac_field.kp", "4\n5\n14\n");
    assert_compiles_and_runs("tests/fixtures/tuple_param.kp", "3\nmoved\n7\n");
    assert_compiles_and_runs(
        "tests/fixtures/tuple_placements.kp",
        "30\n15\n3\n2\n4\ntrue\n19\n6\n",
    );
}

/// A tuple has two or more elements: `(T)` is the parenthesized type `T` and
/// `(e)` the parenthesized expression `e`. A tuple may hold another tuple, a
/// destructuring binding may nest, `_` ignores a position at any level, and
/// `var (a, b) = ...` makes every position a `var`.
#[test]
pub(crate) fn a_tuple_has_two_or_more_positions_and_a_pattern_takes_it_apart() {
    assert_compiles_and_runs("tests/fixtures/tuple_one_element.kp", "42\n42\n42\n");
    assert_compiles_and_runs("tests/fixtures/tuple_nested.kp", "1\n3\n11\n5\n3\n");
}

/// `?` reaches the `Result` a tuple *element* may be and never the tuple:
/// `mk(n)?` is the tuple the call succeeded with, and it destructures like any
/// other value.
#[test]
pub(crate) fn question_about_a_tuple_element_is_about_that_element() {
    assert_compiles_and_runs("tests/fixtures/tuple_try.kp", "5\n6\nnegative\n");
}

/// A tuple takes the derived deep copy and the derived order a `datac` takes,
/// position by position. The copy is deep (the original list is cleared before
/// the copy is read, so a shared buffer is a use-after-free) and the order is
/// the first position's, then the second's, which is what `copy.sort()` and the
/// reads after it pin.
#[test]
pub(crate) fn a_tuple_gets_the_derived_copy_and_order_a_datac_does() {
    assert_compiles_and_runs(
        "tests/fixtures/tuple_derived_copy_sort.kp",
        "3\na\n1\na\n3\nb\n2\n0\n7\nkept\n",
    );
}

/// The refusals, each naming the rule it breaks: the empty type, a borrowed
/// element, the absent value as an element, a pattern whose position count does
/// not match, a destructuring of something that is not a tuple, an element
/// naming a type declared later (open item 12, the by-value field rule), an
/// owned element read out of a borrow, the initializer used twice, and a tuple
/// handed to `print`.
#[test]
pub(crate) fn the_tuple_surface_is_refused_where_its_rules_refuse_it() {
    for (fixture, needle) in [
        ("tuple_empty_type", "'()' is not a type"),
        (
            "tuple_borrowed_element",
            "element 1 of the tuple is a borrowed type ('str')",
        ),
        ("tuple_unit_element", "element 2 of the tuple is 'unit'"),
        ("tuple_arity", "this pattern has 3 position(s)"),
        (
            "tuple_not_a_tuple",
            "a destructuring binding takes one tuple value apart",
        ),
        ("tuple_later_element", "'Later' is declared after this type"),
        (
            "tuple_element_borrow",
            "cannot read element 1 of the tuple out of a borrow by value",
        ),
        ("tuple_moved_twice", "use of moved value 'p'"),
        (
            "tuple_print",
            "cannot pass a value of type '(i32, i32)' to 'print'",
        ),
    ] {
        assert_rejected_with(&format!("tests/fixtures/{fixture}.kp"), needle);
    }
}

// Open item 75 prototype (`kelp issue #242`): a `{ ... }` on the same
// line as a call is that call's last argument, a zero-parameter Unit `fun`.
#[test]
pub(crate) fn a_trailing_block_is_the_calls_last_argument() {
    assert_compiles_and_runs("tests/fixtures/trailing_block.kp", "a\na\nb\nc\nshow\nshow\n");
}

#[test]
pub(crate) fn a_trailing_block_can_capture_into_a_closure_parameter() {
    assert_compiles_and_runs("tests/fixtures/trailing_block_capture.kp", "3\n3\n");
}

/// A program whose reader goes away must be ended by SIGPIPE, as every Unix tool
/// is. The TLS code blocks the signal only around the OpenSSL calls that can
/// write, never process-wide: a runtime that ignored it would leave this loop
/// writing to a closed pipe forever, and `producer | head` would never return.
#[test]
pub(crate) fn a_program_is_ended_by_sigpipe_when_its_reader_closes_the_pipe() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::ExitStatusExt;
    let bin = out_path("pipe_producer");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/hazards/pipe_producer.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'pipe_producer' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let mut child = Command::new(&bin)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run compiled binary");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut first = String::new();
    reader.read_line(&mut first).expect("the producer prints a first line");
    assert_eq!(first.trim(), "0");
    drop(reader);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait on the producer") {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the producer kept running after its reader closed the pipe: SIGPIPE is being ignored");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.signal(), Some(13), "expected death by SIGPIPE, got {status:?}");
}

// `kelp wiki page language/functions.md`: a trailing block declares its parameters by name
// only and returns its tail where the expected type is non-unit, and a
// `scoped fun(...)` parameter accepts such a block (or a `fun` literal)
// written at the call site, capturing the enclosing frame by borrow.
#[test]
pub(crate) fn a_scoped_block_takes_its_parameters_and_return_from_the_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_params.kp",
        "4\n5\n6\n150\n101\n102\n12\n6\n",
    );
}

#[test]
pub(crate) fn a_scoped_block_can_read_a_borrowed_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_borrow.kp",
        "row\nrow\n203\nrow\n",
    );
}

#[test]
pub(crate) fn a_scoped_block_captures_owned_copy_and_borrowed_values() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_captures.kp",
        "108\n109\n7\n2030\n5\n",
    );
}

#[test]
pub(crate) fn a_scoped_block_in_a_loop_sees_the_current_iteration() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_borrow_in_loop.kp",
        "1\n2\n2\n3\n3\n4\n",
    );
}

#[test]
pub(crate) fn a_generic_scoped_parameter_types_its_block_from_the_expected_return() {
    assert_compiles_and_runs("tests/fixtures/scoped_block_generic.kp", "3\n");
}

#[test]
pub(crate) fn a_scoped_value_cannot_be_stored_in_a_datac() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_stored.kp",
        "scoped function value",
    );
}

#[test]
pub(crate) fn a_scoped_value_cannot_be_returned() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_returned.kp",
        "return type mismatch",
    );
}

#[test]
pub(crate) fn a_scoped_block_can_write_what_it_captures() {
    // `kelp wiki page language/functions.md`: a write through a captured `var` makes the
    // capture exclusive for the call.
    assert_compiles_and_runs("tests/fixtures/excl_capture_field.kp", "13\n113\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_copy_var.kp", "1006\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_list_push.kp", "0\n10\n20\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_mut_param.kp", "103\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_two_owners.kp", "63\n2\n2\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_returns_value.kp", "40\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_defer_loop.kp", "3\n30\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_drain.kp", "1\n6\n8\n8\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_view.kp", "2\n500\n");
}

#[test]
pub(crate) fn an_exclusive_capture_conflicts_with_the_rest_of_its_call() {
    assert_rejected_with(
        "tests/fixtures/excl_refused_same_call_arg.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_two_blocks.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_shared_arg.kp",
        "cannot borrow 's.count' as exclusive: already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_receiver.kp",
        "cannot borrow 's.count' as exclusive: already borrowed exclusively",
    );
    // The receiver `s.bag` and the block's `s.count` are different fields of `s`.
    assert_compiles_and_runs("tests/fixtures/excl_capture_disjoint_receiver.kp", "1\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_mut_param_twice.kp",
        "needs the call's other arguments to leave it alone",
    );
    assert_rejected_with(
        "tests/fixtures/refused_mut_param_reborrow_twice.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_live_borrow.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
}

#[test]
pub(crate) fn a_mut_parameter_can_be_reborrowed_shared_twice_for_one_call() {
    // `borrowck::take_call_arguments` must use the *callee* parameter modes:
    // `show(s, s)` is two shared reborrows, while `touch(s, s)` is rejected.
    assert_compiles_and_runs("tests/fixtures/mut_param_shared_reborrow_twice.kp", "4\n4\n5\n");
}

#[test]
pub(crate) fn a_mut_parameter_cannot_go_to_a_mut_and_a_shared_parameter_of_one_call() {
    // The shared reborrow of a `mut` parameter is an `Ident`, not a `Ref`,
    // so only the exclusive reborrow used to be tracked: `f(s, s)` with one
    // `mut` and one `&` parameter was accepted in either order.
    assert_rejected_with(
        "tests/fixtures/refused_mut_param_with_shared_reborrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_shared_then_mut_param_reborrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
}

#[test]
pub(crate) fn a_tuple_has_positions_patterns_in_when_and_a_destructuring_for() {
    // `t.0`, `for (a, b) in xs`, and `when (a, b) { (Some(x), _) -> ... }`
    // (MISSING-Gt6Qc), on both backends.
    assert_compiles_and_runs("tests/fixtures/tuple_access_and_patterns.kp", "1\n2\n3\n2\n12\n8\n5\n40\n0\n3\n");
    assert_compiles_and_runs("tests/fixtures/tuple_when_owned_payloads.kp", "5\n300\n");
    assert_compiles_and_runs("tests/fixtures/tuple_index_nested.kp", "7\n9\n2\n");
    // A tuple as a `const` value.
    assert_compiles_and_runs("tests/fixtures/const_tuple.kp", "1\n3\n7\ntrue\n14\n2\n98\n");
    // Literal positions and nested tuple patterns in a `when`.
    assert_compiles_and_runs(
        "tests/fixtures/tuple_when_literals_and_nested.kp",
        "100\n7\n30\n-1\n7\n7\n5\n8\n11\n0\n1\n2\n3\n4\n",
    );
    // `for (k, v) in map` (the `Map` iteration of MISSING-Hd4Lr).
    assert_compiles_and_runs("tests/fixtures/for_over_a_map.kp", "1\n2\n3\n10\n20\n7\n5\n");
    assert_rejected_with(
        "tests/fixtures/refused_tuple_when_literals_without_fallback.kp",
        "needs an arm that matches anything there",
    );
    assert_rejected_with(
        "tests/fixtures/refused_tuple_when_not_exhaustive.kp",
        "this 'when' is not exhaustive: 'None' of enum",
    );
}

#[test]
pub(crate) fn assigning_to_something_that_is_not_a_place_is_an_error_not_a_crash() {
    // Found by the front-end fuzzer: `str.starts_with -= (...)` reached an
    // `unreachable!` in `check_target_writable`.
    assert_rejected_with(
        "tests/fixtures/refused_assign_to_a_function.kp",
        "cannot assign to this expression",
    );
}

#[test]
pub(crate) fn loop_and_labelled_break_and_continue_leave_nested_loops() {
    // MISSING-Lk9Xs: `loop { }`, `'outer: for ...`, `break 'outer` and
    // `continue 'outer`, on both backends.
    assert_compiles_and_runs("tests/fixtures/loop_and_labels.kp", "4\n4\n51\n");
    // Each exit frees the owned locals of every loop it leaves.
    assert_compiles_and_runs("tests/fixtures/loop_labels_free_owned_locals.kp", "9\n9\n7\n");
    // `loop` as a value and `break value` (the rest of MISSING-Lk9Xs).
    assert_compiles_and_runs("tests/fixtures/loop_value.kp", "9\n8\n4\n23\n");
    assert_rejected_with(
        "tests/fixtures/refused_break_value_in_statement_loop.kp",
        "only a 'loop' used as a value can 'break' with one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_plain_break_in_value_loop.kp",
        "every 'break' out of it carries one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_value_loop_without_break_value.kp",
        "needs a 'break value' to produce one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_unknown_loop_label.kp",
        "no enclosing loop is labelled 'nope",
    );
    assert_rejected_with(
        "tests/fixtures/refused_label_across_a_closure.kp",
        "no enclosing loop is labelled 'o",
    );
}

#[test]
pub(crate) fn a_method_can_declare_type_parameters_of_its_own() {
    // MISSING-Pw3Fz: `fun Box2.map<U>(...)` on a plain type and
    // `fun Pair<A, B>.map_first<C>(...)` on a generic one, inferred at the
    // call, with a `mut` receiver and a trait bound.
    assert_compiles_and_runs("tests/fixtures/method_own_type_params.kp", "3\ntrue\ntrue\n2\n2\n7\n");
    assert_compiles_and_runs("tests/fixtures/method_own_type_params_bound.kp", "11\n");
}

#[test]
pub(crate) fn a_generic_enum_can_carry_methods_and_option_and_result_do() {
    // The receiver on a generic `enum`'s template used to be refused; now
    // `fun Opt<T>.map<U>(...)` is a method, and `std/option` / `std/result`
    // carry their borrowing combinators as methods too.
    assert_compiles_and_runs("tests/fixtures/enum_generic_method.kp", "true\n40\n");
    assert_compiles_and_runs("tests/fixtures/option_result_methods.kp", "true\nfalse\n5\n4\ntrue\nfalse\n");
}

#[test]
pub(crate) fn small_ergonomics_literal_float_math_f32_and_line_continuation() {
    // H-12: an integer literal where a float is expected is that float.
    assert_compiles_and_runs(
        "tests/fixtures/literal_adapts_to_float.kp",
        "5.000000\n3.000000\n5.000000\n3.000000\n2.500000\n-4.000000\n1\n",
    );
    // `f32.to_f64()` / `f64.to_f32()`, and the f32 overloads of `std/math`.
    assert_compiles_and_runs("tests/fixtures/float_width_conversion.kp", "2.500000\n4.500000\n");
    assert_compiles_and_runs(
        "tests/fixtures/math_f32_overloads.kp",
        "2.000000\n16.000000\n9.500000\n4.000000\n4.000000\n1.000000\nfalse\n",
    );
    // A line that ends in a binary operator, `,` or `.` goes on (open item 11).
    assert_compiles_and_runs("tests/fixtures/line_continuation_after_operator.kp", "16\n1\n1\n");
    // A `for (a, b)` over a temporary, whose hidden names must not meet the
    // loop desugar's own.
    assert_compiles_and_runs("tests/fixtures/for_pattern_over_a_temporary.kp", "3\n7\n");
}

#[test]
pub(crate) fn two_fields_of_one_owner_are_different_places_to_borrow() {
    // MISSING-Sw9Hd: roots are field paths now. Disjoint fields borrow
    // together (a call, two live borrows, a scoped block against a call's
    // other argument); anything that overlaps is still refused.
    assert_compiles_and_runs("tests/fixtures/borrow_disjoint_fields.kp", "2\n2\n12\n3\n4\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_disjoint_field.kp", "7\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_same_field.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
    assert_rejected_with("tests/fixtures/refused_field_and_whole.kp", "cannot borrow 's': already borrowed exclusively");
    assert_rejected_with(
        "tests/fixtures/refused_same_field_twice.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_whole_while_field_borrowed.kp",
        "cannot move 's' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/refused_field_borrow_then_whole_borrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_field_shared_then_write.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
}

#[test]
pub(crate) fn iterator_adapters_chain_over_a_list_a_range_and_each_other() {
    // MISSING-Hd4Lr: `std/iter`'s `into_iter` and the adapters. A generic type
    // with an iterator-shaped `next` satisfies `I: Iterator` and names
    // `I::Item`, so the adapters nest without a generic trait impl.
    assert_compiles_and_runs("tests/fixtures/iter_list_take_skip.kp", "1\n2\n3\n4\n5\n7\n24\n");
    assert_compiles_and_runs(
        "tests/fixtures/iter_adapters.kp",
        "60\n80\n0\n10\n1\n11\n2\n12\n100\n102\n1\n2\n7\n8\n4\n14\n",
    );
    assert_compiles_and_runs("tests/fixtures/iter_adapters_owned.kp", "3\n5\n0\n1\n1\n2\n");
    // The consumers: any, all, find, last, fold.
    assert_compiles_and_runs("tests/fixtures/iter_consumers.kp", "true\nfalse\n10\n8\n7\n4\n");
}

#[test]
pub(crate) fn a_trait_can_be_implemented_for_a_generic_type_under_bounds() {
    // MISSING-Ve8Nc (the slice that is built): `impl<A: Describe, B: Describe>
    // Describe for Pair<A, B>`, an associated type over the parameters, and a
    // `where` clause as the other spelling of a bound.
    assert_compiles_and_runs("tests/fixtures/generic_impl_pair.kp", "12\n345\n");
    assert_compiles_and_runs("tests/fixtures/generic_impl_iterator.kp", "6\n6\n");
    assert_compiles_and_runs("tests/fixtures/where_projection_bound.kp", "#1\n#0\n2\n#2\n#1\n2\n");
    assert_compiles_and_runs("tests/fixtures/where_equality_constraint.kp", "12\n103\n");
    assert_rejected_with(
        "tests/fixtures/refused_where_projection_bound.kp",
        "the call to 'show_all' needs 'I::Item', which is 'Plain' for 'Plains', to implement the trait 'fmt.Display', and it does not",
    );
    assert_compiles_and_runs("tests/fixtures/impl_where_constraints.kp", "3\n2\n");
    assert_rejected_with(
        "tests/fixtures/refused_impl_where_projection.kp",
        "its impl needs 'I::Item' to implement 'std.fmt.Display', and it does not",
    );
    assert_rejected_with(
        "tests/fixtures/refused_impl_where_equality.kp",
        "its impl needs 'Strs' ('Wrap' is parameter 'I') to bind 'Item' to 'i64', and it binds 'String'",
    );
    assert_rejected_with(
        "tests/fixtures/refused_negative_bound.kp",
        "a negative bound ('!Name') is excluded from generic trait impls (plan/generic-trait-impls.md section 6)",
    );
    assert_compiles_and_runs("tests/fixtures/where_clause.kp", "3\n");
    assert_rejected_with(
        "tests/fixtures/refused_generic_impl_bound.kp",
        "its impl needs 'bool' ('Pair' is parameter 'B') to implement 'Describe', and it does not",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_impl.kp",
        "bounds its parameter on 'Describe' itself",
    );
}

#[test]
pub(crate) fn a_blanket_impl_applies_to_every_type_that_meets_its_bound() {
    // MISSING-Ve8Nc step 4: `impl<T: Describe> Tagged for T` is a template per
    // method, instantiated for each concrete type that meets the bound, so
    // dispatch stays static. Coherence refuses an overlapping impl by name, and
    // a blanket impl's bound is checked where the method is used.
    assert_compiles_and_runs("tests/fixtures/blanket_impl_concrete.kp", "700\n3400\n");
    assert_compiles_and_runs("tests/fixtures/blanket_impl_generic_fn.kp", "20\n48\n");
    assert_rejected_with(
        "tests/fixtures/refused_blanket_overlap.kp",
        "'impl Describe for T' overlaps 'impl Describe for i32': a blanket impl applies to every type that meets its bound, so it overlaps every other impl of 'Describe', and there is no specialization to choose between them: a blanket impl has to be the trait's only impl",
    );
    assert_rejected_with(
        "tests/fixtures/refused_generic_impl_overlap.kp",
        "'impl Tagged for Pair<A>' overlaps 'impl Tagged for Pair<A>': both apply to every 'Pair', so one type would have two impls of 'Tagged', and there is no specialization to choose between them",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_cycle.kp",
        "the blanket impl of 'Describe' bounds its parameter on 'Describe' itself",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_unchecked_bound.kp",
        "whose bound 'Describe' on the receiver cannot be checked on 'List<i32>'",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_dyn.kp",
        "implements 'Tagged' through a blanket impl, and a blanket impl has no 'dyn' form yet",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_projection_bound.kp",
        "binds no associated type 'Item'",
    );
}

#[test]
pub(crate) fn a_nested_owned_field_can_be_partially_moved_and_reinitialised() {
    assert_compiles_and_runs("tests/fixtures/nested_partial_move_field.kp", "Ada\n7\nLondon\n");
    assert_compiles_and_runs("tests/fixtures/nested_partial_move_reinit.kp", "old\nnew\n");
}

#[test]
pub(crate) fn a_parent_of_a_nested_partial_move_is_not_usable_as_a_whole_value() {
    assert_rejected_with(
        "tests/fixtures/refused_nested_partial_move_parent.kp",
        "use of moved value 'p.name'",
    );
}

#[test]
pub(crate) fn an_exclusive_capture_is_refused_where_it_cannot_be_sound() {
    assert_rejected_with(
        "tests/fixtures/excl_refused_val.kp",
        "it is not declared 'var' in the enclosing function",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_shared_param.kp",
        "it is a shared borrow in the enclosing function",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_returns_borrow.kp",
        "(an exclusive capture) and returns a borrow",
    );
    // A block in a block may capture the outer block's exclusive capture
    // (MISSING-Sw9Hd): the inner write is checked against the rest of the inner
    // call exactly as a local's would be.
    assert_compiles_and_runs("tests/fixtures/excl_capture_nested.kp", "2\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_nested_disjoint.kp", "2\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_nested_same_call.kp",
        "cannot borrow 'a captured value.count' as exclusive: this call also uses it",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_stored.kp",
        "scoped function value",
    );
}

#[test]
pub(crate) fn a_scoped_value_cannot_reach_a_plain_function_parameter() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_to_plain.kp",
        "scoped function value",
    );
}

#[test]
pub(crate) fn a_scoped_parameter_rejects_a_stored_value() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_not_literal.kp",
        "never a stored value",
    );
}

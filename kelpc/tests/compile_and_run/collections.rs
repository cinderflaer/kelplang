use super::*;

// ---------------------------------------------------------------------------
// Collections: the builtin `List<T>`.
// ---------------------------------------------------------------------------

/// Builds a fixture, runs it, and asserts the run fails with `needle` on
/// stderr, the runtime-panic path (`List.pop` on an empty list).
pub(crate) fn assert_fixture_panics(name: &str, needle: &str) {
    assert_fixture_panics_all(name, &[needle]);
}

/// The same, for a panic whose diagnostic is more than its first line: every
/// needle must appear, in any order. The syscall-failure panics use this,
/// because `errno` and `strerror` are reported below the stable message.
pub(crate) fn assert_fixture_panics_all(name: &str, needles: &[&str]) {
    let source = format!("tests/fixtures/{name}.kp");
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{name}' to panic at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    for needle in needles {
        assert!(
            stderr.contains(needle),
            "expected the panic for '{name}' to mention '{needle}', got:\n{stderr}"
        );
    }
}

/// Decision 21: the refusal is the point of these fixtures, so each one asserts
/// that a hosted build refuses it, and that the diagnostic names the reason. The
/// refusal is never relaxed to make a fixture build.
pub(crate) fn assert_fixture_refused(name: &str, needle: &str) {
    let source = format!("tests/fixtures/{name}.kp");
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    // The refusal is reported on stdout, so both streams are searched.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build.status.success(),
        "expected a hosted build to refuse '{source}', but it built"
    );
    assert!(
        text.contains(needle),
        "expected the refusal for '{source}' to mention '{needle}', got:\n{text}"
    );
}

#[test]
pub(crate) fn a_list_sorts_reverses_searches_and_slices() {
    assert_fixture_runs("tests/fixtures/stdlib_sort.kp");
}

#[test]
pub(crate) fn a_list_clones_without_sharing_its_elements() {
    // MISSING-7: `clone` is the whole-list case of `slice(0, len)`. Mutating
    // the original after the clone is what proves the copy is not a second
    // handle on the same buffer, and the fixture is in the AddressSanitizer
    // pass because a `List<String>` clone that shared an element would
    // double-free at scope end.
    assert_fixture_runs("tests/fixtures/list_clone.kp");
}

#[test]
pub(crate) fn sorting_and_slicing_a_list_of_owned_elements_moves_them_safely() {
    // The element shifts are moves, so this fixture runs under
    // AddressSanitizer: a double free or a leak here would be a real one.
    assert_fixture_runs("tests/fixtures/list_slice_insert.kp");
}

#[test]
pub(crate) fn a_list_sorts_and_searches_every_element_kind_it_can_hold() {
    // MK7 §5's sort/contains/slice slice: each element kind reaches a different
    // comparison (signed, unsigned, and a length-aware byte compare for the
    // owned buffers) and the two slice paths differ (a plain copy for a `Copy`
    // element, a clone for an owned one). A wrong element width or a
    // cloned-twice element is a memory bug rather than a wrong number, so this
    // fixture is also in the AddressSanitizer list and under valgrind.
    assert_fixture_runs("tests/fixtures/list_sort_kinds.kp");
}

#[test]
pub(crate) fn list_sort_and_slice_reject_what_they_cannot_do() {
    // A `datac`/`enum` gets its order and its copy from its fields, so an
    // element that holds a `List` has neither and the diagnostic names that
    // rather than failing in codegen; a `slice` range past the end panics at
    // runtime.
    assert_rejected("tests/fixtures/list_unsortable_element.kp");
    assert_fixture_panics("list_slice_bounds", "List.slice: range out of bounds");
}

#[test]
pub(crate) fn list_sort_handles_introsorts_own_trouble_spots() {
    // `List#sort` moved from heapsort to introsort (quicksort with a
    // median-of-three pivot, an insertion-sort cutoff, and a depth-limited
    // heapsort fallback). The existing sort fixtures above were written
    // against heapsort and never stressed quicksort's known trouble spots:
    // an already-sorted run, a reverse-sorted run, an all-equal run, and
    // inputs sized just either side of the insertion-sort cutoff. None of
    // these should crash, hang, or blow the stack, and every one must come
    // out sorted; the depth-limit fallback to the existing heapsort is what
    // is supposed to catch the already-sorted/reverse-sorted/all-equal cases
    // before they could degrade toward O(n^2).
    assert_fixture_runs("tests/fixtures/list_sort_introsort_stress.kp");
}

#[test]
pub(crate) fn a_datac_or_enum_element_gets_a_derived_copy_and_order() {
    // MISSING-jL9tE: `List<Digest>.clone()` and `List<Digest>.sort()` were both
    // "not defined" because a `datac` element had neither. The copy is deep
    // (the original is cleared first) and the order is field by field, with an
    // `enum` ordering by tag before payload.
    assert_fixture_runs("tests/fixtures/list_datac_clone_sort.kp");
}

#[test]
pub(crate) fn a_list_grows_indexes_and_shrinks() {
    assert_fixture_runs("tests/fixtures/list_basic.kp");
}

#[test]
pub(crate) fn a_for_loop_walks_a_list_without_an_index() {
    // A-5: the parser desugars `for x in xs { }` to an index and a `while`, so
    // this pins the desugaring where a caller can see it, the element binding
    // (value for `Copy`, shared borrow for owned), a call iterable evaluated
    // once, the same once for a *field* of a call, nesting, and a list a
    // function only borrows.
    assert_fixture_runs("tests/fixtures/for_loop.kp");
}

#[test]
pub(crate) fn a_for_loop_over_list_iter_sums_a_list_of_i32() {
    // Hd4Lr part 1: `xs.iter()` is the same walk as `xs`, with the element a
    // shared borrow of the list.
    assert_fixture_runs("tests/fixtures/for_iter_list_sum.kp");
}

#[test]
pub(crate) fn a_for_loop_over_list_iter_walks_owned_strings() {
    assert_fixture_runs("tests/fixtures/for_iter_list_strings.kp");
}

#[test]
pub(crate) fn a_for_loop_over_list_iter_refuses_a_mutation_of_the_list() {
    // The same refusal a plain `for x in xs` gives: the walked place is `xs`.
    assert_rejected_with(
        "tests/fixtures/for_iter_list_mutated.kp",
        "cannot mutate 'xs' while iterating over it",
    );
}

#[test]
pub(crate) fn a_borrowed_list_iteration_reads_every_element_once() {
    // `for x in xs.iter()` walks all five elements (count 5, sum 30), nests,
    // lets the body change another list, and allows `xs` to change after the
    // loops. The string walk reads each name through its borrow (6 letters).
    assert_fixture_runs("tests/fixtures/for_iter_list_reads_every_element.kp");
}

#[test]
pub(crate) fn a_borrowed_list_iteration_refuses_every_mutator_of_the_list() {
    // The borrow is the walk itself, so a mutator of `xs` while the loop runs
    // is refused by the same rule as `for x in xs`, including from a nested
    // borrowed walk.
    for fixture in [
        "for_iter_list_refuses_set",
        "for_iter_list_refuses_remove",
        "for_iter_list_refuses_nested_push",
    ] {
        assert_rejected_with(
            &format!("tests/fixtures/{fixture}.kp"),
            "cannot mutate 'xs' while iterating over it",
        );
    }
}

#[test]
pub(crate) fn a_for_loop_over_map_iter_walks_the_entries() {
    assert_fixture_runs("tests/fixtures/for_iter_map.kp");
}

#[test]
pub(crate) fn a_for_loop_needs_its_in() {
    // `in` is not a keyword (syntax.md 2.3): it is read contextually, so the
    // diagnostic says what is missing instead of resolving a function `in`.
    assert_rejected_with("tests/fixtures/for_missing_in.kp", "'in'");
}

#[test]
pub(crate) fn a_must_use_result_cannot_be_dropped() {
    // MISSING-Cn2Ym: the marker is on `Sender.send` and the `fs.try_*`
    // mutators, and a user function can carry it too.
    assert_rejected_with("tests/fixtures/must_use_send_dropped.kp", "must be used");
    assert_rejected_with("tests/fixtures/must_use_user_dropped.kp", "'check' must be used");
    assert_fixture_runs("tests/fixtures/must_use_user_fun.kp");
}

#[test]
pub(crate) fn a_for_loop_only_walks_a_list_or_an_iterator() {
    // A `List`/array keeps the index walk; anything else has to implement
    // `std.iter.Iterator`, and the refusal names the trait so the fix is one
    // `impl` away (`kelp wiki page language/iterators.md`). The span is the `for` line.
    assert_rejected_with("tests/fixtures/for_not_a_list.kp", "implements 'Iterator'");
    assert_rejected_with("tests/fixtures/for_iter_no_impl.kp", "implements 'Iterator'");
}

#[test]
pub(crate) fn a_for_loop_refuses_every_mutation_of_what_it_walks() {
    // `for x in xs { xs.push(x) }` was accepted for a `Copy` element (nothing
    // borrowed the list) and grew the list until memory ran out. Every mutation
    // form is now one refusal, naming the place the `for` walks.
    for fixture in [
        "for_mutates_iterated_push",
        "for_mutates_iterated_remove",
        "for_mutates_iterated_set",
        "for_mutates_iterated_assign",
        "for_mutates_iterated_field",
        "for_mutates_iterated_mutref",
        "for_mutates_iterated_mutarg",
        "for_mutates_iterated_capture",
        "for_mutates_iterated_move",
        "for_mutates_iterated_nested",
    ] {
        assert_rejected_with(
            &format!("tests/fixtures/{fixture}.kp"),
            "while iterating over it",
        );
    }
    // The message names the place: a local for a local, a field path for a
    // field.
    assert_rejected_with(
        "tests/fixtures/for_mutates_iterated_push.kp",
        "cannot mutate 'xs' while iterating over it",
    );
    assert_rejected_with(
        "tests/fixtures/for_mutates_iterated_field.kp",
        "cannot mutate 'self.items' while iterating over it",
    );
}

#[test]
pub(crate) fn a_for_loop_still_allows_reads_another_list_and_a_while() {
    // The rule covers mutation of the walked place only: reading it, changing a
    // different list, changing it after the loop, and the index-`while` that is
    // the supported way for a loop to change what it walks all stay legal.
    assert_fixture_runs("tests/fixtures/for_iterated_place_is_read_only.kp");
    assert_fixture_runs("tests/fixtures/for_mutates_another_field.kp");
}

#[test]
pub(crate) fn a_for_loop_over_an_iterator_walks_it_by_next() {
    // `kelp wiki page language/iterators.md`: a type implementing `std.iter.Iterator` walks through
    // `next()`, and `Range` is the shipped one. A step and a descending step are
    // the same loop with a different `step`.
    assert_fixture_runs("tests/fixtures/for_iter_range.kp");
}

#[test]
pub(crate) fn a_user_iterator_loop_stops_on_break_and_skips_on_continue() {
    // The sequence is infinite, so `break` is the loop's exit; `continue`
    // reaches the next `next()` call.
    assert_fixture_runs("tests/fixtures/for_iter_break.kp");
}

#[test]
pub(crate) fn a_defer_in_an_iterator_body_runs_once_per_iteration() {
    assert_fixture_runs("tests/fixtures/for_iter_defer.kp");
}

#[test]
pub(crate) fn two_iterator_loops_nest_with_independent_cursors() {
    assert_fixture_runs("tests/fixtures/for_iter_nested.kp");
}

#[test]
pub(crate) fn the_iterator_helpers_count_sum_and_collect() {
    assert_fixture_runs("tests/fixtures/for_iter_helpers.kp");
}

#[test]
pub(crate) fn a_place_iterator_is_borrowed_by_its_loop() {
    // The loop holds `&mut f`, so naming `f` in the body is the ordinary
    // "already borrowed" error, the iterator mutability the design promises.
    assert_rejected_with(
        "tests/fixtures/for_iter_borrowed.kp",
        "already borrowed",
    );
}

#[test]
pub(crate) fn a_list_of_owned_and_datac_elements_moves_them() {
    assert_fixture_runs("tests/fixtures/list_owned.kp");
}

#[test]
pub(crate) fn a_list_works_as_a_datac_field_and_as_an_owned_passdown() {
    assert_fixture_runs("tests/fixtures/list_field.kp");
}

#[test]
pub(crate) fn a_list_inserts_and_removes_by_shifting_owned_elements() {
    // The shifts are moves of a twenty-four-byte `String`, so this fixture
    // runs under AddressSanitizer: a shift that copied the wrong number of
    // bytes would leak a buffer or double-free one. It is kept apart from
    // `list_slice_insert.kp` because that one reaches `insert`/`remove` only
    // alongside `sort`/`contains`/`slice`.
    assert_fixture_runs("tests/fixtures/list_insert_remove_shifts.kp");
}

#[test]
pub(crate) fn a_list_grows_past_several_doublings_and_reuses_its_buffer() {
    // Three hundred pushes is six `realloc`s, each one moving every element;
    // clearing a list keeps its buffer, so the refilled list lands twice the
    // checksum from the capacity it already had.
    assert_fixture_runs("tests/fixtures/list_growth_and_rebuild.kp");
}

#[test]
pub(crate) fn a_float_list_copies_and_compares_at_its_own_width() {
    // A plain `List<f64>` element: the copy width is eight bytes in a float
    // register, and `contains` compares with a float `==` rather than an
    // integer one. The differential fuzzer found the `llvm` backend emitting
    // an integer compare for the float element, which was invalid IR. The
    // narrow `f32` list below goes through the same path at four bytes.
    assert_fixture_runs("tests/fixtures/list_float_element.kp");
}

#[test]
pub(crate) fn a_list_copies_elements_at_their_own_width() {
    // A one-byte `bool`, an eight-byte `i64` and a twelve-byte `Trio`, the
    // last is not a power of two, so copying it is two instructions rather
    // than one, and every element this corpus had before was four or
    // thirty-two bytes wide.
    assert_fixture_runs("tests/fixtures/list_element_widths.kp");
}

#[test]
pub(crate) fn a_narrow_aggregate_field_is_read_at_its_own_width() {
    // The regression fixture for a wrong-width load in an aggregate field read:
    // the read loaded a whole `QWORD` whatever the field's own width was. A
    // padded stack slot absorbs the extra bytes, so the printed number came out
    // right and a stdout comparison could not see it, but a field read out of a
    // `Box` block, which is `sizeof(Outer)` bytes and nothing more, made the
    // load cross the allocation, which the valgrind sweep reports.
    // Field widths 1, 2, 3, 4, 5 and 7 are here (the ones the machine cannot
    // move in one instruction are 3, 5 and 7), each read whole and byte by
    // byte, plus an 8-byte field, for which the single load is right.
    assert_fixture_runs("tests/fixtures/heap_narrow_fields.kp");
}

#[test]
pub(crate) fn a_list_returns_a_narrow_element_at_its_own_width() {
    // The same defect in the second place a heap element's own address reaches
    // a one-eightbyte load: `List#pop`/`List#remove` hand the element address
    // to `emit_elem_return`, whose `One` arm loaded eight bytes, and whose
    // `Two` arm loaded a full second eightbyte, out of a buffer sized to the
    // element. The four pushes fill the initial capacity so both pops come off
    // the buffer's end, where the crossing load is a heap overread.
    assert_fixture_runs("tests/fixtures/list_narrow_element_pop.kp");
}

#[test]
pub(crate) fn setting_an_owned_element_frees_the_one_it_replaces() {
    assert_fixture_runs("tests/fixtures/list_owned_replacement.kp");
}

#[test]
pub(crate) fn inserting_or_removing_out_of_bounds_panics() {
    // One past the end is a legal insertion point and index 0 is out of
    // bounds once the list is empty, so both messages are pinned from the
    // side the check is actually written against.
    assert_fixture_panics("list_insert_bounds", "List.insert: index out of bounds");
    assert_fixture_panics("list_remove_bounds", "List.remove: index out of bounds");
}

#[test]
pub(crate) fn pushing_an_owned_element_moves_it() {
    assert_rejected("tests/fixtures/list_moved_element.kp");
}

#[test]
pub(crate) fn a_borrow_cannot_be_a_list_element() {
    assert_rejected("tests/fixtures/list_borrow_element.kp");
}

#[test]
pub(crate) fn an_empty_list_needs_its_element_type() {
    assert_rejected("tests/fixtures/list_no_element_type.kp");
}

#[test]
pub(crate) fn an_empty_container_takes_its_element_from_the_first_use() {
    // The same declaration compiles once something hands the element in: a
    // `push`, a `push` in a nested block, a `push` under a `for`, a map's
    // `insert`, a box's `set`, and a position that expects a container (a
    // parameter, and a function's return type).
    assert_fixture_runs("tests/fixtures/container_element_from_use.kp");
}

#[test]
pub(crate) fn popping_an_empty_list_panics() {
    assert_fixture_panics("list_pop_empty", "List.pop: the list is empty");
}

#[test]
pub(crate) fn a_list_may_hold_a_list_element() {
    // A-10 slice 1: an element lives in the outer list's own buffer, so a
    // `List<List<i32>>` is a list of growable buffers, each dropped once by the
    // outer `set`/`pop`/`clear`.
    assert_fixture_runs("tests/fixtures/list_nested_list.kp");
}

#[test]
pub(crate) fn a_nested_list_of_owned_elements_moves_borrows_and_drops() {
    // The `String`-owning nested list through push/get/set/pop/clear, plus a
    // nested list that drops at scope exit. Valgrind walks the nested drop
    // path (see `tests/memory_gate.rs`).
    assert_fixture_runs("tests/fixtures/list_nested_owned.kp");
}

#[test]
pub(crate) fn a_list_may_hold_a_box_element() {
    // A-10 slice 1: an element is the box's own struct, so `Box.empty()` is a
    // `NULL` slot the list still drops exactly once, and `get` hands back a
    // borrow of the box.
    assert_fixture_runs("tests/fixtures/list_box_element.kp");
}

#[test]
pub(crate) fn a_bare_list_settles_on_a_nested_element() {
    // H-6's open container meets A-10 slice 1: `xs.push(List.of(1))` settles
    // `xs` on `List<List<i32>>`, and a `Box` element settles it the same way.
    assert_fixture_runs("tests/fixtures/list_nested_open.kp");
}

/// The open-container marker (`Unit` where an element type goes) is not an
/// element at any depth. `List.of` over a bare `List.empty()` would give the
/// outer list the marker as its element, and there is then no element type to name. The gate refuses first, and says what to do about it.
#[test]
pub(crate) fn a_container_of_an_undecided_element_cannot_be_a_list_element() {
    assert_rejected_with(
        "tests/fixtures/list_of_open_list.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// The same marker through `push`, where the open receiver is what would decide
/// the element: the slot would settle on `List<List<Unit>>`, so the element is
/// refused before the receiver is repaired.
#[test]
pub(crate) fn a_push_cannot_settle_an_open_list_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/list_nested_open_marker.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// `insert` names its element second, after the index, and the same gate sits
/// on both paths.
#[test]
pub(crate) fn an_insert_cannot_settle_an_open_list_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/list_insert_open_list.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// A box may hold another container, but not one whose element type is undecided:
/// `Box.of` over a bare `Box.empty()` would put the marker in the outer box.
#[test]
pub(crate) fn a_container_of_an_undecided_element_cannot_be_a_box_element() {
    assert_rejected_with(
        "tests/fixtures/box_of_open_box.kp",
        "'Box<unit>' cannot be a 'Box' element: a container whose element type is not decided yet",
    );
}

/// The `Box.set` half of the open-decides path, refused for the same reason.
#[test]
pub(crate) fn a_box_set_cannot_settle_an_open_box_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/box_set_open_box.kp",
        "'Box<unit>' cannot be a 'Box' element: a container whose element type is not decided yet",
    );
}

#[test]
pub(crate) fn split_returns_a_list_of_owned_pieces_and_join_consumes_one() {
    assert_fixture_runs("tests/fixtures/stdlib_list.kp");
}

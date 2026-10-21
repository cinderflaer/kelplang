use super::*;

// ---------------------------------------------------------------------------
// M6.1: the builtin owned heap byte buffer (`Bytes`).
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn bytes_construct_measure_index_mutate_clone_and_print() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_bytes.kp",
        "5\n104\nHello\nHello\nfalse\n0\n",
    );
}

#[test]
pub(crate) fn a_bytes_value_moves() {
    // `Bytes` is non-Copy, so binding it to a second name moves it and the
    // first name is dead (M3's rule, now with a type that makes it matter).
    assert_rejected("tests/fixtures/heap_use_after_move.kp");
}

#[test]
pub(crate) fn a_moved_from_binding_can_be_reassigned() {
    assert_compiles_and_runs("tests/fixtures/heap_move_then_rebind.kp", "2\n1\n");
}

#[test]
pub(crate) fn an_owned_value_can_be_returned_by_a_function() {
    // The callee must not free it; the caller must.
    assert_compiles_and_runs("tests/fixtures/heap_returned.kp", "5\nhello\n");
}

#[test]
pub(crate) fn an_owned_local_dies_at_the_end_of_its_block() {
    assert_compiles_and_runs("tests/fixtures/heap_scope_drop.kp", "first\nafter\n");
}

#[test]
pub(crate) fn drops_follow_control_flow() {
    // A loop body, a `return` from inside a branch with another owned local
    // still live, and a block used as a value.
    assert_compiles_and_runs(
        "tests/fixtures/heap_drops_in_control_flow.kp",
        "loop\nloop\nextra\nkeep\n5\n",
    );
}

#[test]
pub(crate) fn an_owned_local_in_a_desugared_block_is_dropped_once_and_in_its_own_block() {
    // BUG-1: `if let` desugars to a `when` whose synthesized `else` block
    // carried the `then` block's span, and `for` builds an element block
    // inside a `while` body that carries the loop body's span too. A drop
    // plan keyed by span then emitted the arm's `xs` on the `else` path (where
    // the name is not declared, so `cc` refused the program) and `inner`
    // twice. The fixture declares an owned local in each position.
    assert_compiles_and_runs(
        "tests/fixtures/heap_drops_in_desugared_blocks.kp",
        "1\n1\n",
    );
}

#[test]
pub(crate) fn reassigning_an_owned_binding_frees_the_old_value() {
    assert_compiles_and_runs("tests/fixtures/heap_reassign.kp", "three\nagain\n");
}

#[test]
pub(crate) fn ownership_through_branches_frees_exactly_once() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_branches.kp",
        "second\nelse\nfirst\n",
    );
}

#[test]
pub(crate) fn a_move_one_path_takes_is_freed_on_the_path_that_did_not() {
    // The drop-plan defect `memory_gate.rs` found in `std_str_order`: a move
    // out of an owned local that only one path through a branch takes used to
    // mark the binding moved for *both* paths, so the branch that kept the
    // value leaked it. The fixture also covers the assignment after the join,
    // which frees the old value, the path that moved it cleared the slot, so
    // that free finds nothing there.
    assert_compiles_and_runs(
        "tests/fixtures/owned_conditional_move.kp",
        "live\nfresh\nfresh\n",
    );
}

#[test]
pub(crate) fn an_owned_local_bound_in_a_loop_body_is_dropped_on_both_paths() {
    // The defect's own minimal repro (MK7 §5): the owned local is bound inside
    // the loop body, moved on one path through it and abandoned on the other,
    // so the body's end-of-block drop has to run on the path that left the value
    // behind, and the two `while`-pass plan write must not lose it.
    assert_compiles_and_runs("tests/fixtures/owned_move_in_loop.kp", "hi\n");
}

#[test]
pub(crate) fn an_assignment_through_a_dereferenced_borrow_frees_the_old_value() {
    // roadmap open item 15, case 2: `*p = value` (and `p.field = value`
    // through a borrowed base) used to leak the value it overwrote, because
    // `DropPlan::at_assign` only ever recorded a plain binding, and neither
    // target has one, there is no declaration to key a `DropTarget` to,
    // only a place. Both backends now free the old value at the target's own
    // address instead of resolving a binding.
    assert_compiles_and_runs("tests/fixtures/assign_through_deref.kp", "second\nbeta\n");
}

#[test]
pub(crate) fn assigning_an_owned_field_frees_the_field_s_old_value() {
    // roadmap open item 15, case 3: `h.name = s` used to leak the field's old
    // buffer for the same reason case 2 did. Also covers `h.name = h.name`
    // and a field reassigned after being partially moved out of: both leave
    // the field's slot already zeroed by the time this drop runs, so the
    // fixture is what pins that freeing zero stays a safe no-op rather than
    // a double free.
    assert_compiles_and_runs(
        "tests/fixtures/assign_owned_field.kp",
        "beta\ngamma\ndelta\nepsilon\n",
    );
}

#[test]
pub(crate) fn a_rebound_or_shadowed_owned_local_is_dropped_for_its_own_value() {
    // BUG-n7Wq3: the owned set kept one entry per *name* per scope, so a second
    // `var a` in one block replaced the first entry and the buffer the first
    // binding owned had no drop left that reached it, while the C backend
    // resolved the one remaining drop to the newer binding. Both buffers are
    // freed now, the nested block where one spelling is two bindings of two
    // scopes stays pinned beside them, and a partial move after the rebind pins
    // that the clear names the binding that owns the value it empties.
    assert_compiles_and_runs(
        "tests/fixtures/owned_rebind.kp",
        "first\nsecond\ninner\nouter\nnew\n2\n",
    );
}

#[test]
pub(crate) fn a_bytes_parameter_is_a_borrow_not_a_move() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_param_borrow.kp",
        "2\nhi\nXi\nfalse\n",
    );
}

#[test]
pub(crate) fn a_static_str_datac_field_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/datac_str_field.kp", "x\n");
}

#[test]
pub(crate) fn printing_a_temporary_owned_buffer_works() {
    // Until M6.3 this was a diagnostic telling the author to bind a `val`
    // first; now codegen hoists the temporary and drops it after the printf,
    // and the fixture runs under AddressSanitizer to prove it is freed.
    assert_compiles_and_runs(
        "tests/fixtures/heap_temporary_print.kp",
        "x\ny\nAB\nw = CD\n",
    );
}

#[test]
pub(crate) fn a_buffer_in_a_print_template_takes_two_argument_slots() {
    // A `%.*s` interpolation is two variadic arguments (length, then pointer),
    // so the register numbering counts arguments per piece, every piece after
    // a buffer one sits behind both of its arguments, and the three buffer
    // shapes (an owned temporary, a borrow of a name, a `View`) each supply
    // them from somewhere different. `heap_temporary_print` above is the
    // single-temporary case this slice was credited with; this fixture is the
    // numbering case, with a scalar, a float and a second buffer behind the
    // first buffer piece.
    assert_compiles_and_runs(
        "tests/fixtures/template_buffer_pieces.kp",
        "1 owned=one scalar=7 float=1.500000\n\
         2 borrowed=named after=9\n\
         3 view=bcd tail=42\n\
         4 bytes=by len=5\n\
         5 two buffers=ABcd\n",
    );
}

// ---------------------------------------------------------------------------
// M6.2: owned `String`, `extern fun`, the first stdlib module, owned `datac`
// fields, and owned passdowns.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn string_constructs_measures_and_round_trips_through_bytes() {
    // `len` is bytes (h + é(2) + l + l + o = 6) and `char_count` is
    // characters (5), the distinction the two functions exist to make.
    assert_compiles_and_runs(
        "tests/fixtures/string_basic.kp",
        "6\n5\nfalse\nhéllo\nhéllo\n6\nhéllo\ntrue\n0\n",
    );
}

// ---------------------------------------------------------------------------
// S1: growing an owned buffer in place, and failing deliberately.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_string_grows_in_place_and_survives_growth() {
    // Every append re-terminates the buffer, so printing `s` after each one
    // is the check that growth kept the `str` view valid.
    assert_compiles_and_runs(
        "tests/fixtures/string_build.kp",
        "0\nhi\nhi there\n8\nhi there!\n9\n0\ntrue\n0\ntrue\nx\n",
    );
}

#[test]
pub(crate) fn bytes_grow_by_push_and_append() {
    assert_compiles_and_runs(
        "tests/fixtures/bytes_build.kp",
        "0\n2\n1\n2\n5\n97\n98\n99\n7\nfalse\n0\ntrue\n1\n9\n",
    );
}

#[test]
pub(crate) fn growing_a_string_needs_an_exclusive_borrow_and_a_var_binding() {
    // `realloc` can move the buffer a live `str` view points into, and a
    // `val` binding is not writable, both are compile errors, which is what
    // makes growth sound here without a lifetime syntax.
    assert_rejected("tests/fixtures/grow_while_borrowed.kp");
    assert_rejected("tests/fixtures/push_to_val_string.kp");
}

#[test]
pub(crate) fn panic_reports_its_message_and_assert_reports_failure() {
    assert_fixture_panics("panic_msg", "panic: deliberate failure");
    assert_fixture_panics("assert_fail", "panic: assertion failed");
}

#[test]
pub(crate) fn a_panic_message_may_be_an_owned_string() {
    // MISSING-8: the borrowed `str` was the only spelling, so a message built
    // in an error path had to be bound to a `val` or spelled `.as_str()`
    // first. An owned temporary and an owned binding both reach `kelp_panic`
    // with the buffer's own bytes.
    assert_fixture_panics("panic_owned_message", "panic: 7");
    assert_fixture_panics("panic_bound_message", "panic: bound message");
    assert_rejected_with(
        "tests/fixtures/panic_not_a_message.kp",
        "a panic message is a 'str' or an owned 'String'",
    );
}

#[test]
pub(crate) fn stdlib_text_builds_numbers_replaces_and_parses() {
    // Concatenation, replacement, and both live edges of the decimal
    // conversion: `u64`'s maximum and `i64`'s minimum, round-tripped through
    // the parsers. Both have a literal spelling now that a `u64` context admits
    // the whole `u64` range (MISSING-x8Wq3); the parser path is what this
    // fixture is testing.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_build.kp",
        "foobar\na:b:c\naaa\n0\n145\n-145\n18446744073709551615\n-9223372036854775808\n9\n",
    );
}

#[test]
pub(crate) fn stdlib_parsing_rejects_anything_but_a_whole_number() {
    // `Result<T, ParseErr>` since MK5-S6-M1: `is_ok`/`is_err`/`unwrap` are the
    // helpers, and `Empty` is a distinguishable reason from `Invalid`.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_parse.kp",
        "true\n123\ntrue\n-9223372036854775808\ntrue\ntrue\ntrue\ntrue\n1\n2\n0\ntrue\n18446744073709551615\ntrue\ntrue\ntrue\n250.000000\ntrue\n104\n233\n3\n0\n5\n",
    );
}

#[test]
pub(crate) fn stdlib_fmt_formats_numbers_hex_padding_and_bytes() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_fmt.kp",
        "0\n-1234\n4294967295\n3.142\n1.5\n0\nff\nbeef\n007\nlong\n7..\n61 62\n\n007!\n0\n999\n1,000\n1,234,567\n18,446,744,073,709,551,615\n-1,234,567\n0 B\n1023 B\n1.00 KiB\n1.50 KiB\n1.00 TiB\n",
    );
}

/// MK7 §5: the owned `String` and the borrowed `View`, both ways round.
///
/// `String` is a MEMORY-class aggregate like `Bytes`; `View` is the first heap
/// type that is *two* INTEGER eightbytes, so it travels in a register pair
/// rather than through the hidden pointer, a shape no heap fixture had reached
/// before this one. A wrong register pair is a wrong string rather than a build
/// failure, which is why the printed bytes are pinned here.
#[test]
pub(crate) fn string_and_view_construct_measure_slice_and_convert() {
    assert_compiles_and_runs(
        "tests/fixtures/string_view_basic.kp",
        "the quick brown fox\n19\nfalse\ntrue\nquick\nthe\nhe quick brown fox\ntrue\ntrue\nfalse\ntrue\nthe\n3\ntrue\n0\ntrue\nquick\n5\nowned text\ntext\nraw\n3\n",
    );
}

/// The bug the spec's own uncertainty list found, and the reason it mattered:
/// `String.with_capacity(0)` left `data == NULL` while claiming to be an empty
/// string, so the first thing that treated it as a C string (`as_str` through
/// `strlen`) segfaulted. This fixture crashed the C backend before the fix and
/// now passes, which is what makes it worth keeping: the hole was in a
/// constructor nothing in the corpus called with zero.
#[test]
pub(crate) fn a_string_with_zero_capacity_is_a_valid_empty_string() {
    assert_compiles_and_runs(
        "tests/fixtures/string_with_capacity_zero.kp",
        "0\ntrue\n0\ntrue\n\n0\n3\nabc\nabcd\n4\ntrue\n0\ntrue\n0\n",
    );
}

/// A literal that does not fit the type it lands on is refused at its own line
/// rather than narrowed to that type (BUG-t6Mz2), on every path that decides a
/// literal's type: the `i32` default when nothing determines one, a type the
/// context supplies, and the declared type a `const` folds into. The one
/// literal wider than its type that survives is the one a sign covers, and
/// `int_literal_width.kp` pins what both backends do with it.
///
/// Before the refusal, the fixture that stood here asserted the narrowing this
/// now rejects: `print(1000000000007)` printed `-727379961`, by the same rule
/// that applied to `val x: i32 = 1000000000007`, and the two backends had once
/// disagreed about it because each narrowed the literal itself.
#[test]
pub(crate) fn an_out_of_range_literal_is_refused_at_its_own_line() {
    assert_rejected_with(
        "tests/fixtures/int_literal_out_of_range.kp",
        "'4294967295' does not fit 'i32', the type a literal takes when nothing determines one",
    );
    assert_rejected_with(
        "tests/fixtures/int_literal_method_receiver.kp",
        "'4294967295' does not fit 'i32', the type a literal takes when nothing determines one",
    );
    assert_rejected_with(
        "tests/fixtures/int_literal_const_out_of_range.kp",
        "'4294967295' does not fit 'i32': write a type that holds it",
    );
    assert_compiles_and_runs(
        "tests/fixtures/int_literal_typed.kp",
        "4294967295\n4294967295\n-2147483648\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/int_literal_width.kp",
        "-2147483648\n-147483634\n-2147483647\n2147483648\n147483634\n147483633\n-2147483648\n255\n65535\n999993014\n",
    );
}

/// A literal may spell the whole `u64` range where the context asks for a `u64`
/// (MISSING-x8Wq3). The scanner used to stop at `i64::MAX`, so the top half of
/// `u64` could not be written down and a mask or a maximum sentinel was spelled
/// as arithmetic. The range comes from the type the literal lands on, which is
/// the rule the lexical page already states, so the language needs no suffix;
/// and a value wider than the type it lands on is still refused at its own line,
/// naming that type (BUG-t6Mz2).
#[test]
pub(crate) fn a_u64_literal_takes_the_range_of_its_context() {
    // The item's repro: the hex mask and the decimal spelling of the same value,
    // both a `u64` from the annotation on the binding, plus the same two
    // spellings of 2^63, the first value `i64` cannot hold.
    assert_compiles_and_runs(
        "tests/fixtures/u64_literal_range.kp",
        "18446744073709551615\n18446744073709551615\n9223372036854775808\n9223372036854775808\n9223372036854775807\n",
    );
    // The same rule on every other path that gives a literal a type: a `const`'s
    // declared type, a call argument against the parameter's type, a `fun`'s
    // return type, and the `i64` minimum spelled as its magnitude under a sign.
    assert_compiles_and_runs(
        "tests/fixtures/u64_literal_contexts.kp",
        "18446744073709551615\n18446744073709551615\n18446744073709551615\n-9223372036854775808\n",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_beyond_max.kp",
        "'18446744073709551616' is larger than the largest integer literal, 18446744073709551615",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_does_not_narrow.kp",
        "'18446744073709551615' does not fit 'i32': write a type that holds it",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_needs_a_context.kp",
        "'18446744073709551615' does not fit 'i32', the type a literal takes when nothing determines one",
    );
}

#[test]
pub(crate) fn integer_conversions_widen_exactly_and_narrow_with_a_check() {
    assert_compiles_and_runs(
        "tests/fixtures/int_conversions.kp",
        "65\n65\n65\n65\n65\n300\n300\n251\n-7\n-7\n9\n9\n9\n4294967295\n5\n6\n",
    );
    // A narrowing that cannot represent the value panics instead of
    // truncating, which is why `-1 as u8` is a runtime failure and not 255.
    assert_fixture_panics("int_conversion_overflow", "i64.to_u8: value out of range");
}

#[test]
pub(crate) fn a_fallible_conversion_returns_an_option_instead_of_panicking() {
    // MISSING-RbhdW: `try_to_<T>` is the twin of `to_<T>` for every pair whose
    // plain conversion can lose a value. Both bounds of both sign directions
    // are exercised, at the boundary and one past it, plus the receiver being
    // read once (the count `noisy()` prints) and the result used through
    // `if let`, `when` and `?`.
    assert_compiles_and_runs(
        "tests/fixtures/try_to_conversion.kp",
        "255\ntrue\ntrue\n7\n-2147483648\ntrue\ntrue\ntrue\n7\ntrue\ntrue\n255\ntrue\ntrue\n7\n5\n5\n2\n255\nnone\n9\ntrue\n",
    );
    // There is no `try_to` where the plain conversion cannot fail: a widening
    // is already exact, and the diagnostic says which method to write instead.
    assert_rejected_with(
        "tests/fixtures/try_to_widening.kp",
        "a try_to_<T> twin of every conversion that can lose a value",
    );
    // The `Option` is the stdlib's own, so a program with no `std/option`
    // anywhere in its graph is told to import it rather than getting an
    // internal error about an unknown type.
    assert_rejected_with(
        "tests/fixtures/try_to_without_option.kp",
        "add `impl \"std/option\"` to the file that writes the call",
    );
}

/// The remainder of a division at every integer width. This is the fixture that
/// was missing when a narrow remainder was moved into a sixty-four-bit
/// destination: `%` on `i32`/`u8`/`u16` came out wrong at a width no fixture had
/// reached, because the only fixtures dividing a narrow value were refused for
/// other reasons first.
#[test]
pub(crate) fn remainder_works_at_every_integer_width() {
    assert_compiles_and_runs(
        "tests/fixtures/int_mod_narrow.kp",
        "1\n-1\n1\n-1\n6\n3\n4\n15\n2\n535\n999993014\n-999993014\n",
    );
}

#[test]
pub(crate) fn narrow_integer_arithmetic_wraps_at_its_own_width() {
    assert_compiles_and_runs(
        "tests/fixtures/int_narrow_width_arithmetic.kp",
        "44\n100\n156\n32\n2\n0\n64\n236\n172\n32\n50\n55\n255\n44464\n24064\n5535\n10000\n88\n244\nsmaller\n44\n32\n4\n4\n",
    );
}

#[test]
pub(crate) fn a_failed_syscall_reports_errno_and_strerror() {
    // `ENOENT` is 2 on Linux, and glibc and musl both spell it this way; the
    // message line is asserted separately so a stable prefix is what a
    // golden test would pin, and the errno line is the new detail.
    assert_fixture_panics_all(
        "fs_errno",
        &[
            "fs.read: cannot open file",
            "errno 2: No such file or directory",
        ],
    );
}

#[test]
pub(crate) fn fs_has_fallible_read_and_open() {
    // MISSING-d6Qy1: a failure is an `io.IoErr` value (errno plus text) rather
    // than a panic, and the invalid-UTF-8 case carries errno 0 because no
    // syscall failed. `try_open`'s mode is not a path argument: the `"r"`
    // below would be denied by the capability check if it were.
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&fixture("tests/fixtures/fs_fallible.kp"), "fs_fallible_src");
    assert_project_runs(
        "fs_fallible",
        &target,
        "hello\n\n2\nNo such file or directory\nhello\n\n0\nfs.try_read_text: input is not valid utf-8\ntrue\n22\n2\n",
    );
}

#[test]
pub(crate) fn fs_mutators_have_fallible_twins() {
    // MISSING-k7Rwq: `None` is success, a failure is an `io.IoErr`, and the run
    // goes on after each one (ENOENT 2, EEXIST 17, ENOTDIR 20).
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&fixture("tests/fixtures/fs_try_mutators.kp"), "fs_try_mutators_src");
    assert_project_runs(
        "fs_try_mutators",
        &target,
        "mkdir_all\n0\nwrite_text\n0\nappend\n0\nwrite\n0\nrename\n0\nsymlink\n0\nremove\n0\nwrite_text_missing_dir\n2\nwrite_missing_dir\n2\nappend_missing_dir\n2\nremove_missing\n2\nrename_missing\n2\nsymlink_exists\n17\nmkdir_all_under_file\n20\n3\n2\nremove_all\n0\nfalse\nremove_all_missing\n2\ndone\n",
    );
}

#[test]
pub(crate) fn a_long_constant_array_literal_is_a_global_plus_a_copy() {
    // MISSING-Nf3Rb: 300 elements is past the threshold. The binding gets its own
    // copy (the mutation of `copy` does not reach `table`), a literal with a
    // non-constant element keeps the aggregate form, and a literal passed
    // straight to a call still works.
    assert_compiles_and_runs(
        "tests/fixtures/array_literal_big.kp",
        "36344\n99\n0\n36443\n50100\n36344\n",
    );
}

#[test]
pub(crate) fn a_pool_closes_connections_that_idled_too_long() {
    // Open item 56: `close_idle(after_ms)` and `set_idle_timeout(ms)`.
    assert_compiles_and_runs(
        "tests/fixtures/http_pool_idle.kp",
        "warm 4\nclosed fresh 0\nkept 4\nclosed all 4\nleft 0\nfalse\nafter stale checkout 0\ndone\n",
    );
}

#[test]
pub(crate) fn http_builds_a_multipart_form_body() {
    // MISSING-j2Wka: the exact wire bytes, with CR shown as `\r`.
    assert_compiles_and_runs("tests/fixtures/http_multipart.kp", "multipart/form-data; boundary=xXboundaryXx\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"content\"\\r\n\\r\nhello\\r\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"a%22b%0D%0AX: y\"\\r\n\\r\nv\\r\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"file\"; filename=\"n%22ote.txt\"\\r\nContent-Type: text/plain\\r\n\\r\nline1\nline2\\r\n--xXboundaryXx--\\r\n\nmultipart/form-data; boundary=xXboundaryXx\nPOST\n");
}

#[test]
pub(crate) fn owned_datac_fields_are_dropped_with_the_struct() {
    assert_compiles_and_runs(
        "tests/fixtures/string_field.kp",
        "first\n7\nafter scope\nsecond\n7\ndone\n",
    );
}

/// The same mechanism with all three owned heap shapes in one struct: a
/// `String`, a `Bytes`, and a nested `datac` that itself owns a `String`, with
/// the `i32` scalar deliberately *not* last so a heap field follows a non-heap
/// one. This is the shape that pins the generated per-`datac` drop on order:
/// dropping `Row` must recurse through `Label` into its `String`, and reading
/// the fields through a borrowed `Row` must not steal them.
///
/// The `View` here is a local borrow (`moved.name.view()`), not a field:
/// `is_borrowed_field_ty` (`kelpc/src/types.rs`) refuses a `View`/`&str`/`Ref`
/// datac field outright, which `view_field.kp` and
/// `a_view_cannot_be_stored_in_a_datac` pin, so a borrow that owns nothing is
/// expressible only outside the struct. It is printed and then dropped without
/// being freed, which is the one way a heap type in this fixture must *not*
/// reach a free.
#[test]
pub(crate) fn datac_owned_fields_recurse_and_drop_once() {
    assert_compiles_and_runs(
        "tests/fixtures/datac_owned_fields.kp",
        "alpha\n7\nraw\nnested\n7\nafter scope\nbeta\n9\nmore\ndeep\nbeta\n4\ndeep\ndone\n",
    );
}

#[test]
pub(crate) fn an_owned_passdown_is_freed_at_exit() {
    assert_compiles_and_runs("tests/fixtures/pd_owned.kp", "== x ==\n== x ==\n");
}

#[test]
pub(crate) fn an_owned_passdown_no_expression_reads_is_still_dropped() {
    // An owned `pd` is dropped by the generated `atexit` handler, so the
    // container function set its drop calls has to exist even when nothing
    // reads the value. The differential fuzzer found the `llvm` backend
    // reporting an internal error for exactly this shape.
    assert_compiles_and_runs("tests/fixtures/pd_owned_container.kp", "ok\n");
}

#[test]
pub(crate) fn a_compiler_shipped_stdlib_module_can_be_imported() {
    // `impl "std/mem"`, the module's Kelp body calls a compiler-supplied
    // `extern` for the allocation itself.
    assert_compiles_and_runs("tests/fixtures/stdlib_mem.kp", "3\nhi\n0\n");
}

#[test]
pub(crate) fn the_str_stdlib_module_runs_from_kelp_source() {
    // `std/str` is written in Kelp; its only compiler-supplied pieces are the
    // two byte reads it wraps.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_str.kp",
        "HI THERE\n8\n8\ntrue\nfalse\nhello\n98\nababab\n6\n",
    );
}

#[test]
pub(crate) fn the_str_case_mapping_is_unicode_simple_and_the_slice_is_codepoint_aware() {
    // `to_upper`/`to_lower` apply the simple Unicode case mappings (table
    // generated by kelpc/tools/gen_case_table.pl). `str.slice` takes byte
    // offsets that must sit on codepoint boundaries.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_str_case.kp",
        "CAFÉ NAÏVE\n\
         école ñandú\n\
         STRAßE\n\
         ΑΒΓ ΩΣΣ\n\
         αβγ ωσ\n\
         ПРИВЕТ ЁЖ\n\
         привет ёж\n\
         istanbul\n\
         ⱥ\n\
         3\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_str_slice.kp",
        "11\na\né\n€\n😀\n!\n0\naé€😀!\n",
    );
    assert_fixture_panics(
        "str_slice_split_codepoint",
        "str.slice: byte 1 is inside a codepoint",
    );
}

#[test]
pub(crate) fn the_path_stdlib_module_runs_from_kelp_source() {
    // `std/path` imports `std/str`, so this also exercises a stdlib module
    // importing another one.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_path.kp",
        "/tmp/x.txt\n/tmp/x.txt\n/abs\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn the_encoding_stdlib_module_runs_from_kelp_source() {
    // MISSING-FZqe6: the alphabet-and-codec module. `std/encoding` imports
    // `std/str`, `std/option`, `std/result` and `std/fmt`, so this fixture also
    // exercises the owned payloads those modules hand back (`Result<Bytes, _>`
    // and `Result<String, _>`) without a `?` or a `Copy` payload to lean on.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_encoding.kp",
        "aGVsbG8=\naGk=\nYQ==\n\nhello\nhi\na\ntrue\ntrue\nNBSWY3DP\nNBUQ====\nhello\ntrue\ntrue\nCn8eVZg\n2g\nhello\ntrue\n4869\nHi\nHello\ntrue\ntrue\na%20b%2Fc%3Fd%3De%26f\nplain-._~\na b/c\ntrue\ntrue\na+b%2Bc\na b+c\ntrue\n00000000  48 69                                             |Hi|\n\nda39a3ee5e6b4b0d3255bfef95601890afd80709\na9993e364706816aba3e25717850c26c9cd0d89d\n84983e441c3bd26ebaae4aa1f95129e5e54670f1\n",
    );
}

// `std/regex`: a linear-time regular expression engine in pure Kelp. Each
// fixture prints its own verdict, so a failure names the case; the table and the
// differential check both end in a count that has to be zero.
#[test]
pub(crate) fn the_regex_conformance_table_passes() {
    assert_compiles_and_runs("tests/fixtures/stdlib_regex_table.kp", "cases: 183, failed: 0\n");
}

#[test]
pub(crate) fn the_regex_refuses_bad_patterns_with_their_message_and_offset() {
    assert_compiles_and_runs("tests/fixtures/stdlib_regex_errors.kp", "cases: 45, failed: 0\n");
}

#[test]
pub(crate) fn the_regex_api_finds_captures_replaces_and_splits() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_api.kp",
        include_str!("../fixtures/stdlib_regex_api.expected"),
    );
}

#[test]
pub(crate) fn the_regex_is_linear_on_patterns_that_explode_a_backtracker() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_linear.kp",
        include_str!("../fixtures/stdlib_regex_linear.expected"),
    );
}

#[test]
pub(crate) fn the_regex_agrees_with_a_slow_backtracking_reference() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_differential.kp",
        "differing cases: 0\n",
    );
}

#[test]
pub(crate) fn regex_is_match_panics_on_a_bad_pattern() {
    assert_fixture_panics("stdlib_regex_panic", "regex.is_match: unclosed group");
}

// A body that ends in `while true { ... }` never falls through, so a function
// returning an owned type needs no fallthrough `ret`, which it could not build
// one for anyway, a `String` having no zero value. Both backends agree here now;
// before, the LLVM one refused the shape as a compiler defect.
#[test]
pub(crate) fn a_function_whose_body_ends_in_an_endless_loop_returns_an_owned_value() {
    assert_compiles_and_runs(
        "tests/fixtures/owned_return_endless_loop.kp",
        "xxx\n\n4\n9\n",
    );
}

// `std/tz`: time zones over the system TZif database. The synthetic fixture
// builds its own TZif byte arrays, so it pins the parser, the footer rule and
// the corrupt-data refusals on a machine with no database at all; the zone
// fixture needs one and prints a single skip line when there is none, which the
// test expects instead of the table (the same probe, `zoneinfo_has_the_test_zones`).
#[test]
pub(crate) fn the_tz_conformance_table_passes() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_tz_synthetic.kp",
        "gap: gap\nambiguous: ambiguous 1730611800 1730615400\none: one 1710052200\ncases: 116, failed: 0\n",
    );
}

#[test]
pub(crate) fn the_tz_module_reads_the_system_zoneinfo_database() {
    let expected = if zoneinfo_has_the_test_zones() {
        "cases: 57, failed: 0\n"
    } else {
        "tz: no zoneinfo database\n"
    };
    assert_compiles_and_runs("tests/fixtures/stdlib_tz_zones.kp", expected);
}

/// Whether the host carries every zone `stdlib_tz_zones.kp` reads, in `$TZDIR`
/// or in `/usr/share/zoneinfo`, the same probe the fixture makes, so the two
/// agree about whether the database is there.
pub(crate) fn zoneinfo_has_the_test_zones() -> bool {
    let tzdir = std::env::var("TZDIR").unwrap_or_default();
    let dirs = [tzdir, "/usr/share/zoneinfo".to_string()];
    [
        "America/New_York",
        "Europe/Berlin",
        "Asia/Kolkata",
        "Australia/Lord_Howe",
    ]
    .iter()
    .all(|zone| {
        dirs.iter()
            .any(|dir| !dir.is_empty() && Path::new(dir).join(zone).is_file())
    })
}

#[test]
pub(crate) fn an_extern_without_a_compiler_supplied_body_is_rejected() {
    assert_rejected("tests/fixtures/extern_no_body.kp");
}

#[test]
pub(crate) fn an_extern_receiver_declaration_is_rejected() {
    assert_rejected("tests/fixtures/extern_receiver.kp");
}

#[test]
pub(crate) fn comparing_owned_values_is_rejected() {
    // Without this rule the comparison reached `cc` as `kelp_a == kelp_b` on two
    // `kelp_String` structs, which is where the author would have learned what
    // happened instead of at the type checker.
    assert_rejected("tests/fixtures/compare_owned.kp");
}

#[test]
pub(crate) fn an_owned_value_in_statement_position_is_rejected() {
    assert_rejected("tests/fixtures/owned_statement.kp");
}

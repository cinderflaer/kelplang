use super::*;

// ---------------------------------------------------------------------------
// M6.3: `const`, stdlib-only argument-type overloading, and `&str` return
// elision (which is what the rest of the `str`/`path` surface needed).
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn const_declarations_fold_at_compile_time() {
    assert_compiles_and_runs(
        "tests/fixtures/consts.kp",
        "42\n1.500000\n84\nx\ntrue\n",
    );
}

#[test]
pub(crate) fn std_math_overloads_resolve_on_argument_type() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_math.kp",
        "3.141593\n6.283185\n7\n7.500000\n3\n9\n10\n1.414214\n2.000000\n-1.000000\ntrue\nfalse\n",
    );
}

/// MK7 S3A: the `extern fun` bodies in one program, one per C signature shape
/// they have (a scalar return, a 24-byte `Bytes`/`String` returned through the
/// hidden pointer, a `&str` return, a `*i64` out-parameter, and a body that
/// calls into `std/rt`). This suite is backend-agnostic, so it validates the C
/// backend's copy of the same bodies and the LLVM backend's declared calls to
/// them against one literal output.
#[test]
pub(crate) fn extern_bodies_lower_with_their_c_signature() {
    assert_compiles_and_runs(
        "tests/fixtures/extern_signatures.kp",
        "1.414214\n1024.000000\n2.000000\nfalse\nhi\nff\n3.14\nello\n42\ntrue\n/a/b/c\n/a/c\n",
    );
}

#[test]
pub(crate) fn str_views_borrow_and_owned_truncations_allocate() {
    assert_compiles_and_runs(
        "tests/fixtures/string_views.kp",
        "hello\n5\nhello  \n7\nello\ntrue\ntrue\ntrue\ntrue\nfalse\n2\ntrue\nraw\n5\nown\n",
    );
}

#[test]
pub(crate) fn path_components_split_a_path() {
    assert_compiles_and_runs(
        "tests/fixtures/path_parts.kp",
        "notes.txt\ntxt\n/tmp\nbare.txt\ntxt\n\n/var/log\ntrue\n",
    );
}

#[test]
pub(crate) fn a_str_becomes_a_string_where_one_is_expected() {
    assert_compiles_and_runs(
        "tests/fixtures/str_to_string_coercion.kp",
        "a literal\nassigned\nt\nb\npushed\nv\nthen\nsome\na title\nhey!\n7\ntrue\n1\n",
    );
}

/// M19 gate: the counted `while` over a list index and the `for` that replaces it
/// print the same lines, so the rewrite of one into the other keeps the output.
#[test]
pub(crate) fn a_for_over_a_list_prints_what_the_index_walk_prints() {
    assert_compiles_and_runs(
        "tests/fixtures/for_list_index_walk_equiv.kp",
        "3\n4\n3\n4\nann\nbo\ncy\nann\nbo\ncy\n6\n4\n7\n4\n8\n6\n4\n7\n4\n8\n5\n",
    );
}

/// M24 gate: a `List<T>` and a `&List<T>` parameter, and a `str` and a `&str`
/// parameter, give the same results, so a `&` on a borrowed parameter can go.
#[test]
pub(crate) fn a_borrow_parameter_spelled_with_or_without_ampersand_gives_the_same_result() {
    assert_compiles_and_runs(
        "tests/fixtures/param_borrow_spellings.kp",
        "2\n2\n2\n4\n4\n4\n4\n",
    );
}

/// `str.less` is the ordering `str` never had, and `str.is_digit` is the digit
/// predicate that four libraries in the catalogue wrote by hand.
///
/// `str` refuses `==`, `<` and the rest (they would compare pointers, see
/// `str_comparison.kp` next door), and `std/str` offered no comparison instead, so
/// `fpl/json` carries a private `str_less` for its key sorting and `fpl/json`,
/// `fpl/decimal`, `fpl/encoding` and `fpl/httpx` each have their own digit test.
/// This fixture is the two functions used the way a caller uses them, a number
/// parser that stops at the first non-digit, and an insertion sort, plus the two
/// rules their doc comments commit to, because a codepoint comparison instead of a
/// byte comparison would pass a predicate test and silently change a sort.
#[test]
pub(crate) fn a_str_is_ordered_by_byte_and_a_digit_is_decimal() {
    assert_compiles_and_runs(
        "tests/fixtures/std_str_order.kp",
        "true\nfalse\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\nfalse\nfalse\nfalse\n12\n2\napple\nfig\npear\n",
    );
}

#[test]
pub(crate) fn a_view_carries_its_length_and_slices_without_allocating() {
    assert_compiles_and_runs(
        "tests/fixtures/view.kp",
        "the quick brown fox\n19\nfalse\ntrue\ntrue\nquick\nthe\nhe quick brown fox\ntrue\ntrue\nfalse\ntrue\nthe\nhe quick brown fox\n3\ntrue\n0\ntrue\nquick\nthe\nowned text\ntext\nquick brown\n11\n",
    );
}

#[test]
pub(crate) fn a_view_needs_an_earlier_owner_in_its_datac() {
    assert_rejected_with(
        "tests/fixtures/view_field.kp",
        "construct it from a string literal or from an earlier String/Bytes field",
    );
}

#[test]
pub(crate) fn a_view_cannot_be_a_list_element() {
    assert_rejected("tests/fixtures/view_element.kp");
}

#[test]
pub(crate) fn a_returned_view_must_come_from_a_reference_parameter() {
    assert_rejected("tests/fixtures/view_returned_from_local.kp");
}

#[test]
pub(crate) fn a_view_of_a_temporary_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/view_of_temporary.kp", "x\n");
}

#[test]
pub(crate) fn a_literal_takes_its_sibling_operands_type_on_either_side() {
    assert_compiles_and_runs(
        "tests/fixtures/literal_operand_typing.kp",
        "true\n4\n4\n3000000005\n3000000005\n-2\n-2\nfalse\n240\n240\npositive\n55\n55\n241\n",
    );
}

#[test]
pub(crate) fn a_const_is_not_a_literal_for_operand_typing() {
    assert_rejected("tests/fixtures/literal_const_is_not_a_literal.kp");
}

#[test]
pub(crate) fn bitwise_operators_shift_mask_and_complement() {
    assert_compiles_and_runs(
        "tests/fixtures/bitwise.kp",
        "8\n14\n6\n243\n16\n64\n32\nfalse\ntrue\n228\n184\n173\ntrue\n1\n-1\n-4\n",
    );
}

#[test]
pub(crate) fn bitwise_operators_on_bool_are_rejected() {
    assert_rejected("tests/fixtures/bitwise_bool.kp");
}

/// `0x` and `0b` literals (A-9). They are the same values as their decimal
/// spellings, so the assertion is the masks and the encoder constants a
/// byte-level program writes, not the spellings themselves.
#[test]
pub(crate) fn hexadecimal_and_binary_literals_are_ordinary_integers() {
    assert_compiles_and_runs(
        "tests/fixtures/int_literals.kp",
        "255\n10\n511\ntrue\n196\n184\n173\n1\n0\n255\n4294967295\n2147483647\n4294967295\ntrue\n",
    );
}

/// A malformed radix literal is named at its own span rather than split into
/// two tokens: `0x` needs a digit, and `0b12` has a digit binary does not
/// have.
#[test]
pub(crate) fn a_malformed_radix_literal_is_rejected_by_name() {
    assert_rejected_with(
        "tests/fixtures/hex_literal_without_digits.kp",
        "needs at least one hex digit",
    );
    assert_rejected_with(
        "tests/fixtures/binary_literal_with_a_decimal_digit.kp",
        "invalid digit '2' in a binary literal",
    );
}

#[test]
pub(crate) fn float_arithmetic_comparisons_and_unordered_nan() {
    assert_compiles_and_runs(
        "tests/fixtures/asm_float_scalars.kp",
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\nfalse\nfalse\nfalse\n",
    );
}

#[test]
pub(crate) fn a_view_keeps_its_provenance_through_a_pattern_binding() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_provenance_and_arms.kp",
        "hello\ntrue\nfalse\nfalse\nhello\nnone\n42\ntrue\nfalse\n",
    );
}

/// A function that returns a borrow other than `&str`: a `datac`, a `String`, a
/// `Bytes`, and a returned `if` whose arms both derive from the reference
/// parameter. Each is a shape a return-class classifier can get wrong, so the
/// stdout of all four is pinned.
#[test]
pub(crate) fn a_function_may_return_a_borrow_of_any_type() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_returned.kp",
        "3,4\n3\n4\nhello\n5\n3\n",
    );
}

#[test]
pub(crate) fn character_literals_take_the_type_their_context_asks_for() {
    assert_compiles_and_runs(
        "tests/fixtures/char_literals.kp",
        "34\n48\n53\n10\n9\n92\n39\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\nfalse\n122\n",
    );
}

/// Byte literals (H-11). The assertion is the byte values and the escapes,
/// including the `\xNN` pair that agrees across a byte and a character
/// literal.
#[test]
pub(crate) fn byte_literals_are_u8_and_carry_hex_escapes() {
    assert_compiles_and_runs(
        "tests/fixtures/byte_literals.kp",
        "10\n13\n9\n34\n92\n39\n0\n32\n255\ntrue\ntrue\ntrue\nfalse\n4\nAbc\ntrue\n233\ntrue\n48\n49\n50\n",
    );
}

/// `isize` is `ptrdiff_t`, and a backend that emitted that name without including `<stddef.h>` failed every shape of the type to compile. One
/// shape per way a type can appear, which is why this is a fixture and not a
/// one-line repro: a local, a parameter, a return, a `datac` field and a `List`
/// element.
#[test]
pub(crate) fn isize_works_in_every_position_a_type_can_appear_in() {
    assert_compiles_and_runs(
        "tests/fixtures/isize_kinds.kp",
        "6\n-4\n-3\n7\n-1\n1\n-7\ntrue\ntrue\ntrue\ntrue\n",
    );
}

/// A byte literal is one ASCII character or a `\xNN` escape, it is a `u8` and
/// not the type its context asks for, and the escape is exactly two hex
/// digits in both literal forms.
#[test]
pub(crate) fn a_byte_literal_is_one_byte_and_a_u8() {
    assert_rejected_with(
        "tests/fixtures/byte_literal_holds_two_bytes.kp",
        "a byte literal holds exactly one byte",
    );
    assert_rejected_with(
        "tests/fixtures/byte_literal_is_not_ascii.kp",
        "a byte literal is one ASCII character or a '\\xNN' escape",
    );
    assert_rejected_with(
        "tests/fixtures/byte_literal_does_not_widen.kp",
        "a byte literal is a 'u8', not a 'i64'",
    );
    assert_rejected_with(
        "tests/fixtures/hex_escape_needs_two_digits.kp",
        "a hex escape is written '\\xNN', with two hex digits",
    );
    assert_rejected_with(
        "tests/fixtures/string_hex_escape_is_not_text.kp",
        "not a text character",
    );
}

#[test]
pub(crate) fn a_character_literal_too_large_for_its_type_is_rejected() {
    assert_rejected("tests/fixtures/char_literal_does_not_fit.kp");
}

#[test]
pub(crate) fn integer_to_float_conversions_widen_and_round() {
    assert_compiles_and_runs(
        "tests/fixtures/float_conversions.kp",
        "16777217.000000\n16777216.000000\n-7.000000\n-3.500000\n200.000000\n200.000000\n3000000000.000000\n0.500000\n",
    );
}

#[test]
pub(crate) fn printing_a_string_or_a_float_uses_the_right_register_file() {
    // `%s` reads a pointer out of the *integer* register file and `%f` reads a
    // value out of `xmm`, a variadic call says how many vector registers it used
    // in `al`, and the two files are numbered independently, so a template
    // interpolating one of each is where getting that wrong is silent rather
    // than a crash. The u64 cases are the other half: bit 63 set is the one
    // integer-to-float conversion a plain conversion alone gets wrong.
    assert_compiles_and_runs(
        "tests/fixtures/asm_float_print.kp",
        "2.500000\n0.250000\n0.125000\nn=7 half=2.500000 ok=false s=kelp\n2.500000 0.250000\n\
         18446744073709551616.000000\n9223372036854775808.000000\n9223372036854775808.000000\n\
         kelp\nw=kelp\n",
    );
}

#[test]
pub(crate) fn an_integral_float_literal_stays_a_float() {
    // A wide literal used to make `cc` reject the file (it was emitted as an
    // integer constant), and `-0.0` used to reach C as the *integer* zero, so
    // the sign was gone and an expression of two such literals was evaluated
    // in integer arithmetic.
    assert_compiles_and_runs(
        "tests/fixtures/float_integral_literal.kp",
        "100000000000000000000\n2.0\nnegative zero\nnegative product\n4.500000\n",
    );
}

#[test]
pub(crate) fn a_non_finite_float_literal_emits_a_math_h_constant() {
    // An overflowing literal is infinity and a folded `0.0 / 0.0` is a NaN;
    // neither has a literal spelling, and emitting `Display`'s `inf`/`NaN`
    // does not compile.
    assert_compiles_and_runs(
        "tests/fixtures/float_nonfinite_literal.kp",
        "infinite\nnan\n",
    );
}

#[test]
pub(crate) fn returning_a_borrow_of_the_wrong_parameter_is_rejected() {
    assert_rejected("tests/fixtures/elision_wrong_param.kp");
}

#[test]
pub(crate) fn returning_a_view_of_a_local_is_rejected() {
    assert_rejected("tests/fixtures/elision_not_from_param.kp");
}

#[test]
pub(crate) fn a_live_view_keeps_its_owner_pinned() {
    assert_rejected("tests/fixtures/view_pins_owner.kp");
}

#[test]
pub(crate) fn a_view_releases_its_owner_at_scope_end() {
    assert_compiles_and_runs(
        "tests/fixtures/view_released_at_scope_end.kp",
        "hi\nhi\n",
    );
}

#[test]
pub(crate) fn comparing_str_values_is_rejected() {
    assert_rejected("tests/fixtures/str_comparison.kp");
}

#[test]
pub(crate) fn a_const_that_does_not_fold_is_rejected() {
    assert_rejected("tests/fixtures/const_not_constant.kp");
}

// ---------------------------------------------------------------------------
// A-18: a fixed-size, stack-allocated array; A-19: compile-time evaluation
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_fixed_size_array_is_a_value_and_its_length_is_in_its_type() {
    // `.len()` is the length the type already carries, `.get`/`.set` are
    // bounds-checked against it, a nested array is held inline, and `for`
    // walks one because its desugar is exactly `.len()` plus `.get(i)`.
    assert_compiles_and_runs(
        "tests/fixtures/array_basic.kp",
        "3\n20\nfalse\n7\n6\n2\n2\n2\n",
    );
}

#[test]
pub(crate) fn an_array_element_may_own_memory_a_datac_or_another_array() {
    // `[Gate; 4]` as a `datac` field is the IDT/GDT shape, `[String; 3]`
    // makes the array own its elements (so `set` frees the one it replaces,
    // and a `datac` holding one owns it transitively), a `mut` parameter writes
    // through the borrow while a `move` one takes the value over, an
    // owned-element array is borrowed by a plain parameter (so the caller's
    // array survives the call), and `Box<[i32; 3]>` is a fixed-size buffer kept
    // past its frame.
    assert_compiles_and_runs(
        "tests/fixtures/array_owned.kp",
        "9 2\n6\nbee\n3\n2\n3\n9\n2\n9\n",
    );
}

/// A-18 with the two element kinds the scalar array fixtures do not reach: a
/// float element (`[f64; 2]`, whose element classifier has to answer `SSE`
/// rather than the heap routines' integer default) and an array wide enough to
/// be `MEMORY` class in both directions (`[u8; 300]` as an argument and as a
/// return), plus `[[i32; 5]; 2]`, whose twenty-byte *element* is `MEMORY` class
/// itself and therefore comes back through the hidden pointer, and an
/// owned-element array *literal* handed to a borrow parameter, which is the one
/// drop site an array temporary reaches.
#[test]
pub(crate) fn a_float_element_and_a_memory_class_array_are_values_too() {
    assert_compiles_and_runs(
        "tests/fixtures/array_wide_and_float.kp",
        "1.500000\n2.250000\n3.750000\n4.500000\n9\n9\n7\n10\n4\n3\n",
    );
}

#[test]
pub(crate) fn an_out_of_bounds_array_read_panics() {
    assert_fixture_panics("array_bounds", "Array.get: index out of bounds");
}

#[test]
pub(crate) fn an_out_of_bounds_array_write_panics_before_the_store() {
    assert_fixture_panics("array_bounds_set", "Array.get: index out of bounds");
}

#[test]
pub(crate) fn an_array_length_that_is_not_a_constant_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/array_length_is_a_constant.kp",
        "is not a constant here",
    );
}

#[test]
pub(crate) fn an_array_literal_of_the_wrong_length_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/array_literal_length_mismatch.kp",
        "element(s) is expected here",
    );
}

#[test]
pub(crate) fn a_repeat_literal_over_an_owning_element_is_rejected() {
    // `[x; N]` copies `x` into every slot, so an element that owns memory
    // would end up with N owners of one buffer.
    assert_rejected_with(
        "tests/fixtures/array_repeat_owns_memory.kp",
        "needs an element that owns nothing",
    );
    assert_rejected_with(
        "tests/fixtures/array_repeat_owned_datac.kp",
        "needs an element that owns nothing",
    );
}

#[test]
pub(crate) fn a_borrow_cannot_be_an_array_element() {
    assert_rejected_with(
        "tests/fixtures/array_of_a_borrow.kp",
        "cannot be a container element",
    );
}

#[test]
pub(crate) fn a_container_cannot_be_an_array_element() {
    // A-10 slice 1 widened the `List` element set, not the array one: an array
    // is fixed-size, so a `List`/`Box`/`Map` element stays refused there.
    assert_rejected_with(
        "tests/fixtures/array_container_element.kp",
        "'List<i32>' cannot be an array element",
    );
}

#[test]
pub(crate) fn a_const_may_call_a_function_and_the_call_is_evaluated_at_compile_time() {
    // A-19: parameters are bound, a `while` loop runs and a recursive call
    // terminates, all inside the compiler. The result is a literal at every use
    // site, so no backend sees the call.
    assert_compiles_and_runs(
        "tests/fixtures/const_fn.kp",
        "24\n120\n128\n144\n",
    );
}

// ---------------------------------------------------------------------------
// A-20: const type parameters, and `impl Trait` in argument position. Every
// fixture here runs on the default (LLVM) backend.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_const_parameter_is_a_value_a_length_and_a_repeat_count() {
    // `Buf<const N: usize>` infers `N` from its construction, `sum<const N>`
    // infers `N` from an array argument's own length, `N` is read as a value
    // (`i < N`), used as an array length (`[i32; N]`) and used as a repeat
    // count (`[0; N]`), and a `bool` const parameter is read too.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_basic.kp",
        "3\n3\n6\n100\n7\n3\n0\n15\n1\n5\n",
    );
}

#[test]
pub(crate) fn two_const_instantiations_of_one_ring_buffer_keep_their_own_state() {
    // `Ring<4>` and `Ring<8>` are two monomorphized types, so the loop writes
    // two independent buffers and each keeps its own head and count.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_two_sizes.kp",
        "4\n8\n30\n44\n4\n8\n",
    );
}

#[test]
pub(crate) fn a_const_argument_may_be_a_compile_time_expression() {
    // `Buf<SIZE * 2>`, `Buf<WIDTH>` where `WIDTH` is a `const` computed by a
    // plain `fun`, and `capacity<twice(n = 4)>()`: the existing constant
    // evaluation is the whole of it.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_expr_arg.kp",
        "6\n6\n6\n8\n",
    );
}

#[test]
pub(crate) fn a_const_length_mismatch_names_both_lengths() {
    assert_rejected_with(
        "tests/fixtures/const_generics_mismatch.kp",
        "[i32; 3]",
    );
    assert_rejected_with(
        "tests/fixtures/const_generics_mismatch.kp",
        "[i32; 4]",
    );
}

#[test]
pub(crate) fn arithmetic_on_a_const_parameter_in_a_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/const_generics_arithmetic.kp",
        "arithmetic on one in a type",
    );
}

#[test]
pub(crate) fn a_char_const_parameter_is_a_value_a_type_argument_and_a_comparison() {
    // `glyph<'x'>` and `glyph<'y'>` are two instantiations; `C == '/'` compares
    // the parameter; `Tag<'x'>` and `Tag<'y'>` are two `datac` types.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_char.kp",
        "x\ny\ntrue\nfalse\nx\ny\n3\n",
    );
}

#[test]
pub(crate) fn arithmetic_on_a_char_const_parameter_in_a_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/const_generics_char_arithmetic.kp",
        "'C' is a const parameter, and arithmetic on one in a type",
    );
}

#[test]
pub(crate) fn a_const_parameter_on_a_trait_is_refused_for_a_char_too() {
    assert_rejected_with(
        "tests/fixtures/const_generics_trait_refused.kp",
        "a trait cannot take a const parameter ('const C')",
    );
}

#[test]
pub(crate) fn a_const_parameter_of_any_other_type_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/const_generics_string_refused.kp",
        "'const S' has type 'String': a const parameter's type is one of 'usize', 'u8', 'u16', 'u32', 'u64', 'i32', 'i64', 'bool' or 'char'",
    );
}

#[test]
pub(crate) fn impl_trait_arguments_are_fresh_bounded_type_parameters() {
    // `fun pair_area(a: impl Shape, b: &impl Shape)` is two parameters, so two
    // different types may be passed, and the trait's methods dispatch
    // statically inside the body.
    assert_compiles_and_runs(
        "tests/fixtures/impl_trait_args.kp",
        "19\nsquare\nrect\n",
    );
    // The shape A-12 recorded as "a later slice" now compiles and runs with no
    // output: `impl Trait` is a parameter type, not a refusal.
    assert_compiles_and_runs("tests/fixtures/traits_impl_trait_argument.kp", "");
}

#[test]
pub(crate) fn impl_trait_in_return_position_is_refused() {
    assert_rejected_with(
        "tests/fixtures/impl_trait_return_refused.kp",
        "not allowed in return position",
    );
}

#[test]
pub(crate) fn impl_trait_in_a_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/impl_trait_field_refused.kp",
        "only legal as a whole parameter type",
    );
}

/// The two instantiations of one const-generic buffer are two copies, read back out of the emitted IR. The freestanding `llvm-nm` test below finds both mangled names in the bare-metal image.
#[test]
pub(crate) fn a_const_generics_two_instantiations_emit_two_copies() {
    let bin = out_path("const_generics_two_copies");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/const_generics_two_sizes.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ring fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let generated = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the generated IR sits beside the binary");
    for name in [
        "kelp_Ring__push__l__4__g__",
        "kelp_Ring__push__l__8__g__",
        "kelp_Ring__total__l__4__g__",
        "kelp_Ring__total__l__8__g__",
    ] {
        assert!(
            generated.contains(name),
            "the generated C must hold a copy for each const value, and '{name}' is missing:\n{generated}"
        );
    }
}

/// A-20's freestanding half: a const-generic ring buffer in the kernel image
/// links with no libc symbol, and both `Ring<4>` and `Ring<8>` are emitted (the
/// same acceptance test as `freestanding_min`, run on the const-generic
/// project). It needs the `dev-kernel.nix` tools; a shell without them prints a
/// skip.
#[test]
pub(crate) fn a_freestanding_const_generic_buffer_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding const-generic test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_constgen");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_constgen"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding const-generic build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let symbols = tool("llvm-nm", &[path]);
    for name in [
        "kelp_Ring__push__l__4__g__",
        "kelp_Ring__push__l__8__g__",
    ] {
        assert!(
            symbols.contains(name),
            "the freestanding image must hold one copy per const value, and '{name}' is missing:\n{symbols}"
        );
    }
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

#[test]
pub(crate) fn a_modules_own_items_are_reachable_bare_inside_it() {
    // `util.kp` calls `doubled` and constructs `Pair` without qualifying
    // either, even though `resolve.rs` stores both prefixed.
    assert_project_runs(
        "self_call",
        &project("self_call").join("main.kp"),
        "12\n3\n",
    );
}

// ---------------------------------------------------------------------------
// Struct emission order. `structs` used to be collected from a `HashMap`, so
// a `datac` embedding another one compiled or failed at random. Declaration
// order is a valid topological order because a `datac` may only name datacs
// declared before it.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_datac_can_embed_an_earlier_datac() {
    assert_compiles_and_runs("tests/fixtures/datac_embeds_datac.kp", "8\n");
}

// MK7 §5 item 4, third slice: real SysV eightbyte/MEMORY classification for a
// `datac` over eight bytes.

#[test]
pub(crate) fn a_two_eightbyte_datac_passes_and_returns_in_a_register_pair() {
    assert_compiles_and_runs("tests/fixtures/datac_two_eightbytes.kp", "3\n4\n14\n11\n");
}

#[test]
pub(crate) fn a_memory_class_datac_returns_through_a_hidden_pointer() {
    assert_compiles_and_runs("tests/fixtures/datac_memory_class.kp", "1\n2\n3\n6\n33\n60\n");
}

// MK7 §5 item 4, fourth slice: a `MEMORY`-class `datac` passed and received by
// value, in the caller's outgoing argument area.

#[test]
pub(crate) fn a_memory_class_datac_passes_by_value_in_the_argument_area() {
    assert_compiles_and_runs(
        "tests/fixtures/datac_memory_class_by_value.kp",
        "6\n15\n24\n150\n579\n45\n11\n11\n11\n106\n1234\n13\n",
    );
}

#[test]
pub(crate) fn a_forward_reference_between_datacs_is_rejected() {
    assert_rejected("tests/fixtures/datac_forward_reference.kp");
}

// MK7 §5 item 4, the float-in-aggregate slice: a `datac` or `enum` payload
// holding an `f32`/`f64`, classified per eightbyte. The fixture's stdout covers
// the values; a consistently wrong register file is the failure a literal
// cannot see.

#[test]
pub(crate) fn a_float_field_datac_travels_in_the_vector_register_file() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_boundary.kp",
        "3.000000\n4.500000\n4.000000\n3.750000\n2.500000\n7.500000\n2.500000\n7.500000\n\
         7.000000\n6.000000\n10.000000\n7.000000\n2.500000\n",
    );
}

#[test]
pub(crate) fn a_float_field_datac_over_sixteen_bytes_is_memory_class() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_memory.kp",
        "6.500000\n5.000000\n2.500000\nkelp\n2.500000\n",
    );
}

#[test]
pub(crate) fn an_enum_payload_eightbyte_is_classified_through_the_union() {
    assert_compiles_and_runs(
        "tests/fixtures/float_enum_register_file.kp",
        "1.500000\n2.500000\n0.000000\n2.500000\n",
    );
}

#[test]
pub(crate) fn a_float_field_datac_crosses_the_hand_written_container_boundary() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_containers.kp",
        "1.500000\n2.500000\n2.500000\n3.500000\n4.500000\n4.500000\n3.500000\n5.500000\n",
    );
}

#[test]
pub(crate) fn a_float_field_is_written_through_the_xmm_file() {
    assert_compiles_and_runs(
        "tests/fixtures/float_field_write.kp",
        "3\n2.000000\n3\n4.000000\n9\n4.250000\n",
    );
}

/// A field read of an *owned* temporary, whose temporary has to be dropped
/// once per read.
///
/// Both backends stage the temporary for the field's address and free *it*
/// after the read, so the fixture builds, runs and compares on either one.
/// (A C backend once emitted `&(rvalue).field` and rejected the whole shape, which is why this test was written the way its fixture is.) This is the fixture the leak was found in: without a
/// test here under *either* backend, the loop below was committed
/// non-terminating and nothing said so. The only thing that noticed was
/// `memory_gate`'s 60 s timeout, which reads as a valgrind finding.
#[test]
pub(crate) fn a_field_of_an_owned_temporary_is_dropped_per_read() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_field_of_owned_temporary.kp",
        "aa\nbb\n2\naa\n4\n3\n2\n2\n",
    );
}

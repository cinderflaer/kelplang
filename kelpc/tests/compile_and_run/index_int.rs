//! Index and length functions take `int`, not `usize` (issue #278).

use super::collections::assert_fixture_panics;
use super::*;

/// A length or an index is an `int` (issue #278): `len` on List, Bytes, String
/// and the other containers returns `int`, `get` takes an `int`, and a value
/// bound with `int` needs no conversion. The output is unchanged.
#[test]
pub(crate) fn index_and_length_functions_take_and_return_int() {
    assert_compiles_and_runs("tests/fixtures/index_len_is_int.kp", "3\n20\n2\n3\ntrue\n");
}

/// A `usize` passed where an index is asked for is refused, and the message
/// names both `int` and `usize`, so the fix is named in the error.
#[test]
pub(crate) fn a_usize_index_is_refused_naming_both_types() {
    assert_rejected_naming("tests/fixtures/index_usize_refused.kp", &["'int'", "'usize'"], &[]);
}

/// A negative index is refused by the existing bounds check with today's message.
#[test]
pub(crate) fn a_negative_list_index_panics_with_the_bounds_message() {
    assert_fixture_panics("list_get_negative", "List.get: index out of bounds");
}

/// A negative byte index into a string panics with the `str.byte` message.
#[test]
pub(crate) fn a_negative_str_byte_index_panics_with_the_bounds_message() {
    assert_fixture_panics("str_byte_negative", "str.byte: index out of bounds");
}

/// A negative element count to `with_capacity` panics before it allocates.
#[test]
pub(crate) fn a_negative_capacity_panics_with_a_named_count() {
    assert_fixture_panics("with_capacity_negative", "List.with_capacity: negative count");
}

//! The everyday conversion names (issue #279): `to_byte` and `try_to_byte` on an
//! integer, `to_int` and `to_float` unchanged, and the old `to_u8` still built.

use super::collections::assert_fixture_panics;
use super::*;

/// `to_byte` converts to `u8`, `try_to_byte` hands back an `Option<u8>`, and
/// `to_int` and `to_float` keep their behaviour.
#[test]
pub(crate) fn the_byte_conversions_use_the_everyday_names() {
    assert_fixture_runs("tests/fixtures/numeric_byte_name.kp");
}

/// A value past 255 panics at `to_byte`. The panic names the builtin's key,
/// `to_u8`, which is the same text the conversion has always printed.
#[test]
pub(crate) fn a_byte_overflow_panics_at_the_call() {
    assert_fixture_panics("numeric_byte_overflow", "to_u8: value out of range");
}

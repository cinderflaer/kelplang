//! A std function that only forwards to a C symbol is a binding (issue #294).

use super::*;

/// The std bindings behave like the wrappers they replaced: the same names,
/// the same arguments in the same order, and the same results.
#[test]
pub(crate) fn std_bindings_forward_their_arguments_to_the_c_symbol() {
    assert_compiles_and_runs("tests/fixtures/std_binding_forwards.kp", "true\n6\nllo\n98\nfalse\n");
}

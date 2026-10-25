//! Default argument values (issue #296): a parameter of a plain `fun` may take
//! one literal, checked at every call; a call may leave it out.

use super::*;

/// A call with and without the argument, positionally and by name.
#[test]
pub(crate) fn a_call_may_leave_out_an_argument_that_has_a_default() {
    assert_fixture_runs("tests/fixtures/default_args.kp");
}

/// A default that is an expression is refused, and the refusal names the rule.
#[test]
pub(crate) fn a_default_that_is_not_a_literal_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_expr_refused.kp", "must be a literal");
}

/// A default that names another parameter is refused by the same rule.
#[test]
pub(crate) fn a_default_that_names_another_parameter_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_param_ref_refused.kp", "must be a literal");
}

/// A defaulted parameter before a required one is refused, and the name is given.
#[test]
pub(crate) fn a_parameter_with_a_default_must_come_last() {
    assert_rejected_with("tests/fixtures/default_args_order_refused.kp", "after every parameter without a default");
}

/// The default is checked against the parameter type at the call.
#[test]
pub(crate) fn a_default_of_the_wrong_type_is_refused_at_the_call() {
    assert_rejected_with("tests/fixtures/default_args_mismatch_refused.kp", "'factor'");
}

/// Receiver methods, generic functions, fun literals and extern funs take no default yet.
#[test]
pub(crate) fn a_default_on_a_method_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_method_refused.kp", "receiver method");
}

#[test]
pub(crate) fn a_default_on_a_generic_function_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_generic_refused.kp", "generic function");
}

#[test]
pub(crate) fn a_default_on_a_fun_literal_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_literal_fn_refused.kp", "fun literal");
}

#[test]
pub(crate) fn a_default_on_an_extern_fun_is_refused() {
    assert_rejected_with("tests/fixtures/default_args_extern_refused.kp", "extern fun");
}

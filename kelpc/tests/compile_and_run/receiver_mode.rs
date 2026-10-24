//! The receiver mode is inferred from the body (issue #286).

use super::*;

/// A method that writes self needs no `mut` outside a datac block, and a method
/// that writes self only through another such method is inferred too. A method
/// that only reads self stays a shared borrow, and a shared caller can use it.
#[test]
pub(crate) fn a_method_that_writes_self_is_inferred_exclusive_without_mut() {
    assert_compiles_and_runs("tests/fixtures/receiver_infer.kp", "2\n3\n6\n6\n6\n");
}

/// A method inferred exclusive cannot be called through a shared borrow, and the
/// refusal names the rule (the receiver is inferred from the body).
#[test]
pub(crate) fn a_call_on_a_shared_borrow_to_a_method_that_writes_self_names_the_rule() {
    assert_rejected_with(
        "tests/fixtures/receiver_infer_shared_call.kp",
        "writes its receiver, so a call needs an exclusive borrow",
    );
}

/// Inside a datac block the receiver mode is written: a method that writes self
/// without `mut` is refused, with `mut` named as the fix.
#[test]
pub(crate) fn a_datac_block_method_that_writes_self_needs_mut_written() {
    assert_rejected_with(
        "tests/fixtures/receiver_infer_datac_block.kp",
        "write 'mut bump()'",
    );
}

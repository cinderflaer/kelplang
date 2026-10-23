use super::*;

// ---------------------------------------------------------------------------
// `const NAME = value`: the type of a const comes from its value when the
// annotation is left out. A const used where another type is needed keeps its
// annotation.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_const_without_a_type_takes_the_type_of_its_value() {
    assert_compiles_and_runs(
        "tests/fixtures/const_infer.kp",
        "268435456\n10\n1.500000\nx\ntrue\n44\n11\n",
    );
}

#[test]
pub(crate) fn a_const_without_a_type_is_not_retyped_by_its_use() {
    assert_rejected_with("tests/fixtures/const_infer_address.kp", "usize");
}

#[test]
pub(crate) fn a_const_without_a_type_must_hold_a_scalar() {
    assert_rejected_with("tests/fixtures/const_infer_array.kp", "write the type");
}

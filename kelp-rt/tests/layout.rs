//! The ABI layout of the runtime structs, checked from outside the crate.
//! The compile-time asserts in `abi.rs` cover the same facts; this test pins
//! them where a failure reads as a test name.

use core::mem::{align_of, offset_of, size_of};

use kelp_rt::abi::{Bytes, FieldKind, LAYOUTS, String, View};

#[test]
fn bytes_is_three_words() {
    assert_eq!(size_of::<Bytes>(), 24);
    assert_eq!(align_of::<Bytes>(), 8);
    assert_eq!(offset_of!(Bytes, data), 0);
    assert_eq!(offset_of!(Bytes, len), 8);
    assert_eq!(offset_of!(Bytes, cap), 16);
}

#[test]
fn string_matches_bytes() {
    assert_eq!(size_of::<String>(), size_of::<Bytes>());
    assert_eq!(offset_of!(String, len), 8);
    assert_eq!(offset_of!(String, cap), 16);
}

#[test]
fn view_is_two_words() {
    assert_eq!(size_of::<View>(), 16);
    assert_eq!(offset_of!(View, len), 8);
}

#[test]
fn layout_table_rows_have_the_struct_widths() {
    let bytes = LAYOUTS.iter().find(|l| l.name == "kelp_Bytes").expect("kelp_Bytes row");
    let kinds: Vec<FieldKind> = bytes.fields.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, [FieldKind::Ptr, FieldKind::I64, FieldKind::I64]);
    let view = LAYOUTS.iter().find(|l| l.name == "kelp_View").expect("kelp_View row");
    assert_eq!(view.fields.len(), 2);
}

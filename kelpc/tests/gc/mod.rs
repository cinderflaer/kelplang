//! Collector tests. Included from `src/lib.rs` under `cfg(test)`, run by
//! `cargo test` inside `kelpc/gc`.

use crate::types::{DESC_NOSCAN, KelpGcType, KelpGcVariant};
use core::mem::{align_of, offset_of, size_of};

#[test]
fn c_layout_matches_header() {
    // kelp_gc_type: 4 u32 + u32 + pointer + u32 + u32 + u32 + pointer.
    assert_eq!(offset_of!(KelpGcType, id), 0);
    assert_eq!(offset_of!(KelpGcType, size), 4);
    assert_eq!(offset_of!(KelpGcType, align), 8);
    assert_eq!(offset_of!(KelpGcType, flags), 12);
    assert_eq!(offset_of!(KelpGcType, n_ptrs), 16);
    assert_eq!(offset_of!(KelpGcType, ptr_off), 24);
    assert_eq!(offset_of!(KelpGcType, tag_off), 32);
    assert_eq!(offset_of!(KelpGcType, n_variants), 36);
    assert_eq!(offset_of!(KelpGcType, variants), 40);
    assert_eq!(size_of::<KelpGcType>(), 48);
    assert_eq!(align_of::<KelpGcType>(), 8);
    // kelp_gc_variant: u32 then pointer.
    assert_eq!(offset_of!(KelpGcVariant, n_ptrs), 0);
    assert_eq!(offset_of!(KelpGcVariant, ptr_off), 8);
    assert_eq!(size_of::<KelpGcVariant>(), 16);
    let _ = DESC_NOSCAN;
}

//! Descriptor and header layouts. The C side reads these fields at these
//! offsets; `kelpc/tests/gc` checks the offsets.

use core::ffi::c_void;

pub const DESC_NOSCAN: u32 = 0x1;
pub const DESC_ENUM: u32 = 0x2;

pub const HDR_MARK: u64 = 1 << 0;
pub const HDR_STICKY: u64 = 1 << 1;
pub const HDR_NOSCAN: u64 = 1 << 2;
pub const HDR_ARRAY: u64 = 1 << 3;
pub const HDR_LARGE: u64 = 1 << 4;
pub const HDR_TYPE_SHIFT: u32 = 8;
pub const HDR_COUNT_SHIFT: u32 = 32;

/// Largest type id the 24-bit header field can hold.
pub const TYPE_ID_MAX: u32 = (1 << 24) - 1;
/// Ids the v1 type table can hold (table is 8 MiB of virtual memory). Ids at
/// or above this are a fatal error at registration. Mirrors
/// KELP_GC_TYPE_ID_V1_LIMIT in the header.
pub const TYPE_TABLE_IDS: usize = 1 << 20;
/// Width of the enum tag the descriptor reads (u32 at tag_off). Mirrors
/// KELP_GC_ENUM_TAG_BYTES in the header.
pub const KELP_GC_ENUM_TAG_BYTES: usize = 4;

#[repr(C)]
pub struct KelpGcVariant {
    pub n_ptrs: u32,
    pub ptr_off: *const u32,
}

#[repr(C)]
pub struct KelpGcType {
    pub id: u32,
    pub size: u32,
    pub align: u32,
    pub flags: u32,
    pub n_ptrs: u32,
    pub ptr_off: *const u32,
    pub tag_off: u32,
    pub n_variants: u32,
    pub variants: *const KelpGcVariant,
}

/// Result codes of the root entry points (`kelp_gc_register_root_region` and
/// `kelp_gc_unregister_root_region`).
pub const KELP_GC_ROOT_OK: i32 = 0;
pub const KELP_GC_ROOT_FULL: i32 = 1;
pub const KELP_GC_ROOT_STALE: i32 = 2;

/// The counters `kelp_gc_stats` fills. All fields are u64.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KelpGcStats {
    pub collections: u64,
    pub pause_total_us: u64,
    pub pause_max_us: u64,
    pub roots: u64,
    pub root_slots: u64,
    pub charged_bytes: u64,
}

// The descriptors are static data emitted by the compiler; the pointers they
// hold are read-only, so sharing them across threads is sound.
unsafe impl Sync for KelpGcType {}
unsafe impl Sync for KelpGcVariant {}

/// Opaque pointer alias for C-facing signatures.
pub type CVoid = c_void;

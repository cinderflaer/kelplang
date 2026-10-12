//! The collector's entry points the runtime calls itself. A managed build
//! links them from `kelpc/gc`; a hosted build has no collector, so each one is
//! a no-op there, the same as the C runtime's `KELP_GC_MANAGED` guards.

#[cfg(feature = "managed")]
unsafe extern "C" {
    fn kelp_gc_thread_detach();
    fn kelp_gc_enter_native();
    fn kelp_gc_leave_native();
    fn kelp_gc_safepoint();
}

/// Leaves the collector on a worker's exit. Detaching a thread that never
/// attached does nothing.
#[cfg(feature = "managed")]
pub fn thread_detach() {
    unsafe { kelp_gc_thread_detach() }
}

/// Leaves the collector on a worker's exit. A hosted build has no collector.
#[cfg(not(feature = "managed"))]
pub fn thread_detach() {}

/// Marks this thread as blocked in native code, so a collection on another
/// thread does not wait for it. Paired with [`native_leave`] around a blocking
/// pthread call only.
#[cfg(feature = "managed")]
pub fn native_enter() {
    unsafe { kelp_gc_enter_native() }
}

/// Ends a [`native_enter`] bracket.
#[cfg(feature = "managed")]
pub fn native_leave() {
    unsafe { kelp_gc_leave_native() }
}

/// A hosted build has no collector to hold up.
#[cfg(not(feature = "managed"))]
pub fn native_enter() {}

/// A hosted build has no collector to hold up.
#[cfg(not(feature = "managed"))]
pub fn native_leave() {}

/// Polls the collector from a long runtime loop, so a collection on another
/// thread stops this one at the poll rather than at the stop timeout.
#[cfg(feature = "managed")]
pub fn safepoint() {
    unsafe { kelp_gc_safepoint() }
}

/// A hosted build has no collector, so the poll does nothing.
#[cfg(not(feature = "managed"))]
pub fn safepoint() {}

/// The byte interval between two safepoint polls in a runtime scan, the same as
/// the C runtime's `KELP_POLL_BYTES`.
pub const POLL_BYTES: usize = 64 * 1024;

/// The collector's type descriptor, as `kelpc/gc` reads it (`KelpGcType`).
/// Its field order and sizes are the contract; `kelpc/gc/src/types.rs` pins them.
#[cfg(feature = "managed")]
#[repr(C)]
pub struct GcType {
    pub id: u32,
    pub size: u32,
    pub align: u32,
    pub flags: u32,
    pub n_ptrs: u32,
    pub ptr_off: *const u32,
    pub tag_off: u32,
    pub n_variants: u32,
    pub variants: *const core::ffi::c_void,
}

#[cfg(feature = "managed")]
unsafe impl Sync for GcType {}

#[cfg(feature = "managed")]
unsafe extern "C" {
    fn kelp_gc_register_root_region(base: *mut core::ffi::c_void, layout: *const GcType, out: *mut u64) -> core::ffi::c_int;
    fn kelp_gc_unregister_root_region(h: u64) -> core::ffi::c_int;
    fn kelp_gc_collect();
    fn kelp_gc_test_spawn_pause() -> core::ffi::c_int;
}

/// Registers the `size`-described slot at `base` as a root and returns its
/// handle, or `None` when the collector's root table cannot grow (nothing is
/// registered then). A hosted build has no roots, so the handle is 0.
///
/// # Safety
/// `base` must hold a value described by `layout` until the root is dropped.
#[cfg(feature = "managed")]
pub unsafe fn root_add(base: *mut core::ffi::c_void, layout: *const core::ffi::c_void) -> Option<u64> {
    let mut h = 0u64;
    if unsafe { kelp_gc_register_root_region(base, layout.cast(), &raw mut h) } == 0 { Some(h) } else { None }
}

/// A hosted build has no roots: nothing is registered and the handle is 0.
///
/// # Safety
/// Nothing; the arguments are unused.
#[cfg(not(feature = "managed"))]
pub unsafe fn root_add(base: *mut core::ffi::c_void, layout: *const core::ffi::c_void) -> Option<u64> {
    let _ = (base, layout);
    Some(0)
}

/// Clears a root from [`root_add`]. A zero handle names no root and does nothing.
///
/// # Safety
/// `h` must be 0 or a handle from [`root_add`] not cleared yet.
#[cfg(feature = "managed")]
pub unsafe fn root_drop(h: u64) {
    if h != 0 {
        unsafe {
            kelp_gc_unregister_root_region(h);
        }
    }
}

/// A hosted build has no roots.
///
/// # Safety
/// Nothing; `h` is unused.
#[cfg(not(feature = "managed"))]
pub unsafe fn root_drop(h: u64) {
    let _ = h;
}

/// Runs a full collection, as the spawn test window does.
///
/// # Safety
/// The calling thread must be attached to the collector.
#[cfg(feature = "managed")]
pub unsafe fn collect() {
    unsafe { kelp_gc_collect() }
}

/// A hosted build has no collector to run.
///
/// # Safety
/// Nothing.
#[cfg(not(feature = "managed"))]
pub unsafe fn collect() {}

/// Whether the test hook `KELP_GC_TEST_SPAWN_PAUSE=1` holds a new worker
/// before its first use. Always false in a hosted build.
#[cfg(feature = "managed")]
pub fn spawn_pause() -> bool {
    unsafe { kelp_gc_test_spawn_pause() != 0 }
}

/// A hosted build has no test hold.
#[cfg(not(feature = "managed"))]
pub fn spawn_pause() -> bool {
    false
}

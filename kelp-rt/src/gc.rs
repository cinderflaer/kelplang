//! The collector's entry points the runtime calls itself. A managed build
//! links them from `kelpc/gc`; a hosted build has no collector, so each one is
//! a no-op there, the same as the C runtime's `KELP_GC_MANAGED` guards.

#[cfg(feature = "managed")]
unsafe extern "C" {
    fn kelp_gc_thread_detach();
    fn kelp_gc_enter_native();
    fn kelp_gc_leave_native();
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

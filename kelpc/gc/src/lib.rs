//! KelpScript managed-mode garbage collector core (WP1, phase 1).
//!
//! Non-moving mark-region (Immix-style) tracing collector with a precise heap
//! and conservatively scanned thread stacks. See
//! `kelpscript-gc-paper.md` sections 3 and 4. The C ABI is `include/kelp_gc.h`.

#![cfg_attr(not(test), no_std)]

pub mod types;
pub mod sys;

pub use types::{KelpGcType, KelpGcVariant};

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { sys::abort() }
}

#[cfg(test)]
#[path = "../../tests/gc/mod.rs"]
mod tests;

// ---- Exported C ABI (stubs in this commit; implemented in later commits). ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_init(_heap_min: usize, _heap_max: usize) {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc(_t: *const KelpGcType) -> *mut u8 {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc_array(_elem: *const KelpGcType, _count: usize) -> *mut u8 {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_collect() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_safepoint() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_enter_native() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_leave_native() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_register_root_region(_base: *mut u8, _layout: *const KelpGcType) {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_unregister_root_region(_base: *mut u8) {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_is_heap_ptr(_p: *const u8) -> i32 {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_attach() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_detach() {
    unsafe { sys::abort() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_set_panic_hook(_hook: Option<unsafe extern "C" fn(*const u8)>) {
    unsafe { sys::abort() }
}

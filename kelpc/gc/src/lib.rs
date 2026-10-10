//! KelpScript managed-mode garbage collector core (WP1, phase 1).
//!
//! Non-moving mark-region (Immix-style) tracing collector with a precise heap
//! and conservatively scanned thread stacks. See `kelpscript-gc-paper.md`
//! sections 3 and 4. The C ABI is `include/kelp_gc.h`.
#![cfg_attr(not(test), no_std)]
#![allow(unsafe_op_in_unsafe_fn)]

pub mod heap;
pub mod mark;
pub mod state;
pub mod sys;
pub mod threads;
pub mod types;
pub mod verify;

use core::sync::atomic::Ordering::{Acquire, Release};

use heap::*;
use state::*;
use types::*;

pub use types::{KelpGcType, KelpGcVariant};

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { sys::abort() }
}

/// The personality routine that core's unwinding code names. This crate builds
/// with `panic = "abort"`, so no frame is ever unwound and this is never called.
/// It exists only so the staticlib links without the unwinder.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

#[cfg(test)]
#[path = "../../tests/gc/mod.rs"]
mod tests;

/// Map the heap region and metadata. Called once, under INIT == 1.
unsafe fn do_init(heap_min: usize, heap_max: usize) {
    let s = st();
    let max = if heap_max == 0 { DEFAULT_HEAP_MAX } else { heap_max };
    let nblocks = core::cmp::max(4, max.div_ceil(BLOCK));
    let region_bytes = nblocks * BLOCK;
    let raw = map(region_bytes + BLOCK);
    let meta_bytes = nblocks * core::mem::size_of::<BlockMeta>();
    let meta = map(meta_bytes);
    let types_bytes = TYPE_TABLE_IDS * 8;
    let types = map(types_bytes);
    // One mark entry per possible object (minimum 16 bytes each).
    let mark_cap = region_bytes / 16;
    let mark = map(mark_cap * 8);
    s.region = (raw + BLOCK - 1) & !(BLOCK - 1);
    s.region_bytes = region_bytes;
    s.nblocks = nblocks;
    s.blocks = meta;
    s.types = types;
    s.mark_stack = mark;
    s.mark_cap = mark_cap;
    s.heap_max = region_bytes;
    s.heap_min = if heap_min == 0 { DEFAULT_HEAP_MIN } else { heap_min };
    s.trigger = s.heap_min;
    s.epoch = 0;
    s.stress_n = env_usize(b"KELP_GC_STRESS\0").unwrap_or(0);
    s.verify = env_usize(b"KELP_GC_VERIFY\0").unwrap_or(0) != 0;
    s.scan_stacks = true;
    s.stop_timeout_ms = env_usize(b"KELP_GC_STOP_TIMEOUT_MS\0").unwrap_or(DEFAULT_STOP_TIMEOUT_MS);
    s.large_cap = region_bytes / LARGE_MIN + 1;
    s.large = map(s.large_cap * 8);
    threads::init_locks();
}

/// Decimal value of an environment variable, if set and numeric.
pub unsafe fn env_usize(name: &[u8]) -> Option<usize> {
    let p = sys::getenv(name.as_ptr() as *const _);
    if p.is_null() {
        return None;
    }
    let mut n = 0usize;
    let mut q = p as *const u8;
    if *q == 0 {
        return None;
    }
    while *q >= b'0' && *q <= b'9' {
        n = n.checked_mul(10)?.checked_add((*q - b'0') as usize)?;
        q = q.add(1);
    }
    if *q != 0 {
        return None;
    }
    Some(n)
}

/// Anonymous, zeroed, lazily committed mapping. Aborts the process on failure.
unsafe fn map(bytes: usize) -> usize {
    let p = sys::mmap(
        core::ptr::null_mut(),
        bytes,
        sys::PROT_READ | sys::PROT_WRITE,
        sys::MAP_PRIVATE | sys::MAP_ANONYMOUS | sys::MAP_NORESERVE,
        -1,
        0,
    ) as usize;
    if p == usize::MAX || p == 0 {
        sys::fatal(b"out of memory\0");
    }
    p
}

unsafe fn init_once(heap_min: usize, heap_max: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    match INIT.compare_exchange(0, 1, Acquire, Relaxed) {
        Ok(_) => {
            do_init(heap_min, heap_max);
            INIT.store(2, Release);
        }
        Err(1) => {
            while INIT.load(Acquire) != 2 {
                core::hint::spin_loop();
            }
        }
        Err(_) => {}
    }
}

#[inline]
unsafe fn ensure_init() {
    if INIT.load(Acquire) != 2 {
        init_once(0, 0);
    }
}

/// Allocate one object or array. Returns the payload pointer.
unsafe fn alloc_common(d: *const KelpGcType, count: usize, is_array: bool) -> *mut u8 {
    ensure_init();
    if threads::my_index().is_none() {
        sys::fatal(b"gc: allocation on a thread that is not attached (call kelp_gc_thread_attach before the first allocation)\0");
    }
    let desc = &*d;
    if desc.align > 8 {
        sys::fatal(b"gc: type alignment above 8 is not supported in v1\0");
    }
    let payload = if is_array {
        if count > u32::MAX as usize {
            sys::fatal(b"out of memory\0");
        }
        match count.checked_mul(desc.size as usize) {
            Some(p) => p,
            None => sys::fatal(b"out of memory\0"),
        }
    } else {
        desc.size as usize
    };
    let total = total_for(payload);
    threads::safepoint();
    let s = st();
    if s.stress_n != 0 {
        s.stress_count += 1;
        if s.stress_count % s.stress_n == 0 {
            threads::collect();
        }
    }
    if st().bytes_since >= st().trigger {
        threads::collect();
    }
    let mut collected_here = false;
    loop {
        lock_heap();
        register_type(d);
        let got = try_alloc(total);
        if let Some(h) = got {
            let s = st();
            let mut hdr = (desc.id as u64) << HDR_TYPE_SHIFT;
            if is_array {
                hdr |= HDR_ARRAY | ((count as u64) << HDR_COUNT_SHIFT);
            }
            if desc.flags & DESC_NOSCAN != 0 {
                hdr |= HDR_NOSCAN;
            }
            if total > LARGE_MIN {
                hdr |= HDR_LARGE;
            }

            // New objects carry the unmarked value: the epoch's complement.
            hdr |= (s.epoch ^ 1) as u64;
            wr(h, hdr);
            core::ptr::write_bytes((h + 8) as *mut u8, 0, total - 8);
            s.bytes_since += total;
            s.allocs += 1;
            unlock_heap();
            return (h + 8) as *mut u8;
        }
        unlock_heap();
        if collected_here {
            sys::fatal(b"out of memory\0");
        }
        collected_here = threads::collect();
    }
}

// ---- Exported C ABI. See include/kelp_gc.h. ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_init(heap_min: usize, heap_max: usize) {
    init_once(heap_min, heap_max);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc(t: *const KelpGcType) -> *mut u8 {
    alloc_common(t, 1, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc_array(elem: *const KelpGcType, count: usize) -> *mut u8 {
    alloc_common(elem, count, true)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_collect() {
    ensure_init();
    threads::collect();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_safepoint() {
    ensure_init();
    threads::safepoint();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_enter_native() {
    ensure_init();
    threads::enter_native();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_leave_native() {
    ensure_init();
    threads::leave_native();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_register_root_region(base: *mut u8, layout: *const KelpGcType) {
    threads::register_root(base as usize, layout as usize);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_unregister_root_region(base: *mut u8) {
    threads::unregister_root(base as usize);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_is_heap_ptr(p: *const u8) -> i32 {
    ensure_init();
    lock_heap();
    let r = find_object(p as usize).is_some();
    unlock_heap();
    r as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_attach() {
    ensure_init();
    threads::attach();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_detach() {
    threads::detach();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_set_panic_hook(hook: Option<unsafe extern "C" fn(*const u8)>) {
    st().oom_hook = match hook {
        Some(f) => f as usize,
        None => 0,
    };
}

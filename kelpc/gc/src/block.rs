//! Manual memory blocks, runtime part (design section 3, option A).
//!
//! A block holds one element in its own mapped chunk. The chunk is charged
//! against `heap_max` while it lives, and its root is registered in the same
//! root table the shared pool's roots use, so a clearing marks through it.
//! Creation runs one clearing before it fails (section 18.4).
//!
//! This is a Rust API only. The C entry points come with the lowering, and
//! they change the exported surface that test `t78` pins.

use crate::state::BLOCK;
use crate::state::{lock_heap, st, unlock_heap};
use crate::sys;
use crate::threads;
use crate::types::KelpGcType;

/// The creation succeeded.
pub const BLOCK_OK: i32 = 0;
/// The shared pool and the live blocks would pass `heap_max`, even after a clearing.
pub const BLOCK_OVER_BUDGET: i32 = 1;
/// No root slot could be added (the root table cannot grow).
pub const BLOCK_ROOT_FULL: i32 = 2;
/// The chunk could not be mapped.
pub const BLOCK_NO_MEMORY: i32 = 3;

/// The chunk size for one element of `d`: its size, rounded up to 8 bytes.
fn chunk_bytes(d: &KelpGcType) -> usize {
    core::cmp::max(8, (d.size as usize + 7) & !7)
}

/// Whether `bytes` more charged memory keeps the shared pool and the live
/// blocks within `heap_max`. Heap lock held.
unsafe fn fits(bytes: usize) -> bool {
    let s = st();
    s.used_blocks * BLOCK + s.charged_bytes + bytes <= s.heap_max
}

/// Create a block holding one zeroed element of `d`. Returns the element's
/// address and the root handle, or a `BLOCK_*` status with nothing left
/// charged or registered.
///
/// # Safety
/// The calling thread must be attached and `d` must describe a live type.
pub unsafe fn alloc(d: *const KelpGcType) -> Result<(*mut u8, u64), i32> {
    crate::ensure_init();
    if threads::my_index().is_none() {
        sys::fatal(b"gc: block allocation on a thread that is not attached (call kelp_gc_thread_attach before the first allocation)\0");
    }
    let desc = &*d;
    if desc.align > 8 {
        sys::fatal(b"gc: type alignment above 8 is not supported in v1\0");
    }
    let bytes = chunk_bytes(desc);

    // Step 0 of section 3: the budget check, with one clearing if it fails.
    lock_heap();
    let mut ok = fits(bytes);
    if ok {
        st().charged_bytes += bytes;
    }
    unlock_heap();
    if !ok {
        threads::collect();
        lock_heap();
        ok = fits(bytes);
        if ok {
            st().charged_bytes += bytes;
        }
        unlock_heap();
        if !ok {
            return Err(BLOCK_OVER_BUDGET);
        }
    }

    // Step 3: map the chunk. A fresh anonymous mapping is zero, so the element
    // starts zeroed without a write. A pooled chunk, when one exists, is zeroed
    // on hand-out.
    let Some(base) = sys::try_map(bytes) else {
        uncharge(bytes);
        return Err(BLOCK_NO_MEMORY);
    };

    // Step 4: register the root. Its failure undoes the mapping and the charge.
    match threads::register_root(base, d as usize) {
        Ok(h) => Ok((base as *mut u8, h)),
        Err(_) => {
            sys::munmap(base as *mut core::ffi::c_void, bytes);
            uncharge(bytes);
            Err(BLOCK_ROOT_FULL)
        }
    }
}

/// Free the block that `h` roots and whose element is at `base`. The handle is
/// cleared first, then the memory is released. Returns false, and does nothing,
/// if `h` is stale.
///
/// # Safety
/// `base` and `d` must be the values `alloc` returned and was called with.
pub unsafe fn free(h: u64, base: *mut u8, d: *const KelpGcType) -> bool {
    if !threads::unregister_root(h) {
        return false;
    }
    let bytes = chunk_bytes(&*d);
    sys::munmap(base as *mut core::ffi::c_void, bytes);
    uncharge(bytes);
    true
}

fn uncharge(bytes: usize) {
    lock_heap();
    unsafe {
        st().charged_bytes -= bytes;
    }
    unlock_heap();
}

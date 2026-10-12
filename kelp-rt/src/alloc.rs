//! Port of kelp_rt.c:2-78 (the Box small-block free lists) and the allocation
//! primitives of kelp_rt.c:1119-1131 (`kelp_bytes_alloc_raw`) and the byte
//! buffer allocator of kelp_rt.c:707-713 (`kelp_rt_bytes_alloc`).
//!
//! The Box pool is per thread. The C runtime kept its heads in `__thread`
//! storage that the LLVM backend read inline. This crate is `no_std`, and Rust
//! stable has neither `thread_local!` there nor a named TLS symbol, so each
//! thread's pool is a heap record reached through a pthread key, and
//! [`kelp_box_pool`] returns it. The layout is the C one: `heads` first, then
//! `registered` at byte offset `BOX_POOL_CLASSES * 8`. The key's destructor
//! hands the blocks back to malloc when the thread exits.
use core::ffi::{c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::abi::Bytes;

/// The largest block the pool keeps; bigger blocks go straight to malloc.
pub const BOX_POOL_MAX: usize = 64;
/// One free list per 16-byte size class.
pub const BOX_POOL_CLASSES: usize = BOX_POOL_MAX / 16;

#[repr(C)]
pub struct BoxBlock {
    next: *mut BoxBlock,
}

/// The calling thread's free lists. `registered` is 1 once the pool exists,
/// which is what the inline free path tests.
#[repr(C)]
pub struct BoxPool {
    pub heads: [*mut BoxBlock; BOX_POOL_CLASSES],
    pub registered: c_int,
}

static POOL_KEY: AtomicU32 = AtomicU32::new(0);
static mut POOL_KEY_ONCE: libc::pthread_once_t = libc::PTHREAD_ONCE_INIT;

extern "C" fn make_pool_key() {
    let mut key: libc::pthread_key_t = 0;
    unsafe {
        if libc::pthread_key_create(&raw mut key, Some(drain_pool)) != 0 {
            libc::abort();
        }
    }
    POOL_KEY.store(key as u32, Ordering::Release);
}

fn pool_key() -> libc::pthread_key_t {
    unsafe {
        libc::pthread_once(&raw mut POOL_KEY_ONCE, make_pool_key);
    }
    POOL_KEY.load(Ordering::Acquire) as libc::pthread_key_t
}

/// The key's destructor: returns every pooled block to malloc and frees the
/// pool record.
unsafe extern "C" fn drain_pool(value: *mut c_void) {
    unsafe {
        let pool = value.cast::<BoxPool>();
        for head in (*pool).heads.iter_mut() {
            let mut b = *head;
            while !b.is_null() {
                let next = (*b).next;
                libc::free(b.cast());
                b = next;
            }
            *head = ptr::null_mut();
        }
        libc::free(pool.cast());
    }
}

/// The calling thread's Box pool, created on first use. The LLVM backend pops
/// and pushes the per-class lists through this pointer.
///
/// # Safety
/// The pointer is valid for the calling thread's life; the caller must not
/// keep it past the thread's exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_box_pool() -> *mut BoxPool {
    unsafe {
        let key = pool_key();
        let mut pool = libc::pthread_getspecific(key).cast::<BoxPool>();
        if pool.is_null() {
            pool = libc::calloc(1, core::mem::size_of::<BoxPool>()).cast();
            if pool.is_null() {
                libc::abort();
            }
            (*pool).registered = 1;
            libc::pthread_setspecific(key, pool.cast());
        }
        pool
    }
}

fn class_of(size: usize) -> usize {
    if size == 0 { 0 } else { (size - 1) / 16 }
}

/// Allocates a Box block of `size` bytes at `align`. A pooled size takes the
/// head of its class list when there is one, and malloc otherwise.
///
/// # Safety
/// The result must be released with [`kelp_box_free`] using the same size and
/// align.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_box_alloc(size: usize, align: usize) -> *mut c_void {
    unsafe {
        if align > 16 || size > BOX_POOL_MAX {
            return libc::malloc(size);
        }
        let c = class_of(size);
        let pool = &mut *kelp_box_pool();
        let b = pool.heads[c];
        if !b.is_null() {
            pool.heads[c] = (*b).next;
            return b.cast();
        }
        libc::malloc((c + 1) * 16)
    }
}

/// Releases a Box block allocated by [`kelp_box_alloc`] with the same size and
/// align. A pooled block joins the calling thread's list.
///
/// # Safety
/// `p` must be null or a live block from [`kelp_box_alloc`] of this size class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_box_free(p: *mut c_void, size: usize, align: usize) {
    unsafe {
        if p.is_null() {
            return;
        }
        if align > 16 || size > BOX_POOL_MAX {
            libc::free(p);
            return;
        }
        let c = class_of(size);
        let pool = &mut *kelp_box_pool();
        let b = p.cast::<BoxBlock>();
        (*b).next = pool.heads[c];
        pool.heads[c] = b;
    }
}

/// The buffer allocator the owned `String` and `Bytes` types use. Hosted, it
/// is malloc; a managed build (`--features managed`) takes its buffers from the
/// collector, which zeroes them and reclaims them without a free.
///
/// # Safety
/// The result is `n` bytes of storage, or null on failure; free it with
/// [`buf_free`].
#[cfg(not(feature = "managed"))]
pub unsafe fn buf_alloc(n: usize) -> *mut u8 {
    unsafe { libc::malloc(n).cast() }
}

/// Frees a buffer from [`buf_alloc`].
///
/// # Safety
/// `p` must be null or a buffer from [`buf_alloc`] not yet freed.
#[cfg(not(feature = "managed"))]
pub unsafe fn buf_free(p: *mut u8) {
    unsafe { libc::free(p.cast()) }
}

/// The type descriptor of a byte buffer: a noscan array of `u8`, with the
/// reserved id the program's own table never hands out (`kelp_rt.c`
/// `KELP_RT_BYTES_TYPE_ID`). Mirrors the `KelpGcType` layout in `kelpc/gc/src/types.rs`.
#[cfg(feature = "managed")]
#[repr(C)]
struct GcType {
    id: u32,
    size: u32,
    align: u32,
    flags: u32,
    n_ptrs: u32,
    ptr_off: *const u32,
    tag_off: u32,
    n_variants: u32,
    variants: *const c_void,
}

#[cfg(feature = "managed")]
unsafe impl Sync for GcType {}

#[cfg(feature = "managed")]
const KELP_GC_NOSCAN: u32 = 0x1;
#[cfg(feature = "managed")]
const BYTES_TYPE_ID: u32 = 0xFFFF0;

#[cfg(feature = "managed")]
static BYTES_DESCR: GcType = GcType {
    id: BYTES_TYPE_ID,
    size: 1,
    align: 1,
    flags: KELP_GC_NOSCAN,
    n_ptrs: 0,
    ptr_off: ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: ptr::null(),
};

#[cfg(feature = "managed")]
unsafe extern "C" {
    fn kelp_gc_alloc_array(elem: *const GcType, count: usize) -> *mut c_void;
}

/// Frees nothing: the collector reclaims a managed buffer once no path reaches it.
///
/// # Safety
/// `p` is unused.
#[cfg(feature = "managed")]
pub unsafe fn buf_free(p: *mut u8) {
    let _ = p;
}

/// `n` zeroed bytes from the collector. Panics `out of memory` when it fails.
///
/// # Safety
/// The result belongs to the collector.
#[cfg(feature = "managed")]
pub unsafe fn buf_alloc(n: usize) -> *mut u8 {
    unsafe {
        let p = kelp_gc_alloc_array(&raw const BYTES_DESCR, n).cast::<u8>();
        if p.is_null() {
            crate::panic::kelp_panic(c"out of memory".as_ptr());
        }
        p
    }
}

/// The allocation primitive `std/mem` wraps: an owned, zero-filled,
/// NUL-terminated buffer of `n` bytes. Panics `out of memory` on failure.
///
/// # Safety
/// The result owns its buffer; release it with `kelp_bytes_drop`.
pub unsafe fn bytes_alloc_raw(n: usize) -> Bytes {
    unsafe {
        let p = buf_alloc(n + 1);
        if p.is_null() {
            crate::panic::kelp_panic(c"out of memory".as_ptr());
        }
        ptr::write_bytes(p, 0, n + 1);
        Bytes {
            data: p,
            len: n,
            cap: n,
        }
    }
}

/// The exported spelling of [`bytes_alloc_raw`], used by the C text the
/// compiler emits for `extern fun` bodies.
///
/// # Safety
/// As [`bytes_alloc_raw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_bytes_alloc_raw(n: usize) -> Bytes {
    unsafe { bytes_alloc_raw(n) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freed_pooled_block_is_the_next_one_handed_out() {
        unsafe {
            let a = kelp_box_alloc(24, 8);
            kelp_box_free(a, 24, 8);
            let b = kelp_box_alloc(24, 8);
            assert_eq!(a, b, "the free list hands back the block just freed");
            kelp_box_free(b, 24, 8);
        }
    }

    #[test]
    fn a_zero_sized_block_shares_class_zero() {
        unsafe {
            let a = kelp_box_alloc(0, 1);
            kelp_box_free(a, 0, 1);
            assert_eq!(kelp_box_alloc(1, 1), a);
            kelp_box_free(a, 1, 1);
        }
    }

    #[test]
    fn bytes_alloc_raw_is_zeroed_and_terminated() {
        unsafe {
            let b = kelp_bytes_alloc_raw(5);
            assert_eq!(b.len, 5);
            assert_eq!(b.cap, 5);
            for i in 0..=5 {
                assert_eq!(*b.data.add(i), 0);
            }
            buf_free(b.data);
        }
    }
}

//! Thread registry, safepoints, native state, root regions, and the
//! stop-the-world handshake. All state here is guarded by G_MUTEX.
//!
//! Each attached thread's record is found through a pthread key that holds
//! its registry index, so the allocator and the safepoint poll never scan the
//! table. The record is trusted only if its thread id matches the caller.
#![allow(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::Ordering::{Acquire, Release};

use crate::mark;
use crate::tls;
use crate::state::*;
use crate::sys::{self, Msg, PthreadAttr};
use crate::types::*;

pub const RUNNING: u32 = 0;
pub const PARKED: u32 = 1;
pub const NATIVE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThreadRec {
    pub used: bool,
    pub state: u32,
    pub tid: u64,
    /// One past the highest stack address of the thread.
    pub hi: usize,
    /// Lowest address of the live stack, recorded when the thread stopped.
    pub sp: usize,
    /// Callee-saved registers, spilled when the thread stopped.
    pub regs: [usize; 6],
}

/// One slot of the root table. `gen` counts how many times the slot has been
/// cleared, so a handle to an earlier entry no longer matches (R2). `next_free`
/// links the free list: the index plus one of the next free slot, zero at the end.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RootRec {
    pub used: bool,
    pub generation: u32,
    pub next_free: u32,
    pub base: usize,
    pub layout: usize,
}

/// A root handle: the slot index plus one in the low 32 bits, and the slot's
/// generation in the high 32 bits. Zero is never a handle.
pub type RootHandle = u64;

/// Why a root could not be registered. The only one is a table that cannot grow
/// (R10): the caller gets it back as a value and nothing is registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootError {
    Full,
}

/// The root table: a mapped array of `cap` slots, of which the first `hw` have
/// ever been used (the high-water index), with a free list of cleared slots.
/// Guarded by `G_MUTEX`.
pub struct RootTable {
    pub recs: usize,
    pub cap: usize,
    pub hw: usize,
    pub free_head: u32,
    pub live: usize,
    /// Used-slot bitmap: bit `i` is set while slot `i` holds a live root. One
    /// u64 word covers 64 slots, so the scan skips cleared slots 64 at a time.
    /// Mapped with `recs` and grown with it (`cap / 64` words).
    pub used_bits: usize,
}

pub static ROOTS: Global<RootTable> = Global(core::cell::UnsafeCell::new(RootTable {
    recs: 0,
    cap: 0,
    hw: 0,
    free_head: 0,
    live: 0,
    used_bits: 0,
}));

#[inline]
unsafe fn rec(i: usize) -> *mut RootRec {
    ((*ROOTS.get()).recs as *mut RootRec).add(i)
}

/// Grow the table to `new_cap` slots, copying the used prefix. Registry lock
/// held. Raw pointers only: a `&mut` to the table is never held across a call
/// that also changes it.
unsafe fn grow_roots(new_cap: usize) -> Result<(), RootError> {
    let t = ROOTS.get();
    let bytes = new_cap * core::mem::size_of::<RootRec>();
    let bit_bytes = (new_cap / 64) * 8;
    let Some(fresh) = sys::try_map(bytes) else {
        return Err(RootError::Full);
    };
    let Some(fresh_bits) = sys::try_map(bit_bytes) else {
        sys::munmap(fresh as *mut core::ffi::c_void, bytes);
        return Err(RootError::Full);
    };
    let old = (*t).recs;
    let old_cap = (*t).cap;
    if old != 0 {
        core::ptr::copy_nonoverlapping(old as *const RootRec, fresh as *mut RootRec, old_cap);
        sys::munmap(old as *mut core::ffi::c_void, old_cap * core::mem::size_of::<RootRec>());
        let old_bits = (*t).used_bits;
        core::ptr::copy_nonoverlapping(old_bits as *const u64, fresh_bits as *mut u64, old_cap / 64);
        sys::munmap(old_bits as *mut core::ffi::c_void, (old_cap / 64) * 8);
    }
    (*t).recs = fresh;
    (*t).used_bits = fresh_bits;
    (*t).cap = new_cap;
    Ok(())
}

pub static THREADS: Global<[ThreadRec; MAX_THREADS]> = Global(core::cell::UnsafeCell::new(unsafe { core::mem::zeroed() }));

unsafe fn lock() {
    sys::pthread_mutex_lock(G_MUTEX.get());
}
unsafe fn unlock() {
    sys::pthread_mutex_unlock(G_MUTEX.get());
}
unsafe fn wait() {
    sys::pthread_cond_wait(G_COND.get(), G_MUTEX.get());
}
unsafe fn broadcast() {
    sys::pthread_cond_broadcast(G_COND.get());
}

unsafe fn thr(i: usize) -> *mut ThreadRec {
    (THREADS.get() as *mut ThreadRec).add(i)
}

/// Whether the thread-local cache of section 7 is in use on this target.
const CACHE: bool = cfg!(target_arch = "x86_64");

/// Registry index of the calling thread found through the pthread key, the
/// slow lookup. The record is trusted only if its thread id matches the caller.
unsafe fn slow_index() -> Option<usize> {
    let v = sys::pthread_getspecific(st().tls_key) as usize;
    if v == 0 {
        return None;
    }
    let i = v - 1;
    let t = thr(i);
    if (*t).used && (*t).tid == sys::pthread_self() as u64 {
        Some(i)
    } else {
        None
    }
}

/// Registry index of the calling thread, or None if it is not attached.
/// Lock-free. The fast path reads the thread-local cache (section 7) and does
/// no pthread call. The cache is written only by its own thread, and detach
/// clears it, so it can name a slot only while this thread owns that slot. In
/// verify mode the cache is checked against the slow lookup on every use.
pub unsafe fn my_index() -> Option<usize> {
    if !CACHE {
        return slow_index();
    }
    let v = tls::get();
    let fast = if v != 0 && (*thr(v - 1)).used { Some(v - 1) } else { None };
    if st().verify && fast != slow_index() {
        Msg::new().s(b"gc: cached thread index disagrees with the registry").die();
    }
    fast
}

/// Index of the calling thread's record. Registry lock held.
unsafe fn find_me() -> Option<usize> {
    my_index()
}

#[inline(never)]
unsafe fn marker_sp() -> usize {
    let x = 0u8;
    core::ptr::addr_of!(x) as usize
}

/// Spill callee-saved registers. Explicit `rdi` keeps the compiler from
/// choosing a callee-saved register for the pointer and clobbering it.
#[inline(always)]
unsafe fn spill(regs: *mut usize) {
    core::arch::asm!(
        "mov [rdi], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        in("rdi") regs,
        options(nostack, preserves_flags)
    );
}

/// Record that thread `i` is stopped in `state`, with its registers and sp.
/// Registry lock held. Frames above the caller of this function are scanned.
#[inline(never)]
unsafe fn record_stop(i: usize, state: u32) {
    let t = thr(i);
    (*t).state = state;
    (*t).sp = marker_sp();
    spill(core::ptr::addr_of_mut!((*t).regs) as *mut usize);
}

/// Park until no collection is in progress. Registry lock held on entry and
/// exit; the wait releases it. The caller's record must already be stopped.
unsafe fn wait_for_world(me: Option<usize>) {
    while REQUEST.load(Acquire) != 0 {
        wait();
    }
    if let Some(i) = me {
        (*thr(i)).state = RUNNING;
    }
}

/// Called at a safepoint poll when a collection is requested.
#[inline(never)]
unsafe fn park_here() {
    lock();
    let me = find_me();
    if let Some(i) = me {
        record_stop(i, PARKED);
    }
    broadcast();
    wait_for_world(me);
    unlock();
}

pub unsafe fn safepoint() {
    if REQUEST.load(Acquire) != 0 {
        park_here();
    }
}

pub unsafe fn enter_native() {
    lock();
    if let Some(i) = find_me() {
        record_stop(i, NATIVE);
    }
    broadcast();
    unlock();
}

pub unsafe fn leave_native() {
    lock();
    let me = find_me();
    if let Some(i) = me {
        // Refresh sp and registers: the caller's frames may have moved.
        record_stop(i, NATIVE);
    }
    wait_for_world(me);
    unlock();
}

pub unsafe fn attach() {
    lock();
    if find_me().is_none() {
        let slot = (0..MAX_THREADS).find(|&i| !(*thr(i)).used);
        let Some(i) = slot else {
            Msg::new().s(b"gc: more than ").dec(MAX_THREADS).s(b" attached threads").die();
        };
        let mut attr = PthreadAttr([0; 8]);
        if sys::pthread_getattr_np(sys::pthread_self(), &mut attr) != 0 {
            sys::fatal(b"gc: pthread_getattr_np failed\0");
        }
        let mut addr: *mut core::ffi::c_void = core::ptr::null_mut();
        let mut size: usize = 0;
        sys::pthread_attr_getstack(&attr, &mut addr, &mut size);
        sys::pthread_attr_destroy(&mut attr);
        *thr(i) = ThreadRec {
            used: true,
            state: RUNNING,
            tid: sys::pthread_self() as u64,
            hi: addr as usize + size,
            sp: 0,
            regs: [0; 6],
        };
        sys::pthread_setspecific(st().tls_key, (i + 1) as *const core::ffi::c_void);
        if CACHE {
            tls::set(i + 1);
        }
    }
    unlock();
}

pub unsafe fn detach() {
    lock();
    if let Some(i) = find_me() {
        while REQUEST.load(Acquire) != 0 {
            record_stop(i, PARKED);
            broadcast();
            wait();
        }
        (*thr(i)).used = false;
        sys::pthread_setspecific(st().tls_key, core::ptr::null());
        if CACHE {
            tls::set(0);
        }
        broadcast();
    }
    unlock();
}

/// The first attached thread other than `me` that is still running, if any.
unsafe fn first_running(me: usize) -> Option<usize> {
    (0..MAX_THREADS).find(|&i| i != me && (*thr(i)).used && (*thr(i)).state == RUNNING)
}

/// Every other attached thread must be parked or in native state, with a
/// recorded stack pointer inside its stack, before any marking starts.
unsafe fn check_stopped(me: usize) {
    for i in 0..MAX_THREADS {
        let t = thr(i);
        if i == me || !(*t).used {
            continue;
        }
        if (*t).state == RUNNING || (*t).sp == 0 || (*t).sp > (*t).hi {
            Msg::new()
                .s(b"gc: attached thread ")
                .hex((*t).tid as usize)
                .s(b" is not stopped at a safepoint at mark time")
                .die();
        }
    }
}

/// Run a full stop-the-world cycle. Returns true if this call collected; false
/// if another thread's cycle ran instead (the caller parked for it).
pub unsafe fn collect() -> bool {
    lock();
    if st().collecting {
        let me = find_me();
        if let Some(i) = me {
            record_stop(i, PARKED);
        }
        broadcast();
        wait_for_world(me);
        unlock();
        return false;
    }
    st().collecting = true;
    REQUEST.store(1, Release);
    let Some(me) = find_me() else {
        sys::fatal(b"gc: collecting thread is not attached (call kelp_gc_thread_attach)\0");
    };
    let timeout = st().stop_timeout_ms;
    let deadline = sys::deadline_after_ms(timeout);
    while let Some(i) = first_running(me) {
        let rc = sys::pthread_cond_timedwait(G_COND.get(), G_MUTEX.get(), &deadline);
        if rc == sys::ETIMEDOUT && first_running(me).is_some() {
            let j = first_running(me).unwrap_or(i);
            Msg::new()
                .s(b"gc: thread ")
                .hex((*thr(j)).tid as usize)
                .s(b" did not reach a safepoint within ")
                .dec(timeout)
                .s(b" ms (registry slot ")
                .dec(j)
                .s(b"); a loop must poll kelp_gc_safepoint or enter native state")
                .die();
        }
    }
    check_stopped(me);
    record_stop(me, RUNNING);
    let t0 = sys::now_ns();
    mark::collect_locked();
    let pause = sys::now_ns().saturating_sub(t0);
    st().pause_total_ns += pause;
    st().pause_max_ns = core::cmp::max(st().pause_max_ns, pause);
    st().collecting = false;
    REQUEST.store(0, Release);
    broadcast();
    unlock();
    true
}

/// Mark roots: every attached thread's stack and registers, and every root region.
pub unsafe fn scan_roots() {
    for i in 0..MAX_THREADS {
        let t = thr(i);
        if !(*t).used || !st().scan_stacks {
            continue;
        }
        st().root_bytes += (*t).hi.saturating_sub((*t).sp);
        mark::scan_range_conservative((*t).sp, (*t).hi);
        for r in (*t).regs {
            mark::mark_word(r);
        }
    }
    // Only the slots up to the high-water index can be in use (section 3), and
    // the used-slot bitmap skips the cleared ones 64 at a time (section 20.7).
    let t = ROOTS.get();
    let words = ((*t).hw + 63) / 64;
    let bits = (*t).used_bits as *const u64;
    for w in 0..words {
        let mut m = *bits.add(w);
        while m != 0 {
            let i = w * 64 + m.trailing_zeros() as usize;
            m &= m - 1;
            let r = rec(i);
            if (*r).used {
                let d = (*r).layout as *const KelpGcType;
                st().root_bytes += (*d).size as usize;
                mark::scan_root_value((*r).base, d);
            }
        }
    }
}

/// Register `base` as a root holding a value described by `layout`. Takes a
/// slot from the free list, or the next unused one, growing the table when it
/// is full. Returns the handle, or `RootError::Full` with nothing registered.
pub unsafe fn register_root(base: usize, layout: usize) -> Result<RootHandle, RootError> {
    lock();
    let r = register_locked(base, layout);
    unlock();
    r
}

unsafe fn register_locked(base: usize, layout: usize) -> Result<RootHandle, RootError> {
    let t = ROOTS.get();
    let slot = if (*t).free_head != 0 {
        let i = ((*t).free_head - 1) as usize;
        (*t).free_head = (*rec(i)).next_free;
        i
    } else {
        if (*t).hw == (*t).cap {
            let next = if (*t).cap == 0 { ROOT_CHUNK } else { (*t).cap * 2 };
            grow_roots(next)?;
        }
        let i = (*t).hw;
        (*t).hw += 1;
        i
    };
    let slot_gen = (*rec(slot)).generation;
    *rec(slot) = RootRec { used: true, generation: slot_gen, next_free: 0, base, layout };
    set_used_bit(slot, true);
    (*t).live += 1;
    Ok(((slot_gen as u64) << 32) | (slot as u64 + 1))
}

/// Set or clear slot `i`'s bit in the used-slot bitmap. Registry lock held.
#[inline]
unsafe fn set_used_bit(i: usize, on: bool) {
    let w = ((*ROOTS.get()).used_bits as *mut u64).add(i / 64);
    let bit = 1u64 << (i % 64);
    if on {
        *w |= bit;
    } else {
        *w &= !bit;
    }
}

/// Clear the entry `h` names. A handle whose generation does not match (the
/// entry was cleared, or the slot was reused) does nothing and returns false.
pub unsafe fn unregister_root(h: RootHandle) -> bool {
    lock();
    let ok = unregister_locked(h);
    unlock();
    ok
}

unsafe fn unregister_locked(h: RootHandle) -> bool {
    let t = ROOTS.get();
    let slot_plus = (h & 0xFFFF_FFFF) as usize;
    let want_gen = (h >> 32) as u32;
    if slot_plus == 0 || slot_plus > (*t).hw {
        return false;
    }
    let i = slot_plus - 1;
    let r = rec(i);
    if !(*r).used || (*r).generation != want_gen {
        return false;
    }
    (*r).used = false;
    set_used_bit(i, false);
    (*r).generation = want_gen.wrapping_add(1);
    (*r).base = 0;
    (*r).layout = 0;
    (*r).next_free = (*t).free_head;
    (*t).free_head = (i + 1) as u32;
    (*t).live -= 1;
    true
}

/// Live entries and the high-water index, for the stats hook.
pub unsafe fn root_counts() -> (usize, usize) {
    let t = ROOTS.get();
    ((*t).live, (*t).hw)
}

/// Empty the table without unmapping it (tests reset the heap between cases).
pub unsafe fn reset_roots() {
    let t = ROOTS.get();
    for i in 0..(*t).hw {
        *rec(i) = RootRec { used: false, generation: 0, next_free: 0, base: 0, layout: 0 };
    }
    for w in 0..((*t).hw + 63) / 64 {
        *((*t).used_bits as *mut u64).add(w) = 0;
    }
    (*t).hw = 0;
    (*t).free_head = 0;
    (*t).live = 0;
}

/// The registry lock, for the stats hook (which reads several counters at once).
pub unsafe fn lock_for_stats() {
    lock();
}
pub unsafe fn unlock_for_stats() {
    unlock();
}

pub unsafe fn init_locks() {
    sys::pthread_mutex_init(G_MUTEX.get(), core::ptr::null());
    sys::pthread_cond_init(G_COND.get(), core::ptr::null());
    sys::pthread_key_create(&mut st().tls_key, core::ptr::null());
}

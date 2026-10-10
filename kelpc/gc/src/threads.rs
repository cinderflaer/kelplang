//! Thread registry, safepoints, native state, root regions, and the
//! stop-the-world handshake. All state here is guarded by G_MUTEX.
//!
//! Each attached thread's record is found through a pthread key that holds
//! its registry index, so the allocator and the safepoint poll never scan the
//! table. The record is trusted only if its thread id matches the caller.
#![allow(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::Ordering::{Acquire, Release};

use crate::mark;
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

#[derive(Clone, Copy)]
pub struct RootRec {
    pub used: bool,
    pub base: usize,
    pub layout: usize,
}

pub static THREADS: Global<[ThreadRec; MAX_THREADS]> = Global(core::cell::UnsafeCell::new(unsafe { core::mem::zeroed() }));
pub static ROOTS: Global<[RootRec; MAX_ROOTS]> = Global(core::cell::UnsafeCell::new(unsafe { core::mem::zeroed() }));

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

/// Registry index of the calling thread, or None if it is not attached.
/// Lock-free: the record is written only by its own thread (attach, detach)
/// or by reset, and the thread id check rejects a stale key.
pub unsafe fn my_index() -> Option<usize> {
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
    let dt = sys::now_ns() - t0;
    st().pause_ns_total += dt;
    st().pause_ns_max = core::cmp::max(st().pause_ns_max, dt);
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
        mark::scan_range_conservative((*t).sp, (*t).hi);
        for r in (*t).regs {
            mark::mark_word(r);
        }
    }
    for i in 0..MAX_ROOTS {
        let r = (ROOTS.get() as *mut RootRec).add(i);
        if (*r).used {
            let d = (*r).layout as *const KelpGcType;
            mark::scan_root_value((*r).base, d);
        }
    }
}

pub unsafe fn register_root(base: usize, layout: usize) {
    lock();
    let slot = (0..MAX_ROOTS).find(|&i| !(*(ROOTS.get() as *mut RootRec).add(i)).used);
    match slot {
        Some(i) => *(ROOTS.get() as *mut RootRec).add(i) = RootRec { used: true, base, layout },
        None => sys::fatal(b"gc: too many root regions\0"),
    }
    unlock();
}

pub unsafe fn unregister_root(base: usize) {
    lock();
    for i in 0..MAX_ROOTS {
        let r = (ROOTS.get() as *mut RootRec).add(i);
        if (*r).used && (*r).base == base {
            (*r).used = false;
        }
    }
    unlock();
}

pub unsafe fn init_locks() {
    sys::pthread_mutex_init(G_MUTEX.get(), core::ptr::null());
    sys::pthread_cond_init(G_COND.get(), core::ptr::null());
    sys::pthread_key_create(&mut st().tls_key, core::ptr::null());
}

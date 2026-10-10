//! Port of kelp_rt.c:1124-1676: the worker threads (`thread.spawn` and
//! `thread.join`), the builtin mutex, reader/writer lock and condition
//! variable, and the timed `wait_while`.
//!
//! The worker handle is [`crate::panic::WorkerHandle`], the record the panic
//! path already writes, so a panic in a worker reaches its joiner through the
//! same fields the C runtime used. Every `pthread_cleanup_push` of the C text
//! is a [`crate::cleanup`] frame: the handler runs on a normal pop when the
//! C code popped with `execute` set, and on the `pthread_exit` a worker's
//! panic takes.
//!
//! The body of a spawn, a mutex or a lock is a generated function with the
//! signature `void (*)(void* code, void* env, void* arg, void* out)`, which is
//! [`Body`] here.
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::cleanup::{CleanupFrame, cleanup_pop, cleanup_push};
use crate::panic::{WorkerHandle, kelp_panic, set_worker_handle};

/// `void (*)(void* code, void* env, void* arg, void* out)`: the generated
/// per-signature body the runtime calls.
pub type Body = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void);
/// The element's own drop, or none when the element owns nothing.
pub type Drop1 = unsafe extern "C" fn(*mut c_void);

/// A spawned worker's argument: its handle, the body to call and the closure's
/// code and environment, with the environment's drop.
#[repr(C)]
pub struct ThreadStart {
    handle: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
}

/// The cleanup handler a worker runs on both ways out: frees the record and
/// the closure's environment, then the handle if nobody joined it.
unsafe extern "C" fn thread_release(raw: *mut c_void) {
    unsafe {
        let s = raw.cast::<ThreadStart>();
        let h = (*s).handle;
        if !(*s).env.is_null() {
            if let Some(drop_env) = (*s).env_drop {
                drop_env((*s).env);
            }
        }
        libc::free(s.cast());
        // The worker's last touch of the handle. If the owner already dropped
        // it unjoined, nobody else will free it.
        if state(h).swap(2, Ordering::AcqRel) == 1 {
            thread_dispose(h);
        }
    }
}

fn state(h: *mut WorkerHandle) -> &'static AtomicI32 {
    unsafe { AtomicI32::from_ptr(&raw mut (*h).state) }
}

/// The entry point `pthread_create` is given. `libc` takes a safe function
/// pointer, so the unsafe trampoline is called from here.
extern "C" fn tramp_entry(raw: *mut c_void) -> *mut c_void {
    unsafe { thread_tramp(raw) }
}

/// The trampoline a worker thread starts in.
unsafe fn thread_tramp(raw: *mut c_void) -> *mut c_void {
    unsafe {
        let s = raw.cast::<ThreadStart>();
        set_worker_handle((*s).handle);
        let mut frame = CleanupFrame {
            run: thread_release,
            arg: raw,
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        let body = (*s).body.expect("thread.spawn: no body");
        body((*s).code, (*s).env, (*(*s).handle).arg, (*(*s).handle).result);
        cleanup_pop(&raw mut frame, true);
        ptr::null_mut()
    }
}

/// `thread.spawn`'s handle: a worker record with its result and argument slots.
///
/// # Safety
/// The result is owned by the caller until it is started or dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_new(arg_size: usize, result_size: usize) -> *mut WorkerHandle {
    unsafe {
        let h = libc::malloc(core::mem::size_of::<WorkerHandle>()).cast::<WorkerHandle>();
        if h.is_null() {
            kelp_panic(c"thread.spawn: out of memory".as_ptr());
        }
        (*h).panicked = 0;
        (*h).message = ptr::null();
        (*h).state = 0;
        (*h).result = libc::malloc(if result_size == 0 { 1 } else { result_size });
        (*h).arg = libc::malloc(if arg_size == 0 { 1 } else { arg_size });
        if (*h).result.is_null() || (*h).arg.is_null() {
            kelp_panic(c"thread.spawn: out of memory".as_ptr());
        }
        h
    }
}

/// Starts the worker on `h`.
///
/// # Safety
/// `h` must come from [`kelp_thread_new`] and not be started yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_start(
    h: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
) {
    unsafe {
        let s = libc::malloc(core::mem::size_of::<ThreadStart>()).cast::<ThreadStart>();
        if s.is_null() {
            kelp_panic(c"thread.spawn: out of memory".as_ptr());
        }
        s.write(ThreadStart {
            handle: h,
            body,
            code,
            env,
            env_drop,
        });
        let rc = libc::pthread_create(&raw mut (*h).tid, ptr::null(), tramp_entry, s.cast());
        if rc != 0 {
            kelp_panic(c"thread.spawn: pthread_create failed".as_ptr());
        }
    }
}

/// Waits for the worker and reports whether it panicked.
///
/// # Safety
/// `h` must be a started handle not yet joined.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_wait(h: *mut WorkerHandle) -> c_int {
    unsafe {
        libc::pthread_join((*h).tid, ptr::null_mut());
        (*h).panicked
    }
}

/// The argument slot of a handle.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_arg(h: *mut WorkerHandle) -> *mut c_void {
    unsafe { (*h).arg }
}

/// The result slot of a handle.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_result(h: *mut WorkerHandle) -> *mut c_void {
    unsafe { (*h).result }
}

/// The message a panicking worker died with.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_message(h: *mut WorkerHandle) -> *const c_char {
    unsafe { (*h).message }
}

/// Frees a joined handle's three allocations.
///
/// # Safety
/// The worker must have ended; `h` must not be used afterwards.
unsafe fn thread_dispose(h: *mut WorkerHandle) {
    unsafe {
        libc::free((*h).arg);
        libc::free((*h).result);
        libc::free(h.cast());
    }
}

/// `thread.join`'s drop for a handle nobody joined: detaches the worker and
/// hands it the handle, or frees the handle when the worker already ended.
///
/// # Safety
/// `slot` must point at a handle slot the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_drop(slot: *mut *mut WorkerHandle) {
    unsafe {
        let h = *slot;
        if h.is_null() {
            return;
        }
        libc::pthread_detach((*h).tid);
        *slot = ptr::null_mut();
        if state(h).swap(1, Ordering::AcqRel) == 2 {
            thread_dispose(h);
        }
    }
}

// ---- Mutex ----------------------------------------------------------------

/// The builtin mutex: the pthread lock, the poison flag, a count of the
/// handles that share it, the guarded value and its element's drop.
#[repr(C)]
pub struct Mutex {
    lock: libc::pthread_mutex_t,
    poisoned: c_int,
    refs: c_int,
    size: usize,
    value: *mut c_void,
    drop: Option<Drop1>,
}

/// # Safety
/// `value` must be readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_new(value: *const c_void, size: usize, drop: Option<Drop1>) -> *mut Mutex {
    unsafe {
        let m = libc::malloc(core::mem::size_of::<Mutex>()).cast::<Mutex>();
        if m.is_null() {
            kelp_panic(c"Mutex.new: out of memory".as_ptr());
        }
        if libc::pthread_mutex_init(&raw mut (*m).lock, ptr::null()) != 0 {
            kelp_panic(c"Mutex.new: pthread_mutex_init failed".as_ptr());
        }
        (*m).poisoned = 0;
        (*m).refs = 1;
        (*m).size = size;
        (*m).drop = drop;
        (*m).value = libc::malloc(if size == 0 { 1 } else { size });
        if (*m).value.is_null() {
            kelp_panic(c"Mutex.new: out of memory".as_ptr());
        }
        libc::memcpy((*m).value, value, size);
        m
    }
}

/// Runs while the lock is held: unlocks and poisons, on a panic's cleanup.
unsafe extern "C" fn mutex_guard_release(raw: *mut c_void) {
    unsafe {
        let m = raw.cast::<Mutex>();
        libc::pthread_mutex_unlock(&raw mut (*m).lock);
        (*m).poisoned = 1;
    }
}

/// Runs `body` on the guarded value with the lock held.
///
/// # Safety
/// `m` must be a live mutex.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_run(m: *mut Mutex, body: Option<Body>, code: *mut c_void, env: *mut c_void, out: *mut c_void) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*m).lock);
        let mut frame = CleanupFrame {
            run: mutex_guard_release,
            arg: m.cast(),
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        if (*m).poisoned != 0 {
            kelp_panic(c"mutex poisoned".as_ptr());
        }
        let mut value_slot = (*m).value;
        let body = body.expect("Mutex.run: no body");
        body(code, env, (&raw mut value_slot).cast(), out);
        cleanup_pop(&raw mut frame, false);
        libc::pthread_mutex_unlock(&raw mut (*m).lock);
    }
}

/// `m.share()`: another handle to the same record.
///
/// # Safety
/// `m` must be a live mutex.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_share(m: *mut Mutex) -> *mut Mutex {
    unsafe {
        atomic_i32((&raw mut (*m).refs)).fetch_add(1, Ordering::SeqCst);
        m
    }
}

fn atomic_i32<'a>(p: *mut c_int) -> &'a AtomicI32 {
    unsafe { AtomicI32::from_ptr(p) }
}

/// `m.swap(value)`: replaces the guarded value and hands the old one back.
///
/// # Safety
/// `input` must be readable and `out` writable for the mutex's size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_swap(m: *mut Mutex, input: *const c_void, out: *mut c_void) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*m).lock);
        if (*m).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*m).lock);
            kelp_panic(c"mutex poisoned".as_ptr());
        }
        libc::memcpy(out, (*m).value, (*m).size);
        libc::memcpy((*m).value, input, (*m).size);
        libc::pthread_mutex_unlock(&raw mut (*m).lock);
    }
}

/// `Mutex.into_inner`: copies the value out and frees the record.
///
/// # Safety
/// `m` must be a live mutex with no other handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_into(m: *mut Mutex, out: *mut c_void) {
    unsafe {
        if atomic_i32(&raw mut (*m).refs).load(Ordering::SeqCst) != 1 {
            kelp_panic(c"Mutex.into_inner: the mutex is shared with another handle".as_ptr());
        }
        libc::memcpy(out, (*m).value, (*m).size);
        libc::pthread_mutex_destroy(&raw mut (*m).lock);
        libc::free((*m).value);
        libc::free(m.cast());
    }
}

/// The drop of a mutex: the last handle runs the element's drop and frees
/// the record.
///
/// # Safety
/// `slot` must point at a mutex slot the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_drop(slot: *mut *mut Mutex) {
    unsafe {
        let m = *slot;
        if m.is_null() {
            return;
        }
        *slot = ptr::null_mut();
        if atomic_i32(&raw mut (*m).refs).fetch_sub(1, Ordering::SeqCst) - 1 != 0 {
            return;
        }
        if let Some(drop_value) = (*m).drop {
            drop_value((*m).value);
        }
        libc::pthread_mutex_destroy(&raw mut (*m).lock);
        libc::free((*m).value);
        libc::free(m.cast());
        *slot = ptr::null_mut();
    }
}

// ---- RwLock ---------------------------------------------------------------

/// The builtin reader/writer lock: writer-preferring, with a poison flag.
#[repr(C)]
pub struct RwLock {
    lock: libc::pthread_mutex_t,
    readers_ok: libc::pthread_cond_t,
    writers_ok: libc::pthread_cond_t,
    readers: c_int,
    writer: c_int,
    writers_waiting: c_int,
    poisoned: c_int,
    size: usize,
    value: *mut c_void,
    drop: Option<Drop1>,
}

/// # Safety
/// `value` must be readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_new(value: *const c_void, size: usize, drop: Option<Drop1>) -> *mut RwLock {
    unsafe {
        let rw = libc::malloc(core::mem::size_of::<RwLock>()).cast::<RwLock>();
        if rw.is_null() {
            kelp_panic(c"RwLock.new: out of memory".as_ptr());
        }
        if libc::pthread_mutex_init(&raw mut (*rw).lock, ptr::null()) != 0
            || libc::pthread_cond_init(&raw mut (*rw).readers_ok, ptr::null()) != 0
            || libc::pthread_cond_init(&raw mut (*rw).writers_ok, ptr::null()) != 0
        {
            kelp_panic(c"RwLock.new: pthread init failed".as_ptr());
        }
        (*rw).readers = 0;
        (*rw).writer = 0;
        (*rw).writers_waiting = 0;
        (*rw).poisoned = 0;
        (*rw).size = size;
        (*rw).drop = drop;
        (*rw).value = libc::malloc(if size == 0 { 1 } else { size });
        if (*rw).value.is_null() {
            kelp_panic(c"RwLock.new: out of memory".as_ptr());
        }
        libc::memcpy((*rw).value, value, size);
        rw
    }
}

/// The read side's release, on both exits. A reader never poisons.
unsafe extern "C" fn rwlock_read_end(raw: *mut c_void) {
    unsafe {
        let rw = raw.cast::<RwLock>();
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        (*rw).readers -= 1;
        if (*rw).readers == 0 {
            libc::pthread_cond_signal(&raw mut (*rw).writers_ok);
        }
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
    }
}

/// The writer's normal release.
unsafe extern "C" fn rwlock_write_end(raw: *mut c_void) {
    unsafe {
        let rw = raw.cast::<RwLock>();
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        (*rw).writer = 0;
        libc::pthread_cond_signal(&raw mut (*rw).writers_ok);
        libc::pthread_cond_broadcast(&raw mut (*rw).readers_ok);
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
    }
}

/// The writer's panic release: unlocks and poisons.
unsafe extern "C" fn rwlock_write_poison(raw: *mut c_void) {
    unsafe {
        let rw = raw.cast::<RwLock>();
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        (*rw).poisoned = 1;
        (*rw).writer = 0;
        libc::pthread_cond_signal(&raw mut (*rw).writers_ok);
        libc::pthread_cond_broadcast(&raw mut (*rw).readers_ok);
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
    }
}

/// Waits for a read slot, then runs `body` with the state lock released.
unsafe fn rwlock_read_locked(rw: *mut RwLock, body: Body, code: *mut c_void, env: *mut c_void, out: *mut c_void) {
    unsafe {
        let mut frame = CleanupFrame {
            run: rwlock_read_end,
            arg: rw.cast(),
            prev: ptr::null_mut(),
        };
        let mut value_slot = (*rw).value;
        cleanup_push(&raw mut frame);
        body(code, env, (&raw mut value_slot).cast(), out);
        cleanup_pop(&raw mut frame, true);
    }
}

/// # Safety
/// `rw` must be a live lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_read(rw: *mut RwLock, body: Option<Body>, code: *mut c_void, env: *mut c_void, out: *mut c_void) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        while (*rw).writer != 0 || (*rw).writers_waiting > 0 {
            libc::pthread_cond_wait(&raw mut (*rw).readers_ok, &raw mut (*rw).lock);
        }
        (*rw).readers += 1;
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
        rwlock_read_locked(rw, body.expect("RwLock.read: no body"), code, env, out);
    }
}

/// # Safety
/// `rw` must be a live lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_write(rw: *mut RwLock, body: Option<Body>, code: *mut c_void, env: *mut c_void, out: *mut c_void) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        (*rw).writers_waiting += 1;
        while (*rw).writer != 0 || (*rw).readers > 0 {
            libc::pthread_cond_wait(&raw mut (*rw).writers_ok, &raw mut (*rw).lock);
        }
        (*rw).writers_waiting -= 1;
        (*rw).writer = 1;
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
        let mut value_slot = (*rw).value;
        let mut frame = CleanupFrame {
            run: rwlock_write_poison,
            arg: rw.cast(),
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        let body = body.expect("RwLock.write: no body");
        body(code, env, (&raw mut value_slot).cast(), out);
        cleanup_pop(&raw mut frame, false);
        rwlock_write_end(rw.cast());
    }
}

/// # Safety
/// `rw` must be a live lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_try_read(rw: *mut RwLock, body: Option<Body>, code: *mut c_void, env: *mut c_void, out: *mut c_void) -> c_int {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        if (*rw).writer != 0 || (*rw).writers_waiting > 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            return 0;
        }
        (*rw).readers += 1;
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
        rwlock_read_locked(rw, body.expect("RwLock.try_read: no body"), code, env, out);
        1
    }
}

/// # Safety
/// `rw` must be a live lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_try_write(rw: *mut RwLock, body: Option<Body>, code: *mut c_void, env: *mut c_void, out: *mut c_void) -> c_int {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        if (*rw).writer != 0 || (*rw).readers > 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            return 0;
        }
        (*rw).writer = 1;
        libc::pthread_mutex_unlock(&raw mut (*rw).lock);
        let mut value_slot = (*rw).value;
        let mut frame = CleanupFrame {
            run: rwlock_write_poison,
            arg: rw.cast(),
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        let body = body.expect("RwLock.try_write: no body");
        body(code, env, (&raw mut value_slot).cast(), out);
        cleanup_pop(&raw mut frame, false);
        rwlock_write_end(rw.cast());
        1
    }
}

/// `RwLock.into_inner`: copies the value out and frees the record.
///
/// # Safety
/// `rw` must be a live lock with no reader or writer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_into(rw: *mut RwLock, out: *mut c_void) {
    unsafe {
        libc::memcpy(out, (*rw).value, (*rw).size);
        libc::pthread_mutex_destroy(&raw mut (*rw).lock);
        libc::pthread_cond_destroy(&raw mut (*rw).readers_ok);
        libc::pthread_cond_destroy(&raw mut (*rw).writers_ok);
        libc::free((*rw).value);
        libc::free(rw.cast());
    }
}

/// The drop of a reader/writer lock: runs the element's drop, then frees.
///
/// # Safety
/// `slot` must point at a lock slot the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_drop(slot: *mut *mut RwLock) {
    unsafe {
        let rw = *slot;
        if rw.is_null() {
            return;
        }
        if let Some(drop_value) = (*rw).drop {
            drop_value((*rw).value);
        }
        libc::pthread_mutex_destroy(&raw mut (*rw).lock);
        libc::pthread_cond_destroy(&raw mut (*rw).readers_ok);
        libc::pthread_cond_destroy(&raw mut (*rw).writers_ok);
        libc::free((*rw).value);
        libc::free(rw.cast());
        *slot = ptr::null_mut();
    }
}

// ---- Condvar --------------------------------------------------------------

/// The builtin condition variable, with the count of handles that share it.
#[repr(C)]
pub struct Condvar {
    cond: libc::pthread_cond_t,
    refs: c_int,
}

/// # Safety
/// The result is owned by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_condvar_new() -> *mut Condvar {
    unsafe {
        let cv = libc::malloc(core::mem::size_of::<Condvar>()).cast::<Condvar>();
        if cv.is_null() {
            kelp_panic(c"Condvar.new: out of memory".as_ptr());
        }
        if libc::pthread_cond_init(&raw mut (*cv).cond, ptr::null()) != 0 {
            kelp_panic(c"Condvar.new: pthread_cond_init failed".as_ptr());
        }
        (*cv).refs = 1;
        cv
    }
}

/// `cv.share()`.
///
/// # Safety
/// `cv` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_condvar_share(cv: *mut Condvar) -> *mut Condvar {
    unsafe {
        atomic_i32(&raw mut (*cv).refs).fetch_add(1, Ordering::SeqCst);
        cv
    }
}

/// `cv.notify_one()`.
///
/// # Safety
/// `cv` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_condvar_notify_one(cv: *mut Condvar) {
    unsafe {
        libc::pthread_cond_signal(&raw mut (*cv).cond);
    }
}

/// `cv.notify_all()`.
///
/// # Safety
/// `cv` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_condvar_notify_all(cv: *mut Condvar) {
    unsafe {
        libc::pthread_cond_broadcast(&raw mut (*cv).cond);
    }
}

/// The drop of a condition variable: the last handle frees it.
///
/// # Safety
/// `slot` must point at a condvar slot the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_condvar_drop(slot: *mut *mut Condvar) {
    unsafe {
        let cv = *slot;
        if cv.is_null() {
            return;
        }
        *slot = ptr::null_mut();
        if atomic_i32(&raw mut (*cv).refs).fetch_sub(1, Ordering::SeqCst) - 1 != 0 {
            return;
        }
        libc::pthread_cond_destroy(&raw mut (*cv).cond);
        libc::free(cv.cast());
        *slot = ptr::null_mut();
    }
}

/// `m.wait_while(cv, pred)`: waits while `pred` keeps the value, then runs
/// `body` with the lock still held.
///
/// # Safety
/// All handles must be live; `pred` and `body` are generated functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_wait_while(
    m: *mut Mutex,
    cv: *mut Condvar,
    pred: Option<Body>,
    pcode: *mut c_void,
    penv: *mut c_void,
    pred_by_ref: c_int,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    out: *mut c_void,
) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*m).lock);
        let mut frame = CleanupFrame {
            run: mutex_guard_release,
            arg: m.cast(),
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        if (*m).poisoned != 0 {
            kelp_panic(c"mutex poisoned".as_ptr());
        }
        let mut value_slot = (*m).value;
        let pred = pred.expect("Mutex.wait_while: no predicate");
        let pred_arg = if pred_by_ref != 0 { (&raw mut value_slot).cast() } else { value_slot };
        loop {
            let mut keep: u8 = 0;
            pred(pcode, penv, pred_arg, (&raw mut keep).cast());
            if keep == 0 {
                break;
            }
            libc::pthread_cond_wait(&raw mut (*cv).cond, &raw mut (*m).lock);
        }
        let body = body.expect("Mutex.wait_while: no body");
        body(code, env, (&raw mut value_slot).cast(), out);
        cleanup_pop(&raw mut frame, false);
        libc::pthread_mutex_unlock(&raw mut (*m).lock);
    }
}

/// The timed `wait_while`: one absolute deadline, and a last predicate test so
/// a notify that raced the deadline still counts. Returns 1 when `body` ran.
///
/// # Safety
/// As [`kelp_mutex_wait_while`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn kelp_mutex_wait_while_timeout(
    m: *mut Mutex,
    cv: *mut Condvar,
    pred: Option<Body>,
    pcode: *mut c_void,
    penv: *mut c_void,
    pred_by_ref: c_int,
    millis: c_long,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    out: *mut c_void,
) -> c_int {
    unsafe {
        libc::pthread_mutex_lock(&raw mut (*m).lock);
        let mut timed_out = false;
        let mut frame = CleanupFrame {
            run: mutex_guard_release,
            arg: m.cast(),
            prev: ptr::null_mut(),
        };
        cleanup_push(&raw mut frame);
        if (*m).poisoned != 0 {
            kelp_panic(c"mutex poisoned".as_ptr());
        }
        let mut deadline: libc::timespec = core::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut deadline);
        let millis = if millis < 0 { 0 } else { millis };
        deadline.tv_sec += (millis / 1000) as libc::time_t;
        deadline.tv_nsec += ((millis % 1000) * 1_000_000) as libc::c_long;
        if deadline.tv_nsec >= 1_000_000_000 {
            deadline.tv_sec += 1;
            deadline.tv_nsec -= 1_000_000_000;
        }
        let mut value_slot = (*m).value;
        let pred = pred.expect("Mutex.wait_while_timeout: no predicate");
        let pred_arg = if pred_by_ref != 0 { (&raw mut value_slot).cast() } else { value_slot };
        loop {
            let mut keep: u8 = 0;
            pred(pcode, penv, pred_arg, (&raw mut keep).cast());
            if keep == 0 {
                break;
            }
            if libc::pthread_cond_timedwait(&raw mut (*cv).cond, &raw mut (*m).lock, &raw const deadline)
                == libc::ETIMEDOUT
            {
                let mut keep2: u8 = 0;
                pred(pcode, penv, pred_arg, (&raw mut keep2).cast());
                if keep2 != 0 {
                    timed_out = true;
                }
                break;
            }
        }
        if !timed_out {
            let body = body.expect("Mutex.wait_while_timeout: no body");
            body(code, env, (&raw mut value_slot).cast(), out);
        }
        cleanup_pop(&raw mut frame, false);
        libc::pthread_mutex_unlock(&raw mut (*m).lock);
        if timed_out { 0 } else { 1 }
    }
}

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
use crate::gc;
use crate::panic::{WorkerHandle, kelp_panic, kelp_panic_at, set_worker_handle};

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
    /// Root of `env`, the record's own pointer slot (0 when not rooted).
    env_root: u64,
    /// The argument's own drop, used only when the spawn never starts. A
    /// started worker's body owns its argument (its generated function drops it
    /// on every exit), so the runtime never drops it then.
    arg_drop: Option<Drop1>,
}

/// Spawns still open at once. The collector attaches at most 256 threads, so
/// the spawner refuses the 257th (section 18.8 of the Tidepool design).
#[cfg(feature = "managed")]
const OPEN_WORKERS_MAX: c_int = 256;
#[cfg(feature = "managed")]
static OPEN_WORKERS: AtomicI32 = AtomicI32::new(0);

/// Takes one open-worker slot, or returns false when all are taken.
fn open_workers_take() -> bool {
    #[cfg(feature = "managed")]
    {
        if OPEN_WORKERS.fetch_add(1, Ordering::AcqRel) >= OPEN_WORKERS_MAX {
            OPEN_WORKERS.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
    }
    true
}

/// Gives back a slot taken by [`open_workers_take`].
fn open_workers_give() {
    #[cfg(feature = "managed")]
    {
        OPEN_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The descriptor of one pointer word: a spawn's environment is a collected
/// object, so marking the word marks it, and the object's own descriptor scans
/// its captures.
#[cfg(feature = "managed")]
static WORD_OFF: [u32; 1] = [0];
#[cfg(feature = "managed")]
static WORD_DESCR: crate::gc::GcType = crate::gc::GcType {
    id: 0,
    size: core::mem::size_of::<*mut c_void>() as u32,
    align: core::mem::size_of::<*mut c_void>() as u32,
    flags: 0,
    n_ptrs: 1,
    ptr_off: core::ptr::addr_of!(WORD_OFF).cast(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// The descriptor a spawn's environment is rooted with (none when hosted).
#[cfg(feature = "managed")]
fn word_descr() -> *const c_void {
    (&raw const WORD_DESCR).cast()
}

/// A hosted spawn roots nothing.
#[cfg(not(feature = "managed"))]
fn word_descr() -> *const c_void {
    ptr::null()
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
        // The environment stays rooted through its drop, which may read it.
        gc::root_drop((*s).env_root);
        // Leave the collector on every exit, after the environment's drop (it
        // may read collected values) and before the record is freed. A panicking
        // worker never reaches the end of its body, so this is the one place
        // that detaches it: a worker left attached holds a collection up until
        // the stop timeout.
        gc::thread_detach();
        open_workers_give();
        libc::free(s.cast());
        // The worker's last touch of the handle. If the owner already dropped
        // it unjoined, nobody else will free it.
        if state(h).swap(2, Ordering::AcqRel) == 1 {
            kelp_thread_dispose(h);
        }
    }
}

fn state(h: *mut WorkerHandle) -> &'static AtomicI32 {
    unsafe { AtomicI32::from_ptr(&raw mut (*h).state) }
}

/// `pthread_mutex_lock` as a blocking native call: the thread is marked native
/// around the lock only, so a collection on another thread does not wait for
/// it. Nothing that can panic runs inside the bracket.
unsafe fn blk_lock(m: *mut libc::pthread_mutex_t) -> c_int {
    gc::native_enter();
    let rc = unsafe { libc::pthread_mutex_lock(m) };
    gc::native_leave();
    rc
}

/// `pthread_cond_wait` as a blocking native call (see [`blk_lock`]).
unsafe fn blk_wait(c: *mut libc::pthread_cond_t, m: *mut libc::pthread_mutex_t) -> c_int {
    gc::native_enter();
    let rc = unsafe { libc::pthread_cond_wait(c, m) };
    gc::native_leave();
    rc
}

/// `pthread_cond_timedwait` as a blocking native call (see [`blk_lock`]).
unsafe fn blk_timedwait(
    c: *mut libc::pthread_cond_t,
    m: *mut libc::pthread_mutex_t,
    deadline: *const libc::timespec,
) -> c_int {
    gc::native_enter();
    let rc = unsafe { libc::pthread_cond_timedwait(c, m, deadline) };
    gc::native_leave();
    rc
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
        // Held by the test hook until the spawner has run its clearing (see
        // `start_common`). Only a hold that was set is waited for.
        let hold = AtomicI32::from_ptr(&raw mut (*(*s).handle).hold);
        let gate = AtomicI32::from_ptr(&raw mut (*(*s).handle).gate);
        while hold.load(Ordering::Acquire) != 0 && gate.load(Ordering::Acquire) == 0 {
            libc::sched_yield();
        }
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
        (*h).arg_root = 0;
        (*h).result_root = 0;
        (*h).hold = 0;
        (*h).gate = 0;
        // Zeroed, so the collector never scans a stale word in a slot it roots.
        (*h).result = libc::calloc(1, if result_size == 0 { 1 } else { result_size });
        (*h).arg = libc::calloc(1, if arg_size == 0 { 1 } else { arg_size });
        if (*h).result.is_null() || (*h).arg.is_null() {
            kelp_panic(c"thread.spawn: out of memory".as_ptr());
        }
        h
    }
}

/// Starts the worker on `h` with no roots for its values (a hosted spawn).
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
    unsafe { start_common(h, body, code, env, env_drop, ptr::null(), ptr::null(), None) }
}

/// As [`kelp_thread_start`], and the argument's own drop for a spawn that never
/// starts (decision 125). Emitted only by a program that spawns.
///
/// # Safety
/// As [`kelp_thread_start`]; `arg_drop` must drop a value of the argument's type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_start_dropping(
    h: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
    arg_drop: Option<Drop1>,
) {
    unsafe { start_common(h, body, code, env, env_drop, ptr::null(), ptr::null(), arg_drop) }
}

/// Starts the worker on `h` and roots the spawn's values (a managed spawn):
/// the environment, the argument slot described by `arg_descr` and the result
/// slot described by `result_descr` (null when the value holds no collected
/// memory).
///
/// # Safety
/// As [`kelp_thread_start`]; the descriptors must describe the slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_start_rooted(
    h: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
    arg_descr: *const c_void,
    result_descr: *const c_void,
) {
    unsafe { start_common(h, body, code, env, env_drop, arg_descr, result_descr, None) }
}

/// As [`kelp_thread_start_rooted`], and the argument's own drop for a spawn that
/// never starts (decision 125). Emitted only by a program that spawns.
///
/// # Safety
/// As [`kelp_thread_start_rooted`]; `arg_drop` must drop a value of the argument's type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_start_rooted_dropping(
    h: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
    arg_descr: *const c_void,
    result_descr: *const c_void,
    arg_drop: Option<Drop1>,
) {
    unsafe { start_common(h, body, code, env, env_drop, arg_descr, result_descr, arg_drop) }
}

/// Frees what a spawn that did not start owns: its roots, the open-worker
/// slot, the start record and the handle's slots. The environment's drop runs
/// first, as it always did, then the argument's drop: the argument was never
/// handed to a body, so nothing else will drop it.
///
/// # Safety
/// `h` and `s` must be the handle and start record of a spawn not started.
unsafe fn spawn_abandon(h: *mut WorkerHandle, s: *mut ThreadStart, env: *mut c_void, env_drop: Option<Drop1>) {
    unsafe {
        let arg_drop = (*s).arg_drop;
        gc::root_drop((*s).env_root);
        gc::root_drop((*h).arg_root);
        gc::root_drop((*h).result_root);
        open_workers_give();
        if !env.is_null() {
            if let Some(drop_env) = env_drop {
                drop_env(env);
            }
        }
        if let Some(drop_arg) = arg_drop {
            drop_arg((*h).arg);
        }
        libc::free(s.cast());
        libc::free((*h).arg);
        libc::free((*h).result);
        libc::free(h.cast());
    }
}

/// The shared body of both entry points.
///
/// # Safety
/// As [`kelp_thread_start_rooted`].
unsafe fn start_common(
    h: *mut WorkerHandle,
    body: Option<Body>,
    code: *mut c_void,
    env: *mut c_void,
    env_drop: Option<Drop1>,
    arg_descr: *const c_void,
    result_descr: *const c_void,
    arg_drop: Option<Drop1>,
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
            env_root: 0,
            arg_drop,
        });
        // Refused before anything is rooted: the new thread never sees this,
        // the spawner does.
        if !open_workers_take() {
            libc::free(s.cast());
            if !env.is_null() {
                if let Some(drop_env) = env_drop {
                    drop_env(env);
                }
            }
            if let Some(drop_arg) = arg_drop {
                drop_arg((*h).arg);
            }
            libc::free((*h).arg);
            libc::free((*h).result);
            libc::free(h.cast());
            kelp_panic(c"thread.spawn: 256 threads are already running; join or drop a thread before spawning another".as_ptr());
        }
        // Root the spawn's values before the worker can run. The three slots
        // are malloc'd, so the collector cannot see them otherwise. A table
        // that cannot grow is named here, after the roots already taken are
        // released.
        let mut rooted = true;
        if !env.is_null() {
            match gc::root_add((&raw mut (*s).env).cast(), word_descr()) {
                Some(r) => (*s).env_root = r,
                None => rooted = false,
            }
        }
        if rooted && !arg_descr.is_null() {
            match gc::root_add((*h).arg, arg_descr) {
                Some(r) => (*h).arg_root = r,
                None => rooted = false,
            }
        }
        if rooted && !result_descr.is_null() {
            match gc::root_add((*h).result, result_descr) {
                Some(r) => (*h).result_root = r,
                None => rooted = false,
            }
        }
        if !rooted {
            spawn_abandon(h, s, env, env_drop);
            kelp_panic(c"thread.spawn: the collector's root table could not grow".as_ptr());
        }
        (*h).hold = gc::spawn_pause() as c_int;
        let rc = libc::pthread_create(&raw mut (*h).tid, ptr::null(), tramp_entry, s.cast());
        if rc != 0 {
            // Nothing was started, so nothing else owns the start record, the
            // handle or its slots: release them, then report the return code.
            // pthread_create does not set errno, so the code is passed in.
            spawn_abandon(h, s, env, env_drop);
            kelp_panic_at(c"thread.spawn: pthread_create failed".as_ptr(), rc);
        }
        if (*h).hold != 0 {
            // Test window (section 9): the worker is held before its first use,
            // the spawner clears the heap, and then reuses the freed memory, so
            // any record the collector did not scan reads the wrong values. Only
            // the handle is touched here, because the worker may already have
            // ended and freed the start record.
            gc::collect();
            for _ in 0..64 {
                let _ = crate::alloc::buf_alloc(65536);
            }
            AtomicI32::from_ptr(&raw mut (*h).gate).store(1, Ordering::Release);
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
        gc::native_enter();
        libc::pthread_join((*h).tid, ptr::null_mut());
        gc::native_leave();
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_thread_dispose(h: *mut WorkerHandle) {
    unsafe {
        gc::root_drop((*h).arg_root);
        gc::root_drop((*h).result_root);
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
            kelp_thread_dispose(h);
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
    /// Root of `value` (0 when the element holds no collected memory).
    root: u64,
}

/// # Safety
/// `value` must be readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_new(value: *const c_void, size: usize, drop: Option<Drop1>) -> *mut Mutex {
    unsafe { kelp_mutex_new_rooted(value, size, drop, ptr::null()) }
}

/// A mutex whose value is rooted by `descr` (managed), or none when `descr`
/// is null. The value stays a root until the mutex is dropped or taken apart.
///
/// # Safety
/// `value` must be readable for `size` bytes; `descr` must describe the value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_mutex_new_rooted(value: *const c_void, size: usize, drop: Option<Drop1>, descr: *const c_void) -> *mut Mutex {
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
        (*m).root = 0;
        if !descr.is_null() {
            match gc::root_add((*m).value, descr) {
                Some(r) => (*m).root = r,
                None => {
                    libc::pthread_mutex_destroy(&raw mut (*m).lock);
                    libc::free((*m).value);
                    libc::free(m.cast());
                    kelp_panic(c"Mutex.new: the collector's root table could not grow".as_ptr());
                }
            }
        }
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
        blk_lock(&raw mut (*m).lock);
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
        blk_lock(&raw mut (*m).lock);
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
        gc::root_drop((*m).root);
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
        gc::root_drop((*m).root);
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
    /// Root of `value` (0 when the element holds no collected memory).
    root: u64,
}

/// # Safety
/// `value` must be readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_new(value: *const c_void, size: usize, drop: Option<Drop1>) -> *mut RwLock {
    unsafe { kelp_rwlock_new_rooted(value, size, drop, ptr::null()) }
}

/// A reader/writer lock whose value is rooted by `descr` (managed), or none
/// when `descr` is null (see [`kelp_mutex_new_rooted`]).
///
/// # Safety
/// `value` must be readable for `size` bytes; `descr` must describe the value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rwlock_new_rooted(value: *const c_void, size: usize, drop: Option<Drop1>, descr: *const c_void) -> *mut RwLock {
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
        (*rw).root = 0;
        if !descr.is_null() {
            match gc::root_add((*rw).value, descr) {
                Some(r) => (*rw).root = r,
                None => {
                    libc::pthread_mutex_destroy(&raw mut (*rw).lock);
                    libc::pthread_cond_destroy(&raw mut (*rw).readers_ok);
                    libc::pthread_cond_destroy(&raw mut (*rw).writers_ok);
                    libc::free((*rw).value);
                    libc::free(rw.cast());
                    kelp_panic(c"RwLock.new: the collector's root table could not grow".as_ptr());
                }
            }
        }
        rw
    }
}

/// The read side's release, on both exits. A reader never poisons.
unsafe extern "C" fn rwlock_read_end(raw: *mut c_void) {
    unsafe {
        let rw = raw.cast::<RwLock>();
        blk_lock(&raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        while (*rw).writer != 0 || (*rw).writers_waiting > 0 {
            blk_wait(&raw mut (*rw).readers_ok, &raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
        if (*rw).poisoned != 0 {
            libc::pthread_mutex_unlock(&raw mut (*rw).lock);
            kelp_panic(c"rwlock poisoned".as_ptr());
        }
        (*rw).writers_waiting += 1;
        while (*rw).writer != 0 || (*rw).readers > 0 {
            blk_wait(&raw mut (*rw).writers_ok, &raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
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
        blk_lock(&raw mut (*rw).lock);
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
        gc::root_drop((*rw).root);
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
        gc::root_drop((*rw).root);
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
        blk_lock(&raw mut (*m).lock);
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
            blk_wait(&raw mut (*cv).cond, &raw mut (*m).lock);
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
        blk_lock(&raw mut (*m).lock);
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
            if blk_timedwait(&raw mut (*cv).cond, &raw mut (*m).lock, &raw const deadline)
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;

    /// Drops of the worker body's own copy of the argument: the drop the
    /// generated function registers on its cleanup list.
    static CALLEE_DROPS: AtomicUsize = AtomicUsize::new(0);
    /// Drops the runtime makes of a started worker's argument. Must stay 0.
    static STARTED_ARG_DROPS: AtomicUsize = AtomicUsize::new(0);
    /// Drops the runtime makes of an argument whose spawn never started.
    static ABANDON_DROPS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_callee_drop(_slot: *mut c_void) {
        CALLEE_DROPS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn count_started_arg_drop(_slot: *mut c_void) {
        STARTED_ARG_DROPS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn count_abandon_drop(_slot: *mut c_void) {
        ABANDON_DROPS.fetch_add(1, Ordering::SeqCst);
    }

    /// A worker body that owns its argument, registers it on the cleanup list
    /// the way the generated function does, and then panics before the value
    /// moves out. `code` carries the worker handle, so the body can record the
    /// panic the way `worker_end` does (`panicked` and the message).
    ///
    /// The panic ends in `cleanup_run_all_then_exit`, the path `worker_end`
    /// takes, rather than `kelp_panic`: in this test build a forced unwind that
    /// passes an `extern "C"` frame aborts the process, and `kelp_panic` is
    /// reached through one (the `extern "C"` body). `kelp_panic` itself is
    /// covered by the panic tests.
    unsafe extern "C" fn owning_panicking_body(
        code: *mut c_void,
        _env: *mut c_void,
        arg: *mut c_void,
        _out: *mut c_void,
    ) {
        unsafe {
            let h = code.cast::<WorkerHandle>();
            let mut frame = CleanupFrame {
                run: count_callee_drop,
                arg,
                prev: ptr::null_mut(),
            };
            cleanup_push(&raw mut frame);
            (*h).panicked = 1;
            (*h).message = c"worker boom".as_ptr();
            crate::cleanup::cleanup_run_all_then_exit(ptr::null_mut());
        }
    }

    fn new_handle() -> *mut WorkerHandle {
        unsafe { kelp_thread_new(core::mem::size_of::<usize>(), core::mem::size_of::<usize>()) }
    }

    #[test]
    fn started_worker_that_panics_drops_its_argument_once() {
        let h = new_handle();
        unsafe {
            h.as_ref().unwrap().arg.cast::<usize>().write(7);
            start_common(
                h,
                Some(owning_panicking_body),
                h.cast(),
                ptr::null_mut(),
                None,
                ptr::null(),
                ptr::null(),
                Some(count_started_arg_drop),
            );
            assert_eq!(kelp_thread_wait(h), 1, "the worker must panic");
            kelp_thread_dispose(h);
        }
        assert_eq!(CALLEE_DROPS.load(Ordering::SeqCst), 1, "the callee drops its argument once");
        assert_eq!(STARTED_ARG_DROPS.load(Ordering::SeqCst), 0, "a started argument is not the runtime's");
    }

    #[test]
    fn abandoned_spawn_drops_its_argument_once() {
        let h = new_handle();
        unsafe {
            let s = libc::malloc(core::mem::size_of::<ThreadStart>()).cast::<ThreadStart>();
            s.write(ThreadStart {
                handle: h,
                body: None,
                code: ptr::null_mut(),
                env: ptr::null_mut(),
                env_drop: None,
                env_root: 0,
                arg_drop: Some(count_abandon_drop),
            });
            spawn_abandon(h, s, ptr::null_mut(), None);
        }
        assert_eq!(ABANDON_DROPS.load(Ordering::SeqCst), 1, "an abandoned spawn drops its argument once");
    }
}

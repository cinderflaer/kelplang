//! The per-thread cleanup list: the Rust stand-in for `pthread_cleanup_push`
//! and `pthread_cleanup_pop` (stage 1 decision Q3).
//!
//! A frame lives on the caller's stack. Pushing links it onto the calling
//! thread's list; the list head is kept as a value under a pthread key, so no
//! heap allocation is involved. Glibc runs `pthread_cleanup` handlers only when a
//! thread calls `pthread_exit` (or is cancelled) and when a handler is popped
//! with `execute` set. [`cleanup_run_all`] and [`cleanup_pop`] reproduce both,
//! so the worker-thread panic path behaves as the C runtime does.
//!
//! No function here unwinds: `pthread_exit` is declared `extern "C-unwind"` so
//! the intent is explicit, and the runtime holds no value with a `Drop` impl.

use core::ffi::c_void;
use core::ptr::{addr_of_mut, null_mut};

use libc::{
    PTHREAD_ONCE_INIT, pthread_getspecific, pthread_key_create, pthread_key_t, pthread_once,
    pthread_once_t, pthread_setspecific,
};

unsafe extern "C-unwind" {
    /// Terminates the calling thread, unwinding its forced-unwind frames.
    /// `libc` does not carry the `C-unwind` form, so it is declared here.
    fn pthread_exit(value: *mut c_void) -> !;
}

/// One pending handler. `run(arg)` is called when the frame is popped with
/// `execute` set, or when the thread exits.
#[repr(C)]
pub struct CleanupFrame {
    pub run: unsafe extern "C" fn(*mut c_void),
    pub arg: *mut c_void,
    pub prev: *mut CleanupFrame,
}

static mut KEY: pthread_key_t = 0;
static mut KEY_ONCE: pthread_once_t = PTHREAD_ONCE_INIT;

extern "C" fn make_key() {
    // SAFETY: called once by `pthread_once`; `KEY` is written nowhere else.
    // A failure leaves `KEY` unset, and `head`/`set_head` then fail the same
    // way every time, so the runtime aborts at its first use rather than
    // corrupt a list.
    unsafe {
        let rc = pthread_key_create(addr_of_mut!(KEY), None);
        if rc != 0 {
            libc::abort();
        }
    }
}

fn key() -> pthread_key_t {
    // SAFETY: `pthread_once` runs `make_key` exactly once, before any reader
    // sees the key, and `KEY` is not written after that.
    unsafe {
        pthread_once(addr_of_mut!(KEY_ONCE), make_key);
        *addr_of_mut!(KEY)
    }
}

fn head() -> *mut CleanupFrame {
    // SAFETY: the key was created above; the value is either null or a frame
    // this thread pushed and has not yet popped.
    unsafe { pthread_getspecific(key()) as *mut CleanupFrame }
}

fn set_head(frame: *mut CleanupFrame) {
    // SAFETY: as for `head`. A nonzero return would leave the list out of
    // step with the caller, so it aborts.
    unsafe {
        if pthread_setspecific(key(), frame as *const c_void) != 0 {
            libc::abort();
        }
    }
}

/// Links `frame` onto this thread's list. `frame` must stay valid, and must not
/// move, until the matching [`cleanup_pop`].
///
/// # Safety
/// `frame` must point to a fully initialised [`CleanupFrame`] that outlives its
/// pop. Pops must follow pushes in strict LIFO order on this thread.
pub unsafe fn cleanup_push(frame: *mut CleanupFrame) {
    // SAFETY: the caller guarantees `frame` is valid; it becomes the new head.
    unsafe {
        (*frame).prev = head();
    }
    set_head(frame);
}

/// Unlinks `frame`, the current head, from this thread's list. When `execute`
/// is set the handler runs first, as `pthread_cleanup_pop(1)` does.
///
/// # Safety
/// `frame` must be the frame most recently pushed on this thread and not yet
/// popped.
pub unsafe fn cleanup_pop(frame: *mut CleanupFrame, execute: bool) {
    // SAFETY: the caller guarantees `frame` is the live head.
    unsafe {
        set_head((*frame).prev);
        if execute {
            ((*frame).run)((*frame).arg);
        }
    }
}

/// Runs every handler on this thread's list, newest first, and clears the list.
/// This is the order glibc uses when a thread calls `pthread_exit`.
///
/// # Safety
/// Must be called only on the thread that owns the list, and every frame still
/// on it must be live.
pub unsafe fn cleanup_run_all() {
    let mut f = head();
    set_head(null_mut());
    while !f.is_null() {
        // SAFETY: each frame on the list is live (guaranteed by the caller),
        // and it is read before its handler runs.
        unsafe {
            let next = (*f).prev;
            ((*f).run)((*f).arg);
            f = next;
        }
    }
}

/// Runs the cleanup list, then terminates the calling thread with `status` as
/// its exit value.
///
/// # Safety
/// As for [`cleanup_run_all`]. The thread does not return.
pub unsafe fn cleanup_run_all_then_exit(status: *mut c_void) -> ! {
    // SAFETY: forwarded from the caller's contract.
    unsafe {
        cleanup_run_all();
        pthread_exit(status)
    }
}

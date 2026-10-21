//! Port of kelp_rt.c:58-76 (the worker handle, the prefix the panic path
//! writes) and kelp_rt.c:79-197 (the `kelp_panic` family and `kelp_assert`).
//!
//! The worker-thread panic path ends the thread with `pthread_exit` after the
//! cleanup list runs. The exported functions are plain `extern "C"`: the
//! `no_std` crate does not support `C-unwind`, and the nested static-library
//! build uses `panic=abort`, so the forced unwind of `pthread_exit` is the only
//! unwinding and it passes through frames without landing pads.
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::cleanup;

/// The personality routine the unwind tables name (`force-unwind-tables`, which
/// `pthread_exit`'s forced unwind needs). The runtime is built with `panic=abort`,
/// so there is nothing to catch: every frame answers `_URC_CONTINUE_UNWIND`, so
/// the forced unwind of a worker's `pthread_exit` walks on to the thread's start.
/// Any other answer makes glibc abort the thread.
#[cfg(all(not(test), feature = "panic-handler"))]
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality(
    _version: c_int,
    _actions: c_int,
    _class: u64,
    _exception: *mut c_void,
    _context: *mut c_void,
) -> c_int {
    /// `_URC_CONTINUE_UNWIND`.
    const CONTINUE_UNWIND: c_int = 8;
    CONTINUE_UNWIND
}

#[cfg(all(not(test), feature = "panic-handler"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    const MSG: &[u8] = b"kelp-rt: internal error: the runtime panicked\n";
    // SAFETY: writes a constant buffer to stderr; the result is ignored because
    // the process is ending either way.
    unsafe {
        libc::write(2, MSG.as_ptr() as *const libc::c_void, MSG.len());
        libc::abort()
    }
}

/// The pthread key holding the calling thread's worker handle (null on the
/// main thread). WP3 sets it at thread start through [`set_worker_handle`].
static WORKER_KEY: AtomicU32 = AtomicU32::new(0);
static mut WORKER_KEY_ONCE: libc::pthread_once_t = libc::PTHREAD_ONCE_INIT;

extern "C" fn make_worker_key() {
    let mut key: libc::pthread_key_t = 0;
    unsafe {
        if libc::pthread_key_create(&raw mut key, None) != 0 {
            libc::abort();
        }
    }
    WORKER_KEY.store(key as u32, Ordering::Release);
}

fn worker_key() -> libc::pthread_key_t {
    unsafe {
        libc::pthread_once(&raw mut WORKER_KEY_ONCE, make_worker_key);
    }
    WORKER_KEY.load(Ordering::Acquire) as libc::pthread_key_t
}

/// Sets the calling thread's worker handle. WP3 calls this at thread start.
///
/// # Safety
/// `h` must be null or point to a live [`WorkerHandle`] for the thread's life.
pub unsafe fn set_worker_handle(h: *mut WorkerHandle) {
    unsafe {
        libc::pthread_setspecific(worker_key(), h.cast());
    }
}

/// The prefix of C's `kelp_Thread` (kelp_rt.c:58-76) that the panic path
/// writes. WP3 (`thread.rs`) must use this definition and not a second one.
#[repr(C)]
pub struct WorkerHandle {
    pub tid: libc::pthread_t,
    pub panicked: c_int,
    pub result: *mut c_void,
    pub arg: *mut c_void,
    pub message: *const c_char,
    /// Roots of the argument and the result slot (0 when the value holds no
    /// collected memory). Cleared when the handle is disposed of.
    pub arg_root: u64,
    pub result_root: u64,
    /// Test hold (`KELP_GC_TEST_SPAWN_PAUSE`): the worker waits for `gate`
    /// before it attaches, while the spawner runs a clearing.
    pub hold: c_int,
    pub gate: c_int,
    pub state: c_int,
}

unsafe extern "C" {
    static mut stderr: *mut libc::FILE;
    fn __errno_location() -> *mut c_int;
}

/// The calling thread's worker handle, or null on the main thread.
unsafe fn worker_handle() -> *mut WorkerHandle {
    unsafe { libc::pthread_getspecific(worker_key()).cast() }
}

/// Records `message` in the worker handle and ends the worker. Never returns.
unsafe fn worker_end(message: *const c_char) -> ! {
    unsafe {
        let h = worker_handle();
        (*h).panicked = 1;
        (*h).message = message;
        crate::stdlib::os::cleanup_thread();
        cleanup::cleanup_run_all_then_exit(ptr::null_mut())
    }
}

/// The text a worker hands its joiner: `msg`, then `detail`. Heap, because
/// the joiner reads it after the worker's frame is gone; nothing frees it
/// (the admitted leak of MK9 section 2). Returns `msg` if malloc fails.
unsafe fn kelp_thread_panic_text(msg: *const c_char, detail: *const c_char) -> *const c_char {
    unsafe {
        let n = libc::strlen(msg);
        let m = if detail.is_null() { 0 } else { libc::strlen(detail) };
        let out = libc::malloc(n.wrapping_add(m).wrapping_add(1)) as *mut c_char;
        if out.is_null() {
            return msg;
        }
        if n > 0 {
            libc::memcpy(out.cast(), msg.cast(), n);
        }
        if m > 0 {
            libc::memcpy(out.add(n).cast(), detail.cast(), m);
        }
        *out.add(n.wrapping_add(m)) = 0;
        out
    }
}

/// Prints `panic: <msg>` and exits 1; in a worker, records `msg` and ends
/// the worker instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_panic(msg: *const c_char) -> ! {
    unsafe {
        if !worker_handle().is_null() {
            worker_end(msg)
        }
        libc::fprintf(stderr, c"panic: %s\n".as_ptr(), msg);
        libc::exit(1)
    }
}

/// As `kelp_panic`, plus an `errno N: strerror` detail line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_panic_at(msg: *const c_char, p: c_int) -> ! {
    unsafe {
        if !worker_handle().is_null() {
            let mut detail = [0 as c_char; 256];
            libc::snprintf(
                detail.as_mut_ptr(),
                detail.len(),
                c"\n  errno %d: %s".as_ptr(),
                p,
                libc::strerror(p),
            );
            worker_end(kelp_thread_panic_text(msg, detail.as_ptr()))
        }
        libc::fprintf(stderr, c"panic: %s\n".as_ptr(), msg);
        libc::fprintf(stderr, c"  errno %d: %s\n".as_ptr(), p, libc::strerror(p));
        libc::exit(1)
    }
}

/// `kelp_panic_at` with the current `errno`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_panic_errno(msg: *const c_char) -> ! {
    unsafe { kelp_panic_at(msg, *__errno_location()) }
}

/// As `kelp_panic`, plus a `getaddrinfo: gai_strerror` detail line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_panic_gai(msg: *const c_char, rc: c_int) -> ! {
    unsafe {
        if !worker_handle().is_null() {
            let mut detail = [0 as c_char; 256];
            libc::snprintf(
                detail.as_mut_ptr(),
                detail.len(),
                c"\n  getaddrinfo: %s".as_ptr(),
                libc::gai_strerror(rc),
            );
            worker_end(kelp_thread_panic_text(msg, detail.as_ptr()))
        }
        libc::fprintf(stderr, c"panic: %s\n".as_ptr(), msg);
        libc::fprintf(stderr, c"  getaddrinfo: %s\n".as_ptr(), libc::gai_strerror(rc));
        libc::exit(1)
    }
}

/// As `kelp_panic`, plus an `openssl: reason` detail line. `reason` is
/// resolved by the caller at the failing call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_panic_tls(msg: *const c_char, reason: *const c_char) -> ! {
    unsafe {
        if !worker_handle().is_null() {
            let mut detail = [0 as c_char; 288];
            libc::snprintf(
                detail.as_mut_ptr(),
                detail.len(),
                c"\n  openssl: %s".as_ptr(),
                reason,
            );
            worker_end(kelp_thread_panic_text(msg, detail.as_ptr()))
        }
        libc::fprintf(stderr, c"panic: %s\n".as_ptr(), msg);
        libc::fprintf(stderr, c"  openssl: %s\n".as_ptr(), reason);
        libc::exit(1)
    }
}

/// Panics with "assertion failed" when `cond` is false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_assert(cond: bool) {
    if !cond {
        unsafe { kelp_panic(c"assertion failed".as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;
    use std::process::Command;

    const CASE_ENV: &str = "KELP_RT_PANIC_CASE";

    /// True inside the child process that a parent test re-executed for `case`.
    fn in_child(case: &str) -> bool {
        std::env::var(CASE_ENV).as_deref() == Ok(case)
    }

    /// Re-runs one test of this binary as a child and returns its exit code
    /// and stderr. The child is the process that the panic actually ends.
    fn run_child(case: &str, test_path: &str) -> (Option<i32>, String) {
        let exe = std::env::current_exe().expect("test binary path");
        let out = Command::new(exe)
            .args(["--exact", test_path, "--test-threads=1"])
            .env(CASE_ENV, case)
            .env("LC_ALL", "C")
            .output()
            .expect("spawn child");
        (out.status.code(), String::from_utf8(out.stderr).expect("utf8"))
    }

    unsafe extern "C" {
        fn pthread_create(
            thread: *mut libc::pthread_t,
            attr: *const libc::pthread_attr_t,
            start: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
            arg: *mut c_void,
        ) -> c_int;
    }

    /// A worker run for one panic test: the body runs on a fresh thread whose
    /// handle is `handle`, exactly as WP3 will set it up.
    struct Job {
        handle: WorkerHandle,
        body: fn(),
    }

    unsafe extern "C" fn worker_start(arg: *mut c_void) -> *mut c_void {
        unsafe {
            let job = arg.cast::<Job>();
            set_worker_handle(&raw mut (*job).handle);
            ((*job).body)();
        }
        ptr::null_mut()
    }

    /// Runs `body` on a worker thread and returns the handle it left behind.
    fn run_worker(body: fn()) -> (bool, String) {
        let mut job = Job {
            handle: WorkerHandle {
                tid: 0,
                panicked: 0,
                result: ptr::null_mut(),
                arg: ptr::null_mut(),
                message: ptr::null(),
                arg_root: 0,
                result_root: 0,
                hold: 0,
                gate: 0,
                state: 0,
            },
            body,
        };
        unsafe {
            let mut tid: libc::pthread_t = 0;
            let rc = pthread_create(&raw mut tid, ptr::null(), worker_start, (&raw mut job).cast());
            assert_eq!(rc, 0);
            assert_eq!(libc::pthread_join(tid, ptr::null_mut()), 0);
            let text = CStr::from_ptr(job.handle.message).to_str().expect("utf8").to_owned();
            (job.handle.panicked == 1, text)
        }
    }

    #[test]
    fn kelp_panic_prints_and_exits_1() {
        if in_child("plain") {
            unsafe { kelp_panic(c"boom".as_ptr()) }
        }
        let (code, err) = run_child("plain", "panic::tests::kelp_panic_prints_and_exits_1");
        assert_eq!(code, Some(1));
        assert_eq!(err, "panic: boom\n");
    }

    #[test]
    fn kelp_panic_at_prints_errno_line() {
        if in_child("at") {
            unsafe { kelp_panic_at(c"open failed".as_ptr(), 2) }
        }
        let (code, err) = run_child("at", "panic::tests::kelp_panic_at_prints_errno_line");
        assert_eq!(code, Some(1));
        assert_eq!(err, "panic: open failed\n  errno 2: No such file or directory\n");
    }

    #[test]
    fn kelp_panic_errno_uses_current_errno() {
        if in_child("errno") {
            unsafe {
                *__errno_location() = 13;
                kelp_panic_errno(c"write failed".as_ptr())
            }
        }
        let (code, err) = run_child("errno", "panic::tests::kelp_panic_errno_uses_current_errno");
        assert_eq!(code, Some(1));
        assert_eq!(err, "panic: write failed\n  errno 13: Permission denied\n");
    }

    #[test]
    fn kelp_panic_gai_prints_gai_text() {
        if in_child("gai") {
            // EAI_NONAME is -2 on glibc.
            unsafe { kelp_panic_gai(c"resolve failed".as_ptr(), -2) }
        }
        let (code, err) = run_child("gai", "panic::tests::kelp_panic_gai_prints_gai_text");
        assert_eq!(code, Some(1));
        assert_eq!(
            err,
            "panic: resolve failed\n  getaddrinfo: Name or service not known\n"
        );
    }

    #[test]
    fn kelp_panic_tls_prints_reason() {
        if in_child("tls") {
            unsafe { kelp_panic_tls(c"handshake failed".as_ptr(), c"certificate expired".as_ptr()) }
        }
        let (code, err) = run_child("tls", "panic::tests::kelp_panic_tls_prints_reason");
        assert_eq!(code, Some(1));
        assert_eq!(err, "panic: handshake failed\n  openssl: certificate expired\n");
    }

    #[test]
    fn kelp_assert_true_returns() {
        unsafe { kelp_assert(true) };
    }

    #[test]
    fn kelp_assert_false_panics() {
        if in_child("assert") {
            unsafe { kelp_assert(false) }
        }
        let (code, err) = run_child("assert", "panic::tests::kelp_assert_false_panics");
        assert_eq!(code, Some(1));
        assert_eq!(err, "panic: assertion failed\n");
    }

    #[test]
    fn kelp_thread_panic_text_joins_msg_and_detail() {
        unsafe {
            let p = kelp_thread_panic_text(c"ab".as_ptr(), c"\n  x".as_ptr());
            assert_eq!(CStr::from_ptr(p).to_bytes(), b"ab\n  x");
            libc::free(p.cast_mut().cast());
            let q = kelp_thread_panic_text(c"only".as_ptr(), ptr::null());
            assert_eq!(CStr::from_ptr(q).to_bytes(), b"only");
            libc::free(q.cast_mut().cast());
        }
    }

    #[test]
    fn kelp_panic_in_worker_records_message_and_ends_worker() {
        let (panicked, text) = run_worker(|| unsafe { kelp_panic(c"worker boom".as_ptr()) });
        assert!(panicked);
        assert_eq!(text, "worker boom");
    }

    #[test]
    fn kelp_panic_at_in_worker_appends_errno_line() {
        let (panicked, text) =
            run_worker(|| unsafe { kelp_panic_at(c"open failed".as_ptr(), 2) });
        assert!(panicked);
        assert_eq!(text, "open failed\n  errno 2: No such file or directory");
    }

    #[test]
    fn kelp_panic_gai_in_worker_appends_gai_line() {
        let (panicked, text) =
            run_worker(|| unsafe { kelp_panic_gai(c"resolve failed".as_ptr(), -2) });
        assert!(panicked);
        assert_eq!(text, "resolve failed\n  getaddrinfo: Name or service not known");
    }

    #[test]
    fn kelp_panic_tls_in_worker_appends_openssl_line() {
        let (panicked, text) = run_worker(|| unsafe {
            kelp_panic_tls(c"handshake failed".as_ptr(), c"certificate expired".as_ptr())
        });
        assert!(panicked);
        assert_eq!(text, "handshake failed\n  openssl: certificate expired");
    }
}

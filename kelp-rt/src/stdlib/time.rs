//! `std/time`: wall and monotonic clocks, and sleeping.

use crate::panic::kelp_panic_errno;

/// `clock_gettime` or a panic with `msg` and the errno detail line.
///
/// # Safety
/// `msg` must be a NUL-terminated string.
unsafe fn clock(clock: libc::clockid_t, msg: *const core::ffi::c_char) -> libc::timespec {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(clock, &mut ts) } != 0 {
        unsafe { kelp_panic_errno(msg) };
    }
    ts
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__time__now_unix_raw() -> i64 {
    let ts = unsafe { clock(libc::CLOCK_REALTIME, c"time.now_unix: clock_gettime failed".as_ptr()) };
    ts.tv_sec as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__time__now_mono_ns_raw() -> i64 {
    let ts = unsafe { clock(libc::CLOCK_MONOTONIC, c"time.now_mono_ns: clock_gettime failed".as_ptr()) };
    (ts.tv_sec as i64) * 1_000_000_000 + (ts.tv_nsec as i64)
}

/// Sleeps `ms` milliseconds, finishing the remainder after an interruption.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__time__sleep_ms_raw(ms: u64) {
    sleep_ms(ms);
}

/// The shared sleep body, also the `std/thread` export.
pub(crate) fn sleep_ms(ms: u64) {
    let mut ts = libc::timespec {
        tv_sec: (ms / 1000) as libc::time_t,
        tv_nsec: ((ms % 1000) * 1_000_000) as _,
    };
    let p: *mut libc::timespec = &mut ts;
    while unsafe { libc::nanosleep(p, p) } != 0 {
        // Interrupted: `p` now holds the remainder, so the call resumes.
    }
}

//! Port of kelp_rt.c:184-189 (`kelp_io_flush_stdio`): a write to the
//! process's own stdout or stderr flushes stdio first, so the builtin
//! `print` and `std/io`'s `File` stay in order.
use core::ffi::c_int;

unsafe extern "C" {
    static mut stdout: *mut libc::FILE;
    static mut stderr: *mut libc::FILE;
    fn fflush(stream: *mut libc::FILE) -> c_int;
}

/// Flushes stdio's `stdout` when `fd` is 1 and `stderr` when `fd` is 2.
/// A no-op for every other descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_io_flush_stdio(fd: c_int) {
    unsafe {
        if fd == 1 {
            let _ = fflush(stdout);
        } else if fd == 2 {
            let _ = fflush(stderr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_char;

    unsafe extern "C" {
        fn __fpending(stream: *mut libc::FILE) -> usize;
        fn fputs(s: *const c_char, stream: *mut libc::FILE) -> c_int;
    }

    /// One test for both descriptors, because both use the C `stdout` buffer.
    #[test]
    fn flush_drains_stdout_for_fd_1_only() {
        unsafe {
            let out = stdout;
            fputs(c"pending".as_ptr(), out);
            assert!(__fpending(out) > 0);
            kelp_io_flush_stdio(0);
            assert!(__fpending(out) > 0, "fd 0 must not flush stdout");
            kelp_io_flush_stdio(1);
            assert_eq!(__fpending(out), 0, "fd 1 must flush stdout");
            // stderr is unbuffered in glibc; the call must still be harmless.
            kelp_io_flush_stdio(2);
            assert_eq!(__fpending(stderr), 0);
        }
    }
}

//! `std/result`: the one primitive `Result.orExit` needs, a message to
//! standard error and an exit. `std/result` cannot import `std/io` or
//! `std/process` (both import `std/str`, which imports `std/result`), so the
//! write and the exit are here, with the same bodies those modules use.

use core::ffi::c_char;

/// Writes `message` and a newline to standard error, the way `std/io`'s
/// `eprint` does, then exits with `code`, the way `std/process`'s `exit` does
/// (libc `exit`, so the stdio buffers are flushed).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__result__exit_with_message_raw(message: *const c_char, code: i32) {
    unsafe {
        super::io::kelp_std__io__write_str_raw(2, message);
        super::io::kelp_std__io__write_str_raw(2, c"\n".as_ptr());
        libc::exit(code)
    }
}

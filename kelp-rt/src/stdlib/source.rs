//! `std/source`: the source position of a call. Neither is tracked yet, so the
//! answers are the fixed placeholder values the C bodies returned.

use core::ffi::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__source__file() -> *const c_char {
    c"<source>".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__source__line() -> usize {
    0
}

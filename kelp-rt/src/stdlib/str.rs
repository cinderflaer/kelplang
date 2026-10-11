//! `std/str`: byte access, slicing, float formatting and the strict parsers.
//!
//! The parsers reject leading or trailing blanks and any embedded space, tab,
//! newline or carriage return, exactly as the C bodies did.

use core::ffi::{c_char, c_int, c_void};

use crate::abi::String as KString;
use crate::alloc::kelp_bytes_alloc_raw;
use crate::panic::kelp_panic;
use crate::text::kelp_String__from_bytes;

unsafe extern "C" {
    fn snprintf(buf: *mut c_char, n: usize, fmt: *const c_char, ...) -> c_int;
}

/// The byte at `i` of the string, or a panic when `i` is past its end. The
/// check scans `i + 1` bytes, so a read near the start of a long string costs
/// what it reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__byte_at_raw(s: *const c_char, i: usize) -> u8 {
    unsafe {
        if i == usize::MAX || !libc::memchr(s.cast::<c_void>(), 0, i + 1).is_null() {
            kelp_panic(c"str.byte: index out of bounds".as_ptr());
        }
        *s.cast::<u8>().add(i)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__byte_len_raw(s: *const c_char) -> usize {
    unsafe { libc::strlen(s) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__byte_ptr_raw(s: *const c_char) -> *mut u8 {
    s as *mut u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__slice_from_raw(s: *const c_char, from: usize) -> *const c_char {
    unsafe {
        if from > libc::strlen(s) {
            kelp_panic(c"str.substring: index out of bounds".as_ptr());
        }
        s.add(from)
    }
}

/// `%.*f` of `x` with `decimals` places. A number that fits a small stack
/// buffer is formatted once and copied; anything longer takes a sizing pass and
/// a second formatting into the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__from_f64_raw(x: f64, decimals: i32) -> KString {
    unsafe {
        let mut small = [0 as c_char; 128];
        let need = snprintf(small.as_mut_ptr(), small.len(), c"%.*f".as_ptr(), decimals as c_int, x);
        if need < 0 {
            kelp_panic(c"str.from_f64: formatting failed".as_ptr());
        }
        let need = need as usize;
        if need < small.len() {
            let s = kelp_bytes_alloc_raw(need);
            core::ptr::copy_nonoverlapping(small.as_ptr().cast::<u8>(), s.data, need + 1);
            return kelp_String__from_bytes(s);
        }
        let b = kelp_bytes_alloc_raw(need);
        snprintf(b.data.cast(), b.len + 1, c"%.*f".as_ptr(), decimals as c_int, x);
        kelp_String__from_bytes(b)
    }
}

/// The strict-parser precheck: a non-empty string with no blank character.
/// Returns false for the strings the C bodies rejected before `strto*` ran.
unsafe fn plain_token(s: *const c_char) -> bool {
    unsafe {
        if *s == 0 {
            return false;
        }
        let mut p = s.cast::<u8>();
        while *p != 0 {
            if matches!(*p, b' ' | b'\t' | b'\n' | b'\r') {
                return false;
            }
            p = p.add(1);
        }
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__parse_i64_raw(s: *const c_char, out: *mut i64) -> bool {
    unsafe {
        if !plain_token(s) {
            return false;
        }
        let mut end: *mut c_char = core::ptr::null_mut();
        *libc::__errno_location() = 0;
        let v = libc::strtoll(s, &mut end, 10);
        if end as *const c_char == s || *end != 0 || *libc::__errno_location() != 0 {
            return false;
        }
        *out = v;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__parse_u64_raw(s: *const c_char, out: *mut u64) -> bool {
    unsafe {
        if *s == b'-' as c_char || !plain_token(s) {
            return false;
        }
        let mut end: *mut c_char = core::ptr::null_mut();
        *libc::__errno_location() = 0;
        let v = libc::strtoull(s, &mut end, 10);
        if end as *const c_char == s || *end != 0 || *libc::__errno_location() != 0 {
            return false;
        }
        *out = v;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__str__parse_f64_raw(s: *const c_char, out: *mut f64) -> bool {
    unsafe {
        if !plain_token(s) {
            return false;
        }
        let mut end: *mut c_char = core::ptr::null_mut();
        let v = libc::strtod(s, &mut end);
        if end as *const c_char == s || *end != 0 {
            return false;
        }
        *out = v;
        true
    }
}

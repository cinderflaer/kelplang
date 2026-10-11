//! Port of kelp_rt.c:632-1081 (`kelp_View`, `kelp_Bytes`, `kelp_String`, the
//! shared growth routine and the UTF-8 check).
//!
//! Every exported function keeps the C name and signature. A `kelp_Bytes` or
//! `kelp_String` passed or returned by value is the `#[repr(C)]` struct in
//! [`crate::abi`], which the SysV ABI moves the same way the C compiler does.
//! The IR never passes these by value; it uses the `_ptr` adapters the C
//! wrapper used to define (see `kelpc/build.rs`).
use core::ffi::c_char;
use core::ptr;

use crate::abi::{Bytes, String as KString, View};
use crate::alloc::{buf_alloc, buf_free};
use crate::gc::{self, POLL_BYTES};
use crate::panic::kelp_panic;

/// Grows `*data` so `extra` more bytes fit past `len`, doubling (or starting
/// at 8) so appends are amortized O(1), and re-terminates the buffer.
///
/// # Safety
/// `data` and `cap` must describe a live owned buffer of `cap + 1` bytes (or a
/// null buffer with cap 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_buf_reserve(data: *mut *mut u8, len: usize, cap: *mut usize, extra: usize) {
    unsafe {
        if len.wrapping_add(extra) <= *cap {
            return;
        }
        let mut want = if *cap == 0 { 8 } else { *cap };
        while want < len + extra {
            want *= 2;
        }
        let p = grow(*data, len, want);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        *data = p;
        *cap = want;
        *p.add(len) = 0;
    }
}

/// The new buffer for a grown string: hosted, `realloc` (which may move it);
/// managed, a fresh collected buffer with the live bytes copied, since the
/// collector does not grow in place and the old one is its to reclaim.
///
/// # Safety
/// `data` must be null or a live buffer holding `len` bytes.
#[cfg(not(feature = "managed"))]
unsafe fn grow(data: *mut u8, _len: usize, want: usize) -> *mut u8 {
    unsafe { libc_realloc(data, want + 1) }
}

/// See the hosted [`grow`].
///
/// # Safety
/// As the hosted form.
#[cfg(feature = "managed")]
unsafe fn grow(data: *mut u8, len: usize, want: usize) -> *mut u8 {
    unsafe {
        let p = buf_alloc(want + 1);
        if len > 0 {
            ptr::copy_nonoverlapping(data, p, len);
        }
        p
    }
}

#[cfg(not(feature = "managed"))]
unsafe fn libc_realloc(p: *mut u8, n: usize) -> *mut u8 {
    unsafe { libc::realloc(p.cast(), n).cast() }
}

/// Whether `n` bytes at `p` are well-formed UTF-8: no overlong forms, no
/// surrogates, nothing above U+10FFFF, and no truncated sequence.
///
/// # Safety
/// `p` must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_utf8_valid(p: *const u8, n: usize) -> bool {
    unsafe {
        let mut i = 0usize;
        let mut next_poll = POLL_BYTES;
        while i < n {
            if i >= next_poll {
                gc::safepoint();
                next_poll = i + POLL_BYTES;
            }
            let c = *p.add(i);
            let extra: usize;
            let mut cp: u64;
            if c < 0x80 {
                i += 1;
                continue;
            } else if c & 0xE0 == 0xC0 {
                extra = 1;
                cp = (c & 0x1F) as u64;
            } else if c & 0xF0 == 0xE0 {
                extra = 2;
                cp = (c & 0x0F) as u64;
            } else if c & 0xF8 == 0xF0 {
                extra = 3;
                cp = (c & 0x07) as u64;
            } else {
                return false;
            }
            if i + extra >= n {
                return false;
            }
            for k in 1..=extra {
                let b = *p.add(i + k);
                if b & 0xC0 != 0x80 {
                    return false;
                }
                cp = (cp << 6) | (b & 0x3F) as u64;
            }
            if extra == 1 && cp < 0x80 {
                return false;
            }
            if extra == 2 && cp < 0x800 {
                return false;
            }
            if extra == 3 && cp < 0x10000 {
                return false;
            }
            if cp > 0x10FFFF {
                return false;
            }
            if (0xD800..=0xDFFF).contains(&cp) {
                return false;
            }
            i += extra + 1;
        }
        true
    }
}

// ---- kelp_View ----------------------------------------------------------------

/// `View.len`.
#[unsafe(no_mangle)]
pub extern "C" fn kelp_View__len(this: View) -> usize {
    this.len
}

/// `View.is_empty`.
#[unsafe(no_mangle)]
pub extern "C" fn kelp_View__is_empty(this: View) -> bool {
    this.len == 0
}

/// `View.get`: the byte at `i`, panicking past the end.
///
/// # Safety
/// The view's pointer must be readable for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_View__get(this: View, i: usize) -> u8 {
    unsafe {
        if i >= this.len {
            kelp_panic(c"View.get: index out of bounds".as_ptr());
        }
        *this.ptr.cast::<u8>().add(i)
    }
}

/// `View.slice(from, to)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_View__slice(this: View, from: usize, to: usize) -> View {
    unsafe {
        if from > to || to > this.len {
            kelp_panic(c"View.slice: index out of bounds".as_ptr());
        }
        View {
            ptr: this.ptr.add(from),
            len: to - from,
        }
    }
}

/// `View.eq`: length first, then bytes.
///
/// # Safety
/// Both views must be readable for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_View__eq(this: View, other: View) -> bool {
    unsafe {
        if this.len != other.len {
            return false;
        }
        this.len == 0 || libc::memcmp(this.ptr.cast(), other.ptr.cast(), this.len) == 0
    }
}

/// `str.view()`: a view of a whole NUL-terminated string.
///
/// # Safety
/// `s` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_str__view(s: *const c_char) -> View {
    unsafe {
        View {
            ptr: s,
            len: if s.is_null() { 0 } else { libc::strlen(s) },
        }
    }
}

/// `View.to_string`: a copy of the view's bytes, terminated.
///
/// # Safety
/// The view must be readable for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_View__to_string(this: View) -> KString {
    unsafe {
        let p = buf_alloc(this.len + 1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        if this.len > 0 {
            ptr::copy_nonoverlapping(this.ptr.cast::<u8>(), p, this.len);
        }
        *p.add(this.len) = 0;
        KString {
            data: p,
            len: this.len,
            cap: this.len,
        }
    }
}

// ---- kelp_Bytes ---------------------------------------------------------------

/// `Bytes.empty`: no content, but a NUL at `data[0]`, never a null pointer.
///
/// # Safety
/// The result owns its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__empty() -> Bytes {
    unsafe {
        let p = buf_alloc(1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        *p = 0;
        Bytes {
            data: p,
            len: 0,
            cap: 0,
        }
    }
}

/// `Bytes.from_str`: a copy of a NUL-terminated string.
///
/// # Safety
/// `s` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__from_str(s: *const c_char) -> Bytes {
    unsafe {
        let n = libc::strlen(s);
        let p = buf_alloc(n + 1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        ptr::copy_nonoverlapping(s.cast::<u8>(), p, n + 1);
        Bytes {
            data: p,
            len: n,
            cap: n,
        }
    }
}

/// `Bytes.len`.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__len(this: *const Bytes) -> usize {
    unsafe { (*this).len }
}

/// `Bytes.is_empty`.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__is_empty(this: *const Bytes) -> bool {
    unsafe { (*this).len == 0 }
}

/// `Bytes.get`: the byte at `i`, panicking past the end.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__get(this: *const Bytes, i: usize) -> u8 {
    unsafe {
        if i >= (*this).len {
            kelp_panic(c"Bytes.get: index out of bounds".as_ptr());
        }
        *(*this).data.add(i)
    }
}

/// `Bytes.set`: stores at `i`, panicking past the end.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__set(this: *mut Bytes, i: usize, value: u8) {
    unsafe {
        if i >= (*this).len {
            kelp_panic(c"Bytes.set: index out of bounds".as_ptr());
        }
        *(*this).data.add(i) = value;
    }
}

/// `Bytes.clone`: a copy with its own buffer.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__clone(this: *const Bytes) -> Bytes {
    unsafe {
        let len = (*this).len;
        let p = buf_alloc(len + 1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        ptr::copy_nonoverlapping((*this).data, p, len);
        *p.add(len) = 0;
        Bytes {
            data: p,
            len,
            cap: len,
        }
    }
}

/// `Bytes.push`: appends one byte.
///
/// # Safety
/// `this` must be valid and own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__push(this: *mut Bytes, byte: u8) {
    unsafe {
        let b = &mut *this;
        kelp_buf_reserve(&raw mut b.data, b.len, &raw mut b.cap, 1);
        *b.data.add(b.len) = byte;
        b.len += 1;
        *b.data.add(b.len) = 0;
    }
}

/// `Bytes.append`: appends `other`'s bytes.
///
/// # Safety
/// Both must be valid; `this` owns its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__append(this: *mut Bytes, other: *const Bytes) {
    unsafe {
        let n = (*other).len;
        if n == 0 {
            return;
        }
        let b = &mut *this;
        kelp_buf_reserve(&raw mut b.data, b.len, &raw mut b.cap, n);
        ptr::copy_nonoverlapping((*other).data, b.data.add(b.len), n);
        b.len += n;
        *b.data.add(b.len) = 0;
    }
}

/// `Bytes.reserve`: room for `extra` more bytes.
///
/// # Safety
/// `this` must be valid and own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__reserve(this: *mut Bytes, extra: usize) {
    unsafe {
        let b = &mut *this;
        kelp_buf_reserve(&raw mut b.data, b.len, &raw mut b.cap, extra);
    }
}

/// `Bytes.clear`: empties the buffer and keeps it.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__clear(this: *mut Bytes) {
    unsafe {
        (*this).len = 0;
        if !(*this).data.is_null() {
            *(*this).data = 0;
        }
    }
}

/// `Bytes.as_str`: the buffer as a `str`, panicking on invalid UTF-8.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_Bytes__as_str(this: *const Bytes) -> *const c_char {
    unsafe {
        if !kelp_utf8_valid((*this).data, (*this).len) {
            kelp_panic(c"Bytes.as_str: input is not valid utf-8".as_ptr());
        }
        (*this).data.cast()
    }
}

/// Releases a `Bytes` buffer and leaves the value empty with no buffer.
///
/// # Safety
/// `b` must be valid and own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_bytes_drop(b: *mut Bytes) {
    unsafe {
        buf_free((*b).data);
        (*b).data = ptr::null_mut();
        (*b).len = 0;
    }
}

// ---- kelp_String --------------------------------------------------------------

/// `String.empty`.
///
/// # Safety
/// The result owns its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__empty() -> KString {
    unsafe {
        let p = buf_alloc(1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        *p = 0;
        KString {
            data: p,
            len: 0,
            cap: 0,
        }
    }
}

/// `String.from_str`: a copy of a NUL-terminated string.
///
/// # Safety
/// `text` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__from_str(text: *const c_char) -> KString {
    unsafe {
        let n = libc::strlen(text);
        let p = buf_alloc(n + 1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        ptr::copy_nonoverlapping(text.cast::<u8>(), p, n + 1);
        KString {
            data: p,
            len: n,
            cap: n,
        }
    }
}

/// `String.from_bytes`: takes over `b`'s buffer after a UTF-8 check.
///
/// # Safety
/// `b` must own a valid buffer, which the result takes over.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__from_bytes(b: Bytes) -> KString {
    unsafe {
        if !kelp_utf8_valid(b.data, b.len) {
            kelp_panic(c"String.from_bytes: input is not valid utf-8".as_ptr());
        }
        KString {
            data: b.data,
            len: b.len,
            cap: b.cap,
        }
    }
}

/// The pointer-spelled `String.from_bytes` the IR calls (the by-value form
/// cannot cross the IR boundary, MK7 section 3.3, BUG-vN4tR).
///
/// # Safety
/// As [`kelp_String__from_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__from_bytes_ptr(b: *const Bytes) -> KString {
    unsafe { kelp_String__from_bytes(*b) }
}

/// `String.into_bytes`: hands the buffer over as `Bytes`.
///
/// # Safety
/// The result takes over `this`'s buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__into_bytes(this: KString) -> Bytes {
    Bytes {
        data: this.data,
        len: this.len,
        cap: this.cap,
    }
}

/// The pointer-spelled `String.into_bytes` the IR calls.
///
/// # Safety
/// As [`kelp_String__into_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__into_bytes_ptr(this: *const KString) -> Bytes {
    unsafe { kelp_String__into_bytes(*this) }
}

/// `String.len`: the byte length.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__len(this: *const KString) -> usize {
    unsafe { (*this).len }
}

/// `String.is_empty`.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__is_empty(this: *const KString) -> bool {
    unsafe { (*this).len == 0 }
}

/// `String.char_count`: the number of code points (bytes that are not
/// continuation bytes).
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__char_count(this: *const KString) -> usize {
    unsafe {
        let s = &*this;
        let mut n = 0;
        let mut next_poll = POLL_BYTES;
        for i in 0..s.len {
            if i >= next_poll {
                gc::safepoint();
                next_poll = i + POLL_BYTES;
            }
            if *s.data.add(i) & 0xC0 != 0x80 {
                n += 1;
            }
        }
        n
    }
}

/// `String.clone`.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__clone(this: *const KString) -> KString {
    unsafe {
        let len = (*this).len;
        let p = buf_alloc(len + 1);
        if p.is_null() {
            kelp_panic(c"out of memory".as_ptr());
        }
        ptr::copy_nonoverlapping((*this).data, p, len);
        *p.add(len) = 0;
        KString {
            data: p,
            len,
            cap: len,
        }
    }
}

/// `String.push`: appends one byte, unchecked.
///
/// # Safety
/// `this` must be valid and own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__push(this: *mut KString, ch: u8) {
    unsafe {
        let s = &mut *this;
        kelp_buf_reserve(&raw mut s.data, s.len, &raw mut s.cap, 1);
        *s.data.add(s.len) = ch;
        s.len += 1;
        *s.data.add(s.len) = 0;
    }
}

/// `String.push_str`: appends a NUL-terminated string.
///
/// # Safety
/// `this` must own its buffer; `text` must be NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__push_str(this: *mut KString, text: *const c_char) {
    unsafe {
        let n = libc::strlen(text);
        if n == 0 {
            return;
        }
        let s = &mut *this;
        kelp_buf_reserve(&raw mut s.data, s.len, &raw mut s.cap, n);
        ptr::copy_nonoverlapping(text.cast::<u8>(), s.data.add(s.len), n);
        s.len += n;
        *s.data.add(s.len) = 0;
    }
}

/// `String.reserve`.
///
/// # Safety
/// `this` must own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__reserve(this: *mut KString, extra: usize) {
    unsafe {
        let s = &mut *this;
        kelp_buf_reserve(&raw mut s.data, s.len, &raw mut s.cap, extra);
    }
}

/// `String.with_capacity`: an empty string with room for `n` bytes. The
/// buffer is never null, even at capacity zero.
///
/// # Safety
/// The result owns its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__with_capacity(n: usize) -> KString {
    unsafe {
        let mut s = KString {
            data: ptr::null_mut(),
            len: 0,
            cap: 0,
        };
        kelp_buf_reserve(&raw mut s.data, 0, &raw mut s.cap, n);
        if s.data.is_null() {
            let p = buf_alloc(1);
            if p.is_null() {
                kelp_panic(c"out of memory".as_ptr());
            }
            *p = 0;
            s.data = p;
        }
        s
    }
}

/// `String.clear`: empties the string and keeps its buffer.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__clear(this: *mut KString) {
    unsafe {
        (*this).len = 0;
        if !(*this).data.is_null() {
            *(*this).data = 0;
        }
    }
}

/// `String.as_str`: the buffer, which is always NUL-terminated.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__as_str(this: *const KString) -> *const c_char {
    unsafe { (*this).data.cast() }
}

/// `String.view`: a length-carrying view of the same buffer.
///
/// # Safety
/// `this` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_String__view(this: *const KString) -> View {
    unsafe {
        View {
            ptr: (*this).data.cast(),
            len: (*this).len,
        }
    }
}

/// Releases a `String` buffer and leaves the value empty with no buffer.
///
/// # Safety
/// `s` must be valid and own its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_string_drop(s: *mut KString) {
    unsafe {
        buf_free((*s).data);
        (*s).data = ptr::null_mut();
        (*s).len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_check_rejects_what_the_c_check_rejected() {
        let ok = "héllo".as_bytes();
        let bad: &[&[u8]] = &[&[0xC0, 0x80], &[0xED, 0xA0, 0x80], &[0xF4, 0x90, 0x80, 0x80], &[0xE2, 0x82]];
        unsafe {
            assert!(kelp_utf8_valid(ok.as_ptr(), ok.len()));
            for b in bad {
                assert!(!kelp_utf8_valid(b.as_ptr(), b.len()), "{b:?}");
            }
        }
    }

    #[test]
    fn push_keeps_the_buffer_terminated() {
        unsafe {
            let mut s = kelp_String__empty();
            for c in b"abc" {
                kelp_String__push(&raw mut s, *c);
            }
            kelp_String__push_str(&raw mut s, c"def".as_ptr());
            assert_eq!(s.len, 6);
            assert_eq!(*s.data.add(6), 0);
            kelp_string_drop(&raw mut s);
        }
    }
}

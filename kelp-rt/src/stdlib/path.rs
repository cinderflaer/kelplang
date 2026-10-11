//! `std/path`: the normalizer is Kelp's `std/rt` (exported from `cap.rs`); this
//! body supplies the buffer it writes into and the owned `String` it returns.

use core::ffi::c_char;

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::panic::kelp_panic;
use crate::text::kelp_String__from_bytes;

unsafe extern "C" {
    /// The normalizer, a Kelp function (`std/rt`) that the compiler emits into
    /// the program object whenever `std/path` is loaded. It is not a runtime
    /// export, so this is a plain external reference.
    fn kelp_std__rt__path_normalize(path: *const c_char, out: *mut Bytes) -> bool;
}

const PATH_MAX_LEN: usize = libc::PATH_MAX as usize;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__path__normalize_raw(s: *const c_char) -> KString {
    unsafe {
        let mut buf = [0u8; PATH_MAX_LEN + 1];
        // A borrowed view of the stack buffer: `len` is the capacity, which is
        // the normalizer's contract. It is never dropped, since it is not an
        // allocation.
        let mut view = Bytes {
            data: buf.as_mut_ptr(),
            len: PATH_MAX_LEN,
            cap: PATH_MAX_LEN,
        };
        if !kelp_std__rt__path_normalize(s, &mut view) {
            kelp_panic(c"path.normalize: path must be absolute and must not escape its own root".as_ptr());
        }
        let n = libc::strlen(buf.as_ptr().cast());
        let b = kelp_bytes_alloc_raw(n);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), b.data, n);
        kelp_String__from_bytes(b)
    }
}

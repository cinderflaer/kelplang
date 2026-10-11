//! `std/mem`: the raw allocation primitive.

use crate::abi::Bytes;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__mem__alloc_raw(n: usize) -> Bytes {
    unsafe { crate::alloc::kelp_bytes_alloc_raw(n) }
}

//! `std/fmt`: hexadecimal text of an integer and of a byte buffer.

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::text::{kelp_String__empty, kelp_String__from_bytes, kelp_String__from_str};

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__fmt__hex_raw(n: u64) -> KString {
    // The same text as `%llx`: lowercase, no padding, `0` for zero.
    let mut digits = [0u8; 16];
    let mut at = digits.len();
    let mut v = n;
    loop {
        at -= 1;
        digits[at] = b"0123456789abcdef"[(v & 0xF) as usize];
        v >>= 4;
        if v == 0 {
            break;
        }
    }
    let mut text = [0u8; 17];
    let len = digits.len() - at;
    text[..len].copy_from_slice(&digits[at..]);
    unsafe { kelp_String__from_str(text.as_ptr().cast()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fmt__bytes_raw(b: *const Bytes) -> KString {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    unsafe {
        let src = &*b;
        if src.len == 0 {
            return kelp_String__empty();
        }
        let out = kelp_bytes_alloc_raw(src.len * 3 - 1);
        let mut at = 0usize;
        for i in 0..src.len {
            let byte = *src.data.add(i);
            if i > 0 {
                *out.data.add(at) = b' ';
                at += 1;
            }
            *out.data.add(at) = HEX[((byte >> 4) & 0xF) as usize];
            *out.data.add(at + 1) = HEX[(byte & 0xF) as usize];
            at += 2;
        }
        kelp_String__from_bytes(out)
    }
}

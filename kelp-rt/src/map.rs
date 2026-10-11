//! Port of kelp_rt.c:1083-1117 (the `Map` key helpers): FNV-1a over a key's
//! bytes, a length-aware equality against a borrowed key, the 64-bit scalar
//! finalizer and the combine step a derived key hash folds with. The values
//! must match the C ones bit for bit, because a map's iteration order and its
//! bucket layout depend on them.
use core::ffi::c_char;

use crate::abi::String as KString;
use crate::gc::{self, POLL_BYTES};

/// FNV-1a over `len` bytes at `data`.
///
/// # Safety
/// `data` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_map_hash(data: *const c_char, len: usize) -> u64 {
    let mut h: u64 = 1469598103934665603;
    let mut next_poll = POLL_BYTES;
    for i in 0..len {
        if i >= next_poll {
            gc::safepoint();
            next_poll = i + POLL_BYTES;
        }
        // SAFETY: the caller guarantees `len` readable bytes at `data`.
        let b = unsafe { *data.add(i) } as u8;
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Whether the key's bytes equal `len` bytes at `data`: lengths first, then
/// `memcmp`, so a key holding bytes `strlen` would stop at still compares.
///
/// # Safety
/// `key` must be valid; `data` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_map_key_eq(key: *const KString, data: *const c_char, len: usize) -> bool {
    unsafe {
        if (*key).len != len {
            return false;
        }
        len == 0 || libc::memcmp((*key).data.cast(), data.cast(), len) == 0
    }
}

/// The scalar-key hash: a 64-bit finalizer over the widened value.
#[unsafe(no_mangle)]
pub extern "C" fn kelp_map_mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x
}

/// Folds one already-hashed field or tag into a derived key's hash.
#[unsafe(no_mangle)]
pub extern "C" fn kelp_map_combine(mut h: u64, x: u64) -> u64 {
    h ^= kelp_map_mix(x);
    h = h.wrapping_mul(1099511628211);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_of_the_empty_key_is_the_offset_basis() {
        assert_eq!(unsafe { kelp_map_hash(c"".as_ptr(), 0) }, 1469598103934665603);
    }

    /// The offset basis is the C constant as written in `kelp_rt.c`
    /// (1469598103934665603, one digit short of the published FNV basis). It
    /// is kept, not corrected, because the hash decides map bucket layout.
    #[test]
    fn fnv_of_a_known_key_uses_the_c_offset_basis() {
        assert_eq!(unsafe { kelp_map_hash(c"a".as_ptr(), 1) }, 0x44bd8ad473cd9906);
    }
}

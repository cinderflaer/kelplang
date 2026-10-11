//! `std/env`: reads and writes of the process environment. The lookup cache and
//! the `setenv` calls live in `env.rs`; these are the Kelp-facing bodies.

use core::ffi::c_char;

use crate::panic::kelp_panic_errno;
use crate::text::{kelp_String__empty, kelp_String__from_str};
use crate::abi::String as KString;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__env__has_raw(name: *const c_char) -> bool {
    unsafe { !crate::env::kelp_env_lookup(name).is_null() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__env__get_raw(name: *const c_char) -> KString {
    unsafe {
        let v = crate::env::kelp_env_lookup(name);
        if v.is_null() {
            return kelp_String__empty();
        }
        kelp_String__from_str(v)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__env__set_raw(name: *const c_char, value: *const c_char) {
    unsafe {
        if crate::env::kelp_env_set(name, value) != 0 {
            kelp_panic_errno(c"env.set: cannot set variable".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__env__unset_raw(name: *const c_char) {
    unsafe {
        if crate::env::kelp_env_unset(name) != 0 {
            kelp_panic_errno(c"env.unset: cannot unset variable".as_ptr());
        }
    }
}

//! `std/sys`: the host OS and CPU architecture names, fixed at build time.

use core::ffi::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__sys__os() -> *const c_char {
    #[cfg(target_os = "linux")]
    return c"linux".as_ptr();
    #[cfg(target_os = "macos")]
    return c"macos".as_ptr();
    #[cfg(target_os = "windows")]
    return c"windows".as_ptr();
    #[cfg(target_os = "freebsd")]
    return c"freebsd".as_ptr();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "freebsd")))]
    return c"unknown".as_ptr();
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__sys__arch() -> *const c_char {
    #[cfg(target_arch = "x86_64")]
    return c"x86_64".as_ptr();
    #[cfg(target_arch = "aarch64")]
    return c"aarch64".as_ptr();
    #[cfg(all(target_arch = "riscv64", target_pointer_width = "64"))]
    return c"riscv64".as_ptr();
    #[cfg(target_arch = "arm")]
    return c"arm".as_ptr();
    #[cfg(target_arch = "x86")]
    return c"x86".as_ptr();
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64", target_arch = "arm", target_arch = "x86")))]
    return c"unknown".as_ptr();
}

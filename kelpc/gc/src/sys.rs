//! Raw libc and pthread declarations. Linked from the final program's libc;
//! no crate dependency.

use core::ffi::{c_char, c_int, c_long, c_ulong, c_void};

pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_PRIVATE: c_int = 0x02;
pub const MAP_ANONYMOUS: c_int = 0x20;
pub const MAP_NORESERVE: c_int = 0x4000;

/// glibc `pthread_attr_t` is 56 bytes; 64 is a safe upper bound.
#[repr(C, align(16))]
pub struct PthreadAttr(pub [u64; 8]);

/// Storage for `pthread_mutex_t` (40 bytes on glibc) and `pthread_cond_t` (48).
#[repr(C, align(16))]
pub struct Opaque(pub [u64; 8]);

unsafe extern "C" {
    pub fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: c_long) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
    pub fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    pub fn exit(status: c_int) -> !;
    pub fn abort() -> !;
    pub fn getenv(name: *const c_char) -> *const c_char;

    pub fn pthread_self() -> c_ulong;
    pub fn pthread_getattr_np(thread: c_ulong, attr: *mut PthreadAttr) -> c_int;
    pub fn pthread_attr_getstack(attr: *const PthreadAttr, addr: *mut *mut c_void, size: *mut usize) -> c_int;
    pub fn pthread_attr_destroy(attr: *mut PthreadAttr) -> c_int;

    pub fn pthread_mutex_init(m: *mut Opaque, attr: *const c_void) -> c_int;
    pub fn pthread_mutex_lock(m: *mut Opaque) -> c_int;
    pub fn pthread_mutex_unlock(m: *mut Opaque) -> c_int;
    pub fn pthread_cond_init(c: *mut Opaque, attr: *const c_void) -> c_int;
    pub fn pthread_cond_wait(c: *mut Opaque, m: *mut Opaque) -> c_int;
    pub fn pthread_cond_broadcast(c: *mut Opaque) -> c_int;
}

//! `std/rand`: bytes from `/dev/urandom`, read directly with no stdio buffer.

use core::ffi::c_int;

use crate::abi::Bytes;
use crate::panic::{kelp_panic, kelp_panic_errno};

/// Reads exactly `buf.len()` bytes from `/dev/urandom`. `Ok(false)` is a failed
/// or short read; `Err(())` means the device could not be opened, so the caller
/// reports that case with its own message and errno.
fn read_urandom(buf: &mut [u8]) -> Result<bool, ()> {
    let fd: c_int = unsafe { libc::open(c"/dev/urandom".as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        return Err(());
    }
    let mut at = 0usize;
    let mut ok = true;
    while at < buf.len() {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().add(at).cast(), buf.len() - at) };
        if n <= 0 {
            ok = false;
            break;
        }
        at += n as usize;
    }
    unsafe { libc::close(fd) };
    Ok(ok)
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__rand__u64_raw() -> u64 {
    let mut v = [0u8; 8];
    match read_urandom(&mut v) {
        Err(()) => unsafe { kelp_panic_errno(c"rand.u64: no entropy source".as_ptr()) },
        Ok(false) => unsafe { kelp_panic(c"rand.u64: short read from /dev/urandom".as_ptr()) },
        Ok(true) => u64::from_ne_bytes(v),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__rand__fill_raw(buf: *mut Bytes) {
    unsafe {
        let b = &*buf;
        let len = b.len;
        if len == 0 {
            // Nothing to fill: the device is still opened, as before, so a
            // missing entropy source fails here too.
            if read_urandom(&mut []).is_err() {
                kelp_panic_errno(c"rand.bytes: no entropy source".as_ptr());
            }
            return;
        }
        let dst = core::slice::from_raw_parts_mut(b.data, len);
        match read_urandom(dst) {
            Err(()) => kelp_panic_errno(c"rand.bytes: no entropy source".as_ptr()),
            Ok(false) => kelp_panic(c"rand.bytes: short read from /dev/urandom".as_ptr()),
            Ok(true) => {}
        }
    }
}

//! `std/io`: descriptor reads and writes, polling, and the errno text.

use core::ffi::{c_char, c_int};

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::io::kelp_io_flush_stdio;
use crate::panic::kelp_panic_errno;
use crate::text::kelp_String__from_str;

fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

/// Writes `len` bytes from `data` to `fd`, looping over short writes. `EINTR`
/// is a retry; any other failure panics with `msg`.
unsafe fn write_loop(fd: c_int, data: *const u8, len: usize, msg: *const c_char) -> usize {
    unsafe {
        // A pipe may take fewer bytes than asked for, so a write is a loop.
        kelp_io_flush_stdio(fd);
        let mut off = 0usize;
        while off < len {
            let n = libc::write(fd, data.add(off).cast(), len - off);
            if n < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                kelp_panic_errno(msg);
            }
            off += n as usize;
        }
        off
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__is_open_raw(fd: c_int) -> bool {
    // Not a cached flag: the kernel is asked, which is the honest question for
    // a bare descriptor.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__read_raw(fd: c_int, max: usize) -> Bytes {
    unsafe {
        let mut b = kelp_bytes_alloc_raw(max);
        let n = libc::read(fd, b.data.cast(), max);
        if n < 0 {
            kelp_panic_errno(c"io.read: read failed".as_ptr());
        }
        b.len = n as usize;
        *b.data.add(b.len) = 0;
        b
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__read_into_raw(fd: c_int, buf: *mut Bytes, at: usize) -> usize {
    unsafe {
        // The buffer's length is the capacity of this read. Filling it does not
        // change that length, so `at + n` never passes it.
        let b = &*buf;
        let room = if at < b.len { b.len - at } else { 0 };
        if room == 0 {
            return 0;
        }
        let n = libc::read(fd, b.data.add(at).cast(), room);
        if n < 0 {
            kelp_panic_errno(c"io.read_into: read failed".as_ptr());
        }
        *b.data.add(at + n as usize) = 0;
        n as usize
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__write_bytes_raw(fd: c_int, data: *const Bytes) -> usize {
    unsafe { write_loop(fd, (*data).data, (*data).len, c"io.write: write failed".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__write_text_raw(fd: c_int, text: *const KString) -> usize {
    unsafe { write_loop(fd, (*text).data, (*text).len, c"io.write_text: write failed".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__write_str_raw(fd: c_int, s: *const c_char) -> usize {
    unsafe {
        let len = libc::strlen(s);
        write_loop(fd, s.cast::<u8>(), len, c"io.write_str: write failed".as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__close_raw(fd: c_int) {
    unsafe {
        if libc::close(fd) != 0 {
            kelp_panic_errno(c"io.close: close failed".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__set_nonblocking_raw(fd: c_int, on: bool) {
    unsafe {
        let mut flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags < 0 {
            kelp_panic_errno(c"io.set_nonblocking: fcntl failed".as_ptr());
        }
        if on {
            flags |= libc::O_NONBLOCK;
        } else {
            flags &= !libc::O_NONBLOCK;
        }
        if libc::fcntl(fd, libc::F_SETFL, flags) != 0 {
            kelp_panic_errno(c"io.set_nonblocking: fcntl failed".as_ptr());
        }
    }
}

/// `poll` on one descriptor for `events`, waiting `timeout_ms` (a value past
/// `INT_MAX` is forever, the same as a negative timeout). `EINTR` retries.
/// Returns whether the descriptor is ready, where HUP and ERR count as ready.
unsafe fn wait_for(fd: c_int, timeout_ms: u64, events: i16, msg: *const c_char) -> bool {
    unsafe {
        let mut p = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let t: c_int = if timeout_ms > 2_147_483_647 { -1 } else { timeout_ms as c_int };
        let mut r;
        loop {
            r = libc::poll(&mut p, 1, t);
            if !(r < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if r < 0 {
            kelp_panic_errno(msg);
        }
        r > 0 && (p.revents & (events | libc::POLLHUP | libc::POLLERR)) != 0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__wait_readable_raw(fd: c_int, timeout_ms: u64) -> bool {
    unsafe { wait_for(fd, timeout_ms, libc::POLLIN, c"io.wait_readable: poll failed".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__wait_writable_raw(fd: c_int, timeout_ms: u64) -> bool {
    unsafe { wait_for(fd, timeout_ms, libc::POLLOUT, c"io.wait_writable: poll failed".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__io__errno_text_raw(err: c_int) -> KString {
    unsafe { kelp_String__from_str(libc::strerror(err)) }
}

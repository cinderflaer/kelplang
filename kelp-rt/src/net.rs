//! Port of kelp_rt.c:492-527 (`kelp_rt_connect`), the shared TCP dial.
//!
//! The dial reports failures instead of panicking: `*gai_rc` is the
//! `getaddrinfo` code (0 when resolution worked) and `*sys_errno` is the errno
//! of the last failed step. Both are 0 when a descriptor is returned.
use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::tls::set_reason;

/// Resolves `host`, tries each address in order, and returns the connected
/// descriptor, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rt_connect(
    host: *const c_char,
    port: c_uint,
    gai_rc: *mut c_int,
    sys_errno: *mut c_int,
) -> c_int {
    unsafe {
        let mut hints: libc::addrinfo = core::mem::zeroed();
        hints.ai_family = libc::AF_UNSPEC;
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut service = [0 as c_char; 8];
        libc::snprintf(service.as_mut_ptr(), service.len(), c"%u".as_ptr(), port);
        let mut list: *mut libc::addrinfo = ptr::null_mut();
        let rc = libc::getaddrinfo(host, service.as_ptr(), &hints, &mut list);
        if rc != 0 {
            *gai_rc = rc;
            *sys_errno = 0;
            return -1;
        }
        let mut fd: c_int = -1;
        let mut last: c_int = 0;
        let mut a = list;
        while !a.is_null() {
            fd = libc::socket((*a).ai_family, (*a).ai_socktype, (*a).ai_protocol);
            if fd < 0 {
                last = *libc::__errno_location();
                a = (*a).ai_next;
                continue;
            }
            if libc::connect(fd, (*a).ai_addr, (*a).ai_addrlen) == 0 {
                break;
            }
            last = *libc::__errno_location();
            libc::close(fd);
            fd = -1;
            a = (*a).ai_next;
        }
        libc::freeaddrinfo(list);
        *gai_rc = 0;
        *sys_errno = last;
        fd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn connect_to_listening_loopback_returns_descriptor() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let mut gai: c_int = -1;
        let mut err: c_int = -1;
        let fd = unsafe { kelp_rt_connect(c"127.0.0.1".as_ptr(), port as c_uint, &mut gai, &mut err) };
        assert!(fd >= 0);
        assert_eq!((gai, err), (0, 0));
        unsafe { libc::close(fd) };
    }

    #[test]
    fn connect_to_closed_port_reports_refused() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let mut gai: c_int = -1;
        let mut err: c_int = -1;
        let fd = unsafe { kelp_rt_connect(c"127.0.0.1".as_ptr(), port as c_uint, &mut gai, &mut err) };
        assert_eq!(fd, -1);
        assert_eq!(gai, 0);
        assert_eq!(err, libc::ECONNREFUSED);
    }
}

/// The errno of the last failed call.
fn last_errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

/// Fills `text` with `fmt`, which takes the errno and its message as `%d` and `%s`.
unsafe fn errno_text(text: &mut [c_char; 256], fmt: &core::ffi::CStr, err: c_int) {
    unsafe {
        libc::snprintf(text.as_mut_ptr(), text.len(), fmt.as_ptr(), err, libc::strerror(err));
    }
}

/// `std/net` `send_all_try_raw`: sends all of `data`, or stops at the first failure and
/// sets `reason`. Returns 0 on success and -1 on failure.
///
/// # Safety
/// `data` and `reason` are valid pointers; `fd` is an open socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__send_all_try_raw(fd: c_int, data: *const Bytes, reason: *mut KString) -> c_int {
    unsafe {
        let total = (*data).len;
        let mut off: usize = 0;
        while off < total {
            let n = libc::send(fd, (*data).data.add(off) as *const c_void, total - off, libc::MSG_NOSIGNAL);
            if n < 0 {
                let err = last_errno();
                if err == libc::EINTR {
                    continue;
                }
                let mut text = [0 as c_char; 256];
                errno_text(
                    &mut text,
                    c"net.send: send failed (the peer may have closed)\n  errno %d: %s",
                    err,
                );
                set_reason(reason, text.as_ptr());
                return -1;
            }
            off += n as usize;
        }
        0
    }
}

/// `std/net` `recv_try_raw`: up to `max` bytes. A failure sets `reason` and returns an
/// empty `Bytes`.
///
/// # Safety
/// `reason` is a valid pointer; `fd` is an open socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__recv_try_raw(fd: c_int, max: usize, reason: *mut KString) -> Bytes {
    unsafe {
        if max == 0 {
            return kelp_bytes_alloc_raw(0);
        }
        let mut b = kelp_bytes_alloc_raw(max);
        let n = loop {
            let n = libc::recv(fd, b.data as *mut c_void, max, 0);
            if !(n < 0 && last_errno() == libc::EINTR) {
                break n;
            }
        };
        if n < 0 {
            let err = last_errno();
            let mut text = [0 as c_char; 256];
            if err == libc::EAGAIN || err == libc::EWOULDBLOCK {
                libc::snprintf(text.as_mut_ptr(), text.len(), c"%s".as_ptr(), c"net.recv: timed out".as_ptr());
            } else {
                errno_text(
                    &mut text,
                    c"net.recv: receive failed\n  errno %d: %s",
                    err,
                );
            }
            set_reason(reason, text.as_ptr());
            b.len = 0;
            *b.data = 0;
            return b;
        }
        b.len = n as usize;
        *b.data.add(b.len) = 0;
        b
    }
}

/// `std/net` `recv_into_try_raw`: reads into `buf` from `at` on, up to its length. A
/// failure sets `reason` and returns 0.
///
/// # Safety
/// `buf` and `reason` are valid pointers; `fd` is an open socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__recv_into_try_raw(
    fd: c_int,
    buf: *mut Bytes,
    at: usize,
    reason: *mut KString,
) -> usize {
    unsafe {
        let room = if at < (*buf).len { (*buf).len - at } else { 0 };
        if room == 0 {
            return 0;
        }
        let dst = (*buf).data.add(at);
        let n = loop {
            let n = libc::recv(fd, dst as *mut c_void, room, 0);
            if !(n < 0 && last_errno() == libc::EINTR) {
                break n;
            }
        };
        if n < 0 {
            let err = last_errno();
            let mut text = [0 as c_char; 256];
            if err == libc::EAGAIN || err == libc::EWOULDBLOCK {
                libc::snprintf(text.as_mut_ptr(), text.len(), c"%s".as_ptr(), c"net.recv_into: timed out".as_ptr());
            } else {
                errno_text(
                    &mut text,
                    c"net.recv_into: receive failed\n  errno %d: %s",
                    err,
                );
            }
            set_reason(reason, text.as_ptr());
            return 0;
        }
        *dst.add(n as usize) = 0;
        n as usize
    }
}

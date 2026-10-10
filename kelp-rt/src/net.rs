//! Port of kelp_rt.c:492-527 (`kelp_rt_connect`), the shared TCP dial.
//!
//! The dial reports failures instead of panicking: `*gai_rc` is the
//! `getaddrinfo` code (0 when resolution worked) and `*sys_errno` is the errno
//! of the last failed step. Both are 0 when a descriptor is returned.
use core::ffi::{c_char, c_int, c_uint};
use core::ptr;

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

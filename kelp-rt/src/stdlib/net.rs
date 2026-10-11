//! `std/net`: sockets. Every body runs after the capability check the compiler
//! inserted at the caller's call site, so `getaddrinfo` never sees an unchecked
//! name. The dial itself is `net.rs` (`kelp_rt_connect`), shared with the
//! `std/tls` path.

use core::ffi::{c_char, c_int, c_void};
use core::ptr::{null, null_mut};

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::cap::kelp_listen_loopback_take;
use crate::net::kelp_rt_connect;
use crate::panic::{kelp_panic, kelp_panic_at, kelp_panic_errno, kelp_panic_gai};
use crate::text::kelp_String__from_str;

use super::set_reason;

unsafe extern "C" {
    fn inet_ntop(af: c_int, src: *const c_void, dst: *mut c_char, size: libc::socklen_t) -> *const c_char;
}

const INET6_ADDRSTRLEN: usize = 46;

fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__connect_raw(host: *const c_char, port: u16) -> c_int {
    unsafe {
        let mut gai: c_int = 0;
        let mut last: c_int = 0;
        let fd = kelp_rt_connect(host, port as u32, &mut gai, &mut last);
        if fd < 0 {
            if gai != 0 {
                kelp_panic_gai(c"net.connect: cannot resolve host".as_ptr(), gai);
            }
            kelp_panic_at(c"net.connect: connection failed".as_ptr(), last);
        }
        fd
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__connect_try_raw(host: *const c_char, port: u16, reason: *mut KString) -> c_int {
    unsafe {
        let mut gai: c_int = 0;
        let mut last: c_int = 0;
        let fd = kelp_rt_connect(host, port as u32, &mut gai, &mut last);
        if fd < 0 {
            let mut text = [0 as c_char; 512];
            if gai != 0 {
                libc::snprintf(
                    text.as_mut_ptr(),
                    text.len(),
                    c"net.connect: cannot resolve host\n  getaddrinfo: %s".as_ptr(),
                    libc::gai_strerror(gai),
                );
            } else {
                libc::snprintf(
                    text.as_mut_ptr(),
                    text.len(),
                    c"net.connect: connection failed\n  errno %d: %s".as_ptr(),
                    last,
                    libc::strerror(last),
                );
            }
            set_reason(reason, text.as_ptr());
        }
        fd
    }
}

/// `listen` on IPv4 `0.0.0.0` (or `127.0.0.1` for a loopback grant), with one
/// family so `net.listen(0)` plus a `127.0.0.1` connect is deterministic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__listen_raw(port: u16) -> c_int {
    unsafe {
        let mut hints: libc::addrinfo = core::mem::zeroed();
        hints.ai_family = libc::AF_INET;
        hints.ai_socktype = libc::SOCK_STREAM;
        hints.ai_flags = libc::AI_PASSIVE;
        // A grant whose hosts do not admit `0.0.0.0` is a loopback listener: the
        // verdict was left in the flag by `kelp_cap_check_port`, and it is
        // consumed here (threat model N5).
        let mut bind_host: *const c_char = null();
        if kelp_listen_loopback_take() != 0 {
            bind_host = c"127.0.0.1".as_ptr();
            hints.ai_flags = libc::AI_NUMERICHOST;
        }
        let mut service = [0 as c_char; 8];
        libc::snprintf(service.as_mut_ptr(), service.len(), c"%u".as_ptr(), port as libc::c_uint);
        let mut list: *mut libc::addrinfo = null_mut();
        let rc = libc::getaddrinfo(bind_host, service.as_ptr(), &hints, &mut list);
        if rc != 0 {
            kelp_panic_gai(c"net.listen: cannot resolve the bind address".as_ptr(), rc);
        }
        let mut fd: c_int = -1;
        let mut last: c_int = 0;
        let mut a = list;
        while !a.is_null() {
            fd = libc::socket((*a).ai_family, (*a).ai_socktype, (*a).ai_protocol);
            if fd < 0 {
                last = errno();
                a = (*a).ai_next;
                continue;
            }
            let on: c_int = 1;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                (&on as *const c_int).cast::<c_void>(),
                core::mem::size_of::<c_int>() as libc::socklen_t,
            );
            if libc::bind(fd, (*a).ai_addr, (*a).ai_addrlen) == 0 && libc::listen(fd, 128) == 0 {
                break;
            }
            last = errno();
            libc::close(fd);
            fd = -1;
            a = (*a).ai_next;
        }
        libc::freeaddrinfo(list);
        if fd < 0 {
            kelp_panic_at(c"net.listen: cannot bind".as_ptr(), last);
        }
        fd
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__accept_raw(fd: c_int) -> c_int {
    unsafe {
        let mut c;
        loop {
            c = libc::accept(fd, null_mut(), null_mut());
            if !(c < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if c < 0 {
            kelp_panic_errno(c"net.accept: accept failed".as_ptr());
        }
        c
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__accept_try_raw(fd: c_int, reason: *mut KString) -> c_int {
    unsafe {
        let mut c;
        loop {
            c = libc::accept(fd, null_mut(), null_mut());
            if !(c < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if c < 0 {
            let e = errno();
            let mut text = [0 as c_char; 256];
            if e == libc::EAGAIN || e == libc::EWOULDBLOCK {
                libc::snprintf(text.as_mut_ptr(), text.len(), c"net.accept: timed out".as_ptr());
            } else {
                libc::snprintf(
                    text.as_mut_ptr(),
                    text.len(),
                    c"net.accept: accept failed\n  errno %d: %s".as_ptr(),
                    e,
                    libc::strerror(e),
                );
            }
            set_reason(reason, text.as_ptr());
        }
        c
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__mono_ms_raw() -> i64 {
    unsafe {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) != 0 {
            kelp_panic_errno(c"net.mono_ms: clock_gettime failed".as_ptr());
        }
        (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64) / 1_000_000
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__close_raw(fd: c_int) {
    unsafe {
        if libc::close(fd) != 0 {
            kelp_panic_errno(c"net.close: close failed".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__shutdown_write_raw(fd: c_int) {
    unsafe {
        if libc::shutdown(fd, libc::SHUT_WR) != 0 {
            kelp_panic_errno(c"net.shutdown_write: shutdown failed".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__shutdown_write_try_raw(fd: c_int) -> bool {
    unsafe { libc::shutdown(fd, libc::SHUT_WR) == 0 }
}

/// `MSG_NOSIGNAL`: a peer that has closed its end must not kill the process with
/// SIGPIPE; the failure comes back as a panic or an error value instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__send_raw(fd: c_int, data: *const Bytes) -> usize {
    unsafe {
        let mut n;
        loop {
            n = libc::send(fd, (*data).data.cast(), (*data).len, libc::MSG_NOSIGNAL);
            if !(n < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if n < 0 {
            kelp_panic_errno(c"net.send: send failed (the peer may have closed)".as_ptr());
        }
        n as usize
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__send_all_raw(fd: c_int, data: *const Bytes) -> usize {
    unsafe {
        let mut off = 0usize;
        while off < (*data).len {
            let n = libc::send(fd, (*data).data.add(off).cast(), (*data).len - off, libc::MSG_NOSIGNAL);
            if n < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                kelp_panic_errno(c"net.send: send failed (the peer may have closed)".as_ptr());
            }
            off += n as usize;
        }
        off
    }
}

/// `recv(2)` with a length of 0 on a blocking socket waits for a peer event
/// instead of returning, so a request for 0 bytes returns the empty `Bytes`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__recv_raw(fd: c_int, max: usize) -> Bytes {
    unsafe {
        if max == 0 {
            return kelp_bytes_alloc_raw(0);
        }
        let mut b = kelp_bytes_alloc_raw(max);
        let mut n;
        loop {
            n = libc::recv(fd, b.data.cast(), max, 0);
            if !(n < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if n < 0 {
            if errno() == libc::EAGAIN || errno() == libc::EWOULDBLOCK {
                kelp_panic(c"net.recv: timed out".as_ptr());
            }
            kelp_panic_errno(c"net.recv: receive failed".as_ptr());
        }
        b.len = n as usize;
        *b.data.add(b.len) = 0;
        b
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__recv_into_raw(fd: c_int, buf: *mut Bytes, at: usize) -> usize {
    unsafe {
        // The buffer's length is the capacity of this read (as in `io.read_into`):
        // filling it does not change that length.
        let b = &*buf;
        let room = if at < b.len { b.len - at } else { 0 };
        if room == 0 {
            return 0;
        }
        let mut n;
        loop {
            n = libc::recv(fd, b.data.add(at).cast(), room, 0);
            if !(n < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if n < 0 {
            if errno() == libc::EAGAIN || errno() == libc::EWOULDBLOCK {
                kelp_panic(c"net.recv_into: timed out".as_ptr());
            }
            kelp_panic_errno(c"net.recv_into: receive failed".as_ptr());
        }
        *b.data.add(at + n as usize) = 0;
        n as usize
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__set_timeout_raw(fd: c_int, ms: u64) {
    unsafe {
        let tv = libc::timeval {
            tv_sec: (ms / 1000) as libc::time_t,
            tv_usec: ((ms % 1000) * 1000) as libc::suseconds_t,
        };
        let len = core::mem::size_of::<libc::timeval>() as libc::socklen_t;
        let p = (&tv as *const libc::timeval).cast::<c_void>();
        if libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, p, len) != 0 {
            kelp_panic_errno(c"net.set_timeout: cannot set the receive timeout".as_ptr());
        }
        if libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_SNDTIMEO, p, len) != 0 {
            kelp_panic_errno(c"net.set_timeout: cannot set the send timeout".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__local_port_raw(fd: c_int) -> u16 {
    unsafe {
        let mut ss: libc::sockaddr_storage = core::mem::zeroed();
        let mut len = core::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, (&mut ss as *mut libc::sockaddr_storage).cast(), &mut len) != 0 {
            kelp_panic_errno(c"net.port: getsockname failed".as_ptr());
        }
        if ss.ss_family as c_int == libc::AF_INET {
            let sin = &*(&ss as *const libc::sockaddr_storage as *const libc::sockaddr_in);
            return u16::from_be(sin.sin_port);
        }
        if ss.ss_family as c_int == libc::AF_INET6 {
            let sin6 = &*(&ss as *const libc::sockaddr_storage as *const libc::sockaddr_in6);
            return u16::from_be(sin6.sin6_port);
        }
        kelp_panic(c"net.port: unknown address family".as_ptr());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__peer_addr_raw(fd: c_int) -> KString {
    unsafe {
        let mut ss: libc::sockaddr_storage = core::mem::zeroed();
        let mut len = core::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getpeername(fd, (&mut ss as *mut libc::sockaddr_storage).cast(), &mut len) != 0 {
            kelp_panic_errno(c"net.peer_addr: getpeername failed".as_ptr());
        }
        let mut host = [0 as c_char; INET6_ADDRSTRLEN];
        let mut text = [0 as c_char; INET6_ADDRSTRLEN + 16];
        if ss.ss_family as c_int == libc::AF_INET {
            let sin = &*(&ss as *const libc::sockaddr_storage as *const libc::sockaddr_in);
            inet_ntop(libc::AF_INET, (&sin.sin_addr as *const libc::in_addr).cast(), host.as_mut_ptr(), host.len() as libc::socklen_t);
            libc::snprintf(text.as_mut_ptr(), text.len(), c"%s:%u".as_ptr(), host.as_ptr(), u16::from_be(sin.sin_port) as libc::c_uint);
        } else if ss.ss_family as c_int == libc::AF_INET6 {
            let sin6 = &*(&ss as *const libc::sockaddr_storage as *const libc::sockaddr_in6);
            inet_ntop(libc::AF_INET6, (&sin6.sin6_addr as *const libc::in6_addr).cast(), host.as_mut_ptr(), host.len() as libc::socklen_t);
            libc::snprintf(text.as_mut_ptr(), text.len(), c"[%s]:%u".as_ptr(), host.as_ptr(), u16::from_be(sin6.sin6_port) as libc::c_uint);
        } else {
            kelp_panic(c"net.peer_addr: unknown address family".as_ptr());
        }
        kelp_String__from_str(text.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__resolve_raw(host: *const c_char) -> KString {
    unsafe {
        let mut hints: libc::addrinfo = core::mem::zeroed();
        hints.ai_family = libc::AF_UNSPEC;
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut list: *mut libc::addrinfo = null_mut();
        let rc = libc::getaddrinfo(host, null(), &hints, &mut list);
        if rc != 0 {
            kelp_panic_gai(c"net.resolve: cannot resolve host".as_ptr(), rc);
        }
        // One address per line, in the order `getaddrinfo` reported them, with
        // duplicates dropped.
        let mut out = [0 as c_char; 4096];
        let mut at = 0usize;
        let mut seen = [[0 as c_char; INET6_ADDRSTRLEN]; 16];
        let mut seen_n = 0usize;
        let mut a = list;
        while !a.is_null() {
            let src: *const c_void = if (*a).ai_family == libc::AF_INET {
                (&(*(*a).ai_addr.cast::<libc::sockaddr_in>()).sin_addr as *const libc::in_addr).cast()
            } else if (*a).ai_family == libc::AF_INET6 {
                (&(*(*a).ai_addr.cast::<libc::sockaddr_in6>()).sin6_addr as *const libc::in6_addr).cast()
            } else {
                a = (*a).ai_next;
                continue;
            };
            let mut h = [0 as c_char; INET6_ADDRSTRLEN];
            if inet_ntop((*a).ai_family, src, h.as_mut_ptr(), h.len() as libc::socklen_t).is_null() {
                a = (*a).ai_next;
                continue;
            }
            let dup = seen[..seen_n].iter().any(|s| libc::strcmp(s.as_ptr(), h.as_ptr()) == 0);
            if dup {
                a = (*a).ai_next;
                continue;
            }
            if seen_n < 16 {
                libc::snprintf(seen[seen_n].as_mut_ptr(), INET6_ADDRSTRLEN, c"%s".as_ptr(), h.as_ptr());
                seen_n += 1;
            }
            let hl = libc::strlen(h.as_ptr());
            if at + hl + 1 >= out.len() {
                break;
            }
            if at > 0 {
                out[at] = b'\n' as c_char;
                at += 1;
            }
            core::ptr::copy_nonoverlapping(h.as_ptr(), out.as_mut_ptr().add(at), hl);
            at += hl;
            out[at] = 0;
            a = (*a).ai_next;
        }
        libc::freeaddrinfo(list);
        kelp_String__from_str(out.as_ptr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// `net.recv(s, 0)` returns an empty `Bytes` at once, the same answer
    /// `recv_into` gives for a zero length. A blocking `recv(2)` with a length of
    /// 0 waits for a peer event instead, so a body without the short-circuit
    /// spends the 250 ms receive timeout. The measured call time separates a
    /// returned call from a waited one, so a loaded machine does not flake it.
    #[test]
    fn recv_raw_with_zero_length_returns_an_empty_bytes() {
        let mut sv = [0 as c_int; 2];
        let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) };
        assert_eq!(rc, 0, "socketpair");
        let tv = libc::timeval { tv_sec: 0, tv_usec: 250_000 };
        unsafe {
            libc::setsockopt(
                sv[0],
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&tv as *const libc::timeval).cast(),
                core::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
        let start = Instant::now();
        let mut got = unsafe { kelp_std__net__recv_raw(sv[0], 0) };
        let elapsed = start.elapsed();
        assert_eq!(got.len, 0, "recv_raw(fd, 0) returns no bytes");
        assert!(elapsed < Duration::from_millis(50), "recv_raw(fd, 0) waited {elapsed:?}, so it blocked");
        unsafe {
            crate::text::kelp_bytes_drop(&mut got);
            libc::close(sv[0]);
            libc::close(sv[1]);
        }
    }
}

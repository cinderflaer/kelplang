//! Capability glue: the Rust port of `kelpc/runtime/cap_glue.c` (stage 1, WP4).
//!
//! The six exported functions keep the C names and signatures that
//! `declare_cap_fns` in `codegen_llvm.rs` declares. The five helpers are private,
//! as the `static` functions in the C file are. The matcher they call
//! (`glob_match`, `host_glob`, `host_match`, `path_normalize`) is Kelp, compiled
//! into each program, and is declared here as `extern "C"`.
//!
//! `kelp_listen_loopback` is a per-thread flag. Stable Rust cannot export a
//! thread-local symbol, so the flag lives behind a pthread key and is read and
//! written through `kelp_listen_loopback_take` and `kelp_listen_loopback_set`.
//!
//! `snprintf` is kept (variadic FFI), so the text this file formats is the same
//! bytes the C glue formats, truncation included.

use crate::abi::Bytes;
use core::ffi::{CStr, c_char, c_int, c_uint, c_void};
use core::ptr;
use libc::{snprintf, size_t};

/// `PATH_MAX` on linux-gnu (`<limits.h>`).
const PATH_MAX: usize = 4096;

unsafe extern "C" {
    // The matcher: Kelp functions, defined by the program (`std/rt.kp`).
    fn kelp_std__rt__glob_match(pat: *const c_char, s: *const c_char) -> bool;
    fn kelp_std__rt__host_glob(pat: *const c_char, host: *const c_char) -> bool;
    fn kelp_std__rt__host_match(pat: *const c_char, hostport: *const c_char) -> bool;
    fn kelp_std__rt__path_normalize(path: *const c_char, out: *mut Bytes) -> bool;

    // libc items the `libc` crate does not bind here: `stderr` is a data symbol,
    // and `fprintf` takes a `FILE*` this crate never dereferences.
    fn fprintf(stream: *mut c_void, format: *const c_char, ...) -> c_int;
    static stderr: *mut c_void;
    fn inet_pton(af: c_int, src: *const c_char, dst: *mut c_void) -> c_int;
}

/// `kelp_collapse`: collapses `//`, `.` and `..` segments of an absolute path,
/// in place. `..` at the root stays at the root. Used on the text a symlink's
/// target is spliced into, which has not been through the normalizer.
///
/// # Safety
/// `p` must point to a NUL-terminated buffer of at least `PATH_MAX` bytes.
unsafe fn collapse(p: *mut c_char) {
    let mut o = [0u8; PATH_MAX];
    let mut ol = 0usize;
    unsafe {
        let len = CStr::from_ptr(p).count_bytes();
        let src = core::slice::from_raw_parts(p as *const u8, len);
        let mut i = 0usize;
        while i < len {
            while i < len && src[i] == b'/' {
                i += 1;
            }
            if i >= len {
                break;
            }
            let start = i;
            while i < len && src[i] != b'/' {
                i += 1;
            }
            let seg = &src[start..i];
            if seg == b"." {
                // dropped
            } else if seg == b".." {
                while ol > 0 && o[ol - 1] != b'/' {
                    ol -= 1;
                }
                if ol > 0 {
                    ol -= 1;
                }
            } else if ol + 1 + seg.len() < PATH_MAX {
                o[ol] = b'/';
                o[ol + 1..ol + 1 + seg.len()].copy_from_slice(seg);
                ol += 1 + seg.len();
            }
        }
        if ol == 0 {
            o[0] = b'/';
            ol = 1;
        }
        o[ol] = 0;
        ptr::copy_nonoverlapping(o.as_ptr(), p as *mut u8, ol + 1);
    }
}

/// `kelp_resolve_path`: resolves symlinks in `path` into `out` (at most `cap`
/// bytes). The deepest existing ancestor is resolved with `realpath`; the rest
/// is appended verbatim, except that a dangling first missing component is
/// followed by hand, up to 40 hops, like the kernel does.
///
/// # Safety
/// `path` is NUL-terminated; `out` has room for `cap` bytes.
unsafe fn resolve_path(path: *const c_char, out: *mut c_char, cap: usize) -> bool {
    unsafe {
        let mut work = [0 as c_char; PATH_MAX];
        let wp = work.as_mut_ptr();
        snprintf(wp, PATH_MAX as size_t, c"%s".as_ptr(), path);
        for _hops in 0..=40 {
            let mut real = [0 as c_char; PATH_MAX];
            if !libc::realpath(wp, real.as_mut_ptr()).is_null() {
                snprintf(out, cap as size_t, c"%s".as_ptr(), real.as_ptr());
                return true;
            }
            let mut buf = [0 as c_char; PATH_MAX];
            let bp = buf.as_mut_ptr();
            snprintf(bp, PATH_MAX as size_t, c"%s".as_ptr(), wp);
            let mut slash = libc::strrchr(bp, b'/' as c_int);
            let mut tail = 0usize;
            let mut found = false;
            while !slash.is_null() && slash != bp {
                tail = slash.offset_from(bp) as usize;
                *slash = 0;
                if !libc::realpath(bp, real.as_mut_ptr()).is_null() {
                    found = true;
                    break;
                }
                slash = libc::strrchr(bp, b'/' as c_int);
            }
            if !found {
                return false;
            }
            // `tail` is an offset into `work`, which is the same text as `buf`
            // before the cut.
            let rest = wp.add(tail);
            let end = libc::strchr(rest.add(1), b'/' as c_int);
            let first_len = if end.is_null() {
                libc::strlen(rest)
            } else {
                end.offset_from(rest) as usize
            };
            let pre: *const c_char = if libc::strcmp(real.as_ptr(), c"/".as_ptr()) == 0 {
                c"".as_ptr()
            } else {
                real.as_ptr()
            };
            let mut cand = [0 as c_char; PATH_MAX];
            snprintf(
                cand.as_mut_ptr(),
                PATH_MAX as size_t,
                c"%s%.*s".as_ptr(),
                pre,
                first_len as c_int,
                rest,
            );
            let mut link = [0 as c_char; PATH_MAX];
            let n = libc::readlink(cand.as_ptr(), link.as_mut_ptr(), PATH_MAX - 1);
            if n < 0 {
                // Not a link: the component is really absent, and so is all
                // of what follows it.
                snprintf(out, cap as size_t, c"%s%s".as_ptr(), pre, rest);
                return true;
            }
            link[n as usize] = 0;
            let after: *const c_char = if end.is_null() { c"".as_ptr() } else { end };
            let mut next = [0 as c_char; PATH_MAX];
            if link[0] == b'/' as c_char {
                snprintf(
                    next.as_mut_ptr(),
                    PATH_MAX as size_t,
                    c"%s%s".as_ptr(),
                    link.as_ptr(),
                    after,
                );
            } else {
                snprintf(
                    next.as_mut_ptr(),
                    PATH_MAX as size_t,
                    c"%s/%s%s".as_ptr(),
                    pre,
                    link.as_ptr(),
                    after,
                );
            }
            collapse(next.as_mut_ptr());
            snprintf(wp, PATH_MAX as size_t, c"%s".as_ptr(), next.as_ptr());
        }
        // A chain of more than 40 links: the kernel refuses it too (ELOOP), so
        // what is matched is the path as far as it got.
        snprintf(out, cap as size_t, c"%s".as_ptr(), wp);
        true
    }
}

/// `kelp_cap_deny`: prints the denial and the granted list to stderr, then
/// exits with status 1.
///
/// # Safety
/// `patterns` holds `count` NUL-terminated strings; the other pointers are
/// NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_deny(
    domain: *const c_char,
    path: *const c_char,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) -> ! {
    unsafe {
        let err = stderr;
        fprintf(
            err,
            c"%s: panic: '%s' is outside the %s capability granted to module '%s'\n".as_ptr(),
            at,
            path,
            domain,
            module,
        );
        if count == 0 {
            fprintf(err, c"  granted: (nothing)\n".as_ptr());
        } else {
            fprintf(err, c"  granted:\n".as_ptr());
            for i in 0..count {
                fprintf(err, c"    %s\n".as_ptr(), *patterns.add(i));
            }
        }
        libc::exit(1)
    }
}

/// `kelp_cap_check_path`: a path argument is normalized, then resolved through
/// symlinks, and only then matched. A link inside the grant that points outside
/// it is denied.
///
/// # Safety
/// The arguments are as for `kelp_cap_deny`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_check_path(
    domain: *const c_char,
    path: *const c_char,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) {
    unsafe {
        // One byte past PATH_MAX: the normalizer's view has `len == PATH_MAX`,
        // and `Bytes` keeps its NUL in the slot at `len`.
        let mut normalized = [0u8; PATH_MAX + 1];
        let mut view = Bytes {
            data: normalized.as_mut_ptr(),
            len: PATH_MAX,
            cap: PATH_MAX,
        };
        if !kelp_std__rt__path_normalize(path, &mut view) {
            kelp_cap_deny(domain, path, patterns, count, module, at);
        }
        let norm = normalized.as_ptr() as *const c_char;
        let mut resolved = [0 as c_char; PATH_MAX];
        if !resolve_path(norm, resolved.as_mut_ptr(), PATH_MAX) {
            snprintf(
                resolved.as_mut_ptr(),
                PATH_MAX as size_t,
                c"%s".as_ptr(),
                norm,
            );
        }
        for i in 0..count {
            if kelp_std__rt__glob_match(*patterns.add(i), resolved.as_ptr()) {
                return;
            }
        }
        kelp_cap_deny(domain, resolved.as_ptr(), patterns, count, module, at);
    }
}

/// `kelp_cap_check_name`: a name argument (an environment variable name) is
/// matched as written, since a name has no path structure to normalize.
///
/// # Safety
/// The arguments are as for `kelp_cap_deny`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_check_name(
    domain: *const c_char,
    name: *const c_char,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) {
    unsafe {
        for i in 0..count {
            let pat = *patterns.add(i);
            // A name domain's whole-resource grant is the lone `*`, and a `*`
            // stays within one segment, so its verdict is "the name has no /".
            if *pat == b'*' as c_char && *pat.add(1) == 0 {
                if libc::strchr(name, b'/' as c_int).is_null() {
                    return;
                }
                continue;
            }
            if kelp_std__rt__glob_match(pat, name) {
                return;
            }
        }
        kelp_cap_deny(domain, name, patterns, count, module, at);
    }
}

/// Whether a pattern's host part is numeric-shaped: only digits, dots and `*`,
/// with at least one digit. Such a pattern is a statement about IPv4 addresses.
///
/// # Safety
/// `pat` has at least `host_len` readable bytes.
unsafe fn host_pattern_is_numeric(pat: *const c_char, host_len: usize) -> bool {
    unsafe {
        let mut digit = false;
        for i in 0..host_len {
            let c = *pat.add(i) as u8;
            if c.is_ascii_digit() {
                digit = true;
            } else if c != b'.' && c != b'*' {
                return false;
            }
        }
        digit
    }
}

/// The length of a pattern's host part: everything before its last `:`.
///
/// # Safety
/// `pat` is NUL-terminated.
unsafe fn pattern_host_len(pat: *const c_char) -> usize {
    unsafe {
        let colon = libc::strrchr(pat, b':' as c_int);
        if colon.is_null() {
            libc::strlen(pat)
        } else {
            colon.offset_from(pat) as usize
        }
    }
}

/// A host that a numeric-shaped pattern admitted must itself be a dotted-quad
/// IPv4 literal, as `inet_pton` accepts it.
///
/// # Safety
/// `pat` and `host` are NUL-terminated.
unsafe fn numeric_pattern_admits(pat: *const c_char, host: *const c_char) -> bool {
    unsafe {
        if !host_pattern_is_numeric(pat, pattern_host_len(pat)) {
            return true;
        }
        let mut addr = libc::in_addr { s_addr: 0 };
        inet_pton(
            libc::AF_INET,
            host,
            &mut addr as *mut libc::in_addr as *mut c_void,
        ) == 1
    }
}

/// Copies the host part of `pat` (everything before `colon`) into `head`,
/// truncated the way the C `memcpy` at `sizeof head - 1` truncates it.
///
/// # Safety
/// `colon` points into the NUL-terminated `pat`.
unsafe fn host_part(pat: *const c_char, colon: *const c_char, head: &mut [c_char; PATH_MAX]) {
    unsafe {
        let mut n = colon.offset_from(pat) as usize;
        if n >= PATH_MAX {
            n = PATH_MAX - 1;
        }
        libc::memcpy(head.as_mut_ptr() as *mut c_void, pat as *const c_void, n);
        head[n] = 0;
    }
}

/// `kelp_cap_check_host`: a host on its own (`net.resolve`). Resolution is not
/// port-specific, so a pattern's port, if it has one, is ignored.
///
/// # Safety
/// The arguments are as for `kelp_cap_deny`, with `host` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_check_host(
    domain: *const c_char,
    host: *const c_char,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) {
    unsafe {
        for i in 0..count {
            let pat = *patterns.add(i);
            if libc::strcmp(pat, c"*".as_ptr()) == 0 {
                return;
            }
            if kelp_std__rt__host_glob(pat, host) && numeric_pattern_admits(pat, host) {
                return;
            }
            let colon = libc::strrchr(pat, b':' as c_int);
            if !colon.is_null() {
                let mut head = [0 as c_char; PATH_MAX];
                host_part(pat, colon, &mut head);
                if kelp_std__rt__host_glob(head.as_ptr(), host)
                    && numeric_pattern_admits(head.as_ptr(), host)
                {
                    return;
                }
            }
        }
        kelp_cap_deny(domain, host, patterns, count, module, at);
    }
}

/// `kelp_cap_check_hostport`: a host and a port together. The port is the
/// call's argument, so the checked text is the one the code named.
///
/// # Safety
/// The arguments are as for `kelp_cap_check_host`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_check_hostport(
    domain: *const c_char,
    host: *const c_char,
    port: u16,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) {
    unsafe {
        let mut hostport = [0 as c_char; PATH_MAX];
        snprintf(
            hostport.as_mut_ptr(),
            PATH_MAX as size_t,
            c"%s:%u".as_ptr(),
            host,
            port as c_uint,
        );
        for i in 0..count {
            let pat = *patterns.add(i);
            if kelp_std__rt__host_match(pat, hostport.as_ptr())
                && numeric_pattern_admits(pat, host)
            {
                return;
            }
        }
        kelp_cap_deny(domain, hostport.as_ptr(), patterns, count, module, at);
    }
}

/// `kelp_cap_check_port`: a port on its own (`net.listen`). The grant's port
/// governs the listener, and its host decides which interfaces the listener may
/// bind. The verdict is left in the loopback flag for the `listen` that follows:
/// a pattern that admits `0.0.0.0` (or names no host) admits every interface,
/// and anything else gets loopback only.
///
/// # Safety
/// The arguments are as for `kelp_cap_deny`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_cap_check_port(
    domain: *const c_char,
    port: u16,
    patterns: *const *const c_char,
    count: size_t,
    module: *const c_char,
    at: *const c_char,
) {
    unsafe {
        let mut number = [0 as c_char; 16];
        snprintf(
            number.as_mut_ptr(),
            16,
            c"%u".as_ptr(),
            port as c_uint,
        );
        let mut text = [0 as c_char; 32];
        snprintf(text.as_mut_ptr(), 32, c"*:%s".as_ptr(), number.as_ptr());
        let mut port_ok = false;
        let mut every_interface = false;
        for i in 0..count {
            let pat = *patterns.add(i);
            if libc::strcmp(pat, c"*".as_ptr()) == 0 {
                port_ok = true;
                every_interface = true;
                continue;
            }
            let colon = libc::strrchr(pat, b':' as c_int);
            if colon.is_null() {
                // A host-only pattern constrains no port and names no interface
                // unless it is `*` or admits 0.0.0.0.
                port_ok = true;
                let mut head = [0 as c_char; PATH_MAX];
                snprintf(head.as_mut_ptr(), PATH_MAX as size_t, c"%s".as_ptr(), pat);
                if libc::strcmp(head.as_ptr(), c"*".as_ptr()) == 0
                    || kelp_std__rt__host_glob(head.as_ptr(), c"0.0.0.0".as_ptr())
                {
                    every_interface = true;
                }
                continue;
            }
            if kelp_std__rt__host_glob(colon.add(1), number.as_ptr()) {
                port_ok = true;
                let mut head = [0 as c_char; PATH_MAX];
                host_part(pat, colon, &mut head);
                if libc::strcmp(head.as_ptr(), c"*".as_ptr()) == 0
                    || kelp_std__rt__host_glob(head.as_ptr(), c"0.0.0.0".as_ptr())
                {
                    every_interface = true;
                }
            }
        }
        if !port_ok {
            kelp_cap_deny(domain, text.as_ptr(), patterns, count, module, at);
        }
        kelp_listen_loopback_set(if every_interface { 0 } else { 1 });
    }
}

// ---- the loopback flag -----------------------------------------------------

static mut LOOPBACK_KEY: libc::pthread_key_t = 0;
static mut LOOPBACK_ONCE: libc::pthread_once_t = libc::PTHREAD_ONCE_INIT;

extern "C" fn make_loopback_key() {
    unsafe {
        libc::pthread_key_create(&raw mut LOOPBACK_KEY, None);
    }
}

/// The pthread key that holds this thread's loopback flag, created once.
///
/// # Safety
/// Called only from the runtime's own C-ABI entry points.
unsafe fn loopback_key() -> libc::pthread_key_t {
    unsafe {
        libc::pthread_once(&raw mut LOOPBACK_ONCE, make_loopback_key);
        LOOPBACK_KEY
    }
}

/// Stores `v` as this thread's loopback flag and returns it. Replaces the C
/// `_Thread_local int kelp_listen_loopback` write.
///
/// # Safety
/// Safe to call from any thread; the flag is per thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_listen_loopback_set(v: c_int) -> c_int {
    unsafe {
        libc::pthread_setspecific(loopback_key(), v as isize as usize as *const c_void);
    }
    v
}

/// Returns this thread's loopback flag and clears it. Replaces the C read in
/// the `net.listen` body.
///
/// # Safety
/// Safe to call from any thread; the flag is per thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_listen_loopback_take() -> c_int {
    unsafe {
        let key = loopback_key();
        let v = libc::pthread_getspecific(key) as usize as c_int;
        libc::pthread_setspecific(key, ptr::null());
        v
    }
}

#[cfg(test)]
mod tests {
    //! Each function is compared with the C glue (`kelpc/runtime/cap_glue.c`),
    //! compiled with `cc` into an oracle binary that links the same text. The
    //! Kelp matcher is a stand-in in both: a `*` matches within one segment and
    //! `**` matches across segments, and `path_normalize` collapses the path.
    //!
    //! A denial exits the process, so a checked call runs in a child process: the
    //! test binary re-runs `cap_child` with the arguments in the environment.

    use super::*;
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- Kelp matcher stand-ins (the program defines these in real builds) ----

    fn glob(p: &[u8], s: &[u8]) -> bool {
        if p.is_empty() {
            return s.is_empty();
        }
        if p[0] == b'*' {
            if p.len() > 1 && p[1] == b'*' {
                return (0..=s.len()).any(|t| glob(&p[2..], &s[t..]));
            }
            for t in 0..=s.len() {
                if glob(&p[1..], &s[t..]) {
                    return true;
                }
                if t == s.len() || s[t] == b'/' {
                    return false;
                }
            }
            return false;
        }
        !s.is_empty() && s[0] == p[0] && glob(&p[1..], &s[1..])
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kelp_std__rt__glob_match(pat: *const c_char, s: *const c_char) -> bool {
        unsafe { glob(CStr::from_ptr(pat).to_bytes(), CStr::from_ptr(s).to_bytes()) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kelp_std__rt__host_glob(pat: *const c_char, host: *const c_char) -> bool {
        unsafe { glob(CStr::from_ptr(pat).to_bytes(), CStr::from_ptr(host).to_bytes()) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kelp_std__rt__host_match(pat: *const c_char, hp: *const c_char) -> bool {
        unsafe { glob(CStr::from_ptr(pat).to_bytes(), CStr::from_ptr(hp).to_bytes()) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kelp_std__rt__path_normalize(path: *const c_char, out: *mut Bytes) -> bool {
        unsafe {
            let p = CStr::from_ptr(path).to_bytes();
            if p.first() != Some(&b'/') {
                return false;
            }
            let mut o: Vec<u8> = Vec::new();
            for seg in p.split(|&b| b == b'/') {
                if seg.is_empty() || seg == b"." {
                    continue;
                }
                if seg == b".." {
                    while let Some(&last) = o.last() {
                        o.pop();
                        if last == b'/' {
                            break;
                        }
                    }
                    continue;
                }
                o.push(b'/');
                o.extend_from_slice(seg);
            }
            if o.is_empty() {
                o.push(b'/');
            }
            if o.len() >= (*out).cap {
                return false;
            }
            ptr::copy_nonoverlapping(o.as_ptr(), (*out).data, o.len());
            *(*out).data.add(o.len()) = 0;
            (*out).len = o.len();
            true
        }
    }

    // ---- the C oracle ----

    const ORACLE_HEAD: &str = r#"
#include <arpa/inet.h>
#include <errno.h>
#include <limits.h>
#include <netdb.h>
#include <netinet/in.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;
_Thread_local int kelp_listen_loopback = 0;

static bool gm(const char* p, const char* s) {
    if (*p == 0) return *s == 0;
    if (*p == '*') {
        if (p[1] == '*') {
            for (const char* t = s;; t++) {
                if (gm(p + 2, t)) return true;
                if (*t == 0) return false;
            }
        }
        for (const char* t = s;; t++) {
            if (gm(p + 1, t)) return true;
            if (*t == 0 || *t == '/') return false;
        }
    }
    return *s == *p && gm(p + 1, s + 1);
}
bool kelp_std__rt__glob_match(const char* pat, const char* s) { return gm(pat, s); }
bool kelp_std__rt__host_glob(const char* pat, const char* host) { return gm(pat, host); }
bool kelp_std__rt__host_match(const char* pat, const char* hp) { return gm(pat, hp); }
bool kelp_std__rt__path_normalize(const char* path, kelp_Bytes* out) {
    if (path[0] != '/') return false;
    char o[PATH_MAX];
    size_t ol = 0;
    const char* s = path;
    while (*s) {
        while (*s == '/') s++;
        if (!*s) break;
        const char* e = s;
        while (*e && *e != '/') e++;
        size_t l = (size_t)(e - s);
        if (l == 1 && s[0] == '.') {
        } else if (l == 2 && s[0] == '.' && s[1] == '.') {
            while (ol > 0 && o[ol - 1] != '/') ol--;
            if (ol > 0) ol--;
        } else if (ol + 1 + l < sizeof o) {
            o[ol++] = '/';
            memcpy(o + ol, s, l);
            ol += l;
        }
        s = e;
    }
    if (ol == 0) o[ol++] = '/';
    if (ol >= out->cap) return false;
    memcpy(out->data, o, ol);
    out->data[ol] = 0;
    out->len = ol;
    return true;
}
"#;

    const ORACLE_MAIN: &str = r#"
int main(int argc, char** argv) {
    const char* f = argv[1];
    if (strcmp(f, "collapse") == 0) {
        char buf[PATH_MAX];
        snprintf(buf, sizeof buf, "%s", argv[2]);
        kelp_collapse(buf);
        printf("%s\n", buf);
        return 0;
    }
    if (strcmp(f, "resolve") == 0) {
        char out[PATH_MAX] = {0};
        bool ok = kelp_resolve_path(argv[2], out, sizeof out);
        printf("%d %s\n", ok ? 1 : 0, out);
        return 0;
    }
    if (strcmp(f, "numeric") == 0) {
        size_t n = (size_t)strtoul(argv[3], NULL, 10);
        printf("%d\n", kelp_host_pattern_is_numeric(argv[2], n) ? 1 : 0);
        return 0;
    }
    if (strcmp(f, "hostlen") == 0) {
        printf("%zu\n", kelp_pattern_host_len(argv[2]));
        return 0;
    }
    if (strcmp(f, "admits") == 0) {
        printf("%d\n", kelp_numeric_pattern_admits(argv[2], argv[3]) ? 1 : 0);
        return 0;
    }
    /* check functions: argv[2]=domain argv[3]=a argv[4]=port argv[5]=module
       argv[6]=at argv[7..]=patterns */
    const char* domain = argv[2];
    const char* a = argv[3];
    uint16_t port = (uint16_t)strtoul(argv[4], NULL, 10);
    const char* module = argv[5];
    const char* at = argv[6];
    int n = argc - 7;
    const char** pats = malloc(sizeof(char*) * (size_t)(n + 1));
    for (int i = 0; i < n; i++) pats[i] = argv[7 + i];
    pats[n] = NULL;
    if (strcmp(f, "deny") == 0) {
        kelp_cap_deny(domain, a, pats, (size_t)n, module, at);
    } else if (strcmp(f, "check_path") == 0) {
        kelp_cap_check_path(domain, a, pats, (size_t)n, module, at);
    } else if (strcmp(f, "check_name") == 0) {
        kelp_cap_check_name(domain, a, pats, (size_t)n, module, at);
    } else if (strcmp(f, "check_host") == 0) {
        kelp_cap_check_host(domain, a, pats, (size_t)n, module, at);
    } else if (strcmp(f, "check_hostport") == 0) {
        kelp_cap_check_hostport(domain, a, port, pats, (size_t)n, module, at);
    } else if (strcmp(f, "check_port") == 0) {
        kelp_cap_check_port(domain, port, pats, (size_t)n, module, at);
    }
    printf("ok loopback=%d\n", kelp_listen_loopback);
    return 0;
}
"#;

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelp-rt-wp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn oracle() -> &'static PathBuf {
        static BIN: OnceLock<PathBuf> = OnceLock::new();
        BIN.get_or_init(|| {
            let glue = std::fs::canonicalize(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../kelpc/runtime/cap_glue.c"
            ))
            .expect("cap_glue.c");
            let dir = scratch_dir();
            let src = dir.join("oracle.c");
            // The glue is compiled the way the C backend splices it: `static`
            // becomes external linkage for the duration of the include.
            let text = format!(
                "{ORACLE_HEAD}\n#include <limits.h>\n#define static\n#include \"{}\"\n#undef static\n{ORACLE_MAIN}",
                glue.display()
            );
            std::fs::write(&src, text).expect("write oracle source");
            let bin = dir.join("oracle");
            // The glue is C text compiled as the C backend compiles it; its
            // own `-Wformat-truncation` notes are not this test's concern.
            let status = Command::new("cc")
                .arg("-w")
                .arg("-o")
                .arg(&bin)
                .arg(&src)
                .status()
                .expect("cc");
            assert!(status.success(), "building the C oracle failed");
            bin
        })
    }

    struct Outcome {
        status: Option<i32>,
        stdout: String,
        stderr: String,
    }

    fn run_oracle(argv: &[String]) -> Outcome {
        let out = Command::new(oracle())
            .args(argv)
            .output()
            .expect("run oracle");
        Outcome {
            status: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    static NEXT_CHILD: AtomicUsize = AtomicUsize::new(0);

    /// Runs one checked call in a child process of this test binary.
    fn run_rust_child(argv: &[String]) -> Outcome {
        let n = NEXT_CHILD.fetch_add(1, Ordering::Relaxed);
        let out_file = scratch_dir().join(format!("child-{}-{n}.out", std::process::id()));
        let _ = std::fs::remove_file(&out_file);
        let exe = std::env::current_exe().expect("test binary");
        let out = Command::new(exe)
            .args(["--exact", "cap::tests::cap_child", "--nocapture"])
            .env("KELP_CAP_CHILD", argv.join("\u{1f}"))
            .env("KELP_CAP_OUT", &out_file)
            .output()
            .expect("run child");
        Outcome {
            status: out.status.code(),
            stdout: std::fs::read_to_string(&out_file).unwrap_or_default(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// The child side: performs the call named in `KELP_CAP_CHILD` and records
    /// the loopback flag in `KELP_CAP_OUT`. A denial exits from inside the call.
    #[test]
    fn cap_child() {
        let Ok(joined) = std::env::var("KELP_CAP_CHILD") else {
            return;
        };
        let out_file = std::env::var("KELP_CAP_OUT").expect("KELP_CAP_OUT");
        let args: Vec<String> = joined.split('\u{1f}').map(str::to_owned).collect();
        let func = args[0].as_str();
        let cs = |s: &str| CString::new(s).expect("nul-free argument");
        let domain = cs(&args[1]);
        let a = cs(&args[2]);
        let port: u16 = args[3].parse().expect("port");
        let module = cs(&args[4]);
        let at = cs(&args[5]);
        let pats: Vec<CString> = args[6..].iter().map(|p| cs(p)).collect();
        let ptrs: Vec<*const c_char> = pats.iter().map(|p| p.as_ptr()).collect();
        let (pp, n) = (ptrs.as_ptr(), ptrs.len());
        unsafe {
            match func {
                "deny" => kelp_cap_deny(domain.as_ptr(), a.as_ptr(), pp, n, module.as_ptr(), at.as_ptr()),
                "check_path" => kelp_cap_check_path(domain.as_ptr(), a.as_ptr(), pp, n, module.as_ptr(), at.as_ptr()),
                "check_name" => kelp_cap_check_name(domain.as_ptr(), a.as_ptr(), pp, n, module.as_ptr(), at.as_ptr()),
                "check_host" => kelp_cap_check_host(domain.as_ptr(), a.as_ptr(), pp, n, module.as_ptr(), at.as_ptr()),
                "check_hostport" => kelp_cap_check_hostport(domain.as_ptr(), a.as_ptr(), port, pp, n, module.as_ptr(), at.as_ptr()),
                "check_port" => kelp_cap_check_port(domain.as_ptr(), port, pp, n, module.as_ptr(), at.as_ptr()),
                other => panic!("unknown child call {other}"),
            }
            let loopback = kelp_listen_loopback_take();
            std::fs::write(out_file, format!("ok loopback={loopback}\n")).expect("child result");
        }
    }

    /// Runs a checked call through both the oracle and the Rust port and asserts
    /// that exit status, stderr and stdout agree. Returns the Rust outcome.
    fn check_both(func: &str, a: &str, port: u16, pats: &[&str]) -> Outcome {
        let mut argv: Vec<String> = [func, "fs", a].iter().map(|s| s.to_string()).collect();
        argv.push(port.to_string());
        argv.push("app.main".into());
        argv.push("t.at".into());
        argv.extend(pats.iter().map(|p| p.to_string()));
        let c = run_oracle(&argv);
        let r = run_rust_child(&argv);
        assert_eq!(r.status, c.status, "exit status for {argv:?}");
        assert_eq!(r.stderr, c.stderr, "stderr for {argv:?}");
        assert_eq!(r.stdout, c.stdout, "stdout for {argv:?}");
        r
    }

    fn passes(o: &Outcome) -> bool {
        o.status == Some(0)
    }

    fn cbuf(s: &str) -> [c_char; PATH_MAX] {
        let mut b = [0 as c_char; PATH_MAX];
        for (i, &c) in s.as_bytes().iter().enumerate() {
            b[i] = c as c_char;
        }
        b
    }

    fn buf_string(b: &[c_char]) -> String {
        unsafe { CStr::from_ptr(b.as_ptr()).to_str().expect("utf8").to_owned() }
    }

    fn oracle_line(argv: &[&str]) -> String {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = run_oracle(&argv);
        assert_eq!(out.status, Some(0), "oracle {argv:?}: {}", out.stderr);
        out.stdout.trim_end_matches('\n').to_owned()
    }

    #[test]
    fn collapse_matches_c() {
        let cases = [
            "/", "//a//b/", "/a/./b/../c", "/a/b/../../..", "/../x", "/a/..",
            "/a/b/c/../../d", "/x/", "/a/./.", "/..", "/a//..//b",
        ];
        for s in cases {
            let mut b = cbuf(s);
            unsafe { collapse(b.as_mut_ptr()) };
            assert_eq!(buf_string(&b), oracle_line(&["collapse", s]), "collapse({s})");
        }
    }

    #[test]
    fn resolve_path_matches_c() {
        let base = scratch_dir().join("resolve");
        std::fs::create_dir_all(base.join("real")).expect("real dir");
        let link = base.join("link");
        let dangle = base.join("dangle");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&dangle);
        std::os::unix::fs::symlink("/usr", &link).expect("link");
        std::os::unix::fs::symlink("/nonexistent-kelp-wp4/target", &dangle).expect("dangle");
        let b = base.display().to_string();
        let owned = [
            "/".to_string(),
            "/usr/../usr/lib".to_string(),
            "/nonexistent-kelp-wp4/a/../b".to_string(),
            format!("{b}/link/lib/x"),
            format!("{b}/dangle/a/b"),
            format!("{b}/real/../real"),
            format!("{b}/missing/deeper/file"),
        ];
        for p in &owned {
            let c = CString::new(p.as_str()).expect("cstring");
            let mut out = [0 as c_char; PATH_MAX];
            let rc = unsafe { resolve_path(c.as_ptr(), out.as_mut_ptr(), PATH_MAX) };
            let rust = format!("{} {}", rc as i32, buf_string(&out));
            assert_eq!(rust, oracle_line(&["resolve", p]), "resolve({p})");
        }
    }

    #[test]
    fn host_pattern_is_numeric_matches_c() {
        let cases: [(&str, usize); 11] = [
            ("127.0.0.1", 9), ("10.0.0.*", 8), ("*", 1), ("example.com", 11),
            ("1.2.3.4:80", 7), ("12ab", 4), ("*.*", 3), ("", 0), ("1.2.3.4", 3),
            ("a", 1), ("7", 1),
        ];
        for (pat, len) in cases {
            let c = CString::new(pat).expect("cstring");
            let rust = if unsafe { host_pattern_is_numeric(c.as_ptr(), len) } { "1" } else { "0" };
            assert_eq!(rust, oracle_line(&["numeric", pat, &len.to_string()]), "numeric({pat}, {len})");
        }
    }

    #[test]
    fn pattern_host_len_matches_c() {
        for pat in ["127.0.0.1:80", "abc", ":80", "a:b:c", "*", "*:*", ""] {
            let c = CString::new(pat).expect("cstring");
            let rust = unsafe { pattern_host_len(c.as_ptr()) }.to_string();
            assert_eq!(rust, oracle_line(&["hostlen", pat]), "pattern_host_len({pat})");
        }
    }

    #[test]
    fn numeric_pattern_admits_matches_c() {
        let cases = [
            ("127.0.0.1:*", "127.0.0.1"), ("10.0.0.*", "10.0.0.5"), ("10.0.0.*", "10.0.0.xyz"),
            ("example.com", "anything"), ("*", "1.2.3.4"), ("1.2.3.4:80", "1.2.3.4"),
            ("1.2.3.4:80", "5.6.7.8"), ("1.2", "1.2"),
        ];
        for (pat, host) in cases {
            let p = CString::new(pat).expect("cstring");
            let h = CString::new(host).expect("cstring");
            let rust = if unsafe { numeric_pattern_admits(p.as_ptr(), h.as_ptr()) } { "1" } else { "0" };
            assert_eq!(rust, oracle_line(&["admits", pat, host]), "admits({pat}, {host})");
        }
    }

    #[test]
    fn cap_deny_matches_c() {
        for pats in [vec![], vec!["/tmp/*", "/var/**"]] {
            let r = check_both("deny", "/etc/passwd", 0, &pats);
            assert_eq!(r.status, Some(1), "deny exits with status 1");
            let granted = if pats.is_empty() {
                "  granted: (nothing)\n".to_string()
            } else {
                let mut s = "  granted:\n".to_string();
                for p in &pats {
                    s.push_str(&format!("    {p}\n"));
                }
                s
            };
            let expected = format!(
                "t.at: panic: '/etc/passwd' is outside the fs capability granted to module 'app.main'\n{granted}"
            );
            assert_eq!(r.stderr, expected);
        }
    }

    #[test]
    fn cap_check_path_matches_c() {
        let base = scratch_dir().join("path");
        std::fs::create_dir_all(base.join("in/target")).expect("grant dir");
        let inside_link = base.join("in/alias");
        let outside_link = base.join("in/out");
        let _ = std::fs::remove_file(&inside_link);
        let _ = std::fs::remove_file(&outside_link);
        std::os::unix::fs::symlink(base.join("in/target"), &inside_link).expect("inside link");
        std::os::unix::fs::symlink("/usr", &outside_link).expect("outside link");
        let b = base.display().to_string();
        let grant = format!("{b}/in/**");
        let grant_star = format!("{b}/in/*");

        let ok_cases: Vec<(String, String)> = vec![
            ("/tmp/x".into(), "/tmp/*".into()),
            ("/tmp//a/./b".into(), "/tmp/**".into()),
            (format!("{b}/in/alias/f"), grant.clone()),
        ];
        for (path, pat) in &ok_cases {
            let r = check_both("check_path", path, 0, &[pat]);
            assert!(passes(&r), "{path} should pass under {pat}");
        }
        let deny_cases: Vec<(String, String)> = vec![
            ("/etc/passwd".into(), "/tmp/*".into()),
            ("/tmp/../etc/x".into(), "/tmp/*".into()),
            ("rel/x".into(), "*".into()),
            (format!("{b}/in/out/bin"), grant.clone()),
            (format!("{b}/in/target/x"), grant_star.clone()),
            ("/tmp/x".into(), "".into()),
        ];
        for (path, pat) in &deny_cases {
            let pats: Vec<&str> = if pat.is_empty() { vec![] } else { vec![pat] };
            let r = check_both("check_path", path, 0, &pats);
            assert_eq!(r.status, Some(1), "{path} should be denied");
        }
        let r = check_both("check_path", "/tmp/x", 0, &[]);
        assert!(r.stderr.ends_with("  granted: (nothing)\n"));
    }

    #[test]
    fn cap_check_name_matches_c() {
        let cases: Vec<(&str, Vec<&str>, bool)> = vec![
            ("HOME", vec!["*"], true),
            ("A/B", vec!["*"], false),
            ("A/B", vec!["A/*"], true),
            ("X", vec!["Y", "*"], true),
            ("X", vec!["Y"], false),
            ("", vec![], false),
        ];
        for (name, pats, expect) in cases {
            let r = check_both("check_name", name, 0, &pats);
            assert_eq!(passes(&r), expect, "check_name({name:?}, {pats:?})");
        }
    }

    #[test]
    fn cap_check_host_matches_c() {
        let cases: Vec<(&str, Vec<&str>, bool)> = vec![
            ("example.com", vec!["*"], true),
            ("example.com", vec!["example.com:*"], true),
            ("10.0.0.xyz", vec!["10.0.0.*"], false),
            ("10.0.0.5", vec!["10.0.0.*"], true),
            ("evil.com", vec!["good.com"], false),
            ("127.0.0.1", vec!["127.0.0.1:80"], true),
        ];
        for (host, pats, expect) in cases {
            let r = check_both("check_host", host, 0, &pats);
            assert_eq!(passes(&r), expect, "check_host({host}, {pats:?})");
        }
    }

    #[test]
    fn cap_check_hostport_matches_c() {
        let cases: Vec<(&str, u16, Vec<&str>, bool)> = vec![
            ("127.0.0.1", 80, vec!["127.0.0.1:*"], true),
            ("127.0.0.1", 81, vec!["127.0.0.1:80"], false),
            ("10.0.0.xyz", 80, vec!["10.0.0.*:*"], false),
            ("example.com", 443, vec!["*:443"], true),
            ("example.com", 443, vec![], false),
        ];
        for (host, port, pats, expect) in cases {
            let r = check_both("check_hostport", host, port, &pats);
            assert_eq!(passes(&r), expect, "check_hostport({host}, {port}, {pats:?})");
        }
    }

    #[test]
    fn cap_check_port_matches_c() {
        // (port, patterns, expected verdict, expected loopback flag after the call)
        let cases: Vec<(u16, Vec<&str>, bool, i32)> = vec![
            (80, vec!["*"], true, 0),
            (80, vec!["127.0.0.1:*"], true, 1),
            (8080, vec!["127.0.0.1:80"], false, 0),
            (80, vec!["0.0.0.0:80"], true, 0),
            (443, vec!["example.com"], true, 1),
            (80, vec!["*:80"], true, 0),
            (9, vec![], false, 0),
        ];
        for (port, pats, expect, loopback) in cases {
            let r = check_both("check_port", "", port, &pats);
            assert_eq!(passes(&r), expect, "check_port({port}, {pats:?})");
            if expect {
                assert_eq!(r.stdout, format!("ok loopback={loopback}\n"), "loopback for {port} {pats:?}");
            }
        }
    }

    #[test]
    fn listen_loopback_accessors_are_per_thread() {
        unsafe {
            assert_eq!(kelp_listen_loopback_take(), 0);
            assert_eq!(kelp_listen_loopback_set(1), 1);
            assert_eq!(std::thread::spawn(|| kelp_listen_loopback_take()).join().unwrap(), 0);
            assert_eq!(kelp_listen_loopback_take(), 1);
            assert_eq!(kelp_listen_loopback_take(), 0);
            kelp_listen_loopback_set(0);
            assert_eq!(kelp_listen_loopback_take(), 0);
        }
    }
}

//! Capability glue: the Rust port of the check functions the C glue
//! (`kelpc/runtime/cap_glue.c`, removed in step C) held (stage 1, WP4).
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

// The matcher's four entry points are Kelp functions the program defines
// (`std/rt.kp`). A program with no gated call defines none of them, but the
// archive member that holds these checks is still pulled in for the others,
// so the references are weak: they resolve when the program defines them and
// stay unresolved (and unreached) when it does not.
core::arch::global_asm!(
    ".weak kelp_std__rt__glob_match",
    ".weak kelp_std__rt__host_glob",
    ".weak kelp_std__rt__host_match",
    ".weak kelp_std__rt__path_normalize",
);

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
    use super::*;

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

    /// The loopback verdict is per thread: a value set on one thread is not
    /// seen on another, and `take` clears it.
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

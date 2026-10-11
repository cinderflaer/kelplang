//! Port of the `std/tls` extern bodies in `kelpc/src/externs/tls_*.c` and of
//! the helpers those bodies shared (`tls_helpers.c`), plus the `std/net`
//! listener body `net_listen_on.c` (the C removal lane, owner decision
//! 2026-10-11).
//!
//! Each symbol is the one the LLVM backend already calls for the extern: `kelp_`
//! plus the mangled qualified name (`std.tls.handshake_try_raw` becomes
//! `kelp_std__tls__handshake_try_raw`). The per-program C object no longer
//! defines these, so the call resolves against this archive. The behaviour
//! follows the C text: the same return values, the same `std/tls` error codes
//! (`ERR_*`), the same fixed sentences, and the same SIGPIPE handling.
//!
//! A program that reaches `std/tls` links `-lssl -lcrypto` (the driver's
//! `uses_tls` flag), which resolves the OpenSSL symbols declared below.
//! The classifiers `kelp_tls_classify_error` and `kelp_tls_classify_verify`
//! also still live in the C header text of `cdecl.rs`, for the bodies that
//! stay in C; the copies here must keep the same answers.

#![allow(non_camel_case_types, non_upper_case_globals)]

use core::cell::UnsafeCell;
use core::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use core::mem::{size_of, zeroed};
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use crate::abi::{Bytes, String as KString};
use crate::panic::kelp_panic;
use crate::text::kelp_String__from_str;

// ---- OpenSSL, the subset these bodies call ------------------------------
//
// The constants are the values in the OpenSSL 3.x headers. Where the C text
// used a macro (`SSL_set_tlsext_host_name`, `SSL_CTX_set_min_proto_version`,
// `SSL_CTX_add_extra_chain_cert`, `BIO_get_mem_data`,
// `SSL_CTX_get_ex_new_index`) the same `ctrl` or call is spelled out.

enum SSL {}
enum SSL_CTX {}
enum SSL_METHOD {}
enum X509 {}
enum X509_NAME {}
enum X509_STORE {}
enum EVP_PKEY {}
enum BIO {}
enum BIO_METHOD {}
enum CRYPTO_EX_DATA {}

const SSL_ERROR_SSL: c_int = 1;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_SYSCALL: c_int = 5;
const SSL_ERROR_ZERO_RETURN: c_int = 6;
const SSL_VERIFY_PEER: c_int = 0x01;
const SSL_VERIFY_FAIL_IF_NO_PEER_CERT: c_int = 0x02;
const TLS1_2_VERSION: c_long = 0x0303;
const TLS1_3_VERSION: c_long = 0x0304;
const SSL_CTRL_EXTRA_CHAIN_CERT: c_int = 14;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
const SSL_OP_CIPHER_SERVER_PREFERENCE: u64 = 1 << 22;
const SSL_TLSEXT_ERR_OK: c_int = 0;
const SSL_TLSEXT_ERR_ALERT_FATAL: c_int = 2;
const OPENSSL_NPN_NEGOTIATED: c_int = 1;
const BIO_CTRL_INFO: c_int = 3;
const CRYPTO_EX_INDEX_SSL_CTX: c_int = 1;
const ERR_REASON_MASK: c_ulong = 0x7F_FFFF;
const X509_V_OK: c_long = 0;
const X509_V_ERR_CERT_NOT_YET_VALID: c_long = 9;
const X509_V_ERR_CERT_HAS_EXPIRED: c_long = 10;
const X509_V_ERR_CRL_NOT_YET_VALID: c_long = 11;
const X509_V_ERR_CRL_HAS_EXPIRED: c_long = 12;
const X509_V_ERR_HOSTNAME_MISMATCH: c_long = 62;
const X509_V_ERR_IP_ADDRESS_MISMATCH: c_long = 64;
// ASN1_STRFLGS_RFC2253 (0x317) | XN_FLAG_SEP_COMMA_PLUS | XN_FLAG_DN_REV |
// XN_FLAG_FN_SN (0) | XN_FLAG_DUMP_UNKNOWN_FIELDS.
const XN_FLAG_RFC2253: c_ulong = 0x317 | (1 << 16) | (1 << 20) | (1 << 24);
const SSL_R_TLSV1_ALERT_PROTOCOL_VERSION: c_int = 1070;
const SSL_R_UNSUPPORTED_PROTOCOL: c_int = 258;
const SSL_R_NO_PROTOCOLS_AVAILABLE: c_int = 191;
const SSL_R_WRONG_VERSION_NUMBER: c_int = 267;
const SSL_R_UNEXPECTED_EOF_WHILE_READING: c_int = 294;

type PemPasswordCb = unsafe extern "C" fn(*mut c_char, c_int, c_int, *mut c_void) -> c_int;
type AlpnSelectCb =
    unsafe extern "C" fn(*mut SSL, *mut *const u8, *mut u8, *const u8, c_uint, *mut c_void) -> c_int;
type ExFree =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut CRYPTO_EX_DATA, c_int, c_long, *mut c_void);


/// The OpenSSL entry points, each resolved on its first call with `dlsym`.
///
/// A link-time reference would put an undefined `SSL_*` symbol into every
/// program that links the runtime, because the managed runtime is one object
/// and the hosted archive may pull the module in. Resolving at call time keeps
/// those programs free of OpenSSL: a program that reaches `std/tls` is linked
/// with `-lssl -lcrypto` (the driver's `uses_tls` flag), so the lookup finds
/// the library, and no other program calls these functions.
macro_rules! ossl {
    ($( fn $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) $(-> $ret:ty)? ; )*) => {
        $(
            #[allow(non_snake_case)]
            unsafe fn $name($($arg: $ty),*) $(-> $ret)? {
                static ADDR: AtomicUsize = AtomicUsize::new(0);
                let mut addr = ADDR.load(Ordering::Relaxed);
                if addr == 0 {
                    addr = resolve(concat!(stringify!($name), "\0"));
                    ADDR.store(addr, Ordering::Relaxed);
                }
                unsafe {
                    let f: unsafe extern "C" fn($($ty),*) $(-> $ret)? =
                        core::mem::transmute::<usize, unsafe extern "C" fn($($ty),*) $(-> $ret)?>(addr);
                    f($($arg),*)
                }
            }
        )*
    };
}

/// The address of an OpenSSL symbol in the program's global scope. A program
/// that reaches `std/tls` is linked with `-lssl -lcrypto`, so a missing symbol
/// means the link was made without them: a compiler bug, reported as a panic.
fn resolve(name: &str) -> usize {
    let addr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const c_char) } as usize;
    if addr == 0 {
        unsafe { kelp_panic(c"tls: OpenSSL is not linked into this program".as_ptr()) };
    }
    addr
}
ossl! {
    fn TLS_client_method() -> *const SSL_METHOD;
    fn TLS_server_method() -> *const SSL_METHOD;
    fn SSL_CTX_new(method: *const SSL_METHOD) -> *mut SSL_CTX;
    fn SSL_CTX_free(ctx: *mut SSL_CTX);
    fn SSL_CTX_ctrl(ctx: *mut SSL_CTX, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_CTX_set_options(ctx: *mut SSL_CTX, op: u64) -> u64;
    fn SSL_CTX_set_verify(ctx: *mut SSL_CTX, mode: c_int, cb: *const c_void);
    fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> c_int;
    fn SSL_CTX_set_alpn_protos(ctx: *mut SSL_CTX, protos: *const u8, len: c_uint) -> c_int;
    fn SSL_CTX_use_certificate(ctx: *mut SSL_CTX, x: *mut X509) -> c_int;
    fn SSL_CTX_use_PrivateKey(ctx: *mut SSL_CTX, key: *mut EVP_PKEY) -> c_int;
    fn SSL_CTX_check_private_key(ctx: *const SSL_CTX) -> c_int;
    fn SSL_CTX_get_cert_store(ctx: *const SSL_CTX) -> *mut X509_STORE;
    fn SSL_CTX_add_client_CA(ctx: *mut SSL_CTX, x: *mut X509) -> c_int;
    fn SSL_CTX_set_session_id_context(ctx: *mut SSL_CTX, sid: *const u8, len: c_uint) -> c_int;
    fn SSL_CTX_set_ex_data(ctx: *mut SSL_CTX, idx: c_int, data: *mut c_void) -> c_int;
    fn SSL_CTX_set_alpn_select_cb(ctx: *mut SSL_CTX, cb: Option<AlpnSelectCb>, arg: *mut c_void);
    fn CRYPTO_get_ex_new_index(
        class_index: c_int,
        argl: c_long,
        argp: *mut c_void,
        new_func: *const c_void,
        dup_func: *const c_void,
        free_func: Option<ExFree>,
    ) -> c_int;
    fn SSL_new(ctx: *mut SSL_CTX) -> *mut SSL;
    fn SSL_free(ssl: *mut SSL);
    fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
    fn SSL_ctrl(ssl: *mut SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_set1_host(ssl: *mut SSL, host: *const c_char) -> c_int;
    fn SSL_connect(ssl: *mut SSL) -> c_int;
    fn SSL_accept(ssl: *mut SSL) -> c_int;
    fn SSL_get_error(ssl: *const SSL, ret: c_int) -> c_int;
    fn SSL_get_verify_result(ssl: *const SSL) -> c_long;
    fn SSL_pending(ssl: *const SSL) -> c_int;
    fn SSL_get0_alpn_selected(ssl: *const SSL, data: *mut *const u8, len: *mut c_uint);
    fn SSL_get1_peer_certificate(ssl: *const SSL) -> *mut X509;
    fn SSL_select_next_proto(
        out: *mut *mut u8,
        outlen: *mut u8,
        server: *const u8,
        server_len: c_uint,
        client: *const u8,
        client_len: c_uint,
    ) -> c_int;
    fn BIO_new(method: *const BIO_METHOD) -> *mut BIO;
    fn BIO_s_mem() -> *const BIO_METHOD;
    fn BIO_free(bio: *mut BIO) -> c_int;
    fn BIO_new_mem_buf(buf: *const c_void, len: c_int) -> *mut BIO;
    fn BIO_ctrl(bio: *mut BIO, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn PEM_read_bio_X509(
        bio: *mut BIO,
        x: *mut *mut X509,
        cb: *const c_void,
        u: *mut c_void,
    ) -> *mut X509;
    fn PEM_read_bio_X509_AUX(
        bio: *mut BIO,
        x: *mut *mut X509,
        cb: *const c_void,
        u: *mut c_void,
    ) -> *mut X509;
    fn PEM_read_bio_PrivateKey(
        bio: *mut BIO,
        key: *mut *mut EVP_PKEY,
        cb: Option<PemPasswordCb>,
        u: *mut c_void,
    ) -> *mut EVP_PKEY;
    fn EVP_PKEY_free(key: *mut EVP_PKEY);
    fn X509_free(x: *mut X509);
    fn X509_STORE_add_cert(store: *mut X509_STORE, x: *mut X509) -> c_int;
    fn X509_get_subject_name(x: *const X509) -> *mut X509_NAME;
    fn X509_NAME_print_ex(bio: *mut BIO, name: *const X509_NAME, indent: c_int, flags: c_ulong)
        -> c_int;
    fn X509_verify_cert_error_string(n: c_long) -> *const c_char;
    fn ERR_get_error() -> c_ulong;
    fn ERR_clear_error();
    fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
}

// ---- libc helpers the bodies need that the libc crate spells differently ----

fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

fn set_errno(value: c_int) {
    unsafe { *libc::__errno_location() = value }
}

/// `snprintf(text, len, "%s", msg)`: the fixed sentences the bodies report.
unsafe fn say(text: *mut c_char, len: usize, msg: &CStr) {
    unsafe {
        libc::snprintf(text, len, c"%s".as_ptr(), msg.as_ptr());
    }
}

/// Releases the buffer of a Kelp string this module is replacing. A hosted
/// buffer comes from `buf_alloc` (malloc), so it is freed here. A managed
/// buffer lives in the collector's heap, which `free` must never see: the
/// collector reclaims it once nothing refers to it (decision 10d).
unsafe fn release_kelp_buf(data: *mut u8) {
    #[cfg(not(feature = "managed"))]
    unsafe {
        crate::alloc::buf_free(data);
    }
    #[cfg(feature = "managed")]
    let _ = data;
}

/// Replaces `*reason` with a fresh Kelp string holding `text`, releasing the old
/// buffer first, as every failing body does.
unsafe fn set_reason(reason: *mut KString, text: *const c_char) {
    unsafe {
        release_kelp_buf((*reason).data);
        *reason = kelp_String__from_str(text);
    }
}

/// `KELP_NO_SIGPIPE`: blocks SIGPIPE on this thread around `call`, consumes a
/// SIGPIPE that `call` itself raised, and restores `errno` for the caller.
unsafe fn no_sigpipe(call: impl FnOnce() -> c_int) -> c_int {
    unsafe {
        let mut one: libc::sigset_t = zeroed();
        let mut pend: libc::sigset_t = zeroed();
        let mut old: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, libc::SIGPIPE);
        libc::sigpending(&mut pend);
        let was_pending = libc::sigismember(&pend, libc::SIGPIPE) == 1;
        libc::pthread_sigmask(libc::SIG_BLOCK, &one, &mut old);
        let r = call();
        let saved = errno();
        if !was_pending {
            libc::sigpending(&mut pend);
            if libc::sigismember(&pend, libc::SIGPIPE) == 1 {
                let zero = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                libc::sigtimedwait(&one, null_mut(), &zero);
            }
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, null_mut());
        set_errno(saved);
        r
    }
}

/// `SO_RCVTIMEO` and `SO_SNDTIMEO` both set to `ms`.
unsafe fn set_socket_timeouts(fd: c_int, ms: u64) {
    unsafe {
        let tv = libc::timeval {
            tv_sec: (ms / 1000) as libc::time_t,
            tv_usec: ((ms % 1000) * 1000) as libc::suseconds_t,
        };
        let len = size_of::<libc::timeval>() as libc::socklen_t;
        let p = &tv as *const libc::timeval as *const c_void;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, p, len);
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_SNDTIMEO, p, len);
    }
}

// ---- the classifiers (the same answers as `cdecl.rs`'s C copies) ----

/// The `std/tls` code for a verify result, with its sentence in `text`.
unsafe fn classify_verify(v: c_long, text: *mut c_char, len: usize) -> c_int {
    unsafe {
        libc::snprintf(
            text,
            len,
            c"certificate verification failed: %s".as_ptr(),
            X509_verify_cert_error_string(v),
        );
        match v {
            X509_V_ERR_HOSTNAME_MISMATCH | X509_V_ERR_IP_ADDRESS_MISMATCH => 3,
            X509_V_ERR_CERT_HAS_EXPIRED
            | X509_V_ERR_CERT_NOT_YET_VALID
            | X509_V_ERR_CRL_HAS_EXPIRED
            | X509_V_ERR_CRL_NOT_YET_VALID => 2,
            _ => 1,
        }
    }
}

/// The `std/tls` code for a failed SSL call, with its sentence in `text`.
unsafe fn classify_error(_ssl: *mut SSL, err: c_int, text: *mut c_char, len: usize) -> c_int {
    unsafe {
        if err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE {
            say(text, len, c"the operation timed out");
            return 7;
        }
        if err == SSL_ERROR_ZERO_RETURN {
            say(text, len, c"the peer closed the connection");
            return 6;
        }
        if err == SSL_ERROR_SYSCALL {
            if errno() == libc::EAGAIN || errno() == libc::EWOULDBLOCK {
                say(text, len, c"the operation timed out");
                return 7;
            }
            if errno() == 0 {
                say(text, len, c"the peer closed the connection without a close_notify");
                return 6;
            }
            libc::snprintf(
                text,
                len,
                c"socket error: %s".as_ptr(),
                libc::strerror(errno()),
            );
            return 8;
        }
        let e = ERR_get_error();
        if err == SSL_ERROR_SSL {
            let reason = (e & ERR_REASON_MASK) as c_int;
            if reason == SSL_R_TLSV1_ALERT_PROTOCOL_VERSION
                || reason == SSL_R_UNSUPPORTED_PROTOCOL
                || reason == SSL_R_NO_PROTOCOLS_AVAILABLE
                || reason == SSL_R_WRONG_VERSION_NUMBER
            {
                ERR_error_string_n(e, text, len);
                return 4;
            }
            if reason == SSL_R_UNEXPECTED_EOF_WHILE_READING {
                libc::snprintf(
                    text,
                    len,
                    c"the peer closed the connection without a close_notify".as_ptr(),
                );
                return 6;
            }
            ERR_error_string_n(e, text, len);
            return 8;
        }
        if e != 0 {
            ERR_error_string_n(e, text, len);
            return 8;
        }
        libc::snprintf(
            text,
            len,
            c"TLS operation failed (SSL_get_error returned %d)".as_ptr(),
            err,
        );
        8
    }
}

/// A refusal during the handshake (an alert, no common ALPN protocol, a
/// missing client certificate) is a failed handshake, code 5.
unsafe fn classify_handshake(ssl: *mut SSL, err: c_int, text: *mut c_char, len: usize) -> c_int {
    unsafe {
        let code = classify_error(ssl, err, text, len);
        if code == 8 && err == SSL_ERROR_SSL {
            return 5;
        }
        code
    }
}

/// Records a fixed sentence, clears the OpenSSL error queue, and returns 5.
unsafe fn fail(text: *mut c_char, len: usize, msg: &CStr) -> c_int {
    unsafe {
        say(text, len, msg);
        ERR_clear_error();
        5
    }
}

// ---- certificate and key loading ----

/// The pass phrase of an encrypted private key, handed to OpenSSL's callback.
#[repr(C)]
struct Pass {
    pass: *const u8,
    len: usize,
}

unsafe extern "C" fn pass_cb(buf: *mut c_char, size: c_int, _rwflag: c_int, userdata: *mut c_void) -> c_int {
    unsafe {
        let p = userdata as *const Pass;
        if p.is_null() || (*p).len == 0 || size <= 0 {
            return 0;
        }
        let n = if (*p).len < size as usize { (*p).len } else { size as usize };
        core::ptr::copy_nonoverlapping((*p).pass, buf as *mut u8, n);
        n as c_int
    }
}

/// The certificate chain in `pem` (leaf first) as `ctx`'s own.
unsafe fn load_chain(ctx: *mut SSL_CTX, pem: *const u8, len: usize, text: *mut c_char, tlen: usize) -> c_int {
    unsafe {
        if len == 0 || len > 0x7fff_ffff {
            return fail(text, tlen, c"the certificate chain file is empty");
        }
        let bio = BIO_new_mem_buf(pem as *const c_void, len as c_int);
        if bio.is_null() {
            return fail(text, tlen, c"cannot read the certificate chain");
        }
        let leaf = PEM_read_bio_X509_AUX(bio, null_mut(), null(), null_mut());
        if leaf.is_null() {
            BIO_free(bio);
            return fail(text, tlen, c"the certificate chain is not a valid PEM certificate");
        }
        if SSL_CTX_use_certificate(ctx, leaf) != 1 {
            X509_free(leaf);
            BIO_free(bio);
            return fail(text, tlen, c"the certificate cannot be used");
        }
        X509_free(leaf);
        loop {
            let extra = PEM_read_bio_X509(bio, null_mut(), null(), null_mut());
            if extra.is_null() {
                break;
            }
            if SSL_CTX_ctrl(ctx, SSL_CTRL_EXTRA_CHAIN_CERT, 0, extra as *mut c_void) != 1 {
                X509_free(extra);
                BIO_free(bio);
                return fail(text, tlen, c"a certificate of the chain cannot be used");
            }
        }
        // The loop ends on the PEM reader's end-of-file entry, which is expected.
        ERR_clear_error();
        BIO_free(bio);
        0
    }
}

/// The private key in `pem`, which must belong to the certificate in `ctx`.
unsafe fn load_key(
    ctx: *mut SSL_CTX,
    pem: *const u8,
    len: usize,
    pass: *const u8,
    pass_len: usize,
    text: *mut c_char,
    tlen: usize,
) -> c_int {
    unsafe {
        if len == 0 || len > 0x7fff_ffff {
            return fail(text, tlen, c"the private key file is empty");
        }
        let bio = BIO_new_mem_buf(pem as *const c_void, len as c_int);
        if bio.is_null() {
            return fail(text, tlen, c"cannot read the private key");
        }
        let mut pw = Pass { pass, len: pass_len };
        let key = PEM_read_bio_PrivateKey(
            bio,
            null_mut(),
            Some(pass_cb),
            &mut pw as *mut Pass as *mut c_void,
        );
        BIO_free(bio);
        if key.is_null() {
            if pass_len == 0 {
                return fail(
                    text,
                    tlen,
                    c"the private key is malformed or encrypted (an encrypted key needs ServerOptions.key_password)",
                );
            }
            return fail(text, tlen, c"the private key is malformed or the pass phrase is wrong");
        }
        if SSL_CTX_use_PrivateKey(ctx, key) != 1 {
            EVP_PKEY_free(key);
            return fail(text, tlen, c"the private key does not match the certificate");
        }
        EVP_PKEY_free(key);
        if SSL_CTX_check_private_key(ctx) != 1 {
            return fail(text, tlen, c"the private key does not match the certificate");
        }
        0
    }
}

/// Client certificates are verified against the CAs in `pem` (and only those).
unsafe fn load_client_ca(ctx: *mut SSL_CTX, pem: *const u8, len: usize, text: *mut c_char, tlen: usize) -> c_int {
    unsafe {
        if len == 0 || len > 0x7fff_ffff {
            return fail(text, tlen, c"the client CA file is empty");
        }
        let bio = BIO_new_mem_buf(pem as *const c_void, len as c_int);
        if bio.is_null() {
            return fail(text, tlen, c"cannot read the client CA file");
        }
        let store = SSL_CTX_get_cert_store(ctx);
        let mut count = 0;
        loop {
            let ca = PEM_read_bio_X509(bio, null_mut(), null(), null_mut());
            if ca.is_null() {
                break;
            }
            if X509_STORE_add_cert(store, ca) != 1 || SSL_CTX_add_client_CA(ctx, ca) != 1 {
                X509_free(ca);
                BIO_free(bio);
                return fail(text, tlen, c"a client CA certificate cannot be used");
            }
            X509_free(ca);
            count += 1;
        }
        ERR_clear_error();
        BIO_free(bio);
        if count == 0 {
            return fail(text, tlen, c"the client CA file holds no PEM certificate");
        }
        0
    }
}

/// Adds the certificates in `pem` to a client context's trust roots. Only
/// added: verification stays on.
unsafe fn add_roots(ctx: *mut SSL_CTX, pem: *const u8, len: usize, text: *mut c_char, tlen: usize) -> c_int {
    unsafe {
        if len == 0 || len > 0x7fff_ffff {
            return fail(text, tlen, c"the CA file is empty");
        }
        let bio = BIO_new_mem_buf(pem as *const c_void, len as c_int);
        if bio.is_null() {
            return fail(text, tlen, c"cannot read the CA file");
        }
        let store = SSL_CTX_get_cert_store(ctx);
        let mut count = 0;
        loop {
            let ca = PEM_read_bio_X509(bio, null_mut(), null(), null_mut());
            if ca.is_null() {
                break;
            }
            if X509_STORE_add_cert(store, ca) != 1 {
                X509_free(ca);
                BIO_free(bio);
                return fail(text, tlen, c"a CA certificate cannot be used");
            }
            X509_free(ca);
            count += 1;
        }
        ERR_clear_error();
        BIO_free(bio);
        if count == 0 {
            return fail(text, tlen, c"the CA file holds no PEM certificate");
        }
        0
    }
}

// ---- ALPN on the server ----

/// The wire-format protocol list, held on the `SSL_CTX` through ex_data so
/// freeing the context frees it.
#[repr(C)]
struct AlpnCfg {
    wire: *mut u8,
    len: usize,
}

struct OnceCell(UnsafeCell<libc::pthread_once_t>);
// SAFETY: the cell is only ever handed to `pthread_once`, which synchronises.
unsafe impl Sync for OnceCell {}

static ALPN_ONCE: OnceCell = OnceCell(UnsafeCell::new(libc::PTHREAD_ONCE_INIT));
static ALPN_IDX: AtomicI32 = AtomicI32::new(-1);

unsafe extern "C" fn alpn_free(
    _parent: *mut c_void,
    ptr: *mut c_void,
    _ad: *mut CRYPTO_EX_DATA,
    _idx: c_int,
    _argl: c_long,
    _argp: *mut c_void,
) {
    unsafe {
        let c = ptr as *mut AlpnCfg;
        if !c.is_null() {
            libc::free((*c).wire as *mut c_void);
            libc::free(c as *mut c_void);
        }
    }
}

extern "C" fn alpn_init() {
    unsafe {
        let idx = CRYPTO_get_ex_new_index(
            CRYPTO_EX_INDEX_SSL_CTX,
            0,
            null_mut(),
            null(),
            null(),
            Some(alpn_free),
        );
        ALPN_IDX.store(idx, Ordering::SeqCst);
    }
}

/// The server's choice: the first of its own protocols the client also
/// offered. No overlap is a fatal `no_application_protocol` alert.
unsafe extern "C" fn alpn_select(
    _ssl: *mut SSL,
    out: *mut *const u8,
    outlen: *mut u8,
    input: *const u8,
    inlen: c_uint,
    arg: *mut c_void,
) -> c_int {
    unsafe {
        let c = arg as *const AlpnCfg;
        let mut chosen: *mut u8 = null_mut();
        if SSL_select_next_proto(&mut chosen, outlen, (*c).wire, (*c).len as c_uint, input, inlen)
            == OPENSSL_NPN_NEGOTIATED
        {
            *out = chosen;
            return SSL_TLSEXT_ERR_OK;
        }
        SSL_TLSEXT_ERR_ALERT_FATAL
    }
}

/// Whether `wire` is a well-formed ALPN list: length-prefixed names, none
/// empty, none running past the end.
unsafe fn alpn_valid(wire: *const u8, len: usize) -> bool {
    unsafe {
        let mut i = 0;
        while i < len {
            let n = *wire.add(i) as usize;
            if n == 0 || i + 1 + n > len {
                return false;
            }
            i += 1 + n;
        }
        true
    }
}

/// Builds the server's `SSL_CTX`. NULL with 5 in `*code` and a fixed sentence
/// in `text` on any failure.
#[allow(clippy::too_many_arguments)]
unsafe fn server_ctx(
    cert: *const u8,
    cert_len: usize,
    key: *const u8,
    key_len: usize,
    ca: *const u8,
    ca_len: usize,
    alpn: *const u8,
    alpn_len: usize,
    pass: *const u8,
    pass_len: usize,
    min13: bool,
    code: *mut c_int,
    text: *mut c_char,
    tlen: usize,
) -> *mut SSL_CTX {
    unsafe {
        *code = 5;
        if !alpn_valid(alpn, alpn_len) {
            fail(text, tlen, c"the ALPN protocol list is not valid");
            return null_mut();
        }
        let ctx = SSL_CTX_new(TLS_server_method());
        if ctx.is_null() {
            fail(text, tlen, c"cannot create an SSL context");
            return null_mut();
        }
        let floor = if min13 { TLS1_3_VERSION } else { TLS1_2_VERSION };
        SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, floor, null_mut());
        SSL_CTX_set_options(ctx, SSL_OP_CIPHER_SERVER_PREFERENCE);
        // Names the application the session cache belongs to.
        SSL_CTX_set_session_id_context(ctx, b"kelp-tls".as_ptr(), 8);
        if load_chain(ctx, cert, cert_len, text, tlen) != 0
            || load_key(ctx, key, key_len, pass, pass_len, text, tlen) != 0
        {
            SSL_CTX_free(ctx);
            return null_mut();
        }
        if ca_len > 0 {
            if load_client_ca(ctx, ca, ca_len, text, tlen) != 0 {
                SSL_CTX_free(ctx);
                return null_mut();
            }
            SSL_CTX_set_verify(
                ctx,
                SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT,
                null(),
            );
        }
        if alpn_len > 0 {
            libc::pthread_once(ALPN_ONCE.0.get(), alpn_init);
            let c = libc::malloc(size_of::<AlpnCfg>()) as *mut AlpnCfg;
            let wire = libc::malloc(alpn_len) as *mut u8;
            if c.is_null() || wire.is_null() || ALPN_IDX.load(Ordering::SeqCst) < 0 {
                libc::free(c as *mut c_void);
                libc::free(wire as *mut c_void);
                SSL_CTX_free(ctx);
                fail(text, tlen, c"out of memory");
                return null_mut();
            }
            core::ptr::copy_nonoverlapping(alpn, wire, alpn_len);
            (*c).wire = wire;
            (*c).len = alpn_len;
            if SSL_CTX_set_ex_data(ctx, ALPN_IDX.load(Ordering::SeqCst), c as *mut c_void) != 1 {
                libc::free(wire as *mut c_void);
                libc::free(c as *mut c_void);
                SSL_CTX_free(ctx);
                fail(text, tlen, c"cannot attach the ALPN list");
                return null_mut();
            }
            SSL_CTX_set_alpn_select_cb(ctx, Some(alpn_select), c as *mut c_void);
        }
        *code = 0;
        ctx
    }
}

/// What a `ServerConfig` points at: the current `SSL_CTX`, swapped under the
/// lock by a reload.
#[repr(C)]
struct Slot {
    ctx: *mut SSL_CTX,
    mu: libc::pthread_mutex_t,
}

// ---- the exported bodies ----

/// Client handshake over a connected `fd`. Returns the `SSL*`, or NULL with
/// `*err_code` and `*reason` set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__handshake_try_raw(
    fd: i32,
    host: *const c_char,
    ca_pem: *const Bytes,
    timeout_ms: u64,
    alpn: *const Bytes,
    cert_pem: *const Bytes,
    key_pem: *const Bytes,
    err_code: *mut i32,
    reason: *mut KString,
) -> *mut c_void {
    unsafe {
        *err_code = 0;
        let ctx = SSL_CTX_new(TLS_client_method());
        if ctx.is_null() {
            kelp_panic(c"tls.handshake: cannot create an SSL context".as_ptr());
        }
        SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, null_mut());
        SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, null());
        if SSL_CTX_set_default_verify_paths(ctx) != 1 {
            SSL_CTX_free(ctx);
            kelp_panic(c"tls.handshake: cannot load the system trust store".as_ptr());
        }
        // Private CA certificates are *added* to the roots loaded above; nothing
        // here lowers SSL_VERIFY_PEER.
        if (*ca_pem).len > 0 {
            let mut text = [0 as c_char; 128];
            if add_roots(ctx, (*ca_pem).data, (*ca_pem).len, text.as_mut_ptr(), text.len()) != 0 {
                SSL_CTX_free(ctx);
                *err_code = 5;
                set_reason(reason, text.as_ptr());
                return null_mut();
            }
        }
        // ALPN and a client certificate are the two additions of the extended form.
        let kelp_ext = (*alpn).len > 0 || (*cert_pem).len > 0;
        if (*alpn).len > 0
            && (!alpn_valid((*alpn).data, (*alpn).len)
                || SSL_CTX_set_alpn_protos(ctx, (*alpn).data, (*alpn).len as c_uint) != 0)
        {
            SSL_CTX_free(ctx);
            *err_code = 5;
            set_reason(reason, c"the ALPN protocol list is not valid".as_ptr());
            return null_mut();
        }
        if (*cert_pem).len > 0 {
            let mut text = [0 as c_char; 128];
            if load_chain(ctx, (*cert_pem).data, (*cert_pem).len, text.as_mut_ptr(), text.len()) != 0
                || load_key(
                    ctx,
                    (*key_pem).data,
                    (*key_pem).len,
                    null(),
                    0,
                    text.as_mut_ptr(),
                    text.len(),
                ) != 0
            {
                SSL_CTX_free(ctx);
                *err_code = 5;
                set_reason(reason, text.as_ptr());
                return null_mut();
            }
        }
        let ssl = SSL_new(ctx);
        // `ssl` keeps its own reference to `ctx`.
        SSL_CTX_free(ctx);
        if ssl.is_null() {
            kelp_panic(c"tls.handshake: cannot create an SSL handle".as_ptr());
        }
        if SSL_set_fd(ssl, fd) != 1 {
            SSL_free(ssl);
            kelp_panic(c"tls.handshake: cannot bind the socket to the TLS handle".as_ptr());
        }
        if timeout_ms > 0 {
            set_socket_timeouts(fd, timeout_ms);
        }
        // SNI and the hostname check both use the name the caller wrote.
        SSL_ctrl(
            ssl,
            SSL_CTRL_SET_TLSEXT_HOSTNAME,
            TLSEXT_NAMETYPE_HOST_NAME,
            host as *mut c_void,
        );
        if SSL_set1_host(ssl, host) != 1 {
            SSL_free(ssl);
            kelp_panic(c"tls.handshake: cannot set the verification hostname".as_ptr());
        }
        loop {
            let rc = no_sigpipe(|| SSL_connect(ssl));
            if rc == 1 {
                break;
            }
            let err = SSL_get_error(ssl, rc);
            if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE)
                && errno() != libc::EAGAIN
                && errno() != libc::EWOULDBLOCK
            {
                continue;
            }
            let mut text = [0 as c_char; 256];
            let v = SSL_get_verify_result(ssl);
            let code = if v != X509_V_OK {
                classify_verify(v, text.as_mut_ptr(), text.len())
            } else if kelp_ext {
                classify_handshake(ssl, err, text.as_mut_ptr(), text.len())
            } else {
                classify_error(ssl, err, text.as_mut_ptr(), text.len())
            };
            SSL_free(ssl);
            *err_code = code;
            set_reason(reason, text.as_ptr());
            return null_mut();
        }
        let verify = SSL_get_verify_result(ssl);
        if verify != X509_V_OK {
            let mut text = [0 as c_char; 256];
            let code = classify_verify(verify, text.as_mut_ptr(), text.len());
            SSL_free(ssl);
            *err_code = code;
            set_reason(reason, text.as_ptr());
            return null_mut();
        }
        ssl as *mut c_void
    }
}

/// Builds a server configuration slot from PEM bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__server_ctx_try_raw(
    cert: *const Bytes,
    key: *const Bytes,
    ca: *const Bytes,
    alpn: *const Bytes,
    pass: *const Bytes,
    min13: i32,
    err_code: *mut i32,
    reason: *mut KString,
) -> *mut c_void {
    unsafe {
        let mut text = [0 as c_char; 160];
        let mut code: c_int = 0;
        let ctx = server_ctx(
            (*cert).data,
            (*cert).len,
            (*key).data,
            (*key).len,
            (*ca).data,
            (*ca).len,
            (*alpn).data,
            (*alpn).len,
            (*pass).data,
            (*pass).len,
            min13 != 0,
            &mut code,
            text.as_mut_ptr(),
            text.len(),
        );
        *err_code = code;
        if ctx.is_null() {
            set_reason(reason, text.as_ptr());
            return null_mut();
        }
        let slot = libc::malloc(size_of::<Slot>()) as *mut Slot;
        if slot.is_null() {
            SSL_CTX_free(ctx);
            *err_code = 5;
            set_reason(reason, c"out of memory".as_ptr());
            return null_mut();
        }
        (*slot).ctx = ctx;
        libc::pthread_mutex_init(&mut (*slot).mu, null());
        slot as *mut c_void
    }
}

/// Replaces the context a `ServerConfig` hands to new handshakes. A bad
/// renewal leaves the old context in place and reports the failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__reload_try_raw(
    ctx: *mut c_void,
    cert: *const Bytes,
    key: *const Bytes,
    ca: *const Bytes,
    alpn: *const Bytes,
    pass: *const Bytes,
    min13: i32,
    err_code: *mut i32,
    reason: *mut KString,
) {
    unsafe {
        let mut text = [0 as c_char; 160];
        let mut code: c_int = 0;
        let fresh = server_ctx(
            (*cert).data,
            (*cert).len,
            (*key).data,
            (*key).len,
            (*ca).data,
            (*ca).len,
            (*alpn).data,
            (*alpn).len,
            (*pass).data,
            (*pass).len,
            min13 != 0,
            &mut code,
            text.as_mut_ptr(),
            text.len(),
        );
        *err_code = code;
        if fresh.is_null() {
            set_reason(reason, text.as_ptr());
            return;
        }
        let slot = ctx as *mut Slot;
        libc::pthread_mutex_lock(&mut (*slot).mu);
        let old = (*slot).ctx;
        (*slot).ctx = fresh;
        libc::pthread_mutex_unlock(&mut (*slot).mu);
        SSL_CTX_free(old);
    }
}

/// Server handshake over a socket the caller already accepted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__accept_try_raw(
    ctx: *mut c_void,
    fd: i32,
    timeout_ms: u64,
    err_code: *mut i32,
    reason: *mut KString,
) -> *mut c_void {
    unsafe {
        *err_code = 0;
        let slot = ctx as *mut Slot;
        libc::pthread_mutex_lock(&mut (*slot).mu);
        let ssl = SSL_new((*slot).ctx);
        libc::pthread_mutex_unlock(&mut (*slot).mu);
        if ssl.is_null() {
            kelp_panic(c"tls.accept: cannot create an SSL handle".as_ptr());
        }
        if SSL_set_fd(ssl, fd) != 1 {
            SSL_free(ssl);
            kelp_panic(c"tls.accept: cannot bind the socket to the TLS handle".as_ptr());
        }
        if timeout_ms > 0 {
            set_socket_timeouts(fd, timeout_ms);
        }
        loop {
            let rc = no_sigpipe(|| SSL_accept(ssl));
            if rc == 1 {
                break;
            }
            let err = SSL_get_error(ssl, rc);
            if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE)
                && errno() != libc::EAGAIN
                && errno() != libc::EWOULDBLOCK
            {
                continue;
            }
            let mut text = [0 as c_char; 256];
            let v = SSL_get_verify_result(ssl);
            let code = if v != X509_V_OK {
                classify_verify(v, text.as_mut_ptr(), text.len())
            } else {
                classify_handshake(ssl, err, text.as_mut_ptr(), text.len())
            };
            SSL_free(ssl);
            *err_code = code;
            set_reason(reason, text.as_ptr());
            return null_mut();
        }
        ssl as *mut c_void
    }
}

/// Ends a `ServerConfig`: the current context is released once the last
/// session made from it has been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__ctx_free_raw(ctx: *mut c_void) {
    unsafe {
        let slot = ctx as *mut Slot;
        SSL_CTX_free((*slot).ctx);
        libc::pthread_mutex_destroy(&mut (*slot).mu);
        libc::free(slot as *mut c_void);
    }
}

/// The protocol the handshake settled on, or `false` when none was negotiated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__alpn_raw(handle: *mut c_void, out: *mut KString) -> bool {
    unsafe {
        let mut data: *const u8 = null();
        let mut len: c_uint = 0;
        SSL_get0_alpn_selected(handle as *const SSL, &mut data, &mut len);
        if data.is_null() || len == 0 {
            return false;
        }
        let text = libc::malloc(len as usize + 1) as *mut u8;
        if text.is_null() {
            kelp_panic(c"tls.alpn: out of memory".as_ptr());
        }
        core::ptr::copy_nonoverlapping(data, text, len as usize);
        *text.add(len as usize) = 0;
        release_kelp_buf((*out).data);
        *out = kelp_String__from_str(text as *const c_char);
        libc::free(text as *mut c_void);
        true
    }
}

/// The subject of the peer's certificate in RFC 2253 form, or `false` when the
/// peer sent none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__peer_subject_raw(handle: *mut c_void, out: *mut KString) -> bool {
    unsafe {
        let cert = SSL_get1_peer_certificate(handle as *const SSL);
        if cert.is_null() {
            return false;
        }
        let bio = BIO_new(BIO_s_mem());
        if bio.is_null() {
            X509_free(cert);
            kelp_panic(c"tls.peer_cert_subject: out of memory".as_ptr());
        }
        X509_NAME_print_ex(bio, X509_get_subject_name(cert), 0, XN_FLAG_RFC2253);
        let mut data: *mut c_char = null_mut();
        let n = BIO_ctrl(bio, BIO_CTRL_INFO, 0, &mut data as *mut *mut c_char as *mut c_void);
        let text = libc::malloc(n as usize + 1) as *mut c_char;
        if text.is_null() {
            BIO_free(bio);
            X509_free(cert);
            kelp_panic(c"tls.peer_cert_subject: out of memory".as_ptr());
        }
        core::ptr::copy_nonoverlapping(data, text, n as usize);
        *text.add(n as usize) = 0;
        BIO_free(bio);
        X509_free(cert);
        release_kelp_buf((*out).data);
        *out = kelp_String__from_str(text);
        libc::free(text as *mut c_void);
        true
    }
}

/// Decrypted bytes OpenSSL already holds for `handle`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__tls__pending_raw(handle: *mut c_void) -> usize {
    unsafe { SSL_pending(handle as *const SSL) as usize }
}

/// The server's IPv4 listener for `host:port`. The cap check ran before this
/// body. Returns the descriptor or panics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__net__listen_on_raw(host: *const c_char, port: u16) -> i32 {
    unsafe {
        let mut hints: libc::addrinfo = zeroed();
        hints.ai_family = libc::AF_INET;
        hints.ai_socktype = libc::SOCK_STREAM;
        hints.ai_flags = libc::AI_PASSIVE;
        let mut bind_host = host;
        if bind_host.is_null() || *bind_host == 0 || (*bind_host == b'*' as c_char && *bind_host.add(1) == 0) {
            bind_host = null();
        }
        let mut service = [0 as c_char; 8];
        libc::snprintf(service.as_mut_ptr(), service.len(), c"%u".as_ptr(), port as c_uint);
        let mut list: *mut libc::addrinfo = null_mut();
        let rc = libc::getaddrinfo(bind_host, service.as_ptr(), &hints, &mut list);
        if rc != 0 {
            crate::panic::kelp_panic_gai(c"net.listen_on: cannot resolve the bind address".as_ptr(), rc);
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
                &on as *const c_int as *const c_void,
                size_of::<c_int>() as libc::socklen_t,
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
            crate::panic::kelp_panic_at(c"net.listen_on: cannot bind".as_ptr(), last);
        }
        fd
    }
}

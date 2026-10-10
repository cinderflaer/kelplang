//! The C text the `llvm` backend still writes: the runtime headers, the
//! `extern fun` declarations and callback glue of a program's per-program C
//! object, the closure and container typedefs the object's declarations name,
//! and the two string helpers (`asm_operand_text`, `c_string_literal`) shared
//! with the LLVM backend's own escaping.
//!
//! Moved here out of `codegen_c.rs` (stage 2, WP-3) without a change to any
//! emitted byte, so the C generator can be deleted later without losing the
//! text the LLVM backend depends on. Nothing in this module is the oracle.

use crate::tir::*;
use crate::types::{clone_fn_name, cmp_fn_name, drop_fn_name, eq_fn_name, hash_fn_name, needs_drop, EnumDef, StructDef, Ty};
use std::collections::HashSet;
use std::fmt::Write;

/// Where the capability check's own half lives (MK7 §8): the deny path, the
/// buffered plumbing, and the allowlist loops. The *matching* and the *path
/// normalization* it calls are Kelp (`std/rt`), declared just before this text
/// is emitted — see `runtime_prototypes` and `kelpc/runtime/cap_glue.c`.
pub(crate) const CAP_GLUE: &str = include_str!("../runtime/cap_glue.c");

/// The C headers a generated program (and the `extern fun` bodies the LLVM
/// backend compiles separately, MK7 §3.2) relies on. Included unconditionally:
/// `std/math`'s externs call the C library directly, and an unused declaration
/// costs nothing. One list rather than two, because the C text is one text.
pub(crate) const C_HEADERS: &str = "\
#include <errno.h>\n\
#include <limits.h>\n\
#include <dirent.h>\n\
#include <fcntl.h>\n\
#include <arpa/inet.h>\n\
#include <math.h>\n\
#include <netdb.h>\n\
#include <netinet/in.h>\n\
#include <poll.h>\n\
#include <signal.h>\n\
#include <sys/socket.h>\n\
#include <sys/sendfile.h>\n\
#include <sys/stat.h>\n\
#include <sys/types.h>\n\
#include <sys/wait.h>\n\
#include <time.h>\n\
#include <unistd.h>\n\
#include <stdint.h>\n\
#include <stdbool.h>\n\
// `stdatomic.h` is C11 standard, gcc and clang both ship it. The runtime's\n\
// environment read cache (`kelp_env_table` in `kelp_rt.c`) is the one thing\n\
// that needs it; everything else in this preamble predates it.\n\
#include <stdatomic.h>\n\
// `isize` is `ptrdiff_t` (`types.rs`), which is `stddef.h`'s and not brought\n\
// in by any of the headers above or below.\n\
// `pthread.h` is the runtime's thread machinery (MK9-S3-M1 stage C1): the two\n\
// generated backends include the runtime text in different places, so the\n\
// header it needs belongs with the rest of them rather than in the middle of\n\
// the text.\n\
#include <pthread.h>\n\
#include <stddef.h>\n\
#include <stdio.h>\n\
#include <stdlib.h>\n\
#include <string.h>\n\
\n";

/// The OpenSSL declarations, added after [`C_HEADERS`] only for a program that
/// reaches `std/tls` (see [`uses_tls`]). `std/tls`'s externs are the only
/// OpenSSL-touching code in the compiler, and a program without them must build
/// on a host that has no OpenSSL development files (BUG-x7Qmd).
pub(crate) const OPENSSL_HEADERS: &str = concat!("\
#include <openssl/err.h>\n\
#include <openssl/pem.h>\n\
#include <openssl/ssl.h>\n\
#include <openssl/x509v3.h>\n\
// SIGPIPE: OpenSSL writes to its socket with a plain `write()`, so a write to a\n\
// peer that has reset the connection raises SIGPIPE, whose default action ends\n\
// the process before the call can return the error the typed TLS paths read.\n\
// It is blocked on the calling thread for the duration of each OpenSSL call\n\
// that can write, and a SIGPIPE that call itself raised is consumed, so the\n\
// signal's default action stays in force everywhere else: a program piped into\n\
// `head` still stops when its reader goes away. `errno` is restored because\n\
// `SSL_get_error` reads it.\n\
static inline void kelp_sigpipe_block(sigset_t* old, int* was_pending) {\n\
    sigset_t one, pend;\n\
    sigemptyset(&one);\n\
    sigaddset(&one, SIGPIPE);\n\
    sigpending(&pend);\n\
    *was_pending = sigismember(&pend, SIGPIPE);\n\
    pthread_sigmask(SIG_BLOCK, &one, old);\n\
}\n\
static inline void kelp_sigpipe_restore(const sigset_t* old, int was_pending) {\n\
    if (!was_pending) {\n\
        sigset_t one, pend;\n\
        sigemptyset(&one);\n\
        sigaddset(&one, SIGPIPE);\n\
        sigpending(&pend);\n\
        if (sigismember(&pend, SIGPIPE)) {\n\
            struct timespec zero = { 0, 0 };\n\
            sigtimedwait(&one, NULL, &zero);\n\
        }\n\
    }\n\
    pthread_sigmask(SIG_SETMASK, old, NULL);\n\
}\n\
#define KELP_NO_SIGPIPE(call) ({ sigset_t kelp_sp_old; int kelp_sp_was; int kelp_sp_errno; \\\n\
    kelp_sigpipe_block(&kelp_sp_old, &kelp_sp_was); \\\n\
    __typeof__(call) kelp_sp_r = (call); kelp_sp_errno = errno; \\\n\
    kelp_sigpipe_restore(&kelp_sp_old, kelp_sp_was); errno = kelp_sp_errno; kelp_sp_r; })\n\
// The two classifiers `std/tls`'s fallible externs share. They are here\n\
// rather than in each body because every failing SSL call needs the same\n\
// translation, and a copy per call site is how the answers drift apart. The\n\
// integer each one returns is the code `std/tls`'s `ERR_*` constants name.\n\
static inline int kelp_tls_classify_verify(long v, char* text, size_t text_len) {\n\
    snprintf(text, text_len, \"certificate verification failed: %s\", X509_verify_cert_error_string(v));\n\
    switch (v) {\n\
#ifdef X509_V_ERR_HOSTNAME_MISMATCH\n\
        case X509_V_ERR_HOSTNAME_MISMATCH:\n\
#endif\n\
#ifdef X509_V_ERR_IP_ADDRESS_MISMATCH\n\
        case X509_V_ERR_IP_ADDRESS_MISMATCH:\n\
#endif\n\
            return 3;\n\
        case X509_V_ERR_CERT_HAS_EXPIRED:\n\
        case X509_V_ERR_CERT_NOT_YET_VALID:\n\
        case X509_V_ERR_CRL_HAS_EXPIRED:\n\
        case X509_V_ERR_CRL_NOT_YET_VALID:\n\
            return 2;\n\
        default:\n\
            return 1;\n\
    }\n\
}\n\
static inline int kelp_tls_classify_error(SSL* ssl, int err, char* text, size_t text_len) {\n\
    if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) {\n\
        // A blocking socket whose SO_RCVTIMEO/SO_SNDTIMEO fired: OpenSSL\n\
        // reports the EAGAIN as a retry, but there is nothing to retry.\n\
        snprintf(text, text_len, \"the operation timed out\");\n\
        return 7;\n\
    }\n\
    if (err == SSL_ERROR_ZERO_RETURN) {\n\
        snprintf(text, text_len, \"the peer closed the connection\");\n\
        return 6;\n\
    }\n\
    if (err == SSL_ERROR_SYSCALL) {\n\
        if (errno == EAGAIN || errno == EWOULDBLOCK) {\n\
            snprintf(text, text_len, \"the operation timed out\");\n\
            return 7;\n\
        }\n\
        if (errno == 0) {\n\
            snprintf(text, text_len, \"the peer closed the connection without a close_notify\");\n\
            return 6;\n\
        }\n\
        snprintf(text, text_len, \"socket error: %s\", strerror(errno));\n\
        return 8;\n\
    }\n\
    unsigned long e = ERR_get_error();\n\
    if (err == SSL_ERROR_SSL) {\n\
        int reason = (int)ERR_GET_REASON(e);\n\
        if (reason == SSL_R_TLSV1_ALERT_PROTOCOL_VERSION\n\
            || reason == SSL_R_UNSUPPORTED_PROTOCOL\n\
            || reason == SSL_R_NO_PROTOCOLS_AVAILABLE\n\
            || reason == SSL_R_WRONG_VERSION_NUMBER) {\n\
            ERR_error_string_n(e, text, text_len);\n\
            return 4;\n\
        }\n\
#ifdef SSL_R_UNEXPECTED_EOF_WHILE_READING\n\
        if (reason == SSL_R_UNEXPECTED_EOF_WHILE_READING) {\n\
            snprintf(text, text_len, \"the peer closed the connection without a close_notify\");\n\
            return 6;\n\
        }\n\
#endif\n\
        ERR_error_string_n(e, text, text_len);\n\
        return 8;\n\
    }\n\
    if (e != 0) {\n\
        ERR_error_string_n(e, text, text_len);\n\
        return 8;\n\
    }\n\
    snprintf(text, text_len, \"TLS operation failed (SSL_get_error returned %d)\", err);\n\
    return 8;\n\
}\n\n",
    include_str!("externs/tls_helpers.c")
);

/// The `#include <...>` lines a project's `[extern.*] header` keys ask for,
/// written after `C_HEADERS`. Empty when no section names a header, so a
/// project without the key emits exactly the C text it did before (MK9-S1-M1,
/// roadmap B-1).
pub(crate) fn extern_header_includes(headers: &[String]) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for header in headers {
        writeln!(out, "#include <{header}>").unwrap();
    }
    out.push('\n');
    out
}

/// Whether the program carries a `std/tls` extern, which is the one thing that
/// needs OpenSSL's headers at compile time and `-lssl -lcrypto` at link time.
/// `std/http` and `std/ws` import `std/tls`, so a program that uses either has
/// it too.
pub fn uses_tls(program: &Program) -> bool {
    program
        .functions
        .iter()
        .any(|f| f.extern_ && f.module == "std/tls")
}

/// One declaration in the program's merged `datac`/`enum` order (A-1), so the
/// two kinds can be emitted interleaved. `order` is a topological order across
/// both kinds, which is what makes every typedef complete at the point it is
/// used.
pub(crate) enum Decl<'a> {
    Record(&'a StructDef),
    Sum(&'a EnumDef),
}

impl Decl<'_> {
    /// Every type this declaration holds, by value or behind a pointer: what
    /// `emit_arrays_for` scans for the array bodies a declaration needs first.
    pub(crate) fn field_tys(&self) -> Vec<&Ty> {
        match self {
            Decl::Record(s) => s.fields.iter().map(|f| &f.ty).collect(),
            Decl::Sum(e) => e
                .variants
                .iter()
                .flat_map(|v| v.fields.iter())
                .map(|f| &f.ty)
                .collect(),
        }
    }
}

/// Emits an `extern` declaration's body: either the compiler-supplied C text
/// from `externs.rs` (a stdlib module), or — MK9-S1-M1, roadmap B-1 — a
/// direct-call trampoline to the native symbol a *user* module's own
/// `coral.toml` `[extern."<module id>"]` section allowlisted, when
/// `extern_ffi_symbols` (`infer.rs::check_program_pass`) names one for this
/// function. `infer.rs` has already rejected an `extern` that is neither, so
/// reaching the `expect` in the stdlib branch would mean the table and the
/// checker disagreed.
///
/// `closure_by_ref` is the LLVM backend's per-program object: a closure is 24
/// bytes, so the LLVM call site passes it through a stack slot
/// (`rt_arg_value`), and the trampoline compiled into that object takes a
/// pointer to it where the C backend's own takes the struct by value.
pub(crate) fn write_extern(
    out: &mut String,
    f: &Function,
    extern_ffi_symbols: &std::collections::HashMap<String, String>,
    closure_by_ref: bool,
) {
    if let Some(symbol) = extern_ffi_symbols.get(&f.name) {
        write_user_extern(out, f, symbol, closure_by_ref);
        return;
    }
    let body = crate::externs::body(&f.module, &f.name)
        .unwrap_or_else(|| panic!("no compiler-supplied body for extern '{}'", f.name));
    // The C signature renames every parameter (`n` -> `kelp_n`), so a body's C
    // text has to use the mangled spelling; list the mapping next to the
    // body so that is readable rather than mysterious.
    if !f.params.is_empty() {
        let names: Vec<String> = f
            .params
            .iter()
            .map(|(n, _)| format!("{n} -> {}", c_name(n)))
            .collect();
        writeln!(out, "// extern '{}' parameters: {}", f.name, names.join(", ")).unwrap();
    }
    write_signature(out, f);
    out.push_str(" {\n");
    out.push_str(body);
    out.push_str("}\n");
}

/// A user-declared `extern fun`'s whole body (MK9-S1-M1, roadmap B-1): a
/// single call to the native `symbol` its module's manifest allowlisted,
/// forwarding every parameter and returning whatever it returns. A
/// `move closure(...)` parameter is the one parameter not forwarded as
/// itself (gap 4): it expands in place into the `(func, user_data,
/// destroy_notify)` triple the native side expects, which the body below
/// spells and `write_callback_glue` defines. There is nothing else to
/// write — the "compiler-supplied body" that `write_extern`'s
/// stdlib branch looks up in `externs.rs` exists to hand-write a primitive
/// `.kp` cannot express at all (a raw syscall, a `malloc`); a user FFI
/// binding has no such primitive to hand-write, since the whole point is
/// that the native symbol already *is* the primitive.
///
/// MK9-S1-M1 keeps user FFI to a 1:1 name mapping (`symbol` is the bare Kelp
/// name `infer.rs` validated against the allowlist). The native symbol's
/// prototype comes from the `[extern.*] header` key, when the module names
/// one: `generate` emits one `#include <header>` line per project header
/// after the fixed preamble, so a symbol the runtime does not already declare
/// is not an implicit declaration.
pub(crate) fn write_user_extern(out: &mut String, f: &Function, symbol: &str, closure_by_ref: bool) {
    // In the LLVM backend's object a closure parameter arrives as a pointer to
    // the caller's stack slot, so the signature is written with that type and
    // the closure is read through it below.
    let by_ref_fn;
    let sig_fn = if closure_by_ref {
        let mut g = f.clone();
        for (_, ty) in g.params.iter_mut() {
            if matches!(ty, Ty::Closure { .. }) {
                *ty = Ty::Ref { mutable: false, inner: Box::new(ty.clone()) };
            }
        }
        by_ref_fn = g;
        &by_ref_fn
    } else {
        f
    };
    write_signature(out, sig_fn);
    out.push_str(" {\n");
    // A `move closure(...)` parameter expands in place into the GLib
    // `(func, user_data, destroy_notify)` triple the native side expects: the
    // boxed closure is `user_data`, the generated thunk reorders the native
    // arguments into the closure's own calling convention, and the generated
    // destructor frees the environment when the native side is done with it.
    // The box (not the stack parameter) is what crosses because the native
    // side may retain `user_data` past this call.
    let mut args: Vec<String> = Vec::new();
    // The C type each native argument is passed as, for the call below when a
    // callback is involved: the thunk and destructor cross as `void*`.
    let mut native_types: Vec<String> = Vec::new();
    let mut has_callback = false;
    for (name, ty) in f.params.iter().filter(|(_, ty)| *ty != Ty::Unit) {
        if let Ty::Closure { params, ret } = ty {
            has_callback = true;
            let c_ty = ty.c_type();
            writeln!(
                out,
                "    {c_ty}* kelp_extern_cb = ({c_ty}*)malloc(sizeof({c_ty}));"
            )
            .unwrap();
            writeln!(
                out,
                "    if (kelp_extern_cb == NULL) {{ kelp_panic(\"out of memory\"); }}"
            )
            .unwrap();
            let source = if closure_by_ref {
                format!("*{}", c_name(name))
            } else {
                c_name(name)
            };
            writeln!(out, "    *kelp_extern_cb = {source};").unwrap();
            args.push(format!("(void*){}", callback_thunk_name(params, ret)));
            args.push("(void*)kelp_extern_cb".to_string());
            args.push(format!("(void*){}", callback_destroy_name(params, ret)));
            native_types.extend(["void*".to_string(), "void*".to_string(), "void*".to_string()]);
        } else {
            args.push(c_name(name));
            native_types.push(ty.c_type());
        }
    }
    // With a callback the call goes through a function pointer of the type the
    // expansion actually has. The native prototype (from the `header` key)
    // spells its callback parameters as `GCallback`, `GClosureNotify` and the
    // like, which no fixed thunk signature converts to implicitly; the ABI is
    // the same for every pointer, so the cast is what makes the call
    // compile. A call with no callback is the plain call it always was.
    let callee = if has_callback {
        format!("(({} (*)({})){symbol})", f.ret.c_type(), native_types.join(", "))
    } else {
        symbol.to_string()
    };
    if f.ret == Ty::Unit {
        writeln!(out, "    {callee}({});", args.join(", ")).unwrap();
    } else {
        writeln!(out, "    return {callee}({});", args.join(", ")).unwrap();
    }
    out.push_str("}\n");
}

/// One field's deep copy, as one C statement. `dst`/`src` are whole field
/// expressions (`dst->kelp_text`), so the clone of an owned field is the
/// runtime's own clone of it, and a nested `datac`/`enum` recurses into its
/// generated clone.
pub(crate) fn write_field_clone(out: &mut String, ty: &Ty, dst: &str, src: &str) {
    match ty {
        Ty::String => writeln!(out, "    {dst} = kelp_String__clone(&{src});").unwrap(),
        Ty::Bytes => writeln!(out, "    {dst} = kelp_Bytes__clone(&{src});").unwrap(),
        Ty::Struct(_) => writeln!(out, "    {}(&{src}, &{dst});", clone_fn_name(ty)).unwrap(),
        _ => writeln!(out, "    {dst} = {src};").unwrap(),
    }
}

/// One field's contribution to a derived order: return early on the first
/// difference, so the fields are compared lexicographically in declaration
/// order.
pub(crate) fn write_field_cmp(out: &mut String, ty: &Ty, a: &str, b: &str) {
    match ty {
        Ty::String | Ty::Bytes => {
            out.push_str("    {\n");
            writeln!(out, "        size_t n = {a}.len < {b}.len ? {a}.len : {b}.len;").unwrap();
            writeln!(out, "        if (n > 0) {{ int c = memcmp({a}.data, {b}.data, n); if (c != 0) {{ return c < 0 ? -1 : 1; }} }}").unwrap();
            writeln!(out, "        if ({a}.len != {b}.len) {{ return {a}.len < {b}.len ? -1 : 1; }}").unwrap();
            out.push_str("    }\n");
        }
        Ty::Struct(_) => {
            writeln!(out, "    {{ int c = {}(&{a}, &{b}); if (c != 0) {{ return c; }} }}", cmp_fn_name(ty)).unwrap();
        }
        _ => {
            writeln!(out, "    if ({a} != {b}) {{ return {a} < {b} ? -1 : 1; }}").unwrap();
        }
    }
}

/// One field's contribution to a derived hash: the byte hash of a `String`,
/// the mix of a scalar, and a nested keyable `datac`/`enum`'s own hash. Every
/// fold goes through the shared `kelp_map_combine`, so this backend and the
/// LLVM one cannot produce a different hash for the same key.
pub(crate) fn write_field_hash(out: &mut String, ty: &Ty, at: &str) {
    match ty {
        Ty::String => writeln!(
            out,
            "    h = kelp_map_combine(h, kelp_map_hash((const char*){at}.data, {at}.len));"
        )
        .unwrap(),
        Ty::Struct(_) => writeln!(
            out,
            "    h = kelp_map_combine(h, {}(&{at}));",
            hash_fn_name(ty)
        )
        .unwrap(),
        _ => writeln!(
            out,
            "    h = kelp_map_combine(h, kelp_map_mix((uint64_t)({at})));"
        )
        .unwrap(),
    }
}

/// One field's equality in a derived key equality, the same field set
/// `write_field_hash` folds. A `String` field reuses the runtime's stored-key
/// comparison, so its byte-length-aware rule is the one the `String` key path
/// already uses.
pub(crate) fn write_field_eq(out: &mut String, ty: &Ty, a: &str, b: &str) {
    match ty {
        Ty::String => writeln!(
            out,
            "    if (!kelp_map_key_eq(&{a}, (const char*){b}.data, {b}.len)) {{ return false; }}"
        )
        .unwrap(),
        Ty::Struct(_) => writeln!(
            out,
            "    if (!{}(&{a}, &{b})) {{ return false; }}",
            eq_fn_name(ty)
        )
        .unwrap(),
        _ => writeln!(out, "    if ({a} != {b}) {{ return false; }}").unwrap(),
    }
}

/// (M6.2). Emitted in declaration order, and a `datac` may only name earlier
/// `datac`s, so a struct's drop function is always defined before the struct
/// that calls it — no forward declarations needed.
pub(crate) fn write_struct(
    out: &mut String,
    s: &StructDef,
    owned: &HashSet<String>,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
    keyable: &HashSet<String>,
    drop_types: &HashSet<String>,
) {
    // Tagged (M8), because a container typedef emitted *earlier* may hold a
    // pointer to this struct (`Box<Node>` inside `Node` itself), and a
    // forward declaration of the tag is what makes that legal C.
    //
    // `repr(packed)` and `repr(align(N))` (kernel stage 2) are the GNU C
    // attributes with those names; the LLVM body carries the same two facts
    // (`codegen_llvm.rs::declare_aggregates`), which is what keeps the two
    // backends byte-for-byte identical.
    let packed = matches!(s.repr, crate::ast::Repr::Packed);
    write!(
        out,
        "typedef struct {}{} {{\n",
        if packed { "__attribute__((packed)) " } else { "" },
        crate::types::struct_tag(&s.name)
    )
    .unwrap();
    for f in &s.fields {
        write!(out, "    {};\n", f.ty.c_decl(&field_name(&f.name))).unwrap();
    }
    let ty = Ty::Struct(s.name.clone());
    let align = match s.repr {
        crate::ast::Repr::Align(n) => format!(" __attribute__((aligned({n})))"),
        _ => String::new(),
    };
    write!(out, "}}{align} {};\n", ty.c_type()).unwrap();
    // The declared destructor's prototype, right after the typedef that makes
    // its receiver a complete type and before the generated drop that calls
    // it (kernel stage 9, MK9-S6-M1). Its definition is written with the
    // program's other functions, further down.
    if drop_types.contains(&s.name) {
        writeln!(
            out,
            "static void {}({}* v);",
            crate::types::user_drop_fn_name(&s.name),
            ty.c_type()
        )
        .unwrap();
    }
    if needs_drop(&ty, owned) {
        writeln!(
            out,
            "static void kelp_drop_{}({}* v) {{",
            crate::types::c_ident(&s.name),
            ty.c_type()
        )
        .unwrap();
        if drop_types.contains(&s.name) {
            writeln!(out, "    {}(v);", crate::types::user_drop_fn_name(&s.name)).unwrap();
        }
        for f in &s.fields {
            if needs_drop(&f.ty, owned) {
                writeln!(
                    out,
                    "    {}(&v->{});",
                    drop_fn_name(&f.ty),
                    field_name(&f.name)
                )
                .unwrap();
            }
        }
        out.push_str("}\n");
    }
    // A derived deep copy (MISSING-jL9tE). Emitted beside the typedef, so a
    // field of an earlier `datac` already has a clone to call; `List`'s own
    // `clone`/`slice` is what reaches this from a container.
    if clonable.contains(&s.name) {
        writeln!(
            out,
            "static void kelp_clone_{}(const {}* src, {}* dst) {{",
            crate::types::c_ident(&s.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        for f in &s.fields {
            write_field_clone(
                out,
                &f.ty,
                &format!("dst->{}", field_name(&f.name)),
                &format!("src->{}", field_name(&f.name)),
            );
        }
        out.push_str("}\n");
    }
    // A derived order, field by field in declaration order, which is what
    // `List.sort` uses for a `datac` element.
    if sortable.contains(&s.name) {
        writeln!(
            out,
            "static int kelp_cmp_{}(const {}* a, const {}* b) {{",
            crate::types::c_ident(&s.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        for f in &s.fields {
            write_field_cmp(
                out,
                &f.ty,
                &format!("a->{}", field_name(&f.name)),
                &format!("b->{}", field_name(&f.name)),
            );
        }
        out.push_str("    return 0;\n}\n");
    }
    // A derived hash and key equality (A-6): a `datac` key whose every field
    // is keyable gets both, so it can be a `Map` key. Emitted beside the
    // typedef, like the clone and the order, so a field of an earlier keyable
    // type already has its own pair to call.
    if keyable.contains(&s.name) {
        writeln!(
            out,
            "static uint64_t kelp_hash_{}(const {}* v) {{",
            crate::types::c_ident(&s.name),
            ty.c_type()
        )
        .unwrap();
        out.push_str("    uint64_t h = 0;\n");
        for f in &s.fields {
            write_field_hash(out, &f.ty, &format!("v->{}", field_name(&f.name)));
        }
        out.push_str("    return h;\n}\n");
        writeln!(
            out,
            "static bool kelp_eq_{}(const {}* a, const {}* b) {{",
            crate::types::c_ident(&s.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        for f in &s.fields {
            write_field_eq(
                out,
                &f.ty,
                &format!("a->{}", field_name(&f.name)),
                &format!("b->{}", field_name(&f.name)),
            );
        }
        out.push_str("    return true;\n}\n");
    }
}

/// The generated C member name of one variant's payload struct inside an
/// `enum`'s union (`v_` keeps a variant named like a C keyword harmless, the
/// same job `field_name`'s `kelp_` prefix does for fields).
pub(crate) fn variant_member(name: &str) -> String {
    format!("kelp_v_{}", crate::types::c_ident(name))
}

/// One `enum`'s C typedef and drop function (A-1). A tagged union: the tag is
/// the variant's index, and each variant keeps its payload in a struct of its
/// own inside a union — so exactly one payload is live, which is the whole
/// difference from a `datac`.
///
/// A variant with no payload still gets a member with one `char` field: an
/// empty struct is not valid C, and an empty union would be neither valid nor
/// non-empty when every variant is payload-less (`enum Flag { On Off }`).
pub(crate) fn write_enum(
    out: &mut String,
    e: &EnumDef,
    owned: &HashSet<String>,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
    keyable: &HashSet<String>,
    drop_types: &HashSet<String>,
) {
    write!(out, "typedef struct {} {{\n", crate::types::struct_tag(&e.name)).unwrap();
    // The tag's width is the `repr(<int>)` clause's, `i32` by default
    // (kernel stage 2); every tag read, write and comparison in this file is
    // written against `kelp_tag`, so only this declaration changes.
    writeln!(out, "    {} kelp_tag;", e.tag_c_type()).unwrap();
    out.push_str("    union {\n");
    for v in &e.variants {
        writeln!(out, "        struct {{").unwrap();
        if v.is_payload_less() {
            out.push_str("            char kelp_none;\n");
        }
        for f in v.real_fields() {
            writeln!(out, "            {};", f.ty.c_decl(&field_name(&f.name))).unwrap();
        }
        writeln!(out, "        }} {};", variant_member(&v.name)).unwrap();
    }
    out.push_str("    } kelp_v;\n");
    let ty = Ty::Struct(e.name.clone());
    write!(out, "}} {};\n", ty.c_type()).unwrap();
    if drop_types.contains(&e.name) {
        writeln!(
            out,
            "static void {}({}* v);",
            crate::types::user_drop_fn_name(&e.name),
            ty.c_type()
        )
        .unwrap();
    }

    if needs_drop(&ty, owned) {
        writeln!(
            out,
            "static void kelp_drop_{}({}* v) {{",
            crate::types::c_ident(&e.name),
            ty.c_type()
        )
        .unwrap();
        if drop_types.contains(&e.name) {
            writeln!(out, "    {}(v);", crate::types::user_drop_fn_name(&e.name)).unwrap();
        }
        out.push_str("    switch (v->kelp_tag) {\n");
        for (i, v) in e.variants.iter().enumerate() {
            let owned_fields: Vec<&crate::types::FieldDef> = v
                .fields
                .iter()
                .filter(|f| needs_drop(&f.ty, owned))
                .collect();
            if owned_fields.is_empty() {
                continue;
            }
            writeln!(out, "    case {i}: {{").unwrap();
            for f in owned_fields {
                writeln!(
                    out,
                    "        {}(&v->kelp_v.{}.{});",
                    drop_fn_name(&f.ty),
                    variant_member(&v.name),
                    field_name(&f.name)
                )
                .unwrap();
            }
            out.push_str("        break;\n    }\n");
        }
        // No `default:` arm on purpose — `switch` on an unknown tag is a
        // compiler-visible hole, and every tag this program can produce is
        // one of the cases above.
        out.push_str("    }\n}\n");
    }

    // A derived deep copy: the tag, then the live variant's payload.
    if clonable.contains(&e.name) {
        writeln!(
            out,
            "static void kelp_clone_{}(const {}* src, {}* dst) {{",
            crate::types::c_ident(&e.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        out.push_str("    dst->kelp_tag = src->kelp_tag;\n    switch (src->kelp_tag) {\n");
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            writeln!(out, "    case {i}: {{").unwrap();
            for f in v.real_fields() {
                let at = format!("kelp_v.{}.{}", variant_member(&v.name), field_name(&f.name));
                write_field_clone(out, &f.ty, &format!("dst->{at}"), &format!("src->{at}"));
            }
            out.push_str("        break;\n    }\n");
        }
        out.push_str("    }\n}\n");
    }

    // A derived order: the tag first, so every value of one variant sorts
    // together, then the live variant's payload field by field.
    if sortable.contains(&e.name) {
        writeln!(
            out,
            "static int kelp_cmp_{}(const {}* a, const {}* b) {{",
            crate::types::c_ident(&e.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        out.push_str("    if (a->kelp_tag != b->kelp_tag) {\n        return a->kelp_tag < b->kelp_tag ? -1 : 1;\n    }\n    switch (a->kelp_tag) {\n");
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            writeln!(out, "    case {i}: {{").unwrap();
            for f in v.real_fields() {
                let at = format!("kelp_v.{}.{}", variant_member(&v.name), field_name(&f.name));
                write_field_cmp(out, &f.ty, &format!("a->{at}"), &format!("b->{at}"));
            }
            out.push_str("        break;\n    }\n");
        }
        out.push_str("    }\n    return 0;\n}\n");
    }

    // A derived hash and key equality (A-6): the tag folds first, so a value
    // of one variant is never confusable with another, then the live variant's
    // payload, the same shape the order below uses.
    if keyable.contains(&e.name) {
        writeln!(
            out,
            "static uint64_t kelp_hash_{}(const {}* v) {{",
            crate::types::c_ident(&e.name),
            ty.c_type()
        )
        .unwrap();
        out.push_str("    uint64_t h = 0;\n");
        out.push_str("    h = kelp_map_combine(h, kelp_map_mix((uint64_t)(v->kelp_tag)));\n");
        out.push_str("    switch (v->kelp_tag) {\n");
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            writeln!(out, "    case {i}: {{").unwrap();
            for f in v.real_fields() {
                let at = format!("v->kelp_v.{}.{}", variant_member(&v.name), field_name(&f.name));
                write_field_hash(out, &f.ty, &at);
            }
            out.push_str("        break;\n    }\n");
        }
        out.push_str("    }\n    return h;\n}\n");

        writeln!(
            out,
            "static bool kelp_eq_{}(const {}* a, const {}* b) {{",
            crate::types::c_ident(&e.name),
            ty.c_type(),
            ty.c_type()
        )
        .unwrap();
        out.push_str("    if (a->kelp_tag != b->kelp_tag) {\n        return false;\n    }\n    switch (a->kelp_tag) {\n");
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            writeln!(out, "    case {i}: {{").unwrap();
            for f in v.real_fields() {
                let at = format!("kelp_v.{}.{}", variant_member(&v.name), field_name(&f.name));
                write_field_eq(out, &f.ty, &format!("a->{at}"), &format!("b->{at}"));
            }
            out.push_str("        break;\n    }\n");
        }
        out.push_str("    }\n    return true;\n}\n");
    }
}

/// The body of one map entry, split from `write_container_typedef` because both
/// the key and the value are held by value and their typedefs are only complete
/// after the `datac` declarations.
pub(crate) fn write_map_entry_body(out: &mut String, key: &Ty, value: &Ty) {
    let name = crate::types::map_c_type(key, value);
    let entry = format!("{name}_entry");
    writeln!(
        out,
        "struct {entry} {{\n    {} key;\n    {} value;\n    uint8_t live;\n}};",
        key.c_type(),
        crate::types::container_elem_c_type(value)
    )
    .unwrap();
}

/// A container's C typedef, emitted before every `datac`: both containers
/// hold their contents through a pointer, so a container can be named by the
/// `datac`s that use it (including the `datac` whose *own* body has not been
/// emitted yet (`List<Tree>` inside `Tree`) and a mutually recursive pair
/// (M8). A `datac` element is therefore spelled by its struct tag here
/// (`container_elem_c_type`), which is forward-declared by `write_tags`.
///
/// Both `generate`'s whole-program declaration pass and
/// [`write_referenced_type_layouts`] call it, so a container has one C typedef
/// in both backends.
pub(crate) fn write_container_typedef(out: &mut String, ty: &Ty) {
    match ty {
        // A scoped function value is the same fat pointer, so it is the same
        // typedef rather than a second, identical one.
        Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
            write_closure_typedef(out, params, ret);
        }
        Ty::List(elem) => {
            let name = crate::types::list_c_type(elem);
            writeln!(
                out,
                "typedef struct {{\n    {}* data;\n    size_t len;\n    size_t cap;\n}} {name};",
                crate::types::container_elem_c_type(elem)
            )
            .unwrap();
        }
        Ty::Box(elem) => {
            let name = crate::types::box_c_type(elem);
            writeln!(
                out,
                "typedef struct {{\n    {}* value;\n}} {name};",
                crate::types::container_elem_c_type(elem)
            )
            .unwrap();
        }
        // A map stores its entries in insertion order and keeps a separate
        // open-addressed index of `entry index + 1` in a `uint32_t` (0 empty,
        // `UINT32_MAX` tombstone), because `keys()` must return insertion
        // order. Both the key and the value sit inline in the entry, so the
        // entry needs a complete key and value type: it is forward-declared
        // here and its body is written once every `datac` typedef exists
        // (`write_map_entry_body`). The map itself only holds a pointer to the
        // entry, so it stays complete here regardless.
        Ty::Map(key, value) => {
            let name = crate::types::map_c_type(key, value);
            let entry = format!("{name}_entry");
            writeln!(out, "typedef struct {entry} {entry};").unwrap();
            writeln!(
                out,
                "typedef struct {{\n    {entry}* entries;\n    size_t entry_count;\n    size_t entry_cap;\n    size_t len;\n    uint32_t* index;\n    size_t index_cap;\n    size_t index_used;\n}} {name};"
            )
            .unwrap();
        }
        other => unreachable!("not a container: {}", other.display()),
    }
}

/// One closure instantiation's C typedef (A-4 phase E): the fat pointer a
/// capturing `fun` literal becomes. Three members, and each earns its place:
///
/// * `fn` — the generated function, whose first parameter is the environment.
///   Making it typed (rather than `void*`) is why the closure is
///   monomorphized per signature, exactly like a container.
/// * `env` — the environment itself, type-erased: the layout belongs to the
///   one literal that made it, and casting it back inside the generated
///   function is what keeps that layout out of the type system.
/// * `drop` — how to free the environment. The value cannot know its own
///   layout, so it carries the destructor beside the pointer; without that a
///   closure could not drop what it captured.
///
/// A parameter or return type that is a `datac` is spelled by its struct tag
/// here, like a container element: this typedef is emitted before every
/// `datac`, and `write_tags` has forward-declared the tags.
pub(crate) fn write_closure_typedef(out: &mut String, params: &[Ty], ret: &Ty) {
    let name = crate::types::closure_c_type(params, ret);
    // The `fn` member's own parameter list: the environment first, then the
    // closure's declared parameters.
    let list = if params.is_empty() {
        "void*".to_string()
    } else {
        let rest = params
            .iter()
            .map(crate::types::container_elem_c_type)
            .collect::<Vec<_>>()
            .join(", ");
        format!("void*, {rest}")
    };
    writeln!(
        out,
        "typedef struct {{\n    {} (*fn)({});\n    void* env;\n    void (*drop)(void* env);\n}} {name};",
        crate::types::container_elem_c_type(ret),
        list
    )
    .unwrap();
}

/// The generated C name of one closure signature's native-callback thunk
/// (gap 4). The thunk is the `func` half of the `(func, user_data,
/// destroy_notify)` triple a user `extern fun`'s `move closure(...)` parameter
/// expands into. One per signature, not per call site, because what it does is
/// the same for every closure of that shape.
pub(crate) fn callback_thunk_name(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_cb_thunk__{}", crate::types::closure_mangle(params, ret))
}

/// The generated C name of one closure signature's callback destructor
/// (gap 4): the `destroy_notify` half of the triple, which runs the closure's
/// drop on its environment and frees the box that carried it.
pub(crate) fn callback_destroy_name(params: &[Ty], ret: &Ty) -> String {
    format!(
        "kelp_cb_destroy__{}",
        crate::types::closure_mangle(params, ret)
    )
}

/// The closure signatures a program's user `extern fun`s take as native
/// callbacks (gap 4), one per signature, sorted for a deterministic emitted
/// order. A stdlib extern is not a user binding and expands nothing, so it is
/// left out by the same `extern_ffi_symbols` test `write_extern` uses.
pub(crate) fn callback_signatures(program: &Program) -> Vec<Ty> {
    let mut sigs: Vec<Ty> = Vec::new();
    for f in &program.functions {
        if !f.extern_ || !program.extern_ffi_symbols.contains_key(&f.name) {
            continue;
        }
        for (_, ty) in &f.params {
            if matches!(ty, Ty::Closure { .. }) && !sigs.contains(ty) {
                sigs.push(ty.clone());
            }
        }
    }
    sigs.sort_by_key(|t| container_name(t));
    sigs
}

/// One closure signature's native-callback glue (gap 4): the thunk a native
/// `(func, user_data, destroy_notify)` triple takes as `func`, and the
/// destructor it takes as `destroy_notify`.
///
/// GLib's convention (and GTK's signal handlers) put `user_data` last, while a
/// Kelp closure's `fn` member takes its environment first, so the thunk is
/// what reorders the two: the native call's `user_data` is the boxed closure
/// `write_user_extern` handed over, and `c->fn(c->env, a0, ...)` is the
/// closure's own calling convention. The destructor runs the closure's `drop`
/// on the environment and frees the box; `c->env` is `NULL` exactly when the
/// closure was widened from a plain function value, and then there is nothing
/// to drop.
///
/// This is emitted once per signature by both backends (`generate` here, and
/// `codegen_llvm.rs::program_c_source` in the per-program C object), so the
/// two agree on the glue text by construction.
pub(crate) fn write_callback_glue(out: &mut String, sig: &Ty) {
    let Ty::Closure { params, ret } = sig else {
        unreachable!("callback glue is emitted per closure signature");
    };
    let name = sig.c_type();
    let thunk = callback_thunk_name(params, ret);
    let destroy = callback_destroy_name(params, ret);
    // A `unit` argument has no C value, so it takes no place in the thunk,
    // exactly as it takes none in any other generated signature.
    let real: Vec<&Ty> = params.iter().filter(|p| **p != Ty::Unit).collect();
    let names: Vec<String> = (0..real.len()).map(|i| format!("a{i}")).collect();
    let decls = if real.is_empty() {
        "void* user_data".to_string()
    } else {
        real.iter()
            .zip(&names)
            .map(|(p, n)| format!("{} {n}", crate::types::container_elem_c_type(p)))
            .chain(std::iter::once("void* user_data".to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    writeln!(out, "// ---- {thunk} (gap 4) {}", "-".repeat(30)).unwrap();
    writeln!(out, "static {} {thunk}({decls}) {{", ret.c_type()).unwrap();
    writeln!(out, "    {name}* c = ({name}*)user_data;").unwrap();
    let mut forwarded = vec!["c->env".to_string()];
    forwarded.extend(names.iter().cloned());
    let call = format!("c->fn({})", forwarded.join(", "));
    if **ret == Ty::Unit {
        writeln!(out, "    {call};").unwrap();
    } else {
        writeln!(out, "    return {call};").unwrap();
    }
    writeln!(
        out,
        "}}\nstatic void {destroy}(void* user_data) {{\n    {name}* c = ({name}*)user_data;\n    if (c->env != NULL) {{\n        c->drop(c->env);\n        c->env = NULL;\n    }}\n    free(c);\n}}\n"
    )
    .unwrap();
}

/// `struct tag_Foo;` for every `datac`, so a container typedef can hold a
/// pointer to a struct whose body comes later.
///
/// The two callers pass different sets: `generate` every declaration the
/// program has, and [`write_referenced_type_layouts`] the subset an `extern fun`
/// signature reaches. The iterator parameters are what let both share this one
/// pass rather than filter two cloned `Vec`s.
pub(crate) fn write_tags<'a, S, E>(out: &mut String, structs: S, enums: E)
where
    S: IntoIterator<Item = &'a StructDef>,
    E: IntoIterator<Item = &'a EnumDef>,
{
    // Both kinds are typedef'd through a tagged struct, so both need their tag
    // forward-declared: a container typedef emitted before them may hold a
    // pointer to either (`Box<Node>`, `List<Shape>`).
    for s in structs {
        writeln!(out, "struct {};", crate::types::struct_tag(&s.name)).unwrap();
    }
    for e in enums {
        writeln!(out, "struct {};", crate::types::struct_tag(&e.name)).unwrap();
    }
}

/// One `Atomic<T>` instantiation's C typedef (MK9-S3-M1 stage A1): one member
/// of the element's scalar type, which is why an atomic names no `datac` and
/// needs no forward declaration before it.
pub(crate) fn write_atomic_typedef(out: &mut String, ty: &Ty) {
    let Ty::Atomic(elem) = ty else {
        unreachable!("only an atomic instantiation is in this set")
    };
    writeln!(
        out,
        "typedef struct {{ {} v; }} {};",
        elem.c_type(),
        ty.c_type()
    )
    .unwrap();
}

/// The C declarations every type in `roots` needs, in the order the layouts
/// require: the `datac` and `enum` tags reached, forward-declared; the
/// container, array and atomic instantiations their fields hold; and then the
/// `datac` and `enum` typedefs themselves, in the program's merged declaration
/// order.
///
/// The LLVM backend's extern prelude is the caller
/// (`codegen_llvm.rs::program_c_source`): an `extern fun` body is C text whose
/// signature names the Kelp types its declaration names, and C needs those
/// types to exist in the same translation unit. This is deliberate reuse of the
/// C backend's own emitters (`write_tags`, `write_container_typedef`,
/// `write_array`, `write_atomic_typedef`, `write_struct`, `write_enum`), so a
/// `datac` or an `enum` has one C layout in both backends;
/// `layout_tests::an_extern_signature_that_names_a_kelp_type_gets_its_typedef`
/// is what holds the two ends together.
///
/// The derive sets are empty here on purpose: an extern body only ever *names*
/// these types, so it never calls a drop, a clone, a derived order or a hash.
///
/// `roots` is what the caller's own text names, one entry per parameter and one
/// for the return. What those reach is walked too, because a `datac` field and
/// an `enum` payload are places C requires a typedef to already be complete.
pub(crate) fn write_referenced_type_layouts(
    out: &mut String,
    structs: &[StructDef],
    enums: &[EnumDef],
    roots: &[Ty],
) {
    let mut named: HashSet<String> = HashSet::new();
    let mut containers: Vec<Ty> = Vec::new();
    let mut arrays: Vec<Ty> = Vec::new();
    let mut atomics: Vec<Ty> = Vec::new();
    let mut handles: HashSet<String> = HashSet::new();
    for ty in roots {
        collect_layout_tys(
            ty,
            structs,
            enums,
            &mut named,
            &mut containers,
            &mut arrays,
            &mut atomics,
            &mut handles,
        );
    }
    if named.is_empty()
        && containers.is_empty()
        && arrays.is_empty()
        && atomics.is_empty()
        && handles.is_empty()
    {
        return;
    }
    out.push('\n');
    // An opaque handle is a `void*` typedef, and a container typedef emitted
    // just below may name one, so the typedefs come first.
    let mut handles: Vec<&String> = handles.iter().collect();
    handles.sort();
    for name in handles {
        writeln!(out, "typedef void* {};", crate::types::handle_c_type(name)).unwrap();
    }
    write_tags(
        out,
        structs.iter().filter(|s| named.contains(&s.name)),
        enums.iter().filter(|e| named.contains(&e.name)),
    );

    // The sort is `generate`'s own: a container is emitted after the containers
    // it contains, and every array gets its forward typedef first because a
    // container may hold it behind a pointer.
    containers.sort_by_key(|t| (container_depth(t), container_name(t)));
    arrays.sort_by_key(|t| (container_depth(t), container_name(t)));
    atomics.sort_by_key(|t| crate::types::ty_mangle(t));
    if !containers.is_empty() || !arrays.is_empty() {
        for ty in &arrays {
            let name = container_name(ty);
            writeln!(out, "typedef struct {name} {name};").unwrap();
        }
        out.push('\n');
        // A scoped function value and a closure with the same signature are
        // two `Ty`s but one C typedef, so a name is emitted once.
        let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ty in &containers {
            if !emitted.insert(crate::types::ty_mangle(ty)) {
                continue;
            }
            write_container_typedef(out, ty);
        }
        out.push('\n');
    }
    for ty in &atomics {
        write_atomic_typedef(out, ty);
    }
    if !atomics.is_empty() {
        out.push('\n');
    }

    let mut decls: Vec<(usize, Decl)> = structs
        .iter()
        .filter(|s| named.contains(&s.name))
        .map(|s| (s.order, Decl::Record(s)))
        .chain(
            enums
                .iter()
                .filter(|e| named.contains(&e.name))
                .map(|e| (e.order, Decl::Sum(e))),
        )
        .collect();
    decls.sort_by_key(|(order, _)| *order);
    // An empty derive set is what makes `write_struct`/`write_enum` write the
    // typedef alone, which is the same call `generate` makes with its own sets.
    let no_derives: HashSet<String> = HashSet::new();
    let mut emitted_arrays: HashSet<Ty> = HashSet::new();
    for (_, decl) in &decls {
        for ty in decl.field_tys() {
            emit_arrays_for(out, ty, &mut emitted_arrays, &no_derives);
        }
        match decl {
            Decl::Record(s) => {
                write_struct(out, s, &no_derives, &no_derives, &no_derives, &no_derives, &no_derives)
            }
            Decl::Sum(e) => write_enum(out, e, &no_derives, &no_derives, &no_derives, &no_derives, &no_derives),
        }
    }
    if !decls.is_empty() {
        out.push('\n');
    }

    // Every map entry body is written here, after the `datac` typedefs that
    // make an inline key or value a complete C type, exactly where `generate`
    // writes it.
    for ty in &containers {
        if let Ty::Map(key, value) = ty {
            write_map_entry_body(out, key, value);
        }
    }

    // Every array the roots reach that no declaration pulled in earlier.
    for ty in &arrays {
        emit_arrays_for(out, ty, &mut emitted_arrays, &no_derives);
    }
}

/// Every `datac` and `enum` name `ty` reaches, and every container, array and
/// atomic instantiation on the way, for [`write_referenced_type_layouts`].
///
/// A `datac`'s fields and an `enum`'s payloads are walked because those are the
/// positions a typedef has to be complete in, and the `named` set is what stops
/// a type that reaches itself through a `Box` from being walked twice.
pub(crate) fn collect_layout_tys(
    ty: &Ty,
    structs: &[StructDef],
    enums: &[EnumDef],
    named: &mut HashSet<String>,
    containers: &mut Vec<Ty>,
    arrays: &mut Vec<Ty>,
    atomics: &mut Vec<Ty>,
    handles: &mut HashSet<String>,
) {
    match ty {
        Ty::Struct(name) => {
            if !named.insert(name.clone()) {
                return;
            }
            if let Some(s) = structs.iter().find(|s| &s.name == name) {
                for f in &s.fields {
                    collect_layout_tys(
                        &f.ty, structs, enums, named, containers, arrays, atomics, handles,
                    );
                }
            }
            if let Some(e) = enums.iter().find(|e| &e.name == name) {
                for v in &e.variants {
                    for f in &v.fields {
                        collect_layout_tys(
                            &f.ty, structs, enums, named, containers, arrays, atomics, handles,
                        );
                    }
                }
            }
        }
        Ty::Handle(name) => {
            handles.insert(name.clone());
        }
        Ty::List(elem) | Ty::Box(elem) => {
            if !containers.contains(ty) {
                containers.push(ty.clone());
            }
            collect_layout_tys(elem, structs, enums, named, containers, arrays, atomics, handles);
        }
        Ty::Map(key, value) => {
            if !containers.contains(ty) {
                containers.push(ty.clone());
            }
            collect_layout_tys(key, structs, enums, named, containers, arrays, atomics, handles);
            collect_layout_tys(value, structs, enums, named, containers, arrays, atomics, handles);
        }
        Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
            if !containers.contains(ty) {
                containers.push(ty.clone());
            }
            for p in params {
                collect_layout_tys(p, structs, enums, named, containers, arrays, atomics, handles);
            }
            collect_layout_tys(ret, structs, enums, named, containers, arrays, atomics, handles);
        }
        Ty::Array(elem, _) => {
            if !arrays.contains(ty) {
                arrays.push(ty.clone());
            }
            collect_layout_tys(elem, structs, enums, named, containers, arrays, atomics, handles);
        }
        Ty::Atomic(elem) => {
            if !atomics.contains(ty) {
                atomics.push(ty.clone());
            }
            collect_layout_tys(elem, structs, enums, named, containers, arrays, atomics, handles);
        }
        Ty::Ref { inner, .. } => collect_layout_tys(
            inner, structs, enums, named, containers, arrays, atomics, handles,
        ),
        // A raw pointer's C spelling is `<elem>*`, so an extern signature that
        // names `Ptr<Packed>` needs `Packed`'s typedef just as a bare `Packed`
        // parameter would (kernel stage 2).
        Ty::Ptr(elem) => collect_layout_tys(
            elem, structs, enums, named, containers, arrays, atomics, handles,
        ),
        Ty::Fn { params, ret } => {
            for p in params {
                collect_layout_tys(p, structs, enums, named, containers, arrays, atomics, handles);
            }
            collect_layout_tys(ret, structs, enums, named, containers, arrays, atomics, handles);
        }
        _ => {}
    }
}

/// The C name of a container instantiation, without the `kelp_` prefix the
/// functions append: `kelp_List__i32` / `kelp_Box__Node`.
pub(crate) fn container_name(ty: &Ty) -> String {
    match ty {
        Ty::List(elem) => crate::types::list_c_type(elem),
        Ty::Box(elem) => crate::types::box_c_type(elem),
        Ty::Map(key, value) => crate::types::map_c_type(key, value),
        Ty::Closure { .. } | Ty::ScopedFn { .. } => ty.c_type(),
        Ty::Array(elem, len) => crate::types::array_c_type(elem, *len),
        other => unreachable!("not a container: {}", other.display()),
    }
}

/// The one `[T; N]` instantiation C text: the struct that *is* the storage
/// (A-18), the small function set its methods call, and its drop.
///
/// Nothing here allocates. The struct holds `N` elements inline, so an array
/// lives wherever its value lives: a stack frame, a `datac` field, or a
/// `Box`'s heap cell. That is the whole point of the type, since a page table,
/// an IDT/GDT or a register-save frame has to exist with no allocator in the
/// path.
///
/// The struct's tag was typedef'd with the other forward declarations, because
/// a `Box<[T; N]>` typedef written earlier holds a *pointer* to one.
pub(crate) fn write_array(out: &mut String, elem: &Ty, len: u64, owned: &HashSet<String>) {
    let name = crate::types::array_c_type(elem, len);
    let elem_c = crate::types::container_elem_c_type(elem);
    let elem_drops = needs_drop(elem, owned);
    let drop_elem = elem_drops.then(|| drop_fn_name(elem));
    writeln!(out, "// ---- {name} (fixed-size array) {}", "-".repeat(30)).unwrap();
    writeln!(out, "struct {name} {{\n    {elem_c} v[{len}];\n}};\n").unwrap();

    writeln!(out, "static size_t {name}__len(const {name}* self) {{\n    (void)self;\n    return {len};\n}}\n").unwrap();
    writeln!(out, "static bool {name}__is_empty(const {name}* self) {{\n    (void)self;\n    return false;\n}}\n").unwrap();
    writeln!(out, "static void {name}__bounds(size_t i) {{\n    if (i >= {len}) {{\n        kelp_panic(\"Array.get: index out of bounds\");\n    }}\n}}\n").unwrap();
    if elem.is_copy() {
        writeln!(out, "static {elem_c} {name}__get(const {name}* self, size_t i) {{\n    {name}__bounds(i);\n    return self->v[i];\n}}\n").unwrap();
    } else {
        writeln!(out, "static const {elem_c}* {name}__get_const(const {name}* self, size_t i) {{\n    {name}__bounds(i);\n    return &self->v[i];\n}}\n").unwrap();
    }
    writeln!(out, "static void {name}__set({name}* self, size_t i, {elem_c} v) {{\n    {name}__bounds(i);").unwrap();
    if elem_drops {
        writeln!(out, "    {}(&self->v[i]);", drop_elem.as_deref().unwrap()).unwrap();
    }
    out.push_str("    self->v[i] = v;\n}\n\n");

    if elem_drops {
        writeln!(
            out,
            "static void {}({name}* self) {{\n    for (size_t i = 0; i < {len}; i++) {{\n        {}(&self->v[i]);\n    }}\n}}\n",
            crate::types::drop_fn_name(&Ty::Array(Box::new(elem.clone()), len)),
            drop_elem.as_deref().unwrap()
        )
        .unwrap();
    }

    // The repeat literal's helper: `[x; N]` is one call rather than N copies
    // in the emitted C, and a big zero fill stays a loop the C compiler can
    // turn into a `memset`.
    writeln!(out, "static {name} {name}__fill({elem_c} v) {{\n    {name} r;\n    for (size_t i = 0; i < {len}; i++) {{\n        r.v[i] = v;\n    }}\n    return r;\n}}\n").unwrap();
}

/// Emits whichever array instantiations a type needs before it can be written
/// itself: the array when `ty` is one (after any array nested inside it), and
/// nothing for a type that holds its array behind a pointer, which the forward
/// typedef already covers.
pub(crate) fn emit_arrays_for(
    out: &mut String,
    ty: &Ty,
    emitted: &mut HashSet<Ty>,
    owned: &HashSet<String>,
) {
    match ty {
        Ty::Array(elem, len) => {
            emit_arrays_for(out, elem, emitted, owned);
            if emitted.insert(ty.clone()) {
                write_array(out, elem, *len, owned);
            }
        }
        Ty::Ref { inner, .. } => emit_arrays_for(out, inner, emitted, owned),
        _ => {}
    }
}

/// How deeply containers nest: `List<i32>` is 1, `Box<List<i32>>` is 2. Used
/// to emit a container after the containers it contains, so every typedef and
/// every function it calls already exists. A closure counts here for the same
/// reason: one may name another in its signature, and the inner one has to
/// exist first.
pub(crate) fn container_depth(ty: &Ty) -> usize {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => 1 + container_depth(elem),
        // A `Map<V>` value is never a container (`is_map_element` refuses
        // one), so a map is always one level deep.
        Ty::Map(..) => 1,
        Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
            1 + params
                .iter()
                .map(container_depth)
                .chain(std::iter::once(container_depth(ret)))
                .max()
                .unwrap_or(0)
        }
        // A closure signature's parameter is borrowed, so its C typedef spells
        // the pointee (`kelp_List__i32*`), and the pointee has to be complete
        // first: look through the borrow when computing depth.
        Ty::Ref { inner, .. } => container_depth(inner),
        // A nested array is held inline, so it has to be complete before the
        // array that contains it: `[[u8; 4]; 8]` emits the inner one first.
        Ty::Array(elem, _) => 1 + container_depth(elem),
        _ => 0,
    }
}

/// C prototypes for the compiler's runtime module (`std/rt`) as it appears in
/// this program.
///
/// The capability checks are emitted before the program's own functions and
/// call into this module, so the declarations have to come first. They are
/// generated from the module's functions rather than written out as text, so a
/// rename in `std/rt.kp` cannot leave a stale name behind in a C string: the
/// prototype and the definition are two spellings of one `c_name`.
pub(crate) fn runtime_prototypes(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("// The capability matcher, written in Kelp (`std/rt`): one\n");
    out.push_str("// implementation for every backend (MK7 \u{a7}8).\n");
    for f in program
        .functions
        .iter()
        .filter(|f| f.module == crate::stdlib::RT_MODULE)
    {
        write_signature(&mut out, f);
        out.push_str(";\n");
    }
    out
}

pub(crate) fn write_signature(out: &mut String, f: &Function) {
    // A function that *returns* a function value is the one declarator C
    // cannot write as `ret name(params)`: the returned pointer's own
    // parameter list has to be attached to the name, before this function's.
    // One level is what a function type is allowed to be here (a function
    // type may not return a function type — `infer.rs` refuses that), so one
    // level is what this handles. A literal that captured something has a
    // hidden environment parameter (A-4 phase E), which goes first in either
    // spelling.
    let env = f.env.is_some();
    match &f.ret {
        Ty::Fn { params, ret } => {
            write!(
                out,
                "{} (*{}(",
                ret.c_type(),
                c_name(&f.name)
            )
            .unwrap();
            write_params(out, &f.params, env);
            write!(out, "))({})", fn_params_of(params)).unwrap();
            return;
        }
        other => {
            write!(out, "{} {}(", other.c_type(), c_name(&f.name)).unwrap();
        }
    }
    write_params(out, &f.params, env);
    out.push(')');
}

/// The parenthesized parameter list of a function's own signature — `void`
/// when it takes none, which is what a C definition requires. A literal that
/// captured something receives its environment first, as a `void*` the body
/// casts back to the literal's own environment struct: the environment's
/// layout stays out of the type system, which is what lets two closures of one
/// signature share a C type.
pub(crate) fn write_params(out: &mut String, params: &[(String, Ty)], env: bool) {
    let mut first = true;
    if env {
        out.push_str("void* env_raw");
        first = false;
    }
    // A `unit` parameter has no C type to be declared with (`void kelp_u` is
    // not a declaration), and no value crosses for it: it is left out of the
    // signature entirely and a read of it is a no-op expression
    // (`Gen::expr_value`). No `unit` parameter is written, so a function whose
    // parameters are all `unit` still gets the `void` a C definition requires.
    if params.iter().all(|(_, ty)| *ty == Ty::Unit) && first {
        out.push_str("void");
    }
    for (name, ty) in params.iter() {
        if *ty == Ty::Unit {
            continue;
        }
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&ty.c_decl(&c_name(name)));
    }
}

/// A function type's own parameter list, as C — the same text a declarator
/// needs after the name, so `types.rs` and this emitter agree on one spelling:
/// `types::fn_params_c` is that spelling, and a `unit` parameter is left out of
/// it.
pub(crate) fn fn_params_of(params: &[Ty]) -> String {
    crate::types::fn_params_c(params)
}

/// A template with each `{k}` placeholder replaced by `sigil` and
/// `numbers[k]`, the backend's own operand number. A literal `sigil` is
/// doubled, which is how LLVM reads `$$` and GCC reads `%%`; the other
/// backend's sigil is ordinary text (`%` is not special to LLVM's x86
/// templates, and `$` is bare in GCC's). Shared with the LLVM backend for the
/// same reason `c_string_literal` is: the two must translate one template the
/// same way.
pub(crate) fn asm_operand_text(template: &str, numbers: &[usize], sigil: char) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == sigil {
            out.push(sigil);
            out.push(sigil);
            i += 1;
        } else if c == '{' {
            let close = chars[i + 1..].iter().position(|&c| c == '}');
            let number = close.and_then(|close| {
                chars[i + 1..i + 1 + close]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                    .ok()
                    .map(|n| (n, close))
            });
            match number {
                Some((n, close)) if n < numbers.len() => {
                    out.push(sigil);
                    out.push_str(&numbers[n].to_string());
                    i += close + 2;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// A C string literal for `s`, escapes included. Shared with the LLVM
/// backend, which writes a program's capability allowlist arrays as C data in
/// the object it links (`codegen_llvm.rs::program_c_source`), so the escaping
/// of a pattern cannot differ between the backends. Visibility only: the text
/// the C backend generates is unchanged.
pub(crate) fn c_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            // CR was missing, and a raw carriage return inside a C string
            // literal is not a string at all — it breaks the generated line.
            // Nothing could reach this until the lexer learned `\r` (S4), so a
            // protocol that ends its lines with CRLF is what found it.
            '\r' => out.push_str("\\r"),
            // Any other control byte goes out as three octal digits. Not
            // `\x`: C's hex escape swallows the hex digits that follow it, so
            // `\x0d` followed by "a" would be one escape, not two characters.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `.kp` identifiers are valid C identifiers as-is; prefix to dodge any
/// accidental collision with a C keyword or our own runtime symbols.
/// `main` is left alone since the C runtime requires that exact name for
/// the process entry point. Qualified names (M5 module prefixes, `Type#name`
/// method keys) go through `types::c_ident`.
pub(crate) fn c_name(name: &str) -> String {
    if name == "main" {
        name.to_string()
    } else {
        format!("kelp_{}", crate::types::c_ident(name))
    }
}

/// Struct field names live in C's per-struct member namespace, not the
/// ordinary identifier namespace, but prefixed anyway for consistency and to
/// dodge a C keyword used as a field name.
pub(crate) fn field_name(name: &str) -> String {
    format!("kelp_{name}")
}

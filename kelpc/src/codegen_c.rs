//! Lowers the typed IR (`tir`) to C source text.
//!
//! Blocks and `if` used in *value* position (e.g. the RHS of a `let`) are
//! emitted using the GNU statement-expression extension `({ ...; expr; })`,
//! which both `gcc` and `clang` support. This is what lets `let x = if c {
//! 1 } else { 2 };` compile to C at all, since plain C has no
//! expression-valued blocks. When a block/`if` is used as a bare statement
//! its value (if any) is simply discarded, so no extension is needed there.
//!
//! M6 adds `Bytes`, and with it the two things a heap type needs at this
//! layer: a fixed runtime preamble (allocation, bounds checks, `free`) and
//! the actual `free` calls. Where those go is decided by `borrowck.rs`, which
//! hands over a `DropPlan` keyed by span, so this pass only has to ask "which
//! locals die at the end of this block" and "which die before this return" as
//! it writes each one — no second walk, and no lockstep between two passes.

use crate::ast::{BinOp, UnOp};
use crate::borrowck::{stmt_id, BindingId, DropPlan, DropTarget, Projection};
use crate::diag::SourceMap;
use crate::lexer::Span;
use crate::tir::*;
use crate::types::{clone_fn_name, cmp_fn_name, drop_fn_name, eq_fn_name, hash_fn_name, needs_drop, owned_types, EnumDef, StructDef, Ty};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// The fixed C support code every generated program carries: the `Bytes`
/// representation, the bodies of its builtin methods, and the panic path.
/// These are the bodies `infer.rs`'s builtin table promises exist; a builtin
/// the runtime does not define would fail at `cc`, which is the right place
/// for that mistake to surface.
///
/// `kelp_bytes_drop` takes a pointer and clears it, so a drop is idempotent at
/// runtime as well as unreachable twice by construction — together with the
/// move checking that is what makes "no double free is expressible" testable
/// rather than merely argued.
const RUNTIME: &str = include_str!("../runtime/kelp_rt.c");

/// `List#sort`'s introsort: a partition at or below this many elements
/// finishes with insertion sort instead of recursing further. 16-32 is the
/// textbook range (it is what libstdc++'s own introsort threshold, 16, comes
/// from); 32, the top of that range, is what benchmarking actually picked
/// here: `median3`, `partition` and `cmp` are separate emitted functions
/// rather than something the source-level optimizer can inline into one
/// quicksort loop, so each partition step pays real call overhead on top of
/// the comparisons, and a partition small enough to hand to insertion sort
/// skips that call chain entirely. Measured on
/// `benchmarks/sorting/sort.kp` (n = 5,000,000): 16 landed at 254ms, 24 at
/// 243ms, 32 at 238ms, with diminishing returns above that (a further push to
/// 64 was only 227ms) — 32 stays inside the well-tested range while clearing
/// the 250ms target with headroom, rather than chasing the last few
/// milliseconds outside it. Shared with the LLVM backend's
/// `KELP_SORT_INSERTION_CUTOFF`.
const KELP_SORT_INSERTION_CUTOFF: u64 = 32;

/// `List#sort`'s partition, branchless block size: the number of elements
/// scanned per side, per round, before the collected "wrong side" offsets get
/// swapped into place. This is pdqsort's own technique (itself borrowed from
/// BlockQuicksort), not plain introsort: a textbook Hoare partition has a
/// data-dependent conditional branch on every comparison (advance the scan
/// or stop), which mispredicts constantly on random data, since which side a
/// given element belongs on is exactly the unpredictable thing being
/// computed. The block scan replaces that per-element branch with a
/// per-element comparison plus an unconditional store into a small offsets
/// buffer, with the branch outcome folded into a counter increment
/// (`num_l += (cmp(...) >= 0)`) instead of a control-flow branch; a decent C
/// compiler turns that into a conditional move, not a mispredictable branch.
/// The actual element swaps then happen afterward in a second, tight loop
/// over the matched offsets, which is itself branch-free and has excellent
/// spatial locality. 64 is pdqsort's own published block size and what its
/// reference implementation and Rust's `core::slice::sort` both use; there is
/// nothing benchmarked-and-tuned about this specific number the way the
/// insertion cutoff above was, it is simply the well-established constant for
/// this technique. Shared with the LLVM backend's `KELP_SORT_BLOCK`.
const KELP_SORT_BLOCK: u64 = 64;

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
// `std/tls`'s externs (MK5-S4b-M1, open item 37) are the only OpenSSL-touching\n\
// code in the compiler, but the declarations are unconditional here, the same\n\
// as `math.h` above: an unused declaration costs nothing, and `driver.rs`\n\
// already links `-lssl -lcrypto` unconditionally for the same reason.\n\
#include <openssl/err.h>\n\
#include <openssl/ssl.h>\n\
#include <openssl/x509v3.h>\n\
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
}\n\n";

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

/// Which function keys are gated, and each module's patterns per domain.
/// Built once per program in `generate`; `Gen` carries a reference to it so
/// every call site can decide whether a check is needed without threading
/// the whole program through the emitter.
struct CapTable {
    gated: HashMap<String, String>,
    patterns: HashMap<(String, String), Vec<String>>,
}

impl CapTable {
    /// `None` means the calling module holds the domain but has no pattern
    /// list recorded — impossible for a gated call, since the gate is what
    /// admits the call at all.
    fn patterns_for(&self, module: &str, domain: &str) -> Option<&Vec<String>> {
        self.patterns.get(&(module.to_string(), domain.to_string()))
    }

    /// The name of the compiled-in pattern array for one module and domain.
    fn symbol(module: &str, domain: &str) -> String {
        format!(
            "kelp_caps_{}_{}",
            crate::types::c_ident(&module.replace('/', "_")),
            domain
        )
    }
}

/// One declaration in the program's merged `datac`/`enum` order (A-1), so the
/// two kinds can be emitted interleaved. `order` is a topological order across
/// both kinds, which is what makes every typedef complete at the point it is
/// used.
enum Decl<'a> {
    Record(&'a StructDef),
    Sum(&'a EnumDef),
}

impl Decl<'_> {
    /// Every type this declaration holds, by value or behind a pointer: what
    /// `emit_arrays_for` scans for the array bodies a declaration needs first.
    fn field_tys(&self) -> Vec<&Ty> {
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

pub fn generate(program: &Program, drops: &DropPlan, sources: &SourceMap) -> String {
    let owned = owned_types(&program.structs, &program.enums);
    // The user types with a derived deep copy and a derived order
    // (MISSING-jL9tE): the least fixpoint over the declaration graph, computed
    // once here for the same reasons `owned` is — `infer.rs` offers
    // `List.clone`/`List.sort` for exactly the types these sets name, and if
    // the two disagreed the checker would offer a signature with no body.
    let type_decls = type_decls(&program.structs, &program.enums);
    let clonable = crate::types::clonable_types(&type_decls);
    let sortable = crate::types::sortable_types(&type_decls);
    let keyable = crate::types::keyable_types(&type_decls, &program.declared_keys);
    let caps = CapTable {
        gated: program.gated.clone(),
        patterns: program
            .caps
            .iter()
            .map(|g| {
                (
                    (g.module.clone(), g.domain.clone()),
                    g.patterns.clone(),
                )
            })
            .collect(),
    };
    // The capability check and `path.normalize` both reach into `std/rt` now:
    // the check for its matcher and normalizer, `path.normalize` for its
    // normalizer. Either one means the module's functions must be declared
    // before the C text that calls them, and the check's glue is emitted only
    // when some call is actually gated.
    let path_loaded = program.functions.iter().any(|f| f.module == "std/path");
    let cap_needed = !program.gated.is_empty();
    let mut out = String::new();
    out.push_str(C_HEADERS);
    out.push_str(&extern_header_includes(&program.extern_headers));
    out.push_str(RUNTIME);
    out.push('\n');
    if path_loaded || cap_needed {
        // The runtime's own half is Kelp (`std/rt`, MK7 §8), defined with the
        // rest of the program's functions, further down; the C text below calls
        // it, so the declarations come first. They are generated rather than
        // written out so that a name cannot drift from the definition: this and
        // the definition are the same `c_name` of the same function key.
        //
        // The module has to be *in* the program, or the calls below are
        // undefined symbols at link time. That is guaranteed by the resolver,
        // which injects `std/rt` for every module that holds a capability domain
        // and for `std/path` — the same conditions that made either flag true —
        // so an absence here is a compiler bug and is reported as one rather
        // than discovered by `ld` with nothing pointing at the cause.
        let prototypes = runtime_prototypes(program);
        assert!(
            !prototypes.is_empty(),
            "`path.normalize` or a gated call is in this program but no `{}` \
             function is: the resolver must inject that module (see `resolve.rs`)",
            crate::stdlib::RT_MODULE
        );
        out.push_str(&prototypes);
        out.push('\n');
    }
    if cap_needed {
        out.push_str(CAP_GLUE);
        out.push('\n');
    }

    // Each module's granted patterns, as constant data. No API can widen
    // these at runtime, and no stdlib function accepts a capability as a
    // value, so a grant cannot be smuggled through an argument. An empty
    // pattern list is emitted as `{ NULL }` rather than skipped: a module can
    // hold a domain with no patterns (that is how `process.args` and
    // `process.exit` are reachable without granting any program), and a call
    // that *does* name a resource still has to have an array to deny against
    // — skipping it left the generated C referring to a symbol that was never
    // defined.
    if cap_needed {
        out.push_str("// capability allowlists, compiled in per module\n");
        for g in &program.caps {
            writeln!(
                out,
                "static const char* {}[] = {{ {}NULL }};",
                CapTable::symbol(&g.module, &g.domain),
                g.patterns
                    .iter()
                    .map(|p| format!("{}, ", c_string_literal(p)))
                    .collect::<String>()
            )
            .unwrap();
        }
        out.push('\n');
    }

    // Container instantiations (`List<T>`, `Box<T>`): one C typedef plus one
    // function set per element type actually used, and the order is what
    // makes a recursive `datac` work (M8).
    //
    // 1. every `datac`'s struct tag, forward-declared;
    // 2. every container typedef, outermost last — a container holds its
    //    contents through a pointer, so it is complete before any `datac`
    //    that uses it, including a `datac` whose own body comes later
    //    (`List<Tree>` inside `Tree`, or a mutually recursive pair);
    // 3. the `datac`s themselves, each with its drop function;
    // 4. the container function sets, innermost first — their signatures need
    //    the element's complete type, and a container drop calls the element's
    //    drop function, which step 3 has already defined.
    //
    // One prototype per container drop comes before step 3: a `datac` that
    // owns a container drops it, and that function is defined in step 4.
    let mut collection = Instantiations::default();
    collect_containers(program, &mut collection);
    let Instantiations {
        lists: list_elems,
        boxes: box_elems,
        maps: map_values,
        closures: closure_tys,
        coerced: coerced_tys,
        shims: shim_tys,
        arrays: array_tys,
        atomics: atomic_tys,
        spawns: spawn_tys,
    } = collection;
    // An array is emitted after the types it holds by value, so it is sorted
    // innermost-first like a container, and the ones a `datac` field needs are
    // emitted immediately before that `datac` (step 3 below).
    let mut arrays: Vec<Ty> = array_tys.iter().cloned().collect();
    arrays.sort_by_key(|t| (container_depth(t), container_name(t)));
    let mut containers: Vec<Ty> = list_elems
        .iter()
        .map(|e| Ty::List(Box::new(e.clone())))
        .chain(box_elems.iter().map(|e| Ty::Box(Box::new(e.clone()))))
        .chain(
            map_values
                .iter()
                .map(|(k, v)| Ty::Map(Box::new(k.clone()), Box::new(v.clone()))),
        )
        .chain(closure_tys.iter().cloned())
        .collect();
    containers.sort_by_key(|t| (container_depth(t), container_name(t)));
    // An `Atomic<T>` is one member of a scalar type (MK9-S3-M1 stage A1), so it
    // names no `datac` and needs no forward declaration, and it is emitted here
    // before every container and `datac` that may name one: a closure signature
    // whose parameter or return is `&Atomic<i64>` spells the atomic's C type in
    // its typedef, and an unknown type name there is the C compiler's implicit
    // `int`. A `HashSet` has no order, so the sort is only so two runs emit the
    // same text.
    // An `extern type` handle is a `void*` behind its own typedef (gap 1).
    // The typedefs come before every container and every `datac`, because a
    // `List<Widget>` typedef or a `datac` field names the handle's C type and
    // an unknown type name there is the C compiler's implicit `int`.
    if !program.handles.is_empty() {
        for name in &program.handles {
            writeln!(out, "typedef void* {};", crate::types::handle_c_type(name)).unwrap();
        }
        out.push('\n');
    }
    if !atomic_tys.is_empty() {
        let mut atomics: Vec<&Ty> = atomic_tys.iter().collect();
        atomics.sort_by_key(|t| crate::types::ty_mangle(t));
        for ty in &atomics {
            write_atomic_typedef(&mut out, ty);
        }
        out.push('\n');
    }
    if !containers.is_empty() || !arrays.is_empty() {
        write_tags(&mut out, &program.structs, &program.enums);
        // Every array's tag is typedef'd here, before anything can name it: a
        // `Box<[u8; 4]>` typedef written just below holds a *pointer* to one,
        // and a pointer to an incomplete type is legal C. The struct's body
        // lands later, right before the first type that holds it by value.
        for ty in &arrays {
            let name = container_name(ty);
            writeln!(out, "typedef struct {name} {name};").unwrap();
        }
        out.push('\n');
        for ty in &containers {
            write_container_typedef(&mut out, ty);
        }
        out.push('\n');
        // The widening's own C, right after the closure typedefs it names and
        // before the `datac`s: its environment holds a function pointer, so it
        // needs no complete `datac` either (A-4 phase E). The thunk itself is
        // emitted after the `datac`s, because it returns a value by type.
        if !coerced_tys.is_empty() {
            let mut coerced: Vec<&Ty> = coerced_tys.iter().collect();
            coerced.sort_by_key(|t| container_name(t));
            for ty in coerced {
                write_coerce_env(&mut out, ty);
            }
        }
        for ty in &containers {
            writeln!(
                out,
                "static void {}({}* v);",
                drop_fn_name(ty),
                container_name(ty)
            )
            .unwrap();
        }
        out.push('\n');
    }

    // A-1: `datac`s and `enum`s are emitted in one merged declaration order,
    // because a field or payload may only name an earlier type by value — so
    // this order is what makes every typedef complete at the point it is
    // used, exactly as the single-kind declaration order did before.
    let mut decls: Vec<(usize, Decl)> = program
        .structs
        .iter()
        .map(|s| (s.order, Decl::Record(s)))
        .chain(program.enums.iter().map(|e| (e.order, Decl::Sum(e))))
        .collect();
    decls.sort_by_key(|(order, _)| *order);
    let mut emitted_arrays: HashSet<Ty> = HashSet::new();
    for (_, decl) in &decls {
        // A field or payload that holds an array holds it *by value*, so the
        // array's C struct has to be complete before this declaration is
        // written. Emitting it here (rather than with the pointer containers
        // above) is what keeps `val entries: [Entry; 512]` a field a `datac`
        // can actually have.
        for ty in decl.field_tys() {
            emit_arrays_for(&mut out, ty, &mut emitted_arrays, &owned);
        }
        match decl {
            Decl::Record(s) => write_struct(&mut out, s, &owned, &clonable, &sortable, &keyable),
            Decl::Sum(e) => write_enum(&mut out, e, &owned, &clonable, &sortable, &keyable),
        }
    }
    // A key type that declares its own pair (A-12) is reached from the `Map`
    // function sets below, and those are written before the program's own
    // functions: prototype the pair beside the typedefs, which is exactly
    // where a derived pair's definition sits for a key that has one.
    let mut key_pairs: Vec<&Function> = program
        .functions
        .iter()
        .filter(|f| {
            let Some((owner, method)) = f.name.split_once('#') else {
                return false;
            };
            program.declared_keys.contains(owner)
                && (method == crate::types::KEY_PAIR_HASH_METHOD
                    || method == crate::types::KEY_PAIR_EQ_METHOD)
        })
        .collect();
    key_pairs.sort_by(|a, b| a.name.cmp(&b.name));
    if !key_pairs.is_empty() {
        for f in key_pairs {
            write_signature(&mut out, f);
            out.push_str(";\n");
        }
        out.push('\n');
    }
    if !decls.is_empty() {
        out.push('\n');
    }

    // Every map entry body is written here, after every `datac` typedef: the
    // entry holds its key and value inline, so a `datac` in either position
    // needs the typedef that makes it a complete C type. Everything that reads
    // the entry is a map function set, and those all come later.
    for ty in &containers {
        if let Ty::Map(key, value) = ty {
            write_map_entry_body(&mut out, key, value);
        }
    }

    // Every array the program uses that no `datac` field pulled in earlier: a
    // local, a parameter, a return value, a `Box` element. They go here, after
    // every by-value declaration and before the container function sets below,
    // because those sets size and drop the types their elements are.
    for ty in &arrays.clone() {
        emit_arrays_for(&mut out, ty, &mut emitted_arrays, &owned);
    }
    if !emitted_arrays.is_empty() {
        out.push('\n');
    }

    for ty in &containers {
        match ty {
            Ty::List(elem) => write_list(&mut out, elem, &owned, &clonable, &sortable),
            Ty::Box(elem) => write_box(&mut out, elem, &owned),
            Ty::Map(key, value) => {
                write_map(&mut out, key, value, &owned, &program.declared_keys, &clonable)
            }
            // A closure's shared function set is one drop: the `fn` member is
            // whatever generated (or converted) function the value holds, and
            // the environment is dropped through the pointer stored beside
            // it, so nothing else here depends on the signature.
            Ty::Closure { .. } => write_closure(&mut out, ty),
            other => unreachable!("not a container: {}", other.display()),
        }
    }

    // The widening's thunks: each returns the wrapped function's value, so it
    // comes after every `datac` and container typedef (A-4 phase E).
    if !coerced_tys.is_empty() {
        let mut coerced: Vec<&Ty> = coerced_tys.iter().collect();
        coerced.sort_by_key(|t| container_name(t));
        for ty in coerced {
            write_coerce_thunk(&mut out, ty);
        }
        out.push('\n');
    }

    // The native-callback glue (gap 4): the reordering thunk and the
    // destructor every user `extern fun`'s `move closure(...)` parameter
    // expands into. One definition per closure signature, so two externs that
    // share a signature share the pair.
    let callback_sigs = callback_signatures(program);
    if !callback_sigs.is_empty() {
        for sig in &callback_sigs {
            write_callback_glue(&mut out, sig);
        }
        out.push('\n');
    }

    // Each `fun` literal that captured something has an environment struct and
    // a drop function (A-4 phase E). They go here — after every typedef, since
    // a field holds its captured value by value, and before every function, so
    // a literal's body and its drop exist before anything names them.
    for f in &program.functions {
        if f.env.is_some() {
            write_env(&mut out, f, &owned);
        }
    }

    // Prototypes come first: a `pd` initializer may call a function
    // (`pd var r = buf_reader(...)`), and `write_pd_globals` emits
    // `kelp_init_pds` — so the declarations have to be in scope before that
    // definition, not after it.
    for f in &program.functions {
        if f.name == "main" {
            continue; // defined (not just declared) below, with its special `int main(...)` signature
        }
        write_signature(&mut out, f);
        out.push_str(";\n");
    }
    out.push('\n');

    // The shims for widened values whose function is statically known: each
    // calls the function it wraps, so it needs the prototypes above and
    // nothing else.
    if !shim_tys.is_empty() {
        let mut shims: Vec<&(String, Ty)> = shim_tys.iter().collect();
        shims.sort_by_key(|(key, ty)| (container_name(ty), key.clone()));
        for (key, ty) in shims {
            write_shim(&mut out, key, ty);
        }
        out.push('\n');
    }

    // The worker bodies `thread.spawn` lowers to (MK9-S3-M1 stage C1): each
    // calls the closure it was handed through a code pointer and stores the
    // result where the handle expects it, so it needs the closure typedef and
    // nothing else.
    if !spawn_tys.is_empty() {
        let mut spawns: Vec<&Ty> = spawn_tys.iter().collect();
        spawns.sort_by_key(|ty| container_name(ty));
        for ty in spawns {
            write_thread_body(&mut out, ty);
        }
        out.push('\n');
    }

    write_pd_globals(
        &mut out,
        &program.pds,
        &program.enums,
        drops,
        &owned,
        &caps,
        sources,
    );

    for f in &program.functions {
        if f.extern_ {
            write_extern(&mut out, f, &program.extern_ffi_symbols, false);
            out.push('\n');
            continue;
        }
        let mut g = Gen::new(
            drops,
            &owned,
            &program.enums,
            f.name.clone(),
            f.module.clone(),
            &caps,
            sources,
        );
        g.write_function(f, !program.pds.is_empty());
        out.push_str(&g.out);
        out.push('\n');
    }

    out
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
fn write_user_extern(out: &mut String, f: &Function, symbol: &str, closure_by_ref: bool) {
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

/// Emits one file-scope global per passdown plus the generated function that
/// initializes them, in declaration order (M5.2). The global's name is the
/// same `pd#...` name a referencing function takes as a hidden parameter, so
/// inside such a function the parameter shadows its own global and both
/// spell the same symbol in C — including inside `main`, which can take no
/// parameters and therefore reads the globals directly.
///
/// A `pd` initializer *is* code that runs, so a gated call inside one is
/// checked against the *declaring* module's grant, exactly as the same call
/// would be inside a function of that module: `Pd::module` is what carries it
/// (the initializer is not a function, so there is no other place the module
/// could come from), and one `Gen` per initializer is what makes it the
/// module the check names and the allowlist it scans.
fn write_pd_globals(
    out: &mut String,
    pds: &[Pd],
    enums: &[EnumDef],
    drops: &DropPlan,
    owned: &HashSet<String>,
    caps: &CapTable,
    sources: &SourceMap,
) {
    if pds.is_empty() {
        return;
    }
    // A `pd` initializer is not a function, so no drop point can be keyed to
    // it; an owned local inside one would leak. `infer.rs` rejects owned
    // locals in initializers, so this only affects an initializer that
    // happens to build and discard one. The `function` name stays empty for
    // the same reason: the drop plan has no entry keyed to a `pd`.
    out.push_str("// passdowns: assigned once by kelp_init_pds(), read-only afterwards\n");
    for pd in pds {
        writeln!(
            out,
            "static {};",
            pd.value_ty.c_decl(&c_name(&pd.name))
        )
        .unwrap();
    }
    out.push('\n');
    // An owned passdown (M6.2) has no scope to die at, so its drop point is
    // process exit: an `atexit` handler registered by the initializer. One
    // handler for all of them, freeing in declaration order.
    let droppable: Vec<&Pd> = pds
        .iter()
        .filter(|pd| needs_drop(&pd.value_ty, owned))
        .collect();
    if !droppable.is_empty() {
        out.push_str("static void kelp_drop_pds(void) {\n");
        for pd in &droppable {
            writeln!(
                out,
                "    {}(&{});",
                drop_fn_name(&pd.value_ty),
                c_name(&pd.name)
            )
            .unwrap();
        }
        out.push_str("}\n\n");
    }
    out.push_str("static void kelp_init_pds(void) {\n");
    if !droppable.is_empty() {
        out.push_str("    atexit(kelp_drop_pds);\n");
    }
    for pd in pds {
        let g = Gen::new(
            drops,
            owned,
            enums,
            String::new(),
            pd.module.clone(),
            caps,
            sources,
        );
        writeln!(
            out,
            "    {} = {}; // pd {}",
            c_name(&pd.name),
            g.expr_value(&pd.init),
            pd.qualified
        )
        .unwrap();
    }
    out.push_str("}\n\n");
}

/// One `datac`'s C typedef, plus its drop function when it owns heap memory
/// The `(name, each field's type)` pairs of every declaration, for the derived
/// clone/order fixpoints. A `datac` name and an `enum` name cannot collide, so
/// an `enum` is listed once, by its payloads.
fn type_decls(structs: &[StructDef], enums: &[EnumDef]) -> Vec<(String, Vec<Ty>)> {
    let enum_names: HashSet<&str> = enums.iter().map(|e| e.name.as_str()).collect();
    let mut out: Vec<(String, Vec<Ty>)> = Vec::new();
    for s in structs {
        if enum_names.contains(s.name.as_str()) {
            continue;
        }
        out.push((s.name.clone(), s.fields.iter().map(|f| f.ty.clone()).collect()));
    }
    for e in enums {
        out.push((
            e.name.clone(),
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
        ));
    }
    out
}

/// One field's deep copy, as one C statement. `dst`/`src` are whole field
/// expressions (`dst->kelp_text`), so the clone of an owned field is the
/// runtime's own clone of it, and a nested `datac`/`enum` recurses into its
/// generated clone.
fn write_field_clone(out: &mut String, ty: &Ty, dst: &str, src: &str) {
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
fn write_field_cmp(out: &mut String, ty: &Ty, a: &str, b: &str) {
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
fn write_field_hash(out: &mut String, ty: &Ty, at: &str) {
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
fn write_field_eq(out: &mut String, ty: &Ty, a: &str, b: &str) {
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
fn write_struct(
    out: &mut String,
    s: &StructDef,
    owned: &HashSet<String>,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
    keyable: &HashSet<String>,
) {
    // Tagged (M8), because a container typedef emitted *earlier* may hold a
    // pointer to this struct (`Box<Node>` inside `Node` itself), and a
    // forward declaration of the tag is what makes that legal C.
    write!(out, "typedef struct {} {{\n", crate::types::struct_tag(&s.name)).unwrap();
    for f in &s.fields {
        write!(out, "    {};\n", f.ty.c_decl(&field_name(&f.name))).unwrap();
    }
    let ty = Ty::Struct(s.name.clone());
    write!(out, "}} {};\n", ty.c_type()).unwrap();
    if needs_drop(&ty, owned) {
        writeln!(
            out,
            "static void kelp_drop_{}({}* v) {{",
            crate::types::c_ident(&s.name),
            ty.c_type()
        )
        .unwrap();
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
fn variant_member(name: &str) -> String {
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
fn write_enum(
    out: &mut String,
    e: &EnumDef,
    owned: &HashSet<String>,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
    keyable: &HashSet<String>,
) {
    write!(out, "typedef struct {} {{\n", crate::types::struct_tag(&e.name)).unwrap();
    out.push_str("    int32_t kelp_tag;\n");
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

    if needs_drop(&ty, owned) {
        writeln!(
            out,
            "static void kelp_drop_{}({}* v) {{",
            crate::types::c_ident(&e.name),
            ty.c_type()
        )
        .unwrap();
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
fn write_map_entry_body(out: &mut String, key: &Ty, value: &Ty) {
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
fn write_container_typedef(out: &mut String, ty: &Ty) {
    match ty {
        Ty::Closure { params, ret } => {
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
fn write_closure_typedef(out: &mut String, params: &[Ty], ret: &Ty) {
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

/// The generated names for the widening of one closure signature (A-4 phase
/// E): the environment that holds a plain function pointer, the function that
/// frees it, and the thunk that calls through it. One thunk per *signature*,
/// not per function, because what it does is the same for every `fun` value of
/// that shape.
fn coerce_env_c_type(ty: &Ty) -> String {
    format!("kelp_CoerceEnv__{}", coerce_mangle(ty))
}

fn coerce_drop_name(ty: &Ty) -> String {
    format!("kelp_coerce_drop__{}", coerce_mangle(ty))
}

fn coerce_thunk_name(ty: &Ty) -> String {
    format!("kelp_coerce__{}", coerce_mangle(ty))
}

fn coerce_mangle(ty: &Ty) -> String {
    match ty {
        Ty::Closure { params, ret } => crate::types::closure_mangle(params, ret),
        other => unreachable!("not a closure: {}", other.display()),
    }
}

/// The machinery behind `tir::ExprKind::FnToClosure` (A-4 phase E): a plain
/// function value has no environment, so widening it into a closure means
/// giving it one — a one-field heap struct holding the code pointer — and
/// calling through a generated thunk. Emitted with the closure typedefs,
/// because it is C shape rather than data: the thunk takes the environment
/// first, exactly as a literal's generated function does, so the resulting
/// value is a closure like any other and an indirect call needs no special
/// case for it.
fn write_coerce_env(out: &mut String, ty: &Ty) {
    let Ty::Closure { params, ret } = ty else {
        unreachable!("not a closure: {}", ty.display())
    };
    let env = coerce_env_c_type(ty);
    let param_types = params
        .iter()
        .map(crate::types::container_elem_c_type)
        .collect::<Vec<_>>();
    let names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
    writeln!(out, "// ---- {env} (A-4 phase E) {}", "-".repeat(26)).unwrap();
    writeln!(
        out,
        "typedef struct {{\n    {} (*f)({});\n}} {env};",
        crate::types::container_elem_c_type(ret),
        if param_types.is_empty() { "void".to_string() } else { param_types.join(", ") }
    )
    .unwrap();
}

/// The thunk and the environment's destructor for one closed-over signature
/// (A-4 phase E). Split from `write_coerce_env` because a function *definition*
/// needs its return type complete: the environment typedef only mentions it in
/// a pointer, which a forward-declared `datac` tag satisfies, while the thunk
/// returns it by value and so comes after the `datac` typedefs.
fn write_coerce_thunk(out: &mut String, ty: &Ty) {
    let Ty::Closure { params, ret } = ty else {
        unreachable!("not a closure: {}", ty.display())
    };
    let env = coerce_env_c_type(ty);
    let param_types = params
        .iter()
        .map(crate::types::container_elem_c_type)
        .collect::<Vec<_>>();
    let names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
    // The environment holds a function pointer, which owns nothing, so the
    // destructor is just the `free`.
    writeln!(
        out,
        "static void {}(void* raw) {{\n    free(raw);\n}}\n",
        coerce_drop_name(ty)
    )
    .unwrap();
    let decls = if param_types.is_empty() {
        "void* raw".to_string()
    } else {
        let rest = param_types
            .iter()
            .zip(&names)
            .map(|(p, n)| format!("{p} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("void* raw, {rest}")
    };
    writeln!(
        out,
        "static {} {}({decls}) {{",
        ret.c_type(),
        coerce_thunk_name(ty)
    )
    .unwrap();
    writeln!(out, "    {env}* e = ({env}*)raw;").unwrap();
    let call = format!("(e->f)({})", names.join(", "));
    if **ret == Ty::Unit {
        writeln!(out, "    {call};").unwrap();
    } else {
        writeln!(out, "    return {call};").unwrap();
    }
    out.push_str("}\n\n");
}

/// The generated `fn` member of a widened *known* function value (A-4 phase
/// E): a shim that takes the environment a closure's `fn` member is called
/// with and ignores it, then calls the function it wraps with the arguments.
/// So the common widening — `list.map(xs, double)`, `Holder(cb = fun(x: i32)
/// .R: i32 = x)` — costs no allocation and no indirection a `fun` value would
/// not have had, and only a widening of a function value whose identity is not
/// statically known needs the heap wrapper.
fn write_shim(out: &mut String, key: &str, ty: &Ty) {
    let Ty::Closure { params, ret } = ty else {
        unreachable!("not a closure: {}", ty.display())
    };
    let param_types = params
        .iter()
        .map(crate::types::container_elem_c_type)
        .collect::<Vec<_>>();
    let names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
    let decls = if param_types.is_empty() {
        "void* env_raw".to_string()
    } else {
        let rest = param_types
            .iter()
            .zip(&names)
            .map(|(p, n)| format!("{p} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("void* env_raw, {rest}")
    };
    writeln!(
        out,
        "static {} {}({decls}) {{",
        ret.c_type(),
        coerce_shim_name(key, ty)
    )
    .unwrap();
    out.push_str("    (void)env_raw;\n");
    let call = format!("{}({})", c_name(key), names.join(", "));
    if **ret == Ty::Unit {
        writeln!(out, "    {call};").unwrap();
    } else {
        writeln!(out, "    return {call};").unwrap();
    }
    out.push_str("}\n");
}

fn coerce_shim_name(key: &str, ty: &Ty) -> String {
    format!(
        "kelp_shim__{}__{}",
        crate::types::c_ident(key),
        coerce_mangle(ty)
    )
}

/// The generated C name of one spawn signature's worker body (MK9-S3-M1 stage
/// C1). One per closure signature, spelled from the signature's own mangle so
/// the LLVM backend's IR body of the same program carries the same name.
fn thread_body_name(sig: &Ty) -> String {
    let Ty::Closure { params, ret } = sig else {
        unreachable!("a thread body is emitted per closure signature: {}", sig.display())
    };
    format!(
        "kelp_thread_body__{}",
        crate::types::closure_mangle(params, ret)
    )
}

/// One spawn signature's worker body (MK9-S3-M1 stage C1): what the runtime's
/// trampoline calls inside the new thread. Its parameters are pointers, so the
/// shape is the same for every element type and the two backends agree on it:
/// the closure's code pointer, its environment, the address of the moved
/// argument, and the address of the handle's result slot. A body whose closure
/// returns `unit` stores nothing, because there is no slot for a result that is
/// not a value.
fn write_thread_body(out: &mut String, sig: &Ty) {
    let Ty::Closure { params, ret } = sig else {
        unreachable!("a thread body is emitted per closure signature: {}", sig.display())
    };
    let arg_c = params[0].c_type();
    let call = if **ret == Ty::Unit {
        format!("((void (*)(void*, {arg_c}))code)(env, *({arg_c}*)arg);")
    } else {
        format!(
            "*({ret_c}*)out = (({ret_c} (*)(void*, {arg_c}))code)(env, *({arg_c}*)arg);",
            ret_c = ret.c_type()
        )
    };
    writeln!(
        out,
        "static void {}(void* code, void* env, void* arg, void* out) {{\n    {call}\n}}\n",
        thread_body_name(sig)
    )
    .unwrap();
}

/// One closure instantiation's function set — which is one function: the
/// drop. Everything else about a closure is the generated literal's or the
/// caller's, and both are emitted elsewhere.
fn write_closure(out: &mut String, ty: &Ty) {
    let Ty::Closure { .. } = ty else {
        unreachable!("not a closure: {}", ty.display())
    };
    let name = ty.c_type();
    let drop = drop_fn_name(ty);
    writeln!(out, "// ---- {name} (A-4) {}", "-".repeat(34)).unwrap();
    // The environment is `NULL` exactly when the closure was made from a
    // plain function value, and then there is nothing to free: the `env` test
    // comes first because such a value's destructor pointer is `NULL` too.
    writeln!(
        out,
        "static void {drop}({name}* c) {{\n    if (c->env != NULL) {{\n        c->drop(c->env);\n        c->env = NULL;\n    }}\n}}\n"
    )
    .unwrap();
}

/// The generated C name of one closure signature's native-callback thunk
/// (gap 4). The thunk is the `func` half of the `(func, user_data,
/// destroy_notify)` triple a user `extern fun`'s `move closure(...)` parameter
/// expands into. One per signature, not per call site, because what it does is
/// the same for every closure of that shape.
fn callback_thunk_name(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_cb_thunk__{}", crate::types::closure_mangle(params, ret))
}

/// The generated C name of one closure signature's callback destructor
/// (gap 4): the `destroy_notify` half of the triple, which runs the closure's
/// drop on its environment and frees the box that carried it.
fn callback_destroy_name(params: &[Ty], ret: &Ty) -> String {
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
fn write_tags<'a, S, E>(out: &mut String, structs: S, enums: E)
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
fn write_atomic_typedef(out: &mut String, ty: &Ty) {
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
        for ty in &containers {
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
                write_struct(out, s, &no_derives, &no_derives, &no_derives, &no_derives)
            }
            Decl::Sum(e) => write_enum(out, e, &no_derives, &no_derives, &no_derives, &no_derives),
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
fn collect_layout_tys(
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
        Ty::Closure { params, ret } => {
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
fn container_name(ty: &Ty) -> String {
    match ty {
        Ty::List(elem) => crate::types::list_c_type(elem),
        Ty::Box(elem) => crate::types::box_c_type(elem),
        Ty::Map(key, value) => crate::types::map_c_type(key, value),
        Ty::Closure { .. } => ty.c_type(),
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
fn write_array(out: &mut String, elem: &Ty, len: u64, owned: &HashSet<String>) {
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
fn emit_arrays_for(
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
fn container_depth(ty: &Ty) -> usize {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => 1 + container_depth(elem),
        // A `Map<V>` value is never a container (`is_map_element` refuses
        // one), so a map is always one level deep.
        Ty::Map(..) => 1,
        Ty::Closure { params, ret } => {
            1 + params
                .iter()
                .map(container_depth)
                .chain(std::iter::once(container_depth(ret)))
                .max()
                .unwrap_or(0)
        }
        // A nested array is held inline, so it has to be complete before the
        // array that contains it: `[[u8; 4]; 8]` emits the inner one first.
        Ty::Array(elem, _) => 1 + container_depth(elem),
        _ => 0,
    }
}

/// One `Box<T>` instantiation's function set (M8). The box holds a pointer
/// to one value or `NULL`; `take` moves the value out and is the reason a box
/// can be empty at all (`Box.empty()`), which is what makes a recursive
/// `datac` — and a loop that walks one — expressible without `Option`.
fn write_box(out: &mut String, elem: &Ty, owned: &HashSet<String>) {
    let boxed = crate::types::box_c_type(elem);
    let boxed_ty = Ty::Box(Box::new(elem.clone()));
    let elem_c = elem.c_type();
    let elem_drops = needs_drop(elem, owned);
    let drop_elem = elem_drops.then(|| drop_fn_name(elem));
    writeln!(out, "// ---- {} (M8) {}", boxed, "-".repeat(34)).unwrap();

    writeln!(out, "static {boxed} {boxed}__empty(void) {{\n    {boxed} b;\n    b.value = NULL;\n    return b;\n}}\n").unwrap();

    writeln!(out, "static {boxed} {boxed}__of({elem_c} v) {{").unwrap();
    writeln!(out, "    {elem_c}* p = ({elem_c}*)malloc(sizeof({elem_c}));").unwrap();
    out.push_str("    if (p == NULL) {\n        kelp_panic(\"out of memory\");\n    }\n    *p = v;\n");
    writeln!(out, "    {boxed} b;\n    b.value = p;\n    return b;\n}}\n").unwrap();

    writeln!(out, "static bool {boxed}__is_empty(const {boxed}* self) {{\n    return self->value == NULL;\n}}\n").unwrap();

    if elem.is_copy() {
        writeln!(out, "static {elem_c} {boxed}__get(const {boxed}* self) {{").unwrap();
        writeln!(out, "    if (self->value == NULL) {{\n        kelp_panic(\"Box.get: the box is empty\");\n    }}\n    return *self->value;\n}}\n").unwrap();
    } else {
        writeln!(out, "static const {elem_c}* {boxed}__get_const(const {boxed}* self) {{").unwrap();
        writeln!(out, "    if (self->value == NULL) {{\n        kelp_panic(\"Box.get: the box is empty\");\n    }}\n    return self->value;\n}}\n").unwrap();
    }

    writeln!(out, "static void {boxed}__set({boxed}* self, {elem_c} v) {{").unwrap();
    out.push_str("    if (self->value == NULL) {\n");
    writeln!(out, "        self->value = ({elem_c}*)malloc(sizeof({elem_c}));").unwrap();
    out.push_str("        if (self->value == NULL) {\n            kelp_panic(\"out of memory\");\n        }\n    } else {\n");
    if let Some(d) = &drop_elem {
        // The slot is reused: the value it held is freed, the box is not.
        writeln!(out, "        {d}(self->value);").unwrap();
    }
    out.push_str("    }\n    *self->value = v;\n}\n\n");

    writeln!(out, "static {elem_c} {boxed}__take({boxed}* self) {{").unwrap();
    writeln!(out, "    if (self->value == NULL) {{\n        kelp_panic(\"Box.take: the box is empty\");\n    }}\n    {elem_c} v = *self->value;\n    free(self->value);\n    self->value = NULL;\n    return v;\n}}\n").unwrap();

    writeln!(out, "static void {}({boxed}* self) {{", drop_fn_name(&boxed_ty)).unwrap();
    out.push_str("    if (self->value != NULL) {\n");
    if let Some(d) = &drop_elem {
        writeln!(out, "        {d}(self->value);").unwrap();
    }
    out.push_str("        free(self->value);\n        self->value = NULL;\n    }\n}\n\n");
}

/// The C function that hashes a `datac`/`enum` key: the key type's own declared
/// `hash` method when it declares a key pair (A-12), else the derived
/// `kelp_hash_<T>`. Exactly one of the two is emitted for a key type, because a
/// declared pair is used *instead of* the derived one.
fn key_hash_target(key: &Ty, declared_keys: &HashSet<String>) -> String {
    match key {
        Ty::Struct(name) if declared_keys.contains(name) => c_name(
            &crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_HASH_METHOD),
        ),
        _ => hash_fn_name(key),
    }
}

/// As `key_hash_target`, for the key's equality.
fn key_eq_target(key: &Ty, declared_keys: &HashSet<String>) -> String {
    match key {
        Ty::Struct(name) if declared_keys.contains(name) => c_name(
            &crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_EQ_METHOD),
        ),
        _ => eq_fn_name(key),
    }
}

/// One `Map<K, V>` instantiation's function set (MK5-S5). Entries live in
/// insertion order in a dense array, so `keys()` returns insertion order
/// rather than hash order; a separate open-addressed index of `entry index +
/// 1` (`0` empty, `UINT32_MAX` tombstone) does the lookup. `clear()` resets
/// both and keeps the buffers; a removal marks the entry dead and tombstones
/// its index slot rather than shifting anything, so insertion order is
/// stable. The value is held through a pointer, which is what lets this
/// typedef be emitted before every `datac` (a `datac` value is named by its
/// forward-declared struct tag).
///
/// The key is whatever `Map<K, V>` named, and it is the one thing that
/// changes the generated code from instantiation to instantiation: a `String`
/// key is hashed over its bytes, copied in from the `str` view the caller
/// passes, and compared with a length-aware `memcmp`; a scalar key is hashed
/// by mixing its bits, passed by value, and compared with `==` (A-2); a
/// `datac`/`enum` key (A-6) is passed as a shared borrow, hashed and compared
/// by its generated `kelp_hash_<T>`/`kelp_eq_<T>`, and cloned in with its
/// generated `kelp_clone_<T>` so the entry owns its own copy. A key type that
/// declares its own pair (A-12) instead reaches its own two methods, which the
/// query and insert paths name through `key_hash_target`/`key_eq_target`.
fn write_map(
    out: &mut String,
    key: &Ty,
    value: &Ty,
    owned: &HashSet<String>,
    declared_keys: &HashSet<String>,
    clonable_types: &HashSet<String>,
) {
    let map = crate::types::map_c_type(key, value);
    let map_ty = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
    let entry = format!("{map}_entry");
    let value_c = value.c_type();
    let value_drops = needs_drop(value, owned);
    let drop_value = value_drops.then(|| drop_fn_name(value));
    let list_key = crate::types::list_c_type(key);
    let list_value = crate::types::list_c_type(value);
    // The same declaration-graph deep-copy predicate `List.slice` is gated on
    // (`infer::map_sig` asks it with the same set), so `get_or`/`values` exist
    // exactly when this is true.
    let clonable = crate::types::value_is_clonable(value, clonable_types);
    // Reading a value *out* of the map is a copy for a `Copy` value, a clone
    // for `String`/`Bytes`, and the derived `kelp_clone_<T>` into the
    // destination for an owned `datac`/`enum` (a shallow `*ptr` would leave
    // two owners of the value's own fields, MISSING-qM4Zx).
    let copy_from = |dst: &str, ptr: &str| match value {
        Ty::String => format!("{dst} = kelp_String__clone({ptr});"),
        Ty::Bytes => format!("{dst} = kelp_Bytes__clone({ptr});"),
        Ty::Struct(_) => format!("{}({ptr}, &{dst});", clone_fn_name(value)),
        _ => format!("{dst} = *{ptr};"),
    };
    // A `String` key reaches the map as a `str` view (the map copies it in);
    // a scalar key is already the value the map stores; a user key reaches it
    // as a shared borrow (the map clones its own copy in).
    let key_is_string = key == &Ty::String;
    let key_is_struct = matches!(key, Ty::Struct(_));
    let key_drops = key_is_struct && needs_drop(key, owned);
    let key_arg_c = if key_is_string {
        "const char*".to_string()
    } else if key_is_struct {
        format!("const {}*", key.c_type())
    } else {
        key.c_type()
    };
    // Hashing a key *argument* (a `str` view or a borrowed `datac`) and
    // hashing a *stored* key (`kelp_String` or an inline `datac`) read the
    // bytes differently, so they are two helpers.
    let hash_arg = |a: &str| {
        if key_is_string {
            format!("kelp_map_hash({a}, strlen({a}))")
        } else if key_is_struct {
            format!("{}({a})", key_hash_target(key, declared_keys))
        } else {
            format!("kelp_map_mix((uint64_t)({a}))")
        }
    };
    let hash_stored = |s: &str| {
        if key_is_string {
            format!("kelp_map_hash((const char*){s}.data, {s}.len)")
        } else if key_is_struct {
            format!("{}(&{s})", key_hash_target(key, declared_keys))
        } else {
            format!("kelp_map_mix((uint64_t)({s}))")
        }
    };
    // Each of these takes a stored key as an *lvalue*, and adds the `&` a
    // `kelp_String` or a user key needs itself; a scalar key is compared and
    // dropped by value, and taking its address would be a different
    // expression.
    let eq_stored = |stored: &str, a: &str| {
        if key_is_string {
            format!("kelp_map_key_eq(&{stored}, {a}, strlen({a}))")
        } else if key_is_struct {
            format!("{}(&{stored}, {a})", key_eq_target(key, declared_keys))
        } else {
            format!("({stored} == {a})")
        }
    };
    // A statement, terminator included: every caller writes it as one.
    let drop_key = |s: &str| {
        if key_is_string {
            format!("kelp_string_drop(&{s});")
        } else if key_drops {
            format!("{}(&{s});", drop_fn_name(key))
        } else {
            format!("(void)({s});")
        }
    };
    writeln!(out, "// ---- {} (MK5-S5) {}", map, "-".repeat(30)).unwrap();

    writeln!(out, "static {map} {map}__new(void) {{\n    {map} m;\n    m.entries = NULL;\n    m.entry_count = 0;\n    m.entry_cap = 0;\n    m.len = 0;\n    m.index = NULL;\n    m.index_cap = 0;\n    m.index_used = 0;\n    return m;\n}}\n").unwrap();

    // The index probe: returns the slot holding the key (`*found`), or the
    // slot to insert at (the first tombstone seen, else the first empty one).
    writeln!(out, "static size_t {map}__find(const {map}* self, {key_arg_c} k, bool* found) {{").unwrap();
    out.push_str("    *found = false;\n    if (self->index_cap == 0) {\n        return 0;\n    }\n");
    writeln!(out, "    size_t mask = self->index_cap - 1;\n    size_t i = (size_t){} & mask;\n    size_t tomb = SIZE_MAX;", hash_arg("k")).unwrap();
    out.push_str("    for (;;) {\n        uint32_t slot = self->index[i];\n        if (slot == 0) {\n            return tomb != SIZE_MAX ? tomb : i;\n        }\n");
    out.push_str("        if (slot == UINT32_MAX) {\n            if (tomb == SIZE_MAX) {\n                tomb = i;\n            }\n        } else {\n");
    writeln!(
        out,
        "            const {entry}* e = &self->entries[slot - 1];\n            if (e->live && {}) {{\n                *found = true;\n                return i;\n            }}\n        }}",
        eq_stored("e->key", "k")
    )
    .unwrap();
    out.push_str("        i = (i + 1) & mask;\n    }\n}\n\n");

    // A rebuild drops every tombstone, so it sizes the index from the live
    // count alone and leaves the dense entries untouched.
    writeln!(out, "static void {map}__rehash({map}* self, size_t want) {{").unwrap();
    out.push_str("    size_t cap = 8;\n    while (cap < want * 4) {\n        cap *= 2;\n    }\n");
    writeln!(out, "    free(self->index);\n    self->index = (uint32_t*)malloc(cap * sizeof(uint32_t));").unwrap();
    out.push_str("    if (self->index == NULL) {\n        kelp_panic(\"out of memory\");\n    }\n    memset(self->index, 0, cap * sizeof(uint32_t));\n");
    out.push_str("    self->index_cap = cap;\n    self->index_used = 0;\n");
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    writeln!(
        out,
        "        size_t i = (size_t){} & (cap - 1);",
        hash_stored("self->entries[e].key")
    )
    .unwrap();
    out.push_str("        while (self->index[i] != 0) {\n            i = (i + 1) & (cap - 1);\n        }\n        self->index[i] = (uint32_t)(e + 1);\n        self->index_used++;\n    }\n}\n\n");

    // `Map.with_capacity(n)`: both buffers sized up front, and never a fence:
    // a map that outgrows the hint grows like any other.
    writeln!(out, "static {map} {map}__with_capacity(size_t cap) {{\n    {map} m = {map}__new();\n    if (cap == 0) {{\n        return m;\n    }}\n    m.entries = ({entry}*)malloc(cap * sizeof({entry}));\n    if (m.entries == NULL) {{\n        kelp_panic(\"out of memory\");\n    }}\n    m.entry_cap = cap;\n    {map}__rehash(&m, cap);\n    return m;\n}}\n\n").unwrap();

    writeln!(out, "static void {map}__insert({map}* self, {key_arg_c} k, {value_c} v) {{").unwrap();
    writeln!(out, "    if (self->index_cap == 0 || (self->index_used + 1) * 4 >= self->index_cap * 3) {{\n        {map}__rehash(self, self->len + 1);\n    }}").unwrap();
    out.push_str("    bool found = false;\n");
    writeln!(out, "    size_t slot = {map}__find(self, k, &found);").unwrap();
    out.push_str("    if (found) {\n        size_t e = self->index[slot] - 1;\n");
    if value_drops {
        writeln!(out, "        {}(&self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    out.push_str("        self->entries[e].value = v;\n        return;\n    }\n");
    out.push_str("    if (self->entry_count == self->entry_cap) {\n        size_t cap = self->entry_cap == 0 ? 8 : self->entry_cap * 2;\n");
    writeln!(out, "        self->entries = ({entry}*)realloc(self->entries, cap * sizeof({entry}));").unwrap();
    out.push_str("        if (self->entries == NULL) {\n            kelp_panic(\"out of memory\");\n        }\n        self->entry_cap = cap;\n    }\n");
    out.push_str("    size_t e = self->entry_count++;\n");
    if key_is_struct {
        // The entry owns its own copy of the key, like a `String` key's.
        writeln!(out, "    {}(k, &self->entries[e].key);", clone_fn_name(key)).unwrap();
    } else if key_is_string {
        writeln!(out, "    self->entries[e].key = kelp_String__from_str(k);").unwrap();
    } else {
        writeln!(out, "    self->entries[e].key = k;").unwrap();
    }
    out.push_str("    self->entries[e].value = v;\n    self->entries[e].live = 1;\n");
    out.push_str("    if (self->index[slot] == 0) {\n        self->index_used++;\n    }\n    self->index[slot] = (uint32_t)(e + 1);\n    self->len++;\n}\n\n");

    writeln!(out, "static bool {map}__has(const {map}* self, {key_arg_c} k) {{\n    bool found = false;\n    {map}__find(self, k, &found);\n    return found;\n}}\n").unwrap();

    if value.is_copy() {
        writeln!(out, "static {value_c} {map}__get(const {map}* self, {key_arg_c} k) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.get: key not found\");\n    }}\n    return self->entries[self->index[slot] - 1].value;\n}}\n").unwrap();
    } else {
        writeln!(out, "static const {value_c}* {map}__get_const(const {map}* self, {key_arg_c} k) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.get: key not found\");\n    }}\n    return &self->entries[self->index[slot] - 1].value;\n}}\n").unwrap();
    }

    if clonable {
        writeln!(out, "static {value_c} {map}__get_or(const {map}* self, {key_arg_c} k, {value_c} fallback) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        return fallback;\n    }}").unwrap();
        writeln!(out, "    const {value_c}* existing = &self->entries[self->index[slot] - 1].value;\n    {value_c} result;").unwrap();
        writeln!(out, "    {}", copy_from("result", "existing")).unwrap();
        match value {
            Ty::String => out.push_str("    kelp_string_drop(&fallback);\n"),
            Ty::Bytes => out.push_str("    kelp_bytes_drop(&fallback);\n"),
            // The caller moved the fallback in, and this path does not hand
            // it back, so the function owns it and has to drop it.
            Ty::Struct(_) if value_drops => {
                writeln!(out, "    {}(&fallback);", drop_fn_name(value)).unwrap();
            }
            _ => out.push_str("    (void)fallback;\n"),
        }
        out.push_str("    return result;\n}\n\n");
    }

    writeln!(out, "static {value_c} {map}__remove({map}* self, {key_arg_c} k) {{").unwrap();
    writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.remove: key not found\");\n    }}\n    size_t e = self->index[slot] - 1;").unwrap();
    out.push_str("    ");
    writeln!(out, "{value_c} v = self->entries[e].value;\n    {}\n    self->entries[e].live = 0;\n    self->index[slot] = UINT32_MAX;\n    self->len--;\n    return v;\n}}\n", drop_key("self->entries[e].key")).unwrap();

    writeln!(out, "static void {map}__clear({map}* self) {{").unwrap();
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    if value_drops {
        writeln!(out, "        {}(&self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    writeln!(out, "        {}\n        self->entries[e].live = 0;\n    }}", drop_key("self->entries[e].key")).unwrap();
    out.push_str("    self->entry_count = 0;\n    self->len = 0;\n    self->index_used = 0;\n    if (self->index != NULL) {\n        memset(self->index, 0, self->index_cap * sizeof(uint32_t));\n    }\n}\n\n");

    writeln!(out, "static size_t {map}__len(const {map}* self) {{\n    return self->len;\n}}\n").unwrap();
    writeln!(out, "static bool {map}__is_empty(const {map}* self) {{\n    return self->len == 0;\n}}\n").unwrap();

    writeln!(out, "static {list_key} {map}__keys(const {map}* self) {{").unwrap();
    writeln!(out, "    {list_key} out = {list_key}__empty();\n    {list_key}__reserve(&out, self->len);").unwrap();
    let append = if key_is_struct {
        format!(
            "{}(&self->entries[e].key, &out.data[out.len]);\n            out.len++;",
            clone_fn_name(key)
        )
    } else if key_is_string {
        "out.data[out.len++] = kelp_String__clone(&self->entries[e].key);".to_string()
    } else {
        "out.data[out.len++] = self->entries[e].key;".to_string()
    };
    writeln!(
        out,
        "    for (size_t e = 0; e < self->entry_count; e++) {{\n        if (self->entries[e].live) {{\n            {append}\n        }}\n    }}\n    return out;\n}}\n"
    )
    .unwrap();

    if clonable {
        writeln!(out, "static {list_value} {map}__values(const {map}* self) {{").unwrap();
        writeln!(out, "    {list_value} out = {list_value}__empty();\n    {list_value}__reserve(&out, self->len);").unwrap();
        writeln!(out, "    for (size_t e = 0; e < self->entry_count; e++) {{\n        if (self->entries[e].live) {{\n            {}", copy_from("out.data[out.len]", "&self->entries[e].value")).unwrap();
        out.push_str("            out.len++;\n        }\n    }\n");
        writeln!(out, "    return out;\n}}\n").unwrap();
    }

    writeln!(out, "static void {}({map}* self) {{", drop_fn_name(&map_ty)).unwrap();
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    if value_drops {
        writeln!(out, "        {}(&self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    writeln!(out, "        {}\n    }}", drop_key("self->entries[e].key")).unwrap();
    out.push_str("    free(self->entries);\n    free(self->index);\n    self->entries = NULL;\n    self->entry_count = 0;\n    self->entry_cap = 0;\n    self->len = 0;\n    self->index = NULL;\n    self->index_cap = 0;\n    self->index_used = 0;\n}\n\n");
}

/// One `List<T>` instantiation's function set: a growable buffer plus the
/// operations on it.
fn write_list(
    out: &mut String,
    elem: &Ty,
    owned: &HashSet<String>,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
) {
    let list = crate::types::list_c_type(elem);
    let elem_c = elem.c_type();
    let elem_drops = needs_drop(elem, owned);
    // An element with no drop rule (a scalar) has nothing to free, so the
    // name is only ever computed for one that does.
    let drop_elem = elem_drops.then(|| drop_fn_name(elem));
    writeln!(out, "// ---- {} (collections) {}", list, "-".repeat(30)).unwrap();

    writeln!(out, "static {list} {list}__empty(void) {{").unwrap();
    writeln!(out, "    {list} l;\n    l.data = NULL;\n    l.len = 0;\n    l.cap = 0;\n    return l;\n}}\n").unwrap();

    writeln!(out, "static void {list}__reserve({list}* self, size_t want) {{").unwrap();
    out.push_str("    if (want <= self->cap) {\n        return;\n    }\n");
    out.push_str("    size_t cap = self->cap == 0 ? 4 : self->cap;\n");
    out.push_str("    while (cap < want) {\n        cap *= 2;\n    }\n");
    writeln!(out, "    {elem_c}* p = ({elem_c}*)realloc(self->data, cap * sizeof({elem_c}));").unwrap();
    out.push_str("    if (p == NULL) {\n        kelp_panic(\"out of memory\");\n    }\n");
    out.push_str("    self->data = p;\n    self->cap = cap;\n}\n\n");

    writeln!(out, "static {list} {list}__of({elem_c} v) {{").unwrap();
    writeln!(out, "    {list} l = {list}__empty();").unwrap();
    writeln!(out, "    {list}__reserve(&l, 1);\n    l.data[0] = v;\n    l.len = 1;\n    return l;\n}}\n").unwrap();

    writeln!(out, "static size_t {list}__len(const {list}* self) {{\n    return self->len;\n}}\n").unwrap();
    writeln!(out, "static bool {list}__is_empty(const {list}* self) {{\n    return self->len == 0;\n}}\n").unwrap();

    writeln!(out, "static void {list}__push({list}* self, {elem_c} v) {{").unwrap();
    writeln!(out, "    {list}__reserve(self, self->len + 1);\n    self->data[self->len++] = v;\n}}\n").unwrap();

    // A `get` for a Copy element hands the value back; an owned element is
    // reached by pointer, because copying it out would be a second owner.
    writeln!(out, "static void {list}__bounds(const {list}* self, size_t i) {{").unwrap();
    writeln!(out, "    if (i >= self->len) {{\n        kelp_panic(\"List.get: index out of bounds\");\n    }}\n}}\n").unwrap();
    if elem.is_copy() {
        writeln!(out, "static {elem_c} {list}__get(const {list}* self, size_t i) {{").unwrap();
        writeln!(out, "    {list}__bounds(self, i);\n    return self->data[i];\n}}\n").unwrap();
    } else {
        writeln!(out, "static const {elem_c}* {list}__get_const(const {list}* self, size_t i) {{").unwrap();
        writeln!(out, "    {list}__bounds(self, i);\n    return &self->data[i];\n}}\n").unwrap();
    }

    writeln!(out, "static void {list}__set({list}* self, size_t i, {elem_c} v) {{").unwrap();
    writeln!(out, "    {list}__bounds(self, i);").unwrap();
    if elem_drops {
        writeln!(out, "    {}(&self->data[i]);", drop_elem.as_deref().unwrap()).unwrap();
    }
    out.push_str("    self->data[i] = v;\n}\n\n");

    writeln!(out, "static {elem_c} {list}__pop({list}* self) {{").unwrap();
    out.push_str("    if (self->len == 0) {\n        kelp_panic(\"List.pop: the list is empty\");\n    }\n");
    out.push_str("    return self->data[--self->len];\n}\n\n");

    writeln!(out, "static void {list}__clear({list}* self) {{").unwrap();
    if elem_drops {
        writeln!(
            out,
            "    for (size_t i = 0; i < self->len; i++) {{\n        {}(&self->data[i]);\n    }}",
            drop_elem.as_deref().unwrap()
        )
        .unwrap();
    }
    out.push_str("    self->len = 0;\n}\n\n");

    write_list_s1(out, elem, &list, &elem_c, clonable, sortable);

    writeln!(out, "static void {}({list}* self) {{", crate::types::drop_fn_name(&Ty::List(Box::new(elem.clone())))).unwrap();
    writeln!(out, "    {list}__clear(self);\n    free(self->data);\n    self->data = NULL;\n    self->cap = 0;\n}}\n").unwrap();
}

/// The S1 half of a `List<T>` function set: ordering, membership, a copied
/// range, and the two operations that shift elements around. Which of these
/// exist depends on the element type, exactly as `infer::list_sig` decides
/// when it offers the signature — `types::list_sortable_elem` and its
/// siblings are the single source of truth for both passes.
fn write_list_s1(
    out: &mut String,
    elem: &Ty,
    list: &str,
    elem_c: &str,
    clonable: &HashSet<String>,
    sortable: &HashSet<String>,
) {
    let elem_sortable = crate::types::value_is_sortable(elem, sortable);
    let elem_clonable = crate::types::value_is_clonable(elem, clonable);
    if elem_sortable {
        // One comparator per element kind, so the sort itself stays one
        // heapsort. `String`/`Bytes` compare length-aware: `strcmp`
        // would stop at an embedded NUL and ignore everything after it.
        writeln!(out, "static int {list}__cmp(const {elem_c}* a, const {elem_c}* b) {{").unwrap();
        match elem {
            Ty::Bool => {
                out.push_str("    return (*a ? 1 : 0) - (*b ? 1 : 0);\n}\n\n");
            }
            Ty::String | Ty::Bytes => {
                out.push_str("    size_t n = a->len < b->len ? a->len : b->len;\n");
                out.push_str("    if (n > 0) {\n        int c = memcmp(a->data, b->data, n);\n        if (c != 0) {\n            return c < 0 ? -1 : 1;\n        }\n    }\n");
                out.push_str("    return a->len < b->len ? -1 : (a->len > b->len ? 1 : 0);\n}\n\n");
            }
            // A `datac`/`enum` element orders by its derived comparison, which
            // is field by field in declaration order (MISSING-jL9tE).
            Ty::Struct(_) => {
                writeln!(out, "    return {}(a, b);\n}}\n", cmp_fn_name(elem)).unwrap();
            }
            _ => {
                out.push_str("    return (*a > *b) - (*a < *b);\n}\n\n");
            }
        }

        // One sift-down for a max-heap: heapify and heapsort below both use
        // it, so the child/parent comparison lives in one place. `cmp`
        // returns > 0 when its first argument is greater, the comparator's
        // existing convention. `base` is the offset of the heap's own index 0
        // within `self->data`, so the same sift-down serves both a full-list
        // heapsort and the depth-limited introsort fallback's heapsort over
        // just one partition (`self->data[base..base+heap_len)`).
        writeln!(out, "static void {list}__sift_down({list}* self, size_t base, size_t root, size_t heap_len) {{").unwrap();
        out.push_str("    for (;;) {\n        size_t child = 2 * root + 1;\n        if (child >= heap_len) {\n            return;\n        }\n");
        writeln!(out, "        if (child + 1 < heap_len && {list}__cmp(&self->data[base + child], &self->data[base + child + 1]) < 0) {{\n            child++;\n        }}").unwrap();
        writeln!(out, "        if ({list}__cmp(&self->data[base + root], &self->data[base + child]) >= 0) {{\n            return;\n        }}").unwrap();
        writeln!(out, "        {elem_c} t = self->data[base + root];\n        self->data[base + root] = self->data[base + child];\n        self->data[base + child] = t;\n        root = child;\n    }}\n}}\n\n").unwrap();

        // heapsort over the half-open range `[lo, hi)`: guaranteed O(n log n),
        // in place, no extra buffer. Used both as introsort's depth-limit
        // fallback (over a partition) and, with `lo = 0, hi = self->len`, as
        // an ordinary whole-list heapsort.
        writeln!(out, "static void {list}__heapsort_range({list}* self, size_t lo, size_t hi) {{").unwrap();
        out.push_str("    size_t n = hi - lo;\n");
        writeln!(out, "    for (size_t start = n / 2; start-- > 0; ) {{\n        {list}__sift_down(self, lo, start, n);\n    }}").unwrap();
        writeln!(out, "    for (size_t end = n; end-- > 1; ) {{\n        {elem_c} t = self->data[lo];\n        self->data[lo] = self->data[lo + end];\n        self->data[lo + end] = t;\n        {list}__sift_down(self, lo, 0, end);\n    }}").unwrap();
        out.push_str("}\n\n");

        // Insertion sort over `[lo, hi)`: lower constant factor than
        // quicksort for a small partition, so introsort finishes any
        // partition below the cutoff with this instead of recursing further.
        writeln!(out, "static void {list}__insertion_sort({list}* self, size_t lo, size_t hi) {{").unwrap();
        out.push_str("    for (size_t i = lo + 1; i < hi; i++) {\n");
        writeln!(out, "        {elem_c} key = self->data[i];\n        size_t j = i;").unwrap();
        writeln!(out, "        while (j > lo && {list}__cmp(&self->data[j - 1], &key) > 0) {{\n            self->data[j] = self->data[j - 1];\n            j--;\n        }}").unwrap();
        out.push_str("        self->data[j] = key;\n    }\n}\n\n");

        // The index (among `a`, `b`, `c`) whose element is the median of the
        // three, the standard pivot choice that defeats quicksort's classic
        // already-sorted/reverse-sorted worst cases without anything fancier.
        writeln!(out, "static size_t {list}__median3({list}* self, size_t a, size_t b, size_t c) {{").unwrap();
        writeln!(out, "    if ({list}__cmp(&self->data[a], &self->data[b]) < 0) {{").unwrap();
        out.push_str("        if (");
        writeln!(out, "{list}__cmp(&self->data[b], &self->data[c]) < 0) {{\n            return b;\n        }}").unwrap();
        writeln!(out, "        if ({list}__cmp(&self->data[a], &self->data[c]) < 0) {{\n            return c;\n        }}").unwrap();
        out.push_str("        return a;\n    }\n");
        writeln!(out, "    if ({list}__cmp(&self->data[a], &self->data[c]) < 0) {{\n        return a;\n    }}").unwrap();
        writeln!(out, "    if ({list}__cmp(&self->data[b], &self->data[c]) < 0) {{\n        return c;\n    }}").unwrap();
        out.push_str("    return b;\n}\n\n");

        // Partition over the inclusive range `[lo, hi]` around a
        // median-of-three pivot, returning the split point `j`: everything in
        // `[lo, j]` compares <= everything in `[j + 1, hi]`. This is
        // pdqsort/BlockQuicksort's branchless partition, not a plain Hoare
        // scan: while at least two full blocks (`KELP_SORT_BLOCK` elements
        // each) remain between the two cursors `l` and `rr`, each round scans
        // one block from each side and records, into `offsets_l`/`offsets_r`,
        // the in-block offsets of elements on the wrong side of the pivot
        // (>= pivot for the left block, <= pivot for the right block). That
        // scan is the branch-free part: every iteration stores its offset
        // unconditionally and folds the comparison's outcome into an
        // increment (`num_l += (... >= 0)`) instead of a conditional branch,
        // so there is nothing here for the branch predictor to get wrong on
        // random data the way a per-element Hoare scan does. Only `num =
        // min(num_l, num_r)` pairs get swapped this round; if one side has
        // leftover flagged offsets (because the two counts differed), that
        // side's block does not advance and those offsets simply carry over
        // to be matched against next round's fresh block from the other
        // side, exactly as pdqsort's own reference implementation does.
        //
        // Once fewer than `2 * KELP_SORT_BLOCK` elements remain between `l`
        // and `rr`, the block loop stops and the tail is finished with the
        // exact same branchy Hoare scan the previous, block-free version of
        // this function used end to end (`i = l - 1`, `j = rr`, matching
        // `lo = l, hi = rr - 1` passed to that scan): any offsets collected
        // above but not yet swapped are still sitting exactly where they
        // started inside `[l, rr)` (a swap only ever happens for a matched
        // pair, and unmatched offsets are pure bookkeeping, never touched),
        // so `[l, rr)` is still plain, valid, unpartitioned data and the
        // tail scan re-derives its own classification from scratch; this
        // wastes at most `O(KELP_SORT_BLOCK)` redundant comparisons per
        // partition call, negligible next to the elements already handled
        // branch-free above. `i = lo - 1` type wraparound reasoning for the
        // tail scan's `l - 1` is the same as the previous version's: `l >=
        // lo` always holds here, and if `l == lo == 0` this wraps to
        // `SIZE_MAX` and the very next `i++` wraps it back to `lo`. The pivot
        // value is copied into a local once (a plain element copy, never
        // cloned or dropped, exactly as every swap here), so it stays valid
        // as a sentinel no matter how the elements around it move, and
        // comparing against that copy rather than tracking the pivot's index
        // is what keeps an all-equal-keys input from degrading: every scan,
        // block or tail, stops as soon as it reaches a key equal to the
        // pivot rather than running to the end.
        writeln!(out, "static size_t {list}__partition({list}* self, size_t lo, size_t hi) {{").unwrap();
        out.push_str("    size_t mid = lo + (hi - lo) / 2;\n");
        writeln!(out, "    size_t pivot_idx = {list}__median3(self, lo, mid, hi);").unwrap();
        writeln!(out, "    {elem_c} pivot = self->data[pivot_idx];").unwrap();
        out.push_str("    size_t l = lo;\n    size_t rr = hi + 1;\n");
        writeln!(out, "    size_t offsets_l[{KELP_SORT_BLOCK}];").unwrap();
        writeln!(out, "    size_t offsets_r[{KELP_SORT_BLOCK}];").unwrap();
        out.push_str("    size_t num_l = 0, num_r = 0;\n    size_t start_l = 0, start_r = 0;\n");
        writeln!(out, "    while (rr - l > 2 * {KELP_SORT_BLOCK}) {{").unwrap();
        out.push_str("        if (num_l == 0) {\n            start_l = 0;\n");
        writeln!(out, "            for (size_t i = 0; i < {KELP_SORT_BLOCK}; i++) {{").unwrap();
        out.push_str("                offsets_l[num_l] = i;\n");
        writeln!(out, "                num_l += ({list}__cmp(&self->data[l + i], &pivot) >= 0);").unwrap();
        out.push_str("            }\n        }\n");
        out.push_str("        if (num_r == 0) {\n            start_r = 0;\n");
        writeln!(out, "            size_t rbase = rr - {KELP_SORT_BLOCK};").unwrap();
        writeln!(out, "            for (size_t i = 0; i < {KELP_SORT_BLOCK}; i++) {{").unwrap();
        out.push_str("                offsets_r[num_r] = i;\n");
        writeln!(out, "                num_r += ({list}__cmp(&self->data[rbase + i], &pivot) <= 0);").unwrap();
        out.push_str("            }\n        }\n");
        out.push_str("        size_t num = num_l < num_r ? num_l : num_r;\n");
        writeln!(out, "        size_t rbase = rr - {KELP_SORT_BLOCK};").unwrap();
        out.push_str("        for (size_t k = 0; k < num; k++) {\n");
        out.push_str("            size_t li = l + offsets_l[start_l + k];\n");
        out.push_str("            size_t ri = rbase + offsets_r[start_r + k];\n");
        writeln!(out, "            {elem_c} t = self->data[li];\n            self->data[li] = self->data[ri];\n            self->data[ri] = t;\n        }}").unwrap();
        out.push_str("        num_l -= num; start_l += num;\n        num_r -= num; start_r += num;\n");
        writeln!(out, "        if (num_l == 0) {{ l += {KELP_SORT_BLOCK}; }}").unwrap();
        writeln!(out, "        if (num_r == 0) {{ rr -= {KELP_SORT_BLOCK}; }}").unwrap();
        out.push_str("    }\n");
        out.push_str("    size_t i = l - 1;\n    size_t j = rr;\n    for (;;) {\n");
        writeln!(out, "        do {{ i++; }} while ({list}__cmp(&self->data[i], &pivot) < 0);").unwrap();
        writeln!(out, "        do {{ j--; }} while ({list}__cmp(&self->data[j], &pivot) > 0);").unwrap();
        out.push_str("        if (i >= j) {\n            return j;\n        }\n");
        writeln!(out, "        {elem_c} t = self->data[i];\n        self->data[i] = self->data[j];\n        self->data[j] = t;\n    }}\n}}\n\n").unwrap();

        // Introsort's quicksort driver over the half-open range `[lo, hi)`.
        // Below the insertion-sort cutoff, finish the partition with plain
        // insertion sort. Otherwise partition around a median-of-three pivot
        // and recurse into the smaller side while looping into the larger
        // one, which bounds the real call stack to O(log n) on its own; the
        // separate `depth_limit`, decremented once per partition step and
        // starting at roughly `2 * floor(log2(n))`, is what actually bounds
        // the *work*, not just the stack: once it hits zero, the remaining
        // range is handed to the existing heapsort instead of continuing to
        // partition, which is what keeps an adversarial input (one that would
        // make plain quicksort's partitioning stay badly imbalanced) at
        // O(n log n) overall rather than degrading toward O(n^2).
        writeln!(out, "static void {list}__quicksort({list}* self, size_t lo, size_t hi, size_t depth_limit) {{").unwrap();
        writeln!(out, "    while (hi - lo > {}) {{", KELP_SORT_INSERTION_CUTOFF).unwrap();
        out.push_str("        if (depth_limit == 0) {\n");
        writeln!(out, "            {list}__heapsort_range(self, lo, hi);\n            return;\n        }}").unwrap();
        out.push_str("        depth_limit--;\n");
        writeln!(out, "        size_t p = {list}__partition(self, lo, hi - 1);").unwrap();
        out.push_str("        size_t left_len = (p + 1) - lo;\n        size_t right_len = hi - (p + 1);\n");
        out.push_str("        if (left_len < right_len) {\n");
        writeln!(out, "            {list}__quicksort(self, lo, p + 1, depth_limit);").unwrap();
        out.push_str("            lo = p + 1;\n        } else {\n");
        writeln!(out, "            {list}__quicksort(self, p + 1, hi, depth_limit);").unwrap();
        out.push_str("            hi = p + 1;\n        }\n    }\n");
        writeln!(out, "    {list}__insertion_sort(self, lo, hi);\n}}\n\n").unwrap();

        // introsort: quicksort with a median-of-three pivot and an
        // insertion-sort cutoff, falling back to the heapsort above once
        // recursion depth passes roughly `2 * floor(log2(n))`. In place
        // throughout, no extra buffer: a swap or a pivot copy is an
        // element-sized copy, so an owned element is only moved, never cloned
        // or dropped, exactly as {list}__reverse elsewhere.
        writeln!(out, "static void {list}__sort({list}* self) {{").unwrap();
        out.push_str("    size_t n = self->len;\n    if (n < 2) {\n        return;\n    }\n");
        out.push_str("    size_t depth_limit = 0;\n    for (size_t nn = n; nn > 1; nn >>= 1) {\n        depth_limit++;\n    }\n    depth_limit *= 2;\n");
        writeln!(out, "    {list}__quicksort(self, 0, n, depth_limit);\n}}\n\n").unwrap();
    }

    writeln!(out, "static void {list}__reverse({list}* self) {{").unwrap();
    out.push_str("    size_t i = 0;\n    size_t j = self->len;\n    while (i + 1 < j) {\n        j--;\n");
    writeln!(out, "        {elem_c} t = self->data[i];\n        self->data[i] = self->data[j];\n        self->data[j] = t;\n        i++;\n    }}\n}}\n").unwrap();

    if crate::types::list_comparable_elem(elem) {
        let (param, eq) = match elem {
            Ty::String | Ty::Bytes => (
                format!("const {elem_c}*"),
                format!(
                    "self->data[i].len == v->len && (v->len == 0 || memcmp(self->data[i].data, v->data, v->len) == 0)"
                ),
            ),
            _ => (elem_c.to_string(), "self->data[i] == v".to_string()),
        };
        writeln!(out, "static bool {list}__contains(const {list}* self, {param} v) {{").unwrap();
        writeln!(out, "    for (size_t i = 0; i < self->len; i++) {{\n        if ({eq}) {{\n            return true;\n        }}\n    }}\n    return false;\n}}\n").unwrap();
    }

    if elem_clonable {
        writeln!(out, "static {list} {list}__slice(const {list}* self, size_t from, size_t to) {{").unwrap();
        out.push_str("    if (from > to || to > self->len) {\n        kelp_panic(\"List.slice: range out of bounds\");\n    }\n");
        writeln!(out, "    {list} out = {list}__empty();").unwrap();
        writeln!(out, "    {list}__reserve(&out, to - from);").unwrap();
        writeln!(out, "    for (size_t i = from; i < to; i++) {{").unwrap();
        match elem {
            Ty::String => {
                writeln!(out, "        out.data[out.len++] = kelp_String__clone(&self->data[i]);").unwrap();
            }
            Ty::Bytes => {
                writeln!(out, "        out.data[out.len++] = kelp_Bytes__clone(&self->data[i]);").unwrap();
            }
            // An owned `datac`/`enum` element is deep-copied into the new
            // list's own storage, so the two lists share nothing.
            Ty::Struct(_) => {
                writeln!(
                    out,
                    "        {}(&self->data[i], &out.data[out.len]);\n        out.len++;",
                    clone_fn_name(elem)
                )
                .unwrap();
            }
            _ => {
                writeln!(out, "        out.data[out.len++] = self->data[i];").unwrap();
            }
        }
        out.push_str("    }\n    return out;\n}\n\n");
        // `clone()` is the whole-list case of the same copy: no bounds to
        // check, so it forwards to `slice` rather than repeating the loop.
        writeln!(out, "static {list} {list}__clone(const {list}* self) {{\n    return {list}__slice(self, 0, self->len);\n}}\n").unwrap();
    }

    writeln!(out, "static void {list}__insert({list}* self, size_t i, {elem_c} v) {{").unwrap();
    out.push_str("    if (i > self->len) {\n        kelp_panic(\"List.insert: index out of bounds\");\n    }\n");
    writeln!(out, "    {list}__reserve(self, self->len + 1);").unwrap();
    out.push_str("    for (size_t j = self->len; j > i; j--) {\n        self->data[j] = self->data[j - 1];\n    }\n    self->data[i] = v;\n    self->len++;\n}\n\n");

    writeln!(out, "static {elem_c} {list}__remove({list}* self, size_t i) {{").unwrap();
    out.push_str("    if (i >= self->len) {\n        kelp_panic(\"List.remove: index out of bounds\");\n    }\n");
    writeln!(out, "    {elem_c} v = self->data[i];").unwrap();
    out.push_str("    for (size_t j = i; j + 1 < self->len; j++) {\n        self->data[j] = self->data[j + 1];\n    }\n    self->len--;\n    return v;\n}\n\n");
}

/// Every container instantiation the program mentions, so `generate` knows
/// which C typedefs and function sets to write. A `Box<T>` may contain
/// another container (`Box<List<String>>`), so the walk recurses; a borrow
/// of a container (`&List<T>` — every `List` parameter and method receiver)
/// is looked through.
/// Every container and closure instantiation the program uses (M8, A-4
/// phase E): one C typedef plus, for a container, one function set per
/// element type, and for a closure one drop function per signature. Collected
/// in one walk, because the emitters need the same list and the same order.
#[derive(Default)]
struct Instantiations {
    lists: HashSet<Ty>,
    boxes: HashSet<Ty>,
    maps: HashSet<(Ty, Ty)>,
    /// Closure signatures (A-4 phase E). A closure's C type is `{ fn, env,
    /// drop }` monomorphized per signature, so it is emitted exactly like a
    /// container: before the `datac`s, with the datac tags forward-declared.
    closures: HashSet<Ty>,
    /// Closure signatures that a plain function value is widened into
    /// somewhere in the program, so each needs its thunk (A-4 phase E).
    coerced: HashSet<Ty>,
    /// Widened values whose function is a *known* one — a declared `fun`
    /// named as a value, or a literal that captured nothing (whose value is a
    /// `FnRef` too). Each needs a shim instead of a thunk and a heap
    /// environment: the wrapper has nothing to hold, so `env` is `NULL` and
    /// the conversion allocates nothing (A-4 phase E).
    shims: HashSet<(String, Ty)>,
    /// Array instantiations (A-18): `[T; N]` needs one C struct, one function
    /// set and one drop per `(element, length)` pair the program uses. Kept
    /// apart from the pointer containers above because an array holds its
    /// elements *inline*: it has to be emitted after the types it contains,
    /// not before them.
    arrays: HashSet<Ty>,
    /// `Atomic<T>` instantiations (MK9-S3-M1 stage A1): one one-member struct
    /// per element type, which is the whole C representation. There is no
    /// function set beside it, because every operation is a single
    /// `__atomic_*` builtin written at the call site rather than a routine.
    atomics: HashSet<Ty>,
    /// The closure signatures a `thread.spawn` entry reaches (MK9-S3-M1 stage
    /// C1). Each needs one generated worker body, which is what the runtime's
    /// trampoline calls inside the new thread; the body's parameters are
    /// pointers, so one per signature is enough and the element type stays out
    /// of it.
    spawns: HashSet<Ty>,
}

fn collect_containers(program: &Program, out: &mut Instantiations) {
    fn note_ty(ty: &Ty, out: &mut Instantiations) {
        match ty {
            Ty::List(elem) => {
                out.lists.insert((**elem).clone());
                note_ty(elem, out);
            }
            Ty::Box(elem) => {
                out.boxes.insert((**elem).clone());
                note_ty(elem, out);
            }
            Ty::Map(key, value) => {
                out.maps.insert(((**key).clone(), (**value).clone()));
                // `keys()`/`values()` are emitted with every map and return
                // `List<K>`/`List<V>`, so both instantiations have to exist
                // even when the program never calls them.
                out.lists.insert((**key).clone());
                out.lists.insert((**value).clone());
                note_ty(key, out);
                note_ty(value, out);
            }
            // A closure is emitted once per signature, and its parameters and
            // return type may name another closure or a container.
            Ty::Closure { params, ret } => {
                out.closures.insert(ty.clone());
                for p in params {
                    note_ty(p, out);
                }
                note_ty(ret, out);
            }
            Ty::Array(elem, _) => {
                out.arrays.insert(ty.clone());
                note_ty(elem, out);
            }
            Ty::Atomic(elem) => {
                out.atomics.insert(ty.clone());
                note_ty(elem, out);
            }
            Ty::Ref { inner, .. } => note_ty(inner, out),
            _ => {}
        }
    }
    fn note_expr(expr: &Expr, out: &mut Instantiations) {
        note_ty(&expr.ty, out);
        match &expr.kind {
            ExprKind::Unary { expr, .. } | ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } => {
                note_expr(expr, out)
            }
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { target: lhs, value: rhs } => {
                note_expr(lhs, out);
                note_expr(rhs, out);
            }
            ExprKind::Call { callee, args } => {
                // `thread.spawn(entry, arg)` (MK9-S3-M1 stage C1): the entry is
                // a closure by the checker's coercion, and that signature is
                // what the worker body is generated for.
                if callee == crate::stdlib::SPAWN_FN {
                    if let Some(entry) = args.first() {
                        out.spawns.insert(entry.ty.clone());
                    }
                }
                for a in args {
                    note_expr(a, out);
                }
            }
            ExprKind::IndirectCall { callee, args } => {
                note_expr(callee, out);
                for a in args {
                    note_expr(a, out);
                }
            }
            ExprKind::FieldAccess { base, .. } => note_expr(base, out),
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    note_expr(v, out);
                }
            }
            ExprKind::EnumLit { fields, .. } => {
                for v in fields {
                    note_expr(v, out);
                }
            }
            ExprKind::EnumTag { expr: base, .. } | ExprKind::EnumField { base, .. } => {
                note_expr(base, out)
            }
            // A closure literal's initializers are expressions in the
            // *enclosing* function, so their types need the same walk as any
            // other value's.
            ExprKind::ClosureLit { captures, .. } => {
                for c in captures {
                    note_expr(c, out);
                }
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    note_expr(e, out);
                }
            }
            ExprKind::ArrayRepeat { value, .. } => note_expr(value, out),
            // A widened function value: the target signature needs its thunk,
            // and the value being wrapped needs the walk too. A value that is
            // a known function needs a shim instead of the heap wrapper.
            ExprKind::FnToClosure { func } => {
                if let ExprKind::FnRef(key) = &func.kind {
                    out.shims.insert((key.clone(), expr.ty.clone()));
                }
                out.coerced.insert(expr.ty.clone());
                note_expr(func, out);
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                note_expr(cond, out);
                note_block(then_block, out);
                if let Some(b) = else_block {
                    note_block(b, out);
                }
            }
            ExprKind::Block(b) => note_block(b, out),
            ExprKind::Template { pieces, .. } => {
                for p in pieces {
                    if let TemplatePiece::Value(v) = p {
                        note_expr(v, out);
                    }
                }
            }
            _ => {}
        }
    }
    fn note_block(block: &Block, out: &mut Instantiations) {
        for s in &block.stmts {
            match s {
                Stmt::Let { ty, init, .. } => {
                    note_ty(ty, out);
                    note_expr(init, out);
                }
                Stmt::While { cond, body } => {
                    note_expr(cond, out);
                    note_block(body, out);
                }
                Stmt::Return { value, .. } => {
                    if let Some(v) = value {
                        note_expr(v, out);
                    }
                }
                Stmt::Expr(e) => note_expr(e, out),
            }
        }
        if let Some(t) = &block.tail {
            note_expr(t, out);
        }
    }

    for s in &program.structs {
        for f in &s.fields {
            note_ty(&f.ty, out);
        }
    }
    // An enum payload is a struct field as far as containers are concerned: a
    // `List<Shape>` payload needs its instantiation emitted like any other.
    for e in &program.enums {
        note_ty(&Ty::Struct(e.name.clone()), out);
        for v in &e.variants {
            for f in &v.fields {
                note_ty(&f.ty, out);
            }
        }
    }
    for pd in &program.pds {
        note_ty(&pd.value_ty, out);
        note_expr(&pd.init, out);
    }
    for f in &program.functions {
        note_ty(&f.ret, out);
        for (_, t) in &f.params {
            note_ty(t, out);
        }
        // A literal's environment fields are values the generated function
        // holds, so their types need instantiations too.
        if let Some(env) = &f.env {
            for (_, t) in &env.fields {
                note_ty(t, out);
            }
        }
        note_block(&f.body, out);
    }
}

/// One `fun` literal's environment (A-4 phase E): the struct its captures were
/// moved into, and the function that frees it. Emitted immediately before the
/// generated function that receives it, because nothing else names either.
///
/// Each owned field is dropped through `types::drop_fn_name`, then the struct
/// itself is freed — the same order a `datac`'s drop uses, one level down.
fn write_env(out: &mut String, f: &Function, owned: &HashSet<String>) {
    let Some(env) = &f.env else {
        return;
    };
    writeln!(out, "// ---- {} (A-4 phase E) {}", env.struct_name, "-".repeat(28)).unwrap();
    out.push_str("typedef struct {\n");
    for (name, ty) in &env.fields {
        writeln!(out, "    {};", ty.c_decl(name)).unwrap();
    }
    writeln!(out, "}} {};\n", env.struct_name).unwrap();
    writeln!(out, "static void {}(void* raw) {{", env.drop_name).unwrap();
    writeln!(
        out,
        "    {}* e = ({}*)raw;",
        env.struct_name, env.struct_name
    )
    .unwrap();
    for (name, ty) in &env.fields {
        if needs_drop(ty, owned) {
            writeln!(out, "    {}(&e->{name});", drop_fn_name(ty)).unwrap();
        }
    }
    out.push_str("    free(e);\n}\n\n");
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

fn write_signature(out: &mut String, f: &Function) {
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
fn write_params(out: &mut String, params: &[(String, Ty)], env: bool) {
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
fn fn_params_of(params: &[Ty]) -> String {
    crate::types::fn_params_c(params)
}

/// Writing state that outlives a single expression: the output buffer, the
/// drop plan, and which function is being written (the plan is keyed
/// per-function, and a `Gen` is created fresh for each function).
struct Gen<'a> {
    out: String,
    drops: &'a DropPlan,
    /// The `enum` table, so a variant construction and a payload read can name
    /// the C union member and field they mean (A-1). A slice, not a map: the
    /// declaration order is what codegen walks, and the lookups are few.
    enums: &'a [EnumDef],
    /// Which `datac`s own heap memory, so a temporary hoisted for a borrowed
    /// call argument can be freed after the call (see `call_value`).
    owned: &'a HashSet<String>,
    function: String,
    /// The module the current function was declared in: capability checks use
    /// *that* module's patterns, which is what makes a grant a property of
    /// the code that made the call (M7).
    module: String,
    caps: &'a CapTable,
    /// The loaded sources, used only to turn a capability denial's span into
    /// the `file:line:col` the runtime panic names (M7).
    sources: &'a SourceMap,
    /// The `.kp` local bindings the C written so far has in scope, innermost
    /// scope last: a spelling maps to the C name its newest binding declared.
    /// A read resolves through here rather than by spelling the name again,
    /// because a second binding of one name in a scope is a shadow and C has
    /// one namespace per block, so it cannot repeat the declaration
    /// (BUG-v6Qe0).
    locals: Vec<HashMap<String, String>>,
    /// The C name each owned binding's declaration gave it, keyed by the
    /// binding the drop plan recorded. A drop resolves through here rather
    /// than through `locals`, because one spelling can name several bindings
    /// and the drop belongs to the binding that owns the value (BUG-n7Wq3).
    bindings: HashMap<BindingId, String>,
}

impl<'a> Gen<'a> {
    fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.name == name)
    }

    fn new(
        drops: &'a DropPlan,
        owned: &'a HashSet<String>,
        enums: &'a [EnumDef],
        function: String,
        module: String,
        caps: &'a CapTable,
        sources: &'a SourceMap,
    ) -> Self {
        Gen {
            out: String::new(),
            drops,
            enums,
            owned,
            function,
            module,
            caps,
            sources,
            locals: Vec::new(),
            bindings: HashMap::new(),
        }
    }

    /// A nested emitter for a block a value-renderer has to produce: same
    /// plan, same function, same module, its own buffer.
    fn sub(&self) -> Gen<'a> {
        let mut g = Gen::new(
            self.drops,
            self.owned,
            self.enums,
            self.function.clone(),
            self.module.clone(),
            self.caps,
            self.sources,
        );
        // A sub renders a block the enclosing C still nests inside, so the
        // bindings around it are visible in the text it writes, and the drops
        // it writes name them.
        g.locals = self.locals.clone();
        g.bindings = self.bindings.clone();
        g
    }

    /// Opens the scope of one C block, which every emitter writing a `{` the
    /// C nests inside does, so a rebinding in that block shadows rather than
    /// redeclares.
    fn push_locals(&mut self) {
        self.locals.push(HashMap::new());
    }

    fn pop_locals(&mut self) {
        self.locals.pop();
    }

    /// Declares `name` in the innermost scope and returns the C name to write
    /// for it.
    ///
    /// A name any open scope already binds gets a generated name of its own,
    /// and not only a name the innermost scope binds: C puts a block-scope
    /// declaration in scope inside its *own initializer*, so a nested `val a`
    /// written `kelp_a` would make `val a = a + 1` read the uninitialized new
    /// variable instead of the `a` the `.kp` initializer names. Called *after*
    /// the initializer has been rendered, for the same reason.
    fn declare_local(&mut self, name: &str, binding: BindingId) -> String {
        let rebinds = self.locals.iter().any(|scope| scope.contains_key(name));
        let c = if rebinds { shadow_name(name) } else { c_name(name) };
        self.locals
            .last_mut()
            .expect("a local is declared inside a scope")
            .insert(name.to_string(), c.clone());
        self.bindings.insert(binding, c.clone());
        c
    }

    /// The C name a read, a write or a drop of `name` resolves to: the newest
    /// binding of that spelling whose block is still open, or the plain
    /// spelling when the name is not a local at all: a function, a passdown's
    /// storage, the hidden parameter of one.
    fn local_c_name(&self, name: &str) -> String {
        for scope in self.locals.iter().rev() {
            if let Some(c) = scope.get(name) {
                return c.clone();
            }
        }
        c_name(name)
    }

    /// The C name a drop of `target` frees: the C variable the binding's own
    /// declaration made, so a rebound or shadowed spelling frees the value the
    /// declaration owns and not whichever binding of that spelling is newest
    /// (BUG-n7Wq3). A target the plan recorded no binding for (an assignment
    /// to a `pd`'s storage) and one this walk did not declare fall back to the
    /// resolution a read of the name uses.
    fn drop_name(&self, target: &DropTarget) -> String {
        target
            .binding
            .and_then(|binding| self.bindings.get(&binding))
            .cloned()
            .unwrap_or_else(|| self.local_c_name(&target.name))
    }

    fn drops_at_block_end(&self, block: &Block) -> Vec<DropTarget> {
        self.drops.at_block_end(&self.function, block).to_vec()
    }

    /// A condition rendered as a parenthesized C expression, with no second
    /// pair of parentheses when `cond` already renders as one group. Every
    /// binary expression is emitted `(a op b)` — including the `tag == n` a
    /// `when` arm tests — so wrapping it in `if (…)` produced `if ((a == b))`,
    /// which clang flags as an extraneous parenthesis. On the most common
    /// conditions in the language (`if a == b`, `while i < n`, every `when`
    /// arm) that is a warning per occurrence, enough of it on a fresh build to
    /// bury the real diagnostic (BUG-3).
    fn cond_text(&self, cond: &Expr) -> String {
        let rendered = self.expr_value(cond);
        if matches!(&cond.kind, ExprKind::Binary { .. } | ExprKind::EnumTag { .. }) {
            rendered
        } else {
            format!("({rendered})")
        }
    }

    fn drops_at_return(&self, span: Span) -> Vec<DropTarget> {
        self.drops.at_return(&self.function, span).to_vec()
    }

    fn drops_at_try(&self, span: Span) -> Vec<DropTarget> {
        self.drops.at_try(&self.function, span).to_vec()
    }

    /// Frees owned locals in the order `borrowck.rs` decided (reverse
    /// declaration order — a later binding is freed before an earlier one it
    /// may have taken its value from).
    fn write_drops(&mut self, targets: &[DropTarget], depth: usize) {
        for t in targets {
            let target = self.drop_name(t);
            indent(&mut self.out, depth);
            writeln!(self.out, "{}", drop_call(t, &target)).unwrap();
        }
    }

    /// Emits the `#line` directive that maps the C below back to the `.kp` span
    /// it came from (roadmap open item 20).
    ///
    /// Without it a `cc` diagnostic on the generated C names a line of
    /// *generated* C: the whole reason `plan/architecture.md` records "generate
    /// readable, `#line`-annotated C" and the reason a capability denial used to
    /// carry its own baked-in `file:line:col` string. Every `tir` node carries
    /// the span, so this is the mechanical half of a real source-mapping story
    /// (A-16); the column a `#line` cannot express is what the capability denial
    /// still asks `diag::SourceMap` for.
    ///
    /// The directive names a real `.kp` line or it emits nothing: there is no
    /// guess to make, because `sources` either holds the module's text or it
    /// does not. One marker per statement is what makes the *statement*, not
    /// just the function, the resolution, which is the difference between
    /// finding a miscompiled expression and bisecting the function it lives in.
    fn write_line_marker(&mut self, span: Span, depth: usize) {
        if let Some(directive) = self.sources.line_directive(&self.module, span) {
            indent(&mut self.out, depth);
            self.out.push_str(&directive);
            self.out.push('\n');
        }
    }

    // ---- functions, blocks, statements ---------------------------------

    fn write_function(&mut self, f: &Function, init_pds: bool) {
        // Parameters are locals of the body's own block, and C declares both
        // in one scope, so a body binding of a parameter's name is a shadow
        // that needs a name of its own (see `declare_local`). A `unit`
        // parameter has no C parameter of its own (`write_params` leaves it
        // out), so it is not bound to a name either: a read of it is a no-op
        // expression and an assignment to it stores nothing.
        self.push_locals();
        for (index, (name, ty)) in f.params.iter().enumerate() {
            if *ty == Ty::Unit {
                continue;
            }
            let c = c_name(name);
            self.locals
                .last_mut()
                .expect("the function scope is open")
                .insert(name.clone(), c.clone());
            self.bindings.insert(BindingId::Param(index), c);
        }
        if f.name == "main" {
            // The C runtime requires `main` to return `int`; falling off the
            // end is defined by the C standard as `return 0`, so an `.kp`
            // `main` (always `-> unit`) needs no extra trailing return. A
            // bare `return;` from `.kp`, though, is a constraint violation in
            // a function declared to return `int`, so it's rewritten to
            // `return 0;` here rather than threading an "am I in main" flag
            // through every statement-writing function below.
            let mut body = self.sub();
            if init_pds {
                body.out.push_str("    kelp_init_pds();\n");
            }
            body.write_function_body(&f.body, 1, &f.ret);
            let rendered = body.out.replace("return;\n", "return 0;\n");
            // `argv` reaches Kelp through two saved globals rather than through
            // `pd`: `process.args()` is the only reader, and a passdown would
            // have to be initialized by the one function that cannot have
            // parameters (`main`). The signature change is the whole compiler
            // cost of argv (S2, open item 50).
            self.out.push_str("int main(int argc, char** argv) {\n");
            self.out.push_str("    kelp_argc = argc;\n    kelp_argv = argv;\n");
            self.out.push_str(&rendered);
            self.out.push_str("}\n");
            self.pop_locals();
            return;
        }
        write_signature(&mut self.out, f);
        self.out.push_str(" {\n");
        // A literal that captured something receives its environment as the
        // hidden first parameter; binding it here is what makes every
        // `ExprKind::Capture` in the body an `env->cN` read (A-4 phase E).
        if let Some(env) = &f.env {
            writeln!(
                self.out,
                "    {}* env = ({}*)env_raw;",
                env.struct_name, env.struct_name
            )
            .unwrap();
        }
        self.write_function_body(&f.body, 1, &f.ret);
        self.out.push_str("}\n");
        self.pop_locals();
    }

    /// Writes a function body's statements. Unlike a nested block used as a
    /// mere statement (`write_block_body`), a present tail expression here
    /// becomes the function's `return` — not a discarded expression statement
    /// — whenever the function actually returns a value; `infer.rs` treats a
    /// tail-expression body exactly like `return <tail>;` (see
    /// `check_function`), so codegen has to honor that too. For a `Unit`-
    /// returning function (including `main`), the tail is still discarded,
    /// same as any other statement-position block.
    fn write_function_body(&mut self, block: &Block, depth: usize, ret_ty: &Ty) {
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        let drops = self.drops_at_block_end(block);
        if let Some(tail) = &block.tail {
            // The tail is where a function's last expression is, so it gets a
            // marker of its own: it is the line a miscompile in a returned or
            // discarded value has to name.
            self.write_line_marker(tail.span, depth);
            // A body whose tail is a `panic` diverges: `infer.rs` accepts it
            // against any declared return type, and here it is a plain
            // statement — `return kelp_panic(...)` would be a void expression
            // in a non-void function.
            if matches!(&tail.kind, ExprKind::Call { callee, .. } if callee == "panic") {
                self.write_stmt_like_expr(tail, depth);
            } else if *ret_ty == Ty::Unit {
                self.write_stmt_like_expr(tail, depth);
            } else if drops.is_empty() {
                let value = self.expr_value(tail);
                indent(&mut self.out, depth);
                writeln!(self.out, "return {value};").unwrap();
                return;
            } else {
                // Evaluate the returned value *before* freeing the frame's
                // locals: the tail may read through one of them.
                let tmp = self.write_temp_then_drops(tail, &drops, depth);
                indent(&mut self.out, depth);
                writeln!(self.out, "return {tmp};").unwrap();
                return;
            }
        }
        self.write_drops(&drops, depth);
    }

    /// Writes a block's statements (and its discarded tail, if any) as plain
    /// C statements, then its end-of-scope drops. Used for a block in
    /// statement position (`if`/`while` bodies, nested `{ }` blocks).
    fn write_block_body(&mut self, block: &Block, depth: usize) {
        // The block's own `{` is what the caller has already written, so this
        // is one C scope. Drops are written before the scope closes: they name
        // the bindings it declared (see `local_c_name`).
        self.push_locals();
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        if let Some(tail) = &block.tail {
            self.write_line_marker(tail.span, depth);
            self.write_stmt_like_expr(tail, depth);
        }
        let drops = self.drops_at_block_end(block);
        self.write_drops(&drops, depth);
        self.pop_locals();
    }

    fn write_stmt(&mut self, stmt: &Stmt, depth: usize) {
        // One marker per statement, from the span the statement itself carries:
        // a `Let` is the initializer's span (its first token *is* the `val`),
        // a `Return` carries its own, a `While` its condition's.
        match stmt {
            Stmt::Let { init, .. } => self.write_line_marker(init.span, depth),
            Stmt::Return { span, .. } => self.write_line_marker(*span, depth),
            Stmt::While { cond, .. } => self.write_line_marker(cond.span, depth),
            Stmt::Expr(e) => self.write_line_marker(e.span, depth),
        }
        match stmt {
            Stmt::Let { name, ty, init } => {
                // A `unit` binding is zero-sized: it gets no C object to
                // declare (`void kelp_x` is not a declaration) and no drop, so
                // the initializer is emitted for whatever it does and the name
                // is never read as a C variable (`expr_value` answers a `unit`
                // read with a no-op expression).
                if *ty == Ty::Unit {
                    self.write_stmt_like_expr(init, depth);
                    return;
                }
                let value = self.expr_value(init);
                let decl = self.declare_local(name, BindingId::Local(stmt_id(stmt)));
                indent(&mut self.out, depth);
                writeln!(self.out, "{} = {value};", ty.c_decl(&decl)).unwrap();
            }
            Stmt::Return { value, span } => {
                let drops = self.drops_at_return(*span);
                match value {
                    Some(v) if !drops.is_empty() => {
                        let tmp = self.write_temp_then_drops(v, &drops, depth);
                        indent(&mut self.out, depth);
                        writeln!(self.out, "return {tmp};").unwrap();
                    }
                    Some(v) => {
                        let value = self.expr_value(v);
                        indent(&mut self.out, depth);
                        writeln!(self.out, "return {value};").unwrap();
                    }
                    None => {
                        self.write_drops(&drops, depth);
                        indent(&mut self.out, depth);
                        self.out.push_str("return;\n");
                    }
                }
            }
            Stmt::While { cond, body } => {
                let cond = self.cond_text(cond);
                indent(&mut self.out, depth);
                writeln!(self.out, "while {cond} {{").unwrap();
                self.write_block_body(body, depth + 1);
                indent(&mut self.out, depth);
                self.out.push_str("}\n");
            }
            Stmt::Expr(e) => self.write_stmt_like_expr(e, depth),
        }
    }

    /// Writes an expression used as a full statement: `if`/block get rendered
    /// as real C control flow (no statement-expression wrapping needed, since
    /// the value is discarded); everything else becomes `<expr>;`.
    fn write_stmt_like_expr(&mut self, expr: &Expr, depth: usize) {
        match &expr.kind {
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond = self.cond_text(cond);
                indent(&mut self.out, depth);
                writeln!(self.out, "if {cond} {{").unwrap();
                self.write_block_body(then_block, depth + 1);
                indent(&mut self.out, depth);
                self.out.push('}');
                self.write_else(else_block.as_ref(), depth);
                self.out.push('\n');
            }
            ExprKind::Block(block) => {
                indent(&mut self.out, depth);
                self.out.push_str("{\n");
                self.write_block_body(block, depth + 1);
                indent(&mut self.out, depth);
                self.out.push_str("}\n");
            }
            _ => {
                let value = self.expr_value(expr);
                indent(&mut self.out, depth);
                writeln!(self.out, "{value};").unwrap();
            }
        }
    }

    fn write_else(&mut self, else_block: Option<&Block>, depth: usize) {
        let Some(else_block) = else_block else {
            return;
        };
        // Render a chained `else if` without an extra nested block when the
        // else-branch is exactly a single tail `if` expression (an `else if`).
        if else_block.stmts.is_empty() {
            if let Some(tail) = &else_block.tail {
                if let ExprKind::If {
                    cond,
                    then_block,
                    else_block: inner_else,
                } = &tail.kind
                {
                    let cond = self.expr_value(cond);
                    write!(self.out, " else if ({cond}) {{\n").unwrap();
                    self.write_block_body(then_block, depth + 1);
                    indent(&mut self.out, depth);
                    self.out.push('}');
                    self.write_else(inner_else.as_ref(), depth);
                    return;
                }
            }
        }
        self.out.push_str(" else {\n");
        self.write_block_body(else_block, depth + 1);
        indent(&mut self.out, depth);
        self.out.push('}');
    }

    /// Evaluates `expr` into a fresh temporary, then runs `drops` — the shape
    /// every "produce a value, then free the frame" site needs, since the
    /// value may read through something being freed. Records the temporary's
    /// name in `last_temp`.
    fn write_temp_then_drops(&mut self, expr: &Expr, drops: &[DropTarget], depth: usize) -> String {
        let tmp = fresh_temp();
        let value = self.expr_value(expr);
        indent(&mut self.out, depth);
        writeln!(self.out, "{} = {value};", expr.ty.c_decl(&tmp)).unwrap();
        self.write_drops(drops, depth);
        tmp
    }

    // ---- value position -------------------------------------------------

    /// Renders an expression for use as a C *value* (RHS of `=`, call
    /// argument, operand, condition, ...). `if`/block expressions become GNU
    /// statement-expressions here.
    fn expr_value(&self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::IntLit(v) => {
                // Cast to the literal's own type, which is the type `infer.rs`
                // resolved for it from its context. Resolving it is a refusal
                // now when the literal does not fit (BUG-t6Mz2), so a literal
                // wider than its own type reaches this point only under a sign,
                // which is what makes the magnitude representable: `-2147483648`
                // is an `i32` whose inner literal is 2147483648. Without the
                // cast that magnitude would go out at its full 64-bit value and
                // C's usual arithmetic conversions would perform the operation
                // at that width, which is the disagreement with the LLVM
                // backend's `int_const` this cast removes.
                format!("(({})({}))", expr.ty.c_type(), v)
            }
            // A character literal is an integer constant with the code point
            // as its value and the type its context asked for, so it emits a
            // cast and nothing else. `u8` is the common case (`c == '"'`),
            // and `char` is a code point rather than a C `char`, so the cast
            // is what makes both fit.
            ExprKind::CharLit(v) => format!("(({})({}))", expr.ty.c_type(), v),
            ExprKind::FloatLit(v) => c_float_literal(*v),
            ExprKind::StringLit(s) => c_string_literal(s),
            ExprKind::BoolLit(b) => b.to_string(),
            // A read of an owned binding is usually the value itself; at a
            // *move site* it is also the point the binding gives its value up,
            // so the value is copied into a temporary and the binding is
            // zeroed. `DropPlan::moves_at` is what marks one, and the drop the
            // binding still owes on the paths that did not move it then finds
            // the zero value (`kelp_bytes_drop`/`kelp_string_drop` free a NULL
            // `data`, and every generated drop is the same shape one level
            // down). See `borrowck.rs`'s module doc, "a move clears its
            // source".
            ExprKind::Ident(name) => {
                // A `unit` binding is not a C object (`Stmt::Let` declares
                // none), so reading one is a no-op expression rather than a
                // name that does not exist.
                if expr.ty == Ty::Unit {
                    return "(void)0".to_string();
                }
                if self.drops.moves_at(&self.function, expr) {
                    let tmp = fresh_temp();
                    let read = self.local_c_name(name);
                    format!(
                        "({{ {} = {}; {} = ({}){{0}}; {}; }})",
                        expr.ty.c_decl(&tmp),
                        read,
                        read,
                        expr.ty.c_type(),
                        tmp
                    )
                } else {
                    self.local_c_name(name)
                }
            }
            ExprKind::Unary { op, expr: sub } => {
                let sym = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                };
                let inner = self.expr_value(sub);
                format!("({sym}{inner})")
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let sym = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Mod => "%",
                    BinOp::Eq => "==",
                    BinOp::NotEq => "!=",
                    BinOp::Lt => "<",
                    BinOp::LtEq => "<=",
                    BinOp::Gt => ">",
                    BinOp::GtEq => ">=",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                    // The bitwise spellings are C's, including `<<`/`>>`,
                    // which the parser folded from two adjacent `<`/`>`
                    // tokens. `&` is the same character as the borrow prefix
                    // in the surface language and needs no parenthesizing
                    // here: C's `&` is unambiguous by position too.
                    BinOp::BitAnd => "&",
                    BinOp::BitOr => "|",
                    BinOp::BitXor => "^",
                    BinOp::Shl => "<<",
                    BinOp::Shr => ">>",
                };
                let l = self.expr_value(lhs);
                let r = self.expr_value(rhs);
                format!("({l} {sym} {r})")
            }
            ExprKind::Call { callee, args } => self.call_value(callee, args, expr.span, &expr.ty),
            // A function *reference* is the C function itself (A-4): the
            // value carries no environment, so there is nothing to build.
            // Every function in the program is emitted with a prototype, so
            // naming one here needs nothing else.
            ExprKind::FnRef(key) => c_name(key),
            // A `fun` literal that captured something (A-4 phase E): build its
            // environment on the heap, move (or copy) each capture into it,
            // and hand back the fat pointer. One statement expression, so the
            // whole thing is a value like any other — and so the captures are
            // evaluated exactly once, in field order.
            ExprKind::ClosureLit { key, captures } => {
                let env_ty = crate::types::env_struct_name(key);
                let env_drop = crate::types::env_drop_name(key);
                let closure_ty = expr.ty.c_type();
                let env_tmp = fresh_temp();
                let value_tmp = fresh_temp();
                let mut code = format!(
                    "({{ {env_ty}* {env_tmp} = ({env_ty}*)malloc(sizeof({env_ty})); if ({env_tmp} == NULL) {{ kelp_panic(\"out of memory\"); }}"
                );
                for (i, init) in captures.iter().enumerate() {
                    code.push_str(&format!(
                        " {env_tmp}->{} = {};",
                        crate::types::capture_field(i),
                        self.expr_value(init)
                    ));
                }
                code.push_str(&format!(
                    " {closure_ty} {value_tmp}; {value_tmp}.fn = {}; {value_tmp}.env = {env_tmp}; {value_tmp}.drop = {env_drop}; {value_tmp}; }})",
                    c_name(key)
                ));
                code
            }
            // A read of one field of the enclosing closure's environment
            // (A-4 phase E). `env` is the pointer the generated function's
            // prologue bound.
            ExprKind::Capture { index } => {
                format!("env->{}", crate::types::capture_field(*index))
            }
            // `val f: closure(i32).R: i32 = double` (A-4 phase E): a plain
            // function value widened into a closure by giving it a one-field
            // environment and a thunk to call through. A value that *names* a
            // function needs neither the environment nor the indirection: the
            // shim is that function's own signature with the ignored hidden
            // parameter, so `env` is `NULL` and the conversion allocates
            // nothing.
            ExprKind::FnToClosure { func } => {
                let closure_ty = expr.ty.c_type();
                let value_tmp = fresh_temp();
                if let ExprKind::FnRef(key) = &func.kind {
                    return format!(
                        "({{ {closure_ty} {value_tmp}; {value_tmp}.fn = {}; {value_tmp}.env = NULL; {value_tmp}.drop = NULL; {value_tmp}; }})",
                        coerce_shim_name(key, &expr.ty)
                    );
                }
                let env_ty = coerce_env_c_type(&expr.ty);
                let env_tmp = fresh_temp();
                format!(
                    "({{ {env_ty}* {env_tmp} = ({env_ty}*)malloc(sizeof({env_ty})); if ({env_tmp} == NULL) {{ kelp_panic(\"out of memory\"); }} {env_tmp}->f = {}; {closure_ty} {value_tmp}; {value_tmp}.fn = {}; {value_tmp}.env = {env_tmp}; {value_tmp}.drop = {}; {value_tmp}; }})",
                    self.expr_value(func),
                    coerce_thunk_name(&expr.ty),
                    coerce_drop_name(&expr.ty)
                )
            }
            // A call through a function value: the callee is an expression
            // (a local, or a field), so it is parenthesized before the
            // argument list — `(f)(x)`. A closure is a fat pointer, so the
            // call names its code pointer and passes the environment as the
            // hidden first argument; the callee is bound to a temporary
            // first, so a field or a dereference is evaluated once.
            ExprKind::IndirectCall { callee, args } => {
                let (prelude, arg_texts, hoisted) = self.render_arguments(args, &[]);
                let arglist = arg_texts.join(", ");
                match closure_access(&callee.ty) {
                    Some((fn_member, env_member)) => {
                        let tmp = fresh_temp();
                        let value = self.expr_value(callee);
                        let code_ptr = format!("{tmp}{fn_member}");
                        let env_ptr = format!("{tmp}{env_member}");
                        let invoke = if arglist.is_empty() {
                            format!("({code_ptr})({env_ptr})")
                        } else {
                            format!("({code_ptr})({env_ptr}, {arglist})")
                        };
                        // The callee is bound to a temporary of its own, in
                        // front of the call, so a field or a dereference is
                        // evaluated once.
                        let prelude = format!("{prelude}{} = {value};\n", callee.ty.c_decl(&tmp));
                        self.hoisted_call(prelude, invoke, &hoisted, &expr.ty)
                    }
                    None => {
                        let callee = self.expr_value(callee);
                        let call = format!("({callee})({arglist})");
                        self.hoisted_call(prelude, call, &hoisted, &expr.ty)
                    }
                }
            }
            // `[a, b, c]`: a C compound literal of the array's wrapper struct,
            // so the elements are initialized in order and the result *is* the
            // value (no temporary to copy out of).
            ExprKind::ArrayLit(elems) => {
                let Ty::Array(elem, len) = &expr.ty else {
                    unreachable!("an array literal is typed as an array")
                };
                let name = crate::types::array_c_type(elem, *len);
                let values: Vec<String> = elems.iter().map(|e| self.expr_value(e)).collect();
                format!("(({name}){{ .v = {{ {} }} }})", values.join(", "))
            }
            // `[v; N]`: the generated fill helper, one call per repeat
            // literal, so a 4096-element zero fill is not 4096 initializers in
            // the emitted C.
            ExprKind::ArrayRepeat { value, .. } => {
                let Ty::Array(elem, len) = &expr.ty else {
                    unreachable!("a repeat literal is typed as an array")
                };
                format!(
                    "{}__fill({})",
                    crate::types::array_c_type(elem, *len),
                    self.expr_value(value)
                )
            }
            ExprKind::Template { newline, pieces } => {
                // One `printf` with a generated format string: literal chunks
                // are escaped into it, each interpolated value contributes
                // its own specifier plus an argument (reusing `printf_piece`,
                // the same mapping `print` uses).
                let mut fmt = String::new();
                let mut args: Vec<String> = Vec::new();
                let mut holds: Vec<String> = Vec::new();
                let mut drops: Vec<String> = Vec::new();
                for piece in pieces {
                    match piece {
                        TemplatePiece::Text(text) => fmt.push_str(&printf_text(text)),
                        TemplatePiece::Value(value) => {
                            // An owned temporary (nothing else owns it) is
                            // held in a C temporary and freed right after the
                            // `printf`, so interpolating an allocation does
                            // not leak. A borrow of a *field* of a temporary
                            // is the same problem one projection deeper and is
                            // staged the same way, at the temporary and not at
                            // the field, so the buffers the other fields own
                            // are freed with it.
                            if matches!(value.ty, Ty::Bytes | Ty::String) {
                                let tmp = fresh_temp();
                                let rendered = self.expr_value(value);
                                holds.push(format!("{} = {rendered};", value.ty.c_decl(&tmp)));
                                let (spec, text) = self.printf_piece_with(&tmp, &value.ty);
                                fmt.push_str(spec);
                                args.push(text);
                                drops.push(format!("{}(&{tmp});", drop_fn_name(&value.ty)));
                            } else if let Some((hold, text, drop)) =
                                self.stage_borrowed_field(value)
                            {
                                holds.push(hold);
                                let (spec, rendered) = self.printf_piece_with(&text, &value.ty);
                                fmt.push_str(spec);
                                args.push(rendered);
                                if !drop.is_empty() {
                                    drops.push(drop);
                                }
                            } else {
                                let (spec, rendered) = self.printf_piece(value);
                                fmt.push_str(spec);
                                args.push(rendered);
                            }
                        }
                    }
                }
                if *newline {
                    fmt.push_str("\\n");
                }
                let call = if args.is_empty() {
                    format!("printf(\"{fmt}\")")
                } else {
                    format!("printf(\"{fmt}\", {})", args.join(", "))
                };
                if holds.is_empty() {
                    call
                } else {
                    format!(
                        "({{ {}{call}; {} }})",
                        holds.join(" "),
                        drops.join(" ")
                    )
                }
            }
            // A field read in *value* position when the chain hangs off a
            // temporary (`take(mkName().r)`, `print(mkName().r)`): nothing
            // borrows the temporary, so nothing else owns it and nothing else
            // frees it. It is staged and dropped here, at the whole temporary
            // and not at the field. A read out of a place is left alone.
            ExprKind::FieldAccess { base, field } => {
                if let Some(root) = temporary_root(expr) {
                    if let Some(staged) = self.staged_field_value(root, expr) {
                        return staged;
                    }
                }
                let read = self.field_access(base, field);
                self.clear_moved_field(expr, field, read)
            }
            // A variant construction is a compound literal in one expression:
            // the tag is the variant's index, and the payload goes into that
            // variant's union member by designator. A payload-less variant
            // needs no initializer at all — C zero-initializes what the
            // designators do not mention.
            ExprKind::EnumLit {
                enum_name,
                variant,
                fields,
            } => {
                let ty = Ty::Struct(enum_name.clone());
                let edef = self.enum_def(enum_name);
                let member = match edef {
                    Some(e) => variant_member(&e.variants[*variant].name),
                    None => unreachable!("every enum literal names a declared enum"),
                };
                // A `unit`-typed field is never a designator (`MISSING-r7Wqb`):
                // its variant's union member is the bare `char kelp_none`
                // placeholder once every field is `unit`
                // (`VariantDef::is_payload_less`), exactly like a variant
                // declared with no fields. The field's own expression is still
                // evaluated for whatever side effect it has, discarded rather
                // than stored; `Ok`/`Some`/`Err` carry one field each, so the
                // discard-before-literal order below never reorders two
                // fields' side effects against each other in practice.
                let mut discards: Vec<String> = Vec::new();
                let mut inits: Vec<String> = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let value = self.expr_value(f);
                    if f.ty == Ty::Unit {
                        discards.push(format!("(void)({value})"));
                    } else {
                        let name = match edef {
                            Some(e) => field_name(&e.variants[*variant].fields[i].name),
                            None => unreachable!("every enum literal names a declared enum"),
                        };
                        inits.push(format!(".{name} = {value}"));
                    }
                }
                let payload = if inits.is_empty() {
                    String::new()
                } else {
                    format!(", .kelp_v = {{ .{member} = {{ {} }} }}", inits.join(", "))
                };
                let literal = format!("({}){{ .kelp_tag = {variant}{payload} }}", ty.c_type());
                if discards.is_empty() {
                    literal
                } else {
                    format!("({}, {literal})", discards.join(", "))
                }
            }
            ExprKind::EnumTag { expr: base, variant } => {
                format!("({} == {variant})", self.enum_tag_access(base))
            }
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => {
                // A `unit` payload is not a C object (`VariantDef::is_payload_less`
                // gave its variant the bare placeholder member instead of a
                // field), so reading one is a no-op expression rather than a
                // reference to a field that was never declared
                // (`MISSING-r7Wqb`), the same rule `Ident` follows.
                if expr.ty == Ty::Unit {
                    return "(void)0".to_string();
                }
                // Same as a `FieldAccess`: a payload read by value out of an
                // enum temporary stages and drops the temporary.
                if let Some(root) = temporary_root(expr) {
                    if let Some(staged) = self.staged_field_value(root, expr) {
                        return staged;
                    }
                }
                // The base's own type names the enum, so nothing has to be
                // threaded through the TIR node beyond the two indices.
                let ename =
                    struct_name_of_ty(&base.ty).expect("an enum field's base is enum-typed");
                let edef = self
                    .enum_def(ename)
                    .expect("an enum field's base names a declared enum");
                let member = variant_member(&edef.variants[*variant].name);
                let fname = field_name(&edef.variants[*variant].fields[*field].name);
                let read = self.enum_member_access(base, &format!("{member}.{fname}"));
                self.clear_moved_payload(expr, *variant, *field, read)
            }
            // `expr?` (MK5-S6-M1): evaluate the operand once; if it carries
            // the failure tag, rebuild the failure in the enclosing function's
            // return type and return it; otherwise the statement-expression is
            // the live success payload. The temporary is deliberately never
            // dropped — exactly one variant's payload is moved out of it on
            // either path, and dropping the rest would free what was moved.
            ExprKind::Try {
                expr: sub,
                source,
                target,
                ok_variant,
                err_variant,
                target_err_variant,
                err_has_payload,
            } => {
                let src_def = self
                    .enum_def(source)
                    .expect("`?`'s operand names a declared enum");
                let dst_def = self
                    .enum_def(target)
                    .expect("`?`'s target names a declared enum");
                let src_def = src_def.clone();
                let dst_def = dst_def.clone();
                let src_ty = Ty::Struct(source.clone());
                let dst_ty = Ty::Struct(target.clone());
                // `err_has_payload` and the field lookups above are arity, not
                // type: a field typed `unit` has no struct member
                // (`VariantDef::is_payload_less`, `MISSING-r7Wqb`), so a
                // variant whose one field is `unit` is read and copied as
                // payload-less rather than through a member that was never
                // declared.
                let ok_is_unit = src_def.variants[*ok_variant]
                    .fields
                    .first()
                    .is_some_and(|f| f.ty == Ty::Unit);
                let err_is_unit = src_def.variants[*err_variant]
                    .fields
                    .first()
                    .is_some_and(|f| f.ty == Ty::Unit);
                let ok_member = variant_member(&src_def.variants[*ok_variant].name);
                let ok_field = field_name(&src_def.variants[*ok_variant].fields[0].name);
                let err_member = variant_member(&src_def.variants[*err_variant].name);
                let dst_member = variant_member(&dst_def.variants[*target_err_variant].name);
                let tmp = fresh_temp();
                let ret = fresh_temp();
                let mut out = String::new();
                writeln!(out, "({{").unwrap();
                writeln!(
                    out,
                    "{} {} = {};",
                    src_ty.c_type(),
                    tmp,
                    self.expr_value(sub)
                )
                .unwrap();
                writeln!(out, "if ({} == {}) {{", format!("{tmp}.kelp_tag"), err_variant).unwrap();
                writeln!(out, "    {} {} = ({}){{0}};", dst_ty.c_type(), ret, dst_ty.c_type()).unwrap();
                writeln!(out, "    {}.kelp_tag = {};", ret, target_err_variant).unwrap();
                if *err_has_payload && !err_is_unit {
                    let src_field =
                        field_name(&src_def.variants[*err_variant].fields[0].name);
                    let dst_field =
                        field_name(&dst_def.variants[*target_err_variant].fields[0].name);
                    writeln!(
                        out,
                        "    {}.kelp_v.{}.{} = {}.kelp_v.{}.{};",
                        ret, dst_member, dst_field, tmp, err_member, src_field
                    )
                    .unwrap();
                }
                // `?`'s failure arm returns from the function just as a
                // written `return` does, so it frees whatever the frame still
                // owns (borrowck's `DropPlan::at_try`) *after* the failure
                // payload has been moved into `ret`, which is the order
                // `Stmt::Return` uses. Skipping these drops leaked the frame
                // on every propagated failure.
                let drops = self.drops_at_try(expr.span);
                for t in &drops {
                    let target = self.drop_name(t);
                    writeln!(out, "    {}", drop_call(t, &target)).unwrap();
                }
                writeln!(out, "    return {};", ret).unwrap();
                writeln!(out, "}}").unwrap();
                if ok_is_unit {
                    writeln!(out, "(void)0;").unwrap();
                } else {
                    writeln!(out, "{}.kelp_v.{}.{};", tmp, ok_member, ok_field).unwrap();
                }
                out.push_str("})");
                out
            }
            ExprKind::Ref { expr: sub, .. } => {
                // A `&str` borrow is the same C value as the `str` it points
                // at (both `const char*`), so no `&` is emitted for it. Every
                // other pointee is represented by its address.
                if matches!(&expr.ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str)) {
                    return self.expr_value(sub);
                }
                let inner = self.expr_value(sub);
                if is_place(sub) {
                    return format!("(&{inner})");
                }
                // A shared borrow of a *temporary* (`str.join(str.split(s,
                // sep), sep)`) has no address to take, so the temporary is
                // hoisted into a C variable first. Its lifetime is the
                // enclosing expression — the same duration the borrow has,
                // which is why `infer.rs` only allows this for a shared
                // borrow of something that is not itself returned.
                let tmp = fresh_temp();
                format!("({{ {} = {inner}; &{tmp}; }})", sub.ty.c_decl(&tmp))
            }
            ExprKind::Deref { expr: sub } => {
                // `*p` on a `&str` view is the view itself: a `str` is
                // already the pointer.
                if expr.ty == Ty::Str {
                    return self.expr_value(sub);
                }
                let inner = self.expr_value(sub);
                format!("(*{inner})")
            }
            ExprKind::StructLit { name, fields } => {
                let mut s = format!("(({}){{ ", Ty::Struct(name.clone()).c_type());
                for (i, (fname, fval)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    write!(s, ".{} = {}", field_name(fname), self.expr_value(fval)).unwrap();
                }
                s.push_str(" })");
                s
            }
            ExprKind::Assign { target, value } => {
                // A `unit` target stores nothing (it has no C object), so the
                // assignment is the value expression and no store.
                if target.ty == Ty::Unit {
                    return format!("({{ {}; (void)0; }})", self.expr_value(value));
                }
                let target_ty = target.ty.clone();
                let target_text = self.lvalue(target);
                let value_text = self.expr_value(value);
                let drops = self.drops.at_assign(&self.function, expr.span).to_vec();
                if drops.is_empty() {
                    return format!("({target_text} = {value_text})");
                }
                // The old value has to go, but only after the new value has
                // been read (it may borrow from the binding being
                // overwritten) and before it is stored.
                let tmp = fresh_temp();
                let mut out = String::new();
                writeln!(out, "{} = {value_text};", target_ty.c_decl(&tmp)).unwrap();
                for d in &drops {
                    // An `Ident` target's drop names a binding `drop_name`
                    // resolves the usual way. A `FieldAccess`/`Deref` target
                    // (`h.name = ...`, `*p = ...`) has no binding at all —
                    // `borrowck.rs` records `binding: None` for exactly this
                    // shape (roadmap open item 15, cases 2 and 3) — so the
                    // place to free is the target's own lvalue, read again:
                    // nothing between the two evaluations writes it, so a
                    // second render of the same place is the old value.
                    let old = match &target.kind {
                        ExprKind::Ident(_) => self.drop_name(d),
                        _ => target_text.clone(),
                    };
                    writeln!(out, "{}", drop_call(d, &old)).unwrap();
                }
                writeln!(out, "{target_text} = {tmp};").unwrap();
                format!("({{\n{out}}})")
            }
            ExprKind::Block(block) => {
                let mut g = self.sub();
                g.write_block_value(block, 1);
                format!("({{\n{}}})", g.out)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                // GNU statement-expressions only yield a value from a
                // trailing *expression statement*, never from control flow —
                // `if`/`else` has no value of its own in C. So an `if` used
                // here (e.g. the RHS of a `let`) is lowered via a temporary
                // that each branch assigns into, with the temporary itself as
                // the trailing value.
                let mut g = self.sub();
                let cond = self.cond_text(cond);
                if expr.ty == Ty::Unit {
                    writeln!(g.out, "if {cond} {{").unwrap();
                    g.write_block_body(then_block, 1);
                    g.out.push('}');
                    g.write_else(else_block.as_ref(), 0);
                    g.out.push('\n');
                } else {
                    let tmp = fresh_temp();
                    writeln!(g.out, "{};", expr.ty.c_decl(&tmp)).unwrap();
                    writeln!(g.out, "if {cond} {{").unwrap();
                    g.write_block_assign(then_block, &tmp, 1);
                    g.out.push('}');
                    g.write_else_assign(else_block.as_ref(), &tmp, 0);
                    g.out.push('\n');
                    writeln!(g.out, "{tmp};").unwrap();
                }
                format!("({{\n{}}})", g.out)
            }
        }
    }

    /// Like `write_block_body`, but a present tail expression is emitted as
    /// the statement-expression's yielded value (its value is *not*
    /// discarded). When the block also has end-of-scope drops, the tail goes
    /// into a temporary first: the value of a GNU statement expression comes
    /// from its *last* statement, and a drop call is void.
    fn write_block_value(&mut self, block: &Block, depth: usize) {
        self.push_locals();
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        let drops = self.drops_at_block_end(block);
        match (&block.tail, drops.is_empty()) {
            (Some(tail), true) => {
                let value = self.expr_value(tail);
                indent(&mut self.out, depth);
                writeln!(self.out, "{value};").unwrap();
            }
            (Some(tail), false) => {
                let tmp = self.write_temp_then_drops(tail, &drops, depth);
                indent(&mut self.out, depth);
                writeln!(self.out, "{tmp};").unwrap();
            }
            (None, _) => self.write_drops(&drops, depth),
        }
        self.pop_locals();
    }

    /// Writes a block whose tail expression (if any) is assigned into `tmp`
    /// instead of being yielded as a statement-expression value — used for
    /// the branches of an `if` that's itself being lowered via a temporary.
    fn write_block_assign(&mut self, block: &Block, tmp: &str, depth: usize) {
        self.push_locals();
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        if let Some(tail) = &block.tail {
            let value = self.expr_value(tail);
            indent(&mut self.out, depth);
            writeln!(self.out, "{tmp} = {value};").unwrap();
        }
        let drops = self.drops_at_block_end(block);
        self.write_drops(&drops, depth);
        self.pop_locals();
    }

    fn write_else_assign(&mut self, else_block: Option<&Block>, tmp: &str, depth: usize) {
        let Some(else_block) = else_block else {
            return;
        };
        if else_block.stmts.is_empty() {
            if let Some(tail) = &else_block.tail {
                if let ExprKind::If {
                    cond,
                    then_block,
                    else_block: inner_else,
                } = &tail.kind
                {
                    let cond = self.expr_value(cond);
                    write!(self.out, " else if ({cond}) {{\n").unwrap();
                    self.write_block_assign(then_block, tmp, depth + 1);
                    indent(&mut self.out, depth);
                    self.out.push('}');
                    self.write_else_assign(inner_else.as_ref(), tmp, depth);
                    return;
                }
            }
        }
        self.out.push_str(" else {\n");
        self.write_block_assign(else_block, tmp, depth + 1);
        indent(&mut self.out, depth);
        self.out.push('}');
    }

    /// The gated-call shape for one call site: the domain, its pattern count,
    /// the call site as the author wrote it, and which arguments are resource
    /// names. `None` when the call is not gated, the calling module holds no
    /// grant for it, or the call names no resource at all.
    ///
    /// `call_value` and `cap_checked_call` both read this one answer, so the
    /// argument the grant is checked against and the argument the call passes
    /// cannot disagree.
    fn cap_plan(
        &self,
        callee: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<(String, usize, String, Vec<crate::stdlib::CapCheck>)> {
        use crate::stdlib::CapCheck;

        let domain = self.caps.gated.get(callee)?;
        let patterns = self.caps.patterns_for(&self.module, domain)?;
        let arg_is_str: Vec<bool> =
            args.iter().map(|a| crate::stdlib::is_str_like_ty(&a.ty)).collect();
        // The plan is built from the call's key, which for an imported module
        // is qualified (`std.net.connect`); the per-function table in
        // `stdlib.rs` is keyed on the bare name the author wrote.
        let bare = callee.rsplit('.').next().unwrap_or(callee);
        let mut checked: Vec<CapCheck> = crate::stdlib::checked_args(domain, bare, &arg_is_str);
        // A path domain's whole-resource pattern (`/**`) matches any string
        // unconditionally (see `glob_match` in `std/rt.kp`, the `**` branch's
        // `if rest == plen { return true }` fast path). When the calling
        // module's own compiled-in pattern list for this domain holds that
        // literal pattern, a `CapCheck::Path` at this call site is provably
        // going to pass no matter what the argument is, so there is nothing
        // for the runtime check to decide: drop it before it is ever
        // generated. This is NOT done for name domains (`env`, `net`), whose
        // whole-resource pattern is a single `*`: unlike `/**`, a lone `*`
        // does not cross a `/` byte in `glob_match`, so it is not a true
        // unconditional match and this rule never touches `CapCheck::Name`
        // (the name-domain elision below has its own, stricter condition).
        if crate::stdlib::patterns_are_paths(domain)
            && patterns.iter().any(|p| p == crate::stdlib::whole_resource(domain))
        {
            checked.retain(|c| !matches!(c, CapCheck::Path(_)));
        }
        // The counterpart for a name domain (`env`): the module that declares
        // a domain's gated `extern fun` also *calls* it, from its own thin
        // wrapper (`std/env`'s `has` forwarding to `has_raw`), and when that
        // module's own compiled-in list for the domain is exactly the
        // whole-resource `*` (what `impl "std/env" caps(env = ["*"])` or
        // `caps(*)` hands it), the `CapCheck::Name` at that internal call is
        // provably going to pass whenever the caller's own check did, so
        // dropping it cannot admit a name the caller could not already reach.
        //
        // The proof is the domain's own grant vocabulary: an `env` pattern
        // may not contain a `/` (`resolve.rs::validate_cap_pattern` refuses
        // one), and `*` crosses no `/`, so no caller's grant admits a slashy
        // name either. Only a *whole-resource* list qualifies: a narrower one
        // (`caps(env = ["PATH"])`) is the effective gate for what the domain
        // module may read, and eliding that would let any caller of the domain
        // module name everything its own `*` covers. A caller that is not the
        // domain's defining module is never elided, so an ordinary holder of
        // `*` keeps its own check and its own slashy-name denial.
        if !crate::stdlib::patterns_are_paths(domain)
            && crate::stdlib::domain_of(&self.module) == Some(domain.as_str())
            && patterns.len() == 1
            && patterns[0] == crate::stdlib::whole_resource(domain)
        {
            checked.retain(|c| !matches!(c, CapCheck::Name(_)));
        }
        if checked.is_empty() {
            return None;
        }
        // The call site, as the author wrote it. The generated C carries a
        // `#line` marker per statement now (`write_line_marker`), so this is no
        // longer the only way a denial can name a `.kp` location: it is the
        // way that also names the *column*, and the file path it prints is the
        // one the compiler was invoked with rather than whatever the C
        // preprocessor reports for a `#line`.
        let at = self
            .sources
            .locate(&self.module, span)
            .unwrap_or_else(|| self.module.clone());
        Some((domain.clone(), patterns.len(), at, checked))
    }

    /// A gated call (M7): every resource argument is checked against the
    /// calling module's compiled-in allowlist *before* the call runs, and the
    /// whole thing is a statement expression so it works in value position
    /// (`val b = fs.read(p)`) as well as statement position.
    ///
    /// The check reads `args_text`, the same argument texts the call passes,
    /// never a fresh render of the argument. An argument whose expression has
    /// an effect is hoisted into a temporary exactly once, so the value the
    /// grant admits is the value the call uses.
    fn cap_checked_call(
        &self,
        plan: Option<(String, usize, String, Vec<crate::stdlib::CapCheck>)>,
        call: String,
        args_text: &[String],
    ) -> String {
        use crate::stdlib::CapCheck;

        let Some((domain, pattern_count, at, checked)) = plan else {
            return call;
        };
        let symbol = CapTable::symbol(&self.module, &domain);
        // The `at` location, the module name and the domain all go into a C
        // string literal, and a source path may hold a quote or a backslash:
        // the quote would close the literal early and the backslash would open
        // an escape the program never wrote. There is one escaper for a C
        // string body in this crate (`diag::c_string_body`, the `#line` one),
        // so they go through that rather than the raw text.
        let at = crate::diag::c_string_body(&at);
        let module = crate::diag::c_string_body(&self.module);
        let domain = crate::diag::c_string_body(&domain);
        let mut checks = String::new();
        for check in checked {
            match check {
                CapCheck::Path(i) => {
                    // A path is normalized lexically, then resolved through
                    // symlinks, and only then matched.
                    let path = &args_text[i];
                    write!(
                        checks,
                        "kelp_cap_check_path(\"{domain}\", {path}, {symbol}, {pattern_count}, \"{module}\", \"{at}\"); "
                    )
                    .unwrap();
                }
                CapCheck::Name(i) => {
                    let name = &args_text[i];
                    write!(
                        checks,
                        "kelp_cap_check_name(\"{domain}\", {name}, {symbol}, {pattern_count}, \"{module}\", \"{at}\"); "
                    )
                    .unwrap();
                }
                CapCheck::Host(i) => {
                    let host = &args_text[i];
                    write!(
                        checks,
                        "kelp_cap_check_host(\"{domain}\", {host}, {symbol}, {pattern_count}, \"{module}\", \"{at}\"); "
                    )
                    .unwrap();
                }
                CapCheck::HostPort(host_at, port_at) => {
                    let host = &args_text[host_at];
                    let port = &args_text[port_at];
                    write!(
                        checks,
                        "kelp_cap_check_hostport(\"{domain}\", {host}, {port}, {symbol}, {pattern_count}, \"{module}\", \"{at}\"); "
                    )
                    .unwrap();
                }
                CapCheck::Port(i) => {
                    let port = &args_text[i];
                    write!(
                        checks,
                        "kelp_cap_check_port(\"{domain}\", {port}, {symbol}, {pattern_count}, \"{module}\", \"{at}\"); "
                    )
                    .unwrap();
                }
            }
        }
        format!("({{ {checks}{call}; }})")
    }

    /// One atomic operation on an `Atomic<T>` (MK9-S3-M1 stage A1), as the
    /// `__atomic_*` builtin that *is* the operation: `__atomic_load_n`,
    /// `__atomic_store_n`, `__atomic_exchange_n`, `__atomic_fetch_*` and
    /// `__atomic_compare_exchange_n`, every one of them at
    /// `__ATOMIC_SEQ_CST`, the only ordering v1 states.
    ///
    /// The member is a plain `T`, so atomicity lives entirely in these
    /// builtins: nothing here reads or writes `->v` outside one, which is why
    /// a plain load is not a thing this emitter can produce for this type.
    /// `compare_exchange` is one `__atomic_compare_exchange_n` plus the
    /// `Result<T, T>` MK9 4 asks for: `__prev` starts as the compared value, and
    /// the builtin leaves it alone on success (so it is the value that was
    /// there) and overwrites it with the value actually found on failure, so
    /// neither arm needs a second load and no window is lost between them.
    fn atomic_call(&self, op: &str, args: &[Expr], ret: &Ty) -> String {
        // `Atomic.new(v)` is the one operation with no place: it builds the
        // cell, and its only argument is the value.
        if op == "new" {
            let elem = crate::types::atomic_elem_of(ret)
                .expect("Atomic.new's own type names its element");
            return format!(
                "({}){{ .v = ({}) ({}) }}",
                ret.c_type(),
                elem.c_type(),
                self.expr_value(&args[0])
            );
        }
        // The element type decides two spellings: the pointer the `__atomic_*`
        // builtins take, and the cast on each value argument, so a `u8`
        // operation is a `u8` operation and not the `int` a C integer literal
        // would otherwise make of it.
        let elem = crate::types::atomic_elem_of(&args[0].ty)
            .expect("an atomic operation's receiver names its element")
            .c_type();
        // `->v` on the receiver's own address, cast to a plain `T*`. The cast is
        // the type system read back: a shared borrow of an `Atomic<T>` says
        // nothing about immutability, because an atomic mutates *through* a
        // shared borrow (that is what leaves it no `&mut` path), while a `pd
        // val`'s hidden borrow really is `const T*` in C. Without the cast every
        // writing builtin refuses the const pointer, which is exactly the seam
        // where C's qualifier and Kelp's borrow mode mean different things.
        let store = format!("(({elem}*)&({})->v)", self.expr_value(&args[0]));
        let operand = |i: usize| format!("({elem})({})", self.expr_value(&args[i]));
        match op {
            "load" => format!("__atomic_load_n({store}, __ATOMIC_SEQ_CST)"),
            "store" => format!("__atomic_store_n({store}, {}, __ATOMIC_SEQ_CST)", operand(1)),
            "swap" => format!(
                "__atomic_exchange_n({store}, {}, __ATOMIC_SEQ_CST)",
                operand(1)
            ),
            "fetch_add" | "fetch_sub" | "fetch_and" | "fetch_or" | "fetch_xor" => format!(
                "__atomic_{op}({store}, {}, __ATOMIC_SEQ_CST)",
                operand(1)
            ),
            "compare_exchange" => {
                let name =
                    struct_name_of_ty(ret).expect("a compare_exchange's result is the Result enum");
                let edef = self
                    .enum_def(name)
                    .expect("a compare_exchange's Result instance is declared");
                // Recognized by variant name, never by the enum's own name:
                // the same rule `?` uses, so a `std/result` replaced by any
                // two-variant `Ok`/`Err` shape still lowers.
                let arm = |variant: &str| {
                    let index = edef
                        .variants
                        .iter()
                        .position(|v| v.name == variant)
                        .expect("the Result shape has one arm per variant");
                    let v = &edef.variants[index];
                    let member = variant_member(&v.name);
                    let field = field_name(&v.fields[0].name);
                    format!(
                        "({}){{ .kelp_tag = {index}, .kelp_v = {{ .{member} = {{ .{field} = __prev }} }} }}",
                        ret.c_type()
                    )
                };
                format!(
                    "({{ {elem} __prev = {}; __atomic_compare_exchange_n({store}, &__prev, {}, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST) ? {} : {}; }})",
                    operand(1),
                    operand(2),
                    arm("Ok"),
                    arm("Err")
                )
            }
            other => unreachable!("no C spelling for the atomic operation '{other}'"),
        }
    }

    /// `thread.spawn(entry, arg)` (MK9-S3-M1 stage C1), as C: allocate the
    /// handle and its two slots, move the argument into the argument slot, and
    /// start the worker with the generated body for this signature. The
    /// handle's element is the entry's *return* type, which is the value the
    /// worker hands its joiner, so `sizeof` that type is the result slot.
    fn thread_spawn_value(&self, entry: &Expr, arg: &Expr, ret: &Ty) -> String {
        let Ty::Closure { params, .. } = &entry.ty else {
            unreachable!("thread.spawn's entry is a closure by the checker's coercion")
        };
        let Ty::Thread(elem) = ret else {
            unreachable!("thread.spawn hands back a Thread<T> handle")
        };
        let arg_c = params[0].c_type();
        let handle = fresh_temp();
        let closure = fresh_temp();
        // A worker that returns `unit` writes nothing: there is no result value,
        // so the slot is a single byte nothing reads. The runtime always
        // allocates one, which is why the size is 1 rather than 0.
        let result_size = if **elem == Ty::Unit {
            "1".to_string()
        } else {
            format!("sizeof({})", elem.c_type())
        };
        format!(
            "({{ {closure_c} {closure} = {entry}; kelp_Thread* {handle} = kelp_thread_new(sizeof({arg_c}), {result_size}); *({arg_c}*){handle}->arg = {arg}; kelp_thread_start({handle}, {body}, (void*){closure}.fn, {closure}.env, {closure}.drop); {handle}; }})",
            closure_c = entry.ty.c_type(),
            entry = self.expr_value(entry),
            arg = self.expr_value(arg),
            body = thread_body_name(&entry.ty),
        )
    }

    /// `thread.join(handle)` (MK9-S3-M1 stage C1), as C: wait for the worker,
    /// then build `Result<T, ThreadErr>`: `Ok` the value out of the result slot,
    /// or `Err` carrying the message the worker panicked with. The result slot
    /// is read only on the success path, because a worker that panicked never
    /// wrote it; the handle is freed after the value is out of it.
    fn thread_join_value(&self, handle: &Expr, ret: &Ty) -> String {
        let Ty::Struct(name) = ret else {
            unreachable!("thread.join hands back a Result")
        };
        let edef = self
            .enum_def(name)
            .expect("a join's Result instance is declared");
        // Recognized by variant name, never by the enum's own name: the same
        // rule `compare_exchange`'s arm and `?` use.
        let index_of = |variant: &str| {
            edef.variants
                .iter()
                .position(|v| v.name == variant)
                .expect("the Result shape has one arm per variant")
        };
        let ok = index_of("Ok");
        let err = index_of("Err");
        let member = |i: usize| variant_member(&edef.variants[i].name);
        let field = |i: usize| field_name(&edef.variants[i].fields[0].name);
        let Ty::Thread(elem) = &handle.ty else {
            unreachable!("thread.join's argument is a Thread<T> handle")
        };
        let err_ty = crate::infer::thread_err_ty();
        let h = fresh_temp();
        let result = fresh_temp();
        // A worker that returns `unit` wrote nothing to its result slot
        // (`thread_spawn_value`), and the `Ok` variant's field is `unit`
        // too, so it is the bare `char kelp_none` placeholder
        // (`VariantDef::is_payload_less`) rather than a struct member: the
        // success arm sets only the tag (`MISSING-r7Wqb`).
        let ok_arm = if **elem == Ty::Unit {
            format!("{result}.kelp_tag = {ok};")
        } else {
            let elem_c = elem.c_type();
            let value = fresh_temp();
            format!(
                "{elem_c} {value} = *({elem_c}*){h}->result; {result}.kelp_tag = {ok}; {result}.kelp_v.{ok_member}.{ok_field} = {value};",
                ok_member = member(ok),
                ok_field = field(ok),
            )
        };
        format!(
            "({{ {handle_c} {h} = {handle}; kelp_thread_wait({h}); {result_c} {result}; if ({h}->panicked) {{ {result}.kelp_tag = {err}; {result}.kelp_v.{err_member}.{err_field} = (({err_c}){{ .{err_msg} = kelp_String__from_str({h}->message) }}); }} else {{ {ok_arm} }} kelp_thread_dispose({h}); {result}; }})",
            handle_c = handle.ty.c_type(),
            handle = self.expr_value(handle),
            result_c = ret.c_type(),
            err_member = member(err),
            err_field = field(err),
            err_c = err_ty.c_type(),
            err_msg = field_name(crate::stdlib::THREAD_ERR_MESSAGE_FIELD),
        )
    }

    fn call_value(&self, callee: &str, args: &[Expr], span: Span, ret: &Ty) -> String {
        // An atomic operation (MK9-S3-M1 stage A1): one `__atomic_*` builtin,
        // never a plain read or write. The ordering argument is deliberately
        // not rendered: v1 states exactly one ordering, so the builtin's order
        // is spelled `__ATOMIC_SEQ_CST` here rather than threaded through.
        if let Some(op) = callee.strip_prefix("Atomic#") {
            return self.atomic_call(op, args, ret);
        }
        // `thread.fence` (MK9-S3-M1 stage A2): the standalone fence is one
        // `__atomic_thread_fence` builtin, so it needs no place, no cell and no
        // generated function. The ordering argument is deliberately not
        // rendered, for the reason the atomic operations above give: v1 states
        // exactly one ordering, so `__ATOMIC_SEQ_CST` is the whole of what the
        // argument says.
        if callee == crate::stdlib::FENCE_FN {
            return "__atomic_thread_fence(__ATOMIC_SEQ_CST)".to_string();
        }
        // `thread.spawn` and `thread.join` (MK9-S3-M1 stage C1): the runtime
        // owns the thread and the panic policy, and each call site is a small
        // statement expression around it plus, for a spawn, the generated
        // worker body for that signature.
        if callee == crate::stdlib::SPAWN_FN {
            return self.thread_spawn_value(&args[0], &args[1], ret);
        }
        if callee == crate::stdlib::JOIN_FN {
            return self.thread_join_value(&args[0], ret);
        }
        // `n.to_u8()` and friends (S1): a numeric conversion, emitted inline
        // as a cast — with a range check when the target is an integer that
        // can lose the value — rather than a C function per pair.
        if let Some((src, dst)) = callee.split_once("#to_") {
            if let (Some(from), Some(to)) = (Ty::from_name(src), Ty::from_name(dst)) {
                if from.is_integer() && (to.is_integer() || to.is_float()) {
                    let value = self.expr_value(&args[0]);
                    return numeric_conversion(callee, &from, &to, &value);
                }
            }
        }
        // `panic` reaches `kelp_panic`, whose one argument is the
        // `const char*` message. A `str` is already that pointer, but an owned
        // `String` message (MISSING-8) is a struct, so the buffer's data
        // pointer is what goes through, and for a temporary nothing else
        // owns, the struct is hoisted the way `print` hoists one. The drop
        // after the panic would be unreachable, so the hoist is only about
        // having a place to read `data` from.
        if callee == "panic" {
            let arg = &args[0];
            match &arg.ty {
                Ty::String => {
                    let tmp = fresh_temp();
                    let value = self.expr_value(arg);
                    return format!(
                        "({{ {} = {value}; kelp_panic((const char*)({tmp}).data); }})",
                        arg.ty.c_decl(&tmp)
                    );
                }
                Ty::Ref { inner, .. } if matches!(**inner, Ty::String) => {
                    // A message read out of a field of a temporary is the same
                    // staging as `print`'s, minus the free: a drop after a
                    // panic is unreachable.
                    if let Some((hold, text, _)) = self.stage_borrowed_field(arg) {
                        return format!("({{ {hold} kelp_panic((const char*)({text})->data); }})");
                    }
                    let value = self.expr_value(arg);
                    return format!("kelp_panic((const char*)({value})->data)");
                }
                _ => {}
            }
        }
        if callee == "print" || callee == "print!" {
            let arg = &args[0];
            // `print` appends a newline, `print!` doesn't — opposite of the
            // old `println`/`print` naming.
            let newline = if callee == "print" { "\\n" } else { "" };
            // An owned buffer that arrives *unwrapped* is a temporary nothing
            // else owns (`infer.rs` wraps a named place in `Ref`), so it is
            // hoisted into a C temporary and freed after the `printf`:
            // `print(str.to_upper(s))` allocates once, prints, frees.
            if matches!(arg.ty, Ty::Bytes | Ty::String) {
                let tmp = fresh_temp();
                let value = self.expr_value(arg);
                let (fmt, rendered) = self.printf_piece_with(&tmp, &arg.ty);
                return format!(
                    "({{ {} = {value}; printf(\"{fmt}{newline}\", {rendered}); {}(&{tmp}); }})",
                    arg.ty.c_decl(&tmp),
                    drop_fn_name(&arg.ty)
                );
            }
            // A borrow of a *field* of a temporary, as in
            // `print(mkName().name)`, is the same problem one projection
            // deeper: the whole temporary is what has to be freed, so it is
            // staged here, the `printf` reads through the field's address
            // inside it, and the free follows the `printf`.
            if let Some((hold, text, drop)) = self.stage_borrowed_field(arg) {
                let (fmt, rendered) = self.printf_piece_with(&text, &arg.ty);
                return format!("({{ {hold} printf(\"{fmt}{newline}\", {rendered}); {drop} }})");
            }
            let (fmt, value) = self.printf_piece(arg);
            return format!("printf(\"{fmt}{newline}\", {value})");
        }

        // A shared borrow of a *temporary* (`str.join(str.split(s, sep),
        // sep)`) has no address to take, so the temporary is hoisted into a C
        // variable that lives exactly as long as the call — and is freed
        // right after it, since nothing else owns it. The whole call becomes
        // one statement expression so the temporary is still alive when the
        // call runs; hoisting it into the *argument* alone would not do,
        // because a GNU statement expression's temporaries die when it ends,
        // before the call it is an argument to.
        // Which arguments a capability check reads, decided before the
        // arguments are rendered: a checked argument whose text has an effect
        // is hoisted into a temporary so the check and the call read one
        // evaluation of it.
        let plan = self.cap_plan(callee, args, span);
        let hoist: Vec<bool> = plan
            .as_ref()
            .map(|(_, _, _, checked)| checked_arg_flags(checked, args.len()))
            .unwrap_or_default();
        let (prelude, args_text, hoisted) = self.render_arguments(args, &hoist);

        let mut s = format!("{}({})", c_name(callee), args_text.join(", "));
        // Container methods are monomorphized: the C name carries the element
        // type, which comes from the receiver argument (or, for the
        // associated `List.of(v)`/`Box.of(v)`, from `v` itself, and for
        // `empty`/`Map.new()` from the call's own result type).
        for (prefix, name_of, elem_of) in [
            (
                "List#",
                crate::types::list_c_type as fn(&Ty) -> String,
                crate::types::list_elem_of as fn(&Ty) -> Option<&Ty>,
            ),
            (
                "Box#",
                crate::types::box_c_type as fn(&Ty) -> String,
                crate::types::box_elem_of as fn(&Ty) -> Option<&Ty>,
            ),
        ] {
            let Some(op) = callee.strip_prefix(prefix) else {
                continue;
            };
            let elem = if op == "of" {
                args.first().map(|a| a.ty.clone())
            } else if op == "empty" || op == "new" {
                elem_of(ret).cloned()
            } else {
                args.first().and_then(|a| elem_of(&a.ty)).cloned()
            };
            if let Some(elem) = elem {
                let base = name_of(&elem);
                // `get` has two C spellings: by value for a `Copy` element,
                // by pointer for an owned one — which one is decided by the
                // call's own result type, never by guessing.
                let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                    "get_const"
                } else {
                    op
                };
                s = format!("{base}__{suffix}({})", args_text.join(", "));
            }
        }
        if let Some(op) = callee.strip_prefix("Array#") {
            let elem = args
                .first()
                .and_then(|a| crate::types::array_elem_of(&a.ty))
                .cloned();
            if let Some(elem) = elem {
                let len = crate::types::array_len_of(&args[0].ty)
                    .expect("an Array# receiver is an array");
                let base = crate::types::array_c_type(&elem, len);
                // `get` has two C spellings: by value for a `Copy` element, by
                // pointer for an owned one, decided by the call's result type,
                // exactly as `List#get` decides it.
                let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                    "get_const"
                } else {
                    op
                };
                s = format!("{base}__{suffix}({})", args_text.join(", "));
            }
        }
        // A map's name carries *two* type arguments, so it is spelled out
        // separately: its key comes from the receiver (or, for `Map.new()`,
        // from the call's own result type) and its value from the same place.
        if let Some(op) = callee.strip_prefix("Map#") {
            let (from_ret, from_args) = if op == "new" || op == "with_capacity" {
                (
                    crate::types::map_elem_of(ret).cloned(),
                    crate::types::map_key_of(ret).cloned(),
                )
            } else {
                (
                    args.first().and_then(|a| crate::types::map_elem_of(&a.ty)).cloned(),
                    args.first().and_then(|a| crate::types::map_key_of(&a.ty)).cloned(),
                )
            };
            if let (Some(value), Some(key)) = (from_ret, from_args) {
                let base = crate::types::map_c_type(&key, &value);
                // `get` has two C spellings: by value for a `Copy` value, by
                // pointer for an owned one — decided by the call's own result
                // type, never by guessing.
                let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                    "get_const"
                } else {
                    op
                };
                s = format!("{base}__{suffix}({})", args_text.join(", "));
            }
        }
        let call = self.cap_checked_call(plan, s, &args_text);
        self.hoisted_call(prelude, call, &hoisted, ret)
    }

    /// `base.field` in C: `->` when `base` is reached through a borrow
    /// (`Ty::Ref`), `.` otherwise. Each `FieldAccess` node decides this
    /// purely from its own immediate base's type — a chain like `c.inner.x`
    /// (where `c` is a `mut Counter` parameter and `inner` is a plain
    /// embedded field) gets this right node-by-node: the innermost access
    /// sees `c`'s `Ref` type and emits `->`, the outer one sees `inner`'s
    /// plain `Struct` type and emits `.`, with no extra state threaded
    /// through.
    /// `<base>.kelp_tag`, or `<base>->kelp_tag` when the base is itself a
    /// borrow — the same rule `field_access` uses for a datac field.
    fn enum_tag_access(&self, base: &Expr) -> String {
        let base_text = self.expr_value(base);
        if matches!(base.ty, Ty::Ref { .. }) {
            format!("({base_text})->kelp_tag")
        } else {
            format!("({base_text}).kelp_tag")
        }
    }

    /// One payload of one variant: `<base>.kelp_v.kelp_v_Circle.kelp_r`, or
    /// the `->` form for a borrowed base.
    fn enum_member_access(&self, base: &Expr, member: &str) -> String {
        member_access(&self.expr_value(base), &base.ty, &format!("kelp_v.{member}"))
    }

    fn field_access(&self, base: &Expr, field: &str) -> String {
        member_access(&self.expr_value(base), &base.ty, &field_name(field))
    }

    /// A partial move copies the projection out of `place` and then zeroes the
    /// slot it came from, so the binding's own drop frees the projections the
    /// move left behind and nothing where the value now lives. `place` is the
    /// C lvalue the projection resolves to, so the zeroing and the read are the
    /// same expression; the value has to go through a temporary because the
    /// zeroing is a second statement (`codegen_llvm.rs::emit_field_read` stores
    /// a null there instead). `field` is the field this read copies out, which
    /// the recorded projection has to name.
    fn clear_moved_field(&self, expr: &Expr, field: &str, place: String) -> String {
        match self.drops.clear_at(&self.function, expr) {
            // Not a partial move: the read copies nothing out, so the slot
            // keeps its value and the binding's own drop frees it.
            None => return place,
            // The field this read copied out: zero the slot below.
            Some(Projection::Field(moved)) if moved == field => {}
            // A projection the backend has no clear for. Zeroing a slot that
            // still holds its value, or skipping the zero of one that does
            // not, frees the same buffer twice, so an arm per variant is what
            // has to be filled in rather than passed over.
            Some(Projection::Field(moved)) => panic!(
                "codegen_c: a move of field '{moved}' was recorded at a read of '{field}'"
            ),
            Some(Projection::EnumPayload { name, .. }) => panic!(
                "codegen_c: a move of payload '{name}' was recorded at a read of field '{field}'"
            ),
        }
        zeroed_read(place, &expr.ty)
    }

    /// The same clear for one payload of one variant, read out by value (A-11):
    /// `variant` and `field` are the indices of the `tir` node being rendered,
    /// which are the identity the recorded projection has to match.
    fn clear_moved_payload(
        &self,
        expr: &Expr,
        variant: usize,
        field: usize,
        place: String,
    ) -> String {
        match self.drops.clear_at(&self.function, expr) {
            None => return place,
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) if *moved_variant == variant && *moved_field == field => {}
            // A projection the backend has no clear for: an arm per variant is
            // what has to be filled in rather than passed over (see
            // `clear_moved_field`).
            Some(Projection::Field(moved)) => panic!(
                "codegen_c: a move of field '{moved}' was recorded at a read of payload {variant}.{field}"
            ),
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) => panic!(
                "codegen_c: a move of payload {moved_variant}.{moved_field} was recorded at a read of payload {variant}.{field}"
            ),
        }
        zeroed_read(place, &expr.ty)
    }

    /// The C text of the field chain `expr` read out of the temporary its root
    /// was staged into: `chain_off("_xtmp0", mkName().name)` is
    /// `((_xtmp0).kelp_name)`. `expr` is the `FieldAccess`/`EnumField` chain
    /// `temporary_root` answers `Some` for, and `root_text` replaces the base
    /// expression the chain is rooted at, which is the rvalue C refuses to
    /// project through and the whole reason the root was staged.
    fn chain_off(&self, root_text: &str, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::FieldAccess { base, field } => {
                let base_text = self.chain_off(root_text, base);
                member_access(&base_text, &base.ty, &field_name(field))
            }
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => {
                let base_text = self.chain_off(root_text, base);
                let ename =
                    struct_name_of_ty(&base.ty).expect("an enum field's base is enum-typed");
                let edef = self
                    .enum_def(ename)
                    .expect("an enum field's base names a declared enum");
                let member = format!(
                    "{}.{}",
                    variant_member(&edef.variants[*variant].name),
                    field_name(&edef.variants[*variant].fields[*field].name)
                );
                member_access(&base_text, &base.ty, &format!("kelp_v.{member}"))
            }
            _ => root_text.to_string(),
        }
    }

    /// One call argument, with whatever storage its borrow needs staged in
    /// front of the call and the owned temporaries that storage handed over.
    ///
    /// A shared borrow of a temporary has no address to take: `str.join(parts
    /// = str.split(s, sep), sep)` borrows a temporary `List`, and
    /// `take(mkName().name)` borrows a field *of* a temporary. Neither can be
    /// staged inside the argument itself, because a GNU statement expression's
    /// own variables die when it ends, before the call the argument belongs
    /// to. So the declaration comes back as a prelude for the caller to put at the
    /// front of the statement expression that holds the call, and the
    /// temporary comes back in `owned` for the caller to free after it. A
    /// field chain is staged at its *root*: freeing the field alone would
    /// leave the rest of the temporary, and every buffer the other fields own,
    /// unfreed.
    ///
    /// Returns `(prelude, argument text, owned temporaries)`. An argument that
    /// needs nothing comes back as its own C text with an empty prelude.
    ///
    /// `hoist` forces an rvalue argument into a temporary (see
    /// `checked_arg_needs_temp`): the text that comes back is the temporary,
    /// so the caller reads one evaluation of the argument wherever it uses the
    /// text. Nothing drops that temporary: the callee takes the value itself,
    /// exactly as it did when the argument was the expression.
    fn stage_argument(&self, arg: &Expr, hoist: bool) -> (String, String, Vec<(String, Ty)>) {
        let ExprKind::Ref { expr: sub, .. } = &arg.kind else {
            if hoist {
                let tmp = fresh_temp();
                let prelude = format!("{} = {};\n", arg.ty.c_decl(&tmp), self.expr_value(arg));
                return (prelude, tmp, Vec::new());
            }
            return (String::new(), self.expr_value(arg), Vec::new());
        };
        if let Some(root) = temporary_root(sub) {
            let tmp = fresh_temp();
            let prelude = format!("{} = {};\n", root.ty.c_decl(&tmp), self.expr_value(root));
            let chain = self.chain_off(&tmp, sub);
            let text = if is_str_borrow(&arg.ty) {
                chain
            } else {
                format!("&{chain}")
            };
            return (prelude, text, vec![(tmp, root.ty.clone())]);
        }
        // A `&str` borrow is the same C value as the `str` it points at (both
        // `const char*`), so no `&` is emitted for it, and a place already has
        // the address the callee wants.
        if is_str_borrow(&arg.ty) || is_place(sub) {
            return (String::new(), self.expr_value(arg), Vec::new());
        }
        let tmp = fresh_temp();
        let prelude = format!("{} = {};\n", sub.ty.c_decl(&tmp), self.expr_value(sub));
        (prelude, format!("&{tmp}"), vec![(tmp, sub.ty.clone())])
    }

    /// A call's arguments as C text, with each one's staged storage in the
    /// prelude and its owned temporaries in the third slot; see
    /// `stage_argument`. `hoist` is per argument, `false` for any entry it does
    /// not cover.
    fn render_arguments(
        &self,
        args: &[Expr],
        hoist: &[bool],
    ) -> (String, Vec<String>, Vec<(String, Ty)>) {
        let mut prelude = String::new();
        let mut texts: Vec<String> = Vec::new();
        let mut staged: Vec<(String, Ty)> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            // A `unit` argument crosses as nothing: the callee's own signature
            // has no parameter for it (`write_params` leaves it out), so the
            // argument is evaluated for whatever it does and no C value is
            // passed.
            if a.ty == Ty::Unit {
                prelude.push_str(&format!("{};\n", self.expr_value(a)));
                continue;
            }
            let want = hoist.get(i).copied().unwrap_or(false) && checked_arg_needs_temp(a);
            let (decls, text, owned) = self.stage_argument(a, want);
            prelude.push_str(&decls);
            texts.push(text);
            staged.extend(owned);
        }
        (prelude, texts, staged)
    }

    /// The same staging as `stage_argument` for a borrow that is *not* a call
    /// argument: a `print` or template operand, or a `panic` message. There
    /// is no call for a prelude to sit in front of, so the declaration and the
    /// drop come back as the two halves of the statement expression the
    /// operand's own lowering already builds.
    ///
    /// Returns `(declaration, borrow text, drop)`, or `None` when the operand
    /// is not a borrow of a field of a temporary.
    fn stage_borrowed_field(&self, value: &Expr) -> Option<(String, String, String)> {
        let ExprKind::Ref { expr: sub, .. } = &value.kind else {
            return None;
        };
        let root = temporary_root(sub)?;
        let tmp = fresh_temp();
        let hold = format!("{} = {};", root.ty.c_decl(&tmp), self.expr_value(root));
        let chain = self.chain_off(&tmp, sub);
        let text = if is_str_borrow(&value.ty) {
            chain
        } else {
            format!("&{chain}")
        };
        // A `datac` that owns nothing has no drop routine to call, and nothing
        // to free either: the staging is only about having the address.
        let drop = if needs_drop(&root.ty, self.owned) {
            format!("{}(&{tmp});", drop_fn_name(&root.ty))
        } else {
            String::new()
        };
        Some((hold, text, drop))
    }

    /// A field read in *value* position when the chain hangs off a
    /// temporary: `take(mkName().r)` or `print(mkName().r)`. Nothing borrows
    /// the temporary, so nothing else owns it and nothing else frees it, and
    /// the C rvalue the field is read out of has no storage to hang a drop on.
    /// The root is staged, the field read out of it, the staged root dropped,
    /// and the value handed on, so the temporary and every field it owns die
    /// at the end of the statement that read through it. The value is copied
    /// out before the drop because a drop frees memory. `infer.rs` refuses to
    /// read an *owned* field by value, so the value copied out is never the
    /// buffer the drop frees.
    ///
    /// `None` when the temporary owns nothing: it has no drop to record, so
    /// reading the field straight out of the rvalue is already correct and
    /// staging it would only add a local that is never freed from.
    fn staged_field_value(&self, root: &Expr, chain: &Expr) -> Option<String> {
        if !needs_drop(&root.ty, self.owned) {
            return None;
        }
        let tmp = fresh_temp();
        let value = fresh_temp();
        let hold = format!("{} = {}; ", root.ty.c_decl(&tmp), self.expr_value(root));
        let text = self.chain_off(&tmp, chain);
        Some(format!(
            "({{ {hold}{} = {text}; {}(&{tmp}); {value}; }})",
            chain.ty.c_decl(&value),
            drop_fn_name(&root.ty)
        ))
    }

    /// One call as a statement expression: the staged temporaries are built
    /// first, the call runs, and every staged temporary that owns memory is
    /// freed after it (`a drop call returns void`, so a value has to go
    /// through a temporary of its own first).
    fn hoisted_call(
        &self,
        prelude: String,
        call: String,
        staged: &[(String, Ty)],
        ret: &Ty,
    ) -> String {
        let owned: Vec<&(String, Ty)> = staged
            .iter()
            .filter(|(_, ty)| needs_drop(ty, self.owned))
            .collect();
        if prelude.is_empty() {
            return call;
        }
        // The call's value cannot be the statement expression's last statement
        // when a staged temporary has to be freed after it, so the result goes
        // through a temporary of its own and the frees follow. With nothing to
        // free, and only for a non-`unit` result, the call itself can be the
        // last statement.
        if owned.is_empty() && *ret != Ty::Unit {
            return format!("({{ {prelude}{call}; }})");
        }
        let mut body = prelude;
        if *ret == Ty::Unit {
            writeln!(body, "{call};").unwrap();
        } else {
            let result_tmp = fresh_temp();
            writeln!(body, "{} {result_tmp} = {call};", ret.c_type()).unwrap();
            for (tmp, ty) in &owned {
                writeln!(body, "{}(&{tmp});", drop_fn_name(ty)).unwrap();
            }
            writeln!(body, "{result_tmp};").unwrap();
        }
        if *ret == Ty::Unit {
            for (tmp, ty) in &owned {
                writeln!(body, "{}(&{tmp});", drop_fn_name(ty)).unwrap();
            }
        }
        format!("({{ {body}}})")
    }

    /// Renders an assignment target as a C lvalue. The type checker
    /// guarantees an assignment's target is always an `Ident`, a `.field`
    /// chain rooted in one, or a dereferenced borrow (`*p`, `(*p).field`) —
    /// see `infer::is_tir_place`.
    fn lvalue(&self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Ident(name) => self.local_c_name(name),
            ExprKind::FieldAccess { base, field } => self.field_access(base, field),
            ExprKind::Deref { expr: sub } => {
                let inner = self.expr_value(sub);
                format!("(*{inner})")
            }
            other => {
                unreachable!("type checker only allows places as assignment targets, got {other:?}")
            }
        }
    }

    /// Returns the printf format specifier and the (possibly cast) argument
    /// text for printing a value of the given expression's type. A `Bytes`
    /// needs two arguments (`%.*s` takes a length and a pointer), which is
    /// why the second half of the pair is a string that may hold more than
    /// one argument.
    fn printf_piece(&self, expr: &Expr) -> (&'static str, String) {
        let value = self.expr_value(expr);
        self.printf_piece_with(&value, &expr.ty)
    }

    /// The specifier and argument text for a value that has already been
    /// rendered (usually because codegen hoisted it into a temporary).
    fn printf_piece_with(&self, value: &str, ty: &Ty) -> (&'static str, String) {
        match ty {
            Ty::I32 => ("%d", value.to_string()),
            Ty::I64 => ("%lld", format!("(long long){value}")),
            Ty::U32 => ("%u", value.to_string()),
            Ty::U64 => ("%llu", format!("(unsigned long long){value}")),
            Ty::F32 | Ty::F64 => ("%f", format!("(double){value}")),
            Ty::Bool => ("%s", format!("({value} ? \"true\" : \"false\")")),
            Ty::Char => ("%c", value.to_string()),
            Ty::Str => ("%s", value.to_string()),
            // A `View` prints by pointer *and length*, never as a C string:
            // a slice of the source has no NUL after it, so `%s` would run
            // into whatever follows and print a different string than the
            // program holds. The length is cast because `%.*s` takes an
            // `int`.
            Ty::View => (
                "%.*s",
                format!("(int)({value}).len, (const char*)({value}).ptr"),
            ),
            // A `&str` view carries the pointer itself; `%.*s`-style length
            // handling stays for the owned buffers below.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => ("%s", value.to_string()),
            Ty::U8 | Ty::U16 => ("%u", format!("(unsigned){value}")),
            Ty::Usize => ("%zu", format!("(size_t){value}")),
            Ty::Isize => ("%lld", format!("(long long){value}")),
            // `infer.rs` refuses a function value in `print` (there is no C
            // format for a pointer to code), so this is unreachable rather
            // than a missing case.
            Ty::Fn { .. } => unreachable!("a function value cannot be printed"),
            // Same, for a closure: `infer.rs` refuses one in `print` too.
            Ty::Closure { .. } => unreachable!("a closure cannot be printed"),
            // An opaque handle has no readable value, so `infer.rs` refuses
            // one in `print` before codegen sees it.
            Ty::Handle(_) => unreachable!("an opaque handle cannot be printed"),
            // A borrowed `Bytes`/`String` is the only way `print` sees one —
            // `infer.rs` auto-borrows an owned place — so `->` is the accessor
            // that applies. The length is cast because `%.*s` takes an `int`.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Bytes | Ty::String) => (
                "%.*s",
                format!("(int)({value})->len, (const char*)({value})->data"),
            ),
            Ty::Bytes | Ty::String => (
                "%.*s",
                format!("(int)({value}).len, (const char*)({value}).data"),
            ),
            Ty::Unit
            | Ty::Struct(_)
            | Ty::List(_)
            | Ty::Box(_)
            | Ty::Map(..)
            | Ty::Array(..)
            | Ty::Atomic(_)
            | Ty::Thread(_)
            | Ty::Ref { .. } => {
                unreachable!(
                    "type checker rejects printing unit/struct/container/atomic/thread/reference values"
                )
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("    ");
    }
}

/// Whether a `tir` expression has stable storage: an `Ident`, a `.field`
/// chain rooted in one, or a dereferenced borrow. Mirrors
/// `infer::is_tir_place` — used here only to decide whether a borrow needs a
/// hoisted temporary.
/// The user type a value names, looking through a borrow — the codegen-side
/// twin of `infer::struct_name_of`, and the reason an `EnumField` node does
/// not have to carry the enum's name.
fn struct_name_of_ty(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name) => Some(name),
        Ty::Ref { inner, .. } => struct_name_of_ty(inner),
        _ => None,
    }
}

fn is_place(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Ident(_)
            | ExprKind::FieldAccess { .. }
            | ExprKind::EnumField { .. }
            | ExprKind::Deref { .. }
            | ExprKind::Capture { .. }
    )
}

/// The rvalue a field chain hangs off, when the chain's own address is not C
/// storage already: `None` for a place (`c`, `c.f`, `*p.f`), `Some(root)` for a
/// chain rooted at a temporary (`mkName().name`, `xs.pop().a`).
///
/// The chain is staged at this expression and never at the field: the whole
/// temporary is what has to be freed, and freeing the field alone would leave
/// every other field it owns behind. `is_place` above staying shallow is what
/// makes the two separable: a `FieldAccess` is a place for the purpose of
/// taking an address (it is one once the root is staged), and this asks the
/// narrower question of whether the address outlives the statement.
///
/// A chain that steps through a borrow is neither: `headers.get(i).name` lives
/// in whatever `get` pointed at, so its address is the pointee's and nothing
/// has to be staged. That is the same line `infer::is_durable_place` draws.
fn temporary_root(expr: &Expr) -> Option<&Expr> {
    let mut cur = expr;
    loop {
        match &cur.kind {
            ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => {
                if matches!(base.ty, Ty::Ref { .. }) {
                    return None;
                }
                cur = base;
            }
            _ => break,
        }
    }
    if is_place(cur) { None } else { Some(cur) }
}

/// Which arguments a plan's capability checks read: `Path`, `Name`, `Host` and
/// `Port` name one each, `HostPort` names two. The flags line up with the
/// call's arguments, so `render_arguments` can hoist exactly those.
fn checked_arg_flags(checked: &[crate::stdlib::CapCheck], arg_count: usize) -> Vec<bool> {
    use crate::stdlib::CapCheck;

    let mut flags = vec![false; arg_count];
    for check in checked {
        match *check {
            CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) | CapCheck::Port(i) => {
                flags[i] = true;
            }
            CapCheck::HostPort(host, port) => {
                flags[host] = true;
                flags[port] = true;
            }
        }
    }
    flags
}

/// Whether a checked resource argument has to be hoisted into a temporary so
/// that the check and the call share one evaluation of it.
///
/// A durable place (a binding, a `datac` field of one, a dereferenced borrow)
/// reads the same twice with no effect, and a literal has no evaluation to
/// repeat, so those need nothing. Everything else, a call in particular, is
/// evaluated exactly once. A `Ref` is left alone because `stage_argument`
/// already gives a borrow stable text: it stages a temporary root itself, and
/// a `&str` borrow is the pointer.
fn checked_arg_needs_temp(arg: &Expr) -> bool {
    match &arg.kind {
        ExprKind::Ref { .. }
        | ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::BoolLit(_) => false,
        _ => temporary_root(arg).is_some(),
    }
}

/// Whether a borrow is a `&str`: the one borrow C spells as the pointer
/// itself rather than as the address of a value.
fn is_str_borrow(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str))
}

/// One member access on a base whose C text is already rendered: `->` for a
/// borrowed base, `.` otherwise. `field_access`, `enum_member_access`,
/// `enum_tag_access` and `chain_off` all project through a value this way, so
/// the rule lives in one place.
fn member_access(base_text: &str, base_ty: &Ty, suffix: &str) -> String {
    if matches!(base_ty, Ty::Ref { .. }) {
        format!("({base_text})->{suffix}")
    } else {
        format!("({base_text}).{suffix}")
    }
}

/// How a closure callee reaches its code pointer and its environment: `.fn` /
/// `.env` on a closure value, `->fn` / `->env` when the callee is a borrow of
/// one (a parameter of the type borrows it, per the parameter rule). `None`
/// for a plain function value, which is the pointer itself (A-4 phase E).
fn closure_access(ty: &Ty) -> Option<(&'static str, &'static str)> {
    match ty {
        Ty::Closure { .. } => Some((".fn", ".env")),
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => Some(("->fn", "->env")),
        _ => None,
    }
}

fn fresh_temp() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!("_xtmp{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// The C text of `place` read out through the slot zeroed in place: the value
/// is copied into a temporary, the slot is set to the zero value, and the
/// temporary is the expression's value. `place` has to be an lvalue, and the
/// zero is what makes the base's own drop free nothing where the value now
/// lives (the partial-move rule, A-11).
fn zeroed_read(place: String, ty: &Ty) -> String {
    let tmp = fresh_temp();
    format!(
        "({{ {} = {}; {} = ({}){{0}}; {}; }})",
        ty.c_decl(&tmp),
        place,
        place,
        ty.c_type(),
        tmp
    )
}

/// The C name of a local that shadows an earlier binding of the same `.kp`
/// name in its own block (BUG-v6Qe0). The spelling carries the shadowed name
/// so the generated C stays readable. A user's local is always `kelp_<name>`
/// (`c_name` below), so the `_kshadow_` prefix cannot collide with one, and
/// the counter is process-wide the way `fresh_temp`'s is, so two of them
/// cannot collide with each other.
fn shadow_name(name: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "_kshadow_{}_{}",
        crate::types::c_ident(name),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// One drop call, as a C statement. The function names come from
/// `types::drop_fn_name`: the two builtin owned types have bodies in the
/// runtime preamble, and a `datac` that owns memory gets a generated one
/// (see `write_struct`).
/// `name` is the C name the target's binding resolved to at the point of the
/// drop (`Gen::local_c_name`), which for a shadowed spelling is not the name
/// the plan recorded.
fn drop_call(target: &DropTarget, name: &str) -> String {
    format!("{}(&{});", drop_fn_name(&target.ty), name)
}

/// Escapes literal template text for embedding in a generated `printf`
/// format string: `%` must be doubled, and quotes/backslashes/newlines/tabs
/// escaped the same way `c_string_literal` does for ordinary strings.
fn printf_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%%"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            // CR was missing, and a raw carriage return inside a C string
            // literal is not a string at all — it breaks the generated line.
            // Nothing could reach this until the lexer learned `\r` (S4), so
            // it took a protocol that ends its lines with CRLF to find it.
            '\r' => out.push_str("\\r"),
            // Any other control byte goes out as three octal digits. Not
            // `\x`: C's hex escape swallows the hex digits that follow it, so
            // `\x0d` + "a" would be one escape, not two characters.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// A float literal, spelled so C reads it as a floating-point constant of the
/// same value.
///
/// `Display` for `f64` writes the shortest decimal that round-trips, but it
/// writes it in positional form and drops the fractional part of an integral
/// value: `100000000000000000000.0` becomes `100000000000000000000` and `-0.0`
/// becomes `-0`. Emitted as-is, the first makes `cc` reject the file ("integer
/// literal is too large to be represented in any integer type") and the second
/// silently changes the value — C reads `-0` as the integer zero, so a sign
/// test or an integer-arithmetic rewrite of both operands loses the sign and
/// the float-ness. `Debug` is the shortest form that keeps the type: it always
/// emits a `.` or an exponent (`1e20`, `-0.0`, `0.1`, `1.7976931348623157e308`),
/// and every one of those is a valid C `double` literal. An `f32` context is
/// safe too: the `f64` value came from an `f32` originally (or from a fold),
/// the shortest `f64` round-trip is exact, and the narrowing conversion C then
/// applies returns the same `float`.
///
/// The two non-finite values have no literal spelling and need the `<math.h>`
/// macros: a decimal literal long enough to overflow `f64` parses to infinity
/// in the lexer, and constant folding (`0.0 / 0.0`) can produce a NaN.
fn c_float_literal(v: f64) -> String {
    if v.is_nan() {
        return "NAN".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "(-INFINITY)" } else { "INFINITY" }.to_string();
    }
    format!("{v:?}")
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
fn c_name(name: &str) -> String {
    if name == "main" {
        name.to_string()
    } else {
        format!("kelp_{}", crate::types::c_ident(name))
    }
}

fn signed_lit(v: i128) -> String {
    format!("{v}LL")
}

fn unsigned_lit(v: u128) -> String {
    if v <= i64::MAX as u128 {
        format!("{v}LL")
    } else {
        format!("{v}ULL")
    }
}

/// One numeric conversion, as a C expression. A widening is a plain cast; a
/// conversion that can lose a value is a statement expression that checks the
/// range first and panics with the `.kp` spelling of the call, so a narrowing
/// is never a silent truncation. An integer-to-float conversion is a plain
/// cast in both directions (`to_f64` exact, `to_f32` the language's one
/// defined rounding), because `int_bounds` has no bounds for a float target
/// and the check below is skipped. The argument is evaluated once, into a
/// temporary, because the check and the cast both read it.
fn numeric_conversion(callee: &str, from: &Ty, to: &Ty, value: &str) -> String {
    let target = to.c_type();
    let (Some((s_min, s_max)), Some((t_min, t_max))) =
        (crate::types::Ty::int_bounds(from), crate::types::Ty::int_bounds(to))
    else {
        return format!("(({target})({value}))");
    };
    if s_min >= t_min && s_max <= t_max {
        return format!("(({target})({value}))");
    }
    let tmp = fresh_temp();
    // Only the bounds the source's own range cannot rule out. The upper
    // comparison used to be emitted unconditionally, so `i32.to_u32` checked
    // `tmp > 4294967295LL` against an `int32_t` — a comparison clang reports
    // as always false, which is the second half of BUG-3's warning noise. A
    // bound that is unreachable by construction is dropped, not emitted as a
    // tautology.
    let mut checks: Vec<String> = Vec::new();
    if s_min < t_min {
        // The source can go below the target's floor: `0` for an unsigned
        // target, the signed type's real minimum otherwise.
        checks.push(format!("{tmp} < {}", signed_lit(t_min)));
    }
    if s_max > t_max {
        checks.push(format!("{tmp} > {}", unsigned_lit(t_max)));
    }
    // The widening case returned above, so at least one bound is missing.
    let cond = checks.join(" || ");
    let message = callee.replace('#', ".");
    format!(
        "({{ {} {tmp} = {value}; if ({cond}) {{ kelp_panic(\"{message}: value out of range\"); }} ({target}){tmp}; }})",
        from.c_type()
    )
}

/// Struct field names live in C's per-struct member namespace, not the
/// ordinary identifier namespace, but prefixed anyway for consistency and to
/// dodge a C keyword used as a field name.
fn field_name(name: &str) -> String {
    format!("kelp_{name}")
}

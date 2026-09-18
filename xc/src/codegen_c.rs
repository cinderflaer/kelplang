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
use crate::borrowck::{DropPlan, DropTarget};
use crate::diag::SourceMap;
use crate::lexer::Span;
use crate::tir::*;
use crate::types::{drop_fn_name, needs_drop, owned_structs, StructDef, Ty};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// The fixed C support code every generated program carries: the `Bytes`
/// representation, the bodies of its builtin methods, and the panic path.
/// These are the bodies `infer.rs`'s builtin table promises exist; a builtin
/// the runtime does not define would fail at `cc`, which is the right place
/// for that mistake to surface.
///
/// `x_bytes_drop` takes a pointer and clears it, so a drop is idempotent at
/// runtime as well as unreachable twice by construction — together with the
/// move checking that is what makes "no double free is expressible" testable
/// rather than merely argued.
const RUNTIME: &str = r#"// ---- X runtime (M6) -------------------------------------------------------
typedef struct {
    unsigned char* data;
    size_t len;
} x_Bytes;

static void x_panic(const char* msg) {
    fprintf(stderr, "panic: %s\n", msg);
    exit(1);
}

static bool x_utf8_valid(const unsigned char* p, size_t n) {
    size_t i = 0;
    while (i < n) {
        unsigned char c = p[i];
        size_t extra;
        unsigned long cp;
        if (c < 0x80) { i += 1; continue; }
        else if ((c & 0xE0) == 0xC0) { extra = 1; cp = c & 0x1F; }
        else if ((c & 0xF0) == 0xE0) { extra = 2; cp = c & 0x0F; }
        else if ((c & 0xF8) == 0xF0) { extra = 3; cp = c & 0x07; }
        else { return false; }
        if (i + extra >= n) { return false; }
        for (size_t k = 1; k <= extra; k++) {
            if ((p[i + k] & 0xC0) != 0x80) { return false; }
            cp = (cp << 6) | (p[i + k] & 0x3F);
        }
        if (extra == 1 && cp < 0x80) { return false; }
        if (extra == 2 && cp < 0x800) { return false; }
        if (extra == 3 && cp < 0x10000) { return false; }
        if (cp > 0x10FFFF) { return false; }
        if (cp >= 0xD800 && cp <= 0xDFFF) { return false; }
        i += extra + 1;
    }
    return true;
}

static x_Bytes x_Bytes__empty(void) {
    x_Bytes b;
    b.data = NULL;
    b.len = 0;
    return b;
}

static x_Bytes x_Bytes__from_str(const char* s) {
    size_t n = strlen(s);
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        x_panic("out of memory");
    }
    memcpy(p, s, n + 1);
    x_Bytes b;
    b.data = p;
    b.len = n;
    return b;
}

static size_t x_Bytes__len(const x_Bytes* self) {
    return self->len;
}

static bool x_Bytes__is_empty(const x_Bytes* self) {
    return self->len == 0;
}

static uint8_t x_Bytes__get(const x_Bytes* self, size_t i) {
    if (i >= self->len) {
        x_panic("Bytes.get: index out of bounds");
    }
    return self->data[i];
}

static void x_Bytes__set(x_Bytes* self, size_t i, uint8_t value) {
    if (i >= self->len) {
        x_panic("Bytes.set: index out of bounds");
    }
    self->data[i] = value;
}

static x_Bytes x_Bytes__clone(const x_Bytes* self) {
    unsigned char* p = (unsigned char*)malloc(self->len + 1);
    if (p == NULL) {
        x_panic("out of memory");
    }
    memcpy(p, self->data, self->len);
    p[self->len] = 0;
    x_Bytes b;
    b.data = p;
    b.len = self->len;
    return b;
}

static void x_bytes_drop(x_Bytes* b) {
    free(b->data);
    b->data = NULL;
    b->len = 0;
}

// ---- x_String (M6.2) ------------------------------------------------------
// Same shape as x_Bytes, kept a distinct type so text and raw bytes are not
// interchangeable. `from_bytes`/`into_bytes` transfer ownership of the same
// buffer rather than copying it.
typedef struct {
    unsigned char* data;
    size_t len;
} x_String;

static x_String x_String__empty(void) {
    x_String s;
    s.data = NULL;
    s.len = 0;
    return s;
}

static x_String x_String__from_str(const char* text) {
    size_t n = strlen(text);
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        x_panic("out of memory");
    }
    memcpy(p, text, n + 1);
    x_String s;
    s.data = p;
    s.len = n;
    return s;
}

/* Takes ownership of `b`'s buffer: the caller's binding is moved into this
   call, so nothing else frees it. */
static x_String x_String__from_bytes(x_Bytes b) {
    if (!x_utf8_valid(b.data, b.len)) {
        x_panic("String.from_bytes: input is not valid utf-8");
    }
    x_String s;
    s.data = b.data;
    s.len = b.len;
    return s;
}

/* Hands the buffer to the caller: the receiver is moved into this call. */
static x_Bytes x_String__into_bytes(x_String self) {
    x_Bytes b;
    b.data = self.data;
    b.len = self.len;
    return b;
}

static size_t x_String__len(const x_String* self) {
    return self->len;
}

static bool x_String__is_empty(const x_String* self) {
    return self->len == 0;
}

static size_t x_String__char_count(const x_String* self) {
    size_t n = 0;
    for (size_t i = 0; i < self->len; i++) {
        if ((self->data[i] & 0xC0) != 0x80) {
            n++;
        }
    }
    return n;
}

static x_String x_String__clone(const x_String* self) {
    unsigned char* p = (unsigned char*)malloc(self->len + 1);
    if (p == NULL) {
        x_panic("out of memory");
    }
    memcpy(p, self->data, self->len);
    p[self->len] = 0;
    x_String s;
    s.data = p;
    s.len = self->len;
    return s;
}

/* A borrowed view of the same buffer (M6.3). Every constructor keeps the
   buffer NUL-terminated, which is what a `str` (a `const char*`) requires. */
static const char* x_String__as_str(const x_String* self) {
    return (const char*)self->data;
}

static const char* x_Bytes__as_str(const x_Bytes* self) {
    if (!x_utf8_valid(self->data, self->len)) {
        x_panic("Bytes.as_str: input is not valid utf-8");
    }
    return (const char*)self->data;
}

static void x_string_drop(x_String* s) {
    free(s->data);
    s->data = NULL;
    s->len = 0;
}

/* The allocation primitive `std/mem` wraps (M6.2): an owned, zero-filled,
   NUL-terminated buffer of `n` bytes. */
static x_Bytes x_bytes_alloc_raw(size_t n) {
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        x_panic("out of memory");
    }
    memset(p, 0, n + 1);
    x_Bytes b;
    b.data = p;
    b.len = n;
    return b;
}
// ---- end X runtime -------------------------------------------------------
"#;

/// Lexical path normalization, shared by `path.normalize` (M6.3) and the
/// capability check (M7) — the plan requires exactly one implementation,
/// because the security-relevant normalization must not be a second,
/// slightly different function. Returns false when the path is not absolute
/// or escapes its own root via `..`.
const PATH_RUNTIME: &str = r#"// ---- X path runtime -------------------------------------------------------
static bool x_path_normalize(const char* in, char* out, size_t cap) {
    if (in[0] != '/') {
        return false;
    }
    size_t o = 0;
    out[o++] = '/';
    const char* p = in;
    int depth = 0;
    while (*p != 0) {
        while (*p == '/') {
            p++;
        }
        if (*p == 0) {
            break;
        }
        const char* start = p;
        while (*p != 0 && *p != '/') {
            p++;
        }
        size_t len = (size_t)(p - start);
        if (len == 1 && start[0] == '.') {
            continue;
        }
        if (len == 2 && start[0] == '.' && start[1] == '.') {
            if (depth == 0) {
                return false;   /* escapes the root */
            }
            while (o > 1 && out[o - 1] != '/') {
                o--;
            }
            if (o > 1) {
                o--;
            }
            depth--;
            continue;
        }
        if (o > 1) {
            if (o + 1 >= cap) {
                return false;
            }
            out[o++] = '/';
        }
        if (o + len >= cap) {
            return false;
        }
        memcpy(out + o, start, len);
        o += len;
        depth++;
    }
    out[o] = 0;
    return true;
}
// ---- end X path runtime ---------------------------------------------------
"#;

/// The capability check itself (M7). Emitted only for programs that contain a
/// gated call, since `x_path_normalize` is the only piece they share.
const CAP_RUNTIME: &str = r#"// ---- X capability runtime (M7) --------------------------------------------
/* `*` matches within one path segment, `**` crosses segments and is legal
   only as a whole segment; no other metacharacters, no regex. */
static bool x_glob_match(const char* pat, const char* path) {
    while (*pat != 0) {
        if (pat[0] == '*' && pat[1] == '*') {
            const char* rest = pat + 2;
            if (*rest == '/') {
                rest++;
            }
            if (*rest == 0) {
                return true;   /* a trailing '**' matches everything below */
            }
            for (const char* p = path; ; p++) {
                if (x_glob_match(rest, p)) {
                    return true;
                }
                if (*p == 0) {
                    return false;
                }
            }
        }
        if (*pat == '*') {
            const char* rest = pat + 1;
            for (const char* p = path; ; p++) {
                if (x_glob_match(rest, p)) {
                    return true;
                }
                if (*p == '/' || *p == 0) {
                    return false;
                }
            }
        }
        if (*pat != *path) {
            return false;
        }
        pat++;
        path++;
    }
    return *path == 0;
}

/* Symlinks are resolved *before* matching, which is the whole reason this
   exists: a link inside the grant that points outside it must be denied.
   `realpath` fails for a path that does not exist yet (a write or an
   mkdir), so the deepest existing ancestor is resolved and the rest is
   appended verbatim. */
static bool x_resolve_path(const char* path, char* out, size_t cap) {
    char real[PATH_MAX];
    if (realpath(path, real) != NULL) {
        snprintf(out, cap, "%s", real);
        return true;
    }
    char buf[PATH_MAX];
    snprintf(buf, sizeof buf, "%s", path);
    char* slash = strrchr(buf, '/');
    if (slash == NULL || slash == buf) {
        return false;
    }
    *slash = 0;
    char parent[PATH_MAX];
    if (!x_resolve_path(buf, parent, sizeof parent)) {
        return false;
    }
    snprintf(out, cap, "%s/%s", parent, slash + 1);
    return true;
}

static void x_cap_deny(const char* domain, const char* path, const char* const* patterns,
                       size_t count, const char* module, const char* at) {
    fprintf(stderr, "%s: panic: '%s' is outside the %s capability granted to module '%s'\n",
            at, path, domain, module);
    fprintf(stderr, "  granted: %s\n", count == 0 ? "(nothing)" : "");
    for (size_t i = 0; i < count; i++) {
        fprintf(stderr, "    %s\n", patterns[i]);
    }
    exit(1);
}

/* A path argument: lexically normalized, then resolved through symlinks, and
   only then matched — the order `plan.md` insists on, because a link inside
   the grant that points outside it must be denied. */
static void x_cap_check_path(const char* domain, const char* path, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    char normalized[PATH_MAX];
    if (!x_path_normalize(path, normalized, sizeof normalized)) {
        return x_cap_deny(domain, path, patterns, count, module, at);
    }
    char resolved[PATH_MAX];
    if (!x_resolve_path(normalized, resolved, sizeof resolved)) {
        snprintf(resolved, sizeof resolved, "%s", normalized);
    }
    for (size_t i = 0; i < count; i++) {
        if (x_glob_match(patterns[i], resolved)) {
            return;
        }
    }
    return x_cap_deny(domain, resolved, patterns, count, module, at);
}

/* A name argument (`env`'s variable names): matched as written, since a name
   has no path structure to normalize. */
static void x_cap_check_name(const char* domain, const char* name, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    for (size_t i = 0; i < count; i++) {
        if (x_glob_match(patterns[i], name)) {
            return;
        }
    }
    return x_cap_deny(domain, name, patterns, count, module, at);
}

// ---- end X capability runtime ---------------------------------------------
"#;

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
            "x_caps_{}_{}",
            crate::types::c_ident(&module.replace('/', "_")),
            domain
        )
    }
}

/// Whether a type is `str` or a borrow of one — the arguments a capability
/// check treats as resource names.
fn is_str_like_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Str => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Str),
        _ => false,
    }
}

pub fn generate(program: &Program, drops: &DropPlan, sources: &SourceMap) -> String {
    let owned = owned_structs(&program.structs);
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
    // The path runtime is shared by `path.normalize` and the capability
    // check, so it is emitted whenever either could use it; the capability
    // runtime only when some call is actually gated.
    let path_loaded = program.functions.iter().any(|f| f.module == "std/path");
    let cap_needed = !program.gated.is_empty();
    let mut out = String::new();
    // Included unconditionally: `std/math`'s externs call the C library
    // directly, and an unused declaration costs nothing.
    out.push_str("#include <limits.h>\n");
    out.push_str("#include <fcntl.h>\n");
    out.push_str("#include <math.h>\n");
    out.push_str("#include <sys/stat.h>\n");
    out.push_str("#include <sys/types.h>\n");
    out.push_str("#include <time.h>\n");
    out.push_str("#include <unistd.h>\n");
    out.push_str("#include <stdint.h>\n");
    out.push_str("#include <stdbool.h>\n");
    out.push_str("#include <stdio.h>\n");
    out.push_str("#include <stdlib.h>\n");
    out.push_str("#include <string.h>\n\n");
    out.push_str(RUNTIME);
    out.push('\n');
    if path_loaded || cap_needed {
        out.push_str(PATH_RUNTIME);
        out.push('\n');
    }
    if cap_needed {
        out.push_str(CAP_RUNTIME);
        out.push('\n');
    }

    // Each module's granted patterns, as constant data. No API can widen
    // these at runtime, and no stdlib function accepts a capability as a
    // value, so a grant cannot be smuggled through an argument.
    if cap_needed {
        out.push_str("// capability allowlists, compiled in per module\n");
        for g in &program.caps {
            if g.patterns.is_empty() {
                continue;
            }
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

    for s in &program.structs {
        write_struct(&mut out, s, &owned);
    }
    if !program.structs.is_empty() {
        out.push('\n');
    }

    write_pd_globals(&mut out, &program.pds, drops, &owned, sources);

    for f in &program.functions {
        if f.name == "main" {
            continue; // defined (not just declared) below, with its special `int main(void)` signature
        }
        write_signature(&mut out, f);
        out.push_str(";\n");
    }
    out.push('\n');

    for f in &program.functions {
        if f.extern_ {
            write_extern(&mut out, f);
            out.push('\n');
            continue;
        }
        let mut gen = Gen::new(drops, f.name.clone(), f.module.clone(), &caps, sources);
        gen.write_function(f, !program.pds.is_empty());
        out.push_str(&gen.out);
        out.push('\n');
    }

    out
}

/// Emits an `extern` declaration's body, which comes from the compiler rather
/// than from `.x` source (see `externs.rs`). `infer.rs` has already rejected
/// an `extern` with no known body, so reaching the `expect` here would mean
/// the table and the checker disagreed.
fn write_extern(out: &mut String, f: &Function) {
    let body = crate::externs::body(&f.module, &f.name)
        .unwrap_or_else(|| panic!("no compiler-supplied body for extern '{}'", f.name));
    // The C signature renames every parameter (`n` -> `x_n`), so a body's C
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

/// Emits one file-scope global per passdown plus the generated function that
/// initializes them, in declaration order (M5.2). The global's name is the
/// same `pd#...` name a referencing function takes as a hidden parameter, so
/// inside such a function the parameter shadows its own global and both
/// spell the same symbol in C — including inside `main`, which can take no
/// parameters and therefore reads the globals directly.
fn write_pd_globals(
    out: &mut String,
    pds: &[Pd],
    drops: &DropPlan,
    owned: &HashSet<String>,
    sources: &SourceMap,
) {
    if pds.is_empty() {
        return;
    }
    // A `pd` initializer is not a function, so no drop point can be keyed to
    // it; an owned local inside one would leak. `infer.rs` rejects owned
    // locals in initializers, so this only affects an initializer that
    // happens to build and discard one.
    // A `pd` initializer is not a function, so it can hold no gated call
    // either (a gated call needs the module's grants, and this emitter has
    // no module); an empty table is the honest answer here.
    let no_caps = CapTable {
        gated: HashMap::new(),
        patterns: HashMap::new(),
    };
    let gen = Gen::new(drops, String::new(), String::new(), &no_caps, sources);
    out.push_str("// passdowns: assigned once by x_init_pds(), read-only afterwards\n");
    for pd in pds {
        writeln!(
            out,
            "static {} {};",
            pd.value_ty.c_type(),
            c_name(&pd.name)
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
        out.push_str("static void x_drop_pds(void) {\n");
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
    out.push_str("static void x_init_pds(void) {\n");
    if !droppable.is_empty() {
        out.push_str("    atexit(x_drop_pds);\n");
    }
    for pd in pds {
        writeln!(
            out,
            "    {} = {}; // pd {}",
            c_name(&pd.name),
            gen.expr_value(&pd.init),
            pd.qualified
        )
        .unwrap();
    }
    out.push_str("}\n\n");
}

/// One `datac`'s C typedef, plus its drop function when it owns heap memory
/// (M6.2). Emitted in declaration order, and a `datac` may only name earlier
/// `datac`s, so a struct's drop function is always defined before the struct
/// that calls it — no forward declarations needed.
fn write_struct(out: &mut String, s: &StructDef, owned: &HashSet<String>) {
    write!(out, "typedef struct {{\n").unwrap();
    for f in &s.fields {
        write!(out, "    {} {};\n", f.ty.c_type(), field_name(&f.name)).unwrap();
    }
    let ty = Ty::Struct(s.name.clone());
    write!(out, "}} {};\n", ty.c_type()).unwrap();
    if needs_drop(&ty, owned) {
        writeln!(
            out,
            "static void x_drop_{}({}* v) {{",
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
}

fn write_signature(out: &mut String, f: &Function) {
    write!(out, "{} {}(", f.ret.c_type(), c_name(&f.name)).unwrap();
    if f.params.is_empty() {
        out.push_str("void");
    } else {
        for (i, (name, ty)) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{} {}", ty.c_type(), c_name(name)).unwrap();
        }
    }
    out.push(')');
}

/// Writing state that outlives a single expression: the output buffer, the
/// drop plan, and which function is being written (the plan is keyed
/// per-function, and a `Gen` is created fresh for each function).
struct Gen<'a> {
    out: String,
    drops: &'a DropPlan,
    function: String,
    /// The module the current function was declared in: capability checks use
    /// *that* module's patterns, which is what makes a grant a property of
    /// the code that made the call (M7).
    module: String,
    caps: &'a CapTable,
    /// The loaded sources, used only to turn a capability denial's span into
    /// the `file:line:col` the runtime panic names (M7).
    sources: &'a SourceMap,
}

impl<'a> Gen<'a> {
    fn new(
        drops: &'a DropPlan,
        function: String,
        module: String,
        caps: &'a CapTable,
        sources: &'a SourceMap,
    ) -> Self {
        Gen {
            out: String::new(),
            drops,
            function,
            module,
            caps,
            sources,
        }
    }

    /// A nested emitter for a block a value-renderer has to produce: same
    /// plan, same function, same module, its own buffer.
    fn sub(&self) -> Gen<'a> {
        Gen::new(
            self.drops,
            self.function.clone(),
            self.module.clone(),
            self.caps,
            self.sources,
        )
    }

    fn drops_at_block_end(&self, span: Span) -> Vec<DropTarget> {
        self.drops.at_block_end(&self.function, span).to_vec()
    }

    fn drops_at_return(&self, span: Span) -> Vec<DropTarget> {
        self.drops.at_return(&self.function, span).to_vec()
    }

    /// Frees owned locals in the order `borrowck.rs` decided (reverse
    /// declaration order — a later binding is freed before an earlier one it
    /// may have taken its value from).
    fn write_drops(&mut self, targets: &[DropTarget], depth: usize) {
        for t in targets {
            indent(&mut self.out, depth);
            writeln!(self.out, "{}", drop_call(t)).unwrap();
        }
    }

    // ---- functions, blocks, statements ---------------------------------

    fn write_function(&mut self, f: &Function, init_pds: bool) {
        if f.name == "main" {
            // The C runtime requires `main` to return `int`; falling off the
            // end is defined by the C standard as `return 0`, so an `.x`
            // `main` (always `-> unit`) needs no extra trailing return. A
            // bare `return;` from `.x`, though, is a constraint violation in
            // a function declared to return `int`, so it's rewritten to
            // `return 0;` here rather than threading an "am I in main" flag
            // through every statement-writing function below.
            let mut body = self.sub();
            if init_pds {
                body.out.push_str("    x_init_pds();\n");
            }
            body.write_function_body(&f.body, 1, &f.ret);
            let rendered = body.out.replace("return;\n", "return 0;\n");
            self.out.push_str("int main(void) {\n");
            self.out.push_str(&rendered);
            self.out.push_str("}\n");
            return;
        }
        write_signature(&mut self.out, f);
        self.out.push_str(" {\n");
        self.write_function_body(&f.body, 1, &f.ret);
        self.out.push_str("}\n");
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
        let drops = self.drops_at_block_end(block.span);
        if let Some(tail) = &block.tail {
            if *ret_ty == Ty::Unit {
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
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        if let Some(tail) = &block.tail {
            self.write_stmt_like_expr(tail, depth);
        }
        let drops = self.drops_at_block_end(block.span);
        self.write_drops(&drops, depth);
    }

    fn write_stmt(&mut self, stmt: &Stmt, depth: usize) {
        match stmt {
            Stmt::Let { name, ty, init } => {
                let value = self.expr_value(init);
                indent(&mut self.out, depth);
                writeln!(self.out, "{} {} = {value};", ty.c_type(), c_name(name)).unwrap();
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
                let cond = self.expr_value(cond);
                indent(&mut self.out, depth);
                writeln!(self.out, "while ({cond}) {{").unwrap();
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
                let cond = self.expr_value(cond);
                indent(&mut self.out, depth);
                writeln!(self.out, "if ({cond}) {{").unwrap();
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
        writeln!(self.out, "{} {tmp} = {value};", expr.ty.c_type()).unwrap();
        self.write_drops(drops, depth);
        tmp
    }

    // ---- value position -------------------------------------------------

    /// Renders an expression for use as a C *value* (RHS of `=`, call
    /// argument, operand, condition, ...). `if`/block expressions become GNU
    /// statement-expressions here.
    fn expr_value(&self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::IntLit(v) => v.to_string(),
            ExprKind::FloatLit(v) => format!("{v}"),
            ExprKind::StringLit(s) => c_string_literal(s),
            ExprKind::BoolLit(b) => b.to_string(),
            ExprKind::Ident(name) => c_name(name),
            ExprKind::Unary { op, expr: sub } => {
                let sym = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
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
                };
                let l = self.expr_value(lhs);
                let r = self.expr_value(rhs);
                format!("({l} {sym} {r})")
            }
            ExprKind::Call { callee, args } => self.call_value(callee, args, expr.span),
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
                            // not leak.
                            if matches!(value.ty, Ty::Bytes | Ty::String) {
                                let tmp = fresh_temp();
                                let rendered = self.expr_value(value);
                                holds.push(format!("{} {tmp} = {rendered};", value.ty.c_type()));
                                let (spec, text) = self.printf_piece_with(&tmp, &value.ty);
                                fmt.push_str(spec);
                                args.push(text);
                                drops.push(format!("{}(&{tmp});", drop_fn_name(&value.ty)));
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
            ExprKind::FieldAccess { base, field } => self.field_access(base, field),
            ExprKind::Ref { expr: sub, .. } => {
                // A `&str` borrow is the same C value as the `str` it points
                // at (both `const char*`), so no `&` is emitted for it. Every
                // other pointee is represented by its address.
                if matches!(&expr.ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str)) {
                    return self.expr_value(sub);
                }
                let inner = self.expr_value(sub);
                format!("(&{inner})")
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
                writeln!(out, "{} {tmp} = {value_text};", target_ty.c_type()).unwrap();
                for d in &drops {
                    writeln!(out, "{}", drop_call(d)).unwrap();
                }
                writeln!(out, "{target_text} = {tmp};").unwrap();
                format!("({{\n{out}}})")
            }
            ExprKind::Block(block) => {
                let mut gen = self.sub();
                gen.write_block_value(block, 1);
                format!("({{\n{}}})", gen.out)
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
                let mut gen = self.sub();
                let cond = self.expr_value(cond);
                if expr.ty == Ty::Unit {
                    writeln!(gen.out, "if ({cond}) {{").unwrap();
                    gen.write_block_body(then_block, 1);
                    gen.out.push('}');
                    gen.write_else(else_block.as_ref(), 0);
                    gen.out.push('\n');
                } else {
                    let tmp = fresh_temp();
                    writeln!(gen.out, "{} {tmp};", expr.ty.c_type()).unwrap();
                    writeln!(gen.out, "if ({cond}) {{").unwrap();
                    gen.write_block_assign(then_block, &tmp, 1);
                    gen.out.push('}');
                    gen.write_else_assign(else_block.as_ref(), &tmp, 0);
                    gen.out.push('\n');
                    writeln!(gen.out, "{tmp};").unwrap();
                }
                format!("({{\n{}}})", gen.out)
            }
        }
    }

    /// Like `write_block_body`, but a present tail expression is emitted as
    /// the statement-expression's yielded value (its value is *not*
    /// discarded). When the block also has end-of-scope drops, the tail goes
    /// into a temporary first: the value of a GNU statement expression comes
    /// from its *last* statement, and a drop call is void.
    fn write_block_value(&mut self, block: &Block, depth: usize) {
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        let drops = self.drops_at_block_end(block.span);
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
    }

    /// Writes a block whose tail expression (if any) is assigned into `tmp`
    /// instead of being yielded as a statement-expression value — used for
    /// the branches of an `if` that's itself being lowered via a temporary.
    fn write_block_assign(&mut self, block: &Block, tmp: &str, depth: usize) {
        for stmt in &block.stmts {
            self.write_stmt(stmt, depth);
        }
        if let Some(tail) = &block.tail {
            let value = self.expr_value(tail);
            indent(&mut self.out, depth);
            writeln!(self.out, "{tmp} = {value};").unwrap();
        }
        let drops = self.drops_at_block_end(block.span);
        self.write_drops(&drops, depth);
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

    /// A gated call (M7): every resource argument is checked against the
    /// calling module's compiled-in allowlist *before* the call runs, and the
    /// whole thing is a statement expression so it works in value position
    /// (`val b = fs.read(p)`) as well as statement position.
    fn cap_checked_call(&self, callee: &str, call: String, args: &[Expr], span: Span) -> String {
        let Some(domain) = self.caps.gated.get(callee) else {
            return call;
        };
        let Some(patterns) = self.caps.patterns_for(&self.module, domain) else {
            return call;
        };
        let arg_is_str: Vec<bool> = args.iter().map(|a| is_str_like_ty(&a.ty)).collect();
        let checked = crate::stdlib::checked_args(domain, &arg_is_str);
        if checked.is_empty() {
            return call;
        }
        let symbol = CapTable::symbol(&self.module, domain);
        // `fs`/`net`/`process` match paths (normalized, symlinks resolved);
        // `env` matches variable names, which have no path structure.
        let check_fn = if crate::stdlib::patterns_are_paths(domain) {
            "x_cap_check_path"
        } else {
            "x_cap_check_name"
        };
        // The call site, as the author wrote it. The generated C carries no
        // `#line` directives, so this is the only way a runtime denial can
        // name a `.x` location.
        let at = self
            .sources
            .locate(&self.module, span)
            .unwrap_or_else(|| self.module.clone());
        let mut checks = String::new();
        for i in checked {
            let path = self.expr_value(&args[i]);
            write!(
                checks,
                "{check_fn}(\"{domain}\", {path}, {symbol}, {}, \"{}\", \"{at}\"); ",
                patterns.len(),
                self.module
            )
            .unwrap();
        }
        format!("({{ {checks}{call}; }})")
    }

    fn call_value(&self, callee: &str, args: &[Expr], span: Span) -> String {
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
                    "({{ {} {tmp} = {value}; printf(\"{fmt}{newline}\", {rendered}); {}(&{tmp}); }})",
                    arg.ty.c_type(),
                    drop_fn_name(&arg.ty)
                );
            }
            let (fmt, value) = self.printf_piece(arg);
            return format!("printf(\"{fmt}{newline}\", {value})");
        }

        let mut s = format!("{}(", c_name(callee));
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&self.expr_value(a));
        }
        s.push(')');
        self.cap_checked_call(callee, s, args, span)
    }

    /// `base.field` in C: `->` when `base` is reached through a borrow
    /// (`Ty::Ref`), `.` otherwise. Each `FieldAccess` node decides this
    /// purely from its own immediate base's type — a chain like `c.inner.x`
    /// (where `c` is a `mut Counter` parameter and `inner` is a plain
    /// embedded field) gets this right node-by-node: the innermost access
    /// sees `c`'s `Ref` type and emits `->`, the outer one sees `inner`'s
    /// plain `Struct` type and emits `.`, with no extra state threaded
    /// through.
    fn field_access(&self, base: &Expr, field: &str) -> String {
        let base_text = self.expr_value(base);
        if matches!(base.ty, Ty::Ref { .. }) {
            format!("({base_text})->{}", field_name(field))
        } else {
            format!("({base_text}).{}", field_name(field))
        }
    }

    /// Renders an assignment target as a C lvalue. The type checker
    /// guarantees an assignment's target is always an `Ident`, a `.field`
    /// chain rooted in one, or a dereferenced borrow (`*p`, `(*p).field`) —
    /// see `infer::is_tir_place`.
    fn lvalue(&self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Ident(name) => c_name(name),
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
            // A `&str` view carries the pointer itself; `%.*s`-style length
            // handling stays for the owned buffers below.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => ("%s", value.to_string()),
            Ty::U8 | Ty::U16 => ("%u", format!("(unsigned){value}")),
            Ty::Usize => ("%zu", format!("(size_t){value}")),
            Ty::Isize => ("%lld", format!("(long long){value}")),
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
            Ty::Unit | Ty::Struct(_) | Ty::Ref { .. } => {
                unreachable!("type checker rejects printing unit/struct/reference values")
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("    ");
    }
}

fn fresh_temp() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!("_xtmp{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// One drop call, as a C statement. The function names come from
/// `types::drop_fn_name`: the two builtin owned types have bodies in the
/// runtime preamble, and a `datac` that owns memory gets a generated one
/// (see `write_struct`).
fn drop_call(target: &DropTarget) -> String {
    format!("{}(&{});", drop_fn_name(&target.ty), c_name(&target.name))
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
            c => out.push(c),
        }
    }
    out
}

fn c_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `.x` identifiers are valid C identifiers as-is; prefix to dodge any
/// accidental collision with a C keyword or our own runtime symbols.
/// `main` is left alone since the C runtime requires that exact name for
/// the process entry point. Qualified names (M5 module prefixes, `Type#name`
/// method keys) go through `types::c_ident`.
fn c_name(name: &str) -> String {
    if name == "main" {
        name.to_string()
    } else {
        format!("x_{}", crate::types::c_ident(name))
    }
}

/// Struct field names live in C's per-struct member namespace, not the
/// ordinary identifier namespace, but prefixed anyway for consistency and to
/// dodge a C keyword used as a field name.
fn field_name(name: &str) -> String {
    format!("x_{name}")
}

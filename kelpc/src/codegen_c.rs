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
use crate::types::{drop_fn_name, needs_drop, owned_types, EnumDef, StructDef, Ty};
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
const RUNTIME: &str = r#"// ---- Kelp runtime (M6) -------------------------------------------------------
typedef struct {
    unsigned char* data;
    size_t len;
    /* Usable bytes at `data`, *excluding* the NUL terminator slot: the
       allocation is always `cap + 1` bytes, and `data[len]` is kept 0 so a
       `str` view of the buffer stays valid (S1 growth invariant). */
    size_t cap;
} kelp_Bytes;

/* `_Noreturn` is what tells the C compiler a `panic` cannot come back, so a
   generated function whose tail is a `panic` needs no trailing `return` and
   draws no "control reaches end of non-void function" warning. */
_Noreturn static void kelp_panic(const char* msg) {
    fprintf(stderr, "panic: %s\n", msg);
    exit(1);
}

/* A syscall failure names its cause on a second line: `errno` and the text
   `strerror` gives it. The `panic: <msg>` line stays exactly as it was, so a
   golden assertion on a message prefix is unaffected. `p` takes the code as
   an argument because the callers that clean up (a `close`, an `fclose`)
   between the failing call and the panic would otherwise let that cleanup
   overwrite the `errno` being reported. */
_Noreturn static void kelp_panic_at(const char* msg, int p) {
    fprintf(stderr, "panic: %s\n", msg);
    fprintf(stderr, "  errno %d: %s\n", p, strerror(p));
    exit(1);
}

_Noreturn static void kelp_panic_errno(const char* msg) {
    kelp_panic_at(msg, errno);
}

/* `getaddrinfo` reports through its own return code, not `errno`, so a DNS
   failure gets `gai_strerror` instead of a misleading `errno 0: Success`. */
_Noreturn static void kelp_panic_gai(const char* msg, int rc) {
    fprintf(stderr, "panic: %s\n", msg);
    fprintf(stderr, "  getaddrinfo: %s\n", gai_strerror(rc));
    exit(1);
}

/* The process's own argv, saved by the emitted `main` (S2). Both live for
   the whole process, so a `&str` view of `kelp_argv[i]` stays valid for as long
   as any Kelp value can hold it — which is what `process.arg_at` hands out. */
static int kelp_argc = 0;
static char** kelp_argv = NULL;

/* The builtin `print` and string templates go through stdio's buffer while
   `std/io`'s `File` writes straight to the descriptor, so a write to the
   process's own stdout or stderr has to flush that buffer first — otherwise
   the two APIs are observed out of order whenever stdout is not a terminal
   (stdio block-buffers a pipe). A no-op for every other descriptor. */
static void kelp_io_flush_stdio(int fd) {
    if (fd == 1) {
        fflush(stdout);
    } else if (fd == 2) {
        fflush(stderr);
    }
}

static void kelp_assert(bool cond) {
    if (!cond) {
        kelp_panic("assertion failed");
    }
}

/* ---- kelp_View (A-8) --------------------------------------------------------
   A length-carrying borrowed text view: a pointer and a length. `str` is a
   bare `const char*`, so a scan re-derives its length with `strlen` and a
   slice has to allocate; a view carries both, which is what lets the JSON
   reader in this repository stop copying its whole input to hold on to it.

   It borrows, so it follows `str`'s rules exactly: never a `datac` field, an
   enum payload, a `List` element or a `Map` key, and never an implicit
   conversion to `str` — a slice has no NUL after it, so anything that reads
   text as a C string would run past the end. Every operation below is
   length-aware, and `kelp_View__to_string` is the one way to make a `str` out
   of a slice. */
typedef struct {
    const char* ptr;
    size_t len;
} kelp_View;

static size_t kelp_View__len(kelp_View self) {
    return self.len;
}

static bool kelp_View__is_empty(kelp_View self) {
    return self.len == 0;
}

static uint8_t kelp_View__get(kelp_View self, size_t i) {
    if (i >= self.len) {
        kelp_panic("View.get: index out of bounds");
    }
    return (uint8_t)self.ptr[i];
}

static kelp_View kelp_View__slice(kelp_View self, size_t from, size_t to) {
    if (from > to || to > self.len) {
        kelp_panic("View.slice: index out of bounds");
    }
    kelp_View v;
    v.ptr = self.ptr + from;
    v.len = to - from;
    return v;
}

/* Byte-wise, length-aware: `memcmp` over the shorter prefix would call
   "ab" and "abc" equal, so the lengths are compared first. */
static bool kelp_View__eq(kelp_View self, kelp_View other) {
    if (self.len != other.len) {
        return false;
    }
    return self.len == 0 || memcmp(self.ptr, other.ptr, self.len) == 0;
}

/* `s.view()` — a view of a whole `str`. This is the one place a length has
   to be derived from a `str`, because a `str` does not carry one; that cost
   is what the type exists to stop paying everywhere else. The name is
   `kelp_str__view` because `infer.rs` keys the method `str#view`. */
static kelp_View kelp_str__view(const char* s) {
    kelp_View v;
    v.ptr = s;
    v.len = s == NULL ? 0 : strlen(s);
    return v;
}

/* The one growth routine both owned buffers share: make room for `extra`
   more bytes past `len`, doubling (or jumping to 8) so appending is
   amortized O(1), and re-terminate. `data`/`cap` are passed by pointer
   because `realloc` may move the buffer. */
static void kelp_buf_reserve(unsigned char** data, size_t len, size_t* cap, size_t extra) {
    if (len + extra <= *cap) {
        return;
    }
    size_t want = *cap == 0 ? 8 : *cap;
    while (want < len + extra) {
        want *= 2;
    }
    unsigned char* p = (unsigned char*)realloc(*data, want + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    *data = p;
    *cap = want;
    p[len] = 0;
}

static bool kelp_utf8_valid(const unsigned char* p, size_t n) {
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

static kelp_Bytes kelp_Bytes__empty(void) {
    /* One allocated zero byte rather than a NULL pointer, so an empty buffer
       still has a valid NUL terminator at `data[0]`: that is the same
       invariant every non-empty buffer keeps (`data[len] == 0`), and it means
       `as_str()` and `printf("%s")` can never be handed NULL. */
    unsigned char* p = (unsigned char*)malloc(1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    p[0] = 0;
    kelp_Bytes b;
    b.data = p;
    b.len = 0;
    b.cap = 0;
    return b;
}

static kelp_Bytes kelp_Bytes__from_str(const char* s) {
    size_t n = strlen(s);
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    memcpy(p, s, n + 1);
    kelp_Bytes b;
    b.data = p;
    b.len = n;
    b.cap = n;
    return b;
}

static size_t kelp_Bytes__len(const kelp_Bytes* self) {
    return self->len;
}

static bool kelp_Bytes__is_empty(const kelp_Bytes* self) {
    return self->len == 0;
}

static uint8_t kelp_Bytes__get(const kelp_Bytes* self, size_t i) {
    if (i >= self->len) {
        kelp_panic("Bytes.get: index out of bounds");
    }
    return self->data[i];
}

static void kelp_Bytes__set(kelp_Bytes* self, size_t i, uint8_t value) {
    if (i >= self->len) {
        kelp_panic("Bytes.set: index out of bounds");
    }
    self->data[i] = value;
}

static kelp_Bytes kelp_Bytes__clone(const kelp_Bytes* self) {
    unsigned char* p = (unsigned char*)malloc(self->len + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    memcpy(p, self->data, self->len);
    p[self->len] = 0;
    kelp_Bytes b;
    b.data = p;
    b.len = self->len;
    b.cap = self->len;
    return b;
}

static void kelp_Bytes__push(kelp_Bytes* self, uint8_t byte) {
    kelp_buf_reserve(&self->data, self->len, &self->cap, 1);
    self->data[self->len++] = byte;
    self->data[self->len] = 0;
}

static void kelp_Bytes__append(kelp_Bytes* self, const kelp_Bytes* other) {
    if (other->len == 0) {
        return;
    }
    kelp_buf_reserve(&self->data, self->len, &self->cap, other->len);
    memcpy(self->data + self->len, other->data, other->len);
    self->len += other->len;
    self->data[self->len] = 0;
}

static void kelp_Bytes__reserve(kelp_Bytes* self, size_t extra) {
    kelp_buf_reserve(&self->data, self->len, &self->cap, extra);
}

static void kelp_Bytes__clear(kelp_Bytes* self) {
    self->len = 0;
    if (self->data != NULL) {
        self->data[0] = 0;
    }
}

static void kelp_bytes_drop(kelp_Bytes* b) {
    free(b->data);
    b->data = NULL;
    b->len = 0;
}

// ---- kelp_String (M6.2) ------------------------------------------------------
// Same shape as kelp_Bytes, kept a distinct type so text and raw bytes are not
// interchangeable. `from_bytes`/`into_bytes` transfer ownership of the same
// buffer rather than copying it.
typedef struct {
    unsigned char* data;
    size_t len;
    /* Same invariant as `kelp_Bytes.cap`. */
    size_t cap;
} kelp_String;

static kelp_String kelp_String__empty(void) {
    /* As `kelp_Bytes__empty`: an empty string points at a NUL byte, never NULL. */
    unsigned char* p = (unsigned char*)malloc(1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    p[0] = 0;
    kelp_String s;
    s.data = p;
    s.len = 0;
    s.cap = 0;
    return s;
}

static kelp_String kelp_String__from_str(const char* text) {
    size_t n = strlen(text);
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    memcpy(p, text, n + 1);
    kelp_String s;
    s.data = p;
    s.len = n;
    s.cap = n;
    return s;
}

/* Takes ownership of `b`'s buffer: the caller's binding is moved into this
   call, so nothing else frees it. */
static kelp_String kelp_String__from_bytes(kelp_Bytes b) {
    if (!kelp_utf8_valid(b.data, b.len)) {
        kelp_panic("String.from_bytes: input is not valid utf-8");
    }
    kelp_String s;
    s.data = b.data;
    s.len = b.len;
    s.cap = b.cap;
    return s;
}

/* Hands the buffer to the caller: the receiver is moved into this call. */
static kelp_Bytes kelp_String__into_bytes(kelp_String self) {
    kelp_Bytes b;
    b.data = self.data;
    b.len = self.len;
    b.cap = self.cap;
    return b;
}

static size_t kelp_String__len(const kelp_String* self) {
    return self->len;
}

static bool kelp_String__is_empty(const kelp_String* self) {
    return self->len == 0;
}

static size_t kelp_String__char_count(const kelp_String* self) {
    size_t n = 0;
    for (size_t i = 0; i < self->len; i++) {
        if ((self->data[i] & 0xC0) != 0x80) {
            n++;
        }
    }
    return n;
}

static kelp_String kelp_String__clone(const kelp_String* self) {
    unsigned char* p = (unsigned char*)malloc(self->len + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    memcpy(p, self->data, self->len);
    p[self->len] = 0;
    kelp_String s;
    s.data = p;
    s.len = self->len;
    s.cap = self->len;
    return s;
}

/* One byte, unchecked: `String.push` is a byte append, matching
   `String.len`'s byte length (a multi-byte codepoint goes through
   `str`/`utf8` helpers, not here). */
static void kelp_String__push(kelp_String* self, char ch) {
    kelp_buf_reserve(&self->data, self->len, &self->cap, 1);
    self->data[self->len++] = (unsigned char)ch;
    self->data[self->len] = 0;
}

static void kelp_String__push_str(kelp_String* self, const char* text) {
    size_t n = strlen(text);
    if (n == 0) {
        return;
    }
    kelp_buf_reserve(&self->data, self->len, &self->cap, n);
    memcpy(self->data + self->len, text, n);
    self->len += n;
    self->data[self->len] = 0;
}

static void kelp_String__reserve(kelp_String* self, size_t extra) {
    kelp_buf_reserve(&self->data, self->len, &self->cap, extra);
}

static kelp_String kelp_String__with_capacity(size_t n) {
    kelp_String s;
    s.data = NULL;
    s.len = 0;
    s.cap = 0;
    kelp_buf_reserve(&s.data, 0, &s.cap, n);
    return s;
}

static void kelp_String__clear(kelp_String* self) {
    self->len = 0;
    if (self->data != NULL) {
        self->data[0] = 0;
    }
}

/* A borrowed view of the same buffer (M6.3). Every constructor keeps the
   buffer NUL-terminated, which is what a `str` (a `const char*`) requires. */
static const char* kelp_String__as_str(const kelp_String* self) {
    return (const char*)self->data;
}

/* A length-carrying view of the same buffer (A-8). No `strlen`: the length is
   already here, which is the whole point — an owned `String` is the cheapest
   possible source of a `View`. */
static kelp_View kelp_String__view(const kelp_String* self) {
    kelp_View v;
    v.ptr = (const char*)self->data;
    v.len = self->len;
    return v;
}

/* The one safe conversion from a `View` to a `String`: copy `len` bytes and
   terminate the copy. The source is not required to be terminated, which is
   exactly why this allocates rather than handing the pointer out — and why
   the conversion is not the `String.from_str` an implicit one would use. It
   lives here, after `kelp_String`, because it builds one. */
static kelp_String kelp_View__to_string(kelp_View self) {
    unsigned char* p = (unsigned char*)malloc(self.len + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    if (self.len > 0) {
        memcpy(p, self.ptr, self.len);
    }
    p[self.len] = 0;
    kelp_String s;
    s.data = p;
    s.len = self.len;
    s.cap = self.len;
    return s;
}

static const char* kelp_Bytes__as_str(const kelp_Bytes* self) {
    if (!kelp_utf8_valid(self->data, self->len)) {
        kelp_panic("Bytes.as_str: input is not valid utf-8");
    }
    return (const char*)self->data;
}

static void kelp_string_drop(kelp_String* s) {
    free(s->data);
    s->data = NULL;
    s->len = 0;
}

// ---- kelp_Map helpers (MK5-S5) -----------------------------------------------
// A map's key type decides how it is hashed and compared, and each
// instantiation emits its own pair of static helpers from these primitives.
// For a `String` key that is FNV-1a over the key's bytes plus a length-aware
// equality against a borrowed `str` (`memcmp` rather than `strcmp`, because a
// `String` key may hold bytes `strlen` would stop at); for a scalar key it is
// a mix of the value and plain `==`.
static uint64_t kelp_map_hash(const char* data, size_t len) {
    uint64_t h = 1469598103934665603ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= (unsigned char)data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static bool kelp_map_key_eq(const kelp_String* key, const char* data, size_t len) {
    if (key->len != len) {
        return false;
    }
    return len == 0 || memcmp(key->data, data, len) == 0;
}

/* The scalar-key hash: a 64-bit finalizer over the widened value, so keys
   that differ in one bit land in different buckets. */
static uint64_t kelp_map_mix(uint64_t x) {
    x ^= x >> 33;
    x *= 0xff51afd7ed558ccdULL;
    x ^= x >> 33;
    return x;
}

/* The allocation primitive `std/mem` wraps (M6.2): an owned, zero-filled,
   NUL-terminated buffer of `n` bytes. */
static kelp_Bytes kelp_bytes_alloc_raw(size_t n) {
    unsigned char* p = (unsigned char*)malloc(n + 1);
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    memset(p, 0, n + 1);
    kelp_Bytes b;
    b.data = p;
    b.len = n;
    b.cap = n;
    return b;
}
// ---- end Kelp runtime -------------------------------------------------------
"#;

/// Lexical path normalization, shared by `path.normalize` (M6.3) and the
/// capability check (M7) — the plan requires exactly one implementation,
/// because the security-relevant normalization must not be a second,
/// slightly different function. Returns false when the path is not absolute
/// or escapes its own root via `..`.
const PATH_RUNTIME: &str = r#"// ---- Kelp path runtime -------------------------------------------------------
static bool kelp_path_normalize(const char* in, char* out, size_t cap) {
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
// ---- end Kelp path runtime ---------------------------------------------------
"#;

/// The capability check itself (M7). Emitted only for programs that contain a
/// gated call, since `kelp_path_normalize` is the only piece they share.
const CAP_RUNTIME: &str = r#"// ---- Kelp capability runtime (M7) --------------------------------------------
/* `*` matches within one path segment, `**` crosses segments and is legal
   only as a whole segment; no other metacharacters, no regex. */
static bool kelp_glob_match(const char* pat, const char* path) {
    while (*pat != 0) {
        /* A trailing '/**' also matches the directory itself: `**` is zero
           or more segments, and `fs.mkdir_all`/`fs.remove_all` walk up to
           (and back down to) the grant root, so a grant on a tree has to
           admit the tree's own root. */
        if (pat[0] == '/' && pat[1] == '*' && pat[2] == '*' && pat[3] == 0 && *path == 0) {
            return true;
        }
        if (pat[0] == '*' && pat[1] == '*') {
            const char* rest = pat + 2;
            if (*rest == '/') {
                rest++;
            }
            if (*rest == 0) {
                return true;   /* a trailing '**' matches everything below */
            }
            for (const char* p = path; ; p++) {
                if (kelp_glob_match(rest, p)) {
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
                if (kelp_glob_match(rest, p)) {
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
static bool kelp_resolve_path(const char* path, char* out, size_t cap) {
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
    if (!kelp_resolve_path(buf, parent, sizeof parent)) {
        return false;
    }
    snprintf(out, cap, "%s/%s", parent, slash + 1);
    return true;
}

static void kelp_cap_deny(const char* domain, const char* path, const char* const* patterns,
                       size_t count, const char* module, const char* at) {
    fprintf(stderr, "%s: panic: '%s' is outside the %s capability granted to module '%s'\n",
            at, path, domain, module);
    if (count == 0) {
        fprintf(stderr, "  granted: (nothing)\n");
    } else {
        fprintf(stderr, "  granted:\n");
        for (size_t i = 0; i < count; i++) {
            fprintf(stderr, "    %s\n", patterns[i]);
        }
    }
    exit(1);
}

/* A path argument: lexically normalized, then resolved through symlinks, and
   only then matched — the order the plan insists on, because a link inside
   the grant that points outside it must be denied. */
static void kelp_cap_check_path(const char* domain, const char* path, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    char normalized[PATH_MAX];
    if (!kelp_path_normalize(path, normalized, sizeof normalized)) {
        return kelp_cap_deny(domain, path, patterns, count, module, at);
    }
    char resolved[PATH_MAX];
    if (!kelp_resolve_path(normalized, resolved, sizeof resolved)) {
        snprintf(resolved, sizeof resolved, "%s", normalized);
    }
    for (size_t i = 0; i < count; i++) {
        if (kelp_glob_match(patterns[i], resolved)) {
            return;
        }
    }
    return kelp_cap_deny(domain, resolved, patterns, count, module, at);
}

/* A name argument (`env`'s variable names): matched as written, since a name
   has no path structure to normalize. */
static void kelp_cap_check_name(const char* domain, const char* name, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    for (size_t i = 0; i < count; i++) {
        if (kelp_glob_match(patterns[i], name)) {
            return;
        }
    }
    return kelp_cap_deny(domain, name, patterns, count, module, at);
}

/* ---- net (S3): a host, or a host and a port ------------------------------
   the plan's vocabulary: `host`, `host:port`, `*.host`, `*:port`, `*`. `*`
   matches within one label — it does not cross `.` or `:` — so `*.example.com`
   is one label plus a domain, and a lone `*` (handled before this) is the
   whole-resource wildcard the entry module holds. There is no `**` here:
   there is nothing for it to mean that `*` does not already say. */
static bool kelp_host_glob(const char* pat, const char* text) {
    while (*pat != 0) {
        if (*pat == '*') {
            const char* rest = pat + 1;
            for (const char* p = text; ; p++) {
                if (kelp_host_glob(rest, p)) {
                    return true;
                }
                if (*p == 0 || *p == '.' || *p == ':') {
                    return false;
                }
            }
        }
        if (*pat != *text) {
            return false;
        }
        pat++;
        text++;
    }
    return *text == 0;
}

/* The host part of `hostport` ("h:p" -> "h"), copied into `out`. */
static void kelp_host_part(const char* hostport, char* out, size_t cap) {
    const char* colon = strrchr(hostport, ':');
    size_t n = colon == NULL ? strlen(hostport) : (size_t)(colon - hostport);
    if (n >= cap) {
        n = cap - 1;
    }
    memcpy(out, hostport, n);
    out[n] = 0;
}

/* Whether `pat` admits the resource `hostport`. The match is on the string
   the code wrote — no DNS before matching, which is the rule that keeps a
   grant for `example.com` from covering `93.184.216.34` (`fs` resolves
   symlinks first for the opposite reason: a link is not a name the author
   chose). A pattern with no port admits any port. */
static bool kelp_host_match(const char* pat, const char* hostport) {
    if (strcmp(pat, "*") == 0) {
        return true;
    }
    if (strrchr(pat, ':') == NULL) {
        char host[PATH_MAX];
        kelp_host_part(hostport, host, sizeof host);
        return kelp_host_glob(pat, host);
    }
    return kelp_host_glob(pat, hostport);
}

/* A host on its own (`net.resolve`): resolution is not port-specific, so a
   pattern's port, if it has one, is ignored. */
static void kelp_cap_check_host(const char* domain, const char* host, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    for (size_t i = 0; i < count; i++) {
        const char* pat = patterns[i];
        if (strcmp(pat, "*") == 0) {
            return;
        }
        if (kelp_host_glob(pat, host)) {
            return;
        }
        const char* colon = strrchr(pat, ':');
        if (colon != NULL) {
            char head[PATH_MAX];
            size_t n = (size_t)(colon - pat);
            if (n >= sizeof head) {
                n = sizeof head - 1;
            }
            memcpy(head, pat, n);
            head[n] = 0;
            if (kelp_host_glob(head, host)) {
                return;
            }
        }
    }
    return kelp_cap_deny(domain, host, patterns, count, module, at);
}

/* A host and a port together. The port is the *call's* argument, so the
   checked string is the one the code named, not a peer address. */
static void kelp_cap_check_hostport(const char* domain, const char* host, uint16_t port,
                                 const char* const* patterns, size_t count,
                                 const char* module, const char* at) {
    char hostport[PATH_MAX];
    snprintf(hostport, sizeof hostport, "%s:%u", host, (unsigned)port);
    for (size_t i = 0; i < count; i++) {
        if (kelp_host_match(patterns[i], hostport)) {
            return;
        }
    }
    return kelp_cap_deny(domain, hostport, patterns, count, module, at);
}

/* A port on its own (`net.listen`). The grant's port is what governs a
   listener: `net.listen` binds every interface (there is no
   `net.listen(host, port)` in v1), so the port is the resource, and a pattern
   with no port at all constrains none. See `std/net.kp` for why. */
static void kelp_cap_check_port(const char* domain, uint16_t port, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    char text[32];
    char number[16];
    snprintf(number, sizeof number, "%u", (unsigned)port);
    snprintf(text, sizeof text, "*:%s", number);
    for (size_t i = 0; i < count; i++) {
        const char* pat = patterns[i];
        if (strcmp(pat, "*") == 0) {
            return;
        }
        const char* colon = strrchr(pat, ':');
        if (colon == NULL) {
            return;   /* a host-only pattern constrains no port */
        }
        if (kelp_host_glob(colon + 1, number)) {
            return;
        }
    }
    return kelp_cap_deny(domain, text, patterns, count, module, at);
}

// ---- end Kelp capability runtime ---------------------------------------------
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
            "kelp_caps_{}_{}",
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
    let owned = owned_types(&program.structs, &program.enums);
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
    out.push_str("#include <errno.h>\n");
    out.push_str("#include <limits.h>\n");
    out.push_str("#include <dirent.h>\n");
    out.push_str("#include <fcntl.h>\n");
    out.push_str("#include <arpa/inet.h>\n");
    out.push_str("#include <math.h>\n");
    out.push_str("#include <netdb.h>\n");
    out.push_str("#include <netinet/in.h>\n");
    out.push_str("#include <poll.h>\n");
    out.push_str("#include <sys/socket.h>\n");
    out.push_str("#include <sys/sendfile.h>\n");
    out.push_str("#include <sys/stat.h>\n");
    out.push_str("#include <sys/types.h>\n");
    out.push_str("#include <sys/wait.h>\n");
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
    } = collection;
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
    if !containers.is_empty() {
        write_tags(&mut out, &program.structs, &program.enums);
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
    enum Decl<'a> {
        Record(&'a StructDef),
        Sum(&'a EnumDef),
    }
    let mut decls: Vec<(usize, Decl)> = program
        .structs
        .iter()
        .map(|s| (s.order, Decl::Record(s)))
        .chain(program.enums.iter().map(|e| (e.order, Decl::Sum(e))))
        .collect();
    decls.sort_by_key(|(order, _)| *order);
    for (_, decl) in &decls {
        match decl {
            Decl::Record(s) => write_struct(&mut out, s, &owned),
            Decl::Sum(e) => write_enum(&mut out, e, &owned),
        }
    }
    if !decls.is_empty() {
        out.push('\n');
    }

    for ty in &containers {
        match ty {
            Ty::List(elem) => write_list(&mut out, elem, &owned),
            Ty::Box(elem) => write_box(&mut out, elem, &owned),
            Ty::Map(key, value) => write_map(&mut out, key, value, &owned),
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

    write_pd_globals(&mut out, &program.pds, &program.enums, drops, &owned, sources);

    for f in &program.functions {
        if f.extern_ {
            write_extern(&mut out, f);
            out.push('\n');
            continue;
        }
        let mut gen = Gen::new(
            drops,
            &owned,
            &program.enums,
            f.name.clone(),
            f.module.clone(),
            &caps,
            sources,
        );
        gen.write_function(f, !program.pds.is_empty());
        out.push_str(&gen.out);
        out.push('\n');
    }

    out
}

/// Emits an `extern` declaration's body, which comes from the compiler rather
/// than from `.kp` source (see `externs.rs`). `infer.rs` has already rejected
/// an `extern` with no known body, so reaching the `expect` here would mean
/// the table and the checker disagreed.
fn write_extern(out: &mut String, f: &Function) {
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

/// Emits one file-scope global per passdown plus the generated function that
/// initializes them, in declaration order (M5.2). The global's name is the
/// same `pd#...` name a referencing function takes as a hidden parameter, so
/// inside such a function the parameter shadows its own global and both
/// spell the same symbol in C — including inside `main`, which can take no
/// parameters and therefore reads the globals directly.
fn write_pd_globals(
    out: &mut String,
    pds: &[Pd],
    enums: &[EnumDef],
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
    let gen = Gen::new(
        drops,
        owned,
        enums,
        String::new(),
        String::new(),
        &no_caps,
        sources,
    );
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
fn write_enum(out: &mut String, e: &EnumDef, owned: &HashSet<String>) {
    write!(out, "typedef struct {} {{\n", crate::types::struct_tag(&e.name)).unwrap();
    out.push_str("    int32_t kelp_tag;\n");
    out.push_str("    union {\n");
    for v in &e.variants {
        writeln!(out, "        struct {{").unwrap();
        if v.fields.is_empty() {
            out.push_str("            char kelp_none;\n");
        }
        for f in &v.fields {
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
}

/// A container's C typedef, emitted before every `datac`: both containers
/// hold their contents through a pointer, so a container can be named by the
/// `datac`s that use it — including the `datac` whose *own* body has not been
/// emitted yet (`List<Tree>` inside `Tree`) and a mutually recursive pair
/// (M8). A `datac` element is therefore spelled by its struct tag here
/// (`container_elem_c_type`), which is forward-declared by `write_tags`.
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
        // open-addressed index of `entry index + 1` (0 empty, `SIZE_MAX`
        // tombstone), because `keys()` must return insertion order. The key
        // sits inline (`kelp_String` is complete in the runtime preamble) and
        // the value sits behind a pointer, so this typedef can still be
        // emitted before every `datac` — a `datac` value is named by its
        // forward-declared struct tag.
        Ty::Map(key, value) => {
            let name = crate::types::map_c_type(key, value);
            let entry = format!("{name}_entry");
            writeln!(
                out,
                "typedef struct {{\n    {} key;\n    {}* value;\n    uint8_t live;\n}} {entry};",
                key.c_type(),
                crate::types::container_elem_c_type(value)
            )
            .unwrap();
            writeln!(
                out,
                "typedef struct {{\n    {entry}* entries;\n    size_t entry_count;\n    size_t entry_cap;\n    size_t len;\n    size_t* index;\n    size_t index_cap;\n    size_t index_used;\n}} {name};"
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

/// `struct tag_Foo;` for every `datac`, so a container typedef can hold a
/// pointer to a struct whose body comes later.
fn write_tags(out: &mut String, structs: &[StructDef], enums: &[EnumDef]) {
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

/// The C name of a container instantiation, without the `kelp_` prefix the
/// functions append: `kelp_List__i32` / `kelp_Box__Node`.
fn container_name(ty: &Ty) -> String {
    match ty {
        Ty::List(elem) => crate::types::list_c_type(elem),
        Ty::Box(elem) => crate::types::box_c_type(elem),
        Ty::Map(key, value) => crate::types::map_c_type(key, value),
        Ty::Closure { .. } => ty.c_type(),
        other => unreachable!("not a container: {}", other.display()),
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

/// One `Map<K, V>` instantiation's function set (MK5-S5). Entries live in
/// insertion order in a dense array, so `keys()` returns insertion order
/// rather than hash order; a separate open-addressed index of `entry index +
/// 1` (`0` empty, `SIZE_MAX` tombstone) does the lookup. `clear()` resets
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
/// by mixing its bits, passed by value, and compared with `==` (A-2).
fn write_map(out: &mut String, key: &Ty, value: &Ty, owned: &HashSet<String>) {
    let map = crate::types::map_c_type(key, value);
    let map_ty = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
    let entry = format!("{map}_entry");
    let value_c = value.c_type();
    let value_drops = needs_drop(value, owned);
    let drop_value = value_drops.then(|| drop_fn_name(value));
    let list_key = crate::types::list_c_type(key);
    let list_value = crate::types::list_c_type(value);
    let clonable = crate::types::list_clonable_elem(value);
    // Reading a value *out* of the map is a copy for a `Copy` value and a
    // clone for `String`/`Bytes` (a `datac` cannot be cloned, and `infer.rs`
    // never offers `get_or`/`values` for one).
    let copy_from = |ptr: &str| match value {
        Ty::String => format!("kelp_String__clone({ptr})"),
        Ty::Bytes => format!("kelp_Bytes__clone({ptr})"),
        _ => format!("*{ptr}"),
    };
    // A `String` key reaches the map as a `str` view (the map copies it in);
    // a scalar key is already the value the map stores.
    let key_is_string = key == &Ty::String;
    let key_arg_c = if key_is_string {
        "const char*".to_string()
    } else {
        key.c_type()
    };
    // Hashing a key *argument* (a `str` view) and hashing a *stored* key
    // (`kelp_String`) read the bytes differently, so they are two helpers.
    let hash_arg = |a: &str| {
        if key_is_string {
            format!("kelp_map_hash({a}, strlen({a}))")
        } else {
            format!("kelp_map_mix((uint64_t)({a}))")
        }
    };
    let hash_stored = |s: &str| {
        if key_is_string {
            format!("kelp_map_hash((const char*){s}.data, {s}.len)")
        } else {
            format!("kelp_map_mix((uint64_t)({s}))")
        }
    };
    // Each of these takes a stored key as an *lvalue*, and adds the `&` a
    // `kelp_String` needs itself — a scalar key is compared and dropped by
    // value, and taking its address would be a different expression.
    let eq_stored = |stored: &str, a: &str| {
        if key_is_string {
            format!("kelp_map_key_eq(&{stored}, {a}, strlen({a}))")
        } else {
            format!("({stored} == {a})")
        }
    };
    let store_key = |a: &str| {
        if key_is_string {
            format!("kelp_String__from_str({a})")
        } else {
            a.to_string()
        }
    };
    let clone_key = |s: &str| {
        if key_is_string {
            format!("kelp_String__clone(&{s})")
        } else {
            s.to_string()
        }
    };
    // A statement, terminator included: every caller writes it as one.
    let drop_key = |s: &str| {
        if key_is_string {
            format!("kelp_string_drop(&{s});")
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
    out.push_str("    for (;;) {\n        size_t slot = self->index[i];\n        if (slot == 0) {\n            return tomb != SIZE_MAX ? tomb : i;\n        }\n");
    out.push_str("        if (slot == SIZE_MAX) {\n            if (tomb == SIZE_MAX) {\n                tomb = i;\n            }\n        } else {\n");
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
    writeln!(out, "    free(self->index);\n    self->index = (size_t*)malloc(cap * sizeof(size_t));").unwrap();
    out.push_str("    if (self->index == NULL) {\n        kelp_panic(\"out of memory\");\n    }\n    memset(self->index, 0, cap * sizeof(size_t));\n");
    out.push_str("    self->index_cap = cap;\n    self->index_used = 0;\n");
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    writeln!(
        out,
        "        size_t i = (size_t){} & (cap - 1);",
        hash_stored("self->entries[e].key")
    )
    .unwrap();
    out.push_str("        while (self->index[i] != 0) {\n            i = (i + 1) & (cap - 1);\n        }\n        self->index[i] = e + 1;\n        self->index_used++;\n    }\n}\n\n");

    writeln!(out, "static void {map}__insert({map}* self, {key_arg_c} k, {value_c} v) {{").unwrap();
    writeln!(out, "    if (self->index_cap == 0 || (self->index_used + 1) * 4 >= self->index_cap * 3) {{\n        {map}__rehash(self, self->len + 1);\n    }}").unwrap();
    out.push_str("    bool found = false;\n");
    writeln!(out, "    size_t slot = {map}__find(self, k, &found);").unwrap();
    out.push_str("    if (found) {\n        size_t e = self->index[slot] - 1;\n");
    if value_drops {
        writeln!(out, "        {}(self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    out.push_str("        *self->entries[e].value = v;\n        return;\n    }\n");
    out.push_str("    if (self->entry_count == self->entry_cap) {\n        size_t cap = self->entry_cap == 0 ? 8 : self->entry_cap * 2;\n");
    writeln!(out, "        self->entries = ({entry}*)realloc(self->entries, cap * sizeof({entry}));").unwrap();
    out.push_str("        if (self->entries == NULL) {\n            kelp_panic(\"out of memory\");\n        }\n        self->entry_cap = cap;\n    }\n");
    out.push_str("    size_t e = self->entry_count++;\n");
    writeln!(out, "    self->entries[e].key = {};", store_key("k")).unwrap();
    writeln!(out, "    self->entries[e].value = ({value_c}*)malloc(sizeof({value_c}));").unwrap();
    out.push_str("    if (self->entries[e].value == NULL) {\n        kelp_panic(\"out of memory\");\n    }\n");
    out.push_str("    *self->entries[e].value = v;\n    self->entries[e].live = 1;\n");
    out.push_str("    if (self->index[slot] == 0) {\n        self->index_used++;\n    }\n    self->index[slot] = e + 1;\n    self->len++;\n}\n\n");

    writeln!(out, "static bool {map}__has(const {map}* self, {key_arg_c} k) {{\n    bool found = false;\n    {map}__find(self, k, &found);\n    return found;\n}}\n").unwrap();

    if value.is_copy() {
        writeln!(out, "static {value_c} {map}__get(const {map}* self, {key_arg_c} k) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.get: key not found\");\n    }}\n    return *self->entries[self->index[slot] - 1].value;\n}}\n").unwrap();
    } else {
        writeln!(out, "static const {value_c}* {map}__get_const(const {map}* self, {key_arg_c} k) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.get: key not found\");\n    }}\n    return self->entries[self->index[slot] - 1].value;\n}}\n").unwrap();
    }

    if clonable {
        writeln!(out, "static {value_c} {map}__get_or(const {map}* self, {key_arg_c} k, {value_c} fallback) {{").unwrap();
        writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        return fallback;\n    }}").unwrap();
        writeln!(out, "    const {value_c}* existing = self->entries[self->index[slot] - 1].value;\n    {value_c} result = {};", copy_from("existing")).unwrap();
        match value {
            Ty::String => out.push_str("    kelp_string_drop(&fallback);\n"),
            Ty::Bytes => out.push_str("    kelp_bytes_drop(&fallback);\n"),
            _ => out.push_str("    (void)fallback;\n"),
        }
        out.push_str("    return result;\n}\n\n");
    }

    writeln!(out, "static {value_c} {map}__remove({map}* self, {key_arg_c} k) {{").unwrap();
    writeln!(out, "    bool found = false;\n    size_t slot = {map}__find(self, k, &found);\n    if (!found) {{\n        kelp_panic(\"Map.remove: key not found\");\n    }}\n    size_t e = self->index[slot] - 1;").unwrap();
    out.push_str("    ");
    writeln!(out, "{value_c} v = *self->entries[e].value;\n    free(self->entries[e].value);\n    self->entries[e].value = NULL;\n    {}\n    self->entries[e].live = 0;\n    self->index[slot] = SIZE_MAX;\n    self->len--;\n    return v;\n}}\n", drop_key("self->entries[e].key")).unwrap();

    writeln!(out, "static void {map}__clear({map}* self) {{").unwrap();
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    if value_drops {
        writeln!(out, "        {}(self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    writeln!(out, "        free(self->entries[e].value);\n        self->entries[e].value = NULL;\n        {}\n        self->entries[e].live = 0;\n    }}", drop_key("self->entries[e].key")).unwrap();
    out.push_str("    self->entry_count = 0;\n    self->len = 0;\n    self->index_used = 0;\n    if (self->index != NULL) {\n        memset(self->index, 0, self->index_cap * sizeof(size_t));\n    }\n}\n\n");

    writeln!(out, "static size_t {map}__len(const {map}* self) {{\n    return self->len;\n}}\n").unwrap();
    writeln!(out, "static bool {map}__is_empty(const {map}* self) {{\n    return self->len == 0;\n}}\n").unwrap();

    writeln!(out, "static {list_key} {map}__keys(const {map}* self) {{").unwrap();
    writeln!(out, "    {list_key} out = {list_key}__empty();\n    {list_key}__reserve(&out, self->len);").unwrap();
    writeln!(
        out,
        "    for (size_t e = 0; e < self->entry_count; e++) {{\n        if (self->entries[e].live) {{\n            out.data[out.len++] = {};\n        }}\n    }}\n    return out;\n}}\n",
        clone_key("self->entries[e].key")
    )
    .unwrap();

    if clonable {
        writeln!(out, "static {list_value} {map}__values(const {map}* self) {{").unwrap();
        writeln!(out, "    {list_value} out = {list_value}__empty();\n    {list_value}__reserve(&out, self->len);").unwrap();
        writeln!(out, "    for (size_t e = 0; e < self->entry_count; e++) {{\n        if (self->entries[e].live) {{\n            out.data[out.len++] = {};\n        }}\n    }}\n    return out;\n}}\n", copy_from("self->entries[e].value")).unwrap();
    }

    writeln!(out, "static void {}({map}* self) {{", drop_fn_name(&map_ty)).unwrap();
    out.push_str("    for (size_t e = 0; e < self->entry_count; e++) {\n        if (!self->entries[e].live) {\n            continue;\n        }\n");
    if value_drops {
        writeln!(out, "        {}(self->entries[e].value);", drop_value.as_deref().unwrap()).unwrap();
    }
    writeln!(out, "        free(self->entries[e].value);\n        {}\n    }}", drop_key("self->entries[e].key")).unwrap();
    out.push_str("    free(self->entries);\n    free(self->index);\n    self->entries = NULL;\n    self->entry_count = 0;\n    self->entry_cap = 0;\n    self->len = 0;\n    self->index = NULL;\n    self->index_cap = 0;\n    self->index_used = 0;\n}\n\n");
}

/// One `List<T>` instantiation's function set: a growable buffer plus the
/// operations on it.
fn write_list(out: &mut String, elem: &Ty, owned: &HashSet<String>) {
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

    write_list_s1(out, elem, &list, &elem_c, owned);

    writeln!(out, "static void {}({list}* self) {{", crate::types::drop_fn_name(&Ty::List(Box::new(elem.clone())))).unwrap();
    writeln!(out, "    {list}__clear(self);\n    free(self->data);\n    self->data = NULL;\n    self->cap = 0;\n}}\n").unwrap();
}

/// The S1 half of a `List<T>` function set: ordering, membership, a copied
/// range, and the two operations that shift elements around. Which of these
/// exist depends on the element type, exactly as `infer::list_sig` decides
/// when it offers the signature — `types::list_sortable_elem` and its
/// siblings are the single source of truth for both passes.
fn write_list_s1(out: &mut String, elem: &Ty, list: &str, elem_c: &str, _owned: &HashSet<String>) {
    if crate::types::list_sortable_elem(elem) {
        // One comparator per element kind, so the sort itself stays one
        // insertion sort. `String`/`Bytes` compare length-aware: `strcmp`
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
            _ => {
                out.push_str("    return (*a > *b) - (*a < *b);\n}\n\n");
            }
        }

        writeln!(out, "static void {list}__sort({list}* self) {{").unwrap();
        out.push_str("    /* insertion sort: stable, no comparator indirection, and\n       in-place for an owned element because the shifts are moves */\n");
        writeln!(out, "    for (size_t i = 1; i < self->len; i++) {{").unwrap();
        writeln!(out, "        {elem_c} v = self->data[i];").unwrap();
        out.push_str("        size_t j = i;\n");
        writeln!(
            out,
            "        while (j > 0 && {list}__cmp(&self->data[j - 1], &v) > 0) {{\n            self->data[j] = self->data[j - 1];\n            j--;\n        }}"
        )
        .unwrap();
        out.push_str("        self->data[j] = v;\n    }\n}\n\n");
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

    if crate::types::list_clonable_elem(elem) {
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
            _ => {
                writeln!(out, "        out.data[out.len++] = self->data[i];").unwrap();
            }
        }
        out.push_str("    }\n    return out;\n}\n\n");
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
            ExprKind::Call { args, .. } => {
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
    if params.is_empty() && first {
        out.push_str("void");
    }
    for (name, ty) in params.iter() {
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&ty.c_decl(&c_name(name)));
    }
}

/// A function type's own parameter list, as C — the same text a declarator
/// needs after the name, so `types.rs` and this emitter agree on one spelling.
fn fn_params_of(params: &[Ty]) -> String {
    if params.is_empty() {
        "void".to_string()
    } else {
        params.iter().map(|p| p.c_type()).collect::<Vec<_>>().join(", ")
    }
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
        }
    }

    /// A nested emitter for a block a value-renderer has to produce: same
    /// plan, same function, same module, its own buffer.
    fn sub(&self) -> Gen<'a> {
        Gen::new(
            self.drops,
            self.owned,
            self.enums,
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
                writeln!(self.out, "{} = {value};", ty.c_decl(&c_name(name))).unwrap();
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
            ExprKind::IntLit(v) => v.to_string(),
            // A character literal is an integer constant with the code point
            // as its value and the type its context asked for, so it emits a
            // cast and nothing else. `u8` is the common case (`c == '"'`),
            // and `char` is a code point rather than a C `char`, so the cast
            // is what makes both fit.
            ExprKind::CharLit(v) => format!("(({})({}))", expr.ty.c_type(), v),
            ExprKind::FloatLit(v) => format!("{v}"),
            ExprKind::StringLit(s) => c_string_literal(s),
            ExprKind::BoolLit(b) => b.to_string(),
            ExprKind::Ident(name) => c_name(name),
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
                let rendered: Vec<String> = args.iter().map(|a| self.expr_value(a)).collect();
                let arglist = rendered.join(", ");
                match closure_access(&callee.ty) {
                    Some((fn_member, env_member)) => {
                        let tmp = fresh_temp();
                        let value = self.expr_value(callee);
                        let code_ptr = format!("{tmp}{fn_member}");
                        let env_ptr = format!("{tmp}{env_member}");
                        let call = if arglist.is_empty() {
                            format!("({code_ptr})({env_ptr})")
                        } else {
                            format!("({code_ptr})({env_ptr}, {arglist})")
                        };
                        format!("({{ {} = {value}; {call}; }})", callee.ty.c_decl(&tmp))
                    }
                    None => {
                        let callee = self.expr_value(callee);
                        format!("({callee})({arglist})")
                    }
                }
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
                            // not leak.
                            if matches!(value.ty, Ty::Bytes | Ty::String) {
                                let tmp = fresh_temp();
                                let rendered = self.expr_value(value);
                                holds.push(format!("{} = {rendered};", value.ty.c_decl(&tmp)));
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
                let payload = if fields.is_empty() {
                    String::new()
                } else {
                    let inits: Vec<String> = fields
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let name = match edef {
                                Some(e) => {
                                    field_name(&e.variants[*variant].fields[i].name)
                                }
                                None => unreachable!("every enum literal names a declared enum"),
                            };
                            format!(".{name} = {}", self.expr_value(f))
                        })
                        .collect();
                    format!(", .kelp_v = {{ .{member} = {{ {} }} }}", inits.join(", "))
                };
                format!("({}){{ .kelp_tag = {variant}{payload} }}", ty.c_type())
            }
            ExprKind::EnumTag { expr: base, variant } => {
                format!("({} == {variant})", self.enum_tag_access(base))
            }
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => {
                // The base's own type names the enum, so nothing has to be
                // threaded through the TIR node beyond the two indices.
                let ename =
                    struct_name_of_ty(&base.ty).expect("an enum field's base is enum-typed");
                let edef = self
                    .enum_def(ename)
                    .expect("an enum field's base names a declared enum");
                let member = variant_member(&edef.variants[*variant].name);
                let fname = field_name(&edef.variants[*variant].fields[*field].name);
                self.enum_member_access(base, &format!("{member}.{fname}"))
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
                if *err_has_payload {
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
                writeln!(out, "    return {};", ret).unwrap();
                writeln!(out, "}}").unwrap();
                writeln!(out, "{}.kelp_v.{}.{};", tmp, ok_member, ok_field).unwrap();
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
                    writeln!(gen.out, "{};", expr.ty.c_decl(&tmp)).unwrap();
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
        use crate::stdlib::CapCheck;

        let Some(domain) = self.caps.gated.get(callee) else {
            return call;
        };
        let Some(patterns) = self.caps.patterns_for(&self.module, domain) else {
            return call;
        };
        let arg_is_str: Vec<bool> = args.iter().map(|a| is_str_like_ty(&a.ty)).collect();
        // `cap_checked_call` is handed the call's key, which for an imported
        // module is qualified (`std.net.connect`); the per-function table in
        // `stdlib.rs` is keyed on the bare name the author wrote.
        let bare = callee.rsplit('.').next().unwrap_or(callee);
        let checked = crate::stdlib::checked_args(domain, bare, &arg_is_str);
        if checked.is_empty() {
            return call;
        }
        let symbol = CapTable::symbol(&self.module, domain);
        // The call site, as the author wrote it. The generated C carries no
        // `#line` directives, so this is the only way a runtime denial can
        // name a `.kp` location.
        let at = self
            .sources
            .locate(&self.module, span)
            .unwrap_or_else(|| self.module.clone());
        let mut checks = String::new();
        for check in checked {
            match check {
                CapCheck::Path(i) => {
                    // A path is normalized lexically, then resolved through
                    // symlinks, and only then matched.
                    let path = self.expr_value(&args[i]);
                    write!(
                        checks,
                        "kelp_cap_check_path(\"{domain}\", {path}, {symbol}, {}, \"{}\", \"{at}\"); ",
                        patterns.len(),
                        self.module
                    )
                    .unwrap();
                }
                CapCheck::Name(i) => {
                    let name = self.expr_value(&args[i]);
                    write!(
                        checks,
                        "kelp_cap_check_name(\"{domain}\", {name}, {symbol}, {}, \"{}\", \"{at}\"); ",
                        patterns.len(),
                        self.module
                    )
                    .unwrap();
                }
                CapCheck::Host(i) => {
                    let host = self.expr_value(&args[i]);
                    write!(
                        checks,
                        "kelp_cap_check_host(\"{domain}\", {host}, {symbol}, {}, \"{}\", \"{at}\"); ",
                        patterns.len(),
                        self.module
                    )
                    .unwrap();
                }
                CapCheck::HostPort(host_at, port_at) => {
                    let host = self.expr_value(&args[host_at]);
                    let port = self.expr_value(&args[port_at]);
                    write!(
                        checks,
                        "kelp_cap_check_hostport(\"{domain}\", {host}, {port}, {symbol}, {}, \"{}\", \"{at}\"); ",
                        patterns.len(),
                        self.module
                    )
                    .unwrap();
                }
                CapCheck::Port(i) => {
                    let port = self.expr_value(&args[i]);
                    write!(
                        checks,
                        "kelp_cap_check_port(\"{domain}\", {port}, {symbol}, {}, \"{}\", \"{at}\"); ",
                        patterns.len(),
                        self.module
                    )
                    .unwrap();
                }
            }
        }
        format!("({{ {checks}{call}; }})")
    }

    fn call_value(&self, callee: &str, args: &[Expr], span: Span, ret: &Ty) -> String {
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
        let mut prelude = String::new();
        let mut args_text: Vec<String> = Vec::new();
        let mut hoisted: Vec<(String, Ty)> = Vec::new();
        for a in args {
            if let ExprKind::Ref { expr: sub, .. } = &a.kind {
                let is_str_view =
                    matches!(&a.ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str));
                if !is_str_view && !is_place(sub) {
                    let tmp = fresh_temp();
                    writeln!(prelude, "{} = {};", sub.ty.c_decl(&tmp), self.expr_value(sub))
                        .unwrap();
                    args_text.push(format!("&{tmp}"));
                    hoisted.push((tmp, sub.ty.clone()));
                    continue;
                }
            }
            args_text.push(self.expr_value(a));
        }

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
        // A map's name carries *two* type arguments, so it is spelled out
        // separately: its key comes from the receiver (or, for `Map.new()`,
        // from the call's own result type) and its value from the same place.
        if let Some(op) = callee.strip_prefix("Map#") {
            let (from_ret, from_args) = if op == "new" {
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
        let call = self.cap_checked_call(callee, s, args, span);
        let owned_hoisted: Vec<&(String, Ty)> = hoisted
            .iter()
            .filter(|(_, ty)| needs_drop(ty, self.owned))
            .collect();
        if prelude.is_empty() {
            return call;
        }
        // The call's value cannot be the statement expression's last
        // statement when a hoisted temporary has to be freed after it (a drop
        // call returns void), so the result goes through a temporary of its
        // own and the frees follow. With nothing hoisted, and only for a
        // non-`unit` result, the call itself can be the last statement.
        if owned_hoisted.is_empty() && *ret != Ty::Unit {
            return format!("({{ {prelude}{call}; }})");
        }
        let mut body = prelude;
        if *ret == Ty::Unit {
            writeln!(body, "{call};").unwrap();
        } else {
            let result_tmp = fresh_temp();
            writeln!(body, "{} {result_tmp} = {call};", ret.c_type()).unwrap();
            for (tmp, ty) in &owned_hoisted {
                writeln!(body, "{}(&{tmp});", drop_fn_name(ty)).unwrap();
            }
            writeln!(body, "{result_tmp};").unwrap();
        }
        if *ret == Ty::Unit {
            for (tmp, ty) in &owned_hoisted {
                writeln!(body, "{}(&{tmp});", drop_fn_name(ty)).unwrap();
            }
        }
        format!("({{ {body}}})")
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
        let base_text = self.expr_value(base);
        if matches!(base.ty, Ty::Ref { .. }) {
            format!("({base_text})->kelp_v.{member}")
        } else {
            format!("({base_text}).kelp_v.{member}")
        }
    }

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
            Ty::Unit | Ty::Struct(_) | Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Ref { .. } => {
                unreachable!("type checker rejects printing unit/struct/container/reference values")
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

fn c_string_literal(s: &str) -> String {
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

/// The full range of an integer type, as `(min, max)` — `min` signed so a
/// signed type can express its own floor, `max` unsigned so `u64`/`usize` can
/// express theirs. `None` for anything that is not an integer.
fn int_bounds(ty: &Ty) -> Option<(i128, u128)> {
    Some(match ty {
        Ty::I32 => (i32::MIN as i128, i32::MAX as u128),
        Ty::I64 | Ty::Isize => (i64::MIN as i128, i64::MAX as u128),
        Ty::U8 => (0, u8::MAX as u128),
        Ty::U16 => (0, u16::MAX as u128),
        Ty::U32 => (0, u32::MAX as u128),
        Ty::U64 | Ty::Usize => (0, u64::MAX as u128),
        _ => return None,
    })
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
    let (Some((s_min, s_max)), Some((t_min, t_max))) = (int_bounds(from), int_bounds(to)) else {
        return format!("(({target})({value}))");
    };
    if s_min >= t_min && s_max <= t_max {
        return format!("(({target})({value}))");
    }
    let tmp = fresh_temp();
    let max = unsigned_lit(t_max);
    let cond = if s_min < 0 {
        // The source can be negative, so the target's own floor matters:
        // `0` for an unsigned target, the signed type's real minimum
        // otherwise (a signed target accepts negatives).
        format!("{tmp} < {} || {tmp} > {max}", signed_lit(t_min))
    } else {
        format!("{tmp} > {max}")
    };
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

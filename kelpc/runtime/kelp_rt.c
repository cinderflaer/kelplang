// ---- Kelp runtime (M6) -------------------------------------------------------
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
static void kelp_String__push(kelp_String* self, unsigned char ch) {
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
    if (s.data == NULL) {
        /* `n == 0` leaves the reserve above with nothing to do, and an empty
           `String` must still point at a NUL byte rather than at NULL — the
           same rule `kelp_String__empty` and `kelp_Bytes__empty` follow, and
           the reason `strlen`, `%s` and `as_str` are never handed NULL. `cap`
           stays 0, which is what an empty string's capacity is. */
        unsigned char* p = (unsigned char*)malloc(1);
        if (p == NULL) {
            kelp_panic("out of memory");
        }
        p[0] = 0;
        s.data = p;
    }
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

/* Fold one already-hashed field or tag into a `datac`/`enum` key's hash. Both
   backends emit a call to this helper rather than writing the combine step
   themselves, so the derived hash of one key type is byte-for-byte the same
   whichever backend built the program. */
static uint64_t kelp_map_combine(uint64_t h, uint64_t x) {
    h ^= kelp_map_mix(x);
    h *= 1099511628211ULL;
    return h;
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

#include <errno.h>
#include <limits.h>
#include <dirent.h>
#include <fcntl.h>
#include <arpa/inet.h>
#include <math.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/sendfile.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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

struct tag_std__str__ParseErr;
struct tag_std__option__Option__l__usize__g__;
struct tag_std__result__Result__l__i64_std__str__ParseErr__g__;
struct tag_std__result__Result__l__u64_std__str__ParseErr__g__;
struct tag_std__result__Result__l__f64_std__str__ParseErr__g__;

typedef struct {
    kelp_String* data;
    size_t len;
    size_t cap;
} kelp_List__String;

static void kelp_list_drop__String(kelp_List__String* v);

typedef struct tag_std__str__ParseErr {
    int32_t kelp_tag;
    union {
        struct {
            char kelp_none;
        } kelp_v_Empty;
        struct {
            char kelp_none;
        } kelp_v_Invalid;
    } kelp_v;
} struct_std__str__ParseErr;
typedef struct tag_std__option__Option__l__usize__g__ {
    int32_t kelp_tag;
    union {
        struct {
            size_t kelp_value;
        } kelp_v_Some;
        struct {
            char kelp_none;
        } kelp_v_None;
    } kelp_v;
} struct_std__option__Option__l__usize__g__;
typedef struct tag_std__result__Result__l__f64_std__str__ParseErr__g__ {
    int32_t kelp_tag;
    union {
        struct {
            double kelp_value;
        } kelp_v_Ok;
        struct {
            struct_std__str__ParseErr kelp_error;
        } kelp_v_Err;
    } kelp_v;
} struct_std__result__Result__l__f64_std__str__ParseErr__g__;
typedef struct tag_std__result__Result__l__i64_std__str__ParseErr__g__ {
    int32_t kelp_tag;
    union {
        struct {
            int64_t kelp_value;
        } kelp_v_Ok;
        struct {
            struct_std__str__ParseErr kelp_error;
        } kelp_v_Err;
    } kelp_v;
} struct_std__result__Result__l__i64_std__str__ParseErr__g__;
typedef struct tag_std__result__Result__l__u64_std__str__ParseErr__g__ {
    int32_t kelp_tag;
    union {
        struct {
            uint64_t kelp_value;
        } kelp_v_Ok;
        struct {
            struct_std__str__ParseErr kelp_error;
        } kelp_v_Err;
    } kelp_v;
} struct_std__result__Result__l__u64_std__str__ParseErr__g__;

// ---- kelp_List__String (collections) ------------------------------
static kelp_List__String kelp_List__String__empty(void) {
    kelp_List__String l;
    l.data = NULL;
    l.len = 0;
    l.cap = 0;
    return l;
}

static void kelp_List__String__reserve(kelp_List__String* self, size_t want) {
    if (want <= self->cap) {
        return;
    }
    size_t cap = self->cap == 0 ? 4 : self->cap;
    while (cap < want) {
        cap *= 2;
    }
    kelp_String* p = (kelp_String*)realloc(self->data, cap * sizeof(kelp_String));
    if (p == NULL) {
        kelp_panic("out of memory");
    }
    self->data = p;
    self->cap = cap;
}

static kelp_List__String kelp_List__String__of(kelp_String v) {
    kelp_List__String l = kelp_List__String__empty();
    kelp_List__String__reserve(&l, 1);
    l.data[0] = v;
    l.len = 1;
    return l;
}

static size_t kelp_List__String__len(const kelp_List__String* self) {
    return self->len;
}

static bool kelp_List__String__is_empty(const kelp_List__String* self) {
    return self->len == 0;
}

static void kelp_List__String__push(kelp_List__String* self, kelp_String v) {
    kelp_List__String__reserve(self, self->len + 1);
    self->data[self->len++] = v;
}

static void kelp_List__String__bounds(const kelp_List__String* self, size_t i) {
    if (i >= self->len) {
        kelp_panic("List.get: index out of bounds");
    }
}

static const kelp_String* kelp_List__String__get_const(const kelp_List__String* self, size_t i) {
    kelp_List__String__bounds(self, i);
    return &self->data[i];
}

static void kelp_List__String__set(kelp_List__String* self, size_t i, kelp_String v) {
    kelp_List__String__bounds(self, i);
    kelp_string_drop(&self->data[i]);
    self->data[i] = v;
}

static kelp_String kelp_List__String__pop(kelp_List__String* self) {
    if (self->len == 0) {
        kelp_panic("List.pop: the list is empty");
    }
    return self->data[--self->len];
}

static void kelp_List__String__clear(kelp_List__String* self) {
    for (size_t i = 0; i < self->len; i++) {
        kelp_string_drop(&self->data[i]);
    }
    self->len = 0;
}

static int kelp_List__String__cmp(const kelp_String* a, const kelp_String* b) {
    size_t n = a->len < b->len ? a->len : b->len;
    if (n > 0) {
        int c = memcmp(a->data, b->data, n);
        if (c != 0) {
            return c < 0 ? -1 : 1;
        }
    }
    return a->len < b->len ? -1 : (a->len > b->len ? 1 : 0);
}

static void kelp_List__String__sort(kelp_List__String* self) {
    /* insertion sort: stable, no comparator indirection, and
       in-place for an owned element because the shifts are moves */
    for (size_t i = 1; i < self->len; i++) {
        kelp_String v = self->data[i];
        size_t j = i;
        while (j > 0 && kelp_List__String__cmp(&self->data[j - 1], &v) > 0) {
            self->data[j] = self->data[j - 1];
            j--;
        }
        self->data[j] = v;
    }
}

static void kelp_List__String__reverse(kelp_List__String* self) {
    size_t i = 0;
    size_t j = self->len;
    while (i + 1 < j) {
        j--;
        kelp_String t = self->data[i];
        self->data[i] = self->data[j];
        self->data[j] = t;
        i++;
    }
}

static bool kelp_List__String__contains(const kelp_List__String* self, const kelp_String* v) {
    for (size_t i = 0; i < self->len; i++) {
        if (self->data[i].len == v->len && (v->len == 0 || memcmp(self->data[i].data, v->data, v->len) == 0)) {
            return true;
        }
    }
    return false;
}

static kelp_List__String kelp_List__String__slice(const kelp_List__String* self, size_t from, size_t to) {
    if (from > to || to > self->len) {
        kelp_panic("List.slice: range out of bounds");
    }
    kelp_List__String out = kelp_List__String__empty();
    kelp_List__String__reserve(&out, to - from);
    for (size_t i = from; i < to; i++) {
        out.data[out.len++] = kelp_String__clone(&self->data[i]);
    }
    return out;
}

static void kelp_List__String__insert(kelp_List__String* self, size_t i, kelp_String v) {
    if (i > self->len) {
        kelp_panic("List.insert: index out of bounds");
    }
    kelp_List__String__reserve(self, self->len + 1);
    for (size_t j = self->len; j > i; j--) {
        self->data[j] = self->data[j - 1];
    }
    self->data[i] = v;
    self->len++;
}

static kelp_String kelp_List__String__remove(kelp_List__String* self, size_t i) {
    if (i >= self->len) {
        kelp_panic("List.remove: index out of bounds");
    }
    kelp_String v = self->data[i];
    for (size_t j = i; j + 1 < self->len; j++) {
        self->data[j] = self->data[j + 1];
    }
    self->len--;
    return v;
}

static void kelp_list_drop__String(kelp_List__String* self) {
    kelp_List__String__clear(self);
    free(self->data);
    self->data = NULL;
    self->cap = 0;
}

kelp_Bytes kelp_std__mem__alloc_raw(size_t kelp_n);
kelp_Bytes kelp_std__mem__alloc(size_t kelp_n);
size_t kelp_std__str__byte_len_raw(const char* kelp_s);
uint8_t kelp_std__str__byte_at_raw(const char* kelp_s, size_t kelp_i);
const char* kelp_std__str__slice_from_raw(const char* kelp_s, size_t kelp_from);
size_t kelp_std__str__len(const char* kelp_s);
uint8_t kelp_std__str__byte(const char* kelp_s, size_t kelp_i);
bool kelp_std__str__is_empty(const char* kelp_s);
const char* kelp_std__str__substring(const char* kelp_s, size_t kelp_from);
size_t kelp_std__str__char_count(const char* kelp_s);
struct_std__option__Option__l__usize__g__ kelp_std__str__index_of(const char* kelp_s, const char* kelp_needle);
bool kelp_std__str__starts_with_at(const char* kelp_s, const char* kelp_needle, size_t kelp_at);
bool kelp_std__str__starts_with(const char* kelp_s, const char* kelp_prefix);
bool kelp_std__str__ends_with(const char* kelp_s, const char* kelp_suffix);
bool kelp_std__str__contains(const char* kelp_s, const char* kelp_needle);
bool kelp_std__str__eq(const char* kelp_a, const char* kelp_b);
bool kelp_std__str__is_space(uint8_t kelp_c);
const char* kelp_std__str__trim_start(const char* kelp_s);
kelp_String kelp_std__str__trim_end(const char* kelp_s);
kelp_String kelp_std__str__prefix(const char* kelp_s, size_t kelp_n);
kelp_String kelp_std__str__trim(const char* kelp_s);
kelp_String kelp_std__str__to_upper(const char* kelp_s);
kelp_String kelp_std__str__to_lower(const char* kelp_s);
kelp_String kelp_std__str__repeat(const char* kelp_s, size_t kelp_n);
kelp_String kelp_std__str__copy_range(const char* kelp_s, size_t kelp_from, size_t kelp_to);
kelp_List__String kelp_std__str__split(const char* kelp_s, const char* kelp_sep);
size_t kelp_std__str__copy_into(kelp_Bytes* kelp_dst, size_t kelp_at, const char* kelp_src);
kelp_String kelp_std__str__join(const kelp_List__String* kelp_parts, const char* kelp_sep);
kelp_String kelp_std__str__concat(const char* kelp_a, const char* kelp_b);
kelp_String kelp_std__str__from_u64(uint64_t kelp_n);
kelp_String kelp_std__str__from_i64(int64_t kelp_n);
kelp_String kelp_std__str__from_f64_raw(double kelp_x, int32_t kelp_decimals);
kelp_String kelp_std__str__from_f64(double kelp_x, int32_t kelp_decimals);
bool kelp_std__str__parse_i64_raw(const char* kelp_s, int64_t* kelp_out);
bool kelp_std__str__parse_u64_raw(const char* kelp_s, uint64_t* kelp_out);
bool kelp_std__str__parse_f64_raw(const char* kelp_s, double* kelp_out);
struct_std__result__Result__l__i64_std__str__ParseErr__g__ kelp_std__str__parse_i64(const char* kelp_s);
struct_std__result__Result__l__u64_std__str__ParseErr__g__ kelp_std__str__parse_u64(const char* kelp_s);
struct_std__result__Result__l__f64_std__str__ParseErr__g__ kelp_std__str__parse_f64(const char* kelp_s);
struct_std__str__ParseErr kelp_std__str__parse_err(const char* kelp_s);
kelp_String kelp_std__str__replace(const char* kelp_s, const char* kelp_from, const char* kelp_to);
size_t kelp_std__str__byte_index(const char* kelp_s, size_t kelp_i);
uint32_t kelp_std__str__decode_at(const char* kelp_s, size_t kelp_at);
uint32_t kelp_std__str__char_at(const char* kelp_s, size_t kelp_i);
bool kelp_std__option__is_some__l__usize__g__(const struct_std__option__Option__l__usize__g__* kelp_o);

// extern 'std.mem.alloc_raw' parameters: n -> kelp_n
kelp_Bytes kelp_std__mem__alloc_raw(size_t kelp_n) {
    return kelp_bytes_alloc_raw(kelp_n);
}

kelp_Bytes kelp_std__mem__alloc(size_t kelp_n) {
    return kelp_std__mem__alloc_raw(kelp_n);
}

// extern 'std.str.byte_len_raw' parameters: s -> kelp_s
size_t kelp_std__str__byte_len_raw(const char* kelp_s) {
    return strlen(kelp_s);
}

// extern 'std.str.byte_at_raw' parameters: s -> kelp_s, i -> kelp_i
uint8_t kelp_std__str__byte_at_raw(const char* kelp_s, size_t kelp_i) {
    if (kelp_i >= strlen(kelp_s)) {
        kelp_panic("str.byte: index out of bounds");
    }
    return (uint8_t)kelp_s[kelp_i];
}

// extern 'std.str.slice_from_raw' parameters: s -> kelp_s, from -> kelp_from
const char* kelp_std__str__slice_from_raw(const char* kelp_s, size_t kelp_from) {
    if (kelp_from > strlen(kelp_s)) {
        kelp_panic("str.substring: index out of bounds");
    }
    return kelp_s + kelp_from;
}

size_t kelp_std__str__len(const char* kelp_s) {
    return kelp_std__str__byte_len_raw(kelp_s);
}

uint8_t kelp_std__str__byte(const char* kelp_s, size_t kelp_i) {
    return kelp_std__str__byte_at_raw(kelp_s, kelp_i);
}

bool kelp_std__str__is_empty(const char* kelp_s) {
    return (kelp_std__str__len(kelp_s) == 0);
}

const char* kelp_std__str__substring(const char* kelp_s, size_t kelp_from) {
    return kelp_std__str__slice_from_raw(kelp_s, kelp_from);
}

size_t kelp_std__str__char_count(const char* kelp_s) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    size_t kelp_count = 0;
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        uint8_t kelp_c = kelp_std__str__byte(kelp_s, kelp_i);
        if (((kelp_c < 128) || (kelp_c >= 192))) {
            (kelp_count = (kelp_count + 1));
        }
        (kelp_i = (kelp_i + 1));
    }
    return kelp_count;
}

struct_std__option__Option__l__usize__g__ kelp_std__str__index_of(const char* kelp_s, const char* kelp_needle) {
    size_t kelp_ls = kelp_std__str__len(kelp_s);
    size_t kelp_ln = kelp_std__str__len(kelp_needle);
    if ((kelp_ln == 0)) {
        return (struct_std__option__Option__l__usize__g__){ .kelp_tag = 0, .kelp_v = { .kelp_v_Some = { .kelp_value = 0 } } };
    }
    if ((kelp_ln > kelp_ls)) {
        return (struct_std__option__Option__l__usize__g__){ .kelp_tag = 1 };
    }
    size_t kelp_i = 0;
    while (((kelp_i + kelp_ln) <= kelp_ls)) {
        if (kelp_std__str__starts_with_at(kelp_s, kelp_needle, kelp_i)) {
            return (struct_std__option__Option__l__usize__g__){ .kelp_tag = 0, .kelp_v = { .kelp_v_Some = { .kelp_value = kelp_i } } };
        }
        (kelp_i = (kelp_i + 1));
    }
    return (struct_std__option__Option__l__usize__g__){ .kelp_tag = 1 };
}

bool kelp_std__str__starts_with_at(const char* kelp_s, const char* kelp_needle, size_t kelp_at) {
    size_t kelp_ln = kelp_std__str__len(kelp_needle);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_ln)) {
        if ((kelp_std__str__byte(kelp_s, (kelp_at + kelp_i)) != kelp_std__str__byte(kelp_needle, kelp_i))) {
            return false;
        }
        (kelp_i = (kelp_i + 1));
    }
    return true;
}

bool kelp_std__str__starts_with(const char* kelp_s, const char* kelp_prefix) {
    if ((kelp_std__str__len(kelp_prefix) > kelp_std__str__len(kelp_s))) {
        return false;
    }
    return kelp_std__str__starts_with_at(kelp_s, kelp_prefix, 0);
}

bool kelp_std__str__ends_with(const char* kelp_s, const char* kelp_suffix) {
    size_t kelp_ls = kelp_std__str__len(kelp_s);
    size_t kelp_lf = kelp_std__str__len(kelp_suffix);
    if ((kelp_lf > kelp_ls)) {
        return false;
    }
    return kelp_std__str__starts_with_at(kelp_s, kelp_suffix, (kelp_ls - kelp_lf));
}

bool kelp_std__str__contains(const char* kelp_s, const char* kelp_needle) {
    return ({ struct_std__option__Option__l__usize__g__ _xtmp0 = kelp_std__str__index_of(kelp_s, kelp_needle);
kelp_std__option__is_some__l__usize__g__(&_xtmp0); });
}

bool kelp_std__str__eq(const char* kelp_a, const char* kelp_b) {
    if ((kelp_std__str__len(kelp_a) != kelp_std__str__len(kelp_b))) {
        return false;
    }
    size_t kelp_n = kelp_std__str__len(kelp_a);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        if ((kelp_std__str__byte(kelp_a, kelp_i) != kelp_std__str__byte(kelp_b, kelp_i))) {
            return false;
        }
        (kelp_i = (kelp_i + 1));
    }
    return true;
}

bool kelp_std__str__is_space(uint8_t kelp_c) {
    return ((((kelp_c == 32) || (kelp_c == 9)) || (kelp_c == 10)) || (kelp_c == 13));
}

const char* kelp_std__str__trim_start(const char* kelp_s) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    size_t kelp_i = 0;
    while (((kelp_i < kelp_n) && kelp_std__str__is_space(kelp_std__str__byte(kelp_s, kelp_i)))) {
        (kelp_i = (kelp_i + 1));
    }
    return kelp_std__str__slice_from_raw(kelp_s, kelp_i);
}

kelp_String kelp_std__str__trim_end(const char* kelp_s) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    size_t kelp_end = kelp_n;
    while (((kelp_end > 0) && kelp_std__str__is_space(kelp_std__str__byte(kelp_s, (kelp_end - 1))))) {
        (kelp_end = (kelp_end - 1));
    }
    return kelp_std__str__prefix(kelp_s, kelp_end);
}

kelp_String kelp_std__str__prefix(const char* kelp_s, size_t kelp_n) {
    kelp_Bytes kelp_out = kelp_std__mem__alloc(kelp_n);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        kelp_Bytes__set((&kelp_out), kelp_i, kelp_std__str__byte(kelp_s, kelp_i));
        (kelp_i = (kelp_i + 1));
    }
    return kelp_String__from_bytes(kelp_out);
}

kelp_String kelp_std__str__trim(const char* kelp_s) {
    return kelp_std__str__trim_end(kelp_std__str__trim_start(kelp_s));
}

kelp_String kelp_std__str__to_upper(const char* kelp_s) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    kelp_Bytes kelp_out = kelp_std__mem__alloc(kelp_n);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        uint8_t kelp_c = kelp_std__str__byte(kelp_s, kelp_i);
        if (((kelp_c >= 97) && (kelp_c <= 122))) {
            kelp_Bytes__set((&kelp_out), kelp_i, (kelp_c - 32));
        } else {
            kelp_Bytes__set((&kelp_out), kelp_i, kelp_c);
        }
        (kelp_i = (kelp_i + 1));
    }
    return kelp_String__from_bytes(kelp_out);
}

kelp_String kelp_std__str__to_lower(const char* kelp_s) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    kelp_Bytes kelp_out = kelp_std__mem__alloc(kelp_n);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        uint8_t kelp_c = kelp_std__str__byte(kelp_s, kelp_i);
        if (((kelp_c >= 65) && (kelp_c <= 90))) {
            kelp_Bytes__set((&kelp_out), kelp_i, (kelp_c + 32));
        } else {
            kelp_Bytes__set((&kelp_out), kelp_i, kelp_c);
        }
        (kelp_i = (kelp_i + 1));
    }
    return kelp_String__from_bytes(kelp_out);
}

kelp_String kelp_std__str__repeat(const char* kelp_s, size_t kelp_n) {
    size_t kelp_ls = kelp_std__str__len(kelp_s);
    kelp_Bytes kelp_out = kelp_std__mem__alloc((kelp_ls * kelp_n));
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        size_t kelp_j = 0;
        while ((kelp_j < kelp_ls)) {
            kelp_Bytes__set((&kelp_out), ((kelp_i * kelp_ls) + kelp_j), kelp_std__str__byte(kelp_s, kelp_j));
            (kelp_j = (kelp_j + 1));
        }
        (kelp_i = (kelp_i + 1));
    }
    return kelp_String__from_bytes(kelp_out);
}

kelp_String kelp_std__str__copy_range(const char* kelp_s, size_t kelp_from, size_t kelp_to) {
    kelp_Bytes kelp_out = kelp_std__mem__alloc((kelp_to - kelp_from));
    size_t kelp_i = 0;
    while ((kelp_i < (kelp_to - kelp_from))) {
        kelp_Bytes__set((&kelp_out), kelp_i, kelp_std__str__byte(kelp_s, (kelp_from + kelp_i)));
        (kelp_i = (kelp_i + 1));
    }
    return kelp_String__from_bytes(kelp_out);
}

kelp_List__String kelp_std__str__split(const char* kelp_s, const char* kelp_sep) {
    kelp_List__String kelp_out = kelp_List__String__empty();
    size_t kelp_ls = kelp_std__str__len(kelp_s);
    size_t kelp_lsep = kelp_std__str__len(kelp_sep);
    if ((kelp_lsep == 0)) {
        kelp_List__String__push((&kelp_out), kelp_String__from_str(kelp_s));
        return kelp_out;
    }
    size_t kelp_start = 0;
    size_t kelp_i = 0;
    while (((kelp_i + kelp_lsep) <= kelp_ls)) {
        if (kelp_std__str__starts_with_at(kelp_s, kelp_sep, kelp_i)) {
            kelp_List__String__push((&kelp_out), kelp_std__str__copy_range(kelp_s, kelp_start, kelp_i));
            (kelp_i = (kelp_i + kelp_lsep));
            (kelp_start = kelp_i);
        } else {
            (kelp_i = (kelp_i + 1));
        }
    }
    kelp_List__String__push((&kelp_out), kelp_std__str__copy_range(kelp_s, kelp_start, kelp_ls));
    return kelp_out;
}

size_t kelp_std__str__copy_into(kelp_Bytes* kelp_dst, size_t kelp_at, const char* kelp_src) {
    size_t kelp_n = kelp_std__str__len(kelp_src);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_n)) {
        kelp_Bytes__set(kelp_dst, (kelp_at + kelp_i), kelp_std__str__byte(kelp_src, kelp_i));
        (kelp_i = (kelp_i + 1));
    }
    return (kelp_at + kelp_n);
}

kelp_String kelp_std__str__join(const kelp_List__String* kelp_parts, const char* kelp_sep) {
    size_t kelp_count = kelp_List__String__len(kelp_parts);
    size_t kelp_lsep = kelp_std__str__len(kelp_sep);
    kelp_String kelp_out = kelp_String__with_capacity((kelp_count * kelp_lsep));
    size_t kelp_i = 0;
    while ((kelp_i < kelp_count)) {
        if ((kelp_i > 0)) {
            kelp_String__push_str((&kelp_out), kelp_sep);
        }
        kelp_String__push_str((&kelp_out), kelp_String__as_str(kelp_List__String__get_const(kelp_parts, kelp_i)));
        (kelp_i = (kelp_i + 1));
    }
    return kelp_out;
}

kelp_String kelp_std__str__concat(const char* kelp_a, const char* kelp_b) {
    kelp_String kelp_out = kelp_String__with_capacity((kelp_std__str__len(kelp_a) + kelp_std__str__len(kelp_b)));
    kelp_String__push_str((&kelp_out), kelp_a);
    kelp_String__push_str((&kelp_out), kelp_b);
    return kelp_out;
}

kelp_String kelp_std__str__from_u64(uint64_t kelp_n) {
    if ((kelp_n == 0)) {
        return kelp_String__from_str("0");
    }
    kelp_String kelp_rev = kelp_String__with_capacity(20);
    uint64_t kelp_v = kelp_n;
    while ((kelp_v > 0)) {
        kelp_String__push((&kelp_rev), ({ uint64_t _xtmp1 = ((kelp_v % 10) + 48); if (_xtmp1 > 255LL) { kelp_panic("u64.to_u8: value out of range"); } (uint8_t)_xtmp1; }));
        (kelp_v = (kelp_v / 10));
    }
    const char* kelp_digits = kelp_String__as_str((&kelp_rev));
    kelp_String kelp_out = kelp_String__with_capacity(kelp_String__len((&kelp_rev)));
    size_t kelp_i = kelp_String__len((&kelp_rev));
    while ((kelp_i > 0)) {
        (kelp_i = (kelp_i - 1));
        kelp_String__push((&kelp_out), kelp_std__str__byte(kelp_digits, kelp_i));
    }
    kelp_String _xtmp2 = kelp_out;
    kelp_string_drop(&kelp_rev);
    return _xtmp2;
}

kelp_String kelp_std__str__from_i64(int64_t kelp_n) {
    if ((kelp_n == 0)) {
        return kelp_String__from_str("0");
    }
    bool kelp_neg = false;
    int64_t kelp_v = kelp_n;
    if ((kelp_v < 0)) {
        (kelp_neg = true);
    }
    kelp_String kelp_rev = kelp_String__with_capacity(20);
    while ((kelp_v != 0)) {
        int64_t kelp_d = (kelp_v % 10);
        if ((kelp_d < 0)) {
            (kelp_d = (0 - kelp_d));
        }
        kelp_String__push((&kelp_rev), ({ int64_t _xtmp3 = (kelp_d + 48); if (_xtmp3 < 0LL || _xtmp3 > 255LL) { kelp_panic("i64.to_u8: value out of range"); } (uint8_t)_xtmp3; }));
        (kelp_v = (kelp_v / 10));
    }
    const char* kelp_digits = kelp_String__as_str((&kelp_rev));
    kelp_String kelp_out = kelp_String__with_capacity(21);
    if (kelp_neg) {
        kelp_String__push((&kelp_out), 45);
    }
    size_t kelp_i = kelp_String__len((&kelp_rev));
    while ((kelp_i > 0)) {
        (kelp_i = (kelp_i - 1));
        kelp_String__push((&kelp_out), kelp_std__str__byte(kelp_digits, kelp_i));
    }
    kelp_String _xtmp4 = kelp_out;
    kelp_string_drop(&kelp_rev);
    return _xtmp4;
}

// extern 'std.str.from_f64_raw' parameters: x -> kelp_x, decimals -> kelp_decimals
kelp_String kelp_std__str__from_f64_raw(double kelp_x, int32_t kelp_decimals) {
    int need = snprintf(NULL, 0, "%.*f", (int)kelp_decimals, kelp_x);
    if (need < 0) {
        kelp_panic("str.from_f64: formatting failed");
    }
    kelp_Bytes b = kelp_bytes_alloc_raw((size_t)need);
    snprintf((char*)b.data, b.len + 1, "%.*f", (int)kelp_decimals, kelp_x);
    return kelp_String__from_bytes(b);
}

kelp_String kelp_std__str__from_f64(double kelp_x, int32_t kelp_decimals) {
    return kelp_std__str__from_f64_raw(kelp_x, kelp_decimals);
}

// extern 'std.str.parse_i64_raw' parameters: s -> kelp_s, out -> kelp_out
bool kelp_std__str__parse_i64_raw(const char* kelp_s, int64_t* kelp_out) {
    if (kelp_s[0] == 0) {
        return false;
    }
    for (const char* p = kelp_s; *p != 0; p++) {
        if (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') {
            return false;
        }
    }
    char* end = NULL;
    errno = 0;
    long long v = strtoll(kelp_s, &end, 10);
    if (end == kelp_s || *end != 0 || errno != 0) {
        return false;
    }
    *kelp_out = (int64_t)v;
    return true;
}

// extern 'std.str.parse_u64_raw' parameters: s -> kelp_s, out -> kelp_out
bool kelp_std__str__parse_u64_raw(const char* kelp_s, uint64_t* kelp_out) {
    if (kelp_s[0] == 0 || kelp_s[0] == '-') {
        return false;
    }
    for (const char* p = kelp_s; *p != 0; p++) {
        if (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') {
            return false;
        }
    }
    char* end = NULL;
    errno = 0;
    unsigned long long v = strtoull(kelp_s, &end, 10);
    if (end == kelp_s || *end != 0 || errno != 0) {
        return false;
    }
    *kelp_out = (uint64_t)v;
    return true;
}

// extern 'std.str.parse_f64_raw' parameters: s -> kelp_s, out -> kelp_out
bool kelp_std__str__parse_f64_raw(const char* kelp_s, double* kelp_out) {
    if (kelp_s[0] == 0) {
        return false;
    }
    for (const char* p = kelp_s; *p != 0; p++) {
        if (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') {
            return false;
        }
    }
    char* end = NULL;
    double v = strtod(kelp_s, &end);
    if (end == kelp_s || *end != 0) {
        return false;
    }
    *kelp_out = v;
    return true;
}

struct_std__result__Result__l__i64_std__str__ParseErr__g__ kelp_std__str__parse_i64(const char* kelp_s) {
    int64_t kelp_v = 0;
    bool kelp_ok = kelp_std__str__parse_i64_raw(kelp_s, (&kelp_v));
    if ((!kelp_ok)) {
        return (struct_std__result__Result__l__i64_std__str__ParseErr__g__){ .kelp_tag = 1, .kelp_v = { .kelp_v_Err = { .kelp_error = kelp_std__str__parse_err(kelp_s) } } };
    }
    return (struct_std__result__Result__l__i64_std__str__ParseErr__g__){ .kelp_tag = 0, .kelp_v = { .kelp_v_Ok = { .kelp_value = kelp_v } } };
}

struct_std__result__Result__l__u64_std__str__ParseErr__g__ kelp_std__str__parse_u64(const char* kelp_s) {
    uint64_t kelp_v = 0;
    bool kelp_ok = kelp_std__str__parse_u64_raw(kelp_s, (&kelp_v));
    if ((!kelp_ok)) {
        return (struct_std__result__Result__l__u64_std__str__ParseErr__g__){ .kelp_tag = 1, .kelp_v = { .kelp_v_Err = { .kelp_error = kelp_std__str__parse_err(kelp_s) } } };
    }
    return (struct_std__result__Result__l__u64_std__str__ParseErr__g__){ .kelp_tag = 0, .kelp_v = { .kelp_v_Ok = { .kelp_value = kelp_v } } };
}

struct_std__result__Result__l__f64_std__str__ParseErr__g__ kelp_std__str__parse_f64(const char* kelp_s) {
    double kelp_v = 0;
    bool kelp_ok = kelp_std__str__parse_f64_raw(kelp_s, (&kelp_v));
    if ((!kelp_ok)) {
        return (struct_std__result__Result__l__f64_std__str__ParseErr__g__){ .kelp_tag = 1, .kelp_v = { .kelp_v_Err = { .kelp_error = kelp_std__str__parse_err(kelp_s) } } };
    }
    return (struct_std__result__Result__l__f64_std__str__ParseErr__g__){ .kelp_tag = 0, .kelp_v = { .kelp_v_Ok = { .kelp_value = kelp_v } } };
}

struct_std__str__ParseErr kelp_std__str__parse_err(const char* kelp_s) {
    if ((kelp_std__str__len(kelp_s) == 0)) {
        return (struct_std__str__ParseErr){ .kelp_tag = 0 };
    }
    return (struct_std__str__ParseErr){ .kelp_tag = 1 };
}

kelp_String kelp_std__str__replace(const char* kelp_s, const char* kelp_from, const char* kelp_to) {
    size_t kelp_lf = kelp_std__str__len(kelp_from);
    if ((kelp_lf == 0)) {
        return kelp_String__from_str(kelp_s);
    }
    size_t kelp_ls = kelp_std__str__len(kelp_s);
    kelp_String kelp_out = kelp_String__with_capacity(kelp_ls);
    size_t kelp_i = 0;
    while ((kelp_i < kelp_ls)) {
        if ((((kelp_i + kelp_lf) <= kelp_ls) && kelp_std__str__starts_with_at(kelp_s, kelp_from, kelp_i))) {
            kelp_String__push_str((&kelp_out), kelp_to);
            (kelp_i = (kelp_i + kelp_lf));
        } else {
            kelp_String__push((&kelp_out), kelp_std__str__byte(kelp_s, kelp_i));
            (kelp_i = (kelp_i + 1));
        }
    }
    return kelp_out;
}

size_t kelp_std__str__byte_index(const char* kelp_s, size_t kelp_i) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    size_t kelp_chars = 0;
    size_t kelp_at = 0;
    while ((kelp_at < kelp_n)) {
        uint8_t kelp_c = kelp_std__str__byte(kelp_s, kelp_at);
        if (((kelp_c < 128) || (kelp_c >= 192))) {
            if ((kelp_chars == kelp_i)) {
                return kelp_at;
            }
            (kelp_chars = (kelp_chars + 1));
        }
        (kelp_at = (kelp_at + 1));
    }
    return kelp_n;
}

uint32_t kelp_std__str__decode_at(const char* kelp_s, size_t kelp_at) {
    size_t kelp_n = kelp_std__str__len(kelp_s);
    uint32_t kelp_b0 = ((uint32_t)(kelp_std__str__byte(kelp_s, kelp_at)));
    if ((kelp_b0 < 128)) {
        return kelp_b0;
    }
    uint32_t kelp_cp = 0;
    size_t kelp_extra = 0;
    if ((kelp_b0 < 224)) {
        (kelp_cp = (kelp_b0 - 192));
        (kelp_extra = 1);
    } else if ((kelp_b0 < 240)) {
        (kelp_cp = (kelp_b0 - 224));
        (kelp_extra = 2);
    } else {
        (kelp_cp = (kelp_b0 - 240));
        (kelp_extra = 3);
    }
    size_t kelp_k = 0;
    while ((kelp_k < kelp_extra)) {
        if ((((kelp_at + 1) + kelp_k) >= kelp_n)) {
            return 0;
        }
        (kelp_cp = ((kelp_cp * 64) + (((uint32_t)(kelp_std__str__byte(kelp_s, ((kelp_at + 1) + kelp_k)))) - 128)));
        (kelp_k = (kelp_k + 1));
    }
    return kelp_cp;
}

uint32_t kelp_std__str__char_at(const char* kelp_s, size_t kelp_i) {
    size_t kelp_at = kelp_std__str__byte_index(kelp_s, kelp_i);
    if ((kelp_at >= kelp_std__str__len(kelp_s))) {
        return 0;
    }
    return kelp_std__str__decode_at(kelp_s, kelp_at);
}

int main(int argc, char** argv) {
    kelp_argc = argc;
    kelp_argv = argv;
    uint8_t kelp_quote = ((uint8_t)(34));
    printf("%u\n", (unsigned)kelp_quote);
    uint8_t kelp_zero = ((uint8_t)(48));
    printf("%u\n", (unsigned)kelp_zero);
    printf("%u\n", (unsigned)(kelp_zero + 5));
    uint8_t kelp_nl = ((uint8_t)(10));
    printf("%u\n", (unsigned)kelp_nl);
    uint8_t kelp_tab = ((uint8_t)(9));
    printf("%u\n", (unsigned)kelp_tab);
    uint8_t kelp_backslash = ((uint8_t)(92));
    printf("%u\n", (unsigned)kelp_backslash);
    uint8_t kelp_squote = ((uint8_t)(39));
    printf("%u\n", (unsigned)kelp_squote);
    uint8_t kelp_space = ((uint8_t)(32));
    printf("%s\n", ((kelp_space == 32) ? "true" : "false"));
    uint8_t kelp_tilde = ((uint8_t)(126));
    printf("%s\n", ((kelp_tilde == 126) ? "true" : "false"));
    uint8_t kelp_a = ((uint8_t)(65));
    printf("%s\n", ((kelp_a == 65) ? "true" : "false"));
    uint32_t kelp_letter = ((uint32_t)(65));
    printf("%s\n", ((kelp_letter == ((uint32_t)(65))) ? "true" : "false"));
    printf("%s\n", ((kelp_letter == ((uint32_t)(66))) ? "true" : "false"));
    uint32_t kelp_cjk = ((uint32_t)(20013));
    printf("%s\n", ((kelp_cjk == ((uint32_t)(20013))) ? "true" : "false"));
    printf("%s\n", ((kelp_cjk == ((uint32_t)(65))) ? "true" : "false"));
    const char* kelp_s = "hi";
    printf("%s\n", ((kelp_std__str__byte(kelp_s, 0) == ((uint8_t)(104))) ? "true" : "false"));
    printf("%s\n", ((kelp_std__str__byte(kelp_s, 1) == ((uint8_t)(105))) ? "true" : "false"));
    printf("%s\n", ((kelp_std__str__byte(kelp_s, 0) == ((uint8_t)(72))) ? "true" : "false"));
    int64_t kelp_big = ((int64_t)(122));
    printf("%lld\n", (long long)kelp_big);
}

bool kelp_std__option__is_some__l__usize__g__(const struct_std__option__Option__l__usize__g__* kelp_o) {
    return ({
    ({
bool _xtmp5;
if (((kelp_o)->kelp_tag == 0)) {
    _xtmp5 = true;
} else {
    _xtmp5 = false;
}
_xtmp5;
});
});
}


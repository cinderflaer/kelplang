// ---- Kelp runtime (M6) -------------------------------------------------------
typedef struct {
    unsigned char* data;
    size_t len;
    /* Usable bytes at `data`, *excluding* the NUL terminator slot: the
       allocation is always `cap + 1` bytes, and `data[len]` is kept 0 so a
       `str` view of the buffer stays valid (S1 growth invariant). */
    size_t cap;
} kelp_Bytes;

/* ---- Byte-order conversion (kernel stage 3, MK9-S8 / roadmap B-10) ----------
   One swap per width and the two directed conversions the language names,
   emitted at the call site rather than as functions. `__BYTE_ORDER__` is the C
   compiler's own report of the target's endianness, so `to_be`/`to_le` collapse
   to the identity on the little-endian targets this project builds for today
   and still swap on a big-endian one; a hard-coded direction would simply be
   wrong on the other target.

   `KELP_SWAP_SIZE` reads the pointer width from `sizeof` rather than assuming
   eight bytes, so `usize`/`isize` follow the target's own word size. (It is a
   macro and not a helper function because `build.rs` compiles this same text
   with `#define static`, which would turn a `static inline` helper into a bare
   `inline` definition.) */
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define KELP_TARGET_BIG_ENDIAN 1
#else
#define KELP_TARGET_BIG_ENDIAN 0
#endif

#define KELP_SWAP_16(v) __builtin_bswap16((uint16_t)(v))
#define KELP_SWAP_32(v) __builtin_bswap32((uint32_t)(v))
#define KELP_SWAP_64(v) __builtin_bswap64((uint64_t)(v))
#define KELP_TO_BE_16(v) (KELP_TARGET_BIG_ENDIAN ? (uint16_t)(v) : KELP_SWAP_16(v))
#define KELP_TO_BE_32(v) (KELP_TARGET_BIG_ENDIAN ? (uint32_t)(v) : KELP_SWAP_32(v))
#define KELP_TO_BE_64(v) (KELP_TARGET_BIG_ENDIAN ? (uint64_t)(v) : KELP_SWAP_64(v))
#define KELP_TO_LE_16(v) (KELP_TARGET_BIG_ENDIAN ? KELP_SWAP_16(v) : (uint16_t)(v))
#define KELP_TO_LE_32(v) (KELP_TARGET_BIG_ENDIAN ? KELP_SWAP_32(v) : (uint32_t)(v))
#define KELP_TO_LE_64(v) (KELP_TARGET_BIG_ENDIAN ? KELP_SWAP_64(v) : (uint64_t)(v))

#define KELP_SWAP_SIZE(v) \
    ((size_t)(sizeof(size_t) == 8 ? __builtin_bswap64((uint64_t)(v)) \
                                  : __builtin_bswap32((uint32_t)(v))))
#define KELP_TO_BE_SIZE(v) (KELP_TARGET_BIG_ENDIAN ? (size_t)(v) : KELP_SWAP_SIZE(v))
#define KELP_TO_LE_SIZE(v) (KELP_TARGET_BIG_ENDIAN ? KELP_SWAP_SIZE(v) : (size_t)(v))

/* ---- kelp_Thread: the worker handle and the per-thread panic policy ---------
   MK9-S3-M1 stage C1. One `thread.spawn` makes one handle: the worker's
   identity, the slot the worker writes its result into, the slot the argument
   was moved into, and the message the worker died with when it panicked.
   `thread.join` waits on the worker and reads those slots; the drop of a handle
   nobody joined detaches the worker and leaks the handle, because the handle
   cannot be freed while the worker may still write through it.

   The panic policy is MK9 section 2: a panic inside a worker records its
   message here and ends that worker, while a panic anywhere else keeps today's
   behaviour and takes the process. `kelp_thread_self` is the thread-local slot
   that lets a `kelp_panic` call tell the two apart. */
typedef struct {
    pthread_t tid;
    int panicked;
    void* result;
    void* arg;
    const char* message;
} kelp_Thread;

static _Thread_local kelp_Thread* kelp_thread_self = NULL;

/* The text a worker hands its joiner: `msg`, then the detail line the main
   thread would have printed under it (an errno, a getaddrinfo code). Heap,
   because the joiner reads it after the worker's frame is gone; it is freed by
   nothing, which is the same admitted leak a panicking worker's frame is
   (MK9 section 2). */
static const char* kelp_thread_panic_text(const char* msg, const char* detail) {
    size_t n = strlen(msg);
    size_t m = detail == NULL ? 0 : strlen(detail);
    char* out = (char*)malloc(n + m + 1);
    if (out == NULL) {
        return msg;
    }
    if (n > 0) {
        memcpy(out, msg, n);
    }
    if (m > 0) {
        memcpy(out + n, detail, m);
    }
    out[n + m] = 0;
    return out;
}

/* `_Noreturn` is what tells the C compiler a `panic` cannot come back, so a
   generated function whose tail is a `panic` needs no trailing `return` and
   draws no "control reaches end of non-void function" warning. A worker's
   panic still never comes back: it ends the worker with `pthread_exit`, and
   its joiner reads the message out of the handle. */
_Noreturn static void kelp_panic(const char* msg) {
    if (kelp_thread_self != NULL) {
        kelp_thread_self->panicked = 1;
        kelp_thread_self->message = msg;
        pthread_exit(NULL);
    }
    fprintf(stderr, "panic: %s\n", msg);
    exit(1);
}

/* A syscall failure names its cause on a second line: `errno` and the text
   `strerror` gives it. The `panic: <msg>` line stays exactly as it was, so a
   golden assertion on a message prefix is unaffected. `p` takes the code as
   an argument because the callers that clean up (a `close`, an `fclose`)
   between the failing call and the panic would otherwise let that cleanup
   overwrite the `errno` being reported.

   In a worker the same line goes into the message the joiner gets, because
   there is no stderr the main thread is watching for a failure it decided to
   handle itself (MK9 section 2). */
_Noreturn static void kelp_panic_at(const char* msg, int p) {
    if (kelp_thread_self != NULL) {
        char detail[256];
        snprintf(detail, sizeof detail, "\n  errno %d: %s", p, strerror(p));
        kelp_thread_self->panicked = 1;
        kelp_thread_self->message = kelp_thread_panic_text(msg, detail);
        pthread_exit(NULL);
    }
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
    if (kelp_thread_self != NULL) {
        char detail[256];
        snprintf(detail, sizeof detail, "\n  getaddrinfo: %s", gai_strerror(rc));
        kelp_thread_self->panicked = 1;
        kelp_thread_self->message = kelp_thread_panic_text(msg, detail);
        pthread_exit(NULL);
    }
    fprintf(stderr, "panic: %s\n", msg);
    fprintf(stderr, "  getaddrinfo: %s\n", gai_strerror(rc));
    exit(1);
}

/* `std/tls`'s own failures (MK5-S4b-M1, open item 37) report through
   OpenSSL's error queue or `X509_verify_cert_error_string`, neither of which
   is `errno`, so this pairs with `kelp_panic_gai` the same way that one
   pairs with `kelp_panic_errno`: a syscall failure gets `errno`, a DNS
   failure gets `gai_strerror`, and a TLS failure gets whatever OpenSSL
   already rendered to text. `reason` is resolved by the caller, at the
   failing call (`ERR_error_string_n` on the code `ERR_get_error()` just
   returned, or the static string `X509_verify_cert_error_string` returns),
   because draining or re-deriving it here would report a different error
   than the one that actually failed. */
_Noreturn static void kelp_panic_tls(const char* msg, const char* reason) {
    if (kelp_thread_self != NULL) {
        char detail[288];
        snprintf(detail, sizeof detail, "\n  openssl: %s", reason);
        kelp_thread_self->panicked = 1;
        kelp_thread_self->message = kelp_thread_panic_text(msg, detail);
        pthread_exit(NULL);
    }
    fprintf(stderr, "panic: %s\n", msg);
    fprintf(stderr, "  openssl: %s\n", reason);
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

/* ---- the environment read cache (perf-env-cache) -----------------------
   glibc's `getenv` linearly scans `environ` on every call, comparing each
   entry's name; a lookup for a name that is NOT set has to scan the whole
   array every time, since nothing lets it stop early. `std/env`'s four
   externs (`has_raw`, `get_raw`, `set_raw`, `unset_raw`, in `externs.rs`)
   are the only place this runtime touches `environ`, and — because Kelp has
   no `unsafe` and no general C-FFI escape hatch — `set_raw`/`unset_raw` are
   the ONLY way a Kelp program can ever change its own environment. So a
   read-through cache that is rebuilt on exactly those two calls can never go
   stale from anything else the program does.

   The table is open addressing over pointers straight into `environ`'s own
   strings (no copying of the name or the value text), built by one scan of
   `environ`.

     - A single `_Atomic` pointer holds the current table (or NULL before the
       first lookup). Once a table exists, a lookup is a lock-free atomic
       load plus a probe of that table: no lock, ever, on the hot path this
       cache exists for.
     - The scan that *builds* a table, though, has to hold `kelp_env_mutex`
       for as long as it reads `environ` — and the real `setenv`/`unsetenv`
       calls have to hold the same mutex for as long as they mutate it. This
       was not the first design tried here: an earlier version let the build
       race lock-free (double-checked, CAS, loser frees its own candidate),
       on the theory that reads and writes only need the same weak ordering
       plain `getenv`/`setenv` already have. That version segfaulted under
       real concurrent `set`/`unset` calls on different names from different
       threads, reproduced directly with a standalone harness outside Kelp
       entirely: glibc's `setenv` may `realloc` the whole `environ` array
       when a name does not already exist, and an unguarded scan of
       `environ` (or, worse, this table's own two passes over it: one to
       count entries, one to read them) can be reading through a pointer
       `realloc` just freed out from under it, or reading past a now-shorter
       array a concurrent `setenv` swapped in mid-scan. That is a real
       use-after-free / out-of-bounds read, not a benign race, so the mutex
       stays. What it buys is still what matters here: the 5,000,000-call
       hot loop this cache was built for never touches it, because by the
       time that loop runs the table already exists.
     - `set_raw`/`unset_raw` make the real `setenv`/`unsetenv` call and
       rebuild the table inside that same critical section — so the
       process's actual environment (and anything a spawned child inherits)
       is always correct regardless of caching, and the rebuild's scan of
       `environ` can never overlap another thread's mutation of it. The
       table a rebuild replaces is deliberately never freed: a concurrent
       reader may still hold a pointer to it that it loaded (lock-free)
       before the swap, and there is no way to know when the last such
       reader is done with it. Leaking on `set`/`unset` (rare; nothing like
       a hot loop in a real program) is the accepted price for lock-free
       reads, which are not rare.

   Both the very first build and every rebuild after a `set`/`unset` go
   through the same `kelp_env_build`, so there is one code path for "how a
   table gets made", not two that could quietly drift apart. */
typedef struct {
    const char** slots; /* NULL-terminated-by-absence; each entry points into
                            one of `environ`'s own "NAME=value" strings */
    size_t cap;          /* power of two; the table's own open-addressing size */
} kelp_EnvTable;

static _Atomic(kelp_EnvTable*) kelp_env_table = NULL;

/* Guards every read of `environ` this cache does (`kelp_env_build`) and
   every write to it (the real `setenv`/`unsetenv` inside `kelp_env_set` and
   `kelp_env_unset`, below), so the two can never overlap. It does not guard
   `kelp_env_lookup`'s read of an already-published table: that is the
   lock-free path. */
static pthread_mutex_t kelp_env_mutex = PTHREAD_MUTEX_INITIALIZER;

/* FNV-1a, the same hash the validated prototype used. */
static uint64_t kelp_env_hash(const char* data, size_t len) {
    uint64_t h = 14695981039346656037ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint64_t)(unsigned char)data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

/* One scan of `environ`, building a fresh table at roughly 2x its entry
   count so probing stays short. Used both for the very first table and for
   every rebuild after `set`/`unset` (requirement: one code path). Callers
   must hold `kelp_env_mutex`: see the comment above `kelp_EnvTable`. */
static kelp_EnvTable* kelp_env_build(void) {
    extern char** environ;
    size_t n = 0;
    while (environ[n] != NULL) {
        n++;
    }
    size_t cap = 8;
    while (cap < n * 2 + 1) {
        cap *= 2;
    }
    kelp_EnvTable* t = (kelp_EnvTable*)malloc(sizeof(kelp_EnvTable));
    if (t == NULL) {
        kelp_panic("env: out of memory building the lookup table");
    }
    t->slots = (const char**)calloc(cap, sizeof(const char*));
    if (t->slots == NULL) {
        kelp_panic("env: out of memory building the lookup table");
    }
    t->cap = cap;
    for (size_t i = 0; i < n; i++) {
        const char* entry = environ[i];
        const char* eq = strchr(entry, '=');
        size_t name_len = (eq != NULL) ? (size_t)(eq - entry) : strlen(entry);
        size_t idx = (size_t)(kelp_env_hash(entry, name_len) & (uint64_t)(cap - 1));
        while (t->slots[idx] != NULL) {
            idx = (idx + 1) & (cap - 1);
        }
        t->slots[idx] = entry;
    }
    return t;
}

/* The read side: returns the current table, building one first (under the
   mutex, double-checked) if none exists yet. Once a table exists this is a
   single lock-free atomic load: the mutex is only ever taken once per
   process, for whichever thread's lookup or `set`/`unset` happens to run
   first. */
static kelp_EnvTable* kelp_env_ensure(void) {
    kelp_EnvTable* t = atomic_load_explicit(&kelp_env_table, memory_order_acquire);
    if (t != NULL) {
        return t;
    }
    pthread_mutex_lock(&kelp_env_mutex);
    t = atomic_load_explicit(&kelp_env_table, memory_order_relaxed);
    if (t == NULL) {
        t = kelp_env_build();
        atomic_store_explicit(&kelp_env_table, t, memory_order_release);
    }
    pthread_mutex_unlock(&kelp_env_mutex);
    return t;
}

/* Rebuilds the table from the current `environ` and publishes it. Called
   only from `kelp_env_set`/`kelp_env_unset`, which already hold
   `kelp_env_mutex` and have just finished mutating `environ` themselves; the
   table this replaces is deliberately leaked (see the comment above
   `kelp_EnvTable`). */
static void kelp_env_publish(void) {
    kelp_EnvTable* built = kelp_env_build();
    atomic_store_explicit(&kelp_env_table, built, memory_order_release);
}

/* The real `setenv`, and the rebuild that follows it, as one critical
   section: `kelp_env_build` may not overlap a mutation of `environ`, on this
   thread or any other, and this is the one function that performs one.
   Returns what `setenv` returned, so the caller (`std/env`'s `set_raw`) can
   panic on failure outside the lock, the way `kelp_panic_errno` needs to. */
static int kelp_env_set(const char* name, const char* value) {
    pthread_mutex_lock(&kelp_env_mutex);
    int rc = setenv(name, value, 1);
    if (rc == 0) {
        kelp_env_publish();
    }
    pthread_mutex_unlock(&kelp_env_mutex);
    return rc;
}

/* The same for `unsetenv`. */
static int kelp_env_unset(const char* name) {
    pthread_mutex_lock(&kelp_env_mutex);
    int rc = unsetenv(name);
    if (rc == 0) {
        kelp_env_publish();
    }
    pthread_mutex_unlock(&kelp_env_mutex);
    return rc;
}

/* `NULL` if `name` is not set, otherwise a pointer to its value inside
   `environ` (not a copy). Entries are compared up to their `=` byte, exactly
   like `getenv` compares up to it, so a name that is a prefix of a longer one
   is never mistaken for it. */
static const char* kelp_env_lookup(const char* name) {
    kelp_EnvTable* t = kelp_env_ensure();
    size_t name_len = strlen(name);
    size_t idx = (size_t)(kelp_env_hash(name, name_len) & (uint64_t)(t->cap - 1));
    for (size_t probes = 0; probes < t->cap; probes++) {
        const char* entry = t->slots[idx];
        if (entry == NULL) {
            return NULL;
        }
        if (strncmp(entry, name, name_len) == 0 && entry[name_len] == '=') {
            return entry + name_len + 1;
        }
        idx = (idx + 1) & (t->cap - 1);
    }
    return NULL;
}

/* ---- the dial (MISSING-w9Kda) ------------------------------------------------
   One dial, one place. It resolves `host`, tries every address `getaddrinfo`
   reported in order (so a name resolving to both ::1 and 127.0.0.1 works
   whichever one is listening), and returns the connected descriptor.

   A failure is *reported* here rather than panicking, which is what lets the
   two thin extern bodies over it share this text: `connect_raw` panics with
   the code and `connect_try_raw` turns it into the text a caller matches on.
   `*gai_rc` is the `getaddrinfo` return code (0 when resolution succeeded) and
   `*sys_errno` is the errno of the last failed `connect`; both are 0 when the
   dial returned a descriptor. */
static int kelp_rt_connect(const char* host, unsigned port, int* gai_rc, int* sys_errno) {
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    char service[8];
    snprintf(service, sizeof service, "%u", port);
    struct addrinfo* list = NULL;
    int rc = getaddrinfo(host, service, &hints, &list);
    if (rc != 0) {
        *gai_rc = rc;
        *sys_errno = 0;
        return -1;
    }
    int fd = -1;
    int last = 0;
    for (struct addrinfo* a = list; a != NULL; a = a->ai_next) {
        fd = socket(a->ai_family, a->ai_socktype, a->ai_protocol);
        if (fd < 0) {
            last = errno;
            continue;
        }
        if (connect(fd, a->ai_addr, a->ai_addrlen) == 0) {
            break;
        }
        last = errno;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(list);
    *gai_rc = 0;
    *sys_errno = last;
    return fd;
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
// ---- kelp_Thread helpers (MK9-S3-M1 stage C1) --------------------------------
// One worker's lifecycle. `kelp_thread_new` allocates the handle and the two
// slots it owns; the generated code stores the moved argument into `arg`; the
// generated per-signature `body` is what calls the closure. The record below
// carries what the trampoline needs and names no type, so one body per spawn
// signature is all either backend writes.
typedef struct {
    kelp_Thread* handle;
    /* `body(code, env, arg, out)`: call the closure `code`/`env` with the value
       at `arg` and store its result at `out`. One per spawn signature, emitted
       by whichever backend built the program, so this record names no type. */
    void (*body)(void*, void*, void*, void*);
    void* code;
    void* env;
    void (*env_drop)(void*);
} kelp_ThreadStart;

static void kelp_thread_release(void* raw) {
    kelp_ThreadStart* s = (kelp_ThreadStart*)raw;
    if (s->env != NULL && s->env_drop != NULL) {
        s->env_drop(s->env);
    }
    free(s);
}

static void* kelp_thread_tramp(void* raw) {
    kelp_ThreadStart* s = (kelp_ThreadStart*)raw;
    kelp_thread_self = s->handle;
    /* The handler runs on both ways out: a normal return and the `pthread_exit`
       a worker's panic takes. So the record and the closure's environment are
       freed either way, and what a panicking worker still abandons is its own
       frame's owned values, which is exactly the leak MK9 section 2 admits. */
    pthread_cleanup_push(kelp_thread_release, s);
    s->body(s->code, s->env, s->handle->arg, s->handle->result);
    pthread_cleanup_pop(1);
    return NULL;
}

static kelp_Thread* kelp_thread_new(size_t arg_size, size_t result_size) {
    kelp_Thread* h = (kelp_Thread*)malloc(sizeof(kelp_Thread));
    if (h == NULL) {
        kelp_panic("thread.spawn: out of memory");
    }
    h->panicked = 0;
    h->message = NULL;
    h->result = malloc(result_size == 0 ? 1 : result_size);
    h->arg = malloc(arg_size == 0 ? 1 : arg_size);
    if (h->result == NULL || h->arg == NULL) {
        kelp_panic("thread.spawn: out of memory");
    }
    return h;
}

static void kelp_thread_start(kelp_Thread* h, void (*body)(void*, void*, void*, void*), void* code, void* env, void (*env_drop)(void*)) {
    kelp_ThreadStart* s = (kelp_ThreadStart*)malloc(sizeof(kelp_ThreadStart));
    if (s == NULL) {
        kelp_panic("thread.spawn: out of memory");
    }
    s->handle = h;
    s->body = body;
    s->code = code;
    s->env = env;
    s->env_drop = env_drop;
    if (pthread_create(&h->tid, NULL, kelp_thread_tramp, s) != 0) {
        kelp_panic("thread.spawn: pthread_create failed");
    }
}

/* Waits for the worker and reports whether it panicked. `pthread_join` is the
   barrier: every slot the worker wrote is visible to this thread after it. */
static int kelp_thread_wait(kelp_Thread* h) {
    pthread_join(h->tid, NULL);
    return h->panicked;
}

static void* kelp_thread_arg(kelp_Thread* h) {
    return h->arg;
}

static void* kelp_thread_result(kelp_Thread* h) {
    return h->result;
}

static const char* kelp_thread_message(kelp_Thread* h) {
    return h->message;
}

/* Frees a joined handle: the worker has ended, so both slots are dead. The
   result's own contents were moved out by the joiner and the panic text was
   never owned, so this frees the three allocations and nothing else. */
static void kelp_thread_dispose(kelp_Thread* h) {
    free(h->arg);
    free(h->result);
    free(h);
}

/* The drop of a handle nobody joined: detach the worker and leak the handle and
   its two slots. Freeing them would be a use-after-free the moment the worker
   runs, and refusing to drop a handle is a rule the language does not have
   (MK9 section 5, stage C1). The argument is the slot the local holds, so the
   generated drop passes `&h` and the null test covers the zero value a moved
   binding leaves behind. The worker's own record is not part of this: the
   trampoline frees that on the way out, whichever way it leaves. */
static void kelp_thread_drop(kelp_Thread** slot) {
    kelp_Thread* h = *slot;
    if (h == NULL) {
        return;
    }
    pthread_detach(h->tid);
    *slot = NULL;
}

// ---- end Kelp runtime -------------------------------------------------------

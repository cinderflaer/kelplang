#include <limits.h>
#include <fcntl.h>
#include <math.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// ---- X runtime (M6) -------------------------------------------------------
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

// ---- X path runtime -------------------------------------------------------
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

// ---- X capability runtime (M7) --------------------------------------------
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

// capability allowlists, compiled in per module
static const char* x_caps_speed_env[] = { "*", NULL };
static const char* x_caps_speed_rand[] = { "*", NULL };
static const char* x_caps_speed_net[] = { "/**", NULL };
static const char* x_caps_speed_fs[] = { "/**", NULL };
static const char* x_caps_speed_process[] = { "/**", NULL };
static const char* x_caps_speed_time[] = { "*", NULL };

int64_t x_std__time__now_unix_raw(void);
int64_t x_std__time__now_mono_ns_raw(void);
void x_std__time__sleep_ms_raw(uint64_t x_ms);
int64_t x_std__time__now_unix(void);
int64_t x_std__time__now_mono_ns(void);
void x_std__time__sleep_ms(uint64_t x_ms);
int64_t x_collatz_total(int64_t x_n_end);
double x_float_sum(int64_t x_iters, int64_t* x_above);
int64_t x_mix(int64_t x_a, int64_t x_b);
int64_t x_call_chain(int64_t x_iters);
int64_t x_fib(int64_t x_n, int64_t* x_calls);

int64_t x_std__time__now_unix_raw(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        x_panic("time.now_unix: clock_gettime failed");
    }
    return (int64_t)ts.tv_sec;
}

int64_t x_std__time__now_mono_ns_raw(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        x_panic("time.now_mono_ns: clock_gettime failed");
    }
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

// extern 'std.time.sleep_ms_raw' parameters: ms -> x_ms
void x_std__time__sleep_ms_raw(uint64_t x_ms) {
    struct timespec ts;
    ts.tv_sec = (time_t)(x_ms / 1000);
    ts.tv_nsec = (long)((x_ms % 1000) * 1000000);
    while (nanosleep(&ts, &ts) != 0) {
        /* interrupted: finish the remainder */
    }
}

int64_t x_std__time__now_unix(void) {
    return x_std__time__now_unix_raw();
}

int64_t x_std__time__now_mono_ns(void) {
    return x_std__time__now_mono_ns_raw();
}

void x_std__time__sleep_ms(uint64_t x_ms) {
    x_std__time__sleep_ms_raw(x_ms);
}

int64_t x_collatz_total(int64_t x_n_end) {
    int64_t x_total = 0;
    int64_t x_n = 1;
    while ((x_n <= x_n_end)) {
        int64_t x_x = x_n;
        int64_t x_steps = 0;
        while ((x_x != 1)) {
            if (((x_x % 2) == 0)) {
                (x_x = (x_x / 2));
            } else {
                (x_x = ((x_x * 3) + 1));
            }
            (x_steps = (x_steps + 1));
        }
        (x_total = (x_total + x_steps));
        (x_n = (x_n + 1));
    }
    return x_total;
}

double x_float_sum(int64_t x_iters, int64_t* x_above) {
    double x_x = 0;
    double x_acc = 0;
    int64_t x_i = 0;
    while ((x_i < x_iters)) {
        (x_x = (x_x + 0.00000001));
        double x_v = (1 / (1 + (x_x * x_x)));
        (x_acc = (x_acc + x_v));
        if ((x_v > 0.75)) {
            ((*x_above) = ((*x_above) + 1));
        }
        (x_i = (x_i + 1));
    }
    return x_acc;
}

int64_t x_mix(int64_t x_a, int64_t x_b) {
    return (((x_a * 31) + (x_b % 97)) % 1000003);
}

int64_t x_call_chain(int64_t x_iters) {
    int64_t x_a = 12345;
    int64_t x_i = 0;
    while ((x_i < x_iters)) {
        (x_a = x_mix(x_a, x_i));
        (x_i = (x_i + 1));
    }
    return x_a;
}

int64_t x_fib(int64_t x_n, int64_t* x_calls) {
    ((*x_calls) = ((*x_calls) + 1));
    if ((x_n < 2)) {
        return x_n;
    }
    return (x_fib((x_n - 1), x_calls) + x_fib((x_n - 2), x_calls));
}

int main(void) {
    int64_t x_t0 = x_std__time__now_mono_ns();
    int64_t x_c = x_collatz_total(1000000);
    int64_t x_ns = (x_std__time__now_mono_ns() - x_t0);
    printf("RESULT collatz %lld %lld %lld\n", (long long)1000000, (long long)x_ns, (long long)x_c);
    int64_t x_above = 0;
    (x_t0 = x_std__time__now_mono_ns());
    double x_acc = x_float_sum(100000000, (&x_above));
    (x_ns = (x_std__time__now_mono_ns() - x_t0));
    printf("RESULT float %lld %lld %lld\n", (long long)100000000, (long long)x_ns, (long long)x_above);
    printf("VALUE %f\n", (double)x_acc);
    (x_t0 = x_std__time__now_mono_ns());
    int64_t x_c2 = x_call_chain(50000000);
    (x_ns = (x_std__time__now_mono_ns() - x_t0));
    printf("RESULT mix %lld %lld %lld\n", (long long)50000000, (long long)x_ns, (long long)x_c2);
    int64_t x_calls = 0;
    (x_t0 = x_std__time__now_mono_ns());
    int64_t x_c3 = x_fib(34, (&x_calls));
    (x_ns = (x_std__time__now_mono_ns() - x_t0));
    printf("RESULT fib %lld %lld %lld\n", (long long)x_calls, (long long)x_ns, (long long)x_c3);
}


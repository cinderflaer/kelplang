// ---- Kelp capability check: the glue (M7, MK7 §8) ---------------------------
//
// What is left of the capability runtime in C after its matcher moved to Kelp
// (`kelpc/std/rt.kp`). Three things stayed, and each one is deliberately not
// matcher logic:
//
//   * **The buffered plumbing.** `host:port` formatting, the `PATH_MAX`
//     scratch buffers, the `snprintf` that cuts a pattern's host part. It is
//     the same code the C reference did, so no port can change what it means.
//     The buffers are declared one byte longer than `PATH_MAX` where a Kelp
//     function writes into one: `Bytes` keeps its NUL in the slot *at* `len`,
//     and a view whose `len` is the capacity has to be an honest view of
//     something.
//   * **The deny path.** `fprintf` to stderr plus `exit(1)`, which is I/O and
//     belongs with the rest of the runtime's I/O until MK7 §7 replaces it.
//   * **The allowlist loop.** "Any pattern that admits the resource admits the
//     call" is three lines per check kind and has no matching in it; the
//     matching itself — the part a lenient port would widen — is the Kelp.
//
// `kelp_resolve_path` is still C, and is the last piece here that the matcher
// depends on: it resolves symlinks before matching, and it moves to Kelp with
// the filesystem half of the runtime (it needs `realpath` and a buffer, which is
// the same interface question the normalizer just answered).
//
// This file is emitted into every program that contains a gated call, spliced
// together with prototypes for the `std/rt` functions it calls; see
// `cdecl.rs`. It relies on the generated program's own `#include` block
// (`stdio.h`, `stdlib.h`, `string.h`, `limits.h`, and the rest).
// ---- end emission note ------------------------------------------------------

/* Collapses `//`, `.` and `..` segments of an absolute path in place, with
   `..` at the root staying at the root (what the kernel does). Used only on
   the text a symlink's target is spliced into, which has not been through the
   normalizer. */
static void kelp_collapse(char* p) {
    char o[PATH_MAX];
    size_t ol = 0;
    const char* s = p;
    while (*s) {
        while (*s == '/') {
            s++;
        }
        if (!*s) {
            break;
        }
        const char* e = s;
        while (*e && *e != '/') {
            e++;
        }
        size_t l = (size_t)(e - s);
        if (l == 1 && s[0] == '.') {
        } else if (l == 2 && s[0] == '.' && s[1] == '.') {
            while (ol > 0 && o[ol - 1] != '/') {
                ol--;
            }
            if (ol > 0) {
                ol--;
            }
        } else if (ol + 1 + l < sizeof o) {
            o[ol++] = '/';
            memcpy(o + ol, s, l);
            ol += l;
        }
        s = e;
    }
    if (ol == 0) {
        o[ol++] = '/';
    }
    o[ol] = 0;
    memcpy(p, o, ol + 1);
}

/* Symlinks are resolved *before* matching, which is the whole reason this
   exists: a link inside the grant that points outside it must be denied.
   `realpath` fails for a path that does not exist yet (a write or an
   mkdir), so the deepest existing ancestor is resolved and the rest is
   appended verbatim, **except** that the first missing component may itself be
   a dangling symlink: `open(O_CREAT)` follows it and creates the file at the
   link's target, which can be outside the grant (threat model F10). So that
   component is `readlink`ed, the target spliced in, and the walk restarts,
   at most 40 times like the kernel's own limit. It is a loop and not a
   recursion for the same reason the cut below is: the depth is the caller's. */
static bool kelp_resolve_path(const char* path, char* out, size_t cap) {
    char work[PATH_MAX];
    snprintf(work, sizeof work, "%s", path);
    for (int hops = 0; hops <= 40; hops++) {
        char real[PATH_MAX];
        if (realpath(work, real) != NULL) {
            snprintf(out, cap, "%s", real);
            return true;
        }
        /* One loop rather than one recursive call per cut. The call this
           replaces went a frame deep per path segment, so a path of about a
           thousand segments overran an 8 MiB stack and died with SIGSEGV
           instead of failing to resolve and letting the match deny.

           `tail` is an offset into `work`, not a pointer into `buf`: cutting an
           earlier slash writes a NUL over a later one, and `buf` is a
           truncated copy of `work` whose first `sizeof buf - 1` bytes are
           `work`'s. */
        char buf[PATH_MAX];
        snprintf(buf, sizeof buf, "%s", work);
        char* slash = strrchr(buf, '/');
        size_t tail = 0;
        bool found = false;
        while (slash != NULL && slash != buf) {
            tail = (size_t)(slash - buf);
            *slash = 0;
            if (realpath(buf, real) != NULL) {
                found = true;
                break;
            }
            slash = strrchr(buf, '/');
        }
        if (!found) {
            return false;
        }
        const char* rest = work + tail;          /* "/first/more/..." */
        const char* end = strchr(rest + 1, '/');  /* the end of "first" */
        size_t first_len = end != NULL ? (size_t)(end - rest) : strlen(rest);
        char cand[PATH_MAX];
        snprintf(cand, sizeof cand, "%s%.*s", strcmp(real, "/") == 0 ? "" : real, (int)first_len, rest);
        char link[PATH_MAX];
        ssize_t n = readlink(cand, link, sizeof link - 1);
        if (n < 0) {
            /* Not a link: the component is really absent, and so is all of
               what follows it. */
            snprintf(out, cap, "%s%s", strcmp(real, "/") == 0 ? "" : real, rest);
            return true;
        }
        link[n] = 0;
        const char* after = end != NULL ? end : "";
        char next[PATH_MAX];
        if (link[0] == '/') {
            snprintf(next, sizeof next, "%s%s", link, after);
        } else {
            snprintf(next, sizeof next, "%s/%s%s", strcmp(real, "/") == 0 ? "" : real, link, after);
        }
        kelp_collapse(next);
        snprintf(work, sizeof work, "%s", next);
    }
    /* A chain of more than 40 links: the kernel refuses it too (ELOOP), so what
       is matched is the path as far as it got. */
    snprintf(out, cap, "%s", work);
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
    /* One byte past `PATH_MAX`, because the normalizer is handed a borrowed
       `kelp_Bytes` *view* of this array whose `len` is the capacity, and
       `Bytes` puts its NUL in the slot at `len`: an array of exactly `len`
       bytes would make that slot a lie. The view is never dropped — it is not
       an allocation, and dropping it would free a stack address. */
    char normalized[PATH_MAX + 1];
    kelp_Bytes view;
    view.data = (unsigned char*)normalized;
    view.len = PATH_MAX;
    view.cap = PATH_MAX;
    if (!kelp_std__rt__path_normalize(path, &view)) {
        return kelp_cap_deny(domain, path, patterns, count, module, at);
    }
    char resolved[PATH_MAX];
    if (!kelp_resolve_path(normalized, resolved, sizeof resolved)) {
        snprintf(resolved, sizeof resolved, "%s", normalized);
    }
    for (size_t i = 0; i < count; i++) {
        if (kelp_std__rt__glob_match(patterns[i], resolved)) {
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
        const char* pat = patterns[i];
        /* A name domain's whole-resource grant is the lone `*`, and a `*` in
           glob_match stays within one path segment, so its verdict is exactly
           "name has no `/`". Scanning that directly skips the call into the
           Kelp matcher that both the caller's own grant and std/env.kp's own
           forwarding call take on every gated env access. */
        if (pat[0] == '*' && pat[1] == '\0') {
            if (strchr(name, '/') == NULL) {
                return;
            }
            continue;
        }
        if (kelp_std__rt__glob_match(pat, name)) {
            return;
        }
    }
    return kelp_cap_deny(domain, name, patterns, count, module, at);
}

/* Whether a pattern's host part is *numeric-shaped*: only digits, dots and
   `*`, with at least one digit (`127.0.0.1`, `10.0.0.*`). Such a pattern is a
   statement about IPv4 addresses, and a host *name* that happens to match it
   (`10.0.0.xyz` against `10.0.0.*`) is a different thing: it is resolved by
   DNS and can be anything the owner of that name says (threat model N4). */
static bool kelp_host_pattern_is_numeric(const char* pat, size_t host_len) {
    bool digit = false;
    for (size_t i = 0; i < host_len; i++) {
        char c = pat[i];
        if (c >= '0' && c <= '9') {
            digit = true;
        } else if (c != '.' && c != '*') {
            return false;
        }
    }
    return digit;
}

/* The length of a pattern's host part: everything before its last `:`. */
static size_t kelp_pattern_host_len(const char* pat) {
    const char* colon = strrchr(pat, ':');
    return colon == NULL ? strlen(pat) : (size_t)(colon - pat);
}

/* A host that a numeric-shaped pattern admitted must itself be a dotted-quad
   IPv4 literal: what `inet_pton` accepts is exactly what `getaddrinfo` will
   take without asking DNS. */
static bool kelp_numeric_pattern_admits(const char* pat, const char* host) {
    if (!kelp_host_pattern_is_numeric(pat, kelp_pattern_host_len(pat))) {
        return true;
    }
    struct in_addr addr;
    return inet_pton(AF_INET, host, &addr) == 1;
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
        if (kelp_std__rt__host_glob(pat, host) && kelp_numeric_pattern_admits(pat, host)) {
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
            if (kelp_std__rt__host_glob(head, host) && kelp_numeric_pattern_admits(head, host)) {
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
        if (kelp_std__rt__host_match(patterns[i], hostport) && kelp_numeric_pattern_admits(patterns[i], host)) {
            return;
        }
    }
    return kelp_cap_deny(domain, hostport, patterns, count, module, at);
}

/* A port on its own (`net.listen`). The grant's port is what governs a
   listener, and its *host* decides which interfaces the listener may bind:
   `net.listen` has no host argument, so the check leaves a verdict in
   `kelp_listen_loopback` for the `listen_raw` that follows. A pattern whose host
   part admits `0.0.0.0` (`*`, `0.0.0.0`, `0.0.0.*`...) or that names no host at
   all admits every interface; a grant of only loopback or named hosts
   (`127.0.0.1:*`) gets a listener on `127.0.0.1` alone, so a loopback-only grant
   cannot be turned into a public server (threat model N5). */
static void kelp_cap_check_port(const char* domain, uint16_t port, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    char text[32];
    char number[16];
    snprintf(number, sizeof number, "%u", (unsigned)port);
    snprintf(text, sizeof text, "*:%s", number);
    bool port_ok = false;
    bool every_interface = false;
    for (size_t i = 0; i < count; i++) {
        const char* pat = patterns[i];
        if (strcmp(pat, "*") == 0) {
            port_ok = true;
            every_interface = true;
            continue;
        }
        const char* colon = strrchr(pat, ':');
        if (colon == NULL) {
            /* a host-only pattern constrains no port, and names no interface */
            port_ok = true;
            char head[PATH_MAX];
            snprintf(head, sizeof head, "%s", pat);
            if (strcmp(head, "*") == 0 || kelp_std__rt__host_glob(head, "0.0.0.0")) {
                every_interface = true;
            }
            continue;
        }
        if (kelp_std__rt__host_glob(colon + 1, number)) {
            port_ok = true;
            char head[PATH_MAX];
            size_t n = (size_t)(colon - pat);
            if (n >= sizeof head) {
                n = sizeof head - 1;
            }
            memcpy(head, pat, n);
            head[n] = 0;
            if (strcmp(head, "*") == 0 || kelp_std__rt__host_glob(head, "0.0.0.0")) {
                every_interface = true;
            }
        }
    }
    if (!port_ok) {
        return kelp_cap_deny(domain, text, patterns, count, module, at);
    }
    kelp_listen_loopback = every_interface ? 0 : 1;
}

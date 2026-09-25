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
// `codegen_c.rs`. It relies on the generated program's own `#include` block
// (`stdio.h`, `stdlib.h`, `string.h`, `limits.h`, and the rest).
// ---- end emission note ------------------------------------------------------

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
    /* One loop rather than one recursive call per cut. The call this replaces
       went a frame deep per path segment, so a path of about a thousand
       segments overran an 8 MiB stack and died with SIGSEGV instead of failing
       to resolve and letting the match deny. The depth is the caller's, and a
       caller can name any path.

       `tail` is an offset into `path`, not a pointer into `buf`: cutting an
       earlier slash writes a NUL over a later one, and `buf` is a truncated
       copy of `path` whose first `sizeof buf - 1` bytes are `path`'s. */
    char buf[PATH_MAX];
    snprintf(buf, sizeof buf, "%s", path);
    char* slash = strrchr(buf, '/');
    while (slash != NULL && slash != buf) {
        size_t tail = (size_t)(slash - buf);
        *slash = 0;
        if (realpath(buf, real) != NULL) {
            snprintf(out, cap, "%s%s", real, path + tail);
            return true;
        }
        slash = strrchr(buf, '/');
    }
    return false;
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

/* A host on its own (`net.resolve`): resolution is not port-specific, so a
   pattern's port, if it has one, is ignored. */
static void kelp_cap_check_host(const char* domain, const char* host, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    for (size_t i = 0; i < count; i++) {
        const char* pat = patterns[i];
        if (strcmp(pat, "*") == 0) {
            return;
        }
        if (kelp_std__rt__host_glob(pat, host)) {
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
            if (kelp_std__rt__host_glob(head, host)) {
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
        if (kelp_std__rt__host_match(patterns[i], hostport)) {
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
        if (kelp_std__rt__host_glob(colon + 1, number)) {
            return;
        }
    }
    return kelp_cap_deny(domain, text, patterns, count, module, at);
}

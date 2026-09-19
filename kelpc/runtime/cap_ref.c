// ---- Kelp capability runtime (M7) --------------------------------------------
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

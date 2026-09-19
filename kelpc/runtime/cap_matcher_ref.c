// ---- Kelp capability matcher: the C reference (M7, MK7 §8) ------------------
//
// This is the implementation the capability checks ran before MK7 §8 moved the
// matching to Kelp (`kelpc/std/rt.kp`). It is **not** emitted into programs any
// more; nothing includes it except `kelpc/tests/cap_matcher.rs`, which runs a
// pattern corpus through this and through the Kelp version and compares every
// verdict.
//
// It is kept, rather than deleted along with its C text, because a port of a
// *security* check has no other oracle: the fixtures prove the port kept the
// grants working, and only a differential corpus proves it kept the verdicts
// identical — including for the shapes nobody wrote a fixture for. The plan's
// risk register names this exact failure ("a lenient port widens every grant").
//
// The port had to reproduce two things beyond the obvious loops:
//
//   * `kelp_host_part` copies the host out of `host:port` into a `PATH_MAX`
//     buffer, so a host part of 4096 bytes or more is cut to 4095. The Kelp
//     version bounds the match at the same index instead of copying, and the
//     corpus covers lengths either side of that number so the bound is
//     exercised rather than assumed.
//   * `kelp_cap_check_host` cuts a pattern's host part the same way
//     (`char head[PATH_MAX]`). That cut is the *glue*'s, still in C
//     (`kelpc/runtime/cap_glue.c`), so it is unchanged by the port.
#include <limits.h>
#include <stdbool.h>
#include <string.h>

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

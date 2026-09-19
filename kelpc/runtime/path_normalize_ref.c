// ---- Kelp path normalization: the C reference (M6.3, MK7 §8) ----------------
//
// This is the normalization `path.normalize` and the capability check ran
// before MK7 §8 moved it to Kelp (`kelpc/std/rt.kp`, `path_normalize`). Nothing
// includes it but `kelpc/tests/path_normalize.rs`, which feeds a corpus of paths
// to both implementations and compares the verdict *and* the bytes each leaves
// in the buffer.
//
// It is kept as a file rather than deleted with its C text because §8's contract
// is "the C version stays as the reference until the replacement is proven", and
// because what this function decides is a security boundary: a path that
// normalizes to something *inside* the grant while resolving outside it is the
// escape this rejects. The fixtures cover the escapes that were thought of;
// only a differential corpus covers the shapes that were not.
#include <stdbool.h>
#include <stddef.h>
#include <string.h>

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

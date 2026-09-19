// ---- Kelp path runtime -------------------------------------------------------
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

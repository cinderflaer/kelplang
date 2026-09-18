//! Compiler-supplied C bodies for `extern fun` declarations (M6.2).
//!
//! Stdlib decision 3 in `plan.md`: the stdlib ships as `.x` sources, and the
//! few operations that cannot be written in X at all — a raw syscall, a
//! `malloc`, a byte read out of a `&str` — are declared `extern` and have
//! their body supplied here. That keeps the *policy* (the public function,
//! its checks, its documentation) in reviewable X source while confining the
//! unportable primitive to one place.
//!
//! Keyed by `(module id, bare function name)`, e.g. `("std/mem",
//! "alloc_raw")`. A module id is the `impl` path (`std/mem`), not the dotted
//! symbol prefix. An `extern` with no entry here is rejected in `infer.rs`,
//! not silently left undefined — which is also why user code cannot declare
//! one: only compiler-shipped modules have bodies.

/// One compiler-supplied body: the module that may use it, the name written
/// in that module's `extern fun` declaration, and the C statements (already
/// indented one level, ending in a newline).
static EXTERNS: &[(&str, &str, &str)] = &[
    (
        "std/mem",
        "alloc_raw",
        "    return x_bytes_alloc_raw(x_n);\n",
    ),
    (
        "std/str",
        "byte_len_raw",
        "    return strlen(x_s);\n",
    ),
    (
        "std/str",
        "byte_at_raw",
        "    if (x_i >= strlen(x_s)) {\n        x_panic(\"str.byte: index out of bounds\");\n    }\n    return (uint8_t)x_s[x_i];\n",
    ),
    (
        "std/str",
        "slice_from_raw",
        "    if (x_from > strlen(x_s)) {\n        x_panic(\"str.substring: index out of bounds\");\n    }\n    return x_s + x_from;\n",
    ),
    (
        "std/str",
        "from_f64_raw",
        "    int need = snprintf(NULL, 0, \"%.*f\", (int)x_decimals, x_x);\n    if (need < 0) {\n        x_panic(\"str.from_f64: formatting failed\");\n    }\n    x_Bytes b = x_bytes_alloc_raw((size_t)need);\n    snprintf((char*)b.data, b.len + 1, \"%.*f\", (int)x_decimals, x_x);\n    return x_String__from_bytes(b);\n",
    ),
    (
        "std/str",
        "parse_i64_raw",
        "    if (x_s[0] == 0) {\n        return false;\n    }\n    for (const char* p = x_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    errno = 0;\n    long long v = strtoll(x_s, &end, 10);\n    if (end == x_s || *end != 0 || errno != 0) {\n        return false;\n    }\n    *x_out = (int64_t)v;\n    return true;\n",
    ),
    (
        "std/str",
        "parse_u64_raw",
        "    if (x_s[0] == 0 || x_s[0] == '-') {\n        return false;\n    }\n    for (const char* p = x_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    errno = 0;\n    unsigned long long v = strtoull(x_s, &end, 10);\n    if (end == x_s || *end != 0 || errno != 0) {\n        return false;\n    }\n    *x_out = (uint64_t)v;\n    return true;\n",
    ),
    (
        "std/str",
        "parse_f64_raw",
        "    if (x_s[0] == 0) {\n        return false;\n    }\n    for (const char* p = x_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    double v = strtod(x_s, &end);\n    if (end == x_s || *end != 0) {\n        return false;\n    }\n    *x_out = v;\n    return true;\n",
    ),
    (
        "std/fmt",
        "hex_raw",
        "    char buf[32];\n    snprintf(buf, sizeof buf, \"%llx\", (unsigned long long)x_n);\n    return x_String__from_str(buf);\n",
    ),
    (
        "std/fmt",
        "bytes_raw",
        "    static const char* hex = \"0123456789abcdef\";\n    if (x_b->len == 0) {\n        return x_String__empty();\n    }\n    x_Bytes out = x_bytes_alloc_raw(x_b->len * 3 - 1);\n    size_t at = 0;\n    for (size_t i = 0; i < x_b->len; i++) {\n        if (i > 0) {\n            out.data[at++] = ' ';\n        }\n        out.data[at++] = (unsigned char)hex[(x_b->data[i] >> 4) & 0xF];\n        out.data[at++] = (unsigned char)hex[x_b->data[i] & 0xF];\n    }\n    return x_String__from_bytes(out);\n",
    ),
    (
        "std/math",
        "sqrt_raw",
        "    return sqrt(x_x);\n",
    ),
    (
        "std/math",
        "pow_raw",
        "    return pow(x_base, x_exp);\n",
    ),
    (
        "std/math",
        "floor_raw",
        "    return floor(x_x);\n",
    ),
    (
        "std/math",
        "ceil_raw",
        "    return ceil(x_x);\n",
    ),
    (
        "std/math",
        "round_raw",
        "    return round(x_x);\n",
    ),
    (
        "std/math",
        "trunc_raw",
        "    return trunc(x_x);\n",
    ),
    (
        "std/math",
        "exp_raw",
        "    return exp(x_x);\n",
    ),
    (
        "std/math",
        "ln_raw",
        "    return log(x_x);\n",
    ),
    (
        "std/math",
        "log2_raw",
        "    return log2(x_x);\n",
    ),
    (
        "std/math",
        "log10_raw",
        "    return log10(x_x);\n",
    ),
    (
        "std/math",
        "sin_raw",
        "    return sin(x_x);\n",
    ),
    (
        "std/math",
        "cos_raw",
        "    return cos(x_x);\n",
    ),
    (
        "std/math",
        "tan_raw",
        "    return tan(x_x);\n",
    ),
    (
        "std/math",
        "atan2_raw",
        "    return atan2(x_y, x_x);\n",
    ),
    (
        "std/math",
        "is_nan_raw",
        "    return isnan(x_x);\n",
    ),
    (
        "std/math",
        "is_finite_raw",
        "    return isfinite(x_x);\n",
    ),
    (
        "std/fs",
        "read_raw",
        "    FILE* f = fopen(x_path, \"rb\");\n    if (f == NULL) {\n        x_panic(\"fs.read: cannot open file\");\n    }\n    if (fseek(f, 0, SEEK_END) != 0) {\n        fclose(f);\n        x_panic(\"fs.read: cannot seek\");\n    }\n    long n = ftell(f);\n    if (n < 0) {\n        fclose(f);\n        x_panic(\"fs.read: cannot size file\");\n    }\n    rewind(f);\n    /* zero-filled and NUL-terminated, so Bytes.as_str works on the result */\n    x_Bytes b = x_bytes_alloc_raw((size_t)n);\n    if (n > 0 && fread(b.data, 1, (size_t)n, f) != (size_t)n) {\n        fclose(f);\n        x_panic(\"fs.read: short read\");\n    }\n    fclose(f);\n    return b;\n",
    ),
    (
        "std/fs",
        "write_raw",
        "    FILE* f = fopen(x_path, \"wb\");\n    if (f == NULL) {\n        x_panic(\"fs.write: cannot open file\");\n    }\n    if (x_data->len > 0 && fwrite(x_data->data, 1, x_data->len, f) != x_data->len) {\n        fclose(f);\n        x_panic(\"fs.write: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "write_text_raw",
        "    FILE* f = fopen(x_path, \"wb\");\n    if (f == NULL) {\n        x_panic(\"fs.write_text: cannot open file\");\n    }\n    if (x_text->len > 0 && fwrite(x_text->data, 1, x_text->len, f) != x_text->len) {\n        fclose(f);\n        x_panic(\"fs.write_text: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "exists_raw",
        "    struct stat st;\n    return stat(x_path, &st) == 0;\n",
    ),
    (
        "std/fs",
        "is_file_raw",
        "    struct stat st;\n    return stat(x_path, &st) == 0 && S_ISREG(st.st_mode);\n",
    ),
    (
        "std/fs",
        "is_dir_raw",
        "    struct stat st;\n    return stat(x_path, &st) == 0 && S_ISDIR(st.st_mode);\n",
    ),
    (
        "std/fs",
        "size_raw",
        "    struct stat st;\n    if (stat(x_path, &st) != 0) {\n        x_panic(\"fs.size: cannot stat file\");\n    }\n    return (uint64_t)st.st_size;\n",
    ),
    (
        "std/fs",
        "remove_raw",
        "    if (remove(x_path) != 0) {\n        x_panic(\"fs.remove: cannot remove file\");\n    }\n",
    ),
    (
        "std/fs",
        "rename_raw",
        "    if (rename(x_from, x_to) != 0) {\n        x_panic(\"fs.rename: cannot rename file\");\n    }\n",
    ),
    (
        "std/fs",
        "mkdir_raw",
        "    if (mkdir(x_path, 0777) != 0) {\n        x_panic(\"fs.mkdir: cannot create directory\");\n    }\n",
    ),
    (
        "std/fs",
        "append_raw",
        "    FILE* f = fopen(x_path, \"ab\");\n    if (f == NULL) {\n        x_panic(\"fs.append: cannot open file\");\n    }\n    if (x_data->len > 0 && fwrite(x_data->data, 1, x_data->len, f) != x_data->len) {\n        fclose(f);\n        x_panic(\"fs.append: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "list_raw",
        "    struct dirent** names;\n    int n = scandir(x_path, &names, NULL, alphasort);\n    if (n < 0) {\n        x_panic(\"fs.list: cannot read directory\");\n    }\n    size_t total = 0;\n    for (int i = 0; i < n; i++) {\n        if (strcmp(names[i]->d_name, \".\") == 0 || strcmp(names[i]->d_name, \"..\") == 0) {\n            continue;\n        }\n        total += strlen(names[i]->d_name) + 1;\n    }\n    x_Bytes b = x_bytes_alloc_raw(total);\n    size_t at = 0;\n    for (int i = 0; i < n; i++) {\n        if (strcmp(names[i]->d_name, \".\") == 0 || strcmp(names[i]->d_name, \"..\") == 0) {\n            free(names[i]);\n            continue;\n        }\n        size_t len = strlen(names[i]->d_name);\n        memcpy(b.data + at, names[i]->d_name, len);\n        at += len;\n        b.data[at++] = '\\n';\n        free(names[i]);\n    }\n    free(names);\n    if (at > 0) {\n        at--;\n    }\n    b.data[at] = 0;\n    b.len = at;\n    return x_String__from_bytes(b);\n",
    ),
    (
        "std/fs",
        "sync_raw",
        "    int fd = open(x_path, O_RDONLY);\n    if (fd < 0) {\n        x_panic(\"fs.sync: cannot open file\");\n    }\n    if (fsync(fd) != 0) {\n        close(fd);\n        x_panic(\"fs.sync: fsync failed\");\n    }\n    close(fd);\n",
    ),
    (
        "std/time",
        "now_unix_raw",
        "    struct timespec ts;\n    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {\n        x_panic(\"time.now_unix: clock_gettime failed\");\n    }\n    return (int64_t)ts.tv_sec;\n",
    ),
    (
        "std/time",
        "now_mono_ns_raw",
        "    struct timespec ts;\n    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {\n        x_panic(\"time.now_mono_ns: clock_gettime failed\");\n    }\n    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;\n",
    ),
    (
        "std/time",
        "sleep_ms_raw",
        "    struct timespec ts;\n    ts.tv_sec = (time_t)(x_ms / 1000);\n    ts.tv_nsec = (long)((x_ms % 1000) * 1000000);\n    while (nanosleep(&ts, &ts) != 0) {\n        /* interrupted: finish the remainder */\n    }\n",
    ),
    (
        "std/rand",
        "u64_raw",
        "    uint64_t v = 0;\n    FILE* f = fopen(\"/dev/urandom\", \"rb\");\n    if (f == NULL) {\n        x_panic(\"rand.u64: no entropy source\");\n    }\n    if (fread(&v, 1, sizeof v, f) != sizeof v) {\n        fclose(f);\n        x_panic(\"rand.u64: short read from /dev/urandom\");\n    }\n    fclose(f);\n    return v;\n",
    ),
    (
        "std/rand",
        "fill_raw",
        "    FILE* f = fopen(\"/dev/urandom\", \"rb\");\n    if (f == NULL) {\n        x_panic(\"rand.bytes: no entropy source\");\n    }\n    if (x_buf->len > 0 && fread(x_buf->data, 1, x_buf->len, f) != x_buf->len) {\n        fclose(f);\n        x_panic(\"rand.bytes: short read from /dev/urandom\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/path",
        "normalize_raw",
        "    char buf[PATH_MAX];\n    if (!x_path_normalize(x_s, buf, sizeof buf)) {\n        x_panic(\"path.normalize: path must be absolute and must not escape its own root\");\n    }\n    x_Bytes b = x_bytes_alloc_raw(strlen(buf));\n    memcpy(b.data, buf, b.len);\n    return x_String__from_bytes(b);\n",
    ),
    (
        "std/env",
        "unsupported_bool",
        "    (void)x_what;\n    x_panic(\"env: the 'env' namespace is gated but not implemented yet\");\n    return false;\n",
    ),
    (
        "std/env",
        "unsupported_string",
        "    (void)x_what;\n    x_panic(\"env: the 'env' namespace is gated but not implemented yet\");\n    return x_String__empty();\n",
    ),
    (
        "std/env",
        "unsupported_unit",
        "    (void)x_what;\n    x_panic(\"env: the 'env' namespace is gated but not implemented yet\");\n",
    ),
];

/// The C body for an `extern fun` in `module`, or `None` when the compiler
/// has no body for it. `qualified_name` is the function's `tir::Function`
/// key, which `resolve.rs` has already prefixed with the module path.
pub fn body(module: &str, qualified_name: &str) -> Option<&'static str> {
    let prefix = if module.is_empty() {
        String::new()
    } else {
        format!("{}.", module.replace('/', "."))
    };
    let bare = qualified_name
        .strip_prefix(prefix.as_str())
        .unwrap_or(qualified_name);
    EXTERNS
        .iter()
        .find(|(m, n, _)| *m == module && *n == bare)
        .map(|(_, _, body)| *body)
}

/// Whether the compiler ships a body for this `(module, bare name)` pair.
/// Used by `infer.rs` to reject an `extern` declaration it cannot honor.
pub fn has_body(module: &str, bare_name: &str) -> bool {
    EXTERNS
        .iter()
        .any(|(m, n, _)| *m == module && *n == bare_name)
}

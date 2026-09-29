//! Compiler-supplied C bodies for `extern fun` declarations (M6.2).
//!
//! Stdlib decision 3 in the plan: the stdlib ships as `.kp` sources, and the
//! few operations that cannot be written in Kelp at all — a raw syscall, a
//! `malloc`, a byte read out of a `&str` — are declared `extern` and have
//! their body supplied here. That keeps the *policy* (the public function,
//! its checks, its documentation) in reviewable Kelp source while confining the
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
        "    return kelp_bytes_alloc_raw(kelp_n);\n",
    ),
    (
        "std/str",
        "byte_len_raw",
        "    return strlen(kelp_s);\n",
    ),
    (
        "std/str",
        "byte_at_raw",
        "    if (kelp_i >= strlen(kelp_s)) {\n        kelp_panic(\"str.byte: index out of bounds\");\n    }\n    return (uint8_t)kelp_s[kelp_i];\n",
    ),
    (
        "std/str",
        "slice_from_raw",
        "    if (kelp_from > strlen(kelp_s)) {\n        kelp_panic(\"str.substring: index out of bounds\");\n    }\n    return kelp_s + kelp_from;\n",
    ),
    (
        "std/str",
        "from_f64_raw",
        "    int need = snprintf(NULL, 0, \"%.*f\", (int)kelp_decimals, kelp_x);\n    if (need < 0) {\n        kelp_panic(\"str.from_f64: formatting failed\");\n    }\n    kelp_Bytes b = kelp_bytes_alloc_raw((size_t)need);\n    snprintf((char*)b.data, b.len + 1, \"%.*f\", (int)kelp_decimals, kelp_x);\n    return kelp_String__from_bytes(b);\n",
    ),
    (
        "std/str",
        "parse_i64_raw",
        "    if (kelp_s[0] == 0) {\n        return false;\n    }\n    for (const char* p = kelp_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    errno = 0;\n    long long v = strtoll(kelp_s, &end, 10);\n    if (end == kelp_s || *end != 0 || errno != 0) {\n        return false;\n    }\n    *kelp_out = (int64_t)v;\n    return true;\n",
    ),
    (
        "std/str",
        "parse_u64_raw",
        "    if (kelp_s[0] == 0 || kelp_s[0] == '-') {\n        return false;\n    }\n    for (const char* p = kelp_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    errno = 0;\n    unsigned long long v = strtoull(kelp_s, &end, 10);\n    if (end == kelp_s || *end != 0 || errno != 0) {\n        return false;\n    }\n    *kelp_out = (uint64_t)v;\n    return true;\n",
    ),
    (
        "std/str",
        "parse_f64_raw",
        "    if (kelp_s[0] == 0) {\n        return false;\n    }\n    for (const char* p = kelp_s; *p != 0; p++) {\n        if (*p == ' ' || *p == '\\t' || *p == '\\n' || *p == '\\r') {\n            return false;\n        }\n    }\n    char* end = NULL;\n    double v = strtod(kelp_s, &end);\n    if (end == kelp_s || *end != 0) {\n        return false;\n    }\n    *kelp_out = v;\n    return true;\n",
    ),
    (
        "std/fmt",
        "hex_raw",
        "    char buf[32];\n    snprintf(buf, sizeof buf, \"%llx\", (unsigned long long)kelp_n);\n    return kelp_String__from_str(buf);\n",
    ),
    (
        "std/fmt",
        "bytes_raw",
        "    static const char* hex = \"0123456789abcdef\";\n    if (kelp_b->len == 0) {\n        return kelp_String__empty();\n    }\n    kelp_Bytes out = kelp_bytes_alloc_raw(kelp_b->len * 3 - 1);\n    size_t at = 0;\n    for (size_t i = 0; i < kelp_b->len; i++) {\n        if (i > 0) {\n            out.data[at++] = ' ';\n        }\n        out.data[at++] = (unsigned char)hex[(kelp_b->data[i] >> 4) & 0xF];\n        out.data[at++] = (unsigned char)hex[kelp_b->data[i] & 0xF];\n    }\n    return kelp_String__from_bytes(out);\n",
    ),
    (
        "std/math",
        "sqrt_raw",
        "    return sqrt(kelp_x);\n",
    ),
    (
        "std/math",
        "pow_raw",
        "    return pow(kelp_base, kelp_exp);\n",
    ),
    (
        "std/math",
        "floor_raw",
        "    return floor(kelp_x);\n",
    ),
    (
        "std/math",
        "ceil_raw",
        "    return ceil(kelp_x);\n",
    ),
    (
        "std/math",
        "round_raw",
        "    return round(kelp_x);\n",
    ),
    (
        "std/math",
        "trunc_raw",
        "    return trunc(kelp_x);\n",
    ),
    (
        "std/math",
        "exp_raw",
        "    return exp(kelp_x);\n",
    ),
    (
        "std/math",
        "ln_raw",
        "    return log(kelp_x);\n",
    ),
    (
        "std/math",
        "log2_raw",
        "    return log2(kelp_x);\n",
    ),
    (
        "std/math",
        "log10_raw",
        "    return log10(kelp_x);\n",
    ),
    (
        "std/math",
        "sin_raw",
        "    return sin(kelp_x);\n",
    ),
    (
        "std/math",
        "cos_raw",
        "    return cos(kelp_x);\n",
    ),
    (
        "std/math",
        "tan_raw",
        "    return tan(kelp_x);\n",
    ),
    (
        "std/math",
        "atan2_raw",
        "    return atan2(kelp_y, kelp_x);\n",
    ),
    (
        "std/math",
        "is_nan_raw",
        "    return isnan(kelp_x);\n",
    ),
    (
        "std/math",
        "is_finite_raw",
        "    return isfinite(kelp_x);\n",
    ),
    (
        "std/fs",
        "read_raw",
        "    FILE* f = fopen(kelp_path, \"rb\");\n    if (f == NULL) {\n        kelp_panic_errno(\"fs.read: cannot open file\");\n    }\n    if (fseek(f, 0, SEEK_END) != 0) {\n        int e = errno;\n        fclose(f);\n        kelp_panic_at(\"fs.read: cannot seek\", e);\n    }\n    long n = ftell(f);\n    if (n < 0) {\n        int e = errno;\n        fclose(f);\n        kelp_panic_at(\"fs.read: cannot size file\", e);\n    }\n    rewind(f);\n    /* zero-filled and NUL-terminated, so Bytes.as_str works on the result */\n    kelp_Bytes b = kelp_bytes_alloc_raw((size_t)n);\n    if (n > 0 && fread(b.data, 1, (size_t)n, f) != (size_t)n) {\n        fclose(f);\n        kelp_panic(\"fs.read: short read\");\n    }\n    fclose(f);\n    return b;\n",
    ),
    (
        "std/fs",
        "write_raw",
        "    FILE* f = fopen(kelp_path, \"wb\");\n    if (f == NULL) {\n        kelp_panic_errno(\"fs.write: cannot open file\");\n    }\n    if (kelp_data->len > 0 && fwrite(kelp_data->data, 1, kelp_data->len, f) != kelp_data->len) {\n        fclose(f);\n        kelp_panic(\"fs.write: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "write_text_raw",
        "    FILE* f = fopen(kelp_path, \"wb\");\n    if (f == NULL) {\n        kelp_panic_errno(\"fs.write_text: cannot open file\");\n    }\n    if (kelp_text->len > 0 && fwrite(kelp_text->data, 1, kelp_text->len, f) != kelp_text->len) {\n        fclose(f);\n        kelp_panic(\"fs.write_text: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "exists_raw",
        "    struct stat st;\n    return stat(kelp_path, &st) == 0;\n",
    ),
    (
        "std/fs",
        "is_file_raw",
        "    struct stat st;\n    return stat(kelp_path, &st) == 0 && S_ISREG(st.st_mode);\n",
    ),
    (
        "std/fs",
        "is_dir_raw",
        "    struct stat st;\n    return stat(kelp_path, &st) == 0 && S_ISDIR(st.st_mode);\n",
    ),
    (
        "std/fs",
        "size_raw",
        "    struct stat st;\n    if (stat(kelp_path, &st) != 0) {\n        kelp_panic_errno(\"fs.size: cannot stat file\");\n    }\n    return (uint64_t)st.st_size;\n",
    ),
    (
        "std/fs",
        "remove_raw",
        "    if (remove(kelp_path) != 0) {\n        kelp_panic_errno(\"fs.remove: cannot remove file\");\n    }\n",
    ),
    (
        "std/fs",
        "rename_raw",
        "    if (rename(kelp_from, kelp_to) != 0) {\n        kelp_panic_errno(\"fs.rename: cannot rename file\");\n    }\n",
    ),
    (
        "std/fs",
        "mkdir_raw",
        "    if (mkdir(kelp_path, 0777) != 0) {\n        kelp_panic_errno(\"fs.mkdir: cannot create directory\");\n    }\n",
    ),
    (
        "std/fs",
        "mkdir_all_raw",
        "    char buf[PATH_MAX];\n    size_t n = strlen(kelp_path);\n    if (n == 0 || n >= sizeof buf) {\n        kelp_panic(\"fs.mkdir_all: path is empty or too long\");\n    }\n    memcpy(buf, kelp_path, n + 1);\n    /* Every prefix, in order: an existing component is success, anything\n       else is a failure. Done here rather than in Kelp so that the grant is\n       checked once, on the whole path, and never on a prefix above it. */\n    for (size_t i = 1; i <= n; i++) {\n        if (buf[i] == '/' || buf[i] == 0) {\n            char saved = buf[i];\n            buf[i] = 0;\n            if (mkdir(buf, 0777) != 0 && errno != EEXIST) {\n                kelp_panic_errno(\"fs.mkdir_all: cannot create directory\");\n            }\n            buf[i] = saved;\n        }\n    }\n",
    ),
    (
        "std/fs",
        "append_raw",
        "    FILE* f = fopen(kelp_path, \"ab\");\n    if (f == NULL) {\n        kelp_panic_errno(\"fs.append: cannot open file\");\n    }\n    if (kelp_data->len > 0 && fwrite(kelp_data->data, 1, kelp_data->len, f) != kelp_data->len) {\n        fclose(f);\n        kelp_panic(\"fs.append: short write\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/fs",
        "list_raw",
        "    struct dirent** names;\n    int n = scandir(kelp_path, &names, NULL, alphasort);\n    if (n < 0) {\n        kelp_panic_errno(\"fs.list: cannot read directory\");\n    }\n    size_t total = 0;\n    for (int i = 0; i < n; i++) {\n        if (strcmp(names[i]->d_name, \".\") == 0 || strcmp(names[i]->d_name, \"..\") == 0) {\n            continue;\n        }\n        total += strlen(names[i]->d_name) + 1;\n    }\n    kelp_Bytes b = kelp_bytes_alloc_raw(total);\n    size_t at = 0;\n    for (int i = 0; i < n; i++) {\n        if (strcmp(names[i]->d_name, \".\") == 0 || strcmp(names[i]->d_name, \"..\") == 0) {\n            free(names[i]);\n            continue;\n        }\n        size_t len = strlen(names[i]->d_name);\n        memcpy(b.data + at, names[i]->d_name, len);\n        at += len;\n        b.data[at++] = '\\n';\n        free(names[i]);\n    }\n    free(names);\n    if (at > 0) {\n        at--;\n    }\n    b.data[at] = 0;\n    b.len = at;\n    return kelp_String__from_bytes(b);\n",
    ),
    (
        "std/fs",
        "sync_raw",
        "    int fd = open(kelp_path, O_RDONLY);\n    if (fd < 0) {\n        kelp_panic_errno(\"fs.sync: cannot open file\");\n    }\n    if (fsync(fd) != 0) {\n        int e = errno;\n        close(fd);\n        kelp_panic_at(\"fs.sync: fsync failed\", e);\n    }\n    close(fd);\n",
    ),
    (
        "std/fs",
        "open_raw",
        "    int flags;\n    if (strcmp(kelp_mode, \"r\") == 0) {\n        flags = O_RDONLY;\n    } else if (strcmp(kelp_mode, \"w\") == 0) {\n        flags = O_WRONLY | O_CREAT | O_TRUNC;\n    } else if (strcmp(kelp_mode, \"a\") == 0) {\n        flags = O_WRONLY | O_CREAT | O_APPEND;\n    } else if (strcmp(kelp_mode, \"rw\") == 0) {\n        flags = O_RDWR | O_CREAT;\n    } else {\n        kelp_panic(\"fs.open: mode must be \\\"r\\\", \\\"w\\\", \\\"a\\\" or \\\"rw\\\"\");\n        return -1;\n    }\n    int fd = open(kelp_path, flags, 0666);\n    if (fd < 0) {\n        kelp_panic_errno(\"fs.open: cannot open file\");\n    }\n    return fd;\n",
    ),
    (
        "std/fs",
        "symlink_raw",
        "    if (symlink(kelp_target, kelp_link) != 0) {\n        kelp_panic_errno(\"fs.symlink: cannot create symlink\");\n    }\n",
    ),
    (
        "std/fs",
        "copy_raw",
        "    int in = open(kelp_from, O_RDONLY);\n    if (in < 0) {\n        kelp_panic_errno(\"fs.copy: cannot open source\");\n    }\n    struct stat st;\n    if (fstat(in, &st) != 0) {\n        int e = errno;\n        close(in);\n        kelp_panic_at(\"fs.copy: cannot stat source\", e);\n    }\n    int out = open(kelp_to, O_WRONLY | O_CREAT | O_TRUNC, 0666);\n    if (out < 0) {\n        int e = errno;\n        close(in);\n        kelp_panic_at(\"fs.copy: cannot open destination\", e);\n    }\n    off_t at = 0;\n    while (at < st.st_size) {\n        ssize_t n = sendfile(out, in, &at, (size_t)(st.st_size - at));\n        if (n < 0) {\n            int e = errno;\n            close(in);\n            close(out);\n            kelp_panic_at(\"fs.copy: copy failed\", e);\n        }\n        if (n == 0) {\n            break;\n        }\n    }\n    close(in);\n    close(out);\n",
    ),
    (
        "std/fs",
        "touch_raw",
        "    int fd = open(kelp_path, O_WRONLY | O_CREAT, 0666);\n    if (fd < 0) {\n        kelp_panic_errno(\"fs.touch: cannot create file\");\n    }\n    close(fd);\n    if (utimensat(AT_FDCWD, kelp_path, NULL, 0) != 0) {\n        kelp_panic_errno(\"fs.touch: cannot update timestamp\");\n    }\n",
    ),
    (
        "std/fs",
        "temp_path_raw",
        "    char tmpl[PATH_MAX];\n    int written = snprintf(tmpl, sizeof tmpl, \"%sXXXXXX\", kelp_prefix);\n    if (written < 0 || (size_t)written >= sizeof tmpl) {\n        kelp_panic(\"fs.temp_path: prefix is too long\");\n    }\n    int fd = mkstemp(tmpl);\n    if (fd < 0) {\n        kelp_panic_errno(\"fs.temp_path: cannot create temporary file\");\n    }\n    close(fd);\n    return kelp_String__from_str(tmpl);\n",
    ),
    (
        "std/fs",
        "mtime_raw",
        "    struct stat st;\n    if (stat(kelp_path, &st) != 0) {\n        kelp_panic_errno(\"fs.metadata: cannot stat file\");\n    }\n    return (int64_t)st.st_mtime;\n",
    ),
    (
        "std/fs",
        "mode_raw",
        "    struct stat st;\n    if (stat(kelp_path, &st) != 0) {\n        kelp_panic_errno(\"fs.metadata: cannot stat file\");\n    }\n    return (uint32_t)(st.st_mode & 07777);\n",
    ),
    (
        "std/fs",
        "metadata_raw",
        "    struct stat st;\n    if (stat(kelp_path, &st) != 0) {\n        kelp_panic_errno(\"fs.metadata: cannot stat file\");\n    }\n    *kelp_size = (uint64_t)st.st_size;\n    *kelp_mtime = (int64_t)st.st_mtime;\n    *kelp_mode = (uint32_t)(st.st_mode & 07777);\n    *kelp_is_dir = S_ISDIR(st.st_mode);\n    *kelp_is_file = S_ISREG(st.st_mode);\n",
    ),
    (
        "std/fs",
        "is_symlink_raw",
        "    struct stat st;\n    return lstat(kelp_path, &st) == 0 && S_ISLNK(st.st_mode);\n",
    ),
    (
        "std/fs",
        "read_link_raw",
        "    char buf[PATH_MAX];\n    ssize_t n = readlink(kelp_path, buf, sizeof buf);\n    if (n < 0) {\n        kelp_panic_errno(\"fs.read_link: cannot read link\");\n    }\n    if ((size_t)n >= sizeof buf) {\n        kelp_panic(\"fs.read_link: target is too long\");\n    }\n    buf[n] = 0;\n    return kelp_String__from_str(buf);\n",
    ),
    (
        "std/fs",
        "cwd_raw",
        "    char buf[PATH_MAX];\n    if (getcwd(buf, sizeof buf) == NULL) {\n        kelp_panic_errno(\"fs.cwd: cannot read the current directory\");\n    }\n    return kelp_String__from_str(buf);\n",
    ),
    (
        "std/fs",
        "try_read_raw",
        "    *kelp_err = 0;\n    FILE* f = fopen(kelp_path, \"rb\");\n    if (f == NULL) {\n        *kelp_err = errno;\n        return kelp_bytes_alloc_raw(0);\n    }\n    if (fseek(f, 0, SEEK_END) != 0) {\n        int e = errno;\n        fclose(f);\n        *kelp_err = e;\n        return kelp_bytes_alloc_raw(0);\n    }\n    long n = ftell(f);\n    if (n < 0) {\n        int e = errno;\n        fclose(f);\n        *kelp_err = e;\n        return kelp_bytes_alloc_raw(0);\n    }\n    rewind(f);\n    kelp_Bytes b = kelp_bytes_alloc_raw((size_t)n);\n    if (n > 0 && fread(b.data, 1, (size_t)n, f) != (size_t)n) {\n        int e = errno;\n        fclose(f);\n        kelp_bytes_drop(&b);\n        *kelp_err = e != 0 ? e : EIO;\n        return kelp_bytes_alloc_raw(0);\n    }\n    fclose(f);\n    return b;\n",
    ),
    (
        "std/fs",
        "try_open_raw",
        "    int flags;\n    if (strcmp(kelp_mode, \"r\") == 0) {\n        flags = O_RDONLY;\n    } else if (strcmp(kelp_mode, \"w\") == 0) {\n        flags = O_WRONLY | O_CREAT | O_TRUNC;\n    } else if (strcmp(kelp_mode, \"a\") == 0) {\n        flags = O_WRONLY | O_CREAT | O_APPEND;\n    } else if (strcmp(kelp_mode, \"rw\") == 0) {\n        flags = O_RDWR | O_CREAT;\n    } else {\n        *kelp_err = EINVAL;\n        return -1;\n    }\n    int fd = open(kelp_path, flags, 0666);\n    if (fd < 0) {\n        *kelp_err = errno;\n        return -1;\n    }\n    *kelp_err = 0;\n    return fd;\n",
    ),
    (
        "std/fs",
        "utf8_valid_raw",
        "    return kelp_utf8_valid(kelp_data->data, kelp_data->len);\n",
    ),
    (
        "std/time",
        "now_unix_raw",
        "    struct timespec ts;\n    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {\n        kelp_panic_errno(\"time.now_unix: clock_gettime failed\");\n    }\n    return (int64_t)ts.tv_sec;\n",
    ),
    (
        "std/time",
        "now_mono_ns_raw",
        "    struct timespec ts;\n    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {\n        kelp_panic_errno(\"time.now_mono_ns: clock_gettime failed\");\n    }\n    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;\n",
    ),
    (
        "std/time",
        "sleep_ms_raw",
        "    struct timespec ts;\n    ts.tv_sec = (time_t)(kelp_ms / 1000);\n    ts.tv_nsec = (long)((kelp_ms % 1000) * 1000000);\n    while (nanosleep(&ts, &ts) != 0) {\n        /* interrupted: finish the remainder */\n    }\n",
    ),
    (
        "std/thread",
        "sleep_ms_raw",
        "    struct timespec ts;\n    ts.tv_sec = (time_t)(kelp_ms / 1000);\n    ts.tv_nsec = (long)((kelp_ms % 1000) * 1000000);\n    while (nanosleep(&ts, &ts) != 0) {\n        /* interrupted: finish the remainder */\n    }\n",
    ),
    (
        "std/thread",
        "current_id_raw",
        "    return (uint64_t)pthread_self();\n",
    ),
    (
        "std/rand",
        "u64_raw",
        "    uint64_t v = 0;\n    FILE* f = fopen(\"/dev/urandom\", \"rb\");\n    if (f == NULL) {\n        kelp_panic_errno(\"rand.u64: no entropy source\");\n    }\n    if (fread(&v, 1, sizeof v, f) != sizeof v) {\n        fclose(f);\n        kelp_panic(\"rand.u64: short read from /dev/urandom\");\n    }\n    fclose(f);\n    return v;\n",
    ),
    (
        "std/rand",
        "fill_raw",
        "    FILE* f = fopen(\"/dev/urandom\", \"rb\");\n    if (f == NULL) {\n        kelp_panic_errno(\"rand.bytes: no entropy source\");\n    }\n    if (kelp_buf->len > 0 && fread(kelp_buf->data, 1, kelp_buf->len, f) != kelp_buf->len) {\n        fclose(f);\n        kelp_panic(\"rand.bytes: short read from /dev/urandom\");\n    }\n    fclose(f);\n",
    ),
    (
        "std/path",
        "normalize_raw",
        // The normalization itself is Kelp (`std/rt`), so this body is the
        // buffer it writes into and the owned `String` it hands back. The
        // `kelp_Bytes` here is a borrowed *view* of a stack array — `len` is
        // the capacity, which is the normalizer's contract — and it is
        // deliberately never dropped: it is not an allocation, and dropping it
        // would `free` a stack address.
        "    char buf[PATH_MAX + 1];\n    kelp_Bytes view;\n    view.data = (unsigned char*)buf;\n    view.len = PATH_MAX;\n    view.cap = PATH_MAX;\n    if (!kelp_std__rt__path_normalize(kelp_s, &view)) {\n        kelp_panic(\"path.normalize: path must be absolute and must not escape its own root\");\n    }\n    kelp_Bytes b = kelp_bytes_alloc_raw(strlen(buf));\n    memcpy(b.data, buf, b.len);\n    return kelp_String__from_bytes(b);\n",
    ),
    (
        "std/env",
        "has_raw",
        // `kelp_env_lookup` (`kelp_rt.c`) is a read-through cache over
        // `environ`, kept correct by `set_raw`/`unset_raw` below rebuilding
        // it on every write: see the comment on `kelp_EnvTable` for why that
        // is airtight (those two are the only way a Kelp program can ever
        // change its own environment).
        "    return kelp_env_lookup(kelp_name) != NULL;\n",
    ),
    (
        "std/env",
        "get_raw",
        "    const char* v = kelp_env_lookup(kelp_name);\n    if (v == NULL) {\n        return kelp_String__empty();\n    }\n    return kelp_String__from_str(v);\n",
    ),
    (
        "std/env",
        "set_raw",
        // `kelp_env_set` (`kelp_rt.c`) makes the real `setenv` call and
        // rebuilds the cache from the now-current `environ` as one critical
        // section, so the rebuild's scan can never race another thread's own
        // `setenv`/`unsetenv`. The panic stays outside that section.
        "    if (kelp_env_set(kelp_name, kelp_value) != 0) {\n        kelp_panic_errno(\"env.set: cannot set variable\");\n    }\n",
    ),
    (
        "std/env",
        "unset_raw",
        "    if (kelp_env_unset(kelp_name) != 0) {\n        kelp_panic_errno(\"env.unset: cannot unset variable\");\n    }\n",
    ),
    // ---- std/io (S2) -------------------------------------------------------
    (
        "std/io",
        "is_open_raw",
        "    /* Not a cached flag: the kernel is asked. A `File` is only a\n       descriptor, so this is the honest question, and it costs one `fcntl`. */\n    return fcntl(kelp_fd, F_GETFD) >= 0;\n",
    ),
    (
        "std/io",
        "read_raw",
        "    kelp_Bytes b = kelp_bytes_alloc_raw(kelp_max);\n    ssize_t n = read(kelp_fd, b.data, kelp_max);\n    if (n < 0) {\n        kelp_panic_errno(\"io.read: read failed\");\n    }\n    b.len = (size_t)n;\n    b.data[b.len] = 0;\n    return b;\n",
    ),
    (
        "std/io",
        "read_into_raw",
        "    /* `buf`'s length is the capacity of this read: the caller sized it.\n       Filling it does not change that length, so `at + n` may never pass it\n       and the NUL terminator slot is always inside the allocation. */\n    size_t room = kelp_at < kelp_buf->len ? kelp_buf->len - kelp_at : 0;\n    if (room == 0) {\n        return 0;\n    }\n    ssize_t n = read(kelp_fd, kelp_buf->data + kelp_at, room);\n    if (n < 0) {\n        kelp_panic_errno(\"io.read_into: read failed\");\n    }\n    kelp_buf->data[kelp_at + (size_t)n] = 0;\n    return (size_t)n;\n",
    ),
    (
        "std/io",
        "write_bytes_raw",
        "    /* A pipe may take fewer bytes than asked for, so a write is a loop,\n       not one call. EINTR is a retry, not a failure. */\n    kelp_io_flush_stdio(kelp_fd);\n    size_t off = 0;\n    while (off < kelp_data->len) {\n        ssize_t n = write(kelp_fd, kelp_data->data + off, kelp_data->len - off);\n        if (n < 0) {\n            if (errno == EINTR) {\n                continue;\n            }\n            kelp_panic_errno(\"io.write: write failed\");\n        }\n        off += (size_t)n;\n    }\n    return off;\n",
    ),
    (
        "std/io",
        "write_text_raw",
        "    kelp_io_flush_stdio(kelp_fd);\n    size_t off = 0;\n    while (off < kelp_text->len) {\n        ssize_t n = write(kelp_fd, kelp_text->data + off, kelp_text->len - off);\n        if (n < 0) {\n            if (errno == EINTR) {\n                continue;\n            }\n            kelp_panic_errno(\"io.write_text: write failed\");\n        }\n        off += (size_t)n;\n    }\n    return off;\n",
    ),
    (
        "std/io",
        "write_str_raw",
        "    kelp_io_flush_stdio(kelp_fd);\n    size_t len = strlen(kelp_s);\n    size_t off = 0;\n    while (off < len) {\n        ssize_t n = write(kelp_fd, kelp_s + off, len - off);\n        if (n < 0) {\n            if (errno == EINTR) {\n                continue;\n            }\n            kelp_panic_errno(\"io.write_str: write failed\");\n        }\n        off += (size_t)n;\n    }\n    return off;\n",
    ),
    (
        "std/io",
        "close_raw",
        "    if (close(kelp_fd) != 0) {\n        kelp_panic_errno(\"io.close: close failed\");\n    }\n",
    ),
    (
        "std/io",
        "set_nonblocking_raw",
        "    int flags = fcntl(kelp_fd, F_GETFL, 0);\n    if (flags < 0) {\n        kelp_panic_errno(\"io.set_nonblocking: fcntl failed\");\n    }\n    if (kelp_on) {\n        flags |= O_NONBLOCK;\n    } else {\n        flags &= ~O_NONBLOCK;\n    }\n    if (fcntl(kelp_fd, F_SETFL, flags) != 0) {\n        kelp_panic_errno(\"io.set_nonblocking: fcntl failed\");\n    }\n",
    ),
    (
        "std/io",
        "wait_readable_raw",
        "    struct pollfd p;\n    p.fd = kelp_fd;\n    p.events = POLLIN;\n    p.revents = 0;\n    /* > INT_MAX ms is 24 days: the same answer as forever, and `poll`'s\n       only spelling of forever is a negative timeout. */\n    int t = kelp_timeout_ms > 2147483647ULL ? -1 : (int)kelp_timeout_ms;\n    int r;\n    do {\n        r = poll(&p, 1, t);\n    } while (r < 0 && errno == EINTR);\n    if (r < 0) {\n        kelp_panic_errno(\"io.wait_readable: poll failed\");\n    }\n    /* HUP/ERR mean \"read now\" too: the read that follows returns 0. */\n    return r > 0 && (p.revents & (POLLIN | POLLHUP | POLLERR)) != 0;\n",
    ),
    (
        "std/io",
        "wait_writable_raw",
        "    struct pollfd p;\n    p.fd = kelp_fd;\n    p.events = POLLOUT;\n    p.revents = 0;\n    int t = kelp_timeout_ms > 2147483647ULL ? -1 : (int)kelp_timeout_ms;\n    int r;\n    do {\n        r = poll(&p, 1, t);\n    } while (r < 0 && errno == EINTR);\n    if (r < 0) {\n        kelp_panic_errno(\"io.wait_writable: poll failed\");\n    }\n    return r > 0 && (p.revents & (POLLOUT | POLLHUP | POLLERR)) != 0;\n",
    ),
    (
        "std/io",
        "errno_text_raw",
        "    return kelp_String__from_str(strerror(kelp_errno));\n",
    ),
    // ---- std/process (S2) --------------------------------------------------
    (
        "std/process",
        "arg_count_raw",
        "    return (size_t)kelp_argc;\n",
    ),
    (
        "std/process",
        "arg_at_raw",
        "    if (kelp_i >= (size_t)kelp_argc) {\n        kelp_panic(\"process.arg_at: index out of bounds\");\n    }\n    return kelp_String__from_str(kelp_argv[kelp_i]);\n",
    ),
    (
        "std/process",
        "exists_raw",
        "    struct stat st;\n    return stat(kelp_path, &st) == 0;\n",
    ),
    (
        "std/process",
        "executable_raw",
        "    struct stat st;\n    if (stat(kelp_path, &st) != 0) {\n        return false;\n    }\n    /* A directory carries an execute bit too, so the regular-file test is\n       what keeps `process.executable(\"/tmp\")` false. */\n    return S_ISREG(st.st_mode) && access(kelp_path, X_OK) == 0;\n",
    ),
    (
        "std/process",
        "is_dir_raw",
        "    struct stat st;\n    return stat(kelp_path, &st) == 0 && S_ISDIR(st.st_mode);\n",
    ),
    (
        "std/process",
        "spawn_raw",
        "    /* `kelp_List__String` is named literally here on purpose: an `extern`\n       body is C text, so it has to know the monomorphized container's name\n       and the `kelp_String` field layout (`data`/`len`). That is the cost of\n       marshalling an Kelp list into `execvp`'s `char*[]` without a generic\n       container type. */\n    int in_pipe[2];\n    int out_pipe[2];\n    int err_pipe[2];\n    if (pipe(in_pipe) != 0 || pipe(out_pipe) != 0 || pipe(err_pipe) != 0) {\n        kelp_panic_errno(\"process.spawn: cannot create a pipe\");\n    }\n    pid_t pid = fork();\n    if (pid < 0) {\n        kelp_panic_errno(\"process.spawn: fork failed\");\n    }\n    if (pid == 0) {\n        /* The child: wire the pipes to stdio, close every copy it does not\n           need, then exec. `_exit` (not `exit`) on the way out, so an\n           inherited stdio buffer is never flushed a second time. */\n        dup2(in_pipe[0], 0);\n        dup2(out_pipe[1], 1);\n        dup2(err_pipe[1], 2);\n        close(in_pipe[0]);\n        close(in_pipe[1]);\n        close(out_pipe[0]);\n        close(out_pipe[1]);\n        close(err_pipe[0]);\n        close(err_pipe[1]);\n        /* The per-call working directory (MK5-S2-M2): in the *child*, between `fork`\n           and `exec`, so the parent's own directory is unchanged and no grant\n           pattern's meaning moves. An empty `dir` is \"keep the parent's\",\n           which is what `spawn`/`run` pass. 126 is reserved for this failure. */\n        if (kelp_dir[0] != 0) {\n            if (chdir(kelp_dir) != 0) {\n                fprintf(stderr, \"process.spawn_in: cannot enter '%s'\\n\", kelp_dir);\n                _exit(126);\n            }\n        }\n        size_t n = kelp_args->len;\n        char** argv = (char**)malloc((n + 2) * sizeof(char*));\n        if (argv == NULL) {\n            fprintf(stderr, \"process.spawn: out of memory\\n\");\n            _exit(127);\n        }\n        argv[0] = (char*)kelp_program;\n        for (size_t i = 0; i < n; i++) {\n            argv[i + 1] = (char*)kelp_args->data[i].data;\n        }\n        argv[n + 1] = NULL;\n        execvp(kelp_program, argv);\n        /* `exec` did not replace this image, so the array it was handed is\n           still ours and this child is about to leave: free it before the\n           `_exit`, or valgrind reports it as definitely lost (the valgrind\n           memory gate found exactly this on `fixtures/process_spawn.kp`). */\n        free(argv);\n        fprintf(stderr, \"process.spawn: cannot exec '%s'\\n\", kelp_program);\n        _exit(127);\n    }\n    /* The parent keeps only its own ends: the child's stdin write end, and\n       the read ends of its output. */\n    close(in_pipe[0]);\n    close(out_pipe[1]);\n    close(err_pipe[1]);\n    *kelp_pid = (int32_t)pid;\n    *kelp_stdin_fd = in_pipe[1];\n    *kelp_stdout_fd = out_pipe[0];\n    *kelp_stderr_fd = err_pipe[0];\n",
    ),
    (
        "std/process",
        "reap_raw",
        "    /* The `-1` below is \"still running\", not a user-visible sentinel:\n       `Child.is_running` is the only caller that asks for it, and it turns\n       it into a `bool` before anything outside this module sees it. */\n    int status = 0;\n    pid_t r;\n    do {\n        r = waitpid(kelp_pid, &status, kelp_block ? 0 : WNOHANG);\n    } while (r < 0 && errno == EINTR);\n    if (r == 0) {\n        return -1;\n    }\n    if (r < 0) {\n        kelp_panic_errno(\"process.wait: waitpid failed\");\n    }\n    if (WIFEXITED(status)) {\n        return WEXITSTATUS(status);\n    }\n    if (WIFSIGNALED(status)) {\n        return 128 + WTERMSIG(status);\n    }\n    return 0;\n",
    ),
    (
        "std/process",
        "kill_raw",
        "    if (kill(kelp_pid, kelp_signal) != 0) {\n        kelp_panic_errno(\"process.kill: kill failed\");\n    }\n",
    ),
    ("std/process", "exit_raw", "    exit(kelp_code);\n"),
    ("std/process", "getpid_raw", "    return (int64_t)getpid();\n"),
    // ---- std/net (S3) ------------------------------------------------------
    //
    // The sockets themselves. Every one of these runs *after* the capability
    // check the compiler inserted at the caller's call site, so the check is
    // against the host string the code wrote and `getaddrinfo` here never
    // sees an unchecked name.
    (
        "std/net",
        "connect_raw",
        "    int gai = 0;\n    int last = 0;\n    int fd = kelp_rt_connect(kelp_host, (unsigned)kelp_port, &gai, &last);\n    if (fd < 0) {\n        if (gai != 0) {\n            kelp_panic_gai(\"net.connect: cannot resolve host\", gai);\n        }\n        kelp_panic_at(\"net.connect: connection failed\", last);\n    }\n    return fd;\n",
    ),
    // The fallible half of the same dial: the fd, or -1 with the reason the
    // panicking body above would have printed (its message line and the detail
    // line under it, without the `panic: ` lead-in) written into `kelp_reason`,
    // so a `net.try_connect` caller reads what `net.connect` prints. The
    // caller's own buffer is dropped inside the assignment: a Kelp `=` would run
    // the old value's drop, and a C store to `*kelp_reason` cannot.
    (
        "std/net",
        "connect_try_raw",
        "    int gai = 0;\n    int last = 0;\n    int fd = kelp_rt_connect(kelp_host, (unsigned)kelp_port, &gai, &last);\n    if (fd < 0) {\n        char text[512];\n        if (gai != 0) {\n            snprintf(text, sizeof text, \"net.connect: cannot resolve host\\n  getaddrinfo: %s\", gai_strerror(gai));\n        } else {\n            snprintf(text, sizeof text, \"net.connect: connection failed\\n  errno %d: %s\", last, strerror(last));\n        }\n        free(kelp_reason->data);\n        *kelp_reason = kelp_String__from_str(text);\n    }\n    return fd;\n",
    ),
    (
        "std/net",
        "listen_raw",
        "    /* IPv4 (`0.0.0.0`): a listener that answers every interface, with\n       one family, so `net.listen(0)` plus a `127.0.0.1` connect is\n       deterministic. An IPv6 listener is a later question. */\n    struct addrinfo hints;\n    memset(&hints, 0, sizeof hints);\n    hints.ai_family = AF_INET;\n    hints.ai_socktype = SOCK_STREAM;\n    hints.ai_flags = AI_PASSIVE;\n    char service[8];\n    snprintf(service, sizeof service, \"%u\", (unsigned)kelp_port);\n    struct addrinfo* list = NULL;\n    int rc = getaddrinfo(NULL, service, &hints, &list);\n    if (rc != 0) {\n        kelp_panic_gai(\"net.listen: cannot resolve the bind address\", rc);\n    }\n    int fd = -1;\n    int last = 0;\n    for (struct addrinfo* a = list; a != NULL; a = a->ai_next) {\n        fd = socket(a->ai_family, a->ai_socktype, a->ai_protocol);\n        if (fd < 0) {\n            last = errno;\n            continue;\n        }\n        int on = 1;\n        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);\n        if (bind(fd, a->ai_addr, a->ai_addrlen) == 0 && listen(fd, 128) == 0) {\n            break;\n        }\n        last = errno;\n        close(fd);\n        fd = -1;\n    }\n    freeaddrinfo(list);\n    if (fd < 0) {\n        kelp_panic_at(\"net.listen: cannot bind\", last);\n    }\n    return fd;\n",
    ),
    (
        "std/net",
        "accept_raw",
        "    int c;\n    do {\n        c = accept(kelp_fd, NULL, NULL);\n    } while (c < 0 && errno == EINTR);\n    if (c < 0) {\n        kelp_panic_errno(\"net.accept: accept failed\");\n    }\n    return c;\n",
    ),
    (
        "std/net",
        "close_raw",
        "    if (close(kelp_fd) != 0) {\n        kelp_panic_errno(\"net.close: close failed\");\n    }\n",
    ),
    (
        "std/net",
        "shutdown_write_raw",
        "    if (shutdown(kelp_fd, SHUT_WR) != 0) {\n        kelp_panic_errno(\"net.shutdown_write: shutdown failed\");\n    }\n",
    ),
    (
        "std/net",
        "send_raw",
        "    /* `MSG_NOSIGNAL`: a peer that has closed its end must not kill this\n       process with SIGPIPE; the failure comes back as a panic instead. */\n    ssize_t n;\n    do {\n        n = send(kelp_fd, kelp_data->data, kelp_data->len, MSG_NOSIGNAL);\n    } while (n < 0 && errno == EINTR);\n    if (n < 0) {\n        kelp_panic_errno(\"net.send: send failed (the peer may have closed)\");\n    }\n    return (size_t)n;\n",
    ),
    (
        "std/net",
        "send_all_raw",
        "    size_t off = 0;\n    while (off < kelp_data->len) {\n        ssize_t n = send(kelp_fd, kelp_data->data + off, kelp_data->len - off, MSG_NOSIGNAL);\n        if (n < 0) {\n            if (errno == EINTR) {\n                continue;\n            }\n            kelp_panic_errno(\"net.send: send failed (the peer may have closed)\");\n        }\n        off += (size_t)n;\n    }\n    return off;\n",
    ),
    (
        "std/net",
        "recv_raw",
        "    /* `recv(2)` with a length of 0 on a blocking socket waits for a peer\n       event instead of returning, so a request for 0 bytes short-circuits to\n       the empty `Bytes` that `recv_into` answers with 0. */\n    if (kelp_max == 0) {\n        return kelp_bytes_alloc_raw(0);\n    }\n    kelp_Bytes b = kelp_bytes_alloc_raw(kelp_max);\n    ssize_t n;\n    do {\n        n = recv(kelp_fd, b.data, kelp_max, 0);\n    } while (n < 0 && errno == EINTR);\n    if (n < 0) {\n        if (errno == EAGAIN || errno == EWOULDBLOCK) {\n            kelp_panic(\"net.recv: timed out\");\n        }\n        kelp_panic_errno(\"net.recv: receive failed\");\n    }\n    b.len = (size_t)n;\n    b.data[b.len] = 0;\n    return b;\n",
    ),
    (
        "std/net",
        "recv_into_raw",
        "    /* `buf`'s length is the capacity here, exactly as in `io.read_into`\n       (open item 51): the caller sized it, and filling it does not change\n       that length, so the NUL terminator slot is always inside the\n       allocation. */\n    size_t room = kelp_at < kelp_buf->len ? kelp_buf->len - kelp_at : 0;\n    if (room == 0) {\n        return 0;\n    }\n    ssize_t n;\n    do {\n        n = recv(kelp_fd, kelp_buf->data + kelp_at, room, 0);\n    } while (n < 0 && errno == EINTR);\n    if (n < 0) {\n        if (errno == EAGAIN || errno == EWOULDBLOCK) {\n            kelp_panic(\"net.recv_into: timed out\");\n        }\n        kelp_panic_errno(\"net.recv_into: receive failed\");\n    }\n    kelp_buf->data[kelp_at + (size_t)n] = 0;\n    return (size_t)n;\n",
    ),
    (
        "std/net",
        "send_all_try_raw",
        "    size_t off = 0;\n    while (off < kelp_data->len) {\n        ssize_t n = send(kelp_fd, kelp_data->data + off, kelp_data->len - off, MSG_NOSIGNAL);\n        if (n < 0) {\n            if (errno == EINTR) {\n                continue;\n            }\n            char text[256];\n            snprintf(text, sizeof text, \"net.send: send failed (the peer may have closed)\\n  errno %d: %s\", errno, strerror(errno));\n            free(kelp_reason->data);\n            *kelp_reason = kelp_String__from_str(text);\n            return -1;\n        }\n        off += (size_t)n;\n    }\n    return 0;\n",
    ),
    (
        "std/net",
        "recv_try_raw",
        "    if (kelp_max == 0) {\n        return kelp_bytes_alloc_raw(0);\n    }\n    kelp_Bytes b = kelp_bytes_alloc_raw(kelp_max);\n    ssize_t n;\n    do {\n        n = recv(kelp_fd, b.data, kelp_max, 0);\n    } while (n < 0 && errno == EINTR);\n    if (n < 0) {\n        char text[256];\n        if (errno == EAGAIN || errno == EWOULDBLOCK) {\n            snprintf(text, sizeof text, \"net.recv: timed out\");\n        } else {\n            snprintf(text, sizeof text, \"net.recv: receive failed\\n  errno %d: %s\", errno, strerror(errno));\n        }\n        free(kelp_reason->data);\n        *kelp_reason = kelp_String__from_str(text);\n        b.len = 0;\n        b.data[0] = 0;\n        return b;\n    }\n    b.len = (size_t)n;\n    b.data[b.len] = 0;\n    return b;\n",
    ),
    (
        "std/net",
        "recv_into_try_raw",
        "    size_t room = kelp_at < kelp_buf->len ? kelp_buf->len - kelp_at : 0;\n    if (room == 0) {\n        return 0;\n    }\n    ssize_t n;\n    do {\n        n = recv(kelp_fd, kelp_buf->data + kelp_at, room, 0);\n    } while (n < 0 && errno == EINTR);\n    if (n < 0) {\n        char text[256];\n        if (errno == EAGAIN || errno == EWOULDBLOCK) {\n            snprintf(text, sizeof text, \"net.recv_into: timed out\");\n        } else {\n            snprintf(text, sizeof text, \"net.recv_into: receive failed\\n  errno %d: %s\", errno, strerror(errno));\n        }\n        free(kelp_reason->data);\n        *kelp_reason = kelp_String__from_str(text);\n        return 0;\n    }\n    kelp_buf->data[kelp_at + (size_t)n] = 0;\n    return (size_t)n;\n",
    ),
    (
        "std/net",
        "set_timeout_raw",
        "    struct timeval tv;\n    tv.tv_sec = (time_t)(kelp_ms / 1000);\n    tv.tv_usec = (suseconds_t)((kelp_ms % 1000) * 1000);\n    if (setsockopt(kelp_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv) != 0) {\n        kelp_panic_errno(\"net.set_timeout: cannot set the receive timeout\");\n    }\n    if (setsockopt(kelp_fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv) != 0) {\n        kelp_panic_errno(\"net.set_timeout: cannot set the send timeout\");\n    }\n",
    ),
    (
        "std/net",
        "local_port_raw",
        "    struct sockaddr_storage ss;\n    socklen_t len = sizeof ss;\n    if (getsockname(kelp_fd, (struct sockaddr*)&ss, &len) != 0) {\n        kelp_panic_errno(\"net.port: getsockname failed\");\n    }\n    if (ss.ss_family == AF_INET) {\n        return (uint16_t)ntohs(((struct sockaddr_in*)&ss)->sin_port);\n    }\n    if (ss.ss_family == AF_INET6) {\n        return (uint16_t)ntohs(((struct sockaddr_in6*)&ss)->sin6_port);\n    }\n    kelp_panic(\"net.port: unknown address family\");\n    return 0;\n",
    ),
    (
        "std/net",
        "peer_addr_raw",
        "    struct sockaddr_storage ss;\n    socklen_t len = sizeof ss;\n    if (getpeername(kelp_fd, (struct sockaddr*)&ss, &len) != 0) {\n        kelp_panic_errno(\"net.peer_addr: getpeername failed\");\n    }\n    char host[INET6_ADDRSTRLEN];\n    char text[INET6_ADDRSTRLEN + 16];\n    if (ss.ss_family == AF_INET) {\n        inet_ntop(AF_INET, &((struct sockaddr_in*)&ss)->sin_addr, host, sizeof host);\n        snprintf(text, sizeof text, \"%s:%u\", host,\n                 (unsigned)ntohs(((struct sockaddr_in*)&ss)->sin_port));\n    } else if (ss.ss_family == AF_INET6) {\n        inet_ntop(AF_INET6, &((struct sockaddr_in6*)&ss)->sin6_addr, host, sizeof host);\n        snprintf(text, sizeof text, \"[%s]:%u\", host,\n                 (unsigned)ntohs(((struct sockaddr_in6*)&ss)->sin6_port));\n    } else {\n        kelp_panic(\"net.peer_addr: unknown address family\");\n    }\n    return kelp_String__from_str(text);\n",
    ),
    (
        "std/net",
        "resolve_raw",
        "    struct addrinfo hints;\n    memset(&hints, 0, sizeof hints);\n    hints.ai_family = AF_UNSPEC;\n    hints.ai_socktype = SOCK_STREAM;\n    struct addrinfo* list = NULL;\n    int rc = getaddrinfo(kelp_host, NULL, &hints, &list);\n    if (rc != 0) {\n        kelp_panic_gai(\"net.resolve: cannot resolve host\", rc);\n    }\n    /* One address per line, in the order `getaddrinfo` reported them, with\n       duplicates dropped — an address literal resolves to itself, so this\n       is also the honest way to see what `connect` would try first. */\n    char out[4096];\n    size_t at = 0;\n    out[0] = 0;\n    char seen[16][INET6_ADDRSTRLEN];\n    size_t seen_n = 0;\n    for (struct addrinfo* a = list; a != NULL; a = a->ai_next) {\n        void* src = NULL;\n        if (a->ai_family == AF_INET) {\n            src = &((struct sockaddr_in*)a->ai_addr)->sin_addr;\n        } else if (a->ai_family == AF_INET6) {\n            src = &((struct sockaddr_in6*)a->ai_addr)->sin6_addr;\n        } else {\n            continue;\n        }\n        char host[INET6_ADDRSTRLEN];\n        if (inet_ntop(a->ai_family, src, host, sizeof host) == NULL) {\n            continue;\n        }\n        bool dup = false;\n        for (size_t i = 0; i < seen_n; i++) {\n            if (strcmp(seen[i], host) == 0) {\n                dup = true;\n                break;\n            }\n        }\n        if (dup) {\n            continue;\n        }\n        if (seen_n < 16) {\n            snprintf(seen[seen_n], sizeof seen[seen_n], \"%s\", host);\n            seen_n++;\n        }\n        size_t hl = strlen(host);\n        if (at + hl + 1 >= sizeof out) {\n            break;\n        }\n        if (at > 0) {\n            out[at++] = '\\n';\n        }\n        memcpy(out + at, host, hl);\n        at += hl;\n        out[at] = 0;\n    }\n    freeaddrinfo(list);\n    return kelp_String__from_str(out);\n",
    ),
    // ---- std/tls (MK5-S4b-M1, open item 37) --------------------------------
    //
    // The only OpenSSL-touching code in the compiler. `std/tls` never dials a
    // socket itself (see the module's header comment for why: a capability
    // grant is resolved one hop from the importing program's `impl` line, and
    // a `net.connect` written here would be checked against this module's own
    // — always-empty — grant instead). Every body below operates on an `fd`
    // the caller already connected, or on the `SSL*` `handshake_raw` handed
    // back, carried as `usize` because v1 has no pointer type at the Kelp
    // level (the same reason a socket crosses as a plain `fd: i32`).
    (
        "std/tls",
        "handshake_raw",
        "    SSL_CTX* ctx = SSL_CTX_new(TLS_client_method());\n    if (ctx == NULL) {\n        kelp_panic(\"tls.handshake: cannot create an SSL context\");\n    }\n    SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);\n    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);\n    if (SSL_CTX_set_default_verify_paths(ctx) != 1) {\n        SSL_CTX_free(ctx);\n        kelp_panic(\"tls.handshake: cannot load the system trust store\");\n    }\n    SSL* ssl = SSL_new(ctx);\n    /* `ssl` keeps its own reference to `ctx`, so freeing this reference now\n       leaves exactly one object (`ssl`) for every failure path below to\n       free — the same one-thing-to-clean-up shape `net.connect_raw` has\n       with its `fd`. */\n    SSL_CTX_free(ctx);\n    if (ssl == NULL) {\n        kelp_panic(\"tls.handshake: cannot create an SSL handle\");\n    }\n    if (SSL_set_fd(ssl, kelp_fd) != 1) {\n        SSL_free(ssl);\n        kelp_panic(\"tls.handshake: cannot bind the socket to the TLS handle\");\n    }\n    /* SNI and the certificate's hostname check both use the name the caller\n       wrote — the same name `std/http.kp`'s capability check ran against,\n       not whatever the socket resolved to. */\n    SSL_set_tlsext_host_name(ssl, kelp_host);\n    if (SSL_set1_host(ssl, kelp_host) != 1) {\n        SSL_free(ssl);\n        kelp_panic(\"tls.handshake: cannot set the verification hostname\");\n    }\n    for (;;) {\n        int rc = SSL_connect(ssl);\n        if (rc == 1) {\n            break;\n        }\n        int err = SSL_get_error(ssl, rc);\n        if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) {\n            continue;\n        }\n        char text[256];\n        ERR_error_string_n(ERR_get_error(), text, sizeof text);\n        SSL_free(ssl);\n        kelp_panic_tls(\"tls.handshake: the TLS handshake failed\", text);\n    }\n    long verify = SSL_get_verify_result(ssl);\n    if (verify != X509_V_OK) {\n        const char* reason = X509_verify_cert_error_string(verify);\n        SSL_free(ssl);\n        kelp_panic_tls(\"tls.handshake: certificate verification failed\", reason);\n    }\n    return (size_t)(uintptr_t)ssl;\n",
    ),
    (
        "std/tls",
        "write_raw",
        "    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    int n = SSL_write(ssl, kelp_data->data, (int)kelp_data->len);\n    if (n <= 0) {\n        int err = SSL_get_error(ssl, n);\n        if (err == SSL_ERROR_ZERO_RETURN) {\n            kelp_panic(\"tls.write: the peer closed the connection\");\n        }\n        char text[256];\n        ERR_error_string_n(ERR_get_error(), text, sizeof text);\n        kelp_panic_tls(\"tls.write: write failed\", text);\n    }\n    return (size_t)n;\n",
    ),
    (
        "std/tls",
        "write_all_raw",
        "    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    size_t off = 0;\n    while (off < kelp_data->len) {\n        int n = SSL_write(ssl, kelp_data->data + off, (int)(kelp_data->len - off));\n        if (n <= 0) {\n            int err = SSL_get_error(ssl, n);\n            if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) {\n                continue;\n            }\n            if (err == SSL_ERROR_ZERO_RETURN) {\n                kelp_panic(\"tls.write: the peer closed the connection\");\n            }\n            char text[256];\n            ERR_error_string_n(ERR_get_error(), text, sizeof text);\n            kelp_panic_tls(\"tls.write: write failed\", text);\n        }\n        off += (size_t)n;\n    }\n    return off;\n",
    ),
    (
        "std/tls",
        "read_raw",
        "    /* Mirrors `net.recv_raw`'s zero-length short-circuit: a `max` of 0\n       needs no `SSL_read` call, whose own return of 0 is ambiguous with a\n       clean close. */\n    if (kelp_max == 0) {\n        return kelp_bytes_alloc_raw(0);\n    }\n    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    kelp_Bytes b = kelp_bytes_alloc_raw(kelp_max);\n    int n = SSL_read(ssl, b.data, (int)kelp_max);\n    if (n <= 0) {\n        int err = SSL_get_error(ssl, n);\n        /* End of stream: a clean close_notify, or a peer that dropped the TCP connection without one (OpenSSL 3 reports that as SSL_R_UNEXPECTED_EOF_WHILE_READING, older builds as a SYSCALL error with errno 0 or a reset). */\n        if (err == SSL_ERROR_ZERO_RETURN || (err == SSL_ERROR_SYSCALL && (errno == 0 || errno == ECONNRESET || errno == EPIPE)) || (err == SSL_ERROR_SSL && ERR_GET_REASON(ERR_peek_error()) == SSL_R_UNEXPECTED_EOF_WHILE_READING)) {\n            ERR_clear_error();\n            b.len = 0;\n            b.data[0] = 0;\n            return b;\n        }\n        char text[256];\n        ERR_error_string_n(ERR_get_error(), text, sizeof text);\n        kelp_panic_tls(\"tls.read: read failed\", text);\n    }\n    b.len = (size_t)n;\n    b.data[b.len] = 0;\n    return b;\n",
    ),
    (
        "std/tls",
        "read_into_raw",
        "    size_t room = kelp_at < kelp_buf->len ? kelp_buf->len - kelp_at : 0;\n    if (room == 0) {\n        return 0;\n    }\n    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    int n = SSL_read(ssl, kelp_buf->data + kelp_at, (int)room);\n    if (n <= 0) {\n        int err = SSL_get_error(ssl, n);\n        /* End of stream: a clean close_notify, or a peer that dropped the TCP connection without one (OpenSSL 3 reports that as SSL_R_UNEXPECTED_EOF_WHILE_READING, older builds as a SYSCALL error with errno 0 or a reset). */\n        if (err == SSL_ERROR_ZERO_RETURN || (err == SSL_ERROR_SYSCALL && (errno == 0 || errno == ECONNRESET || errno == EPIPE)) || (err == SSL_ERROR_SSL && ERR_GET_REASON(ERR_peek_error()) == SSL_R_UNEXPECTED_EOF_WHILE_READING)) {\n            ERR_clear_error();\n            return 0;\n        }\n        char text[256];\n        ERR_error_string_n(ERR_get_error(), text, sizeof text);\n        kelp_panic_tls(\"tls.read_into: read failed\", text);\n    }\n    kelp_buf->data[kelp_at + (size_t)n] = 0;\n    return (size_t)n;\n",
    ),
    (
        "std/tls",
        "handshake_try_raw",
        "    char text[320];\n    SSL_CTX* ctx = SSL_CTX_new(TLS_client_method());\n    SSL* ssl = NULL;\n    if (ctx == NULL) {\n        snprintf(text, sizeof text, \"tls.handshake: cannot create an SSL context\");\n        goto fail;\n    }\n    SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);\n    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);\n    if (SSL_CTX_set_default_verify_paths(ctx) != 1) {\n        snprintf(text, sizeof text, \"tls.handshake: cannot load the system trust store\");\n        goto fail;\n    }\n    ssl = SSL_new(ctx);\n    SSL_CTX_free(ctx);\n    ctx = NULL;\n    if (ssl == NULL) {\n        snprintf(text, sizeof text, \"tls.handshake: cannot create an SSL handle\");\n        goto fail;\n    }\n    if (SSL_set_fd(ssl, kelp_fd) != 1) {\n        snprintf(text, sizeof text, \"tls.handshake: cannot bind the socket to the TLS handle\");\n        goto fail;\n    }\n    SSL_set_tlsext_host_name(ssl, kelp_host);\n    if (SSL_set1_host(ssl, kelp_host) != 1) {\n        snprintf(text, sizeof text, \"tls.handshake: cannot set the verification hostname\");\n        goto fail;\n    }\n    for (;;) {\n        int rc = SSL_connect(ssl);\n        if (rc == 1) {\n            break;\n        }\n        int err = SSL_get_error(ssl, rc);\n        if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) {\n            continue;\n        }\n        char detail[256];\n        ERR_error_string_n(ERR_get_error(), detail, sizeof detail);\n        snprintf(text, sizeof text, \"tls.handshake: the TLS handshake failed\\n  %s\", detail);\n        goto fail;\n    }\n    long verify = SSL_get_verify_result(ssl);\n    if (verify != X509_V_OK) {\n        snprintf(text, sizeof text, \"tls.handshake: certificate verification failed\\n  %s\", X509_verify_cert_error_string(verify));\n        goto fail;\n    }\n    return (size_t)(uintptr_t)ssl;\nfail:\n    if (ctx != NULL) {\n        SSL_CTX_free(ctx);\n    }\n    if (ssl != NULL) {\n        SSL_free(ssl);\n    }\n    free(kelp_reason->data);\n    *kelp_reason = kelp_String__from_str(text);\n    return 0;\n",
    ),
    (
        "std/tls",
        "write_all_try_raw",
        "    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    size_t off = 0;\n    while (off < kelp_data->len) {\n        int n = SSL_write(ssl, kelp_data->data + off, (int)(kelp_data->len - off));\n        if (n <= 0) {\n            int err = SSL_get_error(ssl, n);\n            if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) {\n                continue;\n            }\n            char text[320];\n            if (err == SSL_ERROR_ZERO_RETURN) {\n                snprintf(text, sizeof text, \"tls.write: the peer closed the connection\");\n            } else {\n                char detail[256];\n                ERR_error_string_n(ERR_get_error(), detail, sizeof detail);\n                snprintf(text, sizeof text, \"tls.write: write failed\\n  %s\", detail);\n            }\n            free(kelp_reason->data);\n            *kelp_reason = kelp_String__from_str(text);\n            return -1;\n        }\n        off += (size_t)n;\n    }\n    return 0;\n",
    ),
    (
        "std/tls",
        "read_try_raw",
        "    if (kelp_max == 0) {\n        return kelp_bytes_alloc_raw(0);\n    }\n    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    kelp_Bytes b = kelp_bytes_alloc_raw(kelp_max);\n    int n = SSL_read(ssl, b.data, (int)kelp_max);\n    if (n <= 0) {\n        int err = SSL_get_error(ssl, n);\n        b.len = 0;\n        b.data[0] = 0;\n        if (err == SSL_ERROR_ZERO_RETURN || (err == SSL_ERROR_SYSCALL && (errno == 0 || errno == ECONNRESET || errno == EPIPE)) || (err == SSL_ERROR_SSL && ERR_GET_REASON(ERR_peek_error()) == SSL_R_UNEXPECTED_EOF_WHILE_READING)) {\n            ERR_clear_error();\n            return b;\n        }\n        char text[320];\n        char detail[256];\n        ERR_error_string_n(ERR_get_error(), detail, sizeof detail);\n        snprintf(text, sizeof text, \"tls.read: read failed\\n  %s\", detail);\n        free(kelp_reason->data);\n        *kelp_reason = kelp_String__from_str(text);\n        return b;\n    }\n    b.len = (size_t)n;\n    b.data[b.len] = 0;\n    return b;\n",
    ),
    (
        "std/tls",
        "read_into_try_raw",
        "    size_t room = kelp_at < kelp_buf->len ? kelp_buf->len - kelp_at : 0;\n    if (room == 0) {\n        return 0;\n    }\n    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    int n = SSL_read(ssl, kelp_buf->data + kelp_at, (int)room);\n    if (n <= 0) {\n        int err = SSL_get_error(ssl, n);\n        if (err == SSL_ERROR_ZERO_RETURN || (err == SSL_ERROR_SYSCALL && (errno == 0 || errno == ECONNRESET || errno == EPIPE)) || (err == SSL_ERROR_SSL && ERR_GET_REASON(ERR_peek_error()) == SSL_R_UNEXPECTED_EOF_WHILE_READING)) {\n            ERR_clear_error();\n            return 0;\n        }\n        char text[320];\n        char detail[256];\n        ERR_error_string_n(ERR_get_error(), detail, sizeof detail);\n        snprintf(text, sizeof text, \"tls.read_into: read failed\\n  %s\", detail);\n        free(kelp_reason->data);\n        *kelp_reason = kelp_String__from_str(text);\n        return 0;\n    }\n    kelp_buf->data[kelp_at + (size_t)n] = 0;\n    return (size_t)n;\n",
    ),
    (
        "std/tls",
        "shutdown_raw",
        "    /* Not fatal on failure, unlike `net.close_raw`: a peer that has already\n       gone away without waiting for `close_notify` is the common case here,\n       not an error worth ending the process over — the socket close that\n       follows in `tls.close` still happens either way. */\n    SSL* ssl = (SSL*)(uintptr_t)kelp_handle;\n    SSL_shutdown(ssl);\n    SSL_free(ssl);\n",
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

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    /// `net.recv(s, 0)` returns an empty `Bytes` at once, the same answer
    /// `recv_into` gives for a zero length. Linux `recv(2)` with a length of 0
    /// on a blocking socket waits for a peer event instead, so a body without
    /// the short-circuit hangs until the peer sends or closes. The body is C,
    /// so the test compiles it against stubs and runs it on a socket pair whose
    /// reading end carries a short receive timeout: a waited call spends that
    /// timeout and then takes the body's own timed-out panic, while the
    /// short-circuited call returns in microseconds. The measured call time,
    /// not the wall clock of the whole process, is what separates returned from
    /// gave up, so a loaded machine cannot flake it.
    #[test]
    fn recv_raw_with_zero_length_returns_an_empty_bytes() {
        let body = super::body("std/net", "recv_raw").expect("a shipped recv_raw body");

        // The struct is the layout `runtime/kelp_rt.c` defines and
        // `codegen_llvm::program_c_source` repeats in the extern prelude; the
        // three functions are the stubs the body calls (`kelp_bytes_alloc_raw`
        // copied from the runtime).
        let source = format!(
            "{headers}\
             #include <sys/time.h>\n\
             typedef struct {{ unsigned char* data; size_t len; size_t cap; }} kelp_Bytes;\n\
             static void kelp_panic(const char* msg) {{\n    fprintf(stderr, \"%s\\n\", msg);\n    exit(2);\n}}\n\
             static void kelp_panic_errno(const char* msg) {{\n    fprintf(stderr, \"%s: %s\\n\", msg, strerror(errno));\n    exit(2);\n}}\n\
             static kelp_Bytes kelp_bytes_alloc_raw(size_t n) {{\n    unsigned char* p = (unsigned char*)malloc(n + 1);\n    if (p == NULL) {{\n        kelp_panic(\"out of memory\");\n    }}\n    memset(p, 0, n + 1);\n    kelp_Bytes b;\n    b.data = p;\n    b.len = n;\n    b.cap = n;\n    return b;\n}}\n\
             static kelp_Bytes recv_raw(int kelp_fd, size_t kelp_max) {{\n{body}}}\n\
             int main(void) {{\n    int sv[2];\n    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {{\n        perror(\"socketpair\");\n        return 1;\n    }}\n    struct timeval tv;\n    tv.tv_sec = 0;\n    tv.tv_usec = 250000;\n    setsockopt(sv[0], SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);\n    struct timespec t0, t1;\n    clock_gettime(CLOCK_MONOTONIC, &t0);\n    kelp_Bytes b = recv_raw(sv[0], 0);\n    clock_gettime(CLOCK_MONOTONIC, &t1);\n    long us = (t1.tv_sec - t0.tv_sec) * 1000000L + (t1.tv_nsec - t0.tv_nsec) / 1000L;\n    printf(\"%zu %ld\\n\", b.len, us);\n    return 0;\n}}\n",
            headers = crate::codegen_c::C_HEADERS,
        );

        let dir = std::env::temp_dir().join(format!("kelp_recv_zero_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory for the harness");
        let bin = dir.join("recv_zero");
        crate::driver::compile_c(&source, &bin, crate::opt::OptLevel::DEFAULT, &[], false)
            .unwrap_or_else(|e| panic!("the recv_raw harness did not compile: {}", e.message));

        let out = Command::new(&bin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to run the recv_raw harness");
        assert!(
            out.status.success(),
            "the recv_raw harness exited with {:?}; a zero length receive blocked \
             rather than returning:\nstdout: {}\nstderr: {}\n",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split_whitespace();
        let len: usize = parts
            .next()
            .expect("a length on stdout")
            .parse()
            .expect("the length is a number");
        let us: i64 = parts
            .next()
            .expect("an elapsed time on stdout")
            .parse()
            .expect("the elapsed time is a number");
        assert_eq!(
            len, 0,
            "recv_raw(fd, 0) returned {len} bytes, not the empty Bytes recv_into reports"
        );
        // The call is microseconds when it returns. The 250 ms receive timeout
        // is what the blocking behaviour would have spent and then reported as
        // `net.recv: timed out`; 50 ms leaves room for a loaded machine and
        // still names the timeout as the thing that did not happen.
        assert!(
            us < 50_000,
            "recv_raw(fd, 0) waited {us}us, so it blocked instead of returning"
        );
    }
}

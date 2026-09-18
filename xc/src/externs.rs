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

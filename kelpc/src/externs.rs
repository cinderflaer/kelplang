//! Bodies for the `extern fun` declarations of the compiler's stdlib (M6.2).
//!
//! Stdlib decision 3 in the plan: the stdlib ships as `.kp` sources, and the
//! few operations that cannot be written in Kelp at all - a raw syscall, a
//! `malloc`, a byte read out of a `&str` - are declared `extern` and have
//! their body in the Rust runtime (`kelp-rt`). That keeps the *policy* (the
//! public function, its checks, its documentation) in reviewable Kelp source
//! while confining the unportable primitive to one place.
//!
//! Keyed by `(module id, bare function name)`, e.g. `("std/mem",
//! "alloc_raw")`. A module id is the `impl` path (`std/mem`), not the dotted
//! symbol prefix. An `extern` with no entry in [`RUST_PORTED`] is rejected in
//! `infer.rs`, not silently left undefined - which is also why user code cannot
//! declare one: only compiler-shipped modules have bodies.

/// The externs whose body is the Rust runtime's export of the same name
/// (`kelp-rt/src/stdlib/*.rs`, `kelp-rt/src/tls.rs`), so no C text is written
/// for them. The LLVM call resolves against `libkelp_rt.a` as `kelp_` plus the
/// mangled qualified name.
static RUST_PORTED: &[(&str, &str)] = &[
    ("std/tls", "handshake_try_raw"),
    ("std/tls", "server_ctx_try_raw"),
    ("std/net", "listen_on_raw"),
    ("std/tls", "reload_try_raw"),
    ("std/tls", "accept_try_raw"),
    ("std/tls", "ctx_free_raw"),
    ("std/tls", "alpn_raw"),
    ("std/tls", "peer_subject_raw"),
    ("std/tls", "pending_raw"),
    ("std/math", "sqrt_raw"),
    ("std/math", "pow_raw"),
    ("std/math", "floor_raw"),
    ("std/math", "ceil_raw"),
    ("std/math", "round_raw"),
    ("std/math", "trunc_raw"),
    ("std/math", "exp_raw"),
    ("std/math", "ln_raw"),
    ("std/math", "log2_raw"),
    ("std/math", "log10_raw"),
    ("std/math", "sin_raw"),
    ("std/math", "cos_raw"),
    ("std/math", "tan_raw"),
    ("std/math", "atan2_raw"),
    ("std/math", "is_nan_raw"),
    ("std/math", "is_finite_raw"),
    ("std/sys", "os"),
    ("std/sys", "arch"),
    ("std/source", "file"),
    ("std/source", "line"),
    ("std/mem", "alloc_raw"),
    ("std/sync", "chan_id_raw"),
    ("std/sync", "chan_wake_raw"),
    ("std/sync", "chan_now_ms_raw"),
    ("std/sync", "waiter_new_raw"),
    ("std/sync", "waiter_free_raw"),
    ("std/sync", "waiter_watch_raw"),
    ("std/sync", "waiter_wait_raw"),
    ("std/time", "now_unix_raw"),
    ("std/time", "now_mono_ns_raw"),
    ("std/time", "sleep_ms_raw"),
    ("std/thread", "sleep_ms_raw"),
    ("std/thread", "current_id_raw"),
    ("std/env", "has_raw"),
    ("std/env", "get_raw"),
    ("std/env", "set_raw"),
    ("std/env", "unset_raw"),
    ("std/rand", "u64_raw"),
    ("std/rand", "fill_raw"),
    ("std/path", "normalize_raw"),
    ("std/fmt", "hex_raw"),
    ("std/fmt", "bytes_raw"),
    ("std/str", "byte_len_raw"),
    ("std/str", "byte_at_raw"),
    ("std/str", "slice_from_raw"),
    ("std/str", "byte_ptr_raw"),
    ("std/str", "from_f64_raw"),
    ("std/str", "parse_i64_raw"),
    ("std/str", "parse_u64_raw"),
    ("std/str", "parse_f64_raw"),
    ("std/fs", "read_raw"),
    ("std/fs", "write_raw"),
    ("std/fs", "write_text_raw"),
    ("std/fs", "exists_raw"),
    ("std/fs", "is_file_raw"),
    ("std/fs", "is_dir_raw"),
    ("std/fs", "size_raw"),
    ("std/fs", "remove_raw"),
    ("std/fs", "rename_raw"),
    ("std/fs", "mkdir_raw"),
    ("std/fs", "mkdir_all_raw"),
    ("std/fs", "append_raw"),
    ("std/fs", "list_raw"),
    ("std/fs", "sync_raw"),
    ("std/fs", "open_raw"),
    ("std/fs", "symlink_raw"),
    ("std/fs", "copy_raw"),
    ("std/fs", "touch_raw"),
    ("std/fs", "set_mode_raw"),
    ("std/fs", "temp_path_raw"),
    ("std/fs", "mtime_raw"),
    ("std/fs", "mode_raw"),
    ("std/fs", "metadata_raw"),
    ("std/fs", "is_symlink_raw"),
    ("std/fs", "read_link_raw"),
    ("std/fs", "cwd_raw"),
    ("std/fs", "try_read_raw"),
    ("std/fs", "try_open_raw"),
    ("std/fs", "try_write_raw"),
    ("std/fs", "try_write_text_raw"),
    ("std/fs", "try_append_raw"),
    ("std/fs", "try_remove_raw"),
    ("std/fs", "try_rename_raw"),
    ("std/fs", "try_symlink_raw"),
    ("std/fs", "try_mkdir_all_raw"),
    ("std/fs", "try_list_raw"),
    ("std/fs", "utf8_valid_raw"),
    ("std/io", "is_open_raw"),
    ("std/io", "read_raw"),
    ("std/io", "read_into_raw"),
    ("std/io", "write_bytes_raw"),
    ("std/io", "write_text_raw"),
    ("std/io", "write_str_raw"),
    ("std/io", "close_raw"),
    ("std/io", "set_nonblocking_raw"),
    ("std/io", "wait_readable_raw"),
    ("std/io", "wait_writable_raw"),
    ("std/io", "errno_text_raw"),
    ("std/process", "arg_count_raw"),
    ("std/process", "arg_at_raw"),
    ("std/process", "exists_raw"),
    ("std/process", "executable_raw"),
    ("std/process", "is_dir_raw"),
    ("std/process", "spawn_raw"),
    ("std/process", "spawn_inherit_raw"),
    ("std/process", "reap_raw"),
    ("std/process", "kill_raw"),
    ("std/process", "exit_raw"),
    ("std/process", "getpid_raw"),
    ("std/net", "connect_raw"),
    ("std/net", "connect_try_raw"),
    ("std/net", "listen_raw"),
    ("std/net", "accept_raw"),
    ("std/net", "accept_try_raw"),
    ("std/net", "mono_ms_raw"),
    ("std/net", "close_raw"),
    ("std/net", "shutdown_write_raw"),
    ("std/net", "shutdown_write_try_raw"),
    ("std/net", "send_raw"),
    ("std/net", "send_all_raw"),
    ("std/net", "recv_raw"),
    ("std/net", "recv_into_raw"),
    ("std/net", "send_all_try_raw"),
    ("std/net", "recv_try_raw"),
    ("std/net", "recv_into_try_raw"),
    ("std/net", "set_timeout_raw"),
    ("std/net", "local_port_raw"),
    ("std/net", "peer_addr_raw"),
    ("std/net", "resolve_raw"),
    ("std/tls", "set_timeout_raw"),
    ("std/tls", "write_try_raw"),
    ("std/tls", "write_all_try_raw"),
    ("std/tls", "read_try_raw"),
    ("std/tls", "read_into_try_raw"),
    ("std/tls", "shutdown_try_raw"),
];

/// The name `module` writes in its own `extern fun` declarations for
/// `qualified_name`: the module prefix comes off the function's `tir::Function`
/// key, which `resolve.rs` has already prefixed with the module path.
fn bare_name<'a>(module: &str, qualified_name: &'a str) -> &'a str {
    let prefix = if module.is_empty() {
        String::new()
    } else {
        format!("{}.", module.replace('/', "."))
    };
    qualified_name
        .strip_prefix(prefix.as_str())
        .unwrap_or(qualified_name)
}

/// Whether the extern is provided by the Rust runtime archive instead of a C
/// body in the program's object.
pub fn is_rust_ported(module: &str, qualified_name: &str) -> bool {
    let bare = bare_name(module, qualified_name);
    RUST_PORTED.iter().any(|(m, n)| *m == module && *n == bare)
}

/// Whether the compiler ships a body for this `(module, bare name)` pair, in
/// the Rust runtime. Used by `infer.rs` to reject an `extern` declaration it
/// cannot honor.
pub fn has_body(module: &str, bare_name: &str) -> bool {
    RUST_PORTED
        .iter()
        .any(|(m, n)| *m == module && *n == bare_name)
}

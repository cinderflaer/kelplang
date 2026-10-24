use super::*;

// ---------------------------------------------------------------------------
// MK9-S1-M1 (roadmap B-1): a user module's own `extern fun`, bound to a
// native symbol through coral.toml's `[extern."<module id>"]` section instead
// of a compiler-supplied body from `externs.rs`. Gated exactly like a stdlib
// module's domain (same `gated` map, same resolution-time refusal), plus two
// manifest-shape checks the stdlib path has no equivalent for: an unknown
// domain, and a symbol outside the module's own allowlist.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_manifest_assigned_ffi_module_calls_its_allowlisted_native_symbol() {
    assert_project_runs("ffi_getpid", &project("ffi_getpid"), "true\n");
}

#[test]
pub(crate) fn an_ungranted_ffi_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "ffi_denied",
        &project("ffi_denied"),
        "needs the 'process' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn a_manifest_assigned_ffi_ui_module_is_reached_with_the_wildcard_grant() {
    assert_project_runs("ffi_ui_domain", &project("ffi_ui_domain"), "true\n");
}

#[test]
pub(crate) fn an_ffi_ui_grant_that_names_a_pattern_is_rejected() {
    // A display call carries no argument to match, so `ui = ["wayland-0"]`
    // would read as enforced and deny nothing; only the wildcard is accepted
    // (kelp wiki page libraries/polyp.md).
    assert_project_rejected(
        "ffi_ui_pattern",
        &project("ffi_ui_pattern"),
        "patterns are not enforceable in the 'ui' domain yet",
    );
}

#[test]
pub(crate) fn an_ungranted_ffi_ui_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "ffi_ui_denied",
        &project("ffi_ui_denied"),
        "needs the 'ui' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn an_extern_manifest_section_with_an_unknown_domain_is_rejected() {
    assert_project_rejected(
        "ffi_bad_domain",
        &project("ffi_bad_domain"),
        "'disk' is not a capability domain",
    );
}

#[test]
pub(crate) fn an_extern_fun_outside_its_manifest_symbol_allowlist_is_rejected() {
    assert_project_rejected(
        "ffi_bad_symbol",
        &project("ffi_bad_symbol"),
        "is not in this module's coral.toml",
    );
}

#[test]
pub(crate) fn a_manifest_without_a_header_key_builds_the_ffi_project() {
    assert_project_runs("ffi_header", &project("ffi_header"), "true\n");
}

#[test]
pub(crate) fn an_unknown_ffi_pkg_config_package_fails_the_build() {
    let target = project("ffi_unknown_pkg_config");
    let bin = out_path("ffi_unknown_pkg_config");
    let build = kelpc()
        .env_remove("PKG_CONFIG_PATH")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected 'ffi_unknown_pkg_config' to be rejected, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("pkg-config failed") && stderr.contains("kelp-nonexistent-pkg-9zz"),
        "expected the error to name pkg-config and the package, got:\n{stderr}"
    );
}

#[test]
pub(crate) fn a_removed_ffi_header_key_is_rejected_even_when_empty() {
    assert_project_rejected(
        "ffi_bad_header",
        &project("ffi_bad_header"),
        "header' was removed",
    );
}

#[test]
pub(crate) fn a_removed_ffi_header_key_with_a_parent_segment_is_rejected() {
    assert_project_rejected(
        "ffi_bad_header_dotdot",
        &project("ffi_bad_header_dotdot"),
        "header' was removed",
    );
}

#[test]
pub(crate) fn a_manifest_ffi_symbol_rename_binds_the_kelp_name_to_the_c_symbol() {
    assert_project_runs("ffi_rename", &project("ffi_rename"), "true\n");
}

#[test]
pub(crate) fn an_ffi_symbol_rename_to_a_non_identifier_is_rejected() {
    assert_project_rejected(
        "ffi_bad_rename",
        &project("ffi_bad_rename"),
        "C symbol for '[extern.\"sys\"] symbols' must be a C identifier",
    );
}

#[test]
pub(crate) fn an_ffi_opaque_handle_allocates_compares_and_frees() {
    // Gap 1: "malloc" returns an opaque "Buffer", "==" compares two handles
    // of that type, and "free" takes one back, all straight through the C
    // trampoline.
    assert_project_runs("ffi_handle", &project("ffi_handle"), "true\n");
}

#[test]
pub(crate) fn an_ffi_extern_type_outside_an_extern_section_is_rejected() {
    assert_project_rejected(
        "ffi_handle_no_section",
        &project("ffi_handle_no_section"),
        "is only legal in a module with a coral.toml",
    );
}

#[test]
pub(crate) fn an_ffi_opaque_handle_upcasts_to_its_parent_and_casts_back_down() {
    // Gap 1: "Buffer" is a "Block". The "Buffer" from "malloc" converts
    // implicitly to "Block" at a call argument, at a `val` binding and at a
    // return, and "Buffer.cast(block)" is the unchecked way back down.
    assert_project_runs("ffi_handle_upcast", &project("ffi_handle_upcast"), "true\n");
}

#[test]
pub(crate) fn an_ffi_opaque_handle_carries_a_null_value() {
    // Gap 1: "fopen" returns the null handle on failure, and "File.null()"
    // spells the same value; "is_null()" is true for both and the only method
    // a handle has.
    assert_project_runs("ffi_handle_null", &project("ffi_handle_null"), "true\ntrue\n");
}

#[test]
pub(crate) fn an_implicit_ffi_handle_downcast_is_rejected() {
    // Gap 1: only a child converts up to its parent implicitly, so a "Block"
    // returned where a "Buffer" is expected is a type error, not an implicit
    // (and unchecked) downcast.
    assert_project_rejected(
        "ffi_handle_bad_downcast",
        &project("ffi_handle_bad_downcast"),
        "declared to return",
    );
}

#[test]
pub(crate) fn an_ffi_handle_null_outside_an_extern_section_is_rejected() {
    // Gap 1: a handle operation is legal only where an `[extern.*]` section
    // exists, even when the handle type itself is imported from an extern
    // module.
    assert_project_rejected(
        "ffi_handle_null_outside",
        &project("ffi_handle_null_outside"),
        "is only legal in a module with a coral.toml",
    );
}

#[test]
pub(crate) fn printing_an_ffi_opaque_handle_is_rejected() {
    assert_project_rejected(
        "ffi_handle_print",
        &project("ffi_handle_print"),
        "opaque native handle with no printable value",
    );
}

#[test]
pub(crate) fn an_ffi_callback_closure_parameter_that_moves_is_rejected() {
    assert_project_rejected(
        "ffi_callback_move_param",
        &project("ffi_callback_move_param"),
        "forwards every argument by value",
    );
}

#[test]
pub(crate) fn an_ffi_extern_fun_with_two_callback_closures_is_rejected() {
    assert_project_rejected(
        "ffi_callback_two",
        &project("ffi_callback_two"),
        "at most one 'move closure fun(...)' parameter",
    );
}

#[test]
pub(crate) fn an_ffi_variadic_marker_that_is_not_last_is_rejected() {
    // A fixture, not a project: `...` followed by another parameter is a
    // syntax error, and the fixtures are where the suite keeps programs that
    // do not parse (the formatter is right to refuse one).
    assert_rejected_with(
        "tests/fixtures/ffi_variadic_not_last.kp",
        "must be the last parameter of an 'extern fun'",
    );
}

#[test]
pub(crate) fn an_ffi_variadic_extra_argument_of_a_c_unsafe_type_is_rejected() {
    assert_project_rejected(
        "ffi_variadic_bad_arg",
        &project("ffi_variadic_bad_arg"),
        "extra argument 1 of 'helper.kelp_va_sum' has type 'String'",
    );
}

#[test]
pub(crate) fn a_granted_module_can_write_rename_size_and_remove_inside_its_grant() {
    let dir = caps_scratch("write");
    assert_project_runs(
        "caps_write",
        &caps_project("caps_write").join("main.kp"),
        "true\n5\nfalse\nhello\nfalse\ntrue\n3\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_path_outside_the_grant_is_denied_at_runtime() {
    assert_project_denied_at_runtime(
        "caps_denied",
        &caps_project("caps_denied").join("main.kp"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_symlink_inside_the_grant_pointing_outside_is_denied() {
    let dir = caps_scratch("symlink");
    std::os::unix::fs::symlink("/etc/passwd", dir.join("escape")).unwrap();
    assert_project_denied_at_runtime(
        "caps_symlink",
        &caps_project("caps_symlink").join("main.kp"),
        "/etc/passwd",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied() {
    // The capability check used to re-evaluate the resource argument, so the check admitted one path and the call read another. This
    // project pins both halves of that fix in one run.
    //
    // `util.inside_once()` is the fail-open shape: `mk()` names a path inside
    // the grant the first time it runs and `/etc/passwd` the second, and it
    // prints `SIDE` on every call. The argument is evaluated once now, so the
    // stdout is one `SIDE` and the in-grant file's contents; before the fix it
    // was two and the contents of `/etc/passwd`.
    //
    // `util.outside()` is the control, out of the grant on every evaluation,
    // and its denial is asserted whole rather than by substring, so the path,
    // the location, the `granted:` line and its indentation are all pinned.
    let dir = caps_scratch("double_eval");
    std::fs::write(dir.join("a"), "FILECONTENT").unwrap();
    let target = caps_project("caps_double_eval").join("main.kp");
    let bin = out_path("caps_double_eval");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'caps_double_eval' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "SIDE\nFILECONTENT\n",
        "the resource argument was not evaluated exactly once, or the call read \
         a different path than the check admitted"
    );
    assert_eq!(run.status.code(), Some(1), "the control must be denied");
    // `util.kp:23:31` is `fs.read_text("/etc/passwd")` in the control, as the
    // compiler was invoked with the file. (`pub` on the control's own line is
    // what moved the column from 27; A-13.)
    let expected = format!(
        "{}:23:31: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_double_eval/**\n",
        caps_project("caps_double_eval").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the capability denial's text is not what the compiler writes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// MK7 extern gating: the four routes that made a grant advisory (`BUG-wQ9tL`).
//
// A gated module's item is gated wherever it is *named*: its `extern fun`
// primitives and its receiver methods sit in the gate table beside its free
// functions, and the naming sites the check has to reach are the direct call,
// a function value, and a `pd` initializer. One test per route, each refused or
// denied where before it read the file.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn an_ungranted_module_cannot_name_another_modules_primitive() {
    // Before the fix this built and printed the length of `/etc/hostname` on
    // both backends: the gate table held only a gated module's non-extern free
    // functions, so a user module could reach around the wrapper to the raw
    // syscall with no grant at all.
    assert_project_rejected(
        "caps_raw_ungranted",
        &project("caps_raw_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn a_path_outside_the_grant_is_denied_at_a_primitive_too() {
    // The domain is held, so the name resolves and the *pattern* check is what
    // has to fire. A raw call goes through the same `cap_plan` a wrapper call
    // does, so its denial names the same module, call site and grant.
    assert_project_denied_at_runtime(
        "caps_raw_denied",
        &caps_project("caps_raw_denied").join("main.kp"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_granted_module_reads_inside_its_grant_through_a_primitive() {
    // The positive half of the same rule: a non-entry module that holds the
    // domain names the primitive and reads a path inside its grant. A fix that
    // hid the primitives instead would fail here.
    let dir = caps_scratch("raw_granted");
    std::fs::write(dir.join("note.txt"), "hello caps").unwrap();
    assert_project_runs(
        "caps_raw_granted",
        &caps_project("caps_raw_granted").join("main.kp"),
        "hello caps\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn the_position_tables_keep_the_mode_and_the_directory_out_of_the_check() {
    // `fs.open_raw(path, "r")` must be checked on the path and not on the mode
    // (a relative "r" denies), and `process.spawn_raw(dir, program, ...)` on the
    // program and not on the directory. The mirror case is the next test.
    let dir = caps_scratch("raw_positions");
    std::fs::write(dir.join("note.txt"), "note content\n").unwrap();
    assert_project_runs(
        "caps_raw_positions",
        &caps_project("caps_raw_positions").join("main.kp"),
        "true\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn the_spawn_check_matches_the_program_and_not_the_directory() {
    // The mirror of the case above: the directory is inside the grant and the
    // program is not, so the denial has to name the *program*. Checking
    // argument 0 would have denied the directory and let the program through.
    assert_project_denied_at_runtime(
        "caps_raw_positions_denied",
        &caps_project("caps_raw_positions_denied").join("main.kp"),
        "is outside the process capability granted to module 'spawner'",
    );
}

#[test]
pub(crate) fn a_primitive_checks_its_resource_argument_once() {
    // The eval-once invariant at a primitive call site, the shape
    // `a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied`
    // pins for `fs.read_text`. `mk()` prints on every call and names an
    // in-grant path the first time and `/etc/passwd` the second, so a stdout
    // other than one `SIDE` and the in-grant file's length is the check and the
    // call reading different evaluations. The control's denial is asserted
    // whole, so the path, the location and the `granted:` block are pinned.
    let dir = caps_scratch("raw_double_eval");
    std::fs::write(dir.join("a"), "FILECONTENT").unwrap();
    let target = caps_project("caps_raw_double_eval").join("main.kp");
    let bin = out_path("caps_raw_double_eval");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'caps_raw_double_eval' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "SIDE\n11\n",
        "the primitive's resource argument was not evaluated exactly once"
    );
    assert_eq!(run.status.code(), Some(1), "the control must be denied");
    let expected = format!(
        "{}:20:28: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_raw_double_eval/**\n",
        caps_project("caps_raw_double_eval").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the primitive's capability denial is not what the compiler writes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn an_ungranted_module_cannot_name_a_gated_item_as_a_function_value() {
    // A function value names the item without a call site, and before the fix
    // `check_capability` was reached only from the direct-call arm, so both
    // spellings built and read `/etc/hostname` with no grant. The domain gate
    // is the half that can apply to a name, and it applies to both.
    assert_project_rejected(
        "caps_value_ungranted",
        &project("caps_value_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
    assert_project_rejected(
        "caps_raw_value_ungranted",
        &project("caps_raw_value_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn an_ungranted_module_cannot_name_a_gated_modules_receiver_method() {
    // A method of a gated module reaches the syscall without naming any of the
    // module's free functions, so the gate has to be on the method's own key
    // (`Type#method`). Before the fix there was no gate check on the method path
    // at all, and `Child.kill(0)` ran a real `kill(2)` with no grant.
    assert_project_rejected(
        "caps_method_ungranted",
        &project("caps_method_ungranted").join("main.kp"),
        "needs the 'process' capability, which module 'util' was not granted",
    );
    // The same rule through another gated module's methods, so this is the gate
    // table's answer and not a special case for `std/process`.
    assert_project_rejected(
        "caps_method_net_ungranted",
        &project("caps_method_net_ungranted").join("main.kp"),
        "needs the 'net' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn a_granted_module_calls_a_gated_receiver_method_with_no_pattern() {
    // The positive control: `caller` holds `process` with no program at all,
    // because `Child.wait`/`Child.is_running` name no resource the grant
    // governs. A false denial here would leave every tool that manages its own
    // children unbuildable.
    //
    assert_project_runs(
        "caps_method_granted",
        &project("caps_method_granted").join("main.kp"),
        "true\n",
    );
}

#[test]
pub(crate) fn a_pd_initializer_outside_the_grant_is_denied_at_runtime() {
    // The initializer is checked at compile time against its module's grant and
    // has to be checked at run time against the same module's patterns. The
    // denial is pinned whole here, including the module name and the `granted:`
    // block from `util`'s own allowlist.
    let target = caps_project("pd_gated_denied").join("main.kp");
    let bin = out_path("pd_gated_denied");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'pd_gated_denied' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(run.status.code(), Some(1), "the initializer must be denied");
    let expected = format!(
        "{}:3:21: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_pd_gated_denied/**\n",
        caps_project("pd_gated_denied").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the `pd` initializer's capability denial is not what the compiler writes"
    );
}

#[test]
pub(crate) fn a_pd_initializer_inside_the_grant_still_runs() {
    let dir = caps_scratch("pd_gated_granted");
    std::fs::write(dir.join("note.txt"), "x").unwrap();
    assert_project_runs(
        "pd_gated_granted",
        &caps_project("pd_gated_granted").join("main.kp"),
        "true\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_granted_module_creates_a_symlink_inside_its_grant() {
    // `fs.symlink` checks *both* of its paths, so a link can only be created
    // where the grant reaches and can only point where the grant reaches. The
    // project proves the link is a link (removing the target dangles it), then
    // removes it through `fs.remove`, whose check resolves the link's own path.
    let dir = caps_scratch("link");
    assert_project_runs(
        "caps_link",
        &caps_project("caps_link").join("main.kp"),
        "true\nhello link\nfalse\n0\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_symlink_whose_target_is_outside_the_grant_cannot_be_created() {
    // The link's own path is inside the grant; the target is not, and the
    // denial names the *target*, the check runs in argument order, and it is
    // what keeps a grant closed under link creation.
    let dir = caps_scratch("link_denied");
    assert_project_denied_at_runtime(
        "caps_link_denied",
        &caps_project("caps_link_denied").join("main.kp"),
        "'/etc/passwd' is outside the fs capability granted to module 'util'",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_granted_env_reads_sets_and_unsets_only_the_granted_names() {
    // MK5-S5: the bodies are real now. `HOME` is read through the grant, and
    // `KELP_ENV_PROBE` is set, read back, unset and confirmed gone, all inside
    // the child process, so the harness's environment is untouched.
    assert_project_runs(
        "caps_env",
        &project("caps_env").join("main.kp"),
        "true\ntrue\ntrue\n",
    );
}

#[test]
pub(crate) fn an_ungranted_env_name_is_denied_before_the_call() {
    // The name is the governed resource: a grant on PATH does not let the
    // holder ask whether HOME exists, so the panic names the attempted name
    // and the granted pattern.
    assert_project_denied_at_runtime(
        "caps_env_denied",
        &project("caps_env_denied").join("main.kp"),
        "'HOME' is outside the env capability granted to module 'util'",
    );
}

// ---------------------------------------------------------------------------
// perf-cap-llvm-inline: `CapCheck::Name`'s two fast paths, hand-built as
// inline LLVM IR in `codegen_llvm.rs::emit_cap_checks` instead of a call into
// `cap_glue.c`'s separately-compiled `kelp_cap_check_name`. These are
// functional tests (allow and deny, run through the default `llvm` backend);
// the generated-IR shape itself (no `call ... @kelp_cap_check_name` left at
// the specialized call site) is inspected by hand, not asserted here, the
// same way `a_full_grant_elides_the_runtime_path_check_but_a_narrow_one_keeps_it`
// reads generated text only for the C-backend elision it is about.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_lone_wildcard_name_grant_admits_a_slash_free_name() {
    // `env = ["*"]` is the domain's whole-resource pattern. `cap_glue.c`'s own
    // fast path (and now the LLVM backend's inline IR) reads it as "the name
    // has no `/`", so a plain name passes.
    assert_project_runs(
        "caps_name_wildcard_allowed",
        &project("caps_name_wildcard_allowed").join("main.kp"),
        "true\n",
    );
}

#[test]
pub(crate) fn the_lone_wildcard_name_grant_denies_a_name_with_a_slash() {
    // The same `*` grant does not cross a `/` byte: a name that has one is
    // denied even though `*` is the whole-resource pattern for the domain.
    assert_project_denied_at_runtime(
        "caps_name_wildcard_denied",
        &project("caps_name_wildcard_denied").join("main.kp"),
        "'has/nested' is outside the env capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_single_literal_name_grant_admits_the_exact_granted_name() {
    // One pattern, no `*` anywhere: the fast path is a direct byte compare
    // against the granted text, and the exact name it names passes.
    assert_project_runs(
        "caps_name_literal_allowed",
        &project("caps_name_literal_allowed").join("main.kp"),
        "true\n",
    );
}

#[test]
pub(crate) fn a_single_literal_name_grant_denies_a_name_that_is_only_a_prefix_match() {
    // `PATH` granted, `PATHX` asked for: a naive prefix compare would let
    // this through. The fast path compares the full length AND confirms
    // `name[pattern.len()]` is the terminator, so a longer name sharing the
    // pattern as a prefix is still denied, not silently admitted.
    assert_project_denied_at_runtime(
        "caps_name_literal_prefix_denied",
        &project("caps_name_literal_prefix_denied").join("main.kp"),
        "'PATHX' is outside the env capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn time_and_rand_are_gated_domains_with_no_named_resource() {
    assert_compiles_and_runs(
        "tests/fixtures/caps_time_rand.kp",
        "true\ntrue\ntrue\n4\nslept\n",
    );
}

#[test]
pub(crate) fn path_normalize_is_the_function_the_capability_checker_uses() {
    assert_compiles_and_runs(
        "tests/fixtures/path_normalize.kp",
        "/a/b/c\n/a/c\n/a\n/tmp/scratch\n",
    );
}

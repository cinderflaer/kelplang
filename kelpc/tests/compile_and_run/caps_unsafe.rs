use super::*;

// ---------------------------------------------------------------------------
// M7: capability-based security. A module's grant is received from its
// importer, never self-declared; the gate is a resolution error; the matching
// is a compiler-inserted runtime check against the caller's compiled-in
// allowlist.
// ---------------------------------------------------------------------------

/// The placeholder a capability fixture's source carries instead of a literal
/// scratch root (BUG-q8Vkr). A capability pattern has to be a literal, absolute
/// path in the fixture's own source text (a grant is source), so the harness
/// cannot simply hand the fixture a per-run directory the way `assert_*`
/// passes an expected string: the substitution has to happen in the text
/// itself, before `kelpc` ever lexes it. This mirrors `std/http`'s
/// `__KELP_HTTP_POOL__` (`stdlib.rs::HTTP_POOL_PLACEHOLDER`), substituted into
/// shipped module text before lexing; `caps_project` is the fixture-suite
/// equivalent, substituting into a throwaway copy instead of compiler-shipped
/// text.
pub(crate) const SCRATCH_PLACEHOLDER: &str = "__KELP_SCRATCH__";

/// The root every capability fixture's scratch paths are substituted under for
/// this process, computed once and reused for the life of the test binary.
///
/// Two suites running at once, each in its own worktree, used to both write
/// and delete the literal `/tmp/kelp_caps_*` trees the fixtures' source named,
/// which is the flake BUG-q8Vkr records. Keying the root on this process's own
/// id keeps two concurrent `cargo test` invocations apart even when they share
/// one `/tmp` (a `TMPDIR` pointed elsewhere works the same way, since
/// `std::env::temp_dir()` is the base rather than a literal `/tmp`).
pub(crate) fn scratch_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::temp_dir().join(format!("kelpc_test_scratch_{}", std::process::id()))
    })
}

/// The scratch directory of the running test. It sits under `scratch_root()`,
/// so it is unique to this process, and it is named after the test (libtest
/// names each test's thread after the test), so no two tests share it.
pub(crate) fn test_scratch() -> PathBuf {
    let test = std::thread::current().name().unwrap_or("main").to_string();
    scratch_root().join(test)
}

/// Owns the running test's scratch directory. It is cleared when the test
/// starts and removed when the guard drops, so a test leaves nothing behind
/// whether it passes or panics. The process's scratch root is not removed: other
/// tests in the same process may be creating their directories under it at
/// the same moment, so the root is left for the OS temp cleanup.
pub(crate) struct TestScratch(PathBuf);

impl TestScratch {
    pub(crate) fn new() -> Self {
        let dir = test_scratch();
        let _ = std::fs::remove_dir_all(&dir);
        TestScratch(dir)
    }
}

impl Drop for TestScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `std/fs` fixtures name their scratch paths as literal `/tmp/kelp_fs_*`
/// text. Two suites running at once share the host's `/tmp`, so one suite's
/// run could delete or read another's files, and the `fs_breadth` and
/// `fs_set_mode` tests failed that way. `reroute_tmp` moves those paths under
/// `test_scratch()`, so each test writes only under its own directory. Each
/// fixture keeps its `/tmp` text on disk, so it still runs as written (the
/// ignored golden sweep builds it that way).
pub(crate) fn reroute_tmp(text: &str) -> String {
    text.replace("/tmp/kelp_fs_", &format!("{}/kelp_fs_", test_scratch().display()))
}

/// Writes a copy of `src` (a fixture file, or a project's `main.kp`) into a
/// fresh directory named `dir_name` under `out_path`, with its scratch paths
/// rerouted by `reroute_tmp`, and returns the copy to build. The test's
/// scratch directory is created first, so the program can write under it.
pub(crate) fn rerouted_copy(src: &Path, dir_name: &str) -> PathBuf {
    std::fs::create_dir_all(test_scratch()).expect("could not create the test's scratch directory");
    let dir = out_path(dir_name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the rerouted fixture directory");
    let text = std::fs::read_to_string(src).expect("could not read fixture source");
    let target = dir.join(src.file_name().expect("a fixture source has a file name"));
    std::fs::write(&target, reroute_tmp(&text)).expect("could not write the rerouted fixture");
    target
}

/// A scratch directory the capability fixtures point at. Their patterns are
/// absolute (a grant must be), so the path has to exist before the program
/// runs and is removed afterwards.
pub(crate) fn caps_scratch(name: &str) -> PathBuf {
    let dir = scratch_root().join(format!("kelp_caps_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the capability scratch directory");
    dir
}

/// A capability fixture project, copied into a throwaway, process-unique
/// directory with `SCRATCH_PLACEHOLDER` substituted for `scratch_root()` in
/// every file's text. The original under `tests/projects/<name>` is untouched
/// (it is shared with every other worktree checked out from the same
/// history), and the copy is deliberately thrown away and rewritten on every
/// call, the same way `caps_scratch`'s directory is.
///
/// Callers use this exactly like `project(name)`: bare, for a manifest-driven
/// project directory (`coral.toml` + `src/`, C-2's `[caps]` fixtures), or
/// `.join("main.kp")`/`.join("util.kp")` for a flat, manifest-less one (every
/// other capability fixture) whose build target or denial-message path is one
/// file beside its siblings.
pub(crate) fn caps_project(name: &str) -> PathBuf {
    let src_dir = project(name);
    let dst_dir = out_path(&format!("{name}_src"));
    let _ = std::fs::remove_dir_all(&dst_dir);
    std::fs::create_dir_all(&dst_dir).expect("could not create the substituted project copy");
    let root = scratch_root().to_string_lossy().into_owned();
    copy_substituted(&src_dir, &dst_dir, &root);
    dst_dir
}

/// `caps_project`'s recursive half: copies every file under `src` to the same
/// relative path under `dst`, substituting `SCRATCH_PLACEHOLDER` for `root` in
/// each one's text, and recreates every subdirectory rather than skipping it.
/// A flat fixture (`main.kp`, `util.kp` beside each other) never recurses; a
/// manifest-driven one (`coral.toml` at the root, `.kp` files under `src/`)
/// needs the recursion to bring `src/` along at all.
pub(crate) fn copy_substituted(src: &Path, dst: &Path, root: &str) {
    for entry in std::fs::read_dir(src).expect("could not read fixture project dir") {
        let entry = entry.expect("could not read a fixture project dir entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target)
                .expect("could not create a substituted project subdirectory");
            copy_substituted(&path, &target, root);
        } else {
            let text = std::fs::read_to_string(&path).expect("could not read fixture source");
            let substituted = text.replace(SCRATCH_PLACEHOLDER, root);
            std::fs::write(&target, substituted)
                .expect("could not write the substituted fixture source");
        }
    }
}

/// Builds a caps project, runs it, and asserts the run fails with `needle` on
/// stderr, the runtime-denial path, which only a *granted* module can reach.
pub(crate) fn assert_project_denied_at_runtime(name: &str, target: &Path, needle: &str) {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{name}' to be denied at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the denial for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

#[test]
pub(crate) fn a_granted_module_reads_inside_its_grant() {
    let dir = caps_scratch("granted");
    std::fs::write(dir.join("note.txt"), "hello caps").unwrap();
    assert_project_runs(
        "caps_granted",
        &caps_project("caps_granted").join("main.kp"),
        "hello caps\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn an_ungranted_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "caps_ungranted",
        &caps_project("caps_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
pub(crate) fn a_module_cannot_delegate_more_than_it_holds() {
    assert_project_rejected(
        "caps_escalation",
        &caps_project("caps_escalation").join("main.kp"),
        "can delegate less than it has, never more",
    );
}

// ---------------------------------------------------------------------------
// C-2 (roadmap open item 18): coral.toml's [caps] section restricts the
// entry module's own capability set, which the entry module could not
// restrict any other way, an ordinary `caps(...)` clause on one of its own
// `impl` lines grants the *imported* module, never main itself. A project
// with no [caps] section keeps the old default (main holds everything, every
// project above this point in the file has none), so these three are the
// only fixtures pinning the new surface.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_entry_modules_own_caps_section_can_leave_a_domain_ungranted() {
    // This project's coral.toml [caps] grants only 'env', so main's direct
    // reference to 'fs' resolves against nothing, a resolution error at the
    // same place and with the same wording an imported module's ungranted
    // reference gets. Nothing in this fixture touches disk (the build never
    // gets past resolution), so it needs no scratch directory at all.
    assert_project_rejected(
        "caps_entry_ungranted",
        &caps_project("caps_entry_ungranted"),
        "needs the 'fs' capability, which module 'main' was not granted",
    );
}

#[test]
pub(crate) fn a_caps_restricted_entry_module_can_still_reach_what_it_was_granted() {
    let dir = caps_scratch("entry_restricted");
    assert_project_runs(
        "caps_entry_restricted",
        &caps_project("caps_entry_restricted"),
        "true\nhi\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn a_caps_restricted_entry_module_is_denied_outside_its_own_pattern_at_runtime() {
    // 'fs' is held (resolution passes), but only for a scratch directory this
    // project's main.kp never names (it reads '/tmp', not the granted
    // directory), so the denial is the runtime pattern check, not the
    // resolution gate above, and it names 'main', the way
    // a_path_outside_the_grant_is_denied_at_runtime names 'util' for a
    // delegated module. No scratch directory needs to exist on disk for that
    // same reason (a_path_outside_the_grant_is_denied_at_runtime creates none
    // either): the check is a pattern match against the argument, not a
    // lookup inside the granted directory.
    assert_project_denied_at_runtime(
        "caps_entry_pattern_denied",
        &caps_project("caps_entry_pattern_denied"),
        "outside the fs capability granted to module 'main'",
    );
}

#[test]
pub(crate) fn a_delegated_wildcard_lets_the_import_reach_every_resource() {
    // `caps(*)` is the whole-resource wildcard of every domain, handed to the
    // import. `helper` reads one file in each of two unrelated trees, which no
    // single narrow pattern could cover, so a passing run is the wildcard
    // travelling with the import.
    let first = caps_scratch("star");
    let second = caps_scratch("star_elsewhere");
    std::fs::write(first.join("first.txt"), "hello ").unwrap();
    std::fs::write(second.join("second.txt"), "world").unwrap();
    assert_project_runs(
        "caps_star",
        &caps_project("caps_star").join("main.kp"),
        "hello world\n",
    );
    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

#[test]
pub(crate) fn an_env_grant_cannot_name_a_slashy_variable() {
    // The scenario the elision was approved on was an exact, literal grant for
    // a name that contains a `/`, which `std/env`'s own wildcard could never
    // admit and the elision would have made newly sufficient. No such program
    // can be written: `resolve.rs::validate_cap_pattern` refuses a `/` in an
    // `env` pattern by name, at the `impl` line, so the only grant that can
    // ever admit a slashy name is none at all. That is also why the elision
    // cannot admit one where a grant could not.
    assert_project_rejected(
        "caps_elide_name_slashy_grant",
        &project("caps_elide_name_slashy_grant").join("main.kp"),
        "'has/slash' is not a variable name",
    );
}

#[test]
pub(crate) fn a_module_cannot_delegate_the_wildcard_it_does_not_hold() {
    // `mid` holds one narrow `fs` pattern, so `caps(*)` on its own import
    // claims a wildcard it was never given; the refusal names the wildcard.
    assert_project_rejected(
        "caps_star_escalation",
        &caps_project("caps_star_escalation").join("main.kp"),
        "cannot give 'inner' the whole-resource wildcard for 'fs' with `caps(*)`",
    );
}

#[test]
pub(crate) fn a_named_domain_beside_the_wildcard_is_refused() {
    // A fixture, not a project: the mixed clause is a syntax error, and the
    // fixtures are where the suite keeps programs that do not parse (the
    // formatter is right to refuse one and must not be asked to format it).
    assert_rejected_with(
        "tests/fixtures/caps_star_mixing.kp",
        "already means every domain",
    );
}

#[test]
pub(crate) fn the_wildcard_on_a_module_that_reaches_no_domain_is_refused() {
    assert_project_rejected(
        "caps_star_ungated",
        &project("caps_star_ungated").join("main.kp"),
        "'std/str' is ungated: it reaches no capability domain",
    );
}

#[test]
pub(crate) fn an_ungranted_importer_is_named_as_the_line_to_change() {
    // `main` imports `store` with no `caps(...)`, so the error names `store`
    // even though the grant that has to change is `main`'s bare `impl` line.
    assert_project_rejected(
        "caps_delegate_from_nothing",
        &project("caps_delegate_from_nothing").join("main.kp"),
        "'store' holds nothing here because the module that imports it wrote no",
    );
}

#[test]
pub(crate) fn a_pattern_containing_dotdot_is_rejected() {
    assert_project_rejected(
        "caps_bad_pattern",
        &project("caps_bad_pattern").join("main.kp"),
        "contains a '..' segment",
    );
}

#[test]
pub(crate) fn an_unknown_capability_domain_is_rejected() {
    assert_project_rejected(
        "caps_bad_domain",
        &project("caps_bad_domain").join("main.kp"),
        "'disk' is not a capability domain",
    );
}

// ---------------------------------------------------------------------------
// Stage n (MK9-S2, roadmap B-3): `unsafe` and raw pointers. `Ptr<T>` is a
// builtin generic like `Atomic<T>`, its operations are `Ptr#<method>` on both
// backends, and the manifest's `[unsafe] modules` list is the audit rule: only
// a module named there may contain an `unsafe` block or an `unsafe fun`.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_raw_pointer_allocates_writes_reads_volatile_and_frees() {
    assert_project_runs(
        "unsafe_ptr",
        &project("unsafe_ptr"),
        "100\n99\n65\n42\n42\ntrue\n4096\n",
    );
}

/// Kernel gap 2: `Ptr.from_fn(f)` is the address of a declared `fun` or a
/// non-capturing `fun` literal, and it is legal outside `unsafe`. Two distinct
/// functions have distinct, non-null addresses, and the same function answers
/// the same address twice.
#[test]
pub(crate) fn a_function_address_is_a_checked_pointer() {
    assert_fixture_runs("tests/fixtures/ptr_from_fn.kp");
}

/// A closure carries the environment its captures were moved into, so its code
/// address alone is not the value: `Ptr.from_fn` refuses one and says why.
#[test]
pub(crate) fn a_closure_has_no_standalone_function_address() {
    assert_rejected_with(
        "tests/fixtures/ptr_from_fn_closure.kp",
        "it carries an environment the code address alone cannot describe",
    );
}

#[test]
pub(crate) fn an_unsafe_fun_is_callable_from_an_unsafe_block() {
    assert_project_runs("unsafe_fn", &project("unsafe_fn"), "77\n");
}

#[test]
pub(crate) fn a_module_with_unsafe_but_no_manifest_entry_is_rejected() {
    assert_project_rejected(
        "unsafe_not_listed",
        &project("unsafe_not_listed"),
        "module 'main' is not listed in coral.toml [unsafe] modules",
    );
}

#[test]
pub(crate) fn an_unsafe_pointer_read_outside_a_block_is_rejected() {
    assert_project_rejected(
        "unsafe_outside_block",
        &project("unsafe_outside_block"),
        "'Ptr.read' is an unsafe operation",
    );
}

#[test]
pub(crate) fn an_unsafe_fun_called_outside_a_block_is_rejected() {
    assert_project_rejected(
        "unsafe_fn_outside",
        &project("unsafe_fn_outside"),
        "'poke' is an 'unsafe fun'",
    );
}

#[test]
pub(crate) fn a_pointer_to_an_owned_type_is_rejected() {
    assert_project_rejected(
        "unsafe_bad_elem",
        &project("unsafe_bad_elem"),
        "'String' is an owned heap value",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 2 (MK9-S1-M2, roadmap B-2): layout control. `repr(c)` (the
// default, now guaranteed), `repr(packed)`, `repr(align(N))`, the compile-time
// queries `size_of`/`align_of`/`offset_of`, and explicit discriminants on a
// field-less enum with `to_int`/`from_int`. Every fixture here runs on the
// default (LLVM) backend.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn a_packed_datac_has_the_c_layout_of_its_fields() {
    assert_fixture_runs("tests/fixtures/repr_packed.kp");
}

#[test]
pub(crate) fn a_repr_c_datac_keeps_the_default_padded_layout() {
    assert_fixture_runs("tests/fixtures/repr_c_padded.kp");
}

#[test]
pub(crate) fn a_layout_query_folds_in_an_array_length_and_a_constant() {
    assert_fixture_runs("tests/fixtures/layout_query_in_a_constant.kp");
}

#[test]
pub(crate) fn a_repr_align_datac_raises_its_own_and_an_embedded_type_s_alignment() {
    assert_fixture_runs("tests/fixtures/repr_align.kp");
}

#[test]
pub(crate) fn an_enum_round_trips_its_discriminants_through_to_int_and_from_int() {
    assert_fixture_runs("tests/fixtures/enum_discriminant.kp");
}

#[test]
pub(crate) fn a_repr_align_that_is_not_a_power_of_two_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/repr_align_not_power_of_two.kp",
        "power of two",
    );
}

#[test]
pub(crate) fn a_duplicate_enum_discriminant_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/enum_discriminant_duplicate.kp",
        "repeats the discriminant",
    );
}

#[test]
pub(crate) fn an_enum_with_a_payload_cannot_have_a_discriminant() {
    assert_rejected_with(
        "tests/fixtures/enum_discriminant_with_payload.kp",
        "may have an explicit discriminant",
    );
}

#[test]
pub(crate) fn offset_of_a_field_the_datac_does_not_have_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/offset_of_missing_field.kp",
        "has no field 'y'",
    );
}

/// The same stage, checked in Kelp alone: a packed datac has the size, alignment and field offsets of
/// the packed C struct it mirrors (1 + 4 + 2 bytes, no padding). The C helper that used to print the
/// second column is gone, so the expected values are the literals.
#[test]
pub(crate) fn a_packed_datac_has_the_packed_layout() {
    assert_project_runs(
        "ffi_layout",
        &project("ffi_layout"),
        "7\n1\n1\n5\n",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 3 (MK9-S8, roadmap B-10): endianness and unaligned access. The
// integer byte-order conversions (`swap_bytes`, `to_be`/`to_le` and the static
// `u32.from_be`/`u32.from_le`), the bounds-checked byte-order readers and
// writers on a `Bytes`, and `Ptr.read_unaligned`/`write_unaligned`. Every
// fixture here runs on the default (LLVM) backend.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn integer_byte_order_conversions_reverse_the_right_bytes() {
    // 23 hand-computed values: `0x12345678u32.swap_bytes()` is `0x78563412`,
    // `to_le` is the identity on this little-endian target, `to_be` is not, and
    // swapping twice is the identity.
    assert_fixture_runs("tests/fixtures/endian_scalars.kp");
}

#[test]
pub(crate) fn a_wire_header_round_trips_through_both_byte_orders() {
    // A 10-byte header read field by field in both orders, then rebuilt with
    // `write_*` and compared byte for byte with the pushed one.
    assert_fixture_runs("tests/fixtures/bytes_endian.kp");
}

#[test]
pub(crate) fn an_out_of_bounds_byte_order_read_panics() {
    assert_fixture_panics("bytes_read_oob", "Bytes.read_u32_le: offset out of bounds");
}

#[test]
pub(crate) fn an_out_of_bounds_byte_order_write_panics() {
    assert_fixture_panics("bytes_write_oob", "Bytes.write_u16_be: offset out of bounds");
}

/// An offset near `usize`'s maximum wraps `offset + width` back under the
/// buffer's length, so a naive test would read out of bounds instead of
/// panicking.
#[test]
pub(crate) fn an_offset_that_would_wrap_the_bounds_check_panics() {
    assert_fixture_panics(
        "bytes_read_wrapped_oob",
        "Bytes.read_u16_le: offset out of bounds",
    );
}

/// `Ptr.read_unaligned`/`write_unaligned`: every access is at an odd address,
/// which is exactly what the aligned `read`/`write` are not allowed to do.
#[test]
pub(crate) fn an_unaligned_pointer_read_and_write_round_trip() {
    assert_project_runs(
        "unsafe_unaligned",
        &project("unsafe_unaligned"),
        "12345678\nbeef\n102030405060708\n5060708\nfreed\n",
    );
}

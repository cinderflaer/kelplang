use super::*;

/// The container fixtures the LLVM backend builds, the ones whose binaries the
/// monomorphized `List`/`Map`/`Box`/`[T; N]` function sets generated into the
/// module (MK7 S2D) own heap memory for. A fixed list, not a sweep: only these
/// fixtures exercise those sets, and a fixture that uses a container but also
/// calls `std/mem` or reads a `pd` is refused by the backend (S3) rather than
/// built, so it would be skipped anyway.
pub(crate) const LLVM_CONTAINER_FIXTURES: &[&str] = &[
    "array_basic",
    "array_bounds",
    "array_bounds_set",
    "array_owned",
    "array_wide_and_float",
    "box_basic",
    "box_empty_panic",
    "box_field_take",
    "box_owned_elements",
    "box_recursive",
    "container_element_from_use",
    "enum_container",
    "enum_in_containers",
    "enum_recursive",
    "float_datac_containers",
    "generic_recursive",
    "list_basic",
    "list_box_element",
    "list_datac_element",
    "list_element_widths",
    "list_float_element",
    "list_growth_and_rebuild",
    "list_insert_bounds",
    "list_insert_remove_shifts",
    "list_narrow_element_pop",
    // A-10 slice 1: a `List`/`Box` element owns its own buffer or cell, so the
    // nested drop, `set` replacement and scope-exit drop all have to free each
    // inner container exactly once.
    "list_nested_list",
    "list_nested_open",
    "list_nested_owned",
    "list_owned_replacement",
    "list_pop_empty",
    "list_remove_bounds",
    "list_slice_insert",
    "list_sort_kinds",
    "map_clear_then_reuse",
    "map_growth_and_rehash",
    "map_key_types",
    "map_key_widths",
    "map_missing_key",
    "map_remove_missing",
    "map_tombstones_and_reuse",
    // A-6: the derived `datac`/`enum` map key, including the `keys()` clone
    // path and the entry's own copy of an owned key field.
    "map_datac_key",
    "map_datac_key_ok",
    "map_nested_datac_key",
    "map_enum_key",
    // A-12: a declared key pair, where the map reaches the type's own `hash`
    // and `eq` instead of a generated pair. The `datac` half owns a `String`
    // the pair walks, so the entry's clone and drop are checked; the `enum`
    // half owns nothing and checks the call path.
    "map_declared_key",
    "map_declared_key_enum",
    "nested_literal_staging",
    "recursive_datac",
    "stdlib_sort",
    // A-11: a partial move clears the field's or payload's slot in generated
    // IR, so the binding's own generated drop frees the projection that stayed
    // and nothing where the moved value now lives. The sweep covers every
    // fixture; these are here because the clear and the drop are IR this backend
    // emits, and the last three carry the enum-payload half out through the
    // stdlib helpers that take their receiver with `move`.
    "partial_move_field",
    "partial_move_conditional",
    "partial_move_reassign",
    "partial_move_move_param",
    "partial_move_enum_payload_owned",
    "option_unwrap_owned",
    "result_map_owned",
];

/// The S3A extern fixtures: the ones whose heap an `extern fun` body allocates,
/// converts or adopts. `extern_signatures` is this slice's own fixture (a
/// `Bytes` from `mem.alloc`, a `String` from `fmt.hex`/`str.from_f64` and one
/// from a parsed path), `path_normalize` is the body that calls into `std/rt`,
/// and the rest are the stdlib paths that reach `String.from_bytes` (the
/// 24-byte-by-value entry point the runtime object carries a pointer-spelled
/// adapter for, MK7 section 3.3).
pub(crate) const LLVM_EXTERN_FIXTURES: &[&str] = &[
    "extern_signatures",
    "path_normalize",
    "stdlib_build",
    "stdlib_fmt",
    "stdlib_math",
    "stdlib_mem",
    "stdlib_str",
    "string_views",
];

/// The S3A half of the memory gate: every extern-body binary the LLVM backend
/// builds, run under valgrind. The bodies themselves are C and the sweep
/// covers them, but the *calls* to them, the `String.from_bytes` adapter and the
/// buffers the bodies allocate and adopt move memory across that boundary, so
/// this is where a buffer freed twice or handed over without an owner shows up.
#[test]
#[ignore = "builds the extern fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
pub(crate) fn the_llvm_extern_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM extern sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_EXTERN_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build(name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM extern fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm extern", &checked, ""));

    assert!(
        checked.len() == LLVM_EXTERN_FIXTURES.len(),
        "the LLVM backend refused {:?}, which this slice must build",
        rejected,
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM extern runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S2D half of the memory gate: every container binary the LLVM backend
/// builds, run under valgrind. The sweep above covers every fixture; this one
/// covers the code the LLVM backend generates *instead* of C, which is the
/// part another backend cannot check for it: a container's drop, its growth
/// (`realloc`), a map's tombstone reuse and an `[T; N]`'s element drops all
/// free memory in generated IR, and only valgrind sees a double free or a leak
/// in it.
#[test]
#[ignore = "builds the container fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
pub(crate) fn the_llvm_container_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM container sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_CONTAINER_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build(name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM container fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm container", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the container fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM container runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S2E closure fixtures the LLVM backend builds: the ones a closure or a
/// function value gives heap to own. A capturing literal mallocs its
/// environment and the closure value owns it from then on, so a missed drop is
/// a leak and a second one a double free, and neither prints anything. The
/// widening fixtures add the two other allocations: a shim allocates nothing,
/// and a value whose identity the call knows gets a one-field heap environment
/// (`closure_value_widening` reaches both). `try_drops_frame`'s closure is one
/// the frame holds while `?` leaves it, so its environment is freed on the
/// early-return path.
pub(crate) const LLVM_CLOSURE_FIXTURES: &[&str] = &[
    "closure_datac_borrow_param",
    "closure_env_wide_ret",
    "closure_move",
    "closure_two_captures",
    "closure_value_widening",
    "fn_literal_capture",
    "fn_value_plain",
    "try_drops_frame",
];

/// The S2E half of the memory gate: every closure binary the LLVM backend
/// builds, run under valgrind. The sweep above covers every fixture; this covers
/// the environment the LLVM backend generates the allocation, the capture stores
/// and the destructor for, which is exactly the code another backend cannot
/// check for it.
#[test]
#[ignore = "builds the closure fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
pub(crate) fn the_llvm_closure_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM closure sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_CLOSURE_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build(name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM closure fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm closure", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the closure fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM closure runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S3 `pd` and capability fixtures the LLVM backend builds: the ones that
/// give a module-scope global something to own, and the ones that run the
/// capability glue.
///
/// A `pd` is storage that outlives every frame, so its drop point is the
/// `atexit` handler the generated `kelp_init_pds` registers; a missed handler
/// leaks, and one that frees twice fails here rather than in production. The
/// capability glue allocates: `kelp_cap_check_path` builds a `PATH_MAX` scratch
/// buffer on the stack and the matcher runs over it, and the deny path exits
/// through glibc's `atexit` chain, which is where a `pd` drop and the deny path
/// meet. Everything here also prints nothing about memory, so valgrind is the
/// only instrument that can see it.
pub(crate) const LLVM_PD_CAP_FIXTURES: &[&str] = &[
    // The passdowns, including the owned one and the container-typed one.
    "passdowns",
    "passdown_datac",
    "passdown_globals",
    "pd_owned",
    "pd_owned_container",
    "pd_shadowed",
    "pd_argument_forwarding",
    "pd_var_write",
    "pd_write_reaches_callers",
    "pd_var_datac_reassign",
    "pd_var_copy_read_twice",
    "pd_var_mut_argument",
    "pd_var_read_only_datac",
    "pd_var_read_only_shared",
    "list_field",
    "for_loop",
    // A gated call with a resource argument, checked at the call site.
    "caps_time_rand",
    "fs_errno",
    "fs_fallible",
    "fs_path_family",
    "fs_symlink_kind",
    "io_stdio",
    "io_streams",
    "io_write_bodies",
    "net_loopback",
    "net_poll",
    "net_resolve",
    "net_timeout",
    "path_absolute",
    "process_spawn",
    // The last three refusals: the receiver adapter and the owned-temporary
    // staging.
    "string_basic",
    "heap_field_of_temporary",
    "heap_field_of_owned_temporary",
];

/// The S3 half of the memory gate: every `pd` and capability binary the LLVM
/// backend builds, run under valgrind. `pd` storage and the capability glue are
/// the two places this slice touches memory across a boundary the sweep
/// cannot see: the `atexit` drop is registered by generated IR, and the check
/// calls into the C glue through the per-program object.
#[test]
#[ignore = "builds the passdown and capability fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
pub(crate) fn the_llvm_passdown_and_capability_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM passdown and capability sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));
    let _fs_run = FsRunDir;

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_PD_CAP_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build(name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM passdown/capability fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm passdown/capability", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the passdown or capability fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM passdown/capability runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

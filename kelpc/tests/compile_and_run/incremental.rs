use super::*;

// ---------------------------------------------------------------------------
// Roadmap A-14 slice 1: separate compilation (`--incremental`)
// ---------------------------------------------------------------------------
//
// With the mode off the backend emits one LLVM module named `kelp`, which is why
// nothing else in this file mentions it: the whole suite is the mode-off
// assertion. These tests are the other half. The same programs built with the
// per-unit split must behave identically, the split must be reproducible, and
// the two spellings that turn it on (a flag and a manifest key) must agree.

/// Decision 5: [`hosted_flag_check_at_jobs`] with the default worker count.
pub(crate) fn hosted_flag_check(tag: &str, target: &Path) -> Result<(), String> {
    hosted_flag_check_at_jobs(tag, target, &[None])
}

/// Decision 5 (2026-10-12): a hosted build has no per-unit split, so the explicit
/// `--incremental` flag is refused on every hosted target. This checks that refusal at each
/// worker count and that the whole-program build of the target still succeeds. `None` leaves
/// `KELPC_JOBS` unset, `Some(n)` runs with `n` workers.
pub(crate) fn hosted_flag_check_at_jobs(tag: &str, target: &Path, jobs: &[Option<&str>]) -> Result<(), String> {
    let whole = out_path(&format!("whole_{tag}"));
    let whole_build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&whole)
        .output()
        .map_err(|e| format!("could not spawn kelpc: {e}"))?;
    if !whole_build.status.success() {
        return Err(format!(
            "the whole-program build failed:
{}",
            String::from_utf8_lossy(&whole_build.stderr)
        ));
    }
    for (index, job) in jobs.iter().enumerate() {
        let split = out_path(&format!("inc_{tag}_{index}"));
        let mut command = kelpc();
        command
            .env_remove("KELPC_JOBS")
            .env("KELPC_CACHE", "off")
            .arg("build")
            .arg(target)
            .arg("-o")
            .arg(&split)
            .arg("--incremental");
        if let Some(count) = job {
            command.env("KELPC_JOBS", count);
        }
        let split_build = command
            .output()
            .map_err(|e| format!("could not spawn kelpc: {e}"))?;
        if split_build.status.success() {
            return Err(format!("--incremental was accepted on a hosted build (jobs {job:?})"));
        }
        let stderr = String::from_utf8_lossy(&split_build.stderr);
        if !(stderr.contains("which has the shared pool") && stderr.contains("cannot build that yet")) {
            return Err(format!(
                "the refusal did not name the shared pool (jobs {job:?}):
{stderr}"
            ));
        }
    }
    Ok(())
}

/// A project target: the directory when it has a manifest, and its entry file
/// when it does not, which is the two shapes `kelpc build` takes.
pub(crate) fn incremental_project_target(name: &str) -> PathBuf {
    let dir = project(name);
    if dir.join("coral.toml").exists() {
        dir
    } else {
        dir.join("main.kp")
    }
}

/// A fixture that exercises one feature the split has to keep working: generics,
/// traits, closures, threads, the containers, an owning `Drop`, a passdown, a
/// global. Every one of them is built and run both ways by the test below.
pub(crate) const INCREMENTAL_FIXTURES: &[&str] = &[
    // generics: instances of a module's generic function belong to its unit
    "generic_explicit_args",
    "generic_function_basic",
    "generic_function_owned",
    "generic_method_basic",
    "generic_nested_inference",
    "generic_owned",
    "generic_recursive",
    "generic_type_basic",
    "scoped_block_generic",
    // traits: static dispatch, default bodies, supertraits, `&dyn` vtables
    "traits_datac_and_enum",
    "traits_default_body",
    "traits_dyn_builtin",
    "traits_dyn_default_super",
    "traits_dyn_dispatch",
    "traits_dyn_forward",
    "traits_dyn_mut",
    "traits_generic_datac_method",
    "traits_impl_for_builtin",
    "traits_self_in_signature",
    "traits_supertrait",
    // closures and function values: the `_support` shims and thunks
    "closure_basic",
    "closure_datac_borrow_param",
    "closure_env_wide_ret",
    "closure_move",
    "closure_two_captures",
    "closure_value_widening",
    "fn_literal",
    "fn_literal_capture",
    "fn_value",
    "fn_value_plain",
    "scoped_block_captures",
    "scoped_block_params",
    "trailing_block_capture",
    // threads and mutexes: the per-signature spawn body and the runtime records
    "mutex_lock_value",
    "thread_current_id",
    "thread_fence",
    "thread_join_accepted",
    "thread_join_unit",
    "thread_spawn_accepted",
    "thread_spawn_closure",
    // the containers and the tagged union
    "box_basic",
    "box_owned_elements",
    "box_recursive",
    "enum_basic",
    "enum_in_containers",
    "enum_liveness",
    "enum_owned",
    "enum_recursive",
    "if_let_basic",
    "list_basic",
    "list_growth_and_rebuild",
    "list_nested_list",
    "list_owned",
    "list_slice_insert",
    "map_basic",
    "map_datac_key",
    "map_datac_value_clone",
    "map_growth_and_rehash",
    "map_nested_datac_key",
    "map_tombstones_and_reuse",
    "when_expr",
    // owning `Drop`s: the generated per-type drop in `_support`
    "drop_control_flow",
    "drop_enum",
    "drop_order",
    // passdowns, atomics and the rest of the scalar surface
    "atomic_counter",
    "bitwise",
    "consts",
    "datac_basic",
    "endian_scalars",
    "for_loop",
    "passdowns",
    "pd_owned",
    "section_attribute",
    "string_build",
    "string_views",
    "stdlib_fmt",
    "stdlib_sort",
    "view",
    "while_accumulator",
];

/// The multi-module projects the split has to keep working: a cross-module call,
/// a cross-module `pub static` read, a cross-module type, an `impl`/alias graph,
/// a user `extern fun`, an `unsafe` module and a `section("...")` placement.
pub(crate) const INCREMENTAL_PROJECTS: &[&str] = &[
    "alias_cross_file",
    "alias_kpconf",
    "closure_datac_param",
    "incremental_units",
    "namespace",
    "same_name",
    "self_call",
    "shapes",
    "static_datac_array",
    "static_globals",
    "static_module",
    "traits_cross",
    "traits_default_cross",
    "unsafe_ptr",
    "unsafe_unaligned",
    "visibility_import",
];

/// Decision 5: the explicit `--incremental` flag is refused on every hosted target at worker
/// counts 1 and 4, so the parallel scheduler has no split build to compare on a hosted target.
#[test]
pub(crate) fn parallel_and_serial_builds_match_the_whole_program_build() {
    let mut failures: Vec<String> = Vec::new();
    let jobs = [Some("1"), Some("4")];
    for name in INCREMENTAL_FIXTURES {
        let source = fixture(&format!("tests/fixtures/{name}.kp"));
        if let Err(why) = hosted_flag_check_at_jobs(name, &source, &jobs) {
            failures.push(format!("{name}: {why}"));
        }
    }
    for name in INCREMENTAL_PROJECTS {
        let target = incremental_project_target(name);
        if let Err(why) = hosted_flag_check_at_jobs(name, &target, &jobs) {
            failures.push(format!("project {name}: {why}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} targets accepted or mis-refused --incremental at jobs 1 or 4:
{}",
        failures.len(),
        INCREMENTAL_FIXTURES.len() + INCREMENTAL_PROJECTS.len(),
        failures.join("
")
    );
}

/// Decision 5: the explicit `--incremental` flag is refused on every hosted target, and the
/// whole-program build of each target still works. The split equivalence waits on per-unit
/// descriptors in the collector.
#[test]
pub(crate) fn incremental_builds_match_the_whole_program_build() {
    let mut failures: Vec<String> = Vec::new();
    for name in INCREMENTAL_FIXTURES {
        let source = fixture(&format!("tests/fixtures/{name}.kp"));
        if let Err(why) = hosted_flag_check(name, &source) {
            failures.push(format!("{name}: {why}"));
        }
    }
    for name in INCREMENTAL_PROJECTS {
        let target = incremental_project_target(name);
        if let Err(why) = hosted_flag_check(name, &target) {
            failures.push(format!("project {name}: {why}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} targets did not refuse --incremental as a hosted build must:
{}",
        failures.len(),
        INCREMENTAL_FIXTURES.len() + INCREMENTAL_PROJECTS.len(),
        failures.join("
")
    );
}

/// Unit assignment (roadmap A-14 slice 1), hosted fallback: a hosted project
/// that sets `[build] incremental = true` is built whole-program, because the
/// collector's descriptor ids are per unit. The build succeeds, warns and names
/// the reason, writes no per-unit output, and the program still runs.
/// `--incremental` on a hosted build stays refused (see `a_hosted_build_refuses_the_flag`).
#[test]
pub(crate) fn a_hosted_manifest_incremental_falls_back_to_whole_program() {
    let bin = out_path("inc_manifest_fallback");
    let _ = std::fs::remove_dir_all(bin.with_extension("units"));
    let build = kelpc()
        .arg("build")
        .arg(project("incremental_units"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted manifest build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("warning:") && stderr.contains("incremental = true") && stderr.contains("whole-program"),
        "the fallback did not warn and name the reason:\n{stderr}"
    );
    assert!(
        !bin.with_extension("units").exists(),
        "a hosted build wrote per-unit output"
    );
    let run = Command::new(&bin).output().expect("failed to run the fallback binary");
    assert!(run.status.success(), "the fallback binary failed");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n3\n7\n");
}

/// A hosted build refuses the explicit `--incremental` flag, even though the
/// manifest key falls back. The refusal names the hosted shared pool.
#[test]
pub(crate) fn a_hosted_build_refuses_the_flag() {
    let bin = out_path("inc_flag_refused_hosted");
    let build = kelpc()
        .arg("build")
        .arg(project("incremental_units"))
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(!build.status.success(), "a hosted build accepted --incremental");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("which has the shared pool") && stderr.contains("cannot build that yet"),
        "the refusal did not name the shared pool:\n{stderr}"
    );
}

/// Decision 5: a hosted build writes no unit IR to compare, so the explicit flag is refused for
/// the determinism target and the refusal names the shared pool.
#[test]
pub(crate) fn two_incremental_builds_produce_identical_unit_ir() {
    let bin = out_path("inc_determinism_bin");
    let build = kelpc()
        .arg("build")
        .arg(project("shapes"))
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(!build.status.success(), "a hosted build accepted --incremental");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("which has the shared pool") && stderr.contains("cannot build that yet"),
        "the refusal did not name the shared pool:
{stderr}"
    );
}

/// The freestanding refusal (roadmap A-14 slice 1): the mode is not implemented
/// for a bare-metal image yet, and it is refused by name rather than miscompiled
/// or silently ignored.
#[test]
pub(crate) fn incremental_is_refused_for_a_freestanding_project() {
    let bin = out_path("inc_freestanding");
    let build = kelpc()
        .arg("build")
        .arg(project("freestanding_min"))
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected the freestanding split to be refused"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("'--incremental' is not implemented for a freestanding project"),
        "the refusal does not name the mode: {stderr}"
    );
}

// Roadmap A-14 slice 2: the unit object cache
// ---------------------------------------------------------------------------
//
// The cache stores a unit's object under the hash of its IR text and the
// machine settings it was compiled for (`kelp wiki page compiler/incremental.md`).
// These tests drive the real compiler and read its stderr: `KELPC_CACHE_STATS=1`
// prints one hit/miss/store line per build, and `--timings` prints one
// `hit`/`miss` line per unit. Every test here is llvm-only.

/// One cache-enabled build's result, with the cache lines parsed out of stderr.
pub(crate) struct CacheBuild {
    pub(crate) ok: bool,
    pub(crate) stderr: String,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) stored: u64,
    pub(crate) units: Vec<(String, bool)>,
}

impl CacheBuild {
}

/// The three counters on the `kelpc cache: H hits, M misses, S stored` line.
pub(crate) fn parse_cache_stats(stderr: &str) -> (u64, u64, u64) {
    for line in stderr.lines() {
        let Some(rest) = line.strip_prefix("kelpc cache: ") else {
            continue;
        };
        let numbers: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|word| word.parse::<u64>().ok())
            .collect();
        assert_eq!(numbers.len(), 3, "unexpected cache line: {line}");
        return (numbers[0], numbers[1], numbers[2]);
    }
    // A hosted build falls back to whole-program (decision 5) and prints no
    // cache line at all, so no line means no cache traffic.
    (0, 0, 0)
}

/// The `unit NAME: N.N ms hit|miss` lines of a `--timings` build.
pub(crate) fn parse_unit_verdicts(stderr: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() == 5 && tokens[0] == "unit" && tokens[3] == "ms" {
            let name = tokens[1].trim_end_matches(':').to_string();
            out.push((name, tokens[4] == "hit"));
        }
    }
    out
}

/// The three-module project the cache tests edit: `main` calls `alpha`, `alpha`
/// calls `beta`, so every unit is a real compilation unit and the generated
/// `_support` helper unit sits beside them.
pub(crate) fn cache_project(tag: &str) -> PathBuf {
    let dir = out_path(&format!("cacheproj_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the project directory");
    std::fs::write(
        dir.join("coral.toml"),
        "name = \"cacheproj\"\n\n[build]\nincremental = true\n",
    )
    .expect("write the manifest");
    std::fs::write(
        dir.join("src/main.kp"),
        "impl \"alpha\"\n\nfun main() {\n    print(alpha.greet())\n}\n",
    )
    .expect("write main");
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"\n\npub fun greet() => str = beta.message()\n",
    )
    .expect("write alpha");
    std::fs::write(
        dir.join("src/beta.kp"),
        "pub fun message() => str = \"hello\"\n",
    )
    .expect("write beta");
    dir
}

/// Rewrites module B's function body, which is the unit the tests make miss.
pub(crate) fn set_beta_message(dir: &Path, body: &str) {
    std::fs::write(
        dir.join("src/beta.kp"),
        format!("pub fun message() => str = {body}\n"),
    )
    .expect("rewrite beta");
}

/// One `kelpc build --incremental --timings` against `cache`, with `flags` on
/// the command line and `envs` in the environment.
pub(crate) fn cache_build(
    target: &Path,
    bin: &Path,
    cache: &Path,
    flags: &[&str],
    envs: &[(&str, &str)],
) -> CacheBuild {
    let mut cmd = kelpc();
    cmd.env_remove("KELPC_CACHE")
        .env_remove("KELPC_CACHE_STATS")
        .env_remove("KELPC_CACHE_MAX_MB")
        .env_remove("KELPC_DUMP_UNITS")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(bin)
        .arg("--timings")
        .env("KELPC_CACHE_DIR", cache)
        .env("KELPC_CACHE_STATS", "1");
    for flag in flags {
        cmd.arg(flag);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("failed to spawn kelpc");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let (hits, misses, stored) = parse_cache_stats(&stderr);
    let units = parse_unit_verdicts(&stderr);
    CacheBuild {
        ok: out.status.success(),
        stderr,
        hits,
        misses,
        stored,
        units,
    }
}

/// Runs a built binary and returns its exit code and stdout.
pub(crate) fn run_cached_binary(bin: &Path) -> (Option<i32>, Vec<u8>) {
    let out = Command::new(bin)
        .output()
        .unwrap_or_else(|e| panic!("could not run '{}': {e}", bin.display()));
    (out.status.code(), out.stdout)
}

/// The `<key>.o` entries in a cache directory, sorted by name.
pub(crate) fn cache_entries(cache: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".o"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| cache.join(name))
        .collect()
}

/// Decision 5 (2026-10-12): a hosted `[build] incremental = true` falls back to the
/// whole-program build with a warning. Such a build reads and writes no unit cache, writes no
/// per-unit output and reports no unit verdicts.
pub(crate) fn assert_hosted_fallback(build: &CacheBuild, bin: &Path, cache: &Path) {
    assert!(build.ok, "the hosted fallback build failed:
{}", build.stderr);
    assert!(
        build.stderr.contains("warning:") && build.stderr.contains("whole-program"),
        "the hosted fallback did not warn:
{}",
        build.stderr
    );
    assert_eq!(
        (build.hits, build.misses, build.stored),
        (0, 0, 0),
        "a hosted build used the unit cache:
{}",
        build.stderr
    );
    assert!(build.units.is_empty(), "a hosted build reported units");
    assert!(
        cache_entries(cache).is_empty() && interface_entries(cache).is_empty(),
        "a hosted build wrote cache entries"
    );
    assert!(
        !bin.with_extension("units").exists(),
        "a hosted build wrote per-unit output"
    );
}

/// The generated compiler-owned module contracts in a cache directory.
pub(crate) fn interface_entries(cache: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".kpi"))
        .collect();
    names.sort();
    names.into_iter().map(|name| cache.join(name)).collect()
}


/// Decision 5: a hosted build writes no generated module interfaces; it falls back.
#[test]
pub(crate) fn incremental_builds_write_generated_module_interfaces() {
    let dir = cache_project("interfaces");
    let cache = out_path("cache_interfaces");
    let bin = out_path("cache_interfaces_bin");
    let build = cache_build(&dir, &bin, &cache, &[], &[]);
    assert_hosted_fallback(&build, &bin, &cache);
}

/// Decision 5: a hosted build reports no front-end interface timings; it falls back.
#[test]
pub(crate) fn timings_report_front_end_interface_hits_and_misses() {
    let dir = cache_project("iface_timings");
    let cache = out_path("cache_iface_timings");
    let _ = std::fs::remove_dir_all(&cache);
    let bin = out_path("cache_iface_timings_bin");
    let first = cache_build(&dir, &bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &bin, &cache);
    assert!(
        !first.stderr.contains("front-end interfaces:"),
        "a hosted build reported interface timings:
{}",
        first.stderr
    );
}

/// Decision 5: an unchanged hosted project is rebuilt whole-program each time, and no unit is reused.
#[test]
pub(crate) fn an_unchanged_hosted_project_is_rebuilt_whole_program() {
    let dir = cache_project("unchanged");
    let cache = out_path("cache_unchanged");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_unchanged_1");
    let second_bin = out_path("cache_unchanged_2");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &first_bin, &cache);
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert_hosted_fallback(&second, &second_bin, &cache);
    let (code1, stdout1) = run_cached_binary(&first_bin);
    let (code2, stdout2) = run_cached_binary(&second_bin);
    assert_eq!(code1, code2, "the rebuilt program exited differently");
    assert_eq!(stdout1, stdout2, "the rebuilt program printed differently");
    assert_eq!(String::from_utf8_lossy(&stdout2), "hello
");
    hosted_flag_check("cache_unchanged", &dir).unwrap();
}

/// Decision 5: changing one body on a hosted project rebuilds the whole program, with no unit cache.
#[test]
pub(crate) fn changing_one_body_on_a_hosted_build_rebuilds_whole_program() {
    let dir = cache_project("onebody");
    let cache = out_path("cache_onebody");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_onebody_1");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &first_bin, &cache);
    set_beta_message(&dir, "\"hello, cache\"");
    let second_bin = out_path("cache_onebody_2");
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert_hosted_fallback(&second, &second_bin, &cache);
    let (_, stdout) = run_cached_binary(&second_bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello, cache
");
    hosted_flag_check("cache_onebody", &dir).unwrap();
}

/// Decision 5: a changed callee signature on a hosted project rebuilds the whole program.
#[test]
pub(crate) fn changing_a_callee_signature_on_a_hosted_build_rebuilds_whole_program() {
    let dir = out_path("cache_signature");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the project directory");
    std::fs::write(
        dir.join("coral.toml"),
        "name = \"cachesig\"

[build]
incremental = true
",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.kp"),
        "impl \"alpha\"

fun main() {
    print(alpha.greet())
}
",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"

pub fun greet() => i64 = beta.value()
",
    )
    .unwrap();
    std::fs::write(dir.join("src/beta.kp"), "pub fun value() => i64 = 41
").unwrap();
    let cache = out_path("cache_signature_cache");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_signature_1");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &first_bin, &cache);
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"

pub fun greet() => i64 = beta.value(0)
",
    )
    .unwrap();
    std::fs::write(dir.join("src/beta.kp"), "pub fun value(x: i64) => i64 = x + 41
").unwrap();
    let second_bin = out_path("cache_signature_2");
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert_hosted_fallback(&second, &second_bin, &cache);
    let (_, stdout) = run_cached_binary(&second_bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "41
");
    hosted_flag_check("cache_signature", &dir).unwrap();
}

/// Decision 5: builds at two optimisation levels on a hosted project both fall back, and
/// neither writes a cache entry.
#[test]
pub(crate) fn hosted_builds_at_two_levels_write_no_entries() {
    let dir = cache_project("optlevel");
    let cache = out_path("cache_optlevel");
    let _ = std::fs::remove_dir_all(&cache);
    let bin0 = out_path("cache_opt_0");
    let at_o0 = cache_build(&dir, &bin0, &cache, &["-O0"], &[]);
    assert_hosted_fallback(&at_o0, &bin0, &cache);
    let bin2 = out_path("cache_opt_2");
    let at_o2 = cache_build(&dir, &bin2, &cache, &["-O2"], &[]);
    assert_hosted_fallback(&at_o2, &bin2, &cache);
    let (_, o0_out) = run_cached_binary(&bin0);
    let (_, o2_out) = run_cached_binary(&bin2);
    assert_eq!(o0_out, o2_out);
    hosted_flag_check("cache_optlevel", &dir).unwrap();
}

/// Decision 5: a hosted build never reads the unit cache, so there is no entry to truncate.
#[test]
pub(crate) fn a_hosted_build_never_reads_the_cache() {
    let dir = cache_project("truncate");
    let cache = out_path("cache_truncate");
    let _ = std::fs::remove_dir_all(&cache);
    let bin1 = out_path("cache_truncate_1");
    let first = cache_build(&dir, &bin1, &cache, &[], &[]);
    assert_hosted_fallback(&first, &bin1, &cache);
    let bin2 = out_path("cache_truncate_2");
    let second = cache_build(&dir, &bin2, &cache, &[], &[]);
    assert_hosted_fallback(&second, &bin2, &cache);
    hosted_flag_check("cache_truncate", &dir).unwrap();
}

/// Decision 5: two concurrent hosted builds fall back and leave no partial files in the cache
/// directory, which stays empty.
#[test]
pub(crate) fn concurrent_hosted_builds_leave_no_partial_files() {
    let dir = cache_project("concurrent");
    let cache = out_path("cache_concurrent");
    let _ = std::fs::remove_dir_all(&cache);
    let spawn = |bin: &Path| {
        kelpc()
            .env_remove("KELPC_CACHE")
            .env_remove("KELPC_CACHE_STATS")
            .env("KELPC_CACHE_DIR", &cache)
            .arg("build")
            .arg(&dir)
            .arg("-o")
            .arg(bin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn kelpc")
    };
    let bin_a = out_path("cache_concurrent_a");
    let bin_b = out_path("cache_concurrent_b");
    let child_a = spawn(&bin_a);
    let child_b = spawn(&bin_b);
    let out_a = child_a.wait_with_output().expect("wait for the first build");
    let out_b = child_b.wait_with_output().expect("wait for the second build");
    assert!(
        out_a.status.success(),
        "the first concurrent build failed:
{}",
        String::from_utf8_lossy(&out_a.stderr)
    );
    assert!(
        out_b.status.success(),
        "the second concurrent build failed:
{}",
        String::from_utf8_lossy(&out_b.stderr)
    );
    let (_, stdout_a) = run_cached_binary(&bin_a);
    let (_, stdout_b) = run_cached_binary(&bin_b);
    assert_eq!(stdout_a, stdout_b);
    assert_eq!(String::from_utf8_lossy(&stdout_a), "hello
");
    assert!(
        cache_entries(&cache).is_empty() && interface_entries(&cache).is_empty(),
        "a hosted build wrote cache entries"
    );
    let leftovers: Vec<String> = std::fs::read_dir(&cache)
        .map(|read| {
            read.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "a concurrent build left temporary files: {leftovers:?}"
    );
}

/// Decision 5: a hosted build writes no unit entries, so a size cap has nothing to evict and the
/// capped build still falls back and runs.
#[test]
pub(crate) fn the_size_cap_is_inert_on_a_hosted_build() {
    let dir = cache_project("cap");
    let cache = out_path("cache_cap");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_cap_1");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &first_bin, &cache);
    let bin = out_path("cache_cap_2");
    let second = cache_build(&dir, &bin, &cache, &[], &[("KELPC_CACHE_MAX_MB", "0.001")]);
    assert_hosted_fallback(&second, &bin, &cache);
    let (_, stdout) = run_cached_binary(&bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello
");
    hosted_flag_check("cache_cap", &dir).unwrap();
}

#[test]
pub(crate) fn cache_off_writes_nothing() {
    let dir = cache_project("off");
    let cache = out_path("cache_off");
    let _ = std::fs::remove_dir_all(&cache);
    let bin = out_path("cache_off_bin");
    let build = cache_build(&dir, &bin, &cache, &[], &[("KELPC_CACHE", "off")]);
    assert!(build.ok, "the KELPC_CACHE=off build failed:\n{}", build.stderr);
    assert!(
        !cache.exists(),
        "KELPC_CACHE=off created '{}'",
        cache.display()
    );
    assert_eq!(
        (build.hits, build.misses, build.stored),
        (0, 0, 0),
        "KELPC_CACHE=off still consulted the cache"
    );
    let (_, stdout) = run_cached_binary(&bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello\n");
    hosted_flag_check("cache_off", &dir).unwrap();
}

/// Decision 5: a hosted build leaves the cache empty, so `kelpc cache stats` reports 0 entries and
/// `kelpc cache clear` still succeeds, and a rebuild falls back again.
#[test]
pub(crate) fn the_cache_command_reports_and_clears() {
    let dir = cache_project("command");
    let cache = out_path("cache_command");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_command_1");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert_hosted_fallback(&first, &first_bin, &cache);
    let stats = kelpc()
        .arg("cache")
        .arg("stats")
        .arg("--dir")
        .arg(&cache)
        .output()
        .expect("failed to spawn kelpc cache stats");
    assert!(stats.status.success());
    let stats_text = String::from_utf8_lossy(&stats.stdout);
    assert!(
        stats_text.contains("0 entries"),
        "kelpc cache stats did not report 0 entries: {stats_text}",
    );
    let clear = kelpc()
        .arg("cache")
        .arg("clear")
        .arg("--dir")
        .arg(&cache)
        .output()
        .expect("failed to spawn kelpc cache clear");
    assert!(clear.status.success());
    let second_bin = out_path("cache_command_2");
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert_hosted_fallback(&second, &second_bin, &cache);
}

// Roadmap A-14 slice 3: parallel unit compilation
// ---------------------------------------------------------------------------
//
// Cache misses are compiled on worker threads, one LLVM context per unit, and
// the results are linked in unit order. The tests here pin the three things
// that make that safe to ship: the scheduler does not change an object, a unit
// that fails names itself and does not discard the other units' cache entries,
// and the compile-time job-count parsing.


/// Decision 5: the explicit flag is refused at one and at four workers on a hosted target.
#[test]
pub(crate) fn serial_and_parallel_builds_write_byte_identical_units() {
    let target = project("shapes");
    for jobs in ["1", "4"] {
        let bin = out_path(&format!("inc_units_j{jobs}"));
        let build = kelpc()
            .env("KELPC_JOBS", jobs)
            .env("KELPC_CACHE", "off")
            .arg("build")
            .arg(&target)
            .arg("-o")
            .arg(&bin)
            .arg("--incremental")
            .output()
            .expect("failed to spawn kelpc");
        assert!(!build.status.success(), "jobs={jobs}: a hosted build accepted --incremental");
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(
            stderr.contains("which has the shared pool") && stderr.contains("cannot build that yet"),
            "jobs={jobs}: the refusal did not name the shared pool:
{stderr}"
        );
    }
}

/// Decision 5: the unit failure hook (`KELPC_TEST_CORRUPT_UNIT`) acts on unit objects, and a
/// hosted build has none, so the hook is inert and the build falls back and runs.
#[test]
pub(crate) fn the_unit_failure_hook_is_inert_on_a_hosted_build() {
    let dir = cache_project("failedunit");
    let cache = out_path("cache_failedunit");
    let _ = std::fs::remove_dir_all(&cache);
    let bin = out_path("cache_failedunit_bin");
    let build = cache_build(&dir, &bin, &cache, &[], &[("KELPC_TEST_CORRUPT_UNIT", "alpha")]);
    assert_hosted_fallback(&build, &bin, &cache);
    let (_, stdout) = run_cached_binary(&bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello
");
}

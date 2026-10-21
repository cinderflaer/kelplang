use super::*;

// ---------------------------------------------------------------------------
// std/os: the target, process, environment and machine facts, and the
// temporary directories and files that clean up after themselves.
//
// Each fixture lives in `kelpc/tests/std_os/`, and its scratch paths are
// written as `__SCRATCH__`, which `os_source` replaces with a directory the test
// owns. The fixtures are not in `tests/fixtures/`, because a placeholder path
// is not a program the corpus sweep can build as written.
// ---------------------------------------------------------------------------

const SCRATCH: &str = "__SCRATCH__";

/// The test's own scratch directory, with its path resolved through symlinks,
/// because grants are matched against resolved paths.
fn os_scratch(name: &str) -> PathBuf {
    let dir = caps_scratch(name);
    std::fs::canonicalize(&dir).expect("could not resolve the scratch directory")
}

/// A copy of `kelpc/tests/std_os/<name>.kp` with the scratch path in place of
/// the placeholder, written into a directory of its own.
fn os_source(name: &str, scratch: &Path) -> PathBuf {
    let src = fixture(&format!("tests/std_os/{name}.kp"));
    let dir = out_path(&format!("std_os_src_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the fixture directory");
    let text = std::fs::read_to_string(&src)
        .expect("could not read the fixture")
        .replace(SCRATCH, &scratch.display().to_string());
    let target = dir.join(format!("{name}.kp"));
    std::fs::write(&target, text).expect("could not write the fixture copy");
    target
}

/// Builds `src`, returning the binary, or the compiler's stderr when the build
/// is refused.
fn os_build(name: &str, src: &Path) -> Result<PathBuf, String> {
    let bin = out_path(name);
    let out = kelpc()
        .arg("build")
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    if out.status.success() {
        Ok(bin)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// Runs a built binary with `TMPDIR` set to `tmp`, so its temporary paths land
/// in the test's scratch directory.
fn os_run(bin: &Path, tmp: &Path) -> std::process::Output {
    Command::new(bin)
        .env("TMPDIR", tmp)
        .output()
        .expect("failed to run the compiled binary")
}

/// Builds `name` from `kelpc/tests/std_os/`, runs it, and asserts stdout.
fn os_runs(name: &str, expected: &str) {
    let scratch = os_scratch(name);
    let src = os_source(name, &scratch);
    let bin = os_build(name, &src).unwrap_or_else(|err| panic!("'{name}' did not build:\n{err}"));
    let out = os_run(&bin, &scratch);
    assert!(
        out.status.success(),
        "'{name}' exited unsuccessfully:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "unexpected stdout for '{name}'");
}

/// Whether anything is left in `dir`.
fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false)
}

#[test]
pub(crate) fn os_target_and_process_facts_need_no_grant() {
    // The target OS and architecture, the process ids, the executable path,
    // the processor count and the memory size. None of them is gated.
    os_runs("facts", "true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
pub(crate) fn os_current_dir_reads_back_what_set_current_dir_wrote() {
    os_runs("cwd", "true\ntrue\n");
}

#[test]
pub(crate) fn os_environment_reads_and_writes_a_granted_name() {
    // A variable the program sets is present until it is unset. A name it
    // never set is `None`.
    os_runs("env", "true\none\ntrue\ntrue\n");
}

#[test]
pub(crate) fn os_machine_facts_read_under_the_machine_grant() {
    os_runs("machine", "true\ntrue\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
pub(crate) fn os_temp_dirs_and_files_are_removed_when_dropped() {
    // `scoped` drops its directory when it returns. The directory and file made
    // in `main` are dropped when `main` ends, and the paths are printed so the
    // test can check them after the process has gone.
    let scratch = os_scratch("temp");
    let src = os_source("temp", &scratch);
    let bin = os_build("temp", &src).expect("the temp fixture did not build");
    let out = os_run(&bin, &scratch);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(&lines[..6], &["false", "true", "true", "true", "true", "true"], "stdout:\n{stdout}");
    let (dir, file) = (Path::new(lines[6]), Path::new(lines[7]));
    assert!(!dir.exists(), "the temporary directory outlived its owner: {}", dir.display());
    assert!(!file.exists(), "the temporary file outlived its owner: {}", file.display());
    assert!(is_empty_dir(&scratch), "something was left in the scratch directory");
}

#[test]
pub(crate) fn os_temp_paths_are_removed_on_the_panic_path() {
    // A panic runs no destructor. The runtime removes the registered paths when
    // the process ends, so the directory and file made here are gone after
    // `panic("boom")` has exited the process.
    let scratch = os_scratch("panic_cleanup");
    let src = os_source("panic_cleanup", &scratch);
    let bin = os_build("panic_cleanup", &src).expect("the panic fixture did not build");
    let out = os_run(&bin, &scratch);
    assert!(!out.status.success(), "the panic fixture should not exit successfully");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("panic: boom"),
        "the panic message is missing:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "stdout:\n{stdout}");
    assert!(!Path::new(lines[0]).exists(), "the temporary directory survived the panic");
    assert!(!Path::new(lines[1]).exists(), "the temporary file survived the panic");
    assert!(is_empty_dir(&scratch), "something was left in the scratch directory");
}

#[test]
pub(crate) fn os_temp_paths_are_removed_on_exit() {
    // `os.exit` is the same exit path, with its own status.
    let scratch = os_scratch("exit_cleanup");
    let src = os_source("exit_cleanup", &scratch);
    let bin = os_build("exit_cleanup", &src).expect("the exit fixture did not build");
    let out = os_run(&bin, &scratch);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "stdout:\n{stdout}");
    assert!(!Path::new(lines[0]).exists(), "the temporary directory survived exit");
    assert!(!Path::new(lines[1]).exists(), "the temporary file survived exit");
    assert!(is_empty_dir(&scratch), "something was left in the scratch directory");
}

#[test]
pub(crate) fn std_sys_re_exports_the_target_facts_of_std_os() {
    os_runs("sys", "true\ntrue\n");
}

/// Builds the project `kelpc/tests/std_os/<name>/`, in which a module other
/// than the entry one makes the call. The entry module holds every domain by
/// default, so the refusal has to come from a module the entry does not grant.
fn os_project(name: &str) -> PathBuf {
    fixture(&format!("tests/std_os/{name}"))
}

#[test]
pub(crate) fn an_env_call_without_the_env_grant_is_refused_at_build() {
    let err = os_build("denied_env", &os_project("denied_env")).expect_err("the ungranted call built");
    assert!(
        err.contains("needs the 'env' capability"),
        "the refusal does not name the env capability:\n{err}"
    );
}

#[test]
pub(crate) fn a_machine_call_without_the_machine_grant_is_refused_at_build() {
    let err = os_build("denied_machine", &os_project("denied_machine")).expect_err("the ungranted call built");
    assert!(
        err.contains("needs the 'machine' capability"),
        "the refusal does not name the machine capability:\n{err}"
    );
}

#[test]
pub(crate) fn an_env_name_outside_the_grant_is_denied_cleanly_at_run() {
    // The name is checked before the lookup: HOME is not in the grant, so the
    // program stops with the capability message and reads nothing.
    let scratch = os_scratch("denied_env_runtime");
    let src = os_source("denied_env_runtime", &scratch);
    let bin = os_build("denied_env_runtime", &src).expect("the runtime-denied fixture did not build");
    let out = os_run(&bin, &scratch);
    assert!(!out.status.success(), "a name outside the grant was read");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is outside the env capability granted to module"),
        "the denial does not name the env capability:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
pub(crate) fn a_temp_dir_outside_the_fs_grant_is_denied_cleanly_at_run() {
    // The grant covers `keep/` under the scratch directory, and the temporary
    // directory is made directly under TMPDIR, so the check refuses it before
    // `mkdtemp` runs.
    let scratch = os_scratch("denied_fs_runtime");
    let src = os_source("denied_fs_runtime", &scratch);
    let bin = os_build("denied_fs_runtime", &src).expect("the fs-denied fixture did not build");
    let out = os_run(&bin, &scratch);
    assert!(!out.status.success(), "a temporary directory outside the grant was made");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is outside the fs capability granted to module"),
        "the denial does not name the fs capability:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(is_empty_dir(&scratch), "a directory was made outside the grant");
}

#[test]
pub(crate) fn a_temp_dir_outside_the_callers_fs_grant_is_denied_even_through_std_os() {
    // `util` holds `fs` over `/nonexistent/**` and passes that grant on to
    // `std/os`. The template is under TMPDIR, so the caller's own check refuses
    // it at the call, and nothing is made.
    let scratch = os_scratch("denied_fs_caller");
    let bin = os_build("denied_fs_caller", &os_project("denied_fs_caller")).expect("the caller fixture did not build");
    let out = os_run(&bin, &scratch);
    assert!(!out.status.success(), "a temporary directory was made outside the caller's grant");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is outside the fs capability granted to module 'util'"),
        "the denial does not name the caller's fs grant:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(is_empty_dir(&scratch), "a directory was made outside the grant");
}

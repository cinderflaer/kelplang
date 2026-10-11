//! Managed mode, containers: the program in `tests/managed/containers` (a List
//! of Strings, a Map of Strings to datacs, a closure that captures a Box)
//! prints the same output in a managed build as in a hosted one.
//!
//! The managed build needs the collector (`kelpc/gc`, WP1) linked into the
//! runtime and the managed lowering on the same branch. Until both are here the
//! managed comparison is `#[ignore]`d; the hosted half always runs, so the
//! fixture itself is checked.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed/containers")
}

/// A copy of the fixture in a scratch directory with its `mode` replaced, so
/// one source serves both builds. `tag` keeps concurrent tests apart.
fn copy_with_mode(mode: &str, tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpc-containers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create scratch dir");
    let src = fixture_dir();
    std::fs::copy(src.join("src/main.kp"), dir.join("src/main.kp")).expect("copy main.kp");
    let manifest = std::fs::read_to_string(src.join("coral.toml")).expect("read coral.toml");
    std::fs::write(dir.join("coral.toml"), manifest.replace("mode = \"managed\"", &format!("mode = \"{mode}\"")))
        .expect("write coral.toml");
    dir
}

fn build_and_run(dir: &Path, tag: &str) -> (String, String) {
    let bin = dir.join(format!("containers-{tag}"));
    let build: Output = kelpc()
        .arg("build")
        .arg(dir)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the {tag} build failed:\n{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(run.status.success(), "the {tag} binary failed: {}", String::from_utf8_lossy(&run.stderr));
    (String::from_utf8_lossy(&run.stdout).to_string(), String::from_utf8_lossy(&run.stderr).to_string())
}

#[test]
fn hosted_containers_program_prints_the_expected_output() {
    let dir = copy_with_mode("hosted", "hosted");
    let (stdout, _) = build_and_run(&dir, "hosted");
    let expected = std::fs::read_to_string(fixture_dir().join("expected.txt")).expect("read expected.txt");
    assert_eq!(stdout, expected);
}

#[test]
#[ignore = "needs the collector (kelpc/gc, WP1) linked by the managed runtime and the managed lowering on this branch"]
fn managed_containers_program_matches_the_hosted_output() {
    let hosted = copy_with_mode("hosted", "cmp-hosted");
    let managed = copy_with_mode("managed", "cmp-managed");
    let (hosted_out, _) = build_and_run(&hosted, "hosted");
    let (managed_out, _) = build_and_run(&managed, "managed");
    assert_eq!(managed_out, hosted_out);
}

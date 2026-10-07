//! A capability denial bakes its source location, its module name and its
//! domain into the generated C as a string literal, and a `.kp` path may hold
//! a backslash or a double quote, or a control byte such as a newline. Passed
//! through raw, the backslash is eaten as a C escape (the denial prints the path
//! with the byte dropped), the quote closes the literal (the generated C does
//! not compile at all), and the newline breaks the `#line` directive that points
//! `cc` at the `.kp` file, so the C backend fails to build the project. The C
//! backend must escape the bytes it interns, and the denial must still print the
//! path as it is on disk.
//!
//! The shape comes from the source *directory* name rather than the file name,
//! because that is how the fuzzing lane measured it and because a directory is
//! the untrusted part of the path the compiler is handed. A build with the C
//! backend succeeding is itself the `#line` case: without the escape, `cc`
//! refuses the broken directive. The denial the binary prints is the `at`
//! location case.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per shape: two tests running at once must not see each
/// other's project, and one shape's name is the input under test.
fn scratch(shape: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-deny-message-escaping-tests"),
    };
    let dir = root.join("deny-message-escaping").join(shape);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

// `pub`: `main` reaches `probe` through the module alias (A-13).
const UNIT: &str = "\
impl \"std/fs\"

pub fun probe().return: String = fs.read_text(\"/etc/hostname\")
";

// The grant is a prefix nothing under `/etc` can match, so the denial is
// unconditional and does not depend on the host's filesystem.
const MAIN: &str = "\
impl \"u\" caps(fs = [\"/kelp-deny-message-escaping-nowhere/**\"])

fun main() {
    print(u.probe())
}
";

/// Builds `dir/main.kp` with one backend, asserts the build succeeds, runs the
/// binary and returns the denial it printed on stderr. `c` is pinned through the
/// project's oracle switch; any other name is the default `llvm` backend.
///
/// `NIX_CC_USE_RESPONSE_FILE=0` is set because the Nix `cc` wrapper hands its
/// arguments to clang through a response file it quotes with bash `%q`, and
/// clang mis-parses that `$'...'` form when a control byte is in the path: it
/// fails to find the generated C before the `#line` this test is about is even
/// read. The variable makes the wrapper `exec` clang with the arguments
/// directly, so the test measures kelp's escaper rather than the wrapper's
/// quoting. A `cc` that is not the wrapper ignores it.
fn denial(dir: &Path, backend: &str, bin_name: &str) -> Vec<u8> {
    let bin = dir.join(bin_name);
    let mut cmd = kelpc();
    cmd.arg("build");
    cmd.env_remove("KELPC_BACKEND");
    cmd.env("NIX_CC_USE_RESPONSE_FILE", "0");
    if backend == "c" {
        cmd.env("KELPC_ORACLE", "c");
    } else {
        cmd.env_remove("KELPC_ORACLE");
    }
    let build = cmd
        .arg(dir.join("main.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the {backend} backend failed to build a project under '{}':\n{}",
        dir.display(),
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to run the compiled binary");
    assert_eq!(
        run.status.code(),
        Some(1),
        "the {backend} backend's denial for '{}' did not exit 1; stderr was:\n{}",
        dir.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    run.stderr
}

/// The same project under one shape of special byte in its directory name: the
/// C backend compiles it, the binary denies the call, and the denial names the
/// directory as it is on disk.
fn the_denial_names_the_source_directory(shape: &str) {
    let dir = scratch(shape);
    fs::write(dir.join("u.kp"), UNIT).expect("write u.kp");
    fs::write(dir.join("main.kp"), MAIN).expect("write main.kp");

    let c = denial(&dir, "c", "c.bin");

    let c_text = String::from_utf8_lossy(&c);
    assert!(
        c_text.contains("is outside the fs capability granted to module 'u'"),
        "the compiled binary did not deny the call:\n{c_text}"
    );
    let dir_text = dir.to_string_lossy();
    assert!(
        c_text.contains(dir_text.as_ref()),
        "the denial does not name the source directory as it is on \
         disk ('{dir_text}'):\n{c_text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn denial_message_survives_a_backslash_in_the_source_path() {
    the_denial_names_the_source_directory("esc\\bs");
}

#[test]
fn denial_message_survives_a_quote_in_the_source_path() {
    the_denial_names_the_source_directory("esc\"qt");
}

#[test]
fn denial_message_survives_a_newline_in_the_source_path() {
    the_denial_names_the_source_directory("esc\nnl");
}

#[test]
fn denial_message_survives_a_tab_in_the_source_path() {
    the_denial_names_the_source_directory("esc\ttb");
}

#[test]
fn denial_message_survives_a_carriage_return_in_the_source_path() {
    the_denial_names_the_source_directory("esc\rcr");
}

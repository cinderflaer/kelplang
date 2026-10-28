//! The camelCase names of the standard library's functions and methods, and the
//! one-cycle outdated aliases of their snake_case spellings (issue #375, phase 1
//! of the naming change in #368).
//!
//! A program that uses the camelCase name builds with `--deny-outdated` and
//! prints nothing. A program that still uses the snake_case name builds, runs to
//! the same output, and prints one `outdated:` finding per call that names the
//! camelCase spelling (the register in `kelpc/src/deprecated.rs`). The retired
//! spelling is refused under `--deny-outdated`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test, so two tests running at once never share one.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-std-camel-names-tests"),
    };
    let dir = root.join("std_camel_names").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the program's directory");
    }
    fs::write(&path, text).expect("write the program");
    path
}

fn build(path: &Path, out: &Path, deny: bool) -> Output {
    let mut cmd = kelpc();
    cmd.arg("build").arg(path).arg("-o").arg(out);
    if deny {
        cmd.arg("--deny-outdated");
    }
    cmd.output().expect("failed to spawn kelpc")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

const CAMEL: &str = "impl \"std/str\"\n\nfun main() {\n    \
     print(str.charCount(\"h\u{e9}llo\"))\n    \
     print(str.startsWith(\"abc\", \"a\"))\n    \
     print(str.indexOf(\"abc\", \"b\").isSome())\n}\n";

const SNAKE: &str = "impl \"std/str\"\n\nfun main() {\n    \
     print(str.char_count(\"h\u{e9}llo\"))\n    \
     print(str.starts_with(\"abc\", \"a\"))\n    \
     print(str.index_of(\"abc\", \"b\").is_some())\n}\n";

#[test]
fn the_camel_names_build_clean_and_pass_deny_outdated() {
    let dir = scratch("camel");
    let program = write(&dir, "src/new.kp", CAMEL);
    let out = dir.join("new");
    let result = build(&program, &out, true);
    assert!(result.status.success(), "the camelCase names must build under --deny-outdated:\n{}", stderr(&result));
    assert!(!stderr(&result).contains("outdated"), "{}", stderr(&result));

    let ran = Command::new(&out).output().expect("run the new program");
    assert!(ran.status.success());
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "5\ntrue\ntrue\n");
}

#[test]
fn the_snake_names_build_for_one_cycle_and_each_prints_a_finding() {
    let dir = scratch("snake");
    let program = write(&dir, "src/old.kp", SNAKE);
    let out = dir.join("old");
    let result = build(&program, &out, false);
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stderr(&result);
    assert_eq!(text.matches("outdated: ").count(), 4, "{text}");
    for name in ["`.char_count(`", "`.starts_with(`", "`.index_of(`", "`.is_some(`"] {
        assert!(text.contains(name), "no finding for {name}:\n{text}");
    }
    for line in text.lines().filter(|l| l.starts_with("outdated: ")) {
        assert!(line.contains("write `."), "a finding does not name the fix: {line}");
    }

    let ran = Command::new(&out).output().expect("run the old program");
    assert!(ran.status.success());
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "5\ntrue\ntrue\n", "the snake names keep their behaviour");

    let denied = build(&program, &out, true);
    assert!(!denied.status.success(), "--deny-outdated must refuse the snake names");
}

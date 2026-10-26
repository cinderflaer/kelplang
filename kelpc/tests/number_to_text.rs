//! `n.toText()` on a number, and the retirement of `fmt.int`, `fmt.uint`,
//! `fmt.float`, `str.from_u64`, `str.from_i64` and `str.from_f64` (issue #262).
//!
//! The retired names still build for one cycle and each call prints one
//! `outdated:` finding that names `toText` (the register in
//! `kelpc/src/deprecated.rs`). The method itself is checked by the fixture
//! `number_to_text.kp`; the cases here are the refusals, which only a real
//! invocation reaches: an argument on an integer, a float with two arguments,
//! and a float with decimals in a file that does not import `std/str`.

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
        None => std::env::temp_dir().join("kelpc-number-to-text-tests"),
    };
    let dir = root.join("number_to_text").join(name);
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

/// The stderr of a check (`kelpc check`), which stops before any code is made.
fn check(path: &Path) -> String {
    let out = kelpc().arg("check").arg(path).output().expect("failed to spawn kelpc");
    format!("{}{}", stderr(&out), String::from_utf8_lossy(&out.stdout))
}

#[test]
fn the_retired_names_build_for_one_cycle_and_each_prints_a_finding() {
    let dir = scratch("retired");
    let program = write(
        &dir,
        "src/old.kp",
        "impl \"std/fmt\"\nimpl \"std/str\"\n\nfun main() {\n    \
         print(fmt.int(-7))\n    print(fmt.uint(18446744073709551615))\n    \
         print(fmt.float(3.14159))\n    print(str.from_u64(5))\n    \
         print(str.from_i64(-7))\n    print(str.from_f64(3.14159, 2))\n}\n",
    );
    let out = dir.join("old");
    let result = build(&program, &out, false);
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stderr(&result);
    assert_eq!(text.matches("outdated: ").count(), 6, "{text}");
    for name in ["`fmt.int(`", "`fmt.uint(`", "`fmt.float(`", "`str.from_u64(`", "`str.from_i64(`", "`str.from_f64(`"] {
        assert!(text.contains(name), "no finding for {name}:\n{text}");
    }
    for line in text.lines().filter(|l| l.starts_with("outdated: ")) {
        assert!(line.contains("toText"), "a finding does not name the fix: {line}");
    }

    let ran = Command::new(&out).output().expect("run the old program");
    assert!(ran.status.success());
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "-7\n18446744073709551615\n3.14\n5\n-7\n3.14\n",
        "the retired names keep their text"
    );

    let denied = build(&program, &out, true);
    assert!(!denied.status.success(), "--deny-outdated must refuse the retired names");
}

#[test]
fn the_method_is_clean_and_passes_deny_outdated() {
    let dir = scratch("clean");
    let program = write(
        &dir,
        "src/new.kp",
        "impl \"std/str\"\n\nfun main() {\n    val n: i64 = -7\n    val f: f64 = 3.14159\n    \
         print(n.toText())\n    print(f.toText(2))\n    print(f.toText())\n}\n",
    );
    let out = dir.join("new");
    let result = build(&program, &out, true);
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(stderr(&result).is_empty(), "{}", stderr(&result));
}

#[test]
fn an_integer_takes_no_argument() {
    let dir = scratch("int-arg");
    let program = write(&dir, "src/a.kp", "fun main() {\n    val n: int = 3\n    print(n.toText(2))\n}\n");
    let text = check(&program);
    assert!(text.contains("'int.toText' takes no argument, found 1"), "{text}");
    assert!(text.contains("write 'n.toText()' with no argument"), "{text}");
}

#[test]
fn a_float_takes_zero_or_one_argument() {
    let dir = scratch("float-args");
    let program = write(
        &dir,
        "src/a.kp",
        "impl \"std/str\"\n\nfun main() {\n    val x: f64 = 1.5\n    print(x.toText(2, 3))\n}\n",
    );
    let text = check(&program);
    assert!(text.contains("'f64.toText' takes zero or one argument, found 2"), "{text}");
}

#[test]
fn a_float_with_decimals_needs_std_str() {
    let dir = scratch("float-no-str");
    let program = write(&dir, "src/a.kp", "fun main() {\n    val x: f64 = 1.5\n    print(x.toText(2))\n}\n");
    let text = check(&program);
    assert!(text.contains("does not import 'std/str'"), "{text}");
    assert!(text.contains("impl \"std/str\""), "the fix is not named:\n{text}");
}

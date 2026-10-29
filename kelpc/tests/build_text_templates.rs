//! Text is built with a template, not with `str.concat` runs or a `push_str` run
//! (issue #269, after the boilerplate plans #202 and #203).
//!
//! This is a source scan over the shipped standard library (`kelpc/std/`) and
//! the example programs (`kelpc/tests/fixtures/examples/`), with the vendored
//! `deps` copies left out. It fails on two shapes:
//!
//! * a `val`, `var` or `return` line whose value is a `str.concat(...)` call.
//!   The template `"${a}.part"` is the spelling for it. A `str.concat` inside
//!   `print(...)` or a match arm is not a value binding and is not checked here;
//! * a run of three or more `push_str` lines on one local that was declared in
//!   the same function as `var X = String.empty()` (or `String.from_str` or
//!   `String.with_capacity`). The run is built once as a template instead.
//!
//! A `push_str` on a parameter, a field, or a local not built from `String.*`
//! is not checked: it is not a value being built from scratch.
//!
//! `str.concat` itself stays valid and is not flagged. The fixtures outside the
//! examples (`kelpc/tests/fixtures/*.kp`, `kelpc/tests/projects`) test the
//! function and are not scanned.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_kelpc() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn kelp_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // The vendored copies of the polyp sources are not scanned.
            if path.file_name().is_some_and(|n| n == "deps") {
                continue;
            }
            kelp_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "kp") {
            out.push(path);
        }
    }
}

/// Files that stay on `str.concat` on purpose. A template needs `printf`, so a
/// freestanding build refuses it ("print is unavailable in a freestanding
/// build"). Each file below is reached by a freestanding example (`kernel0-*`,
/// `polyp-fb-*`, `polyp-v4-riscv64` and `polyp-v4-x86_64`), by its own tree or
/// through a shared `deps` link. `std/fmt` is imported by those examples too.
/// Paths are relative to `kelpc/`.
const FREESTANDING_KEPT: &[&str] = &[
    "std/fmt.kp",
    "tests/fixtures/examples/kernel0-eq/src/main.kp",
    "tests/fixtures/examples/kernel0-eq-x86_64/src/main.kp",
    "tests/fixtures/examples/polyp/core/core.kp",
    "tests/fixtures/examples/polyp/backends/gtk/gtk_backend.kp",
    "tests/fixtures/examples/polyp/backends/headless/headless_backend.kp",
    "tests/fixtures/examples/polyp/backends/soft/soft/ppm_platform.kp",
    "tests/fixtures/examples/polyp-todo/src/scenario.kp",
    "tests/fixtures/examples/polyp-v4-demo/src/app.kp",
];

/// The scanned files: the standard library and the example programs, less the
/// freestanding ones that stay on `str.concat`.
fn scanned_files() -> Vec<PathBuf> {
    let root = repo_kelpc();
    let mut files = Vec::new();
    kelp_files(&root.join("std"), &mut files);
    kelp_files(&root.join("tests/fixtures/examples"), &mut files);
    files.retain(|f| {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        !FREESTANDING_KEPT.contains(&rel.as_str())
    });
    files.sort();
    assert!(!files.is_empty(), "the scan found no Kelp files");
    files
}

/// The receiver of a `name.push_str(` line, if the line is one.
fn push_str_receiver(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let idx = t.find(".push_str(")?;
    let name = &t[..idx];
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some(name)
}

/// The local a `var X = String.empty()`-style line declares, if the line is one.
fn string_local(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix("var ")?;
    let (name, value) = rest.split_once('=')?;
    let name = name.trim();
    let value = value.trim_start();
    let built = ["String.empty(", "String.from_str(", "String.with_capacity("]
        .iter()
        .any(|p| value.starts_with(p));
    (built && !name.is_empty()).then_some(name)
}

/// Whether a `var` of `recv` built from `String.*` sits above line `at` in the
/// same function (the nearest column-zero or indented `fun` line bounds the
/// search).
fn built_in_same_fn(lines: &[&str], at: usize, recv: &str) -> bool {
    for k in (0..at).rev() {
        let t = lines[k].trim_start();
        if t.starts_with("fun ") || t.starts_with("pub fun ") {
            return false;
        }
        if string_local(lines[k]) == Some(recv) {
            return true;
        }
    }
    false
}

fn violations_in(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).expect("read a Kelp source");
    violations_of(&path.display().to_string(), &text)
}

fn violations_of(name: &str, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        let value_line = t.starts_with("val ") || t.starts_with("var ") || t.starts_with("return ");
        if value_line && t.contains("str.concat(") {
            found.push(format!("{name}:{}: str.concat in a value", i + 1));
        }
        if let Some(recv) = push_str_receiver(line) {
            let mut j = i;
            while j < lines.len() && push_str_receiver(lines[j]) == Some(recv) {
                j += 1;
            }
            if j - i >= 3 && built_in_same_fn(&lines, i, recv) {
                found.push(format!(
                    "{name}:{}: {} push_str lines on the built local '{}'",
                    i + 1,
                    j - i,
                    recv
                ));
            }
            i = j;
            continue;
        }
        i += 1;
    }
    found
}

#[test]
fn values_are_built_with_templates_not_str_concat() {
    let mut found = Vec::new();
    for path in scanned_files() {
        found.extend(violations_in(&path));
    }
    assert!(
        found.is_empty(),
        "{} site(s) still build text with str.concat or a push_str run; write a template instead:\n{}",
        found.len(),
        found.join("\n"),
    );
}

#[test]
fn the_scan_flags_a_concat_value_and_a_built_push_str_run() {
    let found = violations_of(
        "sample.kp",
        "fun f(a: &str) => String {\n    var out = String.empty()\n    out.push_str(\"a\")\n    out.push_str(a)\n    out.push_str(\"c\")\n    val part = str.concat(a, \".part\")\n    return str.concat(part, \"x\")\n}\n\nfun g(s: &String) {\n    s.push_str(\"a\")\n    s.push_str(\"b\")\n    s.push_str(\"c\")\n}\n",
    );
    assert_eq!(found.len(), 3, "unexpected findings: {found:?}");
    assert!(found[0].contains(":3:") && found[0].contains("built local 'out'"));
    assert!(found[1].contains(":6:") && found[1].contains("str.concat"));
    assert!(found[2].contains(":7:") && found[2].contains("str.concat"));
}

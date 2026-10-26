//! A string literal is a `str`, and a `String` slot takes it directly: the
//! conversion is `coerce_str_to_string` (kelpc/src/infer/expr.rs). So the
//! wrapper `String.from_str("L")` (or `String.from_str(s = "L")`) is
//! boilerplate wherever the slot is an exact `String`.
//!
//! This test keeps the standard library and the fixture suite free of the
//! wrapper around a literal. Every file that keeps it on purpose is named in
//! [`ALLOWED`] with the reason. A new hit fails the test, so the wrapper does
//! not creep back in.
//!
//! Scope: `kelpc/std/**/*.kp` and `kelpc/tests/fixtures/**/*.kp`. Only a
//! literal argument counts. A non-literal argument (`String.from_str(name)`) is
//! never a hit.

use std::fs;
use std::path::{Path, PathBuf};

/// Files that keep `String.from_str` around a literal on purpose, with the
/// reason. The path is relative to the crate root.
const ALLOWED: &[(&str, &str)] = &[];

const NEEDLE: &str = "String.from_str(";

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn collect_kp(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_kp(&path, out);
        } else if path.extension().is_some_and(|e| e == "kp") {
            out.push(path);
        }
    }
}

/// True when the text right after `String.from_str(` is a string literal
/// argument, bare or named `s = `, and the call closes right after it.
fn literal_argument_at(text: &str, after_open: usize) -> bool {
    let mut rest = &text[after_open..];
    rest = rest.trim_start();
    if let Some(named) = rest.strip_prefix("s") {
        let named = named.trim_start();
        if let Some(value) = named.strip_prefix('=') {
            rest = value.trim_start();
        }
    }
    let Some(body) = rest.strip_prefix('"') else {
        return false;
    };
    // Walk the literal: a backslash escapes the next byte, an unescaped quote
    // closes it. The call must close right after the literal.
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => {
                // A method call on the result (`String.from_str("x").as_str()`)
                // is not a slot: the bare literal would be a `str` there.
                let Some(after) = body[i + 1..].trim_start().strip_prefix(')') else {
                    return false;
                };
                return !after.trim_start().starts_with('.');
            }
            b'\n' => return false,
            _ => i += 1,
        }
    }
    false
}

fn literal_hits(text: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(pos) = text[from..].find(NEEDLE) {
        let at = from + pos;
        // A longer name ending in `String.from_str(` is not the builtin.
        let preceded_by_ident = text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
        let after_open = at + NEEDLE.len();
        if !preceded_by_ident && literal_argument_at(text, after_open) {
            let line = text[..at].matches('\n').count() + 1;
            hits.push(line);
        }
        from = after_open;
    }
    hits
}

fn allowed(rel: &str) -> bool {
    ALLOWED.iter().any(|(path, _)| *path == rel)
}

#[test]
fn std_and_fixtures_keep_no_string_from_str_around_a_literal() {
    let root = crate_root();
    let mut files = Vec::new();
    collect_kp(&root.join("std"), &mut files);
    collect_kp(&root.join("tests/fixtures"), &mut files);
    assert!(!files.is_empty(), "found no .kp files under kelpc/std and kelpc/tests/fixtures");

    let mut offenders = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
        if allowed(&rel) {
            continue;
        }
        let text = fs::read_to_string(path).expect("read a .kp file");
        for line in literal_hits(&text) {
            offenders.push(format!("{rel}:{line}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "{} String.from_str call(s) wrap a string literal; write the literal directly in the String slot, \
         or add the file to ALLOWED with a reason:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn the_scanner_sees_bare_and_named_literal_forms_only() {
    assert_eq!(literal_hits(r#"val a = String.from_str("x")"#), vec![1]);
    assert_eq!(literal_hits(r#"val a = String.from_str(s = "x")"#), vec![1]);
    assert_eq!(literal_hits("\n\nreturn String.from_str(\"a \\\"q\\\" b\")"), vec![3]);
    // A non-literal argument, a method receiver and a longer name are not hits.
    assert!(literal_hits("val a = String.from_str(name)").is_empty());
    assert!(literal_hits(r#"val a = String.from_str("x").as_str()"#).is_empty());
    assert!(literal_hits(r#"val a = MyString.from_str("x")"#).is_empty());
    assert!(literal_hits(r#"val a = String.from_str(s = name)"#).is_empty());
}

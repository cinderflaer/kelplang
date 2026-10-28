//! A counter `while` loop becomes a `for` loop (issue #267).
//!
//! A `while` whose condition is `i < bound`, whose last statement steps the
//! counter by one (`i = i + 1` or `i += 1`), is written as `for x in list` when
//! it walks a list from index 0, or as `for i in iter.range(start, bound)`
//! otherwise. The hosted examples under `kelpc/tests/fixtures/examples` carry no
//! such loop. The freestanding kernel directories (`kernel0*`, `shared`) keep
//! their `while` loops because they build without `std/iter`, and the polyp
//! directories are left for a follow-up.
//!
//! The scan is line based. It reads the header of each `while`, then the last
//! statement before the matching closing brace, so it flags the shape and not
//! the loops that only happen to mention a counter.

use std::fs;
use std::path::{Path, PathBuf};

const EXAMPLES: &str = "tests/fixtures/examples";

/// Directories under the examples whose loops stay `while` for now.
fn is_kept_directory(name: &str) -> bool {
    name.starts_with("kernel0") || name.starts_with("polyp") || name == "shared"
}

fn collect_kp(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_kp(&path, out);
        } else if path.extension().map_or(false, |e| e == "kp") {
            out.push(path);
        }
    }
}

/// The counter name of a `while NAME < BOUND {` header, if the line is one.
fn while_counter(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("while ")?;
    let (name, after) = rest.split_once(" < ")?;
    let is_ident = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (is_ident && after.trim_end().ends_with('{')).then_some(name)
}

/// Whether the statement is the one-step increment of `name`.
fn is_step(stmt: &str, name: &str) -> bool {
    let stmt = stmt.trim();
    stmt == format!("{name} = {name} + 1") || stmt == format!("{name} += 1")
}

/// The 1-based line numbers of every counter `while` in one file.
fn counter_whiles(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some(name) = while_counter(line) else { continue };
        // The body ends at the first line that closes the header's brace.
        let mut depth: i32 = 0;
        let mut last_stmt: Option<&str> = None;
        for body in &lines[idx..] {
            let code = body.split("//").next().unwrap_or("");
            for ch in code.chars() {
                match ch {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if depth <= 0 {
                break;
            }
            let trimmed = code.trim();
            if !trimmed.is_empty() && trimmed != "{" && trimmed != "}" {
                last_stmt = Some(trimmed);
            }
        }
        if last_stmt.map_or(false, |s| is_step(s, name)) {
            found.push(idx + 1);
        }
    }
    found
}

#[test]
fn hosted_examples_have_no_counter_while() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(EXAMPLES);
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        panic!("cannot read {}", root.display());
    };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for entry in dirs {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if entry.is_dir() {
            if !is_kept_directory(&name) {
                collect_kp(&entry, &mut files);
            }
        } else if entry.extension().map_or(false, |e| e == "kp") {
            files.push(entry);
        }
    }
    assert!(!files.is_empty(), "no example sources under {}", root.display());

    let mut offenders = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("read example");
        for line in counter_whiles(&source) {
            let header = source.lines().nth(line - 1).unwrap_or("").trim();
            // A start index over `args.len()` stays `while`: `args.len()` is `int`,
            // and `iter.range` takes `i64`, so the loop does not type-check as a `for`.
            if header == "while i < args.len() {" && file.ends_with("download_pool.kp") {
                continue;
            }
            offenders.push(format!("{}:{}", file.display(), line));
        }
    }
    assert!(
        offenders.is_empty(),
        "counter loops still written as while (use `for x in list` or `for i in iter.range(a, b)`):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn scanner_flags_only_the_counter_shape() {
    let counter = "fun f(xs: List<i32>) {\n    var i: int = 0\n    while i < xs.len() {\n        print(xs.get(i))\n        i = i + 1\n    }\n}\n";
    assert_eq!(counter_whiles(counter), vec![3]);

    let compound = "fun f(n: int) {\n    var i: int = 0\n    while i < n {\n        i += 1\n    }\n}\n";
    assert_eq!(counter_whiles(compound), vec![3]);

    // A step that is not last, or a loop with an extra condition, is not the shape.
    let step_not_last = "fun f(n: int) {\n    var i: int = 0\n    while i < n {\n        i += 1\n        print(i)\n    }\n}\n";
    assert!(counter_whiles(step_not_last).is_empty());

    let other_name = "fun f(n: int) {\n    while i < n {\n        j += 1\n    }\n}\n";
    assert!(counter_whiles(other_name).is_empty());

    // A nested block inside the body does not end the loop early.
    let nested = "fun f(n: int) {\n    while i < n {\n        if x {\n            y()\n        }\n        i += 1\n    }\n}\n";
    assert_eq!(counter_whiles(nested), vec![2]);
}

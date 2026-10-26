//! The example folders share one copy of each `.kp` file they all use.
//!
//! An example's `src/` holds either its own file or a symbolic link into
//! `tests/fixtures/examples/shared/`, never a second copy of a shared file.
//! The compiler reads a link as the file it names, so the module id and every
//! diagnostic path stay the same (issue #305).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/examples")
}

/// Every `.kp` under an example's `src/`, except the fetched `deps/` tree.
fn example_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(examples()).expect("reading tests/fixtures/examples") {
        let dir = entry.expect("reading an examples entry").path();
        if !dir.is_dir() || dir.file_name() == Some("shared".as_ref()) {
            continue;
        }
        let src = dir.join("src");
        if src.is_dir() {
            collect(&src, &mut out);
        }
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("reading an example's src") {
        let path = entry.expect("reading a src entry").path();
        if path.file_name() == Some("deps".as_ref()) {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            collect(&path, out);
        } else if path.extension() == Some("kp".as_ref()) {
            out.push(path);
        }
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .expect("stat an example source")
        .file_type()
        .is_symlink()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(examples())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn no_example_source_is_a_copy_of_another() {
    let mut by_bytes: BTreeMap<Vec<u8>, Vec<String>> = BTreeMap::new();
    for path in example_sources() {
        if is_link(&path) {
            continue;
        }
        let bytes = fs::read(&path).expect("reading an example source");
        by_bytes.entry(bytes).or_default().push(relative(&path));
    }
    let copies: Vec<String> = by_bytes
        .values()
        .filter(|group| group.len() > 1)
        .map(|group| group.join(", "))
        .collect();
    assert!(
        copies.is_empty(),
        "{} identical copies of a shared file; link each one to examples/shared instead:\n{}",
        copies.len(),
        copies.join("\n")
    );
}

#[test]
fn every_shared_file_is_linked_and_every_link_points_into_shared() {
    let shared = examples().join("shared");
    assert!(shared.is_dir(), "missing {}", relative(&shared));
    let shared_canon = fs::canonicalize(&shared).expect("canonicalizing examples/shared");

    let mut problems = Vec::new();
    let mut linked: BTreeSet<PathBuf> = BTreeSet::new();
    for path in example_sources() {
        if !is_link(&path) {
            continue;
        }
        match fs::canonicalize(&path) {
            Ok(target) if target.starts_with(&shared_canon) => {
                linked.insert(target);
            }
            Ok(target) => problems.push(format!(
                "{} links to {}, which is outside examples/shared",
                relative(&path),
                target.display()
            )),
            Err(e) => problems.push(format!("{} does not resolve: {e}", relative(&path))),
        }
    }

    let mut unlinked = Vec::new();
    for entry in fs::read_dir(&shared).expect("reading examples/shared") {
        let path = entry.expect("reading a shared entry").path();
        if path.extension() != Some("kp".as_ref()) {
            continue;
        }
        let canon = fs::canonicalize(&path).expect("canonicalizing a shared file");
        if !linked.contains(&canon) {
            unlinked.push(relative(&path));
        }
    }

    assert!(
        problems.is_empty() && unlinked.is_empty(),
        "shared copies are not wired up:\n{}\nshared files with no link:\n{}",
        problems.join("\n"),
        unlinked.join("\n")
    );
}

/// Every example module that opens with a comment opens with a `//!` module
/// header: the doc form that `kelpc doc` reads as the module's introduction,
/// not a `//` note that the lint counts as a comment run. The header is
/// accepted by the formatter, and `kelpc doc` renders its first line into the
/// module's page (issue #306).
#[test]
fn every_example_module_opens_with_a_module_header() {
    let mut all = Vec::new();
    collect_every_kp(&examples(), &mut all);
    all.sort();
    let mut problems = Vec::new();
    for path in all {
        let text = fs::read_to_string(&path).expect("reading an example source");
        let first = text.lines().next().unwrap_or("");
        if !first.starts_with("//") {
            // A file with no header at all has nothing to convert.
            continue;
        }
        let Some(intro) = first.strip_prefix("//!") else {
            problems.push(format!("{}: first line is a '//' comment, not a '//!' header", relative(&path)));
            continue;
        };
        let intro = intro.trim();

        // `fmt` may still reflow other parts of a file that were never
        // formatted, so the check is that it leaves the header alone: format a
        // copy, then compare the leading `//!` block.
        let copy = std::env::temp_dir()
            .join(format!("kelpc-module-header-fmt-{}.kp", std::process::id()));
        fs::write(&copy, &text).expect("writing a copy for kelpc fmt");
        let fmt = std::process::Command::new(env!("CARGO_BIN_EXE_kelpc"))
            .args(["fmt", "-w"])
            .arg(&copy)
            .output()
            .expect("spawn kelpc fmt");
        let formatted = fs::read_to_string(&copy).unwrap_or_default();
        let _ = fs::remove_file(&copy);
        if !fmt.status.success() {
            problems.push(format!(
                "{}: kelpc fmt refuses it: {}",
                relative(&path),
                String::from_utf8_lossy(&fmt.stderr).trim()
            ));
        } else if header_of(&formatted) != header_of(&text) {
            problems.push(format!("{}: kelpc fmt changes the module header", relative(&path)));
        }

        let out = std::env::temp_dir().join(format!("kelpc-module-header-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        let doc = std::process::Command::new(env!("CARGO_BIN_EXE_kelpc"))
            .args(["doc"])
            .arg(&path)
            .arg("-o")
            .arg(&out)
            .output()
            .expect("spawn kelpc doc");
        if !doc.status.success() {
            problems.push(format!(
                "{}: kelpc doc refuses it: {}",
                relative(&path),
                String::from_utf8_lossy(&doc.stderr).trim()
            ));
        } else {
            let mut rendered = String::new();
            for entry in fs::read_dir(&out).expect("reading the doc output") {
                let p = entry.expect("reading a doc page").path();
                rendered.push_str(&fs::read_to_string(&p).unwrap_or_default());
            }
            if !intro.is_empty() && !rendered.contains(intro) {
                problems.push(format!(
                    "{}: kelpc doc does not render the header's first line",
                    relative(&path)
                ));
            }
        }
        let _ = fs::remove_dir_all(&out);
    }
    assert!(
        problems.is_empty(),
        "{} example modules without a usable module header:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Every `.kp` under `dir`, including the shared library trees such as
/// `polyp/`, skipping fetched and built output and not descending into links.
fn collect_every_kp(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("reading an example directory") {
        let path = entry.expect("reading an example entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "deps" || name == "build" || name == "target" {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            collect_every_kp(&path, out);
        } else if path.extension() == Some("kp".as_ref()) {
            out.push(path);
        }
    }
}

/// The leading `//!` lines of a source file.
fn header_of(text: &str) -> Vec<&str> {
    text.lines().take_while(|l| l.starts_with("//!")).collect()
}

//! Guard for issue #295: a std method never only forwards to a free function of
//! the same name. The method holds the body, written in place, and the free
//! function is removed where nothing else calls it.
//!
//! The scan reads every shipped `std/*.kp` file. A method is a `fun` whose name
//! is qualified by a receiver type. Its body is either the text after the last
//! top-level ` = ` on the signature line, or the one statement of a block body
//! on the next line. The method is a forwarder when that body is a call to a
//! function of the same name whose first argument is `self`.

use std::fs;
use std::path::Path;

/// The name of a method declared on a line like `fun Option<T>.map<U>(...)`,
/// or `None` when the line declares a plain function.
fn method_name(signature: &str) -> Option<String> {
    let rest = signature.strip_prefix("fun ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let head = match rest.find('(') {
        Some(at) => &rest[..at],
        None => rest,
    };
    // Drop generic arguments on the receiver and on the method name.
    let mut flat = String::new();
    let mut depth = 0;
    for c in head.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => flat.push(c),
            _ => {}
        }
    }
    let (receiver, name) = flat.rsplit_once('.')?;
    if receiver.trim().is_empty() {
        return None;
    }
    Some(name.trim().to_string())
}

/// The text after the last ` = ` of a signature line that has one, which is
/// the body of an expression-bodied function. `=>` is the return arrow, not a
/// body, and is not matched.
fn expression_body(signature: &str) -> Option<&str> {
    signature.rsplit_once(" = ").map(|(_, body)| body.trim())
}

/// Whether a method declared on `signature` forwards to a free function of its
/// own name. `next` is the line after the signature, which a block body puts
/// its one statement on.
fn forwards_to_same_name(signature: &str, next: Option<&str>, after: Option<&str>) -> bool {
    let Some(name) = method_name(signature) else {
        return false;
    };
    let name = name.as_str();
    let call_of_self = |body: &str| {
        body.strip_prefix(name)
            .and_then(|tail| tail.strip_prefix('('))
            .is_some_and(|args| args.trim_start().starts_with("self"))
    };
    if let Some(body) = expression_body(signature) {
        return call_of_self(body);
    }
    if !signature.trim_end().ends_with('{') {
        return false;
    }
    let statement = next.map(str::trim).unwrap_or("");
    let closes = after.map(str::trim) == Some("}");
    closes && call_of_self(statement)
}

/// Every `file:line` in the shipped std that forwards to a same-name function.
fn forwarders(source: &str, file: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with("fun ") {
            continue;
        }
        let next = lines.get(i + 1).copied();
        let after = lines.get(i + 2).copied();
        if forwards_to_same_name(line, next, after) {
            found.push(format!("{file}:{}", i + 1));
        }
    }
    found
}

#[test]
fn the_scan_finds_a_forwarder_in_either_body_form() {
    assert!(forwards_to_same_name("fun Option<T>.is_some() => bool = is_some(self)", None, None));
    assert!(forwards_to_same_name(
        "fun Option<T>.each(f: closure fun(&T)) {",
        Some("    each(self, f)"),
        Some("}"),
    ));
    assert!(!forwards_to_same_name("fun Option<T>.is_some() => bool = when self {", None, None));
    assert!(!forwards_to_same_name("fun Frame.addr() => usize = addr", None, None));
    assert!(!forwards_to_same_name("fun Pool.len() => int = idle.len()", None, None));
    assert!(!forwards_to_same_name("fun is_some<T>(o: &Option<T>) => bool = o.is_some()", None, None));
}

#[test]
fn no_std_method_only_forwards_to_a_free_function_of_the_same_name() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
    let mut entries: Vec<_> = fs::read_dir(&root)
        .expect("kelpc/std is readable")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "kp"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "no std sources found under {}", root.display());

    let mut hits = Vec::new();
    for path in &entries {
        let source = fs::read_to_string(path).expect("std source is readable");
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        hits.extend(forwarders(&source, &file));
    }
    assert!(
        hits.is_empty(),
        "std methods that only forward to a free function of the same name (issue #295); \
         write the body in the method and drop the wrapper: {hits:?}"
    );
}

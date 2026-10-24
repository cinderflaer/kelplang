//! Guard for issue #289: a function whose whole body is one `unsafe` block is
//! written `unsafe fun`, so the block and its closing brace are not repeated.
//!
//! Only the sites converted under the issue are listed. The other whole-body
//! candidates keep their block: each has a Kelp caller outside any unsafe
//! context, and the `unsafe fun` form would make that caller an error (see the
//! issue's design comment).

use std::fs;
use std::path::Path;

const CONVERTED: &[(&str, &str)] = &[
    ("tests/fixtures/examples/kernel0-trap/src/uart.kp", "putc"),
    ("tests/fixtures/examples/kernel0-x86_64-idt/src/idt.kp", "set_gs_base"),
    ("tests/fixtures/examples/kernel0-stack/src/trap.kp", "resume_pc"),
];

#[test]
fn a_whole_body_unsafe_block_is_written_as_an_unsafe_fun() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (rel, name) in CONVERTED {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let lines: Vec<&str> = text.lines().collect();
        let plain = format!("fun {name}(");
        let header = lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                t.starts_with(&format!("unsafe {plain}")) || t.starts_with(&format!("pub unsafe {plain}"))
            })
            .unwrap_or_else(|| panic!("{rel}: `{name}` is not declared `unsafe fun`"));
        assert!(
            !lines.iter().any(|l| {
                let t = l.trim_start();
                t.starts_with(&plain) || t.starts_with(&format!("pub {plain}"))
            }),
            "{rel}: `{name}` is still declared without `unsafe`"
        );
        let first_body = lines[header + 1].trim();
        assert!(
            !first_body.starts_with("unsafe {") && first_body != "unsafe",
            "{rel}: the body of `{name}` still opens with an `unsafe` block"
        );
    }
}

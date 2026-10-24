//! Guard for issue #291: a `datac` pair that compares field by field is written
//! `derive(eq, hash)` above the type, not as a hand-written `eq` and `hash`.
//!
//! Only the converted pairs are listed. `Blob` in the same programs keeps its
//! hand-written `eq`: its `Bytes` field is walked byte by byte, which derive
//! does not write for it.

use std::fs;
use std::path::Path;

const CONVERTED: &[(&str, &str)] = &[
    ("tests/fixtures/examples/kernel0-eq/src/main.kp", "Pair"),
    ("tests/fixtures/examples/kernel0-eq-x86_64/src/main.kp", "Pair"),
];

const KEPT: &[(&str, &str)] = &[
    ("tests/fixtures/examples/kernel0-eq/src/main.kp", "Blob"),
    ("tests/fixtures/examples/kernel0-eq-x86_64/src/main.kp", "Blob"),
];

#[test]
fn a_field_wise_pair_is_derived_not_written_by_hand() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (rel, name) in CONVERTED {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let lines: Vec<&str> = text.lines().collect();
        let datac = lines
            .iter()
            .position(|l| l.starts_with(&format!("datac {name}(")))
            .unwrap_or_else(|| panic!("{rel}: no `datac {name}(` line"));
        assert!(
            datac > 0 && lines[datac - 1].trim() == "derive(eq, hash)",
            "{rel}: `{name}` is not declared with `derive(eq, hash)` on the line above it"
        );
        for method in ["eq", "hash"] {
            let prefix = format!("fun {name}.{method}");
            assert!(
                !lines.iter().any(|l| l.starts_with(&prefix)),
                "{rel}: `{name}.{method}` is still written by hand"
            );
        }
    }
    for (rel, name) in KEPT {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(
            text.lines().any(|l| l.starts_with(&format!("fun {name}.eq("))),
            "{rel}: `{name}` lost its hand-written `eq`, which must stay"
        );
    }
}

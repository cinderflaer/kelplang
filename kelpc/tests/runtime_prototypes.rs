//! The C spelling of every runtime export (stage 1 WP6, the C probe).
//!
//! The Rust runtime (`kelp-rt`, WP0 onward) owns the list of its exported C
//! functions in `kelp-rt/src/abi.rs` as `PROTOTYPES`, a table of
//! `("name", "C prototype")` pairs. The LLVM backend's C prelude is spelled from
//! that table, so a prototype that does not compile against the archive is a
//! prelude that would not compile either. This probe writes one C file that
//! declares every prototype verbatim, takes the address of every name (so each
//! one must be declared and defined), and links it against the archive.
//!
//! Only the table is read, as text, because `kelpc` cannot depend on the
//! `kelp-rt` crate from a test without a change to `kelpc/Cargo.toml`, which
//! WP0 owns. The parser accepts the table shape `PROTOTYPES` is written in: a
//! string literal for the name followed by one for the prototype, in order,
//! with no escaped quotes inside.
//!
//! The probe needs the archive that WP0's `build.rs` writes to `OUT_DIR`, and
//! `cc`. It is ignored until WP0 lands; run it with
//! `cargo test --test runtime_prototypes -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The C declarations the prelude needs before any prototype: the three
/// container layouts from `kelp-rt/src/abi.rs` (`LAYOUTS`, plan section 3.2).
const LAYOUT_TYPEDEFS: &str = "\
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
typedef struct kelp_Bytes { uint8_t *data; size_t len; size_t cap; } kelp_Bytes;
typedef struct kelp_String { uint8_t *data; size_t len; size_t cap; } kelp_String;
typedef struct kelp_View { const char *ptr; size_t len; } kelp_View;
";

fn abi_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../kelp-rt/src/abi.rs")
}

fn archive() -> PathBuf {
    match option_env!("OUT_DIR") {
        Some(dir) => PathBuf::from(dir).join("libkelp_rt.a"),
        None => panic!("OUT_DIR is not set for this test build"),
    }
}

/// Every string literal inside the `PROTOTYPES` table, in order, as
/// `(name, prototype)` pairs.
fn prototypes() -> Vec<(String, String)> {
    let src = fs::read_to_string(abi_source())
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", abi_source().display()));
    let start = src
        .find("pub const PROTOTYPES")
        .expect("kelp-rt/src/abi.rs has no `pub const PROTOTYPES` table");
    let body = &src[start..];
    let end = body.find("];").expect("PROTOTYPES table is not closed with `];`");
    let mut literals = Vec::new();
    let mut rest = &body[..end];
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let close = after.find('"').expect("unterminated string in PROTOTYPES");
        literals.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    assert!(literals.len() % 2 == 0, "PROTOTYPES must hold name and prototype pairs");
    literals
        .chunks(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("runtime-prototypes-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "stage1 WP0: needs kelp-rt/src/abi.rs PROTOTYPES and the libkelp_rt.a archive"]
fn every_prototype_compiles_as_c_and_links_against_the_archive() {
    let protos = prototypes();
    assert!(!protos.is_empty(), "PROTOTYPES is empty");

    let mut c = String::from(LAYOUT_TYPEDEFS);
    for (_, proto) in &protos {
        // The prototype text is the C spelling, ending in `;` or not; the
        // probe adds the terminator when it is missing.
        c.push_str(proto.trim_end_matches(';'));
        c.push_str(";\n");
    }
    c.push_str("\nint main(void) {\n");
    for (name, _) in &protos {
        // Taking the address needs the declaration and the definition, and
        // does not call anything.
        c.push_str(&format!("    (void)&{name};\n"));
    }
    c.push_str("    return 0;\n}\n");

    let dir = scratch("probe");
    let src = dir.join("probe.c");
    let exe = dir.join("probe");
    fs::write(&src, c).unwrap();

    let compile = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Werror=implicit-function-declaration", "-Werror=incompatible-pointer-types"])
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .arg(archive())
        .args(["-pthread", "-lm"])
        .output()
        .expect("failed to run cc");
    assert!(
        compile.status.success(),
        "the prototypes do not compile and link against {}:\n{}",
        archive().display(),
        String::from_utf8_lossy(&compile.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "stage1 WP0: needs kelp-rt/src/abi.rs PROTOTYPES"]
fn the_prototype_table_names_match_their_prototypes() {
    // A pair whose prototype does not mention its name is a copy error in the
    // table, and the probe above would then declare the wrong function.
    for (name, proto) in prototypes() {
        assert!(
            proto.contains(&name),
            "PROTOTYPES entry `{name}` has the prototype `{proto}`, which does not name it"
        );
    }
}

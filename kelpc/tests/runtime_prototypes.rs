//! Every runtime export is spelled in `kelp-rt/src/abi.rs` as `PROTOTYPES`, a table of
//! `(name, prototype)` pairs. The LLVM backend's prelude is spelled from that table, so a row
//! whose name is not a real export would be a prelude that does not link.
//!
//! The first probe is written in Rust: it declares every name as an `extern "C"` symbol,
//! takes its address (so each one must be defined in the archive), and links the object against
//! `libkelp_rt.a` with `ld.lld`, using the same argv `kelpc` links a program with. The table is
//! checked for consistency by the second test as well.
//!
//! The probe needs the archive that `build.rs` writes to `OUT_DIR`. It is ignored until it is
//! run with `cargo test --test runtime_prototypes -- --ignored`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[allow(dead_code)]
#[path = "../src/guard.rs"]
mod guard;
#[allow(dead_code)]
#[path = "../src/sysroot.rs"]
mod sysroot;

use kelp_rt::abi::PROTOTYPES;

fn archive() -> PathBuf {
    match option_env!("OUT_DIR") {
        Some(dir) => PathBuf::from(dir).join("libkelp_rt.a"),
        None => panic!("OUT_DIR is not set for this test build"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("runtime-prototypes-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "needs the libkelp_rt.a archive from build.rs; run with --ignored"]
fn every_prototype_name_is_defined_in_the_archive() {
    assert!(!PROTOTYPES.is_empty(), "PROTOTYPES is empty");

    // A `#![no_std]` object with no entry point: `main` is defined here, the panic handler comes
    // from the archive, and the names are only referenced, so each one must resolve at link time.
    let mut rs = String::from("#![no_std]\n#![no_main]\nunsafe extern \"C\" {\n");
    for (name, _) in PROTOTYPES {
        rs.push_str(&format!("    fn {name}();\n"));
    }
    rs.push_str("}\n\n#[unsafe(no_mangle)]\npub extern \"C\" fn main(_argc: i32, _argv: *const *const u8) -> i32 {\n");
    for (name, _) in PROTOTYPES {
        rs.push_str(&format!("    core::hint::black_box({name} as unsafe extern \"C\" fn() as usize);\n"));
    }
    rs.push_str("    0\n}\n");

    let dir = scratch("probe");
    let src = dir.join("probe.rs");
    let obj = dir.join("probe.o");
    let exe = dir.join("probe");
    fs::write(&src, rs).unwrap();

    let compile = Command::new("rustc")
        .args(["--edition", "2024", "--crate-type", "lib", "--emit", "obj", "-C", "panic=abort", "-O", "-o"])
        .arg(&obj)
        .arg(&src)
        .output()
        .expect("failed to spawn rustc");
    assert!(
        compile.status.success(),
        "the name probe did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let sys = sysroot::discover_here().expect("could not find the link inputs");
    let runtime = archive();
    let objects = [obj.clone()];
    let argv = sysroot::hosted_link_argv(
        &sys,
        &sysroot::HostedLink {
            out: &exe,
            objects: &objects,
            runtime_obj: &runtime,
            externs_obj: None,
            gc_archive: None,
            library_dirs: &[],
            pkg_config_flags: &[],
            tls: false,
            link_libs: &[],
        },
    );
    let link = Command::new(&sys.ld_lld)
        .args(&argv)
        .output()
        .expect("failed to spawn ld.lld");
    assert!(
        link.status.success(),
        "the PROTOTYPES names are not all defined in {}:\n{}",
        archive().display(),
        String::from_utf8_lossy(&link.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_prototype_table_names_match_their_prototypes() {
    // A pair whose prototype does not mention its name is a copy error in the table, and the
    // probe above would then declare the wrong function.
    assert!(!PROTOTYPES.is_empty(), "PROTOTYPES is empty");
    for (name, proto) in PROTOTYPES {
        assert!(
            proto.contains(name),
            "PROTOTYPES entry `{name}` has the prototype `{proto}`, which does not name it"
        );
    }
}

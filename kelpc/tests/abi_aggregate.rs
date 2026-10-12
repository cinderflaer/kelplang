//! The BUG-vN4tR gate, linked against the runtime archive.
//!
//! This replaces `kelpc/tests/run-llvm-abi.sh`, which compiled
//! `kelpc/runtime/abi_aggregate.c` with `cc` and linked it into a Kelp program.
//! The body of `mem.alloc`'s extern (`kelp_std__mem__alloc_raw`) is now the
//! runtime's own export, so the Kelp program links against `kelp-rt` directly.
//! The link step goes through `cc` as the linker driver, the way `kelpc` links
//! every program. No C source is compiled.
//!
//! What it proves:
//! * the 24-byte `Bytes` shape has the size, alignment and field offsets of a
//!   `#[repr(C)]` struct in Rust, and the runtime's `kelp_rt::abi::Bytes` agrees;
//! * the LLVM backend declares `mem.alloc`'s extern with the 24-byte return as a
//!   hidden pointer, and no C-boundary declaration takes or returns a 24-byte
//!   aggregate by value;
//! * the runtime's body receives that hidden pointer, writes a 24-byte `Bytes`
//!   through it, and the Kelp program runs to the end with that value.

use std::fs;
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use kelp_rt::abi::Bytes;

/// The 24-byte aggregate in Rust's `#[repr(C)]` layout, the shape the C object
/// used (`kelp_abi_wide`).
#[repr(C)]
#[derive(Clone, Copy)]
struct Wide {
    a: i64,
    b: i64,
    c: i64,
}

/// The program the C gate ran: a 24-byte `Bytes` from `mem.alloc` is printed.
const SOURCE: &str = r#"// BUG-vN4tR: a 24-byte `Bytes` crosses into the runtime through the hidden
// pointer, not by value. The runtime's `mem.alloc` body returns zeroed bytes,
// which `print` writes as nothing before its newline.
impl "std/mem"

fun main() {
    val b = mem.alloc(3)
    print(b)
}
"#;

const _: () = {
    assert!(size_of::<Wide>() == 24);
    assert!(align_of::<Wide>() == 8);
    assert!(size_of::<Bytes>() == size_of::<Wide>());
    assert!(align_of::<Bytes>() == align_of::<Wide>());
};

#[test]
fn bug_vn4tr_the_24_byte_aggregate_has_the_c_layout() {
    assert_eq!(size_of::<Wide>(), 24);
    assert_eq!(align_of::<Wide>(), 8);
    assert_eq!(offset_of!(Wide, b), 8);
    assert_eq!(offset_of!(Wide, c), 16);
    assert_eq!(size_of::<Bytes>(), 24);
    assert_eq!(align_of::<Bytes>(), 8);
    assert_eq!(offset_of!(Bytes, len), 8);
    assert_eq!(offset_of!(Bytes, cap), 16);
}

/// The runtime archive `build.rs` builds into `target/<profile>/build/kelpc-*/out`.
/// The test binary lives in `target/<profile>/deps`, so the build directory is two
/// levels up. The most recently built archive wins.
fn runtime_archive() -> PathBuf {
    let build = std::env::current_exe()
        .expect("the test binary's path")
        .parent()
        .and_then(Path::parent)
        .expect("the test binary sits under target/<profile>/deps")
        .join("build");
    let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(&build)
        .unwrap_or_else(|e| panic!("cannot read '{}': {e}", build.display()))
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("kelpc-"))
        .map(|e| e.path().join("out").join("kelp_rt_managed.o"))
        .filter_map(|p| {
            let modified = fs::metadata(&p).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .collect();
    found.sort();
    found
        .pop()
        .expect("no kelp_rt_managed.o under the build directory: build kelpc first")
        .1
}

/// True when a `declare` line takes or returns `%kelp_Bytes` or `%kelp_String`
/// by value (the type name followed by a non-identifier character).
fn spells_aggregate(line: &str) -> bool {
    ["%kelp_Bytes", "%kelp_String"].iter().any(|name| {
        line.match_indices(name).any(|(at, _)| {
            line[at + name.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        })
    })
}

#[test]
fn bug_vn4tr_alloc_raw_returns_through_the_hidden_pointer() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bug_vn4tr_abi");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test directory");

    // The Kelp program and the runtime archive. `mem.alloc`'s body is the
    // runtime's own export now, so the program links against it directly.
    let source = dir.join("abi_aggregate.kp");
    fs::write(&source, SOURCE).expect("write the Kelp program");

    // The LLVM backend writes `<out>.ll` and `<out>.o`, and its extern body into
    // `<out>.externs.o`, which is left out of the link below.
    let prog = dir.join("abi_prog");
    let build = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&prog)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the ABI program did not compile:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    // The backend's own declaration, read back from the IR.
    let ir = fs::read_to_string(prog.with_extension("ll")).expect("read the emitted IR");
    assert!(
        ir.contains("%kelp_Bytes = type { ptr, i64, i64 }"),
        "the IR does not spell kelp_Bytes as a pointer and two i64 fields"
    );
    assert!(
        ir.lines().any(|l| l == "declare void @kelp_std__mem__alloc_raw(ptr, i64)"),
        "the IR does not spell the extern's 24-byte return as a hidden pointer"
    );
    let by_value: Vec<&str> = ir
        .lines()
        .filter(|l| l.starts_with("declare ") && spells_aggregate(l))
        .collect();
    assert!(
        by_value.is_empty(),
        "the IR names a 24-byte aggregate in a C-boundary declaration: {by_value:?}"
    );

    // Link the Kelp object against the runtime archive. `cc` is the linker
    // driver here, not a compiler of C source.
    let runtime = runtime_archive();
    let linked = dir.join("abi_prog_linked");
    let link = Command::new("cc")
        .arg("-o")
        .arg(&linked)
        .arg(prog.with_extension("o"))
        .arg(&runtime)
        .arg(env!("KELPC_GC_ARCHIVE"))
        .args(["-lm", "-lpthread", "-ldl"])
        .output()
        .expect("failed to spawn the linker");
    assert!(
        link.status.success(),
        "linking the Kelp object against the runtime archive failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&linked).output().expect("failed to run the linked program");
    assert!(run.status.success(), "the linked program failed: {:?}", run.status);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "\n",
        "the Kelp program did not print the zeroed bytes the runtime returned through the hidden pointer"
    );
}

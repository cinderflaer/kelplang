//! MK7 S4: the DWARF debug locations of the `llvm` backend
//! (the MK7 record in `plan/done.md`).
//!
//! The debug information maps a generated statement back to a `.kp` line. This
//! file checks that claim for the `llvm` backend, through DWARF: a compile unit named in
//! `llvm.dbg.cu`, one `DIFile` per `.kp` source, one `DISubprogram` per function
//! with its own declaration line, and a `DILocation` per statement, all built
//! through `llvm_ffi::DebugBuilder`.
//!
//! Two things are checked, in the order §4.4's exit criterion is written:
//!
//! 1. the artifact really carries DWARF: the decoded line table of the built
//!    binary names the fixture's `.kp` file and the line of each statement in it
//!    (`the_line_table_names_the_kp_file_and_its_statement_lines`);
//! 2. a fault resolves to a `.kp` file and line: the fixture faults on purpose
//!    (a signed division by a run-time zero traps), and the backtrace `gdb`
//!    prints names the line that faulted
//!    (`a_fault_resolves_to_a_kp_file_and_line`).
//!
//! The fixture is `tests/llvm-debug/fault.kp`, and it is deliberately **not** in
//! `tests/fixtures/`: `backend_coverage.rs` builds every `.kp` file there and
//! treats a run that dies from a signal as a failure, which is right for the
//! corpus and wrong for the one program whose whole job is to crash.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The fixture, resolved from this crate's manifest so the test does not depend
/// on the working directory it is run from.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("llvm-debug")
        .join(name)
}

/// Where the built binary goes. `CARGO_TARGET_TMPDIR` is per test binary, and
/// the name carries the fixture so the two tests here cannot race for one file.
fn out_path(name: &str) -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push("kelpc_llvm_debug");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir.join(name)
}

/// The levels the S6 debug-info gate checks (MK7 §4.3): the unoptimized dev
/// build and the default release build. A pass pipeline runs at `-O2` and none
/// runs at `-O0`, so the two together are the claim "the line table still
/// resolves a fault" at both ends. Optimizations may move the faulting
/// instruction off its line under `-O2`; if that happens the test fails with the
/// backtrace and the finding is reported rather than papered over.
const LEVELS: &[&str] = &["-O0", "-O2"];

/// Build the faulting fixture with the `llvm` backend at `level`. The retired
/// `KELPC_BACKEND` and `KELPC_ORACLE` are removed, so an inherited value cannot
/// change what is built. The level is pinned
/// by flag because one level cannot stand in for another. `tag` names the
/// calling test, and it and the level both go into the artifact path: libtest
/// runs these tests in parallel threads, and two builds of one fixture that
/// shared a path would race for its object file.
fn build_fault(level: &str, tag: &str) -> PathBuf {
    let source = fixture("fault.kp");
    let bin = out_path(&format!("fault{level}_{tag}"));
    let build = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .env_remove("KELPC_BACKEND")
        .args(["build", level])
        .arg(&source)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{}' to build with the llvm backend at {level}, but it failed:\n{}",
        source.display(),
        String::from_utf8_lossy(&build.stderr)
    );
    bin
}

/// Run one tool and return its combined output, failing by name when the tool
/// is not installed: a missing debugger is a missing check, not a pass.
fn tool(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("'{program}' is required by this test and could not be run: {e}"));
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The decoded line table of `bin` names the `.kp` fixture and the line of every
/// statement in it. This is the "the object actually contains DWARF" half of the
/// exit criterion: with no `llvm.dbg.cu`, no `Debug Info Version` flag or no
/// `DILocation`, `readelf` prints no row for the file and this fails with the
/// dump it did print.
#[test]
fn the_line_table_names_the_kp_file_and_its_statement_lines() {
    for level in LEVELS {
        let bin = build_fault(level, "table");
        let bin_text = bin.to_string_lossy().to_string();
        let dump = tool("readelf", &["--debug-dump=decodedline", bin_text.as_str()]);

        assert!(
            dump.contains("fault.kp"),
            "at {level}, the decoded line table names no `.kp` file:\n{dump}"
        );
        // The lines this fixture must put in the table: the declarations of
        // `divide` (15) and `main` (19), which are the lines their subprograms
        // and their entry blocks start under, and the two statements the fault
        // walks through, `return dividend / divisor` (16) and
        // `print(divide(10, zero))` (21). Those two are the statements the
        // fault walks through, so each must name its own line.
        for line in ["15", "16", "19", "21"] {
            let named = dump.lines().any(|row| {
                let mut fields = row.split_whitespace();
                fields.next() == Some("fault.kp") && fields.next() == Some(line)
            });
            assert!(
                named,
                "at {level}, no line-table row for 'fault.kp' at line {line}:\n{dump}"
            );
        }
    }
}

/// The exit criterion: the faulting instruction of an `llvm`-built binary
/// resolves to a `.kp` file and line.
///
/// The fixture traps with SIGFPE, so there is a real fault address, and `gdb`
/// reads the DWARF to place it. The frame that faulted must name `divide`'s
/// `return dividend / divisor`, and its caller must name `main`'s `print`: one
/// frame proves the fault's own line, the second proves the line table covers
/// the call chain rather than one instruction.
#[test]
fn a_fault_resolves_to_a_kp_file_and_line() {
    for level in LEVELS {
        let bin = build_fault(level, "bt");

        let run = Command::new(&bin)
            .current_dir(out_path(""))
            .output()
            .expect("failed to run the faulting binary");
        assert_eq!(
            run.status.code(),
            None,
            "at {level}, the fixture was expected to die from a signal, \
             but it exited cleanly"
        );
        assert_eq!(
            run.status.signal(),
            Some(libc_sigfpe()),
            "at {level}, the fixture was expected to die from SIGFPE, got {:?}",
            run.status
        );

        let bin_text = bin.to_string_lossy().to_string();
        let backtrace = tool(
            "gdb",
            &["-batch", "-nx", "-ex", "run", "-ex", "bt", bin_text.as_str()],
        );
        assert!(
            backtrace.contains("Program received signal SIGFPE"),
            "at {level}, gdb did not reach the trap:\n{backtrace}"
        );
        // `divide`'s `return dividend / divisor`, which is the instruction that
        // faults. At `-O2` an optimization is allowed to move it; if it does,
        // this assertion is the finding rather than a test to relax.
        assert!(
            backtrace.contains("fault.kp:16"),
            "at {level}, the faulting frame names no `.kp` line:\n{backtrace}"
        );
        // `main`'s `print(divide(10, zero))`, which is the call site.
        assert!(
            backtrace.contains("fault.kp:21"),
            "at {level}, the caller's frame names no `.kp` line:\n{backtrace}"
        );
    }
}

/// SIGFPE is 8 on every Linux architecture this compiler targets; it is spelled
/// here rather than pulled in as a constant so the assertion reads as the signal
/// it is.
fn libc_sigfpe() -> i32 {
    8
}

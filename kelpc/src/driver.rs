use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CompileError {
    pub message: String,
}

/// Writes `c_source` to `<out_path>.c` (kept alongside the binary, not
/// deleted, so a generated-C debugging session is a simple `gdb ./foo`
/// against a file that's actually there) and shells out to `cc -O2` to
/// produce the native binary at `out_path`.
///
/// This is the C backend's *finishing* step, and the only place in the
/// compiler that invokes an external tool. A later backend does not call it:
/// MK7 tier 1 emits assembly text and invokes `as`/`ld`, and tier 2 emits
/// machine code and writes the ELF itself, at which point nothing here is on
/// the path and this file's whole dependency on a system toolchain is gone.
/// Keeping that boundary explicit is what makes the removal a deletion rather
/// than a rewrite.
pub fn compile_c(c_source: &str, out_path: &Path) -> Result<(), CompileError> {
    let c_path = out_path.with_extension("c");
    std::fs::write(&c_path, c_source).map_err(|e| CompileError {
        message: format!("could not write generated C to '{}': {e}", c_path.display()),
    })?;

    let output = Command::new("cc")
        .arg("-O2")
        .arg("-o")
        .arg(out_path)
        .arg(&c_path)
        // `-fwrapv` makes signed overflow wrap instead of being UB, which is
        // what the plan's arithmetic-overflow section promises. `-lm` is for
        // `std/math`'s externs (an unused library is free to link).
        .arg("-fwrapv")
        .arg("-lm")
        .output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'cc': {e}"),
        })?;

    if !output.status.success() {
        return Err(CompileError {
            message: format!(
                "cc failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    Ok(())
}

/// Assembles `asm_source` to an object and links it into a native binary,
/// using `as` and `ld` directly (MK7 tier 1).
///
/// The point of this path is that no C compiler is involved: `cc` is a
/// compiler driver that preprocesses, compiles and links, and only the last of
/// those three is still wanted once the backend emits assembly. `as` finishes
/// the instruction encoding and `ld` does the linking, which is the whole of
/// what the C toolchain was contributing.
///
/// libc is still linked, and deliberately so — tier 1 keeps the runtime it has
/// and removes only the compiler. Where glibc *is* has to be discovered rather
/// than assumed, because this is not a directory-layout-standard system: see
/// `glibc_lib_dir`.
pub fn assemble_and_link(asm_source: &str, out_path: &Path) -> Result<(), CompileError> {
    // Both intermediates are kept next to the binary, for the same reason the
    // generated C is: `objdump` of the `.o` and reading the `.s` are how a
    // codegen bug is actually diagnosed, and a temp file that is deleted on the
    // way out makes both impossible.
    let s_path = out_path.with_extension("s");
    std::fs::write(&s_path, asm_source).map_err(|e| CompileError {
        message: format!("could not write generated assembly to '{}': {e}", s_path.display()),
    })?;

    let o_path = out_path.with_extension("o");
    let assembled = Command::new("as")
        .arg("--64")
        .arg("-o")
        .arg(&o_path)
        .arg(&s_path)
        .output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'as': {e}"),
        })?;
    if !assembled.status.success() {
        return Err(CompileError {
            message: format!(
                "as failed:\n{}",
                String::from_utf8_lossy(&assembled.stderr)
            ),
        });
    }

    let lib = glibc_lib_dir().ok_or_else(|| CompileError {
        message: "could not find the C library directory (looked for crt1.o via 'cc' and in \
                  the usual places); set KELPC_GLIBC_DIR to the directory holding crt1.o \
                  and libc.so"
            .to_string(),
    })?;
    let interp = [lib.join("ld-linux-x86-64.so.2"), lib.join("../lib64/ld-linux-x86-64.so.2")]
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| CompileError {
            message: format!(
                "no dynamic linker (ld-linux-x86-64.so.2) found under '{}'",
                lib.display()
            ),
        })?;

    // `crt1.o` provides `_start`, which is what calls `main` after libc's own
    // initialization; `crti.o`/`crtn.o` bracket the constructor and destructor
    // sections. Skipping them would mean providing `_start` ourselves, which is
    // tier 3's job (`plan/asm-backend/plan.md` §7 item 7), not this one's.
    let linked = Command::new("ld")
        .arg("-o")
        .arg(out_path)
        .arg("-dynamic-linker")
        .arg(&interp)
        .arg(lib.join("crt1.o"))
        .arg(lib.join("crti.o"))
        .arg(&o_path)
        .arg(lib.join("crtn.o"))
        .arg("-lc")
        .arg("-lm")
        .arg(format!("-L{}", lib.display()))
        .output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'ld': {e}"),
        })?;
    if !linked.status.success() {
        return Err(CompileError {
            message: format!(
                "ld failed:\n{}",
                String::from_utf8_lossy(&linked.stderr)
            ),
        });
    }

    Ok(())
}

/// The directory holding `crt1.o`, `libc.so` and the matching dynamic linker.
///
/// Found by asking the system compiler where its own `crt1.o` is, because that
/// is the one C library this machine will actually run against and a
/// hard-coded `/usr/lib/...` is wrong anywhere the layout is not the Debian
/// one. The query is a *path lookup*, not a compilation: `cc -print-file-name`
/// prints a filename and compiles nothing, so tier 1 still removes `cc` from
/// the work of turning a Kelp program into a binary.
fn glibc_lib_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("KELPC_GLIBC_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }

    for tool in ["cc", "clang"] {
        let Ok(out) = Command::new(tool).arg("-print-file-name=crt1.o").output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // A tool that does not know the name echoes it back, which is not a
        // path and must not be treated as one.
        if printed.is_empty() || printed == "crt1.o" {
            continue;
        }
        let p = PathBuf::from(&printed);
        if p.exists() {
            if let Some(dir) = p.parent() {
                return Some(dir.to_path_buf());
            }
        }
    }

    for candidate in ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib"] {
        let dir = Path::new(candidate);
        if dir.join("crt1.o").exists() {
            return Some(dir.to_path_buf());
        }
    }

    None
}

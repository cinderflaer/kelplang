use std::path::Path;
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

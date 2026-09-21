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
/// compiler that invokes an external tool. A later backend does not call it: it
/// writes its own object and links it, and this file's whole dependency on a
/// system C compiler ends at this function.
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

/// Links the object `codegen_llvm` wrote into the final binary: the LLVM
/// backend's *finishing* step, the sibling of [`compile_c`].
///
/// The same `cc` invocation as `compile_c`, and for the same reason (MK7 §1.3
/// keeps the C linker, so `glibc_lib_dir()` and the `crt1.o` lookup stay
/// unchanged), with one difference: there is no `-fwrapv` here. That flag is a
/// C-frontend flag that pins signed overflow; the LLVM backend emits plain
/// `add`/`sub`/`mul` IR, which wraps by definition, so the guarantee is already
/// in the object and the flag would be inert. `-lm` is for `std/math`'s externs
/// and is free whether or not they are used.
pub fn link_llvm_object(object_path: &Path, out_path: &Path) -> Result<(), CompileError> {
    let output = Command::new("cc")
        .arg("-O2")
        .arg("-o")
        .arg(out_path)
        .arg(object_path)
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

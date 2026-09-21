use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// The C runtime object, compiled from `runtime/kelp_rt.c` by `build.rs` and
/// embedded in the compiler binary. The driver writes it to a temporary file at
/// link time, so `kelpc` needs nothing beside its own binary and does not depend
/// on its install location (MK7 §3.2).
static RUNTIME_OBJECT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kelp_rt.o"));

/// Makes the temporary runtime object's name unique within one process: several
/// compilations can run at once (the test suite does), and two of them must not
/// write the same path.
static RUNTIME_TMP: AtomicU64 = AtomicU64::new(0);

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
/// unchanged), with two differences: there is no `-fwrapv` here, and the runtime
/// object the compiler embeds is written to a temporary file and linked
/// alongside. `-fwrapv` is a C-frontend flag that pins signed overflow; the LLVM
/// backend emits plain `add`/`sub`/`mul` IR, which wraps by definition, so the
/// guarantee is already in the object and the flag would be inert. `-lm` is for
/// `std/math`'s externs and is free whether or not they are used.
pub fn link_llvm_object(object_path: &Path, out_path: &Path) -> Result<(), CompileError> {
    let runtime_path = std::env::temp_dir().join(format!(
        "kelp_rt_{}_{}.o",
        std::process::id(),
        RUNTIME_TMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&runtime_path, RUNTIME_OBJECT).map_err(|e| CompileError {
        message: format!(
            "could not write the runtime object to '{}': {e}",
            runtime_path.display()
        ),
    })?;
    let output = Command::new("cc")
        .arg("-O2")
        .arg("-o")
        .arg(out_path)
        .arg(object_path)
        .arg(&runtime_path)
        .arg("-lm")
        .output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'cc': {e}"),
        })?;
    // The object is embedded in `kelpc`; the file was only a place for `cc` to
    // read from, and leaving one per compile behind would be litter.
    let _ = std::fs::remove_file(&runtime_path);

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

use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// A directory under the system temp directory, removed on drop.
///
/// `CARGO_TARGET_TMPDIR` is only set for integration tests, so a unit test
/// picks its own directory. The name carries the process id and the
/// nanosecond clock, which is what keeps two test processes from sharing one
/// directory; nothing else in this suite uses the system temp directory.
pub(crate) struct TempDir {
    pub(crate) path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kelpc-llvm-ffi-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("could not create the test's temp directory");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The whole pipeline this milestone claims, in one test: build `i32 @main()`
/// in IR (it calls `i32 @puts(i8*)` on a global string and returns 0),
/// verify, print the IR, emit an object for the host triple, link with `cc`,
/// and run the binary.
///
/// The proof is the program's stdout: `hello, kelp\n` and exit status 0. A
/// failure in any step fails loudly at that step, so the message says which
/// half of the pipeline broke.
#[test]
fn hello_world_ir_emits_an_object_that_cc_links_and_that_prints() {
    initialize_native_target().expect("the host has an LLVM target but no code generator");
    let triple = default_target_triple();
    assert!(
        !triple.is_empty(),
        "LLVMGetDefaultTargetTriple returned an empty triple"
    );

    let out = TempDir::new("hello-world");
    let object = out.path().join("hello.o");
    let executable = out.path().join("hello");

    let ctx = Context::new();
    let mut module = Module::new(&ctx, "hello");
    module.set_triple(&triple);
    assert_eq!(
        module.triple(),
        triple,
        "the module's triple is not the one LLVMGetDefaultTargetTriple returned"
    );

    // i32 @puts(i8*) is a declaration: no body, so it is the linker that
    // resolves it from libc.
    let i32_ty = ctx.i32();
    let puts_ty = ctx.fn_type(i32_ty, &[ctx.opaque_ptr()], false);
    let puts = module.add_function("puts", puts_ty);
    assert!(
        module.get_function("puts").is_some(),
        "LLVMGetNamedFunction could not find the declaration LLVMAddFunction made"
    );

    // define i32 @main() { entry: %greeting = ...; call i32 @puts(ptr %greeting); ret i32 0 }
    let main = module.add_function("main", ctx.fn_type(i32_ty, &[], false));
    let builder = Builder::new(&ctx);
    builder.position_at_end(main.append_block("entry"));
    // The global carries the text without a newline because `puts` writes one
    // of its own: with a trailing `\n` in the string the program would print
    // `hello, kelp` followed by two newlines, and stdout would not be the
    // `hello, kelp\n` this test asserts.
    let greeting = builder.global_string("hello, kelp", "greeting");
    builder.build_call(puts, puts_ty, &[greeting], "");
    builder.build_ret(ctx.const_int(i32_ty, 0, false));

    if let Err(message) = module.verify() {
        panic!("LLVMVerifyModule rejected the module:\n{message}");
    }

    let ir = module.to_ir_string();
    println!("--- LLVM IR ---\n{ir}--- end LLVM IR ---");
    assert!(
        ir.contains("define i32 @main()"),
        "the IR has no i32 @main() definition:\n{ir}"
    );
    assert!(
        ir.contains(&format!("target triple = \"{triple}\"")),
        "the IR does not carry the host triple '{triple}':\n{ir}"
    );

    let machine = TargetMachine::for_host()
        .unwrap_or_else(|e| panic!("no target machine for the host triple '{triple}': {e}"));
    machine.emit_object(&module, &object).unwrap_or_else(|e| {
        panic!("could not emit an object to '{}': {e}", object.display())
    });
    assert!(object.is_file(), "emit_object wrote no object file");

    // Link it the way a Kelp program is linked: `ld.lld` with the sysroot argv.
    crate::driver::link_llvm_objects(
        std::slice::from_ref(&object),
        None,
        &executable,
        crate::opt::OptLevel::O0,
        &[],
        &[],
        &[],
        false,
        false,
    )
    .unwrap_or_else(|e| {
        panic!("ld.lld failed linking '{}': {}", object.display(), e.message)
    });

    let run = Command::new(&executable)
        .output()
        .expect("could not run the linked binary");
    assert!(
        run.status.success(),
        "the linked binary exited with {:?}\nstderr:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        run.stdout,
        b"hello, kelp\n",
        "stdout is not byte-identical to the literal: {:?}",
        run.stdout
    );
}

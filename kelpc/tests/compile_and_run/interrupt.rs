use super::*;

// ---------------------------------------------------------------------------
// Kernel stage 5 (MK9-S7-M3, roadmap B-16): the `interrupt` attribute. An
// `interrupt fun` is a CPU handler: the compiler saves what the body touches
// and returns with the target's interrupt-return instruction. Riscv64 gets the
// `interrupt("machine")` attribute (`mret`); x86-64 gets `x86_intrcc` (`iretq`).
// The refusals are checked by name, and the boot proves the handler runs.
// ---------------------------------------------------------------------------

/// Writes a scratch freestanding project for `triple` into a fresh directory
/// and returns the directory. `main` is the entry module's source, and the
/// linker script is the x86-64 or riscv64 example's own.
fn interrupt_project(name: &str, triple: &str, main: &str) -> PathBuf {
    interrupt_project_with(name, triple, main, "[\"main\"]")
}

/// `interrupt_project`, with the `[unsafe]` audit list written out: the boot
/// projects also list `uart`, the module that drives the device.
fn interrupt_project_with(name: &str, triple: &str, main: &str, unsafe_modules: &str) -> PathBuf {
    // The project directory is not the binary's path, which is `out_path(name)`.
    let dir = out_path(&format!("{name}-project"));
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&src).expect("create the scratch project");
    let script = if triple.starts_with("x86_64") {
        fixture("tests/fixtures/examples/kernel0-x86_64-idt/kernel.ld")
    } else {
        project("freestanding_ptr_alloc_hook").join("kernel.ld")
    };
    std::fs::copy(script, dir.join("kernel.ld")).expect("copy the linker script");
    // A project whose UART module drives the device needs the window granted to
    // its entry module, which delegates it to `uart`.
    let caps = if unsafe_modules.contains("uart") {
        "\n[caps]\nmmio = [\"0x10000000+0x100\"]\n"
    } else {
        ""
    };
    std::fs::write(
        dir.join("coral.toml"),
        format!(
            "name = \"{name}\"\n\n[target]\nmode = \"freestanding\"\ntriple = \"{triple}\"\nentry = \"kmain\"\nlinker_script = \"kernel.ld\"\npanic = \"kp_panic\"\n\n[unsafe]\nmodules = {unsafe_modules}\n{caps}"
        ),
    )
    .expect("write the manifest");
    std::fs::write(src.join("main.kp"), main).expect("write the entry module");
    dir
}

/// The panic handler every scratch project names: it does nothing, since none
/// of the compiled programs here reaches it.
const PANIC: &str = "\nfun kp_panic(msg: str) {\n}\n";

/// A riscv64 handler the boot can enter: `kmain` installs it in `mtvec` and
/// executes `ecall`, which traps to the handler, and the handler prints and
/// halts. The UART is the same NS16550 driver the other riscv64 examples use.
fn riscv_boot_main() -> String {
    format!(
        "impl \"uart\" caps(mmio = [\"0x10000000+0x100\"])\n\n\
         // The handler: the CPU enters it with `ecall`, through `mtvec`, whose low two\n\
         // bits are the mode, so the vector is `align(4)`.\n\
         align(4) interrupt fun trap() {{\n    uart.puts(\"trap\\n\")\n    uart.halt()\n}}\n\n\
         section(\".text.boot\") fun kmain() {{\n    unsafe {{\n        asm(\"csrw mtvec, {{0}}\", in(\"r\") Ptr.from_fn(trap).addr())\n        asm(\"ecall\")\n    }}\n    uart.puts(\"not reached\\n\")\n    uart.halt()\n}}\n{PANIC}"
    )
}

/// A handler that returns, so its body ends in the interrupt-return instruction:
/// it counts the interrupts it has taken in a static.
const RETURNING_MAIN: &str = "static mut TICKS: usize = 0\n\n\
align(4) interrupt fun trap() {\n    unsafe {\n        TICKS = TICKS + 1\n    }\n}\n\n\
section(\".text.boot\") fun kmain() {\n}\n";

/// The riscv64 module the boot reads, copied from the Ptr.alloc example.
fn copy_uart(dir: &Path) {
    std::fs::copy(
        project("freestanding_ptr_alloc_hook").join("src").join("uart.kp"),
        dir.join("src").join("uart.kp"),
    )
    .expect("copy the UART driver");
}

/// A riscv64 `interrupt fun` compiles to a handler LLVM marks with the
/// `interrupt` attribute, and its body ends in `mret`, not a plain `ret`.
///
/// It needs `ld.lld` and `llvm-objdump` from `dev-kernel.nix`.
#[test]
pub(crate) fn an_interrupt_fun_on_riscv64_returns_with_mret() {
    if !have_ld_lld() {
        println!("skipping the riscv64 interrupt test: 'ld.lld' is not on PATH");
        return;
    }
    let dir = interrupt_project(
        "interrupt_riscv",
        "riscv64-unknown-none-elf",
        &format!("{RETURNING_MAIN}{PANIC}"),
    );
    let bin = build_freestanding("interrupt_riscv", &dir);
    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    assert!(
        ir.contains("\"interrupt\"=\"machine\""),
        "an 'interrupt fun' on riscv64 should carry the 'interrupt' attribute:\n{ir}"
    );
    let disassembly = tool("llvm-objdump", &["-d", bin.to_str().expect("utf-8 path")]);
    let body = disassembled_body(&disassembly, "kelp_trap");
    assert!(
        body.iter().any(|line| line.contains("mret")),
        "the riscv64 handler should return with 'mret':\n{}",
        body.join("\n")
    );
}

/// The boot half: the CPU enters the handler through `mtvec` on `ecall`, the
/// handler prints its line, and nothing after the `ecall` runs.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn an_interrupt_fun_is_entered_through_mtvec() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the interrupt boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let dir = interrupt_project_with(
        "interrupt_boot",
        "riscv64-unknown-none-elf",
        &riscv_boot_main(),
        "[\"main\", \"uart\"]",
    );
    copy_uart(&dir);
    boot_under_qemu(&RISCV64_QEMU, "interrupt_boot", &dir, &["trap"]);
}

/// An x86-64 `interrupt fun` is lowered with `x86_intrcc`, which takes the
/// interrupt frame, and its body returns with `iretq`.
///
/// It needs `ld.lld` and `llvm-objdump` from `dev-kernel.nix`.
#[test]
pub(crate) fn an_interrupt_fun_on_x86_64_returns_with_iretq() {
    if !have_ld_lld() {
        println!("skipping the x86-64 interrupt test: 'ld.lld' is not on PATH");
        return;
    }
    let dir = interrupt_project(
        "interrupt_x86",
        "x86_64-unknown-none",
        &format!("{RETURNING_MAIN}{PANIC}"),
    );
    let bin = build_freestanding("interrupt_x86", &dir);
    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    assert!(
        ir.contains("x86_intrcc"),
        "an 'interrupt fun' on x86-64 should use the 'x86_intrcc' calling convention:\n{ir}"
    );
    let disassembly = tool("llvm-objdump", &["-d", bin.to_str().expect("utf-8 path")]);
    let body = disassembled_body(&disassembly, "kelp_trap");
    assert!(
        body.iter().any(|line| line.contains("iretq")),
        "the x86-64 handler should return with 'iretq':\n{}",
        body.join("\n")
    );
}

/// A handler that takes parameters is refused: the CPU passes it nothing the
/// call could bind.
#[test]
pub(crate) fn an_interrupt_fun_with_parameters_is_refused() {
    let dir = interrupt_project(
        "interrupt_params",
        "riscv64-unknown-none-elf",
        &format!("interrupt fun trap(code: i32) {{\n}}\n\nsection(\".text.boot\") fun kmain() {{\n}}\n{PANIC}"),
    );
    assert_project_rejected("interrupt_params", &dir, "'trap' is an 'interrupt fun' and takes parameters");
}

/// A handler that returns a value is refused: it returns through the CPU's
/// interrupt return, not a Kelp return.
#[test]
pub(crate) fn an_interrupt_fun_returning_a_value_is_refused() {
    let dir = interrupt_project(
        "interrupt_result",
        "riscv64-unknown-none-elf",
        &format!("interrupt fun trap() => i32 {{\n    return 0\n}}\n\nsection(\".text.boot\") fun kmain() {{\n}}\n{PANIC}"),
    );
    assert_project_rejected("interrupt_result", &dir, "'trap' is an 'interrupt fun' and returns a value");
}

/// Kelp code cannot call a handler: the CPU enters it. The refusal names the
/// function and the way to install it instead.
#[test]
pub(crate) fn a_direct_call_to_an_interrupt_fun_is_refused() {
    let dir = interrupt_project(
        "interrupt_call",
        "riscv64-unknown-none-elf",
        &format!("interrupt fun trap() {{\n}}\n\nsection(\".text.boot\") fun kmain() {{\n    trap()\n}}\n{PANIC}"),
    );
    assert_project_rejected(
        "interrupt_call",
        &dir,
        "'trap' is an 'interrupt fun': the CPU enters it when an interrupt arrives, and Kelp code cannot call it",
    );
}

/// `interrupt fun` is a freestanding construct: a hosted program refuses it.
#[test]
pub(crate) fn an_interrupt_fun_in_a_hosted_build_is_refused() {
    let dir = out_path("interrupt_hosted");
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&src).expect("create the scratch project");
    std::fs::write(dir.join("coral.toml"), "name = \"interruptHosted\"\n")
        .expect("write the manifest");
    std::fs::write(src.join("main.kp"), "interrupt fun trap() {\n}\n\nfun main() {\n}\n")
        .expect("write the hosted program");
    assert_project_rejected(
        "interrupt_hosted",
        &dir,
        "'trap' is an 'interrupt fun', which is only legal in a freestanding build",
    );
}

/// A triple with no interrupt lowering is refused by name, not left to the
/// linker or the backend.
#[test]
pub(crate) fn an_interrupt_fun_on_aarch64_is_refused_by_name() {
    let dir = interrupt_project(
        "interrupt_aarch64",
        "aarch64-unknown-none",
        &format!("interrupt fun trap() {{\n}}\n\nsection(\".text.boot\") fun kmain() {{\n}}\n{PANIC}"),
    );
    assert_project_rejected("interrupt_aarch64", &dir, "has no interrupt lowering");
}

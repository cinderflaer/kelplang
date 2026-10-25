use super::*;

// ---------------------------------------------------------------------------
// Kernel stage 5 (MK9-S7-M1/M3, roadmap B-9/B-16): inline `asm` and `naked
// fun`. The templates in these fixtures are x86-64 assembly in Intel syntax,
// which is the dialect the compiler selects for an x86 target on both backends
// (`kelp wiki page language/unsafe.md`), so the fixtures run on the default LLVM backend.
// ---------------------------------------------------------------------------

/// `inout` is read then written, and `in` is read: `add {0}, {1}` accumulates
/// the input into the inout place and the result is printed.
#[test]
pub(crate) fn inline_asm_inout_and_in_operands_compute_a_sum() {
    assert_project_runs("asm_inout", &project("asm_inout"), "42\n");
}

/// An `out` operand writes its place, and the anonymous form `out("r") _` is
/// the asm expression's own value.
#[test]
pub(crate) fn inline_asm_out_operand_writes_its_place_and_the_anonymous_form_is_a_value() {
    assert_project_runs("asm_out", &project("asm_out"), "1234\n7\n");
}

/// `clobber(...)` adds a register to the constraint string, which is what tells
/// the compiler not to keep a value live in one. The program's own result is
/// unchanged, so the value of the fixture is that the clobber builds and runs.
#[test]
pub(crate) fn inline_asm_clobber_builds_and_runs() {
    assert_project_runs("asm_clobber", &project("asm_clobber"), "4\n");
}

/// A `naked fun` is called like any other function: the caller follows the
/// System V return-register convention and the asm is what puts 42 in `rax`.
#[test]
pub(crate) fn a_naked_function_returns_a_constant_in_rax_and_is_called_normally() {
    assert_project_runs("asm_naked", &project("asm_naked"), "42\n");
}

/// `asm` is an unsafe operation, so it needs an `unsafe` context on top of the
/// module's `[unsafe]` listing.
#[test]
pub(crate) fn an_asm_outside_unsafe_is_rejected() {
    assert_project_rejected(
        "asm_outside_unsafe",
        &project("asm_outside_unsafe"),
        "'asm' is an unsafe operation",
    );
}

/// The operand kinds are `in`, `out`, `inout` and `clobber`; anything else is
/// named in the refusal. A parser-level rejection, so it is a single negative
/// fixture rather than a project.
#[test]
pub(crate) fn an_unknown_asm_operand_kind_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/asm_unknown_operand.kp",
        "unknown asm operand kind 'sideways'",
    );
}

/// A `naked fun` body is exactly one `asm`: nothing else can be emitted around
/// it.
#[test]
pub(crate) fn a_naked_function_with_a_second_statement_is_rejected() {
    assert_project_rejected(
        "asm_naked_two_stmts",
        &project("asm_naked_two_stmts"),
        "a 'naked fun' body must be exactly one 'asm(...)' statement",
    );
}

/// `asm` is what puts a module in the audit list, exactly as an `unsafe` block
/// does: a module that writes one without the listing is refused at the `asm`.
#[test]
pub(crate) fn an_asm_in_a_module_without_the_manifest_listing_is_rejected() {
    assert_project_rejected(
        "asm_unlisted",
        &project("asm_unlisted"),
        "module 'main' is not listed in coral.toml [unsafe] modules",
    );
}

/// The riscv64 half of the stage: `csrr` and `wfi` have no Kelp spelling, so
/// the only way to write them is `asm`, and this is the object-only check that
/// the templates assemble. It needs `ld.lld` and `llvm-objdump` from
/// `dev-kernel.nix`; a shell without them prints a skip.
///
/// `csrr` is the decisive disassembly line: the generated `_start` stub already
/// contains a `wfi`, so only `csrr` could not have come from anywhere but this
/// fixture's asm.
#[test]
pub(crate) fn inline_asm_assembles_csrr_and_wfi_for_riscv64() {
    if !have_ld_lld() {
        println!(
            "skipping the riscv64 asm test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("asm_riscv");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("asm_riscv"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the riscv64 asm build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("csrr"),
        "the asm's csrr is not in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("wfi"),
        "the asm's wfi is not in the image:\n{disassembly}"
    );
}

/// Kernel stage 6's acceptance test: `examples/kernel0` is a complete Kelp
/// kernel that boots on QEMU's riscv64 `virt` machine and writes one line to
/// the NS16550 UART. The assertion is against what QEMU captured, so it proves
/// the boot, the linker script's stack and the driver's polling, not that the
/// source says `hello from kelp`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`; a shell
/// without them prints a skip.
#[test]
pub(crate) fn kernel0_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0",
        &fixture("tests/fixtures/examples/kernel0"),
        &["hello from kelp"],
    );
}

/// The trap half of kernel stage 6: `mtvec` points at a naked function, an
/// `ecall` reaches it, the handler prints and `mret`s back, and `kmain`
/// finishes. Both lines have to come out of the UART in one boot.
#[test]
pub(crate) fn kernel0_handles_a_trap_and_continues_after_it() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 trap test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_trap",
        &fixture("tests/fixtures/examples/kernel0-trap"),
        &["trap: ecall", "done"],
    );
}

/// Kernel stage 7's panic half: `examples/kernel0-panic` reads one element past
/// an array's end on purpose, so the compiler's own bounds check calls the
/// project's `[target] panic` handler with the static message the check was
/// written with. The handler prints `panic: ` and the message, which is only
/// possible now that a freestanding program has `str` operations to walk.
///
/// The assertion is the shape of the line rather than a whole line: the text is
/// the compiler's, and the test should not have to restate it verbatim to know
/// that the check fired.
#[test]
pub(crate) fn kernel0_panic_prints_the_checks_own_message_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 panic test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu_containing(
        &RISCV64_QEMU,
        "kernel0_panic",
        &fixture("tests/fixtures/examples/kernel0-panic"),
        "index out of bounds",
        "panic: ",
    );
}

/// Kernel stage 15: both accepted panic-handler shapes pass the front end, and
/// the three-argument one is the example's. `kelpc check` is the front end
/// alone, so this needs no `ld.lld` and runs in the ordinary suite.
#[test]
pub(crate) fn a_freestanding_panic_handler_may_take_a_location() {
    for name in ["kernel0-panic", "kernel0-panic2"] {
        let target = fixture(&format!("tests/fixtures/examples/{name}"));
        let check = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .arg("check")
            .arg(&target)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            check.status.success(),
            "'kelpc check' refused examples/{name}, whose panic handler is an accepted shape:\n{}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// Kernel stage 15: a handler whose shape is neither `fun(msg: str)` nor
/// `fun(msg: str, file: str, line: u32)` is a compile error naming both. Two
/// projects cover the wrong arity and the wrong parameter type.
#[test]
pub(crate) fn a_freestanding_panic_handler_with_another_shape_is_rejected() {
    let needle = "must be a 'fun(msg: str)' or a 'fun(msg: str, file: str, line: u32)'";
    assert_project_rejected(
        "freestanding_panic_bad_arity",
        &project("freestanding_panic_bad_arity"),
        needle,
    );
    assert_project_rejected(
        "freestanding_panic_bad_type",
        &project("freestanding_panic_bad_type"),
        needle,
    );
}

/// Kernel stage 15's acceptance boot: `examples/kernel0-panic2` prints the
/// message, file and 1-based line of every check it exercises, and answers a
/// panic raised inside its own handler with the project's `kelp_double_panic`
/// rather than the compiler's weak wait-for-interrupt fallback. The example's
/// `run.sh` boots one phase per check and asserts the exact `panic: <message>
/// at <file>:<line>` line, and phase 7 also asserts that a live `defer` and a
/// user `Drop` print nothing after the panic (a panic runs no cleanup). This
/// runs that script, which is where the serial assertions live.
#[test]
pub(crate) fn kernel0_panic2_names_each_checks_message_and_location() {
    if !have_ld_lld() || !have_llvm_mc() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-panic2 boot test: 'ld.lld', 'llvm-mc' or \
             'qemu-system-riscv64' is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let script = fixture("tests/fixtures/examples/kernel0-panic2/run.sh");
    let run = Command::new("sh")
        .arg(&script)
        .env("KELPC", kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .output()
        .expect("failed to spawn the kernel0-panic2 run script");
    assert!(
        run.status.success(),
        "examples/kernel0-panic2/run.sh failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("all seven phases printed the lines the compiler named"),
        "the run script did not reach its final line:\n{stdout}"
    );
}

/// Kernel stage 15: the compiler always defines `kelp_double_panic`, and
/// defines it **weak**, so an image links with no undefined symbol and a
/// project's own strong definition wins. `kelp_panic_entry` is the internal
/// entry every check calls.
#[test]
pub(crate) fn a_freestanding_double_panic_is_a_weak_defined_symbol() {
    if !have_ld_lld() {
        println!(
            "skipping the double-panic symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding(
        "kernel0_panic2_symbols",
        &fixture("tests/fixtures/examples/kernel0-panic2"),
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let double = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("kelp_double_panic"))
        .unwrap_or_else(|| panic!("the image has no kelp_double_panic:\n{symbols}"));
    assert_eq!(
        double.split_whitespace().nth(1),
        Some("W"),
        "kelp_double_panic is not weak, so a project cannot override it: {double}"
    );
    assert!(
        symbols
            .lines()
            .any(|line| line.split_whitespace().last() == Some("kelp_panic_entry")),
        "the image has no kelp_panic_entry:\n{symbols}"
    );
    assert!(
        tool("llvm-nm", &["-u", path]).trim().is_empty(),
        "the double-panic image has undefined symbols"
    );
}

/// Kernel stage 15's hosted half: the entry that carries a location is a
/// freestanding emission and changes nothing about a hosted panic. The runtime
/// still writes exactly `panic: <msg>` and exits non-zero, on both backends.
#[test]
pub(crate) fn a_hosted_panic_is_still_exactly_its_message() {
    let bin = out_path("hosted_panic_unchanged");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/panic_msg.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted panic fixture did not build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(run.status.code(), Some(1), "a hosted panic exits 1");
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        "panic: deliberate failure\n",
        "the hosted panic format changed"
    );
}

/// Kernel stage 7's hosted half: the `str` operations a freestanding build
/// lowers itself (`len`, `byte`, `eq`, `starts_with` and `ptr`) behave exactly
/// as they did before the stage. The run is compared with one literal.
#[test]
pub(crate) fn the_hosted_str_core_operations_are_unchanged() {
    assert_compiles_and_runs(
        "tests/fixtures/str_core.kp",
        "0\n4\n107\n112\ntrue\nfalse\ntrue\ntrue\ntrue\nfalse\nfalse\n",
    );
}

/// `str.index_of`, `str.contains` and `str.replace` on their edge cases: empty
/// and over-long needles, overlapping candidates, byte offsets in non-ASCII
/// text, and a match only at the end of a long haystack. Their answers are
/// pinned to the 26.10.4 output, which the search loops must keep.
#[test]
pub(crate) fn the_str_search_and_replace_edges_are_unchanged() {
    assert_compiles_and_runs(
        "tests/fixtures/str_search_edges.kp",
        "4\n0\nnone\n0\nnone\n0\n2\n2\n7\n0\n2\n0\nnone\nnone\n999\nnone\ntrue\ntrue\ntrue\nfalse\ntrue\na+b+c\nba\nabc\n\nhello\n\nxyz\nab\n3000\n1000\n",
    );
}

/// Kernel stage 7's rejection half: `std/str` is only partly on the freestanding
/// allowed set, so its other functions are refused at the call, by name. The check runs before any backend is named, so the message is the one `kelpc build` gives.
#[test]
pub(crate) fn a_std_str_function_outside_the_freestanding_five_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_str_refused",
        "'std.str.char_count' is unavailable in a freestanding build",
    );
}

/// Kernel stage 7's acceptance test: the five `str` operations, called from a
/// freestanding entry, compile through LLVM and `ld.lld` into an image whose
/// symbols all resolve. The point of the test is the `llvm-nm` line: the
/// obvious lowering of a NUL scan is a loop LLVM's own optimizer rewrites into
/// a `strlen` call, and the module's allocating bodies would leave `malloc` and
/// friends behind, so an image that links at all is the stage's whole claim.
#[test]
pub(crate) fn freestanding_str_builds_the_five_operations_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding str test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_str");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_str"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding str build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding str image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    assert!(
        !symbols.contains("strlen"),
        "the NUL scan was rewritten into a libc strlen call:\n{symbols}"
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 19 (release goal 3): the freestanding standard subset. Each
// admitted module has a project here that imports it and calls a
// representative function; every one is built for riscv64 and checked for zero
// undefined symbols, which is what "admitted" means. `kelp wiki page language/freestanding.md`
// is the table and how it was derived; `kelp wiki page language/freestanding.md` owns the mode.
// ---------------------------------------------------------------------------

/// The admitted set: one project per module the table admits, plus the boot.
/// Every one must link with no undefined symbol, so a module that reaches libc
/// or the allocator cannot be on the list by accident.
#[test]
pub(crate) fn every_admitted_std_module_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding std tests: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    for dir in [
        "freestanding_std_option",
        "freestanding_std_result",
        "freestanding_std_atomic",
        "freestanding_std_arena",
        "freestanding_std_irq",
        "freestanding_std_frame",
        "freestanding_std_str",
        "freestanding_std_math",
        "freestanding_std_alloc",
        "freestanding_std_regex",
        "freestanding_std_kernel",
        "freestanding_view",
    ] {
        let bin = out_path(dir);
        let build = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .arg("build")
            .arg(project(dir))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            build.status.success(),
            "the freestanding build of '{dir}' failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let path = bin.to_str().expect("the artifact path is utf-8");
        let undefined = tool("llvm-nm", &["-u", path]);
        assert!(
            undefined.trim().is_empty(),
            "'{dir}' links with undefined symbols:\n{undefined}"
        );
    }
}

/// Kernel stage 19's boot: `std/option`, `std/result` and `std/arena` in one
/// riscv64 image, printed over the real UART. `std/bits` is not shipped on this
/// branch, so those are the three the boot names.
#[test]
pub(crate) fn freestanding_std_kernel_boots_with_option_result_and_arena() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding std boot: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_std_kernel",
        &project("freestanding_std_kernel"),
        &[
            "some 1",
            "none 1",
            "unwrapped 14",
            "ok 1",
            "err 1",
            "fallback 9",
            "arena aligned 1",
            "arena byte 66",
            "arena used 4",
            "done",
        ],
    );
}

/// A whole std module that needs a hosted runtime is refused by name, and the
/// refusal lists the modules a freestanding project may implement.
#[test]
pub(crate) fn a_hosted_std_module_is_refused_by_name_with_the_allowed_list() {
    let allowed = "std/arena, std/atomic, std/convert, std/frame, std/irq, std/iter, std/list, \
                   std/math, std/ops, std/option, std/regex, std/result, std/str";
    for (dir, module) in [
        ("freestanding_std_refused_time", "std/time"),
        ("freestanding_std_refused_thread", "std/thread"),
        ("freestanding_std_refused_env", "std/env"),
        ("freestanding_std_refused_tz", "std/tz"),
    ] {
        let bin = out_path(dir);
        let build = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .arg("build")
            .arg(project(dir))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !build.status.success(),
            "expected '{dir}' to be rejected in a freestanding build"
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        for needle in [
            format!("the module '{module}' is unavailable in a freestanding build"),
            "needs a hosted runtime".to_string(),
            allowed.to_string(),
        ] {
            assert!(
                stderr.contains(&needle),
                "the refusal of '{dir}' does not mention '{needle}':\n{stderr}"
            );
        }
    }
}

/// `std/math` is admitted but its libm externs are not: a call to one is
/// refused by name inside the admitted module, rather than left for `ld.lld`.
#[test]
pub(crate) fn a_std_math_function_outside_the_integer_subset_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_std_math_refused",
        "'std.math.sqrt' is unavailable in a freestanding build",
    );
    assert_rejected_with(
        "tests/projects/freestanding_std_math_refused",
        "needs the hosted runtime",
    );
}

/// A `std/option` callback combinator is admitted only with the allocator pair:
/// its closure environment is a heap box, and without one the call is refused by
/// name rather than left as an undefined `free`.
#[test]
pub(crate) fn a_std_option_callback_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_std_option_no_alloc",
        "'std.option.Option#map<i32, i32>' is unavailable in a freestanding build",
    );
    assert_rejected_with(
        "tests/projects/freestanding_std_option_no_alloc",
        "needs an allocator",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 7: global `static` data. A `static` is one object with a stable
// address, `static mut` is the audited mutable form, and an `Atomic<T>` static
// is the safe shared mutable. Every run test here passes on the default LLVM
// backend; the freestanding
// and kernel tests are LLVM-only by design and skip in a shell without the
// tools.
// ---------------------------------------------------------------------------

/// A `static mut` counter bumped in `unsafe` from more than one function, read
/// back, addressed with `Ptr.from_mut` and written through, beside an immutable
/// lookup table and a `datac` static read by field.
#[test]
pub(crate) fn static_mut_counter_and_a_lookup_table_run() {
    assert_project_runs(
        "static_globals",
        &project("static_globals"),
        "5\n50\n100\n1\n2\n100\n4\n8\n",
    );
}

/// Kernel stage 21: an array static's element may be a `datac`, and a nested
/// array is the same rule one level down. This is the shape a scheduler's table
/// of tasks needs (a saved frame and a stack per task), and it runs on both
/// backends because the project is an ordinary hosted program.
#[test]
pub(crate) fn a_static_can_hold_an_array_of_datac_values() {
    assert_project_runs(
        "static_datac_array",
        &project("static_datac_array"),
        "45\n2\n3\n9\n1\n",
    );
}

/// An `Atomic<i64>` static incremented from two `thread.spawn`ed workers and
/// read back after both joins: exactly 1000, with no `unsafe`, because an
/// atomic mutates through a shared borrow.
#[test]
pub(crate) fn a_static_atomic_counter_sums_across_two_threads() {
    assert_project_runs("static_atomic_threads", &project("static_atomic_threads"), "1000\n");
}

/// A `static mut` read outside `unsafe` is refused, the same way a `Ptr.read`
/// is: the gate names the object and says what would make it legal.
#[test]
pub(crate) fn a_static_mut_read_outside_unsafe_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_mut_outside_unsafe.kp",
        "'static mut C' is an unsafe operation",
    );
}

/// A static's initializer must be a constant expression.
#[test]
pub(crate) fn a_non_constant_static_initializer_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_not_constant.kp",
        "the initializer is not constant",
    );
}

/// A heap type cannot be a static's type.
#[test]
pub(crate) fn a_heap_type_static_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_heap_type.kp",
        "'String' cannot be a static's type",
    );
}

/// Assigning to an immutable static is refused.
#[test]
pub(crate) fn assigning_to_an_immutable_static_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_assign_immutable.kp",
        "cannot assign to 'static X': it is not declared 'static mut'",
    );
}

/// The freestanding placement test: a nonzero immutable table lands in
/// `.rodata`, a nonzero `static mut` in `.data`, and the zero cursor and the
/// 4096-byte zero buffer in `.bss`. The `NOBITS` `.bss` is what proves the big
/// zero array does not grow the image, which is the whole reason a static goes
/// there and a folded `const` cannot.
///
/// It needs `ld.lld`, `llvm-readelf` and `llvm-size` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_statics_land_in_data_rodata_and_bss() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding statics test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_statics");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_statics"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding statics build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let sections = tool("llvm-readelf", &["-S", path]);
    let bss = sections
        .lines()
        .find(|line| line.contains("] .bss"))
        .unwrap_or_else(|| panic!("no .bss section:\n{sections}"));
    assert!(
        bss.contains("NOBITS"),
        "the zero statics should be in a NOBITS .bss:\n{bss}"
    );
    assert!(
        sections.lines().any(|line| line.contains("] .data") && line.contains("PROGBITS")),
        "the nonzero static mut should be in a PROGBITS .data:\n{sections}"
    );
    assert!(
        sections.lines().any(|line| line.contains("] .rodata") && line.contains("PROGBITS")),
        "the immutable table should be in a PROGBITS .rodata:\n{sections}"
    );

    let sizes = tool("llvm-size", &[path]);
    let total = sizes
        .lines()
        .find(|line| line.contains("fs.elf") || line.contains("freestanding_statics"))
        .or_else(|| sizes.lines().last())
        .unwrap_or_default();
    let numbers: Vec<u64> = total
        .split_whitespace()
        .take(4)
        .filter_map(|field| field.parse().ok())
        .collect();
    assert!(
        numbers.len() >= 3,
        "could not read llvm-size's text/data/bss columns:\n{sizes}"
    );
    assert_eq!(
        numbers[1], 8,
        "the nonzero static mut should be 8 bytes of .data:\n{sizes}"
    );
    assert!(
        numbers[2] >= 4096,
        "the zero statics should be at least 4096 bytes of .bss:\n{sizes}"
    );
}

/// Kernel stage 7's acceptance test: `examples/kernel0-statics` bumps a
/// `static mut` counter three times in `unsafe` and prints `counter 3` through
/// the NS16550 UART, then halts. The counter starts in `.bss`, which QEMU's ELF
/// loader zeroes, so a nonzero line proves the static round trip.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_statics_boots_and_prints_its_counter() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-statics boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_statics",
        &fixture("tests/fixtures/examples/kernel0-statics"),
        &["counter 3"],
    );
}

/// Kernel stage 8 (MK9-S3-M3): `percpu` is a freestanding-only storage class,
/// so a hosted project that declares one is refused by the front end, before any backend is named.
#[test]
pub(crate) fn a_percpu_static_is_rejected_outside_freestanding() {
    assert_rejected_with(
        "tests/projects/percpu_hosted",
        "only legal in a freestanding build",
    );
}

/// A `percpu` static has to be `mut`: a per-CPU value nothing may write is an
/// ordinary `static` with extra address space.
#[test]
pub(crate) fn a_percpu_static_without_mut_is_rejected() {
    assert_rejected_with("tests/projects/percpu_no_mut", "must be 'mut'");
}

/// A `percpu` static is a `static mut` in every way, so naming it outside an
/// `unsafe` block is the ordinary `static mut` error.
#[test]
pub(crate) fn a_percpu_static_named_outside_unsafe_is_rejected() {
    assert_rejected_with("tests/projects/percpu_outside_unsafe", "'static mut SLOT'");
}

/// Its initializer is folded to a constant and replicated into every copy, so
/// anything that would need code at startup is refused.
#[test]
pub(crate) fn a_percpu_static_with_a_non_constant_initializer_is_rejected() {
    assert_rejected_with("tests/projects/percpu_initializer", "not constant");
}

/// `[target] cpus` is an integer in `1..=64`; both ends are refused by the
/// manifest reader naming the key.
#[test]
pub(crate) fn a_cpu_count_outside_one_to_sixty_four_is_rejected() {
    assert_rejected_with("tests/projects/percpu_cpus_zero", "'[target] cpus'");
    assert_rejected_with("tests/projects/percpu_cpus_65", "'[target] cpus'");
}

/// Kernel stage 8's object-layout test: a `percpu` static is `cpus` copies of
/// its type, each padded to a 64-byte cache line, in a 64-byte-aligned
/// `.percpu`. At `cpus = 1` a `usize` is one 64-byte copy.
///
/// It needs `ld.lld`, `llvm-nm` and `llvm-readelf` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_percpu_static_is_one_cache_line_at_cpus_1() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding per-CPU layout test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let obj = build_freestanding_object("freestanding_percpu_1", "freestanding_percpu_1");
    let path = obj.to_str().expect("the artifact path is utf-8");
    assert_eq!(
        symbol_size(&tool("llvm-nm", &["-S", path]), "kelp_SLOT"),
        64,
        "one usize copy should be padded to a 64-byte cache line"
    );
    let sections = tool("llvm-readelf", &["-S", path]);
    let percpu: Vec<&str> = sections
        .lines()
        .filter(|line| line.contains("] .percpu"))
        .collect();
    assert_eq!(percpu.len(), 1, "expected one .percpu section:\n{sections}");
    assert_eq!(
        percpu_section_size(percpu[0]),
        64,
        "the .percpu section should be one 64-byte copy:\n{}",
        percpu[0]
    );
    assert_eq!(
        percpu_section_alignment(percpu[0]),
        64,
        "the .percpu section should be 64-byte aligned:\n{}",
        percpu[0]
    );
}

/// The same at `cpus = 4`, with one copy whose size fills a cache line exactly
/// (`usize`, 8 bytes padded to 64) and one rounded up to the next multiple
/// (`[u8; 100]`, 100 bytes padded to 128), so the size column checks both.
#[test]
pub(crate) fn freestanding_percpu_statics_are_four_aligned_copies_at_cpus_4() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding per-CPU layout test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let obj = build_freestanding_object("freestanding_percpu_4", "freestanding_percpu_4");
    let path = obj.to_str().expect("the artifact path is utf-8");
    let syms = tool("llvm-nm", &["-S", path]);
    assert_eq!(
        symbol_size(&syms, "kelp_SLOT"),
        4 * 64,
        "four usize copies at 64 bytes each:\n{syms}"
    );
    assert_eq!(
        symbol_size(&syms, "kelp_TABLE"),
        4 * 128,
        "four [u8; 100] copies rounded up to 128 bytes each:\n{syms}"
    );
    let sections = tool("llvm-readelf", &["-S", path]);
    let percpu: Vec<&str> = sections
        .lines()
        .filter(|line| line.contains("] .percpu"))
        .collect();
    assert!(!percpu.is_empty(), "no .percpu section:\n{sections}");
    for line in &percpu {
        assert_eq!(
            percpu_section_alignment(line),
            64,
            "every .percpu section should be 64-byte aligned:\n{line}"
        );
    }
}

/// Kernel stage 8's acceptance boot: `examples/kernel0-percpu` runs on a
/// two-hart riscv64 `virt` with no firmware, so the compiler's `_start` stub has
/// to park the second hart before it touches memory. Hart 0 writes 4 into its
/// own copy of the `percpu` static and prints it, then sets `tp = 1` by hand and
/// reads the other copy, which still holds the initial 7; `cpu_index()` and
/// `cpu_count()` print 1 and 2.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_percpu_boots_with_two_harts_and_shows_the_other_copy() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU_SMP2) {
        println!(
            "skipping the kernel0-percpu boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU_SMP2,
        "kernel0_percpu",
        &fixture("tests/fixtures/examples/kernel0-percpu"),
        &["cpu0 4", "cpu1 7", "idx 1", "count 2"],
    );
}

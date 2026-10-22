use super::*;

// ---------------------------------------------------------------------------
// Kernel stage 18 (MK10-S1's second half): IRQ lines and physical frames as
// capabilities. The hosted fixture below is a value test both backends run;
// the boot is freestanding riscv64 and skips in a shell without `ld.lld` and
// `qemu-system-riscv64`. The front-end refusals live in `tests/irq_frames.rs`.
// ---------------------------------------------------------------------------

/// The two capability values as plain hosted values: with no `[caps]` section
/// the entry holds the whole resource, so a constant claim is admitted and the
/// value it carries is the number the compiler checked.
#[test]
pub(crate) fn an_irq_line_and_a_frame_are_values_a_hosted_program_can_claim() {
    assert_compiles_and_runs(
        "tests/fixtures/irq_frame_claim.kp",
        "10\n2148532224\n2148532240\n2148536319\n",
    );
}

/// Kernel stage 18's acceptance boot: `examples/kernel0-caps` holds exactly one
/// interrupt line and one physical frame in its `[caps]` section, claims both
/// with the gated constructors, and a driver that holds the same grants prints
/// `line 10`, `frame 0x80100000` and `offset 0x80100010` over the NS16550. The
/// ungranted module in the same `src/` is a compile-error fixture the driver
/// never imports, so it does not reach the image.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_caps_claims_one_line_and_one_frame_and_prints_them() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-caps boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_caps",
        &fixture("tests/fixtures/examples/kernel0-caps"),
        &["line 10", "frame 0x80100000", "offset 0x80100010"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 10 (MK9-S4-M8): Sv39 address spaces. The encoders below are
// pure arithmetic and run hosted on both backends; the boot and the two faults
// are freestanding riscv64 and skip in a shell without `ld.lld` and
// `qemu-system-riscv64`.
// ---------------------------------------------------------------------------

/// Kernel stage 10's hosted half: the Sv39 VPN split, the PTE encoder and the
/// `satp` encoder, copied out of `examples/kernel0-vm/src/vm.kp` into a project
/// the build runs. The suite compares the output with one literal, so the literal pins what the freestanding image computes.
#[test]
pub(crate) fn the_sv39_vpn_split_and_pte_and_satp_encoders_are_the_specs() {
    assert_project_runs(
        "vm_pure",
        &project("vm_pure"),
        "255\n511\n511\n0\n4095\n536871119\n207\n2147483648\ntrue\ntrue\nfalse\ntrue\n8\n0\n524293\n",
    );
}

/// Build a freestanding project and assert the linked image names no undefined
/// symbol. Kernel stage 10's page tables reach only the frame-region symbols the
/// linker script defines, and the walker's `Ptr<u64>` accesses and `asm` must
/// leave no libc behind; a `malloc`, a `memcpy` or an unresolved frame bound
/// would show up here rather than at the boot.
pub(crate) fn assert_no_undefined_symbols(name: &str, target: &Path) {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &["-u", path]);
    assert!(
        symbols.trim().is_empty(),
        "the image for '{name}' has undefined symbols:\n{symbols}"
    );
}

/// Kernel stage 10's acceptance test: `examples/kernel0-vm` builds an Sv39
/// root table, identity-maps the image, its stack and the frame region, maps
/// the UART and one high page, drops from machine mode to supervisor mode,
/// turns the MMU on, and proves that the high virtual name and the
/// identity-mapped physical name reach the same frame. Every step is a line on
/// the UART. The mappings go through the `Space` owner, so the last two lines
/// are its range check: an unaligned page and a page above the owner's range
/// are both refused.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_vm_boots_and_aliases_a_high_page() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-vm boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_vm",
        &fixture("tests/fixtures/examples/kernel0-vm"),
        &[
            "vm: m-mode",
            "vm: root",
            "vm: identity",
            "vm: uart page",
            "vm: high page",
            "vm: s-mode",
            "vm: paging on",
            "vm: alias ok",
            "vm: unmap ok",
            "vm: align refused",
            "vm: range refused",
        ],
    );
}

/// Kernel stage 10's fault half: `examples/kernel0-vm-fault` takes a load page
/// fault (`scause` 13) on an unmapped address and a store page fault (15) on a
/// read-only leaf, and the S-mode trap handler prints both. The lines are the
/// handler's own hex rendering of `scause` and `stval`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_vm_fault_reports_a_load_and_a_store_page_fault() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-vm-fault boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_vm_fault",
        &fixture("tests/fixtures/examples/kernel0-vm-fault"),
        &[
            "fault: m-mode",
            "fault: paging on",
            "fault: scause 0xd stval 0x40000000",
            "fault: scause 0xf stval 0x3ffffff000",
        ],
    );
}

/// The two address-space images link with no undefined symbol: the page-table
/// walker, the frame allocator and the dropped-to-supervisor boot must each
/// resolve entirely inside the image.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_vm_images_link_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-vm symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_vm", &fixture("tests/fixtures/examples/kernel0-vm"));
    assert_no_undefined_symbols("kernel0_vm_fault", &fixture("tests/fixtures/examples/kernel0-vm-fault"));
}

/// Kernel stage 11's frame half: the trap frame is 264 bytes with `x1` at 0,
/// `x2` at 8, `x31` at 240, `sepc` at 248 and `sstatus` at 256. Those numbers
/// are what `trap_entry`'s asm hardcodes, so the project prints them and the
/// test pins them; the boot checks the same four against the same constants and
/// prints `user: frame ok` only when they agree. `trap_frame_layout` is a copy
/// of the example's declaration, and the layout queries fold in the front end,
/// so this runs on both backends with the same output.
#[test]
pub(crate) fn the_trap_frame_offsets_are_what_the_trap_entry_hardcodes() {
    assert_project_runs(
        "trap_frame_layout",
        &project("trap_frame_layout"),
        "264\n0\n8\n240\n248\n256\n",
    );
}

/// Kernel stage 21: the scheduler's task table is the second hardcoded layout.
/// The naked trap entry saves `x1..x31` at 0..240, `sepc` at 248 and `mstatus`
/// at 256, reads the scheduler's address from the word at 264, and the
/// scheduler reaches `state` at 272, `id` at 280 and the 4096-byte stack at 288
/// from the table's base. `sched.layout_ok` checks the same numbers at boot.
#[test]
pub(crate) fn the_scheduler_task_offsets_are_what_the_trap_entry_hardcodes() {
    assert_project_runs(
        "sched_task_layout",
        &project("sched_task_layout"),
        "264\n8\n248\n256\n4384\n0\n264\n272\n280\n288\n",
    );
}

/// Kernel stage 11's acceptance test: `examples/kernel0-user` drops to U-mode
/// and takes `ecall`s. Before any program runs, the boot checks every refusal
/// the ABI names (`syscall: refusals ok`). The first program writes `hello from
/// user`, earns `-14` from a write whose pointer names a mapped kernel page, turns
/// the errno into a hex digit and writes `bad write -> E`, then exits with 7; the
/// second loads from an unmapped address, and the kernel prints the cause and
/// survives it. Every line is QEMU's own stdout.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_user_runs_a_syscall_abi_and_kills_a_faulting_program() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-user boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_user",
        &fixture("tests/fixtures/examples/kernel0-user"),
        &[
            "user: s-mode",
            "user: paging on",
            "user: frame ok",
            "syscall: refusals ok",
            "hello from user",
            "user: efault ok",
            "bad write -> E",
            "user exited 7",
            "user killed: scause 13",
            "kernel survived",
        ],
    );
}

/// The user-mode image links with no undefined symbol: the trap entry, the
/// dispatcher and the syscall copy reach only the image's own code and the
/// frame region's linker symbols, and the two user programs live in their own
/// sections rather than in a libc the image does not have.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_user_image_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-user symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_user", &fixture("tests/fixtures/examples/kernel0-user"));
}

/// Kernel stage 21's acceptance boot: `examples/kernel0-sched` programs the
/// CLINT's `mtimecmp`, enables the machine timer interrupt and round-robins
/// three tasks that never yield, so the letters in the serial log can only come
/// from preemption. The example's `run.sh` boots it and asserts that all three
/// letters appear, that the letter changes at least three times and that no
/// line is torn; this runs that script, which is where the serial assertions
/// live.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_sched_preempts_three_tasks_on_the_machine_timer() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-sched boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let script = fixture("tests/fixtures/examples/kernel0-sched/run.sh");
    let run = Command::new("sh")
        .arg(&script)
        .env("KELPC", kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .output()
        .expect("failed to spawn the kernel0-sched run script");
    assert!(
        run.status.success(),
        "examples/kernel0-sched/run.sh failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("no torn lines"),
        "the run script did not reach its final line:\n{stdout}"
    );
}

/// The scheduler image links with no undefined symbol: the trap entry, the
/// scheduler and the CLINT driver reach only the image's own code, the linker
/// script's stack symbols and the two device windows it was granted.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_sched_image_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-sched symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_sched", &fixture("tests/fixtures/examples/kernel0-sched"));
}

/// A `pub static`/`pub static mut` is reached across modules exactly as a
/// `pub const` is (`state.COUNT`, `state.LIMIT`), and the mutable one is still
/// `unsafe` on the importing side.
#[test]
pub(crate) fn a_public_static_crosses_a_module_boundary() {
    assert_project_runs("static_module", &project("static_module"), "2\n3\n");
}

/// A static without `pub` is private to its module, which is the same
/// visibility gate a `const` goes through.
#[test]
pub(crate) fn a_private_static_is_not_reachable_from_another_module() {
    assert_project_rejected(
        "static_private",
        &project("static_private"),
        "'state.HIDDEN' is private",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 6 (MK9-S6-M1, roadmap B-8): `defer`. `defer.rs` lowers every
// deferred action to ordinary code at each exit of its block, so the fixtures
// below are ordinary programs by the time either backend sees them: LLVM is the
// default and these run there.
// ---------------------------------------------------------------------------

/// The deferred actions of one block run last-in-first-out, after the block's
/// own statements, and the block form (`defer { ... }`) is one action.
#[test]
pub(crate) fn deferred_actions_run_last_in_first_out() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_lifo.kp",
        "body\nthird-a\nthird-b\nsecond\nfirst\n",
    );
}

/// A `return` nested in an `if` inside a `while` leaves the loop body and then
/// the function body, so the loop body's defer runs and then the function's.
#[test]
pub(crate) fn a_return_inside_a_loop_runs_both_blocks_defers() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_return_in_loop.kp",
        "loop-out\nloop-out\nloop-out\nf-out\n20\n",
    );
}

/// Each block owns its own deferred actions, so an inner block's run when it is
/// left and the outer block's wait for the outer exit.
#[test]
pub(crate) fn a_nested_blocks_defers_run_at_its_own_exit() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_nested_blocks.kp",
        "inner-body\ninner-end\nouter-body\nouter-end\n",
    );
}

/// A deferred action reads its locals at the block's exit, so a mutation after
/// the `defer` statement is visible to it.
#[test]
pub(crate) fn a_deferred_action_sees_the_final_value_of_a_local() {
    assert_compiles_and_runs("tests/fixtures/defer_sees_final_value.kp", "42\n");
}

/// The kernel pattern: a flag set at the top is released by a `defer`, and the
/// caller sees it clear after the function returns.
#[test]
pub(crate) fn a_defer_releases_an_atomic_lock_flag_on_the_way_out() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_lock_flag.kp",
        "true\ntrue\nfalse\n",
    );
}

/// A function with a borrowed return type keeps its elided lifetime when its
/// body has a `defer`, and a `panic` tail stays a diverging tail (so the
/// deferred action does not run on it).
#[test]
pub(crate) fn a_defer_keeps_a_borrowed_return_and_a_panic_tail_working() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_borrow_return.kp",
        "as_str-out\nhello\n",
    );
}

/// A `defer` written in an `if` block belongs to that block, so the branch's
/// action runs before the function body's.
#[test]
pub(crate) fn a_defer_in_a_branch_runs_before_the_function_body_defer() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_then_branch.kp",
        "then\nout\n1\nout\n2\n",
    );
}

/// `for` desugars to a `while`, so a `defer` in the loop body runs once per
/// iteration.
#[test]
pub(crate) fn a_defer_in_a_for_body_runs_once_per_iteration() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_for_loop.kp",
        "1\niter-end\n2\niter-end\n",
    );
}

/// A block used as a value keeps that value while its deferred actions run,
/// and a function whose body is a block expression is the same case.
#[test]
pub(crate) fn a_value_block_runs_its_defers_and_keeps_its_value() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_value_block.kp",
        "block-out\ntext\nf-out\n5\n",
    );
}

/// The `?` refusal is about the block the `defer` is written in: a `?` inside a
/// `fun` literal belongs to the closure and leaves the outer `defer` alone.
#[test]
pub(crate) fn a_defer_tolerates_a_question_mark_inside_a_closure() {
    assert_compiles_and_runs("tests/fixtures/defer_closure_try.kp", "4\nout\n");
}

/// A panic never returns, so a deferred action does not run on the way out: the
/// action's line is absent from stdout and the process still reports the panic.
#[test]
pub(crate) fn a_panic_does_not_run_a_deferred_action() {
    let source = "tests/fixtures/defer_panic_skips_action.kp";
    let bin = out_path("defer_panic_skips_action");
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{source}' to panic, but it exited successfully"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("before"), "the program printed nothing before the panic:\n{stdout}");
    assert!(
        !stdout.contains("after"),
        "a panic ran the deferred action:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("panic: stop"), "unexpected stderr:\n{stderr}");
}

/// `return f()` where `f` returns `unit` is legal: there is no value to carry,
/// so both backends emit the call for its side effect and a bare return.
#[test]
pub(crate) fn returning_a_unit_valued_call_is_legal_on_both_backends() {
    assert_compiles_and_runs("tests/fixtures/unit_return_value.kp", "said\n");
}

/// `?` leaves the enclosing function, which would skip the deferred action, so
/// a `defer` and a `?` in one block are refused by name.
#[test]
pub(crate) fn a_defer_with_a_question_mark_exit_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/defer_try_exit.kp",
        "defer with an early `?` exit is not supported yet",
    );
}

/// A deferred action has no caller to return to, so a `return` inside one is
/// refused rather than duplicated at every exit.
#[test]
pub(crate) fn a_return_inside_a_deferred_action_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_return_inside.kp",
        "a deferred action cannot contain 'return'",
    );
}

/// A local moved out before the block's exit is a use of a moved value when the
/// deferred action reads it, reported at the `defer`.
#[test]
pub(crate) fn a_deferred_action_reading_a_moved_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_use_after_move.kp",
        "use of moved value",
    );
}

/// A deferred action sees only the locals already in scope where the `defer` is
/// written, so naming one declared later in the block is refused.
#[test]
pub(crate) fn a_defer_naming_a_later_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_later_local.kp",
        "declared later in the block",
    );
}

// Kernel stage 6 `errdefer` (`design/syntax.md (kelp wiki)`, errdefer): an action that runs
// only when the function leaves through its error variant. `defer.rs` lowers the
// returned value's test into a `when`, so the error path runs the errdefers and
// the other paths do not.

/// On the error path the `errdefer` runs before the function returns its `Err`;
/// on the success path it does not run.
#[test]
pub(crate) fn an_errdefer_runs_on_the_error_path_only() {
    assert_compiles_and_runs(
        "tests/fixtures/errdefer_error_path.kp",
        "8\ncleanup\nerr\n",
    );
}

/// A `?` that fails after an `errdefer` has been written runs it, with the
/// locals bound before it still in scope; a `?` that succeeds does not.
#[test]
pub(crate) fn an_errdefer_runs_when_a_later_question_mark_fails() {
    assert_compiles_and_runs(
        "tests/fixtures/errdefer_second_try.kp",
        "3\n1\nerr\n",
    );
}

/// An `errdefer` in a function whose return type is neither Result nor Option is
/// refused, because there is no error variant for it to run on.
#[test]
pub(crate) fn an_errdefer_in_a_function_that_does_not_return_result_is_refused() {
    assert_rejected_with(
        "tests/fixtures/errdefer_non_result.kp",
        "an 'errdefer' runs only on the error path",
    );
}

/// An `errdefer` naming a local declared after it is refused by name, as a
/// `defer` is.
#[test]
pub(crate) fn an_errdefer_naming_a_later_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/errdefer_later_local.kp",
        "declared later in the block",
    );
}

// A `String` passed to a `str` parameter is borrowed for the call, as `.as_str()`
// would borrow it (`design/syntax.md (kelp wiki)`, 11.2).

/// A `String` argument for a `str` parameter prints the text the callee sees and
/// returns its length, exactly as the explicit `.as_str()` form does.
#[test]
pub(crate) fn a_string_argument_is_borrowed_for_a_str_parameter() {
    assert_compiles_and_runs("tests/fixtures/string_to_str_arg.kp", "kelp\n4\n4\n");
}

/// A `String` moved into a call in the same argument list as its borrow is
/// refused by the borrow rules, as the explicit `.as_str()` form is.
#[test]
pub(crate) fn a_string_borrowed_for_a_str_parameter_cannot_be_moved_in_the_same_call() {
    assert_rejected_with("tests/fixtures/string_to_str_move.kp", "cannot move 'name' while it is borrowed");
}

/// Only a `str` parameter borrows a `String`: any other parameter type keeps the
/// ordinary mismatch error.
#[test]
pub(crate) fn a_string_for_a_non_str_parameter_is_refused() {
    assert_rejected_with("tests/fixtures/string_to_str_non_str.kp", "argument 'n' has type 'String', expected 'i32'");
}

/// A `str` parameter of a generic function takes no implicit borrow, and the
/// refusal names the function and the `.as_str()` fix.
#[test]
pub(crate) fn a_string_for_a_str_parameter_of_a_generic_function_is_refused_by_name() {
    assert_rejected_with("tests/fixtures/string_to_str_generic.kp", "generic function 'label' takes no implicit borrow");
}

/// `break`/`continue` are ordinary loop control in a written `while`
/// (`kelp wiki page language/iterators.md`).
#[test]
pub(crate) fn break_and_continue_control_a_while_loop() {
    assert_compiles_and_runs("tests/fixtures/break_and_continue.kp", "1\n2\n4\n5\n");
}

/// A deferred action runs while its block is being left, so a `break` inside it
/// has no loop to leave.
#[test]
pub(crate) fn a_break_inside_a_deferred_action_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_break_inside.kp",
        "a deferred action cannot contain 'break'",
    );
}

/// `break`/`continue` outside every loop is refused at the statement.
#[test]
pub(crate) fn break_outside_a_loop_is_refused() {
    assert_rejected_with(
        "tests/fixtures/for_iter_break_outside.kp",
        "no loop encloses it",
    );
}

/// Kernel stage 6's freestanding acceptance test: a project that releases a
/// lock flag with a `defer` builds through LLVM and `ld.lld` into an ELF64
/// RISC-V image that references no undefined symbol.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
pub(crate) fn a_freestanding_project_can_release_a_lock_with_a_defer() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding defer test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_defer");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_defer"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding defer build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    assert!(disassembly.contains("wfi"), "the _start halt is missing:\n{disassembly}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding defer image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 9 (MK9-S6-M1, roadmap B-8): the second half of stage 6, a user
// `Drop`. `fun mut T.drop()` is the destructor; the generated `kelp_drop_<T>`
// calls it first and drops the fields after, so every drop point the plan
// already has reaches it. The fixtures run on the LLVM backend.
// ---------------------------------------------------------------------------

/// The user destructor runs before the fields, fields drop in declaration
/// order, and locals die last-in-first-out.
#[test]
pub(crate) fn a_user_destructor_runs_before_fields_in_declaration_order() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_order.kp",
        "body\nleaf\n3\npair\nleaf\n1\nleaf\n2\n",
    );
}

/// Every drop point the plan already had reaches the destructor: an early
/// `return` frees the frame after reading the returned value, and a loop body
/// frees one value per iteration.
#[test]
pub(crate) fn a_destructor_runs_at_every_drop_point() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_control_flow.kp",
        "leaf\n1\n9\nleaf\n2\nleaf\n1\n0\nloop\nleaf\n10\nloop\nleaf\n11\n",
    );
}

/// Reassigning an owned `var` of a destructor type runs the destructor on the
/// value it replaces.
#[test]
pub(crate) fn reassigning_an_owned_destructor_value_runs_the_destructor() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_reassign.kp",
        "leaf\n1\nafter\nleaf\n2\n",
    );
}

/// A moved-out value is not dropped by the scope that declared it.
#[test]
pub(crate) fn a_moved_out_destructor_value_is_not_dropped() {
    assert_compiles_and_runs("tests/fixtures/drop_moved.kp", "moved\nleaf\n1\n");
}

/// Decision 21: a container of a destructor type is refused in a hosted build, because
/// a resource never enters the collected heap. The refusal is what this test asserts.
#[test]
pub(crate) fn containers_drop_each_element_through_its_destructor() {
    assert_fixture_refused("drop_container", "'Leaf' is a resource");
}

/// An `enum` destructor runs for every variant, and the live variant's payload
/// drops after it.
#[test]
pub(crate) fn an_enum_destructor_runs_for_every_variant() {
    assert_compiles_and_runs("tests/fixtures/drop_enum.kp", "body\nmaybe\nmaybe\nleaf\n1\n");
}

/// Decision 21: a `Map` value of a destructor type is refused in a hosted build, because
/// a resource never enters the collected heap. The refusal is what this test asserts.
#[test]
pub(crate) fn map_values_drop_through_their_destructor() {
    assert_fixture_refused("drop_map", "'Guard' is a resource");
}

/// The kernel pattern the stage exists for: the guard's destructor releases the
/// flag, so the caller sees it clear after the guarded call returns.
#[test]
pub(crate) fn a_lock_guard_releases_the_flag_through_its_destructor() {
    assert_compiles_and_runs("tests/fixtures/drop_guard_lock.kp", "true\nfalse\n");
}

/// A destructor type that owns a buffer: the user body runs first and can read
/// the field, and the field drop frees it after. It is in the AddressSanitizer
/// pass, where a missing drop is a leak and a second one a double free.
#[test]
pub(crate) fn a_destructor_type_owns_its_buffer_until_the_last_field_drop() {
    assert_compiles_and_runs("tests/fixtures/drop_owned_field.kp", "body\nnamed\n4\nnamed\n5\n");
}

/// Decision 21: a closure that captures a destructor type cannot cross a thread
/// boundary in a hosted build, so the refusal is what this test asserts.
#[test]
pub(crate) fn a_destructor_in_a_thread_closures_environment_runs_on_the_worker() {
    assert_fixture_refused("drop_thread_closure", "'Guard' cannot be a value captured by a closure environment in a hosted build");
}

/// A destructor already runs when the value dies, so naming it explicitly would
/// run it twice.
#[test]
pub(crate) fn an_explicit_destructor_call_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/explicit_destructor_call.kp",
        "explicit destructor call",
    );
}

/// A destructor's shape is checked where it is declared: a shared receiver is
/// not the mutable borrow the drop body needs.
#[test]
pub(crate) fn a_destructor_must_take_its_receiver_with_mut() {
    assert_rejected_with(
        "tests/fixtures/drop_wrong_signature.kp",
        "must take its receiver with 'mut'",
    );
}

/// One destructor per type: a second declaration shares the `T#drop` key.
#[test]
pub(crate) fn a_second_destructor_for_one_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_two_impls.kp",
        "method 'Leaf.drop' is defined more than once",
    );
}

/// Only a user `datac`/`enum` can declare a destructor; a builtin type's
/// methods are the compiler's.
#[test]
pub(crate) fn a_destructor_on_a_builtin_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_builtin_type.kp",
        "is a builtin type",
    );
}

/// A destructor type is not `Copy`, so the derived deep-copy fixpoint excludes
/// it: `List.clone` is refused rather than producing a second owner.
#[test]
pub(crate) fn a_destructor_type_cannot_be_copied() {
    assert_rejected_with(
        "tests/fixtures/drop_not_copy.kp",
        "a type with a destructor is not `Copy`",
    );
}

/// The map clones every key in, so a destructor type cannot declare a key
/// pair either.
#[test]
pub(crate) fn a_destructor_type_cannot_be_a_map_key() {
    assert_rejected_with(
        "tests/fixtures/drop_declared_key.kp",
        "a type with a destructor is not deep-copyable",
    );
}

/// A partial move of a destructor-typed field zeroes the slot it came from, so
/// the base's own drop would run the destructor a second time on the empty
/// slot; the move is refused instead.
#[test]
pub(crate) fn a_partial_move_of_a_destructor_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_partial_move.kp",
        "it is a type with a destructor, and a partial move zeroes the slot",
    );
}

/// Kernel stage 9's freestanding acceptance test: a project whose guard
/// releases a static lock flag through its destructor builds through LLVM and
/// `ld.lld` into an ELF64 RISC-V image that references no undefined symbol, and
/// the generated drop and the user body are both in the image.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
pub(crate) fn a_freestanding_project_can_release_a_lock_with_a_destructor() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding drop test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_drop");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_drop"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding drop build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    // The generated drop and the user body it calls are both in the image, so
    // the destructor is linked in and not merely referenced.
    assert!(
        disassembly.contains("kelp_drop_LockGuard"),
        "the generated drop is missing:\n{disassembly}"
    );
    assert!(
        disassembly.contains("kelp_LockGuard__drop"),
        "the declared destructor is missing:\n{disassembly}"
    );

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding drop image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

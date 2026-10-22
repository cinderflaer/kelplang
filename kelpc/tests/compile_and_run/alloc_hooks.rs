use super::*;

// ---------------------------------------------------------------------------
// Kernel stage 7 (MK9-S5-M1, roadmap B-7): the global allocator hook. A
// freestanding manifest names `alloc`/`dealloc` (and optionally `realloc`), and
// `Box`/`List`/`Map` then allocate and free through them instead of libc. The
// rejection fixtures are front-end refusals, so they name the same construct on
// both backends; the builds and the boot are LLVM-only by design.
// ---------------------------------------------------------------------------

/// Kernel stage 7's acceptance build: a freestanding project that configures all
/// three allocator functions and instantiates `Box`, `List` and `Map`, linked
/// into an image with no undefined symbol. The IR beside the binary is checked
/// too: every allocation calls `@kelp_alloc`/`@kelp_realloc` and every free
/// `@kelp_dealloc`, with no `malloc`/`free` call at all.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_containers_allocate_through_the_project_allocator() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding allocator test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_alloc");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_alloc"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding allocator build failed:\n{}",
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
        "the freestanding allocator image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in [
        "@kelp_alloc(",
        "@kelp_dealloc(",
        "@kelp_realloc(",
        "@__kelp_freestanding_memset(",
    ] {
        assert!(
            ir.contains(symbol),
            "the container bodies should call '{symbol}'"
        );
    }
    for symbol in ["call ptr @malloc", "call ptr @realloc", "call void @free", "call ptr @memset", "call ptr @memcpy"] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// `Ptr.alloc` with the manifest's `[target] alloc`/`dealloc` pair lowers to the
/// project's own allocator, the same call `Box` and `List` make, and not to libc's
/// `malloc`. The image has no undefined symbol and the IR calls `@kelp_alloc`.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_ptr_alloc_goes_through_the_project_allocator() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding Ptr.alloc test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding(
        "freestanding_ptr_alloc_hook",
        &project("freestanding_ptr_alloc_hook"),
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the Ptr.alloc image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    assert!(
        ir.contains("@kelp_alloc("),
        "Ptr.alloc should call the project's '@kelp_alloc'"
    );
    assert!(
        !ir.contains("call ptr @malloc"),
        "a freestanding Ptr.alloc must not call libc's 'malloc'"
    );
}

/// The boot half: `Ptr.alloc` hands back an aligned block the program can write
/// and read back through the project's allocator.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_ptr_alloc_boots_on_the_project_allocator() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the Ptr.alloc boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_ptr_alloc_hook",
        &project("freestanding_ptr_alloc_hook"),
        &["ptr 0", "align 0", "val 1234"],
    );
}

/// `Ptr.free` is still refused in a freestanding build, even with the allocator
/// pair configured: a project's `dealloc` takes the block's byte size, which a
/// pointer does not carry.
#[test]
pub(crate) fn freestanding_ptr_free_is_refused_with_an_allocator() {
    assert_project_rejected(
        "freestanding_ptr_free_hook",
        &project("freestanding_ptr_free_hook"),
        "'Ptr#free' is unavailable in a freestanding build",
    );
}

/// Kernel stage 7's acceptance boot: `examples/kernel0-heap` defines a bump
/// allocator over a `static mut` arena, builds a `List<i32>` of 1..=10 with no
/// `realloc` (the alloc + copy + dealloc fallback), sums it and prints
/// `heap sum 55`, then prints `balanced` once every allocation has reached a
/// matching `dealloc`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_heap_boots_and_balances_its_allocations() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-heap boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_heap",
        &fixture("tests/fixtures/examples/kernel0-heap"),
        &["heap sum 55", "balanced"],
    );
}

/// Kernel stage 12's acceptance build: `examples/kernel0-string` builds a
/// `String` from a literal and through `str.concat`/`str.from_u64`, clones and
/// grows it, and grows a `Bytes` past several doublings, all on the project's
/// allocator. The `llvm-nm` line is the claim: no `strlen`, no `memcpy`, no
/// `malloc`/`realloc`/`free` may appear as undefined, and the IR must call the
/// project's own allocator and no libc allocation.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_string_builds_on_the_project_allocator_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-string build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_string");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(fixture("tests/fixtures/examples/kernel0-string"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-string build failed:\n{}",
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
        "the kernel0-string image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in ["@kelp_alloc(", "@kelp_realloc(", "@kelp_dealloc("] {
        assert!(
            ir.contains(symbol),
            "the buffer bodies should call the project allocator through '{symbol}'"
        );
    }
    for symbol in [
        "call ptr @malloc",
        "call ptr @realloc",
        "call void @free",
        "call i64 @strlen",
    ] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// Kernel stage 12's acceptance boot: `examples/kernel0-string` concatenates,
/// formats an integer and grows a `Bytes`, prints four lines through the NS16550
/// UART, and prints `balanced` only when every allocation reached a matching
/// free. The lines are QEMU's own stdout, not the source's.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_string_boots_and_prints_its_text_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-string boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_string",
        &fixture("tests/fixtures/examples/kernel0-string"),
        &["s: hello kelp!", "n: 12345", "b: 300", "balanced"],
    );
}

/// Kernel stage 23's acceptance build: `examples/kernel0-eq` compares a
/// `String`-field `datac` (a declared `eq` for `==`, and the derived key pair a
/// `Map<Pair, i32>` resolves through), slices a `str` view and copies the slice
/// with `View.to_string`, and runs a `std/regex` match. The `llvm-nm` line is
/// the claim that all of it is freestanding: `kelp_map_key_eq` and the two
/// `View` bodies are emitted into the module instead of being left for the
/// hosted runtime object, and no libc byte routine or allocator is called.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_eq_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-eq build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_eq");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(fixture("tests/fixtures/examples/kernel0-eq"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-eq build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let symbols = tool("llvm-nm", &[bin.to_str().expect("the artifact path is utf-8")]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the kernel0-eq image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in ["kelp_map_key_eq", "kelp_View__slice", "kelp_View__to_string"] {
        let defined = ir
            .lines()
            .any(|line| line.starts_with("define ") && line.contains(&format!("@{symbol}(")));
        assert!(
            defined,
            "a freestanding build should emit '@{symbol}' into the module"
        );
    }
    for symbol in [
        "call ptr @malloc",
        "call ptr @realloc",
        "call void @free",
        "call i64 @strlen",
    ] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// Kernel stage 23's acceptance boot: `examples/kernel0-eq` prints the
/// equality, `View` and `std/regex` results over the NS16550 UART, and prints
/// `balanced` only when every allocation reached a matching free. The lines are
/// QEMU's own stdout, not the source's.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_eq_boots_and_prints_the_equality_and_regex_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-eq boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_eq",
        &fixture("tests/fixtures/examples/kernel0-eq"),
        &[
            "eq same: yes",
            "eq differ: yes",
            "map keys: 2",
            "map hit: yes",
            "bytes eq: yes",
            "view slice: kelp",
            "view eq: yes",
            "regex match: hello42",
            "regex group: 42",
            "balanced",
        ],
    );
}

/// `List` with no `[target] alloc`/`dealloc` is refused exactly as before, with
/// a message that names the two keys whose absence is why.
#[test]
pub(crate) fn a_freestanding_list_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_list_no_alloc",
        "'[target] alloc' and '[target] dealloc'",
    );
}

/// Without an allocator `String` stays refused, with the message that names the
/// two manifest keys whose absence is why (kernel stage 12).
#[test]
pub(crate) fn a_freestanding_string_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_string_no_alloc",
        "'String', which is unavailable in a freestanding build",
    );
}

/// An allocator makes `String` legal, but a method with no libc-free lowering
/// is still refused by name at the call rather than left to the linker.
#[test]
pub(crate) fn a_freestanding_string_method_outside_the_libc_free_set_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_string",
        "needs the hosted runtime",
    );
}

/// `str.from_f64`'s formatting is `snprintf`, so a float is refused by name
/// even when the allocator makes `String` legal.
#[test]
pub(crate) fn a_freestanding_float_format_is_refused_by_name() {
    assert_rejected_with(
        "tests/projects/freestanding_string_float",
        "needs the hosted runtime",
    );
}

/// The `View` surface is a borrowed `{ptr, len}` pair with no storage of its
/// own, so `len`/`get`/`slice`/`eq` build in a freestanding image with no
/// allocator; only `View.to_string` copies into an owned `String` and so needs
/// the pair.
#[test]
pub(crate) fn a_freestanding_view_slice_and_eq_build_without_an_allocator() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding view test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_view");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_view"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of 'freestanding_view' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let undefined = tool("llvm-nm", &["-u", bin.to_str().unwrap()]);
    assert!(
        undefined.trim().is_empty(),
        "the freestanding view image has undefined symbols:\n{undefined}"
    );
}

/// `alloc` and `dealloc` are required together: naming one alone is a manifest
/// error rather than an allocator that can never free.
#[test]
pub(crate) fn an_allocator_without_a_dealloc_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_half",
        "'alloc' without 'dealloc'",
    );
}

/// A named allocator whose signature is not the `(size, align)` pair the
/// generated calls assume is refused with the expected signature.
#[test]
pub(crate) fn an_allocator_with_the_wrong_signature_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_bad_sig",
        "wrong signature",
    );
}

/// The hosted regression: with no `[target]` allocator configured, containers still
/// allocate, and every hosted build collects memory (decision 21). The program runs and
/// prints its sum, and (on the LLVM backend) the IR takes its buffers from the
/// collector, `@kelp_gc_alloc_array`, and never from libc `@malloc`.
#[test]
pub(crate) fn hosted_containers_allocate_through_the_collector() {
    let bin = out_path("hosted_list_alloc");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/hosted_list_alloc.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted list fixture failed to build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(run.status.success(), "the hosted list fixture failed to run");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "6\n");

    {
        let ir = std::fs::read_to_string(bin.with_extension("ll"))
            .expect("the LLVM backend writes textual IR beside the object");
        // `List.reserve` takes its buffer from the collector, and no hosted
        // container calls libc `malloc` for it (decision 21).
        assert!(
            ir.contains("@kelp_gc_alloc_array"),
            "a hosted container should allocate through the collector"
        );
        assert!(
            !ir.contains("call ptr @malloc"),
            "a hosted container should not allocate through libc malloc"
        );
    }
}

// ---------------------------------------------------------------------------
// Kernel gap 4: the freestanding runtime essentials. `_start` zeroes
// `[__bss_start, __bss_end)`, and the compiler defines the five `mem*`
// routines an aggregate copy calls, weakly and as loops. Freestanding is
// LLVM-only by design (the C backend's preamble is libc), so these tests build
// through the default backend; they need
// `ld.lld` and the llvm tools from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// Whether `llvm-mc` is on PATH. The weak-linkage test assembles the fixture
/// project's own riscv64 `memcpy` with it, because no C compiler on this host
/// targets that triple; the other kernel-gap-4 tests need only `ld.lld`.
pub(crate) fn have_llvm_mc() -> bool {
    Command::new("llvm-mc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Builds a freestanding project through the default (LLVM) backend and returns
/// the ELF's path. The object and IR sit beside it (`<name>.o`, `<name>.ll`),
/// which is what the object-level checks read.
pub(crate) fn build_freestanding(name: &str, target: &Path) -> PathBuf {
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
    bin
}

/// The address `llvm-nm` reports for `name`, as an integer.
pub(crate) fn nm_address(symbols: &str, name: &str) -> u64 {
    let line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some(name))
        .unwrap_or_else(|| panic!("the image has no symbol '{name}':\n{symbols}"));
    let address = line.split_whitespace().next().unwrap_or_default();
    u64::from_str_radix(address, 16)
        .unwrap_or_else(|_| panic!("the symbol '{name}' has no numeric address: {line}"))
}

/// The name of an `llvm-objdump` symbol header (`0000000080000034 <memcpy>:`).
pub(crate) fn disassembly_header(line: &str) -> Option<&str> {
    let (address, rest) = line.split_once(" <")?;
    if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    rest.strip_suffix(">:")
}

/// The instruction lines of the function named `name`, from its `<_name>:`
/// header to the next header that names another function. `llvm-objdump` gives
/// each `la` in `_start` its own `.Lpcrel_hi<n>` label, so a header with that
/// prefix is part of the body and not a boundary.
pub(crate) fn disassembled_body(disassembly: &str, name: &str) -> Vec<String> {
    let mut body = Vec::new();
    let mut inside = false;
    for line in disassembly.lines() {
        let line = line.trim_end();
        if let Some(header) = disassembly_header(line) {
            if inside {
                if header != name && !header.starts_with(".Lpcrel_hi") {
                    break;
                }
            } else {
                inside = header == name;
            }
            continue;
        }
        if inside && !line.trim().is_empty() {
            body.push(line.to_string());
        }
    }
    assert!(
        !body.is_empty(),
        "the disassembly has no '{name}' body:\n{disassembly}"
    );
    body
}

/// One `llvm-objdump -d --no-show-raw-insn` instruction line, split into its
/// address, mnemonic and comma-stripped operands.
pub(crate) fn disassembled_instruction(line: &str) -> Option<(u64, String, Vec<String>)> {
    let (address, rest) = line.split_once(':')?;
    let address = u64::from_str_radix(address.trim(), 16).ok()?;
    let mut tokens = rest.split_whitespace().map(|token| token.trim_matches(',').to_string());
    let mnemonic = tokens.next()?;
    Some((address, mnemonic, tokens.collect()))
}

/// A signed decimal or `0x`-prefixed immediate, the two spellings
/// `llvm-objdump` prints.
pub(crate) fn parse_immediate(text: &str) -> i64 {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("-0x") {
        -i64::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    } else if let Some(hex) = text.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    } else {
        text.parse().unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    }
}

/// The address a `la rd, symbol` sequence resolves to, read off the
/// disassembly: `auipc rd, hi` then `addi rd, rd, lo` is
/// `address(auipc) + (hi << 12) + lo`, and the linker resolved `symbol` into
/// exactly those two immediates.
pub(crate) fn la_target(body: &[(u64, String, Vec<String>)], register: &str) -> u64 {
    for pair in body.windows(2) {
        let (address, mnemonic, operands) = &pair[0];
        let (_, next_mnemonic, next_operands) = &pair[1];
        if mnemonic != "auipc" || operands.len() != 2 || operands[0] != register {
            continue;
        }
        if next_mnemonic != "addi"
            || next_operands.len() != 3
            || next_operands[0] != register
            || next_operands[1] != register
        {
            continue;
        }
        let high = parse_immediate(&operands[1]);
        let low = parse_immediate(&next_operands[2]);
        return (*address as i64).wrapping_add(high << 12).wrapping_add(low) as u64;
    }
    panic!("the body has no `la {register}, ...`: {body:?}");
}

/// Kernel gap 4's first acceptance test: a freestanding project whose entry
/// copies a 4 KiB `datac` (through its generated clone) and a `[u8; 4096]`
/// array. Both shapes lower to a `mem*` library call, which a bare-metal image
/// has no libc to define, so an image with no undefined symbol is the whole
/// claim. The `llvm-objdump -r` check pins the other half: the copy really does
/// reach a library routine the fallback has to supply.
#[test]
pub(crate) fn freestanding_memcpy_copies_a_large_aggregate_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding memcpy test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("freestanding_memcpy", &project("freestanding_memcpy"));
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let object = bin.with_extension("o");
    let relocations = tool(
        "llvm-objdump",
        &["-r", object.to_str().expect("the object path is utf-8")],
    );
    assert!(
        relocations.contains("memmove"),
        "the 4 KiB copies did not lower to a library call at all:\n{relocations}"
    );
}

/// Kernel gap 4's second acceptance test: the object defines all five `mem*`
/// routines as weak symbols, so an image that carries its own wins at the link,
/// and each body is a loop. The self-call check is the decisive one: LLVM's
/// loop-idiom pass rewrites a loop into a call to the library function of the
/// same name, which for `memcpy` is the function the loop is already in, and
/// `nobuiltin` plus the definition's own name are what stop it.
#[test]
pub(crate) fn the_freestanding_mem_routines_are_weak_and_do_not_call_themselves() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding mem routines test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("freestanding_mem_weak", &project("freestanding_memcpy"));
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    for name in ["memcpy", "memmove", "memset", "memcmp", "bcmp"] {
        let line = symbols
            .lines()
            .find(|line| line.split_whitespace().last() == Some(name))
            .unwrap_or_else(|| panic!("the image does not define '{name}':\n{symbols}"));
        assert_eq!(
            line.split_whitespace().nth(1),
            Some("W"),
            "'{name}' is not a weak definition: {line}"
        );
    }

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    for name in ["memcpy", "memmove", "memset", "memcmp", "bcmp"] {
        let body = disassembled_body(&disassembly, name);
        let self_call = body.iter().find(|line| {
            (line.contains("jal") || line.contains("call")) && line.contains(&format!("<{name}"))
        });
        assert!(
            self_call.is_none(),
            "'{name}' calls itself:\n{}",
            body.join("\n")
        );
    }
}

/// Kernel gap 4's third acceptance test: `_start` really clears the range the
/// linker script exported. The two registers the clear loop loads are decoded
/// back out of the disassembly and compared against the addresses `llvm-nm`
/// reports for `__bss_start` and `__bss_end`, so the loop cannot be iterating
/// some other range and passing by luck. `kernel0-bss` gives the range real
/// content; the stub is the same in every freestanding image.
#[test]
pub(crate) fn the_freestanding_entry_clears_between_the_bss_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding entry-clear test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_bss_entry", &fixture("tests/fixtures/examples/kernel0-bss"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let symbols = tool("llvm-nm", &[path]);
    let bss_start = nm_address(&symbols, "__bss_start");
    let bss_end = nm_address(&symbols, "__bss_end");
    assert!(
        bss_end > bss_start,
        "the fixture's bss range is empty: {bss_start:#x}..{bss_end:#x}"
    );

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    let body = disassembled_body(&disassembly, "_start");
    let instructions: Vec<(u64, String, Vec<String>)> = body
        .iter()
        .filter_map(|line| disassembled_instruction(line))
        .collect();

    assert_eq!(
        la_target(&instructions, "t0"),
        bss_start,
        "_start does not load __bss_start into t0:\n{}",
        body.join("\n")
    );
    assert_eq!(
        la_target(&instructions, "t1"),
        bss_end,
        "_start does not load __bss_end into t1:\n{}",
        body.join("\n")
    );
    assert!(
        instructions
            .iter()
            .any(|(_, mnemonic, operands)| mnemonic == "sd"
                && operands.first().map(String::as_str) == Some("zero")),
        "the clear loop has no word store:\n{}",
        body.join("\n")
    );
    assert!(
        instructions.iter().any(|(_, mnemonic, _)| mnemonic == "bgeu"),
        "the clear loop has no compare:\n{}",
        body.join("\n")
    );
}

/// Kernel gap 4's weak-linkage test: a project whose own `memcpy` is a strong
/// definition links with that one, not the compiler's weak fallback. The
/// project ships `user_memcpy.s`; the test assembles it for the same triple
/// with `llvm-mc` (no C compiler here targets riscv64) and relinks the object
/// `kelpc` emitted beside it, then reads back which definition the link chose.
#[test]
pub(crate) fn a_project_that_defines_its_own_memcpy_links_with_that_one() {
    if !have_ld_lld() || !have_llvm_mc() {
        println!(
            "skipping the freestanding memcpy-override test: 'ld.lld' or 'llvm-mc' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let target = project("freestanding_memcpy_user");
    let bin = build_freestanding("freestanding_memcpy_user", &target);

    let user_object = out_path("freestanding_memcpy_user_object");
    let assembled = Command::new("llvm-mc")
        .arg("-triple=riscv64-unknown-none-elf")
        .arg("-filetype=obj")
        .arg("-o")
        .arg(&user_object)
        .arg(target.join("user_memcpy.s"))
        .output()
        .expect("failed to spawn llvm-mc");
    assert!(
        assembled.status.success(),
        "llvm-mc failed on the project's own memcpy:\n{}",
        String::from_utf8_lossy(&assembled.stderr)
    );

    let relinked = out_path("freestanding_memcpy_user_relinked");
    let linked = Command::new("ld.lld")
        .arg("-T")
        .arg(target.join("kernel.ld"))
        .arg("-o")
        .arg(&relinked)
        .arg("--static")
        .arg(bin.with_extension("o"))
        .arg(&user_object)
        .output()
        .expect("failed to spawn ld.lld");
    assert!(
        linked.status.success(),
        "the relink with the project's own memcpy failed:\n{}",
        String::from_utf8_lossy(&linked.stderr)
    );

    let path = relinked.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let memcpy_line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("memcpy"))
        .unwrap_or_else(|| panic!("the relinked image has no memcpy:\n{symbols}"));
    assert_eq!(
        memcpy_line.split_whitespace().nth(1),
        Some("T"),
        "the compiler's weak memcpy won over the project's own: {memcpy_line}"
    );
    let memmove_line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("memmove"))
        .unwrap_or_else(|| panic!("the relinked image has no memmove:\n{symbols}"));
    assert_eq!(
        memmove_line.split_whitespace().nth(1),
        Some("W"),
        "the untouched fallback is no longer weak: {memmove_line}"
    );

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    let body = disassembled_body(&disassembly, "memcpy").join("\n");
    assert!(
        body.contains("0x5eed"),
        "the linked memcpy is not the project's own body:\n{body}"
    );
}

/// Kernel gap 4's runtime proof. QEMU's RAM starts zeroed, so the example's
/// marker is a 64-byte pattern the image itself loads non-zero into a
/// `PROGBITS` section the script places inside the cleared range: `bss clear`
/// can only come out of the UART if `_start` really erased it. The ELF checks
/// pin the other half, that the pattern is in the image and covers exactly the
/// range the clear loop was pointed at, so the boot cannot pass vacuously.
#[test]
pub(crate) fn kernel0_bss_clears_an_image_loaded_marker_before_the_entry_runs() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 bss test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let target = fixture("tests/fixtures/examples/kernel0-bss");
    let bin = build_freestanding("kernel0_bss_image", &target);
    let path = bin.to_str().expect("the artifact path is utf-8");

    let sections = tool("llvm-readelf", &["-S", path]);
    let bss = sections
        .lines()
        .find(|line| line.split_whitespace().any(|token| token == ".bss"))
        .unwrap_or_else(|| panic!("the image has no .bss section:\n{sections}"));
    assert!(
        bss.contains("PROGBITS"),
        "the .bss is not image-loaded, so the boot would prove nothing:\n{bss}"
    );

    let symbols = tool("llvm-nm", &[path]);
    let bss_start = nm_address(&symbols, "__bss_start");
    let bss_end = nm_address(&symbols, "__bss_end");
    let marker = nm_address(&symbols, "kelp_bss_marker");
    // The marker is the first thing in `.bss`, which the project's script
    // orders `.bss.dirty` first. It is no longer the *whole* cleared range:
    // kernel stage 15 put the panic entry's `.bss` state (`kelp_panic_active`,
    // `kelp_panic_file`, `kelp_panic_line`) after it, so the assertion is that
    // the range starts at the marker and covers all 64 bytes of it.
    assert_eq!(
        bss_start, marker,
        "the marker is not the first byte of the cleared range: \
         {bss_start:#x}..{marker:#x}..{bss_end:#x}"
    );
    assert!(
        bss_end >= marker + 64,
        "the cleared range does not cover the whole marker: \
         {bss_start:#x}..{marker:#x}..{bss_end:#x}"
    );

    let contents = tool("llvm-objdump", &["-s", "-j", ".bss", path]);
    assert!(
        contents.contains("abababab"),
        "the image does not carry the marker pattern:\n{contents}"
    );

    boot_under_qemu(&RISCV64_QEMU, "kernel0_bss", &target, &["bss clear"]);
}

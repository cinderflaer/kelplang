use super::*;

/// One compilation unit of the incremental mode (roadmap A-14 slice 1).
#[derive(Clone)]
pub(crate) struct Unit {
    /// The unit's name: a source module's canonical id, or `_support`.
    pub(crate) name: String,
    /// `_support` defines the generated helpers; every other unit only declares
    /// them.
    pub(crate) support: bool,
    /// The program items this unit defines that another unit references, so
    /// their definitions keep external linkage.
    pub(crate) exported: HashSet<String>,
}

/// The name of the `_support` unit: the one unit whose name is not a module id,
/// because it owns every compiler-generated helper.
pub(crate) const SUPPORT_UNIT: &str = "_support";

/// The incremental mode's unit list, in a deterministic order: every module a
/// program item is declared in, sorted by id, then `_support` last.
///
/// A unit's `exported` set is the program items it defines that some *other*
/// unit references, plus `main`; those keep external linkage. Everything else a
/// unit defines is internal, which is what lets LLVM optimize and drop within a
/// unit the way it does in the whole-program build. Over-approximating the set
/// is harmless (the symbol is merely visible); a missed reference is a link
/// error rather than a miscompile, and the equivalence tests link real binaries.
pub(crate) fn plan_units(program: &Program) -> Vec<Unit> {
    let mut modules: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in &program.functions {
        modules.insert(f.module.clone());
    }
    for s in &program.statics {
        modules.insert(s.module.clone());
    }
    for c in &program.const_data {
        modules.insert(c.module.clone());
    }

    // Which unit references which program item. A reference from a generated
    // helper or a `pd` initializer is `_support`'s, because that is the unit
    // whose code emits the call.
    let mut refs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut note = |key: &str, from: &str| {
        refs.entry(key.to_string())
            .or_default()
            .insert(from.to_string());
    };
    for f in &program.functions {
        if f.extern_ {
            continue;
        }
        collect_block_refs(&f.body, &f.module, &mut note);
    }
    for pd in &program.pds {
        collect_expr_refs(&pd.init, SUPPORT_UNIT, &mut note);
    }
    // Every function a *known* function value is widened from gets a shim in
    // `_support` that calls it directly, the same set `declare_closure_fns`
    // declares (`inst.shims`): the widening is one of the generated helpers even
    // though the reference that created it was written in the function's own
    // unit.
    for (key, _) in &collect_instantiations(program).shims {
        note(key, SUPPORT_UNIT);
    }
    for vtable in &program.dyn_vtables {
        for slot in &vtable.slots {
            note(&slot.target, SUPPORT_UNIT);
        }
    }
    for name in &program.drop_types {
        note(&crate::types::drop_method_key(name), SUPPORT_UNIT);
    }
    for name in &program.declared_keys {
        note(
            &crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_HASH_METHOD),
            SUPPORT_UNIT,
        );
        note(
            &crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_EQ_METHOD),
            SUPPORT_UNIT,
        );
    }

    let mut owner: HashMap<&str, &str> = HashMap::new();
    for f in &program.functions {
        owner.insert(f.name.as_str(), f.module.as_str());
    }
    for s in &program.statics {
        owner.insert(s.name.as_str(), s.module.as_str());
    }

    let mut units: Vec<Unit> = modules
        .into_iter()
        .map(|name| Unit {
            name,
            support: false,
            exported: HashSet::new(),
        })
        .collect();
    for (key, from) in &refs {
        let Some(owner) = owner.get(key.as_str()) else {
            continue;
        };
        if !from.iter().any(|u| u != owner) {
            continue;
        }
        if let Some(unit) = units.iter_mut().find(|u| &u.name == owner) {
            unit.exported.insert(key.clone());
        }
    }
    // `main` is called by the process startup code, not by a unit, so nothing in
    // the program references it and the reference scan above cannot see it.
    if let Some(main) = program.functions.iter().find(|f| f.name == "main") {
        if let Some(unit) = units.iter_mut().find(|u| u.name == main.module) {
            unit.exported.insert("main".to_string());
        }
    }

    units.push(Unit {
        name: SUPPORT_UNIT.to_string(),
        support: true,
        exported: HashSet::new(),
    });
    units
}

/// A unit's name as a file name: a module id is a path (`shapes/circle`) and a
/// file name is not, so `/` becomes `+`. A module id is a path of identifiers,
/// so its characters are `[A-Za-z0-9_]` and the mapping is injective: two units
/// never share an artifact name. The IR module name is still the id, so folding
/// the artifact name cannot make two units' IR disagree.
pub(crate) fn unit_file_name(name: &str) -> String {
    name.replace('/', "+")
}

/// Every program item one function body references, reported through
/// `note(key, unit)`: a call, a function value, a closure literal, a `static`,
/// or a `pd` global.
pub(crate) fn collect_block_refs(block: &Block, from: &str, note: &mut impl FnMut(&str, &str)) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { init, .. } => collect_expr_refs(init, from, note),
            Stmt::While { cond, body, .. } => {
                collect_expr_refs(cond, from, note);
                collect_block_refs(body, from, note);
            }
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expr_refs(value, from, note);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Expr(expr) => collect_expr_refs(expr, from, note),
        }
    }
    if let Some(tail) = &block.tail {
        collect_expr_refs(tail, from, note);
    }
}

pub(crate) fn collect_expr_refs(expr: &Expr, from: &str, note: &mut impl FnMut(&str, &str)) {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            note(callee, from);
            args.iter().for_each(|a| collect_expr_refs(a, from, note));
        }
        ExprKind::VariadicCall {
            callee,
            wrapper,
            args,
        } => {
            note(callee, from);
            note(wrapper, from);
            args.iter().for_each(|a| collect_expr_refs(a, from, note));
        }
        ExprKind::FnRef(key) => note(key, from),
        ExprKind::ClosureLit { key, captures } | ExprKind::ScopedClosureLit { key, captures } => {
            note(key, from);
            captures.iter().for_each(|c| collect_expr_refs(c, from, note));
        }
        ExprKind::StaticRef(name) => note(name, from),
        // A `pd` read names its global directly: `Ident("pd#<name>")` is what
        // `FnGen::pd_place` resolves when a function receives no hidden
        // parameter of its own.
        ExprKind::Ident(name) if name.starts_with("pd#") => note(name, from),
        ExprKind::Unary { expr, .. } | ExprKind::Deref { expr } | ExprKind::EnumTag { expr, .. } => {
            collect_expr_refs(expr, from, note)
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_refs(lhs, from, note);
            collect_expr_refs(rhs, from, note);
        }
        ExprKind::FieldAccess { base, .. } => collect_expr_refs(base, from, note),
        ExprKind::StructLit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_refs(value, from, note);
            }
        }
        ExprKind::EnumLit { fields, .. } => {
            fields.iter().for_each(|f| collect_expr_refs(f, from, note))
        }
        ExprKind::EnumField { base, .. } => collect_expr_refs(base, from, note),
        ExprKind::Try { expr, convert, .. } => {
            collect_expr_refs(expr, from, note);
            if let Some((key, _)) = convert {
                note(key, from);
            }
        }
        ExprKind::Assign { target, value } => {
            collect_expr_refs(target, from, note);
            collect_expr_refs(value, from, note);
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_expr_refs(cond, from, note);
            collect_block_refs(then_block, from, note);
            if let Some(otherwise) = else_block {
                collect_block_refs(otherwise, from, note);
            }
        }
        ExprKind::Block(block) => collect_block_refs(block, from, note),
        ExprKind::Region { body, .. } => collect_block_refs(body, from, note),
        ExprKind::FnToClosure { func } => collect_expr_refs(func, from, note),
        ExprKind::IndirectCall { callee, args } => {
            collect_expr_refs(callee, from, note);
            args.iter().for_each(|a| collect_expr_refs(a, from, note));
        }
        ExprKind::Ref { expr, .. } => collect_expr_refs(expr, from, note),
        ExprKind::DynCoerce { expr, .. }
        | ExprKind::BoxDynCoerce { expr, .. }
        | ExprKind::BoxDynBorrow { expr, .. }
        | ExprKind::BoxDynIsEmpty { expr } => collect_expr_refs(expr, from, note),
        ExprKind::DynCall { receiver, args, .. } => {
            collect_expr_refs(receiver, from, note);
            args.iter().for_each(|a| collect_expr_refs(a, from, note));
        }
        ExprKind::Template { pieces, .. } => {
            for piece in pieces {
                if let TemplatePiece::Value(value) = piece {
                    collect_expr_refs(value, from, note);
                }
            }
        }
        ExprKind::ArrayLit(elems) => elems.iter().for_each(|e| collect_expr_refs(e, from, note)),
        ExprKind::ArrayRepeat { value } => collect_expr_refs(value, from, note),
        ExprKind::Asm(block) => {
            for operand in &block.operands {
                if let Some(operand) = &operand.operand {
                    collect_expr_refs(operand, from, note);
                }
            }
        }
        ExprKind::IntLit(_)
        | ExprKind::BlockBox { .. }
        | ExprKind::CharLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Capture { .. }
        | ExprKind::Ident(_) => {}
    }
}

/// Everything the LLVM backend needs to know about a freestanding project
/// (kernel stage 4): the target triple, the entry function's bare Kelp name,
/// the emitted name of the project's panic handler (or `None` for the default
/// infinite-loop handler) and the linker script, already resolved to an
/// absolute path.
#[derive(Clone)]
pub struct Freestanding {
    pub triple: String,
    pub entry: String,
    pub panic: Option<String>,
    pub linker_script: String,
    /// The global allocator (kernel stage 7, MK9-S5-M1): the entry module's
    /// bare `alloc`, `dealloc` and optional `realloc` names. All `None` when the
    /// manifest configures none, which keeps the heap types refused.
    pub alloc: Option<String>,
    pub dealloc: Option<String>,
    pub realloc: Option<String>,
    /// The manifest's `[target] cpus` (kernel stage 8): how many copies a
    /// `percpu` static is laid out as, and the bound `cpu_index()` is checked
    /// against in a debug build.
    pub cpus: u64,
}

/// The project's own allocator, resolved to the LLVM functions the container
/// bodies call instead of libc's `malloc`/`realloc`/`free` (kernel stage 7,
/// MK9-S5-M1). `alloc` takes `(size, align)` and returns a nullable pointer;
/// `dealloc` takes `(ptr, size, align)`; `realloc` takes
/// `(ptr, old, new, align)`. Without one, every helper falls back to libc.
#[derive(Clone, Copy)]
pub(crate) struct AllocFns<'a> {
    pub(crate) alloc: (LlvmFunction<'a>, FnType<'a>),
    pub(crate) dealloc: (LlvmFunction<'a>, FnType<'a>),
    pub(crate) realloc: Option<(LlvmFunction<'a>, FnType<'a>)>,
}

/// The `_start` stub a freestanding build emits as module-level assembly
/// (kernel stage 4), chosen by the manifest's `[target] triple`. It sets `sp`
/// from the linker-script symbol `__stack_top`, zeroes
/// `[__bss_start, __bss_end)` a word at a time (kernel gap 4), calls `entry` and
/// loops on the target's wait-for-interrupt instruction if it returns. It goes
/// in `.text.boot` so a linker script can place it first; `entry_symbol` is the
/// mangled name of the project's entry `fun` (`kelp_kmain`).
///
/// The two bounds are declared `.weak` so a script that does not assign them
/// still links: both then resolve to 0, the loop runs zero times, and the image
/// behaves exactly as it did before this stage. `sp` is loaded first and nothing
/// touches memory before it, which is what lets the clear run on a kernel's
/// first instruction. Only registers are used until the entry is called, so no
/// stack has to exist yet.
///
/// Loading `sp` from a linker-script symbol is not expressible in the language,
/// so each architecture gets its own hand-written stub. `Err` names the triple
/// for an architecture this compiler has no stub for, rather than emitting an
/// image whose entry point is another ISA's code.
pub(crate) fn freestanding_entry_asm(triple: &str, entry_symbol: &str) -> Result<String, String> {
    if triple.starts_with("riscv64") || triple.starts_with("riscv32") {
        Ok(riscv64_entry_asm(entry_symbol))
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        Ok(aarch64_entry_asm(entry_symbol))
    } else if triple.starts_with("x86_64") {
        Ok(x86_64_entry_asm(entry_symbol))
    } else {
        Err(format!(
            "kelpc has no freestanding '_start' stub for the triple '{triple}': it has one for riscv64, aarch64 and x86_64"
        ))
    }
}

/// The target's wait-for-interrupt instruction, for the double-panic halt and
/// the default panic loop's tail. The same architecture split as the entry
/// stub: riscv64 halts on `wfi`, aarch64 on `wfe`, and a triple this compiler
/// has neither for is refused by name here exactly as the stub refuses it.
pub(crate) fn halt_asm_template(triple: &str) -> Result<&'static str, String> {
    if triple.starts_with("riscv64") || triple.starts_with("riscv32") {
        Ok("wfi")
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        Ok("wfe")
    } else if triple.starts_with("x86_64") {
        Ok("hlt")
    } else {
        Err(format!(
            "kelpc has no freestanding wait-for-interrupt instruction for the triple '{triple}': it has 'wfi' for riscv64, 'wfe' for aarch64 and 'hlt' for x86_64"
        ))
    }
}

/// The riscv64 stub: park every hart but hart 0, then `la sp, __stack_top`,
/// the word-at-a-time clear, the entry call, and a `wfi` halt.
///
/// QEMU's `-bios none` starts every hart at the entry with no firmware to park
/// the secondaries, so a kernel built with `-smp 2` (kernel stage 8) would have
/// both harts racing through the boot path. The `mhartid` test is the first
/// instruction: a hart whose id is not zero waits on `wfi` before the stack is
/// loaded, so it never touches memory and never reaches the entry. A single
/// hart (the default `-smp 1`) reads 0 and falls through.
pub(crate) fn riscv64_entry_asm(entry_symbol: &str) -> String {
    format!(
        "    .section .text.boot, \"ax\", @progbits\n\
         \x20   .weak __bss_start\n\
         \x20   .weak __bss_end\n\
         \x20   .globl _start\n\
         \x20   .type _start, @function\n\
         _start:\n\
         \x20   csrr t0, mhartid\n\
         \x20   bnez t0, .Lkelp_park\n\
         \x20   la sp, __stack_top\n\
         \x20   la t0, __bss_start\n\
         \x20   la t1, __bss_end\n\
         .Lkelp_bss_loop:\n\
         \x20   bgeu t0, t1, .Lkelp_bss_done\n\
         \x20   sd zero, 0(t0)\n\
         \x20   addi t0, t0, 8\n\
         \x20   j .Lkelp_bss_loop\n\
         .Lkelp_bss_done:\n\
         \x20   call {entry_symbol}\n\
         1:\n\
         \x20   wfi\n\
         \x20   j 1b\n\
         .Lkelp_park:\n\
         \x20   wfi\n\
         \x20   j .Lkelp_park\n\
         \x20   .size _start, . - _start\n"
    )
}

/// The aarch64 stub, the same shape as the riscv64 one in AArch64's own
/// assembly: `ldr x0, =__stack_top` loads the symbol's absolute address into
/// `x0`, which `mov sp, x0` makes the stack pointer. The clear is a
/// post-indexed `str xzr, [x0], #8` while `x0` is below `x1` (unsigned), so it
/// is the same word-at-a-time loop. `wfe` is the halt; `b 1b` spins on it.
pub(crate) fn aarch64_entry_asm(entry_symbol: &str) -> String {
    format!(
        "    .section .text.boot, \"ax\", @progbits\n\
         \x20   .weak __bss_start\n\
         \x20   .weak __bss_end\n\
         \x20   .globl _start\n\
         \x20   .type _start, @function\n\
         _start:\n\
         \x20   ldr x0, =__stack_top\n\
         \x20   mov sp, x0\n\
         \x20   ldr x0, =__bss_start\n\
         \x20   ldr x1, =__bss_end\n\
         .Lkelp_bss_loop:\n\
         \x20   cmp x0, x1\n\
         \x20   b.hs .Lkelp_bss_done\n\
         \x20   str xzr, [x0], #8\n\
         \x20   b .Lkelp_bss_loop\n\
         .Lkelp_bss_done:\n\
         \x20   bl {entry_symbol}\n\
         1:\n\
         \x20   wfe\n\
         \x20   b 1b\n\
         \x20   .size _start, . - _start\n"
    )
}

/// The x86-64 stub. QEMU's `-kernel` on x86 loads an ELF64 image through the
/// Xen PVH note and enters the 32-bit physical address the note names, in
/// 32-bit protected mode with paging off, so this stub has to leave 32-bit mode
/// before it can call the entry: `.note.Xen` holds the PVH note (type 18) whose
/// description is `_start`, `_start` loads a three-entry GDT, builds an
/// identity map for the first 1 GiB out of 2 MiB pages in the `.boot_pt` NOBITS
/// section (zeroed here, because the `.bss` clear must not touch the tables it
/// runs under), enables PAE and `EFER.LME`, loads `CR3`, sets `CR0.PG|PE`, and
/// far-jumps (`0xEA`) into the `.code64` half. That half reloads the data
/// segments, loads `rsp` from `__stack_top`, clears `[__bss_start, __bss_end)`,
/// enables SSE (`CR0.EM` clear, `CR0.MP` set, `CR4.OSFXSR` and
/// `CR4.OSXMMEXCPT` set, `MXCSR` to its reset value) so the compiler's SSE
/// lowering is safe, calls the entry, and spins on `hlt` if it returns. The
/// note is its own `.note.Xen` section and `_start` is the first thing in
/// `.text.boot`, which the linker script places at the image's 1 MiB base.
pub(crate) fn x86_64_entry_asm(entry_symbol: &str) -> String {
    format!(
        "    .section .note.Xen, \"a\", @note\n\
         \x20   .align 4\n\
         \x20   .long 4\n\
         \x20   .long 4\n\
         \x20   .long 18\n\
         \x20   .asciz \"Xen\"\n\
         \x20   .align 4\n\
         \x20   .long _start\n\
         \x20   .align 4\n\
         \x20   .section .text.boot, \"ax\", @progbits\n\
         \x20   .weak __bss_start\n\
         \x20   .weak __bss_end\n\
         \x20   .globl _start\n\
         \x20   .type _start, @function\n\
         _start:\n\
         \x20   .code32\n\
         \x20   cli\n\
         \x20   leal gdt_desc, %eax\n\
         \x20   lgdt (%eax)\n\
         \x20   movw $0x10, %ax\n\
         \x20   movw %ax, %ds\n\
         \x20   movw %ax, %es\n\
         \x20   movw %ax, %ss\n\
         \x20   leal __boot_pt_start, %edi\n\
         \x20   leal __boot_pt_end, %ecx\n\
         \x20   subl %edi, %ecx\n\
         \x20   shrl $2, %ecx\n\
         \x20   xorl %eax, %eax\n\
         \x20   cld\n\
         \x20   rep stosl\n\
         \x20   leal __boot_pdpt, %eax\n\
         \x20   orl $0x3, %eax\n\
         \x20   leal __boot_pml4, %edi\n\
         \x20   movl %eax, (%edi)\n\
         \x20   leal __boot_pd, %eax\n\
         \x20   orl $0x3, %eax\n\
         \x20   leal __boot_pdpt, %edi\n\
         \x20   movl %eax, (%edi)\n\
         \x20   leal __boot_pd, %edi\n\
         \x20   xorl %ecx, %ecx\n\
         \x20   movl $0x83, %eax\n\
         .Lkelp_pd_loop:\n\
         \x20   movl %eax, (%edi,%ecx,8)\n\
         \x20   addl $0x200000, %eax\n\
         \x20   incl %ecx\n\
         \x20   cmpl $512, %ecx\n\
         \x20   jne .Lkelp_pd_loop\n\
         \x20   movl %cr4, %eax\n\
         \x20   orl $0x20, %eax\n\
         \x20   movl %eax, %cr4\n\
         \x20   movl $0xC0000080, %ecx\n\
         \x20   rdmsr\n\
         \x20   orl $0x100, %eax\n\
         \x20   wrmsr\n\
         \x20   movl $__boot_pml4, %eax\n\
         \x20   movl %eax, %cr3\n\
         \x20   movl %cr0, %eax\n\
         \x20   orl $0x80000001, %eax\n\
         \x20   movl %eax, %cr0\n\
         \x20   .byte 0xEA\n\
         \x20   .long .Lkelp_long\n\
         \x20   .word 0x08\n\
         \x20   .code64\n\
         .Lkelp_long:\n\
         \x20   movw $0x10, %ax\n\
         \x20   movw %ax, %ds\n\
         \x20   movw %ax, %es\n\
         \x20   movw %ax, %ss\n\
         \x20   movq $__stack_top, %rsp\n\
         \x20   movq $__bss_start, %rdi\n\
         \x20   movq $__bss_end, %rcx\n\
         \x20   subq %rdi, %rcx\n\
         \x20   addq $7, %rcx\n\
         \x20   shrq $3, %rcx\n\
         \x20   xorq %rax, %rax\n\
         \x20   cld\n\
         \x20   rep stosq\n\
         \x20   movq %cr0, %rax\n\
         \x20   andq $-5, %rax\n\
         \x20   orq $0x2, %rax\n\
         \x20   movq %rax, %cr0\n\
         \x20   movq %cr4, %rax\n\
         \x20   orq $0x600, %rax\n\
         \x20   movq %rax, %cr4\n\
         \x20   movl $0x1F80, %eax\n\
         \x20   pushq %rax\n\
         \x20   ldmxcsr (%rsp)\n\
         \x20   addq $8, %rsp\n\
         \x20   call {entry_symbol}\n\
         1:\n\
         \x20   hlt\n\
         \x20   jmp 1b\n\
         \x20   .size _start, . - _start\n\
         \x20   .section .rodata.boot, \"a\", @progbits\n\
         \x20   .align 8\n\
         gdt:\n\
         \x20   .quad 0x0000000000000000\n\
         \x20   .quad 0x00AF9A000000FFFF\n\
         \x20   .quad 0x00CF92000000FFFF\n\
         gdt_desc:\n\
         \x20   .word 0x17\n\
         \x20   .long gdt\n\
         \x20   .section .boot_pt, \"aw\", @nobits\n\
         \x20   .align 4096\n\
         __boot_pt_start:\n\
         __boot_pml4:\n\
         \x20   .zero 4096\n\
         __boot_pdpt:\n\
         \x20   .zero 4096\n\
         __boot_pd:\n\
         \x20   .zero 4096\n\
         __boot_pt_end:\n"
    )
}

/// One declared callable: the LLVM function, its LLVM type, and the `tir` return
/// type (which decides whether a call yields a value). A clone is cheap and keeps
/// the borrow of the function table out of the walk.
#[derive(Clone)]
pub(crate) struct FnInfo<'a> {
    pub(crate) func: LlvmFunction<'a>,
    pub(crate) ty: FnType<'a>,
    pub(crate) ret_ty: Ty,
}

/// One module-scope `static`'s LLVM global, its `tir` type, and, for a
/// `percpu` static (kernel stage 8), how to index it. `percpu` is `Some` when
/// the global is an `[N x stride]` array; a reference computes the address of
/// the `cpu_index()`-th element rather than using the global directly.
#[derive(Clone)]
pub(crate) struct StaticGlobal<'a> {
    pub(crate) global: Value<'a>,
    pub(crate) ty: Ty,
    pub(crate) percpu: Option<PercpuStatic<'a>>,
}

/// How one `percpu` static's copies are laid out: the array's element type
/// (the `tir` type, or a `{ T, pad }` wrapper when its size is not a multiple
/// of the 64-byte cache line) and whether that wrapper is in use.
#[derive(Clone, Copy)]
pub(crate) struct PercpuStatic<'a> {
    pub(crate) stride_ty: Type<'a>,
    pub(crate) padded: bool,
}

/// One runtime helper the IR calls by declaration: the function, its LLVM type,
/// whether it yields a value (a `void` one must not be given a result name), the
/// struct it produces, and whether that production is the C ABI's hidden return
/// pointer.
#[derive(Clone, Copy)]
pub(crate) struct RtFn<'a> {
    pub(crate) func: LlvmFunction<'a>,
    pub(crate) ty: FnType<'a>,
    pub(crate) yields: bool,
    pub(crate) ret: Option<Type<'a>>,
    /// True when the C function returns a struct larger than two eightbytes,
    /// which the SysV ABI returns through a hidden pointer in `rdi`. clang marks
    /// that pointer `sret` in the IR it emits; this backend models it as an
    /// explicit first `ptr` parameter instead, which produces the *same machine
    /// call* (the destination lands in `rdi`, the real arguments follow) without
    /// growing the wrapper with a parameter-attribute API. The declared type is
    /// therefore the C signature's ABI shape rather than its written spelling.
    pub(crate) sret: bool,
}

/// One gated call site's check plan, the mirror of what
/// the capability plan returns: the domain to match against, how many
/// patterns the calling module holds for it, the call site as written, and
/// which arguments are resource names.
pub(crate) struct CapPlan {
    pub(crate) domain: String,
    pub(crate) pattern_count: usize,
    /// The calling module's whole compiled-in pattern list for `domain`, text
    /// and all, not just its count: `emit_cap_checks` inspects it to decide
    /// whether a `CapCheck::Name` here is one of the two fast-path shapes
    /// the runtime capability glue's own `kelp_cap_check_name` already fast-paths (the lone
    /// `*`, or a literal with no `*`), which can be hand-built as inline IR
    /// instead of a call into that separately-compiled object.
    pub(crate) patterns: Vec<String>,
    pub(crate) at: String,
    pub(crate) checks: Vec<crate::stdlib::CapCheck>,
}

/// Which fast path, if any, a `CapCheck::Name` at one call site can take,
/// decided from the calling module's whole pattern list for the domain: every
/// `CapCheck::Name` inside one `CapPlan` shares the same grant, so the list is
/// read once per plan rather than once per check. `None` covers every shape
/// this task leaves alone: more than one pattern, or a single pattern whose
/// `*` is not the lone wildcard (a real glob, still matched through
/// `kelp_std__rt__glob_match` exactly as before).
pub(crate) enum NameFastPath<'p> {
    /// The pattern list is exactly `["*"]`. the runtime capability glue's own fast path reads
    /// this as "the name has no `/`", so the inline IR is the same scan.
    Wildcard,
    /// The pattern list is exactly one pattern with no `*` byte anywhere: the
    /// check is byte-for-byte identity between the checked name and it.
    Literal(&'p str),
}

/// Whether `patterns` (a calling module's whole compiled-in allowlist for one
/// domain) is one of the two shapes the runtime capability glue's `kelp_cap_check_name`
/// already fast-paths, so `emit_cap_checks` can hand-build the check as
/// inline IR instead of emitting a call into that separately-compiled object.
pub(crate) fn name_fast_path(patterns: &[String]) -> Option<NameFastPath<'_>> {
    if patterns.len() != 1 {
        return None;
    }
    let pattern = patterns[0].as_str();
    if pattern == "*" {
        Some(NameFastPath::Wildcard)
    } else if !pattern.contains('*') {
        Some(NameFastPath::Literal(pattern))
    } else {
        None
    }
}

/// One `enum`'s LLVM layout, mirroring the `{ tag, union }` typedef
/// the enum emitter emits (MK7 §3.1).
#[derive(Clone)]
pub(crate) struct EnumInfo<'a> {
    /// The outer struct: field 0 is the tag, field 1 the union.
    pub(crate) llvm_ty: Type<'a>,
    /// The tag's LLVM type: `i32` by default, or the integer type a
    /// `repr(u8)`-style clause names (kernel stage 2). Every tag load, store
    /// and comparison uses it, so a narrower tag is the same object the C
    /// typedef declares.
    pub(crate) tag_ty: Type<'a>,
    /// Whether that tag type is signed, so the hash's widening extension
    /// matches the earlier emitter's `(uint64_t)` cast of a signed or unsigned tag.
    pub(crate) tag_signed: bool,
    /// The union type. It is a struct anchored on the widest payload's own
    /// alignment type, sized to the widest payload; the anchor field is what
    /// makes LLVM place it where the C `union` sits (`enum_union_body`).
    pub(crate) union_ty: Type<'a>,
    /// One payload type per variant, in declaration order. A variant with no
    /// payload gets a bare `i8`, which is the one-byte `char kelp_none` the C
    /// typedef gives it, so `union` sizes agree.
    pub(crate) variants: Vec<EnumVariant<'a>>,
}

/// One variant's payload: the identified struct its fields live in, and those
/// fields' `tir` types (the drop needs the types, the access needs the struct).
#[derive(Clone)]
pub(crate) struct EnumVariant<'a> {
    pub(crate) payload_ty: Type<'a>,
    pub(crate) fields: Vec<Ty>,
}

impl<'a> EnumVariant<'a> {
    /// The payload struct's field index for declared field `field`: the count
    /// of `unit`-typed fields declared before it, subtracted out, because a
    /// `unit` field gets no struct member (`MISSING-r7Wqb`). LLVM addresses a
    /// struct positionally, unlike the earlier emitter's by-name
    /// `.kelp_v.<variant>.<field>`, so a declared index and a payload-struct
    /// index only coincide when nothing before `field` was filtered out.
    /// `field` itself must not be `unit`: reading one is answered by
    /// `emit_expr`'s own `unit` guard before a place is ever asked for.
    pub(crate) fn payload_index(&self, field: usize) -> u32 {
        self.fields[..field]
            .iter()
            .filter(|t| **t != Ty::Unit)
            .count() as u32
    }
}

/// The tag is field 0 and the union field 1 of every `enum` struct, so those two
/// indices are named once rather than spelled at each projection.
pub(crate) const ENUM_TAG_FIELD: u32 = 0;
pub(crate) const ENUM_UNION_FIELD: u32 = 1;

/// The synthetic first field a `repr(align(N))` datac's LLVM body carries. A
/// `#` cannot begin a source identifier, so it can never collide with a real
/// field name.
pub(crate) const DATAC_ALIGN_FIELD: &str = "#align";

/// Module-level state: the context, the module, the builder, and the runtime
/// symbols every function shares. Per-function state lives in [`FnGen`].
/// The Box pool's geometry, mirrored from the runtime crate (`KELP_BOX_POOL_MAX` and
/// `KELP_BOX_POOL_CLASSES`): blocks of up to 64 bytes at an alignment of 16 or
/// less are pooled in 16-byte size classes, and every other block is malloc.
pub(crate) const BOX_POOL_CLASSES: u64 = 4;

/// The pool class of a `size`-byte, `align`-aligned Box block, or `None` when
/// the block is not pooled. Class 0 holds sizes 0 to 16, class 1 holds 17 to 32,
/// and so on, which is `(size - 1) / 16` with a zero size in class 0.
pub(crate) fn box_pool_class(size: u64, align: u64) -> Option<u64> {
    if align > 16 || size > BOX_POOL_CLASSES * 16 {
        return None;
    }
    Some(if size == 0 { 0 } else { (size - 1) / 16 })
}

pub(crate) struct Gen<'a> {
    pub(crate) ctx: &'a Context,
    pub(crate) module: &'a Module<'a>,
    pub(crate) builder: Builder<'a>,
    pub(crate) program: &'a Program,
    pub(crate) funcs: HashMap<String, FnInfo<'a>>,
    pub(crate) printf_fn: LlvmFunction<'a>,
    pub(crate) printf_ty: FnType<'a>,
    pub(crate) panic_fn: LlvmFunction<'a>,
    pub(crate) panic_ty: FnType<'a>,
    /// The location the freestanding panic entry reads: the file name the
    /// current statement stored, and its line. Both `None` in a hosted build.
    pub(crate) panic_file_global: Option<Value<'a>>,
    pub(crate) panic_line_global: Option<Value<'a>>,
    /// One string global per module's source file name, so a statement's
    /// location is a pointer store rather than a fresh global per check.
    pub(crate) panic_files: HashMap<String, Value<'a>>,
    /// The target's data layout, read for the tagged union's size and alignment.
    /// An LLVM struct has no field-alignment annotation, so the type is built
    /// from the numbers the target reports rather than from an assumption about
    /// x86-64 (`TargetMachine::data_layout`).
    pub(crate) layout: &'a DataLayout,
    /// Whether that layout is little-endian, asked of the same target the object
    /// is written for rather than assumed. `to_le` is the identity exactly when
    /// this is true, so a big-endian triple swaps instead of silently keeping a
    /// wrong byte order (kernel stage 3, MK9-S8).
    pub(crate) little_endian: bool,
    /// One identified LLVM struct type per `datac`, keyed by the `tir` name. The
    /// body is set once per declaration in `declare_aggregates`, before any
    /// function signature is built, so a `datac` parameter or return names a
    /// complete type. An `enum` is a `Ty::Struct` too, so its outer type is in
    /// this same table, which is what lets `llvm_type` resolve one name.
    pub(crate) structs: HashMap<String, Type<'a>>,
    /// Each `enum`'s layout, keyed by the same `tir` name.
    pub(crate) enums: HashMap<String, EnumInfo<'a>>,
    /// Each `datac`'s field names in declaration order, so a `FieldAccess`
    /// resolves its field to the struct index a `getelementptr` needs without
    /// re-walking `program.structs`.
    pub(crate) field_index: HashMap<String, Vec<String>>,
    /// The `DropPlan`, keyed by the same node identity the earlier emitter reads, so
    /// the two backends free the same set of owned locals at the same points
    /// (MK7 §3.4).
    pub(crate) drops: &'a DropPlan,
    /// The runtime helpers this slice reaches, keyed by their mangled symbol,
    /// plus every `extern fun` the program carries (S3). the runtime crate defines the
    /// helpers (compiled into the object `build.rs` leaves in `OUT_DIR`) and the
    /// program's own object defines the extern bodies (`externs.rs`), and in both
    /// cases the module only *declares* the function: the earlier emitter emits the
    /// same text into one translation unit.
    pub(crate) runtime: HashMap<String, RtFn<'a>>,
    /// The generated per-`datac` drop functions (`kelp_drop_<name>`), the mirror
    /// of the ones the earlier emitter emitted beside each typedef.
    pub(crate) drop_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The `datac`/`enum` names that own memory, so `types::needs_drop` can
    /// answer for a type without re-walking the declarations.
    pub(crate) owned: HashSet<String>,
    /// The LLVM type of every container instantiation the program uses, keyed
    /// by the `tir` container type. A container is a named struct built here
    /// rather than in `llvm_type`, because it is created once and then resolved
    /// by name (`Box<List<i32>>` names another instantiation).
    pub(crate) containers: HashMap<Ty, Type<'a>>,
    /// The entry struct of each `Map<K, V>` instantiation, keyed by the map
    /// type: `{ key, value*, live }`, the shape `write_map`'s typedef has.
    pub(crate) map_entries: HashMap<Ty, Type<'a>>,
    /// The monomorphized container functions, keyed by their mangled symbol
    /// (`kelp_List__i32__push`), with the signature they are declared under.
    pub(crate) container_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The generated `kelp_clone_<name>` functions of a `datac`/`enum` whose
    /// value can be deep-copied, the mirror of the ones `write_struct`/
    /// `write_enum` emit beside each typedef.
    pub(crate) clone_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The S2E closure machinery, keyed by its mangled symbol: the
    /// per-signature drop, the per-signature widening thunk and its
    /// environment's destructor, and the per-`(function, signature)` shim. One
    /// map because `emit_closure_bodies` defines each under the name it was
    /// declared with, and the two cannot drift.
    pub(crate) closure_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The `&dyn` machinery (A-12 slice 4): one thunk per vtable slot, keyed by
    /// its mangled symbol, defined by `emit_dyn_bodies` under the name
    /// `declare_dyn_fns` gave it.
    pub(crate) dyn_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// One read-only table global per `(trait, type)` vtable, keyed by the
    /// pair: the literal struct `{ [N x ptr], ptr, i64, i64 }` whose remaining
    /// fields are the drop entry and the concrete type's size and align
    /// (A-12 slice 7). Its address is what a trait object's second word holds.
    pub(crate) dyn_tables: HashMap<(String, String), Value<'a>>,
    /// The vtable's literal struct type, keyed by *trait*: every pair of one
    /// trait has the same slot count, so the same table layout. `DynCall`
    /// needs it to GEP the method array, and the owned trait object's drop
    /// function needs it to read the drop, size and align fields.
    pub(crate) dyn_table_tys: HashMap<String, Type<'a>>,
    /// The LLVM environment struct of each capturing literal, keyed by the
    /// literal's `tir` key: one field per capture, in capture order, the same
    /// fields `write_env` gives the C struct.
    pub(crate) env_types: HashMap<String, Type<'a>>,
    /// The one-field environment a widened function value's thunk reads its code
    /// pointer out of: `{ ptr f }`, the shape `write_coerce_env` declares.
    /// Shared by every signature because opaque pointers make the field's type
    /// the same for all of them.
    pub(crate) coerce_env_ty: Type<'a>,
    /// The generated `kelp_cmp_<name>` functions of an orderable `datac`/`enum`.
    pub(crate) cmp_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The generated `kelp_hash_<name>` and `kelp_eq_<name>` functions of a
    /// keyable `datac`/`enum`, the mirror of the pair `write_struct`/
    /// `write_enum` emit for the same types.
    pub(crate) hash_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    pub(crate) eq_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The libc primitives the container function sets call: the standard
    /// headers' allocation, copy and scan functions, declared and never
    /// re-implemented, exactly as the earlier emitter reaches them.
    pub(crate) libc: HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)>,
    /// The project's own allocator, when a freestanding manifest configures one
    /// (kernel stage 7). `None` means libc or, in a freestanding build, no
    /// allocator at all, in which case the heap types were refused before
    /// codegen.
    pub(crate) alloc_fns: Option<AllocFns<'a>>,
    /// The module's own `memcpy`/`memset` definitions, present only in a
    /// freestanding build: there is no libc to provide them, and the container
    /// bodies still need a byte copy and a zero fill.
    pub(crate) mem: HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)>,
    /// Every container instantiation the program mentions, so the module emits
    /// exactly the sets it uses (mirrors the instantiation table).
    pub(crate) instantiations: Instantiations,
    /// Whether this program spawns a worker, so every function it emits gets a
    /// cleanup frame (decision 125). `false` emits no frame code at all.
    pub(crate) spawn_frames: bool,
    /// The user types whose value can be deep-copied, the same fixpoint
    /// the earlier emitter computes for `List.clone`/`List.slice`.
    pub(crate) clonable: HashSet<String>,
    /// The user types whose value has a derived order, for `List.sort`.
    pub(crate) sortable: HashSet<String>,
    /// The user types whose value is a key, for `Map<K, V>` with a user key
    /// (A-6): the same fixpoint the earlier emitter computes for `kelp_hash_<T>`
    /// and `kelp_eq_<T>`.
    pub(crate) keyable: HashSet<String>,
    /// One module-scope global per `pd` (MK7 §3.1 and §5), keyed by the
    /// internal `pd#<name>` identifier `tir` names it with, plus that
    /// passdown's own type. The earlier emitter's `static <type> <name>;` is the
    /// same storage: a `pd` initializer fills it once and a function with no
    /// hidden parameter of its own (`main`) reads it directly.
    pub(crate) pd_globals: HashMap<String, (Value<'a>, Ty)>,
    /// The generated `kelp_init_pds` and `kelp_drop_pds`, present only for a
    /// program that declares a passdown. The init function is what `main`
    /// calls first; the drop function is what `atexit` registers, the only
    /// drop point an owned passdown has (the passdown globals emitter).
    pub(crate) init_pds_fn: Option<LlvmFunction<'a>>,
    pub(crate) drop_pds_fn: Option<LlvmFunction<'a>>,
    /// One module-scope global per `static` (kernel stage 7), keyed by the
    /// module-prefixed name `tir` gives it, plus its type and, for a `percpu`
    /// static (kernel stage 8), the number of copies. Each holds its folded
    /// constant, so no code runs at startup and a freestanding image needs no
    /// init hook; a `StaticRef` loads from it, stores through it and takes its
    /// address.
    pub(crate) static_globals: HashMap<String, StaticGlobal<'a>>,
    /// The runtime's two saved `argv` globals (the runtime crate), which
    /// `main` writes and `process.args()` reads. They are `extern` here: the
    /// runtime object defines them, exactly as it does for the earlier emitter's
    /// generated `main`.
    pub(crate) argc_global: Value<'a>,
    pub(crate) argv_global: Value<'a>,
    /// The source map, which `cap_plan` reads for the `.kp` call site a
    /// capability denial names. The earlier emitter reads the same map for the same
    /// text (the capability plan), so the two cannot name different
    /// places.
    pub(crate) sources: &'a SourceMap,
    /// The compiled-in allowlist array of each `(module, domain)` grant, keyed
    /// by that pair. The data is C in the per-program object
    /// (`program_c_source`); this is the symbol a check passes as `patterns`.
    pub(crate) cap_arrays: HashMap<(String, String), Value<'a>>,
    /// The capability check's C entry points (`kelp_cap_check_path` and
    /// friends), declared with the signature `the runtime capability glue` defines.
    pub(crate) cap_fns: HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)>,
    /// The DWARF debug-information builder (MK7 S4 §4.4): the one place a
    /// `.kp` file and line become metadata. It owns the compile unit, the
    /// per-module `DIFile`s and the per-function `DISubprogram`s, and it is
    /// finalized before the module is verified or written.
    pub(crate) debug: DebugBuilder<'a>,
    /// The `DISubprogram` of each `.kp`-declared function, keyed by the same
    /// `Function::name` the lowering looks a function up by. A function with no
    /// entry here has no `.kp` source behind it (an `extern fun`, whose body is
    /// C and compiled separately, or a module whose text was never registered),
    /// so its instructions carry no location rather than a wrong one.
    pub(crate) debug_scopes: HashMap<String, Metadata<'a>>,
    /// The `DISubprogram` of the generated `kelp_init_pds`, when the program
    /// has passdowns with a registered source. The passdown initializers are
    /// emitted into that one function, and its scope is what makes their
    /// statements resolve to the `.kp` line that declared them.
    pub(crate) pd_init_scope: Option<Metadata<'a>>,
    /// The freestanding configuration, when this is a freestanding build (kernel
    /// stage 4). It decides the panic handler the bounds and overflow checks
    /// call: the project's own `panic = "..."` function, or a generated
    /// infinite loop when the manifest names none.
    pub(crate) freestanding: Option<Freestanding>,
    /// The manifest's `[target] cpus` (kernel stage 8), defaulted to 1 for a
    /// hosted build (which has no `percpu` statics anyway).
    pub(crate) cpus: u64,
    /// Whether the `cpu_index()` bounds check is emitted: true at `-O0`, the
    /// dev build. A release image trusts the register and carries no check
    /// (`kelp wiki page language/unsafe-ub-catalogue.md` stage 11).
    pub(crate) debug_checks: bool,
    /// The compilation unit this pass emits (roadmap A-14 slice 1), or `None`
    /// for the whole-program single module every build had before the mode
    /// existed. See [`Gen::defines_function`] for what it decides.
    pub(crate) unit: Option<Unit>,
    /// Whether this is a managed build (WP3): heap objects are GC allocations,
    /// frees are dropped, and a drop runs only where a resource needs one.
    pub(crate) managed: bool,
    /// The managed heap's limits, from the manifest's `heap_max` and `[tidepool]`.
    /// `kelp_gc_init` receives them. `None` in every other build.
    pub(crate) managed_heap: Option<HeapLimits>,
    /// The managed build's LLVM descriptors and collector entry points
    /// (`gc_descr`), declared once per module. `None` in every other build.
    pub(crate) gc: Option<crate::gc_descr::GcTypes<'a>>,
    /// The `datac` and `enum` names that hold a resource (`gc_descr::resource_types`).
    /// Empty in every other build.
    pub(crate) resources: HashSet<String>,
}

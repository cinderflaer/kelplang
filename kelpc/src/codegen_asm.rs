//! Lowers the typed IR (`tir`) to x86-64 assembly text (MK7 tier 1).
//!
//! This is the **first slice** of the own backend, and it is deliberately the
//! simplest thing that can be correct rather than the fastest thing that can
//! run. Three decisions are worth naming, because each one is a thing this
//! module will grow out of:
//!
//! 1. **A stack machine, not an allocator.** Every expression evaluates into
//!    `rax`, and a binary operator spills the left operand to the stack so the
//!    right one can be computed. There is no register allocator, no liveness
//!    analysis, and no attempt to keep a value in a register across statements;
//!    locals live in frame slots. This is *at or below* the `-O0` column the
//!    plan gates on, which is exactly why it is the right first step:
//!    correctness is checkable against the C backend before any performance
//!    claim has to be made. `plan/mk7-own-backend.md` §5 item 3 is where a real
//!    allocator replaces this.
//!
//! 2. **Assembly text, not machine code.** The output goes to `as` and `ld`, so
//!    instruction encoding and relocation stay somebody else's problem for now;
//!    tier 2 removes both. What that buys here is that a wrong instruction is a
//!    readable diff rather than a byte offset.
//!
//! 3. **A refusal is a feature.** Anything this slice cannot lower — a heap
//!    type, an aggregate, a closure, an `extern` call — is reported as
//!    `unsupported` with the type or node named, rather than emitted wrongly.
//!    The fixture suite then measures exactly how much of the language is
//!    covered, and that number rises by removing named gaps instead of by
//!    hunting silent miscompiles.
//!
//! Covered: every integer width, `bool`, `char`, `f32`/`f64` arithmetic and
//! comparisons (including the unordered/NaN case), all unary and binary
//! operators including short-circuiting `&&`/`||`, `if`/`else` in statement and
//! value position, `while`, assignment, calls with up to six integer and eight
//! float arguments classified independently per SysV, recursion, and
//! `print`/`print!` of integers and characters through a single `printf`.
//! Printing a float itself is not covered — that is `fmt`'s `Display`
//! machinery, a separate gap from this item's ABI/arithmetic scope
//! (`plan/mk7-own-backend.md` §5 item 2).
//!
//! # Calling convention
//!
//! System V AMD64 — the same convention the C backend gets from clang, and
//! therefore the one the differential oracle compares against: integer
//! arguments in `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`, the result in `rax`,
//! and `rsp` sixteen-byte aligned at every `call`.
//!
//! # Alignment
//!
//! The stack-machine spills move `rsp` by eight bytes per pending operand, so a
//! call site cannot assume alignment. Rather than track the depth, every call
//! saves `rsp` into a reserved frame slot, rounds it down to a multiple of
//! sixteen, calls, and restores it from the same slot. Rounding *down* only
//! ever moves `rsp` below the pushed operands, so it cannot clobber one.
//!
//! The save is a memory slot and not a register because a register is a bug
//! waiting to happen: the first version used `r11`, `printf` clobbered it, and a
//! function that printed twice restored a garbage stack pointer. See `SP_SLOT`.

use crate::ast::{BinOp, UnOp};
use crate::borrowck::DropPlan;
use crate::diag::SourceMap;
use crate::tir::*;
use crate::types::Ty;
use std::fmt::Write as _;

/// Integer width, which is also the operand size the arithmetic must run at:
/// Kelp's overflow promise is that an operation wraps *at its own type's
/// width*, so `i32` addition has to be a 32-bit `add`, not a 64-bit one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    W1,
    W2,
    W4,
    W8,
}

/// The register file this backend addresses values through, per width. `rax`
/// is where every expression's value lives; `rcx` is the scratch a binary
/// operator's right operand is evaluated into.
const RAX: [&str; 4] = ["al", "ax", "eax", "rax"];
const RCX: [&str; 4] = ["cl", "cx", "ecx", "rcx"];
/// The low halves of `rdx`, for the dividend's high half and for a byte/word
/// remainder.
const RDX: [&str; 4] = ["dl", "dx", "edx", "rdx"];
/// The argument registers, widest form, in order.
const ARG64: [&str; 6] = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
/// The same registers at each narrower width, because the assembler needs the
/// operand name to match the store size exactly.
const ARG: [[&str; 6]; 4] = [
    ["dil", "sil", "dl", "cl", "r8b", "r9b"],
    ["di", "si", "dx", "cx", "r8w", "r9w"],
    ["edi", "esi", "edx", "ecx", "r8d", "r9d"],
    ["rdi", "rsi", "rdx", "rcx", "r8", "r9"],
];
/// The size directive a `mov` to a frame slot needs written out, since the
/// slot is addressed as `[rbp-8]` with no register to infer a size from.
const MEM: [&str; 4] = ["BYTE PTR", "WORD PTR", "DWORD PTR", "QWORD PTR"];
/// The SysV vector argument registers, in order. Classified *independently*
/// of `ARG64`: a call's Nth float argument goes in `FARG[k]` where `k` counts
/// only the float arguments seen so far, not the argument's overall position —
/// `f(1, 2.0, 3)` passes `2.0` in `xmm0`, not `xmm1`.
const FARG: [&str; 8] = [
    "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7",
];

impl Width {
    fn idx(self) -> usize {
        match self {
            Width::W1 => 0,
            Width::W2 => 1,
            Width::W4 => 2,
            Width::W8 => 3,
        }
    }

    fn rax(self) -> &'static str {
        RAX[self.idx()]
    }

    fn rcx(self) -> &'static str {
        RCX[self.idx()]
    }

    fn mem(self) -> &'static str {
        MEM[self.idx()]
    }

    fn bytes(self) -> i32 {
        match self {
            Width::W1 => 1,
            Width::W2 => 2,
            Width::W4 => 4,
            Width::W8 => 8,
        }
    }

    /// Re-normalizes `rax` after an operation: the arithmetic leaves a value
    /// that is only meaningful in the low `width` bytes, and every consumer
    /// assumes an integer value is sign- or zero-extended across the whole
    /// register. Doing this after each operation, rather than at each use, is
    /// what lets everything else treat a value as a plain 64-bit number.
    fn extend(self, signed: bool) -> String {
        match (self, signed) {
            (Width::W1, true) => "    movsx rax, al\n".to_string(),
            (Width::W1, false) => "    movzx rax, al\n".to_string(),
            (Width::W2, true) => "    movsx rax, ax\n".to_string(),
            (Width::W2, false) => "    movzx rax, ax\n".to_string(),
            (Width::W4, true) => "    movsxd rax, eax\n".to_string(),
            // A 32-bit `mov` zeroes the upper half by architectural
            // definition, so an unsigned 32-bit value already is extended.
            (Width::W4, false) => "    mov eax, eax\n".to_string(),
            (Width::W8, _) => String::new(),
        }
    }

    /// Signed comparisons use the `l`/`g` family, unsigned the `b`/`a` family.
    fn setcc(op: BinOp, signed: bool) -> Option<&'static str> {
        Some(match (op, signed) {
            (BinOp::Eq, _) => "sete",
            (BinOp::NotEq, _) => "setne",
            (BinOp::Lt, true) => "setl",
            (BinOp::LtEq, true) => "setle",
            (BinOp::Gt, true) => "setg",
            (BinOp::GtEq, true) => "setge",
            (BinOp::Lt, false) => "setb",
            (BinOp::LtEq, false) => "setbe",
            (BinOp::Gt, false) => "seta",
            (BinOp::GtEq, false) => "setae",
            _ => return None,
        })
    }
}

/// `f32`/`f64`, which live in the `xmm` registers rather than the integer
/// file and therefore need their own mnemonics at every site an integer
/// `Width` would otherwise be used. `F4`/`F8` name the *value's* size, not a
/// register width — every `xmm` register is the same physical size regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FWidth {
    F4,
    F8,
}

impl FWidth {
    fn bytes(self) -> i32 {
        match self {
            FWidth::F4 => 4,
            FWidth::F8 => 8,
        }
    }

    /// The scalar move mnemonic: `movss`/`movsd`, used for both loads/stores
    /// and register-to-register copies since there is no separate "load"
    /// opcode for a scalar `xmm` value.
    fn mov(self) -> &'static str {
        match self {
            FWidth::F4 => "movss",
            FWidth::F8 => "movsd",
        }
    }

    /// The arithmetic mnemonic suffix (`add` + this = `addss`/`addsd`).
    fn suffix(self) -> &'static str {
        match self {
            FWidth::F4 => "ss",
            FWidth::F8 => "sd",
        }
    }

    /// `ucomiss`/`ucomisd`, the only scalar float comparison this slice needs:
    /// it sets the flags the same way an unsigned integer compare would,
    /// which is what lets `Width::setcc`'s unsigned family (with reversed
    /// operands for `<`/`<=`, see `Gen::binary_float`) read the result
    /// correctly, including the unordered (NaN) case.
    fn ucomi(self) -> &'static str {
        match self {
            FWidth::F4 => "ucomiss",
            FWidth::F8 => "ucomisd",
        }
    }
}

/// `f32`/`f64` classification, the float-side counterpart of `int_ty`.
fn float_ty(ty: &Ty) -> Option<FWidth> {
    Some(match ty {
        Ty::F32 => FWidth::F4,
        Ty::F64 => FWidth::F8,
        _ => return None,
    })
}

/// A type this slice can lower, as `(width, signed)`.
///
/// `None` is the coverage boundary: every use site turns it into an
/// `unsupported` diagnostic naming the type, so a gap is reported where it is
/// hit rather than guessed at.
fn int_ty(ty: &Ty) -> Option<(Width, bool)> {
    Some(match ty {
        Ty::I32 => (Width::W4, true),
        Ty::I64 | Ty::Isize => (Width::W8, true),
        Ty::U32 => (Width::W4, false),
        Ty::U64 | Ty::Usize => (Width::W8, false),
        Ty::U8 => (Width::W1, false),
        Ty::U16 => (Width::W2, false),
        // `bool` is a byte and unsigned for narrowing purposes. Its only
        // arithmetic is `&`/`|`/`^` and comparison, both width-independent once
        // the value is 0 or 1.
        Ty::Bool => (Width::W1, false),
        // `char` is a Unicode scalar value, so it is a 32-bit *unsigned* code
        // point rather than a byte; `types.rs` gives it `uint32_t` for the
        // same reason.
        Ty::Char => (Width::W4, false),
        _ => return None,
    })
}

/// The `printf` specifier a type prints through, or `None` if this slice cannot
/// print it. `bool` prints as the text `true`/`false`, which the C backend
/// spells `%s` over a conditional — so the specifier alone is not enough for it
/// and `Gen::bool_str` supplies the pointer.
fn printable_ty(ty: &Ty) -> Option<&'static str> {
    Some(match ty {
        Ty::I32 => "%d",
        Ty::I64 | Ty::Isize => "%lld",
        Ty::U32 => "%u",
        Ty::U64 => "%llu",
        Ty::U8 | Ty::U16 => "%u",
        Ty::Usize => "%zu",
        Ty::Char => "%c",
        Ty::Bool => "%s",
        _ => return None,
    })
}

/// Every uncovered construct reports through this, so the message shape — and
/// therefore what a reader greps for — is uniform.
fn unsupported(what: &str) -> String {
    format!("the 'asm' backend does not support {what} yet")
}

/// Generates assembly for `program`.
///
/// `Err` carries a complete diagnostic, printed the same way a frontend error
/// is, so an uncovered construct fails the build instead of producing a binary
/// that is quietly wrong.
pub fn generate(program: &Program, _drops: &DropPlan, _sources: &SourceMap) -> Result<String, String> {
    // Program-level shapes this slice does not lower. Checking them up front
    // gives a message naming the *declaration* rather than whatever expression
    // happened to touch it first.
    if let Some(f) = program.functions.iter().find(|f| f.extern_) {
        return Err(unsupported(&format!("the `extern fun {}`", f.name)));
    }
    if let Some(pd) = program.pds.first() {
        return Err(unsupported(&format!("the passdown `pd {}`", pd.qualified)));
    }
    if let Some(s) = program.structs.first() {
        return Err(unsupported(&format!("the datac `{}`", s.name)));
    }
    if let Some(e) = program.enums.first() {
        return Err(unsupported(&format!("the enum `{}`", e.name)));
    }

    let mut g = Gen::default();
    // The gated-call table, so a call that the C backend would wrap in a
    // runtime capability check is refused rather than emitted unchecked. This
    // is the one place where an unimplemented feature would be a *security*
    // hole rather than a wrong number: a grant that is never checked is a grant
    // that is not there, and silently dropping it would let a fixture pass
    // while the promise the language makes about capabilities stopped holding.
    g.gated = program.gated.clone();
    g.out.push_str("    .intel_syntax noprefix\n");
    // Without this the linker warns that the object implies an executable
    // stack. Nothing here needs one, so the object declares that it does not.
    g.out.push_str("    .section .note.GNU-stack,\"\",@progbits\n");
    g.out.push_str("    .text\n");

    for f in &program.functions {
        // The compiler's runtime module (`std/rt`, MK7 §8) is not defined here.
        // Every function in it is Kelp that the C backend compiles in for its
        // capability checks, and this backend refuses every gated call rather
        // than emitting one unchecked — so nothing in this backend's output can
        // reach the matcher, and defining it would replace the refusal that
        // names the real gap with a refusal that names the runtime. A program
        // that calls into the module itself is refused in `call`, so skipping
        // the definitions cannot turn into an undefined symbol at link time.
        if f.module == crate::stdlib::RT_MODULE {
            continue;
        }
        g.function(f)?;
    }
    Ok(g.finish())
}

/// One function's frame: each local gets a slot, and its offset is fixed when
/// the declaration is reached — a `Let` initializer cannot name the binding
/// being made, so no offset is ever needed before it is assigned, and the body
/// can be generated before the frame size is known.
struct Frame {
    /// Bytes reserved below `rbp`, grown one slot per declaration. Starts at
    /// `SP_SAVE_SIZE`, because the first slot is not a local: it belongs to the
    /// call-site stack-pointer save (see `SP_SLOT`).
    used: i32,
    /// Innermost last, so a shadowed name is restored when its block ends.
    scopes: Vec<Vec<(String, i32, Ty)>>,
}

/// Where a call site saves `rsp` across the call.
///
/// It is a frame slot rather than a register, and that is the whole point: the
/// first version of this used `r11`, which is **caller-saved**, so `printf`
/// overwrote it and `mov rsp, r11` after the call restored a garbage stack
/// pointer. The bug was invisible in a function that printed once — the
/// epilogue reloads `rsp` from `rbp`, so the corruption was masked until a
/// second call tried to push onto it. Memory cannot be clobbered by the callee,
/// so this cannot recur, and it costs the same two instructions.
const SP_SLOT: i32 = -8;
const SP_SAVE_SIZE: i32 = 8;

impl Frame {
    fn new() -> Frame {
        Frame {
            used: SP_SAVE_SIZE,
            scopes: vec![Vec::new()],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to a fresh slot of `ty`'s size and returns the offset from
    /// `rbp`, always negative.
    fn declare(&mut self, name: &str, ty: &Ty) -> Result<i32, String> {
        let bytes = int_ty(ty)
            .map(|(w, _)| w.bytes())
            .or_else(|| float_ty(ty).map(|w| w.bytes()))
            .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
        self.used += bytes;
        let offset = -self.used;
        self.scopes
            .last_mut()
            .expect("a scope always exists")
            .push((name.to_string(), offset, ty.clone()));
        Ok(offset)
    }

    fn lookup(&self, name: &str) -> Option<(i32, Ty)> {
        for scope in self.scopes.iter().rev() {
            for (n, offset, ty) in scope.iter().rev() {
                if n == name {
                    return Some((*offset, ty.clone()));
                }
            }
        }
        None
    }

    /// The frame size rounded up to sixteen bytes, which together with the
    /// pushed `rbp` is what keeps `rsp` aligned between calls. It is never
    /// zero, because the stack-pointer save slot is always reserved.
    fn size(&self) -> i32 {
        (self.used + 15) & !15
    }
}

/// Emits the two halves of a call's stack-pointer discipline, which every call
/// site needs: `rsp` must be sixteen-byte aligned *at* the `call`, and the
/// pending stack-machine operands move it by eight bytes each, so the depth
/// cannot be assumed. Rounding `rsp` down is safe by construction — it only
/// ever moves the pointer below the pushed operands — and restoring the saved
/// value afterwards puts the depth back exactly as it was.
fn call_align_begin(out: &mut String) {
    let _ = writeln!(out, "    mov QWORD PTR [rbp{SP_SLOT}], rsp");
    out.push_str("    and rsp, -16\n");
}

fn call_align_end(out: &mut String) {
    let _ = writeln!(out, "    mov rsp, QWORD PTR [rbp{SP_SLOT}]");
}

#[derive(Default)]
struct Gen {
    out: String,
    /// Byte-string constants, emitted at the end as `.byte` runs. Emitting a
    /// literal's bytes rather than quoting it into `.string` sidesteps escaping
    /// twice over — once for the assembler and once for `printf` — which is
    /// where a hand-written backend quietly gets a format string wrong.
    data: Vec<(String, Vec<u8>)>,
    next_label: usize,
    return_label: String,
    /// Callee key -> capability domain, for the calls whose C emission carries
    /// a runtime allowlist check. Carried here so a call site cannot be emitted
    /// without one: the check is the language's headline promise, and a backend
    /// that skipped it would be removing a guarantee rather than a feature.
    gated: std::collections::HashMap<String, String>,
}

impl Gen {
    fn label(&mut self, hint: &str) -> String {
        self.next_label += 1;
        format!(".L{hint}{}", self.next_label)
    }

    /// Interns `bytes` as a NUL-terminated constant and returns its label.
    fn bytes_label(&mut self, bytes: &[u8]) -> String {
        let label = format!(".LC{}", self.data.len());
        let mut all = bytes.to_vec();
        all.push(0);
        self.data.push((label.clone(), all));
        label
    }

    fn finish(self) -> String {
        let mut out = self.out;
        if !self.data.is_empty() {
            out.push_str("    .section .rodata\n");
            for (label, bytes) in &self.data {
                let _ = writeln!(out, "{label}:");
                // Sixteen bytes per line, so a long literal stays readable in a
                // diff and in `objdump` output.
                for chunk in bytes.chunks(16) {
                    let list: Vec<String> = chunk.iter().map(|b| b.to_string()).collect();
                    let _ = writeln!(out, "    .byte {}", list.join(", "));
                }
            }
        }
        out
    }

    /// The symbol a Kelp function lowers to. Kept identical to
    /// `codegen_c.rs::c_name` so `nm`, a stack trace and a differential test all
    /// name the same thing; `main` stays `main` because the C runtime's entry
    /// point is what calls it.
    fn symbol(name: &str) -> String {
        if name == "main" {
            "main".to_string()
        } else {
            format!("kelp_{}", crate::types::c_ident(name))
        }
    }

    fn function(&mut self, f: &Function) -> Result<(), String> {
        if f.env.is_some() {
            return Err(unsupported(&format!(
                "the closure environment of `{}`",
                f.name
            )));
        }
        for (name, ty) in &f.params {
            if int_ty(ty).is_none() && float_ty(ty).is_none() {
                return Err(unsupported(&format!(
                    "the parameter `{name}: {}` of `{}`",
                    ty.display(),
                    f.name
                )));
            }
        }
        if f.ret != Ty::Unit && int_ty(&f.ret).is_none() && float_ty(&f.ret).is_none() {
            return Err(unsupported(&format!(
                "the return type `{}` of `{}`",
                f.ret.display(),
                f.name
            )));
        }
        if f.params.iter().filter(|(_, ty)| int_ty(ty).is_some()).count() > ARG64.len() {
            return Err(unsupported(&format!(
                "`{}`, which takes more than six integer parameters",
                f.name
            )));
        }
        if f.params.iter().filter(|(_, ty)| float_ty(ty).is_some()).count() > FARG.len() {
            return Err(unsupported(&format!(
                "`{}`, which takes more than eight float parameters",
                f.name
            )));
        }

        let mut frame = Frame::new();
        frame.push_scope();
        // Parameters are bound to the same slots a local would use; their
        // incoming registers are spilled in the prologue, which is written
        // after the body because only the body knows the frame's size. An
        // integer and a float parameter at the same *position* do not share a
        // register: SysV classifies and numbers each register file
        // independently, so two counters are threaded through in declaration
        // order rather than one shared index.
        enum ParamSlot {
            Int(i32, Width, usize),
            Float(i32, FWidth, usize),
        }
        let mut params: Vec<ParamSlot> = Vec::new();
        let mut int_idx = 0usize;
        let mut float_idx = 0usize;
        for (name, ty) in &f.params {
            let offset = frame.declare(name, ty)?;
            if let Some((width, _)) = int_ty(ty) {
                params.push(ParamSlot::Int(offset, width, int_idx));
                int_idx += 1;
            } else {
                let width = float_ty(ty).expect("checked above");
                params.push(ParamSlot::Float(offset, width, float_idx));
                float_idx += 1;
            }
        }

        let saved = std::mem::take(&mut self.out);
        self.return_label = self.label("ret");
        self.function_body(f, &mut frame)?;
        let body = std::mem::take(&mut self.out);
        self.out = saved;

        let symbol = Gen::symbol(&f.name);
        let _ = writeln!(self.out, "    .globl {symbol}");
        let _ = writeln!(self.out, "    .type {symbol}, @function");
        let _ = writeln!(self.out, "{symbol}:");
        self.out.push_str("    push rbp\n    mov rbp, rsp\n");
        let _ = writeln!(self.out, "    sub rsp, {}", frame.size().max(16));

        for p in &params {
            match p {
                ParamSlot::Int(offset, width, idx) => {
                    let _ = writeln!(
                        self.out,
                        "    mov {} [rbp{offset}], {}",
                        width.mem(),
                        ARG[width.idx()][*idx]
                    );
                }
                ParamSlot::Float(offset, width, idx) => {
                    let _ = writeln!(
                        self.out,
                        "    {} [rbp{offset}], {}",
                        width.mov(),
                        FARG[*idx]
                    );
                }
            }
        }

        self.out.push_str(&body);
        let return_label = self.return_label.clone();
        let _ = writeln!(self.out, "{return_label}:");
        if f.name == "main" {
            // `main`'s Kelp type is `unit`, and its C caller reads `rax`.
            self.out.push_str("    xor eax, eax\n");
        }
        self.out.push_str("    mov rsp, rbp\n    pop rbp\n    ret\n");
        let _ = writeln!(self.out, "    .size {symbol}, .-{symbol}");
        Ok(())
    }

    fn function_body(&mut self, f: &Function, frame: &mut Frame) -> Result<(), String> {
        for stmt in &f.body.stmts {
            self.stmt(stmt, frame)?;
        }
        if let Some(tail) = &f.body.tail {
            self.expr(tail, frame)?;
            if f.ret != Ty::Unit {
                // A tail expression is a `return`, which `infer.rs` treats the
                // same as `return <tail>`, so the backend must too.
                let label = self.return_label.clone();
                let _ = writeln!(self.out, "    jmp {label}");
            }
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt, frame: &mut Frame) -> Result<(), String> {
        match stmt {
            Stmt::Let { name, ty, init } => {
                self.expr(init, frame)?;
                let offset = frame.declare(name, ty)?;
                if let Some(width) = float_ty(ty) {
                    let _ = writeln!(self.out, "    {} [rbp{offset}], xmm0", width.mov());
                    return Ok(());
                }
                let (width, _) = int_ty(ty).expect("declare validated the type");
                let _ = writeln!(
                    self.out,
                    "    mov {} [rbp{offset}], {}",
                    width.mem(),
                    width.rax()
                );
                Ok(())
            }
            Stmt::While { cond, body } => {
                let top = self.label("while");
                let end = self.label("wend");
                let _ = writeln!(self.out, "{top}:");
                self.expr(cond, frame)?;
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    je {end}");
                self.block(body, frame)?;
                let _ = writeln!(self.out, "    jmp {top}");
                let _ = writeln!(self.out, "{end}:");
                Ok(())
            }
            Stmt::Return { value, .. } => {
                match value {
                    Some(v) => self.expr(v, frame)?,
                    None => self.out.push_str("    xor eax, eax\n"),
                }
                let label = self.return_label.clone();
                let _ = writeln!(self.out, "    jmp {label}");
                Ok(())
            }
            Stmt::Expr(e) => self.expr(e, frame),
        }
    }

    /// A block in statement position. Its tail, if any, is computed for its
    /// effects and discarded — only a function body's tail is a return.
    fn block(&mut self, block: &Block, frame: &mut Frame) -> Result<(), String> {
        frame.push_scope();
        for stmt in &block.stmts {
            self.stmt(stmt, frame)?;
        }
        if let Some(tail) = &block.tail {
            self.expr(tail, frame)?;
        }
        frame.pop_scope();
        Ok(())
    }

    fn expr(&mut self, e: &Expr, frame: &mut Frame) -> Result<(), String> {
        match &e.kind {
            ExprKind::IntLit(v) => {
                // Cut to the expression's own width, so an `i32` literal behaves
                // like a computed `i32` instead of carrying a 64-bit magnitude
                // into a narrow operation.
                let (width, signed) = int_ty(&e.ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", e.ty.display())))?;
                let cut = match (width, signed) {
                    (Width::W1, true) => (*v as i8) as i64,
                    (Width::W1, false) => (*v as u8) as i64,
                    (Width::W2, true) => (*v as i16) as i64,
                    (Width::W2, false) => (*v as u16) as i64,
                    (Width::W4, true) => (*v as i32) as i64,
                    (Width::W4, false) => (*v as u32) as i64,
                    (Width::W8, _) => *v,
                };
                let _ = writeln!(self.out, "    mov rax, {cut}");
                Ok(())
            }
            ExprKind::BoolLit(b) => {
                let _ = writeln!(self.out, "    mov eax, {}", u8::from(*b));
                Ok(())
            }
            ExprKind::CharLit(cp) => {
                let _ = writeln!(self.out, "    mov eax, {cp}");
                Ok(())
            }
            ExprKind::Ident(name) => {
                let (offset, ty) = frame
                    .lookup(name)
                    .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                if let Some(width) = float_ty(&ty) {
                    let _ = writeln!(self.out, "    {} xmm0, [rbp{offset}]", width.mov());
                    return Ok(());
                }
                let (width, signed) = int_ty(&ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
                // Load at the value's own width and extend, so a narrow local
                // never leaks neighbouring frame bytes into `rax`. The
                // destination has to be the register of that same width —
                // `mov rax, DWORD PTR [...]` is not an instruction — which is
                // why this is the one `mov` that does not name `rax` directly.
                let _ = writeln!(
                    self.out,
                    "    mov {}, {} [rbp{offset}]",
                    width.rax(),
                    width.mem()
                );
                self.out.push_str(&width.extend(signed));
                Ok(())
            }
            ExprKind::Unary { op, expr } => {
                self.expr(expr, frame)?;
                if let Some(width) = float_ty(&e.ty) {
                    // `!x` and `~x` do not exist for a float, so `Neg` is the
                    // only unary op infer.rs can have accepted here. There is
                    // no `neg` instruction for `xmm`, so the sign bit is
                    // flipped through the integer file instead: move the bits
                    // out, `xor` the top bit, move them back. This sidesteps
                    // the 16-byte alignment a memory-operand `xorps`/`xorpd`
                    // would need for a mask constant.
                    if *op != UnOp::Neg {
                        return Err(unsupported("this unary operator on a float"));
                    }
                    match width {
                        FWidth::F8 => {
                            self.out.push_str("    movq rax, xmm0\n");
                            self.out
                                .push_str("    mov rcx, 0x8000000000000000\n    xor rax, rcx\n");
                            self.out.push_str("    movq xmm0, rax\n");
                        }
                        FWidth::F4 => {
                            self.out.push_str("    movd eax, xmm0\n");
                            self.out.push_str("    xor eax, 0x80000000\n");
                            self.out.push_str("    movd xmm0, eax\n");
                        }
                    }
                    return Ok(());
                }
                let (width, signed) = int_ty(&e.ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", e.ty.display())))?;
                match op {
                    UnOp::Neg => {
                        let _ = writeln!(self.out, "    neg {}", width.rax());
                        self.out.push_str(&width.extend(signed));
                    }
                    UnOp::BitNot => {
                        let _ = writeln!(self.out, "    not {}", width.rax());
                        self.out.push_str(&width.extend(signed));
                    }
                    UnOp::Not => {
                        // `!x` on a `bool` is `x == 0`, expressed through
                        // `test`/`sete` so it does not depend on the width.
                        self.out.push_str("    test rax, rax\n");
                        self.out.push_str("    sete al\n    movzx rax, al\n");
                    }
                }
                Ok(())
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, &e.ty, frame),
            ExprKind::Assign { target, value } => {
                let (offset, ty) = match &target.kind {
                    ExprKind::Ident(name) => frame.lookup(name).ok_or_else(|| {
                        unsupported(&format!("assignment to the name `{name}`"))
                    })?,
                    _ => return Err(unsupported("assignment to anything but a local")),
                };
                self.expr(value, frame)?;
                if let Some(width) = float_ty(&ty) {
                    let _ = writeln!(self.out, "    {} [rbp{offset}], xmm0", width.mov());
                    // An assignment's value is the assigned value, so `xmm0`
                    // is already correct and is deliberately not reloaded.
                    return Ok(());
                }
                let (width, _) = int_ty(&ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
                let _ = writeln!(
                    self.out,
                    "    mov {} [rbp{offset}], {}",
                    width.mem(),
                    width.rax()
                );
                // An assignment's value is the assigned value, so `rax` is
                // already correct and is deliberately not reloaded.
                Ok(())
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let alt = self.label("else");
                let end = self.label("endif");
                self.expr(cond, frame)?;
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    je {alt}");
                self.block(then_block, frame)?;
                let _ = writeln!(self.out, "    jmp {end}");
                let _ = writeln!(self.out, "{alt}:");
                if let Some(else_block) = else_block {
                    self.block(else_block, frame)?;
                }
                let _ = writeln!(self.out, "{end}:");
                Ok(())
            }
            ExprKind::Block(block) => self.block(block, frame),
            ExprKind::Call { callee, args } => self.call(callee, args, frame),
            ExprKind::Template { newline, pieces } => self.template(*newline, pieces, frame),
            ExprKind::IndirectCall { .. } => Err(unsupported("a call through a function value")),
            ExprKind::FnRef(_) | ExprKind::FnToClosure { .. } | ExprKind::ClosureLit { .. } => {
                Err(unsupported("a function value"))
            }
            ExprKind::Capture { .. } => Err(unsupported("a closure capture")),
            ExprKind::StringLit(_) => Err(unsupported("a string value")),
            ExprKind::FloatLit(v) => {
                // There is no `mov xmm, imm`: a float constant has to arrive
                // through the integer file as its raw bit pattern and cross
                // over with `movq`/`movd`, which is exactly what `codegen_c.rs`
                // does not need to worry about but every assembler-level
                // backend does.
                let width = float_ty(&e.ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", e.ty.display())))?;
                match width {
                    FWidth::F8 => {
                        let _ = writeln!(self.out, "    mov rax, {:#x}", v.to_bits());
                        self.out.push_str("    movq xmm0, rax\n");
                    }
                    FWidth::F4 => {
                        let bits = (*v as f32).to_bits();
                        let _ = writeln!(self.out, "    mov eax, {bits:#x}");
                        self.out.push_str("    movd xmm0, eax\n");
                    }
                }
                Ok(())
            }
            ExprKind::FieldAccess { .. } => Err(unsupported("a field read")),
            ExprKind::StructLit { .. } => Err(unsupported("a datac literal")),
            ExprKind::EnumLit { .. } => Err(unsupported("an enum literal")),
            ExprKind::EnumTag { .. } => Err(unsupported("an enum tag test")),
            ExprKind::EnumField { .. } => Err(unsupported("an enum payload read")),
            ExprKind::Try { .. } => Err(unsupported("the `?` operator")),
            ExprKind::Ref { .. } => Err(unsupported("a borrow")),
            ExprKind::Deref { .. } => Err(unsupported("a dereference")),
        }
    }

    /// A binary operator: the left operand is spilled so the right one can be
    /// computed, which is the whole of the "allocator" this slice has.
    fn binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        ty: &Ty,
        frame: &mut Frame,
    ) -> Result<(), String> {
        // `&&`/`||` are control flow, not arithmetic: evaluating both sides
        // would run the right side's effects where the language promises
        // short-circuiting, and would diverge where it must not.
        if op == BinOp::And || op == BinOp::Or {
            return self.short_circuit(op, lhs, rhs, frame);
        }
        // Comparisons produce `bool` regardless of the operand type, so the
        // *result* type cannot be used to detect a float operation — only the
        // operands can.
        if float_ty(&lhs.ty).is_some() || float_ty(&rhs.ty).is_some() {
            return self.binary_float(op, lhs, rhs, frame);
        }

        self.expr(lhs, frame)?;
        self.out.push_str("    push rax\n");
        self.expr(rhs, frame)?;
        self.out.push_str("    mov rcx, rax\n");
        self.out.push_str("    pop rax\n");

        // The width comes from the *operands*, not from the expression's own
        // type: a comparison's result is a `bool`, and a `bool` is one byte, so
        // taking the result type here would compare the low bytes of two
        // `i32`s and be wrong for every value above 255.
        let operand_ty = match op {
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                &lhs.ty
            }
            _ => ty,
        };
        let (width, signed) = int_ty(operand_ty)
            .or_else(|| int_ty(&lhs.ty))
            .or_else(|| int_ty(&rhs.ty))
            .ok_or_else(|| unsupported(&format!("the type `{}`", operand_ty.display())))?;

        match op {
            BinOp::Add | BinOp::Sub | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let mnemonic = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::BitAnd => "and",
                    BinOp::BitOr => "or",
                    BinOp::BitXor => "xor",
                    _ => unreachable!("matched above"),
                };
                let _ = writeln!(
                    self.out,
                    "    {mnemonic} {}, {}",
                    width.rax(),
                    width.rcx()
                );
                self.out.push_str(&width.extend(signed));
            }
            BinOp::Mul => {
                // The two-operand `imul` is the same instruction for every
                // width and both signednesses: the low half of a product does
                // not depend on how its operands are interpreted.
                let _ = writeln!(self.out, "    imul {}, {}", width.rax(), width.rcx());
                self.out.push_str(&width.extend(signed));
            }
            BinOp::Div | BinOp::Mod => {
                // The dividend is `rdx:rax`, so extending the sign into the
                // high half is part of the operation, not an optimization.
                let (hi, ext) = match width {
                    Width::W1 => ("ah", "cbw"),
                    Width::W2 => ("dx", "cwd"),
                    Width::W4 => ("edx", "cdq"),
                    Width::W8 => ("rdx", "cqo"),
                };
                if signed {
                    let _ = writeln!(self.out, "    {ext}");
                    let _ = writeln!(self.out, "    idiv {}", width.rcx());
                } else {
                    let _ = writeln!(self.out, "    xor {hi}, {hi}");
                    let _ = writeln!(self.out, "    div {}", width.rcx());
                }
                if op == BinOp::Mod {
                    // The remainder is left in the high half of the dividend.
                    let _ = writeln!(self.out, "    mov rax, {}", RDX[width.idx()]);
                }
                self.out.push_str(&width.extend(signed));
            }
            BinOp::Shl | BinOp::Shr => {
                // `cl` already holds the count, which is where the right
                // operand was evaluated.
                let mnemonic = if op == BinOp::Shl {
                    "shl"
                } else if signed {
                    "sar"
                } else {
                    "shr"
                };
                let _ = writeln!(self.out, "    {mnemonic} {}, cl", width.rax());
                self.out.push_str(&width.extend(signed));
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                let _ = writeln!(self.out, "    cmp {}, {}", width.rax(), width.rcx());
                let set = Width::setcc(op, signed)
                    .ok_or_else(|| unsupported("this comparison operator"))?;
                let _ = writeln!(self.out, "    {set} al");
                self.out.push_str("    movzx rax, al\n");
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        }
        Ok(())
    }

    /// The float-operand counterpart of `binary`. `xmm0`/`xmm1` play the role
    /// `rax`/`rcx` play for integers: `xmm0` ends up holding the left operand
    /// and `xmm1` the right, spilling the left across the right's evaluation
    /// the same way the integer path spills `rax` to the stack.
    fn binary_float(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        frame: &mut Frame,
    ) -> Result<(), String> {
        let width = float_ty(&lhs.ty)
            .or_else(|| float_ty(&rhs.ty))
            .expect("caller checked at least one operand is a float");

        self.expr(lhs, frame)?;
        self.out.push_str("    sub rsp, 8\n");
        let _ = writeln!(self.out, "    {} [rsp], xmm0", width.mov());
        self.expr(rhs, frame)?;
        let _ = writeln!(self.out, "    {} xmm1, xmm0", width.mov());
        let _ = writeln!(self.out, "    {} xmm0, [rsp]", width.mov());
        self.out.push_str("    add rsp, 8\n");

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let mnemonic = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    BinOp::Div => "div",
                    _ => unreachable!("matched above"),
                };
                let _ = writeln!(self.out, "    {mnemonic}{} xmm0, xmm1", width.suffix());
                Ok(())
            }
            // `ucomi{ss,sd}` sets flags as an unsigned integer compare would,
            // including the all-ones (unordered/NaN) case. `<`/`<=` compare
            // with the operands reversed and read `above`/`above-or-equal`,
            // which is `false` when unordered because that reading needs
            // both CF=0 and ZF=0 — the standard trick for skipping an explicit
            // parity check on those two operators. `==`/`!=` cannot use that
            // trick (there is no reordering that makes "equal" a one-flag
            // read) and check the parity flag directly instead.
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                let (a, b) = match op {
                    BinOp::Lt | BinOp::LtEq => ("xmm1", "xmm0"),
                    _ => ("xmm0", "xmm1"),
                };
                let _ = writeln!(self.out, "    {} {a}, {b}", width.ucomi());
                let set = match op {
                    BinOp::Lt | BinOp::Gt => "seta",
                    _ => "setae",
                };
                let _ = writeln!(self.out, "    {set} al");
                self.out.push_str("    movzx rax, al\n");
                Ok(())
            }
            BinOp::Eq => {
                let _ = writeln!(self.out, "    {} xmm0, xmm1", width.ucomi());
                self.out.push_str("    sete al\n    setnp cl\n    and al, cl\n");
                self.out.push_str("    movzx rax, al\n");
                Ok(())
            }
            BinOp::NotEq => {
                let _ = writeln!(self.out, "    {} xmm0, xmm1", width.ucomi());
                self.out.push_str("    setne al\n    setp cl\n    or al, cl\n");
                self.out.push_str("    movzx rax, al\n");
                Ok(())
            }
            _ => Err(unsupported("this operator on a float")),
        }
    }

    fn short_circuit(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        frame: &mut Frame,
    ) -> Result<(), String> {
        let end = self.label("short");
        self.expr(lhs, frame)?;
        self.out.push_str("    test rax, rax\n");
        // `false && _` and `true || _` both skip the right side, so `je` and
        // `jne` respectively jump to a label that leaves the left operand's
        // own 0/1 in `rax`.
        let _ = writeln!(self.out, "    {} {end}", if op == BinOp::And { "je" } else { "jne" });
        self.expr(rhs, frame)?;
        self.out.push_str("    test rax, rax\n    setne al\n    movzx rax, al\n");
        let _ = writeln!(self.out, "{end}:");
        Ok(())
    }

    fn call(&mut self, callee: &str, args: &[Expr], frame: &mut Frame) -> Result<(), String> {
        // `print`/`print!` are builtins rather than functions, and only a
        // *template* argument becomes an `ExprKind::Template`: `print(x)` where
        // `x` is a plain value stays a call to the builtin, so both spellings
        // have to be lowered here.
        if callee == "print" || callee == "print!" {
            return self.builtin_print(callee == "print", args, frame);
        }
        // `n.to_u8()` and friends arrive as `<src>#to_<dst>` and are emitted
        // inline by the C backend as a cast plus a range check that panics.
        // Refusing here rather than falling through is deliberate: the symbol
        // does not exist, so letting it reach the linker would report a missing
        // `kelp_i32#to_u8` as a link error, which is a much worse diagnostic
        // than naming the feature.
        if callee.contains("#to_") {
            return Err(unsupported(&format!(
                "the numeric conversion `{}` (it needs the panic path, which is part of the \
                 runtime this slice does not have)",
                callee.replace('#', ".")
            )));
        }
        // A call into the compiler's runtime module (`std/rt`, MK7 §8). Its
        // definitions are skipped above, so this cannot be left to the linker:
        // the refusal has to name the feature, not the missing symbol.
        if callee.starts_with(&crate::stdlib::key_prefix(crate::stdlib::RT_MODULE)) {
            return Err(unsupported(&format!(
                "the runtime function `{}` — the 'asm' backend compiles no `{}` in yet",
                callee.replace('.', "::"),
                crate::stdlib::RT_MODULE
            )));
        }
        if let Some(domain) = self.gated.get(callee) {
            return Err(unsupported(&format!(
                "the capability check that gates `{callee}` (domain `{domain}`) — the asm \
                 backend cannot emit an unchecked call, so it refuses instead"
            )));
        }
        // SysV numbers the integer and vector argument registers
        // independently, so `f(1, 2.0, 3)` passes `2.0` in `xmm0` — the
        // classification below counts each argument's own class, not its
        // position in the list.
        enum ArgSlot {
            Int(usize),
            Float(usize),
        }
        let mut slots: Vec<ArgSlot> = Vec::with_capacity(args.len());
        let mut int_idx = 0usize;
        let mut float_idx = 0usize;
        for arg in args {
            if float_ty(&arg.ty).is_some() {
                if float_idx >= FARG.len() {
                    return Err(unsupported(&format!(
                        "a call to `{callee}` with more than eight float arguments"
                    )));
                }
                slots.push(ArgSlot::Float(float_idx));
                float_idx += 1;
            } else {
                if int_idx >= ARG64.len() {
                    return Err(unsupported(&format!(
                        "a call to `{callee}` with more than six integer arguments"
                    )));
                }
                slots.push(ArgSlot::Int(int_idx));
                int_idx += 1;
            }
        }
        // Every argument is evaluated and pushed (as one 8-byte stack slot
        // either way), then popped into its register in reverse. Going
        // through the stack rather than straight into the argument registers
        // is what makes a nested call safe: an inner call cannot overwrite a
        // register the outer one has not loaded yet, and its own pushes are
        // popped before it returns.
        for arg in args {
            self.expr(arg, frame)?;
            if let Some(width) = float_ty(&arg.ty) {
                self.out.push_str("    sub rsp, 8\n");
                let _ = writeln!(self.out, "    {} [rsp], xmm0", width.mov());
            } else {
                self.out.push_str("    push rax\n");
            }
        }
        for (arg, slot) in args.iter().zip(slots.iter()).rev() {
            match slot {
                ArgSlot::Int(i) => {
                    let _ = writeln!(self.out, "    pop {}", ARG64[*i]);
                }
                ArgSlot::Float(i) => {
                    let width = float_ty(&arg.ty).expect("classified as float above");
                    let _ = writeln!(self.out, "    {} {}, [rsp]", width.mov(), FARG[*i]);
                    self.out.push_str("    add rsp, 8\n");
                }
            }
        }
        let symbol = Gen::symbol(callee);
        call_align_begin(&mut self.out);
        let _ = writeln!(self.out, "    call {symbol}");
        call_align_end(&mut self.out);
        Ok(())
    }

    /// Turns the 0/1 in `reg` into a pointer to `"true"` or `"false"`.
    ///
    /// The order matters: `test` sets the flags from the *value*, and the `lea`
    /// between it and the `je` deliberately sits there because `lea` does not
    /// touch the flags. Testing after the load would branch on the address of
    /// the string, which is always non-zero.
    fn bool_str(&mut self, reg: &str) {
        let t = self.bytes_label(b"true");
        let f = self.bytes_label(b"false");
        let skip = self.label("bool");
        let _ = writeln!(self.out, "    test {reg}, {reg}");
        let _ = writeln!(self.out, "    lea {reg}, [rip+{f}]");
        let _ = writeln!(self.out, "    je {skip}");
        let _ = writeln!(self.out, "    lea {reg}, [rip+{t}]");
        let _ = writeln!(self.out, "{skip}:");
    }

    /// `print(v)` / `print!(v)`: one `printf` with one specifier, matching the
    /// C backend's shape so a fixture's bytes are identical across backends.
    fn builtin_print(
        &mut self,
        newline: bool,
        args: &[Expr],
        frame: &mut Frame,
    ) -> Result<(), String> {
        let [arg] = args else {
            return Err(unsupported("a print that is not given exactly one value"));
        };
        let spec = printable_ty(&arg.ty)
            .ok_or_else(|| unsupported(&format!("printing a value of type `{}`", arg.ty.display())))?;
        let mut fmt = spec.as_bytes().to_vec();
        if newline {
            fmt.push(b'\n');
        }
        self.expr(arg, frame)?;
        // The value is moved straight into the argument register: there is
        // nothing else to evaluate afterwards, so the stack is not needed.
        self.out.push_str("    mov rsi, rax\n");
        if arg.ty == Ty::Bool {
            self.bool_str("rsi");
        }
        let label = self.bytes_label(&fmt);
        let _ = writeln!(self.out, "    lea rdi, [rip+{label}]");
        self.out.push_str("    xor eax, eax\n");
        call_align_begin(&mut self.out);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out);
        Ok(())
    }

    /// `print`/`print!`: one `printf` with one generated format string, the
    /// same shape the C backend emits, so a fixture's bytes match across
    /// backends by construction rather than by coincidence.
    fn template(
        &mut self,
        newline: bool,
        pieces: &[TemplatePiece],
        frame: &mut Frame,
    ) -> Result<(), String> {
        let mut fmt: Vec<u8> = Vec::new();
        let mut values: Vec<&Expr> = Vec::new();
        for piece in pieces {
            match piece {
                TemplatePiece::Text(text) => {
                    for b in text.bytes() {
                        // `%` is the one byte `printf` reads, so a literal one
                        // has to be doubled.
                        if b == b'%' {
                            fmt.push(b'%');
                        }
                        fmt.push(b);
                    }
                }
                TemplatePiece::Value(value) => {
                    let spec = printable_ty(&value.ty).ok_or_else(|| {
                        unsupported(&format!(
                            "printing a value of type `{}`",
                            value.ty.display()
                        ))
                    })?;
                    fmt.extend_from_slice(spec.as_bytes());
                    values.push(value);
                }
            }
        }
        if newline {
            fmt.push(b'\n');
        }
        let value_regs = ["rsi", "rdx", "rcx", "r8", "r9"];
        if values.len() > value_regs.len() {
            return Err(unsupported("a print with more than five interpolations"));
        }

        for value in &values {
            self.expr(value, frame)?;
            self.out.push_str("    push rax\n");
        }
        let fmt_label = self.bytes_label(&fmt);
        for i in (0..values.len()).rev() {
            let _ = writeln!(self.out, "    pop {}", value_regs[i]);
            // A `bool` interpolated into a template needs its pointer built
            // after the pop, in the register it landed in.
            if values[i].ty == Ty::Bool {
                self.bool_str(value_regs[i]);
            }
        }
        let _ = writeln!(self.out, "    lea rdi, [rip+{fmt_label}]");
        // `%al` holds the number of vector registers used, which is zero:
        // nothing here passes a float.
        self.out.push_str("    xor eax, eax\n");
        call_align_begin(&mut self.out);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out);
        Ok(())
    }
}

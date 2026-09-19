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
//! `rsp` sixteen-byte aligned at every `call`, a `datac` of nine to sixteen
//! bytes in a pair of integer registers, and a `datac` over sixteen bytes
//! (`MEMORY` class) either through a hidden pointer (as a return value) or in
//! the caller's outgoing argument area (as an argument) — see
//! `memory_arg_area`. The stack-passed arguments are the one part of the
//! convention that needs agreement between a call site and a callee's prologue
//! rather than a fixed register, which is why both are generated from that one
//! function.
//!
//! # Alignment
//!
//! The stack-machine spills move `rsp` by eight bytes per pending operand, so a
//! call site cannot assume alignment. Rather than track the depth, every call
//! saves `rsp` into a reserved frame slot, rounds it down to a multiple of
//! sixteen, calls, and restores it from the same slot. Rounding *down* only
//! ever moves `rsp` below the pushed operands, so it cannot clobber one — and
//! because the outgoing argument area is subtracted from `rsp` *after* that
//! rounding and is itself a multiple of sixteen, the first stack-passed argument
//! sits at `[rsp+0]` with `rsp` still aligned at the `call`.
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

/// The register pool a real allocator (MK7 §5 item 3) hands *integer* locals
/// out of, at each of the four widths a local might need. All five are
/// callee-saved, which is the whole point: a local that lives in one of these
/// across a call does not need reloading afterward, unlike `rax`/`rcx`/`rdx`
/// or the argument registers, all of which the stack machine and the call
/// sequence already own. `rsp`/`rbp` are never candidates.
const CALLEE_SAVED: [[&str; 4]; 5] = [
    ["bl", "bx", "ebx", "rbx"],
    ["r12b", "r12w", "r12d", "r12"],
    ["r13b", "r13w", "r13d", "r13"],
    ["r14b", "r14w", "r14d", "r14"],
    ["r15b", "r15w", "r15d", "r15"],
];
/// The 64-bit spelling of each pool register, in the same order, for the
/// push/pop pair a function's prologue/epilogue needs around whichever of
/// them it actually uses.
const CALLEE_SAVED64: [&str; 5] = ["rbx", "r12", "r13", "r14", "r15"];

/// Where one local variable lives for the whole of its function: a fixed
/// callee-saved register, or (as every local got before this item) a frame
/// slot. A local in `Reg` is read and written as a plain register-to-register
/// `mov`; the width-normalized-to-64-bits invariant `Width::extend` already
/// keeps for `rax` between operations applies to a pool register the same
/// way, so no extension is needed at a read — only at the one place a value
/// enters the register file narrower than 64 bits: a parameter arriving in an
/// `ARG` register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loc {
    Reg(usize),
    Stack(i32),
}

impl Loc {
    /// `None` for a register — the float path is the only caller, and floats
    /// are never register-allocated by this item (see `allocate_registers`),
    /// so a `Reg` here would be this module's own bug, not a user program.
    fn stack_offset(self) -> Option<i32> {
        match self {
            Loc::Stack(o) => Some(o),
            Loc::Reg(_) => None,
        }
    }
}

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
        self.extend_named(signed, RAX)
    }

    /// `extend`, generalized to any register's four-width name table — the
    /// one other site this is needed is a register-allocated parameter's
    /// prologue load, which extends within its assigned pool register
    /// instead of `rax`.
    fn extend_named(self, signed: bool, names: [&'static str; 4]) -> String {
        let r64 = names[3];
        match (self, signed) {
            (Width::W1, true) => format!("    movsx {r64}, {}\n", names[0]),
            (Width::W1, false) => format!("    movzx {r64}, {}\n", names[0]),
            (Width::W2, true) => format!("    movsx {r64}, {}\n", names[1]),
            (Width::W2, false) => format!("    movzx {r64}, {}\n", names[1]),
            (Width::W4, true) => format!("    movsxd {r64}, {}\n", names[2]),
            // A 32-bit `mov` zeroes the upper half by architectural
            // definition, so an unsigned 32-bit value already is extended.
            (Width::W4, false) => format!("    mov {}, {}\n", names[2], names[2]),
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

/// MK7 §5 item 4, first slice: a `datac` whose fields are all scalar
/// (`int_ty`-representable — no float field, no heap type) — or, as of the
/// second slice, another qualifying `datac` nested by value — and whose C
/// layout fits in eight bytes. Such a value needs no eightbyte classification
/// at all: it *is* one eightbyte, so it can move through exactly the same
/// `rax`/stack-slot/argument-register paths a `u64` already does — a call
/// argument, a return value, a `Let`, an `Assign` and even a pool register all
/// already know how to carry an opaque 8-byte value. Only a **field read**
/// needs the layout below, to find the byte offset and width to load out of
/// the 8 bytes — and, for a nested field (`o.inner.v`), to find the nested
/// struct's own offset first.
///
/// A `datac` that does not qualify — a float field, a `str`/`String`/`List`/
/// `Map`/`Box` field, a nested `datac` whose own size is not itself a valid
/// move width (1/2/4/8 bytes — see `width_for_size`), or more than eight bytes
/// of fields overall — simply has no entry here, and every site that would
/// need one reports `unsupported` naming the type. That is deliberately the
/// same "coverage boundary" discipline the rest of this module uses:
/// two-eightbyte and `MEMORY`-class structs (`plan/mk7-own-backend.md` §5 item
/// 4's larger scope) are a later slice, not a silent truncation.
#[derive(Clone)]
struct StructLayout {
    /// One entry per field, in declaration order, preserved because
    /// `struct_lit` must evaluate field-initializer expressions in the order
    /// they were written for side-effect correctness even though scalar and
    /// nested fields are looked up through different lists below.
    entries: Vec<FieldEntry>,
    /// This struct's own size and alignment, in bytes — needed when this
    /// struct is itself nested inside another one, to place it and to know
    /// how wide a single `mov` must be to copy it whole.
    size: i32,
    align: i32,
}

#[derive(Clone)]
enum FieldEntry {
    /// `(name, byte offset from the struct's base, width, signed)`. Offsets
    /// follow the same natural C alignment `codegen_c.rs`'s emitted typedef
    /// gets from clang.
    Scalar(String, i32, Width, bool),
    /// `(name, byte offset from the struct's base, nested struct's name)`.
    Nested(String, i32, String),
}

impl StructLayout {
    fn scalar_field(&self, name: &str) -> Option<(i32, Width, bool)> {
        self.entries.iter().find_map(|e| match e {
            FieldEntry::Scalar(n, o, w, s) if n == name => Some((*o, *w, *s)),
            _ => None,
        })
    }

    fn nested_field(&self, name: &str) -> Option<(i32, &str)> {
        self.entries.iter().find_map(|e| match e {
            FieldEntry::Nested(n, o, ty) if n == name => Some((*o, ty.as_str())),
            _ => None,
        })
    }
}

/// The `Width` that moves exactly `size` bytes in one instruction, or `None`
/// if `size` is not one of the four this backend's `mov` forms cover. A
/// top-level qualifying struct's own size can be anything up to eight bytes
/// (it is always moved whole, as one opaque `QWORD`, by `value_kind`'s
/// blanket `Width::W8`), but a struct nested *inside* another one is copied
/// by exactly this many bytes into the parent's slot, so its size has to be a
/// real move width or the nesting is refused rather than done wrong.
fn width_for_size(size: i32) -> Option<Width> {
    Some(match size {
        1 => Width::W1,
        2 => Width::W2,
        4 => Width::W4,
        8 => Width::W8,
        _ => return None,
    })
}

fn align_up(cursor: i32, align: i32) -> i32 {
    (cursor + align - 1) / align * align
}

/// MK7 §5 item 4, third slice: a top-level `datac`'s own SysV eightbyte class,
/// derived purely from its size — sound *only* because every field this
/// module accepts at all is `int_ty`-representable or itself a nested
/// qualifying (≤8-byte) `datac`, which by induction is the same thing. A real
/// SysV classifier also has to consider whether each eightbyte is reachable
/// only through float fields (making it `SSE` class); this module never
/// builds a layout for a `datac` with a float field in the first place (see
/// `try_build_layout`'s field loop — `Ty::F32`/`Ty::F64` fall through to
/// `None`), so every eightbyte a layout here ever describes is `INTEGER`
/// class, and the three-way split below is exactly "how many `INTEGER`
/// eightbytes, and does the count still fit in registers" rather than a
/// general classifier.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AggClass {
    /// ≤8 bytes: one opaque eightbyte, unchanged from the first two slices.
    One,
    /// 9-16 bytes: two `INTEGER` eightbytes, passed/returned in a pair of
    /// integer registers (`rax`/`rdx` on return; two consecutive `ARG64`
    /// slots as a by-value argument or parameter).
    Two,
    /// Over 16 bytes: SysV `MEMORY` class — returned through the hidden
    /// first-argument-pointer convention, and (as of the fourth slice) passed
    /// as a by-value argument or parameter **in memory**, in the caller's
    /// outgoing argument area, rather than in registers. There is no
    /// register-*pair* shortcut for one and no register at all: `int_regs_for`
    /// counts it as zero integer registers, which is what keeps the numbering
    /// of every other argument unaffected by it.
    Mem,
}

/// Every `MEMORY`-class value among `tys`, in declaration order, as
/// `(index within tys, byte offset in the outgoing argument area, size)` —
/// plus the area's own size, rounded up to sixteen bytes.
///
/// This is the one rule call sites and callees have to agree on: SysV lays the
/// stack-passed arguments out in declaration order starting at `[rsp]` when the
/// `call` executes, each at its own eight-byte-aligned offset, and the area is
/// padded (above the last argument) so that `rsp` is still sixteen-byte aligned
/// at the call. The callee reads the same value at `[rbp+16+offset]`, because at
/// its entry `rsp` is `caller_rsp - 8` (the return address) and `rbp` is that
/// minus the pushed `rbp`. Sharing this function between `Gen::function` and
/// `Gen::call` is what makes the two ends of that convention impossible to
/// drift apart.
fn memory_arg_area<'a>(
    tys: impl Iterator<Item = &'a Ty>,
    layouts: &std::collections::HashMap<String, StructLayout>,
) -> (Vec<(usize, i32, i32)>, i32) {
    let mut out = Vec::new();
    let mut cursor: i32 = 0;
    for (i, ty) in tys.enumerate() {
        let Some(layout) = struct_layout(ty, layouts) else {
            continue;
        };
        if layout.class() != AggClass::Mem {
            continue;
        }
        cursor = align_up(cursor, 8);
        out.push((i, cursor, layout.size));
        cursor += align_up(layout.size, 8);
    }
    (out, align_up(cursor, 16))
}

fn agg_class(size: i32) -> AggClass {
    if size <= 8 {
        AggClass::One
    } else if size <= 16 {
        AggClass::Two
    } else {
        AggClass::Mem
    }
}

impl StructLayout {
    fn class(&self) -> AggClass {
        agg_class(self.size)
    }
}

/// One struct's layout, given every struct already decided (accepted or
/// rejected) so far. `None` covers both "not decided yet" (a nested struct
/// this pass hasn't reached) and "permanently disqualified" — `build_layouts`
/// cannot tell them apart without re-trying, which is why it loops.
fn try_build_layout(
    s: &crate::types::StructDef,
    out: &std::collections::HashMap<String, StructLayout>,
) -> Option<StructLayout> {
    let mut cursor: i32 = 0;
    let mut max_align: i32 = 1;
    let mut entries = Vec::with_capacity(s.fields.len());
    for f in &s.fields {
        if let Some((width, signed)) = int_ty(&f.ty) {
            let align = width.bytes();
            cursor = align_up(cursor, align);
            entries.push(FieldEntry::Scalar(f.name.clone(), cursor, width, signed));
            cursor += width.bytes();
            max_align = max_align.max(align);
        } else if let Ty::Struct(name) = &f.ty {
            // A *nested* field must still be a one-eightbyte struct — see
            // `width_for_size`'s doc — regardless of what the third slice
            // does for a top-level value's own size. Only a top-level
            // `datac` gets the wider 9-16-byte/`MEMORY` treatment.
            let inner = out.get(name)?;
            width_for_size(inner.size)?;
            let align = inner.align;
            cursor = align_up(cursor, align);
            entries.push(FieldEntry::Nested(f.name.clone(), cursor, name.clone()));
            cursor += inner.size;
            max_align = max_align.max(align);
        } else {
            return None;
        }
    }
    let size = align_up(cursor, max_align);
    // MK7 §5 item 4, third slice: no size cutoff here any more — every size
    // gets a layout, and `AggClass` (derived from `size` wherever a value is
    // actually moved) is what decides whether it needs eightbyte-pair or
    // hidden-pointer handling. Only a *nested* field (above) is still capped.
    Some(StructLayout { entries, size, align: max_align })
}

/// Computes `StructLayout` for every `datac` that qualifies (see
/// `StructLayout`'s doc), by simulating the same sequential, naturally-aligned
/// layout a C compiler gives the equivalent typedef. A struct that does not
/// qualify is simply absent from the returned map. Nested structs mean a
/// struct's own qualification can depend on another one's, in either textual
/// order, so this runs to a fixed point rather than in one pass — bounded by
/// `structs.len()` passes, which is enough to resolve any nesting depth since
/// a by-value `datac` cannot nest itself (rejected earlier, in `infer.rs`).
fn build_layouts(structs: &[crate::types::StructDef]) -> std::collections::HashMap<String, StructLayout> {
    let mut out = std::collections::HashMap::new();
    for _ in 0..=structs.len() {
        let mut changed = false;
        for s in structs {
            if out.contains_key(&s.name) {
                continue;
            }
            if let Some(layout) = try_build_layout(s, &out) {
                out.insert(s.name.clone(), layout);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// `ty`'s layout, if it is a `datac` `build_layouts` accepted.
fn struct_layout<'a>(
    ty: &Ty,
    layouts: &'a std::collections::HashMap<String, StructLayout>,
) -> Option<&'a StructLayout> {
    match ty {
        Ty::Struct(name) => layouts.get(name),
        _ => None,
    }
}

/// `int_ty`, widened to also accept a qualifying aggregate as one opaque
/// eightbyte (`Width::W8`, `signed` is meaningless there since `Width::W8`'s
/// `extend` is already a no-op — see `StructLayout`'s doc for why this is
/// sound). Every site that used to call `int_ty(ty).ok_or_else(...)` for a
/// value it moves whole (a local, a parameter, a return, a call argument)
/// calls this instead; a site that needs a *field*'s own width still goes
/// through `StructLayout::fields` directly.
fn value_kind(
    ty: &Ty,
    layouts: &std::collections::HashMap<String, StructLayout>,
) -> Option<(Width, bool)> {
    int_ty(ty)
        // MK7 §5 item 4, third slice: only a *one-eightbyte* struct is an
        // opaque `Width::W8` value moved through `rax` this way any more — a
        // `Two`/`Mem`-class struct has its own transport (a register pair or
        // a hidden pointer) and is handled at each site through
        // `struct_layout`/`AggClass` directly, never through this function.
        .or_else(|| {
            struct_layout(ty, layouts)
                .filter(|l| l.class() == AggClass::One)
                .map(|_| (Width::W8, false))
        })
        .or_else(|| match ty {
            // A `&datac`/`&mut datac` is a plain pointer — `Width::W8`, same
            // as the aggregate itself, just one level of indirection away.
            // Nothing else about `Ty::Ref` is accepted here: a reference to a
            // scalar needs assignment-through-a-pointer codegen this slice
            // does not have (`ExprKind::Deref` as an *assignment target*
            // stays refused), so only the `datac` case — read-only field
            // access through the pointer — is claimed.
            Ty::Ref { inner, .. } if struct_layout(inner, layouts).is_some() => {
                Some((Width::W8, false))
            }
            _ => None,
        })
}

/// How many consecutive *integer* argument/parameter registers a value of
/// `ty` occupies, or `None` if `ty` cannot be passed by value at all (a
/// `String`/`List`/`Map`/`Box`, a `str`, a scalar reference — every type this
/// backend still refuses). A `MEMORY`-class `datac` is `Some(0)`: it *can* be
/// passed by value, and the fourth slice passes it the way SysV says to, in
/// memory rather than in any register — see `memory_arg_area`. That zero is
/// what keeps a `Mem`-class argument from disturbing the numbering of the
/// arguments around it, since the integer register file is numbered by how
/// many registers each earlier argument took, and this one took none.
///
/// (The third slice drew the opposite conclusion — `None`, a flat refusal —
/// because passing one that way needs the value staged somewhere addressable
/// until the call executes. The fourth slice's `Frame::scratch` is exactly that
/// storage, so the narrowing is gone.) `float_ty` values are not covered here —
/// every call site checks that first, since SysV classifies the vector and
/// integer register files independently.
fn int_regs_for(ty: &Ty, layouts: &std::collections::HashMap<String, StructLayout>) -> Option<usize> {
    if let Some(layout) = struct_layout(ty, layouts) {
        return match layout.class() {
            AggClass::One => Some(1),
            AggClass::Two => Some(2),
            AggClass::Mem => Some(0),
        };
    }
    if int_ty(ty).is_some() {
        return Some(1);
    }
    if let Ty::Ref { inner, .. } = ty {
        if struct_layout(inner, layouts).is_some() {
            return Some(1);
        }
    }
    None
}

/// A signed frame-relative or pointer-relative displacement, formatted with
/// an explicit sign — `[rbp{off_str(-24)}]` is `[rbp-24]`, matching every
/// existing frame-slot reference in this module, but `[r11{off_str(8)}]` is
/// `[r11+8]`, which plain `i32` interpolation would get wrong (`[r118]`).
/// Needed once offsets stop being frame slots (always negative, by
/// construction) and start being byte offsets from an arbitrary base
/// (`emit_copy`, `Dest::Indirect`), which can be zero or positive.
fn off_str(n: i32) -> String {
    if n >= 0 {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

/// Copies exactly `size` bytes from `[src_reg+src_off]` to `[dst_reg+dst_off]`
/// through `rax`, in falling chunk widths (8, then 4, then 2, then 1 — enough
/// to cover any byte count with no alignment assumption on `size` itself,
/// since a `datac`'s total size is only ever aligned to its *widest field*,
/// which can still leave an odd tail: three `i32`s is 12 bytes, not a
/// multiple of eight). This is the whole of how a `Two`/`Mem`-class `datac`
/// value moves between two named locations (a local read into another local,
/// or a parameter's incoming pointer into its frame slot) — there is no
/// single instruction that moves an arbitrary-width blob, unlike the
/// one-eightbyte case's plain `mov`.
fn emit_copy(out: &mut String, dst_reg: &str, dst_off: i32, src_reg: &str, src_off: i32, mut size: i32) {
    let mut off = 0i32;
    while size > 0 {
        let (w, n) = if size >= 8 {
            (Width::W8, 8)
        } else if size >= 4 {
            (Width::W4, 4)
        } else if size >= 2 {
            (Width::W2, 2)
        } else {
            (Width::W1, 1)
        };
        let _ = writeln!(
            out,
            "    mov {}, {} [{src_reg}{}]",
            w.rax(),
            w.mem(),
            off_str(src_off + off)
        );
        let _ = writeln!(
            out,
            "    mov {} [{dst_reg}{}], {}",
            w.mem(),
            off_str(dst_off + off),
            w.rax()
        );
        off += n;
        size -= n;
    }
}

/// How a function's own return value leaves it — the return-side counterpart
/// of `AggClass`, widened to also cover the non-aggregate cases so every
/// function has exactly one of these rather than `AggClass` plus a separate
/// "or nothing" case scattered across call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetClass {
    Unit,
    Scalar,
    Float,
    AggOne,
    AggTwo,
    AggMem,
}

/// Where a `Two`/`Mem`-class `datac` value is being written, for the sites
/// that can produce one (`Gen::eval_struct_into`, and `Gen::call`'s own
/// return handling): either a plain frame-relative address (`Direct`, used
/// for a local's own slot or the scratch slot a call's outgoing argument or a
/// function's own `Two`-class return stages through), or a pointer that
/// itself lives in a frame slot (`Indirect` — the hidden return-pointer case,
/// `RetClass::AggMem`, where the *destination* is wherever the caller
/// pointed, not a location this function owns). `Indirect` always reloads the
/// pointer from its slot immediately before each use rather than caching it
/// in a register across other codegen (in particular, across whatever
/// `struct_lit_at` evaluates for one field before storing the next) — a
/// register is never guaranteed to survive a nested call, and a frame slot
/// always is.
#[derive(Clone, Copy)]
enum Dest {
    Direct(i32),
    Indirect(i32),
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
    if let Some(e) = program.enums.first() {
        return Err(unsupported(&format!("the enum `{}`", e.name)));
    }

    let mut g = Gen::default();
    // MK7 §5 item 4, first slice: a `datac` is no longer refused outright — a
    // program keeps compiling as long as every `datac` it actually *uses* is
    // one `build_layouts` accepted (see `StructLayout`'s doc). A `datac` that
    // does not qualify is not rejected here either: it is rejected wherever a
    // program tries to move a value of that type, which names the offending
    // type instead of the mere presence of an unrelated declaration.
    g.layouts = build_layouts(&program.structs);
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

/// MK7 §5 item 3: a linear-scan register allocator over each function's
/// locals, run once before that function's body is generated.
///
/// The stack machine `Gen::expr` runs stays exactly as it was — `rax`/`rcx`
/// and `xmm0`/`xmm1` are still where an expression's intermediate values
/// live, spilled to the real stack across a right operand's evaluation
/// exactly as before. What this item changes is *where a named local lives
/// between statements*: today every one of them is a frame slot, reloaded
/// from memory at every read and stored to memory at every write. A local
/// whose live range fits is now instead pinned to one of five callee-saved
/// registers (`rbx`, `r12`-`r15`) for the whole function, and a read or write
/// becomes a register-to-register `mov` — no memory traffic, and (because
/// they are callee-saved) no reloading across a call either.
///
/// `tir.rs` has no CFG or SSA form — it is the structured AST `infer.rs`
/// produced, with `if`/`while` as nested `Block`s — so this does not attempt
/// general liveness analysis over a control-flow graph. Instead it assigns
/// every statement and expression node a strictly increasing position in a
/// single preorder walk (`LiveAnalysis::pos`), and takes each local's raw
/// interval as `[first touch, last touch]` in that numbering. That alone is
/// already correct for anything whose scope does not cross a loop: if a local
/// is declared before a `while` and used only after it, the loop sits
/// entirely inside its interval by construction, so the interval already
/// reserves the loop's positions without the local ever being touched there.
///
/// It is *not* correct, on its own, for a local that is touched **inside** a
/// loop and needed again on the next iteration: its last textual touch can
/// sit early in the loop body, with more code after it before the back edge,
/// and a naive interval would let that register be handed to something else
/// for the remainder of the body — live across one iteration boundary but not
/// the next, invisible to the C backend's memory-backed locals but fatal to a
/// register one. `LiveAnalysis` corrects for exactly this: for every `while`,
/// any local declared **outside** it whose raw interval already overlaps the
/// loop's `[condition-start, body-end]` span has that interval widened to
/// cover the span in full. A local declared *inside* the loop is exempt —
/// codegen visits its `Let` once regardless of how many times the loop runs
/// at runtime, so it gets one physical location reused every iteration
/// either way, and nothing outside the loop ever needs its value to survive
/// past the body's end. Nested loops are handled by applying this widening
/// smallest-span-first (which, for a properly nested AST, is innermost
/// first): an already-widened inner interval can then trigger the outer
/// loop's own widening in the same pass, with no fixed point needed.
///
/// A local this item is not confident about does not get a register — see
/// `allocate_registers`'s doc for what stays off the table entirely (`f32`/
/// `f64`; nothing else is excluded, but nothing else is attempted either
/// beyond the interval discipline above).
struct LiveAnalysis {
    pos: usize,
    scopes: Vec<Vec<(String, usize)>>,
    /// Per declaration id, in the same order `Frame::declare` will hand them
    /// out: `(first touch, last touch)`, both inclusive, before any loop
    /// widening.
    ranges: Vec<(usize, usize)>,
    /// Per declaration id: the position the `Let` (or, for a parameter,
    /// position 0) itself sits at — this is what "declared outside the loop"
    /// is tested against.
    decl_pos: Vec<usize>,
    /// Per declaration id: whether it is a float, which this item never
    /// register-allocates (see `allocate_registers`).
    is_float: Vec<bool>,
    /// Per declaration id: whether it is a `datac` (MK7 §5 item 4). Also
    /// never register-allocated: a struct local's only reader that needs its
    /// *address* is `Gen::field_access`, and giving every struct local a
    /// stack slot — the same way a float local always does — is what lets
    /// that stay a plain `[rbp+offset]` computation instead of a second,
    /// register-aware addressing mode nothing else in this module needs yet.
    is_agg: Vec<bool>,
    /// One `(start, end)` per `while` encountered, collected in the same
    /// preorder the loops are visited (so an inner loop's span is pushed
    /// before its enclosing loop's, though nothing here depends on that
    /// order — `allocate_registers` sorts by span width itself).
    spans: Vec<(usize, usize)>,
}

impl LiveAnalysis {
    fn new() -> LiveAnalysis {
        LiveAnalysis {
            pos: 0,
            scopes: vec![Vec::new()],
            ranges: Vec::new(),
            decl_pos: Vec::new(),
            is_float: Vec::new(),
            is_agg: Vec::new(),
            spans: Vec::new(),
        }
    }

    fn tick(&mut self) -> usize {
        self.pos += 1;
        self.pos
    }

    /// Records a declaration and returns its id. Ids are handed out in
    /// exactly the order `Frame::declare` will later be called in — params
    /// first (by `Gen::function`), then each `Let` in preorder — which is
    /// what lets `allocate_registers`'s output be indexed by id and consumed
    /// by `Frame::declare` with no other bookkeeping.
    fn declare(&mut self, name: &str, ty: &Ty) -> usize {
        let id = self.ranges.len();
        let p = self.tick();
        self.ranges.push((p, p));
        self.decl_pos.push(p);
        self.is_float.push(float_ty(ty).is_some());
        self.is_agg.push(matches!(ty, Ty::Struct(_)));
        self.scopes
            .last_mut()
            .expect("a scope always exists")
            .push((name.to_string(), id));
        id
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            for (n, id) in scope.iter().rev() {
                if n == name {
                    return Some(*id);
                }
            }
        }
        None
    }

    /// A read *or* a write of `name` at the current position — both extend
    /// the interval the same way, since either one means the register (or
    /// slot) has to hold the right value at this point.
    fn touch(&mut self, name: &str) {
        let p = self.tick();
        if let Some(id) = self.lookup(name) {
            let r = &mut self.ranges[id];
            r.0 = r.0.min(p);
            r.1 = r.1.max(p);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn function_body(&mut self, f: &Function) {
        for stmt in &f.body.stmts {
            self.stmt(stmt);
        }
        if let Some(tail) = &f.body.tail {
            self.expr(tail);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, ty, init } => {
                self.expr(init);
                self.declare(name, ty);
            }
            Stmt::While { cond, body } => {
                let start = self.pos;
                self.expr(cond);
                self.block(body);
                self.spans.push((start, self.pos));
            }
            Stmt::Return { value, .. } => match value {
                Some(v) => self.expr(v),
                None => {
                    self.tick();
                }
            },
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn block(&mut self, block: &Block) {
        self.push_scope();
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.expr(tail);
        }
        self.pop_scope();
    }

    /// Covers every node `Gen::expr` can actually lower. Anything else falls
    /// to the catch-all and is left untouched: `Gen::expr` refuses every one
    /// of those kinds outright, so a function containing one never reaches
    /// code generation for the (possibly under-counted) locals around it —
    /// see this struct's own doc comment for why that makes the omission
    /// safe rather than a second, silent gap.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::IntLit(_) | ExprKind::BoolLit(_) | ExprKind::CharLit(_) | ExprKind::FloatLit(_) => {
                self.tick();
            }
            ExprKind::Ident(name) => self.touch(name),
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Assign { target, value } => {
                self.expr(value);
                // A `FieldAccess` target is a write *through* its base, so the
                // base is as live at this point as it would be for a read —
                // which matters for a `&datac`/`&mut datac` parameter, since a
                // reference to a `datac` is a register candidate (`is_agg`
                // only covers a bare `Ty::Struct`).
                match &target.kind {
                    ExprKind::Ident(name) => self.touch(name),
                    ExprKind::FieldAccess { .. } => self.expr(target),
                    _ => {
                        self.tick();
                    }
                }
            },
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(else_block) = else_block {
                    self.block(else_block);
                }
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::Call { args, .. } => {
                for a in args {
                    self.expr(a);
                }
                self.tick();
            }
            ExprKind::Template { pieces, .. } => {
                for p in pieces {
                    if let TemplatePiece::Value(v) = p {
                        self.expr(v);
                    }
                }
            }
            // MK7 §5 item 4, third slice: a struct literal's field
            // initializers are ordinary expressions and can read a plain
            // scalar local or parameter (`Pair3(a = a, b = b)`) — a real gap
            // this item's own fixtures found, not something the third slice
            // introduces: without this arm, `a`/`b` above would look
            // dead-on-declaration to `allocate_registers` (nothing after
            // their `Let`/parameter binding ever calls `touch`), and the
            // linear scan would happily hand two live parameters the *same*
            // register, believing their intervals never overlap. A struct
            // literal's own base has no name to `touch` (it is not a read of
            // an existing local), so only its fields are walked.
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            // MK7 §5 item 4, fourth slice: a field read, a borrow and a
            // dereference all *read* their inner expression, and the inner
            // expression is normally a name. Leaving these to the catch-all
            // (which only advances the position) is the same liveness gap the
            // `StructLit` arm above was added for, one layer out: a
            // `&datac` parameter that is only ever read through
            // (`total(c)` in `fun total(c: Counter)`) would look dead on
            // arrival, and a second such parameter would be handed the *same*
            // pool register — caught by
            // `tests/fixtures/borrow_temp_slots.kp`'s two-reference
            // `pair_sum`, whose two parameters both landed in `r15`.
            ExprKind::FieldAccess { base, .. } => self.expr(base),
            ExprKind::Ref { expr, .. } => self.expr(expr),
            ExprKind::Deref { expr } => self.expr(expr),
            _ => {
                self.tick();
            }
        }
    }
}

/// Runs `LiveAnalysis` over `f`'s parameters and body, applies the loop
/// widening, then a first-fit linear scan over `CALLEE_SAVED`. Returns one
/// slot per declaration id, `Some(register index)` or `None` for a spill —
/// `Frame::declare` consumes this vector id-by-id in the same order it was
/// built in.
///
/// `f32`/`f64` locals are never assigned a register here: xmm live-range
/// allocation is a second, separate piece of work (the plan explicitly notes
/// item 2 kept float locals on the stack-machine, memory-backed discipline
/// rather than pre-empting this item), and this function keeps that boundary
/// by excluding every id `LiveAnalysis` marked `is_float` from the candidate
/// list before the scan ever runs — a float id is `None` unconditionally.
fn allocate_registers(f: &Function) -> Vec<Option<usize>> {
    let mut la = LiveAnalysis::new();
    for (name, ty) in &f.params {
        la.declare(name, ty);
    }
    la.function_body(f);

    // Widen smallest span first, which for a properly nested AST is
    // innermost first: by the time an outer loop's span is processed, any
    // interval an inner loop already widened is widened again if it now
    // reaches the outer loop's own span (it does, if the local was touched
    // anywhere inside the inner loop, since the inner loop's span sits
    // inside the outer one).
    let mut spans = la.spans.clone();
    spans.sort_by_key(|&(s, e)| e - s);
    for (start, end) in spans {
        for id in 0..la.ranges.len() {
            // Declared inside this loop's own span: exempt, see the struct
            // doc for why codegen's single visit per `Let` makes this safe.
            if la.decl_pos[id] >= start && la.decl_pos[id] <= end {
                continue;
            }
            let (mn, mx) = la.ranges[id];
            if mn <= end && mx >= start {
                la.ranges[id] = (mn.min(start), mx.max(end));
            }
        }
    }

    struct Interval {
        id: usize,
        start: usize,
        end: usize,
    }
    let mut intervals: Vec<Interval> = (0..la.ranges.len())
        .filter(|&id| !la.is_float[id] && !la.is_agg[id])
        .map(|id| Interval {
            id,
            start: la.ranges[id].0,
            end: la.ranges[id].1,
        })
        .collect();
    intervals.sort_by_key(|iv| iv.start);

    let mut assign: Vec<Option<usize>> = vec![None; la.ranges.len()];
    let mut free: Vec<usize> = (0..CALLEE_SAVED.len()).collect();
    // `(end position, register)` for every interval currently holding a
    // register, so a later interval's start can expire whichever of these
    // are already over.
    let mut active: Vec<(usize, usize)> = Vec::new();
    for iv in intervals {
        active.retain(|&(end, reg)| {
            if end < iv.start {
                free.push(reg);
                false
            } else {
                true
            }
        });
        free.sort_unstable();
        if let Some(reg) = free.pop() {
            assign[iv.id] = Some(reg);
            active.push((iv.end, reg));
        }
        // Else: no register free, `assign[iv.id]` stays `None` — the same
        // frame-slot path every local used before this item, unchanged.
    }
    assign
}

/// One function's frame: each local gets a location fixed when its
/// declaration is reached (a `Let` initializer cannot name the binding being
/// made, so no location is ever needed before it is assigned) — either one of
/// the callee-saved registers `allocate_registers` handed it, or, as every
/// local got before this item, a stack slot whose offset is likewise decided
/// at that point and the body can be generated before the frame's size is
/// known.
struct Frame {
    /// Bytes reserved below `rbp`, grown one slot per *spilled* declaration
    /// (a register-allocated one costs nothing here). Starts at
    /// `SP_SAVE_SIZE`, because the first slot is not a local: it belongs to
    /// the call-site stack-pointer save (see `sp_slot`).
    used: i32,
    /// Innermost last, so a shadowed name is restored when its block ends.
    scopes: Vec<Vec<(String, Loc, Ty)>>,
    /// `allocate_registers`'s output, consumed one id at a time as
    /// `declare` is called — id `i` is `locs[i]`, and the call order here is
    /// guaranteed to match the order `LiveAnalysis` built it in because both
    /// walk the same function the same way (params, then body in preorder).
    locs: Vec<Option<usize>>,
    next_id: usize,
    /// Where this function's call sites save `rsp` across a call — see the
    /// doc below. No longer a fixed `[rbp-8]`: MK7 §5 item 3 pushes this
    /// function's used pool registers *between* `mov rbp, rsp` and the frame
    /// slots, at `[rbp-8]`, `[rbp-16]`, and so on, so a fixed `[rbp-8]` would
    /// alias the first pushed register's saved value and a call would
    /// silently overwrite a live callee-saved register with a stack pointer.
    /// This is computed once per function, below every pushed register.
    sp_slot: i32,
    /// MK7 §5 item 4: where a `datac` value under construction or in transit
    /// is staged — field by field for a struct literal (first slice), then,
    /// as of the third slice, also a `Two`-class by-value call argument
    /// (built here, then both its eightbytes pushed) and a function's own
    /// `Two`-class return value (written here by every `return`/tail site,
    /// then loaded into `rax`/`rdx` once at `return_label`). Sixteen bytes —
    /// widened from the first slice's eight — because a `Two`-class value
    /// needs both eightbytes live at once. Reserved once per function
    /// directly below `sp_slot`, exactly the way `sp_slot` itself is reserved
    /// below every pushed register. A single fixed slot is safe to reuse
    /// across every use because none of them overlaps in time: an argument
    /// list evaluates one aggregate fully (into here, then off the stack via
    /// `push`) before the next argument starts, and a function's `return`
    /// value is written and immediately consumed by the following `jmp`.
    agg_slot: i32,
    /// MK7 §5 item 4, third slice: where a `Mem`-class (`AggClass::Mem`)
    /// function stashes the hidden return pointer its caller passed in `rdi`,
    /// so it survives past whatever else the body does with `rdi` before the
    /// value is actually written — see `Dest::Indirect`. Unused (but always
    /// reserved — a fixed per-function layout is simpler than a conditional
    /// one) when `ret_class` is not `RetClass::AggMem`.
    ret_ptr_slot: i32,
    /// This function's own `RetClass`, carried on `Frame` rather than `Gen`
    /// because `Stmt::Return` and a function body's tail expression both need
    /// it and both already thread `frame` through, whereas threading it
    /// through `Gen` would mean resetting a field at the start of every
    /// function instead of it simply falling out of scope with the frame.
    ret_class: RetClass,
}

/// Where a call site saves `rsp` across the call — `Frame::sp_slot`, computed
/// once per function as `-8 * (pushed registers + 1)`.
///
/// It is a frame slot rather than a register, and that is the whole point: the
/// first version of this used `r11`, which is **caller-saved**, so `printf`
/// overwrote it and `mov rsp, r11` after the call restored a garbage stack
/// pointer. The bug was invisible in a function that printed once — the
/// epilogue reloads `rsp` from `rbp`, so the corruption was masked until a
/// second call tried to push onto it. Memory cannot be clobbered by the callee,
/// so this cannot recur, and it costs the same two instructions.
const SP_SAVE_SIZE: i32 = 8;

/// `Frame::agg_slot`'s width — see that field's doc for why it grew from
/// eight bytes to this.
const AGG_SLOT_SIZE: i32 = 16;

impl Frame {
    fn new(locs: Vec<Option<usize>>, pushed_regs: usize, ret_class: RetClass) -> Frame {
        let sp_slot = -SP_SAVE_SIZE * (pushed_regs as i32 + 1);
        let agg_slot = sp_slot - AGG_SLOT_SIZE;
        let ret_ptr_slot = agg_slot - SP_SAVE_SIZE;
        Frame {
            // `-ret_ptr_slot`, the lowest (most negative) reserved offset so
            // far, is exactly how many bytes are already spoken for below
            // `rbp` before the first real local is declared.
            used: -ret_ptr_slot,
            scopes: vec![Vec::new()],
            locs,
            next_id: 0,
            sp_slot,
            agg_slot,
            ret_ptr_slot,
            ret_class,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to its next declaration id's location — a register if
    /// `allocate_registers` gave it one, else a fresh stack slot exactly as
    /// every local was assigned before this item.
    fn declare(
        &mut self,
        name: &str,
        ty: &Ty,
        layouts: &std::collections::HashMap<String, StructLayout>,
    ) -> Result<Loc, String> {
        // A struct local's slot is sized from its own layout regardless of
        // class — `value_kind` only ever answers for a `One`-class struct
        // (see its doc), so a `Two`/`Mem`-class local has to be sized here
        // directly or it would silently get an 8-byte slot with the rest of
        // its bytes overlapping whatever the next local reserves.
        let bytes = if let Some(layout) = struct_layout(ty, layouts) {
            align_up(layout.size, 8)
        } else {
            value_kind(ty, layouts)
                .map(|(w, _)| w.bytes())
                .or_else(|| float_ty(ty).map(|w| w.bytes()))
                .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?
        };
        let assigned = self.locs[self.next_id];
        self.next_id += 1;
        let loc = match assigned {
            Some(reg) => Loc::Reg(reg),
            None => {
                self.used += bytes;
                Loc::Stack(-self.used)
            }
        };
        self.scopes
            .last_mut()
            .expect("a scope always exists")
            .push((name.to_string(), loc, ty.clone()));
        Ok(loc)
    }

    /// MK7 §5 item 4, fourth slice: reserves `bytes` of scratch below every
    /// declared local and returns its frame offset. Unlike `declare` this
    /// consumes no `next_id`, so it cannot desync the codegen-time declaration
    /// order from `LiveAnalysis`'s — nothing here is a name a program can read,
    /// it is storage for a value whose *address* outlives the expression that
    /// produced it (a borrowed temporary) or that has to survive until a call
    /// executes (a `MEMORY`-class argument, see `Gen::call`).
    ///
    /// A slot reserved here is **never reused** by a later one. That is the
    /// whole reason the previous slice's `agg_slot` could not serve: a single
    /// shared staging slot is invalidated by the next value staged into it, so
    /// a pointer into it dangles as soon as a second `datac` literal appears in
    /// the same call. Wasting a few bytes per syntactic occurrence is the price
    /// of an address that is stable for the enclosing statement; the frame is
    /// sized after the body is generated, so these are accounted for.
    fn scratch(&mut self, bytes: i32) -> i32 {
        self.used += bytes;
        -self.used
    }

    fn lookup(&self, name: &str) -> Option<(Loc, Ty)> {
        for scope in self.scopes.iter().rev() {
            for (n, loc, ty) in scope.iter().rev() {
                if n == name {
                    return Some((*loc, ty.clone()));
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
fn call_align_begin(out: &mut String, sp_slot: i32) {
    let _ = writeln!(out, "    mov QWORD PTR [rbp{sp_slot}], rsp");
    out.push_str("    and rsp, -16\n");
}

fn call_align_end(out: &mut String, sp_slot: i32) {
    let _ = writeln!(out, "    mov rsp, QWORD PTR [rbp{sp_slot}]");
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
    /// MK7 §5 item 4: every `datac` `build_layouts` accepted, keyed by name.
    layouts: std::collections::HashMap<String, StructLayout>,
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
            if float_ty(ty).is_none() && int_regs_for(ty, &self.layouts).is_none() {
                return Err(unsupported(&format!(
                    "the parameter `{name}: {}` of `{}`",
                    ty.display(),
                    f.name
                )));
            }
        }
        // MK7 §5 item 4, third slice: a function's return value is one of
        // six shapes (`RetClass`) rather than the two `value_kind`/`float_ty`
        // used to distinguish — a `Two`-class `datac` returns in `rax:rdx`,
        // and a `Mem`-class one through the hidden-pointer convention, both
        // of which `Stmt::Return`/the tail expression need to know about
        // (`Frame::ret_class`), not just whether the type is acceptable at
        // all.
        let ret_class = if f.ret == Ty::Unit {
            RetClass::Unit
        } else if float_ty(&f.ret).is_some() {
            RetClass::Float
        } else if let Some(layout) = struct_layout(&f.ret, &self.layouts) {
            match layout.class() {
                AggClass::One => RetClass::AggOne,
                AggClass::Two => RetClass::AggTwo,
                AggClass::Mem => RetClass::AggMem,
            }
        } else if int_ty(&f.ret).is_some() {
            RetClass::Scalar
        } else {
            return Err(unsupported(&format!(
                "the return type `{}` of `{}`",
                f.ret.display(),
                f.name
            )));
        };
        // A qualifying `datac` parameter consumes one *integer* argument
        // register if it is `One`- or `Two`-class... no, a `Two`-class value
        // needs two consecutive registers (`int_regs_for`), and a `Mem`-class
        // return reserves `rdi` for the hidden pointer before any real
        // parameter is numbered — both have to be counted or a function
        // could be wrongly accepted with more live integer registers than
        // SysV actually has.
        let mut int_regs_needed = if ret_class == RetClass::AggMem { 1 } else { 0 };
        for (_, ty) in &f.params {
            if let Some(n) = int_regs_for(ty, &self.layouts) {
                int_regs_needed += n;
            }
        }
        if int_regs_needed > ARG64.len() {
            return Err(unsupported(&format!(
                "`{}`, which needs more than six integer argument registers",
                f.name
            )));
        }
        if f.params.iter().filter(|(_, ty)| float_ty(ty).is_some()).count() > FARG.len() {
            return Err(unsupported(&format!(
                "`{}`, which takes more than eight float parameters",
                f.name
            )));
        }

        // MK7 §5 item 3: decide every local's location (register or slot)
        // before generating a single instruction of the body — a location
        // has to be fixed for the whole function, and the loop-widening
        // correction `allocate_registers` applies needs the *entire* body's
        // shape up front, not just what has been seen so far.
        let assign = allocate_registers(f);
        let mut used_regs: Vec<usize> = assign.iter().filter_map(|r| *r).collect();
        used_regs.sort_unstable();
        used_regs.dedup();

        let mut frame = Frame::new(assign, used_regs.len(), ret_class);
        frame.push_scope();
        // Parameters are bound to the same locations a local would use;
        // their incoming registers are moved into place (spilled to a frame
        // slot, or moved into a pool register) in the prologue, which is
        // written after the body because only the body knows the frame's
        // size. An integer and a float parameter at the same *position* do
        // not share a register: SysV classifies and numbers each register
        // file independently, so two counters are threaded through in
        // declaration order rather than one shared index.
        enum ParamSlot {
            Int(Loc, Width, bool, usize),
            Float(i32, FWidth, usize),
            /// MK7 §5 item 4, third slice: a `Two`-class `datac` parameter —
            /// `(frame offset, first eightbyte's `ARG64` index, second
            /// eightbyte's)`. Always a stack slot (struct locals are never
            /// register-allocated), never a single register the way a
            /// `One`-class parameter's `Int` variant can be.
            AggTwo(i32, usize, usize),
            /// MK7 §5 item 4, fourth slice: a `MEMORY`-class `datac`
            /// parameter — `(frame offset, byte offset in the caller's
            /// outgoing argument area, size)`. No register is involved at
            /// either end; the prologue copies the bytes out of the caller's
            /// area into this function's own slot, so the parameter behaves
            /// exactly like any other `datac` local afterwards (the model
            /// every struct-typed value in this module already follows — see
            /// `AggClass`'s doc).
            AggMem(i32, i32, i32),
        }
        let mut params: Vec<ParamSlot> = Vec::new();
        // The stack half of the argument convention, computed from the same
        // function `Gen::call` uses — see `memory_arg_area`.
        let (mem_params, _) = memory_arg_area(f.params.iter().map(|(_, ty)| ty), &self.layouts);
        // `rdi` (`ARG64[0]`) is the hidden return pointer for a `Mem`-class
        // return, so real parameter numbering starts one register later.
        let mut int_idx = if ret_class == RetClass::AggMem { 1 } else { 0 };
        let mut float_idx = 0usize;
        for (pos, (name, ty)) in f.params.iter().enumerate() {
            let loc = frame.declare(name, ty, &self.layouts)?;
            if let Some(width) = float_ty(ty) {
                let offset = loc
                    .stack_offset()
                    .expect("float locals are never register-allocated");
                params.push(ParamSlot::Float(offset, width, float_idx));
                float_idx += 1;
            } else if let Some(layout) = struct_layout(ty, &self.layouts) {
                match layout.class() {
                    AggClass::One => {
                        params.push(ParamSlot::Int(loc, Width::W8, false, int_idx));
                        int_idx += 1;
                    }
                    AggClass::Two => {
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        params.push(ParamSlot::AggTwo(off, int_idx, int_idx + 1));
                        int_idx += 2;
                    }
                    AggClass::Mem => {
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        let (_, stack_off, size) = *mem_params
                            .iter()
                            .find(|(i, _, _)| *i == pos)
                            .expect("memory_arg_area covers every Mem-class parameter");
                        params.push(ParamSlot::AggMem(off, stack_off, size));
                    }
                }
            } else {
                let (width, signed) =
                    value_kind(ty, &self.layouts).expect("int_ty or Ref-to-struct, checked above");
                params.push(ParamSlot::Int(loc, width, signed, int_idx));
                int_idx += 1;
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
        // Every pool register this function actually uses is saved here and
        // restored in the epilogue, in exact reverse — see the epilogue's
        // comment for why plain `add`/`pop`, rather than `mov rsp, rbp`, is
        // what makes that pairing correct regardless of how many locals
        // spilled.
        for &reg in &used_regs {
            let _ = writeln!(self.out, "    push {}", CALLEE_SAVED64[reg]);
        }
        let frame_size = frame.size().max(16);
        let _ = writeln!(self.out, "    sub rsp, {frame_size}");

        if ret_class == RetClass::AggMem {
            let rp = frame.ret_ptr_slot;
            let _ = writeln!(self.out, "    mov QWORD PTR [rbp{rp}], rdi");
        }
        for p in &params {
            match p {
                ParamSlot::AggTwo(off, idx0, idx1) => {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{off}], {}", ARG64[*idx0]);
                    let off2 = off + 8;
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{off2}], {}", ARG64[*idx1]);
                }
                ParamSlot::AggMem(off, stack_off, size) => {
                    // The caller's argument area starts at `[rbp+16]` — the
                    // return address and the pushed `rbp` are what sit between
                    // this frame's base and it. Copied rather than aliased so
                    // every `datac` local, whatever its class, is uniformly
                    // owned storage in this function's own frame; the callee is
                    // allowed to modify the incoming area, but nothing here
                    // depends on that. `emit_copy` clobbers only `rax`, which
                    // no other prologue move reads or writes (the frame's pool
                    // registers are `rbx`/`r12`-`r15`, and the incoming argument
                    // registers are `ARG64`/`FARG`).
                    let src = 16 + *stack_off;
                    emit_copy(&mut self.out, "rbp", *off, "rbp", src, *size);
                }
                ParamSlot::Int(Loc::Stack(offset), width, _signed, idx) => {
                    let _ = writeln!(
                        self.out,
                        "    mov {} [rbp{offset}], {}",
                        width.mem(),
                        ARG[width.idx()][*idx]
                    );
                }
                ParamSlot::Int(Loc::Reg(reg), width, signed, idx) => {
                    // The incoming value is only as wide as the parameter's
                    // own type — SysV does not promise the caller cleared the
                    // upper bits of a narrower argument — so this extends
                    // within the pool register exactly as a stack-machine
                    // operation extends `rax`, just aimed at a different
                    // register.
                    let names = CALLEE_SAVED[*reg];
                    let _ = writeln!(self.out, "    mov {}, {}", names[width.idx()], ARG[width.idx()][*idx]);
                    self.out.push_str(&width.extend_named(*signed, names));
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
        match ret_class {
            // MK7 §5 item 4, third slice: every `return`/tail site for this
            // function already wrote the value into `agg_slot`
            // (`RetClass::AggTwo`) or through the hidden pointer
            // (`RetClass::AggMem`) — see `Gen::stmt`'s `Return` arm and
            // `Gen::function_body`'s tail handling — so the one thing left is
            // loading the SysV-mandated registers from that fixed location,
            // once, regardless of which `return` actually ran.
            RetClass::AggTwo => {
                let agg = frame.agg_slot;
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{agg}]");
                let agg2 = agg + 8;
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{agg2}]");
            }
            RetClass::AggMem => {
                let rp = frame.ret_ptr_slot;
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{rp}]");
            }
            _ => {
                if f.name == "main" {
                    // `main`'s Kelp type is `unit`, and its C caller reads `rax`.
                    self.out.push_str("    xor eax, eax\n");
                }
            }
        }
        // The inverse of the prologue, in exact reverse order: `add rsp` puts
        // `rsp` back where it sat right after the pushes below (this is
        // correct regardless of alignment or spill count, because it is the
        // exact arithmetic inverse of the `sub` above — nothing between here
        // and there can have left `rsp` anywhere else, since every
        // stack-machine push within the body is popped before the statement
        // it belongs to ends, and `call_align_begin`/`call_align_end` restore
        // `rsp` from a memory slot rather than leaving it moved), then each
        // saved register is popped back into place before `rbp`/`ret`. A
        // `mov rsp, rbp` here, as before this item, would skip straight past
        // the pushed registers and never restore them.
        let _ = writeln!(self.out, "    add rsp, {frame_size}");
        for &reg in used_regs.iter().rev() {
            let _ = writeln!(self.out, "    pop {}", CALLEE_SAVED64[reg]);
        }
        self.out.push_str("    pop rbp\n    ret\n");
        let _ = writeln!(self.out, "    .size {symbol}, .-{symbol}");
        Ok(())
    }

    fn function_body(&mut self, f: &Function, frame: &mut Frame) -> Result<(), String> {
        for stmt in &f.body.stmts {
            self.stmt(stmt, frame)?;
        }
        if let Some(tail) = &f.body.tail {
            // MK7 §5 item 4, third slice: a tail expression is a `return`,
            // which `infer.rs` treats the same as `return <tail>` — so a
            // `Two`/`Mem`-class return value has to go through the same
            // `Dest`-based staging `Stmt::Return` uses below, not a plain
            // `self.expr` that would leave it (wrongly) in `rax` alone.
            match frame.ret_class {
                RetClass::AggTwo => {
                    let agg = frame.agg_slot;
                    self.eval_struct_into(tail, frame, Dest::Direct(agg))?;
                }
                RetClass::AggMem => {
                    let rp = frame.ret_ptr_slot;
                    self.eval_struct_into(tail, frame, Dest::Indirect(rp))?;
                }
                _ => self.expr(tail, frame)?,
            }
            if f.ret != Ty::Unit {
                let label = self.return_label.clone();
                let _ = writeln!(self.out, "    jmp {label}");
            }
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt, frame: &mut Frame) -> Result<(), String> {
        match stmt {
            Stmt::Let { name, ty, init } => {
                // MK7 §5 item 4, third slice: a `Two`/`Mem`-class `datac`
                // initializer is staged straight into its local's own slot
                // (`eval_struct_into`) rather than round-tripping through
                // `rax`, which cannot hold more than one eightbyte. This has
                // to declare the local *first* — unlike the scalar path
                // below, which evaluates before declaring purely because
                // nothing in that path needs the destination address ahead
                // of time.
                if let Some(layout) = struct_layout(ty, &self.layouts) {
                    if layout.class() != AggClass::One {
                        let loc = frame.declare(name, ty, &self.layouts)?;
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        return self.eval_struct_into(init, frame, Dest::Direct(off));
                    }
                }
                self.expr(init, frame)?;
                let loc = frame.declare(name, ty, &self.layouts)?;
                if let Some(width) = float_ty(ty) {
                    let offset = loc
                        .stack_offset()
                        .expect("float locals are never register-allocated");
                    let _ = writeln!(self.out, "    {} [rbp{offset}], xmm0", width.mov());
                    return Ok(());
                }
                let (width, _) = value_kind(ty, &self.layouts).expect("declare validated the type");
                match loc {
                    // The stack-machine invariant already leaves `rax`
                    // zero/sign-extended to 64 bits after evaluating `init`,
                    // which is exactly the invariant a pool register is
                    // supposed to hold between statements — so the store is
                    // a plain full-width copy, no width-specific mnemonic
                    // needed the way a memory store needs one.
                    Loc::Reg(reg) => {
                        let _ = writeln!(self.out, "    mov {}, rax", CALLEE_SAVED64[reg]);
                    }
                    Loc::Stack(offset) => {
                        let _ = writeln!(
                            self.out,
                            "    mov {} [rbp{offset}], {}",
                            width.mem(),
                            width.rax()
                        );
                    }
                }
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
                    Some(v) => match frame.ret_class {
                        RetClass::AggTwo => {
                            let agg = frame.agg_slot;
                            self.eval_struct_into(v, frame, Dest::Direct(agg))?;
                        }
                        RetClass::AggMem => {
                            let rp = frame.ret_ptr_slot;
                            self.eval_struct_into(v, frame, Dest::Indirect(rp))?;
                        }
                        _ => self.expr(v, frame)?,
                    },
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
                let (loc, ty) = frame
                    .lookup(name)
                    .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                if let Some(width) = float_ty(&ty) {
                    let offset = loc
                        .stack_offset()
                        .expect("float locals are never register-allocated");
                    let _ = writeln!(self.out, "    {} xmm0, [rbp{offset}]", width.mov());
                    return Ok(());
                }
                let (width, signed) = value_kind(&ty, &self.layouts)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
                match loc {
                    // A pool register already holds the full 64-bit
                    // zero/sign-extended value by construction — every write
                    // to it goes through the same invariant `rax` keeps — so
                    // reading it is a plain copy, no extension needed.
                    Loc::Reg(reg) => {
                        let _ = writeln!(self.out, "    mov rax, {}", CALLEE_SAVED64[reg]);
                    }
                    Loc::Stack(offset) => {
                        // Load at the value's own width and extend, so a
                        // narrow local never leaks neighbouring frame bytes
                        // into `rax`. The destination has to be the register
                        // of that same width — `mov rax, DWORD PTR [...]` is
                        // not an instruction — which is why this is the one
                        // `mov` that does not name `rax` directly.
                        let _ = writeln!(
                            self.out,
                            "    mov {}, {} [rbp{offset}]",
                            width.rax(),
                            width.mem()
                        );
                        self.out.push_str(&width.extend(signed));
                    }
                }
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
                if let ExprKind::FieldAccess { base, field } = &target.kind {
                    return self.field_assign(base, field, value, frame);
                }
                let (loc, ty) = match &target.kind {
                    ExprKind::Ident(name) => frame.lookup(name).ok_or_else(|| {
                        unsupported(&format!("assignment to the name `{name}`"))
                    })?,
                    _ => return Err(unsupported("assignment to anything but a local")),
                };
                if let Some(layout) = struct_layout(&ty, &self.layouts) {
                    if layout.class() != AggClass::One {
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        self.eval_struct_into(value, frame, Dest::Direct(off))?;
                        return Ok(());
                    }
                }
                self.expr(value, frame)?;
                if let Some(width) = float_ty(&ty) {
                    let offset = loc
                        .stack_offset()
                        .expect("float locals are never register-allocated");
                    let _ = writeln!(self.out, "    {} [rbp{offset}], xmm0", width.mov());
                    // An assignment's value is the assigned value, so `xmm0`
                    // is already correct and is deliberately not reloaded.
                    return Ok(());
                }
                let (width, _) = value_kind(&ty, &self.layouts)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
                match loc {
                    Loc::Reg(reg) => {
                        let _ = writeln!(self.out, "    mov {}, rax", CALLEE_SAVED64[reg]);
                    }
                    Loc::Stack(offset) => {
                        let _ = writeln!(
                            self.out,
                            "    mov {} [rbp{offset}], {}",
                            width.mem(),
                            width.rax()
                        );
                    }
                }
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
            ExprKind::Call { callee, args } => self.call(callee, args, &e.ty, None, frame),
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
            ExprKind::FieldAccess { base, field } => self.field_read(base, field, frame),
            ExprKind::StructLit { name, fields } => self.struct_lit(name, fields, frame),
            ExprKind::EnumLit { .. } => Err(unsupported("an enum literal")),
            ExprKind::EnumTag { .. } => Err(unsupported("an enum tag test")),
            ExprKind::EnumField { .. } => Err(unsupported("an enum payload read")),
            ExprKind::Try { .. } => Err(unsupported("the `?` operator")),
            ExprKind::Ref { expr, .. } => {
                // MK7 §5 item 4, first slice: a borrow of a named `datac`
                // local, which is what most of this item's gaps actually need
                // — a `datac`-typed call argument that is not marked `move`
                // becomes exactly this (`infer.rs::check_call_arg`), and it is
                // also what an explicit `&c` compiles to. No copy: the local's
                // own slot already *is* the object.
                if let ExprKind::Ident(name) = &expr.kind {
                    let (loc, ty) = frame
                        .lookup(name)
                        .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                    if struct_layout(&ty, &self.layouts).is_some() {
                        let off = loc
                            .stack_offset()
                            .expect("struct locals are never register-allocated");
                        let _ = writeln!(self.out, "    lea rax, [rbp{off}]");
                        return Ok(());
                    }
                }
                // MK7 §5 item 4, fourth slice: a borrow of a *temporary* —
                // `sum(Pair(a = 1, b = 2))`, or the same shape over a call that
                // returns a `datac`. The temporary gets a slot of its own
                // (`Frame::scratch`), built into it the way a `let`'s
                // initializer is built into its local, and the borrow is a
                // `lea` of that slot — so the address is valid for the whole
                // enclosing expression, which is exactly how long `borrowck.rs`
                // says such a borrow lasts. This is why a *fresh* slot and not
                // `agg_slot`: the shared staging slot would be overwritten by
                // the next `datac` literal the same call evaluates, which is
                // the hazard that made this shape a refusal until now.
                if let Some(layout) = struct_layout(&expr.ty, &self.layouts) {
                    let slot = frame.scratch(align_up(layout.size, 8));
                    self.eval_struct_into(expr, frame, Dest::Direct(slot))?;
                    let _ = writeln!(self.out, "    lea rax, [rbp{slot}]");
                    return Ok(());
                }
                Err(unsupported(
                    "a borrow of anything but a named `datac` local or a `datac` temporary",
                ))
            }
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

    /// MK7 §5 item 4: `Name { field = expr, ... }`. Every field is evaluated
    /// and stored straight into `frame.agg_slot` at its own offset and width —
    /// no different from an ordinary local's store — and the whole eightbyte
    /// is then loaded into `rax`, which is where every expression leaves its
    /// value regardless of type. `entries` is walked in declaration order
    /// (Kelp's literal syntax names each field, so the surface order can
    /// differ — but the *evaluation* order must still be declaration order,
    /// since a field initializer can have side effects) and each field's
    /// name is looked up in `fields` to find its expression. A nested-struct
    /// field (`FieldEntry::Nested`) evaluates the same way — its initializer
    /// is itself a `StructLit`, which leaves the nested value packed in
    /// `rax` — and is stored at its own byte width, computed from the nested
    /// struct's own size (`width_for_size`, guaranteed present by
    /// `try_build_layout` already having checked it).
    fn struct_lit(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
        frame: &mut Frame,
    ) -> Result<(), String> {
        let layout = self.layouts.get(name).cloned().ok_or_else(|| {
            unsupported(&format!(
                "the datac `{name}` (it is too large, or holds a field this slice cannot \
                 lower yet — see plan/mk7-own-backend.md §5 item 4)"
            ))
        })?;
        // MK7 §5 item 4, third slice: this rax-returning form is only sound
        // for a `One`-class literal — the four contexts that can hold a
        // wider value (`Let`, an ident-targeted `Assign`, a `return`/tail,
        // and a `Two`-class call argument) all intercept a `Two`/`Mem`-class
        // `StructLit` *before* reaching the generic `expr()` dispatch that
        // calls this function, via `eval_struct_into`/`struct_lit_at`
        // directly — so reaching here with a wider class would be this
        // module's own bug (a missed interception site), not a user program,
        // and the guard exists to fail loudly rather than truncate silently.
        if layout.class() != AggClass::One {
            return Err(unsupported(&format!(
                "the datac `{name}` used as a plain value (it is over eight bytes; only a \
                 `let`, an assignment, a `return`, or a call argument may hold one directly)"
            )));
        }
        let agg_slot = frame.agg_slot;
        self.struct_lit_at(name, fields, frame, Dest::Direct(agg_slot))?;
        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{agg_slot}]");
        Ok(())
    }

    /// The class-agnostic core of `struct_lit`: writes every field's value
    /// straight to `dest` (a fixed frame address, or a pointer stashed in a
    /// frame slot — see `Dest`), field by field, in declaration order (see
    /// `struct_lit`'s original doc for why the order matters). No trailing
    /// load into `rax` — the caller decides what "the value" means for a
    /// wider-than-one-eightbyte result, since `rax` alone cannot hold one.
    fn struct_lit_at(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
        frame: &mut Frame,
        dest: Dest,
    ) -> Result<(), String> {
        let layout = self.layouts.get(name).cloned().ok_or_else(|| {
            unsupported(&format!(
                "the datac `{name}` (it is too large, or holds a field this slice cannot \
                 lower yet — see plan/mk7-own-backend.md §5 item 4)"
            ))
        })?;
        for entry in &layout.entries {
            let (fname, foffset, fwidth) = match entry {
                FieldEntry::Scalar(n, o, w, _) => (n, *o, *w),
                FieldEntry::Nested(n, o, nested_name) => {
                    let nested = self.layouts.get(nested_name).expect(
                        "a nested field's struct is always in `self.layouts`, since \
                         `try_build_layout` only accepted it after looking it up there",
                    );
                    let w = width_for_size(nested.size).expect(
                        "try_build_layout already checked the nested struct's size is a valid move width",
                    );
                    (n, *o, w)
                }
            };
            let value = fields
                .iter()
                .find(|(n, _)| n == fname)
                .map(|(_, e)| e)
                .ok_or_else(|| {
                    unsupported(&format!("a `{name}` literal missing its `{fname}` field"))
                })?;
            self.expr(value, frame)?;
            match dest {
                Dest::Direct(base) => {
                    let addr = base + foffset;
                    let _ = writeln!(
                        self.out,
                        "    mov {} [rbp{addr}], {}",
                        fwidth.mem(),
                        fwidth.rax()
                    );
                }
                Dest::Indirect(slot) => {
                    // `rax` already holds the field's value; loading the
                    // pointer doesn't touch it, so no save/restore is needed
                    // around this the way `field_assign` needs one around its
                    // own address computation.
                    let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                    let _ = writeln!(
                        self.out,
                        "    mov {} [r11{}], {}",
                        fwidth.mem(),
                        off_str(foffset),
                        fwidth.rax()
                    );
                }
            }
        }
        Ok(())
    }

    /// MK7 §5 item 4, third slice: evaluates a `Two`/`Mem`-class `datac`
    /// expression directly into `dest`, for the handful of contexts that can
    /// hold one — a `let` initializer, an ident-targeted assignment, a
    /// `return`/tail value, and (from `call`) an outgoing `Two`-class
    /// argument. Only three expression shapes are covered, which between
    /// them are every one `infer.rs` can actually produce in those
    /// positions: a struct literal (`struct_lit_at`), a named local of the
    /// same struct type (a plain byte copy — `emit_copy`), and a call whose
    /// own return type is this wide (`call`'s own `Dest`-aware return
    /// handling). Anything else — nesting one of these inside a further
    /// expression, e.g. as another call's argument or a struct literal
    /// field's initializer — is refused: this slice's contexts are the only
    /// ones a wide `datac` value is staged through, deliberately, rather than
    /// threading `Dest` generally through every expression kind `expr`
    /// handles.
    fn eval_struct_into(&mut self, expr: &Expr, frame: &mut Frame, dest: Dest) -> Result<(), String> {
        match &expr.kind {
            ExprKind::StructLit { name, fields } => self.struct_lit_at(name, fields, frame, dest),
            ExprKind::Ident(name) => {
                let (loc, ty) = frame
                    .lookup(name)
                    .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                let layout = struct_layout(&ty, &self.layouts)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", ty.display())))?;
                let src_off = loc.stack_offset().expect("agg locals are stack-only");
                match dest {
                    Dest::Direct(off) => {
                        emit_copy(&mut self.out, "rbp", off, "rbp", src_off, layout.size);
                    }
                    Dest::Indirect(slot) => {
                        let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                        emit_copy(&mut self.out, "r11", 0, "rbp", src_off, layout.size);
                    }
                }
                Ok(())
            }
            ExprKind::Call { callee, args } => self.call(callee, args, &expr.ty, Some(dest), frame),
            _ => Err(unsupported(
                "this struct-returning expression (only a struct literal, a named local, or a \
                 call are supported for a value over eight bytes wide)",
            )),
        }
    }

    /// Computes `base`'s address into `r11` — a datac's own address, or the
    /// pointer a `&datac`/`&mut datac` value holds — and returns the layout
    /// to look a field up in. `r11` is caller-saved and otherwise unused in
    /// this module (its predecessor as the call-site `rsp` save is exactly
    /// why it was retired from that job — see `Frame::sp_slot`'s doc — but
    /// that hazard was about a value expected to survive *across* a call;
    /// nothing here does, the address is computed and consumed immediately).
    ///
    /// Four shapes of `base` are covered, which between them are every shape
    /// `infer.rs` produces for a field access this slice's types can reach:
    /// a plain local of `datac` type (`c.count`, address is its own frame
    /// slot); a plain local of `&datac`/`&mut datac` type (`c.count` again,
    /// but inside a function whose `c` parameter is a reference — `infer.rs`
    /// does not insert a separate `Deref` node for this case, the field
    /// access's `base` is simply the reference-typed `Ident` itself, so the
    /// distinction is only in which kind of value the slot holds); an
    /// explicit `*r` (`(*r).count`), whose inner expression is evaluated
    /// generically since anything typed `&datac` already knows how to put
    /// its pointer value in `rax` (it is `value_kind`'s `Width::W8` case, the
    /// same path a plain pointer-sized local uses); and a nested field access
    /// itself (`o.inner` as the base of `o.inner.v`) — the recursive case a
    /// nested `datac` needs, which resolves `inner`'s own offset within `o`
    /// by recursing on `o` first and then stepping `r11` forward by that
    /// offset, without a second `lea` (an `add` is enough, since the nested
    /// value lives inside the outer one rather than behind its own pointer).
    fn addr_of_struct(&mut self, base: &Expr, frame: &mut Frame) -> Result<StructLayout, String> {
        if let ExprKind::FieldAccess { base: inner_base, field } = &base.kind {
            let inner_layout = self.addr_of_struct(inner_base, frame)?;
            let (offset, nested_name) = inner_layout.nested_field(field).ok_or_else(|| {
                unsupported(&format!("the field `{field}` of `{}`", inner_base.ty.display()))
            })?;
            if offset != 0 {
                let _ = writeln!(self.out, "    add r11, {offset}");
            }
            return self
                .layouts
                .get(nested_name)
                .cloned()
                .ok_or_else(|| unsupported(&format!("the datac `{nested_name}`")));
        }
        match &base.kind {
            ExprKind::Ident(name) => {
                let (loc, ty) = frame
                    .lookup(name)
                    .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                if let Some(_layout) = struct_layout(&ty, &self.layouts) {
                    let off = loc
                        .stack_offset()
                        .expect("struct locals are never register-allocated");
                    let _ = writeln!(self.out, "    lea r11, [rbp{off}]");
                } else if let Ty::Ref { inner, .. } = &ty {
                    if struct_layout(inner, &self.layouts).is_none() {
                        return Err(unsupported(&format!("the type `{}`", ty.display())));
                    }
                    match loc {
                        Loc::Reg(reg) => {
                            let _ = writeln!(self.out, "    mov r11, {}", CALLEE_SAVED64[reg]);
                        }
                        Loc::Stack(off) => {
                            let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{off}]");
                        }
                    }
                } else {
                    return Err(unsupported(&format!("the type `{}`", ty.display())));
                }
            }
            ExprKind::Deref { expr } => {
                self.expr(expr, frame)?;
                self.out.push_str("    mov r11, rax\n");
            }
            _ => return Err(unsupported("a field access on anything but a local or `*local`")),
        }
        // Re-derive the layout from `base.ty` (stripping one `Ref`, the way
        // `infer.rs::struct_name_of` does) rather than threading the borrow
        // out of the match above — `self.layouts` outlives this call either
        // way, so this is only about not fighting the borrow checker over a
        // value already known.
        let inner_ty = match &base.ty {
            Ty::Ref { inner, .. } => inner.as_ref(),
            other => other,
        };
        struct_layout(inner_ty, &self.layouts)
            .cloned()
            .ok_or_else(|| unsupported(&format!("the type `{}`", base.ty.display())))
    }

    /// `base.field` as a value: load it at its own width and extend, through
    /// the address `addr_of_struct` computes into `r11`.
    fn field_read(&mut self, base: &Expr, field: &str, frame: &mut Frame) -> Result<(), String> {
        let layout = self.addr_of_struct(base, frame)?;
        let (foffset, width, signed) = layout
            .scalar_field(field)
            .ok_or_else(|| unsupported(&format!("the field `{field}` of `{}`", base.ty.display())))?;
        let _ = writeln!(self.out, "    mov {}, {} [r11+{foffset}]", width.rax(), width.mem());
        self.out.push_str(&width.extend(signed));
        Ok(())
    }

    /// `base.field = value`: the assignment counterpart of `field_read`. The
    /// address is computed *before* `value` is evaluated, so a value
    /// expression that itself reads through `r11` (nested field access is not
    /// reachable here today, but nothing about the ordering assumes that)
    /// cannot clobber the address it is about to be stored through.
    fn field_assign(
        &mut self,
        base: &Expr,
        field: &str,
        value: &Expr,
        frame: &mut Frame,
    ) -> Result<(), String> {
        let layout = self.addr_of_struct(base, frame)?;
        let (foffset, width, _signed) = layout
            .scalar_field(field)
            .ok_or_else(|| unsupported(&format!("the field `{field}` of `{}`", base.ty.display())))?;
        self.out.push_str("    push r11\n");
        self.expr(value, frame)?;
        self.out.push_str("    pop r11\n");
        let _ = writeln!(self.out, "    mov {} [r11+{foffset}], {}", width.mem(), width.rax());
        Ok(())
    }

    fn call(
        &mut self,
        callee: &str,
        args: &[Expr],
        ret_ty: &Ty,
        ret_dst: Option<Dest>,
        frame: &mut Frame,
    ) -> Result<(), String> {
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
        // MK7 §5 item 4, third slice: a call's own return shape — see
        // `RetClass`'s sibling classification here (there is no `RetClass`
        // value for "not a struct", so this stays a plain `AggClass` lookup
        // rather than reusing that enum). `ret_dst` is required exactly when
        // the return is wider than one eightbyte: `eval_struct_into` always
        // supplies one for such a call, and `expr()`'s generic dispatch never
        // does, so a struct-returning call reached generically (as a plain
        // value, e.g. nested inside another call's argument list) is refused
        // here rather than silently losing everything past the first
        // eightbyte.
        let ret_class = struct_layout(ret_ty, &self.layouts).map(|l| l.class());
        if matches!(ret_class, Some(AggClass::Two) | Some(AggClass::Mem)) && ret_dst.is_none() {
            return Err(unsupported(
                "a struct-returning call of more than eight bytes used as a plain value (only a \
                 `let`, an assignment, a `return`, or a discarded statement may hold one)",
            ));
        }

        // SysV numbers the integer and vector argument registers
        // independently, so `f(1, 2.0, 3)` passes `2.0` in `xmm0` — the
        // classification below counts each argument's own class, not its
        // position in the list. A `Mem`-class return reserves `rdi` for the
        // hidden pointer before any real argument is numbered, exactly the
        // way a `Mem`-class *parameter* shifts `Gen::function`'s own
        // numbering.
        enum ArgSlot {
            Int(usize),
            Float(usize),
            /// A `Two`-class by-value struct argument — both eightbytes'
            /// `ARG64` indices.
            IntPair(usize, usize),
            /// MK7 §5 item 4, fourth slice: a `Mem`-class by-value struct
            /// argument — its byte offset in the outgoing argument area.
            /// Nothing is popped for this slot: the value is placed in the
            /// area directly, after every register argument has already been
            /// loaded (see below), and stays there across the call.
            Stack(i32),
        }
        // The stack half of the convention, decided before anything is
        // evaluated so both ends agree — `memory_arg_area` is the same
        // function the callee's prologue uses.
        let (mem_args, area_size) =
            memory_arg_area(args.iter().map(|a| &a.ty), &self.layouts);
        let mut slots: Vec<ArgSlot> = Vec::with_capacity(args.len());
        let mut int_idx = if ret_class == Some(AggClass::Mem) { 1 } else { 0 };
        let mut float_idx = 0usize;
        for (pos, arg) in args.iter().enumerate() {
            if float_ty(&arg.ty).is_some() {
                if float_idx >= FARG.len() {
                    return Err(unsupported(&format!(
                        "a call to `{callee}` with more than eight float arguments"
                    )));
                }
                slots.push(ArgSlot::Float(float_idx));
                float_idx += 1;
                continue;
            }
            let need = int_regs_for(&arg.ty, &self.layouts).ok_or_else(|| {
                unsupported(&format!(
                    "the type `{}` as an argument to `{callee}`",
                    arg.ty.display()
                ))
            })?;
            if int_idx + need > ARG64.len() {
                return Err(unsupported(&format!(
                    "a call to `{callee}` with more than six integer argument registers"
                )));
            }
            match need {
                2 => slots.push(ArgSlot::IntPair(int_idx, int_idx + 1)),
                1 => slots.push(ArgSlot::Int(int_idx)),
                // `need == 0`: a `MEMORY`-class value, which takes no integer
                // register, so `int_idx` deliberately does not advance — an
                // argument after it keeps the register it would have had
                // anyway.
                _ => {
                    let (_, off, _) = *mem_args
                        .iter()
                        .find(|(i, _, _)| *i == pos)
                        .expect("memory_arg_area covers every Mem-class argument");
                    slots.push(ArgSlot::Stack(off));
                }
            }
            int_idx += need;
        }
        // Every argument is evaluated and pushed — one 8-byte stack slot for
        // a scalar/float/`One`-class value, two consecutive ones (built
        // through `frame.agg_slot`, since a `Two`-class value cannot fit in
        // `rax` alone) for a `Two`-class argument — then popped into its
        // register(s) in reverse. Going through the stack rather than
        // straight into the argument registers is what makes a nested call
        // safe: an inner call cannot overwrite a register the outer one has
        // not loaded yet, and its own pushes are popped before it returns. A
        // `Mem`-class argument is the exception to "popped in reverse": it is
        // evaluated (in declaration order, like every other argument) into a
        // slot of its own, and only *copied* into the outgoing area once every
        // register argument is already in its register — see below.
        let mut mem_plan: Vec<Option<(i32, i32, i32)>> = vec![None; args.len()];
        for (pos, arg) in args.iter().enumerate() {
            if float_ty(&arg.ty).is_some() {
                self.expr(arg, frame)?;
                let width = float_ty(&arg.ty).expect("checked above");
                self.out.push_str("    sub rsp, 8\n");
                let _ = writeln!(self.out, "    {} [rsp], xmm0", width.mov());
            } else if int_regs_for(&arg.ty, &self.layouts) == Some(2) {
                let agg = frame.agg_slot;
                self.eval_struct_into(arg, frame, Dest::Direct(agg))?;
                let _ = writeln!(self.out, "    push QWORD PTR [rbp{agg}]");
                let agg2 = agg + 8;
                let _ = writeln!(self.out, "    push QWORD PTR [rbp{agg2}]");
            } else if int_regs_for(&arg.ty, &self.layouts) == Some(0) {
                let size = struct_layout(&arg.ty, &self.layouts)
                    .expect("a zero-register argument is a Mem-class struct")
                    .size;
                let ArgSlot::Stack(off) = slots[pos] else {
                    unreachable!("the classification above put this argument on the stack")
                };
                let staged = frame.scratch(align_up(size, 8));
                self.eval_struct_into(arg, frame, Dest::Direct(staged))?;
                mem_plan[pos] = Some((staged, off, size));
            } else {
                self.expr(arg, frame)?;
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
                ArgSlot::IntPair(i0, i1) => {
                    // Pushed as (eightbyte 0, eightbyte 1) in that order, so
                    // eightbyte 1 is on top and comes off first.
                    let _ = writeln!(self.out, "    pop {}", ARG64[*i1]);
                    let _ = writeln!(self.out, "    pop {}", ARG64[*i0]);
                }
                // Already in the outgoing area — nothing was pushed for it.
                ArgSlot::Stack(_) => {}
            }
        }
        // The hidden return pointer, set last (after every real argument is
        // already in its own register) so nothing above can clobber `rdi`
        // before the call — `int_idx` starting at 1 for a `Mem`-class return
        // already guarantees no *real* argument was assigned `ARG64[0]`, but
        // setting it last keeps that invariant visibly true here too rather
        // than relying on the reader to check the numbering above. The
        // outgoing argument area is not part of that: it is memory, and every
        // copy into it is done by then.
        if ret_class == Some(AggClass::Mem) {
            match ret_dst.expect("checked above") {
                Dest::Direct(off) => {
                    let _ = writeln!(self.out, "    lea rdi, [rbp{off}]");
                }
                Dest::Indirect(slot) => {
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot}]");
                }
            }
        }
        let symbol = Gen::symbol(callee);
        call_align_begin(&mut self.out, frame.sp_slot);
        // MK7 §5 item 4, fourth slice: the outgoing argument area, laid out
        // *after* `call_align_begin` has rounded `rsp` down to sixteen bytes,
        // so `[rsp]` is the area's first byte and `rsp` is still aligned at
        // the `call`. `area_size` is already a multiple of sixteen, which is
        // what keeps it that way; the padding it can contain sits above the
        // last argument, where SysV expects it. Each value is copied out of the
        // slot it was staged in during evaluation, in declaration order, and
        // `emit_copy` touches only `rax` — so nothing already loaded into an
        // argument register, and not the hidden return pointer either, is
        // disturbed by this.
        if area_size > 0 {
            let _ = writeln!(self.out, "    sub rsp, {area_size}");
            for (staged, off, size) in mem_plan.iter().flatten() {
                emit_copy(&mut self.out, "rsp", *off, "rbp", *staged, *size);
            }
        }
        let _ = writeln!(self.out, "    call {symbol}");
        call_align_end(&mut self.out, frame.sp_slot);
        if ret_class == Some(AggClass::Two) {
            match ret_dst.expect("checked above") {
                Dest::Direct(off) => {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{off}], rax");
                    let off2 = off + 8;
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{off2}], rdx");
                }
                Dest::Indirect(slot) => {
                    let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                    self.out.push_str("    mov QWORD PTR [r11], rax\n");
                    self.out.push_str("    mov QWORD PTR [r11+8], rdx\n");
                }
            }
        }
        // MK7 §5 item 4, fourth slice: a `One`-class aggregate return needs
        // its `Dest` honoured too, now that a call can *be* a borrowed
        // temporary's initializer (`sum(make(7, 8))`) and so reach
        // `eval_struct_into` with a destination. The value is one opaque
        // eightbyte, exactly as `struct_lit` treats it, and every slot it can
        // be written to is `align_up(size, 8)` bytes, so an eight-byte store
        // is always contained in the slot it belongs to. Before this arm the
        // destination was silently left untouched — the third slice's
        // `Dest`-aware handling covered only the two wider classes, and no
        // fixture had ever asked for a `One`-class call result to be written
        // anywhere but `rax`.
        if ret_class == Some(AggClass::One) {
            if let Some(dest) = ret_dst {
                match dest {
                    Dest::Direct(off) => {
                        let _ = writeln!(self.out, "    mov QWORD PTR [rbp{off}], rax");
                    }
                    Dest::Indirect(slot) => {
                        let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                        self.out.push_str("    mov QWORD PTR [r11], rax\n");
                    }
                }
            }
        }
        // `AggClass::Mem`: the callee already wrote the result through the
        // pointer this call site passed in `rdi` — there is nothing further
        // to copy. `AggClass::One`/no aggregate at all: the result is
        // already exactly where every other caller of `call` expects it
        // (`rax`, or `xmm0` for `print`/a template, which never reach this
        // function at all).
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
        call_align_begin(&mut self.out, frame.sp_slot);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out, frame.sp_slot);
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
        call_align_begin(&mut self.out, frame.sp_slot);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out, frame.sp_slot);
        Ok(())
    }
}

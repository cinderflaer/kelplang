//! Lowers the typed IR (`tir`) to x86-64 assembly text (MK7 tier 1, in
//! progress). `kelpc build --backend=asm` writes this module's output to a `.s`
//! file and finishes it with `as` and `ld`; no C compiler is in that chain.
//!
//! Three decisions are worth naming, because each one is a thing this module
//! grew out of and each is still load-bearing:
//!
//! 1. **A stack machine, with a register allocator on top.** Every expression
//!    evaluates into `rax`, and a binary operator spills the left operand to the
//!    stack so the right one can be computed. On top of that, a local whose live
//!    range fits is pinned to one of five callee-saved registers for the whole
//!    function (`allocate_registers`), so a read or write of it is a
//!    register-to-register `mov` with no memory traffic and no reloading across a
//!    call. `f32`/`f64` locals are deliberately excluded — xmm live-range
//!    allocation is separate work — and expression evaluation itself is
//!    untouched, which is why this is *at or below* the `-O0` column the plan
//!    gates on: correctness is checkable against the C backend before any
//!    performance claim has to be made.
//!
//! 2. **Assembly text, not machine code.** The output goes to `as` and `ld`, so
//!    instruction encoding and relocation stay somebody else's problem for now;
//!    tier 2 removes both. What that buys here is that a wrong instruction is a
//!    readable diff rather than a byte offset.
//!
//! 3. **A refusal is a feature.** Anything this slice cannot lower — a `float`
//!    payload in an aggregate, a closure, a capability-gated call, a `List`
//!    method whose element would need a comparison or a clone, an owning
//!    `pd` — is reported as
//!    `unsupported` with the type, method or node named, rather than emitted
//!    wrongly. A capability-gated call is refused rather than emitted unchecked,
//!    because that is the one gap that would be a security hole instead of a
//!    wrong number. The fixture suite then measures exactly how much of the
//!    language is covered, and that number rises by removing named gaps instead
//!    of by hunting silent miscompiles. The C backend is the oracle: every
//!    fixture is built by both and their stdout is compared
//!    (`kelpc/tests/backend_coverage.rs`), and the current ranking of what is
//!    still refused is in `plan/mk7-own-backend.md` §5.
//!
//! Covered: every integer width, `bool`, `char`, `f32`/`f64` arithmetic and
//! comparisons (including the unordered/NaN case), all unary and binary
//! operators including short-circuiting `&&`/`||`, `if`/`else` in statement and
//! value position, `while`, assignment, a borrow of a named scalar local and
//! the read and store through it (`*p`/`*p = v` for an integer or float
//! pointee, which is the caller's half and the callee's half of a `mut` out
//! parameter), calls with up to six integer and eight
//! float arguments classified independently per SysV, recursion, `print`/`print!`
//! of integers, characters, booleans, `str`, floats and an owned buffer, `datac`
//! aggregates by value at any size (including a `MEMORY`-class one as an
//! argument), `enum` — the tagged union, its four node kinds, `?`, and a
//! generated drop that switches on the runtime tag — `Bytes`, `String`, `View`,
//! `Box<T>`, `List<T>` and `Map<K, V>`
//! with their own free at every drop point, a `pd`'s generated storage, and
//! calls to the
//! `extern fun`s whose bodies this backend owns or can forward to libc through
//! the PLT. Printing through `fmt`'s `Display` machinery is not covered, and
//! neither is anything in the refusal list above.
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
//!
//! The epilogue rebuilds `rsp` from `rbp` rather than adding the frame size back,
//! because a `jmp` to the return label does not always arrive from a statement
//! boundary: `?`'s failure arm returns from the middle of an expression, with a
//! pending operand still pushed. See `Gen::function`'s epilogue.

use crate::ast::{BinOp, UnOp};
use crate::borrowck::{DropPlan, DropTarget};
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

/// The full range of an integer type as `(min, max)` — `min` signed so a
/// signed type can express its own floor, `max` unsigned so `u64`/`usize` can
/// express theirs. `codegen_c.rs::int_bounds`'s table, repeated here because
/// the two backends have to agree on exactly which conversions can lose a
/// value: the C backend emits a conversion as a bare cast when
/// `s_min >= t_min && s_max <= t_max`, and that has to be a bare move here.
fn int_bounds(ty: &Ty) -> Option<(i128, u128)> {
    Some(match ty {
        Ty::I32 => (i32::MIN as i128, i32::MAX as u128),
        Ty::I64 | Ty::Isize => (i64::MIN as i128, i64::MAX as u128),
        Ty::U8 => (0, u8::MAX as u128),
        Ty::U16 => (0, u16::MAX as u128),
        Ty::U32 => (0, u32::MAX as u128),
        Ty::U64 | Ty::Usize => (0, u64::MAX as u128),
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
///
/// MK7 §5 item 8's remaining gaps are `str` and the floats: the C backend
/// prints both through one specifier each (`printf_piece_with`), and so does
/// this — `%f` takes its value in the `xmm` file rather than an integer
/// register, which is the one thing a call site has to get right beyond the
/// specifier itself (see `Gen::builtin_print` and `Gen::template`).
fn printable_ty(ty: &Ty) -> Option<&'static str> {
    if str_ty(ty) {
        return Some("%s");
    }
    Some(match ty {
        Ty::I32 => "%d",
        Ty::I64 | Ty::Isize => "%lld",
        Ty::U32 => "%u",
        Ty::U64 => "%llu",
        Ty::U8 | Ty::U16 => "%u",
        Ty::Usize => "%zu",
        Ty::Char => "%c",
        Ty::Bool => "%s",
        Ty::F32 | Ty::F64 => "%f",
        _ => return None,
    })
}

/// `str` and `&str` — the same value at the ABI level (`const char*`: a
/// reference to an already-borrowed view is not a pointer to a pointer, which
/// is also `types.rs`'s `c_type` rule), so everything that moves, passes,
/// returns or prints one treats both spellings identically.
fn str_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Str)
        || matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str))
}

/// `Bytes` and `&Bytes`/`&mut Bytes` — the owned heap buffer and a borrow of
/// it, which is the shape every `Bytes#*` method takes as its receiver and
/// `print`/`print!` accept. MK7 §5's first heap slice: a `Bytes` value is a
/// 24-byte `{data, len, cap}` aggregate (`struct_layout` synthesizes its
/// layout), so an *owned* one travels through the same `MEMORY`-class paths a
/// three-field `datac` does, while a borrow is a plain pointer.
fn bytes_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Bytes),
        _ => false,
    }
}

/// `String` and `&String`/`&mut String` — the owned text buffer and a borrow
/// of it, the `String#*` methods' receiver and one of the two shapes
/// `print`/`print!` accept (MK7 §5's second heap slice). `kelp_String` is the
/// same `{data, len, cap}` aggregate `kelp_Bytes` is, so every site that
/// handles an owned `Bytes` handles an owned `String` too — the only thing
/// this predicate adds is where the *drop* and the method table differ.
fn string_ty(ty: &Ty) -> bool {
    match ty {
        Ty::String => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::String),
        _ => false,
    }
}

/// `View` — the two-eightbyte `{ptr, len}` borrowed text view (MK7 §5's
/// second heap slice). It is `Copy`, so there is no `&View` shape to look
/// through: a view parameter and a view argument are the value itself.
fn view_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::View)
}

/// `codegen_c.rs::is_place`'s predicate: an expression that names storage an
/// address can be taken of. Used to tell `&b` (a borrow of a local, no copy)
/// from `&Bytes.from_str(..)` (a borrow of a temporary, which needs a slot of
/// its own) — see `Gen::hoist_bytes_temporary`.
fn is_place(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Ident(_) | ExprKind::FieldAccess { .. } | ExprKind::Deref { .. }
    )
}

/// The user type a value names, looking through a borrow — `codegen_c.rs`'s own
/// helper of the same name, needed here because an `EnumTag`/`EnumField` node
/// carries only the two variant/field indices and the base's type is what names
/// the `enum` a refusal should mention.
fn enum_name_of(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => name.clone(),
        Ty::Ref { inner, .. } => enum_name_of(inner),
        other => other.display(),
    }
}

/// The key `struct_layout` looks `Ty::Bytes` up under in `Gen::layouts`.
/// `$` is deliberate: no Kelp identifier can contain one, so a user `datac`
/// can never collide with it — the same reason `Routine::label` uses one.
const BYTES_LAYOUT_KEY: &str = "$Bytes";

/// The C layout of `kelp_Bytes` in `codegen_c.rs`'s RUNTIME: `data` at 0,
/// `len` at 8, `cap` at 16, size 24, align 8. Synthesized rather than parsed,
/// because the two backends agreeing on this struct's *offsets* is the same
/// promise the fixture suite makes for every `datac` typedef.
fn bytes_layout() -> StructLayout {
    scalar_layout(
        vec![
            FieldEntry::Scalar("data".to_string(), 0, Width::W8, false),
            FieldEntry::Scalar("len".to_string(), 8, Width::W8, false),
            FieldEntry::Scalar("cap".to_string(), 16, Width::W8, false),
        ],
        24,
        8,
    )
}

/// The key `struct_layout` looks `Ty::String` up under — `$Bytes`'s sibling,
/// for the same reason (no Kelp identifier can contain a `$`).
const STRING_LAYOUT_KEY: &str = "$String";

/// The key `struct_layout` looks `Ty::View` up under.
const VIEW_LAYOUT_KEY: &str = "$View";

/// The key `struct_layout` looks every `Ty::Box<T>` up under. **One key for
/// every element type**, unlike the routines: a box is one pointer whatever it
/// points at, so `types::box_c_type`'s per-instantiation name is needed for the
/// *functions* (`kelp_Box__<mangle>__of`) and not for the layout. Keeping one
/// key is what lets `layout_key` answer for a `Box` field without knowing
/// anything about the element, and it is why a recursive `datac`
/// (`datac Node(val next: Box<Node>)`) resolves at all: the box's own layout is
/// available before `Node`'s is.
const BOX_LAYOUT_KEY: &str = "$Box";

/// The C layout of `kelp_Box__*` (`codegen_c.rs:1106-1111`): one `value`
/// pointer at offset 0, size 8, align 8 — one INTEGER eightbyte, so a box
/// travels in `rax`/`rdi` like a scalar and needs no new ABI code anywhere,
/// exactly as the third and fourth item-4 slices promised a heap type would.
/// `NULL` means empty, which is the whole of the type's representation.
fn box_layout() -> StructLayout {
    scalar_layout(
        vec![FieldEntry::Scalar("value".to_string(), 0, Width::W8, false)],
        8,
        8,
    )
}

/// The key `struct_layout` looks every `Ty::List<T>` up under. **One key for
/// every element type**, `BOX_LAYOUT_KEY`'s sibling and for the same reason:
/// `kelp_List__<mangle>` is 24 bytes whatever it holds (only the *pointee*
/// type of `data` changes), so `types::list_c_type`'s per-instantiation name is
/// needed for the *functions* and not for the layout. Keeping one key is what
/// lets a `List` field be placed inside a `datac` without the element's own
/// layout being available — and the element's layout is deliberately not
/// required there, exactly as a `Box` field's element is not: `List<Shape>`
/// over an `enum` is a 24-byte list the ABI handles fine, and what a program
/// cannot do with it (push, get, drop) is refused where it is done, by name.
const LIST_LAYOUT_KEY: &str = "$List";

/// The C layout of `kelp_List__*` (`codegen_c.rs:1096-1101`): `data` at 0,
/// `len` at 8, `cap` at 16, size 24, align 8 — byte for byte the `Bytes`/
/// `String` layout, because the C typedef is the same three fields with a
/// different pointee. Three INTEGER eightbytes, so **MEMORY** class: a `List`
/// local, parameter, argument and return travel exactly the way a `Bytes` one
/// already does, and the fourth item-4 slice's `MEMORY`-class argument area is
/// what passes one by value. `cap == 0` implies `data == NULL`; `len <= cap`;
/// the buffer is `cap * sizeof(elem)` bytes, and it is the list's own to free.
fn list_layout() -> StructLayout {
    scalar_layout(
        vec![
            FieldEntry::Scalar("data".to_string(), 0, Width::W8, false),
            FieldEntry::Scalar("len".to_string(), 8, Width::W8, false),
            FieldEntry::Scalar("cap".to_string(), 16, Width::W8, false),
        ],
        24,
        8,
    )
}

/// The key `struct_layout` looks every `Ty::Map<K, V>` up under. **One key for
/// every key and value type**, `LIST_LAYOUT_KEY`'s sibling and for the same
/// reason: the map *header* is `{entries, entry_count, entry_cap, len, index,
/// index_cap, index_used}` — seven words whatever it holds — so
/// `types::map_c_type`'s per-instantiation name is needed for the *functions*
/// and not for the layout. The **entry** does depend on the key type, but an
/// entry is only ever an array element (see `map_entry`), never a by-value
/// argument, so only its size is needed and this one key is enough.
const MAP_LAYOUT_KEY: &str = "$Map";

/// The C layout of `kelp_Map__K__V` (`codegen_c.rs:1121-1134`): seven eight-byte
/// fields at 0/8/16/24/32/40/48 — `entries`, `entry_count`, `entry_cap`, `len`,
/// `index`, `index_cap`, `index_used` — size 56, align 8. Seven INTEGER
/// eightbytes, so **MEMORY** class: a `Map` local, parameter, argument and
/// return travel exactly the way a `Bytes`/`List` one already does, and the
/// fourth item-4 slice's `MEMORY`-class argument area is what passes one by
/// value. `entries` and `index` are both nullable; `len == 0` does not imply
/// either is. The header does **not** depend on `K`/`V` — that is the whole
/// reason one key serves every instantiation.
fn map_layout() -> StructLayout {
    scalar_layout(
        vec![
            FieldEntry::Scalar("entries".to_string(), 0, Width::W8, false),
            FieldEntry::Scalar("entry_count".to_string(), 8, Width::W8, false),
            FieldEntry::Scalar("entry_cap".to_string(), 16, Width::W8, false),
            FieldEntry::Scalar("len".to_string(), 24, Width::W8, false),
            FieldEntry::Scalar("index".to_string(), 32, Width::W8, false),
            FieldEntry::Scalar("index_cap".to_string(), 40, Width::W8, false),
            FieldEntry::Scalar("index_used".to_string(), 48, Width::W8, false),
        ],
        56,
        8,
    )
}

/// One `kelp_Map__K__V_entry`'s layout, which **does** depend on the key type:
/// `{K key; V* value; uint8_t live;}` (`codegen_c.rs:1121`), laid out by the
/// same sequential, naturally-aligned rule clang applies to that struct. This
/// is the arithmetic `write_map`'s generated code counts on for
/// `realloc(entries, cap * sizeof(entry))` and for every `entries[e]` access,
/// and it is the whole reason the per-instantiation routines exist.
///
/// Only the *key* matters: `value` is always a pointer and `live` always a
/// byte, so a `String` key gives value at 24, live at 32 and size 40, while a
/// scalar key gives value at 8, live at 16 and size 24 — the two points the
/// plan's annex verifies. An entry is never a by-value argument, so this is
/// the only place the layout is needed.
struct MapEntry {
    value_offset: i32,
    live_offset: i32,
    size: i32,
}

fn map_entry(key: &Ty, layouts: &std::collections::HashMap<String, StructLayout>) -> Option<MapEntry> {
    let key_size = elem_size(key, layouts)?;
    let value_offset = align_up(key_size, 8);
    let live_offset = value_offset + 8;
    Some(MapEntry { value_offset, live_offset, size: align_up(live_offset + 1, 8) })
}

/// The C layout of `kelp_String` (`codegen_c.rs:316-321`): the same
/// `{data, len, cap}` the `Bytes` layout is, so a `String` local, parameter,
/// argument and return travel exactly the way a `Bytes` one does.
fn string_layout() -> StructLayout {
    scalar_layout(
        vec![
            FieldEntry::Scalar("data".to_string(), 0, Width::W8, false),
            FieldEntry::Scalar("len".to_string(), 8, Width::W8, false),
            FieldEntry::Scalar("cap".to_string(), 16, Width::W8, false),
        ],
        24,
        8,
    )
}

/// The C layout of `kelp_View` (`codegen_c.rs:115-118`): `ptr` at 0, `len` at
/// 8, size 16, align 8. Two INTEGER eightbytes, so this is the first heap type
/// whose `AggClass` is `Two` — it travels in a register pair (`rdi`/`rsi` in,
/// `rax`/`rdx` out) rather than through the hidden pointer, which is the path
/// item 4's third slice built and no heap type had exercised until now.
fn view_layout() -> StructLayout {
    scalar_layout(
        vec![
            FieldEntry::Scalar("ptr".to_string(), 0, Width::W8, false),
            FieldEntry::Scalar("len".to_string(), 8, Width::W8, false),
        ],
        16,
        8,
    )
}

/// MK7 §5: one passdown's generated storage — the internal `pd#<qualified>`
/// name (which is also the name of the hidden parameter a referencing function
/// receives), the symbol that storage is emitted under, and the passdown's own
/// type. What the name *means* depends on where it resolves, and that is the
/// whole of this slice: inside a function it resolves to the parameter, whose
/// value is already the borrow; in `main` it resolves here, to the object
/// itself.
struct PdGlobal {
    hidden: String,
    label: String,
    value_ty: Ty,
}

impl PdGlobal {
    /// Whether a read of this passdown's name is the storage's *value* rather
    /// than its *address*. True for a `str`, and only for a `str`: `&str` and
    /// `str` are the same `const char*` (`types.rs`'s rule), so the global
    /// holds the pointer and a borrow of it is that pointer, while every other
    /// passdown is reached through its address — which is exactly the two
    /// shapes `infer::PdInfo::arg` emits.
    fn str_borrow(&self) -> bool {
        str_ty(&self.value_ty)
    }
}

/// How many bytes one passdown's storage needs, or `None` for a type this
/// backend has no storage for at all. Every type it accepts is rounded up to
/// eight bytes, the way a local's slot is: a passdown's storage is addressed
/// by `lea` and copied whole, so there is no reason for it to be tighter, and
/// a multiple of eight keeps every access naturally aligned.
fn pd_storage_size(
    ty: &Ty,
    layouts: &std::collections::HashMap<String, StructLayout>,
) -> Option<i32> {
    if let Some(layout) = struct_layout(ty, layouts) {
        return Some(align_up(layout.size, 8));
    }
    if str_ty(ty) {
        return Some(8);
    }
    int_ty(ty).map(|(width, _)| align_up(width.bytes(), 8))
}

/// Every uncovered construct reports through this, so the message shape — and
/// therefore what a reader greps for — is uniform.
fn unsupported(what: &str) -> String {
    format!("the 'asm' backend does not support {what} yet")
}

/// How this backend lowers one `extern fun` (MK7 §5, the first slice of item 1
/// — the runtime starts arriving).
///
/// The point of this table is that an `extern fun` is no longer a *program*
/// refusal: a program that never calls one compiles, and a program that calls
/// one compiles when this table can supply the body. That matters beyond the
/// handful of fixtures it unblocks today: the blanket refusal was masking every
/// other gap behind it (a program that imports a gated module or `std/str`
/// declares ninety externs whether or not it calls them, so the first refusal
/// every one of those programs hit named an `extern fun` rather than the heap
/// type or the `enum` that was actually in the way).
///
/// The call site needs no per-extern knowledge at all, which is why this is a
/// symbol and not a code path: the argument marshalling in `Gen::call` is
/// driven by the *declared* signature, and `externs.rs` is what keeps a
/// compiler-supplied body honest about that signature. So an extern is either a
/// libc function with the same signature, or one of this backend's own
/// routines with the same signature.
#[derive(Clone, Copy)]
enum Extern {
    /// A C library function whose signature is exactly the extern's own. The
    /// call goes through the PLT, which is how the panic routine already
    /// reaches `fprintf`/`exit`.
    Libc(&'static str),
    /// One of this backend's own hand-written routines — for the externs whose
    /// C body does more than call one library function (a bounds check, a
    /// libm *macro* like `isnan`, a syscall pair, the `Bytes` allocator).
    Routine(Routine),
}

/// One routine this backend owns, emitted once per program that needs it
/// (`Gen::emit_routine`). Named by an enum rather than a string so the set of
/// them is closed and a typo is a compile error rather than a missing symbol.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Routine {
    StrByteAtRaw,
    StrSliceFromRaw,
    /// MK7 §5's next slice: the three `str` parsers, transliterated from their
    /// `externs.rs` bodies. Each is `strtoll`/`strtoull`/`strtod` through the
    /// PLT behind the same empty-or-whitespace check and the same whole-string
    /// check, with the one store through the caller's `mut` out-parameter that
    /// the scalar-borrow slice made expressible. `parse_u64_raw` is the one
    /// that also rejects a leading `-`, and `parse_i64_raw`/`parse_u64_raw` are
    /// the two that read `errno` — `strtoll`'s overflow signal, which is what
    /// makes an out-of-range literal a failure rather than a saturated
    /// success.
    StrParseI64Raw,
    StrParseU64Raw,
    StrParseF64Raw,
    MathIsNanRaw,
    MathIsFiniteRaw,
    /// MK7 §5's first heap slice: the `Bytes` runtime this backend owns — the
    /// C bodies of `codegen_c.rs`'s RUNTIME, transliterated instruction by
    /// instruction. `BufReserve` and `BytesDrop` are the two internal helpers
    /// the method routines call (`kelp_buf_reserve`, `kelp_bytes_drop`);
    /// everything else is one `Bytes` method or the allocation primitive
    /// `std/mem` wraps.
    BufReserve,
    BytesDrop,
    BytesAllocRaw,
    BytesEmpty,
    BytesFromStr,
    BytesLen,
    BytesIsEmpty,
    BytesGet,
    BytesSet,
    BytesClone,
    BytesPush,
    BytesAppend,
    BytesClear,
    /// MK7 §5's second heap slice: `Bytes#as_str` — a bounds-checked borrow of
    /// the buffer as text, which needs `Utf8Valid` and was the one `Bytes#*`
    /// method the first slice left unmapped.
    BytesAsStr,
    /// The shared UTF-8 well-formedness check `String#from_bytes` and
    /// `Bytes#as_str` both use — `codegen_c.rs:186-208`, transliterated.
    Utf8Valid,
    /// The owned `String` runtime: `codegen_c.rs`'s sixteen `kelp_String__*`
    /// functions plus `kelp_string_drop`. Same `{data, len, cap}` shape as
    /// `Bytes`, so the call/return ABI is identical (`String#reserve` is
    /// `BufReserve` directly, exactly as `Bytes#reserve` is).
    StringEmpty,
    StringFromStr,
    StringFromBytes,
    StringIntoBytes,
    StringLen,
    StringIsEmpty,
    StringCharCount,
    StringClone,
    StringPush,
    StringPushStr,
    StringWithCapacity,
    StringClear,
    StringAsStr,
    StringView,
    StringDrop,
    /// The borrowed `View` runtime: `codegen_c.rs`'s seven `kelp_View__*`/
    /// `kelp_str__view` functions. A `View` borrows, so it has no drop — this
    /// is the one heap type whose runtime is all read-only.
    ViewLen,
    ViewIsEmpty,
    ViewGet,
    ViewSlice,
    ViewEq,
    ViewToString,
    StrView,
    /// MK7 §5: the `str`/`fmt` externs whose bodies became lowerable once
    /// `String` existed. `StrFromF64Raw` is one `snprintf` pair plus the
    /// `Bytes` allocator; `FmtHexRaw` and `FmtBytesRaw` are `snprintf`/byte
    /// shuffling plus `String.from_bytes`. The `parse_*_raw` family is left
    /// out — see `extern_target`.
    StrFromF64Raw,
    FmtHexRaw,
    FmtBytesRaw,
    /// MK7 §5's list slice: the one piece of a `List<T>`'s function set that is
    /// **not** per-instantiation. `codegen_c.rs` monomorphizes `__reserve`
    /// because the element type is baked into its C signature; here the element
    /// size is just a third argument (`rdi` = the list, `rsi` = the wanted
    /// length, `rdx` = `sizeof(elem)`), so one copy serves every instantiation —
    /// the only arithmetic that actually differs between them.
    ListReserve,
    /// MK7 §5's map slice: the three shared key helpers `codegen_c.rs`'s runtime
    /// defines once and every map instantiation calls, transliterated from
    /// `codegen_c.rs:502-542`. `MapHash` is FNV-1a over a key's bytes (a
    /// `String` key), `MapKeyEq` is the length-aware `memcmp` equality against
    /// a borrowed `str`, and `MapMix` is the scalar-key 64-bit finalizer. Like
    /// `ListReserve`, none of them is monomorphized: the key type's difference
    /// is entirely in *how the caller reads the key*, which the generated
    /// routine does — the arithmetic is the same three bodies for every key.
    MapHash,
    MapKeyEq,
    MapMix,
}

impl Routine {
    /// The symbol this routine is emitted under. `$` is deliberate: a Kelp
    /// identifier cannot contain one, `types::c_ident` never produces one, and
    /// so no key a program could write mangles into this name — which a plain
    /// `kelp_rt_str_byte_at_raw` would not have (a module `rt` holding
    /// `str_byte_at_raw` mangles to exactly that).
    fn label(self) -> &'static str {
        match self {
            Routine::StrByteAtRaw => "kelp_rt$str_byte_at_raw",
            Routine::StrSliceFromRaw => "kelp_rt$str_slice_from_raw",
            Routine::StrParseI64Raw => "kelp_rt$str_parse_i64_raw",
            Routine::StrParseU64Raw => "kelp_rt$str_parse_u64_raw",
            Routine::StrParseF64Raw => "kelp_rt$str_parse_f64_raw",
            Routine::MathIsNanRaw => "kelp_rt$math_is_nan_raw",
            Routine::MathIsFiniteRaw => "kelp_rt$math_is_finite_raw",
            Routine::BufReserve => "kelp_rt$buf_reserve",
            Routine::BytesDrop => "kelp_rt$bytes_drop",
            Routine::BytesAllocRaw => "kelp_rt$bytes_alloc_raw",
            Routine::BytesEmpty => "kelp_rt$bytes_empty",
            Routine::BytesFromStr => "kelp_rt$bytes_from_str",
            Routine::BytesLen => "kelp_rt$bytes_len",
            Routine::BytesIsEmpty => "kelp_rt$bytes_is_empty",
            Routine::BytesGet => "kelp_rt$bytes_get",
            Routine::BytesSet => "kelp_rt$bytes_set",
            Routine::BytesClone => "kelp_rt$bytes_clone",
            Routine::BytesPush => "kelp_rt$bytes_push",
            Routine::BytesAppend => "kelp_rt$bytes_append",
            Routine::BytesClear => "kelp_rt$bytes_clear",
            Routine::BytesAsStr => "kelp_rt$bytes_as_str",
            Routine::Utf8Valid => "kelp_rt$utf8_valid",
            Routine::StringEmpty => "kelp_rt$string_empty",
            Routine::StringFromStr => "kelp_rt$string_from_str",
            Routine::StringFromBytes => "kelp_rt$string_from_bytes",
            Routine::StringIntoBytes => "kelp_rt$string_into_bytes",
            Routine::StringLen => "kelp_rt$string_len",
            Routine::StringIsEmpty => "kelp_rt$string_is_empty",
            Routine::StringCharCount => "kelp_rt$string_char_count",
            Routine::StringClone => "kelp_rt$string_clone",
            Routine::StringPush => "kelp_rt$string_push",
            Routine::StringPushStr => "kelp_rt$string_push_str",
            Routine::StringWithCapacity => "kelp_rt$string_with_capacity",
            Routine::StringClear => "kelp_rt$string_clear",
            Routine::StringAsStr => "kelp_rt$string_as_str",
            Routine::StringView => "kelp_rt$string_view",
            Routine::StringDrop => "kelp_rt$string_drop",
            Routine::ViewLen => "kelp_rt$view_len",
            Routine::ViewIsEmpty => "kelp_rt$view_is_empty",
            Routine::ViewGet => "kelp_rt$view_get",
            Routine::ViewSlice => "kelp_rt$view_slice",
            Routine::ViewEq => "kelp_rt$view_eq",
            Routine::ViewToString => "kelp_rt$view_to_string",
            Routine::StrView => "kelp_rt$str_view",
            Routine::StrFromF64Raw => "kelp_rt$str_from_f64_raw",
            Routine::FmtHexRaw => "kelp_rt$fmt_hex_raw",
            Routine::FmtBytesRaw => "kelp_rt$fmt_bytes_raw",
            Routine::ListReserve => "kelp_rt$list_reserve",
            Routine::MapHash => "kelp_rt$map_hash",
            Routine::MapKeyEq => "kelp_rt$map_key_eq",
            Routine::MapMix => "kelp_rt$map_mix",
        }
    }
}

/// The routine a `Bytes#*` builtin method lowers to, or `None` for one this
/// slice does not implement — which becomes a refusal at the call site rather
/// than an undefined symbol at link time. `Bytes#reserve` and
/// `Bytes#push`/`Bytes#append` share `kelp_buf_reserve`'s ABI (`self` in
/// `rdi`, `extra` in `rsi`), so `reserve` is that routine directly.
fn bytes_method(callee: &str) -> Option<Routine> {
    Some(match callee {
        "Bytes#empty" => Routine::BytesEmpty,
        "Bytes#from_str" => Routine::BytesFromStr,
        "Bytes#len" => Routine::BytesLen,
        "Bytes#is_empty" => Routine::BytesIsEmpty,
        "Bytes#get" => Routine::BytesGet,
        "Bytes#set" => Routine::BytesSet,
        "Bytes#clone" => Routine::BytesClone,
        "Bytes#push" => Routine::BytesPush,
        "Bytes#append" => Routine::BytesAppend,
        "Bytes#reserve" => Routine::BufReserve,
        "Bytes#clear" => Routine::BytesClear,
        "Bytes#as_str" => Routine::BytesAsStr,
        _ => return None,
    })
}

/// The routine a `String#*` builtin method lowers to (MK7 §5's second heap
/// slice). Same discipline as `bytes_method`: `String#reserve` is
/// `BufReserve` directly (identical `(String*, size_t)` ABI), and an unmapped
/// method is refused by name at the call site.
fn string_method(callee: &str) -> Option<Routine> {
    Some(match callee {
        "String#empty" => Routine::StringEmpty,
        "String#from_str" => Routine::StringFromStr,
        "String#from_bytes" => Routine::StringFromBytes,
        "String#into_bytes" => Routine::StringIntoBytes,
        "String#len" => Routine::StringLen,
        "String#is_empty" => Routine::StringIsEmpty,
        "String#char_count" => Routine::StringCharCount,
        "String#clone" => Routine::StringClone,
        "String#push" => Routine::StringPush,
        "String#push_str" => Routine::StringPushStr,
        "String#reserve" => Routine::BufReserve,
        "String#with_capacity" => Routine::StringWithCapacity,
        "String#clear" => Routine::StringClear,
        "String#as_str" => Routine::StringAsStr,
        "String#view" => Routine::StringView,
        _ => return None,
    })
}

/// The routine a `View#*` or `str#*` builtin method lowers to. `str#view`
/// shares the table because `str.view()` is the other constructor of a
/// `View`; every `View` method takes `self` by value (a `View` is `Copy`),
/// so the receiver is already the pair of registers the routine expects.
fn view_method(callee: &str) -> Option<Routine> {
    Some(match callee {
        "str#view" => Routine::StrView,
        "View#len" => Routine::ViewLen,
        "View#is_empty" => Routine::ViewIsEmpty,
        "View#get" => Routine::ViewGet,
        "View#slice" => Routine::ViewSlice,
        "View#eq" => Routine::ViewEq,
        "View#to_string" => Routine::ViewToString,
        _ => return None,
    })
}

/// The lowering for the extern with canonical key `key` (`std.math.sqrt_raw`),
/// or `None` for one this backend cannot supply a body for yet — which becomes
/// a refusal at the *call site* rather than at the declaration.
fn extern_target(key: &str) -> Option<Extern> {
    Some(match key {
        // `std/str`'s three byte primitives: `strlen` is exactly the first
        // one, and the other two wrap it in the bounds check the C body has.
        "std.str.byte_len_raw" => Extern::Libc("strlen"),
        "std.str.byte_at_raw" => Extern::Routine(Routine::StrByteAtRaw),
        "std.str.slice_from_raw" => Extern::Routine(Routine::StrSliceFromRaw),
        // `std/math`: one libm call each, all `double` in and `double` out. The
        // names differ (`ln_raw` is C's `log`), the signatures do not, and
        // calling the same libm the C backend calls is what makes the two
        // backends agree bit for bit rather than approximately.
        "std.math.sqrt_raw" => Extern::Libc("sqrt"),
        "std.math.pow_raw" => Extern::Libc("pow"),
        "std.math.floor_raw" => Extern::Libc("floor"),
        "std.math.ceil_raw" => Extern::Libc("ceil"),
        "std.math.round_raw" => Extern::Libc("round"),
        "std.math.trunc_raw" => Extern::Libc("trunc"),
        "std.math.exp_raw" => Extern::Libc("exp"),
        "std.math.ln_raw" => Extern::Libc("log"),
        "std.math.log2_raw" => Extern::Libc("log2"),
        "std.math.log10_raw" => Extern::Libc("log10"),
        "std.math.sin_raw" => Extern::Libc("sin"),
        "std.math.cos_raw" => Extern::Libc("cos"),
        "std.math.tan_raw" => Extern::Libc("tan"),
        "std.math.atan2_raw" => Extern::Libc("atan2"),
        // `isnan`/`isfinite` are C *macros*, not functions: there is no
        // `isnan` symbol to call, and glibc's `__isnan`/`__finite` are private.
        // Both are pure bit tests on the operand, so they are ours.
        "std.math.is_nan_raw" => Extern::Routine(Routine::MathIsNanRaw),
        "std.math.is_finite_raw" => Extern::Routine(Routine::MathIsFiniteRaw),
        // MK7 §5: `std.mem.alloc_raw` is the plan's named
        // "hand-written assembly routine for allocation, PLT-called" — one
        // `malloc` through the PLT plus the zero-fill and NUL-termination
        // contract `mem.kp` documents.
        "std.mem.alloc_raw" => Extern::Routine(Routine::BytesAllocRaw),
        // The `str`/`fmt` externs that became lowerable once `String` existed
        // (MK7 §5's second heap slice). Each is a transliteration of its
        // `externs.rs` body.
        "std.str.from_f64_raw" => Extern::Routine(Routine::StrFromF64Raw),
        // MK7 §5's next slice: the parsers, whose `out` is a `mut
        // i64`/`u64`/`f64` parameter. What kept them out until now was that
        // a borrow of a *scalar* local had no lowering, so `ExprKind::Ref`
        // refused `&mut v` at the call site; with that arm in place the
        // declared signature is all the call marshalling needs, exactly as
        // every other extern here.
        "std.str.parse_i64_raw" => Extern::Routine(Routine::StrParseI64Raw),
        "std.str.parse_u64_raw" => Extern::Routine(Routine::StrParseU64Raw),
        "std.str.parse_f64_raw" => Extern::Routine(Routine::StrParseF64Raw),
        "std.fmt.hex_raw" => Extern::Routine(Routine::FmtHexRaw),
        "std.fmt.bytes_raw" => Extern::Routine(Routine::FmtBytesRaw),
        _ => return None,
    })
}

/// Every function key a program can actually reach.
///
/// This is *dead-code elimination at function granularity*, and it exists
/// because a Kelp program's function list is not a program's code: importing
/// `std/str` brings in all 40-odd of its functions, and each one this backend
/// cannot lower (a `String` parameter, a `Bytes` return) used to refuse the
/// whole program even when nothing called it. The C backend emits every one of
/// them and lets `cc` discard the unused ones, so refusing them here was
/// refusing programs the oracle builds.
///
/// It is safe by construction: an unreachable function cannot run, cannot
/// write to stdout and cannot drop anything, so it cannot change what a fixture
/// observes. What it *does* change is honest and worth stating plainly — an
/// uncovered shape that only appears in dead code is no longer a build error,
/// which is the same trade `generate`'s `datac` handling already makes (a
/// `datac` that does not qualify is refused where a value of it is moved, not
/// where it is declared).
///
/// The roots are `main` and every passdown initializer, because `main` is what
/// runs the `pd_init` sequence. `FnRef`/`ClosureLit` are edges like any call:
/// their keys name generated top-level functions, so a literal's body is live
/// exactly when the expression naming it is.
fn live_functions(program: &Program) -> std::collections::HashSet<String> {
    let mut found: Vec<String> = vec!["main".to_string()];
    for pd in &program.pds {
        collect_calls_expr(&pd.init, &mut found);
    }
    let mut live = std::collections::HashSet::new();
    while let Some(key) = found.pop() {
        if !live.insert(key.clone()) {
            continue;
        }
        let Some(f) = program.functions.iter().find(|f| f.name == key) else {
            continue;
        };
        collect_calls_block(&f.body, &mut found);
    }
    live
}

fn collect_calls_block(block: &Block, out: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { init, .. } => collect_calls_expr(init, out),
            Stmt::While { cond, body } => {
                collect_calls_expr(cond, out);
                collect_calls_block(body, out);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    collect_calls_expr(v, out);
                }
            }
            Stmt::Expr(e) => collect_calls_expr(e, out),
        }
    }
    if let Some(tail) = &block.tail {
        collect_calls_expr(tail, out);
    }
}

fn collect_calls_expr(e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::IntLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::Capture { .. } => {}
        ExprKind::Unary { expr, .. }
        | ExprKind::Ref { expr, .. }
        | ExprKind::Deref { expr }
        | ExprKind::EnumTag { expr, .. } => collect_calls_expr(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_calls_expr(lhs, out);
            collect_calls_expr(rhs, out);
        }
        ExprKind::Call { callee, args } => {
            out.push(callee.clone());
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        ExprKind::IndirectCall { callee, args } => {
            collect_calls_expr(callee, out);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => {
            collect_calls_expr(base, out)
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                collect_calls_expr(v, out);
            }
        }
        ExprKind::EnumLit { fields, .. } => {
            for v in fields {
                collect_calls_expr(v, out);
            }
        }
        ExprKind::Try { expr, .. } => collect_calls_expr(expr, out),
        ExprKind::Assign { target, value } => {
            collect_calls_expr(target, out);
            collect_calls_expr(value, out);
        }
        ExprKind::If { cond, then_block, else_block } => {
            collect_calls_expr(cond, out);
            collect_calls_block(then_block, out);
            if let Some(b) = else_block {
                collect_calls_block(b, out);
            }
        }
        ExprKind::Block(b) => collect_calls_block(b, out),
        ExprKind::FnRef(key) => out.push(key.clone()),
        ExprKind::ClosureLit { key, captures } => {
            out.push(key.clone());
            for c in captures {
                collect_calls_expr(c, out);
            }
        }
        ExprKind::FnToClosure { func } => collect_calls_expr(func, out),
        ExprKind::Template { pieces, .. } => {
            for p in pieces {
                if let TemplatePiece::Value(v) = p {
                    collect_calls_expr(v, out);
                }
            }
        }
    }
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
/// A `datac` that does not qualify — a float field, an `enum` or closure
/// field, or a nested `datac` that has one — simply has no entry here, and
/// every site
/// that would need one reports `unsupported` naming the type. That is
/// deliberately the same "coverage boundary" discipline the rest of this
/// module uses: a later slice's shapes are later slices, not silent
/// truncations.
#[derive(Clone)]
struct StructLayout {
    /// One entry per field, in declaration order, preserved because
    /// `struct_lit` must evaluate field-initializer expressions in the order
    /// they were written for side-effect correctness even though scalar and
    /// aggregate fields are looked up through different lists below. Empty for
    /// an `enum` (see `variants`): a tagged union has no flat field list, and
    /// every site that reads a field of one goes through `variants`.
    entries: Vec<FieldEntry>,
    /// This struct's own size and alignment, in bytes — needed when this
    /// struct is itself nested inside another one, to place it and to know
    /// how many bytes to copy out of it.
    size: i32,
    align: i32,
    /// `Some` for an `enum` (MK7 §5's enum slice), one entry per variant in
    /// declaration order; `None` for a `datac` and for the synthesized heap
    /// layouts. `size`/`align` above describe the *enum* — tag at offset 0,
    /// union at `payload_offset`, total size `codegen_c.rs::write_enum`'s
    /// `typedef` gets from clang — so every ABI and frame site that already
    /// asked `struct_layout` for a class or a size answers for an `enum`
    /// unchanged. What is enum-specific is only ever a *field*: which
    /// variant's payload is live, and where inside it a payload field sits.
    variants: Option<Vec<EnumVariantLayout>>,
    /// Every leaf `(offset, size, kind)` this layout expands to, with an
    /// aggregate field's own atoms already folded in at its offset. `class`
    /// is derived from these; a *parent* aggregate reads them to classify the
    /// eightbytes this layout lands in, which is the one thing a size cannot
    /// tell it (a mixed nested `datac` at a non-eightbyte-aligned offset
    /// splits across the parent's eightbytes differently than its own do).
    atoms: Vec<(i32, i32, Eb)>,
    /// This layout's SysV classification — see `classify`. Stored rather than
    /// recomputed because a call site consults it at every aggregate
    /// boundary and the atoms it came from are the only input.
    class: AggClass,
}

/// One variant's payload inside an `enum`'s union (MK7 §5's enum slice).
///
/// `entries` is one entry per payload field, in declaration order — the order
/// `ExprKind::EnumField`'s `field` index numbers them — and each entry's byte
/// offset is **from the enum's own base**, with the union's own offset already
/// folded in. That is deliberate: every reader of a payload field already
/// holds the enum value's address in `r11` (`addr_of_struct`), so a payload
/// field is reached by exactly the arithmetic a `datac` field is, and the one
/// extra number the union costs is added once, here, rather than at every
/// read.
#[derive(Clone)]
struct EnumVariantLayout {
    entries: Vec<FieldEntry>,
}

#[derive(Clone)]
enum FieldEntry {
    /// `(name, byte offset from the struct's base, width, signed)`. Offsets
    /// follow the same natural C alignment `codegen_c.rs`'s emitted typedef
    /// gets from clang.
    Scalar(String, i32, Width, bool),
    /// `(name, byte offset from the struct's base, layout key, field type)` —
    /// a field that is itself an aggregate. Two shapes land here. A *nested
    /// `datac`* is one, copied by `emit_copy` at its own (arbitrary) size
    /// rather than a single `mov`; and, as of MK7 §5's heap-field slice, a
    /// `String` or `Bytes` field is another — a field whose layout is one of
    /// the synthesized ones in `build_layouts` and which the containing
    /// `datac`'s generated drop function has to free. Keeping the field's own
    /// `Ty` (rather than just a name to look a layout up by) is what lets
    /// `emit_datac_drop` ask `drop_site_for` what frees it.
    Aggregate(String, i32, String, Ty),
    /// `(name, byte offset from the struct's base, width)` — an `f32`/`f64`
    /// field (MK7 §5 item 4's float-in-aggregate slice). A float field is not
    /// a `Scalar`: it lives in `xmm`, so every read, write and initializer
    /// that touches it is a different instruction from the integer one. It is
    /// what makes an eightbyte `SSE` in `classify`, and it is the only new
    /// field kind a `datac` or `enum` can hold that this slice adds.
    Float(String, i32, FWidth),
}

impl StructLayout {
    /// This layout's SysV class — the stored `class` field, which every
    /// aggregate boundary consults.
    fn class(&self) -> AggClass {
        self.class
    }

    /// `Some` for an `enum` — which is also the test every enum-specific path
    /// uses to tell an `enum` from a `datac`, since both are keyed here by
    /// name and both are `Ty::Struct`.
    fn variant_layout(&self, variant: usize) -> Option<&EnumVariantLayout> {
        self.variants.as_ref()?.get(variant)
    }

    fn scalar_field(&self, name: &str) -> Option<(i32, Width, bool)> {
        self.entries.iter().find_map(|e| match e {
            FieldEntry::Scalar(n, o, w, s) if n == name => Some((*o, *w, *s)),
            _ => None,
        })
    }

    /// `(byte offset, width)` for a float field — the `field_read`/
    /// `field_assign` counterpart of `scalar_field`.
    fn float_field(&self, name: &str) -> Option<(i32, FWidth)> {
        self.entries.iter().find_map(|e| match e {
            FieldEntry::Float(n, o, w) if n == name => Some((*o, *w)),
            _ => None,
        })
    }

    /// `(byte offset, layout key, field type)` for an aggregate field, which
    /// is what both a chained field access (`o.inner.v`) and a heap field
    /// (`h.name`) are addressed through.
    fn aggregate_field(&self, name: &str) -> Option<(i32, &str, &Ty)> {
        self.entries.iter().find_map(|e| match e {
            FieldEntry::Aggregate(n, o, key, ty) if n == name => Some((*o, key.as_str(), ty)),
            _ => None,
        })
    }
}

/// The layout key `struct_layout` looks a field's type up under: a `datac`'s
/// own name, or the synthesized `$String`/`$Bytes`/`$View` key. This is the
/// one place the two spellings are related, so `try_build_layout` can place a
/// heap field with the same code it places a nested `datac`.
fn layout_key(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Struct(name) => Some(name.clone()),
        Ty::Bytes => Some(BYTES_LAYOUT_KEY.to_string()),
        Ty::String => Some(STRING_LAYOUT_KEY.to_string()),
        Ty::View => Some(VIEW_LAYOUT_KEY.to_string()),
        Ty::Box(_) => Some(BOX_LAYOUT_KEY.to_string()),
        Ty::List(_) => Some(LIST_LAYOUT_KEY.to_string()),
        Ty::Map(..) => Some(MAP_LAYOUT_KEY.to_string()),
        _ => None,
    }
}

/// How many bytes a value of `ty` occupies in the C layout — `sizeof(T)` for a
/// `Box<T>` element. `None` for a type this backend cannot lay out at all
/// (an `enum`, a closure, a `Vec`-like shape it has no layout for), which is
/// the same answer `struct_layout`
/// gives and what makes `Box<List<i32>>` a refusal rather than an allocation of
/// the wrong size.
fn elem_size(ty: &Ty, layouts: &std::collections::HashMap<String, StructLayout>) -> Option<i32> {
    if let Some(layout) = struct_layout(ty, layouts) {
        return Some(layout.size);
    }
    int_ty(ty).map(|(width, _)| width.bytes())
}

/// The class of a scalar element: one `INTEGER` eightbyte. `elem_class` uses
/// it as the default for a non-aggregate element, whose transport is exactly
/// `One(Int)`'s (one integer register in, `rax` out).
const INT_ONE: AggClass = AggClass::One(EbClass { kind: Eb::Int, size: 8 });

/// The element's SysV class as a *by-value* element: a scalar is `INT_ONE` (the
/// transport of a pointer or an integer), and every aggregate is whatever its
/// own fields classify to. The mirror of `emit_box_routine`'s `class`, repeated
/// for the list routines because both need it and neither is the other's owner.
fn elem_class(elem: &Ty, layouts: &std::collections::HashMap<String, StructLayout>) -> AggClass {
    struct_layout(elem, layouts)
        .map(|l| l.class())
        .unwrap_or(INT_ONE)
}

/// The symbol one `Box<T>` routine is emitted (and called) under. Kept
/// identical to `codegen_c.rs::write_box`'s generated names —
/// `types::box_c_type` plus the operation — so `nm` and a differential run
/// agree, and `drop_fn_name`'s own spelling is used for the drop rather than a
/// second convention being invented here.
#[derive(Clone, PartialEq, Eq)]
enum BoxFn {
    Empty(Ty),
    Of(Ty),
    IsEmpty(Ty),
    /// `__get` when the element comes back by value (in `rax`) and
    /// `__get_const` when a pointer to it does. The flag is decided at the
    /// call site from the call's own result type — a `&T` result is the
    /// borrowing form — which is exactly how `codegen_c.rs` decides it
    /// (`matches!(ret, Ty::Ref { .. })`), so the two backends cannot pick
    /// different spellings for the same instantiation.
    Get(Ty, bool),
    Set(Ty),
    Take(Ty),
    Drop(Ty),
}

impl BoxFn {
    fn elem(&self) -> &Ty {
        match self {
            BoxFn::Empty(e)
            | BoxFn::Of(e)
            | BoxFn::IsEmpty(e)
            | BoxFn::Get(e, _)
            | BoxFn::Set(e)
            | BoxFn::Take(e)
            | BoxFn::Drop(e) => e,
        }
    }

    fn label(&self) -> String {
        let base = format!("kelp_Box__{}", crate::types::ty_mangle(self.elem()));
        match self {
            BoxFn::Empty(_) => format!("{base}__empty"),
            BoxFn::Of(_) => format!("{base}__of"),
            BoxFn::IsEmpty(_) => format!("{base}__is_empty"),
            BoxFn::Get(_, by_value) => {
                if *by_value {
                    format!("{base}__get")
                } else {
                    format!("{base}__get_const")
                }
            }
            BoxFn::Set(_) => format!("{base}__set"),
            BoxFn::Take(_) => format!("{base}__take"),
            // `types::drop_fn_name`'s spelling, which is not the
            // `kelp_Box__*__<op>` shape the rest of the set uses.
            BoxFn::Drop(e) => format!("kelp_box_drop__{}", crate::types::ty_mangle(e)),
        }
    }
}

/// The symbol one `List<T>` routine is emitted (and called) under — kept
/// identical to `codegen_c.rs::write_list`'s generated names
/// (`types::list_c_type` plus the operation), for the same reason `BoxFn`'s
/// are: `nm`, a stack trace and a differential run all have to name the same
/// thing. As with `BoxFn`, `drop_fn_name`'s own spelling is used for the drop
/// rather than a second convention being invented here.
///
/// The set is this slice's deliberate scope: the eight operations a list needs
/// to be a list plus the four S1 ones that are pure element-size arithmetic
/// (`reverse`, `insert`, `remove`, and `bounds`, which they share). `sort`,
/// `contains` and `slice` are **not** here — each needs an element-level
/// comparison or clone the backend has no way to express yet — and a call to
/// one is refused by name at the call site (`Gen::call`'s `List#` dispatch),
/// which keeps the gap visible instead of linking against a symbol nothing
/// emits.
#[derive(Clone, PartialEq, Eq)]
enum ListFn {
    Empty(Ty),
    Of(Ty),
    Len(Ty),
    IsEmpty(Ty),
    Push(Ty),
    Bounds(Ty),
    /// `__get` when the element comes back by value (in `rax`, or `rax`/`rdx`
    /// for a pair) and `__get_const` when a pointer to it does. The flag is
    /// decided at the call site from the call's own result type — a `&T`
    /// result is the borrowing form — which is exactly how `codegen_c.rs`
    /// decides it (`matches!(ret, Ty::Ref { .. })`), and how `BoxFn::Get`
    /// already does.
    Get(Ty, bool),
    Set(Ty),
    Pop(Ty),
    Clear(Ty),
    Insert(Ty),
    Remove(Ty),
    Reverse(Ty),
    Drop(Ty),
}

impl ListFn {
    fn elem(&self) -> &Ty {
        match self {
            ListFn::Empty(e)
            | ListFn::Of(e)
            | ListFn::Len(e)
            | ListFn::IsEmpty(e)
            | ListFn::Push(e)
            | ListFn::Bounds(e)
            | ListFn::Get(e, _)
            | ListFn::Set(e)
            | ListFn::Pop(e)
            | ListFn::Clear(e)
            | ListFn::Insert(e)
            | ListFn::Remove(e)
            | ListFn::Reverse(e)
            | ListFn::Drop(e) => e,
        }
    }

    fn label(&self) -> String {
        let base = format!("kelp_List__{}", crate::types::ty_mangle(self.elem()));
        match self {
            ListFn::Empty(_) => format!("{base}__empty"),
            ListFn::Of(_) => format!("{base}__of"),
            ListFn::Len(_) => format!("{base}__len"),
            ListFn::IsEmpty(_) => format!("{base}__is_empty"),
            ListFn::Push(_) => format!("{base}__push"),
            ListFn::Bounds(_) => format!("{base}__bounds"),
            ListFn::Get(_, by_value) => {
                if *by_value {
                    format!("{base}__get")
                } else {
                    format!("{base}__get_const")
                }
            }
            ListFn::Set(_) => format!("{base}__set"),
            ListFn::Pop(_) => format!("{base}__pop"),
            ListFn::Clear(_) => format!("{base}__clear"),
            ListFn::Insert(_) => format!("{base}__insert"),
            ListFn::Remove(_) => format!("{base}__remove"),
            ListFn::Reverse(_) => format!("{base}__reverse"),
            // `types::drop_fn_name`'s spelling, which is not the
            // `kelp_List__*__<op>` shape the rest of the set uses.
            ListFn::Drop(e) => format!("kelp_list_drop__{}", crate::types::ty_mangle(e)),
        }
    }
}

/// The symbol one `Map<K, V>` routine is emitted (and called) under — kept
/// identical to `codegen_c.rs::write_map`'s generated names
/// (`types::map_c_type` plus the operation) and, for the three shared key
/// helpers, to the same arithmetic those helpers implement. As with `BoxFn`
/// and `ListFn`, `drop_fn_name`'s own spelling is used for the drop rather
/// than a second convention being invented here.
///
/// The set is what a map needs to exist at all: `new`, the probe (`find`),
/// the index rebuild (`rehash`), the mutators (`insert`/`remove`/`clear`),
/// the readers (`has`/`get`/`get_const`/`len`/`is_empty`), the two list
/// producers (`keys`/`values`) and `get_or`. The two conditional operations are
/// `get_or` and `values`, offered only where `types::list_clonable_elem`
/// answers for the value type — the same predicate `infer.rs` uses to offer
/// them in the first place, so the call site and the emitter cannot disagree
/// about whether a symbol exists.
#[derive(Clone, PartialEq, Eq)]
enum MapFn {
    New(Ty, Ty),
    Find(Ty, Ty),
    Rehash(Ty, Ty),
    Insert(Ty, Ty),
    Has(Ty, Ty),
    /// `__get` when the value comes back by value (a `Copy` scalar, in `rax`)
    /// and `__get_const` when a pointer to it does. The flag is decided at the
    /// call site from the call's own result type — a `&V` result is the
    /// borrowing form — which is exactly how `codegen_c.rs` decides it
    /// (`matches!(ret, Ty::Ref { .. })`), and how `BoxFn::Get`/`ListFn::Get`
    /// already do.
    Get(Ty, Ty, bool),
    GetOr(Ty, Ty),
    Remove(Ty, Ty),
    Clear(Ty, Ty),
    Len(Ty, Ty),
    IsEmpty(Ty, Ty),
    Keys(Ty, Ty),
    Values(Ty, Ty),
    Drop(Ty, Ty),
}

impl MapFn {
    fn key(&self) -> &Ty {
        match self {
            MapFn::New(k, _)
            | MapFn::Find(k, _)
            | MapFn::Rehash(k, _)
            | MapFn::Insert(k, _)
            | MapFn::Has(k, _)
            | MapFn::Get(k, _, _)
            | MapFn::GetOr(k, _)
            | MapFn::Remove(k, _)
            | MapFn::Clear(k, _)
            | MapFn::Len(k, _)
            | MapFn::IsEmpty(k, _)
            | MapFn::Keys(k, _)
            | MapFn::Values(k, _)
            | MapFn::Drop(k, _) => k,
        }
    }

    fn value(&self) -> &Ty {
        match self {
            MapFn::New(_, v)
            | MapFn::Find(_, v)
            | MapFn::Rehash(_, v)
            | MapFn::Insert(_, v)
            | MapFn::Has(_, v)
            | MapFn::Get(_, v, _)
            | MapFn::GetOr(_, v)
            | MapFn::Remove(_, v)
            | MapFn::Clear(_, v)
            | MapFn::Len(_, v)
            | MapFn::IsEmpty(_, v)
            | MapFn::Keys(_, v)
            | MapFn::Values(_, v)
            | MapFn::Drop(_, v) => v,
        }
    }

    fn label(&self) -> String {
        let base = crate::types::map_c_type(self.key(), self.value());
        match self {
            MapFn::New(..) => format!("{base}__new"),
            MapFn::Find(..) => format!("{base}__find"),
            MapFn::Rehash(..) => format!("{base}__rehash"),
            MapFn::Insert(..) => format!("{base}__insert"),
            MapFn::Has(..) => format!("{base}__has"),
            MapFn::Get(_, _, by_value) => {
                if *by_value {
                    format!("{base}__get")
                } else {
                    format!("{base}__get_const")
                }
            }
            MapFn::GetOr(..) => format!("{base}__get_or"),
            MapFn::Remove(..) => format!("{base}__remove"),
            MapFn::Clear(..) => format!("{base}__clear"),
            MapFn::Len(..) => format!("{base}__len"),
            MapFn::IsEmpty(..) => format!("{base}__is_empty"),
            MapFn::Keys(..) => format!("{base}__keys"),
            MapFn::Values(..) => format!("{base}__values"),
            // `types::drop_fn_name`'s spelling, which is not the
            // `kelp_Map__*__<op>` shape the rest of the set uses.
            MapFn::Drop(k, v) => format!(
                "kelp_map_drop__{}__{}",
                crate::types::ty_mangle(k),
                crate::types::ty_mangle(v)
            ),
        }
    }
}

fn align_up(cursor: i32, align: i32) -> i32 {
    (cursor + align - 1) / align * align
}

/// Whether a value of `ty` owns heap memory: `Bytes`, `String`, or a `datac`
/// that (transitively) holds one. The free-function twin of
/// `Gen::datac_owns`, for the checks that run before a `Gen` exists or without
/// one to hand — currently the passdown storage check in `generate`, which has
/// to refuse an owning passdown by name because nothing here frees one at
/// process exit.
fn ty_owns(ty: &Ty, layouts: &std::collections::HashMap<String, StructLayout>) -> bool {
    match ty {
        Ty::Bytes | Ty::String | Ty::Box(_) => true,
        // A `List<T>` always owns its buffer, whatever it holds — `empty()` is
        // the only list with nothing allocated, and it still has a header to
        // free (`free(NULL)`). This is why `drop_site_for` must never answer
        // `None` for one: there is no list that needs no drop.
        Ty::List(_) => true,
        // A `Map<K, V>` always owns its two arrays (and, for an owning key or
        // value, what they point at), for the same reason a list does. There
        // is no map that needs no drop.
        Ty::Map(..) => true,
        Ty::Struct(name) => datac_owns_in(name, layouts, &mut std::collections::HashSet::new()),
        _ => false,
    }
}

fn datac_owns_in(
    name: &str,
    layouts: &std::collections::HashMap<String, StructLayout>,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    if !seen.insert(name.to_string()) {
        return false;
    }
    let Some(layout) = layouts.get(name) else {
        return false;
    };
    // MK7 §5's enum slice: an `enum` owns memory when *any* variant does,
    // which is the predicate `types.rs::owned_types` uses ("`Shape` is owned
    // if any variant owns"). `Ty::Struct` recursion below covers an enum
    // payload for free, because both kinds are keyed by name in one map —
    // which is also what `drop_site_for` relies on to tell the two apart only
    // once, where the drop *body* is chosen.
    let mut owns = |ty: &Ty| match ty {
        Ty::Bytes | Ty::String | Ty::Box(_) | Ty::List(_) | Ty::Map(..) => true,
        Ty::Struct(inner) => datac_owns_in(inner, layouts, seen),
        _ => false,
    };
    if layout.entries.iter().any(|e| match e {
        FieldEntry::Aggregate(_, _, _, ty) => owns(ty),
        FieldEntry::Scalar(..) | FieldEntry::Float(..) => false,
    }) {
        return true;
    }
    layout
        .variants
        .as_ref()
        .is_some_and(|variants| {
            variants.iter().any(|v| {
                v.entries.iter().any(|e| match e {
                    FieldEntry::Aggregate(_, _, _, ty) => owns(ty),
                    FieldEntry::Scalar(..) | FieldEntry::Float(..) => false,
                })
            })
        })
}

/// The register file one eightbyte of an aggregate travels in: `SSE` if every
/// field overlapping it is a float or a double, `INTEGER` otherwise (SysV
/// merges `INTEGER` over `SSE`, so one non-float field in an eightbyte is
/// enough to make the whole eightbyte `INTEGER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eb {
    Int,
    Sse,
}

/// One eightbyte's class and how many of its eight bytes the value actually
/// occupies (1..=8). The size is only read for an `SSE` eightbyte, where it
/// selects `movss` (four bytes, a lone `f32`) or `movsd` (eight, everything
/// else); an `INTEGER` eightbyte is always moved as a whole 64-bit word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EbClass {
    kind: Eb,
    size: i32,
}

impl EbClass {
    /// The `movss`/`movsd` width for an `SSE` eightbyte; `None` for an
    /// `INTEGER` one (which is moved through `rax`/`rdx` instead).
    fn float_width(self) -> Option<FWidth> {
        match (self.kind, self.size) {
            (Eb::Sse, 4) => Some(FWidth::F4),
            (Eb::Sse, 8) => Some(FWidth::F8),
            _ => None,
        }
    }
}

/// MK7 §5 item 4, the float-in-aggregate slice: a `datac`'s or `enum`'s own
/// SysV eightbyte classes. This is the real classifier the third slice's
/// size-only split deliberately was not (see that slice's commit message):
/// for a value of at most sixteen bytes, each eightbyte is derived from the
/// *fields* that overlap it — not from the total size — so an all-float
/// `{f64}` is `One(Sse)` while a mixed `{i32, f64}` is `Two(Int, Sse)` and an
/// `enum` whose union has a non-float or payload-less variant in an eightbyte
/// merges that eightbyte back to `INTEGER` through the same rule.
///
/// Everything over sixteen bytes is `MEMORY` whatever its field types, which
/// is why this classifier is total: `try_build_layout` refuses a field it
/// cannot place, so a layout that exists has a class that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AggClass {
    /// ≤8 bytes: one eightbyte.
    One(EbClass),
    /// 9-16 bytes: two eightbytes, in order.
    Two(EbClass, EbClass),
    /// Over 16 bytes: SysV `MEMORY` class — returned through the hidden
    /// first-argument-pointer convention, and (as of the fourth slice) passed
    /// as a by-value argument or parameter **in memory**, in the caller's
    /// outgoing argument area, rather than in registers. There is no
    /// register-*pair* shortcut for one and no register at all: `int_regs`
    /// counts it as zero integer registers, which is what keeps the numbering
    /// of every other argument unaffected by it.
    Mem,
}

impl AggClass {
    /// One `INTEGER` eightbyte — the only aggregate class that is a plain
    /// value `rax` can hold whole (see `value_kind`). Every other class has
    /// its own transport (`xmm`, a register pair, or the hidden pointer) and
    /// is staged through a `Dest`.
    fn is_int_one(self) -> bool {
        matches!(self, AggClass::One(EbClass { kind: Eb::Int, .. }))
    }

    fn is_mem(self) -> bool {
        matches!(self, AggClass::Mem)
    }

    /// The eightbyte at index `i`, or `None` past this class's arity.
    fn eb(self, i: usize) -> Option<EbClass> {
        match self {
            AggClass::One(e) if i == 0 => Some(e),
            AggClass::Two(a, _) if i == 0 => Some(a),
            AggClass::Two(_, b) if i == 1 => Some(b),
            _ => None,
        }
    }

    /// How many integer argument/return registers this class consumes. The
    /// integer and vector files are numbered independently, so this and
    /// `sse_regs` are two separate budgets at every call site and parameter
    /// list — the fact that made the third slice's single `int_regs_for`
    /// insufficient the moment an eightbyte could be `SSE`.
    fn int_regs(self) -> usize {
        match self {
            AggClass::One(e) => usize::from(e.kind == Eb::Int),
            AggClass::Two(a, b) => {
                usize::from(a.kind == Eb::Int) + usize::from(b.kind == Eb::Int)
            }
            AggClass::Mem => 0,
        }
    }

    /// How many vector (`xmm`) argument/return registers this class consumes.
    fn sse_regs(self) -> usize {
        match self {
            AggClass::One(e) => usize::from(e.kind == Eb::Sse),
            AggClass::Two(a, b) => {
                usize::from(a.kind == Eb::Sse) + usize::from(b.kind == Eb::Sse)
            }
            AggClass::Mem => 0,
        }
    }
}

/// Builds one of the synthesized heap layouts, whose fields are all scalars:
/// the atoms and class are computed exactly as `try_build_layout` computes them
/// for a `datac`, so a heap type and a user `datac` are classified by the same
/// code.
fn scalar_layout(entries: Vec<FieldEntry>, size: i32, align: i32) -> StructLayout {
    let empty = std::collections::HashMap::new();
    let mut atoms = Vec::new();
    collect_atoms(&entries, 0, &empty, &mut atoms);
    let class = classify(size, &atoms);
    StructLayout { entries, size, align, variants: None, atoms, class }
}

/// The classifier itself, over a layout's leaf `(offset, size, kind)` atoms.
/// Every eightbyte an atom overlaps is marked with that atom's kind; `INTEGER`
/// wins a tie, and an eightbyte no atom reaches stays `INTEGER` (SysV's
/// `NoClass` merges to nothing, and the only way that happens here is padding,
/// which C also leaves as whatever the other eightbytes' classification says).
/// A size over sixteen bytes is `MEMORY` regardless, which is the one case the
/// atoms do not decide.
fn classify(size: i32, atoms: &[(i32, i32, Eb)]) -> AggClass {
    if size > 16 {
        return AggClass::Mem;
    }
    let n_eb = if size <= 8 { 1 } else { 2 };
    let mut has_int = [false; 2];
    let mut has_sse = [false; 2];
    for &(off, len, kind) in atoms {
        if len <= 0 {
            continue;
        }
        let first = (off / 8).clamp(0, 1) as usize;
        let last = ((off + len - 1) / 8).clamp(0, 1) as usize;
        for e in first..=last {
            match kind {
                Eb::Int => has_int[e] = true,
                Eb::Sse => has_sse[e] = true,
            }
        }
    }
    let eb = |i: usize| {
        let kind = if has_int[i] || !has_sse[i] { Eb::Int } else { Eb::Sse };
        let size = if i == 0 { size.min(8) } else { size - 8 };
        EbClass { kind, size }
    };
    if n_eb == 1 {
        AggClass::One(eb(0))
    } else {
        AggClass::Two(eb(0), eb(1))
    }
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
        if !layout.class().is_mem() {
            continue;
        }
        cursor = align_up(cursor, 8);
        out.push((i, cursor, layout.size));
        cursor += align_up(layout.size, 8);
    }
    (out, align_up(cursor, 16))
}

/// Expands an entry list into leaf atoms for `classify`, shifting an
/// aggregate field's stored atoms by its own offset so a nested layout's
/// per-eightbyte split survives into the parent. A field whose layout is
/// missing contributes nothing — `try_build_layout` only builds an entry for
/// a field it resolved, so that cannot happen for a layout that exists.
fn collect_atoms(
    entries: &[FieldEntry],
    base: i32,
    layouts: &std::collections::HashMap<String, StructLayout>,
    into: &mut Vec<(i32, i32, Eb)>,
) {
    for e in entries {
        match e {
            FieldEntry::Scalar(_, off, w, _) => {
                into.push((base + off, w.bytes(), Eb::Int));
            }
            FieldEntry::Float(_, off, fw) => {
                into.push((base + off, fw.bytes(), Eb::Sse));
            }
            FieldEntry::Aggregate(_, off, key, _) => {
                if let Some(inner) = layouts.get(key) {
                    for &(o, s, k) in &inner.atoms {
                        into.push((base + off + o, s, k));
                    }
                }
            }
        }
    }
}

/// The atoms of an `enum`'s layout: the four-byte tag, then every variant's
/// payload entries, then — for each payload-less variant — the one `char` C
/// writes at the union's own offset. That last atom is what makes the union
/// merge work: it is `INTEGER` in the same eightbyte as a float payload of
/// another variant, so `classify` merges that eightbyte back to `INTEGER`.
fn enum_atoms(
    variants: &[EnumVariantLayout],
    payload_offset: i32,
    layouts: &std::collections::HashMap<String, StructLayout>,
) -> Vec<(i32, i32, Eb)> {
    let mut atoms = vec![(0, ENUM_TAG_WIDTH.bytes(), Eb::Int)];
    for v in variants {
        if v.entries.is_empty() {
            atoms.push((payload_offset, 1, Eb::Int));
        } else {
            collect_atoms(&v.entries, 0, layouts, &mut atoms);
        }
    }
    atoms
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
        } else if let Some(width) = float_ty(&f.ty) {
            // MK7 §5 item 4's float-in-aggregate slice: an `f32`/`f64` field is
            // placed with its own natural alignment and recorded as a distinct
            // `Float` entry, because every read, write and initializer that
            // touches it moves through `xmm` rather than `rax`. Its alignment
            // is 4 for an `f32` and 8 for an `f64`, exactly `FWidth::bytes`.
            let align = width.bytes();
            cursor = align_up(cursor, align);
            entries.push(FieldEntry::Float(f.name.clone(), cursor, width));
            cursor += width.bytes();
            max_align = max_align.max(align);
        } else if let Some(inner) = struct_layout(&f.ty, out) {
            // MK7 §5's heap-field slice: any field whose own layout exists is
            // placed here with natural alignment. That covers a nested `datac`
            // (any size — the copy is `emit_copy`'s arbitrary-width one, not a
            // single `mov`) *and* a `String`/`Bytes` field, whose layout is one
            // of the synthesized ones. The field's key is what its own layout
            // is looked up by later, which is why it is recorded rather than
            // the type alone.
            //
            // A `Box` field (the box slice) is placed the same way, and the
            // element's own layout is deliberately *not* required here: a box
            // is eight bytes whatever it points at, and requiring the element
            // is exactly what would make `datac Node(val next: Box<Node>)`
            // unresolvable, since `Node` has no layout while its own fields
            // are being placed. A `Box<List<i32>>` field is therefore accepted
            // here and refused where it matters — by `Gen::call`'s `Box#`
            // dispatch for a value this program builds, and by
            // `emit_datac_drop` for one it only drops, both by name.
            let key = layout_key(&f.ty).expect("a field with a layout has a layout key");
            let align = inner.align;
            cursor = align_up(cursor, align);
            entries.push(FieldEntry::Aggregate(f.name.clone(), cursor, key, f.ty.clone()));
            cursor += inner.size;
            max_align = max_align.max(align);
        } else {
            return None;
        }
    }
    let size = align_up(cursor, max_align);
    // MK7 §5 item 4, third slice: no size cutoff here any more — every size
    // gets a layout, &classify is what decides whether it needs eightbyte-pair
    // or hidden-pointer handling. Float fields (above) are placed like any
    // other; the classifier, not the field loop, is what turns them into an
    // `SSE` eightbyte.
    let mut atoms = Vec::new();
    collect_atoms(&entries, 0, out, &mut atoms);
    let class = classify(size, &atoms);
    Some(StructLayout { entries, size, align: max_align, variants: None, atoms, class })
}

/// The width of an `enum`'s tag, which is also the width of the variant index
/// written into it: `codegen_c.rs::write_enum` emits `int32_t kelp_tag;` at
/// offset 0 of every tagged union, and the tag's *value* is the variant's index
/// in the declaration — the same number `ExprKind::EnumTag` compares against and
/// `ExprKind::EnumLit` stores, everywhere, with no separate encoding.
const ENUM_TAG_WIDTH: Width = Width::W4;

/// One `enum`'s layout (MK7 §5's enum slice), mirroring
/// `codegen_c.rs::write_enum`'s emitted `typedef` field for field: an
/// `int32_t` tag at offset 0, then a union of one payload struct per variant,
/// sized to the largest of them. `None` for an enum this backend cannot lower
/// — the same answer `try_build_layout` gives a `datac`, and for the same
/// reason: a payload field of a type with no layout (a float above all) makes
/// the whole declaration unusable, and `struct_layout` then refuses by name
/// wherever a value of it is moved rather than misplacing a field.
///
/// The three numbers clang derives and this has to reproduce:
///
/// - **The union's alignment** is the largest alignment among its members, and
///   it is *not* necessarily the payload's own offset — the union is laid out
///   after a four-byte tag, so its offset is `align_up(4, union_align)`. That
///   is **eight**, not four, for every enum whose widest payload holds an
///   eight-byte scalar, which is most of them. `tests/fixtures/enum_layout.kp`
///   pins the resulting sizes and offsets *observably*, by reading a payload of
///   each variant and passing each size class by value, since a Kelp program
///   cannot call `offsetof`; the numbers themselves were checked against
///   `offsetof` on the C typedefs clang produces for the same declarations.
/// - **The union's size** is the largest member's own size rounded up to the
///   union's alignment; a member's size is its own field run rounded up to
///   *its* own alignment, exactly what `try_build_layout` computes for a
///   `datac`. A payload-less variant is one `char` in C (an empty struct is not
///   valid), so it contributes size 1.
/// - **The enum's size** is the union's offset plus the union's size, rounded
///   up to the enum's own alignment — the tag's four bytes and the union's
///   alignment, whichever is larger.
fn try_build_enum_layout(
    e: &crate::types::EnumDef,
    out: &std::collections::HashMap<String, StructLayout>,
) -> Option<StructLayout> {
    let mut variants = Vec::with_capacity(e.variants.len());
    let mut union_align: i32 = 1;
    let mut union_size: i32 = 0;
    for v in &e.variants {
        let mut cursor: i32 = 0;
        let mut max_align: i32 = 1;
        let mut entries = Vec::with_capacity(v.fields.len());
        // The same field rule `try_build_layout` applies to a `datac` — every
        // accepted field is `int_ty`-representable or an aggregate whose own
        // layout exists — and the same refusal when one is neither. A float
        // payload lands here, which is exactly what keeps the four
        // float-payload fixtures reading "this enum" rather than a wrong
        // number.
        for f in &v.fields {
            if let Some((width, signed)) = int_ty(&f.ty) {
                let align = width.bytes();
                cursor = align_up(cursor, align);
                entries.push(FieldEntry::Scalar(f.name.clone(), cursor, width, signed));
                cursor += width.bytes();
                max_align = max_align.max(align);
            } else if let Some(width) = float_ty(&f.ty) {
                // MK7 §5 item 4's float-in-aggregate slice: the same placement
                // and the same `Float` entry a `datac` field gets. A float
                // payload is what makes the union's eightbyte `SSE` — unless
                // another variant or the payload-less `char` merges it back to
                // `INTEGER`, which `enum_atoms`/`classify` decide.
                let align = width.bytes();
                cursor = align_up(cursor, align);
                entries.push(FieldEntry::Float(f.name.clone(), cursor, width));
                cursor += width.bytes();
                max_align = max_align.max(align);
            } else if let Some(inner) = struct_layout(&f.ty, out) {
                let key = layout_key(&f.ty).expect("a field with a layout has a layout key");
                let align = inner.align;
                cursor = align_up(cursor, align);
                entries.push(FieldEntry::Aggregate(f.name.clone(), cursor, key, f.ty.clone()));
                cursor += inner.size;
                max_align = max_align.max(align);
            } else {
                return None;
            }
        }
        // This variant's own member struct, at its own alignment; at least one
        // byte even when it carries nothing (C writes a `char kelp_none`).
        let member_size = align_up(cursor, max_align).max(1);
        union_align = union_align.max(max_align);
        union_size = union_size.max(member_size);
        variants.push(EnumVariantLayout { entries });
    }
    let union_size = align_up(union_size.max(1), union_align);
    let payload_offset = align_up(4, union_align);
    let align = union_align.max(4);
    let size = align_up(payload_offset + union_size, align);
    // Every payload entry's offset is rebased onto the enum's own base, so a
    // reader that already has the enum's address in `r11` needs no second
    // offset for the union.
    for v in &mut variants {
        for entry in &mut v.entries {
            match entry {
                FieldEntry::Scalar(_, offset, ..)
                | FieldEntry::Aggregate(_, offset, ..)
                | FieldEntry::Float(_, offset, ..) => {
                    *offset += payload_offset;
                }
            }
        }
    }
    // The enum's own atoms, computed after the rebase so every offset is
    // relative to the enum's base like the entries are. The tag is an
    // `INTEGER` atom in eightbyte 0, and each payload-less variant contributes
    // the one-byte `char` C puts at the union's offset — the atom that makes
    // the union merge (`enum { C(f64), Empty }` is `INTEGER`/`INTEGER`, not
    // `INTEGER`/`SSE`) fall out of the same classifier every other aggregate
    // uses.
    let atoms = enum_atoms(&variants, payload_offset, out);
    let class = classify(size, &atoms);
    Some(StructLayout {
        entries: Vec::new(),
        size,
        align,
        variants: Some(variants),
        atoms,
        class,
    })
}

/// Computes `StructLayout` for every `datac` and `enum` that qualifies (see
/// `StructLayout`'s doc), by simulating the same sequential, naturally-aligned
/// layout a C compiler gives the equivalent typedef. A declaration that does not
/// qualify is simply absent from the returned map. Nesting means one
/// declaration's own qualification can depend on another's, in either textual
/// order, so this runs to a fixed point rather than in one pass — bounded by the
/// number of declarations, which is enough to resolve any nesting depth since a
/// by-value `datac` or enum payload cannot contain itself (rejected earlier, in
/// `infer.rs`).
fn build_layouts(
    structs: &[crate::types::StructDef],
    enums: &[crate::types::EnumDef],
) -> std::collections::HashMap<String, StructLayout> {
    let mut out = std::collections::HashMap::new();
    // MK7 §5's heap slice: `Bytes` is not a `datac` the program declared, but
    // it *is* an aggregate on the ABI and in the frame, so it gets a layout
    // like one. Adding it here is what makes every site below (`value_kind`,
    // `int_regs_for`, `Frame::declare`, `RetClass`, `eval_struct_into`)
    // handle it with no special case: it is a `Mem`-class three-eightbyte
    // struct, exactly as the C typedef says.
    out.insert(BYTES_LAYOUT_KEY.to_string(), bytes_layout());
    out.insert(STRING_LAYOUT_KEY.to_string(), string_layout());
    out.insert(VIEW_LAYOUT_KEY.to_string(), view_layout());
    // MK7 §5's box slice: one entry covers every `Box<T>` (see
    // `BOX_LAYOUT_KEY`).
    out.insert(BOX_LAYOUT_KEY.to_string(), box_layout());
    // MK7 §5's list slice: same shape of shortcut — one entry covers every
    // `List<T>` (see `LIST_LAYOUT_KEY`).
    out.insert(LIST_LAYOUT_KEY.to_string(), list_layout());
    // MK7 §5's map slice: one entry covers every `Map<K, V>` (see
    // `MAP_LAYOUT_KEY`).
    out.insert(MAP_LAYOUT_KEY.to_string(), map_layout());
    for _ in 0..=(structs.len() + enums.len()) {
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
        // MK7 §5's enum slice: an `enum` is placed by the same fixed point, so
        // a payload that is a `datac` (or another `enum`) declared either
        // before or after it resolves the same way.
        for e in enums {
            if out.contains_key(&e.name) {
                continue;
            }
            if let Some(layout) = try_build_enum_layout(e, &out) {
                out.insert(e.name.clone(), layout);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// `ty`'s layout, if it is a `datac` `build_layouts` accepted — or `Bytes`,
/// whose layout `build_layouts` synthesizes (see `BYTES_LAYOUT_KEY`).
fn struct_layout<'a>(
    ty: &Ty,
    layouts: &'a std::collections::HashMap<String, StructLayout>,
) -> Option<&'a StructLayout> {
    match ty {
        Ty::Struct(name) => layouts.get(name),
        Ty::Bytes => layouts.get(BYTES_LAYOUT_KEY),
        Ty::String => layouts.get(STRING_LAYOUT_KEY),
        Ty::View => layouts.get(VIEW_LAYOUT_KEY),
        Ty::Box(_) => layouts.get(BOX_LAYOUT_KEY),
        Ty::List(_) => layouts.get(LIST_LAYOUT_KEY),
        // MK7 §5's map slice: one key covers every `Map<K, V>` (see
        // `MAP_LAYOUT_KEY`). The entry's own layout is per-key-type, but an
        // entry is never a value the ABI moves — see `map_entry`.
        Ty::Map(..) => layouts.get(MAP_LAYOUT_KEY),
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
                .filter(|l| {
                    matches!(
                        l.class(),
                        AggClass::One(EbClass { kind: Eb::Int, .. })
                    )
                })
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
            // A reference to a *scalar* is the same one-word pointer, which is
            // what makes a `pd var count = 0`'s hidden parameter (`&i32`) and
            // the `Deref` that reads and writes it expressible at all (MK7 §5,
            // the passdown slice). The pointer itself is the value here; what
            // it points at is only ever touched through `ExprKind::Deref`.
            // MK7 §5's next slice adds the float pointee: a `mut f64`
            // out-parameter is one word of address too, so `int_ty(inner)` and
            // `float_ty(inner)` are the same case from here.
            Ty::Ref { inner, .. } if int_ty(inner).is_some() || float_ty(inner).is_some() => {
                Some((Width::W8, false))
            }
            _ => None,
        })
        // MK7 §5 item 8: a `str`/`&str` is a pointer, so it is a value this
        // module already knows how to move whole (`Width::W8`), read from a
        // slot, pass as an argument and return — the only thing missing was
        // saying so. Both spellings, since they are the same C value.
        .or_else(|| if str_ty(ty) { Some((Width::W8, false)) } else { None })
}

/// How many *integer* and *SSE* argument/parameter registers a value of `ty`
/// occupies, or `None` if `ty` cannot be passed by value at all (a closure,
/// `unit`, a type with no layout). A `MEMORY`-class `datac` is `Some((0, 0))`:
/// it *can* be passed by value, in memory rather than in any register — see
/// `memory_arg_area` — and taking no register is what keeps it from disturbing
/// the numbering of the arguments around it. A `float_ty` value is not covered
/// here: every call site checks that first, since SysV classifies the vector
/// and integer register files independently and a scalar float is `(0, 1)`.
///
/// MK7 §5 item 4's float-in-aggregate slice: this is the pair the third slice's
/// single `int_regs_for` could not express. Once an eightbyte can be `SSE`, how
/// many registers a value takes has to be counted per file, and the two
/// registers of a mixed `{i32, f64}` argument are numbered independently
/// (`edi` from the integer file, `xmm0` from the vector file).
fn arg_regs_for(
    ty: &Ty,
    layouts: &std::collections::HashMap<String, StructLayout>,
) -> Option<(usize, usize)> {
    if let Some(layout) = struct_layout(ty, layouts) {
        let class = layout.class();
        return Some((class.int_regs(), class.sse_regs()));
    }
    if int_ty(ty).is_some() {
        return Some((1, 0));
    }
    if str_ty(ty) {
        return Some((1, 0));
    }
    if let Ty::Ref { inner, .. } = ty {
        if struct_layout(inner, layouts).is_some() {
            return Some((1, 0));
        }
        // A `&mut f64` is the same one pointer as a `&mut i64`; MK7 §5's next
        // slice needs it because a `mut f64` out-parameter is declared that
        // way, and the pointer travels in one integer register.
        if int_ty(inner).is_some() || float_ty(inner).is_some() {
            return Some((1, 0));
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

/// Loads exactly `size` bytes (one to eight) at `[base + off]` into `names`'s
/// 64-bit register, zero-extending across it, and reads no byte past the
/// `size` the load names. `names` is a register's four-width name table
/// (`RAX`, `RDX`), the same shape `Width::extend_named` takes.
///
/// This exists because a one-eightbyte aggregate field or element may sit in
/// a heap block *sized to the value* — `Box.of(Outer(...))` allocates
/// `sizeof(Outer)` bytes, and a `List<T>` element is `sizeof(T)` bytes — so a
/// plain eight-byte load of a three-byte field reads past the allocation,
/// which the memory gate reports as an invalid read. The one-instruction
/// forms are the four widths the machine moves directly, each of which
/// already zeroes the rest of the destination register (`movzx` for the byte
/// and word loads, a 32-bit `mov` for the dword); the remaining widths — 3,
/// 5, 6 and 7, what a `datac` of several narrow fields leaves — are assembled
/// byte by byte from the high byte down, which is six instructions on a path
/// nothing hot reaches and the point is that it is exact. Every consumer
/// treats a `One`-class aggregate as an opaque zero-extended eightbyte, so
/// leaving the high bytes zero is the same value the old padded stack slot
/// happened to supply.
fn load_agg_bytes(out: &mut String, names: [&'static str; 4], base: &str, off: i32, size: i32) {
    match size {
        8 => {
            let _ = writeln!(out, "    mov {}, QWORD PTR [{base}{}]", names[3], off_str(off));
        }
        4 => {
            let _ = writeln!(out, "    mov {}, DWORD PTR [{base}{}]", names[2], off_str(off));
        }
        2 => {
            let _ = writeln!(out, "    movzx {}, WORD PTR [{base}{}]", names[3], off_str(off));
        }
        1 => {
            let _ = writeln!(out, "    movzx {}, BYTE PTR [{base}{}]", names[3], off_str(off));
        }
        _ => {
            let _ = writeln!(out, "    xor {}, {}", names[2], names[2]);
            for b in (0..size).rev() {
                if b != size - 1 {
                    let _ = writeln!(out, "    shl {}, 8", names[3]);
                }
                let _ = writeln!(
                    out,
                    "    mov {}, BYTE PTR [{base}{}]",
                    names[0],
                    off_str(off + b)
                );
            }
        }
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

fn emit_copy_to_global(
    out: &mut String,
    label: &str,
    dst_off: i32,
    src_reg: &str,
    src_off: i32,
    mut size: i32,
) {
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
            "    mov {} [rip+{label}{}], {}",
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
    /// An aggregate return, carrying its full `AggClass` so the epilogue and
    /// every call site know which register file each eightbyte comes back in.
    /// `Agg(AggClass::Mem)` is the hidden-pointer case; every other class is
    /// one or two registers (`rax`/`rdx` and/or `xmm0`/`xmm1`).
    Agg(AggClass),
}

impl RetClass {
    /// The hidden-pointer return: the caller passed its destination in `rdi`
    /// and the callee writes the value through it rather than returning one.
    fn is_mem(self) -> bool {
        matches!(self, RetClass::Agg(c) if c.is_mem())
    }
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
    /// MK7 §5 (the passdown slice): one passdown's generated storage, by index
    /// into `Gen::pds` — a `pd` initializer writes straight to the global
    /// rather than into a frame slot. An index rather than the label keeps this
    /// enum `Copy`, which every `Dest`-threading site relies on. It is a
    /// *direct* destination like `Direct` (the storage is at a fixed address),
    /// just reached through `rip` instead of `rbp`.
    Global(usize),
}

/// A register a drop call would clobber, parked in a frame scratch slot for
/// the duration of the drops — see `Gen::park`. `xmm` is `Some` when the
/// value came out of `xmm0` (a float), `None` when it came out of `rax`.
#[derive(Clone, Copy)]
struct Parked {
    slot: i32,
    xmm: Option<FWidth>,
}

/// Generates assembly for `program`.
///
/// `Err` carries a complete diagnostic, printed the same way a frontend error
/// is, so an uncovered construct fails the build instead of producing a binary
/// that is quietly wrong.
pub fn generate(program: &Program, drops: &DropPlan, _sources: &SourceMap) -> Result<String, String> {
    // MK7 §5: an `extern fun` is no longer a program-level refusal. Only the
    // ones a program can actually *call* matter, and only the ones this
    // backend can supply a body for build — see `extern_target`. A live extern
    // with no entry is refused here, naming the declaration exactly as the
    // blanket refusal used to, so nothing about the message shape changed for
    // a program that really does call one.
    let live = live_functions(program);
    for f in program.functions.iter().filter(|f| f.extern_ && live.contains(&f.name)) {
        if extern_target(&f.name).is_none() {
            return Err(unsupported(&format!("the `extern fun {}`", f.name)));
        }
    }
    // Program-level shapes this slice does not lower. Checking them up front
    // gives a message naming the *declaration* rather than whatever expression
    // happened to touch it first. An `enum` is deliberately no longer one of
    // them: like a `datac` that does not qualify (see the `layouts` line
    // below), it is refused where a program actually *uses* a value of it, so a
    // module that merely declares one — `std/str`'s `ParseErr`, which every
    // `impl "std/str"` brings along — no longer poisons the whole program.

    let mut g = Gen::default();
    // MK7 §5 item 5: the drop plan `borrowck.rs` handed over, so an owned
    // `Bytes` is freed at exactly the points the C backend frees one.
    g.drops = drops.clone();
    for f in program.functions.iter().filter(|f| f.extern_ && live.contains(&f.name)) {
        if let Some(target) = extern_target(&f.name) {
            g.externs.insert(f.name.clone(), target);
        }
    }
    // MK7 §5 item 4, first slice: a `datac` is no longer refused outright — a
    // program keeps compiling as long as every `datac` it actually *uses* is
    // one `build_layouts` accepted (see `StructLayout`'s doc). A `datac` that
    // does not qualify is not rejected here either: it is rejected wherever a
    // program tries to move a value of that type, which names the offending
    // type instead of the mere presence of an unrelated declaration. An
    // `enum` is placed by the same pass (MK7 §5's enum slice) and refused by
    // the same rule; `program.enums` is the table `tir.rs` carries so that a
    // `Ty::Struct` can be told apart from a `datac`.
    g.layouts = build_layouts(&program.structs, &program.enums);
    // MK7 §5: one file-scope storage location per passdown, in initialization
    // order, plus the type each one holds — see `PdGlobal`. Every passdown is
    // named by the same internal `pd#<qualified>` identifier its hidden
    // parameter carries, so the parameter and the global are the same name
    // resolved in two different places (`Gen::function`'s prologue binds the
    // parameter; `Gen::expr`'s `Ident` arm falls back to the global).
    for pd in &program.pds {
        let label = Gen::symbol(&pd.name);
        if pd_storage_size(&pd.value_ty, &g.layouts).is_none() {
            return Err(unsupported(&format!(
                "the passdown `pd {}` (its type is `{}`, and the asm backend has no storage \
                 for one — see plan/mk7-own-backend.md §5)",
                pd.qualified,
                pd.value_ty.display()
            )));
        }
        // An *owned* passdown has no scope to die at, so the C backend frees it
        // from an `atexit` handler at process exit (see
        // `codegen_c.rs::write_pd_globals`). This backend does the same thing
        // with a generated `kelp_drop_pds` — the same symbol, the same
        // declaration order, registered from `main` — so the two backends
        // free a passdown at the same moment. The test is `ty_owns` rather
        // than a `Bytes`/`String` match because a `datac` that holds a
        // `String` owns memory too, and one of those as a passdown frees in
        // exactly the same way.
        let drop = if ty_owns(&pd.value_ty, &g.layouts) {
            // `drop_site_for` is what registers the routines the drop body
            // calls, so it has to run here, before `generate` snapshots that
            // set. `None` from it means the type owns memory the backend has
            // no routine for — a `List<Shape>` over an `enum`, say — which is
            // refused by name rather than left unfreed, the rule this module
            // follows everywhere a skipped free could hide.
            let site = g.drop_site_for(&pd.value_ty).ok_or_else(|| {
                unsupported(&format!(
                    "the passdown `pd {}` (its type is `{}`, and the asm backend has no drop \
                     routine for it — see plan/mk7-own-backend.md §5)",
                    pd.qualified,
                    pd.value_ty.display()
                ))
            })?;
            // A fixed routine (`Bytes`/`String`) is only *named* by
            // `drop_site_for` — a drop site registers it when the site is
            // emitted, which is too late here, so name it while the routine set
            // is still open (`require_aggregate_drop`/`require_list_fn` already do
            // this for their own dependencies).
            if let DropSite::Routine(r) = &site {
                g.require_routine(*r);
            }
            Some(site)
        } else {
            None
        };
        g.pds.push(PdGlobal { hidden: pd.name.clone(), label, value_ty: pd.value_ty.clone() });
        g.pd_inits.push(pd.init.clone());
        g.pd_drops.push(drop);
    }
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
        // An `extern fun` has no body to emit — a *call* to one names libc or
        // one of this backend's own routines (`Gen::call`).
        if f.extern_ {
            continue;
        }
        // Unreachable functions are not this program's code: see
        // `live_functions`.
        if !live.contains(&f.name) {
            continue;
        }
        g.function(f)?;
    }
    // MK7 §5: the routines this backend owns for the externs it implements,
    // emitted once each, on demand. Before the panic routine because a routine
    // that can panic is what sets `needs_panic`.
    let mut routines: Vec<Routine> = g.routines.iter().copied().collect();
    routines.sort_by_key(|r| r.label());
    for r in routines {
        g.emit_routine(r);
    }
    // MK7 §5's heap-field slice: one drop function per `datac` that owns
    // memory, for the ones this program actually drops. Emitted after the
    // fixed routines (`require_aggregate_drop` recorded every `Bytes`/`String`
    // routine a body calls into that set) and before the panic routine, so no
    // function body is ever written in the middle of another. The loop re-reads
    // the set each time rather than iterating a snapshot, so a drop function
    // that turns out to need another one (a `datac` held by value in a `datac`
    // dropped later in the program) is emitted too instead of linking against
    // a symbol nothing wrote.
    //
    // MK7 §5's enum slice: one generated drop per `enum` that owns memory too,
    // in the *same* loop — the two kinds call each other (a `datac` field that
    // is an `enum`, an `enum` payload that is a `datac`), and a `datac` drop
    // emitted after the enum loop had stopped would leave the enum's body
    // unwritten. One `emitted` list and two sets is what makes that a fixpoint
    // rather than an ordering assumption; `emit_*` is what re-reads the sets,
    // since writing one body can register the next one.
    let mut emitted: Vec<String> = Vec::new();
    loop {
        let next = g
            .drop_datacs
            .iter()
            .find(|n| !emitted.contains(n))
            .cloned();
        if let Some(name) = next {
            g.emit_datac_drop(&name)?;
            emitted.push(name);
            continue;
        }
        let next = g
            .drop_enums
            .iter()
            .find(|n| !emitted.contains(n))
            .cloned();
        let Some(name) = next else { break };
        g.emit_enum_drop(&name)?;
        emitted.push(name);
    }
    // MK7 §5's box slice: one routine set per `Box<T>` instantiation this
    // program needs. Emitted after the `datac` drops (a `datac` drop can call
    // a box's drop), and read to a fixpoint for the same reason the `datac`
    // loop is: a box's drop calls the drop of its element, which may be
    // another box (`Box<Box<Node>>`).
    let mut emitted_boxes: Vec<String> = Vec::new();
    loop {
        let next = g
            .box_fns
            .iter()
            .find(|f| !emitted_boxes.contains(&f.label()))
            .cloned();
        let Some(f) = next else { break };
        emitted_boxes.push(f.label());
        g.emit_box_routine(&f);
    }
    // MK7 §5's list slice: one routine set per `List<T>` instantiation this
    // program needs. Emitted after the box routines (a list's drop can call a
    // box's drop) and read to a fixpoint for the same reason the other two are:
    // a `List<List<T>>`'s drop calls the inner list's drop.
    let mut emitted_lists: Vec<String> = Vec::new();
    loop {
        let next = g
            .list_fns
            .iter()
            .find(|f| !emitted_lists.contains(&f.label()))
            .cloned();
        let Some(f) = next else { break };
        emitted_lists.push(f.label());
        g.emit_list_routine(&f)?;
    }
    // MK7 §5's map slice: one routine set per `Map<K, V>` instantiation this
    // program needs. Emitted last, after the list routines — a map's
    // `keys`/`values` call the list's `empty` and the shared growth helper, and
    // its drop calls the value's own drop — and read to a fixpoint for the
    // same reason the other three loops are.
    let mut emitted_maps: Vec<String> = Vec::new();
    loop {
        let next = g
            .map_fns
            .iter()
            .find(|f| !emitted_maps.contains(&f.label()))
            .cloned();
        let Some(f) = next else { break };
        emitted_maps.push(f.label());
        g.emit_map_routine(&f)?;
    }
    // MK7 §5: one `kelp_drop_pds` for the passdowns that own memory, emitted
    // after every drop routine it can call (`datac`, `Box`, `List`, `Map`) and
    // before the panic routine, for the same reason those are ordered the way
    // they are: no body is written in the middle of another.
    if g.pd_drops.iter().any(|site| site.is_some()) {
        g.emit_pd_drop_routine();
    }
    // MK7 §5 item 8: the panic routine, after every function and before the
    // data section `finish` writes. One copy per program, and only for a
    // program that can reach it.
    if g.needs_panic {
        g.emit_panic_routine();
    }
    // MK7 §5: the passdowns' storage, in `.bss` — zeroed and assigned by the
    // sequence `Gen::function` puts at the top of `main`, exactly as the C
    // backend's uninitialized `static` globals are assigned by its generated
    // `kelp_init_pds`. A passdown that is read before its own initializer is
    // refused by `infer.rs`, so nothing can observe the zero.
    if !g.pds.is_empty() {
        g.out.push_str("    .section .bss\n");
        for pd in &g.pds {
            let bytes = pd_storage_size(&pd.value_ty, &g.layouts)
                .expect("checked when the table was built");
            let label = pd.label.clone();
            let _ = writeln!(g.out, "    .p2align 3\n{label}:\n    .zero {bytes}");
        }
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
    /// Per declaration id: whether this local's address is ever taken, which
    /// forces it into a frame slot for the same reason a float or an aggregate
    /// does — a pool register has no address, so `&x` on one would be a `lea`
    /// of nothing. MK7 §5's next slice needs this for the caller's half of a
    /// `mut` out-parameter (`parse_i64_raw(s, out = v)` is `&mut v`, and the
    /// callee stores through the address), which is the first time a *scalar*
    /// local is borrowed.
    addr_taken: Vec<bool>,
    /// Per declaration id: whether `&name` on this local would actually take an
    /// address. A `str`/`&T` local is already the pointer such a borrow would
    /// produce — `Gen::expr`'s `Ref` arm returns the name's own value for both
    /// — so marking those would spill a pool register for a borrow that takes
    /// no address at all.
    already_a_pointer: Vec<bool>,
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
            addr_taken: Vec::new(),
            already_a_pointer: Vec::new(),
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
        // A `Bytes` local is 24 bytes and is only ever addressed through
        // `eval_struct_into`/`emit_copy`, so it must be stack-only for the
        // same reason a `datac` is — the register allocator must not hand it
        // one of its five registers. `Box` joined the list with the box slice:
        // it is eight bytes and *could* live in a register, but `&b`/`&mut b`
        // is how every method on it is called, and a borrowed local has to
        // have an address. `List` joined it with the list slice, for the first
        // of those reasons (24 bytes, `eval_struct_into`/`emit_copy` only) and
        // because every method on one takes `&self`/`&mut self`. `Map` joined
        // it with the map slice for both of them at once: its header is 56
        // bytes and every method on one takes `&self`/`&mut self`.
        self.is_agg.push(matches!(
            ty,
            Ty::Struct(_)
                | Ty::Bytes
                | Ty::String
                | Ty::View
                | Ty::Box(_)
                | Ty::List(_)
                | Ty::Map(..)
        ));
        // A `str`/`&T` name is already the pointer a borrow of it would
        // produce (`Gen::expr`'s `Ref` arm returns the value for both), so an
        // `&` written on one takes no address and must not cost the local its
        // register. Anything else whose address is taken does (see
        // `addr_taken`).
        self.already_a_pointer
            .push(str_ty(ty) || matches!(ty, Ty::Ref { .. }));
        self.addr_taken.push(false);
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

    /// The `&name` shape of a use: the same tick and touch `expr` would do for
    /// the name, plus the one thing only a borrow needs — the local loses its
    /// claim on a pool register, because a register has no address. A name
    /// that is already a pointer (`str`, `&T`) takes no address, so it keeps
    /// its register. Anything that is not a bare name (a field of one, a
    /// dereference, a literal) is just walked.
    fn address_of(&mut self, e: &Expr) {
        let ExprKind::Ident(name) = &e.kind else {
            self.expr(e);
            return;
        };
        let p = self.tick();
        let Some(id) = self.lookup(name) else {
            return;
        };
        let r = &mut self.ranges[id];
        r.0 = r.0.min(p);
        r.1 = r.1.max(p);
        if !self.already_a_pointer[id] {
            self.addr_taken[id] = true;
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
                    // A `FieldAccess` target is a write *through* its base, so
                    // the base is as live at this point as it would be for a
                    // read — which matters for a `&datac`/`&mut datac`
                    // parameter, since a reference to a `datac` is a register
                    // candidate (`is_agg` only covers a bare `Ty::Struct`). A
                    // `Deref` target is the same thing one level down: the
                    // operand *is* the borrow being written through, so
                    // `*p = v` has to touch `p`. Missing that touch gave the
                    // operand a one-instruction live range — declaration to
                    // nothing — and the allocator handed its register to
                    // another parameter, which is a silent miscompile rather
                    // than a refusal; two aggregate parameters with one of them
                    // written through are what exposed it, and `pd_owned_reassign.kp`
                    // is the fixture for it.
                    ExprKind::FieldAccess { .. } | ExprKind::Deref { .. } => self.expr(target),
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
            // MK7 §5's enum slice: the four node kinds a tagged union adds, and
            // every one of them has to be walked here for the same reason the
            // `StructLit`/`FieldAccess` arms above exist — a kind left to the
            // catch-all ticks a position and touches nothing, so a live local
            // named inside it looks dead on declaration and the linear scan
            // hands its register to somebody else. That is exactly how
            // `StructLit` and then `FieldAccess`/`Ref`/`Deref` were each caught
            // (by a fixture, as wrong output rather than as a refusal), so the
            // four arms below are written before that can happen a third time,
            // and `tests/fixtures/enum_liveness.kp` is the two-live-locals
            // regression test for them.
            ExprKind::EnumLit { fields, .. } => {
                for v in fields {
                    self.expr(v);
                }
            }
            ExprKind::EnumTag { expr, .. } => self.expr(expr),
            ExprKind::EnumField { base, .. } => self.expr(base),
            // `?` evaluates its operand (which is the only expression the node
            // holds — the rebuilt failure is codegen's own construction).
            ExprKind::Try { expr, .. } => self.expr(expr),
            ExprKind::Ref { expr, .. } => self.address_of(expr),
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
/// list before the scan ever runs — a float id is `None` unconditionally. The
/// same exclusion covers the aggregate kinds (`is_agg`) and, since the
/// scalar-borrow slice, every local whose address is taken (`addr_taken`):
/// both need an address, and a pool register has none.
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
        .filter(|&id| !la.is_float[id] && !la.is_agg[id] && !la.addr_taken[id])
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
    /// MK7 §5 item 8: whether anything in this program can panic (`panic`,
    /// `assert`, or a narrowing conversion), which is what decides whether the
    /// hand-written `kelp_panic` routine is emitted at all — see
    /// `Gen::emit_panic_routine`.
    needs_panic: bool,
    /// MK7 §5 (the passdown slice): every `pd`'s storage, in initialization
    /// order, indexed by `Dest::Global`. Built once from `program.pds`; the
    /// storage itself is emitted in `.bss` and assigned by the sequence
    /// `Gen::function` puts at the top of `main`.
    pds: Vec<PdGlobal>,
    /// MK7 §5: every `pd`'s initializer, in the same order as `pds` —
    /// `pd_init` runs them from `main`. Kept as a second vector rather than
    /// inside `PdGlobal` so that `PdGlobal` stays a plain description of
    /// storage, which is all `Dest::Global` needs.
    pd_inits: Vec<Expr>,
    /// MK7 §5: the drop site of every passdown that owns memory, in the same
    /// order as `pds` — `None` for one that owns nothing. Resolved once, before
    /// any function body is emitted, because `drop_site_for` is what records
    /// the `Bytes`/`String`/`datac`/`List`/`Map` routines that drop calls, and
    /// that set is snapshotted after the last function. `kelp_drop_pds` calls
    /// these in declaration order from an `atexit` handler registered by
    /// `main`, exactly as the C backend's `kelp_init_pds` does.
    pd_drops: Vec<Option<DropSite>>,
    /// MK7 §5 item 1: the lowering for each `extern fun` this program calls and
    /// this backend can supply a body for, keyed by the extern's canonical key
    /// (`std.math.sqrt_raw`). Empty for a program that calls no extern, which
    /// is what makes the refusal a property of the call rather than of the
    /// declaration.
    externs: std::collections::HashMap<String, Extern>,
    /// The routines this backend owns that this program needs, collected as
    /// call sites name them and emitted once each after every function — a
    /// route that writes into `Gen::out` mid-function would put a routine
    /// inside a caller's body.
    routines: std::collections::HashSet<Routine>,
    /// MK7 §5 item 5, first slice: where every owned `Bytes` is freed,
    /// decided by `borrowck.rs` and looked up by span exactly as
    /// `codegen_c.rs` does. Cloned into `Gen` rather than borrowed so the
    /// emitter can hold it while mutating everything else.
    drops: DropPlan,
    /// The function currently being generated — `DropPlan`'s keys are
    /// `(function, span)`, so the drop lookups need it. Set at the top of
    /// `Gen::function` and reset for each one.
    function: String,
    /// MK7 §5's heap-field slice: the `datac`s whose generated drop function
    /// this program calls, transitively. A `datac` that owns a `String` field
    /// needs one; so does a `datac` holding *that* `datac` by value, which is
    /// why this is closed over nested fields at the point a drop is first
    /// needed (`require_aggregate_drop`) rather than being a single name.
    drop_datacs: std::collections::BTreeSet<String>,
    /// MK7 §5's enum slice: the `enum`s whose generated drop function this
    /// program calls, transitively, under the same rule `drop_datacs` follows.
    /// A separate set rather than one more name in that one because the two
    /// emit *different bodies* from the same layout (`emit_datac_drop` walks
    /// `entries`, `emit_enum_drop` switches on the tag and walks one variant),
    /// and a name is only ever in one of them: a layout has `variants` or it
    /// does not.
    drop_enums: std::collections::BTreeSet<String>,
    /// MK7 §5's box slice: the `Box<T>` routines this program needs, labelled
    /// by `BoxFn::label` and emitted after the fixed routines are. A `Vec`
    /// rather than a set because `BoxFn` holds a `Ty` and the label is what
    /// dedupes; the count is bounded by the instantiation count, and `emit`
    /// re-reads it to a fixpoint (`Box<Box<T>>`'s drop calls another box's).
    box_fns: Vec<BoxFn>,
    /// MK7 §5's list slice: the `List<T>` routines this program needs, under
    /// the same discipline as `box_fns` — labelled by `ListFn::label`, emitted
    /// after the box set, and re-read to a fixpoint so a `List<List<T>>`'s drop
    /// (which calls the inner list's drop) is emitted too.
    list_fns: Vec<ListFn>,
    /// MK7 §5's map slice: the `Map<K, V>` routines this program needs, under
    /// the same discipline as `list_fns` — labelled by `MapFn::label`, emitted
    /// last (a map's `keys`/`values` call the list routines, and its drop calls
    /// the value's own drop), and re-read to a fixpoint for the same reason.
    map_fns: Vec<MapFn>,
}

/// One owned type's drop, as a symbol this module can `call`.
#[derive(Clone, PartialEq, Eq)]
enum DropSite {
    /// One of the fixed routines this backend already emits (`Bytes`,
    /// `String`).
    Routine(Routine),
    /// A per-`datac` drop function, generated on demand by
    /// `Gen::emit_datac_drop` under the same symbol `types::drop_fn_name`
    /// gives the C backend (`kelp_drop_<c_ident>`), so a stack trace and a
    /// differential test name the same thing.
    Datac(String),
    /// A per-`enum` drop function (MK7 §5's enum slice), generated by
    /// `Gen::emit_enum_drop` under the same symbol `types::drop_fn_name` gives
    /// the C backend — `kelp_drop_<c_ident>`, exactly as for a `datac`, which
    /// is why the two are distinguished here by *which* generator runs and not
    /// by a different name. The body is the one thing the two share nothing
    /// of: an `enum`'s has to switch on the runtime tag.
    Enum(String),
    /// A per-instantiation `Box<T>` drop, `kelp_box_drop__<mangle>` —
    /// `types::drop_fn_name`'s spelling for `Ty::Box`, emitted by
    /// `Gen::emit_box_routine` like the rest of the box set. Carries the
    /// element so the generator knows which instantiation to write.
    Box(Ty),
    /// A per-instantiation `List<T>` drop, `kelp_list_drop__<mangle>` —
    /// `types::drop_fn_name`'s spelling for `Ty::List`, emitted by
    /// `Gen::emit_list_routine`. Unlike a box, a list **always** owns its
    /// buffer, so this is the one site that can never be skipped when it
    /// applies.
    List(Ty),
    /// A per-instantiation `Map<K, V>` drop, `kelp_map_drop__<k>__<v>` —
    /// `types::drop_fn_name`'s spelling for `Ty::Map`, emitted by
    /// `Gen::emit_map_routine`. A map **always** owns its two arrays, so —
    /// like the list site — this one can never be skipped when it applies.
    Map(Ty, Ty),
}

impl DropSite {
    fn label(&self) -> String {
        match self {
            DropSite::Routine(r) => r.label().to_string(),
            DropSite::Datac(name) | DropSite::Enum(name) => {
                format!("kelp_drop_{}", crate::types::c_ident(name))
            }
            DropSite::Box(elem) => BoxFn::Drop(elem.clone()).label(),
            DropSite::List(elem) => ListFn::Drop(elem.clone()).label(),
            DropSite::Map(key, value) => MapFn::Drop(key.clone(), value.clone()).label(),
        }
    }
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
        // `DropPlan` is keyed by `(function, span)`, so every drop lookup
        // below needs the name of the function whose body is being written.
        self.function = f.name.clone();
        if f.env.is_some() {
            return Err(unsupported(&format!(
                "the closure environment of `{}`",
                f.name
            )));
        }
        for (name, ty) in &f.params {
            if float_ty(ty).is_none() && arg_regs_for(ty, &self.layouts).is_none() {
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
            RetClass::Agg(layout.class())
        } else if int_ty(&f.ret).is_some() {
            RetClass::Scalar
        } else if str_ty(&f.ret) {
            // MK7 §5 item 8: a returned `str` is a pointer in `rax`, which is
            // the same place a scalar returns — `Scalar` is about the
            // transport, not about the type being an integer.
            RetClass::Scalar
        } else {
            return Err(unsupported(&format!(
                "the return type `{}` of `{}`",
                f.ret.display(),
                f.name
            )));
        };
        // A qualifying `datac` parameter consumes its own class's registers:
        // one or two integer registers, one or two vector registers, or none
        // (a `MEMORY`-class value goes in the outgoing area). A `MEMORY`-class
        // *return* reserves `rdi` for the hidden pointer before any real
        // parameter is numbered. Both files are counted, because SysV numbers
        // them independently and a function with seven integer parameters
        // plus two floats is fine while seven floats is not.
        let mut int_regs_needed = if ret_class.is_mem() { 1 } else { 0 };
        let mut sse_regs_needed = 0usize;
        for (_, ty) in &f.params {
            if float_ty(ty).is_some() {
                sse_regs_needed += 1;
            } else if let Some((ints, sses)) = arg_regs_for(ty, &self.layouts) {
                int_regs_needed += ints;
                sse_regs_needed += sses;
            }
        }
        if int_regs_needed > ARG64.len() {
            return Err(unsupported(&format!(
                "`{}`, which needs more than six integer argument registers",
                f.name
            )));
        }
        if sse_regs_needed > FARG.len() {
            return Err(unsupported(&format!(
                "`{}`, which needs more than eight float argument registers",
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
            /// MK7 §5 item 4's float-in-aggregate slice: a by-value aggregate
            /// parameter that is not `MEMORY`-class — `(frame offset, its
            /// class, the first integer-register index, the first
            /// vector-register index)`. Always a stack slot (struct locals are
            /// never register-allocated). The two indices are where this
            /// parameter's own eightbytes start in each file; which of them a
            /// given eightbyte reads is `class.eb(k)`'s answer.
            Agg(i32, AggClass, usize, usize),
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
        let mut int_idx = if ret_class.is_mem() { 1 } else { 0 };
        let mut sse_idx = 0usize;
        for (pos, (name, ty)) in f.params.iter().enumerate() {
            let loc = frame.declare(name, ty, &self.layouts)?;
            if let Some(width) = float_ty(ty) {
                let offset = loc
                    .stack_offset()
                    .expect("float locals are never register-allocated");
                params.push(ParamSlot::Float(offset, width, sse_idx));
                sse_idx += 1;
            } else if let Some(layout) = struct_layout(ty, &self.layouts) {
                let class = layout.class();
                let off = loc.stack_offset().expect("agg locals are stack-only");
                if class.is_mem() {
                    let (_, stack_off, size) = *mem_params
                        .iter()
                        .find(|(i, _, _)| *i == pos)
                        .expect("memory_arg_area covers every Mem-class parameter");
                    params.push(ParamSlot::AggMem(off, stack_off, size));
                } else {
                    params.push(ParamSlot::Agg(off, class, int_idx, sse_idx));
                    int_idx += class.int_regs();
                    sse_idx += class.sse_regs();
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
        // restored in the epilogue from the offset `push` wrote it at — see
        // the epilogue's comment for why it rebuilds `rsp` from `rbp` rather
        // than adding the frame size back, and why that makes the pairing
        // correct however the return was reached.
        for &reg in &used_regs {
            let _ = writeln!(self.out, "    push {}", CALLEE_SAVED64[reg]);
        }
        let frame_size = frame.size().max(16);
        let _ = writeln!(self.out, "    sub rsp, {frame_size}");

        if ret_class.is_mem() {
            let rp = frame.ret_ptr_slot;
            let _ = writeln!(self.out, "    mov QWORD PTR [rbp{rp}], rdi");
        }
        for p in &params {
            match p {
                ParamSlot::Agg(off, class, int0, sse0) => {
                    // Each eightbyte of the parameter is stored from the
                    // register its class names. An `INTEGER` eightbyte is
                    // moved as a whole 64-bit word (the slot is
                    // `align_up(size, 8)` bytes, so this is always contained);
                    // an `SSE` one as `movss`/`movsd` at its own width.
                    let (mut i, mut s) = (*int0, *sse0);
                    for k in 0..2 {
                        let Some(eb) = class.eb(k) else { break };
                        let at = off + 8 * k as i32;
                        match eb.float_width() {
                            Some(w) => {
                                let _ =
                                    writeln!(self.out, "    {} [rbp{at}], {}", w.mov(), FARG[s]);
                                s += 1;
                            }
                            None => {
                                let _ = writeln!(
                                    self.out,
                                    "    mov QWORD PTR [rbp{at}], {}",
                                    ARG64[i]
                                );
                                i += 1;
                            }
                        }
                    }
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
            // MK7 §5 item 4's float-in-aggregate slice: every `return`/tail
            // site for this function already wrote the value into `agg_slot`
            // (`RetClass::Agg` for a register-class aggregate) or through the
            // hidden pointer (`RetClass::Agg(AggClass::Mem)`) — see
            // `Gen::stmt`'s `Return` arm and `Gen::function_body`'s tail
            // handling — so the one thing left is loading the SysV-mandated
            // registers from that fixed location, once, regardless of which
            // `return` actually ran. The two register files are numbered
            // independently and in eightbyte order: the first `INTEGER`
            // eightbyte is `rax`, the second `rdx`; the first `SSE` eightbyte
            // is `xmm0`, the second `xmm1`.
            RetClass::Agg(class) if !class.is_mem() => {
                let agg = frame.agg_slot;
                let (mut i, mut s) = (0usize, 0usize);
                for k in 0..2 {
                    let Some(eb) = class.eb(k) else { break };
                    let at = agg + 8 * k as i32;
                    match eb.float_width() {
                        Some(w) => {
                            let _ = writeln!(
                                self.out,
                                "    {} {}, QWORD PTR [rbp{at}]",
                                w.mov(),
                                FARG[s]
                            );
                            s += 1;
                        }
                        None => {
                            let reg = if i == 0 { "rax" } else { "rdx" };
                            let _ = writeln!(self.out, "    mov {reg}, QWORD PTR [rbp{at}]");
                            i += 1;
                        }
                    }
                }
            }
            RetClass::Agg(_) => {
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
        // The inverse of the prologue: each saved register is loaded back
        // from where `push` put it — `[rbp-8]` for the first, `[rbp-16]` for
        // the next, and so on — and then `rsp` is rebuilt from `rbp` itself.
        //
        // The obvious form is `add rsp, frame_size` followed by the pops, and
        // that is what this was until MK7 §5's enum slice. It is a *return
        // instruction* of a stack machine, so it only works if `rsp` is
        // exactly at the frame's bottom when the jump to `return_label`
        // happens — true for a `return` statement, because a statement is
        // never in the middle of evaluating an expression, and false for the
        // first mid-expression early return this backend has: `?`'s failure
        // arm, which runs while the left operand of a binary operator is
        // still pushed (`1 + half(n)?`) and turned `ret` into a jump to a
        // stack address. Rebuilding `rsp` from `rbp` makes the epilogue
        // independent of how the return was reached, which is what any later
        // mid-expression exit will also need. The offsets are the same ones
        // `push` wrote, so the values restored are identical either way.
        self.out.push_str("    mov rsp, rbp\n");
        for (i, &reg) in used_regs.iter().enumerate() {
            let offset = -8 * (i as i32 + 1);
            let _ = writeln!(
                self.out,
                "    mov {}, QWORD PTR [rbp{offset}]",
                CALLEE_SAVED64[reg]
            );
        }
        self.out.push_str("    pop rbp\n    ret\n");
        let _ = writeln!(self.out, "    .size {symbol}, .-{symbol}");
        Ok(())
    }

    fn function_body(&mut self, f: &Function, frame: &mut Frame) -> Result<(), String> {
        // MK7 §5 (the passdown slice): `main` assigns every passdown before
        // its own body runs, in declaration order — the same job the C
        // backend's generated `kelp_init_pds()` does, and for the same reason:
        // `main` is the one function that can have no hidden parameters, so it
        // is the one place a passdown's storage can be filled from nothing.
        // An initializer may read an earlier passdown (`pd val doubled = base
        // * 2`), which works because this loop is in the same order
        // `program.pds` is. A `pd` read before its own initializer is refused
        // by `infer.rs`, so the order cannot be observed wrong.
        if f.name == "main" {
            for index in 0..self.pds.len() {
                self.pd_init(index, frame)?;
            }
            if self.pd_drops.iter().any(|site| site.is_some()) {
                // An owned passdown has no scope to die at, so its drop point
                // is process exit: `atexit` here, as in the C backend's
                // generated `kelp_init_pds`. Both a `return` from `main` and
                // `exit` run the handler, which is what makes `std/process.exit`
                // free the passdowns too.
                call_align_begin(&mut self.out, frame.sp_slot);
                self.out.push_str("    lea rdi, [rip+kelp_drop_pds]\n    call atexit@PLT\n");
                call_align_end(&mut self.out, frame.sp_slot);
            }
        }
        for stmt in &f.body.stmts {
            self.stmt(stmt, frame)?;
        }
        // MK7 §5 item 5: the function body's own end-of-scope drops, keyed by
        // the body block's identity exactly as
        // `codegen_c.rs::write_function_body` looks them up. A present
        // non-`unit` tail is a `return` (`infer.rs`
        // treats a tail expression the same as `return <tail>`), so a value it
        // leaves in a register is parked while the drops run — the same
        // ordering `write_temp_then_drops` gives a written `return`.
        let drops = self.drops.at_block_end(&self.function, &f.body).to_vec();
        if let Some(tail) = &f.body.tail {
            // MK7 §5 item 4, third slice: a tail expression is a `return`,
            // which `infer.rs` treats the same as `return <tail>` — so a
            // `Two`/`Mem`-class return value has to go through the same
            // `Dest`-based staging `Stmt::Return` uses below, not a plain
            // `self.expr` that would leave it (wrongly) in `rax` alone.
            let parked = match frame.ret_class {
                RetClass::Agg(class) if !class.is_mem() => {
                    let agg = frame.agg_slot;
                    self.eval_struct_into(tail, frame, Dest::Direct(agg))?;
                    None
                }
                RetClass::Agg(_) => {
                    let rp = frame.ret_ptr_slot;
                    self.eval_struct_into(tail, frame, Dest::Indirect(rp))?;
                    None
                }
                _ => {
                    self.expr(tail, frame)?;
                    if drops.is_empty() {
                        None
                    } else {
                        Some(self.park(float_ty(&tail.ty), frame))
                    }
                }
            };
            for t in &drops {
                self.emit_drop(t, frame)?;
            }
            if let Some(p) = parked {
                self.unpark(p);
            }
            if f.ret != Ty::Unit {
                let label = self.return_label.clone();
                let _ = writeln!(self.out, "    jmp {label}");
            }
            return Ok(());
        }
        for t in &drops {
            self.emit_drop(t, frame)?;
        }
        Ok(())
    }

    /// MK7 §5: one passdown's initializer, run at the top of `main`. A struct
    /// is built straight into the passdown's storage (`Dest::Global`, which
    /// every `eval_struct_into` shape understands as a destination, so a
    /// literal, a named local and a call all work); everything else evaluates
    /// as an ordinary expression and is stored at its own width. The two paths
    /// are the same split `Stmt::Let` makes, for the same reason: a value wider
    /// than one eightbyte cannot travel through `rax`.
    fn pd_init(&mut self, index: usize, frame: &mut Frame) -> Result<(), String> {
        let init = self.pd_inits[index].clone();
        let value_ty = self.pds[index].value_ty.clone();
        let label = self.pds[index].label.clone();
        if struct_layout(&value_ty, &self.layouts).is_some() {
            return self.eval_struct_into(&init, frame, Dest::Global(index));
        }
        self.expr(&init, frame)?;
        if str_ty(&value_ty) {
            // A `str` passdown's storage holds the pointer itself.
            let _ = writeln!(self.out, "    mov QWORD PTR [rip+{label}], rax");
            return Ok(());
        }
        let (width, _) = int_ty(&value_ty)
            .ok_or_else(|| unsupported(&format!("the type `{}`", value_ty.display())))?;
        let _ = writeln!(self.out, "    mov {} [rip+{label}], {}", width.mem(), width.rax());
        Ok(())
    }

    /// Looks up a passdown's generated storage by its internal name — the
    /// fallback for a name that is not a local, which is what makes the same
    /// identifier mean "the hidden parameter" inside a function and "the
    /// global" in `main` (and in a `pd` initializer).
    fn pd_global(&self, name: &str) -> Option<usize> {
        self.pds.iter().position(|pd| pd.hidden == name)
    }

    /// MK7 §5 item 5: emits one owned local's `free` at a drop point. `Bytes`
    /// and `String` have fixed routines; a `datac` that (transitively) owns
    /// memory has a generated one, `kelp_drop_<name>` — the same symbol the C
    /// backend's `write_struct` gives the equivalent typedef's drop. A target
    /// of any other type is refused by name rather than silently skipped,
    /// because a skipped free is a leak and a skip that *looks* handled is
    /// exactly what this backend's refusals exist to prevent.
    fn emit_drop(&mut self, target: &DropTarget, frame: &Frame) -> Result<(), String> {
        // A `View` is a `Copy` borrow and has no drop arm in `types.rs`
        // (`drop_fn_name(View)` is `unreachable!`), so this can only be
        // reached if a `DropPlan` were built wrong. Refused by name rather
        // than skipped, the same rule the rest of this module follows: a skip
        // that looks handled is what the refusals exist to prevent.
        if target.ty == Ty::View {
            return Err(unsupported(&format!(
                "dropping the borrowed view `{}` (a `View` owns nothing and is never freed)",
                target.name
            )));
        }
        let Some(site) = self.drop_site_for(&target.ty) else {
            return Err(unsupported(&format!(
                "dropping the owned value `{}` of type `{}`",
                target.name,
                target.ty.display()
            )));
        };
        let off = match frame.lookup(&target.name) {
            Some((loc, _)) => loc
                .stack_offset()
                .expect("an owned heap local is stack-only"),
            None => return Err(unsupported(&format!("a drop of the name `{}`", target.name))),
        };
        self.emit_drop_slot(&site, off, frame)
    }

    /// The drop call for `ty`, if a value of `ty` owns anything: a fixed
    /// routine for `Bytes`/`String`, or a generated per-`datac` function for a
    /// `datac` whose fields (transitively) include one. `None` for a type that
    /// owns nothing — a scalar, a `View`, or a `datac` of scalars — which is
    /// the same answer `types::needs_drop` gives the C backend, and what makes
    /// a `datac`'s generated drop skip its scalar fields instead of calling a
    /// function that does not exist.
    fn drop_site_for(&mut self, ty: &Ty) -> Option<DropSite> {
        match ty {
            Ty::Bytes => Some(DropSite::Routine(Routine::BytesDrop)),
            Ty::String => Some(DropSite::Routine(Routine::StringDrop)),
            // A `Box<T>` owns exactly when its element does: `Box.empty()` is
            // `NULL` and frees nothing, and the box's generated drop tests for
            // that itself. `Box::<List<i32>>::drop` has no element size this
            // backend can allocate, so it is `None` here — and every path that
            // creates one refuses the element first (`Gen::call`'s `Box#`
            // dispatch, and `try_build_layout` for a `Box` field), so a `None`
            // here cannot turn into a silently skipped free.
            Ty::Box(elem) => {
                elem_size(elem, &self.layouts)?;
                self.require_box_fn(BoxFn::Drop((**elem).clone()));
                Some(DropSite::Box((**elem).clone()))
            }
            Ty::Struct(name) => {
                if !self.datac_owns(name) {
                    return None;
                }
                // MK7 §5's enum slice: both kinds are `Ty::Struct` and both are
                // keyed in `layouts` by name, so the layout is what tells them
                // apart — and the two need different *bodies* from the same
                // symbol, a flat field walk versus a switch on the tag. The
                // dependency walk is the same for either, which is why one
                // `require_aggregate_drop` registers the closure and only the
                // site it returns differs.
                self.require_aggregate_drop(name);
                if self.layouts.get(name).is_some_and(|l| l.variants.is_some()) {
                    Some(DropSite::Enum(name.clone()))
                } else {
                    Some(DropSite::Datac(name.clone()))
                }
            }
            // A `List<T>` owns its buffer unconditionally, so this arm is
            // reached for every list a program drops. The element's own size
            // is required first — a routine for a `List<Shape>` over an `enum`
            // has no `sizeof(elem)` to allocate with — and an element this
            // backend cannot lay out comes back as `None`, which is *not* a
            // silent skip: `emit_drop` refuses by name on it, and
            // `emit_datac_drop` guards the field case explicitly, the way the
            // box slice guards a `Box` field.
            Ty::List(elem) => {
                elem_size(elem, &self.layouts)?;
                self.require_list_fn(ListFn::Drop((**elem).clone()));
                Some(DropSite::List((**elem).clone()))
            }
            // A `Map<K, V>` owns its two arrays unconditionally, so this arm is
            // reached for every map a program drops. Both types are required to
            // have a size first — the generated drop indexes entries by the
            // key's size and frees values of the value's size — and a type this
            // backend cannot lay out comes back as `None`, which `emit_drop`
            // refuses by name rather than skipping.
            Ty::Map(key, value) => {
                elem_size(key, &self.layouts)?;
                elem_size(value, &self.layouts)?;
                self.require_map_fn(MapFn::Drop((**key).clone(), (**value).clone()));
                Some(DropSite::Map((**key).clone(), (**value).clone()))
            }
            _ => None,
        }
    }

    /// Records that one `Box<T>` routine is needed, along with everything its
    /// body calls. The element's own drop routine has to be registered *here*
    /// rather than where the body is written, because the routine set is
    /// snapshotted before any body is emitted — the trap the
    /// `String.with_capacity(0)` fix documented. `Drop` is the only box routine
    /// that frees the element (`Set` frees the value it replaces), so both of
    /// those declare the dependency; `Of`/`Take`/`Get` never drop anything.
    fn require_box_fn(&mut self, f: BoxFn) {
        if self.box_fns.iter().any(|x| x.label() == f.label()) {
            return;
        }
        match &f {
            BoxFn::Set(elem) | BoxFn::Drop(elem) => {
                let elem = elem.clone();
                // Registering the element's own drop is the whole point of this
                // arm: `drop_site_for` records whatever that site needs
                // (a `Bytes`/`String` routine, a `datac`'s generated drop, or
                // another box's) before the routine snapshot is taken — and
                // for the two fixed routines it only *names* the site, so
                // naming it here is what puts it in the snapshot, exactly as
                // `require_aggregate_drop` does.
                if let Some(site) = self.drop_site_for(&elem) {
                    if let DropSite::Routine(r) = site {
                        self.require_routine(r);
                    }
                }
            }
            _ => {}
        }
        self.box_fns.push(f);
    }

    /// Records that one `List<T>` routine is needed, along with everything its
    /// body calls. Same before-the-snapshot rule as `require_box_fn`: the
    /// routine set is emitted from a snapshot, so a call registered while a
    /// body is being written would link against a symbol nothing emitted.
    ///
    /// The dependencies, read off the C bodies one for one:
    /// `of`/`push`/`insert` grow the buffer (`Routine::ListReserve`), `get`,
    /// `get_const` and `set` bounds-check (`ListFn::Bounds`), `drop` clears
    /// first, and `set`/`clear`/`drop` free the element they overwrite or
    /// remove — which is `drop_site_for`'s job to resolve (a `Bytes`/`String`
    /// routine, a generated `datac` drop, another box, or another list). A
    /// `Drop` also depends on `Clear`, which is emitted as its own routine
    /// rather than inlined so the two cannot drift.
    fn require_list_fn(&mut self, f: ListFn) {
        if self.list_fns.iter().any(|x| x.label() == f.label()) {
            return;
        }
        match &f {
            ListFn::Of(_) | ListFn::Push(_) | ListFn::Insert(_) => {
                self.require_routine(Routine::ListReserve);
            }
            ListFn::Get(_, _) | ListFn::Set(_) => {
                self.require_list_fn(ListFn::Bounds(f.elem().clone()));
            }
            ListFn::Drop(elem) => {
                let elem = elem.clone();
                self.require_list_fn(ListFn::Clear(elem));
            }
            _ => {}
        }
        if matches!(f, ListFn::Set(_) | ListFn::Clear(_) | ListFn::Drop(_)) {
            // The element's own drop, recorded here rather than at emission
            // time, for the same reason `require_box_fn` records its own: it is
            // `drop_site_for` that decides *which* call that is — and, when it
            // answers with one of the fixed `Bytes`/`String` routines, this is
            // also what puts that routine in the snapshot (`drop_site_for`
            // itself only names it; registering it is the caller's job, exactly
            // as `require_aggregate_drop` does for the same two routines).
            if let Some(site) = self.drop_site_for(f.elem()) {
                if let DropSite::Routine(r) = site {
                    self.require_routine(r);
                }
            }
        }
        self.list_fns.push(f);
    }

    /// Records that one `Map<K, V>` routine is needed, along with everything
    /// its body calls. Same before-the-snapshot rule as `require_box_fn` and
    /// `require_list_fn`: the routine set is emitted from a snapshot, so a call
    /// registered while a body is being written would link against a symbol
    /// nothing emitted.
    ///
    /// The dependencies are `write_map`'s bodies read one for one. Every routine
    /// that probes the index — `insert`, `has`, `get`/`get_const`, `get_or`,
    /// `remove` — needs `find`; `insert` also grows and rebuilds the index
    /// (`rehash`); `find` and `rehash` hash the key, which is `MapHash` +
    /// `MapKeyEq` for a `String` key and `MapMix` for a scalar one; `clear`,
    /// `insert`'s overwrite path and `drop` free live values, so each registers
    /// the value's own drop through `drop_site_for`; `remove` frees the stored
    /// key (`StringDrop`, for a `String` key); `keys` and `values` return a
    /// `List`, so their `ListFn::Empty` and the shared growth helper are
    /// registered *here* rather than at emission time — and the clone of each
    /// element they append (`StringClone`/`BytesClone`) with them; and `drop` is
    /// `clear` plus the two `free`s.
    fn require_map_fn(&mut self, f: MapFn) {
        if self.map_fns.iter().any(|x| x.label() == f.label()) {
            return;
        }
        let key = f.key().clone();
        let value = f.value().clone();
        // Hashing a key is `kelp_map_hash` + `kelp_map_key_eq` for a `String`
        // key and `kelp_map_mix` for a scalar — the same split `write_map`'s
        // `hash_arg`/`hash_stored`/`eq_stored` closures make.
        if key == Ty::String {
            self.require_routine(Routine::MapHash);
            self.require_routine(Routine::MapKeyEq);
        } else {
            self.require_routine(Routine::MapMix);
        }
        match &f {
            MapFn::New(..) | MapFn::Find(..) | MapFn::Rehash(..) | MapFn::Len(..)
            | MapFn::IsEmpty(..) => {}
            MapFn::Has(..) | MapFn::Get(..) | MapFn::GetOr(..) | MapFn::Remove(..) => {
                self.require_map_fn(MapFn::Find(key.clone(), value.clone()));
            }
            MapFn::Insert(..) => {
                self.require_map_fn(MapFn::Find(key.clone(), value.clone()));
                self.require_map_fn(MapFn::Rehash(key.clone(), value.clone()));
                // `entries[e].key = kelp_String__from_str(k)` — the key's own
                // copy-in, for a `String` key.
                if key == Ty::String {
                    self.require_routine(Routine::StringFromStr);
                }
            }
            MapFn::Clear(..) => {
                // The stored key is dropped per live entry, for a `String` key.
                if key == Ty::String {
                    self.require_routine(Routine::StringDrop);
                }
            }
            MapFn::Drop(..) => {
                // `clear` frees every live value and key and resets the header;
                // the drop then frees the two arrays and zeroes the rest.
                self.require_map_fn(MapFn::Clear(key.clone(), value.clone()));
            }
            MapFn::Keys(..) => {
                self.require_list_fn(ListFn::Empty(key.clone()));
                self.require_routine(Routine::ListReserve);
                if key == Ty::String {
                    self.require_routine(Routine::StringClone);
                }
            }
            MapFn::Values(..) => {
                self.require_list_fn(ListFn::Empty(value.clone()));
                self.require_routine(Routine::ListReserve);
                match value {
                    Ty::String => self.require_routine(Routine::StringClone),
                    Ty::Bytes => self.require_routine(Routine::BytesClone),
                    _ => {}
                }
            }
        }
        // The value's own drop, wherever a live value is freed. `drop_site_for`
        // answers `None` for a value that owns nothing (a scalar, a non-owning
        // `datac`), which is exactly `needs_drop`'s answer in the C backend. Its
        // one fixed-routine answer still has to be *registered* here, because
        // `drop_site_for` only names a `Bytes`/`String` routine — registering it
        // is the caller's job, exactly as `require_aggregate_drop`'s callers do.
        if matches!(f, MapFn::Insert(..) | MapFn::Clear(..) | MapFn::Drop(..)) {
            if let Some(site) = self.drop_site_for(&value) {
                if let DropSite::Routine(r) = site {
                    self.require_routine(r);
                }
            }
        }
        if matches!(f, MapFn::GetOr(..)) {
            // `get_or` clones the existing value out on the hit path and drops
            // the fallback it no longer needs.
            match value {
                Ty::String => {
                    self.require_routine(Routine::StringClone);
                    self.require_routine(Routine::StringDrop);
                }
                Ty::Bytes => {
                    self.require_routine(Routine::BytesClone);
                    self.require_routine(Routine::BytesDrop);
                }
                _ => {}
            }
        }
        if matches!(f, MapFn::Remove(..)) && key == Ty::String {
            // `remove` drops the stored key (`kelp_string_drop`); a scalar key
            // is Copy and has nothing to free.
            self.require_routine(Routine::StringDrop);
        }
        self.map_fns.push(f);
    }

    /// Whether a `datac` owns anything, read off the layouts: a `String` or
    /// `Bytes` field anywhere in it, or a nested `datac` that has one. This is
    /// the asm backend's twin of `types::owned_types`, computed on demand
    /// instead of in declaration order — the layouts are already a fixed point
    /// (`build_layouts` runs to one), so this recursion needs no ordering.
    fn datac_owns(&self, name: &str) -> bool {
        datac_owns_in(name, &self.layouts, &mut std::collections::HashSet::new())
    }

    /// Records that `name`'s generated drop function is needed, and every
    /// generated drop it calls — so `generate` can emit the whole closure after
    /// the routine set has been snapshotted. The fixed `Bytes`/`String`
    /// routines a drop body calls are registered here too, for the same reason:
    /// `require_routine` is what puts a routine in that snapshot, and a call
    /// registered after the snapshot links against a symbol nothing emitted.
    ///
    /// MK7 §5's enum slice: one walk serves a `datac` and an `enum`, because
    /// the dependencies are the same in both directions — a `datac` holding an
    /// `enum` calls the enum's generated drop, an `enum` variant holding a
    /// `datac` calls the datac's, and either can hold the other's kind. Which
    /// *set* a name lands in (`drop_datacs` or `drop_enums`) is what decides
    /// which generator writes its body; the fields it depends on are the same
    /// question either way, so they are flattened out of whichever shape the
    /// layout has.
    fn require_aggregate_drop(&mut self, name: &str) {
        let mut stack = vec![name.to_string()];
        while let Some(n) = stack.pop() {
            let Some(layout) = self.layouts.get(&n).cloned() else {
                continue;
            };
            let is_enum = layout.variants.is_some();
            let fresh = if is_enum {
                self.drop_enums.insert(n.clone())
            } else {
                self.drop_datacs.insert(n.clone())
            };
            if !fresh {
                continue;
            }
            let mut fields: Vec<Ty> = Vec::new();
            for entry in &layout.entries {
                if let FieldEntry::Aggregate(_, _, _, ty) = entry {
                    fields.push(ty.clone());
                }
            }
            for variant in layout.variants.iter().flatten() {
                for entry in &variant.entries {
                    if let FieldEntry::Aggregate(_, _, _, ty) = entry {
                        fields.push(ty.clone());
                    }
                }
            }
            for ty in &fields {
                match ty {
                    Ty::Bytes => {
                        self.require_routine(Routine::BytesDrop);
                    }
                    Ty::String => {
                        self.require_routine(Routine::StringDrop);
                    }
                    // MK7 §5's list slice: a `datac` holding a `List` field is
                    // dropped by the list's generated drop, so that one (and
                    // its element's drop, and the growth helper) has to be
                    // registered before the routine snapshot — the same rule
                    // the two routines above and the box below follow. The
                    // element's own size is required first, for the reason
                    // `drop_site_for`'s list arm gives; the guard in
                    // `emit_datac_drop` turns the refusal into a named message.
                    Ty::List(elem) => {
                        if elem_size(elem, &self.layouts).is_some() {
                            self.require_list_fn(ListFn::Drop((**elem).clone()));
                        }
                    }
                    // MK7 §5's box slice: a `datac` holding a `Box` field is
                    // dropped by calling the box's generated drop, so that one
                    // (and everything it calls) has to be registered here —
                    // the same before-the-snapshot rule as the two routines
                    // above. `try_build_layout` only accepted the field after
                    // `elem_size` answered for its element, so this cannot
                    // require a box this backend cannot write.
                    Ty::Box(elem) => {
                        if elem_size(elem, &self.layouts).is_some() {
                            self.require_box_fn(BoxFn::Drop((**elem).clone()));
                        }
                    }
                    // MK7 §5's map slice: a `datac` holding a `Map` field is
                    // dropped by the map's generated drop, so that one (and
                    // everything it calls, including the `List` routines a
                    // `keys()` needs if a field of *that* datac asks for them)
                    // has to be registered before the routine snapshot — the
                    // same rule as the two containers above. Both type
                    // arguments are required to have a size first; the guard in
                    // `emit_datac_drop` turns the refusal into a named message.
                    Ty::Map(key, value) => {
                        if elem_size(key, &self.layouts).is_some()
                            && elem_size(value, &self.layouts).is_some()
                        {
                            self.require_map_fn(MapFn::Drop((**key).clone(), (**value).clone()));
                        }
                    }
                    Ty::Struct(inner) => {
                        if self.datac_owns(inner) {
                            stack.push(inner.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Frees an owned value whose storage this code generator staged in a
    /// `Frame::scratch` slot — a temporary borrowed for a call argument, or
    /// printed directly. The same call `emit_drop` makes, addressed by frame
    /// offset rather than by name, and with the drop chosen by the caller
    /// because a scratch slot carries no type.
    fn emit_drop_slot(&mut self, site: &DropSite, slot: i32, frame: &Frame) -> Result<(), String> {
        if let DropSite::Routine(r) = site {
            self.require_routine(*r);
        }
        call_align_begin(&mut self.out, frame.sp_slot);
        let _ = writeln!(self.out, "    lea rdi, [rbp{slot}]");
        let _ = writeln!(self.out, "    call {}", site.label());
        call_align_end(&mut self.out, frame.sp_slot);
        Ok(())
    }

    /// MK7 §5: `kelp_drop_pds`, the routine `main` hands to `atexit`. One
    /// `lea`/`call` per owning passdown in declaration order — the same order
    /// and the same symbol the C backend generates, so a differential test and
    /// a stack trace name the same thing. `rdi` is the passdown's own storage,
    /// which is why this needs no frame: `rsp` is sixteen-byte aligned at the
    /// calls because `push rbp` lands it there (entry is eight off).
    fn emit_pd_drop_routine(&mut self) {
        let drops: Vec<(String, DropSite)> = self
            .pds
            .iter()
            .zip(self.pd_drops.iter())
            .filter_map(|(pd, site)| site.clone().map(|site| (pd.label.clone(), site)))
            .collect();
        self.out
            .push_str("    .type kelp_drop_pds, @function\nkelp_drop_pds:\n");
        self.out.push_str("    push rbp\n    mov rbp, rsp\n");
        for (label, site) in drops {
            let _ = writeln!(self.out, "    lea rdi, [rip+{label}]");
            let _ = writeln!(self.out, "    call {}", site.label());
        }
        self.out.push_str("    pop rbp\n    ret\n");
        // `atexit` in this glibc is `__cxa_atexit(fn, NULL, __dso_handle)`,
        // living in `libc_nonshared.a`, and `__dso_handle` is normally defined
        // by `crtbegin.o` — which the driver's `ld` line deliberately does not
        // carry (it links `crt1.o`/`crti.o`/`crtn.o` and `-lc` only, with no C
        // compiler in the chain). The value is a *label a handler belongs to*;
        // glibc's `exit` runs every registered handler regardless of it, so a
        // definition here is what `crtbegin.o` would have supplied.
        self.out.push_str(
            "    .section .data\n    .globl __dso_handle\n    .hidden __dso_handle\n\
             __dso_handle:\n    .quad 0\n",
        );
    }

    /// One `datac`'s generated drop function: each owned field in declaration
    /// order, at its own offset, exactly the body `codegen_c.rs::write_struct`
    /// writes. `rdi` is the pointer to the value, kept in a callee-saved
    /// register because a nested drop call is entitled to clobber every
    /// caller-saved one. The `sub rsp, 8` is alignment: at entry `rsp` is eight
    /// bytes off sixteen, `push rbp` and `push rbx` leave it eight off again,
    /// and a `call` must see it aligned.
    fn emit_datac_drop(&mut self, name: &str) -> Result<(), String> {
        let Some(layout) = self.layouts.get(name).cloned() else {
            return Ok(());
        };
        let label = format!("kelp_drop_{}", crate::types::c_ident(name));
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        self.out
            .push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    sub rsp, 8\n");
        self.out.push_str("    mov rbx, rdi\n");
        for entry in &layout.entries {
            let FieldEntry::Aggregate(_, offset, _, ty) = entry else {
                continue;
            };
            // `drop_site_for` returning `None` means "this field owns
            // nothing" — a scalar, a `View`, a `datac` of scalars — *except*
            // for the one case `field_drop_refusal` names, which is a refusal:
            // skipping that would be the silently-leaked free this module's
            // refusals exist to prevent.
            if let Some(what) = self.field_drop_refusal(ty, name) {
                return Err(unsupported(&what));
            }
            let Some(site) = self.drop_site_for(ty) else {
                continue;
            };
            let _ = writeln!(self.out, "    lea rdi, [rbx{}]", off_str(*offset));
            let _ = writeln!(self.out, "    call {}", site.label());
        }
        self.out
            .push_str("    add rsp, 8\n    pop rbx\n    pop rbp\n    ret\n");
        Ok(())
    }

    /// The refusal for an owning field this backend cannot free, or `None` for
    /// every field `drop_site_for` can answer for. `drop_site_for` returns
    /// `None` both for a field that owns nothing and for one whose *own* size
    /// it cannot work out, and those two must not be conflated: the first is a
    /// `continue`, the second is a named refusal, because a skipped free is a
    /// leak the differential suite cannot see (it prints nothing). Shared by
    /// the `datac` and `enum` generated drops, the two bodies that walk owning
    /// fields.
    fn field_drop_refusal(&self, ty: &Ty, owner: &str) -> Option<String> {
        match ty {
            Ty::Box(elem) if elem_size(elem, &self.layouts).is_none() => {
                Some(format!("the `Box<{}>` field of `{owner}`", elem.display()))
            }
            Ty::List(elem) if elem_size(elem, &self.layouts).is_none() => {
                Some(format!("the `List<{}>` field of `{owner}`", elem.display()))
            }
            Ty::Map(key, value)
                if elem_size(key, &self.layouts).is_none()
                    || elem_size(value, &self.layouts).is_none() =>
            {
                Some(format!(
                    "the `Map<{}, {}>` field of `{owner}`",
                    key.display(),
                    value.display()
                ))
            }
            _ => None,
        }
    }

    /// One `enum`'s generated drop (MK7 §5's enum slice), transliterated from
    /// `codegen_c.rs::write_enum`'s `switch`: `switch (v->kelp_tag)` with one
    /// `case i` per variant that owns, dropping each of that variant's owning
    /// payload fields in declaration order, and **no `default` arm** — an
    /// unknown tag is a compiler-visible hole, and every tag this program can
    /// produce is one of the cases. The comparison chain replaces the jump
    /// table clang picks for the C `switch`; the set of tags it can see is the
    /// same, so the two bodies free the same things in the same order.
    ///
    /// Only the live variant's fields are dropped, which is the whole reason
    /// this body has to exist at all: for a `datac` the compiler knows which
    /// fields own, and for an `enum` it does not until the tag is read at
    /// runtime.
    fn emit_enum_drop(&mut self, name: &str) -> Result<(), String> {
        let Some(layout) = self.layouts.get(name).cloned() else {
            return Ok(());
        };
        let Some(variants) = layout.variants.clone() else {
            return Ok(());
        };
        // Resolve every case's sites before writing a single instruction, so a
        // refusal leaves no half-written function behind — and so
        // `drop_site_for`'s registration of the routines a case calls happens
        // in one place. `variants` is cloned, so mutating `self` here is fine.
        let mut cases: Vec<(usize, Vec<(i32, String)>)> = Vec::new();
        for (i, variant) in variants.iter().enumerate() {
            let mut sites: Vec<(i32, String)> = Vec::new();
            for entry in &variant.entries {
                let FieldEntry::Aggregate(_, offset, _, ty) = entry else {
                    continue;
                };
                if let Some(what) = self.field_drop_refusal(ty, name) {
                    return Err(unsupported(&what));
                }
                if let Some(site) = self.drop_site_for(ty) {
                    sites.push((*offset, site.label()));
                }
            }
            if !sites.is_empty() {
                cases.push((i, sites));
            }
        }
        let label = format!("kelp_drop_{}", crate::types::c_ident(name));
        let done = self.label("enum_drop_done");
        let case_labels: Vec<String> = cases.iter().map(|_| self.label("enum_drop_case")).collect();
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        self.out
            .push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    sub rsp, 8\n");
        // The value's own address, in the one register a nested drop call
        // cannot clobber — every other register belongs to the caller.
        self.out.push_str("    mov rbx, rdi\n");
        self.out.push_str("    mov eax, DWORD PTR [rbx]\n");
        for ((variant, _), case) in cases.iter().zip(case_labels.iter()) {
            let _ = writeln!(self.out, "    cmp eax, {variant}");
            let _ = writeln!(self.out, "    je {case}");
        }
        let _ = writeln!(self.out, "    jmp {done}");
        for ((_, sites), case) in cases.iter().zip(case_labels.iter()) {
            let _ = writeln!(self.out, "{case}:");
            for (offset, site) in sites {
                let _ = writeln!(self.out, "    lea rdi, [rbx{}]", off_str(*offset));
                let _ = writeln!(self.out, "    call {site}");
            }
            let _ = writeln!(self.out, "    jmp {done}");
        }
        let _ = writeln!(self.out, "{done}:");
        self.out
            .push_str("    add rsp, 8\n    pop rbx\n    pop rbp\n    ret\n");
        Ok(())
    }

    /// One `Box<T>` instantiation's function set, transliterated from
    /// `codegen_c.rs::write_box`. The box is one pointer; `NULL` is the empty
    /// box, which is what makes a recursive `datac` expressible without an
    /// `Option`.
    ///
    /// The frame is fixed for every routine here: two eight-byte slots at
    /// `rbp-8`/`rbp-16` (the value's address, and the hidden return pointer for
    /// a `MEMORY`-class element) and a buffer at `rbp-K` big enough for the
    /// element, with `K = align_up(size + 16, 16)` so `rsp` stays sixteen-byte
    /// aligned at every `call`. Nothing is kept in a callee-saved register: the
    /// register allocator pins this program's locals to `rbx`/`r12`-`r15`, so a
    /// routine of the runtime must leave them alone, and `malloc`/`free` are
    /// exactly the calls a caller-saved register cannot survive.
    ///
    /// Everything the body calls was registered by `require_box_fn` before the
    /// routine set was snapshotted (see that function), so nothing here has to
    /// remember a dependency at emission time.
    fn emit_box_routine(&mut self, f: &BoxFn) {
        let elem = f.elem().clone();
        let size = elem_size(&elem, &self.layouts)
            .expect("require_box_fn refuses an element with no size");
        // A scalar element is not an aggregate, but its transport is exactly
        // `AggClass::One`'s: one integer register in, `rax` out.
        let class = struct_layout(&elem, &self.layouts)
            .map(|l| l.class())
            .unwrap_or(INT_ONE);
        let label = f.label();
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        let k = align_up(size + 16, 16);
        let buf = -k;
        let slot_self = -8;
        let slot_ret = -16;
        let prologue = format!("    push rbp\n    mov rbp, rsp\n    sub rsp, {k}\n");
        let epilogue = "    mov rsp, rbp\n    pop rbp\n    ret\n";

        match f {
            // `Box<T> b; b.value = NULL; return b;` — the value is one
            // eightbyte in `rax`, which is also how this backend returns any
            // `One`-class aggregate.
            BoxFn::Empty(_) => self.out.push_str("    xor eax, eax\n    ret\n"),
            // `return self->value == NULL;`
            BoxFn::IsEmpty(_) => self.out.push_str(
                "    cmp QWORD PTR [rdi], 0\n    sete al\n    movzx eax, al\n    ret\n",
            ),
            // The `Copy`-element form: the value itself comes back in `rax`.
            // `is_copy` is only ever true for a scalar here, so the width and
            // the extension are the ones an ordinary scalar read uses — an
            // eight-byte load out of a four-byte allocation would be a real
            // out-of-bounds read, which the memory gate would find. The two
            // `get` forms are told apart by `is_copy`, the same predicate
            // `codegen_c.rs::write_box` splits on and `BoxFn`'s label uses.
            BoxFn::Get(_, by_value) if *by_value => {
                let (width, signed) = int_ty(&elem)
                    .expect("the copying `get` form is only emitted for a Copy element");
                let empty = self.label("box_get_empty");
                let msg = self.bytes_label(b"Box.get: the box is empty");
                self.out.push_str("    mov r11, QWORD PTR [rdi]\n    test r11, r11\n");
                let _ = writeln!(self.out, "    je {empty}");
                let _ = writeln!(
                    self.out,
                    "    mov {}, {} [r11]",
                    width.rax(),
                    width.mem()
                );
                self.out.push_str(&width.extend(signed));
                self.out.push_str("    ret\n");
                let _ = writeln!(self.out, "{empty}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // The borrowing form, for an element that is not `Copy`: a pointer
            // to the value in the box.
            BoxFn::Get(_, _) => {
                let empty = self.label("box_get_empty");
                let msg = self.bytes_label(b"Box.get: the box is empty");
                self.out.push_str("    mov rax, QWORD PTR [rdi]\n");
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    je {empty}");
                self.out.push_str("    ret\n");
                let _ = writeln!(self.out, "{empty}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            BoxFn::Of(_) => {
                let oom = self.label("box_of_oom");
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str(&prologue);
                self.marshal_elem(class, size, buf, 0, 0);
                let _ = writeln!(self.out, "    mov edi, {size}");
                self.out.push_str("    call malloc@PLT\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {oom}");
                self.out.push_str("    mov r11, rax\n");
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                self.out.push_str("    mov rax, r11\n");
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{oom}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            BoxFn::Set(_) => {
                let oom = self.label("box_set_oom");
                let alloc = self.label("box_set_alloc");
                let store = self.label("box_set_store");
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                self.marshal_elem(class, size, buf, 1, 0);
                self.out.push_str("    mov r11, QWORD PTR [rdi]\n    test r11, r11\n");
                let _ = writeln!(self.out, "    je {alloc}");
                // The slot is reused: the value it held is freed, the box is
                // not. `self->value` is re-read afterwards rather than kept in
                // a register, because the drop call is free to clobber
                // everything caller-saved.
                if let Some(site) = self.drop_site_for(&elem) {
                    let _ = writeln!(self.out, "    mov rdi, r11");
                    let _ = writeln!(self.out, "    call {}", site.label());
                }
                let _ = writeln!(self.out, "    jmp {store}");
                let _ = writeln!(self.out, "{alloc}:");
                let _ = writeln!(self.out, "    mov edi, {size}");
                self.out.push_str("    call malloc@PLT\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {oom}");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [r11], rax\n");
                let _ = writeln!(self.out, "{store}:");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov r11, QWORD PTR [r11]\n");
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{oom}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            BoxFn::Take(_) => {
                let empty = self.label("box_take_empty");
                let msg = self.bytes_label(b"Box.take: the box is empty");
                self.out.push_str(&prologue);
                // A `MEMORY`-class return takes `rdi` for the hidden pointer
                // *before* the first real argument, so the box comes in `rsi`
                // there and in `rdi` everywhere else.
                let self_reg = if class.is_mem() { "rsi" } else { "rdi" };
                if class.is_mem() {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                }
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], {self_reg}");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [{self_reg}]");
                self.out.push_str("    test r11, r11\n");
                let _ = writeln!(self.out, "    je {empty}");
                // Park the value in the frame before freeing the slot, because
                // `free` is entitled to clobber `rax`/`rdx`/`r11`.
                emit_copy(&mut self.out, "rbp", buf, "r11", 0, size);
                self.out.push_str("    mov rdi, r11\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [r11], 0\n");
                // The value is parked in this routine's own buffer, so hand it
                // back through the same class-driven return helper every other
                // element-returning routine uses.
                let _ = writeln!(self.out, "    lea r11, [rbp{buf}]");
                self.emit_elem_return(&elem, size, class, "r11", slot_ret);
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{empty}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            BoxFn::Drop(_) => {
                let done = self.label("box_drop_done");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                self.out.push_str("    mov r11, QWORD PTR [rdi]\n    test r11, r11\n");
                let _ = writeln!(self.out, "    je {done}");
                if let Some(site) = self.drop_site_for(&elem) {
                    let _ = writeln!(self.out, "    mov rdi, r11");
                    let _ = writeln!(self.out, "    call {}", site.label());
                }
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [r11]\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [r11], 0\n");
                let _ = writeln!(self.out, "{done}:");
                self.out.push_str(epilogue);
            }
        }
    }

    /// Writes each of `class`'s eightbytes from the argument registers it
    /// arrives in into the frame buffer at `buf`: an `INTEGER` eightbyte from
    /// the next `ARG64` register, an `SSE` one from the next `FARG` register.
    /// `int0`/`sse0` are where the element's own registers start in each file —
    /// a routine whose receiver, index or key takes an earlier register starts
    /// the element one (or more) along in the integer file, and the two files
    /// are independent, so an `SSE` element always starts at `xmm0` no matter
    /// how many integer arguments precede it.
    fn store_arg_ebs(&mut self, class: AggClass, buf: i32, int0: usize, sse0: usize) {
        let (mut i, mut s) = (int0, sse0);
        for k in 0..2 {
            let Some(eb) = class.eb(k) else { break };
            let at = buf + 8 * k as i32;
            match eb.float_width() {
                Some(w) => {
                    let _ = writeln!(self.out, "    {} [rbp{at}], {}", w.mov(), FARG[s]);
                    s += 1;
                }
                None => {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{at}], {}", ARG64[i]);
                    i += 1;
                }
            }
        }
    }

    /// The element argument of `__of`/`__set` into the routine's own buffer, by
    /// SysV class: the element's registers for a register-classed element — see
    /// `store_arg_ebs` — and the caller's outgoing argument area, whose first
    /// byte is `[rbp+16]` at a callee's entry, for a `MEMORY`-class one. The
    /// `QWORD` store for an `INTEGER` eightbyte is deliberately wider than some
    /// elements; the buffer is `size + 16` bytes rounded up to sixteen, so it
    /// cannot reach either slot or the caller's frame, and the *allocation*
    /// copy below uses the exact size.
    fn marshal_elem(&mut self, class: AggClass, size: i32, buf: i32, int0: usize, sse0: usize) {
        if class.is_mem() {
            emit_copy(&mut self.out, "rbp", buf, "rbp", 16, size);
        } else {
            self.store_arg_ebs(class, buf, int0, sse0);
        }
    }

    /// `r11 = self->data + i * size` — the address of element `i`, the one piece
    /// of arithmetic every list routine that touches an element shares. `self`
    /// is reloaded from its frame slot and `i` is read from `ireg`; `rax` is
    /// scratch. Nothing else is touched, so a caller may keep a value in any
    /// other register across this (but not across the `call`s around it).
    fn emit_elem_addr(&mut self, ireg: &str, size: i32, slot_self: i32) {
        let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
        self.out.push_str("    mov r11, QWORD PTR [r11]\n");
        let _ = writeln!(self.out, "    mov rax, {ireg}");
        let _ = writeln!(self.out, "    imul rax, {size}");
        self.out.push_str("    add r11, rax\n");
    }

    /// Returns the element sitting at `[addr]` the way its SysV class says to:
    /// `rax` (or `rax`/`rdx`) for a register-classed element, and a copy through
    /// the hidden return pointer for a `MEMORY`-class one — which is what
    /// `List<String>.pop()` is, and why `pop`/`remove` have to know where the
    /// hidden pointer landed. A scalar element is loaded at its own width and
    /// extended, exactly as a local's read is, so a `List<u8>` cannot return
    /// eight bytes of its neighbour.
    fn emit_elem_return(&mut self, elem: &Ty, size: i32, class: AggClass, addr: &str, slot_ret: i32) {
        if class.is_mem() {
            // The destination is the caller's slot, reached through the
            // hidden return pointer; the source stays in its own register,
            // so the two cannot be the same one (`r10` rather than `r11`,
            // which is where every caller of this helper leaves the source
            // address). `emit_copy` touches only `rax`.
            let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot_ret}]");
            emit_copy(&mut self.out, "r10", 0, addr, 0, size);
            self.out.push_str("    mov rax, r10\n");
            return;
        }
        // MK7 §5 item 4's float-in-aggregate slice: an `INTEGER` eightbyte
        // loads into `rax` then `rdx`, an `SSE` one into `xmm0` then `xmm1`.
        // A scalar element is loaded at its own width and extended, exactly as
        // a local's read is, so a `List<u8>` cannot return eight bytes of its
        // neighbour; an aggregate eightbyte is read with `load_agg_bytes` at
        // exactly the eightbyte's own width, so a heap element narrower than
        // eight bytes is never over-read.
        let scalar_int = int_ty(elem);
        let mut s = 0usize;
        for k in 0..2 {
            let Some(eb) = class.eb(k) else { break };
            let off = 8 * k as i32;
            match eb.float_width() {
                Some(w) => {
                    let _ = writeln!(
                        self.out,
                        "    {} {}, [{addr}{}]",
                        w.mov(),
                        FARG[s],
                        off_str(off)
                    );
                    s += 1;
                }
                None => match (k, scalar_int) {
                    (0, Some((width, signed))) => {
                        let _ = writeln!(self.out, "    mov {}, {} [{addr}]", width.rax(), width.mem());
                        self.out.push_str(&width.extend(signed));
                    }
                    (0, None) => load_agg_bytes(&mut self.out, RAX, addr, 0, eb.size),
                    _ => load_agg_bytes(&mut self.out, RDX, addr, off, eb.size),
                },
            }
        }
    }

    /// One `List<T>` instantiation's function set, transliterated from
    /// `codegen_c.rs::write_list` and `write_list_s1`. The list is the same
    /// `{data, len, cap}` header a `Bytes`/`String` is, so a list *value*
    /// travels through paths `Bytes` already uses (a `MEMORY`-class local,
    /// parameter, argument and return — the fourth item-4 slice's outgoing
    /// argument area and hidden return pointer); what is new here is the
    /// per-instantiation routine set over an element that can itself own.
    ///
    /// The frame is fixed for every routine: the receiver at `rbp-8`, the
    /// hidden return pointer at `rbp-16`, two index slots, and a staging buffer
    /// for one element below them, with `k = align_up(size + 48, 16)` so `rsp`
    /// is still sixteen-byte aligned at every `call`. Nothing is kept in a
    /// register across a `call` — `realloc`, `free` and an element's own drop
    /// are all entitled to clobber every caller-saved register, and this
    /// backend's generated functions pin their locals to `rbx`/`r12`-`r15`, so
    /// a runtime routine leaves those alone. That is the whole reason `len` and
    /// `cap` are re-read from memory after a call instead of being cached.
    ///
    /// Everything a body calls was registered by `require_list_fn` before the
    /// routine snapshot was taken, so nothing here has to remember a dependency
    /// at emission time.
    fn emit_list_routine(&mut self, f: &ListFn) -> Result<(), String> {
        let elem = f.elem().clone();
        let size = elem_size(&elem, &self.layouts).ok_or_else(|| {
            unsupported(&format!(
                "the `List<{}>` behind `{}` (the element has no size this backend can allocate)",
                elem.display(),
                f.label()
            ))
        })?;
        let class = elem_class(&elem, &self.layouts);
        let label = f.label();
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        let k = align_up(size + 48, 16);
        let buf = -k;
        let slot_self = -8;
        let slot_ret = -16;
        let slot_i = -24;
        let slot_j = -32;
        let prologue = format!("    push rbp\n    mov rbp, rsp\n    sub rsp, {k}\n");
        let epilogue = "    mov rsp, rbp\n    pop rbp\n    ret\n";
        let reserve = Routine::ListReserve.label();
        let bounds = ListFn::Bounds(elem.clone()).label();
        // The element's own drop, if it has one. Resolved through
        // `drop_site_for` so a `String`, a `Bytes`, a generated `datac` drop, a
        // box and a nested list are all the same call the rest of this module
        // makes — and registered before the snapshot by `require_list_fn`, so a
        // body here can never name a symbol nothing emitted.
        let elem_drop = self.drop_site_for(&elem);
        match f {
            // `List l; l.data = NULL; l.len = 0; l.cap = 0; return l;` — the
            // list is `MEMORY` class, so it is built straight into the caller's
            // return slot and `rdi` is the hidden pointer, not the receiver.
            ListFn::Empty(_) => {
                self.out.push_str(
                    "    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+8], 0\n    mov QWORD PTR [rdi+16], 0\n    mov rax, rdi\n    ret\n",
                );
            }
            // `l = empty(); reserve(&l, 1); l.data[0] = v; l.len = 1;` with `l`
            // built directly in the return slot.
            ListFn::Of(_) => {
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                self.marshal_elem(class, size, buf, 1, 0);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str(
                    "    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+8], 0\n    mov QWORD PTR [rdi+16], 0\n",
                );
                self.out.push_str("    mov esi, 1\n");
                let _ = writeln!(self.out, "    mov edx, {size}");
                let _ = writeln!(self.out, "    call {reserve}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str("    mov r11, QWORD PTR [rdi]\n");
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str("    mov QWORD PTR [rdi+8], 1\n");
                self.out.push_str(epilogue);
            }
            ListFn::Len(_) => {
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n    ret\n");
            }
            ListFn::IsEmpty(_) => {
                self.out
                    .push_str("    cmp QWORD PTR [rdi+8], 0\n    sete al\n    movzx eax, al\n    ret\n");
            }
            // `if (i >= self->len) panic("List.get: index out of bounds");` —
            // the C message, because this is the check `get`/`get_const`/`set`
            // share in the C backend; `insert`/`remove` have their own below.
            ListFn::Bounds(_) => {
                let msg = self.bytes_label(b"List.get: index out of bounds");
                let ok = self.label("list_bounds_ok");
                self.out.push_str("    cmp rsi, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    jb {ok}");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
                let _ = writeln!(self.out, "{ok}:\n    ret");
            }
            // `reserve(self, self->len + 1); self->data[self->len++] = v;` —
            // the element is staged in the frame buffer first, because
            // `realloc` may move the buffer and is free to clobber the register
            // the element arrived in.
            ListFn::Push(_) => {
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                self.marshal_elem(class, size, buf, 1, 0);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rsi, QWORD PTR [rdi+8]\n    add rsi, 1\n");
                let _ = writeln!(self.out, "    mov edx, {size}");
                let _ = writeln!(self.out, "    call {reserve}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                self.emit_elem_addr("rax", size, slot_self);
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str(
                    "    mov rax, QWORD PTR [rdi+8]\n    add rax, 1\n    mov QWORD PTR [rdi+8], rax\n",
                );
                self.out.push_str(epilogue);
            }
            // `bounds(self, i); return self->data[i];` — a `Copy` element by
            // value, or (the borrowing form) a pointer to the element in place.
            // Which one is the call's own result type, never a guess, exactly
            // as `codegen_c.rs::write_list` splits on `elem.is_copy()`.
            ListFn::Get(_, by_value) => {
                if *by_value && class.is_mem() {
                    return Err(unsupported(&format!(
                        "reading a `{}` out of a `List<{}>` by value",
                        elem.display(),
                        elem.display()
                    )));
                }
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rsi");
                let _ = writeln!(self.out, "    call {bounds}");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                self.emit_elem_addr("rsi", size, slot_self);
                if *by_value {
                    self.emit_elem_return(&elem, size, class, "r11", slot_ret);
                } else {
                    self.out.push_str("    mov rax, r11\n");
                }
                self.out.push_str(epilogue);
            }
            // `bounds(self, i); drop(&self->data[i]); self->data[i] = v;`
            ListFn::Set(_) => {
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rsi");
                self.marshal_elem(class, size, buf, 2, 0);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    call {bounds}");
                if let Some(site) = &elem_drop {
                    let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                    self.emit_elem_addr("rsi", size, slot_self);
                    self.out.push_str("    mov rdi, r11\n");
                    let _ = writeln!(self.out, "    call {}", site.label());
                }
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                self.emit_elem_addr("rsi", size, slot_self);
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                self.out.push_str(epilogue);
            }
            // `if (self->len == 0) panic("List.pop: the list is empty");
            //  return self->data[--self->len];` — nothing is dropped, because
            // the element is leaving with the caller.
            ListFn::Pop(_) => {
                let ret_mem = class.is_mem();
                let self_reg = if ret_mem { "rsi" } else { "rdi" };
                let msg = self.bytes_label(b"List.pop: the list is empty");
                let ok = self.label("list_pop_ok");
                self.out.push_str(&prologue);
                if ret_mem {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                }
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], {self_reg}");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [{self_reg}+8]");
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    jne {ok}");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
                let _ = writeln!(self.out, "{ok}:");
                self.out.push_str("    sub rax, 1\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [rdi+8], rax\n");
                self.emit_elem_addr("rax", size, slot_self);
                self.emit_elem_return(&elem, size, class, "r11", slot_ret);
                self.out.push_str(epilogue);
            }
            // `for (i = 0; i < self->len; i++) drop(&self->data[i]);
            //  self->len = 0;` — the loop exists only when the element owns
            // something, which is exactly what `elem_drop` being `Some` means.
            ListFn::Clear(_) => {
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                if let Some(site) = &elem_drop {
                    let top = self.label("list_clear_loop");
                    let done = self.label("list_clear_done");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], 0");
                    let _ = writeln!(self.out, "{top}:");
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                    self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                    let _ = writeln!(self.out, "    cmp QWORD PTR [rbp{slot_i}], rax");
                    let _ = writeln!(self.out, "    jae {done}");
                    let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                    self.emit_elem_addr("rsi", size, slot_self);
                    self.out.push_str("    mov rdi, r11\n");
                    let _ = writeln!(self.out, "    call {}", site.label());
                    let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_i}], 1");
                    let _ = writeln!(self.out, "    jmp {top}");
                    let _ = writeln!(self.out, "{done}:");
                }
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [rdi+8], 0\n");
                self.out.push_str(epilogue);
            }
            // `if (i > self->len) panic("List.insert: index out of bounds");
            //  reserve(self, self->len + 1);
            //  for (j = len; j > i; j--) data[j] = data[j-1];
            //  data[i] = v; self->len++;` — the shift is byte copies, which is
            // the same instruction sequence for a scalar element as for a
            // `String`, so nothing here has to know the element's class.
            ListFn::Insert(_) => {
                let msg = self.bytes_label(b"List.insert: index out of bounds");
                let ok = self.label("list_insert_ok");
                let top = self.label("list_insert_loop");
                let done = self.label("list_insert_done");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rsi");
                self.marshal_elem(class, size, buf, 2, 0);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    cmp QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    jbe {ok}");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
                let _ = writeln!(self.out, "{ok}:");
                self.out.push_str("    mov rsi, rax\n    add rsi, 1\n");
                let _ = writeln!(self.out, "    mov edx, {size}");
                let _ = writeln!(self.out, "    call {reserve}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_j}], rax");
                let _ = writeln!(self.out, "{top}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_j}]");
                let _ = writeln!(self.out, "    cmp rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    jbe {done}");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov r11, QWORD PTR [r11]\n");
                self.out.push_str("    mov r9, rax\n");
                let _ = writeln!(self.out, "    imul r9, {size}");
                self.out.push_str("    add r9, r11\n    mov r8, r9\n");
                let _ = writeln!(self.out, "    sub r8, {size}");
                emit_copy(&mut self.out, "r9", 0, "r8", 0, size);
                let _ = writeln!(self.out, "    sub QWORD PTR [rbp{slot_j}], 1");
                let _ = writeln!(self.out, "    jmp {top}");
                let _ = writeln!(self.out, "{done}:");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                self.emit_elem_addr("rsi", size, slot_self);
                emit_copy(&mut self.out, "r11", 0, "rbp", buf, size);
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str(
                    "    mov rax, QWORD PTR [rdi+8]\n    add rax, 1\n    mov QWORD PTR [rdi+8], rax\n",
                );
                self.out.push_str(epilogue);
            }
            // `if (i >= self->len) panic("List.remove: index out of bounds");
            //  v = self->data[i];
            //  for (j = i; j + 1 < len; j++) data[j] = data[j+1];
            //  self->len--; return v;` — `v` is parked in the frame buffer
            // first, because the shift is free to overwrite the slot it came
            // from (its last iteration always does).
            ListFn::Remove(_) => {
                let ret_mem = class.is_mem();
                let self_reg = if ret_mem { "rsi" } else { "rdi" };
                let i_reg = if ret_mem { "rdx" } else { "rsi" };
                let msg = self.bytes_label(b"List.remove: index out of bounds");
                let ok = self.label("list_remove_ok");
                let top = self.label("list_remove_loop");
                let done = self.label("list_remove_done");
                self.out.push_str(&prologue);
                if ret_mem {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                }
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], {self_reg}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], {i_reg}");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [{self_reg}+8]");
                let _ = writeln!(self.out, "    cmp {i_reg}, rax");
                let _ = writeln!(self.out, "    jb {ok}");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
                let _ = writeln!(self.out, "{ok}:");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_i}]");
                self.emit_elem_addr("rsi", size, slot_self);
                emit_copy(&mut self.out, "rbp", buf, "r11", 0, size);
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_j}], rax");
                let _ = writeln!(self.out, "{top}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_j}]");
                self.out.push_str("    add rcx, 1\n");
                let _ = writeln!(self.out, "    cmp rcx, rax");
                let _ = writeln!(self.out, "    jae {done}");
                self.out.push_str("    mov r11, QWORD PTR [rdi]\n");
                let _ = writeln!(self.out, "    mov r8, QWORD PTR [rbp{slot_j}]");
                let _ = writeln!(self.out, "    imul r8, {size}");
                self.out.push_str("    add r8, r11\n    mov r9, r8\n");
                let _ = writeln!(self.out, "    add r9, {size}");
                emit_copy(&mut self.out, "r8", 0, "r9", 0, size);
                let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_j}], 1");
                let _ = writeln!(self.out, "    jmp {top}");
                let _ = writeln!(self.out, "{done}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str(
                    "    mov rax, QWORD PTR [rdi+8]\n    sub rax, 1\n    mov QWORD PTR [rdi+8], rax\n",
                );
                // `buf` already holds `v`; hand it back the way its class says.
                let _ = writeln!(self.out, "    lea r11, [rbp{buf}]");
                self.emit_elem_return(&elem, size, class, "r11", slot_ret);
                self.out.push_str(epilogue);
            }
            // `i = 0; j = self->len; while (i + 1 < j) { j--;
            //  t = data[i]; data[i] = data[j]; data[j] = t; i++; }` — the swap
            // goes through the frame buffer, which is what makes it work for an
            // element of any size without a second temporary slot.
            ListFn::Reverse(_) => {
                let top = self.label("list_reverse_loop");
                let done = self.label("list_reverse_done");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], 0");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_j}], rax");
                let _ = writeln!(self.out, "{top}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    add rax, 1\n");
                let _ = writeln!(self.out, "    cmp rax, QWORD PTR [rbp{slot_j}]");
                let _ = writeln!(self.out, "    jae {done}");
                let _ = writeln!(self.out, "    sub QWORD PTR [rbp{slot_j}], 1");
                let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov r10, QWORD PTR [r10]\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    imul rax, {size}");
                self.out.push_str("    add rax, r10\n    mov r8, rax\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_j}]");
                let _ = writeln!(self.out, "    imul rax, {size}");
                self.out.push_str("    add rax, r10\n    mov r9, rax\n");
                emit_copy(&mut self.out, "rbp", buf, "r8", 0, size);
                emit_copy(&mut self.out, "r8", 0, "r9", 0, size);
                emit_copy(&mut self.out, "r9", 0, "rbp", buf, size);
                let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_i}], 1");
                let _ = writeln!(self.out, "    jmp {top}");
                let _ = writeln!(self.out, "{done}:");
                self.out.push_str(epilogue);
            }
            // `clear(self); free(self->data); self->data = NULL;
            //  self->cap = 0;` — `clear` resets `len`, so all three fields end
            // up exactly as a fresh `empty()` has them.
            ListFn::Drop(_) => {
                let clear = ListFn::Clear(elem.clone()).label();
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    call {clear}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [rdi]\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+16], 0\n");
                self.out.push_str(epilogue);
            }
        }
        Ok(())
    }

    /// The hash of the map's *key argument* (the `Karg` a method received),
    /// which lives in `key_slot`, into `rax` — `codegen_c.rs::write_map`'s
    /// `hash_arg` closure. A `String` key is a `const char*`, so it is hashed
    /// over its bytes (`strlen` then `kelp_map_hash`); a scalar key is already
    /// the value the map stores, so it is widened and mixed.
    fn emit_map_hash_arg(&mut self, key: &Ty, key_slot: i32) {
        if key == &Ty::String {
            let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{key_slot}]");
            self.out.push_str("    call strlen@PLT\n    mov rsi, rax\n");
            let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{key_slot}]");
            let _ = writeln!(self.out, "    call {}", Routine::MapHash.label());
        } else {
            let (w, signed) =
                int_ty(key).expect("a map key is `String`, an integer or `bool`");
            let _ = writeln!(self.out, "    mov {}, {} [rbp{key_slot}]", w.rax(), w.mem());
            self.out.push_str(&w.extend(signed));
            self.out.push_str("    mov rdi, rax\n");
            let _ = writeln!(self.out, "    call {}", Routine::MapMix.label());
        }
    }

    /// The hash of a *stored* key — the `kelp_String` (or scalar) sitting at
    /// offset 0 of the entry whose address is in `entry_slot` — into `rax`.
    /// `codegen_c.rs::write_map`'s `hash_stored` closure, and the reason
    /// hashing an argument and hashing a stored key are two different reads of
    /// the same bytes.
    fn emit_map_hash_stored(&mut self, key: &Ty, entry_slot: i32) {
        if key == &Ty::String {
            // `kelp_map_hash(key.data, key.len)` — the entry's key is a
            // `{data, len, cap}` at offset 0.
            let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{entry_slot}]");
            self.out.push_str("    mov rsi, QWORD PTR [rax+8]\n    mov rdi, QWORD PTR [rax]\n");
            let _ = writeln!(self.out, "    call {}", Routine::MapHash.label());
        } else {
            let (w, signed) = int_ty(key).expect("a map key is `String`, an integer or `bool`");
            let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{entry_slot}]");
            let _ = writeln!(self.out, "    mov {}, {} [rdx]", w.rax(), w.mem());
            self.out.push_str(&w.extend(signed));
            self.out.push_str("    mov rdi, rax\n");
            let _ = writeln!(self.out, "    call {}", Routine::MapMix.label());
        }
    }

    /// Whether the entry whose address is in `entry_slot` has the same key as
    /// the argument in `key_slot`, as 0/1 in `rax` — `codegen_c.rs::write_map`'s
    /// `eq_stored` closure. A `String` key compares length-first through
    /// `kelp_map_key_eq` (never `strcmp`, because a stored key may hold bytes
    /// `strlen` would stop at); a scalar key compares by value, at its own
    /// width and signedness, so a four-byte key's untouched upper bytes cannot
    /// make two equal keys look different.
    fn emit_map_key_eq(&mut self, key: &Ty, entry_slot: i32, key_slot: i32) {
        if key == &Ty::String {
            let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{key_slot}]");
            self.out.push_str("    call strlen@PLT\n    mov rdx, rax\n");
            let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{entry_slot}]");
            let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{key_slot}]");
            let _ = writeln!(self.out, "    call {}", Routine::MapKeyEq.label());
            self.out.push_str("    movzx eax, al\n");
        } else {
            let (w, signed) = int_ty(key).expect("a map key is `String`, an integer or `bool`");
            let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{entry_slot}]");
            let _ = writeln!(self.out, "    mov {}, {} [rdx]", w.rax(), w.mem());
            self.out.push_str(&w.extend(signed));
            let _ = writeln!(self.out, "    cmp rax, QWORD PTR [rbp{key_slot}]");
            self.out.push_str("    sete al\n    movzx eax, al\n");
        }
    }

    /// `entries[e].key = store_key(k)` — the key argument in `key_slot` written
    /// into the entry whose address is in `entry_slot`. A `String` key copies
    /// in from the borrowed `str` (`kelp_String__from_str` writes its result
    /// straight through the hidden pointer, which is the entry's own key, so no
    /// temporary is needed); a scalar key is stored at its own width.
    fn emit_map_store_key(&mut self, key: &Ty, entry_slot: i32, key_slot: i32) {
        if key == &Ty::String {
            let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{entry_slot}]");
            let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{key_slot}]");
            let _ = writeln!(self.out, "    call {}", Routine::StringFromStr.label());
        } else {
            let (w, _) = int_ty(key).expect("a map key is `String`, an integer or `bool`");
            let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{key_slot}]");
            let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{entry_slot}]");
            let _ = writeln!(self.out, "    mov {} [rdx], {}", w.mem(), w.rax());
        }
    }

    /// The address of the live entry the index slot held in `slot_i` names
    /// (`&self->entries[self->index[slot] - 1]`) into `slot_e`. Every routine
    /// that probes the index and then touches the entry needs exactly this, and
    /// it is in memory rather than a register because the calls in between are
    /// entitled to clobber every caller-saved register.
    fn emit_map_entry_addr(&mut self, entry_size: i32, slot_self: i32, slot_i: i32, slot_e: i32) {
        let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
        self.out.push_str("    mov rcx, QWORD PTR [rcx+32]\n");
        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
        self.out.push_str("    mov rax, QWORD PTR [rcx+rax*8]\n    sub rax, 1\n");
        let _ = writeln!(self.out, "    imul rax, {entry_size}");
        let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
        self.out.push_str("    mov rcx, QWORD PTR [rcx]\n    add rcx, rax\n");
        let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rcx");
    }

    /// A value held at `[src]` handed back the way its SysV class says to:
    /// `rax` (or `rax`/`rdx`) for a register-classed value, and a copy through
    /// the hidden return pointer for a `MEMORY`-class one. The same rule
    /// `emit_elem_return` applies, with one difference: the source here is a
    /// `malloc(sizeof(V))` block, exactly as wide as the value, so a `One`-class
    /// `datac` narrower than eight bytes is staged in a frame slot before the
    /// eight-byte load rather than read past the end of its own allocation
    /// (which the memory gate would — correctly — call an invalid read).
    fn emit_map_value_out(
        &mut self,
        value: &Ty,
        size: i32,
        class: AggClass,
        src: &str,
        _slot_v: i32,
        slot_ret: i32,
    ) {
        if class.is_mem() {
            let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot_ret}]");
            emit_copy(&mut self.out, "r10", 0, src, 0, size);
            self.out.push_str("    mov rax, r10\n");
            return;
        }
        // MK7 §5 item 4's float-in-aggregate slice: the same per-eightbyte
        // return `emit_elem_return` produces, from a `malloc(sizeof(V))` block
        // that is exactly as wide as the value — so an aggregate eightbyte is
        // read with `load_agg_bytes` at its own width and never past the
        // allocation.
        let scalar_int = int_ty(value);
        let mut s = 0usize;
        for k in 0..2 {
            let Some(eb) = class.eb(k) else { break };
            let off = 8 * k as i32;
            match eb.float_width() {
                Some(w) => {
                    let _ = writeln!(
                        self.out,
                        "    {} {}, [{src}{}]",
                        w.mov(),
                        FARG[s],
                        off_str(off)
                    );
                    s += 1;
                }
                None => match (k, scalar_int) {
                    (0, Some((w, signed))) => {
                        let _ = writeln!(self.out, "    mov {}, {} [{src}]", w.rax(), w.mem());
                        self.out.push_str(&w.extend(signed));
                    }
                    (0, None) => load_agg_bytes(&mut self.out, RAX, src, 0, eb.size),
                    _ => load_agg_bytes(&mut self.out, RDX, src, off, eb.size),
                },
            }
        }
    }

    /// `*entries[e].value = v` — the value the method received, written into
    /// the entry's value block at `[dst]`. A register-classed value was staged
    /// in `slot_v` when the routine began (because `malloc`/`realloc` are free
    /// to clobber the register it arrived in); a `MEMORY`-class one is still in
    /// the caller's outgoing argument area at `[rbp+16]`, which no call can
    /// invalidate, so no copy of it is made anywhere else.
    fn emit_map_store_value(&mut self, size: i32, class: AggClass, dst: &str, slot_v: i32) {
        if class.is_mem() {
            emit_copy(&mut self.out, dst, 0, "rbp", 16, size);
        } else {
            emit_copy(&mut self.out, dst, 0, "rbp", slot_v, size);
        }
    }

    /// One `Map<K, V>` instantiation's function set, transliterated from
    /// `codegen_c.rs::write_map`. The map *header* is the same 56 bytes for
    /// every instantiation (`map_layout`), so a map *value* travels the ABI
    /// paths `Bytes`/`List` already use — the fourth item-4 slice's
    /// `MEMORY`-class argument area and hidden return pointer — with no new
    /// code in `Gen::call`. What is per-instantiation is the entry: the key is
    /// inline, so every routine that probes, stores or frees one has to know
    /// the key's own width and hash.
    ///
    /// The frame is fixed for every routine: eleven words of slots (`self`,
    /// the key argument, the `find` out-parameter, the index capacity, three
    /// counters, the hidden return pointer, the entry address and a staging
    /// word) plus a sixteen-byte value staging block, with `sub rsp, 112` so
    /// `rsp` is still sixteen-byte aligned at every `call`. Nothing is kept in
    /// a register across a call — `malloc`, `realloc`, `free`, `memcmp` and an
    /// element's own drop are all entitled to clobber every caller-saved
    /// register, and this backend's generated functions pin their locals to
    /// `rbx`/`r12`-`r15`, so a runtime routine leaves those alone. That is why
    /// every live value is re-read from memory after a call instead of being
    /// cached.
    ///
    /// Everything a body calls was registered by `require_map_fn` before the
    /// routine snapshot was taken, so nothing here has to remember a dependency
    /// at emission time.
    fn emit_map_routine(&mut self, f: &MapFn) -> Result<(), String> {
        let key = f.key().clone();
        let value = f.value().clone();
        let value_size = elem_size(&value, &self.layouts).ok_or_else(|| {
            unsupported(&format!(
                "the `Map<{}, {}>` behind `{}` (the value type has no size this backend can 
                 allocate)",
                key.display(),
                value.display(),
                f.label()
            ))
        })?;
        let entry = map_entry(&key, &self.layouts).ok_or_else(|| {
            unsupported(&format!(
                "the `Map<{}, {}>` behind `{}` (the key type has no size this backend can lay 
                 out)",
                key.display(),
                value.display(),
                f.label()
            ))
        })?;
        let value_class = elem_class(&value, &self.layouts);
        let k_is_str = key == Ty::String;
        let label = f.label();
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        let frame = 112;
        let slot_self = -8;
        let slot_key = -16;
        let slot_found = -24;
        let slot_cap = -32;
        let slot_i = -40;
        let slot_j = -48;
        let slot_tomb = -56;
        let slot_ret = -64;
        let slot_e = -72;
        let slot_w = -80;
        let slot_v = -112;
        let prologue = format!("    push rbp\n    mov rbp, rsp\n    sub rsp, {frame}\n");
        let epilogue = "    mov rsp, rbp\n    pop rbp\n    ret\n";
        let find = MapFn::Find(key.clone(), value.clone()).label();
        let value_drop = self.drop_site_for(&value);
        let entry_size = entry.size;
        let value_offset = entry.value_offset;
        let live_offset = entry.live_offset;
        match f {
            // `m.entries = NULL; ... m.index_used = 0; return m;` — the map is
            // `MEMORY` class, so `rdi` is the hidden destination.
            MapFn::New(..) => {
                self.out.push_str(
                    "    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+8], 0\n\
                     mov QWORD PTR [rdi+16], 0\n    mov QWORD PTR [rdi+24], 0\n\
                     mov QWORD PTR [rdi+32], 0\n    mov QWORD PTR [rdi+40], 0\n\
                     mov QWORD PTR [rdi+48], 0\n    mov rax, rdi\n    ret\n",
                );
            }
            MapFn::Len(..) => {
                self.out.push_str("    mov rax, QWORD PTR [rdi+24]\n    ret\n");
            }
            MapFn::IsEmpty(..) => {
                self.out
                    .push_str("    cmp QWORD PTR [rdi+24], 0\n    sete al\n    movzx eax, al\n    ret\n");
            }
            // `*found = false; if (self->index_cap == 0) return 0;
            //  mask = index_cap - 1; i = hash(k) & mask; tomb = SIZE_MAX;
            //  for (;;) { slot = index[i]; if (slot == 0) return tomb != SIZE_MAX
            //  ? tomb : i; if (slot == SIZE_MAX) { if (tomb == SIZE_MAX) tomb =
            //  i; } else if (entries[slot-1].live && key_eq(...)) { *found =
            //  true; return i; } i = (i + 1) & mask; }`
            MapFn::Find(..) => {
                let zero = self.label("map_find_zero");
                let loop_ = self.label("map_find_loop");
                let empty = self.label("map_find_empty");
                let tomb = self.label("map_find_tomb");
                let next = self.label("map_find_next");
                let hit = self.label("map_find_hit");
                let ret = self.label("map_find_ret");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_found}], rdx");
                self.out.push_str("    mov BYTE PTR [rdx], 0\n");
                self.out.push_str("    mov rax, QWORD PTR [rdi+40]\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {zero}");
                self.out.push_str("    sub rax, 1\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_cap}], rax");
                self.emit_map_hash_arg(&key, slot_key);
                let _ = writeln!(self.out, "    and rax, QWORD PTR [rbp{slot_cap}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                self.out.push_str("    mov rax, -1\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_tomb}], rax");
                let _ = writeln!(self.out, "{loop_}:");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    mov rcx, QWORD PTR [rcx+32]\n");
                self.out.push_str("    mov rcx, QWORD PTR [rcx+rdx*8]\n    test rcx, rcx\n");
                let _ = writeln!(self.out, "    je {empty}");
                self.out.push_str("    cmp rcx, -1\n");
                let _ = writeln!(self.out, "    je {tomb}");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdx, QWORD PTR [rdx]\n    sub rcx, 1\n");
                let _ = writeln!(self.out, "    imul rcx, {entry_size}");
                self.out.push_str("    add rdx, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rdx");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "    je {next}");
                self.emit_map_key_eq(&key, slot_e, slot_key);
                self.out.push_str("    test eax, eax\n");
                let _ = writeln!(self.out, "    jne {hit}");
                let _ = writeln!(self.out, "{next}:");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    add rcx, 1\n");
                let _ = writeln!(self.out, "    and rcx, QWORD PTR [rbp{slot_cap}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rcx");
                let _ = writeln!(self.out, "    jmp {loop_}");
                let _ = writeln!(self.out, "{tomb}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_tomb}]");
                self.out.push_str("    cmp rax, -1\n");
                let _ = writeln!(self.out, "    jne {next}");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_tomb}], rax");
                let _ = writeln!(self.out, "    jmp {next}");
                let _ = writeln!(self.out, "{empty}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_tomb}]");
                self.out.push_str("    cmp rax, -1\n");
                let _ = writeln!(self.out, "    jne {ret}");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "    jmp {ret}");
                let _ = writeln!(self.out, "{hit}:");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_found}]");
                self.out.push_str("    mov BYTE PTR [rdx], 1\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                let _ = writeln!(self.out, "{ret}:");
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{zero}:");
                self.out.push_str("    xor eax, eax\n");
                self.out.push_str(epilogue);
            }
            // `for (e = 0; e < entry_count; e++) if (live) { i = hash(key) &
            //  (cap-1); while (index[i]) i = (i+1) & (cap-1);
            //  index[i] = e+1; index_used++; }`, after rebuilding the index at
            // `cap = max(8, smallest power of two >= want*4)`.
            MapFn::Rehash(..) => {
                let grow = self.label("map_rehash_grow");
                let have = self.label("map_rehash_have");
                let oom = self.label("map_rehash_oom");
                let loop_ = self.label("map_rehash_loop");
                let probe = self.label("map_rehash_probe");
                let place = self.label("map_rehash_place");
                let next = self.label("map_rehash_next");
                let done = self.label("map_rehash_done");
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_w}], rsi");
                self.out.push_str("    mov rax, 8\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_cap}], rax");
                self.out.push_str("    mov rcx, rsi\n    shl rcx, 2\n");
                let _ = writeln!(self.out, "{grow}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    cmp rax, rcx\n");
                let _ = writeln!(self.out, "    jae {have}");
                self.out.push_str("    add rax, rax\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_cap}], rax");
                let _ = writeln!(self.out, "    jmp {grow}");
                let _ = writeln!(self.out, "{have}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [rdi+32]\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    shl rdi, 3\n    call malloc@PLT\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [rcx+32], rax\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {oom}");
                self.out.push_str("    mov rdi, rax\n    xor esi, esi\n");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    shl rdx, 3\n    call memset@PLT\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    mov QWORD PTR [rcx+40], rax\n    mov QWORD PTR [rcx+48], 0\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], 0");
                let _ = writeln!(self.out, "{loop_}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    cmp rcx, rax\n");
                let _ = writeln!(self.out, "    jae {done}");
                self.out.push_str("    mov rdx, QWORD PTR [rdi]\n");
                let _ = writeln!(self.out, "    imul rcx, {entry_size}");
                self.out.push_str("    add rdx, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rdx");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "    je {next}");
                self.emit_map_hash_stored(&key, slot_e);
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    sub rcx, 1\n    and rax, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_j}], rax");
                let _ = writeln!(self.out, "{probe}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [rdi+32]\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_j}]");
                self.out.push_str("    cmp QWORD PTR [rdi+rax*8], 0\n");
                let _ = writeln!(self.out, "    je {place}");
                self.out.push_str("    add rax, 1\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    sub rcx, 1\n    and rax, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_j}], rax");
                let _ = writeln!(self.out, "    jmp {probe}");
                let _ = writeln!(self.out, "{place}:");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    add rcx, 1\n    mov QWORD PTR [rdi+rax*8], rcx\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    add QWORD PTR [rdi+48], 1\n");
                let _ = writeln!(self.out, "{next}:");
                let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_i}], 1");
                let _ = writeln!(self.out, "    jmp {loop_}");
                let _ = writeln!(self.out, "{done}:");
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{oom}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // `if (index_cap == 0 || (index_used + 1) * 4 >= index_cap * 3)
            //  rehash(self, len + 1); slot = find(self, k, &found);
            //  if (found) { e = index[slot]-1; drop(*entries[e].value);
            //  *entries[e].value = v; return; }
            //  if (entry_count == entry_cap) grow entries;
            //  e = entry_count++; entries[e].key = store_key(k);
            //  entries[e].value = malloc(sizeof(V)); *entries[e].value = v;
            //  entries[e].live = 1; if (index[slot] == 0) index_used++;
            //  index[slot] = e + 1; len++;` — `write_map`'s insert, with the
            // value staged in the frame before the first possible `malloc`,
            // because the register it arrived in is caller-saved.
            MapFn::Insert(..) => {
                let rehash = MapFn::Rehash(key.clone(), value.clone()).label();
                let rehash_needed = self.label("map_insert_rehash");
                let after_rehash = self.label("map_insert_after_rehash");
                let fresh = self.label("map_insert_fresh");
                let have_cap = self.label("map_insert_have_cap");
                let double = self.label("map_insert_double");
                let got_cap = self.label("map_insert_got_cap");
                let no_used = self.label("map_insert_no_used");
                let done = self.label("map_insert_done");
                let oom = self.label("map_insert_oom");
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                // Stage the value's own registers into `slot_v` (a
                // `MEMORY`-class value is already in the caller's outgoing
                // argument area at `[rbp+16]`). The integer file starts at
                // `rdx` — `rdi` is the receiver, `rsi` the key — while the
                // vector file starts at `xmm0`, since the two are numbered
                // independently.
                if !value_class.is_mem() {
                    self.store_arg_ebs(value_class, slot_v, 2, 0);
                }
                // Capacity check: `index_cap == 0` or the load factor would
                // exceed three quarters on the next insert.
                self.out.push_str("    mov rax, QWORD PTR [rdi+40]\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {rehash_needed}");
                self.out.push_str("    mov rcx, QWORD PTR [rdi+48]\n    add rcx, 1\n    shl rcx, 2\n");
                self.out.push_str("    lea rdx, [rax+rax*2]\n    cmp rcx, rdx\n");
                let _ = writeln!(self.out, "    jae {rehash_needed}");
                let _ = writeln!(self.out, "    jmp {after_rehash}");
                let _ = writeln!(self.out, "{rehash_needed}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rsi, QWORD PTR [rdi+24]\n    add rsi, 1\n");
                let _ = writeln!(self.out, "    call {rehash}");
                let _ = writeln!(self.out, "{after_rehash}:");
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    je {fresh}");
                // Existing key: replace the value in place.
                self.emit_map_entry_addr(entry_size, slot_self, slot_i, slot_e);
                if let Some(site) = &value_drop {
                    let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rdx+{value_offset}]");
                    let _ = writeln!(self.out, "    call {}", site.label());
                }
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov r10, QWORD PTR [rdx+{value_offset}]");
                self.emit_map_store_value(value_size, value_class, "r10", slot_v);
                let _ = writeln!(self.out, "    jmp {done}");
                let _ = writeln!(self.out, "{fresh}:");
                // Grow the dense entries array when it is full.
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                self.out.push_str("    cmp rax, QWORD PTR [rdi+16]\n");
                let _ = writeln!(self.out, "    jne {have_cap}");
                self.out.push_str("    mov rax, QWORD PTR [rdi+16]\n    test rax, rax\n");
                let _ = writeln!(self.out, "    jne {double}");
                self.out.push_str("    mov eax, 8\n");
                let _ = writeln!(self.out, "    jmp {got_cap}");
                let _ = writeln!(self.out, "{double}:");
                self.out.push_str("    add rax, rax\n");
                let _ = writeln!(self.out, "{got_cap}:");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_cap}], rax");
                self.out.push_str("    mov rsi, rax\n");
                let _ = writeln!(self.out, "    imul rsi, {entry_size}");
                self.out.push_str("    mov rdi, QWORD PTR [rdi]\n    call realloc@PLT\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov QWORD PTR [rcx], rax\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {oom}");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_cap}]");
                self.out.push_str("    mov QWORD PTR [rcx+16], rax\n");
                let _ = writeln!(self.out, "{have_cap}:");
                // `e = entry_count++`, then the entry's own address.
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n    lea rcx, [rax+1]\n");
                self.out.push_str("    mov QWORD PTR [rdi+8], rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_w}], rax");
                self.out.push_str("    mov rdx, QWORD PTR [rdi]\n");
                let _ = writeln!(self.out, "    imul rax, {entry_size}");
                self.out.push_str("    add rdx, rax\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rdx");
                self.emit_map_store_key(&key, slot_e, slot_key);
                let _ = writeln!(self.out, "    mov edi, {value_size}");
                self.out.push_str("    call malloc@PLT\n    test rax, rax\n");
                let _ = writeln!(self.out, "    je {oom}");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rdx+{value_offset}], rax");
                let _ = writeln!(self.out, "    mov r10, rax");
                self.emit_map_store_value(value_size, value_class, "r10", slot_v);
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov BYTE PTR [rdx+{live_offset}], 1");
                // `if (index[slot] == 0) index_used++; index[slot] = e + 1;`
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rcx, QWORD PTR [rcx+32]\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    cmp QWORD PTR [rcx+rax*8], 0\n");
                let _ = writeln!(self.out, "    jne {no_used}");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    add QWORD PTR [rdx+48], 1\n");
                let _ = writeln!(self.out, "{no_used}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_w}]");
                self.out.push_str("    add rax, 1\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rcx, QWORD PTR [rcx+32]\n");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    mov QWORD PTR [rcx+rdx*8], rax\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    add QWORD PTR [rcx+24], 1\n");
                let _ = writeln!(self.out, "{done}:");
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{oom}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // `bool found = false; find(self, k, &found); return found;`
            MapFn::Has(..) => {
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    movzx eax, BYTE PTR [rbp{slot_found}]");
                self.out.push_str(epilogue);
            }
            // `find(self, k, &found); if (!found) panic("Map.get: key not
            //  found"); return *entries[index[slot]-1].value;` — the by-value
            // form, for a `Copy` (scalar) value, so the value itself comes back
            // in `rax`.
            MapFn::Get(_, _, true) => {
                let missing = self.label("map_get_missing");
                let msg = self.bytes_label(b"Map.get: key not found");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    je {missing}");
                self.emit_map_entry_addr(entry_size, slot_self, slot_i, slot_e);
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rdx+{value_offset}]");
                self.emit_map_value_out(
                    &value,
                    value_size,
                    value_class,
                    "r11",
                    slot_v,
                    slot_ret,
                );
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{missing}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // The borrowing form: `return entries[index[slot]-1].value;`
            MapFn::Get(_, _, false) => {
                let missing = self.label("map_get_missing");
                let msg = self.bytes_label(b"Map.get: key not found");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    je {missing}");
                self.emit_map_entry_addr(entry_size, slot_self, slot_i, slot_e);
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rdx+{value_offset}]");
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{missing}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // `find(...); if (!found) return fallback;
            //  existing = entries[index[slot]-1].value; result = copy(existing);
            //  drop(fallback); return result;` — only for a clonable value
            // (`infer.rs` never offers it otherwise, and `Gen::call` refuses
            // the call by name if the two ever disagreed).
            MapFn::GetOr(..) => {
                let absent = self.label("map_get_or_absent");
                self.out.push_str(&prologue);
                if value_class.is_mem() {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rsi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rdx");
                } else {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_v}], rdx");
                }
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    je {absent}");
                self.emit_map_entry_addr(entry_size, slot_self, slot_i, slot_e);
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rdx+{value_offset}]");
                match value {
                    Ty::String => {
                        let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                        self.out.push_str("    mov rsi, r11\n");
                        let _ = writeln!(self.out, "    call {}", Routine::StringClone.label());
                        let _ = writeln!(self.out, "    lea rdi, [rbp+16]");
                        let _ = writeln!(self.out, "    call {}", Routine::StringDrop.label());
                        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_ret}]");
                    }
                    Ty::Bytes => {
                        let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                        self.out.push_str("    mov rsi, r11\n");
                        let _ = writeln!(self.out, "    call {}", Routine::BytesClone.label());
                        let _ = writeln!(self.out, "    lea rdi, [rbp+16]");
                        let _ = writeln!(self.out, "    call {}", Routine::BytesDrop.label());
                        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_ret}]");
                    }
                    _ => {
                        // A `Copy` scalar: `*existing`, with no drop for the
                        // fallback (`(void)fallback`).
                        let (w, signed) =
                            int_ty(&value).expect("a clonable non-`String` value is a scalar");
                        let _ = writeln!(self.out, "    mov {}, {} [r11]", w.rax(), w.mem());
                        self.out.push_str(&w.extend(signed));
                    }
                }
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{absent}:");
                if value_class.is_mem() {
                    let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot_ret}]");
                    emit_copy(&mut self.out, "r10", 0, "rbp", 16, value_size);
                    self.out.push_str("    mov rax, r10\n");
                } else {
                    let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_v}]");
                }
                self.out.push_str(epilogue);
            }
            // `find(...); if (!found) panic("Map.remove: key not found");
            //  v = *entries[e].value; free(entries[e].value);
            //  entries[e].value = NULL; drop_key(entries[e].key);
            //  entries[e].live = 0; index[slot] = SIZE_MAX; len--; return v;`
            MapFn::Remove(..) => {
                let missing = self.label("map_remove_missing");
                let msg = self.bytes_label(b"Map.remove: key not found");
                self.out.push_str(&prologue);
                if value_class.is_mem() {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rsi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rdx");
                } else {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                    let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_key}], rsi");
                }
                let _ = writeln!(self.out, "    mov BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_key}]");
                let _ = writeln!(self.out, "    lea rdx, [rbp{slot_found}]");
                let _ = writeln!(self.out, "    call {find}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], rax");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rbp{slot_found}], 0");
                let _ = writeln!(self.out, "    je {missing}");
                self.emit_map_entry_addr(entry_size, slot_self, slot_i, slot_e);
                // `v = *entries[e].value` — staged in `slot_v` (or written
                // straight to the caller's return slot for a `MEMORY`-class
                // value) *before* the `free`, because both `free` and the key's
                // own drop are entitled to clobber `rax`/`rdx`.
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rdx+{value_offset}]");
                // Stage the value before the `free`: a `MEMORY`-class one
                // straight to the caller's return slot (it is wider than the
                // routine's own staging block), every other class as a byte
                // copy into `slot_v`, because both `free` and the key's own
                // drop are entitled to clobber the registers it arrived in.
                if value_class.is_mem() {
                    self.emit_map_value_out(
                        &value,
                        value_size,
                        value_class,
                        "r11",
                        slot_v,
                        slot_ret,
                    );
                } else {
                    emit_copy(&mut self.out, "rbp", slot_v, "r11", 0, value_size);
                }
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rdx+{value_offset}]");
                self.out.push_str("    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rdx+{value_offset}], 0");
                if k_is_str {
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_e}]");
                    let _ = writeln!(self.out, "    call {}", Routine::StringDrop.label());
                }
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rcx, QWORD PTR [rcx+32]\n");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    mov QWORD PTR [rcx+rax*8], -1\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    sub QWORD PTR [rcx+24], 1\n");
                match value_class {
                    AggClass::Mem => {
                        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_ret}]");
                    }
                    _ => {
                        let _ = writeln!(self.out, "    lea r11, [rbp{slot_v}]");
                        self.emit_elem_return(&value, value_size, value_class, "r11", slot_ret);
                    }
                }
                self.out.push_str(epilogue);
                let _ = writeln!(self.out, "{missing}:");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // `for (e = 0; e < entry_count; e++) if (live) { drop(value);
            //  free(value); value = NULL; drop_key(key); live = 0; }
            //  entry_count = 0; len = 0; index_used = 0;
            //  memset(index, 0, index_cap * sizeof(size_t));`
            MapFn::Clear(..) => {
                let loop_ = self.label("map_clear_loop");
                let skip = self.label("map_clear_skip");
                let skip_memset = self.label("map_clear_memset_skip");
                let done = self.label("map_clear_done");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], 0");
                let _ = writeln!(self.out, "{loop_}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    cmp rcx, rax\n");
                let _ = writeln!(self.out, "    jae {done}");
                self.out.push_str("    mov rdx, QWORD PTR [rdi]\n");
                let _ = writeln!(self.out, "    imul rcx, {entry_size}");
                self.out.push_str("    add rdx, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rdx");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "    je {skip}");
                if let Some(site) = &value_drop {
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rdx+{value_offset}]");
                    let _ = writeln!(self.out, "    call {}", site.label());
                }
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rdx+{value_offset}]");
                self.out.push_str("    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov QWORD PTR [rdx+{value_offset}], 0");
                if k_is_str {
                    let _ = writeln!(self.out, "    mov rdi, rdx");
                    let _ = writeln!(self.out, "    call {}", Routine::StringDrop.label());
                }
                let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                let _ = writeln!(self.out, "    mov BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "{skip}:");
                let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_i}], 1");
                let _ = writeln!(self.out, "    jmp {loop_}");
                let _ = writeln!(self.out, "{done}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str(
                    "    mov QWORD PTR [rdi+8], 0\n    mov QWORD PTR [rdi+24], 0\n\
                     mov QWORD PTR [rdi+48], 0\n",
                );
                self.out.push_str("    mov rdi, QWORD PTR [rdi+32]\n    test rdi, rdi\n");
                let _ = writeln!(self.out, "    je {skip_memset}");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdx, QWORD PTR [rcx+40]\n    shl rdx, 3\n    xor esi, esi\n");
                self.out.push_str("    call memset@PLT\n");
                let _ = writeln!(self.out, "{skip_memset}:");
                self.out.push_str(epilogue);
            }
            // `out = <List<K>>__empty(); <List<K>>__reserve(&out, self->len);
            //  for (e ...) if (live) out.data[out.len++] = clone(key);
            //  return out;` — the list is built straight in the caller's return
            // destination (`MEMORY` class), so no list local is needed.
            MapFn::Keys(..) | MapFn::Values(..) => {
                let is_keys = matches!(f, MapFn::Keys(..));
                let elem = if is_keys { key.clone() } else { value.clone() };
                let elem_bytes = elem_size(&elem, &self.layouts).ok_or_else(|| {
                    unsupported(&format!(
                        "the `Map<{}, {}>` list element `{}`",
                        key.display(),
                        value.display(),
                        elem.display()
                    ))
                })?;
                let empty = ListFn::Empty(elem.clone()).label();
                let reserve = Routine::ListReserve.label();
                let loop_ = self.label("map_list_loop");
                let next = self.label("map_list_next");
                let done = self.label("map_list_done");
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_ret}], rdi");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rsi");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                let _ = writeln!(self.out, "    call {empty}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rsi, QWORD PTR [rcx+24]\n");
                let _ = writeln!(self.out, "    mov edx, {elem_bytes}");
                let _ = writeln!(self.out, "    call {reserve}");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_i}], 0");
                let _ = writeln!(self.out, "{loop_}:");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    mov rcx, QWORD PTR [rbp{slot_i}]");
                self.out.push_str("    cmp rcx, rax\n");
                let _ = writeln!(self.out, "    jae {done}");
                self.out.push_str("    mov rdx, QWORD PTR [rdi]\n");
                let _ = writeln!(self.out, "    imul rcx, {entry_size}");
                self.out.push_str("    add rdx, rcx\n");
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_e}], rdx");
                let _ = writeln!(self.out, "    cmp BYTE PTR [rdx+{live_offset}], 0");
                let _ = writeln!(self.out, "    je {next}");
                // dest = out.data + out.len * elem_size
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str("    mov rcx, QWORD PTR [rdi+8]\n");
                let _ = writeln!(self.out, "    imul rcx, {elem_bytes}");
                self.out.push_str("    add rcx, QWORD PTR [rdi]\n");
                if elem == Ty::String {
                    if is_keys {
                        // The entry's key is at offset 0.
                        let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rbp{slot_e}]");
                    } else {
                        // The value is the pointer at `value_offset`.
                        let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                        let _ = writeln!(
                            self.out,
                            "    mov rsi, QWORD PTR [rdx+{value_offset}]"
                        );
                    }
                    self.out.push_str("    mov rdi, rcx\n");
                    let _ = writeln!(self.out, "    call {}", Routine::StringClone.label());
                } else if elem == Ty::Bytes {
                    // `values()` over a `Bytes` value clones the buffer.
                    let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                    let _ = writeln!(self.out, "    mov rsi, QWORD PTR [rdx+{value_offset}]");
                    self.out.push_str("    mov rdi, rcx\n");
                    let _ = writeln!(self.out, "    call {}", Routine::BytesClone.label());
                } else {
                    // A scalar element (a `String` key's clone is above, and
                    // for `keys()` that is the only non-scalar case).
                    let (w, _) = int_ty(&elem).ok_or_else(|| {
                        unsupported(&format!(
                            "the `Map<{}, {}>` list element `{}`",
                            key.display(),
                            value.display(),
                            elem.display()
                        ))
                    })?;
                    let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rbp{slot_e}]");
                    if is_keys {
                        let _ = writeln!(self.out, "    mov {}, {} [rdx]", w.rax(), w.mem());
                    } else {
                        let _ = writeln!(self.out, "    mov rdx, QWORD PTR [rdx+{value_offset}]");
                        let _ = writeln!(self.out, "    mov {}, {} [rdx]", w.rax(), w.mem());
                    }
                    let _ = writeln!(self.out, "    mov {} [rcx], {}", w.mem(), w.rax());
                }
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str("    add QWORD PTR [rdi+8], 1\n");
                let _ = writeln!(self.out, "{next}:");
                let _ = writeln!(self.out, "    add QWORD PTR [rbp{slot_i}], 1");
                let _ = writeln!(self.out, "    jmp {loop_}");
                let _ = writeln!(self.out, "{done}:");
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot_ret}]");
                self.out.push_str(epilogue);
            }
            // `clear(self); free(self->entries); free(self->index);
            //  self->entries = NULL; ... self->index_used = 0;` — `clear`
            // already dropped every live value and key (in that order, entry by
            // entry) and reset the counters, so the drop is those two frees and
            // the header zeroing, exactly `write_map`'s order.
            MapFn::Drop(..) => {
                let clear = MapFn::Clear(key.clone(), value.clone()).label();
                self.out.push_str(&prologue);
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot_self}], rdi");
                let _ = writeln!(self.out, "    call {clear}");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [rdi]\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                self.out.push_str("    mov rdi, QWORD PTR [rdi+32]\n    call free@PLT\n");
                let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot_self}]");
                for off in [0, 8, 16, 24, 32, 40, 48] {
                    let _ = writeln!(self.out, "    mov QWORD PTR [rdi+{off}], 0");
                }
                self.out.push_str(epilogue);
            }
        }
        Ok(())
    }

    /// Records that a routine, and everything it calls, has to be emitted.
    fn require_routine(&mut self, r: Routine) {
        match r {
            Routine::BytesPush
            | Routine::BytesAppend
            | Routine::StringPush
            | Routine::StringPushStr => {
                self.routines.insert(Routine::BufReserve);
            }
            // `with_capacity` needs the reserve for a real capacity, and
            // `with_capacity(0)` fills its empty `String` by calling
            // `BytesEmpty`, which builds the same `{malloc(1), 0, 0}` value.
            // This is the only place that dependency can be declared: doing it
            // from inside `emit_routine` was too late, because the set of
            // routines to emit is snapshotted before the first one is written,
            // so the call linked against a symbol nothing emitted.
            Routine::StringWithCapacity => {
                self.routines.insert(Routine::BufReserve);
                self.routines.insert(Routine::BytesEmpty);
            }
            Routine::BytesAsStr | Routine::StringFromBytes => {
                self.routines.insert(Routine::Utf8Valid);
            }
            Routine::StrFromF64Raw => {
                self.routines.insert(Routine::Utf8Valid);
                self.routines.insert(Routine::BytesAllocRaw);
            }
            Routine::FmtBytesRaw => {
                self.routines.insert(Routine::Utf8Valid);
                self.routines.insert(Routine::BytesAllocRaw);
                self.routines.insert(Routine::StringEmpty);
            }
            _ => {}
        }
        self.routines.insert(r);
    }

    /// Parks the return value a drop call would otherwise clobber — `xmm0`
    /// when `xmm` is `Some`, `rax` otherwise — in a scratch slot. This is the
    /// asm side of `codegen_c.rs::write_temp_then_drops`: a value produced
    /// before a drop has to be moved somewhere the drop cannot reach.
    fn park(&mut self, xmm: Option<FWidth>, frame: &mut Frame) -> Parked {
        let slot = frame.scratch(8);
        match xmm {
            Some(w) => {
                let _ = writeln!(self.out, "    {} QWORD PTR [rbp{slot}], xmm0", w.mov());
            }
            None => {
                let _ = writeln!(self.out, "    mov QWORD PTR [rbp{slot}], rax");
            }
        }
        Parked { slot, xmm }
    }

    fn unpark(&mut self, p: Parked) {
        match p.xmm {
            Some(w) => {
                let _ = writeln!(self.out, "    {} xmm0, QWORD PTR [rbp{}]", w.mov(), p.slot);
            }
            None => {
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{}]", p.slot);
            }
        }
    }

    /// An owned `Bytes`/`String`/`datac` argument passed as a *borrow* (`&b`,
    /// `&s`, `&holder`) whose inner expression is a temporary —
    /// `b.append(Bytes.from_str("de"))`, `f(String.from_str("x"))`,
    /// `show(Holder(name = String.from_str("h"), n = 1))` — needs storage its
    /// address can be taken of. It gets a fresh scratch slot built through
    /// `eval_struct_into` and a `lea` of that slot; the caller records the slot
    /// and the drop so the temporary is freed right after the call, which is
    /// what `codegen_c.rs::call_value` does by hoisting the same temporary into
    /// a C variable. `None` for a borrow of a place (`&b`, addressed directly),
    /// for a temporary that owns nothing (a `datac` of scalars — the `Ref` arm
    /// in `expr` stages that one), and for anything not an owned aggregate.
    fn hoist_owned_temporary(
        &mut self,
        arg: &Expr,
        frame: &mut Frame,
    ) -> Result<Option<(i32, DropSite)>, String> {
        let ExprKind::Ref { expr: inner, .. } = &arg.kind else {
            return Ok(None);
        };
        let site = match &inner.ty {
            Ty::Bytes => DropSite::Routine(Routine::BytesDrop),
            Ty::String => DropSite::Routine(Routine::StringDrop),
            // A `datac` temporary is staged here only when it owns something:
            // an owning one has to be freed after the call, and a non-owning
            // one is what `expr`'s `Ref` arm already stages with no drop
            // (`is_place` below never sees it, so the two paths cannot
            // disagree about who stages it). MK7 §5's enum slice reaches this
            // arm too, and the site is picked from the layout for the same
            // reason `drop_site_for` picks its own: the symbol is identical,
            // the body is not.
            Ty::Struct(name) if self.datac_owns(name) => {
                self.require_aggregate_drop(name);
                if self.layouts.get(name).is_some_and(|l| l.variants.is_some()) {
                    DropSite::Enum(name.clone())
                } else {
                    DropSite::Datac(name.clone())
                }
            }
            // A `Box` temporary is the same shape one pointer wide: `f(Box.of(1))`
            // where `f` takes the box by borrow has to free the box — and the
            // element with it — after the call. An element this backend cannot
            // allocate is refused by name rather than staged without a drop,
            // which is the same refusal `Gen::call`'s `Box#` dispatch makes.
            Ty::Box(elem) => {
                if elem_size(elem, &self.layouts).is_none() {
                    return Err(unsupported(&format!(
                        "a borrow of the temporary `Box<{}>`",
                        elem.display()
                    )));
                }
                self.require_box_fn(BoxFn::Drop((**elem).clone()));
                DropSite::Box((**elem).clone())
            }
            // MK7 §5's list slice: the same shape as the box above, one
            // aggregate wider — `show(List.of(1))` where `show` takes the list
            // by borrow has to free the list's buffer (and everything in it)
            // after the call. A list always owns its buffer, so this arm is
            // reached for *every* element type; what can still be refused is the
            // element's own size, and it is refused by name rather than staged
            // without a drop.
            Ty::List(elem) => {
                if elem_size(elem, &self.layouts).is_none() {
                    return Err(unsupported(&format!(
                        "a borrow of the temporary `List<{}>`",
                        elem.display()
                    )));
                }
                self.require_list_fn(ListFn::Drop((**elem).clone()));
                DropSite::List((**elem).clone())
            }
            // MK7 §5's map slice: the same shape as the list above, one
            // aggregate wider — `f(Map.new())` where `f` takes the map by
            // borrow has to free both of its arrays (and everything in them)
            // after the call. A map always owns, so this arm is reached for
            // every key and value type; what can still be refused is either
            // type's own size, refused by name rather than staged without a
            // drop.
            Ty::Map(key, value) => {
                if elem_size(key, &self.layouts).is_none()
                    || elem_size(value, &self.layouts).is_none()
                {
                    return Err(unsupported(&format!(
                        "a borrow of the temporary `Map<{}, {}>`",
                        key.display(),
                        value.display()
                    )));
                }
                self.require_map_fn(MapFn::Drop((**key).clone(), (**value).clone()));
                DropSite::Map((**key).clone(), (**value).clone())
            }
            _ => return Ok(None),
        };
        if is_place(inner) {
            return Ok(None);
        }
        let layout = struct_layout(&inner.ty, &self.layouts)
            .expect("an owned aggregate has a layout");
        let slot = frame.scratch(align_up(layout.size, 8));
        self.eval_struct_into(inner, frame, Dest::Direct(slot))?;
        let _ = writeln!(self.out, "    lea rax, [rbp{slot}]");
        Ok(Some((slot, site)))
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
                    if !layout.class().is_int_one() {
                        // Staged in a scratch slot and copied home *after* the
                        // name is declared, not written straight into the
                        // local's own slot: an initializer that names the
                        // binding being declared — which only a shadowing
                        // declaration can do, since a local is not in scope in
                        // its own initializer — would otherwise read the
                        // not-yet-initialized slot. The scalar path below
                        // evaluates first for the same reason.
                        let size = layout.size;
                        let slot = frame.scratch(align_up(size, 8));
                        self.eval_struct_into(init, frame, Dest::Direct(slot))?;
                        let loc = frame.declare(name, ty, &self.layouts)?;
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        emit_copy(&mut self.out, "rbp", off, "rbp", slot, size);
                        return Ok(());
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
            Stmt::Return { value, span } => {
                // MK7 §5 item 5: the drops `borrowck.rs` keyed by this
                // `return`'s span run *after* the value is produced, so an
                // owned local other than the value being moved out is freed on
                // this path — the shape `codegen_c.rs` writes as
                // `tmp = value; drops; return tmp;`.
                let drops = self.drops.at_return(&self.function, *span).to_vec();
                let parked = match value {
                    Some(v) => match frame.ret_class {
                        RetClass::Agg(class) if !class.is_mem() => {
                            let agg = frame.agg_slot;
                            self.eval_struct_into(v, frame, Dest::Direct(agg))?;
                            None
                        }
                        RetClass::Agg(_) => {
                            let rp = frame.ret_ptr_slot;
                            self.eval_struct_into(v, frame, Dest::Indirect(rp))?;
                            None
                        }
                        _ => {
                            self.expr(v, frame)?;
                            if drops.is_empty() {
                                None
                            } else {
                                Some(self.park(float_ty(&v.ty), frame))
                            }
                        }
                    },
                    None => {
                        self.out.push_str("    xor eax, eax\n");
                        None
                    }
                };
                for t in &drops {
                    self.emit_drop(t, frame)?;
                }
                if let Some(p) = parked {
                    self.unpark(p);
                }
                let label = self.return_label.clone();
                let _ = writeln!(self.out, "    jmp {label}");
                Ok(())
            }
            Stmt::Expr(e) => self.expr(e, frame),
        }
    }

    /// A block in statement position. Its tail, if any, is computed for its
    /// effects and discarded — only a function body's tail is a return — and
    /// then the block's own owned locals are freed (MK7 §5 item 5): the
    /// points `borrowck.rs` keyed by this block's identity, in reverse
    /// declaration order. A block in *value* position is the same code — the
    /// tail it leaves in `rax`/`xmm0` is parked across the drops, exactly as
    /// the C backend's `write_temp_then_drops` evaluates into a temporary
    /// first.
    fn block(&mut self, block: &Block, frame: &mut Frame) -> Result<(), String> {
        frame.push_scope();
        for stmt in &block.stmts {
            self.stmt(stmt, frame)?;
        }
        let tail_ty = block.tail.as_ref().map(|t| t.ty.clone());
        if let Some(tail) = &block.tail {
            self.expr(tail, frame)?;
        }
        let drops = self.drops.at_block_end(&self.function, block).to_vec();
        if !drops.is_empty() {
            let parked = tail_ty.as_ref().map(|ty| self.park(float_ty(ty), frame));
            for t in &drops {
                self.emit_drop(t, frame)?;
            }
            if let Some(p) = parked {
                self.unpark(p);
            }
        }
        frame.pop_scope();
        Ok(())
    }

    /// A block in *value* position whose value is wider than one eightbyte
    /// (MK7 §5's enum slice): the same walk `block` does, except that the tail is
    /// written straight to `dest` through `eval_struct_into` and the block's own
    /// drops still run afterwards — the ordering the C backend's
    /// statement-expression gives the same shape. A value that leaves in a
    /// register has to be parked across the drops (what `block` does); one that
    /// leaves in caller-owned memory does not, which is the whole reason this is
    /// a second function rather than a flag on the first.
    fn block_into(&mut self, block: &Block, frame: &mut Frame, dest: Dest) -> Result<(), String> {
        frame.push_scope();
        for stmt in &block.stmts {
            self.stmt(stmt, frame)?;
        }
        if let Some(tail) = &block.tail {
            self.eval_struct_into(tail, frame, dest)?;
        }
        let drops = self.drops.at_block_end(&self.function, block).to_vec();
        for t in &drops {
            self.emit_drop(t, frame)?;
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
            // MK7 §5 item 8: a `str` is a pointer to NUL-terminated bytes, and
            // the literal's own bytes are already interned as one — the same
            // `.byte` run `Gen::bytes_label` gives a format string, so a
            // literal still never goes through a C string escape a second
            // time.
            ExprKind::StringLit(s) => {
                let label = self.bytes_label(s.as_bytes());
                let _ = writeln!(self.out, "    lea rax, [rip+{label}]");
                Ok(())
            }
            ExprKind::CharLit(cp) => {
                let _ = writeln!(self.out, "    mov eax, {cp}");
                Ok(())
            }
            ExprKind::Ident(name) => {
                // MK7 §5 (the passdown slice): a name that is not a local is a
                // passdown's generated storage. `pd#x` cannot be a user
                // identifier, so this cannot capture anything, and the
                // parameter case never reaches here — a function that receives
                // the passdown has it as an ordinary local, which is the lookup
                // above.
                let Some((loc, ty)) = frame.lookup(name) else {
                    let index = self
                        .pd_global(name)
                        .ok_or_else(|| unsupported(&format!("a read of the name `{name}`")))?;
                    let pd = &self.pds[index];
                    let label = pd.label.clone();
                    if pd.str_borrow() {
                        // A `str` passdown's storage *is* the pointer, so the
                        // name means the pointer's value — which is also the
                        // borrow `infer.rs` hands over (`&str` and `str` are
                        // the same `const char*`).
                        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rip+{label}]");
                    } else {
                        // Every other passdown is reached through its address:
                        // `infer::PdInfo::arg` wraps this name in a `Ref` when
                        // the global is what it names, and a `Ref` of the
                        // storage is the storage's address.
                        let _ = writeln!(self.out, "    lea rax, [rip+{label}]");
                    }
                    return Ok(());
                };
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
                // MK7 §5 (the passdown slice): an assignment *through* a
                // borrow, which is what `pd var count` being writable from any
                // function that references it compiles to (`assign` to `*` of
                // the borrow). The pointer is pushed rather than kept in a
                // register across the value's evaluation because the value can
                // itself be a read through the same pointer (`count += 1`),
                // and `addr_of_struct` would claim `r11` for its own address
                // arithmetic on the way — a stack slot cannot be clobbered.
                if let ExprKind::Deref { expr } = &target.kind {
                    // A value wider than one eightbyte cannot travel through
                    // `rax`, so an assignment through the borrow stages the new
                    // value in a scratch slot and copies it through the pointer
                    // — the same two-step `Stmt::Let` and an aggregate local
                    // assignment use. One `push` of the pointer, because
                    // evaluating the value can call and `r11` is the register
                    // `addr_of_struct` claims for its own arithmetic. The value
                    // being replaced is *not* freed, which is the C backend's
                    // behavior for this shape too (the deliberate leaks in
                    // `plan/done.md`): the drop plan records none for a
                    // `*p = v`, so freeing here would be the two backends
                    // disagreeing about a program's output.
                    if let Some(layout) = struct_layout(&target.ty, &self.layouts) {
                        if !layout.class().is_int_one() {
                            let size = layout.size;
                            self.expr(expr, frame)?;
                            self.out.push_str("    push rax\n");
                            let slot = frame.scratch(align_up(size, 8));
                            self.eval_struct_into(value, frame, Dest::Direct(slot))?;
                            self.out.push_str("    pop r11\n");
                            emit_copy(&mut self.out, "r11", 0, "rbp", slot, size);
                            return Ok(());
                        }
                    }
                    if let Some(width) = float_ty(&target.ty) {
                        // MK7 §5's next slice: the store half of a `mut
                        // f64` out-parameter — `*p = v` through the pointer
                        // in `r11`, with the value in `xmm0` where every
                        // float expression leaves it. The pointer is pushed
                        // rather than kept in a register across the value's
                        // evaluation for the same reason the integer arm
                        // below pushes it: the value can itself read through
                        // the same pointer (`*p = *p + 1`), and evaluating
                        // it claims registers.
                        self.expr(expr, frame)?;
                        self.out.push_str("    push rax\n");
                        self.expr(value, frame)?;
                        self.out.push_str("    pop r11\n");
                        let _ = writeln!(self.out, "    {} [r11], xmm0", width.mov());
                        return Ok(());
                    }
                    let (width, _) = int_ty(&target.ty).ok_or_else(|| {
                        unsupported(&format!(
                            "assignment through a reference to `{}`",
                            target.ty.display()
                        ))
                    })?;
                    self.expr(expr, frame)?;
                    self.out.push_str("    push rax\n");
                    self.expr(value, frame)?;
                    self.out.push_str("    pop r11\n");
                    let _ = writeln!(self.out, "    mov {} [r11], {}", width.mem(), width.rax());
                    return Ok(());
                }
                let (loc, ty) = match &target.kind {
                    ExprKind::Ident(name) => frame.lookup(name).ok_or_else(|| {
                        unsupported(&format!("assignment to the name `{name}`"))
                    })?,
                    _ => return Err(unsupported("assignment to anything but a local")),
                };
                if let Some(layout) = struct_layout(&ty, &self.layouts) {
                    if !layout.class().is_int_one() {
                        let off = loc.stack_offset().expect("agg locals are stack-only");
                        let size = layout.size;
                        // MK7 §5 item 5: overwriting an owned binding frees the
                        // value being replaced, *after* the new one has been
                        // read. The new value is staged in its own scratch slot
                        // first because reading it is allowed to read the old
                        // one (`b = b.clone()`), and the old one is gone by the
                        // time it is copied home.
                        //
                        // "Staged first" is not only about drops. The
                        // initializer can read the very binding being assigned
                        // — `list = Node(value = 2, next = Box.of(list))` is how
                        // `box_recursive.kp` grows its chain — and writing each
                        // field straight into the target's own slot would
                        // overwrite the old value *before* the field expression
                        // that reads it ran. The C backend gets this for free
                        // from C's compound-literal-then-assign evaluation
                        // order; here it has to be explicit.
                        let drops = self.drops.at_assign(&self.function, e.span).to_vec();
                        let slot = frame.scratch(align_up(size, 8));
                        self.eval_struct_into(value, frame, Dest::Direct(slot))?;
                        for t in &drops {
                            self.emit_drop(t, frame)?;
                        }
                        emit_copy(&mut self.out, "rbp", off, "rbp", slot, size);
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
            ExprKind::EnumLit { enum_name, variant, fields } => {
                self.enum_lit(enum_name, *variant, fields, frame)
            }
            ExprKind::EnumTag { expr, variant } => {
                let layout = self.addr_of_struct(expr, frame)?;
                self.require_enum_layout(&layout, &expr.ty)?;
                // The tag is the `int32_t` at offset 0 of every enum
                // `write_enum` writes, so the test is one compare against the
                // word the address already points at.
                let _ = writeln!(self.out, "    cmp DWORD PTR [r11], {variant}");
                self.out.push_str("    sete al\n    movzx rax, al\n");
                Ok(())
            }
            ExprKind::EnumField { base, variant, field } => {
                self.enum_field_read(base, *variant, *field, frame)
            }
            // MK5-S6-M1: `expr?`. The generic `expr` path can only carry the
            // success payload in `rax`, so a payload wider than eight bytes is
            // refused here by name; the contexts that can hold one (`let`, an
            // assignment, a `return`, a call argument) reach `try_expr`
            // through `eval_struct_into` with a destination instead.
            ExprKind::Try { .. } => self.try_expr(e, None, frame),
            ExprKind::Ref { expr, .. } => {
                // MK7 §5 item 8: a `&str` is the pointer itself — there is
                // nothing to take the address of, and `codegen_c.rs` makes the
                // same exception (it passes a `str` view through rather than
                // hoisting it the way it hoists a borrowed temporary). A `str`
                // *passdown*'s borrow is the same `const char*` as its value.
                if str_ty(&expr.ty) {
                    return self.expr(expr, frame);
                }
                // MK7 §5 (the passdown slice): `&*p` is `p`. This is how a
                // method on a passdown-typed value is called
                // (`origin.sum()` is `sum(&(*origin))`), and for a plain
                // pointer the deref-then-borrow pair genuinely cancels.
                if let ExprKind::Deref { expr: inner } = &expr.kind {
                    return self.expr(inner, frame);
                }
                // MK7 §5 item 4, first slice: a borrow of a named `datac`
                // local, which is what most of this item's gaps actually need
                // — a `datac`-typed call argument that is not marked `move`
                // becomes exactly this (`infer.rs::check_call_arg`), and it is
                // also what an explicit `&c` compiles to. No copy: the local's
                // own slot already *is* the object.
                if let ExprKind::Ident(name) = &expr.kind {
                    if let Some((loc, ty)) = frame.lookup(name) {
                        if struct_layout(&ty, &self.layouts).is_some() {
                            let off = loc
                                .stack_offset()
                                .expect("struct locals are never register-allocated");
                            let _ = writeln!(self.out, "    lea rax, [rbp{off}]");
                            return Ok(());
                        }
                        if matches!(ty, Ty::Ref { .. }) {
                            // The name is already a borrow — a hidden passdown
                            // parameter is exactly this — so `&*it` is its own
                            // value, and there is no address to take.
                            return self.expr(expr, frame);
                        }
                        // MK7 §5's next slice: a borrow of a named *scalar*
                        // local — `sum(&(*origin))`, an explicit `&mut x`, and
                        // above all the `&mut v` `infer.rs` inserts for a `mut`
                        // out-parameter (`parse_i64_raw(s, out = v)`). Nothing
                        // is copied and no scratch slot is needed: the local
                        // already has a frame slot of its own, because
                        // `LiveAnalysis` keeps every local whose address is
                        // taken out of the register pool (`addr_taken`). The
                        // address is then an ordinary integer-typed pointer, so
                        // the call marshalling above needs no change.
                        if let Some(off) = loc.stack_offset() {
                            let _ = writeln!(self.out, "    lea rax, [rbp{off}]");
                            return Ok(());
                        }
                        return Err(unsupported(&format!(
                            "a borrow of the register-allocated local `{name}`"
                        )));
                    }
                    // Not a local: a passdown's generated storage, whose
                    // borrow is its address (`infer::PdInfo::arg`'s
                    // `global` shape, which wraps the name in a `Ref`).
                    if self.pd_global(name).is_some() {
                        return self.expr(expr, frame);
                    }
                }
                // MK7 §5's heap-field slice: `&d.f` where `f` is an aggregate
                // field — the address of the field inside the base object,
                // computed with the same layout a read uses. This is what a
                // method call on an owned field lowers to (`h.name.len()` is
                // `String#len(&h.name)`), and it has to be an address rather
                // than a copy: the temporary branch below would hand the callee
                // a fresh staging slot whose buffer nothing ever frees.
                if let ExprKind::FieldAccess { base, field } = &expr.kind {
                    if let Ok(layout) = self.addr_of_struct(base, frame) {
                        if let Some((offset, _, _)) = layout.aggregate_field(field) {
                            if offset != 0 {
                                let _ = writeln!(self.out, "    add r11, {offset}");
                            }
                            self.out.push_str("    mov rax, r11\n");
                            return Ok(());
                        }
                    }
                }
                // MK7 §5's enum slice: `&` of one payload field of one variant of
                // an `enum` — the shape an owned payload binding is, since
                // `infer.rs` binds a non-`Copy` payload as a shared borrow
                // (`Word(text) -> text.len()` is `&v.kelp_v.kelp_v_Word.kelp_text`
                // in C). Same arithmetic as the aggregate-field case above, with
                // the field's offset coming from the variant's own payload entry
                // rather than from a `datac` field list.
                if let ExprKind::EnumField { base, variant, field } = &expr.kind {
                    if let Ok(layout) = self.addr_of_struct(base, frame) {
                        if let Some(entry) = layout
                            .variant_layout(*variant)
                            .and_then(|v| v.entries.get(*field))
                        {
                            let offset = match entry {
                                FieldEntry::Scalar(_, offset, ..)
                                | FieldEntry::Float(_, offset, ..)
                                | FieldEntry::Aggregate(_, offset, ..) => *offset,
                            };
                            if offset != 0 {
                                let _ = writeln!(self.out, "    add r11, {offset}");
                            }
                            self.out.push_str("    mov rax, r11\n");
                            return Ok(());
                        }
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
                    "a borrow of anything but a named local (a scalar, a `datac` or a heap \
                     type), a `datac` temporary, or a field of one",
                ))
            }
            ExprKind::Deref { expr } => {
                // MK7 §5 (the passdown slice): a read *through* a borrow — the
                // `*` in "a read of the passdown is `*` of the borrow".
                //
                // A `str` pointee is the identity: `types.rs`'s rule that a
                // reference to an already-borrowed view is not a pointer to a
                // pointer means `*` of a `&str` is the `str` it points at, and
                // that is exactly how a `str` passdown is read (the C backend
                // emits the name with no `*` for the same reason).
                if str_ty(&e.ty) {
                    return self.expr(expr, frame);
                }
                if struct_layout(&e.ty, &self.layouts).is_some() {
                    // Field access through the pointer is what a `datac`
                    // passdown is actually used for, and `addr_of_struct`
                    // already resolves a `Deref` base; reading the whole value
                    // *out* of the borrow needs a copy the fixtures do not ask
                    // for, so it is refused by name rather than half-done.
                    return Err(unsupported(
                        "reading a `datac` out of a reference by value (read its fields, or \
                         take it as a borrow)",
                    ));
                }
                // MK7 §5's next slice: `*p` for a scalar pointee. An integer
                // load is the passdown slice's own — width-specific, then
                // re-extended — and a *float* one is the same `movss`/`movsd`
                // a float local's read uses, aimed at the pointer in `rax`
                // instead of a frame slot. This is the read half of a `mut
                // i64`/`mut f64` out-parameter; the store half is the `Deref`
                // target of `ExprKind::Assign`.
                if let Some(width) = float_ty(&e.ty) {
                    self.expr(expr, frame)?;
                    let _ = writeln!(self.out, "    {} xmm0, [rax]", width.mov());
                    return Ok(());
                }
                let (width, signed) = int_ty(&e.ty)
                    .ok_or_else(|| unsupported(&format!("the type `{}`", e.ty.display())))?;
                self.expr(expr, frame)?;
                // `rax` holds the pointer; load the pointee at its own width
                // and extend, exactly as a narrow local's read does.
                let _ = writeln!(self.out, "    mov {}, {} [rax]", width.rax(), width.mem());
                self.out.push_str(&width.extend(signed));
                Ok(())
            }
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
                    // The remainder is left in the high half of the dividend —
                    // the register `hi` above names — and it has to be moved
                    // into `rax`'s own narrow register before the `extend`
                    // below, which reads `rax`'s names (`al`/`ax`/`eax`). A
                    // byte is the case worth spelling out: a byte dividend's
                    // remainder lands in `ah`, not in `RDX`'s `dl`, so the two
                    // ends of this move are not the same register table.
                    //
                    // Both ends must carry the *same* width — that is the whole
                    // bug this fixes: `mov rax, edx` (a 64-bit destination and
                    // a 32-bit source) is not an instruction, so `%` below
                    // sixty-four bits failed to assemble rather than
                    // miscompiling, and no fixture had ever used one that the
                    // asm backend could reach.
                    let rem = ["ah", "dx", "edx", "rdx"][width.idx()];
                    let _ = writeln!(self.out, "    mov {}, {rem}", width.rax());
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
                "the datac `{name}` (it holds a field this backend cannot lower yet — a \
                 float, a `List`/`Map`/`Box`/`enum`, or a `datac` that does — see \
                 plan/mk7-own-backend.md §5 item 4)"
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
        if !layout.class().is_int_one() {
            return Err(unsupported(&format!(
                "the datac `{name}` used as a plain value (it is over eight bytes; only a \
                 `let`, an assignment, a `return`, or a call argument may hold one directly)"
            )));
        }
        // A slot of this literal's own, not the shared `agg_slot`: a field
        // initializer can evaluate another literal of its own (`Pair(a =
        // f(Pair(x = 1, y = 2)), b = 2)`), and one shared slot would let the
        // inner one overwrite the tag/fields the outer one had already
        // written. See `Gen::call`'s argument staging for the same correction
        // and `tests/fixtures/nested_literal_staging.kp` for the fixture that
        // pins it.
        let slot = frame.scratch(align_up(layout.size, 8));
        self.struct_lit_at(name, fields, frame, Dest::Direct(slot))?;
        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot}]");
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
                "the datac `{name}` (it holds a field this backend cannot lower yet — a \
                 float, a `List`/`Map`/`Box`/`enum`, or a `datac` that does — see \
                 plan/mk7-own-backend.md §5 item 4)"
            ))
        })?;
        for entry in &layout.entries {
            let fname = match entry {
                FieldEntry::Scalar(n, ..)
                | FieldEntry::Float(n, ..)
                | FieldEntry::Aggregate(n, ..) => n,
            };
            let value = fields
                .iter()
                .find(|(n, _)| n == fname)
                .map(|(_, e)| e)
                .ok_or_else(|| {
                    unsupported(&format!("a `{name}` literal missing its `{fname}` field"))
                })?;
            match entry {
                FieldEntry::Scalar(_, foffset, fwidth, _) => {
                    self.expr(value, frame)?;
                    self.store_value(dest, *foffset, *fwidth);
                }
                // MK7 §5 item 4's float-in-aggregate slice: a float field's
                // initializer leaves its value in `xmm0` (every float
                // expression does), so it is stored with `movss`/`movsd`
                // rather than through `rax`.
                FieldEntry::Float(_, foffset, fwidth) => {
                    self.expr(value, frame)?;
                    self.store_float(dest, *foffset, *fwidth);
                }
                // MK7 §5's heap-field slice: an aggregate field — a `String`,
                // a `Bytes`, or a nested `datac` — is staged in a scratch slot
                // of its own (`eval_struct_into`, which is the same code a
                // `let` of that type uses) and then copied into place at its
                // own byte count. A shared `agg_slot` would not do: the field
                // value can itself contain a nested literal that reuses it,
                // and a second field's staging would overwrite the first
                // before this one's copy ran.
                FieldEntry::Aggregate(_, foffset, key, _) => {
                    let inner = self.layouts.get(key).cloned().ok_or_else(|| {
                        unsupported(&format!("the type a field of `{name}` holds"))
                    })?;
                    let slot = frame.scratch(align_up(inner.size, 8));
                    self.eval_struct_into(value, frame, Dest::Direct(slot))?;
                    self.copy_into_dest(dest, *foffset, slot, inner.size);
                }
            }
        }
        Ok(())
    }

    /// Stores `rax`'s low `width` bytes at `offset` into `dest` — the scalar
    /// half of `struct_lit_at`, shared by all three `Dest` shapes.
    fn store_value(&mut self, dest: Dest, offset: i32, width: Width) {
        match dest {
            Dest::Direct(base) => {
                let addr = base + offset;
                let _ = writeln!(
                    self.out,
                    "    mov {} [rbp{addr}], {}",
                    width.mem(),
                    width.rax()
                );
            }
            Dest::Indirect(slot) => {
                // `rax` already holds the field's value; loading the pointer
                // doesn't touch it, so no save/restore is needed around this
                // the way `field_assign` needs one around its own address
                // computation.
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                let _ = writeln!(
                    self.out,
                    "    mov {} [r11{}], {}",
                    width.mem(),
                    off_str(offset),
                    width.rax()
                );
            }
            Dest::Global(index) => {
                // MK7 §5 (the passdown slice): a `pd` initializer builds its
                // fields straight into the passdown's own storage, with no
                // frame slot in between — the same store as `Dest::Direct`'s,
                // with a symbol instead of `rbp`.
                let label = self.pds[index].label.clone();
                let _ = writeln!(
                    self.out,
                    "    mov {} [rip+{label}{}], {}",
                    width.mem(),
                    off_str(offset),
                    width.rax()
                );
            }
        }
    }

    /// The float counterpart of `store_value`: stores `xmm0`'s low
    /// `width` bytes at `offset` into `dest`. `f32`/`f64` values live in the
    /// vector file, so a field initializer's store cannot go through `rax`.
    fn store_float(&mut self, dest: Dest, offset: i32, width: FWidth) {
        match dest {
            Dest::Direct(base) => {
                let addr = base + offset;
                let _ = writeln!(self.out, "    {} [rbp{addr}], xmm0", width.mov());
            }
            Dest::Indirect(slot) => {
                let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                let _ = writeln!(
                    self.out,
                    "    {} [r11{}], xmm0",
                    width.mov(),
                    off_str(offset)
                );
            }
            Dest::Global(index) => {
                let label = self.pds[index].label.clone();
                let _ = writeln!(
                    self.out,
                    "    {} [rip+{label}{}], xmm0",
                    width.mov(),
                    off_str(offset)
                );
            }
        }
    }

    /// Copies `size` bytes from the frame slot `src` to `dest + offset` — the
    /// aggregate half of `struct_lit_at`, and the shape `eval_struct_into`'s
    /// field-access arm writes an owned field through.
    fn copy_into_dest(&mut self, dest: Dest, offset: i32, src: i32, size: i32) {
        match dest {
            Dest::Direct(base) => {
                emit_copy(&mut self.out, "rbp", base + offset, "rbp", src, size)
            }
            Dest::Indirect(slot) => {
                let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot}]");
                emit_copy(&mut self.out, "r10", offset, "rbp", src, size);
            }
            Dest::Global(index) => {
                let label = self.pds[index].label.clone();
                emit_copy_to_global(&mut self.out, &label, offset, "rbp", src, size);
            }
        }
    }

    /// MK7 §5's enum slice: the layout of the `enum` `name`, or a refusal that
    /// names it. The second half is what keeps a float-payload `enum` reading
    /// as "this enum" rather than as the wrong kind of type: such an `enum`
    /// simply has no layout (see `try_build_enum_layout`), so every site that
    /// wanted to move a value of it says so by name.
    fn enum_layout(&self, name: &str) -> Result<StructLayout, String> {
        match self.layouts.get(name) {
            Some(layout) if layout.variants.is_some() => Ok(layout.clone()),
            _ => Err(unsupported(&format!("the enum `{name}`"))),
        }
    }

    /// The same refusal as `enum_layout`, for a site that already holds a
    /// layout (one `addr_of_struct` computed while resolving an address) and
    /// needs to know it really is an `enum`'s.
    fn require_enum_layout(&self, layout: &StructLayout, ty: &Ty) -> Result<(), String> {
        if layout.variants.is_some() {
            Ok(())
        } else {
            Err(unsupported(&format!("the enum `{}`", enum_name_of(ty))))
        }
    }

    /// `Shape.Circle(r = 1.0)`, as a plain value in `rax` — the `One`-class
    /// counterpart of `enum_lit_at`, with the same narrowing `struct_lit` has
    /// and for the same reason (see `eval_struct_into`'s doc).
    fn enum_lit(
        &mut self,
        enum_name: &str,
        variant: usize,
        fields: &[Expr],
        frame: &mut Frame,
    ) -> Result<(), String> {
        let layout = self.enum_layout(enum_name)?;
        if !layout.class().is_int_one() {
            return Err(unsupported(&format!(
                "the enum `{enum_name}` used as a plain value (it is over eight bytes; only a \
                 `let`, an assignment, a `return`, or a call argument may hold one)"
            )));
        }
        // A slot of its own, for the reason `struct_lit`'s doc gives: a
        // payload expression can itself evaluate a literal, and the shared
        // `agg_slot` would let that one overwrite the tag written below.
        let slot = frame.scratch(align_up(layout.size, 8));
        self.enum_lit_at(enum_name, variant, fields, frame, Dest::Direct(slot))?;
        let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot}]");
        Ok(())
    }

    /// The class-agnostic core of `enum_lit`: writes the variant's tag at
    /// offset 0 and its payload — the value of `fields[i]` at the `i`th payload
    /// field's own offset — straight to `dest`, mirroring `codegen_c.rs`'s
    /// compound literal `{ .kelp_tag = v, .kelp_v = { .<member> = { ... } } }`.
    /// The field order is positional because the TIR node carries one value per
    /// payload field in declaration order (`infer.rs::enum_lit`), which is also
    /// why there is no name matching here the way `struct_lit_at` has.
    ///
    /// Nothing writes the *other* variants' payloads, exactly as C's designated
    /// initializer does not: they are not live, nothing reads them, and the only
    /// thing that could observe them is a whole-value copy.
    fn enum_lit_at(
        &mut self,
        enum_name: &str,
        variant: usize,
        fields: &[Expr],
        frame: &mut Frame,
        dest: Dest,
    ) -> Result<(), String> {
        let layout = self.enum_layout(enum_name)?;
        let entries = layout
            .variant_layout(variant)
            .ok_or_else(|| {
                unsupported(&format!("the variant #{variant} of `{enum_name}`"))
            })?
            .entries
            .clone();
        if entries.len() != fields.len() {
            return Err(unsupported(&format!(
                "a `{enum_name}` literal whose payload does not match its variant"
            )));
        }
        let _ = writeln!(self.out, "    mov eax, {variant}");
        self.store_value(dest, 0, ENUM_TAG_WIDTH);
        for (entry, value) in entries.iter().zip(fields.iter()) {
            match entry {
                FieldEntry::Scalar(_, offset, width, _) => {
                    self.expr(value, frame)?;
                    self.store_value(dest, *offset, *width);
                }
                // MK7 §5 item 4's float-in-aggregate slice: a float payload is
                // stored from `xmm0` exactly as a float `datac` field is.
                FieldEntry::Float(_, offset, width) => {
                    self.expr(value, frame)?;
                    self.store_float(dest, *offset, *width);
                }
                // The same staging `struct_lit_at` uses for an aggregate field:
                // a scratch slot of the field's own size, built through
                // `eval_struct_into`, then copied into place. A shared slot
                // would not do — a payload expression can itself contain a
                // literal that reuses it.
                FieldEntry::Aggregate(_, offset, key, _) => {
                    let inner = self.layouts.get(key).cloned().ok_or_else(|| {
                        unsupported(&format!("the type a payload field of `{enum_name}` holds"))
                    })?;
                    let slot = frame.scratch(align_up(inner.size, 8));
                    self.eval_struct_into(value, frame, Dest::Direct(slot))?;
                    self.copy_into_dest(dest, *offset, slot, inner.size);
                }
            }
        }
        Ok(())
    }

    /// One payload field of one variant as a plain value in `rax` — what a
    /// `Copy` payload binds to inside a `when` arm (`Digits(n) -> n`), reached
    /// through the same address `addr_of_struct` computes for the scrutinee.
    ///
    /// An *aggregate* payload field is refused here rather than truncated: a
    /// non-`Copy` payload binds as a shared borrow (`&String`), which goes
    /// through `expr`'s `Ref` arm and never reaches this function, so reaching
    /// it means something asked for the field itself as one register's worth.
    fn enum_field_read(
        &mut self,
        base: &Expr,
        variant: usize,
        field: usize,
        frame: &mut Frame,
    ) -> Result<(), String> {
        let layout = self.addr_of_struct(base, frame)?;
        self.require_enum_layout(&layout, &base.ty)?;
        let entry = layout
            .variant_layout(variant)
            .and_then(|v| v.entries.get(field))
            .cloned()
            .ok_or_else(|| {
                unsupported(&format!(
                    "the payload field #{field} of variant #{variant} of the enum `{}`",
                    enum_name_of(&base.ty)
                ))
            })?;
        match entry {
            FieldEntry::Scalar(_, offset, width, signed) => {
                let _ = writeln!(self.out, "    mov {}, {} [r11+{offset}]", width.rax(), width.mem());
                self.out.push_str(&width.extend(signed));
                Ok(())
            }
            // MK7 §5 item 4's float-in-aggregate slice: a float payload is
            // `Copy`, so this is the path `Digits(n) -> n` for an `f64` takes;
            // it leaves the value in `xmm0` where every float read does.
            FieldEntry::Float(_, offset, width) => {
                let _ = writeln!(self.out, "    {} xmm0, [r11+{offset}]", width.mov());
                Ok(())
            }
            FieldEntry::Aggregate(..) => Err(unsupported(&format!(
                "reading an aggregate payload of the enum `{}` as a plain value (bind it as a \
                 borrow, or pass it on)",
                enum_name_of(&base.ty)
            ))),
        }
    }

    /// The success payload of a `?` (or of one variant of any match) moved out
    /// of the temporary it was evaluated into, at `src_slot + offset` — into
    /// `dest` when the value is wider than one eightbyte, and into `rax`
    /// otherwise. One copy and no drop: the payload is *moved*, exactly as C's
    /// assignment of the same member is, and the temporary it came from is
    /// never dropped.
    fn payload_out(
        &mut self,
        entry: &FieldEntry,
        src_slot: i32,
        dest: Option<Dest>,
        enum_name: &str,
    ) -> Result<(), String> {
        match entry {
            FieldEntry::Scalar(_, offset, width, signed) => {
                let addr = src_slot + offset;
                let _ = writeln!(self.out, "    mov {}, {} [rbp{addr}]", width.rax(), width.mem());
                self.out.push_str(&width.extend(*signed));
                if let Some(dest) = dest {
                    self.store_value(dest, 0, *width);
                }
                Ok(())
            }
            FieldEntry::Float(_, offset, width) => {
                let addr = src_slot + offset;
                let _ = writeln!(self.out, "    {} xmm0, [rbp{addr}]", width.mov());
                if let Some(dest) = dest {
                    self.store_float(dest, 0, *width);
                }
                Ok(())
            }
            FieldEntry::Aggregate(_, offset, key, _) => {
                let (size, one_class) = match self.layouts.get(key) {
                    Some(l) => (l.size, l.class().is_int_one()),
                    None => return Err(unsupported("the type a payload of this enum holds")),
                };
                let src = src_slot + offset;
                let Some(dest) = dest else {
                    // A one-eightbyte payload *is* a value `rax` can hold, so
                    // "wider than eight bytes" is the question, not the
                    // field's kind: a `One`-class payload loads into `rax` at
                    // its own width, the same opaque zero-extended eightbyte
                    // `field_read` produces, and only an actually-wider payload
                    // is refused for want of a destination. (The refusal used
                    // to fire on the field kind alone and said "over eight
                    // bytes" about a three-byte `datac`, which was both a
                    // false statement and a real program refused.)
                    if one_class {
                        load_agg_bytes(&mut self.out, RAX, "rbp", src, size);
                        return Ok(());
                    }
                    return Err(unsupported(&format!(
                        "the enum `{enum_name}` (a `?` whose success payload is wider than \
                         eight bytes, in a position that can only hold one eightbyte)"
                    )));
                };
                match dest {
                    Dest::Direct(off) => emit_copy(&mut self.out, "rbp", off, "rbp", src, size),
                    Dest::Indirect(slot) => {
                        let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot}]");
                        emit_copy(&mut self.out, "r10", 0, "rbp", src, size);
                    }
                    Dest::Global(index) => {
                        let label = self.pds[index].label.clone();
                        emit_copy_to_global(&mut self.out, &label, 0, "rbp", src, size);
                    }
                }
                Ok(())
            }
        }
    }

    /// MK5-S6-M1's `?`, transliterated from `codegen_c.rs`'s
    /// statement-expression: evaluate the operand once into a slot of its own,
    /// and if it carries the failure tag rebuild that failure in the enclosing
    /// function's return type and return it — running the frame's own drops
    /// first, exactly as a written `return` does — otherwise the value is the
    /// live success payload, moved out of the temporary. The temporary is
    /// deliberately never dropped, which is the C backend's rule too: exactly
    /// one variant's payload is moved out of it on either path, and dropping the
    /// rest would free what was moved.
    ///
    /// `dest` is `Some` for the contexts that can hold a value wider than `rax`
    /// (a `let`, an assignment, a `return`, a call argument — the same four
    /// `eval_struct_into` serves). With `None`, a success payload wider than one
    /// eightbyte is refused by name rather than truncated.
    fn try_expr(&mut self, e: &Expr, dest: Option<Dest>, frame: &mut Frame) -> Result<(), String> {
        let ExprKind::Try {
            expr: operand,
            source,
            target,
            ok_variant,
            err_variant,
            target_err_variant,
            err_has_payload,
            ..
        } = &e.kind
        else {
            return Err(unsupported("a `?` outside an expression"));
        };
        let src_layout = self.enum_layout(source)?;
        let tmp = frame.scratch(align_up(src_layout.size, 8));
        self.eval_struct_into(operand, frame, Dest::Direct(tmp))?;
        let ok = self.label("try_ok");
        let _ = writeln!(self.out, "    mov eax, DWORD PTR [rbp{tmp}]");
        let _ = writeln!(self.out, "    cmp eax, {err_variant}");
        let _ = writeln!(self.out, "    jne {ok}");
        // The failure arm: the source's error payload rebuilt in the target
        // enum (`target` is the enclosing function's own return type, which is
        // why the destination is the same place `Stmt::Return` writes a value of
        // that class — the hidden return pointer for a `MEMORY`-class one, the
        // shared aggregate slot for a register-pair one, and `rax` otherwise).
        let target_layout = self.enum_layout(target)?;
        let ret_dst = match frame.ret_class {
            RetClass::Agg(class) if !class.is_mem() => Dest::Direct(frame.agg_slot),
            RetClass::Agg(_) => Dest::Indirect(frame.ret_ptr_slot),
            _ => {
                let slot = frame.scratch(align_up(target_layout.size, 8));
                Dest::Direct(slot)
            }
        };
        let _ = writeln!(self.out, "    mov eax, {target_err_variant}");
        self.store_value(ret_dst, 0, ENUM_TAG_WIDTH);
        if *err_has_payload {
            let src_entry = src_layout
                .variant_layout(*err_variant)
                .and_then(|v| v.entries.first())
                .cloned()
                .ok_or_else(|| unsupported(&format!("the error payload of the enum `{source}`")))?;
            let dst_entry = target_layout
                .variant_layout(*target_err_variant)
                .and_then(|v| v.entries.first())
                .cloned()
                .ok_or_else(|| unsupported(&format!("the error payload of the enum `{target}`")))?;
            self.move_payload(&src_entry, tmp, &dst_entry, ret_dst, source)?;
        }
        for t in self.drops.at_try(&self.function, e.span).to_vec() {
            self.emit_drop(&t, frame)?;
        }
        // The two wider classes are read by the epilogue itself, so only a
        // one-eightbyte target has to be finished here — and `AggTwo` is *not*
        // one of those: its value lives in `agg_slot`, which is where
        // `ret_dst` already put it.
        if let Dest::Direct(slot) = ret_dst {
            if !matches!(frame.ret_class, RetClass::Agg(_)) {
                let _ = writeln!(self.out, "    mov rax, QWORD PTR [rbp{slot}]");
            }
        }
        let label = self.return_label.clone();
        let _ = writeln!(self.out, "    jmp {label}");
        let _ = writeln!(self.out, "{ok}:");
        let ok_entry = src_layout
            .variant_layout(*ok_variant)
            .and_then(|v| v.entries.first())
            .cloned()
            .ok_or_else(|| unsupported(&format!("the success payload of the enum `{source}`")))?;
        self.payload_out(&ok_entry, tmp, dest, source)
    }

    /// Moves one payload field from `src_slot` to the same position of the same
    /// field in `dst`, whose own layout is `dst_entry` — the shape
    /// `codegen_c.rs` writes as
    /// `ret.kelp_v.<member>.<field> = tmp.kelp_v.<member>.<field>;`.
    fn move_payload(
        &mut self,
        src_entry: &FieldEntry,
        src_slot: i32,
        dst_entry: &FieldEntry,
        dst: Dest,
        enum_name: &str,
    ) -> Result<(), String> {
        let src_offset = match src_entry {
            FieldEntry::Scalar(_, o, ..)
            | FieldEntry::Float(_, o, ..)
            | FieldEntry::Aggregate(_, o, ..) => *o,
        };
        match (src_entry, dst_entry) {
            (FieldEntry::Scalar(_, _, width, signed), FieldEntry::Scalar(_, doff, _, _)) => {
                let addr = src_slot + src_offset;
                let _ = writeln!(self.out, "    mov {}, {} [rbp{addr}]", width.rax(), width.mem());
                self.out.push_str(&width.extend(*signed));
                self.store_value(dst, *doff, *width);
                Ok(())
            }
            (FieldEntry::Float(_, _, width), FieldEntry::Float(_, doff, _)) => {
                let addr = src_slot + src_offset;
                let _ = writeln!(self.out, "    {} xmm0, [rbp{addr}]", width.mov());
                self.store_float(dst, *doff, *width);
                Ok(())
            }
            (FieldEntry::Aggregate(_, _, key, _), FieldEntry::Aggregate(_, doff, _, _)) => {
                let size = self
                    .layouts
                    .get(key)
                    .map(|l| l.size)
                    .ok_or_else(|| unsupported("the type an error payload holds"))?;
                self.copy_into_dest(dst, *doff, src_slot + src_offset, size);
                Ok(())
            }
            // Unreachable by construction — the two variants' payload types are
            // the same type, so their layout entries are the same kind — and a
            // refusal rather than a panic because "unreachable" in a compiler is
            // a claim, and this one costs one arm to not have to trust.
            _ => Err(unsupported(&format!(
                "propagating the failure of the enum `{enum_name}`"
            ))),
        }
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
            // MK7 §5's enum slice: a one-variant construction is the same shape
            // `struct_lit_at` is, so it writes the tag and then its payload into
            // `dest` directly.
            ExprKind::EnumLit { enum_name, variant, fields } => {
                self.enum_lit_at(enum_name, *variant, fields, frame, dest)
            }
            // MK5-S6-M1: `expr?` in one of the four positions that can hold a
            // value wider than `rax` — the success payload is moved straight
            // into `dest`, so an owned payload (`Option<String>`'s) travels
            // without ever being truncated to a register.
            ExprKind::Try { .. } => self.try_expr(expr, Some(dest), frame),
            // A block in value position — which is what a `when` with a
            // scrutinee is (`infer.rs` wraps the folded arm chain in one, and
            // `if let` too), so a `when` that produces a wide value at all
            // arrives here. `block_into` is the same walk `Gen::block` does,
            // with the tail written to `dest` instead of parked in a register.
            ExprKind::Block(block) => self.block_into(block, frame, dest),
            // MK7 §5's enum slice: an `if` in *value* position producing a
            // value wider than one eightbyte. The C backend gets this from a
            // statement-expression with a temporary in it; here each arm writes
            // its value into the caller's own destination instead, which is the
            // same thing with no second copy — and it is not enum-specific, it
            // is the missing half of "a wide value has a destination", which
            // `str_to_string_coercion` had been waiting on as its first
            // refusal.
            ExprKind::If { cond, then_block, else_block } => {
                let alt = self.label("else");
                let end = self.label("endif");
                self.expr(cond, frame)?;
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    je {alt}");
                self.block_into(then_block, frame, dest)?;
                let _ = writeln!(self.out, "    jmp {end}");
                let _ = writeln!(self.out, "{alt}:");
                match else_block {
                    Some(block) => self.block_into(block, frame, dest)?,
                    // An `if` without an `else` has type `Unit`, so a wide
                    // value can never arrive through this arm — and writing
                    // nothing is the honest lowering of "there is no value".
                    None => {}
                }
                let _ = writeln!(self.out, "{end}:");
                Ok(())
            }
            // MK7 §5's heap-field slice: an *aggregate field* read by value —
            // `print(h.name)`, `val s = h.name`, an owned field handed to a
            // callee. `addr_of_struct` already computes the base object's
            // address into `r11` (including through a borrow), so the whole of
            // this arm is a byte copy out of `[r11+offset]` at the field
            // layout's own size, exactly what `emit_copy` does for a `Mem`- or
            // `Two`-class value elsewhere.
            ExprKind::FieldAccess { base, field } => {
                let layout = self.addr_of_struct(base, frame)?;
                let (offset, key, _) = layout.aggregate_field(field).ok_or_else(|| {
                    unsupported(&format!("the field `{field}` of `{}`", base.ty.display()))
                })?;
                let inner = self.layouts.get(key).cloned().ok_or_else(|| {
                    unsupported(&format!("the type a field of `{}` holds", base.ty.display()))
                })?;
                match dest {
                    Dest::Direct(off) => {
                        emit_copy(&mut self.out, "rbp", off, "r11", offset, inner.size)
                    }
                    Dest::Indirect(slot) => {
                        let _ = writeln!(self.out, "    mov r10, QWORD PTR [rbp{slot}]");
                        emit_copy(&mut self.out, "r10", 0, "r11", offset, inner.size);
                    }
                    Dest::Global(index) => {
                        let label = self.pds[index].label.clone();
                        emit_copy_to_global(&mut self.out, &label, 0, "r11", offset, inner.size);
                    }
                }
                Ok(())
            }
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
                    Dest::Global(index) => {
                        let label = self.pds[index].label.clone();
                        emit_copy_to_global(&mut self.out, &label, 0, "rbp", src_off, layout.size);
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
            let (offset, nested_name, _) = inner_layout.aggregate_field(field).ok_or_else(|| {
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
            // MK7 §5's box slice: a base whose own value *is* the address —
            // `list.next.get().value`, where `Box#get` returned a `&Node`. Any
            // `Ref`-typed expression evaluates to the pointer in `rax`, so the
            // address is already there.
            //
            // MK7 §5's list slice adds the *by-value* counterpart, which is what
            // `List#pop`/`List#remove` hand back: `xs.pop().a`. A `datac`
            // temporary has no address of its own, so it gets a fresh
            // `Frame::scratch` slot built through the same `eval_struct_into` a
            // `let` initializer uses, and the address is a `lea` of that slot —
            // the fourth item-4 slice's mechanism for a borrowed temporary,
            // reused because it is the same problem. The layout comes from the
            // tail of this function either way, since a by-value base's own
            // type *is* the struct type.
            _ => {
                if let Ty::Ref { inner, .. } = &base.ty {
                    if struct_layout(inner, &self.layouts).is_none() {
                        return Err(unsupported(&format!("the type `{}`", base.ty.display())));
                    }
                    self.expr(base, frame)?;
                    self.out.push_str("    mov r11, rax\n");
                } else if let Some(size) = struct_layout(&base.ty, &self.layouts).map(|l| l.size) {
                    let slot = frame.scratch(align_up(size, 8));
                    self.eval_struct_into(base, frame, Dest::Direct(slot))?;
                    let _ = writeln!(self.out, "    lea r11, [rbp{slot}]");
                } else {
                    return Err(unsupported(
                        "a field access on anything but a local or `*local`",
                    ));
                }
            }
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
        // MK7 §5 item 4's float-in-aggregate slice: a float field is read
        // through `xmm0`, at its own width, from the address `addr_of_struct`
        // already computed — no extension, because a float's bits are the
        // value.
        if let Some((foffset, width)) = layout.float_field(field) {
            let _ = writeln!(self.out, "    {} xmm0, [r11+{foffset}]", width.mov());
            return Ok(());
        }
        let Some((foffset, width, signed)) = layout.scalar_field(field) else {
            // MK7 §5's heap-field slice: an *aggregate* field (`h.name`) is a
            // value wider than `rax`, so `expr` cannot leave one there. The
            // contexts that can hold one — `print`, a `let`, a call argument —
            // stage it through `eval_struct_into`, which is where the field
            // access arm lives; reaching here means something asked for the
            // field as a plain one-register value, and that is refused by name
            // rather than truncated to the field's first eight bytes.
            //
            // MK7 §5's enum slice: a **one-eightbyte** aggregate field is the
            // exception, because one eightbyte *is* a value `rax` can hold —
            // the same opaque "whole eightbyte" rule `struct_lit` uses for a
            // `One`-class literal. Without it an `enum` at that class could not
            // be read out of a `datac` field by value at all (`narrow(a =
            // h.small)`), which is the "an enum as a `datac` field at every
            // size class" half of that slice. Anything wider keeps the
            // refusal, because a `Two`/`Mem`-class field cannot travel through
            // `rax`.
            if let Some((offset, key, _)) = layout.aggregate_field(field) {
                // The field's own size, not eight bytes of it: `r11` may be a
                // heap block sized to the enclosing value (`Box.of(Outer(..))`
                // allocates `sizeof(Outer)`), so a plain `mov rax, QWORD PTR`
                // of a three-byte field reads past the allocation — the
                // defect `load_agg_bytes` exists to close.
                let inner = self.layouts.get(key).filter(|l| l.class().is_int_one());
                if let Some(size) = inner.map(|l| l.size) {
                    load_agg_bytes(&mut self.out, RAX, "r11", offset, size);
                    return Ok(());
                }
                return Err(unsupported(&format!(
                    "reading the aggregate field `{field}` of `{}` as a plain value (bind \
                     it, pass it, or print it whole)",
                    base.ty.display()
                )));
            }
            return Err(unsupported(&format!(
                "the field `{field}` of `{}`",
                base.ty.display()
            )));
        };
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
        // MK7 §5 item 4's float-in-aggregate slice: `c.r = 2.0` stores from
        // `xmm0`, where every float expression leaves its value.
        if let Some((foffset, width)) = layout.float_field(field) {
            self.out.push_str("    push r11\n");
            self.expr(value, frame)?;
            self.out.push_str("    pop r11\n");
            let _ = writeln!(self.out, "    {} [r11+{foffset}], xmm0", width.mov());
            return Ok(());
        }
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
        // MK7 §5 item 8 emits the same shape here — see `Gen::conversion` —
        // including the panic message, which is the callee's own dotted name.
        //
        // The name shape alone is not enough: `View#to_string` also contains
        // `#to_`, and it is a plain method (`kelp_View__to_string`), not a
        // numeric conversion — the same distinction `codegen_c.rs` draws by
        // requiring both halves to name an integer/float type before it takes
        // its conversion path. So the lookup is what decides, and a callee the
        // lookup cannot resolve falls through to the `String`/`View`/`Bytes`
        // method tables below (and then to an ordinary call).
        if callee.contains("#to_") {
            let numeric = callee
                .split_once("#to_")
                .map(|(src, dst)| {
                    Ty::from_name(src).map(|t| t.is_integer()).unwrap_or(false)
                        && Ty::from_name(dst)
                            .map(|t| t.is_integer() || t.is_float())
                            .unwrap_or(false)
                })
                .unwrap_or(false);
            if numeric {
                let [arg] = args else {
                    return Err(unsupported(&format!(
                        "the numeric conversion `{}`",
                        callee.replace('#', ".")
                    )));
                };
                return self.conversion(callee, arg, frame);
            }
        }
        // `panic` and `assert` are builtins (`infer.rs`'s `BUILTINS`), not
        // functions, and the C backend lowers them to its own runtime routines
        // `kelp_panic`/`kelp_assert`. This backend owns the one routine they
        // both need — see `Gen::emit_panic_routine` — and inlines `assert`'s
        // single branch, which is all the runtime's one-line body does.
        if callee == "panic" {
            let [arg] = args else {
                return Err(unsupported("a panic that is not given exactly one value"));
            };
            self.expr(arg, frame)?;
            self.out.push_str("    mov rdi, rax\n");
            self.call_panic();
            return Ok(());
        }
        if callee == "assert" {
            let [cond] = args else {
                return Err(unsupported("an assert that is not given exactly one value"));
            };
            self.expr(cond, frame)?;
            let ok = self.label("assert");
            self.out.push_str("    test rax, rax\n");
            let _ = writeln!(self.out, "    jne {ok}");
            let msg = self.bytes_label(b"assertion failed");
            let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
            self.call_panic();
            let _ = writeln!(self.out, "{ok}:");
            return Ok(());
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
        if let Some(c) = ret_class {
            if !c.is_int_one() && ret_dst.is_none() {
                return Err(unsupported(
                    "a struct-returning call of more than one eightbyte (or one whose eightbyte \
                     is not INTEGER) used as a plain value (only a `let`, an assignment, a \
                     `return`, or a discarded statement may hold one)",
                ));
            }
        }

        // SysV numbers the integer and vector argument registers
        // independently, so `f(1, 2.0, 3)` passes `2.0` in `xmm0` — the
        // classification below counts each argument's own class, not its
        // position in the list. A `Mem`-class return reserves `rdi` for the
        // hidden pointer before any real argument is numbered, exactly the
        // way a `Mem`-class *parameter* shifts `Gen::function`'s own
        // numbering.
        enum ArgSlot {
            Float(usize),
            /// A register-classed argument — a scalar, a pointer, a `str`, or
            /// a by-value aggregate — as one entry per eightbyte, in order:
            /// `(kind, register index in that file, bytes in the eightbyte)`.
            /// A scalar is a single `INTEGER` entry; a mixed `Two`-class
            /// aggregate is `INTEGER`/`SSE` or `SSE`/`INTEGER`.
            Regs(Vec<(Eb, usize, i32)>),
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
        let mut int_idx = if ret_class.map_or(false, |c| c.is_mem()) { 1 } else { 0 };
        let mut sse_idx = 0usize;
        for (pos, arg) in args.iter().enumerate() {
            if let Some(_w) = float_ty(&arg.ty) {
                if sse_idx >= FARG.len() {
                    return Err(unsupported(&format!(
                        "a call to `{callee}` with more than eight float arguments"
                    )));
                }
                slots.push(ArgSlot::Float(sse_idx));
                sse_idx += 1;
                continue;
            }
            let (ints, sses) = arg_regs_for(&arg.ty, &self.layouts).ok_or_else(|| {
                unsupported(&format!(
                    "the type `{}` as an argument to `{callee}`",
                    arg.ty.display()
                ))
            })?;
            if int_idx + ints > ARG64.len() {
                return Err(unsupported(&format!(
                    "a call to `{callee}` with more than six integer argument registers"
                )));
            }
            if sse_idx + sses > FARG.len() {
                return Err(unsupported(&format!(
                    "a call to `{callee}` with more than eight float argument registers"
                )));
            }
            if ints == 0 && sses == 0 {
                let (_, off, _) = *mem_args
                    .iter()
                    .find(|(i, _, _)| *i == pos)
                    .expect("memory_arg_area covers every Mem-class argument");
                slots.push(ArgSlot::Stack(off));
            } else {
                // The per-eightbyte schedule for this argument. A scalar
                // (no layout) is one `INTEGER` entry; an aggregate takes its
                // class's own kinds, in order, advancing each file's index by
                // one per eightbyte of that kind.
                let class = struct_layout(&arg.ty, &self.layouts)
                    .map(|l| l.class())
                    .unwrap_or(INT_ONE);
                let mut ebs = Vec::new();
                let (mut ii, mut si) = (int_idx, sse_idx);
                for k in 0..2 {
                    let Some(eb) = class.eb(k) else { break };
                    match eb.kind {
                        Eb::Int => {
                            ebs.push((Eb::Int, ii, eb.size));
                            ii += 1;
                        }
                        Eb::Sse => {
                            ebs.push((Eb::Sse, si, eb.size));
                            si += 1;
                        }
                    }
                }
                slots.push(ArgSlot::Regs(ebs));
            }
            int_idx += ints;
            sse_idx += sses;
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
        // MK7 §5 item 5: a `&Bytes`/`&String` argument whose inner expression
        // is a temporary gets a slot of its own (`hoist_owned_temporary`) and
        // is freed right after the call — the shape `codegen_c.rs::call_value`
        // gets from hoisting the same temporary into a C variable. The drop
        // routine travels with the slot because a scratch slot carries no type.
        let mut hoisted: Vec<(i32, DropSite)> = Vec::new();
        for (pos, arg) in args.iter().enumerate() {
            if let Some(slot) = self.hoist_owned_temporary(arg, frame)? {
                hoisted.push(slot);
                self.out.push_str("    push rax\n");
            } else if float_ty(&arg.ty).is_some() {
                self.expr(arg, frame)?;
                let width = float_ty(&arg.ty).expect("checked above");
                self.out.push_str("    sub rsp, 8\n");
                let _ = writeln!(self.out, "    {} [rsp], xmm0", width.mov());
            } else if let Some(class) = struct_layout(&arg.ty, &self.layouts) {
                let class = class.class();
                let size = struct_layout(&arg.ty, &self.layouts)
                    .expect("checked above")
                    .size;
                if class.is_mem() {
                    let ArgSlot::Stack(off) = slots[pos] else {
                        unreachable!("the classification above put this argument on the stack")
                    };
                    let staged = frame.scratch(align_up(size, 8));
                    self.eval_struct_into(arg, frame, Dest::Direct(staged))?;
                    mem_plan[pos] = Some((staged, off, size));
                } else {
                    // A slot of the argument's own — **not** the shared
                    // `agg_slot` this used to stage into. A register-class
                    // aggregate is built field by field, and a field
                    // initializer can itself be an argument-bearing expression
                    // (`Box.of(Node(value = 2, next = Box.of(...)))`), whose
                    // own argument would then land in the same shared slot
                    // *while the outer value was half-written* —
                    // `generic_recursive`'s three-node chain read the middle
                    // node's `value` as the innermost node's (7 instead of 6)
                    // before this was a fresh slot per argument. Each eightbyte
                    // is then pushed as a whole word, in order, and popped into
                    // the register its kind names.
                    let agg = frame.scratch(align_up(size, 8));
                    self.eval_struct_into(arg, frame, Dest::Direct(agg))?;
                    let n_eb = if class.eb(1).is_some() { 2 } else { 1 };
                    for k in 0..n_eb {
                        let at = agg + 8 * k as i32;
                        let _ = writeln!(self.out, "    push QWORD PTR [rbp{at}]");
                    }
                }
            } else {
                self.expr(arg, frame)?;
                self.out.push_str("    push rax\n");
            }
        }
        for (arg, slot) in args.iter().zip(slots.iter()).rev() {
            match slot {
                ArgSlot::Float(i) => {
                    let width = float_ty(&arg.ty).expect("classified as float above");
                    let _ = writeln!(self.out, "    {} {}, [rsp]", width.mov(), FARG[*i]);
                    self.out.push_str("    add rsp, 8\n");
                }
                ArgSlot::Regs(ebs) => {
                    // Pushed in eightbyte order, so the last is on top and
                    // comes off first. An `INTEGER` eightbyte is a plain `pop`;
                    // an `SSE` one is loaded into `xmm` at its own width and
                    // the slot discarded.
                    for (kind, idx, size) in ebs.iter().rev() {
                        match kind {
                            Eb::Int => {
                                let _ = writeln!(self.out, "    pop {}", ARG64[*idx]);
                            }
                            Eb::Sse => {
                                let w = if *size == 4 { FWidth::F4 } else { FWidth::F8 };
                                let _ = writeln!(
                                    self.out,
                                    "    {} {}, [rsp]",
                                    w.mov(),
                                    FARG[*idx]
                                );
                                self.out.push_str("    add rsp, 8\n");
                            }
                        }
                    }
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
        if ret_class.map_or(false, |c| c.is_mem()) {
            match ret_dst.expect("checked above") {
                Dest::Direct(off) => {
                    let _ = writeln!(self.out, "    lea rdi, [rbp{off}]");
                }
                Dest::Indirect(slot) => {
                    let _ = writeln!(self.out, "    mov rdi, QWORD PTR [rbp{slot}]");
                }
                // MK7 §5 (the passdown slice): a `pd` whose type is
                // `MEMORY`-class, initialized by a call — the hidden pointer
                // is the passdown's own storage.
                Dest::Global(index) => {
                    let label = self.pds[index].label.clone();
                    let _ = writeln!(self.out, "    lea rdi, [rip+{label}]");
                }
            }
        }
        let symbol = if let Some(r) = bytes_method(callee)
            .or_else(|| string_method(callee))
            .or_else(|| view_method(callee))
        {
            // MK7 §5's heap slices: the `Bytes`/`String`/`View` builtins are
            // routines this backend owns, exactly like the externs
            // `extern_target` maps.
            self.require_routine(r);
            r.label().to_string()
        } else if let Some(op) = callee.strip_prefix("Box#") {
            // MK7 §5's box slice: the whole `Box<T>` surface, monomorphized the
            // way `codegen_c.rs` does it. The element type comes from whichever
            // of the three places it can: the argument itself for the
            // associated `Box.of(v)`, the call's own result type for
            // `Box.empty()`, and the receiver for the four methods (through
            // `types::box_elem_of`, which looks through the `&Box<T>` a method
            // receiver is). Nothing else about the call is special — the
            // routines are written against the SysV signature
            // `infer.rs::box_sig` declares, so the argument marshalling below
            // is the ordinary one.
            let elem = match op {
                "of" => args.first().map(|a| a.ty.clone()),
                "empty" => crate::types::box_elem_of(ret_ty).cloned(),
                _ => args.first().and_then(|a| crate::types::box_elem_of(&a.ty)).cloned(),
            }
            .ok_or_else(|| {
                unsupported(&format!("the method `{}`", callee.replace('#', ".")))
            })?;
            if elem_size(&elem, &self.layouts).is_none() {
                return Err(unsupported(&format!(
                    "the `Box<{}>` ahead of `{}" ,
                    elem.display(),
                    callee.replace('#', ".")
                )));
            }
            let f = match op {
                "empty" => BoxFn::Empty(elem),
                "of" => BoxFn::Of(elem),
                "is_empty" => BoxFn::IsEmpty(elem),
                "get" => BoxFn::Get(elem, !matches!(ret_ty, Ty::Ref { .. })),
                "set" => BoxFn::Set(elem),
                "take" => BoxFn::Take(elem),
                _ => {
                    return Err(unsupported(&format!(
                        "the method `{}` on a `Box`",
                        callee.replace('#', ".")
                    )))
                }
            };
            let label = f.label();
            self.require_box_fn(f);
            label
        } else if let Some(op) = callee.strip_prefix("List#") {
            // MK7 §5's list slice: the `List<T>` surface this slice covers,
            // monomorphized the way `codegen_c.rs::write_list` does it. The
            // element type comes from whichever of the three places it can:
            // the argument itself for the associated `List.of(v)`, the call's
            // own result type for `List.empty()`, and the receiver for every
            // method (through `types::list_elem_of`, which looks through the
            // `&List<T>` a method receiver is). Only the *layout* has to exist
            // here — a `List` value is a 24-byte header whatever it points at —
            // while the element's own size is what the routines need, so it is
            // checked too and refused by name.
            let elem = match op {
                "of" => args.first().map(|a| a.ty.clone()),
                "empty" => crate::types::list_elem_of(ret_ty).cloned(),
                _ => args.first().and_then(|a| crate::types::list_elem_of(&a.ty)).cloned(),
            }
            .ok_or_else(|| {
                unsupported(&format!("the method `{}`", callee.replace('#', ".")))
            })?;
            if elem_size(&elem, &self.layouts).is_none() {
                return Err(unsupported(&format!(
                    "the `List<{}>` ahead of `{}`",
                    elem.display(),
                    callee.replace('#', ".")
                )));
            }
            let f = match op {
                "empty" => ListFn::Empty(elem),
                "of" => ListFn::Of(elem),
                "len" => ListFn::Len(elem),
                "is_empty" => ListFn::IsEmpty(elem),
                "push" => ListFn::Push(elem),
                "get" => ListFn::Get(elem, !matches!(ret_ty, Ty::Ref { .. })),
                "set" => ListFn::Set(elem),
                "pop" => ListFn::Pop(elem),
                "clear" => ListFn::Clear(elem),
                "insert" => ListFn::Insert(elem),
                "remove" => ListFn::Remove(elem),
                "reverse" => ListFn::Reverse(elem),
                // `sort`, `contains` and `slice` are the three S1 methods this
                // slice leaves out on purpose: each needs an element-level
                // comparison or clone the backend has no way to express yet.
                // Naming them is what keeps the gap visible — the alternative
                // is a call to a symbol nothing emits.
                _ => {
                    return Err(unsupported(&format!(
                        "the method `{}` on a `List`",
                        callee.replace('#', ".")
                    )))
                }
            };
            let label = f.label();
            self.require_list_fn(f);
            label
        } else if let Some(op) = callee.strip_prefix("Map#") {
            // MK7 §5's map slice: the `Map<K, V>` surface, monomorphized the
            // way `codegen_c.rs::write_map` does it. Both type arguments come
            // from whichever of the two places they can: the call's own result
            // type for the associated `Map.new()`, and the receiver for every
            // method (through `types::map_key_of`/`map_elem_of`, which look
            // through the `&Map<K, V>` a method receiver is). Only the *map's*
            // layout has to exist here — a map value is a 56-byte header
            // whatever it holds — while the key's and value's own sizes are
            // what the routines need, so both are checked and refused by name.
            //
            // `Map.empty()` never arrives: `infer.rs` lowers it to `Map#new`
            // (a map starts empty), which is the one spelling `write_map`
            // emits.
            let (key, value) = if op == "new" {
                (
                    crate::types::map_key_of(ret_ty).cloned(),
                    crate::types::map_elem_of(ret_ty).cloned(),
                )
            } else {
                (
                    args.first().and_then(|a| crate::types::map_key_of(&a.ty)).cloned(),
                    args.first().and_then(|a| crate::types::map_elem_of(&a.ty)).cloned(),
                )
            };
            let (Some(key), Some(value)) = (key, value) else {
                return Err(unsupported(&format!(
                    "the method `{}`",
                    callee.replace('#', ".")
                )));
            };
            if elem_size(&key, &self.layouts).is_none()
                || elem_size(&value, &self.layouts).is_none()
            {
                return Err(unsupported(&format!(
                    "the `Map<{}, {}>` ahead of `{}`",
                    key.display(),
                    value.display(),
                    callee.replace('#', ".")
                )));
            }
            // `get_or` and `values` exist only where the C backend emits them,
            // and `codegen_c.rs::write_map` gates both on
            // `types::list_clonable_elem(value)` — the same predicate
            // `infer.rs::map_sig` uses to offer the method at all. Checking it
            // again here (rather than assuming) is what keeps a call site from
            // ever naming a symbol nothing emitted: a divergence between the
            // two would be a refusal by name instead of an undefined symbol.
            if matches!(op, "get_or" | "values")
                && !crate::types::list_clonable_elem(&value)
            {
                return Err(unsupported(&format!(
                    "the method `{}` on a `Map` (its value type `{}` cannot be copied out, so 
                     the C backend emits no `{op}` for this instantiation)",
                    callee.replace('#', "."),
                    value.display()
                )));
            }
            let f = match op {
                "new" => MapFn::New(key, value),
                "insert" => MapFn::Insert(key, value),
                "has" => MapFn::Has(key, value),
                "get" => MapFn::Get(key, value, !matches!(ret_ty, Ty::Ref { .. })),
                "get_or" => MapFn::GetOr(key, value),
                "remove" => MapFn::Remove(key, value),
                "clear" => MapFn::Clear(key, value),
                "len" => MapFn::Len(key, value),
                "is_empty" => MapFn::IsEmpty(key, value),
                "keys" => MapFn::Keys(key, value),
                "values" => MapFn::Values(key, value),
                _ => {
                    return Err(unsupported(&format!(
                        "the method `{}` on a `Map`",
                        callee.replace('#', ".")
                    )))
                }
            };
            let label = f.label();
            self.require_map_fn(f);
            label
        } else if callee.starts_with("Bytes#")
            || callee.starts_with("String#")
            || callee.starts_with("View#")
        {
            return Err(unsupported(&format!(
                "the method `{}` on an owned value",
                callee.replace('#', ".")
            )));
        } else {
            match self.externs.get(callee).copied() {
                Some(Extern::Libc(sym)) => format!("{sym}@PLT"),
                Some(Extern::Routine(r)) => {
                    // Recorded, not emitted: a routine written into `self.out` here
                    // would land inside this function's body, mid-expression.
                    self.require_routine(r);
                    r.label().to_string()
                }
                None => Gen::symbol(callee),
            }
        };
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
        // MK7 §5 item 4's float-in-aggregate slice: the returned registers are
        // stored into `ret_dst` per eightbyte — `rax`/`rdx` for an `INTEGER`
        // eightbyte, `xmm0`/`xmm1` for an `SSE` one — with the two files
        // numbered independently. A `Mem`-class return needs nothing here: the
        // callee already wrote through the pointer this site passed in `rdi`.
        if let Some(class) = ret_class {
            if !class.is_mem() {
                if let Some(dest) = ret_dst {
                    if let Dest::Indirect(slot) = dest {
                        let _ = writeln!(self.out, "    mov r11, QWORD PTR [rbp{slot}]");
                    }
                    let n_eb = if class.eb(1).is_some() { 2 } else { 1 };
                    let (mut int_n, mut sse_n) = (0usize, 0usize);
                    for k in 0..n_eb {
                        let eb = class.eb(k).expect("k is in range");
                        let off = 8 * k as i32;
                        let (reg, w) = match eb.float_width() {
                            Some(w) => {
                                let r = FARG[sse_n];
                                sse_n += 1;
                                (r, Some(w))
                            }
                            None => {
                                let r = if int_n == 0 { "rax" } else { "rdx" };
                                int_n += 1;
                                (r, None)
                            }
                        };
                        let mem = match dest {
                            Dest::Direct(base) => format!("[rbp{}]", off_str(base + off)),
                            Dest::Indirect(_) => format!("[r11{}]", off_str(off)),
                            Dest::Global(index) => {
                                let label = self.pds[index].label.clone();
                                format!("[rip+{label}{}]", off_str(off))
                            }
                        };
                        match w {
                            Some(w) => {
                                let _ = writeln!(self.out, "    {} {mem}, {reg}", w.mov());
                            }
                            None => {
                                let _ = writeln!(self.out, "    mov QWORD PTR {mem}, {reg}");
                            }
                        }
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
        //
        // MK7 §5 item 5: a hoisted `&Bytes` temporary is freed *here* — after
        // the call has read it, and (for a value-returning call) after the
        // result is already somewhere a drop cannot reach, which is why the
        // result is parked first. This is `codegen_c.rs::call_value`'s
        // "evaluate the temporary, call, free, then yield the result".
        if !hoisted.is_empty() {
            let parked = if *ret_ty != Ty::Unit && ret_class.is_none() {
                Some(self.park(float_ty(ret_ty), frame))
            } else if ret_class.map_or(false, |c| c.is_int_one()) && ret_dst.is_none() {
                Some(self.park(None, frame))
            } else {
                None
            };
            for (slot, site) in &hoisted {
                self.emit_drop_slot(site, *slot, frame)?;
            }
            if let Some(p) = parked {
                self.unpark(p);
            }
        }
        Ok(())
    }

    /// MK7 §5 item 8: a numeric conversion, which `infer.rs` hands over as the
    /// synthetic callee `<src>#to_<dst>` (the same dotted name the C backend
    /// splits on). The C backend emits one inline as a cast — plus a range
    /// check that panics, whenever the source's range is not contained in the
    /// target's — so the two backends agree on which conversions can lose a
    /// value and on the message a losing one dies with.
    fn conversion(&mut self, callee: &str, arg: &Expr, frame: &mut Frame) -> Result<(), String> {
        let display = callee.replace('#', ".");
        let bad = || unsupported(&format!("the numeric conversion `{display}`"));
        let (src, dst) = callee
            .split_once("#to_")
            .expect("the caller matched `#to_` before calling this");
        let from = Ty::from_name(src).ok_or_else(bad)?;
        // A float target has no integer bounds, which is exactly how the C
        // backend decides there is nothing to check: `int_bounds(to)` is
        // `None`, so every integer-to-float conversion is a bare cast.
        if let Some(to_f) = Ty::from_name(dst).and_then(|t| float_ty(&t)) {
            let (from_w, from_signed) = int_ty(&from).ok_or_else(bad)?;
            self.expr(arg, frame)?;
            let mnem = if to_f == FWidth::F4 { "cvtsi2ss" } else { "cvtsi2sd" };
            if from_signed || from_w != Width::W8 {
                // Signed (at any width) or unsigned but narrower than 64 bits:
                // `rax` holds the value non-negative and fully extended — that
                // is the stack machine's own invariant — so the signed form is
                // the exact one.
                let _ = writeln!(self.out, "    {mnem} xmm0, rax");
            } else {
                // An unsigned 64-bit source can have bit 63 set, which
                // `cvtsi2sd` would read as a negative number. The sequence is
                // the classic `(x >> 1) | (x & 1)`, then a doubling — what
                // clang emits for `(double)(uint64_t)x` — and it is exact
                // because `x >> 1` loses one bit that is restored in the low
                // position before the conversion.
                let big = self.label("u64tof");
                let done = self.label("u64tof");
                self.out.push_str("    test rax, rax\n");
                let _ = writeln!(self.out, "    js {big}");
                let _ = writeln!(self.out, "    {mnem} xmm0, rax");
                let _ = writeln!(self.out, "    jmp {done}");
                let _ = writeln!(self.out, "{big}:");
                self.out.push_str(
                    "    mov rcx, rax\n    shr rcx, 1\n    and eax, 1\n    or rcx, rax\n",
                );
                let _ = writeln!(self.out, "    {mnem} xmm0, rcx");
                let double = if to_f == FWidth::F4 { "addss" } else { "addsd" };
                let _ = writeln!(self.out, "    {double} xmm0, xmm0");
                let _ = writeln!(self.out, "{done}:");
            }
            return Ok(());
        }
        let from_signed = int_ty(&from).ok_or_else(bad)?.1;
        let to = Ty::from_name(dst).ok_or_else(bad)?;
        let (to_w, to_signed) = int_ty(&to).ok_or_else(bad)?;
        self.expr(arg, frame)?;
        // `codegen_c.rs::numeric_conversion` falls back to a bare cast whenever
        // either side has no integer bounds. Today only the eight integer types
        // have a conversion at all (`infer.rs`'s `INT_TYPES`), so both are
        // always `Some` here — but a `char`/`bool` conversion added later should
        // come through as that same plain `(T)x` rather than as a compiler
        // panic, which is what an `expect` would have made it.
        if let (Some((s_min, s_max)), Some((t_min, t_max))) =
            (int_bounds(&from), int_bounds(&to))
        {
            if !(s_min >= t_min && s_max <= t_max) {
                // A value the target cannot represent panics rather than being
                // truncated. Every comparison is made at 64 bits, which is sound
                // because `rax` already holds the source value sign- or
                // zero-extended at its own width, and the *signedness of the
                // source* is what decides the comparison family: the `l`/`g` pair
                // for a signed source, `a`/`b` for an unsigned one (`u64` -> `u32`
                // with a huge value must not read as a negative number). Bounds go
                // through `r11` because an immediate like `u32::MAX` does not fit a
                // 32-bit encoding without being sign-extended, which would compare
                // against `-1`. `r11` is this module's `addr_of_struct` scratch and
                // is never live across an expression, which is exactly the position
                // this sequence is emitted in.
                let fails = self.label("conv");
                let ok = self.label("conv");
                if from_signed {
                    if t_min == 0 {
                        self.out.push_str("    test rax, rax\n");
                        let _ = writeln!(self.out, "    js {fails}");
                    } else if t_min > s_min {
                        let _ = writeln!(self.out, "    mov r11, {t_min}");
                        self.out.push_str("    cmp rax, r11\n");
                        let _ = writeln!(self.out, "    jl {fails}");
                    }
                    if t_max < s_max {
                        let _ = writeln!(self.out, "    mov r11, {t_max}");
                        self.out.push_str("    cmp rax, r11\n");
                        let _ = writeln!(self.out, "    jg {fails}");
                    }
                } else if t_max < s_max {
                    let _ = writeln!(self.out, "    mov r11, {t_max}");
                    self.out.push_str("    cmp rax, r11\n");
                    let _ = writeln!(self.out, "    ja {fails}");
                }
                let _ = writeln!(self.out, "    jmp {ok}");
                let _ = writeln!(self.out, "{fails}:");
                let msg = self.bytes_label(format!("{display}: value out of range").as_bytes());
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
                let _ = writeln!(self.out, "{ok}:");
            }
        }
        // The target's own invariant, always — not just after a check. A
        // same-width signedness change (`i32` -> `u32`) is a re-interpretation
        // that genuinely changes the 64-bit extension, and a narrower target
        // needs truncating to its width first, which is what `extend` does
        // (and what the C cast does when the result is read back).
        self.out.push_str(&to_w.extend(to_signed));
        Ok(())
    }

    /// A call to the hand-written `kelp_panic` routine, whose only argument is
    /// already in `rdi`. Never returns, so nothing after it needs `rax`.
    fn call_panic(&mut self) {
        self.needs_panic = true;
        self.out.push_str("    call kelp_panic\n");
    }

    /// MK7 §5 item 1: one of the routines this backend owns for an `extern
    /// fun` whose C body is more than a single library call — see
    /// `extern_target`.
    ///
    /// Each is written against the *declared* signature in `std/*.kp` (the
    /// same contract `externs.rs` keeps the C bodies honest about), so the call
    /// site needs nothing special: parameters are where the extern's own
    /// declaration puts them, and the result is in `rax`.
    ///
    /// `strlen` is reached through the PLT the same way `kelp_panic` reaches
    /// `fprintf`. A routine that can panic sets `needs_panic`, so `kelp_panic`
    /// is emitted exactly when something calls it.
    fn emit_routine(&mut self, r: Routine) {
        let label = r.label();
        let _ = writeln!(self.out, "    .type {label}, @function\n{label}:");
        match r {
            // `if (i >= strlen(s)) panic("str.byte: index out of bounds");
            //  return (uint8_t)s[i];` — the check `externs.rs` writes, for the
            // same message: the byte read is the one place a `str` index can
            // run past the end, and the C backend panics rather than reading
            // whatever follows the NUL.
            Routine::StrByteAtRaw => {
                let msg = self.bytes_label(b"str.byte: index out of bounds");
                // `s` and `i` both have to survive `strlen`, which is entitled
                // to clobber every caller-saved register (`rsi` included), so
                // both go into callee-saved ones. The extra eight bytes keep
                // `rsp` sixteen-byte aligned at the `call`.
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n");
                self.out.push_str("    call strlen@PLT\n");
                self.out.push_str("    cmp r12, rax\n    jae .Lstr_byte_at_oob\n");
                self.out.push_str("    movzx eax, BYTE PTR [rbx+r12]\n");
                self.out.push_str("    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lstr_byte_at_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // `if (from > strlen(s)) panic("str.substring: index out of
            // bounds"); return s + from;` — note `>`, not `>=`: a slice that
            // starts exactly at the NUL is the empty string, not an error.
            Routine::StrSliceFromRaw => {
                let msg = self.bytes_label(b"str.substring: index out of bounds");
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov r12, rdi\n    mov rbx, rsi\n");
                self.out.push_str("    call strlen@PLT\n");
                self.out.push_str("    cmp rbx, rax\n    ja .Lstr_slice_from_oob\n");
                self.out.push_str("    lea rax, [r12+rbx]\n");
                self.out.push_str("    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lstr_slice_from_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            // ---- MK7 §5's next slice: the three `str` parsers -------------
            //
            // Each is its `externs.rs` body, translated: the empty and
            // whitespace checks first (before any frame is set up, so the
            // early refusal is a bare `xor eax, eax; ret`), then one libc
            // conversion, then the checks that it consumed the whole string,
            // then one store through the caller's own `mut` out-parameter.
            // `s` and `out` arrive in `rdi`/`rsi` from the declared
            // signature, so nothing about the call marshalling had to change.
            // `rbx`/`r12` hold `out` and `s` across the call, `[rsp]` is
            // `end`, and — for the two integer parsers — `[rsp+8]` is the
            // `errno` location, fetched once and read back after the
            // conversion. That last check is what makes an out-of-range
            // number a failure instead of a saturated success, and it is why
            // `parse_i64("9223372036854775808")` is `Err`.
            Routine::StrParseI64Raw => {
                self.out.push_str("    cmp BYTE PTR [rdi], 0\n    je .Lpi64_fail_early\n    mov rax, rdi\n");
                self.out.push_str(".Lpi64_ws:\n    movzx ecx, BYTE PTR [rax]\n    test cl, cl\n    je .Lpi64_go\n");
                self.out.push_str("    cmp cl, 32\n    je .Lpi64_fail_early\n    cmp cl, 9\n    je .Lpi64_fail_early\n");
                self.out.push_str("    cmp cl, 10\n    je .Lpi64_fail_early\n    cmp cl, 13\n    je .Lpi64_fail_early\n");
                self.out.push_str("    add rax, 1\n    jmp .Lpi64_ws\n.Lpi64_fail_early:\n    xor eax, eax\n    ret\n");
                self.out.push_str(".Lpi64_go:\n    push rbx\n    push r12\n    sub rsp, 24\n");
                self.out.push_str("    mov rbx, rsi\n    mov r12, rdi\n    mov QWORD PTR [rsp], 0\n    call __errno_location@PLT\n");
                self.out.push_str("    mov QWORD PTR [rsp+8], rax\n    mov DWORD PTR [rax], 0\n");
                self.out.push_str("    mov rdi, r12\n    mov rsi, rsp\n    mov edx, 10\n    call strtoll@PLT\n");
                self.out.push_str("    mov rcx, QWORD PTR [rsp]\n    cmp rcx, r12\n    je .Lpi64_fail\n");
                self.out.push_str("    cmp BYTE PTR [rcx], 0\n    jne .Lpi64_fail\n");
                self.out.push_str("    mov rdx, QWORD PTR [rsp+8]\n    cmp DWORD PTR [rdx], 0\n    jne .Lpi64_fail\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov eax, 1\n    jmp .Lpi64_done\n");
                self.out.push_str(".Lpi64_fail:\n    xor eax, eax\n.Lpi64_done:\n    add rsp, 24\n    pop r12\n    pop rbx\n    ret\n");
            }
            // The same body with `strtoull`, plus the one extra rejection the
            // C source has: a leading `-` is not a number, not a negative
            // one.
            Routine::StrParseU64Raw => {
                self.out.push_str("    cmp BYTE PTR [rdi], 0\n    je .Lpu64_fail_early\n");
                self.out.push_str("    cmp BYTE PTR [rdi], 45\n    je .Lpu64_fail_early\n    mov rax, rdi\n");
                self.out.push_str(".Lpu64_ws:\n    movzx ecx, BYTE PTR [rax]\n    test cl, cl\n    je .Lpu64_go\n");
                self.out.push_str("    cmp cl, 32\n    je .Lpu64_fail_early\n    cmp cl, 9\n    je .Lpu64_fail_early\n");
                self.out.push_str("    cmp cl, 10\n    je .Lpu64_fail_early\n    cmp cl, 13\n    je .Lpu64_fail_early\n");
                self.out.push_str("    add rax, 1\n    jmp .Lpu64_ws\n.Lpu64_fail_early:\n    xor eax, eax\n    ret\n");
                self.out.push_str(".Lpu64_go:\n    push rbx\n    push r12\n    sub rsp, 24\n");
                self.out.push_str("    mov rbx, rsi\n    mov r12, rdi\n    mov QWORD PTR [rsp], 0\n    call __errno_location@PLT\n");
                self.out.push_str("    mov QWORD PTR [rsp+8], rax\n    mov DWORD PTR [rax], 0\n");
                self.out.push_str("    mov rdi, r12\n    mov rsi, rsp\n    mov edx, 10\n    call strtoull@PLT\n");
                self.out.push_str("    mov rcx, QWORD PTR [rsp]\n    cmp rcx, r12\n    je .Lpu64_fail\n");
                self.out.push_str("    cmp BYTE PTR [rcx], 0\n    jne .Lpu64_fail\n");
                self.out.push_str("    mov rdx, QWORD PTR [rsp+8]\n    cmp DWORD PTR [rdx], 0\n    jne .Lpu64_fail\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov eax, 1\n    jmp .Lpu64_done\n");
                self.out.push_str(".Lpu64_fail:\n    xor eax, eax\n.Lpu64_done:\n    add rsp, 24\n    pop r12\n    pop rbx\n    ret\n");
            }
            // `strtod`, and no `errno` — the C body does not read it, because
            // `strtod` reports an out-of-range value as `HUGE_VAL` rather than
            // as a failure. The store is the float form of the same one
            // instruction: `movsd` straight through the caller's pointer.
            Routine::StrParseF64Raw => {
                self.out.push_str("    cmp BYTE PTR [rdi], 0\n    je .Lpf64_fail_early\n    mov rax, rdi\n");
                self.out.push_str(".Lpf64_ws:\n    movzx ecx, BYTE PTR [rax]\n    test cl, cl\n    je .Lpf64_go\n");
                self.out.push_str("    cmp cl, 32\n    je .Lpf64_fail_early\n    cmp cl, 9\n    je .Lpf64_fail_early\n");
                self.out.push_str("    cmp cl, 10\n    je .Lpf64_fail_early\n    cmp cl, 13\n    je .Lpf64_fail_early\n");
                self.out.push_str("    add rax, 1\n    jmp .Lpf64_ws\n.Lpf64_fail_early:\n    xor eax, eax\n    ret\n");
                self.out.push_str(".Lpf64_go:\n    push rbx\n    push r12\n    sub rsp, 24\n");
                self.out.push_str("    mov rbx, rsi\n    mov r12, rdi\n    mov QWORD PTR [rsp], 0\n");
                self.out.push_str("    mov rdi, r12\n    mov rsi, rsp\n    call strtod@PLT\n");
                self.out.push_str("    mov rcx, QWORD PTR [rsp]\n    cmp rcx, r12\n    je .Lpf64_fail\n");
                self.out.push_str("    cmp BYTE PTR [rcx], 0\n    jne .Lpf64_fail\n");
                self.out.push_str("    movsd QWORD PTR [rbx], xmm0\n    mov eax, 1\n    jmp .Lpf64_done\n");
                self.out.push_str(".Lpf64_fail:\n    xor eax, eax\n.Lpf64_done:\n    add rsp, 24\n    pop r12\n    pop rbx\n    ret\n");
            }
            // `isnan(x)`: the comparison sets the parity flag only when the
            // operand is unordered, which is exactly a NaN.
            Routine::MathIsNanRaw => {
                self.out.push_str("    ucomisd xmm0, xmm0\n    setp al\n    movzx eax, al\n    ret\n");
            }
            // `isfinite(x)`: finite iff the exponent field is not all ones.
            // `movq` moves the bits into the integer file, where the field is
            // a shift and a mask — `isfinite` is a macro rather than a symbol,
            // which is why this is a routine instead of a `call`.
            Routine::MathIsFiniteRaw => {
                self.out.push_str(
                    "    movq rax, xmm0\n    shr rax, 52\n    and eax, 2047\n    cmp eax, 2047\n    setne al\n    movzx eax, al\n    ret\n",
                );
            }
            // ---- MK7 §5's heap slice: the `Bytes` runtime -------------------
            //
            // Every routine below is a transliteration of the matching C body
            // in `codegen_c.rs`'s RUNTIME, including its panic message and its
            // bounds check. The `self` pointer is `rdi` for each (the receiver
            // is the first argument, exactly as the C signature says), `rax`
            // carries a scalar result, and a `Bytes`-returning routine writes
            // its 24 bytes through `rdi` — the SysV hidden-pointer convention
            // the call site already uses for a `MEMORY`-class return, so it
            // passes the destination in `rdi` and the first real argument in
            // `rsi`.
            Routine::BufReserve => {
                // kelp_buf_reserve(data**, len, cap*, extra), reached with the
                // whole `kelp_Bytes*` in `rdi` instead of three separate
                // pointers, because every caller has one. Doubling (or jumping
                // to 8) keeps appending amortized O(1); `realloc` may move the
                // buffer, so `rbx` holds the struct and `r14` the old length,
                // both callee-saved so they survive the call.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n    push r14\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n");
                self.out.push_str("    mov r14, QWORD PTR [rbx+8]\n");
                self.out.push_str("    mov r12, r14\n    add r12, rsi\n");
                self.out.push_str("    cmp r12, QWORD PTR [rbx+16]\n    jbe .Lbuf_done\n");
                self.out.push_str("    mov r13, QWORD PTR [rbx+16]\n");
                self.out.push_str("    test r13, r13\n    jne .Lbuf_have\n    mov r13d, 8\n");
                self.out.push_str(".Lbuf_have:\n    cmp r13, r12\n    jae .Lbuf_grow\n    add r13, r13\n    jmp .Lbuf_have\n");
                self.out.push_str(".Lbuf_grow:\n    mov rdi, QWORD PTR [rbx]\n    lea rsi, [r13+1]\n    call realloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lbuf_oom\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    mov BYTE PTR [rax+r14], 0\n");
                self.out.push_str(".Lbuf_done:\n    add rsp, 8\n    pop r14\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lbuf_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesDrop => {
                // kelp_bytes_drop(kelp_Bytes* b): free the buffer, clear the
                // pointer and the length, so a second drop (which the move
                // checker makes unreachable anyway) frees nothing.
                self.out.push_str("    push rdi\n    mov rdi, QWORD PTR [rdi]\n    call free@PLT\n");
                self.out.push_str("    pop rdi\n    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+8], 0\n    ret\n");
            }
            Routine::BytesAllocRaw => {
                // kelp_bytes_alloc_raw(n): malloc(n + 1), zero the whole
                // `n + 1` bytes, and hand back `{data, n, n}` — the zero-fill
                // and the invariant that `data[len] == 0` are what make the
                // result a valid empty-of-content `str` too.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    lea rdi, [r12+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lalloc_oom\n");
                self.out.push_str("    mov rdi, rax\n    xor esi, esi\n    lea rdx, [r12+1]\n    call memset@PLT\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r12\n    mov QWORD PTR [rbx+16], r12\n");
                self.out.push_str("    mov rax, rbx\n    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lalloc_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesEmpty => {
                // kelp_Bytes__empty: one allocated zero byte rather than NULL,
                // so `data[len] == 0` holds and `%s` can never be handed NULL.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    mov rbx, rdi\n    mov edi, 1\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lempty_oom\n");
                self.out.push_str("    mov BYTE PTR [rax], 0\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], 0\n    mov QWORD PTR [rbx+16], 0\n");
                self.out.push_str("    mov rax, rbx\n    pop rbx\n    ret\n");
                self.out.push_str(".Lempty_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesFromStr => {
                // kelp_Bytes__from_str(s): n = strlen(s); malloc(n + 1);
                // memcpy including the NUL, so the copy is terminated too.
                // `memcpy` returns its destination, which is why `rax` is the
                // new buffer again after the call.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rdi, r12\n    call strlen@PLT\n");
                self.out.push_str("    mov r13, rax\n    lea rdi, [r13+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lfrom_str_oom\n");
                self.out.push_str("    mov rdi, rax\n    mov rsi, r12\n    lea rdx, [r13+1]\n    call memcpy@PLT\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r13\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    mov rax, rbx\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lfrom_str_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesLen => {
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n    ret\n");
            }
            Routine::BytesIsEmpty => {
                self.out.push_str("    cmp QWORD PTR [rdi+8], 0\n    sete al\n    movzx eax, al\n    ret\n");
            }
            Routine::BytesGet => {
                let msg = self.bytes_label(b"Bytes.get: index out of bounds");
                self.out.push_str("    cmp rsi, QWORD PTR [rdi+8]\n    jae .Lbytes_get_oob\n");
                self.out.push_str("    mov rax, QWORD PTR [rdi]\n    movzx eax, BYTE PTR [rax+rsi]\n    ret\n");
                self.out.push_str(".Lbytes_get_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesSet => {
                let msg = self.bytes_label(b"Bytes.set: index out of bounds");
                self.out.push_str("    cmp rsi, QWORD PTR [rdi+8]\n    jae .Lbytes_set_oob\n");
                self.out.push_str("    mov rax, QWORD PTR [rdi]\n    mov BYTE PTR [rax+rsi], dl\n    ret\n");
                self.out.push_str(".Lbytes_set_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesClone => {
                // kelp_Bytes__clone(self): a fresh buffer of `self->len` bytes,
                // copied without the terminator (there is nothing to copy), and
                // re-terminated at `len`.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov r13, QWORD PTR [r12+8]\n    lea rdi, [r13+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lclone_oom\n");
                self.out.push_str("    mov rdi, rax\n    mov rsi, QWORD PTR [r12]\n    mov rdx, r13\n    call memcpy@PLT\n");
                self.out.push_str("    mov BYTE PTR [rax+r13], 0\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r13\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    mov rax, rbx\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lclone_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::BytesPush => {
                // kelp_Bytes__push(self, byte): reserve one byte, store it at
                // `len`, advance `len`, re-terminate.
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rdi, rbx\n    mov esi, 1\n");
                let _ = writeln!(self.out, "    call {}", Routine::BufReserve.label());
                self.out.push_str("    mov rax, QWORD PTR [rbx]\n    mov rcx, QWORD PTR [rbx+8]\n");
                self.out.push_str("    mov BYTE PTR [rax+rcx], r12b\n    add rcx, 1\n    mov QWORD PTR [rbx+8], rcx\n    mov BYTE PTR [rax+rcx], 0\n");
                self.out.push_str("    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
            }
            Routine::BytesAppend => {
                // kelp_Bytes__append(self, other): nothing to do for an empty
                // `other`; otherwise reserve `other->len`, `memcpy` it past
                // `self->len`, advance `self->len`, re-terminate.
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rax, QWORD PTR [r12+8]\n    test rax, rax\n    je .Lappend_done\n");
                self.out.push_str("    mov rdi, rbx\n    mov rsi, rax\n");
                let _ = writeln!(self.out, "    call {}", Routine::BufReserve.label());
                self.out.push_str("    mov rdi, QWORD PTR [rbx]\n    add rdi, QWORD PTR [rbx+8]\n");
                self.out.push_str("    mov rsi, QWORD PTR [r12]\n    mov rdx, QWORD PTR [r12+8]\n    call memcpy@PLT\n");
                self.out.push_str("    mov rax, QWORD PTR [r12+8]\n    add QWORD PTR [rbx+8], rax\n");
                self.out.push_str("    mov rcx, QWORD PTR [rbx]\n    mov rdx, QWORD PTR [rbx+8]\n    mov BYTE PTR [rcx+rdx], 0\n");
                self.out.push_str(".Lappend_done:\n    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
            }
            Routine::BytesClear => {
                self.out.push_str("    mov QWORD PTR [rdi+8], 0\n    mov rax, QWORD PTR [rdi]\n    test rax, rax\n    je .Lclear_done\n    mov BYTE PTR [rax], 0\n.Lclear_done:\n    ret\n");
            }
            // ---- MK7 §5's second heap slice: `String`, `View`, and the
            // shared helpers they need -------------------------------------
            //
            // Every routine below is a transliteration of the matching C body
            // in `codegen_c.rs`'s RUNTIME. The `String` ABI is the `Bytes` ABI
            // (`{data, len, cap}`), so a `String` routine is a `String`-typed
            // copy of the `Bytes` one; a `View` routine is different in kind:
            // a `View` is a `Two`-class value, so it arrives in a register
            // pair (`rdi`/`rsi`) and a `View`-returning routine leaves its
            // result in `rax`/`rdx`.
            Routine::Utf8Valid => {
                // kelp_utf8_valid(p, n): the well-formedness scan `from_bytes`
                // and `Bytes.as_str` both run. `i` is `rcx`, `cp` `rax`,
                // `extra` `r8`; every register used is caller-saved and the
                // routine makes no call, so nothing has to be preserved.
                self.out.push_str("    xor ecx, ecx\n.Lutf8_loop:\n    cmp rcx, rsi\n    jae .Lutf8_true\n");
                self.out.push_str("    movzx eax, BYTE PTR [rdi+rcx]\n    cmp eax, 0x80\n    jb .Lutf8_next\n");
                self.out.push_str("    mov edx, eax\n    and edx, 0xE0\n    cmp edx, 0xC0\n    je .Lutf8_two\n");
                self.out.push_str("    mov edx, eax\n    and edx, 0xF0\n    cmp edx, 0xE0\n    je .Lutf8_three\n");
                self.out.push_str("    mov edx, eax\n    and edx, 0xF8\n    cmp edx, 0xF0\n    je .Lutf8_four\n    jmp .Lutf8_false\n");
                self.out.push_str(".Lutf8_two:\n    mov r8d, 1\n    and eax, 0x1F\n    jmp .Lutf8_have\n");
                self.out.push_str(".Lutf8_three:\n    mov r8d, 2\n    and eax, 0x0F\n    jmp .Lutf8_have\n");
                self.out.push_str(".Lutf8_four:\n    mov r8d, 3\n    and eax, 0x07\n");
                self.out.push_str(".Lutf8_have:\n    lea r9, [rcx+r8]\n    cmp r9, rsi\n    jae .Lutf8_false\n    mov r10d, 1\n");
                self.out.push_str(".Lutf8_kloop:\n    cmp r10, r8\n    ja .Lutf8_kafter\n    lea r9, [rcx+r10]\n    movzx r11d, BYTE PTR [rdi+r9]\n");
                self.out.push_str("    mov edx, r11d\n    and edx, 0xC0\n    cmp edx, 0x80\n    jne .Lutf8_false\n    and r11d, 0x3F\n    shl rax, 6\n    or rax, r11\n");
                self.out.push_str("    add r10, 1\n    jmp .Lutf8_kloop\n.Lutf8_kafter:\n");
                self.out.push_str("    cmp r8, 1\n    jne .Lutf8_n1\n    cmp rax, 0x80\n    jb .Lutf8_false\n.Lutf8_n1:\n");
                self.out.push_str("    cmp r8, 2\n    jne .Lutf8_n2\n    cmp rax, 0x800\n    jb .Lutf8_false\n.Lutf8_n2:\n");
                self.out.push_str("    cmp r8, 3\n    jne .Lutf8_n3\n    cmp rax, 0x10000\n    jb .Lutf8_false\n.Lutf8_n3:\n");
                self.out.push_str("    cmp rax, 0x10FFFF\n    ja .Lutf8_false\n    cmp rax, 0xD800\n    jb .Lutf8_nsur\n    cmp rax, 0xDFFF\n    jbe .Lutf8_false\n");
                self.out.push_str(".Lutf8_nsur:\n    add rcx, r8\n    add rcx, 1\n    jmp .Lutf8_loop\n");
                self.out.push_str(".Lutf8_next:\n    add rcx, 1\n    jmp .Lutf8_loop\n");
                self.out.push_str(".Lutf8_true:\n    mov eax, 1\n    ret\n.Lutf8_false:\n    xor eax, eax\n    ret\n");
            }
            Routine::BytesAsStr => {
                // kelp_Bytes__as_str(self): check the buffer is UTF-8, then
                // hand its pointer out as a `str`. `rbx` holds `self` across
                // the check; the one push keeps `rsp` aligned at the call.
                let msg = self.bytes_label(b"Bytes.as_str: input is not valid utf-8");
                self.out.push_str("    push rbx\n    mov rbx, rdi\n");
                self.out.push_str("    mov rdi, QWORD PTR [rbx]\n    mov rsi, QWORD PTR [rbx+8]\n");
                let _ = writeln!(self.out, "    call {}", Routine::Utf8Valid.label());
                self.out.push_str("    test al, al\n    je .Lbytes_as_str_bad\n    mov rax, QWORD PTR [rbx]\n    pop rbx\n    ret\n");
                self.out.push_str(".Lbytes_as_str_bad:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StringEmpty => {
                // kelp_String__empty: as `kelp_Bytes__empty`, one allocated
                // zero byte so an empty string is never NULL.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    mov rbx, rdi\n    mov edi, 1\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lstring_empty_oom\n    mov BYTE PTR [rax], 0\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], 0\n    mov QWORD PTR [rbx+16], 0\n");
                self.out.push_str("    mov rax, rbx\n    pop rbx\n    ret\n.Lstring_empty_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StringFromStr => {
                // kelp_String__from_str(text): strlen, malloc(n + 1), memcpy
                // including the NUL.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rdi, r12\n    call strlen@PLT\n");
                self.out.push_str("    mov r13, rax\n    lea rdi, [r13+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lstring_from_str_oom\n");
                self.out.push_str("    mov rdi, rax\n    mov rsi, r12\n    lea rdx, [r13+1]\n    call memcpy@PLT\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r13\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    mov rax, rbx\n    pop r13\n    pop r12\n    pop rbx\n    ret\n.Lstring_from_str_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StringFromBytes => {
                // kelp_String__from_bytes(kelp_Bytes b): `b` is a MEMORY-class
                // argument in the caller's outgoing area (`[rbp+16]` from this
                // routine's own frame), so `rbp` is set up to reach it. On a
                // valid buffer the three fields transfer wholesale — no copy
                // — and the caller's binding is moved into the call.
                let msg = self.bytes_label(b"String.from_bytes: input is not valid utf-8");
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov rdi, QWORD PTR [rbp+16]\n    mov rsi, QWORD PTR [rbp+24]\n");
                let _ = writeln!(self.out, "    call {}", Routine::Utf8Valid.label());
                self.out.push_str("    test al, al\n    je .Lstring_from_bytes_bad\n");
                self.out.push_str("    mov rax, QWORD PTR [rbp+16]\n    mov QWORD PTR [rbx], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rbp+24]\n    mov QWORD PTR [rbx+8], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rbp+32]\n    mov QWORD PTR [rbx+16], rax\n");
                self.out.push_str("    add rsp, 8\n    pop rbx\n    pop rbp\n    ret\n.Lstring_from_bytes_bad:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StringIntoBytes => {
                // kelp_Bytes kelp_String__into_bytes(kelp_String self): the
                // `String` argument is a MEMORY-class value at `[rsp+8]` (the
                // hidden `Bytes` return pointer is in `rdi`), and the whole
                // 24 bytes transfer with no copy.
                self.out.push_str("    mov rax, QWORD PTR [rsp+8]\n    mov QWORD PTR [rdi], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+16]\n    mov QWORD PTR [rdi+8], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+24]\n    mov QWORD PTR [rdi+16], rax\n    ret\n");
            }
            Routine::StringLen => {
                self.out.push_str("    mov rax, QWORD PTR [rdi+8]\n    ret\n");
            }
            Routine::StringIsEmpty => {
                self.out.push_str("    cmp QWORD PTR [rdi+8], 0\n    sete al\n    movzx eax, al\n    ret\n");
            }
            Routine::StringCharCount => {
                // kelp_String__char_count(self): a byte is a character start
                // unless `(byte & 0xC0) == 0x80`.
                self.out.push_str("    xor eax, eax\n    xor ecx, ecx\n    mov rdx, QWORD PTR [rdi+8]\n    mov r8, QWORD PTR [rdi]\n");
                self.out.push_str(".Lchar_count_loop:\n    cmp rcx, rdx\n    jae .Lchar_count_done\n");
                self.out.push_str("    movzx r9d, BYTE PTR [r8+rcx]\n    mov r10d, r9d\n    and r10d, 0xC0\n    cmp r10d, 0x80\n    je .Lchar_count_next\n    add rax, 1\n");
                self.out.push_str(".Lchar_count_next:\n    add rcx, 1\n    jmp .Lchar_count_loop\n.Lchar_count_done:\n    ret\n");
            }
            Routine::StringClone => {
                // kelp_String__clone(self): a fresh `len`-byte buffer, copy,
                // re-terminate.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov r13, QWORD PTR [r12+8]\n    lea rdi, [r13+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lstring_clone_oom\n");
                self.out.push_str("    mov rdi, rax\n    mov rsi, QWORD PTR [r12]\n    mov rdx, r13\n    call memcpy@PLT\n");
                self.out.push_str("    mov BYTE PTR [rax+r13], 0\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r13\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    mov rax, rbx\n    pop r13\n    pop r12\n    pop rbx\n    ret\n.Lstring_clone_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StringPush => {
                // kelp_String__push(self, ch): reserve one byte, store it at
                // `len`, advance, re-terminate.
                self.out.push_str("    push rbx\n    push r12\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rdi, rbx\n    mov esi, 1\n");
                let _ = writeln!(self.out, "    call {}", Routine::BufReserve.label());
                self.out.push_str("    mov rax, QWORD PTR [rbx]\n    mov rcx, QWORD PTR [rbx+8]\n");
                self.out.push_str("    mov BYTE PTR [rax+rcx], r12b\n    add rcx, 1\n    mov QWORD PTR [rbx+8], rcx\n    mov BYTE PTR [rax+rcx], 0\n");
                self.out.push_str("    add rsp, 8\n    pop r12\n    pop rbx\n    ret\n");
            }
            Routine::StringPushStr => {
                // kelp_String__push_str(self, text): nothing to do for an
                // empty `text`; otherwise reserve `n`, memcpy, advance,
                // re-terminate.
                self.out.push_str("    push rbx\n    push r12\n    push r13\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov rdi, r12\n    call strlen@PLT\n    mov r13, rax\n    test r13, r13\n    je .Lstring_push_str_done\n");
                self.out.push_str("    mov rdi, rbx\n    mov rsi, r13\n");
                let _ = writeln!(self.out, "    call {}", Routine::BufReserve.label());
                self.out.push_str("    mov rdi, QWORD PTR [rbx]\n    add rdi, QWORD PTR [rbx+8]\n    mov rsi, r12\n    mov rdx, r13\n    call memcpy@PLT\n");
                self.out.push_str("    mov rax, QWORD PTR [rbx+8]\n    add rax, r13\n    mov QWORD PTR [rbx+8], rax\n    mov rcx, QWORD PTR [rbx]\n    mov BYTE PTR [rcx+rax], 0\n");
                self.out.push_str(".Lstring_push_str_done:\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
            }
            Routine::StringWithCapacity => {
                // kelp_String__with_capacity(n): `{NULL, 0, 0}` then a reserve
                // of `n`, built in a 24-byte stack temporary and copied out.
                // `n == 0` leaves the reserve nothing to do, so the temporary's
                // `data` is still NULL and an empty `String` would point at it —
                // the C body had the same hole, and both fill it the same way:
                // by allocating the one NUL byte `kelp_String__empty` allocates.
                // `BytesEmpty` already builds exactly that value (`{malloc(1),
                // 0, 0}` with the byte zeroed), so it is called rather than the
                // four instructions being written out a second time. Both
                // routines it calls are declared in `require_routine`, not
                // here — see that function for why.
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    sub rsp, 24\n");
                self.out.push_str("    mov rbx, rdi\n    mov QWORD PTR [rsp], 0\n    mov QWORD PTR [rsp+8], 0\n    mov QWORD PTR [rsp+16], 0\n");
                self.out.push_str("    mov rdi, rsp\n");
                let _ = writeln!(self.out, "    call {}", Routine::BufReserve.label());
                self.out.push_str("    cmp QWORD PTR [rsp], 0\n    jne .Lstr_cap_have\n");
                self.out.push_str("    mov rdi, rsp\n");
                let _ = writeln!(self.out, "    call {}", Routine::BytesEmpty.label());
                self.out.push_str(".Lstr_cap_have:\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp]\n    mov QWORD PTR [rbx], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+8]\n    mov QWORD PTR [rbx+8], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+16]\n    mov QWORD PTR [rbx+16], rax\n");
                self.out.push_str("    add rsp, 24\n    pop rbx\n    pop rbp\n    ret\n");
            }
            Routine::StringClear => {
                self.out.push_str("    mov QWORD PTR [rdi+8], 0\n    mov rax, QWORD PTR [rdi]\n    test rax, rax\n    je .Lstring_clear_done\n    mov BYTE PTR [rax], 0\n.Lstring_clear_done:\n    ret\n");
            }
            Routine::StringAsStr => {
                self.out.push_str("    mov rax, QWORD PTR [rdi]\n    ret\n");
            }
            Routine::StringView => {
                // kelp_View kelp_String__view(const kelp_String* self): no
                // `strlen` — the length is already there.
                self.out.push_str("    mov rax, QWORD PTR [rdi]\n    mov rdx, QWORD PTR [rdi+8]\n    ret\n");
            }
            Routine::StringDrop => {
                // kelp_string_drop(kelp_String* s): as `kelp_bytes_drop`.
                self.out.push_str("    push rdi\n    mov rdi, QWORD PTR [rdi]\n    call free@PLT\n");
                self.out.push_str("    pop rdi\n    mov QWORD PTR [rdi], 0\n    mov QWORD PTR [rdi+8], 0\n    ret\n");
            }
            Routine::ViewLen => {
                // kelp_View__len(self): a `View` is `{rdi, rsi}`, so the
                // length is already in `rsi`.
                self.out.push_str("    mov rax, rsi\n    ret\n");
            }
            Routine::ViewIsEmpty => {
                self.out.push_str("    cmp rsi, 0\n    sete al\n    movzx eax, al\n    ret\n");
            }
            Routine::ViewGet => {
                // kelp_View__get(self, i): `self` in `rdi`/`rsi`, `i` in
                // `rdx`; length-aware, so no NUL is required.
                let msg = self.bytes_label(b"View.get: index out of bounds");
                self.out.push_str("    cmp rdx, rsi\n    jae .Lview_get_oob\n    movzx eax, BYTE PTR [rdi+rdx]\n    ret\n");
                self.out.push_str(".Lview_get_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::ViewSlice => {
                // kelp_View__slice(self, from, to): `self` in `rdi`/`rsi`,
                // `from` `rdx`, `to` `rcx`; result in `rax`/`rdx`.
                let msg = self.bytes_label(b"View.slice: index out of bounds");
                self.out.push_str("    cmp rdx, rcx\n    ja .Lview_slice_oob\n    cmp rcx, rsi\n    ja .Lview_slice_oob\n");
                self.out.push_str("    mov r8, rcx\n    sub r8, rdx\n    lea rax, [rdi+rdx]\n    mov rdx, r8\n    ret\n");
                self.out.push_str(".Lview_slice_oob:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::ViewEq => {
                // kelp_View__eq(a, b): `a` in `rdi`/`rsi`, `b` in `rdx`/`rcx`.
                // Length first (`memcmp` over the shorter prefix would call a
                // prefix equal), then a length-aware `memcmp`.
                self.out.push_str("    cmp rsi, rcx\n    jne .Lview_eq_false\n    test rsi, rsi\n    je .Lview_eq_true\n");
                self.out.push_str("    xchg rsi, rdx\n    sub rsp, 8\n    call memcmp@PLT\n    add rsp, 8\n");
                self.out.push_str("    test eax, eax\n    sete al\n    movzx eax, al\n    ret\n");
                self.out.push_str(".Lview_eq_true:\n    mov eax, 1\n    ret\n.Lview_eq_false:\n    xor eax, eax\n    ret\n");
            }
            Routine::ViewToString => {
                // kelp_String kelp_View__to_string(kelp_View self): hidden
                // `String` destination in `rdi`, the view in `rsi`/`rdx`.
                // Copies exactly `len` bytes — never `strlen`.
                let msg = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    push r12\n    push r13\n    sub rsp, 8\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    mov r13, rdx\n    lea rdi, [r13+1]\n    call malloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Lview_to_string_oom\n    test r13, r13\n    je .Lview_to_string_term\n");
                self.out.push_str("    mov rdi, rax\n    mov rsi, r12\n    mov rdx, r13\n    call memcpy@PLT\n");
                self.out.push_str(".Lview_to_string_term:\n    mov BYTE PTR [rax+r13], 0\n");
                self.out.push_str("    mov QWORD PTR [rbx], rax\n    mov QWORD PTR [rbx+8], r13\n    mov QWORD PTR [rbx+16], r13\n");
                self.out.push_str("    add rsp, 8\n    pop r13\n    pop r12\n    pop rbx\n    pop rbp\n    ret\n.Lview_to_string_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{msg}]");
                self.call_panic();
            }
            Routine::StrView => {
                // kelp_View kelp_str__view(const char* s): the one place a
                // length is derived from a `str`. A NULL `s` views as
                // `{NULL, 0}`, as the C body says.
                self.out.push_str("    push rbx\n    mov rbx, rdi\n    test rdi, rdi\n    je .Lstr_view_null\n");
                self.out.push_str("    call strlen@PLT\n    mov rdx, rax\n    mov rax, rbx\n    pop rbx\n    ret\n");
                self.out.push_str(".Lstr_view_null:\n    xor edx, edx\n    mov rax, rbx\n    pop rbx\n    ret\n");
            }
            // ---- MK7 §5's second heap slice: the `str`/`fmt` externs -------
            Routine::StrFromF64Raw => {
                // std/str.from_f64_raw(x, decimals): `snprintf(NULL, 0, ..)`
                // to size the result, allocate that many bytes through the
                // `Bytes` allocator, format into them, then transfer the
                // buffer to a `String`. `x` is saved on the stack because
                // `snprintf` is entitled to clobber `xmm0`.
                let fmt = self.bytes_label(b"%.*f");
                let bad = self.bytes_label(b"str.from_f64: formatting failed");
                let utf8 = self.bytes_label(b"String.from_bytes: input is not valid utf-8");
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    push r12\n    push r13\n    sub rsp, 40\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n    movsd QWORD PTR [rsp], xmm0\n");
                self.out.push_str("    xor edi, edi\n    xor esi, esi\n");
                let _ = writeln!(self.out, "    lea rdx, [rip+{fmt}]");
                self.out.push_str("    mov rcx, r12\n    movsd xmm0, QWORD PTR [rsp]\n    mov eax, 1\n    call snprintf@PLT\n");
                self.out.push_str("    test eax, eax\n    js .Lfrom_f64_bad\n    mov r13, rax\n");
                let _ = writeln!(self.out, "    lea rdi, [rsp+8]\n    mov rsi, r13\n    call {}", Routine::BytesAllocRaw.label());
                self.out.push_str("    mov rdi, QWORD PTR [rsp+8]\n    mov rsi, QWORD PTR [rsp+16]\n    add rsi, 1\n");
                let _ = writeln!(self.out, "    lea rdx, [rip+{fmt}]");
                self.out.push_str("    mov rcx, r12\n    movsd xmm0, QWORD PTR [rsp]\n    mov eax, 1\n    call snprintf@PLT\n");
                self.out.push_str("    mov rdi, QWORD PTR [rsp+8]\n    mov rsi, QWORD PTR [rsp+16]\n");
                let _ = writeln!(self.out, "    call {}", Routine::Utf8Valid.label());
                self.out.push_str("    test al, al\n    je .Lfrom_f64_utf8\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+8]\n    mov QWORD PTR [rbx], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+16]\n    mov QWORD PTR [rbx+8], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+24]\n    mov QWORD PTR [rbx+16], rax\n");
                self.out.push_str("    add rsp, 40\n    pop r13\n    pop r12\n    pop rbx\n    pop rbp\n    ret\n");
                self.out.push_str(".Lfrom_f64_bad:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{bad}]");
                self.call_panic();
                self.out.push_str(".Lfrom_f64_utf8:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{utf8}]");
                self.call_panic();
            }
            Routine::FmtHexRaw => {
                // std/fmt.hex_raw(n): one `snprintf(buf, 32, "%llx", n)` into
                // a stack buffer, then `String.from_str(buf)`.
                let fmt = self.bytes_label(b"%llx");
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    push rbx\n    sub rsp, 40\n");
                self.out.push_str("    mov rbx, rdi\n    mov rcx, rsi\n    mov rdi, rsp\n    mov esi, 40\n");
                let _ = writeln!(self.out, "    lea rdx, [rip+{fmt}]");
                self.out.push_str("    xor eax, eax\n    call snprintf@PLT\n    mov rdi, rbx\n    mov rsi, rsp\n");
                let _ = writeln!(self.out, "    call {}", Routine::StringFromStr.label());
                self.out.push_str("    add rsp, 40\n    pop rbx\n    pop rbp\n    ret\n");
            }
            Routine::FmtBytesRaw => {
                // std/fmt.bytes_raw(b): an empty buffer is an empty `String`;
                // otherwise a `len * 3 - 1`-byte buffer, each byte as two
                // lowercase hex digits with a space between, then the buffer
                // handed to a `String` (its bytes are ASCII, so the check
                // always passes — it is kept because the C body runs it).
                let hexlab = self.bytes_label(b"0123456789abcdef");
                let utf8 = self.bytes_label(b"String.from_bytes: input is not valid utf-8");
                self.out.push_str("    push rbx\n    push r12\n    push r13\n    push r14\n    sub rsp, 40\n");
                self.out.push_str("    mov rbx, rdi\n    mov r12, rsi\n");
                let _ = writeln!(self.out, "    lea r14, [rip+{hexlab}]");
                self.out.push_str("    mov r13, QWORD PTR [r12+8]\n    test r13, r13\n    jne .Lbytes_raw_go\n");
                self.out.push_str("    mov rdi, rbx\n");
                let _ = writeln!(self.out, "    call {}", Routine::StringEmpty.label());
                self.out.push_str("    add rsp, 40\n    pop r14\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lbytes_raw_go:\n    lea rsi, [r13+r13*2]\n    sub rsi, 1\n    lea rdi, [rsp]\n");
                let _ = writeln!(self.out, "    call {}", Routine::BytesAllocRaw.label());
                self.out.push_str("    xor r8, r8\n    xor r9, r9\n");
                self.out.push_str(".Lbytes_raw_loop:\n    cmp r8, r13\n    jae .Lbytes_raw_done\n    test r8, r8\n    je .Lbytes_raw_nosep\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp]\n    mov BYTE PTR [rax+r9], 0x20\n    add r9, 1\n");
                self.out.push_str(".Lbytes_raw_nosep:\n    mov rax, QWORD PTR [r12]\n    movzx ecx, BYTE PTR [rax+r8]\n");
                self.out.push_str("    mov edx, ecx\n    shr edx, 4\n    and edx, 0xF\n    movzx edx, BYTE PTR [r14+rdx]\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp]\n    mov BYTE PTR [rax+r9], dl\n    add r9, 1\n");
                self.out.push_str("    mov edx, ecx\n    and edx, 0xF\n    movzx edx, BYTE PTR [r14+rdx]\n");
                self.out.push_str("    mov BYTE PTR [rax+r9], dl\n    add r9, 1\n    add r8, 1\n    jmp .Lbytes_raw_loop\n");
                self.out.push_str(".Lbytes_raw_done:\n    mov rdi, QWORD PTR [rsp]\n    mov rsi, QWORD PTR [rsp+8]\n");
                let _ = writeln!(self.out, "    call {}", Routine::Utf8Valid.label());
                self.out.push_str("    test al, al\n    je .Lbytes_raw_utf8\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp]\n    mov QWORD PTR [rbx], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+8]\n    mov QWORD PTR [rbx+8], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rsp+16]\n    mov QWORD PTR [rbx+16], rax\n");
                self.out.push_str("    add rsp, 40\n    pop r14\n    pop r13\n    pop r12\n    pop rbx\n    ret\n");
                self.out.push_str(".Lbytes_raw_utf8:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{utf8}]");
                self.call_panic();
            }
            // MK7 §5's list slice, and the one routine in it that is not
            // per-instantiation:
            //
            // ```c
            // static void reserve(List* self, size_t want, size_t elem) {
            //   if (want <= self->cap) return;
            //   size_t cap = self->cap == 0 ? 4 : self->cap;
            //   while (cap < want) cap *= 2;
            //   void* p = realloc(self->data, cap * elem);
            //   if (p == NULL) kelp_panic("out of memory");
            //   self->data = p; self->cap = cap;
            // }
            // ```
            //
            // `codegen_c.rs` writes one of these per element type because the
            // element is baked into its C signature. Here it is a third
            // argument, which makes it the *only* list routine that does not
            // need monomorphizing — the one place the two backends' symbol sets
            // deliberately differ. The frame is four slots (`self`, `want`, the
            // element size, and the doubled capacity while `realloc` runs),
            // which keeps everything live across the call in memory rather than
            // in a caller-saved register `realloc` is entitled to clobber.
            Routine::ListReserve => {
                let oom = self.bytes_label(b"out of memory");
                self.out.push_str("    push rbp\n    mov rbp, rsp\n    sub rsp, 32\n");
                self.out.push_str(
                    "    mov QWORD PTR [rbp-8], rdi\n    mov QWORD PTR [rbp-16], rsi\n    mov QWORD PTR [rbp-24], rdx\n",
                );
                self.out.push_str("    mov rax, QWORD PTR [rdi+16]\n");
                self.out.push_str("    cmp rsi, rax\n    jbe .Llist_reserve_done\n");
                self.out.push_str("    test rax, rax\n    jne .Llist_reserve_grow\n    mov eax, 4\n");
                self.out.push_str(".Llist_reserve_grow:\n    cmp rax, rsi\n    jae .Llist_reserve_alloc\n    add rax, rax\n    jmp .Llist_reserve_grow\n");
                self.out.push_str(".Llist_reserve_alloc:\n    mov QWORD PTR [rbp-32], rax\n    imul rax, rdx\n    mov rsi, rax\n");
                self.out.push_str("    mov rdi, QWORD PTR [rbp-8]\n    mov rdi, QWORD PTR [rdi]\n    call realloc@PLT\n");
                self.out.push_str("    test rax, rax\n    je .Llist_reserve_oom\n");
                self.out.push_str("    mov rdi, QWORD PTR [rbp-8]\n    mov QWORD PTR [rdi], rax\n");
                self.out.push_str("    mov rax, QWORD PTR [rbp-32]\n    mov QWORD PTR [rdi+16], rax\n");
                self.out.push_str(".Llist_reserve_done:\n    mov rsp, rbp\n    pop rbp\n    ret\n");
                self.out.push_str(".Llist_reserve_oom:\n");
                let _ = writeln!(self.out, "    lea rdi, [rip+{oom}]");
                self.call_panic();
            }
            // `uint64_t kelp_map_hash(const char* data, size_t len)` — FNV-1a
            // over the key's bytes, `codegen_c.rs:502-510` instruction for
            // instruction, constants included. `rdi` is the data, `rsi` the
            // length, the result lands in `rax`; no call is made, so nothing
            // has to be preserved and the four registers used are all
            // caller-saved.
            Routine::MapHash => {
                self.out.push_str("    movabs rax, 1469598103934665603\n    xor rcx, rcx\n");
                self.out.push_str(".Lmap_hash_loop:\n    cmp rcx, rsi\n    jae .Lmap_hash_done\n");
                self.out.push_str("    movzx edx, BYTE PTR [rdi+rcx]\n    xor rax, rdx\n");
                self.out.push_str("    movabs r8, 1099511628211\n    imul rax, r8\n");
                self.out.push_str("    add rcx, 1\n    jmp .Lmap_hash_loop\n.Lmap_hash_done:\n    ret\n");
            }
            // `bool kelp_map_key_eq(const kelp_String* key, const char* data,
            // size_t len)` — a length check before the `memcmp`, because a
            // `String` key may hold bytes `strlen` would stop at
            // (`codegen_c.rs:511-518`). The `sub rsp, 8`/`add rsp, 8` pair is
            // the same alignment shim `View__eq` uses around `memcmp`.
            Routine::MapKeyEq => {
                self.out.push_str("    cmp QWORD PTR [rdi+8], rdx\n    jne .Lmap_key_eq_false\n");
                self.out.push_str("    test rdx, rdx\n    je .Lmap_key_eq_true\n");
                self.out.push_str("    mov rdi, QWORD PTR [rdi]\n    sub rsp, 8\n    call memcmp@PLT\n    add rsp, 8\n");
                self.out.push_str("    test eax, eax\n    sete al\n    movzx eax, al\n    ret\n");
                self.out.push_str(".Lmap_key_eq_true:\n    mov eax, 1\n    ret\n.Lmap_key_eq_false:\n    xor eax, eax\n    ret\n");
            }
            // `uint64_t kelp_map_mix(uint64_t x)` — the scalar-key finalizer
            // (`codegen_c.rs:520-528`): two shifts-xors and one multiply, so
            // keys differing in one bit land in different buckets. The result
            // is in `rax`, the argument in `rdi`.
            Routine::MapMix => {
                self.out.push_str("    mov rax, rdi\n    mov rcx, rax\n    shr rcx, 33\n    xor rax, rcx\n");
                self.out.push_str("    movabs r8, 0xff51afd7ed558ccd\n    imul rax, r8\n");
                self.out.push_str("    mov rcx, rax\n    shr rcx, 33\n    xor rax, rcx\n    ret\n");
            }
        }
        let _ = writeln!(self.out, "    .size {label}, .-{label}");
    }

    /// The one piece of runtime this backend owns so far: the C backend's
    /// `_Noreturn static void kelp_panic(const char* msg)`, written out as
    /// instructions rather than C text. Its *observable* behaviour is what the
    /// fixtures assert on, so it has to match exactly — `panic: <msg>` on
    /// stderr, then `exit(1)` — and `stderr` is reached through its GOT entry
    /// because a dynamically linked program has no fixed address for a libc
    /// data symbol. `and rsp, -16` is not optional: this is called from
    /// mid-expression, where the stack machine's spills may have left `rsp`
    /// eight bytes off, and `fprintf` is entitled to assume the SysV alignment.
    /// Emitted only for a program that can panic at all, so this costs a
    /// program that cannot panic nothing.
    fn emit_panic_routine(&mut self) {
        let fmt = self.bytes_label(b"panic: %s\n");
        self.out.push_str("    .type kelp_panic, @function\nkelp_panic:\n");
        self.out.push_str("    push rbp\n    mov rbp, rsp\n    and rsp, -16\n");
        // `rdi` holds the message; `fprintf(FILE*, fmt, msg)` wants it in `rdx`.
        self.out.push_str("    mov rdx, rdi\n");
        let _ = writeln!(self.out, "    lea rsi, [rip+{fmt}]");
        self.out.push_str("    mov rdi, QWORD PTR stderr@GOTPCREL[rip]\n");
        self.out.push_str("    mov rdi, QWORD PTR [rdi]\n");
        self.out.push_str("    xor eax, eax\n");
        self.out.push_str("    call fprintf@PLT\n");
        self.out.push_str("    mov edi, 1\n    call exit@PLT\n");
        self.out.push_str("    .size kelp_panic, .-kelp_panic\n");
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
    ///
    /// MK7 §5 item 8: a float argument travels in `xmm0` rather than `rsi`, and
    /// a variadic call has to say how many *vector* registers it used in `al`
    /// — zero for everything else, one here. `f32` is promoted to `double`,
    /// which is the `(double)` cast the C backend writes into the argument, and
    /// what `%f` reads.
    fn builtin_print(
        &mut self,
        newline: bool,
        args: &[Expr],
        frame: &mut Frame,
    ) -> Result<(), String> {
        let [arg] = args else {
            return Err(unsupported("a print that is not given exactly one value"));
        };
        // MK7 §5 item 4: an owned `Bytes`/`String` (or a borrow of one) prints
        // through `%.*s`, which takes the length and the pointer as *two*
        // arguments, and a temporary is freed right after the `printf` — see
        // `print_buffer`. `printable_ty` deliberately does not cover them,
        // because their specifier alone is not enough to render them.
        if bytes_ty(&arg.ty) || string_ty(&arg.ty) {
            return self.print_buffer(newline, arg, frame);
        }
        // A `View` is the same `%.*s` shape but from a *value* (a pair of
        // registers), and it is a borrow, so nothing is freed — see
        // `print_view`.
        if view_ty(&arg.ty) {
            return self.print_view(newline, arg, frame);
        }
        let spec = printable_ty(&arg.ty)
            .ok_or_else(|| unsupported(&format!("printing a value of type `{}`", arg.ty.display())))?;
        let mut fmt = spec.as_bytes().to_vec();
        if newline {
            fmt.push(b'\n');
        }
        self.expr(arg, frame)?;
        // A float is already where `printf` reads it (`xmm0`), promoted to
        // `double`, and counts as one vector register; everything else moves
        // from `rax` to `rsi`, with `bool` turned into a `%s` pointer on the
        // way.
        let vector_args = if let Some(width) = float_ty(&arg.ty) {
            if width == FWidth::F4 {
                self.out.push_str("    cvtss2sd xmm0, xmm0\n");
            }
            1
        } else {
            self.out.push_str("    mov rsi, rax\n");
            if arg.ty == Ty::Bool {
                self.bool_str("rsi");
            }
            0
        };
        let label = self.bytes_label(&fmt);
        let _ = writeln!(self.out, "    lea rdi, [rip+{label}]");
        let _ = writeln!(self.out, "    mov eax, {vector_args}");
        call_align_begin(&mut self.out, frame.sp_slot);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out, frame.sp_slot);
        Ok(())
    }

    /// `print(b)` / `print!(b)` where `b` is an owned `Bytes`/`String` or a
    /// borrow of one (MK7 §5 item 4): `printf("%.*s", (int)len,
    /// (const char*)data)`, exactly the C backend's shape. `%.*s` draws the
    /// length from the first two variadic slots (`rsi` an `int`, `rdx` the
    /// pointer), so `rsi` and `rdx` are loaded from the buffer rather than
    /// `rsi` from `rax` the way a one-specifier print works.
    ///
    /// An *owned* `Bytes`/`String` here is a temporary nothing else owns, so
    /// it is staged in a scratch slot and freed after the `printf` — the
    /// lifetime `codegen_c.rs::call_value` gives the same temporary. A borrow
    /// is used where it already lives and is never freed.
    fn print_buffer(&mut self, newline: bool, arg: &Expr, frame: &mut Frame) -> Result<(), String> {
        let owned = match arg.ty {
            Ty::Bytes => Some(DropSite::Routine(Routine::BytesDrop)),
            Ty::String => Some(DropSite::Routine(Routine::StringDrop)),
            _ => None,
        };
        let owned_slot = if let Some(_) = owned {
            let layout = struct_layout(&arg.ty, &self.layouts)
                .expect("Bytes and String have synthesized layouts");
            let slot = frame.scratch(align_up(layout.size, 8));
            self.eval_struct_into(arg, frame, Dest::Direct(slot))?;
            let _ = writeln!(self.out, "    lea rax, [rbp{slot}]");
            Some(slot)
        } else {
            // A `&Bytes`/`&String`: `rax` holds the pointer to the buffer.
            self.expr(arg, frame)?;
            None
        };
        // `data` into `rdx`, `len` into `rsi` — read before either is
        // overwritten, and `rax` (the buffer's address) is not needed after.
        self.out.push_str("    mov rdx, QWORD PTR [rax]\n");
        self.out.push_str("    mov rsi, QWORD PTR [rax+8]\n");
        let mut fmt = b"%.*s".to_vec();
        if newline {
            fmt.push(b'\n');
        }
        let label = self.bytes_label(&fmt);
        let _ = writeln!(self.out, "    lea rdi, [rip+{label}]");
        self.out.push_str("    xor eax, eax\n");
        call_align_begin(&mut self.out, frame.sp_slot);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out, frame.sp_slot);
        if let (Some(slot), Some(site)) = (owned_slot, owned) {
            self.emit_drop_slot(&site, slot, frame)?;
        }
        Ok(())
    }

    /// `print(v)` / `print!(v)` where `v` is a `View` (MK7 §5's second heap
    /// slice): the same `%.*s` shape but the value is a `{ptr, len}` pair
    /// rather than a pointer to a buffer. It is staged in a scratch slot
    /// (`eval_struct_into`, because a `Two`-class value has no single register
    /// `self.expr` could leave it in) and its two fields read out. A `View`
    /// borrows, so nothing is freed — the one place this differs from
    /// `print_buffer`.
    fn print_view(&mut self, newline: bool, arg: &Expr, frame: &mut Frame) -> Result<(), String> {
        let layout = struct_layout(&arg.ty, &self.layouts).expect("View has a synthesized layout");
        let slot = frame.scratch(align_up(layout.size, 8));
        self.eval_struct_into(arg, frame, Dest::Direct(slot))?;
        // `ptr` into `rdx`, `len` into `rsi` — the `%.*s` variadic slots.
        self.out.push_str("    mov rdx, QWORD PTR [rbp");
        self.out.push_str(&off_str(slot));
        self.out.push_str("]\n    mov rsi, QWORD PTR [rbp");
        self.out.push_str(&off_str(slot + 8));
        self.out.push_str("]\n");
        let mut fmt = b"%.*s".to_vec();
        if newline {
            fmt.push(b'\n');
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
        // MK7 §5 item 8: the vector and integer argument files are numbered
        // independently, so each interpolation's destination depends only on
        // how many *of its own class* came before it — the same split a call's
        // argument marshalling makes. `al` is set to the number of vector
        // registers used, which is what a variadic call requires.
        let value_regs = ["rsi", "rdx", "rcx", "r8", "r9"];
        let mut dests: Vec<usize> = Vec::with_capacity(values.len());
        let mut int_slot = 0usize;
        let mut float_slot = 0usize;
        for value in &values {
            if float_ty(&value.ty).is_some() {
                dests.push(float_slot);
                float_slot += 1;
            } else {
                dests.push(int_slot);
                int_slot += 1;
            }
        }
        if int_slot > value_regs.len() {
            return Err(unsupported("a print with more than five non-float interpolations"));
        }
        if float_slot > FARG.len() {
            return Err(unsupported("a print with more than eight float interpolations"));
        }

        for value in &values {
            self.expr(value, frame)?;
            // A float's bits are what travels on the stack: `xmm0` is the
            // stack machine's working register and a later interpolation's
            // evaluation is free to overwrite it. Promoting `f32` to `double`
            // *before* the push is what lets the pop be one `movq`.
            if let Some(width) = float_ty(&value.ty) {
                if width == FWidth::F4 {
                    self.out.push_str("    cvtss2sd xmm0, xmm0\n");
                }
                self.out.push_str("    movq rax, xmm0\n");
            }
            self.out.push_str("    push rax\n");
        }
        let fmt_label = self.bytes_label(&fmt);
        for (i, dest) in dests.iter().enumerate().rev() {
            if float_ty(&values[i].ty).is_some() {
                self.out.push_str("    pop rax\n");
                let _ = writeln!(self.out, "    movq {}, rax", FARG[*dest]);
                continue;
            }
            let _ = writeln!(self.out, "    pop {}", value_regs[*dest]);
            // A `bool` interpolated into a template needs its pointer built
            // after the pop, in the register it landed in.
            if values[i].ty == Ty::Bool {
                self.bool_str(value_regs[*dest]);
            }
        }
        let _ = writeln!(self.out, "    lea rdi, [rip+{fmt_label}]");
        let _ = writeln!(self.out, "    mov eax, {float_slot}");
        call_align_begin(&mut self.out, frame.sp_slot);
        self.out.push_str("    call printf@PLT\n");
        call_align_end(&mut self.out, frame.sp_slot);
        Ok(())
    }
}

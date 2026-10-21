/// Which integer binary operator to emit, named the way the language reads it
/// rather than the way LLVM opcodes are numbered. A `Copy` stand-in for the C
/// API's enum, so `codegen_llvm.rs` never reaches into `llvm-sys`.
///
/// `sdiv`/`udiv`, `srem`/`urem` and `ashr`/`lshr` are separate variants, not a
/// signedness parameter: the signedness the language sees lives in the type and
/// the lowerer picks the instruction. No variant carries `nsw` or `nuw`, because
/// the language promises wrapping (MK7 §3.4) and a flagged form promises the
/// opposite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntBinOp {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
}

/// Which floating-point binary operator to emit. Kept apart from [`IntBinOp`]
/// because LLVM keeps them apart: `fadd` and `add` are different instructions and
/// neither is a signedness choice of the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatBinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// The one ordering every atomic instruction is emitted at (MK9-S3-M1 stage
/// A1). The surface states exactly one (`Ordering.SeqCst`), so the lowering has
/// nothing to choose between and the C API's enum is named once, here, rather
/// than at each of the four instructions that carry it.
pub const SEQ_CST: llvm_sys::LLVMAtomicOrdering =
    llvm_sys::LLVMAtomicOrdering::LLVMAtomicOrderingSequentiallyConsistent;

/// One atomic read-modify-write (`atomicrmw`). Named in our own terms so
/// `codegen_llvm.rs` never has to name an `llvm-sys` type: this module is the
/// only client of that crate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AtomicRmw {
    Xchg,
    Add,
    Sub,
    And,
    Or,
    Xor,
}

impl AtomicRmw {
    pub(super) fn as_raw(self) -> llvm_sys::LLVMAtomicRMWBinOp {
        use llvm_sys::LLVMAtomicRMWBinOp as Op;
        match self {
            AtomicRmw::Xchg => Op::LLVMAtomicRMWBinOpXchg,
            AtomicRmw::Add => Op::LLVMAtomicRMWBinOpAdd,
            AtomicRmw::Sub => Op::LLVMAtomicRMWBinOpSub,
            AtomicRmw::And => Op::LLVMAtomicRMWBinOpAnd,
            AtomicRmw::Or => Op::LLVMAtomicRMWBinOpOr,
            AtomicRmw::Xor => Op::LLVMAtomicRMWBinOpXor,
        }
    }
}

/// An integer comparison. The signed and unsigned spellings are separate because
/// LLVM makes them a different instruction (`icmp slt` vs `icmp ult`), while the
/// language decides by the operand's type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntCmp {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
    Ult,
    Ule,
    Ugt,
    Uge,
}

/// A floating-point comparison, the ordered forms (`o` in LLVM's spelling).
///
/// `One` is `!=` as the language means it: true when the operands are unordered,
/// so a NaN compares not-equal to everything, itself included. The other five
/// are all false on a NaN, which is what an ordered predicate gives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RealCmp {
    Oeq,
    One,
    Olt,
    Ole,
    Ogt,
    Oge,
}

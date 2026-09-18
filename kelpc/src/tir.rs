//! Typed intermediate representation produced by `infer.rs` and consumed by
//! `codegen_c.rs` (and, from M3 onward, `borrowck.rs`).

use crate::ast::{BinOp, UnOp};
use crate::lexer::Span;
use crate::types::{EnumDef, StructDef, Ty};

#[derive(Debug, Clone)]
pub struct Program {
    pub structs: Vec<StructDef>,
    /// `enum` declarations (A-1), in the same merged declaration order as
    /// `structs` — each carries its `order`, so `codegen_c.rs` can emit the
    /// two kinds in one interleaved sequence. An `enum` value is a
    /// `Ty::Struct` at the type level; this table is what makes it a tagged
    /// union instead of a record.
    pub enums: Vec<EnumDef>,
    /// Passdowns (M5.2) in initialization order — module load order, then
    /// declaration order within a module. `codegen_c.rs` emits one global
    /// per entry plus a generated init function that runs before `main`'s
    /// body.
    pub pds: Vec<Pd>,
    pub functions: Vec<Function>,
    /// Capability grants, one per (module, domain) that holds any (M7).
    /// `codegen_c.rs` emits each as constant data and checks the arguments of
    /// every gated call against it.
    pub caps: Vec<CapsGrant>,
    /// Function key -> the capability domain of the module that declares it.
    /// Only gated modules appear, so this is also the set of calls that need
    /// a runtime allowlist check.
    pub gated: std::collections::HashMap<String, String>,
}

/// One module's patterns for one capability domain.
#[derive(Debug, Clone)]
pub struct CapsGrant {
    pub module: String,
    pub domain: String,
    pub patterns: Vec<String>,
}

/// One checked passdown. `name` is the internal `pd#<qualified>` name shared
/// by the generated C global and the hidden parameter every referencing
/// function receives (see `infer::PdInfo`).
#[derive(Debug, Clone)]
pub struct Pd {
    pub name: String,
    /// Module-prefixed name, for diagnostics and comments in the C output.
    pub qualified: String,
    pub value_ty: Ty,
    pub init: Expr,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub body: Block,
    /// Canonical id of the module this function was declared in — needed so
    /// `borrowck.rs` can tag its diagnostics with the right source file (a
    /// `Span` is only meaningful inside its own file).
    pub module: String,
    /// `extern fun` (M6.2): the body below is synthetic (empty) and
    /// `codegen_c.rs` writes the compiler-supplied C for it instead.
    pub extern_: bool,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The block's value: the last statement's expression, if it was written
    /// without a trailing semicolon. `None` means the block's value is unit.
    pub tail: Option<Box<Expr>>,
    /// Source span of the block. Carried so `codegen_c.rs` can look up which
    /// owned locals this block has to drop at its end (see
    /// `borrowck::DropPlan`) — the drop points are keyed by span rather than
    /// by position, so the two passes don't have to walk in lockstep.
    pub span: Span,
}

impl Block {
    pub fn ty(&self) -> Ty {
        self.tail.as_ref().map(|e| e.ty.clone()).unwrap_or(Ty::Unit)
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        init: Expr,
    },
    While {
        cond: Expr,
        body: Block,
    },
    Return {
        value: Option<Expr>,
        /// Source span of the `return` statement itself, for the same reason
        /// `Block::span` exists: dropping the frame's live owned locals
        /// before returning is keyed by it.
        span: Span,
    },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    /// Source span of the surface-syntax expression this was checked from —
    /// needed for borrowck's "value moved here" / "used here" diagnostics.
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    Ident(String),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: String,
        args: Vec<Expr>,
    },
    FieldAccess {
        base: Box<Expr>,
        field: String,
    },
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    /// Construction of one `enum` variant (A-1): `Shape.Circle(r = 1.0)`.
    /// `variant` is the variant's index within the enum, and `fields` is one
    /// value per payload field in declaration order. Emitted as a C compound
    /// literal whose `tag` is that index.
    EnumLit {
        enum_name: String,
        variant: usize,
        fields: Vec<Expr>,
    },
    /// Whether the scrutinee's live tag is `variant` — the test that a
    /// `when` arm lowers to. `ty` is `bool`.
    EnumTag {
        expr: Box<Expr>,
        variant: usize,
    },
    /// One payload field of one variant, read out of a value whose tag is
    /// known to be `variant`. A *place* (like `FieldAccess`), which is what
    /// lets an owned payload be bound as a shared borrow rather than moved
    /// out — v1 has no partial moves.
    EnumField {
        base: Box<Expr>,
        variant: usize,
        field: usize,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    Block(Block),
    /// A borrow: either an explicit `&expr`/`&mut expr` from the surface
    /// syntax, or one synthesized by `infer::check_call_arg` when a plain
    /// place is passed to a `Ty::Ref`-typed parameter (call sites never
    /// need to write `&` themselves — the callee's signature decides).
    Ref {
        mutable: bool,
        expr: Box<Expr>,
    },
    Deref {
        expr: Box<Expr>,
    },
    /// A `print`/`print!` of an interpolated string template, kept as its
    /// pieces so `codegen_c.rs` can emit exactly one `printf` with one
    /// generated format string (rather than concatenating anything, which v1
    /// has no owned string type to do).
    Template {
        /// `true` for `print(...)`, `false` for `print!(...)`.
        newline: bool,
        pieces: Vec<TemplatePiece>,
    },
}

/// One piece of a template: literal text, or an interpolated value that has
/// already been checked to be printable.
#[derive(Debug, Clone)]
pub enum TemplatePiece {
    Text(String),
    Value(Expr),
}

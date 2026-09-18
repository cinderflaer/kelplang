//! Typed intermediate representation produced by `infer.rs` and consumed by
//! `codegen_c.rs` (and, from M3 onward, `borrowck.rs`).

use crate::ast::{BinOp, UnOp};
use crate::lexer::Span;
use crate::types::{StructDef, Ty};

#[derive(Debug, Clone)]
pub struct Program {
    pub structs: Vec<StructDef>,
    pub functions: Vec<Function>,
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
}

#[derive(Debug, Clone, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The block's value: the last statement's expression, if it was written
    /// without a trailing semicolon. `None` means the block's value is unit.
    pub tail: Option<Box<Expr>>,
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
}

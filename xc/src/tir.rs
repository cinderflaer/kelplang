//! Typed intermediate representation produced by `infer.rs` and consumed by
//! `codegen_c.rs` (and, from M3 onward, `borrowck.rs`).

use crate::ast::{BinOp, UnOp};
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
}

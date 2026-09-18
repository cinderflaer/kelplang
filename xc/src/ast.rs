use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    /// `impl "path"` lines, in source order. One file's list only; the
    /// multi-file loader in `resolve.rs` owns the whole module graph.
    pub impls: Vec<ImplDecl>,
    pub datacs: Vec<DatacDecl>,
    pub functions: Vec<FunctionDecl>,
}

/// One `impl "shapes/circle"` / `impl "time" as t` line.
#[derive(Debug, Clone)]
pub struct ImplDecl {
    /// The string literal's contents, verbatim (`shapes/circle` — no `.x`).
    pub path: String,
    /// `Some` only when written `as alias`.
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct DatacDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// Canonical id of the module this was declared in: `""` for the entry
    /// module, otherwise its path relative to `src/` without the `.x`
    /// suffix (`"shapes/circle"`). Set by `resolve.rs`; the parser leaves it
    /// empty. Used to attribute a diagnostic to the right source file — a
    /// `Span` is only meaningful inside its own file.
    pub module: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: String,
    /// `var` field: reassignable through a `var`-bound value. `val` field:
    /// never reassignable, regardless of the binding's own mutability.
    pub mutable: bool,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub name: String,
    /// `Some` for `fun Type.name(...)` / `fun mut Type.name(...)` —
    /// receiver-form methods. The receiver is *not* one of `params`; it's
    /// a separate implicit first argument, matching how the surface syntax
    /// keeps it outside the parens.
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    /// The `.R: T` clause. `None` means `Unit`.
    pub return_type: Option<TypeExpr>,
    pub body: FunctionBody,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// `Type` (shared) or `mut Type` (exclusive) before the `.` in a receiver
/// declaration, e.g. `Circle` in `fun Circle.area()` or `Circle` +
/// `mutable: true` in `fun mut Circle.grow()`.
#[derive(Debug, Clone)]
pub struct Receiver {
    pub type_name: String,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FunctionBody {
    Block(Block),
    /// `fun f(...).R: T = expr` — sugar checked/lowered as if it were
    /// `fun f(...).R: T { return expr }`.
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    /// `true` when the param type was written `mut T` — requests an
    /// exclusive borrow instead of the implicit shared borrow (for a
    /// `datac` type) or plain by-value passing (for a Copy type).
    pub mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        /// `true` for `var`, `false` for `val`.
        mutable: bool,
        ty: Option<TypeExpr>,
        init: Expr,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// An expression used as a full statement. Whether it's the block's
    /// *tail* (value-producing) or purely a side-effecting statement is
    /// decided positionally — textually the block's last statement is the
    /// tail — there's no `;` to mark it either way now that statements are
    /// newline-terminated.
    Expr {
        expr: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// One argument at a call site: `f(1, 2)` (positional) or `f(x = 1)`
/// (named) or a mix of both.
#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum Expr {
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StringLit(String, Span),
    BoolLit(bool, Span),
    Ident(String, Span),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// Also covers `datac` construction (`Circle(r = 2.0)`) — at parse
    /// time a call and a construction look identical; `infer.rs`
    /// disambiguates by whether `callee` names a function or a `datac`.
    Call {
        callee: String,
        args: Vec<CallArg>,
        span: Span,
    },
    FieldAccess {
        base: Box<Expr>,
        field: String,
        span: Span,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Box<ElseBranch>>,
        span: Span,
    },
    Block(Block),
    /// `&expr` (shared) / `&mut expr` (exclusive) — taking a borrow of a
    /// local. Only legal outside signatures; parameter/receiver positions
    /// use the bare/`mut T` forms instead (parsed via `Param.mutable`).
    Ref {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
    /// `*expr` — dereferencing a borrow.
    Deref {
        expr: Box<Expr>,
        span: Span,
    },
    /// `receiver.method(args)` — syntactically indistinguishable from
    /// `.field` access until the parser sees a `(` right after the method
    /// name. `infer.rs` resolves `method` against the receiver expression's
    /// type and lowers this to a plain `Call` (the receiver becomes the
    /// first argument, auto-borrowed exactly like any other `Ty::Ref`
    /// parameter) — no new `tir`/codegen shape needed.
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<CallArg>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum ElseBranch {
    Block(Block),
    If(Expr),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit(_, s)
            | Expr::FloatLit(_, s)
            | Expr::StringLit(_, s)
            | Expr::BoolLit(_, s)
            | Expr::Ident(_, s)
            | Expr::Unary { span: s, .. }
            | Expr::Binary { span: s, .. }
            | Expr::Call { span: s, .. }
            | Expr::FieldAccess { span: s, .. }
            | Expr::Assign { span: s, .. }
            | Expr::If { span: s, .. }
            | Expr::Ref { span: s, .. }
            | Expr::Deref { span: s, .. }
            | Expr::MethodCall { span: s, .. } => *s,
            Expr::Block(b) => b.span,
        }
    }
}

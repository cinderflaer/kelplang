use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    /// `impl "path"` lines, in source order. One file's list only; the
    /// multi-file loader in `resolve.rs` owns the whole module graph.
    pub impls: Vec<ImplDecl>,
    /// `const NAME: T = expr` lines, in source order.
    pub consts: Vec<ConstDecl>,
    /// `pd val`/`pd var` lines, in source order (canonical position: after
    /// `impl`, before `datac`/`fun`).
    pub pds: Vec<PdDecl>,
    pub datacs: Vec<DatacDecl>,
    /// `enum Name { Variant(...) ... }` declarations, in source order, on the
    /// same footing as `datacs`: both declare a type, and either may follow
    /// the other (a field or payload that names another type by value still
    /// has to name an earlier one).
    pub enums: Vec<EnumDecl>,
    pub functions: Vec<FunctionDecl>,
}

/// One `const NAME: T = expr` — a top-level compile-time constant (M6.3).
/// The type is mandatory (a `const` is a declared contract, unlike `val`),
/// and the initializer has to fold to a literal, so there is nothing to
/// allocate and nothing to drop.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub init: Expr,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// One `pd val name = expr` / `pd var name: T = expr` — a passdown
/// (`plan/syntax.md` §9): a top-level binding a function can name without
/// being passed it.
#[derive(Debug, Clone)]
pub struct PdDecl {
    pub name: String,
    /// `true` for `pd var`. Per the plan's resolution of §9's open question,
    /// writability is confined to the top level either way — inside a
    /// function body a passdown is read-only — so this only records which
    /// keyword was written (and is what a future top-level statement form
    /// would consult).
    pub mutable: bool,
    /// The `: T`, if written; otherwise inferred from `init`.
    pub ty: Option<TypeExpr>,
    pub init: Expr,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// One `impl "shapes/circle"` / `impl "time" as t` line, plus its optional
/// `with caps(...)` grants (M7).
#[derive(Debug, Clone)]
pub struct ImplDecl {
    /// The string literal's contents, verbatim (`shapes/circle` — no `.kp`).
    pub path: String,
    /// `Some` only when written `as alias`.
    pub alias: Option<String>,
    /// `with caps(domain = [patterns...])` grants. Empty when the clause is
    /// absent. Two jobs in one syntax: it *delegates* the domain to the
    /// imported module, and it is how the importing module states which
    /// patterns it is willing to hand over.
    pub caps: Vec<CapGrant>,
    pub span: Span,
}

/// One `fs = ["/var/log/**"]` inside a `with caps(...)` clause.
#[derive(Debug, Clone)]
pub struct CapGrant {
    pub domain: String,
    /// Quoted patterns, verbatim. Absolute, glob-only (`*` within a segment,
    /// `**` as a whole segment), and never containing `..`.
    pub patterns: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct DatacDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// Declaration index within this file, across `datac`s *and* `enum`s, so
    /// `resolve.rs` can rebuild one merged declaration order out of two
    /// lists (A-1). A field may only name an earlier type by value, and
    /// `codegen_c.rs` emits typedefs in that same order.
    pub order: usize,
    /// Canonical id of the module this was declared in: `""` for the entry
    /// module, otherwise its path relative to `src/` without the `.kp`
    /// suffix (`"shapes/circle"`). Set by `resolve.rs`; the parser leaves it
    /// empty. Used to attribute a diagnostic to the right source file — a
    /// `Span` is only meaningful inside its own file.
    pub module: String,
    pub span: Span,
}

/// One `enum Shape { Circle(val r: f64) Empty }` — a tagged union (A-1).
/// `variants` is never empty, and variant names are unique within the enum;
/// both are checked in `infer.rs`.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<VariantDecl>,
    /// Declaration index within this file, across `datac`s *and* `enum`s, so
    /// `resolve.rs` can rebuild one merged declaration order out of two
    /// lists (A-1). A field or payload may only name an earlier type by
    /// value, and `codegen_c.rs` emits typedefs in that same order.
    pub order: usize,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// One variant of an `enum`: a name, with zero or more payload fields.
#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
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
    /// `extern fun ...` — the body is supplied by the compiler (M6.2), so
    /// `body` is always `FunctionBody::Extern` here.
    pub extern_: bool,
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
    /// `extern fun f(...)` — no body in `.kp` at all; `codegen_c.rs` writes
    /// the compiler-supplied C body instead. Never reaches `check_function`'s
    /// block/expression paths.
    Extern,
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
    /// Type arguments, for the one generic the compiler knows: `List<T>`.
    /// Empty for every ordinary type name, and the parser only accepts them
    /// after a bare or dotted name — `List<i32>`, never `math.List<i32>`.
    /// `infer.rs` rejects arguments on a name that is not `List`, and more
    /// than one on `List` itself; v1 has no user-definable generics.
    pub args: Vec<TypeExpr>,
    /// `&T` was written with a leading `&`. Only meaningful in a *return*
    /// position (M6.3): `&str`/`&T` there is a borrow of the caller's value
    /// under the elision rule, whereas a parameter's `&str` is what `str`
    /// already is (a borrowed view passed by value).
    pub amp: bool,
    pub span: Span,
}

impl TypeExpr {
    /// The type as it was written, for diagnostics and the pretty-printer.
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.name.clone()
        } else {
            let args: Vec<String> = self.args.iter().map(|a| a.display()).collect();
            format!("{}<{}>", self.name, args.join(", "))
        }
    }
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
    /// A string literal with `$name` / `${expr}` interpolation, already split
    /// into parts by the lexer. Can only be consumed by `print`/`print!` in
    /// v1 — building a `str` value out of pieces needs the owned `String`
    /// type, which lands in M6 — so `infer.rs` rejects it anywhere else.
    Template {
        parts: Vec<TemplatePart>,
        span: Span,
    },
    /// `when expr { Variant(bind) -> value ... }` — pattern matching over an
    /// `enum` scrutinee (A-1, `plan/syntax.md` §6.3). Distinct from the
    /// boolean `when`, which has no scrutinee and is desugared by the parser
    /// (see `parser::parse_when`); this form is checked by `infer.rs`, which
    /// is the first pass that knows the scrutinee's type.
    When {
        scrutinee: Box<Expr>,
        arms: Vec<WhenArm>,
        span: Span,
    },
}

/// One arm of the pattern form of `when`. `pattern` is `None` for an `else`
/// arm, which covers every variant not named above it.
#[derive(Debug, Clone)]
pub struct WhenArm {
    pub pattern: Option<VariantPattern>,
    pub body: Expr,
    pub span: Span,
}

/// `Rect(w, h)` / `Empty` — a variant name and one binding per payload
/// field, in field order. `None` in `binds` is `_`: that payload is ignored
/// rather than named.
#[derive(Debug, Clone)]
pub struct VariantPattern {
    pub variant: String,
    pub binds: Vec<Option<String>>,
    pub span: Span,
}

/// One piece of a string template: literal text, or an interpolated
/// expression.
#[derive(Debug, Clone)]
pub enum TemplatePart {
    Text(String),
    Expr(Expr),
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
            | Expr::MethodCall { span: s, .. }
            | Expr::Template { span: s, .. }
            | Expr::When { span: s, .. } => *s,
            Expr::Block(b) => b.span,
        }
    }
}

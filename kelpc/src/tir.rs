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
    /// Canonical id of the module this `pd` was declared in, like
    /// `Function::module`: an initializer is not a function, so this is the
    /// only carrier of the module whose grant a gated call inside it is
    /// checked against, and of the module the denial names.
    pub module: String,
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
    /// The environment this function receives, when it is a `fun` literal that
    /// captured something (A-4 phase E): `Some` makes the generated C
    /// signature start with a hidden `void*` environment parameter that the
    /// body's `ExprKind::Capture` reads go through. `None` for every ordinary
    /// function and for a literal that captured nothing.
    pub env: Option<ClosureEnv>,
}

/// One literal's environment (A-4 phase E); see `Function::env`.
#[derive(Debug, Clone)]
pub struct ClosureEnv {
    /// The generated C struct (`kelp_Env__#funlit3`), one field per capture.
    pub struct_name: String,
    /// The generated function that drops each owned field and frees the
    /// struct (`kelp_env_drop__#funlit3`).
    pub drop_name: String,
    /// Field name (`c0`, `c1`, …) and type, in the order `ExprKind::Capture`
    /// indexes them.
    pub fields: Vec<(String, Ty)>,
}

#[derive(Debug, Clone)]
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
    /// An integer literal, at `i128` so that a `const` folded to the top of
    /// `u64`'s range is representable; codegen cuts it to the expression's own
    /// type, which is what makes the width the type's rather than the literal's.
    IntLit(i128),
    /// `'a'` — the code point, already checked against the type
    /// `infer.rs` resolved for it. Codegen casts it to that type's C
    /// spelling, so a `'0'` in a `u8` position emits a `uint8_t`.
    CharLit(u32),
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
    /// `expr?` — propagate the failure of an enum shaped `Ok`/`Err` or
    /// `Some`/`None` out of the enclosing function (MK5-S6-M1). `expr` is the
    /// operand, whose own type names the *source* enum; `target` is the
    /// enclosing function's return type, which the failure value is rebuilt
    /// in. Both variant indices are carried so codegen needs no lookup order it
    /// does not already have, and `err_has_payload` is false for `None`, which
    /// carries nothing to move.
    Try {
        expr: Box<Expr>,
        source: String,
        target: String,
        ok_variant: usize,
        err_variant: usize,
        target_err_variant: usize,
        err_has_payload: bool,
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
    /// A named function used as a *value* (A-4): `val f = double`. The key is
    /// the declaration's canonical name, so codegen emits `kelp_double` — a
    /// function reference carries no environment, so the C function *is* the
    /// value and there is nothing to allocate or drop.
    FnRef(String),
    /// A `fun` literal that captured something (A-4 phase E): the value is a
    /// *closure* — a function pointer, the heap environment its captures were
    /// moved into, and the environment's drop. `key` names the generated
    /// function (whose signature starts with the hidden environment
    /// parameter), and `captures` holds one initializer per environment field,
    /// in field order, each written in the enclosing function's terms: a
    /// local by name, or a read of an enclosing closure's environment.
    ClosureLit {
        key: String,
        captures: Vec<Expr>,
    },
    /// A read of one field of the enclosing closure's environment (A-4 phase
    /// E), inside the generated function that literal became. It is a *place*
    /// (the environment owns the value), which is what lets an owned capture
    /// be borrowed — calling a method on it, passing it to a parameter — while
    /// reading it out as a whole value is the partial move `borrowck.rs`
    /// refuses.
    Capture { index: usize },
    /// A plain function value used where a closure is expected (A-4 phase E):
    /// the widening that makes `val f: closure(i32).R: i32 = double` and
    /// `Holder(cb = fun(x: i32).R: i32 = x)` mean what they read as. A `fun`
    /// value is a bare code pointer with no environment, so the value is
    /// wrapped in a one-field environment holding the pointer, plus a thunk
    /// generated per signature that calls through it. The alternative — a
    /// hidden first parameter on every declared function — is not C's calling
    /// convention, so a wrapper is the honest cost of one type.
    FnToClosure { func: Box<Expr> },
    /// A call through a function value: `f(x)`, where `f` is a local (or,
    /// later, a field) of a `Ty::Fn` type. `callee` is a value expression, so
    /// codegen parenthesizes it before the argument list. Distinct from
    /// `Call`, whose `callee` is a resolved declaration key.
    IndirectCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
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
    /// `[a, b, c]`: an array value (A-18). Every element has the array's own
    /// element type (checked here), and the length is the number of elements,
    /// so the expression's `ty` is a complete `[T; n]` and no later pass has
    /// to count anything.
    ArrayLit(Vec<Expr>),
    /// `[v; N]`: `N` copies of `v`, which is how a zeroed fixed-size buffer is
    /// written (A-18). The length is a compile-time constant and lives in the
    /// expression's own type (`[T; N]`), which is where codegen reads it; the
    /// element must own nothing, because the value is copied into every slot.
    ArrayRepeat { value: Box<Expr> },
}

/// One piece of a template: literal text, or an interpolated value that has
/// already been checked to be printable.
#[derive(Debug, Clone)]
pub enum TemplatePiece {
    Text(String),
    Value(Expr),
}

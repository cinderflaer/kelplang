use super::*;

#[derive(Debug, Clone)]
pub enum Expr {
    /// A decimal, hexadecimal or binary integer literal, as the magnitude the
    /// source wrote. `u64` is the payload's width because that is the range a
    /// literal may spell: the widest integer type holds values `i64` cannot, and
    /// a negative value is an `Unary` negation of one of these (MISSING-x8Wq3).
    /// `infer.rs` decides the type from the literal's context.
    IntLit(u64, Span),
    /// `'a'` - one Unicode scalar value (open item 53), from the lexer
    /// already decoded. Its *type* is not decided here: `infer.rs` gives it
    /// the integer type its context asks for (so a `u8` byte comparison can
    /// write `c == '"'`) and `char` when nothing asks for one.
    CharLit(u32, Span),
    /// `b'x'`: one byte, a `u8` by construction (H-11), so `infer.rs` gives it
    /// `u8` whatever the context asks for and refuses a wider one. It is what
    /// deletes the `val c: u8 = '"'` workaround.
    ByteLit(u8, Span),
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
    /// Also covers `datac` construction (`Circle(r = 2.0)`) - at parse
    /// time a call and a construction look identical; `infer.rs`
    /// disambiguates by whether `callee` names a function or a `datac`.
    /// `type_args` is the explicit instantiation (`Pair<i32, String>(...)`,
    /// A-2), empty when the call writes none and inference supplies them.
    Call {
        callee: String,
        type_args: Vec<TypeExpr>,
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
    /// `&expr` (shared) / `&mut expr` (exclusive) - taking a borrow of a
    /// local. Only legal outside signatures; parameter/receiver positions
    /// use the bare/`mut T` forms instead (parsed via `Param.mutable`).
    Ref {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
    /// `expr?` - propagate a failure out of the enclosing function
    /// (MK5-S6-M1). Postfix, and the operand must be an `enum` shaped
    /// `Ok`/`Err` or `Some`/`None`; `infer.rs` decides that and lowers it to a
    /// `tir::ExprKind::Try`.
    Try {
        expr: Box<Expr>,
        span: Span,
    },
    /// `*expr` - dereferencing a borrow.
    Deref {
        expr: Box<Expr>,
        span: Span,
    },
    /// `receiver.method(args)` - syntactically indistinguishable from
    /// `.field` access until the parser sees a `(` right after the method
    /// name. `infer.rs` resolves `method` against the receiver expression's
    /// type and lowers this to a plain `Call` (the receiver becomes the
    /// first argument, auto-borrowed exactly like any other `Ty::Ref`
    /// parameter) - no new `tir`/codegen shape needed.
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        /// The explicit instantiation of a generic `enum`'s variant
        /// (`Option.Some<i32>(v = 1)`), empty otherwise (A-2).
        type_args: Vec<TypeExpr>,
        args: Vec<CallArg>,
        span: Span,
    },
    /// `fun(x: i32) => i32 = x * 2` - an anonymous function (A-4), written
    /// where an expression is expected. `infer.rs` synthesizes one top-level
    /// function per literal; the expression's value is a function value when
    /// the body captured nothing, and a *closure* (a function value plus an
    /// environment holding what it captured) when it did.
    FnLit {
        params: Vec<Param>,
        /// A block header (`{ x, y -> ... }`) writes the parameter *names*
        /// only: their types come from the expected function type of the
        /// parameter the block is passed to (`kelp wiki page language/functions.md` part 1).
        /// `false` for a `fun(...)` literal, whose parameters are always
        /// typed by hand. A literal with this set and no expected function
        /// type is refused rather than guessed at.
        infer_params: bool,
        /// The `=> T`. `None` means `Unit`, exactly as on a declaration -
        /// there is no return-type inference, because a literal's type *is*
        /// its signature. A block literal with `infer_params` takes the return
        /// from the expected type instead: a non-unit expected return makes
        /// the block's tail its value, a unit one discards it as today.
        return_type: Option<TypeExpr>,
        /// `= expr` or `{ ... }`. Never `FunctionBody::Extern`: the parser
        /// requires a body. Boxed because the body holds expressions and a
        /// literal may appear inside one, so the two types are mutually
        /// recursive.
        body: Box<FunctionBody>,
        span: Span,
    },
    /// A string literal with `$name` / `${expr}` interpolation, already split
    /// into parts by the lexer. Can only be consumed by `print`/`print!` in
    /// v1 - building a `str` value out of pieces needs the owned `String`
    /// type, which lands in M6 - so `infer.rs` rejects it anywhere else.
    Template {
        parts: Vec<TemplatePart>,
        span: Span,
    },
    /// `unsafe { ... }` (MK9-S2 stage 1): a block whose body may call the
    /// unsafe operations. It is an expression like any `{ ... }` block, with
    /// the same value, and the checker tracks the nesting while it walks it.
    Unsafe {
        block: Block,
        span: Span,
    },
    /// `block name: T { ... }` (manual memory blocks, slice 1, decision 139):
    /// a named region whose `name.box()` calls reserve block memory of type
    /// `T`. The `?` after the closing brace is an ordinary postfix `Try` around
    /// this expression. Parsed only; `infer.rs` refuses it until blocks are built.
    ManualBlock {
        name: String,
        ty: TypeExpr,
        body: Block,
        span: Span,
    },
    /// `asm("<template>", operands...)` (kernel stage 5): one target-assembler
    /// instruction, legal only inside `unsafe`. The template is the target's
    /// own text and the operands are the values that cross into and out of it;
    /// `kelp wiki page language/unsafe.md` is the design.
    Asm {
        template: String,
        operands: Vec<AsmOperand>,
        clobbers: Vec<String>,
        /// `nomem`: the asm does not touch memory, so the default `~{memory}`
        /// clobber is dropped.
        nomem: bool,
        /// `nostack`: accepted and recorded; neither LLVM's inline-asm IR nor
        /// GCC's extended asm has a stack-clobber spelling, so it changes no
        /// output (see `kelp wiki page language/unsafe.md`).
        // Read only by the parser tests: nothing lowers it, by design (accepted for source
        // compatibility with the option list, documented as unchecked).
        #[allow(dead_code)]
        nostack: bool,
        span: Span,
    },
    /// `when expr { Variant(bind) -> value ... }` - pattern matching over an
    /// `enum` scrutinee (A-1, `design/syntax.md (kelp wiki)` §6.3). Distinct from the
    /// boolean `when`, which has no scrutinee and is desugared by the parser
    /// (see `parser::parse_when`); this form is checked by `infer.rs`, which
    /// is the first pass that knows the scrutinee's type.
    When {
        scrutinee: Box<Expr>,
        arms: Vec<WhenArm>,
        span: Span,
        /// `true` for the `when` a `let PATTERN = EXPR else { BLOCK }` statement
        /// desugars to (`design/syntax.md (kelp wiki)` 4.1): arm one is the pattern over the
        /// rest of the block, arm two is the `else` block. `infer.rs` checks
        /// the form under the let-else rules, so its refusals name the rule.
        let_else: bool,
    },
    /// `[a, b, c]`: an array value (A-18). Its length is the number of
    /// elements written, and its type is `[T; n]` for the element type the
    /// elements agree on. `infer.rs` gives it the declared type when the
    /// context names one (`val xs: [i32; 3] = [1, 2, 3]`) and joins the
    /// elements' own types when it does not.
    ArrayLit { elems: Vec<Expr>, span: Span },
    /// `[v; N]`: `N` copies of `v`, the zeroed-buffer form (A-18). `v` must
    /// be `Copy`: the alternative is cloning an owned value per element,
    /// which is not a thing a literal can do. `N` is a compile-time constant
    /// (A-19), folded here exactly as an array type's length is.
    ArrayRepeat {
        value: Box<Expr>,
        len: Box<Expr>,
        span: Span,
    },
    /// `(lo, hi)`: a tuple value, two or more elements (`design/syntax.md (kelp wiki)` 3).
    /// The one-element form is not a tuple: `(e)` is `e` in parentheses, and
    /// `()` is not an expression at all, so a tuple literal always has at
    /// least two elements and its type is the elements' own, in order.
    TupleLit { elems: Vec<Expr>, span: Span },
}

/// One operand of an `asm(...)` expression (kernel stage 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmOperandKind {
    /// `in("...") expr`: read `expr` in.
    In,
    /// `out("...") place`: write the result into `place`, or `_` for the
    /// anonymous output that is the expression's value.
    Out,
    /// `inout("...") place`: read `place` in and write the result back to it.
    InOut,
}

/// One `in`/`out`/`inout` operand of an `asm(...)` expression (kernel stage 5).
#[derive(Debug, Clone)]
pub struct AsmOperand {
    pub kind: AsmOperandKind,
    /// The LLVM constraint name written, `"r"` for any general register or an
    /// explicit one like `"rax"`; passed to the backend unchanged.
    pub constraint: String,
    /// For `in`, the value. For `out`/`inout`, the place written. `None` is the
    /// anonymous `out("...") _` form, which is the expression's value.
    pub operand: Option<Expr>,
    pub span: Span,
}

/// One arm of the pattern form of `when`. `pattern` is `None` for an `else`
/// arm, which covers every variant not named above it.
#[derive(Debug, Clone)]
pub struct WhenArm {
    pub pattern: Option<VariantPattern>,
    pub body: Expr,
    pub span: Span,
}

/// `Rect(w, h)` / `Empty` - a variant name and one binding per payload
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

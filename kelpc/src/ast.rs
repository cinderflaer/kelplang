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
    /// `pub const ...` (A-13, `plan/syntax.md` 13.4). Every top-level item is
    /// private to its module unless this is written, which is what
    /// `infer.rs`'s visibility gate reads.
    pub is_pub: bool,
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

/// One `impl "shapes/circle"` line, plus its optional `caps(...)` grants (M7).
#[derive(Debug, Clone)]
pub struct ImplDecl {
    /// The string literal's contents, verbatim (`shapes/circle` — no `.kp`).
    pub path: String,
    /// `caps(domain = [patterns...])` grants. Empty when the clause is
    /// absent. Two jobs in one syntax: it *delegates* the domain to the
    /// imported module, and it is how the importing module states which
    /// patterns it is willing to hand over.
    pub caps: Vec<CapGrant>,
    pub span: Span,
}

/// One `fs = ["/var/log/**"]` inside a `caps(...)` clause.
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
    /// `pub datac ...` (A-13). See `ConstDecl::is_pub`. A `pub` type's
    /// methods come with it: a method is reached through a value of the type,
    /// never through a module alias.
    pub is_pub: bool,
    /// `datac Pair<A, B>` — the declared type parameters, in order (A-2).
    /// Empty for an ordinary `datac`. A parameter is in scope inside every
    /// field type of this declaration and nowhere else; the declaration is a
    /// *template*, so `infer.rs` does not resolve its fields until an
    /// instantiation supplies concrete arguments for them.
    pub type_params: Vec<String>,
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
    /// `pub enum ...` (A-13). See `DatacDecl::is_pub`.
    pub is_pub: bool,
    /// `enum Option<T>` — the declared type parameters, in order (A-2). See
    /// `DatacDecl::type_params`.
    pub type_params: Vec<String>,
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
    /// `pub fun ...` / `pub extern fun ...` (A-13). See `ConstDecl::is_pub`.
    /// Never true on a receiver method: the parser refuses `pub` there, since
    /// a method's reachability follows its declaring type.
    pub is_pub: bool,
    /// `fun f<T>(...)` — the function's own type parameters (A-2). Empty for
    /// an ordinary function, and empty for a *receiver method*, whose type
    /// parameters come from the receiver instead (`Receiver::type_args`).
    pub type_params: Vec<String>,
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
    /// `fun Pair<A, B>.first()` — the receiver's type arguments, which are the
    /// method's type parameters (A-2). They must be exactly the generic type's
    /// own parameter list, in order, so the body can name them.
    pub type_args: Vec<String>,
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
    /// `Borrow` (the default), `mut T` (an exclusive borrow) or `move T`
    /// (the callee takes ownership, A-4).
    pub mode: ParamMode,
    pub span: Span,
}

/// How a parameter receives its argument. One enum rather than two flags,
/// because the three are exclusive by construction: `mut move T` is not a
/// state that can be represented, so it cannot be checked for either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// The default: an implicit shared borrow for a `datac`-shaped type,
    /// plain by-value for a `Copy` type.
    Borrow,
    /// `mut T` — an exclusive borrow.
    Mut,
    /// `move T` — the callee owns the argument and the call site moves it.
    Move,
}

impl ParamMode {
    pub fn mutable(self) -> bool {
        matches!(self, ParamMode::Mut)
    }

    pub fn moves(self) -> bool {
        matches!(self, ParamMode::Move)
    }

    /// The qualifier as written, for the pretty-printer.
    pub fn keyword(self) -> &'static str {
        match self {
            ParamMode::Borrow => "",
            ParamMode::Mut => "mut ",
            ParamMode::Move => "move ",
        }
    }
}

/// One parameter of a function *type* (A-4): `fun(String)` (a borrow),
/// `fun(mut Counter)` (an exclusive borrow), `fun(move Job)` (ownership).
/// There is no binding name — a function type is not a declaration — so this
/// is a mode and a type, exactly as `Param` is minus the name.
#[derive(Debug, Clone)]
pub struct FnTypeParam {
    pub mode: ParamMode,
    pub ty: TypeExpr,
}

impl FnTypeParam {
    /// The parameter as written, for diagnostics and the pretty-printer.
    pub fn display(&self) -> String {
        format!("{}{}", self.mode.keyword(), self.ty.display())
    }
}

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    /// The element types of a tuple type, `(A, B)` (`plan/syntax.md` 3):
    /// `Some`, with two or more entries, exactly when the type was written in
    /// parentheses with a comma. `name` is empty in that case, because a tuple
    /// type has no name of its own: it is its element list. `(T)` is not a
    /// tuple and is parsed as the parenthesized type `T`; `()` is refused.
    pub tuple: Option<Vec<TypeExpr>>,
    /// Type arguments, for the one generic the compiler knows: `List<T>`.
    /// Empty for every ordinary type name, and the parser only accepts them
    /// after a bare or dotted name — `List<i32>`, never `math.List<i32>`.
    /// `infer.rs` rejects arguments on a name that is not `List`, and more
    /// than one on `List` itself; v1 has no user-definable generics.
    pub args: Vec<TypeExpr>,
    /// The element type of `[T; N]` (A-18), `Some` exactly when the type was
    /// written with brackets. `name` is empty in that case, because an array
    /// type has no name of its own: it is its element and its length.
    pub array_elem: Option<Box<TypeExpr>>,
    /// The `N` of `[T; N]`, kept as an expression because a length is
    /// allowed to be computed from constants (`PAGE_SIZE / ENTRY_SIZE`,
    /// A-19), folded by `infer.rs` when the type is resolved.
    pub array_len: Option<Box<Expr>>,
    /// `&T` was written with a leading `&`. Only meaningful in a *return*
    /// position (M6.3): `&str`/`&T` there is a borrow of the caller's value
    /// under the elision rule, whereas a parameter's `&str` is what `str`
    /// already is (a borrowed view passed by value).
    pub amp: bool,
    /// `fun(i32, String).R: bool` — the parameters, when this is a function
    /// type (A-4). `Some`, even when empty (`fun()`), is what marks it as one;
    /// `None` for every ordinary type. `name` is `"fun"` in that case, or
    /// `"closure"` for the owned, environment-carrying form (A-4 phase E);
    /// neither can collide with a real name because both are keywords.
    pub fn_params: Option<Vec<FnTypeParam>>,
    /// The `.R: T` of a function type. `None` means `Unit`, exactly as it
    /// does on a declaration.
    pub fn_ret: Option<Box<TypeExpr>>,
    pub span: Span,
}

impl TypeExpr {
    /// An ordinary named type with no arguments — the shape almost every
    /// construction site wants.
    pub fn named(name: impl Into<String>, span: Span) -> TypeExpr {
        TypeExpr {
            name: name.into(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            array_len: None,
            amp: false,
            fn_params: None,
            fn_ret: None,
            span,
        }
    }

    /// Whether this is a closure type (`closure(i32).R: i32`) — the owned,
    /// environment-carrying form of a function type (A-4 phase E).
    pub fn is_closure(&self) -> bool {
        self.fn_params.is_some() && self.name == "closure"
    }

    /// Whether this is a plain function type (`fun(i32).R: i32`).
    pub fn is_fn(&self) -> bool {
        self.fn_params.is_some()
    }

    /// The type as it was written, for diagnostics and the pretty-printer.
    pub fn display(&self) -> String {
        let mut s = if let Some(elems) = &self.tuple {
            let parts: Vec<String> = elems.iter().map(|e| e.display()).collect();
            format!("({})", parts.join(", "))
        } else if let Some(elem) = &self.array_elem {
            let len = self
                .array_len
                .as_ref()
                .map(|l| l.display_source())
                .unwrap_or_else(|| "?".to_string());
            format!("[{}; {}]", elem.display(), len)
        } else if let Some(params) = &self.fn_params {
            // A function type and a closure type differ by one keyword and
            // nothing else, so the keyword written is the keyword shown
            // (A-4 phase E).
            let args: Vec<String> = params.iter().map(|p| p.display()).collect();
            format!("{}({})", self.name, args.join(", "))
        } else if self.args.is_empty() {
            self.name.clone()
        } else {
            let args: Vec<String> = self.args.iter().map(|a| a.display()).collect();
            format!("{}<{}>", self.name, args.join(", "))
        };
        if let Some(ret) = &self.fn_ret {
            s.push_str(&format!(".R: {}", ret.display()));
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// One binding position of a destructuring `val`/`var` (`plan/syntax.md` 4):
/// a name, `_` to ignore the position, or a nested tuple pattern.
#[derive(Debug, Clone)]
pub enum BindingPattern {
    /// One bound name.
    Name(String),
    /// `_`: the position is not bound. What it holds stays in the value that
    /// was destructured, so an owned element an `_` ignores is freed by that
    /// value's own drop.
    Ignore,
    /// `((a, b), c)`: a nested tuple pattern, one entry per element.
    Tuple(Vec<BindingPattern>),
}

impl BindingPattern {
    /// Every name the pattern binds, in position order, with the pattern path
    /// each one sits at: `[(a, [0, 0]), (b, [0, 1]), (c, [1])]`. The path is
    /// what a diagnostic names a position by (`element 1 of element 0`).
    pub fn binds(&self, path: &mut Vec<usize>, out: &mut Vec<(String, Vec<usize>)>) {
        match self {
            BindingPattern::Name(name) => out.push((name.clone(), path.clone())),
            BindingPattern::Ignore => {}
            BindingPattern::Tuple(elems) => {
                for (i, e) in elems.iter().enumerate() {
                    path.push(i);
                    e.binds(path, out);
                    path.pop();
                }
            }
        }
    }
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
    /// `val (lo, hi) = min_max(xs)`: a binding that takes one tuple value
    /// apart into its positions (`plan/syntax.md` 4). A statement of its own
    /// rather than a second `Let` shape, because it binds several names.
    LetTuple {
        pattern: BindingPattern,
        /// `true` for `var`, `false` for `val`: every name the pattern binds
        /// takes that mutability.
        mutable: bool,
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
    /// Bitwise operators (A-9). `BitAnd` and the borrow `&` share one
    /// spelling and are told apart by position: `a & b` is a binary operator,
    /// `&a` is a borrow, exactly as C++ reads `&`.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `~a` — bitwise complement. Prefix only: the language has no `~`
    /// postfix form and no `~=`.
    BitNot,
}

/// One argument at a call site: `f(1, 2)` (positional) or `f(x = 1)`
/// (named) or a mix of both.
#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
}

/// A "place": an expression with stable storage that can be borrowed,
/// assigned into, or matched without moving what it names — an `Ident`, a
/// `.field` chain rooted in one, or a dereferenced borrow (`*p`, `(*p).field`).
/// Shared between `parser.rs` (which uses it to decide whether `if let`'s
/// desugared scrutinee needs a fresh owning temporary — see `parse_if_let`)
/// and `infer.rs` (which uses it for the same question about an explicit
/// `&expr`/method-borrow argument), so the definition of "a place" cannot
/// drift between the two.
///
/// The field case **recurses**, because a field is exactly as durable as what
/// it is a field of: `mkName().name` is a field of an rvalue, so it has no
/// storage of its own. That is what makes `&mkName().name` the "cannot borrow
/// a temporary value" refusal rather than a borrow whose owner is freed at the
/// end of the statement, and it is also what makes an `if let` over such a
/// scrutinee bind a temporary of its own (a payload borrow has to outlive the
/// arm). A `Deref` stays unconditional: the pointer may itself be a temporary,
/// but what it points at belongs to somebody else.
pub fn is_place(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(..) | Expr::Deref { .. } => true,
        Expr::FieldAccess { base, .. } => is_place(base),
        _ => false,
    }
}

/// The rvalue a field chain hangs off, when the chain is not rooted at a place
/// at all: `None` for `c`, `c.f`, `*p`, and `Some(root)` for `mk().f`, or for
/// anything that is not a place in the first place, `mk()` and `5` included
/// (`root` is then the expression itself).
///
/// A pattern match reads its scrutinee once per arm and binds an owned payload
/// as a borrow of it, so the scrutinee has to be a place that outlives the
/// arms. A temporary is instead staged into a local of its own: the caller
/// binds `root` and rewrites the chain over that binding with `rebase_root`,
/// which is exactly what writing `val h = mk()` by hand does. The chain is
/// staged at its root and never at the field, because the whole temporary is
/// what owns the memory and what has to be freed.
pub fn temporary_root(expr: &Expr) -> Option<&Expr> {
    let mut cur = expr;
    while let Expr::FieldAccess { base, .. } = cur {
        cur = base;
    }
    if is_place(cur) {
        None
    } else {
        Some(cur)
    }
}

/// `expr` with its root temporary replaced by `base`, so `mk().t` becomes
/// `#tmp.t` for the staged binding `#tmp`. Expression kinds other than a field
/// chain are `base` itself: they are what `temporary_root` handed over.
pub fn rebase_root(expr: &Expr, base: &Expr) -> Expr {
    match expr {
        Expr::FieldAccess { base: b, field, span } => Expr::FieldAccess {
            base: Box::new(rebase_root(b, base)),
            field: field.clone(),
            span: *span,
        },
        _ => base.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    /// A decimal, hexadecimal or binary integer literal, as the magnitude the
    /// source wrote. `u64` is the payload's width because that is the range a
    /// literal may spell: the widest integer type holds values `i64` cannot, and
    /// a negative value is an `Unary` negation of one of these (MISSING-x8Wq3).
    /// `infer.rs` decides the type from the literal's context.
    IntLit(u64, Span),
    /// `'a'` — one Unicode scalar value (open item 53), from the lexer
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
    /// Also covers `datac` construction (`Circle(r = 2.0)`) — at parse
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
    /// `&expr` (shared) / `&mut expr` (exclusive) — taking a borrow of a
    /// local. Only legal outside signatures; parameter/receiver positions
    /// use the bare/`mut T` forms instead (parsed via `Param.mutable`).
    Ref {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
    /// `expr?` — propagate a failure out of the enclosing function
    /// (MK5-S6-M1). Postfix, and the operand must be an `enum` shaped
    /// `Ok`/`Err` or `Some`/`None`; `infer.rs` decides that and lowers it to a
    /// `tir::ExprKind::Try`.
    Try {
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
        /// The explicit instantiation of a generic `enum`'s variant
        /// (`Option.Some<i32>(v = 1)`), empty otherwise (A-2).
        type_args: Vec<TypeExpr>,
        args: Vec<CallArg>,
        span: Span,
    },
    /// `fun(x: i32).R: i32 = x * 2` — an anonymous function (A-4), written
    /// where an expression is expected. `infer.rs` synthesizes one top-level
    /// function per literal; the expression's value is a function value when
    /// the body captured nothing, and a *closure* (a function value plus an
    /// environment holding what it captured) when it did.
    FnLit {
        params: Vec<Param>,
        /// The `.R: T`. `None` means `Unit`, exactly as on a declaration —
        /// there is no return-type inference, because a literal's type *is*
        /// its signature.
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
    /// `(lo, hi)`: a tuple value, two or more elements (`plan/syntax.md` 3).
    /// The one-element form is not a tuple: `(e)` is `e` in parentheses, and
    /// `()` is not an expression at all, so a tuple literal always has at
    /// least two elements and its type is the elements' own, in order.
    TupleLit { elems: Vec<Expr>, span: Span },
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
            | Expr::CharLit(_, s)
            | Expr::ByteLit(_, s)
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
            | Expr::Try { span: s, .. }
            | Expr::Deref { span: s, .. }
            | Expr::MethodCall { span: s, .. }
            | Expr::Template { span: s, .. }
            | Expr::FnLit { span: s, .. }
            | Expr::When { span: s, .. }
            | Expr::ArrayLit { span: s, .. }
            | Expr::ArrayRepeat { span: s, .. }
            | Expr::TupleLit { span: s, .. } => *s,
            Expr::Block(b) => b.span,
        }
    }

    /// How this expression was written, for the one place a *type* diagnostic
    /// has to quote an expression back: the `N` of an array's `[T; N]`
    /// (`infer.rs` reports a length that is not a constant, or an array whose
    /// literal has the wrong length, and "[i32; 1 + 2]" reads better than
    /// "[i32; ?]"). Kept deliberately small: it covers the shapes a length is
    /// written as, and falls back to a placeholder for anything else rather
    /// than growing a second full pretty-printer.
    pub fn display_source(&self) -> String {
        match self {
            Expr::IntLit(v, _) => v.to_string(),
            Expr::FloatLit(v, _) => v.to_string(),
            Expr::CharLit(c, _) => format!("'{}'", char::from_u32(*c).unwrap_or('?')),
            Expr::ByteLit(b, _) => format!("b'{}'", *b as char),
            Expr::BoolLit(b, _) => b.to_string(),
            Expr::StringLit(s, _) => format!("\"{s}\""),
            Expr::Ident(name, _) => name.clone(),
            Expr::Unary { op, expr, .. } => {
                let op = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                };
                format!("{op}{}", expr.display_source())
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let op = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Mod => "%",
                    BinOp::Eq => "==",
                    BinOp::NotEq => "!=",
                    BinOp::Lt => "<",
                    BinOp::LtEq => "<=",
                    BinOp::Gt => ">",
                    BinOp::GtEq => ">=",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                    BinOp::BitAnd => "&",
                    BinOp::BitOr => "|",
                    BinOp::BitXor => "^",
                    BinOp::Shl => "<<",
                    BinOp::Shr => ">>",
                };
                format!("{} {op} {}", lhs.display_source(), rhs.display_source())
            }
            Expr::MethodCall { receiver, method, .. } => {
                format!("{}.{method}(...)", receiver.display_source())
            }
            Expr::Call { callee, .. } => format!("{callee}(...)"),
            _ => "...".to_string(),
        }
    }
}

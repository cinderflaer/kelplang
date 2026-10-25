use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    /// `impl "path"` lines, in source order. One file's list only; the
    /// multi-file loader in `resolve.rs` owns the whole module graph.
    pub impls: Vec<ImplDecl>,
    /// `const NAME: T = expr` lines, in source order.
    pub consts: Vec<ConstDecl>,
    /// `static NAME: T = expr` / `static mut NAME: T = expr` lines, in source
    /// order (kernel stage 7). A static is one object with a stable address,
    /// unlike a folded `const`.
    pub statics: Vec<StaticDecl>,
    /// `pd val`/`pd var` lines, in source order (canonical position: after
    /// `impl`, before `datac`/`fun`).
    pub pds: Vec<PdDecl>,
    pub datacs: Vec<DatacDecl>,
    /// `enum Name { Variant(...) ... }` declarations, in source order, on the
    /// same footing as `datacs`: both declare a type, and either may follow
    /// the other (a field or payload that names another type by value still
    /// has to name an earlier one).
    pub enums: Vec<EnumDecl>,
    /// `extern type Widget` declarations, in source order (gap 1).
    pub extern_types: Vec<ExternTypeDecl>,
    /// `trait Name { fun Self.m(...) ... }` declarations, in source order
    /// (A-12 slice 2). A trait is a named set of method signatures and nothing
    /// else: it declares no type, no field and no body.
    pub traits: Vec<TraitDecl>,
    /// `impl Trait for Type { fun Self.m(...) { ... } }` blocks, in source
    /// order (A-12 slice 2). The methods themselves are ordinary receiver
    /// methods in `functions`; this is the pairing `infer.rs` validates
    /// coherence and the exact-signature match against.
    pub trait_impls: Vec<TraitImplDecl>,
    pub functions: Vec<FunctionDecl>,
    /// The span of the top-level statements of a script (issue #288), when the
    /// file has any. Their statements are the body of the `main` in `functions`.
    pub script: Option<Span>,
}

mod decls;
pub(crate) use decls::*;

#[derive(Debug, Clone)]
pub struct DatacDecl {
    pub name: String,
    /// `pub datac ...` (A-13). See `ConstDecl::is_pub`. A `pub` type's
    /// methods come with it: a method is reached through a value of the type,
    /// never through a module alias.
    pub is_pub: bool,
    /// `datac Pair<A, B>` - the declared type parameters, in order (A-2).
    /// Empty for an ordinary `datac`. A parameter is in scope inside every
    /// field type of this declaration and nowhere else; the declaration is a
    /// *template*, so `infer.rs` does not resolve its fields until an
    /// instantiation supplies concrete arguments for them. A `const`
    /// parameter is listed here too, in declaration order, and its value is
    /// carried by `const_params` (A-20).
    pub type_params: Vec<String>,
    /// `datac Buf<const N: usize>` - the `const` parameters of the list above,
    /// in declaration order (A-20). Every name here also appears in
    /// `type_params`; this is what says which of them is a value.
    pub const_params: Vec<ConstParamDecl>,
    pub fields: Vec<FieldDecl>,
    /// The `repr(...)` clause, or `None` for the default C layout.
    pub repr: Option<Repr>,
    /// Declaration index within this file, across `datac`s *and* `enum`s, so
    /// `resolve.rs` can rebuild one merged declaration order out of two
    /// lists (A-1). A field may only name an earlier type by value, and
    /// the removed C backend emitted typedefs in that same order.
    pub order: usize,
    /// Canonical id of the module this was declared in: `""` for the entry
    /// module, otherwise its path relative to `src/` without the `.kp`
    /// suffix (`"shapes/circle"`). Set by `resolve.rs`; the parser leaves it
    /// empty. Used to attribute a diagnostic to the right source file - a
    /// `Span` is only meaningful inside its own file.
    pub module: String,
    pub span: Span,
}

/// One `enum Shape { Circle(val r: f64) Empty }` - a tagged union (A-1).
/// `variants` is never empty, and variant names are unique within the enum;
/// both are checked in `infer.rs`.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    /// `pub enum ...` (A-13). See `DatacDecl::is_pub`.
    pub is_pub: bool,
    /// `enum Option<T>` - the declared type parameters, in order (A-2). See
    /// `DatacDecl::type_params`.
    pub type_params: Vec<String>,
    /// `enum Buf<const N: usize>` - the `const` parameters of the list above
    /// (A-20). See `DatacDecl::const_params`.
    pub const_params: Vec<ConstParamDecl>,
    pub variants: Vec<VariantDecl>,
    /// The `repr(...)` clause, or `None` for the default C layout and an `i32`
    /// tag.
    pub repr: Option<Repr>,
    /// Declaration index within this file, across `datac`s *and* `enum`s, so
    /// `resolve.rs` can rebuild one merged declaration order out of two
    /// lists (A-1). A field or payload may only name an earlier type by
    /// value, and the removed C backend emitted typedefs in that same order.
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
    /// `Off = 0`: the value `to_int`/`from_int` use for this variant. Legal
    /// only on a field-less enum; `infer.rs` checks that. `None` means the
    /// previous variant's value plus one, starting at 0.
    pub discriminant: Option<i128>,
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
    /// `fun f<T>(...)` - the function's own type parameters (A-2). Empty for
    /// an ordinary function, and empty for a *receiver method*, whose type
    /// parameters come from the receiver instead (`Receiver::type_args`).
    pub type_params: Vec<String>,
    /// `fun sum<const N: usize>(...)` - the `const` parameters of the list
    /// above (A-20). See `DatacDecl::const_params`.
    pub const_params: Vec<ConstParamDecl>,
    /// `fun f<T: A + B>(...)`: the traits each parameter of `type_params`
    /// must implement, one list per parameter and in the same order (A-12
    /// slice 2). A parameter with no bound has an empty list, so the two
    /// vectors are always the same length. Empty for a receiver method, whose
    /// parameters are its type's and take no bound.
    pub type_param_bounds: Vec<Vec<TypeExpr>>,
    /// `where I::Item: Bound` (MISSING-Ve8Nc step 5): a bound on the associated
    /// type `assoc` projected from the type parameter `param`, which the
    /// instance checks at each call. Empty when the `where` clause has none.
    pub projection_bounds: Vec<ProjectionBound>,
    /// For a method of a generic impl (`impl<I: Iterator> Tr for Type<I>`): the
    /// impl's bounds on the receiver type's parameters, one list per parameter
    /// in order, so the method sees them (`I::Item`) and an instance is checked
    /// against them. Empty otherwise.
    pub receiver_bounds: Vec<Vec<TypeExpr>>,
    /// `Some` for a method declared inside an `impl Trait for Type { ... }`
    /// block (A-12 slice 2).
    pub trait_impl: Option<TraitImplSite>,
    /// `extern fun ...` - the body is supplied by the compiler (M6.2), so
    /// `body` is always `FunctionBody::Extern` here.
    pub extern_: bool,
    /// `unsafe fun ...` (MK9-S2 stage 1): the function may be called only from
    /// an `unsafe { ... }` block or from another `unsafe fun`. It is legal only
    /// in a module the project's `coral.toml` lists under `[unsafe] modules`.
    pub unsafe_: bool,
    /// `must_use fun ...` (MISSING-Cn2Ym): a call used as a statement, with its
    /// result thrown away, is a compile error. `val _r = f()` is the explicit
    /// discard.
    pub must_use: bool,
    /// `extern fun g(a: T, ...)` - a *user* extern whose last parameter is the
    /// `...` variadic marker (gap 7). A call passes the fixed arguments and
    /// then any number of extra arguments, each of a scalar, `bool`, `str` or
    /// opaque-handle type. Only a module with a `coral.toml`
    /// `[extern."<module id>"]` section may write one; `infer.rs` is where
    /// that is enforced.
    pub variadic: bool,
    /// `Some` for `fun Type.name(...)` / `fun mut Type.name(...)` -
    /// receiver-form methods. The receiver is *not* one of `params`; it's
    /// a separate implicit first argument, matching how the surface syntax
    /// keeps it outside the parens.
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    /// The `=> T` clause. `None` means `Unit`.
    pub return_type: Option<TypeExpr>,
    pub body: FunctionBody,
    /// See `DatacDecl::module`.
    pub module: String,
    /// A synthesized trait default (A-12 slice 3) resolves its *body* here when
    /// that differs from `module`: the default was written in the trait's
    /// module, but its signature (`Self` and the types that name it) was
    /// substituted with the implementing type and so resolves in the impl's
    /// module. `None` for every declaration written by a user.
    pub body_module: Option<String>,
    /// `naked fun ...` (kernel stage 5): LLVM's `naked` and `noinline`, whose
    /// body must be exactly one `asm(...)` statement, so no prologue or
    /// epilogue is emitted around it.
    pub naked: bool,
    /// `interrupt fun ...` (kernel stage 5): a CPU interrupt or trap handler.
    /// LLVM saves every register the body touches and returns with the
    /// target's interrupt-return instruction (`mret` on riscv64, `iretq` on
    /// x86-64). Only legal in a freestanding build, with no parameters, no
    /// result and no direct call from Kelp code.
    pub interrupt: bool,
    /// The `align(N)` clause, when one was written (kernel gap 2): the
    /// function's alignment in bytes, so a trap vector (`mtvec`/`VBAR`) can
    /// point at it. `None` is the target's default function alignment.
    pub align: Option<u32>,
    /// The `section("<name>")` clause, when one was written (kernel stage 4):
    /// the object section this function's code is emitted in, so a linker
    /// script can place it. `None` is the target's default `.text`.
    pub section: Option<String>,
    pub span: Span,
}

/// `Type` (shared) or `mut Type` (exclusive) before the `.` in a receiver
/// declaration, e.g. `Circle` in `fun Circle.area()` or `Circle` +
/// `mutable: true` in `fun mut Circle.grow()`.
#[derive(Debug, Clone)]
pub struct Receiver {
    pub type_name: String,
    /// `fun Pair<A, B>.first()` - the receiver's type arguments, which are the
    /// method's type parameters (A-2). They must be exactly the generic type's
    /// own parameter list, in order, so the body can name them.
    pub type_args: Vec<String>,
    /// The `const` parameters of the receiver's own parameter list (A-20),
    /// when one of them is declared with `const` (`fun Buf<const N>.push()`).
    /// Empty for a receiver whose list is all type parameters.
    pub const_params: Vec<ConstParamDecl>,
    pub mutable: bool,
    /// True for a receiver written without `mut` on a `fun Type.name` line
    /// outside a datac block and outside an `impl` or trait: its mode is
    /// decided by the method's body (issue #286), exclusive when the body
    /// writes self. `false` for every other receiver, whose mode is written.
    pub inferable: bool,
    /// True for a method written inside the braces of its datac (issue #285).
    /// Its mode is written and never inferred, so a block method that writes
    /// self without `mut` is refused with the `mut` form named (issue #286).
    pub in_block: bool,
    pub span: Span,
}

/// One `const N: usize` parameter (A-20). It is a `const` type parameter:
/// a parameter of a `datac`/`enum`/`fun` whose argument is a compile-time
/// value rather than a type, and which monomorphizes into one copy per
/// distinct value exactly as a type parameter does.
#[derive(Debug, Clone)]
pub struct ConstParamDecl {
    pub name: String,
    /// The declared integer type, as written (`usize`, `u8`..`u64`, `i32`,
    /// `i64`, `bool`).
    pub ty: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FunctionBody {
    Block(Block),
    /// `fun f(...) => T = expr` - sugar checked/lowered as if it were
    /// `fun f(...) => T { return expr }`.
    Expr(Expr),
    /// `extern fun f(...)`: no body in `.kp` at all; the removed C backend wrote
    /// the compiler-supplied C body instead. Never reaches `check_function`'s
    /// block/expression paths.
    Extern,
    /// A method signature inside a `trait` body (A-12 slice 2): no body at all,
    /// because a trait declares only what an `impl` has to provide. A
    /// `Signature` never reaches a code generator: the impl's own method is what
    /// is emitted.
    Signature,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    /// `Borrow` (the default), `mut T` (an exclusive borrow) or `move T`
    /// (the callee takes ownership, A-4).
    pub mode: ParamMode,
    /// `name: T = literal` (issue #296): the value a call uses when it leaves
    /// this argument out. Always one literal, checked at each call.
    pub default: Option<Expr>,
}

/// How a parameter receives its argument. One enum rather than two flags,
/// because the three are exclusive by construction: `mut move T` is not a
/// state that can be represented, so it cannot be checked for either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// The default: an implicit shared borrow for a `datac`-shaped type,
    /// plain by-value for a `Copy` type.
    Borrow,
    /// `mut T` - an exclusive borrow.
    Mut,
    /// `move T` - the callee owns the argument and the call site moves it.
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
/// There is no binding name - a function type is not a declaration - so this
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
    /// The element types of a tuple type, `(A, B)` (`design/syntax.md (kelp wiki)` 3):
    /// `Some`, with two or more entries, exactly when the type was written in
    /// parentheses with a comma. `name` is empty in that case, because a tuple
    /// type has no name of its own: it is its element list. `(T)` is not a
    /// tuple and is parsed as the parenthesized type `T`; `()` is refused.
    pub tuple: Option<Vec<TypeExpr>>,
    /// Type arguments, for the one generic the compiler knows: `List<T>`.
    /// Empty for every ordinary type name, and the parser only accepts them
    /// after a bare or dotted name - `List<i32>`, never `math.List<i32>`.
    /// `infer.rs` rejects arguments on a name that is not `List`, and more
    /// than one on `List` itself; v1 has no user-definable generics.
    pub args: Vec<TypeExpr>,
    /// The element type of `[T; N]` (A-18), `Some` exactly when the type was
    /// written with brackets. `name` is empty in that case, because an array
    /// type has no name of its own: it is its element and its length.
    pub array_elem: Option<Box<TypeExpr>>,
    /// `I::Item` / `Self::Item` (A-12 slice 5): the associated type `assoc`
    /// projected from the type parameter (or `Self`) named in `name`. `None`
    /// for every ordinary type; the parser only sets it after a `::`.
    pub assoc: Option<String>,
    /// The `N` of `[T; N]`, kept as an expression because a length is
    /// allowed to be computed from constants (`PAGE_SIZE / ENTRY_SIZE`,
    /// A-19), folded by `infer.rs` when the type is resolved.
    pub array_len: Option<Box<Expr>>,
    /// `&T` was written with a leading `&`. Only meaningful in a *return*
    /// position (M6.3): `&str`/`&T` there is a borrow of the caller's value
    /// under the elision rule, whereas a parameter's `&str` is what `str`
    /// already is (a borrowed view passed by value).
    pub amp: bool,
    /// `&mut` was written before `dyn` (A-12 slice 4): the exclusive form of a
    /// borrowed trait object, `&mut dyn Name`. A `&mut` before an ordinary type
    /// name is still refused (a parameter writes `mut T` instead).
    pub amp_mut: bool,
    /// `&dyn Name` / `&mut dyn Name` (A-12 slice 4): the trait of a borrowed
    /// trait object, as written (`Shape`, or `shape.Shape` through an alias).
    /// `None` for every ordinary type; the parser only sets it after a leading
    /// `&` (with or without `mut`).
    pub dyn_trait: Option<String>,
    /// `scoped fun(T) => U` - a *contextual* modifier (`scoped` is not a
    /// keyword, and is only a modifier when the next token is `fun`). A
    /// scoped function value may only be written at a call site and may not
    /// outlive the call (`kelp wiki page language/functions.md`). Legal only on a top-level
    /// parameter of a declaration, which `infer.rs` enforces: nested in
    /// another function type it could be stored as that type, which is the
    /// one thing the modifier forbids.
    pub scoped: bool,
    /// `fun(i32, String) => bool` - the parameters, when this is a function
    /// type (A-4). `Some`, even when empty (`fun()`), is what marks it as one;
    /// `None` for every ordinary type. `name` is `"fun"` in that case, or
    /// `"closure"` for the owned, environment-carrying form (A-4 phase E);
    /// neither can collide with a real name because both are keywords.
    pub fn_params: Option<Vec<FnTypeParam>>,
    /// The `=> T` of a function type. `None` means `Unit`, exactly as it
    /// does on a declaration.
    pub fn_ret: Option<Box<TypeExpr>>,
    /// A **const argument** (A-20): the expression `N` of `Buf<16>` or
    /// `Buf<SIZE * 2>`. `Some` exactly when this "type argument" is really a
    /// compile-time value, which is decided where the parameter it binds is
    /// declared: a type parameter takes a type and a `const` parameter takes
    /// this value. A bare name is also kept here (`Buf<SIZE>`) so a `const`
    /// parameter can fold it, because the two are written identically.
    pub const_expr: Option<Box<Expr>>,
    /// `impl Name` written as a parameter's type (A-20). `Some` is the trait,
    /// as written. The parser records it and the desugar in `parser.rs`
    /// rewrites it into a fresh bounded type parameter, so no pass after the
    /// parser sees this shape; `infer.rs` refuses any that is left (a `let`,
    /// a field, a return).
    pub impl_trait: Option<String>,
    /// The equality constraints written on a bound (`I: Iterator<Item = i64>`):
    /// each `(assoc, type)` pair, in source order (MISSING-Ve8Nc step 5). Empty
    /// for every bound without one, and on every `TypeExpr` that is not a bound.
    pub bound_eq: Vec<(String, TypeExpr)>,
    pub span: Span,
}

impl TypeExpr {
    /// An ordinary named type with no arguments - the shape almost every
    /// construction site wants.
    pub fn named(name: impl Into<String>, span: Span) -> TypeExpr {
        TypeExpr {
            name: name.into(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc: None,
            array_len: None,
            amp: false,
            amp_mut: false,
            dyn_trait: None,
            scoped: false,
            fn_params: None,
            fn_ret: None,
            const_expr: None,
            impl_trait: None,
            bound_eq: Vec::new(),
            span,
        }
    }

    /// A const argument written like a type argument (`Buf<16>`): the folded
    /// expression is carried in `const_expr`, and the name is left empty.
    pub fn const_arg(expr: Expr, span: Span) -> TypeExpr {
        TypeExpr {
            name: String::new(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc: None,
            array_len: None,
            amp: false,
            amp_mut: false,
            dyn_trait: None,
            scoped: false,
            fn_params: None,
            fn_ret: None,
            const_expr: Some(Box::new(expr)),
            impl_trait: None,
            bound_eq: Vec::new(),
            span,
        }
    }

    /// Whether this is a closure type (`closure fun(i32) => i32`) - the owned,
    /// environment-carrying form of a function type (A-4 phase E).
    pub fn is_closure(&self) -> bool {
        self.fn_params.is_some() && self.name == "closure"
    }

    /// Whether this is a plain function type (`fun(i32) => i32`).
    pub fn is_fn(&self) -> bool {
        self.fn_params.is_some()
    }

    /// The type as it was written, for diagnostics and the pretty-printer.
    pub fn display(&self) -> String {
        if let Some(trait_name) = &self.impl_trait {
            return format!("impl {trait_name}");
        }
        if let Some(e) = &self.const_expr {
            return e.display_source();
        }
        if let Some(name) = &self.dyn_trait {
            // The `amp` flag is what tells the borrowed spelling from a bare
            // `dyn Name`, which is legal only as the argument of a `Box`
            // (A-12 slice 7): the bare spelling resolves to its own type, so
            // re-emitting it with a `&` would name a different one.
            return if self.amp_mut {
                format!("&mut dyn {name}")
            } else if self.amp {
                format!("&dyn {name}")
            } else {
                format!("dyn {name}")
            };
        }
        if let Some(item) = &self.assoc {
            return format!("{}::{item}", self.name);
        }
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
            let modifier = if self.scoped { "scoped " } else { "" };
            let keyword = if self.is_closure() { "closure fun" } else { "fun" };
            format!("{}{}({})", modifier, keyword, args.join(", "))
        } else if self.args.is_empty() {
            self.name.clone()
        } else {
            let args: Vec<String> = self.args.iter().map(|a| a.display()).collect();
            format!("{}<{}>", self.name, args.join(", "))
        };
        if let Some(ret) = &self.fn_ret {
            s.push_str(&format!(" => {}", ret.display()));
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// One binding position of a destructuring `val`/`var` (`design/syntax.md (kelp wiki)` 4):
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
    /// apart into its positions (`design/syntax.md (kelp wiki)` 4). A statement of its own
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
        /// The place a `for` loop walks, when it walks one (`xs`, `self.items`).
        /// `infer.rs` copies its path and span into the typed loop so
        /// `borrowck.rs` can refuse a mutation of it while the body runs
        /// (`design/syntax.md (kelp wiki)` 6.2). `None` for a written `while` and for a `for`
        /// over a temporary, which nothing names.
        iterating: Option<Expr>,
        span: Span,
    },
    /// `for x in <expr> { ... }` (`kelp wiki page language/iterators.md`). Kept as written rather
    /// than desugared here, because which desugar applies depends on the
    /// iterable's type: a `List<T>`/`[T; N]` keeps the index walk, and an
    /// `Iterator` becomes the `next()` loop. `infer.rs` is the first pass that
    /// knows the type, so it does the desugar and `borrowck.rs` and both
    /// backends only ever see the `while` it produces.
    For {
        binding: String,
        binding_span: Span,
        iterable: Expr,
        body: Block,
        span: Span,
    },
    /// `break` (`kelp wiki page language/iterators.md`): leave the innermost loop. A statement of
    /// its own so both backends branch out of the loop they are emitting.
    Break { span: Span },
    /// `continue`: skip to the next pass of the innermost loop.
    Continue { span: Span },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// `defer EXPR` (kernel stage 6): the action runs when the enclosing block
    /// is left, on every ordinary exit, in last-in-first-out order against the
    /// block's other defers (`kelp wiki page language/statics-and-defer.md`). The parser only records it;
    /// `defer.rs` rewrites it into ordinary code before name resolution, so
    /// nothing after that pass sees this variant. The action's value is
    /// discarded, and the span is the `defer` statement's, which is what a
    /// spliced copy reports so a diagnostic names the `defer` and not the exit
    /// it was placed before.
    Defer {
        expr: Expr,
        span: Span,
    },
    /// `errdefer EXPR` (kernel stage 6): like `defer`, but the action runs only
    /// when the block is left by an error exit, a `return` or a `?` whose value
    /// is the failure variant (`design/syntax.md (kelp wiki)`, errdefer). `defer.rs` rewrites
    /// it away before name resolution, as it does `Defer`.
    Errdefer {
        expr: Expr,
        span: Span,
    },
    /// An expression used as a full statement. Whether it's the block's
    /// *tail* (value-producing) or purely a side-effecting statement is
    /// decided positionally - textually the block's last statement is the
    /// tail - there's no `;` to mark it either way now that statements are
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
    /// `~a` - bitwise complement. Prefix only: the language has no `~`
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
/// assigned into, or matched without moving what it names - an `Ident`, a
/// `.field` chain rooted in one, or a dereferenced borrow (`*p`, `(*p).field`).
/// Shared between `parser.rs` (which uses it to decide whether `if let`'s
/// desugared scrutinee needs a fresh owning temporary - see `parse_if_let`)
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

/// The canonical path of a place expression: `xs` for an `Ident`, `self.items`
/// for a field chain, and `None` for anything else (a temporary, a dereference,
/// a call). `parser.rs` records the place a `for` walks this way and `infer.rs`
/// hands the same path to the borrow checker, so both agree on what names a
/// list.
pub fn place_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::FieldAccess { base, field, .. } => {
            place_path(base).map(|b| format!("{b}.{field}"))
        }
        _ => None,
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

mod exprs;
pub(crate) use exprs::*;

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
            | Expr::TupleLit { span: s, .. }
            | Expr::Asm { span: s, .. } => *s,
            Expr::Block(b) => b.span,
            Expr::Unsafe { span, .. } => *span,
            Expr::ManualBlock { span, .. } => *span,
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

/// The first `unsafe` construct in a parsed module, if it has one, as
/// `(span, what)`: an `unsafe fun` declaration or an `unsafe { ... }` block.
///
/// `resolve.rs` uses this to enforce the manifest's `[unsafe] modules` list:
/// a module may contain `unsafe` only if `coral.toml` names it, and the
/// diagnostic points at the first construct rather than at the file.
pub fn first_unsafe(program: &Program) -> Option<(Span, &'static str)> {
    for f in &program.functions {
        if f.unsafe_ {
            return Some((f.span, "fun"));
        }
        if let FunctionBody::Block(b) = &f.body {
            if let Some(span) = first_unsafe_in_block(b) {
                return Some((span, "block"));
            }
        }
        if let FunctionBody::Expr(e) = &f.body {
            if let Some(span) = first_unsafe_in_expr(e) {
                return Some((span, "block"));
            }
        }
    }
    // A trait's default methods live in the declaration, not in `functions`,
    // and an `unsafe` there runs in every impl that keeps the default: it has
    // to put the module in the audit list like any other (threat model U3).
    for t in &program.traits {
        for f in &t.methods {
            if f.unsafe_ {
                return Some((f.span, "fun"));
            }
            match &f.body {
                FunctionBody::Block(b) => {
                    if let Some(span) = first_unsafe_in_block(b) {
                        return Some((span, "block"));
                    }
                }
                FunctionBody::Expr(e) => {
                    if let Some(span) = first_unsafe_in_expr(e) {
                        return Some((span, "block"));
                    }
                }
                FunctionBody::Extern | FunctionBody::Signature => {}
            }
        }
    }
    for p in &program.pds {
        if let Some(span) = first_unsafe_in_expr(&p.init) {
            return Some((span, "block"));
        }
    }
    for c in &program.consts {
        if let Some(span) = first_unsafe_in_expr(&c.init) {
            return Some((span, "block"));
        }
    }
    for s in &program.statics {
        if let Some(span) = first_unsafe_in_expr(&s.init) {
            return Some((span, "block"));
        }
    }
    None
}

fn first_unsafe_in_block(block: &Block) -> Option<Span> {
    block.stmts.iter().find_map(|s| match s {
        Stmt::Let { init, .. } => first_unsafe_in_expr(init),
        Stmt::LetTuple { init, .. } => first_unsafe_in_expr(init),
        Stmt::While { cond, body, .. } => {
            first_unsafe_in_expr(cond).or_else(|| first_unsafe_in_block(body))
        }
        Stmt::For {
            iterable, body, ..
        } => first_unsafe_in_expr(iterable).or_else(|| first_unsafe_in_block(body)),
        Stmt::Return { value, .. } => value.as_ref().and_then(first_unsafe_in_expr),
        Stmt::Defer { expr, .. } | Stmt::Errdefer { expr, .. } => first_unsafe_in_expr(expr),
        Stmt::Break { .. } | Stmt::Continue { .. } => None,
        Stmt::Expr { expr, .. } => first_unsafe_in_expr(expr),
    })
}

fn first_unsafe_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        // An `asm(...)` is an unsafe operation (kernel stage 5): writing one is
        // what puts its module in the manifest's audit list, exactly as an
        // `unsafe` block does.
        Expr::Asm { span, .. } => Some(*span),
        Expr::Unsafe { span, .. } => Some(*span),
        Expr::Unary { expr, .. } | Expr::Ref { expr, .. } | Expr::Try { expr, .. } | Expr::Deref { expr, .. } => {
            first_unsafe_in_expr(expr)
        }
        Expr::Binary { lhs, rhs, .. } => {
            first_unsafe_in_expr(lhs).or_else(|| first_unsafe_in_expr(rhs))
        }
        Expr::Call { args, .. } | Expr::MethodCall { args, .. } => args
            .iter()
            .find_map(|a| first_unsafe_in_expr(&a.value))
            .or_else(|| match expr {
                Expr::MethodCall { receiver, .. } => first_unsafe_in_expr(receiver),
                _ => None,
            }),
        Expr::FieldAccess { base, .. } => first_unsafe_in_expr(base),
        Expr::Assign { target, value, .. } => {
            first_unsafe_in_expr(target).or_else(|| first_unsafe_in_expr(value))
        }
        Expr::If { cond, then_block, else_block, .. } => first_unsafe_in_expr(cond)
            .or_else(|| first_unsafe_in_block(then_block))
            .or_else(|| match else_block.as_deref() {
                Some(ElseBranch::Block(b)) => first_unsafe_in_block(b),
                Some(ElseBranch::If(e)) => first_unsafe_in_expr(e),
                None => None,
            }),
        Expr::Block(b) => first_unsafe_in_block(b),
        Expr::FnLit { body, .. } => match body.as_ref() {
            FunctionBody::Block(b) => first_unsafe_in_block(b),
            FunctionBody::Expr(e) => first_unsafe_in_expr(e),
            FunctionBody::Extern | FunctionBody::Signature => None,
        },
        Expr::Template { parts, .. } => parts.iter().find_map(|p| match p {
            TemplatePart::Text(_) => None,
            TemplatePart::Expr(e) => first_unsafe_in_expr(e),
        }),
        Expr::When { scrutinee, arms, .. } => first_unsafe_in_expr(scrutinee)
            .or_else(|| arms.iter().find_map(|a| first_unsafe_in_expr(&a.body))),
        Expr::ArrayLit { elems, .. } => elems.iter().find_map(first_unsafe_in_expr),
        Expr::ArrayRepeat { value, len, .. } => {
            first_unsafe_in_expr(value).or_else(|| first_unsafe_in_expr(len))
        }
        Expr::TupleLit { elems, .. } => elems.iter().find_map(first_unsafe_in_expr),
        _ => None,
    }
}

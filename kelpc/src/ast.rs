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
}

/// One `trait Name { fun Self.m(args) => T ... }` declaration (A-12 slice 2).
#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub name: String,
    /// `pub trait ...` (A-13). See `ConstDecl::is_pub`.
    pub is_pub: bool,
    /// `trait From<T>` (A-12 slice 6): the trait's own type parameters, in
    /// order. Empty for an ordinary trait. A parameter is in scope in every
    /// method signature and default body of the trait.
    pub type_params: Vec<String>,
    /// The bounds on each parameter of `type_params`, in the same order and
    /// aligned with it, exactly as `FunctionDecl::type_param_bounds` is.
    pub type_param_bounds: Vec<Vec<TypeExpr>>,
    /// `trait B: A + C` (A-12 slice 3): the supertraits, in source order. Each
    /// is stored as the bound parser wrote it (`A`, `alias.A`); `infer.rs`
    /// qualifies and resolves them, exactly as it does a bound on a function's
    /// type parameter.
    pub supertraits: Vec<TypeExpr>,
    /// The methods, in source order. Each is an ordinary `FunctionDecl` with a
    /// `Self` receiver; a `FunctionBody::Signature` is a required signature and
    /// any other body is a default method an impl may omit.
    pub methods: Vec<FunctionDecl>,
    /// `type Item` / `type Item: Named` (A-12 slice 5), in source order. Each
    /// impl has to bind every one of them.
    pub assoc_types: Vec<AssocTypeDecl>,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// One `type Name[: Bound + Bound]` inside a `trait` body (A-12 slice 5).
#[derive(Debug, Clone)]
pub struct AssocTypeDecl {
    pub name: String,
    /// The bounds after a `:`, as written (`Named`, `alias.Named`), resolved
    /// in the trait's own module by `infer.rs`, exactly as a supertrait is.
    pub bounds: Vec<TypeExpr>,
}

/// One `impl Name for Type { ... }` block (A-12 slice 2).
#[derive(Debug, Clone)]
pub struct TraitImplDecl {
    /// The trait, as written (`Shape`, or `shape.Shape` through an alias).
    /// `resolve.rs` rewrites it to its canonical, module-qualified key before
    /// `infer.rs` sees it.
    pub trait_name: String,
    /// `impl From<IoErr> for AppErr` (A-12 slice 6): the trait's arguments, as
    /// written. Empty for an ordinary trait. `resolve.rs` canonicalizes each
    /// one and derives the trait-instance key the impl and the methods carry.
    pub trait_args: Vec<TypeExpr>,
    /// The canonical name of each trait argument (`pkg.IoErr`, `i32`), so
    /// `infer.rs` can build the concrete type without a second scope.
    pub trait_arg_names: Vec<String>,
    /// The canonical trait-instance key (`pkg.From<pkg.IoErr>`), or the plain
    /// canonical trait name for an ordinary trait. Set by `resolve.rs`.
    pub trait_instance: String,
    /// The implementing type, as written. Same qualification rule.
    pub type_name: String,
    /// The trait name's own span, and the `for` type's.
    pub trait_span: Span,
    pub type_span: Span,
    /// The names the block declares, in source order, so an extra or a
    /// missing one can be reported against the block.
    pub methods: Vec<String>,
    /// `type Item = i32` (A-12 slice 5), in source order. Every associated
    /// type the trait declares has to be bound here, exactly once.
    pub assoc_types: Vec<AssocTypeBinding>,
    /// See `DatacDecl::module`.
    pub module: String,
    /// The `impl` keyword's own span, what a coherence error points at.
    pub impl_span: Span,
    /// `impl<T: Bound> Trait for Type<T>`: the impl's own type parameters, which
    /// are the generic type's own in order, and the bounds on each. Empty for an
    /// impl on a concrete type. A generic impl is a set of template methods plus
    /// a condition (the bounds) under which an instance implements the trait.
    pub type_params: Vec<String>,
    pub type_param_bounds: Vec<Vec<TypeExpr>>,
    /// `where I::Item: Bound` on the impl (MISSING-Ve8Nc step 5), as for a function.
    pub projection_bounds: Vec<ProjectionBound>,
}

/// One `where P::Assoc: Bound` clause (MISSING-Ve8Nc step 5): the bound `bound`
/// on the associated type `assoc` of the type parameter `param`.
#[derive(Debug, Clone)]
pub struct ProjectionBound {
    pub param: String,
    pub assoc: String,
    pub bound: TypeExpr,
}

/// One `type Name = Type` inside an `impl` body (A-12 slice 5).
#[derive(Debug, Clone)]
pub struct AssocTypeBinding {
    pub name: String,
    /// The concrete type, as written. `infer.rs` resolves it in the impl's own
    /// module, exactly as it does an impl method's signature.
    pub ty: TypeExpr,
    pub span: Span,
}

/// The site one method was written at: `Some` exactly for a method inside an
/// `impl Name for Type { ... }` block (A-12 slice 2). `resolve.rs` substitutes
/// the written type for every `Self` in the method's signature before any later
/// pass sees it, so nothing downstream knows the block existed.
#[derive(Debug, Clone)]
pub struct TraitImplSite {
    /// The trait, as written.
    pub trait_name: String,
    /// `impl From<IoErr> for AppErr`: the trait's arguments as written, so
    /// `resolve.rs` can tell one instantiation's method from another's when
    /// both are for the same type (A-12 slice 6). Empty for an ordinary trait.
    pub trait_args: Vec<TypeExpr>,
    /// `impl From<IoErr> for AppErr` (A-12 slice 6): the canonical
    /// trait-instance key (`pkg.From<pkg.IoErr>`), empty for an ordinary
    /// trait. `resolve.rs` computes it before `infer.rs` sees the method, and
    /// it is what keeps a static impl method of one instance distinct from the
    /// same-named method of another (`AppErr#from@pkg.From<pkg.IoErr>`).
    pub trait_instance: String,
    /// The type `Self` stands for, as written in the `for` clause.
    pub self_type: String,
}

/// One `extern type Widget`: an opaque, pointer-sized native handle. It is a
/// named type like a `datac` and is exported the same way, but it has no
/// fields, no methods, no dereference, no arithmetic and no conversion to or
/// from an integer. It is legal only in a module whose `coral.toml` carries an
/// `[extern."<module id>"]` section; `resolve.rs` refuses it anywhere else.
///
/// `extern type Window: Widget` adds a parent: a `Window` value converts
/// implicitly to `Widget` (never the reverse), which is the one relation two
/// handles can have.
#[derive(Debug, Clone)]
pub struct ExternTypeDecl {
    pub name: String,
    /// The written parent from `extern type Name: Parent`, if any: another
    /// `extern type` declared earlier in this module or imported. Left as
    /// written; `infer.rs` qualifies and resolves it, exactly as it does a
    /// field's type name.
    pub parent: Option<String>,
    /// `pub extern type ...` (A-13). See `ConstDecl::is_pub`.
    pub is_pub: bool,
    /// See `DatacDecl::module`.
    pub module: String,
    pub span: Span,
}

/// One `const NAME: T = expr` - a top-level compile-time constant (M6.3).
/// The type is mandatory (a `const` is a declared contract, unlike `val`),
/// and the initializer has to fold to a literal, so there is nothing to
/// allocate and nothing to drop.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    /// `pub const ...` (A-13, `design/syntax.md (kelp wiki)` 13.4). Every top-level item is
    /// private to its module unless this is written, which is what
    /// `infer.rs`'s visibility gate reads.
    pub is_pub: bool,
    pub ty: TypeExpr,
    pub init: Expr,
    /// See `DatacDecl::module`.
    pub module: String,
    /// The `section("<name>")` clause, when one was written (kernel stage 4):
    /// the object section a top-level integer-array `const` is emitted in, so a
    /// linker script can place it. `None` is the compilers's default section,
    /// which is every const written before this clause existed.
    pub section: Option<String>,
    pub span: Span,
}

/// One `static NAME: T = expr` - a top-level global object (kernel stage 7).
/// Unlike a `const`, which folds to a literal, a static is one object with a
/// stable address, so its read is a load and `&NAME` is meaningful.
#[derive(Debug, Clone)]
pub struct StaticDecl {
    pub name: String,
    /// `pub static ...` (A-13). See `ConstDecl::is_pub`.
    pub is_pub: bool,
    /// `static mut NAME` - every access to it is `unsafe`.
    pub mutable: bool,
    /// `percpu static mut NAME` (kernel stage 8, MK9-S3-M3): the static is one
    /// object *per CPU*, laid out as the manifest's `[target] cpus` copies and
    /// indexed by `cpu_index()`. Legal only in a freestanding build, and only
    /// with `mut`.
    pub percpu: bool,
    pub ty: TypeExpr,
    /// The constant initializer, checked to fold at compile time.
    pub init: Expr,
    /// See `DatacDecl::module`.
    pub module: String,
    /// The `section("...")` clause, when one was written. See
    /// `ConstDecl::section`.
    pub section: Option<String>,
    pub span: Span,
}

/// One `pd val name = expr` / `pd var name: T = expr` - a passdown
/// (`design/syntax.md (kelp wiki)` §9): a top-level binding a function can name without
/// being passed it.
#[derive(Debug, Clone)]
pub struct PdDecl {
    pub name: String,
    /// `true` for `pd var`. Per the plan's resolution of §9's open question,
    /// writability is confined to the top level either way - inside a
    /// function body a passdown is read-only - so this only records which
    /// keyword was written (and is what a future top-level statement form
    /// would consult).
    pub mutable: bool,
    /// `true` for `tl`: thread-local storage. Each thread has its own copy, and
    /// `init` is a literal every copy starts from (issue 234).
    pub thread_local: bool,
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
    /// The string literal's contents, verbatim (`shapes/circle` - no `.kp`).
    pub path: String,
    /// `caps(domain = [patterns...])` grants. Empty when the clause is
    /// absent. Two jobs in one syntax: it *delegates* the domain to the
    /// imported module, and it is how the importing module states which
    /// patterns it is willing to hand over.
    pub caps: Vec<CapGrant>,
    /// `caps(*)`: delegate the whole resource of every domain, whatever the
    /// importing module holds. Mutually exclusive with `caps` by construction:
    /// the parser refuses a named domain beside the wildcard.
    pub caps_all: bool,
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

/// A `repr(...)` clause written before a `datac` or an `enum` (kernel stage 2,
/// MK9-S1-M2 / B-2). The absence of one is [`Repr::C`]: declaration order with
/// the C alignment and padding rules, which is what the default layout already
/// was and is now guaranteed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repr {
    /// `repr(c)`, and every declaration with no clause at all.
    C,
    /// `repr(packed)`: alignment 1 and no padding between fields.
    Packed,
    /// `repr(align(N))`: the natural (C) field layout with the type's alignment
    /// raised to `N`, a power of two. `N` is a literal, checked at parse time.
    Align(u64),
    /// `repr(u8)` and its siblings: the integer type a field-less `enum`'s tag
    /// is stored in. Not legal on a `datac`.
    Int(String),
}

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

    /// Whether this is a closure type (`closure(i32) => i32`) - the owned,
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
            format!("{}{}({})", modifier, self.name, args.join(", "))
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

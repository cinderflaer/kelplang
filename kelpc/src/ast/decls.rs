use super::*;

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

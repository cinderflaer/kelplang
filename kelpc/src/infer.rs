use std::collections::HashMap;

use crate::ast::{self, BinOp, Repr, UnOp};
use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::resolve::Project;
use crate::tir;
use crate::types::{EnumDef, FieldDef, FnSig, StructDef, Ty};

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

type Result<T> = std::result::Result<T, TypeError>;

impl TypeError {
    /// A `Span` only means something inside the file it came from, so every
    /// error leaving this pass is tagged with the module that produced it.
    fn in_module(self, module: &str) -> Diagnostic {
        Diagnostic::new(self.message, self.span, module)
    }

    /// The same error re-pointed at another span, for the sites that only
    /// learn the real location after an inner resolution succeeds: a parameter
    /// type resolved at a call site belongs to the call, not to the
    /// declaration it was written in.
    fn at(mut self, span: Span) -> TypeError {
        self.span = span;
        self
    }
}

#[derive(Clone)]
struct VarInfo {
    ty: Ty,
    mutable: bool,
}

/// One user-declared generic type (A-2): a `datac` or `enum` written with
/// type parameters. Its fields are kept *as written* rather than resolved,
/// because a type parameter has no type until an instantiation binds one -
/// resolution happens in `TypeScope::instantiate`, with the parameters bound
/// to the concrete arguments.
#[derive(Clone)]
struct GenericTemplate {
    /// Qualified name, exactly the key a non-generic `datac` of the same name
    /// would have had.
    name: String,
    params: Vec<String>,
    /// The declared type of each `const` parameter of `params`, in declaration
    /// order (A-20). A name here is also in `params`; this is what says which
    /// entries are values and how wide their values are.
    const_params: Vec<(String, Ty)>,
    kind: GenericKind,
    /// The `repr(...)` clause of the declaration, so an instantiation keeps
    /// the layout its template asked for (kernel stage 2).
    repr: Repr,
    /// The declaring module's symbol prefix and alias table: a field type
    /// written inside the template resolves exactly where it was written.
    prefix: String,
    aliases: HashMap<String, String>,
}

#[derive(Clone)]
enum GenericKind {
    Record(Vec<ast::FieldDecl>),
    Sum(Vec<ast::VariantDecl>),
}

/// One `trait Name { fun Self.m(...) ... }` declaration (A-12 slice 2). The
/// method signatures stay as written, because `Self` has no meaning until a
/// type implements the trait: `check_trait_impls` binds it per impl.
#[derive(Clone)]
struct TraitInfo {
    /// Qualified name, exactly the key a `datac` of the same name would have.
    name: String,
    /// `trait From<T>` (A-12 slice 6): the trait's own type parameters, in
    /// order. Empty for an ordinary trait.
    type_params: Vec<String>,
    /// The bound traits on each parameter of `type_params`, as
    /// `(written, canonical)` and in the same order, so an impl's argument can
    /// be checked against them. Empty list per parameter for no bound.
    type_param_bounds: Vec<Vec<(String, String)>>,
    /// The declaring module's symbol prefix and alias table, so a signature's
    /// types resolve where the trait was written rather than where it is
    /// implemented.
    prefix: String,
    aliases: HashMap<String, String>,
    /// The trait's own methods, in source order: a `FunctionBody::Signature`
    /// is required, any other body is a default an impl may omit (A-12 slice
    /// 3).
    methods: Vec<ast::FunctionDecl>,
    /// The supertraits, canonical and in source order (A-12 slice 3).
    /// Implementing this trait requires an impl of each, and a bound on this
    /// trait satisfies a bound on any of them.
    supertraits: Vec<String>,
    /// The associated types the trait declares (A-12 slice 5), in source order.
    assoc_types: Vec<AssocTypeInfo>,
    module: String,
}

/// One `type Item[: Bound + Bound]` a trait declares (A-12 slice 5).
#[derive(Clone)]
struct AssocTypeInfo {
    name: String,
    /// The bound traits, each `(written, canonical)`, in source order: the
    /// written name is what a diagnostic quotes, the canonical one is what the
    /// impl's type is checked against.
    bounds: Vec<(String, String)>,
}

/// The concrete `Ty` of each trait argument, from the canonical names
/// `resolve.rs` computed (A-12 slice 6). A canonical name is a `Ty::Struct` key
/// unless it is a builtin, so no second scope is needed.
fn trait_arg_tys(names: &[String]) -> Vec<Ty> {
    names
        .iter()
        .map(|n| Ty::from_name(n).unwrap_or_else(|| Ty::Struct(n.clone())))
        .collect()
}

/// The `sigs` key one impl method is stored under (A-12 slice 6): a receiver
/// method is `Type#name`, and a static member carries its trait instance
/// (`Type#name@Trait<Args>`), which is what lets one type implement
/// `From<A>` and `From<B>` at once.
fn impl_method_key(imp: &TraitImplInfo, m: &ast::FunctionDecl) -> String {
    if m.receiver.is_none() {
        format!("{}#{}@{}", imp.type_name, m.name, imp.trait_instance)
    } else {
        format!("{}#{}", imp.type_name, m.name)
    }
}

/// One `impl Name for Type { ... }` block (A-12 slice 2), with both names
/// qualified in the impl's own module. The methods themselves are ordinary
/// receiver methods on the type, so this holds only what the pairing checks
/// need.
#[derive(Clone)]
struct TraitImplInfo {
    /// The canonical trait name (`pkg.From`), i.e. the trait *declaration*'s
    /// key, not the instance's.
    trait_name: String,
    /// The canonical trait-instance key (A-12 slice 6): `pkg.From<pkg.IoErr>`
    /// for a generic trait, the trait name alone for an ordinary one.
    trait_instance: String,
    /// The canonical name of each trait argument (`pkg.IoErr`, `i32`), in
    /// order, so the concrete `Ty` the trait's parameters are substituted with
    /// is built with no second scope.
    trait_arg_names: Vec<String>,
    type_name: String,
    methods: Vec<String>,
    /// `type Item = i32` (A-12 slice 5), in source order.
    assoc_types: Vec<(String, ast::TypeExpr, Span)>,
    module: String,
    impl_span: Span,
    /// `impl<T: Bound> Trait for Type<T>`: the impl's own parameters (the generic
    /// type's, in order) and the bounds on each. Empty for a concrete type.
    type_params: Vec<String>,
    type_param_bounds: Vec<Vec<ast::TypeExpr>>,
    /// `where I::Item: Bound` on the impl (MISSING-Ve8Nc step 5).
    projection_bounds: Vec<ast::ProjectionBound>,
}

/// One generic impl, registered for `implements_trait`: an instance of
/// `template` implements the trait when every bound on the impl's parameters
/// holds for the instance's argument at that position.
#[derive(Clone)]
struct GenericImpl {
    template: String,
    /// The trait key each parameter must implement, one list per parameter.
    bounds: Vec<Vec<String>>,
    /// `type Item = I::Item`, as written, over the impl's parameters.
    assoc_types: Vec<(String, ast::TypeExpr)>,
    params: Vec<String>,
    /// The equality constraints (`I: Iterator<Item = i64>`) on each parameter,
    /// as `(assoc, type)`, one list per parameter (MISSING-Ve8Nc step 5).
    eqs: Vec<Vec<(String, ast::TypeExpr)>>,
    /// The `where I::Item: Bound` clauses, as `(parameter index, assoc, trait
    /// key)`.
    projections: Vec<(usize, String, String)>,
    /// The impl's module prefix and alias table, so a constraint's right-hand
    /// name resolves where the impl was written.
    prefix: String,
    aliases: HashMap<String, String>,
}

/// One blanket impl (`impl<T: Bound> Trait for T`), registered for
/// `implements_trait` and for method lookup: every type that meets `bounds`
/// implements `trait_key`, and its methods are templates keyed `impl#Trait#name`
/// (`types::blanket_receiver_name`).
#[derive(Clone)]
struct BlanketImpl {
    trait_key: String,
    trait_name: String,
    /// The trait keys the parameter must implement.
    bounds: Vec<String>,
    /// The method names the block writes, which is what lookup searches.
    methods: Vec<String>,
    module: String,
    impl_span: Span,
}

/// One built instantiation, on its way into the instance table.
enum Instance {
    Record(StructDef),
    Sum(EnumDef),
}

/// Concrete instantiations created on demand while checking (A-2), plus the
/// in-progress marks that turn a recursive instance (`Node<i32>` holding
/// `Box<Node<i32>>`) into a terminating lookup instead of infinite recursion.
#[derive(Default)]
struct InstanceState {
    records: HashMap<String, StructDef>,
    sums: HashMap<String, EnumDef>,
    /// Instance key -> the template it came from and the arguments it was
    /// built with, so an expected type can supply type parameters a
    /// construction cannot infer from its own arguments (`Option.None`).
    origin: HashMap<String, (String, Vec<Ty>)>,
    in_progress: std::collections::HashSet<String>,
}

/// The program's generic templates and the instances built so far. Shared by
/// reference between the `Ctx` clones each function gets, so an instantiation
/// discovered while checking one body is visible to every other - and to the
/// final `tir::Program`.
#[derive(Clone, Default)]
struct TypeEnv {
    generics: std::rc::Rc<HashMap<String, GenericTemplate>>,
    instances: std::rc::Rc<std::cell::RefCell<InstanceState>>,
    /// The user types that declare their own `Map` key pair (A-12). Decided
    /// once, from the whole function list, because a `Map<T, V>` in a
    /// *declaration* is resolved before the signatures are: the field of a
    /// `datac` that names such a type has to see the same key set a body
    /// does.
    declared_keys: std::rc::Rc<std::collections::HashSet<String>>,
    /// The user types that declare their own destructor (kernel stage 9,
    /// MK9-S6-M1): a `datac`/`enum` with `fun mut T.drop()`. Decided once, from
    /// the whole function list, for the same reason `declared_keys` is: a
    /// `datac` field or a `List<T>` element type is resolved before the
    /// signatures are, and the derived-copy fixpoints those resolutions ask
    /// must already know which types a destructor keeps out of them.
    declared_drops: std::rc::Rc<std::collections::HashSet<String>>,
    /// The canonical keys of the items each module exports (A-13). Private by
    /// default, so a key is here only for a declaration written `pub`, or for
    /// any declaration in a compiler-shipped module (the transitional rule
    /// `plan/syntax.md` 13.4 records). Shared through the same `Rc` every
    /// `Ctx` clone and every `TypeScope` see, since it is one answer for the
    /// whole program.
    exported: std::rc::Rc<std::collections::HashSet<String>>,
    /// The canonical names of the program's `extern type` handles (gap 1). One
    /// answer for the whole program, like `exported`: a handle declared in one
    /// module is named through an alias in another, and its name has to mean
    /// the same thing in every scope.
    handles: std::rc::Rc<std::collections::HashSet<String>>,
    /// Handle parent relation (gap 1): a canonical child name -> its canonical
    /// parent name, for the `extern type Name: Parent` declarations. One answer
    /// for the whole program, exactly as `handles` is, so the implicit upcast
    /// answers the same in every scope.
    handle_parents: std::rc::Rc<HashMap<String, String>>,
    /// The canonical keys of the program's variadic user `extern fun`s
    /// (gap 7) -> the module each was declared in. One answer for the whole
    /// program, so a call site knows both that a callee takes extra arguments
    /// and which module the generated wrapper belongs to.
    variadic_externs: std::rc::Rc<HashMap<String, String>>,
    /// The program's traits, keyed by qualified name (A-12 slice 2). One
    /// answer for the whole program: the same trait is implemented in one
    /// module and bounded in another.
    traits: std::rc::Rc<HashMap<String, TraitInfo>>,
    /// Trait key -> the qualified types that implement it. This is what a
    /// bound at a call site is checked against, and it is one set for the
    /// whole program because an impl may live in the trait's module while the
    /// call site is elsewhere. The key is the trait *instance* (A-12 slice 6),
    /// so `From<i32>` and `From<String>` are two entries.
    impls: std::rc::Rc<HashMap<String, std::collections::HashSet<String>>>,
    /// Every `impl` record, in source order (A-12 slice 6): a static method call
    /// searches this for the impl whose trait declares the method and whose
    /// substituted signatures match the arguments.
    trait_impls: std::rc::Rc<Vec<TraitImplInfo>>,
    /// The `(trait, type)` pairs a `&dyn` coercion used (A-12 slice 4),
    /// collected while bodies are checked. Each one gets a vtable in
    /// `tir::Program`, and a pair no coercion names gets none.
    dyn_pairs: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<(String, String)>>>,
    /// Every trait named by a `dyn` type, borrowed or owned (A-12 slices 4
    /// and 7), collected where the type is written. The fat-pair typedef and
    /// the vtable tag are emitted per *trait*, so a `dyn` type that is only
    /// named (a parameter nothing coerces to) still needs its C type to exist.
    dyn_traits: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
    /// The associated-type binding each `impl` declares (A-12 slice 5), keyed
    /// by `(trait, type)` and then by the associated type's name. One answer
    /// for the whole program, exactly as `impls` is, so a projection at a call
    /// site and the impl's own signature agree.
    assoc: std::rc::Rc<HashMap<(String, String), HashMap<String, Ty>>>,
    /// Generic types that are iterators by shape: the type template declares
    /// `fun mut Name<P, ...>.next().return: option.Option<Item>`, where `Item` is
    /// a written type over the template's own parameters. Keyed by template
    /// name. `std/iter`'s adapters are such types (`TakeIter<I, T>`), and this is
    /// what lets one satisfy `I: Iterator` and name `I::Item` without a generic
    /// trait impl (`plan/generic-trait-impls.md`).
    iter_nexts: std::rc::Rc<HashMap<String, IterNext>>,
    /// The generic impls (`impl<T: Bound> Trait for Type<T>`), by trait-instance
    /// key.
    generic_impls: std::rc::Rc<HashMap<String, Vec<GenericImpl>>>,
    /// The blanket impls (`impl<T: Bound> Trait for T`), in source order. Coherence
    /// (`check_trait_impl_overlap`) allows at most one per trait, and no other impl
    /// of that trait.
    blanket_impls: std::rc::Rc<Vec<BlanketImpl>>,
}

/// See `TypeEnv::iter_nexts`.
#[derive(Clone)]
struct IterNext {
    /// The template's type parameters, in order.
    params: Vec<String>,
    /// The `Item` of the `option.Option<Item>` `next` returns, as written.
    item: ast::TypeExpr,
}

/// One user-declared generic *function* (A-2): a `fun` with its own type
/// parameters (`fun first<T>(xs: List<T>).return: T`) or a receiver method on a
/// generic type (`fun Pair<A, B>.first().return: A`). Its parameter and return
/// types are kept as written, because a type parameter has no type until an
/// instantiation binds it - exactly like `GenericTemplate`.
#[derive(Clone)]
struct FuncTemplate {
    /// Declaration key: the prefixed name, or `Type#name` for a receiver.
    key: String,
    /// The method's type parameters, in order, which for a receiver method are
    /// the receiver's own arguments.
    params: Vec<String>,
    /// The declared type of each `const` parameter of `params` (A-20). See
    /// `GenericTemplate::const_params`.
    const_params: Vec<(String, Ty)>,
    /// `fun f<T: A + B>`: the traits each type parameter must implement, one
    /// list per entry of `params` and in the same order (A-12 slice 2). A
    /// parameter with no bound has an empty list, and a receiver method's
    /// parameters are its type's, so its lists are all empty.
    bounds: Vec<Vec<ast::TypeExpr>>,
    /// `where I::Item: Bound` (MISSING-Ve8Nc step 5): checked at each call on
    /// the concrete associated type.
    projection_bounds: Vec<ast::ProjectionBound>,
    /// `Some` for a receiver method: the canonical receiver type name (already
    /// prefixed) and whether the receiver is `mut`.
    receiver: Option<(String, bool)>,
    /// Declared parameters, without the receiver.
    declared: Vec<ast::Param>,
    ret: Option<ast::TypeExpr>,
    /// The declaring module's symbol prefix and alias table, so a parameter or
    /// return type written inside the template resolves where it was written.
    prefix: String,
    aliases: HashMap<String, String>,
    /// Index in `program.functions`, so a diagnostic can name the declaration.
    ast_index: usize,
}

/// The concrete signatures created on demand (A-2) and the queue of bodies
/// still to check. `sigs` is what a call site resolves against once the
/// declaration is instantiated; `pending` is the worklist `check_program`
/// drains, because one instantiation's body can create another.
#[derive(Default)]
struct FunctionState {
    sigs: HashMap<String, FnSig>,
    /// Instance key -> the template key it came from, for passdown wiring.
    origin: HashMap<String, String>,
    /// Instances whose bodies still have to be checked, in creation order
    /// alongside the type arguments they were built with.
    pending: std::collections::VecDeque<(String, Vec<Ty>)>,
    /// The synthesized `tir::Function` for each `fun` literal checked (A-4).
    /// A literal has no declaration in the AST, so `infer.rs` is what creates
    /// its function; the bodies are collected here and appended to the
    /// program's function list once checking is done. Shared through
    /// `FuncEnv`'s `Rc`, so a literal discovered inside another literal's body
    /// lands in the same table.
    literals: Vec<tir::Function>,
    /// How many literals have been synthesized, for the next one's name.
    literal_count: usize,
    /// The fixed-arity C wrappers one variadic user `extern fun` call site
    /// needs (gap 7), keyed by the wrapper's own name so two call sites that
    /// pass the same list of extra types share one. Each entry carries the
    /// extern's key so the pass that assembles the program can point the
    /// wrapper at the same native symbol the extern binds.
    variadic_wrappers: Vec<(String, tir::Function)>,
    variadic_wrappers_seen: std::collections::HashSet<String>,
}

/// The program's function templates and their instances (A-2), shared between
/// every `Ctx` clone for the same reason `TypeEnv` is.
#[derive(Clone, Default)]
struct FuncEnv {
    templates: std::rc::Rc<HashMap<String, FuncTemplate>>,
    instances: std::rc::Rc<std::cell::RefCell<FunctionState>>,
}

/// How many generic instantiations one program may create before the compiler
/// gives up. A generic function that calls itself with a different type
/// argument (`fun g<T>(x: T).return: i32 = g(x = List.of(x))`) grows without bound;
/// this turns that into a diagnostic instead of a hang.
const MAX_FUNCTION_INSTANTIATIONS: usize = 2048;

/// The name prefix of the local a `when` stages its scrutinee into when the
/// scrutinee is a temporary. `#` cannot appear in a lexed identifier, so a
/// synthesized name can never collide with one the user wrote: the same trick
/// `parser::IF_LET_PREFIX` and a hidden passdown parameter use.
const WHEN_TEMP_PREFIX: &str = "#when";
const BORROW_TEMP_PREFIX: &str = "#borrow";

/// The name prefix of the local a destructuring binding takes its value apart
/// from, and of the local a *nested* pattern takes an inner tuple into
/// (`plan/syntax.md` 4). Same `#` trick as `WHEN_TEMP_PREFIX`: a synthesized
/// name can never collide with one the user wrote.
const TUPLE_TEMP_PREFIX: &str = "#tuple";

/// The type parameters a function declaration introduces: its own for a free
/// function, the receiver's for a method on a generic type.
fn decl_type_params(f: &ast::FunctionDecl) -> Vec<String> {
    match &f.receiver {
        // The type's own parameters first, then the method's (`fun
        // Pair<A, B>.map_first<C>`).
        Some(r) => r.type_args.iter().chain(f.type_params.iter()).cloned().collect(),
        None => f.type_params.clone(),
    }
}

/// The declared type of each `const` parameter of a declaration, validated
/// (A-20). A const parameter's type is one of the integer types a value can be
/// judged against at compile time, or `bool`; anything else is refused where
/// it is written.
fn const_param_tys(
    decls: &[ast::ConstParamDecl],
    module: &str,
) -> std::result::Result<Vec<(String, Ty)>, Diagnostic> {
    let mut out = Vec::with_capacity(decls.len());
    for d in decls {
        let ty = match d.ty.as_str() {
            "usize" => Ty::Usize,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "bool" => Ty::Bool,
            "char" => Ty::Char,
            other => {
                return Err(Diagnostic::new(
                    format!(
                        "'const {}' has type '{other}': a const parameter's type is one of 'usize', 'u8', 'u16', 'u32', 'u64', 'i32', 'i64', 'bool' or 'char'",
                        d.name
                    ),
                    d.span,
                    module.to_string(),
                ))
            }
        };
        out.push((d.name.clone(), ty));
    }
    Ok(out)
}

/// Checks each const argument against its parameter's declared type and
/// re-types it, and refuses a const argument in a type parameter's position
/// (A-20). `args` is positional against `params`, so the two must be the same
/// length.
fn bind_const_args(
    params: &[String],
    const_params: &[(String, Ty)],
    args: &mut [Ty],
    span: Span,
) -> Result<()> {
    for (name, declared) in const_params {
        let Some(idx) = params.iter().position(|p| p == name) else {
            continue;
        };
        let Some(arg) = args.get(idx) else { continue };
        let Ty::ConstInt { value, ty: arg_ty } = arg else {
            return Err(TypeError {
                message: format!(
                    "const parameter '{name}' expects a constant expression of type '{}', but a type was written: write a literal or a 'const' ('Buf<16>', 'Buf<SIZE>'), not a type name",
                    declared.display()
                ),
                span,
            });
        };
        let value = *value;
        // A `char` parameter takes a character and nothing else, and no
        // other parameter takes one (A-20, MISSING-Ey4Mt).
        let arg_is_char = matches!(**arg_ty, Ty::Char);
        let wants_char = matches!(declared, Ty::Char);
        if wants_char && !arg_is_char {
            return Err(TypeError {
                message: format!(
                    "const parameter '{name}' is a 'char', so its argument has to be a character literal or a 'char' const ('Tag<'x'>'), not a number"
                ),
                span,
            });
        }
        if !wants_char && arg_is_char {
            return Err(TypeError {
                message: format!(
                    "const parameter '{name}' is a '{}', so its argument has to be an integer or 'bool' value, and a character is not one",
                    declared.display()
                ),
                span,
            });
        }
        if matches!(declared, Ty::Bool) {
            if value != 0 && value != 1 {
                return Err(TypeError {
                    message: format!(
                        "const parameter '{name}' is a 'bool', so its argument has to be 'true' or 'false'"
                    ),
                    span,
                });
            }
        } else if wants_char {
            if u32::try_from(value).ok().and_then(char::from_u32).is_none() {
                return Err(TypeError {
                    message: format!(
                        "const parameter '{name}' is a 'char', and U+{value:04X} is not a Unicode scalar value"
                    ),
                    span,
                });
            }
        } else {
            check_int_literal(value, declared, false, span)?;
        }
        args[idx] = Ty::ConstInt {
            value,
            ty: Box::new(declared.clone()),
        };
    }
    for (idx, name) in params.iter().enumerate() {
        if const_params.iter().any(|(n, _)| n == name) {
            continue;
        }
        if let Some(Ty::ConstInt { .. }) = args.get(idx) {
            return Err(TypeError {
                message: format!(
                    "type parameter '{name}' expects a type, but a constant expression was written: a 'const' parameter is declared 'const {name}: usize' where the value belongs"
                ),
                span,
            });
        }
    }
    Ok(())
}

/// The name of the first `const` type parameter a written length expression
/// mentions (A-20), if any. Used to refuse type-level arithmetic on a const
/// parameter by name rather than folding it.
fn const_param_in(
    expr: &ast::Expr,
    params: &HashMap<String, Ty>,
) -> Option<String> {
    match expr {
        ast::Expr::Ident(name, _) => match params.get(name) {
            Some(Ty::ConstInt { .. }) => Some(name.clone()),
            _ => None,
        },
        ast::Expr::Unary { expr, .. } => const_param_in(expr, params),
        ast::Expr::Binary { lhs, rhs, .. } => {
            const_param_in(lhs, params).or_else(|| const_param_in(rhs, params))
        }
        ast::Expr::Call { args, .. } => {
            args.iter().find_map(|a| const_param_in(&a.value, params))
        }
        ast::Expr::FieldAccess { base, .. } => const_param_in(base, params),
        _ => None,
    }
}

/// The constant expression a written type argument stands for when it is a
/// bare or dotted name (`Buf<SIZE>`, `Buf<math.PAGE>`, A-20): the parser kept
/// those as a type, and a `const` parameter position reconstructs the
/// expression the name stands for. `None` for any other shape.
fn named_const_expr(ty: &ast::TypeExpr) -> Option<ast::Expr> {
    if ty.name.is_empty()
        || !ty.args.is_empty()
        || ty.array_elem.is_some()
        || ty.tuple.is_some()
        || ty.fn_params.is_some()
        || ty.dyn_trait.is_some()
        || ty.impl_trait.is_some()
        || ty.assoc.is_some()
    {
        return None;
    }
    Some(match ty.name.split_once('.') {
        Some((alias, field)) if !field.contains('.') => ast::Expr::FieldAccess {
            base: Box::new(ast::Expr::Ident(alias.to_string(), ty.span)),
            field: field.to_string(),
            span: ty.span,
        },
        _ => ast::Expr::Ident(ty.name.clone(), ty.span),
    })
}

/// The canonical name of one instantiation: `Pair<i32, String>`. It is a
/// `Ty::Struct` key like any other declaration's name, and `types::c_ident`
/// mangles it into a C identifier, so a type argument list can never be
/// written by hand in a position that would collide.
fn instance_key(name: &str, args: &[Ty]) -> String {
    let parts: Vec<String> = args.iter().map(|a| a.display()).collect();
    format!("{}<{}>", name, parts.join(", "))
}

/// The type of a tuple, and the one place its derived record is registered
/// (MISSING-z2bB9).
///
/// A tuple is a record with positional fields (`plan/syntax.md` 3) and v1's
/// record type is `Ty::Struct(<key>)`, keyed by the tuple as written. The
/// record is *derived*: `types::tuple_struct_def` builds it and it goes into
/// the same on-demand registry a generic instantiation uses (A-2), so every
/// later pass treats it as an ordinary declaration: the derived deep copy,
/// order and drop, the partial move of one position, and both backends'
/// struct emission all read the declaration table and need no tuple case of
/// their own. Idempotent: two use sites of one element list are one type.
fn tuple_ty(env: &TypeEnv, elems: Vec<Ty>) -> Ty {
    let name = crate::types::tuple_struct_name(&elems);
    let mut state = env.instances.borrow_mut();
    if !state.records.contains_key(&name) {
        let def = crate::types::tuple_struct_def(&name, &elems);
        state.records.insert(name.clone(), def);
    }
    drop(state);
    Ty::Struct(name)
}

/// The element types of a tuple type, from the type itself: `Some` for a
/// record whose name is a tuple's written form, with the field types of each
/// position in order. `None` for every declared `datac`/`enum`.
///
/// A borrow of a tuple is the same tuple here: `val (lo, hi) = xs.get(i = 0)`
/// reads the positions out of the element the borrow points at, and an owned
/// element stays refused by the partial-move rule exactly as reading one field
/// of a borrowed `datac` is.
fn tuple_elems_of(ty: &Ty, ctx: &Ctx) -> Option<Vec<Ty>> {
    let pointee = match ty {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    let Ty::Struct(name) = pointee else {
        return None;
    };
    if !crate::types::is_tuple_name(name) {
        return None;
    }
    Some(
        ctx.struct_def(name)?
            .fields
            .iter()
            .map(|f| f.ty.clone())
            .collect(),
    )
}

/// Cloned once per function body checked, and once more per `fun` literal
/// (A-4), whose body is checked in a scope of its own with the same module
/// tables. The two `Rc` fields are what make a clone share state: a generic
/// instantiation (or a literal's synthesized function) created while checking
/// one clone is visible to every other.
#[derive(Clone)]
struct Ctx {
    sigs: HashMap<String, FnSig>,
    structs: HashMap<String, StructDef>,
    /// The `enum` table (A-1), keyed exactly like `structs`. An enum name is
    /// also present in `structs` (as a definition with no fields) purely so
    /// that one name-resolution path covers both kinds of user type; this
    /// table is what construction, pattern matching and the `has no fields`
    /// diagnostics consult.
    enums: HashMap<String, EnumDef>,
    /// Top-level `const`s (M6.3), keyed by their module-qualified name and
    /// already folded to a literal - so a use site is a substitution, not a
    /// load from anywhere.
    consts: HashMap<String, ConstInfo>,
    /// Top-level `static`s (kernel stage 7), keyed by their module-qualified
    /// name. A static is a real object, so a use site is a `StaticRef` place
    /// rather than a substitution, and the type and mutability travel with it.
    statics: HashMap<String, StaticInfo>,
    /// The functions a compile-time fold may call (A-19). Shared rather than
    /// cloned, because a `Ctx` is cloned once per body checked and this table
    /// is the whole program's functions.
    const_fns: std::rc::Rc<ConstFns>,
    /// Which user types own heap memory, transitively (`types::owned_types`).
    /// A `[T; N]` repeat literal (`[Gate(...); 256]`) copies its value into
    /// every element, so it needs the same answer the drop planner does: does
    /// this type own anything (A-18)?
    owned_structs: std::collections::HashSet<String>,
    /// Overload sets for compiler-shipped stdlib functions (M6.3): plain key
    /// (`std.math.abs`) -> each declaration's parameter types and its
    /// type-suffixed key.
    overloads: HashMap<String, Vec<(Vec<Ty>, String)>>,
    /// The module being checked - needed by capability diagnostics, which
    /// have to name the module that lacks the grant.
    module: String,
    /// Capability domains *this* module holds, with their patterns (M7). The
    /// gate below is what makes an ungranted `fs.read` a resolution error
    /// rather than a runtime check that happens to reject it.
    caps: HashMap<String, Vec<String>>,
    /// Function key -> gated domain, for the modules that gate one.
    gated: HashMap<String, String>,
    /// Whether this module's `coral.toml` carries an `[extern."<module id>"]`
    /// section. A handle operation (`null()`, `is_null()`, `cast()`) is legal
    /// only where one exists, even when the handle type is imported from an
    /// extern module.
    has_extern_section: bool,
    /// The program's variadic user `extern fun`s (gap 7): canonical key ->
    /// declaring module. A call site reads it to know a callee takes extra
    /// arguments and which wrapper to name.
    variadic_externs: std::rc::Rc<HashMap<String, String>>,
    ret_ty: Ty,
    scopes: Vec<HashMap<String, VarInfo>>,
    /// Symbol prefix of the module being checked (`""` for the entry
    /// module, else `"shapes.circle."`) - a bare type name in this module
    /// resolves to `prefix + name`.
    prefix: String,
    /// alias -> target module's symbol prefix, for this module's `impl`
    /// lines (M5). This is also how a module-qualified *call* is
    /// recognized: `circle.make(...)` is a module item reference, not a
    /// method call, when `circle` is one of these aliases.
    aliases: HashMap<String, String>,
    /// This module's passdowns, keyed by the bare name a function writes.
    /// `plan/syntax.md` §9 leaves the cross-module question open; M5.2
    /// answers it the same way M5.1 answered imports - module-scoped, so
    /// only functions in the declaring module can name it.
    pds: HashMap<String, PdInfo>,
    /// The program's generic type templates and the instances built so far
    /// (A-2). `structs`/`enums` hold the hand-written declarations; an
    /// instantiation lives in `env.instances` and is reached through
    /// `struct_def`/`enum_def`.
    env: TypeEnv,
    /// The program's generic function templates and their instantiated
    /// signatures (A-2). `sigs` holds the hand-written, non-generic
    /// declarations; an instantiated one lives in `fns.instances` and is
    /// reached through `sig_of`.
    fns: FuncEnv,
    /// The type parameters in scope while a generic function's body is being
    /// checked, bound to the instantiation's arguments. Empty everywhere
    /// else, which is what `type_scope` reads.
    type_args: HashMap<String, Ty>,
    /// Passdowns of *other* modules, bare name -> declaring module. Used for
    /// diagnostics: naming one is an error, and the message says why.
    foreign_pds: HashMap<String, String>,
    /// sig key -> passdowns that callee references, in declaration order:
    /// what every call site has to pass as hidden arguments.
    wired: HashMap<String, Vec<PdInfo>>,
    /// While a `pd` initializer is being checked: the hidden names already
    /// initialized. `None` inside a function body, where every passdown is
    /// available. This is what stops an initializer from reading a
    /// passdown that is still zero-initialized at that point.
    init_state: Option<std::collections::HashSet<String>>,
    /// True only while checking `main`'s body. `main` is emitted with a
    /// fixed C signature - it cannot receive hidden parameters - so its
    /// passdown references take the address of the generated global instead
    /// of naming a parameter. Every other function that references a
    /// passdown is wired and uses its parameter.
    pd_global_borrow: bool,
    /// The hidden passdown parameters *this* function received, by their
    /// internal name (`pd#<qualified>`). The generated parameter and the
    /// generated global share that name, so an identifier in the body means
    /// the parameter when it is listed here and the file-scope global when it
    /// is not - a distinction `hidden_args` has to make when it hands a
    /// passdown on: a parameter is already the borrow, while the global needs
    /// its address taken. Empty for `main`, a `pd` initializer, and a `fun`
    /// literal, none of which can receive one.
    own_hidden: Vec<String>,
    /// The wiring key this body is checked under - `main`, a declaration key,
    /// or the template an instantiation came from. The borrow modes and the
    /// probe's collected uses are both filed under it.
    fn_key: String,
    /// The passdown borrow modes this body check runs under (roadmap item
    /// 17). Empty in the probe pass, where every passdown is shared and a
    /// write is recorded instead of refused; `pd_global_borrow` bodies
    /// (`main`, a `pd` initializer) are filed under their own key, so a
    /// `main` that only reads a `pd var` takes a shared borrow of it too.
    pd_modes: std::rc::Rc<HashMap<(String, String), bool>>,
    /// Set only in the probe pass: where an exclusive use is recorded and the
    /// call edges are collected (see `PdProbe`).
    pd_probe: Option<std::rc::Rc<std::cell::RefCell<PdProbe>>>,
    /// True while checking the body of a `fun` literal (A-4). A literal is
    /// its own function, so two diagnostics need to know: a name it does not
    /// bind itself is a *capture*, and a passdown is a hidden argument it
    /// cannot receive.
    in_fn_literal: bool,
    /// The *types* of the bindings that were in scope where the literal
    /// currently being checked was written (A-4 phase E), plus whatever the
    /// enclosing literal had already captured. A name the literal's own body
    /// does not bind that appears here is a capture; a name that appears
    /// nowhere is a typo or a missing `impl`, and the two read differently.
    outer_bindings: HashMap<String, Ty>,
    /// The captures this literal has collected so far, in the order its body
    /// first named them (A-4 phase E). The body check fills this in, and
    /// `check_fn_literal` reads it afterwards to decide whether the literal
    /// is a plain function value or a closure, and what its environment
    /// holds. Empty outside a literal body, which is also how a nested
    /// literal finds the *enclosing* literal's captures (indices into the
    /// enclosing environment).
    captures: Vec<(String, Ty)>,
    /// True while checking a *scoped* literal (`plan/scoped-blocks.md`),
    /// whose environment is stack and whose captures may be borrows: an
    /// owned non-`Copy` enclosing binding is captured as a shared borrow
    /// rather than moved, so `note_capture` records `&T` and the body reads
    /// the capture through it.
    borrowed_captures: bool,
    /// The enclosing owned `var` bindings a scoped literal may capture by
    /// *exclusive* borrow (`plan/exclusive-captures.md`): names that are
    /// mutable, owned and not `Copy`.
    outer_mut: std::collections::HashSet<String>,
    /// This pass's upgrades: the captures `note_capture` types `&mut T`
    /// instead of `&T`. Grown by `check_fn_literal`'s retry loop.
    excl_captures: std::collections::HashSet<String>,
    /// The names an *enclosing* scoped literal already captures exclusively:
    /// a block inside that block cannot hold them (v1, `plan/exclusive-captures.md`).
    outer_excl: std::collections::HashSet<String>,
    /// The captures this pass wrote through (or passed as `mut`) while typed
    /// as a shared borrow: what the retry loop upgrades. One cell per pass.
    excl_needed: std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>>,
    /// The `datac` whose body the literal was written in, when the literal is
    /// inside a receiver-form method (A-4 phase E). Its fields are names the
    /// literal can *see* but cannot capture: reading one out of a borrowed
    /// receiver is a partial move (A-11), so the diagnostic says that instead
    /// of reporting an undefined name.
    outer_receiver: Option<String>,
    /// Set while checking a receiver-form function whose receiver resolves
    /// to a `datac`: lets a bare identifier that isn't a local fall back to
    /// an implicit `self.field` (`fun Circle.area().return: f64 = 3.14159 * r *
    /// r` - no `self.` qualifier written, per `plan/syntax.md` §5's "no
    /// `self` keyword").
    receiver_struct: Option<String>,
    /// How many scrutinee temporaries a `when` has staged, so each one's
    /// synthesized binding has a name of its own (`infer::WHEN_TEMP_PREFIX`).
    when_temps: usize,
    /// How many `for` loops this body has desugared, so each loop's
    /// synthesized sequence and index names are unique (`plan/iterators.md`).
    /// A `fun` literal's body starts a fresh count, exactly as a function
    /// does.
    for_temps: usize,
    /// How many loops enclose the statement being checked. A `break` or
    /// `continue` outside every loop is refused here rather than reaching a
    /// backend that has no loop to branch out of.
    loop_depth: usize,
    /// The same counter for a destructuring binding's value and its nested
    /// levels (`infer::TUPLE_TEMP_PREFIX`).
    tuple_temps: usize,
    /// Set only while the initializer of a `let` with no written type is being
    /// checked. A bare `List.empty()`/`Box.empty()`/`Map.new()` there leaves
    /// its element type open instead of failing at the declaration, for the
    /// first `push`/`insert` on the binding to decide (: the marker and the
    /// repair are `types::is_open_container` and `check_block`). Anywhere
    /// else an empty container with no type to take its element from is a
    /// declaration-time error, exactly as it always was.
    defer_empty_container: bool,
    /// Expressions already checked once with no (or a hint-only) expectation to
    /// learn a type (a method call's receiver, a generic call's arguments),
    /// kept for the `check_call_arg` that follows so they are not checked a
    /// second time: nested calls would otherwise double their work at every
    /// level. Keyed by the expression's address and span; an entry is used
    /// only when its type is exactly the one the argument is then expected to
    /// have, and is dropped when used.
    probes: Vec<(*const ast::Expr, Span, tir::Expr)>,
    /// How many `unsafe { ... }` blocks enclose the expression being checked
    /// (MK9-S2 stage 1). Non-zero means the unsafe-only operations are legal;
    /// the body of an `unsafe fun` starts at 1.
    unsafe_depth: usize,
    /// The functions declared `unsafe fun`, by canonical key. Calling one
    /// requires `unsafe_depth > 0`.
    unsafe_fns: std::collections::HashSet<String>,
    /// The functions declared `must_use fun`, by canonical key: a statement
    /// that is a call to one, with its result unread, is refused.
    must_use_fns: std::collections::HashSet<String>,
    /// Whether the project is a freestanding build (kernel stage 8). The
    /// `percpu` storage class and its two intrinsics exist only there.
    freestanding: bool,
    /// The manifest's `[target] cpus` (kernel stage 8), which is the value
    /// `cpu_count()` folds to and the number of copies a `percpu` static has.
    cpus: u64,
    /// Set only while checking the block argument of `Mutex.lock` (B-5): a
    /// scoped block's return type is the block's own tail type, so
    /// `check_fn_literal` infers it instead of taking it from the expected
    /// `scoped fun(...).return: R`. Cleared for the body of the literal itself, so
    /// a nested scoped block is typed by its own callee again.
    infer_scoped_ret: bool,
}

/// One checked `static` (kernel stage 7).
#[derive(Debug, Clone)]
struct StaticInfo {
    /// The module-prefixed name, which is the emitted symbol.
    qualified: String,
    /// The bare name a function in the declaring module writes.
    name: String,
    ty: Ty,
    /// `static mut NAME` - every access is `unsafe`.
    mutable: bool,
}

/// One checked passdown (M5.2).
#[derive(Debug, Clone)]
struct PdInfo {
    /// Module-prefixed name, for diagnostics.
    qualified: String,
    /// The bare name a function in the declaring module writes.
    name: String,
    /// Internal name shared by the generated C global and the hidden
    /// parameter a referencing function receives. `#` can't appear in a
    /// user identifier, so `pd#x` can never collide with a local of the
    /// same *written* name (which C would otherwise reject as a duplicate
    /// declaration in the function's own scope).
    hidden: String,
    /// The passdown's own type.
    value_ty: Ty,
    /// Whether it was declared `pd var` - used for the wording of the
    /// "read-only inside a function" diagnostic, and to tell a write that
    /// can be given an exclusive borrow from one that can never be allowed.
    mutable: bool,
    module: String,
    /// Global initialization order (position in the merged program), used to
    /// keep hidden-parameter order and the generated init function stable.
    index: usize,
}

impl PdInfo {
    /// The *borrow* of the passdown's storage: what a call site passes for
    /// this passdown's hidden parameter, and what a read dereferences.
    ///
    /// `exclusive` is the referencing function's mode (roadmap item 17): a
    /// function that *writes* the passdown - directly or through a callee -
    /// holds it as an exclusive borrow, and one that only reads it holds a
    /// shared one. A `pd var` that no function writes is therefore a shared
    /// borrow everywhere, so `show(cfg)` does not collide with the
    /// passdown's own hidden argument.
    fn borrow_ty(&self, exclusive: bool) -> Ty {
        Ty::Ref {
            mutable: exclusive,
            inner: Box::new(self.value_ty.clone()),
        }
    }

    /// Two shapes, because the two contexts reach the storage differently:
    /// inside a function the hidden name is already the borrow (`&*hidden`
    /// would be a pointer to a pointer), while `main` and a `pd` initializer
    /// have no hidden parameter and name the generated global directly, so
    /// the borrow is explicit.
    fn arg(&self, span: Span, global: bool, exclusive: bool) -> tir::Expr {
        let ty = self.borrow_ty(exclusive);
        if global {
            let global = tir::Expr {
                kind: tir::ExprKind::Ident(self.hidden.clone()),
                ty: self.value_ty.clone(),
                span,
            };
            return tir::Expr {
                kind: tir::ExprKind::Ref {
                    mutable: exclusive,
                    expr: Box::new(global),
                },
                ty,
                span,
            };
        }
        tir::Expr {
            kind: tir::ExprKind::Ident(self.hidden.clone()),
            ty,
            span,
        }
    }

    /// A read of the passdown (M6.4): `*` of the borrow, so reads and writes
    /// go through the same pointer, and the borrow's mutability is what
    /// decides whether a write through it is allowed.
    fn value(&self, span: Span, global: bool, exclusive: bool) -> tir::Expr {
        tir::Expr {
            kind: tir::ExprKind::Deref {
                expr: Box::new(self.arg(span, global, exclusive)),
            },
            ty: self.value_ty.clone(),
            span,
        }
    }
}

/// What one pass of body checking discovers about passdown writes, so the
/// next pass can hand the passdowns over with the right borrow (roadmap item
/// 17). The probe pass runs with every passdown shared, records the uses that
/// need an exclusive borrow instead of refusing them, and records the call
/// edges; `close_pd_modes` then walks the edges to a fixpoint, and the pass
/// that emits the program runs with the result.
#[derive(Debug, Default)]
struct PdProbe {
    /// `(caller's wiring key, hidden name)` - the caller uses the passdown
    /// exclusively, so it must hold an exclusive borrow of it.
    exclusive_uses: std::collections::HashSet<(String, String)>,
    /// `(caller's wiring key, callee's wiring key)` - the call graph the
    /// closure walks: a function that calls a writer holds the passdown
    /// exclusively too, or it could hand a shared borrow to a callee that
    /// writes through it.
    edges: std::collections::HashSet<(String, String)>,
}

/// The passdown borrow modes a body check runs under.
#[derive(Clone, Default)]
struct PdWiring {
    /// `(wiring key, hidden name) -> the function writes the passdown`. A
    /// missing entry means the shared borrow, which is the common case.
    modes: std::rc::Rc<HashMap<(String, String), bool>>,
    /// `Some` only in the probe pass (see `PdProbe`).
    probe: Option<std::rc::Rc<std::cell::RefCell<PdProbe>>>,
}

impl PdWiring {
    /// Whether the function filed under `key` holds `pd` exclusively.
    fn exclusive(&self, key: &str, pd: &PdInfo) -> bool {
        self.modes
            .get(&(key.to_string(), pd.hidden.clone()))
            .copied()
            .unwrap_or(false)
    }
}

impl Ctx {
    /// Remembers `done`, the result of checking `expr`, for the `check_call_arg`
    /// that will check the same expression (see `probes`).
    fn keep_probe(&mut self, expr: &ast::Expr, done: tir::Expr) {
        if self.probes.len() >= 4096 {
            self.probes.clear();
        }
        self.probes.push((expr as *const ast::Expr, expr.span(), done));
    }

    fn take_probe(&mut self, expr: &ast::Expr) -> Option<tir::Expr> {
        let at = expr as *const ast::Expr;
        let span = expr.span();
        let i = self.probes.iter().rposition(|(p, s, _)| *p == at && *s == span)?;
        Some(self.probes.swap_remove(i).2)
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Ty, mutable: bool) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), VarInfo { ty, mutable });
    }

    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    /// Resolves a written type name to a canonical `Ty::Handle` name, or
    /// `None` when it is not a handle: `Buffer` in the module that declares it,
    /// `mem.Buffer` through an alias anywhere else. This is the same
    /// qualification `TypeScope::resolve_base` gives a handle in a signature.
    fn handle_type_named(&self, name: &str) -> Option<String> {
        let qualified = qualify_written(name, &self.prefix, &self.aliases);
        self.env.handles.contains(&qualified).then_some(qualified)
    }

    /// The gate on `null()`/`is_null()`/`cast()` (gap 1): a handle operation is
    /// legal only in a module with its own `[extern."<module id>"]` section,
    /// even when the handle type itself is imported.
    fn require_extern_section(&self, what: &str, span: Span) -> Result<()> {
        if self.has_extern_section {
            return Ok(());
        }
        Err(TypeError {
            message: format!(
                "'{what}' is only legal in a module with a coral.toml '[extern.\"<module id>\"]' section, because only there can a native handle exist"
            ),
            span,
        })
    }

    /// Rewrites a binding's type in the scope that holds it. Used for one
    /// thing only: an open container whose element type an operation decided
    /// after the binding was declared, so every later statement (and the
    /// emitted local) sees the real type rather than the marker.
    fn set_binding_ty(&mut self, name: &str, ty: Ty) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.ty = ty;
                return;
            }
        }
    }

    /// Looks up `name` as a field of the current receiver's `datac`, if
    /// this function has one.
    fn receiver_field_ty(&self, name: &str) -> Option<Ty> {
        let sname = self.receiver_struct.as_deref()?;
        self.struct_def(sname)?.field(name).map(|f| f.ty.clone())
    }

    /// Whether `name` is a field of the named `datac` - used only to tell a
    /// literal's diagnostic "this is a field of the receiver you are in, and
    /// reading it by value is a partial move" apart from "undefined name"
    /// (A-4 phase E).
    fn field_of(&self, owner: &str, name: &str) -> Option<Ty> {
        self.struct_def(owner)?.field(name).map(|f| f.ty.clone())
    }

    /// Records `name` as a capture of the `fun` literal being checked and
    /// returns its type, or `None` when the name is not an enclosing binding
    /// (A-4 phase E). One per distinct name, in the order the body first
    /// names them, because that order is the environment's field order: the
    /// body's `ExprKind::Capture` indices and the environment `codegen_c.rs`
    /// writes have to agree.
    fn note_capture(&mut self, name: &str) -> Option<Ty> {
        let ty = self.outer_bindings.get(name).cloned()?;
        // A scoped literal's environment is stack and owns nothing, so an
        // owned non-`Copy` capture is a *shared borrow* of the enclosing
        // binding rather than a move of it (`plan/scoped-blocks.md`). The
        // body then reads `&T`, which is what makes a field read and a method
        // call through the capture type-check, and what makes a write through
        // it a borrow error.
        let ty = if self.borrowed_captures
            && (!ty.is_copy() || self.excl_captures.contains(name))
            && !crate::types::is_borrowed_field_ty(&ty)
        {
            Ty::Ref {
                mutable: self.excl_captures.contains(name),
                inner: Box::new(ty),
            }
        } else {
            ty
        };
        if !self.captures.iter().any(|(n, _)| n == name) {
            self.captures.push((name.to_string(), ty.clone()));
        }
        Some(ty)
    }

    /// The visibility gate (A-13): an item another module reaches through an
    /// alias is reachable only when its declaration is `pub` there. `key` is
    /// the canonical lookup key, and `alias.item` is how the reference is
    /// written. Called only after the item is known to exist, so the refusal
    /// always means "private" and never "missing".
    fn check_exported(&self, key: &str, alias: &str, item: &str, span: Span) -> Result<()> {
        if exported(key, &self.env) {
            return Ok(());
        }
        Err(private_item_error(alias, item, span))
    }

    /// The visibility gate for a type path an expression names through an
    /// alias (`shapes.Shape.Empty`, `shapes.Shape.Circle(...)`). A local,
    /// parameter or passdown of the alias's own name shadows it, exactly as
    /// it shadows any other name lookup, so `a.b` stays a field of the value
    /// `a` and is not read as an alias at all.
    fn check_type_path_exported(&self, path: &str, span: Span) -> Result<()> {
        if let Some((alias, _)) = path.split_once('.') {
            if self.lookup(alias).is_some() || self.pds.contains_key(alias) {
                return Ok(());
            }
        }
        check_type_exported(
            path,
            &self.structs,
            &self.enums,
            &self.aliases,
            &self.env,
            span,
        )
    }

    /// The capability gate (M7): a gated module's item is reachable only
    /// from a module that holds the domain. This is a *resolution* error, not
    /// a runtime check - the point of the feature is that code without a
    /// grant has no name to reach the resource with.
    fn check_capability(&self, key: &str, span: Span) -> Result<()> {
        let Some(domain) = self.gated.get(key) else {
            return Ok(());
        };
        if self.caps.contains_key(domain) {
            return Ok(());
        }
        // A module's own items are reachable from inside it with no grant:
        // the gate exists to stop *other* modules from naming the resource,
        // and a gated stdlib module has to be able to call its own helpers
        // (`process.run` is `spawn` plus a drain loop, and `std/net` will
        // want the same shape). Nothing outside the compiler can declare a
        // gated item - `stdlib::domain_of` only knows stdlib ids - so this
        // exemption cannot widen a user module's authority.
        if !self.prefix.is_empty() && key.starts_with(self.prefix.as_str()) {
            return Ok(());
        }
        Err(TypeError {
            message: format!(
                "'{}' needs the '{}' capability, which module '{}' was not granted; the module that imports this one must delegate it (`impl \"<this module>\" caps({} = [\"<pattern>\"])`)",
                display_callee(key),
                domain,
                self.module,
                domain
            ),
            span,
        })
    }

    /// `resolve.rs` stores a non-entry module's top-level names with their
    /// module prefix, but the plan's rule is that a file's own items are
    /// reachable *bare* inside that file - so a bare lookup has to try the
    /// prefixed spelling too. The entry module's prefix is empty, so this is
    /// the identity there.
    fn qualify_own(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}{name}", self.prefix)
        }
    }

    fn type_scope(&self) -> TypeScope<'_> {
        TypeScope {
            structs: &self.structs,
            enums: &self.enums,
            env: self.env.clone(),
            params: &self.type_args,
            prefix: &self.prefix,
            aliases: &self.aliases,
            // Every datac is known by the time bodies are checked.
            later_datacs: None,
            in_progress: None,
            consts: Some(&self.consts),
            const_fns: Some(&self.const_fns),
            handles: &self.env.handles,
        }
    }

    /// A function's checked signature, whether it was declared by hand or is
    /// an instantiation of a generic template (A-2).
    fn sig_of(&self, key: &str) -> Option<FnSig> {
        if let Some(s) = self.sigs.get(key) {
            return Some(s.clone());
        }
        self.fns.instances.borrow().sigs.get(key).cloned()
    }

    /// The passdowns a callee needs, looking through an instantiation to the
    /// template it came from: the wiring scan runs on declarations, so its
    /// table is keyed by the declaration, not by each instance.
    fn wired_for(&self, key: &str) -> Option<Vec<PdInfo>> {
        if let Some(w) = self.wired.get(key) {
            return Some(w.clone());
        }
        let tmpl = self.fns.instances.borrow().origin.get(key).cloned()?;
        self.wired.get(&tmpl).cloned()
    }

    /// The key the passdown wiring table is filed under: an instantiation's
    /// wiring is its template's (A-2), and so is its borrow mode - every use
    /// site of one instance is `wired_for`-ed through the same entry.
    fn wiring_key_of(&self, key: &str) -> String {
        if self.wired.contains_key(key) {
            return key.to_string();
        }
        self.fns
            .instances
            .borrow()
            .origin
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Whether the body filed under `key` holds `pd` exclusively (roadmap
    /// item 17).
    fn pd_exclusive_for(&self, key: &str, pd: &PdInfo) -> bool {
        self.pd_modes
            .get(&(key.to_string(), pd.hidden.clone()))
            .copied()
            .unwrap_or(false)
    }

    /// Whether this body holds `pd` exclusively: it writes it, or it calls a
    /// function that does (roadmap item 17). Everything else is a shared
    /// borrow, so a read-only reference never blocks another one.
    fn pd_exclusive(&self, pd: &PdInfo) -> bool {
        self.pd_exclusive_for(&self.fn_key, pd)
    }

    /// A use that needs an exclusive borrow. In the probe pass it is what the
    /// next pass is computed from, so it is recorded and allowed; in the pass
    /// that emits the program the modes already say the answer, and reaching
    /// here means the closure missed the use - say so instead of emitting an
    /// unchecked write.
    fn note_pd_exclusive_use(&self, pd: &PdInfo) -> bool {
        let Some(probe) = &self.pd_probe else {
            return false;
        };
        probe
            .borrow_mut()
            .exclusive_uses
            .insert((self.fn_key.clone(), pd.hidden.clone()));
        true
    }

    /// The definition of a user type, whether it was declared by hand or is
    /// an instantiation of a generic template (A-2). Every lookup that used
    /// to read `self.structs` goes through here, so an instance key
    /// (`Pair<i32, String>`) behaves exactly like a declared name.
    fn struct_def(&self, name: &str) -> Option<StructDef> {
        if let Some(s) = self.structs.get(name) {
            return Some(s.clone());
        }
        self.env.instances.borrow().records.get(name).cloned()
    }

    /// As `struct_def`, for the `enum` table.
    fn enum_def(&self, name: &str) -> Option<EnumDef> {
        if let Some(e) = self.enums.get(name) {
            return Some(e.clone());
        }
        self.env.instances.borrow().sums.get(name).cloned()
    }

    /// Whether `name` is an `enum` (declared or instantiated).
    fn is_enum(&self, name: &str) -> bool {
        self.enums.contains_key(name) || self.env.instances.borrow().sums.contains_key(name)
    }

    /// Every `datac`/`enum` declaration the checker knows, generic instances
    /// included, as `(name, each field's type)` - what the derived
    /// clone/order/key fixpoints (`types::clonable_types`) run over.
    fn type_decls(&self) -> Vec<(String, Vec<Ty>)> {
        type_decls_of(&self.structs, &self.enums, &self.env.instances.borrow())
    }

    /// Whether `List<T>.clone`/`.slice` can be offered for a value of `ty`:
    /// a scalar, `String`/`Bytes`, or a `datac`/`enum` all of whose fields can
    /// be deep-copied.
    fn value_clonable(&self, ty: &Ty) -> bool {
        let set = crate::types::clonable_types(&self.type_decls(), &self.env.declared_drops);
        crate::types::value_is_clonable(ty, &set)
    }

    /// Whether `List<T>.sort` can be offered for a value of `ty`, by the same
    /// declaration-graph rule.
    fn value_sortable(&self, ty: &Ty) -> bool {
        let set = crate::types::sortable_types(&self.type_decls());
        crate::types::value_is_sortable(ty, &set)
    }

    /// The user types that are `Map` keys, by the same declaration-graph rule
    /// (A-6), plus the types that declare their own pair (A-12).
    fn keyable_set(&self) -> std::collections::HashSet<String> {
        crate::types::map_key_types(
            &self.type_decls(),
            &self.env.declared_keys,
            &self.env.declared_drops,
        )
    }

    /// Instantiates a generic template for one use site (A-2). Idempotent: a
    /// key that already exists is returned as-is, which is also what makes a
    /// recursive instantiation terminate.
    fn instantiate_type(&self, name: &str, args: &[Ty], span: Span) -> Result<Ty> {
        self.type_scope().instantiate(name, args, span)
    }

    /// Whether a passdown referenced from here is already initialized. True
    /// inside any function body (`init_state` is `None` there) and for
    /// anything declared before the initializer currently being checked.
    fn pd_initialized(&self, hidden: &str) -> bool {
        match &self.init_state {
            None => true,
            Some(done) => done.contains(hidden),
        }
    }
}

/// Every `datac`/`enum` declaration as `(name, each field's type)`, generic
/// instances included: what the derived clone, order and map-key fixpoints run
/// over. A name that is an `enum` also sits in `structs` as an empty record
/// (one name-resolution path covers both kinds), so the enum table's entry is
/// the one to keep.
fn type_decls_of(
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    instances: &InstanceState,
) -> Vec<(String, Vec<Ty>)> {
    let mut out: Vec<(String, Vec<Ty>)> = Vec::new();
    let mut push = |name: &str, fields: Vec<Ty>| out.push((name.to_string(), fields));
    for (name, s) in structs {
        if enums.contains_key(name) {
            continue;
        }
        push(name, s.fields.iter().map(|f| f.ty.clone()).collect());
    }
    let payloads = |e: &EnumDef| -> Vec<Ty> {
        e.variants
            .iter()
            .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
            .collect()
    };
    for (name, e) in enums {
        push(name, payloads(e));
    }
    for (name, s) in &instances.records {
        push(name, s.fields.iter().map(|f| f.ty.clone()).collect());
    }
    for (name, e) in &instances.sums {
        push(name, payloads(e));
    }
    out
}

/// Everything needed to turn a *written* type name into a `Ty`, with M5's
/// module qualification rules:
/// - a builtin (`i32`, `f64`, `str`, ...) is itself;
/// - `alias.Item` resolves through the current module's `impl` table;
/// - a bare name belongs to the current module, so it gets that module's
///   prefix - imported types are never reachable unqualified (strict
///   module-qualified namespace, per the plan).
struct TypeScope<'a> {
    structs: &'a HashMap<String, StructDef>,
    /// The declared `enum`s, which share one declaration order with the
    /// `datac`s above: a `Map` key's keyability recurses through an enum's
    /// variant payloads, and an enum is not in `structs` with them.
    enums: &'a HashMap<String, EnumDef>,
    /// Generic templates and the instances built so far (A-2). Shared, and
    /// mutable through a `RefCell`, because resolving `Pair<i32, String>`
    /// *creates* the instantiation it names - including from inside another
    /// instantiation's fields.
    env: TypeEnv,
    /// The type parameters in scope, bound to concrete arguments: non-empty
    /// only while a generic template's own fields are being resolved. A bare
    /// name found here wins over every other name, so `datac Pair<A, B>`'s
    /// `A` is the argument, never a type that happens to share the name.
    params: &'a HashMap<String, Ty>,
    prefix: &'a str,
    aliases: &'a HashMap<String, String>,
    /// Qualified names of `datac`s in this module that are declared *after*
    /// the declaration currently being checked. Used only to turn "unknown
    /// type 'Inner'" into an explanation: a `datac` may only name `datac`s
    /// declared before it (the checker builds its struct table in
    /// declaration order, so forward references were never resolvable).
    /// `None` at the sites where every `datac` in the program is already
    /// known (signatures, bodies, passdown types).
    later_datacs: Option<&'a std::collections::HashSet<String>>,
    /// Qualified name of the `datac` whose fields are being resolved right
    /// now, if any. A field of that type *by value* is rejected (a
    /// self-referential struct has no finite size); through a container it
    /// is allowed, which is how `datac Node(val next: Box<Node>)` works.
    in_progress: Option<&'a str>,
    /// The folded `const`s of the whole program, and the functions a compile
    /// time fold may call (A-19). Both are here for one reason: a type may
    /// carry an expression, and the only expression a type carries is an
    /// array's `[T; N]` length (A-18). `None` at the few sites where no
    /// constant is in reach, which turns a length that needs one into a
    /// diagnostic instead of a wrong answer.
    consts: Option<&'a HashMap<String, ConstInfo>>,
    const_fns: Option<&'a ConstFns>,
    /// The canonical names of the program's opaque handles (gap 1), so a
    /// written `Widget` or `ffi.Widget` resolves to `Ty::Handle` instead of
    /// falling through to "unknown type".
    handles: &'a std::collections::HashSet<String>,
}

impl TypeScope<'_> {
    /// The user types that are `Map` keys, from the declarations this view of
    /// the program can see (A-6, A-12). A `Map` key is resolved before the
    /// whole program is known, so this runs the same fixpoint `Ctx` runs, over
    /// the declarations gathered so far: a type a field could legally name is
    /// always an earlier declaration, and an earlier declaration is present.
    fn keyable_set(&self) -> std::collections::HashSet<String> {
        let decls = type_decls_of(self.structs, self.enums, &self.env.instances.borrow());
        crate::types::map_key_types(&decls, &self.env.declared_keys, &self.env.declared_drops)
    }

    /// A written type name, or a container over one. `indirect` is true
    /// when the type is being resolved *inside* a `Box`/`List` argument,
    /// where referring to the `datac` currently being declared (or to a
    /// `datac` declared later) is legal: both containers hold a pointer, so
    /// the indirection is what makes a recursive type finite.
    fn resolve(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        let resolved = self.resolve_base(ty, indirect)?;
        // A `datac` that contains itself *by value* has no finite size.
        // Through a container it is fine - that is the whole point of `Box`
        // and of a list's pointer buffer - so the check is skipped there.
        if !indirect {
            if let Some(ip) = self.in_progress {
                if resolved == Ty::Struct(ip.to_string()) {
                    return Err(TypeError {
                        message: format!(
                            "'{ip}' cannot hold itself by value - that type has no finite size; put it behind indirection ('val next: Box<{ip}>', or a 'List<{ip}>' for many)"
                        ),
                        span: ty.span,
                    });
                }
            }
        }
        Ok(resolved)
    }

    /// Whether a canonical key names a declaration of the module this scope
    /// belongs to. A canonical key carries its declaring module's symbol
    /// prefix (`shapes.circle.Circle`), which is how a receiver declaration
    /// arrives here, and a module's own keys are that prefix plus one bare
    /// identifier. The entry module's prefix is empty and its keys are the
    /// unprefixed names, so a key that keeps a dot in it is never one of its
    /// own.
    fn owns_canonical_key(&self, key: &str) -> bool {
        key.strip_prefix(self.prefix)
            .is_some_and(|item| !item.contains('.'))
    }

    fn resolve_base(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        // A const argument (`Buf<16>`, A-20) only means something where a
        // `const` parameter's declaration says so; the generic-instantiation
        // path folds it there. Anywhere else it is a value written where a
        // type belongs.
        if ty.const_expr.is_some() {
            return Err(TypeError {
                message: format!(
                    "'{}' is a constant expression, not a type: a constant argument is only legal where the declaration's parameter is 'const' ('datac Buf<const N: usize> ...'), and a type belongs here",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        // `impl Trait` in argument position (A-20) is desugared at the
        // declaration, so any one still here is in a position the sugar does
        // not cover (a `let`, a field, a return).
        if let Some(trait_name) = &ty.impl_trait {
            return Err(TypeError {
                message: format!(
                    "'impl {trait_name}' is only legal as a whole parameter type of a declared function: write 'fun f(x: impl {trait_name})', and name a type parameter where a stored, returned or container type belongs"
                ),
                span: ty.span,
            });
        }
        // `scoped` is a *parameter* modifier (`plan/scoped-blocks.md` part 2)
        // and is handled by `resolve_param_mode`, before this is reached. Any
        // other position - a return type, a `val` annotation, a `datac`
        // field, another function type's own parameter - would store the
        // value, which is exactly what the modifier forbids.
        if ty.scoped {
            return Err(TypeError {
                message: format!(
                    "'scoped {}' is only legal on a top-level parameter of a declaration, not here: a scoped function value may only live for the call it was written in, so it cannot be stored, returned or nested in another type",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        // A borrowed trait object (A-12 slice 4): `&dyn Name` / `&mut dyn
        // Name`. The trait must be declared and object safe; the legal
        // positions (a parameter or a local) are enforced by the callers that
        // would otherwise store the value, exactly as they are for `Ty::Ref`
        // (`is_borrowed_field_ty` and `resolve_element`).
        if let Some(written) = &ty.dyn_trait {
            // A bare `dyn Name` is legal only as the argument of a `Box`, which
            // `resolve_base` handles in its `Box` arm before the argument is
            // resolved, so reaching here without the `&` is the bare spelling
            // everywhere else (A-12 slice 7).
            if !ty.amp {
                return Err(TypeError {
                    message: format!(
                        "'dyn {written}' is not a type on its own: an owned trait object is written 'Box<dyn {written}>', and a borrowed one '&dyn {written}'"
                    ),
                    span: ty.span,
                });
            }
            let qualified = self.qualify(written, ty.span)?;
            if !self.env.traits.contains_key(&qualified) {
                return Err(TypeError {
                    message: format!(
                        "'&dyn {written}' names no declared trait: declare 'trait {written} {{ ... }}' first (or import the module that does)"
                    ),
                    span: ty.span,
                });
            }
            // A trait named through another module's alias has to be exported
            // by that module, exactly as a bound on the same trait does.
            if let Some((alias, item)) = written.split_once('.') {
                if !exported(&qualified, &self.env) {
                    return Err(private_item_error(alias, item, ty.span));
                }
            }
            let spelling = if ty.amp_mut {
                format!("&mut dyn {written}")
            } else {
                format!("&dyn {written}")
            };
            check_dyn_object_safety(&spelling, &qualified, ty.span, &self.env.traits)?;
            self.env.dyn_traits.borrow_mut().insert(qualified.clone());
            return Ok(Ty::Dyn {
                mutable: ty.amp_mut,
                trait_name: qualified,
            });
        }
        // A tuple type (`plan/syntax.md` 3): `(A, B)`. Every element is stored
        // by value, exactly as a `datac` field is, so the element rules are the
        // field rules: an element may not be a borrow, and `unit` cannot be
        // stored either. `indirect` is passed through, because a tuple inside a
        // `Box`/`List` argument is behind a pointer just as a named type is
        // there, so an element declared later is legal in that position.
        if let Some(elems) = &ty.tuple {
            let mut resolved = Vec::with_capacity(elems.len());
            for (i, e) in elems.iter().enumerate() {
                if e.amp {
                    return Err(TypeError {
                        message: format!(
                            "'&{}' cannot be element {} of a tuple: an element is stored by value, so a borrow of one has nowhere to put its lifetime; use the owned type",
                            e.display(),
                            i + 1
                        ),
                        span: e.span,
                    });
                }
                let elem = self.resolve(e, indirect)?;
                if crate::types::is_borrowed_field_ty(&elem) {
                    return Err(TypeError {
                        message: format!(
                            "element {} of the tuple is a borrowed type ('{}'): a tuple holds owned data only in v1, exactly as a datac field does, so a '&str', 'View' or reference element is not expressible; an owned 'String' is the way to keep text in one",
                            i + 1,
                            elem.display()
                        ),
                        span: e.span,
                    });
                }
                if elem == Ty::Unit {
                    return Err(TypeError {
                        message: format!(
                            "element {} of the tuple is 'unit': 'unit' is the absent value and cannot be stored, exactly as a datac field cannot be",
                            i + 1
                        ),
                        span: e.span,
                    });
                }
                resolved.push(elem);
            }
            return Ok(tuple_ty(&self.env, resolved));
        }
        // A fixed-size array (A-18): `[T; N]`. The length is a compile-time
        // constant expression (A-19) folded here, once, so the type is a
        // concrete `[T; N]` for every pass after this one.
        if let Some(elem) = &ty.array_elem {
            let len_expr = ty
                .array_len
                .as_ref()
                .expect("the parser builds an array type with both halves");
            let len = self.array_len(len_expr)?;
            let elem = self.resolve_element(elem)?;
            if !crate::types::is_array_element(&elem) {
                return Err(TypeError {
                    message: array_element_error(&elem),
                    span: ty.span,
                });
            }
            return Ok(Ty::Array(Box::new(elem), len));
        }
        // A function type (A-4): `fun(i32, String).return: bool`. Checked before
        // every name, because it is the one written type that is not one -
        // and `fun` cannot be a name at all, since it is a keyword. A closure
        // type (A-4 phase E) is the same shape under `closure`.
        if let Some(params) = &ty.fn_params {
            if ty.is_closure() {
                return self.resolve_closure_type(ty, params);
            }
            return self.resolve_fn_type(ty, params);
        }
        // `I::Item` / `Self::Item` (A-12 slice 5): an associated type projected
        // from a type parameter, or from `Self` in a trait signature. The
        // projection's own key is stored beside the parameter's binding, so a
        // bound that declares no such type is refused here.
        if let Some(item) = &ty.assoc {
            let key = format!("{}::{}", ty.name, item);
            if let Some(bound) = self.params.get(&key) {
                return Ok(bound.clone());
            }
            return Err(TypeError {
                message: format!(
                    "'{}::{}' is not a known associated type: bind '{}' with a trait that carries 'type {}' ('trait ... {{ type {} }}'), so the projection names the type an impl supplies",
                    ty.name, item, ty.name, item, item
                ),
                span: ty.span,
            });
        }
        // A type parameter in scope (`datac Pair<A, B>`'s `A`) is whatever
        // the instantiation bound it to (A-2). Checked before every other
        // name, so a parameter shadows a type of the same name.
        if ty.args.is_empty() && !ty.name.contains('.') {
            if let Some(bound) = self.params.get(&ty.name) {
                if let Ty::ConstInt { .. } = bound {
                    return Err(TypeError {
                        message: format!(
                            "'{}' is a const parameter, not a type: use it as a value ('i < {}'), as an array length ('[i32; {}]') or in a repeat expression ('[0; {}]'), and write a type parameter where a type belongs",
                            ty.name, ty.name, ty.name, ty.name
                        ),
                        span: ty.span,
                    });
                }
                return Ok(bound.clone());
            }
        }
        // The generic names v1 knows (`List<T>` from C1, `Box<T>` from M8,
        // `Map<V>` from MK5-S5, `Atomic<T>` from MK9-S3-M1 stage A1, `Thread<T>`
        // from its stage B1). All are compiler builtins, not user-definable
        // types; each checks its element against what its representation can
        // hold.
        // `Box<dyn Name>` (A-12 slice 7): the one place a bare `dyn Name` is a
        // type. It is handled before the element checks below, because a trait
        // object is not an element type: the box owns the erased value itself,
        // not a value of the element's type. The trait must be declared and
        // object safe, exactly as it must be behind a `&dyn`, and the resolved
        // type name is recorded so the pair's C typedef exists even when
        // nothing coerces to it.
        if ty.name == "Box" && ty.args.len() == 1 && ty.args[0].dyn_trait.is_some() {
            let arg = &ty.args[0];
            let written = arg.dyn_trait.as_ref().expect("matched above");
            if arg.amp {
                return Err(TypeError {
                    message: format!(
                        "'Box<&dyn {written}>' is not an owned trait object: a box holds its value, so write 'Box<dyn {written}>' for an owned one or '&dyn {written}' for a borrowed one"
                    ),
                    span: arg.span,
                });
            }
            let qualified = self.qualify(written, arg.span)?;
            if !self.env.traits.contains_key(&qualified) {
                return Err(TypeError {
                    message: format!(
                        "'Box<dyn {written}>' names no declared trait: declare 'trait {written} {{ ... }}' first (or import the module that does)"
                    ),
                    span: arg.span,
                });
            }
            if let Some((alias, item)) = written.split_once('.') {
                if !exported(&qualified, &self.env) {
                    return Err(private_item_error(alias, item, arg.span));
                }
            }
            check_dyn_object_safety(
                &format!("Box<dyn {written}>"),
                &qualified,
                arg.span,
                &self.env.traits,
            )?;
            self.env.dyn_traits.borrow_mut().insert(qualified.clone());
            return Ok(Ty::BoxDyn {
                trait_name: qualified,
            });
        }
        if ty.name == "List"
            || ty.name == "Box"
            || ty.name == "Map"
            || ty.name == "Atomic"
            || ty.name == "Thread"
            || ty.name == "Mutex"
            || ty.name == "RwLock"
            || ty.name == "Ptr"
        {
            // `Map<K, V>` names its key type, and `Map<V>` is the shorthand
            // for a `String` key - which is what every use site written before
            // A-2 says, and the two spellings are the same `Ty`.
            let (key, value_arg, value_span) = if ty.name == "Map" {
                match ty.args.len() {
                    1 => (Ty::String, &ty.args[0], ty.args[0].span),
                    2 => {
                        let key = self.resolve_element(&ty.args[0])?;
                        if !crate::types::is_map_key_with(&key, &self.keyable_set()) {
                            return Err(TypeError {
                                message: map_key_error(&key),
                                span: ty.args[0].span,
                            });
                        }
                        (key, &ty.args[1], ty.args[1].span)
                    }
                    n => {
                        return Err(TypeError {
                            message: format!(
                                "'Map' takes 1 or 2 type arguments ('Map<V>' or 'Map<K, V>'), found {n}"
                            ),
                            span: ty.span,
                        })
                    }
                }
            } else {
                if ty.args.len() != 1 {
                    return Err(TypeError {
                        message: format!(
                            "'{}' takes exactly 1 type argument, found {}",
                            ty.name,
                            ty.args.len()
                        ),
                        span: ty.span,
                    });
                }
                (Ty::Unit, &ty.args[0], ty.args[0].span)
            };
            let elem = self.resolve_element(value_arg)?;
            let (ok, error) = if ty.name == "List" {
                (crate::types::is_list_element(&elem), list_element_error(&elem))
            } else if ty.name == "Map" {
                (crate::types::is_map_element(&elem), map_element_error(&elem))
            } else if ty.name == "Atomic" {
                (crate::types::is_atomic_element(&elem), atomic_element_error(&elem))
            } else if ty.name == "Thread" {
                (crate::types::is_thread_element(&elem), thread_element_error(&elem))
            } else if ty.name == "Mutex" {
                (crate::types::is_mutex_element(&elem), mutex_element_error(&elem))
            } else if ty.name == "RwLock" {
                (crate::types::is_rwlock_element(&elem), rwlock_element_error(&elem))
            } else if ty.name == "Ptr" {
                (crate::types::is_ptr_element(&elem), crate::types::ptr_element_error(&elem))
            } else {
                (crate::types::is_box_element(&elem), box_element_error(&elem))
            };
            if !ok {
                return Err(TypeError {
                    message: error,
                    span: value_span,
                });
            }
            return Ok(if ty.name == "List" {
                Ty::List(Box::new(elem))
            } else if ty.name == "Map" {
                Ty::Map(Box::new(key), Box::new(elem))
            } else if ty.name == "Atomic" {
                Ty::Atomic(Box::new(elem))
            } else if ty.name == "Thread" {
                Ty::Thread(Box::new(elem))
            } else if ty.name == "Mutex" {
                Ty::Mutex(Box::new(elem))
            } else if ty.name == "RwLock" {
                Ty::RwLock(Box::new(elem))
            } else if ty.name == "Ptr" {
                Ty::Ptr(Box::new(elem))
            } else {
                Ty::Box(Box::new(elem))
            });
        }
        if !ty.args.is_empty() {
            // A user-declared generic type (A-2): `Pair<i32, String>`. The
            // arguments are resolved first (they are types in their own
            // right), then the declaration is instantiated for them.
            let qualified = self.qualify(&ty.name, ty.span)?;
            if let Some(tmpl) = self.env.generics.get(&qualified) {
                check_type_exported(
                    &ty.name,
                    self.structs,
                    self.enums,
                    self.aliases,
                    &self.env,
                    ty.span,
                )?;
                if ty.args.len() != tmpl.params.len() {
                    return Err(TypeError {
                        message: format!(
                            "'{}' takes {} type argument(s), found {}",
                            tmpl.name,
                            tmpl.params.len(),
                            ty.args.len()
                        ),
                        span: ty.span,
                    });
                }
                let mut args = Vec::with_capacity(ty.args.len());
                for (i, a) in ty.args.iter().enumerate() {
                    let declared = tmpl
                        .const_params
                        .iter()
                        .find(|(n, _)| tmpl.params.get(i) == Some(n))
                        .map(|(_, t)| t);
                    args.push(self.resolve_type_arg(a, declared)?);
                }
                return self.instantiate(&qualified, &args, ty.span);
            }
            return Err(TypeError {
                message: format!(
                    "'{}' does not take type arguments: the compiler's generic types are 'List<T>', 'Box<T>', 'Map<V>', 'Atomic<T>', 'Thread<T>', 'Mutex<T>', 'RwLock<T>' and 'Ptr<T>', and a user type has to be declared with parameters to take them ('datac {name}<T> ...')",
                    ty.name,
                    name = ty.name
                ),
                span: ty.span,
            });
        }
        if let Some(builtin) = Ty::from_name(&ty.name) {
            return Ok(builtin);
        }
        // A name that is already a known `struct` key is canonical: this is
        // how a receiver declaration (`fun Circle.area()` inside
        // `shapes/circle.kp`) arrives here, since `resolve.rs` rewrites the
        // receiver type to its module-prefixed name when it merges modules.
        // Only the declaring module's own keys may take that shortcut. A
        // canonical key of *another* module is how that module's type is
        // written through an alias (`util.Tally`), and it has to take the
        // same qualifying and visibility path every other written name takes
        // (A-13). Skipping it is what let a private type cross the boundary.
        if self.structs.contains_key(&ty.name) && self.owns_canonical_key(&ty.name) {
            return Ok(Ty::Struct(ty.name.clone()));
        }
        let qualified = self.qualify(&ty.name, ty.span)?;
        if self.handles.contains(&qualified) {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            // Always a pointer, so `indirect` has nothing to decide: a handle
            // is complete the moment it is declared, and naming one by value
            // is naming a pointer.
            Ok(Ty::Handle(qualified))
        } else if self.structs.contains_key(&qualified) {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            Ok(Ty::Struct(qualified))
        } else if let Some(tmpl) = self
            .env
            .generics
            .get(&qualified)
            .or_else(|| self.env.generics.get(&ty.name))
        {
            check_type_exported(
                &ty.name,
                self.structs,
                self.enums,
                self.aliases,
                &self.env,
                ty.span,
            )?;
            // A generic type written without its arguments. The generic half
            // of the name is enough to say what is missing.
            Err(TypeError {
                message: format!(
                    "'{}' is a generic type - it needs {} type argument(s): '{}<{}>'",
                    tmpl.name,
                    tmpl.params.len(),
                    tmpl.name,
                    tmpl.params.join(", ")
                ),
                span: ty.span,
            })
        } else if self
            .later_datacs
            .is_some_and(|later| later.contains(&qualified))
        {
            // Forward reference. Legal through a container (M8: a pointer is
            // complete before either `datac` is, so the pair can be
            // mutually recursive), rejected by value, which is the
            // infinite-size case and the reason the struct table is built in
            // declaration order at all.
            if indirect {
                Ok(Ty::Struct(qualified))
            } else {
                Err(TypeError {
                    message: format!(
                        "'{}' is declared after this type - a type may only name types declared before it by value; behind a 'Box' or 'List' it may name any type, including itself",
                        ty.name
                    ),
                    span: ty.span,
                })
            }
        } else {
            Err(TypeError {
                message: match qualified_hint(
                    &ty.name,
                    self.structs.keys(),
                    self.aliases,
                    &self.env,
                ) {
                    Some(hint) => format!("unknown type '{}' - did you mean {hint}?", ty.name),
                    None => format!("unknown type '{}'", ty.name),
                },
                span: ty.span,
            })
        }
    }

    /// Resolves one element (or type argument) of a container: an array's
    /// element, a `List`/`Box` element, a `Map` key or value. Its own `&` is
    /// refused here rather than dropped, because `resolve_base` has no position
    /// to read it in and a silently-ignored `&` turns `List<&i32>` into
    /// `List<i32>`, a written borrow that means something else entirely.
    fn resolve_element(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                message: format!(
                    "'&{}' cannot be a container element: an element has to outlive the container that holds it, and v1 has no lifetime syntax to say so; use the owned type",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        self.resolve(ty, true)
    }

    /// Folds the length of an array type or of a repeat literal (A-19).
    fn array_len(&self, expr: &ast::Expr) -> Result<u64> {
        // A const parameter as a length (A-20): `[T; N]` inside a template,
        // where the instantiation binds `N` to a value.
        if let ast::Expr::Ident(name, _) = expr {
            if let Some(Ty::ConstInt { value, ty }) = self.params.get(name) {
                if matches!(**ty, Ty::Char) {
                    return Err(TypeError {
                        message: format!(
                            "'{name}' is a 'char' const parameter, and an array length has to be an integer"
                        ),
                        span: expr.span(),
                    });
                }
                if *value < 1 {
                    return Err(TypeError {
                        message: format!(
                            "an array length has to be at least 1, and '{name}' is {value}: v1 has no zero-length form ('[T; 0]' would be a C array with no elements, which is not standard C)"
                        ),
                        span: expr.span(),
                    });
                }
                return Ok(*value as u64);
            }
        }
        // A const parameter anywhere else in a length is arithmetic on a
        // const parameter in a type, which is not in this slice (A-20):
        // refuse it by name rather than folding it.
        if let Some(name) = const_param_in(expr, self.params) {
            return Err(TypeError {
                message: format!(
                    "'{name}' is a const parameter, and arithmetic on one in a type ('[T; {name} + 1]') is not in this slice: write the length the instantiation binds ('[T; {name}]'), or compute it from a 'const' ('const M: usize = ...')"
                ),
                span: expr.span(),
            });
        }
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                message: "an array length has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts).array_len(
            expr,
            self.prefix,
            self.aliases,
        )
    }

    /// One type-argument position of a generic instantiation (A-20). When the
    /// parameter there is a `const`, the written argument is folded with the
    /// parameter's declared type; otherwise it resolves as a type and a
    /// constant expression written in its place is refused.
    fn resolve_type_arg(&self, ty: &ast::TypeExpr, declared: Option<&Ty>) -> Result<Ty> {
        match declared {
            None => {
                if ty.const_expr.is_some() {
                    return Err(TypeError {
                        message: format!(
                            "a type argument expected a type, but '{}' is a constant expression: declare the parameter 'const' ('datac Buf<const N: usize>') where a value belongs",
                            ty.display()
                        ),
                        span: ty.span,
                    });
                }
                self.resolve(ty, true)
            }
            Some(declared) => {
                let expr = ty
                    .const_expr
                    .as_deref()
                    .cloned()
                    .or_else(|| named_const_expr(ty));
                let Some(expr) = expr else {
                    return Err(TypeError {
                        message: format!(
                            "a 'const' parameter expects a constant expression of type '{}', but the type '{}' was written: write a literal or a 'const' ('Buf<16>', 'Buf<SIZE>')",
                            declared.display(),
                            ty.display()
                        ),
                        span: ty.span,
                    });
                };
                self.fold_const_arg(&expr, declared)
            }
        }
    }

    /// Folds one const argument to a value of the parameter's declared type
    /// (A-20). The existing constant evaluation is the whole of it, so a
    /// literal, a `const`, a module `const` and a plain `fun` call all work
    /// exactly as they do in a `const` initializer.
    fn fold_const_arg(&self, expr: &ast::Expr, declared: &Ty) -> Result<Ty> {
        // A const parameter written as a const argument (`Ring<N>` in the
        // template's own signature, A-20): its value is what the enclosing
        // instantiation bound, re-typed to the parameter it is being passed
        // to.
        if let ast::Expr::Ident(name, span) = expr {
            if let Some(Ty::ConstInt { value, ty: source }) = self.params.get(name) {
                let value = *value;
                if matches!(**source, Ty::Char) != matches!(declared, Ty::Char) {
                    return Err(TypeError {
                        message: format!(
                            "const parameter '{name}' is a '{}' and cannot be passed where a '{}' is expected",
                            source.display(),
                            declared.display()
                        ),
                        span: *span,
                    });
                }
                if !matches!(declared, Ty::Bool | Ty::Char) {
                    check_int_literal(value, declared, false, *span)?;
                }
                return Ok(Ty::ConstInt {
                    value,
                    ty: Box::new(declared.clone()),
                });
            }
        }
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                message: "a const argument has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        let evaluator =
            ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts);
        let mut scope = ConstScope::new(self.prefix.to_string(), self.aliases.clone());
        let (value, _) = evaluator.eval(expr, Some(declared), &mut scope)?;
        let value = match (value, declared) {
            (ConstValue::Char(c), Ty::Char) => i128::from(c),
            (ConstValue::Int(n), t) if !matches!(t, Ty::Char) => n,
            (ConstValue::Bool(b), t) if !matches!(t, Ty::Char) => i128::from(b),
            _ => {
                return Err(TypeError {
                    message: format!(
                        "a const parameter of type '{}' takes {}, and this expression is not one",
                        declared.display(),
                        if matches!(declared, Ty::Char) {
                            "a character value"
                        } else {
                            "an integer or boolean value"
                        }
                    ),
                    span: expr.span(),
                })
            }
        };
        Ok(Ty::ConstInt {
            value,
            ty: Box::new(declared.clone()),
        })
    }

    /// Folds a const argument with no parameter type to steer it (A-20): used
    /// by the explicit-instantiation path, which resolves its arguments before
    /// it knows which template they belong to. The folded result carries the
    /// expression's own type, and `bind_const_args` re-types it where the
    /// declaration is known.
    fn fold_const_arg_any(&self, expr: &ast::Expr) -> Result<Ty> {
        let (Some(consts), Some(const_fns)) = (self.consts, self.const_fns) else {
            return Err(TypeError {
                message: "a const argument has to be a compile-time constant, and this position has no constants in reach".to_string(),
                span: expr.span(),
            });
        };
        let evaluator =
            ConstEvaluator::new(const_fns, self.structs, &self.enums, &self.env, consts);
        let mut scope = ConstScope::new(self.prefix.to_string(), self.aliases.clone());
        let (value, ty) = evaluator.eval(expr, None, &mut scope)?;
        let (value, ty) = match value {
            ConstValue::Int(n) => (n, ty),
            ConstValue::Bool(b) => (i128::from(b), ty),
            ConstValue::Char(c) => (i128::from(c), Ty::Char),
            _ => {
                return Err(TypeError {
                    message: "a const argument has to be an integer, boolean or character value".to_string(),
                    span: expr.span(),
                })
            }
        };
        Ok(Ty::ConstInt {
            value,
            ty: Box::new(ty),
        })
    }

    /// A written name as its canonical, module-qualified key: a bare name
    /// belongs to the current module, and `alias.Item` resolves through the
    /// module's `impl` table. Factored out of `resolve_base` so the generic
    /// path and the ordinary one qualify a name exactly the same way.
    fn qualify(&self, name: &str, span: Span) -> Result<String> {
        match name.split_once('.') {
            Some((alias, item)) => match self.aliases.get(alias) {
                Some(prefix) => Ok(format!("{prefix}{item}")),
                None => Err(TypeError {
                    message: format!(
                        "unknown module alias '{alias}' in type '{name}' - was it imported?"
                    ),
                    span,
                }),
            },
            None => Ok(format!("{}{}", self.prefix, name)),
        }
    }

    /// Builds (or looks up) the concrete type for one instantiation of a
    /// generic template (A-2). The instance is registered *before* its fields
    /// are resolved only as an in-progress mark, so a recursive field
    /// (`Box<Node<i32>>` inside `Node<i32>`) terminates on the key rather
    /// than recursing forever; the finished definition is stored afterwards.
    fn instantiate(&self, name: &str, args: &[Ty], span: Span) -> Result<Ty> {
        // A-20: a const argument is re-typed to its parameter's declared type
        // (and range-checked) before anything keys off it, so the instance key
        // and every later binding see the same value.
        let mut args = args.to_vec();
        let tmpl = self.env.generics[name].clone();
        bind_const_args(&tmpl.params, &tmpl.const_params, &mut args, span)?;
        let args = args.as_slice();
        let key = instance_key(name, args);
        {
            let st = self.env.instances.borrow();
            if st.records.contains_key(&key) || st.sums.contains_key(&key) {
                return Ok(Ty::Struct(key));
            }
        }
        {
            let mut st = self.env.instances.borrow_mut();
            if !st.in_progress.insert(key.clone()) {
                return Ok(Ty::Struct(key));
            }
        }
        let params: HashMap<String, Ty> = tmpl
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let scope = TypeScope {
            structs: self.structs,
            enums: self.enums,
            env: self.env.clone(),
            params: &params,
            prefix: &tmpl.prefix,
            aliases: &tmpl.aliases,
            later_datacs: None,
            in_progress: Some(&key),
            consts: self.consts,
            const_fns: self.const_fns,
            handles: self.handles,
        };
        let built = scope.build_instance(&tmpl, &key);
        self.env.instances.borrow_mut().in_progress.remove(&key);
        let inst = built.map_err(|e| TypeError {
            // A template's fields live in the template's own file, so an
            // error found while resolving them cannot keep its own span: the
            // caller attributes the error to *its* module, which is where
            // the instantiation was written. Re-point the span at the use
            // site and name the instance in the message instead of printing
            // a location in the wrong file.
            message: format!("in '{}': {}", key, e.message),
            span,
        })?;
        let mut st = self.env.instances.borrow_mut();
        match inst {
            Instance::Record(s) => {
                st.records.insert(key.clone(), s);
            }
            Instance::Sum(e) => {
                st.sums.insert(key.clone(), e);
            }
        }
        st.origin
            .insert(key.clone(), (name.to_string(), args.to_vec()));
        Ok(Ty::Struct(key))
    }

    /// Resolves a template's fields with its parameters bound, producing one
    /// concrete definition. Validation the non-generic path does at
    /// declaration time (duplicate field names, borrowed fields, duplicate
    /// variants) happens here instead, because a field type only means
    /// something once the parameters are bound.
    fn build_instance(&self, tmpl: &GenericTemplate, key: &str) -> Result<Instance> {
        match &tmpl.kind {
            GenericKind::Record(fields) => {
                let mut out: Vec<FieldDef> = Vec::new();
                for f in fields {
                    if out.iter().any(|fd| fd.name == f.name) {
                        return Err(TypeError {
                            message: format!(
                                "field '{}' is defined more than once in datac '{}'",
                                f.name, tmpl.name
                            ),
                            span: f.span,
                        });
                    }
                    let ty = self.resolve(&f.ty, false)?;
                    if crate::types::is_borrowed_field_ty(&ty) {
                        return Err(TypeError {
                            message: format!(
                                "field '{}' is a borrowed type ('{}') - a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible - an owned 'String' is the way to keep text in one",
                                f.name,
                                ty.display()
                            ),
                            span: f.span,
                        });
                    }
                    out.push(FieldDef {
                        name: f.name.clone(),
                        ty,
                        mutable: f.mutable,
                    });
                }
                Ok(Instance::Record(StructDef {
                    name: key.to_string(),
                    fields: out,
                    repr: tmpl.repr.clone(),
                    // Reassigned by `order_declarations` once every
                    // instantiation is known; the value here is a sentinel.
                    order: usize::MAX,
                }))
            }
            GenericKind::Sum(variants) => {
                let mut out: Vec<crate::types::VariantDef> = Vec::new();
                for v in variants {
                    if out.iter().any(|vd| vd.name == v.name) {
                        return Err(TypeError {
                            message: format!(
                                "variant '{}' is defined more than once in enum '{}'",
                                v.name, tmpl.name
                            ),
                            span: v.span,
                        });
                    }
                    let mut fields: Vec<FieldDef> = Vec::new();
                    for f in &v.fields {
                        if fields.iter().any(|fd| fd.name == f.name) {
                            return Err(TypeError {
                                message: format!(
                                    "payload field '{}' is defined more than once in variant '{}'",
                                    f.name, v.name
                                ),
                                span: f.span,
                            });
                        }
                        let ty = self.resolve(&f.ty, false)?;
                        if crate::types::is_borrowed_field_ty(&ty) {
                            return Err(TypeError {
                                message: format!(
                                    "payload field '{}' is a borrowed type ('{}') - an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible - an owned 'String' is the way to keep text in one",
                                    f.name,
                                    ty.display()
                                ),
                                span: f.span,
                            });
                        }
                        fields.push(FieldDef {
                            name: f.name.clone(),
                            ty,
                            mutable: f.mutable,
                        });
                    }
                    out.push(crate::types::VariantDef {
                        name: v.name.clone(),
                        fields,
                        discriminant: v.discriminant,
                    });
                }
                Ok(Instance::Sum(EnumDef {
                    name: key.to_string(),
                    variants: out,
                    repr: tmpl.repr.clone(),
                    order: usize::MAX,
                }))
            }
        }
    }

    fn resolve_param(&self, p: &ast::Param) -> Result<Ty> {
        self.resolve_param_mode(&p.ty, p.mode)
    }

    /// One parameter's type, from the type as written and the qualifier in
    /// front of it. Shared by a declaration's parameter and a function type's
    /// parameter (A-4), because the two have to mean the same thing: a
    /// written `fun(String)` has to be the type of `fun f(s: String)`.
    ///
    /// `move T` resolves to the bare type - the same by-value shape a `Copy`
    /// type already has. That is the whole type-level difference, and it is
    /// what makes a moved argument an ordinary whole-value read at the call
    /// site: no new argument-matching rule, and `borrowck.rs` marks the
    /// caller's binding moved exactly as `val q = p` does.
    fn resolve_param_mode(&self, ty: &ast::TypeExpr, mode: ast::ParamMode) -> Result<Ty> {
        if ty.scoped {
            return self.resolve_scoped_param(ty, mode);
        }
        if mode.moves() {
            return self.resolve_move_param(ty);
        }
        // `&[T; N]`: a written `&` on a *parameter* is otherwise what the bare
        // name already means, but an array is a by-value type, so the `&` is
        // the one way to write "a borrow of the caller's array" (A-20, and the
        // shape the const-generic examples use). An exclusive `mut [T; N]`
        // takes the ordinary path below.
        if ty.array_elem.is_some() && ty.amp && !mode.mutable() {
            let base = self.resolve(ty, false)?;
            return Ok(Ty::Ref {
                mutable: false,
                inner: Box::new(base),
            });
        }
        let base = self.resolve(ty, false)?;
        if mode.mutable() && matches!(base, Ty::Fn { .. }) {
            // A function value is Copy, so an exclusive borrow of one would
            // need a pointer to a function pointer that nothing can usefully
            // write through.
            return Err(TypeError {
                message: format!(
                    "'mut' cannot be written on the function type '{}' - a function value is Copy (it is a plain function pointer), so there is nothing to write through; write the type without 'mut'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        Ok(implicit_borrow_wrap(base, mode.mutable()))
    }

    /// `scoped fun(T).return: U` - the non-escaping callback type
    /// (`plan/scoped-blocks.md` part 2). The qualifier is only meaningful as
    /// written: `mut` needs something to write through and `move` something
    /// to own, and a scoped value is neither, so both are refused by name
    /// rather than silently dropped. The parameters and return resolve as a
    /// `fun` type's do, with `scoped` refused inside them (a scoped value
    /// nested in another type could be stored as that type).
    fn resolve_scoped_param(&self, ty: &ast::TypeExpr, mode: ast::ParamMode) -> Result<Ty> {
        if mode.moves() {
            return Err(TypeError {
                message: format!(
                    "'move' cannot be written on '{}' - a scoped function value is a fat pointer with a stack environment, so there is nothing to own; write it without 'move'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        if mode.mutable() {
            return Err(TypeError {
                message: format!(
                    "'mut' cannot be written on '{}' - a scoped function value is Copy and has nothing to write through; write it without 'mut'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        if ty.amp {
            return Err(TypeError {
                message: format!(
                    "'&' cannot be written on '{}' - a scoped function value is already a fat pointer passed by value; write it without '&'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let params = ty.fn_params.as_ref().expect("the parser only sets 'scoped' before 'fun'");
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            if p.ty.scoped {
                return Err(TypeError {
                    message: format!(
                        "'scoped' cannot be nested inside another function type ('{}'): a value of such a type could be stored, which a scoped function value may not be",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        let ret = match &ty.fn_ret {
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::ScopedFn {
            params: resolved,
            ret: Box::new(ret),
        })
    }

    /// The type of a `move T` parameter, with the spellings that have nothing
    /// to hand over refused: a `str` is already a borrowed view, `&T` is
    /// already a copy of a borrow, and a function value is already Copy.
    /// Those would read as an ownership transfer and mean nothing.
    fn resolve_move_param(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        let base = self.resolve(ty, false)?;
        let nothing_to_own = ty.amp
            || matches!(base, Ty::Str | Ty::Ref { .. } | Ty::Fn { .. });
        if nothing_to_own {
            // `TypeExpr::display` prints only the name, so an explicit `&`
            // has to be put back for the diagnostic to quote what was
            // written.
            let written = if ty.amp {
                format!("&{}", ty.display())
            } else {
                ty.display()
            };
            return Err(TypeError {
                message: format!(
                    "'move' cannot be written on '{written}' - a 'str' is already a borrowed view, '&T' is already a copy of a borrow, and a function value is already Copy, so there is nothing to take ownership of; write the owned type instead ('move String')"
                ),
                span: ty.span,
            });
        }
        Ok(base)
    }

    /// A function type (A-4). Each parameter goes through the *same* rule a
    /// declaration's parameter does - an implicit shared borrow for an owned
    /// type, `mut` for an exclusive one, `move` for ownership - which is what
    /// makes a written `fun(String)` and the signature of `fun f(s: String)`
    /// the same type, and therefore what makes passing `f` where a
    /// `fun(String)` is expected type-check with no conversion.
    ///
    /// Two placements are refused, both because the C declarator has no
    /// sensible spelling for them: a function type that *returns* a function
    /// type, and a borrow of a function value (which is Copy, so there is
    /// nothing to borrow). A function type as a parameter of a function type
    /// is fine - `int32_t (*)(int32_t (*)(int32_t))` is ordinary C.
    fn resolve_fn_type(&self, ty: &ast::TypeExpr, params: &[ast::FnTypeParam]) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                message: format!(
                    "'&' cannot be written on the function type '{}' - a function value is Copy (it is a plain function pointer), so a borrowed one would be a pointer to a pointer; write the type without '&'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            // `scoped` is a parameter modifier of a *declaration*, never of a
            // parameter inside a function type: such a value could be stored
            // as the function type, which is the one thing it forbids.
            if p.ty.scoped {
                return Err(TypeError {
                    message: format!(
                        "'scoped {}' cannot be a parameter of a function type - a function type is a value that can be stored and passed on, and a scoped function value may only live for one call; write it as a top-level parameter of a declaration instead",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    message: format!(
                        "a function type cannot return a borrow ('&{}') - the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a function *type* is called through a pointer where there is none; return an owned value instead",
                        r.display()
                    ),
                    span: r.span,
                });
            }
        }
        let ret = match &ty.fn_ret {
            Some(r) if r.is_fn() => {
                return Err(TypeError {
                    message: format!(
                        "a function type cannot return another function type ('{}') - C cannot write that declarator, and nothing needs it; return a 'datac' holding the function value instead",
                        ty.display()
                    ),
                    span: r.span,
                })
            }
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::Fn { params: resolved, ret: Box::new(ret) })
    }

    fn resolve_closure_type(&self, ty: &ast::TypeExpr, params: &[ast::FnTypeParam]) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                message: format!(
                    "'&' cannot be written on the closure type '{}' - a closure is an owned value, and a parameter of that type already borrows it; write the type without '&', or 'move {}' to take ownership",
                    ty.display(),
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            if p.ty.scoped {
                return Err(TypeError {
                    message: format!(
                        "'scoped {}' cannot be a parameter of a closure type - a closure type is a value that can be stored and passed on, and a scoped function value may only live for one call; write it as a top-level parameter of a declaration instead",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    message: format!(
                        "a closure type cannot return a borrow ('&{}') - the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a closure is called through a value; return an owned value instead",
                        r.display()
                    ),
                    span: r.span,
                });
            }
        }
        let ret = match &ty.fn_ret {
            Some(r) if r.is_fn() || r.is_closure() => {
                return Err(TypeError {
                    message: format!(
                        "a closure type cannot return a function type ('{}') - C cannot write that declarator, and a closure's result is a value rather than a call target; return a 'datac' holding the function value instead",
                        ty.display()
                    ),
                    span: r.span,
                })
            }
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::Closure { params: resolved, ret: Box::new(ret) })
    }

    fn resolve_receiver(&self, r: &ast::Receiver) -> Result<Ty> {
        // A receiver names a *type*, so a generic type needs its arguments
        // - and there is no spelling for them on a receiver yet. Say that
        // rather than let the ordinary "needs N type arguments" message
        // arrive with no explanation of where they would go (A-2).
        //
        // The name may already be canonical (`std.io.File`, rewritten by
        // `resolve.rs`) or still be written with an alias, so both spellings
        // are tried; a name that resolves through neither is left to
        // `resolve` below, which reports it properly.
        if self.env.generics.contains_key(&r.type_name)
            || self
                .qualify(&r.type_name, r.span)
                .map(|q| self.env.generics.contains_key(&q))
                .unwrap_or(false)
        {
            return Err(TypeError {
                message: format!(
                    "'{}' is a generic type - the receiver has to name its type parameters, and they have to be the type's own, in order ('fun {}<{}>.method(...)')",
                    r.type_name,
                    r.type_name,
                    self.env
                        .generics
                        .get(&r.type_name)
                        .map(|t| t.params.join(", "))
                        .unwrap_or_default()
                ),
                span: r.span,
            });
        }
        let te = ast::TypeExpr::named(r.type_name.clone(), r.span);
        Ok(implicit_borrow_wrap(self.resolve(&te, false)?, r.mutable))
    }

    /// A declared return type. This is the one place a borrow *can* be
    /// written down: `&str`/`&T` in a return position becomes a `Ty::Ref`
    /// under the elision rule, meaning "a borrow of the first reference
    /// parameter" (see `borrowck.rs`, which enforces exactly that).
    fn resolve_return(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        // A borrowed trait object may not be returned (A-12 slice 4): a return
        // would have to name the lifetime of what it borrows, and v1 has no
        // lifetime syntax, exactly as for a `Ty::Ref` return.
        if ty.dyn_trait.is_some() {
            return Err(TypeError {
                message: format!(
                    "'{}' cannot be a return type: a borrowed trait object points at a value the caller owns, and a return would have to name how long that borrow lives; take it as a parameter, or return an owned value",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let base = self.resolve(ty, false)?;
        if ty.amp {
            Ok(Ty::Ref {
                mutable: false,
                inner: Box::new(base),
            })
        } else {
            Ok(base)
        }
    }
}


/// The user types that declare their own `Map` key (A-12): the receiver name of
/// every `fun T.hash()` declaration in the program.
///
/// `hash` is the half that makes the type a key, which is what makes a declared
/// key possible with no trait and no new syntax: the pair is the two method
/// names. The signature loop in `check_program_pass` then insists the
/// declaration is one the map can actually call, so a `hash` with no `eq` or a
/// malformed one is an error at the declaration rather than a `Map` use site
/// that can only say the type is not a key. A lone `eq` (H-5) is deliberately
/// *not* here: it gives the type `==`/`!=` without making it a key.
fn declared_key_names(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for f in &program.functions {
        if let Some(r) = &f.receiver {
            if f.name == crate::types::KEY_PAIR_HASH_METHOD {
                out.insert(r.type_name.clone());
            }
        }
    }
    out
}

/// The user types that declare their own destructor (kernel stage 9,
/// MK9-S6-M1): the receiver name of every `fun mut T.drop()` in the program.
///
/// Like a declared key pair, the method name *is* the declaration and there is
/// no trait and no new syntax; this is the one set both backends and
/// `types::owned_types` read to know which types get a user call at the top of
/// their generated `kelp_drop_<T>`.
fn declared_drop_names(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for f in &program.functions {
        if let Some(r) = &f.receiver {
            if f.name == crate::types::DROP_METHOD {
                out.insert(r.type_name.clone());
            }
        }
    }
    out
}

/// The message for two methods that share one `Type#name` key where at least
/// one of them came from an `impl` block (A-12 slice 2): a trait method and an
/// explicit method of the type, or two traits' methods, cannot both answer a
/// call of that name. `None` when neither declaration is a trait impl, which
/// leaves the ordinary "defined more than once" message to the caller.
fn trait_method_collision_message(
    a: &ast::FunctionDecl,
    b: Option<&ast::FunctionDecl>,
) -> Option<String> {
    let site_a = a.trait_impl.as_ref();
    let site_b = b.and_then(|o| o.trait_impl.as_ref());
    if site_a.is_none() && site_b.is_none() {
        return None;
    }
    let owner = a
        .receiver
        .as_ref()
        .map(|r| r.type_name.clone())
        .unwrap_or_default();
    let method = &a.name;
    let message = match (site_a, site_b) {
        (Some(a), Some(b)) => format!(
            "traits '{}' and '{}' both declare '{}.{method}': one type cannot answer a call of that name through two traits, so rename the method in one of them",
            a.trait_name, b.trait_name, owner
        ),
        (Some(a), None) => format!(
            "'impl {} for {}' declares '{}.{method}', which '{}' already has as an explicit method: a trait method and an explicit method cannot share a name, so rename one of them",
            a.trait_name, a.self_type, owner, owner
        ),
        (None, Some(b)) => format!(
            "'impl {} for {}' declares '{}.{method}', which '{}' already has as an explicit method: a trait method and an explicit method cannot share a name, so rename one of them",
            b.trait_name, b.self_type, owner, owner
        ),
        (None, None) => return None,
    };
    Some(message)
}

/// Whether one declared destructor is the shape the compiler calls: an
/// exclusive (`mut`) receiver, no parameters, and no return value. Checked at
/// the declaration, where the receiver, the parameters and the return type are
/// already resolved, so a malformed destructor is an error at its own line
/// rather than a call the generated drop cannot make.
fn check_drop_method(
    f: &ast::FunctionDecl,
    ret: &Ty,
) -> std::result::Result<(), Diagnostic> {
    let receiver = f
        .receiver
        .as_ref()
        .expect("only a receiver method is a destructor");
    let owner = &receiver.type_name;
    let err = |message: String| Diagnostic::new(message, f.span, f.module.clone());
    if !receiver.mutable {
        return Err(err(format!(
            "'{owner}.drop' must take its receiver with 'mut': the drop body reads and may adjust the value it is running for, so write 'fun mut {owner}.drop()'"
        )));
    }
    if !f.params.is_empty() {
        return Err(err(format!(
            "'{owner}.drop' takes no parameters: the compiler calls it with the value being dropped, so write 'fun mut {owner}.drop()'"
        )));
    }
    if *ret != Ty::Unit {
        return Err(err(format!(
            "'{owner}.drop' must not return a value: a drop runs where the value dies and has nowhere to hand a result, so write 'fun mut {owner}.drop()'"
        )));
    }
    Ok(())
}

/// Every `trait Name { ... }` in the program, keyed by qualified name (A-12
/// slice 2). The signatures are kept as written, because `Self` is only bound
/// when a type implements the trait.
///
/// The shape checks that need no resolved type are here: a method name is
/// declared once, and `drop` is not a trait method at all (a destructor is a
/// property of the type, written `fun mut T.drop()` in its own module).
fn collect_traits(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<HashMap<String, TraitInfo>, Diagnostic> {
    let mut out: HashMap<String, TraitInfo> = HashMap::new();
    for t in &program.traits {
        let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for m in &t.methods {
            if m.name == crate::types::DROP_METHOD {
                return Err(Diagnostic::new(
                    format!(
                        "a trait method cannot be named 'drop': a destructor belongs to the type and is written 'fun mut {}.drop()' in the type's own module, not through a trait",
                        m.receiver.as_ref().map(|r| r.type_name.as_str()).unwrap_or("T")
                    ),
                    m.span,
                    t.module.clone(),
                ));
            }
            if m.variadic {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' method '{}' writes '...', which only a user 'extern fun' may write: a trait signature declares the parameters an impl has to take",
                        t.name, m.name
                    ),
                    m.span,
                    t.module.clone(),
                ));
            }
            if !names.insert(&m.name) {
                return Err(Diagnostic::new(
                    format!("trait '{}' declares method '{}' more than once", t.name, m.name),
                    m.span,
                    t.module.clone(),
                ));
            }
        }
        if !t.type_params.is_empty() && !t.assoc_types.is_empty() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' declares both type parameters and an associated type, which is not in this slice: a generic trait has no single associated-type binding to name",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
        if out.contains_key(&t.name) {
            return Err(Diagnostic::new(
                format!("trait '{}' is defined more than once", t.name),
                t.span,
                t.module.clone(),
            ));
        }
        out.insert(
            t.name.clone(),
            TraitInfo {
                name: t.name.clone(),
                type_params: t.type_params.clone(),
                type_param_bounds: Vec::new(),
                prefix: project.prefix_of(&t.module).to_string(),
                aliases: project.alias_prefixes_of(&t.module),
                methods: t.methods.clone(),
                supertraits: Vec::new(),
                assoc_types: Vec::new(),
                module: t.module.clone(),
            },
        );
    }
    // A-12 slice 3: a supertrait is resolved in the trait's own module, once
    // every trait is known, so `trait B: A` may name an `A` declared later in
    // load order. It has to name a declared trait, and the relation has to be
    // acyclic.
    for t in &program.traits {
        let mut canonical = Vec::new();
        // A-12 slice 5: the associated types and their bounds, resolved in the
        // trait's own module exactly as a supertrait is.
        let mut assoc: Vec<AssocTypeInfo> = Vec::new();
        for a in &t.assoc_types {
            let mut bounds = Vec::new();
            for b in &a.bounds {
                let key = match qualify_written_name(
                    &b.name,
                    project.prefix_of(&t.module),
                    &project.alias_prefixes_of(&t.module),
                ) {
                    Some(key) => key,
                    None => {
                        return Err(Diagnostic::new(
                            format!(
                                "'{}' names no module alias, so the bound on 'type {}' of '{}' cannot be resolved",
                                b.name, a.name, t.name
                            ),
                            b.span,
                            t.module.clone(),
                        ))
                    }
                };
                if !out.contains_key(&key) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a trait, so it cannot bound 'type {}' of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                            b.name, a.name, t.name, b.name
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                bounds.push((b.name.clone(), key));
            }
            assoc.push(AssocTypeInfo {
                name: a.name.clone(),
                bounds,
            });
        }
        for s in &t.supertraits {
            // A supertrait of a generic trait may not be written with
            // arguments in this slice (`trait A<T>: B<T>`): the substitution a
            // parameterized supertrait needs is a follow-up, and a plain
            // supertrait works for both a generic and an ordinary trait.
            if !s.args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "a supertrait with arguments ('{}') is not in this slice: write the supertrait without arguments ('{}'), and bound the parameter where it is used",
                        s.display(),
                        s.name
                    ),
                    s.span,
                    t.module.clone(),
                ));
            }
            let key = match qualify_written_name(
                &s.name,
                project.prefix_of(&t.module),
                &project.alias_prefixes_of(&t.module),
            ) {
                Some(key) => key,
                None => {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' names no module alias, so the supertrait of '{}' cannot be resolved",
                            s.name, t.name
                        ),
                        s.span,
                        t.module.clone(),
                    ))
                }
            };
            if !out.contains_key(&key) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not a trait, so it cannot be a supertrait of '{}': a supertrait names a declared trait ('trait {} {{ ... }}')",
                        s.name, t.name, s.name
                    ),
                    s.span,
                    t.module.clone(),
                ));
            }
            if !canonical.contains(&key) {
                canonical.push(key);
            }
        }
        // A bound on the trait's own type parameter (`trait Store<K, V: Named>`)
        // is resolved in the trait's module exactly as a supertrait is, and
        // checked against the impl's argument below.
        let mut param_bounds: Vec<Vec<(String, String)>> = Vec::new();
        for bounds in &t.type_param_bounds {
            let mut resolved = Vec::new();
            for b in bounds {
                if !b.bound_eq.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "an equality constraint on a generic trait's parameter ('{}') is not in this slice: bound the parameter with a plain trait name",
                            b.display()
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                if !b.args.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "a bound with arguments ('{}') is not in this slice: a generic trait's parameter is bounded by a plain trait name",
                            b.display()
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                let key = match qualify_written_name(
                    &b.name,
                    project.prefix_of(&t.module),
                    &project.alias_prefixes_of(&t.module),
                ) {
                    Some(key) => key,
                    None => {
                        return Err(Diagnostic::new(
                            format!(
                                "'{}' names no module alias, so the bound on '{}' of '{}' cannot be resolved",
                                b.name, t.name, t.name
                            ),
                            b.span,
                            t.module.clone(),
                        ))
                    }
                };
                if !out.contains_key(&key) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a trait, so it cannot bound a parameter of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                            b.name, t.name, b.name
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                resolved.push((b.name.clone(), key));
            }
            param_bounds.push(resolved);
        }
        if let Some(info) = out.get_mut(&t.name) {
            info.supertraits = canonical;
            info.assoc_types = assoc;
            info.type_param_bounds = param_bounds;
        }
    }
    if let Some(cycle) = supertrait_cycle(&out) {
        let name = &cycle[0];
        let span = program
            .traits
            .iter()
            .find(|t| &t.name == name)
            .map(|t| t.span)
            .unwrap_or_else(|| program.traits[0].span);
        let module = program
            .traits
            .iter()
            .find(|t| &t.name == name)
            .map(|t| t.module.clone())
            .unwrap_or_default();
        return Err(Diagnostic::new(
            format!(
                "the supertrait relation is cyclic: {}: a trait cannot inherit from itself",
                cycle.join(" inherits from ")
            ),
            span,
            module,
        ));
    }
    Ok(out)
}

/// Whether the supertrait relation has a cycle, returning one cycle (its names,
/// first repeated once) so the error can name every participant. The relation
/// has to be acyclic or the transitive bound check would not terminate.
fn supertrait_cycle(traits: &HashMap<String, TraitInfo>) -> Option<Vec<String>> {
    fn dfs(
        name: &str,
        traits: &HashMap<String, TraitInfo>,
        state: &mut HashMap<String, u8>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        state.insert(name.to_string(), 1);
        path.push(name.to_string());
        for sup in &traits[name].supertraits {
            match state.get(sup).copied().unwrap_or(0) {
                1 => {
                    let start = path.iter().position(|p| p == sup).unwrap_or(0);
                    let mut cycle: Vec<String> = path[start..].to_vec();
                    cycle.push(sup.clone());
                    return Some(cycle);
                }
                0 => {
                    if let Some(c) = dfs(sup, traits, state, path) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        state.insert(name.to_string(), 2);
        None
    }
    let mut state: HashMap<String, u8> = HashMap::new();
    for name in traits.keys() {
        if state.get(name).copied().unwrap_or(0) == 0 {
            let mut path = Vec::new();
            if let Some(c) = dfs(name, traits, &mut state, &mut path) {
                return Some(c);
            }
        }
    }
    None
}

/// The methods of `name` and its supertraits, supertraits first, recursively in
/// declaration order, then the trait's own methods in source order (A-12 slice 4).
/// This is the vtable's slot order and the order a method call through a `&dyn`
/// resolves in, computed the same way at both sites so the two cannot drift.
fn flattened_trait_methods<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    name: &str,
    out: &mut Vec<&'a ast::FunctionDecl>,
) {
    let Some(info) = traits.get(name) else {
        return;
    };
    for sup in &info.supertraits {
        flattened_trait_methods(traits, sup, out);
    }
    for m in &info.methods {
        out.push(m);
    }
}

/// Whether a written type mentions `Self` anywhere. The object-safety check
/// uses this: a method that returns `Self`, or takes another `Self`, cannot be
/// dispatched through a trait object because the concrete type is not known at
/// the call.
fn type_mentions_self(ty: &ast::TypeExpr) -> bool {
    if ty.name == "Self" {
        return true;
    }
    if ty
        .tuple
        .as_ref()
        .is_some_and(|elems| elems.iter().any(type_mentions_self))
    {
        return true;
    }
    if ty.args.iter().any(type_mentions_self) {
        return true;
    }
    if ty
        .array_elem
        .as_ref()
        .is_some_and(|elem| type_mentions_self(elem))
    {
        return true;
    }
    if let Some(params) = &ty.fn_params {
        if params.iter().any(|p| type_mentions_self(&p.ty)) {
            return true;
        }
    }
    if ty
        .fn_ret
        .as_ref()
        .is_some_and(|ret| type_mentions_self(ret))
    {
        return true;
    }
    false
}

/// `name` and every supertrait reachable from it, in source order, without
/// repeats. A projection through a subtrait names a supertrait's associated
/// type, exactly as a bound on the subtrait satisfies a bound on the supertrait
/// (A-12 slices 3 and 5).
fn trait_keys_with_supertraits(env: &TypeEnv, name: &str) -> Vec<String> {
    fn go(env: &TypeEnv, name: &str, out: &mut Vec<String>) {
        if out.iter().any(|k| k == name) {
            return;
        }
        out.push(name.to_string());
        if let Some(info) = env.traits.get(name) {
            for sup in &info.supertraits {
                go(env, sup, out);
            }
        }
    }
    let mut out = Vec::new();
    go(env, name, &mut out);
    out
}

/// The first associated type `name` or one of its supertraits declares, if any
/// (A-12 slice 5). A trait that has one cannot be a trait object.
fn first_assoc_type<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    name: &str,
) -> Option<&'a str> {
    let info = traits.get(name)?;
    if let Some(a) = info.assoc_types.first() {
        return Some(a.name.as_str());
    }
    for sup in &info.supertraits {
        if let Some(found) = first_assoc_type(traits, sup) {
            return Some(found);
        }
    }
    None
}

/// Object safety for a trait object, `&dyn Name` (A-12 slice 4,
/// `plan/traits.md` 9.2) or `Box<dyn Name>` (slice 7): no method may return
/// `Self`, no parameter other than the receiver may mention `Self`, and no two
/// methods of the flattened list may share a name (the vtable holds one slot per
/// name). A violation names the spelling the program wrote, the trait and the
/// method, because the same rule now guards two type spellings.
fn check_dyn_object_safety(
    spelling: &str,
    name: &str,
    span: Span,
    traits: &HashMap<String, TraitInfo>,
) -> Result<()> {
    // A-12 slice 6: a generic trait has no object-safe form, because the
    // vtable has no slot for a type parameter. Refused by name here, where the
    // `dyn` type is written.
    if let Some(info) = traits.get(name) {
        if !info.type_params.is_empty() {
            return Err(TypeError {
                message: format!(
                    "'{}' is not allowed: the trait takes type parameters ('{}'), and a trait object has no vtable slot for one; use a bounded type parameter ('<T: {}<...>>') instead",
                    spelling,
                    info.type_params.join(", "),
                    name
                ),
                span,
            });
        }
    }
    // A-12 slice 5: a trait that declares (or inherits) an associated type has
    // no object-safe form, because the erased value carries no single binding
    // to name. Refused by name here, where the `dyn` type is written.
    if let Some(item) = first_assoc_type(traits, name) {
        return Err(TypeError {
            message: format!(
                "'{}' is not allowed: the trait declares the associated type '{}', and a trait object does not know which type an implementor binds; use a bounded type parameter ('<T: {}>') instead",
                spelling, item, name
            ),
            span,
        });
    }
    let mut flat = Vec::new();
    flattened_trait_methods(traits, name, &mut flat);
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in &flat {
        // A-12 slice 6: a static method has no receiver for the erased pointer
        // to stand for, so it has no vtable slot.
        if m.receiver.is_none() {
            return Err(TypeError {
                message: format!(
                    "'{}' is not allowed: method '{}' has no receiver ('Self'), so there is no value for the vtable to dispatch on; call it on a concrete type instead",
                    spelling, m.name
                ),
                span,
            });
        }
        if let Some(ret) = &m.return_type {
            if type_mentions_self(ret) {
                return Err(TypeError {
                    message: format!(
                        "'{}' is not allowed: method '{}' returns 'Self', and a trait object does not know the concrete type it would build",
                        spelling, m.name
                    ),
                    span,
                });
            }
        }
        for p in &m.params {
            if type_mentions_self(&p.ty) {
                return Err(TypeError {
                    message: format!(
                        "'{}' is not allowed: method '{}' takes a 'Self' parameter, which a trait object cannot pass (the vtable erases the concrete type); make the parameter a borrow of a concrete type, or use a generic function instead",
                        spelling, m.name
                    ),
                    span,
                });
            }
        }
        if !seen.insert(m.name.as_str()) {
            return Err(TypeError {
                message: format!(
                    "'{}' is not allowed: two methods are named '{}', and a vtable holds one slot per name",
                    spelling, m.name
                ),
                span,
            });
        }
    }
    Ok(())
}

/// Every `impl Name for Type { ... }` in the program (A-12 slice 2), with both
/// names already canonical: `resolve.rs` qualified them where they were
/// written. The trait has to exist and the type has to be declared; those two
/// answers, plus coherence and the module rule, are checked by
/// `check_trait_impl_names`.
fn collect_trait_impls(program: &ast::Program) -> Vec<TraitImplInfo> {
    program
        .trait_impls
        .iter()
        .map(|i| TraitImplInfo {
            trait_name: i.trait_name.clone(),
            trait_instance: i.trait_instance.clone(),
            trait_arg_names: i.trait_arg_names.clone(),
            type_name: i.type_name.clone(),
            methods: i.methods.clone(),
            assoc_types: i
                .assoc_types
                .iter()
                .map(|a| (a.name.clone(), a.ty.clone(), a.span))
                .collect(),
            module: i.module.clone(),
            impl_span: i.impl_span,
            type_params: i.type_params.clone(),
            type_param_bounds: i.type_param_bounds.clone(),
            projection_bounds: i.projection_bounds.clone(),
        })
        .collect()
}

/// A written name as its canonical key, the way `TypeScope::qualify` does it:
/// a bare name belongs to the current module, and `alias.Item` resolves through
/// that module's `impl` table. `None` when the name's first segment is not an
/// imported alias.
fn qualify_written_name(
    name: &str,
    prefix: &str,
    aliases: &HashMap<String, String>,
) -> Option<String> {
    match name.split_once('.') {
        Some((alias, item)) => aliases.get(alias).map(|p| format!("{p}{item}")),
        None => Some(format!("{prefix}{name}")),
    }
}

/// One declaration's written signature, for a diagnostic: `fun mut
/// Self.scale(f: f64)`. A trait's signature is what an impl has to match, so
/// the error quotes it back the way it was written.
fn render_fn_decl(f: &ast::FunctionDecl) -> String {
    let mut out = String::from("fun ");
    if let Some(r) = &f.receiver {
        if r.mutable {
            out.push_str("mut ");
        }
        out.push_str(&r.type_name);
        if !r.type_args.is_empty() {
            out.push_str(&format!("<{}>", r.type_args.join(", ")));
        }
        out.push('.');
    }
    out.push_str(&f.name);
    out.push('(');
    let parts: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}{}: {}", p.mode.keyword(), p.name, p.ty.display()))
        .collect();
    out.push_str(&parts.join(", "));
    out.push(')');
    if let Some(t) = &f.return_type {
        out.push_str(&format!(".return: {}", t.display()));
    }
    out
}

/// Resolves one trait method signature with `Self` bound to the implementing
/// type: the receiver, each parameter and the return, through the same
/// resolution a declaration's own signature goes through, so a trait signature
/// and its impl mean exactly the same types.
fn resolve_trait_method_sig(
    trait_info: &TraitInfo,
    m: &ast::FunctionDecl,
    self_ty: &Ty,
    trait_args: &[Ty],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
) -> Result<FnSig> {
    let mut params_map: HashMap<String, Ty> = HashMap::new();
    params_map.insert("Self".to_string(), self_ty.clone());
    // A-12 slice 6: the trait's own parameters, bound to the impl's concrete
    // arguments, so `fun from(value: T).return: Self` resolves for `From<IoErr>` as
    // `fn(IoErr) -> AppErr`.
    for (param, arg) in trait_info.type_params.iter().zip(trait_args.iter()) {
        params_map.insert(param.clone(), arg.clone());
    }
    // A-12 slice 5: the associated types the implementing type binds, so
    // `Self::Item` in the trait's own signature resolves to the concrete type.
    if let Some(name) = struct_name_of(self_ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(self_ty).map(|s| s.to_string()))
    {
        for key in trait_keys_with_supertraits(env, &trait_info.name) {
            if let Some(bindings) = env.assoc.get(&(key, name.clone())) {
                for (item, ty) in bindings {
                    params_map
                        .entry(format!("Self::{item}"))
                        .or_insert_with(|| ty.clone());
                }
            }
        }
    }
    let scope = TypeScope {
        structs,
        enums,
        env: env.clone(),
        params: &params_map,
        prefix: &trait_info.prefix,
        aliases: &trait_info.aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(consts),
        const_fns: Some(const_fns),
        handles: &env.handles,
    };
    let receiver = m.receiver.as_ref();
    let mut params = Vec::new();
    if let Some(receiver) = receiver {
        params.push(("self".to_string(), scope.resolve_receiver(receiver)?));
    }
    for p in &m.params {
        params.push((p.name.clone(), scope.resolve_param(p)?));
    }
    let ret = match &m.return_type {
        Some(t) => scope.resolve_return(t)?,
        None => Ty::Unit,
    };
    Ok(FnSig { params, ret })
}

/// Coherence (MISSING-Ve8Nc step 4): two impls of one trait that could both
/// apply to the same type are an error at the later one, with the earlier one
/// named as a note. Two impls overlap when their type patterns unify, whatever
/// their bounds: a blanket impl overlaps every other impl of its trait, and two
/// impls of one generic type overlap. There is no specialization, so no bound
/// is used to tell two impls apart. Two concrete impls for one type keep their
/// own message in `check_trait_impl_names`.
fn check_trait_impl_overlap(
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
) -> std::result::Result<(), Diagnostic> {
    let is_blanket = |i: &TraitImplInfo| i.type_params.contains(&i.type_name);
    let display = |i: &TraitImplInfo| {
        if i.type_params.is_empty() || is_blanket(i) {
            i.type_name.clone()
        } else {
            format!("{}<{}>", i.type_name, i.type_params.join(", "))
        }
    };
    for (j, imp) in impls.iter().enumerate() {
        for earlier in &impls[..j] {
            if earlier.trait_instance != imp.trait_instance {
                continue;
            }
            let blanket = is_blanket(imp) || is_blanket(earlier);
            let same_generic = !imp.type_params.is_empty()
                && !earlier.type_params.is_empty()
                && imp.type_name == earlier.type_name;
            if !blanket && !same_generic {
                continue;
            }
            let trait_name = traits
                .get(&imp.trait_name)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| imp.trait_name.clone());
            let message = if blanket {
                format!(
                    "'impl {} for {}' overlaps 'impl {} for {}': a blanket impl applies to every type that meets its bound, so it overlaps every other impl of '{}', and there is no specialization to choose between them: a blanket impl has to be the trait's only impl",
                    trait_name,
                    display(imp),
                    trait_name,
                    display(earlier),
                    trait_name
                )
            } else {
                format!(
                    "'impl {} for {}' overlaps 'impl {} for {}': both apply to every '{}', so one type would have two impls of '{}', and there is no specialization to choose between them",
                    trait_name,
                    display(imp),
                    trait_name,
                    display(earlier),
                    imp.type_name,
                    trait_name
                )
            };
            let mut err = Diagnostic::new(message, imp.impl_span, imp.module.clone());
            err.notes.push(crate::diag::DiagnosticNote {
                message: format!("the overlapping impl of '{trait_name}' is written here"),
                span: earlier.impl_span,
                module: earlier.module.clone(),
            });
            return Err(err);
        }
    }
    Ok(())
}

/// The name-level checks on every `impl Name for Type { ... }` block (A-12
/// slice 2): the trait exists, the type is a declared `datac`/`enum`, the impl
/// is in one of the two modules, and no second impl of the same trait for the
/// same type exists.
///
/// Runs before the signature loop, so a second impl is reported as the
/// coherence error it is rather than as the duplicate method the two blocks
/// happen to share.
fn check_trait_impl_names(
    program: &ast::Program,
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    env: &TypeEnv,
) -> std::result::Result<(), Diagnostic> {
    // The module each declared type belongs to, for the "the impl lives in the
    // trait's module or the type's" rule.
    let mut type_modules: HashMap<&str, &str> = HashMap::new();
    for d in &program.datacs {
        type_modules.insert(&d.name, &d.module);
    }
    for e in &program.enums {
        type_modules.insert(&e.name, &e.module);
    }
    // The (trait, type) pairs declared anywhere in the program, so a
    // supertrait requirement (A-12 slice 3) is answered once every impl has
    // been seen rather than in source order.
    let declared: std::collections::HashSet<(&str, &str)> = impls
        .iter()
        .map(|i| (i.trait_instance.as_str(), i.type_name.as_str()))
        .collect();
    let mut seen: HashMap<(&str, &str), &TraitImplInfo> = HashMap::new();
    for imp in impls {
        let Some(info) = traits.get(&imp.trait_name) else {
            return Err(Diagnostic::new(
                format!(
                    "'impl {} for {}' names no declared trait: declare 'trait {} {{ ... }}' first (or import the module that does)",
                    imp.trait_name, imp.type_name, imp.trait_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        // A-12 slice 6: the impl names the trait instance, so it has to supply
        // exactly the arguments the trait declares, and each one has to satisfy
        // the bound the trait writes on that parameter.
        if info.type_params.is_empty() && !imp.trait_arg_names.is_empty() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' takes no type arguments, and 'impl {} for {}' writes {}: write 'impl {} for {}' (an operator trait's right-hand side is always '&Self', so there is no 'Add<Other>')",
                    info.name,
                    imp.trait_name,
                    imp.type_name,
                    imp.trait_arg_names.len(),
                    imp.trait_name,
                    imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        if imp.trait_arg_names.len() != info.type_params.len() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' takes {} type argument(s), and 'impl {} for {}' writes {}: write 'impl {}<{}> for {}' to name the instance",
                    info.name,
                    info.type_params.len(),
                    imp.trait_name,
                    imp.type_name,
                    imp.trait_arg_names.len(),
                    imp.trait_name,
                    info.type_params.join(", "),
                    imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        for (i, bounds) in info.type_param_bounds.iter().enumerate() {
            let Some(arg) = imp.trait_arg_names.get(i) else {
                continue;
            };
            for (written, key) in bounds {
                let name = Ty::from_name(arg)
                    .and_then(|t| builtin_type_name(&t).map(|s| s.to_string()))
                    .unwrap_or_else(|| arg.clone());
                if !implements_trait(env, key, &name) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' does not satisfy the bound '{written}' on parameter '{}' of '{}': write 'impl {written} for {arg} {{ ... }}' in the trait's module or in the type's",
                            arg, info.type_params[i], info.name
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
        }
        // A builtin `for` type (A-12 slice 3) has no module of its own, so the
        // impl must sit in the trait's module (the orphan rule); a declared
        // `datac`/`enum` may be implemented in either owner's module.
        let type_module: &str = if imp.type_params.contains(&imp.type_name) {
            // A blanket impl names no type of its own, so the orphan rule leaves
            // it the trait's module alone.
            if imp.module != info.module {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is written in module '{}', but a blanket impl names no type of its own, so it lives in the trait's module ('{}'): write it there",
                        imp.trait_name, imp.type_name, imp.module, info.module
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            info.module.as_str()
        } else if let Some(_builtin) = builtin_for_type_name(&imp.type_name) {
            if imp.module != info.module {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is written in module '{}', but '{}' is a builtin type, which has no module: implement a trait for a builtin in the trait's own module ('{}')",
                        imp.trait_name, imp.type_name, imp.module, imp.type_name, info.module
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            info.module.as_str()
        } else if let Some(module) = type_modules.get(imp.type_name.as_str()).copied() {
            module
        } else if crate::types::Ty::from_name(&imp.type_name).is_some() {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is a builtin type that carries no methods, so 'impl {} for {}' has nothing to add: implement the trait for a 'datac'/'enum', or for a builtin whose methods the compiler already provides ('i32', 'String', ...)",
                    imp.type_name, imp.trait_name, imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        } else {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is not a declared 'datac' or 'enum', so 'impl {} for {}' has nothing to implement",
                    imp.type_name, imp.trait_name, imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        if imp.module != info.module && imp.module != type_module {
            return Err(Diagnostic::new(
                format!(
                    "'impl {} for {}' is written in module '{}', which owns neither: an impl lives in the trait's module ('{}') or in the type's ('{}')",
                    imp.trait_name, imp.type_name, imp.module, info.module, type_module
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        // Reaching across a module boundary is the ordinary visibility rule
        // (A-13): a trait or a type an impl names from another module has to be
        // exported by the module that declares it.
        for (name, owner, what) in [
            (&imp.trait_name, info.module.as_str(), "trait"),
            (&imp.type_name, type_module, "type"),
        ] {
            if owner != imp.module && !exported(name, env) {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' names the {what} '{name}', which module '{owner}' does not export: add 'pub' to its declaration, or write the impl in '{owner}'",
                        imp.trait_name, imp.type_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
        // A-12 slice 3: implementing a trait means implementing its
        // supertraits too, because a bound on the supertrait is what the
        // callers of the subtrait's methods rely on.
        for sup in &info.supertraits {
            if !declared.contains(&(sup.as_str(), imp.type_name.as_str())) {
                let display = traits.get(sup).map(|t| t.name.as_str()).unwrap_or(sup);
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is missing its supertrait: '{}' needs 'impl {} for {}' as well, because trait '{}' inherits from '{}'",
                        imp.trait_name, imp.type_name, imp.trait_name, display, imp.type_name, info.name, display
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
        if let Some(first) = seen.insert((&imp.trait_instance, &imp.type_name), imp) {
            let mut err = Diagnostic::new(
                format!(
                    "trait '{}' already has an impl for '{}' (written in module '{}'): a trait is implemented for a type at most once",
                    info.name, imp.type_name, first.module
                ),
                imp.impl_span,
                imp.module.clone(),
            );
            err.notes.push(crate::diag::DiagnosticNote {
                message: format!("the other impl of '{}' for '{}' is written here", info.name, first.type_name),
                span: first.impl_span,
                module: first.module.clone(),
            });
            return Err(err);
        }
    }
    Ok(())
}

/// The concrete type each `impl` binds for every associated type its trait
/// declares (A-12 slice 5), with the shape checks. The binding is resolved in
/// the impl's own module, exactly as an impl method's signature is, and each
/// bound the trait declares is checked against the bound type here, so a
/// generic caller can rely on it.
#[allow(clippy::too_many_arguments)]
fn collect_assoc_bindings(
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    project: &Project,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
) -> std::result::Result<HashMap<(String, String), HashMap<String, Ty>>, Diagnostic> {
    let mut out: HashMap<(String, String), HashMap<String, Ty>> = HashMap::new();
    for imp in impls {
        let info = &traits[&imp.trait_name];
        if !imp.type_params.is_empty() {
            // A generic impl binds its associated types over its parameters, so
            // they are resolved per instance (`resolve_impl_assoc`); here only
            // that every declared one is bound.
            for declared in &info.assoc_types {
                if !imp.assoc_types.iter().any(|(name, _, _)| *name == declared.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "trait '{}' declares 'type {}', and 'impl {} for {}' does not bind it: add 'type {} = <type>' to the block",
                            info.name, declared.name, imp.trait_name, imp.type_name, declared.name
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
            continue;
        }
        let prefix = project.prefix_of(&imp.module);
        let aliases = project.alias_prefixes_of(&imp.module);
        let scope = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(consts),
            const_fns: Some(const_fns),
            handles: &env.handles,
        };
        let mut bound: HashMap<String, Ty> = HashMap::new();
        for declared in &info.assoc_types {
            let Some((_, written, span)) =
                imp.assoc_types.iter().find(|(name, _, _)| *name == declared.name)
            else {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' declares 'type {}', and 'impl {} for {}' does not bind it: add 'type {} = <concrete type>' to the block",
                        info.name, declared.name, imp.trait_name, imp.type_name, declared.name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            let ty = scope
                .resolve(written, false)
                .map_err(|e| e.in_module(&imp.module))?;
            let name = struct_name_of(&ty)
                .map(|s| s.to_string())
                .or_else(|| builtin_type_name(&ty).map(|s| s.to_string()));
            for (written_bound, key) in &declared.bounds {
                let implements = name
                    .as_deref()
                    .map(|n| implements_trait(env, key, n))
                    .unwrap_or(false);
                if !implements {
                    return Err(Diagnostic::new(
                        format!(
                            "'type {} = {}' in 'impl {} for {}' does not satisfy the bound 'type {}: {}': '{}' does not implement '{}', which the trait requires of every binding",
                            declared.name,
                            ty.display(),
                            imp.trait_name,
                            imp.type_name,
                            declared.name,
                            written_bound,
                            ty.display(),
                            written_bound
                        ),
                        *span,
                        imp.module.clone(),
                    ));
                }
            }
            bound.insert(declared.name.clone(), ty);
        }
        for (name, _, span) in &imp.assoc_types {
            if !info.assoc_types.iter().any(|a| a.name == *name) {
                return Err(Diagnostic::new(
                    format!(
                        "'type {name}' is not declared by trait '{}': an 'impl' binds exactly the trait's associated types, so remove it or add 'type {name}' to the trait",
                        info.name
                    ),
                    *span,
                    imp.module.clone(),
                ));
            }
        }
        out.insert((imp.trait_instance.clone(), imp.type_name.clone()), bound);
    }
    Ok(out)
}

/// A builtin type a trait may be implemented for (A-12 slice 3): one the
/// compiler already gives methods to, which is exactly the set `builtin_type_name`
/// names. `None` for a `datac`/`enum` name, a name that resolves to nothing, or
/// a builtin with no method surface (`bool`, `char`, `f32`, `f64`, `unit`).
fn builtin_for_type_name(name: &str) -> Option<Ty> {
    let ty = crate::types::Ty::from_name(name)?;
    builtin_type_name(&ty).map(|_| ty)
}

/// Checks that every `impl Name for Type { ... }` block implements exactly the
/// signatures its trait declares (A-12 slice 2). Runs after the signature loop,
/// because the impl's methods have to be resolved before they can be compared,
/// and it is the only place that knows both resolved sides.
#[allow(clippy::too_many_arguments)]
fn check_trait_impl_signatures(
    program: &ast::Program,
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    sigs: &HashMap<String, FnSig>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    env: &TypeEnv,
) -> std::result::Result<(), Diagnostic> {
    // Key -> declaration, so a mismatch can quote the impl back the way it was
    // written rather than describing a resolved `FnSig`.
    let decl_by_key: HashMap<String, &ast::FunctionDecl> = program
        .functions
        .iter()
        .map(|f| (sig_key(f), f))
        .collect();
    for imp in impls {
        let info = &traits[&imp.trait_name];
        if !imp.type_params.is_empty() {
            // A blanket impl takes no trait default yet: every method it has
            // with a body is written in the block (the defaults are not copied).
            if imp.type_params.contains(&imp.type_name) {
                for m in &info.methods {
                    if !matches!(m.body, ast::FunctionBody::Signature) && !imp.methods.contains(&m.name) {
                        return Err(Diagnostic::new(
                            format!(
                                "trait '{}' gives '{}' a default body, and 'impl {} for {}' uses that default: a blanket impl writes every method itself (a default in a blanket impl is not supported yet)",
                                info.name, m.name, imp.trait_name, imp.type_name
                            ),
                            imp.impl_span,
                            imp.module.clone(),
                        ));
                    }
                }
            }
            // Template methods have no signature until an instance is made, so
            // a generic impl is checked where it is used; here only that it
            // writes every method the trait has no body for.
            for m in &info.methods {
                let needs_body = matches!(m.body, ast::FunctionBody::Signature);
                if needs_body && !imp.methods.contains(&m.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "trait '{}' declares '{}', and 'impl {} for {}' does not implement it: a generic impl writes every method (it cannot use the trait's default)",
                            info.name,
                            render_fn_decl(m),
                            imp.trait_name,
                            imp.type_name,
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
            continue;
        }
        let self_ty = builtin_for_type_name(&imp.type_name)
            .unwrap_or_else(|| Ty::Struct(imp.type_name.clone()));
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            let key = impl_method_key(imp, m);
            let Some(impl_sig) = sigs.get(&key) else {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' declares '{}', and 'impl {} for {}' does not implement it: add '{} {{ ... }}' to the block",
                        info.name,
                        render_fn_decl(m),
                        imp.trait_name,
                        imp.type_name,
                        render_fn_decl(m)
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            let declared = resolve_trait_method_sig(
                info,
                m,
                &self_ty,
                &trait_args,
                structs,
                enums,
                env,
                consts,
                const_fns,
            )
            .map_err(|e| e.in_module(&info.module))?;
            let impl_decl = decl_by_key.get(&key).copied();
            let receiver_mut = |f: &ast::FunctionDecl| {
                f.receiver.as_ref().map(|r| r.mutable).unwrap_or(false)
            };
            if let Some(d) = impl_decl {
                if receiver_mut(d) != receiver_mut(m) {
                    return Err(Diagnostic::new(
                        format!(
                            "method '{}' takes its receiver differently than trait '{}' declares it: the trait writes '{}', the impl writes '{}'",
                            m.name,
                            info.name,
                            render_fn_decl(m),
                            render_fn_decl(d)
                        ),
                        d.span,
                        d.module.clone(),
                    ));
                }
            }
            if declared.params != impl_sig.params || declared.ret != impl_sig.ret {
                let written = impl_decl
                    .map(render_fn_decl)
                    .unwrap_or_else(|| key.clone());
                return Err(Diagnostic::new(
                    format!(
                        "method '{}' does not match trait '{}': the trait declares '{}', the impl writes '{}'",
                        m.name,
                        info.name,
                        render_fn_decl(m),
                        written
                    ),
                    impl_decl.map(|d| d.span).unwrap_or(imp.impl_span),
                    impl_decl
                        .map(|d| d.module.clone())
                        .unwrap_or_else(|| imp.module.clone()),
                ));
            }
        }
        for name in &imp.methods {
            if !info.methods.iter().any(|m| &m.name == name) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not declared by trait '{}': an 'impl' implements exactly the trait's methods, so remove it or add the signature to the trait",
                        name, info.name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
    }
    Ok(())
}

/// One user-declared key-pair half, kept while the signatures are built so a
/// type that declares only one of the two can be named with the span of the
/// half it did declare.
struct KeyPairHalf {
    method: &'static str,
    span: Span,
    module: String,
}

/// Whether one half of a declared `Map` key pair is a shape the map can call
/// (A-12), with `params` the resolved parameter list (the receiver first) and
/// `ret` the resolved return type.
///
/// The borrow is checked as *written* as well as resolved: an owned `other: T`
/// and a `other: &T` resolve to the same `Ty::Ref` (a `datac` parameter is
/// borrowed either way), and the map passes the caller's key by reference, so a
/// parameter that reads as an ownership transfer is refused rather than
/// silently accepted.
fn check_key_pair_method(
    f: &ast::FunctionDecl,
    params: &[(String, Ty)],
    ret: &Ty,
) -> std::result::Result<(), Diagnostic> {
    let receiver = f
        .receiver
        .as_ref()
        .expect("only a receiver method is a key-pair half");
    let owner = &receiver.type_name;
    let err = |message: String| Diagnostic::new(message, f.span, f.module.clone());
    if f.name == crate::types::KEY_PAIR_HASH_METHOD {
        if !f.params.is_empty() {
            return Err(err(format!(
                "'{owner}.hash' takes no parameters: a key's hash is computed from the key alone, so write 'fun {owner}.hash().return: u64'"
            )));
        }
        if *ret != Ty::U64 {
            return Err(err(format!(
                "'{owner}.hash' must return 'u64', found '{}': the `Map` probe folds the hash of every key through 'kelp_map_combine'",
                ret.display()
            )));
        }
        return Ok(());
    }
    if f.params.len() != 1 {
        return Err(err(format!(
            "'{owner}.eq' takes exactly one parameter, a shared borrow of '{owner}', so write 'fun {owner}.eq(other: &{owner}).return: bool'"
        )));
    }
    let written = &f.params[0];
    if !written.ty.amp || written.mode != ast::ParamMode::Borrow {
        return Err(err(format!(
            "'{owner}.eq' must take 'other: &{owner}': a lookup compares the caller's key against an entry the caller does not own, so the argument cannot be moved"
        )));
    }
    let same = matches!(
        params.get(1).map(|(_, ty)| ty),
        Some(Ty::Ref { mutable: false, inner }) if **inner == Ty::Struct(owner.clone())
    );
    if !same {
        return Err(err(format!(
            "'{owner}.eq' takes a shared borrow of '{owner}', found '{}'",
            params[1].1.display()
        )));
    }
    if *ret != Ty::Bool {
        return Err(err(format!(
            "'{owner}.eq' must return 'bool', found '{}'",
            ret.display()
        )));
    }
    Ok(())
}

/// The `Ty::Fn` a declared signature has (A-4): its parameter types, in order,
/// and its return type. A function *type* spells exactly this, so a written
/// `fun(String).return: usize` and the signature of `fun f(s: String).return: usize` are
/// the same type with no conversion anywhere.
fn fn_value_ty(sig: &FnSig) -> Ty {
    Ty::Fn {
        params: sig.params.iter().map(|(_, t)| t.clone()).collect(),
        ret: Box::new(sig.ret.clone()),
    }
}

/// The canonical key and function type of a *bare* name used as a value
/// (A-4), or `None` when the name is not a function at all (so the caller can
/// report whatever else it might have been).
///
/// The lookup order is a call's: the calling module's own spelling first, then
/// the name as written. Three names that *are* functions are refused rather
/// than silently dropped, because "undefined variable" would be a lie:
/// a generic template (its signature is not concrete until a call
/// instantiates it), `main` (the entry point cannot be called), and a function
/// that reads a passdown (the hidden argument it receives is not part of a C
/// function pointer).
fn function_value(name: &str, ctx: &Ctx, span: Span) -> Result<Option<(String, Ty)>> {
    let owned = ctx.qualify_own(name);
    // A generic template has no concrete signature yet, so it is never in
    // `sigs`; its refusal is `function_value_at`'s, and the key it looks under
    // is still the calling module's own.
    if ctx.sig_of(&owned).is_some() || ctx.fns.templates.contains_key(&owned) {
        return function_value_at(&owned, name, ctx, span);
    }
    // Same rule as the signature lookup in `check_call`: only a name that
    // already carries its module prefix may fall back to the name as written,
    // because that is the only spelling of another module's key. A bare name
    // is the calling module's own, and the entry module is the one whose keys
    // are bare (A-13).
    if !name.contains('.') {
        return Ok(None);
    }
    function_value_at(name, name, ctx, span)
}

/// `function_value` for an already-canonical key, with `written` as the name
/// to show in a diagnostic (a bare name, or `math.sqrt`).
fn function_value_at(
    key: &str,
    written: &str,
    ctx: &Ctx,
    span: Span,
) -> Result<Option<(String, Ty)>> {
    if let Some(sig) = ctx.sig_of(key) {
        if key == "main" {
            return Err(TypeError {
                message: "cannot reference 'main': it is the program entry point".to_string(),
                span,
            });
        }
        if ctx.wired_for(key).is_some_and(|w| !w.is_empty()) {
            return Err(TypeError {
                message: format!(
                    "'{written}' reads a passdown, so it cannot be a function value - a passdown is passed to the callee as a hidden argument, and a function value is a plain C function pointer; call it directly instead"
                ),
                span,
            });
        }
        // A gated item named as a value is a *name* like any other, so the
        // domain half of the gate applies here exactly as it does at a call
        // site: without the capability the name does not resolve at all.
        ctx.check_capability(key, span)?;
        // Holding the domain is not enough. The pattern half of the gate is
        // emitted from the *call's* own callee key (`codegen_c.rs::cap_plan`),
        // and a call through a function value names the pointer, not the
        // gated item: the check would be skipped and the pointer would reach
        // the resource on the domain gate's say-so alone, which makes a
        // narrowed grant advisory for this one shape. A value cannot carry
        // the naming module's patterns, and the resource argument position is
        // a property of the callee's signature, so no check can follow the
        // pointer. Refuse the value, the way the domain half refuses the
        // ungranted name.
        if let Some(domain) = ctx.gated.get(key) {
            return Err(TypeError {
                message: format!(
                    "'{written}' is a gated function ('{domain}' capability), so it cannot be a function value: a call through the pointer is not a call site, and the '{domain}' check on its resource argument cannot be emitted there; call it directly instead"
                ),
                span,
            });
        }
        return Ok(Some((key.to_string(), fn_value_ty(&sig))));
    }
    if ctx.fns.templates.contains_key(key) {
        return Err(TypeError {
            message: format!(
                "'{written}' is a generic function - a function value needs one concrete signature, so wrap the call in a small non-generic function and pass that"
            ),
            span,
        });
    }
    Ok(None)
}

/// The callee *expression* and type of a name that holds a function value
/// (A-4): a local, or a passdown. `Ok(None)` when the name holds no function
/// value at all, so the caller can report it as an undefined function.
fn indirect_callee(
    name: &str,
    span: Span,
    ctx: &mut Ctx,
) -> Result<Option<(tir::Expr, Ty)>> {
    // A parameter of a closure type is a *borrow* of one (the parameter rule
    // borrows an owned type), so the callee's type may be `&closure(..)` -
    // codegen dereferences it. A `fun` is `Copy`, so it is never borrowed.
    let callable = |ty: &Ty| -> Option<Ty> {
        match ty {
            Ty::Fn { .. } => Some(ty.clone()),
            Ty::Closure { .. } => Some(ty.clone()),
            // A scoped function value is `Copy`, so a parameter of one is the
            // fat pointer itself (no borrow is added by the parameter rule).
            Ty::ScopedFn { .. } => Some(ty.clone()),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => Some(ty.clone()),
            _ => None,
        }
    };
    if let Some(info) = ctx.lookup(name) {
        if let Some(ty) = callable(&info.ty) {
            return Ok(Some((
                tir::Expr {
                    kind: tir::ExprKind::Ident(name.to_string()),
                    ty: ty.clone(),
                    span,
                },
                ty,
            )));
        }
        return Ok(None);
    }
    if ctx.in_fn_literal {
        // A closure the enclosing frame holds, called from inside this
        // literal (A-4 phase E): the literal captures it like any other
        // binding, and the call goes through the capture. The capture is
        // registered here, which is why this takes `&mut Ctx`.
        if let Some(ty) = ctx.note_capture(name) {
            // A scoped literal captures an owned closure by shared borrow, so
            // a callable capture may arrive as `&closure(...)` as well.
            let callable = matches!(ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. })
                || matches!(&ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }));
            if callable {
                let index = ctx
                    .captures
                    .iter()
                    .position(|(n, _)| n == name)
                    .expect("note_capture records the name");
                return Ok(Some((
                    tir::Expr {
                        kind: tir::ExprKind::Capture { index },
                        ty: ty.clone(),
                        span,
                    },
                    ty,
                )));
            }
        }
    }
    // A passdown holding a function value is the same thing one module up:
    // its storage is the generated global (or the hidden parameter), so the
    // callee expression is whatever a read of that passdown already is.
    if let Some(pd) = ctx.pds.get(name) {
        let Some(ty) = callable(&pd.value_ty) else {
            return Ok(None);
        };
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                message: format!(
                    "passdown '{}' is read before it is initialized - a 'pd' initializer can only use passdowns declared before it",
                    pd.qualified
                ),
                span,
            });
        }
        return Ok(Some((
            pd.value(span, ctx.pd_global_borrow, ctx.pd_exclusive(&pd)),
            ty,
        )));
    }
    Ok(None)
}

/// `f(x)` where `f` holds a function value (A-4): a local, a passdown, or a
/// `datac` field (`cfg.on_event(x)`). A function *type* has no parameter
/// names, so arguments are positional and counted against the callee's own
/// list; everything else - the borrow an owned parameter implies, the move a
/// `move` parameter implies - is the rule `check_call_arg` already applies to
/// a declared call, because the parameter types are the same types.
fn check_indirect_call_expr(
    callee: tir::Expr,
    fn_ty: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    what: &str,
) -> Result<tir::Expr> {
    // A closure parameter is a borrow of a closure, and a borrow is not a
    // value: look through it for the signature (A-4 phase E). The callee
    // expression keeps its borrowed type, which is what codegen uses to pick
    // `->` over `.`.
    let ty = match fn_ty {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    let (params, ret) = match ty {
        Ty::Fn { params, ret } => (params, ret),
        Ty::Closure { params, ret } => (params, ret),
        // A scoped function value is called exactly as a closure is: the
        // fat pointer's code member with the environment first.
        Ty::ScopedFn { params, ret } => (params, ret),
        other => unreachable!(
            "an indirect call is only reached with a function type, found '{}'",
            other.display()
        ),
    };
    if args.len() != params.len() {
        return Err(TypeError {
            message: format!(
                "a call through {what} takes {} argument(s), found {}",
                params.len(),
                args.len()
            ),
            span,
        });
    }
    for a in args {
        if a.name.is_some() {
            return Err(TypeError {
                message: format!(
                    "a call through {what} cannot use named arguments - a function type has no parameter names, so every argument is positional"
                ),
                span: a.value.span(),
            });
        }
    }
    let mut checked = Vec::with_capacity(args.len());
    for (i, (pty, a)) in params.iter().zip(args).enumerate() {
        checked.push(check_call_arg(&(i + 1).to_string(), pty, &a.value, ctx)?);
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::IndirectCall {
            callee: Box::new(callee),
            args: checked,
        },
        ty: (**ret).clone(),
        span,
    })
}

/// The type of a `datac` field read through a (possibly borrowed) receiver -
/// `None` when there is no such field. Used only to find a field holding a
/// function value, which is callable with method syntax.
fn fn_field_ty(ty: &Ty, name: &str, ctx: &Ctx) -> Option<Ty> {
    let owner = struct_name_of(ty)?.to_string();
    let def = ctx.struct_def(&owner)?;
    Some(def.field(name)?.ty.clone())
}

/// A function's lookup key in `sigs`/`tir::Function.name`: a receiver-form
/// method is keyed `Type#name` (a receiver method and a free function can
/// share a bare name with no collision, and `#` can't appear in a real
/// identifier so it can't collide with a hand-written name either) - a
/// plain function is keyed by its bare name, unchanged from before
/// receivers existed.
fn sig_key(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}#{}", r.type_name, f.name),
        // A static member of a trait `impl` (A-12 slice 6) carries its trait
        // instance, so `From<IoErr>`'s `from` and `From<String>`'s `from` for
        // the same type are two functions rather than a collision.
        None => match &f.trait_impl {
            Some(site) if !site.trait_instance.is_empty() => {
                format!("{}#{}@{}", site.self_type, f.name, site.trait_instance)
            }
            _ => f.name.clone(),
        },
    }
}

const PRINTABLE_BUILTINS: &[&str] = &["print", "print!"];

/// Function names that may be declared more than once, once per parameter
/// type, in a compiler-shipped module (M6.3, stdlib decision 4). The call
/// site picks by argument type; user code cannot define overloads at all,
/// and cannot see the mechanism except as `math.abs` working on whatever
/// number it is given.
/// The overloaded `std/math` names that stayed usable as a function value when
/// they gained an `f32` overload: `abs`, `min`, `max` and `clamp` were never one.
const FLOAT_MATH_VALUES: &[&str] = &[
    "sqrt", "pow", "floor", "ceil", "round", "trunc", "exp", "ln", "log2", "log10", "sin", "cos", "tan", "atan2",
    "is_nan", "is_finite", "sign",
];

const OVERLOADABLE: &[&str] = &[
    "abs", "min", "max", "clamp", "sqrt", "pow", "floor", "ceil", "round", "trunc", "exp", "ln", "log2", "log10", "sin",
    "cos", "tan", "atan2", "is_nan", "is_finite", "sign",
];

/// Whether a module id is one the compiler ships (M6.3, stdlib decision 4).
/// The `std/` prefix is deliberately *not* the test: a project file at
/// `src/std/x.kp` is loaded from disk and prefixed like any other module, so
/// an id that only looks shipped would take the exemptions the compiler's own
/// text earns: the export-everything rule among them, which would silently
/// switch privacy off there. `stdlib::source` is the table of shipped text,
/// and `std/rt` (the injected runtime) is in it for the same reason.
fn is_shipped_module(module: &str) -> bool {
    crate::stdlib::source(module).is_some()
}

/// The canonical keys of the items each module exports (A-13, `plan/syntax.md`
/// 13.4). Private by default, so a key is present only for a declaration
/// written `pub`. A key is a declaration's own lookup name, which is exactly
/// what a cross-module lookup is keyed by.
///
/// A compiler-shipped module exports everything. That is the transitional
/// rule, not a convenience: shipped modules import each other (`std/fmt` ->
/// `std/str`, `std/net` -> `std/io` + `std/str`), so without it the standard
/// library stops building, and marking every shipped declaration `pub` is a
/// sweep this slice does not need.
///
/// The names are read from the *merged* program, where `resolve.rs` has
/// already prefixed every non-entry declaration with its module path, so a
/// declaration's own `name` is its canonical key as it stands.
fn collect_exports(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut export = |module: &str, name: &str, is_pub: bool| {
        if is_pub || is_shipped_module(module) {
            out.insert(name.to_string());
        }
    };
    for d in &program.datacs {
        export(&d.module, &d.name, d.is_pub);
    }
    for e in &program.enums {
        export(&e.module, &e.name, e.is_pub);
    }
    for t in &program.extern_types {
        export(&t.module, &t.name, t.is_pub);
    }
    for t in &program.traits {
        export(&t.module, &t.name, t.is_pub);
    }
    for c in &program.consts {
        export(&c.module, &c.name, c.is_pub);
    }
    for s in &program.statics {
        export(&s.module, &s.name, s.is_pub);
    }
    for f in &program.functions {
        // A method's reachability follows its declaring type, and the parser
        // refuses `pub` on one, so only a free function contributes a key: a
        // method is reached through a value, never through a module alias.
        if f.receiver.is_none() {
            export(&f.module, &f.name, f.is_pub);
        }
    }
    out
}

/// Whether the module that owns `key` exports it (A-13).
fn exported(key: &str, env: &TypeEnv) -> bool {
    env.exported.contains(key)
}

/// The refusal a cross-module reference to an existing but unexported item
/// gets (A-13). It is deliberately apart from the "has no item" message: the
/// item exists, and its own module is the reason it is out of reach.
fn private_item_error(alias: &str, item: &str, span: Span) -> TypeError {
    TypeError {
        message: format!(
            "'{alias}.{item}' is private: '{item}' is declared in module '{alias}' without 'pub', so only that module can name it; add 'pub' to its declaration to export it"
        ),
        span,
    }
}

/// Whether a canonical lookup key names some declared item in the merged
/// program. This is the existence half of the visibility gate, so a reference
/// to an item that does not exist still reads as "has no item" rather than
/// as "private".
fn declares(key: &str, ctx: &Ctx) -> bool {
    ctx.structs.contains_key(key)
        || ctx.enums.contains_key(key)
        || ctx.env.handles.contains(key)
        || ctx.consts.contains_key(key)
        || ctx.statics.contains_key(key)
        || ctx.sigs.contains_key(key)
        || ctx.env.generics.contains_key(key)
        || ctx.fns.templates.contains_key(key)
        || ctx.overloads.contains_key(key)
}

/// The visibility gate for a type name written through an alias
/// (`alias.Type`, A-13). `Ok(())` when the name is not alias-qualified, when
/// the alias is unknown, or when the name is no type at all, so the ordinary
/// "unknown type" path still reports those.
fn check_type_exported(
    name: &str,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    aliases: &HashMap<String, String>,
    env: &TypeEnv,
    span: Span,
) -> Result<()> {
    let Some((alias, item)) = name.split_once('.') else {
        return Ok(());
    };
    let Some(prefix) = aliases.get(alias) else {
        return Ok(());
    };
    let key = format!("{prefix}{item}");
    let known = structs.contains_key(&key)
        || enums.contains_key(&key)
        || env.generics.contains_key(&key)
        || env.handles.contains(&key);
    if known && !exported(&key, env) {
        return Err(private_item_error(alias, item, span));
    }
    Ok(())
}

/// The names an `extern type` may take (gap 1). A handle is a type name like
/// any other, so it must be unique in the program and must not collide with a
/// `datac`, an `enum` or a builtin: `Ty::Handle` and `Ty::Struct` are separate
/// variants, and a collision would make one of the two silently unreachable
/// rather than a diagnostic.
fn check_extern_type_names(program: &ast::Program) -> std::result::Result<(), Diagnostic> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for t in &program.extern_types {
        if !seen.insert(t.name.as_str()) {
            return Err(Diagnostic::new(
                format!("extern type '{}' is declared more than once", t.name),
                t.span,
                t.module.clone(),
            ));
        }
        if program.datacs.iter().any(|d| d.name == t.name)
            || program.enums.iter().any(|e| e.name == t.name)
        {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is used for both an 'extern type' and a datac or enum",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
        if Ty::from_name(&t.name).is_some() {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is a builtin type and cannot be redeclared as an 'extern type'",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
    }
    Ok(())
}

/// The parent relation of the program's `extern type`s (gap 1): a canonical
/// child name -> its canonical parent name. The written parent is qualified
/// with its declaring module's prefix and alias table, exactly as a field's
/// type name is, so `extern type Window: Widget` in one module and
/// `extern type Window: widgets.Widget` in another name the same type.
///
/// A parent must be another `extern type`, and the chain must be acyclic: a
/// value converts implicitly to *any* ancestor, and a cycle would make the
/// relation meaningless rather than merely deep. `Ok` on a program with no
/// parented handles, so the map is empty for every program that predates it.
fn build_handle_parents(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<HashMap<String, String>, Diagnostic> {
    let handles: std::collections::HashSet<&str> =
        program.extern_types.iter().map(|t| t.name.as_str()).collect();
    let mut parents: HashMap<String, String> = HashMap::new();
    for t in &program.extern_types {
        let Some(written) = &t.parent else {
            continue;
        };
        let prefix = project.prefix_of(&t.module);
        let aliases = project.alias_prefixes_of(&t.module);
        let key = qualify_written(written, prefix, &aliases);
        if !handles.contains(key.as_str()) {
            return Err(Diagnostic::new(
                format!(
                    "'extern type {}' names parent '{written}', which is not an 'extern type': a handle's parent must be another handle declared earlier in this module or imported",
                    bare_name(&t.name, prefix)
                ),
                t.span,
                t.module.clone(),
            ));
        }
        parents.insert(t.name.clone(), key);
    }
    for t in &program.extern_types {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut cur = t.name.as_str();
        loop {
            if !seen.insert(cur) {
                let prefix = project.prefix_of(&t.module);
                return Err(Diagnostic::new(
                    format!(
                        "'extern type {}' has a cycle in its parent chain; a handle may only convert up to an ancestor",
                        bare_name(&t.name, prefix)
                    ),
                    t.span,
                    t.module.clone(),
                ));
            }
            match parents.get(cur) {
                Some(p) => cur = p.as_str(),
                None => break,
            }
        }
    }
    Ok(parents)
}

/// Whether `ancestor` is `descendant` itself or one of its parent-chain
/// ancestors. This is the direction the implicit conversion runs: a child
/// converts to an ancestor, never the other way.
fn handle_is_same_or_ancestor(
    parents: &HashMap<String, String>,
    ancestor: &str,
    descendant: &str,
) -> bool {
    let mut cur = descendant;
    for _ in 0..=parents.len() {
        if cur == ancestor {
            return true;
        }
        match parents.get(cur) {
            Some(p) => cur = p.as_str(),
            None => return false,
        }
    }
    false
}

/// The private-in-public rule (A-13, `plan/syntax.md` 13.4): a declaration
/// another module can reach must not name, anywhere in its signature, a type
/// its module does not export. A private declaration may name anything,
/// because nothing outside its module can name it either.
///
/// The check runs once over the *merged* program with the whole export set
/// already in hand (`collect_exports`), rather than per item while the
/// declarations are collected, so a private type declared after the item that
/// names it is caught the same way as one declared before: the order of two
/// declarations in one file does not decide the answer.
fn check_pub_signatures(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<(), Diagnostic> {
    let exported = collect_exports(program);
    let declared: std::collections::HashSet<&str> = program
        .datacs
        .iter()
        .map(|d| d.name.as_str())
        .chain(program.enums.iter().map(|e| e.name.as_str()))
        .chain(program.extern_types.iter().map(|t| t.name.as_str()))
        .collect();
    for d in &program.datacs {
        if !exported.contains(&d.name) {
            continue;
        }
        SignatureScope::new(project, &d.module, &d.name, &d.name, &d.type_params, &exported, &declared)
            .check_fields(&d.fields)
            .map_err(|e| e.in_module(&d.module))?;
    }
    for e in &program.enums {
        if !exported.contains(&e.name) {
            continue;
        }
        let scope =
            SignatureScope::new(project, &e.module, &e.name, &e.name, &e.type_params, &exported, &declared);
        for v in &e.variants {
            scope.check_fields(&v.fields).map_err(|err| err.in_module(&e.module))?;
        }
    }
    for c in &program.consts {
        if !exported.contains(&c.name) {
            continue;
        }
        SignatureScope::new(project, &c.module, &c.name, &c.name, &[], &exported, &declared)
            .check_type(&c.ty)
            .map_err(|e| e.in_module(&c.module))?;
    }
    for s in &program.statics {
        if !exported.contains(&s.name) {
            continue;
        }
        SignatureScope::new(project, &s.module, &s.name, &s.name, &[], &exported, &declared)
            .check_type(&s.ty)
            .map_err(|e| e.in_module(&s.module))?;
    }
    for f in &program.functions {
        // A method is reached through a value of its declaring type, never
        // through a module alias (the parser refuses `pub` on one), so it is
        // as reachable as that type is, and the type is the declaration a
        // reader would make private instead.
        let (reachable, item, fix, type_params) = match &f.receiver {
            Some(r) => (
                exported.contains(&r.type_name),
                format!("{}.{}", r.type_name, f.name),
                r.type_name.clone(),
                &r.type_args,
            ),
            None => (
                exported.contains(&f.name),
                f.name.clone(),
                f.name.clone(),
                &f.type_params,
            ),
        };
        if !reachable {
            continue;
        }
        SignatureScope::new(project, &f.module, &item, &fix, type_params, &exported, &declared)
            .check_function(f)
            .map_err(|e| e.in_module(&f.module))?;
    }
    Ok(())
}

/// One reachable declaration's signature, checked against the export set. The
/// written type names are canonicalized exactly as resolution canonicalizes
/// them (`qualify_written`), so a bare name belongs to the declaring module
/// and an `alias.Item` spelling to the alias target, one naming scheme and
/// no second.
struct SignatureScope<'a> {
    project: &'a Project,
    module: &'a str,
    prefix: &'a str,
    aliases: HashMap<String, String>,
    /// The public item, as a reader writes it, for the refusal.
    item: &'a str,
    /// The declaration to make private instead: the item itself, or a
    /// method's declaring type.
    fix: &'a str,
    /// The type parameters in scope. A parameter is not a declared type, so
    /// naming one is never a violation, exactly as naming a builtin is not.
    type_params: Vec<&'a str>,
    exported: &'a std::collections::HashSet<String>,
    declared: &'a std::collections::HashSet<&'a str>,
}

impl<'a> SignatureScope<'a> {
    fn new(
        project: &'a Project,
        module: &'a str,
        item: &'a str,
        fix: &'a str,
        type_params: &'a [String],
        exported: &'a std::collections::HashSet<String>,
        declared: &'a std::collections::HashSet<&'a str>,
    ) -> SignatureScope<'a> {
        SignatureScope {
            project,
            module,
            prefix: project.prefix_of(module),
            aliases: project.alias_prefixes_of(module),
            item,
            fix,
            type_params: type_params.iter().map(String::as_str).collect(),
            exported,
            declared,
        }
    }

    fn check_fields(&self, fields: &[ast::FieldDecl]) -> Result<()> {
        for f in fields {
            self.check_type(&f.ty)?;
        }
        Ok(())
    }

    fn check_function(&self, f: &ast::FunctionDecl) -> Result<()> {
        for p in &f.params {
            self.check_type(&p.ty)?;
        }
        if let Some(ret) = &f.return_type {
            self.check_type(ret)?;
        }
        Ok(())
    }

    /// One written type, walked to the bottom: a container's element, a
    /// `Map`'s key and value, an array's element (`[T; N]`), a generic
    /// instantiation's arguments, a function type's parameters and return,
    /// and a borrow's pointee. A name is refused only when it is *declared*
    /// and its module does not export it, so an unknown name and a builtin
    /// both fall through to the messages that already report them.
    fn check_type(&self, ty: &ast::TypeExpr) -> Result<()> {
        if let Some(elem) = &ty.array_elem {
            return self.check_type(elem);
        }
        if let Some(elems) = &ty.tuple {
            for e in elems {
                self.check_type(e)?;
            }
            return Ok(());
        }
        if let Some(fn_params) = &ty.fn_params {
            for p in fn_params {
                self.check_type(&p.ty)?;
            }
            if let Some(ret) = &ty.fn_ret {
                self.check_type(ret)?;
            }
            return Ok(());
        }
        for arg in &ty.args {
            self.check_type(arg)?;
        }
        // A type parameter is not a declared type, and it shadows a name of
        // its own while an instantiation is resolved (A-2).
        if ty.args.is_empty()
            && !ty.name.contains('.')
            && self.type_params.contains(&ty.name.as_str())
        {
            return Ok(());
        }
        if Ty::from_name(&ty.name).is_some()
            || matches!(ty.name.as_str(), "List" | "Box" | "Map" | "Atomic" | "Thread" | "Mutex" | "RwLock")
        {
            return Ok(());
        }
        let key = qualify_written(&ty.name, self.prefix, &self.aliases);
        if self.declared.contains(key.as_str()) && !self.exported.contains(&key) {
            let bare = ty.name.rsplit('.').next().unwrap_or(&ty.name);
            let owner = self.type_module(&ty.name);
            return Err(TypeError {
                message: format!(
                    "'{}' is exported but its signature names '{key}', which module '{owner}' does not export: add 'pub' to '{bare}', or make '{}' private",
                    self.item, self.fix
                ),
                span: ty.span,
            });
        }
        Ok(())
    }

    /// The module a written type name belongs to: the declaring module for a
    /// bare name, the alias target for an `alias.Item` spelling.
    fn type_module(&self, name: &str) -> &str {
        let Some((alias, _)) = name.split_once('.') else {
            return self.module;
        };
        self.project
            .modules
            .iter()
            .find(|m| m.id == self.module)
            .and_then(|m| m.aliases.get(alias))
            .map(String::as_str)
            .unwrap_or(self.module)
    }
}

/// The name a callee key shows in a diagnostic: overload keys carry their
/// parameter types (`std.math.abs@f64`), which read as noise in an error
/// message about arguments.
fn display_callee(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// A function's signature key. An overloadable name in a stdlib module
/// carries its parameter types, which is what lets several declarations of
/// `abs` coexist in `std/math.kp` and be told apart at a call site.
fn sig_key_for(f: &ast::FunctionDecl, prefix: &str, params: &[(String, Ty)]) -> String {
    let plain = sig_key(f);
    if f.receiver.is_none()
        && is_shipped_module(&f.module)
        && OVERLOADABLE.contains(&bare_name(&f.name, prefix))
    {
        let types: Vec<String> = params.iter().map(|(_, t)| t.display()).collect();
        format!("{plain}@{}", types.join(","))
    } else {
        plain
    }
}

/// A compiler-provided function (M6). These live in `sigs`' namespace (keyed
/// `Type#name`, exactly like a receiver method) but their bodies are emitted
/// by `codegen_c.rs` from the fixed runtime preamble, so nothing about call
/// resolution, argument matching, or passdown wiring needs to know they are
/// special - only codegen, and only in that the C function it names already
/// exists.
struct Builtin {
    params: Vec<(String, Ty)>,
    ret: Ty,
    /// `Bytes.empty()` - an *associated* function written on the type name,
    /// with no receiver argument - versus `b.len()`, which takes one.
    associated: bool,
}

/// The `Bytes` surface (M6). Kept deliberately small: construct, measure,
/// index, mutate, copy. Everything else about `Bytes` needs either `String`
/// or growable buffers, which are later milestones.
///
/// `List<T>` (collections milestone) is deliberately *not* in this static
/// table: its signatures depend on the element type, so they are built on
/// demand by `list_sig` below, keyed by the same `List#name` spelling.
static BUILTINS: std::sync::LazyLock<HashMap<String, Builtin>> =
    std::sync::LazyLock::new(|| {
        let bytes = Ty::Bytes;
        let shared = Ty::Ref { mutable: false, inner: Box::new(bytes.clone()) };
        let exclusive = Ty::Ref { mutable: true, inner: Box::new(bytes.clone()) };
        let mut m = HashMap::new();
        let mut add = |key: &str, params: Vec<(String, Ty)>, ret: Ty, associated: bool| {
            m.insert(key.to_string(), Builtin { params, ret, associated });
        };
        add("Bytes#empty", vec![], bytes.clone(), true);
        add("Bytes#from_str", vec![("s".to_string(), Ty::Str)], bytes.clone(), true);
        add("Bytes#len", vec![("self".to_string(), shared.clone())], Ty::Usize, false);
        add("Bytes#is_empty", vec![("self".to_string(), shared.clone())], Ty::Bool, false);
        add(
            "Bytes#get",
            vec![("self".to_string(), shared.clone()), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "Bytes#set",
            vec![
                ("self".to_string(), exclusive),
                ("i".to_string(), Ty::Usize),
                ("b".to_string(), Ty::U8),
            ],
            Ty::Unit,
            false,
        );
        add("Bytes#clone", vec![("self".to_string(), shared)], bytes, false);

        // Growing a `Bytes` in place (S1). `push`/`append` need an exclusive
        // borrow, so the borrow checker refuses to grow a buffer while a
        // `str`/`&Bytes` view of it is alive - which is what makes the
        // reallocation sound without a lifetime syntax (see the bump of
        // `cap` in the runtime preamble).
        let bytes_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        add(
            "Bytes#push",
            vec![("self".to_string(), bytes_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#append",
            vec![
                ("self".to_string(), bytes_exclusive.clone()),
                ("other".to_string(), Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) }),
            ],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#reserve",
            vec![("self".to_string(), bytes_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("Bytes#clear", vec![("self".to_string(), bytes_exclusive)], Ty::Unit, false);

        // Byte-order reads and writes on a byte buffer (kernel stage 3,
        // MK9-S8): `b.read_u16_le(offset)` and its siblings, one name per
        // width and byte order, each bounds-checked and with no alignment
        // requirement. The receiver is `&Bytes` for a read and `mut Bytes` for
        // a write, so a write goes through the same exclusive borrow `push`
        // takes; every offset is a `usize`. `u8` needs no reader (`Bytes.get`
        // is one), `i16` does not exist, and `usize`/`isize` are absent because
        // a wire format fixes its field widths rather than the host's pointer
        // width.
        let bytes_shared = Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) };
        let bytes_mut = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        for value in ["u16", "u32", "u64", "i32", "i64"] {
            let ty = Ty::from_name(value).expect("integer type");
            for order in ["le", "be"] {
                add(
                    &format!("Bytes#read_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_shared.clone()),
                        ("offset".to_string(), Ty::Usize),
                    ],
                    ty.clone(),
                    false,
                );
                add(
                    &format!("Bytes#write_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_mut.clone()),
                        ("offset".to_string(), Ty::Usize),
                        ("value".to_string(), ty.clone()),
                    ],
                    Ty::Unit,
                    false,
                );
            }
        }

        // The `String` surface (M6.2): construct, measure, copy, and convert
        // to/from `Bytes` with an ownership transfer. `as_str`/`as_bytes`
        // return a borrow of the receiver, which needs the `&str`
        // return-elision rule (M6.3) before it can be typed at all.
        let string = Ty::String;
        let string_shared = Ty::Ref { mutable: false, inner: Box::new(string.clone()) };
        add("String#empty", vec![], string.clone(), true);
        add("String#from_str", vec![("s".to_string(), Ty::Str)], string.clone(), true);
        // By value: `from_bytes` takes ownership of the buffer, so the
        // receiver local is moved into this call rather than borrowed.
        add("String#from_bytes", vec![("b".to_string(), Ty::Bytes)], string.clone(), true);
        add("String#len", vec![("self".to_string(), string_shared.clone())], Ty::Usize, false);
        add(
            "String#char_count",
            vec![("self".to_string(), string_shared.clone())],
            Ty::Usize,
            false,
        );
        add("String#is_empty", vec![("self".to_string(), string_shared.clone())], Ty::Bool, false);
        add("String#clone", vec![("self".to_string(), string_shared.clone())], string.clone(), false);
        add(
            "String#into_bytes",
            vec![("self".to_string(), string.clone())],
            Ty::Bytes,
            false,
        );
        // A borrowed view of an owned buffer (M6.3). Both are the return-
        // borrow form: the result borrows from `self`, which is the whole
        // point of `&str` return elision.
        let str_ref = Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        };
        add(
            "String#as_str",
            vec![("self".to_string(), string_shared.clone())],
            str_ref.clone(),
            false,
        );
        add(
            "Bytes#as_str",
            vec![(
                "self".to_string(),
                Ty::Ref {
                    mutable: false,
                    inner: Box::new(Ty::Bytes),
                },
            )],
            str_ref,
            false,
        );

        // A length-carrying view of text (A-8). The two constructors are the
        // only places a length is *derived* (`str.view` runs one `strlen`,
        // `String.view` reads the length it already has); every operation on a
        // `View` is length-aware, and none of them is a C string function - a
        // slice has no NUL after it, so `strlen`/`strcmp` on one would read
        // past the slice and report a different string than the program has.
        // That is also why none of these takes or returns a `str`.
        //
        // `self` is by value for every one of them: a `View` is two words of
        // `Copy` data (a pointer and a length), so a borrow would add an
        // indirection and buy nothing - the same choice the integer
        // conversions make.
        add("str#view", vec![("self".to_string(), Ty::Str)], Ty::View, false);
        add(
            "String#view",
            vec![("self".to_string(), string_shared.clone())],
            Ty::View,
            false,
        );
        add("View#len", vec![("self".to_string(), Ty::View)], Ty::Usize, false);
        add("View#is_empty", vec![("self".to_string(), Ty::View)], Ty::Bool, false);
        add(
            "View#get",
            vec![("self".to_string(), Ty::View), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "View#slice",
            vec![
                ("self".to_string(), Ty::View),
                ("from".to_string(), Ty::Usize),
                ("to".to_string(), Ty::Usize),
            ],
            Ty::View,
            false,
        );
        add(
            "View#eq",
            vec![("self".to_string(), Ty::View), ("other".to_string(), Ty::View)],
            Ty::Bool,
            false,
        );
        // The one way to make a `str`-shaped value out of a slice, and the
        // reason it exists rather than an implicit conversion: a copy is the
        // only safe answer when the source may not be terminated.
        add(
            "View#to_string",
            vec![("self".to_string(), Ty::View)],
            Ty::String,
            false,
        );

        // Growing a `String` in place (S1). The same exclusive-borrow rule
        // as `Bytes`, and the same NUL-termination invariant: `push_str`
        // appends a borrowed view, so a view of `self` would alias the
        // buffer `realloc` may move - the borrow checker rejects that.
        let string_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::String) };
        add(
            "String#push",
            vec![("self".to_string(), string_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "String#push_str",
            vec![("self".to_string(), string_exclusive.clone()), ("s".to_string(), Ty::Str)],
            Ty::Unit,
            false,
        );
        add(
            "String#reserve",
            vec![("self".to_string(), string_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("String#clear", vec![("self".to_string(), string_exclusive)], Ty::Unit, false);
        add("String#with_capacity", vec![("n".to_string(), Ty::Usize)], string.clone(), true);

        // Failing deliberately (S1): `kelp_panic`/`kelp_assert` already exist in
        // the runtime, these two entries are all that was missing. They are
        // free functions, not methods - the map key is a bare name, which is
        // the same namespace `print` occupies.
        add("panic", vec![("msg".to_string(), Ty::Str)], Ty::Unit, false);
        add("assert", vec![("cond".to_string(), Ty::Bool)], Ty::Unit, false);

        // `thread.fence` (MK9-S3-M1 stage A2). Its key is module-qualified
        // rather than bare, because the function is reached through an import
        // (`thread.fence`) and not as a prelude name, and the shipped
        // `std/thread` module declares nothing: a fence is one instruction on
        // either side (`__atomic_thread_fence` in C, `fence seq_cst` in IR)
        // with no C body to write, so the compiler lowers it and the module's
        // source documents the surface instead, exactly as `std/atomic`
        // documents the atomic methods it does not declare. (Stage A2 first
        // tried this as an `extern fun fence(order: atomic.Ordering)` and
        // BUG-kQ7Rv was the extern prelude having no typedef for the enum;
        // that prelude now writes one, and this stays a builtin for the
        // instruction reason above.)
        add(
            crate::stdlib::FENCE_FN,
            vec![("order".to_string(), atomic_order_ty())],
            Ty::Unit,
            false,
        );

        // Integer conversions (S1). Every integer type has a `to_<T>` for
        // every integer type, because nothing else in the language converts
        // one width to another: operands must match exactly, so a `u64 % 10`
        // digit cannot reach `String.push(u8)` and a `u8` byte cannot index
        // with `usize` without one. Widening is exact; a conversion that can
        // lose a value is a checked narrowing that panics with the call's own
        // name (v1 has no `Option`), which `codegen_c.rs` emits inline.
        const INT_TYPES: &[&str] =
            &["i32", "i64", "u32", "u64", "u8", "u16", "usize", "isize"];
        for src in INT_TYPES {
            for dst in INT_TYPES {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("integer type"),
                    false,
                );
            }
            // Integer -> float (the jsonlibary port's fourth gap). An integer
            // never mixes with a float by itself - `n * 1.0` is a type error,
            // because operands must match exactly - and before this the only
            // way to widen one was to format it and parse it back
            // (`str.parse_f64(fmt.int(n).as_str())`), which allocates a
            // `String` to add zero to a number. `to_f64` is the exact
            // widening the hardware already has; `to_f32` is the lossy one,
            // and it is *unchecked* on purpose, because C's float conversion
            // is defined for every input (a value too large becomes an
            // infinity), so there is no lost-value case to refuse.
            //
            // The reverse (a float `to_<int>`) is deliberately absent: it can
            // lose a value, and v1 has no `Option` for it to report the loss
            // with. Write `math.trunc`/`math.floor` first and then a checked
            // integer conversion.
            for dst in ["f64", "f32"] {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("float type"),
                    false,
                );
            }
        }

        // Float width conversion: `f32.to_f64()` is exact, `f64.to_f32()` rounds
        // to the nearest `f32` (an infinity when the value is too large), and
        // neither can lose a value in the checked sense an integer narrowing
        // can, so both are unchecked like the integer-to-float pair above.
        add("f32#to_f64", vec![("self".to_string(), Ty::F32)], Ty::F64, false);
        add("f64#to_f32", vec![("self".to_string(), Ty::F64)], Ty::F32, false);

        // Byte-order conversion (kernel stage 3, MK9-S8 / roadmap B-10). Every
        // integer wider than a byte has `swap_bytes`, `to_be` and `to_le`, plus
        // the two static forms written on the type name
        // (`u32.from_be(x)`/`u32.from_le(x)`) that the associated-builtin path
        // above already resolves, the same way `Bytes.empty()` is resolved.
        //
        // `u8` is deliberately absent: swapping one byte is the identity, and
        // `to_be`/`to_le` on a byte are the identity on every target, so the
        // three names would only invite a reader to wonder which they meant.
        // `i16` is absent because v1 has no `i16` scalar to be one (the
        // `repr(i16)` tag storage in kernel stage 2 is a different thing).
        for src in ["u16", "u32", "u64", "i32", "i64", "usize", "isize"] {
            let ty = Ty::from_name(src).expect("integer type");
            for op in ["swap_bytes", "to_be", "to_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("self".to_string(), ty.clone())],
                    ty.clone(),
                    false,
                );
            }
            for op in ["from_be", "from_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("x".to_string(), ty.clone())],
                    ty.clone(),
                    true,
                );
            }
        }
        m
    });

/// The name a builtin type answers to in a method key - the non-`datac`
/// counterpart of `struct_name_of`.
fn builtin_type_name(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::Bytes => Some("Bytes"),
        Ty::String => Some("String"),
        // A `str` **value** can now have methods, which it could not before
        // A-8: `s.view()` needs a receiver, and a `str` has no other name to
        // be one (the `std/str` helpers are free functions called
        // `str.len(s)`). Only `str#view` exists, so nothing else changes.
        Ty::Str => Some("str"),
        Ty::View => Some("View"),
        Ty::List(_) => Some("List"),
        Ty::Box(_) => Some("Box"),
        Ty::Array(..) => Some("Array"),
        Ty::Map(..) => Some("Map"),
        // An `Atomic<T>` answers to one name whatever its element, so
        // `c.load(...)` keys on `Atomic#load` and the element type comes from
        // the receiver (`atomic_sig`).
        Ty::Atomic(_) => Some("Atomic"),
        // A `Mutex<T>` answers to one name whatever its element, exactly as an
        // `Atomic<T>` does (B-5, `std/sync`).
        Ty::Mutex(_) => Some("Mutex"),
        // An `RwLock<T>` answers to one name whatever its element, and the
        // non-generic `Condvar` answers to its own.
        Ty::RwLock(_) => Some("RwLock"),
        Ty::Condvar => Some("Condvar"),
        // The integer types answer to their written names so that the
        // conversion methods below (`n.to_u8()`) have a receiver to sit on.
        Ty::I32 => Some("i32"),
        Ty::I64 => Some("i64"),
        Ty::U32 => Some("u32"),
        Ty::U64 => Some("u64"),
        Ty::U8 => Some("u8"),
        Ty::U16 => Some("u16"),
        Ty::Usize => Some("usize"),
        Ty::Isize => Some("isize"),
        Ty::F32 => Some("f32"),
        Ty::F64 => Some("f64"),
        // An owned parameter is a borrow (M4's rule: every owned parameter is
        // borrowed), so `b.len()` on one arrives here as `&Bytes`/`&String`.
        Ty::Ref { inner, .. } => builtin_type_name(inner),
        _ => None,
    }
}

/// Why a type cannot be a container element: a borrow (or the `str` view
/// that is the same thing) would outlive the value it points at, and v1 has
/// no lifetime syntax to say otherwise.
fn borrowed_element_reason(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Str => "'str' is a borrowed view".to_string(),
        // A `View` is the same kind of thing with a length, and it has the
        // same problem: an element outlives the text it points at, and v1 has
        // no lifetime syntax to say otherwise (A-8, A-11).
        Ty::View => "'View' is a borrowed view".to_string(),
        Ty::Ref { mutable, inner } => format!(
            "'{}' is a borrow",
            Ty::Ref { mutable: *mutable, inner: inner.clone() }.display()
        ),
        Ty::Unit => "'unit' is not a value".to_string(),
        _ => return None,
    })
}

/// Why a *function value* cannot be a container element, in one sentence
/// that says the real reason (A-4). It is not the borrow rule and not the
/// inline-storage rule: a function pointer is a plain inline value, and the
/// only thing missing is a C spelling - a container's generated type names its
/// element as a prefix (`kelp_List__i32`), and a function pointer's name goes
/// *inside* the declarator (`int32_t (*kelp_f)(int32_t)`), so there is no such
/// prefix. The way round it is a one-field `datac`, which is what a program
/// needing a table of callbacks should write anyway.
fn fn_element_reason(ty: &Ty, container: &str) -> Option<String> {
    match ty {
        Ty::Fn { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet - a function value is a plain inline value, but a container's generated C type names its element type as a name prefix, and a function pointer has no such spelling (the name goes inside its declarator); hold it in a one-field 'datac' instead",
            ty.display()
        )),
        // A closure *is* nameable (it is an ordinary C struct), so the honest
        // reason is different: it owns an environment, and a container holds
        // its elements inline, so every element would have to be moved in and
        // dropped out through the container's own generated functions - which
        // is the same shape a function value is refused for, one level up
        // (A-4 phase E).
        Ty::Closure { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet - a closure owns the environment its captures were moved into, and a container's elements are held inline; put it in a 'Box' or a one-field 'datac' instead",
            ty.display()
        )),
        _ => None,
    }
}

/// Why an `Atomic<T>` cannot be a container element. MK9 9.3 left "may an
/// atomic be a `List` element, a `Map` value, a `datac` field" open, and A-10
/// slice 1 closed it: an atomic stays refused as a `List`/`Map`/`Box`/array
/// element, and a `datac` field of atomic type stays accepted. A container's
/// own routines read and write an element as a plain location, and every access
/// to an atomic has to be one atomic operation; a field is reached by field
/// access, which is where the atomic's own methods sit.
///
/// `container` is the whole phrase the refusal names, with its article, so a
/// `Map` value reads "a 'Map' value" and an array element reads "an array
/// element".
fn atomic_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Atomic(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Atomic<{}>' cannot be {container}: the container's own routines read and write it as a plain location, and every access to an atomic has to be one atomic operation. Keep the atomic in a place of its own ('val c = AtomicI64.new(0)') and put its value in the container",
        elem.display()
    ))
}

/// Why a `Thread<T>` cannot be a container element (MK9-S3-M1 stage B1, where
/// `Thread<T>` arrives). The same shape as the atomic refusal above and for a
/// neighbouring reason: a handle names one running worker, and a container's
/// own routines copy and drop an element as a plain location, which would name
/// the worker twice and join it once. A handle is accepted as a local, a field
/// and a parameter, which is where a join has one place to happen.
fn thread_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Thread(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Thread<{}>' cannot be {container}: a handle names one running worker and is joined once, while a container's own routines copy and drop an element like a plain location. Keep the handle in a local or a field ('val h = thread.spawn(...)') and join it there",
        elem.display()
    ))
}

/// Why a `Mutex<T>` cannot be a container element (B-5). Mirrors the thread
/// refusal: a mutex owns a runtime record and is reached by borrowing it, while
/// a container's own routines move and drop an element like a plain location.
/// A local, a field or a `pd` gives `lock` one place to borrow.
fn mutex_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Mutex(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Mutex<{}>' cannot be {container}: a mutex owns a runtime record and is reached by borrowing it, while a container's own routines move and drop an element like a plain location. Keep the mutex in a local or a field ('val m = Mutex.new(0)') and lock it there",
        elem.display()
    ))
}

/// Why an `RwLock<T>` or a `Condvar` cannot be a container element. Mirrors the
/// mutex refusal for the lock; a `Condvar` is shared by construction and a
/// container's copy-and-drop would name the one record twice.
fn rwlock_element_reason(ty: &Ty, container: &str) -> Option<String> {
    match ty {
        Ty::RwLock(elem) => Some(format!(
            "'RwLock<{}>' cannot be {container}: an rwlock owns a runtime record and is reached by borrowing it, while a container's own routines move and drop an element like a plain location. Keep the rwlock in a local or a field ('val rw = RwLock.new(0)') and borrow it there",
            elem.display()
        )),
        Ty::Condvar => Some(format!(
            "'Condvar' cannot be {container}: a condition variable is a shared runtime record paired with one mutex, while a container's own routines move and drop an element like a plain location. Keep it in a local or a field beside the mutex it pairs with"
        )),
        _ => None,
    }
}

/// Why a container that still carries the open-container marker cannot be a
/// container element, in one sentence that says the real reason: `ty` is such
/// a container, or holds one, so the generated typedef, its `push` and its drop
/// have no element type to name (`is_open_container` and
/// `contains_open_container` in `types.rs`). The check is at any depth, because
/// the outer container's own slot would carry the marker in.
fn open_container_element_reason(ty: &Ty, container: &str) -> Option<String> {
    if !crate::types::contains_open_container(ty) {
        return None;
    }
    Some(format!(
        "'{}' cannot be a '{container}' element: a container whose element type is not decided yet has no element type to name, so push a value into it (or write its element type) before using it as an element",
        ty.display()
    ))
}

/// Why a type cannot be a `List<T>` element, in one place so the `List<T>`
/// type position and `List.of(x)` give the same explanation. A borrow, a view,
/// an atomic, a container nobody decided and a function value each have their
/// own reason; what is left for the fall-through is a `Map` (a map value is not
/// a container either), a fixed-size array (a `List` element is not one) and
/// `unit` (the open-container marker).
fn list_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "List") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = open_container_element_reason(ty, "List") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'List' element is owned by the list, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', another 'List', or a 'Box'"
        );
    }
    format!(
        "'{}' cannot be a 'List' element: a 'Map', an array, a borrow or a view, an atomic, a function value, and 'unit' are refused. A list holds its elements in its own buffer, so a 'List' or a 'Box' element is fine; use a scalar, 'String', 'Bytes', a 'datac', an 'enum', another 'List', or a 'Box'",
        ty.display()
    )
}

/// Why a type cannot be a `Map<V>` value, in one place so the `Map<V>` type
/// position and `Map.new()` share the explanation. The value set is narrower
/// than the `List` element set: a container is refused here, so a `List` or a
/// `Box` value stays out even though it is a legal list element (A-10 slice 1).
fn map_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Map") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'Map' value is owned by the map, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', or an 'enum'"
        );
    }
    format!(
        "'{}' cannot be a 'Map' value: the value set is a scalar, 'String', 'Bytes', a 'datac', or an 'enum', which is narrower than a list's (that also takes a 'List' or a 'Box'). A container, an array, a borrow or a view, an atomic, a function value, and 'unit' are refused",
        ty.display()
    )
}

/// Whether a value of `ty` owns memory, for the repeat literal's "the element
/// must own nothing" rule (A-18).
///
/// `needs_drop` answers from the declared owned set, which is computed once
/// before any body is checked, so it cannot know about a *derived* record,
/// whose declaration is created on demand while bodies are checked: a tuple's.
/// A tuple is resolved through `struct_def`, which finds both kinds, and its
/// positions are walked (a nested tuple recurses). Everything else is
/// `needs_drop`'s own answer, which already looks through an array.
fn elem_owns_memory(ctx: &Ctx, ty: &Ty) -> bool {
    if crate::types::needs_drop(ty, &ctx.owned_structs) {
        return true;
    }
    match ty {
        Ty::Struct(name) if crate::types::is_tuple_name(name) => ctx.struct_def(name).map_or(
            false,
            |def| def.fields.iter().any(|f| elem_owns_memory(ctx, &f.ty)),
        ),
        _ => false,
    }
}

/// Why a type cannot be an array's element (A-18). The set is a scalar,
/// `String`, `Bytes`, a `datac`, or another array, since an array holds its
/// elements inline, so `[[u8; 4]; 8]` is a page table and is as expressible as
/// it is in C. A `List`/`Box`/`Map` element is refused: an array is fixed-size,
/// so a container element would make it a variable-size thing it is not (A-10
/// slice 1 left this rule alone even as it widened the `List` element set).
fn array_element_error(ty: &Ty) -> String {
    if let Some(reason) = atomic_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "an array element") {
        return reason;
    }
    match ty {
        Ty::Fn { .. } => {
            return format!(
                "'{}' cannot be an array element yet: a function value is a plain inline value, but the generated C names an array's element type as a name prefix, and a function pointer has no such spelling (the name goes inside its declarator); hold it in a one-field 'datac' instead",
                ty.display()
            )
        }
        Ty::Closure { .. } => {
            return format!(
                "'{}' cannot be an array element yet: a closure owns the environment its captures were moved into, and an array's elements are held inline; put it in a 'Box' or a one-field 'datac' instead",
                ty.display()
            )
        }
        _ => {}
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. An array holds its elements inline, so they have to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or another array"
        );
    }
    format!(
        "'{}' cannot be an array element: an array holds its elements inline and is fixed-size, so a 'List'/'Box'/'Map' element is refused here (a container finds its home in a 'List' or a 'Box' instead); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or a nested array",
        ty.display()
    )
}

/// The same for a `Box<T>` element, which may be a container: a box holds a
/// pointer, so `Box<List<i32>>` and `Box<Box<i32>>` are both fine, provided
/// the inner container's own element type is decided.
fn box_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Box") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = open_container_element_reason(ty, "Box") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'Box' owns its value, so the value has to outlive the box (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or another 'Box'/'List'"
        );
    }
    format!(
        "'{}' cannot be a 'Box' element: use a scalar, 'String', 'Bytes', a 'datac', an 'enum', a 'List', or another 'Box'",
        ty.display()
    )
}

/// Why a `Mutex<T>`'s element is refused: the value set is any owned type with
/// no borrows (the `Send`-shaped rule B-5 shares with `Thread<T>`). A borrow,
/// a `str` or a `View` in the slot would be a thread reading data another frame
/// owns.
fn mutex_element_error(ty: &Ty) -> String {
    if crate::types::is_borrowed_field_ty(ty) {
        return format!(
            "'{}' cannot be a 'Mutex' element: a mutex is shared across threads, and a borrow, a 'str' or a 'View' in the slot would be a thread reading data another frame owns. Guard an owned value instead",
            ty.display()
        );
    }
    format!(
        "'{}' cannot be a 'Mutex' element: a mutex guards an owned value, and 'unit' and a function value are refused",
        ty.display()
    )
}

/// Why an `RwLock<T>`'s element is refused: the same `Send`-shaped rule
/// `Mutex<T>` uses. A borrow, a `str` or a `View` in the slot would be a thread
/// reading data another frame owns.
fn rwlock_element_error(ty: &Ty) -> String {
    if crate::types::is_borrowed_field_ty(ty) {
        return format!(
            "'{}' cannot be an 'RwLock' element: an rwlock is shared across threads, and a borrow, a 'str' or a 'View' in the slot would be a thread reading data another frame owns. Guard an owned value instead",
            ty.display()
        );
    }
    format!(
        "'{}' cannot be an 'RwLock' element: an rwlock guards an owned value, and 'unit' and a function value are refused",
        ty.display()
    )
}

/// Why a type cannot be an `Atomic<T>`'s element. The set is closed (`bool` and
/// the integer types) because a single-word atomic cannot hold an owned value,
/// a float has no atomic arithmetic worth claiming, and a borrow is somebody
/// else's data.
fn atomic_element_error(ty: &Ty) -> String {
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. An 'Atomic<T>' is a location, and a borrow in one would be a location holding somebody else's location; use 'bool' or an integer type"
        );
    }
    format!(
        "'{}' cannot be an 'Atomic' element in v1: the element set is 'bool' and the integer types ('i32', 'i64', 'u32', 'u64', 'u8', 'u16', 'usize', 'isize'), because one atomic operation is one machine-word operation and there is no atomic arithmetic for a float",
        ty.display()
    )
}

/// Why a type cannot be a `Thread<T>`'s element. The element set is everything
/// a worker may hand its joiner, so the only refusals are the borrowed shapes:
/// a value that points at data owned by a frame that ends before the joiner
/// reads the result. The stored positions refuse the same three types
/// (`is_borrowed_field_ty`), which is what keeps this message and theirs in
/// step.
fn thread_element_error(ty: &Ty) -> String {
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. A 'Thread<T>' carries the value its worker returns to the joiner, and that value leaves the worker's frame, so T has to be an owned type; hand an owned value back instead"
        );
    }
    format!(
        "'{}' cannot be a 'Thread' element: a thread handle carries the value its worker returns, so T has to be a type that can be moved out of the worker's frame",
        ty.display()
    )
}

/// The `List<T>` builtins, instantiated for one element type. The element
/// decides two signatures:
///
/// - `get` returns a *copy* for a `Copy` element (`List<i32>` → `i32`) and a
///   shared borrow for an owned one (`List<String>` → `&String`, rooted at
///   the receiver by the same elision rule `.kp` functions use), because
///   copying a `String` out of a list would be an unowned second owner;
/// - `push`/`set`/`pop` take and give the element by value, which is where a
///   move in or out of the list happens (M3's move tracking runs on the
///   ordinary call-argument path, so no new borrow rules were needed).
///
/// A `Map`'s value type is the container's element here, so `elem_clonable` is
/// what `Map#get_or`/`Map#values` are gated on as well.
fn container_sig(
    key: &str,
    elem: &Ty,
    map_key: &Ty,
    array_len: Option<u64>,
    elem_sortable: bool,
    elem_clonable: bool,
) -> Option<FnSig> {
    if key.starts_with("List#") {
        list_sig(key, elem, elem_sortable, elem_clonable)
    } else if key.starts_with("Map#") {
        map_sig(key, map_key, elem, elem_clonable)
    } else if key.starts_with("Array#") {
        array_sig(key, elem, array_len.unwrap_or(0))
    } else {
        box_sig(key, elem)
    }
}

/// The `[T; N]` builtins (A-18), instantiated for one element type. The
/// surface is deliberately small, and `len` is not a function of the length
/// here at all: the length is part of the type, so the signature only has to
/// say that `.len()` is a `usize`; `codegen_c.rs` emits the constant.
///
/// `get` follows `List.get` exactly: a `Copy` element is handed back by value,
/// an owned one by shared borrow (`infer.rs` roots that borrow at the receiver
/// through `builtin_borrow_root`). `set` takes the element by value, and both
/// check the index against the type's own length at runtime.
fn array_sig(key: &str, elem: &Ty, len: u64) -> Option<FnSig> {
    // The receiver's own array type, length included: it is the type the call
    // site's argument has, so the signature has to spell the same one.
    let array = Ty::Array(Box::new(elem.clone()), len);
    let shared = Ty::Ref { mutable: false, inner: Box::new(array.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(array) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "Array#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Usize },
        "Array#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Array#get" => FnSig {
            params: vec![p("self", shared), p("i", Ty::Usize)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "Array#set" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        _ => return None,
    })
}

/// The `Box<T>` builtins (M8), instantiated for one element type. A box
/// holds one value or nothing at all, which is what makes a recursive type
/// expressible; `take` is how a value moves back out, and `get` borrows the
/// value in place.
fn box_sig(key: &str, elem: &Ty) -> Option<FnSig> {
    let boxed = Ty::Box(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(boxed.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(boxed.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "Box#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Box#get" => FnSig {
            params: vec![p("self", shared)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "Box#set" => FnSig {
            params: vec![p("self", exclusive), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "Box#take" => FnSig {
            params: vec![p("self", exclusive)],
            ret: elem.clone(),
        },
        _ => return None,
    })
}

/// The written type of an atomic operation's ordering argument: the `Ordering`
/// enum the shipped `std/atomic` module declares, named by that module's key
/// prefix. A builtin signature has no declaration site whose module prefix the
/// resolver could have applied, so the key is built here the same way
/// `lower_try_to` builds `Option`'s, and a program that never imports the
/// module cannot write a value of this type, so the key always names a
/// declaration by the time one is passed.
pub fn atomic_order_ty() -> Ty {
    Ty::Struct(format!(
        "{}Ordering",
        crate::stdlib::key_prefix(crate::stdlib::ATOMIC_MODULE)
    ))
}

/// The type of the failure half of `thread.join`'s result: the `ThreadErr`
/// `datac` the shipped `std/thread` module declares, named by that module's key
/// prefix. Built here for the same reason `atomic_order_ty` builds `Ordering`:
/// a builtin signature has no declaration site for the resolver to qualify a
/// type at, and a program that writes `thread.join` has imported the module, so
/// the key always names a declaration by the time one is reached.
pub fn thread_err_ty() -> Ty {
    Ty::Struct(format!(
        "{}{}",
        crate::stdlib::key_prefix(crate::stdlib::THREAD_MODULE),
        crate::stdlib::THREAD_ERR_NAME
    ))
}

/// The `Atomic<T>` methods (MK9-S3-M1 stage A1), instantiated for one element
/// type. Each takes the ordering as its last argument even though v1 has one
/// legal value: MK9 3 reserves the argument position, so a second variant is a
/// widening of the enum and a change to no signature.
///
/// Two things keep this out of the static table. The element type decides every
/// signature, exactly as it does for a container; and `compare_exchange` hands
/// back `Result<T, T>` (MK9 4: `Ok` the value that was there, `Err` the value
/// that turned out to be there), which is an instance of the stdlib's enum and
/// so has to be instantiated at the use site. The `Result` is named through its
/// module id rather than by the enum's own name, and a program that never
/// imports `std/result` is told to rather than given an internal error, the
/// same rule `try_to_<T>`'s `Option` follows.
///
/// `AtomicBool` gets no `fetch_*` at all: there is no atomic add for a flag, and
/// a `fetch_or` on one would be a way to write `true` without saying so.
///
/// **Every receiver is shared, `store`'s included.** MK9 1 states the reason and
/// 4's `mut` spelling was wrong: an atomic mutates through a shared borrow,
/// which is what leaves it with **no `&mut` path by construction**, which is
/// what makes a `&Atomic<T>` the thing that may cross a thread boundary and what
/// stage B1's `Sync`-shaped rule is built on. A `mut` receiver would give the
/// type a `&mut` path and make that rule unsound.
///
/// This is also what makes a `pd val` atomic the shape a shared counter wants:
/// a passdown declared `val` hands a function a *shared* borrow, which is the
/// borrow stage B1's rule lets cross to a thread. A `pd var` counter works too,
/// including a store followed by a load in one function, because the atomic
/// operations are shared borrows reborrowed from the passdown's hidden
/// reference; what `var` adds is the exclusive borrow a function takes when it
/// assigns to the passdown itself, and that is the borrow a thread argument may
/// not take.
fn atomic_sig(key: &str, elem: &Ty, ctx: &Ctx, span: Span) -> Result<Option<FnSig>> {
    let cell = Ty::Atomic(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(cell.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let order = atomic_order_ty();
    Ok(Some(match key {
        "Atomic#new" => FnSig { params: vec![p("value", elem.clone())], ret: cell },
        "Atomic#load" => FnSig {
            params: vec![p("self", shared), p("order", order)],
            ret: elem.clone(),
        },
        "Atomic#store" => FnSig {
            params: vec![p("self", shared), p("value", elem.clone()), p("order", order)],
            ret: Ty::Unit,
        },
        // `swap` and the five `fetch_*`: one shape, because each is a
        // read-modify-write that hands back the value that was there. The
        // ordering comes last, which is the argument position MK9 3 reserves.
        // The integer guard is what keeps `fetch_add` off `AtomicBool`.
        "Atomic#swap"
        | "Atomic#fetch_add"
        | "Atomic#fetch_sub"
        | "Atomic#fetch_and"
        | "Atomic#fetch_or"
        | "Atomic#fetch_xor"
            if key == "Atomic#swap" || elem.is_integer() =>
        {
            FnSig {
                params: vec![
                    p("self", shared),
                    p("value", elem.clone()),
                    p("order", order),
                ],
                ret: elem.clone(),
            }
        }
        "Atomic#compare_exchange" => {
            let result_key = format!(
                "{}Result",
                crate::stdlib::key_prefix(crate::stdlib::RESULT_MODULE)
            );
            if !ctx.env.generics.contains_key(&result_key) {
                return Err(TypeError {
                    message: format!(
                        "'Atomic.compare_exchange' hands back 'Result<T, T>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                        result_key,
                        crate::stdlib::RESULT_MODULE
                    ),
                    span,
                });
            }
            let result = ctx.instantiate_type(&result_key, &[elem.clone(), elem.clone()], span)?;
            FnSig {
                params: vec![
                    p("self", shared),
                    p("current", elem.clone()),
                    p("new", elem.clone()),
                    p("order", order),
                ],
                ret: result,
            }
        }
        _ => return Ok(None),
    }))
}

/// The `Ptr<T>` operations that need an unsafe context (MK9-S2 stage 1, with
/// the fallible `try_alloc` from kernel stage 13).
///
/// `Ptr.alloc`, `Ptr.try_alloc`, `read`, `write`, `read_volatile`,
/// `write_volatile` and `free` touch memory the language did not check: they
/// are the whole reason `unsafe` exists, so each names itself in the refusal.
fn ptr_op_is_unsafe(key: &str) -> bool {
    matches!(
        key,
        "Ptr#read"
            | "Ptr#write"
            | "Ptr#read_volatile"
            | "Ptr#write_volatile"
            | "Ptr#read_unaligned"
            | "Ptr#write_unaligned"
            | "Ptr#alloc"
            | "Ptr#try_alloc"
            | "Ptr#free"
    )
}

/// The refusal for an unsafe-only pointer operation written where no `unsafe`
/// encloses it. The operation is named, because "unsafe operation" alone would
/// leave the reader to find which call in a statement it was.
fn require_ptr_unsafe(key: &str, span: Span, ctx: &Ctx) -> Result<()> {
    if ctx.unsafe_depth > 0 || !ptr_op_is_unsafe(key) {
        return Ok(());
    }
    let name = key.strip_prefix("Ptr#").unwrap_or(key);
    Err(TypeError {
        message: format!(
            "'Ptr.{name}' is an unsafe operation: it can only be used inside 'unsafe {{ ... }}' or in a function declared 'unsafe fun'"
        ),
        span,
    })
}

/// Refuses a name that reaches a `static mut` outside an `unsafe` context
/// (kernel stage 7). The gate fires where the name is resolved, so every way
/// of touching the object - a read, a write, a field access, a method
/// receiver or an address - goes through it, exactly as `Ptr`'s unsafe
/// operations do.
fn require_static_unsafe(s: &StaticInfo, span: Span, ctx: &Ctx) -> Result<()> {
    if !s.mutable || ctx.unsafe_depth > 0 {
        return Ok(());
    }
    Err(TypeError {
        message: format!(
            "'static mut {}' is an unsafe operation: reading, writing or taking its address can only be done inside 'unsafe {{ ... }}' or in a function declared 'unsafe fun'",
            s.name
        ),
        span,
    })
}

/// Whether a method call receiver is a `Ptr`: the bare type name (`Ptr.null()`,
/// which is an *associated* call) or a value whose type is `Ptr<T>` (which is
/// what lets `p.cast<u8>()` carry a type argument at all).
fn ptr_receiver(receiver: &ast::Expr, ctx: &mut Ctx) -> bool {
    if let ast::Expr::Ident(name, _) = receiver {
        if name == "Ptr" && ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
            return true;
        }
    }
    check_expr(receiver, ctx, None)
        .map(|e| crate::types::ptr_elem_of(&e.ty).is_some())
        .unwrap_or(false)
}

/// `Ptr.null()`, `Ptr.from_addr(a)`, `Ptr.from_ref(&x)`, `Ptr.from_mut(x)`,
/// `Ptr.alloc<T>(count)` (MK9-S2 stage 1) and `Ptr.try_alloc<T>(count)` (kernel
/// stage 13).
///
/// The element type comes from the call's own `<T>` when it writes one, and
/// otherwise from the binding's written type or the expected type: a constructor
/// that produces an `i32` location has nothing else to read it from, so
/// `val p = Ptr.null()` is refused and the message says where to write it.
fn check_ptr_associated(
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Ptr.from_fn(f)` (kernel gap 2): the address of a declared `fun` or a
    // `fun` value with no environment, as a `Ptr<u8>`. It is the one pointer
    // constructor that is *safe*: it names code the compiler already checked,
    // and the result is an address, not a call. Its element is always `u8`, so
    // it never reads the call's own type argument or the expected type.
    if method == "from_fn" {
        if !type_args.is_empty() {
            return Err(TypeError {
                message: "'Ptr.from_fn' takes no type argument: its result is always 'Ptr<u8>'"
                    .to_string(),
                span,
            });
        }
        let [arg] = args else {
            return Err(TypeError {
                message: format!("'Ptr.from_fn' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        };
        let value = check_expr(&arg.value, ctx, None)?;
        match &value.ty {
            Ty::Fn { .. } => {}
            Ty::Closure { .. } => {
                return Err(TypeError {
                    message: "'Ptr.from_fn' takes a function, but this is a closure: its body captured something, so it carries an environment the code address alone cannot describe; pass a declared 'fun' or a 'fun' literal that captures nothing".to_string(),
                    span,
                })
            }
            Ty::ScopedFn { .. } => {
                return Err(TypeError {
                    message: "'Ptr.from_fn' takes a function, but this is a scoped function value: it is a code pointer paired with a stack environment that lives only for the call it was written in, so it has no standalone code address".to_string(),
                    span,
                })
            }
            other => {
                return Err(TypeError {
                    message: format!(
                        "'Ptr.from_fn' takes a function value, found '{}': name a declared 'fun' or write a 'fun(...)' literal that captures nothing",
                        other.display()
                    ),
                    span,
                })
            }
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "Ptr#from_fn".to_string(),
                args: vec![value],
            },
            ty: Ty::Ptr(Box::new(Ty::U8)),
            span,
        });
    }
    let explicit = resolve_explicit_args(type_args, ctx)?;
    let elem = match explicit.len() {
        0 => expected.and_then(crate::types::ptr_elem_of).cloned(),
        1 => Some(explicit[0].clone()),
        n => {
            return Err(TypeError {
                message: format!("'Ptr' takes exactly 1 type argument, found {n}"),
                span,
            })
        }
    };
    let Some(elem) = elem else {
        return Err(TypeError {
            message: format!(
                "cannot tell which type 'Ptr.{method}' points at: write it in the call ('Ptr.{method}<i32>(...)' where the operation takes a type argument) or on the binding ('val p: Ptr<i32> = Ptr.null()')"
            ),
            span,
        });
    };
    if !crate::types::is_ptr_element(&elem) {
        return Err(TypeError {
            message: crate::types::ptr_element_error(&elem),
            span,
        });
    }
    let ptr = Ty::Ptr(Box::new(elem.clone()));
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let (key, params, ret) = match method {
        "null" => ("Ptr#null".to_string(), Vec::new(), ptr),
        "from_addr" => (
            "Ptr#from_addr".to_string(),
            vec![p("addr", Ty::Usize)],
            ptr,
        ),
        "from_ref" => (
            "Ptr#from_ref".to_string(),
            vec![p("place", Ty::Ref { mutable: false, inner: Box::new(elem) })],
            ptr,
        ),
        "from_mut" => (
            "Ptr#from_mut".to_string(),
            vec![p("place", Ty::Ref { mutable: true, inner: Box::new(elem) })],
            ptr,
        ),
        "alloc" => ("Ptr#alloc".to_string(), vec![p("count", Ty::Usize)], ptr.clone()),
        // `Ptr.try_alloc<T>(count)` (kernel stage 13): the fallible twin of
        // `Ptr.alloc`, so the same element resolution and the same signature,
        // only a different callee key.
        "try_alloc" => ("Ptr#try_alloc".to_string(), vec![p("count", Ty::Usize)], ptr),
        other => {
            return Err(TypeError {
                message: format!(
                    "'Ptr.{other}' is not a pointer operation: a pointer is made with 'Ptr.null()', 'Ptr.from_addr(a)', 'Ptr.from_ref(&x)', 'Ptr.from_mut(x)', 'Ptr.alloc<i32>(count)' or 'Ptr.try_alloc<i32>(count)', and read through the value it is bound to"
                ),
                span,
            })
        }
    };
    require_ptr_unsafe(&key, span, ctx)?;
    let checked = match_args(&format!("'Ptr.{method}'"), &params, args, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked },
        ty: ret,
        span,
    })
}

/// A method on a `Ptr<T>` value: `addr`, `is_null`, `offset`, `byte_offset`,
/// `cast<U>`, `read`, `write`, `read_volatile`, `write_volatile`,
/// `read_unaligned`, `write_unaligned` and `free` (MK9-S2 stage 1, with the
/// two unaligned forms from kernel stage 3).
fn check_ptr_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let elem = crate::types::ptr_elem_of(&probe.ty)
        .cloned()
        .expect("the caller matched a Ptr receiver");
    let explicit = resolve_explicit_args(type_args, ctx)?;
    let cast_to = if method == "cast" {
        match explicit.len() {
            1 => Some(explicit[0].clone()),
            n => {
                return Err(TypeError {
                    message: format!("'Ptr.cast' takes exactly 1 type argument, found {n}"),
                    span,
                })
            }
        }
    } else {
        if !explicit.is_empty() {
            return Err(TypeError {
                message: format!(
                    "'Ptr.{method}' does not take type arguments - a method's type parameters come from its receiver"
                ),
                span,
            });
        }
        None
    };
    if let Some(target) = &cast_to {
        if !crate::types::is_ptr_element(target) {
            return Err(TypeError {
                message: crate::types::ptr_element_error(target),
                span,
            });
        }
    }
    let key = format!("Ptr#{method}");
    let Some(sig) = ptr_sig(&key, &elem, cast_to.as_ref(), span)? else {
        return Err(TypeError {
            message: format!(
                "'Ptr.{method}' is not a pointer operation: a pointer has addr, is_null, offset, byte_offset, cast<U>, read, write, read_volatile, write_volatile, read_unaligned, write_unaligned and free"
            ),
            span,
        });
    };
    require_ptr_unsafe(&key, span, ctx)?;
    let mut checked = vec![check_call_arg("'self'", &sig.params[0].1, receiver, ctx)?];
    checked.extend(match_args(
        &format!("method 'Ptr.{method}'"),
        &sig.params[1..],
        args,
        span,
        ctx,
    )?);
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked },
        ty: sig.ret,
        span,
    })
}

/// The `Ptr<T>` value methods, instantiated for one element type (MK9-S2 stage
/// 1). Every receiver is written by value: a `Ptr<T>` is `Copy`, so passing one
/// moves nothing and the borrow checker has nothing to add here.
///
/// `read` and `read_volatile` hand back a `T`, which is only sound when `T` is
/// `Copy`: a read of an owned value would produce a second owner of the same
/// memory, and `write` would drop nothing, so the two would leak or double-free
/// depending on which came first. The refusal names that instead of leaving a
/// type error.
fn ptr_sig(key: &str, elem: &Ty, cast_to: Option<&Ty>, span: Span) -> Result<Option<FnSig>> {
    let ptr = Ty::Ptr(Box::new(elem.clone()));
    let p = |n: &str, t: Ty| (n.to_string(), t);
    Ok(Some(match key {
        "Ptr#addr" => FnSig { params: vec![p("self", ptr)], ret: Ty::Usize },
        "Ptr#is_null" => FnSig { params: vec![p("self", ptr)], ret: Ty::Bool },
        "Ptr#offset" | "Ptr#byte_offset" => FnSig {
            params: vec![p("self", ptr.clone()), p("n", Ty::Isize)],
            ret: ptr,
        },
        "Ptr#cast" => {
            let target = cast_to.expect("Ptr.cast resolves its target before its signature");
            FnSig { params: vec![p("self", ptr)], ret: Ty::Ptr(Box::new(target.clone())) }
        }
        "Ptr#free" => FnSig { params: vec![p("self", ptr)], ret: Ty::Unit },
        "Ptr#read" | "Ptr#read_volatile" | "Ptr#read_unaligned" => {
            if !elem.is_copy() {
                let op = key.strip_prefix("Ptr#").unwrap_or(key);
                return Err(TypeError {
                    message: format!(
                        "'Ptr<{}>.{op}' would read a '{}' out of raw memory, and '{}' is not 'Copy': a second owner of the same value would be made, so the memory could be freed twice. Read a 'Copy' element (a scalar, a handle, a pointer) instead",
                        elem.display(),
                        elem.display(),
                        elem.display()
                    ),
                    span,
                });
            }
            FnSig { params: vec![p("self", ptr)], ret: elem.clone() }
        }
        "Ptr#write" | "Ptr#write_volatile" | "Ptr#write_unaligned" => FnSig {
            params: vec![p("self", ptr), p("value", elem.clone())],
            ret: Ty::Unit,
        },
        _ => return Ok(None),
    }))
}

/// `sortable`/`clonable` are what `types::value_is_sortable` and
/// `value_is_clonable` say about `elem` with `ctx`'s own declaration graph, so
/// a `datac`/`enum` element whose fields are all copyable or all ordered
/// reaches the same two methods a scalar does (MISSING-jL9tE).
fn list_sig(key: &str, elem: &Ty, sortable: bool, clonable: bool) -> Option<FnSig> {
    let list = Ty::List(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(list.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(list.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "List#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Usize },
        "List#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "List#push" => FnSig {
            params: vec![p("self", exclusive), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#get" => FnSig {
            params: vec![p("self", shared), p("i", Ty::Usize)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "List#set" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#pop" => FnSig {
            params: vec![p("self", exclusive)],
            ret: elem.clone(),
        },
        "List#clear" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        // S1's additions. Three of them are only meaningful for an element
        // type that has an order / equality / a clone, so an unsupported
        // element type falls through to the "no method" diagnostic rather
        // than getting a signature codegen cannot emit (see
        // `types::list_sortable_elem` and friends).
        "List#sort" if sortable => {
            FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit }
        }
        "List#reverse" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        "List#contains" if crate::types::list_comparable_elem(elem) => FnSig {
            params: vec![p("self", shared), p("v", contains_param(elem))],
            ret: Ty::Bool,
        },
        "List#slice" if clonable => FnSig {
            params: vec![p("self", shared), p("from", Ty::Usize), p("to", Ty::Usize)],
            ret: list.clone(),
        },
        // `clone()` under the name a caller reaches for first: the copy
        // `slice(0, len)` already makes, under the same clonable gate, so an
        // element that cannot be copied out gets the "no method" diagnostic
        // rather than a signature codegen has no body for.
        "List#clone" if clonable => {
            FnSig { params: vec![p("self", shared)], ret: list.clone() }
        }
        "List#insert" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#remove" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize)],
            ret: elem.clone(),
        },
        _ => return None,
    })
}

/// The `Map<V>` builtins (MK5-S5), instantiated for one key and value type.
/// A `String` key is passed as a `str` view, a scalar by value and a user key
/// by shared borrow; the value type decides the same things the `List` element
/// type does:
///
/// - `get` returns a *copy* for a `Copy` value and a shared borrow for an
///   owned one, rooted at the receiver by the elision rule (`Map#get` is in
///   `builtin_borrow_root` for exactly this reason);
/// - `get_or` and `values` have to copy or clone a value *out*, so they are
///   offered only for a clonable value type, by the same declaration-graph
///   predicate (`types::value_is_clonable`) that gates `List.slice`, which
///   reaches a `datac`/`enum` whose fields all deep-copy (MISSING-qM4Zx);
/// - `insert` moves the value in, `remove` moves it out.
fn map_sig(key: &str, map_key: &Ty, value: &Ty, clonable: bool) -> Option<FnSig> {
    let map = Ty::Map(Box::new(map_key.clone()), Box::new(value.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(map.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(map.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let value_ref = Ty::Ref { mutable: false, inner: Box::new(value.clone()) };
    // A `String` key is passed as a view and copied in by the map; a scalar
    // key is passed by value.
    let key_param = map_key_param(map_key);
    Some(match key {
        "Map#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Usize },
        "Map#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Map#has" => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone())],
            ret: Ty::Bool,
        },
        "Map#get" => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone())],
            ret: if value.is_copy() { value.clone() } else { value_ref },
        },
        "Map#get_or" if clonable => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone()), p("fallback", value.clone())],
            ret: value.clone(),
        },
        "Map#insert" => FnSig {
            params: vec![p("self", exclusive), p("key", key_param.clone()), p("v", value.clone())],
            ret: Ty::Unit,
        },
        "Map#remove" => FnSig {
            params: vec![p("self", exclusive), p("key", key_param.clone())],
            ret: value.clone(),
        },
        "Map#clear" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        "Map#keys" => FnSig {
            params: vec![p("self", shared)],
            ret: Ty::List(Box::new(map_key.clone())),
        },
        "Map#values" if clonable => FnSig {
            params: vec![p("self", shared)],
            ret: Ty::List(Box::new(value.clone())),
        },
        _ => return None,
    })
}

/// Why a type cannot be a `Map` key (A-2, A-6, A-12). The compiler derives the
/// key's hash and its equality from the fields, and the only way past a field
/// the derivation refuses is the declared pair, so the message names it: this
/// is the one refusal a caller's next edit can answer.
fn map_key_error(ty: &Ty) -> String {
    format!(
        "'{}' cannot be a Map key: the key has to be 'String', an integer, 'bool' or 'char', or a 'datac'/'enum' whose every field is one of those, because the compiler derives the key's hash and its equality from the fields; a 'datac' or 'enum' outside that set becomes a key by declaring 'fun T.hash().return: u64' and 'fun T.eq(other: &T).return: bool'",
        ty.display()
    )
}

/// How a `Map`'s key is passed to its own methods: a `String` key is a `str`
/// view (the map copies it in, and a view is what a `str` caller already has),
/// a scalar key is passed by value, and a user key is a shared borrow the map
/// clones from (passing it by value would move the caller's key, or copy an
/// owned field it does not own twice).
fn map_key_param(key: &Ty) -> Ty {
    match key {
        Ty::String => Ty::Str,
        Ty::Struct(_) => Ty::Ref { mutable: false, inner: Box::new(key.clone()) },
        other => other.clone(),
    }
}

/// How `List.contains` takes its needle: a `Copy` element by value, an owned
/// one by borrow. Taking a `String` by value would move it into a membership
/// test, which is never what the caller meant.
fn contains_param(elem: &Ty) -> Ty {
    if elem.is_copy() {
        elem.clone()
    } else {
        Ty::Ref { mutable: false, inner: Box::new(elem.clone()) }
    }
}

/// The signature a builtin key resolves to, if it is one.
fn builtin_sig(key: &str) -> Option<FnSig> {
    BUILTINS.get(key).map(|b| FnSig {
        params: b.params.clone(),
        ret: b.ret.clone(),
    })
}

/// For a callee whose result is a *borrow*, the parameter index the elided
/// lifetime binds to - always the first reference parameter, which is why
/// this only has to answer for functions that return one at all.
/// `borrowck.rs` needs it at every return site and every call site; builtins
/// share the rule with `.kp` functions.
pub fn borrow_root_param(params: &[(String, Ty)]) -> Option<usize> {
    params.iter().position(|(_, t)| is_ref_param(t))
}

/// Whether a parameter is a reference the elision rule can borrow from.
/// `str` and `View` count: a `str` parameter *is* `&str`, and a `View` is a
/// `&str` with a length, both modelled as flat borrowed values (`Ty::Str`,
/// `Ty::View`) rather than a `Ty::Ref`. That is the same set as
/// `types::is_borrowed_field_ty` - a type that points at somebody else's data
/// cannot be a parameter's elision root unless it is also a type you cannot
/// store, which is the same fact stated twice.
pub fn is_ref_param(ty: &Ty) -> bool {
    crate::types::is_borrowed_field_ty(ty)
}

/// Whether a value of this type **borrows**: the result of a function that
/// returns one has to be rooted in a parameter (the elision rule), and the
/// caller has to keep that owner borrowed while the value lives.
///
/// `Ty::Ref` is the general case. `Ty::View` is the flat one: it is a pointer
/// and a length passed *by value*, so it is not a `Ty::Ref` in the type
/// system, but it points at text somebody else owns exactly as `&str` does -
/// which is why every rule that applies to a returned `&str` has to apply to a
/// returned `View` (A-8). `borrowck.rs` keys those rules on this predicate
/// rather than on `Ty::Ref` alone.
pub fn is_borrowed_value(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. } | Ty::View)
}

/// The elided borrow root for a builtin method, if it returns a borrow.
pub fn builtin_borrow_root(key: &str) -> Option<Option<usize>> {
    // `List#get`/`Box#get`/`Map#get` on an owned element type return a
    // borrow of the container itself - the first parameter, like every
    // builtin method's `self`. (For a `Copy` element the type checker never
    // asks, since the call's result is not a borrow; see
    // `borrowck::check_call`.)
    if key == "List#get" || key == "Box#get" || key == "Map#get" || key == "Array#get" {
        return Some(Some(0));
    }
    let b = BUILTINS.get(key)?;
    // A method that returns a *view* borrows its receiver just as one that
    // returns a `Ty::Ref` does: `s.view()`, `v.slice(a, b)`. `View` is passed
    // by value (two words of `Copy` data), so it is not a `Ty::Ref` in the
    // type system, but the value still points into the receiver.
    if crate::infer::is_borrowed_value(&b.ret) {
        Some(borrow_root_param(&b.params))
    } else {
        None
    }
}

/// A `str` and a `&str` are the same value, so a function declaring one may
/// return the other; whether a *borrowed* return is reachable is the borrow
/// checker's call, not the type's (see `borrowck::check_return_borrow`).
fn str_view_compatible(actual: &Ty, declared: &Ty) -> bool {
    join_value_type(actual, declared).is_some()
}

/// Whether a tail expression diverges, so the function never returns through
/// it and it satisfies any declared return type - including a borrowed one.
/// This is the `panic` ending every payload reader has:
///
/// ```kelp
/// fun as_str(v: &Json).return: &str {
///     if let Str(s) = v {
///         return s.as_str()
///     }
///     panic("json.as_str: value is not a string")
/// }
/// ```
///
/// `infer.rs` has had this rule since `Option`/`Result` shipped ("a body whose
/// tail is a `panic` diverges, so it satisfies any declared return type"). The
/// borrow checker's tail check did not, so it read the `panic` as the returned
/// value and refused it for not being derived from a reference parameter. One
/// predicate, both callers: two passes disagreeing about which expression is
/// the returned value is not a bug either pass can see on its own.
pub fn tail_diverges(expr: &tir::Expr) -> bool {
    matches!(&expr.kind, tir::ExprKind::Call { callee, .. } if callee == "panic")
}

/// The type two values of these types have in common, or `None` when they do
/// not agree: `==`, with the one relaxation `str` and `&str` get.
///
/// `is_str_like` is the language's own statement that those two are one type -
/// `str.eq`, `print`, `str.len`, a `str` parameter and a `&str` return all take
/// either, because both are the same C value (`const char*`). Returns already
/// said so (`str_view_compatible`), but arm unification compared them by
/// identity, so ordinary code was refused:
///
/// ```kelp
/// fun f(v: &J).return: &str = when v {
///     S(s) -> s.as_str()
///     N -> "n"
/// }
/// // error: 'when' arms have different types: '&str' and 'str'
/// ```
///
/// The join is the **borrow**, not the bare `str`: the borrow checker
/// understands `Ty::Ref` - it pins the owner and it drives the elision rule -
/// and does not understand `Ty::Str`, so a value that might be a view of
/// somebody else's text has to be typed as one. The two are the same C value,
/// so nothing else about the program changes. An *exclusive* `&mut str` is not
/// joined, so this cannot quietly downgrade one to a shared view.
fn join_value_type(left: &Ty, right: &Ty) -> Option<Ty> {
    if left == right {
        return Some(left.clone());
    }
    let exclusive = |ty: &Ty| matches!(ty, Ty::Ref { mutable: true, .. });
    if is_str_like(left) && is_str_like(right) && !exclusive(left) && !exclusive(right) {
        return Some(Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        });
    }
    None
}

/// Whether a type is `str` or a borrow of one - the two spellings of the
/// same borrowed view, which `print` and argument matching treat alike.
fn is_str_like(ty: &Ty) -> bool {
    match ty {
        Ty::Str => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Str),
        _ => false,
    }
}

/// A `char` value and an integer do not mix, and the reason is easy to miss:
/// a character literal takes whatever type its context asks for, so with
/// nothing to ask, it is `char`. Every such mismatch gets the same sentence
/// saying what to write instead.
fn char_integer_hint(found: &Ty, expected: &Ty) -> String {
    if let (Ty::Array(fe, fl), Ty::Array(ee, el)) = (found, expected) {
        if fe == ee && fl != el {
            return " - an array's length is part of its type; pass an array of the expected length, or declare the parameter over any length (`fun f<const N: usize>(a: &[T; N])`)".to_string();
        }
    }
    if matches!(found, Ty::Char) && expected.is_integer() {
        " - a character literal takes the type its context asks for, so with nothing to ask it is 'char'; give it the type you mean (`val c: u8 = '\"'`) or write the value as an integer".to_string()
    } else {
        String::new()
    }
}

/// The direction 5.1 does *not* make implicit: a `str`/`View` becomes an
/// owned `String`/`Bytes` automatically, but the reverse needs an explicit
/// `.as_str()`/`.view()` - `print` and `.as_str()` on a bound `String` both
/// look like the conversion happens for free, so a caller who has only met
/// those two reasonably expects `str.concat(a_string, ...)` to work the same
/// way, and the type-mismatch alone (`'String'`, expected 'str'`) does not
/// say why one direction is free and the other is not.
fn str_view_hint(found: &Ty, expected: &Ty) -> String {
    if !matches!(found, Ty::String) {
        return String::new();
    }
    let method = match expected {
        Ty::Str => "as_str()",
        Ty::View => "view()",
        _ => return String::new(),
    };
    format!(
        " - there is no implicit conversion this direction (5.1 only goes the other way, 'str'/'View' to an owned 'String'); call `.{method}` to borrow it"
    )
}

/// Whether the character literal `cp` can be a value of the integer type
/// `ty`. A character literal takes whatever integer type its context asks
/// for, and this is the one thing that can go wrong with that: `'中'` is a
/// value a `u8` cannot hold.
fn char_fits(cp: u32, ty: &Ty) -> bool {
    match ty {
        Ty::U8 => cp <= u8::MAX as u32,
        Ty::U16 => cp <= u16::MAX as u32,
        Ty::I32 => cp <= i32::MAX as u32,
        // `u32`/`i64`/`u64`/`usize`/`isize` hold every Unicode scalar value,
        // and `f32`/`f64`/`bool`/`char` are not integer targets at all (the
        // caller only asks this for an integer type).
        _ => true,
    }
}

/// The type an integer literal takes, and whether that is the `i32` a literal
/// falls back to when nothing determines one (syntax.md 2.4) rather than a
/// type its context supplied. `check_expr_inner` and the constant evaluator
/// have to answer this the same way, because both refuse an out-of-range
/// literal and the refusal says which of the two applied.
fn int_lit_ty(expected: Option<&Ty>) -> (Ty, bool) {
    match expected {
        Some(t) if t.is_integer() => (t.clone(), false),
        _ => (Ty::I32, true),
    }
}

/// The narrowest integer type that holds `value`, for the repair line of a
/// literal that does not fit the type it landed on. Every value the lexer
/// yields is within `u64`'s range (the widest integer type), and a sign only
/// adds the minimum, so this always names a type that holds the value.
fn int_lit_repair(value: i128) -> &'static str {
    if value < 0 {
        if value >= i32::MIN as i128 {
            "i32"
        } else {
            "i64"
        }
    } else if value <= u8::MAX as i128 {
        "u8"
    } else if value <= u16::MAX as i128 {
        "u16"
    } else if value <= u32::MAX as i128 {
        "u32"
    } else {
        "u64"
    }
}

/// Refuse an integer literal whose value does not fit the type it landed on
/// (BUG-t6Mz2). The type is fixed by this point, and past it the value is cut
/// to that type: a literal that does not fit leaves a different number behind,
/// at a line that is not the one that is wrong, so the refusal stands here.
fn check_int_literal(value: i128, ty: &Ty, defaulted: bool, span: Span) -> Result<()> {
    let Some((min, max)) = Ty::int_bounds(ty) else {
        return Ok(());
    };
    if value >= min && value <= max as i128 {
        return Ok(());
    }
    let repair = int_lit_repair(value);
    let message = if defaulted {
        format!(
            "'{value}' does not fit 'i32', the type a literal takes when nothing determines one: write the type it needs, as in 'val big: {repair} = {value}'"
        )
    } else {
        format!(
            "'{value}' does not fit '{}': write a type that holds it, as in 'val big: {repair} = {value}'",
            ty.display()
        )
    };
    Err(TypeError { message, span })
}

/// A code point as a diagnostic can show it: the character itself when it is
/// printable, `U+XXXX` otherwise (a control character has nothing to show).
fn printable_codepoint(cp: u32) -> String {
    match char::from_u32(cp) {
        Some(c) if !c.is_control() => c.to_string(),
        _ => format!("U+{cp:04X}"),
    }
}

/// One checked compile-time constant (M6.3). Folded at its declaration and
/// substituted at every use, so it has no storage, no initializer function,
/// and no runtime cost - `math.PI` is the literal `3.141592653589793` in the
/// generated C.
#[derive(Debug, Clone)]
struct ConstInfo {
    value: tir::Expr,
}
/// A folded constant value (A-19). Deliberately small: a compile-time value is
/// a scalar, so there is nothing to allocate and nothing to drop.
#[derive(Debug, Clone, PartialEq)]
enum ConstValue {
    /// Every integer width, `u64`'s whole range included, which is why the
    /// payload is `i128` rather than `i64`: the width and the signedness live
    /// in the `Ty` carried beside the value, and the arithmetic wraps at that
    /// width instead of guessing one.
    Int(i128),
    Float(f64),
    Bool(bool),
    /// A character constant's code point, kept apart from `Int` so that
    /// `const C: char = 'a'` substitutes as a character literal rather than as
    /// the integer `97`.
    Char(u32),
    Str(String),
    /// A tuple of constants, each with its own type: `const ORIGIN: (i32, i32)
    /// = (1, 2)`. It substitutes as the record literal the tuple is, so a use
    /// site pays nothing a runtime tuple would not.
    Tuple(Vec<(ConstValue, Ty)>),
    /// The value of an expression that has none: a `fun` called for its effect
    /// (or one that fell off its end), which is what makes `Unit` a result a
    /// caller has to handle rather than a case that cannot arise.
    Unit,
}

/// The functions a compile-time evaluator may call (A-19): one entry per
/// declared, non-generic `fun`, holding only what a compile-time call needs.
///
/// Built once, before anything is folded, because an initializer may name a
/// function declared *later* in the file, because a `const` and a `fun` are not in
/// each other's declaration order. What the evaluator does with an entry is
/// cross the body against its own step budget; what it will not do is reach
/// code it cannot see, and a call to an `extern` or to a stdlib overload is
/// refused with a message that says so.
#[derive(Default)]
struct ConstFns {
    fns: HashMap<String, ConstFn>,
}

/// One function a const evaluator may call: its parameters, its declared
/// return type, and its body, all as written. Nothing here is resolved yet:
/// a parameter's type is resolved when a call binds an argument to it, which
/// is the first moment it matters.
struct ConstFn {
    params: Vec<(String, ast::TypeExpr)>,
    ret: Option<ast::TypeExpr>,
    body: ast::Block,
    /// Symbol prefix of the module the body was written in, and that module's
    /// alias table. Carried here rather than looked up per call so the
    /// evaluator needs no `Project`: a name inside a called body resolves the
    /// same way it does when that module is the one being compiled.
    prefix: String,
    aliases: HashMap<String, String>,
}

/// Collects the compile-time callable functions (A-19). Generic functions are
/// left out on purpose: an instantiation's parameter types depend on the call
/// site, and a const initializer has no type arguments to offer.
fn collect_const_fns(program: &ast::Program, project: &Project) -> ConstFns {
    let mut out = ConstFns::default();
    for f in &program.functions {
        if !f.type_params.is_empty() || f.receiver.is_some() {
            continue;
        }
        if out.fns.contains_key(&f.name) {
            continue;
        }
        out.fns.insert(
            f.name.clone(),
            ConstFn {
                params: f.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
                ret: match &f.return_type {
                    Some(ast::TypeExpr { name, .. }) if name == "unit" => None,
                    Some(t) => Some(t.clone()),
                    None => None,
                },
                prefix: project.prefix_of(&f.module).to_string(),
                aliases: project.alias_prefixes_of(&f.module),
                body: match &f.body {
                    ast::FunctionBody::Block(b) => b.clone(),
                    // `fun f() = expr` is the same body one statement long.
                    ast::FunctionBody::Expr(e) => ast::Block {
                        stmts: vec![ast::Stmt::Expr {
                            expr: e.clone(),
                            span: e.span(),
                        }],
                        span: e.span(),
                    },
                    // An `extern fun` has no body to run at compile time.
                    ast::FunctionBody::Extern => continue,
                    // A trait signature has no body at all.
                    ast::FunctionBody::Signature => continue,
                },
            },
        );
    }
    out
}

/// Stand-in for "no type parameters in scope" at every site that is not
/// inside a generic template's own fields (A-2).
static EMPTY_PARAMS: std::sync::LazyLock<HashMap<String, Ty>> =
    std::sync::LazyLock::new(HashMap::new);

/// Resolves every top-level `const`, folding its initializer to a literal on
/// the way. Order matters: a `const` may only reference constants declared
/// before it, which is what keeps folding a single pass.
fn collect_consts(
    program: &ast::Program,
    project: &Project,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    const_fns: &ConstFns,
) -> std::result::Result<HashMap<String, ConstInfo>, Diagnostic> {
    let mut out: HashMap<String, ConstInfo> = HashMap::new();
    for decl in &program.consts {
        let prefix = project.prefix_of(&decl.module);
        let aliases = project.alias_prefixes_of(&decl.module);
        let bare = bare_name(&decl.name, prefix).to_string();
        let declared = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(&out),
            const_fns: Some(const_fns),
            handles: &env.handles,
        }
        .resolve(&decl.ty, false)
        .map_err(|e| e.in_module(&decl.module))?;
        if out.contains_key(&decl.name) {
            return Err(Diagnostic::new(
                format!("const '{bare}' is declared more than once"),
                decl.span,
                decl.module.clone(),
            ));
        }
        // A const shares its module's top-level namespace with `datac`s,
        // pds and functions. `resolve.rs` has already prefixed `decl.name`,
        // so a plain comparison against the prefixed names is enough.
        if structs.contains_key(&decl.name)
            || program
                .functions
                .iter()
                .any(|f| f.receiver.is_none() && f.name == decl.name)
            || program.pds.iter().any(|p| p.name == decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{bare}' is used for both a const and another top-level item"),
                decl.span,
                decl.module.clone(),
            ));
        }
        let literal = {
            if let Some(result) = const_int_array(decl, &declared) {
                let elems = result.map_err(|message| Diagnostic::new(message, decl.span, decl.module.clone()))?;
                tir::Expr {
                    kind: tir::ExprKind::ArrayLit(elems),
                    ty: declared.clone(),
                    span: decl.init.span(),
                }
            } else {
                let evaluator = ConstEvaluator::new(const_fns, structs, enums, env, &out);
                evaluator
                    .fold_const(&bare, &decl.init, &declared, prefix, &aliases)
                    .map_err(|e| e.in_module(&decl.module))?
            }
        };
        out.insert(decl.name.clone(), ConstInfo { value: literal });
    }
    Ok(out)
}

/// The name a module's own source writes for a prefixed top-level item.
fn bare_name<'a>(qualified: &'a str, prefix: &str) -> &'a str {
    qualified.strip_prefix(prefix).unwrap_or(qualified)
}

/// Resolves and constant-folds every top-level `static` (kernel stage 7). Runs
/// before any body is checked, because a body may name a static declared
/// anywhere in its module and a `pd` initializer may read one. The returned
/// table is keyed by the module-qualified name, exactly as `consts` is; the
/// value carries the type and mutability the use site needs, and the folded
/// initializer is what the backends emit as a global constant.
fn collect_statics(
    program: &ast::Program,
    project: &Project,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    env: &TypeEnv,
    fns: &FuncEnv,
) -> std::result::Result<(HashMap<String, StaticInfo>, Vec<tir::Static>), Diagnostic> {
    let mut table: HashMap<String, StaticInfo> = HashMap::new();
    let mut out: Vec<tir::Static> = Vec::new();
    for decl in &program.statics {
        let prefix = project.prefix_of(&decl.module);
        let bare = bare_name(&decl.name, prefix).to_string();
        if table.contains_key(&decl.name)
            || consts.contains_key(&decl.name)
            || structs.contains_key(&decl.name)
            || sigs.keys().any(|k| k == &decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{bare}' is used for both a static and another top-level item"),
                decl.span,
                decl.module.clone(),
            ));
        }
        // `percpu` is a freestanding-only storage class, and it is only
        // meaningful when mutable: one shared copy per CPU that no code may
        // write is an ordinary `static` with extra address space (kernel
        // stage 8). Both are checked here, before any backend is named, so
        // `kelpc check` and `kelpc build` refuse identically and the C oracle
        // refuses for the same reason the LLVM backend does.
        if decl.percpu && !decl.mutable {
            return Err(Diagnostic::new(
                format!(
                    "a per-CPU static must be 'mut', written 'percpu static mut {bare}: T = INIT': a read-only per-CPU is just a 'static'"
                ),
                decl.span,
                decl.module.clone(),
            ));
        }
        if decl.percpu && !project.target().is_freestanding() {
            return Err(Diagnostic::new(
                format!(
                    "the per-CPU static '{bare}' is only legal in a freestanding build: one copy per CPU needs a bare-metal target whose CPU index register the kernel sets, so '[target] mode = \"freestanding\"' and a '[target] cpus' are required"
                ),
                decl.span,
                decl.module.clone(),
            ));
        }
        let aliases = project.alias_prefixes_of(&decl.module);
        let ty = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(consts),
            const_fns: Some(const_fns),
            handles: &env.handles,
        }
        .resolve(&decl.ty, false)
        .map_err(|e| e.in_module(&decl.module))?;
        check_static_type(&ty, structs)
            .map_err(|msg| Diagnostic::new(msg, decl.span, decl.module.clone()))?;
        // A static's initializer is checked as an ordinary expression against
        // the declared type, so a type error reads the same here as anywhere
        // else; it is then folded to a constant, and anything that is not one
        // is refused with its own message rather than silently reaching a
        // backend that cannot run code at startup. The table holds only the
        // statics declared *before* this one, so a forward reference is an
        // undefined name and a backward one is the "not constant" refusal.
        let mut ctx = base_ctx(
            &decl.module,
            sigs,
            structs,
            enums,
            consts,
            const_fns,
            owned_structs,
            overloads,
            project,
            &[],
            None,
            &HashMap::new(),
            env,
            fns,
            &table,
        );
        let value = check_expr(&decl.init, &mut ctx, Some(&ty))
            .map_err(|e| e.in_module(&decl.module))?;
        let init = static_init_of(&value, &ty, structs)
            .map_err(|msg| Diagnostic::new(msg, decl.span, decl.module.clone()))?;
        let info = StaticInfo {
            qualified: decl.name.clone(),
            name: bare,
            ty: ty.clone(),
            mutable: decl.mutable,
        };
        table.insert(decl.name.clone(), info);
        out.push(tir::Static {
            name: decl.name.clone(),
            module: decl.module.clone(),
            ty,
            mutable: decl.mutable,
            percpu: decl.percpu,
            section: decl.section.clone(),
            init,
        });
    }
    Ok((table, out))
}

/// Whether `ty` may be a static's type (kernel stage 7): a `Copy` scalar, an
/// integer/`bool`/`char` array, a `datac` of those (recursively), or an
/// `Atomic<T>`. Everything else owns memory, is a borrow or a function value,
/// or is a shape the linker cannot place as one object.
fn check_static_type(ty: &Ty, structs: &HashMap<String, StructDef>) -> std::result::Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    check_static_type_inner(ty, structs, &mut seen)
}

fn check_static_type_inner(
    ty: &Ty,
    structs: &HashMap<String, StructDef>,
    seen: &mut std::collections::HashSet<String>,
) -> std::result::Result<(), String> {
    match ty {
        Ty::Bool | Ty::Char => Ok(()),
        t if t.is_integer() || t.is_float() => Ok(()),
        Ty::Atomic(elem) => {
            if crate::types::is_atomic_element(elem) {
                Ok(())
            } else {
                Err(format!(
                    "a static cannot hold '{}': an 'Atomic' element is a 'bool' or an integer type",
                    ty.display()
                ))
            }
        }
        Ty::Array(elem, _) => {
            if matches!(**elem, Ty::Bool | Ty::Char) || elem.is_integer() {
                Ok(())
            } else if matches!(**elem, Ty::Struct(_) | Ty::Array(..)) {
                // Kernel stage 21: an array of `datac` is what a scheduler's
                // table of tasks is, and a nested array is the same rule one
                // level down. The element has to obey its own type's rule, so
                // an array of a heap type or a `Ptr` is refused by the
                // recursion rather than by a second list.
                check_static_type_inner(elem.as_ref(), structs, seen)
            } else {
                Err(format!(
                    "an array static's element must be an integer, 'bool', 'char' or a \
                     'datac' of those, not '{}'",
                    elem.display()
                ))
            }
        }
        Ty::Struct(name) => {
            // A `datac` cannot contain itself by value, so `seen` never fires
            // for a well-formed program; it is a guard against a malformed
            // table rather than a rule a program can break.
            if !seen.insert(name.clone()) {
                return Ok(());
            }
            let Some(sd) = structs.get(name) else {
                return Err(format!("'{}' cannot be a static's type", ty.display()));
            };
            for field in &sd.fields {
                check_static_type_inner(&field.ty, structs, seen)?;
            }
            Ok(())
        }
        other => Err(format!(
            "'{}' cannot be a static's type: a static holds a Copy scalar, an integer/'bool'/'char' array, a 'datac' of those, or an 'Atomic<T>' - a heap type, a borrow, a 'View', a function value, a 'Ptr<T>' and an 'extern type' handle are all refused",
            other.display()
        )),
    }
}

/// Folds one checked initializer into the constant tree the backends emit
/// (kernel stage 7). The type is already checked against the initializer, so
/// the only job here is to refuse a shape that would need code to run: a call,
/// a name, an arithmetic expression, or a static read.
fn static_init_of(
    expr: &tir::Expr,
    ty: &Ty,
    structs: &HashMap<String, StructDef>,
) -> std::result::Result<tir::StaticInit, String> {
    use tir::StaticInit as SI;
    match ty {
        Ty::Bool => match &expr.kind {
            tir::ExprKind::BoolLit(b) => Ok(SI::Bool(*b)),
            tir::ExprKind::IntLit(v) => Ok(SI::Bool(*v != 0)),
            _ => Err(not_constant(ty)),
        },
        Ty::Char => match &expr.kind {
            tir::ExprKind::CharLit(c) => Ok(SI::Char(*c)),
            tir::ExprKind::IntLit(v) => Ok(SI::Char(*v as u32)),
            _ => Err(not_constant(ty)),
        },
        t if t.is_integer() => match &expr.kind {
            tir::ExprKind::IntLit(v) => Ok(SI::Int(*v)),
            tir::ExprKind::CharLit(c) => Ok(SI::Int(*c as i128)),
            _ => Err(not_constant(ty)),
        },
        t if t.is_float() => match &expr.kind {
            tir::ExprKind::FloatLit(v) => Ok(SI::Float(*v)),
            tir::ExprKind::IntLit(v) => Ok(SI::Float(*v as f64)),
            _ => Err(not_constant(ty)),
        },
        Ty::Array(elem, len) => match &expr.kind {
            tir::ExprKind::ArrayLit(elems) => {
                if elems.len() as u64 != *len {
                    return Err(format!(
                        "the initializer has {} element(s), but the static's type is an array of {len}",
                        elems.len()
                    ));
                }
                let mut out = Vec::with_capacity(elems.len());
                for e in elems {
                    out.push(static_init_of(e, elem, structs)?);
                }
                Ok(SI::Array(out))
            }
            tir::ExprKind::ArrayRepeat { value } => Ok(SI::Repeat {
                value: Box::new(static_init_of(value, elem, structs)?),
                len: *len,
            }),
            _ => Err(not_constant(ty)),
        },
        Ty::Struct(name) => match &expr.kind {
            tir::ExprKind::StructLit { fields, .. } => {
                let sd = structs
                    .get(name)
                    .ok_or_else(|| format!("'{}' cannot be a static's type", ty.display()))?;
                let mut out = Vec::with_capacity(fields.len());
                for (i, (_, value)) in fields.iter().enumerate() {
                    let field_ty = &sd.fields[i].ty;
                    out.push(static_init_of(value, field_ty, structs)?);
                }
                Ok(SI::Struct(out))
            }
            _ => Err(not_constant(ty)),
        },
        Ty::Atomic(elem) => match &expr.kind {
            tir::ExprKind::Call { callee, args } if callee == "Atomic#new" && args.len() == 1 => {
                Ok(SI::Atomic(Box::new(static_init_of(&args[0], elem, structs)?)))
            }
            _ => Err(not_constant(ty)),
        },
        other => Err(format!("'{}' cannot be a static's type", other.display())),
    }
}

fn not_constant(ty: &Ty) -> String {
    format!(
        "the initializer is not constant: a '{}' static is initialized with a literal, an array literal or repeat, a datac constructor with constant arguments, 'Atomic.new(<const>)', or another const",
        ty.display()
    )
}

/// The elements of a top-level `const` whose declared type is a fixed-size
/// array of integers and whose initializer is a literal array of integer
/// literals (kernel stage 4). The const folding evaluator refuses array
/// values, and a `section("...")` const has to be a data global, so this is
/// the one array shape it accepts: `[1, 2, 3]` or `[0; 16]`, element by
/// element, which is what a boot table or an IDT-shaped array is.
///
/// `None` means "not this shape, fold it the ordinary way"; `Some(Err(..))`
/// is a shape that *is* an integer array but whose initializer does not match,
/// so it is refused rather than folded into a confusing "array value" error.
fn const_int_array(
    decl: &ast::ConstDecl,
    declared: &Ty,
) -> Option<std::result::Result<Vec<tir::Expr>, String>> {
    let Ty::Array(elem, len) = declared else {
        return None;
    };
    if !elem.is_integer() {
        return None;
    }
    let values: Vec<i128> = match &decl.init {
        ast::Expr::ArrayLit { elems, .. } => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                match int_literal(e) {
                    Some(v) => out.push(v),
                    None => return None,
                }
            }
            out
        }
        ast::Expr::ArrayRepeat { value, .. } => match int_literal(value) {
            Some(v) => vec![v; *len as usize],
            None => return None,
        },
        _ => return None,
    };
    if values.len() != *len as usize {
        return Some(Err(format!(
            "the initializer has {} element(s), but the const's type is an array of {len}",
            values.len()
        )));
    }
    if let Some((lo, hi)) = Ty::int_bounds(elem) {
        for v in &values {
            if *v < lo || (*v as u128) > hi {
                return Some(Err(format!(
                    "the array element {v} does not fit the element type '{}'",
                    elem.display()
                )));
            }
        }
    }
    let span = decl.init.span();
    Some(Ok(values
        .into_iter()
        .map(|v| tir::Expr {
            kind: tir::ExprKind::IntLit(v),
            ty: (**elem).clone(),
            span,
        })
        .collect()))
}

/// The value of an integer literal, with `-` folded in so a negative array
/// element is one value rather than two expressions.
fn int_literal(expr: &ast::Expr) -> Option<i128> {
    match expr {
        ast::Expr::IntLit(v, _) => Some(*v as i128),
        ast::Expr::Unary {
            op: ast::UnOp::Neg,
            expr,
            ..
        } => int_literal(expr).map(|v| -v),
        _ => None,
    }
}

/// The folded value behind a `ConstInfo`, for a constant referenced by another
/// constant expression: its value *and* its type, because the next operator's
/// width is the pair's rather than the value's alone.
fn value_of(e: &tir::Expr) -> Option<ConstResult> {
    let value = match &e.kind {
        tir::ExprKind::IntLit(v) => ConstValue::Int(*v),
        tir::ExprKind::CharLit(c) => ConstValue::Char(*c),
        tir::ExprKind::FloatLit(v) => ConstValue::Float(*v),
        tir::ExprKind::BoolLit(b) => ConstValue::Bool(*b),
        tir::ExprKind::StringLit(s) => ConstValue::Str(s.clone()),
        // A folded tuple comes back out as the record literal it substituted as.
        tir::ExprKind::StructLit { fields, .. } => {
            let mut elems = Vec::with_capacity(fields.len());
            for (_, v) in fields {
                elems.push(value_of(v)?);
            }
            ConstValue::Tuple(elems)
        }
        _ => return None,
    };
    Some((value, e.ty.clone()))
}

/// Turns a folded value into the literal node a use site substitutes. The
/// typed C literal comes out of `codegen_c.rs`'s ordinary literal path, so a
/// `const` adds no code path of its own.
fn literal_for(value: &ConstValue, declared: &Ty) -> std::result::Result<tir::Expr, String> {
    use ConstValue::*;
    let kind = match (value, declared) {
        (Int(v), t) if t.is_integer() => tir::ExprKind::IntLit(*v),
        // A `const C: char = 'a'` folds to a code point and comes back out as
        // a character literal, so the substituted value prints and compares
        // exactly as the literal did.
        (Int(v), Ty::Char) => tir::ExprKind::CharLit(*v as u32),
        (Int(v), t) if t.is_float() => tir::ExprKind::FloatLit(*v as f64),
        (Float(v), t) if t.is_float() => tir::ExprKind::FloatLit(*v),
        (Bool(b), Ty::Bool) => tir::ExprKind::BoolLit(*b),
        (Char(c), Ty::Char) => tir::ExprKind::CharLit(*c),
        (Char(c), t) if t.is_integer() => tir::ExprKind::IntLit(*c as i128),
        (Str(s), Ty::Str) => tir::ExprKind::StringLit(s.clone()),
        (Tuple(elems), Ty::Struct(name)) => {
            let mut fields = Vec::with_capacity(elems.len());
            for (i, (v, t)) in elems.iter().enumerate() {
                fields.push((crate::types::tuple_field_name(i), literal_for(v, t)?));
            }
            tir::ExprKind::StructLit { name: name.clone(), fields, borrowed_fields: Vec::new() }
        }
        (_, t) => return Err(format!("its initializer is not a '{}' value", t.display())),
    };
    Ok(tir::Expr {
        kind,
        ty: declared.clone(),
        span: Span::new(0, 0),
    })
}

/// How many evaluation steps one compile-time fold may take before it is
/// refused (A-19). A constant expression is not a program, and a `fun` that a
/// `const` calls is running inside the compiler: the budget is what turns a
/// runaway `while` into a diagnostic instead of a compiler that never returns.
const CONST_EVAL_STEPS: u64 = 1_000_000;

/// How deep a compile-time call may nest, for the same reason.
const CONST_EVAL_DEPTH: u32 = 64;

/// The result of one compile-time evaluation: the value and the type it has.
type ConstResult = (ConstValue, Ty);

/// One local binding of an evaluated body: its value, its type, and whether
/// the body may write it (`var`).
struct ConstBinding {
    value: ConstResult,
    mutable: bool,
}

/// What a const-evaluated body has in scope: its local bindings, innermost
/// frame last, the module its names resolve against, and the return type of
/// the function being evaluated (for `return`).
struct ConstScope {
    frames: Vec<HashMap<String, ConstBinding>>,
    prefix: String,
    aliases: HashMap<String, String>,
    ret_ty: Option<Ty>,
    /// A `return` that has happened while this body is being evaluated, with
    /// the value it carried. Every expression after it is skipped and the call
    /// boundary reads it as the function's result, which is how a `return`
    /// inside an `if` unwinds out of a `fun` exactly as it does at runtime.
    ///
    /// It lives on the scope rather than in an error type so that a `return`
    /// reaches only the body it belongs to: a call evaluates its body in a
    /// scope of its own, so the flag cannot leak into the caller.
    returning: Option<Option<ConstResult>>,
}

/// How a block finished.
enum ConstFlow {
    /// It ran to the end. `Some` is the tail expression's value.
    Fell(Option<ConstResult>),
    /// A `return` left it.
    Returned(Option<ConstResult>),
}

impl ConstScope {
    fn new(prefix: String, aliases: HashMap<String, String>) -> ConstScope {
        ConstScope {
            frames: vec![HashMap::new()],
            prefix,
            aliases,
            ret_ty: None,
            returning: None,
        }
    }

    fn get(&self, name: &str) -> Option<&ConstResult> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.get(name).map(|b| &b.value))
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut ConstBinding> {
        self.frames.iter_mut().rev().find_map(|f| f.get_mut(name))
    }

    /// Whether a `return` has already happened in this body, in which case
    /// nothing after it is evaluated and the value it carries is the one the
    /// enclosing call produces.
    fn halted(&self) -> bool {
        self.returning.is_some()
    }
}

/// The two's-complement width of an integer type, in bits.
fn int_bits(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::I32 | Ty::U32 => 32,
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => 64,
        Ty::U8 => 8,
        Ty::U16 => 16,
        _ => return None,
    })
}

/// Whether an integer type's values are non-negative.
fn int_unsigned(ty: &Ty) -> bool {
    matches!(ty, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize)
}

/// `v` reduced into `ty`'s range, so a folding arithmetic step behaves like the
/// generated C: a `u8` wraps at 256 and an `i64` at its two's-complement
/// range, and neither silently grows into the next width.
fn wrap_int(v: i128, ty: &Ty) -> i128 {
    let Some(bits) = int_bits(ty) else {
        return v;
    };
    let mask: i128 = (1i128 << bits) - 1;
    let raw = v & mask;
    if !int_unsigned(ty) && raw >= (1i128 << (bits - 1)) {
        raw - (1i128 << bits)
    } else {
        raw
    }
}

/// What a compile-time fold can run (A-19).
///
/// A fold is *evaluation*, not execution: it walks expressions and the bodies
/// of plain `fun`s with parameter bindings of its own, and the only state it
/// carries is those bindings. Everything it can reach is a borrowed table built
/// before it started, so one of these is cheap to make per constant and shares
/// one step budget across every call it makes.
struct ConstEvaluator<'a> {
    const_fns: &'a ConstFns,
    structs: &'a HashMap<String, StructDef>,
    enums: &'a HashMap<String, EnumDef>,
    env: &'a TypeEnv,
    consts: &'a HashMap<String, ConstInfo>,
    steps: std::cell::Cell<u64>,
    depth: std::cell::Cell<u32>,
}

impl<'a> ConstEvaluator<'a> {
    fn new(
        const_fns: &'a ConstFns,
        structs: &'a HashMap<String, StructDef>,
        enums: &'a HashMap<String, EnumDef>,
        env: &'a TypeEnv,
        consts: &'a HashMap<String, ConstInfo>,
    ) -> ConstEvaluator<'a> {
        ConstEvaluator {
            const_fns,
            structs,
            enums,
            env,
            consts,
            steps: std::cell::Cell::new(0),
            depth: std::cell::Cell::new(0),
        }
    }

    /// The type-namespace view of one module, so a parameter's or a return
    /// value's written type resolves where it was written.
    fn type_scope<'s>(
        &'s self,
        prefix: &'s str,
        aliases: &'s HashMap<String, String>,
    ) -> TypeScope<'s> {
        TypeScope {
            structs: self.structs,
            enums: self.enums,
            env: self.env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(self.consts),
            const_fns: Some(self.const_fns),
            handles: &self.env.handles,
        }
    }

    /// Charges one step against the budget.
    fn tick(&self, span: Span) -> Result<()> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > CONST_EVAL_STEPS {
            return Err(TypeError {
                message: format!(
                    "this constant expression did not finish within {CONST_EVAL_STEPS} steps: a 'const' is folded while the compiler runs, so a 'fun' it calls has to terminate on the values it is given"
                ),
                span,
            });
        }
        Ok(())
    }

    /// Folds a `const` declaration's initializer into the literal a use site
    /// substitutes.
    fn fold_const(
        &self,
        name: &str,
        expr: &ast::Expr,
        declared: &Ty,
        prefix: &str,
        aliases: &HashMap<String, String>,
    ) -> Result<tir::Expr> {
        let mut scope = ConstScope::new(prefix.to_string(), aliases.clone());
        let (value, found) = self.eval(expr, Some(declared), &mut scope)?;
        literal_for(&value, declared).map_err(|_| TypeError {
            message: format!(
                "const '{name}' is declared as '{}' but its initializer is a '{}'",
                declared.display(),
                found.display()
            ),
            span: expr.span(),
        })
    }

    /// Folds the `N` of an array type or of a repeat literal (A-18): the one
    /// number a type or a literal needs, evaluated as a `usize` because a
    /// length is a count.
    fn array_len(
        &self,
        expr: &ast::Expr,
        prefix: &str,
        aliases: &HashMap<String, String>,
    ) -> Result<u64> {
        let mut scope = ConstScope::new(prefix.to_string(), aliases.clone());
        let (value, ty) = self.eval(expr, Some(&Ty::Usize), &mut scope)?;
        match value {
            ConstValue::Int(n) if n > 0 => Ok(n as u64),
            ConstValue::Int(n) => Err(TypeError {
                message: format!(
                    "an array length has to be at least 1, and this one is {n}: v1 has no zero-length form ('[T; 0]' would be a C array with no elements, which is not standard C)"
                ),
                span: expr.span(),
            }),
            _ => Err(TypeError {
                message: format!(
                    "an array length has to be an integer constant, and this one is a '{}' value",
                    ty.display()
                ),
                span: expr.span(),
            }),
        }
    }

    /// Folds one written expression to a value of `expected`'s type, or to its
    /// own default type when the context names none.
    fn eval(
        &self,
        expr: &ast::Expr,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        self.tick(expr.span())?;
        if scope.halted() {
            return Ok((ConstValue::Unit, Ty::Unit));
        }
        match expr {
            ast::Expr::IntLit(v, span) => Ok(match expected {
                Some(t) if t.is_float() => (ConstValue::Float(*v as f64), t.clone()),
                Some(t) if t.is_integer() => {
                    check_int_literal(*v as i128, t, false, *span)?;
                    (ConstValue::Int(*v as i128), t.clone())
                }
                Some(Ty::Char) => (ConstValue::Int(*v as i128), Ty::Char),
                _ => {
                    check_int_literal(*v as i128, &Ty::I32, true, *span)?;
                    (ConstValue::Int(*v as i128), Ty::I32)
                }
            }),
            ast::Expr::CharLit(v, span) => Ok(match expected {
                Some(t) if t.is_integer() => {
                    if !char_fits(*v, t) {
                        return Err(TypeError {
                            message: format!(
                                "the character '{}' (U+{:04X}) does not fit in '{}'",
                                printable_codepoint(*v),
                                v,
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                    (ConstValue::Int(*v as i128), t.clone())
                }
                _ => (ConstValue::Char(*v), Ty::Char),
            }),
            ast::Expr::ByteLit(v, span) => {
                if let Some(t) = expected {
                    if !matches!(t, Ty::U8) {
                        return Err(TypeError {
                            message: format!(
                                "a byte literal is a 'u8', not a '{}': write it as a character literal when it should take this type",
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                }
                Ok((ConstValue::Int(*v as i128), Ty::U8))
            }
            ast::Expr::FloatLit(v, _) => Ok(match expected {
                Some(t) if t.is_float() => (ConstValue::Float(*v), t.clone()),
                _ => (ConstValue::Float(*v), Ty::F64),
            }),
            ast::Expr::BoolLit(b, _) => Ok((ConstValue::Bool(*b), Ty::Bool)),
            ast::Expr::StringLit(s, _) => Ok((ConstValue::Str(s.clone()), Ty::Str)),
            ast::Expr::Ident(name, span) => {
                if let Some(v) = scope.get(name) {
                    return Ok(v.clone());
                }
                let key = format!("{}{}", scope.prefix, name);
                if let Some(c) = self.consts.get(&key).and_then(|c| value_of(&c.value)) {
                    return Ok(c);
                }
                if self.const_fns.fns.contains_key(&key) {
                    return Err(TypeError {
                        message: format!(
                            "'{name}' is a function, not a value: call it, and a constant expression can call it too when its body is one a constant can evaluate"
                        ),
                        span: *span,
                    });
                }
                Err(TypeError {
                    message: format!(
                        "'{name}' is not a constant here: a compile-time expression (an array length, or a 'const' initializer) can only use literals, 'const's declared before it, and the locals of the 'fun' it is evaluating; a 'val' from the enclosing function is not one"
                    ),
                    span: *span,
                })
            }
            // `math.PI`: another module's constant, reached through an alias.
            ast::Expr::FieldAccess { base, field, span } => {
                let ast::Expr::Ident(alias, _) = base.as_ref() else {
                    return Err(self.unsupported(expr.span(), "a field of a value"));
                };
                let Some(prefix) = scope.aliases.get(alias).cloned() else {
                    return Err(TypeError {
                        message: format!(
                            "'{alias}' is not a module alias, so '{alias}.{field}' is a field of a value; a constant expression has no values to read fields from"
                        ),
                        span: *span,
                    });
                };
                let key = format!("{prefix}{field}");
                if self.consts.contains_key(&key) && !exported(&key, self.env) {
                    return Err(private_item_error(alias, field, *span));
                }
                match self.consts.get(&key).and_then(|c| value_of(&c.value)) {
                    Some(c) => Ok(c),
                    None => Err(TypeError {
                        message: format!(
                            "module '{alias}' has no constant '{field}' declared before this one"
                        ),
                        span: *span,
                    }),
                }
            }
            ast::Expr::Unary { op, expr: sub, span } => {
                // The sign-and-literal pair `check_expr_inner` handles the same
                // way: under `-`, the value is the negated literal, not the
                // literal the lexer read.
                if *op == ast::UnOp::Neg {
                    if let ast::Expr::IntLit(v, lit_span) = &**sub {
                        if expected.map_or(true, |t| t.is_integer()) {
                            self.tick(*lit_span)?;
                            let (ty, defaulted) = int_lit_ty(expected);
                            check_int_literal(-(*v as i128), &ty, defaulted, *lit_span)?;
                            return self.fold_unary(*op, ConstValue::Int(*v as i128), &ty, *span);
                        }
                    }
                }
                let (v, ty) = self.eval(sub, expected, scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                self.fold_unary(*op, v, &ty, *span)
            }
            ast::Expr::Binary { op, lhs, rhs, span } => self.fold_binary(*op, lhs, rhs, *span, expected, scope),
            ast::Expr::If {
                cond,
                then_block,
                else_block,
                span: _,
            } => {
                let (value, ty) = self.eval(cond, Some(&Ty::Bool), scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                let ConstValue::Bool(take) = value else {
                    return Err(TypeError {
                        message: format!(
                            "the condition of an 'if' has to be a 'bool', and this one is a '{}'",
                            ty.display()
                        ),
                        span: cond.span(),
                    });
                };
                if take {
                    return match self.eval_block(then_block, expected, scope)? {
                        ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                            Some(v) => Ok(v),
                            None => Ok((ConstValue::Unit, Ty::Unit)),
                        },
                    };
                }
                match else_block.as_deref() {
                    Some(ast::ElseBranch::Block(b)) => match self.eval_block(b, expected, scope)? {
                        ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                            Some(v) => Ok(v),
                            None => Ok((ConstValue::Unit, Ty::Unit)),
                        },
                    },
                    Some(ast::ElseBranch::If(e)) => self.eval(e, expected, scope),
                    None => Ok((ConstValue::Unit, Ty::Unit)),
                }
            }
            ast::Expr::Block(block) => match self.eval_block(block, expected, scope)? {
                ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                    Some(v) => Ok(v),
                    None => Ok((ConstValue::Unit, Ty::Unit)),
                },
            },
            ast::Expr::Assign { target, value, span } => {
                let ast::Expr::Ident(name, nspan) = target.as_ref() else {
                    return Err(self.unsupported(expr.span(), "an assignment to anything but a local"));
                };
                // The declared type is the binding's, so `x = 1` in an `i64`
                // local is an `i64` assignment and not an `i32` one.
                let declared = scope.get(name).map(|(_, t)| t.clone());
                let v = self.eval(value, declared.as_ref(), scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                if let Some((_, ty)) = scope.get(name) {
                    if v.1 != *ty {
                        return Err(TypeError {
                            message: format!(
                                "cannot assign a '{}' to '{name}', which is a '{}'",
                                v.1.display(),
                                ty.display()
                            ),
                            span: *span,
                        });
                    }
                }
                match scope.get_mut(name) {
                    Some(b) if b.mutable => {
                        b.value = v.clone();
                        Ok((ConstValue::Unit, Ty::Unit))
                    }
                    Some(_) => Err(TypeError {
                        message: format!("cannot assign to '{name}': it was declared with 'val'"),
                        span: *nspan,
                    }),
                    None => Err(TypeError {
                        message: format!("undefined local '{name}'"),
                        span: *nspan,
                    }),
                }
            }
            ast::Expr::Call {
                callee,
                type_args,
                args,
                span,
            } => {
                // A layout query is foldable wherever it is written, including
                // an array length and a `const` that names a builtin
                // (kernel stage 2). It is resolved here as well as in
                // `check_layout_query`, because those two positions fold
                // through this evaluator rather than through the expression
                // checker.
                if !type_args.is_empty() {
                    if matches!(callee.as_str(), "size_of" | "align_of" | "offset_of")
                        && !callee.contains('.')
                    {
                        return self.layout_query(callee, type_args, args, *span, scope);
                    }
                    return Err(self.unsupported(expr.span(), "a call with explicit type arguments"));
                }
                let key = format!("{}{}", scope.prefix, callee);
                self.call_fn(&key, callee, args, *span, scope)
            }
            // `math.sqrt(2.0)` reaches here as a method call on a module
            // alias, exactly as `math.PI` does; anything else is either an
            // integer conversion (`n.to_u8()`) or out of reach.
            ast::Expr::MethodCall {
                receiver,
                method,
                args,
                type_args: _,
                span,
            } => {
                if let ast::Expr::Ident(alias, _) = receiver.as_ref() {
                    if let Some(prefix) = scope.aliases.get(alias).cloned() {
                        let key = format!("{prefix}{method}");
                        if self.const_fns.fns.contains_key(&key) && !exported(&key, self.env) {
                            return Err(private_item_error(alias, method, *span));
                        }
                        return self.call_fn(&key, &format!("{alias}.{method}"), args, *span, scope);
                    }
                }
                if let Some(to) = method.strip_prefix("to_") {
                    if !args.is_empty() {
                        return Err(self.unsupported(expr.span(), "a conversion call with arguments"));
                    }
                    let (value, from) = self.eval(receiver, None, scope)?;
                    if scope.halted() {
                        return Ok((ConstValue::Unit, Ty::Unit));
                    }
                    return self.convert(&value, &from, to, method, *span);
                }
                Err(self.unsupported(expr.span(), "a method call"))
            }
            ast::Expr::ArrayLit { .. } | ast::Expr::ArrayRepeat { .. } => {
                Err(self.unsupported(expr.span(), "an array value"))
            }
            ast::Expr::TupleLit { elems, .. } => {
                // The declared tuple type, when there is one, gives each
                // position the type a bare literal should take.
                let hinted: Option<Vec<Ty>> = match expected {
                    Some(Ty::Struct(name)) => self
                        .env
                        .instances
                        .borrow()
                        .records
                        .get(name)
                        .map(|def| def.fields.iter().map(|f| f.ty.clone()).collect()),
                    _ => None,
                };
                if hinted.as_ref().is_some_and(|h| h.len() != elems.len()) {
                    return Err(TypeError {
                        message: format!(
                            "a tuple of {} element(s) is expected here, and this one has {}",
                            hinted.as_ref().map_or(0, |h| h.len()),
                            elems.len()
                        ),
                        span: expr.span(),
                    });
                }
                let mut values = Vec::with_capacity(elems.len());
                for (i, e) in elems.iter().enumerate() {
                    let hint = hinted.as_ref().map(|h| &h[i]);
                    let (v, t) = self.eval(e, hint, scope)?;
                    if scope.halted() {
                        return Ok((ConstValue::Unit, Ty::Unit));
                    }
                    values.push((v, t));
                }
                let ty = tuple_ty(self.env, values.iter().map(|(_, t)| t.clone()).collect());
                Ok((ConstValue::Tuple(values), ty))
            }
            ast::Expr::Template { .. } => Err(self.unsupported(expr.span(), "a string template")),
            ast::Expr::FnLit { .. } => Err(self.unsupported(expr.span(), "a 'fun' literal")),
            ast::Expr::When { .. } => Err(self.unsupported(expr.span(), "a 'when' pattern match")),
            ast::Expr::Try { .. } => Err(self.unsupported(expr.span(), "the '?' operator")),
            ast::Expr::Deref { .. } => Err(self.unsupported(expr.span(), "a dereference")),
            ast::Expr::Ref { .. } => Err(self.unsupported(expr.span(), "a borrow")),
            ast::Expr::Unsafe { .. } => Err(self.unsupported(expr.span(), "an 'unsafe' block")),
            ast::Expr::Asm { .. } => Err(self.unsupported(expr.span(), "an 'asm' expression")),
        }
    }

    /// Folds `size_of<T>()`, `align_of<T>()` or `offset_of<T>("field")` while
    /// a `const` initializer or an array length is being evaluated. The type
    /// tables are the same ones the expression checker reads, so the two
    /// answer identically whenever both can answer at all; a `const` is folded
    /// before any `datac` exists, so a query there can only name a builtin.
    fn layout_query(
        &self,
        callee: &str,
        type_args: &[ast::TypeExpr],
        args: &[ast::CallArg],
        span: Span,
        scope: &ConstScope,
    ) -> Result<(ConstValue, Ty)> {
        self.tick(span)?;
        if type_args.len() != 1 {
            return Err(TypeError {
                message: format!(
                    "'{callee}' takes exactly one type argument, written '{callee}<T>()'"
                ),
                span,
            });
        }
        let ty = self
            .type_scope(&scope.prefix, &scope.aliases)
            .resolve(&type_args[0], true)?;
        let field = if callee == "offset_of" {
            if args.len() != 1 {
                return Err(TypeError {
                    message: "'offset_of' takes exactly one field name, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                });
            }
            match &args[0].value {
                ast::Expr::StringLit(field, _) if args[0].name.is_none() => Some(field.clone()),
                _ => {
                    return Err(TypeError {
                        message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                            .to_string(),
                        span,
                    })
                }
            }
        } else {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'{callee}' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            None
        };
        let value = layout_query_value(
            callee,
            &ty,
            field.as_deref(),
            self.structs,
            self.enums,
            span,
        )?;
        Ok((ConstValue::Int(value as i128), Ty::Usize))
    }

    /// The message every out-of-reach construct shares: it names the construct
    /// and says what a constant expression is allowed to do.
    fn unsupported(&self, span: Span, what: &str) -> TypeError {
        TypeError {
            message: format!(
                "a 'const' initializer cannot use {what}: what it can evaluate is literals, other 'const's, the arithmetic, comparison and bitwise operators, 'if', and a call to a plain 'fun' whose own body stays inside that"
            ),
            span,
        }
    }

    fn fold_unary(
        &self,
        op: ast::UnOp,
        v: ConstValue,
        ty: &Ty,
        span: Span,
    ) -> Result<ConstResult> {
        use ConstValue::*;
        let kind_mismatch = || TypeError {
            message: format!(
                "operator does not apply to '{}'",
                ty.display()
            ),
            span,
        };
        Ok(match (op, v) {
            (ast::UnOp::Neg, Int(i)) => (Int(wrap_int(-i, ty)), ty.clone()),
            (ast::UnOp::Neg, Float(f)) => (Float(-f), ty.clone()),
            (ast::UnOp::Not, Bool(b)) => (Bool(!b), Ty::Bool),
            (ast::UnOp::BitNot, Int(i)) => (Int(wrap_int(!i, ty)), ty.clone()),
            _ => return Err(kind_mismatch()),
        })
    }

    fn fold_binary(
        &self,
        op: ast::BinOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        use ast::BinOp::*;
        let hint = match op {
            Add | Sub | Mul | Div | Mod => expected.filter(|t| t.is_numeric()),
            BitAnd | BitOr | BitXor | Shl | Shr => expected.filter(|t| t.is_integer()),
            _ => None,
        };
        let (l, r) = if hint.is_none() && is_untyped_literal(lhs) && !is_untyped_literal(rhs) {
            let right = self.eval(rhs, None, scope)?;
            let ty = right.1.clone();
            let left = self.eval(lhs, Some(&ty), scope)?;
            (left, right)
        } else {
            let left = self.eval(lhs, hint, scope)?;
            let ty = left.1.clone();
            let right = self.eval(rhs, Some(&ty), scope)?;
            (left, right)
        };
        // A `return` in either operand means the whole expression is skipped:
        // its value belongs to the function that returned, not to this
        // operator.
        if scope.halted() {
            return Ok((ConstValue::Unit, Ty::Unit));
        }
        let (lv, lt) = l;
        let (rv, rt) = r;
        if lt != rt {
            return Err(TypeError {
                message: format!(
                    "a constant expression cannot mix '{}' and '{}': every operator requires matching operands, exactly as it does at runtime",
                    lt.display(),
                    rt.display()
                ),
                span,
            });
        }
        let no = |what: &str, ty: &Ty| TypeError {
            message: format!(
                "a 'const' expression cannot use {what} on '{}' values",
                ty.display()
            ),
            span,
        };
        let zero = || TypeError {
            message: "division by zero in a constant expression".to_string(),
            span,
        };
        use ConstValue::*;
        Ok(match (lv, rv) {
            (Int(a), Int(b)) => {
                let bits = int_bits(&lt).unwrap_or(64);
                match op {
                    Add => (Int(wrap_int(a + b, &lt)), lt),
                    Sub => (Int(wrap_int(a - b, &lt)), lt),
                    Mul => (Int(wrap_int(a * b, &lt)), lt),
                    Div => {
                        if b == 0 {
                            return Err(zero());
                        }
                        (Int(wrap_int(a / b, &lt)), lt)
                    }
                    Mod => {
                        if b == 0 {
                            return Err(zero());
                        }
                        (Int(wrap_int(a % b, &lt)), lt)
                    }
                    BitAnd => (Int(wrap_int(a & b, &lt)), lt),
                    BitOr => (Int(wrap_int(a | b, &lt)), lt),
                    BitXor => (Int(wrap_int(a ^ b, &lt)), lt),
                    // A shift by the width or more is undefined in C, so it is
                    // refused here rather than folded to whatever this machine
                    // happens to do; `>>` is arithmetic on a signed type,
                    // which is what the generated code does.
                    Shl | Shr if b < 0 || b >= bits as i128 => {
                        return Err(TypeError {
                            message: format!(
                                "a shift by {b} is not defined for a '{}', whose width is {bits} bits",
                                lt.display()
                            ),
                            span,
                        })
                    }
                    Shl => (Int(wrap_int(a << (b as u32), &lt)), lt),
                    // `>>` on the `i128` that holds a signed value is already
                    // arithmetic, and an unsigned value is never negative in
                    // that representation, so one shift covers both.
                    Shr => (Int(wrap_int(a >> (b as u32), &lt)), lt),
                    Eq => (Bool(a == b), Ty::Bool),
                    NotEq => (Bool(a != b), Ty::Bool),
                    Lt => (Bool(a < b), Ty::Bool),
                    LtEq => (Bool(a <= b), Ty::Bool),
                    Gt => (Bool(a > b), Ty::Bool),
                    GtEq => (Bool(a >= b), Ty::Bool),
                    And | Or => return Err(no(if op == And { "&&" } else { "||" }, &lt)),
                }
            }
            (Float(a), Float(b)) => match op {
                Add => (Float(a + b), lt),
                Sub => (Float(a - b), lt),
                Mul => (Float(a * b), lt),
                Div => (Float(a / b), lt),
                Mod => (Float(a % b), lt),
                Eq => (Bool(a == b), Ty::Bool),
                NotEq => (Bool(a != b), Ty::Bool),
                Lt => (Bool(a < b), Ty::Bool),
                LtEq => (Bool(a <= b), Ty::Bool),
                Gt => (Bool(a > b), Ty::Bool),
                GtEq => (Bool(a >= b), Ty::Bool),
                _ => return Err(no("a bitwise operator", &lt)),
            },
            (Bool(a), Bool(b)) => match op {
                And => (Bool(a && b), Ty::Bool),
                Or => (Bool(a || b), Ty::Bool),
                Eq => (Bool(a == b), Ty::Bool),
                NotEq => (Bool(a != b), Ty::Bool),
                _ => return Err(no("an arithmetic operator", &lt)),
            },
            _ => {
                return Err(TypeError {
                    message: format!(
                        "a constant expression cannot compare or combine '{}' values",
                        lt.display()
                    ),
                    span,
                })
            }
        })
    }

    /// An integer conversion in constant position (`n.to_usize()`): the same
    /// checked narrowing the runtime performs, reported as a diagnostic
    /// instead of a panic because nothing is running.
    fn convert(
        &self,
        value: &ConstValue,
        from: &Ty,
        to: &str,
        call: &str,
        span: Span,
    ) -> Result<ConstResult> {
        let Some(target) = Ty::from_name(to) else {
            return Err(TypeError {
                message: format!("'{}' is not a type to convert to", to),
                span,
            });
        };
        let (ConstValue::Int(n), true) = (value, from.is_integer()) else {
            return Err(TypeError {
                message: format!(
                    "only an integer conversion is available in a constant expression, and this one reads a '{}'",
                    from.display()
                ),
                span,
            });
        };
        if target.is_float() {
            return Ok((
                ConstValue::Float(if matches!(target, Ty::F32) { *n as f32 as f64 } else { *n as f64 }),
                target,
            ));
        }
        if !target.is_integer() {
            return Err(TypeError {
                message: format!("'{call}' is not a conversion a constant expression can perform"),
                span,
            });
        }
        let (lo, hi) = Ty::int_bounds(&target).expect("an integer target has bounds");
        // `hi` is a `u128` so `u64`/`usize` can state theirs, but every bound
        // it holds fits an `i128`, which is what the folded value is.
        let fits = *n >= lo && *n <= hi as i128;
        if !fits {
            return Err(TypeError {
                message: format!("{call}: value out of range"),
                span,
            });
        }
        Ok((ConstValue::Int(wrap_int(*n, &target)), target))
    }

    /// A `fun` call in constant position: bind the arguments to the declared
    /// parameters, then walk the body (A-19).
    fn call_fn(
        &self,
        key: &str,
        display: &str,
        args: &[ast::CallArg],
        span: Span,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        let Some(f) = self.const_fns.fns.get(key) else {
            return Err(TypeError {
                message: format!(
                    "'{display}' cannot be called in a constant expression: only a plain 'fun' whose own body a constant can evaluate is in reach, and this is not one (a builtin, an 'extern', a method and a generic function are all out of reach)"
                ),
                span,
            });
        };
        if self.depth.get() >= CONST_EVAL_DEPTH {
            return Err(TypeError {
                message: format!(
                    "evaluating '{display}' recursed more than {CONST_EVAL_DEPTH} calls deep: a constant expression has to terminate on the values it is given"
                ),
                span,
            });
        }
        let ts = self.type_scope(&f.prefix, &f.aliases);
        let mut callee = ConstScope::new(f.prefix.clone(), f.aliases.clone());
        let mut slots: Vec<Option<ConstResult>> = Vec::with_capacity(f.params.len());
        for _ in 0..f.params.len() {
            slots.push(None);
        }
        let mut next_positional = 0usize;
        for arg in args {
            let index = match &arg.name {
                Some(name) => match f.params.iter().position(|(p, _)| p == name) {
                    Some(i) => i,
                    None => {
                        return Err(TypeError {
                            message: format!("'{display}' has no parameter named '{name}'"),
                            span: arg.value.span(),
                        })
                    }
                },
                None => {
                    let i = next_positional;
                    next_positional += 1;
                    i
                }
            };
            if index >= f.params.len() {
                return Err(TypeError {
                    message: format!(
                        "'{display}' takes {} argument(s), and this call has more",
                        f.params.len()
                    ),
                    span: arg.value.span(),
                });
            }
            if slots[index].is_some() {
                return Err(TypeError {
                    message: format!(
                        "parameter '{}' of '{display}' is given twice",
                        f.params[index].0
                    ),
                    span: arg.value.span(),
                });
            }
            let pty = ts
                .resolve(&f.params[index].1, false)
                .map_err(|e| e.at(arg.value.span()))?;
            // The argument is evaluated in the *caller's* scope: `f(n)` inside
            // `f`'s own body reads the caller's `n`, which is the binding the
            // call frame is about to replace.
            slots[index] = Some(self.eval(&arg.value, Some(&pty), scope)?);
            if scope.halted() {
                return Ok((ConstValue::Unit, Ty::Unit));
            }
        }
        let ret_ty = match &f.ret {
            Some(t) => Some(ts.resolve(t, false).map_err(|e| e.at(span))?),
            None => None,
        };
        for (i, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(v) => {
                    let declared = ts
                        .resolve(&f.params[i].1, false)
                        .map_err(|e| e.at(span))?;
                    if v.1 != declared {
                        return Err(TypeError {
                            message: format!(
                                "parameter '{}' of '{display}' is a '{}', and this argument is a '{}'",
                                f.params[i].0,
                                declared.display(),
                                v.1.display()
                            ),
                            span,
                        });
                    }
                    callee.frames[0].insert(
                        f.params[i].0.clone(),
                        ConstBinding {
                            value: v,
                            mutable: false,
                        },
                    );
                }
                None => {
                    return Err(TypeError {
                        message: format!(
                            "'{display}' is missing its argument for parameter '{}'",
                            f.params[i].0
                        ),
                        span,
                    })
                }
            }
        }
        callee.ret_ty = ret_ty.clone();
        self.depth.set(self.depth.get() + 1);
        let flow = self.eval_block(&f.body, ret_ty.as_ref(), &mut callee);
        self.depth.set(self.depth.get() - 1);
        let value = match flow? {
            ConstFlow::Fell(v) | ConstFlow::Returned(v) => v,
        };
        match (value, ret_ty) {
            (Some(v), Some(t)) => {
                if v.1 != t {
                    return Err(TypeError {
                        message: format!(
                            "'{display}' declares '{}' but this call produced a '{}'",
                            t.display(),
                            v.1.display()
                        ),
                        span,
                    });
                }
                Ok(v)
            }
            (Some(_), None) => Err(TypeError {
                message: format!("'{display}' returns a value but declares no return type"),
                span,
            }),
            (None, None) => Ok((ConstValue::Unit, Ty::Unit)),
            (None, Some(t)) => Err(TypeError {
                message: format!(
                    "'{display}' has to produce a '{}' value, and it fell off its end",
                    t.display()
                ),
                span,
            }),
        }
    }

    /// Walks a block: its statements in order, one frame of locals, and its
    /// tail expression as the block's value.
    fn eval_block(
        &self,
        block: &ast::Block,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstFlow> {
        scope.frames.push(HashMap::new());
        let last = block.stmts.len().saturating_sub(1);
        let mut tail: Option<ConstResult> = None;
        for (i, stmt) in block.stmts.iter().enumerate() {
            match stmt {
                ast::Stmt::Let {
                    name,
                    mutable,
                    ty,
                    init,
                    ..
                } => {
                    let declared = match ty {
                        Some(t) => Some(
                            self.type_scope(&scope.prefix, &scope.aliases)
                                .resolve(t, false)?,
                        ),
                        None => None,
                    };
                    let value = self.eval(init, declared.as_ref(), scope)?;
                    if scope.halted() {
                        break;
                    }
                    if let Some(t) = &declared {
                        if &value.1 != t {
                            return Err(TypeError {
                                message: format!(
                                    "'{name}' is declared as '{}', and its initializer is a '{}'",
                                    t.display(),
                                    value.1.display()
                                ),
                                span: init.span(),
                            });
                        }
                    }
                    scope.frames.last_mut().expect("frame pushed above").insert(
                        name.clone(),
                        ConstBinding {
                            value,
                            mutable: *mutable,
                        },
                    );
                }
                // A destructuring binding is not a constant expression: the
                // value it takes apart has no fold (there is no aggregate in
                // `ConstValue`), so the binding cannot have one either.
                ast::Stmt::LetTuple { span, .. } => {
                    return Err(self.unsupported(*span, "a destructuring binding"));
                }
                ast::Stmt::While { cond, body, .. } => loop {
                    let (value, ty) = self.eval(cond, Some(&Ty::Bool), scope)?;
                    match value {
                        ConstValue::Bool(true) => {}
                        ConstValue::Bool(false) => break,
                        _ => {
                            return Err(TypeError {
                                message: format!(
                                    "the condition of a 'while' has to be a 'bool', and this one is a '{}'",
                                    ty.display()
                                ),
                                span: cond.span(),
                            })
                        }
                    }
                    self.tick(cond.span())?;
                    if scope.halted() {
                        break;
                    }
                    let _ = self.eval_block(body, None, scope)?;
                },
                ast::Stmt::Return { value, .. } => {
                    let ret = scope.ret_ty.clone();
                    let v = match value {
                        Some(e) => Some(self.eval(e, ret.as_ref(), scope)?),
                        None => None,
                    };
                    // The one place a `return` is recorded: from here on,
                    // `eval` and `eval_block` short-circuit until the call
                    // boundary reads this value as the function's result.
                    scope.returning = Some(v);
                    break;
                }
                ast::Stmt::Expr { expr, .. } => {
                    let v = self.eval(expr, if i == last { expected } else { None }, scope)?;
                    if i == last {
                        tail = Some(v);
                    }
                }
                // A `for` is a loop over a runtime value, which a constant
                // expression cannot fold (there is no loop in `ConstValue`).
                ast::Stmt::For { span, .. } => {
                    return Err(self.unsupported(*span, "a 'for' loop"));
                }
                ast::Stmt::Break { span } | ast::Stmt::Continue { span } => {
                    return Err(self.unsupported(*span, "a 'break' or 'continue'"));
                }
                // `defer` is lowered away before checking, so a constant
                // initializer never contains one.
                ast::Stmt::Defer { span, .. } => {
                    return Err(TypeError {
                        message: "internal error: a 'defer' reached the constant folder; it should have been lowered before checking".to_string(),
                        span: *span,
                    })
                }
            }
            if scope.halted() {
                break;
            }
        }
        scope.frames.pop();
        if scope.halted() {
            // Cloned rather than taken: an outer block in the same body still
            // has to see the same value (and the same flag).
            return Ok(ConstFlow::Returned(scope.returning.clone().flatten()));
        }
        Ok(ConstFlow::Fell(tail))
    }
}

/// Collects every `datac`/`enum` declared with type parameters (A-2) into the
/// template table. Done before anything is resolved, so a concrete type's
/// field can name an instantiation.
///
/// The checks that need the whole declaration set live here, because the main
/// declaration loop skips templates entirely: a template name must not
/// collide with another template, with a builtin type, or with a concrete
/// declaration of either kind, and a type parameter must not name a builtin
/// type (which would make `T` resolve to nothing at all).
fn collect_templates(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<HashMap<String, GenericTemplate>, Diagnostic> {
    let mut out: HashMap<String, GenericTemplate> = HashMap::new();
    // Every declaration, generic or not, so a collision between the two kinds
    // is caught here (the main loop never sees a template).
    let all: Vec<(String, Span, String, bool)> = program
        .datacs
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                d.span,
                d.module.clone(),
                !d.type_params.is_empty(),
            )
        })
        .chain(program.enums.iter().map(|e| {
            (
                e.name.clone(),
                e.span,
                e.module.clone(),
                !e.type_params.is_empty(),
            )
        }))
        .collect();
    for (name, span, module, generic) in all.iter() {
        if !generic {
            continue;
        }
        if Ty::from_name(name).is_some() {
            return Err(Diagnostic::new(
                format!("'{name}' is a builtin type and cannot be redeclared as a generic type"),
                *span,
                module.clone(),
            ));
        }
        if out.contains_key(name) {
            return Err(Diagnostic::new(
                format!("generic type '{name}' is defined more than once"),
                *span,
                module.clone(),
            ));
        }
        if all
            .iter()
            .any(|(other, _, _, other_generic)| !other_generic && other == name)
        {
            return Err(Diagnostic::new(
                format!("'{name}' is used for both a generic and a non-generic type"),
                *span,
                module.clone(),
            ));
        }
        let (params, kind, repr, const_decls) =
            if let Some(d) = program.datacs.iter().find(|d| &d.name == name) {
            (
                d.type_params.clone(),
                GenericKind::Record(d.fields.clone()),
                d.repr.clone().unwrap_or(Repr::C),
                d.const_params.clone(),
            )
        } else {
            let e = program
                .enums
                .iter()
                .find(|e| &e.name == name)
                .expect("the declaration lists name only declared types");
            (
                e.type_params.clone(),
                GenericKind::Sum(e.variants.clone()),
                e.repr.clone().unwrap_or(Repr::C),
                e.const_params.clone(),
            )
        };
        let const_params = const_param_tys(&const_decls, module)?;
        for p in &params {
            if Ty::from_name(p).is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "type parameter '{p}' shadows the builtin type '{p}' - pick another name"
                    ),
                    *span,
                    module.clone(),
                ));
            }
        }
        // `repr(...)` on a template is not threaded through an instantiation
        // yet; a generic type keeps the default C layout and says so rather
        // than silently ignoring the clause.
        if !matches!(repr, Repr::C) {
            return Err(Diagnostic::new(
                "'repr(...)' on a generic datac/enum is not supported yet: an instantiation takes the default 'repr(c)' layout"
                    .to_string(),
                *span,
                module.clone(),
            ));
        }
        // The declaring module's own prefix/alias table, so a field type
        // written inside the template resolves exactly where it was written.
        let (prefix, aliases) = (
            project.prefix_of(module).to_string(),
            project.alias_prefixes_of(module),
        );
        out.insert(
            name.clone(),
            GenericTemplate {
                name: name.clone(),
                params,
                const_params,
                kind,
                repr,
                prefix,
                aliases,
            },
        );
    }
    Ok(out)
}

/// Collects every `fun` declared with type parameters (A-2) into the function
/// template table, and checks the rules that need the whole declaration set:
/// `main` is never generic, a type parameter must not shadow a builtin type,
/// and a generic and a non-generic function may not share a name.
fn collect_func_templates(
    program: &ast::Program,
    project: &Project,
    generics: &HashMap<String, GenericTemplate>,
) -> std::result::Result<HashMap<String, FuncTemplate>, Diagnostic> {
    let mut out: HashMap<String, FuncTemplate> = HashMap::new();
    for (i, f) in program.functions.iter().enumerate() {
        let params = decl_type_params(f);
        if params.is_empty() {
            continue;
        }
        let key = sig_key(f);
        if f.receiver.is_none() && f.name == "main" {
            return Err(Diagnostic::new(
                "'main' cannot take type parameters - it is the program entry point"
                    .to_string(),
                f.span,
                f.module.clone(),
            ));
        }
        for p in &params {
            if Ty::from_name(p).is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "type parameter '{p}' shadows the builtin type '{p}' - pick another name"
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        let const_decls: &[ast::ConstParamDecl] = match &f.receiver {
            Some(r) => &r.const_params,
            None => &f.const_params,
        };
        let const_params = const_param_tys(const_decls, &f.module)?;
        if let Some(r) = &f.receiver {
            // A method on a generic type: the receiver writes the type's own
            // parameters, in order, and they are the method's type parameters
            // (A-2). A receiver that names a non-generic type has nothing to
            // parameterize, and one that writes the wrong names or order
            // would make the body's `T` mean something the type does not.
            let type_tmpl = generics.get(&r.type_name);
            let blanket = crate::types::is_blanket_receiver(&r.type_name);
            if !blanket && type_tmpl.is_none() && (!r.type_args.is_empty() || f.type_params.is_empty()) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not a generic type - only a receiver on a generic type may declare type parameters of the type; a method of a plain type may declare its own ('fun {}.{}<U>(...)')",
                        r.type_name, r.type_name, f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if let Some(type_tmpl) = type_tmpl {
                if type_tmpl.params != r.type_args {
                    return Err(Diagnostic::new(
                        format!(
                            "receiver '{}' must write the type's own type parameters, in order: 'fun {}<{}>.{}(...)'",
                            r.type_name,
                            r.type_name,
                            type_tmpl.params.join(", "),
                            f.name
                        ),
                        f.span,
                        f.module.clone(),
                    ));
                }
            }
            if let Some(own) = f.type_params.iter().find(|p| r.type_args.contains(p)) {
                return Err(Diagnostic::new(
                    format!("the method's type parameter '{own}' has the name of one of the receiver type's: pick another name"),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        if out.contains_key(&key)
            || program.functions.iter().enumerate().any(|(j, other)| {
                j != i && sig_key(other) == key && decl_type_params(other).is_empty()
            })
        {
            return Err(Diagnostic::new(
                format!(
                    "function '{}' is defined more than once",
                    display_callee(&key)
                ),
                f.span,
                f.module.clone(),
            ));
        }
        out.insert(
            key.clone(),
            FuncTemplate {
                key,
                params,
                const_params,
                bounds: {
                    let mut b: Vec<Vec<ast::TypeExpr>> = f
                        .receiver
                        .as_ref()
                        .map(|r| {
                            if f.receiver_bounds.len() == r.type_args.len() {
                                f.receiver_bounds.clone()
                            } else {
                                vec![Vec::new(); r.type_args.len()]
                            }
                        })
                        .unwrap_or_default();
                    b.extend(f.type_param_bounds.iter().cloned());
                    b
                },
                projection_bounds: f.projection_bounds.clone(),
                receiver: f
                    .receiver
                    .as_ref()
                    .map(|r| (r.type_name.clone(), r.mutable)),
                declared: f.params.clone(),
                ret: f.return_type.clone(),
                prefix: project.prefix_of(&f.module).to_string(),
                aliases: project.alias_prefixes_of(&f.module),
                ast_index: i,
            },
        );
    }
    Ok(out)
}

/// Walk the probe's call edges to a fixpoint: a function that calls a writer
/// holds the passdown exclusively too. Holding a *shared* borrow while a
/// callee writes through the same storage is the hazard - and the callee need
/// not name the passdown to do it, since a function that only forwards takes
/// the global's address itself - so the edge is followed whatever the callee
/// names. A promotion for a passdown a function never references is inert
/// (nothing reads the entry), which is what keeps a read-only reference
/// shared and the ergonomics this exists for. Monotone - modes are only ever
/// added - so this terminates.
fn close_pd_modes(
    exclusive_uses: &std::collections::HashSet<(String, String)>,
    edges: &std::collections::HashSet<(String, String)>,
) -> HashMap<(String, String), bool> {
    let mut modes: HashMap<(String, String), bool> =
        exclusive_uses.iter().map(|k| (k.clone(), true)).collect();
    loop {
        let mut changed = false;
        for (caller, callee) in edges {
            let inherited: Vec<String> = modes
                .iter()
                .filter(|((writer, _), held)| **held && writer == callee)
                .map(|((_, pd), _)| pd.clone())
                .collect();
            for pd in inherited {
                let key = (caller.clone(), pd);
                if !modes.get(&key).copied().unwrap_or(false) {
                    modes.insert(key, true);
                    changed = true;
                }
            }
        }
        if !changed {
            return modes;
        }
    }
}

/// The passdown borrow modes (roadmap item 17). Every `pd var` used to be
/// handed to every referencing function as an exclusive borrow, so a function
/// that takes the passdown as an argument *and* reads it - `fun show(s: &str)
/// { print(cfg) }` called as `show(cfg)` - was refused with a borrow
/// conflict, and a read-only `pd var` was as good as unusable.
///
/// The upgrade set is discovered rather than guessed at. A first *probe* pass
/// runs with every passdown shared: a use that needs an exclusive borrow is
/// recorded instead of refused, and every call site records an edge. The probe
/// is a real body check whose `tir` is thrown away - which is what lets the
/// answer come from the checker itself, including generic instances,
/// overloads and `mut` receivers, instead of from a re-implementation of
/// resolution that could disagree with it. `close_pd_modes` then walks the
/// edges to a fixpoint, and the pass that emits the program runs again with
/// the result. A program with no passdown writes at all keeps the probe's
/// result and is compiled once.
pub fn check_program(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<tir::Program, Diagnostic> {
    // A-13: the whole program's export set is known before any body is
    // checked, so the private-in-public rule runs here, once.
    check_pub_signatures(program, project)?;
    check_extern_type_names(program)?;
    let probe = std::rc::Rc::new(std::cell::RefCell::new(PdProbe::default()));
    let probe_wiring = PdWiring {
        modes: std::rc::Rc::new(HashMap::new()),
        probe: Some(probe.clone()),
    };
    let checked = check_program_pass(program, project, &probe_wiring)?;
    let exclusive_uses = probe.borrow().exclusive_uses.clone();
    if exclusive_uses.is_empty() {
        return Ok(checked);
    }
    let edges = probe.borrow().edges.clone();
    let final_wiring = PdWiring {
        modes: std::rc::Rc::new(close_pd_modes(&exclusive_uses, &edges)),
        probe: None,
    };
    Ok(check_program_pass(program, project, &final_wiring)?)
}

/// One full body-checking pass, under the passdown borrow modes `wiring`
/// carries. Called twice at most: once as the probe, and once with the modes
/// the probe's findings produced.
fn check_program_pass(
    program: &ast::Program,
    project: &Project,
    wiring: &PdWiring,
) -> std::result::Result<tir::Program, Diagnostic> {
    // A-2: the generic templates are collected before anything is resolved,
    // because a concrete declaration's field may name an instantiation
    // (`val p: Pair<i32, String>`) and the resolution of that field is what
    // creates it.
    let generics = collect_templates(program, project)?;
    // A-12: which user types declare their own `Map` key pair, decided from the
    // whole function list: a `datac` field may name `Map<T, V>` before any
    // signature has been resolved, so the key set has to be known that early.
    // The signature loop below then refuses a half or a malformed pair.
    let declared_keys = std::rc::Rc::new(declared_key_names(program));
    // Kernel stage 9, MK9-S6-M1: which user types declare `fun mut T.drop()`,
    // decided that early for the same reason as `declared_keys`: a container
    // element or `Map` key type may be resolved before any signature is, and
    // the derived-copy fixpoints must already exclude a destructor type.
    let declared_drops = std::rc::Rc::new(declared_drop_names(program));
    // A-12 slice 2: the program's traits, and the (trait, type) pairs its impls
    // declare, collected before any signature is resolved because a bound at a
    // call site is checked against the second of those and the first is what a
    // signature's `Self` is bound through.
    let traits = std::rc::Rc::new(collect_traits(program, project)?);
    let trait_impls = collect_trait_impls(program);
    // Coherence runs before the templates are collected, because two generic
    // impls of one type would otherwise be reported as one function defined twice.
    check_trait_impl_overlap(&traits, &trait_impls)?;
    let mut impls: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    let mut generic_impls: HashMap<String, Vec<GenericImpl>> = HashMap::new();
    let mut blanket_impls: Vec<BlanketImpl> = Vec::new();
    for imp in &trait_impls {
        if imp.type_params.contains(&imp.type_name) {
            if !imp.projection_bounds.is_empty() || imp.type_param_bounds.iter().flatten().any(|b| !b.bound_eq.is_empty()) {
                return Err(Diagnostic::new(
                    format!(
                        "an equality constraint or a 'where' projection bound on 'impl<...> {} for {}' is not in this slice: a blanket impl takes plain trait bounds, and a generic impl ('impl<T> {} for Type<T>') takes both",
                        imp.trait_name, imp.type_name, imp.trait_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            // `impl<T: Bound> Trait for T`: a blanket impl. It binds no associated
            // type yet, and its trait takes no arguments (the parser refuses those).
            if let Some((name, _, _)) = imp.assoc_types.first() {
                return Err(Diagnostic::new(
                    format!(
                        "'type {name}' in 'impl {} for {}': a blanket impl binds no associated type yet, so it writes the bound and the methods only",
                        imp.trait_name, imp.type_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            let bounds = impl_param_bounds(imp, &traits, project)?
                .into_iter()
                .next()
                .unwrap_or_default();
            blanket_impls.push(BlanketImpl {
                trait_key: imp.trait_instance.clone(),
                trait_name: imp.trait_name.clone(),
                bounds,
                methods: imp.methods.clone(),
                module: imp.module.clone(),
                impl_span: imp.impl_span,
            });
            continue;
        }
        if !imp.type_params.is_empty() {
            let bounds = impl_param_bounds(imp, &traits, project)?;
            let (eqs, projections) = impl_constraints(imp, &traits, project)?;
            generic_impls.entry(imp.trait_instance.clone()).or_default().push(GenericImpl {
                template: imp.type_name.clone(),
                bounds,
                assoc_types: imp.assoc_types.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect(),
                params: imp.type_params.clone(),
                eqs,
                projections,
                prefix: project.prefix_of(&imp.module).to_string(),
                aliases: project.alias_prefixes_of(&imp.module),
            });
            continue;
        }
        impls
            .entry(imp.trait_instance.clone())
            .or_default()
            .insert(imp.type_name.clone());
    }
    check_blanket_cycles(&blanket_impls)?;
    let env = TypeEnv {
        generics: std::rc::Rc::new(generics),
        instances: std::rc::Rc::new(std::cell::RefCell::new(InstanceState::default())),
        declared_keys: declared_keys.clone(),
        declared_drops: declared_drops.clone(),
        exported: std::rc::Rc::new(collect_exports(program)),
        // `resolve.rs` has prefixed every handle name, so the set is already
        // canonical. A handle is a type like any other and is known before any
        // body is checked, which is why a module may name another module's
        // handle in a signature as freely as a `datac`.
        handles: std::rc::Rc::new(
            program
                .extern_types
                .iter()
                .map(|e| e.name.clone())
                .collect(),
        ),
        handle_parents: std::rc::Rc::new(build_handle_parents(program, project)?),
        variadic_externs: std::rc::Rc::new(
            program
                .functions
                .iter()
                .filter(|f| f.extern_ && f.variadic)
                .map(|f| (f.name.clone(), f.module.clone()))
                .collect(),
        ),
        traits: traits.clone(),
        impls: std::rc::Rc::new(impls),
        trait_impls: std::rc::Rc::new(trait_impls.clone()),
        dyn_pairs: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        dyn_traits: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        assoc: std::rc::Rc::new(HashMap::new()),
        iter_nexts: std::rc::Rc::new(collect_iter_nexts(program)),
        generic_impls: std::rc::Rc::new(generic_impls),
        blanket_impls: std::rc::Rc::new(blanket_impls),
    };
    // A-2: the generic *function* templates, collected before signatures are
    // built so a call site can instantiate one on demand.
    let fns = FuncEnv {
        templates: std::rc::Rc::new(collect_func_templates(
            program,
            project,
            &env.generics,
        )?),
        instances: std::rc::Rc::new(std::cell::RefCell::new(FunctionState::default())),
    };
    let mut structs = HashMap::new();
    // Structs are resolved in declaration order, which is also the order
    // `codegen_c.rs` must emit their C typedefs in: a `datac` may only name
    // `datac`s declared before it (see `TypeScope::later_datacs`), so
    // declaration order is a valid topological order. Emitting them from a
    // `HashMap`'s iteration order instead - which is what this did before -
    // made a struct-embedding-struct program compile or fail *at random*
    // from one run to the next.
    let mut struct_order: Vec<String> = Vec::new();
    // A-1: `datac`s and `enum`s share one declaration order, because either
    // may name the other's earlier-declared half by value. `merged` is that
    // order, and `later_of` answers the "declared after this type" question
    // that turns a forward reference into an explanation instead of
    // "unknown type".
    let mut merged: Vec<(usize, String, bool)> = program
        .datacs
        .iter()
        .map(|d| (d.order, d.name.clone(), false))
        .chain(program.enums.iter().map(|e| (e.order, e.name.clone(), true)))
        .collect();
    merged.sort_by_key(|(order, _, _)| *order);
    let later_of: HashMap<String, std::collections::HashSet<String>> = merged
        .iter()
        .enumerate()
        .map(|(i, (_, name, _))| {
            (
                name.clone(),
                merged[i + 1..].iter().map(|(_, n, _)| n.clone()).collect(),
            )
        })
        .collect();
    // Compile-time constants (M6.3, A-19) are folded before any type is
    // declared, because canonical order puts every `const` before every
    // `datac`/`enum`, so a const's own declared type can only name a builtin,
    // and the struct table it is folded against is empty by construction.
    // Folding this early is what makes a const reachable from a *type*: a
    // `datac` field's array length (`[Entry; PAGE_COUNT]`, A-18) is written in
    // a field list, which is resolved long after a function body is.
    //
    // A fold may call a plain `fun` (A-19), and the table it calls through is
    // the declarations themselves, built here from the AST before any
    // signature exists, so a `const` and a `fun` may reference each other in
    // either order.
    let const_fns = std::rc::Rc::new(collect_const_fns(program, project));
    let consts = collect_consts(program, project, &HashMap::new(), &HashMap::new(), &env, &const_fns)?;

    let mut enums: HashMap<String, EnumDef> = HashMap::new();
    let mut enum_order: Vec<String> = Vec::new();
    for (order, name, is_enum) in &merged {
        // A-2: a generic `datac`/`enum` is a *template*, not a concrete type.
        // It gets no entry in either table and no declaration order; it is
        // instantiated on demand by `TypeScope::instantiate`.
        let template_only = if *is_enum {
            program
                .enums
                .iter()
                .any(|e| &e.name == name && !e.type_params.is_empty())
        } else {
            program
                .datacs
                .iter()
                .any(|d| &d.name == name && !d.type_params.is_empty())
        };
        if template_only {
            continue;
        }
        if *is_enum {
            let e = program
                .enums
                .iter()
                .find(|e| &e.name == name)
                .expect("merged order names only declared enums");
            if enums.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("enum '{}' is defined more than once", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if structs.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("'{}' is used for both a datac and an enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if traits.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("'{}' is used for both a trait and a datac/enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if Ty::from_name(&e.name).is_some() {
                return Err(Diagnostic::new(
                    format!("'{}' is a builtin type and cannot be redeclared as an enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            // Registered *before* its payloads are resolved, exactly as a
            // `datac` is: that is what lets a payload name the enum being
            // declared behind a `Box`/`List` (the recursive case), and
            // `in_progress` below is what rejects naming it by value.
            structs.insert(
                e.name.clone(),
                StructDef {
                    name: e.name.clone(),
                    fields: Vec::new(),
                    repr: Repr::C,
                    order: *order,
                },
            );
            let prefix = project.prefix_of(&e.module);
            let aliases = project.alias_prefixes_of(&e.module);
            // A payload of the enum being declared, *by value*, is the
            // infinite-size case `TypeScope::in_progress` rejects; behind a
            // `Box` or a `List` it is the recursive type (`Cons` holding
            // `Box<List2>`) the container rules already allow.
            let in_progress = Some(e.name.clone());
            let later_datacs = later_of[name].clone();
            let mut variants = Vec::new();
            for v in &e.variants {
                if variants
                    .iter()
                    .any(|vd: &crate::types::VariantDef| vd.name == v.name)
                {
                    return Err(Diagnostic::new(
                        format!(
                            "variant '{}' is defined more than once in enum '{}'",
                            v.name, e.name
                        ),
                        v.span,
                        e.module.clone(),
                    ));
                }
                let mut fields: Vec<FieldDef> = Vec::new();
                for f in &v.fields {
                    if fields.iter().any(|fd: &FieldDef| fd.name == f.name) {
                        return Err(Diagnostic::new(
                            format!(
                                "payload field '{}' is defined more than once in variant '{}'",
                                f.name, v.name
                            ),
                            f.span,
                            e.module.clone(),
                        ));
                    }
                    let ty = TypeScope {
                        structs: &structs,
                        enums: &enums,
                        env: env.clone(),
                        params: &EMPTY_PARAMS,
                        prefix,
                        aliases: &aliases,
                        later_datacs: Some(&later_datacs),
                        in_progress: in_progress.as_deref(),
                        consts: Some(&consts),
                        const_fns: Some(&const_fns),
                        handles: &env.handles,
                    }
                    .resolve(&f.ty, false)
                    .map_err(|err| err.in_module(&e.module))?;
                    if crate::types::is_borrowed_field_ty(&ty) {
                        return Err(Diagnostic::new(
                            format!(
                                "payload field '{}' is a borrowed type ('{}') - an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible - an owned 'String' is the way to keep text in one",
                                f.name,
                                ty.display()
                            ),
                            f.span,
                            e.module.clone(),
                        ));
                    }
                    fields.push(FieldDef {
                        name: f.name.clone(),
                        ty,
                        mutable: f.mutable,
                    });
                }
                variants.push(crate::types::VariantDef {
                    name: v.name.clone(),
                    fields,
                    discriminant: v.discriminant,
                });
            }
            enum_order.push(e.name.clone());
            let edef = EnumDef {
                name: e.name.clone(),
                variants,
                repr: e.repr.clone().unwrap_or(Repr::C),
                order: *order,
            };
            validate_enum_layout(&edef, e.span, &e.module)?;
            enums.insert(e.name.clone(), edef);
            // The placeholder inserted above stays: an enum name is a type
            // name like any other, so one name-resolution path covers it.
            // Reading a field off an enum is caught explicitly in
            // `check_expr`/`check_call`, where the real message lives.
            continue;
        }
        let d = program
            .datacs
            .iter()
            .find(|d| &d.name == name)
            .expect("merged order names only declared datacs");
        let later_datacs = later_of[name].clone();
        if enums.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a datac and an enum", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if structs.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("datac '{}' is defined more than once", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if traits.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a trait and a datac/enum", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if Ty::from_name(&d.name).is_some() {
            return Err(Diagnostic::new(
                format!("'{}' is a builtin type and cannot be redeclared as a datac", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        let prefix = project.prefix_of(&d.module);
        let aliases = project.alias_prefixes_of(&d.module);
        // A datac is visible *inside itself* (M8): the entry is a placeholder
        // whose fields are filled in below, which is all `resolve` needs to
        // name the type. A field of it by value is then rejected as an
        // infinite size (`TypeScope::in_progress`); behind a `Box` or a
        // `List` it is the recursive type the milestone is for.
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields: Vec::new(),
                repr: Repr::C,
                order: *order,
            },
        );
        let in_progress = Some(d.name.clone());
        let mut fields = Vec::new();
        for f in &d.fields {
            if fields.iter().any(|fd: &FieldDef| fd.name == f.name) {
                return Err(Diagnostic::new(
                    format!("field '{}' is defined more than once in datac '{}'", f.name, d.name),
                    f.span,
                    d.module.clone(),
                ));
            }
            let ty = TypeScope {
                structs: &structs,
                enums: &enums,
                env: env.clone(),
                params: &EMPTY_PARAMS,
                prefix,
                aliases: &aliases,
                later_datacs: Some(&later_datacs),
                in_progress: in_progress.as_deref(),
                consts: Some(&consts),
                const_fns: Some(&const_fns),
                handles: &env.handles,
            }
            .resolve(&f.ty, false)
            .map_err(|e| e.in_module(&d.module))?;
            // An owned field (`Bytes`, `String`, or a `datac` holding one) is
            // allowed from M6.2 on: `codegen_c.rs` generates a drop function
            // per datac, so dropping a struct recurses into its fields.
            if matches!(ty, Ty::Ref { .. } | Ty::Dyn { .. }) {
                return Err(Diagnostic::new(
                    format!(
                        "field '{}' is a borrowed type ('{}') - a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible - an owned 'String' is the way to keep text in one",
                        f.name,
                        ty.display()
                    ),
                    f.span,
                    d.module.clone(),
                ));
            }
            fields.push(FieldDef {
                name: f.name.clone(),
                ty,
                mutable: f.mutable,
            });
        }
        if let Some(Repr::Int(name)) = &d.repr {
            return Err(Diagnostic::new(
                format!(
                    "'repr({name})' names the integer type an enum tag is stored in: a 'datac' takes 'repr(c)', 'repr(packed)' or 'repr(align(N))'"
                ),
                d.span,
                d.module.clone(),
            ));
        }
        struct_order.push(d.name.clone());
        // Replaces the placeholder with the real definition. Order is kept
        // (same key, same position in `struct_order`), so codegen's emission
        // order is unchanged.
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields,
                repr: d.repr.clone().unwrap_or(Repr::C),
                order: *order,
            },
        );
    }

    // A-12 slice 2: the name-level half of trait validation, before any
    // signature is built, so a second impl of one trait for one type is
    // reported as a coherence error rather than as the duplicate method the
    // two blocks happen to share.
    check_trait_impl_names(program, &traits, &trait_impls, &env)?;

    // A-12 slice 5: every impl's associated-type bindings, resolved in the
    // impl's own module and checked against the trait's declarations. This runs
    // before any signature is built, because a `Self::Item` in an impl method
    // was already substituted by `resolve.rs` and a bound at a call site needs
    // the table. The result is folded into `env` before anything downstream
    // reads it.
    let assoc_bindings = collect_assoc_bindings(
        &traits,
        &trait_impls,
        &structs,
        &enums,
        &env,
        project,
        &consts,
        &const_fns,
    )?;
    let env = TypeEnv {
        assoc: std::rc::Rc::new(assoc_bindings),
        ..env.clone()
    };

    let mut sigs = HashMap::new();
    let mut overloads: HashMap<String, Vec<(Vec<Ty>, String)>> = HashMap::new();
    // Signature key per declaration, in declaration order: once overloads
    // exist a function's key is not derivable from its AST alone, and both
    // the passdown scan and the body checker need the same spelling the call
    // sites will use.
    let mut decl_keys: Vec<String> = Vec::with_capacity(program.functions.len());
    // A-12: one entry per declared half of a key pair, and one per type that
    // declared one, so a type with only half can be named with the span of the
    // half it did declare.
    let mut key_pair_halves: HashMap<String, KeyPairHalf> = HashMap::new();
    let mut key_pair_types: Vec<String> = Vec::new();
    for f in &program.functions {
        // A trait (A-12 slice 2) is named in a bound, which is a type
        // position, so it cannot share a name with a declared type.
        if f.receiver.is_none() && traits.contains_key(&f.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a function and a trait", f.name),
                f.span,
                f.module.clone(),
            ));
        }
        // A receiver method's key already can't collide with a struct name
        // (it always contains `#`), so this check only applies to plain
        // functions.
        if f.receiver.is_none() && structs.contains_key(&f.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a function and a datac", f.name),
                f.span,
                f.module.clone(),
            ));
        }
        // A declared key pair is a declaration on the concrete type, and it is
        // a shape the map calls directly rather than instantiating, so both of
        // these are refused before the generic-template skip below, which
        // would otherwise drop the declaration without a word. The rest of the
        // signature is checked once its types are resolved.
        if let Some(r) = &f.receiver {
            let half = f.name == crate::types::KEY_PAIR_HASH_METHOD
                || f.name == crate::types::KEY_PAIR_EQ_METHOD;
            if half && !r.type_args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot have type parameters: a key pair is declared on the type itself, so write it on '{}' rather than on the generic template",
                        r.type_name, f.name, r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if f.name == crate::types::DROP_METHOD && !r.type_args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.drop' cannot have type parameters: a destructor is declared on the type itself, so write it on '{}' rather than on the generic template",
                        r.type_name, r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if half && r.mutable {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot take 'mut': a key pair's receiver is a shared borrow, so write 'fun {}.{}()' without 'mut'",
                        r.type_name, f.name, r.type_name, f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            // A destructor type is not deep-copyable, and the map clones every
            // key in, so a declared key pair on one would be a second owner of
            // the resource the destructor frees.
            if half && env.declared_drops.contains(&r.type_name) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot be declared: a type with a destructor is not deep-copyable, and a key pair is cloned into the map, so the resource would be freed twice (kernel stage 9, MK9-S6-M1)",
                        r.type_name, f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        // A-2: a generic function has no signature until a call site
        // instantiates it; it is a template, so it gets a declaration key
        // (which the passdown scan needs) and no `sigs` entry.
        if !decl_type_params(f).is_empty() {
            decl_keys.push(sig_key(f));
            continue;
        }
        let prefix = project.prefix_of(&f.module);
        let aliases = project.alias_prefixes_of(&f.module);
        let scope = TypeScope {
            structs: &structs,
            enums: &enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(&consts),
            const_fns: Some(&const_fns),
            handles: &env.handles,
        };
        let mut params = Vec::new();
        if let Some(r) = &f.receiver {
            let ty = scope.resolve_receiver(r).map_err(|e| e.in_module(&f.module))?;
            // A-12 slice 3: an impl method on a builtin type is legal exactly
            // when the builtin already carries methods (`builtin_type_name`
            // names it); the `for` type is what `check_trait_impl_names`
            // vetted, so this is the same set. Every other method is still on
            // a declared `datac`/`enum`.
            if struct_name_of(&ty).is_none()
                && !(f.trait_impl.is_some() && builtin_type_name(&ty).is_some())
            {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is a builtin type - its methods are provided by the compiler and cannot be declared",
                        r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            params.push(("self".to_string(), ty));
        }
        for p in &f.params {
            let ty = scope.resolve_param(p).map_err(|e| e.in_module(&f.module))?;
            params.push((p.name.clone(), ty));
        }
        // `.return: T` never goes through `resolve_param_type`, so a function's
        // return type is always a plain owned `Ty` - never `Ty::Ref`. That's
        // what makes the borrow checker's call-scoped release sound: no
        // surface syntax exists yet for a function to hand a borrow back to
        // its caller, so a borrow can never escape the call frame that took
        // it.
        let ret = match &f.return_type {
            Some(t) => scope.resolve_return(t).map_err(|e| e.in_module(&f.module))?,
            None => Ty::Unit,
        };
        // A-12: `fun T.hash()` and `fun T.eq(other: &T)` on a user type are
        // that type's declared `Map` key pair, so the shape is checked here,
        // where the receiver, the parameters and the return type are already
        // resolved, and the type is recorded so the two halves can be matched
        // up once every declaration has been seen. Only `hash` makes the type
        // a key: a lone `eq` (H-5) is a valid declaration that gives the type
        // `==`/`!=`, so it is validated here but kept out of `key_pair_types`.
        if let Some(r) = &f.receiver {
            if f.name == crate::types::KEY_PAIR_HASH_METHOD
                || f.name == crate::types::KEY_PAIR_EQ_METHOD
            {
                check_key_pair_method(f, &params, &ret)?;
                if f.name == crate::types::KEY_PAIR_HASH_METHOD
                    && !key_pair_types.contains(&r.type_name)
                {
                    key_pair_types.push(r.type_name.clone());
                }
                key_pair_halves.entry(r.type_name.clone()).or_insert(KeyPairHalf {
                    method: if f.name == crate::types::KEY_PAIR_HASH_METHOD {
                        crate::types::KEY_PAIR_HASH_METHOD
                    } else {
                        crate::types::KEY_PAIR_EQ_METHOD
                    },
                    span: f.span,
                    module: f.module.clone(),
                });
            }
        }
        // Kernel stage 9, MK9-S6-M1: `fun mut T.drop()` on a user type is
        // that type's destructor, so its shape is checked here, where the
        // receiver, the parameters and the return type are resolved. The
        // duplicate-key check below is what refuses a second one, since both
        // spellings share the `T#drop` key.
        if f.receiver.is_some() && f.name == crate::types::DROP_METHOD {
            check_drop_method(f, &ret)?;
        }
        // A returned borrow has to have something to borrow from, which the
        // borrow checker enforces per return site (M6.3): either the first
        // reference parameter, or a `str` literal (whose storage is static,
        // which is what `fun classify(n: i32).return: &str = "pos"` needs).
        // `borrowck.rs` cannot be stricter here without knowing the body.
        let key = sig_key_for(f, prefix, &params);
        if sigs.contains_key(&key) {
            // A-12 slice 2: when either declaration is an `impl` method, the
            // collision is between a trait method and something the type
            // already has, so the message names the trait instead of the
            // generic "defined more than once".
            let other = program
                .functions
                .iter()
                .find(|o| !std::ptr::eq(*o, f) && sig_key(o) == sig_key(f));
            if let Some(message) = trait_method_collision_message(f, other) {
                return Err(Diagnostic::new(message, f.span, f.module.clone()));
            }
            let message = match &f.receiver {
                Some(r) => format!("method '{}.{}' is defined more than once", r.type_name, f.name),
                None => format!(
                    "function '{}' is defined more than once for the same parameter types",
                    display_callee(&key)
                ),
            };
            return Err(Diagnostic::new(message, f.span, f.module.clone()));
        }
        if f.receiver.is_none()
            && is_shipped_module(&f.module)
            && OVERLOADABLE.contains(&bare_name(&f.name, prefix))
        {
            let types = params.iter().map(|(_, t)| t.clone()).collect();
            overloads
                .entry(sig_key(f))
                .or_default()
                .push((types, key.clone()));
        }
        decl_keys.push(key.clone());
        sigs.insert(key, FnSig { params, ret });
    }

    // A-12 slice 2: every `impl Name for Type` block, checked against the
    // trait's own signatures now that both sides are resolved.
    check_trait_impl_signatures(
        program,
        &traits,
        &trait_impls,
        &structs,
        &enums,
        &sigs,
        &consts,
        &const_fns,
        &env,
    )?;

    // A-12: a declared `Map` key is both halves. `key_pair_types` holds the
    // types that declare `hash`, so a missing `eq` is refused here and the
    // message names the half that is missing; a half whose own signature was
    // wrong was already refused above. A lone `eq` is not in this list: it is
    // a valid declaration that gives the type `==`/`!=` and no key (H-5).
    for name in &key_pair_types {
        let has = |method: &str| {
            sigs.contains_key(&crate::types::key_pair_method_key(name, method))
        };
        let (hash, eq) = (
            has(crate::types::KEY_PAIR_HASH_METHOD),
            has(crate::types::KEY_PAIR_EQ_METHOD),
        );
        if hash == eq {
            continue;
        }
        let half = &key_pair_halves[name];
        let missing = if hash {
            crate::types::KEY_PAIR_EQ_METHOD
        } else {
            crate::types::KEY_PAIR_HASH_METHOD
        };
        return Err(Diagnostic::new(
            format!(
                "'{name}' declares '{}' but not '{missing}': a declared `Map` key needs both halves, 'fun {name}.hash().return: u64' and 'fun {name}.eq(other: &{name}).return: bool'",
                half.method
            ),
            half.span,
            half.module.clone(),
        ));
    }

    // A-12: the map keeps its own copy of every key it stores, so a key's type
    // has to be deep-copyable, which a derived key always is
    // (`value_is_keyable` names nothing `value_is_clonable` does not). A
    // declared pair is the escape hatch from the *key* fixpoint and not from
    // this one, so the same requirement is checked here rather than leaving a
    // backend to fail on a clone that was never emitted.
    {
        let instances = env.instances.borrow();
        let clonable = crate::types::clonable_types(
            &type_decls_of(&structs, &enums, &instances),
            &env.declared_drops,
        );
        for name in &key_pair_types {
            if clonable.contains(name) {
                continue;
            }
            let half = &key_pair_halves[name];
            return Err(Diagnostic::new(
                format!(
                    "'{name}' declares its own key pair, but the map clones a key in and '{name}' has no deep copy: a key may not hold a field that cannot be copied, so a `List`, `Map` or `Box` field (or a `datac`/`enum` that holds one) is refused"
                ),
                half.span,
                half.module.clone(),
            ));
        }
    }

    // ---- M5.2: passdowns -------------------------------------------------
    //
    // Order matters and is forced by what depends on what:
    //   1. sigs above are final *without* passdown entries, because a
    //      passdown is handed over as a hidden argument at call sites
    //      (see `PdInfo`) rather than as part of the signature's declared
    //      parameter list.
    //   2. pd types are then resolved (declared ones directly, inferred ones
    //      from a type-only probe of the initializer, with only the
    //      passdowns declared before that point visible).
    //   3. the wiring scan says which functions reference which passdowns.
    //   4. initializers and function bodies are checked for real.
    // Which user types own heap memory, transitively, computed once: the
    // array repeat literal and the drop planner both ask it (A-18).
    let owned_structs = crate::types::owned_types(
        &structs.values().cloned().collect::<Vec<_>>(),
        &enums.values().cloned().collect::<Vec<_>>(),
        &declared_drops,
    );

    // Kernel stage 7: every `static` is resolved and folded before any body or
    // passdown is checked, because both may name one and the backends need the
    // folded initializer.
    let (statics, static_decls) = collect_statics(
        program,
        project,
        &sigs,
        &structs,
        &enums,
        &consts,
        &const_fns,
        &owned_structs,
        &overloads,
        &env,
        &fns,
    )?;

    let all_pds = collect_pds(
        program,
        project,
        &sigs,
        &structs,
        &enums,
        &consts,
        &const_fns,
        &owned_structs,
        &overloads,
        &env,
        &fns,
        &statics,
    )?;
    let wired = scan_all(program, &all_pds, &structs, &decl_keys);

    // Kernel stage 4: a freestanding project has no hosted `main`. Its entry
    // point is the `fun` the manifest's `[target] entry` names, in the entry
    // module, and the same rule the hosted entry has holds for it: a program
    // without one is a compile error rather than an artifact with an entry
    // point nothing points at.
    let entry_key = if project.target().is_freestanding() {
        project.target().entry().unwrap_or("main")
    } else {
        "main"
    };
    if !sigs.contains_key(entry_key) {
        return Err(Diagnostic::new(
            if entry_key == "main" {
                "no 'main' function found".to_string()
            } else {
                format!(
                    "no entry function '{entry_key}' found: coral.toml's '[target] entry' names a `fun` with no parameters and no return in the entry module"
                )
            },
            Span::new(0, 0),
            project.entry.clone(),
        ));
    }

    let mut checked_pds = Vec::new();
    let mut initialized: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (decl, pd) in program.pds.iter().zip(all_pds.iter()) {
        let init = check_pd_initializer(
            &decl.init,
            pd,
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &initialized,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&pd.module))?;
        initialized.insert(pd.hidden.clone());
        checked_pds.push(tir::Pd {
            name: pd.hidden.clone(),
            qualified: pd.qualified.clone(),
            value_ty: pd.value_ty.clone(),
            init,
            module: decl.module.clone(),
        });
    }

    let mut functions = Vec::new();
    for (i, f) in program.functions.iter().enumerate() {
        // A-2: a generic function's body is checked once per instantiation,
        // in the worklist loop below - there is no signature to check it
        // against yet.
        if !decl_type_params(f).is_empty() {
            continue;
        }
        let key = &decl_keys[i];
        let sig = sigs[key].clone();
        let checked = check_function(
            f,
            key,
            &sig,
            &HashMap::new(),
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }

    // A-2: drain the instantiation worklist. Checking an instantiated body can
    // create another instantiation, so this runs until the queue is empty;
    // `MAX_FUNCTION_INSTANTIATIONS` is what turns an unbounded chain into a
    // diagnostic.
    loop {
        let next = fns.instances.borrow_mut().pending.pop_front();
        let Some((key, type_args)) = next else {
            break;
        };
        let Some(tmpl_key) = fns.instances.borrow().origin.get(&key).cloned() else {
            continue;
        };
        let tmpl = fns.templates[&tmpl_key].clone();
        let f = &program.functions[tmpl.ast_index];
        let sig = fns
            .instances
            .borrow()
            .sigs
            .get(&key)
            .cloned()
            .expect("an instantiation is registered before it is queued");
        let bindings: HashMap<String, Ty> = tmpl
            .params
            .iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect();
        let mut bindings = bindings;
        bindings.extend(assoc_projection_bindings(
            &env,
            &tmpl.params,
            &tmpl.bounds,
            &tmpl.prefix,
            &tmpl.aliases,
            &type_args,
        ));
        let checked = check_function(
            f,
            &key,
            &sig,
            &bindings,
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }
    // The capability data `codegen_c.rs` compiles in: each module's
    // patterns per domain, and which functions are gated (M7). Sorted, because
    // `Project::caps` is a map and a backend that emitted its entries in
    // iteration order would produce different IR for the same sources on two
    // runs (roadmap A-14 slice 1's determinism requirement).
    let mut caps: Vec<tir::CapsGrant> = project
        .caps()
        .iter()
        .flat_map(|(module, domains)| {
            domains.iter().map(|(domain, patterns)| tir::CapsGrant {
                module: module.clone(),
                domain: domain.clone(),
                patterns: patterns.clone(),
            })
        })
        .collect();
    caps.sort_by(|a, b| (&a.module, &a.domain).cmp(&(&b.module, &b.domain)));
    // A-2: the instantiations discovered on demand are part of the program's
    // declarations, and a hand-written type may contain one by value, so the
    // two sets are merged and given one topological order before codegen sees
    // them.
    let mut records: Vec<StructDef> = struct_order
        .iter()
        .map(|name| structs[name].clone())
        .collect();
    let mut sums: Vec<EnumDef> = enum_order.iter().map(|name| enums[name].clone()).collect();
    {
        let st = env.instances.borrow();
        // Sorted by name: the instances live in a map, and a vector in a hash
        // map's iteration order makes the same sources produce different IR on
        // two runs (the type-creation order, and with it every type's printed
        // number, would differ). `order_declarations` assigns each type's
        // `order` from a `(order, name)` sort, so the input order here decides
        // nothing else.
        let mut instances: Vec<StructDef> = st.records.values().cloned().collect();
        instances.sort_by(|a, b| a.name.cmp(&b.name));
        records.extend(instances);
        let mut instance_sums: Vec<EnumDef> = st.sums.values().cloned().collect();
        instance_sums.sort_by(|a, b| a.name.cmp(&b.name));
        sums.extend(instance_sums);
    }
    order_declarations(&mut records, &mut sums);
    // A-4: one function per `fun` literal, synthesized while bodies were
    // checked (a literal inside a literal's body included). They are ordinary
    // functions from here on - borrowck drops their owned parameters and
    // codegen emits them with a prototype like any other.
    functions.extend(fns.instances.borrow_mut().literals.drain(..));
    // Gap 7: the fixed-arity wrappers the program's variadic user externs
    // need. They are ordinary `extern fun`s to every later pass - an LLVM
    // declaration and a C body that forwards to the native symbol - so they
    // join the same list, and `extern_ffi_symbols` below points each one at
    // the extern's own native symbol.
    let variadic_wrappers: Vec<(String, tir::Function)> =
        fns.instances.borrow_mut().variadic_wrappers.drain(..).collect();
    functions.extend(variadic_wrappers.iter().map(|(_, f)| f.clone()));
    // The native symbol each user-declared `extern fun` binds to
    // (MK9-S1-M1, roadmap B-1), keyed by the function's full, prefixed name -
    // `codegen_c.rs::write_extern` reads this instead of `externs.rs` when a
    // function's module has no compiler-supplied body, and writes a
    // direct-call trampoline to the symbol. The map holds the renamed C
    // symbol when the manifest entry is `"kelp_name=c_symbol"`. Only a
    // function whose extern was actually *accepted* on this manifest path
    // reaches this map: the `FunctionBody::Extern` arm above already refused
    // every other extern this pass would otherwise have to re-validate here.
    let mut extern_ffi_symbols = HashMap::new();
    for f in &functions {
        if !f.extern_ {
            continue;
        }
        let bare = bare_name(&f.name, project.prefix_of(&f.module));
        if crate::externs::has_body(&f.module, bare) {
            continue;
        }
        if let Some(symbol) = project.extern_ffi_of(&f.module).and_then(|d| d.symbol_of(bare)) {
            extern_ffi_symbols.insert(f.name.clone(), symbol.to_string());
        }
    }
    // Each wrapper calls the same native symbol as the extern it wraps.
    for (extern_key, wrapper) in &variadic_wrappers {
        if let Some(symbol) = extern_ffi_symbols.get(extern_key) {
            extern_ffi_symbols.insert(wrapper.name.clone(), symbol.clone());
        }
    }

    // The `section("...")` consts, as data globals (kernel stage 4). A const
    // whose section names a place but whose value is not a fixed-size integer
    // array has no data to place, so it is refused here rather than silently
    // dropped; a const with no section is folded and inlined as before.
    let mut const_data: Vec<tir::ConstData> = Vec::new();
    for decl in &program.consts {
        let Some(section) = &decl.section else {
            continue;
        };
        let Some(info) = consts.get(&decl.name) else {
            continue;
        };
        match (&info.value.ty, &info.value.kind) {
            (Ty::Array(_, _), tir::ExprKind::ArrayLit(_)) => const_data.push(tir::ConstData {
                name: decl.name.clone(),
                module: decl.module.clone(),
                ty: info.value.ty.clone(),
                section: section.clone(),
                values: match &info.value.kind {
                    tir::ExprKind::ArrayLit(elems) => elems
                        .iter()
                        .map(|e| match &e.kind {
                            tir::ExprKind::IntLit(v) => *v,
                            _ => 0,
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            }),
            _ => {
                return Err(Diagnostic::new(
                    format!(
                        "'section(\"{section}\")' on const '{}' needs a fixed-size array of integers: that is the one const value a section can place",
                        bare_name(&decl.name, project.prefix_of(&decl.module))
                    ),
                    decl.span,
                    decl.module.clone(),
                ));
            }
        }
    }
    const_data.sort_by(|a, b| a.name.cmp(&b.name));

    // A-12 slice 4: one vtable per `(trait, type)` pair a coercion used, in a
    // stable order so both backends emit the same tables. Each slot names the
    // concrete method's own key and carries its signature, so the thunk a
    // backend generates needs no lookup of its own.
    let mut dyn_pairs: Vec<(String, String)> = env.dyn_pairs.borrow().iter().cloned().collect();
    dyn_pairs.sort();
    let mut dyn_vtables: Vec<tir::DynVtable> = Vec::new();
    for (trait_name, type_name) in dyn_pairs {
        let mut flat = Vec::new();
        flattened_trait_methods(&traits, &trait_name, &mut flat);
        let trait_prefix = traits
            .get(&trait_name)
            .map(|info| info.prefix.clone())
            .unwrap_or_default();
        let mut slots = Vec::new();
        for m in &flat {
            // A builtin `for` type keeps its written name in the impl record
            // (`impl Name for i32` stores `i32`), while `resolve.rs` qualifies
            // the method's receiver with the declaring module's prefix. So the
            // concrete key is the direct one for a `datac`/`enum` and, for a
            // builtin in a module that has a prefix, the prefixed spelling.
            let direct = format!("{type_name}#{}", m.name);
            let prefixed = format!("{trait_prefix}{type_name}#{}", m.name);
            let Some((target, sig)) = sigs
                .get(&direct)
                .map(|s| (direct.clone(), s))
                .or_else(|| sigs.get(&prefixed).map(|s| (prefixed.clone(), s)))
            else {
                // No concrete method for this pair: the impls table said the
                // pair exists, so a missing signature is an internal error.
                return Err(Diagnostic::new(
                    format!(
                        "internal: '&dyn {trait_name}' names method '{}', and no concrete method '{direct}' exists for '{type_name}'",
                        m.name
                    ),
                    m.span,
                    m.module.clone(),
                ));
            };
            slots.push(tir::DynVtableSlot {
                method: m.name.clone(),
                target,
                params: sig
                    .params
                    .iter()
                    .map(|(_, t)| t.clone())
                    .collect(),
                ret: sig.ret.clone(),
            });
        }
        // The concrete type the erased data pointer addresses: the receiver of
        // the concrete method, which is a borrow for every non-`Copy` type and
        // the value itself for a `Copy` builtin (A-12 slice 7). The vtable's
        // drop entry needs the `Ty` rather than the name, because dropping a
        // `Box<dyn Trait>` runs this type's own drop glue.
        let concrete = match slots.first().map(|s| s.params.first()) {
            Some(Some(Ty::Ref { inner, .. })) => inner.as_ref().clone(),
            Some(Some(t)) => t.clone(),
            _ => Ty::Unit,
        };
        dyn_vtables.push(tir::DynVtable {
            trait_name,
            type_name,
            concrete,
            slots,
        });
    }
    let mut dyn_traits: Vec<String> = env.dyn_traits.borrow().iter().cloned().collect();
    dyn_traits.sort();

    Ok(tir::Program {
        structs: records,
        enums: sums,
        pds: checked_pds,
        functions,
        caps,
        gated: project.gated().clone(),
        declared_keys: (*declared_keys).clone(),
        drop_types: (*declared_drops).clone(),
        extern_ffi_symbols,
        extern_headers: project.extern_headers(),
        handles: {
            let mut names: Vec<String> = program.extern_types.iter().map(|e| e.name.clone()).collect();
            names.sort();
            names
        },
        const_data,
        statics: static_decls,
        dyn_vtables,
        dyn_traits,
    })
}

/// The `Ty::Struct` names a value of `ty` contains *by value*, looking through
/// a borrow and stopping at a container: a `List`/`Box`/`Map` holds a pointer
/// and its typedef is emitted (with a forward-declared struct tag) before every
/// declaration anyway.
fn by_value_structs(ty: &Ty, out: &mut Vec<String>) {
    match ty {
        Ty::Struct(name) => out.push(name.clone()),
        Ty::Ref { inner, .. } => by_value_structs(inner, out),
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) => {}
        _ => {}
    }
}

/// Gives every declaration - hand-written and instantiated alike - a position
/// in one topological order (A-2). A type that contains another *by value* has
/// to be emitted after it, because C needs the complete type; hand-written
/// declarations already arrive in declaration order, which is topological
/// among themselves, but an instantiation can be named by a hand-written field
/// (`val p: Pair<i32, String>`), so the two sets have to be merged rather than
/// concatenated. Ties are broken by the incoming order, so the emitted C is
/// deterministic.
fn order_declarations(records: &mut [StructDef], sums: &mut [EnumDef]) {
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for r in records.iter() {
        let mut out = Vec::new();
        for f in &r.fields {
            by_value_structs(&f.ty, &mut out);
        }
        deps.insert(r.name.clone(), out);
    }
    for e in sums.iter() {
        let mut out = Vec::new();
        for v in &e.variants {
            for f in &v.fields {
                by_value_structs(&f.ty, &mut out);
            }
        }
        deps.insert(e.name.clone(), out);
    }

    let mut seed: Vec<(usize, String)> = records
        .iter()
        .map(|r| (r.order, r.name.clone()))
        .chain(sums.iter().map(|e| (e.order, e.name.clone())))
        .collect();
    seed.sort();

    fn visit(
        name: &str,
        deps: &HashMap<String, Vec<String>>,
        done: &mut HashMap<String, usize>,
        visiting: &mut std::collections::HashSet<String>,
        next: &mut usize,
    ) {
        if done.contains_key(name) || !visiting.insert(name.to_string()) {
            return;
        }
        if let Some(d) = deps.get(name) {
            for dep in d {
                if deps.contains_key(dep) {
                    visit(dep, deps, done, visiting, next);
                }
            }
        }
        visiting.remove(name);
        done.insert(name.to_string(), *next);
        *next += 1;
    }

    let mut done: HashMap<String, usize> = HashMap::new();
    let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next = 0usize;
    for (_, name) in &seed {
        visit(name, &deps, &mut done, &mut visiting, &mut next);
    }
    for r in records.iter_mut() {
        if let Some(o) = done.get(&r.name) {
            r.order = *o;
        }
    }
    for e in sums.iter_mut() {
        if let Some(o) = done.get(&e.name) {
            e.order = *o;
        }
    }
}

/// The passdowns of one module, keyed by the bare name a function writes.
/// `before` limits it to passdowns declared earlier - used while checking a
/// `pd` initializer, so "use before declaration" is a real error rather than
/// a silent read of a zero-initialized global.
fn pd_map(all: &[PdInfo], module: &str, before: Option<usize>) -> HashMap<String, PdInfo> {
    all.iter()
        .filter(|p| p.module == module && before.map_or(true, |i| p.index < i))
        .map(|p| (p.name.clone(), p.clone()))
        .collect()
}

/// Resolves every passdown's type, in merged-program order (module load
/// order, then declaration order within a module), checking for duplicate
/// declarations and name clashes on the way.
fn collect_pds(
    program: &ast::Program,
    project: &Project,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> std::result::Result<Vec<PdInfo>, Diagnostic> {
    let mut out: Vec<PdInfo> = Vec::new();
    for (index, decl) in program.pds.iter().enumerate() {
        // `resolve.rs` already prefixed `decl.name`; the bare name is what
        // functions in this module write.
        let prefix = project.prefix_of(&decl.module);
        let bare = decl
            .name
            .strip_prefix(prefix)
            .unwrap_or(&decl.name)
            .to_string();
        if out
            .iter()
            .any(|p| p.module == decl.module && p.name == bare)
        {
            return Err(Diagnostic::new(
                format!("passdown '{bare}' is declared more than once"),
                decl.span,
                decl.module.clone(),
            ));
        }
        if structs.contains_key(&decl.name)
            || statics.contains_key(&decl.name)
            || sigs.keys().any(|k| k == &decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a passdown and another top-level item", bare),
                decl.span,
                decl.module.clone(),
            ));
        }

        let aliases = project.alias_prefixes_of(&decl.module);
        let value_ty = match &decl.ty {
            Some(t) => TypeScope {
                structs,
                enums,
                env: env.clone(),
                params: &EMPTY_PARAMS,
                prefix,
                aliases: &aliases,
                later_datacs: None,
                in_progress: None,
                consts: Some(consts),
                const_fns: Some(const_fns),
                handles: &env.handles,
            }
            .resolve(t, false)
            .map_err(|e| e.in_module(&decl.module))?,
            None => {
                // Type-only probe: the initializer is checked again for real
                // once every passdown type (and the wiring table) is known.
                // `init_state` is set so diagnostics can say "a `pd`
                // initializer only sees passdowns declared before it".
                let mut ctx = base_ctx(
                    &decl.module,
                    sigs,
                    structs,
                    enums,
                    consts,
                    const_fns,
                    owned_structs,
                    overloads,
                    project,
                    &out,
                    None,
                    &HashMap::new(),
                    env,
                    fns,
                    statics,
                );
                ctx.init_state = Some(out.iter().map(|p| p.hidden.clone()).collect());
                check_expr(&decl.init, &mut ctx, None)
                    .map_err(|e| e.in_module(&decl.module))?
                    .ty
            }
        };
        // An owned passdown is allowed from M6.2 on: it lives in a generated
        // file-scope global and is freed by an `atexit` handler, which is
        // v1's only drop point at process exit (see `write_pd_globals`).
        out.push(PdInfo {
            qualified: decl.name.clone(),
            name: bare,
            hidden: format!("pd#{}", decl.name),
            value_ty,
            mutable: decl.mutable,
            module: decl.module.clone(),
            index,
        });
    }
    Ok(out)
}

/// Checks one `pd` initializer for real, with every passdown that is already
/// initialized visible and nothing else.
fn check_pd_initializer(
    init: &ast::Expr,
    pd: &PdInfo,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    wiring: &PdWiring,
    initialized: &std::collections::HashSet<String>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Result<tir::Expr> {
    let mut ctx = base_ctx(
        &pd.module,
        sigs,
        structs,
        enums,
        consts,
        const_fns,
        owned_structs,
        overloads,
        project,
        all,
        Some(pd.index),
        wired,
        env,
        fns,
        statics,
    );
    // An initializer has no hidden parameters of its own: it reads the
    // globals that earlier initializers already assigned.
    ctx.pd_global_borrow = true;
    ctx.init_state = Some(initialized.clone());
    ctx.fn_key = format!("pd#{}", pd.name);
    ctx.pd_modes = wiring.modes.clone();
    ctx.pd_probe = wiring.probe.clone();
    check_expr(init, &mut ctx, Some(&pd.value_ty))
}

fn base_ctx(
    module: &str,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    before: Option<usize>,
    wired: &HashMap<String, Vec<PdInfo>>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Ctx {
    let module_pds: Vec<&PdInfo> = all_pds.iter().filter(|p| p.module == module).collect();
    let pds = module_pds
        .iter()
        .filter(|p| before.map_or(true, |i| p.index < i))
        .map(|p| (p.name.clone(), (*p).clone()))
        .collect();
    let foreign_pds = all_pds
        .iter()
        .filter(|p| p.module != module)
        .map(|p| (p.name.clone(), p.module.clone()))
        .collect();
    Ctx {
        sigs: sigs.clone(),
        structs: structs.clone(),
        enums: enums.clone(),
        consts: consts.clone(),
        statics: statics.clone(),
        const_fns: const_fns.clone(),
        owned_structs: owned_structs.clone(),
        overloads: overloads.clone(),
        module: module.to_string(),
        caps: project.caps_of(module),
        gated: project.gated().clone(),
        has_extern_section: project.extern_ffi_of(module).is_some(),
        variadic_externs: env.variadic_externs.clone(),
        ret_ty: Ty::Unit,
        scopes: vec![HashMap::new()],
        prefix: project.prefix_of(module).to_string(),
        aliases: project.alias_prefixes_of(module),
        env: env.clone(),
        fns: fns.clone(),
        type_args: HashMap::new(),
        pds,
        foreign_pds,
        wired: wired.clone(),
        init_state: None,
        pd_global_borrow: false,
        own_hidden: Vec::new(),
        fn_key: String::new(),
        pd_modes: std::rc::Rc::new(HashMap::new()),
        pd_probe: None,
        in_fn_literal: false,
        borrowed_captures: false,
        outer_mut: Default::default(),
        excl_captures: Default::default(),
        outer_excl: Default::default(),
        excl_needed: Default::default(),
        outer_bindings: HashMap::new(),
        captures: Vec::new(),
        outer_receiver: None,
        receiver_struct: None,
        when_temps: 0,
        for_temps: 0,
        loop_depth: 0,
        tuple_temps: 0,
        defer_empty_container: false,
        probes: Vec::new(),
        unsafe_depth: 0,
        unsafe_fns: project.unsafe_fns().clone(),
        must_use_fns: project.must_use_fns().clone(),
        freestanding: project.target().is_freestanding(),
        cpus: project.target().cpus(),
        infer_scoped_ret: false,
    }
}

/// sig key -> the passdowns a function references. Computed from the AST
/// before bodies are checked, because a call site has to know what its
/// callee needs in order to pass it.
fn scan_all(
    program: &ast::Program,
    all: &[PdInfo],
    structs: &HashMap<String, StructDef>,
    decl_keys: &[String],
) -> HashMap<String, Vec<PdInfo>> {
    let mut wired = HashMap::new();
    for (i, f) in program.functions.iter().enumerate() {
        // `main` gets no hidden parameters (see `Ctx::pd_global_borrow`), and
        // it is not callable, so it never needs an entry here either.
        if f.receiver.is_none() && f.name == "main" {
            continue;
        }
        let visible = pd_map(all, &f.module, None);
        let used = scan_pd_uses(f, &visible, structs);
        if !used.is_empty() {
            wired.insert(decl_keys[i].clone(), used);
        }
    }
    wired
}

/// Scope-aware walk for the passdowns a function body actually names.
/// Shadowing is honored exactly as `check_expr` honors it: a parameter,
/// receiver field, or `val`/`var` in scope hides a passdown of the same name,
/// so a hidden name is not a passdown reference. A method call's name, a
/// field access's field name, and a call's callee are never passdown
/// references either - only bare identifiers are.
struct PdScan {
    scopes: Vec<std::collections::HashSet<String>>,
    /// Field names of the current receiver's `datac`, which shadow passdowns
    /// the same way locals do.
    receiver_fields: std::collections::HashSet<String>,
    /// Fields only count as shadowing inside a receiver-form function.
    has_receiver: bool,
    visible: std::collections::HashSet<String>,
    used: Vec<String>,
}

impl PdScan {
    fn bound(&self, name: &str) -> bool {
        (self.has_receiver && self.receiver_fields.contains(name))
            || self.scopes.iter().rev().any(|s| s.contains(name))
    }

    fn define(&mut self, name: &str) {
        self.scopes.last_mut().unwrap().insert(name.to_string());
    }

    fn note(&mut self, name: &str) {
        if !self.bound(name) && self.visible.contains(name) && !self.used.iter().any(|u| u == name)
        {
            self.used.push(name.to_string());
        }
    }

    fn scan_block(&mut self, block: &ast::Block) {
        self.scopes.push(std::collections::HashSet::new());
        for stmt in &block.stmts {
            self.scan_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn scan_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Let { name, init, .. } => {
                // Initializer first: `val name = name` reads the outer one.
                self.scan_expr(init);
                self.define(name);
            }
            ast::Stmt::LetTuple { pattern, init, .. } => {
                // The same order as a `let`: the initializer is read before
                // any position it binds exists, so `val (a, b) = a` reads the
                // outer `a`.
                self.scan_expr(init);
                let mut names: Vec<(String, Vec<usize>)> = Vec::new();
                pattern.binds(&mut Vec::new(), &mut names);
                for (name, _) in &names {
                    self.define(name);
                }
            }
            ast::Stmt::While { cond, body, .. } => {
                self.scan_expr(cond);
                self.scan_block(body);
            }
            ast::Stmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.scan_expr(iterable);
                self.scopes.push(std::collections::HashSet::new());
                self.define(binding);
                for stmt in &body.stmts {
                    self.scan_stmt(stmt);
                }
                self.scopes.pop();
            }
            ast::Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.scan_expr(v);
                }
            }
            ast::Stmt::Expr { expr, .. } => self.scan_expr(expr),
            ast::Stmt::Defer { expr, .. } => self.scan_expr(expr),
            ast::Stmt::Break { .. } | ast::Stmt::Continue { .. } => {}
        }
    }

    fn scan_expr(&mut self, expr: &ast::Expr) {
        match expr {
            // A `fun` literal's body is a function of its own, and it cannot
            // read a passdown at all (A-4), so the enclosing scan does not
            // descend into it.
            ast::Expr::FnLit { .. } => {}
            ast::Expr::Ident(name, _) => self.note(name),
            ast::Expr::Unary { expr, .. }
            | ast::Expr::Ref { expr, .. }
            | ast::Expr::Try { expr, .. }
            | ast::Expr::Deref { expr, .. } => self.scan_expr(expr),
            ast::Expr::Binary { lhs, rhs, .. } => {
                self.scan_expr(lhs);
                self.scan_expr(rhs);
            }
            ast::Expr::Call { args, .. } => {
                for a in args {
                    self.scan_expr(&a.value);
                }
            }
            ast::Expr::ArrayLit { elems, .. } => {
                for e in elems {
                    self.scan_expr(e);
                }
            }
            ast::Expr::ArrayRepeat { value, len, .. } => {
                self.scan_expr(value);
                self.scan_expr(len);
            }
            ast::Expr::TupleLit { elems, .. } => {
                for e in elems {
                    self.scan_expr(e);
                }
            }
            // Only the base - a field named like a passdown isn't a read of
            // it.
            ast::Expr::FieldAccess { base, .. } => self.scan_expr(base),
            ast::Expr::Assign { target, value, .. } => {
                self.scan_expr(target);
                self.scan_expr(value);
            }
            ast::Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.scan_expr(cond);
                self.scan_block(then_block);
                match else_block.as_deref() {
                    Some(ast::ElseBranch::Block(b)) => self.scan_block(b),
                    Some(ast::ElseBranch::If(e)) => self.scan_expr(e),
                    None => {}
                }
            }
            ast::Expr::Block(b) => self.scan_block(b),
            ast::Expr::MethodCall { receiver, args, .. } => {
                self.scan_expr(receiver);
                for a in args {
                    self.scan_expr(&a.value);
                }
            }
            ast::Expr::Template { parts, .. } => {
                for p in parts {
                    if let ast::TemplatePart::Expr(e) = p {
                        self.scan_expr(e);
                    }
                }
            }
            // A pattern binding shadows a passdown of the same name inside
            // its arm body, exactly as a `val` does, so the bindings push a
            // scope of their own (A-1).
            ast::Expr::When {
                scrutinee, arms, ..
            } => {
                self.scan_expr(scrutinee);
                for arm in arms {
                    let mut shadowed = std::collections::HashSet::new();
                    if let Some(p) = &arm.pattern {
                        for b in p.binds.iter().flatten() {
                            shadowed.insert(b.clone());
                        }
                    }
                    self.scopes.push(shadowed);
                    self.scan_expr(&arm.body);
                    self.scopes.pop();
                }
            }
            ast::Expr::IntLit(..)
            | ast::Expr::CharLit(..)
            | ast::Expr::ByteLit(..)
            | ast::Expr::FloatLit(..)
            | ast::Expr::StringLit(..)
            | ast::Expr::BoolLit(..) => {}
            // An `unsafe { ... }` block's body is scanned like any other
            // block: a passdown named inside one is still a passdown the
            // enclosing function has to receive.
            ast::Expr::Unsafe { block, .. } => self.scan_block(block),
            // The operand expressions of an `asm(...)` are scanned for the same
            // reason: a place or a value inside one may name a passdown.
            ast::Expr::Asm { operands, .. } => {
                for op in operands {
                    if let Some(e) = &op.operand {
                        self.scan_expr(e);
                    }
                }
            }
        }
    }
}

fn scan_pd_uses(
    f: &ast::FunctionDecl,
    visible: &HashMap<String, PdInfo>,
    structs: &HashMap<String, StructDef>,
) -> Vec<PdInfo> {
    if visible.is_empty() {
        return Vec::new();
    }
    let mut scan = PdScan {
        scopes: Vec::new(),
        receiver_fields: std::collections::HashSet::new(),
        has_receiver: f.receiver.is_some(),
        visible: visible.keys().cloned().collect(),
        used: Vec::new(),
    };
    scan.scopes.push(std::collections::HashSet::new());
    if let Some(r) = &f.receiver {
        // `resolve.rs` already prefixed the receiver type name.
        if let Some(sdef) = structs.get(&r.type_name) {
            scan.receiver_fields = sdef.fields.iter().map(|f| f.name.clone()).collect();
        }
        scan.define("self");
    }
    for p in &f.params {
        scan.define(&p.name);
    }
    match &f.body {
        ast::FunctionBody::Block(b) => scan.scan_block(b),
        ast::FunctionBody::Expr(e) => scan.scan_expr(e),
        // An `extern` has no body to scan for passdown references, and a
        // trait signature has none either.
        ast::FunctionBody::Extern | ast::FunctionBody::Signature => {}
    }

    let mut used: Vec<PdInfo> = scan
        .used
        .iter()
        .map(|name| visible[name].clone())
        .collect();
    // Declaration order, not reference order - so the hidden parameters (and
    // therefore the generated C signatures) are stable.
    used.sort_by_key(|p| p.index);
    used
}

/// Resolves a parameter's type per `plan/syntax.md` §5: a `datac` param is
/// an implicit shared borrow unless `mut`-marked (exclusive); everything
/// else (Copy types) stays a plain by-value type unless explicitly
/// `mut`-marked, since borrowing a Copy scalar by default would add
/// indirection nobody asked for. `move T` (A-4) is the third form, and the
/// only one that resolves an owned type to the bare type: the callee takes
/// ownership, so `TypeScope::resolve_param` handles it before this function
/// is reached. See `TypeScope::resolve_param` and `resolve_receiver` for the
/// M5 module-qualification rule.
fn implicit_borrow_wrap(base: Ty, mut_marked: bool) -> Ty {
    if mut_marked {
        Ty::Ref { mutable: true, inner: Box::new(base) }
    } else if base.is_copy() {
        // A `Copy` value is passed by value: copying one is copying a scalar, a
        // pair of words, or a pointer, so a borrow would only add an
        // indirection. That is `str`, `View`, a function value and the scalars.
        base
    } else {
        // Everything that owns memory, so the caller keeps it. This is the whole
        // list: `datac`/`enum`, `Bytes`, `String`, the containers, a closure -
        // and a fixed-size array whose element owns something (A-18). Asking
        // `is_copy` rather than listing the types is what keeps the two in step:
        // an array of scalars is a value and is passed like one, while
        // `[String; 2]` is an owner, so a plain parameter borrows it exactly as
        // a `String` parameter does instead of consuming the caller's array.
        Ty::Ref { mutable: false, inner: Box::new(base) }
    }
}

/// Unwraps a (possibly borrowed) type down to the `datac` name it names, if
/// any - used by field access/assignment so `.field` auto-derefs through a
/// `Ref` base exactly like an owned struct.
fn struct_name_of(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(s) => Some(s),
        Ty::Ref { inner, .. } => struct_name_of(inner),
        _ => None,
    }
}

/// The message types `panic` takes: the borrowed `str` every call site
/// already had, and an owned `String` (MISSING-8). `print`'s set is wider on
/// purpose (numbers, `bool`, `Bytes`); a panic message that renders as a
/// length-and-pointer pair is a string in every case.
fn is_panic_message(ty: &Ty) -> bool {
    is_str_like(ty)
        || matches!(ty, Ty::String)
        || matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::String))
}

/// Whether a type is one of the builtin owned buffers, or a borrow of one -
/// the types `print` renders straight out of their buffer.
fn is_printable_buffer(ty: &Ty) -> bool {
    match ty {
        // A `View` is printable because its length travels with it: `print`
        // emits `%.*s` from the pair, so a slice prints as the slice.
        Ty::Bytes | Ty::String | Ty::View => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Bytes | Ty::String),
        _ => false,
    }
}

/// `print`/templates read an owned buffer without consuming it, so a *named*
/// one is borrowed here rather than passed by value (which would be a move).
///
/// A temporary is passed through as the owned value it is: `codegen_c.rs`
/// hoists it into a C temporary and frees it right after the `printf`, which
/// is what makes `print(str.to_upper(s))` allocate once, print, and free
/// (M6.3 - until then this was a diagnostic telling the author to bind a
/// `val` first).
fn borrow_for_print(value: tir::Expr, _what: &str, _ctx: &Ctx) -> Result<tir::Expr> {
    if !matches!(value.ty, Ty::Bytes | Ty::String) {
        return Ok(value);
    }
    if !is_tir_place(&value) {
        return Ok(value);
    }
    let inner = value.ty.clone();
    let span = value.span;
    Ok(tir::Expr {
        kind: tir::ExprKind::Ref {
            mutable: false,
            expr: Box::new(value),
        },
        ty: Ty::Ref {
            mutable: false,
            inner: Box::new(inner),
        },
        span,
    })
}

/// A "place": something with stable storage that can be borrowed or
/// assigned into - an `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`).
fn is_tir_place(expr: &tir::Expr) -> bool {
    matches!(
        expr.kind,
        tir::ExprKind::Ident(_)
            | tir::ExprKind::StaticRef(_)
            | tir::ExprKind::FieldAccess { .. }
            | tir::ExprKind::EnumField { .. }
            // A capture is a place too: the environment holds the value (A-4
            // phase E), so `prefix.as_str()` inside a literal borrows it
            // rather than borrowing a temporary.
            | tir::ExprKind::Capture { .. }
            | tir::ExprKind::Deref { .. }
    )
}

/// A place whose storage is still there after the enclosing statement, which
/// is what a returned borrow and an exclusive borrow both need.
///
/// `is_tir_place` is the weaker question, "can an address be taken?", and
/// a field of a temporary answers yes: `codegen_c.rs` stages the temporary so
/// the field has an address. That address dies with the statement, though, so
/// `mkName().name.as_str()` is a borrow of a temporary the same way
/// `String.from_str("x").as_str()` is, one projection deeper, and it needs the
/// same refusal. A `Deref` is durable, and so is any chain that steps through
/// one: what a pointer points at is somebody else's storage, which is what
/// makes `headers.get(i).name.as_str()` fine where `mkName().name.as_str()` is
/// not.
fn is_durable_place(expr: &tir::Expr) -> bool {
    match &expr.kind {
        tir::ExprKind::FieldAccess { base, .. } | tir::ExprKind::EnumField { base, .. } => {
            matches!(base.ty, Ty::Ref { .. }) || is_durable_place(base)
        }
        tir::ExprKind::Deref { .. } => true,
        _ => is_tir_place(expr),
    }
}

/// `ast::is_place`, kept callable under its old local name at every existing
/// call site - the definition itself now lives in `ast.rs` so `parser.rs` can
/// share it (see `parse_if_let`).
fn is_ast_place(expr: &ast::Expr) -> bool {
    ast::is_place(expr)
}

/// Whether a checked place-expression's storage is currently writable:
/// either its root is a plain owned `var`-bound local, or its root (or the
/// pointer stepped through by a `*`) is statically typed as an *exclusive*
/// `Ty::Ref`. Used both by assignment (`p.x = ...`, `*p = ...`) and by
/// `&mut expr` (taking an exclusive borrow requires the place already be
/// writable).
fn check_target_writable(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &target.kind {
        // A static's own mutability decides whether it can be assigned, once
        // the `unsafe` gate has already let the name through (kernel stage 7).
        tir::ExprKind::StaticRef(name) => match ctx.statics.get(name) {
            Some(s) if s.mutable => Ok(()),
            Some(s) => Err(TypeError {
                message: format!(
                    "cannot assign to 'static {}': it is not declared 'static mut'",
                    s.name
                ),
                span: target.span,
            }),
            None => Ok(()),
        },
        tir::ExprKind::Ident(name) => {
            let Some(info) = ctx.lookup(name) else {
                return Err(TypeError {
                    message: format!("cannot assign to '{name}'"),
                    span: target.span,
                });
            };
            match &info.ty {
                Ty::Ref { mutable, .. } => {
                    if *mutable {
                        Ok(())
                    } else if info.mutable {
                        // Reassigning a `var` that *holds* a shared borrow
                        // rebinds the pointer; it does not write through it,
                        // so it needs no exclusivity. This is what makes a
                        // cursor over a borrowed structure expressible
                        // (`var cur = list` ... `cur = cur.next.get()`), which
                        // a recursive type needs before it can be walked
                        // iteratively (M8).
                        Ok(())
                    } else {
                        Err(TypeError {
                            message: format!("cannot write to '{name}': it is a shared borrow, not 'mut'"),
                            span: target.span,
                        })
                    }
                }
                _ => {
                    if info.mutable {
                        Ok(())
                    } else {
                        Err(TypeError {
                            message: format!("cannot assign to '{name}': not declared 'var'"),
                            span: target.span,
                        })
                    }
                }
            }
        }
        tir::ExprKind::FieldAccess { base, .. } => check_target_writable(base, ctx),
        tir::ExprKind::Deref { expr: inner } => match &inner.ty {
            Ty::Ref { mutable: true, .. } => Ok(()),
            Ty::Ref { mutable: false, .. } => {
                // A `pd val` read is a Deref of a shared borrow, so it lands
                // here - worth the passdown's own wording, since "shared
                // borrow" is not what the author wrote (M6.4). A `pd var`
                // holds an exclusive borrow exactly when its body writes it,
                // so a write that arrives through a shared borrow is either
                // the probe pass learning that this function must be upgraded
                // (recorded, allowed, and discarded) or a closure that missed
                // a use (roadmap item 17).
                if let Some(name) = pd_place_hidden(inner) {
                    if let Some(pd) = ctx.pds.values().find(|p| p.hidden == name).cloned() {
                        if !pd.mutable {
                            return Err(TypeError {
                                message: format!(
                                    "cannot assign to passdown '{}': it is declared 'pd val' (only a 'pd var' can be assigned inside a function)",
                                    pd.qualified
                                ),
                                span: target.span,
                            });
                        }
                        if ctx.note_pd_exclusive_use(&pd) {
                            return Ok(());
                        }
                        return Err(TypeError {
                            message: format!(
                                "cannot write to passdown '{}' through a shared borrow",
                                pd.qualified
                            ),
                            span: target.span,
                        });
                    }
                }
                Err(TypeError {
                    message: "cannot write through a shared borrow".to_string(),
                    span: target.span,
                })
            }
            _ => unreachable!("a Deref's operand is always Ref-typed, guaranteed by check_expr"),
        },
        // A write through a captured binding (`plan/exclusive-captures.md`).
        // A scoped block's capture of an owned `var` is typed `&T` until a
        // write (or a `mut` argument) shows it needs `&mut T`;
        // `check_fn_literal` retries the literal with the capture upgraded,
        // so reaching here with a shared type means the capture could not be
        // (or is being probed), and the message says which.
        tir::ExprKind::Capture { index } => {
            let name = ctx.captures.get(*index).map(|(n, _)| n.clone()).unwrap_or_default();
            match &target.ty {
                Ty::Ref { mutable: true, .. } => Ok(()),
                Ty::Ref { mutable: false, .. } => {
                    note_excl_need(target, ctx);
                    let outer_borrow = ctx
                        .outer_bindings
                        .get(&name)
                        .is_some_and(crate::types::is_borrowed_field_ty);
                    let message = if outer_borrow {
                        format!("cannot write to captured '{name}': it is a shared borrow in the enclosing function, so a block cannot write through it; take it as a 'mut' parameter there")
                    } else if ctx.outer_mut.contains(&name) {
                        format!("cannot write to captured '{name}': it is captured by shared borrow here")
                    } else {
                        format!("cannot write to captured '{name}': it is not declared 'var' in the enclosing function; declare it 'var' so a scoped block can write it")
                    };
                    Err(TypeError { message, span: target.span })
                }
                _ => {
                    note_excl_need(target, ctx);
                    Err(TypeError {
                    message: if ctx.borrowed_captures && ctx.outer_mut.contains(&name) {
                        format!("cannot write to captured '{name}': it is copied in by value here")
                    } else if ctx.borrowed_captures {
                        format!("cannot write to captured '{name}': it is not declared 'var' in the enclosing function; declare it 'var' so a scoped block can write it")
                    } else {
                        format!("cannot write to captured '{name}': a closure owns its own copy of it, so a write would change only that copy; return the new value, or use a scoped block over a 'var'")
                    },
                    span: target.span,
                    })
                }
            }
        }
        // A function name, a literal or a call result is not somewhere a value
        // can be stored (`f -= 1`, `1 = x`).
        _ => Err(TypeError {
            message: "cannot assign to this expression: it is not a variable, a field or an element".to_string(),
            span: target.span,
        }),
    }
}


/// The enclosing bindings a scoped literal may capture exclusively: writable
/// (`var`), not a borrow, not a function value.
fn writable_outer(ctx: &Ctx) -> std::collections::HashSet<String> {
    let mut m = std::collections::HashSet::new();
    for scope in ctx.scopes.iter() {
        for (k, v) in scope.iter() {
            if v.mutable
                && !crate::types::is_borrowed_field_ty(&v.ty)
                && !matches!(v.ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. })
            {
                m.insert(k.clone());
            } else {
                m.remove(k);
            }
        }
    }
    m
}

/// Records that the place `e`, a capture of the scoped literal being checked,
/// was used where an exclusive borrow is needed, if the capture is one the
/// literal may upgrade (`plan/exclusive-captures.md`). `check_fn_literal`'s
/// retry loop reads the set.
fn note_excl_need(e: &tir::Expr, ctx: &Ctx) {
    if let tir::ExprKind::Capture { index } = &e.kind {
        if let Some((name, _)) = ctx.captures.get(*index) {
            if ctx.borrowed_captures && ctx.outer_bindings.contains_key(name) {
                ctx.excl_needed.borrow_mut().insert(name.clone());
            }
        }
    }
}

/// `Type.name` for a receiver method, `name` for a plain function - for
/// diagnostics only (`sig_key` is the lookup key; this is what a person
/// reads).
fn display_name(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}.{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

/// Checks the native-callback parameters of a user `extern fun` (gap 4).
///
/// A `move closure(...)` parameter is the one shape the C backend expands in
/// place into a `(thunk, env, destroy)` triple, so the closure's own signature
/// must be one the generated thunk can forward by value: every parameter and
/// the return are a scalar, `bool`, `str`, an opaque handle or `unit`, no
/// parameter takes ownership or an exclusive borrow, and the function has at
/// most one such parameter. A plain `closure(...)` parameter is refused by
/// name: the native side has to own the closure, or the `destroy` notify has
/// nothing safe to free.
fn check_extern_callback_params(f: &ast::FunctionDecl, sig: &FnSig) -> Result<()> {
    let declared = if f.receiver.is_some() {
        &sig.params[1..]
    } else {
        &sig.params[..]
    };
    let mut callbacks = 0usize;
    for (p, (_, ty)) in f.params.iter().zip(declared.iter()) {
        match ty {
            Ty::Closure { params, ret } => {
                callbacks += 1;
                let written = p.ty.display();
                if let Some(written_params) = &p.ty.fn_params {
                    for (wp, resolved) in written_params.iter().zip(params.iter()) {
                        if wp.mode != ast::ParamMode::Borrow {
                            return Err(TypeError {
                                message: format!(
                                    "the callback closure type '{written}' takes '{}' by ownership or an exclusive borrow, but a native callback thunk forwards every argument by value: write the closure parameter as a scalar, 'bool', 'str', an opaque handle or 'unit'",
                                    wp.display()
                                ),
                                span: p.ty.span,
                            });
                        }
                        if !is_callback_abi_ty(resolved) {
                            return Err(TypeError {
                                message: format!(
                                    "the callback closure type '{written}' passes '{}' by value, which a native callback thunk cannot forward: write the closure parameter as a scalar, 'bool', 'str', an opaque handle or 'unit'",
                                    wp.ty.display()
                                ),
                                span: p.ty.span,
                            });
                        }
                    }
                }
                if !is_callback_abi_ty(ret) {
                    return Err(TypeError {
                        message: format!(
                            "the callback closure type '{written}' returns '{}', which a native callback thunk cannot forward: a callback closure must return a scalar, 'bool', 'str', an opaque handle or 'unit'",
                            ret.display()
                        ),
                        span: p.ty.span,
                    });
                }
            }
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => {
                return Err(TypeError {
                    message: format!(
                        "the parameter '{}' of an 'extern fun' is a closure handed to native code, which must own it: write the type 'move {}' so the destroy notify has an environment to free",
                        p.name,
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            _ => {}
        }
    }
    if callbacks > 1 {
        return Err(TypeError {
            message: format!(
                "'{}' takes {callbacks} callback closures, but a user 'extern fun' can expand at most one 'move closure(...)' parameter in this pass",
                f.name
            ),
            span: f.span,
        });
    }
    Ok(())
}

/// The types a native callback thunk forwards by value (gap 4): the scalars,
/// `bool`, `str`, an opaque handle and `unit`. Everything else would need a
/// pointer the language cannot spell yet.
fn is_callback_abi_ty(ty: &Ty) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char | Ty::Str | Ty::Unit | Ty::Handle(_))
}

fn check_function(
    f: &ast::FunctionDecl,
    key: &str,
    sig: &FnSig,
    type_args: &HashMap<String, Ty>,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    wiring: &PdWiring,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Result<tir::Function> {
    // The `...` variadic marker (gap 7) is legal in exactly one place: a
    // *user* `extern fun`, meaning one whose module a `coral.toml`
    // `[extern."<module id>"]` section names. A compiler-shipped `extern
    // fun` cannot carry one (its fixed C body has no way to read extras), a
    // plain function cannot (there is no C call to pass them to), and a
    // generic one cannot, because the wrapper a call site names is fixed by
    // the instantiation and nothing here would specialize it.
    if f.variadic {
        if !f.extern_ {
            return Err(TypeError {
                message: format!(
                    "'{}' writes '...', which is only legal on a user 'extern fun': a plain function has no native call to pass the extra arguments to",
                    display_name(f)
                ),
                span: f.span,
            });
        }
        if project.extern_ffi_of(&f.module).is_none() {
            return Err(TypeError {
                message: format!(
                    "'{}' writes '...', but only a user 'extern fun' - one whose module has a coral.toml '[extern.\"{}\"]' section - may be variadic",
                    display_name(f),
                    f.module
                ),
                span: f.span,
            });
        }
        if !decl_type_params(f).is_empty() {
            return Err(TypeError {
                message: format!(
                    "'{}' is variadic and generic: a variadic user 'extern fun' cannot have type parameters",
                    display_name(f)
                ),
                span: f.span,
            });
        }
    }
    // A `naked fun` (kernel stage 5) is a declaration whose whole body is one
    // `asm(...)`: nothing else can be emitted around it, so nothing else may be
    // written. The check is here rather than in the parser because it is a
    // property of the declaration the type checker already owns.
    if f.naked {
        if f.extern_ {
            return Err(TypeError {
                message: "a 'naked fun' cannot be 'extern': it has a body of its own".to_string(),
                span: f.span,
            });
        }
        if f.receiver.is_some() {
            return Err(TypeError {
                message: "a 'naked fun' cannot be a receiver method: it has no compiler-emitted prologue, so there is nothing that could pass the receiver".to_string(),
                span: f.span,
            });
        }
        if f.name == "main" {
            return Err(TypeError {
                message: "a 'naked fun' cannot be 'main': the compiler emits the hosted entry point's own C signature for it".to_string(),
                span: f.span,
            });
        }
        if !f.params.is_empty() {
            return Err(TypeError {
                message: "a 'naked fun' takes no parameters: the asm reads the incoming registers itself, and the compiler binds nothing".to_string(),
                span: f.span,
            });
        }
        let single_asm = match &f.body {
            ast::FunctionBody::Block(b) => {
                b.stmts.len() == 1
                    && matches!(
                        &b.stmts[0],
                        ast::Stmt::Expr { expr: ast::Expr::Asm { .. }, .. }
                    )
            }
            ast::FunctionBody::Expr(e) => matches!(e, ast::Expr::Asm { .. }),
            ast::FunctionBody::Extern | ast::FunctionBody::Signature => false,
        };
        if !single_asm {
            return Err(TypeError {
                message: "a 'naked fun' body must be exactly one 'asm(...)' statement: nothing else can be emitted around it".to_string(),
                span: f.span,
            });
        }
    }
    let receiver_struct = f
        .receiver
        .as_ref()
        .and_then(|_| struct_name_of(&sig.params[0].1).map(|s| s.to_string()));
    // A declared destructor (kernel stage 9, MK9-S6-M1) reads its passdowns
    // the way `main` does: directly from the generated globals. The reason is
    // that the generated `kelp_drop_<T>` calls it from every drop point with
    // one pointer and nothing else, so a hidden passdown parameter would have
    // no caller able to supply it. The passdown surface is read-only inside a
    // function, so the global read is the same borrow `main` takes.
    let is_drop_method = f
        .receiver
        .as_ref()
        .is_some_and(|_| f.name == crate::types::DROP_METHOD);
    // Passdowns this function references become trailing hidden parameters
    // (see `PdInfo::arg` for the call-site side). `main` is the exception
    // downstream, not here: `codegen_c.rs` emits a fixed `main(argc, argv)`
    // regardless of parameters, and the body's `pd#...` identifiers then
    // resolve to the file-scope globals - which is why the hidden name and
    // the global name are the same string.
    let hidden = if key == "main" || is_drop_method {
        Vec::new()
    } else {
        // An instantiation's passdown wiring is the template's: the scan runs
        // over declarations, and which passdowns a body names does not depend
        // on its type arguments (A-2). The borrow *mode* is the template's for
        // the same reason.
        let wiring_key = if wired.contains_key(key) {
            key.to_string()
        } else {
            fns.instances
                .borrow()
                .origin
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.to_string())
        };
        wired.get(&wiring_key).cloned().unwrap_or_default()
    };
    let mut ctx = base_ctx(
        &f.module,
        sigs,
        structs,
        enums,
        consts,
        const_fns,
        owned_structs,
        overloads,
        project,
        all_pds,
        None,
        wired,
        env,
        fns,
        statics,
    );
    // A synthesized trait default resolves its body in the trait's module (see
    // `FunctionDecl::body_module`): its signature was substituted with the
    // implementing type, but its body's types and free names were written in
    // the trait's module.
    if let Some(m) = &f.body_module {
        ctx.prefix = project.prefix_of(m).to_string();
        ctx.aliases = project.alias_prefixes_of(m);
    }
    ctx.type_args = type_args.clone();
    // A-12 slice 5: an impl method's body may name `Self::Item` in a local
    // annotation. `resolve.rs` substituted it in the signature only, so add the
    // impl's bindings under the `Self::Item` spelling for the body.
    if let Some(site) = &f.trait_impl {
        for key in trait_keys_with_supertraits(&env, &site.trait_name) {
            if let Some(bindings) = env.assoc.get(&(key, site.self_type.clone())) {
                for (item, ty) in bindings {
                    ctx.type_args.insert(format!("Self::{item}"), ty.clone());
                }
            }
        }
    }
    ctx.ret_ty = sig.ret.clone();
    ctx.receiver_struct = receiver_struct;
    ctx.pd_global_borrow = key == "main" || is_drop_method;
    // `unsafe fun` (MK9-S2 stage 1) and `naked fun` (kernel stage 5): the whole
    // body is an unsafe context, so the unsafe-only pointer operations, and the
    // `asm` a naked body must be, are legal without a block around them. Every
    // other function starts at zero and needs an `unsafe { ... }`.
    ctx.unsafe_depth = if f.unsafe_ || f.naked { 1 } else { 0 };
    ctx.own_hidden = hidden.iter().map(|pd| pd.hidden.clone()).collect();
    let wiring_key = ctx.wiring_key_of(key);
    ctx.fn_key = wiring_key;
    ctx.pd_modes = wiring.modes.clone();
    ctx.pd_probe = wiring.probe.clone();
    let mut params = Vec::new();
    // Parameters (and the receiver, if any) are immutable bindings, like
    // `val`; v1 has no syntax to declare a mutable *binding* for one (the
    // borrow itself can still be exclusive - that's `Ty::Ref { mutable:
    // true, .. }`, a property of the type, not the binding).
    if f.receiver.is_some() {
        let (pname, ty) = &sig.params[0];
        ctx.define(pname, ty.clone(), false);
        params.push((pname.clone(), ty.clone()));
    }
    let declared = if f.receiver.is_some() { &sig.params[1..] } else { &sig.params[..] };
    for (p, (_, ty)) in f.params.iter().zip(declared.iter()) {
        ctx.define(&p.name, ty.clone(), false);
        params.push((p.name.clone(), ty.clone()));
    }

    let body = match &f.body {
        ast::FunctionBody::Block(b) => check_block(b, &mut ctx, Some(&sig.ret))?,
        ast::FunctionBody::Expr(e) => {
            let checked = check_expr(e, &mut ctx, Some(&sig.ret))?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(checked)),
            }
        }
        ast::FunctionBody::Extern => {
            // No `.kp` body. Either `codegen_c.rs` writes the
            // compiler-supplied C (see `externs.rs`, stdlib modules only), or
            // - MK9-S1-M1, roadmap B-1 - this module has its own
            // `coral.toml` `[extern."<module id>"]` entry, in which case
            // `codegen_c.rs` writes a direct-call trampoline to the native
            // symbol instead, gated the same way a stdlib extern is (see
            // `resolve.rs::domain_of_with_extern` and the `gated` map it
            // feeds). Confirm one of the two actually admits this
            // declaration before accepting it - otherwise the failure would
            // surface as a `cc` error at best, or as a silently missing
            // symbol, or (worse, for the manifest case) as a user extern
            // that reaches a native symbol its own manifest never
            // allowlisted.
            let prefix = project.prefix_of(&f.module);
            let bare = f.name.strip_prefix(prefix).unwrap_or(&f.name);
            if !crate::externs::has_body(&f.module, bare) {
                match project.extern_ffi_of(&f.module) {
                    Some(decl) if decl.symbol_of(bare).is_some() => {
                        check_extern_callback_params(f, sig)?;
                    }
                    Some(_) => {
                        return Err(TypeError {
                            message: format!(
                                "extern function '{bare}' is not in this module's coral.toml '[extern.\"{}\"]' symbol allowlist - add it there before declaring 'extern fun {bare}'",
                                f.module
                            ),
                            span: f.span,
                        });
                    }
                    None => {
                        return Err(TypeError {
                            message: format!(
                                "extern function '{bare}' has no compiler-supplied body - only compiler-shipped stdlib modules may declare 'extern fun', unless this module has its own coral.toml '[extern.\"{}\"]' section naming '{bare}' in its 'symbols' allowlist", f.module
                            ),
                            span: f.span,
                        });
                    }
                }
            }
            tir::Block {
                stmts: vec![],
                tail: None,
            }
        }
        // A trait method signature (A-12 slice 2) has no body and is never a
        // function of its own: the impl supplies the body, which is an
        // ordinary method this pass does check. Reaching here would mean a
        // signature had been registered as a declaration.
        ast::FunctionBody::Signature => {
            return Err(TypeError {
                message: format!(
                    "'{}' is a trait method signature and has no body: write the body in the 'impl' block",
                    display_name(f)
                ),
                span: f.span,
            })
        }
    };

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1/M2), so a mismatch is only reported when a tail
    // expression is actually present and disagrees with the declared return
    // type.
    // A body whose tail is a `panic` diverges, so it satisfies any declared
    // return type: `fun unwrap<T>(o: &Option<T>).return: T { ... panic("...") }` is
    // the "this cannot happen" accessor. Nothing else in the language is
    // divergence-aware - an `if` arm cannot be a `panic` - so this is
    // deliberately the narrow rule the accessors need.
    let tail_diverges = body
        .tail
        .as_ref()
        .is_some_and(|e| tail_diverges(e));
    if sig.ret != Ty::Unit
        && body.tail.is_some()
        && !tail_diverges
        && !f.naked
        && !str_view_compatible(&body.ty(), &sig.ret)
    {
        return Err(TypeError {
            message: format!(
                "function '{}' declared to return '{}' but body evaluates to '{}'",
                display_name(f),
                sig.ret.display(),
                body.ty().display()
            ),
            span: f.span,
        });
    }

    let body = if f.naked {
        // The single `asm(...)` becomes a statement rather than the block's
        // tail, so each backend emits it and then nothing: the asm itself is
        // what returns, and a `return` of the declared type would be an
        // instruction a naked function cannot have.
        let mut body = body;
        let asm = match body.tail.take() {
            Some(e) => *e,
            None => match body.stmts.pop() {
                Some(tir::Stmt::Expr(e)) => e,
                _ => unreachable!("a naked body is exactly one asm expression"),
            },
        };
        tir::Block {
            stmts: vec![tir::Stmt::Expr(asm)],
            tail: None,
        }
    } else {
        body
    };

    Ok(tir::Function {
        name: key.to_string(),
        params: {
            params.extend(hidden.iter().map(|pd| {
                (
                    pd.hidden.clone(),
                    pd.borrow_ty(wiring.exclusive(&ctx.fn_key, pd)),
                )
            }));
            params
        },
        ret: sig.ret.clone(),
        body,
        // The declaration's own span, which is the line a backend names as the
        // function's: the C backend writes nothing for it today, and the LLVM
        // backend's `DISubprogram` reads it (MK7 S4).
        span: f.span,
        module: f.module.clone(),
        extern_: f.extern_,
        // Only a `fun` literal that captured something has an environment;
        // a declaration never does (A-4 phase E).
        env: None,
        naked: f.naked,
        align: f.align,
        section: f.section.clone(),
    })
}

/// A call to a `must_use fun` whose result goes nowhere (MISSING-Cn2Ym).
fn refuse_dropped_must_use(checked: &tir::Expr, ctx: &Ctx, span: Span) -> Result<()> {
    if let tir::ExprKind::Call { callee, .. } = &checked.kind {
        // A generic instance's key carries its type arguments (`Sender#send<i64>`).
        let plain = callee.split('<').next().unwrap_or(callee);
        if ctx.must_use_fns.contains(plain) {
            return Err(TypeError {
                message: format!(
                    "the result of '{}' must be used: it reports a failure or hands a value back. Branch on it, or write 'val _r = ...' to discard it on purpose",
                    display_callee(plain)
                ),
                span,
            });
        }
    }
    Ok(())
}

fn check_block(block: &ast::Block, ctx: &mut Ctx, expected_tail: Option<&Ty>) -> Result<tir::Block> {
    ctx.push_scope();
    let mut stmts = Vec::new();
    let mut tail = None;
    // The open containers this block declared (`var xs = List.empty()` with no
    // type written anywhere), and where each one's `let` sits in `stmts`. An
    // operation later in the block, or in a block under it, decides the
    // element type and rewrites the binding; the local emitted here has to be
    // rewritten with it, because the struct and the routines the backend
    // monomorphizes are chosen by the element type.
    let mut open: Vec<OpenBinding> = Vec::new();

    for (i, stmt) in block.stmts.iter().enumerate() {
        let is_last = i + 1 == block.stmts.len();
        // Statements are newline-terminated now (no `;`), so "textually
        // last in the block" is the only signal for tail-expression status.
        let is_tail = is_last && matches!(stmt, ast::Stmt::Expr { .. });

        if is_tail {
            if let ast::Stmt::Expr { expr, .. } = stmt {
                let checked = check_expr(expr, ctx, expected_tail)?;
                // A unit block throws its tail value away, so a `must_use`
                // call there is a dropped result like any statement's.
                if matches!(expected_tail, Some(Ty::Unit)) {
                    refuse_dropped_must_use(&checked, ctx, expr.span())?;
                }
                tail = Some(Box::new(checked));
            }
        } else {
            let checked = check_stmt(stmt, ctx)?;
            if let ast::Stmt::Let { name, ty: None, init, .. } = stmt {
                let still_open = ctx
                    .lookup(name)
                    .map_or(false, |i| crate::types::is_open_container(&i.ty));
                if still_open {
                    open.push(OpenBinding {
                        name: name.clone(),
                        index: stmts.len(),
                        span: init.span(),
                        call: empty_container_call(init).map(|(c, m)| (c.to_string(), m.to_string())),
                    });
                }
            }
            // A destructuring binding lowers to several statements (the value
            // it takes apart, then one binding per position), and the open-
            // container repair below is keyed by a statement index, so the
            // count is what the next binding's index has to be measured
            // against. `check_stmt` is the only place that decides how many.
            stmts.extend(checked);
        }
        settle_open_bindings(&mut open, &mut stmts, ctx);
    }
    settle_open_bindings(&mut open, &mut stmts, ctx);

    if let Some(still_open) = open.first() {
        let ty = ctx.lookup(&still_open.name).map(|i| i.ty.clone()).unwrap_or(Ty::Unit);
        let error = open_container_declaration_error(&ty, still_open);
        ctx.pop_scope();
        return Err(error);
    }

    ctx.pop_scope();
    Ok(tir::Block { stmts, tail })
}

/// One `let` in a block whose container started open and may still be open.
struct OpenBinding {
    name: String,
    /// Where the `let` sits in the block's statement list.
    index: usize,
    /// The initializer's span, which is where the diagnostic points.
    span: Span,
    /// The `(Type, method)` the initializer was, when it was the associated
    /// call this is all about: the diagnostic names the constructor that was
    /// written rather than guessing between `Map.new()` and `Map.empty()`.
    call: Option<(String, String)>,
}

/// Rewrites every binding an operation has since decided, and forgets it. The
/// type an open container took is the one the scope now holds for the name,
/// so `check_expr`'s repair is the same repair as a deciding call's: both
/// `set_binding_ty` it, and this moves the result into the emitted local.
fn settle_open_bindings(open: &mut Vec<OpenBinding>, stmts: &mut [tir::Stmt], ctx: &Ctx) {
    open.retain(|binding| {
        let Some(info) = ctx.lookup(&binding.name) else {
            return false;
        };
        if crate::types::is_open_container(&info.ty) {
            return true;
        }
        let ty = info.ty.clone();
        if let Some(tir::Stmt::Let { ty: slot, init, .. }) = stmts.get_mut(binding.index) {
            *slot = ty.clone();
            init.ty = ty;
        }
        false
    });
}

/// The `Type.empty()`/`Type.new()` call an initializer is, when it is one.
/// Used only to name the constructor in a diagnostic: an open container that
/// nothing ever decided is reported where it was written, saying which call
/// left the element type out.
fn empty_container_call(expr: &ast::Expr) -> Option<(&str, &str)> {
    let ast::Expr::MethodCall { receiver, method, args, .. } = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let ast::Expr::Ident(name, _) = receiver.as_ref() else {
        return None;
    };
    matches!(name.as_str(), "List" | "Box" | "Map").then(|| (name.as_str(), method.as_str()))
}

/// The diagnostic an open container gets when nothing in its block decided
/// what it holds: the same one a bare `List.empty()` was always refused with,
/// so the fix a reader is told is still the binding.
fn open_container_declaration_error(ty: &Ty, binding: &OpenBinding) -> TypeError {
    let message = match (&binding.call, ty) {
        (Some((container, method)), _) if container == "Map" => format!(
            "cannot tell what 'Map.{method}()' holds, write the type on the binding ('var m: Map<String, i32> = Map.{method}()')"
        ),
        (Some((container, _)), _) => format!(
            "cannot tell what '{container}.empty()' holds, write the element type on the binding ('var xs: {container}<String> = {container}.empty()') or start from '{container}.of(x)'"
        ),
        (None, Ty::Map(..)) => "cannot tell what 'Map.new()' holds, write the type on the binding ('var m: Map<String, i32> = Map.new()')".to_string(),
        (None, Ty::List(_)) => "cannot tell what 'List.empty()' holds, write the element type on the binding ('var xs: List<String> = List.empty()') or start from 'List.of(x)'".to_string(),
        (None, _) => "cannot tell what 'Box.empty()' holds, write the element type on the binding ('var b: Box<String> = Box.empty()') or start from 'Box.of(x)'".to_string(),
    };
    TypeError { message, span: binding.span }
}

fn check_stmt(stmt: &ast::Stmt, ctx: &mut Ctx) -> Result<Vec<tir::Stmt>> {
    match stmt {
        ast::Stmt::Let {
            name,
            mutable,
            ty,
            init,
            ..
        } => {
            let expected = match ty {
                Some(t) => Some(ctx.type_scope().resolve(t, false)?),
                None => None,
            };
            if let Some((root, rebased)) = binding_borrow_temporary(init, ctx.when_temps) {
                ctx.when_temps += 1;
                let tmp = format!("{}{}", BORROW_TEMP_PREFIX, ctx.when_temps - 1);
                let held = check_expr(root, ctx, None)?;
                let held_ty = held.ty.clone();
                ctx.define(&tmp, held_ty.clone(), false);
                let value = check_expr(&rebased, ctx, expected.as_ref())?;
                let final_ty = expected.unwrap_or_else(|| value.ty.clone());
                if final_ty != value.ty {
                    return Err(TypeError {
                        message: format!(
                            "cannot assign value of type '{}' to '{name}' declared as '{}'",
                            value.ty.display(),
                            final_ty.display()
                        ),
                        span: stmt_span(stmt),
                    });
                }
                ctx.define(name, final_ty.clone(), *mutable);
                return Ok(vec![
                    tir::Stmt::Let { name: tmp, ty: held_ty, init: held },
                    tir::Stmt::Let { name: name.clone(), ty: final_ty, init: value },
                ]);
            }
            // A `let` with no written type whose initializer *is* a bare
            // `List.empty()`/`Box.empty()`/`Map.new()` is the one place an
            // empty container may leave its element open for a later
            // `push`/`insert` to decide. Anywhere else, including a call
            // somewhere inside the initializer, an empty container with no
            // type to take its element from is a declaration-time error,
            // exactly as it always was.
            let deferred = ty.is_none() && empty_container_call(init).is_some();
            let outer_defer = ctx.defer_empty_container;
            ctx.defer_empty_container = deferred;
            let init = check_expr(init, ctx, expected.as_ref());
            ctx.defer_empty_container = outer_defer;
            let init = init?;
            let final_ty = expected.unwrap_or_else(|| init.ty.clone());
            if final_ty != init.ty {
                return Err(TypeError {
                    message: format!(
                        "cannot assign value of type '{}' to '{name}' declared as '{}'",
                        init.ty.display(),
                        final_ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            ctx.define(name, final_ty.clone(), *mutable);
            Ok(vec![tir::Stmt::Let {
                name: name.clone(),
                ty: final_ty,
                init,
            }])
        }
        // `val (lo, hi) = min_max(xs)`: a destructuring binding
        // (MISSING-z2bB9, `plan/syntax.md` 4). One binding becomes one owning
        // temporary plus one ordinary `let` per bound position, read out of
        // that temporary: the lowering is spelled here, in terms of the
        // record a tuple already is, so nothing downstream learns a new
        // statement and the positions the pattern ignores stay in the
        // temporary, whose own drop frees them.
        ast::Stmt::LetTuple {
            pattern,
            mutable,
            init,
            span,
        } => {
            let value = check_expr(init, ctx, None)?;
            // The initializer is moved into the destructuring, so a place is
            // taken by value exactly as `val p = place` takes it, and the
            // positions are then read out of that value one at a time.
            let value_ty = value.ty.clone();
            let elements = tuple_elems_of(&value_ty, ctx).ok_or_else(|| TypeError {
                message: format!(
                    "a destructuring binding takes one tuple value apart, and this initializer is a '{}': 'val (lo, hi) = ...' needs a tuple here ('(i32, i32)'), and a single value is bound with an ordinary 'val name = ...'",
                    value_ty.display()
                ),
                span: init.span(),
            })?;
            if elements.len() != pattern_arity(pattern) {
                return Err(TypeError {
                    message: format!(
                        "this pattern has {} position(s), and the tuple it destructures ('{}') has {}",
                        pattern_arity(pattern),
                        value_ty.display(),
                        elements.len()
                    ),
                    span: *span,
                });
            }
            ctx.tuple_temps += 1;
            let tmp = format!("{}{}", TUPLE_TEMP_PREFIX, ctx.tuple_temps - 1);
            ctx.define(&tmp, value_ty.clone(), false);
            let mut stmts = vec![tir::Stmt::Let {
                name: tmp.clone(),
                ty: value_ty.clone(),
                init: value,
            }];
            lower_pattern(pattern, &tmp, &value_ty, &elements, *mutable, ctx, &mut stmts, *span)?;
            Ok(stmts)
        }
        ast::Stmt::While {
            cond,
            body,
            iterating,
            span,
        } => {
            let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
            if cond.ty != Ty::Bool {
                return Err(TypeError {
                    message: format!("while condition must be 'bool', found '{}'", cond.ty.display()),
                    span: *span,
                });
            }
            ctx.loop_depth += 1;
            let body = check_block(body, ctx, None);
            ctx.loop_depth -= 1;
            let body = body?;
            let iterating = iterating.as_ref().and_then(|place| {
                ast::place_path(place).map(|path| tir::IteratedPlace {
                    path,
                    span: place.span(),
                })
            });
            Ok(vec![tir::Stmt::While { cond, body, iterating }])
        }
        // A `for` keeps its two desugars (`plan/iterators.md`): a `List<T>` or
        // `[T; N]` walks by index exactly as before, and anything that
        // implements `std.iter.Iterator` becomes a `var` over `next()`. Only
        // this pass knows the iterable's type, so this is where the choice is
        // made. A type-only probe is enough to choose and is what
        // `check_method_call` already does before it re-checks its receiver;
        // the desugar re-checks the iterable for real, once.
        ast::Stmt::For {
            binding,
            binding_span,
            iterable: written,
            body,
            span,
        } => {
            // `for x in xs.iter()` (and `for (k, v) in m.iter()`) walks the
            // same place as `for x in xs`: the element is the shared borrow
            // `List.get` already gives, and the body may not mutate `xs`.
            let stripped = for_iter_receiver(written, ctx)?;
            let iterable = stripped.as_ref().unwrap_or(written);
            let probe = check_expr(iterable, ctx, None)?;
            let iter_ty = probe.ty.clone();
            // `for (k, v) in map`: the parser read the pattern as a hidden
            // element binding and a `val (k, v) = <hidden>` at the top of the
            // body. A `Map` has no element that is a pair (its entries are not
            // stored as one), so the pattern is taken apart here: the loop walks
            // the keys, `k` is the key and `v` is `map.get(key)`.
            if crate::types::map_key_of(&iter_ty).is_some() {
                let desugared = desugar_map_for(binding, iterable, body, *span, &iter_ty)?;
                return check_stmt(&desugared, ctx);
            }
            let list = crate::types::list_elem_of(&iter_ty).is_some()
                || crate::types::array_elem_of(&iter_ty).is_some();
            if list {
                let desugared =
                    desugar_list_for(binding, *binding_span, iterable, body, *span, ctx);
                return check_stmt(&desugared, ctx);
            }
            if !implements_iterator(ctx, &iter_ty) {
                return Err(TypeError {
                    message: format!(
                        "'for' walks a 'List<T>' or a type that implements 'Iterator', and '{}' is neither: implement 'std.iter.Iterator' for it ('type Item' and 'fun mut Self.next().return: option.Option<Self::Item>'), or walk a 'List' by index",
                        iter_ty.display()
                    ),
                    span: *span,
                });
            }
            let desugared =
                desugar_iterator_for(binding, *binding_span, iterable, body, *span, &iter_ty, ctx);
            check_stmt(&desugared, ctx)
        }
        ast::Stmt::Break { span } | ast::Stmt::Continue { span } => {
            if ctx.loop_depth == 0 {
                let what = if matches!(stmt, ast::Stmt::Break { .. }) {
                    "break"
                } else {
                    "continue"
                };
                return Err(TypeError {
                    message: format!(
                        "'{what}' is a loop control statement, and no loop encloses it: write it inside a 'while' or a 'for'"
                    ),
                    span: *span,
                });
            }
            Ok(vec![if matches!(stmt, ast::Stmt::Break { .. }) {
                tir::Stmt::Break { span: *span }
            } else {
                tir::Stmt::Continue { span: *span }
            }])
        }
        ast::Stmt::Return { value, .. } => {
            let ret_ty = ctx.ret_ty.clone();
            let value = match value {
                Some(v) => Some(check_expr(v, ctx, Some(&ret_ty))?),
                None => None,
            };
            let actual = value.as_ref().map(|v| v.ty.clone()).unwrap_or(Ty::Unit);
            if !str_view_compatible(&actual, &ret_ty) {
                // A borrow where an owned value is wanted, inside a stdlib
                // module, is a `Copy`-only accessor: the arm that hands the
                // value out cannot take it from the borrow it was given.
                let note = if matches!(&actual, Ty::Ref { inner, .. } if **inner == ret_ty) {
                    stdlib_instantiation_note(ctx)
                } else {
                    String::new()
                };
                return Err(TypeError {
                    message: format!(
                        "return type mismatch: expected '{}', found '{}'{note}",
                        ret_ty.display(),
                        actual.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(vec![tir::Stmt::Return {
                value,
                span: stmt_span(stmt),
            }])
        }
        ast::Stmt::Expr { expr, .. } => {
            let checked = check_expr(expr, ctx, None)?;
            refuse_dropped_must_use(&checked, ctx, stmt_span(stmt))?;
            // A non-tail expression statement discards its value, and nothing
            // owns an owned value afterwards - v1 has no drop point for a
            // temporary, so this would leak. Reject it instead.
            if !checked.ty.is_copy() {
                return Err(TypeError {
                    message: format!(
                        "this expression produces an owned '{}' value that is never used - bind it to a 'val' or 'var', print it, or pass it to something that takes it by value",
                        checked.ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(vec![tir::Stmt::Expr(checked)])
        }
        // `defer` is lowered away by `defer.rs` before this pass runs, so one
        // reaching the checker is a compiler bug rather than a program error.
        ast::Stmt::Defer { span, .. } => Err(TypeError {
            message: "internal error: a 'defer' statement reached the type checker; it should have been lowered before checking"
                .to_string(),
            span: *span,
        }),
    }
}

/// A binding is the one expression context that may keep an owned temporary
/// alive for a borrow it receives. The first slice covers the two text views;
/// other borrow-returning APIs can join this lowering without changing its
/// lifetime or drop model.
fn binding_borrow_temporary(init: &ast::Expr, next: usize) -> Option<(&ast::Expr, ast::Expr)> {
    let ast::Expr::MethodCall { receiver, method, type_args, args, span } = init else {
        return None;
    };
    if !matches!(method.as_str(), "as_str" | "view") {
        return None;
    }
    let root = ast::temporary_root(receiver)?;
    next.checked_add(1)?;
    let tmp = format!("{}{}", BORROW_TEMP_PREFIX, next);
    Some((
        root,
        ast::Expr::MethodCall {
            receiver: Box::new(ast::rebase_root(receiver, &ast::Expr::Ident(tmp, root.span()))),
            method: method.clone(),
            type_args: type_args.clone(),
            args: args.clone(),
            span: *span,
        },
    ))
}

/// The canonical key of `std.iter.Iterator`, the one trait a `for` accepts
/// (`plan/iterators.md`). A project spells it `impl "std/iter"` and reaches the
/// trait as `iter.Iterator`; the canonical key is what the impl table is keyed
/// by.
const ITERATOR_TRAIT_KEY: &str = crate::stdlib::ITERATOR_TRAIT;

/// Whether a `for` may walk this type: it implements `std.iter.Iterator`,
/// possibly through a supertrait.
fn implements_iterator(ctx: &Ctx, ty: &Ty) -> bool {
    let name = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()));
    if name.as_ref().map_or(false, |n| implements_trait(&ctx.env, ITERATOR_TRAIT_KEY, n)) {
        return true;
    }
    // A generic instantiation cannot take a trait impl (the parser reads the
    // `for` type as a bare name), so a shipped generic `datac` such as the
    // channel's `Receiver<T>` is walkable through a method it declares on its
    // template: `fun mut Receiver<T>.next().return: option.Option<T>`. The `for`
    // desugar is an ordinary `next()` call either way, so a wrong shape is
    // the ordinary type error of that call.
    let Some(name) = name else { return false };
    let origin = ctx.env.instances.borrow().origin.get(&name).cloned();
    origin.map_or(false, |(template, _)| {
        ctx.fns.templates.contains_key(&format!("{template}#next"))
    })
}

/// The receiver of a `xs.iter()` written as a `for` iterable, when `xs` is a
/// `List`, an array or a `Map`: such a loop walks `xs` itself, so it takes the
/// same desugar as `for x in xs`. Any other `iter()` (a user type's own method)
/// is left as written and goes down the `Iterator` path.
fn for_iter_receiver(iterable: &ast::Expr, ctx: &mut Ctx) -> Result<Option<ast::Expr>> {
    let ast::Expr::MethodCall { receiver, method, type_args, args, .. } = iterable else {
        return Ok(None);
    };
    if method != "iter" || !type_args.is_empty() || !args.is_empty() {
        return Ok(None);
    }
    let recv_ty = check_expr(receiver, ctx, None)?.ty;
    let walkable = crate::types::list_elem_of(&recv_ty).is_some()
        || crate::types::array_elem_of(&recv_ty).is_some()
        || crate::types::map_key_of(&recv_ty).is_some();
    Ok(walkable.then(|| (**receiver).clone()))
}

/// `for (k, v) in map { body }` becomes `for #key in map.keys() { val k =
/// #key; val v = map.get(#key); body }`. The keys are a snapshot in insertion
/// order, so the map may be changed inside the loop, and a key removed there
/// makes the later `get` panic like any `get` of an absent key.
fn desugar_map_for(
    binding: &str,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    iter_ty: &Ty,
) -> Result<ast::Stmt> {
    let refuse = |message: String| Err(TypeError { message, span });
    if !binding.starts_with("#forpat") {
        return refuse(format!(
            "'for' over a '{}' binds the key and the value together: write 'for (key, value) in map'",
            iter_ty.display()
        ));
    }
    let Some(ast::Stmt::LetTuple { pattern: ast::BindingPattern::Tuple(positions), .. }) = body.stmts.first() else {
        return refuse("a 'for' over a Map needs the pattern '(key, value)'".to_string());
    };
    let (ast::BindingPattern::Name(_) | ast::BindingPattern::Ignore, ast::BindingPattern::Name(_) | ast::BindingPattern::Ignore) =
        (&positions[0], &positions[1])
    else {
        return refuse("a 'for' over a Map binds two names: 'for (key, value) in map', with '_' to ignore one".to_string());
    };
    if positions.len() != 2 {
        return refuse(format!(
            "a 'for' over a Map binds a key and a value, and this pattern has {} positions",
            positions.len()
        ));
    }
    if ast::temporary_root(iterable).is_some() {
        return refuse("a 'for' over a Map walks a named map: bind the map to a 'val' first".to_string());
    }
    let ident = |name: &str| ast::Expr::Ident(name.to_string(), span);
    let mut stmts: Vec<ast::Stmt> = Vec::new();
    if let ast::BindingPattern::Name(k) = &positions[0] {
        stmts.push(ast::Stmt::Let { name: k.clone(), mutable: false, ty: None, init: ident(binding), span });
    }
    if let ast::BindingPattern::Name(v) = &positions[1] {
        stmts.push(ast::Stmt::Let {
            name: v.clone(),
            mutable: false,
            ty: None,
            init: ast::Expr::MethodCall {
                receiver: Box::new(iterable.clone()),
                method: "get".to_string(),
                type_args: Vec::new(),
                // A `String` key is looked up by its `str` view.
                args: vec![ast::CallArg {
                    name: None,
                    value: if matches!(crate::types::map_key_of(iter_ty), Some(Ty::String)) {
                        ast::Expr::MethodCall {
                            receiver: Box::new(ident(binding)),
                            method: "as_str".to_string(),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            span,
                        }
                    } else {
                        ident(binding)
                    },
                }],
                span,
            },
            span,
        });
    }
    stmts.extend(body.stmts[1..].iter().cloned());
    Ok(ast::Stmt::For {
        binding: binding.to_string(),
        binding_span: span,
        iterable: ast::Expr::MethodCall {
            receiver: Box::new(iterable.clone()),
            method: "keys".to_string(),
            type_args: Vec::new(),
            args: Vec::new(),
            span,
        },
        body: ast::Block { stmts, span: body.span },
        span,
    })
}

/// The index desugar a `for x in <List<T>|[T; N]>` becomes (`plan/syntax.md`
/// 6.2), unchanged from the parser's old one except that the index advances
/// **before** the body runs. That is what makes a `continue` in the body jump
/// to the next element instead of re-reading the same one: the desugared loop
/// is an ordinary `while`, and its `continue` re-tests the condition.
///
/// A place iterable is walked as written so `borrowck.rs` can name it and
/// refuse a mutation of it while the body runs; anything else is staged into a
/// synthesized binding so it is evaluated exactly once.
fn desugar_list_for(
    binding: &str,
    binding_span: Span,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    ctx: &mut Ctx,
) -> ast::Stmt {
    let n = ctx.for_temps;
    ctx.for_temps += 1;
    let seq_name = format!("#for{n}");
    let idx_name = format!("#for{n}_i");
    let idx_span = binding_span;
    let iterating = ast::temporary_root(iterable)
        .is_none()
        .then(|| iterable.clone());
    let mut stmts: Vec<ast::Stmt> = Vec::new();
    let seq = match ast::temporary_root(iterable).cloned() {
        None => iterable.clone(),
        Some(root) => {
            let init_span = root.span();
            stmts.push(ast::Stmt::Let {
                name: seq_name.clone(),
                mutable: false,
                ty: None,
                init: root,
                span: init_span,
            });
            ast::rebase_root(iterable, &ast::Expr::Ident(seq_name.clone(), init_span))
        }
    };
    let seq_span = seq.span();
    let cond = ast::Expr::Binary {
        op: ast::BinOp::Lt,
        lhs: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
        rhs: Box::new(ast::Expr::MethodCall {
            receiver: Box::new(seq.clone()),
            method: "len".to_string(),
            type_args: Vec::new(),
            args: Vec::new(),
            span: seq_span,
        }),
        span,
    };
    let element = ast::Stmt::Let {
        name: binding.to_string(),
        mutable: false,
        ty: None,
        init: ast::Expr::MethodCall {
            receiver: Box::new(seq),
            method: "get".to_string(),
            type_args: Vec::new(),
            args: vec![ast::CallArg {
                name: None,
                value: ast::Expr::Ident(idx_name.clone(), idx_span),
            }],
            span: binding_span,
        },
        span: binding_span,
    };
    let increment = ast::Stmt::Expr {
        expr: ast::Expr::Assign {
            target: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
            value: Box::new(ast::Expr::Binary {
                op: ast::BinOp::Add,
                lhs: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
                rhs: Box::new(ast::Expr::IntLit(1, idx_span)),
                span: idx_span,
            }),
            span: idx_span,
        },
        span: idx_span,
    };
    let mut inner_stmts = vec![element, increment];
    inner_stmts.extend(body.stmts.iter().cloned());
    stmts.push(ast::Stmt::Let {
        name: idx_name,
        mutable: true,
        ty: Some(ast::TypeExpr::named("usize", idx_span)),
        init: ast::Expr::IntLit(0, idx_span),
        span: idx_span,
    });
    stmts.push(ast::Stmt::While {
        cond,
        body: ast::Block {
            stmts: vec![ast::Stmt::Expr {
                expr: ast::Expr::Block(ast::Block {
                    stmts: inner_stmts,
                    span: body.span,
                }),
                span: body.span,
            }],
            span: body.span,
        },
        iterating,
        span,
    });
    ast::Stmt::Expr {
        expr: ast::Expr::Block(ast::Block { stmts, span }),
        span,
    }
}

/// The `next()` desugar a `for x in <Iterator>` becomes
/// (`plan/iterators.md`): the loop holds the iterator, calls `next()` once per
/// pass, binds the owned item in the `Some` arm, and leaves on `None`.
///
/// A **place** iterator is borrowed exclusively for the loop's duration, so a
/// use of it in the body is the ordinary "already borrowed" error; an iterator
/// built at the `for` line (a call result) is owned by a synthesized `var`. The
/// item is owned because `next()` returns an owned `Option`.
fn desugar_iterator_for(
    binding: &str,
    binding_span: Span,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    iter_ty: &Ty,
    ctx: &mut Ctx,
) -> ast::Stmt {
    let n = ctx.for_temps;
    ctx.for_temps += 1;
    let it_name = format!("#for{n}");
    let is_place = ast::temporary_root(iterable).is_none();
    let (init, mutable) = if !is_place {
        (iterable.clone(), true)
    } else if matches!(iter_ty, Ty::Ref { mutable: true, .. }) {
        // Already an exclusive borrow (`it: mut I`): bind the borrow itself;
        // `&mut it` would be a borrow of a borrow and would not resolve the
        // receiver's own methods.
        (iterable.clone(), false)
    } else {
        (
            ast::Expr::Ref {
                mutable: true,
                expr: Box::new(iterable.clone()),
                span,
            },
            false,
        )
    };
    let it_span = iterable.span();
    let scrutinee = ast::Expr::MethodCall {
        receiver: Box::new(ast::Expr::Ident(it_name.clone(), it_span)),
        method: "next".to_string(),
        type_args: Vec::new(),
        args: Vec::new(),
        span,
    };
    // The body's statements run in the `Some` arm, and the arm block has to be
    // unit like the `None` arm, so a trailing binding keeps the body's own tail
    // a statement rather than the arm's value (the same rule the index desugar
    // applies when it discards a tail).
    let mut some_stmts = body.stmts.clone();
    some_stmts.push(ast::Stmt::Let {
        name: format!("#for{n}_unit"),
        mutable: false,
        ty: None,
        init: ast::Expr::BoolLit(false, span),
        span,
    });
    let some_body = ast::Expr::Block(ast::Block {
        stmts: some_stmts,
        span: body.span,
    });
    let none_body = ast::Expr::Block(ast::Block {
        stmts: vec![ast::Stmt::Break { span }],
        span,
    });
    let when = ast::Expr::When {
        scrutinee: Box::new(scrutinee),
        arms: vec![
            ast::WhenArm {
                pattern: Some(ast::VariantPattern {
                    variant: "Some".to_string(),
                    binds: vec![Some(binding.to_string())],
                    span: binding_span,
                }),
                body: some_body,
                span,
            },
            ast::WhenArm {
                pattern: Some(ast::VariantPattern {
                    variant: "None".to_string(),
                    binds: Vec::new(),
                    span,
                }),
                body: none_body,
                span,
            },
        ],
        span,
    };
    let while_body = ast::Block {
        stmts: vec![ast::Stmt::Expr { expr: when, span }],
        span,
    };
    let block = ast::Block {
        stmts: vec![
            ast::Stmt::Let {
                name: it_name,
                mutable,
                ty: None,
                init,
                span,
            },
            ast::Stmt::While {
                cond: ast::Expr::BoolLit(true, span),
                body: while_body,
                iterating: None,
                span,
            },
        ],
        span,
    };
    ast::Stmt::Expr {
        expr: ast::Expr::Block(block),
        span,
    }
}

/// One destructuring binding, lowered (`plan/syntax.md` 4): the whole value is
/// already bound to `base`, and every position the pattern binds is read out of
/// it by name. A nested pattern binds the position to a temporary of its own
/// first, so `val ((a, b), c) = ...` is two ordinary levels of the same
/// lowering. A position the pattern ignores is not read at all: what it holds
/// stays in the value that was destructured, whose own drop frees it.
#[allow(clippy::too_many_arguments)]
fn lower_pattern(
    pattern: &ast::BindingPattern,
    base: &str,
    base_ty: &Ty,
    elements: &[Ty],
    mutable: bool,
    ctx: &mut Ctx,
    stmts: &mut Vec<tir::Stmt>,
    span: Span,
) -> Result<()> {
    let ast::BindingPattern::Tuple(positions) = pattern else {
        return Err(TypeError {
            message: "a destructuring pattern is a parenthesized list of two or more positions ('val (lo, hi) = ...')".to_string(),
            span,
        });
    };
    for (i, position) in positions.iter().enumerate() {
        // The `tir` read of one position: a place (an owned local's field), so
        // an owned element read by value is the partial move A-11 already
        // plans, and a `Copy` element is a copy.
        let read = tir::Expr {
            kind: tir::ExprKind::FieldAccess {
                base: Box::new(tir::Expr {
                    kind: tir::ExprKind::Ident(base.to_string()),
                    ty: base_ty.clone(),
                    span,
                }),
                field: crate::types::tuple_field_name(i),
            },
            ty: elements[i].clone(),
            span,
        };
        match position {
            ast::BindingPattern::Ignore => {}
            ast::BindingPattern::Name(name) => {
                ctx.define(name, elements[i].clone(), mutable);
                stmts.push(tir::Stmt::Let {
                    name: name.clone(),
                    ty: elements[i].clone(),
                    init: read,
                });
            }
            ast::BindingPattern::Tuple(_) => {
                let nested = tuple_elems_of(&elements[i], ctx).ok_or_else(|| TypeError {
                    message: format!(
                        "position {} of the pattern is itself a pattern, and element {} of the tuple it destructures ('{}') is not a tuple; bind that element to one name instead",
                        i + 1,
                        i + 1,
                        elements[i].display()
                    ),
                    span,
                })?;
                ctx.tuple_temps += 1;
                let tmp = format!("{}{}", TUPLE_TEMP_PREFIX, ctx.tuple_temps - 1);
                let tmp_ty = elements[i].clone();
                ctx.define(&tmp, tmp_ty.clone(), false);
                stmts.push(tir::Stmt::Let {
                    name: tmp.clone(),
                    ty: tmp_ty.clone(),
                    init: read,
                });
                lower_pattern(position, &tmp, &tmp_ty, &nested, mutable, ctx, stmts, span)?;
            }
        }
    }
    Ok(())
}

/// The number of positions a pattern writes, which is the arity the tuple it
/// destructures must have.
fn pattern_arity(pattern: &ast::BindingPattern) -> usize {
    match pattern {
        ast::BindingPattern::Tuple(positions) => positions.len(),
        _ => 1,
    }
}

fn stmt_span(stmt: &ast::Stmt) -> Span {
    match stmt {
        ast::Stmt::Let { span, .. } => *span,
        ast::Stmt::LetTuple { span, .. } => *span,
        ast::Stmt::While { span, .. } => *span,
        ast::Stmt::For { span, .. } => *span,
        ast::Stmt::Break { span } | ast::Stmt::Continue { span } => *span,
        ast::Stmt::Return { span, .. } => *span,
        ast::Stmt::Expr { span, .. } => *span,
        ast::Stmt::Defer { span, .. } => *span,
    }
}

/// A `fun` literal (A-4): `fun(x: i32).return: i32 = x * 2`.
///
/// A literal is one synthesized top-level function, so most of the work is
/// the same work a declaration needs - resolve the signature, bind the
/// parameters, check the body against the declared return - and the rest is
/// the two things that make it a *literal*: its body is checked in a scope of
/// its own (no captures yet), and the checked body is stored where codegen
/// will emit it.
///
/// A *scoped* literal is checked by a retry loop (`plan/exclusive-captures.md`):
/// every owned `var` capture starts as a shared borrow, a write through one
/// (or passing it as a `mut` argument) records the name instead of being final,
/// and the literal is checked again with exactly those captures typed `&mut T`.
/// The set only grows and is bounded by the number of captures, so the loop
/// ends; a refusal that names no upgradable capture is returned as it is.
fn check_fn_literal(
    params_ast: &[ast::Param],
    infer_params: bool,
    return_type: &Option<ast::TypeExpr>,
    body: &ast::FunctionBody,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if !matches!(expected, Some(Ty::ScopedFn { .. })) {
        return check_fn_literal_pass(
            params_ast, infer_params, return_type, body, span, ctx, expected,
            &std::collections::HashSet::new(),
        );
    }
    let mut excl: std::collections::HashSet<String> = std::collections::HashSet::new();
    let saved_needed = ctx.excl_needed.clone();
    loop {
        let fresh: std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>> =
            Default::default();
        ctx.excl_needed = fresh.clone();
        let (lits, count) = {
            let st = ctx.fns.instances.borrow();
            (st.literals.len(), st.literal_count)
        };
        let probes_mark = ctx.probes.len();
        let result = check_fn_literal_pass(
            params_ast, infer_params, return_type, body, span, ctx, expected, &excl,
        );
        ctx.excl_needed = saved_needed.clone();
        if result.is_err() {
            ctx.probes.truncate(probes_mark);
        }
        match result {
            Ok(e) => return Ok(e),
            Err(err) => {
                let writable = writable_outer(ctx);
                // A need this level cannot meet (the name belongs to a frame
                // further out) is the enclosing literal's to upgrade.
                for n in fresh.borrow().iter().filter(|n| !writable.contains(*n)) {
                    saved_needed.borrow_mut().insert(n.clone());
                }
                let fresh_needs: Vec<String> = fresh
                    .borrow()
                    .iter()
                    .filter(|n| writable.contains(*n) && !excl.contains(*n))
                    .cloned()
                    .collect();
                if fresh_needs.is_empty() {
                    return Err(err);
                }
                excl.extend(fresh_needs);
                // The failed pass may have emitted nested literals; the next
                // pass writes them again under the same keys.
                let mut st = ctx.fns.instances.borrow_mut();
                st.literals.truncate(lits);
                st.literal_count = count;
            }
        }
    }
}

fn check_fn_literal_pass(
    params_ast: &[ast::Param],
    infer_params: bool,
    return_type: &Option<ast::TypeExpr>,
    body: &ast::FunctionBody,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
    excl: &std::collections::HashSet<String>,
) -> Result<tir::Expr> {
    // A literal written at a `scoped fun(...)` parameter is the non-escaping
    // kind: its captures may be borrows, its environment is stack, and the
    // value's own type is `Ty::ScopedFn` (`plan/scoped-blocks.md` part 2).
    let scoped = matches!(expected, Some(Ty::ScopedFn { .. }));
    // A `Mutex.lock` block (`std/sync`, B-5): the scoped parameter fixes the
    // block's own parameter types, but its return is the block's tail type, so
    // the expected `R` is ignored and filled in after the body is checked.
    let infer_ret = ctx.infer_scoped_ret && infer_params && scoped;
    // The function signature the *expected* type names, looked through a
    // borrow because an owned function-typed parameter arrives as
    // `&closure(...)`. A block literal takes both its parameter types and its
    // return from it; a written `fun` literal ignores it for the signature
    // and only uses it to learn whether it is scoped.
    let expected_sig = expected
        .map(|t| match t {
            Ty::Ref { inner, .. } => inner.as_ref(),
            other => other,
        })
        .and_then(|t| match t {
            Ty::Fn { params, ret }
            | Ty::Closure { params, ret }
            | Ty::ScopedFn { params, ret } => Some((params, ret)),
            _ => None,
        });
    // The signature: resolved where the literal was written for a `fun(...)`
    // literal (a parameter type resolves against the enclosing module and the
    // enclosing generic instance's type parameters, exactly as a nested
    // declaration's would), and taken from the expected function type for a
    // block header, which writes names only.
    let (params, ret) = if infer_params {
        match expected_sig {
            Some((want_params, want_ret)) => {
                if want_params.len() != params_ast.len() {
                    return Err(TypeError {
                        message: format!(
                            "this block declares {} parameter(s), but the parameter it is passed to expects a function of {}",
                            params_ast.len(),
                            want_params.len()
                        ),
                        span,
                    });
                }
                let params: Vec<(String, Ty)> = params_ast
                    .iter()
                    .zip(want_params)
                    .map(|(p, ty)| (p.name.clone(), ty.clone()))
                    .collect();
                (params, (**want_ret).clone())
            }
            // No function type to take types from. A headerless block keeps
            // the prototype's zero-parameter, unit-returning literal, so the
            // caller's own type comparison reports the mismatch; a block
            // that declares parameters has nothing to resolve them with, so
            // it is refused by name.
            None => {
                if !params_ast.is_empty() {
                    return Err(TypeError {
                        message: format!(
                            "this block declares parameter(s) ({}), but there is no function type at this position to take their types from - a block header is typed by the closure point it is passed to; write the parameter types on a 'fun(...)' literal instead",
                            params_ast
                                .iter()
                                .map(|p| p.name.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        span,
                    });
                }
                (Vec::new(), Ty::Unit)
            }
        }
    } else {
        let scope = ctx.type_scope();
        let mut params = Vec::with_capacity(params_ast.len());
        for p in params_ast {
            params.push((p.name.clone(), scope.resolve_param(p)?));
        }
        let ret = match return_type {
            Some(rt) => {
                // A borrow would outlive the frame that produced it, and a
                // function value is a plain pointer to code - the elision
                // rule that says *which* argument a borrow comes from has no
                // place to be written here.
                if rt.amp {
                    return Err(TypeError {
                        message: format!(
                            "a 'fun' literal cannot return a borrow ('&{}') - an indirect call has no parameter an elided lifetime could bind to; return an owned value instead",
                            rt.display()
                        ),
                        span: rt.span,
                    });
                }
                scope.resolve(rt, false)?
            }
            None => Ty::Unit,
        };
        (params, ret)
    };
    let mut ret = ret;
    if infer_ret {
        // The placeholder the body is checked against; `return EXPR` inside the
        // block therefore needs a `unit` value. The common shapes (a bare
        // `return`, or a tail expression) infer correctly below.
        ret = Ty::Unit;
    }
    // One literal, one function. The key starts with `#`, which no `.kp`
    // identifier can contain, so it cannot collide with anything a program
    // declares (the same trick receiver methods use for `Type#name`).
    let key = {
        let mut st = ctx.fns.instances.borrow_mut();
        st.literal_count += 1;
        format!("#funlit{}", st.literal_count)
    };
    // The body's own scope. The enclosing locals are not *bindings* here -
    // they are the names the body may capture, which is what makes the
    // literal a closure when it names one (A-4 phase E). The lookup map
    // carries their types, because a capture's type is the captured
    // binding's type. Three sources, because a literal can be nested:
    //
    // * the enclosing frame's locals,
    // * the enclosing literal's own captures (a literal inside a literal),
    // * and everything *that* literal could see, transitively - which is how
    //   `fun` inside `fun` reaches a binding of the function two frames up
    //   (the enclosing closure captures it in turn).
    let mut outer_bindings: HashMap<String, Ty> = ctx.outer_bindings.clone();
    for scope in ctx.scopes.iter() {
        for (k, v) in scope.iter() {
            outer_bindings.insert(k.clone(), v.ty.clone());
        }
    }
    for (name, ty) in &ctx.captures {
        outer_bindings.insert(name.clone(), ty.clone());
    }
    let mut inner = (*ctx).clone();
    inner.scopes = vec![HashMap::new()];
    inner.outer_bindings = outer_bindings;
    inner.captures = Vec::new();
    inner.outer_receiver = ctx
        .receiver_struct
        .clone()
        .or_else(|| ctx.outer_receiver.clone());
    inner.ret_ty = ret.clone();
    inner.receiver_struct = None;
    inner.init_state = None;
    inner.in_fn_literal = true;
    // A literal is its own function: a `break`/`continue` in its body targets
    // a loop in *it*, never a loop of the enclosing frame, and its synthesized
    // loop names start fresh.
    inner.loop_depth = 0;
    inner.for_temps = 0;
    // A nested scoped block is typed by its own callee, never by the
    // surrounding `Mutex.lock` block's inference.
    inner.infer_scoped_ret = false;
    inner.borrowed_captures = scoped;
    inner.excl_captures = excl.clone();
    inner.outer_mut = writable_outer(ctx);
    inner.outer_excl = ctx.excl_captures.union(&ctx.outer_excl).cloned().collect();
    let mut tir_params = Vec::with_capacity(params.len());
    for (name, ty) in &params {
        inner.define(name, ty.clone(), false);
        tir_params.push((name.clone(), ty.clone()));
    }
    let checked = match body {
        ast::FunctionBody::Block(b) => check_block(b, &mut inner, if infer_ret { None } else { Some(&ret) })?,
        ast::FunctionBody::Expr(e) => {
            let value = check_expr(e, &mut inner, if infer_ret { None } else { Some(&ret) })?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(value)),
            }
        }
        ast::FunctionBody::Extern | ast::FunctionBody::Signature => {
            unreachable!("the parser requires a literal body")
        }
    };
    if infer_ret {
        ret = checked.ty();
    }
    // The same tail-versus-declared-return rule a declaration follows,
    // including the one divergence case (`panic` as the tail).
    let tail_diverges = checked
        .tail
        .as_ref()
        .is_some_and(|e| matches!(&e.kind, tir::ExprKind::Call { callee, .. } if callee == "panic"));
    if ret != Ty::Unit
        && checked.tail.is_some()
        && !tail_diverges
        && !str_view_compatible(&checked.ty(), &ret)
    {
        return Err(TypeError {
            message: format!(
                "this 'fun' literal is declared to return '{}' but its body evaluates to '{}'",
                ret.display(),
                checked.ty().display()
            ),
            span,
        });
    }
    let capture_list = inner.captures.clone();
    // The environment: one field per capture, filled at the point the literal
    // is *evaluated* - which is where a capture moves its value out of the
    // enclosing frame. Each field's initializer is written in the enclosing
    // function's terms: a local by name, or - for a literal nested in a
    // literal - a read of the enclosing closure's environment.
    let mut captures = Vec::with_capacity(capture_list.len());
    let mut env_fields: Vec<(String, Ty)> = Vec::with_capacity(capture_list.len());
    for (i, (name, ty)) in capture_list.iter().enumerate() {
        // A scoped function value lives only for the call it was written in,
        // so an ordinary closure's environment - which can be returned - may
        // not hold one (`plan/scoped-blocks.md` part 2).
        if !scoped && matches!(ty, Ty::ScopedFn { .. }) {
            return Err(TypeError {
                message: format!(
                    "a closure cannot capture the scoped function value '{name}': a scoped value may only live for the call it was written in, and a closure's environment can outlive that call; pass it on to another 'scoped fun(...)' parameter instead"
                ),
                span,
            });
        }
        // An exclusive capture is a borrow that lives for the call, so the
        // block's own result must not be derived from it
        // (`plan/exclusive-captures.md`).
        if scoped && matches!(ty, Ty::Ref { mutable: true, .. }) && crate::types::is_borrowed_field_ty(&ret) {
            return Err(TypeError {
                message: format!(
                    "this block writes through '{name}' (an exclusive capture) and returns a borrow ('{}'): the borrow would outlive the call that holds '{name}' exclusively; return an owned value (a copy or a 'String.from_str(...)') instead",
                    ret.display()
                ),
                span,
            });
        }
        // A borrow cannot be captured by an ordinary closure (its environment
        // outlives the frame), but a scoped literal's environment is stack and
        // lives only for the call, so a borrowed *value* (a `&T` parameter, a
        // `str` view) is simply copied in - it is a pointer, and the borrow it
        // carries is already live in the enclosing frame.
        if crate::types::is_borrowed_field_ty(&ty) && !scoped {
            return Err(TypeError {
                message: format!(
                    "a 'fun' literal cannot capture '{name}': it is a borrow ('{}'), and the environment it would be stored in outlives the frame the borrow points into - capture an owned value, or pass it as a parameter of the literal",
                    ty.display()
                ),
                span,
            });
        }
        // The environment field's own type is the capture's recorded type,
        // which `note_capture` already turned into `&T` for a scoped literal's
        // owned capture. Such a capture's *initializer* is the one thing left
        // to decide: `&name` for the owned binding that `note_capture`
        // borrowed, a plain read for a value that is already a pointer.
        let outer_owned = ctx
            .lookup(name)
            .map(|i| i.ty.clone())
            .filter(|t| {
                !crate::types::is_borrowed_field_ty(t)
                    && (!t.is_copy() || matches!(&ty, Ty::Ref { mutable: true, .. }))
            });
        let field_ty = ty.clone();
        let init = if ctx.lookup(name).is_some() {
            if scoped && outer_owned.is_some() {
                let place = tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: outer_owned.expect("checked above"),
                    span,
                };
                tir::Expr {
                    kind: tir::ExprKind::Ref {
                        mutable: matches!(&field_ty, Ty::Ref { mutable: true, .. }),
                        expr: Box::new(place),
                    },
                    ty: field_ty.clone(),
                    span,
                }
            } else {
                // Otherwise the value moves (or, for `Copy`, copies) into the
                // environment, and the enclosing binding is marked moved by
                // `borrowck.rs`.
                tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: ty.clone(),
                    span,
                }
            }
        } else if ctx.note_capture(name).is_some() {
            // The name belongs to the environment of the literal this one was
            // written inside - so the enclosing literal captures it too, and
            // this literal copies it out of that environment. Moving it out
            // instead would be a partial move (A-11), which is why only a
            // `Copy` capture can be nested this way.
            if !ty.is_copy() {
                return Err(TypeError {
                    message: format!(
                        "cannot capture '{name}' into this literal: it belongs to the enclosing closure's environment, which is not a local this frame owns, so there is no owned place to move it out of; capture an owned value from the frame that owns it, or copy a `Copy` one"
                    ),
                    span,
                });
            }
            let index = ctx
                .captures
                .iter()
                .position(|(n, _)| n == name)
                .expect("note_capture records the name");
            tir::Expr {
                kind: tir::ExprKind::Capture { index },
                ty: ty.clone(),
                span,
            }
        } else {
            unreachable!("a capture is always an enclosing local or an enclosing capture")
        };
        env_fields.push((crate::types::capture_field(i), field_ty));
        captures.push(init);
    }
    let fn_ty = if env_fields.is_empty() {
        // Nothing captured: this is a plain function value, exactly the value
        // phase C produced. Keeping the two apart is what keeps every
        // existing `fun(...)` placement (a `datac` field, a container
        // element's absence, a `Copy` binding) meaning what it did. A scoped
        // literal with nothing captured is widened into its fat pointer at
        // the call site, exactly as a plain function value used where a
        // closure is expected is.
        Ty::Fn {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    } else if scoped {
        Ty::ScopedFn {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    } else {
        Ty::Closure {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    };
    let env = if env_fields.is_empty() {
        None
    } else {
        Some(tir::ClosureEnv {
            struct_name: crate::types::env_struct_name(&key),
            drop_name: crate::types::env_drop_name(&key),
            fields: env_fields,
        })
    };
    ctx.fns.instances.borrow_mut().literals.push(tir::Function {
        name: key.clone(),
        params: tir_params,
        ret,
        body: checked,
        // A literal has no `fun` declaration of its own, so the literal's own
        // span is the only line that describes it.
        span,
        module: ctx.module.clone(),
        extern_: false,
        env,
        naked: false,
        align: None,
        section: None,
    });
    let kind = match captures.is_empty() {
        true => tir::ExprKind::FnRef(key),
        // A scoped literal's environment is stack and its drop is `NULL`
        // (`plan/scoped-blocks.md`), which is a node of its own so neither
        // backend ever mallocs it or frees it.
        false if scoped => tir::ExprKind::ScopedClosureLit { key, captures },
        false => tir::ExprKind::ClosureLit { key, captures },
    };
    Ok(tir::Expr {
        kind,
        ty: fn_ty,
        span,
    })
}

/// Checks `expr` against an expected type, then applies the one implicit
/// conversion the language has between borrowed and owned text (open item 21).
///
/// This is the wrapper, not the body: putting the conversion *here* means
/// every site that knows an expected type gets it - a call argument, a `val`
/// with a written type, a `return`, a `datac` field, a container element, a
/// `when` arm, a block tail - with no list of sites to keep in sync. The
/// alternative shape (coerce at each site) is the same behaviour with N
/// chances to forget one.
fn check_expr(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    // A failed check can be retried with other state by a caller that swallows
    // the error, so what it left in `probes` must not outlive it.
    let mark = ctx.probes.len();
    let value = match check_expr_inner(expr, ctx, expected) {
        Ok(v) => v,
        Err(e) => {
            ctx.probes.truncate(mark);
            return Err(e);
        }
    };
    let value = coerce_str_to_string(value, expected);
    Ok(coerce_handle_upcast(value, expected, &ctx.env.handle_parents))
}

/// A handle value used where one of its ancestors is expected converts with no
/// runtime cost (gap 1): the C typedef and the LLVM type are both a plain
/// pointer, so the value is the same bit pattern and only its *type* changes.
/// Only the child-to-ancestor direction fires; a parent used where a child is
/// expected is a type mismatch, and `Parent.cast(child)` is the explicit way to
/// reinterpret one.
fn coerce_handle_upcast(
    value: tir::Expr,
    expected: Option<&Ty>,
    parents: &HashMap<String, String>,
) -> tir::Expr {
    let Some(Ty::Handle(want)) = expected else {
        return value;
    };
    let Ty::Handle(have) = &value.ty else {
        return value;
    };
    if have == want || !handle_is_same_or_ancestor(parents, want, have) {
        return value;
    }
    tir::Expr {
        ty: Ty::Handle(want.clone()),
        ..value
    }
}

/// A `str` used where a `String` is expected becomes one, by `String.from_str`.
///
/// `String.from_str("...")` was the single most repeated construct in the JSON
/// library this came from - once per parser error message, once per `kind()`
/// arm, once per `String.from_str("null")` - because a literal is a `str` and
/// an owned field is a `String`, and nothing bridged the two. The conversion is
/// a copy, which is what an owned value has to be; nothing implicit the other
/// way (`String` to `str`) is added here, because that direction is a
/// *borrow* with a lifetime, and borrowing a value you are about to free is
/// the borrow checker's business, not a conversion's.
///
/// Only an *exact* `String` expectation fires it. A `&String` parameter is
/// left alone (the caller writes `&`-shaped arguments the same way it always
/// did), and a `str` parameter is left alone entirely, so `str.eq(a, b)` and
/// `str.len(s)` keep taking views with no allocation.
fn coerce_str_to_string(value: tir::Expr, expected: Option<&Ty>) -> tir::Expr {
    if !matches!(expected, Some(Ty::String)) {
        return value;
    }
    // A `View` needs a different conversion, not the same one: it may be a
    // *slice*, with no NUL after it, so `String.from_str` (which is `strlen`
    // and a copy) would copy past the slice into the rest of the source. The
    // length-aware copy is the only correct one, and it is `View.to_string`.
    let callee = match &value.ty {
        t if is_str_like(t) => "String#from_str",
        Ty::View => "View#to_string",
        _ => return value,
    };
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::Call {
            callee: callee.to_string(),
            args: vec![value],
        },
        ty: Ty::String,
        span,
    }
}

fn check_expr_inner(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    // A `fun` literal is checked first, with its expected type in hand: a
    // block literal (`{ x -> ... }`) takes its parameters' and its return's
    // types from the expected function type, and a scoped literal needs the
    // expectation to know that borrow captures are allowed
    // (`plan/scoped-blocks.md`).
    if let ast::Expr::FnLit {
        params,
        infer_params,
        return_type,
        body,
        span,
    } = expr
    {
        let value = check_fn_literal(params, *infer_params, return_type, body, *span, ctx, expected)?;
        if let Some(want) = expected {
            if matches!(want, Ty::Closure { .. }) {
                return Ok(coerce_fn_to_closure(want.clone(), value));
            }
            if matches!(want, Ty::ScopedFn { .. }) {
                return Ok(coerce_fn_to_scoped(want.clone(), value));
            }
        }
        return Ok(value);
    }
    // A plain function value where a closure is expected (A-4 phase E): the
    // one widening in this phase, handled here so that every site which knows
    // the expected type gets it - a call argument, a `val` with a written
    // type, a `return`, a `datac` field, `Box.of(x)`. The value is checked
    // with no expectation of its own (nothing about a `fun` literal depends
    // on the closure type it will be widened into) and then wrapped.
    if let Some(Ty::Closure { .. }) = expected {
        let expected = expected.expect("matched above");
        let value = check_expr(expr, ctx, None)?;
        return Ok(coerce_fn_to_closure(expected.clone(), value));
    }
    // A borrow written where a `&dyn Trait` is expected (A-12 slice 4): the one
    // coercion this slice adds, and the reason it is here is that every site
    // that knows an expected type (a call argument, a typed `val`/`var`, a
    // `return`, a field) reaches the operand through this function. The operand
    // is checked with no expectation, so what is coerced is the plain `&x` it
    // already is.
    if let Some(Ty::Dyn { .. }) = expected {
        let want = expected.expect("matched above").clone();
        let value = check_expr(expr, ctx, None)?;
        return coerce_to_dyn(want, value, expr.span(), ctx);
    }
    // A `Box<T>` where a `Box<dyn Trait>` is expected (A-12 slice 7): the same
    // expected-type position, with the box moved in rather than copied. The
    // operand is checked with no expectation, so what is coerced is the box it
    // already is.
    if let Some(Ty::BoxDyn { .. }) = expected {
        let want = expected.expect("matched above").clone();
        // `Box.empty()` is the one box constructor whose trait only the written
        // type can supply, so it is answered through the ordinary associated
        // path with the expected type in hand (which also keeps a local or an
        // alias named `Box` shadowing the type name, exactly as it does
        // everywhere else). Every other expression is the box the coercion
        // moves in.
        if let ast::Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            span,
        } = expr
        {
            if empty_container_call(expr) == Some(("Box", "empty")) {
                return check_method_call(
                    receiver,
                    method,
                    type_args,
                    args,
                    *span,
                    ctx,
                    Some(&want),
                );
            }
        }
        let value = check_expr(expr, ctx, None)?;
        return coerce_box_to_dyn(want, value, expr.span(), ctx);
    }
    match expr {
        // Handled above, before the closure widening.
        ast::Expr::FnLit { .. } => unreachable!("a 'fun' literal is checked before this match"),
        // H-12: an integer literal where a float is expected is that float
        // (`val w: f64 = 3`, `x * 2` with `x: f64`, `f(255)` for an `f32`
        // parameter), when the float holds it exactly. A *value* never
        // converts; only the literal's own spelling adapts.
        ast::Expr::IntLit(v, span) if expected.is_some_and(|t| t.is_float()) => {
            let ty = expected.expect("checked above").clone();
            let exact = if ty == Ty::F32 { 1u64 << 24 } else { 1u64 << 53 };
            if *v > exact {
                return Err(TypeError {
                    message: format!(
                        "the literal {v} is not exactly representable as '{}': write it as a float ({v}.0) to accept the rounding, or use a smaller number",
                        ty.display()
                    ),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v as f64),
                ty,
                span: *span,
            })
        }
        ast::Expr::IntLit(v, span) => {
            let (ty, defaulted) = int_lit_ty(expected);
            check_int_literal(*v as i128, &ty, defaulted, *span)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v as i128),
                ty,
                span: *span,
            })
        }
        // A character literal (open item 53) takes the type its context asks
        // for, exactly as an integer literal does, and `char` when nothing
        // constrains it. That single rule is what lets byte-level code read
        // the way it thinks: `c == '"'` with `c: u8`, `out.push(' ')` into a
        // `String`, `n + '0'` to make a digit.
        //
        // A code point that does not fit the requested integer type is
        // refused rather than truncated - `'中'` in a `u8` position is a
        // mistake, not a byte to keep.
        ast::Expr::CharLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_integer() => {
                    if !char_fits(*v, t) {
                        return Err(TypeError {
                            message: format!(
                                "the character '{}' (U+{:04X}) does not fit in '{}'",
                                printable_codepoint(*v),
                                v,
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                    t.clone()
                }
                _ => Ty::Char,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::CharLit(*v),
                ty,
                span: *span,
            })
        }
        ast::Expr::FloatLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_float() => t.clone(),
                _ => Ty::F64,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v),
                ty,
                span: *span,
            })
        }
        // A byte literal (H-11) is a `u8` by construction, unlike a character
        // literal, which takes the integer type its context asks for. That is
        // the whole difference: `b'\n'` needs no type written on the binding,
        // and it is refused (rather than widened) where the context wants
        // something other than a byte, which is what `'\n'` is for.
        ast::Expr::ByteLit(v, span) => {
            if let Some(t) = expected {
                if !matches!(t, Ty::U8) {
                    return Err(TypeError {
                        message: format!(
                            "a byte literal is a 'u8', not a '{}': write it as a character literal when it should take this type",
                            t.display()
                        ),
                        span: *span,
                    });
                }
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v as i128),
                ty: Ty::U8,
                span: *span,
            })
        }
        ast::Expr::StringLit(s, span) => Ok(tir::Expr {
            kind: tir::ExprKind::StringLit(s.clone()),
            ty: Ty::Str,
            span: *span,
        }),
        ast::Expr::BoolLit(b, span) => Ok(tir::Expr {
            kind: tir::ExprKind::BoolLit(*b),
            ty: Ty::Bool,
            span: *span,
        }),
        ast::Expr::Ident(name, span) => {
            if let Some(info) = ctx.lookup(name) {
                // An open container takes its element from the type this
                // position expects: `take(xs)` on a `fun take(xs: List<i32>)`
                // decides `xs` the same way `xs.push(1)` does, and the repair
                // is the same one (`check_block` rewrites the binding's local
                // to match).
                let ty = if crate::types::is_open_container(&info.ty) {
                    let open = info.ty.clone();
                    let keyable = ctx.keyable_set();
                    match expected
                        .and_then(|exp| crate::types::fill_open_container(&open, exp, &keyable))
                    {
                        Some(filled) => {
                            ctx.set_binding_ty(name, filled.clone());
                            filled
                        }
                        None => open,
                    }
                } else {
                    info.ty.clone()
                };
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty,
                    span: *span,
                });
            }
            // A const type parameter read as a value (A-20): the
            // instantiation folded it to a literal, so `i < N` and `[0; N]`
            // see the value the monomorphization was built for.
            if let Some(Ty::ConstInt { value, ty }) = ctx.type_args.get(name) {
                let kind = match **ty {
                    Ty::Bool => tir::ExprKind::BoolLit(*value != 0),
                    Ty::Char => tir::ExprKind::CharLit(*value as u32),
                    _ => tir::ExprKind::IntLit(*value),
                };
                return Ok(tir::Expr {
                    kind,
                    ty: (**ty).clone(),
                    span: *span,
                });
            }
            // Not a local or parameter - inside a receiver-form method,
            // fall back to the receiver's field of that name (no `self.`
            // qualifier is ever written; see `plan/syntax.md` §5).
            if let Some(field_ty) = ctx.receiver_field_ty(name) {
                let self_ty = ctx.lookup("self").expect("receiver binding exists whenever receiver_struct is set").ty.clone();
                let base = tir::Expr { kind: tir::ExprKind::Ident("self".to_string()), ty: self_ty, span: *span };
                return Ok(tir::Expr {
                    kind: tir::ExprKind::FieldAccess { base: Box::new(base), field: name.clone() },
                    ty: field_ty,
                    span: *span,
                });
            }
            // A passdown is a hidden *argument* of the function that names
            // it, so a function value cannot carry one (A-4): a literal that
            // read one would be a function pointer with a parameter its
            // signature does not have. Refused where it is written, with the
            // way round it.
            if ctx.in_fn_literal && ctx.pds.contains_key(name) {
                return Err(TypeError {
                    message: format!(
                        "a 'fun' literal cannot read passdown '{name}' - a passdown is passed to the function that names it as a hidden argument, and a function value is a plain pointer to code; pass the value as a parameter instead"
                    ),
                    span: *span,
                });
            }
            // Not a local or receiver field - a passdown (M5.2), which the
            // wiring scan has already made this function take as a hidden
            // parameter named `pd#...`, so the reference is that parameter.
            if let Some(pd) = ctx.pds.get(name).cloned() {
                if !ctx.pd_initialized(&pd.hidden) {
                    return Err(TypeError {
                        message: format!(
                            "passdown '{}' is read before it is initialized - a 'pd' initializer can only use passdowns declared before it",
                            pd.qualified
                        ),
                        span: *span,
                    });
                }
                // In a `pd` initializer only Copy values can be read out of
                // a passdown: a non-Copy one is handed around as a shared
                // borrow, and a passdown has to hold a value.
                if ctx.init_state.is_some() && !pd.value_ty.is_copy() {
                    return Err(TypeError {
                        message: format!(
                            "passdown '{}' cannot be read in another passdown's initializer: its value is not Copy",
                            pd.qualified
                        ),
                        span: *span,
                    });
                }
                return Ok(pd.value(*span, ctx.pd_global_borrow, ctx.pd_exclusive(&pd)));
            }
            // Not a local, receiver field, or passdown - a declared `fun`
            // named as a *value* (A-4): `val f = double`. A function
            // reference carries no environment, so the C function itself is
            // the value.
            if let Some((key, fn_ty)) = function_value(name, ctx, *span)? {
                return Ok(tir::Expr {
                    kind: tir::ExprKind::FnRef(key),
                    ty: fn_ty,
                    span: *span,
                });
            }
            // Not a local, receiver field, or passdown - a compile-time
            // constant (M6.3), substituted as the literal it folded to.
            if let Some(c) = ctx.consts.get(&ctx.qualify_own(name)).cloned() {
                let mut value = c.value;
                value.span = *span;
                return Ok(value);
            }
            // Not a local, receiver field, passdown, function value or
            // constant - a top-level `static` (kernel stage 7). A static is a
            // real object, so a use is a `StaticRef` place; a `static mut`
            // may only be named inside `unsafe`, and the gate is here so a
            // read, a write, a field access and an address are all covered
            // by the one check.
            if let Some(s) = ctx.statics.get(&ctx.qualify_own(name)).cloned() {
                require_static_unsafe(&s, *span, ctx)?;
                return Ok(tir::Expr {
                    kind: tir::ExprKind::StaticRef(s.qualified),
                    ty: s.ty,
                    span: *span,
                });
            }
            // Inside a `fun` literal, a name that is none of the above and
            // *is* an enclosing binding is a capture (A-4 phase E): the
            // literal becomes a closure, the value is moved (or, for a `Copy`
            // type, copied) into an environment when the literal is
            // evaluated, and the body reads it out of that environment. The
            // read is recorded here, once per distinct name, in the order the
            // body first names them, which is the environment's field order.
            if ctx.in_fn_literal {
                if let Some(ty) = ctx.note_capture(name) {
                    let index = ctx
                        .captures
                        .iter()
                        .position(|(n, _)| n == name)
                        .expect("note_capture records the name");
                    // A `Copy` binding the literal writes is captured by
                    // exclusive borrow (`plan/exclusive-captures.md`), and its
                    // reads and writes go through the pointer, so the name is
                    // the place the pointer addresses.
                    if let Ty::Ref { mutable: true, inner } = &ty {
                        if ctx.excl_captures.contains(name) && inner.is_copy() {
                            let place_ty = (**inner).clone();
                            return Ok(tir::Expr {
                                kind: tir::ExprKind::Deref {
                                    expr: Box::new(tir::Expr {
                                        kind: tir::ExprKind::Capture { index },
                                        ty: ty.clone(),
                                        span: *span,
                                    }),
                                },
                                ty: place_ty,
                                span: *span,
                            });
                        }
                    }
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Capture { index },
                        ty,
                        span: *span,
                    });
                }
                // A field of the receiver the literal was written in is a
                // name the reader can *see*, but reading it out of a borrowed
                // receiver is a partial move (A-11) - which is why the field
                // itself, not just the literal, has to be the reason.
                if let Some(owner) = &ctx.outer_receiver {
                    if ctx.field_of(owner, name).is_some() {
                        return Err(TypeError {
                            message: format!(
                                "a 'fun' literal cannot capture '{name}': it is a field of '{owner}', and the receiver is a borrow, so its field is not this frame's to move out; bind a `Copy` field to a local first (`val {name}0 = {name}`) and capture that, or take the whole receiver as a 'move' parameter"
                            ),
                            span: *span,
                        });
                    }
                }
            }
            Err(TypeError {
                message: match ctx.foreign_pds.get(name) {
                    Some(module) => format!(
                        "'{name}' is a passdown of module '{module}' - a passdown is only visible inside the module that declares it"
                    ),
                    // Inside a `pd` initializer only passdowns declared
                    // *before* it are in scope at all, so the generic name is
                    // usually a forward reference.
                    None if ctx.init_state.is_some() => format!(
                        "undefined variable '{name}' - a 'pd' initializer can only use passdowns declared before it"
                    ),
                    None => format!("undefined variable '{name}'"),
                },
                span: *span,
            })
        }
        ast::Expr::Unary { op, expr: sub, span } => {
            // `-2147483648` is an `i32`, but the literal under the sign is
            // 2147483648, which no `i32` holds on its own. The sign is part of
            // the value when the range is judged, so the pair is checked here
            // and the literal's own check, which cannot see the sign, is
            // skipped. Every other operand is checked below as it always was.
            if *op == UnOp::Neg && !expected.is_some_and(|t| t.is_float()) {
                if let ast::Expr::IntLit(v, lit_span) = &**sub {
                    let (ty, defaulted) = int_lit_ty(expected);
                    check_int_literal(-(*v as i128), &ty, defaulted, *lit_span)?;
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Unary {
                            op: *op,
                            expr: Box::new(tir::Expr {
                                kind: tir::ExprKind::IntLit(*v as i128),
                                ty: ty.clone(),
                                span: *lit_span,
                            }),
                        },
                        ty,
                        span: *span,
                    });
                }
            }
            let sub_expected = if *op == UnOp::Not {
                Some(&Ty::Bool)
            } else if *op == UnOp::BitNot {
                // `~x` is integral-only, and the expectation only helps when
                // it is an integer (an integer literal has no other way to
                // take a width from `~`).
                expected.filter(|t| t.is_integer())
            } else {
                expected
            };
            let sub_t = check_expr(sub, ctx, sub_expected)?;
            match op {
                UnOp::Neg => {
                    if !sub_t.ty.is_numeric() {
                        // `-a` on a user type that implements `std/ops.Neg` is
                        // `a.neg()` (`plan/traits.md` 13.2), with the operand
                        // borrowed rather than moved.
                        if let Some(owner) =
                            unary_operator_owner(&sub_t, crate::types::NEG_TRAIT, ctx)
                        {
                            return Ok(lower_operator_call(&owner, "neg", &[&sub_t], *span, ctx)?);
                        }
                        return Err(TypeError {
                            message: format!("cannot negate a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
                UnOp::Not => {
                    if sub_t.ty != Ty::Bool {
                        return Err(TypeError {
                            message: format!("cannot apply '!' to a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
                UnOp::BitNot => {
                    if !sub_t.ty.is_integer() {
                        return Err(TypeError {
                            message: format!(
                                "cannot apply '~' to a value of type '{}' - '~' complements the bits of an integer",
                                sub_t.ty.display()
                            ),
                            span: *span,
                        });
                    }
                }
            }
            let ty = sub_t.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Unary {
                    op: *op,
                    expr: Box::new(sub_t),
                },
                ty,
                span: *span,
            })
        }
        ast::Expr::Binary { op, lhs, rhs, span } => check_binary(*op, lhs, rhs, *span, ctx, expected),
        ast::Expr::Call {
            callee,
            type_args,
            args,
            span,
        } => check_call(callee, type_args, args, *span, ctx, expected),
        ast::Expr::FieldAccess { base, field, span } => {
            if let ast::Expr::Ident(name, _) = base.as_ref() {
                if is_module_alias(name, ctx) {
                    // `math.PI` - a module's `const`, written without call
                    // parentheses (M6.3). Everything else in a module is a
                    // function and needs a call.
                    if let Some(prefix) = ctx.aliases.get(name).cloned() {
                        let qualified = format!("{prefix}{field}");
                        if declares(&qualified, ctx) {
                            ctx.check_exported(&qualified, name, field, *span)?;
                        }
                        if let Some(c) = ctx.consts.get(&qualified).cloned() {
                            let mut value = c.value;
                            value.span = *span;
                            return Ok(value);
                        }
                        // `other.NAME` - a module's `pub static`, written
                        // without call parentheses, exactly as its `const` is.
                        if let Some(s) = ctx.statics.get(&qualified).cloned() {
                            require_static_unsafe(&s, *span, ctx)?;
                            return Ok(tir::Expr {
                                kind: tir::ExprKind::StaticRef(s.qualified),
                                ty: s.ty,
                                span: *span,
                            });
                        }
                        // `val f = math.sqrt` - a module's item named as a
                        // function value (A-4), the qualified spelling of the
                        // same thing a bare name means inside its own module.
                        if let Some((key, fn_ty)) =
                            function_value_at(&qualified, &format!("{name}.{field}"), ctx, *span)?
                        {
                            return Ok(tir::Expr {
                                kind: tir::ExprKind::FnRef(key),
                                ty: fn_ty,
                                span: *span,
                            });
                        }
                        // The float math functions have an `f32` overload beside
                        // the `f64` one, and `val m = math.sqrt` predates it: as
                        // a value the name is the overload the expected function
                        // type names, or the `f64` one when nothing does.
                        if FLOAT_MATH_VALUES.contains(&field.as_str()) {
                            let wants = match expected {
                                Some(Ty::Fn { params, .. }) => Some(params.clone()),
                                _ => None,
                            };
                            let mut keys: Vec<String> = ctx
                                .sigs
                                .keys()
                                .filter(|k| k.split('@').next() == Some(&qualified))
                                .cloned()
                                .collect();
                            keys.sort();
                            let pick = keys.iter().find(|k| {
                                let sig = &ctx.sigs[*k];
                                match &wants {
                                    Some(p) => sig.params.iter().map(|(_, t)| t).eq(p.iter()),
                                    None => sig.params.iter().all(|(_, t)| *t == Ty::F64),
                                }
                            });
                            if let Some(key) = pick {
                                let fn_ty = fn_value_ty(&ctx.sigs[key]);
                                return Ok(tir::Expr {
                                    kind: tir::ExprKind::FnRef(key.clone()),
                                    ty: fn_ty,
                                    span: *span,
                                });
                            }
                        }
                        if ctx.sigs.keys().any(|k| k.split('@').next() == Some(&qualified)) {
                            return Err(TypeError {
                                message: format!(
                                    "'{name}.{field}' is an overloaded stdlib function - its parameter types are part of the name, so it is not a value; write a non-generic wrapper function and pass that"
                                ),
                                span: *span,
                            });
                        }
                    }
                    return Err(TypeError {
                        message: format!(
                            "'{name}' is a module, not a value - call its items as '{name}.{field}(...)'"
                        ),
                        span: *span,
                    });
                }
            }
            // A cross-module type path (`shapes.Shape.Empty`) is gated
            // before it is read as a variant: the type has to be exported.
            if let Some(path) = expr_path(base) {
                ctx.check_type_path_exported(&path, *span)?;
            }
            // `Option.None` - a payload-less variant of a generic `enum`
            // (A-2), the generic counterpart of the `Shape.Empty` case below.
            if let Some(tmpl_name) = generic_type_path(base, ctx) {
                return check_generic_bare_variant(&tmpl_name, field, *span, ctx, expected);
            }
            // `Shape.Empty` - a payload-less variant, written without a call
            // (A-1). `Shape.Circle` is the error case: a variant that carries
            // payload has to be constructed with its arguments.
            if let Some(ename) = enum_path(base, ctx) {
                let edef = ctx
                    .enum_def(&ename)
                    .expect("enum_path only returns known enums");
                let Some((vidx, vdef)) = edef.variant(field) else {
                    let names: Vec<String> = edef
                        .variants
                        .iter()
                        .map(|v| format!("'{}'", v.name))
                        .collect();
                    return Err(TypeError {
                        message: format!(
                            "'{field}' is not a variant of enum '{ename}' - the variants are {}",
                            names.join(", ")
                        ),
                        span: *span,
                    });
                };
                if !vdef.fields.is_empty() {
                    let fields: Vec<String> =
                        vdef.fields.iter().map(|f| f.name.clone()).collect();
                    return Err(TypeError {
                        message: format!(
                            "variant '{}.{}' carries payload ({}) - construct it as '{}.{}({})'",
                            ename,
                            field,
                            fields.join(", "),
                            ename,
                            field,
                            fields.join(" = ..., ")
                        ),
                        span: *span,
                    });
                }
                return Ok(tir::Expr {
                    kind: tir::ExprKind::EnumLit {
                        enum_name: ename.clone(),
                        variant: vidx,
                        fields: Vec::new(),
                    },
                    ty: Ty::Struct(ename),
                    span: *span,
                });
            }
            let base_t = check_expr(base, ctx, None)?;
            let Some(sname) = struct_name_of(&base_t.ty).map(|s| s.to_string()) else {
                return Err(TypeError {
                    message: format!("cannot access field '{field}' on non-struct type '{}'", base_t.ty.display()),
                    span: *span,
                });
            };
            // An enum has no fields to read: its payload is reached through a
            // pattern, and its tag through `when` (A-1).
            if ctx.is_enum(&sname) {
                return Err(TypeError {
                    message: format!(
                        "enum '{sname}' has no field '{field}' - match it with 'when {sname} {{ ... }}' to read a payload"
                    ),
                    span: *span,
                });
            }
            let sdef = ctx.struct_def(&sname).expect("struct types are only produced for known structs");
            let field_ty = sdef.field(field).map(|f| f.ty.clone()).ok_or_else(|| TypeError {
                message: format!("datac '{sname}' has no field '{field}'"),
                span: *span,
            })?;
            Ok(tir::Expr {
                kind: tir::ExprKind::FieldAccess {
                    base: Box::new(base_t),
                    field: field.clone(),
                },
                ty: field_ty,
                span: *span,
            })
        }
        ast::Expr::Assign { target, value, span } => check_assign(target, value, *span, ctx),
        ast::Expr::If {
            cond,
            then_block,
            else_block,
            span,
        } => check_if(cond, then_block, else_block.as_deref(), *span, ctx, expected),
        ast::Expr::Block(block) => {
            let b = check_block(block, ctx, expected)?;
            let ty = b.ty();
            Ok(tir::Expr {
                kind: tir::ExprKind::Block(b),
                ty,
                span: block.span,
            })
        }
        // `unsafe { ... }` (MK9-S2 stage 1): the same block, checked with the
        // unsafe nesting raised by one so the operations that need it are
        // legal inside. Nothing about the lowered block differs, so the two
        // backends need no new shape for it: the call is the unsafe operation.
        ast::Expr::Unsafe { block, span } => {
            ctx.unsafe_depth += 1;
            let checked = check_block(block, ctx, expected);
            ctx.unsafe_depth -= 1;
            let b = checked?;
            let ty = b.ty();
            Ok(tir::Expr {
                kind: tir::ExprKind::Block(b),
                ty,
                span: *span,
            })
        }
        // `asm(...)` (kernel stage 5): an unsafe operation in its own right,
        // so it needs the `unsafe` context on top of the module's `[unsafe]`
        // listing (which `ast::first_unsafe` already demanded).
        ast::Expr::Asm {
            template,
            operands,
            clobbers,
            nomem,
            nostack: _,
            span,
        } => check_asm(template, operands, clobbers, *nomem, *span, ctx, expected),
        ast::Expr::Try { expr: sub, span } => check_try(sub, *span, ctx),
        ast::Expr::Ref { mutable, expr: sub, span } => {
            let sub_t = check_expr(sub, ctx, None)?;
            // The address has to outlive the statement the borrow is written
            // in, and a field of a call's result does not: that temporary is
            // staged for one call, not for a binding. `is_durable_place` is the
            // typed form of the question, and the reason an explicit
            // `&headers.get(i).name` is legal where `&mkName().name` is not:
            // the chain steps through a borrow, so the storage is the
            // pointee's (`syntax.md` 10.2).
            if !is_durable_place(&sub_t) {
                return Err(TypeError {
                    message: "cannot borrow a temporary value".to_string(),
                    span: *span,
                });
            }
            // A parameter of an owned type is already a borrow (`syntax.md`
            // 10.2), so `&o` on one is a borrow of a borrow. Left alone the
            // two references unify badly and the caller sees an inference
            // failure naming a type parameter instead of this rule.
            if let Ty::Ref { .. } = sub_t.ty {
                let written = match sub.as_ref() {
                    ast::Expr::Ident(name, _) => format!("'&{name}'"),
                    _ => "this borrow".to_string(),
                };
                return Err(TypeError {
                    message: format!(
                        "this is a borrow of a borrow: {written} borrows a value that is already a borrow, and Kelp has no '&&T'; pass the borrow itself"
                    ),
                    span: *span,
                });
            }
            if *mutable {
                check_target_writable(&sub_t, ctx)?;
            }
            let ty = Ty::Ref { mutable: *mutable, inner: Box::new(sub_t.ty.clone()) };
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::Deref { expr: sub, span } => {
            let sub_t = check_expr(sub, ctx, None)?;
            let Ty::Ref { inner, .. } = &sub_t.ty else {
                return Err(TypeError {
                    message: format!("cannot dereference a value of type '{}'", sub_t.ty.display()),
                    span: *span,
                });
            };
            let ty = inner.as_ref().clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Deref { expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::Template { parts, span } => {
            // Only reachable when a template is *not* the argument of
            // `print`/`print!` (that path is handled in `check_call`).
            let _ = (parts, span);
            Err(TypeError {
                message: "a string template can only be printed: pass it to 'print(...)' or 'print!(...)'; a template is not a value, so build the text with 'str.concat', 'fmt.*' or a 'String'".to_string(),
                span: *span,
            })
        }
        ast::Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            span,
        } => {
            // `circle.make(...)`: a module-qualified reference to an
            // imported item, not a method call. Recognized only when the
            // receiver is a bare identifier that names an `impl` alias and
            // is *not* shadowed by a local (locals win, exactly like any
            // other name lookup); anything else is a real method call.
            if let ast::Expr::Ident(name, _) = receiver.as_ref() {
                if is_module_alias(name, ctx) {
                    if let Some(prefix) = ctx.aliases.get(name).cloned() {
                        return check_module_item(
                            name,
                            &prefix,
                            method,
                            type_args,
                            args,
                            *span,
                            ctx,
                            expected,
                        );
                    }
                }
            }
            // A cross-module type path (`shapes.Shape.Circle(...)`) is gated
            // before it is read as a variant construction.
            if let Some(path) = expr_path(receiver) {
                ctx.check_type_path_exported(&path, *span)?;
            }
            // `Option.Some(v = 1)` - construction of a generic `enum`
            // variant (A-2), before the concrete-enum check because a generic
            // enum has no entry in the enum table.
            if let Some(tmpl_name) = generic_type_path(receiver, ctx) {
                let explicit = resolve_explicit_args(type_args, ctx)?;
                return check_generic_enum_construction(
                    &tmpl_name,
                    method,
                    &explicit,
                    args,
                    *span,
                    ctx,
                    expected,
                );
            }
            // `Shape.Circle(r = 1.0)` - construction of an `enum` variant
            // (A-1). Checked before method resolution because it is the same
            // `Type.member(...)` shape as a method call, and the receiver is
            // a *type*, not a value. A member that is not a variant falls
            // through to a static trait method (A-12 slice 6).
            if let Some(ename) = enum_path(receiver, ctx) {
                let is_variant = method == "from_int"
                    || ctx
                        .enum_def(&ename)
                        .map(|e| e.variant(method).is_some())
                        .unwrap_or(false);
                if !is_variant {
                    if let Some(e) =
                        check_static_method_call(&ename, method, args, *span, ctx, expected)?
                    {
                        return Ok(e);
                    }
                }
                return check_enum_construction(&ename, method, args, *span, ctx);
            }
            if !type_args.is_empty() && !ptr_receiver(receiver, ctx) {
                return Err(TypeError {
                    message: format!(
                        "'{method}' does not take type arguments - a method's type parameters come from its receiver"
                    ),
                    span: *span,
                });
            }
            check_method_call_enum_guard(receiver, method, *span, ctx)?;
            check_method_call(receiver, method, type_args, args, *span, ctx, expected)
        }
        ast::Expr::When {
            scrutinee,
            arms,
            span,
        } => check_when_scrutinee(scrutinee, arms, *span, ctx, expected),
        // `[a, b, c]`: an array literal (A-18). Every element takes the
        // array's element type, which is either the one the context named or,
        // when nothing constrains it, the first element's own type.
        ast::Expr::ArrayLit { elems, span } => {
            let hinted = match expected {
                Some(Ty::Array(elem, len)) => {
                    if *len as usize != elems.len() {
                        return Err(TypeError {
                            message: format!(
                                "an array of {len} element(s) is expected here, and this literal has {}",
                                elems.len()
                            ),
                            span: *span,
                        });
                    }
                    Some((**elem).clone())
                }
                _ => None,
            };
            let mut checked: Vec<tir::Expr> = Vec::with_capacity(elems.len());
            let mut elem_ty = hinted;
            for (i, e) in elems.iter().enumerate() {
                // The first element's own type is what the rest are checked
                // against, which is the same rule `2` in `1 + 2` follows: a
                // literal takes the type of the whole it is part of, and the
                // first element is what defines that whole.
                let v = check_expr(e, ctx, elem_ty.as_ref())?;
                match &elem_ty {
                    None => elem_ty = Some(v.ty.clone()),
                    Some(t) if *t != v.ty => {
                        return Err(TypeError {
                            message: format!(
                                "the elements of an array have different types: element {i} is a '{}', and the first element is a '{}'",
                                v.ty.display(),
                                t.display()
                            ),
                            span: e.span(),
                        })
                    }
                    _ => {}
                }
                checked.push(v);
            }
            let elem_ty = elem_ty.expect("an array literal has at least one element");
            if !crate::types::is_array_element(&elem_ty) {
                return Err(TypeError {
                    message: array_element_error(&elem_ty),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::ArrayLit(checked),
                ty: Ty::Array(Box::new(elem_ty), elems.len() as u64),
                span: *span,
            })
        }
        // `(lo, hi)`: a tuple value (MISSING-z2bB9, `plan/syntax.md` 3). Each
        // element takes its own type, from the expected tuple's position when
        // the context named one and from the element expression otherwise. The
        // value is a `StructLit` of the derived record, whose fields are the
        // positions, so codegen needs no tuple case and the elements are
        // moved into the value exactly as a `datac` construction moves its
        // arguments in.
        ast::Expr::TupleLit { elems, span } => {
            let hinted = match expected {
                Some(t) => tuple_elems_of(t, ctx),
                None => None,
            };
            if let Some(expected_elems) = &hinted {
                if expected_elems.len() != elems.len() {
                    return Err(TypeError {
                        message: format!(
                            "a tuple of {} element(s) is expected here, and this one has {}",
                            expected_elems.len(),
                            elems.len()
                        ),
                        span: *span,
                    });
                }
            }
            let mut checked: Vec<tir::Expr> = Vec::with_capacity(elems.len());
            let mut elem_tys: Vec<Ty> = Vec::with_capacity(elems.len());
            for (i, e) in elems.iter().enumerate() {
                let hint = hinted.as_ref().map(|h| &h[i]);
                let v = check_expr(e, ctx, hint)?;
                if crate::types::is_borrowed_field_ty(&v.ty) {
                    return Err(TypeError {
                        message: format!(
                            "element {} of the tuple is a borrowed type ('{}'): a tuple holds owned data only in v1, exactly as a datac field does, so a '&str', 'View' or reference element is not expressible; an owned 'String' is the way to keep text in one",
                            i + 1,
                            v.ty.display()
                        ),
                        span: e.span(),
                    });
                }
                if v.ty == Ty::Unit {
                    return Err(TypeError {
                        message: format!(
                            "element {} of the tuple produces no value ('unit'): 'unit' is the absent value and cannot be stored, exactly as a datac field cannot be",
                            i + 1
                        ),
                        span: e.span(),
                    });
                }
                elem_tys.push(v.ty.clone());
                checked.push(v);
            }
            let ty = tuple_ty(&ctx.env, elem_tys);
            let name = match &ty {
                Ty::Struct(n) => n.clone(),
                _ => unreachable!("tuple_ty builds a record"),
            };
            let fields = checked
                .into_iter()
                .enumerate()
                .map(|(i, v)| (crate::types::tuple_field_name(i), v))
                .collect();
            Ok(tir::Expr {
                kind: tir::ExprKind::StructLit { name, fields, borrowed_fields: Vec::new() },
                ty,
                span: *span,
            })
        }
        // `[v; N]`: the repeat form (A-18). `N` is folded here exactly as an
        // array type's length is (A-19), and the element has to be `Copy`,
        // because the alternative is cloning an owned value once per element.
        ast::Expr::ArrayRepeat { value, len, span } => {
            let n = ctx.type_scope().array_len(len)?;
            let hinted = match expected {
                Some(Ty::Array(elem, expected_len)) => {
                    if *expected_len != n {
                        return Err(TypeError {
                            message: format!(
                                "an array of {expected_len} element(s) is expected here, and this repeat produces {n}"
                            ),
                            span: *span,
                        });
                    }
                    Some((**elem).clone())
                }
                _ => None,
            };
            let v = check_expr(value, ctx, hinted.as_ref())?;
            let elem_ty = v.ty.clone();
            // The repeat form *copies* its value into every slot, so the
            // element has to own nothing: a scalar, or a `datac` whose fields
            // are all scalars. An element that owns memory would need N
            // clones, and a copy of an owner is a second owner. A tuple is a
            // derived record, so `needs_drop` cannot answer for one from the
            // declared set and its positions are walked instead.
            if elem_owns_memory(ctx, &elem_ty) {
                return Err(TypeError {
                    message: format!(
                        "the repeat form '[x; N]' needs an element that owns nothing, and '{}' owns memory. Copying it into {n} slots would make {n} owners of the same buffer; write the elements out ('[a, b, c]') instead",
                        elem_ty.display()
                    ),
                    span: *span,
                });
            }
            if !crate::types::is_array_element(&elem_ty) {
                return Err(TypeError {
                    message: array_element_error(&elem_ty),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::ArrayRepeat { value: Box::new(v) },
                ty: Ty::Array(Box::new(elem_ty), n),
                span: *span,
            })
        }
    }
}

/// A pattern `when` whose scrutinee is a temporary stages it into a local of
/// its own and matches the rebased place: `when mk().t { ... }` becomes
/// `{ val #when0 = mk()  when #when0.t { ... } }`. The arms read their payloads
/// through a borrow, so the scrutinee has to be a place that outlives them, and
/// binding the *root* is what frees the whole temporary once, at the end of the
/// block and after the last arm, rather than leaking every field the match
/// did not read (`heap_field_of_owned_temporary.kp`).
fn check_when_scrutinee(
    scrutinee: &ast::Expr,
    arms: &[ast::WhenArm],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let Some(root) = ast::temporary_root(scrutinee).cloned() else {
        return check_when(scrutinee, arms, span, ctx, expected);
    };
    ctx.when_temps += 1;
    let tmp = format!("{}{}", WHEN_TEMP_PREFIX, ctx.when_temps - 1);
    let init = check_expr(&root, ctx, None)?;
    let bind_ty = init.ty.clone();
    let rebased = ast::rebase_root(scrutinee, &ast::Expr::Ident(tmp.clone(), span));
    // The binding is visible to the arms and to nothing outside them: a
    // `#when0` cannot collide with a written name, and popping the scope is
    // what keeps a nested `when` from resolving its own temporary to this one.
    ctx.scopes.push(HashMap::new());
    ctx.define(&tmp, bind_ty.clone(), false);
    let inner = check_when(&rebased, arms, span, ctx, expected);
    ctx.scopes.pop();
    let inner = inner?;
    let ty = inner.ty.clone();
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: tmp,
                ty: bind_ty,
                init,
            }],
            tail: Some(Box::new(inner)),
        }),
        ty,
        span,
    })
}

/// The dotted path an expression names, if it is one: `Shape`, `shapes.Shape`,
/// `sh.Shape`. Used to recognize the enum half of `Enum.Variant(...)` before
/// the receiver is checked as a value (A-1).
fn expr_path(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Ident(name, _) => Some(name.clone()),
        ast::Expr::FieldAccess { base, field, .. } => {
            Some(format!("{}.{}", expr_path(base)?, field))
        }
        _ => None,
    }
}

/// Whether `expr` names an `enum` (A-1), resolving the same way a type name
/// does: a bare name belongs to this module, and an alias-qualified one to the
/// module it was imported from.
fn enum_path(expr: &ast::Expr, ctx: &Ctx) -> Option<String> {
    // A local, parameter or passdown of the same name shadows the type, the
    // same way it shadows a module alias.
    if let ast::Expr::Ident(name, _) = expr {
        if ctx.lookup(name).is_some() || ctx.pds.contains_key(name) {
            return None;
        }
    }
    let path = expr_path(expr)?;
    if ctx.is_enum(&path) {
        return Some(path);
    }
    let qualified = match path.split_once('.') {
        Some((alias, item)) => format!("{}{}", ctx.aliases.get(alias)?, item),
        None => ctx.qualify_own(&path),
    };
    ctx.is_enum(&qualified).then_some(qualified)
}

/// The generic-template counterpart of `enum_path` (A-2): the qualified name of
/// the template `expr` names, when it names one. `Option.Some(v = 1)` and
/// `Option.None` both arrive here, because a generic enum has no concrete
/// entry in the enum table for `enum_path` to find.
fn generic_type_path(expr: &ast::Expr, ctx: &Ctx) -> Option<String> {
    if let ast::Expr::Ident(name, _) = expr {
        if ctx.lookup(name).is_some() || ctx.pds.contains_key(name) {
            return None;
        }
    }
    let path = expr_path(expr)?;
    if ctx.env.generics.contains_key(&path) {
        return Some(path);
    }
    let qualified = match path.split_once('.') {
        Some((alias, item)) => format!("{}{}", ctx.aliases.get(alias)?, item),
        None => ctx.qualify_own(&path),
    };
    ctx.env.generics.contains_key(&qualified).then_some(qualified)
}

/// The qualified name of the generic template `name` names, if it names one
/// (A-2).
fn generic_template_name(name: &str, ctx: &Ctx) -> Option<String> {
    if ctx.env.generics.contains_key(name) {
        return Some(name.to_string());
    }
    let qualified = ctx.qualify_own(name);
    ctx.env
        .generics
        .contains_key(&qualified)
        .then_some(qualified)
}

/// Pairs every parameter with the index of the argument that fills it,
/// honouring named arguments exactly as `match_args` does. Factored out so
/// type-argument inference (A-2) can see the pairing before the parameter
/// types are known - `match_args` needs the types, and the types need the
/// pairing.
fn arg_slots(what: &str, names: &[String], args: &[ast::CallArg], span: Span) -> Result<Vec<usize>> {
    if args.len() != names.len() {
        return Err(TypeError {
            message: format!("{what} expects {} argument(s), found {}", names.len(), args.len()),
            span,
        });
    }
    let mut named: HashMap<&str, usize> = HashMap::new();
    for (i, a) in args.iter().enumerate() {
        if let Some(n) = &a.name {
            if named.insert(n.as_str(), i).is_some() {
                return Err(TypeError {
                    message: format!("argument '{n}' specified more than once"),
                    span,
                });
            }
        }
    }
    let mut positional = args.iter().enumerate().filter(|(_, a)| a.name.is_none());
    let mut slots = Vec::with_capacity(names.len());
    for name in names {
        let idx = match named.remove(name.as_str()) {
            Some(i) => i,
            None => positional
                .next()
                .map(|(i, _)| i)
                .ok_or_else(|| TypeError {
                    message: format!("{what} is missing argument '{name}'"),
                    span,
                })?,
        };
        slots.push(idx);
    }
    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }
    Ok(slots)
}

/// Resolves an expression's explicit type arguments (`Pair<i32, String>(...)`,
/// `Option.Some<i32>(...)`), which are written types exactly like a
/// signature's (A-2).
fn resolve_explicit_args(type_args: &[ast::TypeExpr], ctx: &Ctx) -> Result<Vec<Ty>> {
    if type_args.is_empty() {
        return Ok(Vec::new());
    }
    let scope = ctx.type_scope();
    let mut out = Vec::with_capacity(type_args.len());
    for t in type_args {
        // A const argument (A-20): a literal or an arithmetic expression the
        // parser already recognized, folded with no declared type here and
        // re-typed by `bind_const_args` once the template is known.
        if let Some(expr) = &t.const_expr {
            out.push(scope.fold_const_arg_any(expr)?);
            continue;
        }
        match scope.resolve(t, true) {
            Ok(ty) => out.push(ty),
            // A bare or dotted name that is not a type may be a named `const`
            // (`sum<SIZE>`): fold it, and let the template's parameter list
            // decide which one it is. A name that is neither keeps its type
            // error.
            Err(type_err) => match named_const_expr(t) {
                Some(expr) if scope.fold_const_arg_any(&expr).is_ok() => {
                    out.push(scope.fold_const_arg_any(&expr)?)
                }
                _ => return Err(type_err),
            },
        }
    }
    Ok(out)
}

/// Confirms the argument count of an explicit instantiation (`Pair<i32, String>`,
/// A-2) and returns it as the type arguments.
fn check_arity(name: &str, params: &[String], explicit: &[Ty], span: Span) -> Result<Vec<Ty>> {
    if explicit.len() != params.len() {
        return Err(TypeError {
            message: format!(
                "'{name}' takes {} type argument(s), found {}",
                params.len(),
                explicit.len()
            ),
            span,
        });
    }
    Ok(explicit.to_vec())
}

/// Binds a template's type parameters from one written field type and the
/// type the corresponding argument actually has (A-2). Only the shapes that
/// can carry a parameter are inspected: a bare parameter name, or a parameter
/// under a `List`/`Box`/`Map`. A field type that mentions no parameter is
/// ignored here - its argument is checked against it later, by `match_args`,
/// which is also where a mismatch is reported.
/// A written type name as its canonical key, resolved against the module the
/// *declaration* was written in rather than the module the call site is in -
/// `Result<T, E>` inside `std/result.kp` is `std.result.Result<T, E>` no matter
/// who calls it.
fn qualify_written(name: &str, prefix: &str, aliases: &HashMap<String, String>) -> String {
    match name.split_once('.') {
        Some((alias, item)) => match aliases.get(alias) {
            Some(p) => format!("{p}{item}"),
            None => name.to_string(),
        },
        None => format!("{prefix}{name}"),
    }
}

fn bind_type_params(
    env: &TypeEnv,
    prefix: &str,
    aliases: &HashMap<String, String>,
    written: &ast::TypeExpr,
    actual: &Ty,
    params: &[String],
    out: &mut HashMap<String, Ty>,
) {
    // A parameter that is borrowed at the call site (`fun is_some<T>(o: &Option<T>)`
    // called with an `Option<i32>`) still determines the parameter, and a
    // parameter that is *already* a borrow determines it from its pointee.
    let actual = match actual {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    // A function type (A-4) pairs a written type against a function value
    // structurally: the parameters in order, then the return. This is what
    // lets `map<T, U>(o: &Option<T>, f: fun(&T).return: U)` take both `T` and `U`
    // from its arguments, and it is the only place a function type is bound.
    // A closure type (A-4 phase E) pairs the same way, which is what lets a
    // callback written as `f: closure(&T).return: U` infer `U` from a literal.
    if let Some(written_params) = &written.fn_params {
        let implied = match actual {
            Ty::Fn { params, ret }
            | Ty::Closure { params, ret }
            | Ty::ScopedFn { params, ret } => Some((params, ret)),
            _ => None,
        };
        if let Some((actual_params, actual_ret)) = implied {
            if written_params.len() == actual_params.len() {
                for (w, a) in written_params.iter().zip(actual_params.iter()) {
                    bind_type_params(env, prefix, aliases, &w.ty, a, params, out);
                }
                if let Some(written_ret) = &written.fn_ret {
                    bind_type_params(env, prefix, aliases, written_ret, actual_ret, params, out);
                }
            }
        }
        return;
    }
    // A projection (`I::Item`, A-12 slice 5) determines no type parameter: the
    // concrete type is the impl's, so nothing about `I` can be read off an
    // argument of it. An explicit type argument (or another parameter) is what
    // must determine `I`.
    if written.assoc.is_some() {
        return;
    }
    // A fixed-size array (A-18) binds its element parameters and, when the
    // length is a bare parameter name, the const parameter the length stands
    // for (A-20): `sum(&arr)` infers `N` from `arr`'s own length.
    if let Some(written_elem) = &written.array_elem {
        if let Ty::Array(actual_elem, len) = actual {
            bind_type_params(env, prefix, aliases, written_elem, actual_elem, params, out);
            if let Some(len_expr) = written.array_len.as_deref() {
                if let ast::Expr::Ident(name, _) = len_expr {
                    if params.iter().any(|p| p == name) {
                        out.entry(name.clone()).or_insert(Ty::ConstInt {
                            value: *len as i128,
                            ty: Box::new(Ty::Usize),
                        });
                    }
                }
            }
        }
        return;
    }
    if written.args.is_empty() {
        if params.iter().any(|p| p == &written.name) {
            out.entry(written.name.clone())
                .or_insert_with(|| actual.clone());
        }
        return;
    }
    match (written.name.as_str(), actual) {
        ("List", Ty::List(e)) | ("Box", Ty::Box(e)) => {
            bind_type_params(env, prefix, aliases, &written.args[0], e, params, out)
        }
        // `Map<V>` writes the value alone (the key is `String`), so only a
        // two-argument `Map<K, V>` has a key to bind.
        ("Map", Ty::Map(k, v)) => {
            if written.args.len() == 2 {
                bind_type_params(env, prefix, aliases, &written.args[0], k, params, out);
            }
            bind_type_params(
                env,
                prefix,
                aliases,
                &written.args[written.args.len() - 1],
                v,
                params,
                out,
            );
        }
        // A user generic type (`Pair<T, T>` against `Pair<i32, i32>`): the
        // instantiation records the template it came from and the arguments it
        // was built with, which is exactly the pairing wanted here.
        _ => {
            let mine = qualify_written(&written.name, prefix, aliases);
            if let Ty::Struct(key) = actual {
                let origin = env.instances.borrow().origin.get(key).cloned();
                if let Some((tname, targs)) = origin {
                    if tname == mine {
                        for (w, a) in written.args.iter().zip(targs.iter()) {
                            bind_type_params(env, prefix, aliases, w, a, params, out);
                        }
                    }
                }
            }
        }
    }
}

/// Completes a partial set of type-argument bindings from the expected type:
/// `val x: Option<i32> = Option.None` has nothing else to go on (A-2).
fn bind_from_expected(
    tmpl: &GenericTemplate,
    expected: Option<&Ty>,
    ctx: &Ctx,
    out: &mut HashMap<String, Ty>,
) {
    let Some(Ty::Struct(key)) = expected else {
        return;
    };
    let st = ctx.env.instances.borrow();
    let Some((name, args)) = st.origin.get(key) else {
        return;
    };
    if name != &tmpl.name {
        return;
    }
    for (p, a) in tmpl.params.iter().zip(args.iter()) {
        out.entry(p.clone()).or_insert_with(|| a.clone());
    }
}

/// The complete argument list for one instantiation, or an error naming the
/// parameter that could not be inferred.
fn finish_type_args(
    name: &str,
    params: &[String],
    bindings: &HashMap<String, Ty>,
    span: Span,
) -> Result<Vec<Ty>> {
    let mut args = Vec::with_capacity(params.len());
    for p in params {
        match bindings.get(p) {
            Some(t) => args.push(t.clone()),
            None => {
                return Err(TypeError {
                    message: format!(
                        "cannot infer type parameter '{p}' of '{name}' here - annotate the binding (e.g. 'val x: {name}<{}> = ...') or pass an argument that determines it",
                        params.join(", ")
                    ),
                    span,
                })
            }
        }
    }
    Ok(args)
}

/// The type a constructor argument is checked *against*, as far as the
/// bindings so far allow it (A-2). `Node(value = 1, next = Box.empty())`
/// needs the second argument to know it is a `Box<Node<i32>>`, and the only
/// place that says so is the field's written type - which can be resolved
/// once an earlier argument has pinned the parameters. `None` means "not
/// enough is known yet", and the argument is probed unconstrained.
fn field_hint(
    ctx: &Ctx,
    prefix: &str,
    aliases: &HashMap<String, String>,
    written: &ast::TypeExpr,
    bindings: &HashMap<String, Ty>,
) -> Option<Ty> {
    let scope = TypeScope {
        structs: &ctx.structs,
        enums: &ctx.enums,
        env: ctx.env.clone(),
        params: bindings,
        prefix,
        aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(&ctx.consts),
        const_fns: Some(&ctx.const_fns),
        handles: &ctx.env.handles,
    };
    // A generic declaration's `scoped fun(...)` parameter is still a top-level
    // parameter of a declaration, so the scoped type is legal here; the hint
    // is what lets a trailing block header take its parameter and return types
    // from it. `resolve` refuses the modifier everywhere because everywhere
    // else the value could be stored, so the hint goes through the same
    // parameter path a non-generic declaration's signature uses.
    if written.scoped {
        return scope.resolve_scoped_param(written, ast::ParamMode::Borrow).ok();
    }
    scope.resolve(written, false).ok()
}

/// `Pair(first = 1, second = "x")` - construction of a generic `datac` (A-2).
/// The type arguments are inferred from the argument types, falling back to
/// the expected type, so a generic value is built with the same syntax as an
/// ordinary one.
fn check_generic_datac_construction(
    tmpl_name: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Record(fields) = &tmpl.kind else {
        let variants: Vec<String> = match &tmpl.kind {
            GenericKind::Sum(vs) => vs.iter().map(|v| format!("'{}.{}'", tmpl.name, v.name)).collect(),
            GenericKind::Record(_) => unreachable!(),
        };
        return Err(TypeError {
            message: format!(
                "'{}' is an enum - construct one of its variants: {}",
                tmpl.name,
                variants.join(", ")
            ),
            span,
        });
    };
    let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    let slots = arg_slots(&format!("datac '{}'", tmpl.name), &names, args, span)?;
    // With an explicit instantiation there is nothing to infer; otherwise the
    // expected type is consulted first, because it is the only thing that can
    // pin a parameter *before* an argument is probed - which is what makes
    // `xs.push(Option.Some(value = 7))` infer `Option<i64>` from the list's
    // element type rather than defaulting the literal to `i32`.
    let type_args = if explicit.is_empty() {
        let mut bindings = HashMap::new();
        bind_from_expected(&tmpl, expected, ctx, &mut bindings);
        for (i, f) in fields.iter().enumerate() {
            let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &f.ty, &bindings);
            let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &f.ty, &probed.ty, &tmpl.params, &mut bindings);
            ctx.keep_probe(&args[slots[i]].value, probed);
        }
        finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.name, &tmpl.params, explicit, span)?
    };
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let sdef = ctx.struct_def(&key).expect("just instantiated");
    let params: Vec<(String, Ty)> = sdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let checked = match_args(&format!("datac '{}'", key), &params, args, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::StructLit {
            name: key,
            fields: sdef
                .fields
                .iter()
                .map(|f| f.name.clone())
                .zip(checked)
                .collect(),
            borrowed_fields: Vec::new(),
        },
        ty: Ty::Struct(sdef.name.clone()),
        span,
    })
}

/// `Option.Some(v = 1)` - construction of one variant of a generic `enum`
/// (A-2), the enum counterpart of `check_generic_datac_construction`.
fn check_generic_enum_construction(
    tmpl_name: &str,
    variant: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Sum(variants) = &tmpl.kind else {
        return Err(TypeError {
            message: format!(
                "'{}' is a generic datac - construct it as '{}(...)'",
                tmpl.name, tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'{}.{}' is not a variant - the variants of '{}' are {}",
                tmpl.name,
                variant,
                tmpl.name,
                names.join(", ")
            ),
            span,
        });
    };
    let names: Vec<String> = vdef.fields.iter().map(|f| f.name.clone()).collect();
    let slots = arg_slots(
        &format!("variant '{}.{}'", tmpl.name, variant),
        &names,
        args,
        span,
    )?;
    let type_args = if explicit.is_empty() {
        let mut bindings = HashMap::new();
        bind_from_expected(&tmpl, expected, ctx, &mut bindings);
        for (i, f) in vdef.fields.iter().enumerate() {
            let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &f.ty, &bindings);
            let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &f.ty, &probed.ty, &tmpl.params, &mut bindings);
            ctx.keep_probe(&args[slots[i]].value, probed);
        }
        finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.name, &tmpl.params, explicit, span)?
    };
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&key).expect("just instantiated");
    let Some((vidx, vdef)) = edef.variant(variant) else {
        unreachable!("the instance keeps the template's variants")
    };
    let params: Vec<(String, Ty)> = vdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let checked = match_args(
        &format!("variant '{}.{}'", key, variant),
        &params,
        args,
        span,
        ctx,
    )?;
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: key.clone(),
            variant: vidx,
            fields: checked,
        },
        ty: Ty::Struct(key),
        span,
    })
}

/// `Option.None` - a payload-less variant of a generic `enum`, written without
/// a call (A-2). Nothing in the expression determines the type arguments, so
/// this form only checks when the expected type does.
fn check_generic_bare_variant(
    tmpl_name: &str,
    variant: &str,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Sum(variants) = &tmpl.kind else {
        return Err(TypeError {
            message: format!(
                "'{}' is a generic datac - read its fields with '.' after constructing it",
                tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'{}.{}' is not a variant - the variants of '{}' are {}",
                tmpl.name,
                variant,
                tmpl.name,
                names.join(", ")
            ),
            span,
        });
    };
    if !vdef.fields.is_empty() {
        let fields: Vec<String> = vdef.fields.iter().map(|f| f.name.clone()).collect();
        return Err(TypeError {
            message: format!(
                "variant '{}.{}' carries payload ({}) - construct it as '{}.{}({})'",
                tmpl.name,
                variant,
                fields.join(", "),
                tmpl.name,
                variant,
                fields.join(" = ..., ")
            ),
            span,
        });
    }
    let mut bindings = HashMap::new();
    bind_from_expected(&tmpl, expected, ctx, &mut bindings);
    let type_args = finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?;
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&key).expect("just instantiated");
    let Some((vidx, _)) = edef.variant(variant) else {
        unreachable!("the instance keeps the template's variants")
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: key.clone(),
            variant: vidx,
            fields: Vec::new(),
        },
        ty: Ty::Struct(key),
        span,
    })
}

/// The two-variant shapes `?` understands (MK5-S6-M1): `Ok`/`Err`, where the
/// failure carries a payload, and `Some`/`None`, where it does not. Recognized
/// by *variant name and arity*, never by the enum's own name, so a program's
/// own `Result`/`Option` works with no import and no blessed name - the shape
/// is the contract.
struct TryShape {
    ok: usize,
    err: usize,
    err_has_payload: bool,
}

fn try_shape(edef: &EnumDef) -> Option<TryShape> {
    if edef.variants.len() != 2 {
        return None;
    }
    let find = |n: &str| edef.variants.iter().position(|v| v.name == n);
    if let (Some(ok), Some(err)) = (find("Ok"), find("Err")) {
        if edef.variants[ok].fields.len() == 1 && edef.variants[err].fields.len() == 1 {
            return Some(TryShape {
                ok,
                err,
                err_has_payload: true,
            });
        }
        return None;
    }
    if let (Some(some), Some(none)) = (find("Some"), find("None")) {
        if edef.variants[some].fields.len() == 1 && edef.variants[none].fields.is_empty() {
            return Some(TryShape {
                ok: some,
                err: none,
                err_has_payload: false,
            });
        }
    }
    None
}

/// `expr?` (MK5-S6-M1). The operand has to be an `enum` shaped `Ok`/`Err` or
/// `Some`/`None`, held by value (a borrow cannot give its payload away), and
/// the enclosing function has to return an enum of the same shape - with the
/// *same* failure payload type for `Ok`/`Err`, because v1 has no error
/// conversion. What the operator evaluates to is the success payload.
fn check_try(sub: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    let inner = check_expr(sub, ctx, None)?;
    // The operand is read as a whole value and its payload is moved out, so a
    // borrow is not usable here - `when`/`if let` is the form for that.
    let source = match &inner.ty {
        Ty::Struct(name) => name.clone(),
        other => {
            return Err(TypeError {
                message: format!(
                    "'?' needs a value shaped 'Ok'/'Err' or 'Some'/'None' held by value, not '{}' - match it with 'when' or 'if let' instead",
                    other.display()
                ),
                span,
            })
        }
    };
    let Some(edef) = ctx.enum_def(&source) else {
        return Err(TypeError {
            message: format!("'?' needs an enum, but '{}' is a datac", inner.ty.display()),
            span,
        });
    };
    let Some(shape) = try_shape(&edef) else {
        let names: Vec<String> = edef.variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'?' needs an enum with an 'Ok' and an 'Err' variant (or a 'Some' and a 'None'), but '{}' has {}",
                inner.ty.display(),
                names.join(", ")
            ),
            span,
        });
    };
    // The enclosing function's return type is where a failure goes.
    let ret = ctx.ret_ty.clone();
    let ret_name = match &ret {
        Ty::Struct(name) if ctx.enum_def(name).is_some() => name.clone(),
        other => {
            return Err(TypeError {
                message: format!(
                    "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' - it returns '{}'",
                    other.display()
                ),
                span,
            })
        }
    };
    let rdef = ctx
        .enum_def(&ret_name)
        .expect("checked just above that the return type is an enum");
    let Some(rshape) = try_shape(&rdef) else {
        let names: Vec<String> = rdef.variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' - it returns '{}', with {}",
                ret.display(),
                names.join(", ")
            ),
            span,
        });
    };
    if shape.err_has_payload != rshape.err_has_payload {
        return Err(TypeError {
            message: format!(
                "'?' here propagates {} failure, but the enclosing function returns '{}' - the two have to be the same shape; match the value instead (`when r {{ Ok(v) -> ..., Err(e) -> return <your failure> }}`), or make the function return the same kind of enum",
                if shape.err_has_payload { "a carrying" } else { "a payload-less" },
                ret.display()
            ),
            span,
        });
    }
    // A-12 slice 6: when the two failure payload types differ, `?` converts
    // through a declared `impl From<Source> for Target`, calling
    // `Target.from(source)`. When they agree, the payload moves across with no
    // call, exactly as before.
    let mut convert: Option<(String, bool)> = None;
    if shape.err_has_payload {
        let from = edef.variants[shape.err].fields[0].ty.clone();
        let to = rdef.variants[rshape.err].fields[0].ty.clone();
        if from != to {
            match find_from_conversion(ctx, &from, &to)? {
                Some(key) => convert = Some(key),
                None => {
                    return Err(TypeError {
                        message: format!(
                            "'?' cannot convert a failure: '{}' carries '{}' and the enclosing function returns '{}', whose failure carries '{}' - write 'impl From<{}> for {} {{ fun from(value: {}).return: {} {{ ... }} }}' so the operator has a conversion to call",
                            inner.ty.display(),
                            from.display(),
                            ret.display(),
                            to.display(),
                            from.display(),
                            to.display(),
                            from.display(),
                            to.display()
                        ),
                        span,
                    })
                }
            }
        }
    }
    let ok_ty = edef.variants[shape.ok].fields[0].ty.clone();
    Ok(tir::Expr {
        kind: tir::ExprKind::Try {
            expr: Box::new(inner),
            source,
            target: ret_name,
            ok_variant: shape.ok,
            err_variant: shape.err,
            target_err_variant: rshape.err,
            err_has_payload: shape.err_has_payload,
            convert,
        },
        ty: ok_ty,
        span,
    })
}

/// The static `From` method key that converts one failure payload into another
/// (A-12 slice 6), or `None` when no `impl From<from> for to` exists. The
/// implementing type is the *target* payload type itself, because that is what
/// the `for` clause names (`impl From<IoErr> for AppErr`).
fn find_from_conversion(ctx: &Ctx, from_ty: &Ty, to_ty: &Ty) -> Result<Option<(String, bool)>> {
    let Some(to_name) = struct_name_of(to_ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(to_ty).map(|s| s.to_string()))
    else {
        return Ok(None);
    };
    for imp in ctx.env.trait_impls.iter() {
        if imp.type_name != to_name {
            continue;
        }
        let Some(info) = ctx.env.traits.get(&imp.trait_name) else {
            continue;
        };
        // The conversion is the declared `From` trait's `from`, whatever
        // module ships it; the operator names `From` in its refusal for the
        // same reason.
        if !(info.name == "From" || info.name.ends_with(".From")) {
            continue;
        }
        if info.type_params.len() != 1 {
            continue;
        }
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            if m.receiver.is_some() || m.name != "from" || m.params.len() != 1 {
                continue;
            }
            let sig = resolve_trait_method_sig(
                info,
                m,
                to_ty,
                &trait_args,
                &ctx.structs,
                &ctx.enums,
                &ctx.env,
                &ctx.consts,
                &ctx.const_fns,
            )?;
            if sig.params.len() != 1 || sig.ret != *to_ty {
                continue;
            }
            // A `datac` parameter is a shared borrow of the payload; a `Copy`
            // scalar is taken by value. The caller passes whichever the
            // method declares.
            let by_ref = matches!(&sig.params[0].1, Ty::Ref { inner, .. } if **inner == *from_ty);
            if sig.params[0].1 == *from_ty || by_ref {
                return Ok(Some((impl_method_key(imp, m), by_ref)));
            }
        }
    }
    Ok(None)
}

/// `Shape.Circle(r = 1.0)` - one variant's worth of payload, checked against
/// the payload fields exactly as a `datac` construction is checked against its
/// fields (A-1).
fn check_enum_construction(
    ename: &str,
    variant: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let edef = ctx
        .enums
        .get(ename)
        .cloned()
        .expect("enum_path only returns known enums");
    // `Mode.from_int(n)` shares the `Type.member(...)` spelling with a variant
    // construction but is the enum's own associated conversion (kernel stage
    // 2), so it is resolved before the variant lookup refuses it as one. A
    // variant actually named `from_int` still wins.
    if variant == "from_int" && edef.variant("from_int").is_none() {
        return lower_enum_from_int(ename, args, span, ctx);
    }
    let Some((vidx, vdef)) = edef.variant(variant) else {
        let names: Vec<String> = edef
            .variants
            .iter()
            .map(|v| format!("'{}'", v.name))
            .collect();
        return Err(TypeError {
            message: format!(
                "'{ename}.{variant}' is not a variant - the variants of '{ename}' are {}",
                names.join(", ")
            ),
            span,
        });
    };
    let params: Vec<(String, Ty)> = vdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let fields = match_args(
        &format!("variant '{}.{}'", ename, variant),
        &params,
        args,
        span,
        ctx,
    )?;
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: ename.to_string(),
            variant: vidx,
            fields,
        },
        ty: Ty::Struct(ename.to_string()),
        span,
    })
}

/// `when s { Circle(r) -> ... else -> ... }` (A-1) - a pattern match over an
/// `enum`, lowered here to a chain of `if <tag test> { <bindings> <body> }
/// else ...`: the same TIR shape the boolean `when` already lowers to in the
/// parser, plus two node kinds (`EnumTag`, `EnumField`) that codegen emits as
/// `.tag ==` and `.v.<Variant>.<field>`. No new statement machinery and no new
/// drop plan are needed as a result.
///
/// `scrutinee` is always a place by the time this runs: `check_when_scrutinee`
/// stages a temporary into a local of its own first. That is what makes the
/// payload borrows below outlive their arms, and what makes an owned payload of
/// a temporary scrutinee bind by value out of that staged local.
fn check_when(
    scrutinee: &ast::Expr,
    arms: &[ast::WhenArm],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let scrut = check_expr(scrutinee, ctx, None)?;
    let Some(ename) = struct_name_of(&scrut.ty).map(|s| s.to_string()) else {
        return Err(TypeError {
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{}' is not one - for a chain of conditions write 'when {{ cond -> value }}' with no scrutinee",
                scrut.ty.display()
            ),
            span: scrut.span,
        });
    };
    let Some(edef) = ctx.enum_def(&ename) else {
        return Err(TypeError {
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{ename}' is a datac - read its fields with '.' instead"
            ),
            span: scrut.span,
        });
    };

    // An owned payload binds by value only when the scrutinee is a place this
    // frame owns outright: a `val`/`var` local or a `move` parameter, which is
    // the base `borrowck.rs` plans a partial move from. A borrow scrutinee, a
    // field path and a dereference all bind it as a shared borrow instead, so
    // the value stays where it is; the staged `#when0` a temporary scrutinee
    // becomes is an owned local like any other, which is what lets the payload
    // of `when f() { ... }` be handed out.
    let owns_scrutinee =
        matches!(scrut.kind, tir::ExprKind::Ident(_)) && !matches!(scrut.ty, Ty::Ref { .. });
    let mut covered: Vec<Option<usize>> = vec![None; edef.variants.len()];
    let mut else_seen = false;
    let mut checked: Vec<(Option<usize>, tir::Block)> = Vec::new();
    let mut arm_ty: Option<Ty> = None;

    for (ai, arm) in arms.iter().enumerate() {
        if else_seen {
            return Err(TypeError {
                message: "an 'else' arm must be the last arm of a 'when'".to_string(),
                span: arm.span,
            });
        }
        let variant = match &arm.pattern {
            None => {
                else_seen = true;
                None
            }
            Some(p) => {
                let Some((vidx, vdef)) = edef.variant(&p.variant) else {
                    let names: Vec<String> = edef
                        .variants
                        .iter()
                        .map(|v| format!("'{}'", v.name))
                        .collect();
                    return Err(TypeError {
                        message: format!(
                            "'{}' is not a variant of enum '{ename}' - the variants are {}",
                            p.variant,
                            names.join(", ")
                        ),
                        span: p.span,
                    });
                };
                if covered[vidx].is_some() {
                    return Err(TypeError {
                        message: format!("variant '{}' is matched more than once", p.variant),
                        span: p.span,
                    });
                }
                if p.binds.len() != vdef.fields.len() {
                    let fields: Vec<String> =
                        vdef.fields.iter().map(|f| f.name.clone()).collect();
                    return Err(TypeError {
                        message: format!(
                            "pattern '{}({})' has {} binding(s) but variant '{}' carries {} payload field(s) ({})",
                            p.variant,
                            p.binds
                                .iter()
                                .map(|b| b.as_deref().unwrap_or("_").to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            p.binds.len(),
                            p.variant,
                            vdef.fields.len(),
                            fields.join(", ")
                        ),
                        span: p.span,
                    });
                }
                covered[vidx] = Some(ai);
                Some(vidx)
            }
        };

        // Payload bindings live in a scope of their own, so the same name may
        // be used by two arms and shadows nothing outside them.
        let mut bind_stmts: Vec<tir::Stmt> = Vec::new();
        ctx.scopes.push(HashMap::new());
        if let (Some(vidx), Some(p)) = (variant, &arm.pattern) {
            for (fi, f) in edef.variants[vidx].fields.iter().enumerate() {
                let Some(bind) = &p.binds[fi] else { continue };
                let field = tir::Expr {
                    kind: tir::ExprKind::EnumField {
                        base: Box::new(scrut.clone()),
                        variant: vidx,
                        field: fi,
                    },
                    ty: f.ty.clone(),
                    span: p.span,
                };
                let (ty, init) = if f.ty.is_copy() || owns_scrutinee {
                    (f.ty.clone(), field)
                } else {
                    // An owned payload of a scrutinee that is not a local this
                    // frame owns directly binds as a shared borrow: a move out
                    // by value is a partial move, and one only takes a
                    // projection out of a `val`/`var`/`move` local
                    // (`borrowck.rs::partial_move_base`), so a borrow scrutinee,
                    // a field path and a dereference keep the value where it is.
                    let ty = Ty::Ref {
                        mutable: false,
                        inner: Box::new(f.ty.clone()),
                    };
                    (
                        ty.clone(),
                        tir::Expr {
                            kind: tir::ExprKind::Ref {
                                mutable: false,
                                expr: Box::new(field),
                            },
                            ty,
                            span: p.span,
                        },
                    )
                };
                ctx.define(bind, ty.clone(), false);
                bind_stmts.push(tir::Stmt::Let {
                    name: bind.clone(),
                    ty,
                    init,
                });
            }
        }
        let body = check_expr(&arm.body, ctx, expected.or(arm_ty.as_ref()))?;
        ctx.scopes.pop();
        match arm_ty.clone() {
            // `str` and `&str` arms agree, and join to the borrow (see
            // `join_value_type`); anything else has to match exactly.
            Some(t) => match join_value_type(&t, &body.ty) {
                Some(joined) => arm_ty = Some(joined),
                None => {
                    return Err(TypeError {
                        message: format!(
                            "'when' arms have different types: '{}' and '{}'",
                            t.display(),
                            body.ty.display()
                        ),
                        span: arm.span,
                    })
                }
            },
            None => arm_ty = Some(body.ty.clone()),
        }
        checked.push((
            variant,
            tir::Block {
                stmts: bind_stmts,
                tail: Some(Box::new(body)),
            },
        ));
    }

    if !else_seen {
        let missing: Vec<String> = edef
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| covered[*i].is_none())
            .map(|(_, v)| format!("'{}'", v.name))
            .collect();
        if !missing.is_empty() {
            return Err(TypeError {
                message: format!(
                    "this 'when' is not exhaustive: {} of enum '{ename}' has no arm - add an arm, or an 'else' arm",
                    missing.join(" and ")
                ),
                span,
            });
        }
    }

    // Fold right-to-left. The last arm becomes the base `else` and gets no
    // tag test of its own: after every other arm's test has failed, only its
    // variant can be live (and when it is an `else` arm, that is its meaning).
    // Doing it this way also means the chain always has an `else`, so the
    // temporary an `if`-as-a-value lowers to is always assigned on both
    // paths - no uninitialized read, and nothing for a C warning to flag.
    let (_, base_block) = checked
        .pop()
        .expect("a 'when' always parses with at least one arm");
    let mut acc: tir::Block = base_block;
    for (variant, block) in checked.into_iter().rev() {
        let vidx = variant.expect("only the last arm may be an 'else' arm");
        let cond = tir::Expr {
            kind: tir::ExprKind::EnumTag {
                expr: Box::new(scrut.clone()),
                variant: vidx,
            },
            ty: Ty::Bool,
            span,
        };
        let block_ty = block.ty();
        let if_expr = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(cond),
                then_block: block,
                else_block: Some(acc),
            },
            ty: block_ty,
            span,
        };
        acc = tir::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(if_expr)),
        };
    }
    let ty = acc.ty();
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(acc),
        ty,
        span,
    })
}

/// Whether a bare identifier names an `impl` alias rather than a value. A
/// local, parameter, receiver field, or passdown shadows it, exactly as it
/// would shadow any other name.
/// A note appended to an error only a stdlib *instantiation* can produce: a
/// helper written for a `Copy` payload or element fails inside the module, and
/// the span points into a file the caller never opened. Empty everywhere else.
fn stdlib_instantiation_note(ctx: &Ctx) -> String {
    if is_shipped_module(&ctx.module) {
        format!(
            " - this is inside the shipped standard library ('{}'), so the instantiation you asked for is one it does not offer: a helper that hands a value out is written for a 'Copy' payload or element, and the module's header says which",
            ctx.module
        )
    } else {
        String::new()
    }
}

fn is_module_alias(name: &str, ctx: &Ctx) -> bool {
    ctx.lookup(name).is_none()
        && !ctx.pds.contains_key(name)
        && ctx.aliases.contains_key(name)
}

/// A method call whose receiver is a local binding that shares its name with a
/// module alias in scope (`MISSING-9tRe4`). The missing-method message names the
/// local's type and never the module the call was meant to reach, which sends
/// the reader to look for a method on a type that was never going to have one.
///
/// The note is only added when the shadowed module really declares that method:
/// the sentence is a claim about where the call could have gone, so it has to be
/// true of that module, not merely of the name.
fn shadowed_module_alias_note(receiver: &ast::Expr, method: &str, ctx: &Ctx) -> String {
    let ast::Expr::Ident(name, _) = receiver else {
        return String::new();
    };
    if ctx.lookup(name).is_none() {
        return String::new();
    }
    let Some(prefix) = ctx.aliases.get(name) else {
        return String::new();
    };
    let key = format!("{prefix}{method}");
    // The sentence is a claim about where the call could have gone, so the
    // item has to be one the module actually exports (A-13).
    if ctx.sig_of(&key).is_none() || !exported(&key, &ctx.env) {
        return String::new();
    }
    format!(
        " '{name}' is a local binding here, so '{name}.{method}' is not the module '{name}'; rename the binding"
    )
}

/// The passdown whose storage a place expression is, if it is one: the hidden
/// parameter (`pd#<name>`, already a borrow) directly, or behind the explicit
/// `&` that a `main` or `pd` initializer takes of the generated global. Any
/// other place - a local, a field of one, a dereferenced borrow - is `None`.
/// The two shapes are why this looks through a `Ref` rather than matching the
/// identifier alone (M6.4).
fn pd_place_hidden(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ident(name) if name.starts_with("pd#") => Some(name),
        tir::ExprKind::Ref { expr, .. } => pd_place_hidden(expr),
        _ => None,
    }
}

/// Assignment targets: `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`) - anything `is_ast_place`
/// accepts.
/// The two rules a written place obeys: it must be writable at all (`var`, an
/// exclusive borrow, ...), and a field of a `datac` must be declared `var`.
/// Shared by `=` and by an `asm` operand's `out`/`inout` place, so the two
/// cannot drift.
fn check_written_place(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    check_target_writable(target, ctx)?;
    // Only the *immediate* field being written needs to be `var`; fields merely
    // read through on the way there do not.
    if let tir::ExprKind::FieldAccess { base, field } = &target.kind {
        let sname = struct_name_of(&base.ty)
            .expect("field access base is always a struct type, guaranteed by check_expr");
        let sdef = ctx
            .struct_def(sname)
            .expect("struct types are only produced for known structs");
        let fdef = sdef.field(field).expect("field already resolved by check_expr");
        if !fdef.mutable {
            return Err(TypeError {
                message: format!("cannot write to '{field}': declared 'val'"),
                span: target.span,
            });
        }
    }
    Ok(())
}

/// `asm(...)` (kernel stage 5). The construct is an unsafe operation, so on top
/// of the module's `[unsafe]` listing (`ast::first_unsafe`) the use site has to
/// be inside `unsafe { ... }` or an `unsafe fun`, exactly as a raw pointer write
/// does. The operand types are machine words; the constraint names are passed to
/// the backend unchanged.
fn check_asm(
    template: &str,
    operands: &[ast::AsmOperand],
    clobbers: &[String],
    nomem: bool,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if ctx.unsafe_depth == 0 {
        return Err(TypeError {
            message: "'asm' is an unsafe operation: it can only be used inside 'unsafe { ... }' or in a function declared 'unsafe fun'".to_string(),
            span,
        });
    }
    let mut checked: Vec<tir::AsmOperand> = Vec::new();
    let mut value_out: Option<usize> = None;
    let mut value_ty: Option<Ty> = None;
    for (i, op) in operands.iter().enumerate() {
        let operand = match op.kind {
            ast::AsmOperandKind::In => {
                let e = op
                    .operand
                    .as_ref()
                    .expect("the parser requires an operand for 'in'");
                let v = check_expr(e, ctx, None)?;
                require_asm_operand_ty(&v.ty, op.span)?;
                Some(v)
            }
            ast::AsmOperandKind::Out | ast::AsmOperandKind::InOut => match &op.operand {
                Some(e) => {
                    if !is_ast_place(e) {
                        return Err(TypeError {
                            message: format!(
                                "a '{}' operand must be a place: the asm writes it back, so an arbitrary expression has nowhere to be written",
                                asm_kind_word(op.kind)
                            ),
                            span: op.span,
                        });
                    }
                    let p = check_expr(e, ctx, None)?;
                    check_written_place(&p, ctx)?;
                    require_asm_operand_ty(&p.ty, op.span)?;
                    Some(p)
                }
                None => {
                    if value_out.is_some() {
                        return Err(TypeError {
                            message: "an 'asm' expression has at most one anonymous output: name the others with a place ('out(\"r\") x')".to_string(),
                            span: op.span,
                        });
                    }
                    // An output is a machine word, so `i64` answers when the
                    // context names no type; a written type wins.
                    let ty = expected.cloned().unwrap_or(Ty::I64);
                    require_asm_operand_ty(&ty, op.span)?;
                    value_out = Some(i);
                    value_ty = Some(ty);
                    None
                }
            },
        };
        checked.push(tir::AsmOperand {
            kind: op.kind,
            constraint: op.constraint.clone(),
            operand,
        });
    }
    // A `{n}` that names no operand would be left in the template verbatim and
    // read by the assembler as a brace, so it is refused here where the operands
    // are known.
    for n in asm_placeholders(template) {
        if n >= operands.len() {
            return Err(TypeError {
                message: format!(
                    "the asm template refers to operand {{{n}}}, but the asm has {} operand{}",
                    operands.len(),
                    if operands.len() == 1 { "" } else { "s" }
                ),
                span,
            });
        }
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Asm(tir::AsmBlock {
            template: template.to_string(),
            operands: checked,
            clobbers: clobbers.to_vec(),
            nomem,
            value_out,
        }),
        ty: value_ty.unwrap_or(Ty::Unit),
        span,
    })
}

/// Every `{n}` placeholder in an asm template, in the order they appear.
fn asm_placeholders(template: &str) -> Vec<usize> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == '}') {
                if let Ok(n) = chars[i + 1..i + 1 + close]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                {
                    out.push(n);
                    i += close + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `in`, `out` or `inout`, for a diagnostic.
fn asm_kind_word(kind: ast::AsmOperandKind) -> &'static str {    match kind {
        ast::AsmOperandKind::In => "in",
        ast::AsmOperandKind::Out => "out",
        ast::AsmOperandKind::InOut => "inout",
    }
}

/// An `asm` operand is one machine value: an integer, `bool`, `char`, `f32`/
/// `f64`, a `Ptr<T>` or an `extern type` handle. Everything else (an owned
/// buffer, a `datac`, an array, a borrow) has no single-register form, so it is
/// refused by name rather than laid out implicitly.
fn require_asm_operand_ty(ty: &Ty, span: Span) -> Result<()> {
    if ty.is_numeric() || matches!(ty, Ty::Bool | Ty::Char | Ty::Ptr(_) | Ty::Handle(_)) {
        Ok(())
    } else {
        Err(TypeError {
            message: format!(
                "'{}' cannot be an asm operand: an operand is one machine value (an integer, 'bool', 'char', 'f32'/'f64', a 'Ptr' or an 'extern type' handle)",
                ty.display()
            ),
            span,
        })
    }
}

fn check_assign(target: &ast::Expr, value: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if !is_ast_place(target) {
        return Err(TypeError {
            message: "invalid assignment target".to_string(),
            span,
        });
    }

    let target_t = check_expr(target, ctx, None)?;
    check_written_place(&target_t, ctx)?;

    let value_t = check_expr(value, ctx, Some(&target_t.ty))?;
    if value_t.ty != target_t.ty {
        return Err(TypeError {
            message: format!(
                "cannot assign value of type '{}' to target of type '{}'",
                value_t.ty.display(),
                target_t.ty.display()
            ),
            span,
        });
    }

    Ok(tir::Expr {
        kind: tir::ExprKind::Assign {
            target: Box::new(target_t),
            value: Box::new(value_t),
        },
        ty: Ty::Unit,
        span,
    })
}

/// Whether a comparison operator (`==`/`!=`/`<`/…) can be applied to this
/// type at all. Comparisons are only generated for scalars C can compare
/// directly: an owned value, a `datac`, or a `str` has no comparison of its
/// own, and emitting one produces invalid C at `cc` (or, for `str`, a
/// comparison of *addresses* rather than text - silently wrong, which is
/// worse). References are allowed *except* `&str`, which is the same address
/// comparison in disguise.
fn comparable(ty: &Ty) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char)
        || (matches!(ty, Ty::Ref { .. }) && !is_str_like(ty))
}
fn comparison_error(ty: &Ty, op: BinOp) -> String {
    let sym = match op {
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        _ => unreachable!("not a comparison operator"),
    };
    if is_str_like(ty) {
        return format!(
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text - use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        );
    }
    match ty {
        Ty::Str => format!(
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text - use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        ),
        // A `View` *could* compare byte-wise (it has a length), but `==` on a
        // non-scalar is not how any other type reads and `view.eq` says what
        // it does.
        Ty::View => format!(
            "cannot compare 'View' values with '{sym}': use 'view.eq(other)' - it compares the bytes and the lengths, where '{sym}' would compare the pointers"
        ),
        Ty::Bytes | Ty::String => format!(
            "cannot compare '{}' values with '{}': v1 has no equality for owned values",
            ty.display(),
            sym
        ),
        Ty::Struct(_) => format!(
            "cannot compare '{}' values with '{}': a datac has no generated equality",
            ty.display(),
            sym
        ),
        other => format!("cannot compare values of type '{}'", other.display()),
    }
}

/// The owned type an equality operand names: a `datac`/`enum` value, or a
/// borrow of one. `Ty::Struct` covers both a `datac` and an `enum`, and a
/// `Ty::Ref` is looked through because an operand may be written `a` or `*a`
/// or be a `mut T` parameter, and all of them are one pointer to the backend.
fn equality_owner(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name) => Some(name.as_str()),
        Ty::Ref { inner, .. } => match inner.as_ref() {
            Ty::Struct(name) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// One operand of a lowered `==`/`!=` where the declared `eq` wants `&T`: an
/// owned operand is borrowed (`Ref`), and an operand that is already a borrow
/// is handed on unchanged, so neither is ever moved.
fn equality_borrow(expr: &tir::Expr, want: &Ty) -> tir::Expr {
    match &expr.ty {
        Ty::Ref { .. } => expr.clone(),
        _ => tir::Expr {
            kind: tir::ExprKind::Ref {
                mutable: false,
                expr: Box::new(expr.clone()),
            },
            ty: want.clone(),
            span: expr.span,
        },
    }
}

/// One operand of a lowered `View == View`/`View == str`: a `View` is used as
/// it is, a `str` is viewed first (`str#view`, the builtin `s.view()`), since
/// `View.eq` takes a `View`.
fn view_operand(expr: &tir::Expr) -> tir::Expr {
    if matches!(expr.ty, Ty::View) {
        expr.clone()
    } else {
        tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "str#view".to_string(),
                args: vec![expr.clone()],
            },
            ty: Ty::View,
            span: expr.span,
        }
    }
}

/// `!=` is the lowering's result negated; `==` is the call itself.
fn negate_comparison(op: BinOp, call: tir::Expr, span: Span) -> tir::Expr {
    if matches!(op, BinOp::NotEq) {
        tir::Expr {
            kind: tir::ExprKind::Unary {
                op: ast::UnOp::Not,
                expr: Box::new(call),
            },
            ty: Ty::Bool,
            span,
        }
    } else {
        call
    }
}

/// H-5: whether `a == b` / `a != b` is lowered in the front end into a method
/// call instead of the scalar comparison. Two shapes lower:
///
/// * a `datac`/`enum` `T` that declares `fun T.eq(other: &T).return: bool` (the same
///   method name A-12 gives a declared `Map` key), lowered to `a.eq(&b)`;
/// * `View` against `View`, or `View` against a `str`, lowered to the builtin
///   `View.eq`.
///
/// The operands are the already-checked `l`/`r` from `check_binary_operands`,
/// so each is evaluated exactly once and left to right, and a shared `Ref` is
/// what the declared `eq` receives, so an owned operand is borrowed rather than
/// moved. The result is an ordinary `Call`, so `borrowck.rs` and both backends
/// need no new shape. `None` means the ordinary comparison path applies, which
/// includes every type with no declared `eq` and every order operator.
fn lower_equality(
    l: &tir::Expr,
    r: &tir::Expr,
    op: BinOp,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    if !matches!(op, BinOp::Eq | BinOp::NotEq) {
        return Ok(None);
    }
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b {
            let key = crate::types::key_pair_method_key(a, crate::types::KEY_PAIR_EQ_METHOD);
            if let Some(sig) = ctx.sig_of(&key) {
                // The operator is a call, so an `unsafe fun T.eq` is gated
                // exactly as the written `a.eq(&b)` call is (the `Map` path
                // has no such sight of the declaration and is a known gap).
                if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
                    return Err(TypeError {
                        message: format!(
                            "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                            display_callee(&key)
                        ),
                        span,
                    });
                }
                let mut args = vec![
                    equality_borrow(l, &sig.params[0].1),
                    equality_borrow(r, &sig.params[1].1),
                ];
                args.extend(hidden_args(&key, span, ctx)?);
                let call = tir::Expr {
                    kind: tir::ExprKind::Call { callee: key, args },
                    ty: Ty::Bool,
                    span,
                };
                return Ok(Some(negate_comparison(op, call, span)));
            }
        }
    }
    let l_text = matches!(l.ty, Ty::View) || is_str_like(&l.ty);
    let r_text = matches!(r.ty, Ty::View) || is_str_like(&r.ty);
    if l_text
        && r_text
        && (matches!(l.ty, Ty::View) || matches!(r.ty, Ty::View))
        && builtin_sig("View#eq").is_some()
    {
        let call = tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "View#eq".to_string(),
                args: vec![view_operand(l), view_operand(r)],
            },
            ty: Ty::Bool,
            span,
        };
        return Ok(Some(negate_comparison(op, call, span)));
    }
    // `Ord` says nothing about equality, so a type that implements only the
    // order trait is refused `==`/`!=` with a message that says which
    // declaration the operator actually needs (`plan/traits.md` 13.3).
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b && implements_trait(&ctx.env, crate::types::ORD_TRAIT, a) {
            return Err(TypeError {
                message: format!(
                    "cannot compare '{}' values with '{}': '==' and '!=' come from a declared 'eq' method, and 'Ord' does not supply one; write 'fun {}.eq(other: &{}).return: bool'",
                    a,
                    bin_op_symbol(op),
                    a,
                    a
                ),
                span,
            });
        }
    }
    Ok(None)
}

/// The symbol one binary operator is written with, for a diagnostic.
fn bin_op_symbol(op: BinOp) -> &'static str {
    match op {
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
    }
}

/// The `datac`/`enum` an operator applies to, when both operands name the same
/// user type (a borrow of one counts, exactly as an `==` operand may be `&T`)
/// and that type implements the shipped operator trait `trait_key`. `None`
/// means the operator's ordinary rule applies, which is what keeps a builtin
/// numeric pair and a genuine type mismatch on their original paths.
fn operator_owner(l: &tir::Expr, r: &tir::Expr, trait_key: &str, ctx: &Ctx) -> Option<String> {
    let a = equality_owner(&l.ty)?;
    let b = equality_owner(&r.ty)?;
    if a != b || !implements_trait(&ctx.env, trait_key, a) {
        return None;
    }
    Some(a.to_string())
}

/// The `datac`/`enum` a unary operator applies to, when the operand names a
/// user type that implements `trait_key`.
fn unary_operator_owner(operand: &tir::Expr, trait_key: &str, ctx: &Ctx) -> Option<String> {
    let name = equality_owner(&operand.ty)?;
    if !implements_trait(&ctx.env, trait_key, name) {
        return None;
    }
    Some(name.to_string())
}

/// The trait, the trait's bare name and the method one arithmetic operator
/// lowers through (`plan/traits.md` 13.2).
fn arithmetic_operator(op: BinOp) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match op {
        BinOp::Add => (crate::types::ADD_TRAIT, "Add", "add"),
        BinOp::Sub => (crate::types::SUB_TRAIT, "Sub", "sub"),
        BinOp::Mul => (crate::types::MUL_TRAIT, "Mul", "mul"),
        BinOp::Div => (crate::types::DIV_TRAIT, "Div", "div"),
        BinOp::Mod => (crate::types::REM_TRAIT, "Rem", "rem"),
        _ => return None,
    })
}

/// Lowers one operator into the `std/ops` method call it means: the operands
/// are borrowed (never moved) exactly as `==`'s are, and each was already
/// checked, so it is evaluated once and left to right. The result is an
/// ordinary `Call`, so `borrowck.rs` and both backends need no new shape.
fn lower_operator_call(
    owner: &str,
    method: &str,
    operands: &[&tir::Expr],
    span: Span,
    ctx: &Ctx,
) -> Result<tir::Expr> {
    let key = crate::types::key_pair_method_key(owner, method);
    let sig = ctx.sig_of(&key).ok_or_else(|| TypeError {
        message: format!(
            "'{owner}' implements the operator trait, but its '{method}' method is missing"
        ),
        span,
    })?;
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            message: format!(
                "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                display_callee(&key)
            ),
            span,
        });
    }
    let mut args = Vec::with_capacity(operands.len());
    for (i, operand) in operands.iter().enumerate() {
        match sig.params.get(i) {
            Some((_, want)) => args.push(equality_borrow(operand, want)),
            None => args.push((*operand).clone()),
        }
    }
    args.extend(hidden_args(&key, span, ctx)?);
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args },
        ty: sig.ret,
        span,
    })
}

/// Lowers `a OP b` for a user type that implements the shipped operator trait,
/// or `None` when the operand types are not one user type with the impl.
fn lower_arithmetic_operator(
    op: BinOp,
    l: &tir::Expr,
    r: &tir::Expr,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    let Some((trait_key, _, method)) = arithmetic_operator(op) else {
        return Ok(None);
    };
    let Some(owner) = operator_owner(l, r, trait_key, ctx) else {
        return Ok(None);
    };
    Ok(Some(lower_operator_call(&owner, method, &[l, r], span, ctx)?))
}

/// Lowers `a OP b` for a user type that implements `std/ops.Ord`: the `cmp`
/// call's `i32` against zero (`plan/traits.md` 13.3).
fn lower_order_operator(
    op: BinOp,
    l: &tir::Expr,
    r: &tir::Expr,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    let Some(owner) = operator_owner(l, r, crate::types::ORD_TRAIT, ctx) else {
        return Ok(None);
    };
    let call = lower_operator_call(&owner, "cmp", &[l, r], span, ctx)?;
    let zero = tir::Expr {
        kind: tir::ExprKind::IntLit(0),
        ty: Ty::I32,
        span,
    };
    Ok(Some(tir::Expr {
        kind: tir::ExprKind::Binary {
            op,
            lhs: Box::new(call),
            rhs: Box::new(zero),
        },
        ty: Ty::Bool,
        span,
    }))
}

/// Lowers a `Display` value in `"${x}"` or `print(x)` to `x.to_text()`
/// (`plan/traits.md` 13.4). `None` when the type does not implement `Display`,
/// which leaves the existing refusal in place.
fn lower_display(value: &tir::Expr, span: Span, ctx: &Ctx) -> Result<Option<tir::Expr>> {
    let Some(owner) = unary_operator_owner(value, crate::types::DISPLAY_TRAIT, ctx) else {
        return Ok(None);
    };
    Ok(Some(lower_operator_call(&owner, "to_text", &[value], span, ctx)?))
}

/// The refusal for an arithmetic operator whose operands are not a matching
/// numeric pair. When both name the same `datac`/`enum`, the operator traits
/// are the answer, so the message names the trait and the impl to write
/// (`plan/traits.md` 13.2).
fn arithmetic_error(op: BinOp, l: &tir::Expr, r: &tir::Expr) -> String {
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b {
            if let Some((_, simple, method)) = arithmetic_operator(op) {
                return format!(
                    "cannot use '{}' on '{}': it does not implement '{}' from 'std/ops' (write 'impl {} for {} {{ fun Self.{}(rhs: &Self).return: {} }}', importing 'std/ops')",
                    bin_op_symbol(op),
                    a,
                    simple,
                    simple,
                    a,
                    method,
                    a
                );
            }
        }
        return format!(
            "cannot use '{}' on '{}' and '{}': the operands must have the same type",
            bin_op_symbol(op),
            a,
            b
        );
    }
    format!(
        "operator requires matching numeric operands, found '{}' and '{}'{}",
        l.ty.display(),
        r.ty.display(),
        exclusive_borrow_hint(l, r),
    )
}

/// The refusal for a value `print` cannot format. A `datac`/`enum` names
/// `Display`, which is what it has to implement (`plan/traits.md` 13.4).
fn print_error(callee: &str, ty: &Ty) -> String {
    match ty {
        Ty::Struct(_) => format!(
            "cannot pass a value of type '{}' to '{callee}': it does not implement 'Display' (write 'impl fmt.Display for {} {{ fun Self.to_text().return: String }}', importing 'std/fmt')",
            ty.display(),
            ty.display()
        ),
        other => format!("cannot pass a value of type '{}' to '{callee}'", other.display()),
    }
}

/// Whether the expression is a bare numeric or character literal - a value
/// with **no width of its own**, whose type comes entirely from its context
/// (`syntax.md` §2.4/§5). A literal in that position has no type until
/// something asks for one, which is what lets the rule below pick the
/// sibling operand's type for it.
///
/// An `Ident` is deliberately not one: a `const` has a *declared* type, so
/// `const N: i32 = 5` followed by `N + u32_expr` is a real mismatch rather
/// than a literal to retype.
fn is_untyped_literal(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::IntLit(..) | ast::Expr::FloatLit(..) | ast::Expr::CharLit(..)
    )
}

/// Both operands of a binary operator, with the rule that makes the language's
/// own literal story true: **a literal takes the type of its sibling operand,
/// whichever side it is written on.**
///
/// The naive left-to-right order (`l` with the expectation, then `r` with
/// `l`'s type) leaves one hole, and it is the one a program written against
/// Rust's inference falls straight into:
///
/// ```kelp
/// fun push_byte(out: mut String, cp: u32) {
///     out.push((192 + cp / 64).to_u8())   // '192' can only be i32, from the left
/// }
/// ```
///
/// `192 + cp / 64` is `u32` in Rust, where the literal's type is inferred from
/// the expression it meets. Here `192` was typed `i32` before the right side
/// was ever read, and the result was
/// `operator requires matching numeric operands, found 'i32' and 'u32'` - for
/// a rule the reference already stated ("a literal has no width of its own; it
/// takes the integer type its context requires"). The sibling operand *is* the
/// context.
///
/// So: when there is no expectation to work from and the left side is a bare
/// literal while the right side is not, the right side is checked first and
/// the literal takes its type. Both literals, or neither, still go left to
/// right - nothing about the common cases changes.
fn check_binary_operands(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    ctx: &mut Ctx,
    hint: Option<&Ty>,
) -> Result<(tir::Expr, tir::Expr)> {
    if hint.is_none() && is_untyped_literal(lhs) && !is_untyped_literal(rhs) {
        let right = check_expr(rhs, ctx, None)?;
        let left = check_expr(lhs, ctx, Some(&right.ty))?;
        return Ok((left, right));
    }
    let left = check_expr(lhs, ctx, hint)?;
    let right = check_expr(rhs, ctx, Some(&left.ty))?;
    Ok((left, right))
}

/// When one operand of an arithmetic operator is an exclusive borrow, the
/// operator is reading the *reference* rather than the value behind it. That
/// happens with a `mut T` parameter, whose name inside the body is a borrow
/// (`syntax.md` 10.2), and the documented spelling is `*sink`, not `sink`
/// (§10's `*pos` rule). Name the operand and say so, because the bare
/// `mut i64` in the type pair is otherwise a type no source can write.
fn exclusive_borrow_hint(l: &tir::Expr, r: &tir::Expr) -> String {
    let expr = match (&l.ty, &r.ty) {
        (Ty::Ref { mutable: true, .. }, _) => l,
        (_, Ty::Ref { mutable: true, .. }) => r,
        _ => return String::new(),
    };
    let (written, fix) = match &expr.kind {
        tir::ExprKind::Ident(n) => (format!("'{n}'"), format!("*{n}")),
        _ => ("this operand".to_string(), "the dereferenced value".to_string()),
    };
    format!(
        "; {written} is a borrow ('{}'), so the operator reads the reference itself; write '{fix}' to read through it",
        expr.ty.display()
    )
}

fn check_binary(
    op: BinOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => {
            let numeric_expected = expected.filter(|t| t.is_numeric());
            let (l, r) = check_binary_operands(lhs, rhs, ctx, numeric_expected)?;
            if !l.ty.is_numeric() || l.ty != r.ty {
                // A `datac`/`enum` that implements `std/ops`'s trait for this
                // operator is lowered into the trait method here
                // (`plan/traits.md` 13.2); a builtin numeric pair never reaches
                // this point, so the builtin operators are untouched.
                if let Some(expr) = lower_arithmetic_operator(op, &l, &r, span, ctx)? {
                    return Ok(expr);
                }
                return Err(TypeError {
                    message: arithmetic_error(op, &l, &r),
                    span,
                });
            }
            let ty = l.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty,
                span,
            })
        }
        // Bitwise operators (A-9). Integral operands of the same type,
        // result the same type - the same rule arithmetic uses, one domain
        // over. `~` is the prefix form and is checked on the unary path.
        //
        // `bool` is deliberately not a bitwise domain: `a & b` on two
        // booleans reads as a logical `and` that is not short-circuiting,
        // which is a trap, and `&&` is right there.
        BitAnd | BitOr | BitXor | Shl | Shr => {
            let int_expected = expected.filter(|t| t.is_integer());
            let (l, r) = check_binary_operands(lhs, rhs, ctx, int_expected)?;
            if !l.ty.is_integer() || l.ty != r.ty {
                return Err(TypeError {
                    message: format!(
                        "operator requires matching integer operands, found '{}' and '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            // A shift by an amount already known to be out of range is a
            // compile error; any other amount is checked at run time by both
            // backends (`shift amount out of range`).
            if matches!(op, Shl | Shr) {
                if let (tir::ExprKind::IntLit(v), Some(bits)) = (&r.kind, int_bits(&l.ty)) {
                    if *v < 0 || *v >= bits as i128 {
                        return Err(TypeError {
                            message: format!(
                                "a shift by {v} is not defined for a '{}', whose width is {bits} bits: the amount has to be in 0..{bits} (use wrapping_shl/wrapping_shr to mask it)",
                                l.ty.display()
                            ),
                            span,
                        });
                    }
                }
            }
            let ty = l.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty,
                span,
            })
        }
        Lt | LtEq | Gt | GtEq | Eq | NotEq => {
            let (l, r) = check_binary_operands(lhs, rhs, ctx, None)?;
            // Two handles of the same type, or of an ancestor/descendant pair,
            // compare: the pointers are the same width either way, and an
            // upcast would have produced the ancestor value anyway.
            let handle_pair = match (&l.ty, &r.ty) {
                (Ty::Handle(a), Ty::Handle(b)) => {
                    handle_is_same_or_ancestor(&ctx.env.handle_parents, a, b)
                        || handle_is_same_or_ancestor(&ctx.env.handle_parents, b, a)
                }
                _ => false,
            };
            // H-5: a `datac`/`enum` that declares `eq`, and the two `View`
            // shapes, are an ordinary method call (or its negation) by the time
            // the rest of the pipeline sees them. This runs before the
            // operand-type match below because a `&T` against a `T` and a
            // `View` against a `str` are two different spellings of one
            // comparison.
            if let Some(expr) = lower_equality(&l, &r, op, span, ctx)? {
                return Ok(expr);
            }
            // `Ord` gives the four order operators, through `cmp`
            // (`plan/traits.md` 13.3). Equality already returned above.
            if let Some(expr) = lower_order_operator(op, &l, &r, span, ctx)? {
                return Ok(expr);
            }
            if l.ty != r.ty && !handle_pair {
                return Err(TypeError {
                    message: format!(
                        "cannot compare '{}' with '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            // An opaque handle has equality and nothing else: `==`/`!=`
            // compare the two pointers, which is cheap and is the only
            // thing the operator could mean for a value with no readable
            // contents. An order (`<`, `<=`, `>`, `>=`) is still refused.
            let comparable = comparable(&l.ty)
                || ((matches!((&l.ty, &r.ty), (Ty::Handle(_), Ty::Handle(_)))
                    || matches!((&l.ty, &r.ty), (Ty::Ptr(_), Ty::Ptr(_))))
                    && matches!(op, BinOp::Eq | BinOp::NotEq));
            if !comparable {
                return Err(TypeError {
                    message: comparison_error(&l.ty, op),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty: Ty::Bool,
                span,
            })
        }
        And | Or => {
            let l = check_expr(lhs, ctx, Some(&Ty::Bool))?;
            let r = check_expr(rhs, ctx, Some(&Ty::Bool))?;
            if l.ty != Ty::Bool || r.ty != Ty::Bool {
                return Err(TypeError {
                    message: "logical operators require bool operands".to_string(),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty: Ty::Bool,
                span,
            })
        }
    }
}

/// Matches call-site arguments (each optionally named, `f(x = 1)`) against
/// a callee's declared parameter list, by name first and positionally for
/// the rest - shared between plain function calls and `datac` construction,
/// since both look identical at a call site.
fn match_args(
    what: &str,
    params: &[(String, Ty)],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Vec<tir::Expr>> {
    let (fixed, extra) = match_args_split(what, params, args, span, ctx, false)?;
    debug_assert!(extra.is_empty(), "a non-variadic call cannot have extra arguments");
    Ok(fixed)
}

/// `match_args`'s body. When `variadic` is set, trailing *positional*
/// arguments after the declared parameters are returned unchecked instead of
/// being refused: the caller of a variadic user `extern fun` (gap 7) checks
/// and appends them itself. A named argument that matches no parameter is
/// still refused, whatever `variadic` says.
fn match_args_split<'a>(
    what: &str,
    params: &[(String, Ty)],
    args: &'a [ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    variadic: bool,
) -> Result<(Vec<tir::Expr>, Vec<&'a ast::CallArg>)> {
    if args.len() != params.len() && !(variadic && args.len() > params.len()) {
        return Err(TypeError {
            message: format!("{what} expects {} argument(s), found {}", params.len(), args.len()),
            span,
        });
    }

    let mut named: HashMap<&str, &ast::CallArg> = HashMap::new();
    for a in args {
        if let Some(n) = &a.name {
            if named.insert(n.as_str(), a).is_some() {
                return Err(TypeError {
                    message: format!("argument '{n}' specified more than once"),
                    span,
                });
            }
        }
    }

    let mut positional = args.iter().filter(|a| a.name.is_none());
    let mut result = Vec::with_capacity(params.len());
    for (pname, pty) in params {
        let arg = match named.remove(pname.as_str()) {
            Some(a) => a,
            None => positional.next().ok_or_else(|| TypeError {
                message: format!("{what} is missing argument '{pname}'"),
                span,
            })?,
        };
        // The label the diagnostics use: a declared parameter is quoted by
        // name, which is what a call site can see.
        result.push(check_call_arg(&format!("'{pname}'"), pty, &arg.value, ctx)?);
    }

    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }

    // A variadic call's extra arguments have no name to be matched by, so
    // anything left is by definition positional and in order.
    let extra: Vec<&ast::CallArg> = positional.collect();
    Ok((result, extra))
}

/// Checks one call-site argument against its matched parameter type. When
/// the parameter is `Ty::Ref`, the caller never writes `&` - a plain place
/// argument (`p`, `p.x`) is auto-borrowed here (the callee's signature is
/// what decides shared vs exclusive), matching `plan/syntax.md`'s "shared
/// borrow is implicit - write nothing." An argument that's already a borrow
/// (an explicit `&expr`, or forwarding an existing `Ref`-typed parameter)
/// type-matches directly instead of double-wrapping; a shared parameter
/// accepts an exclusive argument (implicit downgrade), never the reverse.
/// A call whose result *borrows from an argument* (the elision rule) cannot
/// borrow from a temporary: the temporary is freed at the end of the
/// enclosing statement, and codegen would have to drop it before the borrow
/// could be used. Argument matching itself allows a temporary for a shared
/// borrow - that is what makes `str.join(str.split(s, sep), sep)` legal - so
/// the one case where the lifetime actually matters is checked here, where
/// the callee's return type and its elided root are both known.
///
/// Only a *freshly auto-borrowed temporary* is the problem. An argument that
/// is already a borrow value (a `&String` from `l.get(i)`, a `&str` from
/// `s.as_str()`) is a pointer whose own chain `borrowck.rs` walks to its
/// root, so it is left alone.
fn require_place_for_borrowed_arg(
    params: &[(String, Ty)],
    ret: &Ty,
    args: &[tir::Expr],
    span: Span,
) -> Result<()> {
    if !is_borrowed_value(ret) {
        return Ok(());
    }
    let Some(idx) = borrow_root_param(params) else {
        return Ok(());
    };
    let Some(arg) = args.get(idx) else {
        return Ok(());
    };
    let tir::ExprKind::Ref { expr: inner, .. } = &arg.kind else {
        return Ok(());
    };
    // `is_durable_place`, not `is_tir_place`: a field of a temporary has an
    // address (codegen stages one) but not one that outlives the statement the
    // borrow is taken in, which is the whole question here.
    if !is_durable_place(inner) {
        return Err(TypeError {
            message: format!(
                "this call returns a borrow of its parameter '{}', so that argument has to be a variable or a field - a temporary would be freed before the borrow could be used; bind it to a local first (`val tmp = <the expression>`) and pass `tmp` instead",
                params[idx].0
            ),
            span,
        });
    }
    Ok(())
}

/// `check_expr(arg, Some(expected))`, except that a receiver a method call
/// already checked with no expectation is reused when its type is exactly the
/// expected one (the expectation then changes nothing about the result).
fn check_expr_or_memo(
    memo: Option<tir::Expr>,
    arg_expr: &ast::Expr,
    ctx: &mut Ctx,
    expected: &Ty,
) -> Result<tir::Expr> {
    match memo {
        Some(done) if done.ty == *expected => Ok(done),
        _ => check_expr(arg_expr, ctx, Some(expected)),
    }
}

/// `pname` is how the argument is named in a diagnostic - `'b'` for a
/// declared parameter, `1` for the position in a call through a function value
/// (a function *type* has no parameter names).
fn check_call_arg(pname: &str, pty: &Ty, arg_expr: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let memo = ctx.take_probe(arg_expr);
    // A `scoped fun(...)` parameter accepts only a `fun` literal or a trailing
    // block written at this call site (`plan/scoped-blocks.md`), never a
    // stored value. The one exception is forwarding a scoped value that is
    // already in hand - a `scoped fun(...)` parameter of this frame, or a
    // `val` bound to one - which is still inside the call.
    if matches!(pty, Ty::ScopedFn { .. }) && !matches!(arg_expr, ast::Expr::FnLit { .. }) {
        let checked = check_expr(arg_expr, ctx, None)?;
        if !matches!(checked.ty, Ty::ScopedFn { .. }) {
            return Err(TypeError {
                message: format!(
                    "argument {pname} has type '{}', expected '{}': a scoped parameter accepts only a function literal or a trailing block written at this call site, never a stored value",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        if checked.ty != *pty {
            return Err(TypeError {
                message: format!(
                    "argument {pname} has type '{}', expected '{}'",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    }
    let Ty::Ref { mutable, inner } = pty else {
        let checked = check_expr_or_memo(memo, arg_expr, ctx, pty)?;
        refuse_open_arg(&checked.ty, arg_expr)?;
        // A `&str` view and a `str` are the same value (both are the flat
        // borrowed view type), so a view satisfies a `str` parameter
        // directly - that is what makes `str.len(s.as_str())` work (M6.3).
        if is_str_like(pty) && is_str_like(&checked.ty) {
            return Ok(checked);
        }
        // The parameter is bare (by value), so this call *moves* its argument
        // in - which a `move T` parameter and every container's
        // `push`/`set`-shaped builtin both do. A borrow cannot be moved out
        // of, so name that instead of leaving a plain type mismatch
        // (`'&String'` against `'String'`) to imply the caller forgot an `&`
        // (A-4).
        if matches!(checked.ty, Ty::Ref { .. }) && !pty.is_copy() {
            return Err(TypeError {
                message: format!(
                    "argument {pname} has type '{}', but this call moves its argument in, and a borrow cannot be moved out of - pass an owned value (a 'Box' holding one moves it out with 'take()'){}",
                    checked.ty.display(),
                    stdlib_instantiation_note(ctx)
                ),
                span: arg_expr.span(),
            });
        }
        if checked.ty != *pty {
            if let Some(why) = closure_kind_mismatch(pty, &checked.ty) {
                return Err(TypeError { message: why, span: arg_expr.span() });
            }
            return Err(TypeError {
                message: format!(
                    "argument {pname} has type '{}', expected '{}'{}{}",
                    checked.ty.display(),
                    pty.display(),
                    char_integer_hint(&checked.ty, pty),
                    str_view_hint(&checked.ty, pty)
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    };

    let checked = check_expr_or_memo(memo, arg_expr, ctx, inner)?;
    refuse_open_arg(&checked.ty, arg_expr)?;
    match &checked.ty {
        t if t == inner.as_ref() => {
            // A *shared* borrow may be taken of a temporary (`str.join(parts
            // = str.split(s, sep), sep = "-")`): the temporary lives for the
            // whole enclosing expression, which is exactly how long a
            // call-scoped borrow lasts, and `codegen_c.rs` hoists it into a C
            // temporary to take its address. An *exclusive* borrow still
            // needs a place to write into, and `is_durable_place` is the
            // question: a temp staged for the call would swallow the write,
            // and a field of one would take the write down with it.
            if *mutable {
                if !is_durable_place(&checked) {
                    return Err(TypeError {
                        message: format!("argument {pname} must be a variable or field, not a temporary value"),
                        span: arg_expr.span(),
                    });
                }
                check_target_writable(&checked, ctx)?;
            }
            let span = checked.span;
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(checked) },
                ty: pty.clone(),
                span,
            })
        }
        Ty::Ref { mutable: a_mut, inner: a_inner } if a_inner == inner => {
            if *mutable && !a_mut {
                note_excl_need(&checked, ctx);
                return Err(TypeError {
                    message: format!("argument {pname} requires an exclusive borrow, found a shared one"),
                    span: arg_expr.span(),
                });
            }
            Ok(checked)
        }
        other => Err(TypeError {
            message: closure_kind_mismatch(pty, other).unwrap_or_else(|| {
                format!(
                    "argument {pname} has type '{}', expected '{}'{}{}",
                    other.display(),
                    pty.display(),
                    char_integer_hint(other, pty),
                    str_view_hint(other, inner)
                )
            }),
            span: arg_expr.span(),
        }),
    }
}

/// The one widening a function value gets (A-4 phase E): a plain `fun` value
/// used where a closure is expected is wrapped, so the two are separate types
/// but one *value* can be written either way. A signature mismatch is left
/// alone - the caller's own type comparison reports it, and a coerced value
/// whose signature does not fit would hide which half was wrong.
fn coerce_fn_to_closure(expected: Ty, value: tir::Expr) -> tir::Expr {
    let Ty::Closure { params, ret } = &expected else {
        return value;
    };
    let Ty::Fn { params: actual_params, ret: actual_ret } = &value.ty else {
        return value;
    };
    if actual_params != params || actual_ret != ret {
        return value;
    }
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::FnToClosure {
            func: Box::new(value),
        },
        ty: expected.clone(),
        span,
    }
}

/// The same widening for a scoped function value: a literal that captured
/// nothing is a plain `fun` pointer, and the `scoped fun(...)` parameter takes
/// the fat pointer, so it is wrapped with `env = NULL` (the shim path) exactly
/// as a plain function value widened into a closure is. Every other scoped
/// value already has the fat-pointer type, because `check_fn_literal` builds a
/// `ScopedClosureLit` for one that captured.
fn coerce_fn_to_scoped(expected: Ty, value: tir::Expr) -> tir::Expr {
    let Ty::ScopedFn { params, ret } = &expected else {
        return value;
    };
    let Ty::Fn { params: actual_params, ret: actual_ret } = &value.ty else {
        return value;
    };
    if actual_params != params || actual_ret != ret {
        return value;
    }
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::FnToClosure {
            func: Box::new(value),
        },
        ty: expected.clone(),
        span,
    }
}

/// The name a `Ty` answers to in the trait-impl table (A-12 slice 4): a
/// `datac`/`enum`'s canonical key, or the written name of a builtin
/// (`i32`, `String`, `AtomicI64`). `None` for a type no trait can be
/// implemented for, which is what makes a coercion from one an error rather
/// than a vtable that names nothing.
fn dyn_target_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Struct(name) => Some(name.clone()),
        Ty::Atomic(elem) => crate::types::atomic_alias_name(elem).map(str::to_string),
        other => builtin_type_name(other).map(str::to_string),
    }
}

/// The one coercion this slice adds (A-12 slice 4, `plan/traits.md` 9.3): a
/// `&T` where `T` implements `Trait` becomes a `&dyn Trait`, and a `&mut T`
/// becomes either spelling. The value is a fat pair, and the coercion records
/// the `(trait, type)` pair so the vtable list knows to emit one.
fn coerce_to_dyn(want: Ty, value: tir::Expr, span: Span, ctx: &Ctx) -> Result<tir::Expr> {
    let Ty::Dyn { mutable: want_mut, trait_name } = &want else {
        return Ok(value);
    };
    if value.ty == want {
        return Ok(value);
    }
    // A `&dyn Name` value retypes to another spelling of the same trait (so a
    // `&mut dyn` forwards to a `&dyn` parameter), but never to a different
    // trait: there is no upcast in this slice.
    if let Ty::Dyn { mutable: src_mut, trait_name: src_trait } = &value.ty {
        if src_trait != trait_name {
            return Err(TypeError {
                message: format!(
                    "expected '{}', found '{}': a trait object does not convert to another trait's ('{}' to '{}'); take a bound on a type parameter, or coerce from the concrete value",
                    want.display(),
                    value.ty.display(),
                    src_trait,
                    trait_name
                ),
                span,
            });
        }
        if *want_mut && !*src_mut {
            return Err(TypeError {
                message: format!(
                    "expected '{}', found '{}': a shared borrow cannot become an exclusive trait object",
                    want.display(),
                    value.ty.display()
                ),
                span,
            });
        }
        let mut retyped = value;
        retyped.ty = want;
        return Ok(retyped);
    }
    let Ty::Ref { mutable: src_mut, inner } = &value.ty else {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': a trait object is built from a borrow, so write '&value' (or '&mut value' for '{want}')",
                want.display(),
                value.ty.display(),
                want = want.display()
            ),
            span,
        });
    };
    if *want_mut && !*src_mut {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': a shared borrow cannot become an exclusive trait object",
                want.display(),
                value.ty.display()
            ),
            span,
        });
    }
    // An `&mut` operand needs a place to write into, exactly as an `&mut`
    // argument of a declared parameter does: a temporary staged for the call
    // would swallow the write, and a field of one would take the write down
    // with it. A value that is already a borrow (a `mut T` parameter read, or
    // a call's result) is a pointer in hand and needs no such place.
    if *src_mut {
        if let tir::ExprKind::Ref { expr: place, .. } = &value.kind {
            if !is_durable_place(place) {
                return Err(TypeError {
                    message: format!(
                        "expected '{}', found '{}': an exclusive borrow needs a variable or field to write through, not a temporary value",
                        want.display(),
                        value.ty.display()
                    ),
                    span,
                });
            }
            check_target_writable(place, ctx)?;
        }
    }
    let Some(type_name) = dyn_target_name(inner) else {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': '{}' cannot be the value behind a trait object",
                want.display(),
                value.ty.display(),
                inner.display()
            ),
            span,
        });
    };
    let implements = ctx
        .env
        .impls
        .get(trait_name)
        .is_some_and(|set| set.contains(&type_name));
    if !implements {
        if blanket_holds(&ctx.env, &trait_name.to_string(), &type_name.to_string()) {
            return Err(blanket_dyn_refusal(&want, &value.ty, &inner, trait_name, span));
        }
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': '{}' does not implement '{}', so write 'impl {} for {}' first",
                want.display(),
                value.ty.display(),
                inner.display(),
                trait_name,
                trait_name,
                inner.display()
            ),
            span,
        });
    }
    ctx.env
        .dyn_pairs
        .borrow_mut()
        .insert((trait_name.clone(), type_name.clone()));
    Ok(tir::Expr {
        kind: tir::ExprKind::DynCoerce {
            trait_name: trait_name.clone(),
            type_name,
            expr: Box::new(value),
        },
        ty: want,
        span,
    })
}

/// The owned half of the coercion (A-12 slice 7): a `Box<T>` where `T`
/// implements `Trait` becomes a `Box<dyn Trait>`. The box's allocation becomes
/// the trait object's, so this is a *move* of the operand (its binding is moved
/// out exactly as any other read of an owned value is) and no second allocation
/// happens. The `(trait, type)` pair is recorded, so the vtable and its drop
/// entry are emitted for the coercion.
fn coerce_box_to_dyn(
    want: Ty,
    value: tir::Expr,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let Ty::BoxDyn { trait_name } = &want else {
        return Ok(value);
    };
    if value.ty == want {
        return Ok(value);
    }
    // A `Box<dyn Name>` retypes to the same type and never to another trait's:
    // there is no upcast in this slice, exactly as for a `&dyn`.
    if let Ty::BoxDyn { trait_name: src_trait } = &value.ty {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': an owned trait object does not convert to another trait's ('{}' to '{}'); take a bound on a type parameter, or coerce from the concrete value",
                want.display(),
                value.ty.display(),
                src_trait,
                trait_name
            ),
            span,
        });
    }
    let Ty::Box(inner) = &value.ty else {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': an owned trait object is built from a box of a value that implements the trait ('Box.of(v)'), and this is not a box",
                want.display(),
                value.ty.display()
            ),
            span,
        });
    };
    let Some(type_name) = dyn_target_name(inner) else {
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': '{}' cannot be the value behind an owned trait object",
                want.display(),
                value.ty.display(),
                inner.display()
            ),
            span,
        });
    };
    let implements = ctx
        .env
        .impls
        .get(trait_name)
        .is_some_and(|set| set.contains(&type_name));
    if !implements {
        if blanket_holds(&ctx.env, &trait_name.to_string(), &type_name.to_string()) {
            return Err(blanket_dyn_refusal(&want, &value.ty, &inner, trait_name, span));
        }
        return Err(TypeError {
            message: format!(
                "expected '{}', found '{}': '{}' does not implement '{}', so write 'impl {} for {}' first",
                want.display(),
                value.ty.display(),
                inner.display(),
                trait_name,
                trait_name,
                inner.display()
            ),
            span,
        });
    }
    ctx.env
        .dyn_pairs
        .borrow_mut()
        .insert((trait_name.clone(), type_name.clone()));
    Ok(tir::Expr {
        kind: tir::ExprKind::BoxDynCoerce {
            trait_name: trait_name.clone(),
            type_name,
            expr: Box::new(value),
        },
        ty: want,
        span,
    })
}

/// The one sentence that explains why a plain function value and a closure are
/// different types, when one is passed where the other is expected (A-4 phase
/// E). The two are the same shape of value and are called identically, so a
/// bare `expected 'closure(i32).return: i32', found 'fun(i32).return: i32'` would leave
/// the reader to guess which half is missing. The expected side is looked
/// through a borrow, because a parameter of a closure type is one.
fn closure_kind_mismatch(expected: &Ty, actual: &Ty) -> Option<String> {
    let expected = match expected {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    match (expected, actual) {
        (Ty::Closure { .. }, Ty::Fn { .. }) => Some(
            "this is a plain function value, not a closure: its body captured nothing, so it carries no environment - a `fun` literal becomes a closure when it names a binding from the enclosing scope, and a declared `fun` is always a plain function value".to_string(),
        ),
        (Ty::Fn { .. }, Ty::Closure { .. }) => Some(
            "this is a closure: its body captured a binding from the enclosing scope, so it carries an environment a plain function value has no room for - write the expected type as 'closure(...)' (the same parameter list and '.return:'), or pass a declared function instead".to_string(),
        ),
        (Ty::Fn { .. } | Ty::Closure { .. }, Ty::ScopedFn { .. }) => Some(
            "this is a scoped function value: it may only be passed to another 'scoped fun(...)' parameter, because it may not outlive the call it was written in - a plain function or closure parameter could store it or return it".to_string(),
        ),
        (Ty::ScopedFn { .. }, Ty::Fn { .. } | Ty::Closure { .. }) => Some(
            "this parameter takes a plain function or closure value, and what was passed is a scoped one: a scoped literal may only be written at the call site of a 'scoped fun(...)' parameter".to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stages B1 and C1: `thread.spawn` / `thread.join`. B1 landed the
// checker surface and the rules of MK9 section 5; C1 lowers both, so the two
// calls below build the `tir` node each backend translates into a real worker.
// ---------------------------------------------------------------------------

/// One of the two thread calls, dispatched on the key `check_call` matched.
fn check_thread_call(
    callee: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if callee == crate::stdlib::SPAWN_FN {
        check_thread_spawn(args, span, ctx, expected)
    } else {
        check_thread_join(args, span, ctx, expected)
    }
}

/// `thread.spawn(entry, arg)` (MK9-S3-M1 stages B1 and C1). `entry` is a
/// declared `fun`, a `fun` literal, or a capturing closure, taking exactly one
/// parameter; `arg` is the value moved into the worker as that parameter, and
/// the handle's element is `entry`'s **return** type, which is the value the
/// worker hands its joiner (MK9 section 4).
///
/// The refusals are MK9 section 5's rules 1 to 4, each naming its own reason.
/// The accepted path is a `tir::Call` under this module's own key, which is what
/// both backends recognize to emit the worker and the trampoline that calls it.
fn check_thread_spawn(
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots(
        "function 'thread.spawn'",
        &["entry".to_string(), "arg".to_string()],
        args,
        span,
    )?;
    let entry_arg = &args[slots[0]];
    let value_arg = &args[slots[1]];

    let entry = check_expr(&entry_arg.value, ctx, None)?;
    let (params, ret) = match &entry.ty {
        Ty::Fn { params, ret } | Ty::Closure { params, ret } => (params.clone(), (**ret).clone()),
        other => {
            return Err(TypeError {
                message: format!(
                    "'thread.spawn' takes a function or closure value as its entry point, found '{}': a declared 'fun', a 'fun' literal and a capturing closure all work, because a function value is widened where a closure is expected",
                    other.display()
                ),
                span: entry_arg.value.span(),
            })
        }
    };
    // One parameter, because the worker is handed exactly one value: the `arg`
    // moved in as that parameter (MK9 section 5 rule 1).
    if params.len() != 1 {
        return Err(TypeError {
            message: format!(
                "'thread.spawn''s entry point has to take exactly one parameter, the value moved into the worker, and this one takes {}",
                params.len()
            ),
            span: entry_arg.value.span(),
        });
    }
    let moved = params[0].clone();
    // A plain function value is widened into a closure here rather than at each
    // backend, so `entry` has one shape downstream: the ordinary A-4 phase E
    // widening, a `fun` value wrapped in the one-field environment a closure's
    // code pointer is called with.
    let entry = coerce_fn_to_closure(
        Ty::Closure {
            params: params.clone(),
            ret: Box::new(ret.clone()),
        },
        entry,
    );

    let value = check_expr(&value_arg.value, ctx, Some(&moved))?;
    check_thread_arg(&value_arg.value, &value, ctx)?;
    if value.ty != moved {
        return Err(TypeError {
            message: format!(
                "'thread.spawn''s argument has type '{}', and its entry point takes '{}' by move: the worker's parameter is the value moved in, so the two have to be the same type",
                value.ty.display(),
                moved.display()
            ),
            span: value_arg.value.span(),
        });
    }

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: crate::stdlib::SPAWN_FN.to_string(),
            args: vec![entry, value],
        },
        ty: Ty::Thread(Box::new(ret)),
        span,
    })
}

/// `thread.join(handle)` (MK9-S3-M1 stages B1 and C1). The handle is a
/// `Thread<T>`, the joiner consumes it, and the result is
/// `Result<T, ThreadErr>`: `Ok` the worker's value, `Err` why it ended without
/// one (MK9 section 2).
fn check_thread_join(
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots(
        "function 'thread.join'",
        &["handle".to_string()],
        args,
        span,
    )?;
    let handle_arg = &args[slots[0]];
    let handle = check_expr(&handle_arg.value, ctx, None)?;
    let elem = match &handle.ty {
        Ty::Thread(elem) => (**elem).clone(),
        other => {
            return Err(TypeError {
                message: format!(
                    "'thread.join' takes a 'Thread<T>' handle, found '{}': a handle comes from 'thread.spawn', which is the only call that makes one",
                    other.display()
                ),
                span: handle_arg.value.span(),
            })
        }
    };

    // The result names the shipped `ThreadErr` through this module's key
    // exactly as the signature states, and a program that never imported
    // `std/result` is told to rather than given an internal error.
    let result_key = format!(
        "{}Result",
        crate::stdlib::key_prefix(crate::stdlib::RESULT_MODULE)
    );
    if !ctx.env.generics.contains_key(&result_key) {
        return Err(TypeError {
            message: format!(
                "'thread.join' hands back 'Result<T, ThreadErr>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                result_key,
                crate::stdlib::RESULT_MODULE
            ),
            span,
        });
    }
    let joined = ctx.instantiate_type(&result_key, &[elem, thread_err_ty()], span)?;

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: crate::stdlib::JOIN_FN.to_string(),
            args: vec![handle],
        },
        ty: joined,
        span,
    })
}

/// MK9 section 5's argument rules, applied to `thread.spawn`'s moved value.
/// The rule is structural and builtin (rule 5): there is no `Send`/`Sync` marker
/// to implement until traits ship (A-12), so this lives beside the existing
/// "a borrowed type cannot be stored" rule rather than in a trait.
///
/// `expr` is the argument as written, because rules 2 and 3 are about the
/// *place* a borrow comes from and that is a lexical question; `value` is it
/// checked, because the borrow's shape is its type.
fn check_thread_arg(expr: &ast::Expr, value: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &value.ty {
        // Rule 1: an exclusive borrow never crosses. A `&mut T` is the promise
        // that no other path reaches the value, and the worker outlives the
        // frame the borrow was made in, so the promise cannot hold.
        Ty::Ref { mutable: true, inner } => Err(TypeError {
            message: format!(
                "'thread.spawn' cannot send '&mut {}': an exclusive borrow promises that no other path reaches the value, and the worker outlives the frame that made the borrow, so the promise cannot cross the thread boundary; move an owned value in, or share a '&Atomic<T>'",
                inner.display()
            ),
            span: expr.span(),
        }),
        // Rules 2 and 3: a shared borrow crosses only when it borrows a `pd`
        // global (process lifetime, so it outlives the worker) and only when
        // the borrowed type has no reachable `&mut` path.
        Ty::Ref { mutable: false, inner } => {
            if !borrows_a_pd(expr, ctx) {
                return Err(TypeError {
                    message: format!(
                        "'thread.spawn' cannot send '{}': a shared borrow may cross to a worker only when the place it borrows has process lifetime, which in v1 means a 'pd' global, and this one borrows a frame local that ends before the worker does; share a 'pd' value instead, or move an owned value in",
                        value.ty.display()
                    ),
                    span: expr.span(),
                });
            }
            if has_reachable_mut_path(inner, ctx) {
                return Err(TypeError {
                    message: format!(
                        "'thread.spawn' cannot share '{}' across a thread boundary: the borrow reaches a 'var' field, so two threads could write it through the same shared borrow. A shared borrow may cross only when its pointee has no reachable '&mut' path, which an 'Atomic<T>' has by construction",
                        inner.display()
                    ),
                    span: expr.span(),
                });
            }
            Ok(())
        }
        // Rule 2's other shape: a borrowed trait object is a borrow of the
        // value its data pointer addresses, exactly as a `Ty::Ref` is, so it
        // never crosses. An *owned* trait object (`Box<dyn Trait>`) is not this
        // arm: it owns its value and falls through to rule 4 like any other
        // owned value (A-12 slice 7).
        Ty::Dyn { trait_name, .. } => Err(TypeError {
            message: format!(
                "'thread.spawn' cannot send '&dyn {trait_name}': a borrowed trait object points at a value the frame owns, and the worker outlives that frame; move an owned 'Box<dyn {trait_name}>' in instead, or share a 'pd' value"
            ),
            span: expr.span(),
        }),
        // Rule 2's other shapes: a `str` and a `View` are borrowed text with
        // no place of their own, so there is nothing with process lifetime for
        // them to borrow.
        Ty::Str | Ty::View => Err(TypeError {
            message: format!(
                "'thread.spawn' cannot send a '{}': it is a borrowed view of text somebody else owns, and the worker outlives the frame that owns the text; pass an owned 'String' or 'Bytes' instead, or share a 'pd' value",
                value.ty.display()
            ),
            span: expr.span(),
        }),
        // Rule 4 falls out of `move T`: an owned value that carried a borrow
        // could not be constructed in the first place (`is_borrowed_field_ty`
        // refuses a borrow, a `str` or a `View` in a `datac` field, an enum
        // payload, a `List` element or a `Map` value), and the top-level
        // borrow shapes are the three arms above. There is no hole to open.
        _ => Ok(()),
    }
}

/// Whether the expression that made a shared borrow borrows a `pd` global,
/// which is the one place MK9 section 5 rule 2 lets a borrow cross to a worker:
/// a `pd`'s lifetime is the process, so it outlives the worker.
///
/// Checked over the place expression as written, which is what makes the rule
/// decidable: the root identifier has to be a passdown of this module and not
/// shadowed by a local of the same name.
fn borrows_a_pd(expr: &ast::Expr, ctx: &Ctx) -> bool {
    let ast::Expr::Ref { expr: place, .. } = expr else {
        return false;
    };
    matches!(place_root_ident(place), Some(name) if ctx.pds.contains_key(name) && ctx.lookup(name).is_none())
}

/// The identifier a place expression is rooted at: `counter`, `counter.field`
/// and `counter.a.b` all answer `counter`. `None` for anything else, a
/// dereference included, whose storage this lexical check does not follow.
fn place_root_ident(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Ident(name, _) => Some(name),
        // The one field spelling a place is written with: `p.x` parses as a
        // `FieldAccess`. There is no other way to write a place.
        ast::Expr::FieldAccess { base, .. } => place_root_ident(base),
        _ => None,
    }
}

/// Whether a shared borrow of `ty` reaches a `var` field, which is exactly the
/// `Sync`-shaped question MK9 section 5 rule 3 asks: a type with no reachable
/// `&mut` path may be shared across a thread boundary, and one with a reachable
/// `var` field may not. An `Atomic<T>` answers no by construction, which is what
/// makes a `&Atomic<T>` the shareable type v1 ships.
fn has_reachable_mut_path(ty: &Ty, ctx: &Ctx) -> bool {
    let mut seen = std::collections::HashSet::new();
    reachable_mut_path(ty, ctx, &mut seen)
}

fn reachable_mut_path(
    ty: &Ty,
    ctx: &Ctx,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    match ty {
        // A `datac` field and an `enum` payload are both reachable by a field
        // or a pattern access, so either one being `var` is a `&mut` path. The
        // walk recurses through a field whatever the field's own mutability:
        // `a.b.c` is reachable from `a` when `c` is `var`, and section 5 asks
        // about reachability rather than about the first step.
        Ty::Struct(name) => {
            if !seen.insert(name.clone()) {
                return false;
            }
            if let Some(def) = ctx.struct_def(name) {
                return def
                    .fields
                    .iter()
                    .any(|f| f.mutable || reachable_mut_path(&f.ty, ctx, seen));
            }
            if let Some(def) = ctx.enum_def(name) {
                return def
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| f.mutable || reachable_mut_path(&f.ty, ctx, seen));
            }
            false
        }
        // A container's element is reached through the container's own access,
        // so a `var` field under one is just as reachable.
        Ty::List(elem) | Ty::Box(elem) | Ty::Array(elem, _) => {
            reachable_mut_path(elem, ctx, seen)
        }
        Ty::Map(_, value) => reachable_mut_path(value, ctx, seen),
        // An atomic mutates through a shared borrow and has no `&mut` path at
        // all, which is the property the whole rule is built on (MK9 section 4).
        Ty::Atomic(_) => false,
        // A `Mutex<T>` has no reachable path to `T` at all outside the scoped
        // `lock` block, so it is shareable across a thread boundary whatever
        // its element: two threads can only take turns through the lock
        // (B-5).
        Ty::Mutex(_) => false,
        // An `RwLock<T>` reaches `T` only through its scoped `read`/`write`
        // blocks, so it is shareable across a thread boundary whatever its
        // element, exactly as `Mutex<T>` is (B-5). A `Condvar` holds no `T` at
        // all and is shared by construction.
        Ty::RwLock(_) => false,
        Ty::Condvar => false,
        _ => false,
    }
}

/// A compile-time layout query (kernel stage 2): `size_of<T>()`,
/// `align_of<T>()` and `offset_of<T>("field")`. Each folds to a `usize`
/// literal here, so both backends see an ordinary integer and no code
/// generator has anything layout-specific to lower. `T` is a scalar, an
/// opaque handle, a `Ptr<U>`, a `datac` or an `enum`: the types that have a
/// machine-level layout and no heap representation to reason about.
fn check_layout_query(
    callee: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &Ctx,
) -> Result<tir::Expr> {
    if explicit.len() != 1 {
        return Err(TypeError {
            message: format!(
                "'{callee}' takes exactly one type argument, written '{callee}<T>()'"
            ),
            span,
        });
    }
    let field = if callee == "offset_of" {
        if args.len() != 1 {
            return Err(TypeError {
                message: "'offset_of' takes exactly one field name, written 'offset_of<T>(\"field\")'"
                    .to_string(),
                span,
            });
        }
        match &args[0].value {
            ast::Expr::StringLit(field, _) if args[0].name.is_none() => Some(field.as_str()),
            _ => {
                return Err(TypeError {
                    message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                })
            }
        }
    } else {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'{callee}' takes no arguments, found {}", args.len()),
                span,
            });
        }
        None
    };
    let value = layout_query_value(
        callee,
        &explicit[0],
        field,
        &ctx.structs,
        &ctx.enums,
        span,
    )?;
    Ok(tir::Expr {
        kind: tir::ExprKind::IntLit(value as i128),
        ty: Ty::Usize,
        span,
    })
}

/// The value one layout query folds to, shared by the expression checker and
/// the compile-time evaluator so both refuse the same things with the same
/// words.
fn layout_query_value(
    callee: &str,
    t: &Ty,
    field: Option<&str>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    span: Span,
) -> Result<u64> {
    let allowed = matches!(
        t,
        Ty::U8
            | Ty::U16
            | Ty::I32
            | Ty::I64
            | Ty::U32
            | Ty::U64
            | Ty::Usize
            | Ty::Isize
            | Ty::F32
            | Ty::F64
            | Ty::Bool
            | Ty::Char
            | Ty::Handle(_)
            | Ty::Ptr(_)
            | Ty::Struct(_)
    );
    if !allowed {
        return Err(TypeError {
            message: format!(
                "'{callee}' cannot be asked about '{}': the queried type is a scalar, an 'extern type' handle, a 'Ptr<U>', a 'datac' or an 'enum'"
                , t.display()
            ),
            span,
        });
    }
    match callee {
        "size_of" | "align_of" => {
            let l = crate::types::type_layout(t, structs, enums).ok_or_else(|| TypeError {
                message: format!("'{}' has no layout this compiler can compute", t.display()),
                span,
            })?;
            Ok(if callee == "size_of" { l.size } else { l.align })
        }
        "offset_of" => {
            let Ty::Struct(name) = t else {
                return Err(TypeError {
                    message: format!("'offset_of' needs a 'datac': '{}' is not one", t.display()),
                    span,
                });
            };
            let Some(field) = field else {
                return Err(TypeError {
                    message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                });
            };
            crate::types::field_offset(name, field, structs, enums).ok_or_else(|| TypeError {
                message: format!("'{}' has no field '{field}'", t.display()),
                span,
            })
        }
        _ => unreachable!("only the three layout queries reach here"),
    }
}

/// The layout rules an enum's discriminants and `repr` have to obey (kernel
/// stage 2): `repr(packed)`/`repr(align(N))` are not an enum's to ask for, a
/// written discriminant is legal only on a field-less enum, the resolved tag
/// values are unique, and each fits the tag type `repr(<int>)` names.
fn validate_enum_layout(
    e: &EnumDef,
    span: Span,
    module: &str,
) -> std::result::Result<(), Diagnostic> {
    if let Repr::Packed | Repr::Align(_) = &e.repr {
        return Err(Diagnostic::new(
            "'repr(packed)' and 'repr(align(N))' are not supported on an enum: an enum's layout knob is the tag's integer type, written 'repr(u8)', 'repr(i16)', ..."
                .to_string(),
            span,
            module.to_string(),
        ));
    }
    if !e.variants.iter().any(|v| v.discriminant.is_some()) {
        return Ok(());
    }
    if !e.is_field_less() {
        return Err(Diagnostic::new(
            format!(
                "enum '{}' has a variant with a payload, so no variant may have an explicit discriminant: a discriminant is a value with no data beside it, so it is written only on a field-less enum",
                e.name
            ),
            span,
            module.to_string(),
        ));
    }
    let (min, max): (i128, i128) = match &e.repr {
        Repr::Int(name) => match name.as_str() {
            "u8" => (0, u8::MAX as i128),
            "u16" => (0, u16::MAX as i128),
            "u32" => (0, u32::MAX as i128),
            "u64" => (0, u64::MAX as i128),
            "i8" => (i8::MIN as i128, i8::MAX as i128),
            "i16" => (i16::MIN as i128, i16::MAX as i128),
            "i32" => (i32::MIN as i128, i32::MAX as i128),
            _ => (i64::MIN as i128, i64::MAX as i128),
        },
        _ => (i32::MIN as i128, i32::MAX as i128),
    };
    let values = e.discriminants();
    for (i, value) in values.iter().enumerate() {
        let holder = e
            .variants
            .iter()
            .find(|v| v.discriminant == Some(*value))
            .map(|v| v.name.clone())
            .unwrap_or_else(|| e.variants[i].name.clone());
        if values[..i].contains(value) {
            return Err(Diagnostic::new(
                format!(
                    "variant '{holder}' of enum '{}' repeats the discriminant {value}: every variant of a field-less enum has its own value",
                    e.name
                ),
                span,
                module.to_string(),
            ));
        }
        if *value < min || *value > max {
            let tag = e.tag_ty().display();
            return Err(Diagnostic::new(
                format!(
                    "variant '{holder}' of enum '{}' has discriminant {value}, which does not fit its tag type '{tag}'"
                    , e.name
                ),
                span,
                module.to_string(),
            ));
        }
    }
    Ok(())
}

fn check_call(
    callee: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // An explicit instantiation, `Pair<i32, String>(...)` (A-2). Resolved
    // once here and handed to whichever path consumes it; every other path
    // refuses it, because a call that is not generic has nothing to
    // instantiate.
    let explicit = resolve_explicit_args(type_args, ctx)?;
    // The compile-time layout queries (kernel stage 2). They are resolved
    // before the program's own declarations, like `print` and `panic`, so a
    // program cannot shadow one and get a different `size_of` than the ABI.
    if matches!(callee, "size_of" | "align_of" | "offset_of") && !callee.contains('.') {
        return check_layout_query(callee, &explicit, args, span, ctx);
    }
    // The per-CPU intrinsics (kernel stage 8). `cpu_count()` folds to the
    // manifest's `[target] cpus`; `cpu_index()` stays a call, read from the
    // register the kernel set at boot. Both are freestanding-only and take no
    // arguments, and both are resolved before the program's own declarations
    // for the same reason the layout queries are.
    if matches!(callee, "cpu_index" | "cpu_count") && !callee.contains('.') {
        if !ctx.freestanding {
            return Err(TypeError {
                message: format!(
                    "'{callee}' is only available in a freestanding build: it reads the CPU index register the kernel sets at boot"
                ),
                span,
            });
        }
        if !explicit.is_empty() {
            return Err(TypeError {
                message: format!("'{callee}' takes no type arguments"),
                span,
            });
        }
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'{callee}' takes no arguments, found {}", args.len()),
                span,
            });
        }
        let kind = if callee == "cpu_count" {
            tir::ExprKind::IntLit(i128::from(ctx.cpus))
        } else {
            tir::ExprKind::Call {
                callee: "cpu_index".to_string(),
                args: Vec::new(),
            }
        };
        return Ok(tir::Expr {
            kind,
            ty: Ty::Usize,
            span,
        });
    }
    if callee == "main" {
        // `main` is the program entry point; it takes no arguments, so there
        // is no way to hand it the passdowns it may reference, and its own
        // `argv` is reached through `process.args()` instead.
        return Err(TypeError {
            message: "cannot call 'main': it is the program entry point".to_string(),
            span,
        });
    }
    if PRINTABLE_BUILTINS.contains(&callee) {
        if args.len() != 1 {
            return Err(TypeError {
                message: format!("'{callee}' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        if args[0].name.is_some() {
            return Err(TypeError {
                message: format!("'{callee}' does not take named arguments"),
                span,
            });
        }
        // A template becomes one `printf` (see `tir::ExprKind::Template`)
        // rather than being checked as a `str` value, which v1 can't build.
        if let ast::Expr::Template { parts, span: tspan } = &args[0].value {
            return check_template(parts, callee == "print", *tspan, ctx);
        }
        let arg = check_expr(&args[0].value, ctx, None)?;
        let arg = borrow_for_print(arg, callee, ctx)?;
        // An opaque handle has no readable value, so there is nothing to
        // format: name it rather than fall through to the generic refusal.
        if let Ty::Handle(name) = &arg.ty {
            return Err(TypeError {
                message: format!(
                    "cannot pass a value of type '{name}' to '{callee}': an 'extern type' is an opaque native handle with no printable value"
                ),
                span,
            });
        }
        let printable = arg.ty.is_numeric()
            || matches!(arg.ty, Ty::Bool | Ty::Char)
            || is_str_like(&arg.ty)
            || is_printable_buffer(&arg.ty);
        if !printable {
            // A `datac`/`enum` that implements `Display` prints through
            // `x.to_text()`, which yields the owned `String` the ordinary
            // print path already formats (`plan/traits.md` 13.4).
            if let Some(text) = lower_display(&arg, span, ctx)? {
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Call {
                        callee: callee.to_string(),
                        args: vec![text],
                    },
                    ty: Ty::Unit,
                    span,
                });
            }
            return Err(TypeError {
                message: print_error(callee, &arg.ty),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call {
                callee: callee.to_string(),
                args: vec![arg],
            },
            ty: Ty::Unit,
            span,
        });
    }

    // `panic` takes a message, and the owned spelling of one now sits beside
    // the borrowed `str` every call site already had: a message built as a
    // `String` (`fmt.int(n)`, `str.concat`) no longer has to be bound to a
    // `val` or spelled `.as_str()` first, which is exactly the ceremony an
    // error path does not want. The buffer is read, never consumed, so this is
    // `print`'s lifetime rule and `print`'s temporary handling, and a message
    // that is neither a `str` nor an owned `String` is still refused by name.
    if callee == "panic" {
        if args.len() != 1 {
            return Err(TypeError {
                message: format!("'panic' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        if args[0].name.is_some() {
            return Err(TypeError {
                message: "'panic' does not take named arguments".to_string(),
                span,
            });
        }
        let arg = check_expr(&args[0].value, ctx, None)?;
        let arg = borrow_for_print(arg, callee, ctx)?;
        if !is_panic_message(&arg.ty) {
            return Err(TypeError {
                message: format!(
                    "cannot pass a value of type '{}' to 'panic': a panic message is a 'str' or an owned 'String'",
                    arg.ty.display()
                ),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call {
                callee: callee.to_string(),
                args: vec![arg],
            },
            ty: Ty::Unit,
            span,
        });
    }

    // A-2: construction of a generic `datac`: `Pair(first = 1, second = "x")`.
    // The type arguments are inferred from the argument types (and, failing
    // that, from the expected type), so a generic value is constructed with
    // exactly the same syntax as an ordinary one.
    if let Some(tmpl_name) = generic_template_name(callee, ctx) {
        return check_generic_datac_construction(&tmpl_name, &explicit, args, span, ctx, expected);
    }
    // An explicit instantiation is only meaningful for a generic type or a
    // generic function; every other callee here (a concrete `datac`'s
    // construction, a plain function, a builtin) has nothing to instantiate.
    if !explicit.is_empty()
        && func_template_named(ctx, callee, &ctx.qualify_own(callee)).is_none()
    {
        return Err(TypeError {
            message: format!("'{callee}' does not take type arguments"),
            span,
        });
    }

    // `Shape(r = 1.0)` - an enum is constructed by naming a variant, never by
    // calling the type itself.
    if ctx.is_enum(callee) || ctx.is_enum(&ctx.qualify_own(callee)) {
        let ename = if ctx.is_enum(callee) {
            callee.to_string()
        } else {
            ctx.qualify_own(callee)
        };
        let variants: Vec<String> = ctx
            .enum_def(&ename)
            .expect("is_enum only returns known enums")
            .variants
            .iter()
            .map(|v| format!("'{ename}.{}'", v.name))
            .collect();
        return Err(TypeError {
            message: format!(
                "'{ename}' is an enum - construct one of its variants: {}",
                variants.join(", ")
            ),
            span,
        });
    }
    if let Some(sdef) = ctx
        .struct_def(callee)
        .or_else(|| ctx.struct_def(&ctx.qualify_own(callee)))
    {
        let field_names: Vec<&str> = sdef.fields.iter().map(|f| f.name.as_str()).collect();
        if let Some(domains) = crate::stdlib::handle_ctor_domains(&sdef.name, &field_names) {
            // A module inside the library builds its own handles; any other
            // module needs a domain that mints this kind (threat model H1).
            let inside = !ctx.prefix.is_empty() && sdef.name.starts_with(ctx.prefix.as_str());
            let ctx_is_shipped = is_shipped_module(&ctx.module);
            if !inside && !ctx_is_shipped && !domains.iter().any(|d| ctx.caps.contains_key(*d)) {
                return Err(TypeError {
                    message: format!(
                        "'{}' wraps an operating-system descriptor, so building one from a number needs the '{}' capability, which module '{}' was not granted{}; a module that holds no grant can only use the handles it is given",
                        display_callee(&sdef.name),
                        domains[0],
                        ctx.module,
                        if domains.len() > 1 {
                            format!(" (any of {} would do)", domains.join(", "))
                        } else {
                            String::new()
                        }
                    ),
                    span,
                });
            }
        }
        let params: Vec<(String, Ty)> = sdef.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
        let checked = match_args(&format!("datac '{}'", sdef.name), &params, args, span, ctx)?;
        let borrowed_fields = aggregate_borrowed_fields(&sdef.fields, &checked, span)?;
        let fields = sdef
            .fields
            .iter()
            .zip(checked)
            .map(|(f, e)| (f.name.clone(), e))
            .collect();
        return Ok(tir::Expr {
            kind: tir::ExprKind::StructLit {
                name: sdef.name.clone(),
                fields,
                borrowed_fields,
            },
            ty: Ty::Struct(sdef.name.clone()),
            span,
        });
    }

    // The *calling* module's own spelling first: a bare name inside a module is
    // that module's item (the plan's strict module-qualified namespace), so a
    // name that happens to exist in the entry module must not win here. For
    // the entry module the two keys are the same, and a qualified callee
    // (`check_module_item` passes `std.math.abs`) misses the first and hits the
    // second, so neither case changes.
    //
    // The lookup on the name as written is for a callee that is *already*
    // canonical, and a canonical key always carries its module prefix. The
    // entry module is the only one whose keys are bare, so an ungated fallback
    // here would let every module call the entry module's item by its bare
    // name, `pub` or not (A-13).
    let names_a_module = callee.contains('.');
    let owned_key = ctx.qualify_own(callee);
    let direct = ctx.sig_of(&owned_key).map(|s| (owned_key.clone(), s)).or_else(|| {
        if !names_a_module {
            return None;
        }
        ctx.sig_of(callee).map(|s| (callee.to_string(), s))
    });
    let (key, sig) = match direct {
        Some(found) => found,
        None => match resolve_overload(callee, &owned_key, args, span, ctx)? {
            Some(found) => found,
            // A-2: a generic function is a template, not a signature, so it is
            // instantiated for this call site's argument types before the
            // ordinary argument matching runs.
            None => match func_template_named(ctx, callee, &owned_key) {
                Some(tmpl_key) => {
                    instantiate_function(ctx, &tmpl_key, &explicit, args, span, expected, None)?
                }
                // A local holding a function value (A-4): `f(x)`, where `f`
                // came from `val f = double`. Checked after every declared
                // function, so a call that names a real function is never
                // diverted by a binding of the same name.
                None => match indirect_callee(callee, span, ctx)? {
                    Some((callee_expr, fn_ty)) => {
                        return check_indirect_call_expr(
                            callee_expr,
                            &fn_ty,
                            args,
                            span,
                            ctx,
                            &format!("'{callee}'"),
                        )
                    }
                    // A compiler-provided free function (`panic`, `assert`)
                    // lives in the same table as the builtin methods, so it is
                    // consulted after the program's own signatures - a user
                    // function of the same name wins, and there is no
                    // shadowing surprise.
                    None => match builtin_sig(callee) {
                    Some(sig) => (callee.to_string(), sig),
                    // `thread.spawn` and `thread.join` (MK9-S3-M1 stage B1),
                    // whose signatures depend on the value that crosses the
                    // boundary and so live outside the table. Checked here, with
                    // the other builtin free functions, so a declaration of the
                    // same key still wins.
                    None if crate::stdlib::is_thread_fn(callee) => {
                        return check_thread_call(callee, args, span, ctx, expected)
                    }
                    None => {
                        return Err(TypeError {
                            message: match qualified_hint(
                                callee,
                                ctx.structs
                                    .keys()
                                    .chain(ctx.sigs.keys())
                                    .chain(ctx.fns.templates.keys()),
                                &ctx.aliases,
                                &ctx.env,
                            ) {
                                Some(hint) => format!("call to undefined function '{callee}' - did you mean {hint}?"),
                                None => format!("call to undefined function '{callee}'"),
                            },
                            span,
                        })
                    }
                },
                },
            },
        },
    };
    ctx.check_capability(&key, span)?;
    // A function declared `unsafe fun` (MK9-S2 stage 1) is callable only from
    // an `unsafe { ... }` block or from another `unsafe fun`; the refusal names
    // the function so the reader knows which call in the statement it was.
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            message: format!(
                "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                display_callee(&key)
            ),
            span,
        });
    }
    if let Some(probe) = &ctx.pd_probe {
        // The call graph the borrow-mode closure walks (roadmap item 17):
        // whoever calls a writer writes too, when it holds the passdown
        // itself. Both ends are filed under their wiring key, so an
        // instantiation's edges are its template's.
        let callee_key = ctx.wiring_key_of(&key);
        probe
            .borrow_mut()
            .edges
            .insert((ctx.fn_key.clone(), callee_key));
    }
    // A user `extern fun` whose last parameter is `...` (gap 7) accepts any
    // number of extra arguments after the declared ones. Each extra argument
    // is checked here: its type has to be one a native `va_arg` can read back
    // (an integer, `bool`, `f32`/`f64`, `str` or an opaque handle), with no
    // implicit conversion, and the call names one generated fixed-arity C
    // wrapper that forwards the fixed and the extra arguments to the native
    // symbol. The wrapper is created once per distinct list of extra types,
    // so two call sites that pass the same types share one.
    let variadic = ctx.variadic_externs.contains_key(&key);
    let what = format!("function '{}'", display_callee(&key));
    let (mut checked, extra_args) = if variadic {
        match_args_split(&what, &sig.params, args, span, ctx, true)?
    } else {
        (match_args(&what, &sig.params, args, span, ctx)?, Vec::new())
    };
    require_place_for_borrowed_arg(&sig.params, &sig.ret, &checked, span)?;
    let wrapper = if variadic {
        let mut extra_tys = Vec::with_capacity(extra_args.len());
        let mut extra_exprs = Vec::with_capacity(extra_args.len());
        for (i, a) in extra_args.iter().enumerate() {
            let extra = check_expr(&a.value, ctx, None)?;
            if !crate::types::is_variadic_abi_ty(&extra.ty) {
                return Err(TypeError {
                    message: format!(
                        "extra argument {} of '{}' has type '{}', which a variadic 'extern fun' cannot pass: use an integer, 'bool', 'f32', 'f64', 'str' or an opaque handle",
                        i + 1,
                        display_callee(&key),
                        extra.ty.display()
                    ),
                    span: a.value.span(),
                });
            }
            extra_tys.push(extra.ty.clone());
            extra_exprs.push(extra);
        }
        checked.extend(extra_exprs);
        Some(register_variadic_wrapper(ctx, &key, &sig, &extra_tys, span))
    } else {
        None
    };
    checked.extend(hidden_args(&key, span, ctx)?);
    check_claim_argument(&key, &checked, span, ctx)?;

    Ok(tir::Expr {
        kind: match wrapper {
            Some(wrapper) => tir::ExprKind::VariadicCall {
                callee: key,
                wrapper,
                args: checked,
            },
            None => tir::ExprKind::Call {
                callee: key,
                args: checked,
            },
        },
        ty: sig.ret,
        span,
    })
}

fn aggregate_borrowed_fields(
    fields: &[FieldDef],
    values: &[tir::Expr],
    span: Span,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (i, (field, value)) in fields.iter().zip(values).enumerate() {
        if !matches!(field.ty, Ty::Str | Ty::View) {
            continue;
        }
        if matches!(value.kind, tir::ExprKind::StringLit(_)) {
            continue;
        }
        let Some(source) = aggregate_borrow_source(value) else {
            return Err(TypeError { message: format!("field '{}' stores a borrowed '{}', so construct it from a string literal or from an earlier String/Bytes field in this datac", field.name, field.ty.display()), span });
        };
        let owns_source = fields[..i].iter().zip(&values[..i]).any(|(owner, value)| {
            matches!(owner.ty, Ty::String | Ty::Bytes)
                && matches!(&value.kind, tir::ExprKind::Ident(name) if name == source)
        });
        if !owns_source {
            return Err(TypeError { message: format!("field '{}' borrows '{source}', but that value is not moved into an earlier String/Bytes field of this datac", field.name), span });
        }
        out.push(field.name.clone());
    }
    Ok(out)
}

fn aggregate_borrow_source(value: &tir::Expr) -> Option<&str> {
    let tir::ExprKind::Call { args, .. } = &value.kind else { return None; };
    let tir::ExprKind::Ref { expr, .. } = &args.first()?.kind else { return None; };
    let tir::ExprKind::Ident(name) = &expr.kind else { return None; };
    Some(name)
}

/// The gated resource constructors (`irq.claim`, `frame.claim`, kernel stage
/// 18): the call is the capability's gate, so the argument is checked here,
/// where both backends and both the hosted and the freestanding modes pass.
/// The domain gate already ran (`check_capability`), so the module holds the
/// domain; what is left is the value.
///
/// A constant argument has to fall inside a granted range, and a `frame`
/// address has to be 4 KiB-aligned whatever the grant. A non-constant argument
/// is admitted only under the whole-resource `["*"]`, because the compiler
/// cannot prove a runtime value lands in a range; the same rule the `mmio`
/// address check applies (`plan/device-caps.md`, `plan/irq-frame-caps.md`).
fn check_claim_argument(
    key: &str,
    args: &[tir::Expr],
    span: Span,
    ctx: &Ctx,
) -> Result<()> {
    let Some(domain) = crate::stdlib::claim_domain(key) else {
        return Ok(());
    };
    let patterns = ctx.caps.get(domain).cloned().unwrap_or_default();
    let value = args.first().and_then(crate::stdlib::const_int);
    // The alignment rule holds for a constant whatever the grant, including
    // the wildcard: a misaligned frame is not a frame.
    if domain == "frames" {
        if let Some(v) = value {
            if v % crate::stdlib::FRAME_SIZE != 0 {
                return Err(TypeError {
                    message: format!(
                        "'frame.claim' would take the address 0x{v:X}, which is not {}-aligned: a frame is a {}-byte-aligned physical address",
                        crate::stdlib::FRAME_SIZE, crate::stdlib::FRAME_SIZE
                    ),
                    span,
                });
            }
        }
    }
    if patterns.iter().any(|p| p == crate::stdlib::RANGE_ANY) {
        return Ok(());
    }
    let name = if domain == "frames" { "frame.claim" } else { "irq.claim" };
    let what = if domain == "frames" { "address" } else { "line" };
    let Some(value) = value else {
        return Err(TypeError {
            message: format!(
                "'{name}' takes a {what} the compiler cannot fold, and module '{}' holds only the granted {what}(s) {}; a computed {what} needs the whole-resource grant `caps({domain} = [\"*\"])`",
                ctx.module,
                patterns.join(", "),
            ),
            span,
        });
    };
    if crate::stdlib::range_covers(domain, &patterns, value) {
        return Ok(());
    }
    if domain == "frames" {
        return Err(TypeError {
            message: format!(
                "'frame.claim' would take the address 0x{value:X}, which no 'frames' grant of module '{}' covers (granted: {})",
                ctx.module,
                patterns.join(", ")
            ),
            span,
        });
    }
    Err(TypeError {
        message: format!(
            "'irq.claim' would take the line {value}, which no 'irq' grant of module '{}' covers (granted: {})",
            ctx.module,
            patterns.join(", ")
        ),
        span,
    })
}

/// Records the fixed-arity wrapper one variadic call site needs (gap 7), and
/// returns its name. Idempotent: the wrapper is created the first time a call
/// passes a given list of extra-argument types, and every later call with the
/// same list names the same one. The wrapper's own parameters are the extern's
/// declared parameters followed by one per extra argument, so
/// `codegen_c.rs::write_user_extern` forwards all of them to the native symbol
/// unchanged - the C compiler then applies the default argument promotions a
/// real variadic call gets.
fn register_variadic_wrapper(
    ctx: &mut Ctx,
    extern_key: &str,
    sig: &FnSig,
    extra_tys: &[Ty],
    span: Span,
) -> String {
    let name = crate::types::variadic_wrapper_name(extern_key, extra_tys);
    let module = ctx
        .variadic_externs
        .get(extern_key)
        .cloned()
        .unwrap_or_else(|| ctx.module.clone());
    let mut state = ctx.fns.instances.borrow_mut();
    if state.variadic_wrappers_seen.insert(name.clone()) {
        let mut params = sig.params.clone();
        for (i, ty) in extra_tys.iter().enumerate() {
            params.push((format!("#va{i}"), ty.clone()));
        }
        state.variadic_wrappers.push((
            extern_key.to_string(),
            tir::Function {
                name: name.clone(),
                params,
                ret: sig.ret.clone(),
                body: tir::Block {
                    stmts: Vec::new(),
                    tail: None,
                },
                span,
                module,
                extern_: true,
                env: None,
                section: None,
                naked: false,
                align: None,
            },
        ));
    }
    name
}

/// The key of the generic *function* template `name` or `owned` names, if
/// either does (A-2). The lookup on `name` as written carries the same rule
/// the signature lookup in `check_call` does: it is the canonical spelling,
/// so it has to name a module. A bare name is the calling module's own, and
/// the entry module is the only one whose keys are bare (A-13).
fn func_template_named(ctx: &Ctx, name: &str, owned: &str) -> Option<String> {
    if ctx.fns.templates.contains_key(owned) {
        return Some(owned.to_string());
    }
    (name.contains('.') && ctx.fns.templates.contains_key(name)).then(|| name.to_string())
}

/// The generic types that declare an iterator-shaped `next` (`TypeEnv::iter_nexts`).
fn collect_iter_nexts(program: &ast::Program) -> HashMap<String, IterNext> {
    let mut out = HashMap::new();
    for f in &program.functions {
        let (Some(r), "next") = (&f.receiver, f.name.as_str()) else { continue };
        if r.type_args.is_empty() || !r.mutable || !f.params.is_empty() {
            continue;
        }
        let Some(ret) = &f.return_type else { continue };
        let is_option = ret.name == "Option" || ret.name.ends_with(".Option");
        if !is_option || ret.args.len() != 1 {
            continue;
        }
        out.insert(
            r.type_name.clone(),
            IterNext { params: r.type_args.clone(), item: ret.args[0].clone() },
        );
    }
    out
}

/// A written type that mentions only the template's parameters, builtin scalars
/// and tuples of those, resolved against an instance's arguments.
fn resolve_item_type(te: &ast::TypeExpr, params: &[String], args: &[Ty], env: &TypeEnv) -> Option<Ty> {
    if let Some(elems) = &te.tuple {
        let tys: Option<Vec<Ty>> = elems.iter().map(|e| resolve_item_type(e, params, args, env)).collect();
        return Some(tuple_ty(env, tys?));
    }
    if !te.args.is_empty() || te.amp || te.array_elem.is_some() || te.fn_params.is_some() {
        return None;
    }
    if let Some(i) = params.iter().position(|p| *p == te.name) {
        return args.get(i).cloned();
    }
    Ty::from_name(&te.name)
}

/// The `Item` of a generic type that is an iterator by shape, from the instance's
/// own arguments.
fn shaped_iterator_item(env: &TypeEnv, instance: &str) -> Option<Ty> {
    let (template, args) = env.instances.borrow().origin.get(instance).cloned()?;
    let next = env.iter_nexts.get(&template)?;
    resolve_item_type(&next.item, &next.params, &args, env)
}

/// The equality constraints and the `where` projection bounds of a generic
/// impl (MISSING-Ve8Nc step 5), checked as declared: each constraint names an
/// associated type its trait declares, and a projection bound names a plain
/// trait.
type ImplConstraints = (Vec<Vec<(String, ast::TypeExpr)>>, Vec<(usize, String, String)>);

fn impl_constraints(
    imp: &TraitImplInfo,
    traits: &HashMap<String, TraitInfo>,
    project: &Project,
) -> std::result::Result<ImplConstraints, Diagnostic> {
    let prefix = project.prefix_of(&imp.module);
    let aliases = project.alias_prefixes_of(&imp.module);
    let mut eqs: Vec<Vec<(String, ast::TypeExpr)>> = Vec::new();
    for bs in &imp.type_param_bounds {
        let mut list = Vec::new();
        for b in bs {
            if b.bound_eq.is_empty() {
                continue;
            }
            let key = qualify_written_name(&b.name, prefix, &aliases).unwrap_or_default();
            for (assoc, rhs) in &b.bound_eq {
                let declared = traits
                    .get(&key)
                    .is_some_and(|info| info.assoc_types.iter().any(|a| a.name == *assoc));
                if !declared {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' declares no associated type '{assoc}', so the equality constraint '{assoc} = {}' on a parameter of this impl names nothing",
                            b.name,
                            rhs.display()
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
                list.push((assoc.clone(), rhs.clone()));
            }
        }
        eqs.push(list);
    }
    let mut projections = Vec::new();
    for pb in &imp.projection_bounds {
        let Some(idx) = imp.type_params.iter().position(|p| *p == pb.param) else {
            continue;
        };
        if !pb.bound.args.is_empty() {
            return Err(Diagnostic::new(
                format!("'where {}::{}: {}' bounds a projection with trait arguments, which is not in this slice", pb.param, pb.assoc, pb.bound.display()),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        let Some(key) = qualify_written_name(&pb.bound.name, prefix, &aliases) else {
            return Err(Diagnostic::new(
                format!("'{}' bounds '{}::{}' of this impl, but its first segment is not a module it imports", pb.bound.name, pb.param, pb.assoc),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        if !traits.contains_key(&key) {
            return Err(Diagnostic::new(
                format!("'{}' is not a trait, so it cannot bound '{}::{}' of this impl", pb.bound.name, pb.param, pb.assoc),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        projections.push((idx, pb.assoc.clone(), key));
    }
    Ok((eqs, projections))
}

/// The trait key each parameter of a generic or blanket impl must implement, one
/// list per parameter, in the impl's own module's spelling.
fn impl_param_bounds(
    imp: &TraitImplInfo,
    traits: &HashMap<String, TraitInfo>,
    project: &Project,
) -> std::result::Result<Vec<Vec<String>>, Diagnostic> {
    let prefix = project.prefix_of(&imp.module);
    let aliases = project.alias_prefixes_of(&imp.module);
    let mut bounds: Vec<Vec<String>> = Vec::new();
    for bs in &imp.type_param_bounds {
        let mut keys = Vec::new();
        for b in bs {
            if !b.args.is_empty() {
                return Err(Diagnostic::new(
                    format!("the bound '{}' on an impl parameter takes trait arguments, which a generic impl does not support yet", b.display()),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            let Some(key) = qualify_written_name(&b.name, prefix, &aliases) else {
                return Err(Diagnostic::new(
                    format!("'{}' bounds a parameter of this impl, but its first segment is not a module it imports", b.name),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            if !traits.contains_key(&key) {
                return Err(Diagnostic::new(
                    format!("'{}' is not a trait, so it cannot bound a parameter of this impl", b.name),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            keys.push(key);
        }
        bounds.push(keys);
    }
    Ok(bounds)
}

/// Refuses blanket impls whose bounds lead back to themselves (`impl<T: A> B for
/// T` with `impl<T: B> A for T`, or a blanket impl bounded on its own trait):
/// checking one such instance would need the same check again, forever.
fn check_blanket_cycles(blankets: &[BlanketImpl]) -> std::result::Result<(), Diagnostic> {
    for b in blankets {
        let mut stack: Vec<String> = b.bounds.clone();
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = stack.pop() {
            if key == b.trait_key {
                return Err(Diagnostic::new(
                    format!(
                        "the blanket impl of '{}' bounds its parameter on '{}' itself, so checking any type against it would need the same check again: a blanket impl's bound has to be a different trait",
                        b.trait_name, b.trait_name
                    ),
                    b.impl_span,
                    b.module.clone(),
                ));
            }
            if !seen.insert(key.clone()) {
                continue;
            }
            for other in blankets.iter().filter(|o| o.trait_key == key) {
                stack.extend(other.bounds.iter().cloned());
            }
        }
    }
    Ok(())
}

/// The blanket impl of `trait_key` whose bounds `name` meets, if any: a type
/// that meets them implements the trait (`TypeEnv::blanket_impls`).
fn blanket_holds(env: &TypeEnv, trait_key: &str, name: &str) -> bool {
    env.blanket_impls
        .iter()
        .any(|b| b.trait_key == trait_key && b.bounds.iter().all(|k| implements_trait(env, k, name)))
}

/// The generic impl of `trait_key` that applies to the instance `ty_name`, if
/// there is one: the instance's template has an impl and every bound on the
/// impl's parameters holds for the instance's arguments.
fn generic_impl_applies(env: &TypeEnv, trait_key: &str, ty_name: &str) -> Option<(GenericImpl, Vec<Ty>)> {
    let (template, args) = env.instances.borrow().origin.get(ty_name).cloned()?;
    for gi in env.generic_impls.get(trait_key)? {
        if gi.template != template {
            continue;
        }
        let holds = gi.bounds.iter().enumerate().all(|(i, keys)| {
            keys.iter().all(|key| {
                args.get(i)
                    .and_then(|a| {
                        struct_name_of(a)
                            .map(|s| s.to_string())
                            .or_else(|| builtin_type_name(a).map(|s| s.to_string()))
                    })
                    .is_some_and(|n| implements_trait(env, key, &n))
            })
        });
        if holds && generic_impl_constraints_hold(env, gi, &args) {
            return Some((gi.clone(), args));
        }
    }
    None
}

/// The equality constraints and projection bounds of a generic impl hold for
/// the instance's arguments (MISSING-Ve8Nc step 5).
fn generic_impl_constraints_hold(env: &TypeEnv, gi: &GenericImpl, args: &[Ty]) -> bool {
    let eqs_hold = gi.eqs.iter().enumerate().all(|(i, list)| {
        list.iter().all(|(assoc, rhs)| {
            args.get(i)
                .and_then(|a| assoc_type_of(env, a, assoc))
                .is_some_and(|got| eq_rhs_matches(rhs, &gi.params, args, &gi.prefix, &gi.aliases, &got))
        })
    });
    eqs_hold
        && gi.projections.iter().all(|(i, assoc, key)| {
            args.get(*i)
                .is_some_and(|a| matches!(projection_check(env, a, assoc, key), ProjectionCheck::Holds))
        })
}

/// Why a type with a generic impl of `trait_key` does not implement it: the
/// first argument whose bound fails, spelled out. `None` when the type has no
/// generic impl of the trait at all.
fn generic_impl_shortfall(env: &TypeEnv, trait_key: &str, ty: &Ty) -> Option<String> {
    let name = struct_name_of(ty)?.to_string();
    let (template, args) = env.instances.borrow().origin.get(&name).cloned()?;
    let gi = env.generic_impls.get(trait_key)?.iter().find(|g| g.template == template)?;
    for (i, keys) in gi.bounds.iter().enumerate() {
        let arg = args.get(i)?;
        let arg_name = struct_name_of(arg)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(arg).map(|s| s.to_string()))
            .unwrap_or_else(|| arg.display());
        for key in keys {
            if !implements_trait(env, key, &arg_name) {
                return Some(format!(
                    "its impl needs '{}' ('{}' is parameter '{}') to implement '{}', and it does not",
                    arg.display(),
                    gi.template,
                    gi.params.get(i).map(String::as_str).unwrap_or("?"),
                    key
                ));
            }
        }
        for (assoc, rhs) in gi.eqs.get(i).into_iter().flatten() {
            let got = assoc_type_of(env, arg, assoc).map(|t| t.display()).unwrap_or_else(|| "nothing".to_string());
            if assoc_type_of(env, arg, assoc).is_none_or(|got| !eq_rhs_matches(rhs, &gi.params, &args, &gi.prefix, &gi.aliases, &got)) {
                return Some(format!(
                    "its impl needs '{}' ('{}' is parameter '{}') to bind '{assoc}' to '{}', and it binds '{got}'",
                    arg.display(),
                    gi.template,
                    gi.params.get(i).map(String::as_str).unwrap_or("?"),
                    rhs.display()
                ));
            }
        }
    }
    for (i, assoc, key) in &gi.projections {
        let Some(a) = args.get(*i) else { continue };
        if !matches!(projection_check(env, a, assoc, key), ProjectionCheck::Holds) {
            return Some(format!(
                "its impl needs '{}::{assoc}' to implement '{key}', and it does not",
                gi.params.get(*i).map(String::as_str).unwrap_or("?")
            ));
        }
    }
    None
}

/// The type a trait's associated type `item` is for the instance `ty_name`:
/// bound by a concrete impl, by a generic impl (resolved over the instance's
/// arguments), or the item of an iterator by shape.
fn assoc_type_of(env: &TypeEnv, ty: &Ty, item: &str) -> Option<Ty> {
    let name = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()))?;
    for ((_, tn), bindings) in env.assoc.iter() {
        if *tn == name {
            if let Some(t) = bindings.get(item) {
                return Some(t.clone());
            }
        }
    }
    for trait_key in env.generic_impls.keys() {
        if let Some((gi, args)) = generic_impl_applies(env, trait_key, &name) {
            if let Some((_, te)) = gi.assoc_types.iter().find(|(n, _)| n == item) {
                return resolve_impl_assoc(te, &gi.params, &args, env);
            }
        }
    }
    if item == "Item" {
        return shaped_iterator_item(env, &name);
    }
    None
}

/// A generic impl's `type Item = ...`, resolved over an instance's arguments:
/// a parameter, a builtin, a tuple of those, or a projection `P::Name` of a
/// parameter's argument.
fn resolve_impl_assoc(te: &ast::TypeExpr, params: &[String], args: &[Ty], env: &TypeEnv) -> Option<Ty> {
    if let Some(item) = &te.assoc {
        let i = params.iter().position(|p| *p == te.name)?;
        return assoc_type_of(env, args.get(i)?, item);
    }
    if let Some(elems) = &te.tuple {
        let tys: Option<Vec<Ty>> = elems.iter().map(|e| resolve_impl_assoc(e, params, args, env)).collect();
        return Some(tuple_ty(env, tys?));
    }
    resolve_item_type(te, params, args, env)
}

/// Whether `ty_name` implements `trait_key`, directly or through a supertrait
/// (A-12 slice 3): a type with `impl B for` satisfies a bound on `A` whenever
/// `B: A`, because implementing `B` required an `impl A for` the type.
fn implements_trait(env: &TypeEnv, trait_key: &str, ty_name: &str) -> bool {
    if env
        .impls
        .get(trait_key)
        .map(|types| types.contains(ty_name))
        .unwrap_or(false)
    {
        return true;
    }
    // A generic type whose template declares an iterator-shaped `next` is an
    // `Iterator` by shape (`TypeEnv::iter_nexts`).
    // An explicit generic `impl Iterator` of the type governs it: its bounds
    // (`where I::Item: Display`, `I: Iterator<Item = i64>`) must hold, so the
    // shape alone does not satisfy the trait then (MISSING-Ve8Nc step 5).
    if trait_key == ITERATOR_TRAIT_KEY
        && shaped_iterator_item(env, ty_name).is_some()
        && !has_generic_impl_of(env, trait_key, ty_name)
    {
        return true;
    }
    // `impl<T: Bound> Trait for Type<T>`: an instance implements the trait when
    // each of its arguments meets the bounds the impl puts on its parameter.
    if generic_impl_applies(env, trait_key, ty_name).is_some() {
        return true;
    }
    // `impl<T: Bound> Trait for T`: every type that meets the bound implements it.
    if blanket_holds(env, trait_key, ty_name) {
        return true;
    }
    env.impls.iter().any(|(other, types)| {
        other != trait_key && types.contains(ty_name) && trait_requires(env, other, trait_key)
    })
}

/// Whether the generic type `ty_name` (an instance) has an explicit generic impl
/// of `trait_key` written for its template, whatever the impl's bounds are.
fn has_generic_impl_of(env: &TypeEnv, trait_key: &str, ty_name: &str) -> bool {
    let Some((template, _)) = env.instances.borrow().origin.get(ty_name).cloned() else {
        return false;
    };
    env.generic_impls
        .get(trait_key)
        .is_some_and(|impls| impls.iter().any(|g| g.template == template))
}

/// Whether trait `sub` inherits, directly or transitively, from `sup`.
fn trait_requires(env: &TypeEnv, sub: &str, sup: &str) -> bool {
    let mut stack = vec![sub.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(t) = stack.pop() {
        if !seen.insert(t.clone()) {
            continue;
        }
        if t == sup {
            return true;
        }
        if let Some(info) = env.traits.get(&t) {
            for s in &info.supertraits {
                stack.push(s.clone());
            }
        }
    }
    false
}

/// Checks the trait bounds one instantiation's type arguments must satisfy
/// (A-12 slice 2). `tmpl` is the declaration and `type_args` the concrete
/// arguments, so this answers about the type a call actually binds; `span` is
/// the call site, which is where an unsatisfied bound is reported.
///
/// The bound is written in the callee's module, so its name is qualified with
/// the *template's* prefix and alias table, exactly as its parameter and return
/// types are.
fn check_trait_bounds(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    span: Span,
) -> Result<()> {
    for (i, bounds) in tmpl.bounds.iter().enumerate() {
        if bounds.is_empty() {
            continue;
        }
        let param = tmpl.params.get(i).map(String::as_str).unwrap_or("T");
        let Some(arg) = type_args.get(i) else {
            continue;
        };
        for b in bounds {
            // The alias may not be in the callee's table: the name is read
            // where the declaration was written, and the call site cannot tell
            // the two apart, so the error points at the call.
            let key = match qualify_written_name(&b.name, &tmpl.prefix, &tmpl.aliases) {
                Some(key) => key,
                None => {
                    return Err(TypeError {
                        message: format!(
                            "'{}' bounds '{}' of '{}', but its first segment is not a module the declaration imports",
                            b.name,
                            param,
                            display_callee(&tmpl.key)
                        ),
                        span,
                    })
                }
            };
            let Some(info) = ctx.env.traits.get(&key) else {
                return Err(TypeError {
                    message: format!(
                        "'{}' is not a trait, so it cannot bound '{}' of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                        b.name,
                        param,
                        display_callee(&tmpl.key),
                        b.name
                    ),
                    span,
                });
            };
            // A bound written through another module's alias needs that module
            // to export the trait, exactly as naming any other item there does.
            if let Some((alias, item)) = b.name.split_once('.') {
                if !exported(&key, &ctx.env) {
                    return Err(private_item_error(alias, item, span));
                }
            }
            // A-12 slice 6: the bound names a trait *instance*, so its
            // arguments are resolved here, where the callee's parameters are
            // already bound to the call's types.
            let mut arg_names = Vec::new();
            for a in &b.args {
                let simple = a.tuple.is_none()
                    && a.array_elem.is_none()
                    && a.fn_params.is_none()
                    && a.dyn_trait.is_none()
                    && a.assoc.is_none()
                    && a.args.is_empty()
                    && !a.amp
                    && !a.name.is_empty();
                if !simple {
                    return Err(TypeError {
                        message: format!(
                            "'{}' bounds '{}' of '{}' with the argument '{}', which is not a named type or a builtin: a container, tuple, array, borrow, function type or projection as a bound argument is not in this slice",
                            b.display(),
                            param,
                            display_callee(&tmpl.key),
                            a.display()
                        ),
                        span,
                    });
                }
                let resolved = if let Some(pi) = tmpl.params.iter().position(|p| *p == a.name) {
                    type_args.get(pi).and_then(|at| {
                        struct_name_of(at)
                            .map(|s| s.to_string())
                            .or_else(|| builtin_type_name(at).map(|s| s.to_string()))
                    })
                } else if crate::types::Ty::from_name(&a.name).is_some() {
                    Some(a.name.clone())
                } else {
                    qualify_written_name(&a.name, &tmpl.prefix, &tmpl.aliases)
                };
                let Some(resolved) = resolved else {
                    return Err(TypeError {
                        message: format!(
                            "'{}' bounds '{}' of '{}' with the argument '{}', which names no type",
                            b.display(),
                            param,
                            display_callee(&tmpl.key),
                            a.display()
                        ),
                        span,
                    });
                };
                arg_names.push(resolved);
            }
            if arg_names.len() != info.type_params.len() {
                return Err(TypeError {
                    message: format!(
                        "'{}' bounds '{}' of '{}', and trait '{}' takes {} type argument(s), found {}",
                        b.display(),
                        param,
                        display_callee(&tmpl.key),
                        b.name,
                        info.type_params.len(),
                        arg_names.len()
                    ),
                    span,
                });
            }
            for (bi, pbounds) in info.type_param_bounds.iter().enumerate() {
                let Some(aname) = arg_names.get(bi) else { continue };
                for (written, pkey) in pbounds {
                    if !implements_trait(&ctx.env, pkey, aname) {
                        return Err(TypeError {
                            message: format!(
                                "the call to '{}' instantiates '{}', and '{}' does not satisfy the bound '{written}' on its parameter '{}': write 'impl {written} for {aname} {{ ... }}'",
                                display_callee(&tmpl.key),
                                b.display(),
                                aname,
                                info.type_params[bi]
                            ),
                            span,
                        });
                    }
                }
            }
            let instance = crate::types::trait_instance_key(&key, &arg_names);
            let name = struct_name_of(arg)
                .map(|s| s.to_string())
                .or_else(|| builtin_type_name(arg).map(|s| s.to_string()));
            let implements = name
                .map(|n| implements_trait(&ctx.env, &instance, &n))
                .unwrap_or(false);
            if !implements {
                // A type with a generic impl of the trait fails on one of its
                // arguments; name which, rather than suggesting a concrete impl.
                if let Some(why) = generic_impl_shortfall(&ctx.env, &instance, arg) {
                    return Err(TypeError {
                        message: format!(
                            "the call to '{}' needs '{}' to implement the trait '{}': {why}",
                            display_callee(&tmpl.key),
                            arg.display(),
                            b.display(),
                        ),
                        span,
                    });
                }
                return Err(TypeError {
                    message: format!(
                        "the call to '{}' needs '{}' to implement the trait '{}', and it does not: write 'impl {} for {} {{ ... }}' in the trait's module or in the type's",
                        display_callee(&tmpl.key),
                        arg.display(),
                        b.display(),
                        b.display(),
                        arg.display()
                    ),
                    span,
                });
            }
            check_bound_equalities(ctx, tmpl, type_args, arg, b, &key, param, span)?;
        }
    }
    for pb in &tmpl.projection_bounds {
        check_projection_bound(ctx, tmpl, type_args, pb, span)?;
    }
    Ok(())
}

/// Whether `got` is the type an equality constraint's right-hand side names,
/// read in the spelling of the declaration that wrote it: a parameter of that
/// declaration (bound to `args`), a builtin, or a named type of its module.
fn eq_rhs_matches(
    te: &ast::TypeExpr,
    params: &[String],
    args: &[Ty],
    prefix: &str,
    aliases: &HashMap<String, String>,
    got: &Ty,
) -> bool {
    if !te.args.is_empty() || te.tuple.is_some() || te.array_elem.is_some() || te.amp || te.fn_params.is_some() {
        return false;
    }
    if let Some(i) = params.iter().position(|p| *p == te.name) {
        return args.get(i).is_some_and(|a| a == got);
    }
    if let Some(t) = Ty::from_name(&te.name) {
        return &t == got;
    }
    if builtin_type_name(got).is_some_and(|b| b == te.name) {
        return true;
    }
    qualify_written_name(&te.name, prefix, aliases)
        .is_some_and(|key| struct_name_of(got).is_some_and(|n| n == key))
}

/// Checks the equality constraints (`I: Iterator<Item = i64>`) one bound of a
/// call's parameter writes: the trait declares each associated type, and the
/// argument's type binds it to the named type (MISSING-Ve8Nc step 5).
#[allow(clippy::too_many_arguments)]
fn check_bound_equalities(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    arg: &Ty,
    b: &ast::TypeExpr,
    key: &str,
    param: &str,
    span: Span,
) -> Result<()> {
    let callee = display_callee(&tmpl.key);
    for (assoc, rhs) in &b.bound_eq {
        let declared = ctx
            .env
            .traits
            .get(key)
            .is_some_and(|info| info.assoc_types.iter().any(|a| a.name == *assoc));
        if !declared {
            return Err(TypeError {
                message: format!(
                    "'{}' declares no associated type '{assoc}', so the equality constraint '{assoc} = {}' on '{param}' of '{callee}' names nothing",
                    b.name,
                    rhs.display()
                ),
                span,
            });
        }
        let Some(got) = assoc_type_of(&ctx.env, arg, assoc) else {
            return Err(TypeError {
                message: format!(
                    "the call to '{callee}' instantiates '{param}: {}' with '{}', which binds no '{assoc}', so the equality constraint '{assoc} = {}' cannot hold",
                    b.display(),
                    arg.display(),
                    rhs.display()
                ),
                span,
            });
        };
        if !eq_rhs_matches(rhs, &tmpl.params, type_args, &tmpl.prefix, &tmpl.aliases, &got) {
            return Err(TypeError {
                message: format!(
                    "the call to '{callee}' instantiates '{param}: {}' with '{}', whose '{assoc}' is '{}', not '{}': the equality constraint '{assoc} = {}' fails",
                    b.display(),
                    arg.display(),
                    got.display(),
                    rhs.display(),
                    rhs.display()
                ),
                span,
            });
        }
    }
    Ok(())
}

/// Whether a concrete type's associated type meets a bound (MISSING-Ve8Nc
/// step 5): the type binds `assoc`, and that type implements `key`.
enum ProjectionCheck {
    Holds,
    NoItem,
    Unmet(Ty),
}

fn projection_check(env: &TypeEnv, arg: &Ty, assoc: &str, key: &str) -> ProjectionCheck {
    let Some(item) = assoc_type_of(env, arg, assoc) else {
        return ProjectionCheck::NoItem;
    };
    let name = struct_name_of(&item)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(&item).map(|s| s.to_string()));
    match name {
        Some(n) if implements_trait(env, key, &n) => ProjectionCheck::Holds,
        _ => ProjectionCheck::Unmet(item),
    }
}

/// Checks one `where P::Assoc: Bound` clause of a function at a call: the bound
/// is on the concrete associated type, so `I::Item` is checked where `I` is
/// bound (MISSING-Ve8Nc step 5).
fn check_projection_bound(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    pb: &ast::ProjectionBound,
    span: Span,
) -> Result<()> {
    let callee = display_callee(&tmpl.key);
    let written = format!("{}::{}", pb.param, pb.assoc);
    let Some(pi) = tmpl.params.iter().position(|p| *p == pb.param) else {
        return Ok(());
    };
    let Some(arg) = type_args.get(pi) else {
        return Ok(());
    };
    if !pb.bound.args.is_empty() {
        return Err(TypeError {
            message: format!(
                "'where {written}: {}' bounds a projection with trait arguments, which is not in this slice",
                pb.bound.display()
            ),
            span,
        });
    }
    let Some(key) = qualify_written_name(&pb.bound.name, &tmpl.prefix, &tmpl.aliases) else {
        return Err(TypeError {
            message: format!(
                "'{}' bounds '{written}' of '{callee}', but its first segment is not a module the declaration imports",
                pb.bound.name
            ),
            span,
        });
    };
    if !ctx.env.traits.contains_key(&key) {
        return Err(TypeError {
            message: format!(
                "'{}' is not a trait, so it cannot bound '{written}' of '{callee}': a bound names a declared trait ('trait {} {{ ... }}')",
                pb.bound.name, pb.bound.name
            ),
            span,
        });
    }
    match projection_check(&ctx.env, arg, &pb.assoc, &key) {
        ProjectionCheck::Holds => Ok(()),
        ProjectionCheck::NoItem => Err(TypeError {
            message: format!(
                "the call to '{callee}' bounds '{written}' with '{}', but '{}' binds no associated type '{}': a projection bound needs a type that binds it, such as an iterator",
                pb.bound.display(),
                arg.display(),
                pb.assoc
            ),
            span,
        }),
        ProjectionCheck::Unmet(item) => Err(TypeError {
            message: format!(
                "the call to '{callee}' needs '{written}', which is '{}' for '{}', to implement the trait '{}', and it does not: write 'impl {} for {} {{ ... }}' in the trait's module or in the type's",
                item.display(),
                arg.display(),
                pb.bound.display(),
                pb.bound.display(),
                item.display()
            ),
            span,
        }),
    }
}

/// The refusal for a `dyn` coercion of a type whose trait comes from a blanket
/// impl: a blanket impl has no vtable of its own yet, so the method is reached
/// through a generic function instead.
fn blanket_dyn_refusal(
    want: &Ty,
    value_ty: &Ty,
    inner: &Ty,
    trait_name: &str,
    span: Span,
) -> TypeError {
    TypeError {
        message: format!(
            "expected '{}', found '{}': '{}' implements '{}' through a blanket impl, and a blanket impl has no 'dyn' form yet (MISSING-Ve8Nc step 4 does not include it): call the method through a generic function ('fun f<T: {}>(x: &T)') instead",
            want.display(),
            value_ty.display(),
            inner.display(),
            trait_name,
            trait_name
        ),
        span,
    }
}

/// Whether a builtin receiver carries type arguments its impl table cannot see
/// (`List<i32>` answers to the name `List` for every element type).
fn builtin_arguments_unchecked(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::List(_) | Ty::Box(_) | Ty::Array(..) | Ty::Map(..) | Ty::Atomic(_) | Ty::Mutex(_) | Ty::RwLock(_)
    )
}

/// The template of a blanket impl's method `method` that applies to the receiver
/// `ty`, if one does. `None` when no blanket impl writes the method, or none of
/// them applies to `ty`. `Some(Err)` when two blanket impls of different traits
/// both apply (refused by name), or when the receiver has no name to check the
/// bound against (a reference or a function type has no impl table).
fn blanket_method_template(ctx: &Ctx, ty: &Ty, method: &str, span: Span) -> Option<Result<String>> {
    let candidates: Vec<&BlanketImpl> = ctx
        .env
        .blanket_impls
        .iter()
        .filter(|b| b.methods.iter().any(|m| m == method))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let Some(name) = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()))
    else {
        let b = candidates[0];
        let bound = b.bounds.first().map(|k| k.as_str()).unwrap_or(b.trait_name.as_str());
        return Some(Err(TypeError {
            message: format!(
                "the method '{method}' comes from a blanket impl of '{}', whose bound on the receiver ('{bound}') cannot be checked on '{}': a reference or a function type has no impl table to check a bound against, so call the method on the value it names",
                b.trait_name,
                ty.display()
            ),
            span,
        }));
    };
    let holding: Vec<&BlanketImpl> = candidates
        .iter()
        .copied()
        .filter(|b| b.bounds.iter().all(|k| implements_trait(&ctx.env, k, &name)))
        .collect();
    // A builtin container's impl table is keyed by its bare name (`List`), so a
    // bound on its element cannot be answered for `List<i32>`. A bound that
    // fails by name there is refused, not read as "does not apply".
    if holding.is_empty() && builtin_arguments_unchecked(ty) {
        let b = candidates[0];
        let bound = b.bounds.first().map(|k| k.as_str()).unwrap_or(b.trait_name.as_str());
        return Some(Err(TypeError {
            message: format!(
                "the method '{method}' comes from a blanket impl of '{}', whose bound '{bound}' on the receiver cannot be checked on '{}': a builtin container's impl table has no entry per element type, so the bound on its element is not visible to the check",
                b.trait_name,
                ty.display()
            ),
            span,
        }));
    }
    match holding.as_slice() {
        [] => None,
        [one] => Some(Ok(format!("{}#{method}", crate::types::blanket_receiver_name(&one.trait_key)))),
        [a, b, ..] => Some(Err(TypeError {
            message: format!(
                "'{}.{method}()' is ambiguous: the blanket impls of '{}' and '{}' both provide '{method}' for it, and a method is not chosen between traits by name",
                name, a.trait_name, b.trait_name
            ),
            span,
        })),
    }
}

/// The template key of a method declared on a generic type (A-2): `owner` is
/// the receiver's *instance* key (`Pair<i32, String>`), and the template is
/// declared under the type template's own name (`Pair#first`).
fn method_template_key(ctx: &Ctx, owner: &str, method: &str) -> Option<String> {
    // A method with type parameters of its own on a plain type is a template
    // under the type's own name.
    let plain = format!("{owner}#{method}");
    if ctx.fns.templates.contains_key(&plain) {
        return Some(plain);
    }
    let type_tmpl = ctx
        .env
        .instances
        .borrow()
        .origin
        .get(owner)
        .map(|(t, _)| t.clone())?;
    let key = format!("{type_tmpl}#{method}");
    ctx.fns.templates.contains_key(&key).then_some(key)
}

/// The associated-type projections a generic instantiation makes available
/// (A-12 slice 5): for each type parameter and each trait it is bounded by, the
/// concrete type the argument's impl bound for every associated type the trait
/// declares. The key is the written projection (`I::Item`), which is exactly
/// what `TypeScope::resolve` looks up when it meets one.
fn assoc_projection_bindings(
    env: &TypeEnv,
    params: &[String],
    bounds: &[Vec<ast::TypeExpr>],
    prefix: &str,
    aliases: &HashMap<String, String>,
    type_args: &[Ty],
) -> HashMap<String, Ty> {
    let mut out: HashMap<String, Ty> = HashMap::new();
    for (i, bs) in bounds.iter().enumerate() {
        let (Some(param), Some(arg)) = (params.get(i), type_args.get(i)) else {
            continue;
        };
        let Some(arg_name) = struct_name_of(arg)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(arg).map(|s| s.to_string()))
        else {
            continue;
        };
        for b in bs {
            let Some(key) = qualify_written_name(&b.name, prefix, aliases) else {
                continue;
            };
            for tk in trait_keys_with_supertraits(env, &key) {
                if let Some(bindings) = env.assoc.get(&(tk.clone(), arg_name.clone())) {
                    for (item, ty) in bindings {
                        out.entry(format!("{param}::{item}"))
                            .or_insert_with(|| ty.clone());
                    }
                } else if let Some((gi, targs)) = generic_impl_applies(env, &tk, &arg_name) {
                    for (item, te) in &gi.assoc_types {
                        if let Some(ty) = resolve_impl_assoc(te, &gi.params, &targs, env) {
                            out.entry(format!("{param}::{item}")).or_insert(ty);
                        }
                    }
                } else if tk == ITERATOR_TRAIT_KEY {
                    if let Some(ty) = shaped_iterator_item(env, &arg_name) {
                        out.entry(format!("{param}::Item")).or_insert(ty);
                    }
                }
            }
        }
    }
    out
}

/// Instantiates a generic function for one call site (A-2), returning the
/// instance's key and its concrete signature. The type arguments come from
/// three places, in this order: the receiver's own instantiation (a method),
/// the expected type, and the argument types. The signature is registered
/// *before* the body is queued, so a recursive call inside the body resolves
/// to the same instance instead of instantiating again.
fn instantiate_function(
    ctx: &mut Ctx,
    tmpl_key: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    expected: Option<&Ty>,
    receiver_ty: Option<&Ty>,
) -> Result<(String, FnSig)> {
    let tmpl = ctx.fns.templates[tmpl_key].clone();
    let mut bindings: HashMap<String, Ty> = HashMap::new();
    // A method's type parameters are the receiver type's, which the receiver
    // expression already resolved.
    let blanket = tmpl
        .receiver
        .as_ref()
        .is_some_and(|(name, _)| crate::types::is_blanket_receiver(name));
    if tmpl.receiver.is_some() {
        if blanket {
            // A blanket impl's parameter is the receiver type itself, whatever
            // it is (a builtin included), not an instance's argument.
            if let (Some(rt), Some(p)) = (receiver_ty, tmpl.params.first()) {
                bindings.insert(p.clone(), rt.clone());
            }
        } else if let Some(rk) = receiver_ty.and_then(struct_name_of) {
            if let Some((_, targs)) = ctx.env.instances.borrow().origin.get(rk).cloned() {
                for (p, a) in tmpl.params.iter().zip(targs.iter()) {
                    bindings.insert(p.clone(), a.clone());
                }
            }
        }
    }
    let what = format!("function '{}'", display_callee(&tmpl.key));
    // With an explicit instantiation there is nothing to infer - the arguments
    // are still matched, but by `check_call`, against the signature this
    // produces. Otherwise the expected return type and the argument types
    // supply the parameters.
    if explicit.is_empty() {
        if let (Some(written), Some(exp)) = (&tmpl.ret, expected) {
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, written, exp, &tmpl.params, &mut bindings);
        }
        if tmpl.declared.is_empty() {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("{what} expects 0 argument(s), found {}", args.len()),
                    span,
                });
            }
        } else {
            let names: Vec<String> = tmpl.declared.iter().map(|p| p.name.clone()).collect();
            let slots = arg_slots(&what, &names, args, span)?;
            for (i, p) in tmpl.declared.iter().enumerate() {
                let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &p.ty, &bindings);
                let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
                bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &p.ty, &probed.ty, &tmpl.params, &mut bindings);
                ctx.keep_probe(&args[slots[i]].value, probed);
            }
        }
    }
    let mut type_args = if explicit.is_empty() {
        finish_type_args(&tmpl.key, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.key, &tmpl.params, explicit, span)?
    };
    // A-20: re-type every const argument to its parameter's declared type and
    // range-check it, exactly as a type instantiation does.
    bind_const_args(&tmpl.params, &tmpl.const_params, &mut type_args, span)?;
    // A-12 slice 2: the bound is checked before the instance is registered, and
    // before the early return below, so every call/instantiation of the
    // template answers it. The body is then checked per concrete type exactly
    // as it was before bounds existed, which is what keeps dispatch static.
    check_trait_bounds(ctx, &tmpl, &type_args, span)?;
    let key = instance_key(&tmpl.key, &type_args);
    if let Some(sig) = ctx.sig_of(&key) {
        return Ok((key, sig));
    }
    // The parameter and return types, resolved from the *final* arguments -
    // whichever way they were determined, so an explicit instantiation needs
    // no inference bindings at all. A failure here is a problem in the
    // template's own signature, written in the template's file, so the
    // message names the instance and the span points at the use site, which
    // is the module the caller will attribute the error to.
    let mut resolved: HashMap<String, Ty> = tmpl
        .params
        .iter()
        .cloned()
        .zip(type_args.iter().cloned())
        .collect();
    resolved.extend(assoc_projection_bindings(
        &ctx.env,
        &tmpl.params,
        &tmpl.bounds,
        &tmpl.prefix,
        &tmpl.aliases,
        &type_args,
    ));
    let scope = TypeScope {
        structs: &ctx.structs,
        enums: &ctx.enums,
        env: ctx.env.clone(),
        params: &resolved,
        prefix: &tmpl.prefix,
        aliases: &tmpl.aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(&ctx.consts),
        const_fns: Some(&ctx.const_fns),
        handles: &ctx.env.handles,
    };
    let mut params = Vec::new();
    if let Some((_, receiver_mutable)) = &tmpl.receiver {
        let self_ty = if blanket {
            receiver_ty
                .cloned()
                .expect("a blanket method is only instantiated for a receiver")
        } else {
            let rk = receiver_ty
                .and_then(struct_name_of)
                .expect("a receiver method is only instantiated for a struct receiver");
            Ty::Struct(rk.to_string())
        };
        params.push(("self".to_string(), implicit_borrow_wrap(self_ty, *receiver_mutable)));
    }
    for p in &tmpl.declared {
        let ty = scope.resolve_param(p).map_err(|e| TypeError {
            message: format!("in '{key}': {}", e.message),
            span,
        })?;
        params.push((p.name.clone(), ty));
    }
    let ret = match &tmpl.ret {
        Some(t) => scope.resolve_return(t).map_err(|e| TypeError {
            message: format!("in '{key}': {}", e.message),
            span,
        })?,
        None => Ty::Unit,
    };
    let sig = FnSig { params, ret };
    let mut st = ctx.fns.instances.borrow_mut();
    if st.sigs.len() >= MAX_FUNCTION_INSTANTIATIONS {
        return Err(TypeError {
            message: format!(
                "generic instantiation did not terminate - more than {MAX_FUNCTION_INSTANTIATIONS} function instances were created; a generic function that calls itself with a different type argument grows without bound"
            ),
            span,
        });
    }
    st.sigs.insert(key.clone(), sig.clone());
    st.origin.insert(key.clone(), tmpl.key.clone());
    st.pending.push_back((key.clone(), type_args));
    Ok((key, sig))
}

/// Argument-type overloading (M6.3, stdlib decision 4). Picks the single
/// declaration whose parameter types match the first argument's type. Every
/// parameter of one overload has the same type, so which argument is probed
/// does not matter; and the rule exists only for compiler-shipped modules,
/// so user code has no overload set to resolve against in the first place.
fn resolve_overload(
    callee: &str,
    owned_key: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Option<(String, FnSig)>> {
    if args.is_empty() {
        return Ok(None);
    }
    // The calling module's own spelling first, for the same reason
    // `check_call` tries it first.
    let bases = [owned_key, callee];
    if !bases.iter().any(|b| ctx.overloads.contains_key(*b)) {
        return Ok(None);
    }
    // A type-only probe: `check_expr` has no persistent side effects beyond
    // reading `ctx`, the same assumption `check_method_call` makes.
    let probe = check_expr(&args[0].value, ctx, None)?;
    let mut hits: Vec<String> = Vec::new();
    for base in bases {
        if let Some(set) = ctx.overloads.get(base) {
            for (ptys, key) in set {
                if ptys.iter().all(|t| *t == probe.ty) {
                    hits.push(key.clone());
                }
            }
        }
    }
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => Err(TypeError {
            message: format!(
                "no overload of '{}' takes '{}'{}",
                display_callee(callee),
                probe.ty.display(),
                overload_options(callee, owned_key, ctx)
            ),
            span,
        }),
        1 => {
            let key = hits.remove(0);
            let sig = ctx
                .sigs
                .get(&key)
                .cloned()
                .expect("every overload key is in sigs");
            Ok(Some((key, sig)))
        }
        _ => Err(TypeError {
            message: format!(
                "call to '{}' is ambiguous for argument type '{}'",
                display_callee(callee),
                probe.ty.display()
            ),
            span,
        }),
    }
}

/// " - it is declared for i32, i64, f64", for the message above.
fn overload_options(callee: &str, owned_key: &str, ctx: &Ctx) -> String {
    for base in [callee, owned_key] {
        if let Some(set) = ctx.overloads.get(base) {
            let mut names: Vec<String> = set
                .iter()
                .filter_map(|(ptys, _)| ptys.first().map(|t| t.display()))
                .collect();
            names.sort();
            names.dedup();
            if !names.is_empty() {
                return format!(" - it is declared for {}", names.join(", "));
            }
        }
    }
    String::new()
}

/// The hidden passdown arguments a call to `callee` has to pass, in the
/// order the callee's signature declares them (M5.2). Each one is a borrow of
/// the passdown's storage: the generated global's address, or - inside a
/// function that received that passdown as a hidden parameter - the parameter
/// itself, which *is* that same borrow. A caller that only calls a function
/// reading a passdown never receives one of its own (the wiring is per
/// function's own references), which is why the global's address is the
/// general case rather than the `main`-only case it used to be: emitting the
/// bare identifier there named the global whose type is the passdown's value,
/// so the call passed an integer where the callee's signature wanted a
/// pointer and `cc` refused the file.
fn hidden_args(callee: &str, span: Span, ctx: &Ctx) -> Result<Vec<tir::Expr>> {
    let Some(pds) = ctx.wired_for(callee) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(pds.len());
    for pd in pds {
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                message: format!(
                    "'{callee}' reads passdown '{}', which is not initialized yet here - a 'pd' initializer can only call functions whose passdowns are already set up",
                    pd.qualified
                ),
                span,
            });
        }
        let own_parameter = ctx.own_hidden.iter().any(|h| h == &pd.hidden);
        // The borrow handed over is the callee's (`PdWiring::exclusive`), not
        // the caller's: a caller that only forwards a read-only callee's
        // parameter never needed an exclusive borrow of its own, and one that
        // does hold the parameter exclusively can pass it on either way
        // (the closure in `close_pd_modes` is what makes the two agree).
        let exclusive = if own_parameter {
            ctx.pd_exclusive(&pd)
        } else {
            let callee_key = ctx.wiring_key_of(callee);
            ctx.pd_exclusive_for(&callee_key, &pd)
        };
        out.push(pd.arg(span, !own_parameter, exclusive));
    }
    Ok(out)
}

/// Checks a string template's interpolated pieces and lowers it to one
/// `tir::ExprKind::Template`. Interpolated values must be printable under
/// exactly the rule `print` already applies (numeric, `bool`, `char`,
/// `str`) - a template is a formatted print, not a string builder.
fn check_template(
    parts: &[ast::TemplatePart],
    newline: bool,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let mut pieces = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ast::TemplatePart::Text(text) => {
                pieces.push(tir::TemplatePiece::Text(text.clone()))
            }
            ast::TemplatePart::Expr(e) => {
                let value = check_expr(e, ctx, None)?;
                // The element type decides whether a container is printable at
                // all, and an open container has not decided it yet: report
                // that rather than spell the marker (`List<unit>`).
                refuse_open_arg(&value.ty, e)?;
                let value = borrow_for_print(value, "a string template", ctx)?;
                if let Ty::Handle(name) = &value.ty {
                    return Err(TypeError {
                        message: format!(
                            "cannot interpolate a value of type '{name}' into a string template: an 'extern type' is an opaque native handle with no printable value"
                        ),
                        span: e.span(),
                    });
                }
                let printable = value.ty.is_numeric()
                    || matches!(value.ty, Ty::Bool | Ty::Char)
                    || is_str_like(&value.ty)
                    || is_printable_buffer(&value.ty);
                if !printable {
                    // A `datac`/`enum` that implements `Display` interpolates
                    // through `x.to_text()` (`plan/traits.md` 13.4).
                    if let Some(text) = lower_display(&value, e.span(), ctx)? {
                        pieces.push(tir::TemplatePiece::Value(text));
                        continue;
                    }
                    return Err(TypeError {
                        message: match &value.ty {
                            Ty::Struct(_) => format!(
                                "cannot interpolate a value of type '{}' into a string template: it does not implement 'Display' (write 'impl fmt.Display for {} {{ fun Self.to_text().return: String }}', importing 'std/fmt')",
                                value.ty.display(),
                                value.ty.display()
                            ),
                            other => format!(
                                "cannot interpolate a value of type '{}' into a string template",
                                other.display()
                            ),
                        },
                        span: e.span(),
                    });
                }
                pieces.push(tir::TemplatePiece::Value(value));
            }
        }
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Template { newline, pieces },
        ty: Ty::Unit,
        span,
    })
}

/// Whether this is a call that can say what an open container holds: the
/// element `List`/`Box` hand in, or the value (and the key beside it) a map
/// `insert` hands in. `List.insert` and `List.set` name theirs second, after
/// the index.
fn decides_container_element(ty: &Ty, method: &str) -> bool {
    match ty {
        Ty::List(_) => matches!(method, "push" | "insert" | "set"),
        Ty::Box(_) => method == "set",
        Ty::Map(..) => method == "insert",
        _ => false,
    }
}

/// Checks one deciding argument for the type it contributes to an open
/// container. A `str` literal is checked again as the `String` a container
/// element is: `xs.push("a")` builds a list of text, exactly as
/// `List<String>.push("a")` does. A `str` *value* still has no element to be,
/// and comes back the borrowed view it is for the element check to refuse.
fn check_deciding_element(arg: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let checked = check_expr(arg, ctx, None)?;
    if matches!(checked.ty, Ty::Str) {
        return check_expr(arg, ctx, Some(&Ty::String));
    }
    Ok(checked)
}

/// The container an open container's deciding call names, or `None` when the
/// call is not one that decides anything.
///
/// Every deciding argument is checked here, once, to learn its type. It is the
/// same probe the receiver already goes through, and safe for the same reason:
/// `check_expr` reads the context and reports, and the call's own argument
/// check runs again below with the signature this produced.
fn open_container_from_call(
    open: &Ty,
    method: &str,
    args: &[ast::CallArg],
    ctx: &mut Ctx,
) -> Result<Option<Ty>> {
    match open {
        Ty::List(_) => {
            let arg = match method {
                "push" => args.first(),
                "insert" | "set" => args.get(1),
                _ => return Ok(None),
            };
            let Some(arg) = arg else { return Ok(None) };
            let value = check_deciding_element(&arg.value, ctx)?;
            if !crate::types::is_list_element(&value.ty) {
                return Err(TypeError {
                    message: list_element_error(&value.ty),
                    span: arg.value.span(),
                });
            }
            Ok(crate::types::fill_open_container(
                open,
                &Ty::List(Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        Ty::Box(_) => {
            if method != "set" {
                return Ok(None);
            }
            let Some(arg) = args.first() else { return Ok(None) };
            let value = check_deciding_element(&arg.value, ctx)?;
            if !crate::types::is_box_element(&value.ty) {
                return Err(TypeError {
                    message: box_element_error(&value.ty),
                    span: arg.value.span(),
                });
            }
            Ok(crate::types::fill_open_container(
                open,
                &Ty::Box(Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        Ty::Map(..) => {
            if method != "insert" {
                return Ok(None);
            }
            let (Some(key_arg), Some(value_arg)) = (args.first(), args.get(1)) else {
                return Ok(None);
            };
            let value = check_deciding_element(&value_arg.value, ctx)?;
            if !crate::types::is_map_element(&value.ty) {
                return Err(TypeError {
                    message: map_element_error(&value.ty),
                    span: value_arg.value.span(),
                });
            }
            // The key is read through a `str` view for a `String` key, which
            // is the one key type a call site cannot spell from the argument
            // alone: a `String` key takes `"a"`, and `"a"` is a `str`.
            let key = check_expr(&key_arg.value, ctx, None)?;
            let key_ty = if is_str_like(&key.ty) {
                Ty::String
            } else if crate::types::is_map_key_with(&key.ty, &ctx.keyable_set()) {
                key.ty.clone()
            } else {
                return Err(TypeError {
                    message: map_key_error(&key.ty),
                    span: key_arg.value.span(),
                });
            };
            Ok(crate::types::fill_open_container(
                open,
                &Ty::Map(Box::new(key_ty), Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        _ => Ok(None),
    }
}

/// The diagnostic an open container gets at a use that neither decides what it
/// holds nor is one of the two associated constructors that never knew: the
/// element type is still missing, and saying so names the call that could have
/// supplied it instead of the local that was declared without one.
fn open_container_use_error(ty: &Ty, receiver: &ast::Expr, span: Span) -> TypeError {
    let (binding, constructor, decider, example) = match ty {
        Ty::List(_) => ("xs", "List.empty()", "xs.push(v)", "var xs: List<i32> = List.empty()"),
        Ty::Box(_) => ("b", "Box.empty()", "b.set(v)", "var b: Box<i32> = Box.empty()"),
        _ => ("m", "Map.new()", "m.insert(k, v)", "var m: Map<String, i32> = Map.new()"),
    };
    let named = match receiver {
        ast::Expr::Ident(name, _) => format!("'{name}'"),
        _ => format!("this container (the one bound as '{binding}')"),
    };
    TypeError {
        message: format!(
            "the element type of {named} is still open: nothing said what '{constructor}' holds, and '{decider}' is what decides it. Write it on the binding instead ('{example}')"
        ),
        span,
    }
}

/// An open container handed to a call's argument has nobody left to decide it:
/// a parameter that names a container element takes it (`fun take(xs:
/// List<i32>)` repairs the binding on the way in), while a parameter that takes
/// the container's own type (`fun show<T>(x: T)`) would carry the marker into
/// the emitted program, where the element type is what every routine is named
/// after.
fn refuse_open_arg(ty: &Ty, arg: &ast::Expr) -> Result<()> {
    if crate::types::is_open_container(ty) {
        return Err(open_container_use_error(ty, arg, arg.span()));
    }
    Ok(())
}

/// `E.A.tag()` where `E` is an `enum`: the receiver *builds* a variant, so it
/// names a type rather than a value, while a method an `enum` declares (`fun
/// E.tag()`) is called on a value of the enum type. Refused before the
/// receiver is checked as a value, so the diagnostic is about the spelling
/// rather than about a variant of the wrong arity; a value of enum type
/// reached any other way (`val e = E.A` then `e.tag()`) reaches the method
/// tables like any other receiver.
fn check_method_call_enum_guard(
    receiver: &ast::Expr,
    method: &str,
    span: Span,
    ctx: &Ctx,
) -> Result<()> {
    // `E.A.to_int()` is a read of the variant's value, so it is reached on the
    // variant path itself; only a *declared* method needs a binding first.
    if method == "to_int" {
        return Ok(());
    }
    if let ast::Expr::FieldAccess { base, .. } = receiver {
        if enum_path(base, ctx).is_some() {
            return Err(TypeError {
                message: "an 'enum' method is called on a value, not on a variant path; bind the variant first, as in: val e = E.A  then  e.tag()".to_string(),
                span,
            });
        }
    }
    Ok(())
}

/// `Buffer.null()` and `Window.cast(w)` (gap 1): the two operations written on
/// a handle's *type name*. `null` is `NULL` (C) / `ptr null` (LLVM) in the
/// handle's own type; `cast` is a pointer reinterpretation with no runtime
/// check, so it is the explicit downcast the implicit upcast direction does not
/// give. Both are legal only where an `[extern.*]` section exists.
fn check_handle_associated(
    handle: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    match method {
        "null" => {
            ctx.require_extern_section(&format!("{handle}.null()"), span)?;
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'{handle}.null()' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: crate::types::HANDLE_NULL_KEY.to_string(),
                    args: Vec::new(),
                },
                ty: Ty::Handle(handle.to_string()),
                span,
            })
        }
        "cast" => {
            ctx.require_extern_section(&format!("{handle}.cast(w)"), span)?;
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "'{handle}.cast(w)' takes exactly 1 argument, found {}",
                        args.len()
                    ),
                    span,
                });
            }
            if args[0].name.is_some() {
                return Err(TypeError {
                    message: format!("'{handle}.cast' does not take named arguments"),
                    span,
                });
            }
            let value = check_expr(&args[0].value, ctx, None)?;
            let Ty::Handle(from) = &value.ty else {
                return Err(TypeError {
                    message: format!(
                        "'{handle}.cast(w)' requires 'w' to be a handle, found '{}'",
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            };
            if !handle_is_same_or_ancestor(&ctx.env.handle_parents, from, handle) {
                return Err(TypeError {
                    message: format!(
                        "'{handle}.cast(w)' requires 'w' to be '{handle}' or an ancestor of it, found '{from}'; a child converts up to its parent implicitly, and 'cast' never checks at run time"
                    ),
                    span: args[0].value.span(),
                });
            }
            Ok(tir::Expr {
                ty: Ty::Handle(handle.to_string()),
                ..value
            })
        }
        other => Err(TypeError {
            message: format!(
                "'{handle}.{other}' is not a handle operation: write '{handle}.null()' for the null handle, or call 'is_null()' on a value ('w.is_null()')"
            ),
            span,
        }),
    }
}

/// `receiver.method(args)`: resolves `method` against the receiver
/// expression's `datac` type, then lowers to a plain `tir::ExprKind::Call`
/// - the receiver becomes the first argument, auto-borrowed through
/// `check_call_arg` exactly like any other `Ty::Ref` parameter (so a
/// `fun mut T.m()` method reuses M4's aliasing checks with no special
/// casing). No new `tir`/codegen shape is needed for method calls at all.
///
/// A `List<T>` method is the same thing with the element type read off the
/// receiver (`list_sig`), and `expected` is consulted only for the two
/// associated constructors that cannot name their own element type:
/// `List.empty()` gets it from the binding's written type
/// (`val xs: List<String> = List.empty()`), and `List.of(x)` from `x` or
/// from `expected` when both are available.
/// Whether a static method's parameter accepts an argument of this type: an
/// exact match, or a shared borrow of the argument's type (a `datac` parameter
/// is borrowed, so its resolved type is a `Ty::Ref` over the written type).
fn static_param_accepts(param: &Ty, arg: &Ty) -> bool {
    if param == arg {
        return true;
    }
    matches!(param, Ty::Ref { inner, .. } if **inner == *arg)
}

/// Resolves a static method written on a **trait's** own name (A-12 slice 6):
/// `From.from(x)`. The implementing type has to be inferable from the expected
/// type, because the trait name alone does not name one; the call then resolves
/// against that type's impls exactly as `AppErr.from(x)` does. `None` when the
/// trait declares no such static method, so the ordinary method path can
/// report it.
fn check_trait_static_call(
    trait_key: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<Option<tir::Expr>> {
    let Some(info) = ctx.env.traits.get(trait_key) else {
        return Ok(None);
    };
    if !info
        .methods
        .iter()
        .any(|m| m.receiver.is_none() && m.name == method)
    {
        return Ok(None);
    }
    let Some(exp) = expected else {
        return Err(TypeError {
            message: format!(
                "'{trait_key}.{method}' is a static method, so the implementing type has to be known here: annotate the binding with its type, or call it on the type itself ('<Type>.{method}(...)'), which names it outright"
            ),
            span,
        });
    };
    let Some(owner) = struct_name_of(exp)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(exp).map(|s| s.to_string()))
    else {
        return Err(TypeError {
            message: format!(
                "'{trait_key}.{method}' needs a concrete implementing type, and the expected type '{}' is not one: call it on the type itself ('<Type>.{method}(...)'), which names it outright",
                exp.display()
            ),
            span,
        });
    };
    check_static_method_call(&owner, method, args, span, ctx, expected)
}

/// The canonical type name a `Type.member(...)` receiver written as a bare
/// identifier stands for (A-12 slice 6): a declared `datac`/`enum`, a builtin
/// that carries methods, or the concrete type a bounded type parameter is
/// instantiated with (`X.from(x)` in a generic body). `None` when the name is a
/// local, a module alias, or no type at all.
fn static_owner_name(name: &str, ctx: &Ctx) -> Option<String> {
    if let Some(ty) = ctx.type_args.get(name) {
        return struct_name_of(ty)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(ty).map(|s| s.to_string()));
    }
    if ctx.structs.contains_key(name) || ctx.is_enum(name) {
        return Some(name.to_string());
    }
    if crate::types::Ty::from_name(name).is_some() {
        return Some(name.to_string());
    }
    let qualified = ctx.qualify_own(name);
    if ctx.structs.contains_key(&qualified) || ctx.is_enum(&qualified) {
        return Some(qualified);
    }
    None
}

/// Resolves a **static** method call written on a type name (A-12 slice 6):
/// `AppErr.from(x)`, or `X.from(x)` where `X` is a bounded type parameter
/// instantiated with a concrete type. Returns `None` when the type has no
/// static member of that name, so the caller can fall back to the ordinary
/// method path and its diagnostic.
fn check_static_method_call(
    owner: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<Option<tir::Expr>> {
    let _ = expected;
    let owner_ty = crate::types::Ty::from_name(owner)
        .unwrap_or_else(|| Ty::Struct(owner.to_string()));
    // Every `impl` of any trait for this type whose trait declares a static
    // method of this name is a candidate; the argument types then pick one.
    let mut candidates: Vec<(String, FnSig)> = Vec::new();
    for imp in ctx.env.trait_impls.iter() {
        if imp.type_name != owner {
            continue;
        }
        let Some(info) = ctx.env.traits.get(&imp.trait_name) else {
            continue;
        };
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            if m.receiver.is_some() || m.name != method {
                continue;
            }
            let sig = resolve_trait_method_sig(
                info,
                m,
                &owner_ty,
                &trait_args,
                &ctx.structs,
                &ctx.enums,
                &ctx.env,
                &ctx.consts,
                &ctx.const_fns,
            )?;
            candidates.push((impl_method_key(imp, m), sig));
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.retain(|(_, s)| s.params.len() == args.len());
    if candidates.is_empty() {
        return Err(TypeError {
            message: format!(
                "'{owner}.{method}' is a static method, and no overload takes {} argument(s)",
                args.len()
            ),
            span,
        });
    }
    let (key, sig) = if candidates.len() == 1 {
        candidates.remove(0)
    } else {
        // Several instantiations of one trait for one type (`From<A>` and
        // `From<B>`): the argument types pick the one meant. A type-only
        // probe, exactly as overload resolution does.
        let mut arg_tys = Vec::with_capacity(args.len());
        for a in args {
            arg_tys.push(check_expr(&a.value, ctx, None)?.ty);
        }
        let mut hits: Vec<(String, FnSig)> = candidates
            .into_iter()
            .filter(|(_, s)| {
                s.params
                    .iter()
                    .zip(arg_tys.iter())
                    .all(|((_, p), a)| static_param_accepts(p, a))
            })
            .collect();
        match hits.len() {
            1 => hits.remove(0),
            0 => {
                return Err(TypeError {
                    message: format!(
                        "no static method '{owner}.{method}' matches the argument type(s) {}",
                        arg_tys
                            .iter()
                            .map(|t| format!("'{}'", t.display()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    span,
                })
            }
            _ => {
                return Err(TypeError {
                    message: format!(
                        "call to static method '{owner}.{method}' is ambiguous: several trait impls match these argument types"
                    ),
                    span,
                })
            }
        }
    };
    ctx.check_capability(&key, span)?;
    let checked = match_args(
        &format!("static method '{owner}.{method}'"),
        &sig.params,
        args,
        span,
        ctx,
    )?;
    Ok(Some(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: key,
            args: checked,
        },
        ty: sig.ret,
        span,
    }))
}

fn check_method_call(
    receiver: &ast::Expr,
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Bytes.empty()` / `Bytes.from_str(s)` / `List.of(x)`: an *associated*
    // builtin, written on the type name, so it takes no receiver argument and
    // is resolved before the receiver is checked as a value (it isn't one). A
    // local or an `impl` alias of the same name shadows the type name,
    // exactly as it would anywhere else - which is why those cases fall
    // through to the ordinary method path below.
    if let ast::Expr::Ident(name, _) = receiver {
        if ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
            if name == "List" || name == "Box" || name == "Map" {
                return check_container_associated(name, method, args, span, ctx, expected);
            }
            // `Atomic.new(x)` and `AtomicI64.new(0)`, the two spellings of the
            // one constructor of the builtin atomic cell (MK9-S3-M1 stage A1).
            if name == "Atomic" || crate::types::atomic_alias_elem(name).is_some() {
                return check_atomic_associated(name, method, args, span, ctx, expected);
            }
            // `Mutex.new(x)`, the one constructor of the builtin mutex
            // (B-5, `std/sync`). Like `Atomic.new`, the element type comes
            // from the value or from the binding's written type.
            if name == "Mutex" {
                return check_mutex_associated(name, method, args, span, ctx, expected);
            }
            // `RwLock.new(x)` and `Condvar.new()`, the constructors of the
            // builtin rwlock and condition variable (`std/sync`).
            if name == "RwLock" {
                return check_rwlock_associated(name, method, args, span, ctx, expected);
            }
            if name == "Condvar" {
                return check_condvar_associated(name, method, args, span, ctx, expected);
            }
            // `Widget.null()` and `Window.cast(w)` (gap 1): the two operations
            // written on a handle's type name rather than on a value.
            if let Some(handle) = ctx.handle_type_named(name) {
                return check_handle_associated(&handle, method, args, span, ctx);
            }
            // `Ptr.null()`, `Ptr.from_addr(a)`, `Ptr.from_ref(&x)`,
            // `Ptr.from_mut(x)` and `Ptr.alloc<T>(count)` (MK9-S2 stage 1).
            if name == "Ptr" {
                return check_ptr_associated(method, type_args, args, span, ctx, expected);
            }
            // A-12 slice 6: a static member written on the trait's own name
            // (`From.from(x)`), when the expected type names the implementing
            // type. Ambiguity is refused by name rather than guessed.
            let trait_key = ctx.qualify_own(name);
            if ctx.env.traits.contains_key(&trait_key) {
                if let Some(e) =
                    check_trait_static_call(&trait_key, method, args, span, ctx, expected)?
                {
                    return Ok(e);
                }
            }
            // A-12 slice 6: a static member of a trait impl written on the
            // type (`AppErr.from(x)`), or on a bounded type parameter
            // instantiated with a concrete type (`X.from(x)`).
            if let Some(owner) = static_owner_name(name, ctx) {
                if let Some(e) =
                    check_static_method_call(&owner, method, args, span, ctx, expected)?
                {
                    return Ok(e);
                }
            }
            let key = format!("{name}#{method}");
            if let Some(assoc) = BUILTINS.get(&key).filter(|b| b.associated) {
                let params = assoc.params.clone();
                let ret = assoc.ret.clone();
                let checked = match_args(&format!("'{name}.{method}'"), &params, args, span, ctx)?;
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Call { callee: key, args: checked },
                    ty: ret,
                    span,
                });
            }
        }
    }
    // The same two operations written through a module alias:
    // `mem.Buffer.null()` and `mem.Buffer.cast(w)`. The receiver is a dotted
    // type path, not a value, which is why it is read here before the probe.
    if !matches!(receiver, ast::Expr::Ident(..)) {
        if let Some(path) = expr_path(receiver) {
            if !is_module_alias(&path, ctx) {
                if let Some(handle) = ctx.handle_type_named(&path) {
                    return check_handle_associated(&handle, method, args, span, ctx);
                }
            }
        }
    }

    // Type-only probe to learn the receiver's owner name; re-checked for
    // real (with its actual borrow/move effects) via `check_call_arg`
    // below. `check_expr` has no persistent side effects beyond reading
    // `ctx`, so checking the same expression twice here is safe.
    let probe = check_expr(receiver, ctx, None)?;
    // A method call on a borrowed trait object (A-12 slice 4) goes through the
    // vtable: the trait's flattened method list gives the slot, the receiver's
    // type gives the mutability, and the trait's own signature gives the
    // argument types.
    if let Ty::Dyn { mutable, trait_name } = &probe.ty {
        let (mutable, trait_name) = (*mutable, trait_name.clone());
        return check_dyn_method_call(probe, &trait_name, mutable, method, args, span, ctx);
    }
    // A method call on an owned trait object (A-12 slice 7): `is_empty` and
    // `get` are the box's own two methods, and every other name goes through the
    // trait's vtable exactly as it does on a `&dyn`. A *borrow* of the box (a
    // `mut Box<dyn Trait>` parameter's `&Box<dyn Trait>`, or a `List.get` of an
    // element) is seen through to the box, because a box holds a pointer and the
    // borrow is what reaches it.
    let box_dyn_trait = match &probe.ty {
        Ty::BoxDyn { trait_name } => Some(trait_name.clone()),
        Ty::Ref { inner, .. } => match &**inner {
            Ty::BoxDyn { trait_name } => Some(trait_name.clone()),
            _ => None,
        },
        _ => None,
    };
    if let Some(trait_name) = box_dyn_trait {
        return check_box_dyn_method_call(probe, &trait_name, method, args, span, ctx);
    }
    // `w.is_null()` (gap 1): the one method an opaque handle has, comparing
    // the pointer against the type's null value. Any other name is a method
    // the handle does not have, which is a better diagnostic than the
    // generic "non-struct type" below.
    if let Ty::Handle(handle) = &probe.ty {
        if method == "is_null" {
            ctx.require_extern_section(&format!("{handle}.is_null()"), span)?;
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'is_null' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: crate::types::HANDLE_IS_NULL_KEY.to_string(),
                    args: vec![probe],
                },
                ty: Ty::Bool,
                span,
            });
        }
        return Err(TypeError {
            message: format!(
                "'{handle}' is an opaque native handle with no readable contents: 'is_null()' is its only method"
            ),
            span,
        });
    }
    // A method on a `Ptr<T>` (MK9-S2 stage 1). Every signature depends on the
    // element type, and `cast<U>()` also takes a written target, so none of
    // them can live in the static builtin table. The receiver is `Copy`, so it
    // goes through `check_call_arg` as a plain value rather than a borrow.
    if crate::types::ptr_elem_of(&probe.ty).is_some() {
        return check_ptr_method(probe, receiver, method, type_args, args, span, ctx);
    }
    let owner = struct_name_of(&probe.ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(&probe.ty).map(|s| s.to_string()));
    let Some(owner) = owner else {
        return Err(TypeError {
            message: format!("cannot call method '{method}' on non-struct type '{}'", probe.ty.display()),
            span,
        });
    };
    // Kernel stage 9, MK9-S6-M1: a destructor is not callable. Naming it would
    // run it on top of the drop the value already owes, so the explicit call is
    // refused with the reason.
    if method == crate::types::DROP_METHOD
        && ctx.sig_of(&crate::types::drop_method_key(&owner)).is_some()
    {
        return Err(TypeError {
            message: format!(
                "'{owner}.drop' cannot be called: an explicit destructor call is an error, because the value's own drop already runs when its owner dies"
            ),
            span,
        });
    }
    // A method on an `Atomic<T>` (MK9-S3-M1 stage A1). Its signature depends on
    // the element type, and `compare_exchange`'s `Result<T, T>` is an instance
    // of the stdlib enum rather than a fixed type, so neither can live in the
    // static builtin table. The receiver is a place, so it goes through the same
    // `check_call_arg` a declared method's does, which is what makes `store`
    // and the `fetch_*` require a `mut` binding, since they write the location.
    // `m.to_int()` (kernel stage 2): a field-less enum's compile-time
    // discriminant, read as the repr integer type. Resolved before the method
    // tables because an enum has no declaration to search and no method of
    // that name in the builtin table either; a method the enum declares under
    // that name still wins.
    if method == "to_int"
        && ctx.is_enum(&owner)
        && ctx.sig_of(&format!("{owner}#to_int")).is_none()
    {
        return lower_enum_to_int(probe.ty.clone(), receiver, span, ctx);
    }
    if let Some(elem) = crate::types::mutex_elem_of(&probe.ty).cloned() {
        return check_mutex_method(probe, receiver, &elem, method, args, span, ctx, expected);
    }
    if let Some(elem) = crate::types::rwlock_elem_of(&probe.ty).cloned() {
        return check_rwlock_method(probe, receiver, &elem, method, args, span, ctx, expected);
    }
    if matches!(probe.ty, Ty::Condvar) {
        return check_condvar_method(probe, receiver, method, args, span, ctx);
    }
    if let Some(elem) = crate::types::atomic_elem_of(&probe.ty).cloned() {
        let key = format!("Atomic#{method}");
        if let Some(sig) = atomic_sig(&key, &elem, ctx, span)? {
            let mut checked = vec![check_call_arg("'self'", &sig.params[0].1, receiver, ctx)?];
            checked.extend(match_args(
                &format!("method 'Atomic.{method}'"),
                &sig.params[1..],
                args,
                span,
                ctx,
            )?);
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: checked },
                ty: sig.ret,
                span,
            });
        }
    }
    // `n.try_to_u8()` (MISSING-RbhdW): the fallible twin of `n.to_u8()`. A
    // builtin has no declaration to resolve its return type at, so the call is
    // lowered here into a shape both backends already lower: the receiver
    // bound once, the range test, and one arm per `Option` variant. It is
    // resolved before the method tables because an integer has no table to
    // search and no declaration ever names this method.
    // `x.wrapping_shl(n)` / `x.wrapping_shr(n)`: the shift with the amount
    // masked to the width (`n & (bits - 1)`), the escape from the run-time
    // range check a plain `<<`/`>>` makes. Lowered here to the ordinary
    // shift, so both backends and a freestanding build need nothing new.
    if matches!(method, "wrapping_shl" | "wrapping_shr") {
        if let Some(bits) = int_bits(&probe.ty) {
            let ty = probe.ty.clone();
            let value = check_call_arg("'self'", &ty, receiver, ctx)?;
            let amount = match_args(
                &format!("method '{}.{method}'", ty.display()),
                &[("n".to_string(), ty.clone())],
                args,
                span,
                ctx,
            )?
            .remove(0);
            let mask = (bits - 1) as i128;
            let masked = match &amount.kind {
                tir::ExprKind::IntLit(v) => tir::Expr {
                    kind: tir::ExprKind::IntLit(*v & mask),
                    ty: ty.clone(),
                    span,
                },
                _ => tir::Expr {
                    kind: tir::ExprKind::Binary {
                        op: ast::BinOp::BitAnd,
                        lhs: Box::new(amount),
                        rhs: Box::new(tir::Expr {
                            kind: tir::ExprKind::IntLit(mask),
                            ty: ty.clone(),
                            span,
                        }),
                    },
                    ty: ty.clone(),
                    span,
                },
            };
            let op = if method == "wrapping_shl" { ast::BinOp::Shl } else { ast::BinOp::Shr };
            return Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(value),
                    rhs: Box::new(masked),
                },
                ty,
                span,
            });
        }
    }
    if let Some((src, dst)) = try_to_pair(&probe.ty, method) {
        let value = check_call_arg("'self'", &src, receiver, ctx)?;
        return lower_try_to(&src, &dst, value, span, ctx);
    }
    // `cfg.on_event(x)` where the receiver has a *field* of function type
    // (A-4): the call syntax is a method's, the callee is a field, and the
    // field is a place holding a function value. A declared method of the
    // same name wins - the declaration is the more specific statement of what
    // the type does - so this is only consulted when there is no such method.
    if ctx.sig_of(&format!("{owner}#{method}")).is_none() {
        if let Some(fn_ty) = fn_field_ty(&probe.ty, method, ctx) {
            // A field holding a closure is callable the same way (A-4 phase
            // E): the call goes through the pair the field holds.
            if matches!(fn_ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }) {
                let callee = tir::Expr {
                    kind: tir::ExprKind::FieldAccess {
                        base: Box::new(probe),
                        field: method.to_string(),
                    },
                    ty: fn_ty.clone(),
                    span,
                };
                return check_indirect_call_expr(
                    callee,
                    &fn_ty,
                    args,
                    span,
                    ctx,
                    &format!("field '{method}'"),
                );
            }
        }
    }
    let key = format!("{owner}#{method}");
    // A container a bare `List.empty()`/`Box.empty()`/`Map.new()` left with no
    // element type (`types::is_open_container`) is decided by the first call
    // that has to know it. `push`, `insert` and `set` hand the element in, so
    // the binding is repaired in place here and every later statement sees the
    // real type. A call that does not decide it is refused: monomorphizing a
    // `List<Unit>` routine names a routine nothing builds.
    let decided = if crate::types::is_open_container(&probe.ty) {
        match open_container_from_call(&probe.ty, method, args, ctx)? {
            Some(filled) => {
                if let ast::Expr::Ident(name, _) = receiver {
                    if ctx.lookup(name).is_some() {
                        ctx.set_binding_ty(name, filled.clone());
                    }
                }
                filled
            }
            // A deciding method whose argument list is not the shape it needs:
            // the ordinary arity check says more than this one could.
            None if decides_container_element(&probe.ty, method) => probe.ty.clone(),
            None => return Err(open_container_use_error(&probe.ty, receiver, span)),
        }
    } else {
        probe.ty.clone()
    };
    let container_elem = crate::types::list_elem_of(&decided)
        .or_else(|| crate::types::box_elem_of(&decided))
        .or_else(|| crate::types::map_elem_of(&decided))
        .or_else(|| crate::types::array_elem_of(&decided))
        .cloned();
    let container_key = crate::types::map_key_of(&decided)
        .cloned()
        .unwrap_or(Ty::Unit);
    // A blanket impl's method (`impl<T: Bound> Trait for T`) is the last source
    // a method name is tried against: an inherent method, a concrete or generic
    // impl and a builtin all come first. Its result is taken only in the arm
    // below, and only when nothing else matched.
    let mut blanket_tmpl = blanket_method_template(ctx, &probe.ty, method, span);
    let (key, sig) = match ctx.sig_of(&key) {
        Some(s) => (key, s),
        // A-2: a method declared on a generic type (`fun Pair<A, B>.first()`).
        // Its type arguments are the receiver's own instantiation, which the
        // probe already resolved, so the template key is the *type template*
        // plus the method name - and the instance's key is what the call names.
        None => match method_template_key(ctx, &owner, method) {
            Some(tmpl_key) => {
                instantiate_function(ctx, &tmpl_key, &[], args, span, expected, Some(&probe.ty))?
            }
            None => match container_elem.as_ref().and_then(|e| {
                let elem_sortable = ctx.value_sortable(e);
                let elem_clonable = ctx.value_clonable(e);
                container_sig(
                    &key,
                    e,
                    &container_key,
                    crate::types::array_len_of(&probe.ty),
                    elem_sortable,
                    elem_clonable,
                )
            }) {
                Some(s) => (key, s),
                None => match builtin_sig(&key) {
                    Some(s) => (key, s),
                    None if blanket_tmpl.is_some() => {
                        let found = blanket_tmpl.take().expect("the guard saw a blanket method");
                        let tmpl_key = found?;
                        instantiate_function(ctx, &tmpl_key, &[], args, span, expected, Some(&probe.ty))?
                    }
                    None => {
                        let base = match (&container_elem, owner.as_str()) {
                // A destructor type is not `Copy` (kernel stage 9,
                // MK9-S6-M1): the derived copy fixpoint excludes it, so this
                // names the destructor rather than reporting a field that
                // cannot be copied.
                (Some(Ty::Struct(n)), "List")
                    if method == "clone" && ctx.env.declared_drops.contains(n) =>
                {
                    format!(
                        "List<{n}>.clone is not defined: '{n}' declares 'fun mut {n}.drop()', and a type with a destructor is not `Copy`, so a second owner of the same resource would be freed twice"
                    )
                }
                // Three List methods are defined only for an element type that
                // can be ordered, compared, or copied, so the generic "no
                // method" text would be misleading for exactly these three.
                (Some(e), "List")
                    if matches!(e, Ty::Struct(_))
                        && matches!(method, "sort" | "contains" | "slice" | "clone") =>
                {
                    format!(
                        "List<{}>.{method} is not defined: '{}' has no {}, and a 'datac'/'enum' takes one from its fields, so every field it holds has to have it too - use an element whose fields all do",
                        e.display(),
                        e.display(),
                        match method {
                            "sort" => "order",
                            "contains" => "equality",
                            _ => "copy",
                        }
                    )
                }
                (Some(e), "List") => format!(
                    "List<{}> has no method '{method}' - the List surface is len, is_empty, push, get, set, pop, clear, sort, reverse, contains, slice, clone, insert, remove",
                    e.display()
                ),
                (Some(e), "Map") => format!(
                    "Map<{}> has no method '{method}' - the Map surface is len, is_empty, has, get, get_or, insert, remove, clear, keys, values",
                    e.display()
                ),
                (Some(e), "Array") => format!(
                    "[{}; N] has no method '{method}': the array surface is len, is_empty, get, set",
                    e.display()
                ),
                (Some(e), _) => format!(
                    "Box<{}> has no method '{method}' - the Box surface is is_empty, get, set, take",
                    e.display()
                ),
                // An `Atomic<T>` with no such method: `bool` has the five
                // conversions' worth of surface and nothing more, so the
                // generic "datac 'Atomic'" text would name a datac that does
                // not exist.
                (None, "Atomic") => {
                    let elem = crate::types::atomic_elem_of(&probe.ty)
                        .expect("an Atomic receiver names its element");
                    if elem.is_integer() {
                        format!(
                            "Atomic<{}> has no method '{method}': the atomic surface is new, load, store, swap, compare_exchange, fetch_add, fetch_sub, fetch_and, fetch_or, fetch_xor",
                            elem.display()
                        )
                    } else {
                        format!(
                            "Atomic<{}> has no method '{method}': the atomic surface is new, load, store, swap and compare_exchange, and 'bool' has no atomic arithmetic, so there is no fetch_add/fetch_sub/fetch_and/fetch_or/fetch_xor for it",
                            elem.display()
                        )
                    }
                }
                (None, _) if probe.ty.is_integer() => format!(
                    "type '{}' has no method '{method}': an integer has to_i32, to_i64, to_u32, to_u64, to_u8, to_u16, to_usize, to_isize, to_f64, to_f32, swap_bytes, to_be and to_le, wrapping_shl and wrapping_shr, plus a try_to_<T> twin of every conversion that can lose a value (which hands back 'Option<T>'), and no other type converts",
                    probe.ty.display()
                ),
                // An `enum` and a `datac` miss the same way, and an enum
                // binding reaches this arm since its methods are resolved
                // like any other type's, so the text names the kind it is.
                (None, _) if ctx.is_enum(&owner) => {
                    format!("enum '{owner}' has no method '{method}'")
                }
                (None, _) => format!("datac '{owner}' has no method '{method}'"),
                        };
                        return Err(TypeError {
                            message: format!(
                                "{base}{}",
                                shadowed_module_alias_note(receiver, method, ctx)
                            ),
                            span,
                        });
                    }
                },
            },
        },
    };

    // The capability gate applies to a *method* of a gated module exactly as
    // it does to a free function of one: the key is `Type#method` with the
    // receiver type's module prefix, and the gate table holds it. A method of
    // a gated module reaches the resource without naming any of the module's
    // free functions (`Child.kill` is `kill_raw` plus an error check), so a
    // call that skipped this check reached the syscall with no grant at all.
    ctx.check_capability(&key, span)?;
    // An `unsafe fun` receiver method (MK9-S2 stage 1) is callable only from
    // an `unsafe` context, exactly as a free one is.
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            message: format!(
                "'{owner}.{method}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'"
            ),
            span,
        });
    }
    let mut checked_args = Vec::with_capacity(sig.params.len());
    ctx.keep_probe(receiver, probe);
    checked_args.push(check_call_arg("'self'", &sig.params[0].1, receiver, ctx)?);
    checked_args.extend(match_args(&format!("method '{owner}.{method}'"), &sig.params[1..], args, span, ctx)?);
    // A method whose result borrows from its receiver (`s.as_str()`) has the
    // same rule as a free function.
    require_place_for_borrowed_arg(&sig.params, &sig.ret, &checked_args, span)?;
    // Receiver methods can reference passdowns too; the call site passes the
    // same hidden arguments a free-function call would.
    checked_args.extend(hidden_args(&key, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked_args },
        ty: sig.ret,
        span,
    })
}

/// A method call through a borrowed trait object (A-12 slice 4,
/// `plan/traits.md` 9.4): resolve the method in the trait's flattened list, check
/// the receiver's mutability against the method's, check the arguments against
/// the trait's own signature, and lower to a `tir::ExprKind::DynCall`.
fn check_dyn_method_call(
    receiver: tir::Expr,
    trait_name: &str,
    mutable: bool,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let (slot, decl) = dyn_method_slot(&ctx.env.traits, trait_name, method, span)?;
    let receiver_mut = decl.receiver.as_ref().is_some_and(|r| r.mutable);
    // The declaration is cloned so the borrow of the trait table ends before
    // the call lowerer needs `ctx` mutably; a method declaration is small and
    // this is one call site.
    let decl = decl.clone();
    if receiver_mut && !mutable {
        return Err(TypeError {
            message: format!(
                "'{trait_name}.{method}' takes an exclusive receiver, and a shared '&dyn {trait_name}' cannot call it; make the receiver '&mut dyn {trait_name}'"
            ),
            span,
        });
    }
    lower_dyn_call(receiver, trait_name, &decl, slot, args, span, false, ctx)
}

/// A method call on an owned trait object (A-12 slice 7). The two names the box
/// itself provides are `is_empty` and `get`; every other name is the trait's
/// method, reached through the box's vtable exactly as it is through a `&dyn`,
/// with one difference: a `mut Self` method needs a writable place, because the
/// box owns what it points at rather than borrowing it.
fn check_box_dyn_method_call(
    receiver: tir::Expr,
    trait_name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    // A `&Box<dyn Trait>` (a `mut Box<dyn Trait>` parameter's borrow included)
    // reaches the pair through the borrow, exactly as `*p` does.
    let (recv, borrowed_mut) = match receiver.ty.clone() {
        Ty::Ref { mutable, inner } if matches!(*inner, Ty::BoxDyn { .. }) => {
            let pair = tir::Expr {
                kind: tir::ExprKind::Deref {
                    expr: Box::new(receiver),
                },
                ty: *inner,
                span,
            };
            (pair, Some(mutable))
        }
        _ => (receiver, None),
    };
    if method == "is_empty" {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'is_empty' takes no arguments, found {}", args.len()),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::BoxDynIsEmpty {
                expr: Box::new(recv),
            },
            ty: Ty::Bool,
            span,
        });
    }
    if method == "get" {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'get' takes no arguments, found {}", args.len()),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::BoxDynBorrow {
                mutable: false,
                expr: Box::new(recv),
            },
            ty: Ty::Dyn {
                mutable: false,
                trait_name: trait_name.to_string(),
            },
            span,
        });
    }
    // `take`/`set` would have to move a sized value in or out of the box, and
    // the concrete type behind an owned trait object is exactly what is erased,
    // so both are refused by name with the spelling that works instead.
    if method == "take" || method == "set" {
        return Err(TypeError {
            message: format!(
                "'Box<dyn {trait_name}>.{method}' is not allowed: the concrete type behind an owned trait object is erased, so no sized value can be moved in or out; borrow the trait with 'get()' and call its methods, or keep the boxed type concrete"
            ),
            span,
        });
    }
    let (slot, decl) = dyn_method_slot(&ctx.env.traits, trait_name, method, span)?;
    let receiver_mut = decl.receiver.as_ref().is_some_and(|r| r.mutable);
    let decl = decl.clone();
    if receiver_mut {
        match borrowed_mut {
            Some(true) => {}
            Some(false) => {
                return Err(TypeError {
                    message: format!(
                        "'{trait_name}.{method}' takes an exclusive receiver, and a shared '&Box<dyn {trait_name}>' cannot call it; take the box by 'mut' or make the binding writable"
                    ),
                    span,
                })
            }
            None => check_target_writable(&recv, ctx)?,
        }
    }
    lower_dyn_call(recv, trait_name, &decl, slot, args, span, receiver_mut, ctx)
}

/// The vtable slot and declaration of one trait method, or the refusal that
/// names the methods the trait does have.
fn dyn_method_slot<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    trait_name: &str,
    method: &str,
    span: Span,
) -> Result<(usize, &'a ast::FunctionDecl)> {
    let mut flat = Vec::new();
    flattened_trait_methods(traits, trait_name, &mut flat);
    let Some((slot, decl)) = flat.iter().enumerate().find(|(_, m)| m.name == method) else {
        let names: Vec<&str> = flat.iter().map(|m| m.name.as_str()).collect();
        return Err(TypeError {
            message: format!(
                "'dyn {trait_name}' has no method '{method}'; its methods are {}",
                names.join(", ")
            ),
            span,
        });
    };
    Ok((slot, decl))
}

/// The shared tail of both trait-object method calls: check the arguments
/// against the trait method's own signature and lower to a `DynCall`.
#[allow(clippy::too_many_arguments)]
fn lower_dyn_call(
    receiver: tir::Expr,
    trait_name: &str,
    decl: &ast::FunctionDecl,
    slot: usize,
    args: &[ast::CallArg],
    span: Span,
    exclusive: bool,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    // The argument types come from the trait declaration, resolved in the
    // trait's own module. `Self` has no concrete meaning here, but object
    // safety already refused every method that mentions it outside the
    // receiver, so a placeholder is never observed; `self_ty` only reaches
    // the receiver's own type, which this drops.
    let info = ctx
        .env
        .traits
        .get(trait_name)
        .expect("the receiver's own trait is in the table");
    let sig = resolve_trait_method_sig(
        info,
        decl,
        &Ty::Unit,
        &[],
        &ctx.structs,
        &ctx.enums,
        &ctx.env,
        &ctx.consts,
        &ctx.const_fns,
    )?;
    let params: Vec<(String, Ty)> = sig.params[1..].to_vec();
    let checked = match_args(
        &format!("method '{trait_name}.{}'", decl.name),
        &params,
        args,
        span,
        ctx,
    )?;
    let ret = sig.ret;
    Ok(tir::Expr {
        kind: tir::ExprKind::DynCall {
            receiver: Box::new(receiver),
            slot,
            args: checked,
            exclusive,
        },
        ty: ret,
        span,
    })
}

/// The `(source, target)` pair a `try_to_<T>` method call names, or `None`
/// when the receiver and the method do not spell one (MISSING-RbhdW).
///
/// Only the pairs whose plain `to_<T>` twin can lose a value have a twin at
/// all: a widening cannot fail, so `u8.try_to_u64()` is deliberately not a
/// method. `Ty::int_bounds` is the one place that decides, and it is the same
/// bound table `codegen_c.rs` uses to decide whether `to_<T>` needs a check, so
/// the two can never disagree about which conversions are fallible.
fn try_to_pair(receiver: &Ty, method: &str) -> Option<(Ty, Ty)> {
    let src = Ty::from_name(builtin_type_name(receiver)?)?;
    let dst = Ty::from_name(method.strip_prefix("try_to_")?)?;
    if !src.is_integer() || !dst.is_integer() {
        return None;
    }
    let (s_min, s_max) = Ty::int_bounds(&src)?;
    let (t_min, t_max) = Ty::int_bounds(&dst)?;
    if s_min >= t_min && s_max <= t_max {
        return None;
    }
    Some((src, dst))
}

/// Lowers `n.try_to_u8()` (MISSING-RbhdW) into the shape the language can
/// already express: the receiver bound once, a test for each bound its source
/// range cannot rule out, and one `Option` arm per outcome. Deliberately not a
/// new `tir::ExprKind`: both backends already lower `Block`, `If` and `EnumLit`,
/// so the fallible conversion is one pass's work with no conversion-specific
/// code in either code generator.
///
/// The `Option` is the stdlib's own, found by the module id rather than by the
/// enum's name, because a builtin call site has no declaration the resolver
/// could have qualified the type at. A program with no `std/option` anywhere in
/// its graph gets told to import it rather than an internal error.
fn lower_try_to(src: &Ty, dst: &Ty, value: tir::Expr, span: Span, ctx: &Ctx) -> Result<tir::Expr> {
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            message: format!(
                "'{}.try_to_{}' hands back 'Option<{}>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                src.display(),
                dst.display(),
                dst.display(),
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    let opt_ty = ctx.instantiate_type(&option_key, &[dst.clone()], span)?;
    let opt_name = struct_name_of(&opt_ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&opt_name).expect("just instantiated");
    // Recognized by variant name and arity, never by the enum's own name, the
    // same rule `?` uses (see `try_shape`).
    let Some(shape) = try_shape(&edef).filter(|s| !s.err_has_payload) else {
        return Err(TypeError {
            message: format!(
                "'{}' declares '{option_key}', and it is not the two-variant 'Some'/'None' shape this conversion hands back",
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    };
    let src_name = builtin_type_name(src).expect("an integer has a name");
    let dst_name = builtin_type_name(dst).expect("an integer has a name");
    let (s_min, s_max) = Ty::int_bounds(src).expect("a try_to_ source is an integer");
    let (t_min, t_max) = Ty::int_bounds(dst).expect("a try_to_ target is an integer");
    // The receiver is read once, into a binding of its own: the test and the
    // payload both read it, so `f().try_to_u8()` must not run `f` twice. The
    // name carries a `.`, which no source identifier can contain, so it cannot
    // shadow a binding the program wrote and no program can shadow it.
    let held = format!("__try_to.{}", span.start);
    let read = || tir::Expr {
        kind: tir::ExprKind::Ident(held.clone()),
        ty: src.clone(),
        span,
    };
    let lit = |v: i128| tir::Expr {
        kind: tir::ExprKind::IntLit(v),
        ty: src.clone(),
        span,
    };
    let compare = |op: BinOp, rhs: i128| tir::Expr {
        kind: tir::ExprKind::Binary {
            op,
            lhs: Box::new(read()),
            rhs: Box::new(lit(rhs)),
        },
        ty: Ty::Bool,
        span,
    };
    // Only the bounds the source's own range cannot rule out, exactly as
    // `numeric_conversion` drops them: `u8.try_to_i32` has no upper test to
    // write, and a test that cannot fail would be a tautology in the C.
    let mut checks: Option<tir::Expr> = None;
    for check in [
        (s_min < t_min).then(|| compare(BinOp::Lt, t_min)),
        (s_max > t_max).then(|| compare(BinOp::Gt, t_max as i128)),
    ]
    .into_iter()
    .flatten()
    {
        checks = Some(match checks.take() {
            None => check,
            Some(prev) => tir::Expr {
                kind: tir::ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(prev),
                    rhs: Box::new(check),
                },
                ty: Ty::Bool,
                span,
            },
        });
    }
    let cond = checks.expect("a try_to_ pair fails at least one bound");
    let variant = |variant: usize, fields: Vec<tir::Expr>| tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: opt_name.clone(),
            variant,
            fields,
        },
        ty: opt_ty.clone(),
        span,
    };
    let none = variant(shape.err, Vec::new());
    let some = variant(
        shape.ok,
        vec![tir::Expr {
            kind: tir::ExprKind::Call {
                callee: format!("{src_name}#to_{dst_name}"),
                args: vec![read()],
            },
            ty: dst.clone(),
            span,
        }],
    );
    // An out-of-range pair is `None`, and the `Some` arm is the unchecked
    // payload: the conversion inside it is the plain `to_<T>`, whose own range
    // check the test above has already made unreachable.
    let arms = tir::Expr {
        kind: tir::ExprKind::If {
            cond: Box::new(cond),
            then_block: tir::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(none)),
            },
            else_block: Some(tir::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(some)),
            }),
        },
        ty: opt_ty.clone(),
        span,
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: held,
                ty: src.clone(),
                init: value,
            }],
            tail: Some(Box::new(arms)),
        }),
        ty: opt_ty,
        span,
    })
}

/// `m.to_int()` (kernel stage 2): the declared discriminant of the variant
/// the value holds, as the enum's `repr(<int>)` type (default `i32`). Lowered
/// to the shape `when` already lowers to: a `EnumTag` test per variant, each
/// arm an integer literal, with the receiver read once through a hidden local,
/// so `f().to_int()` calls `f` once. Only a field-less enum has declared
/// values to report.
fn lower_enum_to_int(
    mode_ty: Ty,
    receiver: &ast::Expr,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let ename = struct_name_of(&mode_ty)
        .expect("an enum receiver names a struct type")
        .to_string();
    let edef = ctx
        .enum_def(&ename)
        .expect("the receiver was checked against a declared enum");
    if !edef.is_field_less() {
        return Err(TypeError {
            message: format!(
                "enum '{ename}' has a variant with a payload, so it has no declared integer value: 'to_int' is on a field-less enum"
            ),
            span,
        });
    }
    let tag = edef.tag_ty();
    // The receiver is read through its address for each tag test. A place has
    // one already and is read repeatedly, which is what an `EnumTag` does; an
    // rvalue (`E.A`, a call) is moved into a local of its own first, because a
    // borrow of a bare rvalue does not outlive the statement that made it.
    let probe = check_expr(receiver, ctx, Some(&mode_ty))?;
    let (base, stage) = if is_durable_place(&probe) {
        (probe, None)
    } else {
        // The name carries a `.`, which no source identifier can contain, so
        // it cannot shadow a binding the program wrote.
        let held = format!("__to_int.{}", span.start);
        (
            tir::Expr {
                kind: tir::ExprKind::Ident(held.clone()),
                ty: mode_ty.clone(),
                span,
            },
            Some(tir::Stmt::Let {
                name: held,
                ty: mode_ty.clone(),
                init: probe,
            }),
        )
    };
    let values = edef.discriminants();
    let int = |v: i128| tir::Expr {
        kind: tir::ExprKind::IntLit(v),
        ty: tag.clone(),
        span,
    };
    let last = values.len() - 1;
    let mut chain = int(values[last]);
    for i in (0..last).rev() {
        chain = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(tir::Expr {
                    kind: tir::ExprKind::EnumTag {
                        expr: Box::new(base.clone()),
                        variant: i,
                    },
                    ty: Ty::Bool,
                    span,
                }),
                then_block: tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(int(values[i]))),
                },
                else_block: Some(tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(chain)),
                }),
            },
            ty: tag.clone(),
            span,
        };
    }
    let stmts: Vec<tir::Stmt> = stage.into_iter().collect();
    if stmts.is_empty() {
        return Ok(chain);
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts,
            tail: Some(Box::new(chain)),
        }),
        ty: tag,
        span,
    })
}

/// `Mode.from_int(n)` (kernel stage 2): the variant whose discriminant is `n`,
/// or `None`. Lowered like [`lower_try_to`]: the argument is bound once into a
/// hidden local and the result is one `if n == value { Some(variant) }` arm
/// per variant, falling through to `None`. Requires the `Option` the
/// stdlib declares, exactly as the fallible conversions do.
fn lower_enum_from_int(
    ename: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let edef = ctx
        .enum_def(ename)
        .expect("enum_path only returns known enums");
    if !edef.is_field_less() {
        return Err(TypeError {
            message: format!(
                "enum '{ename}' has a variant with a payload, so it has no declared integer value: 'from_int' is on a field-less enum"
            ),
            span,
        });
    }
    let tag = edef.tag_ty();
    let checked = match_args(
        &format!("'{ename}.from_int'"),
        &[("n".to_string(), tag.clone())],
        args,
        span,
        ctx,
    )?;
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            message: format!(
                "'{ename}.from_int' hands back 'Option<{ename}>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    let mode_ty = Ty::Struct(ename.to_string());
    let opt_ty = ctx.instantiate_type(&option_key, &[mode_ty.clone()], span)?;
    let opt_name = struct_name_of(&opt_ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let odef = ctx.enum_def(&opt_name).expect("just instantiated");
    let Some(shape) = try_shape(&odef).filter(|s| !s.err_has_payload) else {
        return Err(TypeError {
            message: format!(
                "'{}' declares '{option_key}', and it is not the two-variant 'Some'/'None' shape this conversion hands back",
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    };
    let held = format!("__from_int.{}", span.start);
    let read = || tir::Expr {
        kind: tir::ExprKind::Ident(held.clone()),
        ty: tag.clone(),
        span,
    };
    let variant = |variant: usize, fields: Vec<tir::Expr>| tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: opt_name.clone(),
            variant,
            fields,
        },
        ty: opt_ty.clone(),
        span,
    };
    let none = variant(shape.err, Vec::new());
    let values = edef.discriminants();
    let mut chain = none;
    for i in (0..values.len()).rev() {
        let arm = variant(
            shape.ok,
            vec![tir::Expr {
                kind: tir::ExprKind::EnumLit {
                    enum_name: ename.to_string(),
                    variant: i,
                    fields: Vec::new(),
                },
                ty: mode_ty.clone(),
                span,
            }],
        );
        chain = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(tir::Expr {
                    kind: tir::ExprKind::Binary {
                        op: BinOp::Eq,
                        lhs: Box::new(read()),
                        rhs: Box::new(tir::Expr {
                            kind: tir::ExprKind::IntLit(values[i]),
                            ty: tag.clone(),
                            span,
                        }),
                    },
                    ty: Ty::Bool,
                    span,
                }),
                then_block: tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(arm)),
                },
                else_block: Some(tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(chain)),
                }),
            },
            ty: opt_ty.clone(),
            span,
        };
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: held,
                ty: tag,
                init: checked[0].clone(),
            }],
            tail: Some(Box::new(chain)),
        }),
        ty: opt_ty,
        span,
    })
}

/// The associated constructors of the three builtin containers, written on
/// the type name: `List.of(x)`/`List.empty()`, `Box.of(x)`/`Box.empty()`,
/// and `Map.new()`/`Map.empty()`/`Map.with_capacity(n)`. Each needs its
/// element type from somewhere - `of` takes it from its argument (or from
/// the binding's written type, which is what `expected` is for),
/// `empty`/`new` only from the written type, and `with_capacity` from the
/// written type as well because its argument is a count, not an element.
fn check_container_associated(
    container: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Box.empty()` on an owned trait object (A-12 slice 7): the null fat pair,
    // whose trait only the binding's written type can supply. There is no
    // element type to take it from, so it is answered here rather than through
    // the element machinery below.
    if container == "Box" {
        if let Some(want @ Ty::BoxDyn { .. }) = expected {
            if method != "empty" {
                return Err(TypeError {
                    message: format!(
                        "'Box.{method}' is not an operation of an owned trait object: '{}' is made from 'Box.of(v)' where 'v' implements the trait (or 'Box.empty()'), and used through its methods",
                        want.display()
                    ),
                    span,
                });
            }
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'Box.empty' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "BoxDyn#empty".to_string(),
                    args: Vec::new(),
                },
                ty: want.clone(),
                span,
            });
        }
    }
    let expected_elem = expected
        .and_then(|t| match container {
            "List" => crate::types::list_elem_of(t),
            "Map" => crate::types::map_elem_of(t),
            _ => crate::types::box_elem_of(t),
        })
        .cloned();
    // A `Map.new()`/`Map.empty()` also needs its *key* type, which only the
    // binding's written type can supply; `Map<V>` written there means a
    // `String` key, which is what `map_key_of` already reports.
    let expected_key = (container == "Map")
        .then(|| expected.and_then(crate::types::map_key_of).cloned())
        .flatten();
    let wrap = |elem: Ty| match container {
        "List" => Ty::List(Box::new(elem)),
        "Map" => Ty::Map(
            Box::new(expected_key.clone().unwrap_or(Ty::String)),
            Box::new(elem),
        ),
        _ => Ty::Box(Box::new(elem)),
    };
    // What a bare `List.empty()`/`Box.empty()`/`Map.new()` with no type to
    // take its element from becomes: the container with `Unit` where the
    // element goes, which no written type can spell, so a later `push` on the
    // binding can fill it in (`types::is_open_container`). A map's key is
    // open the same way when the binding named neither.
    let open = || match container {
        "List" => Ty::List(Box::new(Ty::Unit)),
        "Map" => Ty::Map(Box::new(Ty::Unit), Box::new(Ty::Unit)),
        _ => Ty::Box(Box::new(Ty::Unit)),
    };
    // `Map.empty()` and `Map.new()` are one operation - a map starts empty -
    // so they lower to one key. They used to lower to two and codegen only
    // ever emitted `__new`, so `Map.empty()` compiled to a call to a function
    // that did not exist; a `datac`/`enum` value type is not what exposed it,
    // just what asked for it first.
    let key = if container == "Map" && method == "empty" {
        "Map#new".to_string()
    } else {
        format!("{container}#{method}")
    };
    match method {
        "new" if container == "Map" => {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'Map.new' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let Some(value) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    message: "cannot tell what 'Map.new()' holds, write the type on the binding ('var m: Map<String, i32> = Map.new()'); there is no 'Map.of(x)' since a map starts empty".to_string(),
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![] },
                ty: wrap(value),
                span,
            })
        }
        // `Map.with_capacity(n)` is `Map.new()` that sizes both buffers up
        // front, which is the only way a caller can pay the growth once: a
        // map that fills three million entries otherwise rebuilds its index a
        // dozen times on the way there. The count is a hint, never a fence:
        // the map grows past it like any other.
        "with_capacity" if container == "Map" || container == "List" => {
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "'{container}.with_capacity' takes exactly 1 argument (the {}), found {}",
                        if container == "Map" { "entry count" } else { "element count" },
                        args.len()
                    ),
                    span,
                });
            }
            let cap = check_expr(&args[0].value, ctx, Some(&Ty::Usize))?;
            if !matches!(cap.ty, Ty::Usize | Ty::I64) {
                return Err(TypeError {
                    message: format!(
                        "'{container}.with_capacity' needs a 'usize' or 'i64' count, found '{}'",
                        cap.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            let Some(value) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![cap] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    message: match container {
                        "Map" => "cannot tell what 'Map.with_capacity(n)' holds, write the type on the binding ('var m: Map<String, i32> = Map.with_capacity(n)')".to_string(),
                        _ => "cannot tell what 'List.with_capacity(n)' holds, write the element type on the binding ('var xs: List<i32> = List.with_capacity(n)')".to_string(),
                    },
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![cap] },
                ty: wrap(value),
                span,
            })
        }
        "empty" => {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'{container}.empty' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let Some(elem) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    message: match container {
                        "Map" => "cannot tell what 'Map.empty()' holds, write the type on the binding ('var m: Map<String, i32> = Map.empty()')".to_string(),
                        _ => format!(
                            "cannot tell what '{container}.empty()' holds, write the element type on the binding ('var xs: {container}<String> = {container}.empty()') or start from '{container}.of(x)'"
                        ),
                    },
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![] },
                ty: wrap(elem),
                span,
            })
        }
        "of" if container == "Map" => Err(TypeError {
            message: "'Map.of' does not exist - a map starts empty; write 'Map.new()' with the value type on the binding, then 'insert(k, v)'".to_string(),
            span,
        }),
        "of" => {
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!("'{container}.of' takes exactly 1 argument, found {}", args.len()),
                    span,
                });
            }
            let value = check_expr(&args[0].value, ctx, expected_elem.as_ref())?;
            let allowed = if container == "List" {
                crate::types::is_list_element(&value.ty)
            } else if container == "Map" {
                crate::types::is_map_element(&value.ty)
            } else {
                crate::types::is_box_element(&value.ty)
            };
            if !allowed {
                return Err(TypeError {
                    message: if container == "List" {
                        list_element_error(&value.ty)
                    } else if container == "Map" {
                        map_element_error(&value.ty)
                    } else {
                        box_element_error(&value.ty)
                    },
                    span: args[0].value.span(),
                });
            }
            let elem = value.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![value] },
                ty: wrap(elem),
                span,
            })
        }
        other => Err(TypeError {
            message: format!(
                "'{container}.{other}' is not a {container} operation - {}",
                if container == "List" {
                    "write 'List.of(x)' for a one-element list, 'List.empty()' or 'List.with_capacity(n)' with the element type on the binding, or call a method on a value ('xs.push(x)', 'xs.get(i)', 'xs.len()')"
                } else if container == "Map" {
                    "a map starts empty - write 'Map.new()' with the type on the binding ('var m: Map<String, i32> = Map.new()'), then call a method on the value ('m.insert(k, v)', 'm.get(k)', 'm.len()')"
                } else {
                    "write 'Box.of(x)' for a one-value box, 'Box.empty()' with the element type on the binding, or call a method on a value ('b.get()', 'b.set(v)', 'b.take()', 'b.is_empty()')"
                }
            ),
            span,
        }),
    }
}

/// The one constructor of the builtin atomic cell (MK9-S3-M1 stage A1):
/// `Atomic.new(x)` and `AtomicI64.new(0)`.
///
/// The element type comes from the named instantiation (`AtomicBool`), from the
/// binding's written type (`val c: Atomic<i64> = Atomic.new(0)`), or from the
/// argument itself, in that order, the same three sources `List.of(x)` has, so
/// the ordinary integer literal default answers `val c = Atomic.new(0)` and a
/// written type overrides it. There is no `empty`: an atomic always holds a
/// value, and a zero-initialized one is `Atomic.new(0)`.
fn check_atomic_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            message: format!(
                "'{name}.{method}' is not an 'Atomic' operation: an atomic is made with 'Atomic.new(x)' (or 'AtomicI64.new(0)' for a named instantiation) and read through the value it is bound to ('c.load(order = ...)', 'c.store(value = x, order = ...)')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            message: format!("'Atomic.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let named = crate::types::atomic_alias_elem(name);
    let written = expected.and_then(crate::types::atomic_elem_of).cloned();
    let from_position = named.clone().or_else(|| written.clone());
    let value = check_expr(&args[0].value, ctx, from_position.as_ref())?;
    let elem = match from_position {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    message: format!(
                        "'Atomic<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        // Nothing said which element type, so the argument's own type is the
        // answer: `Atomic.new(true)` is `Atomic<bool>`.
        None => value.ty.clone(),
    };
    if !crate::types::is_atomic_element(&elem) {
        return Err(TypeError {
            message: atomic_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Atomic#new".to_string(), args: vec![value] },
        ty: Ty::Atomic(Box::new(elem)),
        span,
    })
}

/// `Mutex.new(value)` (B-5, `std/sync`): the one constructor of the builtin
/// mutex. The value is moved in, exactly as a `move T` parameter moves it, and
/// the element type comes from the binding's written type or from the
/// argument, in that order: the same two sources `Atomic.new` has.
fn check_mutex_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            message: format!(
                "'{name}.{method}' is not a 'Mutex' operation: a mutex is made with 'Mutex.new(value)' and reached through the value it is bound to ('m.lock {{ v -> ... }}', 'm.into_inner()')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            message: format!("'Mutex.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let written = expected.and_then(crate::types::mutex_elem_of).cloned();
    let value = check_expr(&args[0].value, ctx, written.as_ref())?;
    if value.ty == Ty::Unit {
        return Err(TypeError {
            message: "'Mutex.new' needs a value to guard, found 'unit'".to_string(),
            span: args[0].value.span(),
        });
    }
    let elem = match written {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    message: format!(
                        "'Mutex<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        None => value.ty.clone(),
    };
    if !crate::types::is_mutex_element(&elem) {
        return Err(TypeError {
            message: mutex_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Mutex#new".to_string(), args: vec![value] },
        ty: Ty::Mutex(Box::new(elem)),
        span,
    })
}

/// A method on a `Mutex<T>` (B-5). `lock` is checked structurally, because its
/// block's return type comes from the block rather than from a fixed signature;
/// `into_inner` consumes the receiver and hands back the guarded value.
fn check_mutex_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    elem: &Ty,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let _ = probe;
    let cell = Ty::Mutex(Box::new(elem.clone()));
    match method {
        "lock" => check_mutex_lock(receiver, elem, args, span, ctx, expected),
        // `m.wait_while(cv, pred) { v -> ... }` and its timed twin: the
        // condition variable's own `wait` cannot be spelled with a scoped
        // guard, so the wait lives on the mutex, which owns the lock the wait
        // has to release atomically (B-5 rem, `plan/sync.md`).
        "wait_while" => check_mutex_wait(receiver, elem, args, span, ctx, expected, false),
        "wait_while_timeout" => {
            check_mutex_wait(receiver, elem, args, span, ctx, expected, true)
        }
        // `m.share()`: another owned handle to the same runtime record, so a
        // channel's two ends or a pool's workers can each own one. The
        // record is freed when the last handle drops.
        "share" => {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'Mutex.share' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg(
                "'self'",
                &Ty::Ref { mutable: false, inner: Box::new(cell.clone()) },
                receiver,
                ctx,
            )?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#share".to_string(),
                    args: vec![self_arg],
                },
                ty: cell,
                span,
            })
        }
        // `m.swap(value)`: put `value` in as the guarded value and take the old
        // one out, in one locked step. A scoped block cannot capture an owned
        // value by move, so this is how one is handed to a mutex from inside
        // a block (a channel's `send` stages the value in a private mutex and
        // swaps it across).
        "swap" => {
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!("'Mutex.swap' takes exactly 1 argument, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg(
                "'self'",
                &Ty::Ref { mutable: false, inner: Box::new(cell.clone()) },
                receiver,
                ctx,
            )?;
            let value = check_call_arg("'value'", elem, &args[0].value, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#swap".to_string(),
                    args: vec![self_arg, value],
                },
                ty: elem.clone(),
                span,
            })
        }
        "into_inner" => {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'Mutex.into_inner' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            // The receiver is consumed: the mutex is dead once its value is
            // out, exactly as `thread.join` consumes a handle.
            let self_arg = check_call_arg("'self'", &cell, receiver, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#into_inner".to_string(),
                    args: vec![self_arg],
                },
                ty: elem.clone(),
                span,
            })
        }
        other => Err(TypeError {
            message: format!(
                "'Mutex.{other}' is not a mutex operation: write 'm.lock {{ v -> ... }}' to reach the value while the lock is held, 'm.share()' for another handle to the same mutex, 'm.swap(value)' to exchange the guarded value, or 'm.into_inner()' to consume the mutex and take the value out"
            ),
            span,
        }),
    }
}

/// `m.lock { v -> ... }` (B-5): the mutex is held for the block's whole run, the
/// block receives `v: mut T`, its value is returned, and the runtime releases
/// the lock on every way out (a normal exit, a `return`/`break` inside the
/// block, and a panic, which poisons the mutex). The block's return type is the
/// block's own tail type, which is why the literal is checked with the
/// `infer_scoped_ret` flag set.
fn check_mutex_lock(
    receiver: &ast::Expr,
    elem: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots("method 'Mutex.lock'", &["block".to_string()], args, span)?;
    let block_arg = &args[slots[0]];
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref { mutable: false, inner: Box::new(Ty::Mutex(Box::new(elem.clone()))) },
        receiver,
        ctx,
    )?;
    // The block takes `mut T`, an exclusive borrow valid only inside the call,
    // which the existing scoped-block rules already refuse to store or return.
    let want = Ty::ScopedFn {
        params: vec![Ty::Ref { mutable: true, inner: Box::new(elem.clone()) }],
        ret: Box::new(expected.cloned().unwrap_or(Ty::Unit)),
    };
    // The block's return is its tail type when the call site names no type
    // (`print(m.lock { v -> *v })`), and the call's own expected type when it
    // does (`val r: Result<_, _> = m.lock { v -> ... ? ... }`), which is what
    // lets a `?` inside the block propagate the block's declared return.
    ctx.infer_scoped_ret = expected.is_none();
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        // A literal that captured nothing is a plain `fun` value: widen it to
        // the fat pointer the runtime calls, exactly as a block passed to any
        // other `scoped fun(...)` parameter is.
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                message: format!(
                    "the block 'Mutex.lock' takes has type '{}', expected a scoped function taking 'mut {}'",
                    other.display(),
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let ret = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: "Mutex#lock".to_string(),
            args: vec![self_arg, block],
        },
        ty: ret,
        span,
    })
}

/// The `Option<T>` a scoped-block call hands back when the block may not run,
/// named through `std/option`'s key exactly as `try_to_<T>` names it, so a
/// program that never imported the module is told to rather than given an
/// internal error.
fn option_ty(ctx: &Ctx, elem: Ty, span: Span) -> Result<Ty> {
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            message: format!(
                "this call hands back 'Option<T>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    ctx.instantiate_type(&option_key, &[elem], span)
}

/// `RwLock.new(value)` (`std/sync`): the one constructor of the builtin
/// reader/writer lock. The value is moved in, exactly as `Mutex.new` moves its
/// value, and the element type comes from the binding's written type or from
/// the argument, in that order.
fn check_rwlock_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            message: format!(
                "'{name}.{method}' is not an 'RwLock' operation: an rwlock is made with 'RwLock.new(value)' and reached through the value it is bound to ('rw.read {{ v -> ... }}', 'rw.write {{ v -> ... }}', 'rw.into_inner()')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            message: format!("'RwLock.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let written = expected.and_then(crate::types::rwlock_elem_of).cloned();
    let value = check_expr(&args[0].value, ctx, written.as_ref())?;
    if value.ty == Ty::Unit {
        return Err(TypeError {
            message: "'RwLock.new' needs a value to guard, found 'unit'".to_string(),
            span: args[0].value.span(),
        });
    }
    let elem = match written {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    message: format!(
                        "'RwLock<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        None => value.ty.clone(),
    };
    if !crate::types::is_rwlock_element(&elem) {
        return Err(TypeError {
            message: rwlock_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "RwLock#new".to_string(), args: vec![value] },
        ty: Ty::RwLock(Box::new(elem)),
        span,
    })
}

/// A method on an `RwLock<T>` (`std/sync`). `read`/`write` are checked
/// structurally, because each block's return type comes from the block rather
/// than from a fixed signature; `try_read`/`try_write` wrap that value in an
/// `Option` (`None` when the lock was busy and the block did not run).
fn check_rwlock_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    elem: &Ty,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let _ = probe;
    let cell = Ty::RwLock(Box::new(elem.clone()));
    match method {
        "read" => check_rwlock_block(receiver, elem, false, false, args, span, ctx, expected),
        "write" => check_rwlock_block(receiver, elem, true, false, args, span, ctx, expected),
        "try_read" => check_rwlock_block(receiver, elem, false, true, args, span, ctx, expected),
        "try_write" => check_rwlock_block(receiver, elem, true, true, args, span, ctx, expected),
        "into_inner" => {
            if !args.is_empty() {
                return Err(TypeError {
                    message: format!("'RwLock.into_inner' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg("'self'", &cell, receiver, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "RwLock#into_inner".to_string(),
                    args: vec![self_arg],
                },
                ty: elem.clone(),
                span,
            })
        }
        other => Err(TypeError {
            message: format!(
                "'RwLock.{other}' is not an rwlock operation: write 'rw.read {{ v -> ... }}' or 'rw.write {{ v -> ... }}' to reach the value with the lock held, 'rw.try_read'/'.try_write' for the non-blocking form, or 'rw.into_inner()' to consume it"
            ),
            span,
        }),
    }
}

/// The shared body of `read`/`write`/`try_read`/`try_write`: the receiver is
/// `&RwLock<T>`, the block is a `scoped fun` taking `&T` (read) or `mut T`
/// (write), and `try` wraps the block's value in `Option`.
#[allow(clippy::too_many_arguments)]
fn check_rwlock_block(
    receiver: &ast::Expr,
    elem: &Ty,
    write: bool,
    try_lock: bool,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let name = if try_lock {
        if write { "try_write" } else { "try_read" }
    } else if write {
        "write"
    } else {
        "read"
    };
    let slots = arg_slots(&format!("method 'RwLock.{name}'"), &["block".to_string()], args, span)?;
    let block_arg = &args[slots[0]];
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::RwLock(Box::new(elem.clone()))),
        },
        receiver,
        ctx,
    )?;
    let param = Ty::Ref { mutable: write, inner: Box::new(elem.clone()) };
    let want = Ty::ScopedFn {
        params: vec![param],
        ret: Box::new(expected.cloned().unwrap_or(Ty::Unit)),
    };
    // `read`/`write` return the block's tail type when the call site names no
    // type, exactly as `Mutex.lock` does. `try_*` always infer, because the
    // call's own type is `Option<R>` and the block's is `R`.
    ctx.infer_scoped_ret = try_lock || expected.is_none();
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                message: format!(
                    "the block 'RwLock.{name}' takes has type '{}', expected a scoped function taking '{} {}'",
                    other.display(),
                    if write { "mut" } else { "&" },
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let ret = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    let ty = if try_lock { option_ty(ctx, ret, span)? } else { ret };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("RwLock#{name}"),
            args: vec![self_arg, block],
        },
        ty,
        span,
    })
}

/// `Condvar.new()` (`std/sync`): the one constructor of the builtin condition
/// variable. It takes no argument: a condition variable guards no value of its
/// own; it is paired with one `Mutex<T>` at each `wait_while`.
fn check_condvar_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    _ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            message: format!(
                "'{name}.{method}' is not a 'Condvar' operation: a condition variable is made with 'Condvar.new()' and reached through the value it is bound to ('cv.notify_one()')"
            ),
            span,
        });
    }
    if !args.is_empty() {
        return Err(TypeError {
            message: format!("'Condvar.new' takes no arguments, found {}", args.len()),
            span,
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Condvar#new".to_string(), args: vec![] },
        ty: Ty::Condvar,
        span,
    })
}

/// `cv.notify_one()` and `cv.notify_all()`: the two ways to wake a waiter.
/// There is no bare `wait`; the wait is `Mutex.wait_while` (B-5 rem,
/// `plan/sync.md`).
fn check_condvar_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let _ = probe;
    if method == "share" {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'Condvar.share' takes no arguments, found {}", args.len()),
                span,
            });
        }
        let self_arg = check_call_arg("'self'", &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) }, receiver, ctx)?;
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call { callee: "Condvar#share".to_string(), args: vec![self_arg] },
            ty: Ty::Condvar,
            span,
        });
    }
    if method != "notify_one" && method != "notify_all" {
        return Err(TypeError {
            message: format!(
                "'Condvar.{method}' is not a condition variable operation: write 'cv.notify_one()' or 'cv.notify_all()' to wake a waiter; a wait is 'm.wait_while(cv, pred) {{ v -> ... }}' on the mutex that guards the state"
            ),
            span,
        });
    }
    if !args.is_empty() {
        return Err(TypeError {
            message: format!("'Condvar.{method}' takes no arguments, found {}", args.len()),
            span,
        });
    }
    let self_arg = check_call_arg("'self'", &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) }, receiver, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("Condvar#{method}"),
            args: vec![self_arg],
        },
        ty: Ty::Unit,
        span,
    })
}

/// `m.wait_while(cv, pred) { v -> ... }` and its timed twin (`std/sync`). The
/// mutex is held for the block's whole run; the wait releases it around every
/// sleep and re-tests `pred(&T)` after each wakeup, so a spurious wakeup never
/// reaches the caller. The timed form hands back `Option<R>`: `None` when the
/// deadline passed with the predicate still true and the block did not run.
#[allow(clippy::too_many_arguments)]
fn check_mutex_wait(
    receiver: &ast::Expr,
    elem: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
    timed: bool,
) -> Result<tir::Expr> {
    let method = if timed { "wait_while_timeout" } else { "wait_while" };
    let names: Vec<String> = if timed {
        vec!["cv".into(), "pred".into(), "millis".into(), "block".into()]
    } else {
        vec!["cv".into(), "pred".into(), "block".into()]
    };
    let slots = arg_slots(&format!("method 'Mutex.{method}'"), &names, args, span)?;
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Mutex(Box::new(elem.clone()))),
        },
        receiver,
        ctx,
    )?;
    let cv_arg = check_call_arg(
        "'cv'",
        &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) },
        &args[slots[0]].value,
        ctx,
    )?;
    // The predicate is a `fun(&T).return: bool` (or a closure of that shape): a
    // parameter is already a shared borrow in Kelp, so its resolved type is
    // `T` for a `Copy` element and `&T` for an owned one
    // (`implicit_borrow_wrap`), and the runtime hands the generated body the
    // element's address accordingly. It is called with the lock held, once
    // before the first wait and again after every wakeup.
    let pred_param = implicit_borrow_wrap(elem.clone(), false);
    let pred_want = Ty::Closure {
        params: vec![pred_param.clone()],
        ret: Box::new(Ty::Bool),
    };
    let pred_expr = check_expr(&args[slots[1]].value, ctx, None)?;
    let pred = match pred_expr.ty.clone() {
        Ty::Closure { params, ret } if params == pred_want_params(&pred_want) && *ret == Ty::Bool => pred_expr,
        Ty::Fn { params, ret }
            if params == pred_want_params(&pred_want) && *ret == Ty::Bool =>
        {
            coerce_fn_to_closure(pred_want, pred_expr)
        }
        other => {
            return Err(TypeError {
                message: format!(
                    "the predicate 'Mutex.{method}' takes has type '{}', expected 'fun({}).return: bool'",
                    other.display(),
                    pred_param.display()
                ),
                span: args[slots[1]].value.span(),
            })
        }
    };
    let millis = if timed {
        Some(check_call_arg("'millis'", &Ty::I64, &args[slots[2]].value, ctx)?)
    } else {
        None
    };
    let block_arg = &args[slots[if timed { 3 } else { 2 }]];
    let want = Ty::ScopedFn {
        params: vec![Ty::Ref { mutable: true, inner: Box::new(elem.clone()) }],
        ret: Box::new(Ty::Unit),
    };
    ctx.infer_scoped_ret = true;
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                message: format!(
                    "the block 'Mutex.{method}' takes has type '{}', expected a scoped function taking 'mut {}'",
                    other.display(),
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let inner = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    let mut call_args = vec![self_arg, cv_arg, pred];
    if let Some(millis) = millis {
        call_args.push(millis);
    }
    call_args.push(block);
    let ty = if timed { option_ty(ctx, inner, span)? } else { inner };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("Mutex#{method}"),
            args: call_args,
        },
        ty,
        span,
    })
}

/// The `&T` parameter a wait predicate takes. Pulled out so the two shape
/// matches above stay readable.
fn pred_want_params(want: &Ty) -> Vec<Ty> {
    match want {
        Ty::Closure { params, .. } => params.clone(),
        _ => unreachable!("the caller built a closure shape"),
    }
}

/// A bare name that only exists as an imported item's *qualified* name is a
/// very likely strict-namespace mistake (M5: imported items are reached
/// only as `alias.Item`). Returns a "did you mean ..." fragment when `name`
/// matches some qualified key's last segment.
/// Only an exported key is offered (A-13): suggesting the qualified spelling
/// of an item another module keeps private would tell the reader to write a
/// line that the visibility gate refuses.
fn qualified_hint<'a>(
    name: &str,
    keys: impl Iterator<Item = &'a String>,
    aliases: &HashMap<String, String>,
    env: &TypeEnv,
) -> Option<String> {
    let want = format!(".{name}");
    let key = keys
        .into_iter()
        .find(|k| k.ends_with(&want) && k.contains('.') && exported(k, env))?;
    let (module_path, _) = key.rsplit_once('.')?;
    let qualified_prefix = format!("{module_path}.");
    Some(match aliases.iter().find(|(_, p)| p.as_str() == qualified_prefix) {
        Some((alias, _)) => format!("'{alias}.{name}'"),
        None => {
            let import_path = module_path.replace('.', "/");
            format!(
                "'{name}' is declared in module '{import_path}' - add `impl \"{import_path}\"` and reach it as `<alias>.{name}`"
            )
        }
    })
}

/// `alias.item(args)` - a reference to an item in an imported module, per
/// M5's strict module-qualified namespace. `prefix` is that module's symbol
/// prefix, so the actual lookup key is just `prefix + item`: the same
/// table `check_call` uses for any other call, which is why imported
/// `datac` construction and imported free functions need no new lowering.
fn check_module_item(
    alias: &str,
    prefix: &str,
    item: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let qualified = format!("{prefix}{item}");
    // Private by default (A-13), and this is the gate: the item is known to
    // exist here, so an unexported one is refused as private rather than
    // reported as missing.
    if declares(&qualified, ctx) {
        ctx.check_exported(&qualified, alias, item, span)?;
    }
    // A capability value's constructor is sealed to its own module (kernel
    // stage 18, `plan/irq-frame-caps.md`): the shipped library exports every
    // item and Kelp has no field-level `pub`, so `irq.Line(n = 3)` would forge
    // the receipt for a grant. Only `irq.claim` and `frame.claim` may build
    // one, and they are reached as functions, not as the type's own name.
    if crate::stdlib::is_sealed_type(&qualified) {
        let module = crate::stdlib::sealed_type_module(&qualified).unwrap_or("its own module");
        return Err(TypeError {
            message: format!(
                "'{alias}.{item}' is a capability value: its constructor is private to module '{module}', so another module cannot build one; take the value from a `claim` call instead"
            ),
            span,
        });
    }
    if ctx.structs.contains_key(&qualified)
        || ctx.env.generics.contains_key(&qualified)
        || ctx.sigs.contains_key(&qualified)
        || ctx.overloads.contains_key(&qualified)
        || ctx.fns.templates.contains_key(&qualified)
    {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    // A module's `const` is reached the same way as its functions: as
    // `math.PI` (M6.3). It takes no arguments, so a call-shaped reference to
    // one is worth a message of its own.
    if let Some(c) = ctx.consts.get(&qualified).cloned() {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'{alias}.{item}' is a constant, not a function"),
                span,
            });
        }
        let mut value = c.value;
        value.span = span;
        return Ok(value);
    }
    // A module's `static` is reached like its `const`, but it is an object
    // rather than a folded literal, so it takes no arguments and is a place.
    if let Some(s) = ctx.statics.get(&qualified).cloned() {
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("'{alias}.{item}' is a static object, not a function"),
                span,
            });
        }
        require_static_unsafe(&s, span, ctx)?;
        return Ok(tir::Expr {
            kind: tir::ExprKind::StaticRef(s.qualified),
            ty: s.ty,
            span,
        });
    }
    // A receiver method lives under a `Type#name` key, which a module path
    // can't spell - say so rather than "no such item".
    let method_suffix = format!("#{item}");
    if let Some(key) = ctx
        .sigs
        .keys()
        .find(|k| k.starts_with(prefix) && k.ends_with(&method_suffix))
    {
        let type_name = key.trim_end_matches(&method_suffix).trim_end_matches(prefix);
        return Err(TypeError {
            message: format!(
                "'{item}' is a method on '{type_name}', not a free function - call it on a value ('value.{item}(...)')"
            ),
            span,
        });
    }
    // A builtin free function a shipped module *documents* rather than
    // declares (`thread.fence`, MK9-S3-M1 stage A2): its signature is in the
    // same builtin table `panic`'s is, for the reason the module carries no
    // declaration (see the entry in `BUILTINS`). Checked last, so a real
    // declaration of the same key always wins.
    if builtin_sig(&qualified).is_some() {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    // `thread.spawn` and `thread.join` (MK9-S3-M1 stage B1) sit beside the
    // fence for the same reason, one step further: their signatures depend on
    // the value that crosses the boundary, so they are checked in `check_call`
    // rather than read from the table.
    if crate::stdlib::is_thread_fn(&qualified) {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    Err(TypeError {
        message: format!("module '{alias}' has no item '{item}'"),
        span,
    })
}

fn check_if(
    cond: &ast::Expr,
    then_block: &ast::Block,
    else_block: Option<&ast::ElseBranch>,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
    if cond.ty != Ty::Bool {
        return Err(TypeError {
            message: format!("if condition must be 'bool', found '{}'", cond.ty.display()),
            span,
        });
    }

    let then_t = check_block(then_block, ctx, expected)?;

    let else_t = match else_block {
        Some(ast::ElseBranch::Block(b)) => Some(check_block(b, ctx, expected)?),
        Some(ast::ElseBranch::If(e)) => {
            let checked = check_expr(e, ctx, expected)?;
            // The parser folds a plain `else if` into an `If`, but an
            // `else if let` desugars to the block `parse_if_let` builds (a
            // bound scrutinee and a pattern `when`), and that block is the
            // else branch's value exactly as an `If` is: wrap either one and
            // let the join below see a block. Assuming `If` here was an
            // internal error on every `else if let` a program wrote.
            Some(tir::Block {
                stmts: vec![],
                tail: Some(Box::new(checked)),
            })
        }
        None => None,
    };

    let ty = match &else_t {
        Some(e) => join_value_type(&then_t.ty(), &e.ty()).ok_or_else(|| TypeError {
            message: format!(
                "if/else branches have different types: '{}' and '{}'",
                then_t.ty().display(),
                e.ty().display()
            ),
            span,
        })?,
        None => {
            if then_t.ty() != Ty::Unit {
                return Err(TypeError {
                    message: "if without an else branch must evaluate to 'unit'".to_string(),
                    span,
                });
            }
            Ty::Unit
        }
    };

    Ok(tir::Expr {
        kind: tir::ExprKind::If {
            cond: Box::new(cond),
            then_block: then_t,
            else_block: else_t,
        },
        ty,
        span,
    })
}

#[cfg(test)]
mod tests {
    use super::is_shipped_module;

    /// The export-everything rule keys on this predicate, and a project file
    /// under a `std/` path is a project file: its id must not pass for the
    /// compiler's own text, or privacy stops applying to it (A-13).
    #[test]
    fn only_the_compilers_own_text_is_shipped() {
        assert!(is_shipped_module("std/mem"));
        assert!(is_shipped_module("std/list"));
        // The injected runtime (`stdlib::RT_MODULE`) ships like the rest.
        assert!(is_shipped_module(crate::stdlib::RT_MODULE));
        assert!(!is_shipped_module("std/mine"));
        assert!(!is_shipped_module("std"));
        assert!(!is_shipped_module("lib"));
    }
}

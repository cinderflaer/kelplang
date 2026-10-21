use super::*;

#[derive(Clone)]
pub(crate) struct VarInfo {
    pub(crate) ty: Ty,
    pub(crate) mutable: bool,
}

/// One user-declared generic type (A-2): a `datac` or `enum` written with
/// type parameters. Its fields are kept *as written* rather than resolved,
/// because a type parameter has no type until an instantiation binds one -
/// resolution happens in `TypeScope::instantiate`, with the parameters bound
/// to the concrete arguments.
#[derive(Clone)]
pub(crate) struct GenericTemplate {
    /// Qualified name, exactly the key a non-generic `datac` of the same name
    /// would have had.
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    /// The declared type of each `const` parameter of `params`, in declaration
    /// order (A-20). A name here is also in `params`; this is what says which
    /// entries are values and how wide their values are.
    pub(crate) const_params: Vec<(String, Ty)>,
    pub(crate) kind: GenericKind,
    /// The `repr(...)` clause of the declaration, so an instantiation keeps
    /// the layout its template asked for (kernel stage 2).
    pub(crate) repr: Repr,
    /// The declaring module's symbol prefix and alias table: a field type
    /// written inside the template resolves exactly where it was written.
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub(crate) enum GenericKind {
    Record(Vec<ast::FieldDecl>),
    Sum(Vec<ast::VariantDecl>),
}

/// One `trait Name { fun Self.m(...) ... }` declaration (A-12 slice 2). The
/// method signatures stay as written, because `Self` has no meaning until a
/// type implements the trait: `check_trait_impls` binds it per impl.
#[derive(Clone)]
pub(crate) struct TraitInfo {
    /// Qualified name, exactly the key a `datac` of the same name would have.
    pub(crate) name: String,
    /// `trait From<T>` (A-12 slice 6): the trait's own type parameters, in
    /// order. Empty for an ordinary trait.
    pub(crate) type_params: Vec<String>,
    /// The bound traits on each parameter of `type_params`, as
    /// `(written, canonical)` and in the same order, so an impl's argument can
    /// be checked against them. Empty list per parameter for no bound.
    pub(crate) type_param_bounds: Vec<Vec<(String, String)>>,
    /// The declaring module's symbol prefix and alias table, so a signature's
    /// types resolve where the trait was written rather than where it is
    /// implemented.
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
    /// The trait's own methods, in source order: a `FunctionBody::Signature`
    /// is required, any other body is a default an impl may omit (A-12 slice
    /// 3).
    pub(crate) methods: Vec<ast::FunctionDecl>,
    /// The supertraits, canonical and in source order (A-12 slice 3).
    /// Implementing this trait requires an impl of each, and a bound on this
    /// trait satisfies a bound on any of them.
    pub(crate) supertraits: Vec<String>,
    /// The associated types the trait declares (A-12 slice 5), in source order.
    pub(crate) assoc_types: Vec<AssocTypeInfo>,
    pub(crate) module: String,
}

/// One `type Item[: Bound + Bound]` a trait declares (A-12 slice 5).
#[derive(Clone)]
pub(crate) struct AssocTypeInfo {
    pub(crate) name: String,
    /// The bound traits, each `(written, canonical)`, in source order: the
    /// written name is what a diagnostic quotes, the canonical one is what the
    /// impl's type is checked against.
    pub(crate) bounds: Vec<(String, String)>,
}

/// The concrete `Ty` of each trait argument, from the canonical names
/// `resolve.rs` computed (A-12 slice 6). A canonical name is a `Ty::Struct` key
/// unless it is a builtin, so no second scope is needed.
pub(crate) fn trait_arg_tys(names: &[String]) -> Vec<Ty> {
    names
        .iter()
        .map(|n| Ty::from_name(n).unwrap_or_else(|| Ty::Struct(n.clone())))
        .collect()
}

/// The `sigs` key one impl method is stored under (A-12 slice 6): a receiver
/// method is `Type#name`, and a static member carries its trait instance
/// (`Type#name@Trait<Args>`), which is what lets one type implement
/// `From<A>` and `From<B>` at once.
pub(crate) fn impl_method_key(imp: &TraitImplInfo, m: &ast::FunctionDecl) -> String {
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
pub(crate) struct TraitImplInfo {
    /// The canonical trait name (`pkg.From`), i.e. the trait *declaration*'s
    /// key, not the instance's.
    pub(crate) trait_name: String,
    /// The canonical trait-instance key (A-12 slice 6): `pkg.From<pkg.IoErr>`
    /// for a generic trait, the trait name alone for an ordinary one.
    pub(crate) trait_instance: String,
    /// The canonical name of each trait argument (`pkg.IoErr`, `i32`), in
    /// order, so the concrete `Ty` the trait's parameters are substituted with
    /// is built with no second scope.
    pub(crate) trait_arg_names: Vec<String>,
    pub(crate) type_name: String,
    pub(crate) methods: Vec<String>,
    /// `type Item = i32` (A-12 slice 5), in source order.
    pub(crate) assoc_types: Vec<(String, ast::TypeExpr, Span)>,
    pub(crate) module: String,
    pub(crate) impl_span: Span,
    /// `impl<T: Bound> Trait for Type<T>`: the impl's own parameters (the generic
    /// type's, in order) and the bounds on each. Empty for a concrete type.
    pub(crate) type_params: Vec<String>,
    pub(crate) type_param_bounds: Vec<Vec<ast::TypeExpr>>,
    /// `where I::Item: Bound` on the impl (MISSING-Ve8Nc step 5).
    pub(crate) projection_bounds: Vec<ast::ProjectionBound>,
}

/// One generic impl, registered for `implements_trait`: an instance of
/// `template` implements the trait when every bound on the impl's parameters
/// holds for the instance's argument at that position.
#[derive(Clone)]
pub(crate) struct GenericImpl {
    pub(crate) template: String,
    /// The trait key each parameter must implement, one list per parameter.
    pub(crate) bounds: Vec<Vec<String>>,
    /// `type Item = I::Item`, as written, over the impl's parameters.
    pub(crate) assoc_types: Vec<(String, ast::TypeExpr)>,
    pub(crate) params: Vec<String>,
    /// The equality constraints (`I: Iterator<Item = i64>`) on each parameter,
    /// as `(assoc, type)`, one list per parameter (MISSING-Ve8Nc step 5).
    pub(crate) eqs: Vec<Vec<(String, ast::TypeExpr)>>,
    /// The `where I::Item: Bound` clauses, as `(parameter index, assoc, trait
    /// key)`.
    pub(crate) projections: Vec<(usize, String, String)>,
    /// The impl's module prefix and alias table, so a constraint's right-hand
    /// name resolves where the impl was written.
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
}

/// One blanket impl (`impl<T: Bound> Trait for T`), registered for
/// `implements_trait` and for method lookup: every type that meets `bounds`
/// implements `trait_key`, and its methods are templates keyed `impl#Trait#name`
/// (`types::blanket_receiver_name`).
#[derive(Clone)]
pub(crate) struct BlanketImpl {
    pub(crate) trait_key: String,
    pub(crate) trait_name: String,
    /// The trait keys the parameter must implement.
    pub(crate) bounds: Vec<String>,
    /// The method names the block writes, which is what lookup searches.
    pub(crate) methods: Vec<String>,
    pub(crate) module: String,
    pub(crate) impl_span: Span,
}

/// One built instantiation, on its way into the instance table.
pub(crate) enum Instance {
    Record(StructDef),
    Sum(EnumDef),
}

/// Concrete instantiations created on demand while checking (A-2), plus the
/// in-progress marks that turn a recursive instance (`Node<i32>` holding
/// `Box<Node<i32>>`) into a terminating lookup instead of infinite recursion.
#[derive(Default)]
pub(crate) struct InstanceState {
    pub(crate) records: HashMap<String, StructDef>,
    pub(crate) sums: HashMap<String, EnumDef>,
    /// Instance key -> the template it came from and the arguments it was
    /// built with, so an expected type can supply type parameters a
    /// construction cannot infer from its own arguments (`Option.None`).
    pub(crate) origin: HashMap<String, (String, Vec<Ty>)>,
    pub(crate) in_progress: std::collections::HashSet<String>,
}

/// Tidepool: records that a value of `ty` goes into the heap at `slot`.
/// The check itself runs after the pass, in `managed::check_heap_uses`. Hosted
/// builds record nothing.
pub(crate) fn note_heap_use(env: &TypeEnv, slot: managed::HeapSlot, ty: &Ty, span: Span) {
    if env.managed {
        env.heap_uses.borrow_mut().push(managed::HeapUse { slot, ty: ty.clone(), span });
    }
}

/// The program's generic templates and the instances built so far. Shared by
/// reference between the `Ctx` clones each function gets, so an instantiation
/// discovered while checking one body is visible to every other - and to the
/// final `tir::Program`.
#[derive(Clone, Default)]
pub(crate) struct TypeEnv {
    pub(crate) generics: std::rc::Rc<HashMap<String, GenericTemplate>>,
    pub(crate) instances: std::rc::Rc<std::cell::RefCell<InstanceState>>,
    /// The user types that declare their own `Map` key pair (A-12). Decided
    /// once, from the whole function list, because a `Map<T, V>` in a
    /// *declaration* is resolved before the signatures are: the field of a
    /// `datac` that names such a type has to see the same key set a body
    /// does.
    pub(crate) declared_keys: std::rc::Rc<std::collections::HashSet<String>>,
    /// The user types that declare their own destructor (kernel stage 9,
    /// MK9-S6-M1): a `datac`/`enum` with `fun mut T.drop()`. Decided once, from
    /// the whole function list, for the same reason `declared_keys` is: a
    /// `datac` field or a `List<T>` element type is resolved before the
    /// signatures are, and the derived-copy fixpoints those resolutions ask
    /// must already know which types a destructor keeps out of them.
    pub(crate) declared_drops: std::rc::Rc<std::collections::HashSet<String>>,
    /// Tidepool heap (`managed.rs`): whether this program is checked for the
    /// collected heap. Off in hosted and freestanding, where `heap_uses` stays
    /// empty and no check runs.
    pub(crate) managed: bool,
    /// Tidepool: every value that goes into a heap slot, recorded during
    /// the pass and decided after it (`managed::check_heap_uses`).
    pub(crate) heap_uses: std::rc::Rc<std::cell::RefCell<Vec<managed::HeapUse>>>,
    /// The canonical keys of the items each module exports (A-13). Private by
    /// default, so a key is here only for a declaration written `pub`, or for
    /// any declaration in a compiler-shipped module (the transitional rule
    /// `design/syntax.md (kelp wiki)` 13.4 records). Shared through the same `Rc` every
    /// `Ctx` clone and every `TypeScope` see, since it is one answer for the
    /// whole program.
    pub(crate) exported: std::rc::Rc<std::collections::HashSet<String>>,
    /// The canonical names of the program's `extern type` handles (gap 1). One
    /// answer for the whole program, like `exported`: a handle declared in one
    /// module is named through an alias in another, and its name has to mean
    /// the same thing in every scope.
    pub(crate) handles: std::rc::Rc<std::collections::HashSet<String>>,
    /// Handle parent relation (gap 1): a canonical child name -> its canonical
    /// parent name, for the `extern type Name: Parent` declarations. One answer
    /// for the whole program, exactly as `handles` is, so the implicit upcast
    /// answers the same in every scope.
    pub(crate) handle_parents: std::rc::Rc<HashMap<String, String>>,
    /// The canonical keys of the program's variadic user `extern fun`s
    /// (gap 7) -> the module each was declared in. One answer for the whole
    /// program, so a call site knows both that a callee takes extra arguments
    /// and which module the generated wrapper belongs to.
    pub(crate) variadic_externs: std::rc::Rc<HashMap<String, String>>,
    /// The program's traits, keyed by qualified name (A-12 slice 2). One
    /// answer for the whole program: the same trait is implemented in one
    /// module and bounded in another.
    pub(crate) traits: std::rc::Rc<HashMap<String, TraitInfo>>,
    /// Trait key -> the qualified types that implement it. This is what a
    /// bound at a call site is checked against, and it is one set for the
    /// whole program because an impl may live in the trait's module while the
    /// call site is elsewhere. The key is the trait *instance* (A-12 slice 6),
    /// so `From<i32>` and `From<String>` are two entries.
    pub(crate) impls: std::rc::Rc<HashMap<String, std::collections::HashSet<String>>>,
    /// Every `impl` record, in source order (A-12 slice 6): a static method call
    /// searches this for the impl whose trait declares the method and whose
    /// substituted signatures match the arguments.
    pub(crate) trait_impls: std::rc::Rc<Vec<TraitImplInfo>>,
    /// The `(trait, type)` pairs a `&dyn` coercion used (A-12 slice 4),
    /// collected while bodies are checked. Each one gets a vtable in
    /// `tir::Program`, and a pair no coercion names gets none.
    pub(crate) dyn_pairs: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<(String, String)>>>,
    /// Every trait named by a `dyn` type, borrowed or owned (A-12 slices 4
    /// and 7), collected where the type is written. The fat-pair typedef and
    /// the vtable tag are emitted per *trait*, so a `dyn` type that is only
    /// named (a parameter nothing coerces to) still needs its C type to exist.
    pub(crate) dyn_traits: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
    /// The associated-type binding each `impl` declares (A-12 slice 5), keyed
    /// by `(trait, type)` and then by the associated type's name. One answer
    /// for the whole program, exactly as `impls` is, so a projection at a call
    /// site and the impl's own signature agree.
    pub(crate) assoc: std::rc::Rc<HashMap<(String, String), HashMap<String, Ty>>>,
    /// Generic types that are iterators by shape: the type template declares
    /// `fun mut Name<P, ...>.next() => option.Option<Item>`, where `Item` is
    /// a written type over the template's own parameters. Keyed by template
    /// name. `std/iter`'s adapters are such types (`TakeIter<I, T>`), and this is
    /// what lets one satisfy `I: Iterator` and name `I::Item` without a generic
    /// trait impl (`kelp issue #243`).
    pub(crate) iter_nexts: std::rc::Rc<HashMap<String, IterNext>>,
    /// The generic impls (`impl<T: Bound> Trait for Type<T>`), by trait-instance
    /// key.
    pub(crate) generic_impls: std::rc::Rc<HashMap<String, Vec<GenericImpl>>>,
    /// The blanket impls (`impl<T: Bound> Trait for T`), in source order. Coherence
    /// (`check_trait_impl_overlap`) allows at most one per trait, and no other impl
    /// of that trait.
    pub(crate) blanket_impls: std::rc::Rc<Vec<BlanketImpl>>,
}

/// See `TypeEnv::iter_nexts`.
#[derive(Clone)]
pub(crate) struct IterNext {
    /// The template's type parameters, in order.
    pub(crate) params: Vec<String>,
    /// The `Item` of the `option.Option<Item>` `next` returns, as written.
    pub(crate) item: ast::TypeExpr,
}

/// One user-declared generic *function* (A-2): a `fun` with its own type
/// parameters (`fun first<T>(xs: List<T>) => T`) or a receiver method on a
/// generic type (`fun Pair<A, B>.first() => A`). Its parameter and return
/// types are kept as written, because a type parameter has no type until an
/// instantiation binds it - exactly like `GenericTemplate`.
#[derive(Clone)]
pub(crate) struct FuncTemplate {
    /// Declaration key: the prefixed name, or `Type#name` for a receiver.
    pub(crate) key: String,
    /// The method's type parameters, in order, which for a receiver method are
    /// the receiver's own arguments.
    pub(crate) params: Vec<String>,
    /// The declared type of each `const` parameter of `params` (A-20). See
    /// `GenericTemplate::const_params`.
    pub(crate) const_params: Vec<(String, Ty)>,
    /// `fun f<T: A + B>`: the traits each type parameter must implement, one
    /// list per entry of `params` and in the same order (A-12 slice 2). A
    /// parameter with no bound has an empty list, and a receiver method's
    /// parameters are its type's, so its lists are all empty.
    pub(crate) bounds: Vec<Vec<ast::TypeExpr>>,
    /// `where I::Item: Bound` (MISSING-Ve8Nc step 5): checked at each call on
    /// the concrete associated type.
    pub(crate) projection_bounds: Vec<ast::ProjectionBound>,
    /// `Some` for a receiver method: the canonical receiver type name (already
    /// prefixed) and whether the receiver is `mut`.
    pub(crate) receiver: Option<(String, bool)>,
    /// Declared parameters, without the receiver.
    pub(crate) declared: Vec<ast::Param>,
    pub(crate) ret: Option<ast::TypeExpr>,
    /// The declaring module's symbol prefix and alias table, so a parameter or
    /// return type written inside the template resolves where it was written.
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
    /// Index in `program.functions`, so a diagnostic can name the declaration.
    pub(crate) ast_index: usize,
}

/// The concrete signatures created on demand (A-2) and the queue of bodies
/// still to check. `sigs` is what a call site resolves against once the
/// declaration is instantiated; `pending` is the worklist `check_program`
/// drains, because one instantiation's body can create another.
#[derive(Default)]
pub(crate) struct FunctionState {
    pub(crate) sigs: HashMap<String, FnSig>,
    /// Instance key -> the template key it came from, for passdown wiring.
    pub(crate) origin: HashMap<String, String>,
    /// Instances whose bodies still have to be checked, in creation order
    /// alongside the type arguments they were built with.
    pub(crate) pending: std::collections::VecDeque<(String, Vec<Ty>)>,
    /// The synthesized `tir::Function` for each `fun` literal checked (A-4).
    /// A literal has no declaration in the AST, so `infer.rs` is what creates
    /// its function; the bodies are collected here and appended to the
    /// program's function list once checking is done. Shared through
    /// `FuncEnv`'s `Rc`, so a literal discovered inside another literal's body
    /// lands in the same table.
    pub(crate) literals: Vec<tir::Function>,
    /// How many literals have been synthesized, for the next one's name.
    pub(crate) literal_count: usize,
    /// The fixed-arity C wrappers one variadic user `extern fun` call site
    /// needs (gap 7), keyed by the wrapper's own name so two call sites that
    /// pass the same list of extra types share one. Each entry carries the
    /// extern's key so the pass that assembles the program can point the
    /// wrapper at the same native symbol the extern binds.
    pub(crate) variadic_wrappers: Vec<(String, tir::Function)>,
    pub(crate) variadic_wrappers_seen: std::collections::HashSet<String>,
}

/// The program's function templates and their instances (A-2), shared between
/// every `Ctx` clone for the same reason `TypeEnv` is.
#[derive(Clone, Default)]
pub(crate) struct FuncEnv {
    pub(crate) templates: std::rc::Rc<HashMap<String, FuncTemplate>>,
    pub(crate) instances: std::rc::Rc<std::cell::RefCell<FunctionState>>,
}

/// How many generic instantiations one program may create before the compiler
/// gives up. A generic function that calls itself with a different type
/// argument (`fun g<T>(x: T) => i32 = g(x = List.of(x))`) grows without bound;
/// this turns that into a diagnostic instead of a hang.
pub(crate) const MAX_FUNCTION_INSTANTIATIONS: usize = 2048;

/// The name prefix of the local a `when` stages its scrutinee into when the
/// scrutinee is a temporary. `#` cannot appear in a lexed identifier, so a
/// synthesized name can never collide with one the user wrote: the same trick
/// `parser::IF_LET_PREFIX` and a hidden passdown parameter use.
pub(crate) const WHEN_TEMP_PREFIX: &str = "#when";
pub(crate) const BORROW_TEMP_PREFIX: &str = "#borrow";

/// The name prefix of the local a destructuring binding takes its value apart
/// from, and of the local a *nested* pattern takes an inner tuple into
/// (`design/syntax.md (kelp wiki)` 4). Same `#` trick as `WHEN_TEMP_PREFIX`: a synthesized
/// name can never collide with one the user wrote.
pub(crate) const TUPLE_TEMP_PREFIX: &str = "#tuple";

/// The type parameters a function declaration introduces: its own for a free
/// function, the receiver's for a method on a generic type.
pub(crate) fn decl_type_params(f: &ast::FunctionDecl) -> Vec<String> {
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
pub(crate) fn const_param_tys(
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
pub(crate) fn bind_const_args(
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
                help: None,
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
                help: None,
                message: format!(
                    "const parameter '{name}' is a 'char', so its argument has to be a character literal or a 'char' const ('Tag<'x'>'), not a number"
                ),
                span,
            });
        }
        if !wants_char && arg_is_char {
            return Err(TypeError {
                help: None,
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
                    help: None,
                    message: format!(
                        "const parameter '{name}' is a 'bool', so its argument has to be 'true' or 'false'"
                    ),
                    span,
                });
            }
        } else if wants_char {
            if u32::try_from(value).ok().and_then(char::from_u32).is_none() {
                return Err(TypeError {
                    help: None,
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
                help: None,
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
pub(crate) fn const_param_in(
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
pub(crate) fn named_const_expr(ty: &ast::TypeExpr) -> Option<ast::Expr> {
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
pub(crate) fn instance_key(name: &str, args: &[Ty]) -> String {
    let parts: Vec<String> = args.iter().map(|a| a.display()).collect();
    format!("{}<{}>", name, parts.join(", "))
}

/// The type of a tuple, and the one place its derived record is registered
/// (MISSING-z2bB9).
///
/// A tuple is a record with positional fields (`design/syntax.md (kelp wiki)` 3) and v1's
/// record type is `Ty::Struct(<key>)`, keyed by the tuple as written. The
/// record is *derived*: `types::tuple_struct_def` builds it and it goes into
/// the same on-demand registry a generic instantiation uses (A-2), so every
/// later pass treats it as an ordinary declaration: the derived deep copy,
/// order and drop, the partial move of one position, and both backends'
/// struct emission all read the declaration table and need no tuple case of
/// their own. Idempotent: two use sites of one element list are one type.
pub(crate) fn tuple_ty(env: &TypeEnv, elems: Vec<Ty>) -> Ty {
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
pub(crate) fn tuple_elems_of(ty: &Ty, ctx: &Ctx) -> Option<Vec<Ty>> {
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

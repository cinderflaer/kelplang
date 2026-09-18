use std::collections::HashMap;

use crate::ast::{self, BinOp, UnOp};
use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::resolve::Project;
use crate::tir;
use crate::types::{FieldDef, FnSig, StructDef, Ty};

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
}

#[derive(Clone)]
struct VarInfo {
    ty: Ty,
    mutable: bool,
}

struct Ctx {
    sigs: HashMap<String, FnSig>,
    structs: HashMap<String, StructDef>,
    ret_ty: Ty,
    scopes: Vec<HashMap<String, VarInfo>>,
    /// Symbol prefix of the module being checked (`""` for the entry
    /// module, else `"shapes.circle."`) — a bare type name in this module
    /// resolves to `prefix + name`.
    prefix: String,
    /// alias -> target module's symbol prefix, for this module's `impl`
    /// lines (M5). This is also how a module-qualified *call* is
    /// recognized: `circle.make(...)` is a module item reference, not a
    /// method call, when `circle` is one of these aliases.
    aliases: HashMap<String, String>,
    /// This module's passdowns, keyed by the bare name a function writes.
    /// `syntax&shii.md` §9 leaves the cross-module question open; M5.2
    /// answers it the same way M5.1 answered imports — module-scoped, so
    /// only functions in the declaring module can name it.
    pds: HashMap<String, PdInfo>,
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
    /// True only while checking `main`'s body. `main` is emitted as
    /// `int main(void)` — it cannot receive hidden parameters — so its
    /// passdown references take the address of the generated global instead
    /// of naming a parameter. Every other function that references a
    /// passdown is wired and uses its parameter.
    pd_global_borrow: bool,
    /// Set while checking a receiver-form function whose receiver resolves
    /// to a `datac`: lets a bare identifier that isn't a local fall back to
    /// an implicit `self.field` (`fun Circle.area().R: f64 = 3.14159 * r *
    /// r` — no `self.` qualifier written, per `syntax&shii.md` §5's "no
    /// `self` keyword").
    receiver_struct: Option<String>,
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
    /// Whether it was declared `pd var` — used only for the wording of the
    /// "read-only inside a function" diagnostic.
    mutable: bool,
    /// The type it is handed over as: a shared borrow for non-`Copy` values
    /// (reading a `datac` passdown out of the global by value would "move"
    /// it on first use), the plain type otherwise. Always shared, never
    /// exclusive — a passdown is read-only inside a function.
    param_ty: Ty,
    module: String,
    /// Global initialization order (position in the merged program), used to
    /// keep hidden-parameter order and the generated init function stable.
    index: usize,
}

impl PdInfo {
    /// The argument a call site passes for this passdown's hidden parameter.
    fn arg(&self, span: Span) -> tir::Expr {
        let base = tir::Expr {
            kind: tir::ExprKind::Ident(self.hidden.clone()),
            ty: self.value_ty.clone(),
            span,
        };
        match &self.param_ty {
            Ty::Ref { .. } => tir::Expr {
                kind: tir::ExprKind::Ref {
                    mutable: false,
                    expr: Box::new(base),
                },
                ty: self.param_ty.clone(),
                span,
            },
            _ => base,
        }
    }
}

impl Ctx {
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

    /// Looks up `name` as a field of the current receiver's `datac`, if
    /// this function has one.
    fn receiver_field_ty(&self, name: &str) -> Option<Ty> {
        let sname = self.receiver_struct.as_deref()?;
        self.structs.get(sname)?.field(name).map(|f| f.ty.clone())
    }

    fn type_scope(&self) -> TypeScope<'_> {
        TypeScope {
            structs: &self.structs,
            prefix: &self.prefix,
            aliases: &self.aliases,
            // Every datac is known by the time bodies are checked.
            later_datacs: None,
        }
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

/// Everything needed to turn a *written* type name into a `Ty`, with M5's
/// module qualification rules:
/// - a builtin (`i32`, `f64`, `str`, ...) is itself;
/// - `alias.Item` resolves through the current module's `impl` table;
/// - a bare name belongs to the current module, so it gets that module's
///   prefix — imported types are never reachable unqualified (strict
///   module-qualified namespace, per plan.md).
struct TypeScope<'a> {
    structs: &'a HashMap<String, StructDef>,
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
}

impl TypeScope<'_> {
    fn resolve(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        if let Some(builtin) = Ty::from_name(&ty.name) {
            return Ok(builtin);
        }
        // A name that is already a known `struct` key is canonical: this is
        // how a receiver declaration (`fun Circle.area()` inside
        // `shapes/circle.x`) arrives here, since `resolve.rs` rewrites the
        // receiver type to its module-prefixed name when it merges modules.
        // Everything else is a name as the user wrote it and still needs
        // qualifying.
        if self.structs.contains_key(&ty.name) {
            return Ok(Ty::Struct(ty.name.clone()));
        }
        let qualified = match ty.name.split_once('.') {
            Some((alias, item)) => match self.aliases.get(alias) {
                Some(prefix) => format!("{prefix}{item}"),
                None => {
                    return Err(TypeError {
                        message: format!(
                            "unknown module alias '{alias}' in type '{}' — was it imported?",
                            ty.name
                        ),
                        span: ty.span,
                    })
                }
            },
            None => format!("{}{}", self.prefix, ty.name),
        };
        if self.structs.contains_key(&qualified) {
            Ok(Ty::Struct(qualified))
        } else if self
            .later_datacs
            .is_some_and(|later| later.contains(&qualified))
        {
            Err(TypeError {
                message: format!(
                    "'{}' is declared after this datac — a datac may only name datacs declared before it (v1 has no forward references between datacs)",
                    ty.name
                ),
                span: ty.span,
            })
        } else {
            Err(TypeError {
                message: match qualified_hint(&ty.name, self.structs.keys(), self.aliases) {
                    Some(hint) => format!("unknown type '{}' — did you mean {hint}?", ty.name),
                    None => format!("unknown type '{}'", ty.name),
                },
                span: ty.span,
            })
        }
    }

    fn resolve_param(&self, p: &ast::Param) -> Result<Ty> {
        Ok(implicit_borrow_wrap(self.resolve(&p.ty)?, p.mutable))
    }

    fn resolve_receiver(&self, r: &ast::Receiver) -> Result<Ty> {
        let te = ast::TypeExpr {
            name: r.type_name.clone(),
            span: r.span,
        };
        Ok(implicit_borrow_wrap(self.resolve(&te)?, r.mutable))
    }
}

/// A function's lookup key in `sigs`/`tir::Function.name`: a receiver-form
/// method is keyed `Type#name` (a receiver method and a free function can
/// share a bare name with no collision, and `#` can't appear in a real
/// identifier so it can't collide with a hand-written name either) — a
/// plain function is keyed by its bare name, unchanged from before
/// receivers existed.
fn sig_key(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}#{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

const PRINTABLE_BUILTINS: &[&str] = &["print", "print!"];

/// A compiler-provided function (M6). These live in `sigs`' namespace (keyed
/// `Type#name`, exactly like a receiver method) but their bodies are emitted
/// by `codegen_c.rs` from the fixed runtime preamble, so nothing about call
/// resolution, argument matching, or passdown wiring needs to know they are
/// special — only codegen, and only in that the C function it names already
/// exists.
struct Builtin {
    params: Vec<(String, Ty)>,
    ret: Ty,
    /// `Bytes.empty()` — an *associated* function written on the type name,
    /// with no receiver argument — versus `b.len()`, which takes one.
    associated: bool,
}

/// The `Bytes` surface (M6). Kept deliberately small: construct, measure,
/// index, mutate, copy. Everything else about `Bytes` needs either `String`
/// or growable buffers, which are later milestones.
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
        m
    });

/// The name a builtin type answers to in a method key — the non-`datac`
/// counterpart of `struct_name_of`.
fn builtin_type_name(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::Bytes => Some("Bytes"),
        // A `Bytes` parameter is a borrow (M4's rule: every owned parameter is
        // borrowed), so `b.len()` on one arrives here as `&Bytes`.
        Ty::Ref { inner, .. } => builtin_type_name(inner),
        _ => None,
    }
}

/// The signature a builtin key resolves to, if it is one.
fn builtin_sig(key: &str) -> Option<FnSig> {
    BUILTINS.get(key).map(|b| FnSig {
        params: b.params.clone(),
        ret: b.ret.clone(),
    })
}

/// Stand-in for "no later datacs" at the sites where every `datac` in the
/// program is already known (signatures, bodies, passdown types).
static EMPTY_DATACS: std::sync::LazyLock<std::collections::HashSet<String>> =
    std::sync::LazyLock::new(std::collections::HashSet::new);

pub fn check_program(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<tir::Program, Diagnostic> {
    let mut structs = HashMap::new();
    // Structs are resolved in declaration order, which is also the order
    // `codegen_c.rs` must emit their C typedefs in: a `datac` may only name
    // `datac`s declared before it (see `TypeScope::later_datacs`), so
    // declaration order is a valid topological order. Emitting them from a
    // `HashMap`'s iteration order instead — which is what this did before —
    // made a struct-embedding-struct program compile or fail *at random*
    // from one run to the next.
    let mut struct_order: Vec<String> = Vec::new();
    for (i, d) in program.datacs.iter().enumerate() {
        let later_datacs: std::collections::HashSet<String> = program
            .datacs
            .iter()
            .skip(i + 1)
            .map(|later| later.name.clone())
            .collect();
        if structs.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("datac '{}' is defined more than once", d.name),
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
                prefix,
                aliases: &aliases,
                later_datacs: Some(&later_datacs),
            }
            .resolve(&f.ty)
            .map_err(|e| e.in_module(&d.module))?;
            if ty.needs_drop() {
                return Err(Diagnostic::new(
                    format!(
                        "field '{}' has owned heap type '{}' — a datac cannot hold an owned value yet (dropping a struct is not implemented)",
                        f.name,
                        ty.display()
                    ),
                    f.span,
                    d.module.clone(),
                ));
            }
            if matches!(ty, Ty::Str | Ty::Ref { .. }) {
                return Err(Diagnostic::new(
                    format!(
                        "field '{}' is a borrowed type ('{}') — a datac holds owned data only in v1, so a '&str' or reference field is not expressible",
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
        struct_order.push(d.name.clone());
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields,
            },
        );
    }

    let mut sigs = HashMap::new();
    for f in &program.functions {
        let key = sig_key(f);
        if sigs.contains_key(&key) {
            let message = match &f.receiver {
                Some(r) => format!("method '{}.{}' is defined more than once", r.type_name, f.name),
                None => format!("function '{}' is defined more than once", f.name),
            };
            return Err(Diagnostic::new(message, f.span, f.module.clone()));
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
        let prefix = project.prefix_of(&f.module);
        let aliases = project.alias_prefixes_of(&f.module);
        let scope = TypeScope {
            structs: &structs,
            prefix,
            aliases: &aliases,
            later_datacs: None,
        };
        let mut params = Vec::new();
        if let Some(r) = &f.receiver {
            let ty = scope.resolve_receiver(r).map_err(|e| e.in_module(&f.module))?;
            if struct_name_of(&ty).is_none() {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is a builtin type — its methods are provided by the compiler and cannot be declared",
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
        // `.R: T` never goes through `resolve_param_type`, so a function's
        // return type is always a plain owned `Ty` — never `Ty::Ref`. That's
        // what makes the borrow checker's call-scoped release sound: no
        // surface syntax exists yet for a function to hand a borrow back to
        // its caller, so a borrow can never escape the call frame that took
        // it.
        let ret = match &f.return_type {
            Some(t) => scope.resolve(t).map_err(|e| e.in_module(&f.module))?,
            None => Ty::Unit,
        };
        sigs.insert(key, FnSig { params, ret });
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
    let all_pds = collect_pds(program, project, &sigs, &structs)?;
    let wired = scan_all(program, &all_pds, &structs);

    if !sigs.contains_key("main") {
        return Err(Diagnostic::new(
            "no 'main' function found".to_string(),
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
            project,
            &all_pds,
            &wired,
            &initialized,
        )
        .map_err(|e| e.in_module(&pd.module))?;
        initialized.insert(pd.hidden.clone());
        checked_pds.push(tir::Pd {
            name: pd.hidden.clone(),
            qualified: pd.qualified.clone(),
            value_ty: pd.value_ty.clone(),
            init,
        });
    }

    let mut functions = Vec::new();
    for f in &program.functions {
        let checked = check_function(f, &sigs, &structs, project, &all_pds, &wired)
            .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }
    Ok(tir::Program {
        structs: struct_order
            .iter()
            .map(|name| structs[name].clone())
            .collect(),
        pds: checked_pds,
        functions,
    })
}

/// The passdowns of one module, keyed by the bare name a function writes.
/// `before` limits it to passdowns declared earlier — used while checking a
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
                prefix,
                aliases: &aliases,
                later_datacs: None,
            }
            .resolve(t)
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
                    project,
                    &out,
                    None,
                    &HashMap::new(),
                );
                ctx.init_state = Some(out.iter().map(|p| p.hidden.clone()).collect());
                check_expr(&decl.init, &mut ctx, None)
                    .map_err(|e| e.in_module(&decl.module))?
                    .ty
            }
        };
        if value_ty.needs_drop() {
            return Err(Diagnostic::new(
                format!(
                    "passdown '{bare}' has owned heap type '{}' — a 'pd' cannot hold an owned value yet (there is no drop point at process exit)",
                    value_ty.display()
                ),
                decl.span,
                decl.module.clone(),
            ));
        }
        out.push(PdInfo {
            qualified: decl.name.clone(),
            name: bare,
            hidden: format!("pd#{}", decl.name),
            param_ty: implicit_borrow_wrap(value_ty.clone(), false),
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
    project: &Project,
    all: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    initialized: &std::collections::HashSet<String>,
) -> Result<tir::Expr> {
    let mut ctx = base_ctx(&pd.module, sigs, structs, project, all, Some(pd.index), wired);
    ctx.init_state = Some(initialized.clone());
    check_expr(init, &mut ctx, Some(&pd.value_ty))
}

fn base_ctx(
    module: &str,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    project: &Project,
    all_pds: &[PdInfo],
    before: Option<usize>,
    wired: &HashMap<String, Vec<PdInfo>>,
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
        ret_ty: Ty::Unit,
        scopes: vec![HashMap::new()],
        prefix: project.prefix_of(module).to_string(),
        aliases: project.alias_prefixes_of(module),
        pds,
        foreign_pds,
        wired: wired.clone(),
        init_state: None,
        pd_global_borrow: false,
        receiver_struct: None,
    }
}

/// sig key -> the passdowns a function references. Computed from the AST
/// before bodies are checked, because a call site has to know what its
/// callee needs in order to pass it.
fn scan_all(
    program: &ast::Program,
    all: &[PdInfo],
    structs: &HashMap<String, StructDef>,
) -> HashMap<String, Vec<PdInfo>> {
    let mut wired = HashMap::new();
    for f in &program.functions {
        // `main` gets no hidden parameters (see `Ctx::pd_global_borrow`), and
        // it is not callable, so it never needs an entry here either.
        if f.receiver.is_none() && f.name == "main" {
            continue;
        }
        let visible = pd_map(all, &f.module, None);
        let used = scan_pd_uses(f, &visible, structs);
        if !used.is_empty() {
            wired.insert(sig_key(f), used);
        }
    }
    wired
}

/// Scope-aware walk for the passdowns a function body actually names.
/// Shadowing is honored exactly as `check_expr` honors it: a parameter,
/// receiver field, or `val`/`var` in scope hides a passdown of the same name,
/// so a hidden name is not a passdown reference. A method call's name, a
/// field access's field name, and a call's callee are never passdown
/// references either — only bare identifiers are.
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
            ast::Stmt::While { cond, body, .. } => {
                self.scan_expr(cond);
                self.scan_block(body);
            }
            ast::Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.scan_expr(v);
                }
            }
            ast::Stmt::Expr { expr, .. } => self.scan_expr(expr),
        }
    }

    fn scan_expr(&mut self, expr: &ast::Expr) {
        match expr {
            ast::Expr::Ident(name, _) => self.note(name),
            ast::Expr::Unary { expr, .. }
            | ast::Expr::Ref { expr, .. }
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
            // Only the base — a field named like a passdown isn't a read of
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
            ast::Expr::IntLit(..)
            | ast::Expr::FloatLit(..)
            | ast::Expr::StringLit(..)
            | ast::Expr::BoolLit(..) => {}
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
    }

    let mut used: Vec<PdInfo> = scan
        .used
        .iter()
        .map(|name| visible[name].clone())
        .collect();
    // Declaration order, not reference order — so the hidden parameters (and
    // therefore the generated C signatures) are stable.
    used.sort_by_key(|p| p.index);
    used
}

/// Resolves a parameter's type per `syntax&shii.md` §5: a `datac` param is
/// an implicit shared borrow unless `mut`-marked (exclusive); everything
/// else (Copy types) stays a plain by-value type unless explicitly
/// `mut`-marked, since borrowing a Copy scalar by default would add
/// indirection nobody asked for. There is no move-taking parameter spelling
/// in v1 — a `datac` value is only ever moved via local rebinding
/// (`val q = p`), which M3 already checks. See `TypeScope::resolve_param`
/// and `resolve_receiver` for the M5 module-qualification rule.
fn implicit_borrow_wrap(base: Ty, mut_marked: bool) -> Ty {
    if mut_marked {
        Ty::Ref { mutable: true, inner: Box::new(base) }
    } else if matches!(base, Ty::Struct(_) | Ty::Bytes) {
        Ty::Ref { mutable: false, inner: Box::new(base) }
    } else {
        base
    }
}

/// Unwraps a (possibly borrowed) type down to the `datac` name it names, if
/// any — used by field access/assignment so `.field` auto-derefs through a
/// `Ref` base exactly like an owned struct.
fn struct_name_of(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(s) => Some(s),
        Ty::Ref { inner, .. } => struct_name_of(inner),
        _ => None,
    }
}

/// Whether a type can be printed as text straight out of its buffer — the
/// builtin `Bytes` and a borrow of one.
fn is_bytes_like(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Bytes),
        _ => false,
    }
}

/// `print`/templates read a `Bytes` without consuming it, so an owned one is
/// borrowed here rather than passed by value (which would be a move). The
/// borrow has to come from a place: C cannot take the address of a temporary
/// for the two arguments (`len` and `data`) a `%.*s` needs, and doing it
/// twice would evaluate the temporary twice.
fn borrow_for_print(value: tir::Expr, what: &str, _ctx: &Ctx) -> Result<tir::Expr> {
    if !matches!(value.ty, Ty::Bytes) {
        return Ok(value);
    }
    if !is_tir_place(&value) {
        return Err(TypeError {
            message: format!(
                "cannot print a temporary 'Bytes' with {what} — bind it to a 'val' or 'var' first (v1 has no way to hold a temporary's address across a call)"
            ),
            span: value.span,
        });
    }
    let span = value.span;
    Ok(tir::Expr {
        kind: tir::ExprKind::Ref {
            mutable: false,
            expr: Box::new(value),
        },
        ty: Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Bytes),
        },
        span,
    })
}

/// A "place": something with stable storage that can be borrowed or
/// assigned into — an `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`).
fn is_tir_place(expr: &tir::Expr) -> bool {
    matches!(
        expr.kind,
        tir::ExprKind::Ident(_) | tir::ExprKind::FieldAccess { .. } | tir::ExprKind::Deref { .. }
    )
}

fn is_ast_place(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(..) | ast::Expr::FieldAccess { .. } | ast::Expr::Deref { .. })
}

/// Whether a checked place-expression's storage is currently writable:
/// either its root is a plain owned `var`-bound local, or its root (or the
/// pointer stepped through by a `*`) is statically typed as an *exclusive*
/// `Ty::Ref`. Used both by assignment (`p.x = ...`, `*p = ...`) and by
/// `&mut expr` (taking an exclusive borrow requires the place already be
/// writable).
fn check_target_writable(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &target.kind {
        tir::ExprKind::Ident(name) => {
            let Some(info) = ctx.lookup(name) else {
                // Only passdown references get here: their hidden parameter
                // (`pd#...`) is deliberately not a scope binding, so the
                // generic path below would panic on them. A passdown is
                // read-only inside a function by design (M5.2).
                if let Some(pd) = ctx.pds.values().find(|p| &p.hidden == name) {
                    let why = if pd.mutable {
                        "a passdown is read-only inside a function (a 'pd var' can only be assigned at its declaration)"
                    } else {
                        "it is declared 'pd val'"
                    };
                    return Err(TypeError {
                        message: format!("cannot assign to passdown '{}': {why}", pd.qualified),
                        span: target.span,
                    });
                }
                return Err(TypeError {
                    message: format!("cannot assign to '{name}'"),
                    span: target.span,
                });
            };
            match &info.ty {
                Ty::Ref { mutable, .. } => {
                    if *mutable {
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
            Ty::Ref { mutable: false, .. } => Err(TypeError {
                message: "cannot write through a shared borrow".to_string(),
                span: target.span,
            }),
            _ => unreachable!("a Deref's operand is always Ref-typed, guaranteed by check_expr"),
        },
        other => unreachable!("not a place expression: {other:?}"),
    }
}

/// `Type.name` for a receiver method, `name` for a plain function — for
/// diagnostics only (`sig_key` is the lookup key; this is what a person
/// reads).
fn display_name(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}.{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

fn check_function(
    f: &ast::FunctionDecl,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    project: &Project,
    all_pds: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
) -> Result<tir::Function> {
    let key = sig_key(f);
    let sig = sigs.get(&key).unwrap().clone();
    let receiver_struct = f
        .receiver
        .as_ref()
        .and_then(|_| struct_name_of(&sig.params[0].1).map(|s| s.to_string()));
    // Passdowns this function references become trailing hidden parameters
    // (see `PdInfo::arg` for the call-site side). `main` is the exception
    // downstream, not here: `codegen_c.rs` emits `int main(void)` regardless
    // of parameters, and the body's `pd#...` identifiers then resolve to the
    // file-scope globals — which is why the hidden name and the global name
    // are the same string.
    let hidden = if key == "main" {
        Vec::new()
    } else {
        wired.get(&key).cloned().unwrap_or_default()
    };
    let mut ctx = base_ctx(
        &f.module,
        sigs,
        structs,
        project,
        all_pds,
        None,
        wired,
    );
    ctx.ret_ty = sig.ret.clone();
    ctx.receiver_struct = receiver_struct;
    ctx.pd_global_borrow = key == "main";
    let mut params = Vec::new();
    // Parameters (and the receiver, if any) are immutable bindings, like
    // `val`; v1 has no syntax to declare a mutable *binding* for one (the
    // borrow itself can still be exclusive — that's `Ty::Ref { mutable:
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
                span: e.span(),
            }
        }
    };

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1/M2), so a mismatch is only reported when a tail
    // expression is actually present and disagrees with the declared return
    // type.
    if sig.ret != Ty::Unit && body.tail.is_some() && body.ty() != sig.ret {
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

    Ok(tir::Function {
        name: key,
        params: {
            params.extend(hidden.iter().map(|pd| (pd.hidden.clone(), pd.param_ty.clone())));
            params
        },
        ret: sig.ret,
        body,
        module: f.module.clone(),
    })
}

fn check_block(block: &ast::Block, ctx: &mut Ctx, expected_tail: Option<&Ty>) -> Result<tir::Block> {
    ctx.push_scope();
    let mut stmts = Vec::new();
    let mut tail = None;

    for (i, stmt) in block.stmts.iter().enumerate() {
        let is_last = i + 1 == block.stmts.len();
        // Statements are newline-terminated now (no `;`), so "textually
        // last in the block" is the only signal for tail-expression status.
        let is_tail = is_last && matches!(stmt, ast::Stmt::Expr { .. });

        if is_tail {
            if let ast::Stmt::Expr { expr, .. } = stmt {
                tail = Some(Box::new(check_expr(expr, ctx, expected_tail)?));
            }
        } else {
            stmts.push(check_stmt(stmt, ctx)?);
        }
    }

    ctx.pop_scope();
    Ok(tir::Block {
        stmts,
        tail,
        span: block.span,
    })
}

fn check_stmt(stmt: &ast::Stmt, ctx: &mut Ctx) -> Result<tir::Stmt> {
    match stmt {
        ast::Stmt::Let {
            name,
            mutable,
            ty,
            init,
            ..
        } => {
            let expected = match ty {
                Some(t) => Some(ctx.type_scope().resolve(t)?),
                None => None,
            };
            let init = check_expr(init, ctx, expected.as_ref())?;
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
            Ok(tir::Stmt::Let {
                name: name.clone(),
                ty: final_ty,
                init,
            })
        }
        ast::Stmt::While { cond, body, span } => {
            let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
            if cond.ty != Ty::Bool {
                return Err(TypeError {
                    message: format!("while condition must be 'bool', found '{}'", cond.ty.display()),
                    span: *span,
                });
            }
            let body = check_block(body, ctx, None)?;
            Ok(tir::Stmt::While { cond, body })
        }
        ast::Stmt::Return { value, .. } => {
            let ret_ty = ctx.ret_ty.clone();
            let value = match value {
                Some(v) => Some(check_expr(v, ctx, Some(&ret_ty))?),
                None => None,
            };
            let actual = value.as_ref().map(|v| v.ty.clone()).unwrap_or(Ty::Unit);
            if actual != ret_ty {
                return Err(TypeError {
                    message: format!(
                        "return type mismatch: expected '{}', found '{}'",
                        ret_ty.display(),
                        actual.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(tir::Stmt::Return {
                value,
                span: stmt_span(stmt),
            })
        }
        ast::Stmt::Expr { expr, .. } => Ok(tir::Stmt::Expr(check_expr(expr, ctx, None)?)),
    }
}

fn stmt_span(stmt: &ast::Stmt) -> Span {
    match stmt {
        ast::Stmt::Let { span, .. } => *span,
        ast::Stmt::While { span, .. } => *span,
        ast::Stmt::Return { span, .. } => *span,
        ast::Stmt::Expr { span, .. } => *span,
    }
}

fn check_expr(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    match expr {
        ast::Expr::IntLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_integer() => t.clone(),
                _ => Ty::I32,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v),
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
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: info.ty.clone(),
                    span: *span,
                });
            }
            // Not a local or parameter — inside a receiver-form method,
            // fall back to the receiver's field of that name (no `self.`
            // qualifier is ever written; see `syntax&shii.md` §5).
            if let Some(field_ty) = ctx.receiver_field_ty(name) {
                let self_ty = ctx.lookup("self").expect("receiver binding exists whenever receiver_struct is set").ty.clone();
                let base = tir::Expr { kind: tir::ExprKind::Ident("self".to_string()), ty: self_ty, span: *span };
                return Ok(tir::Expr {
                    kind: tir::ExprKind::FieldAccess { base: Box::new(base), field: name.clone() },
                    ty: field_ty,
                    span: *span,
                });
            }
            // Not a local or receiver field — a passdown (M5.2), which the
            // wiring scan has already made this function take as a hidden
            // parameter named `pd#...`, so the reference is that parameter.
            if let Some(pd) = ctx.pds.get(name).cloned() {
                if !ctx.pd_initialized(&pd.hidden) {
                    return Err(TypeError {
                        message: format!(
                            "passdown '{}' is read before it is initialized — a 'pd' initializer can only use passdowns declared before it",
                            pd.qualified
                        ),
                        span: *span,
                    });
                }
                // In a `pd` initializer only Copy values can be read out of
                // a passdown: a non-Copy one is handed around as a shared
                // borrow, and a passdown has to hold a value.
                if ctx.init_state.is_some() && matches!(pd.value_ty, Ty::Struct(_)) {
                    return Err(TypeError {
                        message: format!(
                            "passdown '{}' cannot be read in another passdown's initializer: its value is not Copy",
                            pd.qualified
                        ),
                        span: *span,
                    });
                }
                if ctx.pd_global_borrow {
                    return Ok(pd.arg(*span));
                }
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Ident(pd.hidden.clone()),
                    ty: pd.param_ty.clone(),
                    span: *span,
                });
            }
            Err(TypeError {
                message: match ctx.foreign_pds.get(name) {
                    Some(module) => format!(
                        "'{name}' is a passdown of module '{module}' — a passdown is only visible inside the module that declares it"
                    ),
                    // Inside a `pd` initializer only passdowns declared
                    // *before* it are in scope at all, so the generic name is
                    // usually a forward reference.
                    None if ctx.init_state.is_some() => format!(
                        "undefined variable '{name}' — a 'pd' initializer can only use passdowns declared before it"
                    ),
                    None => format!("undefined variable '{name}'"),
                },
                span: *span,
            })
        }
        ast::Expr::Unary { op, expr: sub, span } => {
            let sub_expected = if *op == UnOp::Not { Some(&Ty::Bool) } else { expected };
            let sub_t = check_expr(sub, ctx, sub_expected)?;
            match op {
                UnOp::Neg => {
                    if !sub_t.ty.is_numeric() {
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
        ast::Expr::Call { callee, args, span } => check_call(callee, args, *span, ctx),
        ast::Expr::FieldAccess { base, field, span } => {
            if let ast::Expr::Ident(name, _) = base.as_ref() {
                if is_module_alias(name, ctx) {
                    return Err(TypeError {
                        message: format!(
                            "'{name}' is a module, not a value — call its items as '{name}.{field}(...)'"
                        ),
                        span: *span,
                    });
                }
            }
            let base_t = check_expr(base, ctx, None)?;
            let Some(sname) = struct_name_of(&base_t.ty).map(|s| s.to_string()) else {
                return Err(TypeError {
                    message: format!("cannot access field '{field}' on non-struct type '{}'", base_t.ty.display()),
                    span: *span,
                });
            };
            let sdef = ctx.structs.get(&sname).expect("struct types are only produced for known structs");
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
        ast::Expr::Ref { mutable, expr: sub, span } => {
            if !is_ast_place(sub) {
                return Err(TypeError {
                    message: "cannot borrow a temporary value".to_string(),
                    span: *span,
                });
            }
            let sub_t = check_expr(sub, ctx, None)?;
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
                message: "a string template can only be printed — pass it to 'print(...)' or 'print!(...)'; v1 has no owned string type to build one with (that's M6)".to_string(),
                span: *span,
            })
        }
        ast::Expr::MethodCall { receiver, method, args, span } => {
            // `circle.make(...)`: a module-qualified reference to an
            // imported item, not a method call. Recognized only when the
            // receiver is a bare identifier that names an `impl` alias and
            // is *not* shadowed by a local (locals win, exactly like any
            // other name lookup); anything else is a real method call.
            if let ast::Expr::Ident(name, _) = receiver.as_ref() {
                if is_module_alias(name, ctx) {
                    if let Some(prefix) = ctx.aliases.get(name).cloned() {
                        return check_module_item(name, &prefix, method, args, *span, ctx);
                    }
                }
            }
            check_method_call(receiver, method, args, *span, ctx)
        }
    }
}

/// Whether a bare identifier names an `impl` alias rather than a value. A
/// local, parameter, receiver field, or passdown shadows it, exactly as it
/// would shadow any other name.
fn is_module_alias(name: &str, ctx: &Ctx) -> bool {
    ctx.lookup(name).is_none()
        && !ctx.pds.contains_key(name)
        && ctx.aliases.contains_key(name)
}

/// Assignment targets: `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`) — anything `is_ast_place`
/// accepts.
fn check_assign(target: &ast::Expr, value: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if !is_ast_place(target) {
        return Err(TypeError {
            message: "invalid assignment target".to_string(),
            span,
        });
    }

    let target_t = check_expr(target, ctx, None)?;
    check_target_writable(&target_t, ctx)?;

    // For a field target, writing it also requires the field itself be
    // declared `var` in its `datac` — a `val` field is never reassignable,
    // regardless of whether it's reached through a `var` local or an
    // exclusive borrow. Only the *immediate* field being assigned needs to
    // be `var`; fields merely read through on the way there do not.
    if let tir::ExprKind::FieldAccess { base, field } = &target_t.kind {
        let sname = struct_name_of(&base.ty).expect("field access base is always a struct type, guaranteed by check_expr");
        let sdef = ctx.structs.get(sname).expect("struct types are only produced for known structs");
        let fdef = sdef.field(field).expect("field already resolved by check_expr");
        if !fdef.mutable {
            return Err(TypeError {
                message: format!("cannot assign to '{field}': declared 'val'"),
                span: target_t.span,
            });
        }
    }

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
            let l = check_expr(lhs, ctx, numeric_expected)?;
            let r = check_expr(rhs, ctx, Some(&l.ty))?;
            if !l.ty.is_numeric() || l.ty != r.ty {
                return Err(TypeError {
                    message: format!(
                        "operator requires matching numeric operands, found '{}' and '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
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
        Lt | LtEq | Gt | GtEq | Eq | NotEq => {
            let l = check_expr(lhs, ctx, None)?;
            let r = check_expr(rhs, ctx, Some(&l.ty))?;
            if l.ty != r.ty {
                return Err(TypeError {
                    message: format!(
                        "cannot compare '{}' with '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
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
/// the rest — shared between plain function calls and `datac` construction,
/// since both look identical at a call site.
fn match_args(
    what: &str,
    params: &[(String, Ty)],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Vec<tir::Expr>> {
    if args.len() != params.len() {
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
        result.push(check_call_arg(pname, pty, &arg.value, ctx)?);
    }

    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }

    Ok(result)
}

/// Checks one call-site argument against its matched parameter type. When
/// the parameter is `Ty::Ref`, the caller never writes `&` — a plain place
/// argument (`p`, `p.x`) is auto-borrowed here (the callee's signature is
/// what decides shared vs exclusive), matching `syntax&shii.md`'s "shared
/// borrow is implicit — write nothing." An argument that's already a borrow
/// (an explicit `&expr`, or forwarding an existing `Ref`-typed parameter)
/// type-matches directly instead of double-wrapping; a shared parameter
/// accepts an exclusive argument (implicit downgrade), never the reverse.
fn check_call_arg(pname: &str, pty: &Ty, arg_expr: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let Ty::Ref { mutable, inner } = pty else {
        let checked = check_expr(arg_expr, ctx, Some(pty))?;
        if checked.ty != *pty {
            return Err(TypeError {
                message: format!(
                    "argument '{pname}' has type '{}', expected '{}'",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    };

    let checked = check_expr(arg_expr, ctx, Some(inner))?;
    match &checked.ty {
        t if t == inner.as_ref() => {
            if !is_tir_place(&checked) {
                return Err(TypeError {
                    message: format!("argument '{pname}' must be a variable or field, not a temporary value"),
                    span: arg_expr.span(),
                });
            }
            if *mutable {
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
                return Err(TypeError {
                    message: format!("argument '{pname}' requires an exclusive borrow, found a shared one"),
                    span: arg_expr.span(),
                });
            }
            Ok(checked)
        }
        other => Err(TypeError {
            message: format!("argument '{pname}' has type '{}', expected '{}'", other.display(), pty.display()),
            span: arg_expr.span(),
        }),
    }
}

fn check_call(callee: &str, args: &[ast::CallArg], span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if callee == "main" {
        // `main` is emitted as `int main(void)` — it takes no arguments, so
        // there is no way to hand it the passdowns it may reference.
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
        let printable = arg.ty.is_numeric()
            || matches!(arg.ty, Ty::Bool | Ty::Char | Ty::Str)
            || is_bytes_like(&arg.ty);
        if !printable {
            return Err(TypeError {
                message: format!("cannot pass a value of type '{}' to '{callee}'", arg.ty.display()),
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

    if let Some(sdef) = ctx.structs.get(callee).cloned() {
        let params: Vec<(String, Ty)> = sdef.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
        let checked = match_args(&format!("datac '{callee}'"), &params, args, span, ctx)?;
        let fields = sdef
            .fields
            .iter()
            .zip(checked)
            .map(|(f, e)| (f.name.clone(), e))
            .collect();
        return Ok(tir::Expr {
            kind: tir::ExprKind::StructLit {
                name: callee.to_string(),
                fields,
            },
            ty: Ty::Struct(callee.to_string()),
            span,
        });
    }

    let sig = ctx.sigs.get(callee).cloned().ok_or_else(|| TypeError {
        message: match qualified_hint(callee, ctx.structs.keys().chain(ctx.sigs.keys()), &ctx.aliases) {
            Some(hint) => format!("call to undefined function '{callee}' — did you mean {hint}?"),
            None => format!("call to undefined function '{callee}'"),
        },
        span,
    })?;
    let mut checked = match_args(&format!("function '{callee}'"), &sig.params, args, span, ctx)?;
    checked.extend(hidden_args(callee, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: callee.to_string(),
            args: checked,
        },
        ty: sig.ret,
        span,
    })
}

/// The hidden passdown arguments a call to `callee` has to pass, in the
/// order the callee's signature declares them (M5.2). Each one is the
/// generated global for that passdown — a call site never receives its
/// callees' passdowns itself, so the value always comes from the global
/// here; the callee's parameter is what makes the dependency visible in its
/// own signature.
fn hidden_args(callee: &str, span: Span, ctx: &Ctx) -> Result<Vec<tir::Expr>> {
    let Some(pds) = ctx.wired.get(callee) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(pds.len());
    for pd in pds {
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                message: format!(
                    "'{callee}' reads passdown '{}', which is not initialized yet here — a 'pd' initializer can only call functions whose passdowns are already set up",
                    pd.qualified
                ),
                span,
            });
        }
        out.push(pd.arg(span));
    }
    Ok(out)
}

/// Checks a string template's interpolated pieces and lowers it to one
/// `tir::ExprKind::Template`. Interpolated values must be printable under
/// exactly the rule `print` already applies (numeric, `bool`, `char`,
/// `str`) — a template is a formatted print, not a string builder.
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
                let value = borrow_for_print(value, "a string template", ctx)?;
                let printable = value.ty.is_numeric()
                    || matches!(value.ty, Ty::Bool | Ty::Char | Ty::Str)
                    || is_bytes_like(&value.ty);
                if !printable {
                    return Err(TypeError {
                        message: format!(
                            "cannot interpolate a value of type '{}' into a string template",
                            value.ty.display()
                        ),
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

/// `receiver.method(args)`: resolves `method` against the receiver
/// expression's `datac` type, then lowers to a plain `tir::ExprKind::Call`
/// — the receiver becomes the first argument, auto-borrowed through
/// `check_call_arg` exactly like any other `Ty::Ref` parameter (so a
/// `fun mut T.m()` method reuses M4's aliasing checks with no special
/// casing). No new `tir`/codegen shape is needed for method calls at all.
fn check_method_call(receiver: &ast::Expr, method: &str, args: &[ast::CallArg], span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    // `Bytes.empty()` / `Bytes.from_str(s)`: an *associated* builtin, written
    // on the type name, so it takes no receiver argument and is resolved
    // before the receiver is checked as a value (it isn't one). A local or an
    // `impl` alias of the same name shadows the type name, exactly as it
    // would anywhere else — which is why those cases fall through to the
    // ordinary method path below.
    if let ast::Expr::Ident(name, _) = receiver {
        if ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
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

    // Type-only probe to learn the receiver's owner name; re-checked for
    // real (with its actual borrow/move effects) via `check_call_arg`
    // below. `check_expr` has no persistent side effects beyond reading
    // `ctx`, so checking the same expression twice here is safe.
    let probe = check_expr(receiver, ctx, None)?;
    let owner = struct_name_of(&probe.ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(&probe.ty).map(|s| s.to_string()));
    let Some(owner) = owner else {
        return Err(TypeError {
            message: format!("cannot call method '{method}' on non-struct type '{}'", probe.ty.display()),
            span,
        });
    };
    let key = format!("{owner}#{method}");
    let sig = ctx
        .sigs
        .get(&key)
        .cloned()
        .or_else(|| builtin_sig(&key))
        .ok_or_else(|| TypeError {
            message: format!("datac '{owner}' has no method '{method}'"),
            span,
        })?;

    let mut checked_args = Vec::with_capacity(sig.params.len());
    checked_args.push(check_call_arg("self", &sig.params[0].1, receiver, ctx)?);
    checked_args.extend(match_args(&format!("method '{owner}.{method}'"), &sig.params[1..], args, span, ctx)?);
    // Receiver methods can reference passdowns too; the call site passes the
    // same hidden arguments a free-function call would.
    checked_args.extend(hidden_args(&key, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked_args },
        ty: sig.ret,
        span,
    })
}

/// A bare name that only exists as an imported item's *qualified* name is a
/// very likely strict-namespace mistake (M5: imported items are reached
/// only as `alias.Item`). Returns a "did you mean ..." fragment when `name`
/// matches some qualified key's last segment.
fn qualified_hint<'a>(
    name: &str,
    keys: impl Iterator<Item = &'a String>,
    aliases: &HashMap<String, String>,
) -> Option<String> {
    let want = format!(".{name}");
    let key = keys
        .into_iter()
        .find(|k| k.ends_with(&want) && k.contains('.'))?;
    let (module_path, _) = key.rsplit_once('.')?;
    let qualified_prefix = format!("{module_path}.");
    Some(match aliases.iter().find(|(_, p)| p.as_str() == qualified_prefix) {
        Some((alias, _)) => format!("'{alias}.{name}'"),
        None => {
            let import_path = module_path.replace('.', "/");
            format!(
                "'{name}' is declared in module '{import_path}' — add `impl \"{import_path}\"` and reach it as `<alias>.{name}`"
            )
        }
    })
}

/// `alias.item(args)` — a reference to an item in an imported module, per
/// M5's strict module-qualified namespace. `prefix` is that module's symbol
/// prefix, so the actual lookup key is just `prefix + item`: the same
/// table `check_call` uses for any other call, which is why imported
/// `datac` construction and imported free functions need no new lowering.
fn check_module_item(
    alias: &str,
    prefix: &str,
    item: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let qualified = format!("{prefix}{item}");
    if ctx.structs.contains_key(&qualified) || ctx.sigs.contains_key(&qualified) {
        return check_call(&qualified, args, span, ctx);
    }
    // A receiver method lives under a `Type#name` key, which a module path
    // can't spell — say so rather than "no such item".
    let method_suffix = format!("#{item}");
    if let Some(key) = ctx
        .sigs
        .keys()
        .find(|k| k.starts_with(prefix) && k.ends_with(&method_suffix))
    {
        let type_name = key.trim_end_matches(&method_suffix).trim_end_matches(prefix);
        return Err(TypeError {
            message: format!(
                "'{item}' is a method on '{type_name}', not a free function — call it on a value ('value.{item}(...)')"
            ),
            span,
        });
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
            match checked.kind {
                tir::ExprKind::If { .. } => Some(tir::Block {
                    stmts: vec![],
                    tail: Some(Box::new(checked)),
                    span: e.span(),
                }),
                _ => unreachable!("parser guarantees else-if parses to Expr::If"),
            }
        }
        None => None,
    };

    let ty = match &else_t {
        Some(e) => {
            if then_t.ty() != e.ty() {
                return Err(TypeError {
                    message: format!(
                        "if/else branches have different types: '{}' and '{}'",
                        then_t.ty().display(),
                        e.ty().display()
                    ),
                    span,
                });
            }
            then_t.ty()
        }
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

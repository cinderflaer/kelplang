use std::collections::HashMap;

use crate::ast::{self, BinOp, UnOp};
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
}

#[derive(Clone)]
struct VarInfo {
    ty: Ty,
    mutable: bool,
}

/// One user-declared generic type (A-2): a `datac` or `enum` written with
/// type parameters. Its fields are kept *as written* rather than resolved,
/// because a type parameter has no type until an instantiation binds one —
/// resolution happens in `TypeScope::instantiate`, with the parameters bound
/// to the concrete arguments.
#[derive(Clone)]
struct GenericTemplate {
    /// Qualified name, exactly the key a non-generic `datac` of the same name
    /// would have had.
    name: String,
    params: Vec<String>,
    kind: GenericKind,
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
/// discovered while checking one body is visible to every other — and to the
/// final `tir::Program`.
#[derive(Clone, Default)]
struct TypeEnv {
    generics: std::rc::Rc<HashMap<String, GenericTemplate>>,
    instances: std::rc::Rc<std::cell::RefCell<InstanceState>>,
}

/// One user-declared generic *function* (A-2): a `fun` with its own type
/// parameters (`fun first<T>(xs: List<T>).R: T`) or a receiver method on a
/// generic type (`fun Pair<A, B>.first().R: A`). Its parameter and return
/// types are kept as written, because a type parameter has no type until an
/// instantiation binds it — exactly like `GenericTemplate`.
#[derive(Clone)]
struct FuncTemplate {
    /// Declaration key: the prefixed name, or `Type#name` for a receiver.
    key: String,
    /// The method's type parameters, in order, which for a receiver method are
    /// the receiver's own arguments.
    params: Vec<String>,
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
/// argument (`fun g<T>(x: T).R: i32 = g(x = List.of(x))`) grows without bound;
/// this turns that into a diagnostic instead of a hang.
const MAX_FUNCTION_INSTANTIATIONS: usize = 2048;

/// The type parameters a function declaration introduces: its own for a free
/// function, the receiver's for a method on a generic type.
fn decl_type_params(f: &ast::FunctionDecl) -> &[String] {
    match &f.receiver {
        Some(r) => &r.type_args,
        None => &f.type_params,
    }
}

/// The canonical name of one instantiation: `Pair<i32, String>`. It is a
/// `Ty::Struct` key like any other declaration's name, and `types::c_ident`
/// mangles it into a C identifier, so a type argument list can never be
/// written by hand in a position that would collide.
fn instance_key(name: &str, args: &[Ty]) -> String {
    let parts: Vec<String> = args.iter().map(|a| a.display()).collect();
    format!("{}<{}>", name, parts.join(", "))
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
    /// already folded to a literal — so a use site is a substitution, not a
    /// load from anywhere.
    consts: HashMap<String, ConstInfo>,
    /// Overload sets for compiler-shipped stdlib functions (M6.3): plain key
    /// (`std.math.abs`) -> each declaration's parameter types and its
    /// type-suffixed key.
    overloads: HashMap<String, Vec<(Vec<Ty>, String)>>,
    /// The module being checked — needed by capability diagnostics, which
    /// have to name the module that lacks the grant.
    module: String,
    /// Capability domains *this* module holds, with their patterns (M7). The
    /// gate below is what makes an ungranted `fs.read` a resolution error
    /// rather than a runtime check that happens to reject it.
    caps: HashMap<String, Vec<String>>,
    /// Function key -> gated domain, for the modules that gate one.
    gated: HashMap<String, String>,
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
    /// `plan/syntax.md` §9 leaves the cross-module question open; M5.2
    /// answers it the same way M5.1 answered imports — module-scoped, so
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
    /// fixed C signature — it cannot receive hidden parameters — so its
    /// passdown references take the address of the generated global instead
    /// of naming a parameter. Every other function that references a
    /// passdown is wired and uses its parameter.
    pd_global_borrow: bool,
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
    /// The `datac` whose body the literal was written in, when the literal is
    /// inside a receiver-form method (A-4 phase E). Its fields are names the
    /// literal can *see* but cannot capture: reading one out of a borrowed
    /// receiver is a partial move (A-11), so the diagnostic says that instead
    /// of reporting an undefined name.
    outer_receiver: Option<String>,
    /// Set while checking a receiver-form function whose receiver resolves
    /// to a `datac`: lets a bare identifier that isn't a local fall back to
    /// an implicit `self.field` (`fun Circle.area().R: f64 = 3.14159 * r *
    /// r` — no `self.` qualifier written, per `plan/syntax.md` §5's "no
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
    /// The *borrow* of the passdown's storage: what a call site passes for
    /// this passdown's hidden parameter, and what a read dereferences.
    ///
    /// Two shapes, because the two contexts reach the storage differently:
    /// inside a function the hidden name is already the borrow (`&*hidden`
    /// would be a pointer to a pointer), while `main` and a `pd` initializer
    /// have no hidden parameter and name the generated global directly, so
    /// the borrow is explicit.
    fn arg(&self, span: Span, global: bool) -> tir::Expr {
        if global {
            let global = tir::Expr {
                kind: tir::ExprKind::Ident(self.hidden.clone()),
                ty: self.value_ty.clone(),
                span,
            };
            return tir::Expr {
                kind: tir::ExprKind::Ref {
                    mutable: self.mutable,
                    expr: Box::new(global),
                },
                ty: self.param_ty.clone(),
                span,
            };
        }
        tir::Expr {
            kind: tir::ExprKind::Ident(self.hidden.clone()),
            ty: self.param_ty.clone(),
            span,
        }
    }

    /// A read of the passdown (M6.4): `*` of the borrow, so reads and writes
    /// go through the same pointer and a `pd var` is assignable from any
    /// function that references it.
    fn value(&self, span: Span, global: bool) -> tir::Expr {
        tir::Expr {
            kind: tir::ExprKind::Deref {
                expr: Box::new(self.arg(span, global)),
            },
            ty: self.value_ty.clone(),
            span,
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
        self.struct_def(sname)?.field(name).map(|f| f.ty.clone())
    }

    /// Whether `name` is a field of the named `datac` — used only to tell a
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
        if !self.captures.iter().any(|(n, _)| n == name) {
            self.captures.push((name.to_string(), ty.clone()));
        }
        Some(ty)
    }

    /// The capability gate (M7): a gated module's item is reachable only
    /// from a module that holds the domain. This is a *resolution* error, not
    /// a runtime check — the point of the feature is that code without a
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
        // gated item — `stdlib::domain_of` only knows stdlib ids — so this
        // exemption cannot widen a user module's authority.
        if !self.prefix.is_empty() && key.starts_with(self.prefix.as_str()) {
            return Ok(());
        }
        Err(TypeError {
            message: format!(
                "'{}' needs the '{}' capability, which module '{}' was not granted — the module that imports this one must delegate it (`impl \"<this module>\" with caps({} = [\"<pattern>\"])`)",
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
    /// reachable *bare* inside that file — so a bare lookup has to try the
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
            env: self.env.clone(),
            params: &self.type_args,
            prefix: &self.prefix,
            aliases: &self.aliases,
            // Every datac is known by the time bodies are checked.
            later_datacs: None,
            in_progress: None,
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

/// Everything needed to turn a *written* type name into a `Ty`, with M5's
/// module qualification rules:
/// - a builtin (`i32`, `f64`, `str`, ...) is itself;
/// - `alias.Item` resolves through the current module's `impl` table;
/// - a bare name belongs to the current module, so it gets that module's
///   prefix — imported types are never reachable unqualified (strict
///   module-qualified namespace, per the plan).
struct TypeScope<'a> {
    structs: &'a HashMap<String, StructDef>,
    /// Generic templates and the instances built so far (A-2). Shared, and
    /// mutable through a `RefCell`, because resolving `Pair<i32, String>`
    /// *creates* the instantiation it names — including from inside another
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
}

impl TypeScope<'_> {
    /// A written type name, or a container over one. `indirect` is true
    /// when the type is being resolved *inside* a `Box`/`List` argument,
    /// where referring to the `datac` currently being declared (or to a
    /// `datac` declared later) is legal: both containers hold a pointer, so
    /// the indirection is what makes a recursive type finite.
    fn resolve(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        let resolved = self.resolve_base(ty, indirect)?;
        // A `datac` that contains itself *by value* has no finite size.
        // Through a container it is fine — that is the whole point of `Box`
        // and of a list's pointer buffer — so the check is skipped there.
        if !indirect {
            if let Some(ip) = self.in_progress {
                if resolved == Ty::Struct(ip.to_string()) {
                    return Err(TypeError {
                        message: format!(
                            "'{ip}' cannot hold itself by value — that type has no finite size; put it behind indirection ('val next: Box<{ip}>', or a 'List<{ip}>' for many)"
                        ),
                        span: ty.span,
                    });
                }
            }
        }
        Ok(resolved)
    }

    fn resolve_base(&self, ty: &ast::TypeExpr, indirect: bool) -> Result<Ty> {
        // A function type (A-4): `fun(i32, String).R: bool`. Checked before
        // every name, because it is the one written type that is not one —
        // and `fun` cannot be a name at all, since it is a keyword. A closure
        // type (A-4 phase E) is the same shape under `closure`.
        if let Some(params) = &ty.fn_params {
            if ty.is_closure() {
                return self.resolve_closure_type(ty, params);
            }
            return self.resolve_fn_type(ty, params);
        }
        // A type parameter in scope (`datac Pair<A, B>`'s `A`) is whatever
        // the instantiation bound it to (A-2). Checked before every other
        // name, so a parameter shadows a type of the same name.
        if ty.args.is_empty() && !ty.name.contains('.') {
            if let Some(bound) = self.params.get(&ty.name) {
                return Ok(bound.clone());
            }
        }
        // The three generic names v1 knows (`List<T>` from C1, `Box<T>` from
        // M8, `Map<V>` from MK5-S5). All are compiler builtins, not
        // user-definable types; each checks its element against what its C
        // representation can hold.
        if ty.name == "List" || ty.name == "Box" || ty.name == "Map" {
            // `Map<K, V>` names its key type, and `Map<V>` is the shorthand
            // for a `String` key — which is what every use site written before
            // A-2 says, and the two spellings are the same `Ty`.
            let (key, value_arg, value_span) = if ty.name == "Map" {
                match ty.args.len() {
                    1 => (Ty::String, &ty.args[0], ty.args[0].span),
                    2 => {
                        let key = self.resolve(&ty.args[0], true)?;
                        if !crate::types::is_map_key(&key) {
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
            let elem = self.resolve(value_arg, true)?;
            let (ok, error) = if ty.name == "List" {
                (crate::types::is_list_element(&elem), list_element_error(&elem))
            } else if ty.name == "Map" {
                (crate::types::is_map_element(&elem), map_element_error(&elem))
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
                for a in &ty.args {
                    args.push(self.resolve(a, true)?);
                }
                return self.instantiate(&qualified, &args, ty.span);
            }
            return Err(TypeError {
                message: format!(
                    "'{}' does not take type arguments — the compiler's generic types are 'List<T>', 'Box<T>' and 'Map<V>', and a user type has to be declared with parameters to take them ('datac {name}<T> ...')",
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
        // Everything else is a name as the user wrote it and still needs
        // qualifying.
        if self.structs.contains_key(&ty.name) {
            return Ok(Ty::Struct(ty.name.clone()));
        }
        let qualified = self.qualify(&ty.name, ty.span)?;
        if self.structs.contains_key(&qualified) {
            Ok(Ty::Struct(qualified))
        } else if let Some(tmpl) = self
            .env
            .generics
            .get(&qualified)
            .or_else(|| self.env.generics.get(&ty.name))
        {
            // A generic type written without its arguments. The generic half
            // of the name is enough to say what is missing.
            Err(TypeError {
                message: format!(
                    "'{}' is a generic type — it needs {} type argument(s): '{}<{}>'",
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
                        "'{}' is declared after this type — a type may only name types declared before it by value; behind a 'Box' or 'List' it may name any type, including itself",
                        ty.name
                    ),
                    span: ty.span,
                })
            }
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
                        "unknown module alias '{alias}' in type '{name}' — was it imported?"
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
        let tmpl = self.env.generics[name].clone();
        let params: HashMap<String, Ty> = tmpl
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let scope = TypeScope {
            structs: self.structs,
            env: self.env.clone(),
            params: &params,
            prefix: &tmpl.prefix,
            aliases: &tmpl.aliases,
            later_datacs: None,
            in_progress: Some(&key),
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
                                "field '{}' is a borrowed type ('{}') — a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible — an owned 'String' is the way to keep text in one",
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
                                    "payload field '{}' is a borrowed type ('{}') — an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible — an owned 'String' is the way to keep text in one",
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
                    });
                }
                Ok(Instance::Sum(EnumDef {
                    name: key.to_string(),
                    variants: out,
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
    /// `move T` resolves to the bare type — the same by-value shape a `Copy`
    /// type already has. That is the whole type-level difference, and it is
    /// what makes a moved argument an ordinary whole-value read at the call
    /// site: no new argument-matching rule, and `borrowck.rs` marks the
    /// caller's binding moved exactly as `val q = p` does.
    fn resolve_param_mode(&self, ty: &ast::TypeExpr, mode: ast::ParamMode) -> Result<Ty> {
        if mode.moves() {
            return self.resolve_move_param(ty);
        }
        let base = self.resolve(ty, false)?;
        if mode.mutable() && matches!(base, Ty::Fn { .. }) {
            // A function value is Copy, so an exclusive borrow of one would
            // need a pointer to a function pointer that nothing can usefully
            // write through.
            return Err(TypeError {
                message: format!(
                    "'mut' cannot be written on the function type '{}' — a function value is Copy (it is a plain function pointer), so there is nothing to write through; write the type without 'mut'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        Ok(implicit_borrow_wrap(base, mode.mutable()))
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
                    "'move' cannot be written on '{written}' — a 'str' is already a borrowed view, '&T' is already a copy of a borrow, and a function value is already Copy, so there is nothing to take ownership of; write the owned type instead ('move String')"
                ),
                span: ty.span,
            });
        }
        Ok(base)
    }

    /// A function type (A-4). Each parameter goes through the *same* rule a
    /// declaration's parameter does — an implicit shared borrow for an owned
    /// type, `mut` for an exclusive one, `move` for ownership — which is what
    /// makes a written `fun(String)` and the signature of `fun f(s: String)`
    /// the same type, and therefore what makes passing `f` where a
    /// `fun(String)` is expected type-check with no conversion.
    ///
    /// Two placements are refused, both because the C declarator has no
    /// sensible spelling for them: a function type that *returns* a function
    /// type, and a borrow of a function value (which is Copy, so there is
    /// nothing to borrow). A function type as a parameter of a function type
    /// is fine — `int32_t (*)(int32_t (*)(int32_t))` is ordinary C.
    fn resolve_fn_type(&self, ty: &ast::TypeExpr, params: &[ast::FnTypeParam]) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                message: format!(
                    "'&' cannot be written on the function type '{}' — a function value is Copy (it is a plain function pointer), so a borrowed one would be a pointer to a pointer; write the type without '&'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    message: format!(
                        "a function type cannot return a borrow ('&{}') — the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a function *type* is called through a pointer where there is none; return an owned value instead",
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
                        "a function type cannot return another function type ('{}') — C cannot write that declarator, and nothing needs it; return a 'datac' holding the function value instead",
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
                    "'&' cannot be written on the closure type '{}' — a closure is an owned value, and a parameter of that type already borrows it; write the type without '&', or 'move {}' to take ownership",
                    ty.display(),
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    message: format!(
                        "a closure type cannot return a borrow ('&{}') — the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a closure is called through a value; return an owned value instead",
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
                        "a closure type cannot return a function type ('{}') — C cannot write that declarator, and a closure's result is a value rather than a call target; return a 'datac' holding the function value instead",
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
        // — and there is no spelling for them on a receiver yet. Say that
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
                    "'{}' is a generic type — the receiver has to name its type parameters, and they have to be the type's own, in order ('fun {}<{}>.method(...)')",
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


/// The `Ty::Fn` a declared signature has (A-4): its parameter types, in order,
/// and its return type. A function *type* spells exactly this, so a written
/// `fun(String).R: usize` and the signature of `fun f(s: String).R: usize` are
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
    if ctx.sig_of(&owned).is_some() {
        return function_value_at(&owned, name, ctx, span);
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
                    "'{written}' reads a passdown, so it cannot be a function value — a passdown is passed to the callee as a hidden argument, and a function value is a plain C function pointer; call it directly instead"
                ),
                span,
            });
        }
        return Ok(Some((key.to_string(), fn_value_ty(&sig))));
    }
    if ctx.fns.templates.contains_key(key) {
        return Err(TypeError {
            message: format!(
                "'{written}' is a generic function — a function value needs one concrete signature, so wrap the call in a small non-generic function and pass that"
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
    // borrows an owned type), so the callee's type may be `&closure(..)` —
    // codegen dereferences it. A `fun` is `Copy`, so it is never borrowed.
    let callable = |ty: &Ty| -> Option<Ty> {
        match ty {
            Ty::Fn { .. } => Some(ty.clone()),
            Ty::Closure { .. } => Some(ty.clone()),
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
            if matches!(ty, Ty::Fn { .. } | Ty::Closure { .. }) {
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
                    "passdown '{}' is read before it is initialized — a 'pd' initializer can only use passdowns declared before it",
                    pd.qualified
                ),
                span,
            });
        }
        return Ok(Some((pd.value(span, ctx.pd_global_borrow), ty)));
    }
    Ok(None)
}

/// `f(x)` where `f` holds a function value (A-4): a local, a passdown, or a
/// `datac` field (`cfg.on_event(x)`). A function *type* has no parameter
/// names, so arguments are positional and counted against the callee's own
/// list; everything else — the borrow an owned parameter implies, the move a
/// `move` parameter implies — is the rule `check_call_arg` already applies to
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
                    "a call through {what} cannot use named arguments — a function type has no parameter names, so every argument is positional"
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

/// The type of a `datac` field read through a (possibly borrowed) receiver —
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

/// Function names that may be declared more than once, once per parameter
/// type, in a compiler-shipped module (M6.3, stdlib decision 4). The call
/// site picks by argument type; user code cannot define overloads at all,
/// and cannot see the mechanism except as `math.abs` working on whatever
/// number it is given.
const OVERLOADABLE: &[&str] = &["abs", "min", "max", "clamp"];

/// Whether a module id belongs to the compiler-shipped standard library —
/// what stdlib decision 4's "available only to compiler-shipped modules"
/// means in code.
fn is_std_module(module: &str) -> bool {
    module == "std" || module.starts_with("std/")
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
        && is_std_module(&f.module)
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
        // `str`/`&Bytes` view of it is alive — which is what makes the
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
        // `View` is length-aware, and none of them is a C string function — a
        // slice has no NUL after it, so `strlen`/`strcmp` on one would read
        // past the slice and report a different string than the program has.
        // That is also why none of these takes or returns a `str`.
        //
        // `self` is by value for every one of them: a `View` is two words of
        // `Copy` data (a pointer and a length), so a borrow would add an
        // indirection and buy nothing — the same choice the integer
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
        // buffer `realloc` may move — the borrow checker rejects that.
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
        // free functions, not methods — the map key is a bare name, which is
        // the same namespace `print` occupies.
        add("panic", vec![("msg".to_string(), Ty::Str)], Ty::Unit, false);
        add("assert", vec![("cond".to_string(), Ty::Bool)], Ty::Unit, false);

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
            // never mixes with a float by itself — `n * 1.0` is a type error,
            // because operands must match exactly — and before this the only
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
        m
    });

/// The name a builtin type answers to in a method key — the non-`datac`
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
        Ty::Map(..) => Some("Map"),
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
/// only thing missing is a C spelling — a container's generated type names its
/// element as a prefix (`kelp_List__i32`), and a function pointer's name goes
/// *inside* the declarator (`int32_t (*kelp_f)(int32_t)`), so there is no such
/// prefix. The way round it is a one-field `datac`, which is what a program
/// needing a table of callbacks should write anyway.
fn fn_element_reason(ty: &Ty, container: &str) -> Option<String> {
    match ty {
        Ty::Fn { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet — a function value is a plain inline value, but a container's generated C type names its element type as a name prefix, and a function pointer has no such spelling (the name goes inside its declarator); hold it in a one-field 'datac' instead",
            ty.display()
        )),
        // A closure *is* nameable (it is an ordinary C struct), so the honest
        // reason is different: it owns an environment, and a container holds
        // its elements inline, so every element would have to be moved in and
        // dropped out through the container's own generated functions — which
        // is the same shape a function value is refused for, one level up
        // (A-4 phase E).
        Ty::Closure { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet — a closure owns the environment its captures were moved into, and a container's elements are held inline; put it in a 'Box' or a one-field 'datac' instead",
            ty.display()
        )),
        _ => None,
    }
}

/// Why a type cannot be a `List<T>` element, in one place so the `List<T>`
/// type position and `List.of(x)` give the same explanation.
fn list_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "List") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason} — a 'List' element is owned by the list, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', or a 'datac'"
        );
    }
    format!(
        "'{}' cannot be a 'List' element in v1 — a list's buffer holds its elements inline, so a nested container is not expressible; use a scalar, 'String', 'Bytes', or a 'datac' (or put the list in a 'Box')",
        ty.display()
    )
}

/// Why a type cannot be a `Map<V>` value, in one place so the `Map<V>` type
/// position and `Map.new()` share the explanation. The value set is the
/// `List` element set, so only the wording differs.
fn map_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Map") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason} — a 'Map' value is owned by the map, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', or a 'datac'"
        );
    }
    format!(
        "'{}' cannot be a 'Map' value in v1 — a map's value is stored inline, so a nested container is not expressible; use a scalar, 'String', 'Bytes', or a 'datac' (or put the map in a 'Box')",
        ty.display()
    )
}

/// The same for a `Box<T>` element, which may be a container: a box holds a
/// pointer, so `Box<List<i32>>` and `Box<Box<i32>>` are both fine.
fn box_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Box") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason} — a 'Box' owns its value, so the value has to outlive the box (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', or another 'Box'/'List'"
        );
    }
    format!(
        "'{}' cannot be a 'Box' element — use a scalar, 'String', 'Bytes', a 'datac', a 'List', or another 'Box'",
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
fn container_sig(key: &str, elem: &Ty, map_key: &Ty) -> Option<FnSig> {
    if key.starts_with("List#") {
        list_sig(key, elem)
    } else if key.starts_with("Map#") {
        map_sig(key, map_key, elem)
    } else {
        box_sig(key, elem)
    }
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

fn list_sig(key: &str, elem: &Ty) -> Option<FnSig> {
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
        "List#sort" if crate::types::list_sortable_elem(elem) => {
            FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit }
        }
        "List#reverse" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        "List#contains" if crate::types::list_comparable_elem(elem) => FnSig {
            params: vec![p("self", shared), p("v", contains_param(elem))],
            ret: Ty::Bool,
        },
        "List#slice" if crate::types::list_clonable_elem(elem) => FnSig {
            params: vec![p("self", shared), p("from", Ty::Usize), p("to", Ty::Usize)],
            ret: list.clone(),
        },
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

/// The `Map<V>` builtins (MK5-S5), instantiated for one value type. Keys are
/// always `String` (so every key parameter is a `str`), and the value type
/// decides the same things the `List` element type does:
///
/// - `get` returns a *copy* for a `Copy` value and a shared borrow for an
///   owned one, rooted at the receiver by the elision rule (`Map#get` is in
///   `builtin_borrow_root` for exactly this reason);
/// - `get_or` and `values` have to copy or clone a value *out*, which a
///   `datac` cannot do, so they are offered only for a clonable value type
///   (`types::list_clonable_elem`) — the same gate `List.slice` uses;
/// - `insert` moves the value in, `remove` moves it out.
fn map_sig(key: &str, map_key: &Ty, value: &Ty) -> Option<FnSig> {
    let map = Ty::Map(Box::new(map_key.clone()), Box::new(value.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(map.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(map.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let value_ref = Ty::Ref { mutable: false, inner: Box::new(value.clone()) };
    let clonable = crate::types::list_clonable_elem(value);
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

/// Why a type cannot be a `Map` key (A-2). The set is closed because the
/// compiler generates the key's hash and its equality for whatever the key
/// type is; a key that needs a user-supplied comparison would be a trait or a
/// comparator function, and v1 has neither.
fn map_key_error(ty: &Ty) -> String {
    format!(
        "'{}' cannot be a Map key — the key has to be 'String', an integer or 'bool', because the compiler generates the key's hash and its equality",
        ty.display()
    )
}

/// How a `Map`'s key is passed to its own methods: a `String` key is a `str`
/// view (the map copies it in, and a view is what a `str` caller already
/// has), and a scalar key is passed by value.
fn map_key_param(key: &Ty) -> Ty {
    match key {
        Ty::String => Ty::Str,
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
/// lifetime binds to — always the first reference parameter, which is why
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
/// `types::is_borrowed_field_ty` — a type that points at somebody else's data
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
/// system, but it points at text somebody else owns exactly as `&str` does —
/// which is why every rule that applies to a returned `&str` has to apply to a
/// returned `View` (A-8). `borrowck.rs` keys those rules on this predicate
/// rather than on `Ty::Ref` alone.
pub fn is_borrowed_value(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. } | Ty::View)
}

/// The elided borrow root for a builtin method, if it returns a borrow.
pub fn builtin_borrow_root(key: &str) -> Option<Option<usize>> {
    // `List#get`/`Box#get`/`Map#get` on an owned element type return a
    // borrow of the container itself — the first parameter, like every
    // builtin method's `self`. (For a `Copy` element the type checker never
    // asks, since the call's result is not a borrow; see
    // `borrowck::check_call`.)
    if key == "List#get" || key == "Box#get" || key == "Map#get" {
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
/// it and it satisfies any declared return type — including a borrowed one.
/// This is the `panic` ending every payload reader has:
///
/// ```kelp
/// fun as_str(v: &Json).R: &str {
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
/// `is_str_like` is the language's own statement that those two are one type —
/// `str.eq`, `print`, `str.len`, a `str` parameter and a `&str` return all take
/// either, because both are the same C value (`const char*`). Returns already
/// said so (`str_view_compatible`), but arm unification compared them by
/// identity, so ordinary code was refused:
///
/// ```kelp
/// fun f(v: &J).R: &str = when v {
///     S(s) -> s.as_str()
///     N -> "n"
/// }
/// // error: 'when' arms have different types: '&str' and 'str'
/// ```
///
/// The join is the **borrow**, not the bare `str`: the borrow checker
/// understands `Ty::Ref` — it pins the owner and it drives the elision rule —
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

/// Whether a type is `str` or a borrow of one — the two spellings of the
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
    if matches!(found, Ty::Char) && expected.is_integer() {
        " — a character literal takes the type its context asks for, so with nothing to ask it is 'char'; give it the type you mean (`val c: u8 = '\"'`) or write the value as an integer".to_string()
    } else {
        String::new()
    }
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
/// and no runtime cost — `math.PI` is the literal `3.141592653589793` in the
/// generated C.
#[derive(Debug, Clone)]
struct ConstInfo {
    value: tir::Expr,
}

/// A folded constant value. Deliberately small: a `const` may only be a
/// number, a `bool`, or a `&str`, so there is nothing to allocate and
/// nothing to drop.
#[derive(Debug, Clone, PartialEq)]
enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

/// Stand-in for "no later datacs" at the sites where every `datac` in the
/// program is already known (signatures, bodies, passdown types).
static EMPTY_DATACS: std::sync::LazyLock<std::collections::HashSet<String>> =
    std::sync::LazyLock::new(std::collections::HashSet::new);

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
    env: &TypeEnv,
) -> std::result::Result<HashMap<String, ConstInfo>, Diagnostic> {
    let mut out: HashMap<String, ConstInfo> = HashMap::new();
    for decl in &program.consts {
        let prefix = project.prefix_of(&decl.module);
        let aliases = project.alias_prefixes_of(&decl.module);
        let bare = bare_name(&decl.name, prefix).to_string();
        let declared = TypeScope {
            structs,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
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
        let value = fold_const(&decl.init, &declared, prefix, &aliases, &out)
            .map_err(|e| e.in_module(&decl.module))?;
        let literal = literal_for(&value, &declared).map_err(|why| {
            Diagnostic::new(
                format!(
                    "const '{bare}' is declared as '{}' but {why}",
                    declared.display()
                ),
                decl.span,
                decl.module.clone(),
            )
        })?;
        out.insert(decl.name.clone(), ConstInfo { value: literal });
    }
    Ok(out)
}

/// The name a module's own source writes for a prefixed top-level item.
fn bare_name<'a>(qualified: &'a str, prefix: &str) -> &'a str {
    qualified.strip_prefix(prefix).unwrap_or(qualified)
}

/// The folded value behind a `ConstInfo`, for a const referenced by another
/// const's initializer.
fn value_of(e: &tir::Expr) -> Option<ConstValue> {
    match &e.kind {
        tir::ExprKind::IntLit(v) => Some(ConstValue::Int(*v)),
        tir::ExprKind::FloatLit(v) => Some(ConstValue::Float(*v)),
        tir::ExprKind::BoolLit(b) => Some(ConstValue::Bool(*b)),
        tir::ExprKind::StringLit(s) => Some(ConstValue::Str(s.clone())),
        _ => None,
    }
}

/// Turns a folded value into the literal node a use site substitutes. The
/// typed C literal comes out of `codegen_c.rs`'s ordinary literal path — a
/// `const` adds no code path of its own.
fn literal_for(
    value: &ConstValue,
    declared: &Ty,
) -> std::result::Result<tir::Expr, String> {
    use ConstValue::*;
    let kind = match (value, declared) {
        (Int(v), t) if t.is_integer() => tir::ExprKind::IntLit(*v),
        // A `const C: char = 'a'` folds to an integer like any other scalar
        // constant and comes back out as a character literal, so the
        // substituted value prints and compares exactly as the literal did.
        (Int(v), Ty::Char) => tir::ExprKind::CharLit(*v as u32),
        (Int(v), t) if t.is_float() => tir::ExprKind::FloatLit(*v as f64),
        (Float(v), t) if t.is_float() => tir::ExprKind::FloatLit(*v),
        (Bool(b), Ty::Bool) => tir::ExprKind::BoolLit(*b),
        (Str(s), Ty::Str) => tir::ExprKind::StringLit(s.clone()),
        (_, t) => return Err(format!("its initializer is not a '{}' value", t.display())),
    };
    Ok(tir::Expr {
        kind,
        ty: declared.clone(),
        span: Span::new(0, 0),
    })
}

/// Folds a `const` initializer. Only what a compile-time value can honestly
/// be: literals, earlier constants, and arithmetic on those.
fn fold_const(
    expr: &ast::Expr,
    declared: &Ty,
    prefix: &str,
    aliases: &HashMap<String, String>,
    seen: &HashMap<String, ConstInfo>,
) -> Result<ConstValue> {
    let span = expr.span();
    let unsupported = || TypeError {
        message: "a 'const' initializer must be a literal, an earlier 'const', or arithmetic on those"
            .to_string(),
        span,
    };
    match expr {
        ast::Expr::IntLit(v, _) => Ok(if declared.is_float() {
            ConstValue::Float(*v as f64)
        } else {
            ConstValue::Int(*v)
        }),
        // A character literal folds by its code point, so `const A: u8 = 'a'`
        // and `const C: char = 'a'` are both constants in the ordinary
        // numeric sense; `check_expr` is what refuses a code point that does
        // not fit the declared type.
        ast::Expr::CharLit(v, _) => Ok(ConstValue::Int(*v as i64)),
        ast::Expr::FloatLit(v, _) => Ok(ConstValue::Float(*v)),
        ast::Expr::BoolLit(b, _) => Ok(ConstValue::Bool(*b)),
        ast::Expr::StringLit(s, _) => Ok(ConstValue::Str(s.clone())),
        ast::Expr::Ident(name, _) => {
            let key = format!("{prefix}{name}");
            match seen.get(&key).and_then(|c| value_of(&c.value)) {
                Some(v) => Ok(v),
                None => Err(TypeError {
                    message: format!(
                        "undefined const '{name}' — a 'const' can only use constants declared before it"
                    ),
                    span,
                }),
            }
        }
        ast::Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if !args.is_empty() {
                return Err(unsupported());
            }
            let ast::Expr::Ident(alias, _) = receiver.as_ref() else {
                return Err(unsupported());
            };
            let Some(target_prefix) = aliases.get(alias) else {
                return Err(TypeError {
                    message: format!("unknown module alias '{alias}' in a 'const' initializer"),
                    span,
                });
            };
            let key = format!("{target_prefix}{method}");
            match seen.get(&key).and_then(|c| value_of(&c.value)) {
                Some(v) => Ok(v),
                None => Err(TypeError {
                    message: format!("module '{alias}' has no const '{method}' declared before this one"),
                    span,
                }),
            }
        }
        ast::Expr::Unary { op, expr: sub, .. } => {
            let v = fold_const(sub, declared, prefix, aliases, seen)?;
            match (op, v) {
                (ast::UnOp::Neg, ConstValue::Int(i)) => Ok(ConstValue::Int(i.wrapping_neg())),
                (ast::UnOp::Neg, ConstValue::Float(f)) => Ok(ConstValue::Float(-f)),
                (ast::UnOp::Not, ConstValue::Bool(b)) => Ok(ConstValue::Bool(!b)),
                _ => Err(unsupported()),
            }
        }
        ast::Expr::Binary { op, lhs, rhs, .. } => {
            let l = fold_const(lhs, declared, prefix, aliases, seen)?;
            let r = fold_const(rhs, declared, prefix, aliases, seen)?;
            fold_binary(*op, l, r, span)
        }
        _ => Err(unsupported()),
    }
}

fn fold_binary(op: ast::BinOp, l: ConstValue, r: ConstValue, span: Span) -> Result<ConstValue> {
    use ast::BinOp::*;
    use ConstValue::*;
    let mismatch = || TypeError {
        message: "a 'const' expression mixes values of different kinds".to_string(),
        span,
    };
    let zero = || TypeError {
        message: "division by zero in a 'const' initializer".to_string(),
        span,
    };
    match (l, r) {
        (Int(a), Int(b)) => Ok(match op {
            Add => Int(a.wrapping_add(b)),
            Sub => Int(a.wrapping_sub(b)),
            Mul => Int(a.wrapping_mul(b)),
            Div if b == 0 => return Err(zero()),
            Div => Int(a.wrapping_div(b)),
            Mod if b == 0 => return Err(zero()),
            Mod => Int(a.wrapping_rem(b)),
            Eq => Bool(a == b),
            NotEq => Bool(a != b),
            Lt => Bool(a < b),
            LtEq => Bool(a <= b),
            Gt => Bool(a > b),
            GtEq => Bool(a >= b),
            And | Or => return Err(mismatch()),
            BitAnd => Int(a & b),
            BitOr => Int(a | b),
            BitXor => Int(a ^ b),
            // `wrapping_shl`/`wrapping_shr` mask the shift count to the
            // width, which is also what the generated C does on x86-64; a
            // `shr` on a signed value is arithmetic, matching `cc`'s own
            // reading of `>>` on an `int64_t`.
            Shl => Int(a.wrapping_shl(b as u32)),
            Shr => Int(a.wrapping_shr(b as u32)),
        }),
        (Float(a), Float(b)) => Ok(match op {
            Add => Float(a + b),
            Sub => Float(a - b),
            Mul => Float(a * b),
            Div => Float(a / b),
            Mod => Float(a % b),
            Eq => Bool(a == b),
            NotEq => Bool(a != b),
            Lt => Bool(a < b),
            LtEq => Bool(a <= b),
            Gt => Bool(a > b),
            GtEq => Bool(a >= b),
            And | Or | BitAnd | BitOr | BitXor | Shl | Shr => return Err(mismatch()),
        }),
        (Bool(a), Bool(b)) => match op {
            And => Ok(Bool(a && b)),
            Or => Ok(Bool(a || b)),
            Eq => Ok(Bool(a == b)),
            NotEq => Ok(Bool(a != b)),
            _ => Err(mismatch()),
        },
        _ => Err(mismatch()),
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
        let (params, kind) = if let Some(d) = program.datacs.iter().find(|d| &d.name == name) {
            (d.type_params.clone(), GenericKind::Record(d.fields.clone()))
        } else {
            let e = program
                .enums
                .iter()
                .find(|e| &e.name == name)
                .expect("the declaration lists name only declared types");
            (e.type_params.clone(), GenericKind::Sum(e.variants.clone()))
        };
        for p in &params {
            if Ty::from_name(p).is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "type parameter '{p}' shadows the builtin type '{p}' — pick another name"
                    ),
                    *span,
                    module.clone(),
                ));
            }
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
                kind,
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
        let params = decl_type_params(f).to_vec();
        if params.is_empty() {
            continue;
        }
        let key = sig_key(f);
        if f.receiver.is_none() && f.name == "main" {
            return Err(Diagnostic::new(
                "'main' cannot take type parameters — it is the program entry point"
                    .to_string(),
                f.span,
                f.module.clone(),
            ));
        }
        for p in &params {
            if Ty::from_name(p).is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "type parameter '{p}' shadows the builtin type '{p}' — pick another name"
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        if let Some(r) = &f.receiver {
            // A method on a generic type: the receiver writes the type's own
            // parameters, in order, and they are the method's type parameters
            // (A-2). A receiver that names a non-generic type has nothing to
            // parameterize, and one that writes the wrong names or order
            // would make the body's `T` mean something the type does not.
            let Some(type_tmpl) = generics.get(&r.type_name) else {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not a generic type — only a receiver on a generic type may declare type parameters",
                        r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            };
            if matches!(type_tmpl.kind, GenericKind::Sum(_)) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is an enum — an enum carries no methods in v1; write a free function over the instantiation instead",
                        r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if type_tmpl.params != params {
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

pub fn check_program(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<tir::Program, Diagnostic> {
    // A-2: the generic templates are collected before anything is resolved,
    // because a concrete declaration's field may name an instantiation
    // (`val p: Pair<i32, String>`) and the resolution of that field is what
    // creates it.
    let generics = collect_templates(program, project)?;
    let env = TypeEnv {
        generics: std::rc::Rc::new(generics),
        instances: std::rc::Rc::new(std::cell::RefCell::new(InstanceState::default())),
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
    // `HashMap`'s iteration order instead — which is what this did before —
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
                        env: env.clone(),
                        params: &EMPTY_PARAMS,
                        prefix,
                        aliases: &aliases,
                        later_datacs: Some(&later_datacs),
                        in_progress: in_progress.as_deref(),
                    }
                    .resolve(&f.ty, false)
                    .map_err(|err| err.in_module(&e.module))?;
                    if crate::types::is_borrowed_field_ty(&ty) {
                        return Err(Diagnostic::new(
                            format!(
                                "payload field '{}' is a borrowed type ('{}') — an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible — an owned 'String' is the way to keep text in one",
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
                });
            }
            enum_order.push(e.name.clone());
            enums.insert(
                e.name.clone(),
                EnumDef {
                    name: e.name.clone(),
                    variants,
                    order: *order,
                },
            );
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
                env: env.clone(),
                params: &EMPTY_PARAMS,
                prefix,
                aliases: &aliases,
                later_datacs: Some(&later_datacs),
                in_progress: in_progress.as_deref(),
            }
            .resolve(&f.ty, false)
            .map_err(|e| e.in_module(&d.module))?;
            // An owned field (`Bytes`, `String`, or a `datac` holding one) is
            // allowed from M6.2 on: `codegen_c.rs` generates a drop function
            // per datac, so dropping a struct recurses into its fields.
            if crate::types::is_borrowed_field_ty(&ty) {
                return Err(Diagnostic::new(
                    format!(
                        "field '{}' is a borrowed type ('{}') — a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible — an owned 'String' is the way to keep text in one",
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
        // Replaces the placeholder with the real definition. Order is kept
        // (same key, same position in `struct_order`), so codegen's emission
        // order is unchanged.
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields,
                order: *order,
            },
        );
    }

    // Compile-time constants (M6.3) are folded before signatures are built:
    // a `const` may not call anything, so it needs no function table, and
    // folding early means every later site can treat a const as a literal.
    let consts = collect_consts(program, project, &structs, &env)?;

    let mut sigs = HashMap::new();
    let mut overloads: HashMap<String, Vec<(Vec<Ty>, String)>> = HashMap::new();
    // Signature key per declaration, in declaration order: once overloads
    // exist a function's key is not derivable from its AST alone, and both
    // the passdown scan and the body checker need the same spelling the call
    // sites will use.
    let mut decl_keys: Vec<String> = Vec::with_capacity(program.functions.len());
    for f in &program.functions {
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
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
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
            Some(t) => scope.resolve_return(t).map_err(|e| e.in_module(&f.module))?,
            None => Ty::Unit,
        };
        // A returned borrow has to have something to borrow from, which the
        // borrow checker enforces per return site (M6.3): either the first
        // reference parameter, or a `str` literal (whose storage is static,
        // which is what `fun classify(n: i32).R: &str = "pos"` needs).
        // `borrowck.rs` cannot be stricter here without knowing the body.
        let key = sig_key_for(f, prefix, &params);
        if sigs.contains_key(&key) {
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
            && is_std_module(&f.module)
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
    let all_pds = collect_pds(
        program,
        project,
        &sigs,
        &structs,
        &enums,
        &consts,
        &overloads,
        &env,
        &fns,
    )?;
    let wired = scan_all(program, &all_pds, &structs, &decl_keys);

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
            &enums,
            &consts,
            &overloads,
            project,
            &all_pds,
            &wired,
            &initialized,
            &env,
            &fns,
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
    for (i, f) in program.functions.iter().enumerate() {
        // A-2: a generic function's body is checked once per instantiation,
        // in the worklist loop below — there is no signature to check it
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
            &overloads,
            project,
            &all_pds,
            &wired,
            &env,
            &fns,
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
        let checked = check_function(
            f,
            &key,
            &sig,
            &bindings,
            &sigs,
            &structs,
            &enums,
            &consts,
            &overloads,
            project,
            &all_pds,
            &wired,
            &env,
            &fns,
        )
        .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }
    // The capability data `codegen_c.rs` compiles in: each module's
    // patterns per domain, and which functions are gated (M7).
    let caps: Vec<tir::CapsGrant> = project
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
        records.extend(st.records.values().cloned());
        sums.extend(st.sums.values().cloned());
    }
    order_declarations(&mut records, &mut sums);
    // A-4: one function per `fun` literal, synthesized while bodies were
    // checked (a literal inside a literal's body included). They are ordinary
    // functions from here on — borrowck drops their owned parameters and
    // codegen emits them with a prototype like any other.
    functions.extend(fns.instances.borrow_mut().literals.drain(..));
    Ok(tir::Program {
        structs: records,
        enums: sums,
        pds: checked_pds,
        functions,
        caps,
        gated: project.gated().clone(),
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

/// Gives every declaration — hand-written and instantiated alike — a position
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
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    env: &TypeEnv,
    fns: &FuncEnv,
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
                env: env.clone(),
                params: &EMPTY_PARAMS,
                prefix,
                aliases: &aliases,
                later_datacs: None,
                in_progress: None,
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
                    overloads,
                    project,
                    &out,
                    None,
                    &HashMap::new(),
                    env,
                    fns,
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
            // Always a borrow, never a by-value copy: a `pd` has one storage
            // location, so a function that reads or writes it must reach that
            // storage. `pd var` is therefore an *exclusive* borrow (writes go
            // through it), `pd val` a shared one (many readers, no writer).
            param_ty: Ty::Ref {
                mutable: decl.mutable,
                inner: Box::new(value_ty.clone()),
            },
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
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    initialized: &std::collections::HashSet<String>,
    env: &TypeEnv,
    fns: &FuncEnv,
) -> Result<tir::Expr> {
    let mut ctx = base_ctx(
        &pd.module,
        sigs,
        structs,
        enums,
        consts,
        overloads,
        project,
        all,
        Some(pd.index),
        wired,
        env,
        fns,
    );
    // An initializer has no hidden parameters of its own: it reads the
    // globals that earlier initializers already assigned.
    ctx.pd_global_borrow = true;
    ctx.init_state = Some(initialized.clone());
    check_expr(init, &mut ctx, Some(&pd.value_ty))
}

fn base_ctx(
    module: &str,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    before: Option<usize>,
    wired: &HashMap<String, Vec<PdInfo>>,
    env: &TypeEnv,
    fns: &FuncEnv,
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
        overloads: overloads.clone(),
        module: module.to_string(),
        caps: project.caps_of(module),
        gated: project.gated().clone(),
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
        in_fn_literal: false,
        outer_bindings: HashMap::new(),
        captures: Vec::new(),
        outer_receiver: None,
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
        // An `extern` has no body to scan for passdown references.
        ast::FunctionBody::Extern => {}
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
    } else if matches!(
        base,
        Ty::Struct(_)
            | Ty::Bytes
            | Ty::String
            | Ty::List(_)
            | Ty::Box(_)
            | Ty::Map(..)
            | Ty::Closure { .. }
    ) {
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

/// Whether a type is one of the builtin owned buffers, or a borrow of one —
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
/// (M6.3 — until then this was a diagnostic telling the author to bind a
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
/// assigned into — an `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`).
fn is_tir_place(expr: &tir::Expr) -> bool {
    matches!(
        expr.kind,
        tir::ExprKind::Ident(_)
            | tir::ExprKind::FieldAccess { .. }
            | tir::ExprKind::EnumField { .. }
            // A capture is a place too: the environment holds the value (A-4
            // phase E), so `prefix.as_str()` inside a literal borrows it
            // rather than borrowing a temporary.
            | tir::ExprKind::Capture { .. }
            | tir::ExprKind::Deref { .. }
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
                // here — worth the passdown's own wording, since "shared
                // borrow" is not what the author wrote (M6.4).
                if let tir::ExprKind::Ident(name) = &inner.kind {
                    if let Some(pd) = ctx.pds.values().find(|p| &p.hidden == name) {
                        return Err(TypeError {
                            message: format!(
                                "cannot assign to passdown '{}': it is declared 'pd val' (only a 'pd var' can be assigned inside a function)",
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
    key: &str,
    sig: &FnSig,
    type_args: &HashMap<String, Ty>,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    env: &TypeEnv,
    fns: &FuncEnv,
) -> Result<tir::Function> {
    let receiver_struct = f
        .receiver
        .as_ref()
        .and_then(|_| struct_name_of(&sig.params[0].1).map(|s| s.to_string()));
    // Passdowns this function references become trailing hidden parameters
    // (see `PdInfo::arg` for the call-site side). `main` is the exception
    // downstream, not here: `codegen_c.rs` emits a fixed `main(argc, argv)`
    // regardless of parameters, and the body's `pd#...` identifiers then
    // resolve to the file-scope globals — which is why the hidden name and
    // the global name are the same string.
    let hidden = if key == "main" {
        Vec::new()
    } else {
        // An instantiation's passdown wiring is the template's: the scan runs
        // over declarations, and which passdowns a body names does not depend
        // on its type arguments (A-2).
        wired
            .get(key)
            .cloned()
            .or_else(|| {
                fns.instances
                    .borrow()
                    .origin
                    .get(key)
                    .and_then(|t| wired.get(t).cloned())
            })
            .unwrap_or_default()
    };
    let mut ctx = base_ctx(
        &f.module,
        sigs,
        structs,
        enums,
        consts,
        overloads,
        project,
        all_pds,
        None,
        wired,
        env,
        fns,
    );
    ctx.type_args = type_args.clone();
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
        ast::FunctionBody::Extern => {
            // No `.kp` body: `codegen_c.rs` writes the compiler-supplied C
            // (see `externs.rs`). Confirm there *is* one before accepting
            // the declaration — otherwise the failure would surface as a
            // `cc` error at best, or as a silently missing symbol.
            let prefix = project.prefix_of(&f.module);
            let bare = f.name.strip_prefix(prefix).unwrap_or(&f.name);
            if !crate::externs::has_body(&f.module, bare) {
                return Err(TypeError {
                    message: format!(
                        "extern function '{bare}' has no compiler-supplied body — only compiler-shipped stdlib modules may declare 'extern fun'"
                    ),
                    span: f.span,
                });
            }
            tir::Block {
                stmts: vec![],
                tail: None,
                span: f.span,
            }
        }
    };

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1/M2), so a mismatch is only reported when a tail
    // expression is actually present and disagrees with the declared return
    // type.
    // A body whose tail is a `panic` diverges, so it satisfies any declared
    // return type: `fun unwrap<T>(o: &Option<T>).R: T { ... panic("...") }` is
    // the "this cannot happen" accessor. Nothing else in the language is
    // divergence-aware — an `if` arm cannot be a `panic` — so this is
    // deliberately the narrow rule the accessors need.
    let tail_diverges = body
        .tail
        .as_ref()
        .is_some_and(|e| tail_diverges(e));
    if sig.ret != Ty::Unit
        && body.tail.is_some()
        && !tail_diverges
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

    Ok(tir::Function {
        name: key.to_string(),
        params: {
            params.extend(hidden.iter().map(|pd| (pd.hidden.clone(), pd.param_ty.clone())));
            params
        },
        ret: sig.ret.clone(),
        body,
        module: f.module.clone(),
        extern_: f.extern_,
        // Only a `fun` literal that captured something has an environment;
        // a declaration never does (A-4 phase E).
        env: None,
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
                Some(t) => Some(ctx.type_scope().resolve(t, false)?),
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
            Ok(tir::Stmt::Return {
                value,
                span: stmt_span(stmt),
            })
        }
        ast::Stmt::Expr { expr, .. } => {
            let checked = check_expr(expr, ctx, None)?;
            // A non-tail expression statement discards its value, and nothing
            // owns an owned value afterwards — v1 has no drop point for a
            // temporary, so this would leak. Reject it instead.
            if !checked.ty.is_copy() {
                return Err(TypeError {
                    message: format!(
                        "this expression produces an owned '{}' value that is never used — bind it to a 'val' or 'var', print it, or pass it to something that takes it by value",
                        checked.ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(tir::Stmt::Expr(checked))
        }
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

/// A `fun` literal (A-4): `fun(x: i32).R: i32 = x * 2`.
///
/// A literal is one synthesized top-level function, so most of the work is
/// the same work a declaration needs — resolve the signature, bind the
/// parameters, check the body against the declared return — and the rest is
/// the two things that make it a *literal*: its body is checked in a scope of
/// its own (no captures yet), and the checked body is stored where codegen
/// will emit it.
fn check_fn_literal(
    params_ast: &[ast::Param],
    return_type: &Option<ast::TypeExpr>,
    body: &ast::FunctionBody,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    // The signature, resolved where the literal was written: a parameter type
    // resolves against the enclosing module and the enclosing generic
    // instance's type parameters, exactly as a nested declaration's would.
    let (params, ret) = {
        let scope = ctx.type_scope();
        let mut params = Vec::with_capacity(params_ast.len());
        for p in params_ast {
            params.push((p.name.clone(), scope.resolve_param(p)?));
        }
        let ret = match return_type {
            Some(rt) => {
                // A borrow would outlive the frame that produced it, and a
                // function value is a plain pointer to code — the elision
                // rule that says *which* argument a borrow comes from has no
                // place to be written here.
                if rt.amp {
                    return Err(TypeError {
                        message: format!(
                            "a 'fun' literal cannot return a borrow ('&{}') — an indirect call has no parameter an elided lifetime could bind to; return an owned value instead",
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
    // One literal, one function. The key starts with `#`, which no `.kp`
    // identifier can contain, so it cannot collide with anything a program
    // declares (the same trick receiver methods use for `Type#name`).
    let key = {
        let mut st = ctx.fns.instances.borrow_mut();
        st.literal_count += 1;
        format!("#funlit{}", st.literal_count)
    };
    // The body's own scope. The enclosing locals are not *bindings* here —
    // they are the names the body may capture, which is what makes the
    // literal a closure when it names one (A-4 phase E). The lookup map
    // carries their types, because a capture's type is the captured
    // binding's type. Three sources, because a literal can be nested:
    //
    // * the enclosing frame's locals,
    // * the enclosing literal's own captures (a literal inside a literal),
    // * and everything *that* literal could see, transitively — which is how
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
    let mut tir_params = Vec::with_capacity(params.len());
    for (name, ty) in &params {
        inner.define(name, ty.clone(), false);
        tir_params.push((name.clone(), ty.clone()));
    }
    let checked = match body {
        ast::FunctionBody::Block(b) => check_block(b, &mut inner, Some(&ret))?,
        ast::FunctionBody::Expr(e) => {
            let value = check_expr(e, &mut inner, Some(&ret))?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(value)),
                span: e.span(),
            }
        }
        ast::FunctionBody::Extern => unreachable!("the parser requires a literal body"),
    };
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
    // is *evaluated* — which is where a capture moves its value out of the
    // enclosing frame. Each field's initializer is written in the enclosing
    // function's terms: a local by name, or — for a literal nested in a
    // literal — a read of the enclosing closure's environment.
    let mut captures = Vec::with_capacity(capture_list.len());
    let mut env_fields: Vec<(String, Ty)> = Vec::with_capacity(capture_list.len());
    for (i, (name, ty)) in capture_list.iter().enumerate() {
        if crate::types::is_borrowed_field_ty(&ty) {
            return Err(TypeError {
                message: format!(
                    "a 'fun' literal cannot capture '{name}': it is a borrow ('{}'), and the environment it would be stored in outlives the frame the borrow points into — capture an owned value, or pass it as a parameter of the literal",
                    ty.display()
                ),
                span,
            });
        }
        let init = if ctx.lookup(name).is_some() {
            // A binding of the frame the literal was written in: the value
            // moves (or, for `Copy`, copies) into the environment, and the
            // enclosing binding is marked moved by `borrowck.rs`.
            tir::Expr {
                kind: tir::ExprKind::Ident(name.clone()),
                ty: ty.clone(),
                span,
            }
        } else if ctx.note_capture(name).is_some() {
            // The name belongs to the environment of the literal this one was
            // written inside — so the enclosing literal captures it too, and
            // this literal copies it out of that environment. Moving it out
            // instead would be a partial move (A-11), which is why only a
            // `Copy` capture can be nested this way.
            if !ty.is_copy() {
                return Err(TypeError {
                    message: format!(
                        "cannot capture '{name}' into this literal: it belongs to the enclosing closure's environment, and moving it out of there is a partial move (v1 has no partial-move rule) — capture an owned value from the frame that owns it, or copy a `Copy` one"
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
        env_fields.push((crate::types::capture_field(i), ty.clone()));
        captures.push(init);
    }
    let fn_ty = if env_fields.is_empty() {
        // Nothing captured: this is a plain function value, exactly the value
        // phase C produced. Keeping the two apart is what keeps every
        // existing `fun(...)` placement (a `datac` field, a container
        // element's absence, a `Copy` binding) meaning what it did.
        Ty::Fn {
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
        module: ctx.module.clone(),
        extern_: false,
        env,
    });
    let kind = match captures.is_empty() {
        true => tir::ExprKind::FnRef(key),
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
/// every site that knows an expected type gets it — a call argument, a `val`
/// with a written type, a `return`, a `datac` field, a container element, a
/// `when` arm, a block tail — with no list of sites to keep in sync. The
/// alternative shape (coerce at each site) is the same behaviour with N
/// chances to forget one.
fn check_expr(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    let value = check_expr_inner(expr, ctx, expected)?;
    Ok(coerce_str_to_string(value, expected))
}

/// A `str` used where a `String` is expected becomes one, by `String.from_str`.
///
/// `String.from_str("...")` was the single most repeated construct in the JSON
/// library this came from — once per parser error message, once per `kind()`
/// arm, once per `String.from_str("null")` — because a literal is a `str` and
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
    // A plain function value where a closure is expected (A-4 phase E): the
    // one widening in this phase, handled here so that every site which knows
    // the expected type gets it — a call argument, a `val` with a written
    // type, a `return`, a `datac` field, `Box.of(x)`. The value is checked
    // with no expectation of its own (nothing about a `fun` literal depends
    // on the closure type it will be widened into) and then wrapped.
    if let Some(Ty::Closure { .. }) = expected {
        let expected = expected.expect("matched above");
        let value = check_expr(expr, ctx, None)?;
        return Ok(coerce_fn_to_closure(expected.clone(), value));
    }
    match expr {
        // A `fun` literal (A-4): one synthesized top-level function per
        // literal, and the expression's value is a reference to it.
        ast::Expr::FnLit {
            params,
            return_type,
            body,
            span,
        } => check_fn_literal(params, return_type, body, *span, ctx),
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
        // A character literal (open item 53) takes the type its context asks
        // for, exactly as an integer literal does, and `char` when nothing
        // constrains it. That single rule is what lets byte-level code read
        // the way it thinks: `c == '"'` with `c: u8`, `out.push(' ')` into a
        // `String`, `n + '0'` to make a digit.
        //
        // A code point that does not fit the requested integer type is
        // refused rather than truncated — `'中'` in a `u8` position is a
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
                        "a 'fun' literal cannot read passdown '{name}' — a passdown is passed to the function that names it as a hidden argument, and a function value is a plain pointer to code; pass the value as a parameter instead"
                    ),
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
                if ctx.init_state.is_some() && !pd.value_ty.is_copy() {
                    return Err(TypeError {
                        message: format!(
                            "passdown '{}' cannot be read in another passdown's initializer: its value is not Copy",
                            pd.qualified
                        ),
                        span: *span,
                    });
                }
                return Ok(pd.value(*span, ctx.pd_global_borrow));
            }
            // Not a local, receiver field, or passdown — a declared `fun`
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
            // Not a local, receiver field, or passdown — a compile-time
            // constant (M6.3), substituted as the literal it folded to.
            if let Some(c) = ctx.consts.get(&ctx.qualify_own(name)).cloned() {
                let mut value = c.value;
                value.span = *span;
                return Ok(value);
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
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Capture { index },
                        ty,
                        span: *span,
                    });
                }
                // A field of the receiver the literal was written in is a
                // name the reader can *see*, but reading it out of a borrowed
                // receiver is a partial move (A-11) — which is why the field
                // itself, not just the literal, has to be the reason.
                if let Some(owner) = &ctx.outer_receiver {
                    if ctx.field_of(owner, name).is_some() {
                        return Err(TypeError {
                            message: format!(
                                "a 'fun' literal cannot capture '{name}': it is a field of '{owner}', and reading a field out of a borrowed receiver by value is a partial move (v1 has no partial-move rule) — bind a `Copy` field to a local first (`val {name}0 = {name}`) and capture that, or take the whole receiver as a 'move' parameter"
                            ),
                            span: *span,
                        });
                    }
                }
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
                                "cannot apply '~' to a value of type '{}' — '~' complements the bits of an integer",
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
                    // `math.PI` — a module's `const`, written without call
                    // parentheses (M6.3). Everything else in a module is a
                    // function and needs a call.
                    if let Some(prefix) = ctx.aliases.get(name).cloned() {
                        let qualified = format!("{prefix}{field}");
                        if let Some(c) = ctx.consts.get(&qualified).cloned() {
                            let mut value = c.value;
                            value.span = *span;
                            return Ok(value);
                        }
                        // `val f = math.sqrt` — a module's item named as a
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
                        if ctx.sigs.keys().any(|k| k.split('@').next() == Some(&qualified)) {
                            return Err(TypeError {
                                message: format!(
                                    "'{name}.{field}' is an overloaded stdlib function — its parameter types are part of the name, so it is not a value; write a non-generic wrapper function and pass that"
                                ),
                                span: *span,
                            });
                        }
                    }
                    return Err(TypeError {
                        message: format!(
                            "'{name}' is a module, not a value — call its items as '{name}.{field}(...)'"
                        ),
                        span: *span,
                    });
                }
            }
            // `Option.None` — a payload-less variant of a generic `enum`
            // (A-2), the generic counterpart of the `Shape.Empty` case below.
            if let Some(tmpl_name) = generic_type_path(base, ctx) {
                return check_generic_bare_variant(&tmpl_name, field, *span, ctx, expected);
            }
            // `Shape.Empty` — a payload-less variant, written without a call
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
                            "'{field}' is not a variant of enum '{ename}' — the variants are {}",
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
                            "variant '{}.{}' carries payload ({}) — construct it as '{}.{}({})'",
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
                        "enum '{sname}' has no field '{field}' — match it with 'when {sname} {{ ... }}' to read a payload"
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
        ast::Expr::Try { expr: sub, span } => check_try(sub, *span, ctx),
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
            // `Option.Some(v = 1)` — construction of a generic `enum`
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
            // `Shape.Circle(r = 1.0)` — construction of an `enum` variant
            // (A-1). Checked before method resolution because it is the same
            // `Type.member(...)` shape as a method call, and the receiver is
            // a *type*, not a value.
            if let Some(ename) = enum_path(receiver, ctx) {
                return check_enum_construction(&ename, method, args, *span, ctx);
            }
            if !type_args.is_empty() {
                return Err(TypeError {
                    message: format!(
                        "'{method}' does not take type arguments — a method's type parameters come from its receiver"
                    ),
                    span: *span,
                });
            }
            check_method_call_enum_guard(receiver, method, *span, ctx)?;
            check_method_call(receiver, method, args, *span, ctx, expected)
        }
        ast::Expr::When {
            scrutinee,
            arms,
            span,
        } => check_when(scrutinee, arms, *span, ctx, expected),
    }
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
/// types are known — `match_args` needs the types, and the types need the
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
        out.push(scope.resolve(t, true)?);
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
/// ignored here — its argument is checked against it later, by `match_args`,
/// which is also where a mismatch is reported.
/// A written type name as its canonical key, resolved against the module the
/// *declaration* was written in rather than the module the call site is in —
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
    // lets `map<T, U>(o: &Option<T>, f: fun(&T).R: U)` take both `T` and `U`
    // from its arguments, and it is the only place a function type is bound.
    // A closure type (A-4 phase E) pairs the same way, which is what lets a
    // callback written as `f: closure(&T).R: U` infer `U` from a literal.
    if let Some(written_params) = &written.fn_params {
        let implied = match actual {
            Ty::Fn { params, ret } | Ty::Closure { params, ret } => Some((params, ret)),
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
                        "cannot infer type parameter '{p}' of '{name}' here — annotate the binding (e.g. 'val x: {name}<{}> = ...') or pass an argument that determines it",
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
/// place that says so is the field's written type — which can be resolved
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
        env: ctx.env.clone(),
        params: bindings,
        prefix,
        aliases,
        later_datacs: None,
        in_progress: None,
    };
    scope.resolve(written, false).ok()
}

/// `Pair(first = 1, second = "x")` — construction of a generic `datac` (A-2).
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
                "'{}' is an enum — construct one of its variants: {}",
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
    // pin a parameter *before* an argument is probed — which is what makes
    // `xs.push(Option.Some(value = 7))` infer `Option<i64>` from the list's
    // element type rather than defaulting the literal to `i32`.
    let type_args = if explicit.is_empty() {
        let mut bindings = HashMap::new();
        bind_from_expected(&tmpl, expected, ctx, &mut bindings);
        for (i, f) in fields.iter().enumerate() {
            let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &f.ty, &bindings);
            let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &f.ty, &probed.ty, &tmpl.params, &mut bindings);
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
        },
        ty: Ty::Struct(sdef.name.clone()),
        span,
    })
}

/// `Option.Some(v = 1)` — construction of one variant of a generic `enum`
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
                "'{}' is a generic datac — construct it as '{}(...)'",
                tmpl.name, tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'{}.{}' is not a variant — the variants of '{}' are {}",
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

/// `Option.None` — a payload-less variant of a generic `enum`, written without
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
                "'{}' is a generic datac — read its fields with '.' after constructing it",
                tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            message: format!(
                "'{}.{}' is not a variant — the variants of '{}' are {}",
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
                "variant '{}.{}' carries payload ({}) — construct it as '{}.{}({})'",
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
/// own `Result`/`Option` works with no import and no blessed name — the shape
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
/// the enclosing function has to return an enum of the same shape — with the
/// *same* failure payload type for `Ok`/`Err`, because v1 has no error
/// conversion. What the operator evaluates to is the success payload.
fn check_try(sub: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    let inner = check_expr(sub, ctx, None)?;
    // The operand is read as a whole value and its payload is moved out, so a
    // borrow is not usable here — `when`/`if let` is the form for that.
    let source = match &inner.ty {
        Ty::Struct(name) => name.clone(),
        other => {
            return Err(TypeError {
                message: format!(
                    "'?' needs a value shaped 'Ok'/'Err' or 'Some'/'None' held by value, not '{}' — match it with 'when' or 'if let' instead",
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
                    "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' — it returns '{}'",
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
                "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' — it returns '{}', with {}",
                ret.display(),
                names.join(", ")
            ),
            span,
        });
    };
    if shape.err_has_payload != rshape.err_has_payload {
        return Err(TypeError {
            message: format!(
                "'?' here propagates {} failure, but the enclosing function returns '{}' — the two have to be the same shape",
                if shape.err_has_payload { "a carrying" } else { "a payload-less" },
                ret.display()
            ),
            span,
        });
    }
    if shape.err_has_payload {
        let from = &edef.variants[shape.err].fields[0].ty;
        let to = &rdef.variants[rshape.err].fields[0].ty;
        if from != to {
            return Err(TypeError {
                message: format!(
                    "'?' cannot convert a failure: '{}' carries '{}' and the enclosing function returns '{}', whose failure carries '{}' — v1 has no error conversion, so the two have to be the same type",
                    inner.ty.display(),
                    from.display(),
                    ret.display(),
                    to.display()
                ),
                span,
            });
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
        },
        ty: ok_ty,
        span,
    })
}

/// `Shape.Circle(r = 1.0)` — one variant's worth of payload, checked against
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
    let Some((vidx, vdef)) = edef.variant(variant) else {
        let names: Vec<String> = edef
            .variants
            .iter()
            .map(|v| format!("'{}'", v.name))
            .collect();
        return Err(TypeError {
            message: format!(
                "'{ename}.{variant}' is not a variant — the variants of '{ename}' are {}",
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

/// `when s { Circle(r) -> ... else -> ... }` (A-1) — a pattern match over an
/// `enum`, lowered here to a chain of `if <tag test> { <bindings> <body> }
/// else ...`: the same TIR shape the boolean `when` already lowers to in the
/// parser, plus two node kinds (`EnumTag`, `EnumField`) that codegen emits as
/// `.tag ==` and `.v.<Variant>.<field>`. No new statement machinery and no new
/// drop plan are needed as a result.
fn check_when(
    scrutinee: &ast::Expr,
    arms: &[ast::WhenArm],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // The scrutinee has to be a place: its payloads are copied or borrowed
    // per arm, and a temporary would have to be evaluated once and kept alive
    // behind the borrow checker's back. Binding it to a `val` first is the
    // fix, and the message says so.
    if !is_ast_place(scrutinee) {
        return Err(TypeError {
            message: "the scrutinee of a pattern 'when' must be a place (a binding, a field path, or a '*deref') — bind it to a 'val' first".to_string(),
            span: scrutinee.span(),
        });
    }
    let scrut = check_expr(scrutinee, ctx, None)?;
    let Some(ename) = struct_name_of(&scrut.ty).map(|s| s.to_string()) else {
        return Err(TypeError {
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{}' is not one — for a chain of conditions write 'when {{ cond -> value }}' with no scrutinee",
                scrut.ty.display()
            ),
            span: scrut.span,
        });
    };
    let Some(edef) = ctx.enum_def(&ename) else {
        return Err(TypeError {
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{ename}' is a datac — read its fields with '.' instead"
            ),
            span: scrut.span,
        });
    };

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
                            "'{}' is not a variant of enum '{ename}' — the variants are {}",
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
                let (ty, init) = if f.ty.is_copy() {
                    (f.ty.clone(), field)
                } else {
                    // An owned payload binds as a shared borrow: v1 has no
                    // partial moves, so a payload can never be moved out of
                    // the scrutinee — the same rule `List.get` follows for an
                    // owned element.
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
                span: arm.span,
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
                    "this 'when' is not exhaustive: {} of enum '{ename}' has no arm — add an arm, or an 'else' arm",
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
    // paths — no uninitialized read, and nothing for a C warning to flag.
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
            span,
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
    if is_std_module(&ctx.module) {
        format!(
            " — this is inside the shipped standard library ('{}'), so the instantiation you asked for is one it does not offer: a helper that hands a value out is written for a 'Copy' payload or element, and the module's header says which",
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
        let sdef = ctx.struct_def(sname).expect("struct types are only produced for known structs");
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

/// Whether a comparison operator (`==`/`!=`/`<`/…) can be applied to this
/// type at all. Comparisons are only generated for scalars C can compare
/// directly: an owned value, a `datac`, or a `str` has no comparison of its
/// own, and emitting one produces invalid C at `cc` (or, for `str`, a
/// comparison of *addresses* rather than text — silently wrong, which is
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
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text — use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        );
    }
    match ty {
        Ty::Str => format!(
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text — use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        ),
        // A `View` *could* compare byte-wise (it has a length), but `==` on a
        // non-scalar is not how any other type reads and `view.eq` says what
        // it does.
        Ty::View => format!(
            "cannot compare 'View' values with '{sym}': use 'view.eq(other)' — it compares the bytes and the lengths, where '{sym}' would compare the pointers"
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

/// Whether the expression is a bare numeric or character literal — a value
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
/// `operator requires matching numeric operands, found 'i32' and 'u32'` — for
/// a rule the reference already stated ("a literal has no width of its own; it
/// takes the integer type its context requires"). The sibling operand *is* the
/// context.
///
/// So: when there is no expectation to work from and the left side is a bare
/// literal while the right side is not, the right side is checked first and
/// the literal takes its type. Both literals, or neither, still go left to
/// right — nothing about the common cases changes.
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
        // Bitwise operators (A-9). Integral operands of the same type,
        // result the same type — the same rule arithmetic uses, one domain
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
            if !comparable(&l.ty) {
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

    Ok(result)
}

/// Checks one call-site argument against its matched parameter type. When
/// the parameter is `Ty::Ref`, the caller never writes `&` — a plain place
/// argument (`p`, `p.x`) is auto-borrowed here (the callee's signature is
/// what decides shared vs exclusive), matching `plan/syntax.md`'s "shared
/// borrow is implicit — write nothing." An argument that's already a borrow
/// (an explicit `&expr`, or forwarding an existing `Ref`-typed parameter)
/// type-matches directly instead of double-wrapping; a shared parameter
/// accepts an exclusive argument (implicit downgrade), never the reverse.
/// A call whose result *borrows from an argument* (the elision rule) cannot
/// borrow from a temporary: the temporary is freed at the end of the
/// enclosing statement, and codegen would have to drop it before the borrow
/// could be used. Argument matching itself allows a temporary for a shared
/// borrow — that is what makes `str.join(str.split(s, sep), sep)` legal — so
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
    if !is_tir_place(inner) {
        return Err(TypeError {
            message: format!(
                "this call returns a borrow of its parameter '{}', so that argument has to be a variable or a field — a temporary would be freed before the borrow could be used",
                params[idx].0
            ),
            span,
        });
    }
    Ok(())
}

/// `pname` is how the argument is named in a diagnostic — `'b'` for a
/// declared parameter, `1` for the position in a call through a function value
/// (a function *type* has no parameter names).
fn check_call_arg(pname: &str, pty: &Ty, arg_expr: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let Ty::Ref { mutable, inner } = pty else {
        let checked = check_expr(arg_expr, ctx, Some(pty))?;
        // A `&str` view and a `str` are the same value (both are the flat
        // borrowed view type), so a view satisfies a `str` parameter
        // directly — that is what makes `str.len(s.as_str())` work (M6.3).
        if is_str_like(pty) && is_str_like(&checked.ty) {
            return Ok(checked);
        }
        // The parameter is bare (by value), so this call *moves* its argument
        // in — which a `move T` parameter and every container's
        // `push`/`set`-shaped builtin both do. A borrow cannot be moved out
        // of, so name that instead of leaving a plain type mismatch
        // (`'&String'` against `'String'`) to imply the caller forgot an `&`
        // (A-4).
        if matches!(checked.ty, Ty::Ref { .. }) && !pty.is_copy() {
            return Err(TypeError {
                message: format!(
                    "argument {pname} has type '{}', but this call moves its argument in, and a borrow cannot be moved out of — pass an owned value (a 'Box' holding one moves it out with 'take()'){}",
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
                    "argument {pname} has type '{}', expected '{}'{}",
                    checked.ty.display(),
                    pty.display(),
                    char_integer_hint(&checked.ty, pty)
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    };

    let checked = check_expr(arg_expr, ctx, Some(inner))?;
    match &checked.ty {
        t if t == inner.as_ref() => {
            // A *shared* borrow may be taken of a temporary (`str.join(parts
            // = str.split(s, sep), sep = "-")`): the temporary lives for the
            // whole enclosing expression, which is exactly how long a
            // call-scoped borrow lasts, and `codegen_c.rs` hoists it into a C
            // temporary to take its address. An *exclusive* borrow still
            // needs a place, since the callee may write through it and a
            // temporary has nowhere for a write to land.
            if *mutable {
                if !is_tir_place(&checked) {
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
                    "argument {pname} has type '{}', expected '{}'{}",
                    other.display(),
                    pty.display(),
                    char_integer_hint(other, pty)
                )
            }),
            span: arg_expr.span(),
        }),
    }
}

/// The one widening a function value gets (A-4 phase E): a plain `fun` value
/// used where a closure is expected is wrapped, so the two are separate types
/// but one *value* can be written either way. A signature mismatch is left
/// alone — the caller's own type comparison reports it, and a coerced value
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

/// The one sentence that explains why a plain function value and a closure are
/// different types, when one is passed where the other is expected (A-4 phase
/// E). The two are the same shape of value and are called identically, so a
/// bare `expected 'closure(i32).R: i32', found 'fun(i32).R: i32'` would leave
/// the reader to guess which half is missing. The expected side is looked
/// through a borrow, because a parameter of a closure type is one.
fn closure_kind_mismatch(expected: &Ty, actual: &Ty) -> Option<String> {
    let expected = match expected {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    match (expected, actual) {
        (Ty::Closure { .. }, Ty::Fn { .. }) => Some(
            "this is a plain function value, not a closure: its body captured nothing, so it carries no environment — a `fun` literal becomes a closure when it names a binding from the enclosing scope, and a declared `fun` is always a plain function value".to_string(),
        ),
        (Ty::Fn { .. }, Ty::Closure { .. }) => Some(
            "this is a closure: its body captured a binding from the enclosing scope, so it carries an environment a plain function value has no room for — write the expected type as 'closure(...)' (the same parameter list and '.R:'), or pass a declared function instead".to_string(),
        ),
        _ => None,
    }
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
        let printable = arg.ty.is_numeric()
            || matches!(arg.ty, Ty::Bool | Ty::Char)
            || is_str_like(&arg.ty)
            || is_printable_buffer(&arg.ty);
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

    // `Shape(r = 1.0)` — an enum is constructed by naming a variant, never by
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
                "'{ename}' is an enum — construct one of its variants: {}",
                variants.join(", ")
            ),
            span,
        });
    }
    if let Some(sdef) = ctx
        .struct_def(callee)
        .or_else(|| ctx.struct_def(&ctx.qualify_own(callee)))
    {
        let params: Vec<(String, Ty)> = sdef.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
        let checked = match_args(&format!("datac '{}'", sdef.name), &params, args, span, ctx)?;
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
    let owned_key = ctx.qualify_own(callee);
    let direct = ctx
        .sig_of(&owned_key)
        .map(|s| (owned_key.clone(), s))
        .or_else(|| ctx.sig_of(callee).map(|s| (callee.to_string(), s)));
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
                    // consulted after the program's own signatures — a user
                    // function of the same name wins, and there is no
                    // shadowing surprise.
                    None => match builtin_sig(callee) {
                    Some(sig) => (callee.to_string(), sig),
                    None => {
                        return Err(TypeError {
                            message: match qualified_hint(
                                callee,
                                ctx.structs
                                    .keys()
                                    .chain(ctx.sigs.keys())
                                    .chain(ctx.fns.templates.keys()),
                                &ctx.aliases,
                            ) {
                                Some(hint) => format!("call to undefined function '{callee}' — did you mean {hint}?"),
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
    let mut checked = match_args(
        &format!("function '{}'", display_callee(&key)),
        &sig.params,
        args,
        span,
        ctx,
    )?;
    require_place_for_borrowed_arg(&sig.params, &sig.ret, &checked, span)?;
    checked.extend(hidden_args(&key, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: key,
            args: checked,
        },
        ty: sig.ret,
        span,
    })
}

/// The key of the generic *function* template `name` or `owned` names, if
/// either does (A-2).
fn func_template_named(ctx: &Ctx, name: &str, owned: &str) -> Option<String> {
    if ctx.fns.templates.contains_key(owned) {
        return Some(owned.to_string());
    }
    ctx.fns
        .templates
        .contains_key(name)
        .then(|| name.to_string())
}

/// The template key of a method declared on a generic type (A-2): `owner` is
/// the receiver's *instance* key (`Pair<i32, String>`), and the template is
/// declared under the type template's own name (`Pair#first`).
fn method_template_key(ctx: &Ctx, owner: &str, method: &str) -> Option<String> {
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
    if tmpl.receiver.is_some() {
        if let Some(rk) = receiver_ty.and_then(struct_name_of) {
            if let Some((_, targs)) = ctx.env.instances.borrow().origin.get(rk).cloned() {
                for (p, a) in tmpl.params.iter().zip(targs.iter()) {
                    bindings.insert(p.clone(), a.clone());
                }
            }
        }
    }
    let what = format!("function '{}'", display_callee(&tmpl.key));
    // With an explicit instantiation there is nothing to infer — the arguments
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
            }
        }
    }
    let type_args = if explicit.is_empty() {
        finish_type_args(&tmpl.key, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.key, &tmpl.params, explicit, span)?
    };
    let key = instance_key(&tmpl.key, &type_args);
    if let Some(sig) = ctx.sig_of(&key) {
        return Ok((key, sig));
    }
    // The parameter and return types, resolved from the *final* arguments —
    // whichever way they were determined, so an explicit instantiation needs
    // no inference bindings at all. A failure here is a problem in the
    // template's own signature, written in the template's file, so the
    // message names the instance and the span points at the use site, which
    // is the module the caller will attribute the error to.
    let resolved: HashMap<String, Ty> = tmpl
        .params
        .iter()
        .cloned()
        .zip(type_args.iter().cloned())
        .collect();
    let scope = TypeScope {
        structs: &ctx.structs,
        env: ctx.env.clone(),
        params: &resolved,
        prefix: &tmpl.prefix,
        aliases: &tmpl.aliases,
        later_datacs: None,
        in_progress: None,
    };
    let mut params = Vec::new();
    if let Some((_, receiver_mutable)) = &tmpl.receiver {
        let rk = receiver_ty
            .and_then(struct_name_of)
            .expect("a receiver method is only instantiated for a struct receiver");
        params.push((
            "self".to_string(),
            implicit_borrow_wrap(Ty::Struct(rk.to_string()), *receiver_mutable),
        ));
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
                "generic instantiation did not terminate — more than {MAX_FUNCTION_INSTANTIATIONS} function instances were created; a generic function that calls itself with a different type argument grows without bound"
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

/// " — it is declared for i32, i64, f64", for the message above.
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
                return format!(" — it is declared for {}", names.join(", "));
            }
        }
    }
    String::new()
}

/// The hidden passdown arguments a call to `callee` has to pass, in the
/// order the callee's signature declares them (M5.2). Each one is the
/// generated global for that passdown — a call site never receives its
/// callees' passdowns itself, so the value always comes from the global
/// here; the callee's parameter is what makes the dependency visible in its
/// own signature.
fn hidden_args(callee: &str, span: Span, ctx: &Ctx) -> Result<Vec<tir::Expr>> {
    let Some(pds) = ctx.wired_for(callee) else {
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
        out.push(pd.arg(span, ctx.pd_global_borrow));
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
                    || matches!(value.ty, Ty::Bool | Ty::Char)
                    || is_str_like(&value.ty)
                    || is_printable_buffer(&value.ty);
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
///
/// A `List<T>` method is the same thing with the element type read off the
/// receiver (`list_sig`), and `expected` is consulted only for the two
/// associated constructors that cannot name their own element type:
/// `List.empty()` gets it from the binding's written type
/// (`val xs: List<String> = List.empty()`), and `List.of(x)` from `x` or
/// from `expected` when both are available.
/// `s.method(...)` where `s` is an `enum` (A-1): there are no methods on an
/// enum, and the message says what to write instead. Checked before the
/// method tables so the diagnostic is about the type, not about a missing
/// `datac 'Shape'`.
fn check_method_call_enum_guard(receiver: &ast::Expr, method: &str, span: Span, ctx: &mut Ctx) -> Result<()> {
    if let Some(ename) = enum_path(receiver, ctx) {
        return Err(TypeError {
            message: format!(
                "enum '{ename}' has no method '{method}' — an enum carries no methods in v1; match it with 'when'"
            ),
            span,
        });
    }
    // A value of enum type reached any other way (`val s = Shape.Empty` then
    // `s.to_text()`).
    if let ast::Expr::Ident(name, _) = receiver {
        if let Some(info) = ctx.lookup(name) {
            if let Some(tname) = struct_name_of(&info.ty) {
                if ctx.is_enum(tname) {
                    return Err(TypeError {
                        message: format!(
                            "enum '{tname}' has no method '{method}' — an enum carries no methods in v1; match it with 'when'"
                        ),
                        span,
                    });
                }
            }
        }
    }
    Ok(())
}

fn check_method_call(
    receiver: &ast::Expr,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Bytes.empty()` / `Bytes.from_str(s)` / `List.of(x)`: an *associated*
    // builtin, written on the type name, so it takes no receiver argument and
    // is resolved before the receiver is checked as a value (it isn't one). A
    // local or an `impl` alias of the same name shadows the type name,
    // exactly as it would anywhere else — which is why those cases fall
    // through to the ordinary method path below.
    if let ast::Expr::Ident(name, _) = receiver {
        if ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
            if name == "List" || name == "Box" || name == "Map" {
                return check_container_associated(name, method, args, span, ctx, expected);
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
    // `cfg.on_event(x)` where the receiver has a *field* of function type
    // (A-4): the call syntax is a method's, the callee is a field, and the
    // field is a place holding a function value. A declared method of the
    // same name wins — the declaration is the more specific statement of what
    // the type does — so this is only consulted when there is no such method.
    if ctx.sig_of(&format!("{owner}#{method}")).is_none() {
        if let Some(fn_ty) = fn_field_ty(&probe.ty, method, ctx) {
            // A field holding a closure is callable the same way (A-4 phase
            // E): the call goes through the pair the field holds.
            if matches!(fn_ty, Ty::Fn { .. } | Ty::Closure { .. }) {
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
    let container_elem = crate::types::list_elem_of(&probe.ty)
        .or_else(|| crate::types::box_elem_of(&probe.ty))
        .or_else(|| crate::types::map_elem_of(&probe.ty))
        .cloned();
    let container_key = crate::types::map_key_of(&probe.ty)
        .cloned()
        .unwrap_or(Ty::Unit);
    let (key, sig) = match ctx.sig_of(&key) {
        Some(s) => (key, s),
        // A-2: a method declared on a generic type (`fun Pair<A, B>.first()`).
        // Its type arguments are the receiver's own instantiation, which the
        // probe already resolved, so the template key is the *type template*
        // plus the method name — and the instance's key is what the call names.
        None => match method_template_key(ctx, &owner, method) {
            Some(tmpl_key) => {
                instantiate_function(ctx, &tmpl_key, &[], args, span, expected, Some(&probe.ty))?
            }
            None => match container_elem
                .as_ref()
                .and_then(|e| container_sig(&key, e, &container_key))
            {
                Some(s) => (key, s),
                None => match builtin_sig(&key) {
                    Some(s) => (key, s),
                    None => {
                        return Err(TypeError {
                            message: match (&container_elem, owner.as_str()) {
                // Three List methods are defined only for an element type that
                // can be ordered, compared, or copied, so the generic "no
                // method" text would be misleading for exactly these three.
                (Some(e), "List")
                    if matches!(e, Ty::Struct(_))
                        && matches!(method, "sort" | "contains" | "slice") =>
                {
                    format!(
                        "List<{}>.{method} is not defined: a 'datac' element has no {}, so it cannot be {} — use a scalar, 'String' or 'Bytes' element",
                        e.display(),
                        match method {
                            "sort" => "order",
                            "contains" => "equality",
                            _ => "copy",
                        },
                        match method {
                            "sort" => "sorted",
                            "contains" => "searched for",
                            _ => "copied into a new list",
                        }
                    )
                }
                (Some(e), "List") => format!(
                    "List<{}> has no method '{method}' — the List surface is len, is_empty, push, get, set, pop, clear, sort, reverse, contains, slice, insert, remove",
                    e.display()
                ),
                (Some(e), "Map") => format!(
                    "Map<{}> has no method '{method}' — the Map surface is len, is_empty, has, get, get_or, insert, remove, clear, keys, values",
                    e.display()
                ),
                (Some(e), _) => format!(
                    "Box<{}> has no method '{method}' — the Box surface is is_empty, get, set, take",
                    e.display()
                ),
                (None, _) if probe.ty.is_integer() => format!(
                    "type '{}' has no method '{method}' — an integer has to_i32, to_i64, to_u32, to_u64, to_u8, to_u16, to_usize, to_isize, to_f64 and to_f32, and no other type converts",
                    probe.ty.display()
                ),
                (None, _) => format!("datac '{owner}' has no method '{method}'"),
                            },
                            span,
                        });
                    }
                },
            },
        },
    };

    let mut checked_args = Vec::with_capacity(sig.params.len());
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

/// The associated constructors of the three builtin containers, written on
/// the type name: `List.of(x)`/`List.empty()`, `Box.of(x)`/`Box.empty()`,
/// and `Map.new()`/`Map.empty()`. Each needs its element type from
/// somewhere — `of` takes it from its argument (or from the binding's
/// written type, which is what `expected` is for), `empty`/`new` only from
/// the written type.
fn check_container_associated(
    container: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
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
    // `Map.empty()` and `Map.new()` are one operation — a map starts empty —
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
                return Err(TypeError {
                    message: "cannot tell what 'Map.new()' holds — write the type on the binding ('var m: Map<String, i32> = Map.new()'); there is no 'Map.of(x)' since a map starts empty".to_string(),
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![] },
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
                return Err(TypeError {
                    message: match container {
                        "Map" => "cannot tell what 'Map.empty()' holds — write the type on the binding ('var m: Map<String, i32> = Map.empty()')".to_string(),
                        _ => format!(
                            "cannot tell what '{container}.empty()' holds — write the element type on the binding ('var xs: {container}<String> = {container}.empty()') or start from '{container}.of(x)'"
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
            message: "'Map.of' does not exist — a map starts empty; write 'Map.new()' with the value type on the binding, then 'insert(k, v)'".to_string(),
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
                "'{container}.{other}' is not a {container} operation — {}",
                if container == "List" {
                    "write 'List.of(x)' for a one-element list, 'List.empty()' with the element type on the binding, or call a method on a value ('xs.push(x)', 'xs.get(i)', 'xs.len()')"
                } else if container == "Map" {
                    "a map starts empty — write 'Map.new()' with the type on the binding ('var m: Map<String, i32> = Map.new()'), then call a method on the value ('m.insert(k, v)', 'm.get(k)', 'm.len()')"
                } else {
                    "write 'Box.of(x)' for a one-value box, 'Box.empty()' with the element type on the binding, or call a method on a value ('b.get()', 'b.set(v)', 'b.take()', 'b.is_empty()')"
                }
            ),
            span,
        }),
    }
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
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let qualified = format!("{prefix}{item}");
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

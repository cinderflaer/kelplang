//! The freestanding mode's allowed set (kernel stage 4, MK9-S4-M1/M2).
//!
//! A project whose `coral.toml` carries `[target] mode = "freestanding"` is
//! built for a machine with no libc and no `kelp_rt` object behind it, so
//! everything that would reach that runtime is a compile error rather than an
//! unresolved symbol at link time. This pass is that rule, and it runs in the
//! front end, before either backend is named, so `kelpc check` enforces exactly
//! what `kelpc build` does.
//!
//! What is allowed is the language the kernel's own machinery needs, and every
//! entry below is a decision with a reason:
//!
//! - every integer width, `bool`, `char`, `f32`/`f64`, `unit`;
//! - `str`, as a borrowed literal in read-only data;
//! - user `datac` and `enum` values, arrays `[T; N]`, borrows `&T`/`&mut T`;
//! - `if`, `when`, loops, functions, and closures that capture nothing (a
//!   capturing literal owns a heap environment, which the runtime would have
//!   to free);
//! - `Atomic<T>` and its methods (`std/atomic`), `Option`/`Result`
//!   (`std/option`, `std/result`), which are pure Kelp;
//! - `Ptr<T>` and the compile-time queries `size_of`/`align_of`/`offset_of`;
//! - `extern fun` declared in a module with its own `[extern."<id>"]` section,
//!   so a project can bind a native symbol it will place itself;
//! - arithmetic, including the overflow and bounds checks: in this mode they
//!   call the project's own `panic = "..."` handler, or a generated infinite
//!   loop when the manifest names none.
//!
//! And what is refused, each naming the construct:
//!
//! - `print`/`print!` (a `Template`), which is `printf`;
//! - `String`, `Bytes`, `List`, `Map`, `Box`, `View` and `Thread`, whose
//!   storage is the runtime allocator's, and therefore any `std/*` module not
//!   on the list above;
//! - passdowns (`pd`), whose initializers run through the generated
//!   `kelp_init_pds` that only the hosted entry point calls.
//!
//! Undefined behaviour this mode deliberately does not remove: a bound or an
//! overflow that the checks do catch calls the project's own handler, whose
//! body the compiler does not check at all (it may never return, but the
//! compiler does not require that), and `Ptr<T>`'s `read`/`write` stay as
//! unchecked as they are hosted. `plan/kernel-ub-notes.md` carries the stage's
//! entry.

use std::collections::HashMap;

use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::resolve::Project;
use crate::tir::{Block, Expr, ExprKind, Program, Stmt};
use crate::types::Ty;

/// The `std/*` modules a freestanding program may reach: the ones that are pure
/// Kelp or a compiler builtin, with no libc call and no allocation behind them.
/// Every other shipped module is refused, by name, when it is part of the
/// program.
pub const ALLOWED_STD_MODULES: &[&str] = &["std/atomic", "std/option", "std/result"];

/// Enforces the allowed set for `project`'s freestanding mode. A hosted project
/// (the default, and what a manifest with no `[target]` section is) passes
/// straight through, so this is a no-op for every project written before this
/// stage.
pub fn check(program: &Program, project: &Project) -> Result<(), Diagnostic> {
    let target = project.target();
    if !target.is_freestanding() {
        return Ok(());
    }

    // The entry point is named by the manifest, so its shape is the compiler's
    // to check rather than infer's: it has to be a plain `fun` in the entry
    // module that takes nothing and returns nothing, which is what `_start` can
    // call.
    let entry_name = target.entry().unwrap_or_default();
    let Some(entry) = program
        .functions
        .iter()
        .find(|f| f.module == project.entry && f.name == entry_name && !f.extern_)
    else {
        return Err(Diagnostic::new(
            format!(
                "no entry function '{entry_name}' found: coral.toml's '[target] entry' names a `fun` with no parameters and no return in the entry module"
            ),
            Span::new(0, 0),
            project.entry.clone(),
        ));
    };
    if entry.env.is_some() || !entry.params.is_empty() || entry.ret != Ty::Unit {
        return Err(Diagnostic::new(
            format!(
                "the freestanding entry function '{entry_name}' must take no parameters and return nothing, because it is what '_start' calls"
            ),
            entry.span,
            entry.module.clone(),
        ));
    }

    // `panic = "<name>"` names the handler the compiler's own checks call. Its
    // shape is fixed (`fun(msg: str)`) because the call site passes exactly the
    // static message the check was written with.
    if let Some(panic_name) = target.panic() {
        let Some(handler) = program
            .functions
            .iter()
            .find(|f| f.module == project.entry && f.name == panic_name && !f.extern_)
        else {
            return Err(Diagnostic::new(
                format!(
                    "the freestanding panic handler '{panic_name}' named by coral.toml's '[target] panic' is not declared in the entry module"
                ),
                Span::new(0, 0),
                project.entry.clone(),
            ));
        };
        let shape_ok = handler.env.is_none()
            && handler.params.len() == 1
            && handler.params[0].1 == Ty::Str
            && handler.ret == Ty::Unit;
        if !shape_ok {
            return Err(Diagnostic::new(
                format!(
                    "the freestanding panic handler '{panic_name}' must be a 'fun(msg: str)' with no return, which is the shape the compiler's checks call"
                ),
                handler.span,
                handler.module.clone(),
            ));
        }
    }

    // A passdown's initializer is a module-scope global the generated
    // `kelp_init_pds` fills in, and only the hosted `main` calls that. A
    // freestanding program therefore has no place for one.
    if let Some(pd) = program.pds.first() {
        return Err(unavailable(
            format!(
                "the passdown '{}' is unavailable in a freestanding build: its initializer runs through the generated 'kelp_init_pds', which only the hosted entry point calls",
                pd.qualified
            ),
            Span::new(0, 0),
            &pd.module,
        ));
    }

    // The hosted entry point saves `argc`/`argv` into the runtime's globals and
    // returns an `int` the C runtime reads. A freestanding image has neither, so
    // a defined `main` is refused by name; the manifest's `[target] entry` is
    // the entry point instead.
    if let Some(main) = program
        .functions
        .iter()
        .find(|f| f.name == "main" && !f.extern_)
    {
        return Err(unavailable(
            "'main' is unavailable in a freestanding build: there is no C runtime to call it, so the manifest's '[target] entry' names the entry function instead"
                .to_string(),
            main.span,
            main.module.clone(),
        ));
    }

    let mut module_of: HashMap<&str, &str> = HashMap::new();
    for f in &program.functions {
        module_of.entry(f.name.as_str()).or_insert(f.module.as_str());
    }
    let checker = Checker { module_of };

    // The program's own declarations first, so a construct in the program is
    // reported by its own name rather than behind the std module it reaches
    // through.
    for s in &program.structs {
        for f in &s.fields {
            if let Some(name) = heap_type(&f.ty) {
                return Err(unavailable(
                    format!(
                        "the field '{}.{}' has type '{name}', which is unavailable in a freestanding build",
                        s.name, f.name
                    ),
                    Span::new(0, 0),
                    "",
                ));
            }
        }
    }
    for e in &program.enums {
        for v in &e.variants {
            for f in &v.fields {
                if let Some(name) = heap_type(&f.ty) {
                    return Err(unavailable(
                        format!(
                            "the payload '{}.{}.{}' has type '{name}', which is unavailable in a freestanding build",
                            e.name, v.name, f.name
                        ),
                        Span::new(0, 0),
                        "",
                    ));
                }
            }
        }
    }
    for f in &program.functions {
        if f.module.starts_with("std/") {
            continue;
        }
        for (name, ty) in &f.params {
            if let Some(heap) = heap_type(ty) {
                return Err(unavailable(
                    format!(
                        "the parameter '{name}' of '{}' has type '{heap}', which is unavailable in a freestanding build",
                        f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        if let Some(heap) = heap_type(&f.ret) {
            return Err(unavailable(
                format!(
                    "'{}' returns '{heap}', which is unavailable in a freestanding build",
                    f.name
                ),
                f.span,
                f.module.clone(),
            ));
        }
        checker.block(&f.body, &f.module)?;
    }

    // Finally, every `std/*` module the program reaches has to be one of the
    // pure three. A module that was imported but never called is still part of
    // the program, so this is where an unused `std/str` import is caught.
    for m in &project.modules {
        if m.id.starts_with("std/") && !ALLOWED_STD_MODULES.contains(&m.id.as_str()) {
            return Err(unavailable(
                format!(
                    "the module '{}' is unavailable in a freestanding build: it reaches the hosted runtime (libc or the 'kelp_rt' helpers)",
                    m.id
                ),
                Span::new(0, 0),
                m.id.clone(),
            ));
        }
    }
    Ok(())
}

/// The compiler builtin a freestanding program may not call, and why, or
/// `None`. Each of these is a direct call into the hosted runtime that no
/// module lookup would catch, because the name is a builtin rather than a
/// declaration in a shipped module.
fn unavailable_builtin(name: &str) -> Option<&'static str> {
    match name {
        "print" | "print!" => Some("it writes through the hosted runtime's 'printf'"),
        // The builtin `panic` is *not* here: codegen lowers it to a call to the
        // same handler the bounds and overflow checks use, so it is allowed and
        // routed (see `Codegen::panic_fn`).
        "assert" => Some("it calls the hosted runtime's 'kelp_assert'"),
        // `Ptr<T>` is allowed; the allocator behind `Ptr.alloc`/`Ptr.free` is
        // not, because it is the hosted `malloc`/`free`. A kernel's own frame
        // allocator arrives in a later stage (MK9-S5).
        "Ptr#alloc" => Some("it allocates through the hosted runtime's 'malloc'"),
        "Ptr#free" => Some("it frees through the hosted runtime's 'free'"),
        _ => None,
    }
}

/// The one diagnostic shape this pass emits, so every refusal names the mode
/// and reads the same way.
fn unavailable(message: String, span: Span, module: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, span, module)
}

/// The name of the heap-backed construct `ty` needs, if it needs one, or
/// `None` for every type a freestanding program may hold. A user `datac` is
/// reported through its own fields (the sweep in `check`), so it is `None`
/// here.
fn heap_type(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::String => Some("String"),
        Ty::Bytes => Some("Bytes"),
        Ty::View => Some("View"),
        Ty::List(_) => Some("List"),
        Ty::Map(_, _) => Some("Map"),
        Ty::Box(_) => Some("Box"),
        Ty::Thread(_) => Some("Thread"),
        Ty::Ref { inner, .. } | Ty::Atomic(inner) | Ty::Ptr(inner) => heap_type(inner),
        Ty::Array(inner, _) => heap_type(inner),
        Ty::Fn { params, ret } | Ty::Closure { params, ret } => params
            .iter()
            .chain(std::iter::once(&**ret))
            .find_map(heap_type),
        _ => None,
    }
}

struct Checker<'a> {
    module_of: HashMap<&'a str, &'a str>,
}

impl Checker<'_> {
    fn block(&self, block: &Block, module: &str) -> Result<(), Diagnostic> {
        for stmt in &block.stmts {
            self.stmt(stmt, module)?;
        }
        if let Some(tail) = &block.tail {
            self.expr(tail, module)?;
        }
        Ok(())
    }

    fn stmt(&self, stmt: &Stmt, module: &str) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { name, ty, init } => {
                if let Some(heap) = heap_type(ty) {
                    return Err(unavailable(
                        format!(
                            "the binding '{name}' has type '{heap}', which is unavailable in a freestanding build"
                        ),
                        init.span,
                        module,
                    ));
                }
                self.expr(init, module)
            }
            Stmt::While { cond, body } => {
                self.expr(cond, module)?;
                self.block(body, module)
            }
            Stmt::Return { value, .. } => match value {
                Some(value) => self.expr(value, module),
                None => Ok(()),
            },
            Stmt::Expr(expr) => self.expr(expr, module),
        }
    }

    fn expr(&self, expr: &Expr, module: &str) -> Result<(), Diagnostic> {
        // Every expression carries its own type, so the heap check is one
        // question per node rather than one per expression form.
        if let Some(heap) = heap_type(&expr.ty) {
            return Err(unavailable(
                format!("the expression has type '{heap}', which is unavailable in a freestanding build"),
                expr.span,
                module,
            ));
        }
        match &expr.kind {
            ExprKind::Template { .. } => Err(unavailable(
                "print is unavailable in a freestanding build: there is no 'printf' to write to"
                    .to_string(),
                expr.span,
                module,
            )),
            ExprKind::Call { callee, args } => {
                if let Some(reason) = unavailable_builtin(callee) {
                    return Err(unavailable(
                        format!("'{callee}' is unavailable in a freestanding build: {reason}"),
                        expr.span,
                        module,
                    ));
                }
                if let Some(callee_module) = self.module_of.get(callee.as_str()) {
                    if callee_module.starts_with("std/")
                        && !ALLOWED_STD_MODULES.contains(callee_module)
                    {
                        return Err(unavailable(
                            format!(
                                "the call to '{callee}' is unavailable in a freestanding build: the module '{callee_module}' reaches the hosted runtime"
                            ),
                            expr.span,
                            module,
                        ));
                    }
                }
                for arg in args {
                    self.expr(arg, module)?;
                }
                Ok(())
            }
            ExprKind::Unary { expr, .. } | ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } => {
                self.expr(expr, module)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs, module)?;
                self.expr(rhs, module)
            }
            ExprKind::FieldAccess { base, .. } => self.expr(base, module),
            ExprKind::StructLit { fields, .. } => {
                for (_, value) in fields {
                    self.expr(value, module)?;
                }
                Ok(())
            }
            ExprKind::EnumLit { fields, .. } => {
                for value in fields {
                    self.expr(value, module)?;
                }
                Ok(())
            }
            ExprKind::EnumTag { expr, .. } => self.expr(expr, module),
            ExprKind::EnumField { base, .. } => self.expr(base, module),
            ExprKind::Try { expr, .. } => self.expr(expr, module),
            ExprKind::Assign { target, value } => {
                self.expr(target, module)?;
                self.expr(value, module)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond, module)?;
                self.block(then_block, module)?;
                match else_block {
                    Some(block) => self.block(block, module),
                    None => Ok(()),
                }
            }
            ExprKind::Block(block) => self.block(block, module),
            ExprKind::ClosureLit { captures, .. } => {
                for capture in captures {
                    self.expr(capture, module)?;
                }
                Ok(())
            }
            ExprKind::FnToClosure { func } => self.expr(func, module),
            ExprKind::IndirectCall { callee, args } => {
                self.expr(callee, module)?;
                for arg in args {
                    self.expr(arg, module)?;
                }
                Ok(())
            }
            ExprKind::Template { .. } => unreachable!("handled above"),
            ExprKind::IntLit(_)
            | ExprKind::CharLit(_)
            | ExprKind::FloatLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Ident(_)
            | ExprKind::FnRef(_)
            | ExprKind::Capture { .. } => Ok(()),
            ExprKind::ArrayLit(values) => {
                for value in values {
                    self.expr(value, module)?;
                }
                Ok(())
            }
            ExprKind::ArrayRepeat { value } => self.expr(value, module),
        }
    }
}

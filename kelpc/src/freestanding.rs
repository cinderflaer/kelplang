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
//!   (`std/option`, `std/result`), which are pure Kelp, and `std/arena`,
//!   `std/irq` and `std/frame`, which are the same;
//! - the `str` operations the compiler lowers as byte loops with no libc call:
//!   `std/str`'s `len`, `byte`, `eq`, `starts_with` and `ptr`, and nothing else
//!   in that module; the `View` operations the shipped modules reach (`len`,
//!   `is_empty`, `get`, `slice`, `eq`), and `View.to_string` once an allocator
//!   is configured, the same way;
//! - `std/math`'s integer `abs`, `min`, `max` and `clamp`, which are plain
//!   Kelp comparisons, and `std/list`, whose callback combinators go through
//!   the project allocator with the rest of the heap types;
//! - with an allocator, the `std/option`/`std/result` callback combinators,
//!   `str.concat` and the integer builders, whose closure environments and
//!   owned buffers are the project allocator's;
//! - `Ptr<T>` and the compile-time queries `size_of`/`align_of`/`offset_of`;
//! - `Box<T>`, `List<T>` and `Map<V>` of allowed element types once `[target]
//!   alloc`/`dealloc` name the project's own allocator (kernel stage 7,
//!   MK9-S5-M1), because then every allocation and free those containers make
//!   calls the project's functions instead of libc's;
//! - `String` and `Bytes` with the same allocator (kernel stage 12,
//!   MK9-S5-M2), narrowed to the operations the LLVM backend lowers as libc-free
//!   byte loops (`ALLOWED_STRING_METHODS`/`ALLOWED_BYTES_METHODS`, plus
//!   `str.concat` and the integer builders); every other method is refused at
//!   the call with "needs the hosted runtime";
//! - `extern fun` declared in a module with its own `[extern."<id>"]` section,
//!   so a project can bind a native symbol it will place itself;
//! - arithmetic, including the overflow and bounds checks: in this mode they
//!   call the project's own `panic = "..."` handler, or a generated infinite
//!   loop when the manifest names none.
//!
//! And what is refused, each naming the construct:
//!
//! - `print`/`print!` (a `Template`), which is `printf`;
//! - `Thread`, whose storage is the hosted C runtime's, so it stays refused
//!   whatever the manifest says; `String`/`Bytes` without an allocator,
//!   and `List`, `Map` and `Box` when no `[target] alloc`/`dealloc` pair is
//!   configured, since their storage is the runtime allocator's; and therefore
//!   any `std/*` module not admitted by `std_admission` below, and every
//!   function of an admitted module that is not on its row (`kelp wiki page language/freestanding.md`);
//! - passdowns (`pd`), whose initializers run through the generated
//!   `kelp_init_pds` that only the hosted entry point calls.
//!
//! Undefined behaviour this mode deliberately does not remove: a bound or an
//! overflow that the checks do catch calls the project's own handler, whose
//! body the compiler does not check at all (it may never return, but the
//! compiler does not require that), and `Ptr<T>`'s `read`/`write` stay as
//! unchecked as they are hosted. `kelp wiki page language/unsafe-ub-catalogue.md` carries the stage's
//! entry.

use std::collections::HashMap;

use crate::ast;
use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::resolve::Project;
use crate::tir::{Block, Expr, ExprKind, Program, Stmt};
use crate::types::Ty;

mod tables;
pub(crate) use tables::*;

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
    // shape is one of two: `fun(msg: str)` or the location-carrying
    // `fun(msg: str, file: str, line: u32)` (kernel stage 15).
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
        // Two shapes are accepted: the original `fun(msg: str)` and the
        // three-argument form that also gets the location. The call site the
        // entry emits passes exactly these.
        let one_arg = handler.params.len() == 1 && handler.params[0].1 == Ty::Str;
        let three_arg = handler.params.len() == 3
            && handler.params[0].1 == Ty::Str
            && handler.params[1].1 == Ty::Str
            && handler.params[2].1 == Ty::U32;
        if handler.env.is_some() || !(one_arg || three_arg) || handler.ret != Ty::Unit {
            return Err(Diagnostic::new(
                format!(
                    "the freestanding panic handler '{panic_name}' must be a 'fun(msg: str)' or a 'fun(msg: str, file: str, line: u32)', with no return, which are the shapes the compiler's checks call"
                ),
                handler.span,
                handler.module.clone(),
            ));
        }
    }

    // The allocator hook (kernel stage 7): `alloc` and `dealloc` name the
    // entry module's functions, and their shapes are what the generated calls
    // assume. `realloc`, when named, is the growth fast path.
    if let (Some(alloc_name), Some(dealloc_name)) = (target.alloc(), target.dealloc()) {
        let Some(alloc) = find_entry_fun(program, project, alloc_name) else {
            return Err(Diagnostic::new(
                format!(
                    "the allocator function '{alloc_name}' named by coral.toml's '[target] alloc' is not declared in the entry module"
                ),
                Span::new(0, 0),
                project.entry.clone(),
            ));
        };
        check_allocator_signature(alloc, alloc_name, &[Ty::Usize, Ty::Usize], true, project)?;

        let Some(dealloc) = find_entry_fun(program, project, dealloc_name) else {
            return Err(Diagnostic::new(
                format!(
                    "the allocator function '{dealloc_name}' named by coral.toml's '[target] dealloc' is not declared in the entry module"
                ),
                Span::new(0, 0),
                project.entry.clone(),
            ));
        };
        check_allocator_signature(
            dealloc,
            dealloc_name,
            &[Ty::Ptr(Box::new(Ty::U8)), Ty::Usize, Ty::Usize],
            false,
            project,
        )?;

        if let Some(realloc_name) = target.realloc() {
            let Some(realloc) = find_entry_fun(program, project, realloc_name) else {
                return Err(Diagnostic::new(
                    format!(
                        "the allocator function '{realloc_name}' named by coral.toml's '[target] realloc' is not declared in the entry module"
                    ),
                    Span::new(0, 0),
                    project.entry.clone(),
                ));
            };
            check_allocator_signature(
                realloc,
                realloc_name,
                &[
                    Ty::Ptr(Box::new(Ty::U8)),
                    Ty::Usize,
                    Ty::Usize,
                    Ty::Usize,
                ],
                true,
                project,
            )?;
        }
    }

    let heap_ok = target.alloc().is_some() && target.dealloc().is_some();

    // Every `std/*` module the program reaches has to be one the table admits,
    // or one whose items are all admitted, so the project is refused by name
    // here, before the construct checks below (a project that imports `std/fs`
    // hears about `std/fs` rather than about a passdown inside `std/io`). A
    // module that was imported but never called is still part of the program,
    // so an unused `std/fmt` import is caught here. `std/list` is the one
    // module whose *presence* needs the allocator: every body it has takes a
    // closure over a `List<T>`.
    if let Some(id) = first_refused_module(project, heap_ok) {
        let reason = if matches!(std_admission(id), StdAdmission::Hosted) {
            format!(
                "it needs a hosted runtime (libc or the 'kelp_rt' helpers); the std modules a freestanding project may implement are {}",
                allowed_modules()
            )
        } else {
            "it needs an allocator: set '[target] alloc' and '[target] dealloc' to the project's own functions".to_string()
        };
        return Err(unavailable(
            format!("the module '{id}' is unavailable in a freestanding build: {reason}"),
            Span::new(0, 0),
            id.to_string(),
        ));
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
    let checker = Checker {
        module_of,
        heap_ok,
        caps: project.caps(),
    };

    // The program's own declarations first, so a construct in the program is
    // reported by its own name rather than behind the std module it reaches
    // through.
    for s in &program.structs {
        for f in &s.fields {
            if let Some(name) = heap_refusal(&f.ty, heap_ok) {
                return Err(unavailable(
                    format!(
                        "the field '{}.{}' has type {}",
                        s.name,
                        f.name,
                        heap_message(name, heap_ok)
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
                if let Some(name) = heap_refusal(&f.ty, heap_ok) {
                    return Err(unavailable(
                        format!(
                            "the payload '{}.{}.{}' has type {}",
                            e.name,
                            v.name,
                            f.name,
                            heap_message(name, heap_ok)
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
            if let Some(heap) = heap_refusal(ty, heap_ok) {
                return Err(unavailable(
                    format!(
                        "the parameter '{name}' of '{}' has type {}",
                        f.name,
                        heap_message(heap, heap_ok)
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        if let Some(heap) = heap_refusal(&f.ret, heap_ok) {
            return Err(unavailable(
                format!(
                    "'{}' returns {}",
                    f.name,
                    heap_message(heap, heap_ok)
                ),
                f.span,
                f.module.clone(),
            ));
        }
        checker.block(&f.body, &f.module)?;
    }

    Ok(())
}

/// Drops the `std/*` declarations a freestanding program does not need, so the
/// `std/str` and `std/mem` bodies that reach `strlen`/`malloc` are never in the
/// program at all. The allowed set above is what a program may *call*; this is
/// the emission half of the same rule, and it runs on the merged AST between
/// the check and the build's own inference.
///
/// It has to run *before* inference rather than after, because inference is
/// what turns a mention of a type into a concrete instance: `std/str.parse_f64`
/// alone is enough to instantiate `Result<f64, ParseErr>` and its generated
/// comparator, and that comparator is emitted whether or not anything calls it
/// (on a target without hardware floats it reaches the soft-float helpers, which
/// no bare-metal image has). Dropping the declaration means the instance never
/// exists. A hosted build compiles the whole module and links the runtime that
/// defines the symbols, so it is untouched.
///
/// Every function the program itself declares is kept, and so is every body
/// the table admits for a `std` module, so the only thing that disappears is
/// dead code in a present or narrowed module. For a `Subset` row that means the
/// named items, the private helpers they reach and, when an allocator is
/// configured, the allocator-only items; a kept body can reach nothing else.
pub fn prune(program: &mut ast::Program, allocator: bool) {
    program.functions.retain(|f| {
        if !f.module.starts_with("std/") {
            return true;
        }
        match std_admission(&f.module) {
            StdAdmission::Pure | StdAdmission::Allocator => true,
            StdAdmission::Present | StdAdmission::Hosted => false,
            StdAdmission::Subset { .. } => {
                // An `extern fun` is a declaration the module's own bodies may
                // call; a pruned body leaves it unused, and an unused
                // declaration is no symbol.
                if f.extern_ {
                    return true;
                }
                // The AST keeps one declaration per overload under the same
                // name (`std/math` has three `abs`s), so a `f32` or `f64`
                // overload is told apart by its parameters rather than its key;
                // a float comparison on the kernel's riscv64 triple is a
                // soft-float libcall (`__ltdf2`, `__ltsf2`), so none of them is
                // emitted.
                if f.module == "std/math" && f.params.iter().any(|p| is_float_type(&p.ty.display())) {
                    return false;
                }
                item_is_callable(&f.module, item_name(&f.name), allocator)
            }
        }
    });
}

/// The items a `Subset` module admits right now, for the refusal that names
/// what would work instead. The overload suffixes are dropped (the reader
/// cares about `abs`, not `abs@i32`) and the names sorted, so the message
/// cannot shuffle between runs.
fn admitted_items(module: &str, allocator: bool) -> String {
    let StdAdmission::Subset {
        callable,
        with_allocator,
        ..
    } = std_admission(module)
    else {
        return String::new();
    };
    let mut items: Vec<&str> = callable
        .iter()
        .chain(with_allocator.iter().filter(|_| allocator))
        .map(|item| item.split('@').next().unwrap_or(item))
        .collect();
    items.sort_unstable();
    items.dedup();
    items.join(", ")
}

/// The last dot-separated component of a function's key with any generic
/// instantiation removed: how a call key names an item when a declaration key
/// is `std.option.is_some` and the call key is `std.option.is_some<u32>`. An
/// overload suffix (`std.math.abs@i32`) is *kept*, because the two overloads
/// are different declarations with different bodies.
fn item_name(name: &str) -> &str {
    let item = name.rsplit('.').next().unwrap_or(name);
    item.split('<').next().unwrap_or(item)
}

/// The module a refusal should name: the first `std/*` module in the program
/// the table does not admit, preferring one that no other present `std` module
/// imports. Resolution keeps no import graph, so the edges are read from the
/// shipped text, which is where an `impl "std/..."` line lives; that is enough
/// to name `std/fs`/`std/net`/`std/tls` rather than a module inside them.
fn first_refused_module(project: &Project, heap_ok: bool) -> Option<&str> {
    let refused = |id: &str| match std_admission(id) {
        StdAdmission::Hosted => true,
        StdAdmission::Allocator => !heap_ok,
        _ => false,
    };
    let present: Vec<&str> = project
        .modules
        .iter()
        .map(|m| m.id.as_str())
        .filter(|id| id.starts_with("std/"))
        .collect();
    let mut fallback: Option<&str> = None;
    for id in &present {
        if !refused(id) {
            continue;
        }
        fallback.get_or_insert(id);
        let imported_by_another = present.iter().any(|other| {
            other != id
                && crate::stdlib::source(other)
                    .map(|text| text.contains(&format!("\"{id}\"")))
                    .unwrap_or(false)
        });
        if !imported_by_another {
            return Some(id);
        }
    }
    fallback
}

/// The compiler builtin a freestanding program may not call, and why, or
/// `None`. Each of these is a direct call into the hosted runtime that no
/// module lookup would catch, because the name is a builtin rather than a
/// declaration in a shipped module.
fn unavailable_builtin(name: &str, heap_ok: bool) -> Option<&'static str> {
    match name {
        "print" | "print!" => Some("it writes through the hosted runtime's 'printf'"),
        // The builtin `panic` is *not* here: codegen lowers it to a call to the
        // same handler the bounds and overflow checks use, so it is allowed and
        // routed (see `Codegen::panic_fn`).
        "assert" => Some("it calls the hosted runtime's 'kelp_assert'"),
        // `Ptr<T>` is allowed; the allocator behind `Ptr.alloc`/`Ptr.free` is
        // not, because it is the hosted `malloc`/`free`.
        "Ptr#alloc" => Some("it allocates through the hosted runtime's 'malloc'"),
        "Ptr#free" => Some("it frees through the hosted runtime's 'free'"),
        // `Ptr.try_alloc` (kernel stage 13) lowers to the project's own
        // allocator, so it is exactly as available as `Box`/`List`/`Map` are:
        // only with the manifest's `[target] alloc`/`dealloc` pair.
        "Ptr#try_alloc" if !heap_ok => Some(
            "it needs an allocator: set '[target] alloc' and '[target] dealloc' to the project's own functions",
        ),
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
///
/// `heap_ok` is whether the manifest configured an `alloc`/`dealloc` pair: with
/// one, `Box`, `List`, `Map`, `String` and `Bytes` are allowed and the walk
/// descends into their element types, because a `List<String>` still reaches the
/// hosted runtime. `View` is a borrowed two-word value with no allocation, and
/// every operation on it the shipped modules reach is a byte loop the backend
/// emits (`emit_freestanding_str_bodies`), so it is allowed with or without the
/// pair; `Thread` stays refused whatever `heap_ok` is.
fn heap_refusal(ty: &Ty, heap_ok: bool) -> Option<&'static str> {
    match ty {
        Ty::String => {
            if heap_ok {
                None
            } else {
                Some("String")
            }
        }
        Ty::Bytes => {
            if heap_ok {
                None
            } else {
                Some("Bytes")
            }
        }
        Ty::View => None,
        Ty::Thread(_) => Some("Thread"),
        Ty::Mutex(inner) => {
            let _ = inner;
            Some("Mutex")
        }
        Ty::RwLock(inner) => {
            let _ = inner;
            Some("RwLock")
        }
        Ty::Condvar => Some("Condvar"),
        Ty::List(inner) => {
            if heap_ok {
                heap_refusal(inner, true)
            } else {
                Some("List")
            }
        }
        Ty::Map(key, value) => {
            if heap_ok {
                heap_refusal(key, true).or_else(|| heap_refusal(value, true))
            } else {
                Some("Map")
            }
        }
        Ty::Box(inner) => {
            if heap_ok {
                heap_refusal(inner, true)
            } else {
                Some("Box")
            }
        }
        Ty::Ref { inner, .. } | Ty::Atomic(inner) | Ty::Ptr(inner) => heap_refusal(inner, heap_ok),
        Ty::Array(inner, _) => heap_refusal(inner, heap_ok),
        Ty::Fn { params, ret } | Ty::Closure { params, ret } => params
            .iter()
            .chain(std::iter::once(&**ret))
            .find_map(|t| heap_refusal(t, heap_ok)),
        _ => None,
    }
}

/// The tail of every heap-type refusal message: a type without an allocator
/// names the two manifest keys that would let it through, and every other type
/// stays a plain "unavailable".
fn heap_message(kind: &'static str, heap_ok: bool) -> String {
    if kind == "Mutex" {
        return "'Mutex', whose runtime half is a pthread mutex: a freestanding program has no threads and no libc, so a kernel would use a spin lock over an `Atomic<T>` instead".to_string();
    }
    if kind == "RwLock" {
        return "'RwLock', whose runtime half is pthread condition variables: a freestanding program has no threads and no libc, so a kernel would use a spin lock over an `Atomic<T>` instead".to_string();
    }
    if kind == "Condvar" {
        return "'Condvar', whose runtime half is a pthread condition variable: a freestanding program has no threads and no libc, so a kernel would use a wait queue over an `Atomic<T>` instead".to_string();
    }
    if !heap_ok && matches!(kind, "Box" | "List" | "Map") {
        format!(
            "'{kind}', which needs an allocator in a freestanding build: set '[target] alloc' and '[target] dealloc' to the project's own functions"
        )
    } else if !heap_ok && matches!(kind, "String" | "Bytes") {
        format!(
            "'{kind}', which is unavailable in a freestanding build without an allocator: set '[target] alloc' and '[target] dealloc' to the project's own functions"
        )
    } else {
        format!("'{kind}', which is unavailable in a freestanding build")
    }
}

/// The entry module's own declaration of `name`, if it has one.
fn find_entry_fun<'a>(
    program: &'a Program,
    project: &Project,
    name: &str,
) -> Option<&'a crate::tir::Function> {
    program
        .functions
        .iter()
        .find(|f| f.module == project.entry && f.name == name && !f.extern_)
}

/// One allocator function's shape: `params` is the exact parameter list the
/// generated call has, and `returns_ptr` says whether it returns `Ptr<u8>`
/// (every function but `dealloc` does). The check is a refusal, not a cast, so
/// a wrong signature is a compile error naming the expected one.
fn check_allocator_signature(
    f: &crate::tir::Function,
    name: &str,
    params: &[Ty],
    returns_ptr: bool,
    project: &Project,
) -> Result<(), Diagnostic> {
    let expected = params
        .iter()
        .map(|p| p.display())
        .collect::<Vec<_>>()
        .join(", ");
    let expected_ret = if returns_ptr { " => Ptr<u8>" } else { "" };
    let shape_ok = f.env.is_none()
        && f.params.len() == params.len()
        && f.params.iter().zip(params).all(|((_, ty), want)| ty == want)
        && f.ret == if returns_ptr { Ty::Ptr(Box::new(Ty::U8)) } else { Ty::Unit };
    if shape_ok {
        return Ok(());
    }
    Err(Diagnostic::new(
        format!(
            "the allocator function '{name}' named by coral.toml's '[target]' has the wrong signature: it must be 'fun {name}({expected}){expected_ret}'"
        ),
        f.span,
        project.entry.clone(),
    ))
}

pub(crate) struct Checker<'a> {
    pub(crate) module_of: HashMap<&'a str, &'a str>,
    /// Whether the manifest configured an `alloc`/`dealloc` pair, which is what
    /// lets `Box`, `List` and `Map` through.
    pub(crate) heap_ok: bool,
    /// The whole capability table (module -> domain -> patterns), so the
    /// `mmio` gate can answer what addresses a module was granted at the call
    /// that builds a pointer (`project.caps()`).
    pub(crate) caps: &'a HashMap<String, HashMap<String, Vec<String>>>,
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
                if let Some(heap) = heap_refusal(ty, self.heap_ok) {
                    return Err(unavailable(
                        format!(
                            "the binding '{name}' has type {}",
                            heap_message(heap, self.heap_ok)
                        ),
                        init.span,
                        module,
                    ));
                }
                self.expr(init, module)
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond, module)?;
                self.block(body, module)
            }
            Stmt::Return { value, .. } => match value {
                Some(value) => self.expr(value, module),
                None => Ok(()),
            },
            Stmt::Break { .. } | Stmt::Continue { .. } => Ok(()),
            Stmt::Expr(expr) => self.expr(expr, module),
        }
    }

    fn expr(&self, expr: &Expr, module: &str) -> Result<(), Diagnostic> {
        // Every expression carries its own type, so the heap check is one
        // question per node rather than one per expression form.
        if let Some(heap) = heap_refusal(&expr.ty, self.heap_ok) {
            return Err(unavailable(
                format!(
                    "the expression has type {}",
                    heap_message(heap, self.heap_ok)
                ),
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
                if callee == "Ptr#from_addr" {
                    self.check_mmio(args, module, expr.span)?;
                }
                if let Some(reason) = unavailable_builtin(callee, self.heap_ok) {
                    return Err(unavailable(
                        format!("'{callee}' is unavailable in a freestanding build: {reason}"),
                        expr.span,
                        module,
                    ));
                }
                // A `String`/`Bytes` method not on the libc-free list needs the
                // hosted runtime's storage (kernel stage 12). The type itself
                // is legal with an allocator, so without this check an
                // unsupported method would reach the linker as an undefined
                // `kelp_rt` symbol; naming it here is the whole point.
                if let Some((owner, method)) = callee.split_once('#') {
                    let allowed = match owner {
                        "String" => ALLOWED_STRING_METHODS.contains(&method),
                        "Bytes" => ALLOWED_BYTES_METHODS.contains(&method),
                        "View" => {
                            ALLOWED_VIEW_METHODS.contains(&method)
                                || (self.heap_ok && ALLOWED_VIEW_WITH_ALLOCATOR.contains(&method))
                        }
                        _ => true,
                    };
                    if !allowed {
                        return Err(unavailable(
                            format!(
                                "the call to '{callee}' is unavailable in a freestanding build: it needs the hosted runtime"
                            ),
                            expr.span,
                            module,
                        ));
                    }
                }
    // Finally, a call into a module that is present but not wholly admissible
    // is checked against that module's row. A `str` operation outside the five,
    // a `std/math` libm function, and the `std/option`/`std/result` callback
    // combinators without an allocator are each refused by name here, rather
    // than left to a pruned body or an undefined symbol at the link.
    if let Some(callee_module) = self.module_of.get(callee.as_str()) {
        if callee_module.starts_with("std/") {
            let item = item_name(callee);
            if !item_is_callable(callee_module, item, self.heap_ok) {
                let reason = if !self.heap_ok && item_needs_allocator(callee_module, item) {
                    "it needs an allocator: set '[target] alloc' and '[target] dealloc' to the project's own functions".to_string()
                } else if matches!(
                    std_admission(callee_module),
                    StdAdmission::Present | StdAdmission::Hosted
                ) {
                    format!("the module '{callee_module}' needs a hosted runtime")
                } else {
                    format!(
                        "it needs the hosted runtime, which '{callee_module}' reaches for everything but {}",
                        admitted_items(callee_module, self.heap_ok)
                    )
                };
                return Err(unavailable(
                    format!(
                        "the call to '{}' is unavailable in a freestanding build: {reason}",
                        // The overload's parameter types are part of its key,
                        // not of the name a reader wrote.
                        callee.split('@').next().unwrap_or(callee)
                    ),
                    expr.span,
                    module,
                ));
            }
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
            ExprKind::Region { .. } => Err(unavailable(
                "a block region is unavailable in a freestanding build (slice 1 is hosted only)".to_string(),
                Span::new(0, 0),
                module,
            )),
            ExprKind::ClosureLit { captures, .. } => {
                for capture in captures {
                    self.expr(capture, module)?;
                }
                Ok(())
            }
            // A scoped literal (`kelp wiki page language/functions.md`) keeps its captures in a stack
            // environment with a no-op drop, so unlike an owning closure it needs nothing from the
            // heap; its captures are checked like a closure's.
            ExprKind::ScopedClosureLit { captures, .. } => {
                for capture in captures {
                    self.expr(capture, module)?;
                }
                Ok(())
            }
            // A variadic extern call goes through a generated fixed-arity C wrapper, which lives in
            // the per-program C object a freestanding build deliberately does not have.
            ExprKind::VariadicCall { callee, .. } => Err(unavailable(
                format!(
                    "the variadic call to '{callee}' is unavailable in a freestanding build: it is made through a generated C wrapper, and there is no per-program C object"
                ),
                expr.span,
                module,
            )),
            ExprKind::FnToClosure { func } => self.expr(func, module),
            ExprKind::IndirectCall { callee, args } => {
                self.expr(callee, module)?;
                for arg in args {
                    self.expr(arg, module)?;
                }
                Ok(())
            }
            // Borrowed trait objects (A-12 slice 4) are legal in a freestanding
            // build: the vtable is `.rodata`, the thunk is ordinary code and no
            // runtime helper is reached. The borrow inside a coercion and a
            // vtable call's receiver and arguments are checked as usual.
            ExprKind::DynCoerce { expr: inner, .. }
            | ExprKind::BoxDynCoerce { expr: inner, .. }
            | ExprKind::BoxDynBorrow { expr: inner, .. }
            | ExprKind::BoxDynIsEmpty { expr: inner } => self.expr(inner, module),
            ExprKind::DynCall { receiver, args, .. } => {
                self.expr(receiver, module)?;
                for arg in args {
                    self.expr(arg, module)?;
                }
                Ok(())
            }
            ExprKind::IntLit(_)
            | ExprKind::BlockBox { .. }
            | ExprKind::CharLit(_)
            | ExprKind::FloatLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Ident(_)
            | ExprKind::StaticRef(_)
            | ExprKind::FnRef(_)
            | ExprKind::Capture { .. } => Ok(()),
            ExprKind::ArrayLit(values) => {
                for value in values {
                    self.expr(value, module)?;
                }
                Ok(())
            }
            ExprKind::ArrayRepeat { value } => self.expr(value, module),
            // Inline asm (kernel stage 5) is allowed: it is the reason this mode
            // exists, and an operand is a machine word with no runtime behind
            // it. The `asm` itself carries no type that the heap check above
            // would have rejected.
            ExprKind::Asm(block) => {
                for op in &block.operands {
                    if let Some(operand) = &op.operand {
                        self.expr(operand, module)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The `mmio` gate (kernel stage 16, MK10-S1): in a freestanding build, a
    /// `Ptr.from_addr` may only name an address the module was granted. The
    /// address is folded when it is a compile-time constant (`stdlib::const_int`)
    /// and matched against the module's half-open ranges; a non-constant address
    /// needs the whole-address-space grant `["*"]`, and a module with no grant
    /// at all is refused before either question is asked.
    ///
    /// This is a check on the address a pointer is *born* at. An offset applied
    /// later (`p.byte_offset(n)`, `p.offset(n)`) is not revisited, and nothing
    /// here says the address is memory-mapped or that it is not RAM
    /// (`kelp wiki page language/capabilities.md`).
    fn check_mmio(&self, args: &[Expr], module: &str, span: Span) -> Result<(), Diagnostic> {
        let patterns = self
            .caps
            .get(module)
            .and_then(|domains| domains.get("mmio"))
            .cloned()
            .unwrap_or_default();
        if patterns.is_empty() {
            return Err(unavailable(
                format!(
                    "'Ptr.from_addr' needs an 'mmio' grant, and module '{module}' holds none: the manifest's '[caps]' section or the importing `impl` line grants one (`caps(mmio = [\"0x10000000+0x100\"])`)"
                ),
                span,
                module,
            ));
        }
        if patterns.iter().any(|p| p == crate::stdlib::RANGE_ANY) {
            return Ok(());
        }
        let Some(addr) = args.first().and_then(crate::stdlib::const_int) else {
            return Err(unavailable(
                format!(
                    "'Ptr.from_addr' takes an address the compiler cannot fold, and module '{module}' holds only the range(s) {}; a computed address needs the whole-address-space grant `caps(mmio = [\"*\"])`",
                    patterns.join(", ")
                ),
                span,
                module,
            ));
        };
        if crate::stdlib::range_covers("mmio", &patterns, addr) {
            return Ok(());
        }
        Err(unavailable(
            format!(
                "'Ptr.from_addr' would take the address 0x{addr:X}, which no 'mmio' grant of module '{module}' covers (granted: {})",
                patterns.join(", ")
            ),
            span,
            module,
        ))
    }
}

#[cfg(test)]
#[path = "freestanding/tests.rs"]
mod tests;

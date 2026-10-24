//! Gitea #280: a function whose body is written `= expr` with no `=> T` takes
//! the type of that body as its return type.
//!
//! The signature phase leaves such a return type as `()`. Before any body is
//! checked, this pass checks each inferred body on its own, callee first, and
//! writes the body's type into the signature, so a caller always reads the
//! final type. A body that depends on itself, directly or through other
//! inferred bodies, has no type to start from and is refused.
//!
//! What is refused, and why:
//! - a trait method (an `impl` method or a synthesized default): its return
//!   type is the trait's, and the trait check reads it before the body;
//! - a generic function or method: one body would have to give a type for
//!   every instance;
//! - a `when` with several arms, as the issue asks: the explicit type stays;
//! - an `asm` body: a naked function's return is fixed by its calling
//!   convention;
//! - a body whose type is a borrow: a return type is always owned, the same
//!   rule `=> T` follows.
//!
//! A body that uses `?` or `return` has no declared type to check them
//! against while it is inferred, so it needs `=> T` too.

use super::*;

/// Whether `f` takes its return type from its body.
fn is_inferred(f: &ast::FunctionDecl) -> bool {
    f.return_type.is_none() && matches!(f.body, ast::FunctionBody::Expr(_))
}

/// The refusal for an inferred body that cannot take its type from itself, if
/// it is one.
fn refuse_untyped(f: &ast::FunctionDecl, body: &ast::Expr) -> Option<String> {
    let name = f.name.as_str();
    if f.trait_impl.is_some() || f.body_module.is_some() {
        return Some(format!(
            "'{name}' is a trait method: its return type comes from the trait, so write '=> T' on its expression body"
        ));
    }
    let generic = !decl_type_params(f).is_empty()
        || !f.const_params.is_empty()
        || f.receiver
            .as_ref()
            .is_some_and(|r| !r.type_args.is_empty() || !r.const_params.is_empty());
    if generic {
        return Some(format!(
            "generic function '{name}' needs a '=> T' return type: one body cannot give the type of every instance"
        ));
    }
    match body {
        ast::Expr::When { arms, .. } if arms.len() > 1 => Some(format!(
            "expression body of '{name}': a 'when' body with several arms requires a '=> T' return type"
        )),
        ast::Expr::Asm { .. } => Some(format!(
            "expression body of '{name}': an 'asm' body requires a '=> T' return type"
        )),
        _ => None,
    }
}

/// The last segment of a name, so `pkg.add` and a method `.add` both match `add`.
fn bare(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Every named type a type is built from, so the privacy check sees each one.
fn nominal_names(ty: &Ty, out: &mut Vec<String>) {
    match ty {
        Ty::Struct(n) => out.push(n.clone()),
        Ty::List(t)
        | Ty::Box(t)
        | Ty::Array(t, _)
        | Ty::Atomic(t)
        | Ty::Thread(t)
        | Ty::Mutex(t)
        | Ty::RwLock(t)
        | Ty::Ptr(t)
        | Ty::Ref { inner: t, .. } => nominal_names(t, out),
        Ty::Map(k, v) => {
            nominal_names(k, out);
            nominal_names(v, out);
        }
        Ty::Fn { params, ret } | Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
            for p in params {
                nominal_names(p, out);
            }
            nominal_names(ret, out);
        }
        _ => {}
    }
}

struct Walk<'a> {
    program: &'a ast::Program,
    decl_keys: &'a [String],
    /// The names of the program's types, and the ones it exports, for the
    /// private-in-public rule on an inferred return type.
    declared: std::collections::HashSet<String>,
    exported: std::collections::HashSet<String>,
    /// Program indices of the inferred functions, in declaration order.
    cands: Vec<usize>,
    /// `deps[a]` holds the candidate positions that `cands[a]`'s body names.
    deps: Vec<Vec<usize>>,
    /// 0 not yet visited, 1 being inferred, 2 done.
    state: Vec<u8>,
}

/// Infers the return type of every expression body that has none, writing each
/// into `sigs` under its declaration key. `check` checks one body against the
/// signatures as they stand, with no expected type.
pub(crate) fn infer_expr_bodies(
    program: &ast::Program,
    decl_keys: &[String],
    sigs: &mut HashMap<String, FnSig>,
    check: &dyn Fn(&ast::FunctionDecl, &str, &HashMap<String, FnSig>) -> Result<tir::Function>,
) -> std::result::Result<(), Diagnostic> {
    let cands: Vec<usize> = (0..program.functions.len())
        .filter(|&i| is_inferred(&program.functions[i]))
        .collect();
    if cands.is_empty() {
        return Ok(());
    }
    for &i in &cands {
        let f = &program.functions[i];
        if let ast::FunctionBody::Expr(body) = &f.body {
            if let Some(message) = refuse_untyped(f, body) {
                return Err(Diagnostic::new(message, f.span, f.module.clone()));
            }
        }
    }

    // The names each body mentions. The scan is by name, so a dependency can
    // be a name that is really a local or another type's method: that can
    // refuse a body that is not recursive, and `=> T` is the fix.
    let names: Vec<std::collections::HashSet<String>> = cands
        .iter()
        .map(|&i| {
            let mut out = std::collections::HashSet::new();
            if let ast::FunctionBody::Expr(e) = &program.functions[i].body {
                crate::defer::free_in_expr(e, &std::collections::HashSet::new(), &mut out);
            }
            out
        })
        .collect();
    let deps: Vec<Vec<usize>> = names
        .iter()
        .map(|mentioned| {
            (0..cands.len())
                .filter(|&b| {
                    let callee = bare(&program.functions[cands[b]].name);
                    mentioned.iter().any(|n| bare(n) == callee)
                })
                .collect()
        })
        .collect();

    let exported = collect_exports(program);
    let declared: std::collections::HashSet<String> = program
        .datacs
        .iter()
        .map(|d| d.name.clone())
        .chain(program.enums.iter().map(|e| e.name.clone()))
        .chain(program.extern_types.iter().map(|t| t.name.clone()))
        .collect();
    let mut walk = Walk {
        program,
        decl_keys,
        declared,
        exported,
        state: vec![0; cands.len()],
        cands,
        deps,
    };
    for a in 0..walk.cands.len() {
        visit(a, &mut walk, sigs, check)?;
    }
    Ok(())
}

fn recursive(program: &ast::Program, i: usize) -> Diagnostic {
    let f = &program.functions[i];
    Diagnostic::new(
        format!(
            "recursive expression-bodied function '{}' requires a '=> T' return type",
            f.name
        ),
        f.span,
        f.module.clone(),
    )
}

fn visit(
    a: usize,
    walk: &mut Walk<'_>,
    sigs: &mut HashMap<String, FnSig>,
    check: &dyn Fn(&ast::FunctionDecl, &str, &HashMap<String, FnSig>) -> Result<tir::Function>,
) -> std::result::Result<(), Diagnostic> {
    if walk.state[a] == 2 {
        return Ok(());
    }
    walk.state[a] = 1;
    for k in 0..walk.deps[a].len() {
        let b = walk.deps[a][k];
        match walk.state[b] {
            1 => return Err(recursive(walk.program, walk.cands[b])),
            0 => visit(b, walk, sigs, check)?,
            _ => {}
        }
    }
    let i = walk.cands[a];
    let f = &walk.program.functions[i];
    let key = walk.decl_keys[i].as_str();
    let checked = check(f, key, sigs).map_err(|e| e.in_module(&f.module))?;
    let ret = checked.body.ty();
    if crate::types::is_borrowed_field_ty(&ret) {
        return Err(Diagnostic::new(
            format!(
                "expression body of '{}' returns a borrow; an inferred return type is always owned, so write '=> T' with the owned type",
                f.name
            ),
            f.span,
            f.module.clone(),
        ));
    }
    // The private-in-public rule (`check_pub_signatures`) reads the written
    // signature, which an inferred one does not have, so it is applied here.
    let reachable = match &f.receiver {
        Some(r) => walk.exported.contains(&r.type_name),
        None => walk.exported.contains(&f.name),
    };
    if reachable {
        let mut named = Vec::new();
        nominal_names(&ret, &mut named);
        if let Some(n) = named.iter().find(|n| {
            walk.declared.contains(bare(n)) && !walk.exported.contains(bare(n))
        }) {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is private, so it cannot be the inferred return type of the public function '{}': make '{}' public, or write '=> T' with a public type",
                    bare(n),
                    f.name,
                    bare(n)
                ),
                f.span,
                f.module.clone(),
            ));
        }
    }
    if let Some(sig) = sigs.get_mut(key) {
        sig.ret = ret;
    }
    walk.state[a] = 2;
    Ok(())
}

use super::*;

/// Canonical module id for a file: its path relative to `src/`, minus the
/// `.kp` suffix, with `/` separators.
/// A map from an associated type's name to the type an impl bound it to, as
/// written (A-12 slice 5), used to substitute `Self::Item` in an impl method.
pub(crate) fn assoc_bindings(bindings: &[ast::AssocTypeBinding]) -> std::collections::HashMap<String, ast::TypeExpr> {
    bindings
        .iter()
        .map(|b| (b.name.clone(), b.ty.clone()))
        .collect()
}

/// One `impl` line's trait argument (A-12 slice 6), canonicalized the way the
/// `for` type is: a builtin keeps its name, and a declared `datac`/`enum` is
/// qualified with its module prefix. A trait argument has to be a named type or
/// a builtin, so a container, a tuple, an array, a borrow, a function type and
/// a projection are refused here, where the argument was written.
pub(crate) fn canon_trait_arg(
    a: &ast::TypeExpr,
    canon: &dyn Fn(&str, Span) -> Result<String, Diagnostic>,
    module: &str,
) -> Result<String, Diagnostic> {
    let simple = a.tuple.is_none()
        && a.array_elem.is_none()
        && a.fn_params.is_none()
        && a.dyn_trait.is_none()
        && a.assoc.is_none()
        && a.args.is_empty()
        && !a.amp
        && !a.name.is_empty();
    if !simple {
        return Err(Diagnostic::new(
            format!(
                "a trait argument has to be a named type or a builtin, not '{}': a container, tuple, array, borrow, function type or projection as a trait's argument is not in this slice",
                a.display()
            ),
            a.span,
            module.to_string(),
        ));
    }
    if crate::types::Ty::from_name(&a.name).is_some() {
        return Ok(a.name.clone());
    }
    canon(&a.name, a.span)
}

/// Replaces the trait's own type parameters with the impl's arguments in a
/// synthesized default method's signature (A-12 slice 6): the trait wrote
/// `value: T`, and the copy has to write the concrete type the impl named.
pub(crate) fn substitute_type_params(f: &mut ast::FunctionDecl, params: &[(String, ast::TypeExpr)]) {
    for (name, repl) in params {
        for p in &mut f.params {
            substitute_named_ty(&mut p.ty, name, repl);
        }
        if let Some(t) = &mut f.return_type {
            substitute_named_ty(t, name, repl);
        }
    }
}

/// One named type anywhere in a type expression replaced by another, for the
/// trait-parameter substitution above. A `Name::Item` projection and a name
/// that already carries arguments are left alone.
pub(crate) fn substitute_named_ty(ty: &mut ast::TypeExpr, name: &str, repl: &ast::TypeExpr) {
    if ty.name == name
        && ty.assoc.is_none()
        && ty.args.is_empty()
        && ty.tuple.is_none()
        && ty.array_elem.is_none()
        && ty.fn_params.is_none()
    {
        *ty = repl.clone();
        return;
    }
    if let Some(elems) = &mut ty.tuple {
        for e in elems {
            substitute_named_ty(e, name, repl);
        }
    }
    for a in &mut ty.args {
        substitute_named_ty(a, name, repl);
    }
    if let Some(elem) = &mut ty.array_elem {
        substitute_named_ty(elem, name, repl);
    }
    if let Some(params) = &mut ty.fn_params {
        for p in params {
            substitute_named_ty(&mut p.ty, name, repl);
        }
    }
    if let Some(ret) = &mut ty.fn_ret {
        substitute_named_ty(ret, name, repl);
    }
}

/// Replaces every `Self` in one `impl` method's signature with the written
/// `for` type (A-12 slice 2). A method inside an `impl` block names its
/// receiver `Self` so the block reads the same wherever it sits, and this is
/// what turns it into the ordinary receiver method every later pass expects.
/// Only a *type* named `Self` is replaced: an expression naming `Self` is left
/// alone, and `infer.rs` refuses it there as an unknown name. A `Self::Item`
/// (A-12 slice 5) becomes the type the impl bound, so the signature reaches
/// the type checker concrete.
pub(crate) fn substitute_self(
    f: &mut ast::FunctionDecl,
    self_type: &str,
    assoc: &std::collections::HashMap<String, ast::TypeExpr>,
) {
    if let Some(r) = &mut f.receiver {
        if r.type_name == "Self" {
            r.type_name = self_type.to_string();
        }
    }
    for p in &mut f.params {
        substitute_self_ty(&mut p.ty, self_type, assoc);
    }
    if let Some(t) = &mut f.return_type {
        substitute_self_ty(t, self_type, assoc);
    }
}

/// One type expression's `Self`, including inside a tuple, an array, a
/// container's arguments and a function type's parameters and return.
pub(crate) fn substitute_self_ty(
    ty: &mut ast::TypeExpr,
    self_type: &str,
    assoc: &std::collections::HashMap<String, ast::TypeExpr>,
) {
    if ty.name == "Self" {
        if let Some(item) = ty.assoc.clone() {
            if let Some(bound) = assoc.get(&item) {
                let mut replacement = bound.clone();
                substitute_self_ty(&mut replacement, self_type, assoc);
                *ty = replacement;
            }
            return;
        }
        if ty.args.is_empty() {
            ty.name = self_type.to_string();
        }
    }
    if let Some(elems) = &mut ty.tuple {
        for e in elems {
            substitute_self_ty(e, self_type, assoc);
        }
    }
    for a in &mut ty.args {
        substitute_self_ty(a, self_type, assoc);
    }
    if let Some(elem) = &mut ty.array_elem {
        substitute_self_ty(elem, self_type, assoc);
    }
    if let Some(params) = &mut ty.fn_params {
        for p in params {
            substitute_self_ty(&mut p.ty, self_type, assoc);
        }
    }
    if let Some(ret) = &mut ty.fn_ret {
        substitute_self_ty(ret, self_type, assoc);
    }
}

pub(crate) fn module_id_of(file: &Path, src_dir: &Path) -> Result<String, Diagnostic> {
    let rel = file.strip_prefix(src_dir).map_err(|_| {
        Diagnostic::new(
            format!(
                "entry file '{}' is not inside '{}'",
                file.display(),
                src_dir.display()
            ),
            Span::new(0, 0),
            "",
        )
    })?;
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = rel.parent().map(|p| p.to_string_lossy().replace('\\', "/"));
    let id = match dir.as_deref() {
        Some(d) if !d.is_empty() && d != "." => format!("{d}/{stem}"),
        _ => stem,
    };
    if id.is_empty() {
        return Err(Diagnostic::new(
            format!("cannot determine a module name for '{}'", file.display()),
            Span::new(0, 0),
            "",
        ));
    }
    if id.starts_with(INTERNAL_STDLIB_PREFIX) {
        return Err(Diagnostic::new(
            format!("module '{id}' is below the compiler-reserved std/internal namespace"),
            Span::new(0, 0),
            "",
        ));
    }
    Ok(id)
}

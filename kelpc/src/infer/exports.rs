use super::*;

/// The private-in-public rule (A-13, `design/syntax.md (kelp wiki)` 13.4): a declaration
/// another module can reach must not name, anywhere in its signature, a type
/// its module does not export. A private declaration may name anything,
/// because nothing outside its module can name it either.
///
/// The check runs once over the *merged* program with the whole export set
/// already in hand (`collect_exports`), rather than per item while the
/// declarations are collected, so a private type declared after the item that
/// names it is caught the same way as one declared before: the order of two
/// declarations in one file does not decide the answer.
pub(crate) fn check_pub_signatures(
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
pub(crate) struct SignatureScope<'a> {
    pub(crate) project: &'a Project,
    pub(crate) module: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) aliases: HashMap<String, String>,
    /// The public item, as a reader writes it, for the refusal.
    pub(crate) item: &'a str,
    /// The declaration to make private instead: the item itself, or a
    /// method's declaring type.
    pub(crate) fix: &'a str,
    /// The type parameters in scope. A parameter is not a declared type, so
    /// naming one is never a violation, exactly as naming a builtin is not.
    pub(crate) type_params: Vec<&'a str>,
    pub(crate) exported: &'a std::collections::HashSet<String>,
    pub(crate) declared: &'a std::collections::HashSet<&'a str>,
}

/// The name a callee key shows in a diagnostic: overload keys carry their
/// parameter types (`std.math.abs@f64`), which read as noise in an error
/// message about arguments.
pub(crate) fn display_callee(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// A function's signature key. An overloadable name in a stdlib module
/// carries its parameter types, which is what lets several declarations of
/// `abs` coexist in `std/math.kp` and be told apart at a call site.
pub(crate) fn sig_key_for(f: &ast::FunctionDecl, prefix: &str, params: &[(String, Ty)]) -> String {
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

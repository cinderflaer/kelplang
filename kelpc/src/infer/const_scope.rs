use super::*;

/// Whether `ty` may be a static's type (kernel stage 7): a `Copy` scalar, an
/// integer/`bool`/`char` array, a `datac` of those (recursively), or an
/// `Atomic<T>`. Everything else owns memory, is a borrow or a function value,
/// or is a shape the linker cannot place as one object.
pub(crate) fn check_static_type(ty: &Ty, structs: &HashMap<String, StructDef>) -> std::result::Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    check_static_type_inner(ty, structs, &mut seen)
}

pub(crate) fn check_static_type_inner(
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
pub(crate) fn static_init_of(
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

pub(crate) fn not_constant(ty: &Ty) -> String {
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
pub(crate) fn const_int_array(
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
pub(crate) fn int_literal(expr: &ast::Expr) -> Option<i128> {
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
pub(crate) fn value_of(e: &tir::Expr) -> Option<ConstResult> {
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
/// typed C literal comes out of the removed C backend's ordinary literal path, so a
/// `const` adds no code path of its own.
pub(crate) fn literal_for(value: &ConstValue, declared: &Ty) -> std::result::Result<tir::Expr, String> {
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
pub(crate) const CONST_EVAL_STEPS: u64 = 1_000_000;

/// How deep a compile-time call may nest, for the same reason.
pub(crate) const CONST_EVAL_DEPTH: u32 = 64;

/// The result of one compile-time evaluation: the value and the type it has.
pub(crate) type ConstResult = (ConstValue, Ty);

/// One local binding of an evaluated body: its value, its type, and whether
/// the body may write it (`var`).
pub(crate) struct ConstBinding {
    pub(crate) value: ConstResult,
    pub(crate) mutable: bool,
}

/// What a const-evaluated body has in scope: its local bindings, innermost
/// frame last, the module its names resolve against, and the return type of
/// the function being evaluated (for `return`).
pub(crate) struct ConstScope {
    pub(crate) frames: Vec<HashMap<String, ConstBinding>>,
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) ret_ty: Option<Ty>,
    /// A `return` that has happened while this body is being evaluated, with
    /// the value it carried. Every expression after it is skipped and the call
    /// boundary reads it as the function's result, which is how a `return`
    /// inside an `if` unwinds out of a `fun` exactly as it does at runtime.
    ///
    /// It lives on the scope rather than in an error type so that a `return`
    /// reaches only the body it belongs to: a call evaluates its body in a
    /// scope of its own, so the flag cannot leak into the caller.
    pub(crate) returning: Option<Option<ConstResult>>,
}

/// How a block finished.
pub(crate) enum ConstFlow {
    /// It ran to the end. `Some` is the tail expression's value.
    Fell(Option<ConstResult>),
    /// A `return` left it.
    Returned(Option<ConstResult>),
}

impl ConstScope {
    pub(crate) fn new(prefix: String, aliases: HashMap<String, String>) -> ConstScope {
        ConstScope {
            frames: vec![HashMap::new()],
            prefix,
            aliases,
            ret_ty: None,
            returning: None,
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&ConstResult> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.get(name).map(|b| &b.value))
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut ConstBinding> {
        self.frames.iter_mut().rev().find_map(|f| f.get_mut(name))
    }

    /// Whether a `return` has already happened in this body, in which case
    /// nothing after it is evaluated and the value it carries is the one the
    /// enclosing call produces.
    pub(crate) fn halted(&self) -> bool {
        self.returning.is_some()
    }
}

/// The two's-complement width of an integer type, in bits.
pub(crate) fn int_bits(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::I32 | Ty::U32 => 32,
        Ty::I64 | Ty::Int | Ty::U64 | Ty::Uint | Ty::Usize | Ty::Isize => 64,
        Ty::U8 => 8,
        Ty::U16 => 16,
        _ => return None,
    })
}

/// Whether an integer type's values are non-negative.
pub(crate) fn int_unsigned(ty: &Ty) -> bool {
    matches!(ty, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Uint | Ty::Usize)
}

/// `v` reduced into `ty`'s range, so a folding arithmetic step behaves like the
/// generated C: a `u8` wraps at 256 and an `i64` at its two's-complement
/// range, and neither silently grows into the next width.
pub(crate) fn wrap_int(v: i128, ty: &Ty) -> i128 {
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
pub(crate) struct ConstEvaluator<'a> {
    pub(crate) const_fns: &'a ConstFns,
    pub(crate) structs: &'a HashMap<String, StructDef>,
    pub(crate) enums: &'a HashMap<String, EnumDef>,
    pub(crate) env: &'a TypeEnv,
    pub(crate) consts: &'a HashMap<String, ConstInfo>,
    pub(crate) steps: std::cell::Cell<u64>,
    pub(crate) depth: std::cell::Cell<u32>,
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
pub(crate) fn collect_templates(
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
pub(crate) fn collect_func_templates(
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
pub(crate) fn close_pd_modes(
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

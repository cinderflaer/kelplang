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
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => 64,
        Ty::U8 => 8,
        Ty::U16 => 16,
        _ => return None,
    })
}

/// Whether an integer type's values are non-negative.
pub(crate) fn int_unsigned(ty: &Ty) -> bool {
    matches!(ty, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize)
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
pub(crate) fn check_program_pass(
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
        managed: project.target().has_shared_pool(),
        heap_uses: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
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
    // the removed C backend must emit their C typedefs in: a `datac` may only name
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
    // `BlockError` is the built-in error of a manual block (decision 139): one
    // variant, `Full`. It is registered for every program in the same tables a
    // declared enum is, so a block's `Result` names it with no import, and its
    // layout comes from the same enum path codegen already reads.
    {
        let name = block_error_name();
        enums.insert(
            name.clone(),
            EnumDef {
                name: name.clone(),
                variants: vec![crate::types::VariantDef {
                    name: "Full".to_string(),
                    fields: Vec::new(),
                    discriminant: None,
                }],
                repr: Repr::C,
                order: 0,
            },
        );
        enum_order.push(name.clone());
        structs.insert(
            name.clone(),
            StructDef {
                name,
                fields: Vec::new(),
                repr: Repr::C,
                order: 0,
            },
        );
    }
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
            // allowed from M6.2 on: the removed C backend generates a drop function
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
        // `=> T` never goes through `resolve_param_type`, so a function's
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
        // which is what `fun classify(n: i32) => &str = "pos"` needs).
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
                "'{name}' declares '{}' but not '{missing}': a declared `Map` key needs both halves, 'fun {name}.hash() => u64' and 'fun {name}.eq(other: &{name}) => bool'",
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
        let message = if entry_key == "main" {
            "no 'main' function found".to_string()
        } else {
            format!(
                "no entry function '{entry_key}' found: coral.toml's '[target] entry' names a `fun` with no parameters and no return in the entry module"
            )
        };
        let mut diag = Diagnostic::new(message, Span::new(0, 0), project.entry.clone());
        if entry_key == "main" {
            diag = diag.with_help("add fun main() to the entry file. It takes no arguments and returns nothing.");
        }
        return Err(diag);
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
        if decl.thread_local {
            check_thread_local(decl, &pd.value_ty, &init)?;
        }
        checked_pds.push(tir::Pd {
            name: pd.hidden.clone(),
            qualified: pd.qualified.clone(),
            value_ty: pd.value_ty.clone(),
            init,
            module: decl.module.clone(),
            thread_local: decl.thread_local,
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
    // The capability data the removed C backend compiles in: each module's
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
    // (MK9-S1-M1, roadmap B-1), keyed by the function's full, prefixed name  - 
    // the removed C backend's `write_extern` reads this instead of `externs.rs` when a
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

    // Tidepool: every heap slot the pass recorded, decided now that every
    // declaration's facts are known (`managed::check_heap_uses`).
    if env.managed {
        let uses = env.heap_uses.borrow().clone();
        managed::check_heap_uses(&uses, &|name: &str| {
            managed::decl_facts(&records, &sums, &declared_drops, name)
        })?;
    }

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
        const_data,
        statics: static_decls,
        dyn_vtables,
        dyn_traits,
    })
}

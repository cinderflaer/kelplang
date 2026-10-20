use super::*;

/// MK9 section 5's argument rules, applied to `thread.spawn`'s moved value.
/// The rule is structural and builtin (rule 5): there is no `Send`/`Sync` marker
/// to implement until traits ship (A-12), so this lives beside the existing
/// "a borrowed type cannot be stored" rule rather than in a trait.
///
/// `expr` is the argument as written, because rules 2 and 3 are about the
/// *place* a borrow comes from and that is a lexical question; `value` is it
/// checked, because the borrow's shape is its type.
pub(crate) fn check_thread_arg(expr: &ast::Expr, value: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &value.ty {
        // Rule 1: an exclusive borrow never crosses. A `&mut T` is the promise
        // that no other path reaches the value, and the worker outlives the
        // frame the borrow was made in, so the promise cannot hold.
        Ty::Ref { mutable: true, inner } => Err(TypeError {
            help: None,
            message: format!(
                "'thread.spawn' cannot send '&mut {}': an exclusive borrow promises that no other path reaches the value, and the worker outlives the frame that made the borrow, so the promise cannot cross the thread boundary; move an owned value in, or share a '&Atomic<T>'",
                inner.display()
            ),
            span: expr.span(),
        }),
        // Rules 2 and 3: a shared borrow crosses only when it borrows a `pd`
        // global (process lifetime, so it outlives the worker) and only when
        // the borrowed type has no reachable `&mut` path.
        Ty::Ref { mutable: false, inner } => {
            if !borrows_a_pd(expr, ctx) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'thread.spawn' cannot send '{}': a shared borrow may cross to a worker only when the place it borrows has process lifetime, which in v1 means a 'pd' global, and this one borrows a frame local that ends before the worker does; share a 'pd' value instead, or move an owned value in",
                        value.ty.display()
                    ),
                    span: expr.span(),
                });
            }
            if has_reachable_mut_path(inner, ctx) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'thread.spawn' cannot share '{}' across a thread boundary: the borrow reaches a 'var' field, so two threads could write it through the same shared borrow. A shared borrow may cross only when its pointee has no reachable '&mut' path, which an 'Atomic<T>' has by construction",
                        inner.display()
                    ),
                    span: expr.span(),
                });
            }
            Ok(())
        }
        // Rule 2's other shape: a borrowed trait object is a borrow of the
        // value its data pointer addresses, exactly as a `Ty::Ref` is, so it
        // never crosses. An *owned* trait object (`Box<dyn Trait>`) is not this
        // arm: it owns its value and falls through to rule 4 like any other
        // owned value (A-12 slice 7).
        Ty::Dyn { trait_name, .. } => Err(TypeError {
            help: None,
            message: format!(
                "'thread.spawn' cannot send '&dyn {trait_name}': a borrowed trait object points at a value the frame owns, and the worker outlives that frame; move an owned 'Box<dyn {trait_name}>' in instead, or share a 'pd' value"
            ),
            span: expr.span(),
        }),
        // Rule 2's other shapes: a `str` and a `View` are borrowed text with
        // no place of their own, so there is nothing with process lifetime for
        // them to borrow.
        Ty::Str | Ty::View => Err(TypeError {
            help: None,
            message: format!(
                "'thread.spawn' cannot send a '{}': it is a borrowed view of text somebody else owns, and the worker outlives the frame that owns the text; pass an owned 'String' or 'Bytes' instead, or share a 'pd' value",
                value.ty.display()
            ),
            span: expr.span(),
        }),
        // Rule 4 falls out of `move T`: an owned value that carried a borrow
        // could not be constructed in the first place (`is_borrowed_field_ty`
        // refuses a borrow, a `str` or a `View` in a `datac` field, an enum
        // payload, a `List` element or a `Map` value), and the top-level
        // borrow shapes are the three arms above. There is no hole to open.
        _ => Ok(()),
    }
}

/// Whether the expression that made a shared borrow borrows a `pd` global,
/// which is the one place MK9 section 5 rule 2 lets a borrow cross to a worker:
/// a `pd`'s lifetime is the process, so it outlives the worker.
///
/// Checked over the place expression as written, which is what makes the rule
/// decidable: the root identifier has to be a passdown of this module and not
/// shadowed by a local of the same name.
pub(crate) fn borrows_a_pd(expr: &ast::Expr, ctx: &Ctx) -> bool {
    let ast::Expr::Ref { expr: place, .. } = expr else {
        return false;
    };
    // A `tl` is a different copy in each thread, so a borrow of one names the
    // spawning thread's copy and never crosses.
    matches!(place_root_ident(place), Some(name) if ctx.pds.get(name).is_some_and(|p| !p.thread_local) && ctx.lookup(name).is_none())
}

/// The identifier a place expression is rooted at: `counter`, `counter.field`
/// and `counter.a.b` all answer `counter`. `None` for anything else, a
/// dereference included, whose storage this lexical check does not follow.
pub(crate) fn place_root_ident(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Ident(name, _) => Some(name),
        // The one field spelling a place is written with: `p.x` parses as a
        // `FieldAccess`. There is no other way to write a place.
        ast::Expr::FieldAccess { base, .. } => place_root_ident(base),
        _ => None,
    }
}

/// Whether a shared borrow of `ty` reaches a `var` field, which is exactly the
/// `Sync`-shaped question MK9 section 5 rule 3 asks: a type with no reachable
/// `&mut` path may be shared across a thread boundary, and one with a reachable
/// `var` field may not. An `Atomic<T>` answers no by construction, which is what
/// makes a `&Atomic<T>` the shareable type v1 ships.
pub(crate) fn has_reachable_mut_path(ty: &Ty, ctx: &Ctx) -> bool {
    let mut seen = std::collections::HashSet::new();
    reachable_mut_path(ty, ctx, &mut seen)
}

pub(crate) fn reachable_mut_path(
    ty: &Ty,
    ctx: &Ctx,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    match ty {
        // A `datac` field and an `enum` payload are both reachable by a field
        // or a pattern access, so either one being `var` is a `&mut` path. The
        // walk recurses through a field whatever the field's own mutability:
        // `a.b.c` is reachable from `a` when `c` is `var`, and section 5 asks
        // about reachability rather than about the first step.
        Ty::Struct(name) => {
            if !seen.insert(name.clone()) {
                return false;
            }
            if let Some(def) = ctx.struct_def(name) {
                return def
                    .fields
                    .iter()
                    .any(|f| f.mutable || reachable_mut_path(&f.ty, ctx, seen));
            }
            if let Some(def) = ctx.enum_def(name) {
                return def
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| f.mutable || reachable_mut_path(&f.ty, ctx, seen));
            }
            false
        }
        // A container's element is reached through the container's own access,
        // so a `var` field under one is just as reachable.
        Ty::List(elem) | Ty::Box(elem) | Ty::Array(elem, _) => {
            reachable_mut_path(elem, ctx, seen)
        }
        Ty::Map(_, value) => reachable_mut_path(value, ctx, seen),
        // An atomic mutates through a shared borrow and has no `&mut` path at
        // all, which is the property the whole rule is built on (MK9 section 4).
        Ty::Atomic(_) => false,
        // A `Mutex<T>` has no reachable path to `T` at all outside the scoped
        // `lock` block, so it is shareable across a thread boundary whatever
        // its element: two threads can only take turns through the lock
        // (B-5).
        Ty::Mutex(_) => false,
        // An `RwLock<T>` reaches `T` only through its scoped `read`/`write`
        // blocks, so it is shareable across a thread boundary whatever its
        // element, exactly as `Mutex<T>` is (B-5). A `Condvar` holds no `T` at
        // all and is shared by construction.
        Ty::RwLock(_) => false,
        Ty::Condvar => false,
        _ => false,
    }
}

/// A compile-time layout query (kernel stage 2): `size_of<T>()`,
/// `align_of<T>()` and `offset_of<T>("field")`. Each folds to a `usize`
/// literal here, so both backends see an ordinary integer and no code
/// generator has anything layout-specific to lower. `T` is a scalar, an
/// opaque handle, a `Ptr<U>`, a `datac` or an `enum`: the types that have a
/// machine-level layout and no heap representation to reason about.
pub(crate) fn check_layout_query(
    callee: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &Ctx,
) -> Result<tir::Expr> {
    if explicit.len() != 1 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{callee}' takes exactly one type argument, written '{callee}<T>()'"
            ),
            span,
        });
    }
    let field = if callee == "offset_of" {
        if args.len() != 1 {
            return Err(TypeError {
                help: None,
                message: "'offset_of' takes exactly one field name, written 'offset_of<T>(\"field\")'"
                    .to_string(),
                span,
            });
        }
        match &args[0].value {
            ast::Expr::StringLit(field, _) if args[0].name.is_none() => Some(field.as_str()),
            _ => {
                return Err(TypeError {
                    help: None,
                    message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                })
            }
        }
    } else {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'{callee}' takes no arguments, found {}", args.len()),
                span,
            });
        }
        None
    };
    let value = layout_query_value(
        callee,
        &explicit[0],
        field,
        &ctx.structs,
        &ctx.enums,
        span,
    )?;
    Ok(tir::Expr {
        kind: tir::ExprKind::IntLit(value as i128),
        ty: Ty::Usize,
        span,
    })
}

/// The value one layout query folds to, shared by the expression checker and
/// the compile-time evaluator so both refuse the same things with the same
/// words.
pub(crate) fn layout_query_value(
    callee: &str,
    t: &Ty,
    field: Option<&str>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    span: Span,
) -> Result<u64> {
    let allowed = matches!(
        t,
        Ty::U8
            | Ty::U16
            | Ty::I32
            | Ty::I64
            | Ty::U32
            | Ty::U64
            | Ty::Usize
            | Ty::Isize
            | Ty::F32
            | Ty::F64
            | Ty::Bool
            | Ty::Char
            | Ty::Handle(_)
            | Ty::Ptr(_)
            | Ty::Struct(_)
    );
    if !allowed {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{callee}' cannot be asked about '{}': the queried type is a scalar, an 'extern type' handle, a 'Ptr<U>', a 'datac' or an 'enum'"
                , t.display()
            ),
            span,
        });
    }
    match callee {
        "size_of" | "align_of" => {
            let l = crate::types::type_layout(t, structs, enums).ok_or_else(|| TypeError {
                help: None,
                message: format!("'{}' has no layout this compiler can compute", t.display()),
                span,
            })?;
            Ok(if callee == "size_of" { l.size } else { l.align })
        }
        "offset_of" => {
            let Ty::Struct(name) = t else {
                return Err(TypeError {
                    help: None,
                    message: format!("'offset_of' needs a 'datac': '{}' is not one", t.display()),
                    span,
                });
            };
            let Some(field) = field else {
                return Err(TypeError {
                    help: None,
                    message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                });
            };
            crate::types::field_offset(name, field, structs, enums).ok_or_else(|| TypeError {
                help: None,
                message: format!("'{}' has no field '{field}'", t.display()),
                span,
            })
        }
        _ => unreachable!("only the three layout queries reach here"),
    }
}

/// The layout rules an enum's discriminants and `repr` have to obey (kernel
/// stage 2): `repr(packed)`/`repr(align(N))` are not an enum's to ask for, a
/// written discriminant is legal only on a field-less enum, the resolved tag
/// values are unique, and each fits the tag type `repr(<int>)` names.
pub(crate) fn validate_enum_layout(
    e: &EnumDef,
    span: Span,
    module: &str,
) -> std::result::Result<(), Diagnostic> {
    if let Repr::Packed | Repr::Align(_) = &e.repr {
        return Err(Diagnostic::new(
            "'repr(packed)' and 'repr(align(N))' are not supported on an enum: an enum's layout knob is the tag's integer type, written 'repr(u8)', 'repr(i16)', ..."
                .to_string(),
            span,
            module.to_string(),
        ));
    }
    if !e.variants.iter().any(|v| v.discriminant.is_some()) {
        return Ok(());
    }
    if !e.is_field_less() {
        return Err(Diagnostic::new(
            format!(
                "enum '{}' has a variant with a payload, so no variant may have an explicit discriminant: a discriminant is a value with no data beside it, so it is written only on a field-less enum",
                e.name
            ),
            span,
            module.to_string(),
        ));
    }
    let (min, max): (i128, i128) = match &e.repr {
        Repr::Int(name) => match name.as_str() {
            "u8" => (0, u8::MAX as i128),
            "u16" => (0, u16::MAX as i128),
            "u32" => (0, u32::MAX as i128),
            "u64" => (0, u64::MAX as i128),
            "i8" => (i8::MIN as i128, i8::MAX as i128),
            "i16" => (i16::MIN as i128, i16::MAX as i128),
            "i32" => (i32::MIN as i128, i32::MAX as i128),
            _ => (i64::MIN as i128, i64::MAX as i128),
        },
        _ => (i32::MIN as i128, i32::MAX as i128),
    };
    let values = e.discriminants();
    for (i, value) in values.iter().enumerate() {
        let holder = e
            .variants
            .iter()
            .find(|v| v.discriminant == Some(*value))
            .map(|v| v.name.clone())
            .unwrap_or_else(|| e.variants[i].name.clone());
        if values[..i].contains(value) {
            return Err(Diagnostic::new(
                format!(
                    "variant '{holder}' of enum '{}' repeats the discriminant {value}: every variant of a field-less enum has its own value",
                    e.name
                ),
                span,
                module.to_string(),
            ));
        }
        if *value < min || *value > max {
            let tag = e.tag_ty().display();
            return Err(Diagnostic::new(
                format!(
                    "variant '{holder}' of enum '{}' has discriminant {value}, which does not fit its tag type '{tag}'"
                    , e.name
                ),
                span,
                module.to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_call(
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
    // The compile-time layout queries (kernel stage 2). They are resolved
    // before the program's own declarations, like `print` and `panic`, so a
    // program cannot shadow one and get a different `size_of` than the ABI.
    if matches!(callee, "size_of" | "align_of" | "offset_of") && !callee.contains('.') {
        return check_layout_query(callee, &explicit, args, span, ctx);
    }
    // The per-CPU intrinsics (kernel stage 8). `cpu_count()` folds to the
    // manifest's `[target] cpus`; `cpu_index()` stays a call, read from the
    // register the kernel set at boot. Both are freestanding-only and take no
    // arguments, and both are resolved before the program's own declarations
    // for the same reason the layout queries are.
    if matches!(callee, "cpu_index" | "cpu_count") && !callee.contains('.') {
        if !ctx.freestanding {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{callee}' is only available in a freestanding build: it reads the CPU index register the kernel sets at boot"
                ),
                span,
            });
        }
        if !explicit.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'{callee}' takes no type arguments"),
                span,
            });
        }
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'{callee}' takes no arguments, found {}", args.len()),
                span,
            });
        }
        let kind = if callee == "cpu_count" {
            tir::ExprKind::IntLit(i128::from(ctx.cpus))
        } else {
            tir::ExprKind::Call {
                callee: "cpu_index".to_string(),
                args: Vec::new(),
            }
        };
        return Ok(tir::Expr {
            kind,
            ty: Ty::Usize,
            span,
        });
    }
    if callee == "main" {
        // `main` is the program entry point; it takes no arguments, so there
        // is no way to hand it the passdowns it may reference, and its own
        // `argv` is reached through `process.args()` instead.
        return Err(TypeError {
            help: None,
            message: "cannot call 'main': it is the program entry point".to_string(),
            span,
        });
    }
    if PRINTABLE_BUILTINS.contains(&callee) {
        if args.len() != 1 {
            return Err(TypeError {
                help: None,
                message: format!("'{callee}' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        if args[0].name.is_some() {
            return Err(TypeError {
                help: None,
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
        // An opaque handle has no readable value, so there is nothing to
        // format: name it rather than fall through to the generic refusal.
        if let Ty::Handle(name) = &arg.ty {
            return Err(TypeError {
                help: None,
                message: format!(
                    "cannot pass a value of type '{name}' to '{callee}': an 'extern type' is an opaque native handle with no printable value"
                ),
                span,
            });
        }
        let printable = arg.ty.is_numeric()
            || matches!(arg.ty, Ty::Bool | Ty::Char)
            || is_str_like(&arg.ty)
            || is_printable_buffer(&arg.ty);
        if !printable {
            // A `datac`/`enum` that implements `Display` prints through
            // `x.to_text()`, which yields the owned `String` the ordinary
            // print path already formats (`design/traits.md (kelp wiki)` 13.4).
            if let Some(text) = lower_display(&arg, span, ctx)? {
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Call {
                        callee: callee.to_string(),
                        args: vec![text],
                    },
                    ty: Ty::Unit,
                    span,
                });
            }
            return Err(TypeError {
                help: None,
                message: print_error(callee, &arg.ty),
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

    // `panic` takes a message, and the owned spelling of one now sits beside
    // the borrowed `str` every call site already had: a message built as a
    // `String` (`fmt.int(n)`, `str.concat`) no longer has to be bound to a
    // `val` or spelled `.as_str()` first, which is exactly the ceremony an
    // error path does not want. The buffer is read, never consumed, so this is
    // `print`'s lifetime rule and `print`'s temporary handling, and a message
    // that is neither a `str` nor an owned `String` is still refused by name.
    if callee == "panic" {
        if args.len() != 1 {
            return Err(TypeError {
                help: None,
                message: format!("'panic' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        if args[0].name.is_some() {
            return Err(TypeError {
                help: None,
                message: "'panic' does not take named arguments".to_string(),
                span,
            });
        }
        let arg = check_expr(&args[0].value, ctx, None)?;
        let arg = borrow_for_print(arg, callee, ctx)?;
        if !is_panic_message(&arg.ty) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "cannot pass a value of type '{}' to 'panic': a panic message is a 'str' or an owned 'String'",
                    arg.ty.display()
                ),
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
            help: None,
            message: format!("'{callee}' does not take type arguments"),
            span,
        });
    }

    // `Shape(r = 1.0)` - an enum is constructed by naming a variant, never by
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
            help: None,
            message: format!(
                "'{ename}' is an enum - construct one of its variants: {}",
                variants.join(", ")
            ),
            span,
        });
    }
    if let Some(sdef) = ctx
        .struct_def(callee)
        .or_else(|| ctx.struct_def(&ctx.qualify_own(callee)))
    {
        let field_names: Vec<&str> = sdef.fields.iter().map(|f| f.name.as_str()).collect();
        if let Some(domains) = crate::stdlib::handle_ctor_domains(&sdef.name, &field_names) {
            // A module inside the library builds its own handles; any other
            // module needs a domain that mints this kind (threat model H1).
            let inside = !ctx.prefix.is_empty() && sdef.name.starts_with(ctx.prefix.as_str());
            let ctx_is_shipped = is_shipped_module(&ctx.module);
            if !inside && !ctx_is_shipped && !domains.iter().any(|d| ctx.caps.contains_key(*d)) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' wraps an operating-system descriptor, so building one from a number needs the '{}' capability, which module '{}' was not granted{}; a module that holds no grant can only use the handles it is given",
                        display_callee(&sdef.name),
                        domains[0],
                        ctx.module,
                        if domains.len() > 1 {
                            format!(" (any of {} would do)", domains.join(", "))
                        } else {
                            String::new()
                        }
                    ),
                    span,
                });
            }
        }
        let params: Vec<(String, Ty)> = sdef.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
        let checked = match_args(&format!("datac '{}'", sdef.name), &params, args, span, ctx)?;
        let borrowed_fields = aggregate_borrowed_fields(&sdef.fields, &checked, span)?;
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
                borrowed_fields,
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
    //
    // The lookup on the name as written is for a callee that is *already*
    // canonical, and a canonical key always carries its module prefix. The
    // entry module is the only one whose keys are bare, so an ungated fallback
    // here would let every module call the entry module's item by its bare
    // name, `pub` or not (A-13).
    let names_a_module = callee.contains('.');
    let owned_key = ctx.qualify_own(callee);
    let generic_callee = func_template_named(ctx, callee, &owned_key).is_some();
    let direct = ctx.sig_of(&owned_key).map(|s| (owned_key.clone(), s)).or_else(|| {
        if !names_a_module {
            return None;
        }
        ctx.sig_of(callee).map(|s| (callee.to_string(), s))
    });
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
                    // consulted after the program's own signatures - a user
                    // function of the same name wins, and there is no
                    // shadowing surprise.
                    None => match builtin_sig(callee) {
                    Some(sig) => (callee.to_string(), sig),
                    // `thread.spawn` and `thread.join` (MK9-S3-M1 stage B1),
                    // whose signatures depend on the value that crosses the
                    // boundary and so live outside the table. Checked here, with
                    // the other builtin free functions, so a declaration of the
                    // same key still wins.
                    None if crate::stdlib::is_thread_fn(callee) => {
                        return check_thread_call(callee, args, span, ctx, expected)
                    }
                    None => {
                        return Err(TypeError {
                            help: crate::foreign::hint(callee).or_else(|| {
                                Some(format!("declare fun {callee} before this call, or import the module that declares it."))
                            }),
                            message: match qualified_hint(
                                callee,
                                ctx.structs
                                    .keys()
                                    .chain(ctx.sigs.keys())
                                    .chain(ctx.fns.templates.keys()),
                                &ctx.aliases,
                                &ctx.env,
                            ) {
                                Some(hint) => format!("call to undefined function '{callee}' - did you mean {hint}?"),
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
    // A function declared `unsafe fun` (MK9-S2 stage 1) is callable only from
    // an `unsafe { ... }` block or from another `unsafe fun`; the refusal names
    // the function so the reader knows which call in the statement it was.
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                display_callee(&key)
            ),
            span,
        });
    }
    if let Some(probe) = &ctx.pd_probe {
        // The call graph the borrow-mode closure walks (roadmap item 17):
        // whoever calls a writer writes too, when it holds the passdown
        // itself. Both ends are filed under their wiring key, so an
        // instantiation's edges are its template's.
        let callee_key = ctx.wiring_key_of(&key);
        probe
            .borrow_mut()
            .edges
            .insert((ctx.fn_key.clone(), callee_key));
    }
    // A user `extern fun` whose last parameter is `...` (gap 7) accepts any
    // number of extra arguments after the declared ones. Each extra argument
    // is checked here: its type has to be one a native `va_arg` can read back
    // (an integer, `bool`, `f32`/`f64`, `str` or an opaque handle), with no
    // implicit conversion, and the call names one generated fixed-arity C
    // wrapper that forwards the fixed and the extra arguments to the native
    // symbol. The wrapper is created once per distinct list of extra types,
    // so two call sites that pass the same types share one.
    let variadic = ctx.variadic_externs.contains_key(&key);
    let what = format!("function '{}'", display_callee(&key));
    ctx.str_arg_site = Some(if generic_callee {
        StrArgSite::Refused(format!("generic function '{callee}'"))
    } else {
        StrArgSite::Implicit
    });
    let (mut checked, extra_args) = if variadic {
        match_args_split(&what, &sig.params, args, span, ctx, true)?
    } else {
        (match_args(&what, &sig.params, args, span, ctx)?, Vec::new())
    };
    require_place_for_borrowed_arg(&sig.params, &sig.ret, &checked, span)?;
    let wrapper = if variadic {
        let mut extra_tys = Vec::with_capacity(extra_args.len());
        let mut extra_exprs = Vec::with_capacity(extra_args.len());
        for (i, a) in extra_args.iter().enumerate() {
            let extra = check_expr(&a.value, ctx, None)?;
            if !crate::types::is_variadic_abi_ty(&extra.ty) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "extra argument {} of '{}' has type '{}', which a variadic 'extern fun' cannot pass: use an integer, 'bool', 'f32', 'f64', 'str' or an opaque handle",
                        i + 1,
                        display_callee(&key),
                        extra.ty.display()
                    ),
                    span: a.value.span(),
                });
            }
            extra_tys.push(extra.ty.clone());
            extra_exprs.push(extra);
        }
        checked.extend(extra_exprs);
        Some(register_variadic_wrapper(ctx, &key, &sig, &extra_tys, span))
    } else {
        None
    };
    checked.extend(hidden_args(&key, span, ctx)?);
    check_claim_argument(&key, &checked, span, ctx)?;

    Ok(tir::Expr {
        kind: match wrapper {
            Some(wrapper) => tir::ExprKind::VariadicCall {
                callee: key,
                wrapper,
                args: checked,
            },
            None => tir::ExprKind::Call {
                callee: key,
                args: checked,
            },
        },
        ty: sig.ret,
        span,
    })
}

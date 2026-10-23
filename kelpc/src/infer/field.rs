use super::*;

/// The field access arm of `check_expr_inner`.
pub(crate) fn check_field_expr(base: &Box<ast::Expr>, field: &String, span: &Span, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    if let ast::Expr::Ident(name, _) = base.as_ref() {
        if is_module_alias(name, ctx) {
            // `math.PI` - a module's `const`, written without call
            // parentheses (M6.3). Everything else in a module is a
            // function and needs a call.
            if let Some(prefix) = ctx.aliases.get(name).cloned() {
                let qualified = format!("{prefix}{field}");
                if declares(&qualified, ctx) {
                    ctx.check_exported(&qualified, name, field, *span)?;
                }
                if let Some(c) = ctx.consts.get(&qualified).cloned() {
                    let mut value = c.value;
                    value.span = *span;
                    return Ok(value);
                }
                // `other.NAME` - a module's `pub static`, written
                // without call parentheses, exactly as its `const` is.
                if let Some(s) = ctx.statics.get(&qualified).cloned() {
                    require_static_unsafe(&s, *span, ctx)?;
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::StaticRef(s.qualified),
                        ty: s.ty,
                        span: *span,
                    });
                }
                // `val f = math.sqrt` - a module's item named as a
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
                // The float math functions have an `f32` overload beside
                // the `f64` one, and `val m = math.sqrt` predates it: as
                // a value the name is the overload the expected function
                // type names, or the `f64` one when nothing does.
                if FLOAT_MATH_VALUES.contains(&field.as_str()) {
                    let wants = match expected {
                        Some(Ty::Fn { params, .. }) => Some(params.clone()),
                        _ => None,
                    };
                    let mut keys: Vec<String> = ctx
                        .sigs
                        .keys()
                        .filter(|k| k.split('@').next() == Some(&qualified))
                        .cloned()
                        .collect();
                    keys.sort();
                    let pick = keys.iter().find(|k| {
                        let sig = &ctx.sigs[*k];
                        match &wants {
                            Some(p) => sig.params.iter().map(|(_, t)| t).eq(p.iter()),
                            None => sig.params.iter().all(|(_, t)| matches!(t, Ty::F64 | Ty::Float)),
                        }
                    });
                    if let Some(key) = pick {
                        let fn_ty = fn_value_ty(&ctx.sigs[key]);
                        return Ok(tir::Expr {
                            kind: tir::ExprKind::FnRef(key.clone()),
                            ty: fn_ty,
                            span: *span,
                        });
                    }
                }
                if ctx.sigs.keys().any(|k| k.split('@').next() == Some(&qualified)) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{name}.{field}' is an overloaded stdlib function - its parameter types are part of the name, so it is not a value; write a non-generic wrapper function and pass that"
                        ),
                        span: *span,
                    });
                }
            }
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{name}' is a module, not a value - call its items as '{name}.{field}(...)'"
                ),
                span: *span,
            });
        }
    }
    // A cross-module type path (`shapes.Shape.Empty`) is gated
    // before it is read as a variant: the type has to be exported.
    if let Some(path) = expr_path(base) {
        ctx.check_type_path_exported(&path, *span)?;
    }
    // `Option.None` - a payload-less variant of a generic `enum`
    // (A-2), the generic counterpart of the `Shape.Empty` case below.
    if let Some(tmpl_name) = generic_type_path(base, ctx) {
        return check_generic_bare_variant(&tmpl_name, field, *span, ctx, expected);
    }
    // `Shape.Empty` - a payload-less variant, written without a call
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
                help: None,
                message: format!(
                    "'{field}' is not a variant of enum '{ename}' - the variants are {}",
                    names.join(", ")
                ),
                span: *span,
            });
        };
        if !vdef.fields.is_empty() {
            let fields: Vec<String> =
                vdef.fields.iter().map(|f| f.name.clone()).collect();
            return Err(TypeError {
                help: None,
                message: format!(
                    "variant '{}.{}' carries payload ({}) - construct it as '{}.{}({})'",
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
    ctx.field_base_ok = matches!(base.as_ref(), ast::Expr::Ident(..));
    let base_t = check_expr(base, ctx, None);
    ctx.field_base_ok = false;
    let base_t = base_t?;
    let Some(sname) = struct_name_of(&base_t.ty).map(|s| s.to_string()) else {
        if let ast::Expr::Ident(bname, _) = base.as_ref() {
            if let Some(block) = ctx.box_bindings.get(bname).cloned() {
                return Err(TypeError {
                    help: Some(format!("read the element with '{bname}.get()', which stays inside '{block}'")),
                    message: format!("block box '{bname}' has no field '{field}': its element is read through 'get()'"),
                    span: *span,
                });
            }
        }
        return Err(TypeError {
            help: None,
            message: format!("cannot access field '{field}' on non-struct type '{}'", base_t.ty.display()),
            span: *span,
        });
    };
    // An enum has no fields to read: its payload is reached through a
    // pattern, and its tag through `when` (A-1).
    if ctx.is_enum(&sname) {
        return Err(TypeError {
            help: None,
            message: format!(
                "enum '{sname}' has no field '{field}' - match it with 'when {sname} {{ ... }}' to read a payload"
            ),
            span: *span,
        });
    }
    let sdef = ctx.struct_def(&sname).expect("struct types are only produced for known structs");
    let field_ty = sdef.field(field).map(|f| f.ty.clone()).ok_or_else(|| TypeError {
        help: None,
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


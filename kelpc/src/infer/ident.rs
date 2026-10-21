use super::*;

/// The identifier arm of `check_expr_inner`.
pub(crate) fn check_ident_expr(name: &String, span: &Span, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    if let Some(block) = ctx.outer_boxes.get(name).cloned() {
        if ctx.lookup(name).is_none() {
            return Err(box_capture_error(name, &block, ctx, *span));
        }
    }
    if let Some(block) = ctx.box_bindings.get(name).cloned() {
        if !ctx.field_base_here {
            return Err(box_move_error(name, &block, ctx.box_sink_here.as_ref(), *span));
        }
    }
    if let Some(info) = ctx.lookup(name) {
        // An open container takes its element from the type this
        // position expects: `take(xs)` on a `fun take(xs: List<i32>)`
        // decides `xs` the same way `xs.push(1)` does, and the repair
        // is the same one (`check_block` rewrites the binding's local
        // to match).
        let ty = if crate::types::is_open_container(&info.ty) {
            let open = info.ty.clone();
            let keyable = ctx.keyable_set();
            match expected
                .and_then(|exp| crate::types::fill_open_container(&open, exp, &keyable))
            {
                Some(filled) => {
                    ctx.set_binding_ty(name, filled.clone());
                    filled
                }
                None => open,
            }
        } else {
            info.ty.clone()
        };
        return Ok(tir::Expr {
            kind: tir::ExprKind::Ident(name.clone()),
            ty,
            span: *span,
        });
    }
    // A const type parameter read as a value (A-20): the
    // instantiation folded it to a literal, so `i < N` and `[0; N]`
    // see the value the monomorphization was built for.
    if let Some(Ty::ConstInt { value, ty }) = ctx.type_args.get(name) {
        let kind = match **ty {
            Ty::Bool => tir::ExprKind::BoolLit(*value != 0),
            Ty::Char => tir::ExprKind::CharLit(*value as u32),
            _ => tir::ExprKind::IntLit(*value),
        };
        return Ok(tir::Expr {
            kind,
            ty: (**ty).clone(),
            span: *span,
        });
    }
    // Not a local or parameter - inside a receiver-form method,
    // fall back to the receiver's field of that name (no `self.`
    // qualifier is ever written; see `design/syntax.md (kelp wiki)` §5).
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
            help: None,
            message: format!(
                "a 'fun' literal cannot read passdown '{name}' - a passdown is passed to the function that names it as a hidden argument, and a function value is a plain pointer to code; pass the value as a parameter instead"
            ),
            span: *span,
        });
    }
    // Not a local or receiver field - a passdown (M5.2), which the
    // wiring scan has already made this function take as a hidden
    // parameter named `pd#...`, so the reference is that parameter.
    if let Some(pd) = ctx.pds.get(name).cloned() {
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "passdown '{}' is read before it is initialized - a 'pd' initializer can only use passdowns declared before it",
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
                help: None,
                message: format!(
                    "passdown '{}' cannot be read in another passdown's initializer: its value is not Copy",
                    pd.qualified
                ),
                span: *span,
            });
        }
        return Ok(pd.value(*span, ctx.pd_global_borrow, ctx.pd_exclusive(&pd)));
    }
    // Not a local, receiver field, or passdown - a declared `fun`
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
    // Not a local, receiver field, or passdown - a compile-time
    // constant (M6.3), substituted as the literal it folded to.
    if let Some(c) = ctx.consts.get(&ctx.qualify_own(name)).cloned() {
        let mut value = c.value;
        value.span = *span;
        return Ok(value);
    }
    // Not a local, receiver field, passdown, function value or
    // constant - a top-level `static` (kernel stage 7). A static is a
    // real object, so a use is a `StaticRef` place; a `static mut`
    // may only be named inside `unsafe`, and the gate is here so a
    // read, a write, a field access and an address are all covered
    // by the one check.
    if let Some(s) = ctx.statics.get(&ctx.qualify_own(name)).cloned() {
        require_static_unsafe(&s, *span, ctx)?;
        return Ok(tir::Expr {
            kind: tir::ExprKind::StaticRef(s.qualified),
            ty: s.ty,
            span: *span,
        });
    }
    // Inside a `fun` literal, a name that is none of the above and
    // *is* an enclosing binding is a capture (A-4 phase E): the
    // literal becomes a closure, the value is moved (or, for a `Copy`
    // type, copied) into an environment when the literal is
    // evaluated, and the body reads it out of that environment. The
    // read is recorded here, once per distinct name, in the order the
    // body first names them, which is the environment's field order.
    if ctx.in_fn_literal {
        if let Some(ty) = ctx.note_capture(name, *span) {
            let index = ctx
                .captures
                .iter()
                .position(|(n, _)| n == name)
                .expect("note_capture records the name");
            // A `Copy` binding the literal writes is captured by
            // exclusive borrow (`kelp wiki page language/functions.md`), and its
            // reads and writes go through the pointer, so the name is
            // the place the pointer addresses.
            if let Ty::Ref { mutable: true, inner } = &ty {
                if ctx.excl_captures.contains(name) && inner.is_copy() {
                    let place_ty = (**inner).clone();
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Deref {
                            expr: Box::new(tir::Expr {
                                kind: tir::ExprKind::Capture { index },
                                ty: ty.clone(),
                                span: *span,
                            }),
                        },
                        ty: place_ty,
                        span: *span,
                    });
                }
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Capture { index },
                ty,
                span: *span,
            });
        }
        // A field of the receiver the literal was written in is a
        // name the reader can *see*, but reading it out of a borrowed
        // receiver is a partial move (A-11) - which is why the field
        // itself, not just the literal, has to be the reason.
        if let Some(owner) = &ctx.outer_receiver {
            if ctx.field_of(owner, name).is_some() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a 'fun' literal cannot capture '{name}': it is a field of '{owner}', and the receiver is a borrow, so its field is not this frame's to move out; bind a `Copy` field to a local first (`val {name}0 = {name}`) and capture that, or take the whole receiver as a 'move' parameter"
                    ),
                    span: *span,
                });
            }
        }
    }
    Err(TypeError {
        help: crate::foreign::hint(name).or_else(|| {
            (ctx.foreign_pds.get(name).is_none() && ctx.init_state.is_none())
                .then(|| "declare it with val or var before this line, or check the spelling.".to_string())
        }),
        message: match ctx.foreign_pds.get(name) {
            Some(module) => format!(
                "'{name}' is a passdown of module '{module}' - a passdown is only visible inside the module that declares it"
            ),
            // Inside a `pd` initializer only passdowns declared
            // *before* it are in scope at all, so the generic name is
            // usually a forward reference.
            None if ctx.init_state.is_some() => format!(
                "undefined variable '{name}' - a 'pd' initializer can only use passdowns declared before it"
            ),
            None => format!("undefined variable '{name}'"),
        },
        span: *span,
    })
}


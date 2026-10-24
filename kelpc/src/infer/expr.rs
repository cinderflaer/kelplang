use super::*;

/// Checks `expr` against an expected type, then applies the one implicit
/// conversion the language has between borrowed and owned text (open item 21).
///
/// This is the wrapper, not the body: putting the conversion *here* means
/// every site that knows an expected type gets it - a call argument, a `val`
/// with a written type, a `return`, a `datac` field, a container element, a
/// `when` arm, a block tail - with no list of sites to keep in sync. The
/// alternative shape (coerce at each site) is the same behaviour with N
/// chances to forget one.
pub(crate) fn check_expr(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    ctx.field_base_here = std::mem::take(&mut ctx.field_base_ok);
    ctx.box_sink_here = std::mem::take(&mut ctx.box_sink_ok);
    // A failed check can be retried with other state by a caller that swallows
    // the error, so what it left in `probes` must not outlive it.
    let mark = ctx.probes.len();
    let value = match check_expr_inner(expr, ctx, expected) {
        Ok(v) => v,
        Err(e) => {
            ctx.probes.truncate(mark);
            return Err(e);
        }
    };
    // A container made inside a block is shared-pool memory, which the block
    // cannot free (section 6, rule 4). A call or a template that yields one is
    // refused by name, and so is a container a call hands back.
    if let (Some((block, _)), Some(kind)) = (ctx.active_blocks.last(), shared_container_kind(&value.ty)) {
        if matches!(expr, ast::Expr::Call { .. } | ast::Expr::MethodCall { .. } | ast::Expr::Template { .. }) {
            return Err(TypeError {
                help: Some(format!("make the {kind} before '{block}' starts, or keep one value in a Box<T> inside the block")),
                message: format!("a {kind} made in block '{block}' is shared-pool memory, so the block cannot free it"),
                span: expr.span(),
            });
        }
    }
    let value = coerce_str_to_string(value, expected);
    Ok(coerce_handle_upcast(value, expected, &ctx.env.handle_parents))
}

/// A handle value used where one of its ancestors is expected converts with no
/// runtime cost (gap 1): the C typedef and the LLVM type are both a plain
/// pointer, so the value is the same bit pattern and only its *type* changes.
/// Only the child-to-ancestor direction fires; a parent used where a child is
/// expected is a type mismatch, and `Parent.cast(child)` is the explicit way to
/// reinterpret one.
pub(crate) fn coerce_handle_upcast(
    value: tir::Expr,
    expected: Option<&Ty>,
    parents: &HashMap<String, String>,
) -> tir::Expr {
    let Some(Ty::Handle(want)) = expected else {
        return value;
    };
    let Ty::Handle(have) = &value.ty else {
        return value;
    };
    if have == want || !handle_is_same_or_ancestor(parents, want, have) {
        return value;
    }
    tir::Expr {
        ty: Ty::Handle(want.clone()),
        ..value
    }
}

/// A `str` used where a `String` is expected becomes one, by `String.from_str`.
///
/// `String.from_str("...")` was the single most repeated construct in the JSON
/// library this came from - once per parser error message, once per `kind()`
/// arm, once per `String.from_str("null")` - because a literal is a `str` and
/// an owned field is a `String`, and nothing bridged the two. The conversion is
/// a copy, which is what an owned value has to be; nothing implicit the other
/// way (`String` to `str`) is added here, because that direction is a
/// *borrow* with a lifetime, and borrowing a value you are about to free is
/// the borrow checker's business, not a conversion's.
///
/// Only an *exact* `String` expectation fires it. A `&String` parameter is
/// left alone (the caller writes `&`-shaped arguments the same way it always
/// did), and a `str` parameter is left alone entirely, so `str.eq(a, b)` and
/// `str.len(s)` keep taking views with no allocation.
pub(crate) fn coerce_str_to_string(value: tir::Expr, expected: Option<&Ty>) -> tir::Expr {
    if !matches!(expected, Some(Ty::String)) {
        return value;
    }
    // A `View` needs a different conversion, not the same one: it may be a
    // *slice*, with no NUL after it, so `String.from_str` (which is `strlen`
    // and a copy) would copy past the slice into the rest of the source. The
    // length-aware copy is the only correct one, and it is `View.to_string`.
    let callee = match &value.ty {
        t if is_str_like(t) => "String#from_str",
        Ty::View => "View#to_string",
        _ => return value,
    };
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::Call {
            callee: callee.to_string(),
            args: vec![value],
        },
        ty: Ty::String,
        span,
    }
}

pub(crate) fn check_expr_inner(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    // A `fun` literal is checked first, with its expected type in hand: a
    // block literal (`{ x -> ... }`) takes its parameters' and its return's
    // types from the expected function type, and a scoped literal needs the
    // expectation to know that borrow captures are allowed
    // (`kelp wiki page language/functions.md`).
    if let ast::Expr::FnLit {
        params,
        infer_params,
        return_type,
        body,
        span,
    } = expr
    {
        let value = check_fn_literal(params, *infer_params, return_type, body, *span, ctx, expected)?;
        if let Some(want) = expected {
            if matches!(want, Ty::Closure { .. }) {
                return Ok(coerce_fn_to_closure(want.clone(), value));
            }
            if matches!(want, Ty::ScopedFn { .. }) {
                return Ok(coerce_fn_to_scoped(want.clone(), value));
            }
        }
        return Ok(value);
    }
    // A plain function value where a closure is expected (A-4 phase E): the
    // one widening in this phase, handled here so that every site which knows
    // the expected type gets it - a call argument, a `val` with a written
    // type, a `return`, a `datac` field, `Box.of(x)`. The value is checked
    // with no expectation of its own (nothing about a `fun` literal depends
    // on the closure type it will be widened into) and then wrapped.
    if let Some(Ty::Closure { .. }) = expected {
        let expected = expected.expect("matched above");
        let value = check_expr(expr, ctx, None)?;
        return Ok(coerce_fn_to_closure(expected.clone(), value));
    }
    // A borrow written where a `&dyn Trait` is expected (A-12 slice 4): the one
    // coercion this slice adds, and the reason it is here is that every site
    // that knows an expected type (a call argument, a typed `val`/`var`, a
    // `return`, a field) reaches the operand through this function. The operand
    // is checked with no expectation, so what is coerced is the plain `&x` it
    // already is.
    if let Some(Ty::Dyn { .. }) = expected {
        let want = expected.expect("matched above").clone();
        let value = check_expr(expr, ctx, None)?;
        return coerce_to_dyn(want, value, expr.span(), ctx);
    }
    // A `Box<T>` where a `Box<dyn Trait>` is expected (A-12 slice 7): the same
    // expected-type position, with the box moved in rather than copied. The
    // operand is checked with no expectation, so what is coerced is the box it
    // already is.
    if let Some(Ty::BoxDyn { .. }) = expected {
        let want = expected.expect("matched above").clone();
        // `Box.empty()` is the one box constructor whose trait only the written
        // type can supply, so it is answered through the ordinary associated
        // path with the expected type in hand (which also keeps a local or an
        // alias named `Box` shadowing the type name, exactly as it does
        // everywhere else). Every other expression is the box the coercion
        // moves in.
        if let ast::Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            span,
        } = expr
        {
            if empty_container_call(expr) == Some(("Box", "empty")) {
                return check_method_call(
                    receiver,
                    method,
                    type_args,
                    args,
                    *span,
                    ctx,
                    Some(&want),
                );
            }
        }
        let value = check_expr(expr, ctx, None)?;
        return coerce_box_to_dyn(want, value, expr.span(), ctx);
    }
    match expr {
        // Handled above, before the closure widening.
        ast::Expr::FnLit { .. } => unreachable!("a 'fun' literal is checked before this match"),
        // H-12: an integer literal where a float is expected is that float
        // (`val w: f64 = 3`, `x * 2` with `x: f64`, `f(255)` for an `f32`
        // parameter), when the float holds it exactly. A *value* never
        // converts; only the literal's own spelling adapts.
        ast::Expr::IntLit(v, span) if expected.is_some_and(|t| t.is_float()) => {
            let ty = expected.expect("checked above").clone();
            let exact = if ty == Ty::F32 { 1u64 << 24 } else { 1u64 << 53 };
            if *v > exact {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "the literal {v} is not exactly representable as '{}': write it as a float ({v}.0) to accept the rounding, or use a smaller number",
                        ty.display()
                    ),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v as f64),
                ty,
                span: *span,
            })
        }
        ast::Expr::IntLit(v, span) => {
            let (ty, defaulted) = int_lit_ty(expected);
            check_int_literal(*v as i128, &ty, defaulted, *span)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v as i128),
                ty,
                span: *span,
            })
        }
        // A character literal (open item 53) takes the type its context asks
        // for, exactly as an integer literal does, and `char` when nothing
        // constrains it. That single rule is what lets byte-level code read
        // the way it thinks: `c == '"'` with `c: u8`, `out.push(' ')` into a
        // `String`, `n + '0'` to make a digit.
        //
        // A code point that does not fit the requested integer type is
        // refused rather than truncated - `'中'` in a `u8` position is a
        // mistake, not a byte to keep.
        ast::Expr::CharLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_integer() => {
                    if !char_fits(*v, t) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "the character '{}' (U+{:04X}) does not fit in '{}'",
                                printable_codepoint(*v),
                                v,
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                    t.clone()
                }
                _ => Ty::Char,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::CharLit(*v),
                ty,
                span: *span,
            })
        }
        ast::Expr::FloatLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_float() => t.clone(),
                _ => Ty::F64,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v),
                ty,
                span: *span,
            })
        }
        // A byte literal (H-11) is a `u8` by construction, unlike a character
        // literal, which takes the integer type its context asks for. That is
        // the whole difference: `b'\n'` needs no type written on the binding,
        // and it is refused (rather than widened) where the context wants
        // something other than a byte, which is what `'\n'` is for.
        ast::Expr::ByteLit(v, span) => {
            if let Some(t) = expected {
                if !matches!(t, Ty::U8) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a byte literal is a 'u8', not a '{}': write it as a character literal when it should take this type",
                            t.display()
                        ),
                        span: *span,
                    });
                }
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v as i128),
                ty: Ty::U8,
                span: *span,
            })
        }
        ast::Expr::StringLit(s, span) => Ok(tir::Expr {
            kind: tir::ExprKind::StringLit(s.clone()),
            ty: Ty::Str,
            span: *span,
        }),
        ast::Expr::BoolLit(b, span) => Ok(tir::Expr {
            kind: tir::ExprKind::BoolLit(*b),
            ty: Ty::Bool,
            span: *span,
        }),
        ast::Expr::Ident(name, span) => check_ident_expr(name, span, ctx, expected),
        ast::Expr::Unary { op, expr: sub, span } => {
            // `-2147483648` fits an `i32` and an `int`, but the literal under the sign
            // is 2147483648, which no `i32` holds on its own. The sign is part of
            // the value when the range is judged, so the pair is checked here
            // and the literal's own check, which cannot see the sign, is
            // skipped. Every other operand is checked below as it always was.
            if *op == UnOp::Neg && !expected.is_some_and(|t| t.is_float()) {
                if let ast::Expr::IntLit(v, lit_span) = &**sub {
                    let (ty, defaulted) = int_lit_ty(expected);
                    check_int_literal(-(*v as i128), &ty, defaulted, *lit_span)?;
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Unary {
                            op: *op,
                            expr: Box::new(tir::Expr {
                                kind: tir::ExprKind::IntLit(*v as i128),
                                ty: ty.clone(),
                                span: *lit_span,
                            }),
                        },
                        ty,
                        span: *span,
                    });
                }
            }
            let sub_expected = if *op == UnOp::Not {
                Some(&Ty::Bool)
            } else if *op == UnOp::BitNot {
                // `~x` is integral-only, and the expectation only helps when
                // it is an integer (an integer literal has no other way to
                // take a width from `~`).
                expected.filter(|t| t.is_integer())
            } else {
                expected
            };
            let sub_t = check_expr(sub, ctx, sub_expected)?;
            match op {
                UnOp::Neg => {
                    if !sub_t.ty.is_numeric() {
                        // `-a` on a user type that implements `std/ops.Neg` is
                        // `a.neg()` (`design/traits.md (kelp wiki)` 13.2), with the operand
                        // borrowed rather than moved.
                        if let Some(owner) =
                            unary_operator_owner(&sub_t, crate::types::NEG_TRAIT, ctx)
                        {
                            return Ok(lower_operator_call(&owner, "neg", &[&sub_t], *span, ctx)?);
                        }
                        return Err(TypeError {
                            help: None,
                            message: format!("cannot negate a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
                UnOp::Not => {
                    if sub_t.ty != Ty::Bool {
                        return Err(TypeError {
                            help: None,
                            message: format!("cannot apply '!' to a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
                UnOp::BitNot => {
                    if !sub_t.ty.is_integer() {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "cannot apply '~' to a value of type '{}' - '~' complements the bits of an integer",
                                sub_t.ty.display()
                            ),
                            span: *span,
                        });
                    }
                }
            }
            let ty = sub_t.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Unary {
                    op: *op,
                    expr: Box::new(sub_t),
                },
                ty,
                span: *span,
            })
        }
        ast::Expr::Binary { op, lhs, rhs, span } => check_binary(*op, lhs, rhs, *span, ctx, expected),
        ast::Expr::Call {
            callee,
            type_args,
            args,
            span,
        } => check_call(callee, type_args, args, *span, ctx, expected),
        ast::Expr::FieldAccess { base, field, span } => check_field_expr(base, field, span, ctx, expected),
        ast::Expr::Assign { target, value, span } => check_assign(target, value, *span, ctx),
        ast::Expr::If {
            cond,
            then_block,
            else_block,
            span,
        } => check_if(cond, then_block, else_block.as_deref(), *span, ctx, expected),
        ast::Expr::Block(block) => {
            let b = check_block(block, ctx, expected)?;
            let ty = b.ty();
            Ok(tir::Expr {
                kind: tir::ExprKind::Block(b),
                ty,
                span: block.span,
            })
        }
        // `unsafe { ... }` (MK9-S2 stage 1): the same block, checked with the
        // unsafe nesting raised by one so the operations that need it are
        // legal inside. Nothing about the lowered block differs, so the two
        // backends need no new shape for it: the call is the unsafe operation.
        // `block name: T { ... }` (manual memory blocks, slice 1): parsed, not
        // built yet. Refused by name until the block runtime lands (step 4).
        ast::Expr::ManualBlock { name, ty, body, span } => check_manual_block(name, ty, body, *span, ctx),
        ast::Expr::Unsafe { block, span } => {
            ctx.unsafe_depth += 1;
            let checked = check_block(block, ctx, expected);
            ctx.unsafe_depth -= 1;
            let b = checked?;
            let ty = b.ty();
            Ok(tir::Expr {
                kind: tir::ExprKind::Block(b),
                ty,
                span: *span,
            })
        }
        // `asm(...)` (kernel stage 5): an unsafe operation in its own right,
        // so it needs the `unsafe` context on top of the module's `[unsafe]`
        // listing (which `ast::first_unsafe` already demanded).
        ast::Expr::Asm {
            template,
            operands,
            clobbers,
            nomem,
            nostack: _,
            span,
        } => check_asm(template, operands, clobbers, *nomem, *span, ctx, expected),
        ast::Expr::Try { expr: sub, span } => check_try(sub, *span, ctx),
        ast::Expr::Ref { mutable, expr: sub, span } => {
            let sub_t = check_expr(sub, ctx, None)?;
            // The address has to outlive the statement the borrow is written
            // in, and a field of a call's result does not: that temporary is
            // staged for one call, not for a binding. `is_durable_place` is the
            // typed form of the question, and the reason an explicit
            // `&headers.get(i).name` is legal where `&mkName().name` is not:
            // the chain steps through a borrow, so the storage is the
            // pointee's (`syntax.md` 10.2).
            if !is_durable_place(&sub_t) {
                return Err(TypeError {
                    help: None,
                    message: "cannot borrow a temporary value".to_string(),
                    span: *span,
                });
            }
            // A parameter of an owned type is already a borrow (`syntax.md`
            // 10.2), so `&o` on one is a borrow of a borrow. Left alone the
            // two references unify badly and the caller sees an inference
            // failure naming a type parameter instead of this rule.
            if let Ty::Ref { .. } = sub_t.ty {
                let written = match sub.as_ref() {
                    ast::Expr::Ident(name, _) => format!("'&{name}'"),
                    _ => "this borrow".to_string(),
                };
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "this is a borrow of a borrow: {written} borrows a value that is already a borrow, and Kelp has no '&&T'; pass the borrow itself"
                    ),
                    span: *span,
                });
            }
            if *mutable {
                ctx.note_self_write(&sub_t)?;
                check_target_writable(&sub_t, ctx)?;
            }
            let ty = Ty::Ref { mutable: *mutable, inner: Box::new(sub_t.ty.clone()) };
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::Deref { expr: sub, span } => {
            let sub_t = check_expr(sub, ctx, None)?;
            let Ty::Ref { inner, .. } = &sub_t.ty else {
                return Err(TypeError {
                    help: None,
                    message: format!("cannot dereference a value of type '{}'", sub_t.ty.display()),
                    span: *span,
                });
            };
            let ty = inner.as_ref().clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Deref { expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::Template { parts, span } => {
            // Outside `print`/`print!` (that path is handled in `check_call`)
            // a template is a `String` value: the text is formatted once into
            // an owned buffer, so `val part = "${out}.part"` binds a String.
            let mut value = check_template(parts, false, *span, ctx)?;
            value.ty = Ty::String;
            Ok(value)
        }
        ast::Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            span,
        } => {
            if let Some(bname) = block_box_receiver(receiver, method, args, ctx) {
                return Err(TypeError {
                    help: Some(format!("bind it with `let x = {bname}.box()?` inside the block")),
                    message: format!("a block box comes out of '{bname}.box()' with '?', and nowhere else"),
                    span: *span,
                });
            }
            // `circle.make(...)`: a module-qualified reference to an
            // imported item, not a method call. Recognized only when the
            // receiver is a bare identifier that names an `impl` alias and
            // is *not* shadowed by a local (locals win, exactly like any
            // other name lookup); anything else is a real method call.
            if let ast::Expr::Ident(name, _) = receiver.as_ref() {
                if is_module_alias(name, ctx) {
                    if let Some(prefix) = ctx.aliases.get(name).cloned() {
                        return check_module_item(
                            name,
                            &prefix,
                            method,
                            type_args,
                            args,
                            *span,
                            ctx,
                            expected,
                        );
                    }
                }
            }
            // A cross-module type path (`shapes.Shape.Circle(...)`) is gated
            // before it is read as a variant construction.
            if let Some(path) = expr_path(receiver) {
                ctx.check_type_path_exported(&path, *span)?;
            }
            // `Option.Some(v = 1)` - construction of a generic `enum`
            // variant (A-2), before the concrete-enum check because a generic
            // enum has no entry in the enum table.
            if let Some(tmpl_name) = generic_type_path(receiver, ctx) {
                let explicit = resolve_explicit_args(type_args, ctx)?;
                return check_generic_enum_construction(
                    &tmpl_name,
                    method,
                    &explicit,
                    args,
                    *span,
                    ctx,
                    expected,
                );
            }
            // `Shape.Circle(r = 1.0)` - construction of an `enum` variant
            // (A-1). Checked before method resolution because it is the same
            // `Type.member(...)` shape as a method call, and the receiver is
            // a *type*, not a value. A member that is not a variant falls
            // through to a static trait method (A-12 slice 6).
            if let Some(ename) = enum_path(receiver, ctx) {
                let is_variant = method == "from_int"
                    || ctx
                        .enum_def(&ename)
                        .map(|e| e.variant(method).is_some())
                        .unwrap_or(false);
                if !is_variant {
                    if let Some(e) =
                        check_static_method_call(&ename, method, args, *span, ctx, expected)?
                    {
                        return Ok(e);
                    }
                }
                return check_enum_construction(&ename, method, args, *span, ctx);
            }
            if !type_args.is_empty() && !ptr_receiver(receiver, ctx) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{method}' does not take type arguments - a method's type parameters come from its receiver"
                    ),
                    span: *span,
                });
            }
            check_method_call_enum_guard(receiver, method, *span, ctx)?;
            // `cell.get()` on a let-bound block box (design section 5): the
            // box is read through its own method, which reads the element and
            // leaves the box in its block. Any other method on the box is refused.
            if let ast::Expr::Ident(bname, _) = receiver.as_ref() {
                if method == "get" && args.is_empty() && ctx.box_bindings.contains_key(bname) {
                    ctx.field_base_ok = true;
                    let got = check_method_call(receiver, method, type_args, args, *span, ctx, expected);
                    ctx.field_base_ok = false;
                    return got;
                }
            }
            check_method_call(receiver, method, type_args, args, *span, ctx, expected)
        }
        ast::Expr::When {
            scrutinee,
            arms,
            span,
            let_else,
        } => {
            if matches!(scrutinee.as_ref(), ast::Expr::ManualBlock { .. }) {
                ctx.block_result_used = true;
            }
            check_when_scrutinee(scrutinee, arms, *span, *let_else, ctx, expected)
        }
        // `[a, b, c]`: an array literal (A-18). Every element takes the
        // array's element type, which is either the one the context named or,
        // when nothing constrains it, the first element's own type.
        ast::Expr::ArrayLit { elems, span } => {
            let hinted = match expected {
                Some(Ty::Array(elem, len)) => {
                    if *len as usize != elems.len() {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "an array of {len} element(s) is expected here, and this literal has {}",
                                elems.len()
                            ),
                            span: *span,
                        });
                    }
                    Some((**elem).clone())
                }
                _ => None,
            };
            let mut checked: Vec<tir::Expr> = Vec::with_capacity(elems.len());
            let mut elem_ty = hinted;
            for (i, e) in elems.iter().enumerate() {
                // The first element's own type is what the rest are checked
                // against, which is the same rule `2` in `1 + 2` follows: a
                // literal takes the type of the whole it is part of, and the
                // first element is what defines that whole.
                let v = check_expr(e, ctx, elem_ty.as_ref())?;
                match &elem_ty {
                    None => elem_ty = Some(v.ty.clone()),
                    Some(t) if *t != v.ty => {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "the elements of an array have different types: element {i} is a '{}', and the first element is a '{}'",
                                v.ty.display(),
                                t.display()
                            ),
                            span: e.span(),
                        })
                    }
                    _ => {}
                }
                checked.push(v);
            }
            let elem_ty = elem_ty.expect("an array literal has at least one element");
            note_heap_use(&ctx.env, managed::HeapSlot::ArrayElement, &elem_ty, *span);
            if !crate::types::is_array_element(&elem_ty) {
                return Err(TypeError {
                    help: None,
                    message: array_element_error(&elem_ty),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::ArrayLit(checked),
                ty: Ty::Array(Box::new(elem_ty), elems.len() as u64),
                span: *span,
            })
        }
        // `(lo, hi)`: a tuple value (MISSING-z2bB9, `design/syntax.md (kelp wiki)` 3). Each
        // element takes its own type, from the expected tuple's position when
        // the context named one and from the element expression otherwise. The
        // value is a `StructLit` of the derived record, whose fields are the
        // positions, so codegen needs no tuple case and the elements are
        // moved into the value exactly as a `datac` construction moves its
        // arguments in.
        ast::Expr::TupleLit { elems, span } => {
            let hinted = match expected {
                Some(t) => tuple_elems_of(t, ctx),
                None => None,
            };
            if let Some(expected_elems) = &hinted {
                if expected_elems.len() != elems.len() {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a tuple of {} element(s) is expected here, and this one has {}",
                            expected_elems.len(),
                            elems.len()
                        ),
                        span: *span,
                    });
                }
            }
            let mut checked: Vec<tir::Expr> = Vec::with_capacity(elems.len());
            let mut elem_tys: Vec<Ty> = Vec::with_capacity(elems.len());
            for (i, e) in elems.iter().enumerate() {
                let hint = hinted.as_ref().map(|h| &h[i]);
                let v = check_expr(e, ctx, hint)?;
                if crate::types::is_borrowed_field_ty(&v.ty) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "element {} of the tuple is a borrowed type ('{}'): a tuple holds owned data only in v1, exactly as a datac field does, so a '&str', 'View' or reference element is not expressible; an owned 'String' is the way to keep text in one",
                            i + 1,
                            v.ty.display()
                        ),
                        span: e.span(),
                    });
                }
                if v.ty == Ty::Unit {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "element {} of the tuple produces no value ('unit'): 'unit' is the absent value and cannot be stored, exactly as a datac field cannot be",
                            i + 1
                        ),
                        span: e.span(),
                    });
                }
                elem_tys.push(v.ty.clone());
                checked.push(v);
            }
            let ty = tuple_ty(&ctx.env, elem_tys);
            let name = match &ty {
                Ty::Struct(n) => n.clone(),
                _ => unreachable!("tuple_ty builds a record"),
            };
            let fields = checked
                .into_iter()
                .enumerate()
                .map(|(i, v)| (crate::types::tuple_field_name(i), v))
                .collect();
            Ok(tir::Expr {
                kind: tir::ExprKind::StructLit { name, fields, borrowed_fields: Vec::new() },
                ty,
                span: *span,
            })
        }
        // `[v; N]`: the repeat form (A-18). `N` is folded here exactly as an
        // array type's length is (A-19), and the element has to be `Copy`,
        // because the alternative is cloning an owned value once per element.
        ast::Expr::ArrayRepeat { value, len, span } => {
            let n = ctx.type_scope().array_len(len)?;
            let hinted = match expected {
                Some(Ty::Array(elem, expected_len)) => {
                    if *expected_len != n {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "an array of {expected_len} element(s) is expected here, and this repeat produces {n}"
                            ),
                            span: *span,
                        });
                    }
                    Some((**elem).clone())
                }
                _ => None,
            };
            let v = check_expr(value, ctx, hinted.as_ref())?;
            let elem_ty = v.ty.clone();
            // The repeat form *copies* its value into every slot, so the
            // element has to own nothing: a scalar, or a `datac` whose fields
            // are all scalars. An element that owns memory would need N
            // clones, and a copy of an owner is a second owner. A tuple is a
            // derived record, so `needs_drop` cannot answer for one from the
            // declared set and its positions are walked instead.
            if elem_owns_memory(ctx, &elem_ty) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "the repeat form '[x; N]' needs an element that owns nothing, and '{}' owns memory. Copying it into {n} slots would make {n} owners of the same buffer; write the elements out ('[a, b, c]') instead",
                        elem_ty.display()
                    ),
                    span: *span,
                });
            }
            note_heap_use(&ctx.env, managed::HeapSlot::ArrayElement, &elem_ty, *span);
            if !crate::types::is_array_element(&elem_ty) {
                return Err(TypeError {
                    help: None,
                    message: array_element_error(&elem_ty),
                    span: *span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::ArrayRepeat { value: Box::new(v) },
                ty: Ty::Array(Box::new(elem_ty), n),
                span: *span,
            })
        }
    }
}

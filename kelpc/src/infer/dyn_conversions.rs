use super::*;

/// A method call through a borrowed trait object (A-12 slice 4,
/// `design/traits.md (kelp wiki)` 9.4): resolve the method in the trait's flattened list, check
/// the receiver's mutability against the method's, check the arguments against
/// the trait's own signature, and lower to a `tir::ExprKind::DynCall`.
pub(crate) fn check_dyn_method_call(
    receiver: tir::Expr,
    trait_name: &str,
    mutable: bool,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let (slot, decl) = dyn_method_slot(&ctx.env.traits, trait_name, method, span)?;
    let receiver_mut = decl.receiver.as_ref().is_some_and(|r| r.mutable);
    // The declaration is cloned so the borrow of the trait table ends before
    // the call lowerer needs `ctx` mutably; a method declaration is small and
    // this is one call site.
    let decl = decl.clone();
    if receiver_mut && !mutable {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{trait_name}.{method}' takes an exclusive receiver, and a shared '&dyn {trait_name}' cannot call it; make the receiver '&mut dyn {trait_name}'"
            ),
            span,
        });
    }
    lower_dyn_call(receiver, trait_name, &decl, slot, args, span, false, ctx)
}

/// A method call on an owned trait object (A-12 slice 7). The two names the box
/// itself provides are `is_empty` and `get`; every other name is the trait's
/// method, reached through the box's vtable exactly as it is through a `&dyn`,
/// with one difference: a `mut Self` method needs a writable place, because the
/// box owns what it points at rather than borrowing it.
pub(crate) fn check_box_dyn_method_call(
    receiver: tir::Expr,
    trait_name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    // A `&Box<dyn Trait>` (a `mut Box<dyn Trait>` parameter's borrow included)
    // reaches the pair through the borrow, exactly as `*p` does.
    let (recv, borrowed_mut) = match receiver.ty.clone() {
        Ty::Ref { mutable, inner } if matches!(*inner, Ty::BoxDyn { .. }) => {
            let pair = tir::Expr {
                kind: tir::ExprKind::Deref {
                    expr: Box::new(receiver),
                },
                ty: *inner,
                span,
            };
            (pair, Some(mutable))
        }
        _ => (receiver, None),
    };
    if method == "is_empty" {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'is_empty' takes no arguments, found {}", args.len()),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::BoxDynIsEmpty {
                expr: Box::new(recv),
            },
            ty: Ty::Bool,
            span,
        });
    }
    if method == "get" {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'get' takes no arguments, found {}", args.len()),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::BoxDynBorrow {
                mutable: false,
                expr: Box::new(recv),
            },
            ty: Ty::Dyn {
                mutable: false,
                trait_name: trait_name.to_string(),
            },
            span,
        });
    }
    // `take`/`set` would have to move a sized value in or out of the box, and
    // the concrete type behind an owned trait object is exactly what is erased,
    // so both are refused by name with the spelling that works instead.
    if method == "take" || method == "set" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'Box<dyn {trait_name}>.{method}' is not allowed: the concrete type behind an owned trait object is erased, so no sized value can be moved in or out; borrow the trait with 'get()' and call its methods, or keep the boxed type concrete"
            ),
            span,
        });
    }
    let (slot, decl) = dyn_method_slot(&ctx.env.traits, trait_name, method, span)?;
    let receiver_mut = decl.receiver.as_ref().is_some_and(|r| r.mutable);
    let decl = decl.clone();
    if receiver_mut {
        match borrowed_mut {
            Some(true) => {}
            Some(false) => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{trait_name}.{method}' takes an exclusive receiver, and a shared '&Box<dyn {trait_name}>' cannot call it; take the box by 'mut' or make the binding writable"
                    ),
                    span,
                })
            }
            None => check_target_writable(&recv, ctx)?,
        }
    }
    lower_dyn_call(recv, trait_name, &decl, slot, args, span, receiver_mut, ctx)
}

/// The vtable slot and declaration of one trait method, or the refusal that
/// names the methods the trait does have.
pub(crate) fn dyn_method_slot<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    trait_name: &str,
    method: &str,
    span: Span,
) -> Result<(usize, &'a ast::FunctionDecl)> {
    let mut flat = Vec::new();
    flattened_trait_methods(traits, trait_name, &mut flat);
    let Some((slot, decl)) = flat.iter().enumerate().find(|(_, m)| m.name == method) else {
        let names: Vec<&str> = flat.iter().map(|m| m.name.as_str()).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'dyn {trait_name}' has no method '{method}'; its methods are {}",
                names.join(", ")
            ),
            span,
        });
    };
    Ok((slot, decl))
}

/// The shared tail of both trait-object method calls: check the arguments
/// against the trait method's own signature and lower to a `DynCall`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_dyn_call(
    receiver: tir::Expr,
    trait_name: &str,
    decl: &ast::FunctionDecl,
    slot: usize,
    args: &[ast::CallArg],
    span: Span,
    exclusive: bool,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    // The argument types come from the trait declaration, resolved in the
    // trait's own module. `Self` has no concrete meaning here, but object
    // safety already refused every method that mentions it outside the
    // receiver, so a placeholder is never observed; `self_ty` only reaches
    // the receiver's own type, which this drops.
    let info = ctx
        .env
        .traits
        .get(trait_name)
        .expect("the receiver's own trait is in the table");
    let sig = resolve_trait_method_sig(
        info,
        decl,
        &Ty::Unit,
        &[],
        &ctx.structs,
        &ctx.enums,
        &ctx.env,
        &ctx.consts,
        &ctx.const_fns,
    )?;
    let params: Vec<(String, Ty)> = sig.params[1..].to_vec();
    let checked = match_args(
        &format!("method '{trait_name}.{}'", decl.name),
        &params,
        args,
        span,
        ctx,
    )?;
    let ret = sig.ret;
    Ok(tir::Expr {
        kind: tir::ExprKind::DynCall {
            receiver: Box::new(receiver),
            slot,
            args: checked,
            exclusive,
        },
        ty: ret,
        span,
    })
}

/// The builtin an everyday numeric name stands for, on an integer receiver
/// (issue #279): `to_byte` is `to_u8` and `try_to_byte` is `try_to_u8`. The
/// two spellings share the builtin's entry, its lowering and its range checks.
/// Only an integer receiver takes the alias, so a user method of the same name
/// on a `datac` is left alone.
pub(crate) fn numeric_alias(receiver: &Ty, method: &str) -> Option<&'static str> {
    let src = Ty::from_name(builtin_type_name(receiver)?)?;
    if !src.is_integer() {
        return None;
    }
    match method {
        "to_byte" => Some("to_u8"),
        "try_to_byte" => Some("try_to_u8"),
        _ => None,
    }
}

/// The `(source, target)` pair a `try_to_<T>` method call names, or `None`
/// when the receiver and the method do not spell one (MISSING-RbhdW).
///
/// Only the pairs whose plain `to_<T>` twin can lose a value have a twin at
/// all: a widening cannot fail, so `u8.try_to_u64()` is deliberately not a
/// method. `Ty::int_bounds` is the one place that decides, and it is the same
/// bound table the removed C backend uses to decide whether `to_<T>` needs a check, so
/// the two can never disagree about which conversions are fallible.
pub(crate) fn try_to_pair(receiver: &Ty, method: &str) -> Option<(Ty, Ty)> {
    let src = Ty::from_name(builtin_type_name(receiver)?)?;
    let dst = Ty::from_name(method.strip_prefix("try_to_")?)?;
    if !src.is_integer() || !dst.is_integer() {
        return None;
    }
    let (s_min, s_max) = Ty::int_bounds(&src)?;
    let (t_min, t_max) = Ty::int_bounds(&dst)?;
    if s_min >= t_min && s_max <= t_max {
        return None;
    }
    Some((src, dst))
}

/// Lowers `n.try_to_u8()` (MISSING-RbhdW) into the shape the language can
/// already express: the receiver bound once, a test for each bound its source
/// range cannot rule out, and one `Option` arm per outcome. Deliberately not a
/// new `tir::ExprKind`: both backends already lower `Block`, `If` and `EnumLit`,
/// so the fallible conversion is one pass's work with no conversion-specific
/// code in either code generator.
///
/// The `Option` is the stdlib's own, found by the module id rather than by the
/// enum's name, because a builtin call site has no declaration the resolver
/// could have qualified the type at. A program with no `std/option` anywhere in
/// its graph gets told to import it rather than an internal error.
pub(crate) fn lower_try_to(src: &Ty, dst: &Ty, value: tir::Expr, span: Span, ctx: &Ctx) -> Result<tir::Expr> {
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}.try_to_{}' hands back 'Option<{}>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                src.display(),
                dst.display(),
                dst.display(),
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    let opt_ty = ctx.instantiate_type(&option_key, &[dst.clone()], span)?;
    let opt_name = struct_name_of(&opt_ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&opt_name).expect("just instantiated");
    // Recognized by variant name and arity, never by the enum's own name, the
    // same rule `?` uses (see `try_shape`).
    let Some(shape) = try_shape(&edef).filter(|s| !s.err_has_payload) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' declares '{option_key}', and it is not the two-variant 'Some'/'None' shape this conversion hands back",
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    };
    let src_name = builtin_type_name(src).expect("an integer has a name");
    let dst_name = builtin_type_name(dst).expect("an integer has a name");
    let (s_min, s_max) = Ty::int_bounds(src).expect("a try_to_ source is an integer");
    let (t_min, t_max) = Ty::int_bounds(dst).expect("a try_to_ target is an integer");
    // The receiver is read once, into a binding of its own: the test and the
    // payload both read it, so `f().try_to_u8()` must not run `f` twice. The
    // name carries a `.`, which no source identifier can contain, so it cannot
    // shadow a binding the program wrote and no program can shadow it.
    let held = format!("__try_to.{}", span.start);
    let read = || tir::Expr {
        kind: tir::ExprKind::Ident(held.clone()),
        ty: src.clone(),
        span,
    };
    let lit = |v: i128| tir::Expr {
        kind: tir::ExprKind::IntLit(v),
        ty: src.clone(),
        span,
    };
    let compare = |op: BinOp, rhs: i128| tir::Expr {
        kind: tir::ExprKind::Binary {
            op,
            lhs: Box::new(read()),
            rhs: Box::new(lit(rhs)),
        },
        ty: Ty::Bool,
        span,
    };
    // Only the bounds the source's own range cannot rule out, exactly as
    // `numeric_conversion` drops them: `u8.try_to_i32` has no upper test to
    // write, and a test that cannot fail would be a tautology in the C.
    let mut checks: Option<tir::Expr> = None;
    for check in [
        (s_min < t_min).then(|| compare(BinOp::Lt, t_min)),
        (s_max > t_max).then(|| compare(BinOp::Gt, t_max as i128)),
    ]
    .into_iter()
    .flatten()
    {
        checks = Some(match checks.take() {
            None => check,
            Some(prev) => tir::Expr {
                kind: tir::ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(prev),
                    rhs: Box::new(check),
                },
                ty: Ty::Bool,
                span,
            },
        });
    }
    let cond = checks.expect("a try_to_ pair fails at least one bound");
    let variant = |variant: usize, fields: Vec<tir::Expr>| tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: opt_name.clone(),
            variant,
            fields,
        },
        ty: opt_ty.clone(),
        span,
    };
    let none = variant(shape.err, Vec::new());
    let some = variant(
        shape.ok,
        vec![tir::Expr {
            kind: tir::ExprKind::Call {
                callee: format!("{src_name}#to_{dst_name}"),
                args: vec![read()],
            },
            ty: dst.clone(),
            span,
        }],
    );
    // An out-of-range pair is `None`, and the `Some` arm is the unchecked
    // payload: the conversion inside it is the plain `to_<T>`, whose own range
    // check the test above has already made unreachable.
    let arms = tir::Expr {
        kind: tir::ExprKind::If {
            cond: Box::new(cond),
            then_block: tir::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(none)),
            },
            else_block: Some(tir::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(some)),
            }),
        },
        ty: opt_ty.clone(),
        span,
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: held,
                ty: src.clone(),
                init: value,
            }],
            tail: Some(Box::new(arms)),
        }),
        ty: opt_ty,
        span,
    })
}

/// `m.to_int()` (kernel stage 2): the declared discriminant of the variant
/// the value holds, as the enum's `repr(<int>)` type (default `i32`). Lowered
/// to the shape `when` already lowers to: a `EnumTag` test per variant, each
/// arm an integer literal, with the receiver read once through a hidden local,
/// so `f().to_int()` calls `f` once. Only a field-less enum has declared
/// values to report.
pub(crate) fn lower_enum_to_int(
    mode_ty: Ty,
    receiver: &ast::Expr,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let ename = struct_name_of(&mode_ty)
        .expect("an enum receiver names a struct type")
        .to_string();
    let edef = ctx
        .enum_def(&ename)
        .expect("the receiver was checked against a declared enum");
    if !edef.is_field_less() {
        return Err(TypeError {
            help: None,
            message: format!(
                "enum '{ename}' has a variant with a payload, so it has no declared integer value: 'to_int' is on a field-less enum"
            ),
            span,
        });
    }
    let tag = edef.tag_ty();
    // The receiver is read through its address for each tag test. A place has
    // one already and is read repeatedly, which is what an `EnumTag` does; an
    // rvalue (`E.A`, a call) is moved into a local of its own first, because a
    // borrow of a bare rvalue does not outlive the statement that made it.
    let probe = check_expr(receiver, ctx, Some(&mode_ty))?;
    let (base, stage) = if is_durable_place(&probe) {
        (probe, None)
    } else {
        // The name carries a `.`, which no source identifier can contain, so
        // it cannot shadow a binding the program wrote.
        let held = format!("__to_int.{}", span.start);
        (
            tir::Expr {
                kind: tir::ExprKind::Ident(held.clone()),
                ty: mode_ty.clone(),
                span,
            },
            Some(tir::Stmt::Let {
                name: held,
                ty: mode_ty.clone(),
                init: probe,
            }),
        )
    };
    let values = edef.discriminants();
    let int = |v: i128| tir::Expr {
        kind: tir::ExprKind::IntLit(v),
        ty: tag.clone(),
        span,
    };
    let last = values.len() - 1;
    let mut chain = int(values[last]);
    for i in (0..last).rev() {
        chain = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(tir::Expr {
                    kind: tir::ExprKind::EnumTag {
                        expr: Box::new(base.clone()),
                        variant: i,
                    },
                    ty: Ty::Bool,
                    span,
                }),
                then_block: tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(int(values[i]))),
                },
                else_block: Some(tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(chain)),
                }),
            },
            ty: tag.clone(),
            span,
        };
    }
    let stmts: Vec<tir::Stmt> = stage.into_iter().collect();
    if stmts.is_empty() {
        return Ok(chain);
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts,
            tail: Some(Box::new(chain)),
        }),
        ty: tag,
        span,
    })
}

/// `Mode.from_int(n)` (kernel stage 2): the variant whose discriminant is `n`,
/// or `None`. Lowered like [`lower_try_to`]: the argument is bound once into a
/// hidden local and the result is one `if n == value { Some(variant) }` arm
/// per variant, falling through to `None`. Requires the `Option` the
/// stdlib declares, exactly as the fallible conversions do.
pub(crate) fn lower_enum_from_int(
    ename: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let edef = ctx
        .enum_def(ename)
        .expect("enum_path only returns known enums");
    if !edef.is_field_less() {
        return Err(TypeError {
            help: None,
            message: format!(
                "enum '{ename}' has a variant with a payload, so it has no declared integer value: 'from_int' is on a field-less enum"
            ),
            span,
        });
    }
    let tag = edef.tag_ty();
    let checked = match_args(
        &format!("'{ename}.from_int'"),
        &[("n".to_string(), tag.clone())],
        args,
        span,
        ctx,
    )?;
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{ename}.from_int' hands back 'Option<{ename}>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    let mode_ty = Ty::Struct(ename.to_string());
    let opt_ty = ctx.instantiate_type(&option_key, &[mode_ty.clone()], span)?;
    let opt_name = struct_name_of(&opt_ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let odef = ctx.enum_def(&opt_name).expect("just instantiated");
    let Some(shape) = try_shape(&odef).filter(|s| !s.err_has_payload) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' declares '{option_key}', and it is not the two-variant 'Some'/'None' shape this conversion hands back",
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    };
    let held = format!("__from_int.{}", span.start);
    let read = || tir::Expr {
        kind: tir::ExprKind::Ident(held.clone()),
        ty: tag.clone(),
        span,
    };
    let variant = |variant: usize, fields: Vec<tir::Expr>| tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: opt_name.clone(),
            variant,
            fields,
        },
        ty: opt_ty.clone(),
        span,
    };
    let none = variant(shape.err, Vec::new());
    let values = edef.discriminants();
    let mut chain = none;
    for i in (0..values.len()).rev() {
        let arm = variant(
            shape.ok,
            vec![tir::Expr {
                kind: tir::ExprKind::EnumLit {
                    enum_name: ename.to_string(),
                    variant: i,
                    fields: Vec::new(),
                },
                ty: mode_ty.clone(),
                span,
            }],
        );
        chain = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(tir::Expr {
                    kind: tir::ExprKind::Binary {
                        op: BinOp::Eq,
                        lhs: Box::new(read()),
                        rhs: Box::new(tir::Expr {
                            kind: tir::ExprKind::IntLit(values[i]),
                            ty: tag.clone(),
                            span,
                        }),
                    },
                    ty: Ty::Bool,
                    span,
                }),
                then_block: tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(arm)),
                },
                else_block: Some(tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(chain)),
                }),
            },
            ty: opt_ty.clone(),
            span,
        };
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: held,
                ty: tag,
                init: checked[0].clone(),
            }],
            tail: Some(Box::new(chain)),
        }),
        ty: opt_ty,
        span,
    })
}

/// The associated constructors of the three builtin containers, written on
/// the type name: `List.of(x)`/`List.empty()`, `Box.of(x)`/`Box.empty()`,
/// and `Map.new()`/`Map.empty()`/`Map.with_capacity(n)`. Each needs its
/// element type from somewhere - `of` takes it from its argument (or from
/// the binding's written type, which is what `expected` is for),
/// `empty`/`new` only from the written type, and `with_capacity` from the
/// written type as well because its argument is a count, not an element.
pub(crate) fn check_container_associated(
    container: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Box.empty()` on an owned trait object (A-12 slice 7): the null fat pair,
    // whose trait only the binding's written type can supply. There is no
    // element type to take it from, so it is answered here rather than through
    // the element machinery below.
    if container == "Box" {
        if let Some(want @ Ty::BoxDyn { .. }) = expected {
            if method != "empty" {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Box.{method}' is not an operation of an owned trait object: '{}' is made from 'Box.of(v)' where 'v' implements the trait (or 'Box.empty()'), and used through its methods",
                        want.display()
                    ),
                    span,
                });
            }
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'Box.empty' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "BoxDyn#empty".to_string(),
                    args: Vec::new(),
                },
                ty: want.clone(),
                span,
            });
        }
    }
    let expected_elem = expected
        .and_then(|t| match container {
            "List" => crate::types::list_elem_of(t),
            "Map" => crate::types::map_elem_of(t),
            _ => crate::types::box_elem_of(t),
        })
        .cloned();
    // A `Map.new()`/`Map.empty()` also needs its *key* type, which only the
    // binding's written type can supply; `Map<V>` written there means a
    // `String` key, which is what `map_key_of` already reports.
    let expected_key = (container == "Map")
        .then(|| expected.and_then(crate::types::map_key_of).cloned())
        .flatten();
    let wrap = |elem: Ty| match container {
        "List" => Ty::List(Box::new(elem)),
        "Map" => Ty::Map(
            Box::new(expected_key.clone().unwrap_or(Ty::String)),
            Box::new(elem),
        ),
        _ => Ty::Box(Box::new(elem)),
    };
    // What a bare `List.empty()`/`Box.empty()`/`Map.new()` with no type to
    // take its element from becomes: the container with `Unit` where the
    // element goes, which no written type can spell, so a later `push` on the
    // binding can fill it in (`types::is_open_container`). A map's key is
    // open the same way when the binding named neither.
    let open = || match container {
        "List" => Ty::List(Box::new(Ty::Unit)),
        "Map" => Ty::Map(Box::new(Ty::Unit), Box::new(Ty::Unit)),
        _ => Ty::Box(Box::new(Ty::Unit)),
    };
    // `Map.empty()` and `Map.new()` are one operation - a map starts empty -
    // so they lower to one key. They used to lower to two and codegen only
    // ever emitted `__new`, so `Map.empty()` compiled to a call to a function
    // that did not exist; a `datac`/`enum` value type is not what exposed it,
    // just what asked for it first.
    let key = if container == "Map" && method == "empty" {
        "Map#new".to_string()
    } else {
        format!("{container}#{method}")
    };
    match method {
        "new" if container == "Map" => {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'Map.new' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let Some(value) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    help: None,
                    message: "cannot tell what 'Map.new()' holds, write the type on the binding ('var m: Map<String, i32> = Map.new()'); there is no 'Map.of(x)' since a map starts empty".to_string(),
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![] },
                ty: wrap(value),
                span,
            })
        }
        // `Map.with_capacity(n)` is `Map.new()` that sizes both buffers up
        // front, which is the only way a caller can pay the growth once: a
        // map that fills three million entries otherwise rebuilds its index a
        // dozen times on the way there. The count is a hint, never a fence:
        // the map grows past it like any other.
        "with_capacity" if container == "Map" || container == "List" => {
            if args.len() != 1 {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{container}.with_capacity' takes exactly 1 argument (the {}), found {}",
                        if container == "Map" { "entry count" } else { "element count" },
                        args.len()
                    ),
                    span,
                });
            }
            let cap = check_expr(&args[0].value, ctx, Some(&Ty::Int))?;
            if !matches!(cap.ty, Ty::Int) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{container}.with_capacity' needs an 'int' count, found '{}'",
                        cap.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            let Some(value) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![cap] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    help: None,
                    message: match container {
                        "Map" => "cannot tell what 'Map.with_capacity(n)' holds, write the type on the binding ('var m: Map<String, i32> = Map.with_capacity(n)')".to_string(),
                        _ => "cannot tell what 'List.with_capacity(n)' holds, write the element type on the binding ('var xs: List<i32> = List.with_capacity(n)')".to_string(),
                    },
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![cap] },
                ty: wrap(value),
                span,
            })
        }
        "empty" => {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'{container}.empty' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let Some(elem) = expected_elem else {
                if ctx.defer_empty_container {
                    return Ok(tir::Expr {
                        kind: tir::ExprKind::Call { callee: key, args: vec![] },
                        ty: open(),
                        span,
                    });
                }
                return Err(TypeError {
                    help: None,
                    message: match container {
                        "Map" => "cannot tell what 'Map.empty()' holds, write the type on the binding ('var m: Map<String, i32> = Map.empty()')".to_string(),
                        _ => format!(
                            "cannot tell what '{container}.empty()' holds, write the element type on the binding ('var xs: {container}<String> = {container}.empty()') or start from '{container}.of(x)'"
                        ),
                    },
                    span,
                });
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![] },
                ty: wrap(elem),
                span,
            })
        }
        "of" if container == "Map" => Err(TypeError {
            help: None,
            message: "'Map.of' does not exist - a map starts empty; write 'Map.new()' with the value type on the binding, then 'insert(k, v)'".to_string(),
            span,
        }),
        "of" => {
            if args.len() != 1 {
                return Err(TypeError {
                    help: None,
                    message: format!("'{container}.of' takes exactly 1 argument, found {}", args.len()),
                    span,
                });
            }
            let value = check_expr(&args[0].value, ctx, expected_elem.as_ref())?;
            let slot = match container {
                "List" => managed::HeapSlot::ListElement,
                "Map" => managed::HeapSlot::MapValue,
                _ => managed::HeapSlot::BoxPayload,
            };
            note_heap_use(&ctx.env, slot, &value.ty, args[0].value.span());
            let allowed = if container == "List" {
                crate::types::is_list_element(&value.ty)
            } else if container == "Map" {
                crate::types::is_map_element(&value.ty) && !ctx.is_enum_ty(&value.ty)
            } else {
                crate::types::is_box_element(&value.ty)
            };
            if !allowed {
                return Err(TypeError {
                    help: None,
                    message: if container == "List" {
                        list_element_error(&value.ty)
                    } else if container == "Map" && ctx.is_enum_ty(&value.ty) {
                        map_enum_value_error(&value.ty)
                    } else if container == "Map" {
                        map_element_error(&value.ty)
                    } else {
                        box_element_error(&value.ty)
                    },
                    span: args[0].value.span(),
                });
            }
            let elem = value.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: vec![value] },
                ty: wrap(elem),
                span,
            })
        }
        other => Err(TypeError {
            help: None,
            message: format!(
                "'{container}.{other}' is not a {container} operation - {}",
                if container == "List" {
                    "write 'List.of(x)' for a one-element list, 'List.empty()' or 'List.with_capacity(n)' with the element type on the binding, or call a method on a value ('xs.push(x)', 'xs.get(i)', 'xs.len()')"
                } else if container == "Map" {
                    "a map starts empty - write 'Map.new()' with the type on the binding ('var m: Map<String, i32> = Map.new()'), then call a method on the value ('m.insert(k, v)', 'm.get(k)', 'm.len()')"
                } else {
                    "write 'Box.of(x)' for a one-value box, 'Box.empty()' with the element type on the binding, or call a method on a value ('b.get()', 'b.set(v)', 'b.take()', 'b.is_empty()')"
                }
            ),
            span,
        }),
    }
}

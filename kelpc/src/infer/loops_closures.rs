use super::*;

/// A binding is the one expression context that may keep an owned temporary
/// alive for a borrow it receives. The first slice covers the two text views;
/// other borrow-returning APIs can join this lowering without changing its
/// lifetime or drop model.
pub(crate) fn binding_borrow_temporary(init: &ast::Expr, next: usize) -> Option<(&ast::Expr, ast::Expr)> {
    let ast::Expr::MethodCall { receiver, method, type_args, args, span } = init else {
        return None;
    };
    if !matches!(method.as_str(), "as_str" | "view") {
        return None;
    }
    let root = ast::temporary_root(receiver)?;
    next.checked_add(1)?;
    let tmp = format!("{}{}", BORROW_TEMP_PREFIX, next);
    Some((
        root,
        ast::Expr::MethodCall {
            receiver: Box::new(ast::rebase_root(receiver, &ast::Expr::Ident(tmp, root.span()))),
            method: method.clone(),
            type_args: type_args.clone(),
            args: args.clone(),
            span: *span,
        },
    ))
}

/// The canonical key of `std.iter.Iterator`, the one trait a `for` accepts
/// (`kelp wiki page language/iterators.md`). A project spells it `impl "std/iter"` and reaches the
/// trait as `iter.Iterator`; the canonical key is what the impl table is keyed
/// by.
pub(crate) const ITERATOR_TRAIT_KEY: &str = crate::stdlib::ITERATOR_TRAIT;

/// Whether a `for` may walk this type: it implements `std.iter.Iterator`,
/// possibly through a supertrait.
pub(crate) fn implements_iterator(ctx: &Ctx, ty: &Ty) -> bool {
    let name = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()));
    if name.as_ref().map_or(false, |n| implements_trait(&ctx.env, ITERATOR_TRAIT_KEY, n)) {
        return true;
    }
    // A generic instantiation cannot take a trait impl (the parser reads the
    // `for` type as a bare name), so a shipped generic `datac` such as the
    // channel's `Receiver<T>` is walkable through a method it declares on its
    // template: `fun mut Receiver<T>.next() => option.Option<T>`. The `for`
    // desugar is an ordinary `next()` call either way, so a wrong shape is
    // the ordinary type error of that call.
    let Some(name) = name else { return false };
    let origin = ctx.env.instances.borrow().origin.get(&name).cloned();
    origin.map_or(false, |(template, _)| {
        ctx.fns.templates.contains_key(&format!("{template}#next"))
    })
}

/// The receiver of a `xs.iter()` written as a `for` iterable, when `xs` is a
/// `List`, an array or a `Map`: such a loop walks `xs` itself, so it takes the
/// same desugar as `for x in xs`. Any other `iter()` (a user type's own method)
/// is left as written and goes down the `Iterator` path.
pub(crate) fn for_iter_receiver(iterable: &ast::Expr, ctx: &mut Ctx) -> Result<Option<ast::Expr>> {
    let ast::Expr::MethodCall { receiver, method, type_args, args, .. } = iterable else {
        return Ok(None);
    };
    if method != "iter" || !type_args.is_empty() || !args.is_empty() {
        return Ok(None);
    }
    let recv_ty = check_expr(receiver, ctx, None)?.ty;
    let walkable = crate::types::list_elem_of(&recv_ty).is_some()
        || crate::types::array_elem_of(&recv_ty).is_some()
        || crate::types::map_key_of(&recv_ty).is_some();
    Ok(walkable.then(|| (**receiver).clone()))
}

/// `for (k, v) in map { body }` becomes `for #key in map.keys() { val k =
/// #key; val v = map.get(#key); body }`. The keys are a snapshot in insertion
/// order, so the map may be changed inside the loop, and a key removed there
/// makes the later `get` panic like any `get` of an absent key.
pub(crate) fn desugar_map_for(
    binding: &str,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    iter_ty: &Ty,
) -> Result<ast::Stmt> {
    let refuse = |message: String| Err(TypeError { message, span, help: None });
    if !binding.starts_with("#forpat") {
        return refuse(format!(
            "'for' over a '{}' binds the key and the value together: write 'for (key, value) in map'",
            iter_ty.display()
        ));
    }
    let Some(ast::Stmt::LetTuple { pattern: ast::BindingPattern::Tuple(positions), .. }) = body.stmts.first() else {
        return refuse("a 'for' over a Map needs the pattern '(key, value)'".to_string());
    };
    let (ast::BindingPattern::Name(_) | ast::BindingPattern::Ignore, ast::BindingPattern::Name(_) | ast::BindingPattern::Ignore) =
        (&positions[0], &positions[1])
    else {
        return refuse("a 'for' over a Map binds two names: 'for (key, value) in map', with '_' to ignore one".to_string());
    };
    if positions.len() != 2 {
        return refuse(format!(
            "a 'for' over a Map binds a key and a value, and this pattern has {} positions",
            positions.len()
        ));
    }
    if ast::temporary_root(iterable).is_some() {
        return refuse("a 'for' over a Map walks a named map: bind the map to a 'val' first".to_string());
    }
    let ident = |name: &str| ast::Expr::Ident(name.to_string(), span);
    let mut stmts: Vec<ast::Stmt> = Vec::new();
    if let ast::BindingPattern::Name(k) = &positions[0] {
        stmts.push(ast::Stmt::Let { name: k.clone(), mutable: false, ty: None, init: ident(binding), span });
    }
    if let ast::BindingPattern::Name(v) = &positions[1] {
        stmts.push(ast::Stmt::Let {
            name: v.clone(),
            mutable: false,
            ty: None,
            init: ast::Expr::MethodCall {
                receiver: Box::new(iterable.clone()),
                method: "get".to_string(),
                type_args: Vec::new(),
                // A `String` key is looked up by its `str` view.
                args: vec![ast::CallArg {
                    name: None,
                    value: if matches!(crate::types::map_key_of(iter_ty), Some(Ty::String)) {
                        ast::Expr::MethodCall {
                            receiver: Box::new(ident(binding)),
                            method: "as_str".to_string(),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            span,
                        }
                    } else {
                        ident(binding)
                    },
                }],
                span,
            },
            span,
        });
    }
    stmts.extend(body.stmts[1..].iter().cloned());
    Ok(ast::Stmt::For {
        binding: binding.to_string(),
        binding_span: span,
        iterable: ast::Expr::MethodCall {
            receiver: Box::new(iterable.clone()),
            method: "keys".to_string(),
            type_args: Vec::new(),
            args: Vec::new(),
            span,
        },
        body: ast::Block { stmts, span: body.span },
        span,
    })
}

/// The index desugar a `for x in <List<T>|[T; N]>` becomes (`design/syntax.md (kelp wiki)`
/// 6.2), unchanged from the parser's old one except that the index advances
/// **before** the body runs. That is what makes a `continue` in the body jump
/// to the next element instead of re-reading the same one: the desugared loop
/// is an ordinary `while`, and its `continue` re-tests the condition.
///
/// A place iterable is walked as written so `borrowck.rs` can name it and
/// refuse a mutation of it while the body runs; anything else is staged into a
/// synthesized binding so it is evaluated exactly once.
pub(crate) fn desugar_list_for(
    binding: &str,
    binding_span: Span,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    ctx: &mut Ctx,
) -> ast::Stmt {
    let n = ctx.for_temps;
    ctx.for_temps += 1;
    let seq_name = format!("#for{n}");
    let idx_name = format!("#for{n}_i");
    let idx_span = binding_span;
    let iterating = ast::temporary_root(iterable)
        .is_none()
        .then(|| iterable.clone());
    let mut stmts: Vec<ast::Stmt> = Vec::new();
    let seq = match ast::temporary_root(iterable).cloned() {
        None => iterable.clone(),
        Some(root) => {
            let init_span = root.span();
            stmts.push(ast::Stmt::Let {
                name: seq_name.clone(),
                mutable: false,
                ty: None,
                init: root,
                span: init_span,
            });
            ast::rebase_root(iterable, &ast::Expr::Ident(seq_name.clone(), init_span))
        }
    };
    let seq_span = seq.span();
    let cond = ast::Expr::Binary {
        op: ast::BinOp::Lt,
        lhs: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
        rhs: Box::new(ast::Expr::MethodCall {
            receiver: Box::new(seq.clone()),
            method: "len".to_string(),
            type_args: Vec::new(),
            args: Vec::new(),
            span: seq_span,
        }),
        span,
    };
    let element = ast::Stmt::Let {
        name: binding.to_string(),
        mutable: false,
        ty: None,
        init: ast::Expr::MethodCall {
            receiver: Box::new(seq),
            method: "get".to_string(),
            type_args: Vec::new(),
            args: vec![ast::CallArg {
                name: None,
                value: ast::Expr::Ident(idx_name.clone(), idx_span),
            }],
            span: binding_span,
        },
        span: binding_span,
    };
    let increment = ast::Stmt::Expr {
        expr: ast::Expr::Assign {
            target: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
            value: Box::new(ast::Expr::Binary {
                op: ast::BinOp::Add,
                lhs: Box::new(ast::Expr::Ident(idx_name.clone(), idx_span)),
                rhs: Box::new(ast::Expr::IntLit(1, idx_span)),
                span: idx_span,
            }),
            span: idx_span,
        },
        span: idx_span,
    };
    let mut inner_stmts = vec![element, increment];
    inner_stmts.extend(body.stmts.iter().cloned());
    stmts.push(ast::Stmt::Let {
        name: idx_name,
        mutable: true,
        ty: Some(ast::TypeExpr::named("int", idx_span)),
        init: ast::Expr::IntLit(0, idx_span),
        span: idx_span,
    });
    stmts.push(ast::Stmt::While {
        cond,
        body: ast::Block {
            stmts: vec![ast::Stmt::Expr {
                expr: ast::Expr::Block(ast::Block {
                    stmts: inner_stmts,
                    span: body.span,
                }),
                span: body.span,
            }],
            span: body.span,
        },
        iterating,
        span,
    });
    ast::Stmt::Expr {
        expr: ast::Expr::Block(ast::Block { stmts, span }),
        span,
    }
}

/// The `next()` desugar a `for x in <Iterator>` becomes
/// (`kelp wiki page language/iterators.md`): the loop holds the iterator, calls `next()` once per
/// pass, binds the owned item in the `Some` arm, and leaves on `None`.
///
/// A **place** iterator is borrowed exclusively for the loop's duration, so a
/// use of it in the body is the ordinary "already borrowed" error; an iterator
/// built at the `for` line (a call result) is owned by a synthesized `var`. The
/// item is owned because `next()` returns an owned `Option`.
pub(crate) fn desugar_iterator_for(
    binding: &str,
    binding_span: Span,
    iterable: &ast::Expr,
    body: &ast::Block,
    span: Span,
    iter_ty: &Ty,
    ctx: &mut Ctx,
) -> ast::Stmt {
    let n = ctx.for_temps;
    ctx.for_temps += 1;
    let it_name = format!("#for{n}");
    let is_place = ast::temporary_root(iterable).is_none();
    let (init, mutable) = if !is_place {
        (iterable.clone(), true)
    } else if matches!(iter_ty, Ty::Ref { mutable: true, .. }) {
        // Already an exclusive borrow (`it: mut I`): bind the borrow itself;
        // `&mut it` would be a borrow of a borrow and would not resolve the
        // receiver's own methods.
        (iterable.clone(), false)
    } else {
        (
            ast::Expr::Ref {
                mutable: true,
                expr: Box::new(iterable.clone()),
                span,
            },
            false,
        )
    };
    let it_span = iterable.span();
    let scrutinee = ast::Expr::MethodCall {
        receiver: Box::new(ast::Expr::Ident(it_name.clone(), it_span)),
        method: "next".to_string(),
        type_args: Vec::new(),
        args: Vec::new(),
        span,
    };
    // The body's statements run in the `Some` arm, and the arm block has to be
    // unit like the `None` arm, so a trailing binding keeps the body's own tail
    // a statement rather than the arm's value (the same rule the index desugar
    // applies when it discards a tail).
    let mut some_stmts = body.stmts.clone();
    some_stmts.push(ast::Stmt::Let {
        name: format!("#for{n}_unit"),
        mutable: false,
        ty: None,
        init: ast::Expr::BoolLit(false, span),
        span,
    });
    let some_body = ast::Expr::Block(ast::Block {
        stmts: some_stmts,
        span: body.span,
    });
    let none_body = ast::Expr::Block(ast::Block {
        stmts: vec![ast::Stmt::Break { span }],
        span,
    });
    let when = ast::Expr::When {
        scrutinee: Box::new(scrutinee),
        arms: vec![
            ast::WhenArm {
                pattern: Some(ast::VariantPattern {
                    variant: "Some".to_string(),
                    binds: vec![Some(binding.to_string())],
                    span: binding_span,
                }),
                body: some_body,
                span,
            },
            ast::WhenArm {
                pattern: Some(ast::VariantPattern {
                    variant: "None".to_string(),
                    binds: Vec::new(),
                    span,
                }),
                body: none_body,
                span,
            },
        ],
        span,
        let_else: false,
    };
    let while_body = ast::Block {
        stmts: vec![ast::Stmt::Expr { expr: when, span }],
        span,
    };
    let block = ast::Block {
        stmts: vec![
            ast::Stmt::Let {
                name: it_name,
                mutable,
                ty: None,
                init,
                span,
            },
            ast::Stmt::While {
                cond: ast::Expr::BoolLit(true, span),
                body: while_body,
                iterating: None,
                span,
            },
        ],
        span,
    };
    ast::Stmt::Expr {
        expr: ast::Expr::Block(block),
        span,
    }
}

/// One destructuring binding, lowered (`design/syntax.md (kelp wiki)` 4): the whole value is
/// already bound to `base`, and every position the pattern binds is read out of
/// it by name. A nested pattern binds the position to a temporary of its own
/// first, so `val ((a, b), c) = ...` is two ordinary levels of the same
/// lowering. A position the pattern ignores is not read at all: what it holds
/// stays in the value that was destructured, whose own drop frees it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_pattern(
    pattern: &ast::BindingPattern,
    base: &str,
    base_ty: &Ty,
    elements: &[Ty],
    mutable: bool,
    ctx: &mut Ctx,
    stmts: &mut Vec<tir::Stmt>,
    span: Span,
) -> Result<()> {
    let ast::BindingPattern::Tuple(positions) = pattern else {
        return Err(TypeError {
            help: None,
            message: "a destructuring pattern is a parenthesized list of two or more positions ('val (lo, hi) = ...')".to_string(),
            span,
        });
    };
    for (i, position) in positions.iter().enumerate() {
        // The `tir` read of one position: a place (an owned local's field), so
        // an owned element read by value is the partial move A-11 already
        // plans, and a `Copy` element is a copy.
        let read = tir::Expr {
            kind: tir::ExprKind::FieldAccess {
                base: Box::new(tir::Expr {
                    kind: tir::ExprKind::Ident(base.to_string()),
                    ty: base_ty.clone(),
                    span,
                }),
                field: crate::types::tuple_field_name(i),
            },
            ty: elements[i].clone(),
            span,
        };
        match position {
            ast::BindingPattern::Ignore => {}
            ast::BindingPattern::Name(name) => {
                ctx.define(name, elements[i].clone(), mutable);
                stmts.push(tir::Stmt::Let {
                    name: name.clone(),
                    ty: elements[i].clone(),
                    init: read,
                });
            }
            ast::BindingPattern::Tuple(_) => {
                let nested = tuple_elems_of(&elements[i], ctx).ok_or_else(|| TypeError {
                    help: None,
                    message: format!(
                        "position {} of the pattern is itself a pattern, and element {} of the tuple it destructures ('{}') is not a tuple; bind that element to one name instead",
                        i + 1,
                        i + 1,
                        elements[i].display()
                    ),
                    span,
                })?;
                ctx.tuple_temps += 1;
                let tmp = format!("{}{}", TUPLE_TEMP_PREFIX, ctx.tuple_temps - 1);
                let tmp_ty = elements[i].clone();
                ctx.define(&tmp, tmp_ty.clone(), false);
                stmts.push(tir::Stmt::Let {
                    name: tmp.clone(),
                    ty: tmp_ty.clone(),
                    init: read,
                });
                lower_pattern(position, &tmp, &tmp_ty, &nested, mutable, ctx, stmts, span)?;
            }
        }
    }
    Ok(())
}

/// The number of positions a pattern writes, which is the arity the tuple it
/// destructures must have.
pub(crate) fn pattern_arity(pattern: &ast::BindingPattern) -> usize {
    match pattern {
        ast::BindingPattern::Tuple(positions) => positions.len(),
        _ => 1,
    }
}

pub(crate) fn stmt_span(stmt: &ast::Stmt) -> Span {
    match stmt {
        ast::Stmt::Let { span, .. } => *span,
        ast::Stmt::LetTuple { span, .. } => *span,
        ast::Stmt::While { span, .. } => *span,
        ast::Stmt::For { span, .. } => *span,
        ast::Stmt::Break { span } | ast::Stmt::Continue { span } => *span,
        ast::Stmt::Return { span, .. } => *span,
        ast::Stmt::Expr { span, .. } => *span,
        ast::Stmt::Defer { span, .. } | ast::Stmt::Errdefer { span, .. } => *span,
    }
}

/// A `fun` literal (A-4): `fun(x: i32) => i32 = x * 2`.
///
/// A literal is one synthesized top-level function, so most of the work is
/// the same work a declaration needs - resolve the signature, bind the
/// parameters, check the body against the declared return - and the rest is
/// the two things that make it a *literal*: its body is checked in a scope of
/// its own (no captures yet), and the checked body is stored where codegen
/// will emit it.
///
/// A *scoped* literal is checked by a retry loop (`kelp wiki page language/functions.md`):
/// every owned `var` capture starts as a shared borrow, a write through one
/// (or passing it as a `mut` argument) records the name instead of being final,
/// and the literal is checked again with exactly those captures typed `&mut T`.
/// The set only grows and is bounded by the number of captures, so the loop
/// ends; a refusal that names no upgradable capture is returned as it is.
pub(crate) fn check_fn_literal(
    params_ast: &[ast::Param],
    infer_params: bool,
    return_type: &Option<ast::TypeExpr>,
    body: &ast::FunctionBody,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if !matches!(expected, Some(Ty::ScopedFn { .. })) {
        return check_fn_literal_pass(
            params_ast, infer_params, return_type, body, span, ctx, expected,
            &std::collections::HashSet::new(),
        );
    }
    let mut excl: std::collections::HashSet<String> = std::collections::HashSet::new();
    let saved_needed = ctx.excl_needed.clone();
    loop {
        let fresh: std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>> =
            Default::default();
        ctx.excl_needed = fresh.clone();
        let (lits, count) = {
            let st = ctx.fns.instances.borrow();
            (st.literals.len(), st.literal_count)
        };
        let probes_mark = ctx.probes.len();
        let result = check_fn_literal_pass(
            params_ast, infer_params, return_type, body, span, ctx, expected, &excl,
        );
        ctx.excl_needed = saved_needed.clone();
        if result.is_err() {
            ctx.probes.truncate(probes_mark);
        }
        match result {
            Ok(e) => return Ok(e),
            Err(err) => {
                let writable = writable_outer(ctx);
                // A need this level cannot meet (the name belongs to a frame
                // further out) is the enclosing literal's to upgrade.
                for n in fresh.borrow().iter().filter(|n| !writable.contains(*n)) {
                    saved_needed.borrow_mut().insert(n.clone());
                }
                let fresh_needs: Vec<String> = fresh
                    .borrow()
                    .iter()
                    .filter(|n| writable.contains(*n) && !excl.contains(*n))
                    .cloned()
                    .collect();
                if fresh_needs.is_empty() {
                    return Err(err);
                }
                excl.extend(fresh_needs);
                // The failed pass may have emitted nested literals; the next
                // pass writes them again under the same keys.
                let mut st = ctx.fns.instances.borrow_mut();
                st.literals.truncate(lits);
                st.literal_count = count;
            }
        }
    }
}

pub(crate) fn check_fn_literal_pass(
    params_ast: &[ast::Param],
    infer_params: bool,
    return_type: &Option<ast::TypeExpr>,
    body: &ast::FunctionBody,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
    excl: &std::collections::HashSet<String>,
) -> Result<tir::Expr> {
    // A literal written at a `scoped fun(...)` parameter is the non-escaping
    // kind: its captures may be borrows, its environment is stack, and the
    // value's own type is `Ty::ScopedFn` (`kelp wiki page language/functions.md` part 2).
    let scoped = matches!(expected, Some(Ty::ScopedFn { .. }));
    // A `Mutex.lock` block (`std/sync`, B-5): the scoped parameter fixes the
    // block's own parameter types, but its return is the block's tail type, so
    // the expected `R` is ignored and filled in after the body is checked.
    let infer_ret = ctx.infer_scoped_ret && infer_params && scoped;
    // The function signature the *expected* type names, looked through a
    // borrow because an owned function-typed parameter arrives as
    // `&closure fun(...)`. A block literal takes both its parameter types and its
    // return from it; a written `fun` literal ignores it for the signature
    // and only uses it to learn whether it is scoped.
    let expected_sig = expected
        .map(|t| match t {
            Ty::Ref { inner, .. } => inner.as_ref(),
            other => other,
        })
        .and_then(|t| match t {
            Ty::Fn { params, ret }
            | Ty::Closure { params, ret }
            | Ty::ScopedFn { params, ret } => Some((params, ret)),
            _ => None,
        });
    // The signature: resolved where the literal was written for a `fun(...)`
    // literal (a parameter type resolves against the enclosing module and the
    // enclosing generic instance's type parameters, exactly as a nested
    // declaration's would), and taken from the expected function type for a
    // block header, which writes names only.
    let (params, ret) = if infer_params {
        match expected_sig {
            Some((want_params, want_ret)) => {
                if want_params.len() != params_ast.len() {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "this block declares {} parameter(s), but the parameter it is passed to expects a function of {}",
                            params_ast.len(),
                            want_params.len()
                        ),
                        span,
                    });
                }
                let params: Vec<(String, Ty)> = params_ast
                    .iter()
                    .zip(want_params)
                    .map(|(p, ty)| (p.name.clone(), ty.clone()))
                    .collect();
                (params, (**want_ret).clone())
            }
            // No function type to take types from. A headerless block keeps
            // the prototype's zero-parameter, unit-returning literal, so the
            // caller's own type comparison reports the mismatch; a block
            // that declares parameters has nothing to resolve them with, so
            // it is refused by name.
            None => {
                if !params_ast.is_empty() {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "this block declares parameter(s) ({}), but there is no function type at this position to take their types from - a block header is typed by the closure point it is passed to; write the parameter types on a 'fun(...)' literal instead",
                            params_ast
                                .iter()
                                .map(|p| p.name.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        span,
                    });
                }
                (Vec::new(), Ty::Unit)
            }
        }
    } else {
        let scope = ctx.type_scope();
        let mut params = Vec::with_capacity(params_ast.len());
        for p in params_ast {
            params.push((p.name.clone(), scope.resolve_param(p)?));
        }
        let ret = match return_type {
            Some(rt) => {
                // A borrow would outlive the frame that produced it, and a
                // function value is a plain pointer to code - the elision
                // rule that says *which* argument a borrow comes from has no
                // place to be written here.
                if rt.amp {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a 'fun' literal cannot return a borrow ('&{}') - an indirect call has no parameter an elided lifetime could bind to; return an owned value instead",
                            rt.display()
                        ),
                        span: rt.span,
                    });
                }
                scope.resolve(rt, false)?
            }
            None => Ty::Unit,
        };
        (params, ret)
    };
    let mut ret = ret;
    if infer_ret {
        // The placeholder the body is checked against; `return EXPR` inside the
        // block therefore needs a `unit` value. The common shapes (a bare
        // `return`, or a tail expression) infer correctly below.
        ret = Ty::Unit;
    }
    // One literal, one function. The key starts with `#`, which no `.kp`
    // identifier can contain, so it cannot collide with anything a program
    // declares (the same trick receiver methods use for `Type#name`).
    let key = {
        let mut st = ctx.fns.instances.borrow_mut();
        st.literal_count += 1;
        format!("#funlit{}", st.literal_count)
    };
    // The body's own scope. The enclosing locals are not *bindings* here -
    // they are the names the body may capture, which is what makes the
    // literal a closure when it names one (A-4 phase E). The lookup map
    // carries their types, because a capture's type is the captured
    // binding's type. Three sources, because a literal can be nested:
    //
    // * the enclosing frame's locals,
    // * the enclosing literal's own captures (a literal inside a literal),
    // * and everything *that* literal could see, transitively - which is how
    //   `fun` inside `fun` reaches a binding of the function two frames up
    //   (the enclosing closure captures it in turn).
    let mut outer_bindings: HashMap<String, Ty> = ctx.outer_bindings.clone();
    for scope in ctx.scopes.iter() {
        for (k, v) in scope.iter() {
            outer_bindings.insert(k.clone(), v.ty.clone());
        }
    }
    for (name, ty) in &ctx.captures {
        outer_bindings.insert(name.clone(), ty.clone());
    }
    let spawn_entry = std::mem::replace(&mut ctx.spawn_entry_next, false);
    let mut inner = (*ctx).clone();
    // Every block box in scope out here is a block value the literal must not
    // capture (decision 139, step 4).
    let mut outer_boxes = ctx.outer_boxes.clone();
    outer_boxes.extend(ctx.box_bindings.iter().map(|(k, v)| (k.clone(), v.clone())));
    inner.outer_boxes = outer_boxes;
    inner.box_sink_ok = None;
    inner.box_sink_here = None;
    inner.spawn_entry = spawn_entry;
    inner.spawn_entry_next = false;
    inner.scopes = vec![HashMap::new()];
    inner.outer_bindings = outer_bindings;
    inner.captures = Vec::new();
    inner.outer_receiver = ctx
        .receiver_struct
        .clone()
        .or_else(|| ctx.outer_receiver.clone());
    inner.ret_ty = ret.clone();
    inner.receiver_struct = None;
    inner.init_state = None;
    inner.in_fn_literal = true;
    // A literal is its own function: a `break`/`continue` in its body targets
    // a loop in *it*, never a loop of the enclosing frame, and its synthesized
    // loop names start fresh.
    inner.loop_depth = 0;
    inner.for_temps = 0;
    // A nested scoped block is typed by its own callee, never by the
    // surrounding `Mutex.lock` block's inference.
    inner.infer_scoped_ret = false;
    inner.borrowed_captures = scoped;
    inner.excl_captures = excl.clone();
    inner.outer_mut = writable_outer(ctx);
    inner.outer_excl = ctx.excl_captures.union(&ctx.outer_excl).cloned().collect();
    let mut tir_params = Vec::with_capacity(params.len());
    for (name, ty) in &params {
        inner.define(name, ty.clone(), false);
        tir_params.push((name.clone(), ty.clone()));
    }
    let checked = match body {
        ast::FunctionBody::Block(b) => check_block(b, &mut inner, if infer_ret { None } else { Some(&ret) })?,
        ast::FunctionBody::Expr(e) => {
            let value = check_expr(e, &mut inner, if infer_ret { None } else { Some(&ret) })?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(value)),
            }
        }
        ast::FunctionBody::Extern | ast::FunctionBody::Signature => {
            unreachable!("the parser requires a literal body")
        }
    };
    if infer_ret {
        ret = checked.ty();
    }
    // The same tail-versus-declared-return rule a declaration follows,
    // including the one divergence case (`panic` as the tail).
    let tail_diverges = checked
        .tail
        .as_ref()
        .is_some_and(|e| matches!(&e.kind, tir::ExprKind::Call { callee, .. } if callee == "panic"));
    if ret != Ty::Unit
        && checked.tail.is_some()
        && !tail_diverges
        && !str_view_compatible(&checked.ty(), &ret)
    {
        return Err(TypeError {
            help: None,
            message: format!(
                "this 'fun' literal is declared to return '{}' but its body evaluates to '{}'",
                ret.display(),
                checked.ty().display()
            ),
            span,
        });
    }
    let capture_list = inner.captures.clone();
    // The environment: one field per capture, filled at the point the literal
    // is *evaluated* - which is where a capture moves its value out of the
    // enclosing frame. Each field's initializer is written in the enclosing
    // function's terms: a local by name, or - for a literal nested in a
    // literal - a read of the enclosing closure's environment.
    let mut captures = Vec::with_capacity(capture_list.len());
    let mut env_fields: Vec<(String, Ty)> = Vec::with_capacity(capture_list.len());
    for (i, (name, ty)) in capture_list.iter().enumerate() {
        // A scoped function value lives only for the call it was written in,
        // so an ordinary closure's environment - which can be returned - may
        // not hold one (`kelp wiki page language/functions.md` part 2).
        if !scoped && matches!(ty, Ty::ScopedFn { .. }) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "a closure cannot capture the scoped function value '{name}': a scoped value may only live for the call it was written in, and a closure's environment can outlive that call; pass it on to another 'scoped fun(...)' parameter instead"
                ),
                span,
            });
        }
        // An exclusive capture is a borrow that lives for the call, so the
        // block's own result must not be derived from it
        // (`kelp wiki page language/functions.md`).
        if scoped && matches!(ty, Ty::Ref { mutable: true, .. }) && crate::types::is_borrowed_field_ty(&ret) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "this block writes through '{name}' (an exclusive capture) and returns a borrow ('{}'): the borrow would outlive the call that holds '{name}' exclusively; return an owned value (a copy or a 'String.from_str(...)') instead",
                    ret.display()
                ),
                span,
            });
        }
        // A borrow cannot be captured by an ordinary closure (its environment
        // outlives the frame), but a scoped literal's environment is stack and
        // lives only for the call, so a borrowed *value* (a `&T` parameter, a
        // `str` view) is simply copied in - it is a pointer, and the borrow it
        // carries is already live in the enclosing frame.
        if crate::types::is_borrowed_field_ty(&ty) && !scoped {
            return Err(TypeError {
                help: None,
                message: format!(
                    "a 'fun' literal cannot capture '{name}': it is a borrow ('{}'), and the environment it would be stored in outlives the frame the borrow points into - capture an owned value, or pass it as a parameter of the literal",
                    ty.display()
                ),
                span,
            });
        }
        // The environment field's own type is the capture's recorded type,
        // which `note_capture` already turned into `&T` for a scoped literal's
        // owned capture. Such a capture's *initializer* is the one thing left
        // to decide: `&name` for the owned binding that `note_capture`
        // borrowed, a plain read for a value that is already a pointer.
        let outer_owned = ctx
            .lookup(name)
            .map(|i| i.ty.clone())
            .filter(|t| {
                !crate::types::is_borrowed_field_ty(t)
                    && (!t.is_copy() || matches!(&ty, Ty::Ref { mutable: true, .. }))
            });
        let field_ty = ty.clone();
        let init = if ctx.lookup(name).is_some() {
            if scoped && outer_owned.is_some() {
                let place = tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: outer_owned.expect("checked above"),
                    span,
                };
                tir::Expr {
                    kind: tir::ExprKind::Ref {
                        mutable: matches!(&field_ty, Ty::Ref { mutable: true, .. }),
                        expr: Box::new(place),
                    },
                    ty: field_ty.clone(),
                    span,
                }
            } else {
                // Otherwise the value moves (or, for `Copy`, copies) into the
                // environment, and the enclosing binding is marked moved by
                // `borrowck.rs`.
                tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: ty.clone(),
                    span,
                }
            }
        } else if ctx.note_capture(name, span).is_some() {
            // The name belongs to the environment of the literal this one was
            // written inside - so the enclosing literal captures it too, and
            // this literal copies it out of that environment. Moving it out
            // instead would be a partial move (A-11), which is why only a
            // `Copy` capture can be nested this way.
            if !ty.is_copy() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "cannot capture '{name}' into this literal: it belongs to the enclosing closure's environment, which is not a local this frame owns, so there is no owned place to move it out of; capture an owned value from the frame that owns it, or copy a `Copy` one"
                    ),
                    span,
                });
            }
            let index = ctx
                .captures
                .iter()
                .position(|(n, _)| n == name)
                .expect("note_capture records the name");
            tir::Expr {
                kind: tir::ExprKind::Capture { index },
                ty: ty.clone(),
                span,
            }
        } else {
            unreachable!("a capture is always an enclosing local or an enclosing capture")
        };
        env_fields.push((crate::types::capture_field(i), field_ty));
        captures.push(init);
    }
    let fn_ty = if env_fields.is_empty() {
        // Nothing captured: this is a plain function value, exactly the value
        // phase C produced. Keeping the two apart is what keeps every
        // existing `fun(...)` placement (a `datac` field, a container
        // element's absence, a `Copy` binding) meaning what it did. A scoped
        // literal with nothing captured is widened into its fat pointer at
        // the call site, exactly as a plain function value used where a
        // closure is expected is.
        Ty::Fn {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    } else if scoped {
        Ty::ScopedFn {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    } else {
        Ty::Closure {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: Box::new(ret.clone()),
        }
    };
    let env = if env_fields.is_empty() {
        None
    } else {
        Some(tir::ClosureEnv {
            struct_name: crate::types::env_struct_name(&key),
            drop_name: crate::types::env_drop_name(&key),
            fields: env_fields,
        })
    };
    ctx.fns.instances.borrow_mut().literals.push(tir::Function {
        name: key.clone(),
        params: tir_params,
        ret,
        body: checked,
        // A literal has no `fun` declaration of its own, so the literal's own
        // span is the only line that describes it.
        span,
        module: ctx.module.clone(),
        extern_: false,
        env,
        naked: false,
        interrupt: false,
        align: None,
        section: None,
    });
    let kind = match captures.is_empty() {
        true => tir::ExprKind::FnRef(key),
        // A scoped literal's environment is stack and its drop is `NULL`
        // (`kelp wiki page language/functions.md`), which is a node of its own so neither
        // backend ever mallocs it or frees it.
        false if scoped => tir::ExprKind::ScopedClosureLit { key, captures },
        false => tir::ExprKind::ClosureLit { key, captures },
    };
    Ok(tir::Expr {
        kind,
        ty: fn_ty,
        span,
    })
}

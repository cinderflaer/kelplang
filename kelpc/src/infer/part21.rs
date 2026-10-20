use super::*;

/// The one constructor of the builtin atomic cell (MK9-S3-M1 stage A1):
/// `Atomic.new(x)` and `AtomicI64.new(0)`.
///
/// The element type comes from the named instantiation (`AtomicBool`), from the
/// binding's written type (`val c: Atomic<i64> = Atomic.new(0)`), or from the
/// argument itself, in that order, the same three sources `List.of(x)` has, so
/// the ordinary integer literal default answers `val c = Atomic.new(0)` and a
/// written type overrides it. There is no `empty`: an atomic always holds a
/// value, and a zero-initialized one is `Atomic.new(0)`.
pub(crate) fn check_atomic_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}.{method}' is not an 'Atomic' operation: an atomic is made with 'Atomic.new(x)' (or 'AtomicI64.new(0)' for a named instantiation) and read through the value it is bound to ('c.load(order = ...)', 'c.store(value = x, order = ...)')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            help: None,
            message: format!("'Atomic.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let named = crate::types::atomic_alias_elem(name);
    let written = expected.and_then(crate::types::atomic_elem_of).cloned();
    let from_position = named.clone().or_else(|| written.clone());
    let value = check_expr(&args[0].value, ctx, from_position.as_ref())?;
    let elem = match from_position {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Atomic<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        // Nothing said which element type, so the argument's own type is the
        // answer: `Atomic.new(true)` is `Atomic<bool>`.
        None => value.ty.clone(),
    };
    if !crate::types::is_atomic_element(&elem) {
        return Err(TypeError {
            help: None,
            message: atomic_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Atomic#new".to_string(), args: vec![value] },
        ty: Ty::Atomic(Box::new(elem)),
        span,
    })
}

/// `Mutex.new(value)` (B-5, `std/sync`): the one constructor of the builtin
/// mutex. The value is moved in, exactly as a `move T` parameter moves it, and
/// the element type comes from the binding's written type or from the
/// argument, in that order: the same two sources `Atomic.new` has.
pub(crate) fn check_mutex_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}.{method}' is not a 'Mutex' operation: a mutex is made with 'Mutex.new(value)' and reached through the value it is bound to ('m.lock {{ v -> ... }}', 'm.into_inner()')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            help: None,
            message: format!("'Mutex.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let written = expected.and_then(crate::types::mutex_elem_of).cloned();
    let value = check_expr(&args[0].value, ctx, written.as_ref())?;
    if value.ty == Ty::Unit {
        return Err(TypeError {
            help: None,
            message: "'Mutex.new' needs a value to guard, found 'unit'".to_string(),
            span: args[0].value.span(),
        });
    }
    let elem = match written {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Mutex<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        None => value.ty.clone(),
    };
    if !crate::types::is_mutex_element(&elem) {
        return Err(TypeError {
            help: None,
            message: mutex_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    note_heap_use(&ctx.env, managed::HeapSlot::LockValue, &elem, args[0].value.span());
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Mutex#new".to_string(), args: vec![value] },
        ty: Ty::Mutex(Box::new(elem)),
        span,
    })
}

/// A method on a `Mutex<T>` (B-5). `lock` is checked structurally, because its
/// block's return type comes from the block rather than from a fixed signature;
/// `into_inner` consumes the receiver and hands back the guarded value.
pub(crate) fn check_mutex_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    elem: &Ty,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let _ = probe;
    let cell = Ty::Mutex(Box::new(elem.clone()));
    match method {
        "lock" => check_mutex_lock(receiver, elem, args, span, ctx, expected),
        // `m.wait_while(cv, pred) { v -> ... }` and its timed twin: the
        // condition variable's own `wait` cannot be spelled with a scoped
        // guard, so the wait lives on the mutex, which owns the lock the wait
        // has to release atomically (B-5 rem, `kelp wiki page stdlib/sync.md`).
        "wait_while" => check_mutex_wait(receiver, elem, args, span, ctx, expected, false),
        "wait_while_timeout" => {
            check_mutex_wait(receiver, elem, args, span, ctx, expected, true)
        }
        // `m.share()`: another owned handle to the same runtime record, so a
        // channel's two ends or a pool's workers can each own one. The
        // record is freed when the last handle drops.
        "share" => {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'Mutex.share' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg(
                "'self'",
                &Ty::Ref { mutable: false, inner: Box::new(cell.clone()) },
                receiver,
                ctx,
            )?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#share".to_string(),
                    args: vec![self_arg],
                },
                ty: cell,
                span,
            })
        }
        // `m.swap(value)`: put `value` in as the guarded value and take the old
        // one out, in one locked step. A scoped block cannot capture an owned
        // value by move, so this is how one is handed to a mutex from inside
        // a block (a channel's `send` stages the value in a private mutex and
        // swaps it across).
        "swap" => {
            if args.len() != 1 {
                return Err(TypeError {
                    help: None,
                    message: format!("'Mutex.swap' takes exactly 1 argument, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg(
                "'self'",
                &Ty::Ref { mutable: false, inner: Box::new(cell.clone()) },
                receiver,
                ctx,
            )?;
            let value = check_call_arg("'value'", elem, &args[0].value, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#swap".to_string(),
                    args: vec![self_arg, value],
                },
                ty: elem.clone(),
                span,
            })
        }
        "into_inner" => {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'Mutex.into_inner' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            // The receiver is consumed: the mutex is dead once its value is
            // out, exactly as `thread.join` consumes a handle.
            let self_arg = check_call_arg("'self'", &cell, receiver, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "Mutex#into_inner".to_string(),
                    args: vec![self_arg],
                },
                ty: elem.clone(),
                span,
            })
        }
        other => Err(TypeError {
            help: None,
            message: format!(
                "'Mutex.{other}' is not a mutex operation: write 'm.lock {{ v -> ... }}' to reach the value while the lock is held, 'm.share()' for another handle to the same mutex, 'm.swap(value)' to exchange the guarded value, or 'm.into_inner()' to consume the mutex and take the value out"
            ),
            span,
        }),
    }
}

/// `m.lock { v -> ... }` (B-5): the mutex is held for the block's whole run, the
/// block receives `v: mut T`, its value is returned, and the runtime releases
/// the lock on every way out (a normal exit, a `return`/`break` inside the
/// block, and a panic, which poisons the mutex). The block's return type is the
/// block's own tail type, which is why the literal is checked with the
/// `infer_scoped_ret` flag set.
pub(crate) fn check_mutex_lock(
    receiver: &ast::Expr,
    elem: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots("method 'Mutex.lock'", &["block".to_string()], args, span)?;
    let block_arg = &args[slots[0]];
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref { mutable: false, inner: Box::new(Ty::Mutex(Box::new(elem.clone()))) },
        receiver,
        ctx,
    )?;
    // The block takes `mut T`, an exclusive borrow valid only inside the call,
    // which the existing scoped-block rules already refuse to store or return.
    let want = Ty::ScopedFn {
        params: vec![Ty::Ref { mutable: true, inner: Box::new(elem.clone()) }],
        ret: Box::new(expected.cloned().unwrap_or(Ty::Unit)),
    };
    // The block's return is its tail type when the call site names no type
    // (`print(m.lock { v -> *v })`), and the call's own expected type when it
    // does (`val r: Result<_, _> = m.lock { v -> ... ? ... }`), which is what
    // lets a `?` inside the block propagate the block's declared return.
    ctx.infer_scoped_ret = expected.is_none();
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        // A literal that captured nothing is a plain `fun` value: widen it to
        // the fat pointer the runtime calls, exactly as a block passed to any
        // other `scoped fun(...)` parameter is.
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the block 'Mutex.lock' takes has type '{}', expected a scoped function taking 'mut {}'",
                    other.display(),
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let ret = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: "Mutex#lock".to_string(),
            args: vec![self_arg, block],
        },
        ty: ret,
        span,
    })
}

/// The `Option<T>` a scoped-block call hands back when the block may not run,
/// named through `std/option`'s key exactly as `try_to_<T>` names it, so a
/// program that never imported the module is told to rather than given an
/// internal error.
pub(crate) fn option_ty(ctx: &Ctx, elem: Ty, span: Span) -> Result<Ty> {
    let option_key = format!(
        "{}Option",
        crate::stdlib::key_prefix(crate::stdlib::OPTION_MODULE)
    );
    if !ctx.env.generics.contains_key(&option_key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "this call hands back 'Option<T>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                option_key,
                crate::stdlib::OPTION_MODULE
            ),
            span,
        });
    }
    ctx.instantiate_type(&option_key, &[elem], span)
}

/// `RwLock.new(value)` (`std/sync`): the one constructor of the builtin
/// reader/writer lock. The value is moved in, exactly as `Mutex.new` moves its
/// value, and the element type comes from the binding's written type or from
/// the argument, in that order.
pub(crate) fn check_rwlock_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}.{method}' is not an 'RwLock' operation: an rwlock is made with 'RwLock.new(value)' and reached through the value it is bound to ('rw.read {{ v -> ... }}', 'rw.write {{ v -> ... }}', 'rw.into_inner()')"
            ),
            span,
        });
    }
    if args.len() != 1 {
        return Err(TypeError {
            help: None,
            message: format!("'RwLock.new' takes exactly 1 argument, found {}", args.len()),
            span,
        });
    }
    let written = expected.and_then(crate::types::rwlock_elem_of).cloned();
    let value = check_expr(&args[0].value, ctx, written.as_ref())?;
    if value.ty == Ty::Unit {
        return Err(TypeError {
            help: None,
            message: "'RwLock.new' needs a value to guard, found 'unit'".to_string(),
            span: args[0].value.span(),
        });
    }
    let elem = match written {
        Some(elem) => {
            if value.ty != elem {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'RwLock<{}>.new' needs a '{}' value, found '{}'",
                        elem.display(),
                        elem.display(),
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            }
            elem
        }
        None => value.ty.clone(),
    };
    if !crate::types::is_rwlock_element(&elem) {
        return Err(TypeError {
            help: None,
            message: rwlock_element_error(&elem),
            span: args[0].value.span(),
        });
    }
    note_heap_use(&ctx.env, managed::HeapSlot::LockValue, &elem, args[0].value.span());
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "RwLock#new".to_string(), args: vec![value] },
        ty: Ty::RwLock(Box::new(elem)),
        span,
    })
}

/// A method on an `RwLock<T>` (`std/sync`). `read`/`write` are checked
/// structurally, because each block's return type comes from the block rather
/// than from a fixed signature; `try_read`/`try_write` wrap that value in an
/// `Option` (`None` when the lock was busy and the block did not run).
pub(crate) fn check_rwlock_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    elem: &Ty,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let _ = probe;
    let cell = Ty::RwLock(Box::new(elem.clone()));
    match method {
        "read" => check_rwlock_block(receiver, elem, false, false, args, span, ctx, expected),
        "write" => check_rwlock_block(receiver, elem, true, false, args, span, ctx, expected),
        "try_read" => check_rwlock_block(receiver, elem, false, true, args, span, ctx, expected),
        "try_write" => check_rwlock_block(receiver, elem, true, true, args, span, ctx, expected),
        "into_inner" => {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'RwLock.into_inner' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            let self_arg = check_call_arg("'self'", &cell, receiver, ctx)?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: "RwLock#into_inner".to_string(),
                    args: vec![self_arg],
                },
                ty: elem.clone(),
                span,
            })
        }
        other => Err(TypeError {
            help: None,
            message: format!(
                "'RwLock.{other}' is not an rwlock operation: write 'rw.read {{ v -> ... }}' or 'rw.write {{ v -> ... }}' to reach the value with the lock held, 'rw.try_read'/'.try_write' for the non-blocking form, or 'rw.into_inner()' to consume it"
            ),
            span,
        }),
    }
}

/// The shared body of `read`/`write`/`try_read`/`try_write`: the receiver is
/// `&RwLock<T>`, the block is a `scoped fun` taking `&T` (read) or `mut T`
/// (write), and `try` wraps the block's value in `Option`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_rwlock_block(
    receiver: &ast::Expr,
    elem: &Ty,
    write: bool,
    try_lock: bool,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let name = if try_lock {
        if write { "try_write" } else { "try_read" }
    } else if write {
        "write"
    } else {
        "read"
    };
    let slots = arg_slots(&format!("method 'RwLock.{name}'"), &["block".to_string()], args, span)?;
    let block_arg = &args[slots[0]];
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::RwLock(Box::new(elem.clone()))),
        },
        receiver,
        ctx,
    )?;
    let param = Ty::Ref { mutable: write, inner: Box::new(elem.clone()) };
    let want = Ty::ScopedFn {
        params: vec![param],
        ret: Box::new(expected.cloned().unwrap_or(Ty::Unit)),
    };
    // `read`/`write` return the block's tail type when the call site names no
    // type, exactly as `Mutex.lock` does. `try_*` always infer, because the
    // call's own type is `Option<R>` and the block's is `R`.
    ctx.infer_scoped_ret = try_lock || expected.is_none();
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the block 'RwLock.{name}' takes has type '{}', expected a scoped function taking '{} {}'",
                    other.display(),
                    if write { "mut" } else { "&" },
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let ret = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    let ty = if try_lock { option_ty(ctx, ret, span)? } else { ret };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("RwLock#{name}"),
            args: vec![self_arg, block],
        },
        ty,
        span,
    })
}

/// `Condvar.new()` (`std/sync`): the one constructor of the builtin condition
/// variable. It takes no argument: a condition variable guards no value of its
/// own; it is paired with one `Mutex<T>` at each `wait_while`.
pub(crate) fn check_condvar_associated(
    name: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    _ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if method != "new" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}.{method}' is not a 'Condvar' operation: a condition variable is made with 'Condvar.new()' and reached through the value it is bound to ('cv.notify_one()')"
            ),
            span,
        });
    }
    if !args.is_empty() {
        return Err(TypeError {
            help: None,
            message: format!("'Condvar.new' takes no arguments, found {}", args.len()),
            span,
        });
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: "Condvar#new".to_string(), args: vec![] },
        ty: Ty::Condvar,
        span,
    })
}

/// `cv.notify_one()` and `cv.notify_all()`: the two ways to wake a waiter.
/// There is no bare `wait`; the wait is `Mutex.wait_while` (B-5 rem,
/// `kelp wiki page stdlib/sync.md`).
pub(crate) fn check_condvar_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let _ = probe;
    if method == "share" {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'Condvar.share' takes no arguments, found {}", args.len()),
                span,
            });
        }
        let self_arg = check_call_arg("'self'", &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) }, receiver, ctx)?;
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call { callee: "Condvar#share".to_string(), args: vec![self_arg] },
            ty: Ty::Condvar,
            span,
        });
    }
    if method != "notify_one" && method != "notify_all" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'Condvar.{method}' is not a condition variable operation: write 'cv.notify_one()' or 'cv.notify_all()' to wake a waiter; a wait is 'm.wait_while(cv, pred) {{ v -> ... }}' on the mutex that guards the state"
            ),
            span,
        });
    }
    if !args.is_empty() {
        return Err(TypeError {
            help: None,
            message: format!("'Condvar.{method}' takes no arguments, found {}", args.len()),
            span,
        });
    }
    let self_arg = check_call_arg("'self'", &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) }, receiver, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("Condvar#{method}"),
            args: vec![self_arg],
        },
        ty: Ty::Unit,
        span,
    })
}

/// `m.wait_while(cv, pred) { v -> ... }` and its timed twin (`std/sync`). The
/// mutex is held for the block's whole run; the wait releases it around every
/// sleep and re-tests `pred(&T)` after each wakeup, so a spurious wakeup never
/// reaches the caller. The timed form hands back `Option<R>`: `None` when the
/// deadline passed with the predicate still true and the block did not run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_mutex_wait(
    receiver: &ast::Expr,
    elem: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
    timed: bool,
) -> Result<tir::Expr> {
    let method = if timed { "wait_while_timeout" } else { "wait_while" };
    let names: Vec<String> = if timed {
        vec!["cv".into(), "pred".into(), "millis".into(), "block".into()]
    } else {
        vec!["cv".into(), "pred".into(), "block".into()]
    };
    let slots = arg_slots(&format!("method 'Mutex.{method}'"), &names, args, span)?;
    let self_arg = check_call_arg(
        "'self'",
        &Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Mutex(Box::new(elem.clone()))),
        },
        receiver,
        ctx,
    )?;
    let cv_arg = check_call_arg(
        "'cv'",
        &Ty::Ref { mutable: false, inner: Box::new(Ty::Condvar) },
        &args[slots[0]].value,
        ctx,
    )?;
    // The predicate is a `fun(&T) => bool` (or a closure of that shape): a
    // parameter is already a shared borrow in Kelp, so its resolved type is
    // `T` for a `Copy` element and `&T` for an owned one
    // (`implicit_borrow_wrap`), and the runtime hands the generated body the
    // element's address accordingly. It is called with the lock held, once
    // before the first wait and again after every wakeup.
    let pred_param = implicit_borrow_wrap(elem.clone(), false);
    let pred_want = Ty::Closure {
        params: vec![pred_param.clone()],
        ret: Box::new(Ty::Bool),
    };
    let pred_expr = check_expr(&args[slots[1]].value, ctx, None)?;
    let pred = match pred_expr.ty.clone() {
        Ty::Closure { params, ret } if params == pred_want_params(&pred_want) && *ret == Ty::Bool => pred_expr,
        Ty::Fn { params, ret }
            if params == pred_want_params(&pred_want) && *ret == Ty::Bool =>
        {
            coerce_fn_to_closure(pred_want, pred_expr)
        }
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the predicate 'Mutex.{method}' takes has type '{}', expected 'fun({}) => bool'",
                    other.display(),
                    pred_param.display()
                ),
                span: args[slots[1]].value.span(),
            })
        }
    };
    let millis = if timed {
        Some(check_call_arg("'millis'", &Ty::I64, &args[slots[2]].value, ctx)?)
    } else {
        None
    };
    let block_arg = &args[slots[if timed { 3 } else { 2 }]];
    let want = Ty::ScopedFn {
        params: vec![Ty::Ref { mutable: true, inner: Box::new(elem.clone()) }],
        ret: Box::new(Ty::Unit),
    };
    ctx.infer_scoped_ret = true;
    let checked = check_expr(&block_arg.value, ctx, Some(&want));
    ctx.infer_scoped_ret = false;
    let checked = checked?;
    let block = match checked.ty.clone() {
        Ty::ScopedFn { .. } => checked,
        Ty::Fn { params, ret } => coerce_fn_to_scoped(Ty::ScopedFn { params, ret }, checked),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the block 'Mutex.{method}' takes has type '{}', expected a scoped function taking 'mut {}'",
                    other.display(),
                    elem.display()
                ),
                span: block_arg.value.span(),
            })
        }
    };
    let inner = match &block.ty {
        Ty::ScopedFn { ret, .. } => (**ret).clone(),
        _ => unreachable!("the block was widened to a scoped function"),
    };
    let mut call_args = vec![self_arg, cv_arg, pred];
    if let Some(millis) = millis {
        call_args.push(millis);
    }
    call_args.push(block);
    let ty = if timed { option_ty(ctx, inner, span)? } else { inner };
    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: format!("Mutex#{method}"),
            args: call_args,
        },
        ty,
        span,
    })
}

/// The `&T` parameter a wait predicate takes. Pulled out so the two shape
/// matches above stay readable.
pub(crate) fn pred_want_params(want: &Ty) -> Vec<Ty> {
    match want {
        Ty::Closure { params, .. } => params.clone(),
        _ => unreachable!("the caller built a closure shape"),
    }
}

/// A bare name that only exists as an imported item's *qualified* name is a
/// very likely strict-namespace mistake (M5: imported items are reached
/// only as `alias.Item`). Returns a "did you mean ..." fragment when `name`
/// matches some qualified key's last segment.
/// Only an exported key is offered (A-13): suggesting the qualified spelling
/// of an item another module keeps private would tell the reader to write a
/// line that the visibility gate refuses.
pub(crate) fn qualified_hint<'a>(
    name: &str,
    keys: impl Iterator<Item = &'a String>,
    aliases: &HashMap<String, String>,
    env: &TypeEnv,
) -> Option<String> {
    let want = format!(".{name}");
    let key = keys
        .into_iter()
        .find(|k| k.ends_with(&want) && k.contains('.') && exported(k, env))?;
    let (module_path, _) = key.rsplit_once('.')?;
    let qualified_prefix = format!("{module_path}.");
    Some(match aliases.iter().find(|(_, p)| p.as_str() == qualified_prefix) {
        Some((alias, _)) => format!("'{alias}.{name}'"),
        None => {
            let import_path = module_path.replace('.', "/");
            format!(
                "'{name}' is declared in module '{import_path}' - add `impl \"{import_path}\"` and reach it as `<alias>.{name}`"
            )
        }
    })
}

/// `alias.item(args)` - a reference to an item in an imported module, per
/// M5's strict module-qualified namespace. `prefix` is that module's symbol
/// prefix, so the actual lookup key is just `prefix + item`: the same
/// table `check_call` uses for any other call, which is why imported
/// `datac` construction and imported free functions need no new lowering.
pub(crate) fn check_module_item(
    alias: &str,
    prefix: &str,
    item: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let qualified = format!("{prefix}{item}");
    // Private by default (A-13), and this is the gate: the item is known to
    // exist here, so an unexported one is refused as private rather than
    // reported as missing.
    if declares(&qualified, ctx) {
        ctx.check_exported(&qualified, alias, item, span)?;
    }
    // A capability value's constructor is sealed to its own module (kernel
    // stage 18, `kelp wiki page stdlib/irq.md`): the shipped library exports every
    // item and Kelp has no field-level `pub`, so `irq.Line(n = 3)` would forge
    // the receipt for a grant. Only `irq.claim` and `frame.claim` may build
    // one, and they are reached as functions, not as the type's own name.
    if crate::stdlib::is_sealed_type(&qualified) {
        let module = crate::stdlib::sealed_type_module(&qualified).unwrap_or("its own module");
        return Err(TypeError {
            help: None,
            message: format!(
                "'{alias}.{item}' is a capability value: its constructor is private to module '{module}', so another module cannot build one; take the value from a `claim` call instead"
            ),
            span,
        });
    }
    if ctx.structs.contains_key(&qualified)
        || ctx.env.generics.contains_key(&qualified)
        || ctx.sigs.contains_key(&qualified)
        || ctx.overloads.contains_key(&qualified)
        || ctx.fns.templates.contains_key(&qualified)
    {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    // A module's `const` is reached the same way as its functions: as
    // `math.PI` (M6.3). It takes no arguments, so a call-shaped reference to
    // one is worth a message of its own.
    if let Some(c) = ctx.consts.get(&qualified).cloned() {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'{alias}.{item}' is a constant, not a function"),
                span,
            });
        }
        let mut value = c.value;
        value.span = span;
        return Ok(value);
    }
    // A module's `static` is reached like its `const`, but it is an object
    // rather than a folded literal, so it takes no arguments and is a place.
    if let Some(s) = ctx.statics.get(&qualified).cloned() {
        if !args.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!("'{alias}.{item}' is a static object, not a function"),
                span,
            });
        }
        require_static_unsafe(&s, span, ctx)?;
        return Ok(tir::Expr {
            kind: tir::ExprKind::StaticRef(s.qualified),
            ty: s.ty,
            span,
        });
    }
    // A receiver method lives under a `Type#name` key, which a module path
    // can't spell - say so rather than "no such item".
    let method_suffix = format!("#{item}");
    if let Some(key) = ctx
        .sigs
        .keys()
        .find(|k| k.starts_with(prefix) && k.ends_with(&method_suffix))
    {
        let type_name = key.trim_end_matches(&method_suffix).trim_end_matches(prefix);
        return Err(TypeError {
            help: None,
            message: format!(
                "'{item}' is a method on '{type_name}', not a free function - call it on a value ('value.{item}(...)')"
            ),
            span,
        });
    }
    // A builtin free function a shipped module *documents* rather than
    // declares (`thread.fence`, MK9-S3-M1 stage A2): its signature is in the
    // same builtin table `panic`'s is, for the reason the module carries no
    // declaration (see the entry in `BUILTINS`). Checked last, so a real
    // declaration of the same key always wins.
    if builtin_sig(&qualified).is_some() {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    // `thread.spawn` and `thread.join` (MK9-S3-M1 stage B1) sit beside the
    // fence for the same reason, one step further: their signatures depend on
    // the value that crosses the boundary, so they are checked in `check_call`
    // rather than read from the table.
    if crate::stdlib::is_thread_fn(&qualified) {
        return check_call(&qualified, type_args, args, span, ctx, expected);
    }
    Err(TypeError {
        help: None,
        message: format!("module '{alias}' has no item '{item}'"),
        span,
    })
}

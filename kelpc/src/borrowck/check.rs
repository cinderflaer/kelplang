use super::*;

/// Checks an expression used as a whole value: a bare `Ident` here moves a
/// non-Copy binding, and `*p` here (not just read *through*) is rejected
/// when the pointee is non-Copy - mirrors Rust's "cannot move out of a
/// shared reference."
pub(crate) fn check_value(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => ctx.use_value_moving(name, expr),
        // A `static` read (kernel stage 7) is a read of a place the frame does
        // not own, so nothing moves and nothing drops: a static's type is a
        // scalar, an array of scalars, a datac of those, or an `Atomic`, none
        // of which owns memory.
        tir::ExprKind::StaticRef(_) => Ok(()),
        tir::ExprKind::FieldAccess { base, field } => {
            check_place_through(base, ctx)?;
            // A field read is a *place*, and a partial move is the one way a
            // read of it gives the frame's value away. It is allowed only out
            // of an owned binding this frame holds directly: the move copies
            // the field out and codegen zeroes the slot it came from, so the
            // binding's own end-of-scope drop frees the rest and nothing
            // twice. Reading through a borrow would hand out a second owner of
            // a buffer the borrowed value still owns, the heap-use-after-free
            // `owned_field_read.kp` pinned, so it stays refused.
            if crate::types::needs_drop(&expr.ty, &ctx.owned_structs) {
                let Some((root, mut path)) = partial_move_target(base, ctx) else {
                    // A tuple position is read by value only by the lowering a
                    // destructuring binding does, whose base is that binding's
                    // own temporary, so the message names the position rather
                    // than the derived record's field (`f0`) and the
                    // synthesized name a reader never wrote.
                    if let Some(index) = crate::types::tuple_field_index(field) {
                        let message = match base.ty {
                            Ty::Ref { .. } => format!(
                                "cannot read element {} of the tuple out of a borrow by value: the value is borrowed, so it is not this frame's to give away; read a 'Copy' element, use the tuple through a borrow (pass it to a parameter, call a method on it), or take it with 'move'",
                                index + 1
                            ),
                            _ => format!(
                                "cannot read element {} of the tuple by value: the base is not a local this frame owns, so a partial move cannot take the element out; use the tuple through a borrow, or hold it in a 'Box' and 'take()' it",
                                index + 1
                            ),
                        };
                        return Err(BorrowError { message, span: expr.span, help: None });
                    }
                    let message = match base.ty {
                        Ty::Ref { .. } => format!(
                            "cannot move the field '{field}' out of '{}': the value is borrowed, so it is not this frame's to give away",
                            tir_root_ident(base).unwrap_or("a borrow")
                        ),
                        _ => format!(
                            "cannot read the owned field '{field}' by value: the base is not a local this frame owns, so a partial move cannot take the field out; use it through a borrow (pass it to a parameter, call a method on it) or hold it in a 'Box' and 'take()' it"
                        ),
                    };
                    return Err(BorrowError { message, span: expr.span, help: None });
                };
                path.push(field.clone());
                let proj = if path.len() == 1 {
                    Projection::Field(path.remove(0))
                } else {
                    Projection::NestedField(path)
                };
                if crate::types::is_drop_type(&expr.ty, &ctx.drop_types) {
                    let (what, base_name) = match crate::types::tuple_field_index(field) {
                        Some(index) => (
                            format!("element {}", index + 1),
                            "the tuple".to_string(),
                        ),
                        None => (
                            format!("field '{field}'"),
                            tir_root_ident(base).unwrap_or("a value").to_string(),
                        ),
                    };
                    return Err(BorrowError {
                        help: None,
                        message: format!(
                            "cannot move {what} out of '{base_name}' by value: it is a type with a destructor, and a partial move zeroes the slot it came from, so the base's own drop would run that destructor a second time; take the whole value by moving it, or use it through a borrow"
                        ),
                        span: expr.span,
                    });
                }
                ctx.move_field_out(root, proj, expr)?;
            }
            Ok(())
        }
        tir::ExprKind::IntLit(_) | tir::ExprKind::BlockBox { .. } | tir::ExprKind::CharLit(_) | tir::ExprKind::FloatLit(_) | tir::ExprKind::StringLit(_) | tir::ExprKind::BoolLit(_) => Ok(()),
        tir::ExprKind::Unary { expr: sub, .. } => check_value(sub, ctx),
        tir::ExprKind::Binary { lhs, rhs, .. } => {
            check_value(lhs, ctx)?;
            check_value(rhs, ctx)
        }
        // A function reference is a value with no owner: nothing to move,
        // nothing to drop (A-4).
        tir::ExprKind::FnRef(_) => Ok(()),
        // A `fun` literal that captured something (A-4 phase E): the value it
        // produces *owns* its environment, so it is a value like any other
        // owned one (its drop is planned by whoever binds or passes it), and
        // each captured binding is moved in exactly as `val b = a` moves one.
        // A `Copy` capture is a copy, which is what the type says.
        tir::ExprKind::ClosureLit { captures, .. } => {            for init in captures {
                if is_copy(&init.ty) {
                    // A `Copy` value is copied into the environment, so the
                    // enclosing binding stays usable and nothing is tracked.
                    continue;
                }
                match &init.kind {
                    tir::ExprKind::Ident(name) => ctx.use_value_moving(name, init),
                    // A nested literal copying out of the enclosing closure's
                    // environment: only `Copy` values get here, so the read
                    // moves nothing.
                    tir::ExprKind::Capture { .. } => Ok(()),
                    _ => Ok(()),
                }?;
            }
            Ok(())
        }
        // A scoped literal's environment is stack and owns nothing
        // (`kelp wiki page language/functions.md`): every capture is a `Copy` value or a
        // shared borrow, so nothing moves and nothing drops. The captured
        // borrows themselves are started by `take_call_arguments`, which is
        // the only place a scoped literal can be written.
        tir::ExprKind::ScopedClosureLit { .. } => Ok(()),
        // A plain function value widened into a closure (A-4 phase E): the
        // value being wrapped is read as a whole value - a `fun` is `Copy`, so
        // that moves nothing, and the wrapper itself is an owned temporary
        // exactly like a closure literal.
        tir::ExprKind::FnToClosure { func } => check_value(func, ctx),
        // A read of one field of the enclosing closure's environment (A-4
        // phase E). The environment owns the value, but it is reached through
        // the closure value rather than held in a local of this frame, and a
        // partial move is only planned for a local the frame owns, so reading
        // it out whole would give two owners of the same buffer. An owned
        // enum payload is refused the same way, and for the same reason.
        tir::ExprKind::Capture { .. } => {
            if crate::types::needs_drop(&expr.ty, &ctx.owned_structs) {
                return Err(BorrowError {
                    help: None,
                    message: "cannot read a captured value out of the closure's environment by value: the environment is not a local this frame owns, so the read would give two owners of the same buffer; use it through a borrow (read a field of it, call a method on it, pass it to a parameter), or capture a `Copy` value instead".to_string(),
                    span: expr.span,
                });
            }
            Ok(())
        }
        tir::ExprKind::Call { .. } | tir::ExprKind::VariadicCall { .. } => check_call(expr, ctx),
        // A call through a function value (A-4): the callee is read as a
        // *place*, not as a whole value - a function value is `Copy`, and a
        // closure is called through a borrow of the binding that holds it, so
        // calling one must not move it - and the arguments follow exactly the
        // rule a named call's do.
        tir::ExprKind::IndirectCall { callee, args } => {
            check_place(callee, ctx)?;
            let taken = take_call_arguments(args, ctx, None, false, None)?;
            release_call_borrows(&taken, ctx);
            Ok(())
        }
        // A method call through a trait object (A-12 slices 4 and 7). Through a
        // `&dyn` the receiver is a borrow (`Copy`), so reading it moves nothing.
        // Through an *owned* trait object a `mut Self` method writes through the
        // box's data pointer, so the call holds an exclusive borrow of the
        // receiver's own place for its duration, exactly as a `mut T` argument
        // does: that is what refuses `b.scale(f)` while `b.get()`'s `&dyn` is
        // still live.
        tir::ExprKind::DynCall {
            receiver,
            args,
            exclusive,
            ..
        } => {
            let roots = if *exclusive {
                ctx.check_iterated_mutation(receiver)?;
                check_place(receiver, ctx)?;
                let roots = place_roots(receiver, ctx);
                for root in &roots {
                    if root != STATIC_ROOT {
                        ctx.start_borrow(root, true, expr.span)?;
                    }
                }
                roots
            } else {
                check_place(receiver, ctx)?;
                Vec::new()
            };
            let taken = take_call_arguments(args, ctx, None, false, None)?;
            release_call_borrows(&taken, ctx);
            for root in &roots {
                if root != STATIC_ROOT {
                    ctx.end_borrow(root);
                }
            }
            Ok(())
        }
        // `expr?` reads its operand as a whole value: the failure payload is
        // moved out onto the return path and the success payload is the
        // expression's value, so a named operand is moved exactly as a bare
        // read of it would be (MK5-S6-M1).
        tir::ExprKind::Try { expr: sub, .. } => {
            check_value(sub, ctx)?;
            // `?`'s failure arm leaves the function, so it has the same drop
            // obligation a written `return` has - and it needs its own plan
            // entry, because codegen renders that early return inside a C
            // statement-expression and never sees a `Stmt::Return` to look the
            // drops up by. Recorded *after* the operand is checked, so a local
            // the operand moved (the `val r = f()?` shape) is not freed here.
            let drops = ctx.live_drops_in_frame();
            if !drops.is_empty() {
                ctx.plan.at_try.insert(expr.span, drops);
            }
            Ok(())
        }
        tir::ExprKind::Template { pieces, .. } => {
            for piece in pieces {
                if let tir::TemplatePiece::Value(v) = piece {
                    check_value(v, ctx)?;
                }
            }
            Ok(())
        }
        tir::ExprKind::StructLit { fields, borrowed_fields, .. } => {
            for (name, v) in fields {
                if !borrowed_fields.contains(name) {
                    check_value(v, ctx)?;
                }
            }
            Ok(())
        }
        // An array literal's elements are moved in exactly as a `datac`'s
        // field initializers are (A-18): the array owns them from here, and
        // `val a = [s1, s2]` is `s1` and `s2` given up.
        tir::ExprKind::ArrayLit(elems) => {
            for e in elems {
                check_value(e, ctx)?;
            }
            Ok(())
        }
        // The repeat form copies its value into every slot, so `infer.rs` only
        // accepts an element that owns nothing and the source is read as a
        // *place*: a `datac` of scalars is copied into the array rather than
        // moved out of the binding it came from.
        tir::ExprKind::ArrayRepeat { value, .. } => check_place(value, ctx),
        // Construction of an enum variant (A-1): each payload value is used
        // exactly as a struct field's initializer is.
        tir::ExprKind::EnumLit { fields, .. } => {
            for v in fields {
                check_value(v, ctx)?;
            }
            Ok(())
        }
        // Reading the tag is a read of the place, not a move of the value.
        tir::ExprKind::EnumTag { expr: base, .. } => check_place_through(base, ctx),
        // A payload read is a *place* read, and an *owned* payload read by
        // value is the same partial move an owned `datac` field read is: the
        // value is copied out and the slot it came from is zeroed, so the
        // enum's own drop frees the payloads the move left behind and nothing
        // where the value now lives. `infer.rs` reads a payload by value only
        // for an owned local, which is the base this accepts; a payload read
        // out of a borrow binds as a borrow and never reaches here, which is
        // why the two refusals below guard the invariant rather than a shape a
        // program can write.
        tir::ExprKind::EnumField { base, variant, field } => {
            check_place_through(base, ctx)?;
            if crate::types::needs_drop(&expr.ty, &ctx.owned_structs) {
                let name = ctx
                    .enum_payload_name(&base.ty, *variant, *field)
                    .expect("an enum field's base names a declared enum");
                let Some(root) = partial_move_base(base, ctx) else {
                    let message = match base.ty {
                        Ty::Ref { .. } => format!(
                            "cannot move the payload '{name}' out of '{}': the value is borrowed, so it is not this frame's to give away",
                            tir_root_ident(base).unwrap_or("a borrow")
                        ),
                        _ => format!(
                            "cannot read the owned payload '{name}' by value: the base is not a local this frame owns, so a partial move cannot take the payload out; match the enum where it is, which borrows the payload"
                        ),
                    };
                    return Err(BorrowError { message, span: expr.span, help: None });
                };
                if crate::types::is_drop_type(&expr.ty, &ctx.drop_types) {
                    return Err(BorrowError {
                        help: None,
                        message: format!(
                            "cannot move the payload '{name}' out of the enum by value: it is a type with a destructor, and a partial move zeroes the slot it came from, so the enum's own drop would run that destructor a second time; match the enum through a borrow, or move the whole value"
                        ),
                        span: expr.span,
                    });
                }
                let proj = Projection::EnumPayload {
                    variant: *variant,
                    field: *field,
                    name,
                };
                ctx.move_field_out(root, proj, expr)?;
            }
            Ok(())
        }
        tir::ExprKind::Assign { target, value } => {
            // A reference binding's assignment is a *retarget*, not a value
            // replacement: the borrow it held ends here and the new value's
            // roots replace it. That is what lets a pointer local be a cursor
            // (`kelp wiki page language/ownership.md`).
            if let tir::ExprKind::Ident(name) = &target.kind {
                if ctx.binds_reference(name) {
                    return ctx.assign_reference(name, value, target.span);
                }
            }
            check_value(value, ctx)?;
            // Read *before* `check_assign_target` reinitializes the binding:
            // `x = f(x)`, where `f` takes its argument by value, moved x into
            // the call, so the value is not there to be freed any more and
            // dropping it would free memory a new owner now holds. Only a
            // binding that is *definitely* moved skips the free: one moved on
            // some path only was cleared there, so the free of the old value
            // is still owed to the paths that kept it (`owned_definitely_moved`).
            let moved_by_value = match &target.kind {
                tir::ExprKind::Ident(name) => ctx.owned_definitely_moved(name),
                _ => false,
            };
            check_assign_target(target, ctx)?;
            // Assigning a fresh value into an owned binding frees the old one
            // (it has no other owner and v1 has no user destructors, so
            // nothing could observe it either way - freeing it is simply the
            // honest thing). Skipped for `b = b`, where the value being
            // stored *is* the old value.
            if let tir::ExprKind::Ident(name) = &target.kind {
                let self_assign = matches!(&value.kind, tir::ExprKind::Ident(src) if src == name);
                let owned = ctx.owned_structs.clone();
                let target_ty = ctx
                    .lookup_mut(name)
                    .map(|state| state.ty.clone());
                let owned = target_ty
                    .map(|ty| crate::types::needs_drop(&ty, &owned))
                    .unwrap_or(false);
                if owned && !self_assign && !moved_by_value {
                    ctx.plan.at_assign.insert(
                        expr.span,
                        vec![DropTarget {
                            // The binding the assignment overwrites is the one
                            // the name reaches here, the same resolution
                            // `codegen` uses for the target of the store.
                            binding: ctx.owned_binding_of(name),
                            name: name.clone(),
                            ty: target.ty.clone(),
                        }],
                    );
                }
            }
            // Roadmap open item 15, cases 2 and 3: an assignment through a
            // dereferenced borrow (`*p = value`, or `p.field = value` where
            // `p` is a borrow - `FieldAccess`'s base need not be a local this
            // frame owns) or to an owned field of a directly-owned local
            // (`h.name = s`) overwrites a place the drop plan cannot express
            // as a `BindingId` at all - there is no declaration to key it to,
            // only an address. `binding: None` here is exactly that: codegen
            // reads the target's own place expression again (its address is
            // idempotent to recompute - nothing between the two evaluations
            // writes it) rather than resolving a binding.
            //
            // No `self_assign`/`moved_by_value` guard is needed here the way
            // the `Ident` arm above needs one. A `Deref` target's pointee can
            // never be read out by whole value when it is non-Copy in the
            // first place (`check_value`'s `Deref` arm refuses that outright),
            // so `value` can never alias `*p` itself. A `FieldAccess` target
            // whose `value` reads that very same field (`h.name = h.name`, or
            // `h.name = wrap(h.name)`) is a partial move of that field, and a
            // partial move's slot is *unconditionally* zeroed at the read
            // (`field_move_sites`/`at_clear` keeps every field move, not only
            // the ones a branch merge left ambiguous - contrast `at_move`,
            // which is why *that* arm needs the guard). So by the time this
            // drop runs, the place holds either the genuine old value or the
            // zero a move already left there, and freeing zero is the same
            // no-op the rest of this plan already relies on throughout (see
            // the module doc's "a move clears its source").
            match &target.kind {
                tir::ExprKind::FieldAccess { .. } | tir::ExprKind::Deref { .. } => {
                    if crate::types::needs_drop(&target.ty, &ctx.owned_structs) {
                        ctx.plan.at_assign.insert(
                            expr.span,
                            vec![DropTarget {
                                binding: None,
                                name: String::new(),
                                ty: target.ty.clone(),
                            }],
                        );
                    }
                }
                _ => {}
            }
            Ok(())
        }
        tir::ExprKind::If { cond, then_block, else_block } => {
            check_value(cond, ctx)?;
            check_if(then_block, else_block, ctx)
        }
        tir::ExprKind::Block(b) => check_block(b, ctx),
        tir::ExprKind::Region { name, elem, body } => {
            ctx.plan.regions.insert(
                expr_id(expr),
                RegionDrop {
                    name: name.clone(),
                    elem: elem.clone(),
                },
            );
            check_block(body, ctx)
        }
        tir::ExprKind::Ref { mutable, expr: inner } => {
            // A borrow reached generically here (not via a `val`/`var`
            // initializer, a call argument, or an assignment to a reference
            // binding; see `check_let_init`, `take_call_arguments` and
            // `assign_reference`) is transient: started and released
            // immediately. It is a `&expr` written inside a struct literal or
            // another expression whose value borrows and immediately uses it.
            if *mutable {
                ctx.check_iterated_mutation(inner)?;
            }
            check_place(inner, ctx)?;
            for root in place_roots(inner, ctx) {
                if root == STATIC_ROOT {
                    continue;
                }
                ctx.start_borrow(&root, *mutable, expr.span)?;
                ctx.end_borrow(&root);
            }
            Ok(())
        }
        tir::ExprKind::Deref { expr: sub } => {
            check_value(sub, ctx)?; // the pointer itself is Copy; reading it never moves anything
            if !is_copy(&expr.ty) {
                return Err(BorrowError {
                    help: None,
                    message: "cannot move a value out through a dereferenced borrow".to_string(),
                    span: expr.span,
                });
            }
            Ok(())
        }
        // A `&dyn` coercion (A-12 slice 4) is transparent to borrowck: the
        // node inside is the `&x` it was written from, and reading the fat pair
        // is a read of the borrow.
        tir::ExprKind::DynCoerce { expr: inner, .. } => check_value(inner, ctx),
        // An owned trait object's coercion (A-12 slice 7) *moves* the box it
        // was written from: the box's allocation becomes the trait object's, so
        // the operand is read as a whole value exactly as a plain read of it
        // would be.
        tir::ExprKind::BoxDynCoerce { expr: inner, .. } => check_value(inner, ctx),
        // Borrowing the trait back out of the box is a read of a *place*, never
        // a move: `BoxDynBorrow` returns a `&dyn` into the box's own storage.
        tir::ExprKind::BoxDynBorrow { expr: inner, .. } => check_place(inner, ctx),
        // `is_empty` reads the box's data pointer, so it is a place read too.
        tir::ExprKind::BoxDynIsEmpty { expr: inner } => check_place(inner, ctx),
        // `asm(...)` (kernel stage 5): an `in` operand reads its value exactly
        // as any other read, and an `out`/`inout` operand writes its place, so
        // that place is taken as a transient exclusive borrow. The operand
        // types are machine words (`infer.rs` refuses anything else), so no
        // move or drop bookkeeping is owed.
        tir::ExprKind::Asm(block) => {
            for op in &block.operands {
                let Some(operand) = &op.operand else { continue };
                match op.kind {
                    AsmOperandKind::In => check_value(operand, ctx)?,
                    _ => {
                        ctx.check_iterated_mutation(operand)?;
                        check_place(operand, ctx)?;
                        for root in place_roots(operand, ctx) {
                            if root == STATIC_ROOT {
                                continue;
                            }
                            ctx.start_borrow(&root, true, operand.span)?;
                            ctx.end_borrow(&root);
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

/// Checks an expression read *through*, as the base of a place chain (`p`
/// in `p.x`, or the pointer operand of `*p`): errors if already moved, but
/// never itself moves or borrows anything. A place a field was moved out of is
/// refused as a whole, so a projection of it is checked by `check_place_through`
/// instead.
pub(crate) fn check_place(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &base.kind {
        tir::ExprKind::Ident(name) => {
            ctx.use_place(name, base.span)?;
            ctx.use_whole_place(name, base.span)
        }
        tir::ExprKind::FieldAccess { base: inner, field } => {
            check_place_through(inner, ctx)?;
            ctx.use_field_place(inner, field, base.span)
        }
        tir::ExprKind::EnumField { base: inner, .. } => check_place_through(inner, ctx),
        // A capture is a place too (A-4 phase E): reading it through a
        // borrow or calling a method on it is exactly what an owned capture
        // is for.
        tir::ExprKind::Capture { .. } => Ok(()),
        tir::ExprKind::Deref { expr: inner } => check_value(inner, ctx),
        _ => check_value(base, ctx),
    }
}

/// Checks a place read *through* to one of its projections: the place as a
/// whole may be partially moved, because only the projection that is read has
/// to be there, and `check_place`'s caller is the arm that checks the
/// projection itself.
pub(crate) fn check_place_through(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &base.kind {
        tir::ExprKind::Ident(name) => ctx.use_place(name, base.span),
        tir::ExprKind::FieldAccess { base: inner, field } => {
            check_place_through(inner, ctx)?;
            ctx.use_field_through(inner, field, base.span)
        }
        _ => check_place(base, ctx),
    }
}

pub(crate) fn check_assign_target(target: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    ctx.check_iterated_mutation(target)?;
    match &target.kind {
        tir::ExprKind::Ident(name) => ctx.reinit_moving(name, target.span),
        tir::ExprKind::FieldAccess { base, field } => {
            check_place_through(base, ctx)?;
            ctx.reinit_field(base, field);
            Ok(())
        }
        tir::ExprKind::Deref { expr: inner } => check_place(inner, ctx),
        _ => Ok(()), // infer.rs already restricts assignment targets to places
    }
}

/// A call's argument list: every direct `Ref` argument (an auto-borrow or
/// an explicit `&expr` written by the caller) is started *before any of
/// them are released*, so `start_borrow`'s own conflict check catches a
/// same-call aliasing violation like `f(&mut c, &c)` - not a separate
/// pass. All of them release together right after.
///
/// The one exception is a call that *returns* a borrow (M6.3): the argument
/// the elided lifetime binds to is left borrowed for the enclosing scope, so
/// the caller cannot move or free the owner while the view lives.
pub(crate) fn check_call(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    let (tir::ExprKind::Call { callee, args }
    | tir::ExprKind::VariadicCall { callee, args, .. }) = &expr.kind
    else {
        unreachable!("check_call is only called on a Call node")
    };
    // A method call's receiver is borrowed exclusively *before* its own
    // arguments are checked, but only as a reservation until they are. That is
    // what lets a tally read the container it is about to mutate
    // (MISSING-n4Bz7): an exclusive borrow taken first would refuse
    // `counts.insert(k, counts.get_or(..) + 1)`, and committing the
    // reservation afterwards still catches a shared borrow that outlives the
    // arguments (`f(&mut c, &c)`).
    let receiver = method_receiver_index(callee, args, ctx);
    let receiver_is_held = receiver.is_some_and(|i| receiver_is_held_mut_ref(&args[i], ctx));
    if let Some(i) = receiver.filter(|_| !receiver_is_held) {
        if let tir::ExprKind::Ref { expr: inner, .. } = &args[i].kind {
            ctx.check_iterated_mutation(inner)?;
        }
        for root in arg_roots(&args[i], ctx) {
            ctx.reserve_exclusive(&root, args[i].span)?;
        }
    }
    let mut taken = take_call_arguments(args, ctx, receiver, receiver_is_held, Some(callee))?;
    if let Some(i) = receiver.filter(|_| !receiver_is_held) {
        for root in arg_roots(&args[i], ctx) {
            ctx.commit_exclusive(&root, args[i].span)?;
        }
    }
    if crate::infer::is_borrowed_value(&expr.ty) {
        let idx = ctx.borrow_root_param_of(callee);
        if let Some(arg) = idx.and_then(|i| args.get(i)) {
            // Held for the scope the result lives in, exactly like
            // `val p = &c` (see `check_let_init`).
            for root in arg_roots(arg, ctx) {
                ctx.register_scope_borrow(&root);
                taken.retain(|r| *r != root);
            }
        }
    }
    release_call_borrows(&taken, ctx);
    Ok(())
}

/// The argument index of a method call's own receiver, when that receiver is
/// borrowed exclusively and so needs the two-phase treatment. Keyed on the
/// `#` in a method's callee key (`Map#insert`), which nothing else has.
pub(crate) fn method_receiver_index(callee: &str, args: &[tir::Expr], ctx: &Ctx) -> Option<usize> {
    if !callee.contains('#') {
        return None;
    }
    let Some(receiver) = args.first() else {
        return None;
    };
    (matches!(receiver.kind, tir::ExprKind::Ref { mutable: true, .. })
        || (ctx.callee_param_is_mut(callee, 0)
            && matches!(receiver.ty, Ty::Ref { mutable: true, .. })))
    .then_some(0)
}

pub(crate) fn receiver_is_held_mut_ref(receiver: &tir::Expr, ctx: &Ctx) -> bool {
    let inner = match &receiver.kind {
        tir::ExprKind::Ref { mutable: true, expr } => expr.as_ref(),
        tir::ExprKind::Ident(_) if matches!(receiver.ty, Ty::Ref { mutable: true, .. }) => receiver,
        _ => return false,
    };
    let tir::ExprKind::Ident(name) = &inner.kind else {
        return false;
    };
    ctx.holds_exclusive_roots(name, &place_roots(inner, ctx))
}

/// One argument list, borrowed for the duration of a call: a `Ref` argument's
/// place is checked and its borrow started, everything else is read as a
/// value. Returns the roots whose borrow has to be released afterwards. Shared
/// by a named call and a call through a function value (A-4), because the two
/// borrow their arguments identically.
///
/// `receiver` is the index whose exclusive borrow was already reserved by
/// `check_call`; its place is still checked here, but no second borrow is
/// started for it.
pub(crate) fn take_call_arguments(
    args: &[tir::Expr],
    ctx: &mut Ctx,
    receiver: Option<usize>,
    receiver_is_held: bool,
    callee: Option<&str>,
) -> Result<Vec<String>> {
    let mut taken = Vec::new();
    let mut held_reborrows = std::collections::BTreeSet::new();
    // Roots an exclusive holder (a `mut` parameter, a `&mut` local) was
    // passed on as a *shared* reborrow, with the argument's span: the same
    // call may not also hand that holder to a `mut` parameter.
    let mut exclusive_reborrows = std::collections::BTreeSet::new();
    let mut shared_reborrows: std::collections::BTreeMap<String, Span> = std::collections::BTreeMap::new();
    for (i, a) in args.iter().enumerate() {
        if let Some((mutable, inner)) = borrowed_node(a) {
            if *mutable && receiver != Some(i) {
                ctx.check_iterated_mutation(inner)?;
            }
            check_place(inner, ctx)?;
            for root in place_roots(inner, ctx) {
                if root == STATIC_ROOT {
                    continue;
                }
                if receiver == Some(i) && receiver_is_held {
                    continue;
                } else if receiver == Some(i) {
                    taken.push(root);
                } else {
                    ctx.start_borrow(&root, *mutable, a.span)?;
                    taken.push(root);
                }
            }
        } else {
            // A `mut` parameter (or a local carrying an `&mut`) is already a
            // reference, so inference passes it on as an `Ident` rather than
            // wrapping it in a fresh `Ref`. It is an exclusive *reborrow*
            // only when the matched callee parameter is itself `mut`; the
            // expression's type alone cannot distinguish that from a shared
            // reborrow. Without registering this case, `g(s, s)` could hand
            // the same mutable parameter to two `mut` parameters at once and
            // evade the ordinary same-call aliasing check, which only sees
            // `Ref` nodes.
            let reborrowed_exclusive = callee
                .is_some_and(|name| ctx.callee_param_is_mut(name, i))
                && matches!(a.ty, Ty::Ref { mutable: true, .. });
            if !reborrowed_exclusive && matches!(a.ty, Ty::Ref { mutable: true, .. }) {
                if let tir::ExprKind::Ident(_) = &a.kind {
                    for root in place_roots(a, ctx) {
                        if root != STATIC_ROOT {
                            shared_reborrows.entry(root).or_insert(a.span);
                        }
                    }
                }
            }
            if reborrowed_exclusive {
                check_place(a, ctx)?;
                let roots = place_roots(a, ctx);
                let holder = match &a.kind {
                    tir::ExprKind::Ident(name) => Some(name.as_str()),
                    _ => None,
                };
                let already_held = holder.is_some_and(|name| ctx.holds_exclusive_roots(name, &roots));
                for root in roots {
                    if root == STATIC_ROOT {
                        continue;
                    }
                    exclusive_reborrows.insert(root.clone());
                    if receiver == Some(i) && !receiver_is_held {
                        taken.push(root);
                        continue;
                    }
                    if already_held {
                        if !held_reborrows.insert(root.clone()) {
                            return Err(BorrowError {
                                help: None,
                                message: format!(
                                    "cannot borrow {}: already borrowed exclusively",
                                    display_root(&root)
                                ),
                                span: a.span,
                            });
                        }
                        continue;
                    }
                    ctx.start_borrow(&root, true, a.span)?;
                    taken.push(root);
                }
            }
            // A scoped literal captures by borrow (`kelp wiki page language/functions.md`),
            // and the borrow lives exactly as long as the call it was written
            // in, so its captured `Ref`s join the same `taken` list a direct
            // borrow argument does: a same-call aliasing `g(&mut s, { k ->
            // use(s) })` is refused here, and every borrow releases together
            // after the call.
            if let tir::ExprKind::ScopedClosureLit { key, captures } = &a.kind {
                for (ci, c) in captures.iter().enumerate() {
                    if let tir::ExprKind::Ref { mutable, expr: inner } = &c.kind {
                        check_place(inner, ctx)?;
                        for root in capture_roots(place_roots(inner, ctx), key, ci, ctx) {
                            if root == STATIC_ROOT {
                                continue;
                            }
                            ctx.start_borrow(&root, *mutable, c.span)?;
                            taken.push(root);
                        }
                    } else if let (tir::ExprKind::Ident(_) | tir::ExprKind::Capture { .. }, Ty::Ref { mutable: true, .. }) = (&c.kind, &c.ty) {
                        // An exclusive borrow the enclosing function holds
                        // (a `mut` parameter, a `var` of `&mut` type) copied
                        // into the environment is a reborrow for the call: it
                        // is exclusive against every other use of its roots
                        // exactly as a captured `&mut place` is
                        // (`kelp wiki page language/functions.md`).
                        for root in capture_roots(place_roots(c, ctx), key, ci, ctx) {
                            if root == STATIC_ROOT {
                                continue;
                            }
                            // An argument that is itself such a pointer is not
                            // a `Ref` node, so `start_borrow` never saw it:
                            // look at every other argument (and every earlier
                            // exclusive capture) of this call directly.
                            let twice = taken.iter().any(|t| roots_overlap(t, &root))
                                || args.iter().enumerate().any(|(j, other)| {
                                    j != i && arg_roots(other, ctx).iter().any(|r| roots_overlap(r, &root))
                                });
                            if twice {
                                return Err(BorrowError {
                                    help: None,
                                    message: format!(
                                        "cannot borrow {} as exclusive: this call also uses it, and a block that writes through a captured exclusive borrow needs the call's other arguments to leave it alone",
                                        display_root(&root)
                                    ),
                                    span: c.span,
                                });
                            }
                            ctx.start_borrow(&root, true, c.span)?;
                            taken.push(root);
                        }
                    }
                }
            }
            // A reference is Copy, so this has no move effect; keep the
            // normal value walk for nested expressions and diagnostics.
            check_value(a, ctx)?;
        }
    }
    for (root, span) in shared_reborrows {
        if exclusive_reborrows.iter().any(|r| roots_overlap(r, &root)) {
            return Err(BorrowError {
                help: None,
                message: format!(
                    "cannot borrow {}: already borrowed exclusively",
                    display_root(&root)
                ),
                span,
            });
        }
    }
    Ok(taken)
}

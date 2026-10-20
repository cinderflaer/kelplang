use super::*;

/// The owner roots a borrow is ultimately derived from. A borrow-returning call
/// chains back through its own elided lifetime, so `trim_start(s)` roots at
/// `s`, and `f(s.as_str())` roots at whatever `s` was borrowed from. `None`
/// when the value has no such root.
pub(crate) fn borrow_root(expr: &tir::Expr, ctx: &Ctx) -> Option<Vec<String>> {
    match &expr.kind {
        tir::ExprKind::StringLit(_) => Some(vec![STATIC_ROOT.to_string()]),
        // A binding that *is* a reference carries the roots it was made from,
        // so a cursor resolves back through it to the place its value borrows
        // from, and a pattern binding keeps the place its payload came from
        // (`set_roots` has the reasoning).
        tir::ExprKind::Ident(name) => Some(ctx.roots_of(name)),
        // A field read straight off a local is rooted at that field, the way
        // `place_roots` roots the borrow that starts it: the binding that
        // holds the result must pin the same part of the owner.
        tir::ExprKind::FieldAccess { .. } | tir::ExprKind::Ref { .. } | tir::ExprKind::Deref { .. }
            if tir_root_ident(expr).is_some() && field_suffix(expr).is_some() =>
        {
            Some(place_roots(expr, ctx))
        }
        tir::ExprKind::FieldAccess { base, .. }
        | tir::ExprKind::EnumField { base, .. }
        | tir::ExprKind::Ref { expr: base, .. }
        | tir::ExprKind::DynCoerce { expr: base, .. }
        | tir::ExprKind::BoxDynCoerce { expr: base, .. }
        | tir::ExprKind::BoxDynBorrow { expr: base, .. }
        | tir::ExprKind::BoxDynIsEmpty { expr: base }
        | tir::ExprKind::Deref { expr: base } => borrow_root(base, ctx),
        tir::ExprKind::Call { callee, args }
        | tir::ExprKind::VariadicCall { callee, args, .. } => {
            let idx = ctx.borrow_root_param_of(callee)?;
            borrow_root(args.get(idx)?, ctx)
        }
        // `if`/`when` in value position: the joined value may have come from
        // either branch, so its root is the union of the arms' roots, with a
        // `str` literal's static storage dropped as soon as a real owner is in
        // the set. That is what
        // `fun f(v: &J) => &str = when v { S(s) -> s.as_str()  N -> "n" }`
        // needs: one owner (`v`) and one static string.
        tir::ExprKind::Block(b) => block_tail_root(b, ctx),
        tir::ExprKind::If {
            then_block,
            else_block,
            ..
        } => {
            let then_root = block_tail_root(then_block, ctx)?;
            let else_root = block_tail_root(else_block.as_ref()?, ctx)?;
            Some(merge_root_sets(&then_root, &else_root))
        }
        _ => None,
    }
}

pub(crate) fn block_tail_root(block: &tir::Block, ctx: &Ctx) -> Option<Vec<String>> {
    borrow_root(block.tail.as_deref()?, ctx)
}

/// The owner roots a place expression borrows from: the binding at the bottom
/// of the chain, resolved through that binding's own provenance. This is what
/// makes a borrow made through a cursor a borrow of the *owner* rather than of
/// the cursor (`kelp wiki page language/ownership.md`).
pub(crate) fn place_roots(expr: &tir::Expr, ctx: &Ctx) -> Vec<String> {
    match tir_root_ident(expr) {
        Some(name) => {
            let roots = ctx.roots_of(name);
            // A field of a struct is a part of its owner, so a borrow of it is
            // rooted at that part and does not overlap a borrow of another
            // field (`kelp wiki page language/functions.md`). Anything the chain passes
            // through that is not a plain struct field (an enum payload, a
            // dereference, a trait object) stops the path where it is.
            match field_suffix(expr) {
                Some(suffix) if !suffix.is_empty() => roots
                    .into_iter()
                    .map(|r| if r == STATIC_ROOT || r.starts_with("pd#") { r } else { format!("{r}{suffix}") })
                    .collect(),
                _ => roots,
            }
        }
        // A captured binding of a scoped block is a place too, rooted at the
        // capture (`#cap0`): a block nested in this one captures it again, and
        // that has to be checked against the rest of the call it is written in
        // exactly as a local would be.
        None => match capture_base(expr) {
            Some(index) => {
                let suffix = field_suffix(expr).unwrap_or_default();
                vec![format!("#cap{index}{suffix}")]
            }
            None => Vec::new(),
        },
    }
}

/// The capture a place chain bottoms out at, looking through borrows,
/// dereferences and field reads.
pub(crate) fn capture_base(expr: &tir::Expr) -> Option<usize> {
    match &expr.kind {
        tir::ExprKind::Capture { index } => Some(*index),
        tir::ExprKind::FieldAccess { base, .. } => capture_base(base),
        tir::ExprKind::Deref { expr: inner } | tir::ExprKind::Ref { expr: inner, .. } => capture_base(inner),
        _ => None,
    }
}

/// `/items/head` for `s.items.head` when every step is a struct field read
/// straight off a local, `None` when the chain is anything else (the whole
/// owner is then the root).
pub(crate) fn field_suffix(expr: &tir::Expr) -> Option<String> {
    match &expr.kind {
        tir::ExprKind::Ident(_) | tir::ExprKind::Capture { .. } => Some(String::new()),
        tir::ExprKind::FieldAccess { base, field } => {
            let inner = field_suffix(base)?;
            let base_ty = match &base.ty {
                Ty::Ref { inner, .. } => &**inner,
                other => other,
            };
            if !matches!(base_ty, Ty::Struct(_)) {
                return None;
            }
            Some(format!("{inner}{PATH_SEP}{field}"))
        }
        // `*p` and `&x` read through to the same storage as their operand.
        tir::ExprKind::Deref { expr: inner } | tir::ExprKind::Ref { expr: inner, .. } => field_suffix(inner),
        _ => None,
    }
}

/// The one root a value has when it may come from either of two branches: the
/// branches have to agree, **except** that a `str` literal's storage is static
/// (`STATIC_ROOT`), so a branch whose value is a literal needs no owner pinned
/// and does not conflict with a branch that borrows one.
///
/// ```kelp
/// fun f(v: &J) => &str = when v {
///     S(s) -> s.as_str()   // borrows `v`
///     N -> "n"             // static: needs no owner
/// }
/// ```
///
/// One owner plus a literal is a value with a single owner. Two different
/// non-static roots are both kept: the joined value may have come from either
/// owner, so both stay pinned (`kelp wiki page language/ownership.md`).
pub(crate) fn merge_root_sets(left: &[String], right: &[String]) -> Vec<String> {
    union_roots(left, right)
}

pub fn check_program(program: &tir::Program) -> std::result::Result<DropPlan, Diagnostic> {
    let mut plan = DropPlan::default();
    let owned = crate::types::owned_types(&program.structs, &program.enums, &program.drop_types);
    // Every `enum` by name, so a payload move can name the field it moved out
    // (`Ty::Struct` carries only the name; the two indices are what the `tir`
    // node has). Instantiations are in here beside the templates, under the
    // name the field expression's own type spells.
    let enums: HashMap<String, EnumDef> = program
        .enums
        .iter()
        .map(|e| (e.name.clone(), e.clone()))
        .collect();
    // Every callee whose result is a borrow, with the parameter its elided
    // lifetime binds to. Builtins (`String.as_str`) join the same table, so
    // `check_call` has one rule for both.
    let mut fn_borrow_root: HashMap<String, Option<usize>> = HashMap::new();
    let mut fn_mut_params: HashMap<String, Vec<bool>> = HashMap::new();
    let mut capture_footprints: HashMap<String, Vec<Option<Vec<String>>>> = HashMap::new();
    for f in &program.functions {
        if let Some(env) = &f.env {
            capture_footprints.insert(
                f.name.clone(),
                (0..env.fields.len()).map(|i| capture_footprint(&f.body, i)).collect(),
            );
        }
    }
    for f in &program.functions {
        if crate::infer::is_borrowed_value(&f.ret) {
            fn_borrow_root.insert(f.name.clone(), crate::infer::borrow_root_param(&f.params));
        }
        fn_mut_params.insert(
            f.name.clone(),
            f.params
                .iter()
                .map(|(_, ty)| matches!(ty, Ty::Ref { mutable: true, .. }))
                .collect(),
        );
    }
    // The passdown globals, as `(hidden name, value type)`: a function that
    // does *not* take one as a hidden parameter (`main`) still reaches the
    // storage, so its reads and borrows have to be tracked against the same
    // root as everyone else's (M6.4).
    let globals: Vec<(String, Ty)> = program
        .pds
        .iter()
        .map(|p| (p.name.clone(), p.value_ty.clone()))
        .collect();
    for f in &program.functions {
        let function = check_function(f, &owned, &program.drop_types, &fn_borrow_root, &fn_mut_params, &capture_footprints, &globals, &enums).map_err(|e| {
            let mut diag = Diagnostic::new(e.message, e.span, f.module.clone());
            diag.help = e.help;
            diag
        })?;
        plan.functions.insert(f.name.clone(), function);
    }
    Ok(plan)
}

pub(crate) fn check_function(
    f: &tir::Function,
    owned_structs: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
    fn_borrow_root: &HashMap<String, Option<usize>>,
    fn_mut_params: &HashMap<String, Vec<bool>>,
    capture_footprints: &HashMap<String, Vec<Option<Vec<String>>>>,
    globals: &[(String, Ty)],
    enums: &HashMap<String, EnumDef>,
) -> Result<FnDrops> {
    let mut ctx = Ctx {
        scopes: vec![Scope::default()],
        plan: FnDrops::default(),
        owned_structs: owned_structs.clone(),
        drop_types: drop_types.clone(),
        fn_borrow_root: fn_borrow_root.clone(),
        fn_mut_params: fn_mut_params.clone(),
        capture_footprints: capture_footprints.clone(),
        // A borrowed return type borrows from the first reference
        // parameter - `infer.rs` has already rejected a function that
        // returns one without taking any.
        ret_borrow_param: if crate::infer::is_borrowed_value(&f.ret) {
            crate::infer::borrow_root_param(&f.params).map(|i| f.params[i].0.clone())
        } else {
            None
        },
        ret_is_borrow: crate::infer::is_borrowed_value(&f.ret),
        ret_ty: f.ret.clone(),
        move_sites: HashMap::new(),
        field_move_sites: HashMap::new(),
        enums: enums.clone(),
        maybe_moved: std::collections::HashSet::new(),
        iterating: Vec::new(),
        loop_marks: Vec::new(),
        pins: HashMap::new(),
    };
    for (index, (name, ty)) in f.params.iter().enumerate() {
        ctx.define(name, ty);
        // A parameter owns its value only when it was declared `move T`
        // (A-4): the resolved type is then the bare owned type, not a
        // borrow. Registering it here is what frees it at the end of the
        // frame (or on the `return` path), and it is the only thing the
        // callee's side of a move needs.
        if crate::types::needs_drop(ty, &ctx.owned_structs) {
            ctx.define_owned(BindingId::Param(index), name, ty);
        }
    }
    // A passdown the function does not receive as a hidden parameter is
    // still reachable storage (`main` writes the globals directly), so it
    // gets a binding of its own value type - unless a parameter of the same
    // hidden name already claimed the slot, which is the normal case for
    // every function other than `main`.
    for (hidden, value_ty) in globals {
        if !ctx.knows(hidden) {
            ctx.define(hidden, value_ty);
        }
    }
    check_block(&f.body, &mut ctx)?;

    // A tail expression is the function's returned value, so it is bound by
    // the same elision rule an explicit `return` is - unless it diverges, in
    // which case the function does not return through it at all. `infer.rs`
    // has always said so for the *type*; this is the same predicate, so the
    // two passes cannot disagree about which expression is the returned value.
    if let Some(tail) = &f.body.tail {
        if !crate::infer::tail_diverges(tail) {
            ctx.check_return_borrow(tail)?;
        }
    }

    // Anything still owned in the frame at the end belongs to the function
    // body's own block (parameters live as long as the body does). The block
    // already has an entry when the body declared an owned local of its own,
    // because `pop_scope` wrote that local's drop at the same identity, so
    // this is a merge: the body's own locals are dropped first, innermost
    // scope first the way `live_drops_in_frame` walks, and the parameters
    // follow them. Replacing the entry instead dropped the body's local
    // nowhere and leaked it (BUG-tV9qH).
    let leftover = ctx.live_drops_in_frame();
    if !leftover.is_empty() {
        ctx.plan
            .at_block_end
            .entry(block_id(&f.body))
            .or_default()
            .extend(leftover);
    }

    // The move sites whose binding some path leaves live. Each of them has to
    // clear its binding's slot after copying the value out, because that
    // binding is still in a drop set - at a block end, a `return`, a `?` or an
    // assignment - and the free has to find nothing on the path that moved.
    // A binding moved on *every* path is in no drop set, so its move sites are
    // deliberately left out: clearing a slot nothing will ever free is dead
    // code, and an `if`/`loop` merge is the only thing that can leave one
    // `MaybeMoved`.
    let clears: Vec<NodeId> = ctx
        .move_sites
        .iter()
        .filter(|(binding, _)| ctx.maybe_moved.contains(binding))
        .flat_map(|(_, sites)| sites.iter().copied())
        .collect();
    ctx.plan.at_move.extend(clears);
    ctx.plan.moved_any.extend(
        ctx.move_sites
            .values()
            .flat_map(|sites| sites.iter().copied()),
    );
    // A partial move always clears: the binding it came out of stays in every
    // drop set it was already in, so each of these clears a slot a drop reaches.
    ctx.plan.at_clear = ctx.field_move_sites.clone();
    Ok(ctx.plan)
}

pub(crate) fn check_block(block: &tir::Block, ctx: &mut Ctx) -> Result<()> {
    ctx.push_scope();
    let result = (|| {
        for (index, stmt) in block.stmts.iter().enumerate() {
            check_stmt(block, index, stmt, ctx)?;
            ctx.release_expired(index);
        }
        if let Some(tail) = &block.tail {
            check_value(tail, ctx)?;
        }
        Ok(())
    })();
    ctx.pop_scope(block, diverges(block));
    result
}

pub(crate) fn check_stmt(block: &tir::Block, index: usize, stmt: &tir::Stmt, ctx: &mut Ctx) -> Result<()> {
    match stmt {
        tir::Stmt::Let { name, ty, init } => {
            // A borrow taken *inside* the initializer is dead once the value is
            // bound - unless the bound value **is** the borrow. `val back =
            // str.parse_f64(cand.as_str())` borrows `cand` for the length of
            // one call and keeps nothing of it (`Result<f64, _>` owns all its
            // data), so the borrow must end with the statement; holding it to
            // the end of the block is what made `return cand` a few lines
            // later fail with "cannot move 'cand' while it is borrowed".
            //
            // `val v = l.get(0)`, by contrast, binds the borrow itself, and
            // the caller has to keep the owner pinned for as long as `v`
            // lives - so the distinction is the *binding's* type, not the
            // initializer's shape. This is deliberately the same rule the
            // `Stmt::Expr` arm below applies to a statement's temporary, one
            // level up: lexical lifetimes with a statement-sized exception for
            // borrows that provably do not escape.
            let mark = ctx.scopes.len();
            let at = ctx.scopes.last().map(|s| s.scope_borrows.len()).unwrap_or(0);
            let result = check_let_init(init, ctx);
            result?;
            ctx.define(name, ty);
            // A reference binding keeps its provenance, so `borrow_root` can
            // follow a pattern binding back to the place its payload came
            // from, and a cursor back to the owner it points into. Recorded
            // *after* `define` (the entry has to exist) and only for
            // references (`set_roots` has the reasoning).
            if is_reference_ty(ty) {
                let roots = borrow_root(init, ctx).unwrap_or_default();
                if ctx.scopes.len() == mark {
                    let last = last_use_index(block, index, name);
                    let mutable = matches!(ty, Ty::Ref { mutable: true, .. });
                    ctx.hold(name, &roots, mutable, at, last, init.span)?;
                }
                ctx.set_roots(name, roots);
            } else if ctx.scopes.len() == mark {
                ctx.release_borrows_since(at);
            }
            // A block box (`let x = name.box()?`) is owned by its region, which
            // releases the element, so the binding is not freed on its own.
            if crate::types::needs_drop(ty, &ctx.owned_structs) && !is_block_box_init(init) {
                ctx.define_owned(BindingId::Local(stmt_id(stmt)), name, ty);
            }
            Ok(())
        }
        tir::Stmt::While { cond, body, iterating } => {
            let mark = ctx.iterating.len();
            if let Some(it) = iterating {
                ctx.iterating.push(it.clone());
            }
            let result = check_while(cond, body, ctx);
            ctx.iterating.truncate(mark);
            result
        }
        tir::Stmt::Return { value, span } => {
            if let Some(v) = value {
                check_value(v, ctx)?;
                ctx.check_return_borrow(v)?;
            }
            // Planned after the value is checked: a local being returned is
            // moved by that check and so is not dropped here.
            let drops = ctx.live_drops_in_frame();
            if !drops.is_empty() {
                ctx.plan.at_return.insert(*span, drops);
            }
            Ok(())
        }
        // A `break`/`continue` frees every owned local declared inside the
        // loop it leaves, which is `live_drops_since_loop`. The innermost loop
        // is the one at the top of `loop_marks`, pushed by `check_while`.
        tir::Stmt::Break { span } => {
            let drops = live_drops_since_loop(ctx);
            if !drops.is_empty() {
                ctx.plan.at_break.insert(*span, drops);
            }
            Ok(())
        }
        tir::Stmt::Continue { span } => {
            let drops = live_drops_since_loop(ctx);
            if !drops.is_empty() {
                ctx.plan.at_continue.insert(*span, drops);
            }
            Ok(())
        }
        tir::Stmt::Expr(e) => {
            // A borrow produced by a statement's *temporary* - `print(l.get(0))`,
            // where the view is consumed inside the statement - is released at
            // the end of that statement, not at the end of the block. Scope
            // lifetimes are otherwise lexical (plan's known limitation: no
            // NLL), and holding every temporary to the end of the block makes
            // ordinary list code fail (`print(l.get(0)); l.push(x)` would
            // conflict with a view that is already dead). A *bound* borrow
            // (`val p = l.get(0)`) still lives as long as its binding, because
            // a `Let` is not this arm - it is the arm above, which applies the
            // same rule to the borrows an initializer takes.
            let mark = ctx.scopes.len();
            let at = ctx.scopes.last().map(|s| s.scope_borrows.len()).unwrap_or(0);
            let checked = check_value(e, ctx);
            if ctx.scopes.len() == mark {
                // Only the untagged temporaries; a borrow a cursor assignment
                // stored has a holder and survives the statement.
                ctx.release_unheld_since(at);
            }
            checked
        }
    }
}

/// The last statement index of `block` at or after `from` whose statement
/// mentions `name`, or `block.stmts.len()` when the block's tail does. `from`
/// itself is the floor: a binding nothing later uses is dead right after its
/// own declaration.
///
/// A use inside a loop body or an `if` arm is a use of the statement that
/// contains it, and that statement's index is what this returns. That is the
/// loop rule and the branch rule together (`kelp wiki page language/ownership.md`): a borrow used in a
/// loop body stays live for the whole loop, including the statements before
/// the use, because the next iteration reaches them.
pub(crate) fn last_use_index(block: &tir::Block, from: usize, name: &str) -> usize {
    let mut last = from;
    for (index, stmt) in block.stmts.iter().enumerate().skip(from + 1) {
        if stmt_mentions(stmt, name) {
            last = index;
        }
    }
    if block.tail.as_ref().map_or(false, |tail| expr_mentions(tail, name)) {
        last = block.stmts.len();
    }
    last
}

pub(crate) fn stmt_mentions(stmt: &tir::Stmt, name: &str) -> bool {
    match stmt {
        tir::Stmt::Let { init, .. } => expr_mentions(init, name),
        tir::Stmt::While { cond, body, .. } => {
            expr_mentions(cond, name) || block_mentions(body, name)
        }
        tir::Stmt::Return { value, .. } => {
            value.as_ref().map_or(false, |v| expr_mentions(v, name))
        }
        tir::Stmt::Break { .. } | tir::Stmt::Continue { .. } => false,
        tir::Stmt::Expr(e) => expr_mentions(e, name),
    }
}

pub(crate) fn block_mentions(block: &tir::Block, name: &str) -> bool {
    block.stmts.iter().any(|s| stmt_mentions(s, name))
        || block.tail.as_ref().map_or(false, |e| expr_mentions(e, name))
}

/// Whether the expression names the binding anywhere in it. Spelled out per
/// `ExprKind` rather than lowered, because the walk has to see through every
/// position a use can hide in: a nested block, a loop, a template piece, an
/// `asm` operand, a closure's capture list. Reading a binding is what extends
/// its borrow's life, and every read reaches this node somewhere.
pub(crate) fn expr_mentions(expr: &tir::Expr, name: &str) -> bool {
    match &expr.kind {
        tir::ExprKind::Ident(n) => n == name,
        tir::ExprKind::Unary { expr, .. }
        | tir::ExprKind::FieldAccess { base: expr, .. }
        | tir::ExprKind::EnumTag { expr, .. }
        | tir::ExprKind::EnumField { base: expr, .. }
        | tir::ExprKind::Try { expr, .. }
        | tir::ExprKind::Deref { expr }
        | tir::ExprKind::Ref { expr, .. }
        | tir::ExprKind::DynCoerce { expr, .. }
        | tir::ExprKind::BoxDynCoerce { expr, .. }
        | tir::ExprKind::BoxDynBorrow { expr, .. }
        | tir::ExprKind::BoxDynIsEmpty { expr }
        | tir::ExprKind::FnToClosure { func: expr }
        | tir::ExprKind::ArrayRepeat { value: expr } => expr_mentions(expr, name),
        tir::ExprKind::Binary { lhs, rhs, .. }
        | tir::ExprKind::Assign { target: lhs, value: rhs } => {
            expr_mentions(lhs, name) || expr_mentions(rhs, name)
        }
        tir::ExprKind::Call { args, .. } | tir::ExprKind::VariadicCall { args, .. } => {
            args.iter().any(|a| expr_mentions(a, name))
        }
        tir::ExprKind::IndirectCall { callee, args }
        | tir::ExprKind::DynCall { receiver: callee, args, .. } => {
            expr_mentions(callee, name) || args.iter().any(|a| expr_mentions(a, name))
        }
        tir::ExprKind::StructLit { fields, .. } => {
            fields.iter().any(|(_, v)| expr_mentions(v, name))
        }
        tir::ExprKind::EnumLit { fields, .. } | tir::ExprKind::ArrayLit(fields) => {
            fields.iter().any(|v| expr_mentions(v, name))
        }
        tir::ExprKind::If { cond, then_block, else_block } => {
            expr_mentions(cond, name)
                || block_mentions(then_block, name)
                || else_block.as_ref().map_or(false, |b| block_mentions(b, name))
        }
        tir::ExprKind::Block(b) => block_mentions(b, name),
        tir::ExprKind::Region { body, .. } => block_mentions(body, name),
        tir::ExprKind::ClosureLit { captures, .. }
        | tir::ExprKind::ScopedClosureLit { captures, .. } => {
            captures.iter().any(|c| expr_mentions(c, name))
        }
        tir::ExprKind::Template { pieces, .. } => pieces.iter().any(|p| match p {
            tir::TemplatePiece::Value(v) => expr_mentions(v, name),
            tir::TemplatePiece::Text(_) => false,
        }),
        tir::ExprKind::Asm(block) => block.operands.iter().any(|op| {
            op.operand.as_ref().map_or(false, |e| expr_mentions(e, name))
        }),
        tir::ExprKind::IntLit(_)
        | tir::ExprKind::BlockBox { .. }
        | tir::ExprKind::CharLit(_)
        | tir::ExprKind::FloatLit(_)
        | tir::ExprKind::StringLit(_)
        | tir::ExprKind::BoolLit(_)
        | tir::ExprKind::StaticRef(_)
        | tir::ExprKind::FnRef(_)
        | tir::ExprKind::Capture { .. } => false,
    }
}


/// Which parts of the capture at `index` a scoped block's body uses, as field
/// paths (`/count`), or `None` when it uses the capture as a whole (passes it
/// on, reads it by value, takes a field of something that is not a plain
/// struct). A block that only writes `s.count` captures that field of `s`, so a
/// call that borrows another field (`s.items`) does not overlap it
/// (MISSING-Sw9Hd, `kelp wiki page language/functions.md`). Sound because the block's body
/// is the only code that holds the capture, and it reaches nothing outside the
/// paths this returns.
pub(crate) fn capture_footprint(body: &tir::Block, index: usize) -> Option<Vec<String>> {
    let mut whole = false;
    let mut paths: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    footprint_block(body, index, &mut whole, &mut paths);
    if whole || paths.is_empty() {
        None
    } else {
        Some(paths.into_iter().collect())
    }
}

/// A field chain that bottoms out at capture `index`, with the path of fields
/// read off it (`/items/head`), looking through borrows and dereferences.
pub(crate) fn capture_chain(expr: &tir::Expr, index: usize) -> Option<String> {
    match &expr.kind {
        tir::ExprKind::Capture { index: i } if *i == index => Some(String::new()),
        tir::ExprKind::FieldAccess { base, field } => {
            let inner = capture_chain(base, index)?;
            let base_ty = match &base.ty {
                Ty::Ref { inner, .. } => &**inner,
                other => other,
            };
            if !matches!(base_ty, Ty::Struct(_)) {
                return None;
            }
            Some(format!("{inner}{PATH_SEP}{field}"))
        }
        tir::ExprKind::Deref { expr: inner } | tir::ExprKind::Ref { expr: inner, .. } => capture_chain(inner, index),
        _ => None,
    }
}

pub(crate) fn footprint_block(
    block: &tir::Block,
    index: usize,
    whole: &mut bool,
    paths: &mut std::collections::BTreeSet<String>,
) {
    for stmt in &block.stmts {
        match stmt {
            tir::Stmt::Let { init, .. } => footprint_expr(init, index, whole, paths),
            tir::Stmt::While { cond, body, .. } => {
                footprint_expr(cond, index, whole, paths);
                footprint_block(body, index, whole, paths);
            }
            tir::Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    footprint_expr(v, index, whole, paths);
                }
            }
            tir::Stmt::Break { .. } | tir::Stmt::Continue { .. } => {}
            tir::Stmt::Expr(e) => footprint_expr(e, index, whole, paths),
        }
    }
    if let Some(tail) = &block.tail {
        footprint_expr(tail, index, whole, paths);
    }
}

pub(crate) fn footprint_expr(
    expr: &tir::Expr,
    index: usize,
    whole: &mut bool,
    paths: &mut std::collections::BTreeSet<String>,
) {
    // The longest field chain off the capture is one use of one part of it.
    if let Some(path) = capture_chain(expr, index) {
        if path.is_empty() {
            *whole = true;
        } else {
            paths.insert(path);
        }
        return;
    }
    let mut go = |e: &tir::Expr| footprint_expr(e, index, whole, paths);
    match &expr.kind {
        tir::ExprKind::Unary { expr, .. }
        | tir::ExprKind::FieldAccess { base: expr, .. }
        | tir::ExprKind::EnumTag { expr, .. }
        | tir::ExprKind::EnumField { base: expr, .. }
        | tir::ExprKind::Try { expr, .. }
        | tir::ExprKind::Deref { expr }
        | tir::ExprKind::Ref { expr, .. }
        | tir::ExprKind::DynCoerce { expr, .. }
        | tir::ExprKind::BoxDynCoerce { expr, .. }
        | tir::ExprKind::BoxDynBorrow { expr, .. }
        | tir::ExprKind::BoxDynIsEmpty { expr }
        | tir::ExprKind::FnToClosure { func: expr }
        | tir::ExprKind::ArrayRepeat { value: expr } => go(expr),
        tir::ExprKind::Binary { lhs, rhs, .. } | tir::ExprKind::Assign { target: lhs, value: rhs } => {
            go(lhs);
            go(rhs);
        }
        tir::ExprKind::Call { args, .. } | tir::ExprKind::VariadicCall { args, .. } => {
            for a in args {
                go(a);
            }
        }
        tir::ExprKind::IndirectCall { callee, args } | tir::ExprKind::DynCall { receiver: callee, args, .. } => {
            go(callee);
            for a in args {
                go(a);
            }
        }
        tir::ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                go(v);
            }
        }
        tir::ExprKind::EnumLit { fields, .. } | tir::ExprKind::ArrayLit(fields) => {
            for v in fields {
                go(v);
            }
        }
        tir::ExprKind::If { cond, then_block, else_block } => {
            go(cond);
            footprint_block(then_block, index, whole, paths);
            if let Some(b) = else_block {
                footprint_block(b, index, whole, paths);
            }
        }
        tir::ExprKind::Block(b) => footprint_block(b, index, whole, paths),
        tir::ExprKind::Region { body, .. } => footprint_block(body, index, whole, paths),
        tir::ExprKind::ClosureLit { captures, .. } | tir::ExprKind::ScopedClosureLit { captures, .. } => {
            for c in captures {
                go(c);
            }
        }
        tir::ExprKind::Template { pieces, .. } => {
            for p in pieces {
                if let tir::TemplatePiece::Value(v) = p {
                    go(v);
                }
            }
        }
        tir::ExprKind::Asm(block) => {
            for op in &block.operands {
                if let Some(e) = &op.operand {
                    go(e);
                }
            }
        }
        // The capture used bare (any other node that reads it) is the whole of
        // it; a capture index that is not ours, a literal, a name: nothing.
        tir::ExprKind::Capture { .. }
        | tir::ExprKind::Ident(_)
        | tir::ExprKind::IntLit(_)
        | tir::ExprKind::BlockBox { .. }
        | tir::ExprKind::CharLit(_)
        | tir::ExprKind::FloatLit(_)
        | tir::ExprKind::StringLit(_)
        | tir::ExprKind::BoolLit(_)
        | tir::ExprKind::StaticRef(_)
        | tir::ExprKind::FnRef(_) => {}
    }
}

//! `defer` lowering (kernel stage 6, `plan/defer.md`).
//!
//! `defer EXPR` is a statement whose action runs when the enclosing block is
//! left. This pass rewrites it away before name resolution, the type checker,
//! the borrow checker and both backends run, by duplicating the action at every
//! ordinary exit of its block: the end of the block, and every `return` in it.
//! Each copy is ordinary code, so inference, move checking, drop placement and
//! both generators apply to it unchanged, and no later pass learns a new form.
//!
//! Three rules the walk enforces rather than lowers:
//!
//! - a `defer` and the `?` operator cannot share a block, because `?` leaves
//!   the enclosing *function* and the duplication has no exit for that;
//! - a deferred action may not contain `return` (there is no caller to return
//!   to, and the block is already being left);
//! - a deferred action sees only the locals already in scope where the `defer`
//!   is written, so naming a local declared later in the block is refused.
//!
//! Every synthesized statement carries the `defer` statement's span, so a
//! diagnostic about the duplicated action names the `defer` and not the exit it
//! was spliced before. Synthesized names start with `#`, which cannot appear in
//! a lexed identifier, so they cannot collide with a written binding.

use std::collections::HashSet;

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::lexer::Span;

const RET_PREFIX: &str = "#defer_ret";
const TMP_PREFIX: &str = "#defer_tmp";
const TAIL_PREFIX: &str = "#defer_tail";

/// Lowers every `defer` in `program`, attributing a refusal to `module`.
pub fn lower(program: &mut Program, module: &str) -> Result<(), Diagnostic> {
    let mut lowerer = Lowerer {
        module: module.to_string(),
        counter: 0,
    };
    for f in &mut program.functions {
        lowerer.function(f)?;
    }
    for p in &mut program.pds {
        let empty = Chain::default();
        lowerer.expr(&mut p.init, &empty, &HashSet::new())?;
    }
    for c in &mut program.consts {
        let empty = Chain::default();
        lowerer.expr(&mut c.init, &empty, &HashSet::new())?;
    }
    for s in &mut program.statics {
        let empty = Chain::default();
        lowerer.expr(&mut s.init, &empty, &HashSet::new())?;
    }
    Ok(())
}

/// One deferred action, ready to be spliced at an exit. `span` is the `defer`
/// statement's own span, which every synthesized copy reports.
#[derive(Clone)]
struct Cleanup {
    expr: Expr,
    span: Span,
}

/// The actions an exit has to run, already in execution order: the innermost
/// block's defers first, each newest first, then the enclosing blocks'.
#[derive(Clone, Default)]
struct Chain(Vec<Cleanup>);

impl Chain {
    /// The actions active after the `before` defers of the current block have
    /// run, followed by what an exit out of the enclosing blocks owes.
    fn exit(before: &[Cleanup], outer: &Chain) -> Chain {
        let mut actions: Vec<Cleanup> = before.iter().rev().cloned().collect();
        actions.extend(outer.0.iter().cloned());
        Chain(actions)
    }
}

struct Lowerer {
    module: String,
    counter: usize,
}

impl Lowerer {
    fn fresh(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    fn function(&mut self, f: &mut FunctionDecl) -> Result<(), Diagnostic> {
        let empty = Chain::default();
        match &mut f.body {
            FunctionBody::Block(block) => self.block(block, &empty, &HashSet::new(), true),
            FunctionBody::Expr(expr) => self.expr(expr, &empty, &HashSet::new()),
            FunctionBody::Extern | FunctionBody::Signature => Ok(()),
        }
    }

    /// Rebuilds one block, splicing the cleanup its defers owe at every exit
    /// and rewriting nested blocks on the way.
    ///
    /// `is_function_body` decides what the block's last expression means. A
    /// function body's tail *is* the returned value, so it is lowered as an
    /// explicit `return` — one return site the cleanup and the elided-borrow
    /// rule see together. Any other block's tail is a value, so it is preserved
    /// through a synthesized binding while the cleanup runs after it.
    fn block(
        &mut self,
        block: &mut Block,
        outer: &Chain,
        not_yet: &HashSet<String>,
        is_function_body: bool,
    ) -> Result<(), Diagnostic> {
        let n = block.stmts.len();
        let has_defer = block.stmts.iter().any(|s| matches!(s, Stmt::Defer { .. }));
        if has_defer {
            if let Some(try_span) = first_try_in_block(block) {
                let defer_span = block
                    .stmts
                    .iter()
                    .find_map(|s| match s {
                        Stmt::Defer { span, .. } => Some(*span),
                        _ => None,
                    })
                    .unwrap_or(try_span);
                return Err(Diagnostic::new(
                    "defer with an early `?` exit is not supported yet".to_string(),
                    defer_span,
                    self.module.clone(),
                )
                .with_note(
                    "this `?` leaves the function, which would skip the deferred action".to_string(),
                    try_span,
                    self.module.clone(),
                ));
            }
        }

        // What each statement declares, so a `defer` can be refused for naming
        // a local that is only declared later in the block.
        let mut declared_after: Vec<HashSet<String>> = vec![HashSet::new(); n + 1];
        for i in (0..n).rev() {
            declared_after[i] = declared_after[i + 1].clone();
            bind_stmt(&block.stmts[i], &mut declared_after[i]);
        }

        let tail_is_expr = n > 0 && matches!(block.stmts[n - 1], Stmt::Expr { .. });
        let falls_through = n == 0 || !matches!(block.stmts[n - 1], Stmt::Return { .. });

        let mut new_stmts: Vec<Stmt> = Vec::with_capacity(n + 2);
        let mut active: Vec<Cleanup> = Vec::new();
        let stmts = std::mem::take(&mut block.stmts);
        for (i, stmt) in stmts.into_iter().enumerate() {
            let is_tail = tail_is_expr && i + 1 == n;
            let mut not_yet_here = not_yet.clone();
            not_yet_here.extend(declared_after[i + 1].iter().cloned());
            match stmt {
                Stmt::Defer { expr, span } => {
                    if let Some(return_span) = first_return_in_expr(&expr) {
                        return Err(Diagnostic::new(
                            "a deferred action cannot contain 'return': the action runs while the block is being left, so it cannot leave it again".to_string(),
                            return_span,
                            self.module.clone(),
                        )
                        .with_note(
                            "the 'defer' whose action this is".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    refuse_later_locals(&expr, &not_yet_here, span, &self.module)?;
                    if let Some(exit_span) = first_loop_exit_in_expr(&expr) {
                        return Err(Diagnostic::new(
                            "a deferred action cannot contain 'break' or 'continue': the action runs while the block is being left, so it cannot leave the loop again".to_string(),
                            exit_span,
                            self.module.clone(),
                        )
                        .with_note(
                            "the 'defer' whose action this is".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    let mut expr = expr;
                    // The action's own inner blocks are lowered once and then
                    // cloned into each exit, so a nested `defer` inside it
                    // still runs when the action's own block is left.
                    let empty = Chain::default();
                    self.expr(&mut expr, &empty, &HashSet::new())?;
                    active.push(Cleanup { expr, span });
                }
                other => {
                    let exit = Chain::exit(&active, outer);
                    match other {
                        Stmt::Return { value, span } => {
                            let mut value = value;
                            if let Some(v) = value.as_mut() {
                                self.expr(v, &exit, &not_yet_here)?;
                            }
                            // With nothing to run, the return stays exactly as
                            // it was written: a rewrite would move the value
                            // through a synthesized binding for no reason.
                            if exit.0.is_empty() {
                                new_stmts.push(Stmt::Return { value, span });
                            } else {
                                self.push_return(&mut new_stmts, value, span, &exit);
                            }
                        }
                        Stmt::Expr { mut expr, span } if is_tail => {
                            self.expr(&mut expr, &exit, &not_yet_here)?;
                            if active.is_empty() {
                                new_stmts.push(Stmt::Expr { expr, span });
                            } else if is_panic_call(&expr) {
                                // A panic tail diverges: the cleanup is
                                // unreachable, and splicing it in would turn
                                // the panic into an ordinary exit. Leave the
                                // tail alone (a panic never runs a defer).
                                new_stmts.push(Stmt::Expr { expr, span });
                            } else if is_function_body {
                                self.push_return(&mut new_stmts, Some(expr), span, &exit);
                            } else {
                                let tail = self.fresh(TAIL_PREFIX);
                                let mut inner: Vec<Stmt> =
                                    Vec::with_capacity(active.len() + 2);
                                inner.push(Stmt::Let {
                                    name: tail.clone(),
                                    mutable: false,
                                    ty: None,
                                    init: expr,
                                    span,
                                });
                                for c in active.iter().rev() {
                                    let name = self.fresh(TMP_PREFIX);
                                    inner.push(Stmt::Let {
                                        name,
                                        mutable: false,
                                        ty: None,
                                        init: c.expr.clone(),
                                        span: c.span,
                                    });
                                }
                                inner.push(Stmt::Expr {
                                    expr: Expr::Ident(tail, span),
                                    span,
                                });
                                new_stmts.push(Stmt::Expr {
                                    expr: Expr::Block(Block { stmts: inner, span }),
                                    span,
                                });
                            }
                        }
                        // A `break`/`continue` leaves the block just as a
                        // `return` does, so the actions active here run first.
                        // A plain `break`/`continue` (no `active`) is pushed
                        // unchanged, so a loop body with no defers emits
                        // exactly what it wrote.
                        Stmt::Break { span } => {
                            self.push_exit_actions(&mut new_stmts, &active);
                            new_stmts.push(Stmt::Break { span });
                        }
                        Stmt::Continue { span } => {
                            self.push_exit_actions(&mut new_stmts, &active);
                            new_stmts.push(Stmt::Continue { span });
                        }
                        mut other => {
                            self.stmt(&mut other, &exit, &not_yet_here)?;
                            new_stmts.push(other);
                        }
                    }
                }
            }
        }

        if falls_through && !tail_is_expr && !active.is_empty() {
            for c in active.iter().rev() {
                let name = self.fresh(TMP_PREFIX);
                new_stmts.push(Stmt::Let {
                    name,
                    mutable: false,
                    ty: None,
                    init: c.expr.clone(),
                    span: c.span,
                });
            }
        }

        block.stmts = new_stmts;
        Ok(())
    }

    /// Run the actions `active` owes, in execution order, as ordinary
    /// statements. Used before a `break`/`continue`, which leaves the block
    /// the way a `return` does.
    fn push_exit_actions(&mut self, out: &mut Vec<Stmt>, active: &[Cleanup]) {
        for c in active.iter().rev() {
            let name = self.fresh(TMP_PREFIX);
            out.push(Stmt::Let {
                name,
                mutable: false,
                ty: None,
                init: c.expr.clone(),
                span: c.span,
            });
        }
    }

    /// `return e` (or a function body's tail) with the actions `exit` owes
    /// spliced in before it. The value is evaluated first, so a return that
    /// moves a local the cleanup names is an ordinary use of a moved value.
    fn push_return(
        &mut self,
        out: &mut Vec<Stmt>,
        value: Option<Expr>,
        span: Span,
        exit: &Chain,
    ) {
        let ret = value.map(|value| {
            let name = self.fresh(RET_PREFIX);
            out.push(Stmt::Let {
                name: name.clone(),
                mutable: false,
                ty: None,
                init: value,
                span,
            });
            name
        });
        for c in &exit.0 {
            let name = self.fresh(TMP_PREFIX);
            out.push(Stmt::Let {
                name,
                mutable: false,
                ty: None,
                init: c.expr.clone(),
                span: c.span,
            });
        }
        out.push(Stmt::Return {
            value: ret.map(|name| Expr::Ident(name, span)),
            span,
        });
    }

    fn stmt(
        &mut self,
        stmt: &mut Stmt,
        outer: &Chain,
        not_yet: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { init, .. } => self.expr(init, outer, not_yet),
            Stmt::LetTuple { init, .. } => self.expr(init, outer, not_yet),
            Stmt::While { cond, body, .. } => {
                self.expr(cond, outer, not_yet)?;
                self.block(body, outer, not_yet, false)
            }
            Stmt::For {
                iterable, body, ..
            } => {
                self.expr(iterable, outer, not_yet)?;
                self.block(body, outer, not_yet, false)
            }
            Stmt::Expr { expr, .. } => self.expr(expr, outer, not_yet),
            // A `return`, a `break` and a `continue` are rewritten by `block`,
            // and a `defer` never reaches this arm; all are handled where their
            // position is known.
            Stmt::Return { .. }
            | Stmt::Defer { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => Ok(()),
        }
    }

    fn expr(
        &mut self,
        expr: &mut Expr,
        outer: &Chain,
        not_yet: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        match expr {
            Expr::IntLit(..)
            | Expr::CharLit(..)
            | Expr::ByteLit(..)
            | Expr::FloatLit(..)
            | Expr::StringLit(..)
            | Expr::BoolLit(..)
            | Expr::Ident(..) => Ok(()),
            Expr::Unary { expr, .. }
            | Expr::Ref { expr, .. }
            | Expr::Deref { expr, .. }
            | Expr::Try { expr, .. } => self.expr(expr, outer, not_yet),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs, outer, not_yet)?;
                self.expr(rhs, outer, not_yet)
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.expr(&mut arg.value, outer, not_yet)?;
                }
                Ok(())
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.expr(receiver, outer, not_yet)?;
                for arg in args {
                    self.expr(&mut arg.value, outer, not_yet)?;
                }
                Ok(())
            }
            Expr::FieldAccess { base, .. } => self.expr(base, outer, not_yet),
            Expr::Assign { target, value, .. } => {
                self.expr(target, outer, not_yet)?;
                self.expr(value, outer, not_yet)
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.expr(cond, outer, not_yet)?;
                self.block(then_block, outer, not_yet, false)?;
                match else_block.as_deref_mut() {
                    Some(ElseBranch::Block(b)) => self.block(b, outer, not_yet, false),
                    Some(ElseBranch::If(e)) => self.expr(e, outer, not_yet),
                    None => Ok(()),
                }
            }
            Expr::Block(b) => self.block(b, outer, not_yet, false),
            // A `fun` literal's body is a function of its own: a `return` in it
            // leaves the closure, not the block around the literal.
            Expr::FnLit { body, .. } => {
                let empty = Chain::default();
                match body.as_mut() {
                    FunctionBody::Block(b) => self.block(b, &empty, &HashSet::new(), true),
                    FunctionBody::Expr(e) => self.expr(e, &empty, &HashSet::new()),
                    FunctionBody::Extern | FunctionBody::Signature => Ok(()),
                }
            }
            Expr::Template { parts, .. } => {
                for part in parts {
                    if let TemplatePart::Expr(e) = part {
                        self.expr(e, outer, not_yet)?;
                    }
                }
                Ok(())
            }
            Expr::Unsafe { block, .. } => self.block(block, outer, not_yet, false),
            Expr::Asm { operands, .. } => {
                for operand in operands {
                    if let Some(e) = operand.operand.as_mut() {
                        self.expr(e, outer, not_yet)?;
                    }
                }
                Ok(())
            }
            Expr::When { scrutinee, arms, .. } => {
                self.expr(scrutinee, outer, not_yet)?;
                for arm in arms {
                    self.expr(&mut arm.body, outer, not_yet)?;
                }
                Ok(())
            }
            Expr::ArrayLit { elems, .. } | Expr::TupleLit { elems, .. } => {
                for e in elems {
                    self.expr(e, outer, not_yet)?;
                }
                Ok(())
            }
            Expr::ArrayRepeat { value, len, .. } => {
                self.expr(value, outer, not_yet)?;
                self.expr(len, outer, not_yet)
            }
        }
    }
}

/// The first `panic(...)` as a block's tail, which diverges. Only this exact
/// shape is recognized, mirroring `infer::tail_diverges`, so the two passes
/// agree about which expression is the returned value.
fn is_panic_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if callee == "panic")
}

/// The names a statement binds, for the "declared later" check.
fn bind_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { name, .. } => {
            out.insert(name.clone());
        }
        Stmt::LetTuple { pattern, .. } => {
            let mut names = Vec::new();
            pattern.binds(&mut Vec::new(), &mut names);
            for (name, _) in names {
                out.insert(name);
            }
        }
        _ => {}
    }
}

/// Refuses a deferred action that names a local only declared later, which at
/// the `defer` statement is not in scope: the action would resolve it at the
/// block's exit instead, against a binding the `defer` never saw.
fn refuse_later_locals(
    expr: &Expr,
    not_yet: &HashSet<String>,
    span: Span,
    module: &str,
) -> Result<(), Diagnostic> {
    if not_yet.is_empty() {
        return Ok(());
    }
    let mut free = HashSet::new();
    free_in_expr(expr, &HashSet::new(), &mut free);
    let mut hits: Vec<&String> = not_yet.iter().filter(|n| free.contains(*n)).collect();
    hits.sort();
    if let Some(name) = hits.first() {
        return Err(Diagnostic::new(
            format!(
                "this 'defer' names '{name}', which is declared later in the block: a deferred action sees only the locals already in scope where the 'defer' is written"
            ),
            span,
            module,
        ));
    }
    Ok(())
}

fn free_in_block(block: &Block, bound: &HashSet<String>, out: &mut HashSet<String>) {
    let mut scope = bound.clone();
    for stmt in &block.stmts {
        free_in_stmt(stmt, &scope, out);
        bind_stmt(stmt, &mut scope);
    }
}

fn free_in_stmt(stmt: &Stmt, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { init, .. } => free_in_expr(init, bound, out),
        Stmt::LetTuple { init, .. } => free_in_expr(init, bound, out),
        Stmt::While { cond, body, .. } => {
            free_in_expr(cond, bound, out);
            free_in_block(body, bound, out);
        }
        Stmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            free_in_expr(iterable, bound, out);
            let mut scope = bound.clone();
            scope.insert(binding.clone());
            free_in_block(body, &scope, out);
        }
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                free_in_expr(v, bound, out);
            }
        }
        Stmt::Expr { expr, .. } => free_in_expr(expr, bound, out),
        Stmt::Defer { expr, .. } => free_in_expr(expr, bound, out),
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

/// Every name the expression reads that is not bound inside it. A `fun`
/// literal contributes its body's free names (its captures are resolved where
/// the literal is written), with its parameters bound.
fn free_in_expr(expr: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(n, _) => {
            if !bound.contains(n) {
                out.insert(n.clone());
            }
        }
        Expr::IntLit(..)
        | Expr::CharLit(..)
        | Expr::ByteLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..) => {}
        Expr::Unary { expr, .. }
        | Expr::Ref { expr, .. }
        | Expr::Deref { expr, .. }
        | Expr::Try { expr, .. } => free_in_expr(expr, bound, out),
        Expr::Binary { lhs, rhs, .. } => {
            free_in_expr(lhs, bound, out);
            free_in_expr(rhs, bound, out);
        }
        Expr::Call { callee, args, .. } => {
            if !bound.contains(callee) {
                out.insert(callee.clone());
            }
            for arg in args {
                free_in_expr(&arg.value, bound, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            free_in_expr(receiver, bound, out);
            for arg in args {
                free_in_expr(&arg.value, bound, out);
            }
        }
        Expr::FieldAccess { base, .. } => free_in_expr(base, bound, out),
        Expr::Assign { target, value, .. } => {
            free_in_expr(target, bound, out);
            free_in_expr(value, bound, out);
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            free_in_expr(cond, bound, out);
            free_in_block(then_block, bound, out);
            match else_block.as_deref() {
                Some(ElseBranch::Block(b)) => free_in_block(b, bound, out),
                Some(ElseBranch::If(e)) => free_in_expr(e, bound, out),
                None => {}
            }
        }
        Expr::Block(b) | Expr::Unsafe { block: b, .. } => free_in_block(b, bound, out),
        Expr::FnLit { params, body, .. } => {
            let mut scope = bound.clone();
            for p in params {
                scope.insert(p.name.clone());
            }
            match body.as_ref() {
                FunctionBody::Block(b) => free_in_block(b, &scope, out),
                FunctionBody::Expr(e) => free_in_expr(e, &scope, out),
                FunctionBody::Extern | FunctionBody::Signature => {}
            }
        }
        Expr::Template { parts, .. } => {
            for part in parts {
                if let TemplatePart::Expr(e) = part {
                    free_in_expr(e, bound, out);
                }
            }
        }
        Expr::Asm { operands, .. } => {
            for operand in operands {
                if let Some(e) = &operand.operand {
                    free_in_expr(e, bound, out);
                }
            }
        }
        Expr::When { scrutinee, arms, .. } => {
            free_in_expr(scrutinee, bound, out);
            for arm in arms {
                let mut scope = bound.clone();
                if let Some(pattern) = &arm.pattern {
                    for b in &pattern.binds {
                        if let Some(n) = b {
                            scope.insert(n.clone());
                        }
                    }
                }
                free_in_expr(&arm.body, &scope, out);
            }
        }
        Expr::ArrayLit { elems, .. } | Expr::TupleLit { elems, .. } => {
            for e in elems {
                free_in_expr(e, bound, out);
            }
        }
        Expr::ArrayRepeat { value, len, .. } => {
            free_in_expr(value, bound, out);
            free_in_expr(len, bound, out);
        }
    }
}

/// The first `?` in a block, not descending into a `fun` literal, whose `?`
/// exits the closure rather than this block. `None` when the block has none.
fn first_try_in_block(block: &Block) -> Option<Span> {
    block.stmts.iter().find_map(|s| match s {
        Stmt::Let { init, .. } => first_try_in_expr(init),
        Stmt::LetTuple { init, .. } => first_try_in_expr(init),
        Stmt::While { cond, body, .. } => first_try_in_expr(cond).or_else(|| first_try_in_block(body)),
        Stmt::For {
            iterable, body, ..
        } => first_try_in_expr(iterable).or_else(|| first_try_in_block(body)),
        Stmt::Return { value, .. } => value.as_ref().and_then(first_try_in_expr),
        Stmt::Expr { expr, .. } => first_try_in_expr(expr),
        Stmt::Defer { expr, .. } => first_try_in_expr(expr),
        Stmt::Break { .. } | Stmt::Continue { .. } => None,
    })
}

fn first_try_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Try { span, .. } => Some(*span),
        Expr::IntLit(..)
        | Expr::CharLit(..)
        | Expr::ByteLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..)
        | Expr::Ident(..) => None,
        Expr::Unary { expr, .. }
        | Expr::Ref { expr, .. }
        | Expr::Deref { expr, .. } => first_try_in_expr(expr),
        Expr::Binary { lhs, rhs, .. } => first_try_in_expr(lhs).or_else(|| first_try_in_expr(rhs)),
        Expr::Call { args, .. } => args.iter().find_map(|a| first_try_in_expr(&a.value)),
        Expr::MethodCall { receiver, args, .. } => first_try_in_expr(receiver)
            .or_else(|| args.iter().find_map(|a| first_try_in_expr(&a.value))),
        Expr::FieldAccess { base, .. } => first_try_in_expr(base),
        Expr::Assign { target, value, .. } => {
            first_try_in_expr(target).or_else(|| first_try_in_expr(value))
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => first_try_in_expr(cond)
            .or_else(|| first_try_in_block(then_block))
            .or_else(|| match else_block.as_deref() {
                Some(ElseBranch::Block(b)) => first_try_in_block(b),
                Some(ElseBranch::If(e)) => first_try_in_expr(e),
                None => None,
            }),
        Expr::Block(b) | Expr::Unsafe { block: b, .. } => first_try_in_block(b),
        Expr::FnLit { .. } => None,
        Expr::Template { parts, .. } => parts.iter().find_map(|p| match p {
            TemplatePart::Text(_) => None,
            TemplatePart::Expr(e) => first_try_in_expr(e),
        }),
        Expr::Asm { operands, .. } => operands
            .iter()
            .find_map(|o| o.operand.as_ref().and_then(first_try_in_expr)),
        Expr::When { scrutinee, arms, .. } => first_try_in_expr(scrutinee)
            .or_else(|| arms.iter().find_map(|a| first_try_in_expr(&a.body))),
        Expr::ArrayLit { elems, .. } | Expr::TupleLit { elems, .. } => {
            elems.iter().find_map(first_try_in_expr)
        }
        Expr::ArrayRepeat { value, len, .. } => {
            first_try_in_expr(value).or_else(|| first_try_in_expr(len))
        }
    }
}

/// The first `return` inside a deferred action, not descending into a `fun`
/// literal, whose `return` belongs to the closure. `break`/`continue` are not
/// statements in the language (`plan/syntax.md` 11), so `return` is the only
/// exit an action could write today.
fn first_return_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Block(b) | Expr::Unsafe { block: b, .. } => first_return_in_block(b),
        Expr::Unary { expr, .. }
        | Expr::Ref { expr, .. }
        | Expr::Deref { expr, .. }
        | Expr::Try { expr, .. } => first_return_in_expr(expr),
        Expr::Binary { lhs, rhs, .. } => {
            first_return_in_expr(lhs).or_else(|| first_return_in_expr(rhs))
        }
        Expr::Call { args, .. } => args.iter().find_map(|a| first_return_in_expr(&a.value)),
        Expr::MethodCall { receiver, args, .. } => first_return_in_expr(receiver)
            .or_else(|| args.iter().find_map(|a| first_return_in_expr(&a.value))),
        Expr::FieldAccess { base, .. } => first_return_in_expr(base),
        Expr::Assign { target, value, .. } => {
            first_return_in_expr(target).or_else(|| first_return_in_expr(value))
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => first_return_in_expr(cond)
            .or_else(|| first_return_in_block(then_block))
            .or_else(|| match else_block.as_deref() {
                Some(ElseBranch::Block(b)) => first_return_in_block(b),
                Some(ElseBranch::If(e)) => first_return_in_expr(e),
                None => None,
            }),
        Expr::Template { parts, .. } => parts.iter().find_map(|p| match p {
            TemplatePart::Text(_) => None,
            TemplatePart::Expr(e) => first_return_in_expr(e),
        }),
        Expr::Asm { operands, .. } => operands
            .iter()
            .find_map(|o| o.operand.as_ref().and_then(first_return_in_expr)),
        Expr::When { scrutinee, arms, .. } => first_return_in_expr(scrutinee)
            .or_else(|| arms.iter().find_map(|a| first_return_in_expr(&a.body))),
        Expr::ArrayLit { elems, .. } | Expr::TupleLit { elems, .. } => {
            elems.iter().find_map(first_return_in_expr)
        }
        Expr::ArrayRepeat { value, len, .. } => {
            first_return_in_expr(value).or_else(|| first_return_in_expr(len))
        }
        Expr::IntLit(..)
        | Expr::CharLit(..)
        | Expr::ByteLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..)
        | Expr::Ident(..)
        | Expr::FnLit { .. } => None,
    }
}

fn first_return_in_block(block: &Block) -> Option<Span> {
    block.stmts.iter().find_map(|s| match s {
        Stmt::Return { span, .. } => Some(*span),
        Stmt::Let { init, .. } => first_return_in_expr(init),
        Stmt::LetTuple { init, .. } => first_return_in_expr(init),
        Stmt::While { cond, body, .. } => {
            first_return_in_expr(cond).or_else(|| first_return_in_block(body))
        }
        Stmt::For {
            iterable, body, ..
        } => first_return_in_expr(iterable).or_else(|| first_return_in_block(body)),
        Stmt::Expr { expr, .. } => first_return_in_expr(expr),
        Stmt::Defer { expr, .. } => first_return_in_expr(expr),
        Stmt::Break { .. } | Stmt::Continue { .. } => None,
    })
}

/// The first `break`/`continue` inside a deferred action, not descending into a
/// `fun` literal (whose loop control is its own).
fn first_loop_exit_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Block(b) | Expr::Unsafe { block: b, .. } => first_loop_exit_in_block(b),
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => first_loop_exit_in_expr(cond)
            .or_else(|| first_loop_exit_in_block(then_block))
            .or_else(|| match else_block.as_deref() {
                Some(ElseBranch::Block(b)) => first_loop_exit_in_block(b),
                Some(ElseBranch::If(e)) => first_loop_exit_in_expr(e),
                None => None,
            }),
        Expr::When { scrutinee, arms, .. } => first_loop_exit_in_expr(scrutinee)
            .or_else(|| arms.iter().find_map(|a| first_loop_exit_in_expr(&a.body))),
        Expr::Unary { expr, .. }
        | Expr::Ref { expr, .. }
        | Expr::Deref { expr, .. }
        | Expr::Try { expr, .. } => first_loop_exit_in_expr(expr),
        Expr::Binary { lhs, rhs, .. } => {
            first_loop_exit_in_expr(lhs).or_else(|| first_loop_exit_in_expr(rhs))
        }
        Expr::Call { args, .. } => args.iter().find_map(|a| first_loop_exit_in_expr(&a.value)),
        Expr::MethodCall { receiver, args, .. } => first_loop_exit_in_expr(receiver)
            .or_else(|| args.iter().find_map(|a| first_loop_exit_in_expr(&a.value))),
        Expr::FieldAccess { base, .. } => first_loop_exit_in_expr(base),
        Expr::Assign { target, value, .. } => {
            first_loop_exit_in_expr(target).or_else(|| first_loop_exit_in_expr(value))
        }
        Expr::Template { parts, .. } => parts.iter().find_map(|p| match p {
            TemplatePart::Text(_) => None,
            TemplatePart::Expr(e) => first_loop_exit_in_expr(e),
        }),
        Expr::Asm { operands, .. } => operands
            .iter()
            .find_map(|o| o.operand.as_ref().and_then(first_loop_exit_in_expr)),
        Expr::ArrayLit { elems, .. } | Expr::TupleLit { elems, .. } => {
            elems.iter().find_map(first_loop_exit_in_expr)
        }
        Expr::ArrayRepeat { value, len, .. } => {
            first_loop_exit_in_expr(value).or_else(|| first_loop_exit_in_expr(len))
        }
        Expr::IntLit(..)
        | Expr::CharLit(..)
        | Expr::ByteLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..)
        | Expr::Ident(..)
        | Expr::FnLit { .. } => None,
    }
}

fn first_loop_exit_in_block(block: &Block) -> Option<Span> {
    block.stmts.iter().find_map(|s| match s {
        Stmt::Break { span } | Stmt::Continue { span } => Some(*span),
        Stmt::Let { init, .. } => first_loop_exit_in_expr(init),
        Stmt::LetTuple { init, .. } => first_loop_exit_in_expr(init),
        Stmt::While { cond, body, .. } => {
            first_loop_exit_in_expr(cond).or_else(|| first_loop_exit_in_block(body))
        }
        Stmt::For {
            iterable, body, ..
        } => first_loop_exit_in_expr(iterable).or_else(|| first_loop_exit_in_block(body)),
        Stmt::Return { value, .. } => value.as_ref().and_then(first_loop_exit_in_expr),
        Stmt::Expr { expr, .. } => first_loop_exit_in_expr(expr),
        Stmt::Defer { expr, .. } => first_loop_exit_in_expr(expr),
    })
}

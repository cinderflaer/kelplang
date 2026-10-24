use std::collections::HashSet;

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::lexer::Span;

/// The names a statement binds, for the "declared later" check.
pub(super) fn bind_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
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
pub(super) fn refuse_later_locals(
    expr: &Expr,
    not_yet: &HashSet<String>,
    span: Span,
    module: &str,
    keyword: &str,
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
                "this '{keyword}' names '{name}', which is declared later in the block: a deferred action sees only the locals already in scope where the '{keyword}' is written"
            ),
            span,
            module,
        ));
    }
    Ok(())
}

pub(super) fn free_in_block(block: &Block, bound: &HashSet<String>, out: &mut HashSet<String>) {
    let mut scope = bound.clone();
    for stmt in &block.stmts {
        free_in_stmt(stmt, &scope, out);
        bind_stmt(stmt, &mut scope);
    }
}

pub(super) fn free_in_stmt(stmt: &Stmt, bound: &HashSet<String>, out: &mut HashSet<String>) {
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
        Stmt::Defer { expr, .. } | Stmt::Errdefer { expr, .. } => free_in_expr(expr, bound, out),
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

/// Every name the expression reads that is not bound inside it. A `fun`
/// literal contributes its body's free names (its captures are resolved where
/// the literal is written), with its parameters bound.
pub(crate) fn free_in_expr(expr: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
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
        Expr::MethodCall { receiver, method, args, .. } => {
            // A method name is recorded as `.name`, which no local can spell,
            // so the capture check ignores it while the return-type pass
            // (Gitea #280) can see which methods a body calls.
            out.insert(format!(".{method}"));
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
        Expr::Block(b) | Expr::Unsafe { block: b, .. } | Expr::ManualBlock { body: b, .. } => free_in_block(b, bound, out),
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
pub(super) fn first_try_in_block(block: &Block) -> Option<Span> {
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
        // An errdefer's own `?` is refused where it is written, so it does not
        // make the block's ordinary `?` refusal fire.
        Stmt::Errdefer { .. } => None,
        Stmt::Break { .. } | Stmt::Continue { .. } => None,
    })
}

pub(super) fn first_try_in_expr(expr: &Expr) -> Option<Span> {
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
        Expr::Block(b) | Expr::Unsafe { block: b, .. } | Expr::ManualBlock { body: b, .. } => first_try_in_block(b),
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
/// statements in the language (`design/syntax.md (kelp wiki)` 11), so `return` is the only
/// exit an action could write today.
pub(super) fn first_return_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Block(b) | Expr::Unsafe { block: b, .. } | Expr::ManualBlock { body: b, .. } => first_return_in_block(b),
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

pub(super) fn first_return_in_block(block: &Block) -> Option<Span> {
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
        Stmt::Defer { expr, .. } | Stmt::Errdefer { expr, .. } => first_return_in_expr(expr),
        Stmt::Break { .. } | Stmt::Continue { .. } => None,
    })
}

/// The first `break`/`continue` inside a deferred action, not descending into a
/// `fun` literal (whose loop control is its own).
pub(super) fn first_loop_exit_in_expr(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Block(b) | Expr::Unsafe { block: b, .. } | Expr::ManualBlock { body: b, .. } => first_loop_exit_in_block(b),
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

pub(super) fn first_loop_exit_in_block(block: &Block) -> Option<Span> {
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
        Stmt::Defer { expr, .. } | Stmt::Errdefer { expr, .. } => first_loop_exit_in_expr(expr),
    })
}

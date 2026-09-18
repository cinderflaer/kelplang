//! M3: ownership/move checking over the typed IR.
//!
//! Rule: primitives (and `&str`, which is just a borrowed pointer with no
//! owned-string type existing yet) are `Copy`; `datac` values are not. A
//! bare `Ident` read of a non-Copy value moves it out of its binding; a
//! `.field` read walks through a place chain without consuming anything,
//! since there's no way to move a single field out in v1's grammar (no
//! reference/borrow support in expression position yet — that's M4).
//!
//! State is tracked per-scope, mirroring `infer.rs`'s scope stack. `if`/
//! `else` branches are checked against independent snapshots of the
//! pre-branch state and then merged (moved in either branch => moved
//! after, conservatively — matches how a runtime "which branch ran" is
//! unknowable statically). `while` bodies are checked twice in sequence,
//! starting the second pass from the state the first pass left behind, to
//! catch a value that would be double-moved on a second iteration; the
//! loop's own possible zero-iteration case is then folded back in by
//! merging with the pre-loop state (nothing is *definitely* moved by a
//! loop that might not run at all).

use std::collections::HashMap;

use crate::lexer::Span;
use crate::tir;
use crate::types::Ty;

#[derive(Debug)]
pub struct BorrowError {
    pub message: String,
    pub span: Span,
}

type Result<T> = std::result::Result<T, BorrowError>;

#[derive(Clone)]
struct VarState {
    is_copy: bool,
    moved_at: Option<Span>,
}

#[derive(Clone, Default)]
struct Scope {
    vars: HashMap<String, VarState>,
}

impl Scope {
    /// A variable is moved after the merge if it was moved down either
    /// branch. Both scopes are expected to hold the same keys: block-local
    /// bindings are popped before a branch's ending state is snapshotted,
    /// so only variables visible before the `if` remain.
    fn merge(a: Scope, b: Scope) -> Scope {
        let mut vars = HashMap::new();
        for (name, va) in a.vars {
            let moved_at = va.moved_at.or_else(|| b.vars.get(&name).and_then(|vb| vb.moved_at));
            vars.insert(name, VarState { is_copy: va.is_copy, moved_at });
        }
        Scope { vars }
    }
}

struct Ctx {
    scopes: Vec<Scope>,
}

impl Ctx {
    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, is_copy: bool) {
        self.scopes
            .last_mut()
            .unwrap()
            .vars
            .insert(name.to_string(), VarState { is_copy, moved_at: None });
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut VarState> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(v) = scope.vars.get_mut(name) {
                return Some(v);
            }
        }
        None
    }

    /// Errors if `name` was already moved; otherwise, if it's non-Copy,
    /// this whole-value read moves it.
    fn use_value(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(()); // infer.rs already validated every identifier
        };
        if v.moved_at.is_some() {
            return Err(BorrowError {
                message: format!("use of moved value '{name}'"),
                span,
            });
        }
        if !v.is_copy {
            v.moved_at = Some(span);
        }
        Ok(())
    }

    /// Errors if `name` was already moved; never itself moves anything
    /// (used when reading *through* a place, e.g. the base of `p.x`).
    fn use_place(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(());
        };
        if v.moved_at.is_some() {
            return Err(BorrowError {
                message: format!("use of moved value '{name}'"),
                span,
            });
        }
        Ok(())
    }

    /// Assigning a fresh value into `name` reinitializes it, regardless of
    /// its prior move state.
    fn set_live(&mut self, name: &str) {
        if let Some(v) = self.lookup_mut(name) {
            v.moved_at = None;
        }
    }

    fn snapshot(&self) -> Vec<Scope> {
        self.scopes.clone()
    }

    fn restore(&mut self, snapshot: Vec<Scope>) {
        self.scopes = snapshot;
    }

    fn merge_into(&mut self, then_snap: Vec<Scope>, else_snap: Vec<Scope>) {
        let merged = then_snap
            .into_iter()
            .zip(else_snap)
            .map(|(a, b)| Scope::merge(a, b))
            .collect();
        self.restore(merged);
    }
}

fn is_copy(ty: &Ty) -> bool {
    !matches!(ty, Ty::Struct(_))
}

pub fn check_program(program: &tir::Program) -> Result<()> {
    for f in &program.functions {
        check_function(f)?;
    }
    Ok(())
}

fn check_function(f: &tir::Function) -> Result<()> {
    let mut ctx = Ctx { scopes: vec![Scope::default()] };
    for (name, ty) in &f.params {
        ctx.define(name, is_copy(ty));
    }
    check_block(&f.body, &mut ctx)
}

fn check_block(block: &tir::Block, ctx: &mut Ctx) -> Result<()> {
    ctx.push_scope();
    let result = (|| {
        for stmt in &block.stmts {
            check_stmt(stmt, ctx)?;
        }
        if let Some(tail) = &block.tail {
            check_value(tail, ctx)?;
        }
        Ok(())
    })();
    ctx.pop_scope();
    result
}

fn check_stmt(stmt: &tir::Stmt, ctx: &mut Ctx) -> Result<()> {
    match stmt {
        tir::Stmt::Let { name, ty, init } => {
            check_value(init, ctx)?;
            ctx.define(name, is_copy(ty));
            Ok(())
        }
        tir::Stmt::While { cond, body } => check_while(cond, body, ctx),
        tir::Stmt::Return { value } => match value {
            Some(v) => check_value(v, ctx),
            None => Ok(()),
        },
        tir::Stmt::Expr(e) => check_value(e, ctx),
    }
}

fn check_while(cond: &tir::Expr, body: &tir::Block, ctx: &mut Ctx) -> Result<()> {
    let before = ctx.snapshot();

    // First pass: one iteration.
    check_value(cond, ctx)?;
    check_block(body, ctx)?;

    // Second pass, starting from the state one iteration leaves behind:
    // catches a value that would be double-moved on a second iteration
    // (loops aren't guaranteed to run only once).
    check_value(cond, ctx)?;
    check_block(body, ctx)?;

    // The loop may also run zero times, so nothing here becomes
    // *definitely* moved unless it already was before the loop.
    let after = ctx.snapshot();
    ctx.merge_into(before, after);
    Ok(())
}

fn check_if(then_block: &tir::Block, else_block: &Option<tir::Block>, ctx: &mut Ctx) -> Result<()> {
    let before = ctx.snapshot();

    check_block(then_block, ctx)?;
    let after_then = ctx.snapshot();

    ctx.restore(before);
    if let Some(eb) = else_block {
        check_block(eb, ctx)?;
    }
    let after_else = ctx.snapshot();

    ctx.merge_into(after_then, after_else);
    Ok(())
}

/// Checks an expression used as a whole value: a bare `Ident` here moves a
/// non-Copy binding.
fn check_value(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => ctx.use_value(name, expr.span),
        tir::ExprKind::FieldAccess { base, .. } => check_place(base, ctx),
        tir::ExprKind::IntLit(_) | tir::ExprKind::FloatLit(_) | tir::ExprKind::StringLit(_) | tir::ExprKind::BoolLit(_) => Ok(()),
        tir::ExprKind::Unary { expr: sub, .. } => check_value(sub, ctx),
        tir::ExprKind::Binary { lhs, rhs, .. } => {
            check_value(lhs, ctx)?;
            check_value(rhs, ctx)
        }
        tir::ExprKind::Call { args, .. } => {
            for a in args {
                check_value(a, ctx)?;
            }
            Ok(())
        }
        tir::ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_value(v, ctx)?;
            }
            Ok(())
        }
        tir::ExprKind::Assign { target, value } => {
            check_value(value, ctx)?;
            check_assign_target(target, ctx)
        }
        tir::ExprKind::If { cond, then_block, else_block } => {
            check_value(cond, ctx)?;
            check_if(then_block, else_block, ctx)
        }
        tir::ExprKind::Block(b) => check_block(b, ctx),
    }
}

/// Checks an expression read *through*, as the base of a place chain
/// (`p` in `p.x`, or `p` in `p.x.y`): errors if already moved, but never
/// itself moves anything.
fn check_place(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &base.kind {
        tir::ExprKind::Ident(name) => ctx.use_place(name, base.span),
        tir::ExprKind::FieldAccess { base: inner, .. } => check_place(inner, ctx),
        _ => check_value(base, ctx),
    }
}

fn check_assign_target(target: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &target.kind {
        tir::ExprKind::Ident(name) => {
            ctx.set_live(name);
            Ok(())
        }
        tir::ExprKind::FieldAccess { base, .. } => check_place(base, ctx),
        _ => Ok(()), // infer.rs already restricts assign targets to Ident/FieldAccess
    }
}

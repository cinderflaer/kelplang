//! M3 (moves) + M4 (borrowing) checking over the typed IR.
//!
//! ## Moves (M3)
//!
//! Rule: primitives (and `&str`, which has no owned-string type to move —
//! it's just a borrowed pointer) are `Copy`; `datac` values are not. A bare
//! `Ident` read of a non-Copy value moves it out of its binding; a `.field`
//! read walks through a place chain without consuming anything, since
//! there's no way to move a single field out in v1's grammar.
//!
//! ## Borrowing (M4)
//!
//! Per `syntax&shii.md` §5, a `datac`-typed function parameter is an
//! implicit shared borrow unless `mut`-marked (exclusive) — `infer.rs`
//! resolves this into `Ty::Ref` and, at call sites, synthesizes the
//! `tir::ExprKind::Ref` node itself (callers never write `&`). There is no
//! move-taking parameter spelling in v1: a `datac` is only ever moved by
//! local rebinding (`val q = p`), which the M3 logic above already covers.
//!
//! Two borrow lifetimes are tracked, both keyed on the *root* variable
//! being borrowed:
//! - **Call-scoped**: a borrow that flows directly into a call's argument
//!   list (an auto-borrow, or an explicit `&expr` written as the argument
//!   itself). All borrows for one call's argument list are started
//!   *before any of them are released*, so `start_borrow`'s own conflict
//!   check catches a same-call aliasing violation (`f(&mut c, &c)`) — not
//!   a separate pass. They're released together right after the call.
//! - **Scope-durable**: a borrow that's the direct initializer of a
//!   `val`/`var` (`val p = &c`) lives until that binding's enclosing scope
//!   ends, mirroring the move-state scope stack.
//!
//! No surface syntax lets a function return a `Ty::Ref` (`infer.rs`'s
//! `resolve_type` never produces one for a `.R: T` clause), so a borrow can
//! never escape the call frame that created it — that's what makes
//! call-scoped release sound without real lifetime inference. Aliasing:
//! exclusive excludes everything else; any number of shared borrows may
//! coexist. Moving or reassigning a variable while it's actively borrowed
//! is rejected, and reading `*p` as a whole value (not just through it) is
//! rejected when the pointee is non-Copy, mirroring "cannot move out of a
//! shared reference."
//!
//! Known, deliberately out-of-scope gap: `p = &c` *reassigning* an
//! existing binding to a fresh borrow is checked at the moment of
//! assignment (conflicts with anything currently active are still caught)
//! but doesn't persist scope-durable tracking the way a `val`/`var`
//! *declaration* does — only declaration-time borrows are exercised by any
//! fixture here. Also out of scope: receiver-form methods, and `&str`
//! return-lifetime elision (moot for now since `&str` isn't routed through
//! `Ty::Ref` at all).

use std::collections::HashMap;

use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::tir;
use crate::types::Ty;

#[derive(Debug)]
pub struct BorrowError {
    pub message: String,
    pub span: Span,
}

type Result<T> = std::result::Result<T, BorrowError>;

#[derive(Clone, PartialEq)]
enum BorrowKind {
    None,
    Shared(u32),
    Exclusive,
}

#[derive(Clone)]
struct VarState {
    is_copy: bool,
    moved_at: Option<Span>,
    borrow: BorrowKind,
}

#[derive(Clone, Default)]
struct Scope {
    vars: HashMap<String, VarState>,
    /// Roots borrowed by a `val`/`var` declared in this scope, released
    /// when the scope pops (see the module doc's "scope-durable" lifetime).
    scope_borrows: Vec<String>,
}

impl Scope {
    fn merge(a: Scope, b: Scope) -> Scope {
        let mut vars = HashMap::new();
        for (name, va) in a.vars {
            let moved_at = va.moved_at.or_else(|| b.vars.get(&name).and_then(|vb| vb.moved_at));
            // Borrow state doesn't survive a branch merge — both arms'
            // borrows are already released (each `if`/`else` block pops
            // its own scope-durable borrows before this runs), so what's
            // left on either side is whatever state existed going in,
            // which is identical on both sides by construction.
            vars.insert(name, VarState { is_copy: va.is_copy, moved_at, borrow: va.borrow });
        }
        Scope { vars, scope_borrows: Vec::new() }
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
        let popped = self.scopes.pop().expect("pop_scope without a matching push_scope");
        for root in popped.scope_borrows {
            self.end_borrow(&root);
        }
    }

    fn define(&mut self, name: &str, is_copy: bool) {
        self.scopes
            .last_mut()
            .unwrap()
            .vars
            .insert(name.to_string(), VarState { is_copy, moved_at: None, borrow: BorrowKind::None });
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut VarState> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(v) = scope.vars.get_mut(name) {
                return Some(v);
            }
        }
        None
    }

    /// Errors if `name` was already moved or is currently borrowed;
    /// otherwise, if it's non-Copy, this whole-value read moves it.
    fn use_value(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(()); // infer.rs already validated every identifier
        };
        if v.moved_at.is_some() {
            return Err(BorrowError { message: format!("use of moved value '{name}'"), span });
        }
        if v.borrow != BorrowKind::None {
            return Err(BorrowError { message: format!("cannot move '{name}' while it is borrowed"), span });
        }
        if !v.is_copy {
            v.moved_at = Some(span);
        }
        Ok(())
    }

    /// Errors if `name` was already moved; never itself moves or borrows
    /// anything (used when reading *through* a place, e.g. the base of
    /// `p.x`, or the pointer operand of `*p`).
    fn use_place(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(());
        };
        if v.moved_at.is_some() {
            return Err(BorrowError { message: format!("use of moved value '{name}'"), span });
        }
        Ok(())
    }

    /// Assigning a fresh value into `name` reinitializes it (clears moved
    /// state), but only while nothing currently borrows it.
    fn reinit(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(());
        };
        if v.borrow != BorrowKind::None {
            return Err(BorrowError { message: format!("cannot assign to '{name}' while it is borrowed"), span });
        }
        v.moved_at = None;
        Ok(())
    }

    fn start_borrow(&mut self, root: &str, mutable: bool, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(root) else {
            return Ok(());
        };
        if v.moved_at.is_some() {
            return Err(BorrowError { message: format!("cannot borrow '{root}': value was moved"), span });
        }
        v.borrow = match (&v.borrow, mutable) {
            (BorrowKind::Exclusive, _) => {
                return Err(BorrowError {
                    message: format!("cannot borrow '{root}': already borrowed exclusively"),
                    span,
                });
            }
            (BorrowKind::Shared(_), true) => {
                return Err(BorrowError {
                    message: format!("cannot borrow '{root}' as exclusive: already borrowed as shared"),
                    span,
                });
            }
            (BorrowKind::Shared(n), false) => BorrowKind::Shared(n + 1),
            (BorrowKind::None, true) => BorrowKind::Exclusive,
            (BorrowKind::None, false) => BorrowKind::Shared(1),
        };
        Ok(())
    }

    fn end_borrow(&mut self, root: &str) {
        if let Some(v) = self.lookup_mut(root) {
            v.borrow = match &v.borrow {
                BorrowKind::Shared(n) if *n > 1 => BorrowKind::Shared(n - 1),
                _ => BorrowKind::None,
            };
        }
    }

    fn register_scope_borrow(&mut self, root: &str) {
        self.scopes.last_mut().unwrap().scope_borrows.push(root.to_string());
    }

    fn snapshot(&self) -> Vec<Scope> {
        self.scopes.clone()
    }

    fn restore(&mut self, snapshot: Vec<Scope>) {
        self.scopes = snapshot;
    }

    fn merge_into(&mut self, then_snap: Vec<Scope>, else_snap: Vec<Scope>) {
        let merged = then_snap.into_iter().zip(else_snap).map(|(a, b)| Scope::merge(a, b)).collect();
        self.restore(merged);
    }
}

fn is_copy(ty: &Ty) -> bool {
    !matches!(ty, Ty::Struct(_))
}

/// The root identifier a place-expression is ultimately reached through.
fn tir_root_ident(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => Some(name),
        tir::ExprKind::FieldAccess { base, .. } => tir_root_ident(base),
        tir::ExprKind::Deref { expr: inner } => tir_root_ident(inner),
        _ => None,
    }
}

pub fn check_program(program: &tir::Program) -> std::result::Result<(), Diagnostic> {
    for f in &program.functions {
        check_function(f).map_err(|e| Diagnostic::new(e.message, e.span, f.module.clone()))?;
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
            check_let_init(init, ctx)?;
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

/// A `val`/`var` initializer gets scope-durable borrow tracking when it's
/// directly `&expr`/`&mut expr` (see the module doc); anything else is a
/// plain value read.
fn check_let_init(init: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    if let tir::ExprKind::Ref { mutable, expr: inner } = &init.kind {
        check_place(inner, ctx)?;
        if let Some(root) = tir_root_ident(inner) {
            ctx.start_borrow(root, *mutable, init.span)?;
            ctx.register_scope_borrow(root);
        }
        Ok(())
    } else {
        check_value(init, ctx)
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
/// non-Copy binding, and `*p` here (not just read *through*) is rejected
/// when the pointee is non-Copy — mirrors Rust's "cannot move out of a
/// shared reference."
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
        tir::ExprKind::Call { args, .. } => check_call(args, ctx),
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
        tir::ExprKind::Ref { mutable, expr: inner } => {
            // A borrow reached generically here (not via a `val`/`var`
            // initializer or a call argument — see check_let_init and
            // check_call) is transient: started and released immediately.
            // The only realistic surface form this covers is `p = &c`
            // reassignment, which is why it's checked against currently
            // active borrows but doesn't persist tracking — see the
            // module doc's "known gap."
            check_place(inner, ctx)?;
            if let Some(root) = tir_root_ident(inner) {
                ctx.start_borrow(root, *mutable, expr.span)?;
                ctx.end_borrow(root);
            }
            Ok(())
        }
        tir::ExprKind::Deref { expr: sub } => {
            check_value(sub, ctx)?; // the pointer itself is Copy; reading it never moves anything
            if !is_copy(&expr.ty) {
                return Err(BorrowError {
                    message: "cannot move a value out through a dereferenced borrow".to_string(),
                    span: expr.span,
                });
            }
            Ok(())
        }
    }
}

/// Checks an expression read *through*, as the base of a place chain (`p`
/// in `p.x`, or the pointer operand of `*p`): errors if already moved, but
/// never itself moves or borrows anything.
fn check_place(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &base.kind {
        tir::ExprKind::Ident(name) => ctx.use_place(name, base.span),
        tir::ExprKind::FieldAccess { base: inner, .. } => check_place(inner, ctx),
        tir::ExprKind::Deref { expr: inner } => check_value(inner, ctx),
        _ => check_value(base, ctx),
    }
}

fn check_assign_target(target: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &target.kind {
        tir::ExprKind::Ident(name) => ctx.reinit(name, target.span),
        tir::ExprKind::FieldAccess { base, .. } => check_place(base, ctx),
        tir::ExprKind::Deref { expr: inner } => check_place(inner, ctx),
        _ => Ok(()), // infer.rs already restricts assignment targets to places
    }
}

/// A call's argument list: every direct `Ref` argument (an auto-borrow or
/// an explicit `&expr` written by the caller) is started *before any of
/// them are released*, so `start_borrow`'s own conflict check catches a
/// same-call aliasing violation like `f(&mut c, &c)` — not a separate
/// pass. All of them release together right after.
fn check_call(args: &[tir::Expr], ctx: &mut Ctx) -> Result<()> {
    let mut taken = Vec::new();
    for a in args {
        if let tir::ExprKind::Ref { mutable, expr: inner } = &a.kind {
            check_place(inner, ctx)?;
            if let Some(root) = tir_root_ident(inner) {
                ctx.start_borrow(root, *mutable, a.span)?;
                taken.push(root.to_string());
            }
        } else {
            check_value(a, ctx)?;
        }
    }
    for root in &taken {
        ctx.end_borrow(root);
    }
    Ok(())
}

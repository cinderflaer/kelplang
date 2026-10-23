//! `defer` lowering (kernel stage 6, `kelp wiki page language/statics-and-defer.md`).
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
//! `errdefer EXPR` is the same statement with a narrower exit set: it runs only
//! on an error exit, a `return` whose value is the failure variant (`Err` of a
//! `Result`, `None` of an `Option`) or a `?` that propagates one. Such an exit
//! is lowered to a `when` on the returned value, so the failure arm runs the
//! errdefers of every block it leaves (innermost first, each block's errdefers
//! before its ordinary defers) and the other arm runs the ordinary defers only.
//!
//! Every synthesized statement carries the `defer` statement's span, so a
//! diagnostic about the duplicated action names the `defer` and not the exit it
//! was spliced before. Synthesized names start with `#`, which cannot appear in
//! a lexed identifier, so they cannot collide with a written binding.

use std::collections::HashSet;

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::lexer::Span;

mod scan;
use scan::*;

const RET_PREFIX: &str = "#defer_ret";
const TMP_PREFIX: &str = "#defer_tmp";
const TAIL_PREFIX: &str = "#defer_tail";
const TRY_PREFIX: &str = "#errdefer_try";

/// Lowers every `defer` in `program`, attributing a refusal to `module`.
pub fn lower(program: &mut Program, module: &str) -> Result<(), Diagnostic> {
    let mut lowerer = Lowerer {
        module: module.to_string(),
        counter: 0,
        shape: None,
        closure_depth: 0,
        ret_ty: None,
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
    /// `true` for an `errdefer`: it runs on an error exit only.
    errdefer: bool,
}

/// The failure variant of a function's return type: `Err` (which carries a
/// payload) for a `Result`, `None` (which does not) for an `Option`. The
/// success variant is every other arm, so the lowering names only the failure.
#[derive(Clone, Copy)]
struct ErrShape {
    err: &'static str,
    payload: bool,
}

fn err_shape(return_type: Option<&TypeExpr>) -> Option<ErrShape> {
    let name = return_type?.name.rsplit('.').next()?;
    match name {
        "Result" => Some(ErrShape { err: "Err", payload: true }),
        "Option" => Some(ErrShape { err: "None", payload: false }),
        _ => None,
    }
}

/// The actions of a normal exit: the ordinary defers, errdefers left out.
fn ordinary(actions: &[Cleanup]) -> Vec<Cleanup> {
    actions.iter().filter(|c| !c.errdefer).cloned().collect()
}

/// The actions an exit has to run, already in execution order: the innermost
/// block's defers first, each newest first, then the enclosing blocks'.
#[derive(Clone, Default)]
struct Chain(Vec<Cleanup>);

impl Chain {
    /// The actions active after the `before` defers of the current block have
    /// run, followed by what an exit out of the enclosing blocks owes. Per
    /// block, the errdefers (newest first) come before the ordinary defers
    /// (newest first): on an error exit a block's errdefers run before its defers.
    fn exit(before: &[Cleanup], outer: &Chain) -> Chain {
        let mut actions: Vec<Cleanup> = before.iter().rev().filter(|c| c.errdefer).cloned().collect();
        actions.extend(before.iter().rev().filter(|c| !c.errdefer).cloned());
        actions.extend(outer.0.iter().cloned());
        Chain(actions)
    }

    fn has_errdefer(&self) -> bool {
        self.0.iter().any(|c| c.errdefer)
    }
}

struct Lowerer {
    module: String,
    counter: usize,
    /// The failure shape of the function being lowered, `None` when its return
    /// type is neither `Result` nor `Option`, and inside a `fun` literal.
    shape: Option<ErrShape>,
    /// How many `fun` literals enclose the code being lowered.
    closure_depth: usize,
    /// The declared return type of the function being lowered. An errdefer
    /// return binds its value with it, so a constructor such as
    /// `Result.Ok(..)` still infers its other type parameter from the signature.
    ret_ty: Option<TypeExpr>,
}

impl Lowerer {
    fn fresh(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    fn function(&mut self, f: &mut FunctionDecl) -> Result<(), Diagnostic> {
        let empty = Chain::default();
        self.shape = err_shape(f.return_type.as_ref());
        self.ret_ty = f.return_type.clone();
        self.closure_depth = 0;
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
    /// explicit `return` - one return site the cleanup and the elided-borrow
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
                    refuse_later_locals(&expr, &not_yet_here, span, &self.module, "defer")?;
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
                    active.push(Cleanup { expr, span, errdefer: false });
                }
                Stmt::Errdefer { expr, span } => {
                    if self.closure_depth > 0 {
                        return Err(Diagnostic::new(
                            "an 'errdefer' inside a 'fun' literal is not supported yet: the literal's own return is its error exit".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    if self.shape.is_none() {
                        return Err(Diagnostic::new(
                            "an 'errdefer' runs only on the error path, so it needs a function that returns Result or Option, and this one does not: declare the return type as Result<..> or Option<..>, or use 'defer' for a cleanup that runs on every exit".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    if let Some(return_span) = first_return_in_expr(&expr) {
                        return Err(Diagnostic::new(
                            "an errdefer action cannot contain 'return': the action runs while the function is already leaving on an error".to_string(),
                            return_span,
                            self.module.clone(),
                        )
                        .with_note(
                            "the 'errdefer' whose action this is".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    if let Some(try_span) = first_try_in_expr(&expr) {
                        return Err(Diagnostic::new(
                            "an errdefer action cannot contain '?': the action runs while the function is already leaving on an error, so a second early exit cannot be taken from it".to_string(),
                            try_span,
                            self.module.clone(),
                        )
                        .with_note(
                            "the 'errdefer' whose action this is".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    refuse_later_locals(&expr, &not_yet_here, span, &self.module, "errdefer")?;
                    if let Some(exit_span) = first_loop_exit_in_expr(&expr) {
                        return Err(Diagnostic::new(
                            "an errdefer action cannot contain 'break' or 'continue': the action runs while the function is leaving on an error".to_string(),
                            exit_span,
                            self.module.clone(),
                        )
                        .with_note(
                            "the 'errdefer' whose action this is".to_string(),
                            span,
                            self.module.clone(),
                        ));
                    }
                    let mut expr = expr;
                    let empty = Chain::default();
                    self.expr(&mut expr, &empty, &HashSet::new())?;
                    active.push(Cleanup { expr, span, errdefer: true });
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
                            } else if ordinary(&active).is_empty() {
                                new_stmts.push(Stmt::Expr { expr, span });
                            } else {
                                let normal = ordinary(&active);
                                let tail = self.fresh(TAIL_PREFIX);
                                let mut inner: Vec<Stmt> =
                                    Vec::with_capacity(normal.len() + 2);
                                inner.push(Stmt::Let {
                                    name: tail.clone(),
                                    mutable: false,
                                    ty: None,
                                    init: expr,
                                    span,
                                });
                                for c in normal.iter().rev() {
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
                            self.push_exit_actions(&mut new_stmts, &ordinary(&active));
                            new_stmts.push(Stmt::Break { span });
                        }
                        Stmt::Continue { span } => {
                            self.push_exit_actions(&mut new_stmts, &ordinary(&active));
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

        let normal = ordinary(&active);
        if falls_through && !tail_is_expr && !normal.is_empty() {
            for c in normal.iter().rev() {
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
        // With an errdefer owed, the value is bound with the declared return
        // type: the failure test then sees a value of the function's own type.
        // A numeric return type is named too, so a bare literal returned from a
        // block with defers takes the function's type and not the default `int`
        // (issue #275).
        let numeric = self.ret_ty.as_ref().filter(|t| {
            t.tuple.is_none()
                && t.args.is_empty()
                && matches!(
                    t.name.as_str(),
                    "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" | "int" | "f32" | "f64"
                )
        });
        let typed = if self.shape.is_some() && exit.has_errdefer() {
            self.ret_ty.clone()
        } else {
            numeric.cloned()
        };
        let ret = value.map(|value| {
            let name = self.fresh(RET_PREFIX);
            out.push(Stmt::Let {
                name: name.clone(),
                mutable: false,
                ty: typed,
                init: value,
                span,
            });
            name
        });
        match (&ret, self.shape) {
            // An errdefer is owed, so the returned value decides which actions
            // run: the failure arm runs everything `exit` owes, and any other
            // value runs the ordinary defers only.
            (Some(name), Some(shape)) if exit.has_errdefer() => {
                let failure = self.cleanup_stmts(exit.0.iter());
                let otherwise = self.cleanup_stmts(exit.0.iter().filter(|c| !c.errdefer));
                out.push(Stmt::Expr {
                    expr: failure_when(name, shape, failure, otherwise, span),
                    span,
                });
            }
            _ => {
                let ordinary_exit = self.cleanup_stmts(exit.0.iter().filter(|c| !c.errdefer));
                out.extend(ordinary_exit);
            }
        }
        out.push(Stmt::Return {
            value: ret.map(|name| Expr::Ident(name, span)),
            span,
        });
    }

    /// One `val #tmp = <action>` per action, in the order given.
    fn cleanup_stmts<'a>(&mut self, actions: impl Iterator<Item = &'a Cleanup>) -> Vec<Stmt> {
        let mut out = Vec::new();
        for c in actions {
            let name = self.fresh(TMP_PREFIX);
            out.push(Stmt::Let {
                name,
                mutable: false,
                ty: None,
                init: c.expr.clone(),
                span: c.span,
            });
        }
        out
    }

    /// `sub?` under errdefers. The failure is an exit the errdefers must see
    /// before the `?` returns, so `sub?` becomes a block that binds the operand,
    /// runs the errdefers when it is the failure variant, and then takes the
    /// `?` as written. The `?` keeps its `From` conversion and its drop plan.
    /// With no errdefer owed the `?` is left as it is.
    fn try_expr(
        &mut self,
        expr: &mut Expr,
        outer: &Chain,
        not_yet: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let Expr::Try { expr: sub, .. } = expr else {
            unreachable!("try_expr is only called on a '?'");
        };
        self.expr(sub, outer, not_yet)?;
        if !outer.has_errdefer() {
            return Ok(());
        }
        let shape = self
            .shape
            .expect("an errdefer is only recorded in a function with a failure shape");
        let Expr::Try { expr: sub, span } = expr else {
            unreachable!("checked above");
        };
        let span = *span;
        let operand = std::mem::replace(sub.as_mut(), Expr::BoolLit(false, span));
        let name = self.fresh(TRY_PREFIX);
        let failure = self.cleanup_stmts(outer.0.iter());
        let block = Block {
            stmts: vec![
                Stmt::Let {
                    name: name.clone(),
                    mutable: false,
                    ty: None,
                    init: operand,
                    span,
                },
                Stmt::Expr {
                    expr: failure_when(&name, shape, failure, Vec::new(), span),
                    span,
                },
                Stmt::Expr {
                    expr: Expr::Try {
                        expr: Box::new(Expr::Ident(name, span)),
                        span,
                    },
                    span,
                },
            ],
            span,
        };
        *expr = Expr::Block(block);
        Ok(())
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
            | Stmt::Errdefer { .. }
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
            Expr::Unary { expr, .. } | Expr::Ref { expr, .. } | Expr::Deref { expr, .. } => {
                self.expr(expr, outer, not_yet)
            }
            Expr::Try { .. } => self.try_expr(expr, outer, not_yet),
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
                let saved = self.shape.take();
                self.closure_depth += 1;
                let result = match body.as_mut() {
                    FunctionBody::Block(b) => self.block(b, &empty, &HashSet::new(), true),
                    FunctionBody::Expr(e) => self.expr(e, &empty, &HashSet::new()),
                    FunctionBody::Extern | FunctionBody::Signature => Ok(()),
                };
                self.closure_depth -= 1;
                self.shape = saved;
                result
            }
            Expr::Template { parts, .. } => {
                for part in parts {
                    if let TemplatePart::Expr(e) = part {
                        self.expr(e, outer, not_yet)?;
                    }
                }
                Ok(())
            }
            Expr::Unsafe { block, .. } | Expr::ManualBlock { body: block, .. } => self.block(block, outer, not_yet, false),
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

/// `when SCRUTINEE { Err(_) -> failure, _ -> otherwise }`: the failure arm
/// names only the failure variant, and a payload is ignored with `_`.
fn failure_when(scrutinee: &str, shape: ErrShape, failure: Vec<Stmt>, otherwise: Vec<Stmt>, span: Span) -> Expr {
    Expr::When {
        scrutinee: Box::new(Expr::Ident(scrutinee.to_string(), span)),
        arms: vec![
            WhenArm {
                pattern: Some(VariantPattern {
                    variant: shape.err.to_string(),
                    binds: if shape.payload { vec![None] } else { Vec::new() },
                    span,
                }),
                body: Expr::Block(Block { stmts: failure, span }),
                span,
            },
            WhenArm {
                pattern: None,
                body: Expr::Block(Block { stmts: otherwise, span }),
                span,
            },
        ],
        span,
        let_else: false,
    }
}

/// The first `panic(...)` as a block's tail, which diverges. Only this exact
/// shape is recognized, mirroring `infer::tail_diverges`, so the two passes
/// agree about which expression is the returned value.
fn is_panic_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if callee == "panic")
}

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
//! ## Drops (M6)
//!
//! The same walk decides where owned heap values (`Bytes`) are freed, and
//! returns that as a `DropPlan` — a side table keyed by *span*, so
//! `codegen_c.rs` can look up "which locals die here" as it writes each
//! block and each `return` without the two passes having to walk in
//! lockstep. Keying on spans also sidesteps the one place the two walks
//! genuinely differ: `while` bodies are checked twice (M3's double-move
//! trick) but emitted once.
//!
//! A local is dropped at the end of the block that declared it, if it is
//! still live there — moved out, and its new owner is responsible for it.
//! Every `return` first drops the frame's live owned locals (the value being
//! returned was itself checked before that, so a returned local is already
//! marked moved and is not in the set). Assignment to an owned `var`
//! re-arms the drop rather than freeing the old value, which leaks it: v1
//! has no user-defined destructors, so nothing can observe the difference
//! except memory use.
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

/// One owned local that has to be freed at a particular point.
#[derive(Debug, Clone)]
pub struct DropTarget {
    pub name: String,
    pub ty: Ty,
}

/// Where every owned local is freed, per function.
///
/// Keyed by span rather than by position because `codegen_c.rs` walks the
/// same tree independently: a block's span identifies its end-of-scope
/// drops, and a `return` statement's span identifies the drops that run
/// before it returns.
#[derive(Debug, Default, Clone)]
pub struct DropPlan {
    functions: HashMap<String, FnDrops>,
}

#[derive(Debug, Default, Clone)]
pub struct FnDrops {
    at_block_end: HashMap<Span, Vec<DropTarget>>,
    at_return: HashMap<Span, Vec<DropTarget>>,
    at_assign: HashMap<Span, Vec<DropTarget>>,
}

impl DropPlan {
    /// Owned locals freed when `span`'s block ends, in drop order (reverse
    /// declaration order, so a later binding is freed before an earlier one
    /// it may have taken its value from).
    pub fn at_block_end(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_block_end.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Owned locals freed by the `return` statement at `span`, before it
    /// returns.
    pub fn at_return(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_return.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The old value of an owned binding, freed by the assignment at `span`
    /// *after* the new value has been read but *before* it is stored. `b = b`
    /// is deliberately not one of these: the value being stored *is* the old
    /// value, so freeing it first would leave the binding dangling.
    pub fn at_assign(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_assign.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

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
    /// The binding's type: `Copy`-ness comes from here, and so does whether
    /// the old value needs freeing when an assignment overwrites it.
    ty: Ty,
    moved_at: Option<Span>,
    borrow: BorrowKind,
}

/// An owned local declared in some scope, tracked for the drop plan.
#[derive(Clone)]
struct OwnedVar {
    name: String,
    ty: Ty,
    /// Set when this binding's value is moved elsewhere; a moved-out local is
    /// never dropped by the scope that declared it.
    moved: bool,
}

#[derive(Clone, Default)]
struct Scope {
    vars: HashMap<String, VarState>,
    /// Roots borrowed by a `val`/`var` declared in this scope, released
    /// when the scope pops (see the module doc's "scope-durable" lifetime).
    scope_borrows: Vec<String>,
    /// Owned (heap-owning) locals declared in this scope, in declaration
    /// order — the candidates for this block's end-of-scope drops.
    owned: Vec<OwnedVar>,
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
            vars.insert(name, VarState { ty: va.ty.clone(), moved_at, borrow: va.borrow });
        }
        // Owned locals declared in *this* scope survive the merge (an `if`
        // arm or loop body is always its own block, so nothing here was
        // declared inside one). A local is treated as moved if *either* side
        // moved it: that can leak in the branch that did not — safe — where
        // the other choice would free a value the other branch had already
        // handed to a new owner.
        let mut owned = a.owned;
        for entry in owned.iter_mut() {
            if let Some(other) = b.owned.iter().find(|o| o.name == entry.name) {
                entry.moved = entry.moved || other.moved;
            }
        }
        Scope { vars, scope_borrows: Vec::new(), owned }
    }
}

struct Ctx {
    scopes: Vec<Scope>,
    plan: FnDrops,
    /// Which `datac`s own heap memory (transitively) — `Ty::Struct(_)`
    /// cannot answer that on its own, so the struct table decision is made
    /// once in `check_program` and threaded here.
    owned_structs: std::collections::HashSet<String>,
    /// For every callee whose result is a borrow: the parameter index the
    /// elided lifetime binds to (M6.3). Both `.x` functions and builtins
    /// follow the same rule, so both are in here.
    fn_borrow_root: HashMap<String, Option<usize>>,
    /// The current function's own return-borrow rule: the name of the
    /// parameter a returned borrow must be rooted at, when the function
    /// declares a borrowed return type. `None` with `ret_is_borrow` set
    /// means the function takes no reference parameter at all, so only a
    /// `str` literal (static storage) is a legal returned borrow.
    ret_borrow_param: Option<String>,
    ret_is_borrow: bool,
    /// The declared return type, for the wording of that diagnostic.
    ret_ty: Ty,
}

impl Ctx {
    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// Ends a block: releases its scope-durable borrows and records the drops
    /// for the owned locals it declared that are still live here.
    fn pop_scope(&mut self, block_span: Span, unreachable_end: bool) {
        let popped = self.scopes.pop().expect("pop_scope without a matching push_scope");
        for root in popped.scope_borrows {
            self.end_borrow(&root);
        }
        // A block that ends by returning never reaches its end, and the
        // return's own drops already cover everything live at that point.
        if unreachable_end {
            return;
        }
        let drops = live_drops(popped.owned.iter());
        if !drops.is_empty() {
            self.plan.at_block_end.insert(block_span, drops);
        }
    }

    /// Every live owned local in *every* enclosing scope, innermost first —
    /// what a `return` from here would have to free before leaving.
    fn live_drops_in_frame(&self) -> Vec<DropTarget> {
        let mut out = Vec::new();
        for scope in self.scopes.iter().rev() {
            out.extend(live_drops(scope.owned.iter()));
        }
        out
    }

    /// Marks the innermost owned binding named `name` as moved out.
    fn mark_moved(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.moved = true;
                return;
            }
        }
    }

    /// Marks the innermost owned binding named `name` as owning again (an
    /// assignment of a fresh value into it).
    fn mark_owned_again(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.moved = false;
                return;
            }
        }
    }

    fn define(&mut self, name: &str, ty: &Ty) {
        self.scopes.last_mut().unwrap().vars.insert(
            name.to_string(),
            VarState {
                ty: ty.clone(),
                moved_at: None,
                borrow: BorrowKind::None,
            },
        );
    }

    /// Registers an owned local so its drop point can be planned.
    fn define_owned(&mut self, name: &str, ty: &Ty) {
        let scope = self.scopes.last_mut().unwrap();
        scope.owned.retain(|o| o.name != name);
        scope.owned.push(OwnedVar {
            name: name.to_string(),
            ty: ty.clone(),
            moved: false,
        });
    }

    /// Whether any scope in the frame already binds `name`.
    fn knows(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|s| s.vars.contains_key(name))
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
            return Err(BorrowError {
                message: format!("use of moved value {}", display_root(name)),
                span,
            });
        }
        if v.borrow != BorrowKind::None {
            return Err(BorrowError {
                message: format!("cannot move {} while it is borrowed", display_root(name)),
                span,
            });
        }
        if !v.ty.is_copy() {
            v.moved_at = Some(span);
        }
        Ok(())
    }

    /// Like `use_value`, but also tells the drop planner that the binding's
    /// value now lives somewhere else.
    fn use_value_moving(&mut self, name: &str, span: Span) -> Result<()> {
        let was_copy = self
            .lookup_mut(name)
            .map(|v| v.ty.is_copy())
            .unwrap_or(true);
        self.use_value(name, span)?;
        if !was_copy {
            self.mark_moved(name);
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
            return Err(BorrowError {
                message: format!("use of moved value {}", display_root(name)),
                span,
            });
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
            return Err(BorrowError {
                message: format!("cannot assign to {} while it is borrowed", display_root(name)),
                span,
            });
        }
        v.moved_at = None;
        Ok(())
    }

    /// Like `reinit`, but also re-arms the drop (the fresh value needs
    /// freeing; the old one is leaked — see the module doc).
    fn reinit_moving(&mut self, name: &str, span: Span) -> Result<()> {
        self.reinit(name, span)?;
        self.mark_owned_again(name);
        Ok(())
    }

    fn start_borrow(&mut self, root: &str, mutable: bool, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(root) else {
            return Ok(());
        };
        if v.moved_at.is_some() {
            return Err(BorrowError {
                message: format!("cannot borrow {}: value was moved", display_root(root)),
                span,
            });
        }
        v.borrow = match (&v.borrow, mutable) {
            (BorrowKind::Exclusive, _) => {
                return Err(BorrowError {
                    message: format!(
                        "cannot borrow {}: already borrowed exclusively",
                        display_root(root)
                    ),
                    span,
                });
            }
            (BorrowKind::Shared(_), true) => {
                return Err(BorrowError {
                    message: format!(
                        "cannot borrow {} as exclusive: already borrowed as shared",
                        display_root(root)
                    ),
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

    /// The parameter index a callee's returned borrow is elided from, for a
    /// `.x` function or a builtin method.
    fn borrow_root_param_of(&self, callee: &str) -> Option<usize> {
        match self.fn_borrow_root.get(callee).copied() {
            Some(v) => v,
            None => crate::infer::builtin_borrow_root(callee)?,
        }
    }

    /// The elided-borrow rule, enforced at every return site of a function
    /// that declares a borrowed return type: the value must be derived from
    /// the first reference parameter (or be a `str` literal, whose storage is
    /// static). This is what makes `&str` return elision sound without real
    /// lifetime inference — the caller keeps the owner borrowed for as long
    /// as the view lives (see `check_call`), and the callee cannot hand back
    /// anything that did not come from that same owner.
    fn check_return_borrow(&mut self, value: &tir::Expr) -> Result<()> {
        if !self.ret_is_borrow {
            return Ok(());
        }
        let root = borrow_root(value, self);
        if root.as_deref() == Some(STATIC_ROOT) {
            return Ok(());
        }
        if let (Some(root), Some(param)) = (root, self.ret_borrow_param.clone()) {
            if root == param {
                return Ok(());
            }
            return Err(BorrowError {
                message: format!(
                    "cannot return a borrow of '{root}': '{}' borrows from its first reference parameter '{param}', so the returned value must be derived from '{param}'",
                    self.ret_ty.display()
                ),
                span: value.span,
            });
        }
        Err(BorrowError {
            message: match &self.ret_borrow_param {
                Some(param) => format!(
                    "cannot return this value as '{}': it is not derived from a reference parameter (an elided return lifetime is '{param}', or a 'str' literal)",
                    self.ret_ty.display()
                ),
                None => format!(
                    "cannot return this value as '{}': with no reference parameter to borrow from, only a 'str' literal can be returned (its storage is static)",
                    self.ret_ty.display()
                ),
            },
            span: value.span,
        })
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
    ty.is_copy()
}

/// A root name as a person wrote it. A passdown's hidden parameter is
/// `pd#<qualified name>`, which no user ever typed, so diagnostics translate
/// it back (M6.4).
fn display_root(name: &str) -> String {
    match name.strip_prefix("pd#") {
        Some(qualified) => format!("passdown '{qualified}'"),
        None => format!("'{name}'"),
    }
}

/// The drops for a set of owned locals: reverse declaration order, live ones
/// only. Reverse order matches the lexical nesting of the bindings — a value
/// moved out of an earlier binding into a later one is freed exactly once,
/// by the later one, and before the earlier one would have been.
fn live_drops<'a>(owned: impl DoubleEndedIterator<Item = &'a OwnedVar>) -> Vec<DropTarget> {
    owned
        .rev()
        .filter(|o| !o.moved)
        .map(|o| DropTarget {
            name: o.name.clone(),
            ty: o.ty.clone(),
        })
        .collect()
}

/// The root identifier a place-expression is ultimately reached through.
fn tir_root_ident(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => Some(name),
        tir::ExprKind::FieldAccess { base, .. } => tir_root_ident(base),
        tir::ExprKind::Deref { expr: inner } => tir_root_ident(inner),
        // A `pd` read in `main`/an initializer is `Deref(Ref(global))` — the
        // explicit borrow of the generated global — so the chain has to
        // descend through that `Ref` for the borrow to be tracked at all
        // (M6.4).
        tir::ExprKind::Ref { expr: inner, .. } => tir_root_ident(inner),
        _ => None,
    }
}

/// The root of a call argument, which is usually an auto-borrow node that
/// `check_call_arg` synthesized — so this unwraps one `Ref` before asking
/// `tir_root_ident`.
fn arg_root(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ref { expr: inner, .. } => tir_root_ident(inner),
        _ => tir_root_ident(expr),
    }
}

/// Stands in for a `str` literal's storage: static, so it satisfies any
/// return lifetime and never pins an owner.
const STATIC_ROOT: &str = "<static>";

/// The variable a borrow is ultimately derived from. A borrow-returning call
/// chains back through its own elided lifetime, so `trim_start(s)` roots at
/// `s`, and `f(s.as_str())` roots at whatever `s` was borrowed from. `None`
/// when the value has no such root.
fn borrow_root(expr: &tir::Expr, ctx: &Ctx) -> Option<String> {
    match &expr.kind {
        tir::ExprKind::StringLit(_) => Some(STATIC_ROOT.to_string()),
        tir::ExprKind::Ident(name) => Some(name.clone()),
        tir::ExprKind::FieldAccess { base, .. }
        | tir::ExprKind::Ref { expr: base, .. }
        | tir::ExprKind::Deref { expr: base } => borrow_root(base, ctx),
        tir::ExprKind::Call { callee, args } => {
            let idx = ctx.borrow_root_param_of(callee)?;
            borrow_root(args.get(idx)?, ctx)
        }
        // `if`/`when` in value position: every branch has to agree, or the
        // value has no single owner — which is exactly why
        // `fun classify(n: i32).R: &str = when { ... "pos" ... }` works
        // (all branches are literals) and a branch mixing a parameter with a
        // literal does not.
        tir::ExprKind::Block(b) => block_tail_root(b, ctx),
        tir::ExprKind::If {
            then_block,
            else_block,
            ..
        } => {
            let then_root = block_tail_root(then_block, ctx)?;
            let else_root = block_tail_root(else_block.as_ref()?, ctx)?;
            if then_root == else_root {
                Some(then_root)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn block_tail_root(block: &tir::Block, ctx: &Ctx) -> Option<String> {
    borrow_root(block.tail.as_deref()?, ctx)
}

pub fn check_program(program: &tir::Program) -> std::result::Result<DropPlan, Diagnostic> {
    let mut plan = DropPlan::default();
    let owned = crate::types::owned_structs(&program.structs);
    // Every callee whose result is a borrow, with the parameter its elided
    // lifetime binds to. Builtins (`String.as_str`) join the same table, so
    // `check_call` has one rule for both.
    let mut fn_borrow_root: HashMap<String, Option<usize>> = HashMap::new();
    for f in &program.functions {
        if matches!(f.ret, Ty::Ref { .. }) {
            fn_borrow_root.insert(f.name.clone(), crate::infer::borrow_root_param(&f.params));
        }
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
        let function = check_function(f, &owned, &fn_borrow_root, &globals).map_err(|e| {
            Diagnostic::new(e.message, e.span, f.module.clone())
        })?;
        plan.functions.insert(f.name.clone(), function);
    }
    Ok(plan)
}

fn check_function(
    f: &tir::Function,
    owned_structs: &std::collections::HashSet<String>,
    fn_borrow_root: &HashMap<String, Option<usize>>,
    globals: &[(String, Ty)],
) -> Result<FnDrops> {
    let mut ctx = Ctx {
        scopes: vec![Scope::default()],
        plan: FnDrops::default(),
        owned_structs: owned_structs.clone(),
        fn_borrow_root: fn_borrow_root.clone(),
        // A borrowed return type borrows from the first reference
        // parameter — `infer.rs` has already rejected a function that
        // returns one without taking any.
        ret_borrow_param: if matches!(f.ret, Ty::Ref { .. }) {
            crate::infer::borrow_root_param(&f.params).map(|i| f.params[i].0.clone())
        } else {
            None
        },
        ret_is_borrow: matches!(f.ret, Ty::Ref { .. }),
        ret_ty: f.ret.clone(),
    };
    for (name, ty) in &f.params {
        ctx.define(name, ty);
        // A parameter is never owned in v1 (`Bytes`/`String` parameters are
        // borrows), but registering one costs nothing and keeps the plan
        // honest if move-taking parameters ever exist.
        if crate::types::needs_drop(ty, &ctx.owned_structs) {
            ctx.define_owned(name, ty);
        }
    }
    // A passdown the function does not receive as a hidden parameter is
    // still reachable storage (`main` writes the globals directly), so it
    // gets a binding of its own value type — unless a parameter of the same
    // hidden name already claimed the slot, which is the normal case for
    // every function other than `main`.
    for (hidden, value_ty) in globals {
        if !ctx.knows(hidden) {
            ctx.define(hidden, value_ty);
        }
    }
    check_block(&f.body, &mut ctx)?;

    // A tail expression is the function's returned value, so it is bound by
    // the same elision rule an explicit `return` is.
    if let Some(tail) = &f.body.tail {
        ctx.check_return_borrow(tail)?;
    }

    // Anything still owned in the frame at the end belongs to the function
    // body's own block (parameters live as long as the body does).
    let leftover = ctx.live_drops_in_frame();
    if !leftover.is_empty() {
        ctx.plan.at_block_end.insert(f.body.span, leftover);
    }
    Ok(ctx.plan)
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
    ctx.pop_scope(block.span, diverges(block));
    result
}

fn check_stmt(stmt: &tir::Stmt, ctx: &mut Ctx) -> Result<()> {
    match stmt {
        tir::Stmt::Let { name, ty, init } => {
            check_let_init(init, ctx)?;
            ctx.define(name, ty);
            if crate::types::needs_drop(ty, &ctx.owned_structs) {
                ctx.define_owned(name, ty);
            }
            Ok(())
        }
        tir::Stmt::While { cond, body } => check_while(cond, body, ctx),
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

/// Whether a block ends by returning. A branch (or loop body) that cannot
/// fall through must not contribute its post-state to a merge: values it had
/// to move before returning are still available to the code that runs after
/// it, and marking them moved would reject ordinary `return early` code.
fn diverges(block: &tir::Block) -> bool {
    matches!(block.stmts.last(), Some(tir::Stmt::Return { .. }))
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
    // *definitely* moved unless it already was before the loop. A body that
    // always returns never falls through at all, so the loop leaves the
    // pre-loop state untouched.
    if diverges(body) {
        ctx.restore(before);
        return Ok(());
    }
    let after = ctx.snapshot();
    ctx.merge_into(before, after);
    Ok(())
}

fn check_if(then_block: &tir::Block, else_block: &Option<tir::Block>, ctx: &mut Ctx) -> Result<()> {
    let before = ctx.snapshot();

    check_block(then_block, ctx)?;
    let after_then = ctx.snapshot();

    ctx.restore(before.clone());
    if let Some(eb) = else_block {
        check_block(eb, ctx)?;
    }
    let after_else = ctx.snapshot();

    // A branch that returns cannot fall through, so only the other branch's
    // state carries on (`before` when there is no other branch).
    match (diverges(then_block), else_block.as_ref().map(diverges)) {
        (true, Some(false)) => ctx.restore(after_else),
        (true, _) => ctx.restore(before),
        (false, Some(true)) => ctx.restore(after_then),
        _ => ctx.merge_into(after_then, after_else),
    }
    Ok(())
}

/// Checks an expression used as a whole value: a bare `Ident` here moves a
/// non-Copy binding, and `*p` here (not just read *through*) is rejected
/// when the pointee is non-Copy — mirrors Rust's "cannot move out of a
/// shared reference."
fn check_value(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => ctx.use_value_moving(name, expr.span),
        tir::ExprKind::FieldAccess { base, .. } => check_place(base, ctx),
        tir::ExprKind::IntLit(_) | tir::ExprKind::FloatLit(_) | tir::ExprKind::StringLit(_) | tir::ExprKind::BoolLit(_) => Ok(()),
        tir::ExprKind::Unary { expr: sub, .. } => check_value(sub, ctx),
        tir::ExprKind::Binary { lhs, rhs, .. } => {
            check_value(lhs, ctx)?;
            check_value(rhs, ctx)
        }
        tir::ExprKind::Call { .. } => check_call(expr, ctx),
        tir::ExprKind::Template { pieces, .. } => {
            for piece in pieces {
                if let tir::TemplatePiece::Value(v) = piece {
                    check_value(v, ctx)?;
                }
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
            check_assign_target(target, ctx)?;
            // Assigning a fresh value into an owned binding frees the old one
            // (it has no other owner and v1 has no user destructors, so
            // nothing could observe it either way — freeing it is simply the
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
                if owned && !self_assign {
                    ctx.plan.at_assign.insert(
                        expr.span,
                        vec![DropTarget {
                            name: name.clone(),
                            ty: target.ty.clone(),
                        }],
                    );
                }
            }
            Ok(())
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
        tir::ExprKind::Ident(name) => ctx.reinit_moving(name, target.span),
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
///
/// The one exception is a call that *returns* a borrow (M6.3): the argument
/// the elided lifetime binds to is left borrowed for the enclosing scope, so
/// the caller cannot move or free the owner while the view lives.
fn check_call(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    let tir::ExprKind::Call { callee, args } = &expr.kind else {
        unreachable!("check_call is only called on a Call node")
    };
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
    if matches!(expr.ty, Ty::Ref { .. }) {
        let idx = ctx.borrow_root_param_of(callee);
        if let Some(root) = idx
            .and_then(|i| args.get(i))
            .and_then(arg_root)
        {
            // Held for the scope the result lives in, exactly like
            // `val p = &c` (see `check_let_init`).
            ctx.register_scope_borrow(root);
            taken.retain(|r| r != root);
        }
    }
    for root in &taken {
        ctx.end_borrow(root);
    }
    Ok(())
}

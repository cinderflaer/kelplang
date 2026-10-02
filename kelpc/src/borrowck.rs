//! M3 (moves) + M4 (borrowing) checking over the typed IR.
//!
//! ## Moves (M3)
//!
//! Rule: primitives (and `&str`, which has no owned-string type to move —
//! it's just a borrowed pointer) are `Copy`; `datac` values are not. A bare
//! `Ident` read of a non-Copy value moves it out of its binding; a `.field`
//! read walks through a place chain without consuming anything, except the one
//! case A-11 added: a field read directly off an owned local is a partial move,
//! which takes that field out and clears the slot it held.
//!
//! ## Borrowing (M4)
//!
//! Per `plan/syntax.md` §5, a `datac`-typed function parameter is an
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
//! returns that as a `DropPlan` — a side table keyed by *node identity*, so
//! `codegen_c.rs` can look up "which locals die here" as it writes each
//! block and each `return` without the two passes having to walk in
//! lockstep. Keying on identity also sidesteps the one place the two walks
//! genuinely differ: `while` bodies are checked twice (M3's double-move
//! trick) but emitted once.
//!
//! Identity rather than source span is what makes the key unique. Parse-time
//! desugaring deliberately gives several *distinct* blocks one user span
//! (`if let`'s empty `else` block carries the `then` block's span; a `for`
//! loop's element block and its `while` body both carry the loop body's span),
//! and a span-keyed plan then handed one block's drops to another block that
//! merely shared its span — which is how an owned local declared in an `if
//! let` arm or a `for` body got a second, out-of-frame drop emitted on the
//! path where its name is not even declared (BUG-1). Both passes walk the very
//! same `tir::Program`, so the node's address is a stable and exact key.
//!
//! A local is dropped at the end of the block that declared it, if it is
//! still live there — moved out, and its new owner is responsible for it.
//! Every `return` first drops the frame's live owned locals (the value being
//! returned was itself checked before that, so a returned local is already
//! marked moved and is not in the set). Assignment to an owned `var` frees
//! the old value before the new one is stored (`DropPlan::at_assign`),
//! except for `b = b` (the value being stored *is* the old value) and
//! except when the value expression already moved the binding out
//! (`x = f(x)`, where `f` takes `x` by value) — there the free belongs to
//! whichever path still holds the old value, not to this assignment. The
//! same freeing now also covers an assignment through a dereferenced borrow
//! (`*p = value`) and to an owned field (`h.name = value`), which have no
//! binding to key a `DropTarget` to at all (roadmap open item 15, cases 2
//! and 3) — see `DropTarget::binding`'s doc.
//!
//! One owned entry is kept per *binding*, not per spelling: two `var a`
//! declarations in one block own two values, so each declaration keeps its own
//! entry and its own drop. The identity is the declaration's own node (a
//! `move T` parameter, which is no statement, is keyed by its position in the
//! parameter list). A read, a move or a partial move still resolves a name to
//! the innermost binding of that spelling, which is what the language's own
//! scoping and both backends' name resolution pick. Keyed by spelling, a
//! second declaration took the first binding's entry away, and the value the
//! first one owned had no drop left that reached it: valgrind reported its
//! buffer as definitely lost (see `tests/fixtures/owned_rebind.kp`).
//!
//! **A move clears its source**, and that is what makes a *conditional* move
//! leak-free. A local moved on one path through a branch or loop body and not
//! on another is neither "live" nor "moved": dropping it would free a value
//! a new owner already holds on one path, and not dropping it leaks on the
//! other. The plan therefore keeps such a local in every drop set it reaches
//! (`MoveState::MaybeMoved`), and records the *move sites* themselves
//! (`FnDrops::at_move`, keyed by the moved expression's own identity). Codegen
//! renders a read at one of those sites as "copy the value out, then zero the
//! binding", so the drop that runs is a free of nothing on the path that
//! moved and a real free on the path that did not. That is also why an
//! assignment to an owned binding frees the old value whenever the binding is
//! not *definitely* moved: in the cleared case the free is of the zero value.
//!
//! Without this, an owned local bound inside a loop body leaked whenever one
//! path through the body moved it and another abandoned it: valgrind reported
//! `3 bytes in 1 blocks` lost on the loop-shape repro before this fix (the C
//! backend's `-O2` build read clean because clang deletes the allocation nothing
//! reads, which is why the gate is valgrind). See
//! `tests/fixtures/owned_conditional_move.kp` for the regression.
//! **A reference binding remembers its owner (open item 33).** A binding of
//! reference type carries a root set — the owners its value may borrow from —
//! and `cur = expr` REPLACES it: the old roots' pins are released at the
//! assignment and the new ones are pinned to the binding's last use. A borrow
//! made through a cursor is therefore a borrow of the owner, which is why
//! `var cur = &list; cur = cur.next.get()` is accepted while mutating `list`
//! under a live cursor stays refused. `plan/provenance.md` is the rule and the
//! algorithm.
//!
//! Also out of scope: receiver-form methods (shipped since), and `&str`
//! return-lifetime elision (moot for now since `&str` isn't routed through
//! `Ty::Ref` at all).
//!
//! The `EnumField` arm of `check_place` has no `use_field_place` analogue, so
//! a *place* read of an already-moved payload is not refused. That is
//! unreachable while surface syntax cannot name an enum's payload as a place,
//! and it becomes a real hole the day payloads gain field syntax.

use std::collections::HashMap;

use crate::ast::AsmOperandKind;
use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::tir;
use crate::types::{EnumDef, Ty};

/// One owned local (or, for `at_assign`, one owned *place*) that has to be
/// freed at a particular point.
#[derive(Debug, Clone)]
pub struct DropTarget {
    /// The binding whose value this frees. It is what codegen resolves the
    /// drop to, so a spelling that names several bindings (a rebound `var`, or
    /// a local shadowing a `move` parameter) frees the value the plan decided
    /// about rather than the newest binding of that spelling (BUG-n7Wq3).
    ///
    /// `None` in two different shapes, both without a declaration to key to:
    /// an assignment to a `pd`'s storage, which name resolution reaches like
    /// any global (`name` is still that global's spelling then); and an
    /// `at_assign` entry for a `*p = value` or `p.field = value` /
    /// `h.name = value` target (`FieldAccess`/`Deref` — roadmap open item 15,
    /// cases 2 and 3), where `name` is empty and there is no binding *or*
    /// global to fall back to at all — codegen has to recompute the target's
    /// own place expression a second time instead of resolving a name.
    pub binding: Option<BindingId>,
    pub name: String,
    pub ty: Ty,
}

/// One place reached from an owned binding without going through a borrow.
///
/// A partial move gives one of these away and leaves the rest of the binding
/// owned, so the binding is still freed as a whole and exactly the projection
/// the move emptied has to be cleared first. Anything reached through a borrow
/// is not a `Projection`: the frame does not own it, so it cannot give it away.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Projection {
    /// One field of a `datac`, by name (each backend resolves the name to the
    /// field's index from the owning struct).
    Field(String),
    /// One payload field of one variant of an `enum`, by the two indices the
    /// `tir` node carries. The variant is part of the identity because two
    /// variants may name a payload field the same; `name` is the payload
    /// field's own name, and is there only so a diagnostic reads the way a
    /// person wrote the place.
    EnumPayload {
        variant: usize,
        field: usize,
        name: String,
    },
}

/// How an owned local's value stands at the point being checked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MoveState {
    /// Still owned by the binding: the scope that declared it frees it.
    Live,
    /// Moved out on every path that reaches here. A new owner holds the
    /// value, this binding is dead, and nothing frees it here.
    Moved,
    /// Moved out on some path and not on another — a branch or a loop body
    /// that moved it joined with one that did not. The binding still has to
    /// be freed (the moved path cleared its slot, so that free is of the
    /// zero value), and the value is no longer readable: the borrow state
    /// treats it as moved.
    MaybeMoved,
}

impl MoveState {
    /// The state two paths agree on. Anything the two do not both say is
    /// `MaybeMoved`, which is the only conservative answer: `Moved` would
    /// drop the path that kept the value on the floor, and `Live` would free
    /// a value the other path handed to a new owner.
    fn merge(self, other: MoveState) -> MoveState {
        if self == other {
            self
        } else {
            MoveState::MaybeMoved
        }
    }
}

/// Where every owned local is freed, per function.
///
/// Keyed by node identity rather than by position because `codegen_c.rs` walks
/// the same tree independently: a block identifies its own end-of-scope drops,
/// and a `return` statement identifies the drops that run before it returns.
/// See the module doc for why a source span cannot serve as that key.
#[derive(Debug, Default, Clone)]
pub struct DropPlan {
    functions: HashMap<String, FnDrops>,
}

/// The identity of one `tir` node, for the plan's key.
///
/// `borrowck.rs` and `codegen_c.rs` walk the same `tir::Program`, so the same
/// node is reached by both, and its address is a stable key for as long as the
/// plan outlives the walk: nothing moves or copies the tree between the two
/// passes, and every block is reached through a `Box` or a `Vec` buffer, so the
/// address does not change under it either.
type NodeId = usize;

fn block_id(block: &tir::Block) -> NodeId {
    block as *const tir::Block as NodeId
}

/// The identity of one `tir::Stmt` node, for the binding a declaration
/// introduces: the same address argument as `block_id`, and for the same
/// reason.
pub fn stmt_id(stmt: &tir::Stmt) -> NodeId {
    stmt as *const tir::Stmt as NodeId
}

/// What identifies one owned binding, so a drop reaches the value that binding
/// owns rather than whichever binding of the same spelling is newest.
///
/// The identity is not the name: one block may declare `var a` twice, and each
/// declaration owns a value of its own (BUG-n7Wq3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingId {
    /// A `val`/`var` declaration, by the identity of the `Stmt::Let` that
    /// declared it (`stmt_id`), which codegen computes for the same node.
    Local(NodeId),
    /// A `move T` parameter, by its index in the function's parameter list: a
    /// parameter is not a statement, so there is no node to key it to, and the
    /// index is the identity both backends already walk that list by.
    Param(usize),
}

/// The identity of one `tir::Expr` node, for the move-site key — the same
/// identity argument as `block_id`, and for the same reason.
fn expr_id(expr: &tir::Expr) -> NodeId {
    expr as *const tir::Expr as NodeId
}

#[derive(Debug, Default, Clone)]
pub struct FnDrops {
    at_block_end: HashMap<NodeId, Vec<DropTarget>>,
    at_return: HashMap<Span, Vec<DropTarget>>,
    at_assign: HashMap<Span, Vec<DropTarget>>,
    at_try: HashMap<Span, Vec<DropTarget>>,
    /// The expressions that *move* an owned binding's value out of it and
    /// therefore have to clear the binding's slot after copying the value out
    /// — one entry per moved expression, so codegen can ask about the node it
    /// is already rendering. Only bindings that some path leaves live get an
    /// entry: a binding moved on every path is dropped nowhere, so its slot's
    /// contents are never read again and clearing it would be dead code.
    at_move: std::collections::HashSet<NodeId>,
    /// The expressions that move one *projection* of an owned binding out (a
    /// `datac` field or an `enum` payload), and the projection each one
    /// emptied. A binding a projection was taken from stays live, so it is
    /// still in a drop set wherever it was before, and the projection's slot
    /// has to be zeroed at the move or that drop frees the value a second
    /// time. Unlike `at_move` this needs no filter: the binding is dropped on
    /// every path, not only on some.
    at_clear: HashMap<NodeId, Projection>,
}

impl DropPlan {
    /// Owned locals freed when `block` ends, in drop order (reverse
    /// declaration order, so a later binding is freed before an earlier one
    /// it may have taken its value from).
    pub fn at_block_end(&self, function: &str, block: &tir::Block) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_block_end.get(&block_id(block)))
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

    /// The old value overwritten by the assignment at `span`, freed *after*
    /// the new value has been read but *before* it is stored. `b = b` is
    /// deliberately not one of these: the value being stored *is* the old
    /// value, so freeing it first would leave the binding dangling.
    ///
    /// Two shapes: an `Ident` target's entry names a `binding` codegen
    /// resolves the usual way, and a `FieldAccess`/`Deref` target's entry
    /// (`*p = value`, `p.field = value`, `h.name = value`; roadmap open item
    /// 15, cases 2 and 3) has `binding: None` and an empty `name` — there is
    /// no declaration to key to, only a place, so codegen recomputes the
    /// target expression's own address again rather than looking one up.
    pub fn at_assign(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_assign.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Owned locals freed by the `?` expression at `span`, on its failure arm
    /// *only*. `expr?` is a `return` in disguise: the failure value is rebuilt
    /// in the enclosing function's return type and returned from the function
    /// the expression sits in, so every owned local still live in the frame has
    /// to be freed first — the same set and the same order a written `return`
    /// uses (see `at_return`). Without this, `?` leaked the whole frame on
    /// every failure, which for a recursive-descent parser is every rejected
    /// document.
    pub fn at_try(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_try.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Whether the value read at `expr` is a move out of an owned binding
    /// whose slot codegen has to clear — copy the value out, then zero the
    /// binding. See the module doc's "a move clears its source".
    pub fn moves_at(&self, function: &str, expr: &tir::Expr) -> bool {
        self.functions
            .get(function)
            .map(|f| f.at_move.contains(&expr_id(expr)))
            .unwrap_or(false)
    }

    /// The projection the value read at `expr` moved out of its base, when
    /// that read is a partial move. Codegen copies the value out and then
    /// zeroes this slot, so the base's own drop frees nothing where the value
    /// used to be (see the module doc's "a move clears its source").
    pub fn clear_at(&self, function: &str, expr: &tir::Expr) -> Option<&Projection> {
        self.functions
            .get(function)
            .and_then(|f| f.at_clear.get(&expr_id(expr)))
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
    /// A pending exclusive borrow, taken for a method call's receiver and not
    /// yet committed (MISSING-n4Bz7). While it is set the place may still be
    /// *read* and shared-borrowed — that is what lets
    /// `counts.insert(k, counts.get_or(..) + 1)` type-check — but not moved
    /// or exclusively borrowed again, and committing turns it into
    /// `borrow = Exclusive`, which fails if a shared borrow taken meanwhile is
    /// still live (`f(&mut c, &c)`).
    reserved: bool,
    /// The owner roots this binding's value may borrow from, when the binding
    /// *is* a reference. The provenance a pattern binding would otherwise lose,
    /// and what makes a pointer local usable as a cursor (open item 33). Empty
    /// means no recorded provenance: the binding's own name is its root. See
    /// `set_roots` and `plan/provenance.md`.
    roots: Vec<String>,
}

/// An owned local declared in some scope, tracked for the drop plan.
#[derive(Clone)]
struct OwnedVar {
    /// Which binding this is: one entry per binding, never one per spelling,
    /// so a second declaration of a name does not take the first binding's
    /// drop away from it (see `define_owned`).
    binding: BindingId,
    name: String,
    ty: Ty,
    /// Whether this binding's value is still here; a definitely moved-out
    /// local is never dropped by the scope that declared it (see
    /// `MoveState`).
    state: MoveState,
    /// The projections moved *out of* this binding while it stays live. The
    /// binding is still freed as a whole, so each of these has to be cleared
    /// at its move; until then the projection is a use of a moved value.
    /// Ordered, not hashed: a diagnostic that names one of these has to name
    /// the same one on every run.
    moved: std::collections::BTreeSet<Projection>,
}

/// One scope-durable borrow: the owner root, the binding that holds it (or
/// `None` for a temporary a statement produced), whether it is exclusive, and
/// the statement after which the borrow is dead.
///
/// `holder` is what lets a reassignment of a reference binding release exactly
/// the borrows that binding held, and it is what lets a statement release only
/// its own temporaries (`release_unheld_since`) without also releasing the
/// borrow an assignment stored. `usize::MAX` is the old lexical answer: live to
/// the end of the scope that declared it.
#[derive(Clone)]
struct ScopeBorrow {
    root: String,
    holder: Option<String>,
    mutable: bool,
    release_after: usize,
}

/// The roots a reference binding's value may borrow from, normalized: sorted,
/// deduplicated, and with a `str` literal's static storage dropped as soon as
/// a real owner is in the set.
fn normalize_roots(mut roots: Vec<String>) -> Vec<String> {
    roots.sort();
    roots.dedup();
    if roots.iter().any(|r| r != STATIC_ROOT) {
        roots.retain(|r| r != STATIC_ROOT);
    }
    roots
}

/// The union of two branches' root sets, the answer a branch merge takes.
fn union_roots(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    out.extend_from_slice(b);
    normalize_roots(out)
}

/// The union of two branches' borrow-entry lists, keyed by (holder, root): a
/// binding one arm points at A and the other at B ends up keeping both roots.
fn union_borrows(a: &[ScopeBorrow], b: &[ScopeBorrow]) -> Vec<ScopeBorrow> {
    let mut out: Vec<ScopeBorrow> = Vec::with_capacity(a.len() + b.len());
    for entry in a.iter().chain(b.iter()) {
        match out
            .iter_mut()
            .find(|o| o.root == entry.root && o.holder == entry.holder)
        {
            Some(existing) => {
                existing.mutable |= entry.mutable;
                existing.release_after = existing.release_after.max(entry.release_after);
            }
            None => out.push(entry.clone()),
        }
    }
    out
}

#[derive(Clone, Default)]
struct Scope {
    vars: HashMap<String, VarState>,
    /// Roots borrowed by a `val`/`var` declared in this scope, each with the
    /// statement its borrowing binding is last used at (see `plan/nll.md`).
    scope_borrows: Vec<ScopeBorrow>,
    /// Owned (heap-owning) locals declared in this scope, in declaration
    /// order — the candidates for this block's end-of-scope drops.
    owned: Vec<OwnedVar>,
}

impl Scope {
    fn merge(a: Scope, b: Scope) -> Scope {
        // Scope-durable borrows recorded before the branch survive it, and the
        // two arms' entries are unioned: a binding the arms point at different
        // roots keeps every root, so every owner it may still point at stays
        // pinned. `merge_into` recomputes the pins from this list.
        let scope_borrows = union_borrows(&a.scope_borrows, &b.scope_borrows);
        let mut vars = HashMap::new();
        for (name, va) in a.vars {
            let moved_at = va.moved_at.or_else(|| b.vars.get(&name).and_then(|vb| vb.moved_at));
            let roots = match b.vars.get(&name) {
                Some(vb) => union_roots(&va.roots, &vb.roots),
                None => va.roots.clone(),
            };
            vars.insert(
                name,
                VarState {
                    ty: va.ty.clone(),
                    moved_at,
                    // Identical on both sides: a reservation is active before
                    // the branch and no arm releases it, so there is nothing to
                    // merge.
                    reserved: va.reserved,
                    roots,
                },
            );
        }
        // Owned locals declared in *this* scope survive the merge (an `if`
        // arm or loop body is always its own block, so nothing here was
        // declared inside one). Two paths that disagree about whether the
        // value was moved leave it `MaybeMoved`: it is *still* dropped, and
        // the path that moved it cleared the slot, which is what makes that
        // drop correct on both paths. `MoveState::merge` has the reasoning.
        let mut owned = a.owned;
        for entry in owned.iter_mut() {
            // The same *binding*, not a matching spelling: two `var a`
            // declarations in one scope are two bindings with two states, and
            // pairing them by name would merge the state of one into the
            // other.
            if let Some(other) = b.owned.iter().find(|o| o.binding == entry.binding) {
                entry.state = entry.state.merge(other.state);
                // A field one path moved out is gone from the merged place:
                // whichever path ran, the read of it is a use of a moved
                // value, and the drop still runs on both (the moving path
                // cleared the slot). Same conservatism as `MoveState`.
                for proj in &other.moved {
                    entry.moved.insert(proj.clone());
                }
            }
        }
        Scope { vars, scope_borrows, owned }
    }
}

struct Ctx {
    scopes: Vec<Scope>,
    plan: FnDrops,
    /// Which `datac`s own heap memory (transitively) — `Ty::Struct(_)`
    /// cannot answer that on its own, so the struct table decision is made
    /// once in `check_program` and threaded here.
    owned_structs: std::collections::HashSet<String>,
    /// Which user types declare a destructor (kernel stage 9, MK9-S6-M1), so a
    /// partial move of one can be refused before it zeroes a slot the base's
    /// own drop would run the destructor on again.
    drop_types: std::collections::HashSet<String>,
    /// For every callee whose result is a borrow: the parameter index the
    /// elided lifetime binds to (M6.3). Both `.kp` functions and builtins
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
    /// The expressions that move an owned binding's value out of it, keyed by
    /// the binding they move it out of, and in the order they were seen. The
    /// binding is the key, not a spelling: a block may hold two `var a`
    /// declarations, each with its own slot to clear (BUG-n7Wq3). These become
    /// `FnDrops::at_move` in `check_function`.
    move_sites: HashMap<BindingId, Vec<NodeId>>,
    /// The expressions that move one field or payload of an owned binding out,
    /// and the projection each one emptied. Unlike `move_sites` this needs no
    /// filter when it becomes the plan's `at_clear`: the binding is dropped on
    /// every path, so every one of these clears a slot a drop will reach.
    field_move_sites: HashMap<NodeId, Projection>,
    /// Every `enum` declaration and instantiation by name, for the one thing a
    /// partial move of a payload needs from it: the payload field's own name,
    /// which the diagnostic and the plan's projection both carry while the
    /// `tir` node carries only the two indices.
    enums: HashMap<String, EnumDef>,
    /// The bindings that some merge left `MaybeMoved`. Only those have a drop
    /// site a move could double up with, so only their move sites are recorded
    /// as clears.
    maybe_moved: std::collections::HashSet<BindingId>,
    /// The lists a `for` in this frame is currently walking (`plan/syntax.md`
    /// 6.2), innermost last. A mutation of one is refused while its body runs;
    /// a written `while` over an index pushes nothing, which is the supported
    /// way to mutate a list a loop walks.
    iterating: Vec<tir::IteratedPlace>,
    /// Which owner root is currently borrowed, and how, for the whole frame.
    /// Frame-wide rather than on the root's `VarState` because a retarget
    /// inside an inner block may reach a root declared in an outer one, and
    /// because a branch merge recomputes it from the merged borrow entries.
    /// See `plan/provenance.md`.
    pins: HashMap<String, BorrowKind>,
}

impl Ctx {
    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pin_of(&self, root: &str) -> BorrowKind {
        self.pins.get(root).cloned().unwrap_or(BorrowKind::None)
    }

    fn set_pin(&mut self, root: &str, kind: BorrowKind) {
        if kind == BorrowKind::None {
            self.pins.remove(root);
        } else {
            self.pins.insert(root.to_string(), kind);
        }
    }

    /// Adds one pin without a conflict check: shared counts up, exclusive wins.
    fn add_pin(&mut self, root: &str, mutable: bool) {
        let kind = match (self.pin_of(root), mutable) {
            (_, true) => BorrowKind::Exclusive,
            (BorrowKind::Exclusive, false) => BorrowKind::Exclusive,
            (BorrowKind::Shared(n), false) => BorrowKind::Shared(n + 1),
            (BorrowKind::None, false) => BorrowKind::Shared(1),
        };
        self.set_pin(root, kind);
    }

    /// Rebuilds the frame's pins from the scope-durable entries after a branch
    /// merge, where one arm's pins and the other's were unioned into one list.
    fn recompute_pins(&mut self) {
        let entries: Vec<(String, bool)> = self
            .scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .scope_borrows
                    .iter()
                    .map(|e| (e.root.clone(), e.mutable))
            })
            .collect();
        self.pins.clear();
        for (root, mutable) in entries {
            self.add_pin(&root, mutable);
        }
    }

    /// Ends a block: releases its scope-durable borrows and records the drops
    /// for the owned locals it declared that are still live here.
    fn pop_scope(&mut self, block: &tir::Block, unreachable_end: bool) {
        let popped = self.scopes.pop().expect("pop_scope without a matching push_scope");
        for entry in popped.scope_borrows {
            self.end_borrow(&entry.root);
        }
        // A block that ends by returning never reaches its end, and the
        // return's own drops already cover everything live at that point.
        if unreachable_end {
            return;
        }
        let drops = live_drops(popped.owned.iter());
        if !drops.is_empty() {
            self.plan.at_block_end.insert(block_id(block), drops);
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

    /// Marks the innermost owned binding named `name` as moved out, and says
    /// which binding that was: only an owned local can need its slot cleared on
    /// the way out (see `mark_move_site`).
    fn mark_moved(&mut self, name: &str) -> Option<BindingId> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.state = MoveState::Moved;
                return Some(owned.binding);
            }
        }
        None
    }

    /// Records that the value read at `expr` is a move out of the owned binding
    /// `binding`: codegen clears that binding's slot there, which is what lets
    /// the binding still be dropped on the path that did not move it. A value
    /// that is not an owned local (a `pd` global's storage, a `str`, a borrow)
    /// is not recorded — nothing frees it, so nothing can double-free.
    fn mark_move_site(&mut self, binding: BindingId, expr: &tir::Expr) {
        let entry = self.move_sites.entry(binding).or_default();
        let id = expr_id(expr);
        if !entry.contains(&id) {
            entry.push(id);
        }
    }

    /// Marks the innermost owned binding named `name` as owning again (an
    /// assignment of a fresh value into it).
    fn mark_owned_again(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.state = MoveState::Live;
                // A fresh value into the binding replaces whatever projection
                // was moved out of the old one.
                owned.moved.clear();
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
                reserved: false,
                roots: Vec::new(),
            },
        );
    }

    /// Whether `name` is a binding whose value is a reference, so an
    /// assignment to it retargets provenance rather than replacing a value
    /// (`plan/provenance.md`).
    fn binds_reference(&self, name: &str) -> bool {
        self.lookup(name)
            .map(|state| is_reference_ty(&state.ty))
            .unwrap_or(false)
    }

    fn lookup(&self, name: &str) -> Option<&VarState> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.vars.get(name) {
                return Some(v);
            }
        }
        None
    }

    /// Records that `name` holds a value derived from the places in `roots`,
    /// when the binding is a reference. `borrow_root` follows these, and that
    /// is what lets a binding introduced by a pattern, or a cursor rebound from
    /// one owner to another, keep its provenance:
    ///
    /// ```kelp
    /// fun as_str(v: &Json).R: &str {
    ///     if let Str(s) = v {
    ///         return s.as_str()      // `s` *is* derived from `v`
    ///     }
    /// }
    /// ```
    ///
    /// `v.x.as_str()` — the same borrow written out — is already accepted, so
    /// refusing the pattern form was a missed edge and not a soundness rule:
    /// the payload binding is a shared borrow of `v`'s payload, and returning a
    /// view of it is exactly as safe as returning a view of a field of `v`.
    ///
    /// **Only reference-typed bindings are recorded.** A binding of an owned
    /// type is a *move*, and tracing a root through one would let a view of a
    /// local masquerade as a view of the parameter:
    ///
    /// ```kelp
    /// fun f(x: String).R: &str {
    ///     val y = x            // an owned move, not a borrow
    ///     return y.as_str()    // must stay refused: `y` dies with this frame
    /// }
    /// ```
    fn set_roots(&mut self, name: &str, roots: Vec<String>) {
        if let Some(state) = self.lookup_mut(name) {
            state.roots = normalize_roots(roots);
        }
    }

    /// The owner roots a place rooted at `name` may borrow from. A binding with
    /// recorded provenance resolves to that set; anything else is its own root,
    /// which is what a plain local, a parameter and a `pd` global are.
    fn roots_of(&self, name: &str) -> Vec<String> {
        for scope in self.scopes.iter().rev() {
            if let Some(state) = scope.vars.get(name) {
                if !state.roots.is_empty() {
                    return state.roots.clone();
                }
                break;
            }
        }
        vec![name.to_string()]
    }

    /// Registers an owned local so its drop point can be planned.
    ///
    /// One entry per *binding*, not per spelling: a block that declares `var a`
    /// twice owns two values, and the entry the first declaration left is what
    /// keeps that value's drop reachable (BUG-n7Wq3). `retain` therefore
    /// matches on the binding, which for the walk of a `while` body that
    /// reaches one declaration twice re-arms the entry it already has rather
    /// than plan the same free twice.
    fn define_owned(&mut self, binding: BindingId, name: &str, ty: &Ty) {
        let scope = self.scopes.last_mut().unwrap();
        scope.owned.retain(|o| o.binding != binding);
        scope.owned.push(OwnedVar {
            binding,
            name: name.to_string(),
            ty: ty.clone(),
            state: MoveState::Live,
            moved: std::collections::BTreeSet::new(),
        });
    }

    /// The binding `name` resolves to here: the innermost owned binding of that
    /// spelling, which is the one a read, a write or a move of the name reaches
    /// (and so the one `mark_moved` marks).
    fn owned_binding_of(&self, name: &str) -> Option<BindingId> {
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return Some(owned.binding);
            }
        }
        None
    }

    /// Whether `name` is an owned local this frame may give a field away from.
    /// A borrow, a pointer, a `pd` global's storage and a name `infer.rs`
    /// invented are all absent here, which is what makes this the same
    /// predicate the drop plan uses.
    fn is_owned_binding(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|s| s.owned.iter().any(|o| o.name == name))
    }

    /// Whether the projection `proj` was already moved out of `name`.
    fn moved_out_of(&self, name: &str, proj: &Projection) -> bool {
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.moved.contains(proj);
            }
        }
        false
    }

    /// The projection the whole-value read of `name` refuses, if any: a field
    /// or payload that was moved out and never put back. The set is ordered, so
    /// a binding with two projections gone always names the same one here.
    fn partial_move_of(&self, name: &str) -> Option<&Projection> {
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.moved.iter().next();
            }
        }
        None
    }

    /// The refusal a whole read of `name` owes when a projection of it was
    /// moved out: the slot that projection used to hold is empty, so the value
    /// as a whole is not here any more.
    fn whole_move_error(&self, name: &str, span: Span) -> Option<BorrowError> {
        let proj = self.partial_move_of(name)?;
        let what = match proj {
            Projection::Field(field) => format!("its field '{field}' was moved out"),
            Projection::EnumPayload { name, .. } => {
                format!("its payload '{name}' was moved out")
            }
        };
        Some(BorrowError {
            message: format!("cannot use {} as a whole value: {what}", display_root(name)),
            span,
        })
    }

    /// Reads a place as a whole (a borrow, a method receiver, a field's base),
    /// refusing one a field was moved out of. The projections of the place are
    /// still there; the place itself is not.
    fn use_whole_place(&self, name: &str, span: Span) -> Result<()> {
        match self.whole_move_error(name, span) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Records that the value read at `expr` moves the projection `proj` out of
    /// the owned binding `name`: codegen clears that slot there, and until the
    /// slot is written again the read is a use of a moved value.
    fn move_field_out(&mut self, name: &str, proj: Projection, expr: &tir::Expr) -> Result<()> {
        self.check_iterated_mutation(expr)?;
        if self.moved_out_of(name, &proj) {
            return Err(BorrowError {
                message: format!("use of moved value '{}'", field_display(name, &proj)),
                span: expr.span,
            });
        }
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.moved.insert(proj.clone());
                break;
            }
        }
        self.field_move_sites.insert(expr_id(expr), proj);
        Ok(())
    }

    /// Whether a place read reaches through the field `field` of an owned
    /// binding that a partial move already emptied.
    fn use_field_place(&self, inner: &tir::Expr, field: &str, span: Span) -> Result<()> {
        let Some(name) = partial_move_base(inner, self) else {
            return Ok(());
        };
        if self.moved_out_of(name, &Projection::Field(field.to_string())) {
            return Err(BorrowError {
                message: format!("use of moved value '{name}.{field}'"),
                span,
            });
        }
        Ok(())
    }

    /// The name of payload field `field` of variant `variant` of the enum
    /// `ty` names, for a diagnostic and for the plan's projection. `None` when
    /// the type is not an enum this program declares, which no payload read
    /// `infer.rs` produces is.
    fn enum_payload_name(&self, ty: &Ty, variant: usize, field: usize) -> Option<String> {
        let Ty::Struct(ename) = ty else {
            return None;
        };
        let def = self.enums.get(ename)?;
        let variant = def.variants.get(variant)?;
        Some(variant.fields.get(field)?.name.clone())
    }

    /// Marks the field `field` of an owned binding as owning again, after an
    /// assignment of a fresh value into it.
    fn reinit_field(&mut self, inner: &tir::Expr, field: &str) {
        let Some(name) = partial_move_base(inner, self) else {
            return;
        };
        let proj = Projection::Field(field.to_string());
        for scope in self.scopes.iter_mut().rev() {
            if let Some(owned) = scope.owned.iter_mut().rev().find(|o| o.name == name) {
                owned.moved.remove(&proj);
                return;
            }
        }
    }

    /// Whether the innermost owned binding named `name` has *definitely* been
    /// moved out. An assignment uses this to decide whether the old value is
    /// still there to free: a `MaybeMoved` binding's slot is still freed,
    /// because the path that moved it cleared it.
    fn owned_definitely_moved(&self, name: &str) -> bool {
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.state == MoveState::Moved;
            }
        }
        false
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
        // A binding a field was taken out of still owns the rest, but the
        // value as a whole is no longer here: the slot one of its fields used
        // to hold is empty, so handing the whole thing on would hand on that
        // hole. Same conservatism as a field read, and the same reason.
        if let Some(e) = self.whole_move_error(name, span) {
            return Err(e);
        }
        let Some(v) = self.lookup_mut(name) else {
            return Ok(()); // infer.rs already validated every identifier
        };
        // A `Copy` value is never moved, so neither check below applies: a
        // read of one is a copy, and copying a pointer while a borrow of it is
        // live is exactly what a borrow is for. Checking the borrow first made
        // the diagnostic a lie — it reported "cannot move 'self' while it is
        // borrowed" about a `&mut Parser`, which is not moved by the read at
        // all, and it did so for the ordinary shape of a receiver method that
        // names a field:
        //
        // ```kelp
        // fun mut Parser.keyword(word: &str).R: Result<i64, Err> {
        //     val text0 = text.as_str()      // borrows `self`
        //     ...
        //     return self.err("bad")         // reads `self`, which is Copy
        // }
        // ```
        if v.ty.is_copy() {
            return Ok(());
        }
        if v.moved_at.is_some() {
            return Err(BorrowError {
                message: format!("use of moved value {}", display_root(name)),
                span,
            });
        }
        let reserved = v.reserved;
        if self.pin_of(name) != BorrowKind::None || reserved {
            return Err(BorrowError {
                message: format!("cannot move {} while it is borrowed", display_root(name)),
                span,
            });
        }
        if let Some(v) = self.lookup_mut(name) {
            v.moved_at = Some(span);
        }
        Ok(())
    }

    /// Like `use_value`, but also tells the drop planner that the binding's
    /// value now lives somewhere else — and, when it does, records the read as
    /// a move site so codegen clears the binding's slot (see the module doc).
    fn use_value_moving(&mut self, name: &str, expr: &tir::Expr) -> Result<()> {
        let was_copy = self
            .lookup_mut(name)
            .map(|v| v.ty.is_copy())
            .unwrap_or(true);
        // Only a real move is a mutation; a `Copy` read (a parameter that is
        // already a borrow, a scalar) leaves the place alone.
        if !was_copy {
            self.check_iterated_mutation(expr)?;
        }
        self.use_value(name, expr.span)?;
        if !was_copy {
            if let Some(binding) = self.mark_moved(name) {
                self.mark_move_site(binding, expr);
            }
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
        if !self.knows(name) {
            return Ok(());
        }
        if self.pin_of(name) != BorrowKind::None
            || self.lookup(name).map(|v| v.reserved).unwrap_or(false)
        {
            return Err(BorrowError {
                message: format!("cannot assign to {} while it is borrowed", display_root(name)),
                span,
            });
        }
        if let Some(v) = self.lookup_mut(name) {
            v.moved_at = None;
        }
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
        let moved = self.lookup(root).map(|v| v.moved_at.is_some()).unwrap_or(false);
        if moved {
            return Err(BorrowError {
                message: format!("cannot borrow {}: value was moved", display_root(root)),
                span,
            });
        }
        // A reservation (a method call's receiver, taken before its own
        // arguments are checked) admits reads and shared borrows, but not a
        // second exclusive borrow: `xs.insert(0, xs.remove(0))` is still two
        // mutable borrows of one list. `commit` is what rejects a shared
        // borrow that is still live when the receiver's own borrow begins.
        let reserved = self.lookup(root).map(|v| v.reserved).unwrap_or(false);
        if reserved {
            if mutable {
                return Err(BorrowError {
                    message: format!(
                        "cannot borrow {} as exclusive: already borrowed exclusively",
                        display_root(root)
                    ),
                    span,
                });
            }
            self.add_pin(root, false);
            return Ok(());
        }
        let new = match (self.pin_of(root), mutable) {
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
        self.set_pin(root, new);
        Ok(())
    }

    fn end_borrow(&mut self, root: &str) {
        let new = match self.pin_of(root) {
            BorrowKind::Shared(n) if n > 1 => BorrowKind::Shared(n - 1),
            _ => BorrowKind::None,
        };
        self.set_pin(root, new);
    }

    /// Phase one of a two-phase borrow: the receiver of a method call is set
    /// aside before its own arguments are checked, so an argument may read the
    /// same place. Nothing is borrow-checked as exclusive yet (MISSING-n4Bz7).
    fn reserve_exclusive(&mut self, root: &str, span: Span) -> Result<()> {
        if !self.knows(root) {
            return Ok(());
        }
        if self.lookup(root).map(|v| v.moved_at.is_some()).unwrap_or(false) {
            return Err(BorrowError {
                message: format!("cannot borrow {}: value was moved", display_root(root)),
                span,
            });
        }
        if self.lookup(root).map(|v| v.reserved).unwrap_or(false) {
            return Err(BorrowError {
                message: format!("cannot borrow {}: already borrowed exclusively", display_root(root)),
                span,
            });
        }
        match self.pin_of(root) {
            BorrowKind::Exclusive => {
                return Err(BorrowError {
                    message: format!("cannot borrow {}: already borrowed exclusively", display_root(root)),
                    span,
                });
            }
            BorrowKind::Shared(_) => {
                return Err(BorrowError {
                    message: format!(
                        "cannot borrow {} as exclusive: already borrowed as shared",
                        display_root(root)
                    ),
                    span,
                });
            }
            BorrowKind::None => {}
        }
        if let Some(v) = self.lookup_mut(root) {
            v.reserved = true;
        }
        Ok(())
    }

    /// Refuses a mutation of `place` while a `for` in this frame is walking it
    /// (`plan/syntax.md` 6.2). The comparison is on the canonical path, so
    /// `for x in self.items { self.other.push(x) }` is legal and naming the
    /// iterated list is refused. The span is the `for` line's iterable, not the
    /// mutation, because the rule is about the loop.
    fn check_iterated_mutation(&self, place: &tir::Expr) -> Result<()> {
        let Some(path) = tir::place_path(place) else {
            return Ok(());
        };
        let Some(it) = self.iterating.iter().rev().find(|it| it.path == path) else {
            return Ok(());
        };
        Err(BorrowError {
            message: format!("cannot mutate '{path}' while iterating over it"),
            span: it.span,
        })
    }

    /// Phase two: the reservation becomes the exclusive borrow the call wants.
    /// A shared borrow started while the reservation was in flight and still
    /// live here is a real conflict (`f(&mut c, &c)`), and so is a move, which
    /// `use_value` already refused.
    fn commit_exclusive(&mut self, root: &str, span: Span) -> Result<()> {
        if let Some(v) = self.lookup_mut(root) {
            v.reserved = false;
        }
        if self.pin_of(root) != BorrowKind::None {
            return Err(BorrowError {
                message: format!(
                    "cannot borrow {} as exclusive: already borrowed as shared",
                    display_root(root)
                ),
                span,
            });
        }
        self.set_pin(root, BorrowKind::Exclusive);
        Ok(())
    }

    fn register_scope_borrow(&mut self, root: &str) {
        self.register_held_borrow(root, None, false, usize::MAX);
    }

    fn register_held_borrow(
        &mut self,
        root: &str,
        holder: Option<&str>,
        mutable: bool,
        release_after: usize,
    ) {
        self.scopes.last_mut().unwrap().scope_borrows.push(ScopeBorrow {
            root: root.to_string(),
            holder: holder.map(|s| s.to_string()),
            mutable,
            release_after,
        });
    }

    /// Records that the reference binding `name` holds the borrows of `roots`.
    /// The entries the initializer just created (untagged, from `at`) are
    /// tagged with `name` and given `last`, the binding's own last use in its
    /// block, as release point. A root the initializer did not borrow (an alias
    /// like `val q = p`, or a pattern binding) gets a fresh pin and entry here;
    /// that is what keeps `val q = p; ...; print(q)` from dropping the pin too
    /// early.
    fn hold(
        &mut self,
        name: &str,
        roots: &[String],
        mutable: bool,
        at: usize,
        last: usize,
        span: Span,
    ) -> Result<()> {
        let at = {
            let scope = self.scopes.last().unwrap();
            at.min(scope.scope_borrows.len())
        };
        let mut missing: Vec<String> = Vec::new();
        for root in roots {
            if root == STATIC_ROOT {
                continue;
            }
            let tagged = self.scopes.last_mut().unwrap().scope_borrows[at..]
                .iter_mut()
                .find(|e| e.holder.is_none() && e.root == *root);
            match tagged {
                Some(entry) => {
                    entry.holder = Some(name.to_string());
                    entry.release_after = last;
                    entry.mutable |= mutable;
                }
                None => missing.push(root.clone()),
            }
        }
        for root in missing {
            self.start_borrow(&root, mutable, span)?;
            self.register_held_borrow(&root, Some(name), mutable, last);
        }
        Ok(())
    }

    /// Releases every borrow `name` holds and returns the scope that held it
    /// and the release point it had, so a reassignment can carry both forward.
    /// `None` when the binding holds no borrow here.
    fn release_holder(&mut self, name: &str) -> Option<(usize, usize)> {
        let mut released: Vec<String> = Vec::new();
        let mut spot: Option<(usize, usize)> = None;
        for (index, scope) in self.scopes.iter_mut().enumerate() {
            scope.scope_borrows.retain(|entry| {
                if entry.holder.as_deref() == Some(name) {
                    released.push(entry.root.clone());
                    spot = Some(match spot {
                        None => (index, entry.release_after),
                        Some((i, r)) => (i.min(index), r.max(entry.release_after)),
                    });
                    false
                } else {
                    true
                }
            });
        }
        for root in released {
            self.end_borrow(&root);
        }
        spot
    }

    /// Moves the untagged borrow entries a statement just created (from `at` in
    /// the current scope) into `target`, the scope that declared the binding
    /// being reassigned, and tags them with `holder`. An entry the value created
    /// lives in the current scope, which for a retarget inside a loop body or
    /// an inner block would release the borrow when that block ends; the
    /// declaring scope is where the binding lives, so that is where its borrow
    /// must be recorded. `filter` limits adoption to the value's own roots.
    /// Returns the roots adopted.
    fn adopt_fresh_borrows(
        &mut self,
        target: usize,
        at: usize,
        holder: &str,
        release: usize,
        filter: Option<&[String]>,
    ) -> Vec<String> {
        let current = self.scopes.len() - 1;
        let at = at.min(self.scopes[current].scope_borrows.len());
        let mut adopted: Vec<ScopeBorrow> = Vec::new();
        {
            let scope = &mut self.scopes[current];
            let mut keep = Vec::with_capacity(scope.scope_borrows.len());
            for (index, entry) in scope.scope_borrows.drain(..).enumerate() {
                if index >= at
                    && entry.holder.is_none()
                    && filter.map_or(true, |roots| roots.contains(&entry.root))
                {
                    adopted.push(ScopeBorrow {
                        root: entry.root,
                        holder: Some(holder.to_string()),
                        mutable: entry.mutable,
                        release_after: release,
                    });
                } else {
                    keep.push(entry);
                }
            }
            scope.scope_borrows = keep;
        }
        let mut roots = Vec::new();
        for entry in adopted {
            roots.push(entry.root.clone());
            self.scopes[target].scope_borrows.push(entry);
        }
        roots
    }

    /// Pushes one borrow entry into `scope`, which is the scope that declared
    /// the holder (not necessarily the current one).
    fn push_entry(
        &mut self,
        scope: usize,
        root: &str,
        holder: Option<&str>,
        mutable: bool,
        release_after: usize,
    ) {
        self.scopes[scope].scope_borrows.push(ScopeBorrow {
            root: root.to_string(),
            holder: holder.map(|s| s.to_string()),
            mutable,
            release_after,
        });
    }

    /// A reference binding's assignment. The old value is overwritten, so the
    /// borrows it held end here and the new value's roots replace them. That is
    /// what lets a pointer local be a cursor: `cur = cur.next.get()` no longer
    /// looks like assigning to a borrowed place, and re-pointing from owner A
    /// to owner B makes A mutable again (`plan/provenance.md`).
    fn assign_reference(&mut self, name: &str, value: &tir::Expr, span: Span) -> Result<()> {
        // The binding itself, not its roots: `&cur` would pin `cur`.
        if self.pin_of(name) != BorrowKind::None {
            return Err(BorrowError {
                message: format!("cannot assign to {} while it is borrowed", display_root(name)),
                span,
            });
        }
        // The old borrow ends at the reassignment; the new one inherits the
        // release point (the binding's own last use), so the new root stays
        // pinned exactly as long as the old one would have.
        let held = self.release_holder(name);
        let target = held.map(|(scope, _)| scope).unwrap_or(self.scopes.len() - 1);
        let release = held.map(|(_, r)| r).unwrap_or(usize::MAX);
        // A cursor's own mutability, for the pin a value that registers no
        // entry needs (an alias like `q = p`, where nothing was called).
        let mutable = matches!(&value.ty, Ty::Ref { mutable: true, .. });
        let roots = match borrowed_node(value) {
            Some((m, inner)) => {
                if *m {
                    self.check_iterated_mutation(inner)?;
                }
                check_place(inner, self)?;
                let roots = place_roots(inner, self);
                for root in &roots {
                    if root == STATIC_ROOT {
                        continue;
                    }
                    self.start_borrow(root, *m, span)?;
                    self.push_entry(target, root, Some(name), *m, release);
                }
                roots
            }
            None => {
                let at = self.scopes.last().map(|s| s.scope_borrows.len()).unwrap_or(0);
                check_value(value, self)?;
                let mut roots = borrow_root(value, self).unwrap_or_default();
                let mut adopted =
                    self.adopt_fresh_borrows(target, at, name, release, Some(&roots));
                if adopted.is_empty() {
                    // `borrow_root` named nothing useful: adopt whatever the
                    // value registered rather than leave the cursor unpinned.
                    adopted = self.adopt_fresh_borrows(target, at, name, release, None);
                }
                // A root the value did not register (an alias `q = p`, or a
                // value produced without a call) needs its own pin and entry
                // here: the holder that had one was released just above, so
                // nothing else keeps the owner borrowed.
                for root in &roots {
                    if root == STATIC_ROOT || adopted.contains(root) {
                        continue;
                    }
                    self.start_borrow(root, mutable, span)?;
                    self.push_entry(target, root, Some(name), mutable, release);
                }
                roots.extend(adopted);
                normalize_roots(roots)
            }
        };
        if let Some(state) = self.lookup_mut(name) {
            state.moved_at = None;
        }
        self.set_roots(name, roots);
        Ok(())
    }

    /// Releases every scope-durable borrow in the current scope whose binding
    /// is dead after statement `index` (`plan/nll.md`).
    fn release_expired(&mut self, index: usize) {
        let Some(scope) = self.scopes.last_mut() else { return };
        let mut released: Vec<String> = Vec::new();
        let mut kept: Vec<ScopeBorrow> = Vec::new();
        for entry in scope.scope_borrows.drain(..) {
            if entry.release_after <= index {
                released.push(entry.root);
            } else {
                kept.push(entry);
            }
        }
        scope.scope_borrows = kept;
        for root in released {
            self.end_borrow(&root);
        }
    }

    /// Releases every scope-durable borrow recorded in the current scope from
    /// index `at` onwards. `at` is a length taken before a statement was
    /// checked, so this is "the borrows that statement took". A mark may be
    /// past the end by the time it is used (a branch merge rebuilds the scope
    /// and drops its borrow list), so it is clamped — releasing only what is
    /// still recorded is the honest answer.
    fn release_borrows_since(&mut self, at: usize) {
        let Some(scope) = self.scopes.last_mut() else {
            return;
        };
        let at = at.min(scope.scope_borrows.len());
        let released: Vec<ScopeBorrow> = scope.scope_borrows.split_off(at);
        for entry in released {
            self.end_borrow(&entry.root);
        }
    }

    /// Releases the *temporary* borrows a statement took: entries from `at`
    /// with no holder. An entry a cursor assignment adopted has a holder and
    /// survives, so the borrow that assignment stored is not dropped at the end
    /// of its own statement (`plan/provenance.md`).
    fn release_unheld_since(&mut self, at: usize) {
        let Some(scope) = self.scopes.last_mut() else {
            return;
        };
        let at = at.min(scope.scope_borrows.len());
        let mut released: Vec<String> = Vec::new();
        let mut kept: Vec<ScopeBorrow> = Vec::new();
        for (index, entry) in scope.scope_borrows.drain(..).enumerate() {
            if index >= at && entry.holder.is_none() {
                released.push(entry.root);
            } else {
                kept.push(entry);
            }
        }
        scope.scope_borrows = kept;
        for root in released {
            self.end_borrow(&root);
        }
    }

    /// The parameter index a callee's returned borrow is elided from, for a
    /// `.kp` function or a builtin method.
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
        // The value may have come from either of two branches, so its root is a
        // set. A `str` literal's static storage needs no owner and is dropped;
        // what is left must be exactly the elided parameter. `None` means the
        // value has no recognised root at all, which is refused.
        let Some(roots) = borrow_root(value, self) else {
            return Err(self.unrooted_return_error(value.span));
        };
        let mut owners: Vec<String> = roots
            .into_iter()
            .filter(|r| r != STATIC_ROOT)
            .collect();
        owners.sort();
        owners.dedup();
        if owners.is_empty() {
            return Ok(()); // a `str` literal: static storage, no owner to pin
        }
        match self.ret_borrow_param.clone() {
            Some(param) if owners.len() == 1 && owners[0] == param => Ok(()),
            Some(param) => Err(BorrowError {
                message: format!(
                    "cannot return a borrow of '{}': '{}' borrows from its first reference parameter '{param}', so the returned value must be derived from '{param}'",
                    owners[0],
                    self.ret_ty.display()
                ),
                span: value.span,
            }),
            None => Err(self.unrooted_return_error(value.span)),
        }
    }

    /// The refusal a borrowed return owes when its value is not derived from
    /// the elided parameter: named after the parameter when there is one.
    fn unrooted_return_error(&self, span: Span) -> BorrowError {
        BorrowError {
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
            span,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            scopes: self.scopes.clone(),
            pins: self.pins.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.scopes = snapshot.scopes;
        self.pins = snapshot.pins;
    }

    fn merge_into(&mut self, then_snap: Snapshot, else_snap: Snapshot) {
        let merged: Vec<Scope> = then_snap
            .scopes
            .into_iter()
            .zip(else_snap.scopes)
            .map(|(a, b)| Scope::merge(a, b))
            .collect();
        self.scopes = merged;
        // The two arms' borrow lists were unioned, so the pins have to be
        // rebuilt from them: a binding one arm points at A and the other at B
        // now keeps both roots (`plan/provenance.md`).
        self.recompute_pins();
        // A binding whose two paths disagree now has a *conditional* move, so
        // its move sites have to clear its slot even though it is still
        // dropped — see the module doc.
        for scope in &self.scopes {
            for owned in &scope.owned {
                if owned.state == MoveState::MaybeMoved {
                    self.maybe_moved.insert(owned.binding);
                }
            }
        }
    }
}

/// The scope stack and the frame-wide pins, captured together: a branch is
/// checked against one snapshot and merged from two.
#[derive(Clone)]
struct Snapshot {
    scopes: Vec<Scope>,
    pins: HashMap<String, BorrowKind>,
}

fn is_copy(ty: &Ty) -> bool {
    ty.is_copy()
}

/// Whether a binding of this type is a reference whose provenance a cursor
/// carries: a shared or exclusive borrow, a `View`, a `str` or a `&dyn`.
/// `str`/`Dyn` join the borrow types here because `borrow_root` and the
/// existing `val`/`var` tracking already treat them as borrowed values.
fn is_reference_ty(ty: &Ty) -> bool {
    crate::infer::is_borrowed_value(ty) || matches!(ty, Ty::Str | Ty::Dyn { .. })
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

/// One projection written the way a person reads it: `bag.xs`, or `o.text`
/// for a payload field of the enum `o` holds.
fn field_display(name: &str, proj: &Projection) -> String {
    match proj {
        Projection::Field(field) => format!("{name}.{field}"),
        Projection::EnumPayload { name: field, .. } => format!("{name}.{field}"),
    }
}

/// The owned binding a value read of `base.<field>` takes the field out of,
/// when `base` is a binding this frame owns *directly*. A borrow, a pointer, a
/// `pd` global's storage, and a nested place all fail here: the frame has
/// nothing to give away from any of them. A nested place is left out because a
/// projection path, not a single field name, is what tracking one would need.
/// An `enum` payload read takes the same base: `infer.rs` reads a payload by
/// value only when the scrutinee is an owned local, which this is.
fn partial_move_base<'a>(base: &'a tir::Expr, ctx: &Ctx) -> Option<&'a str> {
    let tir::ExprKind::Ident(name) = &base.kind else {
        return None;
    };
    if matches!(base.ty, Ty::Ref { .. }) {
        return None;
    }
    ctx.is_owned_binding(name).then_some(name.as_str())
}

/// The drops for a set of owned locals: reverse declaration order, live ones
/// only. Reverse order matches the lexical nesting of the bindings — a value
/// moved out of an earlier binding into a later one is freed exactly once,
/// by the later one, and before the earlier one would have been. "Live"
/// includes a `MaybeMoved` binding: its slot holds either the value or the
/// zero a move left there, and freeing the zero is what makes the drop correct
/// on both paths (the module doc's "a move clears its source").
fn live_drops<'a>(owned: impl DoubleEndedIterator<Item = &'a OwnedVar>) -> Vec<DropTarget> {
    owned
        .rev()
        .filter(|o| o.state != MoveState::Moved)
        .map(|o| DropTarget {
            binding: Some(o.binding),
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
        tir::ExprKind::EnumField { base, .. } => tir_root_ident(base),
        tir::ExprKind::Deref { expr: inner } => tir_root_ident(inner),
        // A `pd` read in `main`/an initializer is `Deref(Ref(global))` — the
        // explicit borrow of the generated global — so the chain has to
        // descend through that `Ref` for the borrow to be tracked at all
        // (M6.4).
        tir::ExprKind::Ref { expr: inner, .. } => tir_root_ident(inner),
        // A `&dyn` coercion wraps the `&x` it was written from (A-12 slice 4),
        // so the same chain continues through it.
        tir::ExprKind::DynCoerce { expr: inner, .. } => tir_root_ident(inner),
        _ => None,
    }
}

/// The `&expr`/`&mut expr` a node is a borrow of, looking through the `&dyn`
/// coercion (A-12 slice 4) that wraps one: a `&dyn` value *is* the borrow it
/// was written from, so every borrow rule applies to the node inside it.
fn borrowed_node(expr: &tir::Expr) -> Option<(&bool, &tir::Expr)> {
    match &expr.kind {
        tir::ExprKind::Ref { mutable, expr: inner } => Some((mutable, inner)),
        tir::ExprKind::DynCoerce { expr: inner, .. } => borrowed_node(inner),
        _ => None,
    }
}

/// The root of a call argument, which is usually an auto-borrow node that
/// `check_call_arg` synthesized — so this unwraps one `Ref` before asking
/// `tir_root_ident`.
/// The owner roots of a call argument, which is usually an auto-borrow node
/// that `check_call_arg` synthesized — so this unwraps one `Ref` before asking
/// for the argument's place roots.
fn arg_roots(expr: &tir::Expr, ctx: &Ctx) -> Vec<String> {
    match &expr.kind {
        tir::ExprKind::Ref { expr: inner, .. } => place_roots(inner, ctx),
        _ => place_roots(expr, ctx),
    }
}

/// Stands in for a `str` literal's storage: static, so it satisfies any
/// return lifetime and never pins an owner.
const STATIC_ROOT: &str = "<static>";

/// The owner roots a borrow is ultimately derived from. A borrow-returning call
/// chains back through its own elided lifetime, so `trim_start(s)` roots at
/// `s`, and `f(s.as_str())` roots at whatever `s` was borrowed from. `None`
/// when the value has no such root.
fn borrow_root(expr: &tir::Expr, ctx: &Ctx) -> Option<Vec<String>> {
    match &expr.kind {
        tir::ExprKind::StringLit(_) => Some(vec![STATIC_ROOT.to_string()]),
        // A binding that *is* a reference carries the roots it was made from,
        // so a cursor resolves back through it to the place its value borrows
        // from, and a pattern binding keeps the place its payload came from
        // (`set_roots` has the reasoning).
        tir::ExprKind::Ident(name) => Some(ctx.roots_of(name)),
        tir::ExprKind::FieldAccess { base, .. }
        | tir::ExprKind::EnumField { base, .. }
        | tir::ExprKind::Ref { expr: base, .. }
        | tir::ExprKind::DynCoerce { expr: base, .. }
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
        // `fun f(v: &J).R: &str = when v { S(s) -> s.as_str()  N -> "n" }`
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

fn block_tail_root(block: &tir::Block, ctx: &Ctx) -> Option<Vec<String>> {
    borrow_root(block.tail.as_deref()?, ctx)
}

/// The owner roots a place expression borrows from: the binding at the bottom
/// of the chain, resolved through that binding's own provenance. This is what
/// makes a borrow made through a cursor a borrow of the *owner* rather than of
/// the cursor (`plan/provenance.md`).
fn place_roots(expr: &tir::Expr, ctx: &Ctx) -> Vec<String> {
    match tir_root_ident(expr) {
        Some(name) => ctx.roots_of(name),
        None => Vec::new(),
    }
}

/// The one root a value has when it may come from either of two branches: the
/// branches have to agree, **except** that a `str` literal's storage is static
/// (`STATIC_ROOT`), so a branch whose value is a literal needs no owner pinned
/// and does not conflict with a branch that borrows one.
///
/// ```kelp
/// fun f(v: &J).R: &str = when v {
///     S(s) -> s.as_str()   // borrows `v`
///     N -> "n"             // static: needs no owner
/// }
/// ```
///
/// One owner plus a literal is a value with a single owner. Two different
/// non-static roots are both kept: the joined value may have come from either
/// owner, so both stay pinned (`plan/provenance.md`).
fn merge_root_sets(left: &[String], right: &[String]) -> Vec<String> {
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
    for f in &program.functions {
        if crate::infer::is_borrowed_value(&f.ret) {
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
        let function = check_function(f, &owned, &program.drop_types, &fn_borrow_root, &globals, &enums).map_err(|e| {
            Diagnostic::new(e.message, e.span, f.module.clone())
        })?;
        plan.functions.insert(f.name.clone(), function);
    }
    Ok(plan)
}

fn check_function(
    f: &tir::Function,
    owned_structs: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
    fn_borrow_root: &HashMap<String, Option<usize>>,
    globals: &[(String, Ty)],
    enums: &HashMap<String, EnumDef>,
) -> Result<FnDrops> {
    let mut ctx = Ctx {
        scopes: vec![Scope::default()],
        plan: FnDrops::default(),
        owned_structs: owned_structs.clone(),
        drop_types: drop_types.clone(),
        fn_borrow_root: fn_borrow_root.clone(),
        // A borrowed return type borrows from the first reference
        // parameter — `infer.rs` has already rejected a function that
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
    // the same elision rule an explicit `return` is — unless it diverges, in
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
    // binding is still in a drop set — at a block end, a `return`, a `?` or an
    // assignment — and the free has to find nothing on the path that moved.
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
    // A partial move always clears: the binding it came out of stays in every
    // drop set it was already in, so each of these clears a slot a drop reaches.
    ctx.plan.at_clear = ctx.field_move_sites.clone();
    Ok(ctx.plan)
}

fn check_block(block: &tir::Block, ctx: &mut Ctx) -> Result<()> {
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

fn check_stmt(block: &tir::Block, index: usize, stmt: &tir::Stmt, ctx: &mut Ctx) -> Result<()> {
    match stmt {
        tir::Stmt::Let { name, ty, init } => {
            // A borrow taken *inside* the initializer is dead once the value is
            // bound — unless the bound value **is** the borrow. `val back =
            // str.parse_f64(cand.as_str())` borrows `cand` for the length of
            // one call and keeps nothing of it (`Result<f64, _>` owns all its
            // data), so the borrow must end with the statement; holding it to
            // the end of the block is what made `return cand` a few lines
            // later fail with "cannot move 'cand' while it is borrowed".
            //
            // `val v = l.get(0)`, by contrast, binds the borrow itself, and
            // the caller has to keep the owner pinned for as long as `v`
            // lives — so the distinction is the *binding's* type, not the
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
            if crate::types::needs_drop(ty, &ctx.owned_structs) {
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
        tir::Stmt::Expr(e) => {
            // A borrow produced by a statement's *temporary* — `print(l.get(0))`,
            // where the view is consumed inside the statement — is released at
            // the end of that statement, not at the end of the block. Scope
            // lifetimes are otherwise lexical (plan's known limitation: no
            // NLL), and holding every temporary to the end of the block makes
            // ordinary list code fail (`print(l.get(0)); l.push(x)` would
            // conflict with a view that is already dead). A *bound* borrow
            // (`val p = l.get(0)`) still lives as long as its binding, because
            // a `Let` is not this arm — it is the arm above, which applies the
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
/// loop rule and the branch rule together (`plan/nll.md`): a borrow used in a
/// loop body stays live for the whole loop, including the statements before
/// the use, because the next iteration reaches them.
fn last_use_index(block: &tir::Block, from: usize, name: &str) -> usize {
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

fn stmt_mentions(stmt: &tir::Stmt, name: &str) -> bool {
    match stmt {
        tir::Stmt::Let { init, .. } => expr_mentions(init, name),
        tir::Stmt::While { cond, body, .. } => {
            expr_mentions(cond, name) || block_mentions(body, name)
        }
        tir::Stmt::Return { value, .. } => {
            value.as_ref().map_or(false, |v| expr_mentions(v, name))
        }
        tir::Stmt::Expr(e) => expr_mentions(e, name),
    }
}

fn block_mentions(block: &tir::Block, name: &str) -> bool {
    block.stmts.iter().any(|s| stmt_mentions(s, name))
        || block.tail.as_ref().map_or(false, |e| expr_mentions(e, name))
}

/// Whether the expression names the binding anywhere in it. Spelled out per
/// `ExprKind` rather than lowered, because the walk has to see through every
/// position a use can hide in: a nested block, a loop, a template piece, an
/// `asm` operand, a closure's capture list. Reading a binding is what extends
/// its borrow's life, and every read reaches this node somewhere.
fn expr_mentions(expr: &tir::Expr, name: &str) -> bool {
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
        | tir::ExprKind::CharLit(_)
        | tir::ExprKind::FloatLit(_)
        | tir::ExprKind::StringLit(_)
        | tir::ExprKind::BoolLit(_)
        | tir::ExprKind::StaticRef(_)
        | tir::ExprKind::FnRef(_)
        | tir::ExprKind::Capture { .. } => false,
    }
}

/// A `val`/`var` initializer gets scope-durable borrow tracking when it's
/// directly `&expr`/`&mut expr` (see the module doc); anything else is a
/// plain value read.
fn check_let_init(init: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    if let Some((mutable, inner)) = borrowed_node(init) {
        if *mutable {
            ctx.check_iterated_mutation(inner)?;
        }
        check_place(inner, ctx)?;
        // A borrow through a cursor is a borrow of the owner the cursor points
        // into, so the roots are resolved through the cursor's provenance
        // (`place_roots`) rather than keyed on the cursor itself.
        for root in place_roots(inner, ctx) {
            if root == STATIC_ROOT {
                continue;
            }
            ctx.start_borrow(&root, *mutable, init.span)?;
            ctx.register_scope_borrow(&root);
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
                let Some(root) = partial_move_base(base, ctx) else {
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
                        return Err(BorrowError { message, span: expr.span });
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
                    return Err(BorrowError { message, span: expr.span });
                };
                let proj = Projection::Field(field.clone());
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
        tir::ExprKind::IntLit(_) | tir::ExprKind::CharLit(_) | tir::ExprKind::FloatLit(_) | tir::ExprKind::StringLit(_) | tir::ExprKind::BoolLit(_) => Ok(()),
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
        // (`plan/scoped-blocks.md`): every capture is a `Copy` value or a
        // shared borrow, so nothing moves and nothing drops. The captured
        // borrows themselves are started by `take_call_arguments`, which is
        // the only place a scoped literal can be written.
        tir::ExprKind::ScopedClosureLit { .. } => Ok(()),
        // A plain function value widened into a closure (A-4 phase E): the
        // value being wrapped is read as a whole value — a `fun` is `Copy`, so
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
                    message: "cannot read a captured value out of the closure's environment by value: the environment is not a local this frame owns, so the read would give two owners of the same buffer; use it through a borrow (read a field of it, call a method on it, pass it to a parameter), or capture a `Copy` value instead".to_string(),
                    span: expr.span,
                });
            }
            Ok(())
        }
        tir::ExprKind::Call { .. } | tir::ExprKind::VariadicCall { .. } => check_call(expr, ctx),
        // A call through a function value (A-4): the callee is read as a
        // *place*, not as a whole value — a function value is `Copy`, and a
        // closure is called through a borrow of the binding that holds it, so
        // calling one must not move it — and the arguments follow exactly the
        // rule a named call's do.
        tir::ExprKind::IndirectCall { callee, args } => {
            check_place(callee, ctx)?;
            let taken = take_call_arguments(args, ctx, None)?;
            release_call_borrows(&taken, ctx);
            Ok(())
        }
        // A method call through a `&dyn` (A-12 slice 4): the receiver is a
        // borrow (`Copy`), so reading it moves nothing; the arguments follow
        // exactly the rule a named call's do.
        tir::ExprKind::DynCall { receiver, args, .. } => {
            check_place(receiver, ctx)?;
            let taken = take_call_arguments(args, ctx, None)?;
            release_call_borrows(&taken, ctx);
            Ok(())
        }
        // `expr?` reads its operand as a whole value: the failure payload is
        // moved out onto the return path and the success payload is the
        // expression's value, so a named operand is moved exactly as a bare
        // read of it would be (MK5-S6-M1).
        tir::ExprKind::Try { expr: sub, .. } => {
            check_value(sub, ctx)?;
            // `?`'s failure arm leaves the function, so it has the same drop
            // obligation a written `return` has — and it needs its own plan
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
        tir::ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_value(v, ctx)?;
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
                    return Err(BorrowError { message, span: expr.span });
                };
                if crate::types::is_drop_type(&expr.ty, &ctx.drop_types) {
                    return Err(BorrowError {
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
            // (`plan/provenance.md`).
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
            // `p` is a borrow — `FieldAccess`'s base need not be a local this
            // frame owns) or to an owned field of a directly-owned local
            // (`h.name = s`) overwrites a place the drop plan cannot express
            // as a `BindingId` at all — there is no declaration to key it to,
            // only an address. `binding: None` here is exactly that: codegen
            // reads the target's own place expression again (its address is
            // idempotent to recompute — nothing between the two evaluations
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
            // the ones a branch merge left ambiguous — contrast `at_move`,
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
        tir::ExprKind::Ref { mutable, expr: inner } => {
            // A borrow reached generically here (not via a `val`/`var`
            // initializer, a call argument, or an assignment to a reference
            // binding — see `check_let_init`, `take_call_arguments` and
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
fn check_place(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
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
fn check_place_through(base: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
    match &base.kind {
        tir::ExprKind::Ident(name) => ctx.use_place(name, base.span),
        _ => check_place(base, ctx),
    }
}

fn check_assign_target(target: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
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
/// same-call aliasing violation like `f(&mut c, &c)` — not a separate
/// pass. All of them release together right after.
///
/// The one exception is a call that *returns* a borrow (M6.3): the argument
/// the elided lifetime binds to is left borrowed for the enclosing scope, so
/// the caller cannot move or free the owner while the view lives.
fn check_call(expr: &tir::Expr, ctx: &mut Ctx) -> Result<()> {
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
    let receiver = method_receiver_index(callee, args);
    if let Some(i) = receiver {
        if let tir::ExprKind::Ref { expr: inner, .. } = &args[i].kind {
            ctx.check_iterated_mutation(inner)?;
        }
        for root in arg_roots(&args[i], ctx) {
            ctx.reserve_exclusive(&root, args[i].span)?;
        }
    }
    let mut taken = take_call_arguments(args, ctx, receiver)?;
    if let Some(i) = receiver {
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
fn method_receiver_index(callee: &str, args: &[tir::Expr]) -> Option<usize> {
    if !callee.contains('#') {
        return None;
    }
    match args.first().map(|a| &a.kind) {
        Some(tir::ExprKind::Ref { mutable: true, .. }) => Some(0),
        _ => None,
    }
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
fn take_call_arguments(
    args: &[tir::Expr],
    ctx: &mut Ctx,
    receiver: Option<usize>,
) -> Result<Vec<String>> {
    let mut taken = Vec::new();
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
                if receiver == Some(i) {
                    taken.push(root);
                } else {
                    ctx.start_borrow(&root, *mutable, a.span)?;
                    taken.push(root);
                }
            }
        } else {
            // A scoped literal captures by borrow (`plan/scoped-blocks.md`),
            // and the borrow lives exactly as long as the call it was written
            // in, so its captured `Ref`s join the same `taken` list a direct
            // borrow argument does: a same-call aliasing `g(&mut s, { k ->
            // use(s) })` is refused here, and every borrow releases together
            // after the call.
            if let tir::ExprKind::ScopedClosureLit { captures, .. } = &a.kind {
                for c in captures {
                    if let tir::ExprKind::Ref { mutable, expr: inner } = &c.kind {
                        check_place(inner, ctx)?;
                        for root in place_roots(inner, ctx) {
                            if root == STATIC_ROOT {
                                continue;
                            }
                            ctx.start_borrow(&root, *mutable, c.span)?;
                            taken.push(root);
                        }
                    }
                }
            }
            check_value(a, ctx)?;
        }
    }
    Ok(taken)
}

fn release_call_borrows(taken: &[String], ctx: &mut Ctx) {
    for root in taken {
        ctx.end_borrow(root);
    }
}

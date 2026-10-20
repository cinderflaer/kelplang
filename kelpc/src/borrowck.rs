//! M3 (moves) + M4 (borrowing) checking over the typed IR.
//!
//! ## Moves (M3)
//!
//! Rule: primitives (and `&str`, which has no owned-string type to move -
//! it's just a borrowed pointer) are `Copy`; `datac` values are not. A bare
//! `Ident` read of a non-Copy value moves it out of its binding; a `.field`
//! read walks through a place chain without consuming anything, except the one
//! case A-11 added: a field read directly off an owned local is a partial move,
//! which takes that field out and clears the slot it held.
//!
//! ## Borrowing (M4)
//!
//! Per `design/syntax.md (kelp wiki)` §5, a `datac`-typed function parameter is an
//! implicit shared borrow unless `mut`-marked (exclusive) - `infer.rs`
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
//!   check catches a same-call aliasing violation (`f(&mut c, &c)`) - not
//!   a separate pass. They're released together right after the call.
//! - **Scope-durable**: a borrow that's the direct initializer of a
//!   `val`/`var` (`val p = &c`) lives until that binding's enclosing scope
//!   ends, mirroring the move-state scope stack.
//!
//! No surface syntax lets a function return a `Ty::Ref` (`infer.rs`'s
//! `resolve_type` never produces one for a `=> T` clause), so a borrow can
//! never escape the call frame that created it - that's what makes
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
//! returns that as a `DropPlan`  -  a side table keyed by *node identity*, so
//! the removed C backend can look up "which locals die here" as it writes each
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
//! merely shared its span - which is how an owned local declared in an `if
//! let` arm or a `for` body got a second, out-of-frame drop emitted on the
//! path where its name is not even declared (BUG-1). Both passes walk the very
//! same `tir::Program`, so the node's address is a stable and exact key.
//!
//! A local is dropped at the end of the block that declared it, if it is
//! still live there - moved out, and its new owner is responsible for it.
//! Every `return` first drops the frame's live owned locals (the value being
//! returned was itself checked before that, so a returned local is already
//! marked moved and is not in the set). Assignment to an owned `var` frees
//! the old value before the new one is stored (`DropPlan::at_assign`),
//! except for `b = b` (the value being stored *is* the old value) and
//! except when the value expression already moved the binding out
//! (`x = f(x)`, where `f` takes `x` by value) - there the free belongs to
//! whichever path still holds the old value, not to this assignment. The
//! same freeing now also covers an assignment through a dereferenced borrow
//! (`*p = value`) and to an owned field (`h.name = value`), which have no
//! binding to key a `DropTarget` to at all (roadmap open item 15, cases 2
//! and 3) - see `DropTarget::binding`'s doc.
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
//! reference type carries a root set: the owners its value may borrow from.
//! `cur = expr` REPLACES it, so the old roots' pins are released at the
//! assignment and the new ones are pinned to the binding's last use. A borrow
//! made through a cursor is therefore a borrow of the owner, which is why
//! `var cur = &list; cur = cur.next.get()` is accepted while mutating `list`
//! under a live cursor stays refused. `kelp wiki page language/ownership.md` is the rule and the
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

/// Whether `init` is `name.box()?`: a block box, which its region owns.
fn is_block_box_init(init: &tir::Expr) -> bool {
    matches!(&init.kind, tir::ExprKind::Try { expr, .. } if matches!(expr.kind, tir::ExprKind::BlockBox { .. }))
}

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
    /// `h.name = value` target (`FieldAccess`/`Deref` - roadmap open item 15,
    /// cases 2 and 3), where `name` is empty and there is no binding *or*
    /// global to fall back to at all - codegen has to recompute the target's
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
    /// A field reached through one or more embedded owned records. The path is
    /// rooted at the local binding and ends at the slot a partial move clears.
    NestedField(Vec<String>),
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
pub(crate) enum MoveState {
    /// Still owned by the binding: the scope that declared it frees it.
    Live,
    /// Moved out on every path that reaches here. A new owner holds the
    /// value, this binding is dead, and nothing frees it here.
    Moved,
    /// Moved out on some path and not on another - a branch or a loop body
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
/// Keyed by node identity rather than by position because the removed C backend walks
/// the same tree independently: a block identifies its own end-of-scope drops,
/// and a `return` statement identifies the drops that run before it returns.
/// See the module doc for why a source span cannot serve as that key.
#[derive(Debug, Default, Clone)]
pub struct DropPlan {
    pub(crate) functions: HashMap<String, FnDrops>,
}

/// The identity of one `tir` node, for the plan's key.
///
/// `borrowck.rs` and the removed C backend walk the same `tir::Program`, so the same
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

/// The identity of one `tir::Expr` node, for the move-site key - the same
/// identity argument as `block_id`, and for the same reason.
pub fn expr_id(expr: &tir::Expr) -> NodeId {
    expr as *const tir::Expr as NodeId
}

/// A manual block region the function declares (`block name: T { ... }`): what
/// its free is for. The region is keyed by the identity of its `Region`
/// expression (`expr_id`), which is what codegen asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionDrop {
    /// The name the programmer wrote after `block`.
    pub name: String,
    /// The element type the region's `name.box()` calls take.
    pub elem: Ty,
}

#[derive(Debug, Default, Clone)]
pub struct FnDrops {
    pub(crate) at_block_end: HashMap<NodeId, Vec<DropTarget>>,
    pub(crate) at_return: HashMap<Span, Vec<DropTarget>>,
    pub(crate) at_assign: HashMap<Span, Vec<DropTarget>>,
    pub(crate) at_try: HashMap<Span, Vec<DropTarget>>,
    /// The owned locals freed by the `break` at `span`, on the way out of the
    /// loop: only the locals declared *inside* that loop, in reverse
    /// declaration order. A `break` leaves the frame alive, so unlike a
    /// `return` it must not free the enclosing scope's locals.
    pub(crate) at_break: HashMap<Span, Vec<DropTarget>>,
    /// The same, for the `continue` at `span` (`kelp wiki page language/iterators.md`).
    pub(crate) at_continue: HashMap<Span, Vec<DropTarget>>,
    /// The expressions that *move* an owned binding's value out of it and
    /// therefore have to clear the binding's slot after copying the value out
    /// - one entry per moved expression, so codegen can ask about the node it
    /// is already rendering. Only bindings that some path leaves live get an
    /// entry: a binding moved on every path is dropped nowhere, so its slot's
    /// contents are never read again and clearing it would be dead code.
    pub(crate) at_move: std::collections::HashSet<NodeId>,
    /// Every expression that moves an owned binding's value out, including the
    /// ones on which the binding is moved on every path. A spawning program's
    /// frame drops a slot a panic finds still holding a value, so each of these
    /// clears its slot (decision 125). `at_move` is the subset codegen clears in
    /// every program.
    pub(crate) moved_any: std::collections::HashSet<NodeId>,
    /// The expressions that move one *projection* of an owned binding out (a
    /// `datac` field or an `enum` payload), and the projection each one
    /// emptied. A binding a projection was taken from stays live, so it is
    /// still in a drop set wherever it was before, and the projection's slot
    /// has to be zeroed at the move or that drop frees the value a second
    /// time. Unlike `at_move` this needs no filter: the binding is dropped on
    /// every path, not only on some.
    pub(crate) at_clear: HashMap<NodeId, Projection>,
    /// The manual block regions the function declares, by the identity of
    /// their `Region` expression. A region is recorded here whether or not any
    /// exit leaves it, so the table answers "is this a region" for codegen.
    pub(crate) regions: HashMap<NodeId, RegionDrop>,
}

impl DropPlan {
    /// Whether any function of the program declares a manual block region. A
    /// program that does links the collector, because a region's element is a
    /// collector block.
    pub fn has_regions(&self) -> bool {
        self.functions.values().any(|f| !f.regions.is_empty())
    }

    /// The manual block region `expr` is, when it is one. `function` is the
    /// function whose body holds `expr`.
    pub fn region(&self, function: &str, expr: &tir::Expr) -> Option<&RegionDrop> {
        self.functions
            .get(function)
            .and_then(|f| f.regions.get(&expr_id(expr)))
    }

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
    /// 15, cases 2 and 3) has `binding: None` and an empty `name` - there is
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
    /// to be freed first - the same set and the same order a written `return`
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

    /// The owned locals freed by the `break` at `span`: every local declared
    /// inside the loop being left, innermost first.
    pub fn at_break(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_break.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The owned locals freed by the `continue` at `span`.
    pub fn at_continue(&self, function: &str, span: Span) -> &[DropTarget] {
        self.functions
            .get(function)
            .and_then(|f| f.at_continue.get(&span))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Whether the value read at `expr` is a move out of an owned binding
    /// whose slot codegen has to clear - copy the value out, then zero the
    /// binding. See the module doc's "a move clears its source".
    pub fn moves_at(&self, function: &str, expr: &tir::Expr) -> bool {
        self.functions
            .get(function)
            .map(|f| f.at_move.contains(&expr_id(expr)))
            .unwrap_or(false)
    }

    /// Whether `expr` moves an owned binding's value out in `function`, on any
    /// path (decision 125). A superset of `moves_at`.
    pub fn moves_any(&self, function: &str, expr: &tir::Expr) -> bool {
        self.functions
            .get(function)
            .map(|f| f.moved_any.contains(&expr_id(expr)))
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
    /// The next step a reader can take, carried onto the `Diagnostic`.
    pub help: Option<String>,
}

type Result<T> = std::result::Result<T, BorrowError>;

#[derive(Clone, PartialEq)]
pub(crate) enum BorrowKind {
    None,
    Shared(u32),
    Exclusive,
}

#[derive(Clone)]
pub(crate) struct VarState {
    /// The binding's type: `Copy`-ness comes from here, and so does whether
    /// the old value needs freeing when an assignment overwrites it.
    pub(crate) ty: Ty,
    pub(crate) moved_at: Option<Span>,
    /// A pending exclusive borrow, taken for a method call's receiver and not
    /// yet committed (MISSING-n4Bz7). While it is set the place may still be
    /// *read* and shared-borrowed - that is what lets
    /// `counts.insert(k, counts.get_or(..) + 1)` type-check - but not moved
    /// or exclusively borrowed again, and committing turns it into
    /// `borrow = Exclusive`, which fails if a shared borrow taken meanwhile is
    /// still live (`f(&mut c, &c)`).
    pub(crate) reserved: Vec<String>,
    /// The owner roots this binding's value may borrow from, when the binding
    /// *is* a reference. The provenance a pattern binding would otherwise lose,
    /// and what makes a pointer local usable as a cursor (open item 33). Empty
    /// means no recorded provenance: the binding's own name is its root. See
    /// `set_roots` and `kelp wiki page language/ownership.md`.
    pub(crate) roots: Vec<String>,
}

/// An owned local declared in some scope, tracked for the drop plan.
#[derive(Clone)]
pub(crate) struct OwnedVar {
    /// Which binding this is: one entry per binding, never one per spelling,
    /// so a second declaration of a name does not take the first binding's
    /// drop away from it (see `define_owned`).
    pub(crate) binding: BindingId,
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// Whether this binding's value is still here; a definitely moved-out
    /// local is never dropped by the scope that declared it (see
    /// `MoveState`).
    pub(crate) state: MoveState,
    /// The projections moved *out of* this binding while it stays live. The
    /// binding is still freed as a whole, so each of these has to be cleared
    /// at its move; until then the projection is a use of a moved value.
    /// Ordered, not hashed: a diagnostic that names one of these has to name
    /// the same one on every run.
    pub(crate) moved: std::collections::BTreeSet<Projection>,
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
pub(crate) struct ScopeBorrow {
    pub(crate) root: String,
    pub(crate) holder: Option<String>,
    pub(crate) mutable: bool,
    pub(crate) release_after: usize,
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
pub(crate) struct Scope {
    pub(crate) vars: HashMap<String, VarState>,
    /// Roots borrowed by a `val`/`var` declared in this scope, each with the
    /// statement its borrowing binding is last used at (see `kelp wiki page language/ownership.md`).
    pub(crate) scope_borrows: Vec<ScopeBorrow>,
    /// Owned (heap-owning) locals declared in this scope, in declaration
    /// order - the candidates for this block's end-of-scope drops.
    pub(crate) owned: Vec<OwnedVar>,
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
                    reserved: va.reserved.clone(),
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

pub(crate) struct Ctx {
    pub(crate) scopes: Vec<Scope>,
    pub(crate) plan: FnDrops,
    /// Which `datac`s own heap memory (transitively) - `Ty::Struct(_)`
    /// cannot answer that on its own, so the struct table decision is made
    /// once in `check_program` and threaded here.
    pub(crate) owned_structs: std::collections::HashSet<String>,
    /// Which user types declare a destructor (kernel stage 9, MK9-S6-M1), so a
    /// partial move of one can be refused before it zeroes a slot the base's
    /// own drop would run the destructor on again.
    pub(crate) drop_types: std::collections::HashSet<String>,
    /// For every callee whose result is a borrow: the parameter index the
    /// elided lifetime binds to (M6.3). Both `.kp` functions and builtins
    /// follow the same rule, so both are in here.
    pub(crate) fn_borrow_root: HashMap<String, Option<usize>>,
    /// Whether each parameter of a user-defined callee is an exclusive
    /// reference. This preserves the callee-side mode for a forwarded `&mut`
    /// argument, whose expression type alone cannot distinguish a reborrow as
    /// shared from one as exclusive.
    pub(crate) fn_mut_params: HashMap<String, Vec<bool>>,
    /// Per scoped-block literal (by key), which field paths of each capture its
    /// body uses (`capture_footprint`).
    pub(crate) capture_footprints: HashMap<String, Vec<Option<Vec<String>>>>,
    /// The current function's own return-borrow rule: the name of the
    /// parameter a returned borrow must be rooted at, when the function
    /// declares a borrowed return type. `None` with `ret_is_borrow` set
    /// means the function takes no reference parameter at all, so only a
    /// `str` literal (static storage) is a legal returned borrow.
    pub(crate) ret_borrow_param: Option<String>,
    pub(crate) ret_is_borrow: bool,
    /// The declared return type, for the wording of that diagnostic.
    pub(crate) ret_ty: Ty,
    /// The expressions that move an owned binding's value out of it, keyed by
    /// the binding they move it out of, and in the order they were seen. The
    /// binding is the key, not a spelling: a block may hold two `var a`
    /// declarations, each with its own slot to clear (BUG-n7Wq3). These become
    /// `FnDrops::at_move` in `check_function`.
    pub(crate) move_sites: HashMap<BindingId, Vec<NodeId>>,
    /// The expressions that move one field or payload of an owned binding out,
    /// and the projection each one emptied. Unlike `move_sites` this needs no
    /// filter when it becomes the plan's `at_clear`: the binding is dropped on
    /// every path, so every one of these clears a slot a drop will reach.
    pub(crate) field_move_sites: HashMap<NodeId, Projection>,
    /// Every `enum` declaration and instantiation by name, for the one thing a
    /// partial move of a payload needs from it: the payload field's own name,
    /// which the diagnostic and the plan's projection both carry while the
    /// `tir` node carries only the two indices.
    pub(crate) enums: HashMap<String, EnumDef>,
    /// The bindings that some merge left `MaybeMoved`. Only those have a drop
    /// site a move could double up with, so only their move sites are recorded
    /// as clears.
    pub(crate) maybe_moved: std::collections::HashSet<BindingId>,
    /// The lists a `for` in this frame is currently walking (`design/syntax.md (kelp wiki)`
    /// 6.2), innermost last. A mutation of one is refused while its body runs;
    /// a written `while` over an index pushes nothing, which is the supported
    /// way to mutate a list a loop walks.
    pub(crate) iterating: Vec<tir::IteratedPlace>,
    /// For each loop currently being checked, the `scopes.len()` at its entry:
    /// a `break`/`continue` frees every owned local declared in the scopes at
    /// or above this mark, and nothing outside the loop (`kelp wiki page language/iterators.md`).
    pub(crate) loop_marks: Vec<usize>,
    /// Which owner root is currently borrowed, and how, for the whole frame.
    /// Frame-wide rather than on the root's `VarState` because a retarget
    /// inside an inner block may reach a root declared in an outer one, and
    /// because a branch merge recomputes it from the merged borrow entries.
    /// See `kelp wiki page language/ownership.md`.
    pub(crate) pins: HashMap<String, BorrowKind>,
}


/// The scope stack and the frame-wide pins, captured together: a branch is
/// checked against one snapshot and merged from two.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub(crate) scopes: Vec<Scope>,
    pub(crate) pins: HashMap<String, BorrowKind>,
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
        None => match name.strip_prefix("#cap") {
            // A captured binding of a scoped block has no name of its own here.
            Some(rest) => {
                let path = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                if path.is_empty() {
                    "'a captured value'".to_string()
                } else {
                    format!("'a captured value{}'", path.replace(PATH_SEP, "."))
                }
            }
            None => format!("'{}'", name.replace(PATH_SEP, ".")),
        },
    }
}

/// The separator between an owner and the fields of it a borrow is rooted at
/// (`s/items`). A character no name contains, so a module path's dots (a
/// passdown is `pd#std.rt.x`) are never read as one.
const PATH_SEP: char = '/';

/// The variable a root lives in: `s` for `s/items/head`.
fn var_of(root: &str) -> &str {
    root.split(PATH_SEP).next().unwrap_or(root)
}

/// Whether two roots can name overlapping storage: the same place, or one a
/// part of the other. Two fields of one owner neither contains the other.
fn roots_overlap(a: &str, b: &str) -> bool {
    a == b
        || (a.len() < b.len() && b.starts_with(a) && b.as_bytes()[a.len()] == PATH_SEP as u8)
        || (b.len() < a.len() && a.starts_with(b) && a.as_bytes()[b.len()] == PATH_SEP as u8)
}

/// One projection written the way a person reads it: `bag.xs`, or `o.text`
/// for a payload field of the enum `o` holds.
fn field_display(name: &str, proj: &Projection) -> String {
    match proj {
        Projection::Field(field) => format!("{name}.{field}"),
        Projection::NestedField(path) => format!("{name}.{}", path.join(".")),
        Projection::EnumPayload { name: field, .. } => format!("{name}.{field}"),
    }
}

/// The path a projection names, one entry per step. A payload field has no
/// name another projection could share, so it is keyed by the two indices its
/// `tir` node carries, behind a `#` no field name can start with: the same
/// payload moved twice then compares equal, and a payload never compares equal
/// to a `datac` field.
fn projection_field_path(proj: &Projection) -> Vec<String> {
    match proj {
        Projection::Field(field) => vec![field.clone()],
        Projection::NestedField(path) => path.clone(),
        Projection::EnumPayload { variant, field, .. } => vec![format!("#{variant}.{field}")],
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

/// The owned local and embedded-field path a partial move reaches. A path is
/// valid only when every link is an owned field of a local this frame owns;
/// dereferences, passdowns and borrowed bases never enter this representation.
fn partial_move_target<'a>(base: &'a tir::Expr, ctx: &Ctx) -> Option<(&'a str, Vec<String>)> {
    match &base.kind {
        tir::ExprKind::Ident(name) if !matches!(base.ty, Ty::Ref { .. }) && ctx.is_owned_binding(name) => {
            Some((name, Vec::new()))
        }
        tir::ExprKind::FieldAccess { base: inner, field } => {
            let (name, mut path) = partial_move_target(inner, ctx)?;
            path.push(field.clone());
            Some((name, path))
        }
        _ => None,
    }
}

/// The drops for a set of owned locals: reverse declaration order, live ones
/// only. Reverse order matches the lexical nesting of the bindings - a value
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
        // A `pd` read in `main`/an initializer is `Deref(Ref(global))` - the
        // explicit borrow of the generated global - so the chain has to
        // descend through that `Ref` for the borrow to be tracked at all
        // (M6.4).
        tir::ExprKind::Ref { expr: inner, .. } => tir_root_ident(inner),
        // A `&dyn` coercion wraps the `&x` it was written from (A-12 slice 4),
        // so the same chain continues through it. An owned trait object
        // (A-12 slice 7) and a `&dyn` borrowed back out of one are the box's
        // own place, so the chain continues through them too.
        tir::ExprKind::DynCoerce { expr: inner, .. }
        | tir::ExprKind::BoxDynCoerce { expr: inner, .. }
        | tir::ExprKind::BoxDynBorrow { expr: inner, .. }
        | tir::ExprKind::BoxDynIsEmpty { expr: inner } => tir_root_ident(inner),
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
        // `b.get()` on an owned trait object (A-12 slice 7) is a borrow of the
        // box, so a binding of it keeps the box borrowed for its own lifetime.
        tir::ExprKind::BoxDynBorrow { mutable, expr: inner } => Some((mutable, inner)),
        _ => None,
    }
}

/// The owner roots of a call argument, which is usually an auto-borrow node
/// that `check_call_arg` synthesized, so this unwraps one `Ref` before asking
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

mod walk;
pub(crate) use walk::*;

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
    matches!(
        block.stmts.last(),
        Some(tir::Stmt::Return { .. })
            | Some(tir::Stmt::Break { .. })
            | Some(tir::Stmt::Continue { .. })
    )
}

fn check_while(cond: &tir::Expr, body: &tir::Block, ctx: &mut Ctx) -> Result<()> {
    let before = ctx.snapshot();
    // Every scope the body opens sits at or above this mark; a `break` or
    // `continue` in it frees the owned locals those scopes declared.
    ctx.loop_marks.push(ctx.scopes.len());

    // First pass: one iteration.
    check_value(cond, ctx)?;
    check_block(body, ctx)?;

    // Second pass, starting from the state one iteration leaves behind:
    // catches a value that would be double-moved on a second iteration
    // (loops aren't guaranteed to run only once).
    check_value(cond, ctx)?;
    check_block(body, ctx)?;
    ctx.loop_marks.pop();

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

/// Every live owned local declared inside the innermost loop, innermost scope
/// first - what a `break`/`continue` there has to free (`kelp wiki page language/iterators.md`).
/// The mark is `scopes.len()` when `check_while` started, so the slice from it
/// holds the loop body's own scope and every scope nested inside it, and
/// nothing declared outside the loop.
fn live_drops_since_loop(ctx: &Ctx) -> Vec<DropTarget> {
    let Some(&mark) = ctx.loop_marks.last() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for scope in ctx.scopes[mark..].iter().rev() {
        out.extend(live_drops(scope.owned.iter()));
    }
    out
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

mod check;
pub(crate) use check::*;

/// The roots a scoped block's capture pins: the capture's own roots, narrowed
/// to the field paths the block's body uses when it only touches parts of it.
fn capture_roots(roots: Vec<String>, key: &str, index: usize, ctx: &Ctx) -> Vec<String> {
    let Some(Some(paths)) = ctx.capture_footprints.get(key).and_then(|v| v.get(index)) else {
        return roots;
    };
    roots
        .into_iter()
        .flat_map(|r| {
            if r == STATIC_ROOT || r.starts_with("pd#") {
                vec![r]
            } else {
                paths.iter().map(|p| format!("{r}{p}")).collect()
            }
        })
        .collect()
}

fn release_call_borrows(taken: &[String], ctx: &mut Ctx) {
    for root in taken {
        ctx.end_borrow(root);
    }
}

#[cfg(test)]
#[path = "borrowck/region_walk_tests.rs"]
mod region_walk_tests;

#[cfg(test)]
#[path = "borrowck/region_drop_tests.rs"]
mod region_drop_tests;

mod ctx_use;

mod ctx_borrow;

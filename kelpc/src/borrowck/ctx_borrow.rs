use super::*;

impl Ctx {
    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// The strongest borrow on `root` or on anything that overlaps it: the same
    /// place, a place it is a part of (`s` for `s/items`), or a part of it
    /// (`s/items` for `s`). Two field paths of one owner that neither contains
    /// the other (`s/items`, `s/count`) do not overlap. This is the question a
    /// conflict check asks; `pin_exact` is the count a borrow adds to and takes
    /// from.
    pub(crate) fn pin_of(&self, root: &str) -> BorrowKind {
        let mut found = BorrowKind::None;
        for (other, kind) in &self.pins {
            if !roots_overlap(root, other) {
                continue;
            }
            found = match (&found, kind) {
                (BorrowKind::Exclusive, _) | (_, BorrowKind::Exclusive) => BorrowKind::Exclusive,
                (BorrowKind::Shared(a), BorrowKind::Shared(b)) => BorrowKind::Shared(a + b),
                (BorrowKind::Shared(a), BorrowKind::None) => BorrowKind::Shared(*a),
                (BorrowKind::None, k) => k.clone(),
            };
        }
        found
    }

    /// The borrow recorded on exactly `root`, for the counter a borrow starts
    /// and ends (`pin_of` answers conflicts, this keeps the books).
    pub(crate) fn pin_exact(&self, root: &str) -> BorrowKind {
        self.pins.get(root).cloned().unwrap_or(BorrowKind::None)
    }

    /// Whether a method call is holding a reservation on `root` or on anything
    /// that overlaps it.
    pub(crate) fn is_reserved(&self, root: &str) -> bool {
        self.lookup(var_of(root))
            .is_some_and(|v| v.reserved.iter().any(|r| roots_overlap(root, r)))
    }

    pub(crate) fn set_pin(&mut self, root: &str, kind: BorrowKind) {
        if kind == BorrowKind::None {
            self.pins.remove(root);
        } else {
            self.pins.insert(root.to_string(), kind);
        }
    }

    /// Adds one pin without a conflict check: shared counts up, exclusive wins.
    pub(crate) fn add_pin(&mut self, root: &str, mutable: bool) {
        let kind = match (self.pin_exact(root), mutable) {
            (_, true) => BorrowKind::Exclusive,
            (BorrowKind::Exclusive, false) => BorrowKind::Exclusive,
            (BorrowKind::Shared(n), false) => BorrowKind::Shared(n + 1),
            (BorrowKind::None, false) => BorrowKind::Shared(1),
        };
        self.set_pin(root, kind);
    }

    /// Rebuilds the frame's pins from the scope-durable entries after a branch
    /// merge, where one arm's pins and the other's were unioned into one list.
    pub(crate) fn recompute_pins(&mut self) {
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
    pub(crate) fn pop_scope(&mut self, block: &tir::Block, unreachable_end: bool) {
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

    /// Every live owned local in *every* enclosing scope, innermost first -
    /// what a `return` from here would have to free before leaving.
    pub(crate) fn live_drops_in_frame(&self) -> Vec<DropTarget> {
        let mut out = Vec::new();
        for scope in self.scopes.iter().rev() {
            out.extend(live_drops(scope.owned.iter()));
        }
        out
    }

    /// Marks the innermost owned binding named `name` as moved out, and says
    /// which binding that was: only an owned local can need its slot cleared on
    /// the way out (see `mark_move_site`).
    pub(crate) fn mark_moved(&mut self, name: &str) -> Option<BindingId> {
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
    /// is not recorded - nothing frees it, so nothing can double-free.
    pub(crate) fn mark_move_site(&mut self, binding: BindingId, expr: &tir::Expr) {
        let entry = self.move_sites.entry(binding).or_default();
        let id = expr_id(expr);
        if !entry.contains(&id) {
            entry.push(id);
        }
    }

    /// Marks the innermost owned binding named `name` as owning again (an
    /// assignment of a fresh value into it).
    pub(crate) fn mark_owned_again(&mut self, name: &str) {
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

    pub(crate) fn define(&mut self, name: &str, ty: &Ty) {
        self.scopes.last_mut().unwrap().vars.insert(
            name.to_string(),
            VarState {
                ty: ty.clone(),
                moved_at: None,
                reserved: Vec::new(),
                roots: Vec::new(),
            },
        );
    }

    /// Whether `name` is a binding whose value is a reference, so an
    /// assignment to it retargets provenance rather than replacing a value
    /// (`kelp wiki page language/ownership.md`).
    pub(crate) fn binds_reference(&self, name: &str) -> bool {
        self.lookup(name)
            .map(|state| is_reference_ty(&state.ty))
            .unwrap_or(false)
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<&VarState> {
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
    /// fun as_str(v: &Json) => &str {
    ///     if let Str(s) = v {
    ///         return s.as_str()      // `s` *is* derived from `v`
    ///     }
    /// }
    /// ```
    ///
    /// `v.x.as_str()` - the same borrow written out - is already accepted, so
    /// refusing the pattern form was a missed edge and not a soundness rule:
    /// the payload binding is a shared borrow of `v`'s payload, and returning a
    /// view of it is exactly as safe as returning a view of a field of `v`.
    ///
    /// **Only reference-typed bindings are recorded.** A binding of an owned
    /// type is a *move*, and tracing a root through one would let a view of a
    /// local masquerade as a view of the parameter:
    ///
    /// ```kelp
    /// fun f(x: String) => &str {
    ///     val y = x            // an owned move, not a borrow
    ///     return y.as_str()    // must stay refused: `y` dies with this frame
    /// }
    /// ```
    pub(crate) fn set_roots(&mut self, name: &str, roots: Vec<String>) {
        if let Some(state) = self.lookup_mut(name) {
            state.roots = normalize_roots(roots);
        }
    }

    /// The owner roots a place rooted at `name` may borrow from. A binding with
    /// recorded provenance resolves to that set; anything else is its own root,
    /// which is what a plain local, a parameter and a `pd` global are.
    pub(crate) fn roots_of(&self, name: &str) -> Vec<String> {
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
    pub(crate) fn define_owned(&mut self, binding: BindingId, name: &str, ty: &Ty) {
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
    pub(crate) fn owned_binding_of(&self, name: &str) -> Option<BindingId> {
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
    pub(crate) fn is_owned_binding(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|s| s.owned.iter().any(|o| o.name == name))
    }

    /// Whether `proj` is itself moved or contains a nested moved field. A
    /// whole read of `p.name` must be refused after `p.name.text` moved: the
    /// outer record would otherwise still own a slot whose inner drop would
    /// free the value handed to the new binding.
    pub(crate) fn moved_at_or_below(&self, name: &str, proj: &Projection) -> bool {
        let requested = projection_field_path(proj);
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.moved.iter().any(|moved| {
                    let path = projection_field_path(moved);
                    !requested.is_empty() && path.starts_with(&requested)
                });
            }
        }
        false
    }

    /// Whether `proj` is itself moved or sits inside a moved projection. This
    /// is the inverse of `moved_at_or_below`: walking through `p.name` to read
    /// `p.name.id` remains valid after `p.name.text` moved, while walking
    /// through a field that itself moved does not.
    pub(crate) fn moved_at_or_above(&self, name: &str, proj: &Projection) -> bool {
        let requested = projection_field_path(proj);
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.moved.iter().any(|moved| {
                    let path = projection_field_path(moved);
                    !path.is_empty() && requested.starts_with(&path)
                });
            }
        }
        false
    }

    /// The projection the whole-value read of `name` refuses, if any: a field
    /// or payload that was moved out and never put back. The set is ordered, so
    /// a binding with two projections gone always names the same one here.
    pub(crate) fn partial_move_of(&self, name: &str) -> Option<&Projection> {
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
    pub(crate) fn whole_move_error(&self, name: &str, span: Span) -> Option<BorrowError> {
        let proj = self.partial_move_of(name)?;
        let what = match proj {
            Projection::Field(field) => format!("its field '{field}' was moved out"),
            Projection::NestedField(path) => format!("its field '{}' was moved out", path.join(".")),
            Projection::EnumPayload { name, .. } => {
                format!("its payload '{name}' was moved out")
            }
        };
        Some(BorrowError {
            help: None,
            message: format!("cannot use {} as a whole value: {what}", display_root(name)),
            span,
        })
    }

    /// Reads a place as a whole (a borrow, a method receiver, a field's base),
    /// refusing one a field was moved out of. The projections of the place are
    /// still there; the place itself is not.
    pub(crate) fn use_whole_place(&self, name: &str, span: Span) -> Result<()> {
        match self.whole_move_error(name, span) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Records that the value read at `expr` moves the projection `proj` out of
    /// the owned binding `name`: codegen clears that slot there, and until the
    /// slot is written again the read is a use of a moved value.
    pub(crate) fn move_field_out(&mut self, name: &str, proj: Projection, expr: &tir::Expr) -> Result<()> {
        self.check_iterated_mutation(expr)?;
        if self.moved_at_or_below(name, &proj) {
            return Err(BorrowError {
                help: Some(format!("pass &{} to keep the value here, or do not use it after the move.", field_display(name, &proj))),
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
    pub(crate) fn use_field_place(&self, inner: &tir::Expr, field: &str, span: Span) -> Result<()> {
        let Some((name, mut path)) = partial_move_target(inner, self) else {
            return Ok(());
        };
        path.push(field.to_string());
        let proj = if path.len() == 1 {
            Projection::Field(path.remove(0))
        } else {
            Projection::NestedField(path)
        };
        if self.moved_at_or_below(name, &proj) {
            return Err(BorrowError {
                help: Some(format!("pass &{name}.{field} to keep the value here, or do not use it after the move.")),
                message: format!("use of moved value '{name}.{field}'"),
                span,
            });
        }
        Ok(())
    }

    /// Checks a projection only as the base of a deeper place. A nested move
    /// leaves its siblings usable, so this accepts `p.name.id` after
    /// `p.name.text` moved, but it still rejects a path through the moved
    /// field itself.
    pub(crate) fn use_field_through(&self, inner: &tir::Expr, field: &str, span: Span) -> Result<()> {
        let Some((name, mut path)) = partial_move_target(inner, self) else {
            return Ok(());
        };
        path.push(field.to_string());
        let proj = if path.len() == 1 { Projection::Field(path.remove(0)) } else { Projection::NestedField(path) };
        if self.moved_at_or_above(name, &proj) {
            return Err(BorrowError {
                help: Some(format!("pass &{} to keep the value here, or do not use it after the move.", field_display(name, &proj))),
                message: format!("use of moved value '{}'", field_display(name, &proj)),
                span,
            });
        }
        Ok(())
    }

    /// The name of payload field `field` of variant `variant` of the enum
    /// `ty` names, for a diagnostic and for the plan's projection. `None` when
    /// the type is not an enum this program declares, which no payload read
    /// `infer.rs` produces is.
    pub(crate) fn enum_payload_name(&self, ty: &Ty, variant: usize, field: usize) -> Option<String> {
        let Ty::Struct(ename) = ty else {
            return None;
        };
        let def = self.enums.get(ename)?;
        let variant = def.variants.get(variant)?;
        Some(variant.fields.get(field)?.name.clone())
    }

    /// Marks the field `field` of an owned binding as owning again, after an
    /// assignment of a fresh value into it.
    pub(crate) fn reinit_field(&mut self, inner: &tir::Expr, field: &str) {
        let Some((name, mut path)) = partial_move_target(inner, self) else {
            return;
        };
        path.push(field.to_string());
        let proj = if path.len() == 1 { Projection::Field(path.remove(0)) } else { Projection::NestedField(path) };
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
    pub(crate) fn owned_definitely_moved(&self, name: &str) -> bool {
        for scope in self.scopes.iter().rev() {
            if let Some(owned) = scope.owned.iter().rev().find(|o| o.name == name) {
                return owned.state == MoveState::Moved;
            }
        }
        false
    }

    /// Whether any scope in the frame already binds `name`.
    pub(crate) fn knows(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|s| s.vars.contains_key(name))
    }

    pub(crate) fn lookup_mut(&mut self, name: &str) -> Option<&mut VarState> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(v) = scope.vars.get_mut(name) {
                return Some(v);
            }
        }
        None
    }
}

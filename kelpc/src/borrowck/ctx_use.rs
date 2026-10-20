use super::*;

impl Ctx {
    /// Errors if `name` was already moved or is currently borrowed;
    /// otherwise, if it's non-Copy, this whole-value read moves it.
    pub(crate) fn use_value(&mut self, name: &str, span: Span) -> Result<()> {
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
        // the diagnostic a lie - it reported "cannot move 'self' while it is
        // borrowed" about a `&mut Parser`, which is not moved by the read at
        // all, and it did so for the ordinary shape of a receiver method that
        // names a field:
        //
        // ```kelp
        // fun mut Parser.keyword(word: &str) => Result<i64, Err> {
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
                help: Some(format!("pass &{name} to keep the value here, or do not use it after the move.")),
                message: format!("use of moved value {}", display_root(name)),
                span,
            });
        }
        let reserved = !v.reserved.is_empty();
        if self.pin_of(name) != BorrowKind::None || reserved {
            return Err(BorrowError {
                help: None,
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
    /// value now lives somewhere else - and, when it does, records the read as
    /// a move site so codegen clears the binding's slot (see the module doc).
    pub(crate) fn use_value_moving(&mut self, name: &str, expr: &tir::Expr) -> Result<()> {
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
    pub(crate) fn use_place(&mut self, name: &str, span: Span) -> Result<()> {
        let Some(v) = self.lookup_mut(name) else {
            return Ok(());
        };
        if v.moved_at.is_some() {
            return Err(BorrowError {
                help: Some(format!("pass &{name} to keep the value here, or do not use it after the move.")),
                message: format!("use of moved value {}", display_root(name)),
                span,
            });
        }
        Ok(())
    }

    /// Assigning a fresh value into `name` reinitializes it (clears moved
    /// state), but only while nothing currently borrows it.
    pub(crate) fn reinit(&mut self, name: &str, span: Span) -> Result<()> {
        if !self.knows(name) {
            return Ok(());
        }
        if self.pin_of(name) != BorrowKind::None
            || self.lookup(name).map(|v| !v.reserved.is_empty()).unwrap_or(false)
        {
            return Err(BorrowError {
                help: None,
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
    /// freeing; the old one is leaked - see the module doc).
    pub(crate) fn reinit_moving(&mut self, name: &str, span: Span) -> Result<()> {
        self.reinit(name, span)?;
        self.mark_owned_again(name);
        Ok(())
    }

    pub(crate) fn start_borrow(&mut self, root: &str, mutable: bool, span: Span) -> Result<()> {
        let moved = self.lookup(var_of(root)).map(|v| v.moved_at.is_some()).unwrap_or(false);
        if moved {
            return Err(BorrowError {
                help: None,
                message: format!("cannot borrow {}: value was moved", display_root(root)),
                span,
            });
        }
        // A reservation (a method call's receiver, taken before its own
        // arguments are checked) admits reads and shared borrows, but not a
        // second exclusive borrow: `xs.insert(0, xs.remove(0))` is still two
        // mutable borrows of one list. `commit` is what rejects a shared
        // borrow that is still live when the receiver's own borrow begins.
        let reserved = self.is_reserved(root);
        if reserved {
            if mutable {
                return Err(BorrowError {
                    help: None,
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
        // The conflict is against everything that overlaps `root`; the count that
        // is kept is `root`'s own.
        let around = self.pin_of(root);
        let own = self.pin_exact(root);
        let new = match (&around, mutable) {
            (BorrowKind::Exclusive, _) => {
                return Err(BorrowError {
                    help: None,
                    message: format!(
                        "cannot borrow {}: already borrowed exclusively",
                        display_root(root)
                    ),
                    span,
                });
            }
            (BorrowKind::Shared(_), true) => {
                return Err(BorrowError {
                    help: None,
                    message: format!(
                        "cannot borrow {} as exclusive: already borrowed as shared",
                        display_root(root)
                    ),
                    span,
                });
            }
            (BorrowKind::Shared(_), false) => match own {
                BorrowKind::Shared(n) => BorrowKind::Shared(n + 1),
                _ => BorrowKind::Shared(1),
            },
            (BorrowKind::None, true) => BorrowKind::Exclusive,
            (BorrowKind::None, false) => BorrowKind::Shared(1),
        };
        self.set_pin(root, new);
        Ok(())
    }

    pub(crate) fn end_borrow(&mut self, root: &str) {
        let new = match self.pin_exact(root) {
            BorrowKind::Shared(n) if n > 1 => BorrowKind::Shared(n - 1),
            _ => BorrowKind::None,
        };
        self.set_pin(root, new);
    }

    /// Phase one of a two-phase borrow: the receiver of a method call is set
    /// aside before its own arguments are checked, so an argument may read the
    /// same place. Nothing is borrow-checked as exclusive yet (MISSING-n4Bz7).
    pub(crate) fn reserve_exclusive(&mut self, root: &str, span: Span) -> Result<()> {
        if !self.knows(var_of(root)) {
            return Ok(());
        }
        if self.lookup(var_of(root)).map(|v| v.moved_at.is_some()).unwrap_or(false) {
            return Err(BorrowError {
                help: None,
                message: format!("cannot borrow {}: value was moved", display_root(root)),
                span,
            });
        }
        if self.is_reserved(root) {
            return Err(BorrowError {
                help: None,
                message: format!("cannot borrow {}: already borrowed exclusively", display_root(root)),
                span,
            });
        }
        match self.pin_of(root) {
            BorrowKind::Exclusive => {
                return Err(BorrowError {
                    help: None,
                    message: format!("cannot borrow {}: already borrowed exclusively", display_root(root)),
                    span,
                });
            }
            BorrowKind::Shared(_) => {
                return Err(BorrowError {
                    help: None,
                    message: format!(
                        "cannot borrow {} as exclusive: already borrowed as shared",
                        display_root(root)
                    ),
                    span,
                });
            }
            BorrowKind::None => {}
        }
        if let Some(v) = self.lookup_mut(var_of(root)) {
            v.reserved.push(root.to_string());
        }
        Ok(())
    }

    /// Refuses a mutation of `place` while a `for` in this frame is walking it
    /// (`design/syntax.md (kelp wiki)` 6.2). The comparison is on the canonical path, so
    /// `for x in self.items { self.other.push(x) }` is legal and naming the
    /// iterated list is refused. The span is the `for` line's iterable, not the
    /// mutation, because the rule is about the loop.
    pub(crate) fn check_iterated_mutation(&self, place: &tir::Expr) -> Result<()> {
        let Some(path) = tir::place_path(place) else {
            return Ok(());
        };
        let Some(it) = self.iterating.iter().rev().find(|it| it.path == path) else {
            return Ok(());
        };
        Err(BorrowError {
            help: None,
            message: format!("cannot mutate '{path}' while iterating over it"),
            span: it.span,
        })
    }

    /// Phase two: the reservation becomes the exclusive borrow the call wants.
    /// A shared borrow started while the reservation was in flight and still
    /// live here is a real conflict (`f(&mut c, &c)`), and so is a move, which
    /// `use_value` already refused.
    pub(crate) fn commit_exclusive(&mut self, root: &str, span: Span) -> Result<()> {
        if let Some(v) = self.lookup_mut(var_of(root)) {
            if let Some(at) = v.reserved.iter().position(|r| r == root) {
                v.reserved.remove(at);
            }
        }
        if self.pin_of(root) != BorrowKind::None {
            return Err(BorrowError {
                help: None,
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

    pub(crate) fn register_scope_borrow(&mut self, root: &str) {
        self.register_held_borrow(root, None, false, usize::MAX);
    }

    pub(crate) fn register_held_borrow(
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
    pub(crate) fn hold(
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
    pub(crate) fn release_holder(&mut self, name: &str) -> Option<(usize, usize)> {
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

    pub(crate) fn holds_exclusive_roots(&self, name: &str, roots: &[String]) -> bool {
        roots.iter().all(|root| {
            root == STATIC_ROOT
                || self.scopes.iter().any(|scope| {
                    scope.scope_borrows.iter().any(|entry| {
                        entry.root == *root
                            && entry.holder.as_deref() == Some(name)
                            && entry.mutable
                    })
                })
        })
    }

    /// Moves the untagged borrow entries a statement just created (from `at` in
    /// the current scope) into `target`, the scope that declared the binding
    /// being reassigned, and tags them with `holder`. An entry the value created
    /// lives in the current scope, which for a retarget inside a loop body or
    /// an inner block would release the borrow when that block ends; the
    /// declaring scope is where the binding lives, so that is where its borrow
    /// must be recorded. `filter` limits adoption to the value's own roots.
    /// Returns the roots adopted.
    pub(crate) fn adopt_fresh_borrows(
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
    pub(crate) fn push_entry(
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
    /// to owner B makes A mutable again (`kelp wiki page language/ownership.md`).
    pub(crate) fn assign_reference(&mut self, name: &str, value: &tir::Expr, span: Span) -> Result<()> {
        // The binding itself, not its roots: `&cur` would pin `cur`.
        if self.pin_of(name) != BorrowKind::None {
            return Err(BorrowError {
                help: None,
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
    /// is dead after statement `index` (`kelp wiki page language/ownership.md`).
    pub(crate) fn release_expired(&mut self, index: usize) {
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
    /// and drops its borrow list), so it is clamped - releasing only what is
    /// still recorded is the honest answer.
    pub(crate) fn release_borrows_since(&mut self, at: usize) {
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
    /// of its own statement (`kelp wiki page language/ownership.md`).
    pub(crate) fn release_unheld_since(&mut self, at: usize) {
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
    pub(crate) fn borrow_root_param_of(&self, callee: &str) -> Option<usize> {
        match self.fn_borrow_root.get(callee).copied() {
            Some(v) => v,
            None => crate::infer::builtin_borrow_root(callee)?,
        }
    }

    pub(crate) fn callee_param_is_mut(&self, callee: &str, index: usize) -> bool {
        self.fn_mut_params
            .get(callee)
            .and_then(|params| params.get(index))
            .copied()
            .unwrap_or(false)
    }

    /// The elided-borrow rule, enforced at every return site of a function
    /// that declares a borrowed return type: the value must be derived from
    /// the first reference parameter (or be a `str` literal, whose storage is
    /// static). This is what makes `&str` return elision sound without real
    /// lifetime inference - the caller keeps the owner borrowed for as long
    /// as the view lives (see `check_call`), and the callee cannot hand back
    /// anything that did not come from that same owner.
    pub(crate) fn check_return_borrow(&mut self, value: &tir::Expr) -> Result<()> {
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
            .map(|r| var_of(&r).to_string())
            .collect();
        owners.sort();
        owners.dedup();
        if owners.is_empty() {
            return Ok(()); // a `str` literal: static storage, no owner to pin
        }
        match self.ret_borrow_param.clone() {
            Some(param) if owners.len() == 1 && owners[0] == param => Ok(()),
            Some(param) => Err(BorrowError {
                help: None,
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
    pub(crate) fn unrooted_return_error(&self, span: Span) -> BorrowError {
        BorrowError {
            help: None,
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

    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            scopes: self.scopes.clone(),
            pins: self.pins.clone(),
        }
    }

    pub(crate) fn restore(&mut self, snapshot: Snapshot) {
        self.scopes = snapshot.scopes;
        self.pins = snapshot.pins;
    }

    pub(crate) fn merge_into(&mut self, then_snap: Snapshot, else_snap: Snapshot) {
        let merged: Vec<Scope> = then_snap
            .scopes
            .into_iter()
            .zip(else_snap.scopes)
            .map(|(a, b)| Scope::merge(a, b))
            .collect();
        self.scopes = merged;
        // The two arms' borrow lists were unioned, so the pins have to be
        // rebuilt from them: a binding one arm points at A and the other at B
        // now keeps both roots (`kelp wiki page language/ownership.md`).
        self.recompute_pins();
        // A binding whose two paths disagree now has a *conditional* move, so
        // its move sites have to clear its slot even though it is still
        // dropped - see the module doc.
        for scope in &self.scopes {
            for owned in &scope.owned {
                if owned.state == MoveState::MaybeMoved {
                    self.maybe_moved.insert(owned.binding);
                }
            }
        }
    }
}

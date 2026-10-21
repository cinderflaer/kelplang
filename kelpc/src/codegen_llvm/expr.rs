use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    pub(crate) fn emit_expr(&mut self, e: &Expr) -> Result<Option<Value<'a>>, String> {
        match &e.kind {
            ExprKind::IntLit(v) => Ok(Some(self.int_const(&e.ty, *v)?)),
            ExprKind::CharLit(v) => Ok(Some(self.int_const(&e.ty, *v as i128)?)),
            ExprKind::FloatLit(v) => Ok(Some(self.float_const(&e.ty, *v)?)),
            ExprKind::BoolLit(b) => Ok(Some(self.g.ctx.const_bool(*b))),
            ExprKind::StringLit(s) => Ok(Some(self.string_global(s)?)),
            // A `unit` read is no value: the binding has no storage, so there
            // is nothing to load (`Stmt::Let` allocates none).
            ExprKind::Ident(_) if e.ty == Ty::Unit => Ok(None),
            ExprKind::Ident(name) => Ok(Some(self.load_ident(e, name)?)),
            // A `static` is one module-scope object (kernel stage 7), so a read
            // is a load from its global; a `percpu` static (kernel stage 8)
            // reads the copy `cpu_index()` selects.
            ExprKind::StaticRef(name) => {
                let (ty, slot) = self.static_slot(name)?;
                let lty = self.lty(&ty)?;
                Ok(Some(self.g.builder.build_load(lty, slot, name)))
            }
            ExprKind::Unary { op, expr } => Ok(Some(self.emit_unary(*op, expr, &e.ty)?)),
            ExprKind::Binary { op, lhs, rhs } => {
                Ok(Some(self.emit_binary(*op, lhs, rhs)?))
            }
            ExprKind::Call { callee, args } => self.emit_call(callee, args, &e.ty, e.span),
            // A variadic user `extern fun` call (gap 7): the call goes to the
            // generated fixed-arity wrapper, whose declaration and per-program
            // body is the same text this module compiles, while the
            // capability check reads the extern's own key.
            ExprKind::VariadicCall {
                callee,
                wrapper,
                args,
            } => self.emit_variadic_call(callee, wrapper, args, e.span),
            // `[a, b, c]`: the array struct is built field by field in order,
            // the same value the C compound literal is.
            ExprKind::ArrayLit(elems) => Ok(Some(self.emit_array_lit(elems, &e.ty)?)),
            // `[v; N]`: one call to the generated fill helper, so a large
            // repeat is a loop rather than N initializers.
            ExprKind::ArrayRepeat { value } => {
                Ok(Some(self.emit_array_repeat(value, &e.ty)?))
            }
            // `asm(...)` (kernel stage 5): one inline-asm call, with each
            // output stored into its place (or, for the anonymous output, the
            // call's own value).
            ExprKind::Asm(block) => self.emit_asm(block, &e.ty),
            ExprKind::Assign { target, value } => {
                self.emit_assign(target, value, e.span)?;
                Ok(None)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => self.emit_if(cond, then_block, else_block.as_ref(), &e.ty),
            ExprKind::Block(block) => self.emit_block_value(block),
            // The checker types a region as `Result<T, BlockError>`, and wrapping
            // its value in `Ok` is the lowering of step 4 (issue 194). Until
            // then a checked region is refused by name; a region typed as its
            // element (or as a statement) is lowered as it always was.
            ExprKind::Region { name, elem, body } => {
                // A checked region is typed `Result<T, BlockError>`: its value is
                // `Ok(value)`, because a block's failures come from `name.box()`.
                if e.ty != *elem && e.ty != Ty::Unit {
                    let Ty::Struct(rname) = &e.ty else {
                        return Err(format!("the block '{name}' has no Result type"));
                    };
                    let rname = rname.clone();
                    let lty = self.lty(&e.ty)?;
                    let res = self.temp_alloca(lty, "region.res");
                    let Some(value) = self.emit_region(name, elem, body)? else {
                        // The body returned, broke or failed: the code after the
                        // region is unreachable, so it goes into a block of its
                        // own, with a zero value standing in for the result.
                        let dead = self.cur_fn.append_block("region.dead");
                        self.g.builder.position_at_end(dead);
                        self.cur = dead;
                        self.terminated = false;
                        return self.zero_const(&e.ty).map(Some);
                    };
                    let ok = self.variant_index(&rname, "Ok")?;
                    self.store_enum_into(res, &rname, ok, &[value])?;
                    return Ok(Some(self.g.builder.build_load(lty, res, "region.ok")));
                }
                self.emit_region(name, elem, body)
            }
            ExprKind::BlockBox { name, elem } => self.emit_block_box(name, elem, &e.ty).map(Some),
            ExprKind::FieldAccess { base, field } => {
                Ok(Some(self.emit_field_read(e, base, field, &e.ty)?))
            }
            ExprKind::StructLit { name, fields, .. } => Ok(Some(self.emit_struct_lit(name, fields)?)),
            // `Enum.Variant(payload = ...)`: the tag, then each payload field
            // into the variant's union member.
            ExprKind::EnumLit {
                enum_name,
                variant,
                fields,
            } => Ok(Some(self.emit_enum_lit(enum_name, *variant, fields)?)),
            // The tag test a `when`/`if let` arm lowers to. `expr` is a place
            // (`infer.rs::check_when_scrutinee` guarantees it), so this reads a
            // field of it rather than of a copy.
            ExprKind::EnumTag { expr, variant } => {
                Ok(Some(self.emit_enum_tag(expr, *variant)?))
            }
            // One payload field of one variant, read out of a value whose tag is
            // already known to be `variant`. It is a *place*, which is what lets
            // an owned payload be bound as a borrow rather than moved out, and
            // what lets the move out by value clear the slot it came from.
            // A `unit` payload read is no value, the same rule an `Ident` read
            // follows: its variant has no struct field for it
            // (`VariantDef::is_payload_less`, `MISSING-r7Wqb`), so there is
            // nothing to load.
            ExprKind::EnumField { .. } if e.ty == Ty::Unit => Ok(None),
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => Ok(Some(self.emit_enum_field(e, base, *variant, *field, &e.ty)?)),
            // `expr?`: the operand once, then a branch on its failure tag. The
            // failure arm rebuilds the failure in the enclosing function's
            // return type, frees the frame (`DropPlan::at_try`) and returns; the
            // success arm is the operator's own value.
            ExprKind::Try {
                expr: sub,
                source,
                target,
                ok_variant,
                err_variant,
                target_err_variant,
                err_has_payload,
                convert,
            } => self.emit_try(
                sub,
                source,
                target,
                *ok_variant,
                *err_variant,
                *target_err_variant,
                *err_has_payload,
                convert,
                e.span,
                &e.ty,
            ),
            // A borrow (`infer.rs::check_call_arg` synthesizes one for a
            // parameter whose type resolves to a shared or exclusive borrow,
            // and `&x`/`&mut x` write one directly). `&str` is the pointer
            // itself, as the earlier emitter writes it, because there is no second
            // indirection; every other pointee is the address of the place it
            // borrows, which is the one indirection `getelementptr` needs and
            // the pointer the runtime's `const kelp_Bytes*` parameters take.
            // `Ty::Ref` is one pointer whatever the pointee (`llvm_type`), so
            // the place's address is the borrow, scalar or `datac` alike.
            ExprKind::Ref { expr: sub, .. } => {
                if is_str_ref(&e.ty) {
                    return Ok(Some(self.value_of(sub)?));
                }
                Ok(Some(self.place_of(sub)?))
            }
            // `*p`: the pointee is whatever the pointer addresses, a `datac`, a
            // scalar, `str` or a container, so the load is the pointee type's
            // own (`Ty::Str` loads the `char*` a `str` value is).
            ExprKind::Deref { expr: sub } => {
                // `*p` on a `&str` is the view itself: a `str` is already the
                // pointer, so there is no second indirection to load
                // (the expression value rule returns `expr_value(sub)` for the
                // same node). This is the shape a `pd` of type `str` reads
                // through, and a `&str` parameter read too.
                if e.ty == Ty::Str {
                    return Ok(Some(self.value_of(sub)?));
                }
                let ptr = self.value_of(sub)?;
                let lty = self.lty(&e.ty)?;
                Ok(Some(self.g.builder.build_load(lty, ptr, "deref")))
            }
            ExprKind::Template { newline, pieces } => {
                if e.ty == Ty::String {
                    return Ok(Some(self.emit_string_template(pieces)?));
                }
                self.emit_template(*newline, pieces)?;
                Ok(None)
            }
            // A declared `fun` (or a literal that captured nothing) used as a
            // value (S2E): the C function is the value, spelled as its address.
            ExprKind::FnRef(key) => Ok(Some(self.emit_fn_ref(key)?)),
            // A capturing `fun` literal: the heap environment, one field per
            // capture in capture order, then the fat pointer.
            ExprKind::ClosureLit { key, captures } => {
                Ok(Some(self.emit_closure_lit(key, captures, &e.ty)?))
            }
            // A scoped `fun` literal (`kelp wiki page language/functions.md` part 2): the
            // same fat pointer, but the environment is an `alloca` of this
            // function and the `drop` member is `null`.
            ExprKind::ScopedClosureLit { key, captures } => {
                Ok(Some(self.emit_scoped_closure_lit(key, captures, &e.ty)?))
            }
            // One field of the enclosing closure's environment. It is a
            // *place* (the environment owns the value), so the read is the
            // field's own type loaded out of that address.
            ExprKind::Capture { index } => {
                let place = self.capture_place(*index)?;
                let lty = self.lty(&e.ty)?;
                Ok(Some(self.g.builder.build_load(lty, place, "capture")))
            }
            // A plain function value where a closure is expected: the widening
            // (S2E), a shim for a known function and a thunk for one whose
            // identity the call knows.
            ExprKind::FnToClosure { func } => {
                Ok(Some(self.emit_fn_to_closure(func, &e.ty)?))
            }
            ExprKind::IndirectCall { callee, args } => {
                self.emit_indirect_call(callee, args, &e.ty)
            }
            // A borrow written where a `&dyn Trait` was expected (A-12 slice
            // 4): the fat pair, the address the borrow already produced and the
            // pair's read-only table.
            ExprKind::DynCoerce { trait_name, type_name, expr: inner } => {
                let data = self.value_of(inner)?;
                let table = *self
                    .g
                    .dyn_tables
                    .get(&(trait_name.clone(), type_name.clone()))
                    .ok_or_else(|| {
                        no_value(&format!(
                            "the vtable for '{trait_name}' and '{type_name}'"
                        ))
                    })?;
                let lty = self.lty(&e.ty)?;
                let mut agg = self.g.ctx.undef(lty);
                agg = self.g.builder.build_insert_value(agg, data, 0, "data");
                agg = self.g.builder.build_insert_value(agg, table, 1, "vtable");
                Ok(Some(agg))
            }
            // An owned trait object where a `Box<dyn Trait>` was expected
            // (A-12 slice 7): the box's own allocation pointer becomes the
            // trait object's data, and the pair carries the same vtable a
            // `&dyn` does. The box field is read, never copied into a second
            // allocation.
            ExprKind::BoxDynCoerce { trait_name, type_name, expr: inner } => {
                let boxed = self.value_of(inner)?;
                let data = self.g.builder.build_extract_value(boxed, 0, "data");
                let table = *self
                    .g
                    .dyn_tables
                    .get(&(trait_name.clone(), type_name.clone()))
                    .ok_or_else(|| {
                        no_value(&format!(
                            "the vtable for '{trait_name}' and '{type_name}'"
                        ))
                    })?;
                let lty = self.lty(&e.ty)?;
                let mut agg = self.g.ctx.undef(lty);
                agg = self.g.builder.build_insert_value(agg, data, 0, "data");
                agg = self.g.builder.build_insert_value(agg, table, 1, "vtable");
                Ok(Some(agg))
            }
            // A `&dyn Trait` borrowed back out of an owned trait object
            // (A-12 slice 7): the same two words, read out of the box's own
            // value. Nothing is allocated and nothing is freed.
            ExprKind::BoxDynBorrow { expr: inner, .. } => Ok(Some(self.value_of(inner)?)),
            // `b.is_empty()` (A-12 slice 7): the box's data pointer is null.
            ExprKind::BoxDynIsEmpty { expr: inner } => {
                let pair = self.value_of(inner)?;
                let data = self.g.builder.build_extract_value(pair, 0, "data");
                let null = self.g.ctx.const_null(self.g.ctx.opaque_ptr());
                Ok(Some(self.g.builder.build_icmp(
                    IntCmp::Eq,
                    data,
                    null,
                    "is_empty",
                )))
            }
            // A method through a vtable (A-12 slice 4):
            // `receiver.vtable[slot](receiver.data, args...)`.
            ExprKind::DynCall { receiver, slot, args, .. } => {
                let recv = self.value_of(receiver)?;
                let data = self.g.builder.build_extract_value(recv, 0, "data");
                let table = self.g.builder.build_extract_value(recv, 1, "vtable");
                // The table is `{ [N x ptr], drop, size, align }`: the method
                // array is its first field, and the slot index is a constant,
                // so the call is one struct GEP and one element GEP (A-12
                // slices 4 and 7).
                let trait_name = match &receiver.ty {
                    Ty::Dyn { trait_name, .. } | Ty::BoxDyn { trait_name } => trait_name.clone(),
                    other => {
                        return Err(no_value(&format!(
                            "the trait of the vtable call on '{}'",
                            other.display()
                        )))
                    }
                };
                let table_ty = *self
                    .g
                    .dyn_table_tys
                    .get(&trait_name)
                    .ok_or_else(|| no_value(&format!("the vtable layout of '{trait_name}'")))?;
                let slots = self
                    .g
                    .builder
                    .build_struct_gep(table_ty, table, 0, "slots");
                let slot_index = self
                    .g
                    .ctx
                    .const_int(self.g.ctx.i64(), *slot as u64, false);
                let slot_ptr = self
                    .g
                    .builder
                    .build_gep(self.g.pty(), slots, slot_index, "m");
                let fp = self.g.builder.build_load(self.g.pty(), slot_ptr, "fp");
                // The first parameter is the erased data pointer the thunk
                // takes in place of the receiver.
                let mut params: Vec<Type<'a>> = vec![self.g.pty()];
                let mut argv: Vec<Value<'a>> = vec![data];
                for a in args {
                    if a.ty == Ty::Unit {
                        continue;
                    }
                    params.push(self.lty(&a.ty)?);
                    argv.push(self.value_of(a)?);
                }
                let ret = if e.ty == Ty::Unit {
                    self.g.ctx.void()
                } else {
                    self.lty(&e.ty)?
                };
                let fnty = self.g.ctx.fn_type(ret, &params, false);
                if e.ty == Ty::Unit {
                    self.g.builder.build_indirect_call(fp, fnty, &argv, "");
                    Ok(None)
                } else {
                    Ok(Some(self.g.builder.build_indirect_call(
                        fp, fnty, &argv, "dyncall",
                    )))
                }
            }
        }
    }

    /// Read one `datac` field: the base's address, a `getelementptr` to the
    /// field's index, then a load of the field's own type.
    ///
    /// `ty` is the field expression's `tir` type, which is the pointee type the
    /// load needs and does not have to be recovered from the struct table.
    pub(crate) fn emit_field_read(
        &mut self,
        e: &Expr,
        base: &Expr,
        field: &str,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        // A chain rooted at an rvalue that owns memory (`mkName().r`) has
        // nowhere to hang the drop of the temporary: nothing else owns it and
        // nothing else frees it. The root is copied into a slot, the field is
        // read out of that copy, the temporary is dropped, and the value is
        // handed on, so the temporary and every buffer its other fields own die
        // at the end of the expression that read through it. This is
        // the staged field value rule, and it is the reason a two-owned-
        // field `datac` is the interesting case: the drop is at the temporary,
        // not at the field.
        //
        // The value is read out before the drop runs, because a drop frees
        // memory. `infer.rs` already refuses to read an *owned* field by value,
        // so the value is never the buffer the drop frees.
        if let Some(root) = temporary_root(base) {
            if self.g.owns_memory(&root.ty) {
                let root_ty = root.ty.clone();
                let lty = self.lty(&root_ty)?;
                let slot = self.temp_alloca(lty, "field.root");
                let value = self.value_of(root)?;
                self.g.builder.build_store(value, slot);
                let base_ptr = self.chain_addr(slot, base)?;
                let (struct_ty, index) = self.field_slot(&base.ty, field)?;
                let field_ptr =
                    self.g
                        .builder
                        .build_struct_gep(struct_ty, base_ptr, index, field);
                let lty = self.lty(ty)?;
                let read = self.g.builder.build_load(lty, field_ptr, field);
                let out = self.temp_alloca(lty, "field.value");
                self.g.builder.build_store(read, out);
                self.g.call_drop(slot, &root_ty)?;
                return Ok(self.g.builder.build_load(lty, out, "field"));
            }
        }
        let (ptr, struct_ty, index) = self.field_place(base, field)?;
        let field_ptr = self
            .g
            .builder
            .build_struct_gep(struct_ty, ptr, index, field);
        let lty = self.lty(ty)?;
        let read = self.g.builder.build_load(lty, field_ptr, field);
        // A partial move copies the field out and zeroes the slot it came
        // from. The binding keeps its value as a whole and is freed at scope
        // exit as it always was, so without this the free would reach the
        // buffer the moved value now owns, the same clear a whole-binding
        // move does in `load_ident`, one level down.
        match self.g.drops.clear_at(&self.fn_name, e) {
            // Not a partial move: the read copies nothing out, so the slot
            // keeps its value and the binding's own drop frees it.
            None => {}
            // The field this read copied out: zero the slot, so the binding's
            // drop does not free the value a second time.
            Some(Projection::Field(moved)) if moved == field => {
                self.g.builder.build_store(self.g.ctx.const_null(lty), field_ptr);
            }
            Some(Projection::NestedField(path)) if path.last().is_some_and(|moved| moved == field) => {
                self.g.builder.build_store(self.g.ctx.const_null(lty), field_ptr);
            }
            // A projection the backend has no clear for. Zeroing a slot that
            // still holds its value, or skipping the zero of one that does
            // not, frees the same buffer twice, so an arm per variant is what
            // has to be filled in rather than passed over.
            Some(Projection::Field(moved)) => panic!(
                "codegen_llvm: a move of field '{moved}' was recorded at a read of '{field}'"
            ),
            Some(Projection::NestedField(path)) => panic!(
                "codegen_llvm: a move of nested field '{}' was recorded at a read of '{field}'",
                path.join(".")
            ),
            Some(Projection::EnumPayload { name, .. }) => panic!(
                "codegen_llvm: a move of payload '{name}' was recorded at a read of field '{field}'"
            ),
        }
        Ok(read)
    }

    /// Build a `datac` value: one `insertvalue` per field, in declaration order.
    ///
    /// `tir::ExprKind::StructLit` always lists every field, in declaration
    /// order (`infer.rs` zips the definition's fields with the checked
    /// arguments), so the index of each value is its position in the vector and
    /// the starting `undef` is fully overwritten.
    pub(crate) fn emit_struct_lit(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Result<Value<'a>, String> {
        let lty = *self
            .g
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let mut agg = self.g.ctx.undef(lty);
        for (index, (fname, fexpr)) in fields.iter().enumerate() {
            let value = self.value_of(fexpr)?;
            agg = self
                .g
                .builder
                .build_insert_value(agg, value, index as u32, fname);
        }
        Ok(agg)
    }

    /// A declared `fun` (or a literal that captured nothing) named as a value
    /// (S2E): the function's own address, which *is* the value. Nothing is
    /// allocated and nothing is owned.
    pub(crate) fn emit_fn_ref(&self, key: &str) -> Result<Value<'a>, String> {
        self.g
            .funcs
            .get(key)
            .map(|info| info.func.as_value())
            // Only an `extern fun` can be missing here: an `extern fun` is a
            // declaration the runtime owns, which is the S3 slice, and its name
            // is the honest refusal.
            .ok_or_else(|| refuse(&format!("the function value '{key}'"), "S3"))
    }

    /// A capturing `fun` literal (S2E): the environment is heap (MK7 §3.3
    /// spells the pointer out), one field per capture in capture order, and the
    /// value is the fat pointer `{ fn, env, drop }`.
    pub(crate) fn emit_closure_lit(
        &mut self,
        key: &str,
        captures: &[Expr],
        sig: &Ty,
    ) -> Result<Value<'a>, String> {
        let func = self
            .g
            .funcs
            .get(key)
            .cloned()
            .ok_or_else(|| no_value(&format!("the literal '{key}'")))?;
        let closure_ty = self.g.closure_value_ty(sig)?;
        let env_ty = *self
            .g
            .env_types
            .get(key)
            .ok_or_else(|| no_value(&format!("the environment of '{key}'")))?;
        let env = if self.g.managed {
            let descr = self.g.closure_env_descr(key)?;
            let (func, fty) = self.g.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?.alloc;
            self.g.call(func, fty, &[descr], "env")
        } else {
            self.g.malloc_for(env_ty)?
        };
        self.cur = self.g.check_alloc(env, self.cur_fn)?;
        for (index, init) in captures.iter().enumerate() {
            let value = self.value_of(init)?;
            let field = self.g.builder.build_struct_gep(
                env_ty,
                env,
                index as u32,
                &capture_field(index),
            );
            self.g.builder.build_store(value, field);
        }
        let drop = self.g.closure_symbol(&crate::types::env_drop_name(key))?;
        let mut agg = self.g.ctx.undef(closure_ty);
        agg = self.g.builder.build_insert_value(agg, func.func.as_value(), 0, "fn");
        agg = self.g.builder.build_insert_value(agg, env, 1, "env");
        agg = self.g.builder.build_insert_value(agg, drop.as_value(), 2, "drop");
        Ok(agg)
    }

    /// A scoped `fun` literal (`kelp wiki page language/functions.md` part 2): the same fat
    /// pointer `emit_closure_lit` builds, but the environment is an `alloca`
    /// of the enclosing function - it cannot outlive the call the literal was
    /// written in - and the `drop` member is `null`, because every field is a
    /// `Copy` value or a shared borrow and the environment owns nothing.
    pub(crate) fn emit_scoped_closure_lit(
        &mut self,
        key: &str,
        captures: &[Expr],
        sig: &Ty,
    ) -> Result<Value<'a>, String> {
        let func = self
            .g
            .funcs
            .get(key)
            .cloned()
            .ok_or_else(|| no_value(&format!("the literal '{key}'")))?;
        let closure_ty = self.g.closure_value_ty(sig)?;
        let env_ty = *self
            .g
            .env_types
            .get(key)
            .ok_or_else(|| no_value(&format!("the environment of '{key}'")))?;
        let env = self.temp_alloca(env_ty, "scoped.env");
        for (index, init) in captures.iter().enumerate() {
            let value = self.value_of(init)?;
            let field = self.g.builder.build_struct_gep(
                env_ty,
                env,
                index as u32,
                &capture_field(index),
            );
            self.g.builder.build_store(value, field);
        }
        let null = self.g.ctx.const_null(self.g.pty());
        let mut agg = self.g.ctx.undef(closure_ty);
        agg = self.g.builder.build_insert_value(agg, func.func.as_value(), 0, "fn");
        agg = self.g.builder.build_insert_value(agg, env, 1, "env");
        agg = self.g.builder.build_insert_value(agg, null, 2, "drop");
        Ok(agg)
    }

    /// The widening of a plain function value into a closure (S2E). A *known*
    /// function takes the shim, which ignores the environment, so the value
    /// carries `null` there and allocates nothing; a value whose identity only
    /// the call knows gets the one-field heap environment and the thunk that
    /// reads the code pointer back out of it.
    pub(crate) fn emit_fn_to_closure(&mut self, func: &Expr, sig: &Ty) -> Result<Value<'a>, String> {
        let closure_ty = self.g.closure_value_ty(sig)?;
        let null = self.g.ctx.const_null(self.g.pty());
        if let ExprKind::FnRef(key) = &func.kind {
            let shim = self.g.closure_symbol(&shim_symbol(key, sig))?;
            let mut agg = self.g.ctx.undef(closure_ty);
            agg = self.g.builder.build_insert_value(agg, shim.as_value(), 0, "fn");
            agg = self.g.builder.build_insert_value(agg, null, 1, "env");
            agg = self.g.builder.build_insert_value(agg, null, 2, "drop");
            return Ok(agg);
        }
        let code = self.value_of(func)?;
        let env_ty = self.g.coerce_env_ty;
        let env = if self.g.managed {
            let descr = self.g.coerce_env_descr()?;
            let (func, fty) = self.g.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?.alloc;
            self.g.call(func, fty, &[descr], "env")
        } else {
            self.g.malloc_for(env_ty)?
        };
        self.cur = self.g.check_alloc(env, self.cur_fn)?;
        let slot = self
            .g
            .builder
            .build_struct_gep(env_ty, env, 0, "coerce.f");
        self.g.builder.build_store(code, slot);
        let thunk = self.g.closure_symbol(&coerce_thunk_symbol(sig))?;
        let drop = self.g.closure_symbol(&coerce_drop_symbol(sig))?;
        let mut agg = self.g.ctx.undef(closure_ty);
        agg = self.g.builder.build_insert_value(agg, thunk.as_value(), 0, "fn");
        agg = self.g.builder.build_insert_value(agg, env, 1, "env");
        agg = self.g.builder.build_insert_value(agg, drop.as_value(), 2, "drop");
        Ok(agg)
    }

    /// A call through a function value (S2E): a plain function pointer is the
    /// callee, and a closure is the fat pointer, so the call names its code
    /// pointer and passes the environment as the hidden first argument
    /// (`(c.fn)(c.env, ...)`, the shape the earlier emitter wrote).
    pub(crate) fn emit_indirect_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        // A closure value or a borrow of one: the code pointer and the
        // environment come out of the same fat pointer the earlier emitter projects.
        if let Some(sig) = closure_callee_sig(&callee.ty) {
            // The callee is read before the arguments, as the earlier emitter binds
            // it to a temporary in front of the call, so a field or a
            // dereference is evaluated once.
            let (code, env) = self.closure_code_and_env(callee)?;
            let (params, ret_lty) = self.g.closure_sig_types(&sig)?;
            let mut all = Vec::with_capacity(params.len() + 1);
            all.push(env);
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            for a in args {
                // A `unit` argument crosses as nothing (see the direct-call
                // arm); the parameter list above has no slot for it either.
                if a.ty == Ty::Unit {
                    self.emit_expr(a)?;
                    continue;
                }
                all.push(self.arg_value(a, &mut held)?);
            }
            let mut call_params = Vec::with_capacity(params.len() + 1);
            call_params.push(self.g.pty());
            call_params.extend(params);
            let call_ty = self.g.ctx.fn_type(ret_lty, &call_params, false);
            let yields = *ret != Ty::Unit;
            let name = if yields { "call" } else { "" };
            let v = self
                .g
                .builder
                .build_indirect_call(code, call_ty, &all, name);
            self.free_held(held)?;
            return Ok(if yields { Some(v) } else { None });
        }

        // A plain function value: the pointer itself is the callee. A borrow of
        // one holds the address of the function pointer, so the call loads it
        // first (`*p`), exactly as reading the pointee would.
        let code = match &callee.ty {
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Fn { .. }) => {
                let p = self.value_of(callee)?;
                self.g.builder.build_load(self.g.pty(), p, "callee")
            }
            _ => self.value_of(callee)?,
        };
        let (params, ret_lty) = self.g.fn_value_sig(&callee.ty)?;
        let mut all: Vec<Value<'a>> = Vec::with_capacity(args.len());
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        for a in args {
            // A `unit` argument crosses as nothing (see the direct-call arm).
            if a.ty == Ty::Unit {
                self.emit_expr(a)?;
                continue;
            }
            all.push(self.arg_value(a, &mut held)?);
        }
        let call_ty = self.g.ctx.fn_type(ret_lty, &params, false);
        let yields = *ret != Ty::Unit;
        let name = if yields { "call" } else { "" };
        let v = self
            .g
            .builder
            .build_indirect_call(code, call_ty, &all, name);
        self.free_held(held)?;
        Ok(if yields { Some(v) } else { None })
    }

    /// The code pointer and the environment of a closure callee (S2E). A
    /// `Ty::Closure` callee is read as a *place* (borrowck guarantees it), so
    /// the fat pointer is projected rather than copied out: the binding that
    /// owns the environment keeps it. A `Ty::Ref` callee is already the
    /// pointer to one.
    pub(crate) fn closure_code_and_env(&mut self, callee: &Expr) -> Result<(Value<'a>, Value<'a>), String> {
        let ptr = self.g.pty();
        let (base, ty) = if matches!(callee.ty, Ty::Ref { .. }) {
            (self.value_of(callee)?, self.g.closure_value_ty(&callee.ty)?)
        } else {
            let ty = self.g.closure_value_ty(&callee.ty)?;
            let place = if is_place_expr(callee) {
                self.place_of(callee)?
            } else {
                let value = self.value_of(callee)?;
                let slot = self.temp_alloca(ty, "callee");
                self.g.builder.build_store(value, slot);
                slot
            };
            (place, ty)
        };
        let code = self.g.builder.build_load(
            ptr,
            self.g.builder.build_struct_gep(ty, base, 0, "fn"),
            "fn",
        );
        let env = self.g.builder.build_load(
            ptr,
            self.g.builder.build_struct_gep(ty, base, 1, "env"),
            "env",
        );
        Ok((code, env))
    }

    /// The address of one field of the enclosing closure's environment (S2E).
    /// The environment owns the capture, so this is a *place*: an owned capture
    /// is borrowed rather than moved out.
    pub(crate) fn capture_place(&mut self, index: usize) -> Result<Value<'a>, String> {
        let slot = self
            .env_slot
            .ok_or_else(|| no_value("a capture outside a closure"))?;
        let env_ty = self
            .env_ty
            .ok_or_else(|| no_value("a capture outside a closure"))?;
        let raw = self.g.builder.build_load(self.g.pty(), slot, "env");
        Ok(self.g.builder.build_struct_gep(
            env_ty,
            raw,
            index as u32,
            &capture_field(index),
        ))
    }

    /// The layout of the `enum` a (possibly borrowed) expression's type names.
    /// A clone, because the callers all need to walk a place mutably at the same
    /// time, and the layout is a handful of `Copy` handles.
    pub(crate) fn enum_info_of(&self, ty: &Ty) -> Result<EnumInfo<'a>, String> {
        let name = struct_name_of_ty(ty).ok_or_else(|| refuse_type(ty))?;
        self.g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))
    }

    /// The address of an `enum` place, plus its layout. `place_of` decides
    /// whether that address is the local's own slot or the pointer a `&Shape`
    /// binding holds, so a `move` parameter and a borrowed one take the same
    /// path here.
    pub(crate) fn enum_place(&mut self, base: &Expr) -> Result<(Value<'a>, EnumInfo<'a>), String> {
        let info = self.enum_info_of(&base.ty)?;
        let ptr = self.place_of(base)?;
        Ok((ptr, info))
    }

    /// `expr.kelp_tag == variant`: the tag test a `when`/`if let` arm lowers to.
    /// The comparison is on `i32` because that is the C typedef's tag width, and
    /// the variant index is the same compile-time constant both backends write.
    pub(crate) fn emit_enum_tag(&mut self, base: &Expr, variant: usize) -> Result<Value<'a>, String> {
        let (ptr, info) = self.enum_place(base)?;
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, ptr, ENUM_TAG_FIELD, "tag");
        let tag = self.g.builder.build_load(info.tag_ty, tag_ptr, "tag");
        let want = self.g.ctx.const_int(info.tag_ty, variant as u64, false);
        Ok(self
            .g
            .builder
            .build_icmp(IntCmp::Eq, tag, want, "isvar"))
    }

    /// The address of one payload field of one variant, inside the union. The
    /// union's own address is the enum's field 1; the address inside it is the
    /// *variant payload struct*'s field index, which is the C
    /// `kelp_v.kelp_v_<Variant>.<field>` path with the same offsets.
    pub(crate) fn enum_field_place(
        &mut self,
        base: &Expr,
        variant: usize,
        field: usize,
    ) -> Result<Value<'a>, String> {
        let (ptr, info) = self.enum_place(base)?;
        let v = info
            .variants
            .get(variant)
            .ok_or_else(|| no_value(&format!("the variant {variant} of an enum")))?;
        let payload_ty = v.payload_ty;
        let index = v.payload_index(field);
        let union_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, ptr, ENUM_UNION_FIELD, "kelp_v");
        Ok(self
            .g
            .builder
            .build_struct_gep(payload_ty, union_ptr, index, "field"))
    }

    /// Read one payload field of one variant as a value.
    ///
    /// An owned payload read by value is a partial move (A-11), exactly as an
    /// owned `datac` field read is: the value is copied out and the slot it came
    /// from is zeroed, so the enum's own drop frees the payloads the move left
    /// behind and nothing where the value now lives. `e` is the node being
    /// rendered, which is the identity the plan's clear is keyed on.
    pub(crate) fn emit_enum_field(
        &mut self,
        e: &Expr,
        base: &Expr,
        variant: usize,
        field: usize,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let ptr = self.enum_field_place(base, variant, field)?;
        let lty = self.lty(ty)?;
        let read = self.g.builder.build_load(lty, ptr, "payload");
        match self.g.drops.clear_at(&self.fn_name, e) {
            // Not a partial move: the read copies nothing out, so the slot
            // keeps its value and the binding's own drop frees it.
            None => {}
            // The payload this read copied out: zero the slot.
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) if *moved_variant == variant && *moved_field == field => {
                self.g
                    .builder
                    .build_store(self.g.ctx.const_null(lty), ptr);
            }
            // A projection the backend has no clear for. Zeroing a slot that
            // still holds its value, or skipping the zero of one that does not,
            // frees the same buffer twice, so an arm per variant is what has to
            // be filled in rather than passed over.
            Some(Projection::Field(moved)) => panic!(
                "codegen_llvm: a move of field '{moved}' was recorded at a read of payload {variant}.{field}"
            ),
            Some(Projection::NestedField(path)) => panic!(
                "codegen_llvm: a move of nested field '{}' was recorded at a read of payload {variant}.{field}",
                path.join(".")
            ),
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) => panic!(
                "codegen_llvm: a move of payload {moved_variant}.{moved_field} was recorded at a read of payload {variant}.{field}"
            ),
        }
        Ok(read)
    }

    /// Build one `enum` value: a zeroinitializer, the tag, then each payload
    /// field stored into the variant's union member.
    ///
    /// A tagged union has no single LLVM value to build field by field, because
    /// the union field's type is not the variant's payload type; the address of
    /// the union is, so the literal is materialized in a temporary and loaded
    /// back. The zeroinitializer is what the C compound literal does for the
    /// members its designators do not mention, so an `enum` copied by value never
    /// carries an undefined byte.
    pub(crate) fn emit_enum_lit(
        &mut self,
        name: &str,
        variant: usize,
        fields: &[Expr],
    ) -> Result<Value<'a>, String> {
        let info = self
            .g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))?;
        let slot = self.temp_alloca(info.llvm_ty, "enum.tmp");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(info.llvm_ty), slot);
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.ctx.const_int(info.tag_ty, variant as u64, false);
        self.g.builder.build_store(tag, tag_ptr);
        // A `unit` field is never stored (`VariantDef::is_payload_less` gives
        // an all-`unit` variant the bare `i8` placeholder rather than a
        // struct, `MISSING-r7Wqb`): its expression is still emitted, for
        // whatever side effect it has, but discarded rather than written
        // through a GEP that would not typecheck against that placeholder.
        // The struct index counts only the fields that do get a slot, so it
        // stays in step with the payload struct `declare_enum_body` built
        // from the same filter.
        let has_real_field = fields.iter().any(|f| f.ty != Ty::Unit);
        let union_ptr = if has_real_field {
            Some(self.g.builder.build_struct_gep(
                info.llvm_ty,
                slot,
                ENUM_UNION_FIELD,
                "kelp_v",
            ))
        } else {
            None
        };
        let payload_ty = if has_real_field {
            Some(
                info.variants
                    .get(variant)
                    .ok_or_else(|| no_value(&format!("the variant {variant} of '{name}'")))?
                    .payload_ty,
            )
        } else {
            None
        };
        let mut index = 0u32;
        for field in fields {
            if field.ty == Ty::Unit {
                self.emit_expr(field)?;
                continue;
            }
            let value = self.value_of(field)?;
            let field_ptr = self.g.builder.build_struct_gep(
                payload_ty.expect("a real field implies a payload struct"),
                union_ptr.expect("a real field implies a union pointer"),
                index,
                "field",
            );
            self.g.builder.build_store(value, field_ptr);
            index += 1;
        }
        Ok(self.g.builder.build_load(info.llvm_ty, slot, "enum"))
    }
}

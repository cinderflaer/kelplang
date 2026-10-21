use super::*;

impl TypeScope<'_> {
    pub(crate) fn resolve_param(&self, p: &ast::Param) -> Result<Ty> {
        self.resolve_param_mode(&p.ty, p.mode)
    }

    /// One parameter's type, from the type as written and the qualifier in
    /// front of it. Shared by a declaration's parameter and a function type's
    /// parameter (A-4), because the two have to mean the same thing: a
    /// written `fun(String)` has to be the type of `fun f(s: String)`.
    ///
    /// `move T` resolves to the bare type - the same by-value shape a `Copy`
    /// type already has. That is the whole type-level difference, and it is
    /// what makes a moved argument an ordinary whole-value read at the call
    /// site: no new argument-matching rule, and `borrowck.rs` marks the
    /// caller's binding moved exactly as `val q = p` does.
    pub(crate) fn resolve_param_mode(&self, ty: &ast::TypeExpr, mode: ast::ParamMode) -> Result<Ty> {
        if ty.scoped {
            return self.resolve_scoped_param(ty, mode);
        }
        if mode.moves() {
            return self.resolve_move_param(ty);
        }
        // `&[T; N]`: a written `&` on a *parameter* is otherwise what the bare
        // name already means, but an array is a by-value type, so the `&` is
        // the one way to write "a borrow of the caller's array" (A-20, and the
        // shape the const-generic examples use). An exclusive `mut [T; N]`
        // takes the ordinary path below.
        if ty.array_elem.is_some() && ty.amp && !mode.mutable() {
            let base = self.resolve(ty, false)?;
            return Ok(Ty::Ref {
                mutable: false,
                inner: Box::new(base),
            });
        }
        let base = self.resolve(ty, false)?;
        if mode.mutable() && matches!(base, Ty::Fn { .. }) {
            // A function value is Copy, so an exclusive borrow of one would
            // need a pointer to a function pointer that nothing can usefully
            // write through.
            return Err(TypeError {
                help: None,
                message: format!(
                    "'mut' cannot be written on the function type '{}' - a function value is Copy (it is a plain function pointer), so there is nothing to write through; write the type without 'mut'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        Ok(implicit_borrow_wrap(base, mode.mutable()))
    }

    /// `scoped fun(T) => U` - the non-escaping callback type
    /// (`kelp wiki page language/functions.md` part 2). The qualifier is only meaningful as
    /// written: `mut` needs something to write through and `move` something
    /// to own, and a scoped value is neither, so both are refused by name
    /// rather than silently dropped. The parameters and return resolve as a
    /// `fun` type's do, with `scoped` refused inside them (a scoped value
    /// nested in another type could be stored as that type).
    pub(crate) fn resolve_scoped_param(&self, ty: &ast::TypeExpr, mode: ast::ParamMode) -> Result<Ty> {
        if mode.moves() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'move' cannot be written on '{}' - a scoped function value is a fat pointer with a stack environment, so there is nothing to own; write it without 'move'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        if mode.mutable() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'mut' cannot be written on '{}' - a scoped function value is Copy and has nothing to write through; write it without 'mut'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        if ty.amp {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'&' cannot be written on '{}' - a scoped function value is already a fat pointer passed by value; write it without '&'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let params = ty.fn_params.as_ref().expect("the parser only sets 'scoped' before 'fun'");
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            if p.ty.scoped {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'scoped' cannot be nested inside another function type ('{}'): a value of such a type could be stored, which a scoped function value may not be",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        let ret = match &ty.fn_ret {
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::ScopedFn {
            params: resolved,
            ret: Box::new(ret),
        })
    }

    /// The type of a `move T` parameter, with the spellings that have nothing
    /// to hand over refused: a `str` is already a borrowed view, `&T` is
    /// already a copy of a borrow, and a function value is already Copy.
    /// Those would read as an ownership transfer and mean nothing.
    pub(crate) fn resolve_move_param(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        let base = self.resolve(ty, false)?;
        let nothing_to_own = ty.amp
            || matches!(base, Ty::Str | Ty::Ref { .. } | Ty::Fn { .. });
        if nothing_to_own {
            // `TypeExpr::display` prints only the name, so an explicit `&`
            // has to be put back for the diagnostic to quote what was
            // written.
            let written = if ty.amp {
                format!("&{}", ty.display())
            } else {
                ty.display()
            };
            return Err(TypeError {
                help: None,
                message: format!(
                    "'move' cannot be written on '{written}' - a 'str' is already a borrowed view, '&T' is already a copy of a borrow, and a function value is already Copy, so there is nothing to take ownership of; write the owned type instead ('move String')"
                ),
                span: ty.span,
            });
        }
        Ok(base)
    }

    /// A function type (A-4). Each parameter goes through the *same* rule a
    /// declaration's parameter does - an implicit shared borrow for an owned
    /// type, `mut` for an exclusive one, `move` for ownership - which is what
    /// makes a written `fun(String)` and the signature of `fun f(s: String)`
    /// the same type, and therefore what makes passing `f` where a
    /// `fun(String)` is expected type-check with no conversion.
    ///
    /// Two placements are refused, both because the C declarator has no
    /// sensible spelling for them: a function type that *returns* a function
    /// type, and a borrow of a function value (which is Copy, so there is
    /// nothing to borrow). A function type as a parameter of a function type
    /// is fine - `int32_t (*)(int32_t (*)(int32_t))` is ordinary C.
    pub(crate) fn resolve_fn_type(&self, ty: &ast::TypeExpr, params: &[ast::FnTypeParam]) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'&' cannot be written on the function type '{}' - a function value is Copy (it is a plain function pointer), so a borrowed one would be a pointer to a pointer; write the type without '&'",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            // `scoped` is a parameter modifier of a *declaration*, never of a
            // parameter inside a function type: such a value could be stored
            // as the function type, which is the one thing it forbids.
            if p.ty.scoped {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'scoped {}' cannot be a parameter of a function type - a function type is a value that can be stored and passed on, and a scoped function value may only live for one call; write it as a top-level parameter of a declaration instead",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a function type cannot return a borrow ('&{}') - the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a function *type* is called through a pointer where there is none; return an owned value instead",
                        r.display()
                    ),
                    span: r.span,
                });
            }
        }
        let ret = match &ty.fn_ret {
            Some(r) if r.is_fn() => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a function type cannot return another function type ('{}') - C cannot write that declarator, and nothing needs it; return a 'datac' holding the function value instead",
                        ty.display()
                    ),
                    span: r.span,
                })
            }
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::Fn { params: resolved, ret: Box::new(ret) })
    }

    pub(crate) fn resolve_closure_type(&self, ty: &ast::TypeExpr, params: &[ast::FnTypeParam]) -> Result<Ty> {
        if ty.amp {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'&' cannot be written on the closure type '{}' - a closure is an owned value, and a parameter of that type already borrows it; write the type without '&', or 'move {}' to take ownership",
                    ty.display(),
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let mut resolved = Vec::with_capacity(params.len());
        for p in params {
            if p.ty.scoped {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'scoped {}' cannot be a parameter of a closure type - a closure type is a value that can be stored and passed on, and a scoped function value may only live for one call; write it as a top-level parameter of a declaration instead",
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            resolved.push(self.resolve_param_mode(&p.ty, p.mode)?);
        }
        if let Some(r) = &ty.fn_ret {
            if r.amp {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a closure type cannot return a borrow ('&{}') - the elision rule that says which argument a borrow comes from reads a declaration's parameter list, and a closure is called through a value; return an owned value instead",
                        r.display()
                    ),
                    span: r.span,
                });
            }
        }
        let ret = match &ty.fn_ret {
            Some(r) if r.is_fn() || r.is_closure() => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a closure type cannot return a function type ('{}') - C cannot write that declarator, and a closure's result is a value rather than a call target; return a 'datac' holding the function value instead",
                        ty.display()
                    ),
                    span: r.span,
                })
            }
            Some(r) => self.resolve(r, false)?,
            None => Ty::Unit,
        };
        Ok(Ty::Closure { params: resolved, ret: Box::new(ret) })
    }

    pub(crate) fn resolve_receiver(&self, r: &ast::Receiver) -> Result<Ty> {
        // A receiver names a *type*, so a generic type needs its arguments
        // - and there is no spelling for them on a receiver yet. Say that
        // rather than let the ordinary "needs N type arguments" message
        // arrive with no explanation of where they would go (A-2).
        //
        // The name may already be canonical (`std.io.File`, rewritten by
        // `resolve.rs`) or still be written with an alias, so both spellings
        // are tried; a name that resolves through neither is left to
        // `resolve` below, which reports it properly.
        if self.env.generics.contains_key(&r.type_name)
            || self
                .qualify(&r.type_name, r.span)
                .map(|q| self.env.generics.contains_key(&q))
                .unwrap_or(false)
        {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is a generic type - the receiver has to name its type parameters, and they have to be the type's own, in order ('fun {}<{}>.method(...)')",
                    r.type_name,
                    r.type_name,
                    self.env
                        .generics
                        .get(&r.type_name)
                        .map(|t| t.params.join(", "))
                        .unwrap_or_default()
                ),
                span: r.span,
            });
        }
        let te = ast::TypeExpr::named(r.type_name.clone(), r.span);
        Ok(implicit_borrow_wrap(self.resolve(&te, false)?, r.mutable))
    }

    /// A declared return type. This is the one place a borrow *can* be
    /// written down: `&str`/`&T` in a return position becomes a `Ty::Ref`
    /// under the elision rule, meaning "a borrow of the first reference
    /// parameter" (see `borrowck.rs`, which enforces exactly that).
    pub(crate) fn resolve_return(&self, ty: &ast::TypeExpr) -> Result<Ty> {
        // A borrowed trait object may not be returned (A-12 slice 4): a return
        // would have to name the lifetime of what it borrows, and v1 has no
        // lifetime syntax, exactly as for a `Ty::Ref` return.
        if ty.dyn_trait.is_some() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' cannot be a return type: a borrowed trait object points at a value the caller owns, and a return would have to name how long that borrow lives; take it as a parameter, or return an owned value",
                    ty.display()
                ),
                span: ty.span,
            });
        }
        let base = self.resolve(ty, false)?;
        if ty.amp {
            Ok(Ty::Ref {
                mutable: false,
                inner: Box::new(base),
            })
        } else {
            Ok(base)
        }
    }
}

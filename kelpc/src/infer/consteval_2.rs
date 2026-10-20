use super::*;

impl<'a> ConstEvaluator<'a> {
    /// Walks a block: its statements in order, one frame of locals, and its
    /// tail expression as the block's value.
    pub(crate) fn eval_block(
        &self,
        block: &ast::Block,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstFlow> {
        scope.frames.push(HashMap::new());
        let last = block.stmts.len().saturating_sub(1);
        let mut tail: Option<ConstResult> = None;
        for (i, stmt) in block.stmts.iter().enumerate() {
            match stmt {
                ast::Stmt::Let {
                    name,
                    mutable,
                    ty,
                    init,
                    ..
                } => {
                    let declared = match ty {
                        Some(t) => Some(
                            self.type_scope(&scope.prefix, &scope.aliases)
                                .resolve(t, false)?,
                        ),
                        None => None,
                    };
                    let value = self.eval(init, declared.as_ref(), scope)?;
                    if scope.halted() {
                        break;
                    }
                    if let Some(t) = &declared {
                        if &value.1 != t {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "'{name}' is declared as '{}', and its initializer is a '{}'",
                                    t.display(),
                                    value.1.display()
                                ),
                                span: init.span(),
                            });
                        }
                    }
                    scope.frames.last_mut().expect("frame pushed above").insert(
                        name.clone(),
                        ConstBinding {
                            value,
                            mutable: *mutable,
                        },
                    );
                }
                // A destructuring binding is not a constant expression: the
                // value it takes apart has no fold (there is no aggregate in
                // `ConstValue`), so the binding cannot have one either.
                ast::Stmt::LetTuple { span, .. } => {
                    return Err(self.unsupported(*span, "a destructuring binding"));
                }
                ast::Stmt::While { cond, body, .. } => loop {
                    let (value, ty) = self.eval(cond, Some(&Ty::Bool), scope)?;
                    match value {
                        ConstValue::Bool(true) => {}
                        ConstValue::Bool(false) => break,
                        _ => {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "the condition of a 'while' has to be a 'bool', and this one is a '{}'",
                                    ty.display()
                                ),
                                span: cond.span(),
                            })
                        }
                    }
                    self.tick(cond.span())?;
                    if scope.halted() {
                        break;
                    }
                    let _ = self.eval_block(body, None, scope)?;
                },
                ast::Stmt::Return { value, .. } => {
                    let ret = scope.ret_ty.clone();
                    let v = match value {
                        Some(e) => Some(self.eval(e, ret.as_ref(), scope)?),
                        None => None,
                    };
                    // The one place a `return` is recorded: from here on,
                    // `eval` and `eval_block` short-circuit until the call
                    // boundary reads this value as the function's result.
                    scope.returning = Some(v);
                    break;
                }
                ast::Stmt::Expr { expr, .. } => {
                    let v = self.eval(expr, if i == last { expected } else { None }, scope)?;
                    if i == last {
                        tail = Some(v);
                    }
                }
                // A `for` is a loop over a runtime value, which a constant
                // expression cannot fold (there is no loop in `ConstValue`).
                ast::Stmt::For { span, .. } => {
                    return Err(self.unsupported(*span, "a 'for' loop"));
                }
                ast::Stmt::Break { span } | ast::Stmt::Continue { span } => {
                    return Err(self.unsupported(*span, "a 'break' or 'continue'"));
                }
                // `defer` is lowered away before checking, so a constant
                // initializer never contains one.
                ast::Stmt::Defer { span, .. } | ast::Stmt::Errdefer { span, .. } => {
                    return Err(TypeError {
                        help: None,
                        message: "internal error: a 'defer' reached the constant folder; it should have been lowered before checking".to_string(),
                        span: *span,
                    })
                }
            }
            if scope.halted() {
                break;
            }
        }
        scope.frames.pop();
        if scope.halted() {
            // Cloned rather than taken: an outer block in the same body still
            // has to see the same value (and the same flag).
            return Ok(ConstFlow::Returned(scope.returning.clone().flatten()));
        }
        Ok(ConstFlow::Fell(tail))
    }
}

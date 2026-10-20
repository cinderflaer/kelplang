use super::*;

pub(crate) fn check_if(
    cond: &ast::Expr,
    then_block: &ast::Block,
    else_block: Option<&ast::ElseBranch>,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
    if cond.ty != Ty::Bool {
        return Err(TypeError {
            help: None,
            message: format!("if condition must be 'bool', found '{}'", cond.ty.display()),
            span,
        });
    }

    let then_t = check_block(then_block, ctx, expected)?;

    let else_t = match else_block {
        Some(ast::ElseBranch::Block(b)) => Some(check_block(b, ctx, expected)?),
        Some(ast::ElseBranch::If(e)) => {
            let checked = check_expr(e, ctx, expected)?;
            // The parser folds a plain `else if` into an `If`, but an
            // `else if let` desugars to the block `parse_if_let` builds (a
            // bound scrutinee and a pattern `when`), and that block is the
            // else branch's value exactly as an `If` is: wrap either one and
            // let the join below see a block. Assuming `If` here was an
            // internal error on every `else if let` a program wrote.
            Some(tir::Block {
                stmts: vec![],
                tail: Some(Box::new(checked)),
            })
        }
        None => None,
    };

    let ty = match &else_t {
        Some(e) => join_value_type(&then_t.ty(), &e.ty()).ok_or_else(|| TypeError {
            help: None,
            message: format!(
                "if/else branches have different types: '{}' and '{}'",
                then_t.ty().display(),
                e.ty().display()
            ),
            span,
        })?,
        None => {
            if then_t.ty() != Ty::Unit {
                return Err(TypeError {
                    help: None,
                    message: "if without an else branch must evaluate to 'unit'".to_string(),
                    span,
                });
            }
            Ty::Unit
        }
    };

    Ok(tir::Expr {
        kind: tir::ExprKind::If {
            cond: Box::new(cond),
            then_block: then_t,
            else_block: else_t,
        },
        ty,
        span,
    })
}

/// One gated item a module reaches, as the capability gate admitted it: the
/// module holds `domain` and names `callee` (a key of a gated module) at `span`.
/// `kelpc caps` reports these per module, and the `[dependencies] caps` check
/// refuses one whose domain the dependency's manifest entry does not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapUse {
    pub module: String,
    pub domain: String,
    pub callee: String,
    pub span: Span,
}

thread_local! {
    // The uses of the program being checked on this thread. `check_program`
    // runs on the compiler's one worker thread, so the list is the uses of the
    // one program the caller just checked, drained by `take_cap_uses`.
    pub(crate) static CAP_USES: std::cell::RefCell<Vec<CapUse>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn record_cap_use(module: &str, domain: &str, callee: &str, span: Span) {
    CAP_USES.with(|uses| {
        uses.borrow_mut().push(CapUse {
            module: module.to_string(),
            domain: domain.to_string(),
            callee: display_callee(callee).to_string(),
            span,
        })
    });
}

/// Drains the gated uses recorded by `check_program` on this thread, sorted and
/// without duplicates (a generic or overloaded body is checked more than once).
pub fn take_cap_uses() -> Vec<CapUse> {
    let mut uses = CAP_USES.with(|uses| std::mem::take(&mut *uses.borrow_mut()));
    let key = |u: &CapUse| (u.module.clone(), u.domain.clone(), u.callee.clone(), u.span.start, u.span.end);
    uses.sort_by_key(key);
    uses.dedup();
    uses
}






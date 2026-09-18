use std::collections::HashMap;

use crate::ast::{self, BinOp, UnOp};
use crate::lexer::Span;
use crate::tir;
use crate::types::{FnSig, Ty};

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

type Result<T> = std::result::Result<T, TypeError>;

struct Ctx {
    sigs: HashMap<String, FnSig>,
    ret_ty: Ty,
    scopes: Vec<HashMap<String, Ty>>,
}

impl Ctx {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Option<&Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }
}

const PRINTABLE_BUILTINS: &[&str] = &["println", "print"];

pub fn check_program(program: &ast::Program) -> Result<tir::Program> {
    let mut sigs = HashMap::new();
    for f in &program.functions {
        if sigs.contains_key(&f.name) {
            return Err(TypeError {
                message: format!("function '{}' is defined more than once", f.name),
                span: f.span,
            });
        }
        let mut params = Vec::new();
        for p in &f.params {
            params.push(resolve_type(&p.ty)?);
        }
        let ret = match &f.return_type {
            Some(t) => resolve_type(t)?,
            None => Ty::Unit,
        };
        sigs.insert(f.name.clone(), FnSig { params, ret });
    }

    if !sigs.contains_key("main") {
        return Err(TypeError {
            message: "no 'main' function found".to_string(),
            span: Span::new(0, 0),
        });
    }

    let mut functions = Vec::new();
    for f in &program.functions {
        functions.push(check_function(f, &sigs)?);
    }
    Ok(tir::Program { functions })
}

fn resolve_type(ty: &ast::TypeExpr) -> Result<Ty> {
    Ty::from_name(&ty.name).ok_or_else(|| TypeError {
        message: format!("unknown type '{}'", ty.name),
        span: ty.span,
    })
}

fn check_function(f: &ast::FunctionDecl, sigs: &HashMap<String, FnSig>) -> Result<tir::Function> {
    let sig = sigs.get(&f.name).unwrap().clone();
    let mut ctx = Ctx {
        sigs: sigs.clone(),
        ret_ty: sig.ret.clone(),
        scopes: vec![HashMap::new()],
    };
    let mut params = Vec::new();
    for (p, ty) in f.params.iter().zip(sig.params.iter()) {
        ctx.define(&p.name, ty.clone());
        params.push((p.name.clone(), ty.clone()));
    }

    let body = check_block(&f.body, &mut ctx, Some(&sig.ret))?;

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1), so a mismatch is only reported when a tail expression
    // is actually present and disagrees with the declared return type.
    if sig.ret != Ty::Unit && body.tail.is_some() && body.ty() != sig.ret {
        return Err(TypeError {
            message: format!(
                "function '{}' declared to return '{}' but body evaluates to '{}'",
                f.name,
                sig.ret.display(),
                body.ty().display()
            ),
            span: f.body.span,
        });
    }

    Ok(tir::Function {
        name: f.name.clone(),
        params,
        ret: sig.ret,
        body,
    })
}

fn check_block(block: &ast::Block, ctx: &mut Ctx, expected_tail: Option<&Ty>) -> Result<tir::Block> {
    ctx.push_scope();
    let mut stmts = Vec::new();
    let mut tail = None;

    for (i, stmt) in block.stmts.iter().enumerate() {
        let is_last = i + 1 == block.stmts.len();
        let is_tail = is_last
            && matches!(stmt, ast::Stmt::Expr { has_semi: false, .. });

        if is_tail {
            if let ast::Stmt::Expr { expr, .. } = stmt {
                tail = Some(Box::new(check_expr(expr, ctx, expected_tail)?));
            }
        } else {
            stmts.push(check_stmt(stmt, ctx)?);
        }
    }

    ctx.pop_scope();
    Ok(tir::Block { stmts, tail })
}

fn check_stmt(stmt: &ast::Stmt, ctx: &mut Ctx) -> Result<tir::Stmt> {
    match stmt {
        ast::Stmt::Let { name, ty, init, .. } => {
            let expected = match ty {
                Some(t) => Some(resolve_type(t)?),
                None => None,
            };
            let init = check_expr(init, ctx, expected.as_ref())?;
            let final_ty = expected.unwrap_or_else(|| init.ty.clone());
            if final_ty != init.ty {
                return Err(TypeError {
                    message: format!(
                        "cannot assign value of type '{}' to '{name}' declared as '{}'",
                        init.ty.display(),
                        final_ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            ctx.define(name, final_ty.clone());
            Ok(tir::Stmt::Let {
                name: name.clone(),
                ty: final_ty,
                init,
            })
        }
        ast::Stmt::Return { value, .. } => {
            let ret_ty = ctx.ret_ty.clone();
            let value = match value {
                Some(v) => Some(check_expr(v, ctx, Some(&ret_ty))?),
                None => None,
            };
            let actual = value.as_ref().map(|v| v.ty.clone()).unwrap_or(Ty::Unit);
            if actual != ret_ty {
                return Err(TypeError {
                    message: format!(
                        "return type mismatch: expected '{}', found '{}'",
                        ret_ty.display(),
                        actual.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(tir::Stmt::Return { value })
        }
        ast::Stmt::Expr { expr, .. } => Ok(tir::Stmt::Expr(check_expr(expr, ctx, None)?)),
    }
}

fn stmt_span(stmt: &ast::Stmt) -> Span {
    match stmt {
        ast::Stmt::Let { span, .. } => *span,
        ast::Stmt::Return { span, .. } => *span,
        ast::Stmt::Expr { span, .. } => *span,
    }
}

fn check_expr(expr: &ast::Expr, ctx: &mut Ctx, expected: Option<&Ty>) -> Result<tir::Expr> {
    match expr {
        ast::Expr::IntLit(v, _) => {
            let ty = match expected {
                Some(t) if t.is_integer() => t.clone(),
                _ => Ty::I32,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v),
                ty,
            })
        }
        ast::Expr::FloatLit(v, _) => {
            let ty = match expected {
                Some(t) if t.is_float() => t.clone(),
                _ => Ty::F64,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v),
                ty,
            })
        }
        ast::Expr::StringLit(s, _) => Ok(tir::Expr {
            kind: tir::ExprKind::StringLit(s.clone()),
            ty: Ty::Str,
        }),
        ast::Expr::BoolLit(b, _) => Ok(tir::Expr {
            kind: tir::ExprKind::BoolLit(*b),
            ty: Ty::Bool,
        }),
        ast::Expr::Ident(name, span) => {
            let ty = ctx.lookup(name).cloned().ok_or_else(|| TypeError {
                message: format!("undefined variable '{name}'"),
                span: *span,
            })?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Ident(name.clone()),
                ty,
            })
        }
        ast::Expr::Unary { op, expr: sub, span } => {
            let sub_expected = if *op == UnOp::Not { Some(&Ty::Bool) } else { expected };
            let sub_t = check_expr(sub, ctx, sub_expected)?;
            match op {
                UnOp::Neg => {
                    if !sub_t.ty.is_numeric() {
                        return Err(TypeError {
                            message: format!("cannot negate a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
                UnOp::Not => {
                    if sub_t.ty != Ty::Bool {
                        return Err(TypeError {
                            message: format!("cannot apply '!' to a value of type '{}'", sub_t.ty.display()),
                            span: *span,
                        });
                    }
                }
            }
            let ty = sub_t.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Unary {
                    op: *op,
                    expr: Box::new(sub_t),
                },
                ty,
            })
        }
        ast::Expr::Binary { op, lhs, rhs, span } => check_binary(*op, lhs, rhs, *span, ctx, expected),
        ast::Expr::Call { callee, args, span } => check_call(callee, args, *span, ctx),
        ast::Expr::If {
            cond,
            then_block,
            else_block,
            span,
        } => check_if(cond, then_block, else_block.as_deref(), *span, ctx, expected),
        ast::Expr::Block(block) => {
            let b = check_block(block, ctx, expected)?;
            let ty = b.ty();
            Ok(tir::Expr {
                kind: tir::ExprKind::Block(b),
                ty,
            })
        }
    }
}

fn check_binary(
    op: BinOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => {
            let numeric_expected = expected.filter(|t| t.is_numeric());
            let l = check_expr(lhs, ctx, numeric_expected)?;
            let r = check_expr(rhs, ctx, Some(&l.ty))?;
            if !l.ty.is_numeric() || l.ty != r.ty {
                return Err(TypeError {
                    message: format!(
                        "operator requires matching numeric operands, found '{}' and '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            let ty = l.ty.clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty,
            })
        }
        Lt | LtEq | Gt | GtEq | Eq | NotEq => {
            let l = check_expr(lhs, ctx, None)?;
            let r = check_expr(rhs, ctx, Some(&l.ty))?;
            if l.ty != r.ty {
                return Err(TypeError {
                    message: format!(
                        "cannot compare '{}' with '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty: Ty::Bool,
            })
        }
        And | Or => {
            let l = check_expr(lhs, ctx, Some(&Ty::Bool))?;
            let r = check_expr(rhs, ctx, Some(&Ty::Bool))?;
            if l.ty != Ty::Bool || r.ty != Ty::Bool {
                return Err(TypeError {
                    message: "logical operators require bool operands".to_string(),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
                ty: Ty::Bool,
            })
        }
    }
}

fn check_call(callee: &str, args: &[ast::Expr], span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if PRINTABLE_BUILTINS.contains(&callee) {
        if args.len() != 1 {
            return Err(TypeError {
                message: format!("'{callee}' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        let arg = check_expr(&args[0], ctx, None)?;
        if arg.ty == Ty::Unit {
            return Err(TypeError {
                message: format!("cannot pass a value of type 'unit' to '{callee}'"),
                span,
            });
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call {
                callee: callee.to_string(),
                args: vec![arg],
            },
            ty: Ty::Unit,
        });
    }

    let sig = ctx.sigs.get(callee).cloned().ok_or_else(|| TypeError {
        message: format!("call to undefined function '{callee}'"),
        span,
    })?;

    if args.len() != sig.params.len() {
        return Err(TypeError {
            message: format!(
                "'{callee}' expects {} argument(s), found {}",
                sig.params.len(),
                args.len()
            ),
            span,
        });
    }

    let mut checked_args = Vec::new();
    for (arg, param_ty) in args.iter().zip(sig.params.iter()) {
        let checked = check_expr(arg, ctx, Some(param_ty))?;
        if checked.ty != *param_ty {
            return Err(TypeError {
                message: format!(
                    "argument to '{callee}' has type '{}', expected '{}'",
                    checked.ty.display(),
                    param_ty.display()
                ),
                span: arg.span(),
            });
        }
        checked_args.push(checked);
    }

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: callee.to_string(),
            args: checked_args,
        },
        ty: sig.ret,
    })
}

fn check_if(
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
            message: format!("if condition must be 'bool', found '{}'", cond.ty.display()),
            span,
        });
    }

    let then_t = check_block(then_block, ctx, expected)?;

    let else_t = match else_block {
        Some(ast::ElseBranch::Block(b)) => Some(check_block(b, ctx, expected)?),
        Some(ast::ElseBranch::If(e)) => {
            let checked = check_expr(e, ctx, expected)?;
            match checked.kind {
                tir::ExprKind::If { .. } => Some(tir::Block {
                    stmts: vec![],
                    tail: Some(Box::new(checked)),
                }),
                _ => unreachable!("parser guarantees else-if parses to Expr::If"),
            }
        }
        None => None,
    };

    let ty = match &else_t {
        Some(e) => {
            if then_t.ty() != e.ty() {
                return Err(TypeError {
                    message: format!(
                        "if/else branches have different types: '{}' and '{}'",
                        then_t.ty().display(),
                        e.ty().display()
                    ),
                    span,
                });
            }
            then_t.ty()
        }
        None => {
            if then_t.ty() != Ty::Unit {
                return Err(TypeError {
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
    })
}

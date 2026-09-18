use std::collections::HashMap;

use crate::ast::{self, BinOp, UnOp};
use crate::lexer::Span;
use crate::tir;
use crate::types::{FnSig, StructDef, Ty};

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

type Result<T> = std::result::Result<T, TypeError>;

#[derive(Clone)]
struct VarInfo {
    ty: Ty,
    mutable: bool,
}

struct Ctx {
    sigs: HashMap<String, FnSig>,
    structs: HashMap<String, StructDef>,
    ret_ty: Ty,
    scopes: Vec<HashMap<String, VarInfo>>,
}

impl Ctx {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Ty, mutable: bool) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), VarInfo { ty, mutable });
    }

    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }
}

const PRINTABLE_BUILTINS: &[&str] = &["println", "print"];

pub fn check_program(program: &ast::Program) -> Result<tir::Program> {
    let mut structs = HashMap::new();
    for s in &program.structs {
        if structs.contains_key(&s.name) {
            return Err(TypeError {
                message: format!("struct '{}' is defined more than once", s.name),
                span: s.span,
            });
        }
        let mut fields = Vec::new();
        for f in &s.fields {
            if fields.iter().any(|(n, _): &(String, Ty)| n == &f.name) {
                return Err(TypeError {
                    message: format!("field '{}' is defined more than once in struct '{}'", f.name, s.name),
                    span: f.span,
                });
            }
            fields.push((f.name.clone(), resolve_type(&f.ty, &structs)?));
        }
        structs.insert(
            s.name.clone(),
            StructDef {
                name: s.name.clone(),
                fields,
            },
        );
    }

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
            params.push(resolve_type(&p.ty, &structs)?);
        }
        let ret = match &f.return_type {
            Some(t) => resolve_type(t, &structs)?,
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
        functions.push(check_function(f, &sigs, &structs)?);
    }
    Ok(tir::Program {
        structs: structs.into_values().collect(),
        functions,
    })
}

fn resolve_type(ty: &ast::TypeExpr, structs: &HashMap<String, StructDef>) -> Result<Ty> {
    if let Some(t) = Ty::from_name(&ty.name) {
        return Ok(t);
    }
    if structs.contains_key(&ty.name) {
        return Ok(Ty::Struct(ty.name.clone()));
    }
    Err(TypeError {
        message: format!("unknown type '{}'", ty.name),
        span: ty.span,
    })
}

fn check_function(
    f: &ast::FunctionDecl,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
) -> Result<tir::Function> {
    let sig = sigs.get(&f.name).unwrap().clone();
    let mut ctx = Ctx {
        sigs: sigs.clone(),
        structs: structs.clone(),
        ret_ty: sig.ret.clone(),
        scopes: vec![HashMap::new()],
    };
    let mut params = Vec::new();
    for (p, ty) in f.params.iter().zip(sig.params.iter()) {
        // Parameters are immutable bindings, like `let` without `mut`; v1 has
        // no syntax to declare a mutable parameter.
        ctx.define(&p.name, ty.clone(), false);
        params.push((p.name.clone(), ty.clone()));
    }

    let body = check_block(&f.body, &mut ctx, Some(&sig.ret))?;

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1/M2), so a mismatch is only reported when a tail
    // expression is actually present and disagrees with the declared return
    // type.
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
        ast::Stmt::Let {
            name,
            mutable,
            ty,
            init,
            ..
        } => {
            let expected = match ty {
                Some(t) => Some(resolve_type(t, &ctx.structs)?),
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
            ctx.define(name, final_ty.clone(), *mutable);
            Ok(tir::Stmt::Let {
                name: name.clone(),
                ty: final_ty,
                init,
            })
        }
        ast::Stmt::While { cond, body, span } => {
            let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
            if cond.ty != Ty::Bool {
                return Err(TypeError {
                    message: format!("while condition must be 'bool', found '{}'", cond.ty.display()),
                    span: *span,
                });
            }
            let body = check_block(body, ctx, None)?;
            Ok(tir::Stmt::While { cond, body })
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
        ast::Stmt::While { span, .. } => *span,
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
            let info = ctx.lookup(name).ok_or_else(|| TypeError {
                message: format!("undefined variable '{name}'"),
                span: *span,
            })?;
            Ok(tir::Expr {
                kind: tir::ExprKind::Ident(name.clone()),
                ty: info.ty.clone(),
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
        ast::Expr::FieldAccess { base, field, span } => {
            let base_t = check_expr(base, ctx, None)?;
            let Ty::Struct(sname) = &base_t.ty else {
                return Err(TypeError {
                    message: format!("cannot access field '{field}' on non-struct type '{}'", base_t.ty.display()),
                    span: *span,
                });
            };
            let sdef = ctx.structs.get(sname).expect("struct types are only produced for known structs");
            let field_ty = sdef.field_ty(field).cloned().ok_or_else(|| TypeError {
                message: format!("struct '{sname}' has no field '{field}'"),
                span: *span,
            })?;
            Ok(tir::Expr {
                kind: tir::ExprKind::FieldAccess {
                    base: Box::new(base_t),
                    field: field.clone(),
                },
                ty: field_ty,
            })
        }
        ast::Expr::StructLit { name, fields, span } => {
            let sdef = ctx.structs.get(name).cloned().ok_or_else(|| TypeError {
                message: format!("unknown struct '{name}'"),
                span: *span,
            })?;
            if fields.len() != sdef.fields.len() {
                return Err(TypeError {
                    message: format!(
                        "struct '{name}' has {} field(s), but {} were provided",
                        sdef.fields.len(),
                        fields.len()
                    ),
                    span: *span,
                });
            }
            let mut seen: Vec<&str> = Vec::new();
            let mut checked_fields = Vec::new();
            for (fname, fval) in fields {
                if seen.contains(&fname.as_str()) {
                    return Err(TypeError {
                        message: format!("field '{fname}' specified more than once"),
                        span: *span,
                    });
                }
                seen.push(fname.as_str());
                let field_ty = sdef.field_ty(fname).cloned().ok_or_else(|| TypeError {
                    message: format!("struct '{name}' has no field '{fname}'"),
                    span: *span,
                })?;
                let checked = check_expr(fval, ctx, Some(&field_ty))?;
                if checked.ty != field_ty {
                    return Err(TypeError {
                        message: format!(
                            "field '{fname}' has type '{}', expected '{}'",
                            checked.ty.display(),
                            field_ty.display()
                        ),
                        span: fval.span(),
                    });
                }
                checked_fields.push((fname.clone(), checked));
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::StructLit {
                    name: name.clone(),
                    fields: checked_fields,
                },
                ty: Ty::Struct(name.clone()),
            })
        }
        ast::Expr::Assign { target, value, span } => check_assign(target, value, *span, ctx),
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

/// Walks a `place` expression (an `Ident`, or a chain of `.field` accesses
/// rooted in one) down to its root identifier. Mutability in v1 is a
/// property of the root binding only — there's no separate per-field
/// mutability, so `p.x = 1` is allowed exactly when `p` itself was bound
/// `mut`.
fn root_ident(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Ident(name, _) => Some(name),
        ast::Expr::FieldAccess { base, .. } => root_ident(base),
        _ => None,
    }
}

fn check_assign(target: &ast::Expr, value: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if !matches!(target, ast::Expr::Ident(..) | ast::Expr::FieldAccess { .. }) {
        return Err(TypeError {
            message: "invalid assignment target".to_string(),
            span,
        });
    }
    let Some(root) = root_ident(target) else {
        return Err(TypeError {
            message: "invalid assignment target".to_string(),
            span,
        });
    };
    let info = ctx.lookup(root).ok_or_else(|| TypeError {
        message: format!("undefined variable '{root}'"),
        span,
    })?;
    if !info.mutable {
        return Err(TypeError {
            message: format!("cannot assign to '{root}': not declared 'mut'"),
            span,
        });
    }

    let target_t = check_expr(target, ctx, None)?;
    let value_t = check_expr(value, ctx, Some(&target_t.ty))?;
    if value_t.ty != target_t.ty {
        return Err(TypeError {
            message: format!(
                "cannot assign value of type '{}' to target of type '{}'",
                value_t.ty.display(),
                target_t.ty.display()
            ),
            span,
        });
    }

    Ok(tir::Expr {
        kind: tir::ExprKind::Assign {
            target: Box::new(target_t),
            value: Box::new(value_t),
        },
        ty: Ty::Unit,
    })
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
        let printable = arg.ty.is_numeric()
            || matches!(arg.ty, Ty::Bool | Ty::Char | Ty::Str);
        if !printable {
            return Err(TypeError {
                message: format!("cannot pass a value of type '{}' to '{callee}'", arg.ty.display()),
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

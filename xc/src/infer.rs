use std::collections::HashMap;

use crate::ast::{self, BinOp, UnOp};
use crate::lexer::Span;
use crate::tir;
use crate::types::{FieldDef, FnSig, StructDef, Ty};

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
    /// Set while checking a receiver-form function whose receiver resolves
    /// to a `datac`: lets a bare identifier that isn't a local fall back to
    /// an implicit `self.field` (`fun Circle.area().R: f64 = 3.14159 * r *
    /// r` — no `self.` qualifier written, per `syntax&shii.md` §5's "no
    /// `self` keyword").
    receiver_struct: Option<String>,
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

    /// Looks up `name` as a field of the current receiver's `datac`, if
    /// this function has one.
    fn receiver_field_ty(&self, name: &str) -> Option<Ty> {
        let sname = self.receiver_struct.as_deref()?;
        self.structs.get(sname)?.field(name).map(|f| f.ty.clone())
    }
}

/// A function's lookup key in `sigs`/`tir::Function.name`: a receiver-form
/// method is keyed `Type#name` (a receiver method and a free function can
/// share a bare name with no collision, and `#` can't appear in a real
/// identifier so it can't collide with a hand-written name either) — a
/// plain function is keyed by its bare name, unchanged from before
/// receivers existed.
fn sig_key(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}#{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

const PRINTABLE_BUILTINS: &[&str] = &["print", "print!"];

pub fn check_program(program: &ast::Program) -> Result<tir::Program> {
    let mut structs = HashMap::new();
    for d in &program.datacs {
        if structs.contains_key(&d.name) {
            return Err(TypeError {
                message: format!("datac '{}' is defined more than once", d.name),
                span: d.span,
            });
        }
        let mut fields = Vec::new();
        for f in &d.fields {
            if fields.iter().any(|fd: &FieldDef| fd.name == f.name) {
                return Err(TypeError {
                    message: format!("field '{}' is defined more than once in datac '{}'", f.name, d.name),
                    span: f.span,
                });
            }
            fields.push(FieldDef {
                name: f.name.clone(),
                ty: resolve_type(&f.ty, &structs)?,
                mutable: f.mutable,
            });
        }
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields,
            },
        );
    }

    let mut sigs = HashMap::new();
    for f in &program.functions {
        let key = sig_key(f);
        if sigs.contains_key(&key) {
            let message = match &f.receiver {
                Some(r) => format!("method '{}.{}' is defined more than once", r.type_name, f.name),
                None => format!("function '{}' is defined more than once", f.name),
            };
            return Err(TypeError { message, span: f.span });
        }
        // A receiver method's key already can't collide with a struct name
        // (it always contains `#`), so this check only applies to plain
        // functions.
        if f.receiver.is_none() && structs.contains_key(&f.name) {
            return Err(TypeError {
                message: format!("'{}' is used for both a function and a datac", f.name),
                span: f.span,
            });
        }
        let mut params = Vec::new();
        if let Some(r) = &f.receiver {
            params.push(("self".to_string(), resolve_receiver_type(r, &structs)?));
        }
        for p in &f.params {
            params.push((p.name.clone(), resolve_param_type(p, &structs)?));
        }
        // `.R: T` never goes through `resolve_param_type`, so a function's
        // return type is always a plain owned `Ty` — never `Ty::Ref`. That's
        // what makes the borrow checker's call-scoped release sound: no
        // surface syntax exists yet for a function to hand a borrow back to
        // its caller, so a borrow can never escape the call frame that took
        // it.
        let ret = match &f.return_type {
            Some(t) => resolve_type(t, &structs)?,
            None => Ty::Unit,
        };
        sigs.insert(key, FnSig { params, ret });
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

/// Resolves a parameter's type per `syntax&shii.md` §5: a `datac` param is
/// an implicit shared borrow unless `mut`-marked (exclusive); everything
/// else (Copy types) stays a plain by-value type unless explicitly
/// `mut`-marked, since borrowing a Copy scalar by default would add
/// indirection nobody asked for. There is no move-taking parameter spelling
/// in v1 — a `datac` value is only ever moved via local rebinding
/// (`val q = p`), which M3 already checks.
fn resolve_param_type(p: &ast::Param, structs: &HashMap<String, StructDef>) -> Result<Ty> {
    let base = resolve_type(&p.ty, structs)?;
    Ok(implicit_borrow_wrap(base, p.mutable))
}

/// Same rule as `resolve_param_type`, for a receiver (`Type` before the
/// `.` in `fun [mut] Type.name(...)`).
fn resolve_receiver_type(r: &ast::Receiver, structs: &HashMap<String, StructDef>) -> Result<Ty> {
    let te = ast::TypeExpr { name: r.type_name.clone(), span: r.span };
    let base = resolve_type(&te, structs)?;
    Ok(implicit_borrow_wrap(base, r.mutable))
}

fn implicit_borrow_wrap(base: Ty, mut_marked: bool) -> Ty {
    if mut_marked {
        Ty::Ref { mutable: true, inner: Box::new(base) }
    } else if matches!(base, Ty::Struct(_)) {
        Ty::Ref { mutable: false, inner: Box::new(base) }
    } else {
        base
    }
}

/// Unwraps a (possibly borrowed) type down to the `datac` name it names, if
/// any — used by field access/assignment so `.field` auto-derefs through a
/// `Ref` base exactly like an owned struct.
fn struct_name_of(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(s) => Some(s),
        Ty::Ref { inner, .. } => struct_name_of(inner),
        _ => None,
    }
}

/// A "place": something with stable storage that can be borrowed or
/// assigned into — an `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`).
fn is_tir_place(expr: &tir::Expr) -> bool {
    matches!(
        expr.kind,
        tir::ExprKind::Ident(_) | tir::ExprKind::FieldAccess { .. } | tir::ExprKind::Deref { .. }
    )
}

fn is_ast_place(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(..) | ast::Expr::FieldAccess { .. } | ast::Expr::Deref { .. })
}

/// Whether a checked place-expression's storage is currently writable:
/// either its root is a plain owned `var`-bound local, or its root (or the
/// pointer stepped through by a `*`) is statically typed as an *exclusive*
/// `Ty::Ref`. Used both by assignment (`p.x = ...`, `*p = ...`) and by
/// `&mut expr` (taking an exclusive borrow requires the place already be
/// writable).
fn check_target_writable(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &target.kind {
        tir::ExprKind::Ident(name) => {
            let info = ctx.lookup(name).expect("root identifier was already type-checked");
            match &info.ty {
                Ty::Ref { mutable, .. } => {
                    if *mutable {
                        Ok(())
                    } else {
                        Err(TypeError {
                            message: format!("cannot write to '{name}': it is a shared borrow, not 'mut'"),
                            span: target.span,
                        })
                    }
                }
                _ => {
                    if info.mutable {
                        Ok(())
                    } else {
                        Err(TypeError {
                            message: format!("cannot assign to '{name}': not declared 'var'"),
                            span: target.span,
                        })
                    }
                }
            }
        }
        tir::ExprKind::FieldAccess { base, .. } => check_target_writable(base, ctx),
        tir::ExprKind::Deref { expr: inner } => match &inner.ty {
            Ty::Ref { mutable: true, .. } => Ok(()),
            Ty::Ref { mutable: false, .. } => Err(TypeError {
                message: "cannot write through a shared borrow".to_string(),
                span: target.span,
            }),
            _ => unreachable!("a Deref's operand is always Ref-typed, guaranteed by check_expr"),
        },
        other => unreachable!("not a place expression: {other:?}"),
    }
}

/// `Type.name` for a receiver method, `name` for a plain function — for
/// diagnostics only (`sig_key` is the lookup key; this is what a person
/// reads).
fn display_name(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}.{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

fn check_function(
    f: &ast::FunctionDecl,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
) -> Result<tir::Function> {
    let key = sig_key(f);
    let sig = sigs.get(&key).unwrap().clone();
    let receiver_struct = f
        .receiver
        .as_ref()
        .and_then(|_| struct_name_of(&sig.params[0].1).map(|s| s.to_string()));
    let mut ctx = Ctx {
        sigs: sigs.clone(),
        structs: structs.clone(),
        ret_ty: sig.ret.clone(),
        scopes: vec![HashMap::new()],
        receiver_struct,
    };
    let mut params = Vec::new();
    // Parameters (and the receiver, if any) are immutable bindings, like
    // `val`; v1 has no syntax to declare a mutable *binding* for one (the
    // borrow itself can still be exclusive — that's `Ty::Ref { mutable:
    // true, .. }`, a property of the type, not the binding).
    if f.receiver.is_some() {
        let (pname, ty) = &sig.params[0];
        ctx.define(pname, ty.clone(), false);
        params.push((pname.clone(), ty.clone()));
    }
    let declared = if f.receiver.is_some() { &sig.params[1..] } else { &sig.params[..] };
    for (p, (_, ty)) in f.params.iter().zip(declared.iter()) {
        ctx.define(&p.name, ty.clone(), false);
        params.push((p.name.clone(), ty.clone()));
    }

    let body = match &f.body {
        ast::FunctionBody::Block(b) => check_block(b, &mut ctx, Some(&sig.ret))?,
        ast::FunctionBody::Expr(e) => {
            let checked = check_expr(e, &mut ctx, Some(&sig.ret))?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(checked)),
            }
        }
    };

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
                display_name(f),
                sig.ret.display(),
                body.ty().display()
            ),
            span: f.span,
        });
    }

    Ok(tir::Function {
        name: key,
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
        // Statements are newline-terminated now (no `;`), so "textually
        // last in the block" is the only signal for tail-expression status.
        let is_tail = is_last && matches!(stmt, ast::Stmt::Expr { .. });

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
        ast::Expr::IntLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_integer() => t.clone(),
                _ => Ty::I32,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::IntLit(*v),
                ty,
                span: *span,
            })
        }
        ast::Expr::FloatLit(v, span) => {
            let ty = match expected {
                Some(t) if t.is_float() => t.clone(),
                _ => Ty::F64,
            };
            Ok(tir::Expr {
                kind: tir::ExprKind::FloatLit(*v),
                ty,
                span: *span,
            })
        }
        ast::Expr::StringLit(s, span) => Ok(tir::Expr {
            kind: tir::ExprKind::StringLit(s.clone()),
            ty: Ty::Str,
            span: *span,
        }),
        ast::Expr::BoolLit(b, span) => Ok(tir::Expr {
            kind: tir::ExprKind::BoolLit(*b),
            ty: Ty::Bool,
            span: *span,
        }),
        ast::Expr::Ident(name, span) => {
            if let Some(info) = ctx.lookup(name) {
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Ident(name.clone()),
                    ty: info.ty.clone(),
                    span: *span,
                });
            }
            // Not a local or parameter — inside a receiver-form method,
            // fall back to the receiver's field of that name (no `self.`
            // qualifier is ever written; see `syntax&shii.md` §5).
            if let Some(field_ty) = ctx.receiver_field_ty(name) {
                let self_ty = ctx.lookup("self").expect("receiver binding exists whenever receiver_struct is set").ty.clone();
                let base = tir::Expr { kind: tir::ExprKind::Ident("self".to_string()), ty: self_ty, span: *span };
                return Ok(tir::Expr {
                    kind: tir::ExprKind::FieldAccess { base: Box::new(base), field: name.clone() },
                    ty: field_ty,
                    span: *span,
                });
            }
            Err(TypeError {
                message: format!("undefined variable '{name}'"),
                span: *span,
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
                span: *span,
            })
        }
        ast::Expr::Binary { op, lhs, rhs, span } => check_binary(*op, lhs, rhs, *span, ctx, expected),
        ast::Expr::Call { callee, args, span } => check_call(callee, args, *span, ctx),
        ast::Expr::FieldAccess { base, field, span } => {
            let base_t = check_expr(base, ctx, None)?;
            let Some(sname) = struct_name_of(&base_t.ty).map(|s| s.to_string()) else {
                return Err(TypeError {
                    message: format!("cannot access field '{field}' on non-struct type '{}'", base_t.ty.display()),
                    span: *span,
                });
            };
            let sdef = ctx.structs.get(&sname).expect("struct types are only produced for known structs");
            let field_ty = sdef.field(field).map(|f| f.ty.clone()).ok_or_else(|| TypeError {
                message: format!("datac '{sname}' has no field '{field}'"),
                span: *span,
            })?;
            Ok(tir::Expr {
                kind: tir::ExprKind::FieldAccess {
                    base: Box::new(base_t),
                    field: field.clone(),
                },
                ty: field_ty,
                span: *span,
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
                span: block.span,
            })
        }
        ast::Expr::Ref { mutable, expr: sub, span } => {
            if !is_ast_place(sub) {
                return Err(TypeError {
                    message: "cannot borrow a temporary value".to_string(),
                    span: *span,
                });
            }
            let sub_t = check_expr(sub, ctx, None)?;
            if *mutable {
                check_target_writable(&sub_t, ctx)?;
            }
            let ty = Ty::Ref { mutable: *mutable, inner: Box::new(sub_t.ty.clone()) };
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::Deref { expr: sub, span } => {
            let sub_t = check_expr(sub, ctx, None)?;
            let Ty::Ref { inner, .. } = &sub_t.ty else {
                return Err(TypeError {
                    message: format!("cannot dereference a value of type '{}'", sub_t.ty.display()),
                    span: *span,
                });
            };
            let ty = inner.as_ref().clone();
            Ok(tir::Expr {
                kind: tir::ExprKind::Deref { expr: Box::new(sub_t) },
                ty,
                span: *span,
            })
        }
        ast::Expr::MethodCall { receiver, method, args, span } => check_method_call(receiver, method, args, *span, ctx),
    }
}

/// Assignment targets: `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`) — anything `is_ast_place`
/// accepts.
fn check_assign(target: &ast::Expr, value: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if !is_ast_place(target) {
        return Err(TypeError {
            message: "invalid assignment target".to_string(),
            span,
        });
    }

    let target_t = check_expr(target, ctx, None)?;
    check_target_writable(&target_t, ctx)?;

    // For a field target, writing it also requires the field itself be
    // declared `var` in its `datac` — a `val` field is never reassignable,
    // regardless of whether it's reached through a `var` local or an
    // exclusive borrow. Only the *immediate* field being assigned needs to
    // be `var`; fields merely read through on the way there do not.
    if let tir::ExprKind::FieldAccess { base, field } = &target_t.kind {
        let sname = struct_name_of(&base.ty).expect("field access base is always a struct type, guaranteed by check_expr");
        let sdef = ctx.structs.get(sname).expect("struct types are only produced for known structs");
        let fdef = sdef.field(field).expect("field already resolved by check_expr");
        if !fdef.mutable {
            return Err(TypeError {
                message: format!("cannot assign to '{field}': declared 'val'"),
                span: target_t.span,
            });
        }
    }

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
        span,
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
                span,
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
                span,
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
                span,
            })
        }
    }
}

/// Matches call-site arguments (each optionally named, `f(x = 1)`) against
/// a callee's declared parameter list, by name first and positionally for
/// the rest — shared between plain function calls and `datac` construction,
/// since both look identical at a call site.
fn match_args(
    what: &str,
    params: &[(String, Ty)],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Vec<tir::Expr>> {
    if args.len() != params.len() {
        return Err(TypeError {
            message: format!("{what} expects {} argument(s), found {}", params.len(), args.len()),
            span,
        });
    }

    let mut named: HashMap<&str, &ast::CallArg> = HashMap::new();
    for a in args {
        if let Some(n) = &a.name {
            if named.insert(n.as_str(), a).is_some() {
                return Err(TypeError {
                    message: format!("argument '{n}' specified more than once"),
                    span,
                });
            }
        }
    }

    let mut positional = args.iter().filter(|a| a.name.is_none());
    let mut result = Vec::with_capacity(params.len());
    for (pname, pty) in params {
        let arg = match named.remove(pname.as_str()) {
            Some(a) => a,
            None => positional.next().ok_or_else(|| TypeError {
                message: format!("{what} is missing argument '{pname}'"),
                span,
            })?,
        };
        result.push(check_call_arg(pname, pty, &arg.value, ctx)?);
    }

    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }

    Ok(result)
}

/// Checks one call-site argument against its matched parameter type. When
/// the parameter is `Ty::Ref`, the caller never writes `&` — a plain place
/// argument (`p`, `p.x`) is auto-borrowed here (the callee's signature is
/// what decides shared vs exclusive), matching `syntax&shii.md`'s "shared
/// borrow is implicit — write nothing." An argument that's already a borrow
/// (an explicit `&expr`, or forwarding an existing `Ref`-typed parameter)
/// type-matches directly instead of double-wrapping; a shared parameter
/// accepts an exclusive argument (implicit downgrade), never the reverse.
fn check_call_arg(pname: &str, pty: &Ty, arg_expr: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let Ty::Ref { mutable, inner } = pty else {
        let checked = check_expr(arg_expr, ctx, Some(pty))?;
        if checked.ty != *pty {
            return Err(TypeError {
                message: format!(
                    "argument '{pname}' has type '{}', expected '{}'",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    };

    let checked = check_expr(arg_expr, ctx, Some(inner))?;
    match &checked.ty {
        t if t == inner.as_ref() => {
            if !is_tir_place(&checked) {
                return Err(TypeError {
                    message: format!("argument '{pname}' must be a variable or field, not a temporary value"),
                    span: arg_expr.span(),
                });
            }
            if *mutable {
                check_target_writable(&checked, ctx)?;
            }
            let span = checked.span;
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(checked) },
                ty: pty.clone(),
                span,
            })
        }
        Ty::Ref { mutable: a_mut, inner: a_inner } if a_inner == inner => {
            if *mutable && !a_mut {
                return Err(TypeError {
                    message: format!("argument '{pname}' requires an exclusive borrow, found a shared one"),
                    span: arg_expr.span(),
                });
            }
            Ok(checked)
        }
        other => Err(TypeError {
            message: format!("argument '{pname}' has type '{}', expected '{}'", other.display(), pty.display()),
            span: arg_expr.span(),
        }),
    }
}

fn check_call(callee: &str, args: &[ast::CallArg], span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if PRINTABLE_BUILTINS.contains(&callee) {
        if args.len() != 1 {
            return Err(TypeError {
                message: format!("'{callee}' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        }
        if args[0].name.is_some() {
            return Err(TypeError {
                message: format!("'{callee}' does not take named arguments"),
                span,
            });
        }
        let arg = check_expr(&args[0].value, ctx, None)?;
        let printable = arg.ty.is_numeric() || matches!(arg.ty, Ty::Bool | Ty::Char | Ty::Str);
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
            span,
        });
    }

    if let Some(sdef) = ctx.structs.get(callee).cloned() {
        let params: Vec<(String, Ty)> = sdef.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
        let checked = match_args(&format!("datac '{callee}'"), &params, args, span, ctx)?;
        let fields = sdef
            .fields
            .iter()
            .zip(checked)
            .map(|(f, e)| (f.name.clone(), e))
            .collect();
        return Ok(tir::Expr {
            kind: tir::ExprKind::StructLit {
                name: callee.to_string(),
                fields,
            },
            ty: Ty::Struct(callee.to_string()),
            span,
        });
    }

    let sig = ctx.sigs.get(callee).cloned().ok_or_else(|| TypeError {
        message: format!("call to undefined function '{callee}'"),
        span,
    })?;
    let checked = match_args(&format!("function '{callee}'"), &sig.params, args, span, ctx)?;

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: callee.to_string(),
            args: checked,
        },
        ty: sig.ret,
        span,
    })
}

/// `receiver.method(args)`: resolves `method` against the receiver
/// expression's `datac` type, then lowers to a plain `tir::ExprKind::Call`
/// — the receiver becomes the first argument, auto-borrowed through
/// `check_call_arg` exactly like any other `Ty::Ref` parameter (so a
/// `fun mut T.m()` method reuses M4's aliasing checks with no special
/// casing). No new `tir`/codegen shape is needed for method calls at all.
fn check_method_call(receiver: &ast::Expr, method: &str, args: &[ast::CallArg], span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    // Type-only probe to learn the receiver's struct name; re-checked for
    // real (with its actual borrow/move effects) via `check_call_arg`
    // below. `check_expr` has no persistent side effects beyond reading
    // `ctx`, so checking the same expression twice here is safe.
    let probe = check_expr(receiver, ctx, None)?;
    let Some(sname) = struct_name_of(&probe.ty).map(|s| s.to_string()) else {
        return Err(TypeError {
            message: format!("cannot call method '{method}' on non-struct type '{}'", probe.ty.display()),
            span,
        });
    };
    let key = format!("{sname}#{method}");
    let sig = ctx.sigs.get(&key).cloned().ok_or_else(|| TypeError {
        message: format!("datac '{sname}' has no method '{method}'"),
        span,
    })?;

    let mut checked_args = Vec::with_capacity(sig.params.len());
    checked_args.push(check_call_arg("self", &sig.params[0].1, receiver, ctx)?);
    checked_args.extend(match_args(&format!("method '{sname}.{method}'"), &sig.params[1..], args, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked_args },
        ty: sig.ret,
        span,
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
        span,
    })
}

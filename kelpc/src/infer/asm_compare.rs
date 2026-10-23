use super::*;

/// `asm(...)` (kernel stage 5). The construct is an unsafe operation, so on top
/// of the module's `[unsafe]` listing (`ast::first_unsafe`) the use site has to
/// be inside `unsafe { ... }` or an `unsafe fun`, exactly as a raw pointer write
/// does. The operand types are machine words; the constraint names are passed to
/// the backend unchanged.
pub(crate) fn check_asm(
    template: &str,
    operands: &[ast::AsmOperand],
    clobbers: &[String],
    nomem: bool,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if ctx.unsafe_depth == 0 {
        return Err(TypeError {
            help: None,
            message: "'asm' is an unsafe operation: it can only be used inside 'unsafe { ... }' or in a function declared 'unsafe fun'".to_string(),
            span,
        });
    }
    let mut checked: Vec<tir::AsmOperand> = Vec::new();
    let mut value_out: Option<usize> = None;
    let mut value_ty: Option<Ty> = None;
    for (i, op) in operands.iter().enumerate() {
        let operand = match op.kind {
            ast::AsmOperandKind::In => {
                let e = op
                    .operand
                    .as_ref()
                    .expect("the parser requires an operand for 'in'");
                let v = check_expr(e, ctx, None)?;
                require_asm_operand_ty(&v.ty, op.span)?;
                Some(v)
            }
            ast::AsmOperandKind::Out | ast::AsmOperandKind::InOut => match &op.operand {
                Some(e) => {
                    if !is_ast_place(e) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "a '{}' operand must be a place: the asm writes it back, so an arbitrary expression has nowhere to be written",
                                asm_kind_word(op.kind)
                            ),
                            span: op.span,
                        });
                    }
                    let p = check_expr(e, ctx, None)?;
                    check_written_place(&p, ctx)?;
                    require_asm_operand_ty(&p.ty, op.span)?;
                    Some(p)
                }
                None => {
                    if value_out.is_some() {
                        return Err(TypeError {
                            help: None,
                            message: "an 'asm' expression has at most one anonymous output: name the others with a place ('out(\"r\") x')".to_string(),
                            span: op.span,
                        });
                    }
                    // An output is a machine word, so `i64` answers when the
                    // context names no type; a written type wins.
                    let ty = expected.cloned().unwrap_or(Ty::I64);
                    require_asm_operand_ty(&ty, op.span)?;
                    value_out = Some(i);
                    value_ty = Some(ty);
                    None
                }
            },
        };
        checked.push(tir::AsmOperand {
            kind: op.kind,
            constraint: op.constraint.clone(),
            operand,
        });
    }
    // A `{n}` that names no operand would be left in the template verbatim and
    // read by the assembler as a brace, so it is refused here where the operands
    // are known.
    for n in asm_placeholders(template) {
        if n >= operands.len() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the asm template refers to operand {{{n}}}, but the asm has {} operand{}",
                    operands.len(),
                    if operands.len() == 1 { "" } else { "s" }
                ),
                span,
            });
        }
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Asm(tir::AsmBlock {
            template: template.to_string(),
            operands: checked,
            clobbers: clobbers.to_vec(),
            nomem,
            value_out,
        }),
        ty: value_ty.unwrap_or(Ty::Unit),
        span,
    })
}

/// Every `{n}` placeholder in an asm template, in the order they appear.
pub(crate) fn asm_placeholders(template: &str) -> Vec<usize> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == '}') {
                if let Ok(n) = chars[i + 1..i + 1 + close]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                {
                    out.push(n);
                    i += close + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `in`, `out` or `inout`, for a diagnostic.
pub(crate) fn asm_kind_word(kind: ast::AsmOperandKind) -> &'static str {    match kind {
        ast::AsmOperandKind::In => "in",
        ast::AsmOperandKind::Out => "out",
        ast::AsmOperandKind::InOut => "inout",
    }
}

/// An `asm` operand is one machine value: an integer, `bool`, `char`, `f32`/
/// `f64`, a `Ptr<T>` or an `extern type` handle. Everything else (an owned
/// buffer, a `datac`, an array, a borrow) has no single-register form, so it is
/// refused by name rather than laid out implicitly.
pub(crate) fn require_asm_operand_ty(ty: &Ty, span: Span) -> Result<()> {
    if ty.is_numeric() || matches!(ty, Ty::Bool | Ty::Char | Ty::Ptr(_) | Ty::Handle(_)) {
        Ok(())
    } else {
        Err(TypeError {
            help: None,
            message: format!(
                "'{}' cannot be an asm operand: an operand is one machine value (an integer, 'bool', 'char', 'f32'/'f64', a 'Ptr' or an 'extern type' handle)",
                ty.display()
            ),
            span,
        })
    }
}

pub(crate) fn check_assign(target: &ast::Expr, value: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    if !is_ast_place(target) {
        return Err(TypeError {
            help: None,
            message: "invalid assignment target".to_string(),
            span,
        });
    }

    let target_t = check_expr(target, ctx, None)?;
    if matches!(value, ast::Expr::Ident(..)) {
        if let Some(name) = place_root_name(target) {
            ctx.box_sink_ok = Some(BoxSink::Store(name));
        }
    }
    check_written_place(&target_t, ctx)?;

    let value_t = check_expr(value, ctx, Some(&target_t.ty))?;
    if value_t.ty != target_t.ty {
        return Err(TypeError {
            help: None,
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

/// Whether a comparison operator (`==`/`!=`/`<`/…) can be applied to this
/// type at all. Comparisons are only generated for scalars C can compare
/// directly: an owned value, a `datac`, or a `str` has no comparison of its
/// own, and emitting one produces invalid C at `cc` (or, for `str`, a
/// comparison of *addresses* rather than text - silently wrong, which is
/// worse). References are allowed *except* `&str`, which is the same address
/// comparison in disguise.
pub(crate) fn comparable(ty: &Ty) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char)
        || (matches!(ty, Ty::Ref { .. }) && !is_str_like(ty))
}
pub(crate) fn comparison_error(ty: &Ty, op: BinOp) -> String {
    let sym = match op {
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        _ => unreachable!("not a comparison operator"),
    };
    if is_str_like(ty) {
        return format!(
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text - use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        );
    }
    match ty {
        Ty::Str => format!(
            "cannot compare 'str' values with '{sym}': a 'str' is a borrowed pointer, so this would compare addresses rather than text - use 'str.eq', or compare bytes with 'str.len'/'str.byte'"
        ),
        // A `View` *could* compare byte-wise (it has a length), but `==` on a
        // non-scalar is not how any other type reads and `view.eq` says what
        // it does.
        Ty::View => format!(
            "cannot compare 'View' values with '{sym}': use 'view.eq(other)' - it compares the bytes and the lengths, where '{sym}' would compare the pointers"
        ),
        Ty::Bytes | Ty::String => format!(
            "cannot compare '{}' values with '{}': v1 has no equality for owned values",
            ty.display(),
            sym
        ),
        Ty::Struct(_) => format!(
            "cannot compare '{}' values with '{}': a datac has no generated equality",
            ty.display(),
            sym
        ),
        other => format!("cannot compare values of type '{}'", other.display()),
    }
}

/// The owned type an equality operand names: a `datac`/`enum` value, or a
/// borrow of one. `Ty::Struct` covers both a `datac` and an `enum`, and a
/// `Ty::Ref` is looked through because an operand may be written `a` or `*a`
/// or be a `mut T` parameter, and all of them are one pointer to the backend.
pub(crate) fn equality_owner(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name) => Some(name.as_str()),
        Ty::Ref { inner, .. } => match inner.as_ref() {
            Ty::Struct(name) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// One operand of a lowered `==`/`!=` where the declared `eq` wants `&T`: an
/// owned operand is borrowed (`Ref`), and an operand that is already a borrow
/// is handed on unchanged, so neither is ever moved.
pub(crate) fn equality_borrow(expr: &tir::Expr, want: &Ty) -> tir::Expr {
    match &expr.ty {
        Ty::Ref { .. } => expr.clone(),
        _ => tir::Expr {
            kind: tir::ExprKind::Ref {
                mutable: false,
                expr: Box::new(expr.clone()),
            },
            ty: want.clone(),
            span: expr.span,
        },
    }
}

/// One operand of a lowered `View == View`/`View == str`: a `View` is used as
/// it is, a `str` is viewed first (`str#view`, the builtin `s.view()`), since
/// `View.eq` takes a `View`.
pub(crate) fn view_operand(expr: &tir::Expr) -> tir::Expr {
    if matches!(expr.ty, Ty::View) {
        expr.clone()
    } else {
        tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "str#view".to_string(),
                args: vec![expr.clone()],
            },
            ty: Ty::View,
            span: expr.span,
        }
    }
}

/// `!=` is the lowering's result negated; `==` is the call itself.
pub(crate) fn negate_comparison(op: BinOp, call: tir::Expr, span: Span) -> tir::Expr {
    if matches!(op, BinOp::NotEq) {
        tir::Expr {
            kind: tir::ExprKind::Unary {
                op: ast::UnOp::Not,
                expr: Box::new(call),
            },
            ty: Ty::Bool,
            span,
        }
    } else {
        call
    }
}

/// H-5: whether `a == b` / `a != b` is lowered in the front end into a method
/// call instead of the scalar comparison. Two shapes lower:
///
/// * a `datac`/`enum` `T` that declares `fun T.eq(other: &T) => bool` (the same
///   method name A-12 gives a declared `Map` key), lowered to `a.eq(&b)`;
/// * `View` against `View`, or `View` against a `str`, lowered to the builtin
///   `View.eq`.
///
/// The operands are the already-checked `l`/`r` from `check_binary_operands`,
/// so each is evaluated exactly once and left to right, and a shared `Ref` is
/// what the declared `eq` receives, so an owned operand is borrowed rather than
/// moved. The result is an ordinary `Call`, so `borrowck.rs` and both backends
/// need no new shape. `None` means the ordinary comparison path applies, which
/// includes every type with no declared `eq` and every order operator.
pub(crate) fn lower_equality(
    l: &tir::Expr,
    r: &tir::Expr,
    op: BinOp,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    if !matches!(op, BinOp::Eq | BinOp::NotEq) {
        return Ok(None);
    }
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b {
            let key = crate::types::key_pair_method_key(a, crate::types::KEY_PAIR_EQ_METHOD);
            if let Some(sig) = ctx.sig_of(&key) {
                // The operator is a call, so an `unsafe fun T.eq` is gated
                // exactly as the written `a.eq(&b)` call is (the `Map` path
                // has no such sight of the declaration and is a known gap).
                if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                            display_callee(&key)
                        ),
                        span,
                    });
                }
                let mut args = vec![
                    equality_borrow(l, &sig.params[0].1),
                    equality_borrow(r, &sig.params[1].1),
                ];
                args.extend(hidden_args(&key, span, ctx)?);
                let call = tir::Expr {
                    kind: tir::ExprKind::Call { callee: key, args },
                    ty: Ty::Bool,
                    span,
                };
                return Ok(Some(negate_comparison(op, call, span)));
            }
        }
    }
    let l_text = matches!(l.ty, Ty::View) || is_str_like(&l.ty);
    let r_text = matches!(r.ty, Ty::View) || is_str_like(&r.ty);
    if l_text
        && r_text
        && (matches!(l.ty, Ty::View) || matches!(r.ty, Ty::View))
        && builtin_sig("View#eq").is_some()
    {
        let call = tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "View#eq".to_string(),
                args: vec![view_operand(l), view_operand(r)],
            },
            ty: Ty::Bool,
            span,
        };
        return Ok(Some(negate_comparison(op, call, span)));
    }
    // `Ord` says nothing about equality, so a type that implements only the
    // order trait is refused `==`/`!=` with a message that says which
    // declaration the operator actually needs (`design/traits.md (kelp wiki)` 13.3).
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b && implements_trait(&ctx.env, crate::types::ORD_TRAIT, a) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "cannot compare '{}' values with '{}': '==' and '!=' come from a declared 'eq' method, and 'Ord' does not supply one; write 'fun {}.eq(other: &{}) => bool'",
                    a,
                    bin_op_symbol(op),
                    a,
                    a
                ),
                span,
            });
        }
    }
    Ok(None)
}

/// The symbol one binary operator is written with, for a diagnostic.
pub(crate) fn bin_op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
    }
}

/// The `datac`/`enum` an operator applies to, when both operands name the same
/// user type (a borrow of one counts, exactly as an `==` operand may be `&T`)
/// and that type implements the shipped operator trait `trait_key`. `None`
/// means the operator's ordinary rule applies, which is what keeps a builtin
/// numeric pair and a genuine type mismatch on their original paths.
pub(crate) fn operator_owner(l: &tir::Expr, r: &tir::Expr, trait_key: &str, ctx: &Ctx) -> Option<String> {
    let a = equality_owner(&l.ty)?;
    let b = equality_owner(&r.ty)?;
    if a != b || !implements_trait(&ctx.env, trait_key, a) {
        return None;
    }
    Some(a.to_string())
}

/// The `datac`/`enum` a unary operator applies to, when the operand names a
/// user type that implements `trait_key`.
pub(crate) fn unary_operator_owner(operand: &tir::Expr, trait_key: &str, ctx: &Ctx) -> Option<String> {
    let name = equality_owner(&operand.ty)?;
    if !implements_trait(&ctx.env, trait_key, name) {
        return None;
    }
    Some(name.to_string())
}

/// The trait, the trait's bare name and the method one arithmetic operator
/// lowers through (`design/traits.md (kelp wiki)` 13.2).
pub(crate) fn arithmetic_operator(op: BinOp) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match op {
        BinOp::Add => (crate::types::ADD_TRAIT, "Add", "add"),
        BinOp::Sub => (crate::types::SUB_TRAIT, "Sub", "sub"),
        BinOp::Mul => (crate::types::MUL_TRAIT, "Mul", "mul"),
        BinOp::Div => (crate::types::DIV_TRAIT, "Div", "div"),
        BinOp::Mod => (crate::types::REM_TRAIT, "Rem", "rem"),
        _ => return None,
    })
}

/// Lowers one operator into the `std/ops` method call it means: the operands
/// are borrowed (never moved) exactly as `==`'s are, and each was already
/// checked, so it is evaluated once and left to right. The result is an
/// ordinary `Call`, so `borrowck.rs` and both backends need no new shape.
pub(crate) fn lower_operator_call(
    owner: &str,
    method: &str,
    operands: &[&tir::Expr],
    span: Span,
    ctx: &Ctx,
) -> Result<tir::Expr> {
    let key = crate::types::key_pair_method_key(owner, method);
    let sig = ctx.sig_of(&key).ok_or_else(|| TypeError {
        help: None,
        message: format!(
            "'{owner}' implements the operator trait, but its '{method}' method is missing"
        ),
        span,
    })?;
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'",
                display_callee(&key)
            ),
            span,
        });
    }
    let mut args = Vec::with_capacity(operands.len());
    for (i, operand) in operands.iter().enumerate() {
        match sig.params.get(i) {
            Some((_, want)) => args.push(equality_borrow(operand, want)),
            None => args.push((*operand).clone()),
        }
    }
    args.extend(hidden_args(&key, span, ctx)?);
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args },
        ty: sig.ret,
        span,
    })
}

/// Lowers `a OP b` for a user type that implements the shipped operator trait,
/// or `None` when the operand types are not one user type with the impl.
pub(crate) fn lower_arithmetic_operator(
    op: BinOp,
    l: &tir::Expr,
    r: &tir::Expr,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    let Some((trait_key, _, method)) = arithmetic_operator(op) else {
        return Ok(None);
    };
    let Some(owner) = operator_owner(l, r, trait_key, ctx) else {
        return Ok(None);
    };
    Ok(Some(lower_operator_call(&owner, method, &[l, r], span, ctx)?))
}

/// Lowers `a OP b` for a user type that implements `std/ops.Ord`: the `cmp`
/// call's `i32` against zero (`design/traits.md (kelp wiki)` 13.3).
pub(crate) fn lower_order_operator(
    op: BinOp,
    l: &tir::Expr,
    r: &tir::Expr,
    span: Span,
    ctx: &Ctx,
) -> Result<Option<tir::Expr>> {
    let Some(owner) = operator_owner(l, r, crate::types::ORD_TRAIT, ctx) else {
        return Ok(None);
    };
    let call = lower_operator_call(&owner, "cmp", &[l, r], span, ctx)?;
    let zero = tir::Expr {
        kind: tir::ExprKind::IntLit(0),
        ty: Ty::I32,
        span,
    };
    Ok(Some(tir::Expr {
        kind: tir::ExprKind::Binary {
            op,
            lhs: Box::new(call),
            rhs: Box::new(zero),
        },
        ty: Ty::Bool,
        span,
    }))
}

/// Lowers a `Display` value in `"${x}"` or `print(x)` to `x.to_text()`
/// (`design/traits.md (kelp wiki)` 13.4). `None` when the type does not implement `Display`,
/// which leaves the existing refusal in place.
pub(crate) fn lower_display(value: &tir::Expr, span: Span, ctx: &Ctx) -> Result<Option<tir::Expr>> {
    let Some(owner) = unary_operator_owner(value, crate::types::DISPLAY_TRAIT, ctx) else {
        return Ok(None);
    };
    Ok(Some(lower_operator_call(&owner, "to_text", &[value], span, ctx)?))
}

/// The refusal for an arithmetic operator whose operands are not a matching
/// numeric pair. When both name the same `datac`/`enum`, the operator traits
/// are the answer, so the message names the trait and the impl to write
/// (`design/traits.md (kelp wiki)` 13.2).
pub(crate) fn arithmetic_error(op: BinOp, l: &tir::Expr, r: &tir::Expr) -> String {
    if let (Some(a), Some(b)) = (equality_owner(&l.ty), equality_owner(&r.ty)) {
        if a == b {
            if let Some((_, simple, method)) = arithmetic_operator(op) {
                return format!(
                    "cannot use '{}' on '{}': it does not implement '{}' from 'std/ops' (write 'impl {} for {} {{ fun Self.{}(rhs: &Self) => {} }}', importing 'std/ops')",
                    bin_op_symbol(op),
                    a,
                    simple,
                    simple,
                    a,
                    method,
                    a
                );
            }
        }
        return format!(
            "cannot use '{}' on '{}' and '{}': the operands must have the same type",
            bin_op_symbol(op),
            a,
            b
        );
    }
    format!(
        "operator requires matching numeric operands, found '{}' and '{}'{}",
        l.ty.display(),
        r.ty.display(),
        exclusive_borrow_hint(l, r),
    )
}

/// The refusal for a value `print` cannot format. A `datac`/`enum` names
/// `Display`, which is what it has to implement (`design/traits.md (kelp wiki)` 13.4).
pub(crate) fn print_error(callee: &str, ty: &Ty) -> String {
    match ty {
        Ty::Struct(_) => format!(
            "cannot pass a value of type '{}' to '{callee}': it does not implement 'Display' (write 'impl fmt.Display for {} {{ fun Self.to_text() => String }}', importing 'std/fmt')",
            ty.display(),
            ty.display()
        ),
        other => format!("cannot pass a value of type '{}' to '{callee}'", other.display()),
    }
}

/// Whether the expression is a bare numeric or character literal - a value
/// with **no width of its own**, whose type comes entirely from its context
/// (`syntax.md` §2.4/§5). A literal in that position has no type until
/// something asks for one, which is what lets the rule below pick the
/// sibling operand's type for it.
///
/// An `Ident` is deliberately not one: a `const` has a *declared* type, so
/// `const N: i32 = 5` followed by `N + u32_expr` is a real mismatch rather
/// than a literal to retype.
pub(crate) fn is_untyped_literal(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::IntLit(..) | ast::Expr::FloatLit(..) | ast::Expr::CharLit(..) => true,
        // A negated literal (`-1`) is the same literal for this purpose, so `-1 * c`
        // takes the type of `c` as `1 * c` does (issue #275: the unannotated default is `int`).
        ast::Expr::Unary { op: ast::UnOp::Neg, expr, .. } => {
            matches!(**expr, ast::Expr::IntLit(..) | ast::Expr::FloatLit(..))
        }
        _ => false,
    }
}

/// Both operands of a binary operator, with the rule that makes the language's
/// own literal story true: **a literal takes the type of its sibling operand,
/// whichever side it is written on.**
///
/// The naive left-to-right order (`l` with the expectation, then `r` with
/// `l`'s type) leaves one hole, and it is the one a program written against
/// Rust's inference falls straight into:
///
/// ```kelp
/// fun push_byte(out: mut String, cp: u32) {
///     out.push((192 + cp / 64).to_u8())   // '192' can only be i32, from the left
/// }
/// ```
///
/// `192 + cp / 64` is `u32` in Rust, where the literal's type is inferred from
/// the expression it meets. Here `192` was typed `i32` before the right side
/// was ever read, and the result was
/// `operator requires matching numeric operands, found 'i32' and 'u32'` - for
/// a rule the reference already stated ("a literal has no width of its own; it
/// takes the integer type its context requires"). The sibling operand *is* the
/// context.
///
/// So: when there is no expectation to work from and the left side is a bare
/// literal while the right side is not, the right side is checked first and
/// the literal takes its type. Both literals, or neither, still go left to
/// right - nothing about the common cases changes.
pub(crate) fn check_binary_operands(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    ctx: &mut Ctx,
    hint: Option<&Ty>,
) -> Result<(tir::Expr, tir::Expr)> {
    if hint.is_none() && is_untyped_literal(lhs) && !is_untyped_literal(rhs) {
        let right = check_expr(rhs, ctx, None)?;
        let left = check_expr(lhs, ctx, Some(&right.ty))?;
        return Ok((left, right));
    }
    let left = check_expr(lhs, ctx, hint)?;
    let right = check_expr(rhs, ctx, Some(&left.ty))?;
    Ok((left, right))
}

/// When one operand of an arithmetic operator is an exclusive borrow, the
/// operator is reading the *reference* rather than the value behind it. That
/// happens with a `mut T` parameter, whose name inside the body is a borrow
/// (`syntax.md` 10.2), and the documented spelling is `*sink`, not `sink`
/// (§10's `*pos` rule). Name the operand and say so, because the bare
/// `mut i64` in the type pair is otherwise a type no source can write.
pub(crate) fn exclusive_borrow_hint(l: &tir::Expr, r: &tir::Expr) -> String {
    let expr = match (&l.ty, &r.ty) {
        (Ty::Ref { mutable: true, .. }, _) => l,
        (_, Ty::Ref { mutable: true, .. }) => r,
        _ => return String::new(),
    };
    let (written, fix) = match &expr.kind {
        tir::ExprKind::Ident(n) => (format!("'{n}'"), format!("*{n}")),
        _ => ("this operand".to_string(), "the dereferenced value".to_string()),
    };
    format!(
        "; {written} is a borrow ('{}'), so the operator reads the reference itself; write '{fix}' to read through it",
        expr.ty.display()
    )
}

pub(crate) fn check_binary(
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
            let (l, r) = check_binary_operands(lhs, rhs, ctx, numeric_expected)?;
            if !l.ty.is_numeric() || l.ty != r.ty {
                // A `datac`/`enum` that implements `std/ops`'s trait for this
                // operator is lowered into the trait method here
                // (`design/traits.md (kelp wiki)` 13.2); a builtin numeric pair never reaches
                // this point, so the builtin operators are untouched.
                if let Some(expr) = lower_arithmetic_operator(op, &l, &r, span, ctx)? {
                    return Ok(expr);
                }
                return Err(TypeError {
                    help: None,
                    message: arithmetic_error(op, &l, &r),
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
        // Bitwise operators (A-9). Integral operands of the same type,
        // result the same type - the same rule arithmetic uses, one domain
        // over. `~` is the prefix form and is checked on the unary path.
        //
        // `bool` is deliberately not a bitwise domain: `a & b` on two
        // booleans reads as a logical `and` that is not short-circuiting,
        // which is a trap, and `&&` is right there.
        BitAnd | BitOr | BitXor | Shl | Shr => {
            let int_expected = expected.filter(|t| t.is_integer());
            let (l, r) = check_binary_operands(lhs, rhs, ctx, int_expected)?;
            if !l.ty.is_integer() || l.ty != r.ty {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "operator requires matching integer operands, found '{}' and '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            // A shift by an amount already known to be out of range is a
            // compile error; any other amount is checked at run time by both
            // backends (`shift amount out of range`).
            if matches!(op, Shl | Shr) {
                if let (tir::ExprKind::IntLit(v), Some(bits)) = (&r.kind, int_bits(&l.ty)) {
                    if *v < 0 || *v >= bits as i128 {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "a shift by {v} is not defined for a '{}', whose width is {bits} bits: the amount has to be in 0..{bits} (use wrapping_shl/wrapping_shr to mask it)",
                                l.ty.display()
                            ),
                            span,
                        });
                    }
                }
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
            let (l, r) = check_binary_operands(lhs, rhs, ctx, None)?;
            // Two handles of the same type, or of an ancestor/descendant pair,
            // compare: the pointers are the same width either way, and an
            // upcast would have produced the ancestor value anyway.
            let handle_pair = match (&l.ty, &r.ty) {
                (Ty::Handle(a), Ty::Handle(b)) => {
                    handle_is_same_or_ancestor(&ctx.env.handle_parents, a, b)
                        || handle_is_same_or_ancestor(&ctx.env.handle_parents, b, a)
                }
                _ => false,
            };
            // H-5: a `datac`/`enum` that declares `eq`, and the two `View`
            // shapes, are an ordinary method call (or its negation) by the time
            // the rest of the pipeline sees them. This runs before the
            // operand-type match below because a `&T` against a `T` and a
            // `View` against a `str` are two different spellings of one
            // comparison.
            if let Some(expr) = lower_equality(&l, &r, op, span, ctx)? {
                return Ok(expr);
            }
            // `Ord` gives the four order operators, through `cmp`
            // (`design/traits.md (kelp wiki)` 13.3). Equality already returned above.
            if let Some(expr) = lower_order_operator(op, &l, &r, span, ctx)? {
                return Ok(expr);
            }
            if l.ty != r.ty && !handle_pair {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "cannot compare '{}' with '{}'",
                        l.ty.display(),
                        r.ty.display()
                    ),
                    span,
                });
            }
            // An opaque handle has equality and nothing else: `==`/`!=`
            // compare the two pointers, which is cheap and is the only
            // thing the operator could mean for a value with no readable
            // contents. An order (`<`, `<=`, `>`, `>=`) is still refused.
            let comparable = comparable(&l.ty)
                || ((matches!((&l.ty, &r.ty), (Ty::Handle(_), Ty::Handle(_)))
                    || matches!((&l.ty, &r.ty), (Ty::Ptr(_), Ty::Ptr(_))))
                    && matches!(op, BinOp::Eq | BinOp::NotEq));
            if !comparable {
                return Err(TypeError {
                    help: None,
                    message: comparison_error(&l.ty, op),
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
                    help: None,
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

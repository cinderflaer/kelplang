use super::*;
use crate::types::Ty;

fn span() -> Span {
    Span { start: 0, end: 0 }
}

fn ident(name: &str) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Ident(name.to_string()),
        ty: Ty::I32,
        span: span(),
    }
}

fn region(name: &str, tail: tir::Expr) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Region {
            name: name.to_string(),
            elem: Ty::I32,
            body: tir::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(tail)),
            },
        },
        ty: Ty::I32,
        span: span(),
    }
}

/// A region's body is part of the expression, so a name used inside it is
/// seen by the walkers the same way a plain block's name is (the arms
/// added with the `Region` node).
#[test]
fn a_name_used_inside_a_region_body_is_seen() {
    let e = region("arena", ident("x"));
    assert!(expr_mentions(&e, "x"));
    assert!(!expr_mentions(&e, "y"));
}

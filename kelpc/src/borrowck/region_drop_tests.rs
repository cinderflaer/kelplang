use super::*;
use crate::types::Ty;
use std::collections::{HashMap, HashSet};

fn span() -> Span {
    Span { start: 0, end: 0 }
}

fn ident(name: &str, ty: Ty) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Ident(name.to_string()),
        ty,
        span: span(),
    }
}

/// A `block name: i32 { body }` region whose body is `stmts` and `tail`.
fn region(name: &str, stmts: Vec<tir::Stmt>, tail: Option<tir::Expr>) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Region {
            name: name.to_string(),
            elem: Ty::I32,
            body: tir::Block {
                stmts,
                tail: tail.map(Box::new),
            },
        },
        ty: Ty::I32,
        span: span(),
    }
}

/// A one-function program named `main`, which takes an `i32` `x` and
/// returns an `i32` from `body`.
fn program_with(body: tir::Block) -> tir::Program {
    tir::Program {
        structs: Vec::new(),
        enums: Vec::new(),
        pds: Vec::new(),
        functions: vec![tir::Function {
            name: "main".to_string(),
            params: vec![("x".to_string(), Ty::I32)],
            ret: Ty::I32,
            body,
            span: span(),
            module: "main".to_string(),
            extern_: false,
            env: None,
            naked: false,
            align: None,
            section: None,
        }],
        caps: Vec::new(),
        gated: HashMap::new(),
        declared_keys: HashSet::new(),
        drop_types: HashSet::new(),
        extern_ffi_symbols: HashMap::new(),
        const_data: Vec::new(),
        statics: Vec::new(),
        dyn_vtables: Vec::new(),
        dyn_traits: Vec::new(),
    }
}

/// The region that is the tail of `main`'s body, by reference into
/// `program`, so its identity is the one the plan was keyed by.
fn tail_region(program: &tir::Program) -> &tir::Expr {
    program.functions[0]
        .body
        .tail
        .as_deref()
        .expect("main's body has a tail")
}

/// The side table records a region the function declares: its name and its
/// element type, keyed by the `Region` expression (no codegen reads it yet).
#[test]
fn the_plan_records_a_region_by_its_expression() {
    let body = tir::Block {
        stmts: Vec::new(),
        tail: Some(Box::new(region("arena", Vec::new(), Some(ident("x", Ty::I32))))),
    };
    let program = program_with(body);
    let plan = check_program(&program).expect("the program checks");
    let found = plan
        .region("main", tail_region(&program))
        .expect("the region is recorded");
    assert_eq!(found.name, "arena");
    assert_eq!(found.elem, Ty::I32);
}

/// An expression that is not a region is not in the table, and a region of
/// another function is not found under this one.
#[test]
fn a_plain_expression_is_not_a_region() {
    let body = tir::Block {
        stmts: Vec::new(),
        tail: Some(Box::new(ident("x", Ty::I32))),
    };
    let program = program_with(body);
    let plan = check_program(&program).expect("the program checks");
    let tail = program.functions[0].body.tail.as_deref().unwrap();
    assert!(plan.region("main", tail).is_none());
    assert!(plan.region("elsewhere", tail_region(&program)).is_none());
}

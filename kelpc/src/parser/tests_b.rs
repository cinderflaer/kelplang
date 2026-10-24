use super::*;
use super::tests::{parse, parse_err};

#[test]
fn parses_datac_decl_with_val_and_var_fields() {
    let p = parse("datac Point(val x: i32, var y: i32)\nfun main() {}");
    assert_eq!(p.datacs.len(), 1);
    let d = &p.datacs[0];
    assert_eq!(d.name, "Point");
    assert!(d.type_params.is_empty());
    assert_eq!(d.fields.len(), 2);
    assert!(!d.fields[0].mutable);
    assert!(d.fields[1].mutable);
}

#[test]
fn parses_type_parameters_on_a_datac_and_an_enum() {
    // A-2: the parameter list sits between the declared name and the
    // declaration's own `(` or `{`, and a use site writes the arguments.
    let p = parse("datac Pair<A, B>(val first: A, val second: B)\nfun main() {}");
    assert_eq!(p.datacs[0].type_params, vec!["A".to_string(), "B".to_string()]);

    let p = parse("enum Option<T> {\n    Some(val value: T)\n    None\n}\nfun main() {}");
    assert_eq!(p.enums[0].type_params, vec!["T".to_string()]);

    let p = parse("fun f(p: Pair<i32, String>) => Option<i32> = 1\nfun main() {}");
    let ty = &p.functions[0].params[0].ty;
    assert_eq!(ty.name, "Pair");
    assert_eq!(ty.args.len(), 2);
    assert_eq!(ty.args[0].name, "i32");
    assert_eq!(ty.args[1].name, "String");
    assert_eq!(p.functions[0].return_type.as_ref().unwrap().args.len(), 1);
}

#[test]
fn generic_declaration_errors_are_parse_errors() {
    parse_err("datac Pair<T, T>(val a: T, val b: T)\nfun main() {}");
    parse_err("datac Pair<>(val a: i32)\nfun main() {}");
}

#[test]
fn parses_type_parameters_on_a_function() {
    // A-2: a free function declares its own parameters after its name, and
    // a receiver method's come from the receiver's argument list.
    let p = parse("fun first<T>(xs: List<T>) => T = xs.get(i = 0)\nfun main() {}");
    assert_eq!(p.functions[0].type_params, vec!["T".to_string()]);
    assert!(p.functions[0].receiver.is_none());

    let p = parse("fun Pair<A, B>.first() => A = a\nfun main() {}");
    let f = &p.functions[0];
    assert!(f.type_params.is_empty());
    let r = f.receiver.as_ref().expect("a receiver");
    assert_eq!(r.type_name, "Pair");
    assert_eq!(r.type_args, vec!["A".to_string(), "B".to_string()]);
}

#[test]
fn parses_const_type_parameters_and_const_arguments() {
    // A-20: `const N: usize` is a value parameter, listed with the type
    // parameters and recorded separately. A const argument is written like
    // a type argument (`Buf<16>`, `Buf<SIZE * 2>`).
    let p = parse(
        "datac Buf<const N: usize>(var data: [i32; N], var len: usize)\nfun main() {}",
    );
    assert_eq!(p.datacs[0].type_params, vec!["N".to_string()]);
    assert_eq!(p.datacs[0].const_params.len(), 1);
    assert_eq!(p.datacs[0].const_params[0].name, "N");
    assert_eq!(p.datacs[0].const_params[0].ty, "usize");

    let p = parse("fun f<const N: usize, T>() => i32 = 0\nfun main() {}");
    assert_eq!(p.functions[0].type_params, vec!["N".to_string(), "T".to_string()]);
    assert_eq!(p.functions[0].const_params.len(), 1);
    assert_eq!(p.functions[0].const_params[0].name, "N");

    let p = parse("fun f(a: Buf<16>) => i32 = 0\nfun main() {}");
    let arg = &p.functions[0].params[0].ty.args[0];
    assert!(arg.const_expr.is_some());
    assert!(arg.name.is_empty());

    let p = parse("fun f(a: Buf<SIZE * 2>) => i32 = 0\nfun main() {}");
    let arg = &p.functions[0].params[0].ty.args[0];
    assert!(matches!(
        arg.const_expr.as_deref(),
        Some(Expr::Binary { op: BinOp::Mul, .. })
    ));

    // A bare name (`Buf<SIZE>`) parses as a type argument; the resolver
    // reads it as the const where the parameter is a `const`.
    let p = parse("fun f(a: Buf<SIZE>) => i32 = 0\nfun main() {}");
    assert_eq!(p.functions[0].params[0].ty.args[0].name, "SIZE");
}

#[test]
fn parses_impl_trait_in_argument_position_as_a_bounded_parameter() {
    // A-20: each `impl Trait` occurrence becomes its own fresh parameter,
    // and the trait is its bound.
    let p = parse("fun f(x: impl Shape, y: &impl Shape) => i32 = 0\nfun main() {}");
    let f = &p.functions[0];
    assert_eq!(f.type_params.len(), 2);
    assert_ne!(f.type_params[0], f.type_params[1]);
    assert_eq!(f.type_param_bounds.len(), 2);
    assert_eq!(f.type_param_bounds[0][0].name, "Shape");
    assert_eq!(f.params[0].ty.name, f.type_params[0]);
    assert_eq!(f.params[1].ty.name, f.type_params[1]);
    assert!(f.params[1].ty.amp);
}

#[test]
fn impl_trait_in_a_return_position_is_refused_at_parse_time() {
    parse_err(
        "trait Shape { fun Self.area() => i32 }\nfun f() => impl Shape = 1\nfun main() {}",
    );
}

#[test]
fn parses_construction_and_field_access_via_call_syntax() {
    let p = parse("fun main() {\n    val p = Point(x = 1, y = 2)\n    val z = p.x.y\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init: Expr::Call { callee, args, .. }, .. } => {
            assert_eq!(callee, "Point");
            assert_eq!(args.len(), 2);
            assert_eq!(args[0].name.as_deref(), Some("x"));
            assert_eq!(args[1].name.as_deref(), Some("y"));
        }
        other => panic!("expected call, got {other:?}"),
    }
    match &stmts[1] {
        Stmt::Let { init: Expr::FieldAccess { field, base, .. }, .. } => {
            assert_eq!(field, "y");
            assert!(matches!(**base, Expr::FieldAccess { .. }));
        }
        other => panic!("expected field access, got {other:?}"),
    }
}

#[test]
fn explicit_type_arguments_are_recognized_only_when_adjacent() {
    // A-2: `Pair<i32, String>(...)` - the `<` touches the name and the
    // `(` touches the `>`, which is what keeps an ordinary comparison
    // chain (`a < b > (c)`) from being read as an instantiation.
    let p = parse("fun main() {\n    val x = Pair<i32, String>(a = 1)\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init: Expr::Call { type_args, .. }, .. } => {
            assert_eq!(type_args.len(), 2);
            assert_eq!(type_args[0].name, "i32");
            assert_eq!(type_args[1].name, "String");
        }
        other => panic!("expected a call with type arguments, got {other:?}"),
    }

    let p = parse("fun main() {\n    val y = a < b > (d)\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init: Expr::Binary { op: BinOp::Gt, .. }, .. } => {}
        other => panic!("expected a comparison, got {other:?}"),
    }
}

#[test]
fn parses_mixed_named_and_positional_call_args() {
    let p = parse("fun main() {\n    print(f(1, y = 2))\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Expr { expr: Expr::Call { args, .. }, .. } => {
            let inner = &args[0].value;
            match inner {
                Expr::Call { args, .. } => {
                    assert!(args[0].name.is_none());
                    assert_eq!(args[1].name.as_deref(), Some("y"));
                }
                other => panic!("expected inner call, got {other:?}"),
            }
        }
        other => panic!("expected call stmt, got {other:?}"),
    }
}

#[test]
fn does_not_confuse_equality_with_named_arg() {
    let p = parse("fun main() {\n    val x = f(a == 1)\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init: Expr::Call { args, .. }, .. } => {
            assert!(args[0].name.is_none());
            assert!(matches!(args[0].value, Expr::Binary { op: BinOp::Eq, .. }));
        }
        other => panic!("expected call, got {other:?}"),
    }
}

#[test]
fn parses_when_and_desugars_to_nested_if() {
    let p = parse(
        "fun classify(n: i32) => i32 = when {\n    n > 0 -> 1\n    n < 0 -> -1\n    else -> 0\n}",
    );
    match &p.functions[0].body {
        FunctionBody::Expr(Expr::If { then_block, else_block, .. }) => {
            assert_eq!(then_block.stmts.len(), 1);
            match else_block.as_deref() {
                Some(ElseBranch::If(Expr::If { else_block, .. })) => {
                    assert!(matches!(else_block.as_deref(), Some(ElseBranch::Block(_))));
                }
                other => panic!("expected nested else-if, got {other:?}"),
            }
        }
        other => panic!("expected desugared if, got {other:?}"),
    }
}

#[test]
fn parses_compound_assignment_operators() {
    let p = parse("fun main() {\n    var x = 1\n    x += 2\n    x -= 1\n    x *= 3\n    x /= 2\n    x %= 2\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    for (stmt, op) in stmts[1..].iter().zip([BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Mod]) {
        match stmt {
            Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                assert!(matches!(&**value, Expr::Binary { op: o, .. } if *o == op));
            }
            other => panic!("expected compound-assign desugared stmt, got {other:?}"),
        }
    }
}

#[test]
fn accepts_ampersand_str_type() {
    let p = parse("fun f(s: &str) => &str = s");
    assert_eq!(p.functions[0].params[0].ty.name, "str");
    assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "str");
}

#[test]
fn parses_a_move_parameter() {
    let p = parse("fun consume(s: move String) => usize = s.len()");
    let param = &p.functions[0].params[0];
    assert!(param.mode.moves());
    assert!(!param.mode.mutable());
    assert_eq!(param.ty.name, "String");
}

#[test]
fn parses_a_function_type_in_parameter_and_return_position() {
    let p = parse("fun apply(f: fun(i32) => i32, x: i32) => fun(move String) = f");
    let f = &p.functions[0];
    let param = &f.params[0];
    assert!(param.ty.is_fn());
    let params = param.ty.fn_params.as_ref().unwrap();
    assert_eq!(params.len(), 1);
    assert!(!params[0].mode.mutable());
    assert_eq!(params[0].ty.name, "i32");
    assert_eq!(param.ty.fn_ret.as_ref().unwrap().name, "i32");

    // The return type is a function type with no return type, so it returns
    // `Unit` - and its parameter is a `move`, which a function type
    // spells the same way a declaration does.
    let ret = f.return_type.as_ref().unwrap();
    assert!(ret.is_fn());
    assert!(ret.fn_ret.is_none());
    assert!(ret.fn_params.as_ref().unwrap()[0].mode.moves());
    assert_eq!(ret.display(), "fun(move String)");
}

#[test]
fn parses_a_closure_type_in_parameter_and_return_position() {
    // A closure type is a function type with the `closure` marker in front of
    // `fun` - the same parameter list, the same optional `=> T` (A-4 phase E,
    // issue #293).
    let p = parse("fun use(f: closure fun(i32) => i32, g: move closure fun()) {}");
    let f = &p.functions[0];

    let f_param = &f.params[0];
    assert!(f_param.ty.is_closure());
    assert_eq!(f_param.ty.display(), "closure fun(i32) => i32");
    assert!(!f_param.mode.moves());

    // A bare closure parameter borrows the environment (an owned type's
    // parameter is a borrow); `move closure fun(...)` takes it over.
    let g_param = &f.params[1];
    assert!(g_param.ty.is_closure());
    assert!(g_param.mode.moves());
    assert_eq!(g_param.ty.display(), "closure fun()");
}

#[test]
fn a_plain_function_type_is_not_a_closure() {
    // `fun(...)` alone is the function pointer; the marker is what makes the
    // closure form, so one keyword carries both meanings (issue #293).
    let p = parse("fun use(f: fun(i32) => i32) {}");
    let f_param = &p.functions[0].params[0];
    assert!(f_param.ty.is_fn());
    assert!(!f_param.ty.is_closure());
    assert_eq!(f_param.ty.display(), "fun(i32) => i32");
}

#[test]
fn the_old_closure_spelling_is_refused_with_the_new_one_named() {
    // `closure fun(...)` was the old spelling of the closure form. It is refused,
    // and the message names the one spelling that replaces it (issue #293).
    let err = parse_err("fun use(f: closure(i32) => i32) {}");
    assert!(err.message.contains("closure fun("), "message was: {}", err.message);
}

#[test]
fn parses_a_fun_literal_in_expression_position() {
    let p = parse("fun main() {\n    val f = fun(x: move String) => usize = x.len()\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let {
            init: Expr::FnLit { params, return_type, .. },
            ..
        } => {
            assert_eq!(params.len(), 1);
            assert!(params[0].mode.moves());
            assert_eq!(return_type.as_ref().unwrap().name, "usize");
        }
        other => panic!("expected a fun literal, got {other:?}"),
    }
}

#[test]
fn a_fun_literal_needs_a_body() {
    parse_err("fun main() {\n    val f = fun(x: i32) => i32\n}");
}

#[test]
fn rejects_move_combined_with_mut_or_a_receiver() {
    // One parameter cannot both borrow exclusively and take ownership.
    parse_err("fun f(s: mut move String) {}");
    // `move` is a parameter qualifier, never a receiver one.
    parse_err("fun move Counter.bump() {}");
}

#[test]
fn rejects_malformed_input() {
    parse_err("fun main( {}");
}

#[test]
fn parses_ref_deref_and_mut_param() {
    let p = parse("fun bump(c: mut Counter) {\n    val r = &c\n    val m = &mut c\n    val v = *r\n}");
    assert!(p.functions[0].params[0].mode.mutable());
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init: Expr::Ref { mutable: false, .. }, .. } => {}
        other => panic!("expected shared ref, got {other:?}"),
    }
    match &stmts[1] {
        Stmt::Let { init: Expr::Ref { mutable: true, .. }, .. } => {}
        other => panic!("expected exclusive ref, got {other:?}"),
    }
    match &stmts[2] {
        Stmt::Let { init: Expr::Deref { .. }, .. } => {}
        other => panic!("expected deref, got {other:?}"),
    }
}

#[test]
fn ampersand_mut_is_rejected_in_param_position() {
    parse_err("fun bump(c: &mut Counter) {}");
}

#[test]
fn parses_unary_operators() {
    let p = parse("fun main() {\n    val x = -1\n    val y = !true\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Neg, .. })),
        other => panic!("expected let stmt, got {other:?}"),
    }
    match &stmts[1] {
        Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Not, .. })),
        other => panic!("expected let stmt, got {other:?}"),
    }
}

#[test]
fn parses_impl_lines_before_decls() {
    let p = parse("impl \"shapes/circle\"\nimpl \"std/time\" caps(time = [\"/7d\"])\n\nfun main() {}");
    assert_eq!(p.impls.len(), 2);
    assert_eq!(p.impls[0].path, "shapes/circle");
    assert_eq!(p.impls[1].path, "std/time");
    assert_eq!(p.impls[1].caps[0].domain, "time");
    assert_eq!(p.functions.len(), 1);
}

#[test]
fn the_impl_as_clause_is_refused_by_name() {
    let err = parse_err("impl \"time\" as t\n\nfun main() {}");
    assert!(
        err.message.contains("is gone: reach the module by its own name"),
        "{}",
        err.message
    );
}

#[test]
fn parses_module_qualified_type_names() {
    let p = parse("fun f(c: circle.Circle) => circle.Circle = c");
    assert_eq!(p.functions[0].params[0].ty.name, "circle.Circle");
    assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "circle.Circle");
}

#[test]
fn impl_after_a_declaration_is_rejected() {
    // Canonical order is impl, pd, then datac/fun.
    let e = parse_err("fun main() {}\nimpl \"util\"");
    assert!(e.message.contains("must come first"), "got: {}", e.message);
}

#[test]
fn parses_a_caps_clause_with_several_grants() {
    let p = parse("impl \"util\" caps(fs = [\"/tmp/**\", \"/var/log/*\"], env = [\"HOME\"])");
    assert_eq!(p.impls.len(), 1);
    let caps = &p.impls[0].caps;
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].domain, "fs");
    assert_eq!(caps[0].patterns, vec!["/tmp/**", "/var/log/*"]);
    assert_eq!(caps[1].domain, "env");
    assert_eq!(caps[1].patterns, vec!["HOME"]);
}

#[test]
fn the_with_caps_spelling_is_refused_by_name() {
    // Removed, not outdated: the clause is the same one under the surviving
    // spelling, and the refusal names the keyword to delete.
    let err = parse_err("impl \"util\" with caps(fs = [\"/tmp/**\"])");
    assert!(
        err.message.contains("is gone: drop the `with`"),
        "{}",
        err.message
    );
    let p = parse("impl \"util\" caps(fs = [\"/tmp/**\"])");
    assert_eq!(p.impls[0].caps[0].domain, "fs");
}

#[test]
fn a_caps_clause_requires_bracketed_string_patterns() {
    parse_err("impl \"util\" caps(fs = [tmp])");
}

#[test]
fn an_impl_without_caps_has_no_grants() {
    let p = parse("impl \"std/fs\"");
    assert!(p.impls[0].caps.is_empty());
    assert!(!p.impls[0].caps_all);
}

#[test]
fn parses_the_caps_wildcard_clause() {
    let p = parse("impl \"helper\" caps(*)");
    assert!(p.impls[0].caps.is_empty());
    assert!(p.impls[0].caps_all);
}

#[test]
fn a_named_domain_beside_the_wildcard_is_refused() {
    // Either order: `*` already means every domain, so a domain named
    // beside it has nothing left to say, and the refusal names both
    // surviving spellings.
    for source in [
        "impl \"helper\" caps(*, fs = [\"/tmp/**\"])",
        "impl \"helper\" caps(fs = [\"/tmp/**\"], *)",
    ] {
        let err = parse_err(source);
        assert!(
            err.message.contains("already means every domain"),
            "{source}: {}",
            err.message
        );
        assert!(err.message.contains("caps(*)` alone"), "{source}: {}", err.message);
    }
}

#[test]
fn impl_requires_a_string_path() {
    parse_err("impl util");
}

#[test]
fn parses_passdown_declarations() {
    let p = parse("pd val app_name = \"shapes\"\npd var count = 0\npd val label: &str = app_name\n\nfun main() {}");
    assert_eq!(p.pds.len(), 3);
    assert_eq!(p.pds[0].name, "app_name");
    assert!(!p.pds[0].mutable);
    assert!(p.pds[0].ty.is_none());
    assert!(p.pds[1].mutable);
    assert_eq!(p.pds[2].ty.as_ref().unwrap().name, "str");
    assert!(matches!(p.pds[2].init, Expr::Ident(..)));
}

#[test]
fn pd_after_a_declaration_is_rejected() {
    let e = parse_err("fun main() {}\npd val x = 1");
    assert!(e.message.contains("must come before"), "got: {}", e.message);
}

#[test]
fn pd_requires_val_or_var() {
    parse_err("pd x = 1");
}

#[test]
fn parses_a_template_into_text_and_expression_parts() {
    let p = parse("fun main() {\n    print(\"a $b ${c.d()}\")\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    let call = match &stmts[0] {
        Stmt::Expr { expr: Expr::Call { args, .. }, .. } => &args[0].value,
        other => panic!("expected a call, got {other:?}"),
    };
    let parts = match call {
        Expr::Template { parts, .. } => parts,
        other => panic!("expected a template, got {other:?}"),
    };
    assert_eq!(parts.len(), 4);
    assert!(matches!(&parts[0], TemplatePart::Text(t) if t == "a "));
    assert!(matches!(&parts[1], TemplatePart::Expr(Expr::Ident(n, _)) if n == "b"));
    assert!(matches!(&parts[2], TemplatePart::Text(t) if t == " "));
    assert!(matches!(&parts[3], TemplatePart::Expr(Expr::MethodCall { method, .. }) if method == "d"));
}

#[test]
fn pub_is_read_on_the_five_item_forms_that_can_carry_it() {
    let p = parse(
        "pub const LIMIT: i32 = 1\npub datac Point(val x: i32)\npub enum Shape { Empty }\npub fun f() => i32 = 1\npub extern fun g() => i32",
    );
    assert!(p.consts[0].is_pub);
    assert!(p.datacs[0].is_pub);
    assert!(p.enums[0].is_pub);
    assert!(p.functions[0].is_pub);
    assert!(p.functions[1].is_pub);
    assert!(p.functions[1].extern_);
}

#[test]
fn extern_type_reads_its_name_and_its_pub() {
    let p = parse("extern type Widget\npub extern type Buffer");
    assert_eq!(p.extern_types.len(), 2);
    assert_eq!(p.extern_types[0].name, "Widget");
    assert!(p.extern_types[0].parent.is_none());
    assert!(!p.extern_types[0].is_pub);
    assert_eq!(p.extern_types[1].name, "Buffer");
    assert!(p.extern_types[1].is_pub);
}

#[test]
fn extern_type_reads_an_optional_parent() {
    let p = parse("extern type Window: Widget\nextern type Dialog: mem.Panel");
    assert_eq!(p.extern_types[0].parent.as_deref(), Some("Widget"));
    assert_eq!(p.extern_types[1].parent.as_deref(), Some("mem.Panel"));
}

#[test]
fn extern_type_takes_no_body_or_fields() {
    let e = parse_err("extern type Widget {}");
    assert!(e.message.contains("expected 'fun'"), "got: {}", e.message);
}

#[test]
fn an_item_without_pub_is_private() {
    let p = parse("fun f() => i32 = 1");
    assert!(!p.functions[0].is_pub);
}

#[test]
fn pub_pd_is_refused_by_name() {
    let e = parse_err("pub pd val x = 1\nfun main() {}");
    assert!(e.message.contains("a 'pd' cannot be 'pub'"), "got: {}", e.message);
}

#[test]
fn pub_on_a_receiver_method_names_the_type() {
    let e = parse_err("pub fun Circle.area() => f64 = 1.0");
    assert!(
        e.message.contains("mark the type 'pub' instead"),
        "got: {}",
        e.message
    );
}

#[test]
fn pub_on_anything_else_names_what_it_applies_to() {
    let e = parse_err("pub impl \"util\"");
    assert!(
        e.message.contains("'pub' applies to a top-level 'fun'"),
        "got: {}",
        e.message
    );
}

#[test]
fn pub_inside_a_block_names_the_top_level_forms() {
    let e = parse_err("fun main() {\n    pub val x = 1\n}");
    assert!(
        e.message.contains("and only at the top level of a file"),
        "got: {}",
        e.message
    );
}

fn body_stmts(p: &Program) -> &Vec<Stmt> {
    match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    }
}

fn first_call_args(src: &str) -> Vec<CallArg> {
    let p = parse(src);
    match &body_stmts(&p)[0] {
        Stmt::Expr { expr: Expr::Call { args, .. } | Expr::MethodCall { args, .. }, .. } => args.clone(),
        other => panic!("expected a call statement, got {other:?}"),
    }
}

#[test]
fn a_trailing_block_is_the_last_argument_as_a_unit_fun_literal() {
    let args = first_call_args("fun main() {\n    run(1) { work() }\n}");
    assert_eq!(args.len(), 2);
    assert!(matches!(args[1].value, Expr::FnLit { ref params, return_type: None, .. } if params.is_empty()));
    assert!(args[1].name.is_none());
}

#[test]
fn a_trailing_block_needs_no_parentheses_when_it_is_the_only_argument() {
    let args = first_call_args("fun main() {\n    column { work() }\n}");
    assert_eq!(args.len(), 1);
    let args = first_call_args("fun main() {\n    ui.column { work() }\n}");
    assert_eq!(args.len(), 1);
}

#[test]
fn a_block_on_the_next_line_is_not_a_trailing_block() {
    // Two statements: the call with one argument, then a block of its own.
    let p = parse("fun main() {\n    run(1)\n    { work() }\n}");
    assert_eq!(body_stmts(&p).len(), 2);
}

#[test]
fn a_control_flow_head_keeps_its_brace_as_the_body() {
    // `f(x)` and `xs.len()` are calls ending a head: the `{` is the body.
    parse("fun main() {\n    if f(x) { work() }\n    while g() { work() }\n    for a in xs.items() { work() }\n}");
    parse("fun main() {\n    if cond { work() } else { other() }\n    val v = when x.pick() { A -> 1 }\n}");
}

#[test]
fn parentheses_and_blocks_reenable_a_trailing_block_inside_a_head() {
    parse("fun main() {\n    if f(run { work() }) { work() }\n    if cond { run { work() } }\n}");
}

#[test]
fn an_unsafe_block_is_an_expression_and_a_statement() {
    // Stage n (MK9-S2): `unsafe` is a contextual word, a block introducer
    // exactly when a `{` follows it.
    let p = parse("fun main() {\n    unsafe {\n        p.write(1)\n    }\n}");
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Expr { expr: Expr::Unsafe { block, .. }, .. } => {
                assert_eq!(block.stmts.len(), 1);
            }
            other => panic!("expected an unsafe block statement, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }
    // As an expression, with a value.
    parse("fun main() {\n    val v = unsafe { p.read() }\n}");
}

#[test]
fn unsafe_is_still_an_ordinary_name_away_from_a_brace() {
    let p = parse("fun main() {\n    val unsafe = 1\n    print(unsafe)\n}");
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Let { name, .. } => assert_eq!(name, "unsafe"),
            other => panic!("expected a let, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }
}

#[test]
fn a_block_header_declares_its_parameters_by_name() {
    let args = first_call_args("fun main() {\n    column { x, y -> f(x, y) }\n}");
    assert_eq!(args.len(), 1);
    match &args[0].value {
        Expr::FnLit { params, infer_params, return_type: None, .. } => {
            assert!(*infer_params);
            assert_eq!(params.len(), 2);
            assert_eq!(params[0].name, "x");
            assert_eq!(params[1].name, "y");
            // No type is written on a block header: it comes from the
            // function type the block is passed to.
            assert_eq!(params[0].ty.name, "");
        }
        other => panic!("expected a block literal, got {other:?}"),
    }
}

#[test]
fn an_unsafe_fun_declaration_sets_its_flag() {
    let p = parse("unsafe fun poke(p: Ptr<i32>, v: i32) {\n    p.write(v)\n}\nfun main() {}");
    assert!(p.functions[0].unsafe_);
    assert!(!p.functions[1].unsafe_);
    assert_eq!(p.functions[0].params[0].ty.display(), "Ptr<i32>");
}

#[test]
fn a_block_whose_first_tokens_are_not_a_header_is_an_ordinary_block() {
    // `x` alone, and `x + 1`, are expressions, not parameter headers.
    let args = first_call_args("fun main() {\n    run { x }\n}");
    assert!(matches!(&args[0].value, Expr::FnLit { params, .. } if params.is_empty()));
    parse("fun main() {\n    run { x + 1 }\n}");
}

#[test]
fn a_scoped_function_parameter_type_parses_as_a_modifier() {
    parse("fun each(f: scoped fun(i32)) {}\n");
    parse("fun each(f: scoped fun(i32) => i32) {}\n");
    // `scoped` is contextual: as an ordinary type name it is untouched.
    parse("datac scoped(val n: i32)\nfun f(x: scoped) {}\n");
}

#[test]
fn an_asm_expression_parses_its_operands_in_order() {
    let p = parse(
        "fun main() {\n    asm(\"add {0}, {1}\", inout(\"r\") x, in(\"r\") y, clobber(\"rax\"), nomem)\n}",
    );
    let stmts = body_stmts(&p);
    let Stmt::Expr { expr, .. } = &stmts[0] else {
        panic!("expected an asm statement");
    };
    let Expr::Asm {
        template,
        operands,
        clobbers,
        nomem,
        nostack,
        ..
    } = expr
    else {
        panic!("expected an asm statement");
    };
    assert_eq!(template, "add {0}, {1}");
    assert_eq!(operands.len(), 2);
    assert_eq!(operands[0].kind, AsmOperandKind::InOut);
    assert_eq!(operands[0].constraint, "r");
    assert!(operands[0].operand.is_some());
    assert_eq!(operands[1].kind, AsmOperandKind::In);
    assert_eq!(clobbers, &vec!["rax".to_string()]);
    assert!(*nomem);
    assert!(!*nostack);
}

#[test]
fn an_anonymous_out_operand_parses_as_no_expression() {
    let p = parse("fun main() {\n    val v: i64 = asm(\"mov {0}, 1\", out(\"r\") _)\n}");
    let stmts = body_stmts(&p);
    let Stmt::Let { init, .. } = &stmts[0] else {
        panic!("expected a let statement");
    };
    let Expr::Asm { operands, .. } = init else {
        panic!("expected an asm expression");
    };
    assert_eq!(operands[0].kind, AsmOperandKind::Out);
    assert!(operands[0].operand.is_none());
}

#[test]
fn an_unknown_asm_operand_kind_is_refused_by_name() {
    let e = parse_err("fun main() {\n    asm(\"nop\", sideways(\"r\") 1)\n}");
    assert!(
        e.message.contains("unknown asm operand kind 'sideways'"),
        "got: {}",
        e.message
    );
}

#[test]
fn a_naked_fun_declaration_sets_its_flag() {
    let p = parse("naked fun f() => i64 {\n    asm(\"ret\")\n}\nfun main() {}");
    assert!(p.functions[0].naked);
    assert!(!p.functions[0].unsafe_);
    assert!(!p.functions[1].naked);
}

#[test]
fn naked_and_unsafe_can_be_written_together() {
    let p = parse("pub unsafe naked fun f() => i64 {\n    asm(\"ret\")\n}\nfun main() {}");
    assert!(p.functions[0].naked);
    assert!(p.functions[0].unsafe_);
    assert!(p.functions[0].is_pub);
}

/// Kernel stage 6: `defer EXPR` is a statement, and a block or a call are
/// both expressions. `defer.rs` is what consumes the form, so the parser
/// only has to record it with the action's own span stitched on.
#[test]
fn parses_defer_with_a_call_and_with_a_block() {
    let p = parse(
        "fun main() {\n    defer unlock()\n    defer {\n        print(\"a\")\n        print(\"b\")\n    }\n}",
    );
    let body = match &p.functions[0].body {
        FunctionBody::Block(b) => b,
        _ => panic!("a body block"),
    };
    let Stmt::Defer { expr, span } = &body.stmts[0] else {
        panic!("a defer statement");
    };
    assert!(matches!(expr, Expr::Call { callee, .. } if callee == "unlock"));
    assert!(span.end > span.start);
    let Stmt::Defer { expr, .. } = &body.stmts[1] else {
        panic!("a defer statement");
    };
    assert!(matches!(expr, Expr::Block(b) if b.stmts.len() == 2));
}

/// `break` and `continue` are not keywords (`design/syntax.md (kelp wiki)` 2.3), so a
/// bare one is the loop control statement (`kelp wiki page language/iterators.md`) and a name
/// used in an expression is still a name.
#[test]
fn break_and_continue_stay_ordinary_names_outside_statement_position() {
    let p = parse("fun main() {\n    val break = 2\n    print(break)\n}");
    assert_eq!(p.functions.len(), 1);
    let p = parse("fun main() {\n    val continue = 3\n    print(continue + 1)\n}");
    assert_eq!(p.functions.len(), 1);
}

/// #284: a field with no `val` or `var` is a `val` field. `var` stays
/// explicit, and the `val` spelling parses to the same tree as the bare one.
#[test]
fn a_bare_field_is_a_val_field() {
    let p = parse("datac Point(x: i32, var y: i32, val z: i32)\nfun main() {}");
    let d = &p.datacs[0];
    assert_eq!(d.fields.len(), 3);
    assert_eq!(d.fields[0].name, "x");
    assert!(!d.fields[0].mutable);
    assert!(d.fields[1].mutable);
    assert!(!d.fields[2].mutable);
    let bare = parse("datac P(x: i32)\nfun main() {}");
    let explicit = parse("datac P(val x: i32)\nfun main() {}");
    assert_eq!(bare.datacs[0].fields[0].mutable, explicit.datacs[0].fields[0].mutable);
}

/// #284: an enum payload uses the same field list as a `datac`, so the bare
/// form works there too.
#[test]
fn a_bare_field_works_in_an_enum_payload() {
    let p = parse("enum Shape {\n    Circle(r: f64)\n    Rect(w: f64, var h: f64)\n    Empty\n}\nfun main() {}");
    let e = &p.enums[0];
    assert_eq!(e.variants[0].fields.len(), 1);
    assert!(!e.variants[0].fields[0].mutable);
    assert_eq!(e.variants[1].fields.len(), 2);
    assert!(!e.variants[1].fields[0].mutable);
    assert!(e.variants[1].fields[1].mutable);
}

/// #284: a bare field still needs a name and a type, and a stray token where
/// a field should start keeps the old `'val' or 'var'` message.
#[test]
fn a_bare_field_still_needs_a_name_and_a_type() {
    let e = parse_err("datac P(x)\nfun main() {}");
    assert!(e.message.contains("':'"), "got: {}", e.message);
    let e = parse_err("datac P(: i32)\nfun main() {}");
    assert!(e.message.contains("'val' or 'var'"), "got: {}", e.message);
}

/// #285: a method written inside the datac block is the method the same line
/// after the type gives: the same name, receiver, mode and parameters.
#[test]
fn a_datac_block_method_is_the_method_written_after_the_type() {
    let inside = parse("datac Counter(val id: int, var count: int) {\n    mut bump() {\n        count += 1\n    }\n}\nfun main() {}");
    let after = parse("datac Counter(val id: int, var count: int)\n\nfun mut Counter.bump() {\n    count += 1\n}\nfun main() {}");
    assert_eq!(inside.functions.len(), 2);
    assert_eq!(after.functions.len(), 2);
    let (a, b) = (&inside.functions[0], &after.functions[0]);
    assert_eq!(a.name, "bump");
    assert_eq!(a.name, b.name);
    let (ra, rb) = (a.receiver.as_ref().unwrap(), b.receiver.as_ref().unwrap());
    assert_eq!(ra.type_name, "Counter");
    assert_eq!(ra.type_name, rb.type_name);
    assert!(ra.mutable);
    assert_eq!(ra.mutable, rb.mutable);
    assert_eq!(a.params.len(), b.params.len());
    assert!(!a.is_pub);
}

/// #285: a datac's type parameters are the receiver's type arguments, and a
/// method may take its own.
#[test]
fn a_datac_block_method_takes_the_datac_type_parameters() {
    let p = parse("datac Pair<A, B>(val first: A, val second: B) {\n    keep<U>(x: U) => U = x\n}\nfun main() {}");
    let f = &p.functions[0];
    let r = f.receiver.as_ref().unwrap();
    assert_eq!(r.type_name, "Pair");
    assert_eq!(r.type_args, vec!["A".to_string(), "B".to_string()]);
    assert!(!r.mutable);
    assert_eq!(f.type_params, vec!["U".to_string()]);
}

/// #285: an empty block is a datac with no methods, and a datac with no block
/// parses as before.
#[test]
fn an_empty_datac_block_declares_no_methods() {
    let p = parse("datac Point(val x: int) {\n}\ndatac Other(val y: int)\nfun main() {}");
    assert_eq!(p.datacs.len(), 2);
    assert_eq!(p.functions.len(), 1);
}

/// #285: `fun` and `pub` are refused inside the block, and a field line is
/// refused where a method should start.
#[test]
fn a_datac_block_refuses_fun_pub_and_a_non_method_line() {
    let e = parse_err("datac Counter(var count: int) {\n    fun bump() {\n    }\n}\nfun main() {}");
    assert!(e.message.contains("written without 'fun'"), "got: {}", e.message);
    let e = parse_err("datac Counter(var count: int) {\n    pub bump() {\n    }\n}\nfun main() {}");
    assert!(e.message.contains("cannot be 'pub'"), "got: {}", e.message);
    let e = parse_err("datac Counter(var count: int) {\n    var total: int\n}\nfun main() {}");
    assert!(e.message.contains("declares methods"), "got: {}", e.message);
}

#[test]
fn a_pub_group_marks_each_item_as_pub() {
    let p = parse(
        "pub {\n    const LIMIT: i32 = 10\n\n    datac Point(val x: i32)\n\n    enum Shape { Empty }\n\n    fun a() => i32 = 1\n\n    fun b() => i32 = 2\n\n    extern fun g() => i32\n}\n\nfun hidden() => i32 = 4\nfun main() {}",
    );
    assert!(p.consts[0].is_pub);
    assert!(p.datacs[0].is_pub);
    assert!(p.enums[0].is_pub);
    assert!(p.functions[0].is_pub);
    assert!(p.functions[1].is_pub);
    assert!(p.functions[2].is_pub);
    assert!(p.functions[2].extern_);
    assert!(!p.functions[3].is_pub);
}

#[test]
fn a_pub_group_ends_at_its_closing_brace() {
    let p = parse("pub {\n    fun a() => i32 = 1\n}\nfun b() => i32 = 2\nfun main() {}");
    assert!(p.functions[0].is_pub);
    assert!(!p.functions[1].is_pub);
}

#[test]
fn an_empty_pub_group_is_refused() {
    let e = parse_err("pub {\n}\nfun main() {}");
    assert!(e.message.contains("needs at least one declaration"), "got: {}", e.message);
}

#[test]
fn an_unclosed_pub_group_is_refused() {
    let e = parse_err("pub {\n    fun a() => i32 = 1\n");
    assert!(e.message.contains("is not closed"), "got: {}", e.message);
}

#[test]
fn pub_inside_a_pub_group_is_refused() {
    let e = parse_err("pub {\n    pub fun a() => i32 = 1\n}\nfun main() {}");
    assert!(e.message.contains("already marks each item"), "got: {}", e.message);
}

#[test]
fn a_pub_group_inside_a_pub_group_is_refused() {
    let e = parse_err("pub {\n    pub {\n        fun a() => i32 = 1\n    }\n}\nfun main() {}");
    assert!(e.message.contains("cannot be nested"), "got: {}", e.message);
}

#[test]
fn a_statement_inside_a_pub_group_is_refused() {
    let e = parse_err("pub {\n    val x = 1\n}\nfun main() {}");
    assert!(e.message.contains("holds declarations only"), "got: {}", e.message);
}

#[test]
fn a_pd_inside_a_pub_group_is_refused_by_name() {
    let e = parse_err("pub {\n    pd val x = 1\n}\nfun main() {}");
    assert!(e.message.contains("a 'pd' cannot be 'pub'"), "got: {}", e.message);
}

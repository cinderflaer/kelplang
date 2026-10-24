use super::*;
use crate::lexer::Lexer;

pub(super) fn parse(src: &str) -> Program {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    Parser::new(tokens).parse_program().expect("parse failed")
}

pub(super) fn parse_err(src: &str) -> ParseError {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    Parser::new(tokens).parse_program().expect_err("expected a parse error")
}

/// Kernel stage 2: a `repr(...)` clause before a `datac` or an `enum`, the
/// two spellings of its position relative to `pub`, and the parse-time
/// refusal of an alignment that is not a power of two.
#[test]
fn parses_a_repr_clause_before_a_datac_and_an_enum() {
    let p = parse(
        "repr(c) datac A(val x: i32)\nrepr(packed) datac B(val x: i32)\nrepr(align(16)) datac C(val x: i32)\npub repr(align(8)) datac D(val x: i32)\nrepr(packed) pub datac E(val x: i32)\nrepr(u8) enum M {\n    Off = 0\n    On = 1\n}\nfun main() {}",
    );
    assert_eq!(p.datacs[0].repr, Some(Repr::C));
    assert_eq!(p.datacs[1].repr, Some(Repr::Packed));
    assert_eq!(p.datacs[2].repr, Some(Repr::Align(16)));
    assert_eq!(p.datacs[3].repr, Some(Repr::Align(8)));
    assert!(p.datacs[3].is_pub);
    assert_eq!(p.datacs[4].repr, Some(Repr::Packed));
    assert!(p.datacs[4].is_pub);
    assert_eq!(p.enums[0].repr, Some(Repr::Int("u8".to_string())));
    assert_eq!(p.enums[0].variants[0].discriminant, Some(0));
    assert_eq!(p.enums[0].variants[1].discriminant, Some(1));
}

/// A-12 slice 2: a trait is a set of `Self`-receiver signatures, an impl
/// block is a set of `Self`-receiver methods, and a bound is written on a
/// function's own type parameter.
#[test]
fn parses_a_trait_an_impl_and_bounds() {
    let p = parse(
        "trait Shape {\n    fun Self.area() => f64\n    fun mut Self.scale(f: f64)\n}\ndatac C(val r: f64)\nimpl Shape for C {\n    fun Self.area() => f64 = r\n    fun mut Self.scale(f: f64) { r = f }\n}\nfun f<T: Shape + Other>(x: &T) => f64 = x.area()\nfun main() {}",
    );
    assert_eq!(p.traits.len(), 1);
    let t = &p.traits[0];
    assert_eq!(t.name, "Shape");
    assert!(!t.is_pub);
    assert_eq!(t.methods.len(), 2);
    assert_eq!(t.methods[0].name, "area");
    assert_eq!(
        t.methods[0].receiver.as_ref().map(|r| r.type_name.as_str()),
        Some("Self")
    );
    assert!(!t.methods[0].receiver.as_ref().unwrap().mutable);
    assert!(t.methods[1].receiver.as_ref().unwrap().mutable);
    assert!(matches!(t.methods[0].body, FunctionBody::Signature));
    assert_eq!(p.trait_impls.len(), 1);
    assert_eq!(p.trait_impls[0].trait_name, "Shape");
    assert_eq!(p.trait_impls[0].type_name, "C");
    assert_eq!(p.trait_impls[0].methods, vec!["area", "scale"]);
    // The methods of an impl block are ordinary receiver methods, tagged
    // with the site they were written at.
    let impl_methods: Vec<&FunctionDecl> = p
        .functions
        .iter()
        .filter(|f| f.trait_impl.is_some())
        .collect();
    assert_eq!(impl_methods.len(), 2);
    assert_eq!(
        impl_methods[0].receiver.as_ref().map(|r| r.type_name.as_str()),
        Some("Self")
    );
    assert_eq!(
        impl_methods[0]
            .trait_impl
            .as_ref()
            .map(|s| s.self_type.as_str()),
        Some("C")
    );
    // `fun f<T: Shape + Other>`: one parameter, two bounds, in order.
    let f = p.functions.iter().find(|f| f.name == "f").unwrap();
    assert_eq!(f.type_params, vec!["T"]);
    assert_eq!(f.type_param_bounds.len(), 1);
    assert_eq!(f.type_param_bounds[0].len(), 2);
    assert_eq!(f.type_param_bounds[0][0].name, "Shape");
    assert_eq!(f.type_param_bounds[0][1].name, "Other");
}

#[test]
fn parses_default_methods_supertraits_and_builtin_impls() {
    let p = parse(
        "trait Shape {\n    fun Self.area() => f64\n}\ntrait Named: Shape + Other {\n    fun Self.name() => str\n    fun Self.loud() => str { return self.name() }\n}\nimpl Named for i32 {\n    fun Self.name() => str = \"n\"\n}\nfun main() {}",
    );
    assert_eq!(p.traits.len(), 2);
    let named = &p.traits[1];
    assert_eq!(named.supertraits.len(), 2);
    assert_eq!(named.supertraits[0].name, "Shape");
    assert_eq!(named.supertraits[1].name, "Other");
    // `name` is a signature, `loud` is a default with a body.
    assert!(matches!(named.methods[0].body, FunctionBody::Signature));
    assert!(matches!(named.methods[1].body, FunctionBody::Block(_)));
    assert_eq!(p.trait_impls[0].trait_name, "Named");
    assert_eq!(p.trait_impls[0].type_name, "i32");
}

#[test]
fn a_trait_item_that_is_not_a_method_or_an_associated_type_does_not_parse() {
    let err = parse_err("trait Shape { type Item = i32 }\nfun main() {}");
    assert!(err.message.contains("default associated type"), "{}", err.message);
    let err = parse_err("trait Shape { type Item<T> }\nfun main() {}");
    assert!(err.message.contains("generic associated type"), "{}", err.message);
    let err = parse_err("trait Shape { const N: i32 = 1 }\nfun main() {}");
    assert!(err.message.contains("associated consts"), "{}", err.message);
}

#[test]
fn an_associated_type_is_declared_in_a_trait_and_bound_in_an_impl() {
    let p = parse(
        "trait Src {\n    type Out: Named\n    fun Self.produce() => Self::Out\n}\n\nimpl Src for i32 {\n    type Out = i32\n    fun Self.produce() => i32 = self\n}\nfun main() {}",
    );
    let src = &p.traits[0];
    assert_eq!(src.assoc_types.len(), 1);
    assert_eq!(src.assoc_types[0].name, "Out");
    assert_eq!(src.assoc_types[0].bounds[0].name, "Named");
    assert_eq!(src.methods[0].return_type.as_ref().unwrap().name, "Self");
    assert_eq!(
        src.methods[0].return_type.as_ref().unwrap().assoc.as_deref(),
        Some("Out")
    );
    assert_eq!(p.trait_impls[0].assoc_types[0].name, "Out");
    assert_eq!(p.trait_impls[0].assoc_types[0].ty.name, "i32");
}

#[test]
fn a_trait_takes_parameters_and_drop_does_not_parse() {
    // A trait may take type parameters (A-12 slice 6).
    let p = parse("trait A<T> { fun Self.m() }\nfun main() {}");
    assert_eq!(p.traits[0].type_params, vec!["T"]);
    let err = parse_err("trait Drop { fun Self.m() }\nfun main() {}");
    assert!(err.message.contains("'Drop' is built in"), "{}", err.message);
    // `trait A: B` no longer parses as an error: it is a supertrait.
    let p = parse("trait B { fun Self.m() }\ntrait A: B { fun Self.m() }\nfun main() {}");
    assert_eq!(p.traits[1].supertraits[0].name, "B");
}

#[test]
fn a_bound_on_a_receivers_type_parameter_does_not_parse() {
    let err = parse_err("datac Pair<A, B>(val first: A, val second: B)\nfun Pair<A: Shape, B>.first() => A = first\nfun main() {}");
    assert!(err.message.contains("bound cannot be written"), "{}", err.message);
}

#[test]
fn a_bound_on_a_type_declarations_parameter_does_not_parse() {
    let err = parse_err("datac Pair<A: Shape>(val first: A)\nfun main() {}");
    assert!(err.message.contains("bound cannot be written"), "{}", err.message);
}

#[test]
fn an_impl_static_method_parses_and_a_wrong_receiver_does_not() {
    // A receiverless impl method is a static member (A-12 slice 6).
    let p = parse("trait Shape { fun Self.area() => f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun area() => f64 = r }\nfun main() {}");
    assert!(
        p.functions
            .iter()
            .any(|f| f.name == "area" && f.receiver.is_none() && f.trait_impl.is_some()),
        "a static impl method should parse"
    );
    let err = parse_err("trait Shape { fun Self.area() => f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun C.area() => f64 = r }\nfun main() {}");
    assert!(err.message.contains("receiver is written 'Self'"), "{}", err.message);
    let err = parse_err("trait Shape { fun Self.area() => f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun Self<A>.area() => f64 = r }\nfun main() {}");
    assert!(err.message.contains("cannot take type parameters"), "{}", err.message);
}

#[test]
fn self_outside_a_trait_or_an_impl_does_not_parse() {
    let err = parse_err("fun Self.area() => f64 = 0.0\nfun main() {}");
    assert!(err.message.contains("inside a 'trait' or an 'impl'"), "{}", err.message);
}

#[test]
fn an_impl_for_a_bare_generic_does_not_parse() {
    let err = parse_err("trait Shape { fun Self.area() => f64 }\nimpl Shape for List<i32> { fun Self.area() => f64 = 0.0 }\nfun main() {}");
    assert!(err.message.contains("an impl for a generic type writes the type's parameters"), "{}", err.message);
}

#[test]
fn a_bare_dyn_is_a_mark_and_impl_in_argument_position_is_desugared() {
    // A bare `dyn Name` parses as a mark so `Box<dyn Name>` can reach the
    // type checker (A-12 slice 7); the checker refuses the bare spelling
    // wherever it is not a `Box` argument, and `tests/fixtures/traits_dyn.kp`
    // pins that message.
    let p = parse("trait Shape { fun Self.area() => f64 }\nfun take(s: dyn Shape) {}\nfun main() {}");
    let ty = &p.functions[0].params[0].ty;
    assert_eq!(ty.dyn_trait.as_deref(), Some("Shape"));
    assert!(!ty.amp);
    // `Box<dyn A + B>` is refused where the `+` is: a trait object names one
    // trait.
    let err = parse_err("trait A { fun Self.a() => i32 }\ntrait B { fun Self.b() => i32 }\nfun take(s: Box<dyn A + B>) {}\nfun main() {}");
    assert!(err.message.contains("names one trait"), "{}", err.message);
    // A-20: `impl Shape` now parses as a *parameter* type and is
    // desugared into a fresh bounded parameter; a stored position (`let`,
    // a field, a return) is refused by the type checker.
    let p = parse("trait Shape { fun Self.area() => f64 }\nfun take(s: impl Shape) {}\nfun main() {}");
    assert_eq!(p.functions[0].params[0].ty.impl_trait, None);
    assert_eq!(p.functions[0].type_params.len(), 1);
    assert_eq!(p.functions[0].type_param_bounds[0][0].name, "Shape");
}

#[test]
fn parses_enum_discriminants_including_a_negative_one() {
    let p = parse("enum E {\n    A = -5\n    B\n    C = 7\n    D\n}\nfun main() {}");
    let e = &p.enums[0];
    assert_eq!(e.variants[0].discriminant, Some(-5));
    assert_eq!(e.variants[1].discriminant, None);
    assert_eq!(e.variants[2].discriminant, Some(7));
    assert_eq!(e.variants[3].discriminant, None);
}

#[test]
fn an_alignment_that_is_not_a_power_of_two_does_not_parse() {
    let err = parse_err("repr(align(3)) datac A(val x: i32)\nfun main() {}");
    assert!(err.message.contains("power of two"), "{}", err.message);
}

#[test]
fn an_unknown_repr_does_not_parse() {
    let err = parse_err("repr(weird) datac A(val x: i32)\nfun main() {}");
    assert!(err.message.contains("unknown 'repr(weird)'"), "{}", err.message);
}

#[test]
fn a_repr_clause_on_a_non_type_item_does_not_parse() {
    let err = parse_err("repr(c) fun main() {}");
    assert!(err.message.contains("applies to a 'datac' or an 'enum'"), "{}", err.message);
}

#[test]
fn a_section_clause_is_read_on_a_const_and_on_a_fun() {
    let p = parse(
        "section(\".rodata.boot\") const K: [u8; 2] = [1, 2]\n\
         section(\".text.boot\") fun boot() {}\n\
         pub section(\".x\") fun h() => i32 = 1\n",
    );
    assert_eq!(p.consts[0].section.as_deref(), Some(".rodata.boot"));
    assert_eq!(p.functions[0].section.as_deref(), Some(".text.boot"));
    assert_eq!(p.functions[1].section.as_deref(), Some(".x"));
}

#[test]
fn a_section_clause_on_a_non_const_or_fun_does_not_parse() {
    let err = parse_err("section(\".x\") datac A(val x: i32)");
    assert!(
        err.message.contains("'const', 'static' or 'fun'"),
        "{}",
        err.message
    );
}

#[test]
fn parses_static_declarations() {
    let p = parse(
        "static A: i32 = 1\n\
         static mut B: [u8; 2] = [1, 2]\n\
         pub section(\".data.boot\") static mut C: u32 = 0\n",
    );
    assert_eq!(p.statics.len(), 3);
    assert_eq!(p.statics[0].name, "A");
    assert!(!p.statics[0].mutable);
    assert_eq!(p.statics[1].name, "B");
    assert!(p.statics[1].mutable);
    assert!(p.statics[2].is_pub);
    assert!(p.statics[2].mutable);
    assert_eq!(p.statics[2].section.as_deref(), Some(".data.boot"));
}

#[test]
fn parses_a_percpu_static_beside_pub_and_section() {
    let p = parse(
        "percpu static mut A: i32 = 1\n\
         pub percpu static mut B: u32 = 2\n\
         section(\".percpu.boot\") percpu static mut C: u8 = 3\n",
    );
    assert_eq!(p.statics.len(), 3);
    assert!(p.statics[0].percpu && p.statics[0].mutable);
    assert!(p.statics[1].percpu && p.statics[1].is_pub);
    assert!(p.statics[2].percpu);
    assert_eq!(p.statics[2].section.as_deref(), Some(".percpu.boot"));
}

#[test]
fn a_percpu_that_does_not_precede_a_static_does_not_parse() {
    let err = parse_err("percpu fun f() {}");
    assert!(
        err.message.contains("'percpu' applies to a 'static'"),
        "{}",
        err.message
    );
}

#[test]
fn a_section_clause_on_an_extern_fun_does_not_parse() {
    let err = parse_err("section(\".x\") extern fun f(n: i32) => i32");
    assert!(
        err.message.contains("no code of its own to place"),
        "{}",
        err.message
    );
}

#[test]
fn an_empty_section_name_does_not_parse() {
    let err = parse_err("section(\"\") fun f() {}");
    assert!(err.message.contains("needs a section name"), "{}", err.message);
}

#[test]
fn an_align_clause_is_read_on_a_fun() {
    let p = parse(
        "align(4) fun h() {}\n\
         pub section(\".text.trap\") align(4096) fun trap() {}\n\
         align(2) section(\".text.other\") fun boot() {}\n",
    );
    assert_eq!(p.functions[0].align, Some(4));
    assert_eq!(p.functions[1].align, Some(4096));
    assert_eq!(p.functions[1].section.as_deref(), Some(".text.trap"));
    assert_eq!(p.functions[2].align, Some(2));
    assert_eq!(p.functions[2].section.as_deref(), Some(".text.other"));
}

#[test]
fn an_align_that_is_not_a_power_of_two_up_to_4096_does_not_parse() {
    for src in ["align(3) fun f() {}", "align(8192) fun f() {}"] {
        let err = parse_err(src);
        assert!(
            err.message.contains("power of two up to 4096"),
            "{src}: {}",
            err.message
        );
    }
}

#[test]
fn an_align_clause_on_a_const_does_not_parse() {
    let err = parse_err("align(4) const K: i32 = 1");
    assert!(
        err.message.contains("applies to a top-level 'fun'"),
        "{}",
        err.message
    );
}

#[test]
fn an_align_clause_on_an_extern_fun_does_not_parse() {
    let err = parse_err("align(4) extern fun f(n: i32) => i32");
    assert!(
        err.message.contains("no code of its own to align"),
        "{}",
        err.message
    );
}

#[test]
fn the_return_type_takes_the_arrow_and_its_alias() {
    // `=>` is the spelling and `->` is an alias of it. A `fun` literal and a
    // function type take the same two forms.
    for src in ["fun f(a: i32) => i32 = a\n", "fun f(a: i32) -> i32 = a\n"] {
        let p = parse(src);
        let ret = p.functions[0].return_type.as_ref().expect("a return type");
        assert_eq!(ret.name, "i32", "{src}");
    }
    parse("fun main() {\n    val f = fun(x: i32) => i32 = x\n    val g = fun(x: i32) -> i32 = x\n}\n");
    parse("fun g(f: fun(i32) => i32, h: fun(i32) -> i32) {}\n");
}

#[test]
fn the_retired_return_marker_is_refused_by_name() {
    // `.return:` is refused at the `return` token, in declarations, fun
    // literals and function types alike, and the message names `=>`.
    for src in [
        "fun f(a: i32).return: i32 = a\n",
        "fun main() {\n    val f = fun(x: i32).return: i32 = x\n}\n",
        "fun g(f: fun(i32).return: i32) {}\n",
    ] {
        let err = parse_err(src);
        assert!(err.message.contains("'.return:' is no longer accepted"), "{src}: {}", err.message);
        assert!(err.message.contains("'=>'"), "{src}: {}", err.message);
    }
}

#[test]
fn parses_empty_block_bodied_function() {
    let p = parse("fun main() {}");
    assert_eq!(p.functions.len(), 1);
    assert_eq!(p.functions[0].name, "main");
    assert!(p.functions[0].params.is_empty());
    assert!(p.functions[0].return_type.is_none());
    assert!(matches!(p.functions[0].body, FunctionBody::Block(_)));
}

#[test]
fn a_tuple_type_is_its_element_list() {
    // MISSING-z2bB9: `(A, B)` is a type, and `(A)` is not a tuple but the
    // parenthesized type `A` (`design/syntax.md (kelp wiki)` 3).
    let p = parse("fun f(p: (i32, String)) => (i32, i32) = (1, 2)");
    let param = &p.functions[0].params[0].ty;
    assert_eq!(param.display(), "(i32, String)");
    assert_eq!(param.tuple.as_ref().unwrap().len(), 2);
    let ret = p.functions[0].return_type.as_ref().unwrap();
    assert_eq!(ret.display(), "(i32, i32)");

    let paren = parse("fun f() => (i32) = 1");
    let ret = paren.functions[0].return_type.as_ref().unwrap();
    assert!(ret.tuple.is_none(), "(T) is the parenthesized type T");
    assert_eq!(ret.display(), "i32");

    let nested = parse("fun f(p: ((i32, i32), i32)) {}");
    assert_eq!(
        nested.functions[0].params[0].ty.display(),
        "((i32, i32), i32)"
    );
}

#[test]
fn an_empty_tuple_type_is_the_unit_type() {
    // `()` as a type is `unit` (Gitea #223), the same type the value `()` has.
    let p = parse("fun f(p: ()) {}");
    let f = &p.functions[0];
    assert_eq!(f.params[0].ty.name, "unit");
    assert!(f.params[0].ty.tuple.is_none());
}

#[test]
fn a_tuple_literal_needs_two_elements() {
    // `(e)` is the parenthesized expression it always was, and a tuple
    // needs a comma, so one element is never a tuple.
    let p = parse("fun f() => i32 {\n    val a = (1)\n    return a\n}");
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::IntLit(1, _)), "got {init:?}");
            }
            other => panic!("expected a let, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }

    let p = parse("fun f() {\n    val p = (1, 2)\n}");
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Let { init, .. } => match init {
                Expr::TupleLit { elems, .. } => assert_eq!(elems.len(), 2),
                other => panic!("expected a tuple literal, got {other:?}"),
            },
            other => panic!("expected a let, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }

    // `()` is the unit value: an empty block, which evaluates to `unit`.
    let p = parse("fun f() {\n    val u = ()\n}");
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Let { init, .. } => match init {
                Expr::Block(unit) => assert!(unit.stmts.is_empty(), "got {init:?}"),
                other => panic!("expected the unit value, got {other:?}"),
            },
            other => panic!("expected a let, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }
}

#[test]
fn a_destructuring_binding_binds_every_position() {
    // MISSING-z2bB9: `val (lo, _) = p`, with `_` ignoring a position and a
    // nested pattern allowed (`design/syntax.md (kelp wiki)` 4).
    let p = parse("fun f(p: (i32, i32)) {\n    val (lo, _) = p\n    var ((a, b), c) = ((1, 2), 3)\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        other => panic!("expected a block body, got {other:?}"),
    };
    assert_eq!(stmts.len(), 2);
    let Stmt::LetTuple {
        pattern, mutable, ..
    } = &stmts[0]
    else {
        panic!("expected a destructuring binding, got {:?}", stmts[0]);
    };
    assert!(!*mutable);
    let BindingPattern::Tuple(positions) = pattern else {
        panic!("expected a tuple pattern, got {pattern:?}");
    };
    assert!(matches!(positions[0], BindingPattern::Name(ref n) if n == "lo"));
    assert!(matches!(positions[1], BindingPattern::Ignore));

    let Stmt::LetTuple {
        pattern, mutable, ..
    } = &stmts[1]
    else {
        panic!("expected a destructuring binding, got {:?}", stmts[1]);
    };
    assert!(*mutable, "`var (a, b) = ...` makes every position a `var`");
    let mut names = Vec::new();
    pattern.binds(&mut Vec::new(), &mut names);
    assert_eq!(
        names.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
    assert_eq!(names[1].1, vec![0, 1], "`b` is element 2 of element 1");
}

#[test]
fn a_destructuring_binding_needs_two_positions_and_no_written_type() {
    let err = parse_err("fun f() {\n    val (a) = 1\n}");
    assert!(
        err.message.contains("needs two or more positions"),
        "{}",
        err.message
    );
    let err = parse_err("fun f() {\n    val (a, b): (i32, i32) = (1, 2)\n}");
    assert!(
        err.message.contains("takes no written type"),
        "{}",
        err.message
    );
}

#[test]
fn parses_params_and_return_clause() {
    let p = parse("fun add(a: i32, b: i32) => i32 {\n    return a + b\n}");
    let f = &p.functions[0];
    assert_eq!(f.params.len(), 2);
    assert_eq!(f.params[0].name, "a");
    assert_eq!(f.params[0].ty.name, "i32");
    assert_eq!(f.return_type.as_ref().unwrap().name, "i32");
    match &f.body {
        FunctionBody::Block(b) => {
            assert_eq!(b.stmts.len(), 1);
            match &b.stmts[0] {
                Stmt::Return { value: Some(e), .. } => {
                    assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. }));
                }
                other => panic!("expected return stmt, got {other:?}"),
            }
        }
        other => panic!("expected block body, got {other:?}"),
    }
}

/// The number of links in an `if` chain: one per `if`/`elif`, plus one for
/// a trailing `else` block. Both spellings have to give the same number,
/// which is the whole claim `elif` makes (`design/syntax.md (kelp wiki)` 6.1).
fn links(branch: &ElseBranch) -> usize {
    match branch {
        ElseBranch::If(e) => match e {
            Expr::If { else_block, .. } => {
                1 + else_block.as_deref().map(links).unwrap_or(0)
            }
            other => panic!("expected an if, got {other:?}"),
        },
        ElseBranch::Block(_) => 1,
    }
}

/// The chain of the first statement of the first function in `src`.
fn chain(src: &str) -> usize {
    let p = parse(src);
    match &p.functions[0].body {
        FunctionBody::Block(b) => match &b.stmts[0] {
            Stmt::Expr { expr, .. } => match expr {
                Expr::If { else_block, .. } => {
                    1 + else_block.as_deref().map(links).unwrap_or(0)
                }
                other => panic!("expected an if, got {other:?}"),
            },
            other => panic!("expected an expression statement, got {other:?}"),
        },
        other => panic!("expected a block body, got {other:?}"),
    }
}

#[test]
fn elif_is_the_same_link_as_else_if() {
    let spelled_out = "fun f(n: i32) => i32 {\n    if n > 2 { return 1 } else if n > 1 { return 2 } else { return 3 }\n}";
    let spelled_elif = "fun f(n: i32) => i32 {\n    if n > 2 { return 1 } elif n > 1 { return 2 } else { return 3 }\n}";
    assert_eq!(chain(spelled_out), 3);
    assert_eq!(chain(spelled_elif), chain(spelled_out));
}

#[test]
fn an_if_chain_may_mix_both_spellings() {
    let mixed = "fun f(n: i32) => i32 {\n    if n > 3 { return 1 } else if n > 2 { return 2 } elif n > 1 { return 3 } else { return 4 }\n}";
    assert_eq!(chain(mixed), 4);
}

#[test]
fn elif_needs_no_else_after_it() {
    let src = "fun f(n: i32) => i32 {\n    if n > 1 { return 1 } elif n > 0 { return 2 }\n}";
    assert_eq!(chain(src), 2);
}

#[test]
fn elif_let_is_the_chained_pattern_link() {
    // `elif let PAT = e { ... }` is `else if let PAT = e { ... }` in the
    // preferred spelling, so it goes through the same desugaring: a block
    // whose `when` carries the link.
    let after_a_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer) => i32 {\n    if let Yes(n) = m {\n        return n\n    } elif let No = m {\n        return 0\n    }\n    return 1\n}";
    assert_eq!(parse(after_a_pattern).functions.len(), 1);
    // And a chain may leave the pattern form for a boolean one, either
    // way round.
    let leaving_the_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer) => i32 {\n    if let Yes(n) = m {\n        return n\n    } elif n > 1 {\n        return 2\n    }\n    return 1\n}";
    assert_eq!(parse(leaving_the_pattern).functions.len(), 1);
}

#[test]
fn an_elif_that_follows_nothing_is_an_error() {
    parse_err("fun f(n: i32) {\n    elif n > 1 { }\n}");
}

#[test]
fn elsif_is_not_a_spelling() {
    parse_err("fun f(n: i32) {\n    if n > 1 { } elsif n > 0 { }\n}");
}

#[test]
fn parses_expression_bodied_function() {
    let p = parse("fun add(a: i32, b: i32) => i32 = a + b");
    match &p.functions[0].body {
        FunctionBody::Expr(e) => assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. })),
        other => panic!("expected expr body, got {other:?}"),
    }
}

#[test]
fn expression_body_without_return_type_parses_with_no_written_type() {
    // Gitea #280: the return type is inferred from the body later, so the
    // parser keeps the body and leaves the type unwritten.
    let p = parse("fun add(a: i32) = a");
    assert!(p.functions[0].return_type.is_none());
    assert!(matches!(p.functions[0].body, FunctionBody::Expr(_)));
}

#[test]
fn parses_val_and_var() {
    let p = parse("fun main() {\n    val x = 1\n    var y: i32 = 2\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    assert_eq!(stmts.len(), 2);
    match &stmts[0] {
        Stmt::Let { name, mutable, ty, init, .. } => {
            assert_eq!(name, "x");
            assert!(!mutable);
            assert!(ty.is_none());
            assert!(matches!(init, Expr::IntLit(1, _)));
        }
        other => panic!("expected let stmt, got {other:?}"),
    }
    match &stmts[1] {
        Stmt::Let { name, mutable, ty, .. } => {
            assert_eq!(name, "y");
            assert!(*mutable);
            assert_eq!(ty.as_ref().unwrap().name, "i32");
        }
        other => panic!("expected let stmt, got {other:?}"),
    }
}

#[test]
fn newline_terminates_statements_without_semicolons() {
    let p = parse("fun main() {\n    val x = 1\n    val y = 2\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    assert_eq!(stmts.len(), 2);
}

#[test]
fn two_statements_on_one_line_without_separator_is_an_error() {
    parse_err("fun main() {\n    val x = 1 val y = 2\n}");
}

#[test]
fn respects_operator_precedence() {
    let p = parse("fun main() {\n    val x = 1 + 2 * 3\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::Let { init, .. } => match init {
            Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                assert!(matches!(**lhs, Expr::IntLit(1, _)));
                assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("expected top-level addition, got {other:?}"),
        },
        other => panic!("expected let stmt, got {other:?}"),
    }
}

#[test]
fn parses_if_else_as_expression_and_statement() {
    let p = parse(
        "fun main() {\n    if 1 < 2 {\n        print(1)\n    } else {\n        print(2)\n    }\n    val x = if true { 1 } else { 2 }\n}",
    );
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    assert_eq!(stmts.len(), 2);
    assert!(matches!(stmts[0], Stmt::Expr { expr: Expr::If { .. }, .. }));
    match &stmts[1] {
        Stmt::Let { init, .. } => assert!(matches!(init, Expr::If { .. })),
        other => panic!("expected let stmt, got {other:?}"),
    }
}

#[test]
fn parses_while_with_compound_assignment() {
    let p = parse("fun main() {\n    var i = 0\n    while i < 10 {\n        i += 1\n    }\n}");
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[1] {
        Stmt::While { body, .. } => {
            assert_eq!(body.stmts.len(), 1);
            match &body.stmts[0] {
                Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                    assert!(matches!(**value, Expr::Binary { op: BinOp::Add, .. }));
                }
                other => panic!("expected assign stmt, got {other:?}"),
            }
        }
        other => panic!("expected while stmt, got {other:?}"),
    }
}

#[test]
fn parses_an_enum_and_a_pattern_when() {
    let p = parse(
        "enum Shape {\n    Circle(val r: f64)\n    Rect(val w: f64, val h: f64)\n    Empty\n}\n\ndatac Point(val x: i32)\n\nfun area(s: &Shape) => f64 = when s {\n    Circle(r) -> r\n    Rect(w, h) -> w * h\n    Empty -> 0.0\n}",
    );
    assert_eq!(p.enums.len(), 1);
    let e = &p.enums[0];
    assert_eq!(e.name, "Shape");
    assert_eq!(e.variants.len(), 3);
    assert_eq!(e.variants[0].fields.len(), 1);
    assert_eq!(e.variants[0].fields[0].name, "r");
    assert_eq!(e.variants[1].fields.len(), 2);
    assert!(e.variants[2].fields.is_empty());
    // `datac` and `enum` share one declaration order, which is what
    // `resolve.rs` merges and codegen emits in.
    assert_eq!(e.order, 0);
    assert_eq!(p.datacs[0].order, 1);

    match &p.functions[0].body {
        FunctionBody::Expr(Expr::When { arms, .. }) => {
            assert_eq!(arms.len(), 3);
            let first = arms[0].pattern.as_ref().unwrap();
            assert_eq!(first.variant, "Circle");
            assert_eq!(first.binds, vec![Some("r".to_string())]);
            assert!(arms[2].pattern.as_ref().unwrap().binds.is_empty());
            assert!(matches!(arms[2].body, Expr::FloatLit(..)));
        }
        other => panic!("expected a pattern `when`, got {other:?}"),
    }
}

#[test]
fn an_else_arm_in_a_pattern_when_parses_as_a_catch_all() {
    let p = parse(
        "enum Flag {\n    On\n    Off\n}\n\nfun f(x: &Flag) => i32 = when x {\n    On -> 1\n    else -> 2\n}",
    );
    match &p.functions[0].body {
        FunctionBody::Expr(Expr::When { arms, .. }) => {
            assert!(arms[0].pattern.is_some());
            assert!(arms[1].pattern.is_none());
        }
        other => panic!("expected a pattern `when`, got {other:?}"),
    }
}

#[test]
fn if_let_desugars_to_a_bound_scrutinee_and_a_single_arm_when() {
    // MK5-S6-M1: `if let P = e { A } else { B }` becomes
    // `{ val #iflet0 = e  when #iflet0 { P -> { A } else -> { B } } }`.
    let p = parse(
        "fun main() {\n    if let Circle(r) = Shape.Circle(r = 1.0) {\n        print(r)\n    } else {\n        print(0.0)\n    }\n}",
    );
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    let inner = match &stmts[0] {
        Stmt::Expr { expr: Expr::Block(b), .. } => b,
        other => panic!("expected a desugared block, got {other:?}"),
    };
    match &inner.stmts[0] {
        Stmt::Let { name, init, .. } => {
            assert_eq!(name, "#iflet0");
            assert!(matches!(init, Expr::MethodCall { .. }));
        }
        other => panic!("expected the scrutinee binding, got {other:?}"),
    }
    match &inner.stmts[1] {
        Stmt::Expr { expr: Expr::When { scrutinee, arms, .. }, .. } => {
            assert!(matches!(**scrutinee, Expr::Ident(ref n, _) if n == "#iflet0"));
            assert_eq!(arms.len(), 2);
            assert!(arms[0].pattern.is_some());
            assert!(arms[1].pattern.is_none());
        }
        other => panic!("expected the `when`, got {other:?}"),
    }
}

#[test]
fn a_for_loop_is_kept_as_written_until_its_iterable_has_a_type() {
    // A-5/`kelp wiki page language/iterators.md`: which desugar applies depends on the
    // iterable's type, which only `infer.rs` knows, so the parser keeps the
    // statement and `infer.rs` lowers it.
    let p = parse(
        "fun main() {\n    for x in xs {\n        print(x)\n    }\n}",
    );
    let stmts = match &p.functions[0].body {
        FunctionBody::Block(b) => &b.stmts,
        _ => panic!("expected block body"),
    };
    match &stmts[0] {
        Stmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            assert_eq!(binding, "x");
            assert!(matches!(iterable, Expr::Ident(n, _) if n == "xs"));
            assert_eq!(body.stmts.len(), 1);
        }
        other => panic!("expected the raw 'for', got {other:?}"),
    }
}

#[test]
fn a_bare_break_or_continue_is_a_loop_control_statement() {
    let p = parse("fun main() {\n    while true {\n        break\n    }\n}");
    let outer = match &p.functions[0].body {
        FunctionBody::Block(b) => b,
        _ => panic!("expected block body"),
    };
    let inner = match &outer.stmts[0] {
        Stmt::While { body, .. } => body,
        other => panic!("expected the while, got {other:?}"),
    };
    assert!(matches!(inner.stmts[0], Stmt::Break { .. }));
}

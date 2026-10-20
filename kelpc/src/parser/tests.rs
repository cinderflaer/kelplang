use super::*;
use crate::lexer::Lexer;

fn parse(src: &str) -> Program {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    Parser::new(tokens).parse_program().expect("parse failed")
}

fn parse_err(src: &str) -> ParseError {
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
fn an_empty_tuple_type_is_refused_by_name() {
    let err = parse_err("fun f(p: ()) {}");
    assert!(err.message.contains("'()' is not a type"), "{}", err.message);
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
fn expression_body_without_return_type_is_an_error() {
    parse_err("fun add(a: i32) = a");
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
    // A closure type is a function type under the `closure` keyword - the
    // same parameter list, the same optional `=> T` (A-4 phase E).
    let p = parse("fun use(f: closure(i32) => i32, g: move closure()) {}");
    let f = &p.functions[0];

    let f_param = &f.params[0];
    assert!(f_param.ty.is_closure());
    assert_eq!(f_param.ty.display(), "closure(i32) => i32");
    assert!(!f_param.mode.moves());

    // A bare closure parameter borrows the environment (an owned type's
    // parameter is a borrow); `move closure(...)` takes it over.
    let g_param = &f.params[1];
    assert!(g_param.ty.is_closure());
    assert!(g_param.mode.moves());
    assert_eq!(g_param.ty.display(), "closure()");
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

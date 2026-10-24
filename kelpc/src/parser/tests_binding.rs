use super::tests::{parse, parse_err};
use crate::ast::{Expr, FunctionBody};

fn find<'a>(p: &'a crate::ast::Program, name: &str) -> &'a crate::ast::FunctionDecl {
    p.functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function named {name}"))
}

#[test]
fn a_binding_yields_a_private_extern_and_a_wrapper_that_calls_it() {
    let p = parse("extern fun read(path: &str) => Bytes = \"read_raw\"\nfun main() {}");
    let ext = find(&p, "read_raw");
    assert!(ext.extern_);
    assert!(!ext.is_pub);
    assert!(matches!(ext.body, FunctionBody::Extern));
    assert_eq!(ext.params.len(), 1);

    let wrapper = find(&p, "read");
    assert!(!wrapper.extern_);
    assert!(!wrapper.is_pub);
    assert_eq!(wrapper.params.len(), 1);
    assert_eq!(wrapper.params[0].name, "path");
    match &wrapper.body {
        FunctionBody::Expr(Expr::Call { callee, args, .. }) => {
            assert_eq!(callee, "read_raw");
            assert_eq!(args.len(), 1);
            assert!(matches!(&args[0].value, Expr::Ident(n, _) if n == "path"));
        }
        _ => panic!("the wrapper of a binding is a call to its C symbol"),
    }
}

#[test]
fn a_binding_passes_every_parameter_in_declaration_order() {
    let p = parse("extern fun f(a: i32, b: i32, c: i32) => i32 = \"g\"\nfun main() {}");
    let wrapper = find(&p, "f");
    match &wrapper.body {
        FunctionBody::Expr(Expr::Call { callee, args, .. }) => {
            assert_eq!(callee, "g");
            let names: Vec<&str> = args
                .iter()
                .map(|a| match &a.value {
                    Expr::Ident(n, _) => n.as_str(),
                    _ => panic!("each argument of a binding is a parameter"),
                })
                .collect();
            assert_eq!(names, ["a", "b", "c"]);
        }
        _ => panic!("the wrapper of a binding is a call to its C symbol"),
    }
}

#[test]
fn pub_on_a_binding_is_read_on_the_wrapper_only() {
    let p = parse("pub extern fun read(path: &str) => Bytes = \"read_raw\"\nfun main() {}");
    assert!(find(&p, "read").is_pub);
    assert!(!find(&p, "read_raw").is_pub);
}

#[test]
fn a_binding_without_a_return_type_does_not_parse() {
    let err = parse_err("extern fun f(x: i32) = \"g\"");
    assert!(err.message.contains("return type"), "{}", err.message);
}

#[test]
fn a_variadic_binding_does_not_parse() {
    let err = parse_err("extern fun f(x: i32, ...) => i32 = \"g\"");
    assert!(err.message.contains("variadic"), "{}", err.message);
}

#[test]
fn a_generic_binding_does_not_parse() {
    let err = parse_err("extern fun f<T>(x: T) => T = \"g\"");
    assert!(err.message.contains("type parameter"), "{}", err.message);
}

#[test]
fn a_symbol_that_is_not_a_c_identifier_does_not_parse() {
    let err = parse_err("extern fun f(x: i32) => i32 = \"not a name\"");
    assert!(err.message.contains("C identifier"), "{}", err.message);
    let err = parse_err("extern fun f(x: i32) => i32 = \"\"");
    assert!(err.message.contains("C identifier"), "{}", err.message);
}

#[test]
fn a_binding_that_names_itself_does_not_parse() {
    let err = parse_err("extern fun f(x: i32) => i32 = \"f\"");
    assert!(err.message.contains("its own symbol"), "{}", err.message);
}

#[test]
fn an_extern_with_a_non_string_after_equals_keeps_its_no_body_error() {
    let err = parse_err("extern fun f(x: i32) => i32 = x");
    assert!(err.message.contains("no body"), "{}", err.message);
}

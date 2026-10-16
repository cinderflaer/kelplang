//! `derive(eq, hash)` (design/syntax.md (kelp wiki) 12.2): a `datac` or `enum` with a
//! `derive(...)` line above it gets an ordinary `fun T.eq(other: &T)` and
//! `fun T.hash()`. The methods are written as Kelp source here and parsed by
//! the same parser as the rest of the file, so the declared-pair machinery
//! (A-12) reaches them unchanged: a `Map` key, `==` and `!=` all use them the
//! way they use a hand-written pair.

use std::collections::HashSet;

use crate::ast::*;
use crate::lexer::{Lexer, Span};
use crate::parser::{ParseError, Parser};

/// One `derive(...)` line, bound to the `datac` or `enum` written under it.
#[derive(Debug, Clone)]
pub struct DeriveRequest {
    pub type_name: String,
    /// The requested names in source order, each with its span.
    pub names: Vec<(String, Span)>,
    /// The span of the `derive` clause, where refusals about the type point.
    pub span: Span,
}

/// The type a field or payload is, as far as `derive` can check it.
enum Kind {
    /// `i32`, `u64`, `usize` and the rest of the integers.
    Int,
    Bool,
    Char,
    /// `f32` and `f64`: equal by `==`, but not hashable.
    Float,
    /// `String` or `Bytes`: compared and hashed through `view()`.
    Text,
    /// A name that is not a builtin: a `datac`/`enum` of this file, or a
    /// qualified name from another module.
    User(String),
    /// A `List`, `Box`, `Map`, tuple, array, or anything else.
    Other,
}

fn mentions(ty: &TypeExpr, params: &[String]) -> bool {
    params.iter().any(|p| p == &ty.name)
        || ty.tuple.as_ref().is_some_and(|ts| ts.iter().any(|t| mentions(t, params)))
        || ty.args.iter().any(|t| mentions(t, params))
        || ty.array_elem.as_ref().is_some_and(|t| mentions(t, params))
}

fn classify(ty: &TypeExpr) -> Kind {
    if ty.tuple.is_some() || ty.array_elem.is_some() || !ty.args.is_empty() || ty.assoc.is_some() {
        return Kind::Other;
    }
    match ty.name.as_str() {
        "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" => Kind::Int,
        "bool" => Kind::Bool,
        "char" => Kind::Char,
        "f32" | "f64" => Kind::Float,
        "String" | "Bytes" => Kind::Text,
        "" => Kind::Other,
        other => Kind::User(other.to_string()),
    }
}

fn type_text(ty: &TypeExpr) -> String {
    if let Some(ts) = &ty.tuple {
        let inner: Vec<String> = ts.iter().map(type_text).collect();
        return format!("({})", inner.join(", "));
    }
    if let Some(elem) = &ty.array_elem {
        return format!("[{}; N]", type_text(elem));
    }
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    let inner: Vec<String> = ty.args.iter().map(type_text).collect();
    format!("{}<{}>", ty.name, inner.join(", "))
}

/// Everything the expansion knows about the file's own types.
struct Scope {
    /// `(type, method)` for every `eq` and `hash` the file declares by hand
    /// or derives, which is what makes a field's type checkable.
    methods: HashSet<(String, String)>,
    /// Names of every `datac` and `enum` declared in this file.
    local_types: HashSet<String>,
}

impl Scope {
    fn has(&self, ty: &str, method: &str) -> bool {
        self.methods.contains(&(ty.to_string(), method.to_string()))
    }
}

/// The refusal for a field the method cannot handle, or `None` when the field
/// can be compared (`eq`) or hashed (`hash`) as written.
fn refusal(scope: &Scope, owner: &str, field: &str, ty: &TypeExpr, method: &str) -> Option<String> {
    let text = type_text(ty);
    let verb = if method == "eq" { "compare" } else { "hash" };
    match classify(ty) {
        Kind::Int | Kind::Bool | Kind::Char | Kind::Text => None,
        Kind::Float if method == "eq" => None,
        Kind::Float => Some(format!(
            "derive(hash) on '{owner}' cannot hash field '{field}' of type '{text}': a float has no hash, because 'nan != nan' would make an inserted key unfindable. Fix: compare an integer field instead, or declare 'fun {owner}.hash() => u64' and 'fun {owner}.eq(other: &{owner}) => bool' by hand"
        )),
        Kind::User(name) if name.contains('.') => {
            // A qualified name is another module's type; its methods are
            // checked where that module declares them.
            let _ = name;
            None
        }
        Kind::User(name) => {
            if scope.has(&name, method) {
                None
            } else if scope.local_types.contains(&name) {
                Some(format!(
                    "derive({method}) on '{owner}' cannot {verb} field '{field}' of type '{name}': '{name}' has no {method} of its own. Fix: add derive(eq, hash) above '{name}', or declare 'fun {name}.{method}' by hand"
                ))
            } else {
                Some(format!(
                    "derive({method}) on '{owner}' cannot {verb} field '{field}' of type '{name}': '{name}' is not declared in this file, so it has no {method} that derive can call. Fix: declare 'fun {owner}.{method}' by hand"
                ))
            }
        }
        Kind::Other => Some(format!(
            "derive({method}) on '{owner}' cannot {verb} field '{field}' of type '{text}': derive does not {verb} that type, and it has no {method} of its own. Fix: declare 'fun {owner}.{method}' by hand"
        )),
    }
}

/// The Kelp statement that refuses two unequal fields in `eq`.
fn eq_step(kind: &Kind, a: &str, b: &str) -> String {
    match kind {
        Kind::Text => format!("    if {a}.view() != {b}.view() {{\n        return false\n    }}\n"),
        _ => format!("    if {a} != {b} {{\n        return false\n    }}\n"),
    }
}

/// The Kelp statements that fold one field into `h` for `hash`. `k` keeps the
/// local names of the fold unique within one function.
fn hash_step(kind: &Kind, access: &str, k: usize) -> String {
    match kind {
        Kind::Int => format!("h = h * 31 + {access}.to_u64()\n"),
        Kind::Bool => format!("if {access} {{\nh = h * 31 + 1\n}} else {{\nh = h * 31 + 2\n}}\n"),
        // The language has no `char` to integer conversion, and a template is not
        // a value, so a `char` adds a fixed step: equal chars still hash equally,
        // and keys that differ only in a `char` share a hash that `eq` separates.
        Kind::Char => "h = h * 31\n".to_string(),
        Kind::Text => hash_text(access, k),
        Kind::Float | Kind::User(_) | Kind::Other => format!("h = h * 31 + {access}.hash()\n"),
    }
}

/// Folds the bytes of a `String` or `Bytes` value, then its length, into `h`.
fn hash_text(access: &str, k: usize) -> String {
    format!(
        "val tv{k} = {access}.view()\nvar ti{k}: usize = 0\nwhile ti{k} < tv{k}.len() {{\nh = h * 31 + tv{k}.get(ti{k}).to_u64()\nti{k} = ti{k} + 1\n}}\nh = h * 31 + tv{k}.len().to_u64()\n"
    )
}

/// The shape of the type a request is written on.
enum Target<'a> {
    Datac(&'a DatacDecl),
    Enum(&'a EnumDecl),
}

/// Builds the Kelp source of the methods one request asks for. Every field is
/// checked before any text is produced, so a refusal names the first field
/// that cannot be handled.
fn method_source(scope: &Scope, req: &DeriveRequest, target: &Target) -> Result<String, ParseError> {
    let (name, params, fields, variants): (&str, &[String], Option<&[FieldDecl]>, Option<&[VariantDecl]>) =
        match target {
            Target::Datac(d) => (&d.name, &d.type_params, Some(&d.fields), None),
            Target::Enum(e) => (&e.name, &e.type_params, None, Some(&e.variants)),
        };
    let generic = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
    let asks = |m: &str| req.names.iter().any(|(n, _)| n == m);
    let check = |ty: &TypeExpr, field: &str, span: Span, method: &str| -> Result<Kind, ParseError> {
        if mentions(ty, params) {
            return Err(ParseError {
                help: None,
                message: format!(
                    "derive({method}) on generic '{name}' cannot check field '{field}' of type '{}': it mentions a type parameter, and derive cannot tell what that is compared or hashed as. Fix: declare 'fun {name}.{method}' by hand",
                    type_text(ty)
                ),
                span,
            });
        }
        if let Some(msg) = refusal(scope, name, field, ty, method) {
            return Err(ParseError { message: msg, span, help: None });
        }
        Ok(classify(ty))
    };

    let mut out = String::new();
    if asks("eq") {
        out.push_str(&format!("fun {name}{generic}.eq(other: &{name}{generic}) => bool {{\n"));
        match (fields, variants) {
            (Some(fields), _) => {
                for f in fields {
                    let kind = check(&f.ty, &f.name, f.span, "eq")?;
                    out.push_str(&eq_step(&kind, &format!("self.{}", f.name), &format!("other.{}", f.name)));
                }
            }
            (_, Some(variants)) => {
                out.push_str("    when self {\n");
                for v in variants {
                    out.push_str(&format!("        {}{} -> {{\n", v.name, binds(v, "a")));
                    out.push_str("            when other {\n");
                    for w in variants {
                        out.push_str(&format!("                {}{} -> {{\n", w.name, binds(w, "b")));
                        if w.name == v.name {
                            if v.fields.is_empty() {
                                out.push_str("                    return true\n");
                            }
                            for (i, f) in v.fields.iter().enumerate() {
                                let kind = check(&f.ty, &f.name, f.span, "eq")?;
                                out.push_str(&eq_step(&kind, &format!("a{i}"), &format!("b{i}")));
                            }
                        } else {
                            out.push_str("                    return false\n");
                        }
                        out.push_str("                }\n");
                    }
                    out.push_str("            }\n        }\n");
                }
                out.push_str("    }\n");
            }
            _ => {}
        }
        out.push_str("    return true\n}\n\n");
    }

    if asks("hash") {
        out.push_str(&format!("fun {name}{generic}.hash() => u64 {{\nvar h: u64 = 0\n"));
        let mut k = 0usize;
        match (fields, variants) {
            (Some(fields), _) => {
                for f in fields {
                    let kind = check(&f.ty, &f.name, f.span, "hash")?;
                    out.push_str(&hash_step(&kind, &format!("self.{}", f.name), k));
                    k += 1;
                }
            }
            (_, Some(variants)) => {
                out.push_str("when self {\n");
                for (vi, v) in variants.iter().enumerate() {
                    out.push_str(&format!("{}{} -> {{\nh = {}\n", v.name, binds(v, "a"), vi + 1));
                    for (i, f) in v.fields.iter().enumerate() {
                        let kind = check(&f.ty, &f.name, f.span, "hash")?;
                        out.push_str(&hash_step(&kind, &format!("a{i}"), k));
                        k += 1;
                        let _ = i;
                    }
                    out.push_str("}\n");
                }
                out.push_str("}\n");
            }
            _ => {}
        }
        out.push_str("return h\n}\n");
    }
    Ok(out)
}

/// The variant's pattern tail: `(a0, a1)` for a payload, empty for a unit
/// variant. `prefix` names the bindings.
fn binds(v: &VariantDecl, prefix: &str) -> String {
    if v.fields.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = (0..v.fields.len()).map(|i| format!("{prefix}{i}")).collect();
        format!("({})", names.join(", "))
    }
}

/// Expands every request into methods, once the file is parsed. A request
/// for a method the type already declares by hand is refused, and the message
/// names the hand-written method.
pub fn expand(
    requests: &[DeriveRequest],
    datacs: &[DatacDecl],
    enums: &[EnumDecl],
    functions: &[FunctionDecl],
) -> Result<Vec<FunctionDecl>, ParseError> {
    let mut methods = HashSet::new();
    for f in functions {
        if let Some(r) = &f.receiver {
            methods.insert((r.type_name.clone(), f.name.clone()));
        }
    }
    for req in requests {
        for (n, _) in &req.names {
            methods.insert((req.type_name.clone(), n.clone()));
        }
    }
    let local_types: HashSet<String> = datacs
        .iter()
        .map(|d| d.name.clone())
        .chain(enums.iter().map(|e| e.name.clone()))
        .collect();
    let scope = Scope { methods, local_types };

    let mut out = Vec::new();
    for req in requests {
        for (n, _) in &req.names {
            let existing = functions.iter().find(|f| {
                f.name == *n && f.receiver.as_ref().is_some_and(|r| r.type_name == req.type_name)
            });
            if let Some(f) = existing {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "derive({n}) on '{t}' would declare 'fun {t}.{n}' a second time, and the hand-written 'fun {t}.{n}' already declares it. Fix: remove derive({n}), or remove the hand-written 'fun {t}.{n}'",
                        t = req.type_name
                    ),
                    span: f.span,
                });
            }
        }
        let target = if let Some(d) = datacs.iter().find(|d| d.name == req.type_name) {
            Target::Datac(d)
        } else if let Some(e) = enums.iter().find(|e| e.name == req.type_name) {
            Target::Enum(e)
        } else {
            continue;
        };
        let const_params = match &target {
            Target::Datac(d) => d.const_params.len(),
            Target::Enum(e) => e.const_params.len(),
        };
        if const_params > 0 {
            return Err(ParseError {
                help: None,
                message: format!(
                    "derive is not supported on '{}': it has const parameters, and a derived method would need them written out",
                    req.type_name
                ),
                span: req.span,
            });
        }
        let src = method_source(&scope, req, &target)?;
        // The generated text is parsed as a file of its own. Every token takes
        // the span of its derive clause, so a diagnostic about generated code
        // points at the line the programmer wrote.
        let internal = |message: String| ParseError {
            help: None,
            message: format!("internal error in derive: {message}"),
            span: req.span,
        };
        let mut tokens = Lexer::new(&src).tokenize().map_err(|e| internal(e.message))?;
        for t in tokens.iter_mut() {
            t.span = req.span;
        }
        let prog = Parser::new(tokens).parse_program().map_err(|e| internal(e.message))?;
        out.extend(prog.functions);
    }
    Ok(out)
}

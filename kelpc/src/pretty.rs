use crate::ast::*;
use std::fmt::Write;

pub fn pretty_print(program: &Program) -> String {
    let mut out = String::new();
    for i in &program.impls {
        write!(out, "impl \"{}\"", i.path).unwrap();
        if let Some(alias) = &i.alias {
            write!(out, " as {alias}").unwrap();
        }
        out.push('\n');
    }
    if !program.impls.is_empty() {
        out.push('\n');
    }
    for d in &program.datacs {
        print_datac(&mut out, d);
        out.push('\n');
    }
    for e in &program.enums {
        print_enum(&mut out, e);
        out.push('\n');
    }
    for f in &program.functions {
        print_function(&mut out, f, 0);
        out.push('\n');
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn print_datac(out: &mut String, d: &DatacDecl) {
    write!(out, "datac {}(", d.name).unwrap();
    for (i, f) in d.fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let kw = if f.mutable { "var" } else { "val" };
        write!(out, "{kw} {}: {}", f.name, f.ty.display()).unwrap();
    }
    out.push_str(")\n");
}

fn print_enum(out: &mut String, e: &EnumDecl) {
    write!(out, "enum {} {{\n", e.name).unwrap();
    for v in &e.variants {
        write!(out, "    {}", v.name).unwrap();
        if !v.fields.is_empty() {
            out.push('(');
            for (i, f) in v.fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let kw = if f.mutable { "var" } else { "val" };
                write!(out, "{kw} {}: {}", f.name, f.ty.display()).unwrap();
            }
            out.push(')');
        }
        out.push('\n');
    }
    out.push_str("}\n");
}

fn print_function(out: &mut String, f: &FunctionDecl, depth: usize) {
    indent(out, depth);
    if f.extern_ {
        out.push_str("extern ");
    }
    out.push_str("fun ");
    if let Some(r) = &f.receiver {
        if r.mutable {
            out.push_str("mut ");
        }
        write!(out, "{}.", r.type_name).unwrap();
    }
    write!(out, "{}(", f.name).unwrap();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{}: {}{}", p.name, p.mode.keyword(), p.ty.display()).unwrap();
    }
    out.push(')');
    if let Some(rt) = &f.return_type {
        write!(out, ".R: {}", rt.display()).unwrap();
    }
    match &f.body {
        FunctionBody::Expr(e) => {
            out.push_str(" = ");
            print_expr(out, e);
            out.push('\n');
        }
        FunctionBody::Block(block) => {
            out.push_str(" {\n");
            print_block_stmts(out, block, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        FunctionBody::Extern => out.push('\n'),
    }
}

fn print_block_stmts(out: &mut String, block: &Block, depth: usize) {
    for stmt in &block.stmts {
        print_stmt(out, stmt, depth);
    }
}

fn print_stmt(out: &mut String, stmt: &Stmt, depth: usize) {
    indent(out, depth);
    match stmt {
        Stmt::Let {
            name,
            mutable,
            ty,
            init,
            ..
        } => {
            out.push_str(if *mutable { "var " } else { "val " });
            out.push_str(name);
            if let Some(ty) = ty {
                write!(out, ": {}", ty.display()).unwrap();
            }
            out.push_str(" = ");
            print_expr(out, init);
            out.push('\n');
        }
        Stmt::While { cond, body, .. } => {
            out.push_str("while ");
            print_expr(out, cond);
            out.push_str(" {\n");
            print_block_stmts(out, body, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        Stmt::Return { value, .. } => {
            out.push_str("return");
            if let Some(v) = value {
                out.push(' ');
                print_expr(out, v);
            }
            out.push('\n');
        }
        Stmt::Expr { expr, .. } => {
            print_expr(out, expr);
            out.push('\n');
        }
    }
}

fn print_expr(out: &mut String, expr: &Expr) {
    match expr {
        Expr::FnLit {
            params,
            return_type,
            body,
            ..
        } => {
            out.push_str("fun(");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{}: {}{}", p.name, p.mode.keyword(), p.ty.display()).unwrap();
            }
            out.push(')');
            if let Some(rt) = return_type {
                write!(out, ".R: {}", rt.display()).unwrap();
            }
            match body.as_ref() {
                FunctionBody::Expr(e) => {
                    out.push_str(" = ");
                    print_expr(out, e);
                }
                FunctionBody::Block(b) => {
                    out.push(' ');
                    print_block_stmts(out, b, 0);
                }
                FunctionBody::Extern => {}
            }
        }
        Expr::IntLit(v, _) => write!(out, "{v}").unwrap(),
        Expr::CharLit(v, _) => write!(out, "{}", char_literal(*v)).unwrap(),
        Expr::FloatLit(v, _) => write!(out, "{v}").unwrap(),
        Expr::StringLit(s, _) => write!(out, "{s:?}").unwrap(),
        Expr::BoolLit(b, _) => write!(out, "{b}").unwrap(),
        Expr::Ident(name, _) => out.push_str(name),
        Expr::Unary { op, expr, .. } => {
            let sym = match op {
                UnOp::Neg => "-",
                UnOp::Not => "!",
                UnOp::BitNot => "~",
            };
            out.push_str(sym);
            print_expr(out, expr);
        }
        Expr::Try { expr, .. } => {
            print_expr(out, expr);
            out.push('?');
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            out.push('(');
            print_expr(out, lhs);
            let sym = match op {
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
            };
            write!(out, " {sym} ").unwrap();
            print_expr(out, rhs);
            out.push(')');
        }
        Expr::Call { callee, args, .. } => {
            write!(out, "{callee}(").unwrap();
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if let Some(name) = &a.name {
                    write!(out, "{name} = ").unwrap();
                }
                print_expr(out, &a.value);
            }
            out.push(')');
        }
        Expr::FieldAccess { base, field, .. } => {
            print_expr(out, base);
            write!(out, ".{field}").unwrap();
        }
        Expr::Assign { target, value, .. } => {
            print_expr(out, target);
            out.push_str(" = ");
            print_expr(out, value);
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            out.push_str("if ");
            print_expr(out, cond);
            out.push_str(" {\n");
            print_block_stmts(out, then_block, 1);
            out.push('}');
            if let Some(else_branch) = else_block {
                out.push_str(" else ");
                match else_branch.as_ref() {
                    ElseBranch::Block(b) => {
                        out.push_str("{\n");
                        print_block_stmts(out, b, 1);
                        out.push('}');
                    }
                    ElseBranch::If(e) => print_expr(out, e),
                }
            }
        }
        Expr::Block(block) => {
            out.push_str("{\n");
            print_block_stmts(out, block, 1);
            out.push('}');
        }
        Expr::Ref { mutable, expr, .. } => {
            out.push('&');
            if *mutable {
                out.push_str("mut ");
            }
            print_expr(out, expr);
        }
        Expr::Deref { expr, .. } => {
            out.push('*');
            print_expr(out, expr);
        }
        Expr::MethodCall { receiver, method, args, .. } => {
            print_expr(out, receiver);
            write!(out, ".{method}(").unwrap();
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if let Some(name) = &a.name {
                    write!(out, "{name} = ").unwrap();
                }
                print_expr(out, &a.value);
            }
            out.push(')');
        }
        Expr::Template { parts, .. } => {
            out.push('"');
            for p in parts {
                match p {
                    TemplatePart::Text(t) => out.push_str(t),
                    TemplatePart::Expr(e) => {
                        out.push_str("${");
                        print_expr(out, e);
                        out.push('}');
                    }
                }
            }
            out.push('"');
        }
        Expr::When {
            scrutinee, arms, ..
        } => {
            out.push_str("when ");
            print_expr(out, scrutinee);
            out.push_str(" {\n");
            for arm in arms {
                out.push_str("    ");
                match &arm.pattern {
                    None => out.push_str("else"),
                    Some(p) => {
                        out.push_str(&p.variant);
                        if !p.binds.is_empty() {
                            out.push('(');
                            for (i, b) in p.binds.iter().enumerate() {
                                if i > 0 {
                                    out.push_str(", ");
                                }
                                out.push_str(b.as_deref().unwrap_or("_"));
                            }
                            out.push(')');
                        }
                    }
                }
                out.push_str(" -> ");
                print_expr(out, &arm.body);
                out.push('\n');
            }
            out.push('}');
        }
    }
}

/// A code point rendered back as the character literal it was written as:
/// `'a'`, `'\n'`, `'\''`, `'\u{4e2d}'`. The printable ASCII range is the only
/// range written literally, so a literal always round-trips through the
/// lexer — `'é'` prints as `'é'` (the lexer accepts a non-ASCII character
/// verbatim), and anything in the control range becomes an escape.
fn char_literal(cp: u32) -> String {
    let escaped = match cp {
        0x0A => Some("\\n".to_string()),
        0x09 => Some("\\t".to_string()),
        0x0D => Some("\\r".to_string()),
        0x00 => Some("\\0".to_string()),
        0x5C => Some("\\\\".to_string()),
        0x27 => Some("\\'".to_string()),
        0x20..=0x7E => Some(char::from_u32(cp).expect("ASCII").to_string()),
        _ => None,
    };
    match escaped {
        Some(s) => format!("'{s}'"),
        None => format!("'\\u{{{cp:x}}}'"),
    }
}

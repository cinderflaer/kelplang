use crate::ast::*;
use std::fmt::Write;

pub fn pretty_print(program: &Program) -> String {
    let mut out = String::new();
    for s in &program.structs {
        print_struct(&mut out, s, 0);
        out.push('\n');
    }
    for f in &program.functions {
        print_function(&mut out, f, 0);
        out.push('\n');
    }
    out
}

fn print_struct(out: &mut String, s: &StructDecl, depth: usize) {
    indent(out, depth);
    write!(out, "struct {} {{\n", s.name).unwrap();
    for field in &s.fields {
        indent(out, depth + 1);
        write!(out, "{}: {},\n", field.name, field.ty.name).unwrap();
    }
    indent(out, depth);
    out.push_str("}\n");
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn print_function(out: &mut String, f: &FunctionDecl, depth: usize) {
    indent(out, depth);
    write!(out, "fn {}(", f.name).unwrap();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{}: {}", p.name, p.ty.name).unwrap();
    }
    out.push(')');
    if let Some(rt) = &f.return_type {
        write!(out, " -> {}", rt.name).unwrap();
    }
    out.push_str(" {\n");
    print_block_stmts(out, &f.body, depth + 1);
    indent(out, depth);
    out.push_str("}\n");
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
            out.push_str("let ");
            if *mutable {
                out.push_str("mut ");
            }
            out.push_str(name);
            if let Some(ty) = ty {
                write!(out, ": {}", ty.name).unwrap();
            }
            out.push_str(" = ");
            print_expr(out, init);
            out.push_str(";\n");
        }
        Stmt::Return { value, .. } => {
            out.push_str("return");
            if let Some(v) = value {
                out.push(' ');
                print_expr(out, v);
            }
            out.push_str(";\n");
        }
        Stmt::Expr { expr, .. } => {
            print_expr(out, expr);
            out.push_str(";\n");
        }
        Stmt::While { cond, body, .. } => {
            out.push_str("while ");
            print_expr(out, cond);
            out.push_str(" {\n");
            print_block_stmts(out, body, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
    }
}

fn print_expr(out: &mut String, expr: &Expr) {
    match expr {
        Expr::IntLit(v, _) => write!(out, "{v}").unwrap(),
        Expr::FloatLit(v, _) => write!(out, "{v}").unwrap(),
        Expr::StringLit(s, _) => write!(out, "{s:?}").unwrap(),
        Expr::BoolLit(b, _) => write!(out, "{b}").unwrap(),
        Expr::Ident(name, _) => out.push_str(name),
        Expr::Unary { op, expr, .. } => {
            let sym = match op {
                UnOp::Neg => "-",
                UnOp::Not => "!",
            };
            out.push_str(sym);
            print_expr(out, expr);
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
                print_expr(out, a);
            }
            out.push(')');
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
        Expr::FieldAccess { base, field, .. } => {
            print_expr(out, base);
            write!(out, ".{field}").unwrap();
        }
        Expr::StructLit { name, fields, .. } => {
            write!(out, "{name} {{ ").unwrap();
            for (i, (fname, fval)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{fname}: ").unwrap();
                print_expr(out, fval);
            }
            out.push_str(" }");
        }
        Expr::Assign { target, value, .. } => {
            print_expr(out, target);
            out.push_str(" = ");
            print_expr(out, value);
        }
    }
}

//! Lowers the typed IR (`tir`) to C source text.
//!
//! Blocks and `if` used in *value* position (e.g. the RHS of a `let`) are
//! emitted using the GNU statement-expression extension `({ ...; expr; })`,
//! which both `gcc` and `clang` support. This is what lets `let x = if c {
//! 1 } else { 2 };` compile to C at all, since plain C has no
//! expression-valued blocks. When a block/`if` is used as a bare statement
//! its value (if any) is simply discarded, so no extension is needed there.

use crate::ast::{BinOp, UnOp};
use crate::tir::*;
use crate::types::{StructDef, Ty};
use std::fmt::Write;

pub fn generate(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("#include <stdint.h>\n");
    out.push_str("#include <stdbool.h>\n");
    out.push_str("#include <stdio.h>\n\n");

    for s in &program.structs {
        write_struct(&mut out, s);
    }
    if !program.structs.is_empty() {
        out.push('\n');
    }

    for f in &program.functions {
        if f.name == "main" {
            continue; // defined (not just declared) below, with its special `int main(void)` signature
        }
        write_signature(&mut out, f);
        out.push_str(";\n");
    }
    out.push('\n');

    for f in &program.functions {
        write_function(&mut out, f);
        out.push('\n');
    }

    out
}

fn write_struct(out: &mut String, s: &StructDef) {
    write!(out, "typedef struct {{\n").unwrap();
    for f in &s.fields {
        write!(out, "    {} {};\n", f.ty.c_type(), field_name(&f.name)).unwrap();
    }
    write!(out, "}} {};\n", Ty::Struct(s.name.clone()).c_type()).unwrap();
}

fn write_signature(out: &mut String, f: &Function) {
    write!(out, "{} {}(", f.ret.c_type(), c_name(&f.name)).unwrap();
    if f.params.is_empty() {
        out.push_str("void");
    } else {
        for (i, (name, ty)) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{} {}", ty.c_type(), c_name(name)).unwrap();
        }
    }
    out.push(')');
}

fn write_function(out: &mut String, f: &Function) {
    if f.name == "main" {
        // The C runtime requires `main` to return `int`; falling off the end
        // is defined by the C standard as `return 0`, so an `.x` `main`
        // (always `-> unit`) needs no extra trailing return. A bare
        // `return;` from `.x`, though, is a constraint violation in a
        // function declared to return `int`, so it's rewritten to `return
        // 0;` here rather than threading an "am I in main" flag through
        // every statement-writing function below.
        out.push_str("int main(void) {\n");
        let mut body = String::new();
        write_function_body(&mut body, &f.body, 1, &f.ret);
        out.push_str(&body.replace("return;\n", "return 0;\n"));
        out.push_str("}\n");
        return;
    }
    write_signature(out, f);
    out.push_str(" {\n");
    write_function_body(out, &f.body, 1, &f.ret);
    out.push_str("}\n");
}

/// Writes a function body's statements. Unlike a nested block used as a
/// mere statement (`write_block_body`), a present tail expression here
/// becomes the function's `return` — not a discarded expression statement —
/// whenever the function actually returns a value; `infer.rs` treats a
/// tail-expression body exactly like `return <tail>;` (see
/// `check_function`), so codegen has to honor that too. For a `Unit`-
/// returning function (including `main`), the tail is still discarded, same
/// as any other statement-position block.
fn write_function_body(out: &mut String, block: &Block, depth: usize, ret_ty: &Ty) {
    for stmt in &block.stmts {
        write_stmt(out, stmt, depth);
    }
    if let Some(tail) = &block.tail {
        if *ret_ty == Ty::Unit {
            write_stmt_like_expr(out, tail, depth);
        } else {
            indent(out, depth);
            writeln!(out, "return {};", expr_value(tail)).unwrap();
        }
    }
}

/// Writes a block's statements (and its discarded tail, if any) as plain C
/// statements, indented by `depth` levels. Used for a block appearing in
/// statement position (`if`/`while` bodies, nested `{ }` blocks) — as
/// opposed to a function's own top-level body (see `write_function_body`).
fn write_block_body(out: &mut String, block: &Block, depth: usize) {
    for stmt in &block.stmts {
        write_stmt(out, stmt, depth);
    }
    if let Some(tail) = &block.tail {
        write_stmt_like_expr(out, tail, depth);
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("    ");
    }
}

fn write_stmt(out: &mut String, stmt: &Stmt, depth: usize) {
    match stmt {
        Stmt::Let { name, ty, init } => {
            indent(out, depth);
            write!(out, "{} {} = {};\n", ty.c_type(), c_name(name), expr_value(init)).unwrap();
        }
        Stmt::Return { value } => {
            indent(out, depth);
            match value {
                Some(v) => write!(out, "return {};\n", expr_value(v)).unwrap(),
                None => out.push_str("return;\n"),
            }
        }
        Stmt::While { cond, body } => {
            indent(out, depth);
            write!(out, "while ({}) {{\n", expr_value(cond)).unwrap();
            write_block_body(out, body, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        Stmt::Expr(e) => write_stmt_like_expr(out, e, depth),
    }
}

/// Writes an expression used as a full statement: `if`/block get rendered as
/// real C control flow (no statement-expression wrapping needed, since the
/// value is discarded); everything else becomes `<expr>;`.
fn write_stmt_like_expr(out: &mut String, expr: &Expr, depth: usize) {
    match &expr.kind {
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            indent(out, depth);
            write!(out, "if ({}) {{\n", expr_value(cond)).unwrap();
            write_block_body(out, then_block, depth + 1);
            indent(out, depth);
            out.push('}');
            write_else(out, else_block.as_ref(), depth);
            out.push('\n');
        }
        ExprKind::Block(block) => {
            indent(out, depth);
            out.push_str("{\n");
            write_block_body(out, block, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        _ => {
            indent(out, depth);
            write!(out, "{};\n", expr_value(expr)).unwrap();
        }
    }
}

fn write_else(out: &mut String, else_block: Option<&Block>, depth: usize) {
    let Some(else_block) = else_block else {
        return;
    };
    // Render a chained `else if` without an extra nested block when the
    // else-branch is exactly a single tail `if` expression (an `else if`).
    if else_block.stmts.is_empty() {
        if let Some(tail) = &else_block.tail {
            if let ExprKind::If {
                cond,
                then_block,
                else_block: inner_else,
            } = &tail.kind
            {
                write!(out, " else if ({}) {{\n", expr_value(cond)).unwrap();
                write_block_body(out, then_block, depth + 1);
                indent(out, depth);
                out.push('}');
                write_else(out, inner_else.as_ref(), depth);
                return;
            }
        }
    }
    out.push_str(" else {\n");
    write_block_body(out, else_block, depth + 1);
    indent(out, depth);
    out.push('}');
}

/// Renders an expression for use as a C *value* (RHS of `=`, call argument,
/// operand, condition, ...). `if`/block expressions become GNU
/// statement-expressions here.
fn expr_value(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::IntLit(v) => v.to_string(),
        ExprKind::FloatLit(v) => format!("{v}"),
        ExprKind::StringLit(s) => c_string_literal(s),
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::Ident(name) => c_name(name),
        ExprKind::Unary { op, expr: sub } => {
            let sym = match op {
                UnOp::Neg => "-",
                UnOp::Not => "!",
            };
            format!("({sym}{})", expr_value(sub))
        }
        ExprKind::Binary { op, lhs, rhs } => {
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
            format!("({} {sym} {})", expr_value(lhs), expr_value(rhs))
        }
        ExprKind::Call { callee, args } => call_value(callee, args),
        ExprKind::FieldAccess { base, field } => field_access(base, field),
        ExprKind::Ref { expr: sub, .. } => format!("(&{})", expr_value(sub)),
        ExprKind::Deref { expr: sub } => format!("(*{})", expr_value(sub)),
        ExprKind::StructLit { name, fields } => {
            let mut s = format!("(({}){{ ", Ty::Struct(name.clone()).c_type());
            for (i, (fname, fval)) in fields.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                write!(s, ".{} = {}", field_name(fname), expr_value(fval)).unwrap();
            }
            s.push_str(" })");
            s
        }
        ExprKind::Assign { target, value } => {
            format!("({} = {})", lvalue(target), expr_value(value))
        }
        ExprKind::Block(block) => {
            let mut inner = String::new();
            write_block_value(&mut inner, block, 1);
            format!("({{\n{inner}}})")
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            // GNU statement-expressions only yield a value from a trailing
            // *expression statement*, never from control flow — `if`/`else`
            // has no value of its own in C. So an `if` used here (e.g. the
            // RHS of a `let`) is lowered via a temporary that each branch
            // assigns into, with the temporary itself as the trailing value.
            let mut inner = String::new();
            if expr.ty == Ty::Unit {
                write!(inner, "if ({}) {{\n", expr_value(cond)).unwrap();
                write_block_body(&mut inner, then_block, 1);
                inner.push('}');
                write_else(&mut inner, else_block.as_ref(), 0);
                inner.push('\n');
            } else {
                let tmp = fresh_temp();
                writeln!(inner, "{} {tmp};", expr.ty.c_type()).unwrap();
                write!(inner, "if ({}) {{\n", expr_value(cond)).unwrap();
                write_block_assign(&mut inner, then_block, &tmp, 1);
                inner.push('}');
                write_else_assign(&mut inner, else_block.as_ref(), &tmp, 0);
                inner.push('\n');
                writeln!(inner, "{tmp};").unwrap();
            }
            format!("({{\n{inner}}})")
        }
    }
}

/// Renders an assignment target as a C lvalue. The type checker guarantees
/// an assignment's target is always an `Ident`, a `.field` chain rooted in
/// one, or a dereferenced borrow (`*p`, `(*p).field`) — see
/// `infer::is_ast_place`.
fn lvalue(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(name) => c_name(name),
        ExprKind::FieldAccess { base, field } => field_access(base, field),
        ExprKind::Deref { expr: sub } => format!("(*{})", expr_value(sub)),
        other => unreachable!("type checker only allows places as assignment targets, got {other:?}"),
    }
}

/// `base.field` in C: `->` when `base` is reached through a borrow
/// (`Ty::Ref`), `.` otherwise. Each `FieldAccess` node decides this purely
/// from its own immediate base's type — a chain like `c.inner.x` (where `c`
/// is a `mut Counter` parameter and `inner` is a plain embedded field) gets
/// this right node-by-node: the innermost access sees `c`'s `Ref` type and
/// emits `->`, the outer one sees `inner`'s plain `Struct` type and emits
/// `.`, with no extra state threaded through.
fn field_access(base: &Expr, field: &str) -> String {
    if matches!(base.ty, Ty::Ref { .. }) {
        format!("({})->{}", expr_value(base), field_name(field))
    } else {
        format!("({}).{}", expr_value(base), field_name(field))
    }
}

fn fresh_temp() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!("_xtmp{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Writes a block whose tail expression (if any) is assigned into `tmp`
/// instead of being yielded as a statement-expression value — used for the
/// branches of an `if` that's itself being lowered via a temporary.
fn write_block_assign(out: &mut String, block: &Block, tmp: &str, depth: usize) {
    for stmt in &block.stmts {
        write_stmt(out, stmt, depth);
    }
    if let Some(tail) = &block.tail {
        indent(out, depth);
        writeln!(out, "{tmp} = {};", expr_value(tail)).unwrap();
    }
}

fn write_else_assign(out: &mut String, else_block: Option<&Block>, tmp: &str, depth: usize) {
    let Some(else_block) = else_block else {
        return;
    };
    if else_block.stmts.is_empty() {
        if let Some(tail) = &else_block.tail {
            if let ExprKind::If {
                cond,
                then_block,
                else_block: inner_else,
            } = &tail.kind
            {
                write!(out, " else if ({}) {{\n", expr_value(cond)).unwrap();
                write_block_assign(out, then_block, tmp, depth + 1);
                indent(out, depth);
                out.push('}');
                write_else_assign(out, inner_else.as_ref(), tmp, depth);
                return;
            }
        }
    }
    out.push_str(" else {\n");
    write_block_assign(out, else_block, tmp, depth + 1);
    indent(out, depth);
    out.push('}');
}

/// Like `write_block_body`, but a present tail expression is emitted as the
/// statement-expression's yielded value (its value is *not* discarded).
fn write_block_value(out: &mut String, block: &Block, depth: usize) {
    for stmt in &block.stmts {
        write_stmt(out, stmt, depth);
    }
    if let Some(tail) = &block.tail {
        indent(out, depth);
        write!(out, "{};\n", expr_value(tail)).unwrap();
    }
}

fn call_value(callee: &str, args: &[Expr]) -> String {
    if callee == "print" || callee == "print!" {
        let arg = &args[0];
        // `print` appends a newline, `print!` doesn't — opposite of the old
        // `println`/`print` naming.
        let newline = if callee == "print" { "\\n" } else { "" };
        let (fmt, value) = printf_piece(arg);
        return format!("printf(\"{fmt}{newline}\", {value})");
    }

    let mut s = format!("{}(", c_name(callee));
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&expr_value(a));
    }
    s.push(')');
    s
}

/// Returns the printf format specifier and the (possibly cast) value text
/// for printing a value of the given expression's type.
fn printf_piece(expr: &Expr) -> (&'static str, String) {
    let value = expr_value(expr);
    match expr.ty {
        Ty::I32 => ("%d", value),
        Ty::I64 => ("%lld", format!("(long long){value}")),
        Ty::U32 => ("%u", value),
        Ty::U64 => ("%llu", format!("(unsigned long long){value}")),
        Ty::F32 | Ty::F64 => ("%f", format!("(double){value}")),
        Ty::Bool => ("%s", format!("({value} ? \"true\" : \"false\")")),
        Ty::Char => ("%c", value),
        Ty::Str => ("%s", value),
        Ty::Unit | Ty::Struct(_) | Ty::Ref { .. } => {
            unreachable!("type checker rejects printing unit/struct/reference values")
        }
    }
}

fn c_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `.x` identifiers are valid C identifiers as-is; prefix to dodge any
/// accidental collision with a C keyword or our own runtime symbols.
/// `main` is left alone since the C runtime requires that exact name for
/// the process entry point.
fn c_name(name: &str) -> String {
    if name == "main" {
        name.to_string()
    } else {
        format!("x_{name}")
    }
}

/// Struct field names live in C's per-struct member namespace, not the
/// ordinary identifier namespace, but prefixed anyway for consistency and to
/// dodge a C keyword used as a field name.
fn field_name(name: &str) -> String {
    format!("x_{name}")
}

#![allow(dead_code)] // spans and struct fields go unused until type-checking/codegen land (M1+)

mod ast;
mod lexer;
mod parser;
mod pretty;

use lexer::{Lexer, Span};
use parser::Parser;
use std::process::ExitCode;

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in src.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn report_error(path: &str, src: &str, message: &str, span: Span) {
    let (line, col) = line_col(src, span.start);
    eprintln!("error: {message}");
    eprintln!("  --> {path}:{line}:{col}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args[1] != "build" {
        eprintln!("usage: xc build <file.x>");
        return ExitCode::FAILURE;
    }
    let path = &args[2];

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return ExitCode::FAILURE;
        }
    };

    let tokens = match Lexer::new(&src).tokenize() {
        Ok(t) => t,
        Err(e) => {
            report_error(path, &src, &e.message, e.span);
            return ExitCode::FAILURE;
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            report_error(path, &src, &e.message, e.span);
            return ExitCode::FAILURE;
        }
    };

    print!("{}", pretty::pretty_print(&program));

    ExitCode::SUCCESS
}

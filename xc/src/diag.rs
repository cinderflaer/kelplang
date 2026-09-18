//! Cross-file diagnostics.
//!
//! A `Span` is a byte range inside *one* file, so with M5's multi-file
//! projects a span alone is no longer enough to print `file:line:col`. Every
//! error carries the canonical id of the module it came from, and
//! `SourceMap` maps that id back to the path and text needed to turn the
//! offset into a line/column.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lexer::Span;

#[derive(Debug)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    /// Canonical module id — the module's path relative to `src/` without
    /// the `.x` suffix (`"shapes/circle"`). The entry module has one too
    /// (its own path), even though its *symbol prefix* is empty.
    pub module: String,
}

impl Diagnostic {
    pub fn new(message: String, span: Span, module: impl Into<String>) -> Self {
        Diagnostic {
            message,
            span,
            module: module.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct SourceMap {
    files: HashMap<String, SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        SourceMap::default()
    }

    pub fn insert(&mut self, module: &str, path: &Path, text: String) {
        self.files.insert(
            module.to_string(),
            SourceFile {
                path: path.to_path_buf(),
                text,
            },
        );
    }

    /// Prints `error: ...` plus an `--> file:line:col` location to stderr.
    pub fn report(&self, d: &Diagnostic) {
        eprintln!("{}", self.render(d));
    }

    pub fn render(&self, d: &Diagnostic) -> String {
        match self.files.get(&d.module) {
            Some(f) => {
                let (line, col) = line_col(&f.text, d.span.start);
                format!(
                    "error: {}\n  --> {}:{line}:{col}",
                    d.message,
                    f.path.display()
                )
            }
            // Module `""` is the sentinel for a diagnostic raised before (or
            // instead of) loading a module — a bad CLI path, a missing
            // `x.toml` — so there is no file to point at.
            None if d.module.is_empty() => format!("error: {}", d.message),
            // Only reachable if a diagnostic was produced for a module whose
            // file was never registered — a compiler bug, not a user error.
            None => format!("error: {}\n  --> <module '{}'>", d.message, d.module),
        }
    }
}

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

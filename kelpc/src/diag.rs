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

/// A secondary location attached to a diagnostic. Used where an error has a
/// natural second site that lives in *another* file — an alias's definition
/// versus the `%*name*%` that used it (M5.3), which is the case that stops a
/// single `(module, span)` pair from being enough.
#[derive(Debug, Clone)]
pub struct DiagnosticNote {
    pub message: String,
    pub span: Span,
    pub module: String,
}

#[derive(Debug)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    /// Canonical module id — the module's path relative to `src/` without
    /// the `.kp` suffix (`"shapes/circle"`). The entry module has one too
    /// (its own path), even though its *symbol prefix* is empty.
    pub module: String,
    /// Extra locations, rendered after the primary one.
    pub notes: Vec<DiagnosticNote>,
}

impl Diagnostic {
    pub fn new(message: String, span: Span, module: impl Into<String>) -> Self {
        Diagnostic {
            message,
            span,
            module: module.into(),
            notes: Vec::new(),
        }
    }

    /// Attaches a secondary location in its own module/file.
    pub fn with_note(
        mut self,
        message: String,
        span: Span,
        module: impl Into<String>,
    ) -> Self {
        self.notes.push(DiagnosticNote {
            message,
            span,
            module: module.into(),
        });
        self
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

    /// `file:line:col` for a span in a module, when the module's text is
    /// known. Used by `codegen_c.rs` for capability denials, whose message has
    /// to name the `.kp` call site at runtime (M7) — the generated C has no
    /// `#line` directives of its own.
    pub fn locate(&self, module: &str, span: Span) -> Option<String> {
        let f = self.files.get(module)?;
        let (line, col) = line_col(&f.text, span.start);
        Some(format!("{}:{line}:{col}", f.path.display()))
    }

    /// Prints `error: ...` plus an `--> file:line:col` location to stderr.
    pub fn report(&self, d: &Diagnostic) {
        eprintln!("{}", self.render(d));
    }

    pub fn render(&self, d: &Diagnostic) -> String {
        let mut out = self.render_at(&d.message, d.span, &d.module, "error: ");
        for note in &d.notes {
            out.push('\n');
            out.push_str(&self.render_at(&note.message, note.span, &note.module, "  note: "));
        }
        out
    }

    fn render_at(&self, message: &str, span: Span, module: &str, label: &str) -> String {
        match self.files.get(module) {
            Some(f) => {
                let (line, col) = line_col(&f.text, span.start);
                format!(
                    "{label}{message}\n  --> {}:{line}:{col}",
                    f.path.display()
                )
            }
            // Module `""` is the sentinel for a diagnostic raised before (or
            // instead of) loading a module — a bad CLI path, a missing
            // `coral.toml` — so there is no file to point at.
            None if module.is_empty() => format!("{label}{message}"),
            // Only reachable if a diagnostic was produced for a module whose
            // file was never registered — a compiler bug, not a user error.
            None => format!("{label}{message}\n  --> <module '{module}'>"),
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

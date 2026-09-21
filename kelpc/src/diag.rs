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

/// What kind of finding a diagnostic is. The prose printer's three labels
/// (`error: `/`warning: `/`outdated: `) are the three values, and `kelpc
/// check --json` spells them the same way, so a consumer that reads only the
/// machine form and one that reads only the prose agree about a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The program does not compile.
    Error,
    /// A lint's finding. `kelpc lint` exits zero with warnings on stderr, so
    /// a consumer has to classify rather than trust the exit code.
    Warning,
    /// A spelling on the register of outdated syntax: it still compiles, and
    /// `--deny-outdated` is what makes it an error instead.
    Outdated,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Outdated => "outdated",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error: ",
            Severity::Warning => "warning: ",
            Severity::Outdated => "outdated: ",
        }
    }
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
    /// to name the `.kp` call site at runtime (M7). The generated C does carry
    /// `#line` directives now (`line_directive` below), but this is still the
    /// one that answers that call: a denial names the *column* as well, and it
    /// is a compile-time constant rather than a runtime `__FILE__`/`__LINE__`
    /// that would have to be formatted into a string before the panic.
    pub fn locate(&self, module: &str, span: Span) -> Option<String> {
        let f = self.files.get(module)?;
        let (line, col) = line_col(&f.text, span.start);
        Some(format!("{}:{line}:{col}", f.path.display()))
    }

    /// The `#line` directive that points the C compiler at `module`'s source at
    /// `span` (roadmap open item 20), or `None` when the module's text was
    /// never registered.
    ///
    /// `codegen_c.rs` emits one before the statements it renders from `.kp`, so
    /// a diagnostic from `cc` on the generated C names the `.kp` file and line
    /// the statement came from instead of a line of generated C. Source mapping
    /// is worth only what it is exact about, so the line is the span's own;
    /// codegen never guesses one.
    pub fn line_directive(&self, module: &str, span: Span) -> Option<String> {
        let f = self.files.get(module)?;
        let (line, _) = line_col(&f.text, span.start);
        Some(format!(
            "#line {line} \"{}\"",
            c_string_body(&f.path.to_string_lossy())
        ))
    }

    /// The path, line and column of `span` in `module`, when the module's
    /// text is known.
    ///
    /// [`SourceMap::locate`] and [`SourceMap::line_directive`] are this value
    /// formatted two ways; the LLVM backend's DWARF needs the three parts
    /// apart, because a `DIFile` carries the path while a `DILocation` carries
    /// the numbers. All three go through one `line_col` call, so the line a
    /// `#line` directive names and the line a debug location names cannot
    /// drift.
    pub fn path_line_col(&self, module: &str, span: Span) -> Option<(&Path, u32, u32)> {
        let f = self.files.get(module)?;
        let (line, col) = line_col(&f.text, span.start);
        Some((f.path.as_path(), line as u32, col as u32))
    }

    /// Prints `error: ...` plus an `--> file:line:col` location to stderr.
    pub fn report(&self, d: &Diagnostic) {
        self.report_as(d, Severity::Error);
    }

    /// The same, labelled `warning:`: a lint's finding is not a build failure,
    /// and the label is the whole difference between the two (`kelpc lint`).
    pub fn report_warning(&self, d: &Diagnostic) {
        self.report_as(d, Severity::Warning);
    }

    /// A deprecation, labelled `outdated: `: the construct still compiles and
    /// the compiler still builds it, so this is not an error, but it is not a
    /// style opinion either (that is what `warning:` is for) and the label says
    /// which kind of finding a reader is looking at. `--deny-outdated` prints
    /// the same diagnostics through `report_as`, as errors.
    pub fn report_outdated(&self, d: &Diagnostic) {
        self.report_as(d, Severity::Outdated);
    }

    /// Prints one finding under the label its kind carries. The three public
    /// entry points above are this function with a kind, so a caller that has
    /// the kind in hand (`kelpc check`) does not have to know which printer
    /// belongs to which.
    pub fn report_as(&self, d: &Diagnostic, severity: Severity) {
        eprintln!("{}", self.render_labeled(d, severity.label()));
    }

    fn render_labeled(&self, d: &Diagnostic, label: &str) -> String {
        let mut out = self.render_at(&d.message, d.span, &d.module, label);
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

    /// One finding as a JSON object, for `kelpc check --json` (MK13-S1-M3).
    ///
    /// The location is a *range*: `line`/`column` are 1-based and name the
    /// first byte of the span, `end_line`/`end_column` are exclusive and name
    /// the byte after its last, so a consumer can underline exactly what the
    /// message is about instead of guessing a length from prose. The location
    /// keys are absent when the module's text was never registered, which is
    /// the one case the prose printer also has nothing to point at;
    /// `"module"` is always there, because a span is a range inside *one*
    /// file and the module id is what says which.
    pub fn render_json(&self, severity: Severity, d: &Diagnostic) -> String {
        let mut out = format!(
            "{{\"severity\":{},\"message\":{},\"module\":{}",
            json_string(severity.as_str()),
            json_string(&d.message),
            json_string(&d.module),
        );
        out.push_str(&self.json_location(d.span, &d.module));
        out.push_str(",\"notes\":[");
        for (i, note) in d.notes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"message\":{},\"module\":{}",
                json_string(&note.message),
                json_string(&note.module),
            ));
            out.push_str(&self.json_location(note.span, &note.module));
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    /// The whole document `kelpc check --json` writes: `ok`, and every finding
    /// in the order it was produced, each with the kind it is. `ok` is the job
    /// the exit status does for a shell, spelled out for a consumer that reads
    /// one stream and never looks at a status.
    pub fn render_findings_json(&self, findings: &[(Severity, &Diagnostic)], ok: bool) -> String {
        let mut out = format!("{{\"schema\":1,\"ok\":{ok},\"diagnostics\":[");
        for (i, (severity, d)) in findings.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&self.render_json(*severity, d));
        }
        out.push_str("]}\n");
        out
    }

    /// The `,\"file\":...` tail of a finding's location, or nothing when the
    /// module's text is unknown.
    fn json_location(&self, span: Span, module: &str) -> String {
        let Some(f) = self.files.get(module) else {
            return String::new();
        };
        let (line, column) = line_col(&f.text, span.start);
        let (end_line, end_column) = line_col(&f.text, span.end);
        format!(
            ",\"file\":{},\"line\":{line},\"column\":{column},\"end_line\":{end_line},\"end_column\":{end_column}",
            json_string(&f.path.to_string_lossy()),
        )
    }
}

/// A string as a JSON string literal. Everything a consumer would mis-read is
/// escaped: the quote, the backslash, and every control character (the three
/// that have a short form, and `\u00XX` for the rest). Text above the ASCII
/// range is written through as UTF-8, which JSON allows and which keeps a
/// message written in another script readable.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A path as the contents of a C string literal: the two bytes that would end
/// the literal or open an escape are escaped, and nothing else is. A `.kp` path
/// holding a quote or a backslash is unusual but has to be right anyway, since
/// the text of a `#line` directive is the same string literal a `printf` would
/// take. `codegen_c.rs` reaches for it too, for the `at` location, the module
/// name and the domain a capability denial bakes into the generated C.
pub(crate) fn c_string_body(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
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

//! `%*alias*%` text substitution (M5.3).
//!
//! A flat, global, purely-textual substitution pass, exactly as specified in
//! the plan's "Imports & project structure": no parameters, no hygiene, no
//! scoping. Two definition sites feed one table:
//!
//! - an `[alias]` table in the project's `coral.toml`, one
//!   `name = "module/path"` per line. The value is a relative module path,
//!   validated when the manifest is read (`resolve.rs`).
//! - an inline `alias %*name*% = <raw text to end of line>` declaration in
//!   any `.kp` file. The declaration gets its own keyword precisely because
//!   the right-hand side is *macro text*, not an expression: it is never
//!   type-checked, just spliced in.
//!
//! The pass runs on the token stream, before the parser, and does three
//! things: strips alias declarations, expands every `%*name*%` reference
//! (including one inside an `impl "..."` path string, which is the main
//! reason import-path aliasing needs no separate mechanism), and rejects
//! cycles.
//!
//! Spans are the fiddly part. A replacement's tokens are lexed from the
//! alias's text, where offsets are meaningless in the file being compiled —
//! so every replacement token gets remapped onto the `%*name*%` occurrence
//! the user actually wrote, which keeps `file:line:col` useful. The one
//! exception is an error *at* the reference itself: those carry the use site
//! as the primary span and the definition as a note (`Diagnostic::with_note`),
//! so both ends show up.
//!
//! Known tradeoff, stated in the plan and kept: because this is pure text
//! substitution, a bad alias produces nonsense that only surfaces as a type
//! error at the (already-expanded) use site. The two-span diagnostics are
//! what keeps that debuggable.

use std::collections::HashMap;

use crate::diag::Diagnostic;
use crate::lexer::{Lexer, Span, Token, TokenKind};
use crate::resolve::{is_ident, is_keyword};

/// Module id the `[alias]` entries of `coral.toml` are attributed to. Not a
/// legal module id (it contains a dot), so it cannot collide with a real
/// module, and `SourceMap` still renders its path.
pub const MANIFEST_MODULE: &str = "coral.toml";

/// The note text that points at where an alias was defined. An entry from
/// the manifest names its table; an inline declaration is named by its own
/// `%*name*%`.
fn defined_here(def: &AliasDef) -> String {
    if def.module == MANIFEST_MODULE {
        format!("alias '{}' in coral.toml [alias] is defined here", def.name)
    } else {
        format!("'%*{}*%' is defined here", def.name)
    }
}

/// One alias definition.
#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: String,
    /// The replacement text, verbatim — the source slice for an inline
    /// declaration, the quoted value for a `[alias]` entry.
    pub text: String,
    /// Absolute offset of `text` in its own file, so a reference *inside* a
    /// definition can be mapped back to a real source span (used by the
    /// cycle diagnostics).
    pub text_offset: usize,
    /// Module the definition came from (`coral.toml` for a `[alias]` entry).
    pub module: String,
    pub span: Span,
}

/// The one flat, global alias table. Definition order is preserved so cycle
/// reporting is deterministic.
#[derive(Debug, Default)]
pub struct AliasTable {
    defs: HashMap<String, AliasDef>,
    order: Vec<String>,
}

impl AliasTable {
    pub fn new() -> Self {
        AliasTable::default()
    }

    #[allow(dead_code)] // introspection used by tests/diagnostics only.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// Registers a definition. The table is one global namespace, so a second
    /// definition of the same name is an error naming both sites.
    pub fn define(&mut self, def: AliasDef) -> Result<(), Diagnostic> {
        if !is_ident(&def.name) {
            return Err(Diagnostic::new(
                format!("'{}' is not a valid alias name", def.name),
                def.span,
                def.module,
            ));
        }
        if is_keyword(&def.name) {
            return Err(Diagnostic::new(
                format!("'{}' is a language keyword and cannot be an alias name", def.name),
                def.span,
                def.module,
            ));
        }
        if let Some(previous) = self.defs.get(&def.name) {
            let previous = previous.clone();
            let where_ = if previous.module == MANIFEST_MODULE {
                "coral.toml [alias]".to_string()
            } else {
                format!("module '{}'", previous.module)
            };
            let message = format!("alias '%*{}*%' is already defined in {where_}", def.name);
            return Err(Diagnostic::new(message, def.span, def.module.clone()).with_note(
                defined_here(&previous),
                previous.span,
                previous.module,
            ));
        }
        self.order.push(def.name.clone());
        self.defs.insert(def.name.clone(), def);
        Ok(())
    }

    /// Rejects a cycle anywhere in the table, used or not — the same rule
    /// imports follow (an import cycle is an error even if nothing calls into
    /// it). Expansion checks for cycles again as it goes; this is what
    /// catches an unused one.
    pub fn check_cycles(&self) -> Result<(), Diagnostic> {
        let mut state: HashMap<String, u8> = HashMap::new();
        let mut stack: Vec<String> = Vec::new();
        for name in &self.order {
            self.visit_alias(name, None, &mut state, &mut stack)?;
        }
        Ok(())
    }

    /// `from` is the reference that led here: `(module, span)` of the
    /// `%*name*%` occurrence inside the previous alias's *text*, already
    /// mapped to its own file's offset — so the two spans of a cycle can land
    /// in two different files.
    fn visit_alias(
        &self,
        name: &str,
        from: Option<(&str, Span)>,
        state: &mut HashMap<String, u8>,
        stack: &mut Vec<String>,
    ) -> Result<(), Diagnostic> {
        match state.get(name) {
            Some(2) => return Ok(()),
            Some(_) => {
                let start = stack.iter().position(|n| n == name).unwrap_or(0);
                let mut chain = stack[start..].to_vec();
                chain.push(name.to_string());
                let def = &self.defs[name];
                let (span, module) = match from {
                    Some((module, span)) => (span, module.to_string()),
                    None => (def.span, def.module.clone()),
                };
                return Err(Diagnostic::new(
                    format!("alias cycle: {}", chain.join(" -> ")),
                    span,
                    module,
                )
                .with_note(defined_here(&def), def.span, def.module.clone()));
            }
            None => {}
        }
        let def = self.defs[name].clone();
        state.insert(name.to_string(), 1);
        stack.push(name.to_string());
        for (referenced, offset) in text_alias_refs(&def.text) {
            if self.defs.contains_key(&referenced) {
                let abs = def.text_offset + offset;
                let span = Span::new(abs, abs + referenced.len() + 4);
                self.visit_alias(&referenced, Some((&def.module, span)), state, stack)?;
            }
        }
        stack.pop();
        state.insert(name.to_string(), 2);
        Ok(())
    }

    /// Expands every `%*name*%` reference in one file's token stream and drops
    /// its inline `alias` declarations, leaving a stream the parser accepts.
    pub fn expand_file(&self, tokens: &[Token], module: &str) -> Result<Vec<Token>, Diagnostic> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut stack: Vec<String> = Vec::new();
        self.expand_into(tokens, module, &mut stack, &mut out)?;
        Ok(out)
    }

    fn expand_into(
        &self,
        tokens: &[Token],
        module: &str,
        stack: &mut Vec<String>,
        out: &mut Vec<Token>,
    ) -> Result<(), Diagnostic> {
        let mut i = 0;
        while i < tokens.len() {
            if let Some((name, end)) = alias_ref_at(tokens, i) {
                let use_span = tokens[i].span.to(tokens[end].span);
                self.expand_name(&name, use_span, module, stack, out)?;
                i = end + 1;
                continue;
            }
            if let Some(shape) = alias_decl_shape(tokens, i) {
                // The declaration itself never reaches the parser; the
                // terminator (a `Newline`) stays, so a declaration in the
                // middle of a line cannot swallow the statement break.
                i = shape.end;
                continue;
            }
            let token = &tokens[i];
            match &token.kind {
                TokenKind::StringLit(text) => {
                    let expanded = self.expand_text(text, token.span, module, stack)?;
                    out.push(Token {
                        kind: TokenKind::StringLit(expanded),
                        span: token.span,
                    });
                }
                TokenKind::StrChunk(text) => {
                    let expanded = self.expand_text(text, token.span, module, stack)?;
                    out.push(Token {
                        kind: TokenKind::StrChunk(expanded),
                        span: token.span,
                    });
                }
                _ => out.push(token.clone()),
            }
            i += 1;
        }
        Ok(())
    }

    /// Appends the expansion of `%*name*%` (and, transitively, of any
    /// references inside the definition) to `out`.
    fn expand_name(
        &self,
        name: &str,
        use_span: Span,
        module: &str,
        stack: &mut Vec<String>,
        out: &mut Vec<Token>,
    ) -> Result<(), Diagnostic> {
        let def = self.lookup(name, use_span, module)?;
        if stack.iter().any(|n| n == name) {
            return Err(self.cycle_error(name, stack, use_span, module, &def));
        }
        // The replacement is expanded with the spans it lexed to, so nested
        // `%*name*%` references are still visible as adjacent tokens; only
        // then is everything remapped onto the `%*name*%` the user wrote.
        let lexed: Vec<Token> = Lexer::new(&def.text)
            .tokenize()
            .map_err(|e| {
                Diagnostic::new(
                    format!(
                        "alias '%*{name}*%' expands to text that is not valid Kelp: {}",
                        e.message
                    ),
                    use_span,
                    module,
                )
                .with_note(defined_here(&def), def.span, def.module.clone())
            })?
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Eof))
            .collect();
        stack.push(name.to_string());
        let mut produced = Vec::new();
        let result = self.expand_into(&lexed, module, stack, &mut produced);
        stack.pop();
        let produced = match result {
            Ok(()) => produced,
            // A span from the alias's own text means nothing in this file, so
            // an error inside expanded text reports the reference instead.
            Err(mut d) => {
                d.span = use_span;
                return Err(d);
            }
        };
        for mut token in produced {
            token.span = use_span;
            out.push(token);
        }
        Ok(())
    }

    /// Textual expansion inside a string literal (or template chunk): an
    /// `impl "%*shared*%"` path is the case this exists for. The replacement
    /// is spliced in as raw text — no re-lexing, so a `$` in the replacement
    /// stays a literal `$`.
    fn expand_text(
        &self,
        text: &str,
        span: Span,
        module: &str,
        stack: &mut Vec<String>,
    ) -> Result<String, Diagnostic> {
        let mut out = String::new();
        let mut i = 0;
        while i < text.len() {
            let Some(relative_start) = text[i..].find("%*") else {
                out.push_str(&text[i..]);
                break;
            };
            let after = i + relative_start + 2;
            let Some(relative_end) = text[after..].find("*%") else {
                // Unterminated sigil: literal text, so `"$100%*"` needs no
                // escaping.
                out.push_str(&text[i..]);
                break;
            };
            out.push_str(&text[i..after - 2]);
            let name = &text[after..after + relative_end];
            if !is_ident(name) {
                out.push_str("%*");
                i = after;
                continue;
            }
            let def = self.lookup(name, span, module)?;
            if stack.iter().any(|n| n == name) {
                return Err(self.cycle_error(name, stack, span, module, &def));
            }
            stack.push(name.to_string());
            let result = self.expand_text(&def.text, span, module, stack);
            stack.pop();
            out.push_str(&result?);
            i = after + relative_end + 2;
        }
        Ok(out)
    }

    fn lookup(&self, name: &str, span: Span, module: &str) -> Result<AliasDef, Diagnostic> {
        self.defs.get(name).cloned().ok_or_else(|| {
            Diagnostic::new(format!("unknown alias '%*{name}*%'"), span, module)
        })
    }

    fn cycle_error(
        &self,
        name: &str,
        stack: &[String],
        use_span: Span,
        use_module: &str,
        def: &AliasDef,
    ) -> Diagnostic {
        let start = stack.iter().position(|n| n == name).unwrap_or(0);
        let mut chain = stack[start..].to_vec();
        chain.push(name.to_string());
        Diagnostic::new(
            format!("alias cycle: {}", chain.join(" -> ")),
            use_span,
            use_module,
        )
        .with_note(defined_here(def), def.span, def.module.clone())
    }
}

/// Matches a contiguous `%*name*%` starting at `i`; returns the name and the
/// index of the closing `%`.
fn alias_ref_at(tokens: &[Token], i: usize) -> Option<(String, usize)> {
    let token = |k: usize| tokens.get(i + k);
    let (first, second, name_tok, fourth, fifth) = (
        token(0)?,
        token(1)?,
        token(2)?,
        token(3)?,
        token(4)?,
    );
    if !matches!(first.kind, TokenKind::Percent) || !matches!(second.kind, TokenKind::Star) {
        return None;
    }
    let TokenKind::Ident(name) = &name_tok.kind else {
        return None;
    };
    if !matches!(fourth.kind, TokenKind::Star) || !matches!(fifth.kind, TokenKind::Percent) {
        return None;
    }
    // Require the five tokens to be adjacent in the source, so `a % * b * %`
    // is arithmetic rather than an alias reference.
    if first.span.end != second.span.start
        || second.span.end != name_tok.span.start
        || name_tok.span.end != fourth.span.start
        || fourth.span.end != fifth.span.start
    {
        return None;
    }
    if !is_ident(name) {
        return None;
    }
    Some((name.clone(), i + 4))
}

/// The token span of one `alias %*name*% = <text>` declaration.
struct DeclShape {
    /// Index of the `alias` keyword itself.
    start: usize,
    /// Index of the closing `%` of the name.
    name_end: usize,
    /// Index of the `=` token.
    eq: usize,
    /// First and last token of the replacement text, when there is one.
    rhs: Option<(usize, usize)>,
    /// Index of the terminator (`Newline`, `}`, or `Eof`).
    end: usize,
}

impl DeclShape {
    /// The declaration's whole-source span, used for diagnostics.
    fn span(&self, tokens: &[Token]) -> Span {
        let last = self.rhs.map(|(_, e)| e).unwrap_or(self.eq);
        tokens[self.start].span.to(tokens[last].span)
    }
}

/// Recognizes `alias %*name*% = <raw text to end of line>` at `i`. The
/// declaration may only start where a statement could (start of file, after a
/// newline, or after `{`/`}`), which is what keeps a user identifier named
/// `alias` usable everywhere else.
fn alias_decl_shape(tokens: &[Token], i: usize) -> Option<DeclShape> {
    if !matches!(&tokens[i].kind, TokenKind::Ident(n) if n == "alias") {
        return None;
    }
    if i > 0
        && !matches!(
            tokens[i - 1].kind,
            TokenKind::Newline | TokenKind::LBrace | TokenKind::RBrace
        )
    {
        return None;
    }
    let opening = tokens.get(i + 1)?;
    let star = tokens.get(i + 2)?;
    if !matches!(opening.kind, TokenKind::Percent) || !matches!(star.kind, TokenKind::Star) {
        return None;
    }
    let name_start = tokens.get(i + 3)?;
    if star.span.end != name_start.span.start {
        return None;
    }
    // Scan forward for the closing `*%` on one line.
    let mut name_end = None;
    let mut j = i + 3;
    while let Some(token) = tokens.get(j) {
        if matches!(token.kind, TokenKind::Newline | TokenKind::Eof) {
            return None;
        }
        if matches!(token.kind, TokenKind::Star) {
            if let Some(percent) = tokens.get(j + 1) {
                if matches!(percent.kind, TokenKind::Percent)
                    && token.span.end == percent.span.start
                {
                    name_end = Some(j + 1);
                    break;
                }
            }
        }
        j += 1;
    }
    let name_end = name_end?;
    if !matches!(tokens.get(name_end + 1)?.kind, TokenKind::Eq) {
        return None;
    }
    let eq = name_end + 1;
    let mut end = eq + 1;
    while end < tokens.len()
        && !matches!(
            tokens[end].kind,
            TokenKind::Newline | TokenKind::Eof | TokenKind::RBrace
        )
    {
        end += 1;
    }
    let rhs = if end > eq + 1 {
        Some((eq + 1, end - 1))
    } else {
        None
    };
    Some(DeclShape {
        start: i,
        name_end,
        eq,
        rhs,
        end,
    })
}

/// Collects the inline `alias` declarations in one file's raw token stream.
/// Runs on the *unexpanded* tokens: the declarations are collected before
/// (and removed during) expansion, so an alias added by a file another file
/// imports is still available.
pub fn scan_inline_aliases(
    tokens: &[Token],
    src: &str,
    module: &str,
) -> Result<Vec<AliasDef>, Diagnostic> {
    let mut defs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let Some(shape) = alias_decl_shape(tokens, i) else {
            i += 1;
            continue;
        };
        let name = src[tokens[i + 3].span.start..tokens[shape.name_end - 1].span.start].to_string();
        let span = shape.span(tokens);
        let Some((rhs_start, rhs_end)) = shape.rhs else {
            return Err(Diagnostic::new(
                format!("alias '%*{name}*%' has no replacement text"),
                span,
                module,
            ));
        };
        let text_offset = tokens[rhs_start].span.start;
        defs.push(AliasDef {
            name,
            text: src[tokens[rhs_start].span.start..tokens[rhs_end].span.end].to_string(),
            text_offset,
            module: module.to_string(),
            span,
        });
        i = shape.end;
    }
    Ok(defs)
}

/// Every `%*name*%` reference inside a piece of alias text, as
/// `(name, byte offset within the text)`.
fn text_alias_refs(text: &str) -> Vec<(String, usize)> {
    let mut refs = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let Some(relative_start) = text[i..].find("%*") else {
            break;
        };
        let start = i + relative_start;
        let after = start + 2;
        let Some(relative_end) = text[after..].find("*%") else {
            break;
        };
        let name = &text[after..after + relative_end];
        if is_ident(name) {
            refs.push((name.to_string(), start));
        }
        i = after + relative_end + 2;
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(&str, &str)]) -> AliasTable {
        let mut table = AliasTable::new();
        for (name, text) in entries {
            table
                .define(AliasDef {
                    name: name.to_string(),
                    text: text.to_string(),
                    text_offset: 0,
                    module: "main".to_string(),
                    span: Span::new(0, 1),
                })
                .unwrap_or_else(|d| panic!("define failed: {}", d.message));
        }
        table
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        Lexer::new(src)
            .tokenize()
            .expect("lex failed")
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, TokenKind::Eof))
            .collect()
    }

    fn expand(table: &AliasTable, src: &str) -> Vec<TokenKind> {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        table
            .expand_file(&tokens, "main")
            .expect("expansion failed")
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, TokenKind::Eof))
            .collect()
    }

    #[test]
    fn expands_a_reference_to_its_text() {
        assert_eq!(
            expand(&table(&[("answer", "40 + 2")]), "print(%*answer*%)"),
            kinds("print(40 + 2)")
        );
    }

    #[test]
    fn expands_a_reference_inside_a_string() {
        assert_eq!(
            expand(&table(&[("shared", "../shared/utils")]), "impl \"%*shared*%\""),
            kinds("impl \"../shared/utils\"")
        );
    }

    #[test]
    fn keeps_the_text_around_a_reference() {
        assert_eq!(
            expand(
                &table(&[("who", "world"), ("greet", "hello")]),
                "print(\"say %*greet*%, %*who*%\")"
            ),
            kinds("print(\"say hello, world\")")
        );
    }

    #[test]
    fn expands_a_chain_of_aliases() {
        assert_eq!(
            expand(&table(&[("a", "%*b*% + 1"), ("b", "41")]), "print(%*a*%)"),
            kinds("print(41 + 1)")
        );
    }

    #[test]
    fn strips_an_inline_declaration() {
        let src = "alias %*answer*% = 40 + 2\nprint(%*answer*%)";
        assert_eq!(
            expand(&table(&[("answer", "40 + 2")]), src),
            kinds("\nprint(40 + 2)")
        );
    }

    #[test]
    fn a_spaced_sigil_is_not_a_reference() {
        assert_eq!(
            expand(&table(&[("answer", "1")]), "a % * answer * % b"),
            kinds("a % * answer * % b")
        );
    }

    #[test]
    fn an_unknown_alias_is_rejected() {
        let tokens = Lexer::new("print(%*nope*%)").tokenize().unwrap();
        let d = table(&[]).expand_file(&tokens, "main").unwrap_err();
        assert!(d.message.contains("unknown alias '%*nope*%'"), "{}", d.message);
    }

    #[test]
    fn a_cycle_is_rejected_by_the_eager_check() {
        let d = table(&[("a", "%*b*%"), ("b", "%*a*%")])
            .check_cycles()
            .unwrap_err();
        assert!(d.message.contains("alias cycle: a -> b -> a"), "{}", d.message);
        assert_eq!(d.notes.len(), 1);
    }

    #[test]
    fn a_duplicate_definition_is_rejected() {
        let mut table = table(&[("a", "1")]);
        let d = table
            .define(AliasDef {
                name: "a".to_string(),
                text: "2".to_string(),
                text_offset: 0,
                module: "util".to_string(),
                span: Span::new(0, 1),
            })
            .unwrap_err();
        assert!(
            d.message.contains("already defined in module 'main'"),
            "{}",
            d.message
        );
    }

    #[test]
    fn scans_inline_declarations_with_raw_text() {
        let src = "alias %*c*% = http.new(timeout = 30)\n";
        let tokens = Lexer::new(src).tokenize().unwrap();
        let defs = scan_inline_aliases(&tokens, src, "main").unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "c");
        assert_eq!(defs[0].text, "http.new(timeout = 30)");
    }
}

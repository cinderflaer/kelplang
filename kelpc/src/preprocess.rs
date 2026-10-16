//! `_alias` text substitution (M5.3).
//!
//! A flat, global, purely-textual substitution pass, exactly as specified in
//! the plan's "Imports & project structure": no parameters, no hygiene, no
//! scoping. Two definition sites feed one table:
//!
//! - an `[alias]` table in the project's `coral.toml`, one
//!   `_name = "module/path"` per line. The value is a relative module path,
//!   validated when the manifest is read (`resolve.rs`).
//! - an inline `alias _name = <raw text to end of line>` declaration in
//!   any `.kp` file. The declaration gets its own keyword precisely because
//!   the right-hand side is *macro text*, not an expression: it is never
//!   type-checked, just spliced in.
//!
//! The pass runs on the token stream, before the parser, and does three
//! things: strips alias declarations, expands every `_name` reference
//! (including one inside an `impl "..."` path string, which is the main
//! reason import-path aliasing needs no separate mechanism), and rejects
//! cycles.
//!
//! Spans are the fiddly part. A replacement's tokens are lexed from the
//! alias's text, where offsets are meaningless in the file being compiled -
//! so every replacement token gets remapped onto the `_name` occurrence
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
/// `_name`.
fn defined_here(def: &AliasDef) -> String {
    if def.module == MANIFEST_MODULE {
        format!("alias '{}' in coral.toml [alias] is defined here", def.name)
    } else {
        format!("'{}' is defined here", def.name)
    }
}

/// One alias definition.
#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: String,
    /// The replacement text, verbatim - the source slice for an inline
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

    /// Whether a definition of `name` is in the table.
    pub fn declares(&self, name: &str) -> bool {
        self.defs.contains_key(name)
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
        if !is_alias_name(&def.name) {
            return Err(Diagnostic::new(
                format!("'{}' is not a valid alias name: it must start with '_' and name something after it", def.name),
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
            let message = format!("alias '{}' is already defined in {where_}", def.name);
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

    /// Rejects a cycle anywhere in the table, used or not - the same rule
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
    /// `_name` occurrence inside the previous alias's *text*, already
    /// mapped to its own file's offset - so the two spans of a cycle can land
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
                let span = Span::new(abs, abs + referenced.len());
                self.visit_alias(&referenced, Some((&def.module, span)), state, stack)?;
            }
        }
        stack.pop();
        state.insert(name.to_string(), 2);
        Ok(())
    }

    /// Expands every `_name` reference in one file's token stream and drops
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
            if let Some(name) = self.reference_at(tokens, i) {
                let use_span = tokens[i].span;
                self.expand_name(&name, use_span, module, stack, out)?;
                i += 1;
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

    /// Appends the expansion of `_name` (and, transitively, of any
    /// references inside the definition) to `out`.
    fn expand_name(
        &self,
        name: &str,
        use_span: Span,
        module: &str,
        stack: &mut Vec<String>,
        out: &mut Vec<Token>,
    ) -> Result<(), Diagnostic> {
        let def = self.defs[name].clone();
        if stack.iter().any(|n| n == name) {
            return Err(self.cycle_error(name, stack, use_span, module, &def));
        }
        // The replacement is expanded with the spans it lexed to, so nested
        // `_name` references are still visible as adjacent tokens; only
        // then is everything remapped onto the `_name` the user wrote.
        let lexed: Vec<Token> = Lexer::new(&def.text)
            .tokenize()
            .map_err(|e| {
                Diagnostic::new(
                    format!(
                        "alias '{name}' expands to text that is not valid Kelp: {}",
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

    /// Whether `tokens[i]` is a plain identifier naming a declared alias. The
    /// reference is an ordinary identifier, so a `_name` that no alias declares
    /// is left alone and stays an ordinary name.
    fn reference_at(&self, tokens: &[Token], i: usize) -> Option<String> {
        let TokenKind::Ident(name) = &tokens.get(i)?.kind else {
            return None;
        };
        self.defs.contains_key(name).then(|| name.clone())
    }

    /// Textual expansion inside a string literal (or template chunk): an
    /// `impl "_shared"` path is the case this exists for. The replacement is
    /// spliced in as raw text - no re-lexing, so a `$` in the replacement stays
    /// a literal `$`. A reference must start a word: `my_shared` is not one.
    fn expand_text(
        &self,
        text: &str,
        span: Span,
        module: &str,
        stack: &mut Vec<String>,
    ) -> Result<String, Diagnostic> {
        let bytes = text.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < text.len() {
            let Some(relative) = text[i..].find('_') else {
                out.push_str(&text[i..]);
                break;
            };
            let start = i + relative;
            out.push_str(&text[i..start]);
            let mut end = start + 1;
            while end < bytes.len() && is_word_byte(bytes[end]) {
                end += 1;
            }
            let name = &text[start..end];
            let starts_word = start == 0 || !is_word_byte(bytes[start - 1]);
            if !starts_word || !self.defs.contains_key(name) {
                out.push_str(name);
                i = end;
                continue;
            }
            let def = self.defs[name].clone();
            if stack.iter().any(|n| n == name) {
                return Err(self.cycle_error(name, stack, span, module, &def));
            }
            stack.push(name.to_string());
            let result = self.expand_text(&def.text, span, module, stack);
            stack.pop();
            out.push_str(&result?);
            i = end;
        }
        Ok(out)
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

/// An alias name: an underscore and at least one more identifier character.
pub fn is_alias_name(name: &str) -> bool {
    name.len() > 1 && name.starts_with('_') && is_ident(name)
}

fn is_word_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// The token span of one `alias _name = <text>` declaration.
struct DeclShape {
    /// Index of the `alias` keyword itself.
    start: usize,
    /// Index of the name token.
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

/// Recognizes `alias _name = <raw text to end of line>` at `i`. The
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
    let TokenKind::Ident(name) = &tokens.get(i + 1)?.kind else {
        return None;
    };
    if !is_alias_name(name) {
        return None;
    }
    let eq = i + 2;
    if !matches!(tokens.get(eq)?.kind, TokenKind::Eq) {
        return None;
    }
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
        name_end: i + 1,
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
        let TokenKind::Ident(name) = &tokens[shape.name_end].kind else {
            unreachable!("alias_decl_shape checks for the name token")
        };
        let name = name.clone();
        let span = shape.span(tokens);
        let Some((rhs_start, rhs_end)) = shape.rhs else {
            return Err(Diagnostic::new(
                format!("alias '{name}' has no replacement text"),
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

/// Every `_name` reference inside a piece of alias text, as
/// `(name, byte offset within the text)`. Only the start of a word counts.
pub(crate) fn text_alias_refs(text: &str) -> Vec<(String, usize)> {
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'_' && (i == 0 || !is_word_byte(bytes[i - 1])) {
            let mut end = i + 1;
            while end < bytes.len() && is_word_byte(bytes[end]) {
                end += 1;
            }
            if end > i + 1 {
                refs.push((text[i..end].to_string(), i));
            }
            i = end;
        } else {
            i += 1;
        }
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
            expand(&table(&[("_answer", "40 + 2")]), "print(_answer)"),
            kinds("print(40 + 2)")
        );
    }

    #[test]
    fn expands_a_reference_inside_a_string() {
        assert_eq!(
            expand(&table(&[("_shared", "../shared/utils")]), "impl \"_shared\""),
            kinds("impl \"../shared/utils\"")
        );
    }

    #[test]
    fn keeps_the_text_around_a_reference() {
        assert_eq!(
            expand(
                &table(&[("_who", "world"), ("_greet", "hello")]),
                "print(\"say _greet, _who\")"
            ),
            kinds("print(\"say hello, world\")")
        );
    }

    #[test]
    fn expands_a_chain_of_aliases() {
        assert_eq!(
            expand(&table(&[("_a", "_b + 1"), ("_b", "41")]), "print(_a)"),
            kinds("print(41 + 1)")
        );
    }

    #[test]
    fn strips_an_inline_declaration() {
        let src = "alias _answer = 40 + 2\nprint(_answer)";
        assert_eq!(
            expand(&table(&[("_answer", "40 + 2")]), src),
            kinds("\nprint(40 + 2)")
        );
    }

    #[test]
    fn a_longer_name_wins_over_its_prefix() {
        let t = table(&[("_ans", "1"), ("_answer", "40 + 2")]);
        assert_eq!(
            expand(&t, "print(_answer + _ans)"),
            kinds("print(40 + 2 + 1)")
        );
        assert_eq!(
            expand(&t, "print(\"_answer _ans\")"),
            kinds("print(\"40 + 2 1\")")
        );
    }

    #[test]
    fn an_undeclared_underscore_name_stays_an_identifier() {
        assert_eq!(
            expand(&table(&[("_answer", "1")]), "print(_nope + _x_answer)"),
            kinds("print(_nope + _x_answer)")
        );
    }

    #[test]
    fn a_name_inside_a_longer_word_is_not_a_reference_in_a_string() {
        assert_eq!(
            expand(&table(&[("_answer", "9")]), "\"my_answer _answer\""),
            kinds("\"my_answer 9\"")
        );
    }

    #[test]
    fn a_modulo_expression_is_untouched() {
        assert_eq!(
            expand(&table(&[("_answer", "9")]), "x = a % b + a%b"),
            kinds("x = a % b + a%b")
        );
    }

    #[test]
    fn an_unknown_alias_is_left_for_the_resolver() {
        let tokens = Lexer::new("print(_nope)").tokenize().unwrap();
        let out = table(&[]).expand_file(&tokens, "main").unwrap();
        assert_eq!(out.len(), tokens.len());
    }

    #[test]
    fn a_cycle_is_rejected_by_the_eager_check() {
        let d = table(&[("_a", "_b"), ("_b", "_a")])
            .check_cycles()
            .unwrap_err();
        assert!(d.message.contains("alias cycle: _a -> _b -> _a"), "{}", d.message);
        assert_eq!(d.notes.len(), 1);
    }

    #[test]
    fn a_duplicate_definition_is_rejected() {
        let mut table = table(&[("_a", "1")]);
        let d = table
            .define(AliasDef {
                name: "_a".to_string(),
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
    fn a_name_without_an_underscore_is_not_an_alias() {
        let mut table = AliasTable::new();
        let d = table
            .define(AliasDef {
                name: "answer".to_string(),
                text: "1".to_string(),
                text_offset: 0,
                module: "main".to_string(),
                span: Span::new(0, 1),
            })
            .unwrap_err();
        assert!(d.message.contains("must start with '_'"), "{}", d.message);
    }

    #[test]
    fn scans_inline_declarations_with_raw_text() {
        let src = "alias _c = http.new(timeout = 30)\n";
        let tokens = Lexer::new(src).tokenize().unwrap();
        let defs = scan_inline_aliases(&tokens, src, "main").unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "_c");
        assert_eq!(defs[0].text, "http.new(timeout = 30)");
    }
}

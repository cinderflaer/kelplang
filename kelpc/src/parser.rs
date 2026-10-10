use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// How many `if let`s have been desugared, so each one's synthesized
    /// scrutinee binding has a name of its own (`parser::IF_LET_PREFIX`).
    if_lets: usize,
    /// How many `for (a, b) in ...` heads have been desugared, so each one's
    /// hidden element binding has a name of its own (`FOR_PATTERN_PREFIX`).
    for_patterns: usize,
    /// The loops the statement being parsed sits inside, innermost last, so a
    /// `break 'outer` can find its target. A function, a `fun` literal and a
    /// trailing block start a fresh stack: a label does not cross them.
    loop_stack: Vec<LoopCtx>,
    /// How many labelled exits have needed a flag (`LABEL_FLAG_PREFIX`).
    label_flags: usize,
    /// The span of every `.return:` return marker read so far (outdated).
    outdated_returns: Vec<Span>,
    /// How many tuple `when`s and variant levels have been rewritten, so each
    /// hidden name is its own (`WHEN_TUPLE_PREFIX`).
    when_temps: usize,
    /// True while parsing the head of an `if`/`elif`/`while`/`for`/`when`, where
    /// a `{` after an expression opens the body and not a trailing block
    /// (`plan/trailing-closure.md`). A call's argument list and a block reset it.
    no_trailing_block: bool,
    /// How many nesting constructs (expression, type, block, pattern) the parser
    /// is inside right now; `nest` refuses to go past `MAX_NESTING`.
    depth: usize,
}

/// How deep source text may nest (parenthesized or prefix-chained expressions,
/// blocks, type arguments, patterns) before the parser refuses it. Every later
/// pass walks the tree recursively, so an unbounded depth is a stack overflow
/// (an abort, not a diagnostic) on a few thousand `(` characters. Written Kelp
/// stays far below this: the deepest fixture nests under 30.
const MAX_NESTING: usize = 200;

/// The hidden element name of a `for` with a destructuring pattern. No source
/// text can spell it, so it cannot collide with a program's own name.
const FOR_PATTERN_PREFIX: &str = "#forpat";

/// The hidden names a `when` over a tuple binds: the tuple itself and each
/// variant level's payload.
const WHEN_TUPLE_PREFIX: &str = "#wt";

/// The hidden flag a labelled `break`/`continue` sets to leave the loops
/// between it and its target.
const LABEL_FLAG_PREFIX: &str = "#lbl";

/// The start of the refusal for a literal position with no arm that matches
/// anything there.
const NO_LITERAL_FALLBACK: &str = "a position matched by literals needs an arm that matches anything there";

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

type PResult<T> = Result<T, ParseError>;

/// One entry of a parameter list `T`, `T: A + B`, or `const N: usize`
/// (A-2, A-20). The const form is a *value* parameter; every other entry is a
/// type parameter.
struct TypeParamParsed {
    name: String,
    bounds: Vec<TypeExpr>,
    /// `Some(ty)` for `const N: ty`, and then `bounds` is empty.
    const_ty: Option<String>,
    span: Span,
}

/// The refusal for a `*` that does not stand alone in a `caps(...)` clause
/// (`caps(*, fs = [...])` and `caps(fs = [...], *)`). `*` already means every
/// domain, so a named domain beside it has nothing left to say; the message
/// names the spelling that does mean what the writer wanted.
fn mixing_wildcard_error(span: Span) -> ParseError {
    ParseError {
        message: "`caps(*)` already means every domain, so nothing can stand beside the `*`: \
                  write `caps(*)` alone, or name the domains it needs \
                  (`caps(net = [\"host:port\"])`, `caps(fs = [\"/a/**\"], env = [\"HOME\"])`)"
            .to_string(),
        span,
    }
}

/// The name prefix of the local an `if let` binds its scrutinee to. `#` cannot
/// appear in a lexed identifier, so a synthesized name can never collide with
/// one the user wrote — the same trick `infer.rs` uses for a hidden passdown
/// parameter.
const IF_LET_PREFIX: &str = "#iflet";

/// The span of the first `impl Trait` anywhere in a written type, when there
/// is one (A-20). Used to refuse the shapes the sugar does not cover: a nested
/// position (`List<impl Shape>`), a return type, a field and a `let`.
fn impl_trait_anywhere(ty: &TypeExpr) -> Option<Span> {
    if ty.impl_trait.is_some() {
        return Some(ty.span);
    }
    if let Some(elem) = &ty.array_elem {
        if let Some(s) = impl_trait_anywhere(elem) {
            return Some(s);
        }
    }
    for a in &ty.args {
        if let Some(s) = impl_trait_anywhere(a) {
            return Some(s);
        }
    }
    if let Some(elems) = &ty.tuple {
        for e in elems {
            if let Some(s) = impl_trait_anywhere(e) {
                return Some(s);
            }
        }
    }
    if let Some(params) = &ty.fn_params {
        for p in params {
            if let Some(s) = impl_trait_anywhere(&p.ty) {
                return Some(s);
            }
        }
    }
    if let Some(ret) = &ty.fn_ret {
        if let Some(s) = impl_trait_anywhere(ret) {
            return Some(s);
        }
    }
    None
}

/// `impl Trait` in argument position (A-20): each occurrence becomes a fresh
/// bounded type parameter, and the parameter's type becomes that parameter.
/// `&impl Trait` keeps its `&`, which is the ordinary borrow a parameter
/// already carries.
fn desugar_impl_trait_params(
    params: Vec<Param>,
    mut type_params: Vec<String>,
    mut type_param_bounds: Vec<Vec<TypeExpr>>,
    is_method: bool,
    is_signature: bool,
    span: Span,
) -> PResult<(Vec<Param>, Vec<String>, Vec<Vec<TypeExpr>>)> {
    let mut out = Vec::with_capacity(params.len());
    let mut fresh = 0usize;
    for mut p in params {
        if let Some(trait_name) = p.ty.impl_trait.clone() {
            if is_signature {
                return Err(ParseError {
                    message: "a trait method cannot write 'impl Trait': its signature names 'Self' and the trait's own types, and a method takes no type parameters of its own".to_string(),
                    span,
                });
            }
            if is_method {
                return Err(ParseError {
                    message: "'impl Trait' cannot be written on a method's parameter: a method's type parameters come from its receiver, so declare the parameter on a free function, or name the receiver type's own parameter".to_string(),
                    span: p.ty.span,
                });
            }
            let name = format!("#impl{fresh}");
            fresh += 1;
            type_params.push(name.clone());
            type_param_bounds.push(vec![TypeExpr::named(trait_name, p.ty.span)]);
            let mut replacement = TypeExpr::named(name, p.ty.span);
            replacement.amp = p.ty.amp;
            replacement.amp_mut = p.ty.amp_mut;
            p.ty = replacement;
        } else if let Some(span) = impl_trait_anywhere(&p.ty) {
            return Err(ParseError {
                message: "'impl Trait' is only legal as a whole parameter type: write it bare ('x: impl Trait') or behind a borrow ('x: &impl Trait'), not nested inside another type".to_string(),
                span,
            });
        }
        out.push(p);
    }
    Ok((out, type_params, type_param_bounds))
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            if_lets: 0,
            for_patterns: 0,
            loop_stack: Vec::new(),
            label_flags: 0,
            outdated_returns: Vec::new(),
            when_temps: 0,
            no_trailing_block: false,
            depth: 0,
        }
    }

    /// Refuses a left-leaning chain (`a < b < c ...`, `x.f.g.h ...`, `e???`)
    /// longer than `MAX_NESTING`: the loop that builds it does not recurse, but
    /// the tree it leaves is as deep as the chain is long.
    fn chain_limit(&self, links: usize) -> PResult<()> {
        if links > MAX_NESTING * 4 {
            return Err(ParseError {
                message: format!(
                    "this chain of operators or accesses is longer than {} links; split it into smaller pieces",
                    MAX_NESTING * 4
                ),
                span: self.peek().span,
            });
        }
        Ok(())
    }

    /// Enters one nesting level, or reports the depth limit. The caller undoes
    /// it with `self.depth -= 1` on every path out (the wrappers below do).
    fn nest(&mut self) -> PResult<()> {
        if self.depth >= MAX_NESTING {
            return Err(ParseError {
                message: format!(
                    "this nests more than {MAX_NESTING} levels deep (expressions, blocks, types and patterns inside one another); split it into smaller pieces"
                ),
                span: self.peek().span,
            });
        }
        self.depth += 1;
        Ok(())
    }

    /// Parses the expression in an `if`/`while`/`for`/`when` head, where a
    /// following `{` is the body and never a trailing block.
    fn parse_head_expr(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_trailing_block, true);
        let e = self.parse_expr();
        self.no_trailing_block = saved;
        e
    }

    /// A `{ ... }` on the same line as the end of a call is that call's last
    /// argument, exactly what `f(fun() { ... })` writes. Nothing else follows
    /// a call on the same line with a `{`, so this only ever accepts text that
    /// was a parse error.
    ///
    /// A `{ x, y -> ... }` header declares the block's parameters by *name*
    /// only; their types come from the expected function type of the parameter
    /// the block is passed to (`plan/scoped-blocks.md` part 1), so the literal
    /// is marked `infer_params`. The header is read only when the first tokens
    /// are identifiers, comma separated, followed by `->`, which appears
    /// nowhere else at the start of a block.
    fn parse_trailing_block(&mut self, args: &mut Vec<CallArg>, end: &mut Span) -> PResult<()> {
        if self.no_trailing_block || !self.check(&TokenKind::LBrace) {
            return Ok(());
        }
        let start = self.advance().span;
        let mut params = Vec::new();
        if self.block_header_starts_params() {
            loop {
                let (name, span) = self.expect_ident()?;
                params.push(Param {
                    name,
                    ty: TypeExpr::named("", span),
                    mode: ParamMode::Borrow,
                });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            self.expect(TokenKind::Arrow, "'->' after the block's parameters")?;
        }
        let outer = std::mem::take(&mut self.loop_stack);
        let block = self.parse_block_body(start);
        self.loop_stack = outer;
        let block = block?;
        let span = block.span;
        *end = span;
        args.push(CallArg {
            name: None,
            value: Expr::FnLit {
                params,
                infer_params: true,
                return_type: None,
                body: Box::new(FunctionBody::Block(block)),
                span,
            },
        });
        Ok(())
    }

    /// Whether the `{` the parser has already consumed opens a block whose
    /// first tokens are a parameter header: one or more identifiers, comma
    /// separated, then `->`. A lookahead only — nothing is consumed.
    fn block_header_starts_params(&self) -> bool {
        let mut i = self.pos;
        if !matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Ident(_))) {
            return false;
        }
        loop {
            i += 1;
            match self.tokens.get(i).map(|t| &t.kind) {
                Some(TokenKind::Arrow) => return true,
                Some(TokenKind::Comma) => {
                    i += 1;
                    if !matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Ident(_))) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// The kind of the token one past the current one (`Eof` past the end).
    fn peek2_kind(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 1)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(kind)
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> PResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                message: format!("expected {what}, found {:?}", self.peek_kind()),
                span: self.peek().span,
            })
        }
    }

    fn expect_ident(&mut self) -> PResult<(String, Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.peek().span;
                self.advance();
                Ok((name, span))
            }
            other => Err(ParseError {
                message: format!("expected identifier, found {other:?}"),
                span: self.peek().span,
            }),
        }
    }

    /// Consumes zero or more `Newline` tokens (blank lines already collapse
    /// to a single token in the lexer).
    fn skip_newlines(&mut self) {
        while self.check(&TokenKind::Newline) {
            self.advance();
        }
    }

    /// True at a point where a statement (or `when`-arm) may legally end:
    /// a newline, the block's closing `}`, or end of input.
    fn at_stmt_end(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof
        )
    }

    pub fn parse_program(&mut self) -> PResult<Program> {
        self.skip_newlines();
        let mut impls = Vec::new();
        let mut consts = Vec::new();
        let mut statics = Vec::new();
        let mut pds = Vec::new();
        let mut datacs = Vec::new();
        let mut enums = Vec::new();
        let mut extern_types = Vec::new();
        let mut traits = Vec::new();
        let mut trait_impls = Vec::new();
        let mut functions = Vec::new();
        // One counter across both type declarations: it is what `resolve.rs`
        // uses to rebuild a single merged declaration order from the two
        // lists (A-1).
        let mut type_order = 0usize;
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            // `repr(...)` is an optional leading clause on a `datac`/`enum`
            // (kernel stage 2), `section("...")` one on a `const`/`fun`
            // (kernel stage 4), and `pub` is an optional leading token on the
            // item forms that can carry one (A-13). They are read here, before
            // the dispatch below, in either order round `pub`, because a
            // modified item is otherwise the same declaration.
            let (leading_section, leading_align) = self.parse_section_and_align_clauses()?;
            let leading_repr = self.parse_repr_clause()?;
            let is_pub = self.check(&TokenKind::Pub);
            if is_pub {
                let pub_span = self.advance().span;
                self.check_pub_target(pub_span)?;
            }
            let repr = match (leading_repr, self.parse_repr_clause()?) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        message: "a 'repr(...)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            let (after_section, after_align) = self.parse_section_and_align_clauses()?;
            let section = match (leading_section, after_section) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        message: "a 'section(...)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            let align = match (leading_align, after_align) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        message: "an 'align(N)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            // `percpu static ...` (kernel stage 8): `percpu` is a contextual
            // word that modifies the `static` it precedes. It is read after
            // the `section(...)`/`align(N)` clauses so a modified item stays
            // the same declaration wherever the clauses are written.
            let percpu = self.check_percpu_marker();
            if !percpu && matches!(self.peek_kind(), TokenKind::Ident(n) if n == "percpu") {
                return Err(ParseError {
                    message: "'percpu' applies to a 'static', written 'percpu static mut NAME: T = INIT'"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            if repr.is_some() && !self.check(&TokenKind::Datac) && !self.check(&TokenKind::Enum) {
                return Err(ParseError {
                    message: format!(
                        "'repr(...)' applies to a 'datac' or an 'enum', found {:?}",
                        self.peek_kind()
                    ),
                    span: self.peek().span,
                });
            }
            if section.is_some() && !self.check(&TokenKind::Const) && !self.check(&TokenKind::Static) && !self.at_function_decl() {
                return Err(ParseError {
                    message: format!(
                        "'section(\"...\")' applies to a top-level 'const', 'static' or 'fun', found {:?}",
                        self.peek_kind()
                    ),
                    span: self.peek().span,
                });
            }
            if align.is_some() && !self.at_function_decl() {
                return Err(ParseError {
                    message: format!(
                        "'align(N)' applies to a top-level 'fun', found {:?}",
                        self.peek_kind()
                    ),
                    span: self.peek().span,
                });
            }
            if self.check(&TokenKind::Impl) && !self.at_module_impl() {
                // `impl Name for Type { ... }` (A-12 slice 2). It declares
                // methods, so it belongs to the last group with `datac`,
                // `enum`, `fun` and `trait`, and may sit anywhere among them.
                let (decl, methods) = self.parse_trait_impl_decl()?;
                trait_impls.push(decl);
                functions.extend(methods);
            } else if self.check(&TokenKind::Trait) {
                traits.push(self.parse_trait_decl(is_pub)?);
            } else if self.check(&TokenKind::Impl) {
                if !consts.is_empty() || !statics.is_empty() || !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'impl' lines must come first (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                impls.push(self.parse_impl_decl()?);
            } else if self.check(&TokenKind::Const) {
                if !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'const' lines must come before any 'pd', 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                consts.push(self.parse_const_decl(is_pub, section)?);
            } else if self.check(&TokenKind::Static) {
                if !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'static' lines must come before any 'pd', 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/static/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                statics.push(self.parse_static_decl(is_pub, section, percpu)?);
            } else if self.check(&TokenKind::Pd) {
                if !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'pd' lines must come before any 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                pds.push(self.parse_pd_decl()?);
            } else if self.check(&TokenKind::Datac) {
                let mut d = self.parse_datac_decl(is_pub, repr)?;
                d.order = type_order;
                type_order += 1;
                datacs.push(d);
            } else if self.check(&TokenKind::Enum) {
                let mut e = self.parse_enum_decl(is_pub, repr)?;
                e.order = type_order;
                type_order += 1;
                enums.push(e);
            } else if self.check(&TokenKind::Extern)
                && matches!(self.peek2_kind(), TokenKind::Ident(ref n) if n == "type")
            {
                extern_types.push(self.parse_extern_type_decl(is_pub)?);
            } else {
                // `unsafe fun ...` / `naked fun ...` (MK9-S2 stage 1, kernel
                // stage 5). Both are contextual words everywhere, so they are
                // identifier tokens and the lookahead is what makes them
                // declaration markers here.
                let (unsafe_, naked, must_use) = self.check_function_markers();
                let mut f = self.parse_function(is_pub, unsafe_, naked, section, align, false)?;
                f.must_use = must_use;
                // `Self` is a receiver only inside a `trait` or an `impl`
                // (A-12 slice 2): at the top level it names nothing.
                if let Some(r) = &f.receiver {
                    if r.type_name == "Self" {
                        return Err(ParseError {
                            message: "'Self' can only be written inside a 'trait' or an 'impl': it names the implementing type, so a method of a named type writes that type ('fun Circle.area()')".to_string(),
                            span: r.span,
                        });
                    }
                }
                functions.push(f);
            }
            self.skip_newlines();
        }
        Ok(Program {
            impls,
            consts,
            statics,
            pds,
            datacs,
            enums,
            extern_types,
            traits,
            trait_impls,
            functions,
            outdated_returns: std::mem::take(&mut self.outdated_returns),
        })
    }

    /// The `return` of `.return: T` after a parameter list, or the outdated
    /// `R` it replaces (`deprecated::RETURN_R`). The `R` spelling still
    /// compiles and its span is kept, so the resolver can report it.
    fn expect_return_marker(&mut self) -> PResult<()> {
        if self.check(&TokenKind::Return) {
            self.advance();
            return Ok(());
        }
        let (marker, mspan) = self.expect_ident()?;
        if marker != "R" {
            return Err(ParseError {
                message: format!("expected 'return' after '.', found '{marker}'"),
                span: mspan,
            });
        }
        self.outdated_returns.push(mspan);
        Ok(())
    }

    /// The `percpu` marker before a top-level `static` (kernel stage 8). Like
    /// `unsafe`/`naked`, it is a contextual word: it is only a marker when
    /// `static` is what follows it, so a program can still use `percpu` as an
    /// ordinary name. Nothing is consumed otherwise.
    fn check_percpu_marker(&mut self) -> bool {
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "percpu")
            && matches!(self.peek2_kind(), TokenKind::Static)
        {
            self.advance();
            true
        } else {
            false
        }
    }

    /// The declaration markers before `fun`: `unsafe` and `naked` (MK9-S2
    /// stage 1, kernel stage 5). Neither is a keyword, so each is only a marker
    /// when `fun` is what follows the run of them, and nothing is consumed
    /// otherwise.
    fn check_function_markers(&mut self) -> (bool, bool, bool) {
        let mut unsafe_ = false;
        let mut naked = false;
        let mut must_use = false;
        let mut i = 0;
        loop {
            match self.tokens.get(self.pos + i).map(|t| &t.kind) {
                Some(TokenKind::Ident(n)) if n == "unsafe" => unsafe_ = true,
                Some(TokenKind::Ident(n)) if n == "naked" => naked = true,
                Some(TokenKind::Ident(n)) if n == "must_use" => must_use = true,
                _ => break,
            }
            i += 1;
        }
        if !matches!(
            self.tokens.get(self.pos + i).map(|t| &t.kind),
            Some(TokenKind::Fun)
        ) {
            return (false, false, false);
        }
        for _ in 0..i {
            self.advance();
        }
        (unsafe_, naked, must_use)
    }

    /// The item a leading `pub` must be followed by (A-13). `pub` on anything
    /// else is refused where it is written, naming what it applies to, rather
    /// than read as a no-op or as something else. A `pd` gets its own message
    /// because it is not a missing feature: a passdown is module-scoped by
    /// design (`plan/roadmap.md` item 10), so `pub` on one can never be true.
    fn check_pub_target(&self, pub_span: Span) -> PResult<()> {
        match self.peek_kind() {
            TokenKind::Fun
            | TokenKind::Extern
            | TokenKind::Datac
            | TokenKind::Enum
            | TokenKind::Trait
            | TokenKind::Const
            | TokenKind::Static => Ok(()),
            // `pub unsafe fun ...` / `pub naked fun ...`, in either order: both
            // are contextual words, so each is only a marker when `fun` follows
            // the run of them.
            TokenKind::Ident(n)
                if (n == "unsafe" || n == "naked" || n == "must_use")
                    && (matches!(self.peek2_kind(), TokenKind::Fun)
                        || matches!(self.peek2_kind(), TokenKind::Ident(m) if m == "unsafe" || m == "naked" || m == "must_use")) =>
            {
                Ok(())
            }
            // `pub repr(c) datac ...`: a `repr(...)` clause may sit between
            // `pub` and the type declaration; the dispatch after it is what
            // requires a 'datac' or an 'enum'.
            TokenKind::Ident(n)
                if n == "repr" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            // `pub section(".x") fun f() ...`: a `section(...)` clause may sit
            // between `pub` and the `const`/`fun` it modifies (kernel stage 4).
            TokenKind::Ident(n)
                if n == "section" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            // `pub percpu static ...`: `percpu` may sit between `pub` and the
            // `static` it modifies (kernel stage 8).
            TokenKind::Ident(n) if n == "percpu" && matches!(self.peek2_kind(), TokenKind::Static) => {
                Ok(())
            }
            // `pub align(4) fun f() ...`: an `align(N)` clause may sit between
            // `pub` and the `fun` it modifies (kernel gap 2).
            TokenKind::Ident(n)
                if n == "align" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            TokenKind::Pd => Err(ParseError {
                message: "a 'pd' cannot be 'pub': a passdown is module-scoped by design, so another module cannot reach it; a 'fun' that reads it can be exported instead".to_string(),
                span: pub_span.to(self.peek().span),
            }),
            _ => Err(ParseError {
                message: format!(
                    "'pub' applies to a top-level 'fun', 'extern fun', 'extern type', 'datac', 'enum', 'const' or 'static', found {:?}",
                    self.peek_kind()
                ),
                span: pub_span.to(self.peek().span),
            }),
        }
    }

    /// `const NAME: T = expr` — the type is mandatory, since a `const` is a
    /// declared contract rather than an inferred binding.
    fn parse_const_decl(&mut self, is_pub: bool, section: Option<String>) -> PResult<ConstDecl> {
        let start = self.expect(TokenKind::Const, "'const'")?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':' (a 'const' must declare its type)")?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(ConstDecl {
            name,
            is_pub,
            ty,
            init,
            module: String::new(),
            section,
            span,
        })
    }

    /// `static NAME: T = expr` / `static mut NAME: T = expr` — a top-level
    /// global object (kernel stage 7), optionally with a leading `percpu`
    /// (kernel stage 8). The type is mandatory, as it is on a `const`, and the
    /// initializer must fold at compile time.
    fn parse_static_decl(
        &mut self,
        is_pub: bool,
        section: Option<String>,
        percpu: bool,
    ) -> PResult<StaticDecl> {
        let start = self.expect(TokenKind::Static, "'static'")?.span;
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':' (a 'static' must declare its type)")?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(StaticDecl {
            name,
            is_pub,
            mutable,
            percpu,
            ty,
            init,
            module: String::new(),
            section,
            span,
        })
    }

    /// `pd val name = expr` / `pd var name = expr` / `pd val name: T = expr`.
    fn parse_pd_decl(&mut self) -> PResult<PdDecl> {
        let start = self.expect(TokenKind::Pd, "'pd'")?.span;
        let mutable = if self.check(&TokenKind::Var) {
            self.advance();
            true
        } else {
            self.expect(TokenKind::Val, "'val' or 'var'")?;
            false
        };
        let (name, _) = self.expect_ident()?;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(PdDecl {
            name,
            mutable,
            ty,
            init,
            module: String::new(),
            span,
        })
    }

    /// `impl "shapes/circle"` and the capability grant
    /// `impl "std/fs" caps(fs = ["/var/log/myapp/*"])` (M7).
    /// `with caps(...)` is refused by name, the way `as name` is: both were
    /// deprecated by `26.09.0` and removed in the cycle that followed.
    fn parse_impl_decl(&mut self) -> PResult<ImplDecl> {
        let start = self.expect(TokenKind::Impl, "'impl'")?.span;
        let (path, path_span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    message: format!("expected a quoted module path after 'impl', found {other:?}"),
                    span: self.peek().span,
                })
            }
        };
        // `as name` was removed once a module's own name became its namespace
        // (`syntax.md` 13.1). The token is still a keyword, so the old clause is
        // refused here, where the path is in hand, rather than falling through
        // to the generic "expected an item" error further up.
        if self.check(&TokenKind::As) {
            return Err(ParseError {
                message: "`impl \"...\" as name` is gone: reach the module by its own name \
                          (`std/path` is `path`, `shapes/circle` is `circle`, and a dependency's \
                          entry module `deps/argparse/lib` is `argparse`)"
                    .to_string(),
                span: self.peek().span,
            });
        }
        // `with caps(...)` was the keyword in front of the grant, and it was
        // removed with the entry that scheduled it (`deprecated.rs`, `syntax.md`
        // 23): `impl` already means "this module declares what it imports", so
        // the `with` states nothing the clause does not. It is refused here,
        // where the path is in hand and the surviving spelling is known, rather
        // than falling through to the generic "expected an item" error above the
        // grants this clause still accepts.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "with") {
            return Err(ParseError {
                message: "`impl \"...\" with caps(...)` is gone: drop the `with`, so the grant is \
                          `impl \"std/fs\" caps(fs = [\"/var/log/**\"])`, and the patterns, the \
                          domains and the no-escalation rule are exactly the same"
                    .to_string(),
                span: self.peek().span,
            });
        }
        // `caps(domain = ["pattern", ...], ...)` — the capability grant
        // (M7). Domains and patterns are validated in `resolve.rs`, where the
        // module graph (and therefore the importer's own set) is known.
        // `caps(*)` is the one alternative spelling: it delegates the whole
        // resource of every domain, so it stands alone and is refused here
        // when a named domain stands beside it.
        let has_caps_clause = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "caps") {
            self.advance();
            true
        } else {
            false
        };
        let mut caps_all = false;
        let caps = if has_caps_clause {
            self.expect(TokenKind::LParen, "'(' after 'caps'")?;
            let mut grants: Vec<CapGrant> = Vec::new();
            if self.check(&TokenKind::Star) {
                let star_span = self.peek().span;
                self.advance();
                caps_all = true;
                if !self.check(&TokenKind::RParen) {
                    return Err(mixing_wildcard_error(star_span));
                }
            }
            while !self.check(&TokenKind::RParen) {
                if self.check(&TokenKind::Star) {
                    return Err(mixing_wildcard_error(self.peek().span));
                }
                let (domain, dspan) = self.expect_ident()?;
                self.expect(TokenKind::Eq, "'=' after the capability domain")?;
                self.expect(TokenKind::LBracket, "'[' before the pattern list")?;
                let mut patterns = Vec::new();
                while !self.check(&TokenKind::RBracket) {
                    match self.peek_kind().clone() {
                        TokenKind::StringLit(p) => {
                            self.advance();
                            patterns.push(p);
                        }
                        other => {
                            return Err(ParseError {
                                message: format!(
                                    "expected a quoted path or host pattern, found {other:?}"
                                ),
                                span: self.peek().span,
                            })
                        }
                    }
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RBracket, "']'")?;
                grants.push(CapGrant {
                    domain,
                    patterns,
                    span: dspan,
                });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
            grants
        } else {
            Vec::new()
        };
        Ok(ImplDecl {
            path,
            caps,
            caps_all,
            span: start.to(path_span),
        })
    }

    /// `trait Name { fun Self.m(...).return: T ... }` (A-12 slice 2). The body is
    /// methods only: an associated item is refused here, where the reason can
    /// name the follow-up. A method with a body is a default (A-12 slice 3).
    fn parse_trait_decl(&mut self, is_pub: bool) -> PResult<TraitDecl> {
        let start = self.expect(TokenKind::Trait, "'trait'")?.span;
        let (name, name_span) = self.expect_ident()?;
        if name == "Drop" {
            return Err(ParseError {
                message: "the trait 'Drop' is built in: a type's destructor is written 'fun mut T.drop()', not through a trait"
                    .to_string(),
                span: name_span,
            });
        }
        if name == "Self" {
            return Err(ParseError {
                message: "'Self' is the implementing type inside a 'trait' or an 'impl', so it cannot be a trait's own name"
                    .to_string(),
                span: name_span,
            });
        }
        if crate::types::Ty::from_name(&name).is_some() {
            return Err(ParseError {
                message: format!(
                    "'{name}' is a builtin type, so it cannot be a trait's name"
                ),
                span: name_span,
            });
        }
        let name_params = self.parse_type_params()?;
        if let Some(p) = name_params.iter().find(|p| p.const_ty.is_some()) {
            return Err(ParseError {
                message: format!(
                    "a trait cannot take a const parameter ('const {}'): a const parameter is a value on a type or a function (A-20), and a trait is implemented for a type",
                    p.name
                ),
                span: p.span,
            });
        }
        let type_params: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
        let type_param_bounds: Vec<Vec<TypeExpr>> =
            name_params.into_iter().map(|p| p.bounds).collect();
        // `trait B: A + C` (A-12 slice 3): the supertraits, resolved in the
        // trait's own module exactly as a bound on a function's type parameter
        // is. A cycle is refused by `infer.rs`, where the whole trait table is
        // in hand.
        let supertraits = if self.check(&TokenKind::Colon) {
            self.advance();
            self.parse_plain_bounds("a supertrait")?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::LBrace, "'{' to open the trait's body")?;
        let mut methods = Vec::new();
        let mut assoc_types: Vec<AssocTypeDecl> = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::RBrace) {
            // `type Item` / `type Item: Named` (A-12 slice 5). `type` is not a
            // keyword, so it is an identifier here and the next token being a
            // name is what makes this the declaration.
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "type")
                && matches!(self.peek2_kind(), TokenKind::Ident(_))
            {
                self.advance();
                let (name, name_span) = self.expect_ident()?;
                if self.check(&TokenKind::Lt) {
                    return Err(ParseError {
                        message: "a generic associated type is not in this slice: 'type Item' declares a plain type, so parameters wait for a later slice".to_string(),
                        span: self.peek().span,
                    });
                }
                let bounds = if self.check(&TokenKind::Colon) {
                    self.advance();
                    self.parse_plain_bounds("an associated type's bound")?
                } else {
                    Vec::new()
                };
                if self.check(&TokenKind::Eq) {
                    return Err(ParseError {
                        message: "a default associated type is not in this slice: write 'type Item' in the trait and bind it in every 'impl' ('type Item = i32'), because a default type would let an impl inherit a type it never names".to_string(),
                        span: self.peek().span,
                    });
                }
                if assoc_types.iter().any(|a| a.name == name) {
                    return Err(ParseError {
                        message: format!("trait declares 'type {name}' more than once"),
                        span: name_span,
                    });
                }
                assoc_types.push(AssocTypeDecl {
                    name,
                    bounds,
                });
                self.skip_newlines();
                continue;
            }
            if !self.check(&TokenKind::Fun) {
                // Any other item form: a trait body is method signatures and
                // associated types only.
                let what = match self.peek_kind() {
                    TokenKind::Ident(n) => format!(
                        "'{n}' is not part of a trait: a trait body declares methods ('fun Self.m(...)') and associated types ('type Item')"
                    ),
                    TokenKind::Const => "associated consts are not in this slice: a trait declares methods and associated types only"
                        .to_string(),
                    other => format!(
                        "a trait body declares methods and associated types, so 'fun' or 'type' is expected here, found {other:?}"
                    ),
                };
                return Err(ParseError {
                    message: what,
                    span: self.peek().span,
                });
            }
            methods.push(self.parse_function(false, false, false, None, None, true)?);
            self.skip_newlines();
        }
        let end = self.expect(TokenKind::RBrace, "'}' to close the trait's body")?.span;
        Ok(TraitDecl {
            name,
            is_pub,
            type_params,
            type_param_bounds,
            supertraits,
            methods,
            assoc_types,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// `impl Name for Type { fun Self.m(...) { ... } ... }` (A-12 slice 2).
    /// Returns the pairing (what `infer.rs` checks coherence and the exact
    /// signatures against) and the methods, which are ordinary receiver
    /// methods from here on: `resolve.rs` substitutes `Self` with the `for`
    /// type before any later pass sees them.
    fn parse_trait_impl_decl(&mut self) -> PResult<(TraitImplDecl, Vec<FunctionDecl>)> {
        let impl_span = self.expect(TokenKind::Impl, "'impl'")?.span;
        // `impl<T: Bound> Trait for Type<T>`: the impl's own type parameters.
        let impl_params = self.parse_type_params()?;
        if let Some(p) = impl_params.iter().find(|p| p.const_ty.is_some()) {
            return Err(ParseError {
                message: format!("an impl cannot take the const parameter '{}' yet: only type parameters", p.name),
                span: p.span,
            });
        }
        let (trait_name, trait_span) = self.parse_dotted_name()?;
        // `impl From<IoErr> for AppErr` (A-12 slice 6): the trait's arguments,
        // which name the trait instance this block implements.
        let trait_args: Vec<TypeExpr> = if self.check(&TokenKind::Lt) {
            self.parse_type_arg_list()?.0
        } else {
            Vec::new()
        };
        if trait_name == "Drop" || trait_name.ends_with(".Drop") {
            return Err(ParseError {
                message: "the trait 'Drop' is built in, so it cannot be implemented: a type's destructor is written 'fun mut T.drop()' inside its own module"
                    .to_string(),
                span: trait_span,
            });
        }
        self.expect(TokenKind::For, "'for' after the trait name ('impl Name for Type') and not 'impl \"path\"', which imports a module")?;
        let (type_name, type_span) = self.parse_dotted_name()?;
        if type_name == "Self" || type_name.ends_with(".Self") {
            return Err(ParseError {
                message: "the 'for' type names the implementing type, not 'Self': 'Self' is what the methods of this block call it"
                    .to_string(),
                span: type_span,
            });
        }
        let mut impl_bounds: Vec<Vec<TypeExpr>> = impl_params.iter().map(|p| p.bounds.clone()).collect();
        // `impl<T: Bound> Trait for T`: a blanket impl, whose type is the impl's
        // own parameter. It applies to every type that meets the bound.
        let blanket = impl_params.iter().any(|p| p.name == type_name);
        if blanket {
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    message: format!("a blanket impl names its parameter as the type it implements for: 'impl<{type_name}: Bound> Trait for {type_name}', with no '<...>' after '{type_name}'"),
                    span: type_span,
                });
            }
            if impl_params.len() != 1 {
                return Err(ParseError {
                    message: format!("a blanket impl has exactly one type parameter, the type it implements for: 'impl<{type_name}: Bound> Trait for {type_name}'"),
                    span: type_span,
                });
            }
            if !trait_args.is_empty() {
                return Err(ParseError {
                    message: format!("a blanket impl of a trait with type arguments is not supported yet: 'impl<{type_name}: Bound> Trait<...> for {type_name}' has no argument to bind the instance by, so write the trait without its arguments"),
                    span: type_span,
                });
            }
        } else if self.check(&TokenKind::Lt) {
            // `Type<T, U>`: the generic type with the impl's own parameters, in
            // order. Anything else (`Pair<i32>`, `Pair<List<T>>`) would be an
            // impl for one instantiation, which a template cannot express.
            let (args, args_span) = self.parse_type_arg_list()?;
            let same = args.len() == impl_params.len()
                && args.iter().zip(impl_params.iter()).all(|(a, p)| {
                    a.name == p.name && a.args.is_empty() && a.tuple.is_none() && !a.amp && a.array_elem.is_none()
                        && a.fn_params.is_none()
                });
            if impl_params.is_empty() || !same {
                return Err(ParseError {
                    message: "an impl for a generic type writes the type's parameters, in order, as its own: 'impl<T: Bound> Trait for Type<T>'; an impl for one instantiation ('Type<i32>') is not supported - implement the trait for a non-generic wrapper, or give the type an inherent method".to_string(),
                    span: args_span,
                });
            }
        } else if !impl_params.is_empty() {
            return Err(ParseError {
                message: "'impl<...>' needs the generic type it is for: 'impl<T: Bound> Trait for Type<T>', or a blanket impl 'impl<T: Bound> Trait for T' with the parameter as the type".to_string(),
                span: type_span,
            });
        }
        // `where T: A + B` after the header: more bounds on the impl's parameters,
        // and `where T::Item: B` on an associated type projected from one.
        let impl_param_names: Vec<String> = impl_params.iter().map(|p| p.name.clone()).collect();
        let mut impl_projections: Vec<ProjectionBound> = Vec::new();
        self.parse_where_clause(&impl_param_names, &mut impl_bounds, &mut impl_projections, "impl")?;
        self.expect(TokenKind::LBrace, "'{' to open the impl's body")?;
        let mut methods = Vec::new();
        let mut names = Vec::new();
        let mut assoc_types: Vec<AssocTypeBinding> = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::RBrace) {
            // `type Item = i32` (A-12 slice 5). `type` is not a keyword, so it
            // is an identifier and the name after it is what makes this the
            // binding.
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "type")
                && matches!(self.peek2_kind(), TokenKind::Ident(_))
            {
                let start = self.advance().span;
                let (name, name_span) = self.expect_ident()?;
                if self.check(&TokenKind::Colon) {
                    return Err(ParseError {
                        message: format!(
                            "a bound on 'type {name}' is written in the trait's declaration, not in the impl: write 'type {name}: Named' in the 'trait' body, and bind it here with 'type {name} = <concrete type>'"
                        ),
                        span: self.peek().span,
                    });
                }
                if !self.check(&TokenKind::Eq) {
                    return Err(ParseError {
                        message: format!(
                            "'type {name}' needs the type the impl binds: write 'type {name} = <concrete type>'"
                        ),
                        span: self.peek().span,
                    });
                }
                self.advance();
                let ty = self.parse_type()?;
                let end = ty.span;
                if assoc_types.iter().any(|a| a.name == name) {
                    return Err(ParseError {
                        message: format!("'type {name}' is bound more than once in this impl"),
                        span: name_span,
                    });
                }
                assoc_types.push(AssocTypeBinding {
                    name,
                    ty,
                    span: start.to(end),
                });
                self.skip_newlines();
                continue;
            }
            if !self.check(&TokenKind::Fun) {
                return Err(ParseError {
                    message: "an 'impl' block declares methods ('fun Self.name(...)') and associated types ('type Name = Type'): move anything else to the top level"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            let mut f = self.parse_function(false, false, false, None, None, false)?;
            match &f.receiver {
                Some(r) if r.type_name == "Self" && !r.type_args.is_empty() => {
                    return Err(ParseError {
                        message: format!(
                            "an 'impl' method cannot take type parameters: the block already names the type it implements for, so write 'fun Self.{}(...)'",
                            f.name
                        ),
                        span: r.span,
                    })
                }
                Some(r) if r.type_name == "Self" => {
                    // A generic impl's methods are template methods of the type.
                    if !impl_params.is_empty() {
                        if let Some(r) = &mut f.receiver {
                            r.type_args = impl_params.iter().map(|p| p.name.clone()).collect();
                        }
                        f.receiver_bounds = impl_bounds.clone();
                    }
                }
                Some(r) => {
                    return Err(ParseError {
                        message: format!(
                            "an 'impl' method's receiver is written 'Self', not '{}': 'Self' stands for the type the block implements for ('fun Self.{}(...)')",
                            r.type_name, f.name
                        ),
                        span: r.span,
                    })
                }
                None if blanket => {
                    return Err(ParseError {
                        message: format!(
                            "a blanket impl has no static member yet: '{}' has no receiver to take the type from, so write it as a method ('fun Self.{}(...)')",
                            f.name, f.name
                        ),
                        span: f.span,
                    })
                }
                None => {}
            }
            f.trait_impl = Some(TraitImplSite {
                trait_name: trait_name.clone(),
                trait_args: trait_args.clone(),
                trait_instance: String::new(),
                self_type: type_name.clone(),
            });
            names.push(f.name.clone());
            methods.push(f);
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "'}' to close the impl's body")?;
        Ok((
            TraitImplDecl {
                trait_name,
                trait_args,
                trait_arg_names: Vec::new(),
                trait_instance: String::new(),
                type_name,
                trait_span,
                type_span,
                methods: names,
                assoc_types,
                module: String::new(),
                impl_span,
                type_params: impl_params.iter().map(|p| p.name.clone()).collect(),
                type_param_bounds: impl_bounds,
                projection_bounds: impl_projections,
            },
            methods,
        ))
    }

    fn parse_datac_decl(&mut self, is_pub: bool, repr: Option<Repr>) -> PResult<DatacDecl> {
        let start = self.expect(TokenKind::Datac, "'datac'")?.span;
        let (name, _) = self.expect_ident()?;
        let (type_params, const_params) =
            Parser::no_type_param_bounds(self.parse_type_params()?)?;
        self.expect(TokenKind::LParen, "'('")?;
        let fields = self.parse_field_list()?;
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        Ok(DatacDecl {
            name,
            is_pub,
            type_params,
            const_params,
            fields,
            repr,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// A `repr(c)` / `repr(packed)` / `repr(align(N))` / `repr(u8)` clause, or
    /// `None` when the cursor is not at one (kernel stage 2). `repr` is a
    /// contextual word, so it only counts when a `(` touches it, exactly as
    /// `unsafe` only counts when `fun` follows.
    fn parse_repr_clause(&mut self) -> PResult<Option<Repr>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "repr")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'repr'")?;
        let (word, word_span) = self.expect_ident()?;
        let repr = match word.as_str() {
            "c" => Repr::C,
            "packed" => Repr::Packed,
            "align" => {
                self.expect(TokenKind::LParen, "'(' after 'repr(align'")?;
                let (n, n_span) = match self.peek_kind().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        (n, self.tokens[self.pos - 1].span)
                    }
                    other => {
                        return Err(ParseError {
                            message: format!(
                                "expected an integer literal alignment in 'repr(align(N))', found {other:?}"
                            ),
                            span: self.peek().span,
                        })
                    }
                };
                self.expect(TokenKind::RParen, "')' after 'repr(align(N)'")?;
                if !n.is_power_of_two() {
                    return Err(ParseError {
                        message: format!(
                            "'repr(align({n}))' needs a power of two: 1, 2, 4, 8, 16, ... are the alignments a type can have"
                        ),
                        span: n_span,
                    });
                }
                // The bound is the C compilers': GCC and Clang refuse an
                // `aligned` above 2^28, and the LLVM alignment anchor is a
                // vector of `N` bytes, so a value that does not fit is a
                // refusal here rather than a nonsense type later.
                if n > 1 << 28 {
                    return Err(ParseError {
                        message: format!(
                            "'repr(align({n}))' is too large: the largest alignment is 268435456"
                        ),
                        span: n_span,
                    });
                }
                Repr::Align(n)
            }
            // `repr(u8)` and its siblings: an integer storage type for an
            // enum tag. Any other word is not a repr this language has.
            other => {
                let is_int = matches!(
                    other,
                    "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64"
                );
                if !is_int {
                    return Err(ParseError {
                        message: format!(
                            "unknown 'repr({other})': the clause is 'c', 'packed', 'align(N)' or an integer type ('u8', 'i8', 'u16', 'i16', 'u32', 'i32', 'u64', 'i64')"
                        ),
                        span: word_span,
                    });
                }
                Repr::Int(other.to_string())
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'repr(...)'")?;
        Ok(Some(repr))
    }

    /// `section("<name>")`, an optional leading clause on a top-level `const`
    /// or `fun` (kernel stage 4), or `None` when the cursor is not at one.
    /// `section` is a contextual word, so it only counts when a `(` touches it,
    /// exactly as `repr` and `unsafe` do.
    fn parse_section_clause(&mut self) -> PResult<Option<String>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "section")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'section'")?;
        let (name, name_span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    message: format!(
                        "expected a quoted section name in 'section(\"...\")', found {other:?}"
                    ),
                    span: self.peek().span,
                })
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'section(...)'")?;
        if name.is_empty() {
            return Err(ParseError {
                message: "'section(\"\")' needs a section name, for example 'section(\".text.boot\")'"
                    .to_string(),
                span: name_span,
            });
        }
        Ok(Some(name))
    }

    /// `section("...")` and `align(N)` are the two clauses of one family on a
    /// `fun` (kernel stages 4 and gap 2), and a program may write either or
    /// both, in either order. Each is consumed at most once; a second one is
    /// the caller's duplicate error.
    fn parse_section_and_align_clauses(&mut self) -> PResult<(Option<String>, Option<u32>)> {
        let mut section = None;
        let mut align = None;
        loop {
            if section.is_none() {
                if let Some(s) = self.parse_section_clause()? {
                    section = Some(s);
                    continue;
                }
            }
            if align.is_none() {
                if let Some(a) = self.parse_align_clause()? {
                    align = Some(a);
                    continue;
                }
            }
            break;
        }
        Ok((section, align))
    }

    /// `align(N)`, an optional leading clause on a top-level `fun` (kernel gap
    /// 2), or `None` when the cursor is not at one. `align` is a contextual
    /// word like `section`, so it only counts when a `(` touches it. `N` is a
    /// power of two up to 4096: an alignment above a page is not one a trap
    /// vector can use, and the bound keeps the LLVM and C attributes in the
    /// range both accept.
    fn parse_align_clause(&mut self) -> PResult<Option<u32>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "align")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'align'")?;
        let (n, n_span) = match self.peek_kind().clone() {
            TokenKind::IntLit(n) => {
                self.advance();
                (n, self.tokens[self.pos - 1].span)
            }
            other => {
                return Err(ParseError {
                    message: format!(
                        "expected an integer literal alignment in 'align(N)', found {other:?}"
                    ),
                    span: self.peek().span,
                })
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'align(...)'")?;
        if !n.is_power_of_two() || n > 4096 {
            return Err(ParseError {
                message: format!(
                    "'align({n})' needs a power of two up to 4096: 1, 2, 4, 8, ... 4096 are the alignments a function can have"
                ),
                span: n_span,
            });
        }
        Ok(Some(n as u32))
    }

    /// Whether the cursor is at a function declaration: `fun`, `extern fun` or
    /// `unsafe fun`. Used to decide whether a `section(...)` clause has an item
    /// it applies to.
    fn at_function_decl(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Fun)
            || (matches!(self.peek_kind(), TokenKind::Extern)
                && matches!(self.peek2_kind(), TokenKind::Fun))
            || (matches!(self.peek_kind(), TokenKind::Ident(n) if n == "unsafe" || n == "naked" || n == "must_use")
                && matches!(self.peek2_kind(), TokenKind::Fun))
    }

    /// `extern type Widget` or `extern type Window: Widget`: an opaque native
    /// handle (gap 1). `type` is not a lexer keyword, so the caller has already
    /// seen the two tokens and this reads the name and an optional parent.
    fn parse_extern_type_decl(&mut self, is_pub: bool) -> PResult<ExternTypeDecl> {
        let start = self.expect(TokenKind::Extern, "'extern'")?.span;
        self.advance();
        let (name, name_span) = self.expect_ident()?;
        let mut span = start.to(name_span);
        let parent = if self.check(&TokenKind::Colon) {
            self.advance();
            let (first, first_span) = self.expect_ident()?;
            let mut parent = first;
            let mut pend = first_span;
            while self.check(&TokenKind::Dot) {
                self.advance();
                let (part, part_span) = self.expect_ident()?;
                parent.push('.');
                parent.push_str(&part);
                pend = part_span;
            }
            span = span.to(pend);
            Some(parent)
        } else {
            None
        };
        Ok(ExternTypeDecl {
            name,
            parent,
            is_pub,
            module: String::new(),
            span,
        })
    }

    /// `<T, U>` after a declared type name (A-2). Returns an empty vector when
    /// no `<` was written, so this is a no-op for every ordinary declaration.
    /// A `<...>` parameter list, one `(name, bounds)` pair per parameter
    /// (A-2, with the bound half from A-12 slice 2): `fun f<T: A + B>` gives
    /// `("T", [A, B])`, and a parameter with no bound gets an empty list, so
    /// the two halves stay in step by construction.
    fn parse_type_params(&mut self) -> PResult<Vec<TypeParamParsed>> {
        if !self.check(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let open = self.advance().span;
        if self.check(&TokenKind::Gt) {
            return Err(ParseError {
                message: "a type parameter list needs at least one parameter ('<T>')".to_string(),
                span: open.to(self.peek().span),
            });
        }
        let mut params: Vec<TypeParamParsed> = Vec::new();
        loop {
            // `const N: usize` (A-20): a compile-time value parameter rather
            // than a type. `const` is a keyword, so the two forms never
            // collide.
            if self.check(&TokenKind::Const) {
                self.advance();
                let (param, pspan) = self.expect_ident()?;
                if params.iter().any(|p| p.name == param) {
                    return Err(ParseError {
                        message: format!("parameter '{param}' is declared more than once"),
                        span: pspan,
                    });
                }
                self.expect(TokenKind::Colon, "': then the integer type of the const parameter, as in 'const N: usize'")?;
                let (ty, _) = self.expect_ident()?;
                if self.check(&TokenKind::Lt) || self.check(&TokenKind::Colon) {
                    return Err(ParseError {
                        message: format!(
                            "a const parameter's type is one of 'usize', 'u8'..'u64', 'i32', 'i64', 'bool' or 'char', so '{ty}' cannot take arguments or bounds"
                        ),
                        span: self.peek().span,
                    });
                }
                params.push(TypeParamParsed {
                    name: param,
                    bounds: Vec::new(),
                    const_ty: Some(ty),
                    span: pspan,
                });
            } else {
                let (param, pspan) = self.expect_ident()?;
                if params.iter().any(|p| p.name == param) {
                    return Err(ParseError {
                        message: format!("type parameter '{param}' is declared more than once"),
                        span: pspan,
                    });
                }
                // `T: Name`: the traits the parameter must implement. A parameter
                // with no `:` has an empty list, which is every parameter written
                // before bounds existed.
                let bounds = if self.check(&TokenKind::Colon) {
                    self.advance();
                    self.parse_bounds()?
                } else {
                    Vec::new()
                };
                params.push(TypeParamParsed {
                    name: param,
                    bounds,
                    const_ty: None,
                    span: pspan,
                });
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(TokenKind::Gt, "'>' to close the type parameter list")?;
        Ok(params)
    }

    /// One or more trait names separated by `+` (A-12 slice 2): `Name`,
    /// `alias.Name`, `A + B`. A bound names a trait and nothing else, so no
    /// argument list and no `&` is accepted.
    fn parse_bounds(&mut self) -> PResult<Vec<TypeExpr>> {
        self.parse_bounds_at(None)
    }

    /// `parse_bounds` for a site that takes plain trait names only (a
    /// supertrait, an associated type's bound): an equality constraint there is
    /// refused where it is written, naming the site.
    fn parse_plain_bounds(&mut self, site: &str) -> PResult<Vec<TypeExpr>> {
        self.parse_bounds_at(Some(site))
    }

    /// One or more bounds separated by `+`. `Name<Arg, Assoc = Type>` (MISSING-
    /// Ve8Nc step 5) is a trait instance whose arguments are types and whose
    /// `Assoc = Type` items are equality constraints on its associated types;
    /// `site` refuses the constraints where a plain name is required.
    fn parse_bounds_at(&mut self, site: Option<&str>) -> PResult<Vec<TypeExpr>> {
        let mut out = Vec::new();
        loop {
            // A negative bound (`T: !Name`) is excluded from this design
            // (`plan/generic-trait-impls.md` 6): refused by name where it starts.
            if self.check(&TokenKind::Bang) {
                return Err(ParseError {
                    message: "a negative bound ('!Name') is excluded from generic trait impls (plan/generic-trait-impls.md section 6): a bound says a type implements a trait, and no bound says that it does not".to_string(),
                    span: self.peek().span,
                });
            }
            let (name, span) = self.parse_dotted_name()?;
            // `Name<Arg>` (A-12 slice 6): a bound on a trait instance. The
            // trait's own arguments are a comma-separated type list, exactly
            // as an impl line writes them.
            let mut bounds_span = span;
            let mut eqs: Vec<(String, TypeExpr)> = Vec::new();
            let args = if self.check(&TokenKind::Lt) {
                let open = self.advance().span;
                let mut args = Vec::new();
                loop {
                    if matches!(self.peek_kind(), TokenKind::Ident(_))
                        && matches!(self.peek2_kind(), TokenKind::Eq)
                    {
                        let (item, item_span) = self.expect_ident()?;
                        if let Some(site) = site {
                            return Err(ParseError {
                                message: format!(
                                    "an equality constraint ('{name}<{item} = ...>') is not in this slice on {site}: an equality constraint is written on a function's or an impl's type parameter ('fun f<I: {name}<{item} = i64>>')"
                                ),
                                span: item_span,
                            });
                        }
                        self.expect(TokenKind::Eq, "'=' after the associated type in an equality constraint")?;
                        let ty = self.parse_type()?;
                        eqs.push((item, ty));
                    } else {
                        args.push(self.parse_type()?);
                    }
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
                let close = self.expect(TokenKind::Gt, "'>' to close the trait argument list")?.span;
                bounds_span = open.to(close);
                args
            } else {
                Vec::new()
            };
            let mut bound = TypeExpr::named(name, bounds_span);
            bound.args = args;
            bound.bound_eq = eqs;
            bound.span = bounds_span;
            out.push(bound);
            if self.check(&TokenKind::Plus) {
                self.advance();
                continue;
            }
            break;
        }
        Ok(out)
    }

    /// The `where` clause after a generic declaration's header (MISSING-Ve8Nc
    /// step 5), shared by a function and an impl: `where T: A + B` bounds a
    /// parameter, and `where T::Assoc: A` bounds an associated type projected
    /// from one. `params` names the declaration's own type parameters; a name
    /// outside them is refused here, where the declaration is in hand.
    fn parse_where_clause(
        &mut self,
        params: &[String],
        bounds: &mut [Vec<TypeExpr>],
        projections: &mut Vec<ProjectionBound>,
        owner: &str,
    ) -> PResult<()> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "where") {
            return Ok(());
        }
        self.advance();
        loop {
            let (pname, pspan) = self.expect_ident()?;
            let Some(at) = params.iter().position(|p| *p == pname) else {
                return Err(ParseError {
                    message: format!(
                        "'where {pname}: ...' names no type parameter of this {owner}: a 'where' clause bounds a parameter written in the '<...>' list, so declare '{pname}' there first"
                    ),
                    span: pspan,
                });
            };
            if self.check(&TokenKind::ColonColon) {
                self.advance();
                let (assoc, _) = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':' after the associated type in a 'where' clause")?;
                let more = self.parse_bounds()?;
                for bound in more {
                    projections.push(ProjectionBound {
                        param: pname.clone(),
                        assoc: assoc.clone(),
                        bound,
                    });
                }
            } else {
                self.expect(TokenKind::Colon, "':' after the parameter in a 'where' clause")?;
                let more = self.parse_bounds()?;
                bounds[at].extend(more);
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        Ok(())
    }

    /// A `datac`/`enum` parameter list, with a bound refused (A-12 slice 2): a
    /// type declaration's parameters are bound at the use site (a function's
    /// `fun f<T: Name>`), not on the type itself. A `const` parameter keeps
    /// its integer type and is split out into `const_params` (A-20).
    fn no_type_param_bounds(
        params: Vec<TypeParamParsed>,
    ) -> PResult<(Vec<String>, Vec<ConstParamDecl>)> {
        if let Some(p) = params.iter().find(|p| !p.bounds.is_empty()) {
            return Err(ParseError {
                message: format!(
                    "a bound cannot be written on a type declaration's parameter '{}': a bound is written where the parameter is used ('fun f<T: Name>'), so a type declaration lists its parameters alone",
                    p.name
                ),
                span: p.bounds[0].span,
            });
        }
        let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let consts: Vec<ConstParamDecl> = params
            .into_iter()
            .filter_map(|p| {
                p.const_ty.map(|ty| ConstParamDecl {
                    name: p.name,
                    ty,
                    span: p.span,
                })
            })
            .collect();
        Ok((names, consts))
    }

    /// Whether the `impl` at the cursor opens an **import** (`impl "path"`,
    /// `plan/syntax.md` 13.1) rather than an `impl Trait for Type { ... }`
    /// block (A-12 slice 2): the two are told apart by the token after the
    /// keyword, a string literal for the import and an identifier for the
    /// block.
    fn at_module_impl(&self) -> bool {
        self.check(&TokenKind::Impl) && matches!(self.peek2_kind(), TokenKind::StringLit(_))
    }

    /// A dotted name: an identifier, then `.` and another identifier, repeated
    /// (`Shape`, `shape.Shape`). Used where only a name is legal (a bound, an
    /// `impl` line's trait, and its `for` type, A-12 slice 2), so a generic
    /// argument or a `&` is refused rather than silently accepted.
    fn parse_dotted_name(&mut self) -> PResult<(String, Span)> {
        let (first, start) = self.expect_ident()?;
        let mut name = first;
        let mut end = start;
        while self.check(&TokenKind::Dot) {
            self.advance();
            let (part, pspan) = self.expect_ident()?;
            name.push('.');
            name.push_str(&part);
            end = pspan;
        }
        Ok((name, start.to(end)))
    }

    /// The payload of a variant, or a `datac`'s field list, with the opening
    /// `(` already consumed and the closing `)` left for the caller. Shared
    /// so an `enum` variant and a `datac` field obey exactly the same
    /// `val`/`var name: Type` rules.
    fn parse_field_list(&mut self) -> PResult<Vec<FieldDecl>> {
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let mutable = if self.check(&TokenKind::Var) {
                self.advance();
                true
            } else {
                self.expect(TokenKind::Val, "'val' or 'var'")?;
                false
            };
            let (fname, fspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            fields.push(FieldDecl {
                name: fname,
                mutable,
                ty,
                span: fspan,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(fields)
    }

    /// `enum Shape { Circle(val r: f64) Rect(val w: f64, val h: f64) Empty }`
    /// — variants are newline-separated, and a variant's payload list is the
    /// same one a `datac` uses (`plan/syntax.md` §9.2).
    fn parse_enum_decl(&mut self, is_pub: bool, repr: Option<Repr>) -> PResult<EnumDecl> {
        let start = self.expect(TokenKind::Enum, "'enum'")?.span;
        let (name, _) = self.expect_ident()?;
        let (type_params, const_params) =
            Parser::no_type_param_bounds(self.parse_type_params()?)?;
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (vname, vspan) = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.check(&TokenKind::LParen) {
                self.advance();
                fields = self.parse_field_list()?;
                self.expect(TokenKind::RParen, "')'")?;
            }
            // `Off = 0`: an explicit discriminant on a field-less variant.
            // Whether the enum has any payload at all is checked in
            // `infer.rs`, where the whole declaration is in view.
            let discriminant = if self.check(&TokenKind::Eq) {
                self.advance();
                let negative = if self.check(&TokenKind::Minus) {
                    self.advance();
                    true
                } else {
                    false
                };
                match self.peek_kind().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        let value = i128::from(n);
                        Some(if negative { -value } else { value })
                    }
                    other => {
                        return Err(ParseError {
                            message: format!(
                                "expected an integer literal discriminant after '=', found {other:?}"
                            ),
                            span: self.peek().span,
                        })
                    }
                }
            } else {
                None
            };
            variants.push(VariantDecl {
                name: vname,
                fields,
                discriminant,
                span: vspan,
            });
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        message: format!(
                            "expected a newline or '}}' after an enum variant, found {:?}",
                            self.peek_kind()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        if variants.is_empty() {
            return Err(ParseError {
                message: "an 'enum' needs at least one variant".to_string(),
                span: start.to(end),
            });
        }
        Ok(EnumDecl {
            name,
            is_pub,
            type_params,
            const_params,
            variants,
            repr,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// One function declaration. `signature` is set only for a method written
    /// inside a `trait` body (A-12 slices 2 and 3): its receiver must be
    /// `Self`, and a body is a default method. Every other spelling of the
    /// declaration is shared, so a trait method, a default and an `impl`
    /// method are parsed by the same code and cannot drift apart.
    fn parse_function(&mut self, is_pub: bool, unsafe_: bool, naked: bool, section: Option<String>, align: Option<u32>, signature: bool) -> PResult<FunctionDecl> {
        let outer = std::mem::take(&mut self.loop_stack);
        let r = self.parse_function_inner(is_pub, unsafe_, naked, section, align, signature);
        self.loop_stack = outer;
        r
    }

    fn parse_function_inner(&mut self, is_pub: bool, unsafe_: bool, naked: bool, section: Option<String>, align: Option<u32>, signature: bool) -> PResult<FunctionDecl> {
        // `extern fun ...` — a declaration whose body the compiler supplies
        // (M6.2). It has no body at all, and cannot be a receiver method
        // (there is nothing to give it a body with).
        let extern_ = if self.check(&TokenKind::Extern) {
            self.advance();
            true
        } else {
            false
        };
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;

        // A receiver-form decl is `fun [mut] Type.name(...)`; a plain one
        // is `fun name(...)`. `mut` here can only ever start a receiver
        // (free functions never write `fun mut name(...)`), so seeing it
        // commits to that form. `move` never belongs here: a method that
        // took its receiver by value would destroy the caller's value at a
        // call that looks like a read (`c.total()`), so the spelling is
        // refused with the parameter form named instead (A-4).
        if self.check(&TokenKind::Move) {
            return Err(ParseError {
                message: "'move' cannot be written on a receiver — a method always borrows its receiver; a move-taking parameter is written 'name: move T'".to_string(),
                span: self.peek().span,
            });
        }
        let receiver_mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (first, first_span) = self.expect_ident()?;
        // `<T>` may follow either the function's own name (`fun f<T>(...)`) or
        // the receiver's type name (`fun Pair<A, B>.first()`); which one it is
        // is settled by whether a `.` comes next (A-2).
        let name_params = self.parse_type_params()?;
        let (receiver, name, type_params, type_param_bounds, const_params) =
            if self.check(&TokenKind::Dot) {
            self.advance();
            let (method, method_span) = self.expect_ident()?;
            // `fun Pair<A, B>.map_first<C>(...)`: the method's own type
            // parameters, after its name. They are separate from the
            // receiver's (which are the type's) and inferred at a call.
            let own_params = self.parse_type_params()?;
            if let Some(p) = own_params.iter().find(|p| p.const_ty.is_some()) {
                return Err(ParseError {
                    message: format!("a method cannot take the const parameter '{}' of its own yet: only type parameters", p.name),
                    span: method_span,
                });
            }
            // A receiver's parameters are its type's, so there is nowhere for a
            // bound to go (A-12 slice 2): the type's own parameters are bound
            // by whoever declares the generic type.
            if let Some(p) = name_params.iter().find(|p| !p.bounds.is_empty()) {
                return Err(ParseError {
                    message: format!(
                        "a bound cannot be written on a receiver's type parameter '{}': those parameters come from the receiver's type, so bind them there, or write the bound on a function's own parameter list ('fun f<T: Name>')",
                        p.name
                    ),
                    span: first_span,
                });
            }
            let type_args: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
            let const_params: Vec<ConstParamDecl> = name_params
                .into_iter()
                .filter_map(|p| {
                    p.const_ty.map(|ty| ConstParamDecl {
                        name: p.name,
                        ty,
                        span: p.span,
                    })
                })
                .collect();
            (
                Some(Receiver {
                    type_name: first,
                    type_args,
                    const_params,
                    mutable: receiver_mutable,
                    span: first_span,
                }),
                method,
                own_params.iter().map(|p| p.name.clone()).collect(),
                own_params.iter().map(|p| p.bounds.clone()).collect(),
                Vec::new(),
            )
        } else {
            if receiver_mutable {
                return Err(ParseError {
                    message: "'mut' before a function name is only valid on a receiver ('fun mut Type.method(...)')".to_string(),
                    span: first_span,
                });
            }
            let type_params: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
            let bounds: Vec<Vec<TypeExpr>> =
                name_params.iter().map(|p| p.bounds.clone()).collect();
            let const_params: Vec<ConstParamDecl> = name_params
                .into_iter()
                .filter_map(|p| {
                    p.const_ty.map(|ty| ConstParamDecl {
                        name: p.name,
                        ty,
                        span: p.span,
                    })
                })
                .collect();
            (None, first, type_params, bounds, const_params)
        };
        // A trait's method signatures are the trait's interface: they take no
        // type parameters of their own, and their receiver is `Self` (A-12
        // slice 2). Both are refused here, where the receiver and the
        // parameter list are already in hand.
        if signature {
            if extern_ {
                return Err(ParseError {
                    message: "a trait method cannot be 'extern': a trait declares the method an impl or the default body provides".to_string(),
                    span: start,
                });
            }
            match &receiver {
                Some(r) if r.type_name == "Self" => {
                    if !type_params.is_empty() {
                        return Err(ParseError {
                            message: "a trait method cannot take type parameters: the signature names 'Self' and the trait's own types".to_string(),
                            span: first_span,
                        });
                    }
                }
                Some(r) => {
                    return Err(ParseError {
                        message: format!(
                            "a trait method's receiver is written 'Self', not '{}': 'Self' stands for the implementing type ('fun Self.{name}(...)')",
                            r.type_name
                        ),
                        span: first_span,
                    })
                }
                // A receiverless trait method is a **static** method (A-12
                // slice 6): `fun from(value: T).return: Self`. It may not take type
                // parameters of its own (a generic method stays refused), which
                // keeps it exactly as object-unsafe-free as a receiver method.
                None => {
                    if !type_params.is_empty() {
                        return Err(ParseError {
                            message: "a trait method cannot take type parameters: the signature names 'Self', the trait's own types and no others"
                                .to_string(),
                            span: first_span,
                        });
                    }
                }
            }
        }

        // A method is reached through a value of its type, never through a
        // module alias, so `pub` on one has nothing to govern: the type's own
        // `pub` is what exports the method with it (A-13).
        if is_pub && receiver.is_some() {
            return Err(ParseError {
                message: "'pub' cannot be written on a method: a method's reachability follows its declaring type, so mark the type 'pub' instead".to_string(),
                span: start,
            });
        }

        let (params, variadic) = self.parse_params()?;
        // A-20: `impl Trait` in argument position is sugar for a fresh bounded
        // type parameter, one per `impl` occurrence. The rewrite happens here,
        // at the declaration, so every later pass sees an ordinary generic
        // function. A receiver method takes no type parameters of its own, and
        // a trait signature declares none either, so both refuse it.
        let (params, type_params, mut type_param_bounds) = desugar_impl_trait_params(
            params,
            type_params,
            type_param_bounds,
            receiver.is_some(),
            signature,
            first_span,
        )?;

        if extern_ && receiver.is_some() {
            return Err(ParseError {
                message: "'extern' cannot be used on a receiver method — declare it as a free function"
                    .to_string(),
                span: start,
            });
        }
        if extern_ && section.is_some() {
            return Err(ParseError {
                message: "'section(\"...\")' cannot be written on an 'extern fun': the declaration has no code of its own to place"
                    .to_string(),
                span: start,
            });
        }
        if extern_ && align.is_some() {
            return Err(ParseError {
                message: "'align(N)' cannot be written on an 'extern fun': the declaration has no code of its own to align"
                    .to_string(),
                span: start,
            });
        }

        let return_type = if self.check(&TokenKind::Dot) {
            self.advance();
            self.expect_return_marker()?;
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            if let Some(span) = impl_trait_anywhere(&ty) {
                return Err(ParseError {
                    message: "'impl Trait' is not allowed in return position: write a named type parameter and bound it ('fun f<T: Trait>(...).return: T') when the caller needs a concrete type, or return the owning value ('Box<dyn Trait>')".to_string(),
                    span,
                });
            }
            Some(ty)
        } else {
            None
        };

        // `where T: Name` is the other spelling of a bound (A-12 slice 2), and
        // `where T::Item: Name` bounds an associated type (MISSING-Ve8Nc step 5).
        let mut projection_bounds: Vec<ProjectionBound> = Vec::new();
        self.parse_where_clause(&type_params, &mut type_param_bounds, &mut projection_bounds, "function")?;

        let body = if signature {
            // A trait method with no body is a signature; one written with `=`
            // or `{ ... }` is a default method (A-12 slice 3).
            if self.check(&TokenKind::Eq) {
                let eq_span = self.advance().span;
                if return_type.is_none() {
                    return Err(ParseError {
                        message: "expression-bodied function requires a '.return: T' return type"
                            .to_string(),
                        span: eq_span,
                    });
                }
                FunctionBody::Expr(self.parse_expr()?)
            } else if self.check(&TokenKind::LBrace) {
                FunctionBody::Block(self.parse_block()?)
            } else {
                FunctionBody::Signature
            }
        } else if extern_ {
            // No body and no `=`: the C is supplied by the compiler. A
            // written body would be dead code, so say so rather than ignore
            // it.
            if self.check(&TokenKind::Eq) || self.check(&TokenKind::LBrace) {
                return Err(ParseError {
                    message: "an 'extern fun' declaration has no body — the compiler supplies it"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            FunctionBody::Extern
        } else if self.check(&TokenKind::Eq) {
            let eq_span = self.advance().span;
            if return_type.is_none() {
                return Err(ParseError {
                    message: "expression-bodied function requires a '.return: T' return type"
                        .to_string(),
                    span: eq_span,
                });
            }
            FunctionBody::Expr(self.parse_expr()?)
        } else if self.check(&TokenKind::LBrace) {
            FunctionBody::Block(self.parse_block()?)
        } else {
            return Err(ParseError {
                message: format!("expected '=' or '{{', found {:?}", self.peek_kind()),
                span: self.peek().span,
            });
        };

        let end = match &body {
            FunctionBody::Block(b) => b.span,
            FunctionBody::Expr(e) => e.span(),
            // A declaration with no body ends where its signature does: at
            // the return type if one was written, otherwise at the last
            // parameter's type.
            FunctionBody::Extern | FunctionBody::Signature => return_type
                .as_ref()
                .map(|t| t.span)
                .or_else(|| params.last().map(|p| p.ty.span))
                .unwrap_or(start),
        };
        Ok(FunctionDecl {
            name,
            is_pub,
            type_params,
            type_param_bounds,
            projection_bounds,
            receiver_bounds: Vec::new(),
            const_params,
            trait_impl: None,
            extern_,
            unsafe_,
            must_use: false,
            variadic,
            naked,
            receiver,
            params,
            return_type,
            body,
            module: String::new(),
            body_module: None,
            section,
            align,
            span: start.to(end),
        })
    }

    fn parse_type(&mut self) -> PResult<TypeExpr> {
        self.nest()?;
        let r = self.parse_type_inner();
        self.depth -= 1;
        r
    }

    fn parse_type_inner(&mut self) -> PResult<TypeExpr> {
        // A trait object is `&dyn Name` (A-12 slice 4) or `Box<dyn Name>`
        // (slice 7). `dyn` is not a keyword, so a bare `dyn Name` is recognized
        // here, where two names in a row can only mean this, and *marked* on
        // the `TypeExpr`: `infer.rs` accepts the mark only as the argument of a
        // `Box` and refuses a bare `dyn Name` everywhere else, which is what
        // makes the message name the position. `impl Name` in a type position
        // is the other spelling of the bound, refused for the same reason.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "dyn")
            && matches!(self.peek2_kind(), TokenKind::Ident(_))
        {
            let start = self.advance().span;
            let (first, first_span) = self.expect_ident()?;
            let mut name = first;
            let mut span = start.to(first_span);
            while self.check(&TokenKind::Dot) {
                self.advance();
                let (part, part_span) = self.expect_ident()?;
                name.push('.');
                name.push_str(&part);
                span = span.to(part_span);
            }
            // `Box<dyn A + B>` is refused where the `+` is: a vtable names one
            // trait, and the trait that combines two is a declared trait with
            // `A` and `B` as supertraits.
            if self.check(&TokenKind::Plus) {
                return Err(ParseError {
                    message: format!(
                        "`dyn` names one trait: write `dyn {name}`, and make a trait that combines them (`trait Both: A + B {{ ... }}`) when both sets of methods are wanted"
                    ),
                    span: self.peek().span,
                });
            }
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: None,
                assoc: None,
                array_len: None,
                amp: false,
                amp_mut: false,
                dyn_trait: Some(name),
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span,
            });
        }
        // A leading `&` is recorded but not applied here: in a parameter
        // position `&str`/`&T` is exactly what the bare name already means
        // (a borrowed view passed by value), while in a *return* position it
        // is the elided-borrow form `infer.rs` turns into a `Ty::Ref`
        // (M6.3). Only the resolver knows which position it is looking at.
        let amp_start = if self.check(&TokenKind::Amp) {
            Some(self.advance().span)
        } else {
            None
        };
        // `&dyn Name` / `&mut dyn Name`: the one place `&mut` may be written in
        // a type position (A-12 slice 4). Everywhere else an exclusive borrow
        // is `mut T` on a parameter, and `&mut T` keeps its refusal.
        if let Some(amp) = amp_start {
            let amp_mut = if self.check(&TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "dyn") {
                self.advance();
                let (first, first_span) = self.expect_ident()?;
                let mut name = first;
                let mut span = amp.to(first_span);
                while self.check(&TokenKind::Dot) {
                    self.advance();
                    let (part, part_span) = self.expect_ident()?;
                    name.push('.');
                    name.push_str(&part);
                    span = span.to(part_span);
                }
                return Ok(TypeExpr {
                    name: String::new(),
                    tuple: None,
                    args: Vec::new(),
                    array_elem: None,
                    assoc: None,
                    array_len: None,
                    amp: true,
                    amp_mut,
                    dyn_trait: Some(name),
                    scoped: false,
                    fn_params: None,
                    fn_ret: None,
                    const_expr: None,
                    impl_trait: None,
                    bound_eq: Vec::new(),
                    span,
                });
            }
            if amp_mut {
                return Err(ParseError {
                    message: "'&mut' is only written before 'dyn' in a type: a parameter writes 'mut T' for an exclusive borrow".to_string(),
                    span: self.peek().span,
                });
            }
        }
        // `impl Name` in a parameter position (A-20): the sugar for a fresh
        // bounded type parameter. The parser records it and the declaration
        // desugars it, so it never reaches type resolution from a `fun`; any
        // other position (`let`, a field, a return) is refused in `infer.rs`.
        // A leading `&` is kept, since `&impl Name` is the borrowed spelling
        // of the same parameter.
        if self.check(&TokenKind::Impl) {
            let start = self.advance().span;
            let (trait_name, tspan) = self.parse_dotted_name()?;
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    message: format!(
                        "'impl {trait_name}' cannot take the trait's own arguments here: write a named type parameter bounded by the trait ('fun f<T: {trait_name}<Arg>>(x: T)')"
                    ),
                    span: self.peek().span,
                });
            }
            let span = amp_start.map(|a| a.to(tspan)).unwrap_or_else(|| start.to(tspan));
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: None,
                assoc: None,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: Some(trait_name),
                bound_eq: Vec::new(),
                span,
            });
        }
        // A function type (A-4): `fun(i32, String).return: bool`. `fun` is a
        // keyword, so this can never be an ordinary type name. A closure type
        // (A-4 phase E) is the same shape under the `closure` keyword:
        // `closure(i32).return: i32` is a function value that owns an environment.
        // A fixed-size array type (A-18): `[T; N]`. Its length is a
        // compile-time constant expression (A-19), kept as an expression here
        // and folded by `infer.rs` when the type is resolved. So
        // `[Entry; PAGE_SIZE / ENTRY_SIZE]` is one type, and the `;` between
        // the two halves is the token that says so.
        if self.check(&TokenKind::LBracket) {
            let open = self.advance().span;
            let elem = self.parse_type()?;
            self.expect(TokenKind::Semi, "';' between an array's element type and its length")?;
            let len = self.parse_expr()?;
            let close = self.expect(TokenKind::RBracket, "']' to close the array type")?.span;
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: Some(Box::new(elem)),
                assoc: None,
                array_len: Some(Box::new(len)),
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span: amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close)),
            });
        }
        // A tuple type (MISSING-z2bB9): `(A, B)`, two or more element types.
        // A tuple is a record with positional fields (`plan/syntax.md` 3), so
        // this is a *type* like any other. `(T)` is not a tuple: it is the
        // parenthesized type `T`, and `()` is refused by name because the
        // absent value's type is spelled `unit`.
        if self.check(&TokenKind::LParen) {
            let open = self.advance().span;
            if self.check(&TokenKind::RParen) {
                let close = self.advance().span;
                return Err(ParseError {
                    message: "'()' is not a type: a tuple has two or more element types ('(i32, i32)'), and the absent value's type is 'unit'"
                        .to_string(),
                    span: amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close)),
                });
            }
            let first = self.parse_type()?;
            if !self.check(&TokenKind::Comma) {
                let close = self.expect(TokenKind::RParen, "')' to close the tuple type")?.span;
                let mut inner = first;
                if let Some(amp) = amp_start {
                    inner.amp = true;
                    inner.span = amp.to(close);
                }
                return Ok(inner);
            }
            let mut elems = vec![first];
            while self.check(&TokenKind::Comma) {
                self.advance();
                elems.push(self.parse_type()?);
            }
            let close = self.expect(TokenKind::RParen, "')' to close the tuple type")?.span;
            let span = amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close));
            return Ok(TypeExpr {
                name: String::new(),
                tuple: Some(elems),
                args: Vec::new(),
                array_elem: None,
                assoc: None,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span,
            });
        }
        if self.check(&TokenKind::Fun) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "fun", false);
        }
        // `scoped fun(T).return: U` — a *contextual* modifier. `scoped` is read as
        // one only when the next token is `fun`, so an ordinary type or
        // binding named `scoped` is untouched, and the modifier never has a
        // keyword that could break an existing program.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "scoped")
            && matches!(self.peek2_kind(), TokenKind::Fun)
        {
            let start = self.advance().span;
            self.advance();
            return self.parse_fn_type(start, amp_start, "fun", true);
        }
        if self.check(&TokenKind::Closure) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "closure", false);
        }
        let (first, first_span) = self.expect_ident()?;
        // A dotted name is a module-qualified reference to an imported item
        // (`circle.Circle`) — see `resolve.rs`, which rewrites it to the
        // item's canonical, module-prefixed name.
        let mut name = first;
        let mut span = first_span;
        while self.check(&TokenKind::Dot) {
            self.advance();
            let (part, part_span) = self.expect_ident()?;
            name.push('.');
            name.push_str(&part);
            span = span.to(part_span);
        }
        // `I::Item` / `Self::Item` (A-12 slice 5): an associated type
        // projected from the name just read. A generic associated type is
        // refused here, where the `::` says this is the shape.
        let assoc = if self.check(&TokenKind::ColonColon) {
            self.advance();
            let (item, ispan) = self.expect_ident()?;
            span = span.to(ispan);
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    message: "a generic associated type is not in this slice: 'type Item' declares a type, not a parameterized one, so write the type the impl binds without an argument list"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            Some(item)
        } else {
            None
        };
        // Type arguments (collections milestone): `List<T>`. Unambiguous in
        // a type position, and v1 knows exactly one generic name, so a
        // comma-separated list is parsed here and rejected in `infer.rs`
        // with a message naming which type does not take arguments.
        if self.check(&TokenKind::Lt) {
            let (args, close) = self.parse_type_arg_list()?;
            span = span.to(close);
            return Ok(TypeExpr {
                name,
                tuple: None,
                args,
                array_elem: None,
                assoc,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span: amp_start.map(|a| a.to(span)).unwrap_or(span),
            });
        }
        Ok(TypeExpr {
            name,
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc,
            array_len: None,
            amp: amp_start.is_some(),
            amp_mut: false,
            dyn_trait: None,
            scoped: false,
            fn_params: None,
            fn_ret: None,
            const_expr: None,
            impl_trait: None,
            bound_eq: Vec::new(),
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    /// `( name: T, ... )` — one parameter list, shared by a declaration and a
    /// `fun` literal (A-4), since the two take exactly the same parameters.
    /// The flag is `true` for a declaration whose last parameter is the `...`
    /// variadic marker (gap 7); a `fun` literal refuses one at its own call
    /// site, because only a user `extern fun` can be variadic.
    fn parse_params(&mut self) -> PResult<(Vec<Param>, bool)> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        let mut variadic = false;
        while !self.check(&TokenKind::RParen) {
            if self.check(&TokenKind::DotDotDot) {
                self.advance();
                if !self.check(&TokenKind::RParen) {
                    return Err(ParseError {
                        message: "'...' must be the last parameter of an 'extern fun': nothing may follow it".to_string(),
                        span: self.peek().span,
                    });
                }
                variadic = true;
                break;
            }
            let (pname, _) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            // `mut T` requests an exclusive borrow and `move T` takes
            // ownership (A-4); this is the *only* spelling for either in
            // parameter position — `&mut T` is rejected here (the leading
            // `&` in `parse_type` expects an identifier next, not the `mut`
            // keyword token), and `&`/`mut` stay expression-only.
            let mode = self.parse_param_mode()?;
            let ty = self.parse_type()?;
            params.push(Param {
                name: pname,
                ty,
                mode,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok((params, variadic))
    }

    /// `fun(x: i32).return: i32 = x * 2` in expression position — an anonymous
    /// function (A-4). A body is required (there is no `extern` spelling for
    /// a literal), and everything else about the header is a declaration's:
    /// the same parameter list, the same optional `.return: T`.
    fn parse_fn_literal(&mut self) -> PResult<Expr> {
        let outer = std::mem::take(&mut self.loop_stack);
        let r = self.parse_fn_literal_inner();
        self.loop_stack = outer;
        r
    }

    fn parse_fn_literal_inner(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;
        let (params, variadic) = self.parse_params()?;
        if variadic {
            return Err(ParseError {
                message: "a 'fun' literal cannot be variadic: '...' is only legal on a user 'extern fun'"
                    .to_string(),
                span: start,
            });
        }
        let return_type = if self.check(&TokenKind::Dot) {
            self.advance();
            self.expect_return_marker()?;
            self.expect(TokenKind::Colon, "':'")?;
            Some(self.parse_type()?)
        } else {
            None
        };
        let (body, end) = if self.check(&TokenKind::Eq) {
            self.advance();
            let e = self.parse_expr()?;
            let end = e.span();
            (FunctionBody::Expr(e), end)
        } else if self.check(&TokenKind::LBrace) {
            let b = self.parse_block()?;
            let end = b.span;
            (FunctionBody::Block(b), end)
        } else {
            return Err(ParseError {
                message: "a 'fun' literal needs a body: '= expr' or '{ ... }'".to_string(),
                span: self.peek().span,
            });
        };
        Ok(Expr::FnLit {
            params,
            infer_params: false,
            return_type,
            body: Box::new(body),
            span: start.to(end),
        })
    }

    /// The `mut`/`move` qualifier before a parameter's type, in either
    /// parameter position (`name: mut T` in a declaration, `fun(mut T)` in a
    /// function type). Reading at most one is what makes the two mutually
    /// exclusive: `mut` says "I write through your value", `move` says "I own
    /// it", and one parameter cannot say both.
    fn parse_param_mode(&mut self) -> PResult<ParamMode> {
        let mode = if self.check(&TokenKind::Mut) {
            self.advance();
            ParamMode::Mut
        } else if self.check(&TokenKind::Move) {
            self.advance();
            ParamMode::Move
        } else {
            return Ok(ParamMode::Borrow);
        };
        if self.check(&TokenKind::Mut) || self.check(&TokenKind::Move) {
            return Err(ParseError {
                message: "'mut' and 'move' cannot be combined on one parameter — 'mut T' is an exclusive borrow, 'move T' takes ownership".to_string(),
                span: self.peek().span,
            });
        }
        Ok(mode)
    }

    /// The rest of `fun(i32, String).return: bool`, with `fun` already consumed.
    /// A parameter of a function type takes the same `mut`/`move` qualifiers
    /// a declaration's parameter takes, because the two have to agree: a
    /// written `fun(String)` is a function whose parameter is a borrow, and
    /// `fun(move String)` is one that takes ownership, exactly as
    /// `fun f(s: String)` and `fun f(s: move String)` do.
    fn parse_fn_type(
        &mut self,
        start: Span,
        amp_start: Option<Span>,
        name: &str,
        scoped: bool,
    ) -> PResult<TypeExpr> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let mode = self.parse_param_mode()?;
            let ty = self.parse_type()?;
            params.push(FnTypeParam { mode, ty });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let close = self.expect(TokenKind::RParen, "')'")?;
        let mut span = start.to(close.span);
        let ret = if self.check(&TokenKind::Dot) {
            self.advance();
            self.expect_return_marker()?;
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            span = span.to(ty.span);
            Some(Box::new(ty))
        } else {
            None
        };
        Ok(TypeExpr {
            name: name.to_string(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc: None,
            array_len: None,
            amp: amp_start.is_some(),
            amp_mut: false,
            dyn_trait: None,
            scoped,
            fn_params: Some(params),
            fn_ret: ret,
            const_expr: None,
            impl_trait: None,
            bound_eq: Vec::new(),
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    /// `"<" type ("," type)* ">"`, with the `<` not yet consumed. Returns the
    /// arguments and the closing `>`'s span, which the expression-level
    /// speculative reader needs in order to check adjacency.
    fn parse_type_arg_list(&mut self) -> PResult<(Vec<TypeExpr>, Span)> {
        self.expect(TokenKind::Lt, "'<'")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_type_arg()?);
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = self.expect(TokenKind::Gt, "'>' to close the type argument list")?;
        Ok((args, close.span))
    }

    /// One argument of a `<...>` list. A type argument is either a type or,
    /// for a `const` parameter, a compile-time expression written the same way
    /// (`Buf<16>`, `Buf<SIZE * 2>`, A-20). The two are told apart by lookahead
    /// here and settled for good where the declaration's parameter list says
    /// which one it takes.
    fn parse_type_arg(&mut self) -> PResult<TypeExpr> {
        self.nest()?;
        let r = self.parse_type_arg_inner();
        self.depth -= 1;
        r
    }

    fn parse_type_arg_inner(&mut self) -> PResult<TypeExpr> {
        if self.starts_const_arg() {
            let start = self.peek().span;
            let e = self.parse_const_arg_expr()?;
            let end = e.span();
            return Ok(TypeExpr::const_arg(e, start.to(end)));
        }
        // A bare or dotted name is ambiguous: it may be a type argument or a
        // named `const` written as a const argument (`Buf<SIZE>`). It is kept
        // as a type here and reconstructed as an expression by
        // `named_const_expr` where a `const` parameter is the one being bound.
        self.parse_type()
    }

    /// Whether the tokens at the cursor begin a const argument rather than a
    /// type: an integer, boolean or character literal, a leading `-`, or a name that is
    /// called or used in an arithmetic expression (`SIZE * 2`, `size()`).
    fn starts_const_arg(&self) -> bool {
        match self.peek_kind() {
            TokenKind::IntLit(_)
            | TokenKind::CharLit(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Minus => true,
            TokenKind::Ident(_) => {
                let mut i = self.pos + 1;
                while matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Dot))
                    && matches!(
                        self.tokens.get(i + 1).map(|t| &t.kind),
                        Some(TokenKind::Ident(_))
                    )
                {
                    i += 2;
                }
                matches!(
                    self.tokens.get(i).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                        | Some(TokenKind::Plus)
                        | Some(TokenKind::Minus)
                        | Some(TokenKind::Star)
                        | Some(TokenKind::Slash)
                        | Some(TokenKind::Percent)
                        | Some(TokenKind::Amp)
                        | Some(TokenKind::Pipe)
                        | Some(TokenKind::Caret)
                )
            }
            _ => false,
        }
    }

    /// `<T, U>` in an *expression*, immediately followed by `(` — the explicit
    /// instantiation of a generic construction or call, `Pair<i32, String>(...)`
    /// or `Option.Some<i32>(v = 1)` (A-2).
    ///
    /// This is the one place the spelling is ambiguous: `a < b > (c)` is a
    /// valid comparison chain, and `a < b, c > (d)` is not (a comma cannot
    /// appear in an expression there), so a lookahead that simply tries is not
    /// safe. The rule is adjacency — the `<` must touch the name and the `(`
    /// must touch the `>` — so comparison code, which is written with spaces,
    /// can never be misread. Returns `None` and restores the token position
    /// when the form does not apply.
    fn try_explicit_type_args(&mut self) -> PResult<Option<Vec<TypeExpr>>> {
        if !self.check(&TokenKind::Lt) {
            return Ok(None);
        }
        let prev_end = if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        };
        if self.peek().span.start != prev_end {
            return Ok(None);
        }
        let save = self.pos;
        if let Ok((args, close)) = self.parse_type_arg_list() {
            if self.check(&TokenKind::LParen) && self.peek().span.start == close.end {
                return Ok(Some(args));
            }
        }
        self.pos = save;
        Ok(None)
    }

    fn parse_block(&mut self) -> PResult<Block> {
        let start = self.expect(TokenKind::LBrace, "'{'")?.span;
        self.parse_block_body(start)
    }

    /// The statements and closing `}` of a block whose `{` (and, for a block
    /// with parameters, whose `{ x, y ->` header) the caller has consumed.
    fn parse_block_body(&mut self, start: Span) -> PResult<Block> {
        self.nest()?;
        let r = self.parse_block_body_inner(start);
        self.depth -= 1;
        r
    }

    fn parse_block_body_inner(&mut self, start: Span) -> PResult<Block> {
        let saved = std::mem::replace(&mut self.no_trailing_block, false);
        self.skip_newlines();
        let stmts = self.parse_stmts_to_rbrace()?;
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        self.no_trailing_block = saved;
        Ok(Block {
            stmts,
            span: start.to(end),
        })
    }

    /// The statements of a block up to, not including, its closing `}`.
    ///
    /// A `let PATTERN = EXPR else { BLOCK }` takes every statement after it
    /// into its pattern arm, so the rest of the block is parsed here, by a
    /// recursive call, and the statement becomes a `when` whose pattern arm
    /// is that rest and whose `else` arm is the else block (`plan/syntax.md`
    /// 4.1). The `when` is the last statement of the block, so its value is
    /// the rest's value, and every existing rule for `when` arms, drops and
    /// divergence applies to the let-else unchanged.
    fn parse_stmts_to_rbrace(&mut self) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            if self.at_let_else() {
                let (pattern, init, else_block, span) = self.parse_let_else_head()?;
                if !self.check(&TokenKind::RBrace) {
                    if !self.check(&TokenKind::Newline) {
                        return Err(ParseError {
                            message: format!(
                                "expected a newline or '}}' after a statement, found {:?}",
                                self.peek_kind()
                            ),
                            span: self.peek().span,
                        });
                    }
                    self.skip_newlines();
                }
                let rest = self.parse_stmts_to_rbrace()?;
                let else_span = else_block.span;
                stmts.push(Stmt::Expr {
                    expr: Expr::When {
                        scrutinee: Box::new(init),
                        arms: vec![
                            WhenArm {
                                pattern: Some(pattern),
                                body: Expr::Block(Block { stmts: rest, span }),
                                span,
                            },
                            WhenArm {
                                pattern: None,
                                body: Expr::Block(else_block),
                                span: else_span,
                            },
                        ],
                        span,
                        let_else: true,
                    },
                    span,
                });
                return Ok(stmts);
            }
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        message: format!(
                            "expected a newline or '}}' after a statement, found {:?}",
                            self.peek_kind()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Val | TokenKind::Var => self.parse_let_stmt(),
            TokenKind::While | TokenKind::For => self.parse_loop_construct(None),
            // `loop { ... }`: an endless loop, `while true` under another name.
            // Contextual, like `break`: `loop` followed by `{` at the start of
            // a statement.
            TokenKind::Ident(name) if name == "loop" && matches!(self.peek2_kind(), TokenKind::LBrace) => {
                self.parse_loop_construct(None)
            }
            // `'outer: while ...` / `'outer: for ...` / `'outer: loop { ... }`.
            TokenKind::Label(name) => {
                let label = name.clone();
                self.advance();
                self.expect(TokenKind::Colon, "':' after the loop label")?;
                let loop_ahead = matches!(self.peek_kind(), TokenKind::While | TokenKind::For)
                    || matches!(self.peek_kind(), TokenKind::Ident(n) if n == "loop");
                if !loop_ahead {
                    return Err(ParseError {
                        message: format!("the label '{label} has to be followed by a 'while', 'for' or 'loop'"),
                        span: self.peek().span,
                    });
                }
                self.parse_loop_construct(Some(label))
            }
            TokenKind::Defer => self.parse_defer_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            // `break` and `continue` are read as ordinary names everywhere
            // else, so a bare one here (the statement ends after it) is the
            // loop control statement (`plan/iterators.md`); `break + 1` and
            // `break()` stay expressions.
            TokenKind::Ident(name)
                if (name == "break" || name == "continue")
                    && (self.statement_ends_here()
                        || matches!(self.peek2_kind(), TokenKind::Label(_))
                        || (name == "break" && self.break_value_at(1))) =>
            {
                let is_break = name == "break";
                let span = self.advance().span;
                let label = if let TokenKind::Label(label) = self.peek_kind().clone() {
                    let lspan = self.advance().span;
                    Some((label, span.to(lspan)))
                } else {
                    None
                };
                // `break value`: the value of the `loop` it leaves.
                let value = if is_break && self.break_value_at(0) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                if let Some((label, lspan)) = label {
                    return self.parse_labelled_exit(is_break, &label, lspan, value);
                }
                self.parse_exit(is_break, span, value)
            }
            // `pub` leads a declaration, and a declaration is not a statement,
            // so the message names the forms it can lead (A-13) rather than
            // reading the keyword as a start-of-expression.
            TokenKind::Pub => Err(ParseError {
                message: "'pub' applies to a top-level 'fun', 'extern fun', 'datac', 'enum', 'const' or 'static', and only at the top level of a file".to_string(),
                span: self.peek().span,
            }),
            _ => {
                let expr = self.parse_expr()?;
                let span = expr.span();
                Ok(Stmt::Expr { expr, span })
            }
        }
    }

    fn parse_let_stmt(&mut self) -> PResult<Stmt> {
        let mutable = self.check(&TokenKind::Var);
        let start = if mutable {
            self.expect(TokenKind::Var, "'var'")?.span
        } else {
            self.expect(TokenKind::Val, "'val'")?.span
        };
        // `val (lo, hi) = min_max(xs)`: a destructuring binding takes one
        // tuple value apart (`plan/syntax.md` 4). The pattern is a comma-
        // separated list in parentheses, so the one thing that tells it from
        // the ordinary `val name = ...` is the `(` right after the keyword.
        if self.check(&TokenKind::LParen) {
            let pattern = self.parse_binding_pattern()?;
            if self.check(&TokenKind::Colon) {
                return Err(ParseError {
                    message: "a destructuring binding takes no written type: every position's type comes from the initializer, in order; bind the whole value first ('val p = ...') and write the type there"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            self.expect(TokenKind::Eq, "'='")?;
            let init = self.parse_expr()?;
            let span = start.to(init.span());
            return Ok(Stmt::LetTuple {
                pattern,
                mutable,
                init,
                span,
            });
        }
        let (name, _) = self.expect_ident()?;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(Stmt::Let {
            name,
            mutable,
            ty,
            init,
            span,
        })
    }

    /// `(lo, hi)` / `((a, b), c)`: the pattern a destructuring binding takes
    /// its value apart with (`plan/syntax.md` 4). A position is a name, `_` to
    /// ignore it, or a nested pattern, and there are two or more of them:
    /// `(x)` is not a pattern but the parenthesized name `x`, exactly as `(T)`
    /// is not a tuple type.
    fn parse_binding_pattern(&mut self) -> PResult<BindingPattern> {
        self.nest()?;
        let r = self.parse_binding_pattern_inner();
        self.depth -= 1;
        r
    }

    fn parse_binding_pattern_inner(&mut self) -> PResult<BindingPattern> {
        let open = self
            .expect(TokenKind::LParen, "'(' to open a destructuring pattern")?
            .span;
        let mut positions = Vec::new();
        loop {
            if self.check(&TokenKind::LParen) {
                positions.push(self.parse_binding_pattern()?);
            } else {
                let (name, _) = self.expect_ident()?;
                positions.push(if name == "_" {
                    BindingPattern::Ignore
                } else {
                    BindingPattern::Name(name)
                });
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = self
            .expect(TokenKind::RParen, "')' to close a destructuring pattern")?
            .span;
        if positions.len() < 2 {
            return Err(ParseError {
                message: "a destructuring pattern needs two or more positions ('val (lo, hi) = ...'); a single name is an ordinary 'val', and '_' ignores a position rather than binding one"
                    .to_string(),
                span: open.to(close),
            });
        }
        Ok(BindingPattern::Tuple(positions))
    }

    /// `loop { ... }`: `while true { ... }` (the loop control statements and the
    /// missing-return analysis already treat that as an endless loop).
    fn parse_loop_stmt(&mut self) -> PResult<Stmt> {
        let start = self.advance().span;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While {
            cond: Expr::BoolLit(true, start),
            body,
            iterating: None,
            span,
        })
    }

    /// One loop (`while`, `for` or `loop`), optionally labelled, with the
    /// bookkeeping a labelled exit needs. A `break 'outer` from inside nested
    /// loops sets a hidden flag and breaks the innermost loop; each loop
    /// between it and the target then re-checks the flag right after it ends
    /// and breaks in turn, and the target either breaks or continues. Every
    /// step is an ordinary `break`, `continue`, `if` and `var`, so borrow
    /// checking, drops and both backends see nothing new.
    fn parse_loop_construct(&mut self, label: Option<String>) -> PResult<Stmt> {
        self.parse_loop_construct_with(label, None).map(|(stmt, _)| stmt)
    }

    /// `parse_loop_construct`, optionally for a `loop` used as a value, whose
    /// `break value`s push to `value_slot`. Also returns how many valued
    /// breaks the loop had.
    fn parse_loop_construct_with(
        &mut self,
        label: Option<String>,
        value_slot: Option<String>,
    ) -> PResult<(Stmt, usize)> {
        self.loop_stack.push(LoopCtx { label, flag: None, exits: Vec::new(), value_slot, valued_breaks: 0 });
        let parsed = match self.peek_kind() {
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            _ => self.parse_loop_stmt(),
        };
        let ctx = self.loop_stack.pop().expect("the loop pushed above");
        let stmt = parsed?;
        let valued = ctx.valued_breaks;
        if ctx.flag.is_none() && ctx.exits.is_empty() {
            return Ok((stmt, valued));
        }
        let span = match &stmt {
            Stmt::While { span, .. } | Stmt::For { span, .. } => *span,
            _ => unreachable!("a loop construct is a while or a for"),
        };
        let mut stmts: Vec<Stmt> = Vec::new();
        if let Some(flag) = &ctx.flag {
            stmts.push(Stmt::Let {
                name: flag.clone(),
                mutable: true,
                ty: None,
                init: Expr::IntLit(0, span),
                span,
            });
        }
        stmts.push(stmt);
        let parent = self.loop_stack.len();
        let ident = |name: &str| Expr::Ident(name.to_string(), span);
        let test = |name: &str, op: BinOp, n: u64| Expr::Binary {
            op,
            lhs: Box::new(Expr::Ident(name.to_string(), span)),
            rhs: Box::new(Expr::IntLit(n, span)),
            span,
        };
        let when = |cond: Expr, body: Vec<Stmt>| Stmt::Expr {
            expr: Expr::If {
                cond: Box::new(cond),
                then_block: Block { stmts: body, span },
                else_block: None,
                span,
            },
            span,
        };
        for target in ctx.exits {
            let flag = self.loop_stack[target].flag.clone().expect("a target with an exit has a flag");
            if target + 1 == parent {
                // The target is the loop this one sits directly in.
                stmts.push(when(test(&flag, BinOp::Eq, 1), vec![Stmt::Break { span }]));
                stmts.push(when(
                    test(&flag, BinOp::Eq, 2),
                    vec![
                        Stmt::Expr {
                            expr: Expr::Assign {
                                target: Box::new(ident(&flag)),
                                value: Box::new(Expr::IntLit(0, span)),
                                span,
                            },
                            span,
                        },
                        Stmt::Continue { span },
                    ],
                ));
            } else {
                stmts.push(when(test(&flag, BinOp::NotEq, 0), vec![Stmt::Break { span }]));
            }
        }
        Ok((Stmt::Expr { expr: Expr::Block(Block { stmts, span }), span }, valued))
    }

    /// `loop { ... break value ... }` used as a value: `val x = loop { ... }`.
    /// The value travels in a hidden one-element list the loop's `break value`s
    /// push to (a bare `List.empty()` takes its element type from the first
    /// push), and the loop's value is what is popped out of it afterwards, so
    /// nothing below the parser knows about it.
    fn parse_loop_value(&mut self, label: Option<String>) -> PResult<Expr> {
        let start = self.peek().span;
        self.label_flags += 1;
        let slot = format!("#loopres{}", self.label_flags - 1);
        let (stmt, valued) = self.parse_loop_construct_with(label, Some(slot.clone()))?;
        let span = start.to(match &stmt {
            Stmt::Expr { span, .. } | Stmt::While { span, .. } | Stmt::For { span, .. } => *span,
            _ => start,
        });
        if valued == 0 {
            return Err(ParseError {
                message: "a 'loop' used as a value needs a 'break value' to produce one".to_string(),
                span,
            });
        }
        let ident = |name: &str| Expr::Ident(name.to_string(), span);
        let call = |receiver: &str, method: &str, args: Vec<CallArg>| Expr::MethodCall {
            receiver: Box::new(ident(receiver)),
            method: method.to_string(),
            type_args: Vec::new(),
            args,
            span,
        };
        Ok(Expr::Block(Block {
            stmts: vec![
                Stmt::Let {
                    name: slot.clone(),
                    mutable: true,
                    ty: None,
                    init: call("List", "empty", Vec::new()),
                    span,
                },
                stmt,
                Stmt::Expr { expr: call(&slot, "pop", Vec::new()), span },
            ],
            span,
        }))
    }

    /// `break 'outer` / `continue 'outer`. The innermost loop is a plain
    /// `break`/`continue`; an outer one sets the target's flag (1 for break,
    /// 2 for continue) and breaks, and `parse_loop_construct` adds the
    /// re-checks after each loop in between.
    fn parse_labelled_exit(&mut self, is_break: bool, label: &str, span: Span, value: Option<Expr>) -> PResult<Stmt> {
        let Some(target) = self.loop_stack.iter().rposition(|l| l.label.as_deref() == Some(label)) else {
            return Err(ParseError {
                message: format!("no enclosing loop is labelled '{label}; a label is written before a loop ('{label}: for ...') and does not cross a function or a block passed to a call"),
                span,
            });
        };
        let top = self.loop_stack.len() - 1;
        let push = self.valued_exit(target, is_break, span, value)?;
        if target == top {
            return Ok(Self::with_push(push, if is_break { Stmt::Break { span } } else { Stmt::Continue { span } }, span));
        }
        let flag = match self.loop_stack[target].flag.clone() {
            Some(f) => f,
            None => {
                self.label_flags += 1;
                let f = format!("{}{}", LABEL_FLAG_PREFIX, self.label_flags - 1);
                self.loop_stack[target].flag = Some(f.clone());
                f
            }
        };
        for between in (target + 1)..=top {
            if !self.loop_stack[between].exits.contains(&target) {
                self.loop_stack[between].exits.push(target);
            }
        }
        let mut stmts = Vec::new();
        stmts.extend(push);
        stmts.push(Stmt::Expr {
            expr: Expr::Assign {
                target: Box::new(Expr::Ident(flag, span)),
                value: Box::new(Expr::IntLit(if is_break { 1 } else { 2 }, span)),
                span,
            },
            span,
        });
        stmts.push(Stmt::Break { span });
        Ok(Stmt::Expr { expr: Expr::Block(Block { stmts, span }), span })
    }

    /// An unlabelled `break`/`continue` (with `break value`'s value, if any).
    fn parse_exit(&mut self, is_break: bool, span: Span, value: Option<Expr>) -> PResult<Stmt> {
        let Some(target) = self.loop_stack.len().checked_sub(1) else {
            // No loop encloses it: the type checker says so, with its own wording.
            return Ok(if is_break { Stmt::Break { span } } else { Stmt::Continue { span } });
        };
        let push = self.valued_exit(target, is_break, span, value)?;
        Ok(Self::with_push(push, if is_break { Stmt::Break { span } } else { Stmt::Continue { span } }, span))
    }

    /// The `slot.push(value)` of a `break value` out of loop `target`, after
    /// checking that the loop is one that produces a value and the exit is
    /// one that gives it: `Ok(None)` for an exit that carries no value.
    fn valued_exit(&mut self, target: usize, is_break: bool, span: Span, value: Option<Expr>) -> PResult<Option<Stmt>> {
        let slot = self.loop_stack[target].value_slot.clone();
        match (slot, value) {
            (Some(slot), Some(v)) => {
                self.loop_stack[target].valued_breaks += 1;
                Ok(Some(Stmt::Expr {
                    expr: Expr::MethodCall {
                        receiver: Box::new(Expr::Ident(slot, span)),
                        method: "push".to_string(),
                        type_args: Vec::new(),
                        args: vec![CallArg { name: None, value: v }],
                        span,
                    },
                    span,
                }))
            }
            (None, Some(_)) => Err(ParseError {
                message: "only a 'loop' used as a value can 'break' with one: write 'val x = loop { ... break value ... }'".to_string(),
                span,
            }),
            (Some(_), None) if is_break => Err(ParseError {
                message: "this 'loop' produces a value, so every 'break' out of it carries one: 'break value'".to_string(),
                span,
            }),
            _ => Ok(None),
        }
    }

    /// Whether the token `offset` past the current one starts a `break` value:
    /// see `starts_break_value`, plus a negative number (`break -1`).
    fn break_value_at(&self, offset: usize) -> bool {
        let kind = |i: usize| self.tokens.get(self.pos + i).map(|t| &t.kind);
        match kind(offset) {
            Some(TokenKind::Minus) => {
                matches!(kind(offset + 1), Some(TokenKind::IntLit(_)) | Some(TokenKind::FloatLit(_)))
            }
            Some(k) => starts_break_value(k),
            None => false,
        }
    }

    /// `push` (if any) followed by the exit itself, as one statement.
    fn with_push(push: Option<Stmt>, exit: Stmt, span: Span) -> Stmt {
        match push {
            None => exit,
            Some(p) => Stmt::Expr { expr: Expr::Block(Block { stmts: vec![p, exit], span }), span },
        }
    }

    fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::While, "'while'")?.span;
        let cond = self.parse_head_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While { cond, body, iterating: None, span })
    }

    /// `for x in xs { BODY }`, desugared here into an indexed walk over the
    /// `List` — the same trick the boolean `when` and `if let` already use, so
    /// nothing downstream learns a new form (A-5, `plan/syntax.md` §6.2):
    ///
    /// ```text
    /// {
    ///     var #for0_i: usize = 0
    ///     while #for0_i < SEQ.len() {
    ///         { val x = SEQ.get(#for0_i)  BODY }
    ///         #for0_i = #for0_i + 1
    ///     }
    /// }
    /// ```
    ///
    /// `SEQ` is the iterable as written when it is a place, and a synthesized
    /// `val #for0 = EXPR` otherwise (so a call is evaluated once and freed at
    /// the end of the block). Binding a place by value would move the list, and
    /// a parameter *is* a borrow — `for x in xs` inside a function would move
    /// out of a borrow, which is the common case.
    ///
    /// The element binding is exactly what `List.get` gives: a copy for a
    /// `Copy` element, a shared borrow for an owned one. It lives in a block
    /// of its own so the borrow ends before the increment, and so `BODY`'s own
    /// tail expression stays the tail of a block.
    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::For, "'for'")?.span;
        // `for (k, v) in xs`: the element is bound to a hidden name and the
        // body starts with the ordinary destructuring `val (k, v) = <hidden>`
        // (`plan/syntax.md` 4), so a pattern costs no pass anything.
        let pattern = if self.check(&TokenKind::LParen) {
            Some((self.peek().span, self.parse_binding_pattern()?))
        } else {
            None
        };
        let (binding, binding_span) = match &pattern {
            Some((span, _)) => {
                self.for_patterns += 1;
                (format!("{}{}", FOR_PATTERN_PREFIX, self.for_patterns - 1), *span)
            }
            None => self.expect_ident()?,
        };
        // `in` is an ordinary identifier (2.3): it is the separator only in
        // this position, the way `let` is a pattern only after `if` (6.1).
        let (word, word_span) = self.expect_ident()?;
        if word != "in" {
            return Err(ParseError {
                message: format!(
                    "expected 'in' between the 'for' binding and what it iterates, found '{word}'"
                ),
                span: word_span,
            });
        }
        let iterable = self.parse_head_expr()?;
        let mut body = self.parse_block()?;
        let span = start.to(body.span);
        if let Some((pspan, pattern)) = pattern {
            body.stmts.insert(
                0,
                Stmt::LetTuple {
                    pattern,
                    mutable: false,
                    init: Expr::Ident(binding.clone(), pspan),
                    span: pspan,
                },
            );
        }
        // The type of the iterable decides the desugar (`plan/iterators.md`),
        // and only `infer.rs` knows it, so the statement is kept as written.
        Ok(Stmt::For {
            binding,
            binding_span,
            iterable,
            body,
            span,
        })
    }

    /// Whether the token after the current one ends the statement: a newline,
    /// the block's `}`, or the end of the file. Used to tell a bare `break` or
    /// `continue` statement from a name used in an expression.
    fn statement_ends_here(&self) -> bool {
        matches!(
            self.peek2_kind(),
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof
        )
    }

    fn parse_defer_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Defer, "'defer'")?.span;
        let expr = self.parse_head_expr()?;
        let span = start.to(expr.span());
        Ok(Stmt::Defer { expr, span })
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Return, "'return'")?.span;
        let value = if self.at_stmt_end() {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let span = match &value {
            Some(v) => start.to(v.span()),
            None => start,
        };
        Ok(Stmt::Return { value, span })
    }

    // ---- expressions (Pratt parsing) ----

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.nest()?;
        let r = self.parse_expr_inner();
        self.depth -= 1;
        r
    }

    fn parse_expr_inner(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> PResult<Expr> {
        let target = self.parse_binary(0)?;

        let compound_op = match self.peek_kind() {
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance();
            let rhs = self.parse_assignment()?;
            let span = target.span().to(rhs.span());
            // Desugar `target OP= rhs` to `target = target OP rhs`.
            let combined = Expr::Binary {
                op,
                lhs: Box::new(target.clone()),
                rhs: Box::new(rhs),
                span,
            };
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(combined),
                span,
            });
        }

        if self.check(&TokenKind::Eq) {
            self.advance();
            let value = self.parse_assignment()?;
            let span = target.span().to(value.span());
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(value),
                span,
            });
        }
        Ok(target)
    }

    fn binding_power(op: &TokenKind) -> Option<(u8, BinOp)> {
        use TokenKind::*;
        Some(match op {
            // C's precedence *except* for the bitwise operators, which sit
            // above the comparisons rather than below them. This is Rust's
            // reading, and it is deliberate: in C, `flags & MASK == 0` means
            // `flags & (MASK == 0)`, which is a bug that has been written a
            // million times. Here it means `(flags & MASK) == 0`. `|` and `^`
            // keep their relative order (`|` loosest, then `^`, then `&`), so
            // `a | b & c` is still `a | (b & c)`.
            OrOr => (1, BinOp::Or),
            AndAnd => (2, BinOp::And),
            EqEq => (3, BinOp::Eq),
            NotEq => (3, BinOp::NotEq),
            Lt => (4, BinOp::Lt),
            LtEq => (4, BinOp::LtEq),
            Gt => (4, BinOp::Gt),
            GtEq => (4, BinOp::GtEq),
            Pipe => (5, BinOp::BitOr),
            Caret => (6, BinOp::BitXor),
            // `&` in *infix* position is bitwise-and. A prefix `&` never
            // reaches here: `parse_unary` consumes it first.
            Amp => (7, BinOp::BitAnd),
            // `a << b` and `a >> b` arrive as two adjacent `<`/`>` tokens
            // (`parse_binary` folds them before consulting this table), so
            // the shift operators are not listed here.
            Plus => (9, BinOp::Add),
            Minus => (9, BinOp::Sub),
            Star => (10, BinOp::Mul),
            Slash => (10, BinOp::Div),
            Percent => (10, BinOp::Mod),
            _ => return None,
        })
    }

    /// `<<` and `>>`, recognized as two *adjacent* `<`/`>` tokens (A-9).
    ///
    /// The lexer deliberately does not produce a single `Shl`/`Shr` token:
    /// `List<List<i32>>` closes a nested type-argument list with two `>`
    /// characters, and a token that swallowed both would leave the type
    /// parser with nothing to close the inner list with. Adjacency is what
    /// separates `a << b` from a comparison followed by another one, and the
    /// two spellings being visually distinct is why the shift reads as one
    /// operator anyway.
    fn shift_op(&self) -> Option<BinOp> {
        let first = &self.tokens[self.pos];
        let second = self.tokens.get(self.pos + 1)?;
        if first.span.end != second.span.start {
            return None;
        }
        match (&first.kind, &second.kind) {
            (TokenKind::Lt, TokenKind::Lt) => Some(BinOp::Shl),
            (TokenKind::Gt, TokenKind::Gt) => Some(BinOp::Shr),
            _ => None,
        }
    }

    fn parse_binary(&mut self, min_bp: u8) -> PResult<Expr> {
        self.parse_binary_filtered(min_bp, true)
    }

    /// A binary expression that stops before every comparison operator. Used
    /// for a const argument (`Buf<SIZE * 2>`, A-20), where `<` and `>` are the
    /// delimiters of the argument list rather than operators.
    fn parse_const_arg_expr(&mut self) -> PResult<Expr> {
        self.parse_binary_filtered(0, false)
    }

    fn parse_binary_filtered(&mut self, min_bp: u8, comparisons: bool) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        let mut chain = 0usize;
        loop {
            // Shifts first: they are two tokens wide, so the table above
            // cannot describe them.
            let (bp, op, width) = if let Some(op) = self.shift_op() {
                (8u8, op, 2usize)
            } else {
                match Self::binding_power(self.peek_kind()) {
                    Some((bp, op)) => (bp, op, 1),
                    None => break,
                }
            };
            if !comparisons
                && matches!(
                    op,
                    BinOp::Lt
                        | BinOp::LtEq
                        | BinOp::Gt
                        | BinOp::GtEq
                        | BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Or
                        | BinOp::And
                )
            {
                break;
            }
            if bp < min_bp {
                break;
            }
            chain += 1;
            self.chain_limit(chain)?;
            for _ in 0..width {
                self.advance();
            }
            let rhs = self.parse_binary_filtered(bp + 1, comparisons)?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        self.nest()?;
        let r = self.parse_unary_inner();
        self.depth -= 1;
        r
    }

    fn parse_unary_inner(&mut self) -> PResult<Expr> {
        match self.peek_kind() {
            TokenKind::Minus => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Bang => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Tilde => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Amp => {
                let start = self.advance().span;
                let mutable = if self.check(&TokenKind::Mut) {
                    self.advance();
                    true
                } else {
                    false
                };
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Ref {
                    mutable,
                    expr: Box::new(expr),
                    span,
                })
            }
            // A leading `*` at the start of a unary-parse position is
            // unambiguously prefix-deref: multiplication's `Star` is only
            // ever consumed by `parse_binary`'s own operator loop, which
            // runs *after* this function returns, never by `parse_unary`
            // itself.
            TokenKind::Star => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Deref {
                    expr: Box::new(expr),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        let mut chain = 0usize;
        loop {
            chain += 1;
            self.chain_limit(chain)?;
            // `expr?` — the propagation operator (MK5-S6-M1). It is postfix,
            // so it binds tighter than any binary operator and chains with the
            // accesses below (`f()?.x`).
            if self.check(&TokenKind::Question) {
                let q = self.advance().span;
                let start = expr.span();
                expr = Expr::Try {
                    expr: Box::new(expr),
                    span: start.to(q),
                };
                continue;
            }
            if self.check(&TokenKind::Dot) {
                self.advance();
                // `t.0`: a tuple position, the field a derived record names
                // `f0` (`plan/syntax.md` 4).
                if let TokenKind::IntLit(n) = self.peek_kind().clone() {
                    let nspan = self.advance().span;
                    let span = expr.span().to(nspan);
                    expr = Expr::FieldAccess {
                        base: Box::new(expr),
                        field: crate::types::tuple_field_name(n as usize),
                        span,
                    };
                    continue;
                }
                let (name, nspan) = self.expect_ident()?;
                // `Option.Some<i32>(v = 1)` — an explicit instantiation on a
                // generic `enum`'s variant (A-2). Recognized only in the
                // adjacent form, so `a.b < c > (d)` stays a comparison.
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen)
                    || (!self.no_trailing_block && self.check(&TokenKind::LBrace))
                {
                    let (mut args, mut end) = if self.check(&TokenKind::LParen) {
                        self.parse_call_args()?
                    } else {
                        (Vec::new(), nspan)
                    };
                    self.parse_trailing_block(&mut args, &mut end)?;
                    let span = expr.span().to(end);
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: name,
                        type_args,
                        args,
                        span,
                    };
                } else {
                    let span = expr.span().to(nspan);
                    expr = Expr::FieldAccess {
                        base: Box::new(expr),
                        field: name,
                        span,
                    };
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Parses a parenthesized, comma-separated argument list (each
    /// optionally named, `f(x = 1)`), starting at the `(`. Shared between
    /// plain calls (`f(...)`) and method calls (`recv.m(...)`).
    fn parse_call_args(&mut self) -> PResult<(Vec<CallArg>, Span)> {
        self.expect(TokenKind::LParen, "'('")?;
        let saved = std::mem::replace(&mut self.no_trailing_block, false);
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) {
            // A named argument is exactly `ident =` (not `==`); parens
            // already make newlines insignificant here.
            let name = if matches!(self.peek_kind(), TokenKind::Ident(_)) && matches!(self.peek2_kind(), TokenKind::Eq) {
                let (n, _) = self.expect_ident()?;
                self.advance(); // '='
                Some(n)
            } else {
                None
            };
            let value = self.parse_expr()?;
            args.push(CallArg { name, value });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        self.no_trailing_block = saved;
        Ok((args, end))
    }

    /// `asm("<template>", operands...)` (kernel stage 5). The template's
    /// placeholders are `{0}`, `{1}`, ... in operand order; each operand is
    /// `in("<constraint>") expr`, `out("<constraint>") place` (or `_` for the
    /// anonymous output that is the expression's value), `inout(...) place`,
    /// or `clobber("<register>")`, and the option words are `volatile`,
    /// `nomem` and `nostack`. `plan/inline-asm.md` is the design.
    fn parse_asm_expr(&mut self) -> PResult<Expr> {
        let start = self.advance().span;
        self.expect(TokenKind::LParen, "'(' after 'asm'")?;
        let (template, _) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                self.advance();
                (s, self.tokens[self.pos - 1].span)
            }
            other => {
                return Err(ParseError {
                    message: format!(
                        "expected a quoted instruction template in 'asm(\"...\", ...)', found {other:?}"
                    ),
                    span: self.peek().span,
                })
            }
        };
        let saved = std::mem::replace(&mut self.no_trailing_block, true);
        let mut operands: Vec<AsmOperand> = Vec::new();
        let mut clobbers: Vec<String> = Vec::new();
        let mut nomem = false;
        let mut nostack = false;
        while self.check(&TokenKind::Comma) {
            self.advance();
            let (word, word_span) = match self.peek_kind().clone() {
                TokenKind::Ident(n) => {
                    self.advance();
                    (n, self.tokens[self.pos - 1].span)
                }
                other => {
                    return Err(ParseError {
                        message: format!(
                            "expected an asm operand ('in', 'out', 'inout', 'clobber') or an option word ('volatile', 'nomem', 'nostack'), found {other:?}"
                        ),
                        span: self.peek().span,
                    })
                }
            };
            match word.as_str() {
                "volatile" | "nomem" | "nostack"
                    if !matches!(self.peek_kind(), TokenKind::LParen) =>
                {
                    match word.as_str() {
                        "volatile" => {}
                        "nomem" => nomem = true,
                        _ => nostack = true,
                    }
                }
                "clobber" if matches!(self.peek_kind(), TokenKind::LParen) => {
                    let (name, _) = self.asm_constraint("clobber")?;
                    if name.is_empty() {
                        return Err(ParseError {
                            message: "'clobber(\"\")' needs a register name, for example 'clobber(\"memory\")' or 'clobber(\"rax\")'"
                                .to_string(),
                            span: word_span,
                        });
                    }
                    clobbers.push(name);
                }
                "in" | "out" | "inout" if matches!(self.peek_kind(), TokenKind::LParen) => {
                    let kind = match word.as_str() {
                        "in" => AsmOperandKind::In,
                        "out" => AsmOperandKind::Out,
                        _ => AsmOperandKind::InOut,
                    };
                    let (constraint, _) = self.asm_constraint(&word)?;
                    let operand = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "_") {
                        self.advance();
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    if operand.is_none() && kind != AsmOperandKind::Out {
                        return Err(ParseError {
                            message: format!(
                                "'{}' needs a place: only 'out' may write an anonymous output ('out(\"...\") _')",
                                word
                            ),
                            span: word_span,
                        });
                    }
                    let end = operand
                        .as_ref()
                        .map(|e| e.span())
                        .unwrap_or(word_span);
                    operands.push(AsmOperand {
                        kind,
                        constraint,
                        operand,
                        span: word_span.to(end),
                    });
                }
                other => {
                    return Err(ParseError {
                        message: format!(
                            "unknown asm operand kind '{other}': expected 'in', 'out', 'inout' or 'clobber'"
                        ),
                        span: word_span,
                    })
                }
            }
        }
        self.no_trailing_block = saved;
        let end = self.expect(TokenKind::RParen, "')' to close 'asm(...)'")?.span;
        Ok(Expr::Asm {
            template,
            operands,
            clobbers,
            nomem,
            nostack,
            span: start.to(end),
        })
    }

    /// `("<constraint>")` after an asm operand kind: the LLVM constraint or
    /// clobber name, passed to the backend unchanged. The compiler checks that
    /// it is a plain word, never that the register exists.
    fn asm_constraint(&mut self, kind: &str) -> PResult<(String, Span)> {
        self.expect(TokenKind::LParen, "'(' after the operand kind")?;
        let (name, span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    message: format!(
                        "expected a quoted constraint in '{kind}(\"...\")', found {other:?}"
                    ),
                    span: self.peek().span,
                })
            }
        };
        if name.is_empty() {
            return Err(ParseError {
                message: format!(
                    "'{kind}(\"\")' needs a constraint name, for example 'in(\"r\")' or 'out(\"rax\")'"
                ),
                span,
            });
        }
        if name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '{' | '}' | '~' | ',' | '$'))
        {
            return Err(ParseError {
                message: format!(
                    "'{name}' is not a constraint name: a constraint is a register class or register name, with no spaces, '{{', '}}', '~', ',' or '$'"
                ),
                span,
            });
        }
        self.expect(TokenKind::RParen, "')' to close the constraint")?;
        Ok((name, span))
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::IntLit(v, tok.span))
            }
            TokenKind::CharLit(v) => {
                self.advance();
                Ok(Expr::CharLit(v, tok.span))
            }
            TokenKind::ByteLit(v) => {
                self.advance();
                Ok(Expr::ByteLit(v, tok.span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr::FloatLit(v, tok.span))
            }
            TokenKind::StringLit(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s, tok.span))
            }
            TokenKind::TemplateStart => self.parse_template(),
            // `loop { ... break value ... }` as a value (contextual: `loop`
            // followed by `{`), optionally labelled (`'a: loop { ... }`).
            TokenKind::Ident(ref name) if name == "loop" && matches!(self.peek2_kind(), TokenKind::LBrace) => {
                self.parse_loop_value(None)
            }
            TokenKind::Label(ref label)
                if matches!(self.peek2_kind(), TokenKind::Colon)
                    && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(TokenKind::Ident(n)) if n == "loop") =>
            {
                let label = label.clone();
                self.advance();
                self.advance();
                self.parse_loop_value(Some(label))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true, tok.span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false, tok.span))
            }
            TokenKind::Ident(ref name) => {
                let name = name.clone();
                // `asm("...", ...)` (kernel stage 5): `asm` is a contextual
                // word, so it only counts when a `(` touches it, exactly as
                // `unsafe` counts only before `{`.
                if name == "asm" && matches!(self.peek2_kind(), TokenKind::LParen) {
                    return self.parse_asm_expr();
                }
                // `unsafe { ... }` (MK9-S2 stage 1): the contextual word is a
                // block introducer exactly when the next token is `{`, so
                // `unsafe` stays usable as an ordinary name everywhere else.
                if name == "unsafe" && matches!(self.peek2_kind(), TokenKind::LBrace) {
                    self.advance();
                    let block = self.parse_block()?;
                    let span = tok.span.to(block.span);
                    return Ok(Expr::Unsafe { block, span });
                }
                self.advance();
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen) {
                    let (mut args, mut end) = self.parse_call_args()?;
                    self.parse_trailing_block(&mut args, &mut end)?;
                    Ok(Expr::Call {
                        callee: name,
                        type_args,
                        args,
                        span: tok.span.to(end),
                    })
                } else if !self.no_trailing_block && self.check(&TokenKind::LBrace) {
                    // `name { ... }`: a call whose only argument is the block.
                    let (mut args, mut end) = (Vec::new(), tok.span);
                    self.parse_trailing_block(&mut args, &mut end)?;
                    Ok(Expr::Call {
                        callee: name,
                        type_args,
                        args,
                        span: tok.span.to(end),
                    })
                } else {
                    Ok(Expr::Ident(name, tok.span))
                }
            }
            TokenKind::LParen => {
                // A tuple literal `(a, b)` is the parenthesized form with a
                // comma in it, so the first element decides which one this is:
                // `(e)` stays the parenthesized expression it always was, and
                // a tuple has two or more elements (`plan/syntax.md` 3).
                self.advance();
                if self.check(&TokenKind::RParen) {
                    // `()` is the absent value, of type `unit` (`plan/syntax.md`
                    // 3, 6.4). It is an empty block: a block with no tail
                    // evaluates to `unit`, so `Ok(value = ())` builds a
                    // `Result<unit, E>` whose payload occupies no storage.
                    let close = self.advance().span;
                    return Ok(Expr::Block(Block {
                        stmts: Vec::new(),
                        span: tok.span.to(close),
                    }));
                }
                let expr = self.parse_expr()?;
                if !self.check(&TokenKind::Comma) {
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(expr);
                }
                let mut elems = vec![expr];
                while self.check(&TokenKind::Comma) {
                    self.advance();
                    elems.push(self.parse_expr()?);
                }
                let end = self.expect(TokenKind::RParen, "')' to close the tuple")?.span;
                Ok(Expr::TupleLit {
                    elems,
                    span: tok.span.to(end),
                })
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }
            TokenKind::LBracket => self.parse_array_literal(tok.span),
            TokenKind::Fun => self.parse_fn_literal(),
            TokenKind::If => self.parse_if(),
            TokenKind::When => self.parse_when(),
            other => Err(ParseError {
                message: format!("unexpected token {other:?}"),
                span: tok.span,
            }),
        }
    }

    /// `[a, b, c]` and `[x; N]`: the two array literals (A-18). The list form
    /// fixes the length by how many elements were written; the repeat form
    /// takes it from a compile-time constant, which is what makes a zeroed
    /// buffer (`[0; 4096]`) one expression rather than 4096 of them.
    ///
    /// `[]` is refused here rather than later: with no elements there is no
    /// element type to infer and no length, so there is nothing a later pass
    /// could do with it.
    fn parse_array_literal(&mut self, start: Span) -> PResult<Expr> {
        self.expect(TokenKind::LBracket, "'['")?;
        if self.check(&TokenKind::RBracket) {
            return Err(ParseError {
                message: "an array literal needs at least one element ('[a, b, c]') or a repeat ('[x; N]')"
                    .to_string(),
                span: start.to(self.peek().span),
            });
        }
        let first = self.parse_expr()?;
        if self.check(&TokenKind::Semi) {
            self.advance();
            let len = self.parse_expr()?;
            let end = self.expect(TokenKind::RBracket, "']' to close the array literal")?.span;
            return Ok(Expr::ArrayRepeat {
                value: Box::new(first),
                len: Box::new(len),
                span: start.to(end),
            });
        }
        let mut elems = vec![first];
        while self.check(&TokenKind::Comma) {
            self.advance();
            if self.check(&TokenKind::RBracket) {
                break;
            }
            elems.push(self.parse_expr()?);
        }
        let end = self.expect(TokenKind::RBracket, "']' to close the array literal")?.span;
        Ok(Expr::ArrayLit {
            elems,
            span: start.to(end),
        })
    }

    /// Parses a full string template, starting at its `TemplateStart` (the
    /// lexer has already split the literal into chunk/interpolation tokens).
    fn parse_template(&mut self) -> PResult<Expr> {
        let start = self
            .expect(TokenKind::TemplateStart, "start of a string template")?
            .span;
        let mut parts = Vec::new();
        loop {
            match self.peek_kind().clone() {
                TokenKind::StrChunk(text) => {
                    self.advance();
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text(text));
                    }
                }
                TokenKind::TemplateExprStart => {
                    self.advance();
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::TemplateExprEnd, "'} ' ending the '${...}' interpolation")?;
                    parts.push(TemplatePart::Expr(value));
                }
                TokenKind::TemplateEnd => {
                    let end = self.advance().span;
                    return Ok(Expr::Template {
                        parts,
                        span: start.to(end),
                    });
                }
                other => {
                    return Err(ParseError {
                        message: format!(
                            "malformed string template: expected literal text or '${{expr}}', found {other:?}"
                        ),
                        span: self.peek().span,
                    })
                }
            }
        }
    }

    fn parse_if(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::If, "'if'")?.span;
        self.parse_if_link(start)
    }

    /// `elif cond { ... }`: the `else if` link written as one keyword
    /// (`plan/syntax.md` 6.1). The token is consumed and the rest of the link
    /// goes through `parse_if_link`, exactly as `else if` does, so the two
    /// spellings cannot mean two different things.
    fn parse_elif(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Elif, "'elif'")?.span;
        self.parse_if_link(start)
    }

    /// One link of an `if` chain: the optional `let` pattern, the condition,
    /// the block, and whatever follows (nothing, `else`, `else if`, or
    /// `elif`). Shared by `if` and `elif`, which is why a chain may mix them.
    fn parse_if_link(&mut self, start: Span) -> PResult<Expr> {
        // `if let PATTERN = EXPR { ... }` — a pattern condition rather than a
        // boolean one. `let` is only an identifier everywhere else in the
        // language, so this is recognized by the token pair rather than by
        // reserving the word; the one thing it costs is that a binding
        // *named* `let` cannot be the condition of an `if` without
        // parentheses.
        if matches!(self.peek_kind(), TokenKind::Ident(word) if word == "let") {
            return self.parse_if_let(start);
        }
        let cond = self.parse_head_expr()?;
        let then_block = self.parse_block()?;
        let mut span = start.to(then_block.span);
        let else_block = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                let elif = self.parse_if()?;
                span = span.to(elif.span());
                Some(Box::new(ElseBranch::If(elif)))
            } else {
                let block = self.parse_block()?;
                span = span.to(block.span);
                Some(Box::new(ElseBranch::Block(block)))
            }
        } else if self.check(&TokenKind::Elif) {
            let link = self.parse_elif()?;
            span = span.to(link.span());
            Some(Box::new(ElseBranch::If(link)))
        } else {
            None
        };
        Ok(Expr::If {
            cond: Box::new(cond),
            then_block,
            else_block,
            span,
        })
    }

    /// `if let PATTERN = EXPR { THEN } [else ...]`, desugared here into a
    /// block that binds the scrutinee to a fresh local and matches it with the
    /// pattern form of `when`:
    ///
    /// ```text
    /// { val #iflet0 = EXPR  when #iflet0 { PATTERN -> { THEN } else -> { ELSE } } }
    /// ```
    ///
    /// Nothing downstream needs a new case — and binding the scrutinee first is
    /// what makes a temporary legal here, since the pattern form of `when`
    /// requires a *place* (an owned payload binds as a borrow of it, and a
    /// borrow of a temporary would not outlive the arm).
    /// The payload binds of `Variant(a, _, c)` after the variant name has been
    /// read: the parenthesised list is optional, so `Empty` binds nothing.
    fn parse_variant_binds(&mut self, variant: String, vspan: Span) -> PResult<VariantPattern> {
        let mut binds = Vec::new();
        if self.check(&TokenKind::LParen) {
            self.advance();
            while !self.check(&TokenKind::RParen) {
                let (bind, _) = self.expect_ident()?;
                binds.push(if bind == "_" { None } else { Some(bind) });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
        }
        Ok(VariantPattern {
            variant,
            binds,
            span: vspan,
        })
    }

    /// `let PATTERN = EXPR else { BLOCK }` (`plan/syntax.md` 4.1), read up to
    /// and including the else block. The statement is only ever a `let` that
    /// starts a statement and is followed by a name or a parenthesis.
    fn at_let_else(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(name) if name == "let")
            && matches!(self.peek2_kind(), TokenKind::Ident(_) | TokenKind::LParen)
    }

    fn parse_let_else_head(&mut self) -> PResult<(VariantPattern, Expr, Block, Span)> {
        let start = self.advance().span; // `let`
        if self.check(&TokenKind::LParen) {
            return Err(ParseError {
                message: "let-else: a tuple pattern always matches, so the else block could never run. Fix: write 'val (a, b) = ...' with no else, or match an enum variant such as 'Some(x)' here".to_string(),
                span: self.peek().span,
            });
        }
        let (name, nspan) = self.expect_ident()?;
        if !self.check(&TokenKind::LParen) && self.check(&TokenKind::Eq) {
            return Err(ParseError {
                message: format!(
                    "let-else: '{name}' is a plain name, which always matches, so the else block could never run. Fix: write 'val {name} = ...' with no else, or match an enum variant such as 'Some({name})' here"
                ),
                span: nspan,
            });
        }
        let pattern = self.parse_variant_binds(name, nspan)?;
        self.expect(TokenKind::Eq, "'=' after the pattern")?;
        let init = self.parse_head_expr()?;
        if !self.check(&TokenKind::Else) {
            return Err(ParseError {
                message: "let-else: a 'let' with a pattern needs an 'else' block that leaves the enclosing code. Fix: add 'else { return ... }' (or break, continue, panic), or use 'if let' when the no-match case needs no exit".to_string(),
                span: self.peek().span,
            });
        }
        self.advance();
        let block = self.parse_block()?;
        let span = start.to(block.span);
        Ok((pattern, init, block, span))
    }

    fn parse_if_let(&mut self, start: Span) -> PResult<Expr> {
        self.advance(); // `let`
        let (variant, vspan) = self.expect_ident()?;
        let pattern = self.parse_variant_binds(variant, vspan)?;
        self.expect(TokenKind::Eq, "'=' after the pattern")?;
        let scrutinee = self.parse_head_expr()?;
        let scrutinee_span = scrutinee.span();
        let then_block = self.parse_block()?;
        let mut span = start.to(then_block.span);
        let else_expr = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                let elif = self.parse_if()?;
                span = span.to(elif.span());
                elif
            } else {
                let block = self.parse_block()?;
                span = span.to(block.span);
                Expr::Block(block)
            }
        } else if self.check(&TokenKind::Elif) {
            // `elif let Foo(x) = e { ... }` — the chained pattern link, in the
            // spelling 6.1 prefers. Same link, same function.
            let link = self.parse_elif()?;
            span = span.to(link.span());
            link
        } else {
            // No `else`, so the whole thing produces `Unit` — exactly what an
            // `if` without one does. An empty block is the `Unit` arm.
            Expr::Block(Block {
                stmts: Vec::new(),
                span: then_block.span,
            })
        };
        // See the long comment below for why a place gets no synthesized
        // binding at all, and anything else still does. A field chain over a
        // temporary stages its *root* (`mk().t` binds `mk()` and matches
        // `#iflet0.t`), so the temporary is freed once, at the end of the
        // desugared block, and the field is borrowed from a place that
        // outlives the arm.
        let (scrutinee_stmt, scrutinee_ref): (Option<Stmt>, Expr) =
            match temporary_root(&scrutinee).cloned() {
                Some(root) => {
                    self.if_lets += 1;
                    let tmp = format!("{}{}", IF_LET_PREFIX, self.if_lets - 1);
                    let root_span = root.span();
                    let stmt = Stmt::Let {
                        name: tmp.clone(),
                        mutable: false,
                        ty: None,
                        init: root,
                        span: root_span,
                    };
                    let rebased = rebase_root(&scrutinee, &Expr::Ident(tmp, scrutinee_span));
                    (Some(stmt), rebased)
                }
                None => (None, scrutinee),
            };
        let when_expr = Stmt::Expr {
            expr: Expr::When {
                scrutinee: Box::new(scrutinee_ref),
                arms: vec![
                    WhenArm {
                        pattern: Some(pattern),
                        body: Expr::Block(then_block),
                        span,
                    },
                    WhenArm {
                        pattern: None,
                        body: else_expr,
                        span,
                    },
                ],
                span,
                let_else: false,
            },
            span,
        };
        // The `when` is the block's last statement, which is what makes it the
        // block's value (statements are positional in this language — there is
        // no `tail` field to set).
        //
        // A scrutinee that is already a *place* (`if let Ok(v) = r`) is used
        // directly, with no synthesized binding at all — exactly what a bare
        // `when r { ... }` already does (6.1's own note: "a bare `when` needs
        // a place, since a payload borrow has to outlive the arm", and
        // matching a place never moves it, only a field of it). This is what
        // makes an `else if let PAT2 = r { ... }` chained after `if let PAT1
        // = r { ... }` legal: each link only *reads* `r` to test its tag, so
        // reading it twice is fine, the same way `when`'s own multi-arm form
        // reads its one scrutinee once and tests it repeatedly. Binding it to
        // a fresh `val #iflet0 = r` first — every earlier version of this
        // desugaring did that unconditionally — makes the *first* `if let`
        // look like it moves `r` into the temporary, and the next `else if
        // let`'s own re-evaluation of the same source expression then reads
        // an already-moved place. A scrutinee that is *not* a place (a call's
        // result, a literal) still needs the temporary, because `when`
        // requires one and there is nothing else to borrow through.
        match scrutinee_stmt {
            Some(stmt) => Ok(Expr::Block(Block {
                stmts: vec![stmt, when_expr],
                span,
            })),
            None => Ok(Expr::Block(Block {
                stmts: vec![when_expr],
                span,
            })),
        }
    }

    /// Parses both forms of `when`:
    ///
    /// - `when { cond -> value, ..., else -> value }` — no scrutinee, so it
    ///   is desugared here and now to a nested `if`/`else` chain; `infer.rs`
    ///   and `codegen_c.rs` never see this form at all.
    /// - `when expr { Variant(bind) -> value, ..., else -> value }` — a
    ///   pattern match (A-1), kept as `Expr::When` because only `infer.rs`
    ///   knows the scrutinee's type, and therefore which variants exist.
    ///
    /// The two never collide: the boolean form has no scrutinee.
    fn parse_when(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::When, "'when'")?.span;
        if !self.check(&TokenKind::LBrace) {
            return self.parse_when_patterns(start);
        }
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();

        let mut arms: Vec<(Option<Expr>, Expr)> = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let cond = if self.check(&TokenKind::Else) {
                self.advance();
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(TokenKind::Arrow, "'->'")?;
            let body = self.parse_expr()?;
            arms.push((cond, body));

            // A newline separates arms, but it is not *required* to: inside
            // `(` `)` the lexer does not emit newline tokens at all (see
            // `Lexer::paren_depth`), and a `when` is allowed to be a call
            // argument. An arm head is unambiguous either way — every arm is
            // `head -> value`, and `->` appears nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        let span = start.to(end);

        if arms.is_empty() {
            return Err(ParseError {
                message: "'when' needs at least one arm".to_string(),
                span,
            });
        }

        // Fold right-to-left: the trailing `else` (if any) becomes the base
        // else-block; every preceding arm wraps the accumulator in another
        // `if cond { body } else { <accumulator> }`.
        let mut acc: Option<ElseBranch> = None;
        for (cond, body) in arms.into_iter().rev() {
            let body_span = body.span();
            let body_block = Block {
                stmts: vec![Stmt::Expr {
                    expr: body,
                    span: body_span,
                }],
                span: body_span,
            };
            match cond {
                None => acc = Some(ElseBranch::Block(body_block)),
                Some(c) => {
                    let if_span = c.span().to(body_span);
                    let if_expr = Expr::If {
                        cond: Box::new(c),
                        then_block: body_block,
                        else_block: acc.take().map(Box::new),
                        span: if_span,
                    };
                    acc = Some(ElseBranch::If(if_expr));
                }
            }
        }

        match acc.unwrap() {
            ElseBranch::If(e) => Ok(e),
            ElseBranch::Block(b) => Ok(Expr::Block(b)),
        }
    }

    /// `when s { Circle(r) -> ... else -> ... }` — the pattern form. Arm
    /// heads are *shapes*, not expressions: a variant name with an optional
    /// `(bind, ...)` list, where each bind is an identifier or `_`.
    fn parse_when_patterns(&mut self, start: Span) -> PResult<Expr> {
        let scrutinee = self.parse_head_expr()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();

        if self.check(&TokenKind::LParen) {
            return self.parse_when_tuple_arms(start, scrutinee);
        }
        let mut arms: Vec<WhenArm> = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let pattern = if self.check(&TokenKind::Else) {
                self.advance();
                None
            } else {
                let (variant, vspan) = self.expect_ident()?;
                let mut binds = Vec::new();
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    while !self.check(&TokenKind::RParen) {
                        let (bind, _) = self.expect_ident()?;
                        // `_` is the ignore-a-payload binding; it lexes as an
                        // identifier because an identifier may start with `_`.
                        binds.push(if bind == "_" { None } else { Some(bind) });
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "')'")?;
                }
                Some(VariantPattern {
                    variant,
                    binds,
                    span: vspan,
                })
            };
            self.expect(TokenKind::Arrow, "'->'")?;
            let body = self.parse_expr()?;
            let arm_span = pattern
                .as_ref()
                .map(|p| p.span)
                .unwrap_or_else(|| body.span())
                .to(body.span());
            arms.push(WhenArm {
                pattern,
                body,
                span: arm_span,
            });

            // A newline separates arms, but it is not *required* to: inside
            // `(` `)` the lexer does not emit newline tokens at all (see
            // `Lexer::paren_depth`), and a `when` is allowed to be a call
            // argument. An arm head is unambiguous either way — every arm is
            // `head -> value`, and `->` appears nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        if arms.is_empty() {
            return Err(ParseError {
                message: "'when' needs at least one arm".to_string(),
                span: start.to(end),
            });
        }
        Ok(Expr::When {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.to(end),
            let_else: false,
        })
    }

    /// `when (a, b) { (Some(x), _) -> ... (n, None) -> ... else -> ... }`: arms
    /// whose heads are tuple patterns (`plan/syntax.md` 4). A position is a
    /// name, `_`, or a variant pattern (a capitalized name, with a bind list in
    /// parentheses when it has a payload). Nothing below the parser learns
    /// about it: the scrutinee is bound once and the arms are rewritten into
    /// nested single-column `when`s, one level per refutable position, so the
    /// existing checks (every variant covered, the payload arity, the moves)
    /// apply to each level. A row is tried in order and a position that
    /// matches everything is bound by name wherever it is reached.
    fn parse_when_tuple_arms(&mut self, start: Span, scrutinee: Expr) -> PResult<Expr> {
        let mut rows: Vec<TupleRow> = Vec::new();
        let mut width: Option<usize> = None;
        while !self.check(&TokenKind::RBrace) {
            let mut pats: Vec<TuplePat> = Vec::new();
            let head = self.peek().span;
            if self.check(&TokenKind::Else) {
                self.advance();
            } else {
                if !self.check(&TokenKind::LParen) {
                    return Err(ParseError {
                        message: "every arm of a 'when' over a tuple is a tuple pattern '(a, b)' or 'else'".to_string(),
                        span: head,
                    });
                }
                self.advance();
                loop {
                    pats.push(self.parse_tuple_position()?);
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
                self.expect(TokenKind::RParen, "')' to close the tuple pattern")?;
                if pats.len() < 2 {
                    return Err(ParseError {
                        message: "a tuple pattern needs two or more positions".to_string(),
                        span: head,
                    });
                }
                match width {
                    None => width = Some(pats.len()),
                    Some(w) if w != pats.len() => {
                        return Err(ParseError {
                            message: format!("this tuple pattern has {} positions and the first arm has {w}", pats.len()),
                            span: head,
                        })
                    }
                    Some(_) => {}
                }
            }
            self.expect(TokenKind::Arrow, "'->'")?;
            let body = self.parse_expr()?;
            rows.push(TupleRow { pats, binds: Vec::new(), body, span: head });
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        let span = start.to(end);
        let Some(width) = width else {
            return Err(ParseError {
                message: "a 'when' over a tuple needs at least one tuple pattern arm".to_string(),
                span,
            });
        };
        for row in rows.iter_mut() {
            if row.pats.is_empty() {
                row.pats = vec![TuplePat::Any(None); width];
            }
        }
        // A scrutinee written as a tuple literal is never built: its elements
        // are the positions, so a borrowed element (a parameter, a field) is
        // matched where it lives, which a tuple value could not hold. Anything
        // else that is not already a place is bound once first.
        let mut stmts: Vec<Stmt> = Vec::new();
        let stage = |this: &mut Parser, e: Expr, stmts: &mut Vec<Stmt>| -> Expr {
            if is_place_chain(&e) {
                return e;
            }
            this.when_temps += 1;
            let name = format!("{}{}", WHEN_TUPLE_PREFIX, this.when_temps - 1);
            let espan = e.span();
            stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: e, span: espan });
            Expr::Ident(name, espan)
        };
        let cols: Vec<Expr> = match scrutinee {
            Expr::TupleLit { elems, .. } if elems.len() == width => {
                let mut cols = Vec::new();
                for e in elems {
                    cols.push(stage(self, e, &mut stmts));
                }
                cols
            }
            other => {
                let whole = stage(self, other, &mut stmts);
                (0..width)
                    .map(|i| Expr::FieldAccess {
                        base: Box::new(whole.clone()),
                        field: crate::types::tuple_field_name(i),
                        span,
                    })
                    .collect()
            }
        };
        let inner = self.compile_tuple_rows(rows, &cols, span)?;
        stmts.push(Stmt::Expr { expr: inner, span });
        Ok(Expr::Block(Block { stmts, span }))
    }

    /// One position of a tuple pattern: `_`, a lowercase name, or a variant
    /// (`None`, `Some(x)`, `Rect(w, _)`).
    fn parse_tuple_position(&mut self) -> PResult<TuplePat> {
        self.nest()?;
        let r = self.parse_tuple_position_inner();
        self.depth -= 1;
        r
    }

    fn parse_tuple_position_inner(&mut self) -> PResult<TuplePat> {
        // A nested tuple pattern.
        if self.check(&TokenKind::LParen) {
            let open = self.advance().span;
            let mut elems = Vec::new();
            loop {
                elems.push(self.parse_tuple_position()?);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            let close = self.expect(TokenKind::RParen, "')' to close the tuple pattern")?.span;
            if elems.len() < 2 {
                return Err(ParseError {
                    message: "a tuple pattern needs two or more positions".to_string(),
                    span: open.to(close),
                });
            }
            return Ok(TuplePat::Tuple(elems, open.to(close)));
        }
        // A literal position.
        let lit_span = self.peek().span;
        match self.peek_kind().clone() {
            TokenKind::IntLit(v) => {
                self.advance();
                return Ok(TuplePat::Lit { key: format!("i{v}"), expr: Expr::IntLit(v, lit_span) });
            }
            TokenKind::Minus if matches!(self.peek2_kind(), TokenKind::IntLit(_)) => {
                self.advance();
                let TokenKind::IntLit(v) = self.peek_kind().clone() else { unreachable!() };
                let end = self.advance().span;
                return Ok(TuplePat::Lit {
                    key: format!("-i{v}"),
                    expr: Expr::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(Expr::IntLit(v, end)),
                        span: lit_span.to(end),
                    },
                });
            }
            TokenKind::CharLit(c) => {
                self.advance();
                return Ok(TuplePat::Lit { key: format!("c{c}"), expr: Expr::CharLit(c, lit_span) });
            }
            TokenKind::True | TokenKind::False => {
                let value = matches!(self.peek_kind(), TokenKind::True);
                self.advance();
                return Ok(TuplePat::Lit { key: format!("b{value}"), expr: Expr::BoolLit(value, lit_span) });
            }
            _ => {}
        }
        let (name, span) = self.expect_ident()?;
        if name == "_" {
            return Ok(TuplePat::Any(None));
        }
        let variant = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if !variant {
            return Ok(TuplePat::Any(Some(name)));
        }
        let mut binds = Vec::new();
        if self.check(&TokenKind::LParen) {
            self.advance();
            while !self.check(&TokenKind::RParen) {
                let (bind, _) = self.expect_ident()?;
                binds.push(if bind == "_" { None } else { Some(bind) });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
        }
        Ok(TuplePat::Variant { name, binds, span })
    }

    /// The rows left to try, as one expression, over the tuple positions
    /// `cols`. The first row decides the shape: when it has no variant pattern
    /// left it wins (its names are bound, then its body runs); otherwise the
    /// first variant position is matched with a `when` of its own, one arm per
    /// variant the rows name, each arm holding the rows that can still match
    /// there, and an `else` arm holding the rows that match anything.
    fn compile_tuple_rows(&mut self, rows: Vec<TupleRow>, cols: &[Expr], span: Span) -> PResult<Expr> {
        self.nest()?;
        let r = self.compile_tuple_rows_inner(rows, cols, span);
        self.depth -= 1;
        r
    }

    fn compile_tuple_rows_inner(&mut self, rows: Vec<TupleRow>, cols: &[Expr], span: Span) -> PResult<Expr> {
        // A nested tuple pattern splits its position into one position per
        // element before anything else is decided, so the rest only ever sees
        // flat columns.
        if let Some(c) = rows[0].pats.iter().position(|p| matches!(p, TuplePat::Tuple(..))) {
            return self.expand_tuple_column(rows, cols, c, span);
        }
        let first = &rows[0];
        // The first position the first row has to test: a variant or a literal.
        let Some(c) = first
            .pats
            .iter()
            .position(|p| matches!(p, TuplePat::Variant { .. } | TuplePat::Lit { .. }))
        else {
            let mut stmts = Vec::new();
            for (name, value) in &first.binds {
                stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: value.clone(), span });
            }
            for (i, p) in first.pats.iter().enumerate() {
                if let TuplePat::Any(Some(name)) = p {
                    stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: cols[i].clone(), span });
                }
            }
            let body = first.body.clone();
            let body_span = body.span();
            stmts.push(Stmt::Expr { expr: body, span: body_span });
            return Ok(Expr::Block(Block { stmts, span: first.span }));
        };
        if matches!(first.pats[c], TuplePat::Lit { .. }) {
            return self.compile_literal_column(rows, cols, c, span);
        }
        // The variants named in column `c`, in order of first appearance.
        let mut variants: Vec<(String, usize, Span)> = Vec::new();
        for row in &rows {
            if let TuplePat::Variant { name, binds, span } = &row.pats[c] {
                match variants.iter().find(|(n, _, _)| n == name) {
                    None => variants.push((name.clone(), binds.len(), *span)),
                    Some((_, arity, _)) if *arity != binds.len() => {
                        return Err(ParseError {
                            message: format!("'{name}' is bound with {} name(s) here and {arity} in an earlier arm", binds.len()),
                            span: *span,
                        })
                    }
                    Some(_) => {}
                }
            }
        }
        let mut arms: Vec<WhenArm> = Vec::new();
        for (vname, arity, vspan) in &variants {
            self.when_temps += 1;
            let payload: Vec<String> = (0..*arity).map(|j| format!("{}{}_{j}", WHEN_TUPLE_PREFIX, self.when_temps - 1)).collect();
            let mut group: Vec<TupleRow> = Vec::new();
            for row in &rows {
                let mut next = row.clone();
                match &row.pats[c] {
                    TuplePat::Variant { name, binds, .. } if name == vname => {
                        for (j, b) in binds.iter().enumerate() {
                            if let Some(b) = b {
                                next.binds.push((b.clone(), Expr::Ident(payload[j].clone(), *vspan)));
                            }
                        }
                    }
                    TuplePat::Variant { .. } => continue,
                    TuplePat::Any(name) => {
                        if let Some(n) = name {
                            next.binds.push((n.clone(), cols[c].clone()));
                        }
                    }
                    TuplePat::Lit { expr, .. } => {
                        return Err(ParseError {
                            message: "a position cannot be matched by a variant in one arm and a literal in another".to_string(),
                            span: expr.span(),
                        })
                    }
                    TuplePat::Tuple(_, tspan) => {
                        return Err(ParseError {
                            message: "a position cannot be matched by a variant in one arm and a tuple in another".to_string(),
                            span: *tspan,
                        })
                    }
                }
                next.pats[c] = TuplePat::Any(None);
                group.push(next);
            }
            let body = self.compile_tuple_rows(group, cols, span)?;
            let bspan = body.span();
            arms.push(WhenArm {
                pattern: Some(VariantPattern {
                    variant: vname.clone(),
                    binds: payload.into_iter().map(Some).collect(),
                    span: *vspan,
                }),
                body,
                span: vspan.to(bspan),
            });
        }
        let mut rest: Vec<TupleRow> = Vec::new();
        for row in &rows {
            if let TuplePat::Any(name) = &row.pats[c] {
                let mut next = row.clone();
                if let Some(n) = name {
                    next.binds.push((n.clone(), cols[c].clone()));
                }
                next.pats[c] = TuplePat::Any(None);
                rest.push(next);
            }
        }
        if !rest.is_empty() {
            // The rows that match any variant are the `else` arm. When they
            // test a literal with nothing to catch the other values, the arm
            // is dropped instead of refused: if the variants above already
            // cover the enum it was never reachable, and if they do not the
            // ordinary "not exhaustive" error names the missing variant.
            match self.compile_tuple_rows(rest, cols, span) {
                Ok(body) => {
                    let bspan = body.span();
                    arms.push(WhenArm { pattern: None, body, span: bspan });
                }
                Err(e) if e.message.starts_with(NO_LITERAL_FALLBACK) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Expr::When {
            scrutinee: Box::new(cols[c].clone()),
            arms,
            span,
            let_else: false,
        })
    }

    /// Splits column `c`, where the first row writes a nested tuple pattern,
    /// into one column per element (`cols[c].f0`, `cols[c].f1`, ...). A row that
    /// writes a tuple there contributes its elements; a row that writes a name
    /// or `_` matches every element and binds the name to the whole position.
    fn expand_tuple_column(&mut self, rows: Vec<TupleRow>, cols: &[Expr], c: usize, span: Span) -> PResult<Expr> {
        let TuplePat::Tuple(first_elems, _) = &rows[0].pats[c] else { unreachable!("the caller found a tuple here") };
        let width = first_elems.len();
        let mut new_cols: Vec<Expr> = cols[..c].to_vec();
        for j in 0..width {
            new_cols.push(Expr::FieldAccess {
                base: Box::new(cols[c].clone()),
                field: crate::types::tuple_field_name(j),
                span,
            });
        }
        new_cols.extend(cols[c + 1..].iter().cloned());
        let mut next_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut pats: Vec<TuplePat> = row.pats[..c].to_vec();
            let mut binds = row.binds.clone();
            match &row.pats[c] {
                TuplePat::Tuple(elems, tspan) => {
                    if elems.len() != width {
                        return Err(ParseError {
                            message: format!("this nested tuple pattern has {} positions and an earlier arm's has {width}", elems.len()),
                            span: *tspan,
                        });
                    }
                    pats.extend(elems.iter().cloned());
                }
                TuplePat::Any(name) => {
                    if let Some(n) = name {
                        binds.push((n.clone(), cols[c].clone()));
                    }
                    pats.extend(std::iter::repeat(TuplePat::Any(None)).take(width));
                }
                TuplePat::Variant { span: vspan, .. } => {
                    return Err(ParseError {
                        message: "a position that is a tuple in one arm cannot be a variant in another".to_string(),
                        span: *vspan,
                    })
                }
                TuplePat::Lit { expr, .. } => {
                    return Err(ParseError {
                        message: "a position that is a tuple in one arm cannot be a literal in another".to_string(),
                        span: expr.span(),
                    })
                }
            }
            pats.extend(row.pats[c + 1..].iter().cloned());
            next_rows.push(TupleRow { pats, binds, body: row.body, span: row.span });
        }
        self.compile_tuple_rows(next_rows, &new_cols, span)
    }

    /// Column `c`, where the first row writes a literal: `if col == lit { ... }
    /// else if col == other { ... } else { rows that match anything }`. A column
    /// of literals needs an arm that matches anything there, since a number has
    /// no list of values to be exhaustive over.
    fn compile_literal_column(&mut self, rows: Vec<TupleRow>, cols: &[Expr], c: usize, span: Span) -> PResult<Expr> {
        let mut lits: Vec<(String, Expr)> = Vec::new();
        for row in &rows {
            match &row.pats[c] {
                TuplePat::Lit { key, expr } => {
                    if !lits.iter().any(|(k, _)| k == key) {
                        lits.push((key.clone(), expr.clone()));
                    }
                }
                TuplePat::Any(_) => {}
                TuplePat::Variant { span: vspan, .. } => {
                    return Err(ParseError {
                        message: "a position cannot be matched by a literal in one arm and a variant in another".to_string(),
                        span: *vspan,
                    })
                }
                TuplePat::Tuple(_, tspan) => {
                    return Err(ParseError {
                        message: "a position cannot be matched by a literal in one arm and a tuple in another".to_string(),
                        span: *tspan,
                    })
                }
            }
        }
        let any_rows = |this: &mut Parser, rows: &[TupleRow], want: Option<&str>| -> Vec<TupleRow> {
            let _ = this;
            let mut group = Vec::new();
            for row in rows {
                let mut next = row.clone();
                match &row.pats[c] {
                    TuplePat::Lit { key, .. } if Some(key.as_str()) == want => {}
                    TuplePat::Lit { .. } => continue,
                    TuplePat::Any(name) => {
                        if let Some(n) = name {
                            next.binds.push((n.clone(), cols[c].clone()));
                        }
                    }
                    _ => continue,
                }
                next.pats[c] = TuplePat::Any(None);
                group.push(next);
            }
            group
        };
        let fallback = any_rows(self, &rows, None);
        if fallback.is_empty() {
            return Err(ParseError {
                message: format!("{NO_LITERAL_FALLBACK}: add an arm with a name or '_' in that position, or 'else'"),
                span: rows[0].span,
            });
        }
        let mut acc: ElseBranch = {
            let body = self.compile_tuple_rows(fallback, cols, span)?;
            ElseBranch::Block(Block { stmts: vec![Stmt::Expr { expr: body, span }], span })
        };
        for (key, lit) in lits.into_iter().rev() {
            let group = any_rows(self, &rows, Some(&key));
            let body = self.compile_tuple_rows(group, cols, span)?;
            let cond = Expr::Binary {
                op: BinOp::Eq,
                lhs: Box::new(cols[c].clone()),
                rhs: Box::new(lit),
                span,
            };
            acc = ElseBranch::If(Expr::If {
                cond: Box::new(cond),
                then_block: Block { stmts: vec![Stmt::Expr { expr: body, span }], span },
                else_block: Some(Box::new(acc)),
                span,
            });
        }
        match acc {
            ElseBranch::If(e) => Ok(e),
            ElseBranch::Block(b) => Ok(Expr::Block(b)),
        }
    }
}

/// Whether an expression is a place a `when` can match in place: a name, or a
/// field chain over one.
fn is_place_chain(e: &Expr) -> bool {
    match e {
        Expr::Ident(..) => true,
        Expr::FieldAccess { base, .. } => is_place_chain(base),
        _ => false,
    }
}

/// Whether a token starts the value of `break value`: a name, a literal, a
/// list or a template. Anything else (`break + 1`, `break(x)`) is not a value,
/// so `break` stays an ordinary name there; wrap other values in a `val`.
fn starts_break_value(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IntLit(_)
            | TokenKind::FloatLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::CharLit(_)
            | TokenKind::ByteLit(_)
            | TokenKind::TemplateStart
            | TokenKind::True
            | TokenKind::False
            | TokenKind::LBracket
            | TokenKind::Ident(_)
    )
}

/// A loop being parsed: its label, the hidden flag a labelled exit to it uses,
/// and the outer loops whose flags a `break`/`continue` inside it sets.
struct LoopCtx {
    label: Option<String>,
    flag: Option<String>,
    exits: Vec<usize>,
    /// For a `loop` used as a value: the hidden list its `break value`s push
    /// to, and how many of them there are.
    value_slot: Option<String>,
    valued_breaks: usize,
}

/// One position of a tuple pattern in a `when` arm.
#[derive(Debug, Clone)]
enum TuplePat {
    /// `_` (no name) or a name: matches anything.
    Any(Option<String>),
    /// `Some(x)` / `None`: a variant and the names of its payload.
    Variant { name: String, binds: Vec<Option<String>>, span: Span },
    /// `0`, `-1`, `'a'`, `true`: a literal the position is compared with. `key`
    /// tells two spellings of one value apart from two values.
    Lit { key: String, expr: Expr },
    /// `(a, b)` inside a tuple pattern: the position is itself a tuple.
    Tuple(Vec<TuplePat>, Span),
}

/// One arm of a tuple `when`, with the names already bound on the way to it.
#[derive(Debug, Clone)]
struct TupleRow {
    pats: Vec<TuplePat>,
    binds: Vec<(String, Expr)>,
    body: Expr,
    span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        Parser::new(tokens).parse_program().expect("parse failed")
    }

    fn parse_err(src: &str) -> ParseError {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        Parser::new(tokens).parse_program().expect_err("expected a parse error")
    }

    /// Kernel stage 2: a `repr(...)` clause before a `datac` or an `enum`, the
    /// two spellings of its position relative to `pub`, and the parse-time
    /// refusal of an alignment that is not a power of two.
    #[test]
    fn parses_a_repr_clause_before_a_datac_and_an_enum() {
        let p = parse(
            "repr(c) datac A(val x: i32)\nrepr(packed) datac B(val x: i32)\nrepr(align(16)) datac C(val x: i32)\npub repr(align(8)) datac D(val x: i32)\nrepr(packed) pub datac E(val x: i32)\nrepr(u8) enum M {\n    Off = 0\n    On = 1\n}\nfun main() {}",
        );
        assert_eq!(p.datacs[0].repr, Some(Repr::C));
        assert_eq!(p.datacs[1].repr, Some(Repr::Packed));
        assert_eq!(p.datacs[2].repr, Some(Repr::Align(16)));
        assert_eq!(p.datacs[3].repr, Some(Repr::Align(8)));
        assert!(p.datacs[3].is_pub);
        assert_eq!(p.datacs[4].repr, Some(Repr::Packed));
        assert!(p.datacs[4].is_pub);
        assert_eq!(p.enums[0].repr, Some(Repr::Int("u8".to_string())));
        assert_eq!(p.enums[0].variants[0].discriminant, Some(0));
        assert_eq!(p.enums[0].variants[1].discriminant, Some(1));
    }

    /// A-12 slice 2: a trait is a set of `Self`-receiver signatures, an impl
    /// block is a set of `Self`-receiver methods, and a bound is written on a
    /// function's own type parameter.
    #[test]
    fn parses_a_trait_an_impl_and_bounds() {
        let p = parse(
            "trait Shape {\n    fun Self.area().return: f64\n    fun mut Self.scale(f: f64)\n}\ndatac C(val r: f64)\nimpl Shape for C {\n    fun Self.area().return: f64 = r\n    fun mut Self.scale(f: f64) { r = f }\n}\nfun f<T: Shape + Other>(x: &T).return: f64 = x.area()\nfun main() {}",
        );
        assert_eq!(p.traits.len(), 1);
        let t = &p.traits[0];
        assert_eq!(t.name, "Shape");
        assert!(!t.is_pub);
        assert_eq!(t.methods.len(), 2);
        assert_eq!(t.methods[0].name, "area");
        assert_eq!(
            t.methods[0].receiver.as_ref().map(|r| r.type_name.as_str()),
            Some("Self")
        );
        assert!(!t.methods[0].receiver.as_ref().unwrap().mutable);
        assert!(t.methods[1].receiver.as_ref().unwrap().mutable);
        assert!(matches!(t.methods[0].body, FunctionBody::Signature));
        assert_eq!(p.trait_impls.len(), 1);
        assert_eq!(p.trait_impls[0].trait_name, "Shape");
        assert_eq!(p.trait_impls[0].type_name, "C");
        assert_eq!(p.trait_impls[0].methods, vec!["area", "scale"]);
        // The methods of an impl block are ordinary receiver methods, tagged
        // with the site they were written at.
        let impl_methods: Vec<&FunctionDecl> = p
            .functions
            .iter()
            .filter(|f| f.trait_impl.is_some())
            .collect();
        assert_eq!(impl_methods.len(), 2);
        assert_eq!(
            impl_methods[0].receiver.as_ref().map(|r| r.type_name.as_str()),
            Some("Self")
        );
        assert_eq!(
            impl_methods[0]
                .trait_impl
                .as_ref()
                .map(|s| s.self_type.as_str()),
            Some("C")
        );
        // `fun f<T: Shape + Other>`: one parameter, two bounds, in order.
        let f = p.functions.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(f.type_params, vec!["T"]);
        assert_eq!(f.type_param_bounds.len(), 1);
        assert_eq!(f.type_param_bounds[0].len(), 2);
        assert_eq!(f.type_param_bounds[0][0].name, "Shape");
        assert_eq!(f.type_param_bounds[0][1].name, "Other");
    }

    #[test]
    fn parses_default_methods_supertraits_and_builtin_impls() {
        let p = parse(
            "trait Shape {\n    fun Self.area().return: f64\n}\ntrait Named: Shape + Other {\n    fun Self.name().return: str\n    fun Self.loud().return: str { return self.name() }\n}\nimpl Named for i32 {\n    fun Self.name().return: str = \"n\"\n}\nfun main() {}",
        );
        assert_eq!(p.traits.len(), 2);
        let named = &p.traits[1];
        assert_eq!(named.supertraits.len(), 2);
        assert_eq!(named.supertraits[0].name, "Shape");
        assert_eq!(named.supertraits[1].name, "Other");
        // `name` is a signature, `loud` is a default with a body.
        assert!(matches!(named.methods[0].body, FunctionBody::Signature));
        assert!(matches!(named.methods[1].body, FunctionBody::Block(_)));
        assert_eq!(p.trait_impls[0].trait_name, "Named");
        assert_eq!(p.trait_impls[0].type_name, "i32");
    }

    #[test]
    fn a_trait_item_that_is_not_a_method_or_an_associated_type_does_not_parse() {
        let err = parse_err("trait Shape { type Item = i32 }\nfun main() {}");
        assert!(err.message.contains("default associated type"), "{}", err.message);
        let err = parse_err("trait Shape { type Item<T> }\nfun main() {}");
        assert!(err.message.contains("generic associated type"), "{}", err.message);
        let err = parse_err("trait Shape { const N: i32 = 1 }\nfun main() {}");
        assert!(err.message.contains("associated consts"), "{}", err.message);
    }

    #[test]
    fn an_associated_type_is_declared_in_a_trait_and_bound_in_an_impl() {
        let p = parse(
            "trait Src {\n    type Out: Named\n    fun Self.produce().return: Self::Out\n}\n\nimpl Src for i32 {\n    type Out = i32\n    fun Self.produce().return: i32 = self\n}\nfun main() {}",
        );
        let src = &p.traits[0];
        assert_eq!(src.assoc_types.len(), 1);
        assert_eq!(src.assoc_types[0].name, "Out");
        assert_eq!(src.assoc_types[0].bounds[0].name, "Named");
        assert_eq!(src.methods[0].return_type.as_ref().unwrap().name, "Self");
        assert_eq!(
            src.methods[0].return_type.as_ref().unwrap().assoc.as_deref(),
            Some("Out")
        );
        assert_eq!(p.trait_impls[0].assoc_types[0].name, "Out");
        assert_eq!(p.trait_impls[0].assoc_types[0].ty.name, "i32");
    }

    #[test]
    fn a_trait_takes_parameters_and_drop_does_not_parse() {
        // A trait may take type parameters (A-12 slice 6).
        let p = parse("trait A<T> { fun Self.m() }\nfun main() {}");
        assert_eq!(p.traits[0].type_params, vec!["T"]);
        let err = parse_err("trait Drop { fun Self.m() }\nfun main() {}");
        assert!(err.message.contains("'Drop' is built in"), "{}", err.message);
        // `trait A: B` no longer parses as an error: it is a supertrait.
        let p = parse("trait B { fun Self.m() }\ntrait A: B { fun Self.m() }\nfun main() {}");
        assert_eq!(p.traits[1].supertraits[0].name, "B");
    }

    #[test]
    fn a_bound_on_a_receivers_type_parameter_does_not_parse() {
        let err = parse_err("datac Pair<A, B>(val first: A, val second: B)\nfun Pair<A: Shape, B>.first().return: A = first\nfun main() {}");
        assert!(err.message.contains("bound cannot be written"), "{}", err.message);
    }

    #[test]
    fn a_bound_on_a_type_declarations_parameter_does_not_parse() {
        let err = parse_err("datac Pair<A: Shape>(val first: A)\nfun main() {}");
        assert!(err.message.contains("bound cannot be written"), "{}", err.message);
    }

    #[test]
    fn an_impl_static_method_parses_and_a_wrong_receiver_does_not() {
        // A receiverless impl method is a static member (A-12 slice 6).
        let p = parse("trait Shape { fun Self.area().return: f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun area().return: f64 = r }\nfun main() {}");
        assert!(
            p.functions
                .iter()
                .any(|f| f.name == "area" && f.receiver.is_none() && f.trait_impl.is_some()),
            "a static impl method should parse"
        );
        let err = parse_err("trait Shape { fun Self.area().return: f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun C.area().return: f64 = r }\nfun main() {}");
        assert!(err.message.contains("receiver is written 'Self'"), "{}", err.message);
        let err = parse_err("trait Shape { fun Self.area().return: f64 }\ndatac C(val r: f64)\nimpl Shape for C { fun Self<A>.area().return: f64 = r }\nfun main() {}");
        assert!(err.message.contains("cannot take type parameters"), "{}", err.message);
    }

    #[test]
    fn self_outside_a_trait_or_an_impl_does_not_parse() {
        let err = parse_err("fun Self.area().return: f64 = 0.0\nfun main() {}");
        assert!(err.message.contains("inside a 'trait' or an 'impl'"), "{}", err.message);
    }

    #[test]
    fn an_impl_for_a_bare_generic_does_not_parse() {
        let err = parse_err("trait Shape { fun Self.area().return: f64 }\nimpl Shape for List<i32> { fun Self.area().return: f64 = 0.0 }\nfun main() {}");
        assert!(err.message.contains("an impl for a generic type writes the type's parameters"), "{}", err.message);
    }

    #[test]
    fn a_bare_dyn_is_a_mark_and_impl_in_argument_position_is_desugared() {
        // A bare `dyn Name` parses as a mark so `Box<dyn Name>` can reach the
        // type checker (A-12 slice 7); the checker refuses the bare spelling
        // wherever it is not a `Box` argument, and `tests/fixtures/traits_dyn.kp`
        // pins that message.
        let p = parse("trait Shape { fun Self.area().return: f64 }\nfun take(s: dyn Shape) {}\nfun main() {}");
        let ty = &p.functions[0].params[0].ty;
        assert_eq!(ty.dyn_trait.as_deref(), Some("Shape"));
        assert!(!ty.amp);
        // `Box<dyn A + B>` is refused where the `+` is: a trait object names one
        // trait.
        let err = parse_err("trait A { fun Self.a().return: i32 }\ntrait B { fun Self.b().return: i32 }\nfun take(s: Box<dyn A + B>) {}\nfun main() {}");
        assert!(err.message.contains("names one trait"), "{}", err.message);
        // A-20: `impl Shape` now parses as a *parameter* type and is
        // desugared into a fresh bounded parameter; a stored position (`let`,
        // a field, a return) is refused by the type checker.
        let p = parse("trait Shape { fun Self.area().return: f64 }\nfun take(s: impl Shape) {}\nfun main() {}");
        assert_eq!(p.functions[0].params[0].ty.impl_trait, None);
        assert_eq!(p.functions[0].type_params.len(), 1);
        assert_eq!(p.functions[0].type_param_bounds[0][0].name, "Shape");
    }

    #[test]
    fn parses_enum_discriminants_including_a_negative_one() {
        let p = parse("enum E {\n    A = -5\n    B\n    C = 7\n    D\n}\nfun main() {}");
        let e = &p.enums[0];
        assert_eq!(e.variants[0].discriminant, Some(-5));
        assert_eq!(e.variants[1].discriminant, None);
        assert_eq!(e.variants[2].discriminant, Some(7));
        assert_eq!(e.variants[3].discriminant, None);
    }

    #[test]
    fn an_alignment_that_is_not_a_power_of_two_does_not_parse() {
        let err = parse_err("repr(align(3)) datac A(val x: i32)\nfun main() {}");
        assert!(err.message.contains("power of two"), "{}", err.message);
    }

    #[test]
    fn an_unknown_repr_does_not_parse() {
        let err = parse_err("repr(weird) datac A(val x: i32)\nfun main() {}");
        assert!(err.message.contains("unknown 'repr(weird)'"), "{}", err.message);
    }

    #[test]
    fn a_repr_clause_on_a_non_type_item_does_not_parse() {
        let err = parse_err("repr(c) fun main() {}");
        assert!(err.message.contains("applies to a 'datac' or an 'enum'"), "{}", err.message);
    }

    #[test]
    fn a_section_clause_is_read_on_a_const_and_on_a_fun() {
        let p = parse(
            "section(\".rodata.boot\") const K: [u8; 2] = [1, 2]\n\
             section(\".text.boot\") fun boot() {}\n\
             pub section(\".x\") fun h().return: i32 = 1\n",
        );
        assert_eq!(p.consts[0].section.as_deref(), Some(".rodata.boot"));
        assert_eq!(p.functions[0].section.as_deref(), Some(".text.boot"));
        assert_eq!(p.functions[1].section.as_deref(), Some(".x"));
    }

    #[test]
    fn a_section_clause_on_a_non_const_or_fun_does_not_parse() {
        let err = parse_err("section(\".x\") datac A(val x: i32)");
        assert!(
            err.message.contains("'const', 'static' or 'fun'"),
            "{}",
            err.message
        );
    }

    #[test]
    fn parses_static_declarations() {
        let p = parse(
            "static A: i32 = 1\n\
             static mut B: [u8; 2] = [1, 2]\n\
             pub section(\".data.boot\") static mut C: u32 = 0\n",
        );
        assert_eq!(p.statics.len(), 3);
        assert_eq!(p.statics[0].name, "A");
        assert!(!p.statics[0].mutable);
        assert_eq!(p.statics[1].name, "B");
        assert!(p.statics[1].mutable);
        assert!(p.statics[2].is_pub);
        assert!(p.statics[2].mutable);
        assert_eq!(p.statics[2].section.as_deref(), Some(".data.boot"));
    }

    #[test]
    fn parses_a_percpu_static_beside_pub_and_section() {
        let p = parse(
            "percpu static mut A: i32 = 1\n\
             pub percpu static mut B: u32 = 2\n\
             section(\".percpu.boot\") percpu static mut C: u8 = 3\n",
        );
        assert_eq!(p.statics.len(), 3);
        assert!(p.statics[0].percpu && p.statics[0].mutable);
        assert!(p.statics[1].percpu && p.statics[1].is_pub);
        assert!(p.statics[2].percpu);
        assert_eq!(p.statics[2].section.as_deref(), Some(".percpu.boot"));
    }

    #[test]
    fn a_percpu_that_does_not_precede_a_static_does_not_parse() {
        let err = parse_err("percpu fun f() {}");
        assert!(
            err.message.contains("'percpu' applies to a 'static'"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_section_clause_on_an_extern_fun_does_not_parse() {
        let err = parse_err("section(\".x\") extern fun f(n: i32).return: i32");
        assert!(
            err.message.contains("no code of its own to place"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_empty_section_name_does_not_parse() {
        let err = parse_err("section(\"\") fun f() {}");
        assert!(err.message.contains("needs a section name"), "{}", err.message);
    }

    #[test]
    fn an_align_clause_is_read_on_a_fun() {
        let p = parse(
            "align(4) fun h() {}\n\
             pub section(\".text.trap\") align(4096) fun trap() {}\n\
             align(2) section(\".text.other\") fun boot() {}\n",
        );
        assert_eq!(p.functions[0].align, Some(4));
        assert_eq!(p.functions[1].align, Some(4096));
        assert_eq!(p.functions[1].section.as_deref(), Some(".text.trap"));
        assert_eq!(p.functions[2].align, Some(2));
        assert_eq!(p.functions[2].section.as_deref(), Some(".text.other"));
    }

    #[test]
    fn an_align_that_is_not_a_power_of_two_up_to_4096_does_not_parse() {
        for src in ["align(3) fun f() {}", "align(8192) fun f() {}"] {
            let err = parse_err(src);
            assert!(
                err.message.contains("power of two up to 4096"),
                "{src}: {}",
                err.message
            );
        }
    }

    #[test]
    fn an_align_clause_on_a_const_does_not_parse() {
        let err = parse_err("align(4) const K: i32 = 1");
        assert!(
            err.message.contains("applies to a top-level 'fun'"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_align_clause_on_an_extern_fun_does_not_parse() {
        let err = parse_err("align(4) extern fun f(n: i32).return: i32");
        assert!(
            err.message.contains("no code of its own to align"),
            "{}",
            err.message
        );
    }

    #[test]
    fn parses_empty_block_bodied_function() {
        let p = parse("fun main() {}");
        assert_eq!(p.functions.len(), 1);
        assert_eq!(p.functions[0].name, "main");
        assert!(p.functions[0].params.is_empty());
        assert!(p.functions[0].return_type.is_none());
        assert!(matches!(p.functions[0].body, FunctionBody::Block(_)));
    }

    #[test]
    fn a_tuple_type_is_its_element_list() {
        // MISSING-z2bB9: `(A, B)` is a type, and `(A)` is not a tuple but the
        // parenthesized type `A` (`plan/syntax.md` 3).
        let p = parse("fun f(p: (i32, String)).return: (i32, i32) = (1, 2)");
        let param = &p.functions[0].params[0].ty;
        assert_eq!(param.display(), "(i32, String)");
        assert_eq!(param.tuple.as_ref().unwrap().len(), 2);
        let ret = p.functions[0].return_type.as_ref().unwrap();
        assert_eq!(ret.display(), "(i32, i32)");

        let paren = parse("fun f().return: (i32) = 1");
        let ret = paren.functions[0].return_type.as_ref().unwrap();
        assert!(ret.tuple.is_none(), "(T) is the parenthesized type T");
        assert_eq!(ret.display(), "i32");

        let nested = parse("fun f(p: ((i32, i32), i32)) {}");
        assert_eq!(
            nested.functions[0].params[0].ty.display(),
            "((i32, i32), i32)"
        );
    }

    #[test]
    fn an_empty_tuple_type_is_refused_by_name() {
        let err = parse_err("fun f(p: ()) {}");
        assert!(err.message.contains("'()' is not a type"), "{}", err.message);
    }

    #[test]
    fn a_tuple_literal_needs_two_elements() {
        // `(e)` is the parenthesized expression it always was, and a tuple
        // needs a comma, so one element is never a tuple.
        let p = parse("fun f().return: i32 {\n    val a = (1)\n    return a\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { init, .. } => {
                    assert!(matches!(init, Expr::IntLit(1, _)), "got {init:?}");
                }
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }

        let p = parse("fun f() {\n    val p = (1, 2)\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { init, .. } => match init {
                    Expr::TupleLit { elems, .. } => assert_eq!(elems.len(), 2),
                    other => panic!("expected a tuple literal, got {other:?}"),
                },
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }

        // `()` is the unit value: an empty block, which evaluates to `unit`.
        let p = parse("fun f() {\n    val u = ()\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { init, .. } => match init {
                    Expr::Block(unit) => assert!(unit.stmts.is_empty(), "got {init:?}"),
                    other => panic!("expected the unit value, got {other:?}"),
                },
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }
    }

    #[test]
    fn a_destructuring_binding_binds_every_position() {
        // MISSING-z2bB9: `val (lo, _) = p`, with `_` ignoring a position and a
        // nested pattern allowed (`plan/syntax.md` 4).
        let p = parse("fun f(p: (i32, i32)) {\n    val (lo, _) = p\n    var ((a, b), c) = ((1, 2), 3)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            other => panic!("expected a block body, got {other:?}"),
        };
        assert_eq!(stmts.len(), 2);
        let Stmt::LetTuple {
            pattern, mutable, ..
        } = &stmts[0]
        else {
            panic!("expected a destructuring binding, got {:?}", stmts[0]);
        };
        assert!(!*mutable);
        let BindingPattern::Tuple(positions) = pattern else {
            panic!("expected a tuple pattern, got {pattern:?}");
        };
        assert!(matches!(positions[0], BindingPattern::Name(ref n) if n == "lo"));
        assert!(matches!(positions[1], BindingPattern::Ignore));

        let Stmt::LetTuple {
            pattern, mutable, ..
        } = &stmts[1]
        else {
            panic!("expected a destructuring binding, got {:?}", stmts[1]);
        };
        assert!(*mutable, "`var (a, b) = ...` makes every position a `var`");
        let mut names = Vec::new();
        pattern.binds(&mut Vec::new(), &mut names);
        assert_eq!(
            names.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(names[1].1, vec![0, 1], "`b` is element 2 of element 1");
    }

    #[test]
    fn a_destructuring_binding_needs_two_positions_and_no_written_type() {
        let err = parse_err("fun f() {\n    val (a) = 1\n}");
        assert!(
            err.message.contains("needs two or more positions"),
            "{}",
            err.message
        );
        let err = parse_err("fun f() {\n    val (a, b): (i32, i32) = (1, 2)\n}");
        assert!(
            err.message.contains("takes no written type"),
            "{}",
            err.message
        );
    }

    #[test]
    fn parses_params_and_return_clause() {
        let p = parse("fun add(a: i32, b: i32).return: i32 {\n    return a + b\n}");
        let f = &p.functions[0];
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "a");
        assert_eq!(f.params[0].ty.name, "i32");
        assert_eq!(f.return_type.as_ref().unwrap().name, "i32");
        match &f.body {
            FunctionBody::Block(b) => {
                assert_eq!(b.stmts.len(), 1);
                match &b.stmts[0] {
                    Stmt::Return { value: Some(e), .. } => {
                        assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. }));
                    }
                    other => panic!("expected return stmt, got {other:?}"),
                }
            }
            other => panic!("expected block body, got {other:?}"),
        }
    }

    /// The number of links in an `if` chain: one per `if`/`elif`, plus one for
    /// a trailing `else` block. Both spellings have to give the same number,
    /// which is the whole claim `elif` makes (`plan/syntax.md` 6.1).
    fn links(branch: &ElseBranch) -> usize {
        match branch {
            ElseBranch::If(e) => match e {
                Expr::If { else_block, .. } => {
                    1 + else_block.as_deref().map(links).unwrap_or(0)
                }
                other => panic!("expected an if, got {other:?}"),
            },
            ElseBranch::Block(_) => 1,
        }
    }

    /// The chain of the first statement of the first function in `src`.
    fn chain(src: &str) -> usize {
        let p = parse(src);
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Expr { expr, .. } => match expr {
                    Expr::If { else_block, .. } => {
                        1 + else_block.as_deref().map(links).unwrap_or(0)
                    }
                    other => panic!("expected an if, got {other:?}"),
                },
                other => panic!("expected an expression statement, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }
    }

    #[test]
    fn elif_is_the_same_link_as_else_if() {
        let spelled_out = "fun f(n: i32).return: i32 {\n    if n > 2 { return 1 } else if n > 1 { return 2 } else { return 3 }\n}";
        let spelled_elif = "fun f(n: i32).return: i32 {\n    if n > 2 { return 1 } elif n > 1 { return 2 } else { return 3 }\n}";
        assert_eq!(chain(spelled_out), 3);
        assert_eq!(chain(spelled_elif), chain(spelled_out));
    }

    #[test]
    fn an_if_chain_may_mix_both_spellings() {
        let mixed = "fun f(n: i32).return: i32 {\n    if n > 3 { return 1 } else if n > 2 { return 2 } elif n > 1 { return 3 } else { return 4 }\n}";
        assert_eq!(chain(mixed), 4);
    }

    #[test]
    fn elif_needs_no_else_after_it() {
        let src = "fun f(n: i32).return: i32 {\n    if n > 1 { return 1 } elif n > 0 { return 2 }\n}";
        assert_eq!(chain(src), 2);
    }

    #[test]
    fn elif_let_is_the_chained_pattern_link() {
        // `elif let PAT = e { ... }` is `else if let PAT = e { ... }` in the
        // preferred spelling, so it goes through the same desugaring: a block
        // whose `when` carries the link.
        let after_a_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer).return: i32 {\n    if let Yes(n) = m {\n        return n\n    } elif let No = m {\n        return 0\n    }\n    return 1\n}";
        assert_eq!(parse(after_a_pattern).functions.len(), 1);
        // And a chain may leave the pattern form for a boolean one, either
        // way round.
        let leaving_the_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer).return: i32 {\n    if let Yes(n) = m {\n        return n\n    } elif n > 1 {\n        return 2\n    }\n    return 1\n}";
        assert_eq!(parse(leaving_the_pattern).functions.len(), 1);
    }

    #[test]
    fn an_elif_that_follows_nothing_is_an_error() {
        parse_err("fun f(n: i32) {\n    elif n > 1 { }\n}");
    }

    #[test]
    fn elsif_is_not_a_spelling() {
        parse_err("fun f(n: i32) {\n    if n > 1 { } elsif n > 0 { }\n}");
    }

    #[test]
    fn parses_expression_bodied_function() {
        let p = parse("fun add(a: i32, b: i32).return: i32 = a + b");
        match &p.functions[0].body {
            FunctionBody::Expr(e) => assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. })),
            other => panic!("expected expr body, got {other:?}"),
        }
    }

    #[test]
    fn expression_body_without_return_type_is_an_error() {
        parse_err("fun add(a: i32) = a");
    }

    #[test]
    fn parses_val_and_var() {
        let p = parse("fun main() {\n    val x = 1\n    var y: i32 = 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
        match &stmts[0] {
            Stmt::Let { name, mutable, ty, init, .. } => {
                assert_eq!(name, "x");
                assert!(!mutable);
                assert!(ty.is_none());
                assert!(matches!(init, Expr::IntLit(1, _)));
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { name, mutable, ty, .. } => {
                assert_eq!(name, "y");
                assert!(*mutable);
                assert_eq!(ty.as_ref().unwrap().name, "i32");
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn newline_terminates_statements_without_semicolons() {
        let p = parse("fun main() {\n    val x = 1\n    val y = 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn two_statements_on_one_line_without_separator_is_an_error() {
        parse_err("fun main() {\n    val x = 1 val y = 2\n}");
    }

    #[test]
    fn respects_operator_precedence() {
        let p = parse("fun main() {\n    val x = 1 + 2 * 3\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init, .. } => match init {
                Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                    assert!(matches!(**lhs, Expr::IntLit(1, _)));
                    assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
                }
                other => panic!("expected top-level addition, got {other:?}"),
            },
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_else_as_expression_and_statement() {
        let p = parse(
            "fun main() {\n    if 1 < 2 {\n        print(1)\n    } else {\n        print(2)\n    }\n    val x = if true { 1 } else { 2 }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::Expr { expr: Expr::If { .. }, .. }));
        match &stmts[1] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::If { .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_while_with_compound_assignment() {
        let p = parse("fun main() {\n    var i = 0\n    while i < 10 {\n        i += 1\n    }\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[1] {
            Stmt::While { body, .. } => {
                assert_eq!(body.stmts.len(), 1);
                match &body.stmts[0] {
                    Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                        assert!(matches!(**value, Expr::Binary { op: BinOp::Add, .. }));
                    }
                    other => panic!("expected assign stmt, got {other:?}"),
                }
            }
            other => panic!("expected while stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_enum_and_a_pattern_when() {
        let p = parse(
            "enum Shape {\n    Circle(val r: f64)\n    Rect(val w: f64, val h: f64)\n    Empty\n}\n\ndatac Point(val x: i32)\n\nfun area(s: &Shape).return: f64 = when s {\n    Circle(r) -> r\n    Rect(w, h) -> w * h\n    Empty -> 0.0\n}",
        );
        assert_eq!(p.enums.len(), 1);
        let e = &p.enums[0];
        assert_eq!(e.name, "Shape");
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].fields.len(), 1);
        assert_eq!(e.variants[0].fields[0].name, "r");
        assert_eq!(e.variants[1].fields.len(), 2);
        assert!(e.variants[2].fields.is_empty());
        // `datac` and `enum` share one declaration order, which is what
        // `resolve.rs` merges and codegen emits in.
        assert_eq!(e.order, 0);
        assert_eq!(p.datacs[0].order, 1);

        match &p.functions[0].body {
            FunctionBody::Expr(Expr::When { arms, .. }) => {
                assert_eq!(arms.len(), 3);
                let first = arms[0].pattern.as_ref().unwrap();
                assert_eq!(first.variant, "Circle");
                assert_eq!(first.binds, vec![Some("r".to_string())]);
                assert!(arms[2].pattern.as_ref().unwrap().binds.is_empty());
                assert!(matches!(arms[2].body, Expr::FloatLit(..)));
            }
            other => panic!("expected a pattern `when`, got {other:?}"),
        }
    }

    #[test]
    fn an_else_arm_in_a_pattern_when_parses_as_a_catch_all() {
        let p = parse(
            "enum Flag {\n    On\n    Off\n}\n\nfun f(x: &Flag).return: i32 = when x {\n    On -> 1\n    else -> 2\n}",
        );
        match &p.functions[0].body {
            FunctionBody::Expr(Expr::When { arms, .. }) => {
                assert!(arms[0].pattern.is_some());
                assert!(arms[1].pattern.is_none());
            }
            other => panic!("expected a pattern `when`, got {other:?}"),
        }
    }

    #[test]
    fn if_let_desugars_to_a_bound_scrutinee_and_a_single_arm_when() {
        // MK5-S6-M1: `if let P = e { A } else { B }` becomes
        // `{ val #iflet0 = e  when #iflet0 { P -> { A } else -> { B } } }`.
        let p = parse(
            "fun main() {\n    if let Circle(r) = Shape.Circle(r = 1.0) {\n        print(r)\n    } else {\n        print(0.0)\n    }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let inner = match &stmts[0] {
            Stmt::Expr { expr: Expr::Block(b), .. } => b,
            other => panic!("expected a desugared block, got {other:?}"),
        };
        match &inner.stmts[0] {
            Stmt::Let { name, init, .. } => {
                assert_eq!(name, "#iflet0");
                assert!(matches!(init, Expr::MethodCall { .. }));
            }
            other => panic!("expected the scrutinee binding, got {other:?}"),
        }
        match &inner.stmts[1] {
            Stmt::Expr { expr: Expr::When { scrutinee, arms, .. }, .. } => {
                assert!(matches!(**scrutinee, Expr::Ident(ref n, _) if n == "#iflet0"));
                assert_eq!(arms.len(), 2);
                assert!(arms[0].pattern.is_some());
                assert!(arms[1].pattern.is_none());
            }
            other => panic!("expected the `when`, got {other:?}"),
        }
    }

    #[test]
    fn a_for_loop_is_kept_as_written_until_its_iterable_has_a_type() {
        // A-5/`plan/iterators.md`: which desugar applies depends on the
        // iterable's type, which only `infer.rs` knows, so the parser keeps the
        // statement and `infer.rs` lowers it.
        let p = parse(
            "fun main() {\n    for x in xs {\n        print(x)\n    }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                assert_eq!(binding, "x");
                assert!(matches!(iterable, Expr::Ident(n, _) if n == "xs"));
                assert_eq!(body.stmts.len(), 1);
            }
            other => panic!("expected the raw 'for', got {other:?}"),
        }
    }

    #[test]
    fn a_bare_break_or_continue_is_a_loop_control_statement() {
        let p = parse("fun main() {\n    while true {\n        break\n    }\n}");
        let outer = match &p.functions[0].body {
            FunctionBody::Block(b) => b,
            _ => panic!("expected block body"),
        };
        let inner = match &outer.stmts[0] {
            Stmt::While { body, .. } => body,
            other => panic!("expected the while, got {other:?}"),
        };
        assert!(matches!(inner.stmts[0], Stmt::Break { .. }));
    }

    #[test]
    fn parses_datac_decl_with_val_and_var_fields() {
        let p = parse("datac Point(val x: i32, var y: i32)\nfun main() {}");
        assert_eq!(p.datacs.len(), 1);
        let d = &p.datacs[0];
        assert_eq!(d.name, "Point");
        assert!(d.type_params.is_empty());
        assert_eq!(d.fields.len(), 2);
        assert!(!d.fields[0].mutable);
        assert!(d.fields[1].mutable);
    }

    #[test]
    fn parses_type_parameters_on_a_datac_and_an_enum() {
        // A-2: the parameter list sits between the declared name and the
        // declaration's own `(` or `{`, and a use site writes the arguments.
        let p = parse("datac Pair<A, B>(val first: A, val second: B)\nfun main() {}");
        assert_eq!(p.datacs[0].type_params, vec!["A".to_string(), "B".to_string()]);

        let p = parse("enum Option<T> {\n    Some(val value: T)\n    None\n}\nfun main() {}");
        assert_eq!(p.enums[0].type_params, vec!["T".to_string()]);

        let p = parse("fun f(p: Pair<i32, String>).return: Option<i32> = 1\nfun main() {}");
        let ty = &p.functions[0].params[0].ty;
        assert_eq!(ty.name, "Pair");
        assert_eq!(ty.args.len(), 2);
        assert_eq!(ty.args[0].name, "i32");
        assert_eq!(ty.args[1].name, "String");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().args.len(), 1);
    }

    #[test]
    fn generic_declaration_errors_are_parse_errors() {
        parse_err("datac Pair<T, T>(val a: T, val b: T)\nfun main() {}");
        parse_err("datac Pair<>(val a: i32)\nfun main() {}");
    }

    #[test]
    fn parses_type_parameters_on_a_function() {
        // A-2: a free function declares its own parameters after its name, and
        // a receiver method's come from the receiver's argument list.
        let p = parse("fun first<T>(xs: List<T>).return: T = xs.get(i = 0)\nfun main() {}");
        assert_eq!(p.functions[0].type_params, vec!["T".to_string()]);
        assert!(p.functions[0].receiver.is_none());

        let p = parse("fun Pair<A, B>.first().return: A = a\nfun main() {}");
        let f = &p.functions[0];
        assert!(f.type_params.is_empty());
        let r = f.receiver.as_ref().expect("a receiver");
        assert_eq!(r.type_name, "Pair");
        assert_eq!(r.type_args, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parses_const_type_parameters_and_const_arguments() {
        // A-20: `const N: usize` is a value parameter, listed with the type
        // parameters and recorded separately. A const argument is written like
        // a type argument (`Buf<16>`, `Buf<SIZE * 2>`).
        let p = parse(
            "datac Buf<const N: usize>(var data: [i32; N], var len: usize)\nfun main() {}",
        );
        assert_eq!(p.datacs[0].type_params, vec!["N".to_string()]);
        assert_eq!(p.datacs[0].const_params.len(), 1);
        assert_eq!(p.datacs[0].const_params[0].name, "N");
        assert_eq!(p.datacs[0].const_params[0].ty, "usize");

        let p = parse("fun f<const N: usize, T>().return: i32 = 0\nfun main() {}");
        assert_eq!(p.functions[0].type_params, vec!["N".to_string(), "T".to_string()]);
        assert_eq!(p.functions[0].const_params.len(), 1);
        assert_eq!(p.functions[0].const_params[0].name, "N");

        let p = parse("fun f(a: Buf<16>).return: i32 = 0\nfun main() {}");
        let arg = &p.functions[0].params[0].ty.args[0];
        assert!(arg.const_expr.is_some());
        assert!(arg.name.is_empty());

        let p = parse("fun f(a: Buf<SIZE * 2>).return: i32 = 0\nfun main() {}");
        let arg = &p.functions[0].params[0].ty.args[0];
        assert!(matches!(
            arg.const_expr.as_deref(),
            Some(Expr::Binary { op: BinOp::Mul, .. })
        ));

        // A bare name (`Buf<SIZE>`) parses as a type argument; the resolver
        // reads it as the const where the parameter is a `const`.
        let p = parse("fun f(a: Buf<SIZE>).return: i32 = 0\nfun main() {}");
        assert_eq!(p.functions[0].params[0].ty.args[0].name, "SIZE");
    }

    #[test]
    fn parses_impl_trait_in_argument_position_as_a_bounded_parameter() {
        // A-20: each `impl Trait` occurrence becomes its own fresh parameter,
        // and the trait is its bound.
        let p = parse("fun f(x: impl Shape, y: &impl Shape).return: i32 = 0\nfun main() {}");
        let f = &p.functions[0];
        assert_eq!(f.type_params.len(), 2);
        assert_ne!(f.type_params[0], f.type_params[1]);
        assert_eq!(f.type_param_bounds.len(), 2);
        assert_eq!(f.type_param_bounds[0][0].name, "Shape");
        assert_eq!(f.params[0].ty.name, f.type_params[0]);
        assert_eq!(f.params[1].ty.name, f.type_params[1]);
        assert!(f.params[1].ty.amp);
    }

    #[test]
    fn impl_trait_in_a_return_position_is_refused_at_parse_time() {
        parse_err(
            "trait Shape { fun Self.area().return: i32 }\nfun f().return: impl Shape = 1\nfun main() {}",
        );
    }

    #[test]
    fn parses_construction_and_field_access_via_call_syntax() {
        let p = parse("fun main() {\n    val p = Point(x = 1, y = 2)\n    val z = p.x.y\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { callee, args, .. }, .. } => {
                assert_eq!(callee, "Point");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0].name.as_deref(), Some("x"));
                assert_eq!(args[1].name.as_deref(), Some("y"));
            }
            other => panic!("expected call, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init: Expr::FieldAccess { field, base, .. }, .. } => {
                assert_eq!(field, "y");
                assert!(matches!(**base, Expr::FieldAccess { .. }));
            }
            other => panic!("expected field access, got {other:?}"),
        }
    }

    #[test]
    fn explicit_type_arguments_are_recognized_only_when_adjacent() {
        // A-2: `Pair<i32, String>(...)` — the `<` touches the name and the
        // `(` touches the `>`, which is what keeps an ordinary comparison
        // chain (`a < b > (c)`) from being read as an instantiation.
        let p = parse("fun main() {\n    val x = Pair<i32, String>(a = 1)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { type_args, .. }, .. } => {
                assert_eq!(type_args.len(), 2);
                assert_eq!(type_args[0].name, "i32");
                assert_eq!(type_args[1].name, "String");
            }
            other => panic!("expected a call with type arguments, got {other:?}"),
        }

        let p = parse("fun main() {\n    val y = a < b > (d)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Binary { op: BinOp::Gt, .. }, .. } => {}
            other => panic!("expected a comparison, got {other:?}"),
        }
    }

    #[test]
    fn parses_mixed_named_and_positional_call_args() {
        let p = parse("fun main() {\n    print(f(1, y = 2))\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Expr { expr: Expr::Call { args, .. }, .. } => {
                let inner = &args[0].value;
                match inner {
                    Expr::Call { args, .. } => {
                        assert!(args[0].name.is_none());
                        assert_eq!(args[1].name.as_deref(), Some("y"));
                    }
                    other => panic!("expected inner call, got {other:?}"),
                }
            }
            other => panic!("expected call stmt, got {other:?}"),
        }
    }

    #[test]
    fn does_not_confuse_equality_with_named_arg() {
        let p = parse("fun main() {\n    val x = f(a == 1)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { args, .. }, .. } => {
                assert!(args[0].name.is_none());
                assert!(matches!(args[0].value, Expr::Binary { op: BinOp::Eq, .. }));
            }
            other => panic!("expected call, got {other:?}"),
        }
    }

    #[test]
    fn parses_when_and_desugars_to_nested_if() {
        let p = parse(
            "fun classify(n: i32).return: i32 = when {\n    n > 0 -> 1\n    n < 0 -> -1\n    else -> 0\n}",
        );
        match &p.functions[0].body {
            FunctionBody::Expr(Expr::If { then_block, else_block, .. }) => {
                assert_eq!(then_block.stmts.len(), 1);
                match else_block.as_deref() {
                    Some(ElseBranch::If(Expr::If { else_block, .. })) => {
                        assert!(matches!(else_block.as_deref(), Some(ElseBranch::Block(_))));
                    }
                    other => panic!("expected nested else-if, got {other:?}"),
                }
            }
            other => panic!("expected desugared if, got {other:?}"),
        }
    }

    #[test]
    fn parses_compound_assignment_operators() {
        let p = parse("fun main() {\n    var x = 1\n    x += 2\n    x -= 1\n    x *= 3\n    x /= 2\n    x %= 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        for (stmt, op) in stmts[1..].iter().zip([BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Mod]) {
            match stmt {
                Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                    assert!(matches!(&**value, Expr::Binary { op: o, .. } if *o == op));
                }
                other => panic!("expected compound-assign desugared stmt, got {other:?}"),
            }
        }
    }

    #[test]
    fn accepts_ampersand_str_type() {
        let p = parse("fun f(s: &str).return: &str = s");
        assert_eq!(p.functions[0].params[0].ty.name, "str");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "str");
    }

    #[test]
    fn parses_a_move_parameter() {
        let p = parse("fun consume(s: move String).return: usize = s.len()");
        let param = &p.functions[0].params[0];
        assert!(param.mode.moves());
        assert!(!param.mode.mutable());
        assert_eq!(param.ty.name, "String");
    }

    #[test]
    fn parses_a_function_type_in_parameter_and_return_position() {
        let p = parse("fun apply(f: fun(i32).return: i32, x: i32).return: fun(move String) = f");
        let f = &p.functions[0];
        let param = &f.params[0];
        assert!(param.ty.is_fn());
        let params = param.ty.fn_params.as_ref().unwrap();
        assert_eq!(params.len(), 1);
        assert!(!params[0].mode.mutable());
        assert_eq!(params[0].ty.name, "i32");
        assert_eq!(param.ty.fn_ret.as_ref().unwrap().name, "i32");

        // The return type is a function type with no `.return`, so it returns
        // `Unit` — and its parameter is a `move`, which a function type
        // spells the same way a declaration does.
        let ret = f.return_type.as_ref().unwrap();
        assert!(ret.is_fn());
        assert!(ret.fn_ret.is_none());
        assert!(ret.fn_params.as_ref().unwrap()[0].mode.moves());
        assert_eq!(ret.display(), "fun(move String)");
    }

    #[test]
    fn parses_a_closure_type_in_parameter_and_return_position() {
        // A closure type is a function type under the `closure` keyword — the
        // same parameter list, the same optional `.return:` (A-4 phase E).
        let p = parse("fun use(f: closure(i32).return: i32, g: move closure()) {}");
        let f = &p.functions[0];

        let f_param = &f.params[0];
        assert!(f_param.ty.is_closure());
        assert_eq!(f_param.ty.display(), "closure(i32).return: i32");
        assert!(!f_param.mode.moves());

        // A bare closure parameter borrows the environment (an owned type's
        // parameter is a borrow); `move closure(...)` takes it over.
        let g_param = &f.params[1];
        assert!(g_param.ty.is_closure());
        assert!(g_param.mode.moves());
        assert_eq!(g_param.ty.display(), "closure()");
    }

    #[test]
    fn parses_a_fun_literal_in_expression_position() {
        let p = parse("fun main() {\n    val f = fun(x: move String).return: usize = x.len()\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let {
                init: Expr::FnLit { params, return_type, .. },
                ..
            } => {
                assert_eq!(params.len(), 1);
                assert!(params[0].mode.moves());
                assert_eq!(return_type.as_ref().unwrap().name, "usize");
            }
            other => panic!("expected a fun literal, got {other:?}"),
        }
    }

    #[test]
    fn a_fun_literal_needs_a_body() {
        parse_err("fun main() {\n    val f = fun(x: i32).return: i32\n}");
    }

    #[test]
    fn rejects_move_combined_with_mut_or_a_receiver() {
        // One parameter cannot both borrow exclusively and take ownership.
        parse_err("fun f(s: mut move String) {}");
        // `move` is a parameter qualifier, never a receiver one.
        parse_err("fun move Counter.bump() {}");
    }

    #[test]
    fn rejects_malformed_input() {
        parse_err("fun main( {}");
    }

    #[test]
    fn parses_ref_deref_and_mut_param() {
        let p = parse("fun bump(c: mut Counter) {\n    val r = &c\n    val m = &mut c\n    val v = *r\n}");
        assert!(p.functions[0].params[0].mode.mutable());
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Ref { mutable: false, .. }, .. } => {}
            other => panic!("expected shared ref, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init: Expr::Ref { mutable: true, .. }, .. } => {}
            other => panic!("expected exclusive ref, got {other:?}"),
        }
        match &stmts[2] {
            Stmt::Let { init: Expr::Deref { .. }, .. } => {}
            other => panic!("expected deref, got {other:?}"),
        }
    }

    #[test]
    fn ampersand_mut_is_rejected_in_param_position() {
        parse_err("fun bump(c: &mut Counter) {}");
    }

    #[test]
    fn parses_unary_operators() {
        let p = parse("fun main() {\n    val x = -1\n    val y = !true\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Neg, .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Not, .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_impl_lines_before_decls() {
        let p = parse("impl \"shapes/circle\"\nimpl \"std/time\" caps(time = [\"/7d\"])\n\nfun main() {}");
        assert_eq!(p.impls.len(), 2);
        assert_eq!(p.impls[0].path, "shapes/circle");
        assert_eq!(p.impls[1].path, "std/time");
        assert_eq!(p.impls[1].caps[0].domain, "time");
        assert_eq!(p.functions.len(), 1);
    }

    #[test]
    fn the_impl_as_clause_is_refused_by_name() {
        let err = parse_err("impl \"time\" as t\n\nfun main() {}");
        assert!(
            err.message.contains("is gone: reach the module by its own name"),
            "{}",
            err.message
        );
    }

    #[test]
    fn parses_module_qualified_type_names() {
        let p = parse("fun f(c: circle.Circle).return: circle.Circle = c");
        assert_eq!(p.functions[0].params[0].ty.name, "circle.Circle");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "circle.Circle");
    }

    #[test]
    fn impl_after_a_declaration_is_rejected() {
        // Canonical order is impl, pd, then datac/fun.
        let e = parse_err("fun main() {}\nimpl \"util\"");
        assert!(e.message.contains("must come first"), "got: {}", e.message);
    }

    #[test]
    fn parses_a_caps_clause_with_several_grants() {
        let p = parse("impl \"util\" caps(fs = [\"/tmp/**\", \"/var/log/*\"], env = [\"HOME\"])");
        assert_eq!(p.impls.len(), 1);
        let caps = &p.impls[0].caps;
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].domain, "fs");
        assert_eq!(caps[0].patterns, vec!["/tmp/**", "/var/log/*"]);
        assert_eq!(caps[1].domain, "env");
        assert_eq!(caps[1].patterns, vec!["HOME"]);
    }

    #[test]
    fn the_with_caps_spelling_is_refused_by_name() {
        // Removed, not outdated: the clause is the same one under the surviving
        // spelling, and the refusal names the keyword to delete.
        let err = parse_err("impl \"util\" with caps(fs = [\"/tmp/**\"])");
        assert!(
            err.message.contains("is gone: drop the `with`"),
            "{}",
            err.message
        );
        let p = parse("impl \"util\" caps(fs = [\"/tmp/**\"])");
        assert_eq!(p.impls[0].caps[0].domain, "fs");
    }

    #[test]
    fn a_caps_clause_requires_bracketed_string_patterns() {
        parse_err("impl \"util\" caps(fs = [tmp])");
    }

    #[test]
    fn an_impl_without_caps_has_no_grants() {
        let p = parse("impl \"std/fs\"");
        assert!(p.impls[0].caps.is_empty());
        assert!(!p.impls[0].caps_all);
    }

    #[test]
    fn parses_the_caps_wildcard_clause() {
        let p = parse("impl \"helper\" caps(*)");
        assert!(p.impls[0].caps.is_empty());
        assert!(p.impls[0].caps_all);
    }

    #[test]
    fn a_named_domain_beside_the_wildcard_is_refused() {
        // Either order: `*` already means every domain, so a domain named
        // beside it has nothing left to say, and the refusal names both
        // surviving spellings.
        for source in [
            "impl \"helper\" caps(*, fs = [\"/tmp/**\"])",
            "impl \"helper\" caps(fs = [\"/tmp/**\"], *)",
        ] {
            let err = parse_err(source);
            assert!(
                err.message.contains("already means every domain"),
                "{source}: {}",
                err.message
            );
            assert!(err.message.contains("caps(*)` alone"), "{source}: {}", err.message);
        }
    }

    #[test]
    fn impl_requires_a_string_path() {
        parse_err("impl util");
    }

    #[test]
    fn parses_passdown_declarations() {
        let p = parse("pd val app_name = \"shapes\"\npd var count = 0\npd val label: &str = app_name\n\nfun main() {}");
        assert_eq!(p.pds.len(), 3);
        assert_eq!(p.pds[0].name, "app_name");
        assert!(!p.pds[0].mutable);
        assert!(p.pds[0].ty.is_none());
        assert!(p.pds[1].mutable);
        assert_eq!(p.pds[2].ty.as_ref().unwrap().name, "str");
        assert!(matches!(p.pds[2].init, Expr::Ident(..)));
    }

    #[test]
    fn pd_after_a_declaration_is_rejected() {
        let e = parse_err("fun main() {}\npd val x = 1");
        assert!(e.message.contains("must come before"), "got: {}", e.message);
    }

    #[test]
    fn pd_requires_val_or_var() {
        parse_err("pd x = 1");
    }

    #[test]
    fn parses_a_template_into_text_and_expression_parts() {
        let p = parse("fun main() {\n    print(\"a $b ${c.d()}\")\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let call = match &stmts[0] {
            Stmt::Expr { expr: Expr::Call { args, .. }, .. } => &args[0].value,
            other => panic!("expected a call, got {other:?}"),
        };
        let parts = match call {
            Expr::Template { parts, .. } => parts,
            other => panic!("expected a template, got {other:?}"),
        };
        assert_eq!(parts.len(), 4);
        assert!(matches!(&parts[0], TemplatePart::Text(t) if t == "a "));
        assert!(matches!(&parts[1], TemplatePart::Expr(Expr::Ident(n, _)) if n == "b"));
        assert!(matches!(&parts[2], TemplatePart::Text(t) if t == " "));
        assert!(matches!(&parts[3], TemplatePart::Expr(Expr::MethodCall { method, .. }) if method == "d"));
    }

    #[test]
    fn pub_is_read_on_the_five_item_forms_that_can_carry_it() {
        let p = parse(
            "pub const LIMIT: i32 = 1\npub datac Point(val x: i32)\npub enum Shape { Empty }\npub fun f().return: i32 = 1\npub extern fun g().return: i32",
        );
        assert!(p.consts[0].is_pub);
        assert!(p.datacs[0].is_pub);
        assert!(p.enums[0].is_pub);
        assert!(p.functions[0].is_pub);
        assert!(p.functions[1].is_pub);
        assert!(p.functions[1].extern_);
    }

    #[test]
    fn extern_type_reads_its_name_and_its_pub() {
        let p = parse("extern type Widget\npub extern type Buffer");
        assert_eq!(p.extern_types.len(), 2);
        assert_eq!(p.extern_types[0].name, "Widget");
        assert!(p.extern_types[0].parent.is_none());
        assert!(!p.extern_types[0].is_pub);
        assert_eq!(p.extern_types[1].name, "Buffer");
        assert!(p.extern_types[1].is_pub);
    }

    #[test]
    fn extern_type_reads_an_optional_parent() {
        let p = parse("extern type Window: Widget\nextern type Dialog: mem.Panel");
        assert_eq!(p.extern_types[0].parent.as_deref(), Some("Widget"));
        assert_eq!(p.extern_types[1].parent.as_deref(), Some("mem.Panel"));
    }

    #[test]
    fn extern_type_takes_no_body_or_fields() {
        let e = parse_err("extern type Widget {}");
        assert!(e.message.contains("expected 'fun'"), "got: {}", e.message);
    }

    #[test]
    fn an_item_without_pub_is_private() {
        let p = parse("fun f().return: i32 = 1");
        assert!(!p.functions[0].is_pub);
    }

    #[test]
    fn pub_pd_is_refused_by_name() {
        let e = parse_err("pub pd val x = 1\nfun main() {}");
        assert!(e.message.contains("a 'pd' cannot be 'pub'"), "got: {}", e.message);
    }

    #[test]
    fn pub_on_a_receiver_method_names_the_type() {
        let e = parse_err("pub fun Circle.area().return: f64 = 1.0");
        assert!(
            e.message.contains("mark the type 'pub' instead"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn pub_on_anything_else_names_what_it_applies_to() {
        let e = parse_err("pub impl \"util\"");
        assert!(
            e.message.contains("'pub' applies to a top-level 'fun'"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn pub_inside_a_block_names_the_top_level_forms() {
        let e = parse_err("fun main() {\n    pub val x = 1\n}");
        assert!(
            e.message.contains("and only at the top level of a file"),
            "got: {}",
            e.message
        );
    }

    fn body_stmts(p: &Program) -> &Vec<Stmt> {
        match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        }
    }

    fn first_call_args(src: &str) -> Vec<CallArg> {
        let p = parse(src);
        match &body_stmts(&p)[0] {
            Stmt::Expr { expr: Expr::Call { args, .. } | Expr::MethodCall { args, .. }, .. } => args.clone(),
            other => panic!("expected a call statement, got {other:?}"),
        }
    }

    #[test]
    fn a_trailing_block_is_the_last_argument_as_a_unit_fun_literal() {
        let args = first_call_args("fun main() {\n    run(1) { work() }\n}");
        assert_eq!(args.len(), 2);
        assert!(matches!(args[1].value, Expr::FnLit { ref params, return_type: None, .. } if params.is_empty()));
        assert!(args[1].name.is_none());
    }

    #[test]
    fn a_trailing_block_needs_no_parentheses_when_it_is_the_only_argument() {
        let args = first_call_args("fun main() {\n    column { work() }\n}");
        assert_eq!(args.len(), 1);
        let args = first_call_args("fun main() {\n    ui.column { work() }\n}");
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn a_block_on_the_next_line_is_not_a_trailing_block() {
        // Two statements: the call with one argument, then a block of its own.
        let p = parse("fun main() {\n    run(1)\n    { work() }\n}");
        assert_eq!(body_stmts(&p).len(), 2);
    }

    #[test]
    fn a_control_flow_head_keeps_its_brace_as_the_body() {
        // `f(x)` and `xs.len()` are calls ending a head: the `{` is the body.
        parse("fun main() {\n    if f(x) { work() }\n    while g() { work() }\n    for a in xs.items() { work() }\n}");
        parse("fun main() {\n    if cond { work() } else { other() }\n    val v = when x.pick() { A -> 1 }\n}");
    }

    #[test]
    fn parentheses_and_blocks_reenable_a_trailing_block_inside_a_head() {
        parse("fun main() {\n    if f(run { work() }) { work() }\n    if cond { run { work() } }\n}");
    }

    #[test]
    fn an_unsafe_block_is_an_expression_and_a_statement() {
        // Stage n (MK9-S2): `unsafe` is a contextual word, a block introducer
        // exactly when a `{` follows it.
        let p = parse("fun main() {\n    unsafe {\n        p.write(1)\n    }\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Expr { expr: Expr::Unsafe { block, .. }, .. } => {
                    assert_eq!(block.stmts.len(), 1);
                }
                other => panic!("expected an unsafe block statement, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }
        // As an expression, with a value.
        parse("fun main() {\n    val v = unsafe { p.read() }\n}");
    }

    #[test]
    fn unsafe_is_still_an_ordinary_name_away_from_a_brace() {
        let p = parse("fun main() {\n    val unsafe = 1\n    print(unsafe)\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { name, .. } => assert_eq!(name, "unsafe"),
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }
    }

    #[test]
    fn a_block_header_declares_its_parameters_by_name() {
        let args = first_call_args("fun main() {\n    column { x, y -> f(x, y) }\n}");
        assert_eq!(args.len(), 1);
        match &args[0].value {
            Expr::FnLit { params, infer_params, return_type: None, .. } => {
                assert!(*infer_params);
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, "x");
                assert_eq!(params[1].name, "y");
                // No type is written on a block header: it comes from the
                // function type the block is passed to.
                assert_eq!(params[0].ty.name, "");
            }
            other => panic!("expected a block literal, got {other:?}"),
        }
    }

    #[test]
    fn an_unsafe_fun_declaration_sets_its_flag() {
        let p = parse("unsafe fun poke(p: Ptr<i32>, v: i32) {\n    p.write(v)\n}\nfun main() {}");
        assert!(p.functions[0].unsafe_);
        assert!(!p.functions[1].unsafe_);
        assert_eq!(p.functions[0].params[0].ty.display(), "Ptr<i32>");
    }

    #[test]
    fn a_block_whose_first_tokens_are_not_a_header_is_an_ordinary_block() {
        // `x` alone, and `x + 1`, are expressions, not parameter headers.
        let args = first_call_args("fun main() {\n    run { x }\n}");
        assert!(matches!(&args[0].value, Expr::FnLit { params, .. } if params.is_empty()));
        parse("fun main() {\n    run { x + 1 }\n}");
    }

    #[test]
    fn a_scoped_function_parameter_type_parses_as_a_modifier() {
        parse("fun each(f: scoped fun(i32)) {}\n");
        parse("fun each(f: scoped fun(i32).return: i32) {}\n");
        // `scoped` is contextual: as an ordinary type name it is untouched.
        parse("datac scoped(val n: i32)\nfun f(x: scoped) {}\n");
    }

    #[test]
    fn an_asm_expression_parses_its_operands_in_order() {
        let p = parse(
            "fun main() {\n    asm(\"add {0}, {1}\", inout(\"r\") x, in(\"r\") y, clobber(\"rax\"), nomem)\n}",
        );
        let stmts = body_stmts(&p);
        let Stmt::Expr { expr, .. } = &stmts[0] else {
            panic!("expected an asm statement");
        };
        let Expr::Asm {
            template,
            operands,
            clobbers,
            nomem,
            nostack,
            ..
        } = expr
        else {
            panic!("expected an asm statement");
        };
        assert_eq!(template, "add {0}, {1}");
        assert_eq!(operands.len(), 2);
        assert_eq!(operands[0].kind, AsmOperandKind::InOut);
        assert_eq!(operands[0].constraint, "r");
        assert!(operands[0].operand.is_some());
        assert_eq!(operands[1].kind, AsmOperandKind::In);
        assert_eq!(clobbers, &vec!["rax".to_string()]);
        assert!(*nomem);
        assert!(!*nostack);
    }

    #[test]
    fn an_anonymous_out_operand_parses_as_no_expression() {
        let p = parse("fun main() {\n    val v: i64 = asm(\"mov {0}, 1\", out(\"r\") _)\n}");
        let stmts = body_stmts(&p);
        let Stmt::Let { init, .. } = &stmts[0] else {
            panic!("expected a let statement");
        };
        let Expr::Asm { operands, .. } = init else {
            panic!("expected an asm expression");
        };
        assert_eq!(operands[0].kind, AsmOperandKind::Out);
        assert!(operands[0].operand.is_none());
    }

    #[test]
    fn an_unknown_asm_operand_kind_is_refused_by_name() {
        let e = parse_err("fun main() {\n    asm(\"nop\", sideways(\"r\") 1)\n}");
        assert!(
            e.message.contains("unknown asm operand kind 'sideways'"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn a_naked_fun_declaration_sets_its_flag() {
        let p = parse("naked fun f().return: i64 {\n    asm(\"ret\")\n}\nfun main() {}");
        assert!(p.functions[0].naked);
        assert!(!p.functions[0].unsafe_);
        assert!(!p.functions[1].naked);
    }

    #[test]
    fn naked_and_unsafe_can_be_written_together() {
        let p = parse("pub unsafe naked fun f().return: i64 {\n    asm(\"ret\")\n}\nfun main() {}");
        assert!(p.functions[0].naked);
        assert!(p.functions[0].unsafe_);
        assert!(p.functions[0].is_pub);
    }

    /// Kernel stage 6: `defer EXPR` is a statement, and a block or a call are
    /// both expressions. `defer.rs` is what consumes the form, so the parser
    /// only has to record it with the action's own span stitched on.
    #[test]
    fn parses_defer_with_a_call_and_with_a_block() {
        let p = parse(
            "fun main() {\n    defer unlock()\n    defer {\n        print(\"a\")\n        print(\"b\")\n    }\n}",
        );
        let body = match &p.functions[0].body {
            FunctionBody::Block(b) => b,
            _ => panic!("a body block"),
        };
        let Stmt::Defer { expr, span } = &body.stmts[0] else {
            panic!("a defer statement");
        };
        assert!(matches!(expr, Expr::Call { callee, .. } if callee == "unlock"));
        assert!(span.end > span.start);
        let Stmt::Defer { expr, .. } = &body.stmts[1] else {
            panic!("a defer statement");
        };
        assert!(matches!(expr, Expr::Block(b) if b.stmts.len() == 2));
    }

    /// `break` and `continue` are not keywords (`plan/syntax.md` 2.3), so a
    /// bare one is the loop control statement (`plan/iterators.md`) and a name
    /// used in an expression is still a name.
    #[test]
    fn break_and_continue_stay_ordinary_names_outside_statement_position() {
        let p = parse("fun main() {\n    val break = 2\n    print(break)\n}");
        assert_eq!(p.functions.len(), 1);
        let p = parse("fun main() {\n    val continue = 3\n    print(continue + 1)\n}");
        assert_eq!(p.functions.len(), 1);
    }
}

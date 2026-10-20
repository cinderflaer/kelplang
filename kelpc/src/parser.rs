use crate::ast::*;
use crate::foreign;
use crate::lexer::{Span, Token, TokenKind};

pub struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
    /// How many `if let`s have been desugared, so each one's synthesized
    /// scrutinee binding has a name of its own (`parser::IF_LET_PREFIX`).
    pub(crate) if_lets: usize,
    /// How many `for (a, b) in ...` heads have been desugared, so each one's
    /// hidden element binding has a name of its own (`FOR_PATTERN_PREFIX`).
    pub(crate) for_patterns: usize,
    /// The loops the statement being parsed sits inside, innermost last, so a
    /// `break 'outer` can find its target. A function, a `fun` literal and a
    /// trailing block start a fresh stack: a label does not cross them.
    pub(crate) loop_stack: Vec<LoopCtx>,
    /// How many labelled exits have needed a flag (`LABEL_FLAG_PREFIX`).
    pub(crate) label_flags: usize,
    /// How many tuple `when`s and variant levels have been rewritten, so each
    /// hidden name is its own (`WHEN_TUPLE_PREFIX`).
    pub(crate) when_temps: usize,
    /// True while parsing the head of an `if`/`elif`/`while`/`for`/`when`, where
    /// a `{` after an expression opens the body and not a trailing block
    /// (`kelp issue #242`). A call's argument list and a block reset it.
    pub(crate) no_trailing_block: bool,
    /// How many nesting constructs (expression, type, block, pattern) the parser
    /// is inside right now; `nest` refuses to go past `MAX_NESTING`.
    pub(crate) depth: usize,
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
    /// The next step a reader can take, carried onto the `Diagnostic`.
    pub help: Option<String>,
}

type PResult<T> = Result<T, ParseError>;

/// One entry of a parameter list `T`, `T: A + B`, or `const N: usize`
/// (A-2, A-20). The const form is a *value* parameter; every other entry is a
/// type parameter.
pub(crate) struct TypeParamParsed {
    pub(crate) name: String,
    pub(crate) bounds: Vec<TypeExpr>,
    /// `Some(ty)` for `const N: ty`, and then `bounds` is empty.
    pub(crate) const_ty: Option<String>,
    pub(crate) span: Span,
}

/// The refusal for a `*` that does not stand alone in a `caps(...)` clause
/// (`caps(*, fs = [...])` and `caps(fs = [...], *)`). `*` already means every
/// domain, so a named domain beside it has nothing left to say; the message
/// names the spelling that does mean what the writer wanted.
fn mixing_wildcard_error(span: Span) -> ParseError {
    ParseError {
        help: None,
        message: "`caps(*)` already means every domain, so nothing can stand beside the `*`: \
                  write `caps(*)` alone, or name the domains it needs \
                  (`caps(net = [\"host:port\"])`, `caps(fs = [\"/a/**\"], env = [\"HOME\"])`)"
            .to_string(),
        span,
    }
}

/// The name prefix of the local an `if let` binds its scrutinee to. `#` cannot
/// appear in a lexed identifier, so a synthesized name can never collide with
/// one the user wrote - the same trick `infer.rs` uses for a hidden passdown
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
                    help: None,
                    message: "a trait method cannot write 'impl Trait': its signature names 'Self' and the trait's own types, and a method takes no type parameters of its own".to_string(),
                    span,
                });
            }
            if is_method {
                return Err(ParseError {
                    help: None,
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
                help: None,
                message: "'impl Trait' is only legal as a whole parameter type: write it bare ('x: impl Trait') or behind a borrow ('x: &impl Trait'), not nested inside another type".to_string(),
                span,
            });
        }
        out.push(p);
    }
    Ok((out, type_params, type_param_bounds))
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
pub(crate) struct LoopCtx {
    pub(crate) label: Option<String>,
    pub(crate) flag: Option<String>,
    pub(crate) exits: Vec<usize>,
    /// For a `loop` used as a value: the hidden list its `break value`s push
    /// to, and how many of them there are.
    pub(crate) value_slot: Option<String>,
    pub(crate) valued_breaks: usize,
}

/// One position of a tuple pattern in a `when` arm.
#[derive(Debug, Clone)]
pub(crate) enum TuplePat {
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
pub(crate) struct TupleRow {
    pub(crate) pats: Vec<TuplePat>,
    pub(crate) binds: Vec<(String, Expr)>,
    pub(crate) body: Expr,
    pub(crate) span: Span,
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;

mod parser_tail;
pub(crate) use parser_tail::*;
mod parser_part7;
mod parser_part6;
mod parser_part5;
mod parser_part4;
mod parser_part3;
mod parser_part2;
mod parser_part1;

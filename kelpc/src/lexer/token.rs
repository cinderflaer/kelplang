use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span::new(self.start, other.end)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // literals
    /// A decimal, hexadecimal or binary integer literal, as the magnitude the
    /// source wrote. The payload is `u64` rather than `i64` because the range a
    /// literal may spell is the widest integer type's: `0xFFFFFFFFFFFFFFFF` is
    /// a legal `u64` literal, and a negative value is the negation of one of
    /// these (MISSING-x8Wq3). The *type* is still not decided here; `infer.rs`
    /// gives the literal the integer type its context requires and refuses one
    /// that does not fit.
    IntLit(u64),
    FloatLit(f64),
    /// A string with no `$` interpolation in it, stored decoded (escapes
    /// already processed). A string *with* interpolation becomes the
    /// `TemplateStart` ... `TemplateEnd` sequence below instead, so every
    /// existing string in every existing test keeps this exact token.
    StringLit(String),
    /// `'a'`, `'\n'`, `'\u{4e2d}'` - one Unicode scalar value, stored as its
    /// code point (open item 53). Distinct from `IntLit` at the token and
    /// AST levels so the parser and the type checker can tell a character
    /// from a number; `infer.rs` is what decides the type, and a character
    /// literal takes the integer type its context asks for, which is what
    /// makes `c == '"'` work when `c` is a `u8` byte.
    CharLit(u32),
    /// `'outer`: a loop label, written before a loop (`'outer: for ...`) and
    /// after `break`/`continue`. It is a `'` and a name with no closing `'`,
    /// which is what tells it from the character literal `'o'`.
    Label(String),
    /// `b'x'`, `b'\n'`, `b'\xff'`: one byte, and a `u8` by construction
    /// (H-11). Distinct from `CharLit` because the two differ in exactly the
    /// places a byte-level program cares about: a byte literal never needs
    /// the type written on a binding (`val nl = b'\n'` is a `u8`), it is
    /// refused where a wider type is asked for rather than widened, and
    /// `b'\xff'` is the byte 255 while `'\xff'` is the code point U+00FF.
    ByteLit(u8),
    Ident(String),
    /// `// ...` up to (not including) the line terminator, or `/* ... */`,
    /// both with their delimiters. Produced only by
    /// [`Lexer::tokenize_with_trivia`]: [`Lexer::tokenize`] skips comments, so
    /// the parser, the preprocess pass and every existing diagnostic never see
    /// one. The variant exists for the formatter, which has to reproduce every
    /// comment verbatim and cannot do that from an AST with no comment node.
    /// A block comment's text is stored exactly as written, newlines included.
    Comment(String),

    // string-template pieces (`"a$b${c}"`), all emitted in one flat run:
    // `TemplateStart StrChunk("a") ExprStart Ident(b) ExprEnd
    //  StrChunk("") ExprStart <expr tokens> ExprEnd StrChunk("") TemplateEnd`.
    // Splitting templates in the lexer is what keeps the sub-expressions'
    // spans *absolute* (no offset arithmetic to get `file:line:col` right)
    // and lets a template nest inside its own `${...}`.
    TemplateStart,
    /// Literal text between interpolations. Always present, possibly empty,
    /// and always one before `TemplateEnd`, so the shape is regular.
    StrChunk(String),
    TemplateExprStart,
    TemplateExprEnd,
    TemplateEnd,

    // keywords
    Fun,
    /// `closure(i32) => i32` - the type of a `fun` literal that captures
    /// (A-4, phase E): a function *value* that carries an environment, so it
    /// is owned (not Copy) and drops that environment. A keyword rather than
    /// a contextual name, for the reason `fun` is one: the spelling is a type
    /// constructor, and a program that named a variable `closure` would
    /// shadow a type name.
    Closure,
    Val,
    Var,
    Mut,
    /// `fun consume(s: move String)` - a parameter that takes ownership of
    /// its argument (A-4), the counterpart of `mut T`'s exclusive borrow.
    Move,
    If,
    Else,
    /// `elif cond { ... }` - the `else if` link spelled as one keyword. It
    /// means exactly what `else if` means (`design/syntax.md (kelp wiki)` 6.1 records both
    /// spellings, with `else if` the preferred one), so the parser hands it to
    /// the same function the `else if` link goes to. `kelpc lint` reads neither
    /// spelling.
    Elif,
    While,
    /// `for x in xs { ... }` - the indexed walk over a `List` (A-5,
    /// `design/syntax.md (kelp wiki)` §6.2). The parser desugars it to a `while`, so no pass
    /// after the parser knows this form exists. `in` is **not** a keyword; it
    /// is read contextually, exactly as `let` is after `if`.
    For,
    When,
    /// `defer EXPR` - a statement whose action runs when the enclosing block
    /// is left (kernel stage 6, `kelp wiki page language/statics-and-defer.md`). `defer.rs` rewrites it away
    /// before name resolution, so no later pass knows this form exists.
    Defer,
    /// `errdefer EXPR`: a `defer` that runs only on an error exit
    /// (`design/syntax.md (kelp wiki)`). `defer.rs` rewrites it away before name resolution.
    Errdefer,
    Return,
    True,
    False,
    Datac,
    /// `enum Shape { Circle(val r: f64) Empty }` - a tagged union
    /// (`design/syntax.md (kelp wiki)` §9.2): exactly one variant is live, and `when` on a
    /// scrutinee matches it (A-1).
    Enum,
    /// `pd val name = expr` / `pd var name = expr` - a passdown
    /// (`design/syntax.md (kelp wiki)` §9).
    Pd,
    /// `tl val name = literal` / `tl var name = literal` - thread-local storage:
    /// each thread has its own copy, initialized to the literal.
    Tl,
    /// `impl "path/to/module"`: the import keyword (`design/syntax.md` (kelp wiki) §8),
    /// and `impl Trait for Type { ... }`: the trait-implementation form
    /// (A-12 slice 2). The parser tells them apart by the token that follows:
    /// a string literal is an import and an identifier is an impl block.
    Impl,
    /// `trait Name { fun Self.m(...) ... }`: a named set of methods a
    /// `datac`/`enum` or a builtin implements (A-12 slices 2 and 3): a method
    /// with no body is a signature, one with a body is a default, and the
    /// optional `: A + B` names supertraits (`design/syntax.md (kelp wiki)` 9.5).
    Trait,
    /// The `as` keyword. Nothing accepts it: `impl "path" as name` was removed
    /// (`design/syntax.md (kelp wiki)` 13.1), and the `impl` parser refuses the clause by name
    /// rather than letting a stale one read as something else.
    As,
    /// `const NAME: T = expr` - a top-level compile-time constant (M6.3).
    Const,
    /// `static NAME: T = expr` - a top-level global object (kernel stage 7).
    /// Unlike a `const`, one with a stable address.
    Static,
    /// `extern fun name(...) => T` - a declaration whose body the compiler
    /// supplies (M6.2). Only compiler-shipped stdlib modules may use it: the
    /// whole point is to confine the unportable primitive (a syscall, a
    /// `malloc`) to a body that is not `.kp` source.
    Extern,
    /// `pub`: a top-level item another module may reach (A-13,
    /// `design/syntax.md (kelp wiki)` 13.4). Items are private to their module by default;
    /// `pub` on a `fun`, `extern fun`, `datac`, `enum` or `const` exports it.
    Pub,

    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    /// `[`/`]`. Used by `caps(fs = ["..."])`, by the `[T; N]` fixed-size
    /// array type (A-18) and by its two literals (`[a, b, c]`, `[x; N]`).
    LBracket,
    RBracket,
    Comma,
    /// `;`, used in exactly one place: between an array's element type and its
    /// length (`[u8; 4096]`, `[x; N]`). Statements are newline-terminated, so
    /// the character had no meaning before A-18 and this is not a second
    /// statement separator.
    Semi,
    Colon,
    /// `::` is the associated-type projection (A-12 slice 5): `I::Item`,
    /// `Self::Item`. One token so a formatter can glue both sides the way it
    /// glues `.`, and two colons written apart stay two `Colon`s.
    ColonColon,
    Dot,
    /// `...` - the variadic marker a user `extern fun` may write as its last
    /// parameter (gap 7). Nothing follows it, and it is not a value: it only
    /// ever appears in a declaration's parameter list.
    DotDotDot,
    Arrow, // -> or => (`when` arms, block parameters)
    Newline,

    // operators
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AndAnd,
    OrOr,
    Bang,
    Amp,
    /// `a | b` - bitwise or (A-9). `||` is still one token; a single `|` used
    /// to be a lex error.
    Pipe,
    /// `a ^ b` - bitwise exclusive or.
    Caret,
    /// `~a` - bitwise complement, prefix only.
    Tilde,
    /// `expr?` - propagate a failure out of the enclosing function
    /// (MK5-S6-M1). Postfix, so it is only ever read after an expression.
    Question,

    Eof,
}

impl TokenKind {
    /// The token as a reader would write it in the source, for a parse error
    /// (`found ';'`, `found 'fun'`, `found end of file`). A literal or a name
    /// shows its text, so a message never prints a Rust debug name.
    pub fn describe(&self) -> String {
        let text = match self {
            TokenKind::IntLit(n) => return n.to_string(),
            TokenKind::FloatLit(f) => return f.to_string(),
            TokenKind::StringLit(s) => return format!("{s:?}"),
            TokenKind::CharLit(c) => {
                return match char::from_u32(*c) {
                    Some(ch) => format!("'{ch}'"),
                    None => format!("character {c:#x}"),
                };
            }
            TokenKind::ByteLit(b) => return format!("b'{}'", *b as char),
            TokenKind::Label(name) => return format!("'{name}"),
            TokenKind::Ident(name) => return format!("'{name}'"),
            TokenKind::Comment(_) => "a comment",
            TokenKind::TemplateStart | TokenKind::TemplateEnd => "a string template",
            TokenKind::StrChunk(_) => "text in a string template",
            TokenKind::TemplateExprStart => "'${'",
            TokenKind::TemplateExprEnd => "'}'",
            TokenKind::Fun => "'fun'",
            TokenKind::Closure => "'closure'",
            TokenKind::Val => "'val'",
            TokenKind::Var => "'var'",
            TokenKind::Mut => "'mut'",
            TokenKind::Move => "'move'",
            TokenKind::If => "'if'",
            TokenKind::Else => "'else'",
            TokenKind::Elif => "'elif'",
            TokenKind::While => "'while'",
            TokenKind::For => "'for'",
            TokenKind::When => "'when'",
            TokenKind::Defer => "'defer'",
            TokenKind::Errdefer => "'errdefer'",
            TokenKind::Return => "'return'",
            TokenKind::True => "'true'",
            TokenKind::False => "'false'",
            TokenKind::Datac => "'datac'",
            TokenKind::Enum => "'enum'",
            TokenKind::Pd => "'pd'",
            TokenKind::Tl => "'tl'",
            TokenKind::Impl => "'impl'",
            TokenKind::Trait => "'trait'",
            TokenKind::As => "'as'",
            TokenKind::Const => "'const'",
            TokenKind::Static => "'static'",
            TokenKind::Extern => "'extern'",
            TokenKind::Pub => "'pub'",
            TokenKind::LParen => "'('",
            TokenKind::RParen => "')'",
            TokenKind::LBrace => "'{'",
            TokenKind::RBrace => "'}'",
            TokenKind::LBracket => "'['",
            TokenKind::RBracket => "']'",
            TokenKind::Comma => "','",
            TokenKind::Semi => "';'",
            TokenKind::Colon => "':'",
            TokenKind::ColonColon => "'::'",
            TokenKind::Dot => "'.'",
            TokenKind::DotDotDot => "'...'",
            TokenKind::Arrow => "'=>'",
            TokenKind::Newline => "end of line",
            TokenKind::Eq => "'='",
            TokenKind::EqEq => "'=='",
            TokenKind::NotEq => "'!='",
            TokenKind::Lt => "'<'",
            TokenKind::LtEq => "'<='",
            TokenKind::Gt => "'>'",
            TokenKind::GtEq => "'>='",
            TokenKind::Plus => "'+'",
            TokenKind::Minus => "'-'",
            TokenKind::Star => "'*'",
            TokenKind::Slash => "'/'",
            TokenKind::Percent => "'%'",
            TokenKind::PlusEq => "'+='",
            TokenKind::MinusEq => "'-='",
            TokenKind::StarEq => "'*='",
            TokenKind::SlashEq => "'/='",
            TokenKind::PercentEq => "'%='",
            TokenKind::AndAnd => "'&&'",
            TokenKind::OrOr => "'||'",
            TokenKind::Bang => "'!'",
            TokenKind::Amp => "'&'",
            TokenKind::Pipe => "'|'",
            TokenKind::Caret => "'^'",
            TokenKind::Tilde => "'~'",
            TokenKind::Question => "'?'",
            TokenKind::Eof => "end of file",
        };
        text.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer<'a> {
    pub(crate) src: &'a str,
    pub(crate) bytes: &'a [u8],
    pub(crate) pos: usize,
    /// Depth of unclosed `(`. Newlines are insignificant (not tokenized)
    /// while this is > 0 - this is what lets a `datac`'s field-list parens,
    /// or any parenthesized sub-expression, span multiple lines.
    pub(crate) paren_depth: usize,
    /// The `paren_depth` in force when each unclosed `{` was opened. A block
    /// *inside* parens is where the rule above stops: `{` opens a statement
    /// block, and statements are separated by newlines, so those are tokenized
    /// again however deep the parens are - which is what lets a `fun` literal
    /// with a multi-statement body be written as a call argument
    /// (`f(fun(x: i32) { a\n b\n })`). A function body's own braces are at
    /// paren depth 0, so a wrapped argument list inside it still ignores
    /// newlines (A-4 phase E).
    pub(crate) brace_parens: Vec<usize>,
    /// Tokens produced by a single source construct but consumed one at a
    /// time - only string templates use this: one `"..."` literal expands to
    /// a whole token run, and `next_token` drains the run before reading
    /// more source.
    pub(crate) pending: VecDeque<Token>,
    /// When true, `//...` is returned as a `TokenKind::Comment` token instead
    /// of being skipped, and a newline run is not allowed to swallow one. Off
    /// by default, so the token stream every other caller sees is unchanged;
    /// see [`Lexer::tokenize_with_trivia`].
    pub(crate) keep_comments: bool,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

/// Whether a line that ends in this token continues onto the next one: a binary
/// or assignment operator, a comma, a `.`, or a `when` arrow. `<` and `>` are
/// left out because they also close and open type argument lists, `%` because an
/// alias pattern (`_name`) ends in one, `&` and `|`
/// because a bare one can end a pattern or a type, and `?` because it is a
/// complete postfix.
pub(crate) fn continues_line(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::LtEq
            | TokenKind::GtEq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::Arrow
    )
}

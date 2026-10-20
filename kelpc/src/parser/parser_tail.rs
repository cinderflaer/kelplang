use super::*;

/// The foreign-word hint for a token kind: an identifier that is another
/// language's keyword or type name, or the `as` keyword.
pub(crate) fn foreign_hint_for(kind: &TokenKind) -> Option<String> {
match kind {
    TokenKind::Ident(name) => foreign::hint(name),
    TokenKind::As => foreign::hint("as"),
    _ => None,
}
}

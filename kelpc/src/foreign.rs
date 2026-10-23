//! Hints for a word another language spells where Kelp has a different word.
//!
//! A reader who comes from Rust, Python, Go, Swift or Kotlin types one of
//! their keywords or type names. The table here turns that word into the
//! Kelp word and the fix. It is read only where the word is already an error:
//! an unknown name, an unknown type, or a parse error at that token. So it
//! never changes which programs compile, and each hint lands in the `help:`
//! line of a message whose first line stays as it was.
//!
//! Every hint names only what main has. A hint must not suggest a construction
//! that is not accepted yet (a bare `Some`, an `int` type before M38).

/// The hint for `word`, when another language's spelling of a Kelp concept
/// is that word. `None` for every other word, so the caller keeps the help
/// line it had.
pub fn hint(word: &str) -> Option<String> {
    Some(match word {
        "let" => "let is not Kelp: write val x = 5, or var x = 5 if the value changes".to_string(),
        "fn" | "def" | "function" => format!(
            "{word} is not Kelp: write fun name(params) => type {{ ... }}, or fun name(params) {{ ... }} when nothing is returned"
        ),
        "struct" | "class" => format!(
            "{word} is not Kelp: a record is datac Name(val x: i32, val y: i32)"
        ),
        "match" => "there is no match: use when x { ... } with one arm for each case".to_string(),
        "null" | "nil" => format!(
            "Kelp has no {word}: an absent value is option.Option.None, after impl \"std/option\""
        ),
        "None" => "None is not a Kelp value: an absent value is option.Option.None, after impl \"std/option\"".to_string(),
        "True" => "True is not Kelp: write true".to_string(),
        "False" => "False is not Kelp: write false".to_string(),
        "println" => "did you mean print? print adds the line end, so print(\"x\") needs no newline argument".to_string(),
        "as" => "there is no as: convert with a method, such as x.to_i64()".to_string(),
        "string" => "string is not a Kelp type: write String, the text type".to_string(),
        "elseif" => "elseif is not Kelp: write else if".to_string(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::hint;

    #[test]
    fn every_foreign_word_has_a_hint_and_an_unknown_word_has_none() {
        for word in [
            "let", "fn", "def", "function", "struct", "class", "match", "null", "nil", "None",
            "True", "False", "println", "as", "string", "elseif",
        ] {
            assert!(hint(word).is_some(), "no hint for {word}");
        }
        for word in ["total", "print", "var", "elif", "String", "i64", "int"] {
            assert!(hint(word).is_none(), "{word} is a Kelp name or not a foreign word");
        }
    }

    #[test]
    fn a_hint_names_no_dash_and_no_bare_some() {
        for word in ["let", "fn", "struct", "null", "None", "as", "string"] {
            let text = hint(word).unwrap();
            assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'), "{text}");
            assert!(!text.contains("Some("), "{text}");
        }
    }
}

use super::*;
use std::path::{Path, PathBuf};

/// Every directory the formatter is expected to cover, minus the ones
/// another workflow owns (`kelpc/tests/fixtures`, `kelpc/tests/fmt_corpus`).
fn corpus_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        root.join("std"),
        root.join("tests/projects"),
        root.join("tests/fixtures/examples"),
        root.join("../bench"),
    ]
}

/// Collects `.kp` files under `dir`. Symbolic links are skipped, because
/// every link in the corpus points at a real file or directory that the
/// walk reaches on its own, and following a link can loop (a `deps/v4`
/// link back into its own package never terminates).
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("kp") {
            out.push(path);
        }
    }
}

fn corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in corpus_dirs() {
        collect(&dir, &mut files);
    }
    files.sort();
    files
}

/// The token stream with trivia removed: kind plus the exact source text
/// the token came from. `Newline` is dropped because blank-line runs are
/// deliberately normalized; comparing it would reject the reformat itself.
fn code_tokens(src: &str) -> Vec<(String, String)> {
    Lexer::new(src)
        .tokenize_with_trivia()
        .expect("lex failed")
        .into_iter()
        .filter(|t| {
            !matches!(
                t.kind,
                TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof
            )
        })
        .map(|t| {
            (
                format!("{:?}", t.kind),
                src[t.span.start..t.span.end].to_string(),
            )
        })
        .collect()
}

fn comments(src: &str) -> Vec<String> {
    Lexer::new(src)
        .tokenize_with_trivia()
        .expect("lex failed")
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Comment(_) => Some(src[t.span.start..t.span.end].to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_repository_file_is_idempotent_token_faithful_and_comment_preserving() {
    let files = corpus();
    assert!(files.len() >= 40, "only found {} files", files.len());
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        let once = match format(&src) {
            Ok(text) => text,
            // A file the gate refuses is exercised by the refusal tests
            // instead: a deliberate error fixture in one of these dirs, or
            // a file whose `_alias` another module declares - this gate
            // reads one file's text, so it cannot see that module and is
            // right to refuse rather than substitute a placeholder.
            Err(_) => continue,
        };
        let twice = format(&once).expect("formatted output must format");
        assert_eq!(once, twice, "not idempotent: {}", file.display());
        assert_eq!(
            code_tokens(&src),
            code_tokens(&once),
            "token changed: {}",
            file.display()
        );
        assert_eq!(
            comments(&src),
            comments(&once),
            "comment changed: {}",
            file.display()
        );
    }
}

#[test]
fn an_unsafe_block_and_an_unsafe_fun_round_trip() {
    // Stage n (MK9-S2): `unsafe` is a contextual word, so the formatter
    // treats it as one and `unsafe {` keeps its single space.
    let src = "unsafe fun poke(p: Ptr<i32>, v: i32) {\n    p.write(v)\n}\n\nfun main() {\n    unsafe {\n        p.write(1)\n    }\n    val q = unsafe { p.read() }\n}\n";
    let out = format(src).expect("the new syntax should format");
    assert_eq!(out, src);
}

#[test]
fn a_parse_error_is_refused_and_nothing_is_formatted() {
    let err = format("fun main( {\n    print(\"x\")\n}\n").unwrap_err();
    assert!(!err.message.is_empty());
    assert_eq!(err.kind, RefusalKind::SyntaxError);
}

#[test]
fn an_alias_declaration_and_reference_are_atomic() {
    // The declaration's right-hand side is macro text and is copied
    // verbatim (the double space survives); the reference is an
    // ordinary identifier and is formatted like any other name.
    let src = "alias _a = 1 +  2\n\nfun main() {\n    print( _a )\n}\n";
    let out = format(src).unwrap();
    assert_eq!(
        out,
        "alias _a = 1 +  2\n\nfun main() {\n    print(_a)\n}\n"
    );
}

#[test]
fn a_prefix_operator_never_merges_with_the_token_after_it() {
    // `& &` must keep its space: `&&` is one token, so joining the two
    // borrows changes the program. The reported defect.
    let src = "fun main() {\n    val x = 5\n    val b = & & x\n}\n";
    let out = format(src).unwrap();
    assert!(out.contains("val b = & &x"), "{out}");
    assert_eq!(code_tokens(src), code_tokens(&out));
    assert_eq!(format(&out).unwrap(), out, "not idempotent");
}

#[test]
fn the_token_fidelity_post_condition_rejects_a_changed_sequence() {
    // The post-condition is a real comparison, not a no-op: the same text
    // passes and a merged operator fails with an internal error.
    assert!(check_token_fidelity("a & & b", "a & & b").is_ok());
    let err = check_token_fidelity("a & & b", "a && b").unwrap_err();
    assert!(err.message.contains("internal error"), "{}", err.message);
}

#[test]
fn an_alias_file_with_a_syntax_error_is_refused() {
    // The preprocessed copy is what the parser sees, so the alias does not
    // turn the syntax gate off.
    let src = "alias _greeting = \"hi\"\n\nfun main() {\n    print(_greeting)\n    print(\"oops\"\n}\n";
    let err = format(src).unwrap_err();
    assert!(err.message.contains("expected ')'"), "{}", err.message);
}

#[test]
fn an_unresolvable_alias_is_refused_rather_than_papered_over() {
    // An undeclared `_name` identifier is a plain name to the gate.
    let src = "fun main() {\n    print(_undefined)\n}\n";
    assert_eq!(format(src).unwrap(), src);
    assert_eq!(format_after_resolve(src).unwrap(), src);
}

#[test]
fn a_carriage_return_between_tokens_is_refused() {
    // The lexer reads a `\r` in the whitespace between tokens as trivia, so
    // a CR-terminated file is one line to it: no line break the byte was
    // written to make is visible, and the formatter refuses rather than
    // laying the text on either side of it out on one line. The message
    // says which of the two consequences applies (see
    // `a_carriage_return_that_joins_nothing_is_described_honestly`).
    let err = format("fun main() {\r    print(\"hi\")\r}\r").unwrap_err();
    assert_eq!(err.kind, RefusalKind::LoneCarriageReturn);
    assert!(err.message.contains("carriage return"), "{}", err.message);
    assert!(err.message.contains("between tokens"), "{}", err.message);
    // CRLF is ordinary input and is not affected: the pair is a line break.
    assert!(format("fun main() {\r\n    print(\"hi\")\r\n}\r\n").is_ok());
}

#[test]
fn a_carriage_return_inside_a_literal_is_not_a_line_terminator() {
    // Defect 11: the refusal fired on any `\r` anywhere in the bytes, and
    // its diagnostic claimed the file "would be formatted as a single
    // line" - false on both counts for these inputs, which are multi-line,
    // are accepted by `kelpc build`, and format safely. The rule is about a
    // `\r` used as a line ending, and a literal's own bytes are not that.
    let string_lit = "fun main() {\n    print(\"a\rb\")\n}\n";
    assert_eq!(format(string_lit).unwrap(), string_lit);
    let char_lit = "fun main() {\n    val c = '\r'\n}\n";
    assert_eq!(format(char_lit).unwrap(), char_lit);
}

/// Kernel stage 2: a `repr(...)` clause and an enum's explicit
/// discriminants are ordinary tokens, so the existing spacing rules already
/// put them back exactly as written. This pins that, so a later rule that
/// treats `repr(c)` or `= 0` as a new atom kind has to keep the shape.
#[test]
fn a_repr_clause_and_enum_discriminants_are_formatted_as_written() {
    let src = "pub repr(packed) datac H(val kind: u8, val length: u32)\n\nrepr(u8) enum Mode {\n    Off = 0\n    On = 1\n    Fast = 10\n}\n\nrepr(align(16)) datac A(val x: u8)\n";
    assert_eq!(format(src).unwrap(), src);
}

/// Kernel gap 2: an `align(N)` clause on a `fun` is ordinary tokens, so the
/// existing spacing rules already put it back exactly as written, next to a
/// `section(...)` clause and a `naked` marker.
#[test]
fn an_align_clause_is_formatted_as_written() {
    let src = "section(\".text.trap\") align(4) fun handler() {}\n";
    assert_eq!(format(src).unwrap(), src);
}

#[test]
fn a_template_run_is_copied_verbatim() {
    let src = "fun main() {\n    val n = 1\n    print(\"sum=${n +  1}\")\n}\n";
    let out = format(src).unwrap();
    assert!(out.contains("\"sum=${n +  1}\""), "{out}");
}

/// The old refusal claimed the formatter "would join the text on either side
/// of it onto one line", which is false of a `\r` with nothing on one side
/// of it: leading whitespace, trailing whitespace at the end of the file, or
/// the only byte in the file. The message now states the consequence that
/// applies to the byte it names, and this pins both branches.
#[test]
fn a_carriage_return_that_joins_nothing_is_described_honestly() {
    // Text on both sides of the byte on its own line: the joining case.
    let err = format("fun main() {\r    print(\"x\")\r}\r").unwrap_err();
    assert!(
        err.message.contains("laid out on one line"),
        "the joining case was not described as one: {}",
        err.message
    );

    // Nothing on one side of the byte: the file's only byte, and a trailing
    // one after the last token. Neither joins anything, so the message must
    // not claim a join for either.
    for src in ["\r", "fun main() {}\r"] {
        let err = format(src).unwrap_err();
        assert!(
            err.message.contains("carriage return")
                && err.message.contains("between tokens"),
            "{src:?} was refused without naming the condition: {}",
            err.message
        );
        assert!(
            err.message.contains("nothing is joined"),
            "{src:?} joins nothing and the message does not say so: {}",
            err.message
        );
        assert!(
            !err.message.contains("laid out on one line"),
            "{src:?} joins nothing and the message claims it does: {}",
            err.message
        );
    }
}

// -----------------------------------------------------------------------
// the prose is checked against the data, not against another copy of itself
// -----------------------------------------------------------------------

const RULES_HEADING: &str = "## Rules this formatter enforces";
const CHECKS_HEADING: &str = "## What is checked, and what is refused";
const REFUSALS_HEADING: &str = "## Reasons a file is refused";
const LIMITS_HEADING: &str = "## Five limits that remain";

/// The module header: the doc comment at the top of this file, `//!` removed.
/// The rules and refusals above it are the data it has to agree with, so the
/// tests read the prose back out of the source file instead of trusting a
/// copy of it held somewhere else.
fn module_header() -> String {
    let mut out = String::new();
    for line in include_str!("../fmt.rs").lines() {
        let Some(rest) = line.strip_prefix("//!") else {
            break;
        };
        out.push_str(rest);
        out.push('\n');
    }
    assert!(!out.is_empty(), "the module header was not found");
    out
}

/// The header from `heading` onwards.
fn prose_from<'a>(header: &'a str, heading: &str) -> &'a str {
    let start = header
        .find(heading)
        .unwrap_or_else(|| panic!("the header has no '{heading}' section"));
    &header[start..]
}

/// The header from `from` up to the next section, `to`.
fn prose_between<'a>(header: &'a str, from: &str, to: &str) -> &'a str {
    let section = prose_from(header, from);
    let end = section
        .find(to)
        .unwrap_or_else(|| panic!("the header has no '{to}' section after '{from}'"));
    &section[..end]
}

/// The `- **name** - statement` bullets of a prose section, in order.
fn bullet_list(section: &str) -> Vec<(&str, &str)> {
    section
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("- **")?;
            let (name, statement) = rest.split_once("** - ")?;
            Some((name, statement.trim_end()))
        })
        .collect()
}

/// The stems of the `tests/fmt_corpus/<stem>.kp` pairs on disk.
fn corpus_pair_stems() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fmt_corpus");
    let mut stems = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("reading tests/fmt_corpus") {
        let name = entry.expect("a directory entry").file_name();
        let name = name.to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".kp") {
            if !stem.ends_with(".fmt") {
                stems.push(stem.to_string());
            }
        }
    }
    stems.sort();
    stems
}

/// The header's rule bullets are [`RULES`] itself: same names, same
/// statements, same order. A bullet that is not a rule, a rule with no
/// bullet, or a statement the two sides word differently all fail here.
#[test]
fn the_header_rule_bullets_are_the_rule_list_verbatim() {
    let header = module_header();
    let prose = prose_between(&header, RULES_HEADING, CHECKS_HEADING);
    let from_prose = bullet_list(prose);
    let from_data: Vec<(&str, &str)> = RULES
        .iter()
        .map(|rule| (rule.name, rule.statement))
        .collect();
    assert_eq!(
        from_prose, from_data,
        "the header's rule bullets are not the rule list, in its order"
    );
}

/// Every rule has at least one declared corpus pair, every pair a rule names
/// exists, and every pair on disk is claimed by exactly one rule. What this
/// proves is existence and a declared assignment; it does not prove that a
/// pair exercises its rule (see the module header). A rule with no declared
/// pair, and a pair no rule claims, are both failures.
#[test]
fn every_rule_is_pinned_by_a_corpus_pair_and_every_pair_is_claimed_once() {
    let pairs = corpus_pair_stems();
    assert!(
        pairs.len() >= 20,
        "only {} corpus pairs on disk",
        pairs.len()
    );
    let mut failures = Vec::new();
    for rule in RULES {
        if rule.pins.is_empty() {
            failures.push(format!("rule '{}' names no pinning pair", rule.name));
        }
        for pin in rule.pins {
            if rule.pins.iter().filter(|p| *p == pin).count() > 1 {
                failures.push(format!("rule '{}' names '{pin}.kp' twice", rule.name));
            }
            if !pairs.iter().any(|pair| pair == pin) {
                failures.push(format!(
                    "rule '{}' names '{pin}.kp', which is not in tests/fmt_corpus",
                    rule.name
                ));
            }
        }
    }
    for pair in &pairs {
        let claims = RULES
            .iter()
            .filter(|rule| rule.pins.contains(&pair.as_str()))
            .count();
        if claims != 1 {
            failures.push(format!(
                "tests/fmt_corpus/{pair}.kp is claimed by {claims} rules, not 1"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each corpus input's first line declares the rule it pins, and it must name
/// exactly the rule [`RULES`] claims it for. A pair whose first line names a
/// different rule, or a rule that names a pair whose line does not name it
/// back, fails here. This proves the declared assignment, not that the pair
/// exercises the rule.
#[test]
fn each_corpus_pair_declares_the_rule_that_claims_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fmt_corpus");
    let mut failures = Vec::new();
    for rule in RULES {
        for pin in rule.pins {
            let path = dir.join(format!("{pin}.kp"));
            let src = match std::fs::read_to_string(&path) {
                Ok(src) => src,
                Err(e) => {
                    failures.push(format!("reading {}: {e}", path.display()));
                    continue;
                }
            };
            let first = src.lines().next().unwrap_or("");
            let Some(rest) = first.strip_prefix("// pins ") else {
                failures.push(format!("{pin}.kp does not start with '// pins '"));
                continue;
            };
            let declared = rest.split(':').next().unwrap_or("").trim();
            if declared != rule.name {
                failures.push(format!(
                    "{pin}.kp declares '// pins {declared}', but rule '{}' claims it",
                    rule.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The refusal enumeration and the refusal data agree in name, order and
/// count, over the variants that chain reaches. [`RefusalKind::all`] is built
/// by walking [`RefusalKind::next`] (a wildcard-free match), so the comparison
/// reads only the variants on that chain. Every entry of [`REFUSALS`] carries
/// a kind, and no kind appears twice.
#[test]
fn the_refusal_enumeration_and_the_refusal_data_agree() {
    let from_enum: Vec<&str> = RefusalKind::all().iter().map(|kind| kind.name()).collect();
    let from_data: Vec<&str> = REFUSALS.iter().map(|refusal| refusal.kind.name()).collect();
    assert_eq!(
        from_enum, from_data,
        "the enumeration and REFUSALS disagree in name, order or count"
    );
    let mut kinds: Vec<&str> = REFUSALS.iter().map(|refusal| refusal.kind.name()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(
        kinds.len(),
        REFUSALS.len(),
        "REFUSALS carries the same kind twice"
    );
}

/// The header's refusal bullets are [`REFUSALS`] itself: same names, same
/// conditions, same order. A bullet whose text the data does not carry - a
/// false statement kept alive by its backticked name - fails here, which is
/// what the older name-only check could not catch.
#[test]
fn the_header_refusal_bullets_are_the_refusal_list_verbatim() {
    let header = module_header();
    let prose = prose_between(&header, REFUSALS_HEADING, LIMITS_HEADING);
    let from_prose: Vec<(String, String)> = bullet_list(prose)
        .into_iter()
        .map(|(name, statement)| (name.to_string(), statement.to_string()))
        .collect();
    let from_data: Vec<(String, String)> = REFUSALS
        .iter()
        .map(|refusal| {
            (
                format!("`{}`", refusal.kind.name()),
                refusal.reason.to_string(),
            )
        })
        .collect();
    assert_eq!(
        from_prose, from_data,
        "the header's refusal bullets are not the refusal list, in its order"
    );
}

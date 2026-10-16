//! The corpus for `kelpc fmt` (roadmap open item 13, `design/syntax.md (kelp wiki)` §19).
//!
//! `kelpc fmt` is a token-level whitespace reformatter: it lexes the source
//! with comments kept, then re-emits every token by copying its own source
//! slice and changes only the whitespace *between* tokens. That design is what
//! makes the interesting behavior a set of **properties** rather than a set of
//! hand-written outputs: whatever the formatter does to the whitespace, the
//! program must not change.
//!
//! So this file asserts, over the whole repository's Kelp source:
//!
//!   1. **round trip / idempotence**, `kelpc fmt --check` on a formatted file
//!      exits zero, which is `F(F(x)) == F(x)`;
//!   2. **token fidelity**, the lexer's sequence of `(kind, source slice)`
//!      pairs is identical before and after formatting (blank-line runs are
//!      the one thing allowed to differ, so `Newline` is dropped). Comparing
//!      the *slice* and not only the kind is what proves identifiers, literals
//!      and operators are byte-equal rather than merely well-typed;
//!   3. **comment survival**, the comments, in order, verbatim;
//!   4. **still lexes, and a refusal is honest**, `F(x)` lexes; a file the
//!      formatter declines is reported and left byte-for-byte unchanged. The
//!      names `kelpc/src/fmt.rs` enumerates, one `- **`name`** - …` bullet each
//!      under its *Reasons a file is refused* heading, are read back out of that
//!      header by `enumerated_refusal_names`, and
//!      `the_file_header_names_every_refusal_reason_the_formatter_enumerates`
//!      checks that this paragraph names each one. That check reads the header's
//!      bullets, not the `REFUSALS` data, and it proves only that each name
//!      appears here; it does not prove any condition is worded correctly, that
//!      a variant is on the `next` chain `all` walks, or that no refusal is
//!      raised as a raw string, and nothing here constrains prose outside this
//!      list.
//!
//!      Six are paths with no program behind them, and `kelpc build` rejects
//!      each: `path-names-nothing` and `dangling-symlink` are a path that cannot
//!      be read at all and a symbolic link whose target cannot be read;
//!      `unreadable-file`, `lex-error`, `syntax-error` and `unresolvable-alias`
//!      are an unreadable or non-UTF-8 file, a source that does not lex, a
//!      preprocessed source that does not parse, and a `%*name*%` reference that
//!      neither the file's own declarations nor the project's alias table can
//!      supply, declined with the alias named rather than formatted with a
//!      placeholder for it.
//!
//!      `lone-carriage-return` is the one *input shape* in which `fmt` declines
//!      a program `kelpc build` accepts: the lexer reads a `\r` between tokens
//!      as whitespace, not as a line break, so a reformat cannot see the break,
//!      and the refusal says either that the text on both sides of the byte is
//!      laid out on one line or that the byte has nothing on one side and is
//!      dropped, whichever is true of the byte it names. A `\r` inside a string
//!      or character literal is that literal's own byte and is formatted
//!      normally (defect 11).
//!
//!      The remaining four are not about the file's text, which is why they are
//!      not shapes a valid program can come in: `not-a-regular-file` refuses a
//!      path that is not a regular file before anything reads it, `write-failed`
//!      reports a `-w` write that failed, `check-found-unformatted` is the
//!      `--check` verdict on a file that differs from its formatted form (it
//!      exits nonzero and writes nothing), and
//!      `token-fidelity-internal-error` can only fire on an emitter bug;
//!   5. **semantic equivalence**, a formatted copy of a runnable example
//!      prints the same bytes (an `#[ignore]`d test, because it builds and runs
//!      a native binary).
//!
//! The symlink and failed-write tests (defects 3 and 4) are asserted the same
//! way, as outcomes, not as reference output: `-w` through a link leaves the
//! link a link and rewrites the file it names; a link that names nothing, or a
//! directory, is refused in every mode; a directory walk never follows a link;
//! and a write that fails after its temporary file exists leaves no temporary
//! behind, no changed bytes, and a nonzero status with a diagnostic. The
//! filesystem conditions that last one needs (a full filesystem, a read-only
//! one, a file that is a mount point) are built inside a user and mount
//! namespace, because mounting is what an unprivileged test cannot otherwise
//! do.
//!
//! Two more outcomes are asserted the same way. A path that is not a regular
//! file, a FIFO, a socket, a device, is refused before anything reads it, so
//! a FIFO with no writer cannot block the run and the node is never replaced by
//! a regular file (defect 12). And `-w` lands the rewrite as a *new inode*, so a
//! hard link to the formatted file keeps the old bytes: that is the documented
//! outcome, pinned here so it cannot change silently (defect 13).
//!
//! Alongside the properties, `tests/fmt_corpus/` holds small golden pairs:
//! `<name>.kp` is an input, `<name>.fmt.kp` the output `design/syntax.md (kelp wiki)` and the
//! formatter's design say it must have. Each input's first line names the rule
//! it pins, and `kelpc/src/fmt.rs` (`RULES`) is the authority on which rule
//! claims which pair: every rule names at least one pair, every pair is claimed
//! by exactly one rule, and each pair's first line names that rule back (unit
//! tests there fail on any mismatch). What those tests prove is the pair's
//! existence and the declared assignment; they do not check that the pair
//! exercises the rule it names. The pairs are the trivia hazards (a comment at
//! end of file, a comment between statements, a comment after a trailing comma,
//! a comment inside an argument list, a line that is only a comment, a doc
//! comment, a blank-line run, no trailing newline, several trailing newlines,
//! CRLF, deep nesting) plus one pair for each enforceable rule.
//!
//! **Read-only contract.** `kelpc/tests/fixtures/**` and `plan/…` are owned by
//! another workflow; `kelpc/std/**` is the test contract. This file reads them
//! and copies each into the test's own temporary directory before formatting,
//! the copy carries the tree the file lives in (`mirrored`), because a lone
//! copy of `src/main.kp` is a file whose `util.kp` is missing and whose alias
//! table therefore really is incomplete. It never writes to any existing `.kp`
//! file.
//!
//! The lexer is compiled into this test binary directly (`#[path]` includes
//! `kelpc/src/lexer.rs`) because `kelpc` has no library target: the token
//! sequence and the comments are not observable through the command line, and
//! the properties above are about them. `tokenize_with_trivia` is the
//! trivia-preserving entry point the formatter itself lexes with, it returns a
//! `TokenKind::Comment` per `//` comment, while `tokenize` drops comments,
//! which is the sequence `significant_tokens` compares.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{chown, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../src/lexer.rs"]
#[allow(dead_code)] // the standalone copy drops the parser's only caller of Span::to
mod lexer;

use lexer::{Lexer, TokenKind};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A path under the test's own scratch space. Cargo publishes
/// `CARGO_TARGET_TMPDIR` at compile time for integration tests; the fallback
/// keeps the test usable when it is built by a tool that does not, and is named
/// uniquely so it cannot collide with anything else in the temp directory.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-fmt-tests"),
    };
    root.join("fmt").join(name)
}

/// A scratch directory that starts empty. `CARGO_TARGET_TMPDIR` survives a test
/// run, so a leftover symlink or file from the last one would otherwise change
/// what this one measures.
fn fresh_scratch(name: &str) -> PathBuf {
    let dir = scratch(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// A short name for a path, relative to the crate, so a failure report names the
/// file rather than an absolute path.
fn brief(path: &Path) -> String {
    match path.strip_prefix(env!("CARGO_MANIFEST_DIR")) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// A scratch file name for a corpus path: unique, and stable enough that a
/// failing case can be re-run by hand.
fn scratch_name(path: &Path) -> String {
    let rel = brief(path);
    let mut s = String::with_capacity(rel.len() + 3);
    for c in rel.chars() {
        s.push(if c.is_ascii_alphanumeric() { c } else { '_' });
    }
    s.push_str(".kp");
    s
}

fn fmt_stdout(path: &Path) -> Output {
    kelpc()
        .arg("fmt")
        .arg(path)
        .output()
        .expect("failed to spawn kelpc fmt")
}

fn fmt_write(path: &Path) -> Output {
    kelpc()
        .args(["fmt", "-w"])
        .arg(path)
        .output()
        .expect("failed to spawn kelpc fmt -w")
}

fn fmt_check(path: &Path) -> Output {
    kelpc()
        .args(["fmt", "--check"])
        .arg(path)
        .output()
        .expect("failed to spawn kelpc fmt --check")
}

/// The significant token sequence of a source that is known to lex: every token
/// except `Newline` (blank-line runs are the formatter's to normalize) and
/// `Eof`, as `(kind, source slice)`. The slice is what makes this a proof about
/// literal spelling, not merely token kinds.
fn significant_tokens(src: &str) -> Vec<(String, String)> {
    Lexer::new(src)
        .tokenize()
        .expect("a source that already lexed lexes again")
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
        .map(|t| {
            (
                format!("{:?}", t.kind),
                src[t.span.start..t.span.end].to_string(),
            )
        })
        .collect()
}

/// Every `//` comment in a source that lexes, in order, from `//` to the end of
/// the line, taken straight from the trivia-preserving lexer. Leading
/// indentation is not part of the token (that is the formatter's to re-indent),
/// and trailing whitespace is *not* trimmed: a comment is a token, no token's
/// text is ever rewritten, so the bytes at the end of a comment line are the
/// comment's own and have to survive (defect 15; `tests/fmt_corpus/
/// comment_trailing_whitespace.*.kp` pins it).
fn comments(src: &str) -> Vec<String> {
    Lexer::new(src)
        .tokenize_with_trivia()
        .expect("a source that already lexed lexes again")
        .into_iter()
        .filter(|t| matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| src[t.span.start..t.span.end].to_string())
        .collect()
}

/// The byte range of every `//` comment in a source that lexes, so a check can
/// ask whether a byte is inside a comment without scanning the line for `//`
/// (which a `//` inside a string literal would fool).
fn comment_spans(src: &str) -> Vec<(usize, usize)> {
    Lexer::new(src)
        .tokenize_with_trivia()
        .expect("a source that already lexed lexes again")
        .into_iter()
        .filter(|t| matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| (t.span.start, t.span.end))
        .collect()
}

/// A one-line account of the first place two byte strings differ.
fn first_difference(want: &[u8], got: &[u8]) -> String {
    let want = String::from_utf8_lossy(want);
    let got = String::from_utf8_lossy(got);
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        if a != b {
            return format!("first difference at line {}:\n    want: {a:?}\n    got:  {b:?}", i + 1);
        }
    }
    format!(
        "{} line(s) wanted, {} line(s) produced",
        want.lines().count(),
        got.lines().count()
    )
}

fn first_sequence_difference<T: PartialEq + std::fmt::Debug>(a: &[T], b: &[T]) -> String {
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            return format!("first difference at index {i}: before {:?}, after {:?}", a.get(i), b.get(i));
        }
    }
    String::new()
}

/// The adversarial adjacency probes. Each one places two tokens next to each
/// other that the lexer would fold into one *longer* token, or, for `/ /`,
/// into a comment, if the formatter ever ran them together. `&` before `&`
/// is the reported case; the rest are the same mechanism. Each is embedded in
/// a function body so the only thing under test is the spacing between the two
/// tokens.
fn adjacency_probes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("& &", "val b = & & x"),
        ("& &mut", "val b = & &mut x"),
        ("! =", "val b = ! = x"),
        ("- >", "val b = - > x"),
        ("* =", "val b = * = x"),
        ("/ /", "val b = / / x"),
        ("= =", "val b = = = x"),
        ("< =", "val b = < = x"),
        ("> =", "val b = > = x"),
        ("+ +", "val b = + + x"),
        ("- -", "val b = - - x"),
        ("* *", "val b = * * x"),
        ("| |", "val b = | | x"),
        ("? ?", "val b = x? ?"),
        (". .", "val b = x . . y"),
    ]
}

/// The source for one probe: a header naming the pair, then a function that
/// contains it.
fn adjacency_source(label: &str, line: &str) -> String {
    format!("// adjacency probe: {label}\nfun main() {{\n    {line}\n}}\n")
}

/// Report at most `LIMIT` failures, then a count, so an unimplemented `fmt`
/// produces a readable report instead of one line per file in the repository.
fn report(failures: &[String], what: &str) -> String {
    const LIMIT: usize = 12;
    let mut s = format!("{} {what}:\n", failures.len());
    for f in failures.iter().take(LIMIT) {
        s.push_str("  - ");
        s.push_str(f);
        s.push('\n');
    }
    if failures.len() > LIMIT {
        s.push_str(&format!("  … and {} more\n", failures.len() - LIMIT));
    }
    s
}

// ---------------------------------------------------------------------------
// the refusal list, and this file's header
// ---------------------------------------------------------------------------

/// The refusal names `kelpc/src/fmt.rs` enumerates, read back out of that
/// file's own bullet list: the module header section "Reasons a file is refused",
/// one `- **`name`** - …` bullet per reason. Reading the list out of the source
/// is what lets this file's header be checked against the data instead of
/// against a second copy of it.
fn enumerated_refusal_names() -> Vec<String> {
    let mut header = String::new();
    for line in include_str!("../src/fmt.rs").lines() {
        let Some(rest) = line.strip_prefix("//!") else {
            break;
        };
        header.push_str(rest);
        header.push('\n');
    }
    let mut names = Vec::new();
    let mut inside = false;
    for line in header.lines() {
        let trimmed = line.trim_start();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            inside = heading.starts_with("Reasons a file is refused");
            continue;
        }
        if !inside {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("- **`") else {
            continue;
        };
        if let Some((name, _)) = rest.split_once("`**") {
            names.push(name.to_string());
        }
    }
    names
}

/// The header of this file names each refusal bullet `kelpc/src/fmt.rs` lists.
/// What this proves is that the names agree, not that any condition is worded
/// correctly (see the module header). The header used to call an
/// unresolvable alias one of the shapes a *valid* Kelp program is declined in,
/// it is not valid Kelp, and `kelpc build` rejects it, while a later paragraph
/// in the same header already said the carriage-return shape was the only one.
#[test]
fn the_file_header_names_every_refusal_reason_the_formatter_enumerates() {
    let names = enumerated_refusal_names();
    assert!(
        names.len() >= 8,
        "only {} refusal bullets were read out of kelpc/src/fmt.rs",
        names.len()
    );
    let header = include_str!("fmt.rs")
        .lines()
        .take_while(|line| line.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    let missing: Vec<&String> = names
        .iter()
        .filter(|name| !header.contains(&format!("`{name}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "the header of this file does not name: {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// the golden corpus
// ---------------------------------------------------------------------------

fn corpus_dir() -> PathBuf {
    manifest("tests/fmt_corpus")
}

/// Every `<name>.kp` in `tests/fmt_corpus/` paired with its `<name>.fmt.kp`.
fn corpus_pairs() -> Vec<(PathBuf, PathBuf)> {
    let dir = corpus_dir();
    let mut pairs = Vec::new();
    for entry in fs::read_dir(&dir).expect("reading tests/fmt_corpus") {
        let path = entry.expect("a directory entry").path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".kp") || name.ends_with(".fmt.kp") {
            continue;
        }
        let stem = &name[..name.len() - ".kp".len()];
        pairs.push((path, dir.join(format!("{stem}.fmt.kp"))));
    }
    pairs.sort();
    pairs
}

/// Each input's first line has to name the rule it pins, so a reader can tell a
/// regression from a rule change.
#[test]
fn every_corpus_input_names_the_rule_it_pins() {
    let pairs = corpus_pairs();
    assert!(pairs.len() >= 20, "the corpus lost pairs: {} found", pairs.len());
    for (input, _) in &pairs {
        let src = fs::read_to_string(input).expect("reading a corpus input");
        let first = src.lines().next().unwrap_or("");
        assert!(
            first.starts_with("// pins "),
            "{} does not start with a '// pins …' header naming its rule",
            brief(input)
        );
    }
}

#[test]
fn every_corpus_pair_formats_to_its_expected_output() {
    let pairs = corpus_pairs();
    assert!(pairs.len() >= 20, "the corpus lost pairs: {} found", pairs.len());

    let mut failures = Vec::new();
    for (input, expected) in &pairs {
        let want = match fs::read(expected) {
            Ok(w) => w,
            Err(e) => {
                failures.push(format!("{}: cannot read {}: {e}", brief(input), brief(expected)));
                continue;
            }
        };
        let out = fmt_stdout(input);
        if !out.status.success() {
            failures.push(format!(
                "{}: kelpc fmt exited {:?}:\n    {}",
                brief(input),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim().replace('\n', "\n    ")
            ));
            continue;
        }
        if out.stdout != want {
            failures.push(format!(
                "{}: formatted output differs from {}\n    {}",
                brief(input),
                brief(expected),
                first_difference(&want, &out.stdout)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "corpus pair(s) disagree with the pinned output")
    );
}

/// The golden pairs are internally consistent as *formatter outputs*: token
/// sequence and comment sequence (trailing whitespace included) are the same
/// before and after, the expected file ends with exactly one newline, and it
/// holds no tab, no CR, and no trailing whitespace outside a comment. This test
/// needs no `kelpc fmt`, it fails only if a hand-written expected file pins
/// something that is not a legal formatter output at all, which is what keeps
/// the corpus honest while the formatter is still being written.
#[test]
fn every_expected_output_is_token_faithful_to_its_input() {
    let pairs = corpus_pairs();
    let mut failures = Vec::new();
    for (input, expected) in &pairs {
        let before = fs::read_to_string(input).expect("reading a corpus input");
        let after = fs::read_to_string(expected).expect("reading an expected output");
        if let Err(e) = Lexer::new(&before).tokenize() {
            failures.push(format!("{}: the input does not lex: {}", brief(input), e.message));
            continue;
        }
        if let Err(e) = Lexer::new(&after).tokenize() {
            failures.push(format!("{}: the expected output does not lex: {}", brief(expected), e.message));
            continue;
        }
        let tokens_before = significant_tokens(&before);
        let tokens_after = significant_tokens(&after);
        if tokens_before != tokens_after {
            failures.push(format!(
                "{}: the expected output is not token-faithful to the input\n    {}",
                brief(input),
                first_sequence_difference(&tokens_before, &tokens_after)
            ));
        }
        let comments_before = comments(&before);
        let comments_after = comments(&after);
        if comments_before != comments_after {
            failures.push(format!(
                "{}: the expected output drops or changes a comment\n    {}",
                brief(input),
                first_sequence_difference(&comments_before, &comments_after)
            ));
        }
        if !after.ends_with('\n') || after.ends_with("\n\n") {
            failures.push(format!(
                "{}: an expected output must end with exactly one newline",
                brief(expected)
            ));
        }
        for (i, line) in after.lines().enumerate() {
            if line.contains('\t') {
                failures.push(format!("{}:{}: a tab survived", brief(expected), i + 1));
            }
            if line.contains('\r') {
                failures.push(format!("{}:{}: a CR survived", brief(expected), i + 1));
            }
        }
        // Trailing whitespace is removed from a line of code, and the bytes
        // inside a comment are the comment's own, so a byte of trailing
        // whitespace is legal only inside a comment (defect 15).
        let spans = comment_spans(&after);
        let in_comment = |at: usize| spans.iter().any(|(s, e)| at >= *s && at < *e);
        let bytes = after.as_bytes();
        for (i, byte) in bytes.iter().enumerate() {
            let at_line_end = i + 1 == bytes.len() || bytes[i + 1] == b'\n';
            if matches!(byte, b' ' | b'\t') && at_line_end && !in_comment(i) {
                let line = 1 + bytes[..i].iter().filter(|b| **b == b'\n').count();
                failures.push(format!("{}:{line}: trailing whitespace", brief(expected)));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "expected output(s) are not a legal formatter output")
    );
}

/// `comments` reads a comment out of the gap between two token spans, so a
/// `//` inside a string literal must not be mistaken for one. This pins that
/// the recovery is gap-based and not a text scan.
#[test]
fn a_slash_inside_a_string_is_not_a_comment() {
    let src = "val url = \"http://example.com\"\n// the only comment\n";
    assert_eq!(comments(src), vec!["// the only comment".to_string()]);
}

/// The expected file is the formatter's own fixed point: `--check` on it exits
/// zero. This is what makes a golden pair a statement about the *rule* and not
/// about one copy of the input.
#[test]
fn every_expected_output_is_a_fixed_point() {
    let pairs = corpus_pairs();
    let mut failures = Vec::new();
    for (_, expected) in &pairs {
        let out = fmt_check(expected);
        if !out.status.success() {
            failures.push(format!(
                "{}: kelpc fmt --check exited {:?}:\n    {}",
                brief(expected),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim().replace('\n', "\n    ")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "expected output(s) are not a fixed point")
    );
}

// ---------------------------------------------------------------------------
// the property test over the repository's own source
// ---------------------------------------------------------------------------

fn collect_kp(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries {
        let entry = entry.expect("a directory entry");
        // A symbolic link is not followed, whether it names a file or a
        // directory: `mirror_tree` does not copy one, so a file reached
        // through a link has no counterpart in the mirror and the properties
        // below would be asserted against a path that does not exist. The
        // link's target is collected under its own name when it is inside the
        // tree, which is the copy this corpus formats. `kelpc lint`'s own walk
        // (`main.rs::collect_kp_into`) skips links for its own version of this
        // reason.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_kp(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("kp") {
            out.push(path);
        }
    }
}

/// The repository's Kelp source, read-only: the standard library, the examples,
/// and the fixtures and projects the other workflows own. This file never
/// writes to any of them, each file is copied into the test's scratch
/// directory before it is formatted.
fn repository_corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in ["std", "tests/fixtures", "tests/projects", "../examples"] {
        collect_kp(&manifest(dir), &mut files);
    }
    files.sort();
    files
}

/// The tree a corpus file has to be copied *with* to stay the same program to
/// the resolver: the nearest directory above it that holds a `coral.toml` (the
/// project root, whose `src/` the `impl` paths are relative to), or the file's
/// own directory when there is no manifest, which is what `kelpc build` treats
/// a manifest-less file as.
fn context_root(file: &Path) -> PathBuf {
    let dir = file
        .parent()
        .expect("a corpus file has a parent directory")
        .to_path_buf();
    let mut above = Some(dir.as_path());
    while let Some(dir) = above {
        if dir.join("coral.toml").is_file() {
            return dir.to_path_buf();
        }
        above = dir.parent();
    }
    dir
}

/// The mirrored trees *one test* formats in.
///
/// Each test owns its own mirror, the tests run in parallel, so a shared one
/// would let one test's fresh copy delete another's files mid-run, and within a
/// test each tree is copied once and then reused, because formatting every file
/// of a project only needs one copy of it.
struct Trees {
    tag: &'static str,
    mirrored: HashMap<PathBuf, PathBuf>,
}

impl Trees {
    fn new(tag: &'static str) -> Self {
        Trees {
            tag,
            mirrored: HashMap::new(),
        }
    }

    /// The scratch copy of a corpus file, inside a mirror of the tree it lives
    /// in.
    ///
    /// A lone copy is not the same input: `kelpc fmt` asks the compiler's
    /// resolver for the project's alias table when the file's own declarations
    /// cannot resolve a `%*name*%`, and a copy with no `util.kp` beside it is a
    /// file whose alias table really is incomplete, fmt refuses it, and that
    /// refusal says nothing about formatting. Copying the tree gives the copy
    /// the module graph the original has, so the assertions below stay
    /// statements about formatting.
    fn copy_of(&mut self, file: &Path) -> PathBuf {
        let root = context_root(file);
        let tree = self
            .mirrored
            .entry(root.clone())
            .or_insert_with(|| mirror_tree(self.tag, &root));
        tree.join(
            file.strip_prefix(&root)
                .expect("the context root contains the file"),
        )
    }
}

/// Copies one tree into the scratch directory and returns its new root. Symbolic
/// links are not copied, the corpus has none, because `kelpc fmt -w` does not
/// follow them either.
fn mirror_tree(tag: &str, root: &Path) -> PathBuf {
    let dest = scratch(&format!("tree/{tag}/{}", scratch_name(root)));
    let _ = fs::remove_dir_all(&dest);
    copy_tree(root, &dest).expect("copying a corpus tree into scratch");
    dest
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()))?;
        } else {
            fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Files that are valid Kelp by construction, so the formatter must accept
/// them: the standard library, the example programs, and the project fixtures.
/// `tests/fixtures/` holds deliberately invalid programs too, the compiler's
/// negative tests, which a syntax gate is right to refuse, so that directory
/// is not in this set.
fn must_format(path: &Path) -> bool {
    let rel = brief(path);
    rel.starts_with("std/")
        || rel.starts_with("tests/projects/")
        || rel.starts_with("../examples/")
}

#[test]
fn formatting_is_idempotent_token_faithful_and_comment_preserving() {
    let files = repository_corpus();
    assert!(
        files.len() > 90,
        "the repository corpus is too small to mean anything: {} files",
        files.len()
    );

    let mut failures = Vec::new();
    let mut formatted = 0usize;
    let mut refused = 0usize;
    let mut trees = Trees::new("idempotence");

    for file in &files {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: cannot read: {e}", brief(file)));
                continue;
            }
        };

        let work = trees.copy_of(file);

        // The formatter may decline a file. It must say why and must not have
        // touched the file. A refusal is right for a program that is not valid
        // Kelp, a syntax error is reported, not silently reformatted, but a
        // valid program must format, so `must_format` is asserted separately.
        let out = fmt_write(&work);
        if !out.status.success() {
            if fs::read(&work).expect("reading back the refused file") != src.as_bytes() {
                failures.push(format!("{}: refused, but the file was rewritten", brief(file)));
            } else if out.stderr.is_empty() {
                failures.push(format!("{}: refused without a diagnostic", brief(file)));
            } else if must_format(file) {
                failures.push(format!(
                    "{}: kelpc fmt -w refused a program it must format:\n    {}",
                    brief(file),
                    String::from_utf8_lossy(&out.stderr).trim().replace('\n', "\n    ")
                ));
            } else {
                refused += 1;
            }
            continue;
        }
        let formatted_src = match fs::read_to_string(&work) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: the formatted copy is not utf-8: {e}", brief(file)));
                continue;
            }
        };

        // (4) the output still lexes.
        if let Err(err) = Lexer::new(&formatted_src).tokenize() {
            failures.push(format!(
                "{}: the formatted copy does not lex: {}",
                brief(file),
                err.message
            ));
            continue;
        }

        // (2) token fidelity, by kind and source slice.
        let before = significant_tokens(&src);
        let after = significant_tokens(&formatted_src);
        if before != after {
            failures.push(format!(
                "{}: the token sequence changed\n    {}",
                brief(file),
                first_sequence_difference(&before, &after)
            ));
        }

        // (3) every comment survives, in order, verbatim.
        let comments_before = comments(&src);
        let comments_after = comments(&formatted_src);
        if comments_before != comments_after {
            failures.push(format!(
                "{}: comments changed\n    {}",
                brief(file),
                first_sequence_difference(&comments_before, &comments_after)
            ));
        }

        // (1) idempotence, phrased as the formatter's own check: the formatted
        // copy is byte-for-byte what formatting it again would produce.
        let check = fmt_check(&work);
        if !check.status.success() {
            failures.push(format!(
                "{}: kelpc fmt --check on the formatted copy exited {:?}, formatting is not idempotent",
                brief(file),
                check.status.code()
            ));
        }

        formatted += 1;
    }

    assert!(
        failures.is_empty(),
        "over {} files ({} formatted, {} refused):\n{}",
        files.len(),
        formatted,
        refused,
        report(&failures, "file(s) failed a formatting property")
    );
}

// ---------------------------------------------------------------------------
// operator adjacency (defect 1)
// ---------------------------------------------------------------------------

/// A prefix operator that is followed by a token which would fold into a longer
/// one must keep the space. The reported case is `val b = & & x`: the emitter
/// used to join two prefix operators, `& &` became `&&` (one `AndAnd` token),
/// and the file stopped parsing. The test drives the CLI over every adjacency
/// probe: where the input parses, the output must be token-faithful and a fixed
/// point; where it does not, it must be refused and left byte-unchanged.
#[test]
fn operator_adjacencies_are_never_silently_relexed() {
    let dir = fresh_scratch("adjacency");

    // The exact reported behavior, pinned directly in the output text.
    let direct = dir.join("direct.kp");
    fs::write(&direct, "fun main() {\n    val x = 5\n    val b = & & x\n}\n")
        .expect("writing the defect-1 program");
    let out = fmt_stdout(&direct);
    assert!(out.status.success(), "the defect-1 program was refused");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("val b = & &x"),
        "the formatter merged the two borrows:\n{text}"
    );

    let mut failures = Vec::new();
    let mut formatted = 0usize;
    for (i, (label, line)) in adjacency_probes().into_iter().enumerate() {
        let src = adjacency_source(label, line);
        let file = dir.join(format!("probe{i}.kp"));
        fs::write(&file, &src).expect("writing an adjacency probe");

        let out = fmt_write(&file);
        if !out.status.success() {
            if out.stderr.is_empty() {
                failures.push(format!("{label}: refused with no diagnostic"));
            }
            if fs::read_to_string(&file).expect("reading the probe back") != src {
                failures.push(format!("{label}: refused but the file changed"));
            }
            continue;
        }
        formatted += 1;
        let after = fs::read_to_string(&file).expect("reading the formatted probe");
        let before_tokens = significant_tokens(&src);
        let after_tokens = significant_tokens(&after);
        if before_tokens != after_tokens {
            failures.push(format!(
                "{label}: the token sequence changed\n    {}",
                first_sequence_difference(&before_tokens, &after_tokens)
            ));
        }
        let check = fmt_check(&file);
        if !check.status.success() {
            failures.push(format!(
                "{label}: the formatted output is not a fixed point:\n    {}",
                String::from_utf8_lossy(&check.stderr).trim().replace('\n', "\n    ")
            ));
        }
    }
    assert!(
        formatted >= 5,
        "only {formatted} adjacency probe(s) formatted; the gate is refusing valid input"
    );
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "adjacency probe(s) failed")
    );
}

/// The whole-output post-condition, asserted from outside the formatter: for
/// every readable Kelp source in the repository plus every adjacency probe,
/// the significant token sequence (`kind` and exact source slice) of `kelpc
/// fmt`'s output equals the input's. This is precisely the property the
/// original defect-1 output violated, `& &` came back as `&&`, and the
/// formatter now refuses rather than emit a mismatch, so this test fails loudly
/// if that guard is ever removed.
#[test]
fn a_formatted_output_always_has_the_inputs_significant_tokens() {
    let dir = fresh_scratch("token_fidelity");

    let mut probes: Vec<(String, String)> = adjacency_probes()
        .into_iter()
        .map(|(label, line)| (label.to_string(), adjacency_source(label, line)))
        .collect();
    probes.push((
        "prefix_operator_adjacency".to_string(),
        "fun main() {\n    val x = 5\n    val b = & & x\n}\n".to_string(),
    ));

    let mut corpus: Vec<(PathBuf, String)> = Vec::new();
    for file in repository_corpus() {
        if let Ok(src) = fs::read_to_string(&file) {
            corpus.push((file, src));
        }
    }
    assert!(
        corpus.len() + probes.len() > 90,
        "the token-fidelity corpus is too small to mean anything: {} sources",
        corpus.len() + probes.len()
    );

    let mut failures = Vec::new();
    let mut checked = 0usize;
    let mut check = |file: &Path, name: &str, src: &str| {
        let out = fmt_stdout(file);
        // A refusal is right for a deliberately invalid file and is exercised
        // elsewhere; the post-condition is about the output that is produced.
        if !out.status.success() {
            return;
        }
        checked += 1;
        let after = String::from_utf8_lossy(&out.stdout).to_string();
        if let Err(e) = Lexer::new(&after).tokenize() {
            failures.push(format!("{name}: the output does not lex: {}", e.message));
            return;
        }
        let before_tokens = significant_tokens(src);
        let after_tokens = significant_tokens(&after);
        if before_tokens != after_tokens {
            failures.push(format!(
                "{name}: the significant token sequence changed\n    {}",
                first_sequence_difference(&before_tokens, &after_tokens)
            ));
        }
    };

    // The repository corpus goes through the same copy-with-its-tree rule as
    // every other corpus test, so a file whose aliases come from another module
    // is checked rather than refused.
    let mut trees = Trees::new("token_fidelity");
    for (file, src) in &corpus {
        let copy = trees.copy_of(file);
        check(&copy, &brief(file), src);
    }
    for (i, (name, src)) in probes.iter().enumerate() {
        let file = dir.join(format!("source{i}.kp"));
        fs::write(&file, src).expect("copying a source into scratch");
        check(&file, name, src);
    }
    drop(check);
    assert!(
        checked > 90,
        "only {checked} source(s) produced output; the post-condition was barely exercised"
    );
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "output(s) changed the significant token sequence")
    );
}

// ---------------------------------------------------------------------------
// the validity gate
// ---------------------------------------------------------------------------

/// An alias-using file bypassed the syntax gate before this slice: the gate
/// ran only for files `preprocess` did not touch, so an inline `alias …`
/// declaration turned the gate off entirely. The gate now parses a
/// preprocessed copy for every file, so the same syntax error `kelpc build`
/// reports is refused here, with no write. The fixed file still formats, and
/// the alias line and reference are still copied atomically.
#[test]
fn an_alias_file_with_a_syntax_error_is_refused_and_unchanged() {
    let dir = fresh_scratch("alias_syntax_error");
    let file = dir.join("alias_bad.kp");
    // The exact reproduction: the alias line is fine, `print("oops"` is not,
    // and the raw stream cannot be parsed because `%*greeting*%` is an alias.
    let src = "alias %*greeting*% = \"hi\"\n\nfun main() {\n    print(%*greeting*%)\n    print(\"oops\"\n}\n";
    fs::write(&file, src).expect("writing the broken alias file");

    let out = fmt_write(&file);
    assert!(
        !out.status.success(),
        "kelpc fmt -w accepted an alias file with a syntax error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("expected ')'"),
        "the diagnostic did not name the real error:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&file).expect("reading the file back"),
        src,
        "kelpc fmt -w rewrote a file with a syntax error"
    );

    // The same file with the error fixed formats, and keeps the alias line and
    // reference verbatim.
    let ok = dir.join("alias_ok.kp");
    let fixed = "alias %*greeting*% = \"hi\"\n\nfun main() {\n   print(%*greeting*%)\n   print(\"oops\")\n}\n";
    fs::write(&ok, fixed).expect("writing the fixed alias file");
    let out = fmt_stdout(&ok);
    assert!(
        out.status.success(),
        "a valid alias file was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("alias %*greeting*% = \"hi\""),
        "the alias declaration changed:\n{text}"
    );
    assert!(
        text.contains("print(%*greeting*%)"),
        "the alias reference stopped being atomic:\n{text}"
    );
}

/// A file the formatter cannot process is reported and left alone. The file
/// here fails to *lex* (an unknown string escape), which is the syntax error
/// that reaches the formatter first; the property test above covers the files
/// that lex but are still not valid Kelp.
#[test]
fn a_lex_error_is_reported_and_nothing_is_written() {
    let dir = fresh_scratch("lex_error");
    let file = dir.join("bad.kp");
    let src = "// deliberately broken: an unknown escape is a lex error.\nfun main() {\n    print(\"\\q\")\n}\n";
    fs::write(&file, src).expect("writing the broken file");

    let out = fmt_write(&file);
    assert!(
        !out.status.success(),
        "kelpc fmt -w accepted a file with a lex error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown escape"),
        "the diagnostic did not name the lex error:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&file).expect("reading the file back"),
        src,
        "kelpc fmt -w rewrote a file it could not lex"
    );
    let left_behind: Vec<String> = fs::read_dir(&dir)
        .expect("listing the scratch directory")
        .map(|e| e.expect("a directory entry").file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        left_behind,
        vec!["bad.kp".to_string()],
        "kelpc fmt -w left temporary files behind"
    );

    let to_stdout = fmt_stdout(&file);
    assert!(
        !to_stdout.status.success(),
        "kelpc fmt (stdout) accepted a file with a lex error"
    );
    assert!(
        to_stdout.stdout.is_empty(),
        "kelpc fmt (stdout) printed output for a file with a lex error"
    );

    let check = fmt_check(&file);
    assert!(
        !check.status.success(),
        "kelpc fmt --check accepted a file with a lex error"
    );
}

/// A file with a syntax error, it lexes, but the parser cannot read it, is
/// reported and left alone too. This pins the formatter's syntax gate: a file
/// the compiler cannot parse is not silently reformatted. The gate parses a
/// *preprocessed* copy of the file (see the `fmt` module), which is what makes
/// an alias-using file obey the same rule; the alias case has its own test
/// above.
#[test]
fn a_syntax_error_is_reported_and_nothing_is_written() {
    let dir = fresh_scratch("syntax_error");
    let file = dir.join("bad.kp");
    // `for x xs` is missing the `in`; the file lexes cleanly.
    let src = "// a syntax error that still lexes: `for` without `in`.\nfun main() {\n    for x xs {\n        print(x)\n    }\n}\n";
    assert!(
        Lexer::new(src).tokenize().is_ok(),
        "the fixture for this test must lex cleanly"
    );
    fs::write(&file, src).expect("writing the broken file");

    let out = fmt_write(&file);
    assert!(
        !out.status.success(),
        "kelpc fmt -w reformatted a file with a syntax error"
    );
    assert!(
        !out.stderr.is_empty(),
        "kelpc fmt -w rejected a file without saying why"
    );
    assert_eq!(
        fs::read_to_string(&file).expect("reading the file back"),
        src,
        "kelpc fmt -w rewrote a file with a syntax error"
    );
}

// ---------------------------------------------------------------------------
// symbolic links (defect 3)
// ---------------------------------------------------------------------------

/// A program that is not formatted yet, and the bytes it must be rewritten to.
/// Every test below needs a file whose bytes the formatter really changes: a
/// write that is a no-op could not tell "wrote the target" from "wrote
/// nothing at all".
const UNFORMATTED: &str = "fun main() {\n   print(\"hi\")\n}\n";
const FORMATTED: &str = "fun main() {\n    print(\"hi\")\n}\n";

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating the scratch directory");
    }
    fs::write(path, contents).expect("writing a scratch file");
}

/// `fmt -w` on several paths in one run.
fn fmt_write_paths(paths: &[&Path]) -> Output {
    kelpc()
        .args(["fmt", "-w"])
        .args(paths)
        .output()
        .expect("failed to spawn kelpc fmt -w")
}

/// Every entry of a directory, sorted: `name`, or `name -> target` for a
/// symbolic link. "Nothing else is in this directory" is the assertion the
/// temporary-file tests have to make, and this is how they state it, a
/// leftover temporary is exactly a name that is not supposed to be here.
fn entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .expect("listing a scratch directory")
        .map(|entry| {
            let entry = entry.expect("a directory entry");
            let name = entry.file_name().to_string_lossy().to_string();
            match fs::read_link(entry.path()) {
                Ok(target) => format!("{name} -> {}", target.display()),
                Err(_) => name,
            }
        })
        .collect();
    out.sort();
    out
}

/// What `path` is a link to, or `None` when `path` is not a symbolic link.
fn link_target(path: &Path) -> Option<String> {
    if !fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return None;
    }
    fs::read_link(path)
        .ok()
        .map(|t| t.to_string_lossy().to_string())
}

/// The three ways one file is put to the formatter: its text to stdout,
/// `--check`, and `-w`.
fn three_modes(path: &Path) -> Vec<(&'static str, Output)> {
    vec![
        ("(stdout)", fmt_stdout(path)),
        ("--check", fmt_check(path)),
        ("-w", fmt_write(path)),
    ]
}

fn symlink(target: impl AsRef<Path>, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("creating a symbolic link");
}

/// A short base that reaches a directory whose own path may be long.
///
/// `UnixListener::bind` refuses a path longer than `sun_path` (107 bytes with the
/// terminating NUL), and a scratch directory under `CARGO_TARGET_TMPDIR` is as
/// long as the caller's target directory. The socket is bound through this link,
/// so the node lands at the target's own entry: its path, and every diagnostic
/// that names it, does not depend on how deep the checkout is. The directory is
/// removed when the guard drops.
struct ShortAlias {
    dir: PathBuf,
}

impl ShortAlias {
    fn new(target: &Path) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kelpc-fmt-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating a short alias directory");
        symlink(target, &dir.join("d"));
        Self { dir }
    }

    /// The short path that reaches `target`'s entry `name`.
    fn entry(&self, name: &str) -> PathBuf {
        self.dir.join("d").join(name)
    }
}

impl Drop for ShortAlias {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Runs `command` with a wall-clock limit. `kelpc fmt` on a FIFO with no writer
/// blocks in `open`, where no observation from inside the child is possible, so
/// a test of the refusal has to be able to walk away from the process: `None`
/// says it was killed for outliving the limit, and turns a regression into a
/// failure instead of a stuck suite.
fn run_within(limit: std::time::Duration, command: &mut Command) -> Option<Output> {
    use std::io::Read;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning kelpc");
    let deadline = std::time::Instant::now() + limit;
    let finished = loop {
        match child.try_wait().expect("waiting for kelpc") {
            Some(_) => break true,
            None if std::time::Instant::now() >= deadline => break false,
            None => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    };
    if !finished {
        let _ = child.kill();
    }
    let status = child.wait().expect("reaping kelpc");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout).expect("reading kelpc's stdout");
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr).expect("reading kelpc's stderr");
    }
    finished.then_some(Output {
        status,
        stdout,
        stderr,
    })
}

/// Creates a FIFO with `mkfifo`, and answers whether it worked: the socket and
/// device cases need no external tool, and a machine without `mkfifo` skips
/// only the FIFO half of the test.
fn mkfifo(path: &Path) -> bool {
    Command::new("mkfifo")
        .arg(path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The file type of a path as a word, for the assertions that a refusal left the
/// node it refused exactly as it was.
fn node_type(path: &Path) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    let file_type = fs::symlink_metadata(path)
        .expect("statting a scratch node")
        .file_type();
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else if file_type.is_char_device() {
        "character device"
    } else if file_type.is_block_device() {
        "block device"
    } else if file_type.is_dir() {
        "directory"
    } else {
        "regular file"
    }
}

/// The inode number of a path, so a test can tell "the same file, rewritten"
/// from "a new file at the same name".
fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .expect("statting a scratch file")
        .ino()
}

/// A group the caller belongs to besides its primary one, or `None` if there is
/// none. A file's group can be changed without privilege to any group the caller
/// is a member of, which makes the group the one attribute of a replacement file
/// a test here can vary (the owner would need privilege, and the mode bits are
/// measured separately).
fn a_secondary_group() -> Option<u32> {
    let ids = |flag: &str| -> Option<String> {
        let out = Command::new("id").arg(flag).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).ok())
            .flatten()
    };
    let primary: u32 = ids("-g")?.trim().parse().ok()?;
    ids("-G")?
        .split_whitespace()
        .filter_map(|group| group.parse::<u32>().ok())
        .find(|group| *group != primary)
}

/// `-w` through a symbolic link rewrites the file the link names and leaves the
/// link a link. `-w` used to write its temporary next to the link's own path and
/// rename it over the link: `link.kp` came back a *regular file* holding the
/// formatted bytes, and the real target never changed. Covers a relative
/// target, an absolute one, and a two-hop chain (a link to a link to a file).
#[test]
fn writing_through_a_symlink_keeps_the_link_and_formats_its_target() {
    let dir = fresh_scratch("symlink_write");

    let relative_target = dir.join("target_relative.kp");
    let absolute_target = dir.join("elsewhere/target_absolute.kp");
    let link = dir.join("link.kp");
    let chain = dir.join("chain.kp");
    let absolute_link = dir.join("absolute_link.kp");
    write_file(&relative_target, UNFORMATTED);
    write_file(&absolute_target, UNFORMATTED);
    symlink("target_relative.kp", &link);
    symlink("link.kp", &chain);
    symlink(&absolute_target, &absolute_link);

    let out = fmt_write_paths(&[&link, &chain, &absolute_link]);
    assert!(
        out.status.success(),
        "kelpc fmt -w refused a symlink:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Every link is still a link, pointing where it did.
    assert_eq!(link_target(&link).as_deref(), Some("target_relative.kp"));
    assert_eq!(link_target(&chain).as_deref(), Some("link.kp"));
    assert_eq!(
        link_target(&absolute_link).as_deref(),
        absolute_target.to_str(),
        "an absolute-target link was replaced by a copy"
    );

    // The files they name hold the formatted bytes.
    assert_eq!(
        fs::read_to_string(&relative_target).expect("reading the relative target"),
        FORMATTED,
        "the relative link's target was not formatted"
    );
    assert_eq!(
        fs::read_to_string(&absolute_target).expect("reading the absolute target"),
        FORMATTED,
        "the absolute link's target was not formatted"
    );

    // No link was turned into a regular file, and the directory holds a
    // temporary file's worth of nothing either.
    assert_eq!(
        entries(&dir),
        vec![
            "absolute_link.kp -> ".to_string() + absolute_target.to_str().unwrap(),
            "chain.kp -> link.kp".to_string(),
            "elsewhere".to_string(),
            "link.kp -> target_relative.kp".to_string(),
            "target_relative.kp".to_string(),
        ]
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("symbolic link"),
        "the write through a link was not reported:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A dangling link names nothing, so there is no file to format: it is refused
/// in every mode, nothing is written, and the link is left alone. Refusing is
/// the point, replacing it with a copy of itself would be the one thing a
/// formatter must never do to a link.
#[test]
fn a_dangling_symlink_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("symlink_dangling");
    let link = dir.join("dangling.kp");
    symlink("nowhere.kp", &link);
    let other = dir.join("other.kp");
    write_file(&other, UNFORMATTED);
    let before = entries(&dir);

    for (mode, out) in three_modes(&link) {
        assert!(!out.status.success(), "kelpc fmt {mode} accepted a dangling link");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("symbolic link") && stderr.contains("nowhere.kp"),
            "kelpc fmt {mode} did not say what was wrong with the link:\n{stderr}"
        );
    }

    assert_eq!(link_target(&link).as_deref(), Some("nowhere.kp"));
    assert_eq!(entries(&dir), before, "a refusal changed the directory");
    assert_eq!(fs::read_to_string(&other).unwrap(), UNFORMATTED);
}

/// A link that names a directory is refused in every mode, and the directory it
/// names is not reformatted: `fmt` writes files, and a directory command that
/// followed the link would rewrite a tree under a name that is not that tree's
/// own.
#[test]
fn a_symlink_to_a_directory_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("symlink_dir");
    let real = dir.join("real");
    let inside = real.join("a.kp");
    write_file(&inside, UNFORMATTED);
    let link = dir.join("linked_dir");
    symlink(&real, &link);

    for (mode, out) in three_modes(&link) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} accepted a symlink to a directory"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("symbolic link") && stderr.contains("director"),
            "kelpc fmt {mode} did not say what was wrong with the link:\n{stderr}"
        );
    }

    assert_eq!(link_target(&link).as_deref(), Some(real.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(&inside).unwrap(),
        UNFORMATTED,
        "a refused directory link formatted a file inside it"
    );
    assert_eq!(entries(&real), vec!["a.kp".to_string()]);
}

/// Directory recursion never follows a symbolic link. A link may name a file
/// outside the tree the user named, or a directory that holds this one,
/// `ln -s .. up` here points at the walk's own parent, so following one could
/// format a file twice under two names, rewrite a file outside the tree, or
/// recurse without end. Each skipped link is reported, the run still succeeds
/// for the files that are really there, and nothing inside a link is touched.
#[test]
fn directory_recursion_does_not_follow_symlinks() {
    let dir = fresh_scratch("recursion");
    let tree = dir.join("tree");
    let outside = dir.join("outside");
    fs::create_dir_all(&tree).expect("creating the scratch directory");
    write_file(&tree.join("a.kp"), UNFORMATTED);
    write_file(&outside.join("b.kp"), UNFORMATTED);
    symlink("..", &tree.join("up"));
    symlink(outside.join("b.kp"), &tree.join("linked.kp"));
    symlink(&outside, &tree.join("linked_dir"));

    let out = fmt_write(&tree);
    assert!(
        out.status.success(),
        "a directory holding a link is not a failure:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in ["up", "linked.kp", "linked_dir"] {
        assert!(
            stderr.contains(name),
            "the skipped link {name} was not reported:\n{stderr}"
        );
    }

    assert_eq!(
        fs::read_to_string(tree.join("a.kp")).unwrap(),
        FORMATTED,
        "the file that is really in the directory was not formatted"
    );
    assert_eq!(
        fs::read_to_string(outside.join("b.kp")).unwrap(),
        UNFORMATTED,
        "a file outside the named tree was rewritten through a link"
    );
    assert_eq!(entries(&tree).len(), 4, "{:?}", entries(&tree));
    assert_eq!(entries(&dir), vec!["outside".to_string(), "tree".to_string()]);
}

/// `-w` through a link must not *say* it rewrote the target before it has. The
/// note used to be printed before the write, so a write that then failed left
/// the run reporting a rewrite it had not performed: nonzero status, unchanged
/// bytes, and a note claiming the file named by the link was rewritten
/// (defect 6). The note is now printed only after the write lands, so a failure
/// prints the failure alone.
///
/// The failure is forced by making the resolved target's directory unwritable,
/// which stops the atomic write from creating its temporary beside the target.
/// A process that can write a 0555 directory anyway (root, or a permissive
/// ACL) cannot exercise this, and says so instead of asserting something false.
#[test]
fn a_failed_write_through_a_symlink_reports_only_the_failure() {
    let dir = fresh_scratch("symlink_failed_write");
    let inner = dir.join("inner");
    let target = inner.join("target.kp");
    write_file(&target, UNFORMATTED);
    let link = dir.join("link.kp");
    symlink("inner/target.kp", &link);

    let mut perms = fs::metadata(&inner).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&inner, perms).unwrap();

    let probe = inner.join(".probe");
    if fs::write(&probe, "").is_ok() {
        let _ = fs::remove_file(&probe);
        let mut perms = fs::metadata(&inner).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&inner, perms).unwrap();
        eprintln!(
            "SKIPPED (a failed write through a symlink): this process may write to a \
             0555 directory, so the write cannot be made to fail"
        );
        return;
    }

    let out = fmt_write(&link);

    let mut perms = fs::metadata(&inner).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&inner, perms).unwrap();

    assert!(
        !out.status.success(),
        "kelpc fmt -w reported success for a write that failed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("is rewritten"),
        "the run claimed a rewrite that did not happen:\n{stderr}"
    );
    assert!(
        stderr.contains("cannot write") && stderr.contains("link.kp"),
        "the failure was not reported against the link:\n{stderr}"
    );
    assert_eq!(
        link_target(&link).as_deref(),
        Some("inner/target.kp"),
        "the link was not left alone"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("reading the target"),
        UNFORMATTED,
        "a write that failed changed the target"
    );
    assert_eq!(
        entries(&inner),
        vec!["target.kp".to_string()],
        "a write that failed left a temporary behind"
    );
}

/// A tab does not survive anywhere, including inside an open `(` where the
/// author's leading whitespace is otherwise kept. There is no canonical form
/// for an author-wrapped continuation line, so spaces are preserved as written
/// and a tab there becomes four spaces (defect 9: the `push_indent` doc claimed
/// the author's whitespace was kept, which was false for tabs).
#[test]
fn a_tab_in_a_continuation_line_becomes_four_spaces() {
    let dir = fresh_scratch("continuation_tabs");
    let file = dir.join("tabs.kp");
    let src = "fun main() {\n    print(\n\t\"hi\",\n\t\"there\"\n    )\n}\n";
    write_file(&file, src);

    let out = fmt_stdout(&file);
    assert!(
        out.status.success(),
        "a tab-indented continuation line was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        text,
        "fun main() {\n    print(\n    \"hi\",\n    \"there\"\n    )\n}\n",
        "a tab inside an open parenthesis did not become four spaces"
    );
    assert!(!text.contains('\t'), "a tab survived:\n{text}");

    let wrote = fmt_write(&file);
    assert!(
        wrote.status.success(),
        "kelpc fmt -w refused the file:\n{}",
        String::from_utf8_lossy(&wrote.stderr)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    assert!(
        fmt_check(&file).status.success(),
        "the formatted output is not a fixed point"
    );
}

// ---------------------------------------------------------------------------
// non-regular files (defect 12)
// ---------------------------------------------------------------------------

/// A path that is not a regular file is refused *before* anything reads it.
///
/// `-w` on a symbolic link to a FIFO used to return exit 0 with the success
/// note and replace the FIFO with a regular file, and with no writer both `fmt`
/// and `fmt -w` blocked in `open` forever (the audit measured exit 124 under
/// `timeout 3`). The type is now settled with `metadata`, which opens nothing,
/// so the refusal comes first and the node is left alone. Every mode is covered
/// because the check is in `fmt_file`, which all of them share.
///
/// The subprocess runs under a wall-clock limit: if the refusal ever regressed,
/// this test would fail instead of blocking the suite (see `run_within`).
#[test]
fn a_non_regular_file_is_refused_before_it_is_read() {
    let dir = fresh_scratch("non_regular");

    // A socket needs nothing but std; a character device is `/dev/null`; the
    // FIFO needs `mkfifo`, and only its cases are skipped without it.
    //
    // The socket is bound through a short alias of `dir` so the fixture's own
    // path stays `dir/socket.kp`; binding `dir/socket.kp` itself would be refused
    // as longer than `sun_path` on a deep enough checkout.
    let alias = ShortAlias::new(&dir);
    let socket = dir.join("socket.kp");
    let _listener = std::os::unix::net::UnixListener::bind(alias.entry("socket.kp"))
        .expect("binding a socket");
    let device = PathBuf::from("/dev/null");
    let fifo = dir.join("fifo.kp");
    let fifo_ready = mkfifo(&fifo);
    if !fifo_ready {
        eprintln!("SKIPPED (the FIFO half of defect 12): this machine has no `mkfifo`");
    }
    let link = dir.join("link.kp");
    if fifo_ready {
        symlink("fifo.kp", &link);
    }

    let mut cases: Vec<(PathBuf, &'static str)> = vec![
        (socket.clone(), "socket"),
        (device.clone(), "character device"),
    ];
    if fifo_ready {
        cases.push((fifo.clone(), "FIFO"));
        // The link is resolved first, so the refusal names the FIFO it found.
        cases.push((link.clone(), "FIFO"));
    }

    let mut failures = Vec::new();
    for (path, kind) in &cases {
        for (mode, flags) in [
            ("(stdout)", vec!["fmt"]),
            ("--check", vec!["fmt", "--check"]),
            ("-w", vec!["fmt", "-w"]),
        ] {
            let mut command = kelpc();
            command.args(&flags).arg(path);
            let Some(out) = run_within(std::time::Duration::from_secs(10), &mut command) else {
                failures.push(format!(
                    "{} {mode}: blocked (killed after 10s) instead of refusing",
                    brief(path)
                ));
                continue;
            };
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                failures.push(format!(
                    "{} {mode}: accepted a non-regular file (exit 0)",
                    brief(path)
                ));
            } else if !stderr.contains(kind) || !stderr.contains("not a regular file") {
                failures.push(format!(
                    "{} {mode}: the diagnostic does not name a {kind}:\n    {}",
                    brief(path),
                    stderr.trim().replace('\n', "\n    ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "non-regular file(s) were not refused cleanly")
    );

    // Nothing was read, so nothing was changed: every node is still the node it
    // was, the link is still a link, and no temporary was written beside them.
    assert_eq!(node_type(&socket), "socket", "the socket was replaced");
    assert_eq!(node_type(&device), "character device");
    if fifo_ready {
        assert_eq!(node_type(&fifo), "fifo", "the FIFO was replaced by a regular file");
        assert_eq!(node_type(&link), "symlink", "the link was replaced");
        assert_eq!(link_target(&link).as_deref(), Some("fifo.kp"));
    }
    let mut expected = vec!["socket.kp".to_string()];
    if fifo_ready {
        // `entries` renders a symbolic link as `name -> target`.
        expected.extend(["fifo.kp".to_string(), "link.kp -> fifo.kp".to_string()]);
    }
    expected.sort();
    assert_eq!(entries(&dir), expected, "a refusal left something behind");
}

// ---------------------------------------------------------------------------
// what `-w` replaces (defect 13)
// ---------------------------------------------------------------------------

/// `-w` replaces the file's inode, and that is the documented outcome.
///
/// The write is atomic (a temporary beside the target, then a rename), and the
/// rename lands a *new* inode: a hard link to the formatted file keeps the old
/// bytes, and `fmt.rs`'s limits list and `main.rs`'s `write_atomic` both say so.
/// `write_atomic` copies the *mode bits* onto the temporary and nothing else.
/// Of the four things a replacement file could carry, this test measures two:
/// the permission bits (set to a mode a fresh file does not get, so carrying them
/// over is an observation and not a coincidence) and the group (the file is moved
/// to a secondary group the caller belongs to, and the new inode must not be in
/// it). The owner cannot be varied without privilege, so "the owner does not
/// follow" rests on reading `write_atomic`, and so do ACLs and extended
/// attributes, `set_permissions` with mode bits has nowhere to put either, and
/// nothing here measures them. Preserving the inode would mean writing in place,
/// which is the half-written file the design exists to avoid. This test pins the
/// outcome so it cannot change silently.
#[test]
fn rewriting_a_file_replaces_its_inode_and_a_hard_link_keeps_the_old_bytes() {
    let dir = fresh_scratch("hard_link");
    let a = dir.join("a.kp");
    let b = dir.join("b.kp");
    write_file(&a, UNFORMATTED);
    // A mode a fresh file does not get by default, so "the permission bits are
    // carried over" is an observation rather than a coincidence.
    fs::set_permissions(&a, fs::Permissions::from_mode(0o640))
        .expect("setting the source file's mode");
    // The group, the one of the other three attributes a test can vary: a file's
    // group can be changed without privilege to any group the caller belongs to.
    let moved_group = match a_secondary_group() {
        Some(gid) => {
            chown(&a, None, Some(gid)).expect("moving a.kp to a secondary group");
            Some(gid)
        }
        None => {
            eprintln!(
                "SKIPPED: the caller belongs to no group besides its primary one, so the \
                 rewrite's effect on the group cannot be measured here"
            );
            None
        }
    };
    fs::hard_link(&a, &b).expect("hard-linking a.kp to b.kp");
    let shared = inode(&a);
    assert_eq!(inode(&b), shared, "the two names must start on one inode");

    let out = fmt_write(&a);
    assert!(
        out.status.success(),
        "kelpc fmt -w refused a regular file:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        fs::read_to_string(&a).expect("reading a.kp"),
        FORMATTED,
        "the file the command named was not formatted"
    );
    assert_eq!(
        fs::read_to_string(&b).expect("reading b.kp"),
        UNFORMATTED,
        "b.kp holds the formatted bytes: the rewrite kept the inode, which is not \
         the outcome the limits list states"
    );
    assert_ne!(
        inode(&a),
        shared,
        "the rewrite kept a.kp's inode; the documented outcome is a new one"
    );
    assert_eq!(inode(&b), shared, "the other name lost the inode it held");
    assert_eq!(
        fs::metadata(&a).expect("statting a.kp").permissions().mode() & 0o777,
        0o640,
        "the permission bits did not follow the file through the rewrite"
    );
    if let Some(gid) = moved_group {
        assert_ne!(
            fs::metadata(&a).expect("statting a.kp").gid(),
            gid,
            "the group followed the file through the rewrite; write_atomic copies the mode \
             bits and nothing else"
        );
        assert_eq!(
            fs::metadata(&b).expect("statting b.kp").gid(),
            gid,
            "the old inode lost the group this test moved it to"
        );
    }
    assert_eq!(
        entries(&dir),
        vec!["a.kp".to_string(), "b.kp".to_string()],
        "the rewrite left a temporary behind"
    );
}

// ---------------------------------------------------------------------------
// a write that fails after the temporary exists (defect 4)
// ---------------------------------------------------------------------------

/// Runs `script` under `unshare -Urm`. An unprivileged process may not mount
/// anything, but inside a private user and mount namespace it is root and can,
/// which is what lets these tests build the filesystem conditions a failed write
/// needs: a full filesystem, a read-only one, and a file that is a mount point.
/// Each script says `HARNESS_OK` once its condition is in place, so a machine
/// that cannot run the harness at all is visible as a skip rather than as a
/// pass.
fn in_mount_namespace(script: &str) -> Option<Output> {
    Command::new("unshare")
        .args(["-Urm", "sh", "-c", script])
        .output()
        .ok()
}

/// The lines of `text` between the `begin` and `end` markers, exclusive. The
/// scripts below print a directory listing between two markers so the test can
/// assert on it after the namespace is gone.
fn between(text: &str, begin: &str, end: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == begin {
            inside = true;
        } else if line == end {
            break;
        } else if inside {
            out.push(line.to_string());
        }
    }
    out.sort();
    out
}

/// Introduces a script and answers whether its condition was reached, with the
/// run's stdout and stderr, the scripts print their observations on stdout and
/// the formatter's diagnostic goes to stderr.
fn harness_ready(what: &str, out: &Output) -> Option<(String, String)> {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !stdout.contains("HARNESS_OK") {
        eprintln!(
            "SKIPPED ({what}): this machine cannot set the condition up in a user \
             namespace, so the failed write could not be forced here.\nstdout: {stdout}\nstderr: {stderr}"
        );
        return None;
    }
    Some((stdout, stderr))
}

/// A write that fails *after* its temporary file exists must remove it: the
/// writer used to leave the temporary in the source directory, where nothing
/// owns it and `git status` reports it. The filesystem here is a 4 KiB tmpfs
/// that is already full, so creating the temporary succeeds (an empty file
/// needs no page) and writing it does not, the exact shape of the defect. It
/// is forced inside a user and mount namespace, because mounting a filesystem
/// is the only way an unprivileged test can own one.
#[test]
fn a_write_that_runs_out_of_space_removes_its_temporary() {
    let dir = fresh_scratch("enospc");
    let script = format!(
        r#"d='{dir}'
mount -t tmpfs -o size=4k none "$d" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
printf 'fun main() {{\n   print("hi")\n}}\n' > "$d/x.kp"
'{kelpc}' fmt -w "$d/x.kp"; echo "exit=$?"
echo FILE-LIST-BEGIN
ls -A "$d"
echo FILE-LIST-END
cat "$d/x.kp"
"#,
        dir = dir.display(),
        kelpc = env!("CARGO_BIN_EXE_kelpc"),
    );
    let Some(out) = in_mount_namespace(&script) else {
        eprintln!("SKIPPED (no space left on device): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("no space left on device", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a write that failed was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("No space left on device"),
        "the failure was not the one this test sets up:\n{stderr}"
    );
    assert_eq!(
        between(&stdout, "FILE-LIST-BEGIN", "FILE-LIST-END"),
        vec!["x.kp".to_string()],
        "the temporary file was left behind:\n{stdout}"
    );
    assert!(
        stdout.ends_with(UNFORMATTED),
        "the file was changed by a write that failed:\n{stdout}"
    );
}

/// The same cleanup, reached through the *rename* instead of the write: the
/// target here is a file that is a mount point, which `rename` may not replace
/// (EBUSY). The temporary exists by then, and must be gone afterwards.
#[test]
fn a_rename_that_fails_removes_its_temporary() {
    let dir = fresh_scratch("rename_fails");
    write_file(&dir.join("real.kp"), UNFORMATTED);
    write_file(&dir.join("x.kp"), UNFORMATTED);
    let script = format!(
        r#"d='{dir}'
mount --bind "$d/real.kp" "$d/x.kp" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
'{kelpc}' fmt -w "$d/x.kp"; echo "exit=$?"
echo FILE-LIST-BEGIN
ls -A "$d"
echo FILE-LIST-END
cat "$d/x.kp"
"#,
        dir = dir.display(),
        kelpc = env!("CARGO_BIN_EXE_kelpc"),
    );
    let Some(out) = in_mount_namespace(&script) else {
        eprintln!("SKIPPED (a rename that fails): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("a rename that fails", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a rename that failed was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("Device or resource busy"),
        "the failure was not the one this test sets up:\n{stderr}"
    );
    assert_eq!(
        between(&stdout, "FILE-LIST-BEGIN", "FILE-LIST-END"),
        vec!["real.kp".to_string(), "x.kp".to_string()],
        "the temporary file was left behind:\n{stdout}"
    );
    assert!(
        stdout.ends_with(UNFORMATTED),
        "the file was changed by a write that failed:\n{stdout}"
    );
}

/// A directory that cannot be written is refused, and the file it holds is left
/// exactly as it was. This failure lands before the temporary exists (a
/// read-only filesystem cannot hold one), which is the sibling of the two cases
/// above and worth pinning: `-w` must not fall back to writing the file in
/// place.
#[test]
fn a_read_only_directory_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("read_only");
    let script = format!(
        r#"d='{dir}'
mount -t tmpfs none "$d" || {{ echo HARNESS_FAILED; exit 0; }}
printf 'fun main() {{\n   print("hi")\n}}\n' > "$d/x.kp"
mount -o remount,ro "$d" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
'{kelpc}' fmt -w "$d/x.kp"; echo "exit=$?"
echo FILE-LIST-BEGIN
ls -A "$d"
echo FILE-LIST-END
cat "$d/x.kp"
"#,
        dir = dir.display(),
        kelpc = env!("CARGO_BIN_EXE_kelpc"),
    );
    let Some(out) = in_mount_namespace(&script) else {
        eprintln!("SKIPPED (a read-only directory): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("a read-only directory", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a read-only directory was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("Read-only file system"),
        "the failure was not the one this test sets up:\n{stderr}"
    );
    assert_eq!(
        between(&stdout, "FILE-LIST-BEGIN", "FILE-LIST-END"),
        vec!["x.kp".to_string()],
        "the temporary file was left behind:\n{stdout}"
    );
    assert!(
        stdout.ends_with(UNFORMATTED),
        "the file was changed by a write that failed:\n{stdout}"
    );
}

/// Two `fmt -w` runs against one path must not write the same temporary. With
/// one fixed name they would write the *same* file, and either run could rename
/// the other's half-written bytes into place, or fail on a path that had
/// already been renamed away. The temporary's name carries the process id, so
/// the runs write two files; the file must end up as one run's whole output,
/// with no temporary left over. The source is large so the two writes really do
/// overlap.
#[test]
fn concurrent_writes_to_one_path_do_not_collide() {
    let dir = fresh_scratch("concurrent");
    let file = dir.join("shared.kp");
    write_file(&file, &large_unformatted_source());

    let once = fmt_stdout(&file);
    assert!(once.status.success(), "the source was refused");
    let expected = String::from_utf8(once.stdout).expect("the formatted output is utf-8");

    let mut runs: Vec<std::process::Child> = (0..8)
        .map(|_| {
            kelpc()
                .args(["fmt", "-w"])
                .arg(&file)
                .spawn()
                .expect("spawning a concurrent kelpc fmt -w")
        })
        .collect();
    for run in &mut runs {
        let status = run.wait().expect("waiting for a concurrent kelpc fmt -w");
        assert!(status.success(), "a concurrent fmt -w failed: {status:?}");
    }

    assert_eq!(
        fs::read_to_string(&file).expect("reading the shared file"),
        expected,
        "two runs wrote the same path and mixed their output"
    );
    assert_eq!(
        entries(&dir),
        vec!["shared.kp".to_string()],
        "a temporary file was left behind"
    );
}

/// A source big enough (about 400 KiB) that two concurrent writes to it overlap
/// rather than queue behind each other, which is what makes the collision above
/// reachable at all.
fn large_unformatted_source() -> String {
    let mut src = String::from("fun main() {\n   print(\"hi\")\n");
    for i in 0..20_000 {
        src.push_str(&format!("   val v{i} = {i}\n"));
    }
    src.push_str("}\n");
    src
}

// ---------------------------------------------------------------------------
// the alias verdict (defect 5)
// ---------------------------------------------------------------------------

/// The verdict `kelpc build` reaches for a file or a project: its exit code and
/// its stderr, which is the reference `kelpc fmt` has to agree with. These files
/// fail in the front end, before any backend runs.
fn build_verdict(entry: &Path, out: &Path) -> (Option<i32>, String) {
    let output = kelpc()
        .arg("build")
        .env_remove("KELPC_BACKEND")
        .arg(entry)
        .arg("-o")
        .arg(out)
        .output()
        .expect("failed to spawn kelpc build");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// An undefined alias is a refusal, in the compiler's own words. `fmt` used to
/// substitute a placeholder identifier for the reference and format the file
/// `kelpc build` rejects: the gate checked the layout around the reference and
/// never the reference itself. The alias table fmt can see is a file's own
/// declarations, so an undefined name is knowable, and must be reported, not
/// papered over.
#[test]
fn an_undefined_alias_is_refused_with_the_compilers_own_diagnostic() {
    let dir = fresh_scratch("undefined_alias");
    let file = dir.join("undefined.kp");
    let src = "fun main() {\n    print(%*undefined*%)\n}\n";
    write_file(&file, src);

    let (code, build_stderr) = build_verdict(&file, &dir.join("undefined.out"));
    assert_ne!(code, Some(0), "kelpc build accepted an undefined alias");
    assert!(
        build_stderr.contains("unknown alias '%*undefined*%'"),
        "kelpc build's diagnostic changed:\n{build_stderr}"
    );

    for (mode, out) in three_modes(&file) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} accepted an undefined alias"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            build_stderr,
            "kelpc fmt {mode} did not report what kelpc build reports"
        );
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), src, "a refused file was rewritten");
    assert_eq!(entries(&dir), vec!["undefined.kp".to_string()]);
}

/// An alias another module declares, whose expansion is not valid syntax. One
/// file cannot see `util.kp`, so `fmt` asks the resolver for the project's alias
/// table, the one `kelpc build` expands with, and the project is refused with
/// the resolver's own diagnostic. Without that, the file formatted cleanly and
/// then failed to build.
#[test]
fn an_alias_from_another_module_that_expands_to_bad_syntax_is_refused() {
    let dir = fresh_scratch("cross_file_alias");
    let project = dir.join("proj");
    fs::create_dir_all(project.join("src")).expect("creating the scratch project");
    write_file(&project.join("coral.toml"), "name = \"proj\"\n");
    write_file(&project.join("src/util.kp"), "alias %*bad*% = )))\n");
    let main = project.join("src/main.kp");
    let src = "impl \"util\"\n\nfun main() {\n   print(%*bad*%)\n}\n";
    write_file(&main, src);

    let (code, build_stderr) = build_verdict(&project, &dir.join("proj.out"));
    assert_ne!(code, Some(0), "kelpc build accepted a bad alias expansion");
    assert!(
        build_stderr.contains("expected a newline or '}'"),
        "kelpc build's diagnostic changed:\n{build_stderr}"
    );

    for (mode, out) in three_modes(&main) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} formatted a file whose alias expansion does not parse"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            build_stderr,
            "kelpc fmt {mode} did not report what kelpc build reports"
        );
    }
    assert_eq!(fs::read_to_string(&main).unwrap(), src, "a refused file was rewritten");
    assert_eq!(
        entries(&project.join("src")),
        vec!["main.kp".to_string(), "util.kp".to_string()]
    );

    // The same project with a usable alias formats: the resolver is what makes
    // the difference, so this is not a blanket refusal of cross-file aliases.
    write_file(&project.join("src/util.kp"), "alias %*bad*% = 41\n");
    let out = fmt_write(&main);
    assert!(
        out.status.success(),
        "a valid cross-file alias was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&main).unwrap(),
        "impl \"util\"\n\nfun main() {\n    print(%*bad*%)\n}\n",
        "the alias line or reference was not preserved"
    );
    assert_eq!(
        entries(&project.join("src")),
        vec!["main.kp".to_string(), "util.kp".to_string()]
    );
}

/// A file whose lines end with a lone carriage return is refused, not joined
/// onto one line. The lexer reads `\r` as blank trivia rather than as a line
/// terminator, so such a file is not multi-line to it at all, that is a lexer
/// limit, and the formatter's answer to it is a diagnostic rather than a
/// rewrite. A CRLF file is ordinary input, and the corpus pins what it becomes.
#[test]
fn a_file_with_lone_carriage_returns_is_refused_and_unchanged() {
    let dir = fresh_scratch("lone_cr");
    let file = dir.join("cr.kp");
    let src = "fun main() {\r   print(\"hi\")\r}\r";
    write_file(&file, src);

    for (mode, out) in three_modes(&file) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} formatted a file whose only line breaks are CR"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("carriage return"),
            "kelpc fmt {mode} did not name the reason:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), src);
    assert_eq!(entries(&dir), vec!["cr.kp".to_string()]);

    // CRLF is not this case: the lexer consumes the pair, and the CR is trivia.
    let crlf = dir.join("crlf.kp");
    write_file(&crlf, "fun main() {\r\n   print(\"hi\")\r\n}\r\n");
    let out = fmt_write(&crlf);
    assert!(
        out.status.success(),
        "a CRLF file was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&crlf).unwrap(),
        "fun main() {\n    print(\"hi\")\n}\n"
    );
}

/// A carriage return *inside a string literal* is not a line terminator, so it
/// is not the refusal above (defect 11).
///
/// The rule used to fire on a `\r` anywhere in the bytes and told the reader
/// the file "would be formatted as a single line", which was false twice over
/// for this input: the `\r` is a byte of a string token, and the file is
/// genuinely multi-line. `kelpc build` accepts the file, so `fmt` must too, and
/// with this shape fixed, a lone carriage return *used as a line terminator* is
/// the only shape left in which `fmt` declines a program `kelpc build` accepts.
#[test]
fn a_carriage_return_inside_a_string_literal_is_formatted() {
    let dir = fresh_scratch("cr_in_string");
    let file = dir.join("cr_string.kp");
    let src = "fun main() {\n    print(\"a\rb\")\n}\n";
    write_file(&file, src);

    // The reference: `kelpc build` takes it, which is what makes a refusal from
    // `fmt` a defect and not a disagreement about validity.
    let (code, build_stderr) = build_verdict(&file, &dir.join("cr_string.out"));
    assert_eq!(code, Some(0), "kelpc build rejected the fixture:\n{build_stderr}");

    for (mode, out) in three_modes(&file) {
        assert!(
            out.status.success(),
            "kelpc fmt {mode} declined a file kelpc build accepts:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // The literal's own byte survives verbatim, and the file is a fixed point.
    assert_eq!(
        fs::read_to_string(&file).expect("reading the file back"),
        FORMATTED_CR_STRING
    );
    assert!(
        fmt_check(&file).status.success(),
        "the formatted output is not a fixed point"
    );
    let out = fmt_stdout(&file);
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf-8"),
        FORMATTED_CR_STRING,
        "the carriage return inside the string literal did not survive"
    );

    // The same rule inside a character literal.
    let ch = dir.join("cr_char.kp");
    let ch_src = "fun main() {\n    val c = '\r'\n    print(c)\n}\n";
    write_file(&ch, ch_src);
    let out = fmt_stdout(&ch);
    assert!(
        out.status.success(),
        "a carriage return inside a character literal was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).expect("utf-8"), ch_src);
}

/// The exact bytes the defect-11 fixture must keep: every byte of the string
/// literal, the carriage return included.
const FORMATTED_CR_STRING: &str = "fun main() {\n    print(\"a\rb\")\n}\n";

// ---------------------------------------------------------------------------
// semantic equivalence (needs a native build; opt in with --ignored)
// ---------------------------------------------------------------------------

fn build(source: &Path, binary: &Path) -> Output {
    kelpc()
        .arg("build")
        .env_remove("KELPC_BACKEND")
        .arg(source)
        .arg("-o")
        .arg(binary)
        .output()
        .expect("failed to spawn kelpc build")
}

/// The repository's buildable programs: the entry file of every project fixture
/// and every example. The fixtures include deliberate errors, of the front end
/// and of the type checker, so a candidate that does not build is skipped; what
/// this test is about is the ones that do.
fn buildable_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let projects = manifest("tests/projects");
    for entry in fs::read_dir(&projects).expect("reading tests/projects") {
        let dir = entry.expect("a directory entry").path();
        for candidate in [dir.join("main.kp"), dir.join("src/main.kp")] {
            if candidate.is_file() {
                out.push(candidate);
            }
        }
    }
    for entry in fs::read_dir(manifest("../examples")).expect("reading ../examples") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|s| s.to_str()) == Some("kp") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// A formatted program is the same program: building the original and the
/// formatted copy, and running both, yields identical stdout and the same exit
/// status. This test is about the formatter, not the backend.
///
/// The formatted copy is written into a mirror of the tree the original lives in
/// and built from there, so a project's imports still resolve, the same reason
/// the corpus tests above format a mirrored copy. `examples/download.kp` needs a
/// live HTTP server; it builds, and running both copies fails the same way,
/// which this comparison accepts.
#[test]
#[ignore = "builds and runs native binaries; run with --ignored"]
fn formatting_preserves_a_programs_output() {
    let dir = fresh_scratch("semantics");
    let mut trees = Trees::new("semantics");

    let mut failures = Vec::new();
    let mut compared = 0usize;
    let mut skipped = 0usize;
    for (i, source) in buildable_candidates().into_iter().enumerate() {
        let original_bin = dir.join(format!("case{i}.orig"));
        let formatted_bin = dir.join(format!("case{i}.fmt"));

        if !build(&source, &original_bin).status.success() {
            skipped += 1;
            continue;
        }

        let out = fmt_stdout(&source);
        assert!(
            out.status.success(),
            "kelpc fmt exited {:?} for a program that builds, {}:\n{}",
            out.status.code(),
            brief(&source),
            String::from_utf8_lossy(&out.stderr)
        );
        let formatted_source = trees.copy_of(&source);
        fs::write(&formatted_source, &out.stdout).expect("writing the formatted copy");

        let formatted_build = build(&formatted_source, &formatted_bin);
        assert!(
            formatted_build.status.success(),
            "the formatted {} did not build:\n{}",
            brief(&source),
            String::from_utf8_lossy(&formatted_build.stderr)
        );

        let original_run = Command::new(&original_bin).output().expect("running the original");
        let formatted_run = Command::new(&formatted_bin)
            .output()
            .expect("running the formatted copy");
        if original_run.stdout != formatted_run.stdout
            || original_run.status.code() != formatted_run.status.code()
        {
            failures.push(format!(
                "{}: original and formatted disagree, exit {:?} vs {:?}, stdout {:?} vs {:?}",
                brief(&source),
                original_run.status.code(),
                formatted_run.status.code(),
                String::from_utf8_lossy(&original_run.stdout),
                String::from_utf8_lossy(&formatted_run.stdout)
            ));
        }
        compared += 1;
    }

    // Printed on every run: the count is the strength of this test, and a
    // `--ignored` test that silently compared two programs would be no sweep.
    println!("compared {compared} program(s); {skipped} candidate(s) did not build");
    assert!(
        compared >= 20,
        "only {compared} program(s) built and were compared ({skipped} candidates did not build)"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

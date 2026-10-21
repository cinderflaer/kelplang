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
//!      preprocessed source that does not parse, and a `_name` reference that
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

#[path = "fmt/corpus.rs"]
mod corpus;
pub(crate) use corpus::*;

#[path = "fmt/mirror.rs"]
mod mirror;
pub(crate) use mirror::*;

#[path = "fmt/faults.rs"]
mod faults;
pub(crate) use faults::*;

#[path = "fmt/inodes.rs"]
mod inodes;
pub(crate) use inodes::*;

#[path = "fmt/renames.rs"]
mod renames;
pub(crate) use renames::*;

#[path = "fmt/carriage.rs"]
mod carriage;

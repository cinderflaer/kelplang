use super::*;

/// Each input's first line has to name the rule it pins, so a reader can tell a
/// regression from a rule change.
#[test]
pub(crate) fn every_corpus_input_names_the_rule_it_pins() {
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
pub(crate) fn every_corpus_pair_formats_to_its_expected_output() {
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
pub(crate) fn every_expected_output_is_token_faithful_to_its_input() {
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
pub(crate) fn a_slash_inside_a_string_is_not_a_comment() {
    let src = "val url = \"http://example.com\"\n// the only comment\n";
    assert_eq!(comments(src), vec!["// the only comment".to_string()]);
}

/// The expected file is the formatter's own fixed point: `--check` on it exits
/// zero. This is what makes a golden pair a statement about the *rule* and not
/// about one copy of the input.
#[test]
pub(crate) fn every_expected_output_is_a_fixed_point() {
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

pub(crate) fn collect_kp(dir: &Path, out: &mut Vec<PathBuf>) {
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
pub(crate) fn repository_corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in ["std", "tests/fixtures", "tests/projects"] {
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
pub(crate) fn context_root(file: &Path) -> PathBuf {
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
pub(crate) struct Trees {
    pub(crate) tag: &'static str,
    pub(crate) mirrored: HashMap<PathBuf, PathBuf>,
}

impl Trees {
    pub(crate) fn new(tag: &'static str) -> Self {
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
    /// cannot resolve a `_name`, and a copy with no `util.kp` beside it is a
    /// file whose alias table really is incomplete, fmt refuses it, and that
    /// refusal says nothing about formatting. Copying the tree gives the copy
    /// the module graph the original has, so the assertions below stay
    /// statements about formatting.
    pub(crate) fn copy_of(&mut self, file: &Path) -> PathBuf {
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

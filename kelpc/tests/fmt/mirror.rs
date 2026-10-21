use super::*;

/// Copies one tree into the scratch directory and returns its new root. Symbolic
/// links are not copied, the corpus has none, because `kelpc fmt -w` does not
/// follow them either.
pub(crate) fn mirror_tree(tag: &str, root: &Path) -> PathBuf {
    let dest = scratch(&format!("tree/{tag}/{}", scratch_name(root)));
    let _ = fs::remove_dir_all(&dest);
    copy_tree(root, &dest).expect("copying a corpus tree into scratch");
    dest
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
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
pub(crate) fn must_format(path: &Path) -> bool {
    let rel = brief(path);
    rel.starts_with("std/")
        || rel.starts_with("tests/projects/")
        || rel.starts_with("tests/fixtures/examples/")
}

#[test]
pub(crate) fn formatting_is_idempotent_token_faithful_and_comment_preserving() {
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
pub(crate) fn operator_adjacencies_are_never_silently_relexed() {
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
pub(crate) fn a_formatted_output_always_has_the_inputs_significant_tokens() {
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
pub(crate) fn an_alias_file_with_a_syntax_error_is_refused_and_unchanged() {
    let dir = fresh_scratch("alias_syntax_error");
    let file = dir.join("alias_bad.kp");
    // The exact reproduction: the alias line is fine, `print("oops"` is not,
    // and the raw stream cannot be parsed because `_greeting` is an alias.
    let src = "alias _greeting = \"hi\"\n\nfun main() {\n    print(_greeting)\n    print(\"oops\"\n}\n";
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
    let fixed = "alias _greeting = \"hi\"\n\nfun main() {\n   print(_greeting)\n   print(\"oops\")\n}\n";
    fs::write(&ok, fixed).expect("writing the fixed alias file");
    let out = fmt_stdout(&ok);
    assert!(
        out.status.success(),
        "a valid alias file was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("alias _greeting = \"hi\""),
        "the alias declaration changed:\n{text}"
    );
    assert!(
        text.contains("print(_greeting)"),
        "the alias reference stopped being atomic:\n{text}"
    );
}

use super::*;

/// A file whose lines end with a lone carriage return is refused, not joined
/// onto one line. The lexer reads `\r` as blank trivia rather than as a line
/// terminator, so such a file is not multi-line to it at all, that is a lexer
/// limit, and the formatter's answer to it is a diagnostic rather than a
/// rewrite. A CRLF file is ordinary input, and the corpus pins what it becomes.
#[test]
pub(crate) fn a_file_with_lone_carriage_returns_is_refused_and_unchanged() {
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
pub(crate) fn a_carriage_return_inside_a_string_literal_is_formatted() {
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
pub(crate) const FORMATTED_CR_STRING: &str = "fun main() {\n    print(\"a\rb\")\n}\n";

// ---------------------------------------------------------------------------
// semantic equivalence (needs a native build; opt in with --ignored)
// ---------------------------------------------------------------------------

pub(crate) fn build(source: &Path, binary: &Path) -> Output {
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
pub(crate) fn buildable_candidates() -> Vec<PathBuf> {
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
    for entry in fs::read_dir(manifest("tests/fixtures/examples")).expect("reading tests/fixtures/examples") {
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
pub(crate) fn formatting_preserves_a_programs_output() {
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

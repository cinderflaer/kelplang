use super::*;

/// A file the formatter cannot process is reported and left alone. The file
/// here fails to *lex* (an unknown string escape), which is the syntax error
/// that reaches the formatter first; the property test above covers the files
/// that lex but are still not valid Kelp.
#[test]
pub(crate) fn a_lex_error_is_reported_and_nothing_is_written() {
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
pub(crate) fn a_syntax_error_is_reported_and_nothing_is_written() {
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
pub(crate) const UNFORMATTED: &str = "fun main() {\n   print(\"hi\")\n}\n";
pub(crate) const FORMATTED: &str = "fun main() {\n    print(\"hi\")\n}\n";

pub(crate) fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating the scratch directory");
    }
    fs::write(path, contents).expect("writing a scratch file");
}

/// `fmt -w` on several paths in one run.
pub(crate) fn fmt_write_paths(paths: &[&Path]) -> Output {
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
pub(crate) fn entries(dir: &Path) -> Vec<String> {
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
pub(crate) fn link_target(path: &Path) -> Option<String> {
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
pub(crate) fn three_modes(path: &Path) -> Vec<(&'static str, Output)> {
    vec![
        ("(stdout)", fmt_stdout(path)),
        ("--check", fmt_check(path)),
        ("-w", fmt_write(path)),
    ]
}

pub(crate) fn symlink(target: impl AsRef<Path>, link: &Path) {
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
pub(crate) struct ShortAlias {
    pub(crate) dir: PathBuf,
}

impl ShortAlias {
    pub(crate) fn new(target: &Path) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos();
        // `/tmp` rather than `temp_dir()`: `$TMPDIR` can be a long path, and the
        // alias is what the socket's `sun_path` is measured on.
        let dir = PathBuf::from("/tmp").join(format!("kelpc-fmt-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating a short alias directory");
        symlink(target, &dir.join("d"));
        Self { dir }
    }

    /// The short path that reaches `target`'s entry `name`.
    pub(crate) fn entry(&self, name: &str) -> PathBuf {
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
pub(crate) fn run_within(limit: std::time::Duration, command: &mut Command) -> Option<Output> {
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
pub(crate) fn mkfifo(path: &Path) -> bool {
    Command::new("mkfifo")
        .arg(path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The file type of a path as a word, for the assertions that a refusal left the
/// node it refused exactly as it was.
pub(crate) fn node_type(path: &Path) -> &'static str {
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

use super::*;

pub(crate) fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

/// A directory of one test's own, rooted in `CARGO_TARGET_TMPDIR` the way
/// `fmt.rs::scratch` is. Test binaries run their tests on several threads, and
/// two heavy tests here build the same seed numbers, so each owns its own
/// directory rather than sharing one.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let mut p = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    p.push("differential_fuzz");
    p.push(name);
    p
}

pub(crate) fn build(source: &Path, bin: &Path) -> Output {
    let mut cmd = Command::new(kelpc_bin());
    cmd.arg("build");
    cmd.env_remove("KELPC_BACKEND");
    cmd.env_remove("KELPC_ORACLE");
    cmd.arg(source)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

/// One binary's outcome: a clean exit code, or the signal that killed it.
pub(crate) struct Run {
    pub(crate) code: Option<i32>,
    pub(crate) signal: Option<i32>,
    pub(crate) stdout: Vec<u8>,
}

pub(crate) fn run(bin: &Path) -> Run {
    let out = Command::new(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to run '{}': {e}", bin.display()));
    Run {
        code: out.status.code(),
        signal: out.status.signal(),
        stdout: out.stdout,
    }
}

impl Run {
    pub(crate) fn ending(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => "an unknown termination".to_string(),
        }
    }
}

/// What one program did under the `llvm` backend. The heavy tests turn the
/// non-`Ran` variants into counts and failures; a failure is never allowed to be
/// one of the numbers a run reports and moves on from.
pub(crate) enum Outcome {
    /// It built, ran to an exit code, and matched what it was checked against
    /// (nothing, for a generated program; its golden, for a corpus program).
    Ran,
    /// The compiler refused it. The generator emits valid programs, so this is a
    /// generator bug or a gap in the surface, never acceptable.
    Refused(String),
    /// The build died from a signal, or failed with no diagnostic.
    BuildCrashed(String),
    /// The run died from a signal, or it did not match its golden.
    Mismatch(String),
}

/// Builds one program with the `llvm` backend and runs it. The failure strings
/// name the seed and the program path, and show the output and the exit status,
/// so a failure is diagnosable from the report alone.
pub(crate) fn check(seed: u64, shapes: &[&str], source: &Path, dir: &Path) -> Outcome {
    let l_dir = dir.join("llvm");
    std::fs::create_dir_all(&l_dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", l_dir.display()));
    let l_bin = l_dir.join(format!("seed_{seed:05}"));
    let l_build = build(source, &l_bin);
    if !l_build.status.success() {
        let message = format!(
            "seed {seed} ({}): the llvm backend did not build '{}' ({}):\n{}",
            shapes.join(" "),
            source.display(),
            l_build.status.code().map_or("signal".to_string(), |c| format!("exit {c}")),
            String::from_utf8_lossy(&l_build.stderr).trim_end(),
        );
        return match l_build.status.code() {
            Some(_) if first_line_is_diagnostic(&l_build.stderr) => Outcome::Refused(message),
            _ => Outcome::BuildCrashed(message),
        };
    }
    let l_run = run(&l_bin);
    if l_run.signal.is_some() {
        return Outcome::Mismatch(format!(
            "seed {seed} ({}): the llvm binary for '{}' was {}",
            shapes.join(" "),
            source.display(),
            l_run.ending(),
        ));
    }
    Outcome::Ran
}

/// Whether a failed build's stderr begins with a `kelpc` diagnostic.
pub(crate) fn first_line_is_diagnostic(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).starts_with("error:")
}

/// The seed list for the heavy run: `FUZZ_SEED_START` and `FUZZ_SEED_COUNT`
/// select it, and the defaults are fixed so the gate is repeatable.
pub(crate) fn fuzz_seeds() -> (u64, u64) {
    let start = std::env::var("FUZZ_SEED_START")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let count = std::env::var("FUZZ_SEED_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    (start, count)
}

// ---------------------------------------------------------------------------
// The conformance corpus.
// ---------------------------------------------------------------------------

/// The seeds whose programs are checked in under `tests/fuzz-corpus/`, one file
/// each. The set is frozen so a reader can name a corpus file by seed, and
/// [`the_corpus_files_are_the_generators_own_output`] keeps it equal to the
/// generator. Chosen to cover every shape in [`SHAPES`].
pub(crate) const CORPUS_SEEDS: &[u64] = &[
    1, 2, 3, 5, 8, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83,
];

pub(crate) fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fuzz-corpus")
}

pub(crate) fn corpus_path(seed: u64) -> PathBuf {
    corpus_dir().join(format!("corpus_{seed:05}.kp"))
}

/// The seed encoded in a corpus file name, or `None` for another name.
pub(crate) fn corpus_seed_of(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_prefix("corpus_")?.parse().ok()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

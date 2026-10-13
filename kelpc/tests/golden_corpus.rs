//! The golden output of the corpus, frozen from the C oracle (stage 2, WP-1 of
//! `stage2-oracle-plan.md`, option A) before the C backend was removed.
//!
//! The C backend was the oracle the `llvm` backend was checked against for the
//! whole of MK7. Its answers were written down before it was removed on
//! 2026-10-10 (stage 2, step A): for every corpus target the C backend built,
//! the exit status, the first line of stderr and the stdout itself. The `llvm`
//! backend is checked against those files now, with no C compiler in the
//! comparison (the link still uses `cc`). The goldens are the reference.
//!
//! The frozen set was taken at release `v26.10.5` (commit `387dc450`), not from
//! `main`, because that is the release the plan names. The files are in
//! `kelpc/tests/golden/`:
//!
//! * `index.tsv` has a header line with the counts, a column line, and one row
//!   per candidate target: `tag`, `status` (`ran`, `c-refused` or `excluded`),
//!   `exit` (the run's exit code), the first stderr line, and the SHA-256 of the
//!   stdout. A `c-refused` target is a compile-error fixture the C backend
//!   rejected by design; it has no golden, the way `backend_coverage.rs` skips it.
//! * `<tag>.out` is the frozen stdout of each `ran` target, byte for byte.
//!
//! Targets that are not defined by the language are not frozen, because a
//! golden would then pin one arbitrary answer as if it were the specification
//! (`kelp wiki page changelog.md:107-120`, section 7 question 2 of the plan). The
//! exclusion list is [`EXCLUDED`].
//!
//! Tests:
//!
//! * `the_sha256_helper_matches_the_standard_vector` and
//!   `golden_manifest_matches_targets` run in the default suite. They read the
//!   index and the files; they compile nothing.
//! * `llvm_matches_every_golden` (`#[ignore]`) builds every `ran` target with the
//!   default backend and compares exit status, stdout and the first stderr line
//!   with the frozen values.
//!
//! ```text
//! # the default checks (cheap):
//! cargo test --release -p kelpc --test golden_corpus
//! # the llvm sweep:
//! cargo test --release -p kelpc --test golden_corpus -- --ignored llvm_matches --nocapture
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Targets that are not frozen and not checked, with the reason. Each reason
/// names where the behaviour is left open. Keep the list short: an entry here
/// is a corner of the corpus nobody checks.
const EXCLUDED: &[(&str, &str)] = &[
    ("fixture_float_format_edges", "prints the formatting of 0.0 / 0.0, whose sign bit the language leaves unspecified (plan/bugfix-pass.md:107-120, the NaN sign)"),
    ("fixture_trace", "its stderr carries a clock reading and the source path, which change from run to run"),
    ("fixture_http_try_send", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_https_try_dial", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_bad_record", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_big_body", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_ca_file", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_concurrent", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_error", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_handshake_no_timeout", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_handshake_socket_timeout", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_partial", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_pool_close", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_reset_handshake", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_reset_read", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_sni", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_timeout_handshake", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_timeout_read", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_truncated", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_try_ok", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_gate", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_try_connect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_wss", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_close_vs_keepalive", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_get", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_pool_off", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_rate_limit", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_redirect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_redirect_denied", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_big_headers", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_get", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_get_denied", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_redirect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
];

// ---------------------------------------------------------------------------
// The corpus, as backend_coverage.rs enumerates it.
// ---------------------------------------------------------------------------

struct Target {
    tag: String,
    source: PathBuf,
}

/// The corpus in a deterministic order: fixtures, then projects with a
/// manifest, then examples. The same enumeration as `backend_coverage.rs`.
fn targets() -> Vec<Target> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut targets = Vec::new();

    for (stem, source) in kp_files(&manifest.join("tests").join("fixtures")) {
        targets.push(Target {
            tag: format!("fixture_{stem}"),
            source,
        });
    }

    let projects = manifest.join("tests").join("projects");
    let mut project_dirs: Vec<PathBuf> = std::fs::read_dir(&projects)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", projects.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.join("coral.toml").is_file().then_some(path)
        })
        .collect();
    project_dirs.sort();
    for source in project_dirs {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        targets.push(Target {
            tag: format!("project_{name}"),
            source,
        });
    }

    for (stem, source) in kp_files(&manifest.join("..").join("examples")) {
        targets.push(Target {
            tag: format!("example_{stem}"),
            source,
        });
    }

    targets
}

/// Every `.kp` file in `dir`, as `(stem, path)`, sorted by stem.
fn kp_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("kp") {
                return None;
            }
            Some((path.file_stem()?.to_str()?.to_string(), path))
        })
        .collect();
    found.sort();
    found
}

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn scratch_bin(backend: &str, tag: &str) -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push("golden_corpus");
    dir.push(backend);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir.join(tag)
}

/// The per-process directory the `fixture_fs_*` targets write their scratch
/// files in. It is keyed on the process id, so two runs at once never share it.
fn fs_run_dir() -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push("golden_corpus");
    dir.push(format!("fs_run_{}", std::process::id()));
    dir
}

/// Where the `fixture_fs_*` targets write: the `files` folder of `fs_run_dir()`.
fn fs_files_dir() -> PathBuf {
    fs_run_dir().join("files")
}

/// Removes `fs_run_dir()` when the sweep ends, whether it passes or panics.
struct FsRunDir;

impl Drop for FsRunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(fs_run_dir());
    }
}

/// The source to build for an `fixture_fs_*` target. Its scratch paths are
/// literal `/tmp/kelp_fs_*` text, so the copy is rewritten to point under
/// `fs_files_dir()`. The golden keeps the `/tmp` text; `unmap_fs_paths` maps the
/// output back before the comparison.
fn fs_source(source: &Path) -> PathBuf {
    let run = fs_run_dir();
    let src_dir = run.join("src");
    std::fs::create_dir_all(&src_dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", src_dir.display()));
    let files = fs_files_dir();
    std::fs::create_dir_all(&files)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", files.display()));
    let text = std::fs::read_to_string(source)
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", source.display()));
    let rerouted = text.replace("/tmp/kelp_fs_", &format!("{}/kelp_fs_", files.display()));
    let target = src_dir.join(source.file_name().expect("a corpus source has a file name"));
    std::fs::write(&target, rerouted)
        .unwrap_or_else(|e| panic!("could not write '{}': {e}", target.display()));
    target
}

/// Maps each `fs_files_dir()/kelp_fs_` in a run's stdout back to the `/tmp`
/// text the frozen golden holds. Bytes that are not part of a match are copied
/// as they are, so non-UTF-8 output still compares exactly.
fn unmap_fs_paths(stdout: &[u8]) -> Vec<u8> {
    let from = format!("{}/kelp_fs_", fs_files_dir().display());
    let from = from.as_bytes();
    let to = b"/tmp/kelp_fs_";
    let mut out = Vec::with_capacity(stdout.len());
    let mut i = 0;
    while i < stdout.len() {
        if stdout[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(stdout[i]);
            i += 1;
        }
    }
    out
}

/// Builds one target with the default backend (LLVM). The retired variables
/// are stripped, so an inherited value cannot change the answer.
fn build(source: &Path, bin: &Path) -> std::process::Output {
    let mut command = Command::new(kelpc_bin());
    command.arg("build");
    command.env_remove("KELPC_BACKEND");
    command.env_remove("KELPC_ORACLE");
    command
        .arg(source)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

/// One run: the exit code (or the signal that killed it), stdout, and the first
/// stderr line. stdin is closed so a program that reads it sees end of input.
struct Run {
    code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
    stderr_first: String,
}

fn run(bin: &Path) -> Run {
    let out = Command::new(bin)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("failed to run '{}': {e}", bin.display()));
    Run {
        code: out.status.code(),
        signal: out.status.signal(),
        stdout: out.stdout,
        stderr_first: first_line(&out.stderr),
    }
}

impl Run {
    fn exit(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => "an unknown termination".to_string(),
        }
    }
}

/// The first line of a stderr, with tabs and carriage returns made spaces so it
/// fits one column of the index.
fn first_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .replace(['\t', '\r'], " ")
        .trim_end()
        .to_string()
}

// ---------------------------------------------------------------------------
// The index.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ran,
    CRefused,
    Excluded,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ran => "ran",
            Status::CRefused => "c-refused",
            Status::Excluded => "excluded",
        }
    }

    fn parse(text: &str) -> Option<Status> {
        match text {
            "ran" => Some(Status::Ran),
            "c-refused" => Some(Status::CRefused),
            "excluded" => Some(Status::Excluded),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    tag: String,
    status: Status,
    exit: String,
    stderr_first: String,
    sha256: String,
}

const INDEX_NAME: &str = "index.tsv";
const INDEX_COLUMNS: &str = "tag\tstatus\texit\tstderr_first_line\tstdout_sha256";

/// The parsed index: the candidate count from its header and the rows.
struct Index {
    candidates: usize,
    ran: usize,
    c_refused: usize,
    excluded: usize,
    rows: Vec<Row>,
}

fn read_index() -> Index {
    let path = golden_dir().join(INDEX_NAME);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "the golden index '{}' is missing ({e})",
            path.display()
        )
    });

    let mut counts: Option<(usize, usize, usize, usize)> = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# counts ") {
            let mut parts = header.split_whitespace().map(|p| {
                p.split('=')
                    .nth(1)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or_else(|| panic!("bad counts line '{line}'"))
            });
            counts = Some((
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            ));
            continue;
        }
        if line.starts_with('#') || line == INDEX_COLUMNS || line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            5,
            "index row has {} columns, not 5: '{line}'",
            cols.len()
        );
        rows.push(Row {
            tag: cols[0].to_string(),
            status: Status::parse(cols[1])
                .unwrap_or_else(|| panic!("unknown status '{}' in index row '{line}'", cols[1])),
            exit: cols[2].to_string(),
            stderr_first: cols[3].to_string(),
            sha256: cols[4].to_string(),
        });
    }

    let (candidates, ran, c_refused, excluded) =
        counts.unwrap_or_else(|| panic!("'{}' has no '# counts' header", path.display()));
    Index {
        candidates,
        ran,
        c_refused,
        excluded,
        rows,
    }
}

fn golden_file(tag: &str) -> PathBuf {
    golden_dir().join(format!("{tag}.out"))
}

// ---------------------------------------------------------------------------
// SHA-256, so the index can name stdout without a dependency.
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data`, as lowercase hex (FIPS 180-4).
fn sha256(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }

    h.iter().map(|x| format!("{x:08x}")).collect()
}

// ---------------------------------------------------------------------------
// The freeze.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The checks.
// ---------------------------------------------------------------------------

#[test]
fn the_sha256_helper_matches_the_standard_vector() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
}

/// The index and the corpus agree, and every frozen file is what the index
/// says. No compilation: this is what the default suite can afford.
#[test]
fn golden_manifest_matches_targets() {
    let index = read_index();
    let corpus: BTreeSet<String> = targets().into_iter().map(|t| t.tag).collect();
    let indexed: BTreeSet<String> = index.rows.iter().map(|r| r.tag.clone()).collect();

    assert_eq!(
        index.rows.len(),
        index.candidates,
        "the index header says {} candidates but it has {} rows",
        index.candidates,
        index.rows.len(),
    );
    assert_eq!(
        indexed.len(),
        index.rows.len(),
        "the index names a target twice",
    );
    let missing: Vec<&String> = corpus.difference(&indexed).collect();
    let extra: Vec<&String> = indexed.difference(&corpus).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the index and the corpus disagree; not indexed: {missing:?}; not in the corpus: {extra:?}",
    );

    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &index.rows {
        *tally.entry(row.status.label()).or_default() += 1;
        match row.status {
            Status::Ran => {
                let path = golden_file(&row.tag);
                let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    panic!("golden '{}' is missing ({e})", path.display())
                });
                assert_eq!(
                    sha256(&bytes),
                    row.sha256,
                    "'{}' does not match its index digest",
                    path.display(),
                );
            }
            Status::CRefused | Status::Excluded => {
                assert!(
                    !golden_file(&row.tag).exists(),
                    "'{}' has a golden file but its status is {}",
                    row.tag,
                    row.status.label(),
                );
            }
        }
    }
    for (tag, _) in EXCLUDED {
        assert!(
            index
                .rows
                .iter()
                .any(|r| r.tag == *tag && r.status == Status::Excluded),
            "'{tag}' is in EXCLUDED but the index does not record it as excluded",
        );
    }

    println!(
        "golden set: {} candidates; {} ran (goldens), {} c-refused, {} excluded",
        index.candidates,
        index.ran,
        index.c_refused,
        index.excluded,
    );
    assert_eq!(
        tally.get("ran").copied().unwrap_or_default(),
        index.ran,
        "the header's ran count is wrong",
    );
}

/// The heavy check: the `llvm` backend, built with its default settings, must
/// reproduce every frozen target's exit status, stdout and first stderr line.
/// Every failure names the target and shows the frozen and the observed output.
#[test]
#[ignore = "builds and runs every golden target with llvm; run with --ignored"]
fn llvm_matches_every_golden() {
    let index = read_index();
    let corpus: BTreeMap<String, PathBuf> =
        targets().into_iter().map(|t| (t.tag, t.source)).collect();

    let _fs_run = FsRunDir;
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for row in index.rows.iter().filter(|r| r.status == Status::Ran) {
        let source = corpus
            .get(&row.tag)
            .unwrap_or_else(|| panic!("no corpus source for '{}'", row.tag));
        let bin = scratch_bin("llvm", &row.tag);
        let is_fs = row.tag.starts_with("fixture_fs_");
        let built_from = if is_fs { fs_source(source) } else { source.clone() };

        let built = build(&built_from, &bin);
        if !built.status.success() {
            failures.push(format!(
                "'{}': the llvm backend did not build it ({}):\n{}",
                row.tag,
                built.status.code().map_or("signal".to_string(), |c| format!("exit {c}")),
                String::from_utf8_lossy(&built.stderr).trim_end(),
            ));
            continue;
        }
        checked += 1;

        let mut observed = run(&bin);
        if is_fs {
            observed.stdout = unmap_fs_paths(&observed.stdout);
        }
        let frozen = std::fs::read(golden_file(&row.tag))
            .unwrap_or_else(|e| panic!("golden for '{}' is missing ({e})", row.tag));

        let mut problems = Vec::new();
        if observed.signal.is_some() {
            problems.push(format!("the binary was {}", observed.exit()));
        }
        if observed.exit() != row.exit {
            problems.push(format!("exit: frozen '{}', llvm '{}'", row.exit, observed.exit()));
        }
        if observed.stderr_first != row.stderr_first {
            problems.push(format!(
                "stderr first line: frozen {:?}, llvm {:?}",
                row.stderr_first, observed.stderr_first
            ));
        }
        if observed.stdout != frozen {
            problems.push(format!(
                "stdout differs (frozen {} bytes, llvm {} bytes):\n  frozen: {:?}\n  llvm:   {:?}",
                frozen.len(),
                observed.stdout.len(),
                String::from_utf8_lossy(&frozen),
                String::from_utf8_lossy(&observed.stdout),
            ));
        }
        if !problems.is_empty() {
            failures.push(format!("'{}':\n  {}", row.tag, problems.join("\n  ")));
        }
    }

    println!(
        "llvm golden sweep: {} frozen target(s), {checked} built by llvm, {} failure(s)",
        index.ran,
        failures.len(),
    );
    assert!(
        failures.is_empty(),
        "{} golden target(s) differ under llvm:\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

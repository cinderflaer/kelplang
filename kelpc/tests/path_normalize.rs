//! The differential corpus for path normalization (MK7 §8).
//!
//! The companion of `cap_matcher.rs`, and it exists for the same reason: §8
//! moves the runtime from C text into Kelp one piece at a time, and this piece
//! decides a *security* boundary. `path.normalize` collapsing `..` and the
//! capability check refusing a path that escapes its grant root are the same
//! function, so a port that is one byte more permissive hands out a path the
//! grant does not cover.
//!
//! The two implementations:
//!
//!   * `kelpc/runtime/path_normalize_ref.c` — the C the language ran before §8,
//!     kept as the oracle and compiled by this test directly;
//!   * `kelpc/std/rt.kp`'s `path_normalize` — the Kelp the language runs now.
//!
//! Both are called with the same capacity (`PATH_MAX`), over a zeroed buffer,
//! and each case is compared as **49 lines**: the first 48 bytes of the buffer,
//! then the verdict. Comparing the bytes and not only the verdict is deliberate
//! — a failure after a different number of segments leaves different bytes
//! behind, so the comparison says *where* in the algorithm two versions parted,
//! not merely that they did.
//!
//! What the corpus covers: the `.`/`..` shapes (including the ones that escape
//! the root, which must be rejected), doubled and trailing separators, relative
//! paths, a deterministic fan over a path alphabet, and lengths either side of
//! the buffer bound — where "either side" is the interesting part, since the C
//! version refuses a result that does not leave room for the NUL and the Kelp
//! version has to refuse exactly the same ones.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Bytes of the buffer each case compares. Enough to see a normalized path of
/// realistic length and the NUL that ends it; a longer result is compared by its
/// verdict and the part that decides the match.
const SHOWN: usize = 48;

/// Lines per case: `SHOWN` bytes plus the verdict.
const LINES_PER_CASE: usize = SHOWN + 1;

/// `PATH_MAX` on Linux, and the capacity both implementations are called with.
const CAP: usize = 4096;

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn tmp(name: &str) -> PathBuf {
    let mut p = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    p.push(name);
    p
}

fn corpus() -> Vec<String> {
    let mut paths: Vec<String> = [
        // ---- absolute, nothing to do ---------------------------------------
        "/",
        "/a",
        "/a/b",
        "/a/b/c",
        "/.",
        "/..",
        "/./",
        "/../",
        "/a/.",
        "/a/..",
        "/a/b/..",
        "/a/b/../",
        "/a/b/../..",
        "/a/b/../../..",
        "/a/b/c/../..",
        "/a/./b",
        "/a/././b",
        "/a/../b",
        "/a/../../b",
        "/a/b/../../c",
        "/a/b/../../../c",
        "/a//b",
        "/a///b",
        "/a//",
        "/a/",
        "//",
        "///",
        "/////a",
        "/tmp/../tmp/x",
        "/tmp/./scratch/../scratch/y",
        // ---- names that only look like dots and separators ----------------
        "/...",
        "/a/...",
        "/..a",
        "/a..",
        "/..a/b",
        "/a../..",
        "/a/.b/c",
        "/a/b./c",
        // ---- relative, and therefore rejected ------------------------------
        "",
        "a",
        "a/b",
        "./a",
        "../a",
        "a/../b",
        ".hidden",
        // ---- whitespace, which a path may legally contain -----------------
        "/a\tb",
        "/a\nb",
        "/a b/c",
        // ---- a grant root, and the shapes a symlink check sees ------------
        "/etc",
        "/etc/",
        "/etc/../etc",
        "/etc/passwd/..",
        "/home/user/../../etc/passwd",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    // A deterministic fan over a path alphabet: `/` runs, dot segments, and
    // ordinary names in every order a rewrite of the segment loop can get wrong.
    const ALPHABET: &[u8] = b"ab/._";
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..120 {
        let len = 1 + next() % 16;
        let mut s = String::new();
        // Half the corpus is absolute and half is not: `is_absolute` is the
        // first decision the function makes, and a corpus that only ever fed it
        // `/...` would never check the rejection.
        if next() % 2 == 0 {
            s.push('/');
        }
        for _ in 0..len {
            s.push(ALPHABET[next() % ALPHABET.len()] as char);
        }
        paths.push(s);
    }

    // ---- either side of the buffer bound ----------------------------------
    //
    // The rule is `o + len >= cap`, so a result of `cap - 1` bytes fits and one
    // of `cap` bytes does not. These are the lengths where that is decided.
    for n in [CAP - 4, CAP - 3, CAP - 2, CAP - 1, CAP, CAP + 1] {
        let name = "a".repeat(n - 1);
        paths.push(format!("/{name}")); // result is exactly `n` bytes
        paths.push(format!("/{name}/")); // trailing separator is dropped
        paths.push(format!("/{name}/b/..")); // long, then popped back
        paths.push(format!("//{name}//")); // long, and every separator doubled
    }
    // A long path built from short segments, and a long path that pops: the
    // cursor arithmetic near the bound is where an off-by-one lives.
    paths.push("/ab".repeat(2000));
    paths.push(format!("{}/..", "/ab".repeat(2000)));
    paths.push(format!("{}/..", "a".repeat(CAP + 10)));

    paths
}

/// A Kelp string literal and a C one take the same four escapes the corpus can
/// need, so one function serves both drivers.
fn literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn c_driver(paths: &[String]) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by kelpc/tests/path_normalize.rs: 49 lines per case. */\n");
    out.push_str(&format!(
        "#include \"{}\"\n",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime/path_normalize_ref.c")
            .display()
    ));
    out.push_str("#include <limits.h>\n#include <stdio.h>\n#include <string.h>\n\nint main(void) {\n");
    for path in paths {
        out.push_str("    {\n");
        out.push_str(&format!("        char buf[PATH_MAX + 1];\n"));
        out.push_str("        memset(buf, 0, sizeof buf);\n");
        out.push_str(&format!(
            "        bool ok = kelp_path_normalize({}, buf, PATH_MAX);\n",
            literal(path)
        ));
        out.push_str(&format!(
            "        for (int i = 0; i < {SHOWN}; i++) printf(\"%u\\n\", (unsigned)(unsigned char)buf[i]);\n"
        ));
        out.push_str("        printf(\"%s\\n\", ok ? \"true\" : \"false\");\n");
        out.push_str("    }\n");
    }
    out.push_str("    return 0;\n}\n");
    out
}

fn kelp_driver(paths: &[String]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by kelpc/tests/path_normalize.rs: 49 lines per case.\n");
    out.push_str("impl \"std/rt\" as rt\nimpl \"std/mem\" as mem\n\nfun main() {\n");
    for path in paths {
        // `mem.alloc(n)` gives `len == cap == n` over a zero-filled `n + 1` byte
        // allocation, which is exactly the C driver's `char buf[PATH_MAX + 1]`
        // after `memset`: the same buffer, the same capacity, the same rule.
        out.push_str("    {\n");
        out.push_str(&format!("        var buf = mem.alloc(n = {CAP})\n"));
        out.push_str(&format!(
            "        val ok = rt.path_normalize({}, buf)\n",
            literal(path)
        ));
        for i in 0..SHOWN {
            out.push_str(&format!("        print(buf.get({i}))\n"));
        }
        out.push_str("        print(ok)\n");
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

fn build_c_driver(paths: &[String]) -> PathBuf {
    let src = tmp("path_normalize_c.c");
    let bin = tmp("path_normalize_c");
    std::fs::write(&src, c_driver(paths)).expect("writing the C driver");
    let out = Command::new("cc")
        .arg("-o")
        .arg(&bin)
        .arg(&src)
        .output()
        .expect("failed to invoke cc for the corpus driver");
    assert!(
        out.status.success(),
        "cc could not build the reference driver:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Builds the Kelp side with the C backend, for the same reason `cap_matcher.rs`
/// pins it: this test is about the normalizer, not about which backend is being
/// validated, and the asm backend does not compile the runtime module in yet.
fn build_kelp_driver(paths: &[String]) -> PathBuf {
    let src = tmp("path_normalize_kelp.kp");
    let bin = tmp("path_normalize_kelp");
    std::fs::write(&src, kelp_driver(paths)).expect("writing the Kelp driver");
    let out = Command::new(kelpc_bin())
        .args(["build", "--backend=c"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "kelpc could not build the Kelp driver:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

fn run_driver(bin: &Path, who: &str) -> Vec<String> {
    let out = Command::new(bin).output().expect("failed to run the driver");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "the {who} driver exited with {:?} after {} verdict lines:\n{}",
        out.status.code(),
        stdout.lines().count(),
        String::from_utf8_lossy(&out.stderr)
    );
    stdout.lines().map(|l| l.to_string()).collect()
}

/// A path, short enough to read in a failure report.
fn brief(s: &str) -> String {
    const LIMIT: usize = 60;
    let head: String = s.chars().take(LIMIT).collect();
    if head.len() == s.len() {
        format!("{head:?}")
    } else {
        format!("{head:?}… ({} bytes)", s.len())
    }
}

#[test]
fn the_kelp_normalizer_agrees_with_the_c_reference_on_the_whole_corpus() {
    let paths = corpus();
    assert!(paths.len() > 100, "the corpus is too small to mean anything");

    let oracle = run_driver(&build_c_driver(&paths), "C reference");
    let port = run_driver(&build_kelp_driver(&paths), "Kelp");

    let expected = paths.len() * LINES_PER_CASE;
    assert_eq!(oracle.len(), expected, "the C driver printed the wrong shape");
    assert_eq!(port.len(), expected, "the Kelp driver printed the wrong shape");

    for (index, path) in paths.iter().enumerate() {
        let want = &oracle[index * LINES_PER_CASE..(index + 1) * LINES_PER_CASE];
        let got = &port[index * LINES_PER_CASE..(index + 1) * LINES_PER_CASE];
        if want == got {
            continue;
        }
        // The verdict is the last line of the case; the rest are the buffer.
        let verdict = LINES_PER_CASE - 1;
        if want[verdict] != got[verdict] {
            panic!(
                "case {index} disagrees on the verdict: {} — the C reference says {}, the Kelp port says {}",
                brief(path),
                want[verdict],
                got[verdict]
            );
        }
        let at = want
            .iter()
            .zip(got.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "case {index} disagrees at byte {at} of the buffer: {} — the C reference wrote {}, the Kelp port wrote {}",
            brief(path),
            want[at],
            got[at]
        );
    }

    // The corpus has to be worth comparing: it must accept and reject, and the
    // accepted cases must include ones the normalizer actually *changed* (a
    // corpus of already-normal paths would agree while proving nothing).
    let mut accepted = 0;
    let mut rejected = 0;
    let mut changed = 0;
    for (path, case) in paths.iter().zip(oracle.chunks(LINES_PER_CASE)) {        if case[LINES_PER_CASE - 1] == "true" {
            accepted += 1;
            let written: String = case
                .iter()
                .take(SHOWN)
                .map(|b| b.parse::<u8>().unwrap() as char)
                .take_while(|c| *c != '\0')
                .collect();
            if written != *path {
                changed += 1;
            }
        } else {
            rejected += 1;
        }
    }
    assert!(accepted > 20 && rejected > 20, "{accepted} accepted / {rejected} rejected");
    assert!(changed > 10, "only {changed} cases were normalized to something else");
}

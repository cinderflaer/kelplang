//! The differential corpus for the capability matcher (MK7 §8).
//!
//! `plan/asm-backend/plan.md` §8 moves the runtime from C text into Kelp, and its
//! risk register names what that costs when it goes wrong: the capability matcher
//! is byte-exact security behavior, and "a lenient port widens every grant". The
//! fixtures prove the grants still *work*; only a corpus proves the *verdicts*
//! are identical, including for shapes nobody wrote a fixture for.
//!
//! So one corpus goes through two implementations:
//!
//!   * `kelpc/runtime/cap_matcher_ref.c` — the C the checks ran before §8, kept
//!     as the oracle. It is no longer emitted into programs, and this test
//!     compiles it directly, which is the reason it is a file rather than a
//!     `const` inside the compiler.
//!   * `kelpc/std/rt.kp` — the Kelp the checks run now, compiled by the C backend
//!     into a driver that prints one verdict per case.
//!
//! Both drivers are generated from the same `Vec<Case>`, so a mismatch is
//! reportable as the exact case that differs rather than as a diff of two large
//! programs' output — and a mismatch means the port is wrong, in the direction
//! the plan is most afraid of.
//!
//! Three kinds of case, because the three functions have three different shapes:
//! `glob_match` (paths, `**` crosses segments), `host_glob` (`*` is one label
//! wide, so it stops at `.` and `:`), and `host_match` (which is `host_glob`
//! against the host part of `host:port`). The corpus counts verdicts per kind and
//! insists each kind has cases on both sides of the decision: a corpus that
//! agreed only on denials would pass while the port accepted everything.
//!
//! What the corpus covers:
//!
//!   * hand-written edge cases — the short patterns (`""`, `*`, `**`, `/**`,
//!     `/*`) whose C reads run to the end of the pattern, the `host:port`
//!     colon cases, empty pieces, `//`, `.` and `:` inside segments;
//!   * a deterministic pseudo-random fan over the metacharacter alphabet
//!     (`a`, `b`, `/`, `*`, `.`, `:`, `_`), which is where a rewrite of a
//!     backtracking matcher actually breaks;
//!   * names either side of the C version's `PATH_MAX` buffer bound (4095
//!     bytes), because `host_match` copies the host part into such a buffer
//!     while the Kelp version bounds the match instead, and the two have to
//!     agree there as well.
//!
//! What it does not cover, and why: bytes that are not valid UTF-8 (a Kelp string
//! literal cannot carry them) and NUL, which no `str` can hold. Both
//! implementations are byte-wise, and the corpus' job is the *algorithm*.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Which matcher a case exercises. All three are exported by `std/rt` and all
/// three were `static` functions in the C runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Glob,
    HostGlob,
    HostMatch,
}

impl Kind {
    fn c_name(self) -> &'static str {
        match self {
            Kind::Glob => "kelp_glob_match",
            Kind::HostGlob => "kelp_host_glob",
            Kind::HostMatch => "kelp_host_match",
        }
    }

    fn kelp_name(self) -> &'static str {
        match self {
            Kind::Glob => "glob_match",
            Kind::HostGlob => "host_glob",
            Kind::HostMatch => "host_match",
        }
    }
}

struct Case {
    kind: Kind,
    pattern: String,
    text: String,
}

fn c_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn tmp(name: &str) -> PathBuf {
    let mut p = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
            std::env::temp_dir().to_string_lossy().to_string()
        }),
    );
    p.push(name);
    p
}

/// The corpus: hand-written edge cases, then a deterministic fan, then the
/// buffer-bound cases. `Vec` order is the line order in both drivers.
fn corpus() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |kind: Kind, pattern: &str, text: &str| {
        cases.push(Case {
            kind,
            pattern: pattern.to_string(),
            text: text.to_string(),
        });
    };

    // ---- paths: the shapes the C version's pointer reads are about ---------
    //
    // `pat[1]`, `pat[2]` and `pat[3]` are read through a pointer whose only
    // guarantee is the NUL terminator, so a pattern shorter than four bytes is
    // exactly where a transliteration can invent a read past the end — or a
    // `str.byte` panic.
    for (p, t) in [
        ("", ""),
        ("", "/"),
        ("/", "/"),
        ("/", ""),
        ("//", "/"),
        ("*", ""),
        ("*", "a"),
        ("*", "a/b"),
        ("*", "/"),
        ("**", ""),
        ("**", "a"),
        ("**", "a/b"),
        ("**", "a/b/c"),
        ("/**", "/"),
        ("/**", ""),
        ("/**", "/a"),
        ("/**", "/a/b"),
        ("/*", "/"),
        ("/*", "/a"),
        ("/*", "/a/b"),
        ("/*x", "/ax"),
        ("/a/*", "/a/b"),
        ("/a/*", "/a/b/c"),
        ("/a/*", "/a/"),
        ("/a/*", "/a"),
        ("/a/**", "/a"),
        ("/a/**", "/a/b"),
        ("/a/**", "/a/b/c"),
        ("/a/**", "/ab"),
        ("/a/**", "/a.b"),
        ("/a/**/b", "/a/b"),
        ("/a/**/b", "/a/x/y/b"),
        ("/a/**/b", "/a/x/y/c"),
        ("/a/*/b", "/a/x/b"),
        ("/a/*/b", "/a/x/y/b"),
        ("/a*/b", "/ab/b"),
        ("/a*/b", "/a/b"),
        ("/a*b", "/ab"),
        ("/a*b", "/a/b"),
        ("*a*", "bab"),
        ("**a**", "xay"),
        ("/a/b/", "/a/b"),
        ("/a/b/", "/a/b/"),
        ("/a//b", "/a/b"),
        ("/a/./b", "/a/./b"),
        ("/a/../b", "/a/../b"),
        ("/tmp/**", "/tmp"),
        ("/tmp/**", "/tmp/x"),
        ("/**/*", "/a"),
        ("/**/*", "/a/b"),
        ("/*/**", "/a"),
        ("/*/**", "/a/b"),
    ] {
        push(Kind::Glob, p, t);
    }

    // ---- hosts: `*` is one label wide, and the port vocabulary has colons --
    for (p, t) in [
        ("*", ""),
        ("*", "example.com"),
        ("*", "example.com:80"),
        ("*", "*"),
        ("*.example.com", "api.example.com"),
        ("*.example.com", "example.com"),
        ("*.example.com", "a.b.example.com"),
        ("*.example.com", "api.example.com:443"),
        ("*.example.com", "api.example.org"),
        ("example.com", "example.com"),
        ("example.com", "example.com:80"),
        ("example.com:80", "example.com:80"),
        ("example.com:80", "example.com:81"),
        ("example.com", "example.com:80:90"),
        ("*:80", "example.com:80"),
        ("*:80", "example.com"),
        ("*:80", "example.com:8080"),
        ("*:80", ":80"),
        (":80", "example.com:80"),
        ("a:b:80", "a:b:80"),
        ("a:b:80", "a:b:81"),
        ("a:b", "a:b:80"),
        ("a", "a:b:80"),
        ("localhost", "localhost"),
        ("localhost:0", "localhost:0"),
        ("*.*", "a.b"),
        ("*.*", "a.b.c"),
        ("*.*", "a:b.c"),
        ("a*", "abc"),
        ("a*", "a.b"),
        ("*b", "ab"),
        ("*b", "a.b"),
        ("*b", "a:b"),
        ("", ""),
        ("", "a"),
        ("a", ""),
        ("127.0.0.1", "127.0.0.1"),
        ("127.0.0.1", "127.0.0.1:80"),
        ("*", "127.0.0.1:80"),
        ("a:b", "a:b"),
        ("a:b", "a:b:c"),
    ] {
        push(Kind::HostGlob, p, t);
        push(Kind::HostMatch, p, t);
    }

    // ---- the deterministic fan --------------------------------------------
    //
    // An LCG rather than a random source: the corpus has to be the same corpus
    // on every run and every machine, or a failure is not reproducible from the
    // report.
    const ALPHABET: &[u8] = b"ab/*.:_";
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..1600 {
        // Ten characters is short enough that the backtracking stays cheap: a
        // pattern of a dozen `*`s against a dozen bytes is already millions of
        // steps in *both* implementations, and the corpus is not a benchmark.
        let plen = next() % 10;
        let tlen = next() % 10;
        let pattern: String = (0..plen)
            .map(|_| ALPHABET[next() % ALPHABET.len()] as char)
            .collect();
        let text: String = (0..tlen)
            .map(|_| ALPHABET[next() % ALPHABET.len()] as char)
            .collect();
        push(Kind::Glob, &pattern, &text);
        push(Kind::HostGlob, &pattern, &text);
        push(Kind::HostMatch, &pattern, &text);
    }

    // ---- whitespace, which a path may legally contain ----------------------
    for (p, t) in [("a\tb", "a\tb"), ("a\nb", "a\nb"), ("a*", "a\nb"), ("/a\t*", "/a\tb")] {
        push(Kind::Glob, p, t);
        push(Kind::HostGlob, p, t);
        push(Kind::HostMatch, p, t);
    }

    // ---- either side of the C version's PATH_MAX bound ---------------------
    //
    // `kelp_host_part` copies the host part into `char host[PATH_MAX]` and cuts
    // it at 4095 bytes; the Kelp version bounds the match at the same index
    // instead of copying. These cases are what makes that claim a measurement.
    for len in [4094usize, 4095, 4096, 4097, 5000] {
        let a = "a".repeat(len);
        push(Kind::Glob, &a, &a);
        push(Kind::Glob, &a, &format!("{a}b"));
        push(Kind::HostGlob, &a, &a);
        push(Kind::HostMatch, &a, &a);
        // A colon past the bound: the C version's buffer keeps the first 4095
        // bytes and a NUL, so the text's own colon never reaches the matcher.
        push(Kind::HostMatch, &a, &format!("{a}:80"));
        push(Kind::HostMatch, &format!("{a}:80"), &a);
        // A pattern longer than the bound, with a port: `kelp_cap_check_host`'s
        // head cut is the glue's, but `host_match`'s own comparison is here.
        push(Kind::HostMatch, &format!("{a}*:{len}"), &format!("{a}:{len}"));
    }

    cases
}

/// A Kelp string literal and a C one need the same five escapes and no others,
/// so one function serves both drivers. Any other control byte would have to be
/// escaped as `\xNN`, which Kelp's lexer does not have — which is why the corpus
/// alphabet is printable ASCII plus explicit `\t`/`\n` cases.
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

fn c_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by kelpc/tests/cap_matcher.rs: one line per case. */\n");
    out.push_str(&format!(
        "#include \"{}\"\n",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime/cap_matcher_ref.c")
            .display()
    ));
    out.push_str("#include <stdio.h>\n\nint main(void) {\n");
    for c in cases {
        out.push_str(&format!(
            "    printf(\"%s\\n\", {} ({}, {}) ? \"true\" : \"false\");\n",
            c.kind.c_name(),
            literal(&c.pattern),
            literal(&c.text)
        ));
    }
    out.push_str("    return 0;\n}\n");
    out
}

fn kelp_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by kelpc/tests/cap_matcher.rs: one line per case.\n");
    out.push_str("impl \"std/rt\"\n\nfun main() {\n");
    for c in cases {
        out.push_str(&format!(
            "    print(rt.{}({}, {}))\n",
            c.kind.kelp_name(),
            literal(&c.pattern),
            literal(&c.text)
        ));
    }
    out.push_str("}\n");
    out
}

/// Builds the C oracle: `cc` on the generated driver, which includes the
/// reference implementation from `kelpc/runtime/`.
fn build_c_driver(cases: &[Case]) -> PathBuf {
    let src = tmp("cap_matcher_c.c");
    let bin = tmp("cap_matcher_c");
    std::fs::write(&src, c_driver(cases)).expect("writing the C driver");
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

/// Builds the Kelp side with the C backend.
///
/// The backend is pinned rather than taken from `KELPC_BACKEND`: this test is
/// about the matcher, not about which backend is under test. Pinning makes the
/// corpus run once per suite invocation with the same result.
fn build_kelp_driver(cases: &[Case]) -> PathBuf {
    let src = tmp("cap_matcher_kelp.kp");
    let bin = tmp("cap_matcher_kelp");
    std::fs::write(&src, kelp_driver(cases)).expect("writing the Kelp driver");
    let out = Command::new(c_bin())
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
    if !out.status.success() {
        panic!(
            "the {who} driver exited with {:?} after {} of {} verdicts:\n{}",
            out.status.code(),
            stdout.lines().count(),
            stdout.lines().count(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    stdout.lines().map(|l| l.to_string()).collect()
}

/// A corpus value, short enough to read in a failure report: a mismatching
/// 4096-byte name is not the interesting part of a failure — which case it was
/// is.
fn brief(s: &str) -> String {
    const LIMIT: usize = 48;
    let head: String = s.chars().take(LIMIT).collect();
    if head.len() == s.len() {
        format!("{head:?}")
    } else {
        format!("{head:?}… ({} bytes)", s.len())
    }
}

#[test]
fn the_kelp_matcher_agrees_with_the_c_reference_on_the_whole_corpus() {
    let cases = corpus();
    assert!(cases.len() > 500, "the corpus is too small to mean anything");

    let c_bin = build_c_driver(&cases);
    let kelp_bin = build_kelp_driver(&cases);
    let oracle = run_driver(&c_bin, "C reference");
    let port = run_driver(&kelp_bin, "Kelp");

    assert_eq!(
        oracle.len(),
        cases.len(),
        "the C driver printed {} verdicts for {} cases",
        oracle.len(),
        cases.len()
    );
    assert_eq!(
        port.len(),
        cases.len(),
        "the Kelp driver printed {} verdicts for {} cases",
        port.len(),
        cases.len()
    );

    // The corpus has to decide both ways, per kind, or agreement is worthless:
    // a matcher that answered "false" to everything would pass a corpus of
    // denials. This is checked against the *oracle*, so a corpus that stopped
    // covering the accepting side fails here rather than silently weakening the
    // test.
    let mut allowed = [0usize; 3];
    let mut denied = [0usize; 3];
    for (case, verdict) in cases.iter().zip(&oracle) {
        let slot = match case.kind {
            Kind::Glob => 0,
            Kind::HostGlob => 1,
            Kind::HostMatch => 2,
        };
        match verdict.as_str() {
            "true" => allowed[slot] += 1,
            "false" => denied[slot] += 1,
            other => panic!("the C reference printed '{other}'"),
        }
    }
    for (kind, slot) in [(Kind::Glob, 0), (Kind::HostGlob, 1), (Kind::HostMatch, 2)] {
        assert!(
            allowed[slot] > 10 && denied[slot] > 10,
            "{kind:?}: the corpus decides {} allowed / {} denied — not both sides",
            allowed[slot],
            denied[slot]
        );
    }

    for (i, (case, (want, got))) in cases.iter().zip(oracle.iter().zip(&port)).enumerate() {
        assert_eq!(
            want,
            got,
            "case {i} disagrees: {:?}({}, {}) — the C reference says {want}, the Kelp port says {got}",
            case.kind,
            brief(&case.pattern),
            brief(&case.text)
        );
    }
}

/// A second, smaller assertion that the *checks* reach the matcher at all: the C
/// backend's glue has to call the Kelp symbol, or the corpus above would be
/// testing a function nothing uses while the checks kept running the C one.
///
/// This reads the generated C for a fixture that holds a capability domain. The
/// driver keeps that file next to the binary, which is what makes it readable
/// here — see `driver.rs`.
#[test]
fn the_generated_capability_check_calls_the_kelp_matcher() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/caps_time_rand.kp");
    let bin = tmp("cap_matcher_wiring");
    let out = Command::new(c_bin())
        .args(["build", "--backend=c"])
        .arg(&fixture)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "the fixture did not build:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let generated = std::fs::read_to_string(bin.with_extension("c")).expect("the generated C");
    assert!(
        generated.contains("kelp_std__rt__glob_match(patterns[i], resolved)"),
        "the capability check does not call the Kelp matcher"
    );
    assert!(
        generated.contains("kelp_std__rt__host_glob(pat, host)"),
        "the host check does not call the Kelp matcher"
    );
    // The C matcher must not be *emitted* any more; only the reference file,
    // which this test compiles itself, still defines it.
    assert!(
        !generated.contains("static bool kelp_glob_match"),
        "the C matcher is still emitted into the program"
    );
}

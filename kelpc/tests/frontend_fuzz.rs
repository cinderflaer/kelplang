//! Front-end robustness fuzzing (plan/fuzzing.md).
//!
//! The differential fuzzer (`differential_fuzz.rs`) generates VALID programs and
//! compares the two backends. This file is its complement: it mutates the
//! checked-in fixtures into mostly INVALID text and feeds it to `kelpc check`,
//! and the only property is that the compiler survives: it never panics, aborts,
//! overflows its stack, runs out of its memory limit or hangs. It either accepts
//! the text (exit 0) or prints a diagnostic and exits 1.
//!
//! The mutator is deterministic (xorshift64*, fixed seeds): mutation `i` of a
//! run is a pure function of `FUZZ_SEED_BASE + i`, so a failure has a number a
//! reader can quote and replay (`FUZZ_SEED_BASE=<n> FUZZ_MUTATIONS=1`). Every
//! mutant is run as a subprocess under `ulimit -v 2097152; ulimit -t 20` (the
//! crate has no library entry point to call in process), most of them through
//! `check --text`, so the fixture's own path still resolves its imports while the
//! mutated text comes from stdin; one in sixteen is written to a file as raw
//! bytes instead, which is the only way to reach invalid UTF-8.
//!
//! A failing input is written to the scratch directory and the test fails naming
//! it. The fix for a finding is a proper diagnostic in the compiler, and the
//! input is then kept under `tests/fuzz-corpus/frontend/` where
//! `the_frontend_regression_inputs_stay_survivable` replays it forever.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const KELPC: &str = env!("CARGO_BIN_EXE_kelpc");

/// xorshift64*: tiny, deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // A zero state is a fixed point of xorshift; splitmix the seed so
        // nearby seeds do not start nearby either.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 1 } else { z })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const KEYWORDS: &[&str] = &[
    "fun", "closure", "val", "var", "mut", "move", "if", "else", "elif", "while", "for", "when",
    "defer", "return", "true", "false", "datac", "enum", "pd", "impl", "trait", "as", "const",
    "static", "extern", "pub", "unsafe", "asm", "use", "in", "caps", "where", "self", "Self",
    "print!(", "Ptr<", "List<", "Map<", "Box<", "Option<", "Result<", "String", "Bytes", "Int",
    "i64", "u8", "f64", "bool", "main", "std/io", "std/str",
];

const OPERATORS: &[&str] = &[
    "+", "-", "*", "/", "%", "<", ">", "=", "==", "!=", "<=", ">=", "&&", "||", "!", "&", "|",
    "^", "~", "<<", ">>", "?", ":", "::", ".", "..", "..=", ",", ";", "->", "=>", "(", ")", "[",
    "]", "{", "}", "@", "#", "$", "\\", "`", "'", "\"", "\"\"\"", "//", "/*", "*/", "+=", "-=",
];

const ODD_TEXT: &[&str] = &[
    "\0",
    "\u{feff}",
    "\u{202e}",
    "\u{200b}",
    "\u{7f}",
    "\r",
    "\r\n",
    "\t",
    "\u{0}\u{0}\u{0}",
    "\u{1F980}",
    "\u{10FFFF}",
    "é",
    "e\u{301}",
    "日本語",
    "\u{0085}",
    "\u{2028}",
    "\u{fffd}",
    "'\u{1F980}'",
    "\"\u{301}\"",
    "\\u{110000}",
    "\\xff",
    "'\\",
    "\"\\",
];

const HUGE: &[&str] = &[
    "99999999999999999999999999999999",
    "18446744073709551616",
    "18446744073709551615",
    "9223372036854775808",
    "-9223372036854775808",
    "0xFFFFFFFFFFFFFFFFFF",
    "0b11111111111111111111111111111111111111111111111111111111111111111",
    "1e999999",
    "1e-999999",
    "0.000000000000000000000000000000000000000000000001",
    "340282366920938463463374607431768211456",
    "1_000_000_000_000_000_000_000",
    "4294967296",
    "[0; 99999999999]",
    "[0; 18446744073709551615]",
];

/// Splits text into tokens loosely: identifier/number runs, whitespace runs, or
/// single characters. It only has to be a reasonable seam for mutation, not the
/// lexer's own view.
fn pieces(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        let c = bytes[i];
        let start = i;
        if c.is_ascii_alphanumeric() || c == b'_' {
            while i < text.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        } else if c == b' ' || c == b'\t' {
            while i < text.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
        } else {
            i += 1;
            while i < text.len() && !text.is_char_boundary(i) {
                i += 1;
            }
        }
        out.push(&text[start..i]);
    }
    out
}

fn join(parts: &[&str]) -> String {
    parts.concat()
}

/// Applies one mutation to `text` and returns the result as bytes (a mutation
/// may deliberately produce invalid UTF-8).
fn mutate_once(rng: &mut Rng, text: &str) -> Vec<u8> {
    let kind = rng.below(17);
    match kind {
        // Delete a token.
        0 | 1 => {
            let mut p = pieces(text);
            if !p.is_empty() {
                p.remove(rng.below(p.len()));
            }
            join(&p).into_bytes()
        }
        // Duplicate a token.
        2 => {
            let mut p = pieces(text);
            if !p.is_empty() {
                let i = rng.below(p.len());
                let t = p[i];
                p.insert(i, t);
            }
            join(&p).into_bytes()
        }
        // Swap two tokens.
        3 => {
            let mut p = pieces(text);
            if p.len() > 1 {
                let (a, b) = (rng.below(p.len()), rng.below(p.len()));
                p.swap(a, b);
            }
            join(&p).into_bytes()
        }
        // Delete a line.
        4 => {
            let mut l: Vec<&str> = text.split_inclusive('\n').collect();
            if !l.is_empty() {
                l.remove(rng.below(l.len()));
            }
            join(&l).into_bytes()
        }
        // Duplicate a line.
        5 => {
            let mut l: Vec<&str> = text.split_inclusive('\n').collect();
            if !l.is_empty() {
                let i = rng.below(l.len());
                let t = l[i];
                l.insert(i, t);
            }
            join(&l).into_bytes()
        }
        // Swap two lines.
        6 => {
            let mut l: Vec<&str> = text.split_inclusive('\n').collect();
            if l.len() > 1 {
                let (a, b) = (rng.below(l.len()), rng.below(l.len()));
                l.swap(a, b);
            }
            join(&l).into_bytes()
        }
        // Truncate at a char boundary.
        7 => {
            let mut cut = rng.below(text.len() + 1);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text[..cut].as_bytes().to_vec()
        }
        // Truncate at an arbitrary byte (may split a code point).
        8 => {
            let cut = rng.below(text.len() + 1);
            text.as_bytes()[..cut].to_vec()
        }
        // Insert a keyword or a type word.
        9 | 10 => insert_at_piece(rng, text, KEYWORDS),
        // Insert an operator or a bracket.
        11 | 12 => insert_at_piece(rng, text, OPERATORS),
        // Nest brackets deeply, sometimes unclosed.
        13 => {
            let depth = [64usize, 500, 3_000, 20_000, 100_000][rng.below(5)];
            let (open, close) = *rng.pick(&[
                ("(", ")"),
                ("[", "]"),
                ("{", "}"),
                ("-", ""),
                ("!", ""),
                ("&", ""),
                ("Box<", ">"),
                ("List<", ">"),
                ("if x ", ""),
                ("fun f() { ", "}"),
                ("closure() { ", "}"),
                ("val a = ", ""),
                ("when x { 1 => ", "}"),
                ("[", ""),
                ("\"", ""),
                ("/*", ""),
            ]);
            let mut nest = open.repeat(depth);
            if rng.below(2) == 0 {
                nest.push_str("1");
                nest.push_str(&close.repeat(depth));
            }
            insert_text_at_line(rng, text, &nest)
        }
        // A huge or boundary literal in place of a token.
        14 => replace_piece(rng, text, HUGE),
        // Odd unicode, NUL bytes and escapes.
        15 => insert_at_piece(rng, text, ODD_TEXT),
        // Raw junk bytes (invalid UTF-8 included).
        _ => {
            let mut bytes = text.as_bytes().to_vec();
            let n = 1 + rng.below(4);
            for _ in 0..n {
                let at = rng.below(bytes.len() + 1);
                let b = [0x00u8, 0xff, 0xfe, 0xc0, 0x80, 0xed, 0xf8][rng.below(7)];
                bytes.insert(at, b);
            }
            bytes
        }
    }
}

fn insert_at_piece(rng: &mut Rng, text: &str, set: &[&str]) -> Vec<u8> {
    let mut p = pieces(text);
    let at = rng.below(p.len() + 1);
    let word = *rng.pick(set);
    p.insert(at, word);
    join(&p).into_bytes()
}

fn replace_piece(rng: &mut Rng, text: &str, set: &[&str]) -> Vec<u8> {
    let mut p = pieces(text);
    if p.is_empty() {
        return Vec::new();
    }
    // Prefer a numeric token so the literal lands where one was; fall back to
    // any token.
    let numeric: Vec<usize> = (0..p.len())
        .filter(|&i| p[i].bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let at = if !numeric.is_empty() && rng.below(4) != 0 {
        numeric[rng.below(numeric.len())]
    } else {
        rng.below(p.len())
    };
    p[at] = *rng.pick(set);
    join(&p).into_bytes()
}

fn insert_text_at_line(rng: &mut Rng, text: &str, inserted: &str) -> Vec<u8> {
    let mut l: Vec<&str> = text.split_inclusive('\n').collect();
    let at = rng.below(l.len() + 1);
    let line = format!("{inserted}\n");
    l.insert(at, &line);
    join(&l).into_bytes()
}

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", dir.display()))
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension().and_then(|x| x.to_str()) == Some("kp")).then_some(p)
        })
        .collect();
    v.sort();
    v
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("frontend_fuzz").join(name);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir
}

/// How one compiler run ended.
enum Verdict {
    Survived,
    Failed(String),
}

/// Runs `kelpc check` on `bytes`: through `--text` for the entry `real_path`
/// when the bytes are UTF-8 and `via_text` holds, otherwise from a file in
/// `dir`. Limits are the design's: 2 GiB of address space, 20 s of CPU, and a
/// wall-clock `timeout` behind them for a program that sleeps instead of
/// spinning.
fn run_check(bytes: &[u8], real_path: &Path, dir: &Path, tag: &str, via_text: bool, json: bool) -> Verdict {
    // An empty `tag` means `real_path` already holds `bytes`: check it in place.
    let in_place = tag.is_empty();
    let mut script = String::from(
        "ulimit -v 2097152; ulimit -t 20; exec timeout 40 \"$0\" check \"$1\" --deny-outdated",
    );
    let mut owned_file = None;
    let target: PathBuf = if in_place {
        real_path.to_path_buf()
    } else if via_text && std::str::from_utf8(bytes).is_ok() {
        script.push_str(" --text");
        real_path.to_path_buf()
    } else {
        let file = dir.join(format!("{tag}.kp"));
        std::fs::write(&file, bytes)
            .unwrap_or_else(|e| panic!("could not write '{}': {e}", file.display()));
        owned_file = Some(file.clone());
        file
    };
    if json {
        script.push_str(" --json");
    }
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .arg(KELPC)
        .arg(&target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("could not start sh: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let payload = if !in_place && via_text && std::str::from_utf8(bytes).is_ok() {
        bytes.to_vec()
    } else {
        Vec::new()
    };
    // The writer runs beside the child so a child that exits without reading
    // all of a large input is a broken pipe, not a hang here.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });
    let out = child.wait_with_output().unwrap_or_else(|e| panic!("wait failed: {e}"));
    let _ = writer.join();
    if let Some(f) = owned_file {
        let _ = std::fs::remove_file(f);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match out.status.code() {
        Some(0) => Verdict::Survived,
        Some(1) if !stderr.contains("panicked") => Verdict::Survived,
        Some(c) => Verdict::Failed(format!(
            "exit status {c}\n{}",
            stderr.chars().take(2000).collect::<String>()
        )),
        None => Verdict::Failed(format!(
            "killed by a signal ({:?})\n{}",
            out.status,
            stderr.chars().take(2000).collect::<String>()
        )),
    }
}

/// One mutant, fully determined by `seed`.
fn mutant(seed: u64, fixtures: &[PathBuf]) -> (PathBuf, Vec<u8>, bool, bool) {
    let mut rng = Rng::new(seed);
    let fixture = rng.pick(fixtures).clone();
    let mut bytes = std::fs::read(&fixture)
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", fixture.display()));
    let rounds = 1 + rng.below(3);
    for _ in 0..rounds {
        // Mutations work on text; a previous round's invalid UTF-8 is repaired
        // lossily, which keeps later rounds meaningful.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        bytes = mutate_once(&mut rng, &text);
    }
    let via_text = rng.below(16) != 0;
    let json = rng.below(2) == 0;
    (fixture, bytes, via_text, json)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Runs mutations `[base, base + count)` over two worker threads and fails with
/// every counterexample, each saved under the scratch directory by seed.
fn fuzz(label: &str, base: u64, count: u64) {
    let files = fixtures();
    assert!(!files.is_empty(), "no fixtures to mutate");
    let dir = scratch(label);
    let next = std::sync::atomic::AtomicU64::new(0);
    let failures = std::sync::Mutex::new(Vec::<String>::new());
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= count {
                        break;
                    }
                    let seed = base + i;
                    let (fixture, bytes, via_text, json) = mutant(seed, &files);
                    let tag = format!("mutant_{seed}");
                    if let Verdict::Failed(why) =
                        run_check(&bytes, &fixture, &dir, &tag, via_text, json)
                    {
                        let saved = dir.join(format!("FAIL_{tag}.kp"));
                        let _ = std::fs::write(&saved, &bytes);
                        failures.lock().unwrap().push(format!(
                            "seed {seed} (fixture {}, text-mode {via_text}, json {json}) saved to {}: {why}",
                            fixture.display(),
                            saved.display()
                        ));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} of {count} mutations broke the compiler:\n{}",
        failures.len(),
        failures.join("\n---\n")
    );
}

#[test]
fn the_mutator_is_deterministic() {
    let files = fixtures();
    for seed in [1u64, 2, 3, 77, 123_456] {
        let a = mutant(seed, &files);
        let b = mutant(seed, &files);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1, "seed {seed} mutated differently twice");
    }
    let distinct: std::collections::HashSet<Vec<u8>> =
        (0..50).map(|s| mutant(s, &files).1).collect();
    assert!(distinct.len() > 40, "the seeds barely reach the mutations");
}

/// The default-suite run: 2,000 mutations, fixed seeds (1..=2000 unless
/// `FUZZ_SEED_BASE` / `FUZZ_MUTATIONS` say otherwise).
#[test]
fn fuzz_frontend() {
    fuzz("short", env_u64("FUZZ_SEED_BASE", 1), env_u64("FUZZ_MUTATIONS", 2_000));
}

/// The maintainer's run: 100,000 mutations on a seed range disjoint from the
/// default suite's. Hours, not minutes; see plan/fuzzing.md.
#[test]
#[ignore = "100,000 mutations; run by the maintainer with --ignored"]
fn fuzz_frontend_long() {
    fuzz(
        "long",
        env_u64("FUZZ_SEED_BASE", 1_000_000),
        env_u64("FUZZ_MUTATIONS", 100_000),
    );
}

/// Every input a past fuzz run broke the compiler with, kept as a file under
/// `tests/fuzz-corpus/frontend/`, still gets a diagnostic or an acceptance, never
/// a crash. A file named `*.kp` is replayed through a file path; its bytes are
/// whatever was saved.
#[test]
fn the_frontend_regression_inputs_stay_survivable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fuzz-corpus")
        .join("frontend");
    let Ok(read) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension().and_then(|x| x.to_str()) == Some("kp")).then_some(p)
        })
        .collect();
    files.sort();
    let mut failures = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for json in [false, true] {
            if let Verdict::Failed(why) = run_check(&bytes, f, &scratch("regress"), "", false, json) {
                failures.push(format!("{}: {why}", f.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

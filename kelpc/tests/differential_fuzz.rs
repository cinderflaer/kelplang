//! Differential fuzzing and the conformance corpus (MK7 S5, `plan/mk7-llvm-backend.md`
//! §4.1, §4.2, §5 and §7).
//!
//! The corpus in `tests/fixtures/` is a curated set a person wrote, and the
//! plan records that it has been blind three times inside one session: a float
//! `List` element only a hand-written probe could see, an enum slice's
//! wrong-width load, and a "byte-identical" claim over 6 of 235 targets that a
//! later measurement found false. A curated corpus is a strong regression
//! instrument and a weak assurance instrument, because it only covers the
//! shapes its author thought of. This file generates programs instead.
//!
//! What it does, and why each part exists:
//!
//! 1. **A deterministic generator.** A hand-written PRNG (SplitMix64, no new
//!    dependency) turns one `u64` seed into one valid Kelp program over the
//!    landed surface: scalars and their arithmetic, comparisons, `if`/`while`,
//!    calls and `print`; `datac`; `enum` with `when`/`if let`/`?`; `Bytes`/
//!    `String` and the drop plan; `List`/`Map`/`Box`/`[T; N]`; references; and
//!    closures and function values. The same seed always produces the same
//!    text, so a failure has a name a reader can quote.
//! 2. **The differential run.** Each program is written to a file *before* it is
//!    built, built with both `--backend=c` and `--backend=llvm`, and both
//!    binaries are run. Stdout must match byte for byte and the exit statuses
//!    must match. A disagreement is a failure and never a skip.
//! 3. **The conformance corpus.** `tests/fuzz-corpus/*.kp` holds a checked-in
//!    subset of the generator's own output, one file per seed, so the sweep
//!    grows into a permanent suite instead of a one-off run. The corpus is run
//!    differentially by [`the_conformance_corpus_agrees_between_the_two_backends`]
//!    and checked to be exactly the generator's output by
//!    [`the_corpus_files_are_the_generators_own_output`], so it cannot drift.
//!
//! The generator is biased toward the shapes the plan names as previously
//! blind, because a generator over straight-line arithmetic would re-find only
//! what the corpus already covers: a float `List` element, an enum slice, an
//! owned element read out of a container, a `datac` that owns a `String` inside
//! another `datac`, a closure over an owned capture, and a `pd` read inside an
//! initializer all get a higher inclusion weight than the others.
//!
//! ## Running it
//!
//! The default suite runs the cheap plumbing tests only: one pinned seed is
//! built with both backends and compared (`the_generator_and_corpus_helpers_agree`),
//! and every corpus file is checked against the generator (no compilation).
//! The two heavy tests are `#[ignore]`d so `cargo test --release` stays
//! reasonable, the way `backend_coverage.rs` and `memory_gate.rs` are:
//!
//! ```text
//! # the fixed seed list, 256 programs, both backends:
//! cargo test --release --test differential_fuzz -- --ignored --nocapture
//!
//! # and the same knobs the memory gate has, for a longer run:
//! FUZZ_SEED_START=1000 FUZZ_SEED_COUNT=2000 \
//!   cargo test --release --test differential_fuzz -- --ignored --nocapture
//! ```
//!
//! `FUZZ_SEED_START` (default 1) and `FUZZ_SEED_COUNT` (default 256) select the
//! seed list, so a gate can be repeatable and still be run big enough to mean
//! something. The generated programs are written under
//! `CARGO_TARGET_TMPDIR/differential_fuzz/`, named by seed, so a mismatch is a
//! file someone can read.
//!
//! `FUZZ_EMIT=1 cargo test --release --test differential_fuzz -- --ignored
//! emit_the_conformance_corpus --nocapture` is the one way the corpus is
//! rewritten: it writes `CORPUS_SEEDS` back to `tests/fuzz-corpus/`. It refuses
//! to run without the variable so a stray `--ignored` cannot touch the tree.

use std::collections::BTreeSet;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ---------------------------------------------------------------------------
// The deterministic PRNG.
// ---------------------------------------------------------------------------

/// SplitMix64, hand-written so this gate adds no dependency and so the stream
/// is a fixed function of the seed rather than a library's version of one. One
/// `next` is the whole state transition; every draw below is a slice of it.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Rng {
        // A seed of zero would still work (the addend moves it), but mixing the
        // seed first keeps the low seeds from sharing their first draws.
        Rng {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03,
        }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A draw in `0..n`. `n` is never zero at a call site.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A draw in `lo..=hi`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }

    /// True with probability `n/of`.
    fn chance(&mut self, n: u64, of: u64) -> bool {
        self.below(of) < n
    }
}

// ---------------------------------------------------------------------------
// The generator.
// ---------------------------------------------------------------------------

/// The shape families a program can draw from. Each labels one region of the
/// landed surface; [`generate`] reports the ones a program actually used, so a
/// failure names the shapes behind it and the corpus can be checked to cover
/// all of them.
const SHAPES: &[&str] = &[
    "scalar",
    "calls",
    "datac",
    "owns",
    "enum",
    "try",
    "strings",
    "list",
    "list_float",
    "list_owned",
    "map",
    "box",
    "array",
    "refs",
    "closure",
    "pd",
];

/// The shapes the plan names as previously blind. They are drawn more often,
/// because a uniform draw over all shapes is what lets the blind ones hide.
const BLIND: &[&str] = &["owns", "enum", "list_float", "list_owned", "closure", "pd"];

/// One generated program and the shapes it drew.
struct Program {
    source: String,
    shapes: Vec<&'static str>,
}

/// The file-scope types every program carries. Emitting them all keeps the
/// generator's declaration bookkeeping out of the shapes, and an unused type is
/// not an error.
const TYPE_DECLS: &str = "\
datac ZPoint(var x: i32, var y: i32)

datac ZVec(val x: f64, val y: f64)

datac ZLabel(val text: String)

datac ZRow(val name: String, val tag: i32, val inner: ZLabel)

enum ZShape {
    Circle(val r: f64)
    Rect(val w: f64, val h: f64)
    Empty
}

enum ZMsg {
    Num(val n: i32)
    Text(val s: String)
    Nil
}

enum ZRes {
    Ok(val value: i32)
    Err(val why: i32)
}
";

/// The generator state. Every method draws from one [`Rng`], so the text is a
/// pure function of the seed.
struct Gen {
    rng: Rng,
}

/// A literal `i32`; a negative one is parenthesized so `a - -3` cannot tokenize
/// as `a --`.
fn i32_literal(v: i64) -> String {
    if v < 0 {
        format!("({v})")
    } else {
        v.to_string()
    }
}

/// The float literals the generator draws from. All are non-zero and exact in
/// binary, so a division cannot reach infinity and both backends print the same
/// digits.
const F64_LITERALS: &[&str] = &[
    "1.5", "2.25", "0.5", "3.75", "0.125", "10.0", "0.25", "7.5", "100.0", "0.0625",
];

/// The words a generated string literal holds.
const WORDS: &[&str] = &[
    "kelp", "alpha", "beta", "word", "x", "yz", "seed", "text", "hi", "data",
];

impl Gen {
    fn i32_lit(&mut self) -> String {
        i32_literal(self.rng.range(-40, 40))
    }

    fn f64_lit(&mut self) -> String {
        self.rng.pick(F64_LITERALS).to_string()
    }

    fn word(&mut self) -> String {
        self.rng.pick(WORDS).to_string()
    }

    fn quoted(&mut self) -> String {
        format!("\"{}\"", self.word())
    }

    /// An `i32` expression tree. Leaves are the given variables or literals;
    /// every binary operator is parenthesized, so precedence is never an
    /// accident. Division and remainder take a literal divisor, which is never
    /// zero, so no generated program traps on an integer divide.
    fn i32_expr(&mut self, vars: &[&str], depth: u32) -> String {
        if depth == 0 || self.rng.below(5) == 0 {
            if !vars.is_empty() && self.rng.chance(1, 2) {
                return self.rng.pick(vars).to_string();
            }
            return self.i32_lit();
        }
        let a = self.i32_expr(vars, depth - 1);
        let b = self.i32_expr(vars, depth - 1);
        match self.rng.below(9) {
            0 => format!("({a} + {b})"),
            1 => format!("({a} - {b})"),
            2 => format!("({a} * {b})"),
            3 => format!("({a} & {b})"),
            4 => format!("({a} | {b})"),
            5 => format!("({a} ^ {b})"),
            6 => {
                let d = self.rng.range(1, 9);
                format!("({a} / {d})")
            }
            7 => {
                let d = self.rng.range(1, 9);
                format!("({a} % {d})")
            }
            _ => {
                let k = self.rng.below(4);
                format!("({a} << {k})")
            }
        }
    }

    /// An `f64` expression tree. Only a literal divides, for the reason
    /// [`i32_expr`] gives: a zero divisor here would print `inf` and make the
    /// two backends agree on a value nobody wants to compare.
    fn f64_expr(&mut self, vars: &[&str], depth: u32) -> String {
        if depth == 0 || self.rng.below(5) == 0 {
            if !vars.is_empty() && self.rng.chance(1, 2) {
                return self.rng.pick(vars).to_string();
            }
            return self.f64_lit();
        }
        let a = self.f64_expr(vars, depth - 1);
        let b = self.f64_expr(vars, depth - 1);
        match self.rng.below(4) {
            0 => format!("({a} + {b})"),
            1 => format!("({a} - {b})"),
            2 => format!("({a} * {b})"),
            _ => {
                let d = self.f64_lit();
                format!("({a} / {d})")
            }
        }
    }

    /// A `bool` expression: comparisons of `i32`/`f64` trees and the logical
    /// operators over them.
    fn bool_expr(&mut self, ivars: &[&str], fvars: &[&str], depth: u32) -> String {
        if depth == 0 || self.rng.below(6) == 0 {
            return match self.rng.below(4) {
                0 => "true".to_string(),
                1 => "false".to_string(),
                2 => {
                    let a = self.f64_expr(fvars, 1);
                    let b = self.f64_expr(fvars, 1);
                    let op = self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
                    format!("({a} {op} {b})")
                }
                _ => {
                    let a = self.i32_expr(ivars, 1);
                    let b = self.i32_expr(ivars, 1);
                    let op = self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
                    format!("({a} {op} {b})")
                }
            };
        }
        match self.rng.below(5) {
            0 => {
                let a = self.bool_expr(ivars, fvars, depth - 1);
                let b = self.bool_expr(ivars, fvars, depth - 1);
                format!("({a} && {b})")
            }
            1 => {
                let a = self.bool_expr(ivars, fvars, depth - 1);
                let b = self.bool_expr(ivars, fvars, depth - 1);
                format!("({a} || {b})")
            }
            2 => {
                let a = self.bool_expr(ivars, fvars, depth - 1);
                format!("(!{a})")
            }
            3 => {
                let a = self.f64_expr(fvars, 2);
                let b = self.f64_expr(fvars, 2);
                let op = self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
                format!("({a} {op} {b})")
            }
            _ => {
                let a = self.i32_expr(ivars, 2);
                let b = self.i32_expr(ivars, 2);
                let op = self.rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
                format!("({a} {op} {b})")
            }
        }
    }

    // -- individual shape families ------------------------------------------
    //
    // Each returns the `fun`s it needs at file scope and the lines of its body
    // block. A shape owns a block, so its local names cannot collide with any
    // other shape's, and its end of block is a drop point.

    fn shape_scalar(&mut self) -> (Vec<String>, Vec<String>) {
        let a = self.i32_lit();
        let b = self.i32_lit();
        let x = self.f64_lit();
        let y = self.f64_lit();
        let k = self.rng.range(3, 6);
        let e1 = self.i32_expr(&["a", "b"], 2);
        let e2 = self.i32_expr(&["a", "b"], 3);
        let ef = self.f64_expr(&["x", "y"], 2);
        let cond = self.bool_expr(&["a", "b"], &["x", "y"], 2);
        let t = self.i32_expr(&["a", "b"], 1);
        let f = self.i32_expr(&["a", "b"], 1);
        let lc = self.bool_expr(&["a", "b", "i"], &["x", "y"], 2);
        let le = self.i32_expr(&["a", "b", "i"], 2);
        let le2 = self.i32_expr(&["a", "b", "i"], 2);
        let nb = self.bool_expr(&["a", "b"], &["x", "y"], 1);
        (
            vec![],
            vec![
                format!("val a = {a}"),
                format!("val b = {b}"),
                format!("val x = {x}"),
                format!("val y = {y}"),
                format!("print({e1})"),
                format!("print({e2})"),
                format!("print({ef})"),
                format!("print({cond})"),
                format!("if {cond} {{"),
                format!("    print({t})"),
                "} else {".to_string(),
                format!("    print({f})"),
                "}".to_string(),
                "var i = 0".to_string(),
                "var acc = 0".to_string(),
                format!("while i < {k} {{"),
                format!("    if {lc} {{"),
                format!("        acc = acc + {le}"),
                "    } else {".to_string(),
                format!("        acc = acc - {le2}"),
                "    }".to_string(),
                "    i = i + 1".to_string(),
                "}".to_string(),
                "print(acc)".to_string(),
                format!("print(!{nb})"),
            ],
        )
    }

    fn shape_calls(&mut self) -> (Vec<String>, Vec<String>) {
        let e = self.i32_expr(&["a", "b"], 2);
        let g = self.f64_expr(&["a", "b"], 2);
        let decls = vec![
            format!("fun z_add(a: i32, b: i32).R: i32 = {e}"),
            format!("fun z_scale(a: f64, b: f64).R: f64 = {g}"),
            "fun z_mk(n: i32).R: ZPoint = ZPoint(x = n, y = n + 1)".to_string(),
        ];
        let lines = vec![
            format!("print(z_add({}, {}))", self.i32_lit(), self.i32_lit()),
            format!(
                "print(z_add(z_add({}, {}), {}))",
                self.i32_lit(),
                self.i32_lit(),
                self.i32_lit()
            ),
            format!("print(z_scale({}, {}))", self.f64_lit(), self.f64_lit()),
            format!("val p = z_mk({})", self.i32_lit()),
            "print(p.x)".to_string(),
            "print(p.y)".to_string(),
        ];
        (decls, lines)
    }

    fn shape_datac(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec![
            "fun z_psum(p: ZPoint).R: i32 = p.x + p.y".to_string(),
            "fun z_vsum(v: ZVec).R: f64 = v.x + v.y".to_string(),
        ];
        let lines = vec![
            format!(
                "var p = ZPoint(x = {}, y = {})",
                self.i32_lit(),
                self.i32_lit()
            ),
            "print(p.x)".to_string(),
            format!("p.x = p.x + {}", self.i32_lit()),
            "print(p.x)".to_string(),
            "print(z_psum(p))".to_string(),
            format!("val v = ZVec(x = {}, y = {})", self.f64_lit(), self.f64_lit()),
            "print(z_vsum(v))".to_string(),
            format!(
                "val w = ZVec(x = {}, y = {})",
                self.f64_lit(),
                self.f64_lit()
            ),
            "print(w.x)".to_string(),
            "print(z_vsum(w))".to_string(),
        ];
        (decls, lines)
    }

    /// `datac` owning a `String` inside a `datac`, read through a borrow and
    /// moved into a second binding, so the drop plan runs for the nested field.
    fn shape_owns(&mut self) -> (Vec<String>, Vec<String>) {
        let q1 = self.quoted();
        let q2 = self.quoted();
        let q3 = self.quoted();
        let q4 = self.quoted();
        let tag1 = self.i32_lit();
        let tag2 = self.i32_lit();
        let lines = vec![
            format!(
                "val row = ZRow(name = String.from_str({q1}), tag = {tag1}, inner = ZLabel(text = String.from_str({q2})))"
            ),
            "print(row.name)".to_string(),
            "print(row.tag)".to_string(),
            "print(row.inner.text)".to_string(),
            "print(row.inner.text.len())".to_string(),
            format!(
                "val owner = ZRow(name = String.from_str({q3}), tag = {tag2}, inner = ZLabel(text = String.from_str({q4})))"
            ),
            "val moved = owner".to_string(),
            "print(moved.name)".to_string(),
            "print(moved.inner.text)".to_string(),
        ];
        (vec![], lines)
    }

    fn shape_enum(&mut self) -> (Vec<String>, Vec<String>) {
        let s = match self.rng.below(3) {
            0 => format!("ZShape.Circle(r = {})", self.f64_lit()),
            1 => format!(
                "ZShape.Rect(w = {}, h = {})",
                self.f64_lit(),
                self.f64_lit()
            ),
            _ => "ZShape.Empty".to_string(),
        };
        let m = match self.rng.below(3) {
            0 => format!("ZMsg.Num(n = {})", self.i32_lit()),
            1 => format!("ZMsg.Text(s = String.from_str({}))", self.quoted()),
            _ => "ZMsg.Nil".to_string(),
        };
        let lines = vec![
            format!("val s = {s}"),
            "when s {".to_string(),
            "    Circle(r) -> print(r)".to_string(),
            "    Rect(w, h) -> print(w * h)".to_string(),
            "    Empty -> print(0.0)".to_string(),
            "}".to_string(),
            format!("val m = {m}"),
            "when m {".to_string(),
            "    Num(n) -> print(n)".to_string(),
            "    Text(t) -> print(t)".to_string(),
            "    Nil -> print(\"nil\")".to_string(),
            "}".to_string(),
            "if let Num(k) = m {".to_string(),
            "    print(k)".to_string(),
            "} else {".to_string(),
            "    print(-1)".to_string(),
            "}".to_string(),
            "if let Circle(r) = s {".to_string(),
            "    print(r * 2.0)".to_string(),
            "}".to_string(),
        ];
        (vec![], lines)
    }

    /// `?` inside a function whose return type is the enum, the shape the
    /// operator exists for.
    fn shape_try(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec![
            "fun z_try(n: i32).R: ZRes {".to_string(),
            "    if n > 0 {".to_string(),
            "        return ZRes.Ok(value = n)".to_string(),
            "    }".to_string(),
            "    return ZRes.Err(why = 0)".to_string(),
            "}".to_string(),
            "fun z_chain(n: i32).R: ZRes {".to_string(),
            "    val v = z_try(n)?".to_string(),
            "    return ZRes.Ok(value = v + 1)".to_string(),
            "}".to_string(),
            "fun z_val(r: ZRes).R: i32 = when r {".to_string(),
            "    Ok(v) -> v".to_string(),
            "    Err(_e) -> -1".to_string(),
            "}".to_string(),
        ];
        let pos = self.rng.range(1, 50);
        let nonpos = self.rng.range(-50, 0);
        let lines = vec![
            format!("print(z_val(r = z_try({pos})))"),
            format!("print(z_val(r = z_try({nonpos})))"),
            format!("print(z_val(r = z_chain({pos})))"),
            format!("print(z_val(r = z_chain({nonpos})))"),
        ];
        (decls, lines)
    }

    /// The owned buffers and their drop plan: a `String` and a `Bytes` built,
    /// grown, printed, and dropped at the end of the block.
    fn shape_strings(&mut self) -> (Vec<String>, Vec<String>) {
        let byte1 = self.rng.range(1, 250);
        let byte2 = self.rng.range(1, 250);
        let q1 = self.quoted();
        let q2 = self.quoted();
        let q3 = self.quoted();
        let lines = vec![
            "var b = Bytes.empty()".to_string(),
            format!("b.push({byte1})"),
            format!("b.push({byte2})"),
            format!("val tail = Bytes.from_str({q1})"),
            "b.append(tail)".to_string(),
            "print(b.len())".to_string(),
            "print(b.get(0))".to_string(),
            "print(b.is_empty())".to_string(),
            format!("val s = String.from_str({q2})"),
            "print(s)".to_string(),
            "print(s.len())".to_string(),
            format!("val e = String.from_str({q3})"),
            "print(e)".to_string(),
        ];
        (vec![], lines)
    }

    fn shape_list(&mut self) -> (Vec<String>, Vec<String>) {
        let (l1, l2, l3, l4) = (
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
        );
        let lines = vec![
            "var xs: List<i32> = List.empty()".to_string(),
            "print(xs.len())".to_string(),
            "print(xs.is_empty())".to_string(),
            format!("xs.push({l1})"),
            format!("xs.push({l2})"),
            format!("xs.push({l3})"),
            "print(xs.get(0))".to_string(),
            format!("xs.set(1, {l4})"),
            "print(xs.get(1))".to_string(),
            "print(xs.pop())".to_string(),
            "var i: usize = 0".to_string(),
            "var sum = 0".to_string(),
            "while i < xs.len() {".to_string(),
            "    sum = sum + xs.get(i)".to_string(),
            "    i += 1".to_string(),
            "}".to_string(),
            "print(sum)".to_string(),
            "for v in xs {".to_string(),
            "    print(v)".to_string(),
            "}".to_string(),
            "xs.reverse()".to_string(),
            "print(xs.get(0))".to_string(),
            "xs.clear()".to_string(),
            "print(xs.len())".to_string(),
        ];
        (vec![], lines)
    }

    /// A `List` whose element is a `float`: the shape the plan records as the
    /// first blind spot, where the index and the copy width disagree.
    fn shape_list_float(&mut self) -> (Vec<String>, Vec<String>) {
        let (f1, f2, f3) = (self.f64_lit(), self.f64_lit(), self.f64_lit());
        let probe = self.f64_lit();
        let lines = vec![
            "var fs: List<f64> = List.empty()".to_string(),
            format!("fs.push({f1})"),
            format!("fs.push({f2})"),
            format!("fs.push({f3})"),
            format!("fs.push({probe})"),
            "print(fs.len())".to_string(),
            "print(fs.get(0))".to_string(),
            "print(fs.get(3))".to_string(),
            format!("print(fs.contains({probe}))"),
            format!("print(fs.contains({f1}))"),
            "fs.sort()".to_string(),
            "print(fs.get(0))".to_string(),
            "print(fs.get(3))".to_string(),
            "val copy = fs.clone()".to_string(),
            "print(copy.len())".to_string(),
            "val mid = fs.slice(0, 2)".to_string(),
            "print(mid.get(0))".to_string(),
            "print(mid.get(1))".to_string(),
            "var i: usize = 0".to_string(),
            "var s = 0.0".to_string(),
            "while i < fs.len() {".to_string(),
            "    s = s + fs.get(i)".to_string(),
            "    i += 1".to_string(),
            "}".to_string(),
            "print(s)".to_string(),
            "var narrow: List<f32> = List.empty()".to_string(),
            "narrow.push(1.5)".to_string(),
            "narrow.push(2.5)".to_string(),
            "print(narrow.get(1))".to_string(),
            "print(narrow.contains(2.5))".to_string(),
        ];
        (vec![], lines)
    }

    /// An owned element read out of a container: `get` hands back a borrow for
    /// a drop-carrying element, `pop` moves one out, and `set` drops the old
    /// element in place.
    fn shape_list_owned(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec![
            "fun z_msg_int(m: ZMsg).R: i32 = when m {".to_string(),
            "    Num(n) -> n".to_string(),
            "    Text(t) -> t.len().to_i32()".to_string(),
            "    Nil -> -1".to_string(),
            "}".to_string(),
        ];
        let (q1, q2, q3, q4) = (
            self.quoted(),
            self.quoted(),
            self.quoted(),
            self.quoted(),
        );
        let n = self.i32_lit();
        let lines = vec![
            "var msgs: List<ZMsg> = List.empty()".to_string(),
            format!("msgs.push(ZMsg.Text(s = String.from_str({q1})))"),
            format!("msgs.push(ZMsg.Num(n = {n}))"),
            "msgs.push(ZMsg.Nil)".to_string(),
            "print(msgs.len())".to_string(),
            "print(z_msg_int(m = msgs.get(0)))".to_string(),
            format!("msgs.set(i = 1, v = ZMsg.Text(s = String.from_str({q2})))"),
            "print(z_msg_int(m = msgs.get(1)))".to_string(),
            "val taken = msgs.pop()".to_string(),
            "print(z_msg_int(m = taken))".to_string(),
            "msgs.clear()".to_string(),
            "print(msgs.len())".to_string(),
            "var labels: List<ZLabel> = List.empty()".to_string(),
            format!("labels.push(ZLabel(text = String.from_str({q3})))"),
            format!("labels.push(ZLabel(text = String.from_str({q4})))"),
            "print(labels.get(0).text)".to_string(),
            "val last = labels.pop()".to_string(),
            "print(last.text)".to_string(),
            "labels.clear()".to_string(),
            "print(labels.len())".to_string(),
        ];
        (decls, lines)
    }

    fn shape_map(&mut self) -> (Vec<String>, Vec<String>) {
        let (k1, k2, k3) = (
            self.rng.range(1, 20),
            self.rng.range(21, 40),
            self.rng.range(1, 40),
        );
        let (v1, v2, v3) = (
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
        );
        let lines = vec![
            "var m: Map<i64, i32> = Map.new()".to_string(),
            "print(m.len())".to_string(),
            format!("m.insert({k1}, {v1})"),
            format!("m.insert({k2}, {v2})"),
            "print(m.len())".to_string(),
            format!("print(m.get({k1}))"),
            format!("print(m.has({k2}))"),
            format!("print(m.has({k3}))"),
            format!("print(m.get_or({k3}, 99))"),
            format!("m.insert({k1}, {v3})"),
            format!("print(m.get({k1}))"),
            format!("print(m.remove({k2}))"),
            format!("print(m.len())"),
            "val ks = m.keys()".to_string(),
            "var i: usize = 0".to_string(),
            "while i < ks.len() {".to_string(),
            "    print(ks.get(i))".to_string(),
            "    i += 1".to_string(),
            "}".to_string(),
            "var bools: Map<bool, i64> = Map.new()".to_string(),
            "bools.insert(true, 5)".to_string(),
            "bools.insert(false, 7)".to_string(),
            "print(bools.get(true))".to_string(),
            "print(bools.len())".to_string(),
        ];
        (vec![], lines)
    }

    fn shape_box(&mut self) -> (Vec<String>, Vec<String>) {
        let n1 = self.i32_lit();
        let n2 = self.i32_lit();
        let f1 = self.f64_lit();
        let q1 = self.quoted();
        let lines = vec![
            "var b: Box<i32> = Box.empty()".to_string(),
            "print(b.is_empty())".to_string(),
            format!("b.set({n1})"),
            "print(b.get())".to_string(),
            "print(b.take())".to_string(),
            "print(b.is_empty())".to_string(),
            format!("val lb = Box.of(ZLabel(text = String.from_str({q1})))"),
            "print(lb.get().text)".to_string(),
            format!("val nested = Box.of(Box.of({n2}))"),
            "print(nested.get().get())".to_string(),
            "var fb: Box<f64> = Box.empty()".to_string(),
            format!("fb.set({f1})"),
            "print(fb.get())".to_string(),
        ];
        (vec![], lines)
    }

    fn shape_array(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec!["fun z_pair_sum(p: [f64; 2]).R: f64 = p.get(i = 0) + p.get(i = 1)"
            .to_string()];
        let (a1, a2, a3) = (
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
        );
        let (f1, f2, f3) = (self.f64_lit(), self.f64_lit(), self.f64_lit());
        let (g1, g2, g3, g4) = (
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
            self.i32_lit(),
        );
        let lines = vec![
            format!("val xs: [i32; 3] = [{a1}, {a2}, {a3}]"),
            "print(xs.len())".to_string(),
            "print(xs.get(i = 1))".to_string(),
            "for v in xs {".to_string(),
            "    print(v)".to_string(),
            "}".to_string(),
            format!("var buf: [f64; 3] = [{f1}, {f2}, {f3}]"),
            format!("buf.set(i = 2, v = {f1})"),
            "print(buf.get(i = 2))".to_string(),
            format!("val pair: [f64; 2] = [{f2}, {f3}]"),
            "print(z_pair_sum(pair))".to_string(),
            format!("val grid: [[i32; 2]; 2] = [[{g1}, {g2}], [{g3}, {g4}]]"),
            "print(grid.get(i = 1).get(i = 0))".to_string(),
            format!(
                "val pts: [ZPoint; 2] = [ZPoint(x = {a1}, y = {a2}), ZPoint(x = {a3}, y = {g1})]"
            ),
            "print(pts.get(i = 0).x)".to_string(),
            "print(pts.get(i = 1).y)".to_string(),
        ];
        (decls, lines)
    }

    /// References that are not a `datac`: a `mut` parameter, an explicit
    /// `&mut` binding, the read and the store through the pointer, and a
    /// shared borrow of a scalar and of a `datac` field.
    fn shape_refs(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec![
            "fun z_bump(p: mut i32, d: i32) {".to_string(),
            "    *p = *p + d".to_string(),
            "}".to_string(),
            "fun z_scale_f(p: mut f64, by: f64) {".to_string(),
            "    *p = *p * by".to_string(),
            "}".to_string(),
        ];
        let n = self.i32_lit();
        let d = self.i32_lit();
        let f = self.f64_lit();
        let by = self.f64_lit();
        let px = self.i32_lit();
        let py = self.i32_lit();
        let lines = vec![
            format!("var n: i32 = {n}"),
            format!("z_bump(n, {d})"),
            "print(n)".to_string(),
            format!("var f: f64 = {f}"),
            format!("z_scale_f(f, {by})"),
            "print(f)".to_string(),
            "val r = &n".to_string(),
            "print(*r)".to_string(),
            format!("var p = ZPoint(x = {px}, y = {py})"),
            "val q = &p".to_string(),
            "print((*q).x)".to_string(),
            format!("p.y = p.y + {d}"),
            "print((*q).y)".to_string(),
        ];
        (decls, lines)
    }

    /// Closures and function values: a closure over an owned capture, one over
    /// a `Copy` capture, a bare `fun` name as a value, a function type as a
    /// parameter, and one environment per loop iteration so the drop at the end
    /// of an iteration is exercised.
    fn shape_closure(&mut self) -> (Vec<String>, Vec<String>) {
        let decls = vec![
            "fun z_double(v: i32).R: i32 = v * 2".to_string(),
            "fun z_apply(f: fun(i32).R: i32, v: i32).R: i32 = f(v)".to_string(),
        ];
        let q1 = self.quoted();
        let k = self.i32_lit();
        let n = self.i32_lit();
        let q2 = self.quoted();
        let lines = vec![
            format!("val s = String.from_str({q1})"),
            "val f = fun() .R: usize = s.len()".to_string(),
            "print(f())".to_string(),
            format!("val k = {k}"),
            "val g = fun(x: i32).R: i32 = x + k".to_string(),
            format!("print(g({n}))"),
            "val h = z_double".to_string(),
            format!("print(h({n}))"),
            format!("print(z_apply(z_double, {n}))"),
            format!("print(z_apply(h, {n}))"),
            "var i = 0".to_string(),
            "while i < 3 {".to_string(),
            format!("    val t = String.from_str({q2})"),
            "    val c = fun() .R: usize = t.len()".to_string(),
            "    print(c())".to_string(),
            "    i = i + 1".to_string(),
            "}".to_string(),
        ];
        (decls, lines)
    }

    /// A `pd` read: in an expression, in the initializer of another `pd`, and
    /// in a `datac` literal, which is the shape the plan names.
    fn shape_pd(&mut self) -> (Vec<String>, Vec<String>) {
        let lines = vec![
            "print(z_base)".to_string(),
            "print(z_doubled)".to_string(),
            "print(z_name)".to_string(),
            "print(z_primes.len())".to_string(),
            "print(z_primes.get(0))".to_string(),
            "print(z_origin.x)".to_string(),
            "print(z_origin.y)".to_string(),
            "print(z_origin.x + z_origin.y)".to_string(),
        ];
        (vec![], lines)
    }
}

/// Wraps body lines in a block, indenting every line of every element by four
/// spaces. An element may hold its own newlines and relative indentation, so a
/// nested block reads correctly.
fn block(lines: &[String]) -> String {
    let mut out = String::from("    {\n");
    for line in lines {
        for sub in line.lines() {
            out.push_str("        ");
            out.push_str(sub);
            out.push('\n');
        }
    }
    out.push_str("    }\n");
    out
}

/// Generates the program for one seed. The text is a pure function of the seed.
fn generate(seed: u64) -> Program {
    let mut g = Gen {
        rng: Rng::new(seed),
    };

    let mut shapes: Vec<&'static str> = Vec::new();
    for shape in SHAPES {
        let take = *shape == "scalar"
            || if BLIND.contains(shape) {
                g.rng.chance(3, 4)
            } else {
                g.rng.chance(1, 2)
            };
        if take {
            shapes.push(shape);
        }
    }

    let mut funs: Vec<String> = Vec::new();
    let mut body = String::new();
    for shape in &shapes {
        let (decls, lines) = match *shape {
            "scalar" => g.shape_scalar(),
            "calls" => g.shape_calls(),
            "datac" => g.shape_datac(),
            "owns" => g.shape_owns(),
            "enum" => g.shape_enum(),
            "try" => g.shape_try(),
            "strings" => g.shape_strings(),
            "list" => g.shape_list(),
            "list_float" => g.shape_list_float(),
            "list_owned" => g.shape_list_owned(),
            "map" => g.shape_map(),
            "box" => g.shape_box(),
            "array" => g.shape_array(),
            "refs" => g.shape_refs(),
            "closure" => g.shape_closure(),
            "pd" => g.shape_pd(),
            other => unreachable!("no generator for shape '{other}'"),
        };
        funs.extend(decls);
        body.push_str(&block(&lines));
        body.push('\n');
    }

    let base = g.rng.range(1, 20);
    let name = g.quoted();
    let globals = format!(
        "pd val z_base = {base}\n\
         pd val z_doubled = z_base * 2\n\
         pd val z_name = String.from_str({name})\n\
         pd val z_origin = ZPoint(x = z_base, y = z_doubled)\n\
         pd val z_primes = List.of(7)\n"
    );

    let mut source = String::new();
    source.push_str(&format!(
        "// fuzz seed {seed}; shapes: {}\n\n",
        shapes.join(", ")
    ));
    source.push_str(&globals);
    source.push('\n');
    source.push_str(TYPE_DECLS);
    source.push('\n');
    for f in &funs {
        source.push_str(f);
        source.push('\n');
    }
    source.push_str("\nfun main() {\n");
    source.push_str(&body);
    source.push_str("}\n");

    Program { source, shapes }
}

// ---------------------------------------------------------------------------
// The differential run.
// ---------------------------------------------------------------------------

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

/// Where the generated programs and the two builds of each are written, rooted
/// in `CARGO_TARGET_TMPDIR` the way the other heavy tests are.
fn scratch_root() -> PathBuf {
    let mut p = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    p.push("differential_fuzz");
    p
}

fn build(backend: &str, source: &Path, bin: &Path) -> Output {
    Command::new(kelpc_bin())
        .args(["build", &format!("--backend={backend}")])
        .arg(source)
        .arg("-o")
        .arg(bin)
        .env_remove("KELPC_BACKEND")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

/// One binary's outcome: a clean exit code, or the signal that killed it.
struct Run {
    code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
}

fn run(bin: &Path) -> Run {
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
    fn ending(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => "an unknown termination".to_string(),
        }
    }
}

/// What one program did under the two backends. The heavy tests turn the three
/// non-`Agreed` variants into counts and failures; a mismatch is never allowed
/// to be one of the numbers a run reports and moves on from.
enum Outcome {
    /// Both backends built it and agreed on stdout and exit status.
    Agreed,
    /// The frontend or the C backend refused it, so the generator emitted an
    /// invalid program. Always a generator bug, never acceptable.
    Rejected(String),
    /// The `llvm` backend did not build it. At S5 the surface is landed, so
    /// this is a gap or an internal error, never a named refusal to tolerate.
    LlvmFailed(String),
    /// Both backends built it and disagreed on stdout or exit status.
    Disagreed(String),
}

/// Builds one program with both backends, runs both, and compares them. The
/// failure strings name the seed and the program path, and show both outputs
/// and both exit statuses, so a mismatch is diagnosable from the report alone.
fn check(seed: u64, shapes: &[&str], source: &Path, dir: &Path) -> Outcome {
    // The binary path must not end in `.c`: the C backend writes the generated
    // C beside the binary (`<out>.c`), and `cc` refuses a source that is also
    // its own output. A directory per backend keeps the two builds apart too.
    let c_dir = dir.join("c");
    let l_dir = dir.join("llvm");
    for d in [&c_dir, &l_dir] {
        std::fs::create_dir_all(d)
            .unwrap_or_else(|e| panic!("could not create '{}': {e}", d.display()));
    }
    let c_bin = c_dir.join(format!("seed_{seed:05}"));
    let l_bin = l_dir.join(format!("seed_{seed:05}"));

    let c_build = build("c", source, &c_bin);
    if !c_build.status.success() {
        return Outcome::Rejected(format!(
            "seed {seed} ({}): the C backend refused it at '{}', so the generator \
             is wrong:\n{}",
            shapes.join(" "),
            source.display(),
            String::from_utf8_lossy(&c_build.stderr).trim_end(),
        ));
    }

    let l_build = build("llvm", source, &l_bin);
    if !l_build.status.success() {
        return Outcome::LlvmFailed(format!(
            "seed {seed} ({}): the llvm backend failed to build '{}' ({}) while the \
             C backend built it:\n{}",
            shapes.join(" "),
            source.display(),
            l_build.status.code().map_or("signal".to_string(), |c| format!(
                "exit {c}"
            )),
            String::from_utf8_lossy(&l_build.stderr).trim_end(),
        ));
    }

    let c_run = run(&c_bin);
    let l_run = run(&l_bin);

    for (backend, finished) in [("c", &c_run), ("llvm", &l_run)] {
        if finished.signal.is_some() {
            return Outcome::Disagreed(format!(
                "seed {seed} ({}): the '{backend}' binary for '{}' was {}",
                shapes.join(" "),
                source.display(),
                finished.ending(),
            ));
        }
    }

    if c_run.stdout != l_run.stdout || c_run.code != l_run.code {
        return Outcome::Disagreed(format!(
            "seed {seed} ({}): '{}' disagrees between the backends\n  \
             c    ({}): {:?}\n  llvm ({}): {:?}\n  the program is '{}'",
            shapes.join(" "),
            source.display(),
            c_run.ending(),
            String::from_utf8_lossy(&c_run.stdout),
            l_run.ending(),
            String::from_utf8_lossy(&l_run.stdout),
            source.display(),
        ));
    }

    Outcome::Agreed
}

/// The seed list for the heavy run: `FUZZ_SEED_START` and `FUZZ_SEED_COUNT`
/// select it, and the defaults are fixed so the gate is repeatable.
fn fuzz_seeds() -> (u64, u64) {
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
const CORPUS_SEEDS: &[u64] = &[
    1, 2, 3, 5, 8, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83,
];

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fuzz-corpus")
}

fn corpus_path(seed: u64) -> PathBuf {
    corpus_dir().join(format!("corpus_{seed:05}.kp"))
}

/// The seed encoded in a corpus file name, or `None` for another name.
fn corpus_seed_of(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_prefix("corpus_")?.parse().ok()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// The cheap default-suite check: the generator is deterministic, and one
/// pinned seed is built with both backends and agrees. It also proves the
/// generator's own output is valid Kelp, because the C backend builds it.
#[test]
fn the_generator_and_corpus_helpers_agree() {
    // Determinism: the same seed is the same bytes, twice.
    let a = generate(7);
    let b = generate(7);
    assert_eq!(a.source, b.source, "the generator is not deterministic");
    assert_ne!(
        generate(8).source,
        a.source,
        "two seeds produced one program, so the seed does not reach the text"
    );

    let dir = scratch_root();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    let source = dir.join("pinned_seed_7.kp");
    std::fs::write(&source, &a.source)
        .unwrap_or_else(|e| panic!("could not write '{}': {e}", source.display()));

    match check(7, &a.shapes, &source, &dir) {
        Outcome::Agreed => {}
        Outcome::Rejected(m) | Outcome::LlvmFailed(m) | Outcome::Disagreed(m) => {
            panic!("{m}")
        }
    }
}

/// The corpus files are exactly the generator's output for the seeds they are
/// named for, and together they cover every shape. No compilation here, so it
/// is cheap enough for the default suite; it is what stops the checked-in
/// corpus from drifting away from the generator that made it.
#[test]
fn the_corpus_files_are_the_generators_own_output() {
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();
    for seed in CORPUS_SEEDS {
        let path = corpus_path(*seed);
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "'{}' is missing ({e}); regenerate it with FUZZ_EMIT=1 \
                 `cargo test --release --test differential_fuzz -- --ignored \
                 emit_the_conformance_corpus`",
                path.display()
            )
        });
        let generated = generate(*seed);
        assert_eq!(
            on_disk, generated.source,
            "'{}' is not the generator's output for seed {seed}; regenerate the \
             corpus (FUZZ_EMIT=1) if the generator changed on purpose",
            path.display(),
        );
        covered.extend(generated.shapes);
    }
    for shape in SHAPES {
        assert!(
            covered.contains(shape),
            "the conformance corpus never exercises the '{shape}' shape; add a \
             seed that draws it to CORPUS_SEEDS",
        );
    }
}

/// The heavy half: the fixed seed list, both backends, byte-for-byte stdout and
/// exit status. Every program is written under the scratch directory before it
/// is built, so a failure is a file someone can open. The counts the plan's
/// §5 wants are printed with `--nocapture`.
#[test]
#[ignore = "generates and compiles hundreds of programs with both backends; run with --ignored"]
fn the_generator_agrees_between_the_two_backends() {
    let dir = scratch_root();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));

    let (start, count) = fuzz_seeds();
    let mut agreed = 0usize;
    let mut rejected = 0usize;
    let mut llvm_failed = 0usize;
    let mut mismatches = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();

    for seed in start..start + count {
        let program = generate(seed);
        covered.extend(program.shapes.iter().copied());

        let source = dir.join(format!("seed_{seed:05}.kp"));
        std::fs::write(&source, &program.source)
            .unwrap_or_else(|e| panic!("could not write '{}': {e}", source.display()));

        match check(seed, &program.shapes, &source, &dir) {
            Outcome::Agreed => agreed += 1,
            Outcome::Rejected(m) => {
                rejected += 1;
                failures.push(m);
            }
            Outcome::LlvmFailed(m) => {
                llvm_failed += 1;
                failures.push(m);
            }
            Outcome::Disagreed(m) => {
                mismatches += 1;
                failures.push(m);
            }
        }
    }

    let missing: Vec<&str> = SHAPES
        .iter()
        .copied()
        .filter(|s| !covered.contains(s))
        .collect();
    println!(
        "differential fuzz: seeds {start}..{} generated {count} programs; \
         {agreed} built by both backends and agreed, {rejected} refused by the \
         frontend or the C backend, {llvm_failed} failed only on llvm, \
         {mismatches} mismatch(es); shapes not drawn: {}",
        start + count,
        if missing.is_empty() {
            "none".to_string()
        } else {
            missing.join(", ")
        },
    );

    assert!(
        failures.is_empty(),
        "{} failing program(s) out of {count} generated (seed {start} upward); \
         the programs are under '{}':\n\n{}",
        failures.len(),
        dir.display(),
        failures.join("\n\n"),
    );
}

/// The differential run of the checked-in corpus: the same comparison, over the
/// files that ship instead of over freshly generated text.
#[test]
#[ignore = "builds and runs the whole fuzz corpus with both backends; run with --ignored"]
fn the_conformance_corpus_agrees_between_the_two_backends() {
    let dir = scratch_root();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));

    let mut entries: Vec<(u64, PathBuf)> = std::fs::read_dir(corpus_dir())
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", corpus_dir().display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("kp") {
                return None;
            }
            Some((corpus_seed_of(&path)?, path))
        })
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "'{}' holds no corpus programs",
        corpus_dir().display(),
    );

    let mut failures: Vec<String> = Vec::new();
    for (seed, source) in &entries {
        let shapes = generate(*seed).shapes;
        match check(*seed, &shapes, source, &dir) {
            Outcome::Agreed => {}
            Outcome::Rejected(m) | Outcome::LlvmFailed(m) | Outcome::Disagreed(m) => {
                failures.push(m)
            }
        }
    }

    println!(
        "conformance corpus: {} program(s), {} failure(s)",
        entries.len(),
        failures.len(),
    );
    assert!(
        failures.is_empty(),
        "{} corpus program(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

/// Rewrites `tests/fuzz-corpus/` from [`CORPUS_SEEDS`]. Guarded by `FUZZ_EMIT`
/// so a stray `--ignored` run cannot touch the tree; this is the only way the
/// corpus changes.
#[test]
#[ignore = "writes the conformance corpus; needs FUZZ_EMIT=1"]
fn emit_the_conformance_corpus() {
    if std::env::var("FUZZ_EMIT").as_deref() != Ok("1") {
        println!(
            "FUZZ_EMIT is not 1: {} corpus file(s) would be written to '{}'",
            CORPUS_SEEDS.len(),
            corpus_dir().display(),
        );
        return;
    }
    std::fs::create_dir_all(corpus_dir())
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", corpus_dir().display()));
    for seed in CORPUS_SEEDS {
        let program = generate(*seed);
        let path = corpus_path(*seed);
        std::fs::write(&path, &program.source)
            .unwrap_or_else(|e| panic!("could not write '{}': {e}", path.display()));
        println!("wrote {} ({})", path.display(), program.shapes.join(" "));
    }
}

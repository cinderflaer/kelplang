//! Compile time must not blow up with the size or the nesting of a program.
//!
//! Every test writes one large, generated program (a long method chain, deeply
//! nested calls, a long `+` chain, ...) and runs `check`, `fmt`, `lint`, `doc`
//! and `build` over it, then runs the binary and compares what it prints with
//! the answer the generator knows. A step that takes
//! longer than `STEP_LIMIT` fails the test: the limit is far above what a linear
//! pass needs, so only a super-linear pass (the exponential one this file was
//! written for was a method receiver checked twice per link) can trip it.
//!
//! Sizes stay under the parser's limits (`tooling/fuzzing.md` (kelp wiki): 800 chain links and
//! 200 levels of nesting), which are part of what is accepted.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const STEP_LIMIT: Duration = Duration::from_secs(20);

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-scaling-tests"),
    };
    let dir = root.join("scaling").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Runs `cmd` bounded in memory and time (the same bounds every program in this
/// repository is run under) and returns its stdout, failing the test when the
/// step is too slow or does not exit zero.
fn step(what: &str, shape: &str, cmd: &mut Command) -> String {
    let started = Instant::now();
    let mut line = format!("ulimit -v 2097152; ulimit -t 30; exec timeout 60 '{}'", cmd.get_program().to_string_lossy());
    for a in cmd.get_args() {
        line.push_str(&format!(" '{}'", a.to_string_lossy()));
    }
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(line);
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            sh.env(k, v);
        }
    }
    let out = sh.output().expect("failed to spawn sh");
    let took = started.elapsed();
    eprintln!("scaling: {shape}: {what}: {:.2}s", took.as_secs_f64());
    assert!(
        took < STEP_LIMIT,
        "{shape}: '{what}' took {:.1}s, more than {}s",
        took.as_secs_f64(),
        STEP_LIMIT.as_secs()
    );
    assert!(
        out.status.success(),
        "{shape}: '{what}' failed ({:?}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Writes `source`, pushes it through every command, builds and runs it, and
/// checks the printed answer.
fn run_shape(shape: &str, source: &str, expected: &str) {
    let dir = scratch(shape);
    let file = dir.join("main.kp");
    fs::write(&file, source).expect("write the program");
    let path = file.to_string_lossy().into_owned();

    step("check", shape, kelpc().arg("check").arg(&path));
    step("fmt", shape, kelpc().arg("fmt").arg(&path));
    step("lint", shape, kelpc().arg("lint").arg(&path));
    let docs = dir.join("docs");
    step(
        "doc",
        shape,
        kelpc().arg("doc").arg(&path).arg("-o").arg(&docs),
    );

    let exe = dir.join("main");
    let mut build = kelpc();
    build.arg("build").arg(&path).arg("-o").arg(&exe);
    step("build", shape, &mut build);
    let printed = step("run", shape, &mut Command::new(&exe));
    assert_eq!(printed.trim_end(), expected, "{shape}: printed the wrong answer");
}

/// The text of `n` copies of `piece`.
fn rep(piece: &str, n: usize) -> String {
    piece.repeat(n)
}

#[test]
fn a_long_chain_of_std_method_calls_is_linear() {
    let src = format!("fun main() {{\n    val x = 1{}\n    print(x)\n}}\n", rep(".to_i32()", 790));
    run_shape("std_chain", &src, "1");
}

#[test]
fn a_long_chain_of_user_method_calls_is_linear() {
    let src = format!(
        "datac P(var v: i32)\nfun P.me().return: P = P(v = self.v)\nfun main() {{\n    val p = P(v = 1)\n    val q = p{}\n    print(q.v)\n}}\n",
        rep(".me()", 790)
    );
    run_shape("user_chain", &src, "1");
}

#[test]
fn a_chain_mixing_receivers_and_arguments_is_linear() {
    // The receiver of every link is itself a call with an argument, so each
    // link both infers a receiver and matches an argument against it.
    let src = format!(
        "datac P(var v: i32)\nfun P.add(k: i32).return: P = P(v = self.v + k)\nfun main() {{\n    val p = P(v = 0)\n    val q = p{}\n    print(q.v)\n}}\n",
        rep(".add(1)", 400)
    );
    run_shape("arg_chain", &src, "400");
}

#[test]
fn deeply_nested_calls_are_linear() {
    let src = format!(
        "fun f(x: i32).return: i32 = x\nfun main() {{\n    val x = {}1{}\n    print(x)\n}}\n",
        rep("f(", 90),
        rep(")", 90)
    );
    run_shape("nested_calls", &src, "1");
}

#[test]
fn deeply_nested_generic_calls_are_linear() {
    let src = format!(
        "fun id<T>(x: T).return: T = x\nfun main() {{\n    val x = {}1{}\n    print(x)\n}}\n",
        rep("id(", 90),
        rep(")", 90)
    );
    run_shape("nested_generic_calls", &src, "1");
}

#[test]
fn nested_generic_instantiations_are_linear() {
    let src = format!(
        "datac W<T>(var v: T)\nfun main() {{\n    val x = {}1{}\n    print(2)\n}}\n",
        rep("W(v = ", 50),
        rep(")", 50)
    );
    run_shape("nested_generic_datac", &src, "2");
}

#[test]
fn nested_generic_enum_constructions_are_linear() {
    let src = format!(
        "enum Opt<T> {{\n    Nothing\n    Just(val v: T)\n}}\nfun main() {{\n    val x = {}1{}\n    print(2)\n}}\n",
        rep("Opt.Just(v = ", 50),
        rep(")", 50)
    );
    run_shape("nested_generic_enum", &src, "2");
}

#[test]
fn nested_blocks_are_linear() {
    let src = format!(
        "fun main() {{\n    var x = 0\n{}    x = x + 1\n{}    print(x)\n}}\n",
        rep("    if x == 0 {\n", 60),
        rep("    }\n", 60)
    );
    run_shape("nested_blocks", &src, "1");
}

#[test]
fn nested_function_literals_are_linear() {
    fn level(i: usize, n: usize, out: &mut String) {
        let ind = "    ".repeat(i + 2);
        if i == n {
            out.push_str(&format!("{ind}return x{i} + k\n"));
            return;
        }
        out.push_str(&format!("{ind}val g{i} = fun(x{}: i32).return: i32 {{\n", i + 1));
        level(i + 1, n, out);
        out.push_str(&format!("{ind}}}\n{ind}return g{i}(x{i})\n"));
    }
    let mut src = String::from("fun main() {\n    val k = 1\n    val f = fun(x0: i32).return: i32 {\n");
    level(0, 50, &mut src);
    src.push_str("    }\n    print(f(1))\n}\n");
    run_shape("nested_closures", &src, "2");
}

#[test]
fn a_long_sequence_of_closures_is_linear() {
    let mut src = String::from("fun main() {\n    val k = 1\n    val f0 = fun(x: i32).return: i32 = x + k\n");
    for i in 1..=150 {
        src.push_str(&format!("    val f{i} = fun(x: i32).return: i32 = f{}(x) + 1\n", i - 1));
    }
    src.push_str("    print(f150(0))\n}\n");
    run_shape("closure_sequence", &src, "151");
}

#[test]
fn nested_scoped_blocks_are_linear() {
    let mut src = String::from(
        "datac C(var n: i32)\nfun C.run(f: scoped fun(i32)) { f(1) }\nfun bump(c: mut C) { c.n += 1 }\nfun main() {\n    var c = C(n = 0)\n    val q = C(n = 5)\n",
    );
    // A block that captures `c` exclusively is checked again once the first
    // pass says so; the retry must not multiply with the nesting, so the
    // exclusive use sits in the innermost block of an otherwise plain nest.
    for i in 0..40 {
        src.push_str(&format!("{}q.run {{ v{i} ->\n", "    ".repeat(i + 1)));
    }
    src.push_str(&format!("{}print(v39)\n", "    ".repeat(41)));
    for i in (1..=40).rev() {
        src.push_str(&format!("{}}}\n", "    ".repeat(i)));
    }
    src.push_str("    bump(c)\n    print(c.n)\n}\n");
    run_shape("nested_scoped_blocks", &src, "1\n1");
}

#[test]
fn a_long_plus_chain_is_linear() {
    let src = format!("fun main() {{\n    val a = 1\n    val x = a{}\n    print(x)\n}}\n", rep(" + a", 400));
    run_shape("plus_chain", &src, "401");
}

#[test]
fn a_long_and_chain_is_linear() {
    let src = format!("fun main() {{\n    val a = true\n    val x = a{}\n    print(x)\n}}\n", rep(" && a", 400));
    run_shape("and_chain", &src, "true");
}

#[test]
fn a_long_list_literal_is_linear() {
    let items = vec!["1"; 5000].join(", ");
    let src = format!("fun main() {{\n    val xs = [{items}]\n    print(xs.len())\n}}\n");
    run_shape("list_literal", &src, "5000");
}

#[test]
fn a_long_string_template_is_linear() {
    let src = format!("fun main() {{\n    val a = 1\n    print(\"{}\")\n}}\n", rep("${a} ", 300));
    run_shape("string_template", &src, rep("1 ", 300).trim_end());
}

#[test]
fn a_long_elif_ladder_is_linear() {
    let mut src = String::from("fun main() {\n    val a = 7\n    if a == 0 {\n        print(0)\n    }");
    for i in 1..300 {
        src.push_str(&format!(" elif a == {i} {{\n        print({i})\n    }}"));
    }
    src.push_str(" else {\n        print(-1)\n    }\n}\n");
    run_shape("elif_ladder", &src, "7");
}

#[test]
fn a_struct_with_many_fields_is_linear() {
    let decl: Vec<String> = (0..200).map(|i| format!("var f{i}: i32")).collect();
    let init: Vec<String> = (0..200).map(|i| format!("f{i} = {i}")).collect();
    let src = format!(
        "datac S({})\nfun main() {{\n    val s = S({})\n    print(s.f0 + s.f199)\n}}\n",
        decl.join(", "),
        init.join(", ")
    );
    run_shape("many_fields", &src, "199");
}

#[test]
fn many_sequential_statements_are_linear() {
    let src = format!("fun main() {{\n    var x = 0\n{}    print(x)\n}}\n", rep("    x = x + 1\n", 300));
    run_shape("sequential_statements", &src, "300");
}

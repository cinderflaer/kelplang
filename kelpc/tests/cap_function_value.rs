//! A gated item named as a *function value* is refused, with the item named,
//! however the value is reached.
//!
//! The gate has two halves. The *domain* half lives at the naming site: a
//! module without the capability cannot name the item at all. The *pattern*
//! half is a runtime check emitted from the call's own callee key
//! (`codegen_c.rs::cap_plan`), because which argument is the resource is a
//! property of the callee's signature. A call through a function value names
//! the pointer, not the gated item, so no check is emitted for it: a module
//! that *holds* the domain could name `fs.read` as a value and read any path
//! it liked, which made a narrowed grant advisory for that one shape
//! (`BUG-k2Qv7`). The value cannot carry the naming module's patterns, so the
//! fix is to refuse the value outright, the way the domain half refuses the
//! ungranted name.
//!
//! The ways a value leaves its naming site are covered here, because a rule
//! enforced at one of them is not a rule: a `val`, a function argument, a
//! `datac` field and a `pd` initializer. The positive control is the other
//! direction, so the rule cannot pass by refusing everything: an *ungated*
//! function is still a value, in a local, a field and a parameter, and calling
//! it prints its result.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test: the tests write projects and build them, and two
/// of them running at once must not see each other's files.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-cap-function-value-tests"),
    };
    let dir = root.join("cap-function-value").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Writes the two-file project: `main.kp` imports `util` with `import_line`,
/// and `util.kp` holds `body`, whose free function is `leak`.
fn project(dir: &Path, import_line: &str, body: &str) {
    let main = format!("{import_line}\n\nfun main() {{\n    print(util.leak())\n}}\n");
    fs::write(dir.join("main.kp"), main).expect("write main.kp");
    fs::write(dir.join("util.kp"), format!("impl \"std/fs\"\n\n{body}\n")).expect("write util.kp");
}

/// Grants `util` a prefix that admits nothing under `/etc`, so the runtime
/// check's answer does not depend on the host's filesystem.
const GRANT: &str = "impl \"util\" caps(fs = [\"/kelp_cap_function_value/**\"])";
const NO_GRANT: &str = "impl \"util\"";

/// The ways a gated item leaves its naming site. Each is a whole project body:
/// `leak` names the item, calls the value at a path outside the grant, and
/// returns the length read. The four are the routes the report names: a `val`,
/// a function argument, a `datac` field, and a `pd` initializer (whose check
/// uses the `pd`'s own module).
fn shapes(item: &str) -> [(&'static str, String); 4] {
    [
        (
            "a_local",
            format!(
                "fun leak().R: usize {{\n    val f: fun(&str).R: Bytes = {item}\n    return f(\"/etc/hostname\").len()\n}}"
            ),
        ),
        (
            "an_argument",
            format!(
                "fun apply(f: fun(&str).R: Bytes).R: usize = f(\"/etc/hostname\").len()\n\nfun leak().R: usize = apply({item})"
            ),
        ),
        (
            "a_field",
            format!(
                "datac Holder(val f: fun(&str).R: Bytes)\n\nfun leak().R: usize {{\n    val h = Holder(f = {item})\n    return h.f(\"/etc/hostname\").len()\n}}"
            ),
        ),
        (
            "a_passdown",
            format!(
                "pd val source: fun(&str).R: Bytes = {item}\n\nfun leak().R: usize = source(\"/etc/hostname\").len()"
            ),
        ),
    ]
}

fn build(dir: &Path, backend: &str, bin_name: &str) -> Output {
    kelpc()
        .arg("build")
        .arg(dir.join("main.kp"))
        .arg("--backend")
        .arg(backend)
        .arg("-o")
        .arg(dir.join(bin_name))
        .output()
        .expect("failed to spawn kelpc")
}

/// Builds `dir/main.kp` with `import_line` and `body` and asserts the build is
/// refused with a message that names `item` and holds `needle`. The refusal is
/// the front end's, not a backend's: inference has already answered, so no
/// code generator ever sees the construct.
fn refused(name: &str, import_line: &str, body: &str, item: &str, needle: &str) {
    let dir = scratch(name);
    project(&dir, import_line, body);

    let c = build(&dir, "c", "c.bin");
    assert!(
        !c.status.success(),
        "expected the build to refuse '{item}' named in {name}, but it built"
    );
    let c_text = String::from_utf8_lossy(&c.stderr).to_string();
    assert!(
        c_text.contains(item),
        "the refusal for {name} does not name '{item}':\n{c_text}"
    );
    assert!(
        c_text.contains(needle),
        "the refusal for {name} does not say '{needle}':\n{c_text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A module that holds the domain still cannot name a gated item as a value:
/// the pattern check cannot follow the pointer, so the value is refused with
/// the item named, in every shape and with either spelling.
#[test]
fn a_granted_module_cannot_name_a_gated_item_as_a_function_value() {
    for item in ["fs.read", "fs.read_raw"] {
        for (shape, body) in shapes(item) {
            refused(
                &format!("granted_{}_{}", item.replace('.', "_"), shape),
                GRANT,
                &body,
                item,
                "is a gated function ('fs' capability), so it cannot be a function value",
            );
        }
    }
}

/// The domain half, unchanged: with no grant the item is refused for the
/// capability rather than for the shape of the value. Both spellings, all four
/// shapes, both backends.
#[test]
fn an_ungranted_module_cannot_name_a_gated_item_as_a_function_value() {
    for item in ["fs.read", "fs.read_raw"] {
        for (shape, body) in shapes(item) {
            refused(
                &format!("ungranted_{}_{}", item.replace('.', "_"), shape),
                NO_GRANT,
                &body,
                item,
                "needs the 'fs' capability, which module 'util' was not granted",
            );
        }
    }
}

/// The rule is about a value, not about the item. A function *literal* over
/// the same gated call is still accepted, and the check still runs: the call
/// inside the literal is a call site, so a path outside the grant is denied at
/// runtime by both backends. Without this the refusal could be over-broad and
/// no test would say so.
#[test]
fn a_function_literal_over_a_gated_call_is_still_checked() {
    let dir = scratch("literal");
    project(
        &dir,
        GRANT,
        "fun leak().R: usize {\n    val f = fun(p: &str).R: Bytes = fs.read(p)\n    return f(\"/etc/hostname\").len()\n}",
    );

    let build = build(&dir, "c", "c.bin");
    assert!(
        build.status.success(),
        "expected a function literal over 'fs.read' to build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(dir.join("c.bin"))
        .output()
        .expect("failed to run the compiled binary");
    assert!(
        !run.status.success(),
        "expected the binary to deny a path outside the grant"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("is outside the fs capability granted to module 'util'"),
        "the binary did not deny the path:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The positive control: an ungated function is a function value like any
/// other, in a local, a `datac` field and a parameter, and its call runs. This
/// project is built and run rather than only built, so "compiles" cannot hide a
/// refusal that a later phase applies.
const UNGATED: &str = r#"fun double(x: i32).R: i32 = x * 2

datac Holder(val f: fun(i32).R: i32)

fun apply(f: fun(i32).R: i32, x: i32).R: i32 = f(x)

fun main() {
    val f: fun(i32).R: i32 = double
    print(f(21))
    val h = Holder(f = double)
    print(h.f(3))
    print(apply(double, 4))
}
"#;

#[test]
fn an_ungated_function_is_still_a_value_in_a_local_a_field_and_a_parameter() {
    let dir = scratch("ungated");
    fs::write(dir.join("main.kp"), UNGATED).expect("write main.kp");

    let build = build(&dir, "c", "c.bin");
    assert!(
        build.status.success(),
        "expected an ungated function value to build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(dir.join("c.bin"))
        .output()
        .expect("failed to run the compiled binary");
    assert!(
        run.status.success(),
        "expected the binary to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "42\n6\n8\n",
        "unexpected stdout from the binary"
    );
    let _ = fs::remove_dir_all(&dir);
}

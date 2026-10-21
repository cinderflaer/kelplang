use super::*;

pub(crate) const PROC: &[&str] = &["std/process"];

/// Two scripts the test writes: `tools/ok.sh` (echoes its arguments) and
/// `tools/evil.sh`, whose only effect is to create the marker `pwned` in the
/// lab. Neither runs anything but `echo`.
pub(crate) fn tools(lab: &Lab) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(lab.root.join("tools")).unwrap();
    for (name, text) in [
        ("ok.sh", "#!/bin/sh\necho ok \"$@\"\n"),
        ("evil.sh", "#!/bin/sh\necho pwned > \"$(dirname \"$0\")/../pwned\"\necho evil\n"),
    ] {
        let p = lab.root.join("tools").join(name);
        fs::write(&p, text).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

pub(crate) fn run_prog(program_expr: &str, args: &[&str]) -> String {
    let mut s = String::from("    var args: List<String> = List.empty()\n");
    for a in args {
        s.push_str(&format!("    args.push(String.from_str(\"{a}\"))\n"));
    }
    s.push_str(&format!(
        "    val r = process.run({program_expr}, args)\n    print(r.code)\n    print(r.out)"
    ));
    s
}

pub(crate) fn no_marker(lab: &Lab) {
    assert!(!lab.root.join("pwned").exists(), "the unlisted program ran");
}

/// P1: a module without the grant cannot name `process`.
#[test]
pub(crate) fn p01_a_module_without_the_grant_cannot_name_process() {
    let lab = Lab::new("p01");
    tools(&lab);
    lab.simple("", PROC, &run_prog("\"__T__/tools/ok.sh\"", &[]));
    lab.refused(&["needs the 'process' capability"]);
}

/// P2: a program outside the grant is denied before the fork.
#[test]
pub(crate) fn p02_a_program_outside_the_grant_is_not_started() {
    let lab = Lab::new("p02");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        PROC,
        &run_prog("\"__T__/tools/evil.sh\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P3: a bare name is never looked up on `PATH`: it is not absolute, so it is a
/// denial, not a search.
#[test]
pub(crate) fn p03_a_bare_program_name_is_not_searched_on_path() {
    let lab = Lab::new("p03");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/*\"]", lab.root.display()),
        PROC,
        &run_prog("\"evil.sh\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P4: `..` and a file-as-directory segment cannot name another program.
#[test]
pub(crate) fn p04_dotdot_cannot_name_another_program() {
    for path in [
        "__T__/tools/../tools/evil.sh",
        "__T__/tools/ok.sh/../evil.sh",
        "__T__/tools/./ok.sh/../evil.sh",
    ] {
        let lab = Lab::new("p04");
        tools(&lab);
        lab.simple(
            &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
            PROC,
            &run_prog(&format!("\"{path}\""), &[]),
        );
        lab.denied("is outside the process capability");
        no_marker(&lab);
    }
}

/// P5: a name inside the grant that is a symlink to a program outside it is
/// matched after resolution, so it is denied.
#[test]
pub(crate) fn p05_a_granted_name_that_links_to_another_program_is_denied() {
    let lab = Lab::new("p05");
    tools(&lab);
    lab.symlink("evil.sh", "tools/okl");
    lab.simple(
        &format!("process = [\"{}/tools/ok*\"]", lab.root.display()),
        PROC,
        &run_prog("\"__T__/tools/okl\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P6: arguments are not interpreted by a shell: metacharacters reach the
/// granted program as literal text, so no second command runs.
#[test]
pub(crate) fn p06_argument_injection_is_inert() {
    let lab = Lab::new("p06");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        PROC,
        &run_prog(
            "\"__T__/tools/ok.sh\"",
            &["; sh __T__/tools/evil.sh", "$(sh __T__/tools/evil.sh)", "`sh __T__/tools/evil.sh`", "| sh __T__/tools/evil.sh"],
        ),
    );
    for out in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.starts_with("0\nok ; sh "), "the arguments were not literal: {stdout}");
        no_marker(&lab);
    }
}

/// P7: the documented breadth of the grant: naming a shell lets the module run
/// whatever that shell runs. The grant is on the *program*, and the arguments
/// are the module's business (`std/process.kp` says so). The test pins it so a
/// change in either direction is a decision, and runs only `echo`.
#[test]
pub(crate) fn p07_granting_a_shell_grants_what_the_shell_runs() {
    let lab = Lab::new("p07");
    tools(&lab);
    lab.simple(
        "process = [\"/bin/sh\", \"/nix/store/**\", \"/usr/bin/**\"]",
        PROC,
        &run_prog("\"/bin/sh\"", &["-c", "echo through-the-shell"]),
    );
    lab.prints("0\nthrough-the-shell\n\n");
}

/// P8: `process.kill` signals only a child this process started. Signal 0 to
/// the test's own pid (argv[1]) is harmless if it were delivered, and it must
/// not be: a narrow program grant is not authority over every process of the
/// user (the Minecraft server, a shell, the parent).
#[test]
pub(crate) fn p08_kill_reaches_only_children_this_program_started() {
    let lab = Lab::new("p08");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        &["std/process", "std/str", "std/result"],
        "    val args = process.args()\n    val pid = result.expect(r = str.parse_u64(args.get(1).as_str()), message = \"pid\")\n    process.kill_raw(pid.to_i32(), 0)\n    print(\"signalled a process that is not a child\")",
    );
    let pid = std::process::id().to_string();
    let b = lab.build();
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let out = lab.run(&[&pid]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
        "kill reached a process this program did not start: {stdout}{stderr}"
    );
}

/// P9: a forged `Child` handle carries the same fate as P8: `Child.kill` goes
/// through the same primitive, so naming a foreign pid in a handle does not
/// reach it either.
#[test]
pub(crate) fn p09_a_forged_child_handle_cannot_signal_a_foreign_pid() {
    let lab = Lab::new("p09");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        &["std/process", "std/io", "std/str", "std/result"],
        "    val args = process.args()\n    val pid = result.expect(r = str.parse_u64(args.get(1).as_str()), message = \"pid\")\n    val c = process.Child(pid = pid.to_i32(), stdin = io.File(fd = -1), stdout = io.File(fd = -1), stderr = io.File(fd = -1), exited = false, code = 0, signal = 0)\n    c.kill(0)\n    print(\"signalled a process that is not a child\")",
    );
    let pid = std::process::id().to_string();
    let b = lab.build();
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let out = lab.run(&[&pid]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
        "a forged handle signalled a foreign pid: {stdout}{stderr}"
    );
}

// ---------------------------------------------------------------------------
// env
// ---------------------------------------------------------------------------

pub(crate) const ENV: &[&str] = &["std/env"];

/// E1: a name outside the grant can be neither read, probed, set nor unset.
#[test]
pub(crate) fn e01_a_name_outside_the_grant_is_denied_for_every_operation() {
    for body in [
        "    print(env.get(\"HOME\"))",
        "    print(env.has(\"HOME\"))",
        "    env.set(\"HOME\", \"x\")",
        "    env.unset(\"HOME\")",
        "    print(env.get_raw(\"HOME\"))",
    ] {
        let lab = Lab::new("e01");
        lab.simple("env = [\"APP_MODE\"]", ENV, body);
        lab.denied("is outside the env capability");
    }
}

/// E2: two modules with disjoint grants: neither can read or write the other's
/// variable, even though both are granted `env`.
#[test]
pub(crate) fn e02_one_modules_variable_is_not_anothers() {
    let lab = Lab::new("e02");
    lab.src(
        "main.kp",
        "impl \"a\" caps(env = [\"VAR_A\"])\nimpl \"b\" caps(env = [\"VAR_B\"])\n\nfun main() {\n    a.set()\n    b.steal()\n}\n",
    );
    lab.src("a.kp", "impl \"std/env\"\n\npub fun set() {\n    env.set(\"VAR_A\", \"secret-a\")\n}\n");
    lab.src("b.kp", "impl \"std/env\"\n\npub fun steal() {\n    print(env.get(\"VAR_A\"))\n}\n");
    lab.denied("is outside the env capability");
}

/// E3: a wildcard name grant does not cross `/`, and a name that merely starts
/// like the pattern but is a different variable is outside it.
#[test]
pub(crate) fn e03_a_name_wildcard_matches_names_only() {
    for name in ["APP_/../HOME", "APP", "XAPP_MODE", "app_mode"] {
        let lab = Lab::new("e03");
        lab.simple("env = [\"APP_*\"]", ENV, &format!("    print(env.get(\"{name}\"))"));
        lab.denied("is outside the env capability");
    }
}

/// E4: the environment cannot be enumerated: `env.names` does not exist, so a
/// one-name grant cannot be widened by listing.
#[test]
pub(crate) fn e04_the_environment_cannot_be_listed() {
    let lab = Lab::new("e04");
    lab.simple("env = [\"APP_MODE\"]", ENV, "    print(env.names().len())");
    lab.refused(&["names"]);
}

/// E5: a one-name grant does not reach `LD_PRELOAD` or `PATH`: the variables
/// that change what a *later* process start does are ordinary names under the
/// ordinary check.
#[test]
pub(crate) fn e05_loader_variables_need_their_own_grant() {
    for name in ["LD_PRELOAD", "LD_LIBRARY_PATH", "PATH"] {
        let lab = Lab::new("e05");
        lab.simple(
            "env = [\"APP_MODE\"]",
            ENV,
            &format!("    env.set(\"{name}\", \"/tmp/x\")"),
        );
        lab.denied("is outside the env capability");
    }
}

/// E6: a module that holds both `env` and `process` is the composition the
/// model documents rather than prevents: with `env = ["*"]` it can set
/// `LD_PRELOAD` for the programs it may start. That is two grants the
/// manifest author wrote, each narrow, and the test pins the check that does
/// hold: with `env` granted only a harmless name, the child sees nothing it
/// set under another name.
#[test]
pub(crate) fn e06_a_child_only_sees_variables_the_module_could_name() {
    let lab = Lab::new("e06");
    tools(&lab);
    lab.simple(
        &format!(
            "env = [\"APP_MODE\"], process = [\"{}/tools/ok.sh\"]",
            lab.root.display()
        ),
        &["std/env", "std/process"],
        "    env.set(\"LD_PRELOAD\", \"/nonexistent/evil.so\")\n    print(\"unreachable\")",
    );
    lab.denied("is outside the env capability");
}

// ---------------------------------------------------------------------------
// net
// ---------------------------------------------------------------------------

use super::*;

pub(crate) const UNSAFE_MANIFEST: &str = "name = \"u\"\n\n[unsafe]\nmodules = [\"main\", \"listed\"]\n";
pub(crate) const NOT_LISTED: &str = "is not listed in coral.toml [unsafe] modules";

/// Builds a project whose `evil` module (not on the audit list) holds `evil`,
/// and asserts the build is refused with `needle`.
pub(crate) fn unsafe_attack(name: &str, evil: &str, needle: &str) {
    let lab = Lab::new(name);
    lab.manifest_project(
        UNSAFE_MANIFEST,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
            ("evil.kp", evil),
        ],
    );
    lab.refused(&[needle]);
}

/// U1: an `unsafe` block or an `unsafe fun` in a module that is not listed.
#[test]
pub(crate) fn u01_unsafe_in_an_unlisted_module_is_refused() {
    unsafe_attack(
        "u01a",
        "pub fun go() {\n    var x: i32 = 1\n    val p: Ptr<i32> = Ptr.from_mut(x)\n    print(unsafe { p.read() })\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u01b",
        "unsafe fun poke(p: Ptr<i32>) => i32 = p.read()\n\npub fun go() {\n}\n",
        NOT_LISTED,
    );
}

/// U2: the same operation hidden in a position the audit walk has to reach:
/// a closure, a generic function, a `when` arm, an `if`/`else` branch, a
/// template, a call argument, a tuple, an array, a loop condition, a `for`
/// iterable, a `defer`.
#[test]
pub(crate) fn u02_unsafe_hidden_in_an_expression_position_is_still_found() {
    let head = "pub fun go() {\n    var x: i32 = 1\n    val p: Ptr<i32> = Ptr.from_mut(x)\n";
    for (what, body) in [
        ("closure", "    val f = fun() => i32 = unsafe { p.read() }\n    print(f())"),
        ("if_cond", "    if unsafe { p.read() } == 1 {\n        print(1)\n    }"),
        ("else_branch", "    if x == 2 {\n        print(0)\n    } else {\n        print(unsafe { p.read() })\n    }"),
        ("when_arm", "    val o: i32 = 1\n    val v = when o {\n        _ -> unsafe { p.read() }\n    }"),
        ("template", "    print(\"v={unsafe { p.read() }}\")"),
        ("call_arg", "    print(abs_of(unsafe { p.read() }))"),
        ("tuple", "    val t = (1, unsafe { p.read() })"),
        ("array", "    val a = [1, unsafe { p.read() }]"),
        ("while_cond", "    while unsafe { p.read() } == 0 {\n        print(1)\n    }"),
        ("for_iterable", "    for i in unsafe { make_list(p) } {\n        print(i)\n    }"),
        ("defer", "    defer print(unsafe { p.read() })\n    print(2)"),
        ("assign", "    var y: i32 = 0\n    y = unsafe { p.read() }"),
        ("return", "    print(helper(p))"),
    ] {
        let evil = format!(
            "fun abs_of(n: i32) => i32 = n\nfun make_list(p: Ptr<i32>) => List<i32> {{\n    var l: List<i32> = List.empty()\n    return l\n}}\nfun helper(p: Ptr<i32>) => i32 {{\n    return unsafe {{ p.read() }}\n}}\n{head}{body}\n}}\n"
        );
        unsafe_attack(&format!("u02_{what}"), &evil, NOT_LISTED);
    }
}

/// U3: a trait's default method lives in the declaration and not in the
/// module's function list, and the audit walk used to skip it, so a module
/// outside the list could keep an `unsafe` body that every impl inherited.
#[test]
pub(crate) fn u03_unsafe_in_a_trait_default_method_is_found() {
    unsafe_attack(
        "u03",
        "pub trait Poke {\n    fun Self.addr() => usize\n    fun Self.poke() => i32 {\n        val p: Ptr<i32> = Ptr.from_addr(self.addr())\n        return unsafe { p.read() }\n    }\n}\n\npub datac A(val a: usize)\n\nimpl Poke for A {\n    fun Self.addr() => usize = a\n}\n\npub fun go() {\n    var x: i32 = 7\n    val p: Ptr<i32> = Ptr.from_mut(x)\n    val a = A(a = p.addr())\n    print(a.poke())\n}\n",
        NOT_LISTED,
    );
}

/// U4: an impl method and a generic function with an `unsafe` body.
#[test]
pub(crate) fn u04_unsafe_in_an_impl_method_or_a_generic_is_found() {
    unsafe_attack(
        "u04a",
        "pub trait Get {\n    fun Self.get() => i32\n}\n\npub datac A(val p: Ptr<i32>)\n\nimpl Get for A {\n    fun Self.get() => i32 = unsafe { p.read() }\n}\n\npub fun go() {\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u04b",
        "fun peek<T>(p: Ptr<i32>, x: T) => i32 = unsafe { p.read() }\n\npub fun go() {\n    var x: i32 = 1\n    print(peek(Ptr.from_mut(x), 1))\n}\n",
        NOT_LISTED,
    );
}

/// U5: `asm` and `naked fun` are unsafe operations: an unlisted module cannot
/// contain either.
#[test]
pub(crate) fn u05_asm_and_naked_fun_need_the_audit_list() {
    unsafe_attack(
        "u05a",
        "pub fun go() {\n    unsafe { asm(\"nop\") }\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u05b",
        "naked fun forty_two() => i64 {\n    asm(\"mov rax, 42\\nret\")\n}\n\npub fun go() {\n    print(forty_two())\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u05c",
        "pub fun go() {\n    asm(\"nop\")\n}\n",
        NOT_LISTED,
    );
}

/// U6: an `unsafe` initializer of a `const`, `static` or `pd` is found too.
#[test]
pub(crate) fn u06_unsafe_in_a_module_level_initializer_is_found() {
    unsafe_attack(
        "u06a",
        "static mut N: i32 = 0\n\npub fun go() {\n    unsafe { N = 1 }\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u06b",
        "pd val V: i32 = unsafe { read_it() }\n\nunsafe fun read_it() => i32 = 1\n\npub fun go() {\n    print(V)\n}\n",
        NOT_LISTED,
    );
}

/// U7: an unlisted module cannot call a listed module's `unsafe fun`, with or
/// without an `unsafe` block of its own, and a `static mut` it does not own is
/// out of reach the same way. The listed module's *safe* wrapper is the audit
/// design working: it is callable and does its job (the positive control).
#[test]
pub(crate) fn u07_an_unlisted_module_reaches_unsafe_only_through_listed_wrappers() {
    let listed = "pub unsafe fun raw(p: Ptr<i32>) => i32 = p.read()\n\npub fun safe_read(x: i32) => i32 {\n    var y: i32 = x\n    val p: Ptr<i32> = Ptr.from_mut(y)\n    return unsafe { raw(p) }\n}\n";
    for (what, evil, needle) in [
        (
            "no_block",
            "impl \"listed\"\n\npub fun go() {\n    var x: i32 = 1\n    print(listed.raw(Ptr.from_mut(x)))\n}\n",
            "is an 'unsafe fun'",
        ),
        (
            "with_block",
            "impl \"listed\"\n\npub fun go() {\n    var x: i32 = 1\n    print(unsafe { listed.raw(Ptr.from_mut(x)) })\n}\n",
            NOT_LISTED,
        ),
    ] {
        let lab = Lab::new(&format!("u07_{what}"));
        lab.manifest_project(
            UNSAFE_MANIFEST,
            &[
                ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
                ("listed.kp", listed),
                ("evil.kp", evil),
            ],
        );
        lab.refused(&[needle]);
    }
    let lab = Lab::new("u07_wrapper");
    lab.manifest_project(
        UNSAFE_MANIFEST,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
            ("listed.kp", listed),
            ("evil.kp", "impl \"listed\"\n\npub fun go() {\n    print(listed.safe_read(41))\n}\n"),
        ],
    );
    lab.prints("41\n");
}

// ---------------------------------------------------------------------------
// mmio (freestanding)
// ---------------------------------------------------------------------------

pub(crate) const MMIO_LINKER: &str = "ENTRY(_start)\nSECTIONS\n{\n    . = 0x80000000;\n    .text : { *(.text.boot) *(.text .text.*) }\n}\n";

pub(crate) fn mmio_lab(name: &str, caps: &str, files: &[(&str, &str)]) -> Lab {
    let lab = Lab::new(name);
    lab.manifest_project(
        &format!(
            "name = \"mmio_attack\"\n\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\nlinker_script = \"kernel.ld\"\n\n[unsafe]\nmodules = [\"main\", \"dev\"]\n\n{caps}"
        ),
        files,
    );
    fs::write(lab.root.join("kernel.ld"), MMIO_LINKER).unwrap();
    lab
}

pub(crate) fn poke_at(expr: &str) -> String {
    format!(
        "pub fun poke() {{\n    val p: Ptr<u8> = unsafe {{ Ptr.from_addr({expr}) }}\n    unsafe {{ p.write_volatile(65) }}\n}}\n"
    )
}

pub(crate) const KMAIN_DEV: &str = "impl \"dev\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke()\n    while true { unsafe { asm(\"wfi\") } }\n}\n";

/// M1: a driver module with no `mmio` grant cannot make a pointer from an
/// address, constant or not.
#[test]
pub(crate) fn m01_a_module_without_an_mmio_grant_cannot_take_an_address() {
    let lab = mmio_lab(
        "m01",
        "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
        &[
            (
                "main.kp",
                "impl \"dev\"\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke()\n    while true { unsafe { asm(\"wfi\") } }\n}\n",
            ),
            ("dev.kp", &poke_at("0x10000000")),
        ],
    );
    lab.check_refused(&["needs an 'mmio' grant", "module 'dev' holds none"]);
}

/// M2: constant folding must mean what the emitted code computes. A shift by
/// 64 or more folds to 0 in the compiler's 128-bit arithmetic but is a shift
/// by `n mod 64` on the target, so `0x10000000 + (0x100000 << 70)` looked like
/// the in-range `0x10000000` and addressed `0x14000000`. A shift that wide is
/// now refused where it is written (a constant shift by the width or more is a
/// compile error; before tl7 it was "cannot fold", which a range grant refused).
#[test]
pub(crate) fn m02_an_oversized_shift_does_not_fold_into_the_granted_range() {
    for expr in [
        "0x10000000 + (0x100000 << 70)",
        "0x10000000 + (0x100000 << 64)",
        "0x10000000 | (0x4000000 << 128)",
        "0x10000000 + (0x1 >> 70)",
    ] {
        let lab = mmio_lab(
            "m02",
            "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
            &[("main.kp", KMAIN_DEV), ("dev.kp", &poke_at(expr))],
        );
        lab.check_refused(&["a shift by", "is not defined for a 'usize'"]);
    }
}

/// M3: an address computed from another pointer, a parameter or a runtime
/// value is not a constant, so only the whole-address-space grant admits it.
#[test]
pub(crate) fn m03_a_computed_address_is_refused_under_a_range_grant() {
    let dev = "pub fun poke(base: usize) {\n    val p: Ptr<u8> = unsafe { Ptr.from_addr(base + 4) }\n    unsafe { p.write_volatile(65) }\n}\n";
    let main = "impl \"dev\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke(0x10000000)\n    while true { unsafe { asm(\"wfi\") } }\n}\n";
    let lab = mmio_lab("m03", "[caps]\nmmio = [\"0x10000000+0x100\"]\n", &[("main.kp", main), ("dev.kp", dev)]);
    lab.check_refused(&["cannot fold"]);
}

/// M4: a module cannot delegate a bigger window than it holds, nor the whole
/// address space from a range.
#[test]
pub(crate) fn m04_a_range_cannot_be_widened_by_delegation() {
    for wider in ["0x10000000+0x1000", "0x0F000000+0x100", "*"] {
        let main = format!(
            "impl \"mid\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {{\n    mid.go()\n    while true {{ unsafe {{ asm(\"wfi\") }} }}\n}}\n"
        );
        let mid = format!("impl \"dev\" caps(mmio = [\"{wider}\"])\n\npub fun go() {{\n    dev.poke()\n}}\n");
        let lab = mmio_lab(
            "m04",
            "[caps]\nmmio = [\"0x10000000+0x1000\"]\n",
            &[("main.kp", &main), ("mid.kp", &mid), ("dev.kp", &poke_at("0x10000000"))],
        );
        lab.check_refused(&["cannot grant"]);
    }
}

/// M5 (known gap): the range is checked where a pointer is *born*. Arithmetic
/// on the pointer afterwards reaches any address, so a granted window is not a
/// fence. The design is provenance (roadmap D-1): a pointer born from a range
/// carries it, and `offset`/`byte_offset`/`cast` keep it. The honest record is
/// in `language/capabilities.md` (kelp wiki) ("Born-at only"); this is its reproduction.
#[test]
#[ignore = "known gap: mmio ranges are checked at the pointer's birth, not after offset arithmetic (plan/device-caps.md, plan/threat-model.md M5)"]
pub(crate) fn m05_offset_arithmetic_leaves_the_granted_window() {
    let dev = "pub fun poke() {\n    val base: Ptr<u8> = unsafe { Ptr.from_addr(0x10000000) }\n    val far: Ptr<u8> = base.byte_offset(0x10000000)\n    unsafe { far.write_volatile(65) }\n}\n";
    let lab = mmio_lab(
        "m05",
        "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
        &[("main.kp", KMAIN_DEV), ("dev.kp", dev)],
    );
    lab.check_refused(&["outside"]);
}

// ---------------------------------------------------------------------------
// indirect use
// ---------------------------------------------------------------------------

pub(crate) const NEEDS_FS: &str = "needs the 'fs' capability, which module 'evil' was not granted";

/// Runs a three-module project: `main` grants `util` the `data/**` window and
/// leaves `evil` ungranted; `evil` is the attacker.
pub(crate) fn indirect(name: &str, util: &str, evil: &str) -> Lab {
    let lab = Lab::new(name);
    lab.src(
        "main.kp",
        "impl \"util\" caps(fs = [\"__T__/data/**\"])\nimpl \"evil\"\n\nfun main() {\n    evil.go()\n}\n",
    );
    lab.src("util.kp", util);
    lab.src("evil.kp", evil);
    lab
}

/// I1: a function value or closure made by a granted module and handed to an
/// ungranted one is still checked against the *maker's* grant when called.
#[test]
pub(crate) fn i01_a_closure_from_a_granted_module_keeps_its_makers_grant() {
    let lab = Lab::new("i01");
    lab.src(
        "main.kp",
        "impl \"util\" caps(fs = [\"__T__/data/**\"])\nimpl \"evil\"\n\nfun main() {\n    evil.go(util.reader())\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/fs\"\n\npub fun reader() => fun(&str) => String = fun(p: &str) => String = fs.read_text(p)\n",
    );
    lab.src("evil.kp", "pub fun go(f: fun(&str) => String) {\n    print(f(\"__T__/secret/s.txt\"))\n}\n");
    lab.denied("granted to module 'util'");
}

/// I2: a generic function in a granted module is checked against that module's
/// grant whatever type the caller instantiates it with; a generic function in
/// an ungranted module cannot name `fs` at all.
#[test]
pub(crate) fn i02_generics_do_not_change_whose_grant_applies() {
    let lab = indirect(
        "i02a",
        "impl \"std/fs\"\n\npub fun get<T>(p: &str, x: T) => String = fs.read_text(p)\n",
        "impl \"util\"\n\npub fun go() {\n    print(util.get(\"__T__/secret/s.txt\", 1))\n}\n",
    );
    lab.denied("granted to module 'util'");
    let lab = indirect(
        "i02b",
        "pub fun x() {\n}\n",
        "impl \"std/fs\"\n\nfun get<T>(p: &str, x: T) => String = fs.read_text(p)\n\npub fun go() {\n    print(get(\"__T__/secret/s.txt\", 1))\n}\n",
    );
    lab.refused(&[NEEDS_FS]);
}

/// I3: an ungranted module cannot reach `fs` through a trait impl, a trait
/// default body supplied by a granted module, a `Drop` impl, a `defer`, a
/// scoped block or a module-level initializer: each is code of the module that
/// writes it (or of the impl's module), and is checked as such.
#[test]
pub(crate) fn i03_traits_drop_defer_scoped_and_initializers_do_not_launder_a_grant() {
    let util = "impl \"std/fs\"\n\npub trait Loader {\n    fun Self.path() => String\n    fun Self.load() => String {\n        return fs.read_text(self.path().as_str())\n    }\n}\n\npub fun with(cb: scoped fun() => i32) => i32 = cb()\n";
    for (what, evil) in [
        (
            "trait_default",
            "impl \"util\"\n\npub datac L(val p: String)\n\nimpl util.Loader for L {\n    fun Self.path() => String = String.from_str(p.as_str())\n}\n\npub fun go() {\n    val l = L(p = String.from_str(\"__T__/secret/s.txt\"))\n    print(l.load())\n}\n",
        ),
        (
            "trait_impl",
            "impl \"std/fs\"\nimpl \"util\"\n\npub datac L(val p: String)\n\nimpl util.Loader for L {\n    fun Self.path() => String = fs.read_text(\"__T__/secret/s.txt\")\n}\n\npub fun go() {\n}\n",
        ),
        (
            "drop",
            "impl \"std/fs\"\n\ndatac Guard(val n: i32)\n\nfun mut Guard.drop() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n\npub fun go() {\n    val g = Guard(n = 1)\n}\n",
        ),
        (
            "defer",
            "impl \"std/fs\"\n\npub fun go() {\n    defer print(fs.read_text(\"__T__/secret/s.txt\"))\n    print(1)\n}\n",
        ),
        (
            "scoped",
            "impl \"util\"\nimpl \"std/fs\"\n\npub fun go() {\n    print(util.with(fun() => i32 = fs.read_text(\"__T__/secret/s.txt\").len()))\n}\n",
        ),
        (
            "pd_initializer",
            "impl \"std/fs\"\n\npd val S: String = fs.read_text(\"__T__/secret/s.txt\")\n\npub fun go() {\n    print(S.len())\n}\n",
        ),
    ] {
        let lab = indirect(&format!("i03_{what}"), util, evil);
        lab.refused(&["capability"]);
    }
}

/// I4: re-export tricks: an ungranted module cannot reach `fs` through the
/// import of a module that holds it (`util.fs.read_text`), and cannot delegate
/// itself a grant on its own `impl` line.
#[test]
pub(crate) fn i04_importing_a_granted_module_does_not_import_its_grant() {
    let lab = indirect(
        "i04a",
        "impl \"std/fs\"\n\npub fun x() {\n}\n",
        "impl \"util\"\n\npub fun go() {\n    print(util.fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["util"]);
    let lab = indirect(
        "i04b",
        "pub fun x() {\n}\n",
        "impl \"std/fs\" caps(fs = [\"/**\"])\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["cannot grant"]);
}

/// I5: a user `extern fun` has no body unless the manifest binds it, so an
/// ungranted module cannot declare the library's own raw primitive or libc's
/// `system` and call it.
#[test]
pub(crate) fn i05_a_user_extern_needs_a_manifest_entry() {
    for decl in [
        "extern fun read_raw(path: &str) => Bytes",
        "extern fun system(cmd: &str) => i32",
    ] {
        let lab = indirect(
            "i05",
            "pub fun x() {\n}\n",
            &format!("{decl}\n\npub fun go() {{\n}}\n"),
        );
        lab.refused(&["has no compiler-supplied body"]);
    }
}

// ---------------------------------------------------------------------------
// delegation and the manifest
// ---------------------------------------------------------------------------

/// D1: a module cannot hand out more than it holds: a wider pattern, a parent
/// directory, the whole-resource wildcard, in any domain.
#[test]
pub(crate) fn d01_a_module_cannot_delegate_more_than_it_holds() {
    for (grant_line, domain_import) in [
        ("fs = [\"/**\"]", "std/fs"),
        ("fs = [\"__T__/**\"]", "std/fs"),
        ("fs = [\"__T__/data/../**\"]", "std/fs"),
        ("net = [\"*\"]", "std/net"),
        ("env = [\"*\"]", "std/env"),
        ("process = [\"/**\"]", "std/process"),
    ] {
        let lab = Lab::new("d01");
        lab.src(
            "main.kp",
            "impl \"util\" caps(fs = [\"__T__/data/**\"], net = [\"127.0.0.1:1\"], env = [\"A\"], process = [\"/bin/true\"])\n\nfun main() {\n    util.go()\n}\n",
        );
        lab.src(
            "util.kp",
            &format!("impl \"{domain_import}\" caps({grant_line})\n\npub fun go() {{\n}}\n"),
        );
        lab.refused(&[]);
    }
}

/// D2: `caps(*)` is the whole resource and only a module that holds the whole
/// resource may hand it over: from a narrow importer it is refused.
#[test]
pub(crate) fn d02_the_wildcard_delegation_is_bounded_by_the_importer() {
    let lab = Lab::new("d02");
    lab.src(
        "main.kp",
        "impl \"mid\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    mid.go()\n}\n",
    );
    lab.src("mid.kp", "impl \"leaf\" caps(*)\n\npub fun go() {\n    leaf.go()\n}\n");
    lab.src(
        "leaf.kp",
        "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["whole-resource wildcard"]);
}

/// D3: the manifest's `[caps]` narrows the entry module itself: it cannot read
/// outside its own window, and cannot grant an import more than the manifest
/// gave it.
#[test]
pub(crate) fn d03_the_manifest_narrows_the_entry_module() {
    let lab = Lab::new("d03a");
    lab.manifest_project(
        "name = \"d\"\n\n[caps]\nfs = [\"__T__/data/**\"]\n",
        &[(
            "main.kp",
            "impl \"std/fs\"\n\nfun main() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
        )],
    );
    lab.denied("granted to module 'main'");
    let lab = Lab::new("d03b");
    lab.manifest_project(
        "name = \"d\"\n\n[caps]\nfs = [\"__T__/data/**\"]\n",
        &[
            ("main.kp", "impl \"util\" caps(fs = [\"__T__/**\"])\n\nfun main() {\n    util.go()\n}\n"),
            ("util.kp", "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n"),
        ],
    );
    lab.refused(&["cannot grant"]);
}

/// D4: module ids cannot escape `src/` or shadow the library: `..`, absolute
/// and backslash paths are refused, and a project file named like a shipped
/// module (`src/std/rt.kp`, whose `glob_match` says yes to everything) is
/// never read in place of the shipped one, so the matcher cannot be replaced.
#[test]
pub(crate) fn d04_module_ids_cannot_escape_or_shadow_the_library() {
    for id in ["../outside", "/etc/passwd", "a\\\\b", "std/../std/fs", "util/../util"] {
        let lab = Lab::new("d04a");
        lab.src("main.kp", &format!("impl \"{id}\"\n\nfun main() {{\n}}\n"));
        let out = lab.build();
        assert!(!out.status.success(), "the module id '{id}' was accepted");
    }
    let lab = Lab::new("d04b");
    lab.manifest_project(
        "name = \"d\"\n",
        &[
            (
                "main.kp",
                "impl \"util\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    util.go()\n}\n",
            ),
            (
                "util.kp",
                "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
            ),
        ],
    );
    fs::create_dir_all(lab.root.join("src/std")).unwrap();
    let real = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/std/rt.kp")).unwrap();
    let evil = real.replacen(
        "fun glob_match(pat: &str, path: &str) => bool {",
        "fun glob_match(pat: &str, path: &str) => bool {\n    return true",
        1,
    );
    fs::write(lab.root.join("src/std/rt.kp"), evil).unwrap();
    lab.denied("granted to module 'util'");
}

/// D5: a user extern section is the manifest author's decision, but within it
/// the allowlist holds: a symbol that is not listed is refused, and a module
/// the manifest does not name cannot bind the symbol of one it does.
#[test]
pub(crate) fn d05_the_extern_allowlist_is_per_module_and_per_symbol() {
    let manifest = "name = \"x\"\n\n[extern.\"sys\"]\ndomain = \"process\"\nlibrary = \"c\"\nsymbols = [\"getpid\"]\n";
    let lab = Lab::new("d05a");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"sys\" caps(process = [\"/**\"])\n\nfun main() {\n    print(sys.getppid() > 0)\n}\n"),
            ("sys.kp", "pub extern fun getppid() => i32\n"),
        ],
    );
    lab.refused(&["getppid"]);
    let lab = Lab::new("d05b");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    print(evil.getpid() > 0)\n}\n"),
            ("evil.kp", "pub extern fun getpid() => i32\n"),
        ],
    );
    lab.refused(&["has no compiler-supplied body"]);
    // And the manifest's domain gate applies to the module that does bind it.
    let lab = Lab::new("d05c");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"util\"\n\nfun main() {\n    print(util.pid() > 0)\n}\n"),
            ("util.kp", "impl \"sys\"\n\npub fun pid() => i32 = sys.getpid()\n"),
            ("sys.kp", "pub extern fun getpid() => i32\n"),
        ],
    );
    lab.refused(&["needs the 'process' capability"]);
}

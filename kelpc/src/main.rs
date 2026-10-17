mod ast;
mod commands;
mod backend;
mod borrowck;
mod caps_report;
mod cache;
mod codegen_llvm;
mod deprecated;
mod derive;
mod defer;
mod diag;
mod doc;
mod driver;
mod extern_agg;
mod extern_object;
mod escape;
mod externs;
mod fmt;
mod foreign;
mod freestanding;
mod gc_descr;
mod guard;
mod infer;
mod inline_asm;
mod lexer;
mod lint;
mod managed;
mod llvm_ffi;
mod opt;
mod parser;
mod preprocess;
mod repl;
mod resolve;
mod sha256;
mod stdlib;
mod sysroot;
mod timings;
mod tir;
mod types;

use commands::*;
use diag::{Diagnostic, Severity, SourceMap};
use fmt::RefusalKind;
use opt::OptLevel;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

/// Name this compiler is installed and invoked as. Kept in sync with the
/// crate's `[package] name` and `[[bin]] name` in `Cargo.toml`.
const BIN_NAME: &str = "kelpc";

/// The version this build was made from, taken from `[package] version` at
/// compile time so it cannot drift from `Cargo.toml`.
///
/// A Cargo version cannot carry a leading zero in a component (Cargo refuses
/// `26.09.0` with `invalid leading zero in minor version number`), and the
/// release name can: releases are numbered `YY.MM.N`, so `Cargo.toml` writes
/// the semver-legal `26.9.0` and every human-facing spelling goes through
/// [`release_name`]. The tag (`v26.09.0`), the release asset
/// (`kelpc-26.09.0-linux-x86_64`) and the string `kelpc version` prints are
/// therefore one text, which is what lets Coral install an asset by tag.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `26.9.0` to `26.09.0`: the padded release name for a crate version.
///
/// Only the minor is padded, because only the month carries a zero in the
/// scheme, and a version whose first component is not two digits (the old
/// numbering, or anything else odd) is returned unchanged rather than mangled.
fn release_name(crate_version: &str) -> String {
    let parts: Vec<&str> = crate_version.split('.').collect();
    let numeric = parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if parts.len() == 3 && numeric && parts[0].len() == 2 {
        return format!("{}.{:0>2}.{}", parts[0], parts[1], parts[2]);
    }
    crate_version.to_string()
}

/// The directory a build with no `-o` writes into, relative to the working
/// directory. `kelpc build <file>.kp` used to write `<stem>` into the working
/// directory itself, which meant a build run from a checkout littered the
/// checkout: ten such binaries were committed to this repository's root across
/// six commits before anyone noticed (`git log --diff-filter=A -- bitwise`),
/// and the generated C beside each one was hidden by the `*.c` ignore rule. A
/// build directory the tool owns is the fix; `.gitignore` ignores it too, so
/// even the directory never has to be remembered.
const BUILD_DIR: &str = "build";

fn usage() {
    eprintln!(
        "usage: {BIN_NAME} build <file.{EXT}> [-o <out>] [-O0|-O1|-O2|-O3|-Os] [--deny-outdated] [--incremental] [--timings]\n       \
         {BIN_NAME} run <file.{EXT}> [-O0|-O1|-O2|-O3|-Os] [--deny-outdated] [--incremental] [--timings]\n       \
         {BIN_NAME} check <file.{EXT}> [--json] [--deny-outdated] [--text]\n       \
         {BIN_NAME} caps <file.{EXT}|project dir>\n       \
         {BIN_NAME} repl [--script <file>]\n       \
         {BIN_NAME} fmt <file.{EXT}>\n       \
         {BIN_NAME} fmt -w <path>...\n       \
         {BIN_NAME} fmt --check <path>...\n       \
         {BIN_NAME} lint <path>...\n       \
         {BIN_NAME} doc <file.{EXT}|project dir>... [-o <dir>] [--format md|html] [--private] [--check]\n       \
         {BIN_NAME} doc <file.{EXT}|project dir>... --format json\n       \
         {BIN_NAME} doc --text <file.{EXT}> --format json < buffer\n       \
         {BIN_NAME} doc --std [-o <dir>] [--format md|html] [--private] [--check]\n       \
         {BIN_NAME} stdlib --json --dir <dir>\n       \
         {BIN_NAME} cache stats|clear [--dir <dir>]\n       \
         {BIN_NAME} version [--verbose]\n\n\
         The optimization level is -O0, -O1, -O2, -O3 or -Os, and -O2 is the\n\
         default. One level is one level for the whole build: it selects the\n\
         LLVM pass pipeline and the target machine's codegen level, and it is\n\
         the -O flag the per-program C object and the final link are given.\n\
         -Oz is not offered: it is a clang-only spelling and the linking cc may\n\
         be gcc, so the vocabulary has to be one both accept; -Os is for size.\n\n\
         A spelling in the register of outdated syntax still compiles, and\n\
         build and run print it as an 'outdated:' warning naming what to write\n\
         instead and saying it is removed in the next development cycle.\n\
         --deny-outdated turns those findings into errors, which is the\n\
         migration gate for a project that wants to know when its own code is\n\
         on the list.\n\n\
         --incremental (or '[build] incremental = true' in coral.toml) splits\n\
         the backend's output into one compilation unit per source module and\n\
         links the unit objects together: separate compilation, with no\n\
         cross-unit inlining. It is off by default and produces the same\n\
         program either way. A freestanding project is refused by name for\n\
         now (plan/incremental-compilation.md).\n\n\
         Cache misses are compiled on worker threads, each unit in its own\n\
         LLVM context. KELPC_JOBS names the worker count (a positive integer;\n\
         the default is 2, not the CPU count); more workers than units spawns\n\
         one per unit.\n\n\
         --sanitize=... is refused: the LLVM backend links no sanitizer\n\
         passes (plan/sanitizers.md).\n\n\
         --timings prints per-phase wall times (parse, resolve, infer,\n\
         borrowck, codegen, unit compile, link) and, for an --incremental\n\
         build, one line per unit with hit or miss, to stderr.\n\n\
         An --incremental build keys each unit's object by the hash of its IR\n\
         and the machine it is compiled for, and reuses the object on a later\n\
         build when the IR is unchanged. The cache lives in\n\
         <out dir>/.kelpc-cache unless KELPC_CACHE_DIR names another\n\
         directory. KELPC_CACHE=off turns lookups and stores off for one build;\n\
         KELPC_CACHE_STATS=1 prints 'kelpc cache: N hits, N misses, N stored';\n\
         KELPC_CACHE_MAX_MB caps the cache size (default 1024).\n\
         'kelpc cache stats' and 'kelpc cache clear' inspect and empty it.\n\n\
         With no -o, the binary and the backend's intermediate are written to\n\
         {BUILD_DIR}/<file stem>, which the compiler creates if it is missing.\n\n\
         check runs everything a build runs except code generation: the same\n\
         modules, the same type check and the same borrow check, so it answers\n\
         whether the program compiles without writing an artifact. --json\n\
         prints the findings as one JSON object on stdout instead of prose on\n\
         stderr: {{\"schema\":1,\"ok\":...,\"diagnostics\":[...]}}, each finding\n\
         with its severity (error, warning or outdated), its message, and a\n\
         1-based [line:column, end_line:end_column) range inside its file. The\n\
         exit status is nonzero only for an error: a warning or an outdated\n\
         finding on its own exits zero, the rule lint already follows.\n\n\
         fmt prints formatted source to stdout; -w rewrites in place, and\n\
         --check exits nonzero without writing when a file is not already\n\
         formatted. A path may be a .{EXT} file or a directory (recursing\n\
         .{EXT} files). A symbolic link given by name is followed to the file\n\
         it names, so -w rewrites that file and the link survives; a link\n\
         found inside a directory is skipped and reported. A path that is not\n\
         a regular file (a FIFO, a socket, a device) is refused before it is\n\
         read, so fmt never waits on a node that has no data to give and\n\
         never replaces one. Formatting changes whitespace and indentation\n\
         only: it never rewrites a token, so a formatted file is the same\n\
         program, and a rewritten file is a new inode - a hard link to it\n\
         keeps the old bytes. A file that does not parse - by fmt's own\n\
         reading of it, or by the compiler's when the project's aliases are\n\
         needed - is reported as a parse error and left alone.\n\n\
         lint prints warnings for the sources under each path (.{EXT} files,\n\
         recursing a directory, with a symbolic link inside a directory\n\
         skipped and reported). A warning is not a failure: lint exits zero\n\
         with warnings on stderr, and nonzero only for a path it could not\n\
         read or a file it could not lex. It never writes to a file. It also\n\
         warns about a '///' or '//!' comment attached to nothing.\n\n\
         doc writes one page per module plus an index into -o <dir> (default\n\
         'doc'), Markdown unless --format html (one self-contained page, no\n\
         external assets). A '///' block directly above a fun, datac, enum,\n\
         trait, pd, const, static, extern type, enum variant, field or trait\n\
         method documents it, and a '//!' block at the top of a file\n\
         documents the module. Only pub items are listed (--private lists\n\
         all); a shipped std module has no pub, so --std lists all of its\n\
         functions but not its extern ones. The argument is a .{EXT} file or\n\
         a project directory (every module under src/); --std documents the\n\
         compiler's own std modules. --check writes nothing and exits nonzero\n\
         when a listed item has no doc comment. The output has no\n\
         timestamps, so it can be checked in and diffed.",
        EXT = resolve::EXT
    );
}

/// The front end of a build: modules loaded, types checked, borrows checked,
/// with the findings *collected* rather than printed.
///
/// Codegen is the whole difference between this and `build`, which is why
/// `kelpc check` can answer whether a program compiles without writing an
/// artifact. The front end is fail-fast: the first error is the last finding
/// produced, so a program with four independent mistakes reports one.
///
/// `ok` is false when any finding is an error, which is the same question the
/// exit status answers. Outdated syntax is a finding rather than a failure
/// unless `deny_outdated` asks otherwise, matching `build`.
///
/// `entry_text` is `--text`'s buffer (MISSING-b6Ytr): when given, it replaces
/// what is on disk at `path` for the one module `path` names, so a caller can
/// ask about an unsaved edit.
fn front_end(
    path: &str,
    deny_outdated: bool,
    sources: &mut SourceMap,
    entry_text: Option<String>,
) -> (Vec<(Severity, Diagnostic)>, bool) {
    let mut findings: Vec<(Severity, Diagnostic)> = Vec::new();

    let loaded = match resolve::load(Path::new(path), sources, entry_text, None) {
        Ok(l) => l,
        Err(d) => {
            findings.push((Severity::Error, d));
            return (findings, false);
        }
    };
    for d in loaded.deprecations {
        let severity = if deny_outdated {
            Severity::Error
        } else {
            Severity::Outdated
        };
        findings.push((severity, d));
    }
    if !findings.is_empty() {
        return (findings, !deny_outdated);
    }

    let _ = infer::take_cap_uses();
    let typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            findings.push((Severity::Error, d));
            return (findings, false);
        }
    };

    // The `[dependencies] caps` check (`caps_report.rs`): a dependency that
    // reaches a domain its manifest entry does not list fails the build here,
    // so `check`, `build` and `run` all refuse it.
    let uses = infer::take_cap_uses();
    if let Err(d) = caps_report::check_dependencies(&loaded.project, &uses, sources) {
        findings.push((Severity::Error, d));
        return (findings, false);
    }

    if let Err(d) = borrowck::check_program(&typed) {
        findings.push((Severity::Error, d));
        return (findings, false);
    }

    // The freestanding allowed set (kernel stage 4) runs in the front end, so
    // `kelpc check` enforces exactly what `kelpc build` does. It is a no-op for
    // a hosted project, which is every project written before this stage.
    if let Err(d) = freestanding::check(&typed, &loaded.project) {
        findings.push((Severity::Error, d));
        return (findings, false);
    }

    (findings, true)
}

/// `kelpc check <path> [--json] [--deny-outdated] [--text]`: whether the
/// program compiles, without an artifact.
///
/// `--json` is the machine-readable contract Coral and `lighthouse` read
/// (MK13-S1-M3): one JSON object on stdout, the prose form on stderr
/// otherwise. A caller that reads the JSON never has to parse a message to
/// find out which file and which range it is about, which is the whole reason
/// the form exists.
///
/// `--text` reads the entry file's content from stdin instead of from disk
/// (MISSING-b6Ytr): `<path>` still names the real file, so module resolution
/// and every finding's `file`/`module` are the ones a caller expects, but the
/// text checked is the buffer, not the last save. It needs a file, not a
/// project directory: a buffer is always one open file.
fn check_command(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut deny_outdated = false;
    let mut text = false;
    let mut path: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--deny-outdated" => deny_outdated = true,
            "--text" => text = true,
            other if other.starts_with('-') && other != "-" => {
                eprintln!(
                    "error: unknown flag '{other}'; expected one of: --json, --deny-outdated, --text"
                );
                return ExitCode::FAILURE;
            }
            other if path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!("error: unexpected argument '{other}'");
                usage();
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("error: 'check' needs a .{EXT} file or a project directory", EXT = resolve::EXT);
        usage();
        return ExitCode::FAILURE;
    };

    let entry_text = if text {
        if Path::new(&path).is_dir() {
            eprintln!("error: '--text' needs a .{EXT} file, not a project directory: a buffer is always one open file", EXT = resolve::EXT);
            return ExitCode::FAILURE;
        }
        let mut buf = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut buf) {
            eprintln!("error: cannot read '--text' from stdin: {e}");
            return ExitCode::FAILURE;
        }
        Some(buf)
    } else {
        None
    };

    let mut sources = SourceMap::new();
    let (findings, ok) = front_end(&path, deny_outdated, &mut sources, entry_text);
    if json {
        let borrowed: Vec<(Severity, &Diagnostic)> = findings
            .iter()
            .map(|(severity, d)| (*severity, d))
            .collect();
        print!("{}", sources.render_findings_json(&borrowed, ok));
    } else {
        for (severity, d) in &findings {
            sources.report_as(d, *severity);
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `kelpc caps <project dir or entry file>`: the capability report. One line per
/// project module (what it holds, what it reaches, what it grants to its
/// imports), then one line per `[dependencies]` entry. Sorted, so a CI job can
/// diff it. Nothing is written to stdout when the program does not check, and
/// the refusal is the one `kelpc check` would print.
fn caps_command(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            other if other.starts_with('-') && other != "-" => {
                eprintln!("error: unknown flag '{other}'; 'caps' takes no flags");
                return ExitCode::FAILURE;
            }
            other if path.is_none() => path = Some(other),
            other => {
                eprintln!("error: unexpected argument '{other}'");
                usage();
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("error: 'caps' needs a .{EXT} file or a project directory", EXT = resolve::EXT);
        usage();
        return ExitCode::FAILURE;
    };

    let mut sources = SourceMap::new();
    let loaded = match resolve::load(Path::new(path), &mut sources, None, None) {
        Ok(l) => l,
        Err(d) => {
            sources.report(&d);
            return ExitCode::FAILURE;
        }
    };
    let _ = infer::take_cap_uses();
    if let Err(d) = infer::check_program(&loaded.program, &loaded.project) {
        sources.report(&d);
        return ExitCode::FAILURE;
    }
    let uses = infer::take_cap_uses();
    if let Err(d) = caps_report::check_dependencies(&loaded.project, &uses, &sources) {
        sources.report(&d);
        return ExitCode::FAILURE;
    }
    print!("{}", caps_report::render(&loaded.project, &uses));
    ExitCode::SUCCESS
}

/// `kelpc cache stats|clear [--dir <dir>]`: inspect or empty the object cache
/// the per-unit split keys by IR hash (roadmap A-14 slice 2).
///
/// The directory is `--dir` when given, else `KELPC_CACHE_DIR`, else
/// `build/.kelpc-cache`, which is where a build with the default output path
/// keeps it.
fn cache_command(args: &[String]) -> ExitCode {
    let mut action: Option<String> = None;
    let mut dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--dir" {
            let value = match args.get(i + 1) {
                Some(v) => v.clone(),
                None => {
                    eprintln!("error: '--dir' requires a value");
                    return ExitCode::FAILURE;
                }
            };
            dir = Some(PathBuf::from(value));
            i += 2;
        } else if let Some(value) = arg.strip_prefix("--dir=") {
            dir = Some(PathBuf::from(value));
            i += 1;
        } else if action.is_none() && !arg.starts_with('-') {
            action = Some(arg.to_string());
            i += 1;
        } else {
            eprintln!(
                "error: unknown argument '{arg}'; usage: {BIN_NAME} cache stats|clear [--dir <dir>]"
            );
            return ExitCode::FAILURE;
        }
    }
    let Some(action) = action else {
        eprintln!("usage: {BIN_NAME} cache stats|clear [--dir <dir>]");
        return ExitCode::FAILURE;
    };
    let dir = dir.unwrap_or_else(default_cache_dir);
    let cache = cache::Cache::with_dir(dir);
    match action.as_str() {
        "stats" => {
            let (count, size) = cache.entry_count_and_size();
            println!(
                "kelpc cache: {count} entries, {size} bytes in {}",
                cache.dir().display()
            );
            ExitCode::SUCCESS
        }
        "clear" => match cache.clear() {
            Ok(removed) => {
                println!(
                    "kelpc cache: removed {removed} files from {}",
                    cache.dir().display()
                );
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        },
        other => {
            eprintln!("error: unknown cache action '{other}'; expected 'stats' or 'clear'");
            ExitCode::FAILURE
        }
    }
}

/// The cache directory a bare `kelpc cache` command uses, matching the build's
/// own default (`KELPC_CACHE_DIR`, else `build/.kelpc-cache`).
fn default_cache_dir() -> PathBuf {
    match std::env::var_os(cache::DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(BUILD_DIR).join(cache::CACHE_DIR_NAME),
    }
}

/// Runs the full pipeline (load modules, typecheck, borrowcheck, codegen,
/// finish the artifact) and produces a native binary at `out_path`. Returns
/// `false` (having already printed a diagnostic) on any failure.
///
/// `path` may be a `.kp` file or a project directory containing a manifest
/// (in which case the manifest's entry point is used).
///
/// `deny_outdated` is `--deny-outdated`: a load that uses outdated syntax
/// reports those findings as errors and stops, before anything is generated.
///
/// `opt` is the `-O` level (MK7 S6, the record in `kelp wiki page meta/milestones.md`). It is
/// passed to the LLVM backend and to the `cc` calls it makes.
///
/// The LLVM backend is the only code generator, and it is called from the one
/// place below.
fn build(
    path: &str,
    out_path: &Path,
    deny_outdated: bool,
    opt: OptLevel,
    incremental_flag: bool,
    freestanding_out: &mut bool,
    emit_out: &mut resolve::Emit,
    print_timings: bool,
) -> bool {
    let mut sources = SourceMap::new();
    let mut timing = timings::Timings::new();

    let loaded = match resolve::load(Path::new(path), &mut sources, None, Some(&mut timing)) {
        Ok(l) => l,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };
    if !report_deprecations(&sources, &loaded.deprecations, deny_outdated) {
        return false;
    }

    // The allowed set is checked on the whole program, before the prune below
    // removes the `std` declarations it names: a call to `str.concat` has to be
    // refused by name, and that needs the declaration it names to still be
    // there. The check is a no-op for a hosted project.
    let infer_start = Instant::now();
    let _ = infer::take_cap_uses();
    let mut typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };
    timing.infer += infer_start.elapsed();
    // The `[dependencies] caps` check (`caps_report.rs`), on the uses this first
    // check recorded, before any later pass can add to them.
    let uses = infer::take_cap_uses();
    if let Err(d) = caps_report::check_dependencies(&loaded.project, &uses, &sources) {
        sources.report(&d);
        return false;
    }
    if let Err(d) = freestanding::check(&typed, &loaded.project) {
        sources.report(&d);
        return false;
    }
    // The allowed set's emission half: a freestanding build drops the `std/str`
    // and `std/mem` declarations no call can reach and infers the program again,
    // so their `strlen`/`malloc` bodies and the type instances they would
    // otherwise instantiate never exist.
    if loaded.project.target().is_freestanding() {
        let mut pruned = loaded.program.clone();
        let allocator = loaded.project.target().alloc().is_some()
            && loaded.project.target().dealloc().is_some();
        freestanding::prune(&mut pruned, allocator);
        let prune_infer_start = Instant::now();
        typed = match infer::check_program(&pruned, &loaded.project) {
            Ok(p) => p,
            Err(d) => {
                sources.report(&d);
                return false;
            }
        };
        timing.infer += prune_infer_start.elapsed();
    }

    let borrow_start = Instant::now();
    let drops = match borrowck::check_program(&typed) {
        Ok(plan) => plan,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };
    timing.borrowck = borrow_start.elapsed();

    *freestanding_out = loaded.project.target().is_freestanding();
    *emit_out = loaded.project.emit();

    // The LLVM backend links no sanitizer passes. The manifest key still
    // parses, so a project that names one gets the refusal here rather than a
    // silent build without it. The
    // `--sanitize=` flag is refused where it is read.
    if let Some(kind) = loaded.project.sanitize() {
        eprintln!(
            "error: '{}' sets '[build] sanitize = \"{}\"', and sanitizers are not available: the LLVM backend links no sanitizer passes",
            path,
            kind.name()
        );
        return false;
    }

    // Obfuscation step 1 (`[build] obfuscate = "names"`) is hosted only: a
    // freestanding image has its own linker script and entry stub, so it is
    // refused by name until that is designed.
    if loaded.project.obfuscate() == resolve::Obfuscate::Names && loaded.project.target().is_freestanding() {
        eprintln!(
            "error: '{}' sets '[build] obfuscate = \"names\"', which is not available for a freestanding project yet: its image has its own linker script and entry stub",
            path
        );
        return false;
    }

    // Roadmap A-14 slice 1: the backend's per-unit split. `--incremental` and
    // the manifest's `[build] incremental = true` both turn it on. A freestanding project is refused by name for now: its image
    // is one ELF with one entry stub and one panic state, which is a slice of
    // its own (`kelp wiki page compiler/incremental.md`).
    // Every hosted build has the shared pool (Tidepool): the collector is linked
    // in from its archive, which is looked up here, before any code is
    // generated, so a missing archive is named before a link fails. It is
    // whole-program too, since `kelp_gc_init` and the descriptors are one module's.
    let hosted = loaded.project.target().has_shared_pool();
    if hosted && incremental_flag {
        eprintln!(
            "error: '{}' is a hosted build, which has the shared pool, and '--incremental' cannot build that yet: the collector's descriptors and init are whole-program",
            path
        );
        return false;
    }
    // A hosted project whose manifest sets `[build] incremental = true` falls
    // back to the whole-program build, with a warning that names the reason.
    if hosted && loaded.project.incremental() {
        eprintln!(
            "warning: '{}' sets '[build] incremental = true', but a hosted build is whole-program for now: the collector's descriptor ids are per unit, so a per-unit split cannot share them. Building the whole program.",
            path
        );
    }
    let managed = if hosted {
        match driver::managed_archive() {
            Ok(gc_archive) => Some(codegen_llvm::Managed {
                heap: codegen_llvm::HeapLimits::from_target(loaded.project.target()),
                gc_archive,
            }),
            Err(e) => {
                eprintln!("error: {}", e.message);
                return false;
            }
        }
    } else {
        None
    };
    let incremental = incremental_flag || (loaded.project.incremental() && !hosted);
    if incremental && *freestanding_out {
        eprintln!(
            "error: '--incremental' is not implemented for a freestanding project yet: its image is one ELF with a single entry stub and one panic state, so the per-unit split is its own slice (plan/incremental-compilation.md)"
        );
        return false;
    }
    if incremental {
        let cache = cache::Cache::for_build(out_path);
        if let Err(message) = cache.store_interfaces(&loaded.interfaces) {
            eprintln!("error: {message}");
            return false;
        }
        timing.interfaces = Some((cache.stats().interface_hits(), cache.stats().interface_misses()));
    }

    // One `pkg-config` invocation for every package the manifest's `[extern.*]`
    // sections named (MK9-S1-M1). A package that cannot be resolved is a build
    // error, and the flags reach the LLVM backend's `cc` calls.
    let pkg_config_flags =
        match driver::pkg_config_flags(&loaded.project.extern_pkg_config_packages()) {
            Ok(flags) => flags,
            Err(e) => {
                eprintln!("error: {}", e.message);
                return false;
            }
        };

    // The LLVM backend is the only code generator. It builds its own module,
    // writes the textual IR beside the binary and links the object itself.
    let freestanding = if loaded.project.target().is_freestanding() {
        let target = loaded.project.target();
        let root = loaded.project.root.clone().unwrap_or_default();
        let script = root.join(target.linker_script().unwrap_or_default());
        Some(codegen_llvm::Freestanding {
            triple: target.triple().unwrap_or_default().to_string(),
            entry: target.entry().unwrap_or_default().to_string(),
            panic: target.panic().map(str::to_string),
            linker_script: script.to_string_lossy().into_owned(),
            alloc: target.alloc().map(str::to_string),
            dealloc: target.dealloc().map(str::to_string),
            realloc: target.realloc().map(str::to_string),
            cpus: target.cpus(),
        })
    } else {
        None
    };
    let link_libs = loaded.project.extern_libraries();
    let library_dirs = loaded.project.extern_library_dirs();
    if let Err(message) = codegen_llvm::compile(
        &typed,
        &drops,
        &sources,
        out_path,
        opt,
        &link_libs,
        &library_dirs,
        &pkg_config_flags,
        freestanding.as_ref(),
        managed.as_ref(),
        incremental,
        loaded.project.obfuscate() == resolve::Obfuscate::Names,
        &mut timing,
    ) {
        eprintln!("error: {message}");
        return false;
    }


    if print_timings {
        timing.print();
    }
    true
}

/// `[build] emit = "html"`: runs the built program once, with no extra environment,
/// and writes what it prints as `index.html` in the output directory, which is the
/// directory the binary was written to. The program is `[build] emit_entry`, which
/// `resolve::load` compiled in place of the entry, so its first view is chosen by the
/// build and needs no capability at run time. A program that prints no HTML document is
/// refused rather than written out, so the file is never a stray stdout.
fn emit_html(out_path: &Path) -> ExitCode {
    let abs_out = if out_path.is_absolute() {
        out_path.to_path_buf()
    } else {
        PathBuf::from("./").join(out_path)
    };
    guard::stop();
    let output = match Command::new(&abs_out).stdin(Stdio::null()).output() {
        Ok(output) => output,
        Err(e) => {
            eprintln!("error: could not run '{}' to emit its HTML: {e}", abs_out.display());
            return ExitCode::FAILURE;
        }
    };
    if !output.status.success() {
        eprintln!(
            "error: '{}' failed while emitting its HTML ({}):\n{}",
            abs_out.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return ExitCode::FAILURE;
    }
    let html = match String::from_utf8(output.stdout) {
        Ok(text) => text,
        Err(_) => {
            eprintln!("error: '{}' printed bytes that are not UTF-8 while emitting its HTML", abs_out.display());
            return ExitCode::FAILURE;
        }
    };
    if !html.starts_with("<!doctype html>") {
        eprintln!(
            "error: '{}' printed no HTML document for '[build] emit = \"html\"': the [build] emit_entry must call polyp.emit_html (kelpc/tests/fixtures/examples/polyp/backends/gtk/polyp/lib.kp)",
            abs_out.display()
        );
        return ExitCode::FAILURE;
    }
    let dir = match out_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let index = dir.join("index.html");
    if let Err(e) = write_atomic(&index, &html) {
        eprintln!("error: could not write '{}': {e}", index.display());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Prints the deprecations one load found, and says whether the build may
/// continue.
///
/// Without `--deny-outdated` they are warnings labelled `outdated: `, the exit
/// status is untouched, and the artifact is built: a spelling on the register
/// still works, which is the point of the register. With the flag they are
/// errors, and the caller stops before codegen, so a project can put the flag
/// in CI and find out when its own code is on the list rather than at removal
/// time. A load that found nothing is not denied by the flag either: there is
/// nothing on the list to stop for, which is what makes the flag usable over a
/// tree that is already migrated.
fn report_deprecations(sources: &SourceMap, found: &[Diagnostic], deny: bool) -> bool {
    for d in found {
        if deny {
            sources.report(d);
        } else {
            sources.report_outdated(d);
        }
    }
    found.is_empty() || !deny
}

/// Where a build with no `-o` writes: `build/<stem>`.
///
/// Relative to the working directory rather than to the source, because the
/// artifact is a program to run rather than a file to keep, and the working
/// directory is where the person who asked for it is standing. The stem keeps
/// the old naming (`examples/hello.kp` is `build/hello`), so nothing about how
/// a build is *named* changes - only where it lands.
fn default_out_path(source_path: &str) -> PathBuf {
    let stem = Path::new(source_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "a.out".to_string());
    Path::new(BUILD_DIR).join(stem)
}


/// `kelpc lint <path>...`: warns about the sources under each path, and never
/// fails a build for a warning.
///
/// The contract, which is the whole difference from `fmt` and `build`:
///
/// - **A warning is not a failure.** Warnings go to stderr and the exit stays
///   zero, so a lint run in a script tells "clean" from "has findings" by the
///   output and "could not check" by the exit code.
/// - **Nothing is written.** No file is rewritten, so there is no `-w` and no
///   `--check`: the command says what it found and leaves the tree alone.
/// - **Only a read failure or a lex failure is an error**, and those are
///   reported exactly as `build` reports them for the same file.
///
/// Paths are collected the way `fmt` collects them (a `.kp` file or a
/// directory, recursing, a link inside a directory skipped and reported), so a
/// script that lints a tree names the tree once.
fn lint_command(args: &[String]) -> ExitCode {
    let mut paths: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("error: unknown flag '{other}'; 'lint' takes paths only");
                return ExitCode::FAILURE;
            }
            other => paths.push(other.to_string()),
        }
    }
    if paths.is_empty() {
        eprintln!("error: 'lint' needs at least one path (a '.{}' file or a directory)", resolve::EXT);
        return ExitCode::FAILURE;
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for path in &paths {
        match collect_kp(Path::new(path), "lint") {
            Ok((mut found, mut links)) => {
                files.append(&mut found);
                skipped.append(&mut links);
            }
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::FAILURE;
            }
        }
    }
    for link in &skipped {
        eprintln!(
            "note: skipping '{link}': a symbolic link found inside a directory is not \
             linted, because the file behind it is already in the tree or not in it"
        );
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        eprintln!("error: no '.{}' files found", resolve::EXT);
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for file in &files {
        failed |= !lint_one(file);
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Lints one file. `false` means a diagnostic was printed, which only a
/// failure to read the file or to lex it produces: a warning is a success.
fn lint_one(path: &Path) -> bool {
    let src = match read_source(path) {
        Ok(src) => src,
        Err(()) => return false,
    };
    let naming = naming_for(path);
    let mut sources = SourceMap::new();
    let module = path.to_string_lossy().to_string();
    let found = lint::findings(&src, naming.as_ref());
    // The file is registered either way: a lex error carries a span into it,
    // and that span has to print as `file:line:col` like every other one.
    sources.insert(&module, path, src);
    match found {
        Ok(findings) => {
            for finding in findings {
                sources.report_warning(&Diagnostic::new(
                    finding.message,
                    finding.span,
                    module.clone(),
                ));
            }
            true
        }
        Err(e) => {
            sources.report(&Diagnostic::new(e.message, e.span, module));
            false
        }
    }
}

/// The naming rule's setting for one file, or `None` when the rule does not
/// apply to it.
///
/// `None` is a fetched dependency under the project's `src/deps/`: another
/// repository's names are not this project's convention to enforce, and a
/// warning about them would be one nobody could fix from their own tree.
///
/// A file with no manifest above it gets the option's default, because the
/// question the rule asks ("what case does this project write?") has an answer
/// everywhere, and the answer with no manifest is the option's own default.
/// A manifest that is missing or unreadable is the same case, deliberately: a
/// broken `coral.toml` is `kelpc build`'s diagnostic, and `lint` is not the
/// command that reports build failures.
fn naming_for(path: &Path) -> Option<lint::Naming> {
    let Some(root) = resolve::find_manifest_root(path) else {
        return Some(lint::Naming::default_without_manifest());
    };
    if path.starts_with(root.join("src").join("deps")) {
        return None;
    }
    match resolve::manifest_naming_case(&root) {
        Some(option) => lint::NamingCase::from_option(option).map(|case| lint::Naming {
            case,
            from_manifest: true,
        }),
        None => Some(lint::Naming::default_without_manifest()),
    }
}

/// Collects the `.kp` files under `dir`, never following a symbolic link.
///
/// A link may name a file outside the tree the caller named, or a directory that
/// holds this one - `ln -s . here` is enough - so following one could format a
/// file twice under two names, rewrite a file outside the tree, or recurse
/// without end. The link is recorded instead, and the caller reports it.
fn collect_kp_into(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    skipped: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            skipped.push(path.display().to_string());
        } else if file_type.is_dir() {
            collect_kp_into(&path, out, skipped)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some(resolve::EXT) {
            out.push(path);
        }
    }
    Ok(())
}

/// `kelpc stdlib --json --dir <dir>`: writes every shipped std module's source
/// to `<dir>/<build-id>/<module path>.kp` and prints the listing as JSON: the
/// build id, the compiler version, and per module its id, file path, SHA-256 of
/// the source and its synthetic declarations (`stdlib::SYNTHETIC_ITEMS`).
///
/// A file that already holds the right bytes is left alone, so a second run is
/// a read. The build id changes whenever the table does, so a directory written
/// by another compiler is never read for this one.
fn stdlib_command(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--dir" => {
                i += 1;
                match args.get(i) {
                    Some(d) => dir = Some(PathBuf::from(d)),
                    None => {
                        eprintln!("error: '--dir' needs a directory");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other => {
                eprintln!("error: unexpected argument '{other}'; expected --json --dir <dir>");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }
    if !json {
        eprintln!("error: 'stdlib' writes its listing as JSON; pass '--json --dir <dir>'");
        return ExitCode::FAILURE;
    }
    let Some(dir) = dir else {
        eprintln!("error: 'stdlib --json' needs '--dir <dir>' to write the sources to");
        return ExitCode::FAILURE;
    };
    let dir = if dir.is_absolute() {
        dir
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(dir),
            Err(e) => {
                eprintln!("error: cannot read the working directory: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    let build = stdlib::build_id();
    let root = dir.join(&build);
    let mut rows = Vec::new();
    for id in stdlib::shipped() {
        let Some(text) = stdlib::source(id) else {
            eprintln!("error: the compiler ships '{id}' with no source");
            return ExitCode::FAILURE;
        };
        let sha = sha256::sha256_hex(text.as_bytes());
        let target = root.join(format!("{id}.{}", resolve::EXT));
        let current = fs::read(&target).ok().map(|bytes| sha256::sha256_hex(&bytes));
        if current.as_deref() != Some(sha.as_str()) {
            if let Some(parent) = target.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("error: cannot create '{}': {e}", parent.display());
                    return ExitCode::FAILURE;
                }
            }
            if let Err(e) = fs::write(&target, &text) {
                eprintln!("error: cannot write '{}': {e}", target.display());
                return ExitCode::FAILURE;
            }
        }
        let syms: Vec<String> = stdlib::synthetic_items(id)
            .iter()
            .map(|item| {
                format!(
                    "{{\"name\":{},\"kind\":{},\"sig\":{}}}",
                    diag::json_string(item.name),
                    diag::json_string(item.kind),
                    diag::json_string(item.sig),
                )
            })
            .collect();
        rows.push(format!(
            "{{\"id\":{},\"path\":{},\"sha256\":{},\"builtin_syms\":[{}]}}",
            diag::json_string(id),
            diag::json_string(&target.to_string_lossy()),
            diag::json_string(&sha),
            syms.join(","),
        ));
    }
    println!(
        "{{\"schema\":1,\"build\":{},\"compiler\":{},\"dir\":{},\"modules\":[{}]}}",
        diag::json_string(&build),
        diag::json_string(&release_name(VERSION)),
        diag::json_string(&root.to_string_lossy()),
        rows.join(","),
    );
    ExitCode::SUCCESS
}

/// `kelpc doc`: the documentation generator (`kelp wiki page tooling/kelpc-doc.md`). The pages
/// are built by `doc.rs`; this reads the sources, writes the files, and runs
/// the `--check` gate.
fn doc_command(args: &[String]) -> ExitCode {
    let mut out_dir = PathBuf::from("doc");
    let mut out_given = false;
    let mut format = doc::Format::Markdown;
    let mut private = false;
    let mut check = false;
    let mut std_mode = false;
    let mut text_mode = false;
    let mut paths: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                out_given = true;
                match args.get(i) {
                    Some(dir) => out_dir = PathBuf::from(dir),
                    None => {
                        eprintln!("error: '-o' needs a directory");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--format" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("md") => format = doc::Format::Markdown,
                    Some("html") => format = doc::Format::Html,
                    Some("json") => format = doc::Format::Json,
                    other => {
                        eprintln!(
                            "error: '--format' takes 'md', 'html' or 'json', found '{}'",
                            other.unwrap_or("nothing")
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--private" => private = true,
            "--check" => check = true,
            "--std" => std_mode = true,
            "--text" => text_mode = true,
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!(
                    "error: unknown flag '{other}'; expected one of: -o, --format, --private, --check, --std, --text"
                );
                return ExitCode::FAILURE;
            }
            other => paths.push(other.to_string()),
        }
        i += 1;
    }
    if !std_mode && paths.is_empty() {
        eprintln!(
            "error: 'doc' needs a '.{}' file, a project directory, or '--std'",
            resolve::EXT
        );
        return ExitCode::FAILURE;
    }
    if format == doc::Format::Json {
        if out_given {
            eprintln!("error: '--format json' writes to stdout; '-o' does not apply to it");
            return ExitCode::FAILURE;
        }
        if check {
            eprintln!("error: '--format json' and '--check' are separate; run one at a time");
            return ExitCode::FAILURE;
        }
    }
    // `--text` documents a buffer read from stdin, as `check --text` checks
    // one. The one path names the buffer, which may not be on disk.
    let buffer: Option<String> = if text_mode {
        if std_mode || paths.len() != 1 || Path::new(&paths[0]).is_dir() {
            eprintln!(
                "error: '--text' needs exactly one '.{}' file name, not a directory or '--std': a buffer is always one open file",
                resolve::EXT
            );
            return ExitCode::FAILURE;
        }
        let mut buf = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut buf) {
            eprintln!("error: cannot read '--text' from stdin: {e}");
            return ExitCode::FAILURE;
        }
        Some(buf)
    } else {
        None
    };

    let mut sources = SourceMap::new();
    let mut modules: Vec<doc::Module> = Vec::new();
    let mut failed = false;
    let mut add = |id: String, shown: String, path: Option<&Path>, text: String, all_public: bool| -> bool {
        let name = path.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| shown.clone());
        match doc::scan(&text) {
            Ok(scan) => {
                modules.push(doc::Module { id, path: shown, scan, all_public });
                true
            }
            Err(e) => {
                sources.insert(&name, path.unwrap_or(Path::new(&name)), text);
                sources.report(&Diagnostic::new(e.message, e.span, name));
                false
            }
        }
    };
    if std_mode {
        for id in stdlib::shipped() {
            if let Some(text) = stdlib::source(id) {
                failed |= !add(id.to_string(), format!("{id}.{}", resolve::EXT), None, text, true);
            }
        }
    }
    for path in &paths {
        let p = Path::new(path);
        // A project directory documents its `src/`; any other directory, itself.
        let (files, base) = if p.is_dir() && p.join("coral.toml").is_file() {
            let src = p.join("src");
            match collect_kp(&src, "doc") {
                Ok((found, _)) => (found, src),
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            }
        } else if p.is_dir() {
            match collect_kp(p, "doc") {
                Ok((found, _)) => (found, p.to_path_buf()),
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            let base = match resolve::find_manifest_root(p) {
                Some(root) => root.join("src"),
                None => p.parent().map(Path::to_path_buf).unwrap_or_default(),
            };
            (vec![p.to_path_buf()], base)
        };
        let mut files = files;
        files.sort();
        files.dedup();
        for file in files {
            let text = match &buffer {
                Some(buf) => buf.clone(),
                None => match read_source(&file) {
                    Ok(text) => text,
                    Err(()) => {
                        failed = true;
                        continue;
                    }
                },
            };
            let rel = file.strip_prefix(&base).unwrap_or(&file).with_extension("");
            let id = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            failed |= !add(id, file.to_string_lossy().to_string(), Some(&file), text, false);
        }
    }
    if failed {
        return ExitCode::FAILURE;
    }
    if modules.is_empty() {
        eprintln!("error: no '.{}' files found", resolve::EXT);
        return ExitCode::FAILURE;
    }
    let opts = doc::Options { format, private };
    if format == doc::Format::Json {
        for (_, text) in doc::render(&modules, &opts) {
            print!("{text}");
        }
        return ExitCode::SUCCESS;
    }
    if check {
        let missing = doc::missing_docs(&modules, &opts);
        for (file, line, message) in &missing {
            eprintln!("{file}:{line}: {message}");
        }
        return if missing.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!("error: cannot create '{}': {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    for (name, text) in doc::render(&modules, &opts) {
        let target = out_dir.join(&name);
        if let Err(e) = fs::write(&target, text) {
            eprintln!("error: cannot write '{}': {e}", target.display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// The compiler's passes recurse over the syntax tree, and the parser's depth
/// limits (`parser::MAX_NESTING`) bound how deep that tree can be, but a debug
/// build's frames are large enough that the 8 MiB main-thread stack is not
/// enough for a tree at the limit. The work runs on a thread with a stack sized
/// for it instead; the memory is address space until it is touched.
const COMPILER_STACK_BYTES: usize = 512 * 1024 * 1024;

fn main() -> ExitCode {
    let worker = std::thread::Builder::new()
        .name("kelpc".to_string())
        .stack_size(COMPILER_STACK_BYTES)
        .spawn(real_main);
    match worker {
        Ok(handle) => handle.join().unwrap_or(ExitCode::FAILURE),
        // No such stack to be had (a tight address-space limit): run on the
        // main thread's own, which is what every build did before.
        Err(_) => real_main(),
    }
}

fn real_main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let command = match args.get(1).map(String::as_str) {
        // `kelpc version` needs no further arguments, so it is answered before
        // the two commands that do.
        Some("version") => {
            println!("{BIN_NAME} {}", release_name(VERSION));
            if args[2..].iter().any(|a| a == "--verbose") {
                println!("commit: {}", env!("KELPC_COMMIT"));
                println!("build date: {}", env!("KELPC_BUILD_DATE"));
                println!("rustc: {}", env!("KELPC_RUSTC_VERSION"));
                println!("llvm: {}", env!("KELPC_LLVM_VERSION"));
            }
            return ExitCode::SUCCESS;
        }
        Some("fmt") => return fmt_command(&args[2..]),
        Some("lint") => return lint_command(&args[2..]),
        Some("doc") => return doc_command(&args[2..]),
        Some("stdlib") => return stdlib_command(&args[2..]),
        Some("check") => return check_command(&args[2..]),
        Some("caps") => return caps_command(&args[2..]),
        Some("cache") => return cache_command(&args[2..]),
        Some("repl") => return repl::repl_command(&args[2..]),
        Some(c @ ("build" | "run")) => {
            guard::start();
            c
        }
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    // A scan rather than a positional read: `-o` may come before or after the
    // source path and takes a value, so the source path is the first argument
    // that is neither a flag nor the value of one.
    let mut path: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut deny_outdated = false;
    let mut incremental = false;
    let mut print_timings = false;
    let mut opt = OptLevel::DEFAULT;
    let mut i = 2;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--backend" || arg.starts_with("--backend=") {
            // The flag that used to choose a backend is refused by name rather
            // than read as a value or reported as an unknown flag: a caller who
            // still passes it needs to know the choice is gone and which
            // backend the program gets, not that the spelling was never heard
            // of. `--backend=<name>` is the same flag run together.
            eprintln!("error: {}", backend::FLAG_RETIRED);
            return ExitCode::FAILURE;
        } else if arg == "-o" {
            let value = match args.get(i + 1) {
                Some(v) => v.clone(),
                None => {
                    eprintln!("error: '{arg}' requires a value");
                    return ExitCode::FAILURE;
                }
            };
            out = Some(PathBuf::from(value));
            i += 2;
        } else if let Some(value) = arg.strip_prefix("-o=") {
            out = Some(PathBuf::from(value));
            i += 1;
        } else if arg == "--deny-outdated" {
            deny_outdated = true;
            i += 1;
        } else if arg == "--timings" {
            // Roadmap A-14 slice 2: per-phase and per-unit wall times on stderr.
            print_timings = true;
            i += 1;
        } else if arg == "--sanitize" || arg.starts_with("--sanitize=") {
            // Refused by name: the LLVM backend links no sanitizer passes.
            eprintln!(
                "error: sanitizers are not available: the LLVM backend links no sanitizer passes"
            );
            return ExitCode::FAILURE;
        } else if arg == "--incremental" {
            // Roadmap A-14 slice 1: the backend's per-unit split. Refused for
            // `check`/`fmt`/`lint` by the same "unknown flag" path, since this
            // scan only runs for `build` and `run`.
            incremental = true;
            i += 1;
        } else if arg.starts_with("-O") {
            // One `-O<n>` argument, not `-O <n>`: the level is a single
            // spelling, so an out-of-range or unparseable one is a diagnostic
            // rather than a value read from the next argument.
            match OptLevel::parse(arg) {
                Ok(level) => opt = level,
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            }
            i += 1;
        } else if path.is_none() {
            path = Some(arg.to_string());
            i += 1;
        } else {
            eprintln!("error: unexpected argument '{arg}'");
            usage();
            return ExitCode::FAILURE;
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    // The retired backend variables are refused rather than ignored: a shell
    // that still exports one would believe its builds used another backend.
    // Presence, not the value, is what counts, so a non-UTF-8 value is reported.
    if let Err(message) = backend::check_retired_env(
        std::env::var_os(backend::BACKEND_ENV).is_some(),
        std::env::var_os(backend::ORACLE_ENV).is_some(),
    ) {
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    }

    // The default output path is a directory this tool owns, so it is created
    // on demand; an explicit `-o` is a path the *caller* chose, and a missing
    // parent directory there stays an error rather than a directory appearing
    // somewhere nobody asked for.
    let owned_dir = out.is_none();
    let out_path = out.unwrap_or_else(|| default_out_path(&path));
    if owned_dir {
        if let Err(e) = std::fs::create_dir_all(BUILD_DIR) {
            eprintln!("error: could not create '{BUILD_DIR}': {e}");
            return ExitCode::FAILURE;
        }
    }

    let mut freestanding = false;
    let mut emit = resolve::Emit::Binary;
    if !build(
        &path,
        &out_path,
        deny_outdated,
        opt,
        incremental,
        &mut freestanding,
        &mut emit,
        print_timings,
    ) {
        return ExitCode::FAILURE;
    }
    // `[build] emit = "html"` replaces the run: the `emit_entry` program is run once to
    // print its first view, and `run` does not start it as an application.
    if emit == resolve::Emit::Html {
        if freestanding {
            eprintln!(
                "error: '{path}' sets '[build] emit = \"html\"', which needs a hosted program to run once; a freestanding project's artifact is a bare-metal ELF"
            );
            return ExitCode::FAILURE;
        }
        return emit_html(&out_path);
    }

    if command == "run" {
        // A freestanding artifact is a bare-metal image: running it here would
        // be an exec-format error at best. `-o` is how it is written and a
        // boot loader or emulator is how it is run.
        if freestanding {
            eprintln!(
                "error: '{path}' is a freestanding project, whose artifact is a bare-metal ELF; 'kelpc run' does not run one"
            );
            return ExitCode::FAILURE;
        }
        let abs_out = if out_path.is_absolute() {
            out_path.clone()
        } else {
            PathBuf::from("./").join(&out_path)
        };
        guard::stop();
        match Command::new(&abs_out).status() {
            Ok(status) => {
                return ExitCode::from(status.code().unwrap_or(1) as u8);
            }
            Err(e) => {
                eprintln!("error: could not run '{}': {e}", abs_out.display());
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;

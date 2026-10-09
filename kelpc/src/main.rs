mod ast;
mod backend;
mod borrowck;
mod cache;
mod codegen_c;
mod codegen_llvm;
mod deprecated;
mod defer;
mod diag;
mod doc;
mod driver;
mod externs;
mod fmt;
mod freestanding;
mod guard;
mod infer;
mod lexer;
mod lint;
mod llvm_ffi;
mod opt;
mod parser;
mod preprocess;
mod resolve;
mod sha256;
mod stdlib;
mod timings;
mod tir;
mod types;

use backend::Backend;
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
        "usage: {BIN_NAME} build <file.{EXT}> [-o <out>] [-O0|-O1|-O2|-O3|-Os] [--deny-outdated] [--incremental] [--timings] [--sanitize=address|undefined|thread]\n       \
         {BIN_NAME} run <file.{EXT}> [-O0|-O1|-O2|-O3|-Os] [--deny-outdated] [--incremental] [--timings] [--sanitize=address|undefined|thread]\n       \
         {BIN_NAME} check <file.{EXT}> [--json] [--deny-outdated] [--text]\n       \
         {BIN_NAME} fmt <file.{EXT}>\n       \
         {BIN_NAME} fmt -w <path>...\n       \
         {BIN_NAME} fmt --check <path>...\n       \
         {BIN_NAME} lint <path>...\n       \
         {BIN_NAME} doc <file.{EXT}|project dir>... [-o <dir>] [--format md|html] [--private] [--check]\n       \
         {BIN_NAME} doc --std [-o <dir>] [--format md|html] [--private] [--check]\n       \
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
         program either way. The C oracle (KELPC_ORACLE=c) ignores it and\n\
         keeps its one translation unit, and a freestanding project is\n\
         refused by name for now (plan/incremental-compilation.md).\n\n\
         Cache misses are compiled on worker threads, each unit in its own\n\
         LLVM context. KELPC_JOBS names the worker count (a positive integer;\n\
         the default is 2, not the CPU count); more workers than units spawns\n\
         one per unit.\n\n\
         --sanitize=address|undefined|thread (or '[build] sanitize = \"address\"'\n\
         in coral.toml; the flag wins) builds the program with that sanitizer\n\
         and links its runtime. Only the C backend can do it, so the build is\n\
         refused on the default LLVM backend with a message naming the way:\n\
         run it as KELPC_ORACLE=c kelpc run --sanitize=address file.kp. address\n\
         catches out-of-bounds and use-after-free through unsafe, Ptr<T>, FFI\n\
         and asm, and leaks; undefined stops the program at the first undefined\n\
         C behavior; thread reports data races between spawned threads\n\
         (plan/sanitizers.md).\n\n\
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

    let typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            findings.push((Severity::Error, d));
            return (findings, false);
        }
    };

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
/// `opt` is the `-O` level (MK7 S6, the record in `plan/done.md`). It is
/// passed to whichever backend runs below, and to the `cc` calls each one makes,
/// so one level is one level for both backends.
///
/// Everything before `backend` is backend-independent, which is why the suite
/// that exercises it does not care which backend is selected; the seam is the
/// one `match` below, and it is the only place a code generator is named.
fn build(
    path: &str,
    out_path: &Path,
    backend: Backend,
    deny_outdated: bool,
    opt: OptLevel,
    incremental_flag: bool,
    sanitize_flag: Option<driver::Sanitizer>,
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
    let mut typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };
    timing.infer += infer_start.elapsed();
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

    // Kernel stage 4: a freestanding project has no hosted runtime to build
    // against, so the C backend's libc preamble cannot express it. The refusal
    // is here, before either generator runs, rather than at the `cc` failure a
    // C build would produce.
    if loaded.project.target().is_freestanding() && backend == Backend::C {
        eprintln!(
            "error: '{}' sets '[target] mode = \"freestanding\"', which only the llvm backend can build: the C backend's runtime preamble is libc (the KELPC_ORACLE switch cannot build a freestanding project)",
            path
        );
        return false;
    }
    *freestanding_out = loaded.project.target().is_freestanding();
    *emit_out = loaded.project.emit();

    // The sanitizer: the flag wins over the manifest. Only the C backend can
    // build one (the LLVM backend links no sanitizer passes), so the default
    // backend refuses by name and points at the oracle switch.
    let sanitize = sanitize_flag.or(loaded.project.sanitize());
    if let Some(kind) = sanitize {
        if backend == Backend::Llvm {
            eprintln!(
                "error: sanitizer '{}' is not available on the LLVM backend, which links no sanitizer passes; build with the C backend instead, which supports address, undefined and thread: KELPC_ORACLE=c {BIN_NAME} build --sanitize={} {path}",
                kind.name(),
                kind.name()
            );
            return false;
        }
    }

    // Roadmap A-14 slice 1: the backend's per-unit split. `--incremental` and
    // the manifest's `[build] incremental = true` both turn it on, and the C
    // oracle ignores it, because the C backend's one translation unit is the
    // other answer to the same question rather than a second implementation of
    // this mode. A freestanding project is refused by name for now: its image
    // is one ELF with one entry stub and one panic state, which is a slice of
    // its own (`plan/incremental-compilation.md`).
    let incremental = incremental_flag || loaded.project.incremental();
    if incremental && *freestanding_out && backend == Backend::Llvm {
        eprintln!(
            "error: '--incremental' is not implemented for a freestanding project yet: its image is one ELF with a single entry stub and one panic state, so the per-unit split is its own slice (plan/incremental-compilation.md)"
        );
        return false;
    }
    let incremental = incremental && backend == Backend::Llvm;
    if incremental {
        let cache = cache::Cache::for_build(out_path);
        if let Err(message) = cache.store_interfaces(&loaded.interfaces) {
            eprintln!("error: {message}");
            return false;
        }
        timing.interfaces = Some((cache.stats().interface_hits(), cache.stats().interface_misses()));
    }

    // One `pkg-config` invocation for every package the manifest's `[extern.*]`
    // sections named (MK9-S1-M1), shared by both backends. It runs before the
    // backend seam because a package that cannot be resolved is a build error,
    // not a per-backend one, and the same flags have to reach both.
    let pkg_config_flags =
        match driver::pkg_config_flags(&loaded.project.extern_pkg_config_packages()) {
            Ok(flags) => flags,
            Err(e) => {
                eprintln!("error: {}", e.message);
                return false;
            }
        };

    // The backend seam, and the only place a code generator is named. Both arms
    // are live: `llvm` is what every build gets, and `c` is reached only through
    // the project's oracle switch (`backend::ORACLE_ENV`), which is how the
    // suite builds the fixtures it compares an `llvm` build against.
    match backend {
        Backend::C => {
            let c_source = codegen_c::generate(&typed, &drops, &sources);
            let link_libs = loaded.project.extern_libraries();
            let include_dirs = loaded.project.extern_include_dirs();
            let library_dirs = loaded.project.extern_library_dirs();
            if let Err(e) = driver::compile_c(
                &c_source,
                out_path,
                opt,
                &link_libs,
                &include_dirs,
                &library_dirs,
                &pkg_config_flags,
                codegen_c::uses_tls(&typed),
                sanitize,
            ) {
                eprintln!("error: {}", e.message);
                return false;
            }
        }
        Backend::Llvm => {
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
            // The LLVM backend builds its own module, writes the textual IR
            // beside the binary, and links the object itself; the C source
            // string the arm above writes has no counterpart here.
            let link_libs = loaded.project.extern_libraries();
            let include_dirs = loaded.project.extern_include_dirs();
            let library_dirs = loaded.project.extern_library_dirs();
            if let Err(message) = codegen_llvm::compile(
                &typed,
                &drops,
                &sources,
                out_path,
                opt,
                &link_libs,
                &include_dirs,
                &library_dirs,
                &pkg_config_flags,
                freestanding.as_ref(),
                incremental,
                &mut timing,
            ) {
                eprintln!("error: {message}");
                return false;
            }
        }
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
            "error: '{}' printed no HTML document for '[build] emit = \"html\"': the [build] emit_entry must call polyp.emit_html (examples/polyp/lib.kp)",
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

/// `kelpc fmt`: reformat `.kp` source. One file is written to stdout;
/// `-w` and `--check` take a file or a directory (recursing `.kp` files).
///
/// Formatting is per-file: the file is lexed and parsed on its own and no
/// `coral.toml` is required. The one thing one file cannot carry is the alias
/// table - a `%*name*%` may be declared by a module this file imports - so
/// when a file's own declarations cannot resolve its references, the file is
/// put to the compiler's own resolver, which reads the project exactly as
/// `kelpc build` does; see `fmt_text`.
///
/// Every path is resolved and its type settled by `fmt_file` before anything
/// reads it, so a FIFO, a socket or a device is refused rather than blocked on,
/// in every mode.
fn fmt_command(args: &[String]) -> ExitCode {
    let mut write = false;
    let mut check = false;
    let mut paths: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-w" => write = true,
            "--check" => check = true,
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("error: unknown flag '{other}'; expected one of: -w, --check");
                return ExitCode::FAILURE;
            }
            other => paths.push(other.to_string()),
        }
    }
    if write && check {
        eprintln!("error: '-w' and '--check' cannot be combined");
        return ExitCode::FAILURE;
    }
    if !write && !check {
        if paths.len() != 1 {
            eprintln!("error: 'fmt' without '-w' or '--check' writes exactly one file to stdout");
            return ExitCode::FAILURE;
        }
        return match format_one(Path::new(&paths[0])) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(()) => ExitCode::FAILURE,
        };
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for path in &paths {
        match collect_kp(Path::new(path), "fmt") {
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
    // A link inside a directory is never followed, and never silently dropped
    // either: the run says which ones it left out, so nobody has to guess why
    // a file they can see was not formatted.
    for link in &skipped {
        eprintln!(
            "note: skipping '{link}': a symbolic link found inside a directory is not \
             followed (-w and --check never rewrite a file through one)"
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
        let ok = if check {
            check_one(file)
        } else {
            write_one(file)
        };
        failed |= !ok;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Formats one file for the mode that only reports its text. `Err(())` means a
/// diagnostic was already printed.
fn format_one(path: &Path) -> Result<String, ()> {
    let target = match fmt_file(path) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(());
        }
    };
    let src = read_source(path)?;
    fmt_text(&target, &src)
}

/// Reads a file, reporting a failure the way every `fmt` mode reports one.
fn read_source(path: &Path) -> Result<String, ()> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!(
            "error: {}",
            RefusalKind::UnreadableFile.message(format!("cannot read '{}': {e}", path.display()))
        );
    })
}

/// The formatter's verdict on one file, with `kelpc build`'s front end as the
/// second opinion.
///
/// `fmt`'s own gate parses the file against the `alias` declarations *the file
/// itself* holds, which is enough for every file that declares what it uses.
/// It is not enough for a `%*name*%` another module declares: the gate cannot
/// see that module, so it cannot tell an undefined alias from a project one,
/// and it refuses both.
///
/// That refusal is not final. The resolver is the authority on it, because it
/// is what `kelpc build` runs and it loads the whole project: it expands the
/// file against the project's own alias table, the `[alias]` section included, and
/// parses the result. So when this file's own gate refuses it, the file is put
/// to the resolver:
///
/// - the resolver accepts it - the file parses under the project's table, which
///   is a stronger check than the one that refused it - and it is formatted;
/// - the resolver rejects it too, and its diagnostic is printed verbatim. That
///   is the same text, at the same position, that `kelpc build` prints for that
///   file.
///
/// The resolver is only consulted for a file the gate could not check on its
/// own, so a file with no aliases never loads a module graph at all. What `fmt`
/// validates is therefore the file's own text, plus - only when the file's own
/// gate refuses it - the project alias table it needs to expand that file. It
/// does not validate the project. A project broken somewhere this file does not
/// reach (a cyclic or malformed `[alias]` entry, or an imported module with a
/// syntax error) makes `kelpc build` exit 1 with a real diagnostic while `fmt`
/// formats a file that references no unresolvable alias; the two answer
/// different questions and agree only about the file they are given.
fn fmt_text(path: &Path, src: &str) -> Result<String, ()> {
    let own = fmt::format(src);
    let Ok(text) = own else {
        let mut sources = SourceMap::new();
        return match resolve::load(path, &mut sources, None, None) {
            Ok(_) => fmt::format_after_resolve(src).map_err(|e| {
                report_fmt_error(path, src, &e);
            }),
            Err(d) => {
                sources.report(&d);
                Err(())
            }
        };
    };
    Ok(text)
}

/// Renders a formatter failure through the compiler's own diagnostics, so a
/// parse error reads the same whether it came from `build` or from `fmt`.
fn report_fmt_error(path: &Path, src: &str, e: &fmt::FmtError) {
    let mut sources = SourceMap::new();
    let module = path.to_string_lossy().to_string();
    sources.insert(&module, path, src.to_string());
    sources.report(&Diagnostic::new(e.message.clone(), e.span, module));
}

/// The file a `fmt` path names.
///
/// A symbolic link given by name is resolved to the file it points at - through
/// a chain of links, absolute or relative targets alike - so that `-w` rewrites
/// *that* file and the link is left as a link. Writing to the link's own path
/// instead would rename a temporary over the link itself: the link would be
/// replaced by a regular file and the file the user meant would never change.
/// A link that names a directory, or names nothing at all, is refused rather
/// than guessed at.
///
/// The file that is finally named has to be a *regular* file, and that is
/// settled here, before any caller reads it: an open is what blocks on a FIFO
/// with no writer, so a check that ran after a read would never run at all, and
/// `-w` would replace the node it was handed with a regular file.
///
/// A path that is not a link is returned unchanged, so diagnostics keep naming
/// the path the user typed.
fn fmt_file(path: &Path) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        RefusalKind::PathNamesNothing
            .message(format!("cannot read '{}': {e}", path.display()))
    })?;
    if !meta.file_type().is_symlink() {
        require_regular(path, None)?;
        return Ok(path.to_path_buf());
    }
    let target = fs::canonicalize(path).map_err(|e| {
        let link = fs::read_link(path)
            .map(|t| t.display().to_string())
            .unwrap_or_else(|_| "<unreadable>".to_string());
        RefusalKind::DanglingSymlink.message(format!(
            "'{}' is a symbolic link to '{link}', which cannot be read: {e}",
            path.display()
        ))
    })?;
    if target.is_dir() {
        return Err(RefusalKind::NotARegularFile.message(format!(
            "'{}' is a symbolic link to the directory '{}'; fmt writes files, not \
             directories - pass the directory itself",
            path.display(),
            target.display()
        )));
    }
    require_regular(path, Some(&target))?;
    Ok(target)
}

/// Refuses a path that is not a regular file. Nothing has opened it: `metadata`
/// stats, and a *read* is what would block on a FIFO with no writer, or hang on
/// a socket nobody is writing to. `given` is the path the user typed, and
/// `resolved` is the file it names when the two differ (a symbolic link), so
/// the diagnostic names what was found and where.
fn require_regular(given: &Path, resolved: Option<&Path>) -> Result<(), String> {
    let target = resolved.unwrap_or(given);
    let meta = fs::metadata(target).map_err(|e| {
        RefusalKind::PathNamesNothing
            .message(format!("cannot read '{}': {e}", target.display()))
    })?;
    if meta.is_file() {
        return Ok(());
    }
    let kind = node_kind(&meta);
    Err(RefusalKind::NotARegularFile.message(match resolved {
        None => format!(
            "'{}' is a {kind}, not a regular file: fmt reads and rewrites regular files only",
            given.display()
        ),
        Some(target) => format!(
            "'{}' is a symbolic link to '{}', which is a {kind}, not a regular file: fmt \
             reads and rewrites regular files only",
            given.display(),
            target.display()
        ),
    }))
}

/// What a node is, for the refusal above. `metadata` never lies about this: the
/// type is a property of the directory entry, not of anything read out of it.
fn node_kind(meta: &fs::Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = meta.file_type();
        if file_type.is_fifo() {
            return "FIFO";
        }
        if file_type.is_socket() {
            return "socket";
        }
        if file_type.is_block_device() {
            return "block device";
        }
        if file_type.is_char_device() {
            return "character device";
        }
    }
    if meta.is_dir() {
        return "directory";
    }
    "non-regular file"
}

/// Whether `path` itself is a symbolic link, without following it.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// `--check`: true when the file is already formatted.
fn check_one(path: &Path) -> bool {
    let target = match fmt_file(path) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("error: {message}");
            return false;
        }
    };
    let src = match read_source(path) {
        Ok(src) => src,
        Err(()) => return false,
    };
    match fmt_text(&target, &src) {
        Ok(text) if text == src => true,
        Ok(_) => {
            eprintln!(
                "error: {}",
                RefusalKind::CheckFoundUnformatted
                    .message(format!("'{}' is not formatted", path.display()))
            );
            false
        }
        Err(()) => false,
    }
}

/// `-w`: rewrite in place, and only when the bytes actually changed.
fn write_one(path: &Path) -> bool {
    let target = match fmt_file(path) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("error: {message}");
            return false;
        }
    };
    let src = match read_source(path) {
        Ok(src) => src,
        Err(()) => return false,
    };
    let text = match fmt_text(&target, &src) {
        Ok(text) => text,
        Err(()) => return false,
    };
    if text == src {
        return true;
    }
    match write_atomic(&target, &text) {
        Ok(()) => {
            // The link case is said out loud, but only after the write landed:
            // the note asserts that the file named by the link was rewritten,
            // and a note printed before the write would claim a rewrite that a
            // later failure prevented. On failure only the failure is reported.
            // The write lands on a file the caller did not name, and a reader
            // of the output should not have to work out which.
            if is_symlink(path) {
                eprintln!(
                    "note: '{}' is a symbolic link to '{}'; the link is kept and the file \
                     it names is rewritten",
                    path.display(),
                    target.display()
                );
            }
            true
        }
        Err(e) => {
            eprintln!(
                "error: {}",
                RefusalKind::WriteFailed
                    .message(format!("cannot write '{}': {e}", path.display()))
            );
            false
        }
    }
}

/// Writes `contents` to `target` through a temporary file in the same directory
/// and then renames it, so an interrupted run never leaves a half-written
/// source file: a reader sees the old bytes or the new ones, never a mixture.
///
/// `target` is already the resolved file (`fmt_file`), so the temporary lands in
/// the directory the write happens in - the same filesystem, which is what makes
/// the rename atomic - and not next to a link that points somewhere else.
///
/// The temporary's name carries the process id and a per-process counter, so two
/// `fmt -w` runs against the same path write two different temporaries. With one
/// fixed name they would write the *same* one, and either run could rename the
/// other's half-written bytes into place - or find the file already renamed away
/// and fail on it. The rename is one step either way, so the last run to finish
/// wins and the file is never a mixture of the two. The name is not taken from
/// the source, so a target whose own name is at the filesystem's limit still
/// gets a temporary the directory can hold.
///
/// A failure *after* the temporary exists - a write that runs out of space, a
/// rename that fails - removes it before returning. A leftover temporary in a
/// source directory is a file the next `git status` reports and nothing owns.
///
/// The rename makes the file that lands a **new inode**, and that is a
/// documented limit rather than an oversight (see `fmt.rs`): a hard link to the
/// formatted file, and a file descriptor another process already holds, keep the
/// old bytes. This function copies the target's mode bits onto the temporary and
/// nothing else, so of the four attributes a replacement file could carry: the
/// permission bits and the group are both asserted by `tests/fmt.rs` (the mode
/// bits follow the file, and a file moved to a secondary group first does not
/// carry that group through); the owner cannot be varied without privilege, so
/// "the owner does not follow" rests on reading this function; and ACLs and
/// extended attributes rest on the same reading - mode bits have nowhere to put
/// either, and no test measures them. Writing the file in place instead would
/// keep all of that and lose the atomicity above, which is the property this
/// function exists for.
fn write_atomic(target: &Path, contents: &str) -> std::io::Result<()> {
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let tmp = dir.join(format!(
        ".kelpc-fmt-{}-{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| {
        fs::write(&tmp, contents)?;
        if let Ok(meta) = fs::metadata(target) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        // Whatever went wrong, the temporary is not the caller's to clean up.
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Counter that keeps two temporaries from the same process apart.
static TEMP_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The `.kp` files a `fmt` path names: an explicitly named file (whatever its
/// extension - `fmt FILE` to stdout accepts one too), or every `.kp` file under
/// a directory. Also returns the symbolic links the walk refused to follow, so
/// the caller can report them.
///
/// A link given *by name* stays a file path (`fmt_file` resolves it when it
/// writes); a link that names a directory is refused here, because a directory
/// command would recurse through it and rewrite a tree under a name that is not
/// that tree's own.
fn collect_kp(path: &Path, command: &str) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        RefusalKind::PathNamesNothing
            .message(format!("cannot read '{}': {e}", path.display()))
    })?;
    if meta.file_type().is_symlink() {
        if fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
            let real = fs::canonicalize(path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            return Err(RefusalKind::NotARegularFile.message(format!(
                "'{}' is a symbolic link to the directory '{real}'; {command} does not follow it - \
                 pass the directory itself",
                path.display()
            )));
        }
        // A link to a file - or a dangling one, which the per-file gate reports
        // with the link named.
        return Ok((vec![path.to_path_buf()], Vec::new()));
    }
    if meta.is_dir() {
        let mut out = Vec::new();
        let mut skipped = Vec::new();
        collect_kp_into(path, &mut out, &mut skipped)
            .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
        Ok((out, skipped))
    } else {
        Ok((vec![path.to_path_buf()], Vec::new()))
    }
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

/// `kelpc doc`: the documentation generator (`plan/doc-tool.md`). The pages
/// are built by `doc.rs`; this reads the sources, writes the files, and runs
/// the `--check` gate.
fn doc_command(args: &[String]) -> ExitCode {
    let mut out_dir = PathBuf::from("doc");
    let mut format = doc::Format::Markdown;
    let mut private = false;
    let mut check = false;
    let mut std_mode = false;
    let mut paths: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
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
                    other => {
                        eprintln!(
                            "error: '--format' takes 'md' or 'html', found '{}'",
                            other.unwrap_or("nothing")
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--private" => private = true,
            "--check" => check = true,
            "--std" => std_mode = true,
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!(
                    "error: unknown flag '{other}'; expected one of: -o, --format, --private, --check, --std"
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
            let text = match read_source(&file) {
                Ok(text) => text,
                Err(()) => {
                    failed = true;
                    continue;
                }
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
        Some("check") => return check_command(&args[2..]),
        Some("cache") => return cache_command(&args[2..]),
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
    let mut sanitize: Option<driver::Sanitizer> = None;
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
        } else if let Some(value) = arg.strip_prefix("--sanitize=") {
            match driver::Sanitizer::parse(value) {
                Some(kind) => sanitize = Some(kind),
                None => {
                    eprintln!(
                        "error: '--sanitize={value}' is not a sanitizer; expected address, undefined or thread"
                    );
                    return ExitCode::FAILURE;
                }
            }
            i += 1;
        } else if arg == "--sanitize" {
            eprintln!("error: '--sanitize' takes its value as '--sanitize=address|undefined|thread'");
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

    // The backend. `KELPC_BACKEND` is gone, and a set one is refused rather than
    // ignored; the only switch left is the project's oracle switch
    // (`backend::ORACLE_ENV`), which accepts only the C backend. Presence, not
    // the value, is what makes a stale `KELPC_BACKEND` a refusal, so even a
    // non-UTF-8 one is reported.
    let retired_set = std::env::var_os(backend::RETIRED_ENV).is_some();
    let env_oracle = std::env::var(backend::ORACLE_ENV).ok();
    let backend = match backend::resolve(retired_set, env_oracle.as_deref()) {
        Ok(b) => b,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

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
        backend,
        deny_outdated,
        opt,
        incremental,
        sanitize,
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
mod tests {
    use super::*;

    #[test]
    fn a_crate_version_becomes_the_padded_release_name() {
        assert_eq!(release_name("26.9.0"), "26.09.0");
        assert_eq!(release_name("26.12.3"), "26.12.3");
        assert_eq!(release_name("26.1.10"), "26.01.10");
    }

    #[test]
    fn a_version_that_is_not_this_scheme_is_left_alone() {
        // The numbering before `YY.MM.N`, and anything that is not three
        // numeric parts, is passed through rather than padded into nonsense.
        assert_eq!(release_name("0.1.6"), "0.1.6");
        assert_eq!(release_name("nightly"), "nightly");
        assert_eq!(release_name("26.9"), "26.9");
    }

    #[test]
    fn this_builds_release_name_has_the_schemes_shape() {
        // Durable across a version bump: the shape is what the scheme pins,
        // not the number this tree happens to carry today.
        let name = release_name(VERSION);
        let parts: Vec<&str> = name.split('.').collect();
        assert_eq!(parts.len(), 3, "'{name}' is not YY.MM.N");
        assert_eq!(parts[0].len(), 2, "'{name}' has a one-digit year");
        assert_eq!(parts[1].len(), 2, "'{name}' has a one-digit month");
        assert!(
            parts[2].chars().all(|c| c.is_ascii_digit()) && !parts[2].is_empty(),
            "'{name}' has no release number"
        );
    }
}

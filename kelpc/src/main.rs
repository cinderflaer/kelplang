mod ast;
mod backend;
mod borrowck;
mod codegen_asm;
mod codegen_c;
mod deprecated;
mod diag;
mod driver;
mod externs;
mod fmt;
mod infer;
mod lexer;
mod lint;
mod parser;
mod preprocess;
mod pretty;
mod resolve;
mod stdlib;
mod tir;
mod types;

use backend::Backend;
use diag::{Diagnostic, SourceMap};
use fmt::RefusalKind;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Name this compiler is installed and invoked as. Kept in sync with the
/// crate's `[package] name` and `[[bin]] name` in `Cargo.toml`.
const BIN_NAME: &str = "kelpc";

/// The version `kelpc version` prints, taken from `[package] version` at
/// compile time so it cannot drift from `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        "usage: {BIN_NAME} build <file.{EXT}> [-o <out>] [--backend=<name>] [--deny-outdated]\n       \
         {BIN_NAME} run <file.{EXT}> [--deny-outdated]\n       \
         {BIN_NAME} fmt <file.{EXT}>\n       \
         {BIN_NAME} fmt -w <path>...\n       \
         {BIN_NAME} fmt --check <path>...\n       \
         {BIN_NAME} lint <path>...\n       \
         {BIN_NAME} version\n\n\
         The backend may also be chosen with the KELPC_BACKEND environment\n\
         variable; an explicit --backend wins over it. The default is 'c'.\n\n\
         A spelling in the register of outdated syntax still compiles, and\n\
         build and run print it as an 'outdated:' warning naming what to write\n\
         instead and saying it is removed in the next development cycle.\n\
         --deny-outdated turns those findings into errors, which is the\n\
         migration gate for a project that wants to know when its own code is\n\
         on the list.\n\n\
         With no -o, the binary and the C it was generated from are written to\n\\
         {BUILD_DIR}/<file stem>, which the compiler creates if it is missing.\n\n\
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
         program, and a rewritten file is a new inode — a hard link to it\n\
         keeps the old bytes. A file that does not parse — by fmt's own\n\
         reading of it, or by the compiler's when the project's aliases are\n\
         needed — is reported as a parse error and left alone.\n\n\
         lint prints warnings for the sources under each path (.{EXT} files,\n\
         recursing a directory, with a symbolic link inside a directory\n\
         skipped and reported). A warning is not a failure: lint exits zero\n\
         with warnings on stderr, and nonzero only for a path it could not\n\
         read or a file it could not lex. It never writes to a file.",
        EXT = resolve::EXT
    );
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
/// Everything before `backend` is backend-independent, which is why the suite
/// that exercises it does not care which backend is selected; the seam is the
/// one `match` below, and it is the only place a code generator is named.
fn build(path: &str, out_path: &Path, backend: Backend, deny_outdated: bool) -> bool {
    let mut sources = SourceMap::new();

    let loaded = match resolve::load(Path::new(path), &mut sources) {
        Ok(l) => l,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };
    if !report_deprecations(&sources, &loaded.deprecations, deny_outdated) {
        return false;
    }

    let typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };

    let drops = match borrowck::check_program(&typed) {
        Ok(plan) => plan,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };

    // The backend seam (MK7 stage 1). A second backend adds its own arm here,
    // and the fixture suite already knows how to validate it: it is
    // backend-agnostic by construction, so a second run with
    // `KELPC_BACKEND=<name>` exercises the same fixtures against it.
    match backend {
        Backend::C => {
            let c_source = codegen_c::generate(&typed, &drops, &sources);
            if let Err(e) = driver::compile_c(&c_source, out_path) {
                eprintln!("error: {}", e.message);
                return false;
            }
        }
        Backend::Asm => {
            // A refusal from the generator is a normal compile error, printed
            // the same way a frontend diagnostic is: an uncovered construct
            // must fail the build, never produce a binary that is wrong.
            let asm_source = match codegen_asm::generate(&typed, &drops, &sources) {
                Ok(source) => source,
                Err(message) => {
                    eprintln!("error: {message}");
                    return false;
                }
            };
            if let Err(e) = driver::assemble_and_link(&asm_source, out_path) {
                eprintln!("error: {}", e.message);
                return false;
            }
        }
    }

    true
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
/// a build is *named* changes — only where it lands.
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
/// table — a `%*name*%` may be declared by a module this file imports — so
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
/// file against the project's own alias table, `alias.kpconf` included, and
/// parses the result. So when this file's own gate refuses it, the file is put
/// to the resolver:
///
/// - the resolver accepts it — the file parses under the project's table, which
///   is a stronger check than the one that refused it — and it is formatted;
/// - the resolver rejects it too, and its diagnostic is printed verbatim. That
///   is the same text, at the same position, that `kelpc build` prints for that
///   file.
///
/// The resolver is only consulted for a file the gate could not check on its
/// own, so a file with no aliases never loads a module graph at all. What `fmt`
/// validates is therefore the file's own text, plus — only when the file's own
/// gate refuses it — the project alias table it needs to expand that file. It
/// does not validate the project. A project broken somewhere this file does not
/// reach (a cyclic or malformed `alias.kpconf`, or an imported module with a
/// syntax error) makes `kelpc build` exit 1 with a real diagnostic while `fmt`
/// formats a file that references no unresolvable alias; the two answer
/// different questions and agree only about the file they are given.
fn fmt_text(path: &Path, src: &str) -> Result<String, ()> {
    let own = fmt::format(src);
    let Ok(text) = own else {
        let mut sources = SourceMap::new();
        return match resolve::load(path, &mut sources) {
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
/// A symbolic link given by name is resolved to the file it points at — through
/// a chain of links, absolute or relative targets alike — so that `-w` rewrites
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
             directories — pass the directory itself",
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
/// the directory the write happens in — the same filesystem, which is what makes
/// the rename atomic — and not next to a link that points somewhere else.
///
/// The temporary's name carries the process id and a per-process counter, so two
/// `fmt -w` runs against the same path write two different temporaries. With one
/// fixed name they would write the *same* one, and either run could rename the
/// other's half-written bytes into place — or find the file already renamed away
/// and fail on it. The rename is one step either way, so the last run to finish
/// wins and the file is never a mixture of the two. The name is not taken from
/// the source, so a target whose own name is at the filesystem's limit still
/// gets a temporary the directory can hold.
///
/// A failure *after* the temporary exists — a write that runs out of space, a
/// rename that fails — removes it before returning. A leftover temporary in a
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
/// extended attributes rest on the same reading — mode bits have nowhere to put
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
/// extension — `fmt FILE` to stdout accepts one too), or every `.kp` file under
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
                "'{}' is a symbolic link to the directory '{real}'; {command} does not follow it — \
                 pass the directory itself",
                path.display()
            )));
        }
        // A link to a file — or a dangling one, which the per-file gate reports
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
/// holds this one — `ln -s . here` is enough — so following one could format a
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let command = match args.get(1).map(String::as_str) {
        // `kelpc version` needs no further arguments, so it is answered before
        // the two commands that do.
        Some("version") => {
            println!("{BIN_NAME} {VERSION}");
            return ExitCode::SUCCESS;
        }
        Some("fmt") => return fmt_command(&args[2..]),
        Some("lint") => return lint_command(&args[2..]),
        Some(c @ ("build" | "run")) => c,
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    // A scan rather than a positional read: `--backend` may come before or
    // after the source path, and both it and `-o` take a value. The source
    // path is the first argument that is neither.
    let mut path: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut backend_flag: Option<String> = None;
    let mut deny_outdated = false;
    let mut i = 2;
    while i < args.len() {
        let arg = args[i].as_str();
        let takes_value = arg == "-o" || arg == "--backend";
        let inline = arg
            .strip_prefix("--backend=")
            .or_else(|| arg.strip_prefix("-o="));
        if takes_value {
            let value = match args.get(i + 1) {
                Some(v) => v.clone(),
                None => {
                    eprintln!("error: '{arg}' requires a value");
                    return ExitCode::FAILURE;
                }
            };
            if arg == "-o" {
                out = Some(PathBuf::from(value));
            } else {
                backend_flag = Some(value);
            }
            i += 2;
        } else if let Some(value) = inline {
            if arg.starts_with("--backend=") {
                backend_flag = Some(value.to_string());
            } else {
                out = Some(PathBuf::from(value));
            }
            i += 1;
        } else if arg == "--deny-outdated" {
            deny_outdated = true;
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

    // The flag wins over the environment, so one-off runs are possible without
    // editing whatever set the variable. A non-UTF-8 value reads as unset, the
    // same as an empty one being reported rather than silently defaulting.
    let env_backend = std::env::var("KELPC_BACKEND").ok();
    let backend = match backend::resolve(backend_flag.as_deref(), env_backend.as_deref()) {
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

    if !build(&path, &out_path, backend, deny_outdated) {
        return ExitCode::FAILURE;
    }

    if command == "run" {
        let abs_out = if out_path.is_absolute() {
            out_path.clone()
        } else {
            PathBuf::from("./").join(&out_path)
        };
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

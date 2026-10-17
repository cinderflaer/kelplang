use super::*;

/// `kelpc fmt`: reformat `.kp` source. One file is written to stdout;
/// `-w` and `--check` take a file or a directory (recursing `.kp` files).
///
/// Formatting is per-file: the file is lexed and parsed on its own and no
/// `coral.toml` is required. The one thing one file cannot carry is the alias
/// table - a `_name` may be declared by a module this file imports - so
/// when a file's own declarations cannot resolve its references, the file is
/// put to the compiler's own resolver, which reads the project exactly as
/// `kelpc build` does; see `fmt_text`.
///
/// Every path is resolved and its type settled by `fmt_file` before anything
/// reads it, so a FIFO, a socket or a device is refused rather than blocked on,
/// in every mode.
pub(crate) fn fmt_command(args: &[String]) -> ExitCode {
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
pub(crate) fn format_one(path: &Path) -> Result<String, ()> {
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
pub(crate) fn read_source(path: &Path) -> Result<String, ()> {
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
/// It is not enough for a `_name` another module declares: the gate cannot
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
pub(crate) fn fmt_text(path: &Path, src: &str) -> Result<String, ()> {
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
pub(crate) fn report_fmt_error(path: &Path, src: &str, e: &fmt::FmtError) {
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
pub(crate) fn fmt_file(path: &Path) -> Result<PathBuf, String> {
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
pub(crate) fn require_regular(given: &Path, resolved: Option<&Path>) -> Result<(), String> {
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
pub(crate) fn node_kind(meta: &fs::Metadata) -> &'static str {
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
pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// `--check`: true when the file is already formatted.
pub(crate) fn check_one(path: &Path) -> bool {
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
pub(crate) fn write_one(path: &Path) -> bool {
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
pub(crate) fn write_atomic(target: &Path, contents: &str) -> std::io::Result<()> {
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
pub(crate) static TEMP_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The `.kp` files a `fmt` path names: an explicitly named file (whatever its
/// extension - `fmt FILE` to stdout accepts one too), or every `.kp` file under
/// a directory. Also returns the symbolic links the walk refused to follow, so
/// the caller can report them.
///
/// A link given *by name* stays a file path (`fmt_file` resolves it when it
/// writes); a link that names a directory is refused here, because a directory
/// command would recurse through it and rewrite a tree under a name that is not
/// that tree's own.
pub(crate) fn collect_kp(path: &Path, command: &str) -> Result<(Vec<PathBuf>, Vec<String>), String> {
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

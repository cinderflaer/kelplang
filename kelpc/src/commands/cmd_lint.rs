use super::*;

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
pub(crate) fn lint_command(args: &[String]) -> ExitCode {
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
pub(crate) fn lint_one(path: &Path) -> bool {
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
pub(crate) fn naming_for(path: &Path) -> Option<lint::Naming> {
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
pub(crate) fn collect_kp_into(
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

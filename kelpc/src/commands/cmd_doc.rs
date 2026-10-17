use super::*;

/// `kelpc doc`: the documentation generator (`kelp wiki page tooling/kelpc-doc.md`). The pages
/// are built by `doc.rs`; this reads the sources, writes the files, and runs
/// the `--check` gate.
pub(crate) fn doc_command(args: &[String]) -> ExitCode {
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

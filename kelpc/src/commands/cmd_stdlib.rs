use super::*;

/// `kelpc stdlib --json --dir <dir>`: writes every shipped std module's source
/// to `<dir>/<build-id>/<module path>.kp` and prints the listing as JSON: the
/// build id, the compiler version, and per module its id, file path, SHA-256 of
/// the source and its synthetic declarations (`stdlib::SYNTHETIC_ITEMS`).
///
/// A file that already holds the right bytes is left alone, so a second run is
/// a read. The build id changes whenever the table does, so a directory written
/// by another compiler is never read for this one.
pub(crate) fn stdlib_command(args: &[String]) -> ExitCode {
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

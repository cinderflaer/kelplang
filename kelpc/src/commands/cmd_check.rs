use super::*;

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
pub(crate) fn check_command(args: &[String]) -> ExitCode {
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
pub(crate) fn caps_command(args: &[String]) -> ExitCode {
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

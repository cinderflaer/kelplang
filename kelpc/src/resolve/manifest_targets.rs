use super::*;

/// One `key = value` line inside `coral.toml`'s `[extern."<module_id>"]`
/// section (MK9-S1-M1, roadmap B-1): `domain = "<one of stdlib::DOMAINS>"`,
/// `library = "<native library name>"`, or `symbols = ["<name>", ...]`. A
/// `symbols` entry is `"<kelp name>"` (same native symbol) or
/// `"<kelp name>=<c symbol>"`. The header line already inserted a default `ExternModuleDecl` for
/// `module_id` (mirrors `[caps]`'s duplicate-domain check, but at the
/// section level since a module gets at most one `[extern.*]` entry), so
/// this only ever updates it.
pub(crate) fn parse_extern_line(
    manifest: &mut Manifest,
    module_id: &str,
    line: &str,
    at: Span,
) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!(
                "expected 'key = value' in {MANIFEST_NAME}'s '[extern.\"{module_id}\"]' section, found '{line}'"
            ),
            at,
            "",
        ));
    };
    let key = key.trim();
    let value = value.trim();
    let entry = manifest
        .extern_modules
        .get_mut(module_id)
        .expect("the '[extern.\"...\"]' header always inserts its entry first");
    match key {
        "domain" => {
            let Some(domain) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] domain' must be a double-quoted string"
                    ),
                    at,
                    "",
                ));
            };
            if !crate::stdlib::is_domain(domain) {
                return Err(Diagnostic::new(
                    format!(
                        "'{domain}' is not a capability domain; v1 has {}",
                        crate::stdlib::DOMAINS.join(", ")
                    ),
                    at,
                    "",
                ));
            }
            entry.domain = Some(domain.to_string());
        }
        "library" => {
            let Some(library) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] library' must be a double-quoted string"
                    ),
                    at,
                    "",
                ));
            };
            entry.library = Some(library.to_string());
        }
        "header" => {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME}'s '[extern.\"{module_id}\"] header' was removed: delete the header line. No C is compiled now, so the key does nothing"
                ),
                at,
                "",
            ));
        }
        "include_dirs" => {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME}'s '[extern.\"{module_id}\"] include_dirs' was removed: delete the include_dirs line. No C is compiled now, so include directories do nothing; library_dirs is still read"
                ),
                at,
                "",
            ));
        }
        "library_dirs" => {
            entry.library_dirs = parse_dir_list(value, module_id, "library_dirs", at)?;
        }
        "pkg_config" => {
            if entry.pkg_config.is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] pkg_config' is set more than once"
                    ),
                    at,
                    "",
                ));
            }
            let Some(package) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] pkg_config' must be a double-quoted string"
                    ),
                    at,
                    "",
                ));
            };
            if package.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] pkg_config' must not be empty"
                    ),
                    at,
                    "",
                ));
            }
            // The name is passed as one `pkg-config` argument, so only the
            // characters a `.pc` package name uses are accepted.
            if !package
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
            {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] pkg_config' must contain only letters, digits and '._+-'"
                    ),
                    at,
                    "",
                ));
            }
            entry.pkg_config = Some(package.to_string());
        }
        "symbols" => {
            let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] symbols' must be a bracketed list of double-quoted names, found '{value}'"
                    ),
                    at,
                    "",
                ));
            };
            let inner = inner.trim();
            let mut symbols = Vec::new();
            let mut kelp_names = Vec::new();
            if !inner.is_empty() {
                for part in inner.split(',') {
                    let part = part.trim();
                    let Some(sym) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"'))
                    else {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME} symbol for '[extern.\"{module_id}\"] symbols' must be double-quoted, found '{part}'"
                            ),
                            at,
                            "",
                        ));
                    };
                    // A plain entry binds the Kelp name to the symbol of the
                    // same name; `"kelp=c_symbol"` binds one to the other.
                    let (kelp, symbol) = sym.split_once('=').unwrap_or((sym, sym));
                    if kelp.is_empty() || symbol.is_empty() {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME} symbol for '[extern.\"{module_id}\"] symbols' must name a Kelp function and a C symbol on either side of '=', found '{sym}'"
                            ),
                            at,
                            "",
                        ));
                    }
                    if kelp.chars().any(char::is_whitespace) || symbol.chars().any(char::is_whitespace)
                    {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME} symbol for '[extern.\"{module_id}\"] symbols' must not contain whitespace, found '{sym}'"
                            ),
                            at,
                            "",
                        ));
                    }
                    if !is_ident(symbol) {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME} C symbol for '[extern.\"{module_id}\"] symbols' must be a C identifier, found '{symbol}'"
                            ),
                            at,
                            "",
                        ));
                    }
                    if kelp_names.contains(&kelp) {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME} symbol for '[extern.\"{module_id}\"] symbols' lists the Kelp function '{kelp}' more than once"
                            ),
                            at,
                            "",
                        ));
                    }
                    kelp_names.push(kelp);
                    symbols.push(sym.to_string());
                }
            }
            entry.symbols = symbols;
        }
        other => {
            return Err(Diagnostic::new(
                format!(
                    "unknown key '{other}' in {MANIFEST_NAME}'s '[extern.\"{module_id}\"]' section"
                ),
                at,
                "",
            ));
        }
    }
    Ok(())
}

/// The `library_dirs` value of a `[extern."<module id>"]`
/// section: a bracketed list of double-quoted paths, kept as written (the
/// relative ones are resolved against the manifest's directory when
/// `collect_extern_dirs` reads them). Each entry becomes a `-I<dir>` or
/// `-L<dir>` flag, so an empty entry, a quote, a newline or a NUL is refused,
/// and a leading '-' is refused because it would read as the start of another
/// flag rather than as a directory.
pub(crate) fn parse_dir_list(
    value: &str,
    module_id: &str,
    key: &str,
    at: Span,
) -> Result<Vec<String>, Diagnostic> {
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for '[extern.\"{module_id}\"] {key}' must be a bracketed list of double-quoted paths, found '{value}'"
            ),
            at,
            "",
        ));
    };
    let inner = inner.trim();
    let mut dirs = Vec::new();
    if !inner.is_empty() {
        for part in inner.split(',') {
            let part = part.trim();
            let Some(dir) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} path for '[extern.\"{module_id}\"] {key}' must be double-quoted, found '{part}'"
                    ),
                    at,
                    "",
                ));
            };
            if dir.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} path for '[extern.\"{module_id}\"] {key}' must not be empty"
                    ),
                    at,
                    "",
                ));
            }
            if dir.contains('"') || dir.contains('\n') || dir.contains('\r') || dir.contains('\0') {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} path for '[extern.\"{module_id}\"] {key}' must not contain '\"', a newline or NUL, found '{dir}'"
                    ),
                    at,
                    "",
                ));
            }
            if dir.starts_with('-') {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} path for '[extern.\"{module_id}\"] {key}' must not start with '-', found '{dir}'"
                    ),
                    at,
                    "",
                ));
            }
            dirs.push(dir.to_string());
        }
    }
    Ok(dirs)
}

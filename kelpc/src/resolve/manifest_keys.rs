use super::*;

/// One `domain = ["pattern", ...]` line inside `coral.toml`'s `[caps]` section
/// (C-2), or inside a `[caps."<module id>"]` section (#301). Reuses
/// `stdlib::is_domain` and `validate_cap_pattern`, the same vocabulary an
/// `impl "..." caps(domain = [...])` clause validates its grants against, so a
/// pattern that would be refused on an `impl` line is refused here too, for the
/// same reason and with the same wording. `header` is the section as the
/// manifest spells it (`[caps]`, `[caps."util"]`), for the messages.
fn parse_domain_line(line: &str, at: Span, header: &str) -> Result<(String, Vec<String>), Diagnostic> {
    let Some((domain, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'domain = [\"pattern\", ...]' in {MANIFEST_NAME}'s {header} section, found '{line}'"),
            at,
            "",
        ));
    };
    let domain = domain.trim();
    let value = value.trim();
    if !crate::stdlib::is_domain(domain) {
        return Err(Diagnostic::new(
            format!(
                "'{domain}' is not a capability domain - v1 has {}",
                crate::stdlib::DOMAINS.join(", ")
            ),
            at,
            "",
        ));
    }
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for '{header} {domain}' must be a bracketed list of double-quoted patterns, found '{value}'"
            ),
            at,
            "",
        ));
    };
    let mut patterns = Vec::new();
    let inner = inner.trim();
    if !inner.is_empty() {
        for part in inner.split(',') {
            let part = part.trim();
            let Some(pattern) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} pattern for '{header} {domain}' must be double-quoted, found '{part}'"
                    ),
                    at,
                    "",
                ));
            };
            patterns.push(pattern.to_string());
        }
    }
    validate_cap_grant(domain, &patterns, at, "")?;
    Ok((domain.to_string(), patterns))
}

/// One line inside `coral.toml`'s `[caps]` section (C-2): the entry module's
/// own grant.
pub(crate) fn parse_caps_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let (domain, patterns) = parse_domain_line(line, at, "[caps]")?;
    let caps = manifest.caps.get_or_insert_with(HashMap::new);
    if caps.contains_key(&domain) {
        return Err(Diagnostic::new(
            format!("domain '{domain}' appears more than once in {MANIFEST_NAME}'s [caps] section"),
            at,
            "",
        ));
    }
    caps.insert(domain, patterns);
    Ok(())
}

/// One line inside `coral.toml`'s `[caps."<module id>"]` section (#301): the
/// grant the manifest gives that module, whichever file imports it.
pub(crate) fn parse_module_caps_line(
    manifest: &mut Manifest,
    module_id: &str,
    line: &str,
    at: Span,
) -> Result<(), Diagnostic> {
    let header = format!("[caps.\"{module_id}\"]");
    let (domain, patterns) = parse_domain_line(line, at, &header)?;
    let caps = manifest.module_caps.entry(module_id.to_string()).or_default();
    if caps.contains_key(&domain) {
        return Err(Diagnostic::new(
            format!("domain '{domain}' appears more than once in {MANIFEST_NAME}'s {header} section"),
            at,
            "",
        ));
    }
    caps.insert(domain, patterns);
    Ok(())
}

/// One `modules = ["<module id>", ...]` line inside `coral.toml`'s `[unsafe]`
/// section (MK9-S2 stage 1). The list is the set of modules allowed to contain
/// `unsafe`, spelled by the same module ids the `[extern.*]` sections use.
pub(crate) fn parse_unsafe_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!(
                "expected 'modules = [\"<module id>\", ...]' in {MANIFEST_NAME}'s [unsafe] section, found '{line}'"
            ),
            at,
            "",
        ));
    };
    if key.trim() != "modules" {
        return Err(Diagnostic::new(
            format!(
                "unknown key '{}' in {MANIFEST_NAME}'s [unsafe] section: the section has one key, 'modules'",
                key.trim()
            ),
            at,
            "",
        ));
    }
    let value = value.trim();
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for '[unsafe] modules' must be a bracketed list of double-quoted module ids, found '{value}'"
            ),
            at,
            "",
        ));
    };
    let inner = inner.trim();
    if !inner.is_empty() {
        for part in inner.split(',') {
            let part = part.trim();
            let Some(module) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} module id in '[unsafe] modules' must be double-quoted, found '{part}'"
                    ),
                    at,
                    "",
                ));
            };
            if manifest.unsafe_modules.iter().any(|m| m == module) {
                return Err(Diagnostic::new(
                    format!(
                        "module '{module}' appears more than once in {MANIFEST_NAME}'s '[unsafe] modules' list"
                    ),
                    at,
                    "",
                ));
            }
            manifest.unsafe_modules.push(module.to_string());
        }
    }
    Ok(())
}

/// One `incremental = true|false` line inside `coral.toml`'s `[build]` section
/// (roadmap A-14 slice 1). A bare boolean, because that is what the key is: a
/// switch the backend reads, not a value anyone quotes. The section has one key
/// today, so any other key is refused by name rather than skipped.
pub(crate) fn parse_build_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'incremental = true|false', 'sanitize = \"address\"', 'emit = \"binary\"|\"html\"' or 'obfuscate = \"off\"|\"names\"' in {MANIFEST_NAME}'s [build] section, found '{line}'"),
            at,
            "",
        ));
    };
    if key.trim() == "emit_entry" {
        let value = value.trim();
        let name = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or("");
        if name.is_empty() {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[build] emit_entry' must be a double-quoted file name under src/, found '{value}'"
                ),
                at,
                "",
            ));
        }
        manifest.emit_entry = Some(name.to_string());
        return Ok(());
    }
    if key.trim() == "emit" {
        let value = value.trim();
        let name = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or("");
        manifest.emit = match name {
            "binary" => Emit::Binary,
            "html" => Emit::Html,
            _ => {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[build] emit' must be \"binary\" or \"html\" (double-quoted), found '{value}'"
                    ),
                    at,
                    "",
                ))
            }
        };
        return Ok(());
    }
    if key.trim() == "sanitize" {
        let value = value.trim();
        let name = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or("");
        manifest.sanitize = Some(crate::driver::Sanitizer::parse(name).ok_or_else(|| {
            Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[build] sanitize' must be \"address\", \"undefined\" or \"thread\" (double-quoted), found '{value}'"
                ),
                at,
                "",
            )
        })?);
        return Ok(());
    }
    if key.trim() == "obfuscate" {
        let value = value.trim();
        let name = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or("");
        manifest.obfuscate = match name {
            "off" => Obfuscate::Off,
            "names" => Obfuscate::Names,
            _ => {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[build] obfuscate' must be \"off\" or \"names\" (double-quoted), found '{value}'"
                    ),
                    at,
                    "",
                ))
            }
        };
        return Ok(());
    }
    if key.trim() != "incremental" {
        return Err(Diagnostic::new(
            format!(
                "unknown key '{}' in {MANIFEST_NAME}'s [build] section: its keys are 'incremental', 'sanitize', 'emit', 'emit_entry' and 'obfuscate'",
                key.trim()
            ),
            at,
            "",
        ));
    }
    manifest.incremental = match value.trim() {
        "true" => true,
        "false" => false,
        other => {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[build] incremental' must be true or false, found '{other}'"
                ),
                at,
                "",
            ))
        }
    };
    Ok(())
}

/// One `key = value` line inside `coral.toml`'s `[target]` section (kernel
/// stage 4): `mode`, `triple`, `entry`, `linker_script` or `panic`, each a
/// double-quoted string, plus `cpus` (kernel stage 8) and `heap_max` (managed
/// mode), bare integers. The
/// section's completeness is checked once, after the whole file is read, so an
/// out-of-order line is not an error.
pub(crate) fn parse_target_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!(
                "expected 'key = \"value\"' in {MANIFEST_NAME}'s [target] section, found '{line}'"
            ),
            at,
            "",
        ));
    };
    let key = key.trim();
    let value = value.trim();
    // `cpus` is the one numeric key: a hart count is a number, and writing
    // `"4"` for it would be silly (the same reasoning as the program-level
    // `http_pool`, S4).
    if key == "cpus" {
        if manifest.target_cpus_set {
            return Err(Diagnostic::new(
                format!("'cpus' appears more than once in {MANIFEST_NAME}'s [target] section"),
                at,
                "",
            ));
        }
        let Ok(n) = value.parse::<u64>() else {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[target] cpus' must be an integer between 1 and 64, found '{value}'"
                ),
                at,
                "",
            ));
        };
        if !(1..=64).contains(&n) {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[target] cpus' must be between 1 and 64, found {n}"
                ),
                at,
                "",
            ));
        }
        manifest.target.cpus = n;
        manifest.target_cpus_set = true;
        return Ok(());
    }
    // `heap_max` is the managed heap's cap in MiB, a bare integer like `cpus`.
    // The bound is `managed::HEAP_MAX_MIB_RANGE`, so a typo cannot ask for a heap
    // the host does not have, and a zero cap is refused rather than read as 256.
    if key == "heap_max" {
        if manifest.target.heap_max.is_some() {
            return Err(Diagnostic::new(
                format!("'heap_max' appears more than once in {MANIFEST_NAME}'s [target] section"),
                at,
                "",
            ));
        }
        let range = crate::managed::HEAP_MAX_MIB_RANGE;
        let Ok(n) = value.parse::<u64>() else {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[target] heap_max' must be a whole number of MiB between {} and {}, found '{value}'",
                    range.start(),
                    range.end()
                ),
                at,
                "",
            ));
        };
        if !range.contains(&n) {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME} value for '[target] heap_max' must be between {} and {} MiB, found {n}",
                    range.start(),
                    range.end()
                ),
                at,
                "",
            ));
        }
        manifest.target.heap_max = Some(n);
        return Ok(());
    }
    let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for '[target] {key}' must be a double-quoted string"
            ),
            at,
            "",
        ));
    };
    match key {
        "mode" => {
            if manifest.target_mode_set {
                return Err(Diagnostic::new(
                    format!("'mode' appears more than once in {MANIFEST_NAME}'s [target] section"),
                    at,
                    "",
                ));
            }
            manifest.target.mode = match value {
                "hosted" => TargetMode::Hosted,
                "freestanding" => TargetMode::Freestanding,
                "managed" => {
                    return Err(Diagnostic::new(
                        format!(
                            "{MANIFEST_NAME}'s '[target] mode = \"managed\"' was removed: delete the mode line. Every hosted build has the shared pool, and blocks are written inline in any hosted file"
                        ),
                        at,
                        "",
                    ))
                }
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "unknown '{other}' for {MANIFEST_NAME}'s '[target] mode': it is 'hosted' or 'freestanding'"
                        ),
                        at,
                        "",
                    ))
                }
            };
            manifest.target_mode_set = true;
        }
        "triple" => set_target_key(&mut manifest.target.triple, value, "triple", at)?,
        "entry" => {
            if !is_bare_ident(value) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[target] entry' must be a bare function name, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            set_target_key(&mut manifest.target.entry, value, "entry", at)?;
        }
        "linker_script" => {
            set_target_key(&mut manifest.target.linker_script, value, "linker_script", at)?;
        }
        "panic" => {
            if !is_bare_ident(value) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[target] panic' must be a bare function name, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            set_target_key(&mut manifest.target.panic, value, "panic", at)?;
        }
        // The global allocator (kernel stage 7): `alloc` and `dealloc` name the
        // two functions every container allocation and free calls instead of
        // libc's, and `realloc` is the optional growth fast path.
        "alloc" => {
            if !is_bare_ident(value) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[target] alloc' must be a bare function name, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            set_target_key(&mut manifest.target.alloc, value, "alloc", at)?;
        }
        "dealloc" => {
            if !is_bare_ident(value) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[target] dealloc' must be a bare function name, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            set_target_key(&mut manifest.target.dealloc, value, "dealloc", at)?;
        }
        "realloc" => {
            if !is_bare_ident(value) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for '[target] realloc' must be a bare function name, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            set_target_key(&mut manifest.target.realloc, value, "realloc", at)?;
        }
        other => {
            return Err(Diagnostic::new(
                format!(
                    "unknown key '{other}' in {MANIFEST_NAME}'s [target] section: it is 'mode', 'triple', 'entry', 'linker_script', 'panic', 'alloc', 'dealloc', 'realloc', 'cpus' or 'heap_max'"
                ),
                at,
                "",
            ))
        }
    }
    Ok(())
}

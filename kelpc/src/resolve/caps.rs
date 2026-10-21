use super::*;

/// Whether a pattern the *holder* has covers a pattern it wants to delegate.
///
/// Deliberately syntactic and conservative: either the two patterns are
/// identical, or the held one ends in `/**` and the delegated one is inside
/// that root. Glob-to-glob inclusion is undecidable in general, and being
/// conservative here can only under-grant - never let a module hand out more
/// than it holds. `net` and `env` are name domains: the only pattern that
/// covers another one is the whole-resource `*`.
pub(crate) fn pattern_covered(held: &str, granted: &str, domain: &str) -> bool {
    if held == granted {
        return true;
    }
    if !crate::stdlib::patterns_are_paths(domain) {
        return held == "*";
    }
    match held.strip_suffix("/**") {
        Some(root) => granted == root || granted.starts_with(&format!("{root}/")),
        None => false,
    }
}

/// Whether `module` reaches a capability domain at all: it *is* a gated module,
/// or some module in its import closure is.
///
/// A module that reaches none has nothing to be granted, which is what makes
/// `caps(*)` on it a refusal (`syntax.md` 15.2) rather than a no-op. The
/// closure is over `impl` targets, so an ungated wrapper counts as reaching a
/// domain through what it imports (`std/http` holds `net`), while a module made
/// of arithmetic and byte shuffling (`std/str`, `std/math`) does not.
pub(crate) fn reaches_a_domain(
    modules: &[ModuleInfo],
    module: &str,
    extern_modules: &HashMap<String, ExternModuleDecl>,
) -> bool {
    let by_id: HashMap<&str, &ModuleInfo> = modules.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut queue = vec![module.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if domain_of_with_extern(&id, extern_modules).is_some() {
            return true;
        }
        if let Some(m) = by_id.get(id.as_str()) {
            queue.extend(m.aliases.values().cloned());
        }
    }
    false
}

/// A whole `caps(<domain> = [...])` grant, an `impl` clause's or a `[caps]`
/// line's: every pattern through `validate_cap_pattern`, plus the one domain
/// whose pattern list is not a vocabulary at all.
///
/// `ui` is that domain (gap 6, `kelp wiki page libraries/polyp.md`): a display call
/// carries no argument that names the display server, so no pattern narrower
/// than `*` could be matched at a call site, and accepting one would read as
/// an enforced grant while denying nothing. Only `["*"]` is taken, and any
/// other list is refused here rather than compiled into an allowlist nothing
/// consults.
pub(crate) fn validate_cap_grant(
    domain: &str,
    patterns: &[String],
    span: Span,
    module: &str,
) -> Result<(), Diagnostic> {
    if domain == "ui" && !(patterns.len() == 1 && patterns[0] == "*") {
        return Err(Diagnostic::new(
            "patterns are not enforceable in the 'ui' domain yet: a display call carries no argument to match a grant against, so only `ui = [\"*\"]` is accepted (see plan/ffi-display-domain.md)"
                .to_string(),
            span,
            module,
        ));
    }
    if domain == "mmio" || domain == "irq" || domain == "frames" {
        return validate_range_grant(domain, patterns, span, module);
    }
    for pattern in patterns {
        validate_cap_pattern(domain, pattern, span, module)?;
    }
    Ok(())
}

/// The grant vocabulary of the three range domains, `mmio` and `frames`
/// (`"0xBASE+0xLEN"`) and `irq` (a decimal line or inclusive line range), or
/// the whole resource as `["*"]` alone. A grant is validated here so that a
/// module's resource set is a set of ranges the checks can test a value
/// against without parsing again, and so a malformed or overlapping grant is a
/// compile error at the `impl` line or the manifest line that wrote it rather
/// than a surprise at the one call that reaches outside it
/// (`kelp wiki page language/capabilities.md`, `kelp wiki page stdlib/irq.md`).
pub(crate) fn validate_range_grant(
    domain: &str,
    patterns: &[String],
    span: Span,
    module: &str,
) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if patterns.iter().any(|p| p == crate::stdlib::RANGE_ANY) {
        if patterns.len() != 1 {
            return err(format!(
                "`{domain} = [\"*\"]` grants {} and cannot be written beside a range; keep the wildcard alone, or name the ranges it needs",
                whole_range_phrase(domain)
            ));
        }
        return Ok(());
    }
    let mut parsed: Vec<((u128, u128), &str)> = Vec::new();
    for pattern in patterns {
        let range = crate::stdlib::parse_range(domain, pattern)
            .map_err(|message| Diagnostic::new(message, span, module))?;
        for (earlier, earlier_pattern) in &parsed {
            if crate::stdlib::ranges_overlap(*earlier, range) {
                return err(format!(
                    "the {domain} ranges '{earlier_pattern}' and '{pattern}' overlap - a grant names each range once, and the ranges in it must be disjoint"
                ));
            }
        }
        parsed.push((range, pattern.as_str()));
    }
    Ok(())
}

/// What a range domain's wildcard covers, for the message that refuses it
/// beside a range. One phrase per domain, because "the whole address space"
/// would be wrong for an interrupt line.
pub(crate) fn whole_range_phrase(domain: &str) -> &'static str {
    match domain {
        "irq" => "every interrupt line",
        "frames" => "every physical frame",
        _ => "the whole address space",
    }
}

/// A capability pattern, validated by what the domain actually names.
///
/// `fs` (and `net`/`process`) name paths, so a pattern is an absolute path,
/// glob-only: `*` inside one segment, `**` as a whole segment, and no `..`
/// anywhere - a pattern that could name its way out of its own root is
/// rejected at the `impl` line, at compile time. `env` names variables, so
/// its patterns are names (`HOME`, `LANG*`, or `*` for any). `time` and
/// `rand` have no named resource at all, so their pattern list is unused and
/// is normally empty.
pub(crate) fn validate_cap_pattern(
    domain: &str,
    pattern: &str,
    span: Span,
    module: &str,
) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if pattern.is_empty() {
        return err(format!("a '{domain}' pattern must not be empty"));
    }
    if domain == "net" {
        // Anchored here so the net vocabulary is validated in one place.
        return validate_net_pattern(pattern)
            .map_err(|message| Diagnostic::new(message, span, module));
    }
    if !crate::stdlib::patterns_are_paths(domain) {
        if domain == "env" && pattern.contains('/') {
            return err(format!(
                "'{pattern}' is not a variable name - 'env' patterns name variables ('HOME', 'LANG*', or '*' for any)"
            ));
        }
        return Ok(());
    }
    if !pattern.starts_with('/') {
        return err(format!(
            "capability patterns are absolute paths - '{pattern}' must start with '/'"
        ));
    }
    for seg in pattern.split('/') {
        if seg == ".." {
            return err(format!(
                "capability pattern '{pattern}' contains a '..' segment - a grant must not be able to name its way out of its own root"
            ));
        }
        if seg.contains("**") && seg != "**" {
            return err(format!(
                "'**' is only legal as a whole segment in '{pattern}' ('*' matches within one segment)"
            ));
        }
    }
    Ok(())
}

/// `net`'s vocabulary: `host`, `host:port`, `*.host`, `*:port`, `*`. The host
/// is split from the port at the **last** `:` - so a host can never contain
/// one, which makes an IPv6 literal unexpressible rather than ambiguous, and
/// the message says so. `*` matches within one label (it does not cross `.`
/// or `:`); a lone `*` is the whole-resource wildcard the entry module holds.
pub(crate) fn validate_net_pattern(pattern: &str) -> Result<(), String> {
    let vocabulary = "a 'net' pattern names a host and an optional port \
        ('api.example.com:443', '*.example.com', '*:8080', or '*')";
    if pattern.contains('/') || pattern.contains(char::is_whitespace) {
        return Err(format!("'{pattern}' is not a host - {vocabulary}"));
    }
    if pattern.contains("**") {
        return Err(format!(
            "'**' has no meaning in a 'net' pattern - write '*' ('{pattern}')"
        ));
    }
    let (host, port) = match pattern.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (pattern, None),
    };
    if host.is_empty() {
        return Err(format!("'{pattern}' has no host - {vocabulary}"));
    }
    if host.contains(':') {
        return Err(format!(
            "'{pattern}' has more than one ':' - an IPv6 literal is not expressible as a 'net' pattern, because the ':' is what separates the port ({vocabulary})"
        ));
    }
    if host != "*" && (host.starts_with('.') || host.ends_with('.') || host.contains("..")) {
        return Err(format!(
            "'{pattern}' is not a host name - no label of it may be empty"
        ));
    }
    if let Some(port) = port {
        if port.is_empty() {
            return Err(format!(
                "'{pattern}' names a port after ':' - write the number, or '*' for any ({vocabulary})"
            ));
        }
        if port != "*" {
            if !port.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("the port in '{pattern}' must be a number or '*'"));
            }
            if port.parse::<u32>().map(|p| p > 65535).unwrap_or(true) {
                return Err(format!("the port in '{pattern}' must be 0-65535"));
            }
        }
    }
    Ok(())
}

/// `impl` paths are `src/`-relative, identifier-only, and may not escape the
/// project. `std/...` is reserved for compiler-shipped modules (M6/M7).
pub(crate) fn validate_impl_path(path: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if path.is_empty() {
        return err("'impl' path must not be empty".to_string());
    }
    let ext = format!(".{EXT}");
    if path.ends_with(&ext) {
        return err(format!(
            "'impl' paths omit the '.{EXT}' extension - write \"{}\"",
            path.trim_end_matches(&ext)
        ));
    }
    if path.starts_with('/') {
        return err("'impl' paths are relative to 'src/' and must not start with '/'".to_string());
    }
    if path.contains('\\') {
        return err("use '/' as the separator in 'impl' paths".to_string());
    }
    if path.starts_with(INTERNAL_STDLIB_PREFIX) {
        return err(format!(
            "'impl \"{path}\"' names the compiler-reserved std/internal namespace"
        ));
    }
    if path == "std" || path.starts_with("std/") {
        // `std/mem` and friends ship with the compiler (M6.2). A `std/...`
        // module that is not shipped yet is reported as such rather than as
        // a missing file, so the reservation stays visible.
        if crate::stdlib::source(path).is_some() {
            return Ok(());
        }
        return err(format!(
            "'impl \"{path}\"' - stdlib module '{}' is not implemented yet (shipped today: {}; the rest land in M6.3/M7)",
            path.trim_start_matches("std/"),
            crate::stdlib::shipped().join(", ")
        ));
    }
    for seg in path.split('/') {
        if seg.is_empty() {
            return err("'impl' path has an empty segment".to_string());
        }
        if seg == "." || seg == ".." {
            return err("'impl' paths must not contain '.' or '..' segments".to_string());
        }
        if !is_ident(seg) {
            return err(format!(
                "'{seg}' is not a valid module name - use snake_case identifiers with no dashes or dots"
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_alias(alias: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
    if !is_ident(alias) {
        return Err(Diagnostic::new(
            format!("'{alias}' is not a valid module alias"),
            span,
            module,
        ));
    }
    if is_keyword(alias) {
        return Err(Diagnostic::new(
            format!("'{alias}' is a language keyword and cannot be a module alias"),
            span,
            module,
        ));
    }
    Ok(())
}

pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "fun" | "val"
            | "var"
            | "mut"
            | "if"
            | "else"
            | "while"
            | "when"
            | "defer"
            | "return"
            | "true"
            | "false"
            | "datac"
            | "impl"
            | "as"
            | "pd"
            | "tl"
            | "pub"
            | "alias"
            | "print"
    )
}

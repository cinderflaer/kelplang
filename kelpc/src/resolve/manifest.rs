use super::*;

/// One `name = "path"` line of the `[alias]` section. The path is the module
/// an `impl "_name"` resolves to, so it must name a module below `src/`:
/// a relative path with no leading `/` and no `..` component. The name itself
/// is checked by `AliasTable::define`, which also reports a duplicate across
/// the whole project.
pub(crate) fn parse_alias_line(manifest: &mut Manifest, line: &str, line_offset: usize) -> Result<(), Diagnostic> {
    let at = Span::new(0, 0);
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'name = \"path\"' in {MANIFEST_NAME} [alias], found '{line}'"),
            at,
            "",
        ));
    };
    let name = key.trim();
    if manifest.alias.iter().any(|d| d.name == name) {
        return Err(Diagnostic::new(
            format!("alias '{name}' is already defined in {MANIFEST_NAME} [alias]"),
            at,
            "",
        ));
    }
    let Some(path) = value
        .trim()
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
    else {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME} [alias] value for '{name}' must be a double-quoted string"),
            at,
            "",
        ));
    };
    if path.is_empty() {
        return Err(Diagnostic::new(
            format!("alias '{name}' in {MANIFEST_NAME} [alias] has an empty path"),
            at,
            "",
        ));
    }
    if path.starts_with('/') {
        return Err(Diagnostic::new(
            format!(
                "alias '{name}' in {MANIFEST_NAME} [alias] must be a relative module path, found '{path}'"
            ),
            at,
            "",
        ));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(Diagnostic::new(
            format!("alias '{name}' in {MANIFEST_NAME} [alias] must not contain '..', found '{path}'"),
            at,
            "",
        ));
    }
    // Where the path's first byte sits in the manifest: after the `=`, past
    // the spaces, and one more byte for the opening quote.
    let after_eq = line.find('=').map_or(0, |i| i + 1);
    let value_start = after_eq + (line[after_eq..].len() - line[after_eq..].trim_start().len());
    manifest.alias.push(preprocess::AliasDef {
        name: name.to_string(),
        text: path.to_string(),
        text_offset: line_offset + value_start + 1,
        module: preprocess::MANIFEST_MODULE.to_string(),
        span: Span::new(line_offset, line_offset + line.len()),
    });
    Ok(())
}

pub(crate) fn parse_manifest(text: &str, path: &Path) -> Result<Manifest, Diagnostic> {
    let mut manifest = Manifest {
        name: None,
        version: None,
        entry: format!("src/main.{EXT}"),
        http_pool: crate::stdlib::DEFAULT_HTTP_POOL,
        naming_case: None,
        caps: None,
        extern_modules: HashMap::new(),
        unsafe_modules: Vec::new(),
        target_seen: false,
        target: TargetDecl::hosted(),
        target_mode_set: false,
        target_cpus_set: false,
        incremental: false,
        sanitize: None,
        emit: Emit::Binary,
        obfuscate: Obfuscate::Off,
        emit_entry: None,
        alias: Vec::new(),
        alias_source: None,
        dependencies: Vec::new(),
        tidepool: TidepoolDecl::default(),
    };
    let at = Span::new(0, 0);
    // `None` until the first `[section]` header; `Some("caps")` while inside
    // `[caps]`, `Some("extern.\"<module id>\"")` while inside a
    // `[extern."<module id>"]` section (MK9-S1-M1), `Some(<anything else>)`
    // while inside a section that is Coral's, not the compiler's, to read.
    let mut section: Option<String> = None;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // Where this line starts in the manifest, for the `[alias]` spans.
        // `line` is a sub-slice of `text` (`lines` and `split` only slice), so
        // the byte distance is the offset.
        let line_offset = line.as_ptr() as usize - text.as_ptr() as usize;
        // A `[section]` header hands the rest of that section to Coral:
        // `name`, `version`, `entry`, `http_pool` and `naming_case` must
        // appear before the first one. `[caps]` (C-2) and `[extern."<module
        // id>"]` (MK9-S1-M1, roadmap B-1) are the sections kelpc itself
        // reads - the entry module's own capability grant, and a user
        // module's own domain/library/symbol assignment - so they do not
        // simply stop here; every other section (`[tasks]`, `[dependencies]`,
        // …) is Coral's manifest surface, and this loop skips its lines
        // rather than rejecting them.
        if line.starts_with('[') {
            let name = line.trim_start_matches('[').trim_end_matches(']').trim();
            if let Some(rest) = name.strip_prefix("extern.") {
                let Some(module_id) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
                else {
                    return Err(Diagnostic::new(
                        format!(
                            "{MANIFEST_NAME} section header must be '[extern.\"<module id>\"]', found '[{name}]'"
                        ),
                        at,
                        "",
                    ));
                };
                if manifest.extern_modules.contains_key(module_id) {
                    return Err(Diagnostic::new(
                        format!(
                            "module '{module_id}' appears more than once in {MANIFEST_NAME}'s '[extern.*]' sections"
                        ),
                        at,
                        "",
                    ));
                }
                manifest
                    .extern_modules
                    .insert(module_id.to_string(), ExternModuleDecl::default());
            }
            // A header about Tidepool is refused by name: one that contains
            // `tidepool` in any case (`[target.tidepool]`, `[tidepool.x]`),
            // one that starts with `tid` (`[tidpool]`), the old name `[gc]`,
            // and the double-bracket `[[tidepool]]`, which is an array of
            // tables, not the one table. Skipping any of them would give every
            // Tidepool setting its default without a word. A `[extern."..."]`
            // header names a user module, so its id is not checked.
            let lower = name.to_ascii_lowercase();
            let about_tidepool = !name.starts_with("extern.")
                && (name == "gc" || lower.starts_with("tid") || lower.contains("tidepool"));
            let array_of_tables = line.starts_with("[[");
            if about_tidepool && (name != "tidepool" || array_of_tables) {
                let header = if array_of_tables { format!("[[{name}]]") } else { format!("[{name}]") };
                return Err(Diagnostic::new(
                    format!("unknown section '{header}' in {MANIFEST_NAME}; did you mean '[tidepool]'?"),
                    at,
                    "",
                ));
            }
            if name == "target" {
                manifest.target_seen = true;
            }
            section = Some(name.to_string());
            continue;
        }
        if let Some(name) = &section {
            if name == "caps" {
                parse_caps_line(&mut manifest, line, at)?;
            } else if let Some(rest) = name.strip_prefix("extern.") {
                if let Some(module_id) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    parse_extern_line(&mut manifest, module_id, line, at)?;
                }
            } else if name == "unsafe" {
                parse_unsafe_line(&mut manifest, line, at)?;
            } else if name == "target" {
                parse_target_line(&mut manifest, line, at)?;
            } else if name == "build" {
                parse_build_line(&mut manifest, line, at)?;
            } else if name == "alias" {
                parse_alias_line(&mut manifest, line, line_offset)?;
            } else if name == "dependencies" {
                parse_dependency_line(&mut manifest, line, at)?;
            } else if name == "tidepool" {
                parse_tidepool_line(&mut manifest, line, at)?;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(Diagnostic::new(
                format!("expected 'key = \"value\"' in {MANIFEST_NAME}, found '{line}'"),
                at,
                "",
            ));
        };
        let key = key.trim();
        let value = value.trim();
        // `http_pool` is the one numeric key (S4): a program-level connection
        // budget is a number, and writing `"4"` for it would be silly. Every
        // other key is still a double-quoted string, and an unknown key is
        // still an error rather than an ignored line.
        if key == "http_pool" {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for 'http_pool' must be a non-negative whole number, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            manifest.http_pool = value.parse::<usize>().map_err(|_| {
                Diagnostic::new(
                    format!("{MANIFEST_NAME} value for 'http_pool' is out of range: '{value}'"),
                    at,
                    "",
                )
            })?;
            continue;
        }
        // `naming_case` is the second numeric key, and the only one with a
        // closed set of values: it is `kelpc lint`'s setting (`syntax.md` 19),
        // so a number the option does not define is a typo rather than a
        // future option, and it is refused here where the file is read.
        if key == "naming_case" {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for 'naming_case' must be a non-negative whole number, found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            let option = value.parse::<usize>().map_err(|_| {
                Diagnostic::new(
                    format!("{MANIFEST_NAME} value for 'naming_case' is out of range: '{value}'"),
                    at,
                    "",
                )
            })?;
            if option > 1 {
                return Err(Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} value for 'naming_case' must be 0 (camelCase) or 1 (snake_case), found '{value}'"
                    ),
                    at,
                    "",
                ));
            }
            manifest.naming_case = Some(option);
            continue;
        }
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
        else {
            return Err(Diagnostic::new(
                format!("{MANIFEST_NAME} value for '{key}' must be a double-quoted string"),
                at,
                "",
            ));
        };
        match key {
            "name" => manifest.name = Some(value.to_string()),
            "version" => manifest.version = Some(value.to_string()),
            "entry" => manifest.entry = value.to_string(),
            other => {
                return Err(Diagnostic::new(
                    format!("unknown key '{other}' in {}", path.display()),
                    at,
                    "",
                ))
            }
        }
    }
    // A `[extern."<module id>"]` section that never set `domain` or
    // `library` is half-written: `domain_of_with_extern` would either not
    // gate the module at all (silently missing the whole point of B-1's
    // design) or gate it with no library to link its symbols against.
    // Refusing it here, once, is cheaper than teaching every later reader of
    // `extern_modules` to handle an incomplete entry.
    for (module_id, decl) in &manifest.extern_modules {
        if decl.domain.is_none() {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME}'s '[extern.\"{module_id}\"]' section is missing 'domain = \"<domain>\"'; v1 has {}",
                    crate::stdlib::DOMAINS.join(", ")
                ),
                at,
                "",
            ));
        }
        if decl.library.is_none() {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME}'s '[extern.\"{module_id}\"]' section is missing 'library = \"<name>\"'"
                ),
                at,
                "",
            ));
        }
    }
    // `[build] emit = "html"` builds `emit_entry` instead of `entry`, so it has to
    // name one. The check runs here, after every key, because the two keys may come
    // in either order.
    if manifest.emit == Emit::Html && manifest.emit_entry.is_none() {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME}'s [build] section sets emit = \"html\" but names no emit_entry; add emit_entry = \"<file under src/>\", the Polyp app whose main calls polyp.emit_html"
            ),
            at,
            "",
        ));
    }
    // A `[target]` section has to say which mode it is: the other four keys
    // mean different things in each, so a section that names only `triple`
    // would be silently hosted, which is the one outcome the section exists to
    // prevent. A freestanding mode also has to name all three of its required
    // keys; a hosted one may name none of the freestanding ones.
    // The mode is optional: a `[target]` section without one is hosted.
    if manifest.target_seen {
        match manifest.target.mode {
            TargetMode::Hosted => {
                for (key, value) in [
                    ("triple", manifest.target.triple.as_ref()),
                    ("entry", manifest.target.entry.as_ref()),
                    ("linker_script", manifest.target.linker_script.as_ref()),
                    ("panic", manifest.target.panic.as_ref()),
                    ("alloc", manifest.target.alloc.as_ref()),
                    ("dealloc", manifest.target.dealloc.as_ref()),
                    ("realloc", manifest.target.realloc.as_ref()),
                ] {
                    if value.is_some() {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME}'s [target] key '{key}' is only meaningful with 'mode = \"freestanding\"'"
                            ),
                            at,
                            "",
                        ));
                    }
                }
            }
            TargetMode::Freestanding => {
                if manifest.target.heap_max.is_some() {
                    return Err(Diagnostic::new(
                        format!(
                            "{MANIFEST_NAME}'s [target] key 'heap_max' is only meaningful with 'mode = \"hosted\"', because a freestanding build has no shared pool"
                        ),
                        at,
                        "",
                    ));
                }
                for (key, value) in [
                    ("triple", manifest.target.triple.as_ref()),
                    ("entry", manifest.target.entry.as_ref()),
                    ("linker_script", manifest.target.linker_script.as_ref()),
                ] {
                    if value.map(|v| v.is_empty()).unwrap_or(true) {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME}'s [target] section with 'mode = \"freestanding\"' is missing '{key} = \"<value>\"'"
                            ),
                            at,
                            "",
                        ));
                    }
                }
                // The allocator hook (kernel stage 7): `alloc` and `dealloc`
                // are a pair; either alone leaves a container that can
                // allocate but never free, so the section is refused rather
                // than half-honoured. `realloc` needs the pair it accelerates.
                match (
                    manifest.target.alloc.is_some(),
                    manifest.target.dealloc.is_some(),
                ) {
                    (true, false) => {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME}'s [target] section names 'alloc' without 'dealloc': the allocator hook requires both, because a container that allocates must be able to free"
                            ),
                            at,
                            "",
                        ));
                    }
                    (false, true) => {
                        return Err(Diagnostic::new(
                            format!(
                                "{MANIFEST_NAME}'s [target] section names 'dealloc' without 'alloc': the allocator hook requires both, because a container that allocates must be able to free"
                            ),
                            at,
                            "",
                        ));
                    }
                    _ => {}
                }
                if manifest.target.realloc.is_some() && manifest.target.alloc.is_none() {
                    return Err(Diagnostic::new(
                        format!(
                            "{MANIFEST_NAME}'s [target] section names 'realloc' without 'alloc' and 'dealloc': the allocator hook requires the pair first"
                        ),
                        at,
                        "",
                    ));
                }
            }
        }
    }
    if !manifest.alias.is_empty() {
        manifest.alias_source = Some((path.to_path_buf(), text.to_string()));
    }
    // `start_high_mib` is checked against `heap_max` here, after the whole
    // file is read, because `[target]` may come after `[tidepool]`. A larger
    // start guess would set a first trigger the shared pool can never reach.
    if let Some(high) = manifest.tidepool.start_high_mib {
        let heap_max = manifest.target.heap_max_mib();
        if high > heap_max {
            return Err(Diagnostic::new(
                format!(
                    "{MANIFEST_NAME}'s [tidepool] key 'start_high_mib' must not exceed [target] heap_max ({heap_max} MiB), found '{high}'"
                ),
                at,
                "",
            ));
        }
    }
    // A reversed start pair is refused by name (design D3, G16, test T84). The
    // pair is compared as it takes effect, so an unset key counts as its default.
    let tidepool = &manifest.tidepool;
    let (low, high) = (tidepool.start_low_mib_or_default(), tidepool.start_high_mib_or_default());
    if high < low {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME}'s [tidepool] key 'start_high_mib' must be at least 'start_low_mib', which is '{low}', found '{high}'"
            ),
            at,
            "",
        ));
    }
    // A freestanding build has no shared pool, so a key in `[tidepool]` would
    // do nothing. An empty header is harmless and is still accepted.
    if manifest.target.is_freestanding() && !manifest.tidepool.is_unset() {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME}'s [tidepool] section is only meaningful with 'mode = \"hosted\"', because a freestanding build has no shared pool"
            ),
            at,
            "",
        ));
    }
    // The runtime reads the section through the target, next to heap_max.
    manifest.target.tidepool = std::mem::take(&mut manifest.tidepool);
    Ok(manifest)
}

/// `trigger_rule` in `[tidepool]` (design section 4, Challenge 2): the live
/// size that sets each later trigger. Its ABI code is what `kelp_gc_init`
/// receives, and the codes match `kelp_gc::trigger::Rule`.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerRule {
    /// The larger of the last two measured live sizes (the default, D4).
    #[default]
    TwoSize,
    /// The last measured live size alone.
    LastSize,
    /// The last measured live size plus the stack and global bytes scanned with it.
    GogcStyle,
}

impl TriggerRule {
    /// The names a manifest may give, in the order the error message lists them.
    pub const NAMES: [&'static str; 3] = ["two-size", "last-size", "gogc-style"];

    /// The rule a manifest name selects, or `None` for any other name.
    pub fn from_name(name: &str) -> Option<TriggerRule> {
        match name {
            "two-size" => Some(TriggerRule::TwoSize),
            "last-size" => Some(TriggerRule::LastSize),
            "gogc-style" => Some(TriggerRule::GogcStyle),
            _ => None,
        }
    }

    /// The ABI code `kelp_gc_init` receives for this rule.
    pub fn code(self) -> u64 {
        match self {
            TriggerRule::TwoSize => 0,
            TriggerRule::LastSize => 1,
            TriggerRule::GogcStyle => 2,
        }
    }
}

/// `[tidepool]` (Tidepool step 2, design section 14.2 and 14.4): the first
/// clearing's two start guesses and the headroom percent. Each is a bare
/// whole number of MiB (or percent). `None` takes the default.
#[derive(Default, Debug, Clone)]
pub struct TidepoolDecl {
    /// `headroom_pct`, from 0 to 1000.
    pub headroom_pct: Option<u64>,
    /// `start_low_mib`, at least 1.
    pub start_low_mib: Option<u64>,
    /// `start_high_mib`, at least 1 and no more than `heap_max`.
    pub start_high_mib: Option<u64>,
    /// `trigger_rule`, a double-quoted name. `None` takes the two-size default.
    pub trigger_rule: Option<TriggerRule>,
}

impl TidepoolDecl {
    /// Whether the section names no key at all (an empty `[tidepool]`).
    pub fn is_unset(&self) -> bool {
        self.headroom_pct.is_none()
            && self.start_low_mib.is_none()
            && self.start_high_mib.is_none()
            && self.trigger_rule.is_none()
    }

    /// `start_low_mib`, or the default when the section names none.
    pub fn start_low_mib_or_default(&self) -> u64 {
        self.start_low_mib.unwrap_or(crate::managed::DEFAULT_START_LOW_MIB)
    }

    /// `start_high_mib`, or the default when the section names none.
    pub fn start_high_mib_or_default(&self) -> u64 {
        self.start_high_mib.unwrap_or(crate::managed::DEFAULT_START_HIGH_MIB)
    }

    /// `headroom_pct`, or the default when the section names none.
    pub fn headroom_pct_or_default(&self) -> u64 {
        self.headroom_pct.unwrap_or(crate::managed::DEFAULT_HEADROOM_PCT)
    }

    /// `trigger_rule`, or the two-size default when the section names none.
    pub fn trigger_rule_or_default(&self) -> TriggerRule {
        self.trigger_rule.unwrap_or_default()
    }
}

/// Parse `trigger_rule = "<name>"` in `[tidepool]`. The value is a
/// double-quoted name, one of `TriggerRule::NAMES`; any other name is refused
/// by name, with the names it may take.
pub(crate) fn parse_trigger_rule_line(manifest: &mut Manifest, value: &str, at: Span) -> Result<(), Diagnostic> {
    let key = "trigger_rule";
    if manifest.tidepool.trigger_rule.is_some() {
        return Err(Diagnostic::new(
            format!("'{key}' appears more than once in {MANIFEST_NAME}'s [tidepool] section"),
            at,
            "",
        ));
    }
    let Some(name) = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key '{key}' must be a double-quoted rule name, found '{value}'"),
            at,
            "",
        ));
    };
    let Some(rule) = TriggerRule::from_name(name) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME}'s [tidepool] key '{key}' names no rule '{name}'; its rules are '{}', '{}' and '{}'",
                TriggerRule::NAMES[0],
                TriggerRule::NAMES[1],
                TriggerRule::NAMES[2]
            ),
            at,
            "",
        ));
    };
    manifest.tidepool.trigger_rule = Some(rule);
    Ok(())
}

/// Parse one `key = <whole number>` line of `[tidepool]`. Every key takes a
/// bare number, so a quoted value is refused, and `heap_max` is refused with
/// the section it belongs in.
pub(crate) fn parse_tidepool_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'key = <whole number>' in {MANIFEST_NAME}'s [tidepool] section, found '{line}'"),
            at,
            "",
        ));
    };
    let key = key.trim();
    let value = value.trim();
    if key == "heap_max" {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key 'heap_max' belongs in [target]; move it there"),
            at,
            "",
        ));
    }
    if key == "trigger_rule" {
        return parse_trigger_rule_line(manifest, value, at);
    }
    let (slot, min, max) = match key {
        "headroom_pct" => (&mut manifest.tidepool.headroom_pct, 0u64, 1000u64),
        "start_low_mib" => (&mut manifest.tidepool.start_low_mib, 1, u64::MAX),
        "start_high_mib" => (&mut manifest.tidepool.start_high_mib, 1, u64::MAX),
        other => {
            return Err(Diagnostic::new(
                format!(
                    "unknown key '{other}' in {MANIFEST_NAME}'s [tidepool] section; its keys are 'headroom_pct', 'start_low_mib', 'start_high_mib' and 'trigger_rule'"
                ),
                at,
                "",
            ))
        }
    };
    if slot.is_some() {
        return Err(Diagnostic::new(
            format!("'{key}' appears more than once in {MANIFEST_NAME}'s [tidepool] section"),
            at,
            "",
        ));
    }
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key '{key}' must be a whole number, found '{value}'"),
            at,
            "",
        ));
    }
    let Ok(n) = value.parse::<u64>() else {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key '{key}' is out of range: '{value}'"),
            at,
            "",
        ));
    };
    if n < min {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key '{key}' must be at least {min}, found '{value}'"),
            at,
            "",
        ));
    }
    if n > max {
        return Err(Diagnostic::new(
            format!("{MANIFEST_NAME}'s [tidepool] key '{key}' must be from {min} to {max}, found '{value}'"),
            at,
            "",
        ));
    }
    *slot = Some(n);
    Ok(())
}

pub(crate) struct Manifest {
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    /// Path relative to the project root; `kelpc build <dir>` uses it.
    pub(crate) entry: String,
    /// `http_pool = <n>` (S4): kept-alive HTTP connections per `host:port`.
    /// `0` disables pooling. Absent means `stdlib::DEFAULT_HTTP_POOL`. A
    /// loose `.kp` file with no manifest always gets the default.
    pub(crate) http_pool: usize,
    /// `naming_case = <0|1>`: the case `kelpc lint` asks the project's own
    /// names to be written in (`syntax.md` 19). `None` means the manifest
    /// says nothing about it, which the lint reads as the option's default.
    pub(crate) naming_case: Option<usize>,
    /// `[caps]` (C-2): the entry module's own capability grant, domain ->
    /// patterns, in the same vocabulary a `caps(domain = [...])` clause on an
    /// `impl` line uses. `None` means the section is absent, which keeps the
    /// long-standing default - the entry module holds the whole resource of
    /// every domain (`/**`/`*`) - so a `coral.toml` written before this
    /// section existed, or one that simply omits it, behaves exactly as
    /// before. `Some(map)` may still omit a domain entirely: that domain is
    /// then *not* granted to the entry module at all, a resolution error at
    /// any reference to it, the same as an ordinary ungranted module.
    pub(crate) caps: Option<HashMap<String, Vec<String>>>,
    /// `[extern."<module id>"]` (MK9-S1-M1, roadmap B-1): every user module
    /// this manifest assigns a domain, native library and symbol allowlist
    /// to. Empty when the manifest has none, which behaves exactly as before
    /// this feature existed: a module with no entry here gets no domain from
    /// it, and its `extern fun` declarations stay refused as before.
    pub(crate) extern_modules: HashMap<String, ExternModuleDecl>,
    /// `[unsafe] modules = ["<module id>", ...]` (MK9-S2 stage 1): the modules
    /// allowed to contain `unsafe`. Empty when the section is absent, which
    /// keeps a project that never writes `unsafe` behaving exactly as before.
    pub(crate) unsafe_modules: Vec<String>,
    /// `[alias]`: one `name = "path"` per line, in the order the manifest
    /// lists them. Each is an alias whose replacement text is the relative
    /// module path.
    pub(crate) alias: Vec<preprocess::AliasDef>,
    /// The manifest's path and text, set only when `alias` is non-empty.
    pub(crate) alias_source: Option<(PathBuf, String)>,
    /// The `[tidepool]` section (Tidepool step 2), all `None` when absent.
    pub(crate) tidepool: TidepoolDecl,
    /// Whether a `[target]` section header was seen at all (kernel stage 4).
    /// Distinct from the fields below being unset, because a `[target]` section
    /// that names no `mode` is an error rather than the hosted default.
    pub(crate) target_seen: bool,
    /// The `[target]` section's contents, `TargetDecl::hosted()` when there is
    /// none.
    pub(crate) target: TargetDecl,
    /// Whether `mode` was set, so a section without one is refused rather than
    /// defaulted.
    pub(crate) target_mode_set: bool,
    /// Whether `cpus` was set, so a duplicated key is refused rather than
    /// silently keeping one of the two values.
    pub(crate) target_cpus_set: bool,
    /// `[build] incremental = true` (roadmap A-14 slice 1): whether the backend
    /// splits the program into one compilation unit per source module. Absent
    /// (or `false`) is the whole-program default, which is what every project
    /// written before this section existed gets.
    pub(crate) incremental: bool,
    /// `[build] sanitize = "address"|"undefined"|"thread"`.
    pub(crate) sanitize: Option<crate::driver::Sanitizer>,
    /// `[build] emit = "binary"|"html"`.
    pub(crate) emit: Emit,
    /// `[build] obfuscate = "off"|"names"`, `Off` when absent.
    pub(crate) obfuscate: Obfuscate,
    /// `[build] emit_entry = "<file under src/>"`: the entry compiled when `emit`
    /// is `Html`. Required for `Html`, ignored for `Binary`.
    pub(crate) emit_entry: Option<String>,
    /// `[dependencies]` (Coral's section): every dependency key, with the
    /// `caps` list its inline-table entry may carry. Read here only for the
    /// capability check (`caps_report.rs`); Coral still fetches the sources.
    pub(crate) dependencies: Vec<DependencyDecl>,
}

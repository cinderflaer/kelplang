//! Multi-file module loading and name resolution (M5).
//!
//! Surface design (see `plan.md`'s "Imports & project structure"):
//!
//! - `impl "shapes/circle"` pulls in `src/shapes/circle.x`. The last path
//!   segment (`circle`) becomes the default alias; `impl "time" as t` picks
//!   another. No `mod` declarations, no package registry.
//! - v1 is local-files-only: paths are relative to `src/`, may not contain
//!   `..`, and may not reach outside the project. `std/...` is recognized
//!   but not implemented until M6/M7.
//! - Names are strictly module-qualified: module `m`'s items are reachable
//!   from elsewhere only as `circle.Item` (never bare), and a file's own
//!   items are reachable bare inside that file.
//!
//! This pass turns the module graph into *one* flat `ast::Program` whose
//! top-level names carry their declaring module's prefix
//! (`Circle` in `shapes/circle` becomes `shapes.circle.Circle`) and whose
//! declarations record the module they came from. `infer.rs` then resolves
//! `circle.Item` references against the per-module alias table this pass
//! hands it. A single merged program keeps the rest of the pipeline
//! (infer → borrowck → C codegen → one `cc` invocation) completely
//! unchanged in shape.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::diag::{Diagnostic, SourceMap};
use crate::lexer::{Lexer, Span, Token, TokenKind};
use crate::parser::Parser;
use crate::preprocess::{self, AliasTable};

/// One loaded module, as seen by the rest of the compiler.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// Canonical id: path relative to `src/` without `.x` (`shapes/circle`).
    pub id: String,
    /// Symbol prefix for this module's top-level items. Empty for the entry
    /// module; otherwise the id with `/` replaced by `.` plus a trailing
    /// `.` (`shapes.circle.`).
    pub prefix: String,
    /// alias -> target module id, for the `impl` lines *this* module wrote.
    pub aliases: HashMap<String, String>,
}

pub struct Project {
    /// Directory containing `x.toml`, when there is one. A loose `.x` file
    /// with no manifest still compiles; it is then its own module root.
    #[allow(dead_code)] // not read yet — later milestones (`pd` wiring, `xc fmt`) will.
    pub root: Option<PathBuf>,
    /// Base directory `impl` paths are resolved against (`<root>/src`, or
    /// the entry file's own directory when there is no manifest).
    #[allow(dead_code)]
    pub src_dir: PathBuf,
    /// Module id of the entry module — the one whose `main` is the program
    /// entry point, and the only module whose symbols are unprefixed.
    pub entry: String,
    #[allow(dead_code)]
    pub modules: Vec<ModuleInfo>,
    #[allow(dead_code)]
    pub name: Option<String>,
    #[allow(dead_code)]
    pub version: Option<String>,
    prefixes: HashMap<String, String>,
    /// Per module: alias -> the target module's symbol prefix (what
    /// `infer.rs` needs to qualify `circle.Item`).
    alias_prefixes: HashMap<String, HashMap<String, String>>,
    /// Per module: capability domain -> the patterns it holds (M7). The entry
    /// module holds `/**` for every domain — the plan's "main implicitly
    /// holds the full set" — and every other module holds exactly what its
    /// importers delegated to it, never more.
    caps: HashMap<String, HashMap<String, Vec<String>>>,
    /// Per function key: the capability domain of the module that declares
    /// it, for the gated modules only. `infer.rs` gates resolution on it and
    /// `codegen_c.rs` inserts the runtime check because of it.
    gated: HashMap<String, String>,
}

impl Project {
    /// Symbol prefix for a module id (`""` for the entry module).
    pub fn prefix_of(&self, module: &str) -> &str {
        self.prefixes.get(module).map(|s| s.as_str()).unwrap_or("")
    }

    /// `alias -> prefix` table for one module, used to resolve
    /// module-qualified references inside that module's bodies.
    pub fn alias_prefixes_of(&self, module: &str) -> HashMap<String, String> {
        self.alias_prefixes.get(module).cloned().unwrap_or_default()
    }

    /// The capability domains a module holds, with their patterns. An empty
    /// map means "no capabilities at all", which is where every module starts
    /// (the entry module excepted).
    pub fn caps_of(&self, module: &str) -> HashMap<String, Vec<String>> {
        self.caps.get(module).cloned().unwrap_or_default()
    }

    /// The whole capability table: module -> domain -> patterns.
    pub fn caps(&self) -> &HashMap<String, HashMap<String, Vec<String>>> {
        &self.caps
    }

    /// Function key -> gated domain, for the modules that gate one.
    pub fn gated(&self) -> &HashMap<String, String> {
        &self.gated
    }
}

pub struct Loaded {
    /// One merged program: all modules' decls, prefixed and module-tagged.
    pub program: ast::Program,
    pub project: Project,
}

/// Loads the entry file, follows every `impl` transitively, rejects import
/// cycles and bad paths, and returns the merged program plus the module
/// tables `infer.rs` needs.
pub fn load(entry_path: &Path, sources: &mut SourceMap) -> Result<Loaded, Diagnostic> {
    let given = fs::canonicalize(entry_path).map_err(|e| {
        Diagnostic::new(
            format!("cannot read '{}': {e}", entry_path.display()),
            Span::new(0, 0),
            "",
        )
    })?;

    let (entry_file, root, src_dir, source_name, source_version) = if given.is_dir() {
        let manifest_path = given.join("x.toml");
        let text = fs::read_to_string(&manifest_path).map_err(|_| {
            Diagnostic::new(
                format!(
                    "'{}' is a directory but has no x.toml (pass a .x file, or add a manifest)",
                    given.display()
                ),
                Span::new(0, 0),
                "",
            )
        })?;
        let manifest = parse_manifest(&text, &manifest_path)?;
        let entry_file = fs::canonicalize(given.join(&manifest.entry)).map_err(|e| {
            Diagnostic::new(
                format!(
                    "cannot read the manifest's entry point '{}': {e}",
                    manifest.entry
                ),
                Span::new(0, 0),
                "",
            )
        })?;
        (
            entry_file,
            Some(given.clone()),
            given.join("src"),
            manifest.name,
            manifest.version,
        )
    } else {
        match find_manifest_root(&given) {
            Some(root) => {
                let manifest_path = root.join("x.toml");
                let text = fs::read_to_string(&manifest_path).unwrap_or_default();
                let manifest = parse_manifest(&text, &manifest_path)?;
                let src_dir = root.join("src");
                if !given.starts_with(&src_dir) {
                    return Err(Diagnostic::new(
                        format!(
                            "entry file '{}' is not inside '{}'",
                            given.display(),
                            src_dir.display()
                        ),
                        Span::new(0, 0),
                        "",
                    ));
                }
                (given, Some(root), src_dir, manifest.name, manifest.version)
            }
            None => {
                let dir = given
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                (given, None, dir, None, None)
            }
        }
    };

    let entry = module_id_of(&entry_file, &src_dir)?;

    // `alias.xconf` lives at the project root — the directory holding
    // `x.toml`, or the entry file's own directory for a manifest-less file.
    let xconf_path = match &root {
        Some(r) => r.join(preprocess::XCONF_FILE_NAME),
        None => src_dir.join(preprocess::XCONF_FILE_NAME),
    };

    let mut loader = Loader {
        src_dir: src_dir.clone(),
        entry: entry.clone(),
        xconf_path,
        sources,
        aliases: AliasTable::new(),
        raw: HashMap::new(),
        parsed: HashMap::new(),
        state: HashMap::new(),
        stack: Vec::new(),
        modules: Vec::new(),
        prefixes: HashMap::new(),
        grants: Vec::new(),
    };
    loader.load_project()?;

    let mut program = ast::Program {
        impls: Vec::new(),
        consts: Vec::new(),
        pds: Vec::new(),
        datacs: Vec::new(),
        functions: Vec::new(),
    };
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for c in &parsed.consts {
            let mut c = c.clone();
            c.name = format!("{}{}", m.prefix, c.name);
            c.module = m.id.clone();
            program.consts.push(c);
        }
        for p in &parsed.pds {
            let mut p = p.clone();
            p.name = format!("{}{}", m.prefix, p.name);
            p.module = m.id.clone();
            program.pds.push(p);
        }
        for d in &parsed.datacs {
            let mut d = d.clone();
            d.name = format!("{}{}", m.prefix, d.name);
            d.module = m.id.clone();
            program.datacs.push(d);
        }
        for f in &parsed.functions {
            let mut f = f.clone();
            match &mut f.receiver {
                // A method's key is `Type#name`, so the *receiver type* gets
                // the prefix, not the method name.
                Some(r) => r.type_name = format!("{}{}", m.prefix, r.type_name),
                None => f.name = format!("{}{}", m.prefix, f.name),
            }
            f.module = m.id.clone();
            program.functions.push(f);
        }
    }

    let mut alias_prefixes = HashMap::new();
    for m in &loader.modules {
        let mut table = HashMap::new();
        for (alias, target) in &m.aliases {
            let prefix = loader
                .prefixes
                .get(target)
                .cloned()
                .unwrap_or_default();
            table.insert(alias.clone(), prefix);
        }
        alias_prefixes.insert(m.id.clone(), table);
    }

    // ---- M7: capability sets -------------------------------------------
    //
    // A module's set is *received*, never self-declared: the entry module
    // holds everything (`/**`), and every other module holds exactly what its
    // importers granted it. That makes the no-escalation rule checkable —
    // a module can delegate less than it has, never more — and is what keeps
    // an ungranted `fs.read` a resolution error instead of a runtime check.
    let mut caps: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut entry_caps = HashMap::new();
    for domain in crate::stdlib::DOMAINS {
        // The entry module's implicit set has to be spelled in whatever
        // vocabulary the domain uses: paths for `fs`, a bare name wildcard
        // for `env`.
        let wildcard = if crate::stdlib::patterns_are_paths(domain) {
            "/**"
        } else {
            "*"
        };
        entry_caps.insert(domain.to_string(), vec![wildcard.to_string()]);
    }
    caps.insert(entry.clone(), entry_caps);
    for g in &loader.grants {
        let slot = caps
            .entry(g.target.clone())
            .or_default()
            .entry(g.domain.clone())
            .or_default();
        for pattern in &g.patterns {
            if !slot.contains(pattern) {
                slot.push(pattern.clone());
            }
        }
    }
    for g in &loader.grants {
        let held = caps
            .get(&g.importer)
            .and_then(|d| d.get(&g.domain))
            .cloned()
            .unwrap_or_default();
        for pattern in &g.patterns {
            if !held
                .iter()
                .any(|h| pattern_covered(h, pattern, &g.domain))
            {
                return Err(Diagnostic::new(
                    format!(
                        "module '{}' cannot grant '{}' access to '{}': it does not hold that pattern itself (held: {}) — a module can delegate less than it has, never more",
                        g.importer,
                        g.domain,
                        pattern,
                        if held.is_empty() {
                            "none".to_string()
                        } else {
                            held.join(", ")
                        }
                    ),
                    g.span,
                    g.importer.clone(),
                ));
            }
        }
    }

    // Which of the shipped items are gated: a module that gates a domain
    // contributes every free function it declares. (Overloaded names only
    // exist in `std/math`, which is ungated, so the plain key is enough.)
    let mut gated = HashMap::new();
    for m in &loader.modules {
        let Some(domain) = crate::stdlib::domain_of(&m.id) else {
            continue;
        };
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            // Externs are the *primitive*: the raw syscall inside the gated
            // module. They are not themselves gated, or the module wrapping
            // them would need a grant to call its own body — the check
            // belongs at the boundary where a *user* module names the public
            // function, which is what this table drives.
            if f.receiver.is_none() && !f.extern_ {
                gated.insert(format!("{}{}", m.prefix, f.name), domain.to_string());
            }
        }
    }

    let project = Project {
        root,
        src_dir,
        entry,
        modules: loader.modules,
        name: source_name,
        version: source_version,
        prefixes: loader.prefixes,
        alias_prefixes,
        caps,
        gated,
    };
    Ok(Loaded { program, project })
}

struct Loader<'a> {
    src_dir: PathBuf,
    /// Module id of the entry module — the only one with an empty prefix.
    entry: String,
    /// `<project root>/alias.xconf`, whether or not it exists.
    xconf_path: PathBuf,
    sources: &'a mut SourceMap,
    /// The one flat, global alias table: `alias.xconf` plus every inline
    /// declaration in the project.
    aliases: AliasTable,
    /// Raw, unexpanded token streams per module, kept so a later round can
    /// re-expand against a table that has grown.
    raw: HashMap<String, Vec<Token>>,
    parsed: HashMap<String, ast::Program>,
    /// 1 = currently being loaded (on `stack`), 2 = finished.
    state: HashMap<String, u8>,
    stack: Vec<String>,
    modules: Vec<ModuleInfo>,
    prefixes: HashMap<String, String>,
    /// Every `with caps(...)` seen while loading, in load order (M7). Kept
    /// until the whole graph is known, because a module's own set is the
    /// union of the grants made *to* it by importers loaded from anywhere.
    grants: Vec<GrantEdge>,
}

/// One `impl "target" with caps(domain = [patterns])` from `importer` (M7).
#[derive(Debug, Clone)]
struct GrantEdge {
    importer: String,
    target: String,
    domain: String,
    patterns: Vec<String>,
    span: Span,
}

/// Bound on alias-driven re-expansion. Each discovery round only ever *adds*
/// modules, so this is a guard against a pathological case, not an expected
/// limit.
const MAX_ALIAS_ROUNDS: usize = 8;

impl Loader<'_> {
    /// Reads `alias.xconf`, discovers the module graph, then loads it for
    /// real.
    ///
    /// Two phases, because the two halves of M5.3 need each other: an `impl`
    /// path may contain `%*alias*%`, so the file graph is only knowable after
    /// expansion, while the alias table is only complete once every file is
    /// known. Discovery therefore tolerates what it cannot yet resolve — a
    /// file that fails to expand or parse is skipped, an unresolvable import
    /// is ignored — and just collects aliases; the final strict round runs
    /// against the complete table, where every failure is a real error.
    fn load_project(&mut self) -> Result<(), Diagnostic> {
        self.load_alias_xconf()?;

        let mut last_error = None;
        for _ in 0..MAX_ALIAS_ROUNDS {
            // Discovery: close over every file reachable without the aliases
            // this round has not collected yet.
            loop {
                let before = self.raw.len();
                self.reset_round();
                let entry = self.entry.clone();
                // Discovery never fails: it skips what it cannot resolve yet.
                let _ = self.visit(&entry, Span::new(0, 0), "", false);
                if self.raw.len() == before {
                    break;
                }
            }

            let before = self.raw.len();
            self.reset_round();
            let entry = self.entry.clone();
            let result = self.visit(&entry, Span::new(0, 0), "", true);
            if self.raw.len() == before {
                // Nothing new was loaded, so no later round could resolve
                // more aliases: this result is final.
                result?;
                // Catches a cycle nothing references; expansion only finds
                // one it actually expands.
                return self.aliases.check_cycles();
            }
            // The strict round found a file via an aliased path, which may in
            // turn have contributed the alias an earlier error was about.
            last_error = result.err();
        }

        Err(last_error.unwrap_or_else(|| {
            Diagnostic::new(
                format!("alias expansion did not settle after {MAX_ALIAS_ROUNDS} rounds"),
                Span::new(0, 0),
                "",
            )
        }))
    }

    fn reset_round(&mut self) {
        self.state.clear();
        self.stack.clear();
        self.modules.clear();
        self.prefixes.clear();
        self.parsed.clear();
    }

    fn load_alias_xconf(&mut self) -> Result<(), Diagnostic> {
        let Ok(text) = fs::read_to_string(&self.xconf_path) else {
            return Ok(()); // the file is optional
        };
        self.sources
            .insert(preprocess::XCONF_MODULE, &self.xconf_path, text.clone());
        for def in preprocess::parse_xconf(&text)? {
            self.aliases.define(def)?;
        }
        Ok(())
    }

    /// `strict == false` is the discovery round: every failure is skipped, so
    /// a file loaded later can still contribute the alias an earlier file
    /// used.
    fn visit(&mut self, id: &str, at: Span, from: &str, strict: bool) -> Result<(), Diagnostic> {
        let depth = self.stack.len();
        let result = self.visit_body(id, at, from, strict);
        if result.is_err() {
            // A failed visit must not leave the module looking half-loaded —
            // a sibling import should be free to try it again.
            self.stack.truncate(depth);
            self.state.remove(id);
        }
        result
    }

    /// The raw token stream for one module, loaded from disk on first sight.
    /// Inline `alias` declarations are collected here, from the *unexpanded*
    /// tokens, so every file the walk reaches contributes its aliases.
    fn tokens_of(&mut self, id: &str, at: Span, from: &str) -> Result<Vec<Token>, Diagnostic> {
        if let Some(tokens) = self.raw.get(id) {
            return Ok(tokens.clone());
        }
        // A `std/...` id comes from the compiler's own shipped sources (M6.2)
        // instead of the project directory; everything downstream — lexing,
        // aliases, parsing, prefixing — is identical.
        let (path, text) = match crate::stdlib::source(id) {
            Some(text) => (
                self.src_dir.join(id).with_extension("x"),
                text.to_string(),
            ),
            None => {
                let path = self.src_dir.join(format!("{id}.x"));
                let text = fs::read_to_string(&path).map_err(|_| {
                    Diagnostic::new(
                        format!("cannot find module '{id}': expected '{}'", path.display()),
                        at,
                        from,
                    )
                })?;
                (path, text)
            }
        };
        self.sources.insert(id, &path, text.clone());

        let tokens = Lexer::new(&text)
            .tokenize()
            .map_err(|e| Diagnostic::new(e.message, e.span, id))?;
        for def in preprocess::scan_inline_aliases(&tokens, &text, id)? {
            self.aliases.define(def)?;
        }
        self.raw.insert(id.to_string(), tokens.clone());
        Ok(tokens)
    }

    fn visit_body(&mut self, id: &str, at: Span, from: &str, strict: bool) -> Result<(), Diagnostic> {
        match self.state.get(id) {
            Some(2) => return Ok(()),
            Some(_) => {
                let start = self.stack.iter().position(|m| m == id).unwrap_or(0);
                let mut chain: Vec<String> = self.stack[start..].to_vec();
                chain.push(id.to_string());
                return Err(Diagnostic::new(
                    format!("import cycle: {}", chain.join(" -> ")),
                    at,
                    from,
                ));
            }
            _ => {}
        }

        let tokens = self.tokens_of(id, at, from)?;

        if !strict {
            // Discovery. Imports come straight off the raw token stream, so a
            // file whose body cannot expand yet — it uses an alias another
            // file has still to contribute — nevertheless contributes its own
            // aliases and its unaliased imports. Unresolvable paths are
            // skipped here and become real errors in the strict round.
            self.state.insert(id.to_string(), 1);
            self.stack.push(id.to_string());
            for (path, span) in raw_imports(&tokens) {
                if validate_impl_path(&path, span, id).is_ok() {
                    let _ = self.visit(&path, span, id, false);
                }
            }
            self.stack.pop();
            self.state.insert(id.to_string(), 2);
            return Ok(());
        }

        let expanded = self.aliases.expand_file(&tokens, id)?;
        let program = Parser::new(expanded)
            .parse_program()
            .map_err(|e| Diagnostic::new(e.message, e.span, id))?;

        self.state.insert(id.to_string(), 1);
        self.stack.push(id.to_string());

        let mut aliases = HashMap::new();
        let mut seen_aliases: HashSet<String> = HashSet::new();
        for imp in &program.impls {
            validate_impl_path(&imp.path, imp.span, id)?;
            let alias = match &imp.alias {
                Some(a) => a.clone(),
                None => default_alias(&imp.path),
            };
            validate_alias(&alias, imp.span, id)?;
            if !seen_aliases.insert(alias.clone()) {
                return Err(Diagnostic::new(
                    format!("alias '{alias}' is already used by another import in this module"),
                    imp.span,
                    id,
                ));
            }
            for grant in &imp.caps {
                if !crate::stdlib::is_domain(&grant.domain) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a capability domain — v1 has {}",
                            grant.domain,
                            crate::stdlib::DOMAINS.join(", ")
                        ),
                        grant.span,
                        id,
                    ));
                }
                for pattern in &grant.patterns {
                    validate_cap_pattern(&grant.domain, pattern, grant.span, id)?;
                }
                self.grants.push(GrantEdge {
                    importer: id.to_string(),
                    target: imp.path.clone(),
                    domain: grant.domain.clone(),
                    patterns: grant.patterns.clone(),
                    span: grant.span,
                });
            }
            let target = imp.path.clone();
            aliases.insert(alias, target.clone());
            self.visit(&target, imp.span, id, strict)?;
        }

        self.stack.pop();
        self.state.insert(id.to_string(), 2);

        // Dependencies are pushed first (post-order), so by the time a
        // module is pushed here every prefix it needs is already known.
        let prefix = if id == self.entry {
            String::new()
        } else {
            format!("{}.", id.replace('/', "."))
        };
        self.prefixes.insert(id.to_string(), prefix.clone());
        self.modules.push(ModuleInfo {
            id: id.to_string(),
            prefix,
            aliases,
        });
        self.parsed.insert(id.to_string(), program);
        Ok(())
    }
}

/// The alias used when `as` is not written: the last path segment.
fn default_alias(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Every `impl "path"` in a raw, unexpanded token stream. The discovery
/// round walks these instead of a parsed AST, which is what lets it find the
/// files that define the aliases a file's own body cannot expand without.
fn raw_imports(tokens: &[Token]) -> Vec<(String, Span)> {
    let mut imports = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::Impl) {
            continue;
        }
        if let Some(Token {
            kind: TokenKind::StringLit(path),
            span,
        }) = tokens.get(i + 1)
        {
            imports.push((path.clone(), *span));
        }
    }
    imports
}

fn parse_manifest(text: &str, path: &Path) -> Result<Manifest, Diagnostic> {
    let mut manifest = Manifest {
        name: None,
        version: None,
        entry: "src/main.x".to_string(),
    };
    let at = Span::new(0, 0);
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            return Err(Diagnostic::new(
                format!("x.toml sections are not supported (found '{line}')"),
                at,
                "",
            ));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(Diagnostic::new(
                format!("expected 'key = \"value\"' in x.toml, found '{line}'"),
                at,
                "",
            ));
        };
        let key = key.trim();
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
        else {
            return Err(Diagnostic::new(
                format!("x.toml value for '{key}' must be a double-quoted string"),
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
    Ok(manifest)
}

struct Manifest {
    name: Option<String>,
    version: Option<String>,
    /// Path relative to the project root; `xc build <dir>` uses it.
    entry: String,
}

fn find_manifest_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        if d.join("x.toml").is_file() {
            return Some(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Canonical module id for a file: its path relative to `src/`, minus the
/// `.x` suffix, with `/` separators.
fn module_id_of(file: &Path, src_dir: &Path) -> Result<String, Diagnostic> {
    let rel = file.strip_prefix(src_dir).map_err(|_| {
        Diagnostic::new(
            format!(
                "entry file '{}' is not inside '{}'",
                file.display(),
                src_dir.display()
            ),
            Span::new(0, 0),
            "",
        )
    })?;
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = rel.parent().map(|p| p.to_string_lossy().replace('\\', "/"));
    let id = match dir.as_deref() {
        Some(d) if !d.is_empty() && d != "." => format!("{d}/{stem}"),
        _ => stem,
    };
    if id.is_empty() {
        return Err(Diagnostic::new(
            format!("cannot determine a module name for '{}'", file.display()),
            Span::new(0, 0),
            "",
        ));
    }
    Ok(id)
}

/// Whether a pattern the *holder* has covers a pattern it wants to delegate.
///
/// Deliberately syntactic and conservative: either the two patterns are
/// identical, or the held one ends in `/**` and the delegated one is inside
/// that root. Glob-to-glob inclusion is undecidable in general, and being
/// conservative here can only under-grant — never let a module hand out more
/// than it holds.
fn pattern_covered(held: &str, granted: &str, domain: &str) -> bool {
    if held == granted {
        return true;
    }
    if !crate::stdlib::patterns_are_paths(domain) {
        // A name domain: `*` is the only pattern that covers other patterns.
        return held == "*";
    }
    match held.strip_suffix("/**") {
        Some(root) => granted == root || granted.starts_with(&format!("{root}/")),
        None => false,
    }
}

/// A capability pattern, validated by what the domain actually names.
///
/// `fs` (and `net`/`process`) name paths, so a pattern is an absolute path,
/// glob-only: `*` inside one segment, `**` as a whole segment, and no `..`
/// anywhere — a pattern that could name its way out of its own root is
/// rejected at the `impl` line, at compile time. `env` names variables, so
/// its patterns are names (`HOME`, `LANG*`, or `*` for any). `time` and
/// `rand` have no named resource at all, so their pattern list is unused and
/// is normally empty.
fn validate_cap_pattern(
    domain: &str,
    pattern: &str,
    span: Span,
    module: &str,
) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if pattern.is_empty() {
        return err(format!("a '{domain}' pattern must not be empty"));
    }
    if !crate::stdlib::patterns_are_paths(domain) {
        if domain == "env" && pattern.contains('/') {
            return err(format!(
                "'{pattern}' is not a variable name — 'env' patterns name variables ('HOME', 'LANG*', or '*' for any)"
            ));
        }
        return Ok(());
    }
    if !pattern.starts_with('/') {
        return err(format!(
            "capability patterns are absolute paths — '{pattern}' must start with '/'"
        ));
    }
    for seg in pattern.split('/') {
        if seg == ".." {
            return err(format!(
                "capability pattern '{pattern}' contains a '..' segment — a grant must not be able to name its way out of its own root"
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

/// `impl` paths are `src/`-relative, identifier-only, and may not escape the
/// project. `std/...` is reserved for compiler-shipped modules (M6/M7).
fn validate_impl_path(path: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if path.is_empty() {
        return err("'impl' path must not be empty".to_string());
    }
    if path.ends_with(".x") {
        return err(format!(
            "'impl' paths omit the '.x' extension — write \"{}\"",
            path.trim_end_matches(".x")
        ));
    }
    if path.starts_with('/') {
        return err("'impl' paths are relative to 'src/' and must not start with '/'".to_string());
    }
    if path.contains('\\') {
        return err("use '/' as the separator in 'impl' paths".to_string());
    }
    if path == "std" || path.starts_with("std/") {
        // `std/mem` and friends ship with the compiler (M6.2). A `std/...`
        // module that is not shipped yet is reported as such rather than as
        // a missing file, so the reservation stays visible.
        if crate::stdlib::source(path).is_some() {
            return Ok(());
        }
        return err(format!(
            "'impl \"{path}\"' — stdlib module '{}' is not implemented yet (shipped today: {}; the rest land in M6.3/M7)",
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
                "'{seg}' is not a valid module name — use snake_case identifiers with no dashes or dots"
            ));
        }
    }
    Ok(())
}

fn validate_alias(alias: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
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
            | "return"
            | "true"
            | "false"
            | "datac"
            | "impl"
            | "as"
            | "pd"
            | "alias"
            | "print"
    )
}

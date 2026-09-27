//! Multi-file module loading and name resolution (M5).
//!
//! Surface design (see the plan's "Imports & project structure"):
//!
//! - `impl "shapes/circle"` pulls in `src/shapes/circle.kp`. The last path
//!   segment (`circle`) is the namespace. No `mod` declarations, no package
//!   registry. An `as name` clause cannot override it: it was removed, and the
//!   parser refuses it by name.
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

/// Source-file extension, without the leading dot. Every extension decision
/// in this file goes through it, so a future rebrand is one line.
pub const EXT: &str = "kp";

/// Manifest filename at a project root. Named after the build tool (Coral),
/// not the language: the manifest is Coral's, the sources are Kelp's.
pub const MANIFEST_NAME: &str = "coral.toml";

/// One loaded module, as seen by the rest of the compiler.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// Canonical id: path relative to `src/` without `.kp` (`shapes/circle`).
    pub id: String,
    /// Symbol prefix for this module's top-level items. Empty for the entry
    /// module; otherwise the id with `/` replaced by `.` plus a trailing
    /// `.` (`shapes.circle.`).
    pub prefix: String,
    /// alias -> target module id, for the `impl` lines *this* module wrote.
    pub aliases: HashMap<String, String>,
}

pub struct Project {
    /// Directory containing `coral.toml`, when there is one. A loose `.kp` file
    /// with no manifest still compiles; it is then its own module root.
    #[allow(dead_code)] // not read yet — later milestones (`pd` wiring, `kelpc fmt`) will.
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
    /// Per module: its `coral.toml` `[extern."<module id>"]` entry (MK9-S1-M1,
    /// roadmap B-1), when it has one. This is what lets a *user* module's own
    /// `extern fun` declarations exist at all: `domain_of` below folds this
    /// table in beside `stdlib::domain_of`'s stdlib-only table, so a module
    /// named here is gated exactly like a stdlib module is, and `infer.rs`
    /// checks a declared `extern fun`'s bare name against the entry's
    /// `symbols` allowlist instead of refusing every non-stdlib extern
    /// outright.
    extern_modules: HashMap<String, ExternModuleDecl>,
}

impl Project {
    /// The capability domain a module is gated on: a stdlib module's fixed
    /// domain (`stdlib::domain_of`), or, when there is none, this project's
    /// own `[extern."<module id>"]` assignment. Both tables are consulted the
    /// same way at every call site that used to read `stdlib::domain_of`
    /// alone (M7's `gated` map, the RT-module injection, the `caps(*)`
    /// reachability walk), which is the point of MK9-S1-M1's design: a user
    /// module's domain behaves exactly like a stdlib one downstream, because
    /// it is the same table with one more source.
    pub fn domain_of(&self, module: &str) -> Option<String> {
        domain_of_with_extern(module, &self.extern_modules)
    }

    /// The native library and symbol allowlist a `[extern."<module id>"]`
    /// section declared for `module`, if it has one. `infer.rs` checks a
    /// declared `extern fun`'s bare name against `symbols`;
    /// `codegen_c.rs`/`codegen_llvm.rs` use it to write the direct-call
    /// trampoline instead of looking the body up in `externs.rs`.
    pub fn extern_ffi_of(&self, module: &str) -> Option<&ExternModuleDecl> {
        self.extern_modules.get(module)
    }

    /// Every native library named across the project's `[extern.*]`
    /// sections, deduplicated: `driver.rs`'s link line adds `-l<name>` for
    /// each one.
    pub fn extern_libraries(&self) -> Vec<String> {
        let mut libs: Vec<String> = self
            .extern_modules
            .values()
            .map(|e| e.library().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        libs.sort();
        libs.dedup();
        libs
    }
}

/// One `[extern."<module id>"]` section's contents (MK9-S1-M1, roadmap B-1):
/// the native library and exact symbol set a user module's `extern fun`
/// declarations may bind to, once `domain` has also assigned it one of
/// `stdlib::DOMAINS`. `domain`/`library` are `Option` only while parsing —
/// `parse_manifest` refuses a section that finishes with either still unset,
/// so every entry a caller sees has both.
#[derive(Debug, Clone, Default)]
pub struct ExternModuleDecl {
    domain: Option<String>,
    /// The library `-l<name>` names on the link line.
    library: Option<String>,
    /// The exact native symbol names this module's `extern fun` declarations
    /// may bind to. MK9-S1-M1 keeps user FFI to a 1:1 name mapping: the bare
    /// Kelp function name a declaration writes *is* the symbol
    /// `codegen_c.rs` calls, so a rename is not this pass's problem to solve.
    pub symbols: Vec<String>,
}

impl ExternModuleDecl {
    /// The native library this module's symbols are declared to live in.
    pub fn library(&self) -> &str {
        self.library.as_deref().unwrap_or_default()
    }
}

/// `stdlib::domain_of`, folded with a project's own `[extern.*]` table
/// (MK9-S1-M1). Shared by `Project::domain_of` and by the loader's own three
/// domain checks (the `gated` map, the RT-module import injection, and
/// `caps(*)`'s reachability walk), so a user module is gated exactly like a
/// stdlib one at every one of them, not just some.
fn domain_of_with_extern(
    module: &str,
    extern_modules: &HashMap<String, ExternModuleDecl>,
) -> Option<String> {
    crate::stdlib::domain_of(module)
        .map(|d| d.to_string())
        .or_else(|| extern_modules.get(module).and_then(|e| e.domain.clone()))
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
    /// Outdated syntax the loaded modules use, in load order
    /// (`deprecated.rs`). `kelpc build` and `kelpc run` report them, as
    /// `outdated:` warnings or as errors under `--deny-outdated`; the
    /// formatter ignores them, because a formatter that printed warnings would
    /// mix them into the text it writes to stdout.
    pub deprecations: Vec<Diagnostic>,
}

/// Loads the entry file, follows every `impl` transitively, rejects import
/// cycles and bad paths, and returns the merged program plus the module
/// tables `infer.rs` needs.
///
/// `entry_text` overrides the entry module's text with a buffer instead of
/// what is on disk at `entry_path` (`kelpc check --text`, MISSING-b6Ytr): the
/// path is still resolved and canonicalized normally, so module identity, the
/// `src/` layout rule and every other module's disk read are unaffected, and
/// only the one module named by `entry_path` reads from the override.
pub fn load(
    entry_path: &Path,
    sources: &mut SourceMap,
    entry_text: Option<String>,
) -> Result<Loaded, Diagnostic> {
    let given = fs::canonicalize(entry_path).map_err(|e| {
        Diagnostic::new(
            format!("cannot read '{}': {e}", entry_path.display()),
            Span::new(0, 0),
            "",
        )
    })?;

    let (
        entry_file,
        root,
        src_dir,
        source_name,
        source_version,
        http_pool,
        manifest_caps,
        manifest_extern,
    ) = if given.is_dir() {
            let manifest_path = given.join(MANIFEST_NAME);
            let text = fs::read_to_string(&manifest_path).map_err(|_| {
                Diagnostic::new(
                    format!(
                        "'{}' is a directory but has no {} (pass a .{} file, or add a manifest)",
                        given.display(),
                        MANIFEST_NAME,
                        EXT
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
                manifest.http_pool,
                manifest.caps,
                manifest.extern_modules,
            )
        } else {
            match find_manifest_root(&given) {
                Some(root) => {
                    let manifest_path = root.join(MANIFEST_NAME);
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
                    (
                        given,
                        Some(root),
                        src_dir,
                        manifest.name,
                        manifest.version,
                        manifest.http_pool,
                        manifest.caps,
                        manifest.extern_modules,
                    )
                }
                None => {
                    let dir = given
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("."));
                    (
                        given,
                        None,
                        dir,
                        None,
                        None,
                        crate::stdlib::DEFAULT_HTTP_POOL,
                        None,
                        HashMap::new(),
                    )
                }
            }
        };

    let entry = module_id_of(&entry_file, &src_dir)?;

    // `alias.kpconf` lives at the project root — the directory holding
    // `coral.toml`, or the entry file's own directory for a manifest-less file.
    let kpconf_path = match &root {
        Some(r) => r.join(preprocess::KPCONF_FILE_NAME),
        None => src_dir.join(preprocess::KPCONF_FILE_NAME),
    };

    let mut loader = Loader {
        src_dir: src_dir.clone(),
        entry: entry.clone(),
        entry_text,
        kpconf_path,
        sources,
        http_pool,
        aliases: AliasTable::new(),
        raw: HashMap::new(),
        parsed: HashMap::new(),
        state: HashMap::new(),
        stack: Vec::new(),
        modules: Vec::new(),
        prefixes: HashMap::new(),
        grants: Vec::new(),
        all_grants: Vec::new(),
        deprecations: Vec::new(),
        extern_modules: manifest_extern,
    };
    loader.load_project()?;

    let mut program = ast::Program {
        impls: Vec::new(),
        consts: Vec::new(),
        pds: Vec::new(),
        datacs: Vec::new(),
        enums: Vec::new(),
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
        for e in &parsed.enums {
            let mut e = e.clone();
            // The enum's name is prefixed like a `datac`'s, and each variant
            // is addressed as `Enum.Variant` at the surface — the same
            // `Type.member` shape a method call has, which is what makes
            // construction parse without a new expression form (A-1).
            e.name = format!("{}{}", m.prefix, e.name);
            e.module = m.id.clone();
            program.enums.push(e);
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

    // One merged declaration order across `datac`s and `enum`s (A-1). Each
    // declaration carries its index *within its own file*, and the module
    // list is already in load order, so renumbering here is what gives
    // `types::owned_types` and `codegen_c` a single sequence to walk: a
    // field or payload may only name a type declared before it by value, and
    // the two declaration lists are separate vectors.
    let mut merged_order = 0usize;
    for m in &loader.modules {
        let mut local: Vec<(usize, bool, usize)> = Vec::new();
        for (i, d) in program.datacs.iter().enumerate() {
            if d.module == m.id {
                local.push((d.order, false, i));
            }
        }
        for (i, e) in program.enums.iter().enumerate() {
            if e.module == m.id {
                local.push((e.order, true, i));
            }
        }
        local.sort_by_key(|(order, _, _)| *order);
        for (_, is_enum, idx) in local {
            if is_enum {
                program.enums[idx].order = merged_order;
            } else {
                program.datacs[idx].order = merged_order;
            }
            merged_order += 1;
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
    // A module's set is *received*, never self-declared, with one exception:
    // the entry module's set. By default it still holds everything (`/**`/
    // `*`, every domain), but `coral.toml`'s `[caps]` section (C-2) lets a
    // project restrict it to exactly what is declared there — the natural
    // fix for open item 18, since `main` cannot write a `caps(...)` clause on
    // itself. `manifest_caps` is `None` whenever the section is absent (no
    // manifest at all, or a manifest that does not have one), which is what
    // keeps every `coral.toml` written before this existed behaving exactly
    // as before. Every other module holds exactly what its importers granted
    // it. That makes the no-escalation rule checkable — a module can
    // delegate less than it has, never more — and is what keeps an ungranted
    // `fs.read` a resolution error instead of a runtime check.
    let mut caps: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let entry_caps = manifest_caps.unwrap_or_else(|| {
        let mut whole = HashMap::new();
        for domain in crate::stdlib::DOMAINS {
            whole.insert(
                domain.to_string(),
                vec![crate::stdlib::whole_resource(domain).to_string()],
            );
        }
        whole
    });
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
    // `caps(*)` hands the target the same set the entry module holds: the
    // whole resource of every domain. That happens here, before the
    // no-escalation check below, so a module that *received* the wildcard can
    // delegate it onward exactly as it could delegate a pattern it was given.
    for g in &loader.all_grants {
        let slot = caps.entry(g.target.clone()).or_default();
        for domain in crate::stdlib::DOMAINS {
            let wildcard = crate::stdlib::whole_resource(domain).to_string();
            let patterns = slot.entry(domain.to_string()).or_default();
            if !patterns.contains(&wildcard) {
                patterns.push(wildcard);
            }
        }
    }
    for g in &loader.all_grants {
        // A module that declares nothing gated and reaches no module that
        // does is ungated, so there is no domain to be granted: `caps(*)` on
        // it is noise at best and a hint that the wrong module was named at
        // worst. Refusing it by name is the same refusal the ungranted-
        // namespace error is: the clause and the module disagree about what
        // there is to hand over.
        if !reaches_a_domain(&loader.modules, &g.target, &loader.extern_modules) {
            return Err(Diagnostic::new(
                format!(
                    "module '{}' is ungated: it reaches no capability domain, so `caps(*)` would grant it nothing; drop the clause",
                    g.target
                ),
                g.span,
                g.importer.clone(),
            ));
        }
        let held = caps.get(&g.importer).cloned().unwrap_or_default();
        for domain in crate::stdlib::DOMAINS {
            let wildcard = crate::stdlib::whole_resource(domain);
            let holds_it = held
                .get(*domain)
                .map(|patterns| {
                    patterns
                        .iter()
                        .any(|h| pattern_covered(h, wildcard, domain))
                })
                .unwrap_or(false);
            if holds_it {
                continue;
            }
            let shown = held
                .get(*domain)
                .filter(|patterns| !patterns.is_empty())
                .map(|patterns| patterns.join(", "))
                .unwrap_or_else(|| "none".to_string());
            // The wildcard is one clause over every domain, so the message
            // names `caps(*)` and the domain that failed rather than a single
            // pattern the importer never wrote.
            return Err(Diagnostic::new(
                format!(
                    "module '{}' cannot give '{}' the whole-resource wildcard for '{}' with `caps(*)`: it does not hold that wildcard itself (it holds: {}){}; a module can delegate less than it has, never more",
                    g.importer,
                    g.target,
                    domain,
                    shown,
                    if held.get(*domain).map(|p| p.is_empty()).unwrap_or(true) {
                        format!(
                            "; '{}' holds nothing for '{}' because the module that imports it wrote no 'caps({} = [...])' or `caps(*)` grant on that `impl` line",
                            g.importer, domain, domain
                        )
                    } else {
                        String::new()
                    }
                ),
                g.span,
                g.importer.clone(),
            ));
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
                // When the importer holds nothing, the message so far names
                // *it* and leaves the reader looking at the wrong module: the
                // line to change is the `impl` in whoever imported this
                // module, which wrote no grant at all.
                let missing = if held.is_empty() {
                    format!(
                        "; '{}' holds nothing here because the module that imports it wrote no 'caps({} = [...])' grant on that `impl` line",
                        g.importer, g.domain
                    )
                } else {
                    String::new()
                };
                return Err(Diagnostic::new(
                    format!(
                        "module '{}' cannot grant '{}' access to '{}': it does not hold that pattern itself (held: {}){}; a module can delegate less than it has, never more",
                        g.importer,
                        g.domain,
                        pattern,
                        if held.is_empty() {
                            "none".to_string()
                        } else {
                            held.join(", ")
                        },
                        missing
                    ),
                    g.span,
                    g.importer.clone(),
                ));
            }
        }
    }

    // Which of the shipped items are gated: a module that gates a domain
    // contributes **every item it declares**, its `extern fun` primitives and
    // its receiver methods included. The domain gate is what decides whether
    // the name resolves at all, and the resource patterns are the runtime
    // half, so an item that is left out of this table is reachable by any
    // module, granted or not: that was the `*_raw` bypass (`BUG-wQ9tL`) and
    // the reason a receiver method of a gated module reached its syscall with
    // no grant. The declaring module's own calls stay exempt in
    // `infer.rs::check_capability` by prefix, which is what lets a gated
    // module wrap its own primitive. (Overloaded names only exist in
    // `std/math`, which is ungated, so the plain key is enough.)
    let mut gated = HashMap::new();
    for m in &loader.modules {
        let Some(domain) = domain_of_with_extern(&m.id, &loader.extern_modules) else {
            continue;
        };
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            // A method's key is `Type#name` and the *receiver type* carries
            // the module prefix (`infer.rs::sig_key`, and the prefixing loop
            // above); a free function's key is its prefixed name.
            let key = match &f.receiver {
                Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                None => format!("{}{}", m.prefix, f.name),
            };
            gated.insert(key, domain.clone());
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
        extern_modules: loader.extern_modules,
    };
    Ok(Loaded {
        program,
        project,
        deprecations: loader.deprecations,
    })
}

struct Loader<'a> {
    src_dir: PathBuf,
    /// Module id of the entry module — the only one with an empty prefix.
    entry: String,
    /// The entry module's text, when it comes from a buffer (`kelpc check
    /// --text`, MISSING-b6Ytr) rather than from disk. Taken the first time
    /// the entry module is read, so a re-expansion round reads the cached
    /// tokens instead of consuming it twice.
    entry_text: Option<String>,
    /// `<project root>/alias.kpconf`, whether or not it exists.
    kpconf_path: PathBuf,
    /// The manifest's `http_pool` ceiling (S4), or the compiler default when
    /// there is no manifest. Threaded into the loader because the value is
    /// spliced into `std/http`'s *source text* before it is lexed — the one
    /// configuration value the shipped stdlib reads.
    http_pool: usize,
    sources: &'a mut SourceMap,
    /// The one flat, global alias table: `alias.kpconf` plus every inline
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
    /// Every `caps(...)` seen while loading, in load order (M7). Kept
    /// until the whole graph is known, because a module's own set is the
    /// union of the grants made *to* it by importers loaded from anywhere.
    grants: Vec<GrantEdge>,
    /// Every `caps(*)` seen while loading, in load order. Kept apart from
    /// `grants` because the wildcard is not one pattern: it delegates the
    /// whole resource of every domain at once, its refusal names that set
    /// rather than a single pattern, and it is refused outright for a module
    /// that reaches no domain at all.
    all_grants: Vec<AllGrantEdge>,
    /// Outdated syntax found while loading, in load order (`deprecated.rs`).
    deprecations: Vec<Diagnostic>,
    /// `coral.toml`'s `[extern."<module id>"]` sections (MK9-S1-M1), read once
    /// up front the same way `manifest_caps` is. Consulted beside
    /// `stdlib::domain_of` at every one of this loader's own domain checks,
    /// so a user module named here is gated exactly like a stdlib one.
    extern_modules: HashMap<String, ExternModuleDecl>,
}

/// One `impl "target" caps(domain = [patterns])` from `importer` (M7).
#[derive(Debug, Clone)]
struct GrantEdge {
    importer: String,
    target: String,
    domain: String,
    patterns: Vec<String>,
    span: Span,
}

/// One `impl "target" caps(*)` from `importer`: the whole resource of every
/// domain that `importer` holds, delegated in one clause (`syntax.md` 15.2).
#[derive(Debug, Clone)]
struct AllGrantEdge {
    importer: String,
    target: String,
    span: Span,
}

/// Bound on alias-driven re-expansion. Each discovery round only ever *adds*
/// modules, so this is a guard against a pathological case, not an expected
/// limit.
const MAX_ALIAS_ROUNDS: usize = 8;

impl Loader<'_> {
    /// Reads `alias.kpconf`, discovers the module graph, then loads it for
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
        self.load_alias_kpconf()?;

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

    /// A round is one full load attempt: the discovery pass, then the strict
    /// pass that decides the round. Everything a round produced is discarded
    /// here so that the last round is the only one anything reads — including
    /// the deprecations, which both passes collect. Leaving them in printed
    /// every outdated finding twice, once for the discovery pass and once for
    /// the strict one.
    fn reset_round(&mut self) {
        self.state.clear();
        self.stack.clear();
        self.modules.clear();
        self.prefixes.clear();
        self.parsed.clear();
        self.deprecations.clear();
    }

    fn load_alias_kpconf(&mut self) -> Result<(), Diagnostic> {
        let Ok(text) = fs::read_to_string(&self.kpconf_path) else {
            return Ok(()); // the file is optional
        };
        self.sources
            .insert(preprocess::KPCONF_MODULE, &self.kpconf_path, text.clone());
        for def in preprocess::parse_kpconf(&text)? {
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
        let (path, text) = match crate::stdlib::source_configured(id, self.http_pool) {
            Some(text) => (self.src_dir.join(id).with_extension(EXT), text),
            None => {
                let path = self.src_dir.join(format!("{id}.{EXT}"));
                // `kelpc check --text` hands the entry module's buffer in on
                // stdin (MISSING-b6Ytr): the path still names the file on
                // disk, an editor's unsaved edit does not, so an unsaved
                // buffer gets a real diagnostic instead of the last save's.
                let text = if id == self.entry {
                    match self.entry_text.take() {
                        Some(text) => text,
                        None => fs::read_to_string(&path).map_err(|_| {
                            Diagnostic::new(
                                format!("cannot find module '{id}': expected '{}'", path.display()),
                                at,
                                from,
                            )
                        })?,
                    }
                } else {
                    fs::read_to_string(&path).map_err(|_| {
                        Diagnostic::new(
                            format!("cannot find module '{id}': expected '{}'", path.display()),
                            at,
                            from,
                        )
                    })?
                };
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
            let alias = default_alias(&imp.path);
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
            if imp.caps_all {
                self.all_grants.push(AllGrantEdge {
                    importer: id.to_string(),
                    target: imp.path.clone(),
                    span: imp.span,
                });
            }
            let target = imp.path.clone();
            aliases.insert(alias, target.clone());
            self.visit(&target, imp.span, id, strict)?;
            // MK7 §8: importing a module that holds a capability domain is what
            // makes generated code call the capability matcher, and
            // `std/path`'s own `normalize` calls the normalizer; both live in
            // Kelp (`std/rt`) rather than in C text emitted into the program.
            // Injecting it at the import that needs it — keyed on the same
            // conditions as the calls themselves — is what keeps the runtime out
            // of programs that use neither, and what makes the module the
            // emitted C calls into always present.
            if domain_of_with_extern(&target, &self.extern_modules).is_some() || target == "std/path" {
                self.visit(crate::stdlib::RT_MODULE, imp.span, id, strict)?;
            }
        }

        self.stack.pop();
        self.state.insert(id.to_string(), 2);

        // Dependencies are pushed first (post-order), so by the time a
        // module is pushed here every prefix it needs is already known.
        let prefix = if id == self.entry {
            String::new()
        } else {
            crate::stdlib::key_prefix(id)
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

/// The namespace a module is reached by: the last path segment.
///
/// `impl "shapes/circle"` is `circle`, `impl "std/fs"` is `fs`.
///
/// A **dependency's entry module** is the exception: Coral copies a dependency
/// to `src/deps/<key>/` with `lib.kp` as the entry file, so a path ending in
/// `/lib` under `deps/` is named after the directory it sits in, which is the
/// `[dependencies]` key (`impl "deps/argparse/lib"` is `argparse`) or, for a
/// whole-repository dependency, the library's own folder inside it
/// (`impl "deps/fpl/json/lib"` is `json`). Everything else under `deps/` is an
/// internal module and keeps its own name.
///
/// The `deps/` prefix is what makes this coral's rule rather than the
/// language's: `src/deps/` is the directory Coral fetches into, and a file
/// named `lib.kp` elsewhere in a project is just a file.
fn default_alias(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("deps/") {
        if let Some(directory) = rest.strip_suffix("/lib") {
            if let Some(name) = directory.rsplit('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
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
        entry: format!("src/main.{EXT}"),
        http_pool: crate::stdlib::DEFAULT_HTTP_POOL,
        naming_case: None,
        caps: None,
        extern_modules: HashMap::new(),
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
        // A `[section]` header hands the rest of that section to Coral:
        // `name`, `version`, `entry`, `http_pool` and `naming_case` must
        // appear before the first one. `[caps]` (C-2) and `[extern."<module
        // id>"]` (MK9-S1-M1, roadmap B-1) are the sections kelpc itself
        // reads — the entry module's own capability grant, and a user
        // module's own domain/library/symbol assignment — so they do not
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
                    "{MANIFEST_NAME}'s '[extern.\"{module_id}\"]' section is missing 'domain = \"<domain>\"' — v1 has {}",
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
    Ok(manifest)
}

struct Manifest {
    name: Option<String>,
    version: Option<String>,
    /// Path relative to the project root; `kelpc build <dir>` uses it.
    entry: String,
    /// `http_pool = <n>` (S4): kept-alive HTTP connections per `host:port`.
    /// `0` disables pooling. Absent means `stdlib::DEFAULT_HTTP_POOL`. A
    /// loose `.kp` file with no manifest always gets the default.
    http_pool: usize,
    /// `naming_case = <0|1>`: the case `kelpc lint` asks the project's own
    /// names to be written in (`syntax.md` 19). `None` means the manifest
    /// says nothing about it, which the lint reads as the option's default.
    naming_case: Option<usize>,
    /// `[caps]` (C-2): the entry module's own capability grant, domain ->
    /// patterns, in the same vocabulary a `caps(domain = [...])` clause on an
    /// `impl` line uses. `None` means the section is absent, which keeps the
    /// long-standing default — the entry module holds the whole resource of
    /// every domain (`/**`/`*`) — so a `coral.toml` written before this
    /// section existed, or one that simply omits it, behaves exactly as
    /// before. `Some(map)` may still omit a domain entirely: that domain is
    /// then *not* granted to the entry module at all, a resolution error at
    /// any reference to it, the same as an ordinary ungranted module.
    caps: Option<HashMap<String, Vec<String>>>,
    /// `[extern."<module id>"]` (MK9-S1-M1, roadmap B-1): every user module
    /// this manifest assigns a domain, native library and symbol allowlist
    /// to. Empty when the manifest has none, which behaves exactly as before
    /// this feature existed: a module with no entry here gets no domain from
    /// it, and its `extern fun` declarations stay refused as before.
    extern_modules: HashMap<String, ExternModuleDecl>,
}

/// One `domain = ["pattern", ...]` line inside `coral.toml`'s `[caps]`
/// section (C-2). Reuses `stdlib::is_domain` and `validate_cap_pattern`, the
/// same vocabulary an `impl "..." caps(domain = [...])` clause validates its
/// grants against, so a pattern that would be refused on an `impl` line is
/// refused here too, for the same reason and with the same wording.
fn parse_caps_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((domain, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!(
                "expected 'domain = [\"pattern\", ...]' in {MANIFEST_NAME}'s [caps] section, found '{line}'"
            ),
            at,
            "",
        ));
    };
    let domain = domain.trim();
    let value = value.trim();
    if !crate::stdlib::is_domain(domain) {
        return Err(Diagnostic::new(
            format!(
                "'{domain}' is not a capability domain — v1 has {}",
                crate::stdlib::DOMAINS.join(", ")
            ),
            at,
            "",
        ));
    }
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for '[caps] {domain}' must be a bracketed list of double-quoted patterns, found '{value}'"
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
                        "{MANIFEST_NAME} pattern for '[caps] {domain}' must be double-quoted, found '{part}'"
                    ),
                    at,
                    "",
                ));
            };
            validate_cap_pattern(domain, pattern, at, "")?;
            patterns.push(pattern.to_string());
        }
    }
    let caps = manifest.caps.get_or_insert_with(HashMap::new);
    if caps.contains_key(domain) {
        return Err(Diagnostic::new(
            format!("domain '{domain}' appears more than once in {MANIFEST_NAME}'s [caps] section"),
            at,
            "",
        ));
    }
    caps.insert(domain.to_string(), patterns);
    Ok(())
}

/// One `key = value` line inside `coral.toml`'s `[extern."<module_id>"]`
/// section (MK9-S1-M1, roadmap B-1): `domain = "<one of stdlib::DOMAINS>"`,
/// `library = "<native library name>"`, or `symbols = ["<name>", ...]`. The
/// header line already inserted a default `ExternModuleDecl` for
/// `module_id` (mirrors `[caps]`'s duplicate-domain check, but at the
/// section level since a module gets at most one `[extern.*]` entry), so
/// this only ever updates it.
fn parse_extern_line(
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
                        "'{domain}' is not a capability domain — v1 has {}",
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

/// The project's `naming_case`, for `kelpc lint` (`syntax.md` 19, roadmap
/// item 23).
///
/// `None` covers every way the setting can be absent or unusable: there is no
/// manifest above `file`, the key is not there, or the file does not parse.
/// None of those is a lint diagnostic: a broken `coral.toml` is reported by
/// `kelpc build`, which is the command that runs against a project, and `lint`
/// answering the question "what case does this project write?" from a file it
/// cannot read would mean inventing an answer. The option's default is the
/// honest one.
pub fn manifest_naming_case(root: &Path) -> Option<usize> {
    let path = root.join(MANIFEST_NAME);
    let text = fs::read_to_string(&path).ok()?;
    parse_manifest(&text, &path).ok()?.naming_case
}

/// The directory holding the nearest `coral.toml` above `file`, if there is
/// one. `kelpc lint` uses it to find the settings a file is linted under,
/// which is the same walk a project build does.
pub fn find_manifest_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        if d.join(MANIFEST_NAME).is_file() {
            return Some(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Canonical module id for a file: its path relative to `src/`, minus the
/// `.kp` suffix, with `/` separators.
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
/// than it holds. `net` and `env` are name domains: the only pattern that
/// covers another one is the whole-resource `*`.
fn pattern_covered(held: &str, granted: &str, domain: &str) -> bool {
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
fn reaches_a_domain(
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
    if domain == "net" {
        // Anchored here so the net vocabulary is validated in one place.
        return validate_net_pattern(pattern)
            .map_err(|message| Diagnostic::new(message, span, module));
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

/// `net`'s vocabulary: `host`, `host:port`, `*.host`, `*:port`, `*`. The host
/// is split from the port at the **last** `:` — so a host can never contain
/// one, which makes an IPv6 literal unexpressible rather than ambiguous, and
/// the message says so. `*` matches within one label (it does not cross `.`
/// or `:`); a lone `*` is the whole-resource wildcard the entry module holds.
fn validate_net_pattern(pattern: &str) -> Result<(), String> {
    let vocabulary = "a 'net' pattern names a host and an optional port \
        ('api.example.com:443', '*.example.com', '*:8080', or '*')";
    if pattern.contains('/') || pattern.contains(char::is_whitespace) {
        return Err(format!("'{pattern}' is not a host — {vocabulary}"));
    }
    if pattern.contains("**") {
        return Err(format!(
            "'**' has no meaning in a 'net' pattern — write '*' ('{pattern}')"
        ));
    }
    let (host, port) = match pattern.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (pattern, None),
    };
    if host.is_empty() {
        return Err(format!("'{pattern}' has no host — {vocabulary}"));
    }
    if host.contains(':') {
        return Err(format!(
            "'{pattern}' has more than one ':' — an IPv6 literal is not expressible as a 'net' pattern, because the ':' is what separates the port ({vocabulary})"
        ));
    }
    if host != "*" && (host.starts_with('.') || host.ends_with('.') || host.contains("..")) {
        return Err(format!(
            "'{pattern}' is not a host name — no label of it may be empty"
        ));
    }
    if let Some(port) = port {
        if port.is_empty() {
            return Err(format!(
                "'{pattern}' names a port after ':' — write the number, or '*' for any ({vocabulary})"
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
fn validate_impl_path(path: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
    let err = |message: String| Err(Diagnostic::new(message, span, module));
    if path.is_empty() {
        return err("'impl' path must not be empty".to_string());
    }
    let ext = format!(".{EXT}");
    if path.ends_with(&ext) {
        return err(format!(
            "'impl' paths omit the '.{EXT}' extension — write \"{}\"",
            path.trim_end_matches(&ext)
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
            | "pub"
            | "alias"
            | "print"
    )
}

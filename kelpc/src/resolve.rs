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
use std::time::Instant;

use crate::ast;
use crate::diag::{Diagnostic, SourceMap};
use crate::lexer::{Lexer, Span, Token, TokenKind};
use crate::parser::Parser;
use crate::preprocess::{self, AliasTable};
use crate::timings::Timings;

/// Source-file extension, without the leading dot. Every extension decision
/// in this file goes through it, so a future rebrand is one line.
pub const EXT: &str = "kp";

/// Manifest filename at a project root. Named after the build tool (Coral),
/// not the language: the manifest is Coral's, the sources are Kelp's.
pub const MANIFEST_NAME: &str = "coral.toml";

/// Compiler-owned implementation modules live below this root. They are not a
/// second public standard library: a project may neither declare nor import one.
pub const INTERNAL_STDLIB_PREFIX: &str = "std/internal/";

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

/// One `impl "target" caps(domain = [patterns])` clause as the loader saw it:
/// the module that wrote it, the module it names, and the grant.
#[derive(Debug, Clone)]
pub struct GrantRecord {
    pub importer: String,
    pub target: String,
    pub domain: String,
    pub patterns: Vec<String>,
}

mod project;
pub(crate) use project::*;

/// One `[extern."<module id>"]` section's contents (MK9-S1-M1, roadmap B-1):
/// the native library and exact symbol set a user module's `extern fun`
/// declarations may bind to, once `domain` has also assigned it one of
/// `stdlib::DOMAINS`. `domain`/`library` are `Option` only while parsing -
/// `parse_manifest` refuses a section that finishes with either still unset,
/// so every entry a caller sees has both.
#[derive(Debug, Clone, Default)]
pub struct ExternModuleDecl {
    pub(crate) domain: Option<String>,
    /// The library `-l<name>` names on the link line.
    pub(crate) library: Option<String>,
    /// The exact native symbols this module's `extern fun` declarations may
    /// bind to, each written as it appeared in `symbols`: either
    /// `"kelp_name"` (the Kelp name *is* the symbol the removed C backend calls), or
    /// `"kelp_name=c_symbol"` when the two differ. Every entry's Kelp name is
    /// unique, so `symbol_of` resolves one unambiguously.
    pub symbols: Vec<String>,
    /// The directories `-L<dir>` names on the link line, resolved the same
    /// way.
    pub(crate) library_dirs: Vec<String>,
    /// The `pkg-config` package this section names, when it names one.
    /// `driver.rs` asks `pkg-config` for its flags and passes them to `cc`
    /// on both the compile and the link line.
    pub(crate) pkg_config: Option<String>,
}

impl ExternModuleDecl {
    /// The native library this module's symbols are declared to live in.
    pub fn library(&self) -> &str {
        self.library.as_deref().unwrap_or_default()
    }

    /// The library directories this section named, as written.
    pub fn library_dirs(&self) -> &[String] {
        &self.library_dirs
    }

    /// The `pkg-config` package this section named, if it named one.
    pub fn pkg_config(&self) -> Option<&str> {
        self.pkg_config.as_deref()
    }

    /// The C symbol the `extern fun` whose bare Kelp name is `kelp` binds to,
    /// or `None` when this module's allowlist does not name it. `infer.rs`
    /// matches the allowlist on the Kelp name and reads the C symbol from the
    /// same entry.
    pub fn symbol_of(&self, kelp: &str) -> Option<&str> {
        self.symbols.iter().find_map(|entry| {
            let (kelp_name, symbol) = entry.split_once('=').unwrap_or((entry, entry));
            (kelp_name == kelp).then_some(symbol)
        })
    }
}

/// `stdlib::domain_of`, folded with a project's own `[extern.*]` table
/// (MK9-S1-M1). Every caller is a loader domain check (the `gated` map, the
/// RT-module import injection, and `caps(*)`'s reachability walk), so a user
/// module is gated exactly like a stdlib one at each of them, not just some.
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

    /// The `[dependencies]` entries of `coral.toml`, in manifest order.
    pub fn dependencies(&self) -> &[DependencyDecl] {
        &self.dependencies
    }

    /// Every `caps(domain = [...])` clause, in load order.
    pub fn grants(&self) -> &[GrantRecord] {
        &self.grants
    }

    /// Every `caps(*)` clause as (importer, target), in load order.
    pub fn wildcard_grants(&self) -> &[(String, String)] {
        &self.wildcard_grants
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
    /// Compiler-generated public contracts in dependency order. Slice 4 uses
    /// these hashes to decide which modules a future front-end cache may reuse.
    #[allow(dead_code)] // The next cache slice consumes this after artifact I/O lands.
    pub interfaces: Vec<ModuleInterface>,
}

/// One module's generated front-end contract. `api_hash` is its own public
/// surface; `hash` additionally commits to the direct contracts it imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInterface {
    pub module: String,
    pub api_hash: String,
    pub hash: String,
    pub dependencies: Vec<(String, String)>,
}

mod loader;
pub(crate) use loader::*;

mod loader_visit;

/// Builds module contracts after discovery, whose post-order guarantees every
/// imported module's interface hash is available before its importer needs it.
fn build_interfaces(modules: &[ModuleInfo], sources: &SourceMap) -> Vec<ModuleInterface> {
    let mut hashes: HashMap<String, String> = HashMap::new();
    let mut interfaces = Vec::with_capacity(modules.len());
    for module in modules {
        let text = sources
            .text(&module.id)
            .expect("every loaded module has registered source text");
        let api_hash = interface_api_hash(&module.id, text);
        let mut dependencies: Vec<String> = module
            .aliases
            .values()
            .cloned()
            .collect::<Vec<_>>();
        dependencies.sort();
        dependencies.dedup();
        let dependencies: Vec<(String, String)> = dependencies
            .into_iter()
            .map(|id| {
                let hash = hashes
                    .get(&id)
                    .expect("module loading visits imports before importers")
                    .clone();
                (id, hash)
            })
            .collect();
        let mut input = format!("kelpc-front-end-interface-1\nmodule={}\napi={}\n", module.id, api_hash);
        for (id, hash) in &dependencies {
            input.push_str(&format!("dependency={id}:{hash}\n"));
        }
        let hash = crate::sha256::sha256_hex(input.as_bytes());
        hashes.insert(module.id.clone(), hash.clone());
        interfaces.push(ModuleInterface {
            module: module.id.clone(),
            api_hash,
            hash,
            dependencies,
        });
    }
    interfaces
}

/// The stable public surface of one module, intentionally excluding function
/// bodies and doc comments. `doc::scan` already knows how to isolate formatted
/// declaration headers while balancing arbitrary bodies, so reusing it keeps
/// this cache boundary aligned with the documentation tool instead of creating
/// a second partial parser.
fn interface_api_hash(module: &str, text: &str) -> String {
    let scan = crate::doc::scan(text).expect("a loaded module was already lexed successfully");
    let public_types: HashSet<String> = scan
        .items
        .iter()
        .filter(|item| item.is_pub && matches!(item.kind, crate::doc::Kind::Datac | crate::doc::Kind::Enum | crate::doc::Kind::Trait | crate::doc::Kind::ExternType))
        .map(|item| item.name.clone())
        .collect();
    let mut surface = format!("module={module}\n");
    for import in &scan.imports {
        surface.push_str(&format!("import={}\n", import.text));
    }
    for item in &scan.items {
        let public_method = item
            .receiver
            .as_ref()
            .map(|receiver| public_types.contains(receiver))
            .unwrap_or(false);
        if !item.is_pub && !public_method {
            continue;
        }
        surface.push_str(&format!("item={:?}:{}:{}:{}:{}:{}\n", item.kind, item.name, item.sig, item.unsafe_, item.extern_, item.body_domains.iter().cloned().collect::<Vec<_>>().join(",")));
        for member in &item.members {
            surface.push_str(&format!("member={:?}:{}:{}\n", member.kind, member.name, member.sig));
        }
    }
    for imp in &scan.impls {
        surface.push_str(&format!("trait_impl={}:{}\n", imp.trait_name, imp.ty));
        for member in &imp.members {
            surface.push_str(&format!("impl_member={:?}:{}:{}\n", member.kind, member.name, member.sig));
        }
    }
    // A dependency's body can reach an importer's checked result through four
    // routes the signature text does not show (`kelp issue #244`,
    // design pass): generic templates, passdown modes, folded constants and the
    // drop set. The scan keeps no bodies, so a module that declares or touches
    // any of them contributes its whole text. That is conservative: it can only
    // invalidate more, never reuse wrongly.
    if leaks_body(text, &scan) {
        surface.push_str(&format!("body={}\n", crate::sha256::sha256_hex(text.as_bytes())));
    }
    crate::sha256::sha256_hex(surface.as_bytes())
}

/// Whether a private edit in `text` could change what an importer checks to:
/// the module declares a generic item, a `const`, a `static`, a `pd`, or a
/// destructor. Word matching over the source, so a false positive only costs a
/// cache miss.
fn leaks_body(text: &str, scan: &crate::doc::Scan) -> bool {
    let generic = scan.items.iter().any(|item| {
        let head = item.sig.split('(').next().unwrap_or("");
        head.contains('<')
    });
    generic
        || text
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| matches!(word, "const" | "static" | "pd" | "tl" | "drop"))
}

pub(crate) struct Loader<'a> {
    pub(crate) src_dir: PathBuf,
    /// Module id of the entry module - the only one with an empty prefix.
    pub(crate) entry: String,
    /// The entry module's text, when it comes from a buffer (`kelpc check
    /// --text`, MISSING-b6Ytr) rather than from disk. Taken the first time
    /// the entry module is read, so a re-expansion round reads the cached
    /// tokens instead of consuming it twice.
    pub(crate) entry_text: Option<String>,
    /// The `[alias]` entries of `coral.toml`, already checked by
    /// `parse_manifest`. Defined into the table before any module is read.
    pub(crate) manifest_alias: Vec<preprocess::AliasDef>,
    /// The manifest's path and text, when it has `[alias]` entries. Registered
    /// with the source map so a diagnostic about one entry can show its line.
    pub(crate) manifest_alias_source: Option<(PathBuf, String)>,
    /// The manifest's `http_pool` ceiling (S4), or the compiler default when
    /// there is no manifest. Threaded into the loader because the value is
    /// spliced into `std/http`'s *source text* before it is lexed - the one
    /// configuration value the shipped stdlib reads.
    pub(crate) http_pool: usize,
    pub(crate) sources: &'a mut SourceMap,
    /// The one flat, global alias table: the manifest's `[alias]` entries plus
    /// every inline declaration in the project.
    pub(crate) aliases: AliasTable,
    /// Raw, unexpanded token streams per module, kept so a later round can
    /// re-expand against a table that has grown.
    pub(crate) raw: HashMap<String, Vec<Token>>,
    pub(crate) parsed: HashMap<String, ast::Program>,
    /// 1 = currently being loaded (on `stack`), 2 = finished.
    pub(crate) state: HashMap<String, u8>,
    pub(crate) stack: Vec<String>,
    pub(crate) modules: Vec<ModuleInfo>,
    pub(crate) prefixes: HashMap<String, String>,
    /// Every `caps(...)` seen while loading, in load order (M7). Kept
    /// until the whole graph is known, because a module's own set is the
    /// union of the grants made *to* it by importers loaded from anywhere.
    pub(crate) grants: Vec<GrantEdge>,
    /// Every `caps(*)` seen while loading, in load order. Kept apart from
    /// `grants` because the wildcard is not one pattern: it delegates the
    /// whole resource of every domain at once, its refusal names that set
    /// rather than a single pattern, and it is refused outright for a module
    /// that reaches no domain at all.
    pub(crate) all_grants: Vec<AllGrantEdge>,
    /// Outdated syntax found while loading, in load order (`deprecated.rs`).
    pub(crate) deprecations: Vec<Diagnostic>,
    /// `coral.toml`'s `[extern."<module id>"]` sections (MK9-S1-M1), read once
    /// up front the same way `manifest_caps` is. Consulted beside
    /// `stdlib::domain_of` at every one of this loader's own domain checks,
    /// so a user module named here is gated exactly like a stdlib one.
    pub(crate) extern_modules: HashMap<String, ExternModuleDecl>,
    /// `coral.toml`'s `[unsafe] modules` list (MK9-S2 stage 1): the module ids
    /// allowed to contain `unsafe`. Empty when the section is absent, which
    /// keeps a project that never writes `unsafe` behaving as before.
    pub(crate) unsafe_modules: Vec<String>,
}

/// One `impl "target" caps(domain = [patterns])` from `importer` (M7).
#[derive(Debug, Clone)]
pub(crate) struct GrantEdge {
    pub(crate) importer: String,
    pub(crate) target: String,
    pub(crate) domain: String,
    pub(crate) patterns: Vec<String>,
    pub(crate) span: Span,
}

/// One `impl "target" caps(*)` from `importer`: the whole resource of every
/// domain that `importer` holds, delegated in one clause (`syntax.md` 15.2).
#[derive(Debug, Clone)]
pub(crate) struct AllGrantEdge {
    pub(crate) importer: String,
    pub(crate) target: String,
    pub(crate) span: Span,
}

/// Bound on alias-driven re-expansion. Each discovery round only ever *adds*
/// modules, so this is a guard against a pathological case, not an expected
/// limit.
const MAX_ALIAS_ROUNDS: usize = 8;

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
pub(crate) fn default_alias(path: &str) -> String {
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

/// The pure std modules a file names (#300), in table order. A file gets one
/// of them in scope without an `impl` line only when it writes the name: an
/// alias followed by a dot (`str.len`), the bare `Option` or `Result`, or a
/// `derive(...)` clause that lists `text`. A local that shares the name does
/// not count, because a local is never a use of the module. Loading only the
/// named ones keeps every other program's module set exactly as it was, which
/// the freestanding build depends on: it refuses a hosted module that is
/// present in the program.
pub(crate) fn named_default_modules(tokens: &[Token]) -> Vec<&'static str> {
    let mut found: HashSet<&'static str> = HashSet::new();
    for (i, token) in tokens.iter().enumerate() {
        let TokenKind::Ident(name) = &token.kind else {
            continue;
        };
        let next = tokens.get(i + 1).map(|t| &t.kind);
        if matches!(next, Some(TokenKind::Dot)) {
            if let Some(module) = crate::stdlib::DEFAULT_MODULES
                .iter()
                .find(|module| default_alias(module) == *name)
            {
                found.insert(module);
            }
        }
        if let Some(module) = crate::stdlib::default_type_module(name) {
            found.insert(module);
        }
        if name == "derive" && matches!(next, Some(TokenKind::LParen)) {
            // `derive(eq, text)`: the clause runs to its closing parenthesis.
            for inner in &tokens[i + 2..] {
                match &inner.kind {
                    TokenKind::RParen => break,
                    TokenKind::Ident(word) if word == "text" => {
                        found.insert(crate::stdlib::FMT_MODULE);
                    }
                    _ => {}
                }
            }
        }
    }
    crate::stdlib::DEFAULT_MODULES
        .iter()
        .copied()
        .filter(|module| found.contains(module))
        .collect()
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
        // `impl "a", "b"` (#299): every path of the list, each one a module.
        let mut at = i + 1;
        while let Some(Token {
            kind: TokenKind::StringLit(path),
            span,
        }) = tokens.get(at)
        {
            imports.push((path.clone(), *span));
            match tokens.get(at + 1) {
                Some(Token {
                    kind: TokenKind::Comma,
                    ..
                }) => at += 2,
                _ => break,
            }
        }
    }
    imports
}

mod manifest;
pub(crate) use manifest::*;

/// One `[dependencies]` entry of `coral.toml`. `caps` is `None` for the plain
/// string form (`name = "fpl/name"`), which carries no extra check, and the
/// allowed domains for `name = { source = "...", caps = ["ui"] }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDecl {
    pub key: String,
    pub caps: Option<Vec<String>>,
}

/// One `[dependencies]` line: `key = "source"` or
/// `key = { source = "...", caps = ["domain", ...] }`. Only the `caps` list is
/// read; `source` is Coral's to check, and any other key of the inline table
/// is Coral's too, so it is skipped rather than refused. A `caps` value that is
/// not a list of known domains is refused by name, because a manifest that
/// writes one has asked for the check and must get it.
fn parse_dependency_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Ok(());
    };
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || key.contains(['"', '{', '}', '[', ']']) {
        return Ok(());
    }
    let caps = if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
        let mut found = None;
        for part in split_top_level(inner) {
            let Some((field, field_value)) = part.split_once('=') else {
                continue;
            };
            if field.trim() != "caps" {
                continue;
            }
            found = Some(parse_dependency_caps(key, field_value.trim(), at)?);
        }
        found
    } else {
        None
    };
    if manifest.dependencies.iter().any(|d| d.key == key) {
        return Err(Diagnostic::new(
            format!("dependency '{key}' appears more than once in {MANIFEST_NAME}'s [dependencies] section"),
            at,
            "",
        ));
    }
    manifest.dependencies.push(DependencyDecl {
        key: key.to_string(),
        caps,
    });
    Ok(())
}

/// Splits `a = 1, b = ["x", "y"]` at the commas outside brackets and quotes.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => depth += 1,
            ']' if !quoted => depth = depth.saturating_sub(1),
            ',' if !quoted && depth == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        parts.push(rest);
    }
    parts
}

/// The `["domain", ...]` list of a `caps` field in a `[dependencies]` entry.
fn parse_dependency_caps(key: &str, value: &str, at: Span) -> Result<Vec<String>, Diagnostic> {
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(Diagnostic::new(
            format!(
                "{MANIFEST_NAME} value for dependency '{key}' caps must be a bracketed list of domain names, found '{value}'"
            ),
            at,
            "",
        ));
    };
    let mut domains: Vec<String> = Vec::new();
    for part in split_top_level(inner) {
        let domain = part
            .strip_prefix('"')
            .and_then(|p| p.strip_suffix('"'))
            .ok_or_else(|| {
                Diagnostic::new(
                    format!(
                        "{MANIFEST_NAME} caps entry for dependency '{key}' must be a double-quoted domain name, found '{part}'"
                    ),
                    at,
                    "",
                )
            })?;
        if !crate::stdlib::is_domain(domain) {
            return Err(Diagnostic::new(
                format!(
                    "'{domain}' is not a capability domain, so it cannot be in dependency '{key}' caps; v1 has {}",
                    crate::stdlib::DOMAINS.join(", ")
                ),
                at,
                "",
            ));
        }
        if !domains.iter().any(|d| d == domain) {
            domains.push(domain.to_string());
        }
    }
    Ok(domains)
}

mod manifest_keys;
pub(crate) use manifest_keys::*;

/// Sets one optional `[target]` key, refusing a second assignment so a
/// duplicated key cannot silently keep the first or the last value.
fn set_target_key(
    slot: &mut Option<String>,
    value: &str,
    key: &str,
    at: Span,
) -> Result<(), Diagnostic> {
    if slot.is_some() {
        return Err(Diagnostic::new(
            format!("'{key}' appears more than once in {MANIFEST_NAME}'s [target] section"),
            at,
            "",
        ));
    }
    *slot = Some(value.to_string());
    Ok(())
}

/// Whether `name` is a bare Kelp identifier: a letter or `_` followed by
/// letters, digits and `_`. Used for the `[target]` keys that name a function,
/// so a manifest cannot smuggle a path or a module qualification through them.
fn is_bare_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

mod manifest_targets;
pub(crate) use manifest_targets::*;

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


mod subst;
pub(crate) use subst::*;

mod caps;
pub(crate) use caps::*;

#[cfg(test)]
#[path = "resolve/tests.rs"]
mod tests;

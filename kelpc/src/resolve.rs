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

/// Canonical module id for a file: its path relative to `src/`, minus the
/// `.kp` suffix, with `/` separators.
/// A map from an associated type's name to the type an impl bound it to, as
/// written (A-12 slice 5), used to substitute `Self::Item` in an impl method.
fn assoc_bindings(bindings: &[ast::AssocTypeBinding]) -> std::collections::HashMap<String, ast::TypeExpr> {
    bindings
        .iter()
        .map(|b| (b.name.clone(), b.ty.clone()))
        .collect()
}

/// One `impl` line's trait argument (A-12 slice 6), canonicalized the way the
/// `for` type is: a builtin keeps its name, and a declared `datac`/`enum` is
/// qualified with its module prefix. A trait argument has to be a named type or
/// a builtin, so a container, a tuple, an array, a borrow, a function type and
/// a projection are refused here, where the argument was written.
fn canon_trait_arg(
    a: &ast::TypeExpr,
    canon: &dyn Fn(&str, Span) -> Result<String, Diagnostic>,
    module: &str,
) -> Result<String, Diagnostic> {
    let simple = a.tuple.is_none()
        && a.array_elem.is_none()
        && a.fn_params.is_none()
        && a.dyn_trait.is_none()
        && a.assoc.is_none()
        && a.args.is_empty()
        && !a.amp
        && !a.name.is_empty();
    if !simple {
        return Err(Diagnostic::new(
            format!(
                "a trait argument has to be a named type or a builtin, not '{}': a container, tuple, array, borrow, function type or projection as a trait's argument is not in this slice",
                a.display()
            ),
            a.span,
            module.to_string(),
        ));
    }
    if crate::types::Ty::from_name(&a.name).is_some() {
        return Ok(a.name.clone());
    }
    canon(&a.name, a.span)
}

/// Replaces the trait's own type parameters with the impl's arguments in a
/// synthesized default method's signature (A-12 slice 6): the trait wrote
/// `value: T`, and the copy has to write the concrete type the impl named.
fn substitute_type_params(f: &mut ast::FunctionDecl, params: &[(String, ast::TypeExpr)]) {
    for (name, repl) in params {
        for p in &mut f.params {
            substitute_named_ty(&mut p.ty, name, repl);
        }
        if let Some(t) = &mut f.return_type {
            substitute_named_ty(t, name, repl);
        }
    }
}

/// One named type anywhere in a type expression replaced by another, for the
/// trait-parameter substitution above. A `Name::Item` projection and a name
/// that already carries arguments are left alone.
fn substitute_named_ty(ty: &mut ast::TypeExpr, name: &str, repl: &ast::TypeExpr) {
    if ty.name == name
        && ty.assoc.is_none()
        && ty.args.is_empty()
        && ty.tuple.is_none()
        && ty.array_elem.is_none()
        && ty.fn_params.is_none()
    {
        *ty = repl.clone();
        return;
    }
    if let Some(elems) = &mut ty.tuple {
        for e in elems {
            substitute_named_ty(e, name, repl);
        }
    }
    for a in &mut ty.args {
        substitute_named_ty(a, name, repl);
    }
    if let Some(elem) = &mut ty.array_elem {
        substitute_named_ty(elem, name, repl);
    }
    if let Some(params) = &mut ty.fn_params {
        for p in params {
            substitute_named_ty(&mut p.ty, name, repl);
        }
    }
    if let Some(ret) = &mut ty.fn_ret {
        substitute_named_ty(ret, name, repl);
    }
}

/// Replaces every `Self` in one `impl` method's signature with the written
/// `for` type (A-12 slice 2). A method inside an `impl` block names its
/// receiver `Self` so the block reads the same wherever it sits, and this is
/// what turns it into the ordinary receiver method every later pass expects.
/// Only a *type* named `Self` is replaced: an expression naming `Self` is left
/// alone, and `infer.rs` refuses it there as an unknown name. A `Self::Item`
/// (A-12 slice 5) becomes the type the impl bound, so the signature reaches
/// the type checker concrete.
fn substitute_self(
    f: &mut ast::FunctionDecl,
    self_type: &str,
    assoc: &std::collections::HashMap<String, ast::TypeExpr>,
) {
    if let Some(r) = &mut f.receiver {
        if r.type_name == "Self" {
            r.type_name = self_type.to_string();
        }
    }
    for p in &mut f.params {
        substitute_self_ty(&mut p.ty, self_type, assoc);
    }
    if let Some(t) = &mut f.return_type {
        substitute_self_ty(t, self_type, assoc);
    }
}

/// One type expression's `Self`, including inside a tuple, an array, a
/// container's arguments and a function type's parameters and return.
fn substitute_self_ty(
    ty: &mut ast::TypeExpr,
    self_type: &str,
    assoc: &std::collections::HashMap<String, ast::TypeExpr>,
) {
    if ty.name == "Self" {
        if let Some(item) = ty.assoc.clone() {
            if let Some(bound) = assoc.get(&item) {
                let mut replacement = bound.clone();
                substitute_self_ty(&mut replacement, self_type, assoc);
                *ty = replacement;
            }
            return;
        }
        if ty.args.is_empty() {
            ty.name = self_type.to_string();
        }
    }
    if let Some(elems) = &mut ty.tuple {
        for e in elems {
            substitute_self_ty(e, self_type, assoc);
        }
    }
    for a in &mut ty.args {
        substitute_self_ty(a, self_type, assoc);
    }
    if let Some(elem) = &mut ty.array_elem {
        substitute_self_ty(elem, self_type, assoc);
    }
    if let Some(params) = &mut ty.fn_params {
        for p in params {
            substitute_self_ty(&mut p.ty, self_type, assoc);
        }
    }
    if let Some(ret) = &mut ty.fn_ret {
        substitute_self_ty(ret, self_type, assoc);
    }
}

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
    if id.starts_with(INTERNAL_STDLIB_PREFIX) {
        return Err(Diagnostic::new(
            format!("module '{id}' is below the compiler-reserved std/internal namespace"),
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
/// conservative here can only under-grant - never let a module hand out more
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
fn validate_cap_grant(
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
fn validate_range_grant(
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
fn whole_range_phrase(domain: &str) -> &'static str {
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
fn validate_net_pattern(pattern: &str) -> Result<(), String> {
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
fn validate_impl_path(path: &str, span: Span, module: &str) -> Result<(), Diagnostic> {
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

#[cfg(test)]
#[path = "resolve/tests.rs"]
mod tests;

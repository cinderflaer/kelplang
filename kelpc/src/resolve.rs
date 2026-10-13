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

pub struct Project {
    /// Directory containing `coral.toml`, when there is one. A loose `.kp` file
    /// with no manifest still compiles; it is then its own module root.
    #[allow(dead_code)] // not read yet - later milestones (`pd` wiring, `kelpc fmt`) will.
    pub root: Option<PathBuf>,
    /// Base directory `impl` paths are resolved against (`<root>/src`, or
    /// the entry file's own directory when there is no manifest).
    #[allow(dead_code)]
    pub src_dir: PathBuf,
    /// Module id of the entry module - the one whose `main` is the program
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
    /// module holds `/**` for every domain - the plan's "main implicitly
    /// holds the full set" - and every other module holds exactly what its
    /// importers delegated to it, never more.
    caps: HashMap<String, HashMap<String, Vec<String>>>,
    /// Per function key: the capability domain of the module that declares
    /// it, for the gated modules only. `infer.rs` gates resolution on it and
    /// the removed C backend inserts the runtime check because of it.
    gated: HashMap<String, String>,
    /// The `[dependencies]` entries of `coral.toml`, in manifest order. A
    /// dependency's modules are the ones under `deps/<key>/` (Coral copies each
    /// dependency into `src/deps/<key>/`), which is how the capability check
    /// finds them.
    dependencies: Vec<DependencyDecl>,
    /// Every `impl "target" caps(domain = [...])` clause, in load order, for
    /// `kelpc caps`'s grant lines.
    grants: Vec<GrantRecord>,
    /// Every `impl "target" caps(*)` clause as (importer, target), in load order.
    wildcard_grants: Vec<(String, String)>,
    /// Per module: its `coral.toml` `[extern."<module id>"]` entry (MK9-S1-M1,
    /// roadmap B-1), when it has one. This is what lets a *user* module's own
    /// `extern fun` declarations exist at all: `domain_of` below folds this
    /// table in beside `stdlib::domain_of`'s stdlib-only table, so a module
    /// named here is gated exactly like a stdlib module is, and `infer.rs`
    /// checks a declared `extern fun`'s bare name against the entry's
    /// `symbols` allowlist instead of refusing every non-stdlib extern
    /// outright.
    extern_modules: HashMap<String, ExternModuleDecl>,
    /// The functions written `unsafe fun` (MK9-S2 stage 1), by canonical key:
    /// the set `infer.rs` requires an `unsafe` context to call. Empty for a
    /// program that declares none.
    unsafe_fns: std::collections::HashSet<String>,
    /// The functions written `must_use fun` (MISSING-Cn2Ym), by canonical key:
    /// a call to one used as a statement is refused.
    must_use_fns: std::collections::HashSet<String>,
    /// The manifest's `[target]` section (kernel stage 4), or the hosted default
    /// when there is none. `infer.rs` reads the entry name from it, the
    /// freestanding checker its allowed set, and the driver the triple and
    /// linker script.
    target: TargetDecl,
    /// `coral.toml`'s `[build] incremental = true` (roadmap A-14 slice 1): the
    /// project wants the backend's per-unit split. `None`/absent is the
    /// default, which is the whole-program single module the compiler has
    /// always emitted.
    incremental: bool,
    /// `coral.toml`'s `[build] sanitize = "address"`: the sanitizer the C
    /// backend builds the program with. `--sanitize=` on the command line wins.
    sanitize: Option<crate::driver::Sanitizer>,
    /// `coral.toml`'s `[build] emit = "binary"|"html"`: what `kelpc build`
    /// leaves behind. `Binary` is the default.
    emit: Emit,
}

/// What `kelpc build` writes for a project (`[build] emit`). `Html` builds the
/// project's `[build] emit_entry` instead of its `entry`, runs it once, and writes
/// its stdout as `index.html` beside the binary (see `polyp.emit_html` in
/// `examples/polyp/lib.kp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emit {
    Binary,
    Html,
}

impl Project {
    /// The manifest's `[build] sanitize`, if it names one.
    pub fn sanitize(&self) -> Option<crate::driver::Sanitizer> {
        self.sanitize
    }

    /// The manifest's `[build] emit`, `Binary` when it names none.
    pub fn emit(&self) -> Emit {
        self.emit
    }

    /// The native library and symbol allowlist a `[extern."<module id>"]`
    /// section declared for `module`, if it has one. `infer.rs` checks a
    /// declared `extern fun`'s bare name against `symbols`;
    /// the removed C backend/`codegen_llvm.rs` use it to write the direct-call
    /// trampoline instead of looking the body up in `externs.rs`.
    pub fn extern_ffi_of(&self, module: &str) -> Option<&ExternModuleDecl> {
        self.extern_modules.get(module)
    }

    /// The functions declared `unsafe fun` (MK9-S2 stage 1), by canonical key.
    pub fn unsafe_fns(&self) -> &std::collections::HashSet<String> {
        &self.unsafe_fns
    }

    /// The functions declared `must_use fun`, by canonical key.
    pub fn must_use_fns(&self) -> &std::collections::HashSet<String> {
        &self.must_use_fns
    }

    /// The manifest's `[target]` section, or the hosted default (kernel stage
    /// 4). Every read of the execution environment goes through this.
    pub fn target(&self) -> &TargetDecl {
        &self.target
    }

    /// Whether the manifest asked for the backend's per-unit split (roadmap
    /// A-14 slice 1). `kelpc build` ORs this with its own `--incremental` flag.
    pub fn incremental(&self) -> bool {
        self.incremental
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

    /// Every library directory named across the project's `[extern.*]`
    /// sections, resolved against the `coral.toml` directory. The link takes
    /// each one as `-L<dir>`.
    pub fn extern_library_dirs(&self) -> Vec<String> {
        collect_extern_dirs(&self.extern_modules, self.root.as_deref(), |e| e.library_dirs())
    }

    /// Every `pkg-config` package named across the project's `[extern.*]`
    /// sections, deduplicated and sorted. `driver.rs` runs one
    /// `pkg-config --cflags --libs` invocation over the whole list.
    pub fn extern_pkg_config_packages(&self) -> Vec<String> {
        collect_extern_pkg_config(&self.extern_modules)
    }
}

fn collect_extern_pkg_config(extern_modules: &HashMap<String, ExternModuleDecl>) -> Vec<String> {
    let mut packages: Vec<String> = extern_modules
        .values()
        .filter_map(|e| e.pkg_config().map(str::to_string))
        .collect();
    packages.sort();
    packages.dedup();
    packages
}

/// The shared body of `extern_library_dirs`: each
/// section's entries are resolved against the directory holding `coral.toml`
/// (an absolute entry is left as written, since `Path::join` keeps it), then
/// the union is deduplicated and sorted.
fn collect_extern_dirs(
    extern_modules: &HashMap<String, ExternModuleDecl>,
    root: Option<&Path>,
    select: impl Fn(&ExternModuleDecl) -> &[String],
) -> Vec<String> {
    let mut dirs: Vec<String> = extern_modules
        .values()
        .flat_map(|e| select(e).iter())
        .map(|dir| match root {
            Some(root) => root.join(dir).to_string_lossy().into_owned(),
            None => dir.clone(),
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// The execution environment a `[target]` section names (kernel stage 4,
/// MK9-S4-M1). Without a `[target]` section, or with `mode = "hosted"`, a
/// project builds for the host exactly as every project before this stage did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    /// The ambient runtime: libc, the `kelp_rt` object, the hosted entry point.
    Hosted,
    /// No libc, no runtime object, a user entry point and a linker script
    /// (LLVM backend only).
    Freestanding,
}

/// One `coral.toml` `[target]` section (kernel stage 4): where the program runs.
/// `mode` is required, and the other four keys are required exactly when it is
/// `freestanding`; the validation is `parse_manifest`'s, so every `TargetDecl` a
/// caller sees is complete for its mode.
#[derive(Debug, Clone)]
pub struct TargetDecl {
    mode: TargetMode,
    triple: Option<String>,
    entry: Option<String>,
    linker_script: Option<String>,
    panic: Option<String>,
    /// The global allocator (kernel stage 7, MK9-S5-M1): the bare names of the
    /// entry module's `alloc`, `dealloc` and optional `realloc` functions. All
    /// three are `None` when the section configures no allocator, which is the
    /// behaviour every project before this stage had.
    alloc: Option<String>,
    dealloc: Option<String>,
    realloc: Option<String>,
    /// `cpus = N` (kernel stage 8, MK9-S3-M3): how many copies a `percpu`
    /// static is laid out as. 1 when the section names none, which is what a
    /// single-hart image and every project before this stage get.
    cpus: u64,
    /// `heap_max = N` (managed mode): the collected heap's cap in MiB. `None`
    /// when the section names none; `heap_max_mib` then gives the default.
    heap_max: Option<u64>,
}

impl TargetDecl {
    /// The hosted default: the mode a project with no `[target]` section runs
    /// in, byte for byte the behaviour every earlier build had.
    pub fn hosted() -> Self {
        TargetDecl {
            mode: TargetMode::Hosted,
            triple: None,
            entry: None,
            linker_script: None,
            panic: None,
            alloc: None,
            dealloc: None,
            realloc: None,
            cpus: 1,
            heap_max: None,
        }
    }

    // Read only by this module's own tests; production code asks `is_freestanding`.
    #[cfg(test)]
    pub fn mode(&self) -> TargetMode {
        self.mode
    }

    pub fn is_freestanding(&self) -> bool {
        self.mode == TargetMode::Freestanding
    }

    /// Whether the program gets the shared pool (Tidepool). Every hosted build
    /// does; a freestanding build has no mapping guarantee, so it gets none.
    pub fn has_shared_pool(&self) -> bool {
        self.mode == TargetMode::Hosted
    }

    /// The LLVM triple (`riscv64-unknown-none-elf`), freestanding only.
    pub fn triple(&self) -> Option<&str> {
        self.triple.as_deref()
    }

    /// The bare Kelp name of the entry `fun`, freestanding only.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }

    /// The linker script path, as written in the manifest (relative to the
    /// directory holding it), freestanding only.
    pub fn linker_script(&self) -> Option<&str> {
        self.linker_script.as_deref()
    }

    /// The bare Kelp name of the `fun(msg: str)` panic handler, when the
    /// section named one; the default handler is an infinite loop.
    pub fn panic(&self) -> Option<&str> {
        self.panic.as_deref()
    }

    /// The bare Kelp name of the entry module's `alloc` function, when the
    /// section configures an allocator.
    pub fn alloc(&self) -> Option<&str> {
        self.alloc.as_deref()
    }

    /// The bare Kelp name of the entry module's `dealloc` function.
    pub fn dealloc(&self) -> Option<&str> {
        self.dealloc.as_deref()
    }

    /// The bare Kelp name of the entry module's `realloc` function, when the
    /// section names one; without it, growth is alloc + copy + dealloc.
    pub fn realloc(&self) -> Option<&str> {
        self.realloc.as_deref()
    }

    /// How many copies a `percpu` static is laid out as (kernel stage 8):
    /// the manifest's `[target] cpus`, or 1 when it names none.
    pub fn cpus(&self) -> u64 {
        self.cpus
    }

    /// The managed heap's cap in MiB: the manifest's `heap_max`, or
    /// `managed::DEFAULT_HEAP_MAX_MIB` (256) when it names none.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn heap_max_mib(&self) -> u64 {
        self.heap_max.unwrap_or(crate::managed::DEFAULT_HEAP_MAX_MIB)
    }
}

/// One `[extern."<module id>"]` section's contents (MK9-S1-M1, roadmap B-1):
/// the native library and exact symbol set a user module's `extern fun`
/// declarations may bind to, once `domain` has also assigned it one of
/// `stdlib::DOMAINS`. `domain`/`library` are `Option` only while parsing -
/// `parse_manifest` refuses a section that finishes with either still unset,
/// so every entry a caller sees has both.
#[derive(Debug, Clone, Default)]
pub struct ExternModuleDecl {
    domain: Option<String>,
    /// The library `-l<name>` names on the link line.
    library: Option<String>,
    /// The exact native symbols this module's `extern fun` declarations may
    /// bind to, each written as it appeared in `symbols`: either
    /// `"kelp_name"` (the Kelp name *is* the symbol the removed C backend calls), or
    /// `"kelp_name=c_symbol"` when the two differ. Every entry's Kelp name is
    /// unique, so `symbol_of` resolves one unambiguously.
    pub symbols: Vec<String>,
    /// The directories `-L<dir>` names on the link line, resolved the same
    /// way.
    library_dirs: Vec<String>,
    /// The `pkg-config` package this section names, when it names one.
    /// `driver.rs` asks `pkg-config` for its flags and passes them to `cc`
    /// on both the compile and the link line.
    pkg_config: Option<String>,
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
    mut timings: Option<&mut Timings>,
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
        manifest_unsafe,
        manifest_target,
        manifest_incremental,
        manifest_sanitize,
        manifest_emit,
        manifest_alias,
        manifest_alias_source,
        manifest_dependencies,
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
            // `[build] emit = "html"` compiles `[build] emit_entry` (a file under
            // `src/`) in place of the manifest's entry, so the HTML program is a
            // separate module graph and its grants are its own.
            let entry_file = match (manifest.emit, &manifest.emit_entry) {
                (Emit::Html, Some(emit_entry)) => {
                    let path = given.join("src").join(emit_entry);
                    fs::canonicalize(&path).map_err(|e| {
                        Diagnostic::new(
                            format!(
                                "cannot read the [build] emit_entry point '{emit_entry}' ({}): {e}",
                                path.display()
                            ),
                            Span::new(0, 0),
                            "",
                        )
                    })?
                }
                _ => fs::canonicalize(given.join(&manifest.entry)).map_err(|e| {
                    Diagnostic::new(
                        format!(
                            "cannot read the manifest's entry point '{}': {e}",
                            manifest.entry
                        ),
                        Span::new(0, 0),
                        "",
                    )
                })?,
            };
            (
                entry_file,
                Some(given.clone()),
                given.join("src"),
                manifest.name,
                manifest.version,
                manifest.http_pool,
                manifest.caps,
                manifest.extern_modules,
                manifest.unsafe_modules,
                manifest.target,
                manifest.incremental,
                manifest.sanitize,
                manifest.emit,
                manifest.alias,
                manifest.alias_source,
                manifest.dependencies,
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
                        manifest.unsafe_modules,
                        manifest.target,
                        manifest.incremental,
                        manifest.sanitize,
                        manifest.emit,
                        manifest.alias,
                        manifest.alias_source,
                        manifest.dependencies,
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
                        Vec::new(),
                        TargetDecl::hosted(),
                        false,
                        None,
                        Emit::Binary,
                        Vec::new(),
                        None,
                        Vec::new(),
                    )
                }
            }
        };

    let entry = module_id_of(&entry_file, &src_dir)?;

    let mut loader = Loader {
        src_dir: src_dir.clone(),
        entry: entry.clone(),
        entry_text,
        manifest_alias,
        manifest_alias_source,
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
        unsafe_modules: manifest_unsafe,
    };
    let parse_start = Instant::now();
    loader.load_project()?;
    if let Some(sink) = &mut timings {
        sink.parse = parse_start.elapsed();
    }
    let resolve_start = Instant::now();

    let mut program = ast::Program {
        impls: Vec::new(),
        consts: Vec::new(),
        statics: Vec::new(),
        pds: Vec::new(),
        datacs: Vec::new(),
        enums: Vec::new(),
        extern_types: Vec::new(),
        traits: Vec::new(),
        trait_impls: Vec::new(),
        functions: Vec::new(),
        outdated_returns: Vec::new(),
    };
    // `alias -> the target module's symbol prefix`, per module. Built before
    // the merge loop because an `impl` line's two names are qualified with it
    // (A-12 slice 2): the trait and the `for` type may each be this module's
    // own or reached through an alias, and the canonical key is what every
    // later pass looks a trait up by.
    let mut alias_prefixes = HashMap::new();
    for m in &loader.modules {
        let mut table = HashMap::new();
        for (alias, target) in &m.aliases {
            let prefix = loader.prefixes.get(target).cloned().unwrap_or_default();
            table.insert(alias.clone(), prefix);
        }
        alias_prefixes.insert(m.id.clone(), table);
    }

    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        let module_alias_prefixes = alias_prefixes.get(&m.id).cloned().unwrap_or_default();
        // A written type name as its canonical, module-qualified key: a bare
        // name is this module's own, and `alias.Item` goes through the import
        // table: exactly the rule `TypeScope::qualify` applies, but at the
        // point the name was written.
        let canon = |name: &str, span: Span| -> Result<String, Diagnostic> {
            match name.split_once('.') {
                Some((alias, item)) => match module_alias_prefixes.get(alias) {
                    Some(p) => Ok(format!("{p}{item}")),
                    None => Err(Diagnostic::new(
                        format!("unknown module alias '{alias}' in type '{name}': was it imported?"),
                        span,
                        m.id.clone(),
                    )),
                },
                None => Ok(format!("{}{}", m.prefix, name)),
            }
        };
        // `unsafe` is auditable, not ambient (kernel-path rule 1): a module may
        // contain an `unsafe` block or an `unsafe fun` only if `coral.toml`
        // lists it, the same shape `[extern.*]` takes. A project with no
        // `[unsafe]` section lists nothing, so it cannot contain `unsafe` at
        // all and behaves exactly as before this stage. A module *shipped with
        // the compiler* is admitted by the compiler's own short list instead
        // (`stdlib::UNSAFE_MODULES`, kernel stage 13): no project should have to
        // write `"std/arena"` to import it, and only the compiler's source can
        // add that entry, so a dependency still cannot grant itself the power.
        if let Some((span, _)) = ast::first_unsafe(parsed) {
            if !loader.unsafe_modules.contains(&m.id)
                && !crate::stdlib::may_use_unsafe(&m.id)
            {
                return Err(Diagnostic::new(
                    format!(
                        "module '{}' is not listed in coral.toml [unsafe] modules",
                        m.id
                    ),
                    span,
                    m.id.clone(),
                ));
            }
        }
        for c in &parsed.consts {
            let mut c = c.clone();
            c.name = format!("{}{}", m.prefix, c.name);
            c.module = m.id.clone();
            program.consts.push(c);
        }
        for s in &parsed.statics {
            let mut s = s.clone();
            s.name = format!("{}{}", m.prefix, s.name);
            s.module = m.id.clone();
            program.statics.push(s);
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
            // is addressed as `Enum.Variant` at the surface - the same
            // `Type.member` shape a method call has, which is what makes
            // construction parse without a new expression form (A-1).
            e.name = format!("{}{}", m.prefix, e.name);
            e.module = m.id.clone();
            program.enums.push(e);
        }
        // An opaque handle is a module's own type, prefixed and exported like
        // a `datac`. It is legal only where a native surface exists to reach a
        // symbol with: a project's own `[extern.*]` section, or a module the
        // compiler ships (`stdlib::source`), whose extern bodies come from
        // `externs.rs` and need no manifest. The refusal is here, next to the
        // name resolution that would otherwise let it cross modules.
        for e in &parsed.extern_types {
            if !loader.extern_modules.contains_key(&m.id)
                && crate::stdlib::source(&m.id).is_none()
            {
                return Err(Diagnostic::new(
                    format!(
                        "'extern type {}' is only legal in a module with a coral.toml '[extern.\"{}\"]' section",
                        e.name, m.id
                    ),
                    e.span,
                    m.id.clone(),
                ));
            }
            let mut e = e.clone();
            e.name = format!("{}{}", m.prefix, e.name);
            e.module = m.id.clone();
            program.extern_types.push(e);
        }
        for t in &parsed.traits {
            let mut t = t.clone();
            t.name = format!("{}{}", m.prefix, t.name);
            t.module = m.id.clone();
            program.traits.push(t);
        }
        for i in &parsed.trait_impls {
            let mut i = i.clone();
            i.trait_name = canon(&i.trait_name, i.trait_span)?;
            // A builtin `for` type has no module to be prefixed with, and
            // `Ty::from_name` is exactly the set of builtin names (A-12 slice
            // 3); a declared `datac`/`enum` is qualified the usual way.
            // A blanket impl's `for` type is its own parameter (`impl<T> Tr for
            // T`), which names no declared type, so it is kept as written.
            i.type_name = if i.type_params.contains(&i.type_name)
                || crate::types::Ty::from_name(&i.type_name).is_some()
            {
                i.type_name.clone()
            } else {
                canon(&i.type_name, i.type_span)?
            };
            // `impl From<IoErr> for AppErr` (A-12 slice 6): each trait argument
            // is a named type or a builtin, canonicalized the same way. The
            // canonical instance key is what the methods below carry, so a
            // static impl method of `From<IoErr>` and one of `From<String>` are
            // two functions rather than a name collision.
            let mut arg_names = Vec::new();
            for a in &i.trait_args {
                arg_names.push(canon_trait_arg(a, &canon, &m.id)?);
            }
            i.trait_instance =
                crate::types::trait_instance_key(&i.trait_name, &arg_names);
            i.trait_arg_names = arg_names;
            i.module = m.id.clone();
            program.trait_impls.push(i);
        }
        for f in &parsed.functions {
            let mut f = f.clone();
            match &mut f.receiver {
                // A method's key is `Type#name`, so the *receiver type* gets
                // the prefix, not the method name. An `impl` block's method is
                // the exception (A-12 slice 2): its receiver was written
                // `Self`, and the `for` type's canonical name is substituted
                // for it below, so prefixing `Self` here would name nothing.
                Some(r) if f.trait_impl.is_none() => {
                    r.type_name = format!("{}{}", m.prefix, r.type_name)
                }
                Some(_) => {}
                // A static member of a trait `impl` (A-12 slice 6) is keyed
                // from its impl site, so its bare name is kept and the
                // instance is what `infer.rs` appends.
                None if f.trait_impl.is_some() => {}
                None => f.name = format!("{}{}", m.prefix, f.name),
            }
            if let Some(mut site) = f.trait_impl.clone() {
                let written_trait = site.trait_name.clone();
                let written_type = site.self_type.clone();
                // A blanket impl's method: its receiver was written `Self` over the
                // impl's own parameter, so `Self` becomes that parameter name.
                let blanket = f
                    .receiver
                    .as_ref()
                    .is_some_and(|r| r.type_args.contains(&written_type));
                let self_type = if blanket {
                    written_type.clone()
                } else {
                    canon(&written_type, f.span)?
                };
                // A-12 slice 6: the canonical trait-instance key, computed from
                // the arguments this method's own `impl` line wrote, so two
                // instantiations of one trait for one type are two functions
                // rather than a name collision.
                let trait_key = canon(&written_trait, f.span)?;
                let mut arg_names = Vec::new();
                for a in &site.trait_args {
                    arg_names.push(canon_trait_arg(a, &canon, &m.id)?);
                }
                // A-12 slice 5: the associated types this block binds, as
                // written, so a `Self::Item` in the method's signature becomes
                // the concrete type before the signature loop resolves it.
                // Matched on the canonical instance (an associated type only
                // exists on an ordinary trait, whose instance is its name).
                let impl_decl = program.trait_impls.iter().find(|i| {
                    i.trait_name == trait_key
                        && i.type_name == self_type
                        && i.trait_arg_names == arg_names
                });
                let assoc = impl_decl
                    .map(|i| assoc_bindings(&i.assoc_types))
                    .unwrap_or_default();
                site.self_type = self_type.clone();
                site.trait_name = trait_key.clone();
                site.trait_instance = crate::types::trait_instance_key(&trait_key, &arg_names);
                f.trait_impl = Some(site);
                substitute_self(&mut f, &self_type, &assoc);
                if blanket {
                    // The receiver names the blanket impl itself, so its methods
                    // key as `impl#Trait#name` and never collide with another
                    // blanket impl's method of the same name.
                    if let Some(r) = &mut f.receiver {
                        r.type_name = crate::types::blanket_receiver_name(&trait_key);
                    }
                }
            }
            f.module = m.id.clone();
            program.functions.push(f);
        }
    }

    // A-12 slice 3: a trait default method. Every `impl Trait for Type` that
    // omits a method the trait gives a body gets a concrete copy of it, with
    // `Self` substituted and tagged with the impl site, so the ordinary
    // signature loop checks and emits it exactly as if the impl had written
    // it. The signature resolves in the impl's module (the impl wrote the
    // `for` type, so it can name it); the body resolves in the trait's module,
    // where it was written, through `body_module`. A builtin's impl is in the
    // trait's module, so the two coincide there.
    let default_sources: Vec<(String, String, String, String, String, Vec<String>, Vec<ast::FunctionDecl>, std::collections::HashMap<String, ast::TypeExpr>, Vec<(String, ast::TypeExpr)>)> = program
        .trait_impls
        .iter()
        .map(|i| {
            let trait_decl = program.traits.iter().find(|t| t.name == i.trait_name);
            let trait_module = trait_decl.map(|t| t.module.clone()).unwrap_or_default();
            // A blanket impl takes no trait default yet (infer.rs refuses it by
            // name), so none is generated for it.
            let blanket = i.type_params.contains(&i.type_name);
            let defaults: Vec<ast::FunctionDecl> = trait_decl
                .filter(|_| !blanket)
                .map(|t| {
                    t.methods
                        .iter()
                        .filter(|m| !matches!(m.body, ast::FunctionBody::Signature))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            // A-12 slice 6: the trait's own parameters, paired with the impl's
            // written arguments, so a synthesized default's signature has its
            // `T` replaced by the concrete type the impl names.
            let params: Vec<(String, ast::TypeExpr)> = trait_decl
                .map(|t| t.type_params.iter().cloned().zip(i.trait_args.iter().cloned()).collect())
                .unwrap_or_default();
            (
                i.trait_name.clone(),
                i.trait_instance.clone(),
                i.type_name.clone(),
                trait_module,
                i.module.clone(),
                i.methods.clone(),
                defaults,
                assoc_bindings(&i.assoc_types),
                params,
            )
        })
        .collect();
    for (trait_name, trait_instance, type_name, trait_module, impl_module, written, defaults, assoc, params) in default_sources {
        for m in defaults {
            if written.contains(&m.name) {
                continue;
            }
            let mut f = m;
            f.trait_impl = Some(ast::TraitImplSite {
                trait_name: trait_name.clone(),
                trait_args: Vec::new(),
                trait_instance: trait_instance.clone(),
                self_type: type_name.clone(),
            });
            substitute_type_params(&mut f, &params);
            substitute_self(&mut f, &type_name, &assoc);
            f.module = impl_module.clone();
            f.body_module = Some(trait_module.clone());
            program.functions.push(f);
        }
    }

    // One merged declaration order across `datac`s and `enum`s (A-1). Each
    // declaration carries its index *within its own file*, and the module
    // list is already in load order, so renumbering here is what gives
    // `types::owned_types` and the removed C backend a single sequence to walk: a
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

    // ---- M7: capability sets -------------------------------------------
    //
    // A module's set is *received*, never self-declared, with one exception:
    // the entry module's set. By default it still holds everything (`/**`/
    // `*`, every domain), but `coral.toml`'s `[caps]` section (C-2) lets a
    // project restrict it to exactly what is declared there - the natural
    // fix for open item 18, since `main` cannot write a `caps(...)` clause on
    // itself. `manifest_caps` is `None` whenever the section is absent (no
    // manifest at all, or a manifest that does not have one), which is what
    // keeps every `coral.toml` written before this existed behaving exactly
    // as before. Every other module holds exactly what its importers granted
    // it. That makes the no-escalation rule checkable - a module can
    // delegate less than it has, never more - and is what keeps an ungranted
    // `fs.read` a resolution error instead of a runtime check.
    let mut caps: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let entry_caps = manifest_caps.unwrap_or_else(|| {
        let mut whole = HashMap::new();
        for domain in crate::stdlib::DOMAINS {
            // `mmio` is the one domain the entry module does not hold by
            // default: its patterns are a machine's address ranges rather
            // than a resource the root of trust owns, so a freestanding
            // project has to write `[caps] mmio = [...]` down before any
            // module may build a pointer from an address
            // (`kelp wiki page language/capabilities.md`).
            if *domain == "mmio" {
                continue;
            }
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
            // `caps(*)` is the whole resource of every domain *except* mmio:
            // the whole address space is the escape hatch a reviewer greps
            // for, so it is never handed over implicitly (`kelp wiki page language/capabilities.md`).
            if *domain == "mmio" {
                continue;
            }
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
            // The wildcard is not handed over for `mmio`, so it is not required
            // of the importer either; see the granting loop above.
            if *domain == "mmio" {
                continue;
            }
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
                // module, which wrote no grant at all. The entry module is the
                // one importer with no such line, and its grant is the
                // manifest's `[caps]` section - the common case for `mmio`,
                // the one domain the entry module does not hold by default.
                let missing = if !held.is_empty() {
                    String::new()
                } else if g.importer == entry {
                    format!(
                        "; the entry module is granted by coral.toml's '[caps]' section, and this project's does not name '{}' (write `[caps] {} = [...]`)",
                        g.domain, g.domain
                    )
                } else {
                    format!(
                        "; '{}' holds nothing here because the module that imports it wrote no 'caps({} = [...])' grant on that `impl` line",
                        g.importer, g.domain
                    )
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
                        missing,
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
    // The functions an `unsafe` context is required to call, by canonical key
    // (MK9-S2 stage 1). `infer.rs` consults this at every call site, and the
    // list is exactly the modules the manifest listed under `[unsafe]`,
    // because `resolve.rs` refused any `unsafe` elsewhere above.
    let mut unsafe_fns = std::collections::HashSet::new();
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            if f.unsafe_ {
                let key = match &f.receiver {
                    Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                    None => format!("{}{}", m.prefix, f.name),
                };
                unsafe_fns.insert(key);
            }
        }
    }
    let mut must_use_fns = std::collections::HashSet::new();
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            if f.must_use {
                let key = match &f.receiver {
                    Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                    None => format!("{}{}", m.prefix, f.name),
                };
                must_use_fns.insert(key);
            }
        }
    }
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

    let interfaces = {
        let source_map: &SourceMap = &*loader.sources;
        build_interfaces(&loader.modules, source_map)
    };

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
        unsafe_fns,
        must_use_fns,
        target: manifest_target,
        incremental: manifest_incremental,
        sanitize: manifest_sanitize,
        emit: manifest_emit,
        dependencies: manifest_dependencies,
        grants: loader
            .grants
            .iter()
            .map(|g| GrantRecord {
                importer: g.importer.clone(),
                target: g.target.clone(),
                domain: g.domain.clone(),
                patterns: g.patterns.clone(),
            })
            .collect(),
        wildcard_grants: loader
            .all_grants
            .iter()
            .map(|g| (g.importer.clone(), g.target.clone()))
            .collect(),
    };
    if let Some(sink) = &mut timings {
        sink.resolve = resolve_start.elapsed();
    }
    Ok(Loaded {
        program,
        project,
        deprecations: loader.deprecations,
        interfaces,
    })
}

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
    // routes the signature text does not show (`plan/front-end-incremental.md`,
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
            .any(|word| matches!(word, "const" | "static" | "pd" | "drop"))
}

struct Loader<'a> {
    src_dir: PathBuf,
    /// Module id of the entry module - the only one with an empty prefix.
    entry: String,
    /// The entry module's text, when it comes from a buffer (`kelpc check
    /// --text`, MISSING-b6Ytr) rather than from disk. Taken the first time
    /// the entry module is read, so a re-expansion round reads the cached
    /// tokens instead of consuming it twice.
    entry_text: Option<String>,
    /// The `[alias]` entries of `coral.toml`, already checked by
    /// `parse_manifest`. Defined into the table before any module is read.
    manifest_alias: Vec<preprocess::AliasDef>,
    /// The manifest's path and text, when it has `[alias]` entries. Registered
    /// with the source map so a diagnostic about one entry can show its line.
    manifest_alias_source: Option<(PathBuf, String)>,
    /// The manifest's `http_pool` ceiling (S4), or the compiler default when
    /// there is no manifest. Threaded into the loader because the value is
    /// spliced into `std/http`'s *source text* before it is lexed - the one
    /// configuration value the shipped stdlib reads.
    http_pool: usize,
    sources: &'a mut SourceMap,
    /// The one flat, global alias table: the manifest's `[alias]` entries plus
    /// every inline declaration in the project.
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
    /// `coral.toml`'s `[unsafe] modules` list (MK9-S2 stage 1): the module ids
    /// allowed to contain `unsafe`. Empty when the section is absent, which
    /// keeps a project that never writes `unsafe` behaving as before.
    unsafe_modules: Vec<String>,
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
    /// Registers the manifest's `[alias]` entries, discovers the module graph,
    /// then loads it for real.
    ///
    /// Two phases, because the two halves of M5.3 need each other: an `impl`
    /// path may contain `%*alias*%`, so the file graph is only knowable after
    /// expansion, while the alias table is only complete once every file is
    /// known. Discovery therefore tolerates what it cannot yet resolve - a
    /// file that fails to expand or parse is skipped, an unresolvable import
    /// is ignored - and just collects aliases; the final strict round runs
    /// against the complete table, where every failure is a real error.
    fn load_project(&mut self) -> Result<(), Diagnostic> {
        self.load_manifest_aliases()?;

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
    /// here so that the last round is the only one anything reads - including
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

    /// Defines the manifest's `[alias]` entries. They go in before any inline
    /// declaration is scanned, so a module that re-declares one of their names
    /// is the duplicate that gets reported, naming the manifest.
    fn load_manifest_aliases(&mut self) -> Result<(), Diagnostic> {
        if let Some((path, text)) = self.manifest_alias_source.take() {
            self.sources.insert(preprocess::MANIFEST_MODULE, &path, text);
        }
        for def in std::mem::take(&mut self.manifest_alias) {
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
            // A failed visit must not leave the module looking half-loaded -
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
        // instead of the project directory; everything downstream - lexing,
        // aliases, parsing, prefixing - is identical.
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
            // file whose body cannot expand yet - it uses an alias another
            // file has still to contribute - nevertheless contributes its own
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
        let mut program = Parser::new(expanded)
            .parse_program()
            .map_err(|e| Diagnostic::new(e.message, e.span, id))?;
        for span in &program.outdated_returns {
            // Still accepted, no longer wanted: `.return:` says what the
            // marker means (`deprecated.rs`).
            self.deprecations
                .push(crate::deprecated::finding(crate::deprecated::RETURN_R, *span, id));
        }
        // `defer` is lowered away here, before resolution, the type checker,
        // the borrow checker and both backends, so every deferred action is
        // ordinary code by the time any of them sees the tree
        // (`kelp wiki page language/statics-and-defer.md`).
        crate::defer::lower(&mut program, id)?;

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
                            "'{}' is not a capability domain - v1 has {}",
                            grant.domain,
                            crate::stdlib::DOMAINS.join(", ")
                        ),
                        grant.span,
                        id,
                    ));
                }
                validate_cap_grant(&grant.domain, &grant.patterns, grant.span, id)?;
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
            // Injecting it at the import that needs it - keyed on the same
            // conditions as the calls themselves - is what keeps the runtime out
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

/// One `name = "path"` line of the `[alias]` section. The path is the module
/// an `impl "%*name*%"` resolves to, so it must name a module below `src/`:
/// a relative path with no leading `/` and no `..` component. The name itself
/// is checked by `AliasTable::define`, which also reports a duplicate across
/// the whole project.
fn parse_alias_line(manifest: &mut Manifest, line: &str, line_offset: usize) -> Result<(), Diagnostic> {
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

fn parse_manifest(text: &str, path: &Path) -> Result<Manifest, Diagnostic> {
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
        emit_entry: None,
        alias: Vec::new(),
        alias_source: None,
        dependencies: Vec::new(),
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
    /// long-standing default - the entry module holds the whole resource of
    /// every domain (`/**`/`*`) - so a `coral.toml` written before this
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
    /// `[unsafe] modules = ["<module id>", ...]` (MK9-S2 stage 1): the modules
    /// allowed to contain `unsafe`. Empty when the section is absent, which
    /// keeps a project that never writes `unsafe` behaving exactly as before.
    unsafe_modules: Vec<String>,
    /// `[alias]`: one `name = "path"` per line, in the order the manifest
    /// lists them. Each is an alias whose replacement text is the relative
    /// module path.
    alias: Vec<preprocess::AliasDef>,
    /// The manifest's path and text, set only when `alias` is non-empty.
    alias_source: Option<(PathBuf, String)>,
    /// Whether a `[target]` section header was seen at all (kernel stage 4).
    /// Distinct from the fields below being unset, because a `[target]` section
    /// that names no `mode` is an error rather than the hosted default.
    target_seen: bool,
    /// The `[target]` section's contents, `TargetDecl::hosted()` when there is
    /// none.
    target: TargetDecl,
    /// Whether `mode` was set, so a section without one is refused rather than
    /// defaulted.
    target_mode_set: bool,
    /// Whether `cpus` was set, so a duplicated key is refused rather than
    /// silently keeping one of the two values.
    target_cpus_set: bool,
    /// `[build] incremental = true` (roadmap A-14 slice 1): whether the backend
    /// splits the program into one compilation unit per source module. Absent
    /// (or `false`) is the whole-program default, which is what every project
    /// written before this section existed gets.
    incremental: bool,
    /// `[build] sanitize = "address"|"undefined"|"thread"`.
    sanitize: Option<crate::driver::Sanitizer>,
    /// `[build] emit = "binary"|"html"`.
    emit: Emit,
    /// `[build] emit_entry = "<file under src/>"`: the entry compiled when `emit`
    /// is `Html`. Required for `Html`, ignored for `Binary`.
    emit_entry: Option<String>,
    /// `[dependencies]` (Coral's section): every dependency key, with the
    /// `caps` list its inline-table entry may carry. Read here only for the
    /// capability check (`caps_report.rs`); Coral still fetches the sources.
    dependencies: Vec<DependencyDecl>,
}

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
            patterns.push(pattern.to_string());
        }
    }
    validate_cap_grant(domain, &patterns, at, "")?;
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

/// One `modules = ["<module id>", ...]` line inside `coral.toml`'s `[unsafe]`
/// section (MK9-S2 stage 1). The list is the set of modules allowed to contain
/// `unsafe`, spelled by the same module ids the `[extern.*]` sections use.
fn parse_unsafe_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
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
fn parse_build_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'incremental = true|false', 'sanitize = \"address\"' or 'emit = \"binary\"|\"html\"' in {MANIFEST_NAME}'s [build] section, found '{line}'"),
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
    if key.trim() != "incremental" {
        return Err(Diagnostic::new(
            format!(
                "unknown key '{}' in {MANIFEST_NAME}'s [build] section: its keys are 'incremental', 'sanitize', 'emit' and 'emit_entry'",
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
fn parse_target_line(manifest: &mut Manifest, line: &str, at: Span) -> Result<(), Diagnostic> {
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

/// One `key = value` line inside `coral.toml`'s `[extern."<module_id>"]`
/// section (MK9-S1-M1, roadmap B-1): `domain = "<one of stdlib::DOMAINS>"`,
/// `library = "<native library name>"`, or `symbols = ["<name>", ...]`. A
/// `symbols` entry is `"<kelp name>"` (same native symbol) or
/// `"<kelp name>=<c symbol>"`. The header line already inserted a default `ExternModuleDecl` for
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
fn parse_dir_list(
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
            | "pub"
            | "alias"
            | "print"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_hash_ignores_private_bodies_but_tracks_public_signatures() {
        let first = "pub fun api(x: i32).return: i32 = x\n\nfun hidden().return: i32 = 1\n";
        let private_body_changed =
            "pub fun api(x: i32).return: i32 = x\n\nfun hidden().return: i32 = 999\n";
        let public_signature_changed =
            "pub fun api(x: i64).return: i64 = x\n\nfun hidden().return: i32 = 1\n";
        assert_eq!(
            interface_api_hash("main", first),
            interface_api_hash("main", private_body_changed)
        );
        assert_ne!(
            interface_api_hash("main", first),
            interface_api_hash("main", public_signature_changed)
        );
    }

    #[test]
    fn an_interface_hash_tracks_bodies_that_leak_into_importers() {
        let edits = [
            // a generic template body
            ("pub fun id<T>(x: T).return: T = x\n", "pub fun id<T>(x: T).return: T = x\n\nfun hidden().return: i32 = 2\n"),
            // a folded constant
            ("const N: i32 = 1\npub fun f().return: i32 = N\n", "const N: i32 = 2\npub fun f().return: i32 = N\n"),
            // a passdown's initializer
            ("pd val depth: i32 = 1\npub fun f().return: i32 = depth\n", "pd val depth: i32 = 2\npub fun f().return: i32 = depth\n"),
            // a destructor body
            ("datac B { v: i32 }\nfun mut B.drop() { }\n", "datac B { v: i32 }\nfun mut B.drop() { val x = 1 }\n"),
        ];
        for (before, after) in edits {
            assert_ne!(
                interface_api_hash("main", before),
                interface_api_hash("main", after),
                "an edit that reaches importers did not change the hash: {before}"
            );
        }
    }

    fn manifest(text: &str) -> Manifest {
        parse_manifest(text, Path::new("coral.toml")).expect("manifest should parse")
    }

    fn manifest_err(text: &str) -> Diagnostic {
        parse_manifest(text, Path::new("coral.toml"))
            .err()
            .expect("manifest should be rejected")
    }

    fn extern_section(module: &str, header: &str) -> String {
        format!(
            "[extern.\"{module}\"]\n\
             domain = \"time\"\n\
             library = \"c\"\n\
             symbols = []\n\
             header = {header}\n"
        )
    }

    #[test]
    fn a_header_key_is_refused_as_removed_whatever_its_value() {
        for value in ["\"zlib.h\"", "zlib.h", "\"\"", "\"../evil.h\""] {
            let text = format!("name = \"x\"\n{}", extern_section("a", value));
            let err = manifest_err(&text);
            assert!(
                err.message.contains("'[extern.\"a\"] header' was removed"),
                "{value}: {}",
                err.message
            );
        }
    }

    fn extern_symbols_section(symbols: &str) -> String {
        format!(
            "name = \"x\"\n[extern.\"a\"]\n\
             domain = \"time\"\n\
             library = \"c\"\n\
             symbols = {symbols}\n"
        )
    }

    fn extern_symbol_of(symbols: &str, kelp: &str) -> Option<String> {
        let manifest = manifest(&extern_symbols_section(symbols));
        manifest
            .extern_modules
            .get("a")
            .expect("extern section")
            .symbol_of(kelp)
            .map(str::to_string)
    }

    #[test]
    fn a_plain_symbol_entry_binds_the_kelp_name_to_itself() {
        assert_eq!(extern_symbol_of("[\"getpid\"]", "getpid"), Some("getpid".into()));
        assert_eq!(extern_symbol_of("[\"getpid\"]", "other"), None);
    }

    #[test]
    fn a_renamed_symbol_entry_binds_the_kelp_name_to_the_c_symbol() {
        assert_eq!(
            extern_symbol_of("[\"window_new=gtk_window_new\"]", "window_new"),
            Some("gtk_window_new".into())
        );
        assert_eq!(
            extern_symbol_of("[\"window_new=gtk_window_new\"]", "gtk_window_new"),
            None
        );
    }

    #[test]
    fn a_symbol_entry_with_an_empty_side_is_rejected() {
        for bad in ["[\"=getpid\"]", "[\"getpid=\"]"] {
            let err = manifest_err(&extern_symbols_section(bad));
            assert!(err.message.contains("must name a Kelp function and a C symbol"), "{bad}");
        }
    }

    #[test]
    fn a_symbol_entry_with_whitespace_is_rejected() {
        for bad in ["[\"get pid\"]", "[\"getpid=gtk window_new\"]"] {
            let err = manifest_err(&extern_symbols_section(bad));
            assert!(err.message.contains("must not contain whitespace"), "{bad}");
        }
    }

    #[test]
    fn a_symbol_entry_whose_c_symbol_is_not_an_identifier_is_rejected() {
        for bad in ["[\"getpid=a-b\"]", "[\"getpid=7z\"]", "[\"getpid=g(h)\"]"] {
            let err = manifest_err(&extern_symbols_section(bad));
            assert!(err.message.contains("must be a C identifier"), "{bad}");
        }
    }

    #[test]
    fn a_kelp_name_listed_twice_is_rejected() {
        let err = manifest_err(&extern_symbols_section("[\"getpid\", \"getpid=other_pid\"]"));
        assert!(err.message.contains("'getpid' more than once"), "{}", err.message);
    }

    fn extern_dirs_section(module: &str, key: &str, value: &str) -> String {
        format!(
            "[extern.\"{module}\"]\n\
             domain = \"time\"\n\
             library = \"c\"\n\
             symbols = []\n\
             {key} = {value}\n"
        )
    }

    #[test]
    fn an_include_dirs_key_is_refused_as_removed() {
        let err = manifest_err(&extern_dirs_section("a", "include_dirs", "[\"include\"]"));
        assert!(
            err.message.contains("'[extern.\"a\"] include_dirs' was removed"),
            "{}",
            err.message
        );
    }

    #[test]
    fn library_dirs_are_parsed_as_written() {
        let parsed = manifest(&extern_dirs_section("a", "library_dirs", "[\"lib\", \"/opt/lib\"]"));
        assert_eq!(
            parsed
                .extern_modules
                .get("a")
                .expect("extern section")
                .library_dirs(),
            ["lib".to_string(), "/opt/lib".to_string()].as_slice()
        );
    }

    #[test]
    fn a_dir_entry_that_is_not_a_bracketed_list_is_rejected() {
        let err = manifest_err(&extern_dirs_section("a", "library_dirs", "include"));
        assert!(err.message.contains("library_dirs' must be a bracketed list"), "{}", err.message);
    }

    #[test]
    fn a_dir_entry_that_is_not_double_quoted_is_rejected() {
        let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[include]"));
        assert!(err.message.contains("library_dirs' must be double-quoted"), "{}", err.message);
    }

    #[test]
    fn an_empty_dir_entry_is_rejected() {
        let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[\"\"]"));
        assert!(err.message.contains("library_dirs' must not be empty"), "{}", err.message);
    }

    #[test]
    fn a_dir_entry_with_a_forbidden_character_is_rejected() {
        for bad in ["\"a\"b\"", "\"a\nb\"", "\"a\0b\""] {
            let err = parse_dir_list(&format!("[{bad}]"), "a", "library_dirs", Span::new(0, 0))
                .expect_err("expected a forbidden character to be refused");
            assert!(err.message.contains("must not contain"), "{bad:?}");
        }
    }

    #[test]
    fn a_dir_entry_starting_with_a_dash_is_rejected() {
        let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[\"-I/usr/include\"]"));
        assert!(err.message.contains("library_dirs' must not start with '-'"), "{}", err.message);
    }

    #[test]
    fn extern_dirs_are_resolved_deduplicated_and_sorted() {
        let text = format!(
            "{}{}",
            extern_dirs_section("a", "library_dirs", "[\"include\", \"/opt/inc\"]"),
            extern_dirs_section("b", "library_dirs", "[\"include\"]"),
        );
        let parsed = manifest(&text);
        assert_eq!(
            collect_extern_dirs(
                &parsed.extern_modules,
                Some(Path::new("/proj")),
                |e| e.library_dirs()
            ),
            ["/opt/inc".to_string(), "/proj/include".to_string()].as_slice()
        );
    }

    #[test]
    fn a_relative_dir_is_resolved_against_the_manifest_directory() {
        let parsed = manifest(&extern_dirs_section("a", "library_dirs", "[\"lib\"]"));
        let dirs = collect_extern_dirs(
            &parsed.extern_modules,
            Some(Path::new("/proj")),
            |e| e.library_dirs(),
        );
        assert_eq!(dirs, ["/proj/lib".to_string()].as_slice());
    }

    #[test]
    fn pkg_config_packages_are_deduplicated_and_sorted() {
        let text = format!(
            "{}{}{}",
            extern_dirs_section("b", "pkg_config", "\"zlib\""),
            extern_dirs_section("a", "pkg_config", "\"gtk4\""),
            extern_dirs_section("c", "pkg_config", "\"zlib\""),
        );
        let parsed = manifest(&text);
        assert_eq!(
            collect_extern_pkg_config(&parsed.extern_modules),
            ["gtk4".to_string(), "zlib".to_string()].as_slice()
        );
    }

    #[test]
    fn a_section_without_a_pkg_config_key_names_none() {
        let manifest = manifest(
            "name = \"x\"\n[extern.\"a\"]\n\
             domain = \"time\"\n\
             library = \"c\"\n\
             symbols = []\n",
        );
        assert!(collect_extern_pkg_config(&manifest.extern_modules).is_empty());
    }

    #[test]
    fn a_pkg_config_that_is_not_a_quoted_string_is_rejected() {
        let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", "zlib"));
        let err = manifest_err(&text);
        assert!(err.message.contains("pkg_config' must be a double-quoted string"), "{}", err.message);
    }

    #[test]
    fn an_empty_pkg_config_is_rejected() {
        let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", "\"\""));
        let err = manifest_err(&text);
        assert!(err.message.contains("pkg_config' must not be empty"), "{}", err.message);
    }

    #[test]
    fn a_pkg_config_name_with_a_forbidden_character_is_rejected() {
        for bad in ["\"z lib\"", "\"zlib;rm\"", "\"z/lib\"", "\"zli\"b\""] {
            let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", bad));
            let err = manifest_err(&text);
            assert!(
                err.message.contains("pkg_config' must contain only letters, digits and '._+-'"),
                "expected '{bad}' to be refused, got: {}",
                err.message
            );
        }
    }

    #[test]
    fn a_ui_pattern_in_the_manifest_caps_section_is_rejected() {
        let err = manifest_err("name = \"x\"\n[caps]\nui = [\"wayland-0\"]\n");
        assert!(
            err.message.contains("patterns are not enforceable in the 'ui' domain yet"),
            "{}",
            err.message
        );
        // The wildcard is the vocabulary, so the section parses with it.
        assert_eq!(
            manifest("name = \"x\"\n[caps]\nui = [\"*\"]\n")
                .caps
                .expect("the [caps] section")
                .get("ui"),
            Some(&vec!["*".to_string()])
        );
    }

    #[test]
    fn a_pkg_config_key_set_twice_is_rejected() {
        let text = "name = \"x\"\n[extern.\"a\"]\n\
                    domain = \"time\"\n\
                    library = \"c\"\n\
                    symbols = []\n\
                    pkg_config = \"zlib\"\n\
                    pkg_config = \"gtk4\"\n"
            .to_string();
        let err = manifest_err(&text);
        assert!(err.message.contains("pkg_config' is set more than once"), "{}", err.message);
    }

    #[test]
    fn the_build_section_turns_incremental_on_and_off() {
        // Roadmap A-14 slice 1: absent is the whole-program default, and the one
        // key the section has reads either boolean.
        assert!(!manifest("name = \"x\"\n").incremental);
        assert!(manifest("name = \"x\"\n[build]\nincremental = true\n").incremental);
        assert!(!manifest("name = \"x\"\n[build]\nincremental = false\n").incremental);
    }

    #[test]
    fn the_build_section_names_a_sanitizer() {
        use crate::driver::Sanitizer;
        assert_eq!(manifest("name = \"x\"\n").sanitize, None);
        assert_eq!(
            manifest("name = \"x\"\n[build]\nsanitize = \"address\"\n").sanitize,
            Some(Sanitizer::Address)
        );
        let err = manifest_err("name = \"x\"\n[build]\nsanitize = \"memory\"\n");
        assert!(err.message.contains("must be \"address\""), "{}", err.message);
    }

    #[test]
    fn a_malformed_build_section_is_rejected() {
        let err = manifest_err("name = \"x\"\n[build]\nincremental = \"true\"\n");
        assert!(
            err.message.contains("must be true or false"),
            "{}",
            err.message
        );
        let err = manifest_err("name = \"x\"\n[build]\nunits = true\n");
        assert!(
            err.message.contains("its keys are 'incremental', 'sanitize', 'emit' and 'emit_entry'"),
            "{}",
            err.message
        );
    }

    #[test]
    fn the_build_section_chooses_what_is_emitted() {
        use super::Emit;
        assert_eq!(manifest("name = \"x\"\n").emit, Emit::Binary);
        assert_eq!(
            manifest("name = \"x\"\n[build]\nemit = \"html\"\nemit_entry = \"emit.kp\"\n").emit,
            Emit::Html
        );
        assert_eq!(
            manifest("name = \"x\"\n[build]\nemit = \"binary\"\n").emit,
            Emit::Binary
        );
        let err = manifest_err("name = \"x\"\n[build]\nemit = \"exe\"\n");
        assert!(
            err.message.contains("must be \"binary\" or \"html\""),
            "{}",
            err.message
        );
        let err = manifest_err("name = \"x\"\n[build]\nemit = html\n");
        assert!(err.message.contains("(double-quoted)"), "{}", err.message);
    }

    #[test]
    fn html_emit_names_the_entry_it_compiles() {
        let m = manifest("name = \"x\"\n[build]\nemit = \"html\"\nemit_entry = \"emit.kp\"\n");
        assert_eq!(m.emit_entry.as_deref(), Some("emit.kp"));
        // The key order does not matter, and a binary build needs no entry.
        let m = manifest("name = \"x\"\n[build]\nemit_entry = \"emit.kp\"\nemit = \"html\"\n");
        assert_eq!(m.emit_entry.as_deref(), Some("emit.kp"));
        assert_eq!(manifest("name = \"x\"\n[build]\nemit = \"binary\"\n").emit_entry, None);
    }

    #[test]
    fn html_emit_without_an_emit_entry_is_an_error() {
        let err = manifest_err("name = \"x\"\n[build]\nemit = \"html\"\n");
        assert!(
            err.message.contains("names no emit_entry"),
            "{}",
            err.message
        );
        let err = manifest_err("name = \"x\"\n[build]\nemit_entry = \"\"\nemit = \"html\"\n");
        assert!(err.message.contains("'[build] emit_entry'"), "{}", err.message);
    }

    #[test]
    fn the_unsafe_section_lists_modules_in_order() {
        let m = manifest("name = \"x\"\n[unsafe]\nmodules = [\"kernel/mmio\", \"main\"]\n");
        assert_eq!(m.unsafe_modules, ["kernel/mmio".to_string(), "main".to_string()]);
        // No section means no module may contain `unsafe`.
        assert!(manifest("name = \"x\"\n").unsafe_modules.is_empty());
    }

    #[test]
    fn a_malformed_unsafe_section_is_rejected() {
        let err = manifest_err("name = \"x\"\n[unsafe]\nmodules = \"main\"\n");
        assert!(
            err.message.contains("bracketed list of double-quoted module ids"),
            "{}",
            err.message
        );
        let err = manifest_err("name = \"x\"\n[unsafe]\nmods = [\"main\"]\n");
        assert!(err.message.contains("the section has one key, 'modules'"), "{}", err.message);
        let err = manifest_err("name = \"x\"\n[unsafe]\nmodules = [\"main\", \"main\"]\n");
        assert!(err.message.contains("appears more than once"), "{}", err.message);
    }

    #[test]
    fn no_target_section_is_the_hosted_default() {
        let m = manifest("name = \"x\"\n");
        assert!(!m.target_seen);
        assert_eq!(m.target.mode(), TargetMode::Hosted);
        assert!(!m.target.is_freestanding());
        assert!(m.target.triple().is_none());
        assert!(m.target.entry().is_none());
        assert!(m.target.linker_script().is_none());
        assert!(m.target.panic().is_none());
    }

    #[test]
    fn a_freestanding_target_section_is_read_whole() {
        let text = "name = \"x\"\n[target]\n\
                    mode = \"freestanding\"\n\
                    triple = \"riscv64-unknown-none-elf\"\n\
                    entry = \"kmain\"\n\
                    linker_script = \"kernel.ld\"\n\
                    panic = \"kp_panic\"\n";
        let m = manifest(text);
        assert!(m.target.is_freestanding());
        assert_eq!(m.target.triple(), Some("riscv64-unknown-none-elf"));
        assert_eq!(m.target.entry(), Some("kmain"));
        assert_eq!(m.target.linker_script(), Some("kernel.ld"));
        assert_eq!(m.target.panic(), Some("kp_panic"));
    }

    #[test]
    fn a_freestanding_target_section_reads_the_allocator_keys() {
        let text = "name = \"x\"\n[target]\n\
                    mode = \"freestanding\"\n\
                    triple = \"riscv64-unknown-none-elf\"\n\
                    entry = \"kmain\"\n\
                    linker_script = \"kernel.ld\"\n\
                    alloc = \"kmalloc\"\n\
                    dealloc = \"kfree\"\n\
                    realloc = \"krealloc\"\n";
        let m = manifest(text);
        assert_eq!(m.target.alloc(), Some("kmalloc"));
        assert_eq!(m.target.dealloc(), Some("kfree"));
        assert_eq!(m.target.realloc(), Some("krealloc"));
        // A section with no allocator keys configures no allocator.
        assert!(manifest("name = \"x\"\n").target.alloc().is_none());
    }

    #[test]
    fn the_cpu_count_is_read_as_a_bare_integer() {
        assert_eq!(manifest("name = \"x\"\n").target.cpus(), 1);
        let m = manifest(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
             triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
             linker_script = \"k.ld\"\ncpus = 4\n",
        );
        assert_eq!(m.target.cpus(), 4);
    }

    #[test]
    fn a_cpu_count_outside_one_to_sixty_four_is_rejected() {
        for bad in ["0", "65", "yes", "\"4\""] {
            let err = manifest_err(&format!(
                "name = \"x\"\n[target]\nmode = \"hosted\"\ncpus = {bad}\n"
            ));
            assert!(
                err.message.contains("'[target] cpus'"),
                "{bad}: {}",
                err.message
            );
        }
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\ncpus = 2\ncpus = 4\n");
        assert!(err.message.contains("appears more than once"), "{}", err.message);
    }

    #[test]
    fn a_half_allocator_pair_is_rejected() {
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
             triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
             linker_script = \"k.ld\"\nalloc = \"kmalloc\"\n",
        );
        assert!(err.message.contains("'alloc' without 'dealloc'"), "{}", err.message);
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
             triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
             linker_script = \"k.ld\"\nrealloc = \"krealloc\"\n",
        );
        assert!(err.message.contains("'realloc' without 'alloc'"), "{}", err.message);
    }

    #[test]
    fn a_hosted_target_section_names_no_freestanding_keys() {
        let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\n");
        assert_eq!(m.target.mode(), TargetMode::Hosted);
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"hosted\"\ntriple = \"riscv64-unknown-none-elf\"\n",
        );
        assert!(
            err.message.contains("only meaningful with 'mode = \"freestanding\"'"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_target_section_without_a_mode_is_hosted() {
        let m = manifest("name = \"x\"\n[target]\n");
        assert!(m.target.has_shared_pool());
        assert!(!m.target.is_freestanding());
    }

    #[test]
    fn mode_managed_is_refused_with_the_replacement() {
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"managed\"\n");
        assert!(err.message.contains("was removed"), "{}", err.message);
        assert!(err.message.contains("delete the mode line"), "{}", err.message);
    }

    #[test]
    fn heap_max_is_accepted_in_a_hosted_section_and_refused_when_freestanding() {
        let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 64\n");
        assert_eq!(m.target.heap_max_mib(), 64);
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\n\
             entry = \"kmain\"\nlinker_script = \"k.ld\"\nheap_max = 64\n",
        );
        assert!(err.message.contains("only meaningful with 'mode = \"hosted\"'"), "{}", err.message);
    }

    fn a_hosted_target_section_is_accepted_with_the_default_heap() {
        let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\n");
        assert!(m.target.has_shared_pool());
        assert!(!m.target.is_freestanding());
        assert_eq!(m.target.heap_max_mib(), 256);
        assert_eq!(m.target.heap_max_mib(), crate::managed::DEFAULT_HEAP_MAX_MIB);
    }

    #[test]
    fn heap_max_is_read_in_mib_for_a_hosted_section() {
        let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 512\n");
        assert_eq!(m.target.heap_max_mib(), 512);
    }

    #[test]
    fn a_bad_heap_max_is_a_clear_error() {
        for bad in ["\"512\"", "abc", "-1", "0", "65537", "1.5"] {
            let err = manifest_err(&format!(
                "name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = {bad}\n"
            ));
            assert!(
                err.message.contains("'[target] heap_max'"),
                "{bad}: {}",
                err.message
            );
        }
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 0\n");
        assert!(err.message.contains("between 1 and 65536 MiB"), "{}", err.message);
    }

    #[test]
    fn a_duplicate_heap_max_is_refused() {
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 64\nheap_max = 128\n",
        );
        assert!(err.message.contains("appears more than once"), "{}", err.message);
    }

    #[test]
    fn a_hosted_section_refuses_the_freestanding_and_allocator_keys() {
        for line in ["triple = \"x\"", "entry = \"main\"", "alloc = \"a\""] {
            let err = manifest_err(&format!("name = \"x\"\n[target]\nmode = \"hosted\"\n{line}\n"));
            assert!(err.message.contains("is only meaningful with 'mode = \"freestanding\"'"), "{}", err.message);
        }
    }

    #[test]
    fn an_unknown_key_message_names_heap_max() {
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nnope = \"1\"\n");
        assert!(err.message.contains("'heap_max'"), "{}", err.message);
    }

    #[test]
    fn a_freestanding_section_missing_a_required_key_is_rejected() {
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\nentry = \"kmain\"\n",
        );
        assert!(err.message.contains("missing 'triple"), "{}", err.message);
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
             triple = \"riscv64-unknown-none-elf\"\nlinker_script = \"k.ld\"\n",
        );
        assert!(err.message.contains("missing 'entry"), "{}", err.message);
    }

    #[test]
    fn an_unknown_target_mode_is_rejected() {
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"based\"\n");
        assert!(err.message.contains("unknown 'based'"), "{}", err.message);
    }

    #[test]
    fn a_target_entry_that_is_not_a_bare_name_is_rejected() {
        let err = manifest_err(
            "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
             triple = \"riscv64-unknown-none-elf\"\nentry = \"kernel.kmain\"\n\
             linker_script = \"k.ld\"\n",
        );
        assert!(err.message.contains("must be a bare function name"), "{}", err.message);
    }

    #[test]
    fn an_unknown_key_in_the_target_section_is_rejected() {
        let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nnope = \"1\"\n");
        assert!(err.message.contains("unknown key 'nope'"), "{}", err.message);
    }

    #[test]
    fn alias_entries_are_read_from_the_alias_section() {
        let text = "name = \"x\"\n[alias]\nutil = \"helpers/util\"\n";
        let m = manifest(text);
        assert_eq!(m.alias.len(), 1);
        assert_eq!(m.alias[0].name, "util");
        assert_eq!(m.alias[0].text, "helpers/util");
        assert_eq!(m.alias[0].module, "coral.toml");
        let at = m.alias[0].text_offset;
        assert_eq!(&text[at..at + "helpers/util".len()], "helpers/util");
        assert!(m.alias_source.is_some());
    }

    #[test]
    fn alias_paths_must_be_relative_and_non_empty() {
        for (value, needle) in [
            ("\"\"", "has an empty path"),
            ("\"/abs/util\"", "must be a relative module path"),
            ("\"../up\"", "must not contain '..'"),
            ("\"a/../b\"", "must not contain '..'"),
            ("util", "must be a double-quoted string"),
        ] {
            let err = manifest_err(&format!("[alias]\nu = {value}\n"));
            assert!(err.message.contains(needle), "{value}: {}", err.message);
        }
    }

    #[test]
    fn a_duplicate_alias_in_the_section_is_rejected() {
        let err = manifest_err("[alias]\nu = \"a\"\nu = \"b\"\n");
        assert!(
            err.message.contains("alias 'u' is already defined in coral.toml [alias]"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_manifest_without_an_alias_section_has_no_aliases() {
        let m = manifest("name = \"x\"\n");
        assert!(m.alias.is_empty());
        assert!(m.alias_source.is_none());
    }
}

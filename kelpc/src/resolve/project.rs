use super::*;

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
    pub(crate) prefixes: HashMap<String, String>,
    /// Per module: alias -> the target module's symbol prefix (what
    /// `infer.rs` needs to qualify `circle.Item`).
    pub(crate) alias_prefixes: HashMap<String, HashMap<String, String>>,
    /// Per module: capability domain -> the patterns it holds (M7). The entry
    /// module holds `/**` for every domain - the plan's "main implicitly
    /// holds the full set" - and every other module holds exactly what its
    /// importers delegated to it, never more.
    pub(crate) caps: HashMap<String, HashMap<String, Vec<String>>>,
    /// Per function key: the capability domain of the module that declares
    /// it, for the gated modules only. `infer.rs` gates resolution on it and
    /// the removed C backend inserts the runtime check because of it.
    pub(crate) gated: HashMap<String, String>,
    /// The `[dependencies]` entries of `coral.toml`, in manifest order. A
    /// dependency's modules are the ones under `deps/<key>/` (Coral copies each
    /// dependency into `src/deps/<key>/`), which is how the capability check
    /// finds them.
    pub(crate) dependencies: Vec<DependencyDecl>,
    /// Every `impl "target" caps(domain = [...])` clause, in load order, for
    /// `kelpc caps`'s grant lines.
    pub(crate) grants: Vec<GrantRecord>,
    /// Every `impl "target" caps(*)` clause as (importer, target), in load order.
    pub(crate) wildcard_grants: Vec<(String, String)>,
    /// Per module: its `coral.toml` `[extern."<module id>"]` entry (MK9-S1-M1,
    /// roadmap B-1), when it has one. This is what lets a *user* module's own
    /// `extern fun` declarations exist at all: `domain_of` below folds this
    /// table in beside `stdlib::domain_of`'s stdlib-only table, so a module
    /// named here is gated exactly like a stdlib module is, and `infer.rs`
    /// checks a declared `extern fun`'s bare name against the entry's
    /// `symbols` allowlist instead of refusing every non-stdlib extern
    /// outright.
    pub(crate) extern_modules: HashMap<String, ExternModuleDecl>,
    /// The functions written `unsafe fun` (MK9-S2 stage 1), by canonical key:
    /// the set `infer.rs` requires an `unsafe` context to call. Empty for a
    /// program that declares none.
    pub(crate) unsafe_fns: std::collections::HashSet<String>,
    /// The functions written `must_use fun` (MISSING-Cn2Ym), by canonical key:
    /// a call to one used as a statement is refused.
    pub(crate) must_use_fns: std::collections::HashSet<String>,
    /// The manifest's `[target]` section (kernel stage 4), or the hosted default
    /// when there is none. `infer.rs` reads the entry name from it, the
    /// freestanding checker its allowed set, and the driver the triple and
    /// linker script.
    pub(crate) target: TargetDecl,
    /// `coral.toml`'s `[build] incremental = true` (roadmap A-14 slice 1): the
    /// project wants the backend's per-unit split. `None`/absent is the
    /// default, which is the whole-program single module the compiler has
    /// always emitted.
    pub(crate) incremental: bool,
    /// `coral.toml`'s `[build] sanitize = "address"`: the sanitizer the C
    /// backend builds the program with. `--sanitize=` on the command line wins.
    pub(crate) sanitize: Option<crate::driver::Sanitizer>,
    /// `coral.toml`'s `[build] emit = "binary"|"html"`: what `kelpc build`
    /// leaves behind. `Binary` is the default.
    pub(crate) emit: Emit,
    /// `coral.toml`'s `[build] obfuscate = "off"|"names"` (obfuscation step 1).
    pub(crate) obfuscate: Obfuscate,
}

/// What `[build] obfuscate` asks for. `Names` keeps the intermediate IR and
/// objects out of the output directory and strips the binary's symbol and
/// debug sections at link time. It is not security.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obfuscate {
    Off,
    Names,
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

    /// The manifest's `[build] obfuscate`, `Off` when it names none.
    pub fn obfuscate(&self) -> Obfuscate {
        self.obfuscate
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

pub(crate) fn collect_extern_pkg_config(extern_modules: &HashMap<String, ExternModuleDecl>) -> Vec<String> {
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
pub(crate) fn collect_extern_dirs(
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
    pub(crate) mode: TargetMode,
    pub(crate) triple: Option<String>,
    pub(crate) entry: Option<String>,
    pub(crate) linker_script: Option<String>,
    pub(crate) panic: Option<String>,
    /// The global allocator (kernel stage 7, MK9-S5-M1): the bare names of the
    /// entry module's `alloc`, `dealloc` and optional `realloc` functions. All
    /// three are `None` when the section configures no allocator, which is the
    /// behaviour every project before this stage had.
    pub(crate) alloc: Option<String>,
    pub(crate) dealloc: Option<String>,
    pub(crate) realloc: Option<String>,
    /// `cpus = N` (kernel stage 8, MK9-S3-M3): how many copies a `percpu`
    /// static is laid out as. 1 when the section names none, which is what a
    /// single-hart image and every project before this stage get.
    pub(crate) cpus: u64,
    /// `heap_max = N` (Tidepool heap): the collected heap's cap in MiB. `None`
    /// when the section names none; `heap_max_mib` then gives the default.
    pub(crate) heap_max: Option<u64>,
    /// The manifest's `[tidepool]` section, copied in at the end of
    /// `parse_manifest` (Tidepool step 2). The defaults apply when it is empty.
    pub(crate) tidepool: TidepoolDecl,
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
            tidepool: TidepoolDecl::default(),
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

    /// The manifest's `[tidepool]` section (Tidepool step 2).
    pub fn tidepool(&self) -> &TidepoolDecl {
        &self.tidepool
    }
}

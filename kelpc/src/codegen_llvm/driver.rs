use super::*;

/// The layout facts `gc_descr` reads, answered from the same LLVM types and
/// target layout the function bodies use.
impl<'a> crate::gc_descr::GcLayout for Gen<'a> {
    fn struct_fields(&self, name: &str) -> Result<Vec<Ty>, String> {
        let s = self
            .program
            .structs
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("a hosted build does not describe the enum '{name}' in a GC object yet"))?;
        if !matches!(s.repr, crate::ast::Repr::C) {
            return Err(format!(
                "a hosted build describes only `repr(c)` datac layouts; '{name}' is not one"
            ));
        }
        Ok(s.fields.iter().map(|f| f.ty.clone()).collect())
    }

    fn size_align(&self, ty: &Ty) -> Result<(u64, u64), String> {
        let t = self.lty2(ty)?;
        Ok((self.layout.abi_size_of(t), self.layout.abi_alignment_of(t) as u64))
    }

    fn enum_variants(&self, name: &str) -> Option<Vec<Vec<Ty>>> {
        self.program.enums.iter().find(|e| e.name == name).map(|e| {
            e.variants
                .iter()
                .map(|v| v.fields.iter().map(|f| f.ty.clone()).collect())
                .collect()
        })
    }
}

/// Lower one module and write its object: the per-module half of the backend.
/// `compile` calls it once for the whole program; the incremental mode calls it
/// once per compilation unit, with `unit` naming the part of `program` this
/// module defines (see `kelp wiki page compiler/incremental.md`).
///
/// `ir_path` receives the unoptimized lowering (the artifact `out.ll` is for the
/// whole-program case) and `object_path` the object the level's pipeline and
/// codegen level produced.
///
/// `cache` is the object cache in the incremental mode (roadmap A-14 slice 2):
/// the IR text is the key's largest field, so the module is lowered either way
/// and the key is computed before the pass pipeline runs. A hit copies the
/// cached object over `object_path` and skips the passes and codegen, which is
/// the whole point of the cache; `Ok(Some(key))` reports that hit so the caller
/// can delete the entry if the link later fails. `None` is an ordinary compile.
/// Whole-program builds pass `None` for `cache`: there is no unit, so there is
/// nothing to key.
pub(crate) fn lower_to_object(
    program: &Program,
    drops: &DropPlan,
    sources: &SourceMap,
    ctx: &Context,
    machine: &TargetMachine,
    layout: &DataLayout,
    triple: &str,
    module_name: &str,
    ir_path: &Path,
    object_path: &Path,
    opt: OptLevel,
    freestanding: Option<&Freestanding>,
    unit: Option<Unit>,
    cache: Option<&Cache>,
    managed: Option<HeapLimits>,
    timings: &mut Timings,
) -> Result<Option<String>, String> {
    let started = Instant::now();
    let mut module = Module::new(ctx, module_name);
    module.set_triple(triple);
    // The module has to carry the same data layout the machine lays the object
    // out with before any pass runs: a pass that folds a struct field to a byte
    // offset uses the module's layout, and LLVM's default is not the host's (it
    // aligns an `i64` to 4). See `Module::set_data_layout`.
    module.set_data_layout(layout);
    let mut lowering = Gen::new(
        ctx,
        &module,
        program,
        drops,
        sources,
        layout,
        opt.is_optimized(),
        freestanding.cloned(),
        unit,
        managed,
    )?;
    lowering.declare()?;
    lowering.emit_const_data()?;
    lowering.declare_statics()?;
    // The user `extern fun`s that take or return an aggregate by value, lowered
    // to IR (`extern_agg`): the unit that owns one defines its trampoline.
    lowering.define_aggregate_externs()?;
    // The small builtin accessors get their bodies here, as IR rather than a
    // declaration into the runtime object, so the module's own `-O2` inlines
    // them at the call site. `emit_container_bodies` below is the same idea for
    // the monomorphized containers. A freestanding program has no runtime
    // object to define the rest and no heap types to reach these, so it skips
    // them: a dead body that called `strlen` would be exactly the libc
    // reference this mode exists to exclude. The one surface it does need is
    // the `str` one, which `emit_freestanding_str_bodies` emits as the same
    // kind of byte loop with no libc call behind it.
    if freestanding.is_none() {
        if lowering.emits_runtime_accessors() {
            lowering.emit_runtime_bodies()?;
        }
    } else {
        lowering.emit_freestanding_str_bodies()?;
        // The `String`/`Bytes` bodies a project with its own allocator needs
        // (kernel stage 12, MK9-S5-M2). Without an allocator the front end has
        // already refused the two types, and emitting a body that reaches
        // libc's `malloc` would be the reference this mode exists to exclude.
        if lowering.alloc_fns.is_some() {
            lowering.emit_freestanding_buffer_bodies()?;
        }
        // The `mem*` routines LLVM's own aggregate copies call, defined weak
        // and as loops (kernel gap 4).
        lowering.emit_freestanding_mem_bodies()?;
    }
    // The generated per-`datac` and per-`enum` drops, clones and comparators are
    // defined before the function bodies that call them, mirroring the order
    // the earlier emitter wrote each typedef and its drop routine in. The container
    // function sets come next, because a container's own drop calls the
    // element's (MK7 §3.2: one monomorphized set per instantiation, generated
    // into the module). In the incremental mode only the `_support` unit gets
    // these bodies; every other unit only declares them, so the symbol has one
    // definition in the link (roadmap A-14 slice 1).
    if lowering.defines_helpers() {
        lowering.emit_drop_fn_bodies()?;
        lowering.emit_clone_cmp_bodies()?;
        lowering.emit_container_bodies()?;
        // The S2E machinery (per-signature closure drops, the widening's thunk
        // and destructor, the shims) comes before the function bodies, the same
        // order the earlier emitter writes them in.
        lowering.emit_closure_bodies()?;
        // The `&dyn` thunks (A-12 slice 4), after every function they call is
        // declared and defined, the same order the earlier emitter writes them in.
        lowering.emit_dyn_bodies()?;
        // The `pd` globals' two generated functions (MK7 §5), whose bodies call
        // the drops and container sets above.
        lowering.emit_pd_globals()?;
    }
    lowering.emit_all()?;

    // Every function a freestanding image emits carries `probe-stack="inline-asm"`
    // (kernel stage 14): a frame larger than one page then touches each page in
    // order, so a guard page below the stack catches the overflow instead of a
    // single `sub sp` stepping over it. A hosted build is unchanged. Declarations
    // are skipped because there is no prologue to probe; the compiler-generated
    // bodies (the `mem*` fallbacks, the drops and the container sets) are swept
    // here too, because a large frame among them would otherwise be the one
    // frame that skips the guard.
    if freestanding.is_some() {
        // An x86-64 image has a red zone below `rsp` that an interrupt the
        // kernel installs later would clobber, so no function may keep a live
        // value there (kernel stage 20).
        let x86 = triple.starts_with("x86_64");
        for func in module.functions() {
            if !func.is_declaration() {
                func.mark_probe_stack();
                if x86 {
                    func.mark_noredzone();
                }
            }
        }
    }

    // The bare-metal entry stub, after every function is declared so the call
    // to the entry symbol resolves. It is module-level assembly because setting
    // `sp` from a linker-script symbol is not expressible in the language yet;
    // it belongs to the boot section the linker script places first.
    if let Some(fs) = freestanding {
        module.append_inline_asm(&freestanding_entry_asm(&fs.triple, &symbol_name(&fs.entry))?);
    }

    // Every location has been attached now, so the debug descriptors are
    // constructed and frozen before the module is written (MK7 S4, §4.4).
    lowering.finalize_debug();

    // Test-only hook (`kelp wiki page compiler/incremental.md` slice 3): the parallel
    // build's failure path is exercised with a real LLVM-level error, not a
    // front-end refusal. `KELPC_TEST_CORRUPT_UNIT=<unit>` appends an empty,
    // unterminated basic block to one of the unit's own functions, so LLVM's
    // verifier rejects the module and the error names the unit. It does nothing
    // unless the variable names this unit and is not a user surface.
    if std::env::var("KELPC_TEST_CORRUPT_UNIT").as_deref() == Ok(module_name) {
        if let Some(function) = module.functions().into_iter().find(|f| !f.is_declaration()) {
            function.append_block("kelpc_test_corrupt");
        }
    }

    // A caller must verify before emitting: LLVM's object writer assumes a
    // verified module and can crash on an unverified one rather than fail.
    module.verify()?;

    // The lowering is written before the pipeline runs, so the artifact beside
    // the binary is the IR this backend emitted at every level. The stage gates
    // read this text and assert an instruction's shape; an optimized module
    // would answer a different question.
    let ir_text = module.to_ir_string();
    let lower_elapsed = started.elapsed();
    std::fs::write(ir_path, &ir_text).map_err(|e| {
        format!("could not write generated IR to '{}': {e}", ir_path.display())
    })?;

    // The object is a pure function of the IR text and the machine's own
    // settings, so the key is computed from exactly those (`cache::object_key`)
    // before any pass runs.
    let key = crate::cache::object_key(
        &KeyFields {
            triple: machine.triple(),
            cpu: machine.cpu(),
            features: machine.features(),
            reloc: machine.reloc(),
            code_model: machine.code_model(),
            opt: opt.pipeline(),
            debug: DEBUG_INFO_SETTING,
            compiler: &compiler_version(),
        },
        &ir_text,
    );
    if let Some(cache) = cache {
        if let Some(entry) = cache.lookup(&key) {
            std::fs::copy(&entry, object_path).map_err(|e| {
                format!(
                    "could not write the cached unit object to '{}': {e}",
                    object_path.display()
                )
            })?;
            timings.codegen += lower_elapsed;
            timings.units.push(UnitTiming {
                name: module_name.to_string(),
                hit: true,
                elapsed: lower_elapsed,
            });
            return Ok(Some(key));
        }
    }

    // The level's module half: LLVM's own `default<O<n>>` pipeline. It is run
    // over the module the machine below will write, with the machine for the
    // target-specific passes. At `-O0` it is `default<O0>`, which adds no pass,
    // so the IR the lowering wrote is the IR the target machine lowers.
    let compile_start = Instant::now();
    module.run_passes(opt.pipeline(), machine)?;

    // A pass can leave a module LLVM's own verifier rejects, and the object
    // writer assumes a verified module, so the check runs again after the
    // pipeline rather than only before it.
    module.verify()?;

    // A hosted program's functions carry unwind tables, as C functions do by
    // default on x86-64: a worker's panic unwinds them with `pthread_exit`
    // (see `kelp-rt/src/panic.rs`). Set here, after the IR the snapshots compare
    // is final, so the text of the lowered module does not change; a freestanding
    // image has no thread to end and keeps none.
    if freestanding.is_none() {
        module.set_unwind_tables();
    }
    machine.emit_object(&module, object_path)?;
    let compile_elapsed = compile_start.elapsed();
    if let Some(cache) = cache {
        cache.store(&key, object_path)?;
        timings.codegen += lower_elapsed;
        timings.unit_compile += compile_elapsed;
        timings.units.push(UnitTiming {
            name: module_name.to_string(),
            hit: false,
            elapsed: lower_elapsed + compile_elapsed,
        });
        Ok(None)
    } else {
        timings.codegen += lower_elapsed + compile_elapsed;
        Ok(None)
    }
}

/// The debug-info setting a cached object was built with. DWARF is always on
/// (MK7 S4, `compile`), so this is a constant that exists to make the key name
/// the setting rather than to choose one.
/// An all-constant array literal of at least this many elements is emitted as a
/// private global plus a copy instead of an aggregate value (MISSING-Nf3Rb).
pub(crate) const BIG_ARRAY_LITERAL: usize = 256;

/// The object-level settings the cache key names: the debug info format, and
/// that a hosted object carries unwind tables (`set_unwind_tables`), which the
/// IR text does not show. An object cached before that was added must not be
/// reused for a hosted program.
pub(crate) const DEBUG_INFO_SETTING: &str = "dwarf+uwtable";

/// The compiler's identity for the cache key: this `kelpc`'s version and the
/// LLVM that wrote the object. Both matter: an object is the IR run through a
/// specific LLVM, so a compiler built against a different LLVM must not reuse
/// these objects.
pub(crate) fn compiler_version() -> String {
    format!("kelpc {} llvm {}", env!("CARGO_PKG_VERSION"), llvm_ffi::version_string())
}

/// One unit's result from a worker: its cache key when it was a hit, the times
/// to merge into the build's totals, and its per-unit `--timings` line.
pub(crate) struct UnitDone {
    pub(crate) hit_key: Option<String>,
    pub(crate) codegen: Duration,
    pub(crate) unit_compile: Duration,
    pub(crate) timing: UnitTiming,
}

/// The number of unit-compilation worker threads, from `KELPC_JOBS`.
///
/// Unset, empty, unparsable or `0` is the default `2`. The default is not the
/// CPU count on purpose: this host has little memory, and every worker holds a
/// live LLVM context and the module it is compiling
/// (`kelp wiki page compiler/incremental.md`).
pub(crate) fn jobs_from(var: Option<&str>) -> usize {
    const DEFAULT: usize = 2;
    match var.map(str::trim) {
        None | Some("") => DEFAULT,
        Some(value) => match value.parse::<usize>() {
            Ok(jobs) if jobs > 0 => jobs,
            _ => DEFAULT,
        },
    }
}

/// A caught panic's message: the payload of `panic!("...")` or
/// `panic!("{...}")`, or a placeholder for a payload that is neither.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// The incremental mode (roadmap A-14 slices 1 and 3): partition the program
/// into compilation units, lower each into its own module and object under
/// `<out>.units/`, and link every object plus the runtime and the program's
/// `extern fun` object into the final binary.
///
/// The unit rules are `kelp wiki page compiler/incremental.md`'s: one unit per source
/// module id, holding that module's own items and the instances of its generic
/// functions, plus one `_support` unit holding every compiler-generated helper.
///
/// Cache misses are compiled on `KELPC_JOBS` worker threads (default 2), each
/// unit in its own LLVM context and each worker owning one target machine. A
/// result is written into a slot indexed by the unit's place in the unit list,
/// so the link line is the unit order whatever order the workers finished in.
/// A unit that fails or panics is recorded against its index and the other
/// workers still finish, which keeps the successful units' cache entries; after
/// every worker has joined, the build fails with the first failing unit's name.
///
/// `KELPC_DUMP_UNITS=<dir>` writes each unit's unoptimized IR to
/// `<dir>/<unit>.ll` as well, which is what the determinism test reads: the same
/// sources must produce byte-identical unit IR.
///
/// Each unit's object is looked up in the content-addressed cache before it is
/// compiled and stored after (roadmap A-14 slice 2, `crate::cache`): an
/// unchanged unit is a hit and its pass pipeline and codegen are skipped. A
/// link failure is not retried silently: the hit entries are deleted so the
/// next build recompiles them, and the error names the units that were reused.
/// The directory a names-mode build keeps its IR, objects and linker inputs
/// in. It lives under the temp directory and is removed when this value is
/// dropped, so nothing beside the binary is left behind on success or failure.
/// Outside names mode it is absent and the artifacts go beside the binary as
/// they always have.
pub(crate) struct Scratch {
    pub(crate) dir: Option<PathBuf>,
}

pub(crate) static SCRATCH_COUNT: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    pub(crate) fn for_names(names: bool) -> Result<Scratch, String> {
        if !names {
            return Ok(Scratch { dir: None });
        }
        let dir = std::env::temp_dir().join(format!(
            "kelp_obf_{}_{}",
            std::process::id(),
            SCRATCH_COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!("could not create the scratch directory '{}': {e}", dir.display())
        })?;
        Ok(Scratch { dir: Some(dir) })
    }

    /// The path the artifacts are named from: the output's file name inside the
    /// scratch directory, or the output path itself outside names mode.
    pub(crate) fn base(&self, out_path: &Path) -> PathBuf {
        match &self.dir {
            Some(dir) => dir.join(out_path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("kelp"))),
            None => out_path.to_path_buf(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

pub(crate) fn compile_units(
    program: &Program,
    drops: &DropPlan,
    sources: &SourceMap,
    triple: &str,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    obfuscate_names: bool,
    timings: &mut Timings,
) -> Result<(), String> {
    let cache = Cache::for_build(out_path);
    let scratch = Scratch::for_names(obfuscate_names)?;
    let base = scratch.base(out_path);
    let unit_dir = base.with_extension("units");
    std::fs::create_dir_all(&unit_dir).map_err(|e| {
        format!(
            "could not create the unit directory '{}': {e}",
            unit_dir.display()
        )
    })?;
    let dump_dir = std::env::var_os("KELPC_DUMP_UNITS").map(PathBuf::from);
    if let Some(dir) = &dump_dir {
        std::fs::create_dir_all(dir).map_err(|e| {
            format!(
                "could not create 'KELPC_DUMP_UNITS' directory '{}': {e}",
                dir.display()
            )
        })?;
    }
    // The program's `extern fun` bodies, capability glue and cap arrays are one
    // object for the whole program, and it has no unit to belong to. It is generated here rather than at the link,
    // because the units must know which program items it names before they set
    // any linkage.
    // The externs object defines no Kelp function: a trampoline calls only the
    // native symbol and the runtime, and a callback thunk calls through the
    // closure it was handed. So no unit's item is named by it.
    let units = plan_units(program);

    // One slot per unit, filled by whichever worker claims that unit. A slot
    // left `None` means no worker reached it, which the caller treats as a
    // failure so a unit is never silently dropped from the link.
    let workers = jobs_from(std::env::var("KELPC_JOBS").ok().as_deref()).min(units.len());
    let results: Mutex<Vec<Option<Result<UnitDone, String>>>> =
        Mutex::new((0..units.len()).map(|_| None).collect());
    if workers > 0 {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    // One machine per worker: it is a read-mostly description of
                    // the target, its cache-key fields are the same for every
                    // worker, and it is never shared across a thread boundary.
                    let machine = match TargetMachine::for_host_at(opt.codegen_level()) {
                        Ok(machine) => machine,
                        Err(message) => {
                            // Attribute the failure to the first unit; the other
                            // workers still run and keep their cache entries.
                            let mut slots = results.lock().expect("worker result lock");
                            if slots[0].is_none() {
                                slots[0] = Some(Err(message));
                            }
                            return;
                        }
                    };
                    loop {
                        let index = next.fetch_add(1, Ordering::SeqCst);
                        if index >= units.len() {
                            break;
                        }
                        let unit = &units[index];
                        let file = unit_file_name(&unit.name);
                        let ir_path = unit_dir.join(format!("{file}.ll"));
                        let object_path = unit_dir.join(format!("{file}.o"));
                        // The unit's LLVM context is its own: contexts are not
                        // shared between threads, and a panic drops only this
                        // unit's context instead of poisoning a shared one.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let ctx = Context::new();
                                let layout = machine.data_layout();
                                let mut local = Timings::new();
                                let hit_key = lower_to_object(
                                    program,
                                    drops,
                                    sources,
                                    &ctx,
                                    &machine,
                                    &layout,
                                    triple,
                                    &unit.name,
                                    &ir_path,
                                    &object_path,
                                    opt,
                                    None,
                                    Some(unit.clone()),
                                    Some(&cache),
                                    None,
                                    &mut local,
                                )?;
                                if let Some(dir) = &dump_dir {
                                    let dumped = dir.join(format!("{file}.ll"));
                                    std::fs::copy(&ir_path, &dumped).map_err(|e| {
                                        format!(
                                            "could not write the unit IR to '{}': {e}",
                                            dumped.display()
                                        )
                                    })?;
                                }
                                Ok::<UnitDone, String>(UnitDone {
                                    hit_key,
                                    codegen: local.codegen,
                                    unit_compile: local.unit_compile,
                                    timing: local
                                        .units
                                        .pop()
                                        .expect("one lowered unit pushes one timing"),
                                })
                            }));
                        let result = match outcome {
                            Ok(Ok(done)) => Ok(done),
                            Ok(Err(message)) => Err(format!(
                                "compilation unit '{}' failed: {message}",
                                unit.name
                            )),
                            Err(panic) => Err(format!(
                                "compilation unit '{}' panicked: {}",
                                unit.name,
                                panic_message(&panic)
                            )),
                        };
                        results.lock().expect("worker result lock")[index] = Some(result);
                    }
                });
            }
        });
    }

    // Collect the workers' results in unit order, which is the link order and
    // the `--timings` order. The first failure by unit index is the build's
    // error, so which unit is named does not depend on which worker lost a race.
    let mut objects = Vec::with_capacity(units.len());
    // The units whose object came from the cache, with their keys, so a link
    // failure can name them and delete exactly their entries.
    let mut cached_hits: Vec<(String, String)> = Vec::new();
    let mut failure: Option<String> = None;
    for (index, result) in results
        .into_inner()
        .expect("worker result lock")
        .into_iter()
        .enumerate()
    {
        match result {
            Some(Ok(done)) => {
                timings.codegen += done.codegen;
                timings.unit_compile += done.unit_compile;
                timings.units.push(done.timing);
                if let Some(key) = done.hit_key {
                    cached_hits.push((units[index].name.clone(), key));
                }
                let file = unit_file_name(&units[index].name);
                objects.push(unit_dir.join(format!("{file}.o")));
            }
            Some(Err(message)) => {
                if failure.is_none() {
                    failure = Some(message);
                }
            }
            None => {
                if failure.is_none() {
                    failure = Some(format!(
                        "compilation unit '{}' was not compiled",
                        units[index].name
                    ));
                }
            }
        }
    }
    // A unit failed: the build fails naming it, but every unit that did compile
    // has already stored its cache entry, so the next build resumes from there.
    if let Some(message) = failure {
        cache.enforce_cap();
        cache.report_stats();
        return Err(message);
    }

    // The program's `extern fun` trampolines, callback glue and capability
    // arrays are one object for the whole program, exactly as the whole-program
    // build has them: they belong to no unit.
    // The incremental build has no machine of its own outside its workers, so
    // the externs object is written with one made here, for the same target.
    let machine = TargetMachine::for_host_at(opt.codegen_level())?;
    let externs_path = base.with_extension("externs.o");
    let program_object = crate::extern_object::lower_externs_object(program, &machine, &externs_path, true)?
        .then_some(externs_path);

    let link_start = Instant::now();
    let linked = crate::driver::link_llvm_objects(
        &objects,
        program_object.as_deref(),
        out_path,
        opt,
        link_libs,
        library_dirs,
        pkg_config_flags,
        crate::extern_object::uses_tls(program),
        obfuscate_names,
    );
    timings.link += link_start.elapsed();
    // A unit object that came from the cache and does not link is corrupt in a
    // way the lookup could not see. Delete the hit entries rather than retry
    // silently: the next build misses and recompiles them, and the error names
    // the units that were reused so the failure is attributable.
    let result = match linked {
        Ok(()) => Ok(()),
        Err(e) => {
            for (_, key) in &cached_hits {
                cache.delete(key);
            }
            let mut message = e.message;
            if !cached_hits.is_empty() {
                let names: Vec<&str> = cached_hits.iter().map(|(unit, _)| unit.as_str()).collect();
                message = format!(
                    "the link failed with cached unit object(s) for {}; their cache entries were deleted, so the next build recompiles them\n{message}",
                    names.join(", "),
                );
            }
            Err(message)
        }
    };
    cache.enforce_cap();
    cache.report_stats();
    result
}

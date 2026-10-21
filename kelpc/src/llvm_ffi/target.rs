use super::*;

/// The portable architecture baseline `LLVMCreateTargetMachine` is given as
/// its CPU name for `triple` (MISSING-hV6qL). Not the host's actual CPU -
/// that is `LLVMGetHostCPUName`, which is `-march=native` and makes the
/// object non-portable, a different and unrelated thing. This just names
/// the architecture's own floor, the same baseline `clang -O2` (with no
/// `-march`) targets, so LLVM's X86 subtarget stops falling back to `i586`
/// tuning for an empty CPU string - a fallback with a real, cited FIXME in
/// LLVM's own source, not a deliberate choice anyone made for that string.
///
/// An architecture this doesn't know keeps the old empty-string behavior:
/// `kelpc` only ever builds for the host's own triple today
/// (`default_target_triple`), so only that triple's prefix needs an entry
/// here for the fallback to stop biting; an unrecognized prefix is exactly
/// as portable as before this function existed, just not any better tuned.
fn baseline_cpu(triple: &str) -> &'static str {
    if triple.starts_with("x86_64") {
        "x86-64"
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        "generic"
    } else {
        ""
    }
}

/// Guard for `LLVMTargetMachineRef`: the code generator for one triple.
///
/// This is what turns a verified module into an object, and it is why MK7 §1.3
/// keeps `cc` for linking: LLVM writes the object, the driver links it.
pub struct TargetMachine {
    pub(crate) raw: LLVMTargetMachineRef,
    /// The subtarget choices this machine was created with, kept so an object
    /// cache can describe the machine that produced an object (roadmap A-14
    /// slice 2). LLVM's C API has no getters for them, so they are recorded at
    /// creation rather than read back.
    pub(crate) triple: String,
    pub(crate) cpu: String,
    pub(crate) features: String,
    pub(crate) reloc: &'static str,
    pub(crate) code_model: &'static str,
}

/// The relocation model's name, part of a target machine's cache identity.
fn reloc_name(reloc: LLVMRelocMode) -> &'static str {
    match reloc {
        LLVMRelocMode::LLVMRelocDefault => "default",
        LLVMRelocMode::LLVMRelocStatic => "static",
        LLVMRelocMode::LLVMRelocPIC => "pic",
        LLVMRelocMode::LLVMRelocDynamicNoPic => "dynamic-no-pic",
        LLVMRelocMode::LLVMRelocROPI => "ropi",
        LLVMRelocMode::LLVMRelocRWPI => "rwpi",
        LLVMRelocMode::LLVMRelocROPI_RWPI => "ropi-rwpi",
    }
}

/// The code model's name, part of a target machine's cache identity.
fn code_model_name(model: LLVMCodeModel) -> &'static str {
    match model {
        LLVMCodeModel::LLVMCodeModelDefault => "default",
        LLVMCodeModel::LLVMCodeModelJITDefault => "jit-default",
        LLVMCodeModel::LLVMCodeModelTiny => "tiny",
        LLVMCodeModel::LLVMCodeModelSmall => "small",
        LLVMCodeModel::LLVMCodeModelKernel => "kernel",
        LLVMCodeModel::LLVMCodeModelMedium => "medium",
        LLVMCodeModel::LLVMCodeModelLarge => "large",
    }
}

/// The linked LLVM's version, from `LLVMGetVersion`. An object is a function of
/// the LLVM that wrote it as well as of the IR, so the cache key includes it: a
/// compiler built against a different LLVM must not reuse these objects.
pub fn version_string() -> String {
    let mut major = 0u32;
    let mut minor = 0u32;
    let mut patch = 0u32;
    // SAFETY: the three out-parameters point at our own slots.
    unsafe { LLVMGetVersion(&mut major, &mut minor, &mut patch) };
    format!("{major}.{minor}.{patch}")
}

impl TargetMachine {
    /// A target machine for the host triple, at
    /// [`LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault`] (the `-O2` codegen
    /// level).
    ///
    /// The shortcut for the one triple `kelpc` currently emits for at the
    /// default level. [`for_host_at`] is the shaped-for-a-level entry point
    /// `codegen_llvm::compile` uses; see [`for_triple`] for the parameters and
    /// the errors.
    ///
    /// [`for_host_at`]: TargetMachine::for_host_at
    /// [`for_triple`]: TargetMachine::for_triple
    pub fn for_host() -> Result<Self, String> {
        TargetMachine::for_host_at(LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault)
    }

    /// A target machine for the host triple at `level`, the codegen half of an
    /// optimization level (`opt::OptLevel::codegen_level`).
    ///
    /// The level chooses instruction selection, scheduling and register
    /// allocation; the module passes are the other half and `run_passes` runs
    /// them over the module this machine writes.
    pub fn for_host_at(level: LLVMCodeGenOptLevel) -> Result<Self, String> {
        let triple = default_target_triple();
        TargetMachine::for_triple(&triple, level)
    }

    /// A target machine for `triple` (`LLVMGetTargetFromTriple` and
    /// `LLVMCreateTargetMachine`), disposed on drop.
    ///
    /// `initialize_native_target` must have run first for a triple the host's
    /// LLVM build covers. The CPU is `baseline_cpu(triple)` (MISSING-hV6qL):
    /// an empty CPU string makes LLVM's X86 subtarget fall back to tuning for
    /// `i586`, whose scheduling model has no micro-op buffer, which silently
    /// turns off partial loop unrolling for every program this backend
    /// builds - not a feature choice, an LLVM fallback with no legitimate use
    /// here. Features are still left empty: no CPU-specific instruction-set
    /// extension is assumed, only the architecture's own baseline name is
    /// given, so this stays as portable as the old empty string was and is
    /// not `-march=native`. The code model is `LLVMCodeModelDefault`.
    /// Relocations are
    /// `LLVMRelocPIC`, not `LLVMRelocDefault`: LLVM's default for x86-64 Linux is
    /// a static reloc model, whose object carries absolute relocations, and the
    /// host `cc` builds PIE executables by default, so that object would fail to
    /// link with `relocation R_X86_64_32 can not be used when making a PIE
    /// object`. PIC, which is also what clang defaults to for this triple,
    /// produces the same kind of object the C backend's `cc` path does.
    ///
    /// `Err` carries LLVM's message when the triple names no compiled-in target,
    /// which is the one failure mode.
    pub fn for_triple(triple_str: &str, level: LLVMCodeGenOptLevel) -> Result<Self, String> {
        TargetMachine::create(
            triple_str,
            baseline_cpu(triple_str),
            "",
            level,
            LLVMRelocMode::LLVMRelocPIC,
            LLVMCodeModel::LLVMCodeModelDefault,
        )
    }

    /// A target machine for a freestanding triple (kernel stage 4): the same
    /// creation as [`for_triple`], with the choices a bare-metal image needs.
    ///
    /// A `riscv64` triple gets the `generic-rv64` CPU and `+m,+a,+c`: the
    /// extensions the QEMU `virt` machine's baseline core has, and the ones any
    /// RISC-V code has to assume to be portable across the ISA's profiles. An
    /// `aarch64` triple gets the `generic` CPU and no extra features, because
    /// the baseline ARMv8-A feature set (Advanced SIMD included) is already what
    /// `generic` means and a kernel 0 uses none of it. Every other triple keeps
    /// `baseline_cpu` and no features, exactly as the hosted path does.
    /// Relocations are `LLVMRelocStatic`, because there is no
    /// dynamic loader to fix up a position-independent object. riscv64 uses the
    /// `medium` code model, which reaches the whole address space a kernel image
    /// is placed in without the long-call sequences of `large`; aarch64 has no
    /// `medium` code model at all (LLVM refuses it: only small, tiny and large
    /// are allowed), and `small` is the one that reaches a kernel image loaded
    /// at 0x40000000, so it is what aarch64 gets. x86-64 gets `small` too,
    /// because the x86-64 image is linked at 1 MiB and `kernel` assumes the code
    /// sits in the top 2 GiB.
    ///
    /// [`for_triple`]: TargetMachine::for_triple
    pub fn for_freestanding(
        triple_str: &str,
        level: LLVMCodeGenOptLevel,
    ) -> Result<Self, String> {
        let is_aarch64 = triple_str.starts_with("aarch64") || triple_str.starts_with("arm64");
        let is_x86_64 = triple_str.starts_with("x86_64");
        let (cpu, features) = if triple_str.starts_with("riscv64") {
            ("generic-rv64", "+m,+a,+c")
        } else if is_aarch64 {
            ("generic", "")
        } else {
            (baseline_cpu(triple_str), "")
        };
        // `small` is the model that reaches an image linked at 1 MiB on
        // x86-64; `medium` is LLVM's x86-64 default and larger than the low
        // 2 GiB needs, and `kernel` assumes the code sits in the top 2 GiB.
        let code_model = if is_aarch64 || is_x86_64 {
            LLVMCodeModel::LLVMCodeModelSmall
        } else {
            LLVMCodeModel::LLVMCodeModelMedium
        };
        TargetMachine::create(
            triple_str,
            cpu,
            features,
            level,
            LLVMRelocMode::LLVMRelocStatic,
            code_model,
        )
    }

    /// The shared body of [`for_triple`] and [`for_freestanding`]: look the
    /// triple up, then create the machine with the given subtarget choices.
    ///
    /// [`for_triple`]: TargetMachine::for_triple
    /// [`for_freestanding`]: TargetMachine::for_freestanding
    fn create(
        triple_str: &str,
        cpu: &str,
        features: &str,
        level: LLVMCodeGenOptLevel,
        reloc: LLVMRelocMode,
        code_model: LLVMCodeModel,
    ) -> Result<Self, String> {
        let triple = cstring(triple_str);
        let mut target = null_mut();
        let mut message: *mut c_char = null_mut();
        // SAFETY: the out-parameters point at our slots, and the target has no
        // dispose of its own (LLVM owns the registration).
        let status =
            unsafe { LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut message) };
        if status != 0 {
            let text = unsafe { take_message(message) };
            return Err(if text.is_empty() {
                format!("LLVM has no target for the triple '{}'", triple.to_string_lossy())
            } else {
                text
            });
        }
        let cpu = cstring(cpu);
        let features = cstring(features);
        // SAFETY: the target came from the call above and is registered; LLVM
        // copies both strings. `cpu` is a portable architecture baseline (see
        // `baseline_cpu`), never the host's own CPU - that would be
        // `-march=native` and make the object non-portable, which this is
        // not.
        let raw = unsafe {
            LLVMCreateTargetMachine(
                target,
                triple.as_ptr(),
                cpu.as_ptr(),
                features.as_ptr(),
                level,
                reloc,
                code_model,
            )
        };
        if raw.is_null() {
            return Err(format!(
                "LLVM could not create a target machine for the triple '{}'",
                triple.to_string_lossy()
            ));
        }
        Ok(TargetMachine {
            raw,
            triple: triple_str.to_string(),
            cpu: cpu.to_string_lossy().into_owned(),
            features: features.to_string_lossy().into_owned(),
            reloc: reloc_name(reloc),
            code_model: code_model_name(code_model),
        })
    }

    /// The triple this machine targets.
    pub fn triple(&self) -> &str {
        &self.triple
    }

    /// The CPU name the machine was created with (a portable architecture
    /// baseline, not the host's own CPU; see `baseline_cpu`).
    pub fn cpu(&self) -> &str {
        &self.cpu
    }

    /// The subtarget feature string, empty for every machine this compiler
    /// builds except a freestanding riscv64 one.
    pub fn features(&self) -> &str {
        &self.features
    }

    /// The relocation model's name.
    pub fn reloc(&self) -> &'static str {
        self.reloc
    }

    /// The code model's name.
    pub fn code_model(&self) -> &'static str {
        self.code_model
    }

    /// The underlying `LLVMTargetMachineRef`, for a C API call this wrapper lacks.
    pub fn as_raw(&self) -> LLVMTargetMachineRef {
        self.raw
    }

    /// Write an object file for `module` to `path`
    /// (`LLVMTargetMachineEmitToFile` with `LLVMObjectFile`).
    ///
    /// The module should have a triple and should have verified: codegen assumes
    /// both. The write replaces `path`, and the file that results is what the
    /// driver hands to `cc`. `Err` carries LLVM's message when codegen fails.
    pub fn emit_object(&self, module: &Module<'_>, path: &Path) -> Result<(), String> {
        let path = cstring(&path.to_string_lossy());
        let mut message: *mut c_char = null_mut();
        // SAFETY: the machine and module are live, the path is valid, and the
        // out-parameter points at our slot.
        let status = unsafe {
            LLVMTargetMachineEmitToFile(
                self.raw,
                module.as_raw(),
                path.as_ptr(),
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut message,
            )
        };
        if status == 0 {
            return Ok(());
        }
        let text = unsafe { take_message(message) };
        Err(if text.is_empty() {
            format!(
                "LLVM could not write an object to '{}'",
                path.to_string_lossy()
            )
        } else {
            text
        })
    }
}

impl Drop for TargetMachine {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMCreateTargetMachine and is disposed once.
        unsafe { LLVMDisposeTargetMachine(self.raw) };
    }
}

/// Guard for `LLVMTargetDataRef`: the layout the target machine computes for a
/// type.
///
/// The LLVM backend needs two numbers from it to build a tagged union, because
/// an LLVM struct has no way to declare a field's alignment other than through
/// the field's own type: the widest payload's size sets the union's size, and
/// its alignment is the field type the union is anchored on (see
/// `codegen_llvm.rs::enum_union_type`). Asking the target instead of assuming
/// keeps the layout a property of the triple rather than of x86-64 (MK7 §3.1).
///
/// Created from a [`TargetMachine`] with [`TargetMachine::data_layout`] and
/// disposed on drop.
pub struct DataLayout {
    pub(crate) raw: LLVMTargetDataRef,
}

impl TargetMachine {
    /// This machine's data layout (`LLVMCreateTargetDataLayout`), disposed on
    /// drop. LLVM allocates a fresh `LLVMTargetDataRef` per call, so this is a
    /// guard rather than a view of data the machine owns.
    pub fn data_layout(&self) -> DataLayout {
        // SAFETY: the machine is live and owns the target it was created from.
        DataLayout {
            raw: unsafe { LLVMCreateTargetDataLayout(self.raw) },
        }
    }
}

impl DataLayout {
    /// The store size of `ty` in bytes as the target lays it out
    /// (`LLVMABISizeOfType`), including the trailing padding up to the type's
    /// own alignment. `ty` must be a sized type.
    pub fn abi_size_of(&self, ty: Type<'_>) -> u64 {
        // SAFETY: the layout and the type are live, and the type has a body
        // (an opaque one would be LLVM's own error, not unsoundness here).
        unsafe { LLVMABISizeOfType(self.raw, ty.as_raw()) }
    }

    /// The ABI alignment of `ty` in bytes as the target lays it out
    /// (`LLVMABIAlignmentOfType`). `ty` must be a sized type.
    pub fn abi_alignment_of(&self, ty: Type<'_>) -> u32 {
        // SAFETY: as above.
        unsafe { LLVMABIAlignmentOfType(self.raw, ty.as_raw()) }
    }

    /// Whether the target lays a multi-byte value out least-significant byte
    /// first (`LLVMByteOrder`). The byte-order conversions read this rather
    /// than assuming the host, so `to_le` is the identity exactly on a
    /// little-endian target and a swap on a big-endian one.
    pub fn is_little_endian(&self) -> bool {
        // SAFETY: the layout is live and `LLVMByteOrder` only reads it.
        unsafe { LLVMByteOrder(self.raw) == LLVMByteOrdering::LLVMLittleEndian }
    }
}

impl Drop for DataLayout {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMCreateTargetDataLayout and is disposed once.
        unsafe { LLVMDisposeTargetData(self.raw) };
    }
}

use super::*;

/// Handle for `LLVMMetadataRef`: one debug descriptor (a file, a compile unit,
/// a subroutine type, a subprogram or a source location).
///
/// LLVM owns the node: it lives in the metadata context of the LLVM context
/// that built it and is freed with that context, so this is a `Copy` handle
/// rather than a guard. The lifetime marker ties it to the [`Context`] the
/// [`DebugBuilder`] that made it was built in, which is what stops a node from
/// outliving the context it belongs to.
#[derive(Clone, Copy)]
pub struct Metadata<'ctx> {
    pub(crate) raw: LLVMMetadataRef,
    pub(crate) context_marker: PhantomData<&'ctx Context>,
}

/// Guard for `LLVMDIBuilderRef`: the DWARF debug-information builder of one
/// module.
///
/// This is where the metadata a `.kp` file and a `.kp` line become DWARF is
/// constructed (MK7 §4.4). The C API's builder is a `DIBuilder` in C++, and it
/// is disposed here the same way [`Context`] and [`Module`] are: the builder
/// must be finalized before it is disposed, so [`DebugBuilder::finalize`] runs
/// first, and it must be disposed before the module it was built for drops,
/// which the lifetime marker and the ordinary drop order of the two guards give.
///
/// The builder carries no ordering requirement of its own beyond that: a
/// subprogram may be created before or after the function it describes, as long
/// as the module is live for both.
pub struct DebugBuilder<'ctx> {
    pub(crate) raw: LLVMDIBuilderRef,
    // The context is kept rather than re-read from the module: a source location
    // is built from the context plus a scope, and LLVM has no "builder's
    // context" accessor. Valid because the marker ties this to the guard that
    // owns the context.
    pub(crate) context: LLVMContextRef,
    // `finalize` is idempotent from LLVM's side only in the sense that nothing
    // reads its return; running it twice is harmless but running it after the
    // module has been written is too late, so the flag is what keeps the drop
    // path from being the second caller.
    pub(crate) finalized: Cell<bool>,
    // Whether a pass pipeline runs over the module. DWARF carries it as the
    // compile unit's and each subprogram's `isOptimized` flag, which is what a
    // debugger reads to know an instruction may have moved. It is `opt::OptLevel::is_optimized`,
    // false only at `-O0`.
    pub(crate) optimized: bool,
    pub(crate) context_marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DebugBuilder<'ctx> {
    /// A debug builder for `module` (`LLVMCreateDIBuilder`), finalized and
    /// disposed on drop.
    ///
    /// `optimized` is the compile unit's `isOptimized` flag and the same flag
    /// on each subprogram: true when a pass pipeline will run over the module,
    /// so a debugger is told the lines it reads are the optimized ones. The
    /// unresolved-node variant is not used: everything here is created
    /// explicitly, in order, and nothing is left for a later pass to tie up.
    pub fn new(module: &Module<'ctx>, optimized: bool) -> Self {
        DebugBuilder {
            // SAFETY: the module is live for 'ctx, and the builder is owned and
            // disposed on drop.
            raw: unsafe { LLVMCreateDIBuilder(module.as_raw()) },
            context: module.context,
            finalized: Cell::new(false),
            optimized,
            context_marker: PhantomData,
        }
    }

    /// A file descriptor for `path` (`LLVMDIBuilderCreateFile`).
    ///
    /// DWARF carries a file as a name plus a directory; a reader joins them and
    /// an empty directory means the compilation directory. `split_path` keeps
    /// the two parts of a `.kp` path apart for that reason.
    pub fn file(&self, path: &Path) -> Metadata<'ctx> {
        let (directory, filename) = split_path(path);
        let filename = cstring(&filename);
        let directory = cstring(&directory);
        Metadata {
            // SAFETY: the builder is live and both strings outlive the call.
            raw: unsafe {
                LLVMDIBuilderCreateFile(
                    self.raw,
                    filename.as_ptr(),
                    filename.as_bytes().len(),
                    directory.as_ptr(),
                    directory.as_bytes().len(),
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A compile unit whose main file is `file`
    /// (`LLVMDIBuilderCreateCompileUnit`).
    ///
    /// The language is spelled `C` because DWARF has no Kelp code and the tag is
    /// otherwise inert: what this milestone emits is a line table (a file, a
    /// subprogram and a source location per statement), never a type or a
    /// variable, so nothing downstream reads the language out of the unit.
    ///
    /// `isOptimized` is `true` when a pass pipeline runs over this module
    /// (`opt::OptLevel::is_optimized`): the flag tells a debugger the line it
    /// reads may be an instruction that moved, and it is false at `-O0`, where
    /// the IR the lowering wrote is the IR the target machine lowers.
    pub fn compile_unit(&self, file: Metadata<'ctx>, producer: &str) -> Metadata<'ctx> {
        let producer = cstring(producer);
        let empty = cstring("");
        Metadata {
            // SAFETY: the builder is live, `file` is a metadata node in the same
            // context, and every string outlives the call. Empty is passed for
            // the flags, the split name, the sysroot and the SDK, and 0 for the
            // runtime version and the DWO id: there is no split DWARF and no
            // profiled debug info here.
            raw: unsafe {
                LLVMDIBuilderCreateCompileUnit(
                    self.raw,
                    LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
                    file.raw,
                    producer.as_ptr(),
                    producer.as_bytes().len(),
                    self.optimized as LLVMBool,
                    empty.as_ptr(),
                    0,
                    0,
                    empty.as_ptr(),
                    0,
                    LLVMDWARFEmissionKind::LLVMDWARFEmissionKindFull,
                    0,
                    0,
                    0,
                    empty.as_ptr(),
                    0,
                    empty.as_ptr(),
                    0,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// The type of a subroutine that takes no arguments and returns nothing
    /// (`LLVMDIBuilderCreateSubroutineType`).
    ///
    /// One shared node for every function is enough because nothing here
    /// describes a parameter or a value: the unit is a line table, and a
    /// subprogram needs a type only to be well formed. `LLVMDIFlagPrototyped`
    /// says the empty list is a real prototype rather than an unknown one.
    pub fn subroutine_type(&self, file: Metadata<'ctx>) -> Metadata<'ctx> {
        // An empty parameter list is an empty array, and LLVM's C API takes the
        // array as a pointer plus a length. The zero-length array is borrowed
        // for the call only; LLVM copies it into a metadata node.
        let mut params: [LLVMMetadataRef; 0] = [];
        Metadata {
            // SAFETY: the builder is live, `file` belongs to its context, and the
            // array pointer is valid for the zero elements the length claims.
            raw: unsafe {
                LLVMDIBuilderCreateSubroutineType(
                    self.raw,
                    file.raw,
                    params.as_mut_ptr(),
                    0,
                    LLVMDIFlagPrototyped,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A subprogram `name` defined at `line` of `file`, in `scope`, with the
    /// subroutine type `ty` (`LLVMDIBuilderCreateFunction`).
    ///
    /// `scope` is the function's file, which is what clang passes for a
    /// file-scope function; the unit is the builder's own compile unit either
    /// way, so the descriptor does not have to be handed the unit to be part of
    /// it. `linkage_name` is the symbol the object defines, which is not always
    /// the `.kp` name (a mangled function keeps its mangled symbol).
    /// `scope_line` is the line the body opens on and is given the same value as
    /// `line`, which is what a one-line declaration wants; `is_local_to_unit`
    /// is false and `is_optimized` is the builder's own flag, for the reason
    /// `compile_unit` gives.
    #[allow(clippy::too_many_arguments)]
    pub fn subprogram(
        &self,
        scope: Metadata<'ctx>,
        name: &str,
        linkage_name: &str,
        file: Metadata<'ctx>,
        line: u32,
        ty: Metadata<'ctx>,
    ) -> Metadata<'ctx> {
        let name = cstring(name);
        let linkage_name = cstring(linkage_name);
        Metadata {
            // SAFETY: the builder is live, every metadata node belongs to its
            // context, and the strings outlive the call.
            raw: unsafe {
                LLVMDIBuilderCreateFunction(
                    self.raw,
                    scope.raw,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    linkage_name.as_ptr(),
                    linkage_name.as_bytes().len(),
                    file.raw,
                    line,
                    ty.raw,
                    0,
                    1,
                    line,
                    LLVMDIFlagPrototyped,
                    self.optimized as LLVMBool,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A source location at `line:column` inside `scope`
    /// (`LLVMDIBuilderCreateDebugLocation`).
    ///
    /// There is no inlined-at location: nothing in either backend inlines.
    pub fn location(&self, line: u32, column: u32, scope: Metadata<'ctx>) -> Metadata<'ctx> {
        Metadata {
            // SAFETY: the builder's context is live and `scope` is a metadata
            // node in it; a null inlined-at is LLVM's "not inlined".
            raw: unsafe {
                LLVMDIBuilderCreateDebugLocation(
                    self.context,
                    line,
                    column,
                    scope.raw,
                    null_mut(),
                )
            },
            context_marker: PhantomData,
        }
    }

    /// Attach `subprogram` to `function` (`LLVMSetSubprogram`), which is what
    /// makes `function`'s own line the subprogram's `line` in the object.
    pub fn set_subprogram(&self, function: Function<'ctx>, subprogram: Metadata<'ctx>) {
        // SAFETY: the function and the metadata node are live and belong to one
        // module's context.
        unsafe { LLVMSetSubprogram(function.as_raw(), subprogram.raw) };
    }

    /// Construct the descriptors the builder deferred
    /// (`LLVMDIBuilderFinalize`).
    ///
    /// This must run after the last metadata node is made and before the module
    /// is written, so `codegen_llvm.rs` calls it explicitly; the `Drop` below
    /// repeats it only for a builder that never got there, because disposing an
    /// unfinalized builder is what LLVM's own contract forbids.
    pub fn finalize(&self) {
        if self.finalized.replace(true) {
            return;
        }
        // SAFETY: the builder is live and this is its first finalize.
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }

    /// The underlying `LLVMDIBuilderRef`, for a C API call this wrapper lacks.
    pub fn as_raw(&self) -> LLVMDIBuilderRef {
        self.raw
    }
}

impl Drop for DebugBuilder<'_> {
    fn drop(&mut self) {
        // SAFETY: the builder is live; `finalize` runs at most once either way.
        self.finalize();
        // SAFETY: raw came from LLVMCreateDIBuilder and is disposed once.
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

/// Split a `.kp` path into the directory and the file name a `DIFile` carries
/// (`LLVMDIBuilderCreateFile`).
///
/// DWARF keeps the two apart and a reader joins them, so an empty directory
/// means "the compilation directory". A bare file name therefore gets `.`, the
/// directory that means the process's working directory, rather than an empty
/// string a reader would have to interpret.
fn split_path(path: &Path) -> (String, String) {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => (
            dir.to_string_lossy().into_owned(),
            name.to_string_lossy().into_owned(),
        ),
        (_, Some(name)) => (".".to_string(), name.to_string_lossy().into_owned()),
        _ => (String::new(), path.to_string_lossy().into_owned()),
    }
}

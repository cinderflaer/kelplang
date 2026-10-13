//! Managed mode's lowering support (WP3 of the GC design,
//! `kelpscript-gc-paper.md` sections 3.2, 3.4 and 8.2).
//!
//! Three things live here:
//!
//! 1. the static type descriptor (`kelp_gc_type`, paper 3.2) that every GC
//!    allocation names, built from the program's own layouts;
//! 2. the pointer-slot walk that fills in a descriptor's `ptr_off` list;
//! 3. the managed drop policy: which types still owe a drop call once memory
//!    is collected rather than freed (paper 7.2).
//!
//! The collector itself is WP1's (`kelpc/gc`, exporting C-ABI symbols). This
//! module only names those symbols and the struct they take. Nothing here
//! depends on a C file.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::llvm_ffi::{Context, FnType, Function as LlvmFunction, Module, Type, Value};
use crate::tir::Program;
use crate::types::{ty_mangle, Ty};

/// `KELP_GC_NOSCAN` in the descriptor's `flags`: the payload has no GC pointer
/// slots, so the collector never reads it (paper 3.2).
pub const KELP_GC_NOSCAN: u64 = 1;
/// `KELP_GC_ENUM` in the descriptor's `flags`: the tag selects a variant's pointer list.
pub const KELP_GC_ENUM: u64 = 2;

/// The C-ABI symbols WP3 emits calls to (paper 3.4). WP1 defines them; until it
/// lands, the stub in `tests/managed_lower/gc_stub.rs` fixes the same
/// signatures, and the golden test checks the IR against that stub.
pub const GC_ALLOC: &str = "kelp_gc_alloc";
pub const GC_ALLOC_ARRAY: &str = "kelp_gc_alloc_array";
pub const GC_INIT: &str = "kelp_gc_init";
pub const GC_SAFEPOINT: &str = "kelp_gc_safepoint";
pub const GC_THREAD_ATTACH: &str = "kelp_gc_thread_attach";
pub const GC_THREAD_DETACH: &str = "kelp_gc_thread_detach";

/// The minimum heap a managed program starts with (the collection trigger
/// floor), 8 MiB, paper 3.4. The maximum is the manifest's `heap_max`, in
/// `managed::DEFAULT_HEAP_MAX_MIB` when it names none.
pub const DEFAULT_HEAP_MIN: u64 = 8 << 20;

/// The descriptor id reserved for the noscan `u8` byte buffer (`String`,
/// `Bytes`). The lowering's own table counts up from 1 and must stay below it.
pub const U8_BUFFER_DESCR_ID: u64 = 0xFFFF0;

/// The layout facts descriptor emission needs from the program. `codegen_llvm`
/// implements it on its own state, so this module never reaches into `Gen`.
pub trait GcLayout {
    /// The field types of a `datac` in declaration order. An error names a
    /// type the managed lowering does not describe yet.
    fn struct_fields(&self, name: &str) -> Result<Vec<Ty>, String>;
    /// The ABI `(size, align)` of a value of `ty` in the target's layout.
    fn size_align(&self, ty: &Ty) -> Result<(u64, u64), String>;
    /// The payload field types of each variant of the enum `name`, in
    /// declaration order, or `None` when `name` is not an enum.
    fn enum_variants(&self, name: &str) -> Option<Vec<Vec<Ty>>>;
}

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

/// Byte offset of a `Map` header's index buffer pointer: the entries pointer
/// is first, then three `i64` counts, then the index (`codegen_llvm.rs`,
/// `emit_map_bodies`: `{ ptr, i64, i64, i64, ptr, i64, i64 }`).
pub const MAP_INDEX_OFFSET: u64 = 32;

/// Byte offset of a closure value's environment pointer (`{ fn, env, drop }`).
pub const CLOSURE_ENV_OFFSET: u64 = 8;

/// The C layout of a sequence of fields laid out one after another: the total
/// size, the alignment, and each field's offset. The same walk
/// `pointer_offsets` makes for a `datac`, so a descriptor and the LLVM struct
/// agree on every offset.
#[cfg_attr(not(test), allow(dead_code))]
pub fn fields_layout(
    layout: &dyn GcLayout,
    fields: &[Ty],
) -> Result<(u64, u64, Vec<u64>), String> {
    let mut offset = 0;
    let mut align_max = 1;
    let mut offsets = Vec::with_capacity(fields.len());
    for field in fields {
        let (size, align) = layout.size_align(field)?;
        offset = align_up(offset, align);
        offsets.push(offset);
        offset += size;
        align_max = align_max.max(align);
    }
    Ok((align_up(offset, align_max), align_max, offsets))
}

/// The pointer slots of a map entry record `{ key, value, occupied }`, which is
/// the element type of a `Map`'s entries buffer. Returns the record's size and
/// alignment with the pointer offsets, so the caller can name it as the
/// buffer's element descriptor.
#[cfg_attr(not(test), allow(dead_code))]
pub fn map_entry_layout(
    layout: &dyn GcLayout,
    key: &Ty,
    value: &Ty,
) -> Result<(u64, u64, Vec<u32>), String> {
    let (size, align, offsets) = fields_layout(layout, &[key.clone(), value.clone(), Ty::U8])?;
    let mut ptrs = Vec::new();
    pointer_offsets(layout, key, offsets[0], &mut ptrs)?;
    pointer_offsets(layout, value, offsets[1], &mut ptrs)?;
    Ok((size, align, ptrs.into_iter().map(|p| p as u32).collect()))
}

/// The pointer slots of a closure environment: the captures in environment
/// order, laid out as a C struct (`codegen_llvm.rs`, `declare_closure_envs`).
/// Returns the environment's size and alignment with the slot offsets.
#[cfg_attr(not(test), allow(dead_code))]
pub fn env_layout(
    layout: &dyn GcLayout,
    captures: &[Ty],
) -> Result<(u64, u64, Vec<u32>), String> {
    let (size, align, offsets) = fields_layout(layout, captures)?;
    let mut ptrs = Vec::new();
    for (cap, offset) in captures.iter().zip(offsets) {
        pointer_offsets(layout, cap, offset, &mut ptrs)?;
    }
    Ok((size, align, ptrs.into_iter().map(|p| p as u32).collect()))
}

/// Append the byte offset (from `base`) of every GC pointer slot in a value of
/// `ty`, in address order.
///
/// A `Box`, `String`, `Bytes` and `List` value is one pointer at its own
/// offset: the object base. A `datac` is walked field by field with the C
/// layout, which is the same layout the LLVM struct gets (`repr(c)`). A
/// `[T; N]` repeats its element. Scalars and native handles hold no GC pointer.
///
/// Anything else is an error, not a silent empty list: a `Map`, an enum, a
/// closure environment or a trait object needs its own descriptor shape (paper
/// 3.2, enum and variant lists; 3.1, closures), and a missing slot is a
/// missed root once the collector runs.
pub fn pointer_offsets(
    layout: &dyn GcLayout,
    ty: &Ty,
    base: u64,
    out: &mut Vec<u64>,
) -> Result<(), String> {
    match ty {
        Ty::Box(_) | Ty::String | Ty::Bytes | Ty::List(_) => out.push(base),
        // A `Map` header is `{ entries, len, cap, cap2, index, count, mask }`
        // (`codegen_llvm.rs` `emit_map_bodies`): the entries buffer at offset 0
        // and the `u32` index buffer at `MAP_INDEX_OFFSET`. Both are base
        // pointers, and each buffer names its own descriptor in its header.
        Ty::Map(..) => {
            out.push(base);
            out.push(base + MAP_INDEX_OFFSET);
        }
        // A closure value is the fat `{ fn, env, drop }` (`codegen_llvm.rs`,
        // closures). Only `env` is a GC pointer; `fn` and `drop` are code.
        Ty::Closure { .. } => out.push(base + CLOSURE_ENV_OFFSET),
        // A borrow owns nothing: the owner is walked where it is declared.
        Ty::Ref { .. } => {}
        // An atomic cell holds one scalar by value (`atomic_alias_elem`: bool and
        // the integer widths), so its walk is its element's walk, which holds no
        // pointer slot. Decision 41: an `AtomicBool` in a GC object is a scalar.
        Ty::Atomic(elem) => pointer_offsets(layout, elem, base, out)?,
        Ty::Struct(name) => {
            if layout.enum_variants(name).is_some() {
                return Err(format!(
                    "a hosted build describes the enum '{name}' only as a heap object (a Box payload or a List element); an enum stored inside a datac is not described yet"
                ));
            }
            let fields = layout.struct_fields(name)?;
            let (_, _, offsets) = fields_layout(layout, &fields)?;
            for (field, offset) in fields.iter().zip(offsets) {
                pointer_offsets(layout, field, base + offset, out)?;
            }
        }
        Ty::Array(elem, count) => {
            let (size, _) = layout.size_align(elem)?;
            for index in 0..*count {
                pointer_offsets(layout, elem, base + index * size, out)?;
            }
        }
        Ty::I32
        | Ty::I64
        | Ty::U32
        | Ty::U64
        | Ty::F32
        | Ty::F64
        | Ty::Bool
        | Ty::Char
        | Ty::Unit
        | Ty::U8
        | Ty::U16
        | Ty::Usize
        | Ty::Isize
        | Ty::Str
        | Ty::Fn { .. }
        | Ty::Ptr(_)
        | Ty::Handle(_) => {}
        other => {
            return Err(format!(
                "a hosted build does not describe a value of type `{}` in a GC object yet",
                ty_mangle(other)
            ));
        }
    }
    Ok(())
}

/// Whether a runtime record that holds a value of `ty` must root it (section
/// 3). A handle type owns no collected slot of its own: its record roots its own
/// value, so the handle itself is not walked. Any other type is walked as an
/// object would be, and a type the walk does not describe is an error, the same
/// one its descriptor would give.
pub fn record_needs_root(layout: &dyn GcLayout, ty: &Ty) -> Result<bool, String> {
    match ty {
        Ty::Mutex(_) | Ty::RwLock(_) | Ty::Thread(_) | Ty::Condvar | Ty::Unit => Ok(false),
        _ => {
            let mut offsets = Vec::new();
            pointer_offsets(layout, ty, 0, &mut offsets)?;
            Ok(!offsets.is_empty())
        }
    }
}

/// The types whose drop still matters in a managed build: a user destructor, a
/// runtime record that has to be released (thread, mutex, rwlock, condvar), a
/// trait object (its concrete type is unknown here, so the drop is kept), a
/// closure (its captures may hold one), and any `datac`, `enum`, `List`, `Map`,
/// `Box` or array that holds one of those.
///
/// Memory alone is not in the set. A `String`, a `List` of integers or a `Box`
/// of a `datac` of integers is collected, so it owes no drop at all (paper 7.2).
///
/// `resources` is [`resource_types`] of the program.
pub fn managed_needs_drop(ty: &Ty, resources: &HashSet<String>) -> bool {
    match ty {
        Ty::Struct(name) => resources.contains(name),
        Ty::Thread(_)
        | Ty::Mutex(_)
        | Ty::RwLock(_)
        | Ty::Condvar
        | Ty::BoxDyn { .. }
        | Ty::Closure { .. } => true,
        Ty::List(elem) | Ty::Box(elem) | Ty::Array(elem, _) | Ty::Atomic(elem) => {
            managed_needs_drop(elem, resources)
        }
        Ty::Map(key, value) => managed_needs_drop(key, resources) || managed_needs_drop(value, resources),
        _ => false,
    }
}

/// The `datac` and `enum` names that transitively hold a resource. A type with
/// a user destructor is one of them, as is a type with a field or payload that
/// is one. This is a fixed point over the declarations, since a `datac` may name
/// an earlier `datac` or `enum` by value.
pub fn resource_types(program: &Program) -> HashSet<String> {
    let mut set: HashSet<String> = program.drop_types.iter().cloned().collect();
    loop {
        let before = set.len();
        for s in &program.structs {
            if s.fields.iter().any(|f| managed_needs_drop(&f.ty, &set)) {
                set.insert(s.name.clone());
            }
        }
        for e in &program.enums {
            let holds = e
                .variants
                .iter()
                .any(|v| v.fields.iter().any(|f| managed_needs_drop(&f.ty, &set)));
            if holds {
                set.insert(e.name.clone());
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// The LLVM side of the descriptors: the `kelp_gc_type` struct, the one
/// allocation entry point and the cache of descriptors already emitted into the
/// module. Built once per module in managed mode.
pub struct GcTypes<'a> {
    /// `struct kelp_gc_type` (paper 3.2): `{ u32 id, u32 size, u32 align,
    /// u32 flags, u32 n_ptrs, ptr ptr_off, u32 tag_off, u32 n_variants,
    /// ptr variants }`. The field order and widths match the C struct, so the
    /// LLVM layout and the C layout are the same 48 bytes.
    pub descr_ty: Type<'a>,
    /// `struct kelp_gc_variant`: `{ u32 n_ptrs, ptr ptr_off }`, one per enum variant.
    pub variant_ty: Type<'a>,
    /// `void* kelp_gc_alloc(const kelp_gc_type* t)`.
    pub alloc: (LlvmFunction<'a>, FnType<'a>),
    /// `void kelp_gc_init(size_t heap_min, size_t heap_max, size_t start_low,
    /// size_t start_high, size_t headroom_pct, size_t trigger_rule)`, called once from
    /// `main` before anything else (paper 3.4).
    pub init: (LlvmFunction<'a>, FnType<'a>),
    /// `void* kelp_gc_alloc_array(const kelp_gc_type* elem, size_t count)`: a
    /// zeroed array of `count` elements, the form every container buffer takes
    /// (paper 3.4).
    pub alloc_array: (LlvmFunction<'a>, FnType<'a>),
    /// `void kelp_gc_safepoint(void)`, a poll at loop back edges and call sites
    /// (paper 5.1).
    pub safepoint: (LlvmFunction<'a>, FnType<'a>),
    /// `void kelp_gc_thread_attach(void)`: called once by `main` after
    /// `kelp_gc_init`, and at the top of every spawned thread body, before its
    /// first allocation.
    pub thread_attach: (LlvmFunction<'a>, FnType<'a>),
    /// `void kelp_gc_thread_detach(void)`. The spawned body does not call it:
    /// the runtime's cleanup handler does, on a return and on a panic alike.
    pub thread_detach: (LlvmFunction<'a>, FnType<'a>),
    cache: RefCell<HashMap<String, Value<'a>>>,
}

impl<'a> GcTypes<'a> {
    /// Declare the struct and the collector's entry points in `module`.
    pub fn declare(ctx: &'a Context, module: &'a Module<'a>) -> Self {
        let ptr = ctx.opaque_ptr();
        let i32t = ctx.i32();
        let descr_ty = ctx.struct_type("kelp_gc_type");
        ctx.struct_set_body(
            descr_ty,
            &[i32t, i32t, i32t, i32t, i32t, ptr, i32t, i32t, ptr],
        );
        let variant_ty = ctx.struct_type("kelp_gc_variant");
        ctx.struct_set_body(variant_ty, &[i32t, ptr]);
        let alloc_ty = ctx.fn_type(ptr, &[ptr], false);
        let alloc = (module.add_function(GC_ALLOC, alloc_ty), alloc_ty);
        let array_ty = ctx.fn_type(ptr, &[ptr, ctx.i64()], false);
        let alloc_array = (module.add_function(GC_ALLOC_ARRAY, array_ty), array_ty);
        let init_ty = ctx.fn_type(ctx.void(), &[ctx.i64(), ctx.i64(), ctx.i64(), ctx.i64(), ctx.i64(), ctx.i64()], false);
        let init = (module.add_function(GC_INIT, init_ty), init_ty);
        let poll_ty = ctx.fn_type(ctx.void(), &[], false);
        let safepoint = (module.add_function(GC_SAFEPOINT, poll_ty), poll_ty);
        let thread_ty = ctx.fn_type(ctx.void(), &[], false);
        let thread_attach = (module.add_function(GC_THREAD_ATTACH, thread_ty), thread_ty);
        let thread_detach = (module.add_function(GC_THREAD_DETACH, thread_ty), thread_ty);
        GcTypes {
            descr_ty,
            variant_ty,
            alloc,
            alloc_array,
            init,
            safepoint,
            thread_attach,
            thread_detach,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// The module-level descriptor of `ty`, emitted on first use. The `id` is
    /// the descriptor's index in emission order, so a program that lowers the
    /// same way always gets the same ids.
    pub fn descriptor(
        &self,
        ctx: &'a Context,
        module: &'a Module<'a>,
        layout: &dyn GcLayout,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let key = ty_mangle(ty);
        if let Some(existing) = self.cache.borrow().get(&key) {
            return Ok(*existing);
        }
        if let Ty::Struct(name) = ty {
            if let Some(variants) = layout.enum_variants(name) {
                let (size, align) = layout.size_align(ty)?;
                return self.enum_descriptor(ctx, module, layout, &key, size, align, &variants);
            }
        }
        let mut offsets = Vec::new();
        pointer_offsets(layout, ty, 0, &mut offsets)?;
        let (size, align) = layout.size_align(ty)?;
        let offsets: Vec<u32> = offsets.into_iter().map(|o| o as u32).collect();
        Ok(self.descriptor_raw(ctx, module, &key, size, align, &offsets))
    }

    /// The descriptor of an enum (`KELP_GC_ENUM`): the tag is the u32 at offset
    /// 0 of the `{ tag, union }` struct, the union starts at the first offset
    /// past the tag that its widest payload's alignment allows, and each
    /// variant lists its own pointer slots from the payload start. The variant
    /// table is indexed by the tag value.
    fn enum_descriptor(
        &self,
        ctx: &'a Context,
        module: &'a Module<'a>,
        layout: &dyn GcLayout,
        key: &str,
        size: u64,
        align: u64,
        variants: &[Vec<Ty>],
    ) -> Result<Value<'a>, String> {
        if let Some(existing) = self.cache.borrow().get(key) {
            return Ok(*existing);
        }
        let i32t = ctx.i32();
        let ptr = ctx.opaque_ptr();
        let mut payloads = Vec::with_capacity(variants.len());
        let mut union_align = 1u64;
        for fields in variants {
            let (_, field_align, offsets) = fields_layout(layout, fields)?;
            union_align = union_align.max(field_align);
            payloads.push((fields, offsets));
        }
        let union_off = align_up(4, union_align);
        let mut any_ptr = false;
        let mut per_variant = Vec::with_capacity(payloads.len());
        for (fields, offsets) in &payloads {
            let mut ptrs = Vec::new();
            for (field, offset) in fields.iter().zip(offsets) {
                pointer_offsets(layout, field, union_off + offset, &mut ptrs)?;
            }
            any_ptr |= !ptrs.is_empty();
            per_variant.push(ptrs.into_iter().map(|p| p as u32).collect::<Vec<u32>>());
        }
        let variant_ty = self.variant_ty;
        let mut entries = Vec::with_capacity(per_variant.len());
        for (index, ptrs) in per_variant.iter().enumerate() {
            let (count, slots) = if ptrs.is_empty() {
                (0, ctx.const_null(ptr))
            } else {
                let elems: Vec<Value<'a>> =
                    ptrs.iter().map(|off| ctx.const_int(i32t, u64::from(*off), false)).collect();
                let arr_ty = ctx.array_type(i32t, ptrs.len() as u64);
                let arr = module.add_global(&format!("kelp_gc_ptrs_{key}_{index}"), arr_ty);
                arr.set_initializer(ctx.const_array(i32t, &elems));
                arr.set_constant(true);
                arr.set_internal_linkage();
                (ptrs.len() as u64, arr)
            };
            entries.push(ctx.const_named_struct(variant_ty, &[ctx.const_int(i32t, count, false), slots]));
        }
        let variants_global = module.add_global(
            &format!("kelp_gc_variants_{key}"),
            ctx.array_type(variant_ty, entries.len() as u64),
        );
        variants_global.set_initializer(ctx.const_array(variant_ty, &entries));
        variants_global.set_constant(true);
        variants_global.set_internal_linkage();
        let flags = KELP_GC_ENUM | if any_ptr { 0 } else { KELP_GC_NOSCAN };
        let id = self.cache.borrow().len() as u64 + 1;
        assert!(id < U8_BUFFER_DESCR_ID, "descriptor table reached the reserved u8 buffer id");
        let fields = [
            ctx.const_int(i32t, id, false),
            ctx.const_int(i32t, size, false),
            ctx.const_int(i32t, align, false),
            ctx.const_int(i32t, flags, false),
            ctx.const_int(i32t, 0, false),
            ctx.const_null(ptr),
            ctx.const_int(i32t, 0, false),
            ctx.const_int(i32t, entries.len() as u64, false),
            variants_global,
        ];
        let init = ctx.const_named_struct(self.descr_ty, &fields);
        let global = module.add_global(&format!("kelp_gc_type_{key}"), self.descr_ty);
        global.set_initializer(init);
        global.set_constant(true);
        global.set_internal_linkage();
        self.cache.borrow_mut().insert(key.to_string(), global);
        Ok(global)
    }

    /// A descriptor with the given shape, emitted once under `key`. Used for the
    /// types that have no `Ty` of their own: a `Map` entry record and a closure
    /// environment (`map_entry_layout`, `env_layout`). `key` names the global
    /// and must be unique per shape.
    pub fn descriptor_raw(
        &self,
        ctx: &'a Context,
        module: &'a Module<'a>,
        key: &str,
        size: u64,
        align: u64,
        offsets: &[u32],
    ) -> Value<'a> {
        if let Some(existing) = self.cache.borrow().get(key) {
            return *existing;
        }
        let i32t = ctx.i32();
        let ptr = ctx.opaque_ptr();
        let flags = if offsets.is_empty() { KELP_GC_NOSCAN } else { 0 };
        let (offsets_ptr, count) = if offsets.is_empty() {
            (ctx.const_null(ptr), 0)
        } else {
            let elems: Vec<Value<'a>> = offsets
                .iter()
                .map(|off| ctx.const_int(i32t, u64::from(*off), false))
                .collect();
            let arr_ty = ctx.array_type(i32t, offsets.len() as u64);
            let arr = module.add_global(&format!("kelp_gc_ptrs_{key}"), arr_ty);
            arr.set_initializer(ctx.const_array(i32t, &elems));
            arr.set_constant(true);
            arr.set_internal_linkage();
            (arr, offsets.len() as u64)
        };
        let id = self.cache.borrow().len() as u64 + 1;
        assert!(id < U8_BUFFER_DESCR_ID, "descriptor table reached the reserved u8 buffer id");
        let fields = [
            ctx.const_int(i32t, id, false),
            ctx.const_int(i32t, size, false),
            ctx.const_int(i32t, align, false),
            ctx.const_int(i32t, flags, false),
            ctx.const_int(i32t, count, false),
            offsets_ptr,
            ctx.const_int(i32t, 0, false),
            ctx.const_int(i32t, 0, false),
            ctx.const_null(ptr),
        ];
        let init = ctx.const_named_struct(self.descr_ty, &fields);
        let global = module.add_global(&format!("kelp_gc_type_{key}"), self.descr_ty);
        global.set_initializer(init);
        global.set_constant(true);
        global.set_internal_linkage();
        self.cache.borrow_mut().insert(key.to_string(), global);
        global
    }
}

#[cfg(test)]
mod tests {
    use crate::borrowck;
    use crate::codegen_llvm;
    use crate::diag::SourceMap;
    use crate::infer;
    use crate::resolve;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("managed_lower")
            .join(name)
    }

    /// A project under `tests/managed/`, the same directory `kelpc build` reads.
    fn project(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("managed").join(name)
    }

    /// Lower a project's entry module to IR text with the collector cap its
    /// manifest names, the value `kelpc build` hands `codegen_llvm::compile`.
    /// A freestanding project gets no cap.
    pub(super) fn lower_project(dir: &Path) -> Result<String, String> {
        let mut sources = SourceMap::new();
        let loaded = resolve::load(dir, &mut sources, None, None).map_err(|d| format!("{d:?}"))?;
        let typed = infer::check_program(&loaded.program, &loaded.project).map_err(|d| format!("{d:?}"))?;
        let drops = borrowck::check_program(&typed).map_err(|d| format!("{d:?}"))?;
        let cap = loaded
            .project
            .target()
            .has_shared_pool()
            .then(|| codegen_llvm::HeapLimits::from_target(loaded.project.target()));
        // One directory per project: the tests run in parallel threads of one
        // process, so a shared name would let one test read another's IR.
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let out = std::env::temp_dir().join(format!("kelpc-gc-project-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        codegen_llvm::compile_ir(
            &typed,
            &drops,
            &sources,
            cap,
            &out.join("main.ll"),
            &out.join("main.o"),
        )
    }

    /// Lower one fixture to IR text. `managed` picks the managed branch; the
    /// hosted branch is the same code with the flag off.
    pub(super) fn lower(name: &str, managed: bool) -> Result<String, String> {
        let mut sources = SourceMap::new();
        let loaded = resolve::load(&fixture(name), &mut sources, None, None)
            .map_err(|d| format!("{d:?}"))?;
        let typed = infer::check_program(&loaded.program, &loaded.project)
            .map_err(|d| format!("{d:?}"))?;
        let drops = borrowck::check_program(&typed).map_err(|d| format!("{d:?}"))?;
        let dir = std::env::temp_dir().join(format!(
            "kelpc-gc-test-{}-{}",
            std::process::id(),
            if managed { "managed" } else { "hosted" }
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem = name.trim_end_matches(".kp");
        codegen_llvm::compile_ir(
            &typed,
            &drops,
            &sources,
            managed.then(|| codegen_llvm::HeapLimits::from_target(&crate::resolve::TargetDecl::hosted())),
            &dir.join(format!("{stem}.ll")),
            &dir.join(format!("{stem}.o")),
        )
    }

    /// The golden IR is written, not generated, by the test: running with
    /// `KELPC_WRITE_GOLDEN=1` replaces the fixture with the current output, so a
    /// reviewer can read the diff before it is committed.
    /// The debug info names the fixture's directory, which is this checkout's
    /// path. The golden text names it as `@FIXTURE_DIR@` instead, so the test
    /// passes in any worktree.
    fn normalize(ir: &str) -> String {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("managed_lower");
        ir.replace(&dir.display().to_string(), "@FIXTURE_DIR@")
    }

    pub(super) fn check_golden(actual: &str, golden: &str) {
        let path = fixture(golden);
        let actual = normalize(actual);
        if std::env::var("KELPC_WRITE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, &actual).expect("write golden");
            return;
        }
        let expected = std::fs::read_to_string(&path).expect("read golden fixture");
        assert_eq!(actual, expected, "managed IR differs from {golden}");
    }

    #[test]
    fn node_descriptor_lists_the_two_box_slots() {
        let ir = lower("node.kp", true).expect("managed lowering of node.kp");
        check_golden(&ir, "node.managed.ll");
        // One descriptor for Node: two pointer slots at 0 and 8, not noscan.
        assert!(ir.contains("@kelp_gc_ptrs_Node = internal constant [2 x i32] [i32 0, i32 8]"), "{ir}");
        assert!(ir.contains("@kelp_gc_type_Node = internal constant %kelp_gc_type { i32 1, i32 16, i32 8, i32 0, i32 2, ptr @kelp_gc_ptrs_Node, i32 0, i32 0, ptr null }"), "{ir}");
    }

    #[test]
    fn box_of_allocates_through_kelp_gc_alloc() {
        let ir = lower("node.kp", true).expect("managed lowering of node.kp");
        assert!(ir.contains("declare ptr @kelp_gc_alloc(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @kelp_gc_alloc(ptr @kelp_gc_type_Node)"), "{ir}");
        // The managed module never reaches libc's allocator for a Box.
        assert!(!ir.contains("call ptr @malloc"), "{ir}");
    }

    #[test]
    fn managed_build_emits_no_free_for_a_memory_only_value() {
        let ir = lower("node.kp", true).expect("managed lowering of node.kp");
        // The `declare void @free` line is libc's, always present. What matters
        // is that no call reaches it, and no drop function for Node exists.
        assert!(!ir.lines().any(|l| l.contains("call") && l.contains("@free(")), "{ir}");
        assert!(!ir.contains("kelp_drop_Node"), "{ir}");
        // The Box drop for Node is still defined (Box<Node> is instantiated), but
        // its body only clears the slot: no call in it, so no free.
        let body = ir
            .split("define internal void @kelp_box_drop__Node")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("the Box<Node> drop body");
        assert!(!body.contains("call"), "{body}");
    }

    /// Build a managed fixture through the real build path (`codegen_llvm::compile`
    /// with the collector archive `KELP_GC_LIB` names), then run it. Returns the
    /// program's stdout. The archive is built from branch `gc-core`
    /// (`kelpc/gc`, `cargo build --release`).
    pub(super) fn link_and_run(name: &str) -> String {
        let mut sources = SourceMap::new();
        let loaded = resolve::load(&fixture(name), &mut sources, None, None).expect("load fixture");
        let typed = infer::check_program(&loaded.program, &loaded.project).expect("check fixture");
        let drops = borrowck::check_program(&typed).expect("borrowck fixture");
        let dir = std::env::temp_dir().join(format!("kelpc-gc-test-{}-managed", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let stem = name.trim_end_matches(".kp");
        let exe = dir.join(stem);
        let managed = codegen_llvm::Managed {
            heap: codegen_llvm::HeapLimits::from_target(&crate::resolve::TargetDecl::hosted()),
            gc_archive: crate::driver::managed_archive().map_err(|e| e.message).expect("KELP_GC_LIB"),
        };
        codegen_llvm::compile(
            &typed,
            &drops,
            &sources,
            &exe,
            crate::opt::OptLevel::O0,
            &[],
            &[],
            &[],
            None,
            Some(&managed),
            false,
            &mut crate::timings::Timings::new(),
        )
        .expect("managed build");
        let output = std::process::Command::new(&exe).output().expect("run managed program");
        assert!(output.status.success(), "exit {:?}", output.status);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Known blocker: the archive from `gc-core` references
    /// `rust_eh_personality` without defining it (a `panic = "abort"` staticlib),
    /// so these fail to link until the collector defines that symbol. The fix
    /// belongs in `kelpc/gc`, not in the link line. The managed build path
    /// itself (`codegen_llvm::compile` with `Managed`) is wired and calls
    /// `driver::link_managed_object`; only the collector's missing export stops
    /// the link.
    ///
    /// The node fixture prints `false`, because `tree.left` is the non-empty box.
    #[test]
    #[ignore = "links WP1's kelp_gc static library: set KELP_GC_LIB and run with --ignored"]
    fn link_smoke_runs_node_under_the_collector() {
        assert_eq!(link_and_run("node.kp"), "false");
    }

    /// The hello fixture allocates one Box through the collector and prints it.
    #[test]
    #[ignore = "links WP1's kelp_gc static library: set KELP_GC_LIB and run with --ignored"]
    fn link_smoke_hello_prints_under_the_collector() {
        assert_eq!(link_and_run("hello.kp"), "hello from the managed build");
    }

    /// `[target] heap_max = 64` reaches `kelp_gc_init` as the maximum: 64 MiB in
    /// bytes, with the minimum still the 8 MiB default and the first-clearing
    /// guesses and headroom at their defaults (1 MiB, 4 MiB, 10 percent).
    #[test]
    fn managed_project_initialises_the_collector_to_its_heap_max() {
        let ir = lower_project(&project("heap_max_64")).expect("managed lowering of heap_max_64");
        assert!(
            ir.contains("call void @kelp_gc_init(i64 8388608, i64 67108864, i64 1048576, i64 4194304, i64 10, i64 0)"),
            "{ir}"
        );
        assert!(!ir.contains("i64 268435456,"), "the default cap must not be emitted: {ir}");
    }

    /// `[tidepool]` start guesses and headroom reach `kelp_gc_init` in bytes
    /// (the percent unchanged), beside the cap.
    #[test]
    fn tidepool_section_reaches_the_collector_init() {
        let ir = lower_project(&project("tidepool_start")).expect("managed lowering of tidepool_start");
        assert!(
            ir.contains("call void @kelp_gc_init(i64 8388608, i64 67108864, i64 2097152, i64 8388608, i64 50, i64 0)"),
            "{ir}"
        );
    }

    /// `trigger_rule = "last-size"` reaches `kelp_gc_init` as its code, 1. The
    /// default (no key) is 0, as the tests above show.
    #[test]
    fn tidepool_trigger_rule_reaches_the_collector_init() {
        let ir = lower_project(&project("tidepool_rule")).expect("managed lowering of tidepool_rule");
        assert!(
            ir.contains("call void @kelp_gc_init(i64 8388608, i64 67108864, i64 1048576, i64 4194304, i64 50, i64 1)"),
            "{ir}"
        );
    }

    /// The manifest's `heap_max` is refused at load, so a bad cap never reaches
    /// lowering. The named error is the manifest's own.
    #[test]
    fn managed_bad_heap_max_is_refused_before_lowering() {
        for dir in ["heap_max_zero", "heap_max_too_big"] {
            let err = lower_project(&project(dir)).expect_err("a bad heap_max must be refused");
            assert!(err.contains("'[target] heap_max'"), "{dir}: {err}");
        }
    }

    #[test]
    fn hosted_build_still_frees_and_has_no_gc_symbols() {
        let ir = lower("node.kp", false).expect("hosted lowering of node.kp");
        assert!(!ir.contains("kelp_gc_"), "{ir}");
        assert!(ir.contains("@malloc("), "{ir}");
    }
}

#[cfg(test)]
mod hosted_identity {
    use super::tests::lower;

    /// The hosted IR of `node.kp` is byte-identical to origin/main's, recorded
    /// as `hosted_node.ll` by running origin/main's kelpc on the same file. The
    /// managed branches are behind the managed flag, so this is the check that
    /// they do not change hosted output.
    #[test]
    fn hosted_ir_matches_origin_main() {
        let actual = lower("node.kp", false).expect("hosted lowering of node.kp");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("managed_lower");
        let normalized = actual.replace(&dir.display().to_string(), "@FIXTURE_DIR@");
        if std::env::var("KELPC_WRITE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(dir.join("hosted_node.ll"), &normalized).expect("write hosted golden");
            return;
        }
        let golden = std::fs::read_to_string(dir.join("hosted_node.ll")).expect("read hosted fixture");
        assert_eq!(normalized, golden, "hosted IR differs from origin/main");
    }
}

#[cfg(test)]
mod container_layout {
    //! The pointer slots of the container shapes, against a fixed layout: a
    //! `String` or `List` header is 24 bytes with its buffer pointer first, a
    //! `Map` header has its index at 32, a closure value's env is at 8.
    use super::*;

    struct Fixed;

    impl GcLayout for Fixed {
        fn struct_fields(&self, name: &str) -> Result<Vec<Ty>, String> {
            Err(format!("no fields for {name} in this test"))
        }
        fn enum_variants(&self, _name: &str) -> Option<Vec<Vec<Ty>>> {
            None
        }
        fn size_align(&self, ty: &Ty) -> Result<(u64, u64), String> {
            Ok(match ty {
                Ty::I32 => (4, 4),
                Ty::I64 => (8, 8),
                Ty::U8 => (1, 1),
                Ty::Box(_) => (8, 8),
                Ty::String | Ty::Bytes | Ty::List(_) => (24, 8),
                Ty::Map(..) => (56, 8),
                Ty::Closure { .. } => (24, 8),
                other => return Err(format!("no layout for {other:?}")),
            })
        }
    }

    fn string_map() -> Ty {
        Ty::Map(Box::new(Ty::String), Box::new(Ty::I32))
    }

    #[test]
    fn a_string_and_a_list_header_point_at_their_buffer() {
        let mut out = Vec::new();
        pointer_offsets(&Fixed, &Ty::String, 0, &mut out).unwrap();
        assert_eq!(out, vec![0]);
        out.clear();
        pointer_offsets(&Fixed, &Ty::List(Box::new(Ty::I64)), 0, &mut out).unwrap();
        assert_eq!(out, vec![0]);
    }

    #[test]
    fn a_bytes_buffer_has_no_pointer_slots() {
        let mut out = Vec::new();
        pointer_offsets(&Fixed, &Ty::U8, 0, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn a_map_header_points_at_entries_and_index() {
        let mut out = Vec::new();
        pointer_offsets(&Fixed, &string_map(), 0, &mut out).unwrap();
        assert_eq!(out, vec![0, MAP_INDEX_OFFSET]);
    }

    #[test]
    fn a_map_entry_lists_the_key_string_and_no_value_pointer() {
        let (size, align, ptrs) = map_entry_layout(&Fixed, &Ty::String, &Ty::I32).unwrap();
        // { String (24), i32 (4), u8 } is 32 bytes with the value at 24.
        assert_eq!((size, align), (32, 8));
        assert_eq!(ptrs, vec![0]);
    }

    #[test]
    fn a_map_entry_with_a_box_value_lists_its_pointer_after_the_key() {
        let boxed = Ty::Box(Box::new(Ty::I64));
        let (_, _, ptrs) = map_entry_layout(&Fixed, &Ty::String, &boxed).unwrap();
        // The key is 24 bytes, the Box value starts at 24.
        assert_eq!(ptrs, vec![0, 24]);
    }

    #[test]
    fn a_closure_env_lists_each_pointer_capture_at_its_offset() {
        let captures = vec![Ty::Box(Box::new(Ty::I32)), Ty::I64, Ty::String];
        let (size, align, ptrs) = env_layout(&Fixed, &captures).unwrap();
        assert_eq!((size, align), (40, 8));
        assert_eq!(ptrs, vec![0, 16]);
    }

    #[test]
    fn a_closure_value_names_its_env_pointer_only() {
        let closure = Ty::Closure { params: vec![], ret: Box::new(Ty::Unit) };
        let mut out = Vec::new();
        pointer_offsets(&Fixed, &closure, 0, &mut out).unwrap();
        assert_eq!(out, vec![CLOSURE_ENV_OFFSET]);
    }
}

#[cfg(test)]
mod container_lowering {
    //! The container fixture (`managed_lower/containers.kp`): each heap
    //! container's allocation names the descriptor of its element, or of its
    //! entry record or environment, in managed mode.
    use super::tests::{check_golden, link_and_run, lower};

    #[test]
    fn containers_allocate_with_their_descriptors() {
        let ir = lower("containers.kp", true).expect("managed lowering of containers.kp");
        check_golden(&ir, "containers.managed.ll");
        // The List<String> buffer is an array of String: one pointer slot at 0.
        assert!(ir.contains("call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_String, i64"), "{ir}");
        // The Map entries are an array of the entry record { String, Item, live }:
        // the key's buffer at 0 and the value's name buffer at 24, size 64.
        assert!(ir.contains("@kelp_gc_type_map_entry."), "{ir}");
        assert!(ir.contains("i32 64, i32 8, i32 0, i32 2, ptr"), "{ir}");
        // The Map index is a noscan u32 array: no pointer slots.
        assert!(ir.contains("call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_u32, i64"), "{ir}");
        // The capturing literal's environment names its Box capture.
        assert!(ir.contains("@\"kelp_gc_type_env."), "{ir}");
        assert!(ir.contains("call ptr @kelp_gc_alloc(ptr @\"kelp_gc_type_env."), "{ir}");
        // Main joins the collector once, right after it starts (paper 5.1).
        assert!(ir.contains("call void @kelp_gc_thread_attach()"), "{ir}");
        // Managed containers never reach libc's allocator or free.
        assert!(!ir.contains("call ptr @malloc"), "{ir}");
        assert!(!ir.lines().any(|l| l.contains("call") && l.contains("@free(")), "{ir}");
    }

    #[test]
    #[ignore = "links WP1's kelp_gc static library: set KELP_GC_LIB and run with --ignored; the managed run comparison stays ignored until the collector links"]
    fn link_smoke_containers_prints_under_the_collector() {
        assert_eq!(link_and_run("containers.kp"), "1\n42");
    }
}

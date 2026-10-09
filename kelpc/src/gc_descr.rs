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

/// The C-ABI symbols WP3 emits calls to (paper 3.4). WP1 defines them; until it
/// lands, the stub in `tests/managed_lower/gc_stub.rs` fixes the same
/// signatures, and the golden test checks the IR against that stub.
pub const GC_ALLOC: &str = "kelp_gc_alloc";
pub const GC_INIT: &str = "kelp_gc_init";

/// The layout facts descriptor emission needs from the program. `codegen_llvm`
/// implements it on its own state, so this module never reaches into `Gen`.
pub trait GcLayout {
    /// The field types of a `datac` in declaration order. An error names a
    /// type the managed lowering does not describe yet.
    fn struct_fields(&self, name: &str) -> Result<Vec<Ty>, String>;
    /// The ABI `(size, align)` of a value of `ty` in the target's layout.
    fn size_align(&self, ty: &Ty) -> Result<(u64, u64), String>;
}

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
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
        Ty::Struct(name) => {
            let mut offset = 0;
            for field in layout.struct_fields(name)? {
                let (size, align) = layout.size_align(&field)?;
                offset = align_up(offset, align);
                pointer_offsets(layout, &field, base + offset, out)?;
                offset += size;
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
                "managed mode does not describe a value of type `{}` in a GC object yet",
                ty_mangle(other)
            ));
        }
    }
    Ok(())
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
    /// `void* kelp_gc_alloc(const kelp_gc_type* t)`.
    pub alloc: (LlvmFunction<'a>, FnType<'a>),
    /// `int32 kelp_gc_init(size_t heap_min, size_t heap_max)`, called once from
    /// `main` before anything else (paper 3.4).
    pub init: (LlvmFunction<'a>, FnType<'a>),
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
        let alloc_ty = ctx.fn_type(ptr, &[ptr], false);
        let alloc = (module.add_function(GC_ALLOC, alloc_ty), alloc_ty);
        let init_ty = ctx.fn_type(ctx.void(), &[ctx.i64(), ctx.i64()], false);
        let init = (module.add_function(GC_INIT, init_ty), init_ty);
        GcTypes {
            descr_ty,
            alloc,
            init,
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
        let mut offsets = Vec::new();
        pointer_offsets(layout, ty, 0, &mut offsets)?;
        let (size, align) = layout.size_align(ty)?;
        let id = self.cache.borrow().len() as u64 + 1;
        let flags = if offsets.is_empty() { KELP_GC_NOSCAN } else { 0 };
        let i32t = ctx.i32();
        let ptr = ctx.opaque_ptr();
        let (offsets_ptr, count) = if offsets.is_empty() {
            (ctx.const_null(ptr), 0)
        } else {
            let elems: Vec<Value<'a>> = offsets
                .iter()
                .map(|off| ctx.const_int(i32t, *off, false))
                .collect();
            let arr_ty = ctx.array_type(i32t, offsets.len() as u64);
            let arr = module.add_global(&format!("kelp_gc_ptrs_{key}"), arr_ty);
            arr.set_initializer(ctx.const_array(i32t, &elems));
            arr.set_constant(true);
            arr.set_internal_linkage();
            (arr, offsets.len() as u64)
        };
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
        self.cache.borrow_mut().insert(key, global);
        Ok(global)
    }
}

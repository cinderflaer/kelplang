//! The LLVM backend: lower `tir` to LLVM IR, a peer of `codegen_c.rs`.
//!
//! MK7 §3.1 names this module as the whole of the new work: the front half of the
//! compiler is unchanged and already backend-complete, so the lowering walks the
//! same typed IR the C backend walks and emits a different target. This is the
//! **S1** slice (MK7 §5): the scalar subset, `llvm` selectable, with byte-identical
//! stdout on the fixtures that use only it.
//!
//! # What S1 lowers
//!
//! - types: `i32 i64 u32 u64 f32 f64 bool char u8 u16 usize isize unit`, plus
//!   `str` as far as `print` needs it (a bare `ptr` to a NUL-terminated global
//!   string, which is what a string literal becomes)
//! - statements: `Let`, `While`, `Return`, `Expr`
//! - expressions: `IntLit` `CharLit` `FloatLit` `BoolLit` `StringLit` `Ident`
//!   `Unary` `Binary` `Call` `Assign` `If` `Block` `Template`
//! - calls: user functions, `print`/`print!`, and the `"<src>#to_<dst>"` numeric
//!   conversions
//!
//! # What S2A adds (MK7 S2, the `datac` slice)
//!
//! A `datac` whose fields are all scalars or earlier all-scalar `datac`s lowers
//! to an LLVM identified struct with the same field order and natural alignment
//! the C backend's typedef has (MK7 §3.1). Covered:
//!
//! - the struct type itself, as an identified (named) struct type, body set once
//!   per declaration in program order
//! - construction (`StructLit`, one `insertvalue` per field in declaration
//!   order), field read (`FieldAccess`, a `getelementptr` plus a `load`), field
//!   write, and whole-value assignment
//! - a struct as a local, as a `move` parameter, as a return value, and nested
//!   inside another struct
//! - the shared or exclusive borrow a `datac` parameter resolves to
//!   (`infer.rs::implicit_borrow_wrap`): `Ty::Ref` of a struct is the struct's
//!   address, so `p.x` through a borrowed `datac` parameter is the same
//!   `getelementptr` a plain local field read is
//!
//! A move is a load and a store, and an all-scalar struct owns nothing, so at
//! S2A there was no drop plan to run.
//!
//! # What S2B adds (MK7 S2, the runtime and heap slice)
//!
//! - the C runtime object: `runtime/kelp_rt.c` (the same text the C backend
//!   emits) compiled once by `build.rs` and linked into every LLVM binary, with
//!   the helpers reached by declaration, never re-implemented in IR
//! - `String` and `Bytes` as an identified `{ ptr, i64, i64 }` and `View` as
//!   `{ ptr, i64 }`, field for field the structs `kelp_rt.c` defines (MK7 §3.3)
//! - the builtin `Bytes`/`String`/`View` methods, and `print`/templates reading
//!   a buffer or a view as `%.*s` from its own length and pointer
//! - an owned `datac` field, the generated per-`datac` drop, and the
//!   `borrowck::DropPlan` emitted at the same points `codegen_c.rs` emits it:
//!   end of block, before a `return`, and before an assignment overwrites an
//!   owned binding. A move clears its source slot, so the drop a binding still
//!   owes on a path that did not move it frees nothing
//! - `panic`, `assert`, and the out-of-range panic of a narrowing conversion,
//!   all now the runtime's own text
//!
//! A runtime helper that returns a struct larger than two eightbytes uses the
//! ABI shape clang emits for it: the hidden return pointer, modelled here as an
//! explicit first `ptr` argument (see `RtFn.sret`). The two helpers that take
//! such a struct by value are left out, so a call to one is refused by name
//! rather than crossed wrongly. `View` is two eightbytes and crosses unchanged.
//!
//! # What S2C adds (MK7 S2, the tagged-union slice)
//!
//! An `enum` lowers to the `{ tag, union }` shape `codegen_c.rs::write_enum`
//! emits today (MK7 §3.1), with the same tag width and the same payload
//! offsets:
//!
//! - the tag is an `i32` at offset 0, the variant's index, exactly the C
//!   `int32_t kelp_tag`
//! - the union is emitted as a struct anchored on the widest payload's own
//!   alignment type and sized to the widest payload, which is the only way an
//!   LLVM struct can be given an alignment: the anchor is what puts the payload
//!   at the offset the C `union` gets (`enum_union_body`)
//! - each variant's payload is an identified struct with the variant's fields in
//!   declaration order, so a payload field's offset is the C variant struct's
//! - a variant with no payload keeps one byte, which is the `char kelp_none`
//!   `write_enum` gives it, so a payload-less-only enum is still the C size
//! - construction writes the zeroinitializer, then the tag, then each payload
//!   field into the union member's address, the same bytes the C compound
//!   literal's designators write
//! - the per-`enum` drop is one `switch` on the tag (`emit_drop_fn_bodies`), with
//!   a case per variant that owns a payload field, and the default block is the
//!   exit: only the live variant's payloads are freed
//!
//! `when`, `if let` and `?` need no node of their own: the frontend desugars
//! `when` (and therefore `if let`) into an `If` chain over `EnumTag`/`EnumField`
//! (`infer.rs::check_when`), so this slice lowers those two nodes plus `EnumLit`
//! and `Try`, which is what `tir.rs` actually carries. A `when` over a `&Shape`
//! and a `when` over a `move` shape differ only in the `place_of` call behind
//! the tag read.
//!
//! Exhaustiveness is the frontend's job (`check_when` refuses a non-exhaustive
//! match), so no runtime check is emitted here, exactly as the C backend emits
//! no `default:` arm.
//!
//! Everything else is **refused by name** with a clean diagnostic, never
//! miscompiled and never a panic: MK7 §2 keeps the C backend as the oracle and
//! the escape hatch, so a refusal is `--backend=c` and an honest stage number.
//!
//! # The rules this lowering keeps
//!
//! These are the places a second backend goes wrong, and MK7 §3.4 is the plan's
//! own statement of them:
//!
//! - A narrow `IntLit` (an `i128` in `tir`) is cut to the *expression's* type
//!   before the operation, exactly as `codegen_c.rs` casts its literal text, so a
//!   `u64`-wide literal in an `i32` context is truncated first.
//! - Signedness comes from `Ty`: `sdiv`/`udiv`, `srem`/`urem`, `ashr`/`lshr`,
//!   `icmp slt` vs `ult`. `Shr` on a signed type is arithmetic.
//! - No `nsw`/`nuw` on an arithmetic instruction, no `inbounds` on a
//!   data-dependent GEP, and no fast-math flag is ever emitted. The plain forms
//!   wrap by definition, which is what the C backend's `-fwrapv` promised; the C
//!   frontend flag has no counterpart here because there is nothing to turn off.
//!   The one `inbounds`/`nuw` in the IR is the struct-field GEP, where the index
//!   is a compile-time constant inside an object of that exact type, so the
//!   bounds are proven rather than asserted (see `llvm_ffi::Builder::build_struct_gep`).
//! - Division does not invent a zero check. The only guard the language has is
//!   constant folding in the frontend, and this backend mirrors that: a `sdiv` by
//!   zero is UB in LLVM exactly as it is in C.
//! - `print` and templates call libc `printf`, with the same format string and the
//!   same C ABI widths `codegen_c.rs::printf_piece_with` uses. A vararg narrower
//!   than `int` is promoted here, because LLVM, unlike C, does not promote it.
//! - The numeric conversions mirror `codegen_c.rs::numeric_conversion`: a widening
//!   is a plain cast, a narrowing that can lose a value is a range check that
//!   panics with the call's own `.kp` spelling.
//!
//! # No debug locations at S1
//!
//! The C backend writes a `#line` per statement so `cc` can name a `.kp` line.
//! The LLVM equivalent is DWARF, which is S4 (MK7 §5); S1 emits no debug metadata,
//! and the artifact contract it does carry is the textual IR beside the binary.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::{BinOp, UnOp};
use crate::borrowck::{DropPlan, DropTarget};
use crate::llvm_ffi::{
    self, BasicBlock, Builder, Context, DataLayout, FloatBinOp, FnType, Function as LlvmFunction,
    IntBinOp, IntCmp, Module, RealCmp, TargetMachine, Type, Value,
};
use crate::lexer::Span;
use crate::tir::{Block, Expr, ExprKind, Function, Program, Stmt, TemplatePiece};
use crate::types::{drop_fn_name, needs_drop, owned_types, struct_tag, Ty};

/// Build the whole program, verify it, write the textual IR beside the binary,
/// emit the object and link it with `cc`.
///
/// The artifact contract mirrors the C path's `<out>.c`: this writes `<out>.ll`
/// (MK7 §4.4) so the IR is inspectable next to the binary, and keeps the object
/// it hands `cc` as `<out>.o`. Linking itself is `driver::link_llvm_object`, the
/// sibling of `driver::compile_c`.
pub fn compile(program: &Program, drops: &DropPlan, out_path: &Path) -> Result<(), String> {
    llvm_ffi::initialize_native_target()?;
    let triple = llvm_ffi::default_target_triple();
    if triple.is_empty() {
        return Err("LLVM returned an empty host triple".to_string());
    }

    let ctx = Context::new();
    let mut module = Module::new(&ctx, "kelp");
    module.set_triple(&triple);

    // The target machine is created here, before any type is built, because the
    // tagged union's size and alignment are read back from its data layout
    // (`declare_aggregates`). It is the same machine the object is written with,
    // so the layout the types were built for and the layout codegen assumes
    // cannot be two different ones.
    let machine = TargetMachine::for_host()?;
    let layout = machine.data_layout();

    let mut lowering = Gen::new(&ctx, &module, program, drops, &layout)?;
    lowering.declare()?;
    // The generated per-`datac` and per-`enum` drops are defined before the
    // function bodies that call them, mirroring the order `codegen_c.rs` writes
    // the typedef and its drop routine in.
    lowering.emit_drop_fn_bodies()?;
    lowering.emit_all()?;

    // A caller must verify before emitting: LLVM's object writer assumes a
    // verified module and can crash on an unverified one rather than fail.
    module.verify()?;

    let ir_path = out_path.with_extension("ll");
    std::fs::write(&ir_path, module.to_ir_string()).map_err(|e| {
        format!("could not write generated IR to '{}': {e}", ir_path.display())
    })?;

    let object_path = out_path.with_extension("o");
    machine.emit_object(&module, &object_path)?;

    crate::driver::link_llvm_object(&object_path, out_path).map_err(|e| e.message)
}

/// One declared callable: the LLVM function, its LLVM type, and the `tir` return
/// type (which decides whether a call yields a value). A clone is cheap and keeps
/// the borrow of the function table out of the walk.
#[derive(Clone)]
struct FnInfo<'a> {
    func: LlvmFunction<'a>,
    ty: FnType<'a>,
    ret_ty: Ty,
}

/// One runtime helper the IR calls by declaration: the function, its LLVM type,
/// whether it yields a value (a `void` one must not be given a result name), the
/// struct it produces, and whether that production is the C ABI's hidden return
/// pointer.
#[derive(Clone, Copy)]
struct RtFn<'a> {
    func: LlvmFunction<'a>,
    ty: FnType<'a>,
    yields: bool,
    ret: Option<Type<'a>>,
    /// True when the C function returns a struct larger than two eightbytes,
    /// which the SysV ABI returns through a hidden pointer in `rdi`. clang marks
    /// that pointer `sret` in the IR it emits; this backend models it as an
    /// explicit first `ptr` parameter instead, which produces the *same machine
    /// call* (the destination lands in `rdi`, the real arguments follow) without
    /// growing the wrapper with a parameter-attribute API. The declared type is
    /// therefore the C signature's ABI shape rather than its written spelling.
    sret: bool,
}

/// One `enum`'s LLVM layout, mirroring the `{ tag, union }` typedef
/// `codegen_c.rs::write_enum` emits (MK7 §3.1).
#[derive(Clone)]
struct EnumInfo<'a> {
    /// The outer struct: field 0 is the `i32` tag, field 1 the union.
    llvm_ty: Type<'a>,
    /// The union type. It is a struct anchored on the widest payload's own
    /// alignment type, sized to the widest payload; the anchor field is what
    /// makes LLVM place it where the C `union` sits (`enum_union_body`).
    union_ty: Type<'a>,
    /// One payload type per variant, in declaration order. A variant with no
    /// payload gets a bare `i8`, which is the one-byte `char kelp_none` the C
    /// typedef gives it, so `union` sizes agree.
    variants: Vec<EnumVariant<'a>>,
}

/// One variant's payload: the identified struct its fields live in, and those
/// fields' `tir` types (the drop needs the types, the access needs the struct).
#[derive(Clone)]
struct EnumVariant<'a> {
    payload_ty: Type<'a>,
    fields: Vec<Ty>,
}

/// The tag is field 0 and the union field 1 of every `enum` struct, so those two
/// indices are named once rather than spelled at each projection.
const ENUM_TAG_FIELD: u32 = 0;
const ENUM_UNION_FIELD: u32 = 1;

/// Module-level state: the context, the module, the builder, and the runtime
/// symbols every function shares. Per-function state lives in [`FnGen`].
struct Gen<'a> {
    ctx: &'a Context,
    module: &'a Module<'a>,
    builder: Builder<'a>,
    program: &'a Program,
    funcs: HashMap<String, FnInfo<'a>>,
    printf_fn: LlvmFunction<'a>,
    printf_ty: FnType<'a>,
    panic_fn: LlvmFunction<'a>,
    panic_ty: FnType<'a>,
    /// The target's data layout, read for the tagged union's size and alignment.
    /// An LLVM struct has no field-alignment annotation, so the type is built
    /// from the numbers the target reports rather than from an assumption about
    /// x86-64 (`TargetMachine::data_layout`).
    layout: &'a DataLayout,
    /// One identified LLVM struct type per `datac`, keyed by the `tir` name. The
    /// body is set once per declaration in `declare_aggregates`, before any
    /// function signature is built, so a `datac` parameter or return names a
    /// complete type. An `enum` is a `Ty::Struct` too, so its outer type is in
    /// this same table, which is what lets `llvm_type` resolve one name.
    structs: HashMap<String, Type<'a>>,
    /// Each `enum`'s layout, keyed by the same `tir` name.
    enums: HashMap<String, EnumInfo<'a>>,
    /// Each `datac`'s field names in declaration order, so a `FieldAccess`
    /// resolves its field to the struct index a `getelementptr` needs without
    /// re-walking `program.structs`.
    field_index: HashMap<String, Vec<String>>,
    /// The `DropPlan`, keyed by the same node identity `codegen_c.rs` reads, so
    /// the two backends free the same set of owned locals at the same points
    /// (MK7 §3.4).
    drops: &'a DropPlan,
    /// The runtime helpers this slice reaches, keyed by their mangled symbol.
    /// `kelp_rt.c` defines them (compiled into the object `build.rs` leaves in
    /// `OUT_DIR`), and the module only declares them: the C backend emits the
    /// same text into one translation unit.
    runtime: HashMap<String, RtFn<'a>>,
    /// The generated per-`datac` drop functions (`kelp_drop_<name>`), the mirror
    /// of the ones `codegen_c.rs` emits beside each typedef.
    drop_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The `datac`/`enum` names that own memory, so `types::needs_drop` can
    /// answer for a type without re-walking the declarations.
    owned: HashSet<String>,
}

impl<'a> Gen<'a> {
    fn new(
        ctx: &'a Context,
        module: &'a Module<'a>,
        program: &'a Program,
        drops: &'a DropPlan,
        layout: &'a DataLayout,
    ) -> Result<Self, String> {
        let ptr = ctx.opaque_ptr();
        // The one libc symbol this slice reaches: `printf`, for `print` and
        // templates. `kelp_panic` and `kelp_assert` are the runtime's own now
        // that its object is linked, so they are declared with the rest of the
        // helpers instead of defined here.
        let printf_ty = ctx.fn_type(ctx.i32(), &[ptr], true);
        let printf_fn = module.add_function("printf", printf_ty);
        // The builtin heap layouts, field for field the structs `kelp_rt.c`
        // defines: a pointer, then a length, then a capacity (a `View` has no
        // capacity). Keeping the order and the widths identical is what lets a
        // pointer to one cross into the runtime (MK7 §3.3); the runtime's
        // functions all take the pair by pointer or by value through the same
        // C ABI the object was compiled for.
        let mut structs = HashMap::new();
        let bytes_ty = ctx.struct_type("kelp_Bytes");
        ctx.struct_set_body(bytes_ty, &[ptr, ctx.i64(), ctx.i64()]);
        structs.insert("Bytes".to_string(), bytes_ty);
        let string_ty = ctx.struct_type("kelp_String");
        ctx.struct_set_body(string_ty, &[ptr, ctx.i64(), ctx.i64()]);
        structs.insert("String".to_string(), string_ty);
        let view_ty = ctx.struct_type("kelp_View");
        ctx.struct_set_body(view_ty, &[ptr, ctx.i64()]);
        structs.insert("View".to_string(), view_ty);
        let runtime = declare_runtime(ctx, module, bytes_ty, string_ty, view_ty);
        let panic = *runtime
            .get("kelp_panic")
            .expect("kelp_panic is declared with the runtime");
        Ok(Gen {
            ctx,
            module,
            builder: Builder::new(ctx),
            program,
            funcs: HashMap::new(),
            printf_fn,
            printf_ty,
            panic_fn: panic.func,
            panic_ty: panic.ty,
            layout,
            structs,
            enums: HashMap::new(),
            field_index: HashMap::new(),
            drops,
            runtime,
            drop_fns: HashMap::new(),
            owned: owned_types(&program.structs, &program.enums),
        })
    }

    /// Refuse the program-level constructs this slice does not lower, declare
    /// every `datac` and `enum`, their drop functions, and every function.
    ///
    /// Refusing before any function is emitted means a program with a construct
    /// this slice cannot lower fails with the construct named, rather than with
    /// the first expression that mentions it.
    fn declare(&mut self) -> Result<(), String> {
        let program = self.program;
        // The aggregate types first, so a field this slice cannot lay out is
        // reported by its own construct rather than behind an unrelated std
        // declaration.
        self.declare_aggregates()?;
        // A `pd` is a module global plus an `atexit` registration, which is the
        // runtime slice's job.
        if !program.pds.is_empty() {
            return Err(refuse("'pd'", "S3"));
        }
        // `program.caps` holds each module's granted patterns, which is data, not
        // a construct: it is harmless to ignore while nothing calls a gated
        // function. A gated *call* is a stdlib `extern fun` and is refused by
        // name at the call site (S3).

        self.declare_drop_fns();

        let ctx = self.ctx;

        for f in &program.functions {
            // An `extern fun` is a declaration the runtime owns, not a body S1
            // can lower. Skip it here and refuse if a call names it, which keeps
            // a program that merely declares one (through an unused import)
            // buildable.
            if f.extern_ {
                continue;
            }
            if f.env.is_some() {
                return Err(refuse("'a closure'", "S2E"));
            }
            let (params, ret) = if f.name == "main" {
                // The C backend's `main` is `int main(int, char**)`; S1 mirrors
                // the type the C standard cares about and returns 0.
                (Vec::new(), ctx.i32())
            } else {
                let params = f
                    .params
                    .iter()
                    .map(|(_, ty)| llvm_type(ctx, &self.structs, ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let ret = llvm_ret_type(ctx, &self.structs, &f.ret)?;
                (params, ret)
            };
            let ty = ctx.fn_type(ret, &params, false);
            let func = self.module.add_function(&symbol_name(&f.name), ty);
            self.funcs.insert(
                f.name.clone(),
                FnInfo {
                    func,
                    ty,
                    ret_ty: f.ret.clone(),
                },
            );
        }
        Ok(())
    }

    /// Declare one identified LLVM struct type per `datac`, with the fields in
    /// declaration order and the target's natural alignment (MK7 §3.1).
    ///
    /// Field types are checked before any type is created, so a `datac` this
    /// slice cannot lay out fails with the construct named and nothing half
    /// declared. Every named type is created first and its body set afterwards,
    /// so a field may refer to a `datac` declared later in the vector; the
    /// frontend already guarantees the by-value dependency is acyclic, and the
    /// order here only has to be *a* complete one.
    /// Declare one identified LLVM type per `datac` and per `enum`, and set
    /// every body, in the program's merged declaration order (MK7 §3.1).
    ///
    /// Field and payload types are checked before any type is created, so an
    /// aggregate this slice cannot lay out fails with the construct named and
    /// nothing half declared. Every named type is created first and its body set
    /// afterwards, so a field may name a type whose body is set later in the
    /// same pass; the frontend already guarantees the by-value dependency is
    /// acyclic, and `order` (`types::StructDef::order` / `EnumDef::order`) is
    /// that topological order, shared by the two lists.
    ///
    /// The tagged union is where that order matters for a second reason: the
    /// union's size and alignment are read back from the target's data layout,
    /// so every payload type it measures is complete by the time it is built.
    fn declare_aggregates(&mut self) -> Result<(), String> {
        let program = self.program;
        let ctx = self.ctx;
        for s in &program.structs {
            for f in &s.fields {
                refuse_field(&f.ty)?;
            }
        }
        for e in &program.enums {
            for v in &e.variants {
                for f in &v.fields {
                    refuse_field(&f.ty)?;
                }
            }
        }

        // Create every named type before any body is set, so a body may refer
        // to a type whose own body comes later in this same pass.
        for s in &program.structs {
            let ty = ctx.struct_type(&struct_tag(&s.name));
            self.structs.insert(s.name.clone(), ty);
            self.field_index.insert(
                s.name.clone(),
                s.fields.iter().map(|f| f.name.clone()).collect(),
            );
        }
        for e in &program.enums {
            let llvm_ty = ctx.struct_type(&struct_tag(&e.name));
            self.structs.insert(e.name.clone(), llvm_ty);
            let union_ty = ctx.struct_type(&enum_union_name(&e.name));
            let variants = e
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| EnumVariant {
                    // A payload-less variant is a bare `i8` rather than an empty
                    // struct: an empty struct is zero bytes in LLVM and one in
                    // C, and the one byte is what sizes a union every variant of
                    // which carries nothing.
                    payload_ty: if v.fields.is_empty() {
                        ctx.i8()
                    } else {
                        ctx.struct_type(&enum_payload_name(&e.name, i))
                    },
                    fields: v.fields.iter().map(|f| f.ty.clone()).collect(),
                })
                .collect();
            self.enums.insert(
                e.name.clone(),
                EnumInfo {
                    llvm_ty,
                    union_ty,
                    variants,
                },
            );
        }

        // Set every body in the merged declaration order, so a field or payload
        // only ever names a type whose body is already complete.
        enum Decl<'p> {
            Record(&'p crate::types::StructDef),
            Sum(&'p crate::types::EnumDef),
        }
        let mut decls: Vec<(usize, Decl)> = program
            .structs
            .iter()
            .map(|s| (s.order, Decl::Record(s)))
            .chain(program.enums.iter().map(|e| (e.order, Decl::Sum(e))))
            .collect();
        decls.sort_by_key(|(order, _)| *order);
        for (_, decl) in decls {
            match decl {
                Decl::Record(s) => {
                    let ty = *self
                        .structs
                        .get(&s.name)
                        .expect("every datac got a type above");
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| llvm_type(ctx, &self.structs, &f.ty))
                        .collect::<Result<Vec<_>, _>>()?;
                    ctx.struct_set_body(ty, &fields);
                }
                Decl::Sum(e) => self.declare_enum_body(e)?,
            }
        }
        Ok(())
    }

    /// Set one `enum`'s three bodies: each variant's payload struct, the union,
    /// then the outer `{ i32, union }`.
    ///
    /// The union's shape is the whole subtlety. C's `union { struct v0; struct
    /// v1; }` places every member at offset 0 inside the union and sizes the
    /// union to its widest member aligned to its widest alignment. LLVM has no
    /// union type and no per-field alignment, so the union becomes a struct
    /// **anchored on an integer type with the union's own alignment** and padded
    /// out to the union's size. The anchor is what makes LLVM align the union
    /// field to the payload's alignment, which is the offset C gives it; the
    /// padding is what makes the whole `enum` the size C gives it. Both numbers
    /// come from the target's data layout rather than from x86-64 arithmetic.
    fn declare_enum_body(&mut self, e: &crate::types::EnumDef) -> Result<(), String> {
        let ctx = self.ctx;
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        for (i, v) in e.variants.iter().enumerate() {
            let payload_ty = info.variants[i].payload_ty;
            if v.fields.is_empty() {
                continue;
            }
            let fields = v
                .fields
                .iter()
                .map(|f| llvm_type(ctx, &self.structs, &f.ty))
                .collect::<Result<Vec<_>, _>>()?;
            ctx.struct_set_body(payload_ty, &fields);
        }
        let body = self.enum_union_body(&info);
        ctx.struct_set_body(info.union_ty, &body);
        ctx.struct_set_body(info.llvm_ty, &[ctx.i32(), info.union_ty]);
        Ok(())
    }

    /// The union's field list: the alignment anchor, then a byte array filling
    /// out to the union's size. The byte count is exactly what makes the `enum`
    /// the size its widest payload rounds up to, so a value of the type is the
    /// same object the C typedef declares.
    fn enum_union_body(&self, info: &EnumInfo<'a>) -> Vec<Type<'a>> {
        let mut align = 1u32;
        let mut size = 0u64;
        for v in &info.variants {
            align = align.max(self.layout.abi_alignment_of(v.payload_ty));
            size = size.max(self.layout.abi_size_of(v.payload_ty));
        }
        let union_size = round_up(size, align);
        // An integer of exactly the union's alignment: LLVM gives `iN` the
        // alignment of the smallest power-of-two byte width that covers it, so
        // the anchor is what forces the union field to `align`.
        let anchor = self.ctx.int(align * 8);
        let anchor_size = self.layout.abi_size_of(anchor);
        let pad = union_size.saturating_sub(anchor_size);
        if pad == 0 {
            return vec![anchor];
        }
        vec![anchor, self.ctx.array_type(self.ctx.i8(), pad)]
    }

    /// Whether a value of `ty` owns memory in this program, which is exactly
    /// "a drop is owed for it" (`types::needs_drop`). The distinction matters at
    /// a staged temporary: an all-scalar `datac` may be staged for its address
    /// freely, while one that owns memory has to reach a drop site.
    fn owns_memory(&self, ty: &Ty) -> bool {
        needs_drop(ty, &self.owned)
    }

    /// Declare one drop function per `datac` or `enum` that owns memory
    /// (`types::needs_drop`), the mirror of the routine `codegen_c.rs` writes
    /// beside each typedef. The plan's targets call them by name.
    fn declare_drop_fns(&mut self) {
        let owned = owned_types(&self.program.structs, &self.program.enums);
        for s in &self.program.structs {
            if !needs_drop(&Ty::Struct(s.name.clone()), &owned) {
                continue;
            }
            let name = format!("kelp_drop_{}", crate::types::c_ident(&s.name));
            let ty = self
                .ctx
                .fn_type(self.ctx.void(), &[self.ctx.opaque_ptr()], false);
            let func = self.module.add_function(&name, ty);
            self.drop_fns.insert(s.name.clone(), (func, ty));
        }
        // An `enum` whose own name is in `owned` has at least one variant with
        // an owned payload field, so its drop has at least one case.
        for e in &self.program.enums {
            if !needs_drop(&Ty::Struct(e.name.clone()), &owned) {
                continue;
            }
            let name = format!("kelp_drop_{}", crate::types::c_ident(&e.name));
            let ty = self
                .ctx
                .fn_type(self.ctx.void(), &[self.ctx.opaque_ptr()], false);
            let func = self.module.add_function(&name, ty);
            self.drop_fns.insert(e.name.clone(), (func, ty));
        }
    }

    /// Define each generated drop: for a `datac`, one call per owned field in
    /// declaration order, on the field's own address; for an `enum`, one
    /// `switch` on the live tag, so only the live variant's owned payload fields
    /// are freed (MK7 §3.1, "one `switch` per drop"). A builtin buffer goes
    /// straight to the runtime's drop; a nested owned aggregate recurses through
    /// its own drop.
    fn emit_drop_fn_bodies(&self) -> Result<(), String> {
        let owned = owned_types(&self.program.structs, &self.program.enums);
        for s in &self.program.structs {
            let Some((func, _)) = self.drop_fns.get(&s.name) else {
                continue;
            };
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let struct_ty = *self
                .structs
                .get(&s.name)
                .ok_or_else(|| no_value(&format!("the datac '{}'", s.name)))?;
            let self_ptr = func.get_param(0);
            for (index, field) in s.fields.iter().enumerate() {
                if !needs_drop(&field.ty, &owned) {
                    continue;
                }
                let field_ptr =
                    self.builder
                        .build_struct_gep(struct_ty, self_ptr, index as u32, &field.name);
                self.call_drop(field_ptr, &field.ty)?;
            }
            self.builder.build_ret_void();
        }
        for e in &self.program.enums {
            if !self.drop_fns.contains_key(&e.name) {
                continue;
            }
            self.emit_enum_drop_body(e, &owned)?;
        }
        Ok(())
    }

    /// One `enum`'s drop: `switch (tag) { case v: <drop v's owned payload>
    /// ... }`, which is the C backend's version of the same function and the
    /// only place a tagged union frees anything.
    ///
    /// Only a variant with at least one owned payload field gets a case, exactly
    /// as `write_enum` skips the variants with nothing to free. The default block
    /// is the exit and there is no runtime tag check: every tag a program can
    /// produce is one of the variants, and the frontend's exhaustiveness is what
    /// says so.
    fn emit_enum_drop_body(&self, e: &crate::types::EnumDef, owned: &HashSet<String>) -> Result<(), String> {
        let (func, _) = *self
            .drop_fns
            .get(&e.name)
            .ok_or_else(|| no_value(&format!("the drop of '{}'", e.name)))?;
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let self_ptr = func.get_param(0);
        let tag_ptr = self
            .builder
            .build_struct_gep(info.llvm_ty, self_ptr, ENUM_TAG_FIELD, "tag");
        let tag = self.builder.build_load(self.ctx.i32(), tag_ptr, "tag");

        let done = func.append_block("drop.end");
        let cases: Vec<(usize, BasicBlock<'a>)> = e
            .variants
            .iter()
            .enumerate()
            .filter(|(_, v)| v.fields.iter().any(|f| needs_drop(&f.ty, owned)))
            .map(|(i, _)| (i, func.append_block(&format!("drop.v{i}"))))
            .collect();
        let switch = self.builder.build_switch(tag, done, cases.len() as u32);
        for (index, block) in &cases {
            let on = self.ctx.const_int(self.ctx.i32(), *index as u64, false);
            self.builder.add_case(switch, on, *block);
        }
        for (index, block) in &cases {
            self.builder.position_at_end(*block);
            let payload_ty = info.variants[*index].payload_ty;
            let union_ptr = self
                .builder
                .build_struct_gep(info.llvm_ty, self_ptr, ENUM_UNION_FIELD, "kelp_v");
            for (fi, field) in e.variants[*index].fields.iter().enumerate() {
                if !needs_drop(&field.ty, owned) {
                    continue;
                }
                let field_ptr =
                    self.builder
                        .build_struct_gep(payload_ty, union_ptr, fi as u32, &field.name);
                self.call_drop(field_ptr, &field.ty)?;
            }
            self.builder.build_br(done);
        }
        self.builder.position_at_end(done);
        self.builder.build_ret_void();
        Ok(())
    }

    /// Free the value at `ptr`: the runtime's builtin free, or the generated
    /// per-`datac` drop. A type that owns nothing has no call, which is the same
    /// silence `codegen_c.rs` keeps for a dropless field.
    fn call_drop(&self, ptr: Value<'a>, ty: &Ty) -> Result<(), String> {
        let (func, fty) = match ty {
            Ty::Bytes | Ty::String => {
                let symbol = drop_fn_name(ty);
                let rt = self
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?;
                (rt.func, rt.ty)
            }
            Ty::Struct(name) => match self.drop_fns.get(name) {
                Some((func, fty)) => (*func, *fty),
                // An unowned `datac` has no drop function and nothing to free.
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        self.builder.build_call(func, fty, &[ptr], "");
        Ok(())
    }

    /// Emit every function body. Borrows `&self`: after `declare`, the module
    /// state is read-only and the C API builder is the only thing that changes.
    fn emit_all(&self) -> Result<(), String> {
        let program = self.program;
        for f in &program.functions {
            if f.extern_ {
                continue;
            }
            FnGen::new(self, f)?.emit()?;
        }
        Ok(())
    }
}

/// Per-function state: the current block, the entry block, and the local slots.
struct FnGen<'g, 'a> {
    g: &'g Gen<'a>,
    cur_fn: LlvmFunction<'a>,
    entry: BasicBlock<'a>,
    cur: BasicBlock<'a>,
    /// Whether the current block already ends in a terminator, so the walk stops
    /// emitting into it. A `return` terminates; a block after one is dead code.
    terminated: bool,
    /// One map per lexical scope, innermost last, name to alloca slot. A slot is
    /// created in the entry block so `mem2reg` can promote it.
    scopes: Vec<HashMap<String, Value<'a>>>,
    fn_name: String,
    ret_ty: Ty,
    /// The function's `tir` body and parameter list, borrowed for `'a` from the
    /// program so the walk does not hold a borrow of `self`.
    body: &'a Block,
    params: Vec<(String, Ty)>,
}

impl<'g, 'a> FnGen<'g, 'a> {
    fn new(g: &'g Gen<'a>, f: &'a Function) -> Result<Self, String> {
        let info = g
            .funcs
            .get(&f.name)
            .cloned()
            .ok_or_else(|| no_value("a declared function"))?;
        let entry = info.func.append_block("entry");
        Ok(FnGen {
            g,
            cur_fn: info.func,
            entry,
            cur: entry,
            terminated: false,
            scopes: Vec::new(),
            fn_name: f.name.clone(),
            ret_ty: f.ret.clone(),
            body: &f.body,
            params: f.params.clone(),
        })
    }

    fn emit(mut self) -> Result<(), String> {
        self.g.builder.position_at_end(self.entry);
        self.scopes.push(HashMap::new());

        // Bind each parameter into its own slot, so its name is a load like any
        // other local.
        if self.fn_name != "main" {
            for (i, (name, ty)) in self.params.clone().iter().enumerate() {
                let value = self.cur_fn.get_param(i);
                let slot = self.alloca(ty, name)?;
                self.g.builder.build_store(value, slot);
                self.scopes
                    .last_mut()
                    .expect("the function scope is open")
                    .insert(name.clone(), slot);
            }
        }

        let body = self.body;
        self.emit_block_body(body)?;

        if !self.terminated {
            self.emit_fallthrough_return()?;
        }
        Ok(())
    }

    /// Emit a block's statements and then its tail: as a `return` when the tail
    /// belongs to a value-returning function, discarded otherwise, followed by
    /// the block's end-of-scope drops (the C backend draws both lines in
    /// `write_function_body`).
    fn emit_block_body(&mut self, block: &Block) -> Result<(), String> {
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            self.emit_stmt(stmt)?;
        }
        if self.terminated {
            return Ok(());
        }
        let drops = self.drops_at_block_end(block);
        if let Some(tail) = &block.tail {
            if self.ret_ty != Ty::Unit {
                // Read the returned value *before* freeing the frame's locals:
                // the tail may read through one of them.
                let value = if drops.is_empty() {
                    self.value_of(tail)?
                } else {
                    self.value_of_then_drops(tail, &drops)?
                };
                if !self.terminated {
                    self.g.builder.build_ret(value);
                    self.terminated = true;
                }
            } else {
                self.emit_expr(tail)?;
                if !self.terminated {
                    self.emit_drops(&drops)?;
                }
            }
            return Ok(());
        }
        self.emit_drops(&drops)
    }

    /// Emit a block as an expression: its tail is the block's value, `None` when
    /// the block is unit or already diverged. Used for nested blocks and for the
    /// two arms of an `if`.
    fn emit_block_value(&mut self, block: &Block) -> Result<Option<Value<'a>>, String> {
        self.scopes.push(HashMap::new());
        let result = self.emit_block_value_inner(block);
        self.scopes.pop();
        result
    }

    fn emit_block_value_inner(&mut self, block: &Block) -> Result<Option<Value<'a>>, String> {
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            self.emit_stmt(stmt)?;
        }
        if self.terminated {
            return Ok(None);
        }
        let drops = self.drops_at_block_end(block);
        match &block.tail {
            // A unit tail is a statement, not a value: it is emitted and the
            // block's own drops follow it, so a `{ ...; call() }` block in
            // statement position costs nothing extra.
            Some(tail) if tail.ty == Ty::Unit => {
                self.emit_expr(tail)?;
                if !self.terminated {
                    self.emit_drops(&drops)?;
                }
                Ok(None)
            }
            // A block's value has to be read before its own end-of-scope drops
            // run, so it passes through a temporary when anything is freed.
            Some(tail) if !drops.is_empty() => Ok(Some(self.value_of_then_drops(tail, &drops)?)),
            Some(tail) => self.emit_expr(tail),
            None => {
                self.emit_drops(&drops)?;
                Ok(None)
            }
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let { name, ty, init } => {
                let value = self.emit_expr(init)?;
                // An initializer that diverged leaves the binding dead: emitting
                // its store would put an instruction after the block's terminator.
                if self.terminated {
                    return Ok(());
                }
                let slot = self.alloca(ty, name)?;
                self.scopes
                    .last_mut()
                    .expect("a scope is always open")
                    .insert(name.clone(), slot);
                match value {
                    Some(v) => {
                        self.g.builder.build_store(v, slot);
                    }
                    None if *ty == Ty::Unit => {}
                    None => return Err(no_value(&format!("the initializer of '{name}'"))),
                }
            }
            Stmt::Return { value, span } => {
                let drops = self.drops_at_return(*span);
                match value {
                    Some(e) => {
                        let v = if drops.is_empty() {
                            self.value_of(e)?
                        } else {
                            self.value_of_then_drops(e, &drops)?
                        };
                        if !self.terminated {
                            self.g.builder.build_ret(v);
                        }
                    }
                    None => {
                        self.emit_drops(&drops)?;
                        self.emit_fallthrough_return()?;
                    }
                }
                self.terminated = true;
            }
            Stmt::While { cond, body } => self.emit_while(cond, body)?,
            Stmt::Expr(e) => {
                self.emit_expr(e)?;
            }
        }
        Ok(())
    }

    fn drops_at_block_end(&self, block: &Block) -> Vec<DropTarget> {
        self.g.drops.at_block_end(&self.fn_name, block).to_vec()
    }

    fn drops_at_return(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_return(&self.fn_name, span).to_vec()
    }

    fn drops_at_assign(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_assign(&self.fn_name, span).to_vec()
    }

    /// Owned locals freed by the `?` at `span`, on its failure arm only: the same
    /// set and the same order a written `return` frees (`DropPlan::at_try`).
    fn drops_at_try(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_try(&self.fn_name, span).to_vec()
    }

    /// Free the still-live owned locals the plan names, in the plan's order
    /// (reverse declaration order, so a later binding is freed before an earlier
    /// one it may have taken its value from).
    fn emit_drops(&mut self, targets: &[DropTarget]) -> Result<(), String> {
        for t in targets {
            let slot = self
                .lookup(&t.name)
                .ok_or_else(|| no_value(&format!("the owned local '{}'", t.name)))?;
            self.g.call_drop(slot, &t.ty)?;
        }
        Ok(())
    }

    /// Evaluate `e`, store it in a temporary, then run `drops`. A returned or
    /// block-tailed value may read through a local being freed, so it has to be
    /// read first (`codegen_c.rs::write_temp_then_drops`). An expression that
    /// diverged leaves nothing to store and no drop to run.
    fn value_of_then_drops(
        &mut self,
        e: &Expr,
        drops: &[DropTarget],
    ) -> Result<Value<'a>, String> {
        let value = self.value_of(e)?;
        if self.terminated {
            return Ok(value);
        }
        let lty = self.lty(&e.ty)?;
        let slot = self.temp_alloca(lty, "drop.tmp");
        self.g.builder.build_store(value, slot);
        self.emit_drops(drops)?;
        Ok(self.g.builder.build_load(lty, slot, "drop.val"))
    }

    fn emit_while(&mut self, cond: &Expr, body: &Block) -> Result<(), String> {
        let header = self.cur_fn.append_block("while.cond");
        let body_bb = self.cur_fn.append_block("while.body");
        let exit_bb = self.cur_fn.append_block("while.exit");
        self.g.builder.build_br(header);

        self.g.builder.position_at_end(header);
        self.cur = header;
        self.terminated = false;
        let c = self.value_of(cond)?;
        self.g.builder.build_cond_br(c, body_bb, exit_bb);

        self.g.builder.position_at_end(body_bb);
        self.cur = body_bb;
        self.terminated = false;
        self.emit_block_value(body)?;
        if !self.terminated {
            self.g.builder.build_br(header);
        }

        self.g.builder.position_at_end(exit_bb);
        self.cur = exit_bb;
        self.terminated = false;
        Ok(())
    }

    fn emit_expr(&mut self, e: &Expr) -> Result<Option<Value<'a>>, String> {
        match &e.kind {
            ExprKind::IntLit(v) => Ok(Some(self.int_const(&e.ty, *v)?)),
            ExprKind::CharLit(v) => Ok(Some(self.int_const(&e.ty, *v as i128)?)),
            ExprKind::FloatLit(v) => Ok(Some(self.float_const(&e.ty, *v)?)),
            ExprKind::BoolLit(b) => Ok(Some(self.g.ctx.const_bool(*b))),
            ExprKind::StringLit(s) => Ok(Some(self.string_global(s)?)),
            ExprKind::Ident(name) => Ok(Some(self.load_ident(e, name)?)),
            ExprKind::Unary { op, expr } => Ok(Some(self.emit_unary(*op, expr, &e.ty)?)),
            ExprKind::Binary { op, lhs, rhs } => {
                Ok(Some(self.emit_binary(*op, lhs, rhs)?))
            }
            ExprKind::Call { callee, args } => self.emit_call(callee, args),
            ExprKind::Assign { target, value } => {
                self.emit_assign(target, value, e.span)?;
                Ok(None)
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => self.emit_if(cond, then_block, else_block.as_ref(), &e.ty),
            ExprKind::Block(block) => self.emit_block_value(block),
            ExprKind::FieldAccess { base, field } => {
                Ok(Some(self.emit_field_read(base, field, &e.ty)?))
            }
            ExprKind::StructLit { name, fields } => Ok(Some(self.emit_struct_lit(name, fields)?)),
            // `Enum.Variant(payload = ...)`: the tag, then each payload field
            // into the variant's union member.
            ExprKind::EnumLit {
                enum_name,
                variant,
                fields,
            } => Ok(Some(self.emit_enum_lit(enum_name, *variant, fields)?)),
            // The tag test a `when`/`if let` arm lowers to. `expr` is a place
            // (`infer.rs::check_when_scrutinee` guarantees it), so this reads a
            // field of it rather than of a copy.
            ExprKind::EnumTag { expr, variant } => {
                Ok(Some(self.emit_enum_tag(expr, *variant)?))
            }
            // One payload field of one variant, read out of a value whose tag is
            // already known to be `variant`. It is a *place*, which is what lets
            // an owned payload be bound as a borrow rather than moved out.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => Ok(Some(self.emit_enum_field(base, *variant, *field, &e.ty)?)),
            // `expr?`: the operand once, then a branch on its failure tag. The
            // failure arm rebuilds the failure in the enclosing function's
            // return type, frees the frame (`DropPlan::at_try`) and returns; the
            // success arm is the operator's own value.
            ExprKind::Try {
                expr: sub,
                source,
                target,
                ok_variant,
                err_variant,
                target_err_variant,
                err_has_payload,
            } => self.emit_try(
                sub,
                source,
                target,
                *ok_variant,
                *err_variant,
                *target_err_variant,
                *err_has_payload,
                e.span,
                &e.ty,
            ),
            // A borrow (`infer.rs::check_call_arg` synthesizes one for the
            // `datac` parameter whose type resolves to a shared or exclusive
            // borrow). `&str` is the pointer itself, as the C backend writes it;
            // a `datac` or heap borrow is the address of the place it borrows,
            // which is the one indirection `getelementptr` needs and the pointer
            // the runtime's `const kelp_Bytes*` parameters take.
            ExprKind::Ref { expr: sub, .. } => {
                if is_str_ref(&e.ty) {
                    return Ok(Some(self.value_of(sub)?));
                }
                if is_struct_ref(&e.ty) || is_heap_ref(&e.ty) {
                    return Ok(Some(self.place_of(sub)?));
                }
                Err(refuse("'a borrow of a non-datac'", "S2B"))
            }
            // `*p` through a borrow of a `datac`: the pointee is the struct the
            // pointer addresses. A dereference of anything else is the borrow
            // slice.
            ExprKind::Deref { expr: sub } => {
                if !matches!(&e.ty, Ty::Struct(_)) {
                    return Err(refuse("'a dereference of a non-datac'", "S2B"));
                }
                let ptr = self.value_of(sub)?;
                let lty = self.lty(&e.ty)?;
                Ok(Some(self.g.builder.build_load(lty, ptr, "deref")))
            }
            ExprKind::Template { newline, pieces } => {
                self.emit_template(*newline, pieces)?;
                Ok(None)
            }
            other => Err(refuse_expr(other)),
        }
    }

    /// Read one `datac` field: the base's address, a `getelementptr` to the
    /// field's index, then a load of the field's own type.
    ///
    /// `ty` is the field expression's `tir` type, which is the pointee type the
    /// load needs and does not have to be recovered from the struct table.
    fn emit_field_read(
        &mut self,
        base: &Expr,
        field: &str,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let (ptr, struct_ty, index) = self.field_place(base, field)?;
        let field_ptr = self
            .g
            .builder
            .build_struct_gep(struct_ty, ptr, index, field);
        let lty = self.lty(ty)?;
        Ok(self.g.builder.build_load(lty, field_ptr, field))
    }

    /// Build a `datac` value: one `insertvalue` per field, in declaration order.
    ///
    /// `tir::ExprKind::StructLit` always lists every field, in declaration
    /// order (`infer.rs` zips the definition's fields with the checked
    /// arguments), so the index of each value is its position in the vector and
    /// the starting `undef` is fully overwritten.
    fn emit_struct_lit(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Result<Value<'a>, String> {
        let lty = *self
            .g
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let mut agg = self.g.ctx.undef(lty);
        for (index, (fname, fexpr)) in fields.iter().enumerate() {
            let value = self.value_of(fexpr)?;
            agg = self
                .g
                .builder
                .build_insert_value(agg, value, index as u32, fname);
        }
        Ok(agg)
    }

    /// The layout of the `enum` a (possibly borrowed) expression's type names.
    /// A clone, because the callers all need to walk a place mutably at the same
    /// time, and the layout is a handful of `Copy` handles.
    fn enum_info_of(&self, ty: &Ty) -> Result<EnumInfo<'a>, String> {
        let name = struct_name_of_ty(ty).ok_or_else(|| refuse_type(ty))?;
        self.g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))
    }

    /// The address of an `enum` place, plus its layout. `place_of` decides
    /// whether that address is the local's own slot or the pointer a `&Shape`
    /// binding holds, so a `move` parameter and a borrowed one take the same
    /// path here.
    fn enum_place(&mut self, base: &Expr) -> Result<(Value<'a>, EnumInfo<'a>), String> {
        let info = self.enum_info_of(&base.ty)?;
        let ptr = self.place_of(base)?;
        Ok((ptr, info))
    }

    /// `expr.kelp_tag == variant`: the tag test a `when`/`if let` arm lowers to.
    /// The comparison is on `i32` because that is the C typedef's tag width, and
    /// the variant index is the same compile-time constant both backends write.
    fn emit_enum_tag(&mut self, base: &Expr, variant: usize) -> Result<Value<'a>, String> {
        let (ptr, info) = self.enum_place(base)?;
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, ptr, ENUM_TAG_FIELD, "tag");
        let tag = self.g.builder.build_load(self.g.ctx.i32(), tag_ptr, "tag");
        let want = self.g.ctx.const_int(self.g.ctx.i32(), variant as u64, false);
        Ok(self
            .g
            .builder
            .build_icmp(IntCmp::Eq, tag, want, "isvar"))
    }

    /// The address of one payload field of one variant, inside the union. The
    /// union's own address is the enum's field 1; the address inside it is the
    /// *variant payload struct*'s field index, which is the C
    /// `kelp_v.kelp_v_<Variant>.<field>` path with the same offsets.
    fn enum_field_place(
        &mut self,
        base: &Expr,
        variant: usize,
        field: usize,
    ) -> Result<Value<'a>, String> {
        let (ptr, info) = self.enum_place(base)?;
        let payload_ty = info
            .variants
            .get(variant)
            .ok_or_else(|| no_value(&format!("the variant {variant} of an enum")))?
            .payload_ty;
        let union_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, ptr, ENUM_UNION_FIELD, "kelp_v");
        Ok(self
            .g
            .builder
            .build_struct_gep(payload_ty, union_ptr, field as u32, "field"))
    }

    /// Read one payload field of one variant as a value.
    fn emit_enum_field(
        &mut self,
        base: &Expr,
        variant: usize,
        field: usize,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let ptr = self.enum_field_place(base, variant, field)?;
        let lty = self.lty(ty)?;
        Ok(self.g.builder.build_load(lty, ptr, "payload"))
    }

    /// Build one `enum` value: a zeroinitializer, the tag, then each payload
    /// field stored into the variant's union member.
    ///
    /// A tagged union has no single LLVM value to build field by field, because
    /// the union field's type is not the variant's payload type; the address of
    /// the union is, so the literal is materialized in a temporary and loaded
    /// back. The zeroinitializer is what the C compound literal does for the
    /// members its designators do not mention, so an `enum` copied by value never
    /// carries an undefined byte.
    fn emit_enum_lit(
        &mut self,
        name: &str,
        variant: usize,
        fields: &[Expr],
    ) -> Result<Value<'a>, String> {
        let info = self
            .g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))?;
        let slot = self.temp_alloca(info.llvm_ty, "enum.tmp");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(info.llvm_ty), slot);
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.ctx.const_int(self.g.ctx.i32(), variant as u64, false);
        self.g.builder.build_store(tag, tag_ptr);
        if !fields.is_empty() {
            let payload_ty = info
                .variants
                .get(variant)
                .ok_or_else(|| no_value(&format!("the variant {variant} of '{name}'")))?
                .payload_ty;
            let union_ptr = self
                .g
                .builder
                .build_struct_gep(info.llvm_ty, slot, ENUM_UNION_FIELD, "kelp_v");
            for (index, field) in fields.iter().enumerate() {
                let value = self.value_of(field)?;
                let field_ptr = self.g.builder.build_struct_gep(
                    payload_ty,
                    union_ptr,
                    index as u32,
                    "field",
                );
                self.g.builder.build_store(value, field_ptr);
            }
        }
        Ok(self.g.builder.build_load(info.llvm_ty, slot, "enum"))
    }

    /// `expr?`: evaluate the operand once into a temporary, branch on its failure
    /// tag, and on the failure path rebuild the failure in the enclosing
    /// function's return type, free the frame, and return.
    ///
    /// This is the C backend's statement expression, as control flow: the two
    /// arms are two blocks and the operator's value is the success payload read
    /// out of the temporary. The temporary is deliberately never dropped: exactly
    /// one variant's payload is moved out of it on either path, and dropping what
    /// is left would free what was moved.
    #[allow(clippy::too_many_arguments)]
    fn emit_try(
        &mut self,
        sub: &Expr,
        source: &str,
        target: &str,
        ok_variant: usize,
        err_variant: usize,
        target_err_variant: usize,
        err_has_payload: bool,
        span: Span,
        ty: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let src = self
            .g
            .enums
            .get(source)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{source}'")))?;
        let dst = self
            .g
            .enums
            .get(target)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{target}'")))?;
        let src_ty = src.llvm_ty;
        let dst_ty = dst.llvm_ty;

        let value = self.value_of(sub)?;
        // An operand that diverged (`panic(...)?`) leaves the block terminated and
        // no tag to test; the failure path is already the only one.
        if self.terminated {
            return Ok(None);
        }
        let src_slot = self.temp_alloca(src_ty, "try.src");
        self.g.builder.build_store(value, src_slot);
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(src_ty, src_slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.builder.build_load(self.g.ctx.i32(), tag_ptr, "tag");
        let want = self.g.ctx.const_int(self.g.ctx.i32(), err_variant as u64, false);
        let failed = self.g.builder.build_icmp(IntCmp::Eq, tag, want, "iserr");

        let fail_bb = self.cur_fn.append_block("try.err");
        let ok_bb = self.cur_fn.append_block("try.ok");
        self.g.builder.build_cond_br(failed, fail_bb, ok_bb);

        // The failure arm: the failure value in the *enclosing function's* return
        // type, then the frame's drops, then the return. The payload is copied,
        // not moved, because the source temporary is never dropped either.
        self.g.builder.position_at_end(fail_bb);
        self.cur = fail_bb;
        self.terminated = false;
        let dst_slot = self.temp_alloca(dst_ty, "try.ret");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(dst_ty), dst_slot);
        let dst_tag_ptr = self
            .g
            .builder
            .build_struct_gep(dst_ty, dst_slot, ENUM_TAG_FIELD, "tag");
        let dst_tag = self
            .g
            .ctx
            .const_int(self.g.ctx.i32(), target_err_variant as u64, false);
        self.g.builder.build_store(dst_tag, dst_tag_ptr);
        if err_has_payload {
            let src_payload_ty = src.variants[err_variant].payload_ty;
            let src_union = self
                .g
                .builder
                .build_struct_gep(src_ty, src_slot, ENUM_UNION_FIELD, "kelp_v");
            let src_field = self
                .g
                .builder
                .build_struct_gep(src_payload_ty, src_union, 0, "payload");
            let payload_ty = self.lty(&src.variants[err_variant].fields[0])?;
            let payload = self.g.builder.build_load(payload_ty, src_field, "payload");
            let dst_payload_ty = dst.variants[target_err_variant].payload_ty;
            let dst_union = self
                .g
                .builder
                .build_struct_gep(dst_ty, dst_slot, ENUM_UNION_FIELD, "kelp_v");
            let dst_field = self
                .g
                .builder
                .build_struct_gep(dst_payload_ty, dst_union, 0, "payload");
            self.g.builder.build_store(payload, dst_field);
        }
        let drops = self.drops_at_try(span);
        self.emit_drops(&drops)?;
        let ret = self.g.builder.build_load(dst_ty, dst_slot, "try.ret");
        self.g.builder.build_ret(ret);
        self.terminated = true;

        // The success arm is where the operator's value lives.
        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        let ok_payload_ty = src.variants[ok_variant].payload_ty;
        let ok_union = self
            .g
            .builder
            .build_struct_gep(src_ty, src_slot, ENUM_UNION_FIELD, "kelp_v");
        let ok_field = self
            .g
            .builder
            .build_struct_gep(ok_payload_ty, ok_union, 0, "payload");
        let lty = self.lty(ty)?;
        Ok(Some(self.g.builder.build_load(lty, ok_field, "try.ok")))
    }

    /// The address of `base.field`, plus the LLVM struct type it indexes and the
    /// field's index: what a field read and a field write both need.
    fn field_place(
        &mut self,
        base: &Expr,
        field: &str,
    ) -> Result<(Value<'a>, Type<'a>, u32), String> {
        let struct_name = struct_name_of_ty(&base.ty)
            .ok_or_else(|| refuse_type(&base.ty))?
            .to_string();
        let index = self
            .g
            .field_index
            .get(&struct_name)
            .and_then(|names| names.iter().position(|n| n == field))
            .ok_or_else(|| no_value(&format!("the field '{struct_name}.{field}'")))?;
        let struct_ty = *self
            .g
            .structs
            .get(&struct_name)
            .ok_or_else(|| no_value(&format!("the datac '{struct_name}'")))?;
        let ptr = self.place_of(base)?;
        Ok((ptr, struct_ty, index as u32))
    }

    /// The address of a *place* expression: a local's slot, a field of another
    /// place, or the pointer a `datac` or heap borrow names.
    ///
    /// An rvalue base (`mk().field`, a literal's field) has no address, so it is
    /// stored into a temporary slot first, exactly as `codegen_c.rs` hoists one.
    /// The temporary is handed back to the caller, which is what frees it: this
    /// slice's `datac`s can own, so a staged temporary's drop is the caller's
    /// business, not silently dropped here.
    fn place_of(&mut self, e: &Expr) -> Result<Value<'a>, String> {
        match &e.kind {
            ExprKind::Ident(name) => {
                let slot = self
                    .lookup(name)
                    .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
                if is_struct_ref(&e.ty) || is_heap_ref(&e.ty) {
                    // The binding holds the borrow: its own value is the pointer.
                    Ok(self.g.builder.build_load(self.g.ctx.opaque_ptr(), slot, name))
                } else {
                    Ok(slot)
                }
            }
            ExprKind::FieldAccess { base, field } => {
                let (ptr, struct_ty, index) = self.field_place(base, field)?;
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, ptr, index, field))
            }
            // One payload field of one variant: the same union projection a
            // read takes, handed out as an address. This is what a pattern
            // binding of an owned payload borrows through.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => self.enum_field_place(base, *variant, *field),
            // A borrow in a place position is already an address.
            ExprKind::Ref { expr: sub, .. } => self.place_of(sub),
            ExprKind::Deref { expr: sub } => self.value_of(sub),
            _ => {
                // A staged temporary that owns memory has to be freed, and the
                // one drop site that can do it is a call argument (handled in
                // `arg_value`). Reaching one here means a shape whose owner no
                // site would free, so it is refused by name rather than
                // silently leaked.
                if self.g.owns_memory(&e.ty) {
                    return Err(refuse("'a field of an owned temporary'", "S2B"));
                }
                let value = self.value_of(e)?;
                let lty = self.lty(&e.ty)?;
                let slot = self.temp_alloca(lty, "field.base");
                self.g.builder.build_store(value, slot);
                Ok(slot)
            }
        }
    }

    /// An alloca for a temporary, created at the start of the entry block like
    /// a named local, so `mem2reg` can promote it and a loop body does not grow
    /// the stack once per iteration.
    fn temp_alloca(&self, ty: Type<'a>, name: &str) -> Value<'a> {
        self.g.builder.position_at_start(self.entry);
        let slot = self.g.builder.build_alloca(ty, name);
        self.g.builder.position_at_end(self.cur);
        slot
    }

    /// The LLVM type of a `tir` type, with this program's `datac` table.
    fn lty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        llvm_type(self.g.ctx, &self.g.structs, ty)
    }

    /// Evaluate an expression that must have a value, or report the internal
    /// error that it did not (a unit-typed expression in a value position).
    fn value_of(&mut self, e: &Expr) -> Result<Value<'a>, String> {
        self.emit_expr(e)?
            .ok_or_else(|| no_value("an expression that should produce a value"))
    }

    /// One call argument. An explicit borrow of an owned *temporary* is staged
    /// into a slot of its own and handed back in `held` for the caller to free
    /// after the call, which is what `codegen_c.rs::stage_argument` does; every
    /// other argument is its own value.
    fn arg_value(
        &mut self,
        a: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if let ExprKind::Ref { expr: sub, .. } = &a.kind {
            if !is_str_ref(&a.ty) {
                if let Some(root) = temporary_root(sub) {
                    if self.g.owns_memory(&root.ty) {
                        let root_ty = root.ty.clone();
                        let lty = self.lty(&root_ty)?;
                        let slot = self.temp_alloca(lty, "arg.tmp");
                        let value = self.value_of(root)?;
                        self.g.builder.build_store(value, slot);
                        let ptr = self.chain_addr(slot, sub)?;
                        held.push((slot, root_ty));
                        return Ok(ptr);
                    }
                }
            }
        }
        self.value_of(a)
    }

    /// The address of the field chain `e` read out of `root`, the staged
    /// temporary its base is rooted at (the C backend's `chain_off`).
    fn chain_addr(&mut self, root: Value<'a>, e: &Expr) -> Result<Value<'a>, String> {
        match &e.kind {
            ExprKind::FieldAccess { base, field } => {
                let base_ptr = self.chain_addr(root, base)?;
                let struct_name = struct_name_of_ty(&base.ty)
                    .ok_or_else(|| refuse_type(&base.ty))?
                    .to_string();
                let struct_ty = *self
                    .g
                    .structs
                    .get(&struct_name)
                    .ok_or_else(|| no_value(&format!("the datac '{struct_name}'")))?;
                let index = self
                    .g
                    .field_index
                    .get(&struct_name)
                    .and_then(|names| names.iter().position(|n| n == field))
                    .ok_or_else(|| no_value(&format!("the field '{struct_name}.{field}'")))?;
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, base_ptr, index as u32, field))
            }
            // A chain that steps through an enum payload: the union field, then
            // the variant's payload struct, then the field.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => {
                let base_ptr = self.chain_addr(root, base)?;
                let info = self.enum_info_of(&base.ty)?;
                let union_ptr = self.g.builder.build_struct_gep(
                    info.llvm_ty,
                    base_ptr,
                    ENUM_UNION_FIELD,
                    "kelp_v",
                );
                let payload_ty = info.variants[*variant].payload_ty;
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(payload_ty, union_ptr, *field as u32, "field"))
            }
            _ => Ok(root),
        }
    }

    fn int_const(&self, ty: &Ty, value: i128) -> Result<Value<'a>, String> {
        let lty = self.lty(ty)?;
        // Cut the `i128` literal to the expression's own type. `LLVMConstInt`
        // takes the low 64 bits and truncates to the type's width, which is the
        // same narrowing `codegen_c.rs` spells as a C cast; the sign flag is
        // irrelevant for a value already in two's complement.
        Ok(self.g.ctx.const_int(lty, value as u64, false))
    }

    fn float_const(&self, ty: &Ty, value: f64) -> Result<Value<'a>, String> {
        if !ty.is_float() {
            return Err(refuse(&format!("a float literal of type '{}'", ty.display()), "S1"));
        }
        Ok(self.g.ctx.const_real(self.lty(ty)?, value))
    }

    fn string_global(&self, text: &str) -> Result<Value<'a>, String> {
        // A global string is NUL-terminated, so an interior NUL would truncate
        // silently. S1 refuses it by name rather than miscompiling it.
        if text.as_bytes().contains(&0) {
            return Err(refuse("'a string literal containing NUL'", "S2"));
        }
        Ok(self.g.builder.global_string(text, "str"))
    }

    /// Load a binding. A load at a move site also clears the source slot: the
    /// value is copied out and the binding is zeroed, so a drop the binding
    /// still owes on a path that did not move it frees nothing rather than a
    /// second copy (MK7 §3.4; `codegen_c.rs` writes `({0})` for the same
    /// reason). A binding moved on every path gets no `at_move` entry, so this
    /// leaves its slot alone and nothing drops it.
    fn load_ident(&mut self, e: &Expr, name: &str) -> Result<Value<'a>, String> {
        let slot = self
            .lookup(name)
            .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
        let lty = self.lty(&e.ty)?;
        let value = self.g.builder.build_load(lty, slot, name);
        if self.g.owns_memory(&e.ty) && self.g.drops.moves_at(&self.fn_name, e) {
            self.g.builder.build_store(self.g.ctx.const_null(lty), slot);
        }
        Ok(value)
    }

    fn lookup(&self, name: &str) -> Option<Value<'a>> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(*v);
            }
        }
        None
    }

    /// Create a local's slot in the entry block and restore the insertion point.
    ///
    /// `mem2reg` promotes only slots in the entry block, and by the time a `let`
    /// in a later statement allocates one the entry block may already have been
    /// terminated by a `return`, so the slot goes at the block's *start*.
    fn alloca(&self, ty: &Ty, name: &str) -> Result<Value<'a>, String> {
        let lty = self.lty(ty)?;
        self.g.builder.position_at_start(self.entry);
        let slot = self.g.builder.build_alloca(lty, name);
        self.g.builder.position_at_end(self.cur);
        Ok(slot)
    }

    fn emit_assign(&mut self, target: &Expr, value: &Expr, span: Span) -> Result<(), String> {
        let v = self.value_of(value)?;
        if self.terminated {
            return Ok(());
        }
        match &target.kind {
            ExprKind::Ident(name) => {
                let slot = self
                    .lookup(name)
                    .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
                // The old value goes after the new one has been read (it may
                // read through the binding being overwritten) and before it is
                // stored (`DropPlan::at_assign`; `b = b` is not one of these).
                let drops = self.drops_at_assign(span);
                self.emit_drops(&drops)?;
                self.g.builder.build_store(v, slot);
                Ok(())
            }
            // A field write: the field's address, then a store of the new value.
            // The plan records `at_assign` only for an `Ident` target, so a
            // field assignment frees nothing first, the same as the C backend.
            ExprKind::FieldAccess { base, field } => {
                let (ptr, struct_ty, index) = self.field_place(base, field)?;
                let field_ptr = self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, ptr, index, field);
                self.g.builder.build_store(v, field_ptr);
                Ok(())
            }
            _ => Err(refuse("'assignment to a non-local place'", "S2B")),
        }
    }

    fn emit_unary(&mut self, op: UnOp, sub: &Expr, ty: &Ty) -> Result<Value<'a>, String> {
        let v = self.value_of(sub)?;
        Ok(match op {
            UnOp::Neg => {
                if ty.is_float() {
                    self.g.builder.build_fneg(v, "neg")
                } else {
                    let zero = self.zero_const(ty)?;
                    self.g
                        .builder
                        .build_int_binop(IntBinOp::Sub, zero, v, "neg")
                }
            }
            UnOp::Not => {
                if *ty != Ty::Bool {
                    return Err(refuse("'logical not on a non-bool'", "S1"));
                }
                // `i1` xor `true` is logical negation of a boolean.
                let one = self.g.ctx.const_bool(true);
                self.g.builder.build_int_binop(IntBinOp::Xor, v, one, "not")
            }
            UnOp::BitNot => {
                let all = self.all_ones(ty)?;
                self.g
                    .builder
                    .build_int_binop(IntBinOp::Xor, v, all, "bitnot")
            }
        })
    }

    fn emit_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Result<Value<'a>, String> {
        // `&&`/`||` short-circuit, so they are control flow, not a bitwise op.
        match op {
            BinOp::And => return self.emit_logical(lhs, rhs, true),
            BinOp::Or => return self.emit_logical(lhs, rhs, false),
            _ => {}
        }
        let l = self.value_of(lhs)?;
        let r = self.value_of(rhs)?;
        let ot = &lhs.ty;
        let float = ot.is_float();
        let signed = is_signed(ot);
        let b = &self.g.builder;
        Ok(match op {
            BinOp::Add => {
                if float {
                    b.build_float_binop(FloatBinOp::Add, l, r, "fadd")
                } else {
                    b.build_int_binop(IntBinOp::Add, l, r, "add")
                }
            }
            BinOp::Sub => {
                if float {
                    b.build_float_binop(FloatBinOp::Sub, l, r, "fsub")
                } else {
                    b.build_int_binop(IntBinOp::Sub, l, r, "sub")
                }
            }
            BinOp::Mul => {
                if float {
                    b.build_float_binop(FloatBinOp::Mul, l, r, "fmul")
                } else {
                    b.build_int_binop(IntBinOp::Mul, l, r, "mul")
                }
            }
            BinOp::Div => {
                if float {
                    b.build_float_binop(FloatBinOp::Div, l, r, "fdiv")
                } else if signed {
                    b.build_int_binop(IntBinOp::SDiv, l, r, "sdiv")
                } else {
                    b.build_int_binop(IntBinOp::UDiv, l, r, "udiv")
                }
            }
            BinOp::Mod => {
                if float {
                    return Err(refuse("'the float remainder operator'", "S1"));
                } else if signed {
                    b.build_int_binop(IntBinOp::SRem, l, r, "srem")
                } else {
                    b.build_int_binop(IntBinOp::URem, l, r, "urem")
                }
            }
            BinOp::Eq | BinOp::NotEq => {
                let eq = op == BinOp::Eq;
                if float {
                    b.build_fcmp(if eq { RealCmp::Oeq } else { RealCmp::One }, l, r, "fcmp")
                } else {
                    b.build_icmp(if eq { IntCmp::Eq } else { IntCmp::Ne }, l, r, "icmp")
                }
            }
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                if float {
                    let cmp = match op {
                        BinOp::Lt => RealCmp::Olt,
                        BinOp::LtEq => RealCmp::Ole,
                        BinOp::Gt => RealCmp::Ogt,
                        _ => RealCmp::Oge,
                    };
                    b.build_fcmp(cmp, l, r, "fcmp")
                } else {
                    let cmp = match (op, signed) {
                        (BinOp::Lt, true) => IntCmp::Slt,
                        (BinOp::LtEq, true) => IntCmp::Sle,
                        (BinOp::Gt, true) => IntCmp::Sgt,
                        (BinOp::GtEq, true) => IntCmp::Sge,
                        (BinOp::Lt, false) => IntCmp::Ult,
                        (BinOp::LtEq, false) => IntCmp::Ule,
                        (BinOp::Gt, false) => IntCmp::Ugt,
                        _ => IntCmp::Uge,
                    };
                    b.build_icmp(cmp, l, r, "icmp")
                }
            }
            BinOp::BitAnd => b.build_int_binop(IntBinOp::And, l, r, "and"),
            BinOp::BitOr => b.build_int_binop(IntBinOp::Or, l, r, "or"),
            BinOp::BitXor => b.build_int_binop(IntBinOp::Xor, l, r, "xor"),
            BinOp::Shl => b.build_int_binop(IntBinOp::Shl, l, r, "shl"),
            BinOp::Shr => {
                if signed {
                    b.build_int_binop(IntBinOp::AShr, l, r, "ashr")
                } else {
                    b.build_int_binop(IntBinOp::LShr, l, r, "lshr")
                }
            }
            BinOp::And | BinOp::Or => unreachable!("handled above as control flow"),
        })
    }

    /// `a && b` / `a || b` as two blocks and a phi, so the right operand is
    /// evaluated only when the left did not decide, matching C's `&&`/`||`.
    fn emit_logical(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        is_and: bool,
    ) -> Result<Value<'a>, String> {
        let l = self.value_of(lhs)?;
        let rhs_bb = self.cur_fn.append_block("logic.rhs");
        let short_bb = self.cur_fn.append_block("logic.short");
        let merge_bb = self.cur_fn.append_block("logic.end");
        if is_and {
            self.g.builder.build_cond_br(l, rhs_bb, short_bb);
        } else {
            self.g.builder.build_cond_br(l, short_bb, rhs_bb);
        }

        self.g.builder.position_at_end(rhs_bb);
        self.cur = rhs_bb;
        self.terminated = false;
        let r = self.value_of(rhs)?;
        let rhs_end = self.cur;
        self.g.builder.build_br(merge_bb);

        self.g.builder.position_at_end(short_bb);
        self.cur = short_bb;
        self.terminated = false;
        self.g.builder.build_br(merge_bb);

        self.g.builder.position_at_end(merge_bb);
        self.cur = merge_bb;
        self.terminated = false;
        let phi = self.g.builder.build_phi(self.g.ctx.i1(), "logic");
        let short = self.g.ctx.const_bool(!is_and);
        self.g.builder.add_incoming(phi, &[(r, rhs_end), (short, short_bb)]);
        Ok(phi)
    }

    fn emit_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        ty: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let c = self.value_of(cond)?;
        let then_bb = self.cur_fn.append_block("if.then");
        let else_bb = self.cur_fn.append_block("if.else");
        let merge_bb = self.cur_fn.append_block("if.end");
        self.g.builder.build_cond_br(c, then_bb, else_bb);

        self.g.builder.position_at_end(then_bb);
        self.cur = then_bb;
        self.terminated = false;
        let then_val = self.emit_block_value(then_block)?;
        let then_end = self.cur;
        let then_reaches = !self.terminated;
        if then_reaches {
            self.g.builder.build_br(merge_bb);
        }

        self.g.builder.position_at_end(else_bb);
        self.cur = else_bb;
        self.terminated = false;
        let else_val = match else_block {
            Some(b) => self.emit_block_value(b)?,
            None => None,
        };
        let else_end = self.cur;
        let else_reaches = !self.terminated;
        if else_reaches {
            self.g.builder.build_br(merge_bb);
        }

        self.g.builder.position_at_end(merge_bb);
        self.cur = merge_bb;
        self.terminated = false;

        // A unit-typed `if` has no value to join, and neither arm may have
        // reached the merge. An `if` with no reaching arm and no value is dead
        // code after a divergence: terminate the merge.
        if *ty == Ty::Unit {
            if !then_reaches && !else_reaches {
                self.g.builder.build_unreachable();
                self.terminated = true;
            }
            return Ok(None);
        }

        let mut incoming: Vec<(Value<'a>, BasicBlock<'a>)> = Vec::new();
        if then_reaches {
            let v = match then_val {
                Some(v) => v,
                None => self.zero_const(ty)?,
            };
            incoming.push((v, then_end));
        }
        if else_reaches {
            let v = match else_val {
                Some(v) => v,
                None => self.zero_const(ty)?,
            };
            incoming.push((v, else_end));
        }
        if incoming.is_empty() {
            // Both arms diverge, so this value is never used; the merge is dead.
            // The phi must not be built first: one with no incoming does not
            // verify.
            self.g.builder.build_unreachable();
            self.terminated = true;
            return Ok(Some(self.zero_const(ty)?));
        }
        let lty = self.lty(ty)?;
        let phi = self.g.builder.build_phi(lty, "if");
        self.g.builder.add_incoming(phi, &incoming);
        Ok(Some(phi))
    }

    fn emit_call(&mut self, callee: &str, args: &[Expr]) -> Result<Option<Value<'a>>, String> {
        // `print`/`print!` reach libc `printf`, as the C backend does.
        if callee == "print" || callee == "print!" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the argument of print"))?;
            self.emit_template_print(callee == "print", arg)?;
            return Ok(None);
        }

        // `panic` reaches the runtime's `kelp_panic(const char*)`. The message
        // is a `str`/`&str` (already the pointer) or an owned `String` (the
        // buffer's `data`); the call never returns, so the block ends here and
        // no later statement of this block may be emitted.
        if callee == "panic" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the argument of panic"))?;
            let msg = self.heap_data_ptr(arg)?;
            self.g
                .builder
                .build_call(self.g.panic_fn, self.g.panic_ty, &[msg], "");
            self.g.builder.build_unreachable();
            self.terminated = true;
            return Ok(None);
        }

        // `assert` reaches the runtime's `kelp_assert(bool)`, which panics with
        // the same "assertion failed" text the C backend gets.
        if callee == "assert" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the condition of assert"))?;
            let cond = self.value_of(arg)?;
            let rt = *self
                .g
                .runtime
                .get("kelp_assert")
                .ok_or_else(|| no_value("the runtime 'kelp_assert'"))?;
            self.g.builder.build_call(rt.func, rt.ty, &[cond], "");
            return Ok(None);
        }

        // `n.to_u8()` and friends: the key is `"<src>#to_<dst>"`, the same shape
        // `codegen_c.rs::call_value` matches.
        if let Some((src, dst)) = callee.split_once("#to_") {
            if let (Some(from), Some(to)) = (Ty::from_name(src), Ty::from_name(dst)) {
                if from.is_integer() && (to.is_integer() || to.is_float()) {
                    let arg = args
                        .first()
                        .ok_or_else(|| no_value("the argument of a conversion"))?;
                    let v = self.emit_conversion(callee, &from, &to, arg)?;
                    return Ok(Some(v));
                }
            }
        }

        if let Some(info) = self.g.funcs.get(callee).cloned() {
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for a in args {
                values.push(self.arg_value(a, &mut held)?);
            }
            // A call that yields `void` must not be given a name: an instruction
            // with a name has to have a value, and the verifier rejects a named
            // void one. A unit method (a `mut` receiver that writes a field and
            // returns nothing) is exactly that shape.
            let yields_value = info.ret_ty != Ty::Unit;
            let name = if yields_value { "call" } else { "" };
            let v = self.g.builder.build_call(info.func, info.ty, &values, name);
            // An owned temporary staged for the argument is freed *after* the
            // call, as `codegen_c.rs::hoisted_call` frees it.
            for (slot, ty) in held {
                self.g.call_drop(slot, &ty)?;
            }
            if !yields_value {
                return Ok(None);
            }
            return Ok(Some(v));
        }

        // A builtin method on `Bytes`/`String`/`View`: reached by declaration
        // and never re-implemented here, with the machine-level signature
        // `kelp_rt.c` is compiled with. A helper that returns a struct through
        // the hidden pointer takes the destination as its explicit first
        // argument, which is the same call clang would emit.
        if let Some(rt) = self.g.runtime.get(&symbol_name(callee)).copied() {
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            if rt.sret {
                let ret_ty = rt
                    .ret
                    .ok_or_else(|| no_value("the return type of an sret helper"))?;
                let slot = self.temp_alloca(ret_ty, "sret.out");
                let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len() + 1);
                values.push(slot);
                for a in args {
                    values.push(self.arg_value(a, &mut held)?);
                }
                self.g.builder.build_call(rt.func, rt.ty, &values, "");
                for (slot, ty) in held {
                    self.g.call_drop(slot, &ty)?;
                }
                return Ok(Some(self.g.builder.build_load(ret_ty, slot, "sret.val")));
            }
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for a in args {
                values.push(self.arg_value(a, &mut held)?);
            }
            let name = if rt.yields { "call" } else { "" };
            let v = self.g.builder.build_call(rt.func, rt.ty, &values, name);
            for (slot, ty) in held {
                self.g.call_drop(slot, &ty)?;
            }
            return Ok(if rt.yields { Some(v) } else { None });
        }

        // Anything else is outside this slice. An `extern fun` is the runtime
        // and is S3; a container method is the container slice; any other name
        // is a construct this stage has no lowering for.
        let stage = if self.g.program.functions.iter().any(|f| f.name == callee && f.extern_) {
            "S3"
        } else if ["List#", "Map#", "Box#", "Array#"]
            .iter()
            .any(|prefix| callee.starts_with(prefix))
        {
            "S2D"
        } else {
            "S2B"
        };
        Err(refuse(&format!("the call to '{callee}'"), stage))
    }

    fn emit_conversion(
        &mut self,
        callee: &str,
        from: &Ty,
        to: &Ty,
        arg: &Expr,
    ) -> Result<Value<'a>, String> {
        let v = self.value_of(arg)?;
        let to_ty = self.lty(to)?;

        // Integer to float: `int_bounds` has no bounds for a float target, so
        // `numeric_conversion` treats it as a plain cast, and so does this.
        if to.is_float() {
            return Ok(self
                .g
                .builder
                .build_int_to_fp(is_signed(from), v, to_ty, "to_f"));
        }

        let (Some((s_min, s_max)), Some((t_min, t_max))) =
            (Ty::int_bounds(from), Ty::int_bounds(to))
        else {
            return Err(no_value("an integer-to-integer conversion"));
        };

        // A widening (or a same-width re-interpretation whose source range fits
        // the target) is a plain cast.
        if s_min >= t_min && s_max <= t_max {
            return Ok(self.cast_int(from, to, v)?);
        }

        let from_ty = self.lty(from)?;
        let signed = is_signed(from);
        let mut checks: Vec<Value<'a>> = Vec::new();
        // Only the bounds the source's own range cannot rule out, as
        // `numeric_conversion` does: a check that cannot fail is not emitted.
        if s_min < t_min {
            let bound = self.g.ctx.const_int(from_ty, t_min as u64, false);
            checks.push(self.g.builder.build_icmp(
                if signed { IntCmp::Slt } else { IntCmp::Ult },
                v,
                bound,
                "lo",
            ));
        }
        if s_max > t_max {
            let bound = self.g.ctx.const_int(from_ty, t_max as u64, false);
            checks.push(self.g.builder.build_icmp(
                if signed { IntCmp::Sgt } else { IntCmp::Ugt },
                v,
                bound,
                "hi",
            ));
        }
        let cond = checks
            .into_iter()
            .reduce(|a, b| self.g.builder.build_int_binop(IntBinOp::Or, a, b, "oor"))
            .expect("at least one bound is missing when the ranges do not fit");

        let panic_bb = self.cur_fn.append_block("range.panic");
        let ok_bb = self.cur_fn.append_block("range.ok");
        self.g.builder.build_cond_br(cond, panic_bb, ok_bb);

        // The failing arm panics with the `.kp` spelling of the call, the same
        // text `numeric_conversion` passes to `kelp_panic`.
        self.g.builder.position_at_end(panic_bb);
        self.cur = panic_bb;
        self.terminated = false;
        let msg = format!("{}: value out of range", callee.replace('#', "."));
        let msg_ptr = self.string_global(&msg)?;
        self.g
            .builder
            .build_call(self.g.panic_fn, self.g.panic_ty, &[msg_ptr], "");
        self.g.builder.build_unreachable();

        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        self.cast_int(from, to, v)
    }

    /// Cast an integer value to `to`, with no range check: the caller has already
    /// decided the value fits. A same-width conversion is no instruction.
    fn cast_int(&self, from: &Ty, to: &Ty, v: Value<'a>) -> Result<Value<'a>, String> {
        let fw = int_width(from).ok_or_else(|| no_value("an integer source type"))?;
        let tw = int_width(to).ok_or_else(|| no_value("an integer target type"))?;
        if fw == tw {
            return Ok(v);
        }
        let to_ty = self.lty(to)?;
        Ok(self
            .g
            .builder
            .build_int_cast(is_signed(from), tw > fw, v, to_ty, "cast"))
    }

    /// The `print` path for one value: `printf` with the specifier and promotion
    /// `codegen_c.rs::printf_piece_with` uses, so stdout is byte-identical. An
    /// owned temporary is freed right after the `printf`, exactly where the C
    /// backend frees it.
    fn emit_template_print(&mut self, newline: bool, arg: &Expr) -> Result<(), String> {
        let (spec, mut values, hold) = self.printf_piece(arg)?;
        let fmt = if newline {
            format!("{spec}\n")
        } else {
            spec
        };
        let fmt_ptr = self.string_global(&fmt)?;
        let mut all = Vec::with_capacity(values.len() + 1);
        all.push(fmt_ptr);
        all.append(&mut values);
        self.g
            .builder
            .build_call(self.g.printf_fn, self.g.printf_ty, &all, "");
        if let Some((slot, ty)) = hold {
            self.g.call_drop(slot, &ty)?;
        }
        Ok(())
    }

    /// A template: one `printf` whose format string is the literal chunks (with
    /// `%` doubled) plus each interpolated value's specifier. The C backend
    /// builds the same format string in `codegen_c.rs`'s `Template` arm; an
    /// interpolated owned temporary is freed after the `printf`, as `print`'s is.
    fn emit_template(
        &mut self,
        newline: bool,
        pieces: &[TemplatePiece],
    ) -> Result<(), String> {
        let mut fmt = String::new();
        let mut args: Vec<Value<'a>> = Vec::new();
        let mut holds: Vec<(Value<'a>, Ty)> = Vec::new();
        for piece in pieces {
            match piece {
                TemplatePiece::Text(text) => {
                    for c in text.chars() {
                        if c == '%' {
                            fmt.push_str("%%");
                        } else {
                            fmt.push(c);
                        }
                    }
                }
                TemplatePiece::Value(value) => {
                    let (spec, mut values, hold) = self.printf_piece(value)?;
                    fmt.push_str(&spec);
                    args.append(&mut values);
                    if let Some((slot, ty)) = hold {
                        holds.push((slot, ty));
                    }
                }
            }
        }
        if newline {
            fmt.push('\n');
        }
        let fmt_ptr = self.string_global(&fmt)?;
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(fmt_ptr);
        all.extend(args);
        self.g
            .builder
            .build_call(self.g.printf_fn, self.g.printf_ty, &all, "");
        for (slot, ty) in holds {
            self.g.call_drop(slot, &ty)?;
        }
        Ok(())
    }

    /// The `data` pointer of a value `panic` reads as C text: a `str`/`&str` is
    /// already the pointer, an owned `String` (or a borrow of one) contributes
    /// the first field of its buffer. This is the IR twin of the C backend's
    /// `panic` argument staging, minus the rows it does not reach.
    fn heap_data_ptr(&mut self, arg: &Expr) -> Result<Value<'a>, String> {
        match &arg.ty {
            Ty::Str => self.value_of(arg),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => self.value_of(arg),
            Ty::String => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "panic.msg");
                self.g.builder.build_store(v, slot);
                let (data, _) = self.buffer_pair(slot, lty);
                Ok(data)
            }
            Ty::Ref { inner, .. } if matches!(**inner, Ty::String) => {
                let ptr = self.place_of(arg)?;
                let lty = self.lty(inner)?;
                let (data, _) = self.buffer_pair(ptr, lty);
                Ok(data)
            }
            other => Err(refuse_type(other)),
        }
    }

    /// The `(data, len)` pair of a heap value or view, read out of the struct at
    /// `ptr`. Field 0 is the pointer and field 1 the length in all three builtin
    /// layouts, which is what lets one routine serve `Bytes`, `String` and
    /// `View`.
    fn buffer_pair(&self, ptr: Value<'a>, struct_ty: Type<'a>) -> (Value<'a>, Value<'a>) {
        let data_ptr = self.g.builder.build_struct_gep(struct_ty, ptr, 0, "data");
        let data = self
            .g
            .builder
            .build_load(self.g.ctx.opaque_ptr(), data_ptr, "data");
        let len_ptr = self.g.builder.build_struct_gep(struct_ty, ptr, 1, "len");
        let len = self
            .g
            .builder
            .build_load(self.g.ctx.i64(), len_ptr, "len");
        (data, len)
    }

    /// If `arg` is a heap buffer or a view, its `(data, len)` pair and the
    /// temporary the caller must free afterwards. `None` for anything else, which
    /// then takes the scalar path.
    fn buffer_fields(
        &mut self,
        arg: &Expr,
    ) -> Result<Option<(Value<'a>, Value<'a>, Option<(Value<'a>, Ty)>)>, String> {
        match &arg.ty {
            Ty::Bytes | Ty::String => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "buf.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, Some((slot, arg.ty.clone())))))
            }
            Ty::View => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "view.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, None)))
            }
            Ty::Ref { inner, .. }
                if matches!(**inner, Ty::Bytes | Ty::String | Ty::View) =>
            {
                let ptr = self.place_of(arg)?;
                let lty = self.lty(inner)?;
                let (data, len) = self.buffer_pair(ptr, lty);
                Ok(Some((data, len, None)))
            }
            _ => Ok(None),
        }
    }

    /// The format specifier and the already-promoted variadic arguments for one
    /// value, mirroring `codegen_c.rs::printf_piece_with` exactly. A heap buffer
    /// or a view contributes `%.*s` with an `int` length and a pointer, so the
    /// specifier comes back with two arguments; the third slot carries a
    /// temporary the caller frees after the `printf`.
    fn printf_piece(
        &mut self,
        arg: &Expr,
    ) -> Result<(String, Vec<Value<'a>>, Option<(Value<'a>, Ty)>), String> {
        if let Some((data, len, hold)) = self.buffer_fields(arg)? {
            let len = self
                .g
                .builder
                .build_int_cast(false, false, len, self.g.ctx.i32(), "len");
            return Ok(("%.*s".to_string(), vec![len, data], hold));
        }
        let v = self.value_of(arg)?;
        let spec: &'static str = match &arg.ty {
            Ty::I32 => "%d",
            Ty::I64 => "%lld",
            Ty::U32 => "%u",
            Ty::U64 => "%llu",
            // A vararg narrower than `int` must be promoted here: LLVM does not,
            // unlike C, whose default argument promotions do it for free.
            Ty::U8 | Ty::U16 => {
                let promoted = self
                    .g
                    .builder
                    .build_int_cast(false, true, v, self.g.ctx.i32(), "promote");
                return Ok(("%u".to_string(), vec![promoted], None));
            }
            Ty::Usize => "%zu",
            Ty::Isize => "%lld",
            // `%f` reads a `double` out of the vector file, so an `f32` is
            // promoted, exactly as C's default argument promotions do.
            Ty::F32 => {
                let promoted = self
                    .g
                    .builder
                    .build_fp_cast(true, v, self.g.ctx.f64(), "promote");
                return Ok(("%f".to_string(), vec![promoted], None));
            }
            Ty::F64 => "%f",
            Ty::Bool => {
                let t = self.string_global("true")?;
                let f = self.string_global("false")?;
                let selected = self.g.builder.build_select(v, t, f, "bool");
                return Ok(("%s".to_string(), vec![selected], None));
            }
            Ty::Char => "%c",
            Ty::Str => "%s",
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "%s",
            other => return Err(refuse_type(other)),
        };
        Ok((spec.to_string(), vec![v], None))
    }

    fn all_ones(&self, ty: &Ty) -> Result<Value<'a>, String> {
        let bits = int_width(ty).ok_or_else(|| refuse_type(ty))?;
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        Ok(self.g.ctx.const_int(self.lty(ty)?, mask, false))
    }

    fn zero_const(&self, ty: &Ty) -> Result<Value<'a>, String> {
        if ty.is_float() {
            Ok(self.g.ctx.const_real(self.lty(ty)?, 0.0))
        } else if *ty == Ty::Bool {
            Ok(self.g.ctx.const_bool(false))
        } else if ty.is_integer() || *ty == Ty::Char {
            Ok(self.g.ctx.const_int(self.lty(ty)?, 0, false))
        } else if matches!(ty, Ty::Struct(_)) {
            // Only reached for a value the frontend already guarantees is
            // present on every path that reads it, so `undef` is never observed.
            Ok(self.g.ctx.undef(self.lty(ty)?))
        } else {
            Err(refuse_type(ty))
        }
    }

    /// The terminator for a function that reached the end of its body without a
    /// `return`: `main` returns 0 (mirroring the C backend's rewrite of a bare
    /// `return;` in `main`), a unit function returns nothing, and a value-returning
    /// function (which `infer.rs` does not let fall through) returns zero.
    fn emit_fallthrough_return(&mut self) -> Result<(), String> {
        if self.fn_name == "main" {
            self.g
                .builder
                .build_ret(self.g.ctx.const_int(self.g.ctx.i32(), 0, false));
        } else if self.ret_ty == Ty::Unit {
            self.g.builder.build_ret_void();
        } else {
            let zero = self.zero_const(&self.ret_ty)?;
            self.g.builder.build_ret(zero);
        }
        self.terminated = true;
        Ok(())
    }
}

/// Declare every runtime helper this slice calls, with the signature
/// `kelp_rt.c` defines and the ABI shape it is compiled under (MK7 §3.3). The
/// object `build.rs` produces is compiled from that same text, so a signature
/// that drifts is caught by neither the verifier (which knows nothing of the
/// object) nor `cc` at build time: it is caught only when a fixture runs. The
/// list is therefore exactly the surface the S2B fixtures reach: `int` arguments
/// are `i32`, `size_t` and `uint64_t` are `i64`, `uint8_t` is `i8`, `bool` is
/// `i1`; a `View` (two eightbytes) travels by value in registers both ways;
/// and a struct larger than that is returned through the hidden pointer this
/// module models as an explicit first argument (see [`RtFn::sret`]).
///
/// `kelp_String__into_bytes`/`kelp_String__from_bytes` are deliberately absent:
/// each takes a 24-byte struct *by value*, and LLVM's backend passes such an
/// argument in registers while the SysV ABI puts it on the stack, so a call to
/// one would not cross correctly. Leaving them out refuses the call by name
/// rather than miscompiling it.
fn declare_runtime<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
    bytes: Type<'a>,
    string: Type<'a>,
    view: Type<'a>,
) -> HashMap<String, RtFn<'a>> {
    let ptr = ctx.opaque_ptr();
    let i1 = ctx.i1();
    let i8 = ctx.i8();
    let i64 = ctx.i64();
    let void = ctx.void();
    let mut out: HashMap<String, RtFn<'a>> = HashMap::new();
    let decl = |out: &mut HashMap<String, RtFn<'a>>,
                name: &str,
                ret: Type<'a>,
                params: &[Type<'a>],
                yields: bool,
                sret: bool| {
        let (llvm_ret, mut all) = if sret {
            (void, vec![ptr])
        } else {
            (ret, Vec::new())
        };
        all.extend_from_slice(params);
        let ty = ctx.fn_type(llvm_ret, &all, false);
        let func = module.add_function(name, ty);
        out.insert(
            name.to_string(),
            RtFn {
                func,
                ty,
                yields,
                ret: if yields { Some(ret) } else { None },
                sret,
            },
        );
    };
    // The panic path and the assertion: `kelp_panic` never returns, but the
    // module still terminates the block it is called from with `unreachable`.
    decl(&mut out, "kelp_panic", void, &[ptr], false, false);
    decl(&mut out, "kelp_assert", void, &[i1], false, false);
    // `kelp_Bytes`
    decl(&mut out, "kelp_Bytes__empty", bytes, &[], true, true);
    decl(&mut out, "kelp_Bytes__from_str", bytes, &[ptr], true, true);
    decl(&mut out, "kelp_Bytes__clone", bytes, &[ptr], true, true);
    decl(&mut out, "kelp_Bytes__len", i64, &[ptr], true, false);
    decl(&mut out, "kelp_Bytes__is_empty", i1, &[ptr], true, false);
    decl(&mut out, "kelp_Bytes__get", i8, &[ptr, i64], true, false);
    decl(&mut out, "kelp_Bytes__set", void, &[ptr, i64, i8], false, false);
    decl(&mut out, "kelp_Bytes__push", void, &[ptr, i8], false, false);
    decl(&mut out, "kelp_Bytes__append", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_Bytes__reserve", void, &[ptr, i64], false, false);
    decl(&mut out, "kelp_Bytes__clear", void, &[ptr], false, false);
    decl(&mut out, "kelp_Bytes__as_str", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_bytes_drop", void, &[ptr], false, false);
    // `kelp_String`
    decl(&mut out, "kelp_String__empty", string, &[], true, true);
    decl(&mut out, "kelp_String__from_str", string, &[ptr], true, true);
    decl(&mut out, "kelp_String__clone", string, &[ptr], true, true);
    decl(&mut out, "kelp_String__with_capacity", string, &[i64], true, true);
    decl(&mut out, "kelp_String__len", i64, &[ptr], true, false);
    decl(&mut out, "kelp_String__char_count", i64, &[ptr], true, false);
    decl(&mut out, "kelp_String__is_empty", i1, &[ptr], true, false);
    decl(&mut out, "kelp_String__as_str", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_String__view", view, &[ptr], true, false);
    decl(&mut out, "kelp_String__push", void, &[ptr, i8], false, false);
    decl(&mut out, "kelp_String__push_str", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_String__reserve", void, &[ptr, i64], false, false);
    decl(&mut out, "kelp_String__clear", void, &[ptr], false, false);
    decl(&mut out, "kelp_string_drop", void, &[ptr], false, false);
    // `kelp_View`: the two-word struct is passed and returned by value, which is
    // the ABI's `INTEGER, INTEGER` e-classes on this target, so it crosses
    // unchanged.
    decl(&mut out, "kelp_str__view", view, &[ptr], true, false);
    decl(&mut out, "kelp_View__len", i64, &[view], true, false);
    decl(&mut out, "kelp_View__is_empty", i1, &[view], true, false);
    decl(&mut out, "kelp_View__get", i8, &[view, i64], true, false);
    decl(&mut out, "kelp_View__slice", view, &[view, i64, i64], true, false);
    decl(&mut out, "kelp_View__eq", i1, &[view, view], true, false);
    decl(&mut out, "kelp_View__to_string", string, &[view], true, true);
    out
}

/// Map a `tir` type to its LLVM type. The scalar subset, the builtin heap types,
/// `str`, and a `datac` `structs` holds a type for.
fn llvm_type<'a>(
    ctx: &'a Context,
    structs: &HashMap<String, Type<'a>>,
    ty: &Ty,
) -> Result<Type<'a>, String> {
    Ok(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => ctx.i32(),
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => ctx.i64(),
        Ty::U8 => ctx.i8(),
        Ty::U16 => ctx.i16(),
        Ty::F32 => ctx.f32(),
        Ty::F64 => ctx.f64(),
        Ty::Bool => ctx.i1(),
        // `str` is a bare `const char*`, which is the pointer LLVM 21 uses for
        // every pointer. S1 needs it so `print("...")` has a string to hand
        // `%s`; it is not otherwise part of the scalar subset. A borrow of `str`
        // is the same pointer (there is no second indirection), so `&str` maps
        // here too.
        Ty::Str => ctx.opaque_ptr(),
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => ctx.opaque_ptr(),
        // The builtin heap types, in the same field order `runtime/kelp_rt.c`
        // defines them (MK7 §3.3). `View` is the borrowed pair.
        Ty::Bytes => *structs
            .get("Bytes")
            .ok_or_else(|| no_value("the builtin 'Bytes' layout"))?,
        Ty::String => *structs
            .get("String")
            .ok_or_else(|| no_value("the builtin 'String' layout"))?,
        Ty::View => *structs
            .get("View")
            .ok_or_else(|| no_value("the builtin 'View' layout"))?,
        // A borrow of a heap type is its address, which is the pointer every
        // runtime helper taking `const kelp_Bytes*` wants.
        Ty::Ref { inner, .. }
            if matches!(**inner, Ty::Bytes | Ty::String | Ty::View) =>
        {
            ctx.opaque_ptr()
        }
        // A `datac` parameter resolves to a borrow of the struct, and a borrow is
        // its address (MK7 §3.3: pass a pointer, not a value, between frames).
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Struct(_)) => ctx.opaque_ptr(),
        // A borrow of anything but a `datac` (a `mut i64` parameter, say) is the
        // borrow slice, and it is named rather than mis-lowered to a pointer.
        Ty::Ref { .. } => return Err(refuse("'a borrow of a non-datac'", "S2B")),
        Ty::Struct(name) => *structs
            .get(name)
            .ok_or_else(|| refuse(&format!("the datac '{name}'"), "S2A"))?,
        other => return Err(refuse_type(other)),
    })
}

/// The return type of a function: `void` for unit, otherwise the type itself.
fn llvm_ret_type<'a>(
    ctx: &'a Context,
    structs: &HashMap<String, Type<'a>>,
    ty: &Ty,
) -> Result<Type<'a>, String> {
    if *ty == Ty::Unit {
        Ok(ctx.void())
    } else {
        llvm_type(ctx, structs, ty)
    }
}

/// The name of the `datac` a (possibly borrowed) type names, if any.
fn struct_name_of_ty(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name) => Some(name),
        Ty::Ref { inner, .. } => struct_name_of_ty(inner),
        _ => None,
    }
}

/// Whether a type is a borrow of a `datac`, the one `Ref` shape this slice
/// lowers, because `infer.rs::implicit_borrow_wrap` gives every `datac`
/// parameter that type.
fn is_struct_ref(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Struct(_)))
}

/// Whether a type is a borrow of `str` (or `str` itself), which is the pointer
/// rather than a second indirection.
fn is_str_ref(ty: &Ty) -> bool {
    match ty {
        Ty::Str => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Str),
        _ => false,
    }
}

/// Whether a type is a borrow of one of the builtin heap types. A binding of
/// such a type holds the pointer itself, exactly as a `datac` borrow does, so a
/// place read has to load it rather than hand out the slot.
fn is_heap_ref(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Bytes | Ty::String | Ty::View))
}

/// Whether an expression has stable storage, mirroring the subset of
/// `codegen_c.rs::is_place` this slice lowers.
fn is_place_expr(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Ident(_)
            | ExprKind::FieldAccess { .. }
            | ExprKind::EnumField { .. }
            | ExprKind::Deref { .. }
    )
}

/// The rvalue a field chain hangs off, when the chain's own address is not
/// storage already: the C backend's `temporary_root`, minus the rows this slice
/// does not lower. `None` for a place, `Some(root)` for a chain rooted at an
/// rvalue. An `enum` payload field is part of a chain exactly as a `datac` field
/// is (`codegen_c.rs` walks the two the same way).
fn temporary_root(expr: &Expr) -> Option<&Expr> {
    let mut cur = expr;
    loop {
        match &cur.kind {
            ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => {
                // A chain that steps through a borrow lives in the pointee, so
                // nothing has to be staged for it.
                if matches!(base.ty, Ty::Ref { .. }) {
                    return None;
                }
                cur = base;
            }
            _ => break,
        }
    }
    if is_place_expr(cur) { None } else { Some(cur) }
}

/// The identified LLVM type name of one `enum`'s union, and of one variant's
/// payload struct. Named after the tag `struct_tag` gives the enum, so the IR
/// names the same type the C spelling does and two enums cannot collide.
fn enum_union_name(name: &str) -> String {
    format!("{}.kelp_v", struct_tag(name))
}

fn enum_payload_name(name: &str, variant: usize) -> String {
    format!("{}.kelp_v_{variant}", struct_tag(name))
}

/// `size` rounded up to a multiple of `align`, which is a power of two. The
/// union's size rule, the same one C's own layout uses.
fn round_up(size: u64, align: u32) -> u64 {
    let align = align as u64;
    size.div_ceil(align) * align
}

/// A symbol name for a LLVM function, mirroring `codegen_c.rs::c_name` without
/// reaching into that (private) helper: `main` keeps its name because the C
/// runtime requires it, everything else gets the `kelp_` prefix and the same
/// sanitizing mangler, so the name is a valid symbol and cannot collide with a
/// libc one.
fn symbol_name(name: &str) -> String {
    if name == "main" {
        "main".to_string()
    } else {
        format!("kelp_{}", crate::types::c_ident(name))
    }
}

/// Whether an integer type is signed. The signedness is what picks `sdiv` over
/// `udiv`, `ashr` over `lshr` and `icmp slt` over `ult`; `char` is unsigned
/// because it is a code point (`uint32_t`), and `bool` is neither (it never
/// reaches a division).
fn is_signed(ty: &Ty) -> bool {
    matches!(ty, Ty::I32 | Ty::I64 | Ty::Isize)
}

/// The bit width of an integer-like type, or `None` for anything else.
fn int_width(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => 32,
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => 64,
        Ty::U8 => 8,
        Ty::U16 => 16,
        Ty::Bool => 1,
        _ => return None,
    })
}

/// The refusal message for a construct this stage does not lower. `what` is the
/// construct's name, quoted when it is a language word (`'enum'`) and a phrase
/// when it is a shape; `stage` is the MK7 stage that will lower it: this slice
/// lowers plain records, owned fields, the drop plan, the runtime and the tagged
/// union, S2D is the containers and arrays, S2E is closures and function values,
/// and S3 is the stdlib's `extern fun` bodies.
fn refuse(what: &str, stage: &str) -> String {
    format!("the llvm backend cannot lower {what} yet (MK7 {stage}); use --backend=c")
}

/// Refuse a `datac` field or an `enum` payload field this slice cannot lay out,
/// with the construct named.
///
/// A `String`/`Bytes` field, a field of an owned aggregate and a field that
/// names a `datac` or an `enum` are laid out (MK7 §3.1, §3.4); everything else
/// is named by its own construct. A container or array field is the container
/// slice (S2D), and the `str`/`View` rows are unreachable from the frontend
/// (`types::is_borrowed_field_ty` rejects them) but are refused here rather than
/// mis-lowered if they ever appear.
fn refuse_field(ty: &Ty) -> Result<(), String> {
    match ty {
        Ty::Bytes | Ty::String => Ok(()),
        Ty::List(_) => Err(refuse("'a List field'", "S2D")),
        Ty::Box(_) => Err(refuse("'a Box field'", "S2D")),
        Ty::Map(..) => Err(refuse("'a Map field'", "S2D")),
        Ty::Array(..) => Err(refuse("'an array field'", "S2D")),
        Ty::Str => Err(refuse("'a 'str' field'", "S2D")),
        Ty::View => Err(refuse("'a 'View' field'", "S2D")),
        Ty::Ref { .. } => Err(refuse("'a reference field'", "S2B")),
        Ty::Fn { .. } | Ty::Closure { .. } => Err(refuse("'a function-valued field'", "S2E")),
        Ty::Struct(_) => Ok(()),
        other if llvm_scalar_type(other) => Ok(()),
        other => Err(refuse_type(other)),
    }
}

/// Whether a `tir` type is one of the scalars the S1 lowering already emits. A
/// type that is not a scalar and not an aggregate is refused by name in
/// `refuse_field`, so this only has to recognize the scalars.
fn llvm_scalar_type(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::I32
            | Ty::I64
            | Ty::U32
            | Ty::U64
            | Ty::Usize
            | Ty::Isize
            | Ty::U8
            | Ty::U16
            | Ty::F32
            | Ty::F64
            | Ty::Bool
            | Ty::Char
    )
}

/// Name the type in a refusal, with the stage that will lower it. A container
/// is the container slice (S2D) and a function value the closure slice (S2E);
/// the heap types are lowered above, so their rows are unreachable and kept only
/// so the match stays total in the author's eye.
fn refuse_type(ty: &Ty) -> String {
    let (what, stage) = match ty {
        Ty::Struct(_) => ("'datac'", "S2B"),
        Ty::Bytes => ("'Bytes'", "S2B"),
        Ty::String => ("'String'", "S2B"),
        Ty::List(_) => ("'List'", "S2D"),
        Ty::Box(_) => ("'Box'", "S2D"),
        Ty::Map(..) => ("'Map'", "S2D"),
        Ty::Array(..) => ("'an array'", "S2D"),
        Ty::Ref { .. } => ("'a borrow of a non-datac'", "S2B"),
        Ty::Fn { .. } => ("'a function value'", "S2E"),
        Ty::Closure { .. } => ("'a closure'", "S2E"),
        Ty::Str | Ty::View => ("a borrowed view", "S2D"),
        Ty::Unit => ("'unit'", "S1"),
        other => return refuse(&format!("the type '{}'", other.display()), "S1"),
    };
    refuse(what, stage)
}

/// Name the expression in a refusal. A `FieldAccess`, a `StructLit`, an `enum`
/// construct and a `?` never reach here: all are lowered. An array literal is
/// S2D, and a function value, a closure and an indirect call are S2E.
fn refuse_expr(kind: &ExprKind) -> String {
    let (what, stage) = match kind {
        ExprKind::FnRef(_) | ExprKind::FnToClosure { .. } => ("'a function value'", "S2E"),
        ExprKind::ClosureLit { .. } => ("'a closure'", "S2E"),
        ExprKind::Capture { .. } => ("'a capture'", "S2E"),
        ExprKind::IndirectCall { .. } => ("'a call through a function value'", "S2E"),
        ExprKind::Ref { .. } => ("'a borrow of a non-datac'", "S2B"),
        ExprKind::Deref { .. } => ("'a dereference of a non-datac'", "S2B"),
        ExprKind::ArrayLit(_) | ExprKind::ArrayRepeat { .. } => ("'an array'", "S2D"),
        _ => ("'this expression'", "S2B"),
    };
    refuse(what, stage)
}

/// The internal-error text for a value position the lowering expected to hold a
/// value and did not. It is an `Err`, not a panic: even an internal mistake
/// leaves the user with a `--backend=c` line rather than a backtrace.
fn no_value(what: &str) -> String {
    format!("internal error: the llvm backend has no value for {what} (MK7 S1)")
}

#[cfg(test)]
mod layout_tests {
    use crate::llvm_ffi::{Context, Module};

    /// The IR's builtin heap layouts must match the C structs in
    /// `runtime/kelp_rt.c` field for field (MK7 §3.3): a pointer, a length and a
    /// capacity in that order for `Bytes` and `String`, and a pointer and a
    /// length for `View`. The verifier cannot see the object, so a reordered or
    /// re-widened field would only be caught when a fixture printed the wrong
    /// bytes; this pins the agreement where it is made.
    #[test]
    fn the_builtin_heap_layouts_match_the_c_runtime() {
        let ctx = Context::new();
        let module = Module::new(&ctx, "layout");
        let ptr = ctx.opaque_ptr();
        let bytes = ctx.struct_type("kelp_Bytes");
        ctx.struct_set_body(bytes, &[ptr, ctx.i64(), ctx.i64()]);
        let string = ctx.struct_type("kelp_String");
        ctx.struct_set_body(string, &[ptr, ctx.i64(), ctx.i64()]);
        let view = ctx.struct_type("kelp_View");
        ctx.struct_set_body(view, &[ptr, ctx.i64()]);
        // A named type is only printed once something references it, so one
        // declaration each is what puts the bodies into the module text.
        module.add_function("probe_bytes", ctx.fn_type(bytes, &[ptr], false));
        module.add_function("probe_string", ctx.fn_type(string, &[ptr], false));
        module.add_function("probe_view", ctx.fn_type(view, &[ptr], false));
        let ir = module.to_ir_string();
        assert!(
            ir.contains("%kelp_Bytes = type { ptr, i64, i64 }"),
            "unexpected Bytes layout:\n{ir}"
        );
        assert!(
            ir.contains("%kelp_String = type { ptr, i64, i64 }"),
            "unexpected String layout:\n{ir}"
        );
        assert!(
            ir.contains("%kelp_View = type { ptr, i64 }"),
            "unexpected View layout:\n{ir}"
        );
    }
}

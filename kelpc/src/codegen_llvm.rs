//! The LLVM backend: lower `tir` to LLVM IR. It is the only code generator.
//!
//! MK7 §3.1 names this module as the whole of the new work: the front half of the
//! compiler is unchanged and already backend-complete, so the lowering walks the
//! same typed IR the earlier emitter walks and emits a different target. This is the
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
//! the earlier emitter's typedef has (MK7 §3.1). Covered:
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
//! - the runtime object, built from the `kelp-rt` crate and linked into every
//!   LLVM binary, with the helpers reached by declaration, never re-implemented in IR
//! - `String` and `Bytes` as an identified `{ ptr, i64, i64 }` and `View` as
//!   `{ ptr, i64 }`, field for field the structs `kelp-rt` defines (MK7 §3.3)
//! - the builtin `Bytes`/`String`/`View` methods, and `print`/templates reading
//!   a buffer or a view as `%.*s` from its own length and pointer
//! - an owned `datac` field, the generated per-`datac` drop, and the
//!   `borrowck::DropPlan` emitted at the same points the earlier emitter emitted it:
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
//! An `enum` lowers to the `{ tag, union }` shape the enum emitter
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
//! match), so no runtime check is emitted here, exactly as the earlier emitter emits
//! no `default:` arm.
//!
//! # What S2D adds (MK7 S2, the container slice)
//!
//! `List<T>`, `Map<K, V>`, `Box<T>` and `[T; N]`. MK7 §3.2 names the one
//! exception to "the runtime stays C": the monomorphized container code, which
//! the earlier emitter generates per element or value type and which therefore cannot
//! live in a fixed C file. So it is generated into the module, as IR, one
//! function set per instantiation, with the same function names, the same bodies
//! and the same panics the earlier emitter writes:
//!
//! - `List<T>` is `{ data, len, cap }`; `reserve` doubles through `realloc`, and
//!   `get`/`set` run the same bounds check (`List.get: index out of bounds`). The
//!   S1 methods (`sort`, `reverse`, `contains`, `slice`, `clone`) are emitted
//!   with the same element gates `types::value_is_sortable` and its siblings
//!   decide, so a `datac` element gets `sort` but never `contains`.
//! - `Box<T>` is one pointer, `empty`/`of`/`get`/`set`/`take`, and `take` moves
//!   the value out, which is what makes a recursive `datac` expressible.
//! - `Map<K, V>` keeps its entries dense in insertion order and a separate
//!   open-addressed index of `entry index + 1` in a `u32` (`0` empty,
//!   `UINT32_MAX` tombstone), so `keys()` returns insertion order. Its key
//!   helpers are the runtime's own (`kelp_map_hash`, `kelp_map_mix`,
//!   `kelp_map_key_eq`), reached by declaration rather than re-derived.
//! - `[T; N]` is inline, not a pointer: `len()` is the type's constant,
//!   `get`/`set` are the same bounds check with the array's panic text, and the
//!   repeat literal goes through a generated `fill` loop rather than N
//!   initializers.
//!
//! A container header is an aggregate over 16 bytes, so it never crosses to the
//! runtime by value: every helper the module declares takes a pointer
//! (BUG-vN4tR, MK7 §3.3), and `the_container_layouts_match_the_c_typedefs` pins
//! the field-for-field agreement with `write_container_typedef`.
//!
//! The refusals the earlier slices left under a stale label are corrected here:
//! a borrow of a non-`datac` and a dereference of one are `S2F` (the reference
//! slice the stage-2 table reserves), not `S2B`.
//!
//! # What S2F adds (MK7 S2, the reference slice)
//!
//! Every borrow that is not a `datac`, a container, a heap type or `str`. A
//! `Ty::Ref` is one pointer whatever the pointee (MK7 §3.1), so the pointee's
//! own type decides the load and nothing else:
//!
//! - `&place` is the place's address: a scalar local's frame slot, a field of
//!   one (a `getelementptr` into the same struct the datac slice already
//!   projects), or the pointer a container method hands back. A binding of a
//!   borrow type holds that pointer, so a place read loads it.
//! - `*p` is a load of the pointee type, whatever that type is: a scalar, a
//!   `datac`, `str` or a container. `*p = v` is a store through the same
//!   pointer, and `(*p).field = v` is the datac slice's field store one
//!   indirection in.
//! - a `&T` parameter is a `ptr` in the signature and a borrow return is a
//!   `ptr` result, so `infer.rs::implicit_borrow_wrap`'s `mut i64` parameter
//!   and `syntax.md` 10.2's borrowed return are the same one-pointer transport
//! - the panic tail (BUG-b2Ldp): a body whose tail is a `panic` diverges, so it
//!   is emitted as a statement that ends the block with `unreachable` rather
//!   than asked for a value it does not have. the function body emitter
//!   draws the same line, and without it the backend reported its own
//!   "internal error" for `move_param_return` and `panic_tail`.
//!
//! `str` and `View` are pointer-plus-length and are untouched: a `&str` is the
//! pointer itself, and a borrow of a heap type stays the address the runtime
//! helpers take.
//!
//! # What S2E adds (MK7 S2, the closure slice)
//!
//! Function values and closures, mirrored from the earlier emitter: a declared `fun`
//! named as a value, a `fun` literal (with and without captures), a call through
//! a value, and a closure or a function value as a local, a parameter, a return
//! value, a `datac` field and a `Box` element.
//!
//! - a `Ty::Fn` is a bare `ptr`: the address of the declared function, which is
//!   what `FnRef` yields (`LLVM` already treats a function as a pointer value).
//!   It is `Copy`, owns nothing and has no drop.
//! - a `Ty::Closure` is the earlier emitter's fat pointer, one identified struct per
//!   **signature** (`{ ptr fn, ptr env, ptr drop }`), exactly the three members
//!   `write_closure_typedef` declares. `fn` takes the environment as its hidden
//!   first parameter, so the call site is `fn(env, args...)`; the members are
//!   all `ptr` here because LLVM 21 pointers are opaque, while the C typedef
//!   types them and so had to monomorphize the same way.
//! - a capture is an environment read: `Capture { index }` is a
//!   `getelementptr` into the literal's environment struct (named and laid out
//!   by `declare_closure_envs`), and it is a *place*, so an owned capture is
//!   borrowed rather than moved out. `ClosureLit` mallocs that struct, stores
//!   each capture in field order and builds the fat pointer (MK7 §3.3: the
//!   environment is heap, so its pointer is spelled out).
//! - the widening of a plain function value where a closure is expected keeps
//!   the earlier emitter's two shapes: a *known* function (`FnRef`) gets a generated
//!   shim with a `null` environment and allocates nothing, while a value whose
//!   identity only the call knows gets a one-field heap environment and a
//!   thunk that loads the code pointer back out of it.
//! - the per-signature closure drop is the one function in a closure's set, as
//!   it is in the earlier emitter: `if (env != null) { drop(env); env = null; }`.
//!
//! Where an aggregate crosses to C the pointer stays spelled out (BUG-vN4tR),
//! but a closure never crosses: its environment is Kelp-only and its drop is
//! called from IR. What the runtime does see is `malloc`/`free` for the
//! environment and the widened value's, the same two libc calls the generated C
//! makes.
//!
//! # What S3A adds (MK7 S3, the `extern fun` bodies)
//!
//! The compiler-supplied bodies of `extern fun` declarations (`externs.rs`),
//! which are C and stay C (MK7 §3.2): the raw allocator, the byte reads behind
//! `str`, `malloc`-shaped `String`/`Bytes` construction, `sqrt`, `clock_gettime`
//! and the rest of the unportable primitives the stdlib is a Kelp wrapper
//! around.
//!
//! - every `extern fun` the program carries is **declared** with the exact
//!   signature its body has, not the written one: an owned parameter is a
//!   borrow (so it is a pointer), and a `Bytes`/`String` return is 24 bytes, so
//!   it comes back through the hidden first argument clang compiles the body's
//!   `return` into (`declare_extern`, BUG-vN4tR).
//! - a call to one is lowered as a call through the same table the runtime
//!   helpers use: the declaration and the call are the same IR shape whether the
//!   callee's text is in the runtime crate or in the program's own object.
//! - a user `extern fun` has no body in the program: its trampoline is written
//!   per program as LLVM IR in `<out>.externs.o` (`extern_object.rs`), together
//!   with the callback glue of its `move closure(...)` parameters and the
//!   capability allowlist arrays. It is per program because it names the native
//!   symbols the program allowlisted, and no fixed object could carry those.
//!
//! # What S3 adds (MK7 S3, `pd` globals and the capability check)
//!
//! The rest of the runtime slice, and the security-relevant half of it.
//!
//! - a `pd` is a module-scope global with internal linkage, a zero
//!   initializer, and the passdown's own mangled `pd#<name>` identifier
//!   (`declare_pd_globals`). `kelp_init_pds` runs the initializers in
//!   declaration order, so a chained one (`pd val doubled = base * 2`) reads
//!   what an earlier one wrote, and `main` calls it first, exactly where the C
//!   backend inserts `kelp_init_pds();`. An owned passdown is freed by the
//!   generated `kelp_drop_pds`, which the initializer registers with `atexit`:
//!   a `pd` has no scope to die at, so process exit is its only drop point.
//!   A function that received no hidden parameter names the global directly,
//!   which is what `main` and another `pd`'s initializer are.
//! - a gated call site is guarded by the same runtime allowlist check the C
//!   backend emits. `cap_plan` mirrors the capability plan site for
//!   site: the same domain, the same resource arguments, the same pattern
//!   count, the same `.kp` call site. Every checked argument is evaluated
//!   **once** into a value the check and the call share (BUG-Kf7Rn), and the
//!   check is one `kelp_cap_check_path`/`_name`/`_host`/`_hostport`/`_port`
//!   call against the module's compiled-in array.
//! - that array is data in the per-program externs object, and the check
//!   functions are `kelp-rt`'s own (`kelp-rt/src/cap.rs`), so the deny path's
//!   text is written once. The matcher and the path normalizer stay Kelp
//!   (`std/rt`) and are lowered like any other code.
//!
//! The whole fixture corpus now lowers: the `llvm` backend is the only backend,
//! and it builds every one of the 223 corpus targets the earlier emitter built before
//! its removal, with an empty refusal ranking (MK7 §4.1). The differential
//! fuzzer (`kelpc/tests/differential_fuzz.rs`, MK7 S5) covers the surface the
//! curated corpus does not, and found the two shapes the sweep's fixtures were
//! missing.
//!
//! What remains in this file is **refused by name** with a clean diagnostic,
//! never miscompiled and never a panic, for a shape the frontend does not
//! currently produce, and a refusal names the construct and says the shortfall
//! is the backend's. A
//! wrong lowering is worse than a refusal, which is why the guards stay.
//!
//! # The rules this lowering keeps
//!
//! These are the places a second backend goes wrong, and MK7 §3.4 is the plan's
//! own statement of them:
//!
//! - A narrow `IntLit` (an `i128` in `tir`) is cut to the *expression's* type
//!   before the operation, exactly as the earlier emitter casts its literal text, so a
//!   `u64`-wide literal in an `i32` context is truncated first.
//! - Signedness comes from `Ty`: `sdiv`/`udiv`, `srem`/`urem`, `ashr`/`lshr`,
//!   `icmp slt` vs `ult`. `Shr` on a signed type is arithmetic.
//! - No `nsw`/`nuw` on an arithmetic instruction, no `inbounds` on a
//!   data-dependent GEP, and no fast-math flag is ever emitted. The plain forms
//!   wrap by definition, which is what the earlier emitter's `-fwrapv` promised; the C
//!   frontend flag has no counterpart here because there is nothing to turn off.
//!   The one `inbounds`/`nuw` in the IR is the struct-field GEP, where the index
//!   is a compile-time constant inside an object of that exact type, so the
//!   bounds are proven rather than asserted (see `llvm_ffi::Builder::build_struct_gep`).
//! - Division does not invent a zero check. The only guard the language has is
//!   constant folding in the frontend, and this backend mirrors that: a `sdiv` by
//!   zero is UB in LLVM exactly as it is in C.
//! - `print` and templates call libc `printf`, with the same format string and the
//!   same C ABI widths the printf piece rule uses. A vararg narrower
//!   than `int` is promoted here, because LLVM, unlike C, does not promote it.
//! - The numeric conversions mirror the numeric conversion rule: a widening
//!   is a plain cast, a narrowing that can lose a value is a range check that
//!   panics with the call's own `.kp` spelling.
//!
//! # What S4 adds (MK7 S4, the debug locations)
//!
//! DWARF, so that a fault in an `llvm`-built binary names a `.kp` file and line.
//! MK7 §4.4 states this as an exit criterion rather than a nice-to-have: a crash
//! report that points into generated IR instead of a Kelp source line is the
//! difference between actionable and not, and the earlier emitter buys the same
//! resolution with one `#line` per statement (the line-marker emitter).
//!
//! - one compile unit for the module, named in `llvm.dbg.cu` and marked by the
//!   `Debug Info Version` module flag: the two halves are what make the object
//!   carry the sections at all, and a compile unit alone reaches the object as
//!   nothing
//! - one `DIFile` per `.kp` source, and one `DISubprogram` per `.kp`-declared
//!   function, whose own line is the line its declaration opens on (`tir`
//!   `Function::span`, which S4 added for exactly this)
//! - one `DILocation` per statement and per block tail, given to LLVM's builder
//!   so every instruction the statement emits carries it. The spans are the four
//!   the statement emitter writes its `#line` from, and the line and column
//!   are read through the accessor both backends share
//!   (`diag::SourceMap::path_line_col`), so the two cannot name different lines
//!   for one statement
//!
//! The DWARF builder itself is `llvm_ffi::DebugBuilder`, which owns the
//! `unsafe`; this module only says which span belongs to which instruction.
//!
//! Two things are deliberately **not** described, because describing them would
//! mean naming a line that is not theirs: the generated helpers (the per-`datac`
//! drops, the container function sets, the closure machinery and `kelp_drop_pds`)
//! carry no subprogram and so no location, and neither does an `extern fun`
//! declaration, whose body is C compiled into the program's own object. A
//! passdown initializer *is* a `.kp` expression, so the generated
//! `kelp_init_pds` does carry a subprogram and its statements do carry their own
//! lines; the one piece of glue in it, the `atexit` registration, is attributed
//! to the first passdown, since a call inside a function with debug info must
//! have a location and the registration exists for that passdown's drop.
//!
//! Nothing here describes a variable or a type. The unit is a line table: what a
//! reader gets is the file and the line, which is what the exit criterion asks
//! for, and a debugger is told so through the compile unit's `isOptimized` flag
//! (true when a pass pipeline runs, false at `-O0`) and a subroutine type with
//! no parameters.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ast::{AsmOperandKind, BinOp, UnOp};
use crate::cache::{Cache, KeyFields};
use crate::inline_asm::asm_operand_text;
use crate::borrowck::{stmt_id, BindingId, DropPlan, DropTarget, Projection};
use crate::llvm_ffi::{
    self, AtomicRmw, BasicBlock, Builder, Context, DataLayout, DebugBuilder, FloatBinOp, FnType,
    Function as LlvmFunction, IntBinOp, IntCmp, IntExtension, Metadata, Module, RealCmp,
    TargetMachine, Type, Value,
};
use crate::diag::SourceMap;
use crate::lexer::Span;
use crate::opt::OptLevel;
use crate::timings::{Timings, UnitTiming};
use crate::tir::{shift_amount_in_range, shift_width, Block, Expr, ExprKind, Function, Pd, Program, Stmt, TemplatePiece};
use crate::types::{
    box_c_type, capture_field, clone_fn_name, closure_c_type, closure_mangle, cmp_fn_name,
    drop_fn_name, eq_fn_name, hash_fn_name, list_c_type, map_c_type,
    needs_drop, owned_types, struct_tag, ty_mangle, StructDef, Ty,
};

/// Build the whole program, verify it, write the textual IR beside the binary,
/// run the level's pass pipeline, emit the object and link it with `ld.lld`.
///
/// This writes `<out>.ll` (MK7 §4.4) so the IR is inspectable next to the
/// binary, and keeps the object it hands the linker as `<out>.o`. Linking itself
/// is `driver::link_llvm_object`, the counterpart of the linker step in `driver`.
///
/// `opt` is the S6 optimization level (MK7 §4.3). It is one level for two
/// things: the `default<O<n>>` pass pipeline `run_passes` runs over the module,
/// and the codegen level the `TargetMachine` is created with. The textual IR is
/// written **before** the pipeline so the artifact beside the binary is the
/// lowering at every level, which is what the MK7 stage gates assert on; the
/// object is what the pipeline and the codegen level produced.
///
/// `incremental` is the roadmap A-14 slice 1 mode: instead of one module named
/// `kelp`, the program becomes one module per source module plus a `_support`
/// module (`kelp wiki page compiler/incremental.md`), each compiled to its own object
/// under `<out>.units/` and linked together with the runtime and `extern fun`
/// objects. With it off (the default) this function is exactly what it was
/// before the mode existed.
pub fn compile(
    program: &Program,
    drops: &DropPlan,
    sources: &SourceMap,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    freestanding: Option<&Freestanding>,
    managed: Option<&Managed>,
    incremental: bool,
    obfuscate_names: bool,
    timings: &mut Timings,
) -> Result<(), String> {
    if managed.is_some() && incremental {
        return Err("a managed build is not split per unit yet: the collector's descriptors and init are whole-program".to_string());
    }
    if drops.has_regions() && incremental {
        return Err("a build with a block region is not split per unit yet: the block runtime is whole-program".to_string());
    }
    if let Some(fs) = freestanding {
        llvm_ffi::initialize_target_for(&fs.triple)?;
    } else {
        llvm_ffi::initialize_native_target()?;
    }
    let triple = match freestanding {
        Some(fs) => fs.triple.clone(),
        None => llvm_ffi::default_target_triple(),
    };
    if triple.is_empty() {
        return Err("LLVM returned an empty host triple".to_string());
    }

    // The target machine is created before any type is built, because the
    // tagged union's size and alignment are read back from its data layout
    // (`declare_aggregates`). It is the same machine the object is written with
    // and the one the pass pipeline reads its target from, so the layout the
    // types were built for and the layout codegen assumes cannot be two
    // different ones. It carries the level's codegen half; the pipeline below
    // carries the module half. The incremental mode builds one of these per
    // worker instead (`kelp wiki page compiler/incremental.md` slice 3).
    if incremental {
        return compile_units(
            program,
            drops,
            sources,
            &triple,
            out_path,
            opt,
            link_libs,
            library_dirs,
            pkg_config_flags,
            obfuscate_names,
            timings,
        );
    }

    let ctx = Context::new();
    let machine = match freestanding {
        Some(_) => TargetMachine::for_freestanding(&triple, opt.codegen_level())?,
        None => TargetMachine::for_host_at(opt.codegen_level())?,
    };
    let layout = machine.data_layout();

    let scratch = Scratch::for_names(obfuscate_names)?;
    let base = scratch.base(out_path);
    let ir_path = base.with_extension("ll");
    let object_path = base.with_extension("o");
    lower_to_object(
        program,
        drops,
        sources,
        &ctx,
        &machine,
        &layout,
        &triple,
        "kelp",
        &ir_path,
        &object_path,
        opt,
        freestanding,
        None,
        None,
        managed.map(|m| m.heap),
        timings,
    )?;

    // The `extern fun` bodies this program reaches and the capability check's
    // own glue, compiled into their own object (MK7 §3.2, §5). That object is
    // LLVM IR written by the program's own target machine.
    //
    // A freestanding build never compiles it: its trampolines call into the
    // hosted runtime object, which is the whole thing this mode excludes. A user `extern fun` therefore stays a bare declaration here and
    // is the linker's to resolve against whatever the script or the build puts
    // beside this object.
    if let Some(fs) = freestanding {
        let link_start = Instant::now();
        let linked = crate::driver::link_freestanding_object(
            &object_path,
            Path::new(&fs.linker_script),
            out_path,
        );
        timings.link += link_start.elapsed();
        return linked.map_err(|e| e.message);
    }

    let externs_path = base.with_extension("externs.o");
    let program_object = crate::extern_object::lower_externs_object(program, &machine, &externs_path, true)?
        .then_some(externs_path);

    let link_start = Instant::now();
    // A hosted build with a block region links the collector archive the
    // managed build links, so the block entry points resolve.
    let block_archive = match managed {
        None if drops.has_regions() => Some(crate::driver::managed_archive().map_err(|e| e.message)?),
        _ => None,
    };
    let linked = match managed {
        Some(m) => crate::driver::link_managed_object(
            &object_path,
            program_object.as_deref(),
            &m.gc_archive,
            out_path,
            opt,
            link_libs,
            library_dirs,
            pkg_config_flags,
            crate::extern_object::uses_tls(program),
            obfuscate_names,
        ),
        None if block_archive.is_some() => crate::driver::link_managed_object(
            &object_path,
            program_object.as_deref(),
            block_archive.as_ref().expect("the guard checked the archive"),
            out_path,
            opt,
            link_libs,
            library_dirs,
            pkg_config_flags,
            crate::extern_object::uses_tls(program),
            obfuscate_names,
        ),
        None => crate::driver::link_llvm_object(
            &object_path,
            program_object.as_deref(),
            out_path,
            opt,
            link_libs,
            library_dirs,
            pkg_config_flags,
            crate::extern_object::uses_tls(program),
            obfuscate_names,
        ),
    };
    timings.link += link_start.elapsed();
    linked.map_err(|e| e.message)
}

/// The heap settings `kelp_gc_init` receives: the cap, the two start guesses
/// of the first clearing, the headroom percent and the trigger rule's code.
/// Bytes, except the percent and the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapLimits {
    pub max: u64,
    pub start_low: u64,
    pub start_high: u64,
    pub headroom_pct: u64,
    pub trigger_rule: u64,
}

impl HeapLimits {
    /// The limits a target asks for: `[target] heap_max` and the `[tidepool]`
    /// section, with the defaults where the manifest names none.
    pub fn from_target(target: &crate::resolve::TargetDecl) -> Self {
        let tidepool = target.tidepool();
        HeapLimits {
            max: target.heap_max_mib() << 20,
            start_low: tidepool.start_low_mib_or_default() << 20,
            start_high: tidepool.start_high_mib_or_default() << 20,
            headroom_pct: tidepool.headroom_pct_or_default(),
            trigger_rule: tidepool.trigger_rule_or_default().code(),
        }
    }
}

/// A managed build's settings (WP2): the heap limits `kelp_gc_init` receives,
/// and the collector archive the link takes.
#[derive(Debug, Clone)]
pub struct Managed {
    pub heap: HeapLimits,
    pub gc_archive: crate::driver::CollectorArchive,
}

/// Lower a whole program and return the IR text, without linking, in the hosted
/// (`managed = None`) or managed (`Some(heap cap in bytes)`) form. The object is written
/// beside the IR, as every lowering does, so the tests can check that the IR
/// also verifies and emits. Only the tests call this: a real build goes through
/// `compile`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn compile_ir(
    program: &Program,
    drops: &DropPlan,
    sources: &SourceMap,
    managed: Option<HeapLimits>,
    ir_path: &Path,
    object_path: &Path,
) -> Result<String, String> {
    llvm_ffi::initialize_native_target()?;
    let triple = llvm_ffi::default_target_triple();
    let ctx = Context::new();
    let machine = TargetMachine::for_host_at(OptLevel::O0.codegen_level())?;
    let layout = machine.data_layout();
    let mut timings = Timings::new();
    lower_to_object(
        program,
        drops,
        sources,
        &ctx,
        &machine,
        &layout,
        &triple,
        "kelp",
        ir_path,
        object_path,
        OptLevel::O0,
        None,
        None,
        None,
        managed,
        &mut timings,
    )?;
    std::fs::read_to_string(ir_path)
        .map_err(|e| format!("could not read generated IR '{}': {e}", ir_path.display()))
}


#[cfg(test)]
#[path = "codegen_llvm/jobs_tests.rs"]
mod jobs_tests;

mod fn_state;
pub(crate) use fn_state::*;

mod libc_decls;
pub(crate) use libc_decls::*;

mod runtime_helpers;

mod container_types;

mod driver;
pub(crate) use driver::*;

mod units;
pub(crate) use units::*;

mod instantiations;
pub(crate) use instantiations::*;

mod closure_decls;
pub(crate) use closure_decls::*;

mod bodies;
pub(crate) use bodies::*;

mod layout;
pub(crate) use layout::*;

mod util;
pub(crate) use util::*;

#[cfg(test)]
#[path = "codegen_llvm/layout_tests.rs"]
mod layout_tests;

mod calls;
mod sync;
mod sync_calls;

mod container_specs;

mod ops_asm;
mod places;
mod expr;
mod frame;
mod declare_items;
mod declare;
